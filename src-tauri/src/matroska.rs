//! Matroska（.mka）封装与解封装。
//!
//! 用途：
//! - 把「音频 + 字幕 + 封面」合成为一个 MKA 文件（无需外部 ffmpeg）；
//! - 从 MKA 中解出音频轨道，生成 WebView 可播放的临时音频文件（供音乐模块打开 MKA）。
//!
//! ## 音频
//!
//! 任意常见音频先经 Windows Media Foundation 转码为 AAC（ADTS），再按帧写入
//! Matroska（`A_AAC`）。MP3 源直接按帧复制（`A_MPEG/L3`），避免二次有损。
//! 转码失败且输入本身就是 MP3 时，回退到 MP3 帧解析。
//!
//! ## 字幕
//!
//! 支持 SRT / WebVTT / ASS / SSA：
//! - SRT  → `S_TEXT/UTF8`
//! - VTT  → `S_TEXT/WEBVTT`
//! - ASS  → `S_TEXT/ASS`（CodecPrivate 存脚本头，每个事件按 Matroska 约定存
//!   `ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,Text`）
//! - SSA  → `S_TEXT/SSA`
//!
//! ## 封面
//!
//! 作为 Matroska Attachment 写入（`cover.jpg` / `cover.png`），foobar2000、
//! MPC-HC、VLC 等播放器可直接显示。
//!
//! ## 解封装
//!
//! 只实现播放所需的最小 EBML 子集：Info / Tracks / Cluster / Attachments。
//! 支持无 lacing 的 SimpleBlock 与 BlockGroup；带 lacing 的音频块会明确报错，
//! 而不是静默产出损坏音频。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// ─────────────────────────── EBML / Matroska 常量 ───────────────────────────

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

const TRACK_TYPE_AUDIO: u64 = 2;
const TRACK_TYPE_SUBTITLE: u64 = 17;
const TIMECODE_SCALE_NS: u64 = 1_000_000; // 1 ms
const MAX_CLUSTER_MS: u64 = 30_000;

// ─────────────────────────── EBML 写入工具 ───────────────────────────

/// 写入 EBML 变长尺寸（最短编码，禁用「全 1」保留值）
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

// ─────────────────────────── 音轨数据结构 ───────────────────────────

struct AudioFrame {
    ts_ms: u64,
    dur_ms: u64,
    data: Vec<u8>,
}

struct MuxAudio {
    codec_id: String,
    codec_private: Vec<u8>,
    sample_rate: f64,
    channels: u64,
    default_duration_ns: u64,
    frames: Vec<AudioFrame>,
}

