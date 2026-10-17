//! 壁纸管理：设置系统壁纸、扫描壁纸文件夹

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

/// 扫描壁纸文件夹，返回支持的图片文件绝对路径（png/jpg/jpeg/webp/bmp/gif）
#[tauri::command]
pub fn scan_wallpaper_folder(dir: String) -> Result<Vec<String>, String> {
    let exts = ["png", "jpg", "jpeg", "webp", "bmp", "gif"];
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("读取文件夹失败: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let ext_lower = ext.to_lowercase();
            if exts.contains(&ext_lower.as_str()) {
                if let Some(s) = path.to_str() {
                    out.push(s.to_string());
                }
            }
        }
    }
    out.sort();
    Ok(out)
}
