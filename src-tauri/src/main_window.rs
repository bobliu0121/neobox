//! 主窗口位置持久化模块
//!
//! 复用竖排悬浮窗的持久化模式：将主窗口的物理像素坐标写入 settings.json 的
//! `main-window` 键（保留其他键），启动时恢复，关闭/退出时保存兜底。

use tauri::{AppHandle, Manager, Runtime};

/// 从持久化 settings.json 读取主窗口保存的位置（物理坐标）
pub fn read_saved_position<R: Runtime>(app: &AppHandle<R>) -> Option<(i32, i32)> {
    let dir = app.path().app_data_dir().ok()?;
    let path = dir.join("settings.json");
    let content = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    let main = json.get("main-window")?;
    let x = main.get("position_x")?.as_i64()? as i32;
    let y = main.get("position_y")?.as_i64()? as i32;
    Some((x, y))
}

/// 将主窗口位置写入 settings.json 的 `main-window` 键（仅更新位置，保留其他 key）
///
/// 注意：这个函数在窗口每次移动/关闭时都会调用，触发频率极高。
/// 因此**绝不能**在「读失败」时从空对象重建 —— settings.json 同时被前端
/// plugin-store 与本进程读写，读到的可能是空串/半截 JSON（正在被重写），
/// 一旦此时整体覆写，用户的全部配置会被静默清空，只剩窗口坐标。
/// 详见 [`crate::hotkey::save_settings_value`] 的说明与实测证据。
pub fn persist_position<R: Runtime>(app: &AppHandle<R>, x: i32, y: i32) {
    // 复用 vertical_overlay 里的安全写法（同一份 settings.json，同样的并发风险）
    // 见 crate::vertical_overlay::update_settings_json 的说明。
    let _ = (x, y); // 真正写入在下方泛型辅助函数里，这里仅保留签名
    crate::vertical_overlay::update_settings_json(app, "main-window", |obj| {
        let main = obj.entry("main-window").or_insert(serde_json::json!({}));
        if let Some(m) = main.as_object_mut() {
            m.insert("position_x".to_string(), serde_json::json!(x));
            m.insert("position_y".to_string(), serde_json::json!(y));
        }
    });
    log::debug!("[main-window] save position requested x={x} y={y}");
}

/// 判断指定物理坐标是否落在任一显示器窗口区域内（含 10px 容差，复用 tray 归位判据）
pub fn is_on_any_monitor<R: Runtime>(app: &AppHandle<R>, pos: (i32, i32)) -> bool {
    let Some(monitors) = app.available_monitors().ok() else {
        return false;
    };
    monitors.iter().any(|m| {
        let r = m.position();
        let s = m.size();
        let x = pos.0;
        let y = pos.1;
        x + 10 >= r.x && x - 10 <= r.x + s.width as i32
            && y + 10 >= r.y && y - 10 <= r.y + s.height as i32
    })
}

/// 恢复主窗口位置：有保存记录且仍在屏幕内 → 恢复到保存位置；
/// 否则（无记录或离屏）→ 显式居中到主显示器。
pub fn restore_position<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if let Some(pos) = read_saved_position(app) {
        if is_on_any_monitor(app, pos) {
            let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
                x: pos.0,
                y: pos.1,
            }));
            log::info!("[main-window] restore saved position {:?}", pos);
            return;
        }
    }
    // 无记录或保存位置已离屏 → 居中到主显示器
    if let Some(monitor) = app.primary_monitor().ok().flatten() {
        let p = monitor.position();
        let s = monitor.size();
        if let Ok(ws) = window.outer_size() {
            let x = p.x + (s.width as i32 - ws.width as i32) / 2;
            let y = p.y + (s.height as i32 - ws.height as i32) / 2;
            let _ = window.set_position(tauri::Position::Physical(
                tauri::PhysicalPosition { x, y },
            ));
        }
    }
}

/// 保存主窗口当前位置：读取实际 outer_position，跳过最大化/最小化等异常状态。
pub fn save_current_position<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    // 最大化/最小化时 outer_position 不可靠，跳过保存避免恢复到异常状态
    if window.is_maximized().unwrap_or(false) || window.is_minimized().unwrap_or(false) {
        log::info!("[main-window] skip save position (window maximized/minimized)");
        return;
    }
    if let Ok(pos) = window.outer_position() {
        persist_position(app, pos.x, pos.y);
    }
}