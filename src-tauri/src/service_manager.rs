use serde::{Deserialize, Serialize};

use windows_sys::Win32::Foundation::{GetLastError, ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA};
use windows_sys::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, EnumServicesStatusExW, ENUM_SERVICE_STATUS_PROCESSW,
    OpenSCManagerW, OpenServiceW, QueryServiceConfig2W, QueryServiceConfigW, QUERY_SERVICE_CONFIGW,
    SC_ENUM_PROCESS_INFO, SC_HANDLE, SC_MANAGER_CONNECT, SC_MANAGER_ENUMERATE_SERVICE,
    SERVICE_AUTO_START, SERVICE_CHANGE_CONFIG, SERVICE_CONFIG_DESCRIPTION, SERVICE_DESCRIPTIONW,
    SERVICE_DISABLED, SERVICE_NO_CHANGE, SERVICE_QUERY_CONFIG, SERVICE_RUNNING,
    SERVICE_WIN32, ENUM_SERVICE_STATE,
};

// ENUM_SERVICE_STATE_ALL = 0x3（运行中 + 已停止）
const ENUM_SERVICE_STATE_ALL: ENUM_SERVICE_STATE = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceItem {
    /// 服务键名（服务管理器中的名称）
    pub name: String,
    /// 显示名称
    pub display_name: String,
    /// 服务自带描述（SERVICE_CONFIG_DESCRIPTION）
    pub description: Option<String>,
    /// 启动类型是否已禁用（SERVICE_DISABLED）
    pub is_disabled: bool,
    /// 当前运行状态（running/stopped）
    pub is_running: bool,
    /// 服务可执行文件路径（QueryServiceConfig）
    pub binary_path: Option<String>,
}

/// 读取以 NUL 结尾的宽字符串；空指针或空串返回 None
unsafe fn read_wide(ptr: *const u16) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let mut buf: Vec<u16> = Vec::new();
    let mut i: usize = 0;
    loop {
        let ch = *ptr.add(i);
        if ch == 0 {
            break;
        }
        buf.push(ch);
        i += 1;
        if i > 65536 {
            return None;
        }
    }
    if buf.is_empty() {
        return None;
    }
    String::from_utf16(&buf).ok()
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 打开服务管理器（成功后调用方需 CloseServiceHandle）
unsafe fn open_scm(desired_access: u32) -> Result<SC_HANDLE, String> {
    let handle = OpenSCManagerW(std::ptr::null(), std::ptr::null(), desired_access);
    if handle.is_null() {
        return Err(format!("OpenSCManagerW failed (last_error={})", GetLastError()));
    }
    Ok(handle)
}

/// 查询单个服务的（启动类型, 二进制路径）
unsafe fn query_service_config(
    service: SC_HANDLE,
) -> (Option<u32>, Option<String>) {
    let mut buffer: Vec<u8> = vec![0u8; 8192];
    loop {
        let mut needed: u32 = 0;
        let ok = QueryServiceConfigW(
            service,
            buffer.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW,
            buffer.len() as u32,
            &mut needed,
        );
        if ok != 0 {
            let cfg = *(buffer.as_ptr() as *const QUERY_SERVICE_CONFIGW);
            let binary_path = read_wide(cfg.lpBinaryPathName);
            return (Some(cfg.dwStartType), binary_path);
        }
        if GetLastError() == ERROR_INSUFFICIENT_BUFFER && (needed as usize) > buffer.len() {
            buffer.resize(needed as usize, 0);
            continue;
        }
        return (None, None);
    }
}

/// 查询单个服务的描述
unsafe fn query_service_description(service: SC_HANDLE) -> Option<String> {
    let mut buffer: Vec<u8> = vec![0u8; 2048];
    loop {
        let mut needed: u32 = 0;
        let ok = QueryServiceConfig2W(
            service,
            SERVICE_CONFIG_DESCRIPTION,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            &mut needed,
        );
        if ok != 0 {
            let desc_struct = *(buffer.as_ptr() as *const SERVICE_DESCRIPTIONW);
            return read_wide(desc_struct.lpDescription);
        }
        if GetLastError() == ERROR_INSUFFICIENT_BUFFER && (needed as usize) > buffer.len() {
            buffer.resize(needed as usize, 0);
            continue;
        }
        return None;
    }
}

