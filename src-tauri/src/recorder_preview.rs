// ============================================================================
// 录制预览服务
//
// 提供 MJPEG 实时预览流给前端 <img> 显示。
//
// # 为什么单独起一个 ffmpeg 进程
//
// 预览和录制是两个独立需求：预览要低清、低帧率、可随时开关；
// 录制要高码率、完整帧。实测同一 dshow/gdigrab 设备可被两个 ffmpeg
// 进程同时打开（本机已验证），所以预览不需要复用录制进程的帧，
// 直接另起一路采集最省事，也不会拖累录制质量。
//
// # 为什么输出到管道而不是文件
//
// 预览要求「实时」，写文件再读会有延迟且要处理轮转。
// 这里让 ffmpeg 输出 mpjpeg（MIME multipart JPEG）到 stdout，
// 再原样流式转发给浏览器 —— <img src> 原生就能渲染 multipart/x-mixed-replace。
//
// # 资源占用控制
//
// 预览默认限制 12fps、宽度 640，并强制低质量量化（q=8），
// 单路开销远小于录制；用户可在界面关闭预览。
// ============================================================================

use log::{info, warn};
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};

/// 预览端口
static PREVIEW_PORT: AtomicU16 = AtomicU16::new(0);
/// 预览是否启用
static PREVIEW_ENABLED: AtomicBool = AtomicBool::new(false);
/// 当前预览的 ffmpeg 进程
static PREVIEW_CHILD: Mutex<Option<Child>> = Mutex::new(None);
/// 最近一次启动预览用的参数，供 /preview 被重新请求时重建进程
static PREVIEW_PARAMS: Mutex<Option<(std::path::PathBuf, PreviewParams)>> = Mutex::new(None);

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// 预览参数：低清、低帧率，明显区别于录制
#[derive(Clone)]
pub struct PreviewParams {
    pub screen_enabled: bool,
    pub screen_fps: u32,
    pub camera_device: String,
    pub canvas_w: u32,
    pub canvas_h: u32,
    /// 是否包含摄像头图层
    pub camera_visible: bool,
}

/// 构造预览用的 ffmpeg 子进程（输出 mpjpeg 到 stdout）
fn build_preview_child(ffmpeg_path: &Path, p: &PreviewParams) -> Result<Child, String> {
    if !p.screen_enabled && p.camera_device.trim().is_empty() {
        return Err("没有可预览的采集源".to_string());
    }

    // 预览固定最大宽度 640，兼顾清晰度与开销
    let target_w = 640u32;
    let ratio = if p.canvas_w > 0 {
        target_w as f64 / p.canvas_w as f64
    } else {
        1.0
    };
    let target_h = ((p.canvas_h as f64 * ratio) as u32).max(2);
    let target_w = target_w - target_w % 2;
    let target_h = target_h - target_h % 2;

    // 预览帧率：用户设置的 1/3，上限 12
    let fps = (p.screen_fps / 3).clamp(2, 12);

    let mut args: Vec<String> = vec!["-hide_banner".into(), "-loglevel".into(), "error".into()];

    // 输入：屏幕
    let mut idx = 0usize;
    let mut screen_idx = None;
    if p.screen_enabled {
        args.extend([
            "-f".into(), "gdigrab".into(),
            "-framerate".into(), fps.to_string(),
            "-draw_mouse".into(), "1".into(),
            "-i".into(), "desktop".into(),
        ]);
        screen_idx = Some(idx);
        idx += 1;
    }

    // 输入：摄像头
    let mut cam_idx = None;
    if !p.camera_device.trim().is_empty() {
        args.extend([
            "-f".into(), "dshow".into(),
            "-i".into(), format!("video={}", p.camera_device),
        ]);
        cam_idx = Some(idx);
        let _ = idx;
    }

    // 画面合成：缩放画布尺寸，若两源都在则画中画预览
    let filter = match (screen_idx, cam_idx, p.camera_visible) {
        (Some(_s), Some(c), true) => {
            // 预览也做画中画，让用户在预览里就能看到布局关系
            format!(
                "[0:v]scale={target_w}:{target_h}[bg];\
                 [{c}:v]scale=-2:{pw}[cam];\
                 [bg][cam]overlay=x=W-w-16:y=H-h-16[v]",
                pw = (target_h / 3).max(2)
            )
        }
        (Some(_s), _, _) => format!("[0:v]scale={target_w}:{target_h}[v]"),
        (None, Some(c), _) => format!("[{c}:v]scale={target_w}:-2[v]"),
        (None, None, _) => return Err("没有可预览的采集源".to_string()),
    };

    args.extend([
        "-filter_complex".into(), filter,
        "-map".into(), "[v]".into(),
        "-c:v".into(), "mjpeg".into(),
        // 预览追求速度，量化值越大越小越快
        "-q:v".into(), "8".into(),
        "-f".into(), "mpjpeg".into(),
        "-boundary_tag".into(), "ffmpeg".into(),
        "pipe:1".into(),
    ]);

    let mut cmd = Command::new(ffmpeg_path);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

    cmd.spawn().map_err(|e| format!("启动预览 ffmpeg 失败: {e}"))
}

