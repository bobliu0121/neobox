//! 文件格式转换工具：
//! - 图片格式（png/jpg/jpeg/webp/bmp）相互转换（image crate）
//! - 音频格式（wav/mp3/m4a）相互转换（Windows Media Foundation 系统转码）
//!
//! 音频转换依赖 Windows 10/11 自带的 Media Foundation 编码器（MP3/AAC/PCM），
//! 无需外部 ffmpeg；输出容器与编码器由输出文件扩展名决定。

use std::path::Path;

use windows::core::{GUID, PCWSTR};
use windows::Win32::Media::MediaFoundation::{
    MFShutdown, MFStartup, MFCreateAttributes, MFCreateCollection, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL,
    MFCreateSourceReaderFromURL, MFAudioFormat_AAC, MFAudioFormat_MP3,
    MFAudioFormat_PCM, MFMediaType_Audio, MFMediaType_Video, MFSTARTUP_NOSOCKET,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    MFTranscodeContainerType_ADTS, MFTranscodeContainerType_MPEG4,
    MF_TRANSCODE_CONTAINERTYPE,
    MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoInterlace_Progressive,
    MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MPEG2_PROFILE, MF_MT_MPEG2_LEVEL,
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_MAJOR_TYPE,
    MF_MT_SUBTYPE, MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_VERSION, IMFAttributes, IMFMediaType, IMFSample, IMFTransform,
    MFT_MESSAGE_COMMAND_DRAIN, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER, CMSH264EncoderMFT,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows_sys::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// 当前进程提交内存字节数（用于写帧节流：SinkWriter 对异步编码器不节流，
/// 会把未编码样本全部缓存进内存，需在缓存超阈值时暂停写入让编码器消化）
fn current_private_bytes() -> u64 {
    unsafe {
        let handle = GetCurrentProcess();
        let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if K32GetProcessMemoryInfo(handle, &mut pmc, size) != 0 {
            pmc.PagefileUsage as u64
        } else {
            0
        }
    }
}

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

/// 判断是否为视频文件（按扩展名）
fn is_video_file(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    ["mp4", "webm", "mov", "mkv", "avi", "m4v"].contains(&ext.as_str())
}

/// 把「已编码的裸 H.264 Annex-B 帧序列 + 裸 AAC 帧序列」重新封装成 MP4。
///
/// 专为 B 站 DASH 下载设计：它的 video.m4s / audio.m4s 里装的就是编好的帧，
/// 不需要重新编码，只需换容器（remux）。相比走完整转码链路，
/// 速度快一个数量级且画质零损失。
///
/// * `video_frames` —— 每个元素是一帧的 Annex-B 数据（含起始码）
/// * `audio_frames` —— 每个元素是一帧 AAC 原始数据（无 ADTS 头，MP4 用 ASC 描述参数）
/// * `fps_num/fps_den` —— 用于写 stts；B 站常用 30000/1001 或 30/1
/// * `sr/ch` —— 音频采样率与声道数
/// * `audio_asc` —— AAC 的 AudioSpecificConfig（来自 m4s 的 esds，否则按 sr/ch 生成）
#[allow(clippy::too_many_arguments)]
pub fn remux_av_to_mp4(
    output: &str,
    video_frames: &[Vec<u8>],
    audio_frames: &[Vec<u8>],
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
    sr: u32,
    ch: u32,
    audio_asc: Option<Vec<u8>>,
    // 可选的真实 SPS/PPS。B 站 fMP4 的帧里不含参数集（它们在初始化段 avcC 中），
    // 必须由调用方从 moov 读出后传入；否则 avcC 会退化成兜底假值，
    // 播放器无法建解码器 —— 实测表现为「只有声音没有画面」。
    sps_pps: Option<(Vec<u8>, Vec<u8>)>,
    // 每个视频样本的合成时间偏移（1e7 刻度）。空表示无 B 帧、不需要 ctts。
    v_ctos: &[i64],
) -> Result<(), String> {
    let has_audio = !audio_frames.is_empty();
    let asc = audio_asc.unwrap_or_else(|| aac_asc(sr, ch));
    let mut mux = Mp4Muxer::new(
        output,
        width,
        height,
        fps_num,
        fps_den,
        sr,
        ch,
        has_audio,
        asc,
    )?;
    // 优先用调用方给的参数集：add_video 内部「第一帧为空才提取」，
    // 预先填好 v_sps/v_pps 就能让它跳过那段徒劳的提取。
    if let Some((sps, pps)) = sps_pps {
        if !sps.is_empty() && !pps.is_empty() {
            mux.v_sps = sps;
            mux.v_pps = pps;
        }
    }
    // 有 cto 数据时逐帧写入偏移，供 ctts 表使用；否则退回普通写入。
    if v_ctos.len() == video_frames.len() && !v_ctos.is_empty() {
        for (f, cto) in video_frames.iter().zip(v_ctos.iter()) {
            mux.add_video_with_cto(f, *cto)?;
        }
    } else {
        for f in video_frames {
            mux.add_video(f)?;
        }
    }
    for f in audio_frames {
        mux.add_audio(f)?;
    }
    mux.finish().map_err(|e| format!("写入 MP4 失败: {e}"))
}

/// 仅把裸 AAC 帧封装成 .m4a（纯音频下载用）。
pub fn remux_aac_to_m4a(
    output: &str,
    audio_frames: &[Vec<u8>],
    sr: u32,
    ch: u32,
    audio_asc: Option<Vec<u8>>,
) -> Result<(), String> {
    if audio_frames.is_empty() {
        return Err("没有音频帧可封装".to_string());
    }
    let asc = audio_asc.unwrap_or_else(|| aac_asc(sr, ch));
    let mut mux = Mp4Muxer::new(output, 0, 0, 0, 0, sr, ch, true, asc)?;
    for f in audio_frames {
        mux.add_audio(f)?;
    }
    mux.finish().map_err(|e| format!("写入 m4a 失败: {e}"))
}

/// 从 AAC 裸帧流中解析采样率与声道数（读第一个 ADTS 头）。
///
/// B 站的 audio.m4s 经我们提取后是**裸 AAC 帧**（无 ADTS 头），
/// 但若来源带 ADTS 则可用此函数兜底取参数。
fn parse_adts_header(frame: &[u8]) -> Option<(u32, u32)> {
    if frame.len() < 7 || frame[0] != 0xFF || (frame[1] & 0xF0) != 0xF0 {
        return None;
    }
    // ADTS 采样率索引
    const SR_TABLE: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    let sr_idx = ((frame[2] >> 2) & 0x0F) as usize;
    let ch_cfg = (((frame[2] & 0x01) << 2) | ((frame[3] >> 6) & 0x03)) as u32;
    let sr = *SR_TABLE.get(sr_idx)?;
    Some((sr, ch_cfg))
}

/// 供 bilibili 模块使用：从音频裸帧推断采样率/声道。
/// 找不到 ADTS 头时返回 B 站常见默认值（44100/2）。
pub fn infer_audio_params(audio_frames: &[Vec<u8>]) -> (u32, u32) {
    for f in audio_frames.iter().take(4) {
        if let Some((sr, ch)) = parse_adts_header(f) {
            return (sr, ch.max(1));
        }
    }
    (44100, 2)
}


/// 音视频倒放：视频同时倒放画面与声音，音频只倒放声音。
/// 使用 Media Foundation，不依赖外部 ffmpeg。
///
/// `audio_only`：仅提取音频轨做倒放，输出 `.m4a`（AAC-in-MP4），丢弃画面。
/// 对视频文件开启时走音频路径（只解码音频轨，比整段视频解码快得多），
/// 但也因此要求源文件确实含音轨 —— 无音轨会直接报错而不是产出空文件。
#[tauri::command]
pub async fn reverse_media(
    input_path: String,
    output_path: String,
    audio_only: Option<bool>,
) -> Result<(), String> {
    let audio_only = audio_only.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        // 仅提取音频：无论输入是音频还是视频，都只处理音轨
        if audio_only {
            return reverse_audio_mf(&input_path, &output_path);
        }
        if is_video_file(&input_path) {
            // 倒放不放大数据量，忽略提示
            let _ = process_video_media(&input_path, &output_path, MediaOp::Reverse)?;
            Ok(())
        } else {
            reverse_audio_mf(&input_path, &output_path)
        }
    })
    .await
    .map_err(|e| format!("倒放任务异常: {e}"))?
}

/// 媒体处理操作类型
#[derive(Clone, Copy)]
enum MediaOp {
    /// 倒放：画面与声音均反转
    Reverse,
    /// 变速：按倍速抽帧/复制帧
    Speed(f64),
}

/// 用 Media Foundation 实现音频倒放
fn reverse_audio_mf(input: &str, output: &str) -> Result<(), String> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| format!("初始化 COM 失败: {e:?}"))?;
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)
            .map_err(|e| format!("初始化 Media Foundation 失败: {e}"))?;

        let result = (|| -> Result<(), String> {
            // 1. 打开输入文件
            let input_w = wide(input);
            let reader = MFCreateSourceReaderFromURL(PCWSTR(input_w.as_ptr()), None)
                .map_err(|e| format!("打开输入音频失败: {e}"))?;

            // 只选第一条音轨
            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(|e| format!("关闭其它轨道失败: {e}"))?;
            reader
                .SetStreamSelection(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, true)
                .map_err(|e| format!("该文件不含可用的音频轨，无法提取音频（请关闭「仅提取音频」开关，或改用含声音的文件）: {e}"))?;

            // 2. 设置输出为 PCM 16-bit
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

            // 3. 读取 PCM 参数
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
            if bits != 16 {
                return Err(format!("仅支持 16-bit PCM 音频，当前位深: {}", bits));
            }

            // 4. 读取所有 PCM 采样到内存
            let mut all_samples: Vec<i16> = Vec::new();
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
                    let buffer = s.ConvertToContiguousBuffer()
                        .map_err(|e| format!("获取样本缓冲区失败: {e}"))?;
                    let mut ptr: *mut u8 = std::ptr::null_mut();
                    let mut max_len: u32 = 0;
                    let mut current_len: u32 = 0;
                    buffer.Lock(&mut ptr, Some(&mut max_len), Some(&mut current_len))
                        .map_err(|e| format!("锁定缓冲区失败: {e}"))?;
                    if !ptr.is_null() && current_len > 0 {
                        let sample_bytes = std::slice::from_raw_parts(ptr as *const i16, (current_len / 2) as usize);
                        all_samples.extend_from_slice(sample_bytes);
                    }
                    let _ = buffer.Unlock();
                }
            }

            // 5. 倒放：按帧反转采样（编码时按映射取用，不预先复制整份采样，节省内存）
            //
            // 先校验取到了音频数据。视频文件被「仅提取音频」导向本函数时，
            // 若源文件没有音轨，上面循环会读到 EOF 立即结束、all_samples 为空，
            // 此时 channels 可能为 0 → `len() / 0` 会直接 panic；
            // 即便不为 0 也会产出 0 字节的空文件，用户只会看到"处理成功但没声音"。
            // 这里明确报错，让用户知道该关掉开关或换文件。
            if all_samples.is_empty() || channels == 0 {
                return Err(
                    "源文件不包含音轨，无法提取音频。请关闭「仅提取音频」开关后重试，或改用含声音的文件。"
                        .to_string(),
                );
            }
            let samples_per_frame = channels as usize;
            let frame_count = all_samples.len() / samples_per_frame;
            if frame_count == 0 {
                return Err("音频数据为空，无法倒放。".to_string());
            }
            write_audio_aac_mp4(
                output,
                &all_samples,
                channels,
                sample_rate,
                frame_count,
                |i| frame_count - 1 - i,
            )?;
            Ok(())
        })();

        let _ = MFShutdown();
        CoUninitialize();
        result.map_err(|e| format!("音频倒放失败: {e}"))
    }
}

