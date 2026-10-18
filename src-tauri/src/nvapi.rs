//! NVAPI backend for NVIDIA display / resolution management.
//!
//! Dynamically loads nvapi64.dll via libloading (no static link) and provides
//! Tauri commands to enumerate displays, query display modes and set custom
//! resolutions. The DRS (driver settings) portion has been removed.

use std::sync::Mutex;

// ---------------------------------------------------------------------------
// NVAPI C type aliases
// ---------------------------------------------------------------------------
type NvU32 = u32;
type NvU16 = u16;

#[allow(non_camel_case_types)]
type NvAPI_Status = i32;

const NVAPI_OK: NvAPI_Status = 0;

const NVAPI_SHORT_STRING_MAX: usize = 64;

// ---------------------------------------------------------------------------
// NVAPI DISP (Display Control) structures
// nvapi.h uses #pragma pack(push, 8) — #[repr(C)] on x64 matches this layout.
// Field names match the C SDK header names deliberately.
// ---------------------------------------------------------------------------

#[allow(non_snake_case)]
#[repr(C)]
struct NvResolution {
    width: NvU32,
    height: NvU32,
    colorDepth: NvU32,
}

const _: () = assert!(std::mem::size_of::<NvResolution>() == 12);

#[repr(C)]
struct NvPosition {
    x: NvU32,
    y: NvU32,
}

const _: () = assert!(std::mem::size_of::<NvPosition>() == 8);

#[allow(non_snake_case)]
#[repr(C)]
struct NvDisplayConfigSourceModeInfo {
    resolution: NvResolution,       // NV_RESOLUTION
    colorFormat: NvU32,             // NV_FORMAT (ignored, must be NV_FORMAT_UNKNOWN=0)
    position: NvPosition,           // NV_POSITION
    spanningOrientation: NvU32,     // NV_DISPLAYCONFIG_SPANNING_ORIENTATION (0=none)
    flags: NvU32,                   // bGDIPrimary:1, bSLIFocus:1, reserved:30
}

// sizeof(NV_DISPLAYCONFIG_SOURCE_MODE_INFO_V1) = 12 + 4 + 8 + 4 + 4 = 32
const _: () = assert!(std::mem::size_of::<NvDisplayConfigSourceModeInfo>() == 32);

#[allow(non_snake_case)]
#[repr(C)]
struct NvDisplayConfigPathTargetInfo {
    displayId: NvU32,
    details: *mut std::ffi::c_void, // NV_DISPLAYCONFIG_PATH_ADVANCED_TARGET_INFO* (NULL if not needed)
    targetId: NvU32,                // Windows CCD target ID (non-NVIDIA adapter only, 0 for NVIDIA)
}

// sizeof(NV_DISPLAYCONFIG_PATH_TARGET_INFO_V2) = 4 + pad4 + 8 + 4 = 24 (x64 pack 8)
const _: () = assert!(std::mem::size_of::<NvDisplayConfigPathTargetInfo>() == 24);

#[allow(non_snake_case)]
#[repr(C)]
struct NvDisplayConfigPathInfo {
    version: NvU32,                         // MAKE_NVAPI_VERSION(NV_DISPLAYCONFIG_PATH_INFO_V2, 2)
    sourceId: NvU32,                        // union { sourceId, reserved_sourceId }
    targetInfoCount: NvU32,                 // number of elements in targetInfo array
    targetInfo: *mut NvDisplayConfigPathTargetInfo,
    sourceModeInfo: *mut NvDisplayConfigSourceModeInfo, // may be NULL
    flags: NvU32,                           // IsNonNVIDIAAdapter:1, reserved:31
    pOSAdapterID: *mut std::ffi::c_void,    // LUID pointer for non-NVIDIA adapter
}

// sizeof(NV_DISPLAYCONFIG_PATH_INFO_V2) = 4+4+4+pad4+8+8+4+pad4+8 = 48 (x64 pack 8)
const _: () = assert!(std::mem::size_of::<NvDisplayConfigPathInfo>() == 48);

/// Version constant for NV_DISPLAYCONFIG_PATH_INFO_V2 (MAKE_NVAPI_VERSION(type, 2))
const NV_DISPLAYCONFIG_PATH_INFO_VER: NvU32 =
    (std::mem::size_of::<NvDisplayConfigPathInfo>() as NvU32) | (2u32 << 16);



/// Flags for NvAPI_DISP_SetDisplayConfig
#[allow(dead_code)]
const NV_DISPLAYCONFIG_VALIDATE_ONLY: NvU32          = 0x00000001;
const NV_DISPLAYCONFIG_SAVE_TO_PERSISTENCE: NvU32    = 0x00000002;
#[allow(dead_code)]
const NV_DISPLAYCONFIG_DRIVER_RELOAD_ALLOWED: NvU32  = 0x00000004;
#[allow(dead_code)]
const NV_DISPLAYCONFIG_FORCE_MODE_ENUMERATION: NvU32 = 0x00000008;
const NV_FORCE_COMMIT_VIDPN: NvU32                   = 0x00000010;

// ---------------------------------------------------------------------------
// NVAPI DISP mode enumeration structures (NV_MODE_TIMING / NV_MODE_INFO / ...)
//
// NvAPI_DISP_EnumDisplayModes is NOT exported by the bundled SDK nvapi64.lib,
// so it must be loaded dynamically from the runtime nvapi64.dll. These structs
// mirror the public NVAPI headers so we can read width/height/refresh-rate.
// ---------------------------------------------------------------------------
#[allow(non_snake_case)]
#[repr(C)]
#[derive(Clone, Copy)]
struct NvTimingExt {
    flag: NvU32,
    rr: NvU16,
    rrx1k: NvU32,
    aspect: NvU32,
    rep: NvU16,
    status: NvU32,
    name: [u8; 40],
}
const _: () = assert!(std::mem::size_of::<NvTimingExt>() == 64);

#[allow(non_snake_case)]
#[repr(C)]
#[derive(Clone, Copy)]
struct NvTiming {
    h_visible: NvU16,
    h_border: NvU16,
    h_front_porch: NvU16,
    h_sync_width: NvU16,
    h_total: NvU16,
    h_sync_pol: u8,
    v_visible: NvU16,
    v_border: NvU16,
    v_front_porch: NvU16,
    v_sync_width: NvU16,
    v_total: NvU16,
    v_sync_pol: u8,
    interlaced: NvU16,
    pclk: NvU32,
    etc: NvTimingExt,
}
const _: () = assert!(std::mem::size_of::<NvTiming>() == 96);

#[allow(non_snake_case)]
#[repr(C)]
#[derive(Clone, Copy)]
struct NvModeProperty {
    property_version: NvU32,
    scaling: i32,
    rotation: i32,
    b_gdi_compat: NvU32,
    b_stretch_compat: NvU32,
    color_format: i32,
    position: NvU32,
    dithering: NvU32,
    clone_compat: NvU32,
    b_prev_mode: NvU32,
    b_mode_changed: NvU32,
}
const _: () = assert!(std::mem::size_of::<NvModeProperty>() == 44);

#[allow(non_snake_case)]
#[repr(C)]
#[derive(Clone, Copy)]
struct NvModeInfo {
    timing: NvTiming,
    mode_type: i32,
    orig_v_visible: NvU32,
    property: NvModeProperty,
    aspect: NvU32,
}
const _: () = assert!(std::mem::size_of::<NvModeInfo>() == 152);

#[allow(non_snake_case)]
#[repr(C)]
#[derive(Clone, Copy)]
struct NvModeTiming {
    mode_info: NvModeInfo,
}
const _: () = assert!(std::mem::size_of::<NvModeTiming>() == 152);

// ---------------------------------------------------------------------------
// NVAPI 动态加载（libloading）
// 不依赖 nvapi64.lib 静态链接：编译期零依赖；运行时无 NVIDIA 驱动时安全降级。
// ---------------------------------------------------------------------------

