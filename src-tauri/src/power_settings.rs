//! 处理器电源高级设置
//!
//! 通过 Windows Power API（powrprof.dll）检测/修改当前电源方案中的 11 项
//! 处理器高级设置（AC 插电 / DC 电池），并支持解除注册表隐藏（Attributes=0）。
//!
//! 设置数据（GUID、推荐值、默认值）以静态表形式作为唯一事实源，
//! 与 Winhance 开源项目的 Processor Power Management 模块保持一致。

use serde::Serialize;
use std::ptr;

use winreg::enums::*;
use winreg::RegKey;

use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::System::Power::{
    GetSystemPowerStatus, PowerGetActiveScheme, PowerReadACValueIndex,
    PowerReadDCValueIndex, PowerReadFriendlyName, PowerSetActiveScheme,
    PowerWriteACValueIndex, PowerWriteDCValueIndex, SYSTEM_POWER_STATUS,
};

/// 处理器子组 GUID（SUB_PROCESSOR）
const SUBGROUP_PROCESSOR: &str = "54533251-82be-4824-96c1-47b60b740d00";

/// 单项高级设置的静态定义（唯一事实源）
struct PowerSettingDef {
    id: &'static str,
    setting_guid: &'static str,
    /// 允许的最大取值（枚举选项的 0..=max 或百分比的 0..=100）
    max_value: u32,
    recommended_ac: u32,
    recommended_dc: u32,
    default_ac: u32,
    default_dc: u32,
    /// 默认是否被注册表隐藏（Attributes=1，需要在系统电源设置中显示时改为 0）
    hidden_by_default: bool,
}

/// 11 项处理器高级设置（全部位于 SUB_PROCESSOR 子组）
const SETTINGS: &[PowerSettingDef] = &[
    PowerSettingDef {
        id: "processor-min-state",
        setting_guid: "893dee8e-2bef-41e0-89c6-b55d0929964c",
        max_value: 100,
        recommended_ac: 100,
        recommended_dc: 5,
        default_ac: 0,
        default_dc: 5,
        hidden_by_default: false,
    },
    PowerSettingDef {
        id: "processor-max-state",
        setting_guid: "bc5038f7-23e0-4960-96da-33abaf5935ec",
        max_value: 100,
        recommended_ac: 100,
        recommended_dc: 100,
        default_ac: 100,
        default_dc: 100,
        hidden_by_default: false,
    },
    PowerSettingDef {
        id: "system-cooling-policy",
        setting_guid: "94d3a615-a899-4ac5-ae2b-e4d8f634367f",
        max_value: 1,
        recommended_ac: 1,
        recommended_dc: 1,
        default_ac: 1,
        default_dc: 0,
        hidden_by_default: true,
    },
    PowerSettingDef {
        id: "processor-performance-boost-mode",
        setting_guid: "be337238-0d82-4146-a960-4f3749d470c7",
        max_value: 6,
        recommended_ac: 2,
        recommended_dc: 1,
        default_ac: 2,
        default_dc: 2,
        hidden_by_default: true,
    },
    PowerSettingDef {
        id: "processor-performance-increase-policy",
        setting_guid: "465e1f50-b610-473a-ab58-00d1077dc418",
        max_value: 3,
        recommended_ac: 2,
        recommended_dc: 0,
        default_ac: 2,
        default_dc: 0,
        hidden_by_default: true,
    },
    PowerSettingDef {
        id: "processor-performance-decrease-policy",
        setting_guid: "40fbefc7-2e9d-4d25-a185-0cfd8574bac6",
        max_value: 2,
        recommended_ac: 1,
        recommended_dc: 2,
        default_ac: 1,
        default_dc: 0,
        hidden_by_default: true,
    },
    PowerSettingDef {
        id: "processor-core-parking-min-cores",
        setting_guid: "0cc5b647-c1df-4637-891a-dec35c318583",
        max_value: 100,
        recommended_ac: 0,
        recommended_dc: 0,
        default_ac: 100,
        default_dc: 10,
        hidden_by_default: true,
    },
    PowerSettingDef {
        id: "processor-core-parking-max-cores",
        setting_guid: "ea062031-0e34-4ff1-9b6d-eb1059334028",
        max_value: 100,
        recommended_ac: 100,
        recommended_dc: 100,
        default_ac: 100,
        default_dc: 100,
        hidden_by_default: true,
    },
    PowerSettingDef {
        id: "processor-energy-performance-preference",
        setting_guid: "36687f9e-e3a5-4dbf-b1dc-15eb381c6863",
        max_value: 100,
        recommended_ac: 0,
        recommended_dc: 50,
        default_ac: 25,
        default_dc: 50,
        hidden_by_default: true,
    },
    PowerSettingDef {
        id: "processor-performance-increase-threshold",
        setting_guid: "06cadf0e-64ed-448a-8927-ce7bf90eb35d",
        max_value: 100,
        recommended_ac: 10,
        recommended_dc: 30,
        default_ac: 30,
        default_dc: 90,
        hidden_by_default: true,
    },
    PowerSettingDef {
        id: "processor-performance-decrease-threshold",
        setting_guid: "12a0ab44-fe28-4fa9-b3bd-4b64f44960a6",
        max_value: 100,
        recommended_ac: 8,
        recommended_dc: 20,
        default_ac: 10,
        default_dc: 30,
        hidden_by_default: true,
    },
];

