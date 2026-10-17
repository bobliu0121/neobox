//! 壁纸管理：设置/读取系统壁纸、扫描壁纸文件夹、GIF 动态壁纸

use std::sync::Mutex;
use tauri::Manager;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GetClassNameW, GetSystemMetrics, GetWindowLongPtrW, GWL_EXSTYLE,
    HWND_BOTTOM, IsWindowVisible, SendMessageW, SetParent, SetWindowPos, SetWindowLongPtrW,
    SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};

/// 动态壁纸窗口的 label（全局唯一，用于关闭）
static GIF_WINDOW_LABEL: Mutex<Option<String>> = Mutex::new(None);

fn wide_str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 枚举顶级窗口，查找可见的 WorkerW（桌面壁纸层）
unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut class = [0u16; 256];
    let len = GetClassNameW(hwnd, &mut class);
    if len > 0 {
        let name = String::from_utf16_lossy(&class[..len as usize]);
        if name == "WorkerW" && IsWindowVisible(hwnd).as_bool() {
            let p = lparam.0 as *mut HWND;
            *p = hwnd;
            return BOOL(0);
        }
    }
    BOOL(1)
}

/// 关闭动态壁纸窗口（若存在）
fn stop_gif_wallpaper_inner(app: &tauri::AppHandle) {
    if let Some(label) = GIF_WINDOW_LABEL.lock().unwrap().take() {
        if let Some(win) = app.get_webview_window(&label) {
            let _ = win.close();
        }
    }
}

/// 停止动态壁纸（恢复为系统壁纸显示层）
#[tauri::command]
pub fn stop_gif_wallpaper(app: tauri::AppHandle) -> Result<(), String> {
    stop_gif_wallpaper_inner(&app);
    Ok(())
}

/// 设置 GIF 动态壁纸：创建透明置底 WebView 窗口播放 GIF，
/// 优先挂载到桌面壁纸层（WorkerW，图标之下），失败则置底显示
#[tauri::command]
pub fn set_gif_wallpaper(app: tauri::AppHandle, path: String) -> Result<(), String> {
    stop_gif_wallpaper_inner(&app);

    let label = "gif-wallpaper".to_string();
    let encoded = urlencoding::encode(&path);
    let asset_url = format!("asset://localhost/{encoded}");
    let html = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><style>html,body{{margin:0;padding:0;overflow:hidden;background:transparent}}img{{width:100vw;height:100vh;object-fit:cover;display:block}}</style></head><body><img src=\"{asset_url}\"></body></html>"
    );
    let b64 = base64::encode(html.as_bytes());
    let data_url = format!("data:text/html;base64,{b64}");
    let parsed = data_url
        .parse::<tauri::Url>()
        .map_err(|e| format!("无效 URL: {e}"))?;
    let (x, y, w, h) = unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let mut w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let mut h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if w <= 0 || h <= 0 {
            w = GetSystemMetrics(SM_CXSCREEN);
            h = GetSystemMetrics(SM_CYSCREEN);
        }
        (x, y, w, h)
    };

    let win = tauri::WebviewWindowBuilder::new(&app, &label, tauri::WebviewUrl::External(parsed))
        .title("")
        .transparent(true)
        .decorations(false)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .inner_size(w as f64, h as f64)
        .position(x as f64, y as f64)
        .build()
        .map_err(|e| format!("创建动态壁纸窗口失败: {e}"))?;

    let hwnd = HWND(win.hwnd().map_err(|e| format!("获取窗口句柄失败: {e}"))?.0);

    unsafe {
        // 点击穿透 + 不激活 + 不显示在任务栏
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as i32;
        let _ = SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            (ex | WS_EX_TRANSPARENT.0 as i32 | WS_EX_NOACTIVATE.0 as i32 | WS_EX_TOOLWINDOW.0 as i32)
                as isize,
        );
        // 优先挂载到桌面壁纸层（WorkerW）
        let progman = FindWindowW(PCWSTR(wide_str("Progman").as_ptr()), PCWSTR::null())
            .unwrap_or(HWND(std::ptr::null_mut()));
        let mut workerw: HWND = HWND(std::ptr::null_mut());
        if !progman.is_invalid() {
            let _ = SendMessageW(progman, 0x052C, WPARAM(0xD), LPARAM(0));
            let _ = EnumWindows(Some(enum_windows_proc), LPARAM(&mut workerw as *mut HWND as isize));
            if !workerw.is_invalid() {
                let _ = SetParent(hwnd, workerw);
            }
        }
        // 置底
        let _ = SetWindowPos(
            hwnd,
            HWND_BOTTOM,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }

    *GIF_WINDOW_LABEL.lock().unwrap() = Some(label);
    Ok(())
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

/// 支持的壁纸图片扩展名
const WALLPAPER_EXTS: [&str; 8] = ["png", "jpg", "jpeg", "webp", "bmp", "gif", "tiff", "jfif"];

/// 保存壁纸编辑结果（data_url 覆盖/另存为指定路径）
#[tauri::command]
pub fn save_wallpaper_edit(path: String, data_url: String) -> Result<(), String> {
    let b64 = data_url.split(',').nth(1).ok_or_else(|| "无效的图片数据".to_string())?;
    let bytes = base64::decode(b64.trim()).map_err(|e| format!("图片数据解码失败: {e}"))?;
    if let Some(parent) = std::path::Path::new(&path).parent() {
        if !parent.exists() {
            return Err("目标文件夹不存在".to_string());
        }
    }
    std::fs::write(&path, &bytes).map_err(|e| format!("写入文件失败: {e}"))?;
    Ok(())
}

/// 扫描壁纸文件夹，返回支持的图片文件绝对路径
#[tauri::command]
pub fn scan_wallpaper_folder(dir: String) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("读取文件夹失败: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let ext_lower = ext.to_lowercase();
            if WALLPAPER_EXTS.contains(&ext_lower.as_str()) {
                if let Some(s) = path.to_str() {
                    out.push(s.to_string());
                }
            }
        }
    }
    out.sort();
    Ok(out)
}
