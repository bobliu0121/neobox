//! 文件格式转换工具：
//! - 图片格式（png/jpg/jpeg/webp/bmp）相互转换（image crate）
//! - 音频格式（wav/mp3/m4a）相互转换（Windows Media Foundation 系统转码）
//!
//! 音频转换依赖 Windows 10/11 自带的 Media Foundation 编码器（MP3/AAC/PCM），
//! 无需外部 ffmpeg；输出容器与编码器由输出文件扩展名决定。

use std::path::Path;

use windows::core::{GUID, PCWSTR};
use windows::Win32::Media::MediaFoundation::{
    MFShutdown, MFStartup, MFCreateAttributes, MFCreateMediaType, MFCreateSinkWriterFromURL,
    MFCreateSourceReaderFromURL, MFAudioFormat_AAC, MFAudioFormat_MP3,
    MFAudioFormat_PCM, MFMediaType_Audio, MFSTARTUP_NOSOCKET,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MFTranscodeContainerType_ADTS,
    MF_TRANSCODE_CONTAINERTYPE,
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_MAJOR_TYPE,
    MF_MT_SUBTYPE, MF_VERSION, IMFAttributes, IMFSample,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

/// 图片格式转换：按目标格式解码并重新编码
#[tauri::command]
pub async fn convert_image(
    input_path: String,
    output_path: String,
    format: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let fmt = match format.to_lowercase().as_str() {
            "png" => image::ImageFormat::Png,
            "jpg" | "jpeg" => image::ImageFormat::Jpeg,
            "webp" => image::ImageFormat::WebP,
            "bmp" => image::ImageFormat::Bmp,
            _ => return Err(format!("不支持的图片格式: {format}")),
        };
        // 按文件内容魔数探测输入格式（不依赖扩展名），
        // 避免「扩展名 .jpg 实际是 PNG」等改名文件被按错误格式解码
        let reader = image::ImageReader::open(&input_path)
            .map_err(|e| format!("打开图片失败: {e}"))?
            .with_guessed_format()
            .map_err(|e| format!("识别图片格式失败: {e}"))?;
        let img = reader.decode().map_err(|e| format!("读取图片失败: {e}"))?;
        let out = std::fs::File::create(&output_path)
            .map_err(|e| format!("创建输出文件失败: {e}"))?;
        if fmt == image::ImageFormat::Jpeg {
            // JPEG 有损编码固定 90% 质量，避免默认 75% 画质损失
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(out, 90);
            img.write_with_encoder(enc)
                .map_err(|e| format!("写入 JPEG 失败: {e}"))?;
        } else {
            img.write_to(out, fmt)
                .map_err(|e| format!("写入图片失败: {e}"))?;
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("转换任务异常: {e}"))?
}

/// 音频格式转换：输出格式由 output_path 扩展名决定（wav/mp3/m4a）。
/// 源文件也可以是视频（自动选取其中的音轨）。
#[tauri::command]
pub async fn convert_audio(input_path: String, output_path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || convert_audio_impl(&input_path, &output_path))
        .await
        .map_err(|e| format!("转换任务异常: {e}"))?
}

