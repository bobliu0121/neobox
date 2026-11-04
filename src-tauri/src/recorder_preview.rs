// ============================================================================
// 录制预览服务（按源分路，供前端分层合成）
//
// # 设计：为什么每个源一条独立流
//
// 早期实现让 ffmpeg 把整幅画布合成好再输出。问题是**任何布局变化都必须重启
// ffmpeg 进程**，于是拖动图层时画面要等几百毫秒才更新 —— 用户感受到的是
// 「拖过去了，画面才跳过去」，完全不是实时预览。
//
// 现在改为：屏幕、摄像头各输出一路 MJPEG 原始画面，前端用两个 <img>
// 按图层位置/尺寸/裁剪做 CSS 变换叠在自己的画布上。
// 拖拽时只改 CSS，浏览器合成层直接重绘 —— 真正实时（下一帧即生效），
// 且不需要重启任何进程。录制仍由 ffmpeg 端合成，保证产出视频与预览一致。
//
// 实测依据：gdigrab 与 dshow 都允许同时被多个进程打开（本机已验证），
// 所以「预览各占一路」不会与录制抢设备。
//
// # 端点
//
//   /preview/screen  → 屏幕原始画面
//   /preview/camera  → 摄像头原始画面
//
// 两者各自独立 ffmpeg 进程，互不影响：只开屏幕时不会因摄像头不存在而失败。
// ============================================================================

use log::{info, warn};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};

/// 预览 HTTP 服务端口
static PREVIEW_PORT: AtomicU16 = AtomicU16::new(0);
/// 预览总开关
static PREVIEW_ENABLED: AtomicBool = AtomicBool::new(false);

/// 一路预览源（屏幕 / 摄像头）
struct PreviewStream {
    child: Child,
    /// stdout 已被某个客户端取走时置 None；下次请求会重建进程
    stdout: Option<ChildStdout>,
}

/// 屏幕流与摄像头流各自独立
static SCREEN_STREAM: Mutex<Option<PreviewStream>> = Mutex::new(None);
static CAMERA_STREAM: Mutex<Option<PreviewStream>> = Mutex::new(None);

/// 记下启动参数，供流被读干后重建
static SCREEN_PARAMS: Mutex<Option<(PathBuf, PreviewParams)>> = Mutex::new(None);
static CAMERA_PARAMS: Mutex<Option<(PathBuf, PreviewParams)>> = Mutex::new(None);

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// 预览参数
#[derive(Clone)]
pub struct PreviewParams {
    pub screen_enabled: bool,
    pub screen_fps: u32,
    pub camera_device: String,
}

/// 预览输出宽度上限。每路只输出**源原始画面**（不做合成），
/// 前端按画布比例缩放显示，所以这里按画布宽度等比缩到 720 宽即可。
fn preview_target_width() -> u32 {
    720
}

fn ffmpeg_cmd(ffmpeg_path: &Path) -> Command {
    let mut cmd = Command::new(ffmpeg_path);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    cmd
}

/// 构造屏幕预览进程：输出屏幕原始画面（等比缩到预览宽度）
fn build_screen_stream(ffmpeg_path: &Path, p: &PreviewParams) -> Result<Child, String> {
    let fps = (p.screen_fps / 3).clamp(2, 12);
    let tw = preview_target_width();
    // 保持屏幕宽高比：高度用 -2 自动（保证偶数）
    let filter = format!("[0:v]scale={tw}:-2,format=yuv420p[v]");
    let args: Vec<String> = vec![
        "-hide_banner".into(), "-loglevel".into(), "error".into(),
        "-f".into(), "gdigrab".into(),
        "-framerate".into(), fps.to_string(),
        "-draw_mouse".into(), "1".into(),
        "-i".into(), "desktop".into(),
        "-filter_complex".into(), filter,
        "-map".into(), "[v]".into(),
        "-c:v".into(), "mjpeg".into(),
        "-q:v".into(), "7".into(),
        "-f".into(), "mpjpeg".into(),
        "-boundary_tag".into(), "ffmpeg".into(),
        "pipe:1".into(),
    ];
    let mut cmd = ffmpeg_cmd(ffmpeg_path);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cmd.spawn().map_err(|e| format!("启动屏幕预览失败: {e}"))
}