/// 音视频加速/减速：视频同时调整画面与声音，音频只调整声音。
/// 使用 Media Foundation，不依赖外部 ffmpeg。
/// 返回 Ok(Some(提示)) 表示处理成功但带有超限提示；Ok(None) 表示正常完成。
#[tauri::command]
pub async fn change_media_speed(
    input_path: String,
    output_path: String,
    speed: f64,
    audio_only: Option<bool>,
) -> Result<Option<String>, String> {
    let audio_only = audio_only.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        // 仅提取音频：视频输入也只处理音轨，输出 .m4a
        if audio_only {
            return change_audio_speed_mf(&input_path, &output_path, speed);
        }
        if is_video_file(&input_path) {
            process_video_media(&input_path, &output_path, MediaOp::Speed(speed))
        } else {
            change_audio_speed_mf(&input_path, &output_path, speed)
        }
    })
    .await
    .map_err(|e| format!("速度调整任务异常: {e}"))?
}

/// 用 Media Foundation 实现音频加速/减速（调整采样率，会变调）
/// 返回 Ok(Some(提示)) 表示成功但数据量较大；Ok(None) 表示正常完成。
fn change_audio_speed_mf(input: &str, output: &str, speed: f64) -> Result<Option<String>, String> {
    if speed <= 0.0 {
        return Err("速度必须大于 0".to_string());
    }

    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| format!("初始化 COM 失败: {e:?}"))?;
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)
            .map_err(|e| format!("初始化 Media Foundation 失败: {e}"))?;

        let result = (|| -> Result<Option<String>, String> {
            let mut warning: Option<String> = None;
            // 1. 打开输入文件
            let input_w = wide(input);
            let reader = MFCreateSourceReaderFromURL(PCWSTR(input_w.as_ptr()), None)
                .map_err(|e| format!("打开输入音频失败: {e}"))?;

            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(|e| format!("关闭其它轨道失败: {e}"))?;
            reader
                .SetStreamSelection(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, true)
                .map_err(|e| format!("该文件不含可用的音频轨，无法提取音频（请关闭「仅提取音频」开关，或改用含声音的文件）: {e}"))?;

            // 2. 设置输出为 PCM 16-bit
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

            // 3. 读取 PCM 参数
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
            if bits != 16 {
                return Err(format!("仅支持 16-bit PCM 音频，当前位深: {}", bits));
            }

            // 4. 读取所有 PCM 采样到内存
            let mut all_samples: Vec<i16> = Vec::new();
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
                    let buffer = s.ConvertToContiguousBuffer()
                        .map_err(|e| format!("获取样本缓冲区失败: {e}"))?;
                    let mut ptr: *mut u8 = std::ptr::null_mut();
                    let mut max_len: u32 = 0;
                    let mut current_len: u32 = 0;
                    buffer.Lock(&mut ptr, Some(&mut max_len), Some(&mut current_len))
                        .map_err(|e| format!("锁定缓冲区失败: {e}"))?;
                    if !ptr.is_null() && current_len > 0 {
                        let sample_bytes = std::slice::from_raw_parts(ptr as *const i16, (current_len / 2) as usize);
                        all_samples.extend_from_slice(sample_bytes);
                    }
                    let _ = buffer.Unlock();
                }
            }

            // 5. 加速/减速：通过抽样调整采样点数量，保持采样率不变
            // 这样不受 MP3 编码器的采样率限制
            //
            // 与倒放同理：视频经「仅提取音频」进入本函数时，无音轨会让 all_samples 为空、
            // channels 可能为 0，`len() / 0` 会 panic。这里提前拦住并给出可操作提示。
            if all_samples.is_empty() || channels == 0 {
                return Err(
                    "源文件不包含音轨，无法提取音频。请关闭「仅提取音频」开关后重试，或改用含声音的文件。"
                        .to_string(),
                );
            }
            let samples_per_frame = channels as usize;
            let original_frame_count = all_samples.len() / samples_per_frame;
            if original_frame_count == 0 {
                return Err("音频数据为空，无法调整速度。".to_string());
            }

            // 计算目标采样点数量
            // speed = 2.0 表示 2 倍速度，即播放时间减半，采样点数量减半
            let target_frame_count = (original_frame_count as f64 / speed) as usize;

            // 数据量较大时仅提示，不中断处理
            let target_bytes = target_frame_count
                .saturating_mul(samples_per_frame)
                .saturating_mul(2);
            if target_bytes > 1_500_000_000usize {
                warning = Some(format!(
                    "倍速 {}x 将产生约 {} MB 的音频数据，数据量较大，处理与内存占用会明显增加，请耐心等待",
                    speed,
                    target_bytes / 1024 / 1024
                ));
            }

            // 输出采样率保持不变
            let new_sample_rate = sample_rate;

            // 6. 按抽样映射编码为 AAC 并写入 MP4（内存仅占单块缓冲，不复制整份采样）
            let samples_per_frame = channels as usize;
            write_audio_aac_mp4(
                output,
                &all_samples,
                channels,
                sample_rate,
                target_frame_count,
                |i| {
                    let src_pos = (i as f64 * speed) as usize;
                    std::cmp::min(src_pos, original_frame_count - 1)
                },
            )?;
            Ok(warning)
        })();

        let _ = MFShutdown();
        CoUninitialize();
        result.map_err(|e| format!("音频速度调整失败: {e}"))
    }
}