pub(crate) fn convert_audio_impl(input: &str, output: &str) -> Result<(), String> {
    let ext = Path::new(output)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .ok_or("无法识别输出格式")?;
    // WAV 直接写 PCM 文件：不依赖系统 PCM 编码器（部分精简版 Windows 缺少
    // Media Foundation 的 WAV/MP3 编码器，SinkWriter 会报 0xC00D5212）。
    if ext == "wav" {
        return decode_audio_to_wav(input, output);
    }
    let subtype: GUID = match ext.as_str() {
        "wav" => MFAudioFormat_PCM,
        "mp3" => MFAudioFormat_MP3,
        "m4a" | "aac" => MFAudioFormat_AAC,
        _ => return Err(format!("不支持的音频输出格式: {ext}")),
    };

    unsafe {
        // Media Foundation 需要 COM 初始化（多线程单元）
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| format!("COM 初始化失败: {e:?}"))?;
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)
            .map_err(|e| format!("MF 初始化失败: {e}"))?;

        let result = (|| -> Result<(), String> {
            // 1. 源读取器：输入文件
            let input_w = wide(input);
            let reader = MFCreateSourceReaderFromURL(PCWSTR(input_w.as_ptr()), None)
                .map_err(|e| format!("打开输入音频失败: {e}"))?;

            // 视频文件也能转音频：关闭其它轨道，只选第一条音轨
            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(|e| format!("关闭其它轨道失败: {e}"))?;
            reader
                .SetStreamSelection(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, true)
                .map_err(|e| format!("选择音频轨道失败: {e}"))?;

            // 2. 让源读取器输出 PCM（自动解码 mp3/aac 等压缩格式）
            let pcm_req = MFCreateMediaType().map_err(|e| format!("创建媒体类型失败: {e}"))?;
            pcm_req
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            pcm_req
                .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            reader
                .SetCurrentMediaType(
                    MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
                    None,
                    &pcm_req,
                )
                .map_err(|e| format!("设置音频解码失败: {e}"))?;

            // 3. 读取解码后的 PCM 参数（采样率/声道/位深）
            let pcm_out = reader
                .GetCurrentMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32)
                .map_err(|e| format!("读取音频参数失败: {e}"))?;
            let sample_rate = pcm_out
                .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
                .unwrap_or(44100);
            let channels = pcm_out
                .GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)
                .unwrap_or(2);
            let bits = pcm_out
                .GetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE)
                .unwrap_or(16);
            if sample_rate == 0 {
                return Err("无法读取音频采样率".to_string());
            }

            // 4. 目标写入器：输出文件（扩展名决定容器与编码器）
            let output_w = wide(output);
            let writer = MFCreateSinkWriterFromURL(
                PCWSTR(output_w.as_ptr()),
                None::<&windows::Win32::Media::MediaFoundation::IMFByteStream>,
                None::<&windows::Win32::Media::MediaFoundation::IMFAttributes>,
            )
            .map_err(|e| format!("创建输出写入器失败: {e}"))?;

            // 5. 输出媒体类型：编码格式 + 输入采样率/声道
            let out_type = MFCreateMediaType().map_err(|e| format!("创建媒体类型失败: {e}"))?;
            out_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            out_type
                .SetGUID(&MF_MT_SUBTYPE, &subtype)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            out_type
                .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, sample_rate)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            out_type
                .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            if subtype == MFAudioFormat_PCM {
                // WAV/PCM 输出：位深与输入一致
                out_type
                    .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, bits)
                    .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            } else {
                // 有损编码目标码率 192kbps。
                // 注意：MF_MT_AUDIO_AVG_BYTES_PER_SECOND 单位是 字节/秒，
                // 192kbps = 24000 字节/秒（此前误写成 192000 被编码器拒绝 0xC00D36B4）
                out_type
                    .SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 192_000 / 8)
                    .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            }
            let stream_index = writer
                .AddStream(&out_type)
                .map_err(|e| format!("添加输出流失败: {e}"))?;
            writer
                .SetInputMediaType(stream_index, &pcm_out, None::<&IMFAttributes>)
                .map_err(|e| format!("设置输入流失败: {e}"))?;
            // 写入样本前必须先 BeginWriting，否则 WriteSample 报 MF_E_INVALID_REQUEST (0xC00D36B2)
            writer
                .BeginWriting()
                .map_err(|e| format!("开始写入失败: {e}"))?;

            // 6. 转码循环：读样本 → 写入
            loop {
                let mut flags: u32 = 0;
                let mut timestamp: i64 = 0;
                let mut sample: Option<IMFSample> = None;
                reader
                    .ReadSample(
                        MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
                        0,
                        None,
                        Some(&mut flags),
                        Some(&mut timestamp),
                        Some(&mut sample),
                    )
                    .map_err(|e| format!("读取音频数据失败: {e}"))?;
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                if let Some(s) = sample {
                    // 保持样本自带时长，补齐时间戳，确保 SinkWriter 能正确接收
                    let duration = s.GetSampleDuration().unwrap_or(0);
                    s.SetSampleTime(timestamp)
                        .map_err(|e| format!("设置时间戳失败: {e}"))?;
                    if duration > 0 {
                        s.SetSampleDuration(duration)
                            .map_err(|e| format!("设置样本时长失败: {e}"))?;
                    }
                    writer
                        .WriteSample(stream_index, &s)
                        .map_err(|e| format!("写入音频数据失败: {e}"))?;
                }
            }

            // 7. 收尾
            writer
                .Finalize()
                .map_err(|e| format!("完成写入失败: {e}"))?;
            Ok(())
        })();

        let _ = MFShutdown();
        CoUninitialize();
        result.map_err(|e| format!("音频转换失败: {e}"))
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}