/// 构造摄像头预览进程：输出摄像头原始画面
fn build_camera_stream(ffmpeg_path: &Path, p: &PreviewParams) -> Result<Child, String> {
    let fps = (p.screen_fps / 3).clamp(2, 12);
    let tw = preview_target_width();
    let filter = format!("[0:v]scale={tw}:-2,format=yuv420p[v]");
    let args: Vec<String> = vec![
        "-hide_banner".into(), "-loglevel".into(), "error".into(),
        "-f".into(), "dshow".into(),
        "-framerate".into(), fps.to_string(),
        "-i".into(), format!("video={}", p.camera_device),
        "-filter_complex".into(), filter,
        "-map".into(), "[v]".into(),
        "-c:v".into(), "mjpeg".into(),
        "-q:v".into(), "7".into(),
        "-f".into(), "mpjpeg".into(),
        "-boundary_tag".into(), "ffmpeg".into(),
        "pipe:1".into(),
    ];
    let mut cmd = ffmpeg_cmd(ffmpeg_path);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cmd.spawn().map_err(|e| format!("启动摄像头预览失败: {e}"))
}

fn kill_stream(slot: &Mutex<Option<PreviewStream>>) {
    if let Ok(mut g) = slot.lock() {
        if let Some(mut s) = g.take() {
            let _ = s.child.kill();
            let _ = s.child.wait();
        }
    }
}

/// 停止全部预览
pub fn stop_preview() {
    kill_stream(&SCREEN_STREAM);
    kill_stream(&CAMERA_STREAM);
    if let Ok(mut g) = SCREEN_PARAMS.lock() { *g = None; }
    if let Ok(mut g) = CAMERA_PARAMS.lock() { *g = None; }
    PREVIEW_ENABLED.store(false, Ordering::SeqCst);
    info!("[Recorder] 预览已全部停止");
}