/// 单项设置的检测结果（前后端契约，serde camelCase）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerAdvancedSettingInfo {
    pub id: String,
    /// AC（插电）当前值；读取失败 / 设置不存在时为 None
    pub ac_value: Option<u32>,
    /// DC（电池）当前值；无电池 / 读取失败时为 None
    pub dc_value: Option<u32>,
    /// 是否被注册表隐藏（Attributes == 1）
    pub hidden: bool,
    /// 设置是否存在于当前电源方案
    pub supported: bool,
    /// "recommended" | "default" | "custom"
    pub state: String,
    pub recommended_ac: u32,
    pub recommended_dc: u32,
    pub default_ac: u32,
    pub default_dc: u32,
}

/// 检测结果整体响应
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerAdvancedSettingsResponse {
    pub scheme_guid: String,
    pub scheme_name: String,
    /// 是否存在电池（桌面机为 false，前端隐藏 DC 控件）
    pub has_battery: bool,
    pub settings: Vec<PowerAdvancedSettingInfo>,
}

/// 解除隐藏操作结果
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnhidePowerSettingsResult {
    pub success: bool,
    pub message: String,
    pub total: usize,
    pub updated: usize,
}

/// 将规范化的 GUID 字符串转换为 `windows::core::GUID`。
fn guid(s: &str) -> windows::core::GUID {
    windows::core::GUID::from(s)
}

/// 获取当前活动电源方案的 GUID（调用方负责使用后释放）。
fn get_active_scheme() -> Option<windows::core::GUID> {
    let mut ptr: *mut windows::core::GUID = ptr::null_mut();
    let err = unsafe { PowerGetActiveScheme(None, &mut ptr) };
    if err.0 != 0 || ptr.is_null() {
        return None;
    }
    let scheme = unsafe { *ptr };
    // PowerGetActiveScheme 分配的 GUID 需用 LocalFree 释放
    unsafe { LocalFree(HLOCAL(ptr.cast())) };
    Some(scheme)
}

