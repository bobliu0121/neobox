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
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, IsWindowEnabled, SetActiveWindow, SetFocus,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GetAncestor, GetClassNameW, GetClientRect,
    GetDesktopWindow, GetParent, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect, GWL_EXSTYLE,
    GWL_STYLE, HWND_BOTTOM, HWND_TOP, IsWindow, IsWindowVisible, SendMessageTimeoutW,
    SetForegroundWindow, SetParent, SetWindowPos, SetWindowLongPtrW, GA_ROOT, SMTO_NORMAL,
    SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    WS_CHILD, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
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

/// HTML 壁纸交互模式：true 时窗口整体可交互（鼠标或键盘任一开启）。
static INTERACTIVE: Mutex<bool> = Mutex::new(false);

/// 鼠标交互开关（对应设置页「鼠标」）。
static INTERACTIVE_MOUSE: Mutex<bool> = Mutex::new(false);

/// 键盘交互开关（对应设置页「键盘」）。
static INTERACTIVE_KEYBOARD: Mutex<bool> = Mutex::new(false);

/// 交互期间被我们临时启用（`EnableWindow(true)`）的祖先窗口。
///
/// Explorer 把壁纸层 `WorkerW` 创建为禁用状态，子窗口因此收不到任何输入。
/// 开启交互时要临时启用，关闭交互时必须还原成禁用，否则会改变 Explorer
/// 的窗口状态（影响桌面图标的输入行为）。这里记录句柄以便精确还原。
static ENABLED_ANCESTOR: Mutex<Option<isize>> = Mutex::new(None);

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

/// 读取窗口类名（WorkerW / Progman / SHELLDLL_DefView 等）。
unsafe fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let len = GetClassNameW(hwnd, &mut buf);
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len as usize])
}

/// 窗口的屏幕矩形与面积（宽为负或不可见时返回 None）。
unsafe fn window_area(hwnd: HWND) -> Option<(i32, i32, i64)> {
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err() {
        return None;
    }
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    if w <= 0 || h <= 0 {
        return None;
    }
    Some((w, h, (w as i64) * (h as i64)))
}

/// 枚举 WorkerW 时的检索条件与最优结果。
///
/// 桌面壁纸层的特征是：**可见、铺满全屏、内部不含 `SHELLDLL_DefView`**。
/// 含 `SHELLDLL_DefView` 的那个 WorkerW 是桌面图标层，挂进去壁纸会被图标层
/// 遮住；尺寸很小的 WorkerW 是 Office/触摸键盘等控件用的，一并跳过。
struct WorkerWSearch {
    /// true = 必须不含 SHELLDLL_DefView（壁纸层）；false = 必须有（图标层）
    require_no_defview: bool,
    min_w: i32,
    min_h: i32,
    best: Option<HWND>,
    best_area: i64,
}

unsafe extern "system" fn enum_workerw_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let st = &mut *(lparam.0 as *mut WorkerWSearch);

    if class_name(hwnd) != "WorkerW" || !IsWindowVisible(hwnd).as_bool() {
        return BOOL(1);
    }
    let Some((w, h, area)) = window_area(hwnd) else {
        return BOOL(1);
    };
    if w < st.min_w || h < st.min_h {
        return BOOL(1);
    }

    let has_defview = FindWindowExW(hwnd, None, w!("SHELLDLL_DefView"), None).is_ok();
    if has_defview == st.require_no_defview {
        // require_no_defview=true 却含 DefView，或反之
        return BOOL(1);
    }

    if st.best.is_none() || area > st.best_area {
        st.best = Some(hwnd);
        st.best_area = area;
    }
    BOOL(1)
}

/// 在顶级窗口里找符合条件的 WorkerW（取面积最大的）。
unsafe fn find_workerw_top(st: &mut WorkerWSearch) -> Option<HWND> {
    let _ = EnumWindows(
        Some(enum_workerw_proc),
        LPARAM(st as *mut WorkerWSearch as isize),
    );
    st.best
}

/// 在 Progman 的子窗口里找符合条件的 WorkerW。
///
/// Win10 1809+/21H1 实测：壁纸层 WorkerW 会作为 Progman 的**子窗口**出现
/// （见 [`request_wallpaper_layer`]），而不是顶级窗口，所以必须两个层级都找。
unsafe fn find_workerw_child_of(progman: HWND, st: &mut WorkerWSearch) -> Option<HWND> {
    let mut child = FindWindowExW(progman, None, w!("WorkerW"), None);
    while let Ok(hwnd) = child {
        if !hwnd.is_invalid() {
            if IsWindowVisible(hwnd).as_bool() {
                if let Some((w, h, area)) = window_area(hwnd) {
                    if w >= st.min_w && h >= st.min_h {
                        let has_defview =
                            FindWindowExW(hwnd, None, w!("SHELLDLL_DefView"), None).is_ok();
                        if has_defview != st.require_no_defview
                            && (st.best.is_none() || area > st.best_area)
                        {
                            st.best = Some(hwnd);
                            st.best_area = area;
                        }
                    }
                }
            }
        }
        // FindWindowExW 返回 Err 表示没有更多同类子窗口
        child = FindWindowExW(progman, hwnd, w!("WorkerW"), None);
    }
    st.best
}

