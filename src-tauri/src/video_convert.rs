//! 视频转换：视频 → 动图 GIF / 静态图片 / 音频。
//!
//! 解码使用 Windows Media Foundation（系统自带 H.264/H.265/VP9 等解码器），
//! GIF 编码使用 `image` crate，不依赖外部 ffmpeg。
//!
//! 帧提取策略：通过 Source Reader 的「视频处理」能力把任意视频统一转成
//! RGB32，再按目标帧率抽帧。时间戳来自样本本身（100ns 单位），因此
//! 可变帧率视频也能保持正确的时间节奏。

use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFSample, IMFSourceReader, MFCreateAttributes,
    MFCreateMediaType, MFCreateSourceReaderFromURL, MFMediaType_Video, MFShutdown, MFStartup,
    MFVideoFormat_RGB32, MFVideoInterlace_Progressive, MFSTARTUP_NOSOCKET,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_SUBTYPE, MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

use image::{ImageFormat, RgbaImage};

#[derive(Debug, Clone, serde::Serialize)]
pub struct VideoConvertResult {
    pub output_path: String,
    pub frames: u64,
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

struct MfSession;

impl MfSession {
    fn start() -> Result<Self, String> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|e| format!("COM 初始化失败: {e:?}"))?;
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(|e| format!("MF 初始化失败: {e}"))?;
        }
        Ok(Self)
    }
}

impl Drop for MfSession {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}

struct VideoReader {
    reader: IMFSourceReader,
    /// 解码输出的原始尺寸
    width: u32,
    height: u32,
    /// 最终写入 GIF / 图片的目标尺寸（按请求宽度等比缩放）
    out_width: u32,
    out_height: u32,
    stride: i32,
    /// stride 是否来自 MF_MT_DEFAULT_STRIDE（可靠）。false 表示是 fallback 估算，
    /// 需要在每帧用 buffer 实际长度反推真实行步幅，否则会因行对齐 padding 导致画面斜条纹。
    stride_exact: bool,
}

impl VideoReader {
    fn open(path: &str, target_width: u32) -> Result<Self, String> {
        unsafe {
            let mut attributes: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attributes, 2).map_err(|e| format!("创建 MF 属性失败: {e}"))?;
            let attributes = attributes.ok_or_else(|| "创建 MF 属性失败".to_string())?;
            attributes
                .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
                .map_err(|e| format!("启用视频处理失败: {e}"))?;

            let path_w = wide(path);
            let reader = MFCreateSourceReaderFromURL(PCWSTR(path_w.as_ptr()), &attributes)
                .map_err(|e| format!("打开视频失败（系统缺少对应解码器？）: {e}"))?;

            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(|e| format!("关闭其它轨道失败: {e}"))?;
            reader
                .SetStreamSelection(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, true)
                .map_err(|e| format!("选择视频轨道失败: {e}"))?;

            // 先读取原始尺寸
            let native = reader
                .GetNativeMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, 0)
                .map_err(|e| format!("读取视频信息失败: {e}"))?;
            let packed = native
                .GetUINT64(&MF_MT_FRAME_SIZE)
                .map_err(|e| format!("读取视频尺寸失败: {e}"))?;
            let native_width = (packed >> 32) as u32;
            let native_height = (packed & 0xFFFF_FFFF) as u32;
            if native_width == 0 || native_height == 0 {
                return Err("无法读取视频分辨率".to_string());
            }

