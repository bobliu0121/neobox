use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

/// 全部热键总开关（默认开启）
static HOTKEYS_ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_hotkeys_enabled(enabled: bool) {
    HOTKEYS_ENABLED.store(enabled, Ordering::SeqCst);
}

pub fn is_hotkeys_enabled() -> bool {
    HOTKEYS_ENABLED.load(Ordering::SeqCst)
}

// ==================== 每个热键的独立开关 ====================
// 单热键实际生效 = 总开关 && 该热键独立开关。
// 默认开启；连点器热键默认为关闭（需用户手动开启）。

macro_rules! define_hotkey_enabled {
    ($static_name:ident, $get_name:ident, $set_name:ident, $default:expr) => {
        static $static_name: AtomicBool = AtomicBool::new($default);
        pub fn $get_name() -> bool {
            $static_name.load(Ordering::SeqCst)
        }
        pub fn $set_name(enabled: bool) {
            $static_name.store(enabled, Ordering::SeqCst);
        }
    };
}

define_hotkey_enabled!(OVERLAY_ENABLED, is_overlay_enabled, set_overlay_enabled, true);
define_hotkey_enabled!(AUTOCLICKER_ENABLED, is_autoclicker_enabled, set_autoclicker_enabled, false);
define_hotkey_enabled!(MUSIC_PREV_ENABLED, is_music_prev_enabled, set_music_prev_enabled, true);
define_hotkey_enabled!(MUSIC_NEXT_ENABLED, is_music_next_enabled, set_music_next_enabled, true);
define_hotkey_enabled!(MUSIC_PLAYPAUSE_ENABLED, is_music_playpause_enabled, set_music_playpause_enabled, true);
define_hotkey_enabled!(LYRIC_BTN_ENABLED, is_lyric_btn_enabled, set_lyric_btn_enabled, true);

/// 应用单个热键的注册/注销（以总开关与独立开关共同决定生效与否）。
/// - shortcut 为空直接返回。
/// - 生效 = 总开关开启 && feature_enabled。
/// - 键盘热键走 global_shortcut；鼠标键（autoclicker 的 Mouse 前缀）走低级轮询线程。
pub fn apply_single_hotkey(app_handle: &tauri::AppHandle, shortcut: &str, feature_enabled: bool) {
    if shortcut.is_empty() {
        return;
    }
    let effective = is_hotkeys_enabled() && feature_enabled;

    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;

        if is_mouse_key(shortcut) {
            crate::autoclicker::set_mouse_hotkey(
                app_handle,
                if effective { Some(shortcut) } else { None },
            );
        } else if effective {
            let _ = app_handle.global_shortcut().register(shortcut);
        } else {
            let _ = app_handle.global_shortcut().unregister(shortcut);
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app_handle, shortcut, feature_enabled);
    }
}