/// 定位 Progman（Program Manager）窗口。
///
/// 优先 `FindWindowW`，失败再用 `EnumWindows` 兜底：部分 Win10 机器上
/// `FindWindowW("Progman")` 会返回 NULL（Explorer 刚重启/句柄瞬变），
/// 而此时枚举同样能拿到，兜底可避免整条挂载逻辑退化成“贴桌面窗口”。
unsafe fn find_progman() -> Option<HWND> {
    if let Ok(hwnd) = FindWindowW(w!("Progman"), PCWSTR::null()) {
        if !hwnd.is_invalid() {
            return Some(hwnd);
        }
    }

    unsafe extern "system" fn enum_progman(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let slot = &mut *(lparam.0 as *mut Option<HWND>);
        if class_name(hwnd) == "Progman" {
            *slot = Some(hwnd);
            return BOOL(0);
        }
        BOOL(1)
    }

    let mut found: Option<HWND> = None;
    let _ = EnumWindows(
        Some(enum_progman),
        LPARAM(&mut found as *mut Option<HWND> as isize),
    );
    found
}

/// 请求 Explorer 创建（或刷新）桌面壁纸层 WorkerW。
///
/// `WM_SPAWN_WORKER`(0x052C) 的参数在不同 Windows 版本上并不一致：
///
/// | 系统 | 可用参数 |
/// |---|---|
/// | Win10 1809~21H1 | `wParam=0xD, lParam=1` |
/// | Win10 22H2 / Win11 22H2~23H2 | `wParam=0xD, lParam=0` |
/// | 部分旧版本 | `wParam=0x0, lParam=0/1` |
///
/// 只用一种参数会在不匹配的机器上**静默失败**（消息返回成功，但 WorkerW
/// 始终不出现），表现就是动态壁纸在 Win11 正常、在 Win10 完全不显示。
/// 这里按顺序逐个尝试，一旦检测到壁纸层就停止；该消息本身幂等，重复发送
/// 只会让 Explorer 复用已有层，不会产生额外窗口。
unsafe fn request_wallpaper_layer(progman: HWND, min_w: i32, min_h: i32) -> Option<HWND> {
    // 先看是否已经存在（例如别家壁纸软件或上次运行已创建）
    let mut st = WorkerWSearch {
        require_no_defview: true,
        min_w,
        min_h,
        best: None,
        best_area: 0,
    };
    if let Some(hwnd) = find_workerw_top(&mut st) {
        return Some(hwnd);
    }
    st.best = None;
    st.best_area = 0;
    if let Some(hwnd) = find_workerw_child_of(progman, &mut st) {
        return Some(hwnd);
    }

    const VARIANTS: [(usize, isize); 4] = [(0xD, 1), (0xD, 0), (0x0, 1), (0x0, 0)];
    for (wp, lp) in VARIANTS {
        let _ = SendMessageTimeoutW(
            progman,
            0x052C,
            WPARAM(wp),
            LPARAM(lp),
            SMTO_NORMAL,
            1000,
            None,
        );
        // Explorer 创建 WorkerW 是异步的，给它一点时间再枚举
        std::thread::sleep(Duration::from_millis(80));

        let mut st = WorkerWSearch {
            require_no_defview: true,
            min_w,
            min_h,
            best: None,
            best_area: 0,
        };
        if let Some(hwnd) = find_workerw_top(&mut st) {
            log::info!(
                "[Wallpaper] 壁纸层已就绪（顶级 WorkerW，0x052C wParam=0x{wp:X} lParam={lp}）: {hwnd:?}"
            );
            return Some(hwnd);
        }
        st.best = None;
        st.best_area = 0;
        if let Some(hwnd) = find_workerw_child_of(progman, &mut st) {
            log::info!(
                "[Wallpaper] 壁纸层已就绪（Progman 子 WorkerW，0x052C wParam=0x{wp:X} lParam={lp}）: {hwnd:?}"
            );
            return Some(hwnd);
        }
    }
    log::warn!("[Wallpaper] 0x052C 所有参数组合都未能创建壁纸层 WorkerW");
    None
}

