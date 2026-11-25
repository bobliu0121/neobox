//! `.nbmusic` 容器格式（v1.0）后端实现。
//!
//! 物理层是 Matroska/EBML 的受控 profile：
//! - 音频轨：A_AAC / A_MPEG/L3 / A_FLAC / A_PCM
//! - 字幕轨：`S_TEXT/NBMUSIC`，CodecPrivate 存轨道级 JSON，Block 存单条 Cue
//! - 封面 / 字体：Matroska Attachments
//! - 高潮标记：`NBMUSIC_CLIMAX` Tag
//!
//! v1 先实现「单文件多声道 + 多字幕轨 + 封面 + 字体 + 高潮」的读写与播放提取；
//! Cues/SeekHead 暂不写入（播放链路会先解封装到缓存文件，不依赖容器内 seek 索引）。

use crate::matroska::{
    parse_block_payload, parse_subtitle_import_events, prepare_audio, read_float, read_uint,
    read_vint_id, read_vint_size,
};
use image::ImageEncoder;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ─────────────────────────── 常量 ───────────────────────────

const ID_EBML: &[u8] = &[0x1A, 0x45, 0xDF, 0xA3];
const ID_EBML_VERSION: &[u8] = &[0x42, 0x86];
const ID_EBML_READ_VERSION: &[u8] = &[0x42, 0xF7];
const ID_EBML_MAX_ID_LENGTH: &[u8] = &[0x42, 0xF2];
const ID_EBML_MAX_SIZE_LENGTH: &[u8] = &[0x42, 0xF3];
const ID_DOC_TYPE: &[u8] = &[0x42, 0x82];
const ID_DOC_TYPE_VERSION: &[u8] = &[0x42, 0x87];
const ID_DOC_TYPE_READ_VERSION: &[u8] = &[0x42, 0x85];

const ID_SEGMENT: &[u8] = &[0x18, 0x53, 0x80, 0x67];
const ID_INFO: &[u8] = &[0x15, 0x49, 0xA9, 0x66];
const ID_TIMECODE_SCALE: &[u8] = &[0x2A, 0xD7, 0xB1];
const ID_DURATION: &[u8] = &[0x44, 0x89];
const ID_MUXING_APP: &[u8] = &[0x4D, 0x80];
const ID_WRITING_APP: &[u8] = &[0x57, 0x41];
const ID_TITLE: &[u8] = &[0x7B, 0xA9];

const ID_TRACKS: &[u8] = &[0x16, 0x54, 0xAE, 0x6B];
const ID_TRACK_ENTRY: &[u8] = &[0xAE];
const ID_TRACK_NUMBER: &[u8] = &[0xD7];
const ID_TRACK_UID: &[u8] = &[0x73, 0xC5];
const ID_TRACK_TYPE: &[u8] = &[0x83];
const ID_FLAG_LACING: &[u8] = &[0x9C];
const ID_FLAG_DEFAULT: &[u8] = &[0x88];
const ID_CODEC_ID: &[u8] = &[0x86];
const ID_CODEC_PRIVATE: &[u8] = &[0x63, 0xA2];
const ID_TRACK_NAME: &[u8] = &[0x53, 0x6E];
const ID_LANGUAGE: &[u8] = &[0x22, 0xB5, 0x9C];
const ID_DEFAULT_DURATION: &[u8] = &[0x23, 0xE3, 0x83];
const ID_AUDIO: &[u8] = &[0xE1];
const ID_SAMPLING_FREQUENCY: &[u8] = &[0xB5];
const ID_CHANNELS: &[u8] = &[0x9F];

const ID_CLUSTER: &[u8] = &[0x1F, 0x43, 0xB6, 0x75];
const ID_TIMECODE: &[u8] = &[0xE7];
const ID_SIMPLE_BLOCK: &[u8] = &[0xA3];
const ID_BLOCK_GROUP: &[u8] = &[0xA0];
const ID_BLOCK: &[u8] = &[0xA1];
const ID_BLOCK_DURATION: &[u8] = &[0x9B];

const ID_ATTACHMENTS: &[u8] = &[0x19, 0x41, 0xA4, 0x69];
const ID_ATTACHED_FILE: &[u8] = &[0x61, 0xA7];
const ID_FILE_DESCRIPTION: &[u8] = &[0x46, 0x7E];
const ID_FILE_NAME: &[u8] = &[0x46, 0x6E];
const ID_FILE_MIME_TYPE: &[u8] = &[0x46, 0x60];
const ID_FILE_DATA: &[u8] = &[0x46, 0x5C];
const ID_FILE_UID: &[u8] = &[0x46, 0xAE];

const ID_TAGS: &[u8] = &[0x12, 0x54, 0xC3, 0x67];
const ID_TAG: &[u8] = &[0x73, 0x73];
const ID_TARGETS: &[u8] = &[0x63, 0xC0];
const ID_TARGET_TRACK_UID: &[u8] = &[0x63, 0xC5];
const ID_SIMPLE_TAG: &[u8] = &[0x67, 0xC8];
const ID_TAG_NAME: &[u8] = &[0x45, 0xA3];
const ID_TAG_STRING: &[u8] = &[0x44, 0x87];

const TRACK_TYPE_AUDIO: u64 = 2;
const TRACK_TYPE_SUBTITLE: u64 = 17;
const TIMECODE_SCALE_NS: u64 = 1_000_000;
const MAX_CLUSTER_MS: u64 = 30_000;
const MAX_FONT_BYTES: usize = 50 * 1024 * 1024;
const MAX_TOTAL_FONT_BYTES: usize = 100 * 1024 * 1024;
const MAX_COVER_BYTES: usize = 20 * 1024 * 1024;
const MAX_CUE_TEXT_BYTES: usize = 64 * 1024;
const MAX_CODEC_PRIVATE_BYTES: usize = 1024 * 1024;

