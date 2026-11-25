//! Windows 文件关联注册（当前只注册 `.nbmusic` 的「打开方式」）。
//!
//! 便携版没有安装器，Tauri 的 `fileAssociations` 配置不会自动写入注册表。
//! 这里在每次启动时检查/刷新当前 exe 的 HKCU 关联信息，使用户能在
//! 资源管理器的「打开方式」里看到 NeoBox，并可选择把它设为默认程序。
//!
//! 只写 `OpenWithProgids` 和 `Applications\neobox.exe`，不抢改 `.nbmusic`
//! 的默认 ProgID；默认关联仍由用户在 Windows 中确认。

#[cfg(windows)]
pub fn ensure_nbmusic_open_with() {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let Ok(exe) = std::env::current_exe() else {
        log::warn!("[FileAssoc] 无法获取当前 exe 路径，跳过 .nbmusic 关联");
        return;
    };
    let exe_text = exe.to_string_lossy();
    let exe_name = exe
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("neobox.exe");
    let command = format!("\"{}\" \"%1\"", exe_text.replace('"', "\\\""));
    // `.nbmusic` 文件图标使用独立的 NBmusic 图标，而不是 NeoBox 软件图标。
    // 便携版没有安装器，这里把内嵌 ICO 写到 LocalAppData 后注册 DefaultIcon。
    let icon = nbmusic_file_icon()
        .unwrap_or_else(|| format!("\"{}\",0", exe_text.replace('"', "\\\"")));

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let prog_key_path = r"Software\Classes\NeoBox.nbmusic\shell\open\command";
    let app_key_path = format!(r"Software\Classes\Applications\{}\shell\open\command", exe_name);

    let current_prog = hkcu
        .open_subkey(prog_key_path)
        .and_then(|key| key.get_value::<String, _>(""))
        .ok();
    let current_app = hkcu
        .open_subkey(&app_key_path)
        .and_then(|key| key.get_value::<String, _>(""))
        .ok();
    let current_icon = hkcu
        .open_subkey(r"Software\Classes\NeoBox.nbmusic\DefaultIcon")
        .and_then(|key| key.get_value::<String, _>(""))
        .ok();
    let current_ext_icon = hkcu
        .open_subkey(r"Software\Classes\.nbmusic\DefaultIcon")
        .and_then(|key| key.get_value::<String, _>(""))
        .ok();
    if current_prog.as_deref() == Some(command.as_str())
        && current_app.as_deref() == Some(command.as_str())
        && current_icon.as_deref() == Some(icon.as_str())
        && current_ext_icon.as_deref() == Some(icon.as_str())
    {
        return;
    }

    if let Err(error) = register(&hkcu, exe_name, &command, &icon) {
        log::warn!("[FileAssoc] 注册 .nbmusic 打开方式失败: {error}");
        return;
    }
    log::info!("[FileAssoc] 已注册 .nbmusic 打开方式: {exe_text}");
    notify_shell_association_changed();
}

#[cfg(windows)]
fn nbmusic_file_icon() -> Option<String> {
    // 编译期嵌入专门用于 `.nbmusic` 文件的 ICO，避免和 NeoBox 应用图标混用。
    const ICON_BYTES: &[u8] = include_bytes!("../icons/nbmusic.ico");
    let dir = dirs::data_local_dir()?.join("NeoBox").join("icons");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        log::warn!("[FileAssoc] 创建文件图标缓存目录失败: {error}");
        return None;
    }
    let path = dir.join("nbmusic.ico");
    let intact = std::fs::metadata(&path)
        .map(|meta| meta.len() == ICON_BYTES.len() as u64)
        .unwrap_or(false);
    if !intact {
        if let Err(error) = std::fs::write(&path, ICON_BYTES) {
            log::warn!("[FileAssoc] 写入 .nbmusic 文件图标失败: {error}");
            return None;
        }
    }
    let path_text = path.to_string_lossy().replace('"', "\\\"");
    Some(format!("\"{path_text}\",0"))
}

#[cfg(windows)]
fn register(
    hkcu: &winreg::RegKey,
    exe_name: &str,
    command: &str,
    icon: &str,
) -> Result<(), String> {
    let classes = hkcu
        .create_subkey(r"Software\Classes")
        .map(|(key, _)| key)
        .map_err(|e| format!("打开 Software\\Classes 失败: {e}"))?;

    // .nbmusic 扩展信息：只加入 OpenWithProgids，不覆盖默认 ProgID。
    let (ext, _) = classes
        .create_subkey(".nbmusic")
        .map_err(|e| format!("创建 .nbmusic 键失败: {e}"))?;
    set_value(&ext, "Content Type", "audio/x-nbmusic")?;
    set_value(&ext, "PerceivedType", "audio")?;
    // 扩展级 DefaultIcon：即使尚未把 NeoBox 设为默认程序，.nbmusic 文件也显示
    // NBmusic 专属图标；不会改变默认打开程序。
    let (ext_icon, _) = ext
        .create_subkey("DefaultIcon")
        .map_err(|e| format!("创建 .nbmusic DefaultIcon 失败: {e}"))?;
    set_value(&ext_icon, "", icon)?;
    let (open_with, _) = ext
        .create_subkey("OpenWithProgids")
        .map_err(|e| format!("创建 OpenWithProgids 失败: {e}"))?;
    set_value(&open_with, "NeoBox.nbmusic", "")?;

    // ProgID：供用户在「打开方式」里选择 NeoBox，或手动设为默认。
    let (prog_id, _) = classes
        .create_subkey(r"NeoBox.nbmusic")
        .map_err(|e| format!("创建 NeoBox.nbmusic 失败: {e}"))?;
    set_value(&prog_id, "", "NeoBox NBMusic")?;
    set_value(&prog_id, "FriendlyTypeName", "NeoBox 音乐工程")?;
    let (prog_icon, _) = prog_id
        .create_subkey("DefaultIcon")
        .map_err(|e| format!("创建 ProgID DefaultIcon 失败: {e}"))?;
    set_value(&prog_icon, "", icon)?;
    let (prog_cmd, _) = prog_id
        .create_subkey(r"shell\open\command")
        .map_err(|e| format!("创建 ProgID command 失败: {e}"))?;
    set_value(&prog_cmd, "", command)?;

    // Applications\neobox.exe：让「打开方式」把当前便携 exe 识别为可用应用。
    let (app, _) = classes
        .create_subkey(format!(r"Applications\{exe_name}"))
        .map_err(|e| format!("创建 Applications\\{exe_name} 失败: {e}"))?;
    set_value(&app, "FriendlyAppName", "NeoBox")?;
    set_value(&app, "ApplicationCompany", "NeoBox")?;
    let (app_cmd, _) = app
        .create_subkey(r"shell\open\command")
        .map_err(|e| format!("创建 Applications command 失败: {e}"))?;
    set_value(&app_cmd, "", command)?;
    let (supported, _) = app
        .create_subkey("SupportedTypes")
        .map_err(|e| format!("创建 SupportedTypes 失败: {e}"))?;
    set_value(&supported, ".nbmusic", "")?;

    Ok(())
}

#[cfg(windows)]
fn set_value(key: &winreg::RegKey, name: &str, value: &str) -> Result<(), String> {
    key.set_value(name, &value)
        .map_err(|e| format!("写入注册表值 {name:?} 失败: {e}"))
}

#[cfg(windows)]
fn notify_shell_association_changed() {
    use windows_sys::Win32::UI::Shell::{
        SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST,
    };
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}