/// 宿主机窗口：优先承载壁纸的 WorkerW（图标之下），没有则退回桌面窗口。
unsafe fn find_wallpaper_host() -> Option<HWND> {
    let (_, _, vw, vh) = virtual_bounds();
    // 壁纸层至少覆盖半个屏幕，用来排除 135x37 这类控件用 WorkerW
    let min_w = (vw / 2).max(320);
    let min_h = (vh / 2).max(240);

    let progman = find_progman();
    if let Some(progman) = progman {
        if let Some(layer) = request_wallpaper_layer(progman, min_w, min_h) {
            return Some(layer);
        }

        // 兜底 1：图标层（含 SHELLDLL_DefView）后面的那个顶级 WorkerW
        let mut icons = WorkerWSearch {
            require_no_defview: false,
            min_w: 0,
            min_h: 0,
            best: None,
            best_area: 0,
        };
        let icons_host = find_workerw_top(&mut icons);
        if let Some(icons_host) = icons_host {
            if let Ok(sibling) = FindWindowExW(None, icons_host, w!("WorkerW"), None) {
                if !sibling.is_invalid() && IsWindowVisible(sibling).as_bool() {
                    log::info!("[Wallpaper] 用图标层之后的顶层 WorkerW 作宿主: {sibling:?}");
                    return Some(sibling);
                }
            }
        }

        // 兜底 2：直接挂 Progman（壁纸在图标层下面）
        log::warn!("[Wallpaper] 未找到壁纸层 WorkerW，退回 Progman: {progman:?}");
        return Some(progman);
    }

    // 找不到 Progman：从 GetDesktopWindow() 的子窗口里找
    let desktop = GetDesktopWindow();
    if !desktop.is_invalid() {
        if let Ok(prog_child) = FindWindowExW(desktop, None, w!("Progman"), None) {
            if !prog_child.is_invalid() {
                log::info!("[Wallpaper] Found Progman as child of desktop: {prog_child:?}");
                if let Some(layer) = request_wallpaper_layer(prog_child, min_w, min_h) {
                    return Some(layer);
                }
                log::info!("[Wallpaper] No WorkerW, using desktop window: {desktop:?}");
                return Some(desktop);
            }
        }
        log::info!("[Wallpaper] Using GetDesktopWindow() as host: {desktop:?}");
        return Some(desktop);
    }

    None
}

/// 把壁纸窗口挂到桌面壁纸层，并铺满宿主机客户区。
///
/// 返回是否成功挂载；失败时窗口仍是普通置底窗口（也可用，只是可能盖住图标）。
fn attach_to_desktop(win: &WebviewWindow) -> bool {
    let Ok(raw) = win.hwnd() else {
        return false;
    };
    let hwnd = HWND(raw.0);

    unsafe {
        // 鼠标穿透 + 不抢焦点 + 不出现在 Alt+Tab / 任务栏
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let _ = SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            ex | WS_EX_TRANSPARENT.0 as isize
                | WS_EX_NOACTIVATE.0 as isize
                | WS_EX_TOOLWINDOW.0 as isize,
        );

        let Some(host) = find_wallpaper_host() else {
            // 连 Progman 都找不到：保持顶级窗口，全屏大小，压到最底层
            let (_, _, cw, ch) = virtual_bounds();
            let _ = SetWindowPos(
                hwnd,
                HWND_BOTTOM,
                0,
                0,
                cw,
                ch,
                SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_FRAMECHANGED,
            );
            log::warn!("[Wallpaper] find_wallpaper_host returned None, using fullscreen fallback");
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

        let parent_ok = SetParent(hwnd, host).is_ok();
        if !parent_ok {
            // SetParent 失败：恢复样式，保持顶级窗口
            let _ = SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
            log::warn!("[Wallpaper] SetParent failed, staying as top-level window");
        }

        // 获取大小：如果挂载成功用 host 客户区大小，否则用虚拟屏幕大小
        let (cw, ch) = if parent_ok {
            client_size(host)
        } else {
            let (_, _, w, h) = virtual_bounds();
            (w, h)
        };

        // z-order：越低越好，但必须**高于**桌面背景绘制、**低于**图标层。
        //
        // 分两种情况：
        // - 宿主是壁纸层 WorkerW（内部没有 SHELLDLL_DefView）：HWND_BOTTOM 即可，
        //   该 WorkerW 本身就是壁纸专用的，压到底不会盖住图标。
        // - 宿主是 Progman（兜底路径，图标层是它的子窗口）：HWND_BOTTOM 会沉到
        //   桌面背景之下被完全遮住（Win10 实测现象），必须插到 SHELLDLL_DefView
        //   之后 —— SetWindowPos 的 hWndInsertAfter 在 z-order 中是“放在该窗口
        //   之前（更靠下）”，把 DefView 传进去正好让壁纸贴在图标层下面。
        let shell_view = FindWindowExW(host, None, w!("SHELLDLL_DefView"), None);
        let host_class = class_name(host);
        let is_progman_host = host_class == "Progman" || host_class == "#32769";

        let insert_after = if is_progman_host {
            match shell_view {
                Ok(sv) if !sv.is_invalid() => {
                    log::info!("[Wallpaper] 插入到 SHELLDLL_DefView 之后（图标层下方）: {sv:?}");
                    sv
                }
                _ => {
                    log::warn!("[Wallpaper] Progman 宿主但未找到 SHELLDLL_DefView，退化为 HWND_BOTTOM");
                    HWND_BOTTOM
                }
            }
        } else {
            HWND_BOTTOM
        };

        let _ = SetWindowPos(
            hwnd,
            insert_after,
            0,
            0,
            cw,
            ch,
            SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE,
        );

        log::info!(
            "[Wallpaper] 壁纸窗口就绪 host={:?}({}) parent_ok={} size={}x{} insert_after={:?}",
            host,
            host_class,
            parent_ok,
            cw,
            ch,
            insert_after
        );

        // 挂载过程重写了 GWL_STYLE / GWL_EXSTYLE，并把窗口挂到（默认禁用的）
        // 壁纸层下，之前设置的交互状态会失效。这里按当前开关重新应用一次，
        // 否则「HTML 鼠标/键盘」在重新挂载后会静默失效。
        let mouse = *lock(&INTERACTIVE_MOUSE);
        let keyboard = *lock(&INTERACTIVE_KEYBOARD);
        if mouse || keyboard {
            apply_interactive_state(hwnd, mouse, keyboard);
            log::info!("[Wallpaper] 挂载后重新应用交互状态 鼠标={mouse} 键盘={keyboard}");
        }

        true
    }
}