/// 视频倒放 / 变速：用 Media Foundation 读取全部视频帧（NV12）与音轨（PCM），
/// 处理后经 SinkWriter 编码为 H.264 + AAC 写入输出文件。
/// 返回 Ok(Some(提示)) 表示成功但数据量较大；Ok(None) 表示正常完成。
fn process_video_media(input: &str, output: &str, op: MediaOp) -> Result<Option<String>, String> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| format!("初始化 COM 失败: {e:?}"))?;
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)
            .map_err(|e| format!("初始化 Media Foundation 失败: {e}"))?;

        let result = (|| -> Result<Option<String>, String> {
            let mut warning: Option<String> = None;
            // 1. 打开输入文件
            let input_w = wide(input);
            let reader = MFCreateSourceReaderFromURL(PCWSTR(input_w.as_ptr()), None)
                .map_err(|e| format!("打开输入视频失败: {e}"))?;

            // 2. 关闭所有流，再选择性打开视频流
            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(|e| format!("关闭其它轨道失败: {e}"))?;
            reader
                .SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, true)
                .map_err(|e| format!("选择视频轨道失败: {e}（请确认文件包含视频流）"))?;

            // 3. 视频输出设为 NV12（H.264 编码器标准输入格式）
            let nv12_req = MFCreateMediaType().map_err(|e| format!("创建媒体类型失败: {e}"))?;
            nv12_req
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            nv12_req
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            reader
                .SetCurrentMediaType(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    None,
                    &nv12_req,
                )
                .map_err(|e| format!("设置视频解码失败: {e}"))?;

            // 4. 读取视频参数（宽高、帧率）
            let video_type = reader
                .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
                .map_err(|e| format!("读取视频参数失败: {e}"))?;
            let frame_size = video_type
                .GetUINT64(&MF_MT_FRAME_SIZE)
                .map_err(|e| format!("读取画面尺寸失败: {e}"))?;
            // MF_MT_FRAME_SIZE 打包约定：高 32 位 = 宽，低 32 位 = 高
            let vw = (frame_size >> 32) as u32;
            let vh = (frame_size & 0xFFFF_FFFF) as u32;
            if vw == 0 || vh == 0 {
                return Err("无法读取视频画面尺寸".to_string());
            }
            let frame_rate = video_type.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(0);
            // MF_MT_FRAME_RATE 打包约定：高 32 位 = 分子，低 32 位 = 分母
            let fps_num = (frame_rate >> 32) as u32;
            let fps_den = (frame_rate & 0xFFFF_FFFF) as u32;
            let fps = if fps_num > 0 && fps_den > 0 {
                fps_num as f64 / fps_den as f64
            } else {
                30.0
            };
            // NV12 紧凑帧大小（用于推算 stride）
            let frame_bytes = (vw as usize) * (vh as usize) * 3 / 2;

            // 5. 把全部视频帧写入临时原始文件（内存只保留单帧缓冲），
            //    后续处理按偏移随机读取，避免整段视频驻留内存
            let tmp_path = format!("{output}.neobox_tmpframes");
            struct TmpGuard {
                path: String,
                file: Option<std::fs::File>,
                armed: bool,
            }
            impl Drop for TmpGuard {
                fn drop(&mut self) {
                    if self.armed {
                        self.file.take();
                        let _ = std::fs::remove_file(&self.path);
                    }
                }
            }
            let mut _tmp_guard = TmpGuard {
                path: tmp_path.clone(),
                file: Some(std::fs::File::create(&tmp_path).map_err(|e| format!("创建临时帧文件失败: {e}"))?),
                armed: true,
            };

            let mut frame_offsets: Vec<u64> = Vec::new();
            let mut frame_sizes: Vec<u32> = Vec::new();
            let mut cur_offset: u64 = 0;
            let mut total_video_frames = 0usize;
            loop {
                let mut flags: u32 = 0;
                let mut ts: i64 = 0;
                let mut sample: Option<IMFSample> = None;
                reader
                    .ReadSample(
                        MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                        0,
                        None,
                        Some(&mut flags),
                        Some(&mut ts),
                        Some(&mut sample),
                    )
                    .map_err(|e| format!("读取视频帧失败: {e}"))?;
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                if let Some(s) = sample {
                    let buf = s
                        .ConvertToContiguousBuffer()
                        .map_err(|e| format!("获取帧缓冲区失败: {e}"))?;
                    let mut ptr: *mut u8 = std::ptr::null_mut();
                    let mut max_len: u32 = 0;
                    let mut cur_len: u32 = 0;
                    buf.Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len))
                        .map_err(|e| format!("锁定帧缓冲区失败: {e}"))?;
                    if !ptr.is_null() && cur_len > 0 {
                        use std::io::Write;
                        let data = std::slice::from_raw_parts(ptr, cur_len as usize);
                        _tmp_guard
                            .file
                            .as_mut()
                            .ok_or("临时帧文件不可用")?
                            .write_all(data)
                            .map_err(|e| format!("写入临时帧文件失败: {e}"))?;
                        let _ = buf.Unlock();
                        frame_offsets.push(cur_offset);
                        frame_sizes.push(cur_len);
                        cur_offset += cur_len as u64;
                        total_video_frames += 1;
                    } else {
                        let _ = buf.Unlock();
                    }
                }
            }
            if total_video_frames == 0 {
                return Err("未能读取到任何视频帧".to_string());
            }
            use std::io::Write;
            _tmp_guard
                .file
                .as_mut()
                .ok_or("临时帧文件不可用")?
                .flush()
                .map_err(|e| format!("刷新临时帧文件失败: {e}"))?;
            use std::io::{Seek, SeekFrom};
            let mut read_file = std::fs::File::open(&tmp_path)
                .map_err(|e| format!("打开临时帧文件失败: {e}"))?;
            // 输出体积预估。
            //
            // ⚠️ 不能用 `cur_offset / total_video_frames` 当单帧大小：
            // 临时帧文件里存的是**解码后的 NV12 原始帧**（1080p 约 3.0 MB/帧），
            // 与最终 H.264 编码后的字节数相差接近两个数量级。
            // 旧实现据此估算，实测 3746 帧 1080p 得到「约 11194 MB」的提示，
            // 而真实输出只有 128 MB —— 夸大 87 倍，会直接吓退用户。
            //
            // 改为以**输入文件实际大小**为基准（已是压缩后的真实码率），
            // 再按「输出帧数 / 输入帧数」的比例缩放，得到量级正确的估算。
            // 目标码率相对源只有小幅变化（同分辨率同编码器），这个比例足够准；
            // 即便估算偏差几倍，也仍在合理提示范围内，不会出现 87 倍这种量级错误。
            let src_file_bytes = std::fs::metadata(input)
                .map(|m| m.len())
                .unwrap_or(0) as f64;
            let out_frame_ratio = match op {
                MediaOp::Reverse => 1.0,
                // 加速 → 帧数减少 → 体积减小；减速 → 帧数增加 → 体积增大
                MediaOp::Speed(s) if s > 0.0 => 1.0 / s,
                MediaOp::Speed(_) => 1.0,
            };
            let out_bytes_hint = (src_file_bytes * out_frame_ratio) as usize;
            if out_bytes_hint > 1_500_000_000usize {
                warning = Some(format!(
                    "处理后将产生约 {} MB 的视频数据，输出文件会较大，请耐心等待",
                    out_bytes_hint / 1024 / 1024
                ));
            }

            // 6. 检测并读取音轨（PCM 16-bit）
            let mut has_audio = false;
            let mut stream_idx = 0u32;
            loop {
                match reader.GetNativeMediaType(stream_idx, 0) {
                    Ok(t) => {
                        if let Ok(major) = t.GetGUID(&MF_MT_MAJOR_TYPE) {
                            if major == MFMediaType_Audio {
                                has_audio = true;
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
                stream_idx += 1;
            }
            let mut sample_rate: u32 = 44100;
            let mut channels: u32 = 2;
            let mut pcm_out = None;
            let mut all_samples: Vec<i16> = Vec::new();
            if has_audio {
                let audio_input_w = wide(input);
                let audio_reader = MFCreateSourceReaderFromURL(PCWSTR(audio_input_w.as_ptr()), None)
                    .map_err(|e| format!("打开音频流失败: {e}"))?;
                audio_reader
                    .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                    .map_err(|e| format!("关闭其它轨道失败: {e}"))?;
                audio_reader
                    .SetStreamSelection(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, true)
                    .map_err(|e| format!("选择音频轨道失败: {e}"))?;
                let pcm_req = MFCreateMediaType().map_err(|e| format!("创建媒体类型失败: {e}"))?;
                pcm_req
                    .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                    .map_err(|e| format!("设置媒体类型失败: {e}"))?;
                pcm_req
                    .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
                    .map_err(|e| format!("设置媒体类型失败: {e}"))?;
                audio_reader
                    .SetCurrentMediaType(
                        MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
                        None,
                        &pcm_req,
                    )
                    .map_err(|e| format!("设置音频解码失败: {e}"))?;
                let pcm_type = audio_reader
                    .GetCurrentMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32)
                    .map_err(|e| format!("读取音频参数失败: {e}"))?;
                sample_rate = pcm_type
                    .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
                    .unwrap_or(44100);
                channels = pcm_type
                    .GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)
                    .unwrap_or(2);
                let bits = pcm_type
                    .GetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE)
                    .unwrap_or(16);
                if bits != 16 {
                    return Err(format!("仅支持 16-bit PCM 音频，当前位深: {}", bits));
                }
                pcm_out = Some(pcm_type);
                loop {
                    let mut flags: u32 = 0;
                    let mut ts: i64 = 0;
                    let mut sample: Option<IMFSample> = None;
                    audio_reader
                        .ReadSample(
                            MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32,
                            0,
                            None,
                            Some(&mut flags),
                            Some(&mut ts),
                            Some(&mut sample),
                        )
                        .map_err(|e| format!("读取音频数据失败: {e}"))?;
                    if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                        break;
                    }
                    if let Some(s) = sample {
                        let buf = s
                            .ConvertToContiguousBuffer()
                            .map_err(|e| format!("获取样本缓冲区失败: {e}"))?;
                        let mut ptr: *mut u8 = std::ptr::null_mut();
                        let mut max_len: u32 = 0;
                        let mut cur_len: u32 = 0;
                        buf.Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len))
                            .map_err(|e| format!("锁定缓冲区失败: {e}"))?;
                        if !ptr.is_null() && cur_len > 0 {
                            let samples = std::slice::from_raw_parts(
                                ptr as *const i16,
                                (cur_len / 2) as usize,
                            );
                            all_samples.extend_from_slice(samples);
                        }
                        let _ = buf.Unlock();
                    }
                }
            }

            // 7. 计算输出帧总数与取帧映射
            let out_frame_count = match op {
                MediaOp::Reverse => total_video_frames,
                MediaOp::Speed(s) => {
                    if s <= 0.0 {
                        return Err("速度必须大于 0".to_string());
                    }
                    (total_video_frames as f64 / s) as usize
                }
            };
            let total_frames = total_video_frames;
            let pick_frame = |i: usize| -> usize {
                match op {
                    MediaOp::Reverse => total_frames - 1 - i,
                    MediaOp::Speed(s) => {
                        std::cmp::min((i as f64 * s) as usize, total_frames - 1)
                    }
                }
            };
            let audio_total_frames = if has_audio {
                all_samples.len() / channels as usize
            } else {
                0
            };
            let audio_out_frames = if has_audio {
                match op {
                    MediaOp::Reverse => audio_total_frames,
                    MediaOp::Speed(s) => (audio_total_frames as f64 / s) as usize,
                }
            } else {
                0
            };
            let pick_audio_frame = |i: usize| -> usize {
                match op {
                    MediaOp::Reverse => audio_total_frames - 1 - i,
                    MediaOp::Speed(s) => {
                        std::cmp::min((i as f64 * s) as usize, audio_total_frames - 1)
                    }
                }
            };

            // 8. H.264 编码器（IMFTransform 同步驱动；本机 SinkWriter 异步编码管线停滞，改为直驱）
            let encoder: IMFTransform = CoCreateInstance(&CMSH264EncoderMFT, None, CLSCTX_ALL)
                .map_err(|e| format!("系统缺少 H.264 编码器: {e}"))?;
            let in_type = MFCreateMediaType().map_err(|e| format!("创建媒体类型失败: {e}"))?;
            in_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            in_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            in_type
                .SetUINT64(&MF_MT_FRAME_SIZE, frame_size)
                .map_err(|e| format!("设置画面尺寸失败: {e}"))?;
            in_type
                .SetUINT64(&MF_MT_FRAME_RATE, frame_rate)
                .map_err(|e| format!("设置帧率失败: {e}"))?;
            in_type
                .SetUINT32(&MF_MT_DEFAULT_STRIDE, vw)
                .map_err(|e| format!("设置 stride 失败: {e}"))?;
            in_type
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|e| format!("设置隔行模式失败: {e}"))?;
            let mut chosen: Option<IMFMediaType> = None;
            for t_idx in 0..8u32 {
                match encoder.GetOutputAvailableType(0, t_idx) {
                    Ok(t) => {
                        if t.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default() == MFVideoFormat_H264 {
                            chosen = Some(t);
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let mut out_type = chosen.ok_or("系统缺少 H.264 编码器输出类型")?;
            out_type
                .SetUINT64(&MF_MT_FRAME_SIZE, frame_size)
                .map_err(|e| format!("设置画面尺寸失败: {e}"))?;
            out_type
                .SetUINT64(&MF_MT_FRAME_RATE, frame_rate)
                .map_err(|e| format!("设置帧率失败: {e}"))?;
            out_type
                .SetUINT32(&MF_MT_AVG_BITRATE, 8_000_000)
                .map_err(|e| format!("设置码率失败: {e}"))?;
            let _ = out_type.SetUINT32(&MF_MT_MPEG2_PROFILE, 66u32);
            let _ = out_type.SetUINT32(&MF_MT_MPEG2_LEVEL, 40u32);
            let _ = out_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32);
            encoder
                .SetOutputType(0, &out_type, 0)
                .map_err(|e| format!("设置编码器输出类型失败: {e}"))?;
            encoder
                .SetInputType(0, &in_type, 0)
                .map_err(|e| format!("设置编码器输入类型失败: {e}"))?;
            encoder
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|e| format!("开始编码流失败: {e}"))?;
            encoder
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|e| format!("启动编码流失败: {e}"))?;

            // 9. AAC 音频编码器（系统自带 MF AAC encoder MFT；不再使用 Dolby AC-3）
            let mut aac_enc: Option<IMFTransform> = None;
            let mut audio_active = has_audio;
            if audio_active {
                aac_enc = Some(create_aac_encoder(sample_rate, channels)?);
            }

            // 10. 自写 MP4 muxer（不依赖 SinkWriter；H.264 avc1 + AAC mp4a）
            let mut mux = Mp4Muxer::new(
                output,
                vw,
                vh,
                fps_num,
                fps_den,
                sample_rate,
                channels,
                audio_active,
                aac_asc(sample_rate, channels),
            )
            .map_err(|e| format!("创建输出文件失败: {e}"))?;

            // 11. 视频编码循环：按映射取帧 → 紧凑化 → 编码 → 写 muxer
            let vw_usize = vw as usize;
            let vh_usize = vh as usize;

            // 解码帧的真实几何：Media Foundation 解出的 NV12 高度会按 16 对齐
            // （1920x1080 → 1920x1088），即 Y 平面比画面多出 `raw_rows - vh` 行，
            // 而**UV 平面紧跟在完整 raw_rows 行 Y 平面之后**。
            //
            // 旧代码逐行累加 src_stride，Y 只拷了 vh 行就把 src_off 停在
            // `vh * src_stride`，直接当作 UV 起点 —— 比真实 UV 起点早了
            // `(raw_rows - vh) * src_stride` 字节（1080p 上是 8 行 = 15360 字节）。
            // 于是前 8 个色度行读到的是 Y 平面的尾部数据，色度整体上移
            // 16 个亮度行，表现为**画面上方绿条 + 整体颜色错位**。
            //
            // 正确做法：Y 只取前 vh 行，UV 必须从 `raw_rows * src_stride` 开始。
            let first_size = frame_sizes[0] as usize;
            let raw_rows = if vw_usize > 0 && first_size % vw_usize == 0 && first_size * 2 % (vw_usize * 3) == 0 {
                first_size * 2 / (vw_usize * 3)
            } else {
                vh_usize
            };
            let src_stride = if raw_rows > 0 && first_size * 2 % (raw_rows * 3) == 0 {
                first_size * 2 / (raw_rows * 3)
            } else {
                vw_usize
            };
            let uv_plane_offset = raw_rows.saturating_mul(src_stride);
            let mut out_buf: Vec<u8> = Vec::new();
            let mut compact_buf: Vec<u8> = Vec::new();
            let mut video_out_frames = 0usize;
            for i in 0..out_frame_count {
                let src = pick_frame(i);
                let size = frame_sizes[src] as usize;
                if out_buf.len() < size {
                    out_buf.resize(size, 0);
                }
                use std::io::Read;
                read_file
                    .seek(SeekFrom::Start(frame_offsets[src]))
                    .map_err(|e| format!("定位临时帧文件失败: {e}"))?;
                read_file
                    .read_exact(&mut out_buf[..size])
                    .map_err(|e| format!("读取临时帧文件失败: {e}"))?;
                // 解码帧可能带 stride / 高度对齐 padding，裁剪为紧凑 NV12 再喂编码器
                let need_compact = size != frame_bytes
                    && raw_rows >= vh_usize
                    && src_stride >= vw_usize
                    // UV 区必须真的够长，否则说明布局判断有误，宁可原样交给编码器
                    && uv_plane_offset + (vh_usize / 2) * src_stride <= size;
                let (feed_ptr, feed_len) = if need_compact {
                    if compact_buf.len() < frame_bytes {
                        compact_buf.resize(frame_bytes, 0);
                    }
                    let mut dst_off = 0usize;
                    let mut src_off = 0usize;
                    // Y 平面：取前 vh 行（丢弃末尾 raw_rows - vh 行对齐填充）
                    for _ in 0..vh_usize {
                        compact_buf[dst_off..dst_off + vw_usize]
                            .copy_from_slice(&out_buf[src_off..src_off + vw_usize]);
                        dst_off += vw_usize;
                        src_off += src_stride;
                    }
                    // UV 平面：定位到完整 Y 平面之后，而不是 vh * stride
                    src_off = uv_plane_offset;
                    for _ in 0..(vh_usize / 2) {
                        compact_buf[dst_off..dst_off + vw_usize]
                            .copy_from_slice(&out_buf[src_off..src_off + vw_usize]);
                        dst_off += vw_usize;
                        src_off += src_stride;
                    }
                    (compact_buf.as_ptr(), frame_bytes)
                } else {
                    (out_buf.as_ptr(), size)
                };
                let sample = MFCreateSample().map_err(|e| format!("创建样本失败: {e}"))?;
                let buffer = MFCreateMemoryBuffer(feed_len as u32)
                    .map_err(|e| format!("创建缓冲区失败: {e}"))?;
                let mut ptr: *mut u8 = std::ptr::null_mut();
                let mut max_len: u32 = 0;
                buffer
                    .Lock(&mut ptr, Some(&mut max_len), None)
                    .map_err(|e| format!("锁定缓冲区失败: {e}"))?;
                std::ptr::copy_nonoverlapping(feed_ptr, ptr, feed_len);
                buffer
                    .SetCurrentLength(feed_len as u32)
                    .map_err(|e| format!("设置缓冲区长度失败: {e}"))?;
                let _ = buffer.Unlock();
                sample
                    .AddBuffer(&buffer)
                    .map_err(|e| format!("添加缓冲区失败: {e}"))?;
                // 时间戳按源帧率计算（100ns 单位）。旧代码硬编码 333_333，
                // 源是 25/60fps 时输出会变成 30fps，播放速度与实际时长都不对。
                let frame_dur_100ns = if fps_num > 0 && fps_den > 0 {
                    (10_000_000i64 * fps_den as i64) / fps_num as i64
                } else {
                    333_333
                };
                let _ = sample.SetSampleTime(i as i64 * frame_dur_100ns);
                let _ = sample.SetSampleDuration(frame_dur_100ns);
                encoder
                    .ProcessInput(0, &sample, 0)
                    .map_err(|e| format!("编码视频帧失败: {e}"))?;
                // 同步 MFT：有输出立即取（内存恒定，无需节流）
                loop {
                    let st = encoder.GetOutputStatus().unwrap_or(0);
                    if st == 0 {
                        break;
                    }
                    let obuf = MFCreateMemoryBuffer(8_000_000)
                        .map_err(|e| format!("创建输出缓冲区失败: {e}"))?;
                    let mut op: *mut u8 = std::ptr::null_mut();
                    let mut om: u32 = 0;
                    let mut oc: u32 = 0;
                    obuf
                        .Lock(&mut op, Some(&mut om), Some(&mut oc))
                        .map_err(|e| format!("锁定输出缓冲区失败: {e}"))?;
                    let _ = obuf.Unlock();
                    let out_sample = MFCreateSample().map_err(|e| format!("创建输出样本失败: {e}"))?;
                    out_sample
                        .AddBuffer(&obuf)
                        .map_err(|e| format!("添加输出缓冲区失败: {e}"))?;
                    let events = MFCreateCollection().map_err(|e| format!("创建事件集合失败: {e}"))?;
                    let mut data = MFT_OUTPUT_DATA_BUFFER {
                        dwStreamID: 0,
                        pSample: std::mem::ManuallyDrop::new(Some(out_sample)),
                        dwStatus: 0,
                        pEvents: std::mem::ManuallyDrop::new(Some(events)),
                    };
                    let mut buffers = [data];
                    let mut out_status: u32 = 0;
                    match encoder.ProcessOutput(0, &mut buffers, &mut out_status) {
                        Ok(()) => {
                            if let Some(s) = buffers[0].pSample.take() {
                                let b = s.ConvertToContiguousBuffer().map_err(|e| format!("获取编码输出失败: {e}"))?;
                                let mut p2: *mut u8 = std::ptr::null_mut();
                                let mut m2: u32 = 0;
                                let mut c2: u32 = 0;
                                b.Lock(&mut p2, Some(&mut m2), Some(&mut c2)).map_err(|e| format!("锁定编码输出失败: {e}"))?;
                                let raw = std::slice::from_raw_parts(p2, c2 as usize);
                                mux.add_video(raw)?;
                                let _ = b.Unlock();
                                video_out_frames += 1;
                            }
                        }
                        Err(e) => {
                            if e.code() == windows::core::HRESULT(0xC00D6D72u32 as i32) {
                                break; // MF_E_TRANSFORM_NEED_MORE_INPUT
                            }
                            return Err(format!("编码输出失败: {e}"));
                        }
                    }
                }
                if i % 500 == 0 || i + 1 == out_frame_count {
                    eprintln!("[media-debug] video frames encoded: {}/{}", i + 1, out_frame_count);
                }
            }
            // 视频 drain
            let _ = encoder.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
            loop {
                let obuf = MFCreateMemoryBuffer(8_000_000)
                    .map_err(|e| format!("创建输出缓冲区失败: {e}"))?;
                let mut op: *mut u8 = std::ptr::null_mut();
                let mut om: u32 = 0;
                let mut oc: u32 = 0;
                obuf
                    .Lock(&mut op, Some(&mut om), Some(&mut oc))
                    .map_err(|e| format!("锁定输出缓冲区失败: {e}"))?;
                let _ = obuf.Unlock();
                let out_sample = MFCreateSample().map_err(|e| format!("创建输出样本失败: {e}"))?;
                out_sample
                    .AddBuffer(&obuf)
                    .map_err(|e| format!("添加输出缓冲区失败: {e}"))?;
                let events = MFCreateCollection().map_err(|e| format!("创建事件集合失败: {e}"))?;
                let mut data = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: std::mem::ManuallyDrop::new(Some(out_sample)),
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(Some(events)),
                };
                let mut buffers = [data];
                let mut out_status: u32 = 0;
                match encoder.ProcessOutput(0, &mut buffers, &mut out_status) {
                    Ok(()) => {
                        if let Some(s) = buffers[0].pSample.take() {
                            let b = s.ConvertToContiguousBuffer().map_err(|e| format!("获取编码输出失败: {e}"))?;
                            let mut p2: *mut u8 = std::ptr::null_mut();
                            let mut m2: u32 = 0;
                            let mut c2: u32 = 0;
                            b.Lock(&mut p2, Some(&mut m2), Some(&mut c2)).map_err(|e| format!("锁定编码输出失败: {e}"))?;
                            let raw = std::slice::from_raw_parts(p2, c2 as usize);
                            mux.add_video(raw)?;
                            let _ = b.Unlock();
                            video_out_frames += 1;
                        }
                    }
                    Err(e) => {
                        if e.code() == windows::core::HRESULT(0xC00D6D72u32 as i32) {
                            break;
                        }
                        return Err(format!("编码收尾失败: {e}"));
                    }
                }
            }
            eprintln!("[media-debug] video drain done: total out frames = {video_out_frames}");

            // 12. 音频编码循环（系统 AAC 编码器，每块 1024 帧）
            if let Some(aenc) = aac_enc.as_ref() {
                let audio_out_count = encode_audio_aac_loop(
                    aenc,
                    &mut mux,
                    &all_samples,
                    channels,
                    sample_rate,
                    audio_out_frames,
                    &pick_audio_frame,
                )?;
                eprintln!("[media-debug] audio done: out blocks = {audio_out_count}");
            }

            // 13. 完成 mux（写 moov）
            mux.finish().map_err(|e| format!("写入输出文件失败: {e}"))?;
            eprintln!("[media-debug] mux finished");
            Ok(warning)
        })();

        let _ = MFShutdown();
        CoUninitialize();
        result.map_err(|e| format!("音视频处理失败: {e}"))
    }
}