/// 根据总开关状态实际注册或注销所有全局热键。
/// 仅切换开关标志并不会释放被操作系统拦截的按键：例如单独绑定 "P" 后，
/// RegisterHotKey 仍会在系统层面全局拦截 P 键，导致总开关关闭后依然无法打字。
/// 因此开关关闭时必须注销热键释放按键，开启时重新注册（仅注册独立开关为开的热键）。
pub fn apply_hotkeys_enabled(app_handle: &tauri::AppHandle, enabled: bool) {
    if enabled {
        // 总开关开启：逐个按独立开关处理（help 内部以总开关 = true）。
        apply_single_hotkey(app_handle, &get_overlay_shortcut(), is_overlay_enabled());
        apply_single_hotkey(app_handle, &get_autoclicker_shortcut(), is_autoclicker_enabled());
        apply_single_hotkey(app_handle, &get_music_prev_shortcut(), is_music_prev_enabled());
        apply_single_hotkey(app_handle, &get_music_next_shortcut(), is_music_next_enabled());
        apply_single_hotkey(app_handle, &get_music_playpause_shortcut(), is_music_playpause_enabled());
        apply_single_hotkey(app_handle, &get_lyrics_btn_toggle_shortcut(), is_lyric_btn_enabled());
    } else {
        // 总开关关闭：注销全部快捷键（含鼠标键）。
        #[cfg(target_os = "windows")]
        {
            use tauri_plugin_global_shortcut::GlobalShortcutExt;

            let shortcuts = [
                get_overlay_shortcut(),
                get_autoclicker_shortcut(),
                get_music_prev_shortcut(),
                get_music_next_shortcut(),
                get_music_playpause_shortcut(),
                get_lyrics_btn_toggle_shortcut(),
            ];
            for s in shortcuts {
                if s.is_empty() {
                    continue;
                }
                if is_mouse_key(&s) {
                    crate::autoclicker::set_mouse_hotkey(app_handle, None);
                } else {
                    let _ = app_handle.global_shortcut().unregister(s.as_str());
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = app_handle;
        }
    }
}

static OVERLAY_SHORTCUT: Mutex<Option<String>> = Mutex::new(None);
static OVERLAY_SHORTCUT_ID: AtomicU32 = AtomicU32::new(0);


static AUTOCLICKER_SHORTCUT: Mutex<Option<String>> = Mutex::new(None);
static AUTOCLICKER_SHORTCUT_ID: AtomicU32 = AtomicU32::new(0);

static MUSIC_PREV_SHORTCUT: Mutex<Option<String>> = Mutex::new(None);
static MUSIC_PREV_SHORTCUT_ID: AtomicU32 = AtomicU32::new(0);

static MUSIC_NEXT_SHORTCUT: Mutex<Option<String>> = Mutex::new(None);
static MUSIC_NEXT_SHORTCUT_ID: AtomicU32 = AtomicU32::new(0);

static MUSIC_PLAYPAUSE_SHORTCUT: Mutex<Option<String>> = Mutex::new(None);
static MUSIC_PLAYPAUSE_SHORTCUT_ID: AtomicU32 = AtomicU32::new(0);

static LYRIC_BTN_TOGGLE_SHORTCUT: Mutex<Option<String>> = Mutex::new(None);
static LYRIC_BTN_TOGGLE_SHORTCUT_ID: AtomicU32 = AtomicU32::new(0);

pub fn init_overlay(app_handle: &tauri::AppHandle, shortcut: &str) -> Result<(), String> {
    set_overlay_shortcut(shortcut);

    // 独立开关关闭时不注册（即使总开关开启也不生效）
    if !is_overlay_enabled() {
        log::info!("悬浮框热键独立开关关闭，跳过注册: {}", shortcut);
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        app_handle
            .global_shortcut()
            .register(shortcut)
            .map_err(|e| format!("注册悬浮框热键失败: {}", e))?;
    }

    log::info!("悬浮框热键已注册: {}", shortcut);
    Ok(())
}

pub fn update_overlay(app_handle: &tauri::AppHandle, new_shortcut: &str) -> Result<(), String> {
    let old_shortcut = get_overlay_shortcut();

    // 新旧热键相同，无需更新
    if old_shortcut == new_shortcut {
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;

        if !old_shortcut.is_empty() {
            let _ = app_handle.global_shortcut().unregister(old_shortcut.as_str());
        }

        if !new_shortcut.is_empty() {
            if let Err(e) = app_handle.global_shortcut().register(new_shortcut) {
                // 注册失败：回滚旧热键，保证状态一致，避免热键静默失效
                if !old_shortcut.is_empty() {
                    let _ = app_handle.global_shortcut().register(old_shortcut.as_str());
                }
                return Err(hotkey_register_error("注册悬浮框热键", new_shortcut, e));
            }
        }
    }

    set_overlay_shortcut(new_shortcut);
    log::info!("悬浮框热键已更新: {} -> {}", old_shortcut, new_shortcut);
    Ok(())
}

pub fn get_overlay_shortcut() -> String {
    OVERLAY_SHORTCUT
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_default()
}

pub fn get_overlay_shortcut_id() -> u32 {
    OVERLAY_SHORTCUT_ID.load(Ordering::SeqCst)
}

fn set_overlay_shortcut(shortcut: &str) {
    *OVERLAY_SHORTCUT.lock().unwrap() = Some(shortcut.to_string());
    if let Ok(hotkey) = tauri_plugin_global_shortcut::Shortcut::from_str(shortcut) {
        OVERLAY_SHORTCUT_ID.store(hotkey.id(), Ordering::SeqCst);
    }
}


/// 鼠标键热键（tauri 的 global-shortcut 不支持，需用低级鼠标钩子处理）
fn is_mouse_key(shortcut: &str) -> bool {
    shortcut.starts_with("Mouse")
}

/// 将全局快捷键注册失败转换为具体的中文错误提示
fn hotkey_register_error(action: &str, shortcut: &str, err: impl std::fmt::Display) -> String {
    // 鼠标键走低级钩子，不在此校验
    if !is_mouse_key(shortcut) {
        match tauri_plugin_global_shortcut::Shortcut::from_str(shortcut) {
            Ok(hotkey) => {
                // Windows 的 RegisterHotKey 要求带修饰键，否则注册必然失败
                if hotkey.mods.is_empty() {
                    return format!(
                        "{}失败：快捷键必须包含 Ctrl、Alt、Shift 或 Win 修饰键，请重新录制",
                        action
                    );
                }
            }
            Err(_) => {
                return format!("{}失败：快捷键格式无效，请重新录制", action);
            }
        }
    }

    let msg = err.to_string();
    let lower = msg.to_lowercase();
    if lower.contains("already registered") || lower.contains("in use") {
        format!("{}失败：该快捷键已被其他程序占用，请更换组合键", action)
    } else {
        format!("{}失败：{}", action, msg)
    }
}

pub fn init_autoclicker(app_handle: &tauri::AppHandle, shortcut: &str) -> Result<(), String> {
    set_autoclicker_shortcut(shortcut);

    // 独立开关关闭时不注册（即使总开关开启也不生效）
    if !is_autoclicker_enabled() {
        log::info!("连点器热键独立开关关闭，跳过注册: {}", shortcut);
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;

        if is_mouse_key(shortcut) {
            crate::autoclicker::set_mouse_hotkey(app_handle, Some(shortcut));
        } else {
            app_handle
                .global_shortcut()
                .register(shortcut)
                .map_err(|e| format!("注册连点器热键失败: {}", e))?;
        }
    }

    log::info!("连点器热键已注册: {}", shortcut);
    Ok(())
}

pub fn update_autoclicker(app_handle: &tauri::AppHandle, new_shortcut: &str) -> Result<(), String> {
    let old_shortcut = get_autoclicker_shortcut();

    // 新旧热键相同，无需更新
    if old_shortcut == new_shortcut {
        return Ok(());
    }

    let new_is_mouse = is_mouse_key(new_shortcut);

    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;

        // 旧热键若是键盘快捷键，先注销
        if !old_shortcut.is_empty() && !is_mouse_key(&old_shortcut) {
            let _ = app_handle.global_shortcut().unregister(old_shortcut.as_str());
        }

        // 鼠标键走低级钩子，键盘键走全局快捷键
        if new_is_mouse {
            crate::autoclicker::set_mouse_hotkey(app_handle, Some(new_shortcut));
        } else {
            crate::autoclicker::set_mouse_hotkey(app_handle, None);
            if !new_shortcut.is_empty() {
                if let Err(e) = app_handle.global_shortcut().register(new_shortcut) {
                    // 注册失败：回滚旧热键，保证状态一致
                    if !old_shortcut.is_empty() {
                        if is_mouse_key(&old_shortcut) {
                            crate::autoclicker::set_mouse_hotkey(app_handle, Some(&old_shortcut));
                        } else {
                            let _ = app_handle.global_shortcut().register(old_shortcut.as_str());
                        }
                    }
                    return Err(hotkey_register_error("注册连点器热键", new_shortcut, e));
                }
            }
        }
    }

    set_autoclicker_shortcut(new_shortcut);
    log::info!("连点器热键已更新: {} -> {}", old_shortcut, new_shortcut);
    Ok(())
}

pub fn get_autoclicker_shortcut() -> String {
    AUTOCLICKER_SHORTCUT
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_default()
}

pub fn get_autoclicker_shortcut_id() -> u32 {
    AUTOCLICKER_SHORTCUT_ID.load(Ordering::SeqCst)
}

fn set_autoclicker_shortcut(shortcut: &str) {
    *AUTOCLICKER_SHORTCUT.lock().unwrap() = Some(shortcut.to_string());
    if let Ok(hotkey) = tauri_plugin_global_shortcut::Shortcut::from_str(shortcut) {
        AUTOCLICKER_SHORTCUT_ID.store(hotkey.id(), Ordering::SeqCst);
    }
}

// ==================== 音乐控制热键（上一曲/下一曲/播放暂停） ====================

/// 向主窗口发送音乐控制事件，触发前端对应动作
fn emit_music_action(app_handle: &tauri::AppHandle, action: &str) {
    use tauri::Emitter;
    let _ = app_handle.emit("music-hotkey", serde_json::json!({ "action": action }));
}

pub fn init_music_prev(app_handle: &tauri::AppHandle, shortcut: &str) -> Result<(), String> {
    set_music_prev_shortcut(shortcut);

    // 独立开关关闭时不注册（即使总开关开启也不生效）
    if !is_music_prev_enabled() {
        log::info!("上一曲热键独立开关关闭，跳过注册: {}", shortcut);
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        app_handle
            .global_shortcut()
            .register(shortcut)
            .map_err(|e| format!("注册上一曲热键失败: {}", e))?;
    }

    log::info!("上一曲热键已注册: {}", shortcut);
    Ok(())
}

pub fn update_music_prev(app_handle: &tauri::AppHandle, new_shortcut: &str) -> Result<(), String> {
    let old_shortcut = get_music_prev_shortcut();

    if old_shortcut == new_shortcut {
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;

        if !old_shortcut.is_empty() {
            let _ = app_handle.global_shortcut().unregister(old_shortcut.as_str());
        }

        if !new_shortcut.is_empty() {
            if let Err(e) = app_handle.global_shortcut().register(new_shortcut) {
                if !old_shortcut.is_empty() {
                    let _ = app_handle.global_shortcut().register(old_shortcut.as_str());
                }
                return Err(hotkey_register_error("注册上一曲热键", new_shortcut, e));
            }
        }
    }

    set_music_prev_shortcut(new_shortcut);
    log::info!("上一曲热键已更新: {} -> {}", old_shortcut, new_shortcut);
    Ok(())
}

pub fn get_music_prev_shortcut() -> String {
    MUSIC_PREV_SHORTCUT
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_default()
}

pub fn get_music_prev_shortcut_id() -> u32 {
    MUSIC_PREV_SHORTCUT_ID.load(Ordering::SeqCst)
}

fn set_music_prev_shortcut(shortcut: &str) {
    *MUSIC_PREV_SHORTCUT.lock().unwrap() = Some(shortcut.to_string());
    if let Ok(hotkey) = tauri_plugin_global_shortcut::Shortcut::from_str(shortcut) {
        MUSIC_PREV_SHORTCUT_ID.store(hotkey.id(), Ordering::SeqCst);
    }
}

pub fn init_music_next(app_handle: &tauri::AppHandle, shortcut: &str) -> Result<(), String> {
    set_music_next_shortcut(shortcut);

    // 独立开关关闭时不注册（即使总开关开启也不生效）
    if !is_music_next_enabled() {
        log::info!("下一曲热键独立开关关闭，跳过注册: {}", shortcut);
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        app_handle
            .global_shortcut()
            .register(shortcut)
            .map_err(|e| format!("注册下一曲热键失败: {}", e))?;
    }

    log::info!("下一曲热键已注册: {}", shortcut);
    Ok(())
}

pub fn update_music_next(app_handle: &tauri::AppHandle, new_shortcut: &str) -> Result<(), String> {
    let old_shortcut = get_music_next_shortcut();

    if old_shortcut == new_shortcut {
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;

        if !old_shortcut.is_empty() {
            let _ = app_handle.global_shortcut().unregister(old_shortcut.as_str());
        }

        if !new_shortcut.is_empty() {
            if let Err(e) = app_handle.global_shortcut().register(new_shortcut) {
                if !old_shortcut.is_empty() {
                    let _ = app_handle.global_shortcut().register(old_shortcut.as_str());
                }
                return Err(hotkey_register_error("注册下一曲热键", new_shortcut, e));
            }
        }
    }

    set_music_next_shortcut(new_shortcut);
    log::info!("下一曲热键已更新: {} -> {}", old_shortcut, new_shortcut);
    Ok(())
}

pub fn get_music_next_shortcut() -> String {
    MUSIC_NEXT_SHORTCUT
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_default()
}

pub fn get_music_next_shortcut_id() -> u32 {
    MUSIC_NEXT_SHORTCUT_ID.load(Ordering::SeqCst)
}

fn set_music_next_shortcut(shortcut: &str) {
    *MUSIC_NEXT_SHORTCUT.lock().unwrap() = Some(shortcut.to_string());
    if let Ok(hotkey) = tauri_plugin_global_shortcut::Shortcut::from_str(shortcut) {
        MUSIC_NEXT_SHORTCUT_ID.store(hotkey.id(), Ordering::SeqCst);
    }
}

pub fn init_music_playpause(app_handle: &tauri::AppHandle, shortcut: &str) -> Result<(), String> {
    set_music_playpause_shortcut(shortcut);

    // 独立开关关闭时不注册（即使总开关开启也不生效）
    if !is_music_playpause_enabled() {
        log::info!("播放/暂停热键独立开关关闭，跳过注册: {}", shortcut);
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        app_handle
            .global_shortcut()
            .register(shortcut)
            .map_err(|e| format!("注册播放/暂停热键失败: {}", e))?;
    }

    log::info!("播放/暂停热键已注册: {}", shortcut);
    Ok(())
}

pub fn update_music_playpause(app_handle: &tauri::AppHandle, new_shortcut: &str) -> Result<(), String> {
    let old_shortcut = get_music_playpause_shortcut();

    if old_shortcut == new_shortcut {
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;

        if !old_shortcut.is_empty() {
            let _ = app_handle.global_shortcut().unregister(old_shortcut.as_str());
        }

        if !new_shortcut.is_empty() {
            if let Err(e) = app_handle.global_shortcut().register(new_shortcut) {
                if !old_shortcut.is_empty() {
                    let _ = app_handle.global_shortcut().register(old_shortcut.as_str());
                }
                return Err(hotkey_register_error("注册播放/暂停热键", new_shortcut, e));
            }
        }
    }

    set_music_playpause_shortcut(new_shortcut);
    log::info!("播放/暂停热键已更新: {} -> {}", old_shortcut, new_shortcut);
    Ok(())
}

pub fn get_music_playpause_shortcut() -> String {
    MUSIC_PLAYPAUSE_SHORTCUT
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_default()
}

pub fn get_music_playpause_shortcut_id() -> u32 {
    MUSIC_PLAYPAUSE_SHORTCUT_ID.load(Ordering::SeqCst)
}

fn set_music_playpause_shortcut(shortcut: &str) {
    *MUSIC_PLAYPAUSE_SHORTCUT.lock().unwrap() = Some(shortcut.to_string());
    if let Ok(hotkey) = tauri_plugin_global_shortcut::Shortcut::from_str(shortcut) {
        MUSIC_PLAYPAUSE_SHORTCUT_ID.store(hotkey.id(), Ordering::SeqCst);
    }
}

// ==================== 桌面歌词解锁按钮显示/隐藏热键 ====================

pub fn init_lyrics_btn_toggle(app_handle: &tauri::AppHandle, shortcut: &str) -> Result<(), String> {
    set_lyrics_btn_toggle_shortcut(shortcut);

    // 留空（未设置）或独立开关关闭时跳过注册，避免注册空串失败或无效注册
    if shortcut.is_empty() || !is_lyric_btn_enabled() {
        log::info!("解锁按钮热键跳过注册: {}", shortcut);
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        app_handle
            .global_shortcut()
            .register(shortcut)
            .map_err(|e| format!("注册解锁按钮热键失败: {}", e))?;
    }

    log::info!("解锁按钮热键已注册: {}", shortcut);
    Ok(())
}

pub fn update_lyrics_btn_toggle(
    app_handle: &tauri::AppHandle,
    new_shortcut: &str,
) -> Result<(), String> {
    let old_shortcut = get_lyrics_btn_toggle_shortcut();

    if old_shortcut == new_shortcut {
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;

        if !old_shortcut.is_empty() {
            let _ = app_handle.global_shortcut().unregister(old_shortcut.as_str());
        }

        if !new_shortcut.is_empty() {
            if let Err(e) = app_handle.global_shortcut().register(new_shortcut) {
                if !old_shortcut.is_empty() {
                    let _ = app_handle.global_shortcut().register(old_shortcut.as_str());
                }
                return Err(hotkey_register_error("注册解锁按钮热键", new_shortcut, e));
            }
        }
    }

    set_lyrics_btn_toggle_shortcut(new_shortcut);
    log::info!(
        "解锁按钮热键已更新: {} -> {}",
        old_shortcut,
        new_shortcut
    );
    Ok(())
}

pub fn get_lyrics_btn_toggle_shortcut() -> String {
    LYRIC_BTN_TOGGLE_SHORTCUT
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_default()
}

pub fn get_lyrics_btn_toggle_shortcut_id() -> u32 {
    LYRIC_BTN_TOGGLE_SHORTCUT_ID.load(Ordering::SeqCst)
}

fn set_lyrics_btn_toggle_shortcut(shortcut: &str) {
    *LYRIC_BTN_TOGGLE_SHORTCUT.lock().unwrap() = Some(shortcut.to_string());
    if let Ok(hotkey) = tauri_plugin_global_shortcut::Shortcut::from_str(shortcut) {
        LYRIC_BTN_TOGGLE_SHORTCUT_ID.store(hotkey.id(), Ordering::SeqCst);
    }
}

/// 触发音乐热键动作，向主窗口发送对应控制事件
pub fn trigger_music_action(app_handle: &tauri::AppHandle, action: &str) {
    emit_music_action(app_handle, action);
}

pub fn cleanup(app_handle: &tauri::AppHandle) {
    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;

        let overlay = get_overlay_shortcut();
        if !overlay.is_empty() {
            let _ = app_handle.global_shortcut().unregister(overlay.as_str());
        }


        let autoclicker = get_autoclicker_shortcut();
        if !autoclicker.is_empty() && !is_mouse_key(&autoclicker) {
            let _ = app_handle.global_shortcut().unregister(autoclicker.as_str());
        }

        let music_prev = get_music_prev_shortcut();
        if !music_prev.is_empty() {
            let _ = app_handle.global_shortcut().unregister(music_prev.as_str());
        }

        let music_next = get_music_next_shortcut();
        if !music_next.is_empty() {
            let _ = app_handle.global_shortcut().unregister(music_next.as_str());
        }

        let music_playpause = get_music_playpause_shortcut();
        if !music_playpause.is_empty() {
            let _ = app_handle.global_shortcut().unregister(music_playpause.as_str());
        }

        let lyric_btn_toggle = get_lyrics_btn_toggle_shortcut();
        if !lyric_btn_toggle.is_empty() {
            let _ = app_handle.global_shortcut().unregister(lyric_btn_toggle.as_str());
        }
    }
}