/// 在 Tauri 主线程执行挂载。
///
/// 窗口属于主线程；从命令线程直接 `SetParent` 虽然多数情况下能用，但会打乱
/// 窗口的输入队列/线程归属，保险起见统一切回主线程执行。
fn attach_to_desktop_on_main(app: &tauri::AppHandle, win: &WebviewWindow) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    let target = win.clone();
    if app
        .run_on_main_thread(move || {
            let _ = tx.send(attach_to_desktop(&target));
        })
        .is_err()
    {
        return false;
    }
    rx.recv_timeout(Duration::from_secs(3)).unwrap_or(false)
}

/// 判断窗口是否仍挂在桌面壁纸层（Explorer 重启后会掉出来）。
///
/// 父窗口可能是 `WorkerW`（顶级壁纸层 / Progman 子层）或 `Progman` 本身，
/// 也可能是 `#32769`（找不到 Progman 时的兜底宿主）。
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
        let class = class_name(parent);
        matches!(
            class.as_str(),
            "WorkerW" | "Progman" | "#32769"
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

    // 先用 Tauri API 设置
    win.set_position(tauri::PhysicalPosition::new(x, y))
        .map_err(|e| format!("设置动态壁纸窗口位置失败: {e}"))?;
    win.set_size(tauri::PhysicalSize::new(w as u32, h as u32))
        .map_err(|e| format!("设置动态壁纸窗口尺寸失败: {e}"))?;

    // 再用 Win32 API 强制设置一遍（确保生效）
    if let Ok(raw) = win.hwnd() {
        let hwnd = HWND(raw.0);
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                HWND_BOTTOM,
                x,
                y,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_FRAMECHANGED,
            );
        }
    }

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
            if !is_attached(&win) && !attach_to_desktop_on_main(&app, &win) {
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
        attach_to_desktop_on_main(&app, &win);
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
    let attached = attach_to_desktop_on_main(&app, &win);
    log::info!("[Wallpaper] 壁纸页面就绪 attached={attached}");
    Ok(())
}

/// 恢复系统静态壁纸，让桌面背景重新绘制。
///
/// # 为什么必须做这一步
///
/// 动态壁纸窗口挂在 Explorer 的壁纸层 `WorkerW` 下面。该 WorkerW 一旦被
/// `0x052C` 唤醒用来承载第三方窗口，桌面背景（系统壁纸）就不再由它自己绘制，
/// 而是交给我们的子窗口。窗口被销毁后这一层**没有任何东西绘制**，桌面就变成
/// 纯黑 —— Win10 实测：销毁窗口后像素保持不变（残留最后画面）或直接变黑，
/// 单纯 `RedrawWindow` 刷不出系统壁纸。
///
/// 只有重新走一次 `SystemParametersInfoW(SPI_SETDESKWALLPAPER)` 才能让
/// Explorer 重新把壁纸画回桌面背景层（实测：重设后桌面立刻恢复原样）。
///
/// 这个函数是幂等的：没有动态壁纸时调用也无副作用，只是让系统重画一次壁纸。
pub fn restore_desktop_wallpaper(reason: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SPI_SETDESKWALLPAPER,
    };

    unsafe {
        // 读取当前系统壁纸路径；空字符串同样有效 —— 它会让 Explorer 清空并
        // 重画壁纸层（纯色背景也会因此恢复）。
        let mut buf = [0u16; 1024];
        let got = SystemParametersInfoW(
            windows::Win32::UI::WindowsAndMessaging::SPI_GETDESKWALLPAPER,
            buf.len() as u32,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok();

        let path = if got {
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            String::from_utf16_lossy(&buf[..len])
        } else {
            String::new()
        };

        let mut wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let result = SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            Some(wide.as_mut_ptr() as *mut core::ffi::c_void),
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        );

        match result {
            Ok(()) => log::info!(
                "[Wallpaper] 已恢复系统壁纸（{reason}）path='{}'",
                if path.is_empty() { "<空>" } else { &path }
            ),
            Err(e) => log::warn!("[Wallpaper] 恢复系统壁纸失败（{reason}）: {e:?}"),
        }
    }
}

/// 停止动态壁纸并销毁壁纸窗口，同时恢复系统静态壁纸（避免桌面黑屏）。
#[tauri::command]
pub async fn stop_dynamic_wallpaper(app: tauri::AppHandle) -> Result<(), String> {
    *lock(&ACTIVE) = None;
    *lock(&SERVER) = None;
    if let Some(win) = app.get_webview_window(WALLPAPER_WINDOW_LABEL) {
        win.destroy()
            .map_err(|e| format!("关闭动态壁纸窗口失败: {e}"))?;
    }
    // 窗口销毁后壁纸层失去内容来源，必须让 Explorer 重画系统壁纸，
    // 否则桌面会停在黑屏/最后一次画面（Win10 上必现）。
    restore_desktop_wallpaper("停止动态壁纸");
    Ok(())
}