// ==================== 自写 MP4 muxer（H.264 avc1 + AAC mp4a） ====================

fn be32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn be64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn be16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn fourcc(out: &mut Vec<u8>, s: &[u8; 4]) {
    out.extend_from_slice(s);
}
fn box_start(out: &mut Vec<u8>, t: &[u8; 4]) -> usize {
    let pos = out.len();
    be32(out, 0);
    fourcc(out, t);
    pos
}
fn box_end(out: &mut Vec<u8>, pos: usize) {
    let size = (out.len() - pos) as u32;
    out[pos..pos + 4].copy_from_slice(&size.to_be_bytes());
}

/// AAC AudioSpecificConfig（2 字节）：AAC-LC + 采样率索引 + 声道配置
fn aac_asc(sr: u32, ch: u32) -> Vec<u8> {
    let freq_idx = match sr {
        96000 => 0,
        88200 => 1,
        64000 => 2,
        48000 => 3,
        44100 => 4,
        32000 => 5,
        24000 => 6,
        22050 => 7,
        16000 => 8,
        12000 => 9,
        11025 => 10,
        8000 => 11,
        7350 => 12,
        _ => 3,
    };
    let ch_cfg = match ch {
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 5,
        6 => 6,
        8 => 7,
        _ => 2,
    };
    // objectType=2 (AAC LC)
    let asc = (2u16 << 11) | ((freq_idx as u16) << 7) | ((ch_cfg as u16) << 3);
    vec![(asc >> 8) as u8, (asc & 0xFF) as u8]
}

