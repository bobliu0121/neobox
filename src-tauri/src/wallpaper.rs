//! 壁纸管理：系统静态壁纸 + 动态壁纸（GIF / HTML，BETA）。
//!
//! # 动态壁纸
//!
//! 动态壁纸是一个**透明、无边框、鼠标穿透**的 WebView2 窗口，挂到 Windows
//! 桌面壁纸层（`WorkerW`，位于桌面图标之下）：
//!
//! - **GIF**：加载应用内 `/wallpaper-window` 页面，页面用 `<img>` 以
//!   `object-fit: cover` 铺满整块屏幕播放；
//! - **HTML**：同一个页面里用全屏 `<iframe>` 加载 HTML 所在目录的本地静态服务
//!   （只监听 `127.0.0.1` 随机端口，见 [`crate::wallpaper_server`]）。这样相对路径的
//!   CSS / JS / 图片、ES module、`fetch`、CDN 资源都按浏览器语义正常工作。
//!
//!   > 注意：不能让 WebView 直接顶层导航到 asset URL。WebView2 的
//!   > `WebResourceRequested` 过滤器不覆盖顶层文档导航，Tauri 的 asset 协议
//!   > 因此拦不到这次请求，页面会变成空白；而用 asset URL 加载 HTML 时，
//!   > 整条路径被编码成一个 URL 段，相对路径资源会全部 404。
//!
//! ## 旧实现的问题（本次重写修复）
//!
//! 1. 旧代码把 `<img src="asset://localhost/...">` 塞进 data URL：
//!    Windows 上 Tauri 的 asset 协议只注册了 `http://asset.localhost/...`，
//!    `asset://` 根本不会被 WebView2 处理，GIF 永远加载失败 —— 表现为动态壁纸
//!    黑屏/空桌面。这里改用与应用内 `convertFileSrc` 完全一致的 URL 形式。
//! 2. 旧代码 `SetWindowPos(HWND_BOTTOM, ..., SWP_NOZORDER)` 同时传了
//!    `SWP_NOZORDER`，z-order 实际没生效，窗口可能浮在图标之上。
//! 3. 旧代码在 `SetParent` 之后仍按虚拟屏幕坐标摆放窗口，多显示器/负坐标时
//!    会偏出屏幕；现在按父窗口客户区尺寸重新铺满。
//! 4. 旧窗口没有统一的 WebView2 参数，置底窗口会被 Chromium 判定为“被遮挡”
//!    而暂停渲染（GIF 卡住/掉帧）。现在与其它窗口共用 [`BROWSER_ARGS`]。
//! 5. 创建窗口的命令必须是 async 的：同步命令在主线程执行，而
//!    `WebviewWindowBuilder::build()` 需要主线程事件循环，会直接死锁
//!    （表现为桌面无变化、所有 IPC 一起卡住）。

use base64::Engine as _;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GetClassNameW, GetClientRect, GetParent,
    GetSystemMetrics, GetWindowLongPtrW, GWL_EXSTYLE, GWL_STYLE, HWND_BOTTOM, IsWindowVisible,
    SendMessageTimeoutW, SetParent, SetWindowPos, SetWindowLongPtrW, SMTO_NORMAL, SM_CXSCREEN,
    SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, WS_CHILD,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::webview_args::BROWSER_ARGS;
use crate::wallpaper_server::LocalServer;

/// 动态壁纸窗口 label（同时对应前端路由 `/wallpaper-window`）
pub const WALLPAPER_WINDOW_LABEL: &str = "wallpaper-window";