/// 退出时的兜底清理：销毁壁纸窗口并恢复系统壁纸。
///
/// 与 [`stop_dynamic_wallpaper`] 的区别是这里拿不到命令上下文、也不返回错误，
/// 只保证进程退出时桌面不留黑屏（含托盘退出、任务管理器结束前的正常退出路径）。
pub fn cleanup_on_exit(app: &tauri::AppHandle) {
    let had_dynamic = lock(&ACTIVE).is_some();
    *lock(&ACTIVE) = None;
    *lock(&SERVER) = None;

    if let Some(win) = app.get_webview_window(WALLPAPER_WINDOW_LABEL) {
        let _ = win.destroy();
    }

    if had_dynamic {
        restore_desktop_wallpaper("退出程序");
    }
}

/// HTML 壁纸交互设置，由主窗口写入、壁纸窗口读取。
///
/// # 为什么必须走后端而不是 localStorage
///
/// 壁纸窗口是**独立的 WebView 页面**（`http://tauri.localhost/wallpaper-window`），
/// 与主窗口不是同一个 JS 运行时。虽然 origin 相同，但 WebView2 里不同窗口
/// 的 localStorage 并不共享，壁纸窗口读到的永远是空值，于是设置页里的
/// 「鼠标 / 键盘」开关对壁纸完全不起作用 —— 表现为 HTML 壁纸永远无法交互。
///
/// 因此设置统一存在后端：主窗口调用 [`set_html_interaction`] 写入并立即应用到
/// 当前壁纸窗口，壁纸窗口启动时用 [`get_html_interaction`] 拉取。
#[derive(Clone, Copy, Debug, Serialize)]
pub struct HtmlInteraction {
    pub mouse: bool,
    pub keyboard: bool,
    pub audio: bool,
}

impl Default for HtmlInteraction {
    fn default() -> Self {
        // 与设置页默认值保持一致：鼠标开、键盘关、音频开
        Self {
            mouse: true,
            keyboard: false,
            audio: true,
        }
    }
}

static HTML_INTERACTION: Mutex<Option<HtmlInteraction>> = Mutex::new(None);

fn current_html_interaction() -> HtmlInteraction {
    lock(&HTML_INTERACTION).unwrap_or_default()
}

/// 主窗口写入 HTML 壁纸交互设置，并立即应用到正在播放的壁纸窗口。
///
/// 返回实际生效的设置（后端为准），前端据此更新 UI。
#[tauri::command]
pub async fn set_html_interaction(
    app: tauri::AppHandle,
    mouse: bool,
    keyboard: bool,
    audio: bool,
) -> Result<HtmlInteraction, String> {
    let settings = HtmlInteraction {
        mouse,
        keyboard,
        audio,
    };
    *lock(&HTML_INTERACTION) = Some(settings);
    *lock(&INTERACTIVE_MOUSE) = mouse;
    *lock(&INTERACTIVE_KEYBOARD) = keyboard;
    *lock(&INTERACTIVE) = mouse || keyboard;

    if let Some(win) = app.get_webview_window(WALLPAPER_WINDOW_LABEL) {
        if let Ok(raw) = win.hwnd() {
            apply_interactive_state(HWND(raw.0), mouse, keyboard);
        }
        // 通知壁纸窗口更新音频静音等前端侧状态
        let _ = app.emit_to(WALLPAPER_WINDOW_LABEL, "wallpaper:interaction", settings);
    }
    log::info!("[Wallpaper] HTML 交互设置已更新 鼠标={mouse} 键盘={keyboard} 音频={audio}");
    Ok(settings)
}

/// 壁纸窗口启动时读取 HTML 交互设置。
#[tauri::command]
pub fn get_html_interaction() -> HtmlInteraction {
    current_html_interaction()
}

