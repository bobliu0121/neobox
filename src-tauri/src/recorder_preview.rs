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

/// 每一路预览各自的失败原因（空 = 正常或未启用）。
///
/// 必须按源记录：屏幕成功、摄像头失败时整体函数仍算「启动成功」，
/// 若只记一个全局错误，摄像头那一路的原因就会被静默丢掉，
/// 用户看到摄像头区域一片空白却没有任何提示。
static SCREEN_ERROR: Mutex<String> = Mutex::new(String::new());
static CAMERA_ERROR: Mutex<String> = Mutex::new(String::new());

fn set_source_error(slot: &Mutex<String>, msg: &str) {
    if let Ok(mut g) = slot.lock() {
        *g = msg.to_string();
    }
}

fn clear_source_error(slot: &Mutex<String>) {
    if let Ok(mut g) = slot.lock() {
        g.clear();
    }
}

fn read_source_error(slot: &Mutex<String>) -> String {
    slot.lock().map(|g| g.clone()).unwrap_or_default()
}

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
        .stderr(Stdio::piped());
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
        .stderr(Stdio::piped());
    cmd.spawn().map_err(|e| format!("启动摄像头预览失败: {e}"))
}

/// 读取子进程 stderr 的前若干行（用于启动失败时给出可读原因）。
///
/// ffmpeg 在设备打不开时会把原因写 stderr 后**立即退出**（stdout 为空），
/// 若把 stderr 丢进 Stdio::null()，前端就只能看到一片空白，
/// 完全不知道为什么没有画面 —— 这正是「摄像头无法预览」难以定位的原因。
fn drain_stderr(child: &mut Child) -> String {
    use std::io::Read;
    if let Some(mut err) = child.stderr.take() {
        let mut buf = String::new();
        let _ = err.read_to_string(&mut buf);
        return buf.trim().to_string();
    }
    String::new()
}

/// 启动某一源后立即检查它是否已经异常退出；若已退出则返回 stderr 里的原因。
///
/// dshow 设备不存在 / 被其它程序独占时，ffmpeg 几毫秒内就会失败退出。
/// 这里等一小段时间再查，能区分「真在采集」与「一启动就挂了」。
fn check_early_exit(child: &mut Child, label: &str) -> Option<String> {
    std::thread::sleep(std::time::Duration::from_millis(600));
    match child.try_wait() {
        Ok(Some(status)) => {
            let err = drain_stderr(child);
            let reason = if err.is_empty() {
                format!("{label}采集进程已退出（退出码 {:?}）", status.code())
            } else {
                format!("{label}采集失败：{err}")
            };
            warn!("[Recorder] {reason}");
            Some(reason)
        }
        _ => None,
    }
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
    clear_source_error(&SCREEN_ERROR);
    clear_source_error(&CAMERA_ERROR);
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
    clear_source_error(&SCREEN_ERROR);
    if params.screen_enabled {
        match build_screen_stream(&ffmpeg_path, &params) {
            Ok(mut child) => {
                // 启动后立刻查一次是否已秒退（例如 gdigrab 被占用）
                if let Some(reason) = check_early_exit(&mut child, "屏幕") {
                    last_err = reason.clone();
                    set_source_error(&SCREEN_ERROR, &reason);
                } else {
                    if let Ok(mut g) = SCREEN_STREAM.lock() {
                        *g = Some(PreviewStream { child, stdout: None });
                    }
                    if let Ok(mut g) = SCREEN_PARAMS.lock() {
                        *g = Some((ffmpeg_path.clone(), params.clone()));
                    }
                    started_any = true;
                }
            }
            Err(e) => {
                warn!("[Recorder] 屏幕预览启动失败: {e}");
                last_err = e.clone();
                set_source_error(&SCREEN_ERROR, &e);
            }
        }
    }

    // 摄像头流
    clear_source_error(&CAMERA_ERROR);
    if !params.camera_device.trim().is_empty() {
        match build_camera_stream(&ffmpeg_path, &params) {
            Ok(mut child) => {
                if let Some(reason) = check_early_exit(&mut child, "摄像头") {
                    last_err = reason.clone();
                    set_source_error(&CAMERA_ERROR, &reason);
                } else {
                    if let Ok(mut g) = CAMERA_STREAM.lock() {
                        *g = Some(PreviewStream { child, stdout: None });
                    }
                    if let Ok(mut g) = CAMERA_PARAMS.lock() {
                        *g = Some((ffmpeg_path.clone(), params.clone()));
                    }
                    started_any = true;
                }
            }
            Err(e) => {
                warn!("[Recorder] 摄像头预览启动失败: {e}");
                last_err = e.clone();
                set_source_error(&CAMERA_ERROR, &e);
            }
        }
    }

    // 一路都没起来：返回**具体原因**，让界面能显示为什么没画面
    // （设备不存在、被独占、ffmpeg 缺 dshow 等都能据此定位）
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

/// 各源预览的失败原因（供前端按源显示，避免"整体成功但某一路失败"被静默）。
/// 返回 (screen_error, camera_error)，空串表示该源正常或未启用。
pub fn preview_source_errors() -> (String, String) {
    (
        read_source_error(&SCREEN_ERROR),
        read_source_error(&CAMERA_ERROR),
    )
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