            // 关键：总是请求 native 尺寸的 RGB32，不让 MF 做缩放。
            // 原因：MF 在接受缩放请求后，内部 surface 可能按更大边界对齐，
            // stride > width*4 且 MF_MT_DEFAULT_STRIDE 可能不提供或不可靠，
            // 导致行步幅错位产生水平条纹。native 尺寸下 Source Reader 软件解码
            // 输出紧凑帧，stride == native_width*4，稳定可预测。
            // 缩放交给后续 image::imageops::resize 完成。
            let output_type = MFCreateMediaType().map_err(|e| format!("创建媒体类型失败: {e}"))?;
            output_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|e| format!("设置媒体类型失败: {e}"))?;
            output_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                .map_err(|e| format!("设置 RGB32 输出失败: {e}"))?;
            output_type
                .SetUINT64(&MF_MT_FRAME_SIZE, ((native_width as u64) << 32) | native_height as u64)
                .map_err(|e| format!("设置输出尺寸失败: {e}"))?;
            output_type
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(|e| format!("设置逐行扫描失败: {e}"))?;
            reader
                .SetCurrentMediaType(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    None,
                    &output_type,
                )
                .map_err(|e| format!("初始化视频解码失败: {e}"))?;

            let actual = reader
                .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
                .map_err(|e| format!("读取输出格式失败: {e}"))?;
            let packed = actual
                .GetUINT64(&MF_MT_FRAME_SIZE)
                .map_err(|e| format!("读取输出尺寸失败: {e}"))?;
            let width = (packed >> 32) as u32;
            let height = (packed & 0xFFFF_FFFF) as u32;
            let raw_stride = actual.GetUINT32(&MF_MT_DEFAULT_STRIDE);
            let (stride, stride_exact) = match raw_stride {
                Ok(s) if s != 0 => (s as i32, true),
                _ => ((width * 4) as i32, false),
            };

            // 按请求宽度计算最终输出尺寸（后续 resize 使用）
            let target_width = target_width.clamp(64, 1920);
            let out_width = target_width.min(width).max(2) & !1;
            let out_height = (((height as f64 * out_width as f64 / width as f64).round() as u32).max(2)) & !1;

            Ok(Self {
                reader,
                width,
                height,
                out_width,
                out_height,
                stride,
                stride_exact,
            })
        }
    }

    /// 读取下一帧（RGBA）。返回 (时间戳 ms, 图像)；流结束时返回 None。
    fn next_frame(&mut self) -> Result<Option<(u64, RgbaImage)>, String> {
        loop {
            unsafe {
                let mut flags: u32 = 0;
                let mut timestamp: i64 = 0;
                let mut sample: Option<IMFSample> = None;
                self.reader
                    .ReadSample(
                        MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                        0,
                        None,
                        Some(&mut flags),
                        Some(&mut timestamp),
                        Some(&mut sample),
                    )
                    .map_err(|e| format!("读取视频帧失败: {e}"))?;
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    return Ok(None);
                }
                let Some(sample) = sample else { continue };
                // 空样本（格式变更）跳过
                let buffer = match sample.ConvertToContiguousBuffer() {
                    Ok(buffer) => buffer,
                    Err(_) => continue,
                };
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut max_len: u32 = 0;
                let mut current_len: u32 = 0;
                buffer
                    .Lock(&mut data, Some(&mut max_len), Some(&mut current_len))
                    .map_err(|e| format!("锁定视频帧失败: {e}"))?;
                if data.is_null() || current_len == 0 {
                    let _ = buffer.Unlock();
                    continue;
                }

                let width = self.width as usize;
                let height = self.height as usize;
                let row_bytes = width * 4;
                // stride 策略：
                // - MF 提供了 MF_MT_DEFAULT_STRIDE（stride_exact=true）：直接用它。
                // - 未提供：RGB32 格式行宽自然 4 字节对齐，Source Reader 软件解码
                //   输出紧凑帧，stride == width*4。
                // 注意：绝不能用 current_len/height 反推——MF sample buffer 末尾常带
                // 2KB 左右的对齐 padding，导致 current_len > stride*height，反推值偏大，
                // 行间距错位产生横向/斜向条纹。
                let abs_stride = self.stride.unsigned_abs() as usize;
                if abs_stride < row_bytes || (current_len as usize) < abs_stride * height {
                    let _ = buffer.Unlock();
                    return Err("视频帧数据长度异常".to_string());
                }
                let slice = std::slice::from_raw_parts(data, current_len as usize);
                let mut rgba = vec![0u8; row_bytes * height];
                for row in 0..height {
                    // MF 的 RGB32 内存布局是 BGRA。Source Reader 输出通常为
                    // 自上而下（正 stride）；负 stride 才需要从最后一行倒序读取。
                    let src_row = if self.stride < 0 { height - 1 - row } else { row };
                    let src = &slice[src_row * abs_stride..src_row * abs_stride + row_bytes];
                    let dst = &mut rgba[row * row_bytes..(row + 1) * row_bytes];
                    for x in 0..width {
                        let b = src[x * 4];
                        let g = src[x * 4 + 1];
                        let r = src[x * 4 + 2];
                        let a = src[x * 4 + 3];
                        dst[x * 4] = r;
                        dst[x * 4 + 1] = g;
                        dst[x * 4 + 2] = b;
                        dst[x * 4 + 3] = if a == 0 { 255 } else { a };
                    }
                }
                let _ = buffer.Unlock();
                let image = RgbaImage::from_raw(width as u32, height as u32, rgba)
                    .ok_or_else(|| "构建视频帧失败".to_string())?;
                let ts_ms = (timestamp.max(0) as f64 / 10_000.0).round() as u64;
                return Ok(Some((ts_ms, image)));
            }
        }
    }
}

fn resize_if_needed(image: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    if image.width() == width && image.height() == height {
        return image.clone();
    }
    image::imageops::resize(image, width, height, image::imageops::FilterType::Triangle)
}