/// 动态切换壁纸窗口的鼠标/键盘交互能力。
///
/// # 为什么不能只改 `WS_EX_TRANSPARENT`
///
/// 壁纸窗口是挂在 Explorer 壁纸层 `WorkerW` 下的 **`WS_CHILD` 子窗口**。
/// 子窗口的输入能力由三层决定，缺一层都收不到事件：
///
/// 1. **自身 `WS_EX_TRANSPARENT`** —— 置 true 时鼠标点击直接穿透到下层
///    （桌面图标），这是“不可交互”状态的实现方式；
/// 2. **祖先链的 `IsWindowEnabled`** —— Explorer 把壁纸层 `WorkerW` 创建为
///    **禁用状态**（实测 `IsWindowEnabled(layer) == false`）。子窗口会继承
///    祖先的禁用状态，此时即使清掉 `WS_EX_TRANSPARENT`，鼠标也进不来；
///    必须在交互期间 `EnableWindow(layer, true)`，退出交互时还原；
/// 3. **键盘焦点** —— `WS_EX_NOACTIVATE` 会让窗口永远拿不到焦点，
///    键盘事件无从谈起；且 `WS_CHILD` 窗口需要把焦点显式 `SetFocus`
///    到 WebView 内部窗口才有键盘输入。
///
/// 因此这里按 `mouse` / `keyboard` 两个开关分别处理，并在挂载时把
/// 当前状态重新应用一次（挂载会重建样式，之前设置的会被覆盖）。
#[tauri::command]
pub async fn set_wallpaper_interactive(
    app: tauri::AppHandle,
    mouse: Option<bool>,
    keyboard: Option<bool>,
) -> Result<(), String> {
    let mouse = mouse.unwrap_or_else(|| *lock(&INTERACTIVE_MOUSE));
    let keyboard = keyboard.unwrap_or_else(|| *lock(&INTERACTIVE_KEYBOARD));
    *lock(&INTERACTIVE_MOUSE) = mouse;
    *lock(&INTERACTIVE_KEYBOARD) = keyboard;
    // 兼容旧前端：任一开关为真即认为窗口整体可交互
    *lock(&INTERACTIVE) = mouse || keyboard;

    let Some(win) = app.get_webview_window(WALLPAPER_WINDOW_LABEL) else {
        return Ok(());
    };
    let raw = win.hwnd().map_err(|e| format!("获取窗口句柄失败: {e}"))?;
    apply_interactive_state(HWND(raw.0), mouse, keyboard);
    log::info!(
        "[Wallpaper] 交互状态 鼠标={} 键盘={}",
        if mouse { "开" } else { "关" },
        if keyboard { "开" } else { "关" }
    );
    Ok(())
}

/// 把鼠标/键盘交互状态实际应用到壁纸窗口（含祖先链修复与焦点处理）。
pub fn apply_interactive_state(hwnd: HWND, mouse: bool, keyboard: bool) {
    let interactive = mouse || keyboard;
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let ex = if mouse {
            // 允许鼠标：去掉穿透
            ex & !WS_EX_TRANSPARENT.0 as isize
        } else {
            ex | WS_EX_TRANSPARENT.0 as isize
        };
        let ex = if keyboard || mouse {
            // 需要接收输入就不能是 NOACTIVATE，否则永远无法获得焦点
            ex & !WS_EX_NOACTIVATE.0 as isize
        } else {
            ex | WS_EX_NOACTIVATE.0 as isize
        };
        // 始终不占任务栏/Alt+Tab
        let ex = ex | WS_EX_TOOLWINDOW.0 as isize;
        let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex);

        // --- 祖先链：壁纸层 WorkerW 默认是禁用的，必须临时启用 ---
        let parent = GetParent(hwnd).unwrap_or_default();
        if !parent.is_invalid() {
            if interactive {
                // 只启用真正阻断输入的祖先（实测壁纸层 WorkerW 为禁用）
                if !IsWindowEnabled(parent).as_bool() {
                    let _ = EnableWindow(parent, true);
                    *lock(&ENABLED_ANCESTOR) = Some(parent.0 as isize);
                    log::info!("[Wallpaper] 已临时启用壁纸层宿主窗口以接收输入: {parent:?}");
                }
            } else if let Some(saved) = lock(&ENABLED_ANCESTOR).take() {
                // 还原：Explorer 原本就是禁用的，退出交互后要恢复
                let saved = HWND(saved as *mut core::ffi::c_void);
                if !saved.is_invalid() && IsWindow(saved).as_bool() {
                    let _ = EnableWindow(saved, false);
                    log::info!("[Wallpaper] 已还原壁纸层宿主窗口的禁用状态: {saved:?}");
                }
            }
        }

        // --- z-order：交互时必须盖过桌面图标层，否则点击被图标层吃掉 ---
        //
        // 桌面图标层是**另一个** WorkerW 里的 SHELLDLL_DefView，它在 z-order 中
        // 位于壁纸层之上。只去掉 WS_EX_TRANSPARENT 并不够：鼠标落点先命中
        // SysListView32（图标层），壁纸窗口根本收不到事件（Win10 实测）。
        //
        // 交互期间把壁纸窗口提到图标层之上（HWND_TOP），退出交互时再压回
        // 图标层之下（恢复“壁纸在图标后面”的正常观感）。
        if interactive {
            let _ = SetWindowPos(
                hwnd,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            log::info!("[Wallpaper] 交互模式：壁纸窗口已提到图标层之上");
        } else {
            let _ = SetWindowPos(
                hwnd,
                HWND_BOTTOM,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            log::info!("[Wallpaper] 非交互模式：壁纸窗口已压回图标层之下");
        }

        // --- 键盘：把焦点交给 WebView 内部窗口 ---
        if keyboard {
            focus_webview_child(hwnd);
        }
    }
}

