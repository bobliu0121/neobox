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
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};

/// 预览 HTTP 服务端口
static PREVIEW_PORT: AtomicU16 = AtomicU16::new(0);
/// 预览总开关
static PREVIEW_ENABLED: AtomicBool = AtomicBool::new(false);
/// 预览 HTTP 服务是否存活。
///
/// 必须单独跟踪：axum serve 异常退出时端口号不会变，
/// 早先只缓存端口，导致之后每次 start 都直接返回一个**已失效的端口**，
/// 前端请求连接被拒、界面却显示「预览已开启」，且无法自愈（只能重启程序）。
static SERVER_ALIVE: AtomicBool = AtomicBool::new(false);

/// 一路预览源（屏幕 / 摄像头）
struct PreviewStream {
    child: Child,
    /// stdout 已被某个客户端取走时置 None；下次请求会重建进程
    stdout: Option<ChildStdout>,
    /// stderr 末尾若干行（由后台线程持续消费）
    ///
    /// 必须持续读走：ffmpeg 若持续输出 error 日志，管道缓冲（约 64KB）写满后
    /// 会阻塞它写日志 → 采集停止 → 预览画面冻结。
    /// 录制路径有专门的消费线程，预览路径早先漏了这一点。
    ///
    /// 这里保留句柄只为「让后台消费线程活着」——线程持有的是 Arc 副本，
    /// 本字段本身不参与业务逻辑（诊断信息目前通过 early_exit_reason 读取）。
    #[allow(dead_code)]
    stderr_tail: Arc<Mutex<Vec<String>>>,
    /// 该路的 stdout 是否已被某个客户端占用。
    ///
    /// 一路 MJPEG 流只能有一个消费者。用这个标志拒绝第二个并发请求，
    /// 避免「新请求 kill 掉正在供流的进程」导致画面闪断与进程反复重建。
    in_use: bool,
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

/// 预览 start / stop 的串行闸门。
///
/// 两个命令并发穿插时会出现「`preview_running()` 为 true 但采集进程已被杀」
/// 的状态（A 正在 start、B 执行 stop 杀掉进程、A 随后置 enabled=true 并返回端口）。
///
/// 注意必须用 **tokio 的异步 Mutex**：std 的 `MutexGuard` 不是 `Send`，
/// 持着它跨越 `.await` 会让整个命令的 Future 失去 `Send`，Tauri 直接编译不过。
static PREVIEW_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// stop_preview 是同步函数，无法 await 异步闸门；
/// 它改用这把 std 锁，并且**不**跨越 await（只在同步段内持有）。
/// 与 start 的串行仍由 PREVIEW_GATE 的主路径保证。
static STOP_GATE: Mutex<()> = Mutex::new(());

fn set_source_error(slot: &Mutex<String>, msg: &str) {
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = msg.to_string();
}

fn clear_source_error(slot: &Mutex<String>) {
    slot.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

fn read_source_error(slot: &Mutex<String>) -> String {
    // 锁中毒不放弃：错误原因必须能透出，否则用户只能看到空白画面
    slot.lock().unwrap_or_else(|e| e.into_inner()).clone()
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
///
/// # 为什么**不能**给 dshow 摄像头传 `-framerate`
///
/// 实测（用户设备 HP Wide 摄像头）：带上 `-framerate 10` 时 ffmpeg 直接报
///   `[dshow] Could not set video options` → `Error opening input: I/O error`
/// 设备打不开，预览一片空白；而**录制**没有任何视频输出选项，所以同一台设备
/// 录制完全正常 —— 这正是「预览失败但录制正常」的原因。
///
/// dshow 的 `-framerate` 是「强制设备切到该帧率」，很多摄像头只支持特定帧率
/// （常见 30fps / 15fps），强行指定不在支持列表的值就会打开失败。
/// 因此这里不传帧率、也不传 `-video_size`，完全交给设备默认值，
/// 与录制路径保持完全一致。预览的降帧交给后面的 `fps` 滤镜完成。
fn build_camera_stream(ffmpeg_path: &Path, p: &PreviewParams) -> Result<Child, String> {
    let tw = preview_target_width();
    // 预览帧率只作用于**输出**（fps 滤镜），不影响采集：设备按自己的默认帧率出图
    let preview_fps = (p.screen_fps / 3).clamp(2, 12);
    let filter = format!("[0:v]fps={preview_fps},scale={tw}:-2,format=yuv420p[v]");
    let args: Vec<String> = vec![
        "-hide_banner".into(), "-loglevel".into(), "error".into(),
        "-f".into(), "dshow".into(),
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

/// 构造预览进程后，接管它的 stderr 并启动后台消费线程。
///
/// # 为什么必须持续消费 stderr
///
/// ffmpeg 在设备打不开时会把原因写 stderr 后**立即退出**（stdout 为空）；
/// 而正常运行期间它也会持续输出 error 日志（丢帧、设备异常等）。
/// 若不读走，管道缓冲（Windows 约 64KB）写满后会阻塞 ffmpeg 写日志，
/// 采集随之停止 —— 表现为「预览画面突然冻结」。
///
/// 因此把 stderr 交给一个后台线程持续读，只保留末尾若干行用于诊断。
fn attach_stderr_consumer(child: &mut Child) -> Arc<Mutex<Vec<String>>> {
    let tail: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    if let Some(stderr) = child.stderr.take() {
        let t = tail.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let mut guard = t.lock().unwrap_or_else(|e| e.into_inner());
                guard.push(line);
                if guard.len() > 20 {
                    let drain = guard.len() - 20;
                    guard.drain(0..drain);
                }
            }
        });
    }
    tail
}

/// 立即检查子进程是否已异常退出；若已退出则返回 stderr 里的原因。
///
/// 调用前需已 `attach_stderr_consumer`（否则 stderr 已被接管）。
/// 注意：本函数是**同步**的，调用方（async 上下文）必须用
/// `tokio::task::spawn_blocking` 包起来，否则会阻塞 tokio worker。
fn early_exit_reason(
    child: &mut Child,
    tail: &Arc<Mutex<Vec<String>>>,
    label: &str,
) -> Option<String> {
    match child.try_wait() {
        Ok(Some(status)) => {
            let err = tail
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
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

/// 启动后等待一小段时间，判断该源是否「一启动就退出」。
///
/// 返回 `None` 表示仍在正常采集；`Some(原因)` 表示启动失败。
///
/// 用 `tokio::time::sleep` 而不是 `std::thread::sleep`：
/// 本函数在 async 上下文里被 await，同步 sleep 会占住 tokio worker
/// （两路各 600ms 共 1.2 秒，期间其它 IPC 全被排到后面）。
async fn wait_start_ok(
    child: &mut Child,
    tail: &Arc<Mutex<Vec<String>>>,
    label: &str,
) -> Option<String> {
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    early_exit_reason(child, tail, label)
}

fn lock_stream(slot: &'static Mutex<Option<PreviewStream>>) -> std::sync::MutexGuard<'static, Option<PreviewStream>> {
    slot.lock().unwrap_or_else(|e| e.into_inner())
}

fn kill_stream(slot: &'static Mutex<Option<PreviewStream>>) {
    if let Some(mut s) = lock_stream(slot).take() {
        let _ = s.child.kill();
        let _ = s.child.wait();
    }
}

/// 停止全部预览
pub fn stop_preview() {
    // 同步函数无法 await 异步闸门，这里用 std 锁保护「清空槽位」这段临界区。
    // 它只在同步段内持有，不会跨越 await，因此不影响 Future 的 Send。
    let _gate = STOP_GATE.lock().unwrap_or_else(|e| e.into_inner());

    kill_stream(&SCREEN_STREAM);
    kill_stream(&CAMERA_STREAM);
    *SCREEN_PARAMS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *CAMERA_PARAMS.lock().unwrap_or_else(|e| e.into_inner()) = None;
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
    // 串行化启动，防止两个并发 start 各自杀对方刚起的进程。
    // 用 tokio 异步锁：它在 await 点上是 Send 的。
    let _gate = PREVIEW_GATE.lock().await;

    // 全部重启，保证参数是最新的
    kill_stream(&SCREEN_STREAM);
    kill_stream(&CAMERA_STREAM);
    *SCREEN_PARAMS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *CAMERA_PARAMS.lock().unwrap_or_else(|e| e.into_inner()) = None;

    let mut started_any = false;
    let mut last_err = String::new();

    // 屏幕流
    clear_source_error(&SCREEN_ERROR);
    if params.screen_enabled {
        match build_screen_stream(&ffmpeg_path, &params) {
            Ok(mut child) => {
                // 挂 stderr 消费线程，再异步等一小段判断是否秒退
                let tail = attach_stderr_consumer(&mut child);
                match wait_start_ok(&mut child, &tail, "屏幕").await {
                    None => {
                        *lock_stream(&SCREEN_STREAM) = Some(PreviewStream {
                            child,
                            stdout: None,
                            stderr_tail: tail,
                            in_use: false,
                        });
                        *SCREEN_PARAMS.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some((ffmpeg_path.clone(), params.clone()));
                        started_any = true;
                    }
                    Some(reason) => {
                        last_err = reason.clone();
                        set_source_error(&SCREEN_ERROR, &reason);
                    }
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
                let tail = attach_stderr_consumer(&mut child);
                match wait_start_ok(&mut child, &tail, "摄像头").await {
                    None => {
                        *lock_stream(&CAMERA_STREAM) = Some(PreviewStream {
                            child,
                            stdout: None,
                            stderr_tail: tail,
                            in_use: false,
                        });
                        *CAMERA_PARAMS.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some((ffmpeg_path.clone(), params.clone()));
                        started_any = true;
                    }
                    Some(reason) => {
                        last_err = reason.clone();
                        set_source_error(&CAMERA_ERROR, &reason);
                    }
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

    // 只有当 HTTP 服务确实还活着时才复用端口。
    // 早先无条件复用：axum serve 异常退出后端口号不变，之后每次 start 都返回
    // 一个已失效的端口，前端请求连接被拒、界面却显示「预览已开启」，无法自愈。
    let existing_port = PREVIEW_PORT.load(Ordering::SeqCst);
    if existing_port > 0 && SERVER_ALIVE.load(Ordering::SeqCst) {
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
    SERVER_ALIVE.store(true, Ordering::SeqCst);

    tokio::spawn(async move {
        info!("[Recorder] 预览服务监听 127.0.0.1:{port}");
        if let Err(e) = axum::serve(listener, app).await {
            warn!("[Recorder] 预览服务异常退出: {e}");
        }
        // 服务结束（正常或异常）都必须复位存活标志，
        // 否则下次 start 会继续复用这个已经没人监听的端口
        SERVER_ALIVE.store(false, Ordering::SeqCst);
        PREVIEW_PORT.store(0, Ordering::SeqCst);
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
    let mut busy = false;
    {
        let mut g = slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = g.as_mut() {
            if s.in_use {
                // 已有客户端正在消费这路流。
                //
                // 早先这里会 kill 掉正在供流的进程再重建，于是前一个客户端立即 EOF
                // 变空白（预览闪断），重建的进程又可能被下一个请求杀掉，
                // 形成「重建 → 被杀 → 再重建」的循环，CPU 与设备占用反复抖动。
                // 正确做法是让新请求稍后重试，而不是抢走别人的流。
                busy = true;
            } else {
                got = s.stdout.take();
                if got.is_some() {
                    s.in_use = true;
                }
            }
        }
    }
    if busy {
        return (StatusCode::SERVICE_UNAVAILABLE, "preview stream busy, retry")
            .into_response();
    }

    // 没有可用的 stdout（首次请求 / 浏览器重连 / 进程已退出）→ 重建进程
    let stdout = match got {
        Some(s) => s,
        None => {
            // 清掉旧进程（此时已确认没有活跃消费者）
            kill_stream(slot);
            let params = params_slot.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let Some((ffmpeg_path, params)) = params else {
                return (StatusCode::SERVICE_UNAVAILABLE, "preview params missing").into_response();
            };
            match rebuild_fn(&ffmpeg_path, &params) {
                Ok(mut child) => {
                    // 重建也要挂 stderr 消费者，否则长时预览会因管道写满而冻结
                    let tail = attach_stderr_consumer(&mut child);
                    let taken = child.stdout.take();
                    match taken {
                        Some(s) => {
                            {
                                let mut g = slot.lock().unwrap_or_else(|e| e.into_inner());
                                *g = Some(PreviewStream {
                                    child,
                                    stdout: None,
                                    stderr_tail: tail,
                                    in_use: true,
                                });
                            }
                            s
                        }
                        None => {
                            // 拿不到 stdout 就别把半初始化的 child 塞进 slot：
                            // 它没有任何消费者，只会白占设备与 CPU。
                            let mut c = child;
                            let _ = c.kill();
                            let _ = c.wait();
                            return (StatusCode::SERVICE_UNAVAILABLE, "no stdout").into_response();
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

        // 客户端断开（或流读干）后必须收尾，否则这一路会永久停留在 in_use，
        // 之后的请求全部被 503 拒绝，预览从此不再出画面。
        // 这里把该进程收掉并清空 in_use，让下一次请求能重新拉起一路干净的流。
        {
            let mut g = slot.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(s) = g.as_mut() {
                s.in_use = false;
                if let Some(mut child) = g.take().map(|st| st.child) {
                    let _ = child.kill();
                    let _ = child.wait();
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