#[derive(Debug, Clone)]
pub struct SubtitleEvent {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

struct MuxSubtitle {
    codec_id: String,
    codec_private: Vec<u8>,
    events: Vec<SubtitleEvent>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MuxResult {
    pub output_path: String,
    pub audio_codec: String,
    pub subtitle_codec: Option<String>,
    pub has_cover: bool,
    pub duration_ms: u64,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct MuxOptions {
    pub audio_path: String,
    pub subtitle_path: Option<String>,
    pub cover_path: Option<String>,
    pub output_path: String,
    pub title: Option<String>,
}

// ─────────────────────────── 音频解码 / 转码 ───────────────────────────

const SAMPLE_RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

fn extension_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn is_adts(data: &[u8]) -> bool {
    data.len() >= 7 && data[0] == 0xFF && (data[1] & 0xF6) == 0xF0
}

/// 解析 ADTS AAC 文件为逐帧数据
fn parse_adts(data: &[u8]) -> Result<(Vec<u8>, u32, u32, Vec<AudioFrame>), String> {
    if data.len() < 7 {
        return Err("AAC 数据为空".to_string());
    }
    let mut pos = 0usize;
    let mut frames = Vec::new();
    let mut asc = [0u8; 2];
    let mut sample_rate = 44100u32;
    let mut channels = 2u32;
    let mut index = 0u64;

    while pos + 7 <= data.len() {
        if data[pos] != 0xFF || (data[pos + 1] & 0xF0) != 0xF0 {
            // 允许尾部填充 0；其它情况说明数据损坏
            if data[pos..].iter().all(|&b| b == 0) {
                break;
            }
            return Err(format!("AAC 帧同步失败（偏移 {pos}）"));
        }
        let protection_absent = data[pos + 1] & 1 == 1;
        let profile = (data[pos + 2] >> 6) & 0x03;
        let freq_index = ((data[pos + 2] >> 2) & 0x0F) as usize;
        let channel_config = (((data[pos + 2] & 0x01) << 2) | ((data[pos + 3] >> 6) & 0x03)) as u32;
        let frame_length = (((data[pos + 3] & 0x03) as usize) << 11)
            | ((data[pos + 4] as usize) << 3)
            | (((data[pos + 5] >> 5) & 0x07) as usize);
        let header_len = if protection_absent { 7 } else { 9 };
        if frame_length < header_len || pos + frame_length > data.len() {
            return Err(format!("AAC 帧长度异常（偏移 {pos}）"));
        }
        let rate = *SAMPLE_RATES.get(freq_index).ok_or("AAC 采样率索引非法")?;
        if rate == 0 {
            return Err("AAC 采样率非法".to_string());
        }
        sample_rate = rate;
        channels = if channel_config == 0 { 2 } else { channel_config };
        let audio_object_type = profile + 1;
        asc = [
            ((audio_object_type & 0x1F) << 3) | ((freq_index as u8 >> 1) & 0x07),
            ((freq_index as u8 & 0x01) << 7) | ((channels as u8 & 0x0F) << 3),
        ];
        let dur_ms = (1024.0 * 1000.0 / rate as f64).round() as u64;
        let ts_ms = (index as f64 * 1024.0 * 1000.0 / rate as f64).round() as u64;
        frames.push(AudioFrame {
            ts_ms,
            dur_ms,
            data: data[pos + header_len..pos + frame_length].to_vec(),
        });
        index += 1;
        pos += frame_length;
    }

    if frames.is_empty() {
        return Err("未从 AAC 数据中解析到音频帧".to_string());
    }
    Ok((asc.to_vec(), sample_rate, channels, frames))
}

fn skip_id3(data: &[u8]) -> usize {
    if data.len() >= 10 && &data[..3] == b"ID3" {
        let size = ((data[6] as usize & 0x7F) << 21)
            | ((data[7] as usize & 0x7F) << 14)
            | ((data[8] as usize & 0x7F) << 7)
            | (data[9] as usize & 0x7F);
        10 + size
    } else {
        0
    }
}

fn mp3_tables(version: u8, layer: u8) -> (&'static [u32; 16], &'static [u32], u32) {
    // version: 3=MPEG1, 2=MPEG2, 0=MPEG2.5；layer: 1=III, 2=II, 3=I
    match (version, layer) {
        (3, 1) => (
            &[
                0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
            ],
            &[44100, 48000, 32000],
            1152,
        ),
        (3, 2) => (
            &[
                0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 0,
            ],
            &[44100, 48000, 32000],
            1152,
        ),
        (3, 3) => (
            &[
                0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448, 0,
            ],
            &[44100, 48000, 32000],
            384,
        ),
        (2 | 0, 1) => (
            &[
                0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
            ],
            &[22050, 24000, 16000],
            576,
        ),
        _ => (
            &[
                0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
            ],
            &[11025, 12000, 8000],
            576,
        ),
    }
}

/// 解析 MP3 帧（Layer III 为主，Layer I/II 也可按帧边界切分）
fn parse_mp3(data: &[u8]) -> Result<(u32, u32, Vec<AudioFrame>), String> {
    let start = skip_id3(data).min(data.len());
    let mut pos = start;
    let mut frames = Vec::new();
    let mut sample_rate = 44100u32;
    let mut channels = 2u32;

    while pos + 4 <= data.len() {
        if data[pos] != 0xFF || (data[pos + 1] & 0xE0) != 0xE0 {
            pos += 1;
            continue;
        }
        let version = (data[pos + 1] >> 3) & 0x03;
        let layer = (data[pos + 1] >> 1) & 0x03;
        if version == 1 || layer == 0 {
            pos += 1;
            continue;
        }
        let bitrate_index = ((data[pos + 2] >> 4) & 0x0F) as usize;
        let rate_index = ((data[pos + 2] >> 2) & 0x03) as usize;
        let padding = ((data[pos + 2] >> 1) & 0x01) as usize;
        if bitrate_index == 0 || bitrate_index == 15 || rate_index == 3 {
            pos += 1;
            continue;
        }
        let (bitrates, rates, samples) = mp3_tables(version, layer);
        let bitrate = bitrates[bitrate_index] as usize * 1000;
        let rate = rates[rate_index];
        if rate == 0 {
            pos += 1;
            continue;
        }
        let frame_length = match layer {
            3 => (12 * bitrate / rate as usize + padding) * 4,
            2 => 144 * bitrate / rate as usize + padding,
            _ => {
                let coefficient = if version == 3 { 144 } else { 72 };
                coefficient * bitrate / rate as usize + padding
            }
        };
        if frame_length < 4 || pos + frame_length > data.len() {
            break;
        }
        let dur_ms = samples as f64 * 1000.0 / rate as f64;
        let ts_ms = (frames.len() as f64 * dur_ms).round() as u64;
        sample_rate = rate;
        channels = if data[pos + 3] >> 6 == 3 { 1 } else { 2 };
        frames.push(AudioFrame {
            ts_ms,
            dur_ms: dur_ms.round().max(1.0) as u64,
            data: data[pos..pos + frame_length].to_vec(),
        });
        pos += frame_length;
    }

    if frames.is_empty() {
        return Err("未从 MP3 中解析到音频帧".to_string());
    }
    Ok((sample_rate, channels, frames))
}

fn temp_path(suffix: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("neobox_{}_{}.{}", std::process::id(), stamp, suffix))
}

/// 读取任意音频并准备为 Matroska 可写入的帧序列
fn prepare_audio(path: &str) -> Result<MuxAudio, String> {
    let ext = extension_of(path);
    let data = std::fs::read(path).map_err(|e| format!("读取音频失败: {e}"))?;

    if !data.is_empty() {
        // 已经是 ADTS AAC：直接解析，避免重复转码
        if is_adts(&data) {
            if let Ok((asc, sample_rate, channels, frames)) = parse_adts(&data) {
                let default_ns =
                    (frames.first().map(|f| f.dur_ms).unwrap_or(0) as f64 * 1_000_000.0) as u64;
                return Ok(MuxAudio {
                    codec_id: "A_AAC".to_string(),
                    codec_private: asc,
                    sample_rate: sample_rate as f64,
                    channels: channels as u64,
                    default_duration_ns: default_ns,
                    frames,
                });
            }
        }
        // MP3 直接复制帧，避免二次有损编码
        if ext == "mp3" || (data[0] == 0xFF && (data[1] & 0xE0) == 0xE0) {
            if let Ok((sample_rate, channels, frames)) = parse_mp3(&data) {
                let samples = if (data[1] >> 3) & 0x03 == 3 { 1152.0 } else { 576.0 };
                let default_ns = (samples * 1_000_000_000.0 / sample_rate as f64) as u64;
                return Ok(MuxAudio {
                    codec_id: "A_MPEG/L3".to_string(),
                    codec_private: Vec::new(),
                    sample_rate: sample_rate as f64,
                    channels: channels as u64,
                    default_duration_ns: default_ns,
                    frames,
                });
            }
        }
    }

    // 其余格式：Media Foundation 解码为 PCM 后编码成 ADTS AAC
    let temp = temp_path("aac");
    let temp_str = temp.to_string_lossy().to_string();
    let transcode = crate::file_convert::transcode_to_adts(path, &temp_str);
    let result = match transcode {
        Ok(()) => std::fs::read(&temp)
            .map_err(|e| format!("读取转码结果失败: {e}"))
            .and_then(|adts| {
                parse_adts(&adts).map(|(asc, sample_rate, channels, frames)| {
                    let default_ns =
                        (frames.first().map(|f| f.dur_ms).unwrap_or(0) as f64 * 1_000_000.0) as u64;
                    MuxAudio {
                        codec_id: "A_AAC".to_string(),
                        codec_private: asc,
                        sample_rate: sample_rate as f64,
                        channels: channels as u64,
                        default_duration_ns: default_ns,
                        frames,
                    }
                })
            }),
        Err(e) => {
            // 转码失败：MP3 再试一次直接解析（有些系统缺少对应解码器）
            if let Ok((sample_rate, channels, frames)) = parse_mp3(&data) {
                let default_ns = (1152.0 * 1_000_000_000.0 / sample_rate as f64) as u64;
                Ok(MuxAudio {
                    codec_id: "A_MPEG/L3".to_string(),
                    codec_private: Vec::new(),
                    sample_rate: sample_rate as f64,
                    channels: channels as u64,
                    default_duration_ns: default_ns,
                    frames,
                })
            } else {
                Err(format!(
                    "无法解码该音频格式（需要系统 Media Foundation 解码器）：{e}"
                ))
            }
        }
    };
    let _ = std::fs::remove_file(&temp);
    result
}

// ─────────────────────────── 字幕解析 ───────────────────────────

fn parse_timecode(raw: &str) -> Option<u64> {
    // 支持 HH:MM:SS,mmm / HH:MM:SS.mmm / MM:SS.mmm
    let cleaned = raw.trim().replace(',', ".");
    let parts: Vec<&str> = cleaned.split(':').collect();
    let (h, m, rest) = match parts.as_slice() {
        [h, m, s] => (h.parse::<u64>().ok()?, m.parse::<u64>().ok()?, *s),
        [m, s] => (0, m.parse::<u64>().ok()?, *s),
        _ => return None,
    };
    let (sec, ms) = rest.split_once('.').unwrap_or((rest, "0"));
    let sec = sec.parse::<u64>().ok()?;
    let ms = {
        let mut digits = ms.trim().to_string();
        while digits.len() < 3 {
            digits.push('0');
        }
        digits.truncate(3);
        digits.parse::<u64>().ok()?
    };
    Some(((h * 3600 + m * 60 + sec) * 1000) + ms)
}

fn parse_srt(content: &str) -> Vec<SubtitleEvent> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let mut events = Vec::new();
    for block in normalized.split("\n\n") {
        let mut lines = block.lines().map(str::trim_end).collect::<Vec<_>>();
        if lines.is_empty() {
            continue;
        }
        // 首行可能是序号
        let time_index = lines.iter().position(|line| line.contains("-->"));
        let Some(index) = time_index else { continue };
        let time_line = lines[index];
        let Some((start, end)) = time_line.split_once("-->") else {
            continue;
        };
        let (Some(start_ms), Some(end_ms)) = (parse_timecode(start), parse_timecode(end)) else {
            continue;
        };
        lines.drain(..=index);
        let text = lines.join("\n").trim().to_string();
        if text.is_empty() {
            continue;
        }
        events.push(SubtitleEvent {
            start_ms,
            end_ms: end_ms.max(start_ms + 1),
            text,
        });
    }
    events.sort_by_key(|event| event.start_ms);
    events
}

fn parse_vtt(content: &str) -> Vec<SubtitleEvent> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let mut events = Vec::new();
    for block in normalized.split("\n\n") {
        if block.trim_start().starts_with("WEBVTT") || block.trim_start().starts_with("NOTE") {
            continue;
        }
        let mut lines = block.lines().map(str::trim_end).collect::<Vec<_>>();
        let Some(index) = lines.iter().position(|line| line.contains("-->")) else {
            continue;
        };
        let time_line = lines[index];
        let Some((start, rest)) = time_line.split_once("-->") else {
            continue;
        };
        // 结束时间后可能跟 cue 设置，取第一段
        let end = rest.split_whitespace().next().unwrap_or("");
        let (Some(start_ms), Some(end_ms)) = (parse_timecode(start), parse_timecode(end)) else {
            continue;
        };
        lines.drain(..=index);
        let text = lines.join("\n").trim().to_string();
        if text.is_empty() {
            continue;
        }
        events.push(SubtitleEvent {
            start_ms,
            end_ms: end_ms.max(start_ms + 1),
            text,
        });
    }
    events.sort_by_key(|event| event.start_ms);
    events
}