/// 停止预览进程
pub fn stop_preview() {
    let child = {
        match PREVIEW_CHILD.lock() {
            Ok(mut g) => g.take(),
            Err(e) => e.into_inner().take(),
        }
    };
    if let Some(mut c) = child {
        let _ = c.kill();
        let _ = c.wait();
        info!("[Recorder] 预览已停止");
    }
    // 清掉参数，避免下次误用旧配置
    if let Ok(mut g) = PREVIEW_PARAMS.lock() {
        *g = None;
    }
    PREVIEW_ENABLED.store(false, Ordering::SeqCst);
}

/// 启动预览的 HTTP 服务并返回端口。
///
/// 该服务只提供 `/preview` 一个端点，返回 multipart MJPEG 流。
/// 若已在运行则直接返回现有端口。
pub async fn start_preview_server(
    ffmpeg_path: std::path::PathBuf,
    params: PreviewParams,
) -> Result<u16, String> {
    // 已有服务：先停掉旧预览进程（参数可能变了），但保留端口
    let existing_port = PREVIEW_PORT.load(Ordering::SeqCst);

    // 先确保旧进程退出，否则设备被占用可能导致新进程起不来
    stop_preview();

    // 启动新的预览 ffmpeg
    let child = build_preview_child(&ffmpeg_path, &params)?;
    {
        let mut guard = PREVIEW_CHILD
            .lock()
            .map_err(|e| format!("预览进程锁失败: {e}"))?;
        *guard = Some(child);
    }
    // 记下参数：流被读干后（浏览器刷新/重连）需要据此重建进程
    if let Ok(mut g) = PREVIEW_PARAMS.lock() {
        *g = Some((ffmpeg_path.clone(), params.clone()));
    }
    PREVIEW_ENABLED.store(true, Ordering::SeqCst);

    if existing_port > 0 {
        info!("[Recorder] 预览进程已重启（复用端口 {existing_port}）");
        return Ok(existing_port);
    }

    // 首次：起一个新服务
    let app = axum::Router::new().route("/preview", axum::routing::get(preview_handler));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("预览服务绑定端口失败: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("获取预览端口失败: {e}"))?
        .port();
    PREVIEW_PORT.store(port, Ordering::SeqCst);

    tokio::spawn(async move {
        info!("[Recorder] 预览服务监听 127.0.0.1:{port}");
        if let Err(e) = axum::serve(listener, app).await {
            warn!("[Recorder] 预览服务异常退出: {e}");
        }
    });

    Ok(port)
}

/// 获取预览端口（0 表示未启动）
pub fn get_preview_port() -> u16 {
    PREVIEW_PORT.load(Ordering::SeqCst)
}

/// 当前是否启用预览
pub fn preview_running() -> bool {
    PREVIEW_ENABLED.load(Ordering::SeqCst)
}

/// MJPEG 流端点。
///
/// 把 ffmpeg stdout 的字节流原样转发给浏览器。
/// Content-Type 用 `multipart/x-mixed-replace; boundary=ffmpeg`，
/// 与 ffmpeg 的 `-boundary_tag ffmpeg` 对应；<img> 会自动连续渲染每一帧。
async fn preview_handler() -> axum::response::Response {
    use axum::body::Body;
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;

    if !PREVIEW_ENABLED.load(Ordering::SeqCst) {
        return (StatusCode::SERVICE_UNAVAILABLE, "preview not enabled").into_response();
    }

    // 取当前预览进程的 stdout。
    //
    // ffmpeg 的 stdout 只能 take 一次；浏览器刷新 <img>、或前端重新挂载时
    // 会再次请求本端点。此时旧进程的流已经被读干（甚至已退出），
    // 直接报错会让预览"刷新一下就黑了"，体验很差。
    // 因此这里检测到"没有可用 stdout"时，用记下的参数重建一路预览进程，
    // 保证每次请求都能拿到新鲜的实时流。
    let mut need_rebuild = false;
    let mut reused: Option<std::process::ChildStdout> = None;
    {
        let mut guard = match PREVIEW_CHILD.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        match guard.as_mut() {
            Some(child) => {
                reused = child.stdout.take();
                if reused.is_none() {
                    need_rebuild = true;
                }
            }
            None => {
                need_rebuild = true;
            }
        }
    }

    // 需要重建：杀掉旧进程，用保存的参数起一路新的
    let stdout: std::process::ChildStdout = if need_rebuild {
        // 先释放旧进程（它的 stdout 已被前一个客户端读完）
        {
            if let Ok(mut g) = PREVIEW_CHILD.lock() {
                if let Some(mut old) = g.take() {
                    let _ = old.kill();
                    let _ = old.wait();
                }
            }
        }
        let params = {
            match PREVIEW_PARAMS.lock() {
                Ok(g) => g.clone(),
                Err(e) => e.into_inner().clone(),
            }
        };
        let Some((ffmpeg_path, params)) = params else {
            return (StatusCode::SERVICE_UNAVAILABLE, "no preview params").into_response();
        };
        match build_preview_child(&ffmpeg_path, &params) {
            Ok(mut child) => {
                let taken = child.stdout.take();
                if let Ok(mut g) = PREVIEW_CHILD.lock() {
                    *g = Some(child);
                }
                match taken {
                    Some(s) => s,
                    None => {
                        return (StatusCode::SERVICE_UNAVAILABLE, "rebuild produced no stdout")
                            .into_response();
                    }
                }
            }
            Err(e) => {
                warn!("[Recorder] 预览重建失败: {e}");
                return (StatusCode::SERVICE_UNAVAILABLE, format!("preview rebuild failed: {e}"))
                    .into_response();
            }
        }
    } else {
        match reused {
            Some(s) => s,
            None => {
                return (StatusCode::SERVICE_UNAVAILABLE, "no preview stream").into_response();
            }
        }
    };

    // 用阻塞读 + 异步包装简单直接：ffmpeg 输出是持续的字节流
    let body = Body::from_stream(futures_util::stream::unfold(stdout, |mut out| async move {
        let mut buf = [0u8; 8192];
        loop {
            match out.read(&mut buf) {
                Ok(0) => return None,
                Ok(n) => return Some((Ok::<_, std::io::Error>(Vec::from(&buf[..n])), out)),
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Some((Err(e), out)),
            }
        }
    }));

    let mut resp = axum::response::Response::new(body);
    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("multipart/x-mixed-replace; boundary=ffmpeg"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache, no-store, must-revalidate"),
    );
    headers.insert(
        header::CONNECTION,
        header::HeaderValue::from_static("close"),
    );
    resp
}