fn save_still(image: &RgbaImage, output: &str, format: &str) -> Result<(), String> {
    let path = Path::new(output);
    match format {
        "png" => image
            .save_with_format(path, ImageFormat::Png)
            .map_err(|e| format!("写入 PNG 失败: {e}")),
        "webp" => image
            .save_with_format(path, ImageFormat::WebP)
            .map_err(|e| format!("写入 WebP 失败: {e}")),
        "bmp" => image
            .save_with_format(path, ImageFormat::Bmp)
            .map_err(|e| format!("写入 BMP 失败: {e}")),
        _ => {
            let file = std::fs::File::create(path).map_err(|e| format!("创建输出文件失败: {e}"))?;
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 92);
            image
                .write_with_encoder(encoder)
                .map_err(|e| format!("写入 JPEG 失败: {e}"))
        }
    }
}

/// 视频转换入口。
///
/// - `format`: `gif` / `png` / `jpg` / `webp` / `bmp` / `mp3` / `m4a` / `wav`
/// - `fps`: 动图抽帧帧率（1–30）
/// - `width`: 输出宽度（高度等比）
/// - `start_seconds` / `duration_seconds`: 截取区间；`duration_seconds <= 0` 表示到结尾
pub fn convert_video(
    input: &str,
    output: &str,
    format: &str,
    fps: u32,
    width: u32,
    start_seconds: f64,
    duration_seconds: f64,
) -> Result<VideoConvertResult, String> {
    let format = format.to_ascii_lowercase();
    if format == "mp3" || format == "m4a" || format == "aac" || format == "wav" {
        crate::file_convert::convert_audio_impl(input, output)?;
        let duration_ms = 0;
        return Ok(VideoConvertResult {
            output_path: output.to_string(),
            frames: 0,
            width: 0,
            height: 0,
            duration_ms,
        });
    }

    let _session = MfSession::start()?;
    let mut reader = VideoReader::open(input, width)?;
    let start_ms = (start_seconds.max(0.0) * 1000.0) as u64;
    let duration_ms = (duration_seconds.max(0.0) * 1000.0) as u64;

    if format != "gif" {
        // 静态图：取区间内第一帧
        loop {
            match reader.next_frame()? {
                Some((ts, image)) => {
                    if ts < start_ms {
                        continue;
                    }
                    if duration_ms > 0 && ts > start_ms + duration_ms {
                        return Err("截取区间内没有视频帧".to_string());
                    }
                    let resized = resize_if_needed(&image, reader.out_width, reader.out_height);
                    save_still(&resized, output, &format)?;
                    return Ok(VideoConvertResult {
                        output_path: output.to_string(),
                        frames: 1,
                        width: resized.width(),
                        height: resized.height(),
                        duration_ms: 0,
                    });
                }
                None => return Err("视频中没有可用的画面".to_string()),
            }
        }
    }

    let fps = fps.clamp(1, 30);
    let frame_interval = 1000.0 / fps as f64;
    let mut next_capture = start_ms as f64;
    let mut frames_written = 0u64;
    let max_frames: u64 = 1800;

    let file = std::fs::File::create(output).map_err(|e| format!("创建 GIF 失败: {e}"))?;
    let writer = std::io::BufWriter::new(file);
    let mut encoder = image::codecs::gif::GifEncoder::new_with_speed(writer, 10);
    encoder
        .set_repeat(image::codecs::gif::Repeat::Infinite)
        .map_err(|e| format!("设置 GIF 循环失败: {e}"))?;
    let delay = image::Delay::from_numer_denom_ms(1000, fps);

    let mut last_size = (reader.out_width, reader.out_height);
    loop {
        let Some((ts, image)) = reader.next_frame()? else {
            break;
        };
        if ts < start_ms {
            continue;
        }
        if duration_ms > 0 && ts > start_ms + duration_ms {
            break;
        }
        if (ts as f64) + 0.5 < next_capture {
            continue;
        }
        // 按目标帧率控制抽帧节奏（视频帧率低于目标时按实际时间补齐）
        while (ts as f64) + 0.5 >= next_capture {
            next_capture += frame_interval;
        }
        let resized = resize_if_needed(&image, reader.out_width, reader.out_height);
        last_size = (resized.width(), resized.height());
        let frame = image::Frame::from_parts(resized, 0, 0, delay);
        encoder
            .encode_frame(frame)
            .map_err(|e| format!("编码 GIF 帧失败: {e}"))?;
        frames_written += 1;
        if frames_written >= max_frames {
            log::warn!("[VideoConvert] 达到最大帧数 {max_frames}，提前结束");
            break;
        }
    }

    if frames_written == 0 {
        return Err("截取区间内没有视频帧".to_string());
    }

    Ok(VideoConvertResult {
        output_path: output.to_string(),
        frames: frames_written,
        width: last_size.0,
        height: last_size.1,
        duration_ms: (frames_written as f64 / fps as f64 * 1000.0) as u64,
    })
}


