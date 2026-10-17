//! 壁纸管理：设置/读取系统壁纸、扫描壁纸文件夹

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