#[tauri::command]
pub fn get_overlay_hotkey() -> String {
    get_overlay_shortcut()
}

#[tauri::command]
pub fn set_overlay_hotkey(app_handle: tauri::AppHandle, shortcut: String) -> Result<(), String> {
    update_overlay(&app_handle, &shortcut)?;
    save_settings_value(
        &app_handle,
        "overlay-hotkey",
        serde_json::Value::String(shortcut),
    );
    Ok(())
}


#[tauri::command]
pub fn get_autoclicker_hotkey() -> String {
    get_autoclicker_shortcut()
}

#[tauri::command]
pub fn set_autoclicker_hotkey(app_handle: tauri::AppHandle, shortcut: String) -> Result<(), String> {
    update_autoclicker(&app_handle, &shortcut)?;
    save_settings_value(
        &app_handle,
        "autoclicker-hotkey",
        serde_json::Value::String(shortcut),
    );
    Ok(())
}

#[tauri::command]
pub fn get_music_prev_hotkey() -> String {
    get_music_prev_shortcut()
}

#[tauri::command]
pub fn set_music_prev_hotkey(app_handle: tauri::AppHandle, shortcut: String) -> Result<(), String> {
    update_music_prev(&app_handle, &shortcut)?;
    save_settings_value(
        &app_handle,
        "music-prev-hotkey",
        serde_json::Value::String(shortcut),
    );
    Ok(())
}