/// 启动预览 HTTP 服务（并按配置启动各源进程），返回端口
pub async fn start_preview_server(
    ffmpeg_path: PathBuf,
    params: PreviewParams,
) -> Result<u16, String> {
    let existing_port = PREVIEW_PORT.load(Ordering::SeqCst);

    // 全部重启，保证参数是最新的
    stop_preview();

    let mut started_any = false;
    let mut last_err = String::new();

    // 屏幕流
    if params.screen_enabled {
        match build_screen_stream(&ffmpeg_path, &params) {
            Ok(child) => {
                if let Ok(mut g) = SCREEN_STREAM.lock() {
                    *g = Some(PreviewStream { child, stdout: None });
                }
                if let Ok(mut g) = SCREEN_PARAMS.lock() {
                    *g = Some((ffmpeg_path.clone(), params.clone()));
                }
                started_any = true;
            }
            Err(e) => { warn!("[Recorder] 屏幕预览启动失败: {e}"); last_err = e; }
        }
    }

    // 摄像头流
    if !params.camera_device.trim().is_empty() {
        match build_camera_stream(&ffmpeg_path, &params) {
            Ok(child) => {
                if let Ok(mut g) = CAMERA_STREAM.lock() {
                    *g = Some(PreviewStream { child, stdout: None });
                }
                if let Ok(mut g) = CAMERA_PARAMS.lock() {
                    *g = Some((ffmpeg_path.clone(), params.clone()));
                }
                started_any = true;
            }
            Err(e) => { warn!("[Recorder] 摄像头预览启动失败: {e}"); last_err = e; }
        }
    }

    if !started_any {
        let msg = if !params.screen_enabled && params.camera_device.trim().is_empty() {
            "没有可预览的采集源（请启用屏幕或选择摄像头）".to_string()
        } else if last_err.is_empty() {
            "预览启动失败".to_string()
        } else {
            last_err
        };
        return Err(msg);
    }

    PREVIEW_ENABLED.store(true, Ordering::SeqCst);

    if existing_port > 0 {
        info!("[Recorder] 预览进程已重启（复用端口 {existing_port}）");
        return Ok(existing_port);
    }

    let app = axum::Router::new()
        .route("/preview/screen", axum::routing::get(screen_handler))
        .route("/preview/camera", axum::routing::get(camera_handler));

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

pub fn get_preview_port() -> u16 {
    PREVIEW_PORT.load(Ordering::SeqCst)
}

pub fn preview_running() -> bool {
    PREVIEW_ENABLED.load(Ordering::SeqCst)
}

async fn screen_handler() -> axum::response::Response {
    stream_response(StreamKind::Screen).await
}

async fn camera_handler() -> axum::response::Response {
    stream_response(StreamKind::Camera).await
}

#[derive(Clone, Copy, PartialEq)]
enum StreamKind {
    Screen,
    Camera,
}

/// 取（必要时重建）某一路的 stdout，并包装成 MJPEG 响应
async fn stream_response(kind: StreamKind) -> axum::response::Response {
    use axum::body::Body;
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;

    if !PREVIEW_ENABLED.load(Ordering::SeqCst) {
        return (StatusCode::SERVICE_UNAVAILABLE, "preview not enabled").into_response();
    }

    let (slot, params_slot, rebuild_fn): (
        &'static Mutex<Option<PreviewStream>>,
        &'static Mutex<Option<(PathBuf, PreviewParams)>>,
        fn(&Path, &PreviewParams) -> Result<Child, String>,
    ) = match kind {
        StreamKind::Screen => (&SCREEN_STREAM, &SCREEN_PARAMS, build_screen_stream),
        StreamKind::Camera => (&CAMERA_STREAM, &CAMERA_PARAMS, build_camera_stream),
    };

    // 先尝试直接取已有进程的 stdout
    let mut got: Option<ChildStdout> = None;
    if let Ok(mut g) = slot.lock() {
        if let Some(s) = g.as_mut() {
            got = s.stdout.take();
        }
    }

    // 没有可用的 stdout（首次请求 / 浏览器重连 / 进程已退出）→ 重建进程
    let stdout = match got {
        Some(s) => s,
        None => {
            // 清掉旧进程
            kill_stream(slot);
            let params = match params_slot.lock() {
                Ok(g) => g.clone(),
                Err(e) => e.into_inner().clone(),
            };
            let Some((ffmpeg_path, params)) = params else {
                return (StatusCode::SERVICE_UNAVAILABLE, "preview params missing").into_response();
            };
            match rebuild_fn(&ffmpeg_path, &params) {
                Ok(mut child) => {
                    let taken = child.stdout.take();
                    if let Ok(mut g) = slot.lock() {
                        *g = Some(PreviewStream { child, stdout: None });
                    }
                    match taken {
                        Some(s) => s,
                        None => {
                            return (StatusCode::SERVICE_UNAVAILABLE, "no stdout")
                                .into_response();
                        }
                    }
                }
                Err(e) => {
                    warn!("[Recorder] 预览重建失败: {e}");
                    return (StatusCode::SERVICE_UNAVAILABLE, format!("rebuild failed: {e}"))
                        .into_response();
                }
            }
        }
    };

    // 阻塞读放到 spawn_blocking，避免长期占住 tokio worker
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(8);
    let mut stdout_owned = stdout;
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 8192];
        loop {
            match stdout_owned.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.blocking_send(Ok(Vec::from(&buf[..n]))).is_err() {
                        break; // 客户端断开
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    let _ = tx.blocking_send(Err(e));
                    break;
                }
            }
        }
    });

    let body = Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx));
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
    headers.insert(header::CONNECTION, header::HeaderValue::from_static("close"));
    resp
}