/// 扫描系统中「自动启动 + 已禁用」的 Windows 服务（含其自带描述）
#[tauri::command]
pub async fn scan_services() -> Result<Vec<ServiceItem>, String> {
    let scm = unsafe { open_scm(SC_MANAGER_ENUMERATE_SERVICE | SC_MANAGER_CONNECT) }?;

    let close_scm = |scm: SC_HANDLE| {
        unsafe {
            let _ = CloseServiceHandle(scm);
        }
    };

    // 枚举服务（运行中 + 已停止），类型过滤为 Win32 用户态服务
    let mut buffer: Vec<u8> = vec![0u8; 64 * 1024];
    let mut needed: u32 = 0;
    let mut returned: u32 = 0;
    let mut resume: u32 = 0;
    let mut items: Vec<ServiceItem> = Vec::new();

    loop {
        let ok = unsafe {
            EnumServicesStatusExW(
                scm,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                ENUM_SERVICE_STATE_ALL,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut needed,
                &mut returned,
                &mut resume,
                std::ptr::null(),
            )
        };
        if ok != 0 {
            break;
        }
        if unsafe { GetLastError() } == ERROR_MORE_DATA {
            let new_len = (needed as usize).max(buffer.len() * 2);
            buffer.resize(new_len, 0);
        } else {
            close_scm(scm);
            return Err(format!(
                "EnumServicesStatusExW failed (last_error={})",
                unsafe { GetLastError() }
            ));
        }
        if buffer.len() >= 4 * 1024 * 1024 {
            close_scm(scm);
            return Err("too many services".to_string());
        }
    }

    let count = returned as usize;
    let base = buffer.as_ptr();
    let entry_size = std::mem::size_of::<ENUM_SERVICE_STATUS_PROCESSW>();
    let mut offset: usize = 0;
    for _ in 0..count {
        if offset + entry_size > buffer.len() {
            break;
        }
        let entry = unsafe { (base.add(offset) as *const ENUM_SERVICE_STATUS_PROCESSW).read() };
        offset += entry_size;

        let name = match unsafe { read_wide(entry.lpServiceName) } {
            Some(n) if !n.is_empty() => n,
            _ => continue,
        };
        let display_name = unsafe { read_wide(entry.lpDisplayName) }.unwrap_or_default();
        let is_running = entry.ServiceStatusProcess.dwCurrentState == SERVICE_RUNNING;

        // 逐项打开并查询启动类型，仅保留「自动 + 已禁用」两类
        let name_wide = to_wide(&name);
        let service = unsafe { OpenServiceW(scm, name_wide.as_ptr(), SERVICE_QUERY_CONFIG) };
        if service.is_null() {
            continue;
        }

        let (start_type, binary_path) = unsafe { query_service_config(service) };
        let start_type = match start_type {
            Some(st) => st,
            None => {
                unsafe { let _ = CloseServiceHandle(service); }
                continue;
            }
        };

        if start_type == SERVICE_AUTO_START || start_type == SERVICE_DISABLED {
            let description = unsafe { query_service_description(service) };
            items.push(ServiceItem {
                name,
                display_name,
                description,
                is_disabled: start_type == SERVICE_DISABLED,
                is_running,
                binary_path,
            });
        }

        unsafe { let _ = CloseServiceHandle(service); }
    }

    close_scm(scm);
    Ok(items)
}

/// 修改服务启动类型：enable=true -> 自动启动；false -> 禁用。需管理员权限。
#[tauri::command]
pub async fn set_service_start_type(name: String, enable: bool) -> Result<bool, String> {
    if !crate::optimization::is_admin() {
        return Err("需要管理员权限".to_string());
    }

    let scm = unsafe { open_scm(SC_MANAGER_CONNECT) }?;
    let name_wide = to_wide(&name);
    let service = unsafe { OpenServiceW(scm, name_wide.as_ptr(), SERVICE_CHANGE_CONFIG) };
    if service.is_null() {
        unsafe { let _ = CloseServiceHandle(scm); }
        return Err(format!("OpenServiceW failed (last_error={})", unsafe { GetLastError() }));
    }

    let target: u32 = if enable { SERVICE_AUTO_START } else { SERVICE_DISABLED };
    let ok = unsafe {
        ChangeServiceConfigW(
            service,
            SERVICE_NO_CHANGE, // dwServiceType 保持
            target,
            SERVICE_NO_CHANGE, // dwErrorControl 保持
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };

    unsafe {
        let _ = CloseServiceHandle(service);
        let _ = CloseServiceHandle(scm);
    }

    if ok == 0 {
        Err(format!(
            "ChangeServiceConfigW failed (last_error={})",
            unsafe { GetLastError() }
        ))
    } else {
        Ok(true)
    }
}

/// 当前进程是否以管理员身份运行
#[tauri::command]
pub async fn is_app_admin() -> bool {
    crate::optimization::is_admin()
}