struct AssDialogue {
    start_ms: u64,
    end_ms: u64,
    fields: Vec<String>,
    format_fields: Vec<String>,
}

/// 解析 ASS/SSA：返回（脚本头、事件、是否 SSA）
fn parse_ass(content: &str) -> Result<(String, Vec<AssDialogue>, bool), String> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let is_ssa = normalized.contains("[V4 Styles]") && !normalized.contains("[V4+ Styles]");
    let mut in_events = false;
    let mut header_lines: Vec<String> = Vec::new();
    let mut format_fields: Vec<String> = Vec::new();
    let mut events = Vec::new();

    for line in normalized.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            let entering = trimmed.eq_ignore_ascii_case("[Events]");
            in_events = entering;
            header_lines.push(line.to_string());
            continue;
        }
        if !in_events {
            header_lines.push(line.to_string());
            continue;
        }
        if trimmed.to_ascii_lowercase().starts_with("format:") {
            format_fields = trimmed[7..]
                .split(',')
                .map(|field| field.trim().to_string())
                .collect();
            header_lines.push(line.to_string());
            continue;
        }
        if trimmed.to_ascii_lowercase().starts_with("dialogue:") {
            if format_fields.is_empty() {
                return Err("ASS 字幕缺少 Format 行".to_string());
            }
            let body = &trimmed[9..];
            // Text 字段可能包含逗号：按字段数限制切分
            let mut parts: Vec<String> = Vec::new();
            let mut rest = body;
            for _ in 0..format_fields.len().saturating_sub(1) {
                if let Some((head, tail)) = rest.split_once(',') {
                    parts.push(head.trim().to_string());
                    rest = tail;
                } else {
                    parts.push(rest.trim().to_string());
                    rest = "";
                }
            }
            parts.push(rest.trim().to_string());
            if parts.len() < format_fields.len() {
                continue;
            }
            let find = |name: &str| {
                format_fields
                    .iter()
                    .position(|field| field.eq_ignore_ascii_case(name))
            };
            let (Some(start_index), Some(end_index)) = (find("Start"), find("End")) else {
                continue;
            };
            let (Some(start_ms), Some(end_ms)) = (
                parse_timecode(&parts[start_index]),
                parse_timecode(&parts[end_index]),
            ) else {
                continue;
            };
            events.push(AssDialogue {
                start_ms,
                end_ms: end_ms.max(start_ms + 1),
                fields: parts,
                format_fields: format_fields.clone(),
            });
            continue;
        }
        // [Events] 下的注释/空行：保留在头部会让 CodecPrivate 混入事件，跳过
    }

    if events.is_empty() {
        return Err("ASS/SSA 字幕中没有可识别的事件".to_string());
    }
    Ok((header_lines.join("\n") + "\n", events, is_ssa))
}

fn prepare_subtitle(path: &str) -> Result<MuxSubtitle, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("读取字幕失败: {e}"))?;
    let ext = extension_of(path);

    if ext == "ass" || ext == "ssa" || content.contains("[Script Info]") {
        let (header, events, is_ssa) = parse_ass(&content)?;
        let codec_id = if is_ssa { "S_TEXT/SSA" } else { "S_TEXT/ASS" }.to_string();
        let mut out_events = Vec::new();
        for (index, event) in events.iter().enumerate() {
            let field = |name: &str| {
                event
                    .format_fields
                    .iter()
                    .position(|f| f.eq_ignore_ascii_case(name))
                    .and_then(|i| event.fields.get(i))
                    .cloned()
                    .unwrap_or_default()
            };
            let text = field("Text");
            // 去掉 ASS 覆盖标签对普通播放器更友好？保留原样，保持样式语义
            if text.trim().is_empty() {
                continue;
            }
            let block = if is_ssa {
                // SSA：保持原字段顺序，去掉 Start/End
                let mut fields = Vec::new();
                for (i, name) in event.format_fields.iter().enumerate() {
                    if name.eq_ignore_ascii_case("Start") || name.eq_ignore_ascii_case("End") {
                        continue;
                    }
                    fields.push(event.fields.get(i).cloned().unwrap_or_default());
                }
                fields.join(",")
            } else {
                format!(
                    "{},{},{},{},{},{},{},{},{}",
                    index,
                    field("Layer"),
                    field("Style"),
                    field("Name"),
                    field("MarginL"),
                    field("MarginR"),
                    field("MarginV"),
                    field("Effect"),
                    text
                )
            };
            out_events.push(SubtitleEvent {
                start_ms: event.start_ms,
                end_ms: event.end_ms,
                text: block,
            });
        }
        return Ok(MuxSubtitle {
            codec_id,
            codec_private: header.into_bytes(),
            events: out_events,
        });
    }

    if ext == "vtt" || content.trim_start().starts_with("WEBVTT") {
        return Ok(MuxSubtitle {
            codec_id: "S_TEXT/WEBVTT".to_string(),
            codec_private: Vec::new(),
            events: parse_vtt(&content),
        });
    }

    // 默认按 SRT 解析；也兼容无扩展名的字幕文本
    let events = parse_srt(&content);
    if events.is_empty() {
        return Err("字幕文件为空或格式无法识别".to_string());
    }
    Ok(MuxSubtitle {
        codec_id: "S_TEXT/UTF8".to_string(),
        codec_private: Vec::new(),
        events,
    })
}