/// 当前正在播放的动态壁纸（无窗口时也会保留，供前端恢复/展示状态）。
#[derive(Clone, Debug, Serialize)]
pub struct DynamicWallpaperInfo {
    /// `gif` 或 `html`
    pub kind: String,
    pub path: String,
    /// HTML 壁纸的本地服务地址（GIF 为 None）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

static ACTIVE: Mutex<Option<DynamicWallpaperInfo>> = Mutex::new(None);

/// HTML 壁纸使用的本地静态服务器；切走/停止时 drop 掉。
static SERVER: Mutex<Option<LocalServer>> = Mutex::new(None);

/// 文件扩展名 → 动态壁纸类型（GIF / HTML / 视频）。
fn dynamic_kind(path: &str) -> Option<&'static str> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase();
    match ext.as_str() {
        "gif" => Some("gif"),
        "html" | "htm" => Some("html"),
        "mp4" | "webm" | "mov" | "mkv" | "avi" | "m4v" => Some("video"),
        _ => None,
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 应用内壁纸页面 URL（`http://tauri.localhost/wallpaper-window`）。
fn app_page_url(app: &tauri::AppHandle) -> String {
    if tauri::is_dev() {
        if let Some(dev) = &app.config().build.dev_url {
            return format!("{}/wallpaper-window", dev.as_str().trim_end_matches('/'));
        }
    }
    "http://tauri.localhost/wallpaper-window".to_string()
}

/// 虚拟屏幕边界（覆盖所有显示器，含负坐标）。
fn virtual_bounds() -> (i32, i32, i32, i32) {
    let (mut w, mut h) = unsafe {
        (
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    if w <= 0 || h <= 0 {
        unsafe {
            w = GetSystemMetrics(SM_CXSCREEN);
            h = GetSystemMetrics(SM_CYSCREEN);
        }
    }
    let (x, y) = if w > 0 && h > 0 {
        unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
            )
        }
    } else {
        (0, 0)
    };
    (x, y, w.max(1), h.max(1))
}

unsafe fn client_size(host: HWND) -> (i32, i32) {
    let mut rect = RECT::default();
    if GetClientRect(host, &mut rect).is_ok() && rect.right > rect.left && rect.bottom > rect.top {
        (rect.right - rect.left, rect.bottom - rect.top)
    } else {
        let (_, _, w, h) = virtual_bounds();
        (w, h)
    }
}

/// 在枚举到的顶级窗口里找承载壁纸的 WorkerW：
/// 先定位含 `SHELLDLL_DefView`（桌面图标层）的窗口，再取它后面那个 WorkerW。
unsafe extern "system" fn find_workerw_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    if FindWindowExW(hwnd, None, w!("SHELLDLL_DefView"), None).is_ok() {
        if let Ok(worker) = FindWindowExW(None, hwnd, w!("WorkerW"), None) {
            *(lparam.0 as *mut HWND) = worker;
            return BOOL(0);
        }
    }
    BOOL(1)
}

/// 宿主机窗口：优先承载壁纸的 WorkerW（图标之下），没有则退回 Progman。
unsafe fn find_wallpaper_host() -> Option<HWND> {
    let progman = FindWindowW(w!("Progman"), PCWSTR::null()).ok()?;
    // 请求 Explorer 创建/刷新壁纸层（Win10/11 通用做法，幂等）
    let _ = SendMessageTimeoutW(
        progman,
        0x052C,
        WPARAM(0xD),
        LPARAM(0),
        SMTO_NORMAL,
        1000,
        None,
    );

    // Windows 11 24H2 起，壁纸层 WorkerW 是 Progman 的子窗口（与桌面图标层
    // SHELLDLL_DefView 同级）；此时直接挂进去，z-order 天然位于图标之下。
    if let Ok(worker) = FindWindowExW(progman, None, w!("WorkerW"), None) {
        if IsWindowVisible(worker).as_bool() {
            return Some(worker);
        }
    }

    // Windows 10 / 早期 Win11：壁纸层是顶级 WorkerW，位于含 SHELLDLL_DefView
    // 的窗口之后。
    let mut worker = HWND(std::ptr::null_mut());
    let _ = EnumWindows(
        Some(find_workerw_proc),
        LPARAM(&mut worker as *mut HWND as isize),
    );
    if !worker.is_invalid() && IsWindowVisible(worker).as_bool() {
        return Some(worker);
    }

    // 兜底：挂到 Progman，并插到图标层 SHELLDLL_DefView 之后（下方）。
    Some(progman)
}

/// 把壁纸窗口挂到桌面壁纸层，并铺满宿主机客户区。
///
/// `interactive` 为 true 时（HTML 壁纸）不设置鼠标穿透，让 HTML 页面能响应点击。
fn attach_to_desktop(win: &WebviewWindow, interactive: bool) -> bool {
    let Ok(raw) = win.hwnd() else {
        return false;
    };
    let hwnd = HWND(raw.0);

    unsafe {
        // 鼠标穿透 + 不抢焦点 + 不出现在 Alt+Tab / 任务栏
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let ex = if interactive {
            // HTML 壁纸：保留 NOACTIVATE / TOOLWINDOW，去掉 TRANSPARENT 让鼠标事件到达页面
            (ex | WS_EX_NOACTIVATE.0 as isize | WS_EX_TOOLWINDOW.0 as isize) & !WS_EX_TRANSPARENT.0 as isize
        } else {
            ex | WS_EX_TRANSPARENT.0 as isize
                | WS_EX_NOACTIVATE.0 as isize
                | WS_EX_TOOLWINDOW.0 as isize
        };
        let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex);

        let Some(host) = find_wallpaper_host() else {
            // 连 Progman 都找不到：保持顶级窗口，压到最底层
            let _ = SetWindowPos(
                hwnd,
                HWND_BOTTOM,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            return false;
        };

        // 成为宿主的子窗口必须带 WS_CHILD，否则 z-order 仍按顶级窗口处理，
        // 壁纸会浮到桌面图标之上；同时去掉 WS_POPUP（无边框窗口默认是 POPUP）。
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let _ = SetWindowLongPtrW(
            hwnd,
            GWL_STYLE,
            ((style & !WS_POPUP.0) | WS_CHILD.0) as isize,
        );

        if SetParent(hwnd, host).is_err() {
            let _ = SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
            return false;
        }
        let (cw, ch) = client_size(host);

        // 挂到 Progman 时插到图标层之后（下方），否则直接压到宿主窗口最底层。
        let insert_after = FindWindowExW(host, None, w!("SHELLDLL_DefView"), None)
            .unwrap_or(HWND_BOTTOM);
        let _ = SetWindowPos(
            hwnd,
            insert_after,
            0,
            0,
            cw,
            ch,
            SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_FRAMECHANGED,
        );
        log::info!(
            "[Wallpaper] 已挂载到桌面壁纸层 host={:?} size={}x{}",
            host,
            cw,
            ch
        );
        true
    }
}

/// 在 Tauri 主线程执行挂载。
///
/// 窗口属于主线程；从命令线程直接 `SetParent` 虽然多数情况下能用，但会打乱
/// 窗口的输入队列/线程归属，保险起见统一切回主线程执行。
fn attach_to_desktop_on_main(app: &tauri::AppHandle, win: &WebviewWindow, interactive: bool) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    let target = win.clone();
    if app
        .run_on_main_thread(move || {
            let _ = tx.send(attach_to_desktop(&target, interactive));
        })
        .is_err()
    {
        return false;
    }
    rx.recv_timeout(Duration::from_secs(3)).unwrap_or(false)
}