#[tauri::command]
pub fn get_music_next_hotkey() -> String {
    get_music_next_shortcut()
}

#[tauri::command]
pub fn set_music_next_hotkey(app_handle: tauri::AppHandle, shortcut: String) -> Result<(), String> {
    update_music_next(&app_handle, &shortcut)?;
    save_settings_value(
        &app_handle,
        "music-next-hotkey",
        serde_json::Value::String(shortcut),
    );
    Ok(())
}

#[tauri::command]
pub fn get_music_playpause_hotkey() -> String {
    get_music_playpause_shortcut()
}

#[tauri::command]
pub fn set_music_playpause_hotkey(
    app_handle: tauri::AppHandle,
    shortcut: String,
) -> Result<(), String> {
    update_music_playpause(&app_handle, &shortcut)?;
    save_settings_value(
        &app_handle,
        "music-playpause-hotkey",
        serde_json::Value::String(shortcut),
    );
    Ok(())
}

#[tauri::command]
pub fn get_lyrics_btn_hotkey() -> String {
    get_lyrics_btn_toggle_shortcut()
}

#[tauri::command]
pub fn set_lyrics_btn_hotkey(app_handle: tauri::AppHandle, shortcut: String) -> Result<(), String> {
    update_lyrics_btn_toggle(&app_handle, &shortcut)?;
    save_settings_value(
        &app_handle,
        "lyrics-btn-hotkey",
        serde_json::Value::String(shortcut),
    );
    Ok(())
}

