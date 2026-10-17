//! 文件格式转换工具：
//! - 图片格式（png/jpg/jpeg/webp/bmp）相互转换（image crate）
//! - 音频格式（wav/mp3/m4a）相互转换（Windows Media Foundation 系统转码）
//!
//! 音频转换依赖 Windows 10/11 自带的 Media Foundation 编码器（MP3/AAC/PCM），
//! 无需外部 ffmpeg；输出容器与编码器由输出文件扩展名决定。

use std::path::Path;

use windows::core::{GUID, PCWSTR};
use windows::Win32::Media::MediaFoundation::{
    MFShutdown, MFStartup, MFCreateMediaType, MFCreateSinkWriterFromURL,
    MFCreateSourceReaderFromURL, MFAudioFormat_AAC, MFAudioFormat_MP3,
    MFAudioFormat_PCM, MFMediaType_Audio, MFSTARTUP_NOSOCKET,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READER_FLAG,
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_MAJOR_TYPE,
    MF_MT_SUBTYPE, MF_VERSION, IMFAttributes, IMFMediaType, IMFSample, IMFSinkWriter,
    IMFSourceReader,
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

/// 音频格式转换：输出格式由 output_path 扩展名决定（wav/mp3/m4a）
#[tauri::command]
pub async fn convert_audio(input_path: String, output_path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || convert_audio_impl(&input_path, &output_path))
        .await
        .map_err(|e| format!("转换任务异常: {e}"))?
}

fn convert_audio_impl(input: &str, output: &str) -> Result<(), String> {
    let ext = Path::new(output)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .ok_or("无法识别输出格式")?;
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

            // 2. 让源读取器输出 PCM（自动解码 mp3/aac 等压缩格式）
            let pcm_req = MFCreateMediaType().map_err(|e| format!("创建媒体类型失败: {e}"))?;
            pcm_req
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            pcm_req
                .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            reader
                .SetCurrentMediaType(0, None, &pcm_req)
                .map_err(|e| format!("设置音频解码失败: {e}"))?;

            // 3. 读取解码后的 PCM 参数（采样率/声道/位深）
            let pcm_out = reader
                .GetCurrentMediaType(0)
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
                        0,
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

        MFShutdown();
        CoUninitialize();
        result.map_err(|e| format!("音频转换失败: {e}"))
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