/// 判断窗口是否仍挂在桌面壁纸层（Explorer 重启后会掉出来）。
fn is_attached(win: &WebviewWindow) -> bool {
    let Ok(raw) = win.hwnd() else {
        return false;
    };
    unsafe {
        let Ok(parent) = GetParent(HWND(raw.0)) else {
            return false;
        };
        if parent.is_invalid() {
            return false;
        }
        let mut class = [0u16; 64];
        let len = GetClassNameW(parent, &mut class);
        if len <= 0 {
            return false;
        }
        matches!(
            String::from_utf16_lossy(&class[..len as usize]).as_str(),
            "WorkerW" | "Progman"
        )
    }
}

/// 创建（或复用）动态壁纸窗口。
fn ensure_window(app: &tauri::AppHandle) -> Result<WebviewWindow, String> {
    if let Some(win) = app.get_webview_window(WALLPAPER_WINDOW_LABEL) {
        return Ok(win);
    }

    let win = WebviewWindowBuilder::new(
        app,
        WALLPAPER_WINDOW_LABEL,
        WebviewUrl::App("wallpaper-window".into()),
    )
    .title("")
    .additional_browser_args(BROWSER_ARGS)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .skip_taskbar(true)
    .focused(false)
    .visible(false)
    .build()
    .map_err(|e| format!("创建动态壁纸窗口失败: {e}"))?;

    let (x, y, w, h) = virtual_bounds();
    win.set_position(tauri::PhysicalPosition::new(x, y))
        .map_err(|e| format!("设置动态壁纸窗口位置失败: {e}"))?;
    win.set_size(tauri::PhysicalSize::new(w as u32, h as u32))
        .map_err(|e| format!("设置动态壁纸窗口尺寸失败: {e}"))?;

    log::info!(
        "[Wallpaper] 创建动态壁纸窗口 bounds=({x},{y},{w},{h}) actual_size={:?} actual_pos={:?}",
        win.inner_size(),
        win.outer_position()
    );

    Ok(win)
}