/// 创建系统 AAC 编码器 MFT（输入 PCM 16bit，输出 raw AAC）
fn create_aac_encoder(sample_rate: u32, channels: u32) -> Result<IMFTransform, String> {
    unsafe {
        let aac_clsid = GUID::from_u128(0x93AF0C51_2275_45D2_A35B_F2BA21CAED00);
        let aenc: IMFTransform = CoCreateInstance(&aac_clsid, None, CLSCTX_ALL)
            .map_err(|e| format!("系统缺少 AAC 音频编码器: {e}"))?;
        let mut a_chosen: Option<IMFMediaType> = None;
        for t_idx in 0..8u32 {
            match aenc.GetOutputAvailableType(0, t_idx) {
                Ok(t) => {
                    let sub = t.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
                    if a_chosen.is_none() {
                        a_chosen = Some(t.clone());
                    }
                    if sub == MFAudioFormat_AAC {
                        a_chosen = Some(t);
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let mut a_out = a_chosen.ok_or("系统缺少 AAC 音频编码器输出类型")?;
        a_out
            .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, sample_rate)
            .map_err(|e| format!("设置采样率失败: {e}"))?;
        a_out
            .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)
            .map_err(|e| format!("设置声道数失败: {e}"))?;
        // 192kbps 目标码率（字节/秒）
        let _ = a_out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 192_000 / 8);
        aenc
            .SetOutputType(0, &a_out, 0)
            .map_err(|e| format!("设置音频编码器输出失败: {e}"))?;
        let mut pcm_in = MFCreateMediaType().map_err(|e| format!("创建媒体类型失败: {e}"))?;
        pcm_in
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(|e| format!("设置媒体类型失败: {e}"))?;
        pcm_in
            .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
            .map_err(|e| format!("设置媒体类型失败: {e}"))?;
        pcm_in
            .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, sample_rate)
            .map_err(|e| format!("设置采样率失败: {e}"))?;
        pcm_in
            .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)
            .map_err(|e| format!("设置声道数失败: {e}"))?;
        pcm_in
            .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .map_err(|e| format!("设置位深失败: {e}"))?;
        pcm_in
            .SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, sample_rate * channels * 2)
            .map_err(|e| format!("设置码率失败: {e}"))?;
        let _ = pcm_in.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, channels * 2);
        aenc
            .SetInputType(0, &pcm_in, 0)
            .map_err(|e| format!("设置音频编码器输入失败: {e}"))?;
        aenc
            .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
            .map_err(|e| format!("开始音频编码流失败: {e}"))?;
        aenc
            .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
            .map_err(|e| format!("启动音频编码流失败: {e}"))?;
        Ok(aenc)
    }
}

/// 若样本以 ADTS 同步字（0xFFF）开头则剥离 7 字节 ADTS 头，返回 raw AAC 帧
fn strip_adts(raw: &[u8]) -> &[u8] {
    if raw.len() > 9 && raw[0] == 0xFF && (raw[1] & 0xF0) == 0xF0 {
        // ADTS 头长度：无 CRC 7 字节 / 有 CRC 9 字节
        let has_crc = (raw[1] & 0x01) == 0;
        let hdr = if has_crc { 9usize } else { 7usize };
        if raw.len() > hdr {
            return &raw[hdr..];
        }
    }
    raw
}

/// 用系统 AAC 编码器把 PCM 采样按映射编码为 raw AAC 帧并写入 muxer（1024 采样/帧）
fn encode_audio_aac_loop(
    aenc: &IMFTransform,
    mux: &mut Mp4Muxer,
    all_samples: &[i16],
    channels: u32,
    sample_rate: u32,
    target_frame_count: usize,
    pick: impl Fn(usize) -> usize,
) -> Result<usize, String> {
    unsafe {
        let spf = channels as usize;
        let chunk_frames = 1024usize;
        let chunk_bytes = chunk_frames * spf * 2;
        let mut a_buf: Vec<i16> = vec![0i16; chunk_frames * spf];
        let mut offset = 0usize;
        let mut out_blocks = 0usize;
        while offset < target_frame_count {
            let count = std::cmp::min(chunk_frames, target_frame_count - offset);
            for j in 0..count {
                let src = pick(offset + j);
                let start = src * spf;
                let dst = j * spf;
                a_buf[dst..dst + spf].copy_from_slice(&all_samples[start..start + spf]);
            }
            let a_sample = MFCreateSample().map_err(|e| format!("创建音频样本失败: {e}"))?;
            let a_buffer = MFCreateMemoryBuffer(chunk_bytes as u32)
                .map_err(|e| format!("创建音频缓冲区失败: {e}"))?;
            let mut a_ptr: *mut u8 = std::ptr::null_mut();
            let mut a_max: u32 = 0;
            a_buffer
                .Lock(&mut a_ptr, Some(&mut a_max), None)
                .map_err(|e| format!("锁定音频缓冲区失败: {e}"))?;
            std::ptr::copy_nonoverlapping(a_buf.as_ptr(), a_ptr as *mut i16, count * spf);
            a_buffer
                .SetCurrentLength((count * spf * 2) as u32)
                .map_err(|e| format!("设置音频缓冲区长度失败: {e}"))?;
            let _ = a_buffer.Unlock();
            a_sample
                .AddBuffer(&a_buffer)
                .map_err(|e| format!("添加音频缓冲区失败: {e}"))?;
            let _ = a_sample.SetSampleTime((offset as i64) * 1024 * 10_000_000 / (sample_rate as i64));
            let _ = a_sample.SetSampleDuration(1024 * 10_000_000 / (sample_rate as i64));
            aenc
                .ProcessInput(0, &a_sample, 0)
                .map_err(|e| format!("编码音频块失败: {e}"))?;
            loop {
                let obuf = MFCreateMemoryBuffer(64 * 1024)
                    .map_err(|e| format!("创建音频输出缓冲区失败: {e}"))?;
                let mut op: *mut u8 = std::ptr::null_mut();
                let mut om: u32 = 0;
                let mut oc: u32 = 0;
                obuf
                    .Lock(&mut op, Some(&mut om), Some(&mut oc))
                    .map_err(|e| format!("锁定音频输出缓冲区失败: {e}"))?;
                let _ = obuf.Unlock();
                let out_sample = MFCreateSample().map_err(|e| format!("创建音频输出样本失败: {e}"))?;
                out_sample
                    .AddBuffer(&obuf)
                    .map_err(|e| format!("添加音频输出缓冲区失败: {e}"))?;
                let events = MFCreateCollection().map_err(|e| format!("创建事件集合失败: {e}"))?;
                let mut data = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: std::mem::ManuallyDrop::new(Some(out_sample)),
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(Some(events)),
                };
                let mut buffers = [data];
                let mut out_status: u32 = 0;
                match aenc.ProcessOutput(0, &mut buffers, &mut out_status) {
                    Ok(()) => {
                        if let Some(s) = buffers[0].pSample.take() {
                            let b = s.ConvertToContiguousBuffer().map_err(|e| format!("获取音频编码输出失败: {e}"))?;
                            let mut p2: *mut u8 = std::ptr::null_mut();
                            let mut m2: u32 = 0;
                            let mut c2: u32 = 0;
                            b.Lock(&mut p2, Some(&mut m2), Some(&mut c2)).map_err(|e| format!("锁定音频编码输出失败: {e}"))?;
                            let raw = std::slice::from_raw_parts(p2, c2 as usize);
                            let aac = strip_adts(raw);
                            mux.add_audio(aac)?;
                            let _ = b.Unlock();
                            out_blocks += 1;
                        }
                    }
                    Err(e) => {
                        if e.code() == windows::core::HRESULT(0xC00D6D72u32 as i32) {
                            break;
                        }
                        return Err(format!("音频编码输出失败: {e}"));
                    }
                }
            }
            offset += count;
        }
        // 音频 drain
        let _ = aenc.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
        loop {
            let obuf = MFCreateMemoryBuffer(64 * 1024)
                .map_err(|e| format!("创建音频输出缓冲区失败: {e}"))?;
            let mut op: *mut u8 = std::ptr::null_mut();
            let mut om: u32 = 0;
            let mut oc: u32 = 0;
            obuf
                .Lock(&mut op, Some(&mut om), Some(&mut oc))
                .map_err(|e| format!("锁定音频输出缓冲区失败: {e}"))?;
            let _ = obuf.Unlock();
            let out_sample = MFCreateSample().map_err(|e| format!("创建音频输出样本失败: {e}"))?;
            out_sample
                .AddBuffer(&obuf)
                .map_err(|e| format!("添加音频输出缓冲区失败: {e}"))?;
            let events = MFCreateCollection().map_err(|e| format!("创建事件集合失败: {e}"))?;
            let mut data = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: std::mem::ManuallyDrop::new(Some(out_sample)),
                dwStatus: 0,
                pEvents: std::mem::ManuallyDrop::new(Some(events)),
            };
            let mut buffers = [data];
            let mut out_status: u32 = 0;
            match aenc.ProcessOutput(0, &mut buffers, &mut out_status) {
                Ok(()) => {
                    if let Some(s) = buffers[0].pSample.take() {
                        let b = s.ConvertToContiguousBuffer().map_err(|e| format!("获取音频编码输出失败: {e}"))?;
                        let mut p2: *mut u8 = std::ptr::null_mut();
                        let mut m2: u32 = 0;
                        let mut c2: u32 = 0;
                        b.Lock(&mut p2, Some(&mut m2), Some(&mut c2)).map_err(|e| format!("锁定音频编码输出失败: {e}"))?;
                        let raw = std::slice::from_raw_parts(p2, c2 as usize);
                        let aac = strip_adts(raw);
                        mux.add_audio(aac)?;
                        let _ = b.Unlock();
                        out_blocks += 1;
                    }
                }
                Err(e) => {
                    if e.code() == windows::core::HRESULT(0xC00D6D72u32 as i32) {
                        break;
                    }
                    return Err(format!("音频编码收尾失败: {e}"));
                }
            }
        }
        Ok(out_blocks)
    }
}
/// Annex B（起始码）→ AVCC（4 字节大端长度前缀）；跳过 AUD（nal_type 9）
fn annexb_to_avcc(data: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        let mut nal_start = i;
        if data[i..].starts_with(&[0u8, 0, 0, 1]) {
            nal_start = i + 4;
        } else if data[i..].starts_with(&[0u8, 0, 1]) {
            nal_start = i + 3;
        }
        let mut j = nal_start;
        let mut next = data.len();
        while j + 3 < data.len() {
            if data[j] == 0
                && data[j + 1] == 0
                && (data[j + 2] == 1 || (data[j + 2] == 0 && data[j + 3] == 1))
            {
                next = j;
                break;
            }
            j += 1;
        }
        let nal = &data[nal_start..next];
        if !nal.is_empty() {
            let nal_type = nal[0] & 0x1F;
            if nal_type != 9 {
                out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                out.extend_from_slice(nal);
            }
        }
        i = next;
        if next >= data.len() {
            break;
        }
    }
    out
}