// ─────────────────────────── 公共数据模型 ───────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AudioBinding {
    pub track_uid: String,
    pub channels: Option<Vec<u8>>,
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChannelProgram {
    pub channels: Vec<u8>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct InlineTranslation {
    pub lang: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProgressPoint {
    pub at_ms: u64,
    pub ratio: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CharSpan {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub mode: Option<String>,
    pub reading: Option<String>,
    pub progress: Option<Vec<ProgressPoint>>,
    pub confidence: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PronunciationSegment {
    pub source: String,
    pub reading: String,
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Pronunciation {
    pub scheme: String,
    pub lang: Option<String>,
    pub text: String,
    pub segments: Option<Vec<PronunciationSegment>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CueStyle {
    pub font_family: Option<String>,
    pub font_attachment_uid: Option<String>,
    pub font_weight: Option<u16>,
    pub font_style: Option<String>,
    pub font_size_px: Option<u32>,
    pub color: Option<String>,
    pub align: Option<String>,
    pub position: Option<String>,
    pub margin_px: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Cue {
    pub id: Option<String>,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    #[serde(default)]
    pub chars: Vec<CharSpan>,
    #[serde(default)]
    pub pronunciations: Vec<Pronunciation>,
    #[serde(default)]
    pub translations: Vec<InlineTranslation>,
    pub style: Option<CueStyle>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ClimaxMarker {
    pub id: Option<String>,
    pub track_uid: Option<String>,
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    pub peak_ms: Option<u64>,
    pub kind: Option<String>,
    pub label: Option<String>,
    pub strength: Option<u8>,
    pub source: Option<String>,
    pub confidence: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FontAttachment {
    pub attachment_uid: String,
    pub file_name: String,
    pub mime: String,
    pub family: String,
    pub style: Option<String>,
    pub weight: Option<u16>,
    pub license: Option<String>,
    pub subset: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NbmusicAudioInput {
    pub path: String,
    /// 编辑已有工程时保留原 UID；新建工程留空，由封装器按顺序分配。
    pub uid: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub default: Option<bool>,
    /// 双声道工程中的声道角色：left / right；缺省时按轨道顺序推断。
    pub dual_role: Option<String>,
    #[serde(default)]
    pub channel_programs: Vec<ChannelProgram>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FontInput {
    pub path: String,
    pub family: String,
    pub style: Option<String>,
    pub weight: Option<u16>,
    pub license: Option<String>,
    pub subset: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleTrackInput {
    pub uid: Option<String>,
    pub language: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub audio_bindings: Vec<AudioBinding>,
    pub pronunciation_scheme: Option<String>,
    pub source_subtitle_uid: Option<String>,
    pub title: Option<String>,
    pub default: Option<bool>,
    pub forced: Option<bool>,
    pub offset_ms: Option<i64>,
    /// 双声道工程中的字幕声道：left / right；缺省时按字幕轨顺序推断。
    pub dual_side: Option<String>,
    #[serde(default)]
    pub cues: Vec<Cue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NbmusicMuxOptions {
    pub output_path: String,
    /// 编辑已有工程时沿用原 NBMUSIC_ID，避免每保存一次就换一个身份。
    pub id: Option<String>,
    #[serde(default)]
    pub audio_tracks: Vec<NbmusicAudioInput>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub cover_path: Option<String>,
    #[serde(default)]
    pub subtitle_tracks: Vec<SubtitleTrackInput>,
    #[serde(default)]
    pub fonts: Vec<FontInput>,
    #[serde(default)]
    pub climaxes: Vec<ClimaxMarker>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NbmusicMuxResult {
    pub output_path: String,
    pub duration_ms: u64,
    pub audio_track_count: usize,
    pub subtitle_track_count: usize,
    pub has_cover: bool,
    pub font_count: usize,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NbmusicAudioTrackInfo {
    pub uid: String,
    pub codec: String,
    pub sample_rate: f64,
    pub channels: u64,
    pub language: String,
    pub title: String,
    pub default: bool,
    pub dual_role: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NbmusicSubtitleTrackInfo {
    pub uid: String,
    pub language: String,
    pub roles: Vec<String>,
    pub audio_bindings: Vec<AudioBinding>,
    pub pronunciation_scheme: Option<String>,
    pub source_subtitle_uid: Option<String>,
    pub title: String,
    pub default: bool,
    pub forced: bool,
    pub offset_ms: Option<i64>,
    pub dual_side: Option<String>,
    pub cue_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NbmusicProbe {
    pub spec: String,
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub audio_tracks: Vec<NbmusicAudioTrackInfo>,
    pub subtitle_tracks: Vec<NbmusicSubtitleTrackInfo>,
    pub has_cover: bool,
    pub fonts: Vec<FontAttachment>,
    pub climaxes: Vec<ClimaxMarker>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NbmusicProjectAudio {
    /// 解封装到临时目录的音频路径，可直接作为下一次 mux 的输入。
    pub path: String,
    pub uid: String,
    pub codec: String,
    pub sample_rate: f64,
    pub channels: u64,
    pub language: String,
    pub title: String,
    pub default: bool,
    pub dual_role: Option<String>,
    #[serde(default)]
    pub channel_programs: Vec<ChannelProgram>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NbmusicProjectSubtitle {
    pub uid: String,
    pub language: String,
    pub roles: Vec<String>,
    pub audio_bindings: Vec<AudioBinding>,
    pub pronunciation_scheme: Option<String>,
    pub source_subtitle_uid: Option<String>,
    pub title: String,
    pub default: bool,
    pub forced: bool,
    pub offset_ms: Option<i64>,
    pub dual_side: Option<String>,
    pub cues: Vec<Cue>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NbmusicProjectFont {
    /// 解封装到临时目录的字体路径。
    pub path: String,
    pub file_name: String,
    pub mime: String,
    pub family: String,
    pub style: Option<String>,
    pub weight: Option<u16>,
    pub license: Option<String>,
    pub subset: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NbmusicProject {
    pub source_path: String,
    pub spec: String,
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub cover_path: Option<String>,
    pub audio_tracks: Vec<NbmusicProjectAudio>,
    pub subtitle_tracks: Vec<NbmusicProjectSubtitle>,
    pub fonts: Vec<NbmusicProjectFont>,
    pub climaxes: Vec<ClimaxMarker>,
}

// ─────────────────────────── 内部类型 ───────────────────────────

struct PreparedAudio {
    number: u64,
    uid: u64,
    name: String,
    language: String,
    default: bool,
    dual_role: Option<String>,
    audio: crate::matroska::MuxAudio,
    channel_programs: Vec<ChannelProgram>,
}

#[derive(Debug)]
struct PreparedSubtitle {
    number: u64,
    uid: u64,
    name: String,
    language: String,
    default: bool,
    forced: bool,
    offset_ms: Option<i64>,
    dual_side: Option<String>,
    codec_private: Vec<u8>,
    cues: Vec<Cue>,
    bindings: Vec<AudioBinding>,
    roles: Vec<String>,
    source_subtitle_uid: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubtitleCodecPrivate<'a> {
    v: u8,
    uid: &'a str,
    audio_bindings: &'a [AudioBinding],
    roles: &'a [String],
    language: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pronunciation_scheme: Option<&'a str>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SubtitleCodecMeta {
    #[serde(default)]
    v: u8,
    #[serde(default)]
    uid: String,
    #[serde(default)]
    audio_bindings: Vec<AudioBinding>,
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default)]
    language: String,
    #[serde(default)]
    pronunciation_scheme: Option<String>,
}

#[derive(Debug)]
pub struct DemuxAudioTrack {
    pub uid: String,
    pub codec_id: String,
    pub codec_private: Vec<u8>,
    pub sample_rate: f64,
    pub channels: u64,
    pub bit_depth: u64,
    pub language: String,
    pub title: String,
    pub default: bool,
    pub dual_role: Option<String>,
    pub channel_programs: Vec<ChannelProgram>,
    pub frames: Vec<(u64, Vec<u8>)>,
}

#[derive(Debug)]
pub struct DemuxSubtitleTrack {
    pub uid: String,
    pub language: String,
    pub roles: Vec<String>,
    pub audio_bindings: Vec<AudioBinding>,
    pub pronunciation_scheme: Option<String>,
    pub source_subtitle_uid: Option<String>,
    pub title: String,
    pub default: bool,
    pub forced: bool,
    pub offset_ms: Option<i64>,
    pub dual_side: Option<String>,
    pub cues: Vec<Cue>,
}

#[derive(Debug, Clone)]
pub struct FoundFont {
    pub metadata: FontAttachment,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct NbmusicDemux {
    pub spec: String,
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub audio_tracks: Vec<DemuxAudioTrack>,
    pub subtitle_tracks: Vec<DemuxSubtitleTrack>,
    pub cover: Option<(String, Vec<u8>)>,
    pub fonts: Vec<FoundFont>,
    pub climaxes: Vec<ClimaxMarker>,
}

#[derive(Debug)]
struct ParsedTrack {
    number: u64,
    uid: u64,
    track_type: u64,
    codec_id: String,
    codec_private: Vec<u8>,
    name: String,
    language: String,
    default: bool,
    sample_rate: f64,
    channels: u64,
    bit_depth: u64,
}

#[derive(Debug)]
struct RawBlock {
    track: u64,
    ts: u64,
    data: Vec<u8>,
}

#[derive(Debug)]
struct AttachmentFile {
    uid: u64,
    name: String,
    mime: String,
    data: Vec<u8>,
}

// ─────────────────────────── EBML 写入工具 ───────────────────────────

fn write_size(out: &mut Vec<u8>, size: usize) {
    let mut len = 1usize;
    while len < 8 {
        let max = (1u64 << (7 * len)) - 1;
        if (size as u64) < max {
            break;
        }
        len += 1;
    }
    let mut buf = [0u8; 8];
    let mut value = size as u64;
    for i in (0..len).rev() {
        buf[i] = (value & 0xFF) as u8;
        value >>= 8;
    }
    buf[0] |= 1u8 << (8 - len);
    out.extend_from_slice(&buf[..len]);
}

fn elem(id: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(id.len() + 8 + payload.len());
    out.extend_from_slice(id);
    write_size(&mut out, payload.len());
    out.extend_from_slice(payload);
    out
}

fn uint_payload(value: u64) -> Vec<u8> {
    if value == 0 {
        return vec![0];
    }
    let bytes = value.to_be_bytes();
    let start = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len() - 1);
    bytes[start..].to_vec()
}

fn uint_elem(id: &[u8], value: u64) -> Vec<u8> {
    elem(id, &uint_payload(value))
}

fn float_elem(id: &[u8], value: f64) -> Vec<u8> {
    elem(id, &value.to_be_bytes())
}

fn str_elem(id: &[u8], value: &str) -> Vec<u8> {
    elem(id, value.as_bytes())
}

fn concat(parts: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(parts.iter().map(Vec::len).sum());
    for part in parts {
        out.extend_from_slice(part);
    }
    out
}

fn write_vint(out: &mut Vec<u8>, value: u64) {
    let mut len = 1usize;
    while len < 8 && value >= (1u64 << (7 * len)) - 1 {
        len += 1;
    }
    let mut buf = [0u8; 8];
    let mut v = value;
    for i in (0..len).rev() {
        buf[i] = (v & 0xFF) as u8;
        v >>= 8;
    }
    buf[0] |= 1u8 << (8 - len);
    out.extend_from_slice(&buf[..len]);
}

fn build_block(
    track: u64,
    rel: i16,
    data: &[u8],
    keyframe: bool,
    duration: Option<u64>,
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(4 + data.len());
    write_vint(&mut payload, track);
    payload.extend_from_slice(&rel.to_be_bytes());
    payload.push(if keyframe { 0x80 } else { 0x00 });
    payload.extend_from_slice(data);
    if let Some(duration) = duration {
        let block = elem(ID_BLOCK, &payload);
        let duration_elem = uint_elem(ID_BLOCK_DURATION, duration.max(1));
        elem(ID_BLOCK_GROUP, &concat(&[block, duration_elem]))
    } else {
        elem(ID_SIMPLE_BLOCK, &payload)
    }
}

fn build_track_entry(
    number: u64,
    uid: u64,
    track_type: u64,
    codec_id: &str,
    codec_private: &[u8],
    name: &str,
    language: &str,
    default: bool,
    audio: Option<(f64, u64)>,
    default_duration_ns: Option<u64>,
) -> Vec<u8> {
    let mut fields = vec![
        uint_elem(ID_TRACK_NUMBER, number),
        uint_elem(ID_TRACK_UID, uid),
        uint_elem(ID_TRACK_TYPE, track_type),
        uint_elem(ID_FLAG_LACING, 0),
        uint_elem(ID_FLAG_DEFAULT, if default { 1 } else { 0 }),
        str_elem(ID_CODEC_ID, codec_id),
        str_elem(ID_LANGUAGE, if language.is_empty() { "und" } else { language }),
        str_elem(ID_TRACK_NAME, if name.is_empty() { "Track" } else { name }),
    ];
    if !codec_private.is_empty() {
        fields.push(elem(ID_CODEC_PRIVATE, codec_private));
    }
    if let Some(duration) = default_duration_ns {
        fields.push(uint_elem(ID_DEFAULT_DURATION, duration.max(1)));
    }
    if let Some((sample_rate, channels)) = audio {
        let audio_fields = vec![
            float_elem(ID_SAMPLING_FREQUENCY, sample_rate),
            uint_elem(ID_CHANNELS, channels.max(1)),
        ];
        fields.push(elem(ID_AUDIO, &concat(&audio_fields)));
    }
    elem(ID_TRACK_ENTRY, &concat(&fields))
}

fn normalize_dual_side(value: Option<&str>) -> Option<String> {
    match value.map(|item| item.trim().to_ascii_lowercase()) {
        Some(side) if side == "left" || side == "right" => Some(side),
        _ => None,
    }
}

fn build_simple_tag(name: &str, value: &str) -> Vec<u8> {
    elem(
        ID_SIMPLE_TAG,
        &concat(&[str_elem(ID_TAG_NAME, name), str_elem(ID_TAG_STRING, value)]),
    )
}

fn build_tag(target_uid: Option<u64>, simple_tags: Vec<Vec<u8>>) -> Vec<u8> {
    let mut fields = Vec::new();
    if let Some(uid) = target_uid {
        fields.push(elem(ID_TARGETS, &uint_elem(ID_TARGET_TRACK_UID, uid)));
    }
    fields.extend(simple_tags);
    elem(ID_TAG, &concat(&fields))
}

fn build_attachment(uid: u64, name: &str, mime: &str, data: &[u8], description: &str) -> Vec<u8> {
    elem(
        ID_ATTACHED_FILE,
        &concat(&[
            str_elem(ID_FILE_DESCRIPTION, description),
            str_elem(ID_FILE_NAME, name),
            str_elem(ID_FILE_MIME_TYPE, mime),
            uint_elem(ID_FILE_UID, uid),
            elem(ID_FILE_DATA, data),
        ]),
    )
}

// ─────────────────────────── 图片 / 字体识别 ───────────────────────────

fn detect_image(data: &[u8]) -> Option<(&'static str, &'static str)> {
    if data.len() >= 8 && data[..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        Some(("image/png", "png"))
    } else if data.len() >= 3 && data[..3] == [0xFF, 0xD8, 0xFF] {
        Some(("image/jpeg", "jpg"))
    } else if data.len() >= 6 && (&data[..6] == b"GIF87a" || &data[..6] == b"GIF89a") {
        Some(("image/gif", "gif"))
    } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some(("image/webp", "webp"))
    } else {
        None
    }
}

fn font_mime(path: &Path, data: &[u8]) -> Option<&'static str> {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let declared = match ext.as_str() {
        "ttf" => Some("font/ttf"),
        "otf" => Some("font/otf"),
        "woff" => Some("font/woff"),
        "woff2" => Some("font/woff2"),
        _ => None,
    };
    let actual = if data.starts_with(b"OTTO") {
        Some("font/otf")
    } else if data.starts_with(b"wOFF") {
        Some("font/woff")
    } else if data.starts_with(b"wOF2") {
        Some("font/woff2")
    } else if data.starts_with(&[0x00, 0x01, 0x00, 0x00]) || data.starts_with(b"true") {
        Some("font/ttf")
    } else {
        None
    };
    match (declared, actual) {
        (Some(declared), Some(actual)) if declared == actual => Some(actual),
        (None, Some(actual)) => Some(actual),
        _ => None,
    }
}

fn safe_file_name(name: &str, fallback: &str) -> String {
    Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && !value.contains(".."))
        .unwrap_or_else(|| fallback.to_string())
}

fn channel_layout_for(channels: u64) -> &'static str {
    match channels {
        1 => "mono",
        2 => "stereo",
        6 => "5.1",
        8 => "7.1",
        _ => "x-multichannel",
    }
}

// ─────────────────────────── 封装 ───────────────────────────

pub fn mux_nbmusic_file(options: &NbmusicMuxOptions) -> Result<NbmusicMuxResult, String> {
    if options.audio_tracks.is_empty() {
        return Err("至少需要一条音频轨".to_string());
    }
    if options.output_path.trim().is_empty() {
        return Err("输出路径不能为空".to_string());
    }

    let mut prepared_audio: Vec<PreparedAudio> = Vec::new();
    let mut used_uids: Vec<u64> = Vec::new();
    for (index, input) in options.audio_tracks.iter().enumerate() {
        if input.path.trim().is_empty() {
            return Err(format!("第 {} 条音频轨路径为空", index + 1));
        }
        let audio = prepare_audio(&input.path)?;
        if audio.frames.is_empty() {
            return Err(format!("音频轨 {} 没有可用帧", input.path));
        }
        let number = (index + 1) as u64;
        let uid = match input.uid.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => {
                let parsed = value
                    .parse::<u64>()
                    .map_err(|_| format!("音频轨 UID 非法：{value}"))?;
                if parsed == 0 {
                    return Err("音频轨 UID 不能为 0".to_string());
                }
                parsed
            }
            None => number,
        };
        if used_uids.contains(&uid) {
            return Err(format!("音频轨 UID 重复：{uid}"));
        }
        used_uids.push(uid);
        let name = input
            .title
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("Audio {}", index + 1));
        prepared_audio.push(PreparedAudio {
            number,
            uid,
            name,
            language: input.language.clone().unwrap_or_else(|| "und".to_string()),
            default: input.default.unwrap_or(index == 0),
            dual_role: normalize_dual_side(input.dual_role.as_deref()),
            audio,
            channel_programs: input.channel_programs.clone(),
        });
    }

    let audio_uids: Vec<String> = prepared_audio.iter().map(|track| track.uid.to_string()).collect();
    let mut prepared_subtitles: Vec<PreparedSubtitle> = Vec::new();
    for (index, input) in options.subtitle_tracks.iter().enumerate() {
        if input.cues.is_empty() {
            continue;
        }
        for cue in &input.cues {
            if cue.end_ms <= cue.start_ms {
                return Err(format!("字幕 Cue 时间非法：{} -> {}", cue.start_ms, cue.end_ms));
            }
            if cue.text.as_bytes().len() > MAX_CUE_TEXT_BYTES {
                return Err("单条 Cue 文本超过 64 KB".to_string());
            }
        }

        let bindings = if input.audio_bindings.is_empty() {
            prepared_audio
                .iter()
                .map(|track| AudioBinding {
                    track_uid: track.uid.to_string(),
                    channels: None,
                    labels: None,
                })
                .collect::<Vec<_>>()
        } else {
            input.audio_bindings.clone()
        };
        for binding in &bindings {
            if !audio_uids.iter().any(|uid| uid == &binding.track_uid) {
                return Err(format!("字幕绑定了不存在的音频轨 UID：{}", binding.track_uid));
            }
            if let Some(channels) = &binding.channels {
                let audio = prepared_audio
                    .iter()
                    .find(|track| track.uid.to_string() == binding.track_uid)
                    .ok_or_else(|| format!("字幕绑定了不存在的音频轨 UID：{}", binding.track_uid))?;
                if channels.is_empty()
                    || channels.iter().any(|channel| *channel == 0 || *channel as u64 > audio.audio.channels)
                {
                    return Err(format!(
                        "字幕声道选择越界：track {} channels {:?}（该轨共 {} 声道）",
                        binding.track_uid, channels, audio.audio.channels
                    ));
                }
            }
        }

        let mut roles = input.roles.clone();
        if roles.is_empty() {
            roles.push("original".to_string());
        }
        let number = (prepared_audio.len() + index + 1) as u64;
        let uid = match input.uid.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => {
                let parsed = value
                    .parse::<u64>()
                    .map_err(|_| format!("字幕轨 UID 非法：{value}"))?;
                if parsed == 0 {
                    return Err("字幕轨 UID 不能为 0".to_string());
                }
                parsed
            }
            None => number,
        };
        if used_uids.contains(&uid) {
            return Err(format!("字幕轨 UID 与其它轨道重复：{uid}"));
        }
        used_uids.push(uid);
        let name = input
            .title
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("Subtitle {}", index + 1));
        let language = if input.language.trim().is_empty() {
            "und".to_string()
        } else {
            input.language.clone()
        };
        let uid_text = uid.to_string();
        let private = SubtitleCodecPrivate {
            v: 1,
            uid: &uid_text,
            audio_bindings: &bindings,
            roles: &roles,
            language: &language,
            pronunciation_scheme: input.pronunciation_scheme.as_deref(),
        };
        let codec_private = serde_json::to_vec(&private)
            .map_err(|e| format!("序列化字幕轨头失败: {e}"))?;
        if codec_private.len() > MAX_CODEC_PRIVATE_BYTES {
            return Err("字幕轨 CodecPrivate 超过 1 MB".to_string());
        }

        let mut cues = input.cues.clone();
        for (cue_index, cue) in cues.iter_mut().enumerate() {
            if cue.id.is_none() {
                cue.id = Some(format!("c{}", cue_index + 1));
            }
        }

        prepared_subtitles.push(PreparedSubtitle {
            number,
            uid,
            name,
            language,
            default: input.default.unwrap_or(false),
            forced: input.forced.unwrap_or(false),
            offset_ms: input.offset_ms,
            dual_side: normalize_dual_side(input.dual_side.as_deref()),
            codec_private,
            cues,
            bindings,
            roles,
            source_subtitle_uid: input.source_subtitle_uid.clone(),
        });
    }

    // 封面与字体附件
    let mut attachment_blocks: Vec<Vec<u8>> = Vec::new();
    let mut has_cover = false;
    let mut font_attachments: Vec<FontAttachment> = Vec::new();
    let mut next_attachment_uid = 1000u64;

    if let Some(path) = options.cover_path.as_deref().filter(|value| !value.trim().is_empty()) {
        let data = std::fs::read(path).map_err(|e| format!("读取封面失败: {e}"))?;
        if data.len() > MAX_COVER_BYTES {
            return Err("封面超过 20 MB".to_string());
        }
        let Some((mime, ext)) = detect_image(&data) else {
            return Err("封面仅支持 PNG / JPEG / GIF / WebP".to_string());
        };
        let uid = next_attachment_uid;
        next_attachment_uid += 1;
        let name = format!("cover-front.{ext}");
        attachment_blocks.push(build_attachment(uid, &name, mime, &data, "Cover"));
        has_cover = true;
    }

    let mut total_font_bytes = 0usize;
    for (index, font) in options.fonts.iter().enumerate() {
        let path = Path::new(&font.path);
        if !path.is_file() {
            return Err(format!("字体不存在：{}", font.path));
        }
        let data = std::fs::read(path).map_err(|e| format!("读取字体失败: {e}"))?;
        if data.len() > MAX_FONT_BYTES {
            return Err(format!("字体超过 50 MB：{}", font.path));
        }
        total_font_bytes += data.len();
        if total_font_bytes > MAX_TOTAL_FONT_BYTES {
            return Err("内嵌字体总量超过 100 MB".to_string());
        }
        let Some(mime) = font_mime(path, &data) else {
            return Err(format!("不支持的字体格式：{}", font.path));
        };
        let uid = next_attachment_uid;
        next_attachment_uid += 1;
        let fallback = format!("font-{}.{}", index + 1, mime.rsplit('/').next().unwrap_or("bin"));
        let name = safe_file_name(
            path.file_name().and_then(|value| value.to_str()).unwrap_or_default(),
            &fallback,
        );
        attachment_blocks.push(build_attachment(uid, &name, mime, &data, "Font"));
        font_attachments.push(FontAttachment {
            attachment_uid: uid.to_string(),
            file_name: name,
            mime: mime.to_string(),
            family: font.family.clone(),
            style: font.style.clone(),
            weight: font.weight,
            license: font.license.clone(),
            subset: font.subset.clone(),
            sha256: None,
        });
    }

    // 全局标签
    let project_id = options
        .id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mut global_tags: Vec<Vec<u8>> = vec![
        build_simple_tag("NBMUSIC_SPEC", "1.0"),
        build_simple_tag("NBMUSIC_ID", &project_id),
    ];
    if let Some(title) = options.title.as_deref().filter(|value| !value.trim().is_empty()) {
        global_tags.push(build_simple_tag("TITLE", title));
    }
    if let Some(artist) = options.artist.as_deref().filter(|value| !value.trim().is_empty()) {
        global_tags.push(build_simple_tag("ARTIST", artist));
    }
    if let Some(album) = options.album.as_deref().filter(|value| !value.trim().is_empty()) {
        global_tags.push(build_simple_tag("ALBUM", album));
    }
    if !font_attachments.is_empty() {
        let json = serde_json::to_string(&font_attachments)
            .map_err(|e| format!("序列化字体清单失败: {e}"))?;
        global_tags.push(build_simple_tag("NBMUSIC_FONTS", &json));
    }
    if !options.climaxes.is_empty() {
        let json = serde_json::to_string(&options.climaxes)
            .map_err(|e| format!("序列化高潮标记失败: {e}"))?;
        global_tags.push(build_simple_tag("NBMUSIC_CLIMAX", &json));
    }

    let mut tag_blocks: Vec<Vec<u8>> = vec![build_tag(None, global_tags)];

    for track in &prepared_audio {
        let mut simple = vec![build_simple_tag("NBMUSIC_CHANNEL_LAYOUT", channel_layout_for(track.audio.channels))];
        if !track.channel_programs.is_empty() {
            let json = serde_json::to_string(&track.channel_programs)
                .map_err(|e| format!("序列化声道信息失败: {e}"))?;
            simple.push(build_simple_tag("NBMUSIC_CHANNEL_PROGRAMS", &json));
        }
        if let Some(role) = track.dual_role.as_deref() {
            simple.push(build_simple_tag("NBMUSIC_DUAL_ROLE", role));
        }
        tag_blocks.push(build_tag(Some(track.uid), simple));
    }

    for subtitle in &prepared_subtitles {
        let mut simple = vec![
            build_simple_tag(
                "NBMUSIC_AUDIO_BINDINGS",
                &serde_json::to_string(&subtitle.bindings)
                    .map_err(|e| format!("序列化字幕绑定失败: {e}"))?,
            ),
            build_simple_tag(
                "NBMUSIC_SUBTITLE_ROLES",
                &serde_json::to_string(&subtitle.roles)
                    .map_err(|e| format!("序列化字幕角色失败: {e}"))?,
            ),
        ];
        if let Some(source) = subtitle.source_subtitle_uid.as_deref() {
            simple.push(build_simple_tag("NBMUSIC_SOURCE_SUBTITLE", source));
        }
        if let Some(offset) = subtitle.offset_ms {
            simple.push(build_simple_tag("NBMUSIC_LYRICS_OFFSET_MS", &offset.to_string()));
        }
        if subtitle.forced {
            simple.push(build_simple_tag("NBMUSIC_SUBTITLE_FORCED", "1"));
        }
        if let Some(side) = subtitle.dual_side.as_deref() {
            simple.push(build_simple_tag("NBMUSIC_DUAL_SIDE", side));
        }
        tag_blocks.push(build_tag(Some(subtitle.uid), simple));
    }

    let tags = elem(ID_TAGS, &concat(&tag_blocks));
    let attachments = if attachment_blocks.is_empty() {
        Vec::new()
    } else {
        elem(ID_ATTACHMENTS, &concat(&attachment_blocks))
    };

    // 音频与字幕块按时间合并
    enum PendingBlock {
        Audio { track: u64, frame: usize, ts: u64 },
        Subtitle { track: u64, cue: usize, ts: u64, dur: u64 },
    }

    let mut blocks: Vec<PendingBlock> = Vec::new();
    let mut max_end_ms = 0u64;
    for track in &prepared_audio {
        for (frame_index, frame) in track.audio.frames.iter().enumerate() {
            blocks.push(PendingBlock::Audio {
                track: track.number,
                frame: frame_index,
                ts: frame.ts_ms,
            });
            max_end_ms = max_end_ms.max(frame.ts_ms.saturating_add(frame.dur_ms));
        }
    }
    for subtitle in &prepared_subtitles {
        for (cue_index, cue) in subtitle.cues.iter().enumerate() {
            blocks.push(PendingBlock::Subtitle {
                track: subtitle.number,
                cue: cue_index,
                ts: cue.start_ms,
                dur: cue.end_ms.saturating_sub(cue.start_ms),
            });
            max_end_ms = max_end_ms.max(cue.end_ms);
        }
    }
    blocks.sort_by_key(|block| match block {
        PendingBlock::Audio { ts, .. } => (*ts, 0u8),
        PendingBlock::Subtitle { ts, .. } => (*ts, 1u8),
    });

    let mut cluster_payload = Vec::new();
    let mut clusters: Vec<u8> = Vec::new();
    let mut cluster_base: Option<u64> = None;
    let mut cluster_count = 0usize;

    let flush_cluster = |base: u64,
                         payload: &mut Vec<u8>,
                         clusters: &mut Vec<u8>,
                         count: &mut usize| {
        if payload.is_empty() {
            return;
        }
        let timecode = uint_elem(ID_TIMECODE, base);
        let body = concat(&[timecode, std::mem::take(payload)]);
        clusters.extend_from_slice(&elem(ID_CLUSTER, &body));
        *count += 1;
    };

    for block in &blocks {
        let ts = match block {
            PendingBlock::Audio { ts, .. } => *ts,
            PendingBlock::Subtitle { ts, .. } => *ts,
        };
        let base = *cluster_base.get_or_insert(ts);
        if ts.saturating_sub(base) > MAX_CLUSTER_MS && !cluster_payload.is_empty() {
            flush_cluster(base, &mut cluster_payload, &mut clusters, &mut cluster_count);
            cluster_base = Some(ts);
        }
        let base = cluster_base.unwrap_or(ts);
        let rel = ts.saturating_sub(base).min(i16::MAX as u64) as i16;
        match block {
            PendingBlock::Audio { track, frame, .. } => {
                if let Some(audio) = prepared_audio.iter().find(|item| item.number == *track) {
                    if let Some(frame_data) = audio.audio.frames.get(*frame) {
                        cluster_payload.extend_from_slice(&build_block(
                            *track,
                            rel,
                            &frame_data.data,
                            true,
                            None,
                        ));
                    }
                }
            }
            PendingBlock::Subtitle { track, cue, dur, .. } => {
                if let Some(subtitle) = prepared_subtitles.iter().find(|item| item.number == *track) {
                    if let Some(cue) = subtitle.cues.get(*cue) {
                        if let Ok(data) = serde_json::to_vec(cue) {
                            cluster_payload.extend_from_slice(&build_block(
                                *track,
                                rel,
                                &data,
                                false,
                                Some(*dur),
                            ));
                        }
                    }
                }
            }
        }
    }
    if let Some(base) = cluster_base {
        flush_cluster(base, &mut cluster_payload, &mut clusters, &mut cluster_count);
    }

    let mut info_fields = vec![
        uint_elem(ID_TIMECODE_SCALE, TIMECODE_SCALE_NS),
        float_elem(ID_DURATION, max_end_ms as f64),
        str_elem(ID_MUXING_APP, "NeoBox"),
        str_elem(ID_WRITING_APP, "NeoBox"),
    ];
    if let Some(title) = options.title.as_deref().filter(|value| !value.trim().is_empty()) {
        info_fields.push(str_elem(ID_TITLE, title));
    }
    let info = elem(ID_INFO, &concat(&info_fields));

    let mut track_entries: Vec<Vec<u8>> = prepared_audio
        .iter()
        .map(|track| {
            build_track_entry(
                track.number,
                track.uid,
                TRACK_TYPE_AUDIO,
                &track.audio.codec_id,
                &track.audio.codec_private,
                &track.name,
                &track.language,
                track.default,
                Some((track.audio.sample_rate, track.audio.channels)),
                Some(track.audio.default_duration_ns),
            )
        })
        .collect();
    for subtitle in &prepared_subtitles {
        track_entries.push(build_track_entry(
            subtitle.number,
            subtitle.uid,
            TRACK_TYPE_SUBTITLE,
            "S_TEXT/NBMUSIC",
            &subtitle.codec_private,
            &subtitle.name,
            &subtitle.language,
            subtitle.default,
            None,
            None,
        ));
    }
    let tracks = elem(ID_TRACKS, &concat(&track_entries));

    let ebml = elem(
        ID_EBML,
        &concat(&[
            uint_elem(ID_EBML_VERSION, 1),
            uint_elem(ID_EBML_READ_VERSION, 1),
            uint_elem(ID_EBML_MAX_ID_LENGTH, 4),
            uint_elem(ID_EBML_MAX_SIZE_LENGTH, 8),
            str_elem(ID_DOC_TYPE, "matroska"),
            uint_elem(ID_DOC_TYPE_VERSION, 4),
            uint_elem(ID_DOC_TYPE_READ_VERSION, 2),
        ]),
    );

    let segment_body = concat(&[info, tracks, tags, attachments, clusters]);
    let segment = elem(ID_SEGMENT, &segment_body);
    let mut file = Vec::with_capacity(ebml.len() + segment.len());
    file.extend_from_slice(&ebml);
    file.extend_from_slice(&segment);

    std::fs::write(&options.output_path, &file).map_err(|e| format!("写入 .nbmusic 失败: {e}"))?;
    let size = file.len() as u64;

    log::info!(
        "[NBMusic] 写出 {}：音频 {} 轨，字幕 {} 轨，封面 {}，字体 {}，高潮 {}，簇 {}，{} 字节",
        options.output_path,
        prepared_audio.len(),
        prepared_subtitles.len(),
        has_cover,
        font_attachments.len(),
        options.climaxes.len(),
        cluster_count,
        size
    );

    Ok(NbmusicMuxResult {
        output_path: options.output_path.clone(),
        duration_ms: max_end_ms,
        audio_track_count: prepared_audio.len(),
        subtitle_track_count: prepared_subtitles.len(),
        has_cover,
        font_count: font_attachments.len(),
        size,
    })
}

// ─────────────────────────── 解封装 ───────────────────────────

fn parse_children(body: &[u8]) -> Result<Vec<(u32, &[u8])>, String> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < body.len() {
        let id = read_vint_id(body, &mut pos)?;
        let size = read_vint_size(body, &mut pos)?;
        let end = match size {
            Some(size) => (pos + size as usize).min(body.len()),
            None => body.len(),
        };
        out.push((id, &body[pos..end]));
        pos = end;
    }
    Ok(out)
}

fn parse_track_entry(body: &[u8]) -> Option<ParsedTrack> {
    let mut track = ParsedTrack {
        number: 0,
        uid: 0,
        track_type: 0,
        codec_id: String::new(),
        codec_private: Vec::new(),
        name: String::new(),
        language: String::new(),
        default: false,
        sample_rate: 0.0,
        channels: 0,
        bit_depth: 0,
    };
    let mut pos = 0usize;
    while pos < body.len() {
        let id = read_vint_id(body, &mut pos).ok()?;
        let size = read_vint_size(body, &mut pos).ok()?? as usize;
        if pos + size > body.len() {
            return None;
        }
        let value = &body[pos..pos + size];
        match id {
            0xD7 => track.number = read_uint(value),
            0x73C5 => track.uid = read_uint(value),
            0x83 => track.track_type = read_uint(value),
            0x86 => track.codec_id = String::from_utf8_lossy(value).to_string(),
            0x63A2 => track.codec_private = value.to_vec(),
            0x536E => track.name = String::from_utf8_lossy(value).to_string(),
            0x22B59C => track.language = String::from_utf8_lossy(value).to_string(),
            0x88 => track.default = read_uint(value) != 0,
            0xE1 => {
                let mut audio_pos = 0usize;
                while audio_pos < value.len() {
                    let audio_id = read_vint_id(value, &mut audio_pos).ok()?;
                    let audio_size = read_vint_size(value, &mut audio_pos).ok()?? as usize;
                    if audio_pos + audio_size > value.len() {
                        return None;
                    }
                    let audio_value = &value[audio_pos..audio_pos + audio_size];
                    match audio_id {
                        0xB5 => track.sample_rate = read_float(audio_value),
                        0x9F => track.channels = read_uint(audio_value),
                        0x6264 => track.bit_depth = read_uint(audio_value),
                        _ => {}
                    }
                    audio_pos += audio_size;
                }
            }
            _ => {}
        }
        pos += size;
    }
    Some(track)
}

fn parse_tag(body: &[u8]) -> Result<(Option<u64>, Vec<(String, String)>), String> {
    let mut target_uid = None;
    let mut tags = Vec::new();
    for (id, value) in parse_children(body)? {
        match id {
            0x63C0 => {
                for (target_id, target_value) in parse_children(value)? {
                    if target_id == 0x63C5 {
                        target_uid = Some(read_uint(target_value));
                    }
                }
            }
            0x67C8 => {
                let mut name = String::new();
                let mut text = String::new();
                for (simple_id, simple_value) in parse_children(value)? {
                    match simple_id {
                        0x45A3 => name = String::from_utf8_lossy(simple_value).to_string(),
                        0x4487 => text = String::from_utf8_lossy(simple_value).to_string(),
                        _ => {}
                    }
                }
                if !name.is_empty() {
                    tags.push((name, text));
                }
            }
            _ => {}
        }
    }
    Ok((target_uid, tags))
}

fn parse_attachment(body: &[u8]) -> Result<AttachmentFile, String> {
    let mut uid = 0u64;
    let mut name = String::new();
    let mut mime = String::new();
    let mut data = Vec::new();
    for (id, value) in parse_children(body)? {
        match id {
            0x46AE => uid = read_uint(value),
            0x466E => name = String::from_utf8_lossy(value).to_string(),
            0x4660 => mime = String::from_utf8_lossy(value).to_string(),
            0x465C => data = value.to_vec(),
            _ => {}
        }
    }
    Ok(AttachmentFile {
        uid,
        name,
        mime,
        data,
    })
}

fn parse_cluster_blocks(body: &[u8], blocks: &mut Vec<RawBlock>) -> Result<(), String> {
    let mut cluster_ts = 0u64;
    for (id, value) in parse_children(body)? {
        match id {
            0xE7 => cluster_ts = read_uint(value),
            0xA3 => {
                if let Some((track, ts, flags, payload)) = parse_block_payload(value, cluster_ts) {
                    if flags & 0x06 == 0 {
                        blocks.push(RawBlock {
                            track,
                            ts,
                            data: payload.to_vec(),
                        });
                    }
                }
            }
            0xA0 => {
                let mut block_payload: Option<Vec<u8>> = None;
                for (group_id, group_value) in parse_children(value)? {
                    if group_id == 0xA1 {
                        block_payload = Some(group_value.to_vec());
                    }
                }
                if let Some(payload) = block_payload {
                    if let Some((track, ts, flags, body)) = parse_block_payload(&payload, cluster_ts) {
                        if flags & 0x06 == 0 {
                            blocks.push(RawBlock {
                                track,
                                ts,
                                data: body.to_vec(),
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn demux_nbmusic(path: &str) -> Result<NbmusicDemux, String> {
    let data = std::fs::read(path).map_err(|e| format!("读取 .nbmusic 失败: {e}"))?;
    if data.len() < 4 || data[..4] != *ID_EBML {
        return Err("不是 EBML/Matroska 文件".to_string());
    }

    let mut duration_ms = 0u64;
    let mut info_title = String::new();
    let mut tracks: Vec<ParsedTrack> = Vec::new();
    let mut blocks: Vec<RawBlock> = Vec::new();
    let mut attachments: Vec<AttachmentFile> = Vec::new();
    let mut global_tags: Vec<(String, String)> = Vec::new();
    let mut track_tags: Vec<(u64, Vec<(String, String)>)> = Vec::new();

    for (id, body) in parse_children(&data)? {
        if id != 0x18538067 {
            continue;
        }
        for (sub_id, sub_body) in parse_children(body)? {
            match sub_id {
                0x1549A966 => {
                    for (info_id, info_value) in parse_children(sub_body)? {
                        match info_id {
                            0x4489 => {
                                let ms = read_float(info_value);
                                if ms.is_finite() && ms > 0.0 {
                                    duration_ms = ms as u64;
                                }
                            }
                            0x7BA9 => info_title = String::from_utf8_lossy(info_value).to_string(),
                            _ => {}
                        }
                    }
                }
                0x1654AE6B => {
                    for (track_id, track_body) in parse_children(sub_body)? {
                        if track_id == 0xAE {
                            if let Some(track) = parse_track_entry(track_body) {
                                tracks.push(track);
                            }
                        }
                    }
                }
                0x1254C367 => {
                    for (tag_id, tag_body) in parse_children(sub_body)? {
                        if tag_id == 0x7373 {
                            let (target, tags) = parse_tag(tag_body)?;
                            if let Some(uid) = target {
                                track_tags.push((uid, tags));
                            } else {
                                global_tags.extend(tags);
                            }
                        }
                    }
                }
                0x1941A469 => {
                    for (att_id, att_body) in parse_children(sub_body)? {
                        if att_id == 0x61A7 {
                            attachments.push(parse_attachment(att_body)?);
                        }
                    }
                }
                0x1F43B675 => {
                    parse_cluster_blocks(sub_body, &mut blocks)?;
                }
                _ => {}
            }
        }
    }

    let spec = global_tags
        .iter()
        .find(|(name, _)| name == "NBMUSIC_SPEC")
        .map(|(_, value)| value.clone())
        .ok_or_else(|| "不是 .nbmusic：缺少 NBMUSIC_SPEC".to_string())?;
    let id = global_tags
        .iter()
        .find(|(name, _)| name == "NBMUSIC_ID")
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let title = global_tags
        .iter()
        .find(|(name, _)| name == "TITLE")
        .map(|(_, value)| value.clone())
        .filter(|value| !value.is_empty())
        .unwrap_or(info_title);
    let artist = global_tags
        .iter()
        .find(|(name, _)| name == "ARTIST")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    let album = global_tags
        .iter()
        .find(|(name, _)| name == "ALBUM")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();

    let mut audio_tracks: Vec<DemuxAudioTrack> = Vec::new();
    for track in tracks.iter().filter(|track| track.track_type == TRACK_TYPE_AUDIO) {
        let mut frames: Vec<(u64, Vec<u8>)> = blocks
            .iter()
            .filter(|block| block.track == track.number)
            .map(|block| (block.ts, block.data.clone()))
            .collect();
        frames.sort_by_key(|(ts, _)| *ts);
        if frames.is_empty() {
            continue;
        }
        let tags = track_tags
            .iter()
            .find(|(uid, _)| *uid == track.uid)
            .map(|(_, tags)| tags.clone())
            .unwrap_or_default();
        let channel_programs = tags
            .iter()
            .find(|(name, _)| name == "NBMUSIC_CHANNEL_PROGRAMS")
            .and_then(|(_, value)| serde_json::from_str::<Vec<ChannelProgram>>(value).ok())
            .unwrap_or_default();
        let dual_role = tags
            .iter()
            .find(|(name, _)| name == "NBMUSIC_DUAL_ROLE")
            .and_then(|(_, value)| normalize_dual_side(Some(value)));
        audio_tracks.push(DemuxAudioTrack {
            uid: track.uid.to_string(),
            codec_id: track.codec_id.clone(),
            codec_private: track.codec_private.clone(),
            sample_rate: if track.sample_rate > 0.0 { track.sample_rate } else { 44100.0 },
            channels: if track.channels > 0 { track.channels } else { 2 },
            bit_depth: track.bit_depth,
            language: track.language.clone(),
            title: track.name.clone(),
            default: track.default,
            dual_role,
            channel_programs,
            frames,
        });
    }
    if audio_tracks.is_empty() {
        return Err(".nbmusic 中没有可用音频轨".to_string());
    }

    let mut subtitle_tracks: Vec<DemuxSubtitleTrack> = Vec::new();
    for track in tracks.iter().filter(|track| track.track_type == TRACK_TYPE_SUBTITLE && track.codec_id.eq_ignore_ascii_case("S_TEXT/NBMUSIC")) {
        let meta: SubtitleCodecMeta = serde_json::from_slice(&track.codec_private).unwrap_or_default();
        let mut cues: Vec<Cue> = blocks
            .iter()
            .filter(|block| block.track == track.number)
            .filter_map(|block| serde_json::from_slice::<Cue>(&block.data).ok())
            .collect();
        cues.sort_by_key(|cue| cue.start_ms);
        let tags = track_tags
            .iter()
            .find(|(uid, _)| *uid == track.uid)
            .map(|(_, tags)| tags.clone())
            .unwrap_or_default();
        let tag_value = |name: &str| {
            tags.iter()
                .find(|(tag_name, _)| tag_name == name)
                .map(|(_, value)| value.clone())
        };
        let bindings = if !meta.audio_bindings.is_empty() {
            meta.audio_bindings
        } else if let Some(value) = tag_value("NBMUSIC_AUDIO_BINDINGS") {
            serde_json::from_str::<Vec<AudioBinding>>(&value).unwrap_or_default()
        } else {
            Vec::new()
        };
        let roles = if !meta.roles.is_empty() {
            meta.roles
        } else if let Some(value) = tag_value("NBMUSIC_SUBTITLE_ROLES") {
            serde_json::from_str::<Vec<String>>(&value).unwrap_or_default()
        } else {
            vec!["original".to_string()]
        };
        let offset_ms = tag_value("NBMUSIC_LYRICS_OFFSET_MS").and_then(|value| value.parse().ok());
        let forced = tag_value("NBMUSIC_SUBTITLE_FORCED")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        log::debug!("[NBMusic] 字幕轨 v{}：{} 条 Cue", meta.v, cues.len());
        subtitle_tracks.push(DemuxSubtitleTrack {
            uid: if meta.uid.is_empty() { track.uid.to_string() } else { meta.uid },
            language: if meta.language.is_empty() { track.language.clone() } else { meta.language },
            roles,
            audio_bindings: bindings,
            pronunciation_scheme: meta
                .pronunciation_scheme
                .or_else(|| tag_value("NBMUSIC_PRONUNCIATION_SCHEME")),
            source_subtitle_uid: tag_value("NBMUSIC_SOURCE_SUBTITLE"),
            title: track.name.clone(),
            default: track.default,
            forced,
            offset_ms,
            dual_side: normalize_dual_side(tag_value("NBMUSIC_DUAL_SIDE").as_deref()),
            cues,
        });
    }

    let fonts_tag = global_tags
        .iter()
        .find(|(name, _)| name == "NBMUSIC_FONTS")
        .and_then(|(_, value)| serde_json::from_str::<Vec<FontAttachment>>(value).ok())
        .unwrap_or_default();
    let mut fonts = Vec::new();
    for attachment in attachments.iter().filter(|file| file.mime.starts_with("font/")) {
        let metadata = fonts_tag
            .iter()
            .find(|font| font.attachment_uid == attachment.uid.to_string())
            .cloned()
            .unwrap_or_else(|| FontAttachment {
                attachment_uid: attachment.uid.to_string(),
                file_name: attachment.name.clone(),
                mime: attachment.mime.clone(),
                family: Path::new(&attachment.name)
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("Embedded Font")
                    .to_string(),
                style: None,
                weight: None,
                license: None,
                subset: None,
                sha256: None,
            });
        fonts.push(FoundFont {
            metadata,
            data: attachment.data.clone(),
        });
    }

    let climaxes = global_tags
        .iter()
        .find(|(name, _)| name == "NBMUSIC_CLIMAX")
        .and_then(|(_, value)| serde_json::from_str::<Vec<ClimaxMarker>>(value).ok())
        .unwrap_or_default();

    let cover = attachments
        .iter()
        .filter(|file| file.mime.starts_with("image/"))
        .max_by_key(|file| if file.name.to_ascii_lowercase().contains("front") { 1 } else { 0 })
        .map(|file| (file.mime.clone(), file.data.clone()));

    if duration_ms == 0 {
        duration_ms = audio_tracks
            .iter()
            .flat_map(|track| track.frames.iter().map(|(ts, _)| *ts))
            .max()
            .unwrap_or(0);
        duration_ms = duration_ms.max(
            subtitle_tracks
                .iter()
                .flat_map(|track| track.cues.iter().map(|cue| cue.end_ms))
                .max()
                .unwrap_or(0),
        );
    }

    Ok(NbmusicDemux {
        spec,
        id,
        title,
        artist,
        album,
        duration_ms,
        audio_tracks,
        subtitle_tracks,
        cover,
        fonts,
        climaxes,
    })
}

// ─────────────────────────── 查询 / 播放提取 ───────────────────────────

pub fn probe_nbmusic_file(path: &str) -> Result<NbmusicProbe, String> {
    let demux = demux_nbmusic(path)?;
    Ok(NbmusicProbe {
        spec: demux.spec,
        id: demux.id,
        title: demux.title,
        artist: demux.artist,
        album: demux.album,
        duration_ms: demux.duration_ms,
        audio_tracks: demux
            .audio_tracks
            .iter()
            .map(|track| NbmusicAudioTrackInfo {
                uid: track.uid.clone(),
                codec: track.codec_id.clone(),
                sample_rate: track.sample_rate,
                channels: track.channels,
                language: track.language.clone(),
                title: track.title.clone(),
                default: track.default,
                dual_role: track.dual_role.clone(),
            })
            .collect(),
        subtitle_tracks: demux
            .subtitle_tracks
            .iter()
            .map(|track| NbmusicSubtitleTrackInfo {
                uid: track.uid.clone(),
                language: track.language.clone(),
                roles: track.roles.clone(),
                audio_bindings: track.audio_bindings.clone(),
                pronunciation_scheme: track.pronunciation_scheme.clone(),
                source_subtitle_uid: track.source_subtitle_uid.clone(),
                title: track.title.clone(),
                default: track.default,
                forced: track.forced,
                offset_ms: track.offset_ms,
                dual_side: track.dual_side.clone(),
                cue_count: track.cues.len(),
            })
            .collect(),
        has_cover: demux.cover.is_some(),
        fonts: demux.fonts.iter().map(|font| font.metadata.clone()).collect(),
        climaxes: demux.climaxes,
    })
}

pub fn read_basic_metadata(path: &str) -> Result<(String, String, String, u64), String> {
    let demux = demux_nbmusic(path)?;
    Ok((demux.title, demux.artist, demux.album, demux.duration_ms))
}

/// 把 `.nbmusic` 解成可编辑工程：音频 / 封面 / 字体落到临时目录，
/// 字幕 Cue 与标签直接转为结构化 JSON，供编辑页面加载后再次 mux。
pub fn load_nbmusic_project_file(path: &str) -> Result<NbmusicProject, String> {
    let demux = demux_nbmusic(path)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let work_dir = std::env::temp_dir().join(format!(
        "neobox_nbmusic_edit_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&work_dir).map_err(|e| format!("创建编辑临时目录失败: {e}"))?;

    let mut audio_tracks = Vec::new();
    for (index, track) in demux.audio_tracks.iter().enumerate() {
        let ext = if track.codec_id.to_ascii_uppercase().starts_with("A_AAC") {
            "aac"
        } else if track.codec_id.to_ascii_uppercase().starts_with("A_MPEG") {
            "mp3"
        } else if track.codec_id.to_ascii_uppercase().starts_with("A_FLAC") {
            "flac"
        } else if track.codec_id.to_ascii_uppercase().starts_with("A_PCM") {
            "wav"
        } else {
            return Err(format!("暂不支持编辑 {} 音频轨", track.codec_id));
        };
        let output = work_dir.join(format!("audio-{}.{}", index + 1, ext));
        let audio = crate::matroska::DemuxAudio {
            codec_id: track.codec_id.clone(),
            codec_private: track.codec_private.clone(),
            sample_rate: track.sample_rate,
            channels: track.channels,
            bit_depth: track.bit_depth,
            frames: track.frames.clone(),
            duration_ms: demux.duration_ms,
            cover: None,
            subtitle: None,
        };
        crate::matroska::write_playable_audio(&audio, &output)?;
        audio_tracks.push(NbmusicProjectAudio {
            path: output.to_string_lossy().to_string(),
            uid: track.uid.clone(),
            codec: track.codec_id.clone(),
            sample_rate: track.sample_rate,
            channels: track.channels,
            language: track.language.clone(),
            title: track.title.clone(),
            default: track.default,
            dual_role: track.dual_role.clone(),
            channel_programs: track.channel_programs.clone(),
        });
    }

    let cover_path = demux.cover.as_ref().and_then(|(mime, data)| {
        let ext = match mime.as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => return None,
        };
        let output = work_dir.join(format!("cover-front.{ext}"));
        std::fs::write(&output, data).ok()?;
        Some(output.to_string_lossy().to_string())
    });

    let mut fonts = Vec::new();
    for (index, font) in demux.fonts.iter().enumerate() {
        let ext = font.metadata.mime.rsplit('/').next().unwrap_or("bin");
        let output = work_dir.join(format!("font-{}.{}", index + 1, ext));
        std::fs::write(&output, &font.data)
            .map_err(|e| format!("写入编辑字体缓存失败: {e}"))?;
        fonts.push(NbmusicProjectFont {
            path: output.to_string_lossy().to_string(),
            file_name: font.metadata.file_name.clone(),
            mime: font.metadata.mime.clone(),
            family: font.metadata.family.clone(),
            style: font.metadata.style.clone(),
            weight: font.metadata.weight,
            license: font.metadata.license.clone(),
            subset: font.metadata.subset.clone(),
        });
    }

    Ok(NbmusicProject {
        source_path: path.to_string(),
        spec: demux.spec,
        id: demux.id,
        title: demux.title,
        artist: demux.artist,
        album: demux.album,
        duration_ms: demux.duration_ms,
        cover_path,
        audio_tracks,
        subtitle_tracks: demux
            .subtitle_tracks
            .into_iter()
            .map(|track| NbmusicProjectSubtitle {
                uid: track.uid,
                language: track.language,
                roles: track.roles,
                audio_bindings: track.audio_bindings,
                pronunciation_scheme: track.pronunciation_scheme,
                source_subtitle_uid: track.source_subtitle_uid,
                title: track.title,
                default: track.default,
                forced: track.forced,
                offset_ms: track.offset_ms,
                dual_side: track.dual_side,
                cues: track.cues,
            })
            .collect(),
        fonts,
        climaxes: demux.climaxes,
    })
}

/// 双声道音频对：优先读取 mux 时写入的 left/right 角色，缺失时回退到轨道顺序。
fn dual_audio_tracks(demux: &NbmusicDemux) -> Option<(&DemuxAudioTrack, &DemuxAudioTrack)> {
    if demux.audio_tracks.len() < 2 {
        return None;
    }
    let left = demux
        .audio_tracks
        .iter()
        .find(|track| track.dual_role.as_deref() == Some("left"))
        .or_else(|| demux.audio_tracks.first())?;
    let right = demux
        .audio_tracks
        .iter()
        .find(|track| track.dual_role.as_deref() == Some("right") && !std::ptr::eq(*track, left))
        .or_else(|| {
            demux
                .audio_tracks
                .iter()
                .find(|track| !std::ptr::eq(*track, left))
        })?;
    Some((left, right))
}

/// 双声道字幕对：优先读取 dualSide，缺失时回退到字幕轨顺序。
fn dual_subtitle_tracks(demux: &NbmusicDemux) -> Vec<&DemuxSubtitleTrack> {
    let candidates: Vec<&DemuxSubtitleTrack> = demux
        .subtitle_tracks
        .iter()
        .filter(|track| !track.cues.is_empty())
        .collect();
    if candidates.len() < 2 {
        return candidates;
    }
    let left = candidates
        .iter()
        .find(|track| track.dual_side.as_deref() == Some("left"))
        .or_else(|| candidates.first())
        .copied();
    let left_uid = left.map(|track| track.uid.as_str());
    let right = candidates
        .iter()
        .find(|track| {
            track.dual_side.as_deref() == Some("right")
                && left_uid.map(|uid| track.uid != uid).unwrap_or(true)
        })
        .or_else(|| {
            candidates
                .iter()
                .find(|track| left_uid.map(|uid| track.uid != uid).unwrap_or(true))
        })
        .copied();
    match (left, right) {
        (Some(left), Some(right)) => vec![left, right],
        (Some(left), None) => vec![left],
        _ => Vec::new(),
    }
}

fn pick_subtitle<'a>(
    demux: &'a NbmusicDemux,
    audio: &DemuxAudioTrack,
) -> Option<&'a DemuxSubtitleTrack> {    demux
        .subtitle_tracks
        .iter()
        .filter(|track| !track.cues.is_empty())
        .find(|track| {
            track.roles.iter().any(|role| role == "original")
                && (track.language == audio.language || audio.language == "und")
        })
        .or_else(|| {
            demux
                .subtitle_tracks
                .iter()
                .find(|track| !track.cues.is_empty() && track.roles.iter().any(|role| role == "original"))
        })
        .or_else(|| demux.subtitle_tracks.iter().find(|track| !track.cues.is_empty()))
}

fn write_cover_cache(
    cache_dir: &Path,
    cache_key: &str,
    cover: &Option<(String, Vec<u8>)>,
) -> Option<String> {
    let (mime, data) = cover.as_ref()?;

    // 播放缓存不落 PNG：PNG 封面解码后统一转成 JPEG 写入。
    // 这样播放/导入 `.nbmusic` 时缓存目录只会有 jpg，不会每次生成大体积 png。
    let mut jpeg: Option<Vec<u8>> = None;
    let (ext, bytes): (&str, &[u8]) = if mime == "image/png" {
        let img = match image::load_from_memory_with_format(data, image::ImageFormat::Png) {
            Ok(img) => img,
            Err(error) => {
                log::warn!("[NBMusic] 解码 PNG 封面失败，跳过封面缓存：{error}");
                return None;
            }
        };
        let rgb = img.to_rgb8();
        let (width, height) = rgb.dimensions();
        let mut out = Vec::new();
        {
            let mut cursor = std::io::Cursor::new(&mut out);
            if let Err(error) = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, 90)
                .write_image(
                    rgb.as_raw(),
                    width,
                    height,
                    image::ExtendedColorType::Rgb8,
                )
            {
                log::warn!("[NBMusic] PNG 封面转 JPEG 失败，跳过封面缓存：{error}");
                return None;
            }
        }
        jpeg = Some(out);
        ("jpg", jpeg.as_deref().unwrap_or_default())
    } else if mime == "image/gif" {
        ("gif", data.as_slice())
    } else if mime == "image/webp" {
        ("webp", data.as_slice())
    } else {
        ("jpg", data.as_slice())
    };

    let path = cache_dir.join(format!("{cache_key}_cover.{ext}"));
    let intact = std::fs::metadata(&path)
        .map(|meta| meta.len() == bytes.len() as u64)
        .unwrap_or(false);
    if !intact && std::fs::write(&path, bytes).is_err() {
        log::warn!("[NBMusic] 写封面缓存失败：{}", path.display());
        return None;
    }

    // 清掉旧版本遗留的 PNG 封面缓存，避免同一首歌同时留 png/jpg 两份。
    let stale_png = cache_dir.join(format!("{cache_key}_cover.png"));
    if stale_png != path && stale_png.is_file() {
        let _ = std::fs::remove_file(&stale_png);
    }

    Some(path.to_string_lossy().to_string())
}

/// 把 .nbmusic 的主音频解到缓存目录，返回可播放文件路径、封面缓存、时长。
/// 与 matroska::extract_playable_audio 保持相同的调用签名，便于音乐模块复用。
pub fn extract_playable_audio(
    path: &str,
    cache_dir: &Path,
    cache_key: &str,
) -> Result<(String, Option<String>, u64), String> {
    let demux = demux_nbmusic(path)?;
    std::fs::create_dir_all(cache_dir).map_err(|e| format!("创建播放缓存目录失败: {e}"))?;

    let audio = demux
        .audio_tracks
        .iter()
        .find(|track| track.default)
        .or_else(|| demux.audio_tracks.first())
        .ok_or_else(|| ".nbmusic 没有可用音频轨".to_string())?;

    let dual_pair = dual_audio_tracks(&demux);
    let lrc = if dual_pair.is_some() {
        dual_subtitle_lrc(&demux)
    } else {
        pick_subtitle(&demux, audio)
            .map(|subtitle| subtitle_lrc_from_cues(&subtitle.cues))
            .unwrap_or_default()
    };
    if !lrc.is_empty() {
        let _ = std::fs::write(cache_dir.join(format!("{cache_key}.lyrics-v3.lrc")), lrc);
    }

    // 双轨工程：把两条音轨分别解成 PCM，再合成为一个立体声文件，
    // 左轨只进左声道、右轨只进右声道，前端仍然只需一个 audio 元素。
    let playable = if dual_pair.is_some() {
        match build_dual_stereo_playable(&demux, cache_dir, cache_key) {
            Ok(path) => path,
            Err(error) => {
                log::warn!("[NBMusic] 双轨立体声合成失败，回退单轨播放：{error}");
                extract_single_track_playable(audio, &demux, cache_dir, cache_key)?
            }
        }
    } else {
        extract_single_track_playable(audio, &demux, cache_dir, cache_key)?
    };

    let cover_path = write_cover_cache(cache_dir, cache_key, &demux.cover);

    // 字体缓存：为后续字幕渲染预留，失败不影响播放。
    for (index, font) in demux.fonts.iter().enumerate() {
        let ext = font.metadata.mime.rsplit('/').next().unwrap_or("bin");
        let font_path = cache_dir.join(format!("{cache_key}_font_{index}.{ext}"));
        let _ = std::fs::write(font_path, &font.data);
    }

    Ok((playable.to_string_lossy().to_string(), cover_path, demux.duration_ms))
}

fn audio_codec_extension(codec_id: &str) -> Result<(&'static str, bool), String> {
    let codec = codec_id.to_ascii_uppercase();
    if codec.starts_with("A_AAC") {
        Ok(("aac", true))
    } else if codec.starts_with("A_MPEG") {
        Ok(("mp3", false))
    } else if codec.starts_with("A_FLAC") {
        Ok(("flac", false))
    } else if codec.starts_with("A_PCM") {
        Ok(("wav", false))
    } else {
        Err(format!("暂不支持播放 .nbmusic 的 {codec_id} 编码"))
    }
}

/// 解封装单条音频轨到缓存，AAC 额外封装成 M4A（浏览器不能直接播 ADTS）。
fn write_track_playable(
    track: &DemuxAudioTrack,
    duration_ms: u64,
    cache_dir: &Path,
    stem: &str,
) -> Result<PathBuf, String> {
    let (ext, needs_remux) = audio_codec_extension(&track.codec_id)?;
    let temp_audio = cache_dir.join(format!("{stem}.{ext}"));
    let demux_audio = crate::matroska::DemuxAudio {
        codec_id: track.codec_id.clone(),
        codec_private: track.codec_private.clone(),
        sample_rate: track.sample_rate,
        channels: track.channels,
        bit_depth: track.bit_depth,
        frames: track.frames.clone(),
        duration_ms,
        cover: None,
        subtitle: None,
    };
    crate::matroska::write_playable_audio(&demux_audio, &temp_audio)?;
    if !needs_remux {
        return Ok(temp_audio);
    }
    let m4a = cache_dir.join(format!("{stem}.m4a"));
    match crate::file_convert::convert_audio_impl(
        &temp_audio.to_string_lossy(),
        &m4a.to_string_lossy(),
    ) {
        Ok(()) => {
            let _ = std::fs::remove_file(&temp_audio);
            Ok(m4a)
        }
        Err(error) => {
            log::warn!("[NBMusic] AAC 封装 M4A 失败，回退直接播放 ADTS：{error}");
            Ok(temp_audio)
        }
    }
}

fn extract_single_track_playable(
    audio: &DemuxAudioTrack,
    demux: &NbmusicDemux,
    cache_dir: &Path,
    cache_key: &str,
) -> Result<PathBuf, String> {
    write_track_playable(audio, demux.duration_ms, cache_dir, cache_key)
}

fn build_dual_stereo_playable(
    demux: &NbmusicDemux,
    cache_dir: &Path,
    cache_key: &str,
) -> Result<PathBuf, String> {
    let m4a = cache_dir.join(format!("{cache_key}.stereo.m4a"));
    if m4a.is_file() && std::fs::metadata(&m4a).map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(m4a);
    }

    let (left, right) =
        dual_audio_tracks(demux).ok_or_else(|| "音频轨不足两条".to_string())?;
    let tracks = [left, right];

    let mut track_files: Vec<PathBuf> = Vec::new();
    let mut wav_files: Vec<PathBuf> = Vec::new();
    let result = (|| -> Result<PathBuf, String> {
        for (index, track) in tracks.iter().enumerate() {
            let stem = format!("{cache_key}.track{index}");
            let track_file = write_track_playable(track, demux.duration_ms, cache_dir, &stem)?;
            track_files.push(track_file.clone());
            let wav = cache_dir.join(format!("{stem}.decoded.wav"));
            crate::file_convert::decode_audio_to_wav(
                &track_file.to_string_lossy(),
                &wav.to_string_lossy(),
            )?;
            wav_files.push(wav);
        }

        let stereo_wav = cache_dir.join(format!("{cache_key}.stereo.wav"));
        mix_dual_tracks_to_stereo_wav(&wav_files[0], &wav_files[1], &stereo_wav)?;

        match crate::file_convert::convert_audio_impl(
            &stereo_wav.to_string_lossy(),
            &m4a.to_string_lossy(),
        ) {
            Ok(()) => {
                let _ = std::fs::remove_file(&stereo_wav);
                Ok(m4a.clone())
            }
            Err(error) => {
                log::warn!("[NBMusic] 立体声 WAV 封装 M4A 失败，直接播放 WAV：{error}");
                Ok(stereo_wav)
            }
        }
    })();

    for path in track_files.iter().chain(wav_files.iter()) {
        let _ = std::fs::remove_file(path);
    }
    if result.is_err() {
        let _ = std::fs::remove_file(&m4a);
    }
    result
}

struct PcmWav {
    sample_rate: u32,
    channels: u16,
    bits: u16,
    is_float: bool,
    block_align: usize,
    data: Vec<u8>,
}

fn read_pcm_wav(path: &Path) -> Result<PcmWav, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取解码 WAV 失败: {e}"))?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("解码结果不是标准 WAV".to_string());
    }
    let format = u16::from_le_bytes([bytes[20], bytes[21]]);
    let channels = u16::from_le_bytes([bytes[22], bytes[23]]);
    let sample_rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
    let bits = u16::from_le_bytes([bytes[34], bytes[35]]);
    let block_align = u16::from_le_bytes([bytes[32], bytes[33]]) as usize;
    if &bytes[36..40] != b"data" {
        return Err("解码 WAV 缺少 data 块".to_string());
    }
    let data_len = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]) as usize;
    let end = 44usize.saturating_add(data_len).min(bytes.len());
    if channels == 0 || block_align == 0 || sample_rate == 0 {
        return Err("解码 WAV 参数无效".to_string());
    }
    Ok(PcmWav {
        sample_rate,
        channels,
        bits,
        is_float: format == 3,
        block_align,
        data: bytes[44..end].to_vec(),
    })
}

fn wav_sample_at(wav: &PcmWav, frame: usize, channel: usize) -> f32 {
    let bytes_per_sample = (wav.bits as usize / 8).max(1);
    let offset = frame * wav.block_align + channel.min(wav.channels as usize - 1) * bytes_per_sample;
    if offset + bytes_per_sample > wav.data.len() {
        return 0.0;
    }
    let data = &wav.data[offset..offset + bytes_per_sample];
    if wav.is_float {
        return match wav.bits {
            32 => f32::from_bits(u32::from_le_bytes([data[0], data[1], data[2], data[3]])),
            64 => f64::from_le_bytes([
                data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
            ]) as f32,
            _ => 0.0,
        };
    }
    match wav.bits {
        8 => (data[0] as f32 - 128.0) / 128.0,
        16 => i16::from_le_bytes([data[0], data[1]]) as f32 / 32768.0,
        24 => i32::from_le_bytes([0, data[0], data[1], data[2]]) as f32 / 8_388_608.0,
        32 => i32::from_le_bytes([data[0], data[1], data[2], data[3]]) as f32 / 2_147_483_648.0,
        _ => 0.0,
    }
}

/// 两条轨都是「单声道内容复制到左右」的立体声容器：左轨取左声道，右轨取右声道。
fn mix_dual_tracks_to_stereo_wav(
    left_path: &Path,
    right_path: &Path,
    output: &Path,
) -> Result<(), String> {
    let left = read_pcm_wav(left_path)?;
    let right = read_pcm_wav(right_path)?;
    if left.sample_rate != right.sample_rate {
        return Err(format!(
            "两条音轨采样率不一致（{} / {}），无法直接合成",
            left.sample_rate, right.sample_rate
        ));
    }
    let frames = (left.data.len() / left.block_align).min(right.data.len() / right.block_align);
    let mut pcm = Vec::with_capacity(frames * 4);
    for frame in 0..frames {
        let l = wav_sample_at(&left, frame, 0);
        let r = wav_sample_at(&right, frame, 0);
        let l = (l.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        let r = (r.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        pcm.extend_from_slice(&l.to_le_bytes());
        pcm.extend_from_slice(&r.to_le_bytes());
    }

    let channels: u16 = 2;
    let bits: u16 = 16;
    let block_align = channels * (bits / 8);
    let byte_rate = left.sample_rate * block_align as u32;
    let data_len = pcm.len() as u32;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&left.sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&pcm);
    std::fs::write(output, out).map_err(|e| format!("写入立体声 WAV 失败: {e}"))
}

/// 把 `.nbmusic` 的 Cue 转成 LRC：正文第一行是歌词，内联翻译作为第二行，
/// 由前端 parseLrc 还原成主歌词 + 翻译两行显示。
fn subtitle_lrc_from_cues(cues: &[Cue]) -> String {
    let events: Vec<crate::matroska::SubtitleEvent> = cues
        .iter()
        .map(|cue| {
            let mut text = cue.text.trim().to_string();
            if let Some(translation) = cue
                .translations
                .iter()
                .find(|item| !item.text.trim().is_empty())
            {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(translation.text.trim());
            }
            crate::matroska::SubtitleEvent {
                start_ms: cue.start_ms,
                end_ms: cue.end_ms,
                text,
            }
        })
        .collect();
    crate::matroska::subtitle_lrc_from_events(&events)
}

/// 双轨工程把两条字幕轨合并成一条 LRC：同一时间的第一条字幕作主行、
/// 第二条作翻译行，前端 parseLrc 会还原成上下两行同时显示。
fn dual_subtitle_lrc(demux: &NbmusicDemux) -> String {
    let tracks = dual_subtitle_tracks(demux);
    if tracks.len() < 2 {
        return tracks
            .first()
            .map(|track| subtitle_lrc_from_cues(&track.cues))
            .unwrap_or_default();
    }

    let cue_text = |cue: &Cue| cue.text.replace('\r', "").trim().to_string();
    let mut events: Vec<crate::matroska::SubtitleEvent> = Vec::new();

    if tracks[0].cues.len() == tracks[1].cues.len() {
        for (left, right) in tracks[0].cues.iter().zip(tracks[1].cues.iter()) {
            let mut lines = Vec::new();
            let left_text = cue_text(left);
            let right_text = cue_text(right);
            if !left_text.is_empty() {
                lines.push(left_text);
            }
            if !right_text.is_empty() && lines.iter().all(|line| line != &right_text) {
                lines.push(right_text);
            }
            if lines.is_empty() {
                continue;
            }
            events.push(crate::matroska::SubtitleEvent {
                start_ms: left.start_ms.min(right.start_ms),
                end_ms: left.end_ms.max(right.end_ms),
                text: lines.join("\n"),
            });
        }
    } else {
        let mut by_time: std::collections::BTreeMap<u64, (u64, Vec<String>)> =
            std::collections::BTreeMap::new();
        for track in &tracks {
            for cue in &track.cues {
                let text = cue_text(cue);
                if text.is_empty() {
                    continue;
                }
                let entry = by_time
                    .entry(cue.start_ms)
                    .or_insert_with(|| (cue.end_ms, Vec::new()));
                entry.0 = entry.0.max(cue.end_ms);
                if entry.1.last().map(|last| last != &text).unwrap_or(true) {
                    entry.1.push(text);
                }
            }
        }
        for (start_ms, (end_ms, lines)) in by_time {
            events.push(crate::matroska::SubtitleEvent {
                start_ms,
                end_ms,
                text: lines.join("\n"),
            });
        }
    }

    let lrc = crate::matroska::subtitle_lrc_from_events(&events);
    if lrc.is_empty() {
        lrc
    } else {
        format!("NBMUSIC_DUAL=1\n{lrc}")
    }
}

pub fn subtitle_lrc_from_path(path: &str) -> Result<String, String> {
    let demux = demux_nbmusic(path)?;
    if dual_audio_tracks(&demux).is_some() {
        let lrc = dual_subtitle_lrc(&demux);
        if !lrc.is_empty() {
            return Ok(lrc);
        }
    }
    let audio = demux
        .audio_tracks
        .iter()
        .find(|track| track.default)
        .or_else(|| demux.audio_tracks.first());
    let subtitle = audio
        .and_then(|audio| pick_subtitle(&demux, audio))
        .or_else(|| demux.subtitle_tracks.iter().find(|track| !track.cues.is_empty()));
    let Some(subtitle) = subtitle else {
        return Ok(String::new());
    };
    Ok(subtitle_lrc_from_cues(&subtitle.cues))
}

pub fn extract_cover_to_cache(
    path: &str,
    cache_dir: &Path,
    cache_key: &str,
) -> Option<String> {
    let demux = demux_nbmusic(path).ok()?;
    write_cover_cache(cache_dir, cache_key, &demux.cover)
}

// ─────────────────────────── Tauri 命令 ───────────────────────────

#[tauri::command]
pub async fn mux_nbmusic(options: NbmusicMuxOptions) -> Result<NbmusicMuxResult, String> {
    tauri::async_runtime::spawn_blocking(move || mux_nbmusic_file(&options))
        .await
        .map_err(|error| format!("NBMusic 合成任务异常: {error}"))?
}

#[tauri::command]
pub async fn probe_nbmusic(path: String) -> Result<NbmusicProbe, String> {
    tauri::async_runtime::spawn_blocking(move || probe_nbmusic_file(&path))
        .await
        .map_err(|error| format!("NBMusic 探测任务异常: {error}"))?
}

#[tauri::command]
pub async fn load_nbmusic_project(path: String) -> Result<NbmusicProject, String> {
    tauri::async_runtime::spawn_blocking(move || load_nbmusic_project_file(&path))
        .await
        .map_err(|error| format!("NBMusic 编辑工程解包异常: {error}"))?
}

fn contains_cjk(text: &str) -> bool {
    text.chars().any(|ch| {
        matches!(
            ch as u32,
            0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF
        )
    })
}

/// 双行字幕拆分：第一行作为歌词正文，其余行合并为翻译。
/// 语言按第二行文字判断：中文优先 `zh`，否则英文 `en`，无法判断则 `und`。
fn split_bilingual_subtitle(text: &str) -> (String, Vec<InlineTranslation>) {
    let normalized = text
        .replace("\\N", "\n")
        .replace("\\n", "\n")
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n");
    let lines: Vec<&str> = normalized
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.len() < 2 {
        return (text.trim().to_string(), Vec::new());
    }
    let main = lines[0].to_string();
    let translation = lines[1..].join(" ");
    let lang = if contains_cjk(&translation) && !contains_cjk(&main) {
        "zh"
    } else if contains_cjk(&main) && !contains_cjk(&translation) {
        "en"
    } else {
        "und"
    };
    (
        main,
        vec![InlineTranslation {
            lang: lang.to_string(),
            text: translation,
        }],
    )
}

#[tauri::command]
pub async fn parse_nbmusic_subtitle(path: String) -> Result<Vec<Cue>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let events = parse_subtitle_import_events(&path)?;
        Ok(events
            .into_iter()
            .enumerate()
            .map(|(index, event)| {
                // 有逐字进度时不拆行，避免拆行后 chars 与正文对不上
                let (text, translations) = if event.chars.is_empty() {
                    split_bilingual_subtitle(&event.text)
                } else {
                    (event.text, Vec::new())
                };
                Cue {
                    id: Some(format!("c{}", index + 1)),
                    start_ms: event.start_ms,
                    end_ms: event.end_ms,
                    text,
                    chars: event
                        .chars
                        .into_iter()
                        .map(|ch| CharSpan {
                            text: ch.text,
                            start_ms: ch.start_ms,
                            end_ms: ch.end_ms,
                            mode: ch.mode,
                            reading: None,
                            progress: None,
                            confidence: None,
                        })
                        .collect(),
                    pronunciations: Vec::new(),
                    translations,
                    style: None,
                }
            })
            .collect())
    })
    .await
    .map_err(|error| format!("NBMusic 字幕解析异常: {error}"))?
}

// ─────────────────────────── 测试 ───────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cue_json_uses_camel_case_and_roundtrips() {
        let cue = Cue {
            id: Some("c1".to_string()),
            start_ms: 1000,
            end_ms: 2000,
            text: "月亮".to_string(),
            chars: vec![CharSpan {
                text: "月".to_string(),
                start_ms: 1000,
                end_ms: 1500,
                mode: Some("linear".to_string()),
                reading: Some("yuè".to_string()),
                progress: None,
                confidence: None,
            }],
            pronunciations: Vec::new(),
            translations: Vec::new(),
            style: None,
        };
        let json = serde_json::to_value(&cue).unwrap();
        assert_eq!(json["startMs"], 1000);
        assert_eq!(json["chars"][0]["reading"], "yuè");
        let decoded: Cue = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.text, "月亮");
        assert_eq!(decoded.chars.len(), 1);
    }

    #[test]
    fn bilingual_subtitle_splits_translation() {
        let (main, translations) = split_bilingual_subtitle(
            "Now for the official national weather service forecast.\n现在由国家气象局播报天气预报",
        );
        assert_eq!(main, "Now for the official national weather service forecast.");
        assert_eq!(translations.len(), 1);
        assert_eq!(translations[0].lang, "zh");
        assert_eq!(translations[0].text, "现在由国家气象局播报天气预报");
    }

    #[test]
    fn subtitle_codec_private_roundtrips() {
        let bindings = vec![AudioBinding {
            track_uid: "1".to_string(),
            channels: Some(vec![1]),
            labels: Some(vec!["L".to_string()]),
        }];
        let roles = vec!["original".to_string(), "karaoke".to_string()];
        let private = SubtitleCodecPrivate {
            v: 1,
            uid: "2",
            audio_bindings: &bindings,
            roles: &roles,
            language: "zh-CN",
            pronunciation_scheme: Some("pinyin"),
        };
        let bytes = serde_json::to_vec(&private).unwrap();
        let meta: SubtitleCodecMeta = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(meta.uid, "2");
        assert_eq!(meta.audio_bindings[0].channels.as_deref(), Some(&[1][..]));
        assert_eq!(meta.roles, roles);
    }

    #[test]
    fn png_cover_cache_is_written_as_jpeg() {
        let dir = std::env::temp_dir().join("neobox_nbmusic_cover_png_to_jpeg");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let img = image::RgbImage::from_fn(48, 32, |x, y| {
            image::Rgb([(x * 5) as u8, (y * 7) as u8, 180])
        });
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(
                img.as_raw(),
                img.width(),
                img.height(),
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();

        let path = write_cover_cache(
            &dir,
            "cover-test",
            &Some(("image/png".to_string(), png)),
        )
        .expect("PNG 封面应成功转 JPEG 并写入缓存");
        assert!(path.ends_with("cover-test_cover.jpg"), "缓存路径应为 jpg: {path}");
        assert!(
            !dir.join("cover-test_cover.png").exists(),
            "播放缓存不应生成 png"
        );
        let jpeg = std::fs::read(&path).unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "输出应为 JPEG 数据");
        assert!(image::load_from_memory(&jpeg).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tag_roundtrip() {
        let tag = build_tag(
            Some(7),
            vec![build_simple_tag("NBMUSIC_AUDIO_BINDINGS", "[]")],
        );
        let parsed = parse_tag(&tag[4..]).unwrap();
        assert_eq!(parsed.0, Some(7));
        assert_eq!(parsed.1[0].0, "NBMUSIC_AUDIO_BINDINGS");
    }

    #[test]
    fn font_and_image_detection() {
        assert_eq!(detect_image(&[0xFF, 0xD8, 0xFF]).unwrap().0, "image/jpeg");
        assert_eq!(font_mime(Path::new("x.otf"), b"OTTO").unwrap(), "font/otf");
    }
}