/// 动态缓存的 NVAPI 函数指针集合（通过 libloading 按名字解析自 nvapi64.dll）。
#[allow(non_snake_case)]
struct NvApiDyn {
    initialize: libloading::Symbol<'static, unsafe extern "C" fn() -> NvAPI_Status>,
    unload: libloading::Symbol<'static, unsafe extern "C" fn() -> NvAPI_Status>,
    get_error_message: libloading::Symbol<'static, unsafe extern "C" fn(NvAPI_Status, *mut u8) -> NvAPI_Status>,
    get_display_config: libloading::Symbol<'static, unsafe extern "C" fn(path_info_count: *mut NvU32, path_info: *mut NvDisplayConfigPathInfo) -> NvAPI_Status>,
    set_display_config: libloading::Symbol<'static, unsafe extern "C" fn(path_info_count: NvU32, path_info: *const NvDisplayConfigPathInfo, flags: NvU32) -> NvAPI_Status>,
    get_display_id_by_display_name: libloading::Symbol<'static, unsafe extern "C" fn(display_name: *const u8, display_id: *mut NvU32) -> NvAPI_Status>,
}

static NVAPI_DYN: std::sync::OnceLock<Option<NvApiDyn>> = std::sync::OnceLock::new();

/// 加载并缓存 NVAPI 动态库；找不到可用的 nvapi64.dll 时返回 None。
fn load_nvapi_dyn() -> Option<&'static NvApiDyn> {
    NVAPI_DYN
        .get_or_init(|| {
            let path = find_nvapi64_candidates().into_iter().next()?.0;
            let lib: libloading::Library = unsafe { libloading::Library::new(&path) }.ok()?;
            let lib: &'static libloading::Library = Box::leak(Box::new(lib));
            macro_rules! sym {
                ($name:expr, $ty:ty) => {
                    unsafe { lib.get::<$ty>(&$name[..]) }.ok()?
                };
            }
            Some(NvApiDyn {
                initialize: sym!(b"NvAPI_Initialize\0", unsafe extern "C" fn() -> NvAPI_Status),
                unload: sym!(b"NvAPI_Unload\0", unsafe extern "C" fn() -> NvAPI_Status),
                get_error_message: sym!(b"NvAPI_GetErrorMessage\0", unsafe extern "C" fn(NvAPI_Status, *mut u8) -> NvAPI_Status),
                get_display_config: sym!(b"NvAPI_DISP_GetDisplayConfig\0", unsafe extern "C" fn(path_info_count: *mut NvU32, path_info: *mut NvDisplayConfigPathInfo) -> NvAPI_Status),
                set_display_config: sym!(b"NvAPI_DISP_SetDisplayConfig\0", unsafe extern "C" fn(path_info_count: NvU32, path_info: *const NvDisplayConfigPathInfo, flags: NvU32) -> NvAPI_Status),
                get_display_id_by_display_name: sym!(b"NvAPI_DISP_GetDisplayIdByDisplayName\0", unsafe extern "C" fn(display_name: *const u8, display_id: *mut NvU32) -> NvAPI_Status),
            })
        })
        .as_ref()
}

/// 获取 NVAPI 动态接口；不可用时返回错误（供返回 Result 的调用点使用）。
fn nvapi_dyn() -> Result<&'static NvApiDyn, String> {
    load_nvapi_dyn().ok_or_else(|| "NVAPI 不可用：未检测到 NVIDIA 驱动".to_string())
}

// ---------------------------------------------------------------------------
// Global state — Library kept alive, function pointers cached
// ---------------------------------------------------------------------------
struct NvapiState;

unsafe impl Send for NvapiState {}
unsafe impl Sync for NvapiState {}

static NVAPI: Mutex<Option<NvapiState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn cstr_to_string(data: &[u8]) -> String {
    let end = data.iter().position(|&c| c == 0).unwrap_or(data.len());
    String::from_utf8_lossy(&data[..end]).to_string()
}

fn nvapi_error_string(status: NvAPI_Status) -> String {
    let mut buf: [u8; NVAPI_SHORT_STRING_MAX] = [0u8; NVAPI_SHORT_STRING_MAX];
    if let Some(dynapi) = load_nvapi_dyn() {
        unsafe {
            (dynapi.get_error_message)(status, buf.as_mut_ptr());
        }
    }
    cstr_to_string(&buf)
}

fn nvapi_system_dir() -> String {
    std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())
}

// ---------------------------------------------------------------------------
// DLL loading & session init (using libloading)
// ---------------------------------------------------------------------------

/// Find all nvapi64.dll candidates across the system, sorted by filesize (largest first).
/// The real NVIDIA nvapi64.dll is 2-4+ MB; stubs are <1 MB.
fn find_nvapi64_candidates() -> Vec<(String, u64)> {
    let mut files: Vec<(String, u64)> = Vec::new();

    // Helper: add file if it exists
    let mut add_file = |path: &str| {
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.is_file() && meta.len() > 0 {
                files.push((path.to_string(), meta.len()));
            }
        }
    };

    let sys_dir = nvapi_system_dir();
    let is_64bit = std::mem::size_of::<usize>() == 8;

    // Primary: System32
    add_file(&format!("{sys_dir}\\System32\\nvapi64.dll"));
    // Sysnative (bypasses WOW64 redirection for 32-bit processes)
    if !is_64bit {
        add_file(&format!("{sys_dir}\\Sysnative\\nvapi64.dll"));
    }

    // Common NVIDIA program dirs
    let prog_files = std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into());
    let nv_dir = format!("{prog_files}\\NVIDIA Corporation");
    let common_subdirs = ["NvStreamSrv", "Nvapi64", "Display.NvContainer", "FrameViewSDK"];
    for sub in &common_subdirs {
        add_file(&format!("{nv_dir}\\{sub}\\nvapi64.dll"));
    }

    // Scan NVIDIA Corporation dir one level deep
    if let Ok(entries) = std::fs::read_dir(&nv_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let candidate = p.join("nvapi64.dll");
                if let Some(s) = candidate.to_str() {
                    add_file(s);
                }
            }
        }
    }

    // Search DriverStore for nvapi64.dll
    let driver_store = format!("{sys_dir}\\System32\\DriverStore\\FileRepository");
    if let Ok(entries) = std::fs::read_dir(&driver_store) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // NVIDIA driver packages: nv_disp*, nv_dispi*, etc.
            if name_str.starts_with("nv_") || name_str.starts_with("nvd") || name_str.starts_with("nvdisps") {
                let candidate = entry.path().join("nvapi64.dll");
                if let Some(s) = candidate.to_str() {
                    add_file(s);
                }
            }
        }
    }

    // Sort by file size descending (biggest = most likely real NVIDIA DLL)
    files.sort_by(|a, b| b.1.cmp(&a.1));
    files
}

fn try_init_nvapi() -> Result<(), String> {
    let mut guard = NVAPI.lock().map_err(|e| format!("Lock error: {e}"))?;
    if guard.is_some() {
        return Ok(());
    }
    let dynapi = nvapi_dyn()?;

    let init_status = unsafe { (dynapi.initialize)() };
    if init_status != NVAPI_OK {
        return Err(format!(
            "NvAPI_Initialize 失败: {} (代码 {init_status})",
            nvapi_error_string(init_status)
        ));
    }

    let candidates = find_nvapi64_candidates();
    if let Some((path, size)) = candidates.first() {
        log::info!("NVAPI initialized via SDK library; detected runtime candidate: {path} ({size} bytes)");
    } else {
        log::info!("NVAPI initialized via SDK library");
    }
    *guard = Some(NvapiState);
    Ok(())
}

#[derive(serde::Serialize, Clone)]
pub struct NvidiaDisplay {
    pub display_id: NvU32,
    pub device_name: String,
    pub monitor_name: String,
    pub is_primary: bool,
    pub current_width: i32,
    pub current_height: i32,
}

#[derive(serde::Serialize, Clone)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh_rate: f64,
    pub is_current: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq)]
pub struct InjectedResolution {
    pub width: u32,
    pub height: u32,
}

#[derive(serde::Serialize)]
pub struct SetResolutionResult {
    pub applied: bool,
    pub injected: bool,
}