fn extract_sps_pps(data: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut sps: Vec<u8> = Vec::new();
    let mut pps: Vec<u8> = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        let mut nal_start = i;
        if data[i..].starts_with(&[0u8, 0, 0, 1]) {
            nal_start = i + 4;
        } else if data[i..].starts_with(&[0u8, 0, 1]) {
            nal_start = i + 3;
        }
        let mut j = nal_start;
        let mut next = data.len();
        while j + 3 < data.len() {
            if data[j] == 0
                && data[j + 1] == 0
                && (data[j + 2] == 1 || (data[j + 2] == 0 && data[j + 3] == 1))
            {
                next = j;
                break;
            }
            j += 1;
        }
        let nal = &data[nal_start..next];
        if !nal.is_empty() {
            let t = nal[0] & 0x1F;
            if t == 7 {
                sps = nal.to_vec();
            } else if t == 8 {
                pps = nal.to_vec();
            }
        }
        i = next;
        if next >= data.len() {
            break;
        }
    }
    (sps, pps)
}

struct Mp4Muxer {
    file: std::fs::File,
    vw: u32,
    vh: u32,
    v_fps_num: u32,
    v_fps_den: u32,
    v_sps: Vec<u8>,
    v_pps: Vec<u8>,
    v_sample_sizes: Vec<u32>,
    v_sample_offsets: Vec<u64>,
    /// 每个视频样本的**合成时间偏移**（Composition Time Offset，单位=时间刻度）。
    ///
    /// 为什么必须有：B 站等来源的 AVC 流含 B 帧（双向预测帧），B 帧的**解码顺序**
    /// 与**显示顺序**不同。MP4 里由 `stts` 描述解码时间、由 `ctts` 描述合成（显示）
    /// 时间偏移。若只写 stts 不写 ctts，所有帧的显示时间等于解码时间，
    /// 解码器按错序输出 B 帧 → 画面无法正确解码，播放器报「不支持的 avc1 格式」
    /// 并只放音频（实测 42/42 个 trun 均携带非零 cto，值如 277/1044/46…）。
    v_sample_ctos: Vec<i64>,
    sr: u32,
    ch: u32,
    a_sample_sizes: Vec<u32>,
    a_sample_offsets: Vec<u64>,
    a_cfg: Vec<u8>,
    ftyp_len: usize,
    data_start: u64,
    file_pos: u64,
    has_audio: bool,
}

impl Mp4Muxer {
    fn new(
        path: &str,
        vw: u32,
        vh: u32,
        fnum: u32,
        fden: u32,
        sr: u32,
        ch: u32,
        has_audio: bool,
        a_cfg: Vec<u8>,
    ) -> Result<Self, String> {
        let mut file = std::fs::File::create(path).map_err(|e| format!("创建输出文件失败: {e}"))?;
        let mut m = Mp4Muxer {
            file,
            vw,
            vh,
            v_fps_num: fnum,
            v_fps_den: fden,
            v_sps: Vec::new(),
            v_pps: Vec::new(),
            v_sample_sizes: Vec::new(),
            v_sample_offsets: Vec::new(),
            v_sample_ctos: Vec::new(),
            sr,
            ch,
            a_sample_sizes: Vec::new(),
            a_sample_offsets: Vec::new(),
            a_cfg,
            ftyp_len: 0,
            data_start: 0,
            file_pos: 0,
            has_audio,
        };
        m.write_ftyp();
        m.write_mdat_header();
        Ok(m)
    }

    fn write_ftyp(&mut self) {
        let mut b = Vec::new();
        box_start(&mut b, b"ftyp");
        fourcc(&mut b, b"isom");
        be32(&mut b, 0);
        fourcc(&mut b, b"isom");
        fourcc(&mut b, b"iso2");
        fourcc(&mut b, b"avc1");
        fourcc(&mut b, b"mp41");
        box_end(&mut b, 0);
        use std::io::Write;
        let _ = self.file.write_all(&b);
        self.file_pos += b.len() as u64;
        self.ftyp_len = b.len();
    }

    fn write_mdat_header(&mut self) {
        // ⚠️ mdat 是 MP4 里最大的 box，慢放时轻易超过 4 GiB。
        // 普通 box 头只有 32 位 size 字段，最大 4294967295 字节（≈4 GiB）。
        // 一旦越过该边界，回填的 size 会被 `as u32` 截断成错误值，
        // 播放器会认为 mdat 长度不对而直接判整个文件损坏（打不开）。
        //
        // 解决：先**预留 large size 形式**的头（size32 = 1，紧跟 64 位 largesize），
        // 这样无论最终多大都能正确回填，且不必在结尾处搬移数据。
        // 实测 0.01x 慢放 187s 源会产出约 12 GB，必然越过该边界。
        //
        // 布局：[size=1][mdat][largesize(u64)]  —— 头共 4+4+8 = 16 字节。
        let mut b = Vec::new();
        be32(&mut b, 1); // size32 = 1 表示「使用 largesize」
        fourcc(&mut b, b"mdat");
        be64(&mut b, 0); // largesize 占位，finish() 时回填
        use std::io::Write;
        let _ = self.file.write_all(&b);
        self.file_pos += b.len() as u64;
        self.data_start = self.file_pos;
    }

    fn add_video(&mut self, annexb: &[u8]) -> Result<(), String> {
        use std::io::Write;
        if self.v_sps.is_empty() {
            let (s, p) = extract_sps_pps(annexb);
            self.v_sps = s;
            self.v_pps = p;
        }
        let avcc = annexb_to_avcc(annexb);
        // co64/stco 要求相对文件开头的绝对偏移（不是相对 mdat data 的偏移）
        let off = self.file_pos;
        self.v_sample_offsets.push(off);
        self.v_sample_sizes.push(avcc.len() as u32);
        self.file
            .write_all(&avcc)
            .map_err(|e| format!("写入视频样本失败: {e}"))?;
        self.file_pos += avcc.len() as u64;
        Ok(())
    }

    /// 写入一个视频样本，并记录其合成时间偏移（用于生成 ctts）。
    ///
    /// `cto` 单位与 mdhd.timescale 一致（本项目视频轨时间刻度为 1e7）。
    /// 调用方需把源流里的 cto（源 timescale 刻度）换算到 1e7 刻度后再传入。
    fn add_video_with_cto(&mut self, annexb: &[u8], cto: i64) -> Result<(), String> {
        self.add_video(annexb)?;
        self.v_sample_ctos.push(cto);
        Ok(())
    }

    fn add_audio(&mut self, raw: &[u8]) -> Result<(), String> {
        use std::io::Write;
        // co64/stco 要求相对文件开头的绝对偏移（不是相对 mdat data 的偏移）
        let off = self.file_pos;
        self.a_sample_offsets.push(off);
        self.a_sample_sizes.push(raw.len() as u32);
        self.file
            .write_all(raw)
            .map_err(|e| format!("写入音频样本失败: {e}"))?;
        self.file_pos += raw.len() as u64;
        Ok(())
    }

    fn finish(mut self) -> Result<(), String> {
        use std::io::{Seek, Write};
        // largesize 覆盖「size32 + fourcc + largesize」整个头，共 16 字节。
        // 与 write_mdat_header 预留的布局严格对应。
        let mdat_size: u64 = (self.file_pos - self.data_start) + 16;
        let mdat_size_pos = self.ftyp_len as u64;
        self.file
            .seek(std::io::SeekFrom::Start(mdat_size_pos))
            .map_err(|e| format!("定位 mdat 失败: {e}"))?;
        // size32 = 1 已在头部写好；这里只需回填 64 位 largesize。
        // 跳过 4 字节 size32 与 4 字节 fourcc，直接写 largesize。
        self.file
            .seek(std::io::SeekFrom::Start(mdat_size_pos + 8))
            .map_err(|e| format!("定位 mdat largesize 失败: {e}"))?;
        self.file
            .write_all(&mdat_size.to_be_bytes())
            .map_err(|e| format!("回填 mdat largesize 失败: {e}"))?;
        self.file
            .seek(std::io::SeekFrom::Start(self.file_pos))
            .map_err(|e| format!("定位末尾失败: {e}"))?;
        let moov = self.build_moov();
        self.file
            .write_all(&moov)
            .map_err(|e| format!("写 moov 失败: {e}"))?;
        self.file.flush().map_err(|e| format!("刷新文件失败: {e}"))?;
        Ok(())
    }

