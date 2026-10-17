use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// 连点器运行状态
static RUNNING: AtomicBool = AtomicBool::new(false);
/// 当前点击键位: 0=左键, 1=右键
static BUTTON: AtomicU8 = AtomicU8::new(0);
/// 点击间隔（毫秒）
static INTERVAL_MS: AtomicU64 = AtomicU64::new(100);
/// 点击工作线程句柄
static THREAD_HANDLE: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

#[derive(serde::Serialize, Clone, Debug)]
pub struct AutoClickerStatus {
    pub running: bool,
    pub button: String,
    pub interval_ms: u64,
}

fn current_button_label() -> &'static str {
    if BUTTON.load(Ordering::SeqCst) == 1 { "right" } else { "left" }
}

fn simulate_click() {
    #[cfg(target_os = "windows")]
    unsafe {
        use winapi::um::winuser::{
            mouse_event, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_RIGHTDOWN,
            MOUSEEVENTF_RIGHTUP,
        };
        match BUTTON.load(Ordering::SeqCst) {
            1 => {
                mouse_event(MOUSEEVENTF_RIGHTDOWN, 0, 0, 0, 0);
                mouse_event(MOUSEEVENTF_RIGHTUP, 0, 0, 0, 0);
            }
            _ => {
                mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
                mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
            }
        }
    }
}

fn click_loop() {
    // 使用 spin_sleep 保证高频点击时定时精度（std::thread::sleep 误差可达 10ms+）
    let sleeper = spin_sleep::SpinSleeper::default();
    while RUNNING.load(Ordering::SeqCst) {
        simulate_click();
        let interval = INTERVAL_MS.load(Ordering::SeqCst).max(1);
        sleeper.sleep(Duration::from_millis(interval));
    }
}

pub fn get_status() -> AutoClickerStatus {
    AutoClickerStatus {
        running: RUNNING.load(Ordering::SeqCst),
        button: current_button_label().to_string(),
        interval_ms: INTERVAL_MS.load(Ordering::SeqCst),
    }
}

/// 更新键位与间隔（运行中也会实时生效）
pub fn update_settings(button: &str, interval_ms: u64) {
    BUTTON.store(if button == "right" { 1 } else { 0 }, Ordering::SeqCst);
    INTERVAL_MS.store(interval_ms.max(1), Ordering::SeqCst);
}

/// 向前端发送连点器状态事件（热键触发时实时刷新页面状态徽章）
fn emit_status<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    use tauri::Emitter;
    let _ = app.emit("autoclicker-status-changed", get_status());
}

pub fn start<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    if RUNNING.load(Ordering::SeqCst) {
        return Ok(());
    }
    RUNNING.store(true, Ordering::SeqCst);
    let handle = thread::spawn(click_loop);
    *THREAD_HANDLE.lock().unwrap() = Some(handle);
    log::info!(
        "连点器已启动: button={} interval={}ms",
        current_button_label(),
        INTERVAL_MS.load(Ordering::SeqCst)
    );
    emit_status(app);
    Ok(())
}

pub fn stop<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    if !RUNNING.load(Ordering::SeqCst) {
        return Ok(());
    }
    RUNNING.store(false, Ordering::SeqCst);
    if let Some(handle) = THREAD_HANDLE.lock().unwrap().take() {
        let _ = handle.join();
    }
    log::info!("连点器已停止");
    emit_status(app);
    Ok(())
}

/// 切换连点器开关（供热键/前端调用），返回切换后的状态
pub fn toggle<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<bool, String> {
    if RUNNING.load(Ordering::SeqCst) {
        stop(app)?;
        Ok(false)
    } else {
        start(app)?;
        Ok(true)
    }
}

pub fn cleanup() {
    RUNNING.store(false, Ordering::SeqCst);
    if let Some(handle) = THREAD_HANDLE.lock().unwrap().take() {
        let _ = handle.join();
    }
    #[cfg(target_os = "windows")]
    mouse_hotkey::uninstall();
}

/// 设置/清除鼠标键热键（tauri 的 global-shortcut 插件不支持鼠标键，
/// 因此改用 GetAsyncKeyState 轮询线程检测，可识别中键与侧键 X1/X2）。
pub fn set_mouse_hotkey(app: &tauri::AppHandle, key: Option<&str>) {
    #[cfg(target_os = "windows")]
    {
        let vk = mouse_hotkey::mouse_key_to_vk(key);
        mouse_hotkey::set_poll_vk(app.clone(), vk);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, key);
    }
}