#[tauri::command]
pub fn set_hotkeys_enabled_cmd(app_handle: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    set_hotkeys_enabled(enabled);
    // 同步注册/注销全局热键，释放或重新拦截按键
    apply_hotkeys_enabled(&app_handle, enabled);
    save_settings_value(
        &app_handle,
        "hotkeys-enabled",
        serde_json::Value::Bool(enabled),
    );
    log::info!(
        "全部热键总开关: {}",
        if enabled { "开启" } else { "关闭" }
    );
    Ok(())
}

#[tauri::command]
pub fn get_hotkeys_enabled_cmd() -> bool {
    is_hotkeys_enabled()
}

// ==================== 单个热键独立开关命令 ====================
// 每个热键一组 get/set：get 返回当前独立开关，set 写入设置并在总开关开启时按新状态注册/注销该热键。

#[tauri::command]
pub fn set_overlay_hotkey_enabled(app_handle: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    set_overlay_enabled(enabled);
    if is_hotkeys_enabled() {
        apply_single_hotkey(&app_handle, &get_overlay_shortcut(), enabled);
    }
    save_settings_value(&app_handle, "overlay-hotkey-enabled", serde_json::Value::Bool(enabled));
    log::info!("悬浮框热键开关: {}", if enabled { "开启" } else { "关闭" });
    Ok(())
}