/// 把键盘焦点送到壁纸窗口内嵌的 WebView 子窗口。
///
/// Tauri/WRY 的窗口是 `WS_CHILD` 容器，真正接收键盘的是它下面的
/// `Chrome_WidgetWin_0/1` 或 `WRY_WEBVIEW` 子窗口，逐层找第一个可见子窗口聚焦。
unsafe fn focus_webview_child(hwnd: HWND) {
    // 子窗口无法自己成为前台，需要先让它的顶层祖先成为前台
    let root = GetAncestor(hwnd, GA_ROOT);
    if !root.is_invalid() {
        let _ = SetForegroundWindow(root);
    }
    let _ = SetActiveWindow(hwnd);

    let mut target = hwnd;
    // 最多下探数层，找到最内层可见子窗口
    for _ in 0..4 {
        let mut child = FindWindowExW(target, None, None, None);
        let mut found = HWND(std::ptr::null_mut());
        while let Ok(c) = child {
            if c.is_invalid() {
                break;
            }
            if IsWindowVisible(c).as_bool() && IsWindowEnabled(c).as_bool() {
                found = c;
            }
            child = FindWindowExW(target, c, None, None);
        }
        if found.is_invalid() {
            break;
        }
        target = found;
    }
    let _ = SetFocus(target);
    log::info!("[Wallpaper] 键盘焦点已交给窗口 {target:?}（顶层 {root:?}）");
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
///
/// 必须是 async 命令：同步命令由 Tauri 在主线程执行，递归 `walkdir` 扫描
/// 图库（几万条目、机械硬盘）会把主线程连同整个 UI 一起卡死，
/// 表现为「一打开壁纸页就转圈、连标题栏都不响应」。
/// 这里把真正的遍历丢进 `spawn_blocking`，主线程只等待结果。
#[tauri::command]
pub async fn scan_wallpaper_folder(
    dir: String,
    recursive: Option<bool>,
) -> Result<Vec<String>, String> {
    log::info!("[Wallpaper] scan 开始: dir={dir} recursive={recursive:?}");
    let out = tokio::task::spawn_blocking(move || scan_wallpaper_folder_sync(&dir, recursive.unwrap_or(false)))
        .await
        .map_err(|e| format!("扫描任务异常: {e}"))?;
    if let Ok(ref files) = out {
        log::info!("[Wallpaper] scan 完成: {} 个文件", files.len());
    }
    out
}

/// 同步扫描实现（只在阻塞线程池里跑，绝不直接暴露给前端调用）。
fn scan_wallpaper_folder_sync(dir: &str, recursive: bool) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let root = Path::new(dir);
    if !root.is_dir() {
        return Err("文件夹不存在".to_string());
    }

    let visit = |path: &Path, out: &mut Vec<String>| {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let ext_lower = ext.to_ascii_lowercase();
            if WALLPAPER_EXTS.contains(&ext_lower.as_str()) {
                if let Some(s) = path.to_str() {
                    out.push(s.to_string());
                }
            }
        }
    };

    if recursive {
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                visit(entry.path(), &mut out);
            }
        }
    } else {
        let entries = std::fs::read_dir(root).map_err(|e| format!("读取文件夹失败: {e}"))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                visit(&path, &mut out);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// 缩略图最长边（px）。网格里每格只有约 120–200 px 宽，
/// 生成 256 px 已足够清晰，同时把解码/传输量压到原图的 1/50 左右。
const WALLPAPER_THUMB_EDGE: u32 = 256;

/// 缩略图缓存目录名（位于 `app_data_dir` 下）
const WALLPAPER_THUMB_DIR: &str = "wallpaper_thumbnails";

/// 批量生成壁纸缩略图（带磁盘缓存）。
///
/// 为什么必须走缩略图：网格原来用 `convertFileSrc` 直接加载**原始图片**。
/// 4K 壁纸单张解码就要 30–60 ms、占几十 MB 内存，一屏几十张时会长时间卡住
/// 渲染线程 —— 这就是「打开壁纸管理页严重卡顿」的主因。
///
/// 返回 `原路径 -> 缩略图缓存绝对路径`；无法生成（视频/HTML/损坏文件）的项
/// 不在返回里，前端据此回退到图标占位。
#[tauri::command]
pub async fn get_wallpaper_thumbnails(
    app: tauri::AppHandle,
    paths: Vec<String>,
) -> Result<std::collections::HashMap<String, String>, String> {
    if paths.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let cache_dir = app
        .path()
        .app_data_dir()
        .map(|d| d.join(WALLPAPER_THUMB_DIR))
        .map_err(|e| format!("获取缓存目录失败: {e}"))?;
    log::info!(
        "[Wallpaper] 缩略图请求: {} 个路径, 缓存目录={}",
        paths.len(),
        cache_dir.display()
    );

    // 解码 + 缩放是纯 CPU 活，必须离开主线程；同时限制并发，
    // 避免几十张大图同时解码把内存和磁盘 IO 打满（低端/虚拟机环境更明显）。
    let count = paths.len();
    let out = tokio::task::spawn_blocking(move || build_wallpaper_thumbnails(&cache_dir, &paths))
        .await
        .map_err(|e| format!("缩略图任务异常: {e}"))?;
    if let Ok(ref map) = out {
        log::info!("[Wallpaper] 缩略图完成: {}/{} 成功", map.len(), count);
    }
    out
}

/// 多线程生成壁纸缩略图，返回成功项映射。
fn build_wallpaper_thumbnails(
    cache_dir: &Path,
    paths: &[String],
) -> Result<std::collections::HashMap<String, String>, String> {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    if let Err(e) = std::fs::create_dir_all(cache_dir) {
        return Err(format!("创建缩略图缓存目录失败: {e}"));
    }

    // 只处理静态图片；GIF/视频/HTML 由前端走各自的预览方式（视频用 Shell 缩略图）。
    let targets: Vec<(String, PathBuf)> = paths
        .iter()
        .filter(|p| {
            Path::new(p)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    let e = e.to_ascii_lowercase();
                    matches!(
                        e.as_str(),
                        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tiff" | "jfif"
                    )
                })
                .unwrap_or(false)
        })
        .map(|p| (p.clone(), wallpaper_thumb_cache_path(cache_dir, p)))
        .collect();

    if targets.is_empty() {
        return Ok(HashMap::new());
    }

    let all = Arc::new(targets);
    let index = Arc::new(AtomicUsize::new(0));
    let results: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));

    // 并发数取 CPU 一半（至少 2），留出余量给 UI 线程和文件缓存
    let workers = std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(2))
        .unwrap_or(2)
        .min(all.len())
        .min(8);

    std::thread::scope(|scope| {
        for _ in 0..workers {
            let all = Arc::clone(&all);
            let index = Arc::clone(&index);
            let results = Arc::clone(&results);
            scope.spawn(move || loop {
                let i = index.fetch_add(1, Ordering::SeqCst);
                let Some((src, cache)) = all.get(i) else {
                    break;
                };
                if let Some(thumb) = resolve_wallpaper_thumbnail(src, cache) {
                    if let Ok(mut guard) = results.lock() {
                        guard.insert(src.clone(), thumb);
                    }
                }
            });
        }
    });

    let out = results
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    Ok(out)
}