/// 读取电源方案的友好名称（UTF-16 宽字符串）。
fn read_scheme_name(scheme: &windows::core::GUID) -> String {
    let mut buf = vec![0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    let err = unsafe {
        PowerReadFriendlyName(
            None,
            Some(scheme as *const windows::core::GUID),
            None,
            None,
            Some(buf.as_mut_ptr() as *mut u8),
            &mut size,
        )
    };
    if err.0 != 0 {
        return String::new();
    }
    let len = buf
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// 读取指定设置的 AC 值（PowerReadACValueIndex）。
fn read_ac_value(
    scheme: &windows::core::GUID,
    subgroup: &windows::core::GUID,
    setting: &windows::core::GUID,
) -> Option<u32> {
    let mut value: u32 = 0;
    let err = unsafe {
        PowerReadACValueIndex(
            None,
            Some(scheme as *const windows::core::GUID),
            Some(subgroup as *const windows::core::GUID),
            Some(setting as *const windows::core::GUID),
            &mut value,
        )
    };
    if err.0 != 0 {
        return None;
    }
    Some(value)
}

/// 读取指定设置的 DC 值（PowerReadDCValueIndex）。
fn read_dc_value(
    scheme: &windows::core::GUID,
    subgroup: &windows::core::GUID,
    setting: &windows::core::GUID,
) -> Option<u32> {
    let mut value: u32 = 0;
    let code = unsafe {
        PowerReadDCValueIndex(
            None,
            Some(scheme as *const windows::core::GUID),
            Some(subgroup as *const windows::core::GUID),
            Some(setting as *const windows::core::GUID),
            &mut value,
        )
    };
    if code != 0 {
        return None;
    }
    Some(value)
}

/// 读取设置的注册表 Attributes 值（1 = 隐藏）。
fn read_hidden_attr(subgroup: &windows::core::GUID, setting: &windows::core::GUID) -> bool {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let path = format!(
        r"SYSTEM\CurrentControlSet\Control\Power\PowerSettings\{:?}\{:?}",
        subgroup, setting
    );
    match hklm.open_subkey(path) {
        Ok(key) => match key.get_value::<u32, _>("Attributes") {
            Ok(v) => v == 1,
            Err(_) => false,
        },
        Err(_) => false,
    }
}

/// 计算设置状态：AC/DC 均等于推荐值 → recommended；均等于默认值 → default；否则 custom。
fn compute_state(ac: Option<u32>, dc: Option<u32>, def: &PowerSettingDef) -> &'static str {
    let rec_ok = if dc.is_none() {
        ac == Some(def.recommended_ac)
    } else {
        ac == Some(def.recommended_ac) && dc == Some(def.recommended_dc)
    };
    if rec_ok {
        return "recommended";
    }
    let def_ok = if dc.is_none() {
        ac == Some(def.default_ac)
    } else {
        ac == Some(def.default_ac) && dc == Some(def.default_dc)
    };
    if def_ok {
        return "default";
    }
    "custom"
}

/// 读取当前方案下全部设置的检测结果。
fn read_all_settings(scheme: &windows::core::GUID) -> Vec<PowerAdvancedSettingInfo> {
    let subgroup = guid(SUBGROUP_PROCESSOR);
    SETTINGS
        .iter()
        .map(|def| {
            let setting = guid(def.setting_guid);
            let ac_value = read_ac_value(scheme, &subgroup, &setting);
            let dc_value = read_dc_value(scheme, &subgroup, &setting);
            PowerAdvancedSettingInfo {
                id: def.id.to_string(),
                ac_value,
                dc_value,
                hidden: read_hidden_attr(&subgroup, &setting),
                supported: ac_value.is_some(),
                state: compute_state(ac_value, dc_value, def).to_string(),
                recommended_ac: def.recommended_ac,
                recommended_dc: def.recommended_dc,
                default_ac: def.default_ac,
                default_dc: def.default_dc,
            }
        })
        .collect()
}

/// 检测系统是否存在电池（桌面机无电池时前端隐藏 DC 控件）。
fn system_has_battery() -> bool {
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut status).is_ok() && status.BatteryFlag & 0x80 == 0 }
}

/// 组装完整响应。
fn build_response(scheme: &windows::core::GUID) -> PowerAdvancedSettingsResponse {
    PowerAdvancedSettingsResponse {
        scheme_guid: format!("{:?}", scheme),
        scheme_name: read_scheme_name(scheme),
        has_battery: system_has_battery(),
        settings: read_all_settings(scheme),
    }
}

/// 检测当前电源方案下全部处理器高级设置。
#[tauri::command]
pub async fn get_power_advanced_settings() -> Result<PowerAdvancedSettingsResponse, String> {
    if !cfg!(target_os = "windows") {
        return Err("此功能仅支持 Windows 系统".to_string());
    }
    let scheme = get_active_scheme().ok_or("获取当前电源方案失败，请重试")?;
    Ok(build_response(&scheme))
}