#[tauri::command]
pub fn get_overlay_hotkey_enabled() -> bool {
    is_overlay_enabled()
}


#[tauri::command]
pub fn set_autoclicker_hotkey_enabled(app_handle: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    set_autoclicker_enabled(enabled);
    if is_hotkeys_enabled() {
        apply_single_hotkey(&app_handle, &get_autoclicker_shortcut(), enabled);
    }
    save_settings_value(&app_handle, "autoclicker-hotkey-enabled", serde_json::Value::Bool(enabled));
    log::info!("连点器热键开关: {}", if enabled { "开启" } else { "关闭" });
    Ok(())
}

#[tauri::command]
pub fn get_autoclicker_hotkey_enabled() -> bool {
    is_autoclicker_enabled()
}

#[tauri::command]
pub fn set_music_prev_hotkey_enabled(app_handle: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    set_music_prev_enabled(enabled);
    if is_hotkeys_enabled() {
        apply_single_hotkey(&app_handle, &get_music_prev_shortcut(), enabled);
    }
    save_settings_value(&app_handle, "music-prev-hotkey-enabled", serde_json::Value::Bool(enabled));
    log::info!("上一曲热键开关: {}", if enabled { "开启" } else { "关闭" });
    Ok(())
}

#[tauri::command]
pub fn get_music_prev_hotkey_enabled() -> bool {
    is_music_prev_enabled()
}