/// 切换窗口内容：窗口始终停留在应用内 `/wallpaper-window` 页面，
/// 由该页面按 `kind` 渲染 `<img>`（GIF）或 `<iframe>`（HTML）。
fn switch_content(
    app: &tauri::AppHandle,
    win: &WebviewWindow,
    info: &DynamicWallpaperInfo,
) -> Result<(), String> {
    match win.url() {
        // 已在壁纸页面：只通知页面换内容，避免整页重载造成闪白
        Ok(url) if url.path().starts_with("/wallpaper-window") => {
            let _ = app.emit_to(WALLPAPER_WINDOW_LABEL, "wallpaper:changed", info);
            Ok(())
        }
        // 被导航到别处（理论上不会发生）：回到应用内壁纸页面，
        // 页面挂载后会主动拉取当前状态。
        Ok(url) if matches!(url.scheme(), "http" | "https") => {
            navigate(win, &app_page_url(app))
        }
        // 首次创建、页面还在加载：交给页面挂载时读取状态
        _ => Ok(()),
    }
}

fn navigate(win: &WebviewWindow, url: &str) -> Result<(), String> {
    let parsed = tauri::Url::parse(url).map_err(|e| format!("无效的壁纸地址: {e}"))?;
    win.navigate(parsed)
        .map_err(|e| format!("加载壁纸页面失败: {e}"))
}

/// 显示器/Explorer 变化时的守护线程：挂载丢了就重新挂回去。
fn spawn_watchdog(app: tauri::AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let Some(win) = app.get_webview_window(WALLPAPER_WINDOW_LABEL) else {
                break;
            };
            if lock(&ACTIVE).is_none() {
                break;
            }
            let interactive = lock(&ACTIVE)
                .as_ref()
                .map(|i| i.kind == "html")
                .unwrap_or(false);
            if !is_attached(&win) && !attach_to_desktop_on_main(&app, &win, interactive) {
                // 挂载失败就停掉守护，避免反复触发 Explorer 创建 WorkerW
                break;
            }
        }
        RUNNING.store(false, Ordering::SeqCst);
    });
}

/// 设置动态壁纸（GIF / HTML），窗口不存在时自动创建。
///
/// 返回实际生效的 `{ kind, path }`，前端据此更新 UI 与持久化状态。
#[tauri::command]
pub async fn set_dynamic_wallpaper(
    app: tauri::AppHandle,
    path: String,
) -> Result<DynamicWallpaperInfo, String> {
    let kind = dynamic_kind(&path).ok_or_else(|| "仅支持 GIF / HTML 动态壁纸".to_string())?;
    let file = Path::new(&path);
    if !file.is_file() {
        return Err("壁纸文件不存在".to_string());
    }

    // HTML 壁纸通过本地静态服务提供，相对路径资源才能正常加载
    let (url, server) = if kind == "html" {
        let root = file
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let server = crate::wallpaper_server::start(root)?;
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "无法读取 HTML 文件名".to_string())?;
        let url = format!(
            "http://127.0.0.1:{}/{}",
            server.port(),
            urlencoding::encode(name)
        );
        log::info!("[Wallpaper] HTML 本地服务 {} -> {}", url, file.display());
        (Some(url), Some(server))
    } else {
        (None, None)
    };

    // 旧的服务（换壁纸/换类型）在 ensure_window 成功后替换，失败时随局部变量 drop
    let info = DynamicWallpaperInfo {
        kind: kind.to_string(),
        path,
        url,
    };

    let win = ensure_window(&app)?;
    *lock(&SERVER) = server;
    *lock(&ACTIVE) = Some(info.clone());

    if let Err(e) = switch_content(&app, &win, &info) {
        *lock(&ACTIVE) = None;
        *lock(&SERVER) = None;
        log::error!("[Wallpaper] 切换动态壁纸失败: {e}");
        return Err(e);
    }

    // 页面在内容（GIF 解码 / HTML iframe）就绪后会调用 wallpaper_window_ready
    // 再显示窗口，避免透明窗口先闪一帧空白。
    if win.is_visible().unwrap_or(false) {
        attach_to_desktop_on_main(&app, &win, info.kind == "html");
    }
    log::info!(
        "[Wallpaper] 设置动态壁纸 kind={} path={} visible={}",
        info.kind,
        info.path,
        win.is_visible().unwrap_or(false)
    );
    spawn_watchdog(app);
    Ok(info)
}