// ─────────────────────────── 封面 ───────────────────────────

fn detect_image_type(data: &[u8]) -> Option<(&'static str, &'static str)> {
    if data.len() >= 8 && data[..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        Some(("image/png", "cover.png"))
    } else if data.len() >= 3 && data[..3] == [0xFF, 0xD8, 0xFF] {
        Some(("image/jpeg", "cover.jpg"))
    } else if data.len() >= 6 && (&data[..6] == b"GIF87a" || &data[..6] == b"GIF89a") {
        Some(("image/gif", "cover.gif"))
    } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some(("image/webp", "cover.webp"))
    } else {
        None
    }
}

// ─────────────────────────── MKA 写入 ───────────────────────────

fn build_track_entry(
    number: u64,
    uid: u64,
    track_type: u64,
    codec_id: &str,
    codec_private: &[u8],
    name: &str,
    audio: Option<(f64, u64)>,
    default_duration_ns: Option<u64>,
) -> Vec<u8> {
    let mut fields = vec![
        uint_elem(ID_TRACK_NUMBER, number),
        uint_elem(ID_TRACK_UID, uid),
        uint_elem(ID_TRACK_TYPE, track_type),
        uint_elem(ID_FLAG_LACING, 0),
        str_elem(ID_CODEC_ID, codec_id),
        str_elem(ID_LANGUAGE, "und"),
        str_elem(ID_TRACK_NAME, name),
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

fn build_block(track: u64, rel: i16, data: &[u8], keyframe: bool, duration: Option<u64>) -> Vec<u8> {
    let mut payload = Vec::with_capacity(4 + data.len());
    payload.push(0x80 | (track as u8 & 0x7F));
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

pub fn mux_mka(options: &MuxOptions) -> Result<MuxResult, String> {
    let audio = prepare_audio(&options.audio_path)?;
    let subtitle = match options.subtitle_path.as_deref() {
        Some(path) if !path.trim().is_empty() => Some(prepare_subtitle(path)?),
        _ => None,
    };
    let cover = match options.cover_path.as_deref() {
        Some(path) if !path.trim().is_empty() => {
            let data = std::fs::read(path).map_err(|e| format!("读取封面失败: {e}"))?;
            if data.is_empty() {
                return Err("封面文件为空".to_string());
            }
            let Some((mime, name)) = detect_image_type(&data) else {
                return Err("封面仅支持 PNG / JPEG / GIF / WebP".to_string());
            };
            Some((mime, name, data))
        }
        _ => None,
    };

    // 收集所有块并按时间排序，保证簇内时间码单调
    enum Block {
        Audio { ts: u64, dur: u64, data: usize },
        Subtitle { ts: u64, dur: u64, index: usize },
    }
    let mut blocks: Vec<Block> = Vec::with_capacity(audio.frames.len() + 64);
    for (index, frame) in audio.frames.iter().enumerate() {
        blocks.push(Block::Audio {
            ts: frame.ts_ms,
            dur: frame.dur_ms,
            data: index,
        });
    }
    if let Some(subtitle) = &subtitle {
        for (index, event) in subtitle.events.iter().enumerate() {
            blocks.push(Block::Subtitle {
                ts: event.start_ms,
                dur: event.end_ms.saturating_sub(event.start_ms),
                index,
            });
        }
    }
    blocks.sort_by_key(|block| match block {
        Block::Audio { ts, .. } => (*ts, 0u8),
        Block::Subtitle { ts, .. } => (*ts, 1u8),
    });

    let audio_track_number = 1u64;
    let subtitle_track_number = 2u64;
    let audio_codec_id = audio.codec_id.clone();
    let subtitle_codec_id = subtitle.as_ref().map(|s| s.codec_id.clone());

    let mut cluster_payload_blocks: Vec<u8> = Vec::new();
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

    let mut max_end_ms = 0u64;
    for block in &blocks {
        let (ts, dur) = match block {
            Block::Audio { ts, dur, .. } => (*ts, *dur),
            Block::Subtitle { ts, dur, .. } => (*ts, *dur),
        };
        max_end_ms = max_end_ms.max(ts + dur);
        let base = *cluster_base.get_or_insert(ts);
        if ts.saturating_sub(base) > MAX_CLUSTER_MS && !cluster_payload_blocks.is_empty() {
            flush_cluster(base, &mut cluster_payload_blocks, &mut clusters, &mut cluster_count);
            cluster_base = Some(ts);
        }
        let base = cluster_base.unwrap_or(ts);
        let rel = ts.saturating_sub(base).min(i16::MAX as u64) as i16;
        match block {
            Block::Audio { data, .. } => {
                cluster_payload_blocks.extend_from_slice(&build_block(
                    audio_track_number,
                    rel,
                    &audio.frames[*data].data,
                    true,
                    None,
                ));
            }
            Block::Subtitle { index, .. } => {
                if let Some(subtitle) = &subtitle {
                    if let Some(event) = subtitle.events.get(*index) {
                        cluster_payload_blocks.extend_from_slice(&build_block(
                            subtitle_track_number,
                            rel,
                            event.text.as_bytes(),
                            false,
                            Some(dur),
                        ));
                    }
                }
            }
        }
    }
    if let Some(base) = cluster_base {
        flush_cluster(base, &mut cluster_payload_blocks, &mut clusters, &mut cluster_count);
    }

    let mut info_fields = vec![
        uint_elem(ID_TIMECODE_SCALE, TIMECODE_SCALE_NS),
        float_elem(ID_DURATION, max_end_ms as f64),
        str_elem(ID_MUXING_APP, "NeoBox"),
        str_elem(ID_WRITING_APP, "NeoBox"),
    ];
    if let Some(title) = options.title.as_deref().filter(|title| !title.trim().is_empty()) {
        info_fields.push(str_elem(ID_TITLE, title));
    }
    let info = elem(ID_INFO, &concat(&info_fields));

    let mut track_entries = vec![build_track_entry(
        audio_track_number,
        1,
        TRACK_TYPE_AUDIO,
        &audio.codec_id,
        &audio.codec_private,
        "Audio",
        Some((audio.sample_rate, audio.channels)),
        Some(audio.default_duration_ns),
    )];
    if let Some(subtitle) = &subtitle {
        track_entries.push(build_track_entry(
            subtitle_track_number,
            2,
            TRACK_TYPE_SUBTITLE,
            &subtitle.codec_id,
            &subtitle.codec_private,
            "Subtitle",
            None,
            None,
        ));
    }
    let tracks = elem(ID_TRACKS, &concat(&track_entries));

    let attachments = if let Some((mime, name, data)) = &cover {
        let fields = vec![
            str_elem(ID_FILE_DESCRIPTION, "Cover"),
            str_elem(ID_FILE_NAME, name),
            str_elem(ID_FILE_MIME_TYPE, mime),
            uint_elem(ID_FILE_UID, 1),
            elem(ID_FILE_DATA, data),
        ];
        elem(ID_ATTACHMENTS, &elem(ID_ATTACHED_FILE, &concat(&fields)))
    } else {
        Vec::new()
    };

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

    let segment_body = concat(&[info, tracks, attachments, clusters]);
    let segment = elem(ID_SEGMENT, &segment_body);
    let mut file = Vec::with_capacity(ebml.len() + segment.len());
    file.extend_from_slice(&ebml);
    file.extend_from_slice(&segment);

    std::fs::write(&options.output_path, &file).map_err(|e| format!("写入 MKA 失败: {e}"))?;
    let size = std::fs::metadata(&options.output_path)
        .map(|meta| meta.len())
        .unwrap_or(file.len() as u64);

    log::info!(
        "[Matroska] 合成 {}：音频 {} 帧，字幕 {} 条，封面 {}，簇 {}，{} 字节",
        options.output_path,
        audio.frames.len(),
        subtitle.as_ref().map(|s| s.events.len()).unwrap_or(0),
        cover.is_some(),
        cluster_count,
        size
    );

    Ok(MuxResult {
        output_path: options.output_path.clone(),
        audio_codec: audio_codec_id,
        subtitle_codec: subtitle_codec_id,
        has_cover: cover.is_some(),
        duration_ms: max_end_ms,
        size,
    })
}

// ─────────────────────────── MKA 读取 ───────────────────────────

fn read_vint_id(buf: &[u8], pos: &mut usize) -> Result<u32, String> {
    if *pos >= buf.len() {
        return Err("EBML 数据意外结束".to_string());
    }
    let first = buf[*pos];
    let len = first.leading_zeros() as usize + 1;
    if len > 4 || *pos + len > buf.len() {
        return Err("EBML 元素 ID 非法".to_string());
    }
    let mut id = 0u32;
    for i in 0..len {
        id = (id << 8) | buf[*pos + i] as u32;
    }
    *pos += len;
    Ok(id)
}

fn read_vint_size(buf: &[u8], pos: &mut usize) -> Result<Option<u64>, String> {
    if *pos >= buf.len() {
        return Err("EBML 数据意外结束".to_string());
    }
    let first = buf[*pos];
    if first == 0 {
        return Err("EBML 尺寸非法".to_string());
    }
    let len = first.leading_zeros() as usize + 1;
    if *pos + len > buf.len() {
        return Err("EBML 尺寸越界".to_string());
    }
    let mut value = (first as u64) & ((1u64 << (8 - len)) - 1);
    let mut all_ones = value == ((1u64 << (8 - len)) - 1);
    for i in 1..len {
        let byte = buf[*pos + i];
        value = (value << 8) | byte as u64;
        if byte != 0xFF {
            all_ones = false;
        }
    }
    *pos += len;
    Ok(if all_ones { None } else { Some(value) })
}

fn read_uint(buf: &[u8]) -> u64 {
    buf.iter().fold(0u64, |acc, &b| (acc << 8) | b as u64)
}

fn read_float(buf: &[u8]) -> f64 {
    match buf.len() {
        4 => {
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(buf);
            f32::from_be_bytes(bytes) as f64
        }
        8 => {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(buf);
            f64::from_be_bytes(bytes)
        }
        _ => 0.0,
    }
}

#[derive(Debug, Clone)]
pub struct DemuxAudio {
    pub codec_id: String,
    pub codec_private: Vec<u8>,
    pub sample_rate: f64,
    pub channels: u64,
    pub bit_depth: u64,
    /// (时间戳 ms, 原始帧数据)
    pub frames: Vec<(u64, Vec<u8>)>,
    pub duration_ms: u64,
    pub cover: Option<(String, Vec<u8>)>,
    /// 内嵌字幕（用于音乐模块歌词面板 / 桌面歌词）
    pub subtitle: Option<DemuxSubtitle>,
}

#[derive(Debug, Clone)]
pub struct DemuxSubtitle {
    pub codec_id: String,
    pub events: Vec<SubtitleEvent>,
}

struct ParsedTrack {
    number: u64,
    track_type: u64,
    codec_id: String,
    codec_private: Vec<u8>,
    sample_rate: f64,
    channels: u64,
    bit_depth: u64,
}

struct RawBlock {
    track: u64,
    ts: u64,
    data: Vec<u8>,
    /// 字幕块时长（BlockGroup 的 BlockDuration）；音频块为 None
    duration_ms: Option<u64>,
}

fn parse_track_entry(body: &[u8]) -> Option<ParsedTrack> {
    let mut pos = 0usize;
    let mut track = ParsedTrack {
        number: 0,
        track_type: 0,
        codec_id: String::new(),
        codec_private: Vec::new(),
        sample_rate: 0.0,
        channels: 0,
        bit_depth: 0,
    };
    while pos < body.len() {
        let id = read_vint_id(body, &mut pos).ok()?;
        let size = read_vint_size(body, &mut pos).ok()?? as usize;
        if pos + size > body.len() {
            return None;
        }
        let value = &body[pos..pos + size];
        match id {
            0xD7 => track.number = read_uint(value),
            0x83 => track.track_type = read_uint(value),
            0x86 => track.codec_id = String::from_utf8_lossy(value).to_string(),
            0x63A2 => track.codec_private = value.to_vec(),
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

fn parse_block_payload<'a>(payload: &'a [u8], cluster_ts: u64) -> Option<(u64, u64, u8, &'a [u8])> {
    if payload.len() < 4 {
        return None;
    }
    let mut pos = 0usize;
    // 轨道号 vint（第一字节的 marker 位去掉）
    let first = payload[pos];
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || pos + len > payload.len() {
        return None;
    }
    let mut track = (first as u64) & ((1u64 << (8 - len)) - 1);
    for i in 1..len {
        track = (track << 8) | payload[pos + i] as u64;
    }
    pos += len;
    if pos + 3 > payload.len() {
        return None;
    }
    let rel = i16::from_be_bytes([payload[pos], payload[pos + 1]]);
    pos += 2;
    let flags = payload[pos];
    pos += 1;
    let ts = (cluster_ts as i64 + rel as i64).max(0) as u64;
    Some((track, ts, flags, &payload[pos..]))
}

pub fn demux_mka(path: &str) -> Result<DemuxAudio, String> {
    let data = std::fs::read(path).map_err(|e| format!("读取 MKA 失败: {e}"))?;
    let mut pos = 0usize;
    let mut tracks: Vec<ParsedTrack> = Vec::new();
    let mut blocks: Vec<RawBlock> = Vec::new();
    let mut duration_ms = 0u64;
    let mut cover: Option<(String, Vec<u8>)> = None;

    while pos < data.len() {
        let id = read_vint_id(&data, &mut pos)?;
        let size = read_vint_size(&data, &mut pos)?;
        let end = match size {
            Some(size) => (pos + size as usize).min(data.len()),
            None => data.len(),
        };
        let body = &data[pos..end];
        match id {
            0x1A45DFA3 => {}
            0x18538067 => {
                // Segment：递归解析子元素（支持未知长度）
                let mut sub = 0usize;
                while sub < body.len() {
                    let sub_id = read_vint_id(body, &mut sub)?;
                    let sub_size = read_vint_size(body, &mut sub)?;
                    let sub_end = match sub_size {
                        Some(size) => (sub + size as usize).min(body.len()),
                        None => body.len(),
                    };
                    let sub_body = &body[sub..sub_end];
                    match sub_id {
                        0x1549A966 => {
                            let mut info_pos = 0usize;
                            while info_pos < sub_body.len() {
                                let info_id = read_vint_id(sub_body, &mut info_pos)?;
                                let info_size = read_vint_size(sub_body, &mut info_pos)?;
                                let info_end = match info_size {
                                    Some(size) => (info_pos + size as usize).min(sub_body.len()),
                                    None => sub_body.len(),
                                };
                                if info_id == 0x4489 {
                                    let ms = read_float(&sub_body[info_pos..info_end]);
                                    if ms.is_finite() && ms > 0.0 {
                                        duration_ms = ms as u64;
                                    }
                                }
                                info_pos = info_end;
                            }
                        }
                        0x1654AE6B => {
                            let mut tracks_pos = 0usize;
                            while tracks_pos < sub_body.len() {
                                let track_id = read_vint_id(sub_body, &mut tracks_pos)?;
                                let track_size = read_vint_size(sub_body, &mut tracks_pos)?;
                                let track_end = match track_size {
                                    Some(size) => {
                                        (tracks_pos + size as usize).min(sub_body.len())
                                    }
                                    None => sub_body.len(),
                                };
                                if track_id == 0xAE {
                                    if let Some(track) =
                                        parse_track_entry(&sub_body[tracks_pos..track_end])
                                    {
                                        tracks.push(track);
                                    }
                                }
                                tracks_pos = track_end;
                            }
                        }
                        0x1F43B675 => {
                            let mut cluster_ts = 0u64;
                            let mut cluster_pos = 0usize;
                            while cluster_pos < sub_body.len() {
                                let cluster_id = read_vint_id(sub_body, &mut cluster_pos)?;
                                let cluster_size =
                                    read_vint_size(sub_body, &mut cluster_pos)?;
                                let cluster_end = match cluster_size {
                                    Some(size) => {
                                        (cluster_pos + size as usize).min(sub_body.len())
                                    }
                                    None => sub_body.len(),
                                };
                                let cluster_body = &sub_body[cluster_pos..cluster_end];
                                match cluster_id {
                                    0xE7 => cluster_ts = read_uint(cluster_body),
                                    0xA3 => {
                                        if let Some((track, ts, flags, payload)) =
                                            parse_block_payload(cluster_body, cluster_ts)
                                        {
                                            // lacing 位：0b0000_0110 非零表示分片，暂不支持
                                            if flags & 0x06 == 0 {
                                                blocks.push(RawBlock {
                                                    track,
                                                    ts,
                                                    data: payload.to_vec(),
                                                    duration_ms: None,
                                                });
                                            }
                                        }
                                    }
                                    0xA0 => {
                                        let mut group_pos = 0usize;
                                        let mut block_payload: Option<Vec<u8>> = None;
                                        let mut block_duration: Option<u64> = None;
                                        while group_pos < cluster_body.len() {
                                            let group_id =
                                                read_vint_id(cluster_body, &mut group_pos)?;
                                            let group_size =
                                                read_vint_size(cluster_body, &mut group_pos)?;
                                            let group_end = match group_size {
                                                Some(size) => (group_pos + size as usize)
                                                    .min(cluster_body.len()),
                                                None => cluster_body.len(),
                                            };
                                            if group_id == 0xA1 {
                                                block_payload = Some(
                                                    cluster_body[group_pos..group_end].to_vec(),
                                                );
                                            } else if group_id == 0x9B {
                                                block_duration = Some(read_uint(
                                                    &cluster_body[group_pos..group_end],
                                                ));
                                            }
                                            group_pos = group_end;
                                        }
                                        if let Some(payload) = block_payload {
                                            if let Some((track, ts, flags, body)) =
                                                parse_block_payload(&payload, cluster_ts)
                                            {
                                                if flags & 0x06 == 0 {
                                                    blocks.push(RawBlock {
                                                        track,
                                                        ts,
                                                        data: body.to_vec(),
                                                        duration_ms: block_duration,
                                                    });
                                                }
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                                cluster_pos = cluster_end;
                            }
                        }
                        0x1941A469 => {
                            let mut att_pos = 0usize;
                            while att_pos < sub_body.len() {
                                let att_id = read_vint_id(sub_body, &mut att_pos)?;
                                let att_size = read_vint_size(sub_body, &mut att_pos)?;
                                let att_end = match att_size {
                                    Some(size) => (att_pos + size as usize).min(sub_body.len()),
                                    None => sub_body.len(),
                                };
                                if att_id == 0x61A7 {
                                    let file_body = &sub_body[att_pos..att_end];
                                    let mut file_pos = 0usize;
                                    let mut file_name = String::new();
                                    let mut file_mime = String::new();
                                    let mut file_data: Option<Vec<u8>> = None;
                                    while file_pos < file_body.len() {
                                        let file_id =
                                            read_vint_id(file_body, &mut file_pos)?;
                                        let file_size =
                                            read_vint_size(file_body, &mut file_pos)?;
                                        let file_end = match file_size {
                                            Some(size) => (file_pos + size as usize)
                                                .min(file_body.len()),
                                            None => file_body.len(),
                                        };
                                        match file_id {
                                            0x466E => {
                                                file_name = String::from_utf8_lossy(
                                                    &file_body[file_pos..file_end],
                                                )
                                                .to_string()
                                            }
                                            0x4660 => {
                                                file_mime = String::from_utf8_lossy(
                                                    &file_body[file_pos..file_end],
                                                )
                                                .to_string()
                                            }
                                            0x465C => {
                                                file_data = Some(
                                                    file_body[file_pos..file_end].to_vec(),
                                                )
                                            }
                                            _ => {}
                                        }
                                        file_pos = file_end;
                                    }
                                    if let Some(data) = file_data {
                                        let mime = if file_mime.is_empty() {
                                            detect_image_type(&data)
                                                .map(|(mime, _)| mime.to_string())
                                                .unwrap_or_else(|| "image/jpeg".to_string())
                                        } else {
                                            file_mime
                                        };
                                        if !file_name.is_empty() || mime.starts_with("image/") {
                                            cover = Some((mime, data));
                                        }
                                    }
                                }
                                att_pos = att_end;
                            }
                        }
                        _ => {}
                    }
                    sub = sub_end;
                }
            }
            _ => {}
        }
        pos = end;
    }

    let audio_track = tracks
        .iter()
        .find(|track| track.track_type == TRACK_TYPE_AUDIO)
        .ok_or("MKA 中未找到音频轨道")?;

    // 内嵌字幕：按时间排序，BlockDuration 优先，缺失时用下一条开始时间
    let subtitle = tracks
        .iter()
        .find(|track| track.track_type == TRACK_TYPE_SUBTITLE)
        .and_then(|track| {
            let mut items: Vec<(u64, Option<u64>, Vec<u8>)> = blocks
                .iter()
                .filter(|block| block.track == track.number)
                .map(|block| (block.ts, block.duration_ms, block.data.clone()))
                .collect();
            if items.is_empty() {
                return None;
            }
            items.sort_by_key(|(ts, _, _)| *ts);
            let mut events = Vec::with_capacity(items.len());
            for index in 0..items.len() {
                let (start, duration, data) = &items[index];
                let end = duration
                    .map(|value| start.saturating_add(value))
                    .unwrap_or_else(|| {
                        items
                            .get(index + 1)
                            .map(|(next, _, _)| *next)
                            .unwrap_or_else(|| start.saturating_add(3000))
                    })
                    .max(start.saturating_add(1));
                let text = decode_subtitle_text(&track.codec_id, data);
                if !text.trim().is_empty() {
                    events.push(SubtitleEvent {
                        start_ms: *start,
                        end_ms: end,
                        text,
                    });
                }
            }
            if events.is_empty() {
                None
            } else {
                Some(DemuxSubtitle {
                    codec_id: track.codec_id.clone(),
                    events,
                })
            }
        });

    let mut frames: Vec<(u64, Vec<u8>)> = blocks
        .into_iter()
        .filter(|block| block.track == audio_track.number)
        .map(|block| (block.ts, block.data))
        .collect();
    frames.sort_by_key(|(ts, _)| *ts);
    if frames.is_empty() {
        return Err("MKA 中没有可用的音频数据".to_string());
    }
    if duration_ms == 0 {
        duration_ms = frames.last().map(|(ts, _)| *ts).unwrap_or(0);
    }

    Ok(DemuxAudio {
        codec_id: audio_track.codec_id.clone(),
        codec_private: audio_track.codec_private.clone(),
        sample_rate: if audio_track.sample_rate > 0.0 {
            audio_track.sample_rate
        } else {
            44100.0
        },
        channels: if audio_track.channels > 0 {
            audio_track.channels
        } else {
            2
        },
        bit_depth: audio_track.bit_depth,
        frames,
        duration_ms,
        cover,
        subtitle,
    })
}

/// 读取 Matroska 字幕块并转换为纯文本。
/// ASS/SSA 的块内容是 `ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,Text`
/// （SSA 少一层 ReadOrder），这里取出 Text 字段并去掉覆盖标签。
fn decode_subtitle_text(codec_id: &str, data: &[u8]) -> String {
    let codec = codec_id.to_ascii_uppercase();
    let raw = String::from_utf8_lossy(data).to_string();
    if codec.contains("ASS") || codec.contains("SSA") {
        let text = if codec.contains("ASS") {
            raw.splitn(9, ',').nth(8)
        } else {
            raw.splitn(8, ',').nth(7)
        }
        .unwrap_or(&raw);
        strip_ass_tags(text)
    } else {
        raw.replace("\\N", "\n").replace("\\n", "\n")
    }
}

fn strip_ass_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '{' => in_tag = true,
            '}' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("\\N", "\n")
        .replace("\\n", "\n")
        .replace("\\h", " ")
}

/// 字幕事件 → LRC（音乐模块歌词面板与桌面歌词直接复用）
pub fn subtitle_lrc_from_events(events: &[SubtitleEvent]) -> String {
    let mut out = String::new();
    for event in events {
        let text = event.text.replace('\r', " ").replace('\n', " ");
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let centis = event.start_ms / 10;
        let minutes = centis / 6000;
        let seconds = (centis / 100) % 60;
        let fraction = centis % 100;
        out.push_str(&format!("[{minutes:02}:{seconds:02}.{fraction:02}]{text}\n"));
    }
    out
}

/// 直接从 MKA 读取内嵌字幕并生成 LRC；没有字幕轨道时返回空字符串。
pub fn subtitle_lrc_from_path(path: &str) -> Result<String, String> {
    let demux = demux_mka(path)?;
    Ok(demux
        .subtitle
        .as_ref()
        .map(|subtitle| subtitle_lrc_from_events(&subtitle.events))
        .unwrap_or_default())
}

/// 把解出的音频写成 WebView / 音乐模块可播放的文件。
/// 返回值是实际写出的扩展名对应的路径。
pub fn write_playable_audio(demux: &DemuxAudio, output: &Path) -> Result<(), String> {
    let codec = demux.codec_id.to_ascii_uppercase();
    if codec.starts_with("A_AAC") {
        let asc = &demux.codec_private;
        if asc.len() < 2 {
            return Err("MKA 的 AAC 轨道缺少 AudioSpecificConfig".to_string());
        }
        let audio_object_type = (asc[0] >> 3) & 0x1F;
        if audio_object_type == 0 || audio_object_type >= 31 {
            return Err("不支持的 AAC 配置".to_string());
        }
        let freq_index = (((asc[0] & 0x07) as usize) << 1) | ((asc[1] >> 7) as usize);
        if freq_index >= SAMPLE_RATES.len() {
            return Err("AAC 采样率索引非法".to_string());
        }
        let channels = ((asc[1] >> 3) & 0x0F) as usize;
        let mut out = Vec::with_capacity(
            demux.frames.iter().map(|(_, data)| data.len() + 7).sum(),
        );
        for (_, frame) in &demux.frames {
            let frame_length = frame.len() + 7;
            if frame_length > 0x1FFF {
                return Err("AAC 帧过大".to_string());
            }
            let profile = (audio_object_type - 1) & 0x03;
            let chan_cfg = if channels == 0 { 2 } else { channels };
            out.push(0xFF);
            out.push(0xF1);
            out.push((profile << 6) | ((freq_index as u8 & 0x0F) << 2) | ((chan_cfg as u8 >> 2) & 0x01));
            out.push(((chan_cfg as u8 & 0x03) << 6) | ((frame_length >> 11) as u8 & 0x03));
            out.push(((frame_length >> 3) & 0xFF) as u8);
            out.push((((frame_length & 0x07) as u8) << 5) | 0x1F);
            out.push(0xFC);
            out.extend_from_slice(frame);
        }
        std::fs::write(output, out).map_err(|e| format!("写入 AAC 失败: {e}"))?;
        return Ok(());
    }

    if codec.starts_with("A_MPEG") {
        let mut out = Vec::new();
        for (_, frame) in &demux.frames {
            out.extend_from_slice(frame);
        }
        std::fs::write(output, out).map_err(|e| format!("写入 MP3 失败: {e}"))?;
        return Ok(());
    }

    if codec.starts_with("A_FLAC") {
        if demux.codec_private.len() < 34 {
            return Err("MKA 的 FLAC 轨道缺少 STREAMINFO".to_string());
        }
        let mut out = Vec::with_capacity(demux.frames.iter().map(|(_, d)| d.len()).sum::<usize>() + 42);
        out.extend_from_slice(b"fLaC");
        out.push(0x80); // last metadata block + STREAMINFO
        out.extend_from_slice(&[0x00, 0x00, 0x22]);
        out.extend_from_slice(&demux.codec_private[..34]);
        for (_, frame) in &demux.frames {
            out.extend_from_slice(frame);
        }
        std::fs::write(output, out).map_err(|e| format!("写入 FLAC 失败: {e}"))?;
        return Ok(());
    }

    if codec.starts_with("A_PCM") {
        let channels = demux.channels.max(1) as u16;
        let bits = if demux.bit_depth > 0 {
            demux.bit_depth.min(32) as u16
        } else {
            16
        };
        let sample_rate = demux.sample_rate.round() as u32;
        let block_align = channels * bits / 8;
        let byte_rate = sample_rate * block_align as u32;
        let mut pcm = Vec::new();
        for (_, frame) in &demux.frames {
            pcm.extend_from_slice(frame);
        }
        let data_len = pcm.len() as u32;
        let mut out = Vec::with_capacity(44 + pcm.len());
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&byte_rate.to_le_bytes());
        out.extend_from_slice(&block_align.to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        out.extend_from_slice(&pcm);
        std::fs::write(output, out).map_err(|e| format!("写入 WAV 失败: {e}"))?;
        return Ok(());
    }

    Err(format!(
        "暂不支持从 MKA 中提取 {} 音频（仅支持 AAC / MP3 / FLAC / PCM）",
        demux.codec_id
    ))
}

/// 供音乐模块调用：把 MKA 的音频解到缓存目录，返回可播放文件路径。
/// AAC 会再经 Media Foundation 封装为 M4A（WebView 兼容性最好）。
pub fn extract_playable_audio(
    mka_path: &str,
    cache_dir: &Path,
    cache_key: &str,
) -> Result<(String, Option<String>, u64), String> {
    let demux = demux_mka(mka_path)?;
    std::fs::create_dir_all(cache_dir).map_err(|e| format!("创建播放缓存目录失败: {e}"))?;

    // 内嵌字幕同步写为 LRC，音乐模块首次播放即可直接读取，避免重复解析整个 MKA
    if let Some(subtitle) = &demux.subtitle {
        log::info!(
            "[Matroska] 内嵌字幕 {} 条，编码 {}",
            subtitle.events.len(),
            subtitle.codec_id
        );
        let lrc = subtitle_lrc_from_events(&subtitle.events);
        if !lrc.is_empty() {
            let _ = std::fs::write(cache_dir.join(format!("{cache_key}.lrc")), lrc);
        }
    }

    let codec = demux.codec_id.to_ascii_uppercase();
    let (ext, needs_remux) = if codec.starts_with("A_AAC") {
        ("aac", true)
    } else if codec.starts_with("A_MPEG") {
        ("mp3", false)
    } else if codec.starts_with("A_FLAC") {
        ("flac", false)
    } else if codec.starts_with("A_PCM") {
        ("wav", false)
    } else {
        return Err(format!("暂不支持播放 {} 编码的 MKA", demux.codec_id));
    };

    let temp_audio = cache_dir.join(format!("{cache_key}.{ext}"));
    write_playable_audio(&demux, &temp_audio)?;

    let playable = if needs_remux {
        let m4a = cache_dir.join(format!("{cache_key}.m4a"));
        match crate::file_convert::convert_audio_impl(
            &temp_audio.to_string_lossy(),
            &m4a.to_string_lossy(),
        ) {
            Ok(()) => {
                let _ = std::fs::remove_file(&temp_audio);
                m4a
            }
            Err(e) => {
                log::warn!("[Matroska] AAC 封装 M4A 失败，回退直接播放 ADTS：{e}");
                temp_audio.clone()
            }
        }
    } else {
        temp_audio.clone()
    };

    // 封面写入缓存
    let cover_path = demux.cover.as_ref().and_then(|(mime, data)| {
        let ext = match mime.as_str() {
            "image/png" => "png",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => "jpg",
        };
        let path = cache_dir.join(format!("{cache_key}_cover.{ext}"));
        match std::fs::write(&path, data) {
            Ok(()) => Some(path.to_string_lossy().to_string()),
            Err(e) => {
                log::warn!("[Matroska] 写封面缓存失败：{e}");
                None
            }
        }
    });

    Ok((
        playable.to_string_lossy().to_string(),
        cover_path,
        demux.duration_ms,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_timecode() {
        assert_eq!(parse_timecode("00:00:01,500"), Some(1500));
        assert_eq!(parse_timecode("01:02:03.004"), Some(3_723_004));
        assert_eq!(parse_timecode("02:03.5"), Some(123_500));
    }

    #[test]
    fn srt_events() {
        let text = "1\n00:00:01,000 --> 00:00:02,000\n你好\n\n2\n00:00:03,000 --> 00:00:04,000\n世界\n第二行\n";
        let events = parse_srt(text);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].start_ms, 1000);
        assert_eq!(events[1].text, "世界\n第二行");
    }

    #[test]
    fn ass_events() {
        let text = "[Script Info]\nTitle: t\n\n[V4+ Styles]\nFormat: Name\nStyle: Default\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:02.50,Default,,0,0,0,,你好,世界\n";
        let (header, events, is_ssa) = parse_ass(text).unwrap();
        assert!(!is_ssa);
        assert!(header.contains("[Events]"));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].start_ms, 1000);
        assert_eq!(events[0].end_ms, 2500);
    }

    #[test]
    fn ebml_roundtrip_size() {
        let payload = vec![7u8; 130];
        let encoded = elem(&[0xA3], &payload);
        let mut pos = 0usize;
        let id = read_vint_id(&encoded, &mut pos).unwrap();
        assert_eq!(id, 0xA3);
        let size = read_vint_size(&encoded, &mut pos).unwrap();
        assert_eq!(size, Some(130));
    }
}



