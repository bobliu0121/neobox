//! 系统字体枚举
//!
//! 使用 DirectWrite 枚举系统安装字体（家庭名），并检测是否支持中文
//! （判断第一个字体字形是否包含 CJK 字符「中」）。

use serde::Serialize;
use windows::core::PCWSTR;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteLocalizedStrings,
    DWRITE_FACTORY_TYPE_SHARED,
};

/// 单个系统字体信息
#[derive(Serialize)]
pub struct SystemFontInfo {
    /// 字体家庭名（优先 zh-cn，其次 en-us，否则取第一条）
    pub name: String,
    /// 是否支持中文字符（「中」0x4E2D）
    pub supports_chinese: bool,
}

/// 返回某个索引对应的本地化字符串
unsafe fn string_at(names: &IDWriteLocalizedStrings, index: u32) -> String {
    let Ok(len) = names.GetStringLength(index) else {
        return String::new();
    };
    if len == 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len as usize + 1];
    if names.GetString(index, &mut buf).is_err() {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len as usize])
        .trim()
        .to_string()
}

/// 按区域获取本地化家庭名
unsafe fn localized_name(names: &IDWriteLocalizedStrings, locale: &str) -> String {
    let locale_w: Vec<u16> = locale.encode_utf16().chain(std::iter::once(0)).collect();
    let mut index: u32 = 0;
    let mut exists = BOOL(0);
    if names
        .FindLocaleName(PCWSTR(locale_w.as_ptr()), &mut index, &mut exists)
        .is_ok()
        && exists.as_bool()
    {
        return string_at(names, index);
    }
    String::new()
}

/// 枚举系统字体
pub fn list_system_fonts() -> Result<Vec<SystemFontInfo>, String> {
    unsafe {
        let factory: IDWriteFactory =
            DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
                .map_err(|e| format!("DWriteCreateFactory failed: {e}"))?;

        let mut collection: Option<IDWriteFontCollection> = None;
        factory
            .GetSystemFontCollection(&mut collection, false)
            .map_err(|e| format!("GetSystemFontCollection failed: {e}"))?;
        let collection = collection.ok_or("IDWriteFontCollection is None")?;

        let family_count = collection.GetFontFamilyCount();
        let mut result = Vec::with_capacity(family_count as usize);

        for i in 0..family_count {
            let Ok(family) = collection.GetFontFamily(i) else {
                continue;
            };

            let Ok(names) = family.GetFamilyNames() else {
                continue;
            };

            let mut name = localized_name(&names, "zh-cn");
            if name.is_empty() {
                name = localized_name(&names, "en-us");
            }
            if name.is_empty() && names.GetCount() > 0 {
                name = string_at(&names, 0);
            }
            if name.trim().is_empty() {
                continue;
            }

            // 用第一个字体检测是否包含「中」(U+4E2D)，代表支持中文
            let mut supports_chinese = false;
            if family.GetFontCount() > 0 {
                if let Ok(font) = family.GetFont(0) {
                    supports_chinese = font
                        .HasCharacter(0x4E2D)
                        .map(|b| b.as_bool())
                        .unwrap_or(false);
                }
            }

            result.push(SystemFontInfo {
                name,
                supports_chinese,
            });
        }

        Ok(result)
    }
}

/// Tauri 命令：获取系统字体列表
#[tauri::command]
pub fn get_system_fonts() -> Result<Vec<SystemFontInfo>, String> {
    list_system_fonts()
}