/// 壁纸页面首帧就绪后调用：显示窗口并挂到桌面壁纸层。
///
/// GIF 页面在图片解码完成前保持窗口隐藏，避免透明窗口先闪出一帧空白/黑底。
#[tauri::command]
pub async fn wallpaper_window_ready(app: tauri::AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window(WALLPAPER_WINDOW_LABEL)
        .ok_or_else(|| "动态壁纸窗口不存在".to_string())?;
    win.show()
        .map_err(|e| format!("显示动态壁纸窗口失败: {e}"))?;
    let interactive = lock(&ACTIVE)
        .as_ref()
        .map(|i| i.kind == "html")
        .unwrap_or(false);
    let attached = attach_to_desktop_on_main(&app, &win, interactive);
    log::info!("[Wallpaper] 壁纸页面就绪 attached={attached}");
    Ok(())
}

/// 停止动态壁纸并销毁壁纸窗口（恢复系统壁纸可见）。
#[tauri::command]
pub async fn stop_dynamic_wallpaper(app: tauri::AppHandle) -> Result<(), String> {
    *lock(&ACTIVE) = None;
    *lock(&SERVER) = None;
    if let Some(win) = app.get_webview_window(WALLPAPER_WINDOW_LABEL) {
        win.destroy()
            .map_err(|e| format!("关闭动态壁纸窗口失败: {e}"))?;
    }
    Ok(())
}

/// 查询当前动态壁纸（前端页面挂载/恢复时使用）。
#[tauri::command]
pub fn get_dynamic_wallpaper() -> Option<DynamicWallpaperInfo> {
    lock(&ACTIVE).clone()
}

/// 设置系统壁纸（Windows SystemParametersInfoW SPI_SETDESKWALLPAPER）
#[tauri::command]
pub fn set_wallpaper(path: String) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_SETDESKWALLPAPER, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE,
    };
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            Some(wide.as_ptr() as *mut core::ffi::c_void),
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        )
        .map_err(|e| format!("设置壁纸失败: {e:?}"))?;
    }
    Ok(())
}

/// 读取系统当前壁纸路径（SPI_GETDESKWALLPAPER，可反映其他应用/系统设置中的修改）
#[tauri::command]
pub fn get_current_wallpaper() -> Result<Option<String>, String> {
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETDESKWALLPAPER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };
    let mut buf = [0u16; 1024];
    unsafe {
        SystemParametersInfoW(
            SPI_GETDESKWALLPAPER,
            buf.len() as u32,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .map_err(|e| format!("读取当前壁纸失败: {e:?}"))?;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let s = String::from_utf16_lossy(&buf[..len]);
    if s.is_empty() {
        Ok(None)
    } else {
        Ok(Some(s))
    }
}

/// 支持的壁纸文件扩展名（静态图片 + 动态壁纸）
const WALLPAPER_EXTS: [&str; 16] = [
    "png", "jpg", "jpeg", "webp", "bmp", "gif", "tiff", "jfif", "html", "htm",
    "mp4", "webm", "mov", "mkv", "avi", "m4v",
];

/// 保存壁纸编辑结果（data_url 覆盖/另存为指定路径）
#[tauri::command]
pub fn save_wallpaper_edit(path: String, data_url: String) -> Result<(), String> {
    let b64 = data_url
        .split(',')
        .nth(1)
        .ok_or_else(|| "无效的图片数据".to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("图片数据解码失败: {e}"))?;
    if let Some(parent) = std::path::Path::new(&path).parent() {
        if !parent.exists() {
            return Err("目标文件夹不存在".to_string());
        }
    }
    std::fs::write(&path, &bytes).map_err(|e| format!("写入文件失败: {e}"))?;
    Ok(())
}

/// 扫描壁纸文件夹，返回支持的图片/网页文件绝对路径。
/// `recursive` 为 true 时递归包含子文件夹。
#[tauri::command]
pub fn scan_wallpaper_folder(dir: String, recursive: Option<bool>) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let root = Path::new(&dir);
    if !root.is_dir() {
        return Err("文件夹不存在".to_string());
    }

    let mut visit = |path: &Path| {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let ext_lower = ext.to_lowercase();
            if WALLPAPER_EXTS.contains(&ext_lower.as_str()) {
                if let Some(s) = path.to_str() {
                    out.push(s.to_string());
                }
            }
        }
    };

    if recursive.unwrap_or(false) {
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            if entry.file_type().is_file() {
                visit(entry.path());
            }
        }
    } else {
        let entries = std::fs::read_dir(root).map_err(|e| format!("读取文件夹失败: {e}"))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                visit(&path);
            }
        }
    }

    out.sort();
    Ok(out)
}