/// 把任意音频（或视频中的音轨）转码为 ADTS AAC，供 MKA 封装使用。
pub(crate) fn transcode_to_adts(input: &str, output: &str) -> Result<(), String> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| format!("COM 初始化失败: {e:?}"))?;
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(|e| format!("MF 初始化失败: {e}"))?;

        let result = (|| -> Result<(), String> {
            let mut attributes: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attributes, 1)
                .map_err(|e| format!("创建转码属性失败: {e}"))?;
            let attributes = attributes.ok_or_else(|| "创建转码属性失败".to_string())?;
            attributes
                .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_ADTS)
                .map_err(|e| format!("设置 ADTS 容器失败: {e}"))?;

            let output_w = wide(output);
            let writer = MFCreateSinkWriterFromURL(
                PCWSTR(output_w.as_ptr()),
                None::<&windows::Win32::Media::MediaFoundation::IMFByteStream>,
                Some(&attributes),
            )
            .map_err(|e| format!("创建 ADTS 写入器失败: {e}"))?;

            let input_w = wide(input);
            let reader = MFCreateSourceReaderFromURL(PCWSTR(input_w.as_ptr()), None)
                .map_err(|e| format!("打开输入音频失败: {e}"))?;
            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(|e| format!("关闭其它轨道失败: {e}"))?;
            reader
                .SetStreamSelection(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, true)
                .map_err(|e| format!("选择音频轨道失败: {e}"))?;

            let pcm_req = MFCreateMediaType().map_err(|e| format!("创建媒体类型失败: {e}"))?;
            pcm_req
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            pcm_req
                .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            reader
                .SetCurrentMediaType(
                    MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
                    None,
                    &pcm_req,
                )
                .map_err(|e| format!("设置音频解码失败: {e}"))?;

            let pcm_out = reader
                .GetCurrentMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32)
                .map_err(|e| format!("读取音频参数失败: {e}"))?;
            let sample_rate = pcm_out
                .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
                .unwrap_or(44100);
            let channels = pcm_out
                .GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)
                .unwrap_or(2);
            if sample_rate == 0 {
                return Err("无法读取音频采样率".to_string());
            }

            let out_type = MFCreateMediaType().map_err(|e| format!("创建媒体类型失败: {e}"))?;
            out_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            out_type
                .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            out_type
                .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, sample_rate)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            out_type
                .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            out_type
                .SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 192_000 / 8)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;

            let stream_index = writer
                .AddStream(&out_type)
                .map_err(|e| format!("添加输出流失败: {e}"))?;
            writer
                .SetInputMediaType(stream_index, &pcm_out, None::<&IMFAttributes>)
                .map_err(|e| format!("设置输入流失败: {e}"))?;
            writer
                .BeginWriting()
                .map_err(|e| format!("开始写入失败: {e}"))?;

            loop {
                let mut flags: u32 = 0;
                let mut timestamp: i64 = 0;
                let mut sample: Option<IMFSample> = None;
                reader
                    .ReadSample(
                        MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
                        0,
                        None,
                        Some(&mut flags),
                        Some(&mut timestamp),
                        Some(&mut sample),
                    )
                    .map_err(|e| format!("读取音频数据失败: {e}"))?;
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                if let Some(sample) = sample {
                    let duration = sample.GetSampleDuration().unwrap_or(0);
                    sample
                        .SetSampleTime(timestamp)
                        .map_err(|e| format!("设置时间戳失败: {e}"))?;
                    if duration > 0 {
                        sample
                            .SetSampleDuration(duration)
                            .map_err(|e| format!("设置样本时长失败: {e}"))?;
                    }
                    writer
                        .WriteSample(stream_index, &sample)
                        .map_err(|e| format!("写入音频数据失败: {e}"))?;
                }
            }

            writer.Finalize().map_err(|e| format!("完成写入失败: {e}"))?;
            Ok(())
        })();

        let _ = MFShutdown();
        CoUninitialize();
        result.map_err(|e| format!("音频转码失败: {e}"))
    }
}

