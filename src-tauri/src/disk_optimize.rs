use serde::Serialize;
use std::process::Command;

use crate::optimization;

/// defrag.exe 输出使用系统本地代码页（中文为 GBK），需正确解码避免乱码。
fn decode_console(bytes: &[u8]) -> String {
    let (cow, _, _) = encoding_rs::GBK.decode(bytes);
    cow.into_owned()
}

/// 判断是否为 SSD/固态介质，决定走 TRIM/优化而非传统碎片整理。
/// 不信任前端传来的 MediaType 字符串（NVMe 盘的 WMI MediaType 常为空），
/// 而是通过 SMART 直读做权威判定：能读到 NVMe SMART → 必定是 SSD。
/// 若无法判定（无法读 SMART），返回 None，由 defrag.exe /O 让系统自动选择。
fn detect_ssd(index: u32, interface_type: &str, model: &str) -> Option<bool> {
    let it = interface_type.to_lowercase();
    // 接口类型明确为 NVMe → 必定是 SSD
    if it.contains("nvme") || model.to_lowercase().contains("nvme") {
        return Some(true);
    }

    // 通过 SMART 直读确认是否 NVMe（NVMe 盘接口常显示为 SCSI，需 SMART 判定）
    let smart = crate::smart::read_disk_smart(index, true, false, model, "");
    if smart.is_nvme {
        return Some(true);
    }
    // 能读到 ATA SMART 但非 NVMe → 可能是 HDD 或 SATA SSD，交由系统自动判断
    if smart.has_smart {
        return None;
    }
    // 连 SMART 都读不到，交由系统自动判断
    None
}

/// operation → defrag.exe 参数
fn defrag_flag(operation: &str) -> &'static str {
    match operation {
        "retrim" => "/L",
        "defrag" => "/U",
        _ => "/O",
    }
}

#[derive(Serialize)]
pub struct DiskOptimizeResult {
    pub drive_letter: String,
    /// defrag=传统碎片整理(机械盘), retrim=TRIM/优化(固态盘), auto=由系统自动判断
    pub operation: String,
    pub is_ssd: bool,
    /// true = 已在后台启动、立即返回（机械盘整理可能耗时数小时）
    pub background: bool,
    pub success: bool,
    pub message: String,
}

/// 前台同步执行（retrim/TRIM 等快速操作），等待完成后返回输出。
fn run_defrag_sync(drive_letter: &str, operation: &str) -> Result<String, String> {
    let target = format!("{}:", drive_letter);
    let mut cmd = Command::new("defrag.exe");
    cmd.args([&target, defrag_flag(operation)]);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = cmd
        .output()
        .map_err(|e| format!("执行整理命令失败: {}", e))?;
    let stdout = decode_console(&output.stdout);
    let stderr = decode_console(&output.stderr);

    if output.status.success() {
        // 取输出的末尾几行作为结果反馈
        let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
        let tail = lines
            .iter()
            .rev()
            .take(4)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        Ok(if tail.trim().is_empty() {
            "操作完成".to_string()
        } else {
            tail
        })
    } else {
        Err(if stderr.trim().is_empty() { stdout } else { stderr })
    }
}

/// 后台低优先级启动（defrag/auto，可能耗时数小时）。
/// 前台全速整理会占满磁盘 I/O 导致整机卡死，这里参考 Windows"优化驱动器"
/// 计划任务的调度方式：将 defrag 进程降为 BELOW_NORMAL 优先级后台运行，
/// 仅在系统空闲时占用资源，随后立即返回，不阻塞 UI。
fn run_defrag_background(drive_letter: &str, operation: &str) -> Result<String, String> {
    let target = format!("{}:", drive_letter);
    let mut cmd = Command::new("defrag.exe");
    cmd.args([&target, defrag_flag(operation)]);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let child = cmd
        .spawn()
        .map_err(|e| format!("启动整理进程失败: {}", e))?;

    // 立即将 defrag 进程降为 BELOW_NORMAL 优先级，避免占满磁盘 I/O 卡死系统
    #[cfg(windows)]
    unsafe {
        use winapi::um::handleapi::CloseHandle;
        use winapi::um::processthreadsapi::{OpenProcess, SetPriorityClass};
        use winapi::um::winbase::BELOW_NORMAL_PRIORITY_CLASS;
        use winapi::um::winnt::{PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION};

        let h = OpenProcess(
            PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            child.id(),
        );
        if h.is_null() {
            log::warn!(
                "[Defrag] 设置进程优先级失败: {}",
                std::io::Error::last_os_error()
            );
        } else {
            SetPriorityClass(h, BELOW_NORMAL_PRIORITY_CLASS);
            CloseHandle(h);
        }
    }

    // 不等待 defrag 退出（drop Child 不会终止子进程），让其继续在后台低优先级运行
    let _ = child;
    Ok("已在后台开始整理，可继续正常使用电脑".to_string())
}

/// 对指定盘符执行磁盘整理/优化。
/// - SSD 固态盘：TRIM/优化 (Optimize-Volume -ReTrim)，很快 → 前台同步等待
/// - HDD 机械盘：传统碎片整理 (Optimize-Volume -Defrag)，耗时可能数小时 → 后台低优先级运行
/// - 无法判定介质：交给 defrag.exe /O 让系统自动选择 → 后台低优先级运行
#[tauri::command]
pub async fn optimize_disk(
    drive_letter: String,
    index: u32,
    interface_type: String,
    model: String,
) -> Result<DiskOptimizeResult, String> {
    if !optimization::is_admin() {
        return Err("此操作需要管理员权限，请以管理员身份运行 NeoBox 后重试".to_string());
    }

    let letter = drive_letter.trim().trim_end_matches(':').to_string();
    if letter.is_empty() {
        return Err("无效的盘符".to_string());
    }

    // 通过 SMART 直读判定介质（不信任前端 MediaType 字符串，NVMe 盘常为空导致误判）
    let is_ssd = detect_ssd(index, &interface_type, &model).unwrap_or(false);
    let operation = if is_ssd {
        "retrim"
    } else {
        // 机械盘或无法判定 → 交给系统自动选择（/O 会针对 SSD 自动做 TRIM）
        "auto"
    };
    // 只有 SSD 的 TRIM 是快速操作 → 前台等待；其余（含 /O 自动）可能耗时数小时 → 后台低优先级运行
    let background = operation != "retrim";

    let task_letter = letter.clone();
    let task_operation = operation.to_string();
    let task_is_ssd = is_ssd;
    let run_letter = task_letter.clone();
    let run_op = task_operation.clone();

    let run = tauri::async_runtime::spawn_blocking(move || {
        if background {
            run_defrag_background(&run_letter, &run_op)
        } else {
            run_defrag_sync(&run_letter, &run_op)
        }
    })
    .await
    .map_err(|e| format!("异步任务失败: {}", e))?;

    match run {
        Ok(msg) => Ok(DiskOptimizeResult {
            drive_letter: task_letter,
            operation: task_operation,
            is_ssd: task_is_ssd,
            background,
            success: true,
            message: msg,
        }),
        Err(e) => Err(e),
    }
}