    fn build_moov(&self) -> Vec<u8> {
        let mut moov = Vec::new();
        let mpos = box_start(&mut moov, b"moov");
        let v_dur = if self.v_fps_num > 0 {
            (self.v_sample_sizes.len() as u64) * 10_000_000u64 * (self.v_fps_den as u64)
                / (self.v_fps_num as u64)
        } else {
            0
        };
        let a_total_samples = (self.a_sample_sizes.len() as u64) * 1024u64;
        let a_dur = if self.sr > 0 {
            a_total_samples * 10_000_000u64 / (self.sr as u64)
        } else {
            0
        };
        let total_dur = v_dur.max(a_dur);
        {
            let p = box_start(&mut moov, b"mvhd");
            // 与 tkhd/mdhd 同理：总时长超过 32 位（≈429.5s）时必须用 version=1 的 64 位字段，
            // 否则慢放/长视频的 mvhd 会被截断，播放器据此判定影片提前结束。
            if total_dur > 0xFFFF_FFFF {
                be32(&mut moov, 0x0100_0000); // version=1, flags=0
                be64(&mut moov, 0); // creation_time
                be64(&mut moov, 0); // modification_time
                be32(&mut moov, 10_000_000);
                be64(&mut moov, total_dur); // duration (64-bit)
            } else {
                be32(&mut moov, 0);
                be32(&mut moov, 0);
                be32(&mut moov, 0);
                be32(&mut moov, 10_000_000);
                be32(&mut moov, total_dur as u32);
            }
            be32(&mut moov, 0x0001_0000);
            be16(&mut moov, 0x0100);
            be16(&mut moov, 0);
            be32(&mut moov, 0);
            be32(&mut moov, 0);
            for &v in &[
                0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000,
            ] {
                be32(&mut moov, v);
            }
            for _ in 0..6 {
                be32(&mut moov, 0);
            }
            be32(&mut moov, if self.has_audio { 3 } else { 2 });
            box_end(&mut moov, p);
        }
        if !self.v_sample_sizes.is_empty() {
            self.build_video_trak(&mut moov, v_dur);
        }
        if !self.a_sample_sizes.is_empty() {
            self.build_audio_trak(&mut moov, a_dur, a_total_samples);
        }
        box_end(&mut moov, mpos);
        moov
    }