/// 解码任意音频/视频音轨为 PCM 并写出标准 WAV（不经过 SinkWriter 编码器）。
fn decode_audio_to_wav(input: &str, output: &str) -> Result<(), String> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| format!("COM 初始化失败: {e:?}"))?;
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(|e| format!("MF 初始化失败: {e}"))?;
        let result = (|| -> Result<(), String> {
            let input_w = wide(input);
            let reader = MFCreateSourceReaderFromURL(PCWSTR(input_w.as_ptr()), None)
                .map_err(|e| format!("打开输入音频失败: {e}"))?;
            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(|e| format!("关闭其它轨道失败: {e}"))?;
            reader
                .SetStreamSelection(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, true)
                .map_err(|e| format!("选择音频轨道失败: {e}"))?;
            let pcm_req = MFCreateMediaType().map_err(|e| format!("创建媒体类型失败: {e}"))?;
            pcm_req
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            pcm_req
                .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            reader
                .SetCurrentMediaType(
                    MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
                    None,
                    &pcm_req,
                )
                .map_err(|e| format!("设置音频解码失败: {e}"))?;
            let pcm_out = reader
                .GetCurrentMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32)
                .map_err(|e| format!("读取音频参数失败: {e}"))?;
            let sample_rate = pcm_out
                .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
                .unwrap_or(44100);
            let channels = pcm_out
                .GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)
                .unwrap_or(2)
                .max(1);
            let bits = pcm_out
                .GetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE)
                .unwrap_or(16)
                .max(8);
            if sample_rate == 0 {
                return Err("无法读取音频采样率".to_string());
            }

            let mut pcm: Vec<u8> = Vec::new();
            loop {
                let mut flags: u32 = 0;
                let mut timestamp: i64 = 0;
                let mut sample: Option<IMFSample> = None;
                reader
                    .ReadSample(
                        MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
                        0,
                        None,
                        Some(&mut flags),
                        Some(&mut timestamp),
                        Some(&mut sample),
                    )
                    .map_err(|e| format!("读取音频数据失败: {e}"))?;
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                let Some(sample) = sample else { continue };
                let Ok(buffer) = sample.ConvertToContiguousBuffer() else {
                    continue;
                };
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut max_len: u32 = 0;
                let mut current_len: u32 = 0;
                buffer
                    .Lock(&mut data, Some(&mut max_len), Some(&mut current_len))
                    .map_err(|e| format!("读取音频缓冲区失败: {e}"))?;
                if !data.is_null() && current_len > 0 {
                    pcm.extend_from_slice(std::slice::from_raw_parts(data, current_len as usize));
                }
                let _ = buffer.Unlock();
            }

            let block_align = (channels as u16) * (bits as u16 / 8);
            let byte_rate = sample_rate * block_align as u32;
            let data_len = pcm.len() as u32;
            let mut out = Vec::with_capacity(44 + pcm.len());
            out.extend_from_slice(b"RIFF");
            out.extend_from_slice(&(36 + data_len).to_le_bytes());
            out.extend_from_slice(b"WAVEfmt ");
            out.extend_from_slice(&16u32.to_le_bytes());
            out.extend_from_slice(&1u16.to_le_bytes());
            out.extend_from_slice(&(channels as u16).to_le_bytes());
            out.extend_from_slice(&sample_rate.to_le_bytes());
            out.extend_from_slice(&byte_rate.to_le_bytes());
            out.extend_from_slice(&block_align.to_le_bytes());
            out.extend_from_slice(&(bits as u16).to_le_bytes());
            out.extend_from_slice(b"data");
            out.extend_from_slice(&data_len.to_le_bytes());
            out.extend_from_slice(&pcm);
            std::fs::write(output, out).map_err(|e| format!("写入 WAV 失败: {e}"))?;
            Ok(())
        })();
        let _ = MFShutdown();
        CoUninitialize();
        result
    }
}

/// 视频转换命令：GIF / 静态图片 / 音频提取
#[tauri::command]
pub async fn convert_video(
    input_path: String,
    output_path: String,
    format: String,
    fps: Option<u32>,
    width: Option<u32>,
    start_seconds: Option<f64>,
    duration_seconds: Option<f64>,
) -> Result<crate::video_convert::VideoConvertResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::video_convert::convert_video(
            &input_path,
            &output_path,
            &format,
            fps.unwrap_or(12),
            width.unwrap_or(480),
            start_seconds.unwrap_or(0.0),
            duration_seconds.unwrap_or(0.0),
        )
    })
    .await
    .map_err(|e| format!("转换任务异常: {e}"))?
}

/// 把音频 + 可选字幕 + 可选封面合成为 MKA
#[tauri::command]
pub async fn mux_mka(
    audio_path: String,
    subtitle_path: Option<String>,
    cover_path: Option<String>,
    output_path: String,
    title: Option<String>,
) -> Result<crate::matroska::MuxResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::matroska::mux_mka(&crate::matroska::MuxOptions {
            audio_path,
            subtitle_path,
            cover_path,
            output_path,
            title,
        })
    })
    .await
    .map_err(|e| format!("合成任务异常: {e}"))?
}

/// 拖入路径是文件还是文件夹（WebView 拖放只给路径，需要后端判断）
#[tauri::command]
pub fn path_is_directory(path: String) -> bool {
    Path::new(&path).is_dir()
}

/// 读取文本文件（拖入 .fac 预设时用）
#[tauri::command]
pub fn read_text_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("读取文件失败: {e}"))
}

/// 把本地图片读成 data URL，供「拖入设置背景」复用现有上传逻辑
#[tauri::command]
pub fn read_image_as_data_url(path: String) -> Result<String, String> {
    use base64::Engine;
    let bytes = std::fs::read(&path).map_err(|e| format!("读取图片失败: {e}"))?;
    let ext = Path::new(&path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        _ => return Err(format!("不支持的图片格式: {ext}")),
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime};base64,{encoded}"))
}
