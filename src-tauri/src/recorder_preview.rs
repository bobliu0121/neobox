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
///
/// 关键：预览必须使用与录制**完全相同**的图层布局（位置/尺寸/裁剪），
/// 否则用户拖拽或裁剪图层后，预览画面与最终录出的视频不一致 ——
/// 那会让用户以为布局设置没生效，比没有预览更糟。
#[derive(Clone)]
pub struct PreviewParams {
    pub screen_enabled: bool,
    pub screen_fps: u32,
    pub camera_device: String,
    pub canvas_w: u32,
    pub canvas_h: u32,
    /// 屏幕图层布局
    pub screen_layout: crate::recorder::LayerLayout,
    /// 摄像头图层布局
    pub camera_layout: crate::recorder::LayerLayout,
}

/// 构造预览用的 ffmpeg 子进程（输出 mpjpeg 到 stdout）
fn build_preview_child(ffmpeg_path: &Path, p: &PreviewParams) -> Result<Child, String> {
    if !p.screen_enabled && p.camera_device.trim().is_empty() {
        return Err("没有可预览的采集源".to_string());
    }

    // 预览固定最大宽度 640，兼顾清晰度与开销
    let target_w = 640u32;
    let target_h = {
        let r = if p.canvas_w > 0 {
            target_w as f64 / p.canvas_w as f64
        } else {
            1.0
        };
        ((p.canvas_h as f64 * r) as u32).max(2)
    };
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

    // 画面合成：**完全复刻录制的画布式布局**，只是整体缩小到预览尺寸。
    //
    // 做法：先在「画布像素坐标系」里按图层 x/y/w/h/crop 生成 overlay 链，
    // 最后统一 scale 到预览分辨率。这样布局比例与录制结果一致，
    // 用户拖到哪里、裁哪里，预览就显示到哪里。
    //
    // 之所以不能沿用旧的「固定右下角画中画」：那完全不参考 layer 的
    // 位置与裁剪参数，移动/裁剪图层后预览不动，会误导用户。

    let canvas_w = p.canvas_w.max(2);
    let canvas_h = p.canvas_h.max(2);

    let mut filters: Vec<String> = Vec::new();
    // 空白画布
    filters.push(format!(
        "color=c=black:s={canvas_w}x{canvas_h}:r={fps}[canvas0]"
    ));
    let mut composed = String::from("[canvas0]");
    let mut layer_no = 0usize;

    // 屏幕图层
    if let Some(s) = screen_idx {
        let lay = &p.screen_layout;
        if lay.visible {
            let mut chain = String::new();
            if lay.crop_w > 0 && lay.crop_h > 0 {
                chain.push_str(&format!(
                    "crop={}:{}:{}:{},",
                    lay.crop_w, lay.crop_h, lay.crop_x, lay.crop_y
                ));
            }
            let dw = if lay.w > 0 { lay.w - lay.w % 2 } else { canvas_w };
            let dh = if lay.h > 0 {
                lay.h - lay.h % 2
            } else {
                canvas_h - canvas_h % 2
            };
            chain.push_str(&format!("scale={dw}:{dh}"));
            let label = format!("[scr{layer_no}]");
            filters.push(format!("[{s}:v]setpts=PTS-STARTPTS,{chain}{label}"));
            filters.push(format!(
                "{composed}{label}overlay=x={}:y={}:shortest=0[canvas{}]",
                lay.x.max(0),
                lay.y.max(0),
                layer_no + 1
            ));
            composed = format!("[canvas{}]", layer_no + 1);
            layer_no += 1;
        }
    }

    // 摄像头图层
    if let Some(c) = cam_idx {
        let lay = &p.camera_layout;
        if lay.visible {
            let mut chain = String::new();
            if lay.crop_w > 0 && lay.crop_h > 0 {
                chain.push_str(&format!(
                    "crop={}:{}:{}:{},",
                    lay.crop_w, lay.crop_h, lay.crop_x, lay.crop_y
                ));
            }
            let dw = if lay.w > 0 {
                lay.w - lay.w % 2
            } else {
                (canvas_w / 4) - (canvas_w / 4) % 2
            };
            let dh = if lay.h > 0 {
                lay.h - lay.h % 2
            } else {
                0
            };
            if dh > 0 {
                chain.push_str(&format!("scale={dw}:{dh}"));
            } else {
                chain.push_str(&format!("scale={dw}:-2"));
            }
            let label = format!("[cam{layer_no}]");
            filters.push(format!("[{c}:v]setpts=PTS-STARTPTS,{chain}{label}"));
            filters.push(format!(
                "{composed}{label}overlay=x={}:y={}:shortest=0[canvas{}]",
                lay.x.max(0),
                lay.y.max(0),
                layer_no + 1
            ));
            composed = format!("[canvas{}]", layer_no + 1);
            let _ = layer_no + 1;
        }
    }

    // 没有任何可见图层：直接报错，避免用户看到全黑却不知原因
    if layer_no == 0 {
        return Err("没有可见的图层可预览（请在画布上启用至少一个图层）".to_string());
    }

    // 整体缩放到预览分辨率
    filters.push(format!("{composed}scale={target_w}:{target_h},format=yuv420p[v]"));

    let filter = filters.join(";");

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
    let mut stdout: std::process::ChildStdout = if need_rebuild {
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

    // ── 用 spawn_blocking 读管道，避免阻塞 tokio worker ──
    //
    // `ChildStdout::read` 是**阻塞**调用。若直接在 async 流里 read，
    // 每路预览都会长期占住一个 tokio worker 线程（预览是持续不断的流，
    // 意味着该线程永远回不到池里），多开几路就会把运行时饿死，
    // 表现为整个应用的其它 IPC 命令都卡顿。
    // 这里改成：阻塞读放在 spawn_blocking 里，通过 channel 把数据交给
    // 异步流。通道有界（容量 8），慢客户端不会让内存无限增长。
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(8);

    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 8192];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    // blocking_send：等异步侧取走，天然形成背压
                    if tx.blocking_send(Ok(Vec::from(&buf[..n]))).is_err() {
                        break; // 客户端已断开
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
    headers.insert(
        header::CONNECTION,
        header::HeaderValue::from_static("close"),
    );
    resp
}