fn injected_resolutions_path() -> std::path::PathBuf {
    let config_dir = dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    config_dir.join("NeoBox").join("injected_resolutions.json")
}

fn load_injected_resolutions() -> Vec<InjectedResolution> {
    let path = injected_resolutions_path();
    if !path.exists() {
        return Vec::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_injected_resolution(width: u32, height: u32) -> Result<(), String> {
    let path = injected_resolutions_path();
    let mut list = load_injected_resolutions();
    let entry = InjectedResolution { width, height };
    if !list.contains(&entry) {
        list.push(entry);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
    }
    std::fs::write(
        &path,
        serde_json::to_string(&list).map_err(|e| format!("序列化失败: {}", e))?,
    )
    .map_err(|e| format!("写入失败: {}", e))?;
    Ok(())
}

/// Check if a monitor name is a generic/placeholder (any language variant)
fn is_generic_monitor_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("generic")
        || lower.contains("即插即用")
        || lower.contains("通用")
        || lower.contains("pnp")
        || lower.contains("standard monitor")
        || lower.contains("digital display")
        || lower.contains("analog display")
}
/// Enumerate NVIDIA displays with real NVAPI displayId from GetDisplayConfig.
#[tauri::command]
pub async fn list_nvidia_displays() -> Result<Vec<NvidiaDisplay>, String> {
    #[cfg(not(target_os = "windows"))]
    {
        return Err("此功能仅支持 Windows 系统".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        let result = tauri::async_runtime::spawn_blocking(list_nvidia_displays_inner)
            .await
            .map_err(|e| format!("枚举显示器失败: {}", e))?;

        // 重试机制：如果所有显示器分辨率都是 0，等待后重试一次
        if let Ok(ref displays) = result {
            if !displays.is_empty() && displays.iter().all(|d| d.current_width <= 0 || d.current_height <= 0) {
                log::warn!("list_nvidia_displays: 所有显示器分辨率无效，500ms 后重试…");
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let retry = tauri::async_runtime::spawn_blocking(list_nvidia_displays_inner)
                    .await
                    .map_err(|e| format!("枚举显示器重试失败: {}", e))?;
                if let Ok(ref retry_displays) = retry {
                    if retry_displays.iter().any(|d| d.current_width > 0 && d.current_height > 0) {
                        log::info!("list_nvidia_displays: 重试后成功获取到分辨率");
                        return retry;
                    }
                }
                log::warn!("list_nvidia_displays: 重试后分辨率仍未就绪，返回首次结果");
            }
        }

        result
    }
}

#[cfg(target_os = "windows")]
fn list_nvidia_displays_inner() -> Result<Vec<NvidiaDisplay>, String> {
    use std::mem;
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
        EnumDisplayDevicesW, DISPLAY_DEVICEW,
    };

    try_init_nvapi()?;

        // ── Step 1: Collect GDI monitor info ──
        struct GdiMonitor {
            device_name: String,
            monitor_name: String,
            is_primary: bool,
            width: i32,
            height: i32,
        }

        struct MonitorData {
            monitors: Vec<GdiMonitor>,
        }

        unsafe extern "system" fn monitor_enum_proc(
            hmonitor: HMONITOR,
            _hdc: HDC,
            _rect: *mut windows_sys::Win32::Foundation::RECT,
            lparam: isize,
        ) -> i32 {
            let data = &mut *(lparam as *mut MonitorData);
            let mut info: MONITORINFOEXW = mem::zeroed();
            info.monitorInfo.cbSize = mem::size_of::<MONITORINFOEXW>() as u32;

            if GetMonitorInfoW(hmonitor, &mut info as *mut _ as *mut _) != 0 {
                let device_name = String::from_utf16_lossy(
                    &info.szDevice[..info
                        .szDevice
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(info.szDevice.len())],
                );
                let is_primary = (info.monitorInfo.dwFlags & 1) != 0;
                let mut width = info.monitorInfo.rcMonitor.right
                    - info.monitorInfo.rcMonitor.left;
                let mut height = info.monitorInfo.rcMonitor.bottom
                    - info.monitorInfo.rcMonitor.top;

                // 分辨率无效时回退到 EnumDisplaySettingsW
                if width <= 0 || height <= 0 {
                    log::warn!(
                        "nvapi::list_nvidia_displays: monitor[{}] GetMonitorInfoW 分辨率无效 ({}x{}), 尝试 EnumDisplaySettingsW 回退",
                        data.monitors.len(), width, height
                    );
                    use windows_sys::Win32::Graphics::Gdi::{EnumDisplaySettingsW, DEVMODEW, ENUM_CURRENT_SETTINGS};
                    // 尝试原设备名和去掉 \\?\ 前缀的格式
                    let name_attempts = [
                        device_name.clone(),
                        device_name.trim_start_matches("\\\\.\\").to_string(),
                    ];
                    for attempt in &name_attempts {
                        if attempt.is_empty() { continue; }
                        let wide: Vec<u16> = attempt.encode_utf16().chain(std::iter::once(0)).collect();
                        let mut dm: DEVMODEW = std::mem::zeroed();
                        dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
                        if EnumDisplaySettingsW(wide.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) != 0 {
                            let fw = dm.dmPelsWidth as i32;
                            let fh = dm.dmPelsHeight as i32;
                            if fw > 0 && fh > 0 {
                                width = fw;
                                height = fh;
                                log::info!("nvapi::list_nvidia_displays: EnumDisplaySettingsW 回退成功 device={} ({}x{})", attempt, fw, fh);
                                break;
                            }
                        }
                    }
                }

                let monitor_name = {
                    let wide: Vec<u16> =
                        device_name.encode_utf16().chain(std::iter::once(0)).collect();
                    let mut dd: DISPLAY_DEVICEW = mem::zeroed();
                    dd.cb = mem::size_of::<DISPLAY_DEVICEW>() as u32;
                    if EnumDisplayDevicesW(wide.as_ptr(), 0, &mut dd, 0) != 0 {
                        let len = dd.DeviceString
                            .iter()
                            .position(|&c| c == 0)
                            .unwrap_or(dd.DeviceString.len());
                        if len > 0 {
                            let model = String::from_utf16_lossy(&dd.DeviceString[..len]);
                            let t = model.trim();
                            if !t.is_empty() && !is_generic_monitor_name(t) {
                                t.to_string()
                            } else {
                                device_name.trim_start_matches("\\\\.\\").to_string()
                            }
                        } else {
                            device_name.trim_start_matches("\\\\.\\").to_string()
                        }
                    } else {
                        device_name.trim_start_matches("\\\\.\\").to_string()
                    }
                };

                data.monitors.push(GdiMonitor {
                    device_name,
                    monitor_name,
                    is_primary,
                    width,
                    height,
                });
            }
            1
        }

        let mut data = MonitorData {
            monitors: Vec::new(),
        };
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(monitor_enum_proc),
                &mut data as *mut _ as isize,
            );
        }

        if data.monitors.is_empty() {
            return Err("未检测到任何显示器".to_string());
        }

        // ── Step 2: Get real NVAPI displayIds ──
        // 优先通过 NvAPI_DISP_GetDisplayIdByDisplayName 按设备名精确匹配，
        // 避免 GDI 与 NVAPI 枚举顺序不一致导致 displayId 错配（A 的 ID 配到 B，
        // 应用分辨率时会把 A 的宽高设置到 B 上）。
        let name_resolved_ids: Vec<NvU32> = {
            let mut ok = true;
            let mut ids: Vec<NvU32> = Vec::new();
            for m in &data.monitors {
                let ansi: Vec<u8> = m.device_name.bytes().chain(std::iter::once(0)).collect();
                let mut id: NvU32 = 0;
                let s = unsafe { (nvapi_dyn()?.get_display_id_by_display_name)(ansi.as_ptr(), &mut id) };
                if s != NVAPI_OK || id == 0 {
                    log::warn!(
                        "GetDisplayIdByDisplayName({}) 失败: status={}, 回退到 GetDisplayConfig 顺序匹配",
                        m.device_name, s
                    );
                    ok = false;
                    break;
                }
                ids.push(id);
            }
            if ok { ids } else { Vec::new() }
        };

        let nvapi_display_ids: Vec<NvU32> = if name_resolved_ids.is_empty() {
            // 回退：通过 GetDisplayConfig 按顺序获取 displayId
            unsafe {
            let mut path_info_count: NvU32 = 0;
            let mut status =
                (nvapi_dyn()?.get_display_config)(&mut path_info_count, std::ptr::null_mut());
            if status != NVAPI_OK || path_info_count == 0 {
                log::warn!("GetDisplayConfig pass1 failed: status={}", status);
                return Ok(data
                    .monitors
                    .into_iter()
                    .enumerate()
                    .map(|(i, m)| NvidiaDisplay {
                        display_id: i as NvU32,
                        device_name: m.device_name,
                        monitor_name: m.monitor_name,
                        is_primary: m.is_primary,
                        current_width: m.width,
                        current_height: m.height,
                    })
                    .collect());
            }

            // Pass 2: allocate sourceModeInfo
            let mut source_mode_infos: Vec<NvDisplayConfigSourceModeInfo> =
                (0..path_info_count as usize)
                    .map(|_| {
                        let mut sm: NvDisplayConfigSourceModeInfo = mem::zeroed();
                        sm.colorFormat = 0;
                        sm
                    })
                    .collect();

            let mut path_infos: Vec<NvDisplayConfigPathInfo> = (0..path_info_count as usize)
                .map(|i| {
                    let mut p: NvDisplayConfigPathInfo = mem::zeroed();
                    p.version = NV_DISPLAYCONFIG_PATH_INFO_VER;
                    p.sourceModeInfo = &mut source_mode_infos[i];
                    p
                })
                .collect();

            status = (nvapi_dyn()?.get_display_config)(&mut path_info_count, path_infos.as_mut_ptr());
            if status != NVAPI_OK {
                log::warn!("GetDisplayConfig pass2 failed: status={}", status);
                return Ok(data
                    .monitors
                    .into_iter()
                    .enumerate()
                    .map(|(i, m)| NvidiaDisplay {
                        display_id: i as NvU32,
                        device_name: m.device_name,
                        monitor_name: m.monitor_name,
                        is_primary: m.is_primary,
                        current_width: m.width,
                        current_height: m.height,
                    })
                    .collect());
            }

            // Pass 3: allocate targetInfo
            let mut target_allocations: Vec<Vec<NvDisplayConfigPathTargetInfo>> = Vec::new();
            for path in &mut path_infos {
                let count = path.targetInfoCount as usize;
                if count > 0 {
                    let targets: Vec<NvDisplayConfigPathTargetInfo> =
                        (0..count).map(|_| mem::zeroed()).collect();
                    path.targetInfo = targets.as_ptr() as *mut NvDisplayConfigPathTargetInfo;
                    target_allocations.push(targets);
                } else {
                    path.targetInfo = std::ptr::null_mut();
                }
            }

            status = (nvapi_dyn()?.get_display_config)(&mut path_info_count, path_infos.as_mut_ptr());
            if status != NVAPI_OK {
                log::warn!("GetDisplayConfig pass3 failed: status={}", status);
                return Ok(data
                    .monitors
                    .into_iter()
                    .enumerate()
                    .map(|(i, m)| NvidiaDisplay {
                        display_id: i as NvU32,
                        device_name: m.device_name,
                        monitor_name: m.monitor_name,
                        is_primary: m.is_primary,
                        current_width: m.width,
                        current_height: m.height,
                    })
                    .collect());
            }

            // Extract displayIds from paths (first targetInfo per path)
            path_infos
                .iter()
                .map(|p| {
                    if p.targetInfoCount > 0 && !p.targetInfo.is_null() {
                        (*p.targetInfo).displayId
                    } else {
                        0
                    }
                })
                .collect()
            }
        } else {
            log::info!(
                "list_nvidia_displays: 按设备名精确匹配 displayId 成功: {:?}",
                name_resolved_ids
            );
            name_resolved_ids
        };

        log::info!(
            "list_nvidia_displays: {} GDI monitors, display_ids={:?}",
            data.monitors.len(),
            nvapi_display_ids
        );

        // ── Step 3: Combine GDI info with real NVAPI displayIds ──
        // displayId 已优先按设备名精确匹配；仅当按名解析失败时才按顺序
        // (GDI monitor N → NVAPI path N) 对齐。
        let mut displays: Vec<NvidiaDisplay> = data
            .monitors
            .into_iter()
            .enumerate()
            .map(|(i, m)| {
                let display_id = nvapi_display_ids.get(i).copied().unwrap_or(i as NvU32);
                NvidiaDisplay {
                    display_id,
                    device_name: m.device_name,
                    monitor_name: m.monitor_name,
                    is_primary: m.is_primary,
                    current_width: m.width,
                    current_height: m.height,
                }
            })
            .collect();

        // EDID 回退：如果 EnumDisplayDevicesW 返回通用名称，
        // monitor_name 会退化为 GDI 设备名（如 DISPLAY1）。
        // 按 PNP ID 精确匹配 EDID 型号，避免注册表 EDID 枚举顺序与 GDI
        // 枚举顺序不一致时把 A 显示器的型号错配到 B 显示器（型号颠倒）。
        let has_fallback = displays.iter().any(|d| {
            let stripped = d.device_name.trim_start_matches("\\\\.\\");
            d.monitor_name == stripped
        });
        if has_fallback {
            let edid_by_pnp = crate::display_cache::get_edid_monitor_names_by_pnpid();
            if !edid_by_pnp.is_empty() {
                for d in displays.iter_mut() {
                    let stripped = d.device_name.trim_start_matches("\\\\.\\");
                    if d.monitor_name == stripped {
                        if let Some(pnp_id) =
                            crate::display_cache::get_pnp_id_for_device(&d.device_name)
                        {
                            if let Some(name) = edid_by_pnp.get(&pnp_id) {
                                if !name.is_empty() {
                                    d.monitor_name = name.clone();
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(displays)
}

/// 通过 NVAPI 驱动层枚举指定显示器支持的所有模式（分辨率 + 刷新率）。
///
/// GDI 的 `EnumDisplaySettingsW` 在某些驱动/显示器上返回的模式列表不完整
/// （缺少 1920x1440 等非推荐分辨率），而 NVIDIA 控制面板正是通过
/// `NvAPI_DISP_EnumDisplayModes` 列出模式的，因此用它做补充。
///
/// 该函数由运行时的 nvapi64.dll 导出（SDK 附带的 .lib 未导出），
/// 这里通过 libloading 动态加载，失败时安全降级（不影响 GDI 结果）。
#[cfg(target_os = "windows")]
fn enum_nvapi_display_modes(display_id: NvU32) -> Result<Vec<DisplayMode>, String> {
    use libloading::Library;

    try_init_nvapi()?;

    let candidates = find_nvapi64_candidates();
    let path = candidates
        .first()
        .map(|(p, _)| p.clone())
        .ok_or_else(|| "未找到 nvapi64.dll".to_string())?;

    // SAFETY: NvAPI_DISP_EnumDisplayModes 是 NVAPI 公开导出函数，
    // 存在于运行时的完整版 nvapi64.dll 中。
    let lib = unsafe { Library::new(&path) }
        .map_err(|e| format!("加载 nvapi64.dll 失败: {}", e))?;
    let func: libloading::Symbol<
        unsafe extern "C" fn(NvU32, NvU32, *mut NvModeTiming, *mut NvU32) -> NvAPI_Status,
    > = unsafe { lib.get(b"NvAPI_DISP_EnumDisplayModes") }
        .map_err(|e| format!("nvapi64.dll 中未找到 NvAPI_DISP_EnumDisplayModes: {}", e))?;

    // 第一遍：获取模式数量
    let mut count: NvU32 = 0;
    let status = unsafe { func(display_id, 0, std::ptr::null_mut(), &mut count) };
    if status != NVAPI_OK || count == 0 || count > 1000 {
        return Err(format!(
            "NvAPI_DISP_EnumDisplayModes 失败: {} (code {})",
            nvapi_error_string(status),
            status
        ));
    }

    // 第二遍：填充模式数组
    let mut buffer: Vec<NvModeTiming> = vec![unsafe { std::mem::zeroed() }; count as usize];
    let mut filled: NvU32 = count;
    let status = unsafe { func(display_id, 0, buffer.as_mut_ptr(), &mut filled) };
    if status != NVAPI_OK {
        return Err(format!(
            "NvAPI_DISP_EnumDisplayModes(第 2 次调用) 失败: {} (code {})",
            nvapi_error_string(status),
            status
        ));
    }

    let mut modes = Vec::new();
    for m in buffer.into_iter().take(filled as usize) {
        let t = &m.mode_info.timing;
        let width = t.h_visible as NvU32;
        let height = t.v_visible as NvU32;
        if width == 0 || height == 0 {
            continue;
        }
        // rrx1k 单位为 0.001 Hz，除以 1000 得到 Hz；异常时回退到 rr 字段
        let rr = if t.etc.rrx1k != 0 {
            t.etc.rrx1k as f64 / 1000.0
        } else {
            t.etc.rr as f64
        };
        modes.push(DisplayMode {
            width,
            height,
            refresh_rate: rr,
            is_current: false,
        });
    }

    log::info!(
        "NVAPI EnumDisplayModes: displayId={}, 共 {} 个模式",
        display_id,
        modes.len()
    );
    Ok(modes)
}

/// Enumerate all available display modes (resolution + refresh rate) for a given GDI device name.
#[tauri::command]
pub fn get_nvidia_display_modes(device_name: String) -> Result<Vec<DisplayMode>, String> {
    #[cfg(not(target_os = "windows"))]
    {
        return Err("此功能仅支持 Windows 系统".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        use std::mem;
        use windows_sys::Win32::Graphics::Gdi::{
            EnumDisplaySettingsW, DEVMODEW, ENUM_CURRENT_SETTINGS,
        };

        // 有些系统上 EnumDisplaySettingsW 不接受 "\\\\.\\DISPLAY1" 格式，
        // 必须回退到 "DISPLAY1"（与 list_nvidia_displays 中的回退逻辑一致）。
        let candidates: Vec<String> = {
            let mut v = vec![device_name.clone()];
            let stripped = device_name.trim_start_matches("\\\\.\\").to_string();
            if stripped != device_name {
                v.push(stripped);
            }
            v
        };

        // 先读取当前设置，用于标记 is_current，并在枚举列表缺失当前模式时补入，
        // 保证当前分辨率始终可选、刷新率调节始终可用。
        let current: Option<DisplayMode> = (|| {
            for name in &candidates {
                let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
                let mut dm: DEVMODEW = unsafe { mem::zeroed() };
                dm.dmSize = mem::size_of::<DEVMODEW>() as u16;
                if unsafe { EnumDisplaySettingsW(wide.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) }
                    != 0
                {
                    let freq_raw = dm.dmDisplayFrequency as f64;
                    // 真实高刷(144/240/360Hz)直接返回整数 Hz，只有非整数刷新率
                    // (如 59.94Hz) 才编码为 x100 (5994)。> 1000 才是编码值。
                    let refresh_rate = if freq_raw > 1000.0 {
                        freq_raw / 100.0
                    } else {
                        freq_raw
                    };
                    return Some(DisplayMode {
                        width: dm.dmPelsWidth,
                        height: dm.dmPelsHeight,
                        refresh_rate,
                        is_current: true,
                    });
                }
            }
            None
        })();

        let mut modes: Vec<DisplayMode> = Vec::new();
        // 枚举所有候选设备名格式，并把结果合并起来：
        // 部分系统上带 "\\\\.\\" 前缀的 EnumDisplaySettingsW 返回的模式列表
        // 可能不完整（缺少 1920x1440 等非推荐分辨率），需要叠加无前缀格式
        // 的枚举结果，确保与 Windows 显示设置里的列表一致。
        for name in &candidates {
            let wide_name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let mut mode_num: u32 = 0;

            loop {
                let mut dev_mode: DEVMODEW = unsafe { mem::zeroed() };
                dev_mode.dmSize = mem::size_of::<DEVMODEW>() as u16;

                let result =
                    unsafe { EnumDisplaySettingsW(wide_name.as_ptr(), mode_num, &mut dev_mode) };
                if result == 0 {
                    break;
                }

                // Calculate effective refresh rate
                // dmDisplayFrequency 真实高刷(144/240/360Hz)直接返回整数 Hz，
                // 只有非整数刷新率(如 5995 → 59.95Hz)才编码为 x100。> 1000 才是编码值。
                let freq_raw = dev_mode.dmDisplayFrequency as f64;
                let refresh_rate = if freq_raw > 1000.0 {
                    // x100 encoded (e.g., 5995 = 59.95 Hz)
                    freq_raw / 100.0
                } else {
                    freq_raw
                };

                let m = DisplayMode {
                    width: dev_mode.dmPelsWidth,
                    height: dev_mode.dmPelsHeight,
                    refresh_rate,
                    is_current: false,
                };
                // 合并去重：同一显示器的不同设备名格式可能枚举到相同模式
                if !modes.iter().any(|e| {
                    e.width == m.width
                        && e.height == m.height
                        && (e.refresh_rate - m.refresh_rate).abs() < 0.5
                }) {
                    modes.push(m);
                }

                mode_num += 1;
                if mode_num > 300 {
                    break;
                }
            }
        }

        // 再用 NVAPI 驱动层枚举补充 GDI 缺失的模式（如 1920x1440 等
        // 非推荐分辨率），合并去重。失败时静默忽略，不影响 GDI 结果。
        {
            let ansi: Vec<u8> = device_name.bytes().chain(std::iter::once(0)).collect();
            let mut display_id: NvU32 = 0;
            if unsafe { (nvapi_dyn()?.get_display_id_by_display_name)(ansi.as_ptr(), &mut display_id) }
                == NVAPI_OK
                && display_id != 0
            {
                if let Ok(nv_modes) = enum_nvapi_display_modes(display_id) {
                    for m in nv_modes {
                        if !modes.iter().any(|e| {
                            e.width == m.width
                                && e.height == m.height
                                && (e.refresh_rate - m.refresh_rate).abs() < 0.5
                        }) {
                            modes.push(m);
                        }
                    }
                }
            }
        }

        // 枚举列表可能不含当前模式（部分驱动/自定义分辨率），手动补上，
        // 保证当前分辨率始终可选、刷新率调节始终可用。
        if let Some(cur) = &current {
            if !modes.iter().any(|m| {
                m.width == cur.width
                    && m.height == cur.height
                    && (m.refresh_rate - cur.refresh_rate).abs() < 0.5
            }) {
                modes.push(cur.clone());
            }
        }

        if modes.is_empty() {
            return Err(format!("无法枚举显示器 {} 的模式", device_name));
        }

        // Sort: resolution (W*H) desc, then refresh rate desc
        modes.sort_by(|a, b| {
            let area_a = a.width * a.height;
            let area_b = b.width * b.height;
            area_b
                .cmp(&area_a)
                .then(
                    b.refresh_rate
                        .partial_cmp(&a.refresh_rate)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        });

        // Dedup: same (width, height, approx refresh rate)
        modes.dedup_by(|a, b| {
            a.width == b.width
                && a.height == b.height
                && (a.refresh_rate - b.refresh_rate).abs() < 0.5
        });

        // 标记当前使用的模式
        if let Some(cur) = &current {
            for m in modes.iter_mut() {
                if m.width == cur.width
                    && m.height == cur.height
                    && (m.refresh_rate - cur.refresh_rate).abs() < 0.5
                {
                    m.is_current = true;
                }
            }
        }

        Ok(modes)
    }
}

// ---------------------------------------------------------------------------
// NV_Modes registry injection helpers
// ---------------------------------------------------------------------------

/// Inject a custom resolution into all NVIDIA NV_Modes registry keys
/// under HKLM\SYSTEM\CurrentControlSet\Control\Video\.
///
/// Format: "{width}x{height}x{bpp}-{RRx1000}"
fn inject_nv_modes_registry(device_name: &str, width: u32, height: u32, refresh_rate: Option<f64>) -> Result<Vec<String>, String> {
    use std::mem;
    use winreg::enums::*;
    use winreg::RegKey;

    // Calculate NV_Modes refresh rate suffix (RR * 1000)
    let refresh_rrx1k: u32 = if let Some(rr) = refresh_rate {
        // Use specified refresh rate: 60.0 → 60000, 144.0 → 144000, 59.95 → 59950
        (rr * 1000.0).round() as u32
    } else {
        // Auto-detect current refresh rate via GDI
        unsafe {
            let wide: Vec<u16> = device_name.encode_utf16().chain(std::iter::once(0)).collect();
            let mut dm: windows_sys::Win32::Graphics::Gdi::DEVMODEW = mem::zeroed();
            dm.dmSize = mem::size_of_val(&dm) as u16;
            if windows_sys::Win32::Graphics::Gdi::EnumDisplaySettingsW(
                wide.as_ptr(), 0xFFFFFFFF, &mut dm,
            ) != 0
            {
                // 真实高刷(144/240/360Hz)返回整数 Hz，只有非整数刷新率(如 5995=59.95Hz)
                // 才编码为 x100。> 1000 才是编码值，统一换算成 rrx1k(Hz*1000)。
                let freq_raw = dm.dmDisplayFrequency as f64;
                let hz = if freq_raw > 1000.0 { freq_raw / 100.0 } else { freq_raw };
                (hz * 1000.0).round() as u32
            } else {
                144000
            }
        }
    };

    let entry = format!("{}x{}x32-{}", width, height, refresh_rrx1k);
    let video_path = r"SYSTEM\CurrentControlSet\Control\Video";
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);

    let video_key = hklm.open_subkey_with_flags(video_path, KEY_READ)
        .map_err(|e| format!("无法打开注册表路径 {}: {}", video_path, e))?;

    let mut injected: Vec<String> = Vec::new();

    for adapter in video_key.enum_keys().filter_map(|r| r.ok()) {
        let adapter_key = match hklm.open_subkey_with_flags(
            &format!(r"{}\{}", video_path, adapter), KEY_READ,
        ) {
            Ok(k) => k,
            Err(_) => continue,
        };

        for target in adapter_key.enum_keys().filter_map(|r| r.ok()) {
            let target_path = format!(r"{}\{}\{}", video_path, adapter, target);
            let target_key = match hklm.open_subkey_with_flags(
                &target_path, KEY_READ | KEY_WRITE,
            ) {
                Ok(k) => k,
                Err(_) => continue,
            };

            // Check if NV_Modes exists (REG_MULTI_SZ)
            let has_modes = target_key.get_value::<Vec<String>, _>("NV_Modes").is_ok();
            if !has_modes {
                // Try REG_SZ format
                let has_sz = target_key.get_value::<String, _>("NV_Modes").is_ok();
                if !has_sz {
                    continue;
                }
            }

            let mut modes: Vec<String> = target_key.get_value("NV_Modes").unwrap_or_default();
            // Remove stale entry for the same base resolution
            modes.retain(|m| {
                let prefix = format!("{}x{}", width, height);
                !m.starts_with(&prefix)
            });
            // Add our entry if not already present
            if !modes.contains(&entry) {
                modes.push(entry.clone());
            }

            target_key.set_value("NV_Modes", &modes)
                .map_err(|e| format!("写入 NV_Modes 到 {} 失败: {}", target_path, e))?;

            log::info!("已注入分辨率到 NV_Modes: {}  (target={})", entry, target);
            injected.push(target_path);
        }
    }

    if injected.is_empty() {
        // NV_Modes doesn't exist yet — create it under the first suitable NVIDIA target
        for adapter in video_key.enum_keys().filter_map(|r| r.ok()) {
            let adapter_key = match hklm.open_subkey_with_flags(
                &format!(r"{}\{}", video_path, adapter), KEY_READ,
            ) {
                Ok(k) => k,
                Err(_) => continue,
            };

            for target in adapter_key.enum_keys().filter_map(|r| r.ok()) {
                let target_path = format!(r"{}\{}\{}", video_path, adapter, target);
                // Skip known non-target subkeys
                if target == "0000" || target == "Connectivity" || target == "DAL" {
                    continue;
                }
                let target_key = match hklm.create_subkey(&target_path) {
                    Ok((k, _)) => k,
                    Err(_) => continue,
                };
                let modes: Vec<String> = vec![entry.clone()];
                target_key.set_value("NV_Modes", &modes)
                    .map_err(|e| format!("创建 NV_Modes 到 {} 失败: {}", target_path, e))?;
                log::info!("已创建并注入 NV_Modes: {}  (target={})", entry, target);
                injected.push(target_path);
                break;
            }
            if !injected.is_empty() {
                break;
            }
        }
    }

    if injected.is_empty() {
        Err("未找到 NV_Modes 注册表项，且无法创建".to_string())
    } else {
        Ok(injected)
    }
}

/// Re-run NvAPI_DISP_SetDisplayConfig (after NV_Modes injection)
unsafe fn retry_set_display_config(
    display_id: NvU32,
    width: NvU32,
    height: NvU32,
) -> Result<(), String> {
    use std::mem;

    let mut path_info_count: NvU32 = 0;
    let mut s = (nvapi_dyn()?.get_display_config)(&mut path_info_count, std::ptr::null_mut());
    if s != NVAPI_OK || path_info_count == 0 {
        return Err("retry: GetDisplayConfig 失败".to_string());
    }

    let mut source_mode_infos: Vec<NvDisplayConfigSourceModeInfo> = (0..path_info_count as usize)
        .map(|_| {
            let mut sm: NvDisplayConfigSourceModeInfo = mem::zeroed();
            sm.colorFormat = 0;
            sm
        })
        .collect();

    let mut path_infos: Vec<NvDisplayConfigPathInfo> = (0..path_info_count as usize)
        .map(|i| {
            let mut p: NvDisplayConfigPathInfo = mem::zeroed();
            p.version = NV_DISPLAYCONFIG_PATH_INFO_VER;
            p.sourceModeInfo = &mut source_mode_infos[i];
            p
        })
        .collect();

    s = (nvapi_dyn()?.get_display_config)(&mut path_info_count, path_infos.as_mut_ptr());
    if s != NVAPI_OK {
        return Err("retry: GetDisplayConfig pass2 失败".to_string());
    }

    // Allocate targetInfo
    let mut target_allocations: Vec<Vec<NvDisplayConfigPathTargetInfo>> = Vec::new();
    for path in &mut path_infos {
        let count = path.targetInfoCount as usize;
        if count > 0 {
            let targets: Vec<NvDisplayConfigPathTargetInfo> =
                (0..count).map(|_| mem::zeroed()).collect();
            path.targetInfo = targets.as_ptr() as *mut NvDisplayConfigPathTargetInfo;
            target_allocations.push(targets);
        } else {
            path.targetInfo = std::ptr::null_mut();
        }
    }

    s = (nvapi_dyn()?.get_display_config)(&mut path_info_count, path_infos.as_mut_ptr());
    if s != NVAPI_OK {
        return Err("retry: GetDisplayConfig pass3 失败".to_string());
    }

    // Find path for our display_id
    let target_idx = match path_infos.iter().position(|path| {
        if path.targetInfoCount > 0 && !path.targetInfo.is_null() {
            let targets = std::slice::from_raw_parts(path.targetInfo, path.targetInfoCount as usize);
            targets.iter().any(|t| t.displayId == display_id)
        } else {
            false
        }
    }) {
        Some(i) => i,
        None => return Err("retry: 未找到 displayId".to_string()),
    };

    let path = &mut path_infos[target_idx];
    let mode_info = match path.sourceModeInfo.as_mut() {
        Some(m) => m,
        None => return Err("retry: 无 source mode info".to_string()),
    };
    mode_info.resolution.width = width;
    mode_info.resolution.height = height;

    let flags = NV_DISPLAYCONFIG_SAVE_TO_PERSISTENCE | NV_FORCE_COMMIT_VIDPN;
    s = (nvapi_dyn()?.set_display_config)(path_info_count, path_infos.as_ptr(), flags);
    if s == NVAPI_OK {
        log::info!("retry: SetDisplayConfig 成功设置 {}x{}", width, height);
        Ok(())
    } else {
        Err(format!("retry: SetDisplayConfig 失败: {} (code {})", nvapi_error_string(s), s))
    }
}

/// Get the list of resolutions that have been injected into NV_Modes registry.
#[tauri::command]
pub fn get_injected_resolutions() -> Vec<InjectedResolution> {
    load_injected_resolutions()
}

/// Remove a resolution from the injected resolutions list.
#[tauri::command]
pub fn remove_injected_resolution(width: u32, height: u32) -> Result<(), String> {
    let path = injected_resolutions_path();
    let mut list = load_injected_resolutions();
    let before = list.len();
    list.retain(|r| r.width != width || r.height != height);
    if list.len() == before {
        return Ok(()); // nothing to remove
    }
    std::fs::write(
        &path,
        serde_json::to_string(&list).map_err(|e| format!("序列化失败: {}", e))?,
    )
    .map_err(|e| format!("写入失败: {}", e))?;
    Ok(())
}

/// Set display resolution using NVAPI DISP API.
///
/// Steps (3-tier fallback):
/// 1. Try NvAPI_DISP_SetDisplayConfig (works for EDID-listed modes)
/// 2. Inject into NVIDIA NV_Modes registry + ChangeDisplaySettingsExW (custom/non-EDID modes)
/// 3. Retry SetDisplayConfig after injection
#[tauri::command]
pub fn set_nvidia_display_resolution(
    display_id: NvU32,
    width: NvU32,
    height: NvU32,
    device_name: String,
    refresh_rate: Option<f64>,
) -> Result<SetResolutionResult, String> {
    try_init_nvapi()?;

    if width == 0 || height == 0 {
        return Err("分辨率宽高不能为 0".to_string());
    }

    // Validate refresh_rate if provided
    if let Some(rr) = refresh_rate {
        if rr <= 0.0 || rr > 1000.0 {
            return Err(format!("无效的刷新率: {} Hz", rr));
        }
        log::info!(
            "设置分辨率 {}x{} @ {}Hz (displayId={})",
            width, height, rr, display_id
        );
    }

    #[cfg(not(target_os = "windows"))]
    {
        return Err("此功能仅支持 Windows 系统".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        use std::mem;
        use windows_sys::Win32::Graphics::Gdi::{
            ChangeDisplaySettingsExW, EnumDisplaySettingsW, DEVMODEW,
        };

        // ── Strategy 1: Try NVAPI SetDisplayConfig (works for EDID-listed modes) ──
        let nvapi_err = 's1: {
            let mut path_info_count: NvU32 = 0;
            let status;
            unsafe {
                // ── Pass 1: Get pathInfoCount ──
                status =
                    (nvapi_dyn()?.get_display_config)(&mut path_info_count, std::ptr::null_mut());
            }
            if status != NVAPI_OK {
                break 's1 format!(
                    "获取显示配置失败: {} (code {})",
                    nvapi_error_string(status),
                    status
                );
            }
            if path_info_count == 0 {
                break 's1 "没有活动的显示路径".to_string();
            }

            // ── Pass 2: Allocate pathInfo + sourceModeInfo arrays ──
            let mut source_mode_infos: Vec<NvDisplayConfigSourceModeInfo> = (0..path_info_count
                as usize)
                .map(|_| {
                    let mut sm: NvDisplayConfigSourceModeInfo;
                    unsafe { sm = mem::zeroed(); }
                    sm.colorFormat = 0; // NV_FORMAT_UNKNOWN
                    sm
                })
                .collect();

            let mut path_infos: Vec<NvDisplayConfigPathInfo> = (0..path_info_count as usize)
                .map(|i| {
                    let mut p: NvDisplayConfigPathInfo;
                    unsafe { p = mem::zeroed(); }
                    p.version = NV_DISPLAYCONFIG_PATH_INFO_VER;
                    p.sourceModeInfo = &mut source_mode_infos[i];
                    p
                })
                .collect();

            unsafe {
                let s = (nvapi_dyn()?.get_display_config)(
                    &mut path_info_count,
                    path_infos.as_mut_ptr(),
                );
                if s != NVAPI_OK {
                    break 's1 format!(
                        "获取显示配置失败(Pass 2): {} (code {})",
                        nvapi_error_string(s),
                        s
                    );
                }
            }

            // ── Pass 3: Allocate targetInfo arrays (required to get displayId) ──
            let mut target_allocations: Vec<Vec<NvDisplayConfigPathTargetInfo>> = Vec::new();
            for path in &mut path_infos {
                let count = path.targetInfoCount as usize;
                if count > 0 {
                    let targets: Vec<NvDisplayConfigPathTargetInfo> = (0..count)
                        .map(|_| unsafe { mem::zeroed() })
                        .collect();
                    path.targetInfo = targets.as_ptr() as *mut NvDisplayConfigPathTargetInfo;
                    target_allocations.push(targets);
                } else {
                    path.targetInfo = std::ptr::null_mut();
                }
            }

            unsafe {
                let s = (nvapi_dyn()?.get_display_config)(
                    &mut path_info_count,
                    path_infos.as_mut_ptr(),
                );
                if s != NVAPI_OK {
                    break 's1 format!(
                        "获取显示配置失败(Pass 3): {} (code {})",
                        nvapi_error_string(s),
                        s
                    );
                }
            }

            // ── Find the path containing our display_id ──
            let available_ids: Vec<NvU32> = path_infos
                .iter()
                .filter(|p| p.targetInfoCount > 0 && !p.targetInfo.is_null())
                .flat_map(|p| {
                    let targets = unsafe {
                        std::slice::from_raw_parts(p.targetInfo, p.targetInfoCount as usize)
                    };
                    targets.iter().map(|t| t.displayId).collect::<Vec<_>>()
                })
                .collect();
            log::info!(
                "NVIDIA display config: path_count={}, available_display_ids={:?}, searching for {}",
                path_info_count,
                available_ids,
                display_id
            );
            let target_path_idx = match path_infos.iter().position(|path| {
                if path.targetInfoCount > 0 && !path.targetInfo.is_null() {
                    let targets = unsafe {
                        std::slice::from_raw_parts(path.targetInfo, path.targetInfoCount as usize)
                    };
                    targets.iter().any(|t| t.displayId == display_id)
                } else {
                    false
                }
            }) {
                Some(idx) => idx,
                None => {
                    break 's1 format!("未找到 displayId {} 的显示路径", display_id);
                }
            };

            // ── Modify sourceModeInfo resolution ──
            let path = &mut path_infos[target_path_idx];
            let mode_info = match unsafe { path.sourceModeInfo.as_mut() } {
                Some(m) => m,
                None => {
                    break 's1 "目标路径没有 source mode info".to_string();
                }
            };
            let old_w = mode_info.resolution.width;
            let old_h = mode_info.resolution.height;

            log::info!(
                "NVIDIA 分辨率变更: displayId={}, {}x{} → {}x{}",
                display_id,
                old_w,
                old_h,
                width,
                height
            );

            mode_info.resolution.width = width;
            mode_info.resolution.height = height;

            // ── Apply via NVAPI ──
            let flags = NV_DISPLAYCONFIG_SAVE_TO_PERSISTENCE | NV_FORCE_COMMIT_VIDPN;
            let s = unsafe {
                (nvapi_dyn()?.set_display_config)(
                    path_info_count,
                    path_infos.as_ptr(),
                    flags,
                )
            };
            if s == NVAPI_OK {
                if refresh_rate.is_none() {
                    // No refresh rate to set — done
                    log::info!("NVIDIA 分辨率已成功设为 {}x{} (NVAPI)", width, height);
                    return Ok(SetResolutionResult { applied: true, injected: false });
                }
                // NVAPI can't set refresh rate (NvDisplayConfigSourceModeInfo has no frequency field).
                // Fall through to ChangeDisplaySettingsExW to apply the refresh rate.
                log::info!(
                    "NVAPI 分辨率设置成功，继续通过 ChangeDisplaySettingsExW 设置刷新率 {} Hz",
                    refresh_rate.unwrap()
                );
                break 's1 String::new(); // empty = NVAPI success, fall through for refresh rate
            }

            // NVAPI SetDisplayConfig failed — save error for fallback
            format!(
                "{} (code {})",
                nvapi_error_string(s),
                s
            )
        };

        // ── Strategy 2: ChangeDisplaySettingsExW (with optional NV_Modes injection) ──
        let nvapi_succeeded = nvapi_err.is_empty();
        if !nvapi_succeeded {
            log::warn!(
                "NVAPI SetDisplayConfig 失败 ({}), 尝试注入 NV_Modes 注册表",
                nvapi_err
            );
        }

        let reg_inject_err: Option<String> = if nvapi_succeeded {
            // NVAPI already set the resolution successfully; no need to inject NV_Modes.
            // Just need ChangeDisplaySettingsExW to apply the refresh rate.
            None
        } else {
            match inject_nv_modes_registry(&device_name, width, height, refresh_rate) {
                Ok(keys) => {
                    log::info!("NV_Modes 注册表注入成功: {} 个注册表项", keys.len());
                    None
                }
                Err(e) => {
                    log::error!("NV_Modes 注册表注入失败: {}", e);
                    Some(e)
                }
            }
        };

        unsafe {
            let wide_name: Vec<u16> =
                device_name.encode_utf16().chain(std::iter::once(0)).collect();

            // Get current display mode for dm struct
            const ENUM_CURRENT_SETTINGS: u32 = 0xFFFFFFFF;
            let mut dm: DEVMODEW = mem::zeroed();
            dm.dmSize = mem::size_of::<DEVMODEW>() as u16;
            let _enum_ok =
                EnumDisplaySettingsW(wide_name.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm);

            // Set target resolution and optionally refresh rate
            const DM_PELSWIDTH: u32 = 0x00080000;
            const DM_PELSHEIGHT: u32 = 0x00100000;
            const DM_DISPLAYFREQUENCY: u32 = 0x00400000;
            dm.dmPelsWidth = width;
            dm.dmPelsHeight = height;
            let mut dm_fields = DM_PELSWIDTH | DM_PELSHEIGHT;

            if let Some(rr) = refresh_rate {
                // 优先从 Windows 枚举的模式列表里找到 (width,height,rr) 完全匹配的
                // 原始 dmDisplayFrequency，确保与 Windows 显示设置里的值一致。
                // 找不到时回退：整数刷新率(60/144/240Hz)直接以 Hz 写入；
                // 非整数(如 59.95Hz)按 x100 编码写入(5995)。
                let mut freq: u32 = if (rr.fract()).abs() < f64::EPSILON {
                    rr.round() as u32
                } else {
                    (rr * 100.0).round() as u32
                };
                let mut mode_num: u32 = 0;
                while mode_num < 300 {
                    let mut dev_mode: DEVMODEW = mem::zeroed();
                    dev_mode.dmSize = mem::size_of::<DEVMODEW>() as u16;
                    if EnumDisplaySettingsW(wide_name.as_ptr(), mode_num, &mut dev_mode) == 0 {
                        break;
                    }
                    if dev_mode.dmPelsWidth == width && dev_mode.dmPelsHeight == height {
                        let raw = dev_mode.dmDisplayFrequency as f64;
                        let raw_hz = if raw > 1000.0 { raw / 100.0 } else { raw };
                        if (raw_hz - rr).abs() < 0.5 {
                            freq = dev_mode.dmDisplayFrequency;
                            break;
                        }
                    }
                    mode_num += 1;
                }
                dm.dmDisplayFrequency = freq;
                dm_fields |= DM_DISPLAYFREQUENCY;
                log::info!(
                    "ChangeDisplaySettingsExW 设置刷新率: {} Hz (dmDisplayFrequency={})",
                    rr,
                    dm.dmDisplayFrequency
                );
            }
            dm.dmFields = dm_fields;

            let mut cds_result: i32 = -999;

            // Try ChangeDisplaySettingsExW with multiple flag combinations
            const CDS_UPDATEREGISTRY: u32 = 0x00000001;
            const CDS_ENABLE_UNSAFE_MODES: u32 = 0x00000100;
            // Skip CDS_GLOBAL — it requires CDS_SET_PRIMARY and causes BADFLAGS

            let flag_sets: &[u32] = &[
                CDS_UPDATEREGISTRY | CDS_ENABLE_UNSAFE_MODES,
                CDS_UPDATEREGISTRY,
                0,
            ];

            for &flags in flag_sets {
                cds_result = ChangeDisplaySettingsExW(
                    wide_name.as_ptr(),
                    &dm,
                    std::ptr::null_mut(),
                    flags,
                    std::ptr::null_mut(),
                );
                if cds_result == 0 {
                    if nvapi_succeeded {
                        log::info!(
                            "ChangeDisplaySettingsExW(flags={}) 成功设置刷新率 {}x{} @ {}Hz",
                            flags,
                            width,
                            height,
                            refresh_rate.unwrap_or(0.0)
                        );
                    } else {
                        log::info!(
                            "通过 NV_Modes 注入 + ChangeDisplaySettingsExW(flags={}) 成功设置分辨率 {}x{}",
                            flags, width, height
                        );
                    }
                    return Ok(SetResolutionResult { applied: true, injected: false });
                }
                log::warn!("ChangeDisplaySettingsExW(flags={}) 返回 DISP_CHANGE={}", flags, cds_result);
            }

            // ── If NVAPI already succeeded (just needed refresh rate), no further retry ──
            if nvapi_succeeded {
                let err_msg = format!(
                    "NVAPI 分辨率设置成功，但 ChangeDisplaySettingsExW 设置刷新率失败 (DISP_CHANGE={})",
                    cds_result
                );
                log::error!("{}", err_msg);
                return Err(err_msg);
            }

            // ── Strategy 3: Try SetDisplayConfig again (mode might now be in NV_Modes) ──
            log::warn!(
                "ChangeDisplaySettingsExW 失败 (DISP_CHANGE={})，重试 NVAPI SetDisplayConfig",
                cds_result
            );

            // Re-read display config and try SetDisplayConfig again
            let retry_result = retry_set_display_config(display_id, width, height);
            if let Ok(()) = retry_result {
                return Ok(SetResolutionResult { applied: true, injected: false });
            }

            // NV_Modes injection succeeded but mode can't be applied without restart
            if reg_inject_err.is_none() {
                log::info!("NV_Modes 注入成功，需要重启后应用新分辨率");
                if let Err(e) = save_injected_resolution(width, height) {
                    log::warn!("保存注入记录失败: {}", e);
                }
                return Ok(SetResolutionResult { applied: false, injected: true });
            }

            let reg_err_msg = reg_inject_err.unwrap_or_else(|| "注入成功但模式切换失败".to_string());
            Err(format!(
                "所有方案均失败 — NVAPI SetDisplayConfig: {}; NV_Modes注入: {}; ChangeDisplaySettingsExW: DISP_CHANGE={}",
                nvapi_err, reg_err_msg, cds_result
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Cleanup
// ---------------------------------------------------------------------------

pub fn cleanup() {
    if let Ok(mut guard) = NVAPI.lock() {
        if guard.take().is_some() {
            if let Some(dynapi) = load_nvapi_dyn() {
                unsafe {
                    (dynapi.unload)();
                }
            }
            log::info!("NVAPI session cleaned up");
        }
    }
}