/// 修改单项设置（可同时写 AC 与 DC），写入后刷新电源方案并回读最新状态。
#[tauri::command]
pub async fn set_power_advanced_setting(
    id: String,
    ac_value: u32,
    dc_value: Option<u32>,
) -> Result<PowerAdvancedSettingsResponse, String> {
    if !cfg!(target_os = "windows") {
        return Err("此功能仅支持 Windows 系统".to_string());
    }
    let def = SETTINGS
        .iter()
        .find(|d| d.id == id)
        .ok_or_else(|| format!("未知的设置 ID: {}", id))?;

    if ac_value > def.max_value {
        return Err(format!("AC 值超出允许范围 (0-{})", def.max_value));
    }
    if let Some(dc) = dc_value {
        if dc > def.max_value {
            return Err(format!("DC 值超出允许范围 (0-{})", def.max_value));
        }
    }

    let scheme = get_active_scheme().ok_or("获取当前电源方案失败，请重试")?;
    let subgroup = guid(SUBGROUP_PROCESSOR);
    let setting = guid(def.setting_guid);

    let err = unsafe {
        PowerWriteACValueIndex(None, &scheme, Some(&subgroup), Some(&setting), ac_value)
    };
    if err.0 != 0 {
        return Err(format!("写入 AC 值失败（错误码 {}）", err.0));
    }

    if let Some(dc) = dc_value {
        let code = unsafe {
            PowerWriteDCValueIndex(None, &scheme, Some(&subgroup), Some(&setting), dc)
        };
        if code != 0 {
            return Err(format!("写入 DC 值失败（错误码 {}）", code));
        }
    }

    // 刷新使修改生效（PowerSetActiveScheme 立即应用新设置）
    let err = unsafe { PowerSetActiveScheme(None, Some(&scheme as *const windows::core::GUID)) };
    if err.0 != 0 {
        return Err(format!("刷新电源方案失败（错误码 {}）", err.0));
    }

    log::info!(
        "[PowerSettings] 已修改设置 {} (AC={}, DC={:?})",
        id,
        ac_value,
        dc_value
    );

    // 回读验证并返回最新状态
    Ok(build_response(&scheme))
}

/// 解除全部高级设置的注册表隐藏（写 Attributes=0），需管理员权限。
#[tauri::command]
pub async fn unhide_power_advanced_settings() -> Result<UnhidePowerSettingsResult, String> {
    if !cfg!(target_os = "windows") {
        return Err("此功能仅支持 Windows 系统".to_string());
    }
    let hidden_defs: Vec<&PowerSettingDef> =
        SETTINGS.iter().filter(|d| d.hidden_by_default).collect();
    let base = r"SYSTEM\CurrentControlSet\Control\Power\PowerSettings";
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let mut updated = 0usize;
    let mut errors = Vec::new();

    for def in &hidden_defs {
        let path = format!(r"{}\{}\{}", base, SUBGROUP_PROCESSOR, def.setting_guid);
        match hklm.create_subkey(&path) {
            Ok((key, _)) => match key.set_value("Attributes", &0u32) {
                Ok(_) => updated += 1,
                Err(e) => errors.push(format!("{}: {}", def.id, e)),
            },
            Err(e) => errors.push(format!("{}: {}", def.id, e)),
        }
    }

    if !errors.is_empty() {
        log::warn!("[PowerSettings] 部分设置解除隐藏失败: {}", errors.join("; "));
        return Ok(UnhidePowerSettingsResult {
            success: false,
            message: format!("部分设置解除隐藏失败: {}", errors.join("; ")),
            total: hidden_defs.len(),
            updated,
        });
    }

    log::info!("[PowerSettings] 已解除 {} 项高级设置的隐藏状态", updated);
    Ok(UnhidePowerSettingsResult {
        success: true,
        message: format!("已解除 {} 项高级设置的隐藏状态", updated),
        total: hidden_defs.len(),
        updated,
    })
}
