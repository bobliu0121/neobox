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
pub fn persist_position<R: Runtime>(app: &AppHandle<R>, x: i32, y: i32) {
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let path = dir.join("settings.json");
    let mut json: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = json.as_object_mut() {
        let main = obj.entry("main-window").or_insert(serde_json::json!({}));
        if let Some(m) = main.as_object_mut() {
            m.insert("position_x".to_string(), serde_json::json!(x));
            m.insert("position_y".to_string(), serde_json::json!(y));
        }
    }
    if let Ok(content) = serde_json::to_string_pretty(&json) {
        let _ = std::fs::write(&path, content);
        log::info!("[main-window] save position x={x} y={y}");
    }
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