#[tauri::command]
pub fn set_music_next_hotkey_enabled(app_handle: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    set_music_next_enabled(enabled);
    if is_hotkeys_enabled() {
        apply_single_hotkey(&app_handle, &get_music_next_shortcut(), enabled);
    }
    save_settings_value(&app_handle, "music-next-hotkey-enabled", serde_json::Value::Bool(enabled));
    log::info!("下一曲热键开关: {}", if enabled { "开启" } else { "关闭" });
    Ok(())
}

#[tauri::command]
pub fn get_music_next_hotkey_enabled() -> bool {
    is_music_next_enabled()
}

#[tauri::command]
pub fn set_music_playpause_hotkey_enabled(app_handle: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    set_music_playpause_enabled(enabled);
    if is_hotkeys_enabled() {
        apply_single_hotkey(&app_handle, &get_music_playpause_shortcut(), enabled);
    }
    save_settings_value(&app_handle, "music-playpause-hotkey-enabled", serde_json::Value::Bool(enabled));
    log::info!("播放/暂停热键开关: {}", if enabled { "开启" } else { "关闭" });
    Ok(())
}

#[tauri::command]
pub fn get_music_playpause_hotkey_enabled() -> bool {
    is_music_playpause_enabled()
}

// ==================== 配置持久化 ====================