/// 缩略图缓存路径：按路径 + 大小 + 修改时间哈希命名。
///
/// 把 size/mtime 一并纳入哈希，图片被覆盖或编辑后缓存自动失效，
/// 避免像纯路径哈希那样一直显示旧图。
fn wallpaper_thumb_cache_path(cache_dir: &Path, src: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    src.to_ascii_lowercase().hash(&mut hasher);
    if let Ok(meta) = std::fs::metadata(src) {
        meta.len().hash(&mut hasher);
        if let Ok(modified) = meta.modified() {
            if let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH) {
                dur.as_secs().hash(&mut hasher);
            }
        }
    }
    cache_dir.join(format!("{:016x}.jpg", hasher.finish()))
}

/// 命中缓存直接返回；否则解码缩放后写入缓存。失败返回 None（前端回退图标）。
fn resolve_wallpaper_thumbnail(src: &str, cache: &Path) -> Option<String> {
    if cache.is_file() {
        return Some(cache.to_string_lossy().into_owned());
    }
    match generate_wallpaper_thumbnail(src, cache) {
        Ok(()) => Some(cache.to_string_lossy().into_owned()),
        Err(e) => {
            log::debug!("[Wallpaper] 缩略图生成失败 {src}: {e}");
            None
        }
    }
}

/// 解码图片 → 等比缩放到 `WALLPAPER_THUMB_EDGE` → 存 JPEG。
///
/// 用 JPEG 而不是 PNG：照片类壁纸 PNG 会大 5–10 倍，磁盘缓存和
/// asset 协议传输都会变慢，而缩略图场景 82% 质量肉眼无差别。
fn generate_wallpaper_thumbnail(src: &str, dst: &Path) -> Result<(), String> {
    // 按内容魔数探测格式，改名文件（.jpg 实为 PNG）也能正确解码
    let reader = image::ImageReader::open(src)
        .map_err(|e| format!("打开图片失败: {e}"))?
        .with_guessed_format()
        .map_err(|e| format!("识别图片格式失败: {e}"))?;
    let img = reader.decode().map_err(|e| format!("解码图片失败: {e}"))?;

    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Err("图片尺寸为 0".to_string());
    }
    // 等比缩放：先算目标边长，再按比例定另一边，避免拉伸变形
    let scaled = if w.max(h) <= WALLPAPER_THUMB_EDGE {
        img
    } else {
        let ratio = WALLPAPER_THUMB_EDGE as f32 / w.max(h) as f32;
        let nw = ((w as f32 * ratio).round() as u32).max(1);
        let nh = ((h as f32 * ratio).round() as u32).max(1);
        img.resize(nw, nh, image::imageops::FilterType::Triangle)
    };

    // 先写临时文件再 rename，避免进程被中断后留下半张损坏的缓存图
    let tmp = dst.with_extension("jpg.tmp");
    let file = std::fs::File::create(&tmp).map_err(|e| format!("创建缩略图失败: {e}"))?;
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 82);
    scaled
        .write_with_encoder(enc)
        .map_err(|e| format!("写入缩略图失败: {e}"))?;
    std::fs::rename(&tmp, dst).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("提交缩略图失败: {e}")
    })?;
    Ok(())
}