    fn build_video_trak(&self, moov: &mut Vec<u8>, v_dur: u64) {
        // ⚠️ 32 位时长溢出。
        //
        // `v_dur` 的单位是 100ns，所以 0xFFFF_FFFF ≈ 429.5 秒。慢放（如 0.25x）会让
        // 视频轨时长轻易超过这个值：实测 187.3s 的源慢放 4 倍后 v_dur = 7,491,992,508，
        // 旧代码用 `v_dur.min(0xFFFF_FFFF)` 截断成 429.5s，而音频轨（timescale=48000，
        // 用样本数计时）仍是 749.2s —— 结果音视频轨时长差 320 秒，播放器表现就是
        // 画面只放到一半就停止、之后长时间黑屏但声音还在。
        //
        // 正确做法：ISO/MP4 的 tkhd/mdhd 在 version=1 时用 64 位字段存 duration。
        // 这里按需切换版本，短时长仍用 version=0（兼容性最好），超长才升到 version=1。
        let need_v1 = v_dur > 0xFFFF_FFFF;
        let tpos = box_start(moov, b"trak");
        {
            let p = box_start(moov, b"tkhd");
            if need_v1 {
                // version=1, flags=3（启用 + 用于presentation）
                be32(moov, 0x0100_0003);
                be64(moov, 0); // creation_time
                be64(moov, 0); // modification_time
                be32(moov, 1); // track_ID
                be32(moov, 0); // reserved
                be64(moov, v_dur); // duration (64-bit)
            } else {
                be32(moov, 0x0000_0003);
                be32(moov, 0);
                be32(moov, 0);
                be32(moov, 1);
                be32(moov, 0);
                be32(moov, v_dur as u32);
            }
            be32(moov, 0);
            be32(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            for &v in &[
                0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000,
            ] {
                be32(moov, v);
            }
            be32(moov, self.vw << 16);
            be32(moov, self.vh << 16);
            box_end(moov, p);
        }
        {
            let p = box_start(moov, b"mdia");
            let p2 = box_start(moov, b"mdhd");
            // 视频轨 timescale 固定 1e7（100ns 刻度），与 stts 的 delta 单位一致。
            if need_v1 {
                be32(moov, 0x0100_0000); // version=1, flags=0
                be64(moov, 0); // creation_time
                be64(moov, 0); // modification_time
                be32(moov, 10_000_000); // timescale
                be64(moov, v_dur); // duration (64-bit)
            } else {
                be32(moov, 0);
                be32(moov, 0);
                be32(moov, 0);
                be32(moov, 10_000_000);
                be32(moov, v_dur as u32);
            }
            be16(moov, 0x55C4);
            be16(moov, 0);
            box_end(moov, p2);
            let p2 = box_start(moov, b"hdlr");
            be32(moov, 0);
            be32(moov, 0);
            fourcc(moov, b"vide");
            be32(moov, 0);
            be32(moov, 0);
            be32(moov, 0);
            moov.extend_from_slice(b"VideoHandler\0");
            box_end(moov, p2);
            let p2 = box_start(moov, b"minf");
            let p3 = box_start(moov, b"vmhd");
            be32(moov, 1);
            be16(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            box_end(moov, p3);
            let p3 = box_start(moov, b"dinf");
            let p4 = box_start(moov, b"dref");
            be32(moov, 0);
            be32(moov, 1);
            let p5 = box_start(moov, b"url ");
            be32(moov, 1);
            box_end(moov, p5);
            box_end(moov, p4);
            box_end(moov, p3);
            let p3 = box_start(moov, b"stbl");
            let p4 = box_start(moov, b"stsd");
            be32(moov, 0);
            be32(moov, 1);
            let p5 = box_start(moov, b"avc1");
            // ⚠️ VisualSampleEntry 的字段布局（ISO/IEC 14496-12 §12.1.3）。
            // 这里曾经把 reserved 写成 be32+be32（8 字节），比规范多 2 字节，
            // 导致其后所有字段（含 width/height/depth/frame_count）整体错位 2 字节：
            // 实测生成的 avc1 里 reserved[6] 变成了 "avc1" 字样、width/height 读到 0、
            // frame_count 变成 0 —— 播放器无法解析 sample entry，
            // 于是报「不支持的 avc1 格式」，表现就是只有声音没有画面。
            //
            // 正确布局（紧随 box_start 写出的 8 字节 box 头之后）：
            //   reserved[6]           6 字节 全 0
            //   data_reference_index  2 字节 = 1（引用 dref 第 1 项）
            //   pre_defined           2 字节 = 0
            //   reserved              2 字节 = 0
            //   pre_defined[3]       12 字节 全 0
            //   width                 2 字节
            //   height                2 字节
            //   horizresolution       4 字节 = 0x00480000 (72dpi)
            //   vertresolution        4 字节 = 0x00480000 (72dpi)
            //   reserved              4 字节 = 0
            //   frame_count           2 字节 = 1
            //   compressorname       32 字节 全 0（长度字节 + 内容）
            //   depth                 2 字节 = 0x0018 (24)
            //   pre_defined           2 字节 = -1 (0xFFFF)
            be32(moov, 0); // reserved[0..4]
            be16(moov, 0); // reserved[4..6]
            be16(moov, 1); // data_reference_index = 1
            be16(moov, 0); // pre_defined
            be16(moov, 0); // reserved
            for _ in 0..3 {
                be32(moov, 0); // pre_defined[3] = 12 字节
            }
            be16(moov, self.vw as u16); // width
            be16(moov, self.vh as u16); // height
            be32(moov, 0x0048_0000); // horizresolution
            be32(moov, 0x0048_0000); // vertresolution
            be32(moov, 0); // reserved
            be16(moov, 1); // frame_count
            for _ in 0..32 {
                moov.push(0); // compressorname（长度字节 + 31 字节内容，全 0）
            }
            be16(moov, 0x0018); // depth = 24
            be16(moov, 0xFFFF); // pre_defined = -1
            let p6 = box_start(moov, b"avcC");
            moov.push(1);
            if self.v_sps.len() >= 4 {
                moov.push(self.v_sps[1]);
                moov.push(self.v_sps[2]);
                moov.push(self.v_sps[3]);
            } else {
                moov.push(66);
                moov.push(0);
                moov.push(40);
            }
            moov.push(0xFC | 0x03);
            moov.push(0xE0 | 1);
            let sps_len = self.v_sps.len().min(0xFFFF) as u16;
            be16(moov, sps_len);
            moov.extend_from_slice(&self.v_sps);
            moov.push(1);
            let pps_len = self.v_pps.len().min(0xFFFF) as u16;
            be16(moov, pps_len);
            moov.extend_from_slice(&self.v_pps);
            box_end(moov, p6);
            box_end(moov, p5);
            box_end(moov, p4);
            let p4 = box_start(moov, b"stts");
            be32(moov, 0);
            be32(moov, 1);
            be32(moov, self.v_sample_sizes.len() as u32);
            let v_delta = if self.v_fps_num > 0 {
                (10_000_000u64 * (self.v_fps_den as u64) / (self.v_fps_num as u64)) as u32
            } else {
                333333
            };
            be32(moov, v_delta);
            box_end(moov, p4);

            // ctts（Composition Time to Sample）：描述每个样本的显示时间相对解码时间的偏移。
            //
            // 这是 B 帧能正确播放的关键。AVC 流含 B 帧时，解码顺序 ≠ 显示顺序：
            //   - stts 给出解码时间（DTS）
            //   - ctts 给出合成时间偏移（CTO），显示时间 PTS = DTS + CTO
            // 缺少 ctts 时解码器认为 PTS == DTS，B 帧会被按错误顺序输出，
            // 表现为画面无法解码（播放器报「不支持的 avc1 格式」，只放音频）。
            //
            // 实测 B 站源流的 trun 全部携带非零 cto（如 277/1044/46…，源时间刻度 16000），
            // 因此这里必须写入。box 顺序遵循 ISO 14496-12：stts → ctts → stsc → stsz → stco/co64。
            //
            // 写入策略：CTO 通常呈周期性重复，可合并为「(样本数, 偏移)」计数对；
            // version=0 时偏移为无符号（全非负才可用），否则用 version=1（有符号）。
            if self.v_sample_ctos.len() == self.v_sample_sizes.len() && !self.v_sample_ctos.is_empty() {
                // 合并相邻相同偏移，减少表项数
                let mut runs: Vec<(u32, i64)> = Vec::new();
                for &cto in &self.v_sample_ctos {
                    match runs.last_mut() {
                        Some((cnt, val)) if *val == cto => *cnt += 1,
                        _ => runs.push((1, cto)),
                    }
                }
                let all_non_negative = self.v_sample_ctos.iter().all(|&c| c >= 0);
                let p4 = box_start(moov, b"ctts");
                if all_non_negative {
                    // version=0：off 为 u32
                    be32(moov, 0);
                    be32(moov, runs.len() as u32);
                    for (cnt, val) in &runs {
                        be32(moov, *cnt);
                        be32(moov, *val as u32);
                    }
                } else {
                    // version=1：off 为 i32（允许负偏移）
                    be32(moov, 0x0100_0000);
                    be32(moov, runs.len() as u32);
                    for (cnt, val) in &runs {
                        be32(moov, *cnt);
                        // version=1 时该字段按有符号 i32 解释；
                        // be32 只接受 u32，故按二进制补码转回（负数会自然变成其补码）
                        be32(moov, *val as i32 as u32);
                    }
                }
                box_end(moov, p4);
            }

            let p4 = box_start(moov, b"stsc");
            be32(moov, 0);
            be32(moov, 1);
            be32(moov, 1);
            be32(moov, self.v_sample_sizes.len() as u32);
            be32(moov, 1);
            box_end(moov, p4);
            let p4 = box_start(moov, b"stsz");
            be32(moov, 0);
            be32(moov, 0);
            be32(moov, self.v_sample_sizes.len() as u32);
            for s in &self.v_sample_sizes {
                be32(moov, *s);
            }
            box_end(moov, p4);
            let p4 = box_start(moov, b"co64");
            be32(moov, 0);
            be32(moov, self.v_sample_offsets.len() as u32);
            for o in &self.v_sample_offsets {
                be64(moov, *o);
            }
            box_end(moov, p4);
            box_end(moov, p3);
            box_end(moov, p2);
            box_end(moov, p);
        }
        box_end(moov, tpos);
    }

    fn build_audio_trak(&self, moov: &mut Vec<u8>, a_dur: u64, a_total_samples: u64) {
        // 音频侧同样有 32 位边界，但判据不同：
        // - tkhd.duration 是 100ns 刻度（同视频），超过 429.5s 需 version=1；
        // - mdhd.duration 是**采样数**（timescale=采样率），64 位上限极高但与 tkhd 分开判断，
        //   因为 sr 可能是 8000 这类低采样率，此时时长很长但样本数不大。
        let tkhd_v1 = a_dur > 0xFFFF_FFFF;
        let mdhd_v1 = a_total_samples > 0xFFFF_FFFF;
        let tpos = box_start(moov, b"trak");
        {
            let p = box_start(moov, b"tkhd");
            if tkhd_v1 {
                be32(moov, 0x0100_0003);
                be64(moov, 0);
                be64(moov, 0);
                be32(moov, 2);
                be32(moov, 0);
                be64(moov, a_dur);
            } else {
                be32(moov, 0x0000_0003);
                be32(moov, 0);
                be32(moov, 0);
                be32(moov, 2);
                be32(moov, 0);
                be32(moov, a_dur as u32);
            }
            be32(moov, 0);
            be32(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            for &v in &[
                0x00010000u32, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000,
            ] {
                be32(moov, v);
            }
            be32(moov, 0);
            be32(moov, 0);
            box_end(moov, p);
        }
        {
            let p = box_start(moov, b"mdia");
            let p2 = box_start(moov, b"mdhd");
            if mdhd_v1 {
                be32(moov, 0x0100_0000);
                be64(moov, 0);
                be64(moov, 0);
                be32(moov, self.sr);
                be64(moov, a_total_samples);
            } else {
                be32(moov, 0);
                be32(moov, 0);
                be32(moov, 0);
                be32(moov, self.sr);
                be32(moov, a_total_samples as u32);
            }
            be16(moov, 0x55C4);
            be16(moov, 0);
            box_end(moov, p2);
            let p2 = box_start(moov, b"hdlr");
            be32(moov, 0);
            be32(moov, 0);
            fourcc(moov, b"soun");
            be32(moov, 0);
            be32(moov, 0);
            be32(moov, 0);
            moov.extend_from_slice(b"SoundHandler\0");
            box_end(moov, p2);
            let p2 = box_start(moov, b"minf");
            let p3 = box_start(moov, b"smhd");
            be32(moov, 0);
            be16(moov, 0);
            be16(moov, 0);
            box_end(moov, p3);
            let p3 = box_start(moov, b"dinf");
            let p4 = box_start(moov, b"dref");
            be32(moov, 0);
            be32(moov, 1);
            let p5 = box_start(moov, b"url ");
            be32(moov, 1);
            box_end(moov, p5);
            box_end(moov, p4);
            box_end(moov, p3);
            let p3 = box_start(moov, b"stbl");
            let p4 = box_start(moov, b"stsd");
            be32(moov, 0);
            be32(moov, 1);
            let p5 = box_start(moov, b"mp4a");
            be32(moov, 0); // reserved 前 4 字节（共 6 字节 reserved + data_ref）
            be16(moov, 0);
            be16(moov, 1); // data_reference_index = 1（引用 dref 第 1 项；为 0 会被 SourceReader 丢弃音轨）
            be16(moov, 0); // version
            be16(moov, 0); // revision
            be32(moov, 0); // vendor
            be16(moov, self.ch as u16); // channelcount
            be16(moov, 16); // samplesize
            be16(moov, 0); // pre_defined
            be16(moov, 0); // reserved
            be32(moov, self.sr << 16); // samplerate
            // esds box：ES_Descriptor → DecoderConfigDescriptor → AudioSpecificConfig
            // 注意：descriptor 长度必须用多字节 7-bit 编码（0x80 0x80 0x80 xx），
            // 与 Windows 自带 MP4 源文件一致；单字节长度（03 19）实测不被 Windows 解析器接受
            let asc = if self.a_cfg.len() >= 2 {
                self.a_cfg[..2].to_vec()
            } else {
                vec![0x13, 0x10]
            };
            let dsi_len = asc.len() as u32; // 2
            // DCD payload = objectType(1)+streamType(1)+buffer(3)+max(4)+avg(4)+DSI(1+4+asc)=13+7=20
            let dcd_content_len = 13u32 + (1 + 4 + dsi_len);
            // ES payload = ES_ID(2)+flags(1)+DCD(1+4+20)+SL(1+4+1)=3+25+6=34
            let esd_content_len = 3u32 + (1 + 4 + dcd_content_len) + 6;
            let mut esds = Vec::new();
            be32(&mut esds, 0); // version + flags
            esds.push(0x03);
            esds.extend_from_slice(&[0x80, 0x80, 0x80, esd_content_len as u8]);
            be16(&mut esds, 2); // ES_ID（源文件为 0x0002）
            esds.push(0); // flags
            esds.push(0x04);
            esds.extend_from_slice(&[0x80, 0x80, 0x80, dcd_content_len as u8]);
            esds.push(0x40); // objectTypeIndication: MPEG-4 Audio
            esds.push(0x15); // streamType: audio (0x15)
            esds.extend_from_slice(&[0, 0, 0]); // bufferSizeDB
            esds.extend_from_slice(&[0x00, 0x02, 0xee, 0x00]); // maxBitrate = 192000
            esds.extend_from_slice(&[0x00, 0x02, 0xee, 0x00]); // avgBitrate = 192000
            esds.push(0x05); // DecoderSpecificInfo
            esds.extend_from_slice(&[0x80, 0x80, 0x80, dsi_len as u8]);
            esds.extend_from_slice(&asc);
            esds.push(0x06); // SLConfigDescriptor
            esds.extend_from_slice(&[0x80, 0x80, 0x80, 0x01]);
            esds.push(0x02);
            let p6 = box_start(moov, b"esds");
            moov.extend_from_slice(&esds);
            box_end(moov, p6);
            box_end(moov, p5);
            box_end(moov, p4);
            let p4 = box_start(moov, b"stts");
            be32(moov, 0);
            be32(moov, 1);
            be32(moov, self.a_sample_sizes.len() as u32);
            be32(moov, 1024);
            box_end(moov, p4);
            let p4 = box_start(moov, b"stsc");
            be32(moov, 0);
            be32(moov, 1);
            be32(moov, 1);
            be32(moov, self.a_sample_sizes.len() as u32);
            be32(moov, 1);
            box_end(moov, p4);
            let p4 = box_start(moov, b"stsz");
            be32(moov, 0);
            be32(moov, 0);
            be32(moov, self.a_sample_sizes.len() as u32);
            for s in &self.a_sample_sizes {
                be32(moov, *s);
            }
            box_end(moov, p4);
            let p4 = box_start(moov, b"co64");
            be32(moov, 0);
            be32(moov, self.a_sample_offsets.len() as u32);
            for o in &self.a_sample_offsets {
                be64(moov, *o);
            }
            box_end(moov, p4);
            box_end(moov, p3);
            box_end(moov, p2);
            box_end(moov, p);
        }
        box_end(moov, tpos);
    }
}


/// 把 PCM 采样按映射编码为 AAC 并写入自写 MP4（仅音轨；系统 MF AAC 编码器，不再使用 Dolby AC-3）
fn write_audio_aac_mp4(
    output: &str,
    all_samples: &[i16],
    channels: u32,
    sample_rate: u32,
    target_frame_count: usize,
    pick: impl Fn(usize) -> usize,
) -> Result<(), String> {
    unsafe {
        let aenc = create_aac_encoder(sample_rate, channels)?;
        let mut mux = Mp4Muxer::new(
            output,
            0,
            0,
            0,
            0,
            sample_rate,
            channels,
            true,
            aac_asc(sample_rate, channels),
        )
        .map_err(|e| format!("创建输出文件失败: {e}"))?;
        encode_audio_aac_loop(
            &aenc,
            &mut mux,
            all_samples,
            channels,
            sample_rate,
            target_frame_count,
            &pick,
        )?;
        mux.finish().map_err(|e| format!("写入输出文件失败: {e}"))
    }
}