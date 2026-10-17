use serde::{Deserialize, Serialize};
use std::process::Command;
use tauri::Manager;
use winreg::enums::*;
use winreg::RegKey;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[derive(Debug, Serialize, Deserialize)]
pub struct PawnIoStatus {
    /// 是否已安装
    pub installed: bool,
    /// 已安装版本（如 "2.2.0"）
    pub version: Option<String>,
    /// 安装路径
    pub install_path: Option<String>,
}

/// 检查 PawnIO 是否已安装（同 LHM PawnIo.IsInstalled 的注册表检测逻辑）
fn check_pawnio_installed() -> bool {
    let path = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO";

    // 尝试 64 位注册表视图
    if let Ok(key) = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY)
    {
        if key.get_value::<String, _>("DisplayVersion").is_ok() {
            return true;
        }
    }

    // 尝试 32 位注册表视图（WoW64）
    if let Ok(key) = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(path, KEY_READ | KEY_WOW64_32KEY)
    {
        if key.get_value::<String, _>("DisplayVersion").is_ok() {
            return true;
        }
    }

    false
}

/// 获取已安装的 PawnIO 版本和路径
fn get_pawnio_info() -> (Option<String>, Option<String>) {
    let path = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO";

    for flag in &[KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
        if let Ok(key) = RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey_with_flags(path, KEY_READ | *flag)
        {
            let ver = key.get_value::<String, _>("DisplayVersion").ok();
            let loc = key.get_value::<String, _>("InstallLocation").ok();
            if ver.is_some() {
                return (ver, loc);
            }
        }
    }

    (None, None)
}

/// 检查 PawnIO 驱动安装状态
#[tauri::command]
pub async fn check_pawnio_status() -> PawnIoStatus {
    let (version, install_path) = get_pawnio_info();
    PawnIoStatus {
        installed: version.is_some(),
        version,
        install_path,
    }
}

/// 从 Tauri 资源中查找 PawnIO_setup.exe（多路径兼容 dev / bundle / updater 模式）
fn locate_pawnio_setup(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("获取资源目录失败: {}", e))?;

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));

    let mut candidates: Vec<std::path::PathBuf> = vec![
        // 1. 打包模式：resources/binaries/ 被打包到同目录
        resource_dir.join("binaries").join("PawnIO_setup.exe"),
        // 2. dev 模式：src-tauri/resources/binaries/
        resource_dir.join("resources").join("binaries").join("PawnIO_setup.exe"),
    ];
    // 3. 与 exe 同级目录
    if let Some(ref d) = exe_dir {
        candidates.push(d.join("PawnIO_setup.exe"));
        // 4. Tauri updater _up_ 临时目录
        candidates.push(d.join("_up_").join("PawnIO_setup.exe"));
    }

    candidates
        .iter()
        .find(|p| p.exists())
        .cloned()
        .ok_or_else(|| {
            let paths: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();
            format!("未找到 PawnIO_setup.exe 资源文件，已尝试路径: {:?}", paths)
        })
}

/// 提取到临时目录后提权运行安装器并等待完成（同 LHM WaitForExit，关键！）
fn run_pawnio_setup(exe: &std::path::Path, flag: &str) -> Result<(), String> {
    // 提取到临时目录（同 LHM ExtractPawnIO）
    let temp_dir = std::env::temp_dir().join("NeoBox_PawnIO");
    let _ = std::fs::create_dir_all(&temp_dir);
    let temp_exe = temp_dir.join("PawnIO_setup.exe");

    std::fs::copy(exe, &temp_exe)
        .map_err(|e| format!("复制安装程序失败: {}", e))?;

    let ps_script = format!(
        "Start-Process -FilePath '{}' -ArgumentList '{}' -Verb RunAs -Wait -WindowStyle Hidden",
        temp_exe.to_string_lossy().replace('\'', "''"),
        flag
    );

    let output = Command::new("powershell")
        .args(["-WindowStyle", "Hidden", "-NoProfile", "-Command", &ps_script])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| format!("启动安装程序失败: {}", e))?;

    // 清理临时文件（同 LHM File.Delete）
    let _ = std::fs::remove_file(&temp_exe);
    let _ = std::fs::remove_dir(&temp_dir);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("操作失败: {}", stderr));
    }
    Ok(())
}

/// 安装 PawnIO 驱动（同 LHM InstallPawnIO 流程：提取 → 提权运行 -install 并等待完成）
#[tauri::command]
pub async fn install_pawnio_driver(app: tauri::AppHandle) -> Result<String, String> {
    // 如果已安装，直接返回
    if check_pawnio_installed() {
        return Ok("already_installed".to_string());
    }

    let resource_path = locate_pawnio_setup(&app)?;
    run_pawnio_setup(&resource_path, "-install")?;

    // 等待注册表写入并验证
    std::thread::sleep(std::time::Duration::from_secs(3));
    if check_pawnio_installed() {
        Ok("success".to_string())
    } else {
        Err("安装验证失败：注册表中未检测到 PawnIO 信息，请尝试手动安装".to_string())
    }
}

/// 卸载 PawnIO 驱动（提权运行 -uninstall 并等待完成，验证注册表移除）
#[tauri::command]
pub async fn uninstall_pawnio_driver(app: tauri::AppHandle) -> Result<String, String> {
    // 如果未安装，直接返回
    if !check_pawnio_installed() {
        return Ok("not_installed".to_string());
    }

    let resource_path = locate_pawnio_setup(&app)?;
    run_pawnio_setup(&resource_path, "-uninstall")?;

    // 等待注册表移除并验证
    std::thread::sleep(std::time::Duration::from_secs(3));
    if !check_pawnio_installed() {
        Ok("success".to_string())
    } else {
        Err("卸载验证失败：注册表中仍检测到 PawnIO 信息，请尝试手动卸载".to_string())
    }
}