/// 鼠标键热键轮询：用 GetAsyncKeyState 检测中键/侧键的按下边沿。
/// 不依赖低级钩子，可靠性更高，也不会被连点器模拟的点击干扰（模拟的是左右键）。
#[cfg(target_os = "windows")]
mod mouse_hotkey {
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
    use std::sync::Mutex;
    use std::thread::{self, JoinHandle};
    use std::time::Duration;
    use winapi::um::winuser::GetAsyncKeyState;

    /// 虚拟键码：VK_MBUTTON=0x04, VK_XBUTTON1=0x05, VK_XBUTTON2=0x06
    fn vk_for(name: &str) -> Option<i32> {
        match name {
            "MouseMiddle" => Some(0x04),
            "MouseX1" => Some(0x05),
            "MouseX2" => Some(0x06),
            _ => None,
        }
    }

    static WANTED_VK: AtomicI32 = AtomicI32::new(0);
    static PREV_DOWN: AtomicBool = AtomicBool::new(false);
    static RUNNING: AtomicBool = AtomicBool::new(false);
    static APP_HANDLE: Mutex<Option<tauri::AppHandle>> = Mutex::new(None);
    static THREAD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

    /// 鼠标键名称 → 虚拟键码
    pub fn mouse_key_to_vk(key: Option<&str>) -> i32 {
        key.and_then(vk_for).unwrap_or(0)
    }

    fn poll_loop() {
        log::info!("[MouseHotkey] 鼠标键轮询线程已启动");
        let sleeper = spin_sleep::SpinSleeper::default();
        while RUNNING.load(Ordering::SeqCst) {
            let vk = WANTED_VK.load(Ordering::SeqCst);
            if vk != 0 {
                let down = unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000 != 0;
                let prev = PREV_DOWN.swap(down, Ordering::SeqCst);
                // 检测按下边沿（从松开变为按下）
                if down && !prev {
                    if let Some(app) = APP_HANDLE.lock().unwrap().clone() {
                        tauri::async_runtime::spawn(async move {
                            let _ = super::toggle(&app);
                        });
                    }
                }
            } else {
                PREV_DOWN.store(false, Ordering::SeqCst);
            }
            sleeper.sleep(Duration::from_millis(8));
        }
        log::info!("[MouseHotkey] 鼠标键轮询线程已停止");
    }

    pub fn set_poll_vk(app: tauri::AppHandle, vk: i32) {
        if WANTED_VK.load(Ordering::SeqCst) == vk && vk != 0 && RUNNING.load(Ordering::SeqCst) {
            return;
        }
        // 停止旧线程
        RUNNING.store(false, Ordering::SeqCst);
        if let Some(handle) = THREAD.lock().unwrap().take() {
            let _ = handle.join();
        }
        *APP_HANDLE.lock().unwrap() = None;
        PREV_DOWN.store(false, Ordering::SeqCst);

        if vk == 0 {
            WANTED_VK.store(0, Ordering::SeqCst);
            log::info!("[MouseHotkey] 已清除鼠标键热键");
            return;
        }

        WANTED_VK.store(vk, Ordering::SeqCst);
        *APP_HANDLE.lock().unwrap() = Some(app);
        RUNNING.store(true, Ordering::SeqCst);
        let handle = thread::spawn(poll_loop);
        *THREAD.lock().unwrap() = Some(handle);
    }

    pub fn uninstall() {
        RUNNING.store(false, Ordering::SeqCst);
        if let Some(handle) = THREAD.lock().unwrap().take() {
            let _ = handle.join();
        }
        WANTED_VK.store(0, Ordering::SeqCst);
        PREV_DOWN.store(false, Ordering::SeqCst);
        *APP_HANDLE.lock().unwrap() = None;
    }
}

#[tauri::command]
pub fn autoclicker_start(
    app: tauri::AppHandle,
    button: String,
    interval_ms: u64,
) -> Result<bool, String> {
    update_settings(&button, interval_ms);
    start(&app)?;
    Ok(true)
}

#[tauri::command]
pub fn autoclicker_stop(app: tauri::AppHandle) -> Result<bool, String> {
    stop(&app)?;
    Ok(false)
}

#[tauri::command]
pub fn autoclicker_toggle(app: tauri::AppHandle) -> Result<bool, String> {
    toggle(&app)
}

#[tauri::command]
pub fn autoclicker_update(button: String, interval_ms: u64) -> Result<(), String> {
    update_settings(&button, interval_ms);
    Ok(())
}

#[tauri::command]
pub fn autoclicker_get_status() -> AutoClickerStatus {
    get_status()
}