/// 串行化 settings.json 写入，避免多个热键并发保存时互相覆盖
static SETTINGS_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// 将指定 key 写入 settings.json（保留文件中的其他 key，兼容前端 LazyStore）
///
/// # 为什么不能「读失败就从空对象重建」
///
/// 旧实现是 `read_to_string(..).ok().and_then(from_str.ok()).unwrap_or_else(|| json!({}))`，
/// 也就是**读或解析任意一步失败都当成「文件不存在」**，然后整体重写一个只含本次 key
/// 的新文件。而 settings.json 同时被三方读写：前端 `LazyStore`（plugin-store）、
/// 本函数、以及其它命令。plugin-store 保存时会重写整个文件，此时本进程去读很
/// 可能拿到**空串或半截 JSON**（典型表现为读到 0 字节）——解析失败 → 从空对象重建
/// → 用户全部配置（壁纸、优化项、主题、字体、快捷键…）被静默清空，只剩刚写入的
/// 那一个 key。
///
/// 实测证据：一次正常启动后该文件从 7,458 字节变成 298 字节，只剩 10 个键，
/// 丢失 90% 配置，且 mtime 恰好是启动瞬间（写 `has-launched`）。
///
/// 正确语义：**只有在文件确实不存在时**才允许从空对象开始；文件存在但读不出来
/// 说明正被并发写入，此时必须放弃这次写入（本次 key 丢失是可接受的，
/// 丢用户全部配置不可接受）。写入本身也改成「临时文件 + rename」原子替换，
/// 防止写到一半崩溃留下半截文件。
pub(crate) fn save_settings_value(app: &tauri::AppHandle, key: &str, value: serde_json::Value) {
    #[cfg(target_os = "windows")]
    {
        use tauri::Manager;
        let _guard = SETTINGS_WRITE_LOCK.lock().unwrap();
        let Ok(dir) = app.path().app_data_dir() else {
            return;
        };
        // 委托给统一的安全写入实现（同一份 settings.json 的三处写入共用，
        // 避免再出现第四份「读失败即重建」的复制粘贴）。见
        // crate::vertical_overlay::update_settings_json 的详细说明。
        let path = dir.join("settings.json");
        let mut json: serde_json::Value = if !path.exists() {
            serde_json::json!({})
        } else {
            let Ok(content) = std::fs::read_to_string(&path) else {
                log::warn!(
                    "[Settings] 读取 settings.json 失败，跳过本次写入 key={key}，避免覆盖用户配置"
                );
                return;
            };
            match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(v) if v.is_object() => v,
                _ => {
                    log::warn!(
                        "[Settings] settings.json 暂时不可解析，跳过本次写入 key={key}，避免覆盖用户配置"
                    );
                    return;
                }
            }
        };
        if let Some(obj) = json.as_object_mut() {
            obj.insert(key.to_string(), value);
        }
        let Ok(content) = serde_json::to_string_pretty(&json) else {
            return;
        };
        // 原子替换：先写同目录临时文件再 rename。
        // 直接 write 原文件时，若进程在此期间被结束，会留下空/半截文件，
        // 下次启动又会触发上面那条「读失败」路径。
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, content).is_ok() {
            if std::fs::rename(&tmp, &path).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, key, value);
    }
}

/// 从 settings.json（前端 LazyStore 写入）读取指定 key 的值
pub(crate) fn read_settings_value(app: &tauri::AppHandle, key: &str) -> Option<serde_json::Value> {
    #[cfg(target_os = "windows")]
    {
        use tauri::Manager;
        let dir = app.path().app_data_dir().ok()?;
        let path = dir.join("settings.json");
        let content = std::fs::read_to_string(path).ok()?;
        let json: serde_json::Value = serde_json::from_str(&content).ok()?;
        json.get(key).cloned()
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, key);
        None
    }
}

/// 读取保存的快捷键，未保存或值无效时使用默认值
pub fn load_saved_hotkey(app: &tauri::AppHandle, key: &str, default: &str) -> String {
    match read_settings_value(app, key) {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => s,
        _ => default.to_string(),
    }
}

/// 读取热键总开关，未保存时默认开启
pub fn load_saved_hotkeys_enabled(app: &tauri::AppHandle) -> bool {
    match read_settings_value(app, "hotkeys-enabled") {
        Some(serde_json::Value::Bool(b)) => b,
        _ => true,
    }
}

/// 读取单个热键的独立开关，未保存时使用指定的默认值
pub fn load_saved_hotkey_enabled(app: &tauri::AppHandle, key: &str, default: bool) -> bool {
    match read_settings_value(app, key) {
        Some(serde_json::Value::Bool(b)) => b,
        _ => default,
    }
}



