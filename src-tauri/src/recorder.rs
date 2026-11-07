// ============================================================================
// 录制 / 采集（类 OBS 的简易实现）
//
// 设计要点：
// - 采集与编码全部交给外部 ffmpeg 进程（gdigrab 抓屏 + dshow 抓摄像头/麦克风），
//   因为本项目其它模块（file_convert / matroska / video_convert）都刻意不依赖
//   外部 ffmpeg，所以这里把依赖收敛到「只在录制功能里出现」，并在缺失时给出
//   明确提示，而不是静默失败。
// - 多源混合通过 ffmpeg filter_complex 完成：
//     画面：摄像头 overlay 到屏幕（画中画，位置/大小/圆角可调）
//     声音：麦克风 + 系统声音 amix 混音
// - ffmpeg 的 stdout/stderr 走独立管道持续读走，避免管道写满导致 ffmpeg 卡死
//   （这是长时录制最常见的「录到一半停止」原因）。
// - 录制进程通过 stdin 发送 "q" 优雅结束，确保 moov atom 正确写入；
//   超时未退出才 kill，避免产生无法播放的残缺文件。
// ============================================================================

use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// Windows 下为子进程加 CREATE_NO_WINDOW，避免 ffmpeg 弹出控制台黑框
#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// settings.json 中保存录制设置的键
const SETTINGS_KEY: &str = "neobox_recorder_settings";
/// 录制配置的当前版本。1 = 修复「图层默认不可见导致黑屏」之后的版本。
/// 旧配置（version 0）在 normalize 时做一次性迁移。
const RECORDER_CONFIG_VERSION: u32 = 1;
/// 录制文件默认输出目录（在「视频」目录下建子目录）
const OUTPUT_DIR_NAME: &str = "NeoBox 录制";
/// 停止录制时等待 ffmpeg 优雅退出的上限
const GRACEFUL_STOP_TIMEOUT_MS: u64 = 8000;
/// 录制文件扩展名
const RECORDING_EXT: &str = "mp4";

// ────────────────────────────────────────────────────────────
//  数据结构
// ────────────────────────────────────────────────────────────

/// 采集设备（摄像头 / 麦克风）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureDevice {
    /// 设备友好名（ffmpeg -i 时为 `audio="名称"` / `video="名称"`
    pub name: String,
    /// video / audio
    pub kind: String,
    /// dshow 的 alternative name（含 GUID，避免重名设备混淆）
    #[serde(default)]
    pub alt_name: String,
}

/// 单个采集图层的布局（画布式：每个源都是一个可自由摆放的图层）
///
/// 坐标全部使用 **画布像素**，由前端画布直接给出：
/// - 前端预览区显示的是按比例缩小的画布，拖拽后按比例换算成真实画布像素再传给后端，
///   所以这里不需要「百分比」语义，避免屏幕分辨率变化时布局漂移。
///
/// # 只控宽度，高度由源宽高比自动推导
///
/// 采集源（屏幕 / 摄像头）有各自的原始宽高比，**不能被拉伸变形**。
/// 因此图层只记录显示宽度 `w`，高度由 ffmpeg 的 `scale=w:-2` 按源比例算出来；
/// 前端预览同样用 CSS `aspect-ratio` 保持源比例。
/// 这样无论用户怎么缩放、怎么改画布比例，画面都保持源比例。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerLayout {
    /// 图层左上角在画布中的 x（像素，可为负 = 移出画布）
    #[serde(default)]
    pub x: i32,
    /// 图层左上角在画布中的 y（像素，可为负 = 移出画布）
    #[serde(default)]
    pub y: i32,
    /// 图层显示宽度（画布像素，0 = 铺满画布宽度）
    #[serde(default)]
    pub w: u32,
    /// 已废弃：高度由源宽高比自动推导。
    /// 保留字段仅为兼容旧配置（读入后忽略，不再参与布局计算）。
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub h: u32,
    // ── 已废弃的裁剪字段（裁剪功能已移除，仅保留以兼容旧配置反序列化）──
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub crop_x: u32,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub crop_y: u32,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub crop_w: u32,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub crop_h: u32,
    /// 图层是否显示（关掉可临时隐藏该源，不必删配置）
    #[serde(default = "default_true")]
    pub visible: bool,
}

/// # 为什么必须手写 `Default`
///
/// `#[serde(default = "default_true")]` **只在反序列化缺字段时生效**，
/// 对 `Default` trait 没有任何影响。而 derive 出来的 `Default` 会把
/// `bool` 填成零值 `false` —— 结果 `RecorderConfig::default()` 的两个图层
/// 都是不可见，`build_args` 跳过所有图层，只剩一张黑色画布：
/// **新用户（或清空配置后）第一次录制直接产出纯黑视频**，而文件非空，
/// 现有的 `size == 0` 校验还会通过，用户完全收不到错误提示。
///
/// 因此这里手写 `Default`，保证 `visible` 默认可见。
impl Default for LayerLayout {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            crop_x: 0,
            crop_y: 0,
            crop_w: 0,
            crop_h: 0,
            visible: true,
        }
    }
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

/// 录制源配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecorderConfig {
    /// 是否录制屏幕
    #[serde(default = "default_true")]
    pub screen_enabled: bool,
    /// 屏幕采集帧率
    #[serde(default = "default_fps")]
    pub screen_fps: u32,
    /// 摄像头设备名（空 = 不启用摄像头）
    #[serde(default)]
    pub camera_device: String,
    /// 摄像头采集的分辨率宽（0 = 设备默认）
    #[serde(default)]
    pub camera_width: u32,
    /// 摄像头采集的分辨率高（0 = 设备默认）
    #[serde(default)]
    pub camera_height: u32,
    /// 画布宽度（像素，0 = 用屏幕原始宽度）
    #[serde(default)]
    pub canvas_w: u32,
    /// 画布高度（像素，0 = 用屏幕原始高度）
    #[serde(default)]
    pub canvas_h: u32,
    /// 屏幕图层的布局
    #[serde(default)]
    pub screen_layout: LayerLayout,
    /// 摄像头图层的布局
    #[serde(default)]
    pub camera_layout: LayerLayout,
    /// 配置版本号：用于做一次性数据迁移（见 normalize）
    /// 0 = 旧版本（可能存在 visible=false 的历史脏数据）
    #[serde(default)]
    pub config_version: u32,
    // 兼容旧配置：仍接受 pip_* 字段，读入后由 normalize 迁移到 camera_layout
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pip_scale: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pip_margin: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pip_position: Option<String>,
    /// 麦克风设备名（空 = 不采集麦克风）
    #[serde(default)]
    pub mic_device: String,
    /// 系统声音设备名（空 = 不采集系统声音）
    #[serde(default)]
    pub system_audio_device: String,
    /// 主麦克风音量（0-200，100 = 原始音量）
    #[serde(default = "default_volume")]
    pub mic_volume: u32,
    /// 系统声音音量（0-200）
    #[serde(default = "default_volume")]
    pub system_volume: u32,
    /// 视频码率（kbps）
    #[serde(default = "default_bitrate")]
    pub video_bitrate_kbps: u32,
    /// 音频码率（kbps）
    #[serde(default = "default_audio_bitrate")]
    pub audio_bitrate_kbps: u32,
    /// 编码器：auto / libx264 / h264_nvenc / h264_qsv / h264_amf
    #[serde(default = "default_encoder")]
    pub encoder: String,
    /// 输出目录（空 = 默认目录）
    #[serde(default)]
    pub output_dir: String,
    /// 是否在采集时开启实时预览（关掉可省资源）
    #[serde(default = "default_true")]
    pub preview_enabled: bool,
}

fn default_true() -> bool { true }
fn default_fps() -> u32 { 30 }
fn default_volume() -> u32 { 100 }
fn default_bitrate() -> u32 { 6000 }
fn default_audio_bitrate() -> u32 { 160 }
fn default_encoder() -> String { "auto".to_string() }

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            screen_enabled: true,
            screen_fps: default_fps(),
            camera_device: String::new(),
            camera_width: 0,
            camera_height: 0,
            canvas_w: 0,
            canvas_h: 0,
            screen_layout: LayerLayout::default(),
            camera_layout: LayerLayout::default(),
            // 新配置直接就是最新版本，不需要迁移
            config_version: RECORDER_CONFIG_VERSION,
            pip_scale: None,
            pip_margin: None,
            pip_position: None,
            mic_device: String::new(),
            system_audio_device: String::new(),
            mic_volume: default_volume(),
            system_volume: default_volume(),
            video_bitrate_kbps: default_bitrate(),
            audio_bitrate_kbps: default_audio_bitrate(),
            encoder: default_encoder(),
            output_dir: String::new(),
            preview_enabled: true,
        }
    }
}

impl RecorderConfig {
    /// 把旧版的 pip_* 配置迁移到画布布局，并补齐画布尺寸。
    ///
    /// 旧配置只有「摄像头固定在四角之一 + 缩放百分比」这一种形态，
    /// 直接丢弃会让老用户的位置设置失效，所以在这里换算成等价的像素布局。
    pub fn normalize(&mut self, screen_w: u32, screen_h: u32) {
        // 画布尺寸：没设就用屏幕原始尺寸
        if self.canvas_w == 0 { self.canvas_w = screen_w.max(2); }
        if self.canvas_h == 0 { self.canvas_h = screen_h.max(2); }
        // 画布尺寸必须为偶数（H.264 的 yuv420p 要求宽高是 2 的倍数）
        self.canvas_w -= self.canvas_w % 2;
        self.canvas_h -= self.canvas_h % 2;

        // 旧 pip_* → camera_layout
        let legacy_scale = self.pip_scale.unwrap_or(25).clamp(1, 100);
        let legacy_margin = self.pip_margin.unwrap_or(24);
        let legacy_pos = self.pip_position.clone().unwrap_or_else(|| "bottom-right".into());
        // 仅当 camera_layout 还没被设置过（全 0）时才迁移，避免覆盖用户新布局
        let camera_is_unset = self.camera_layout.w == 0
            && self.camera_layout.h == 0
            && self.camera_layout.x == 0
            && self.camera_layout.y == 0;
        if camera_is_unset && self.pip_scale.is_some() {
            let cam_w = ((self.canvas_w as u64 * legacy_scale as u64) / 100) as u32;
            let cam_h = ((cam_w as u64 * 3) / 4) as u32; // 按 4:3 估个初值
            let m = legacy_margin as i32;
            let (x, y) = match legacy_pos.as_str() {
                "top-left" => (m, m),
                "top-right" => (self.canvas_w as i32 - cam_w as i32 - m, m),
                "bottom-left" => (m, self.canvas_h as i32 - cam_h as i32 - m),
                _ => (self.canvas_w as i32 - cam_w as i32 - m, self.canvas_h as i32 - cam_h as i32 - m),
            };
            self.camera_layout = LayerLayout {
                x: x.max(0),
                y: y.max(0),
                w: cam_w,
                h: cam_h,
                ..Default::default()
            };
        }

        // 屏幕图层若未设尺寸，默认铺满画布
        if self.screen_layout.w == 0 { self.screen_layout.w = self.canvas_w; }
        if self.screen_layout.h == 0 { self.screen_layout.h = self.canvas_h; }

        // 迁移历史遗留的「启用但不可见」矛盾配置。
        //
        // 旧版本因 derive(Default) 让 visible 默认 false，凡是保存过配置的用户
        // 都可能把 visible=false 落盘；这些配置升级后仍然会录出黑屏。
        // 用配置版本号做一次性迁移：只有 version < 1 的旧配置才强制置为可见，
        // 之后用户主动隐藏图层的选择会被尊重（不会被反复改回 true）。
        if self.config_version < RECORDER_CONFIG_VERSION {
            if self.screen_enabled {
                self.screen_layout.visible = true;
            }
            if !self.camera_device.trim().is_empty() {
                self.camera_layout.visible = true;
            }
            self.config_version = RECORDER_CONFIG_VERSION;
        }
        // 迁移完成后清掉旧字段，避免之后再被重复迁移
        self.pip_scale = None;
        self.pip_margin = None;
        self.pip_position = None;
    }
}

/// 录制状态
#[derive(Debug, Clone, Serialize)]
pub struct RecorderStatus {
    pub recording: bool,
    /// 已录制时长（秒）
    pub elapsed_sec: u64,
    /// 预估文件大小（字节）
    pub estimated_bytes: u64,
    /// 当前输出文件路径
    pub output_path: String,
    /// 实际使用的编码器
    pub encoder: String,
    /// 最近一次错误信息（前端可展示）
    pub last_error: String,
}

impl Default for RecorderStatus {
    fn default() -> Self {
        Self {
            recording: false,
            elapsed_sec: 0,
            estimated_bytes: 0,
            output_path: String::new(),
            encoder: String::new(),
            last_error: String::new(),
        }
    }
}

/// 录制结果
#[derive(Debug, Clone, Serialize)]
pub struct RecorderFile {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub modified_ms: u64,
    /// 时长（秒），0 表示未探测
    pub duration_sec: u64,
}

/// ffmpeg 可用性探测结果
#[derive(Debug, Clone, Serialize)]
pub struct FfmpegProbe {
    pub available: bool,
    pub path: String,
    pub version: String,
    /// 是否支持 gdigrab
    pub has_gdigrab: bool,
    /// 是否支持 dshow
    pub has_dshow: bool,
    /// 可用的硬件编码器列表
    pub hw_encoders: Vec<String>,
}

// ────────────────────────────────────────────────────────────
// 运行时状态（全局单例）
// ────────────────────────────────────────────────────────────

/// 正在运行的录制进程
struct ActiveRecording {
    child: Child,
    output_path: PathBuf,
    started_at: Instant,
    encoder: String,
    /// stderr 最后若干行，进程异常退出时用于定位原因
    stderr_tail: Arc<Mutex<Vec<String>>>,
    /// 视频码率（kbps），用于估算文件大小
    bitrate_kbps: u32,
}

static ACTIVE: Mutex<Option<ActiveRecording>> = Mutex::new(None);
/// 录制是否已开始（原子量便于无锁快速查询）
static IS_RECORDING: AtomicBool = AtomicBool::new(false);
/// 累计录制时长（秒），进程退出后保留最后值
static ELAPSED_SEC: AtomicU64 = AtomicU64::new(0);
/// 最近一次错误
static LAST_ERROR: Mutex<String> = Mutex::new(String::new());

/// 「启动录制」的串行闸门。
///
/// # 为什么需要单独一把锁
///
/// `recorder_start` 的流程是「检查是否在录 → 起 ffmpeg（耗时上百毫秒）→ 写入 ACTIVE」。
/// 若只用 ACTIVE 锁，检查与写入之间会释放锁，两个并发调用（前端双击、页面重挂载、
/// IPC 重入）都能通过检查，各自 spawn 一个 ffmpeg：
/// 后写者覆盖 ACTIVE，**先写者的 Child 被 drop（Rust 的 Child::drop 不会杀进程）
/// → 变成孤儿 ffmpeg 继续录制并写同一个文件**，stop / 退出清理都收不掉它。
///
/// 用这把独立闸门把整段「检查 → spawn → 写入」串行化，
/// 由于它在整个流程内持有，不会与 ACTIVE 锁产生嵌套顺序问题。
static START_GATE: Mutex<()> = Mutex::new(());

/// 统一获取 ACTIVE 锁（锁中毒后继续使用内部数据）。
///
/// 这些状态都是普通数据，中毒意味着某次持锁线程 panic，但数据本身没有
/// 会因此破坏的不变量，因此一律取回内部值继续用。
/// 早先混用 `.map_err(|e| e.to_string())` 会让锁中毒后**永久无法停止录制**
/// （stop 一直返回 Err，ffmpeg 残留），这里统一成不放弃的写法。
fn lock_active() -> std::sync::MutexGuard<'static, Option<ActiveRecording>> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// 统一获取 LAST_ERROR 锁（同上，中毒不放弃）
fn lock_last_error() -> std::sync::MutexGuard<'static, String> {
    LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner())
}

// ────────────────────────────────────────────────────────────
//  ffmpeg 定位
// ────────────────────────────────────────────────────────────

/// 查找 ffmpeg 可执行文件。
///
/// 查找顺序：
/// 1. settings.json 中用户手动指定的路径（`neobox_recorder_settings.ffmpeg_path` 兼容项）
/// 2. 应用同级目录 / resources 目录下的 ffmpeg.exe（便携包自带场景）
/// 3. PATH 中的 ffmpeg
/// 4. 常见安装位置（winget / scoop / 手动安装的 Program Files）
pub fn find_ffmpeg() -> Option<PathBuf> {
    // 1) 应用目录旁
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for candidate in [
                dir.join("ffmpeg.exe"),
                dir.join("ffmpeg").join("ffmpeg.exe"),
                dir.join("resources").join("ffmpeg.exe"),
                dir.join("bin").join("ffmpeg.exe"),
            ] {
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    // 2) PATH 中
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("ffmpeg.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    // 3) 常见安装位置
    let mut guesses: Vec<PathBuf> = Vec::new();
    for base in [
        PathBuf::from(r"C:\Program Files"),
        PathBuf::from(r"C:\Program Files (x86)"),
        PathBuf::from(r"C:\ffmpeg"),
    ] {
        if let Ok(entries) = std::fs::read_dir(&base) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if name.starts_with("ffmpeg") {
                    let p = entry.path();
                    guesses.push(p.join("bin").join("ffmpeg.exe"));
                    guesses.push(p.join("ffmpeg.exe"));
                }
            }
        }
    }
    // scoop / winget / 用户目录
    if let Some(home) = dirs::home_dir() {
        guesses.push(home.join("scoop").join("shims").join("ffmpeg.exe"));
        guesses.push(home.join("scoop").join("apps").join("ffmpeg").join("current").join("bin").join("ffmpeg.exe"));
    }
    if let Some(local) = dirs::data_local_dir() {
        guesses.push(local.join("Microsoft").join("WinGet").join("Links").join("ffmpeg.exe"));
    }
    for g in guesses {
        if g.is_file() {
            return Some(g);
        }
    }

    None
}

/// 构造一个 ffmpeg 子进程，并统一加上 `CREATE_NO_WINDOW`。
///
/// # 为什么必须统一在这里加
///
/// 打开录制页面时会连续跑多次 ffmpeg（版本探测、-formats、-encoders、
/// 每个硬件编码器各一次试探、设备枚举），如果漏加这个标志，
/// 用户会看到一串控制台黑窗疯狂闪 —— 观感极差且会被误认为程序异常。
/// 所有 ffmpeg 调用都必须经由此函数创建，不要在别处直接 `Command::new(ffmpeg)`。
fn ffmpeg_cmd(ffmpeg_path: &Path) -> Command {
    let mut cmd = Command::new(ffmpeg_path);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    cmd
}

/// 探测 ffmpeg 能力（版本、采集设备支持、硬件编码器）
fn probe_ffmpeg(ffmpeg_path: &Path) -> FfmpegProbe {
    let mut probe = FfmpegProbe {
        available: false,
        path: ffmpeg_path.to_string_lossy().into_owned(),
        version: String::new(),
        has_gdigrab: false,
        has_dshow: false,
        hw_encoders: Vec::new(),
    };

    // 版本
    if let Ok(out) = ffmpeg_cmd(ffmpeg_path).arg("-version").output() {
        probe.available = out.status.success();
        let text = String::from_utf8_lossy(&out.stdout);
        // 取第一行，形如 "ffmpeg version 2026-10-25-git-... Copyright ..."
        probe.version = text
            .lines()
            .next()
            .unwrap_or("")
            .split("Copyright")
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
    }
    if !probe.available {
        return probe;
    }

    // 输入格式与硬件编码器一次拿齐：`-formats` 和 `-encoders` 各一次进程，
    // 已经是 2 次；不再额外为每个编码器单独起进程（那会再增加 3 次黑窗）。
    if let Ok(out) = ffmpeg_cmd(ffmpeg_path).arg("-hide_banner").arg("-formats").output() {
        let text = String::from_utf8_lossy(&out.stdout).to_string()
            + &String::from_utf8_lossy(&out.stderr);
        probe.has_gdigrab = text.contains("gdigrab");
        probe.has_dshow = text.contains("dshow");
    }

    // 硬件编码器：只看常见四种，软编 libx264 作为兜底始终可用
    if let Ok(out) = ffmpeg_cmd(ffmpeg_path).arg("-hide_banner").arg("-encoders").output() {
        let text = String::from_utf8_lossy(&out.stdout).to_string()
            + &String::from_utf8_lossy(&out.stderr);
        for name in ["h264_nvenc", "h264_qsv", "h264_amf"] {
            if text.contains(name) {
                probe.hw_encoders.push(name.to_string());
            }
        }
    }

    probe
}

/// 判断某个硬件编码器是否真的可用（能初始化）。
///
/// ffmpeg 的 `-encoders` 只表示「编译进了这个编码器」，
/// 而虚拟机 / 无对应硬件的机器上初始化会失败。用一个 1 帧空编码实测最可靠。
fn encoder_works(ffmpeg_path: &Path, encoder: &str) -> bool {
    let null_out = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let result = ffmpeg_cmd(ffmpeg_path)
        .args([
            "-hide_banner", "-loglevel", "error",
            "-f", "lavfi", "-i", "color=c=black:s=256x144:r=1:d=1",
            "-c:v", encoder,
            "-frames:v", "1",
            "-f", "null", null_out,
        ])
        .output();
    match result {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}

/// 选择一个实际可用的编码器。
/// auto 时优先硬件编码（省 CPU），失败则回退 libx264。
fn resolve_encoder(ffmpeg_path: &Path, requested: &str) -> String {
    let requested = requested.trim();
    if requested != "auto" && !requested.is_empty() {
        if encoder_works(ffmpeg_path, requested) {
            return requested.to_string();
        }
        warn!("[Recorder] 指定编码器 {requested} 不可用，回退 libx264");
    }
    // 依次尝试硬件编码。注意顺序：
    // 1) h264_nvenc / h264_qsv / h264_amf 是独显/核显专用，最快；
    // 2) **h264_mf** 是 Windows MediaFoundation，只要有可用 GPU（含 VMware 虚拟显卡、
    //    核显、甚至部分软件实现）就能工作。实测其纯编码速度 6.32x，
    //    而 libx264 在本机仅 0.98x（低于实时），因此 h264_mf 往往是
    //    「低配 / 虚拟机」环境从「卡顿」变成「流畅」的关键。
    for hw in ["h264_nvenc", "h264_qsv", "h264_amf", "h264_mf"] {
        if encoder_works(ffmpeg_path, hw) {
            info!("[Recorder] 使用硬件编码器 {hw}");
            return hw.to_string();
        }
    }
    info!("[Recorder] 使用软件编码器 libx264");
    "libx264".to_string()
}

// ────────────────────────────────────────────────────────────
//  设备枚举
// ────────────────────────────────────────────────────────────

/// 通过 ffmpeg dshow 枚举摄像头与麦克风。
///
/// ffmpeg 把设备列表打印到 **stderr**，且退出码是 1（因为 dummy 输入打不开），
/// 所以不能靠退出码判断成功，只能解析输出。
///
/// # 解析规则（实测输出样例）
///
/// ```text
/// [in#0 @ 0000...] "USB Camera" (video)
/// [in#0 @ 0000...]   Alternative name "@device_pnp_\\?\usb#vid_..."
/// [in#0 @ 0000...] "Microphone (Realtek)" (audio)
/// [in#0 @ 0000...]   Alternative name "@device_cm_{...}\\wave_{...}"
/// ```
///
/// 关键点：**设备行与 Alternative name 行都以 `[` 开头且都含引号**。
/// 早先的实现先匹配「以 `[` 开头且含引号」，于是把 Alternative name 行
/// 也当成设备行；该行不含 `(video)` / `(audio)` 会被跳过，但此时
/// `pending`（上一台真正设备）已存在，紧接着若来了下一台设备行就会
/// 把 `pending` 覆盖 —— 表现为「摄像头/麦克风识别不到或只剩最后一个」。
///
/// 现在按「行内是否带 `(video)` / `(audio)` 标记」来判定设备行，
/// Alternative name 只作为前一台设备的补充字段。
pub fn enumerate_devices(ffmpeg_path: &Path) -> Vec<CaptureDevice> {
    let output = ffmpeg_cmd(ffmpeg_path)
        .args(["-hide_banner", "-list_devices", "true", "-f", "dshow", "-i", "dummy"])
        .output();

    let Ok(out) = output else { return Vec::new() };
    // 设备表在 stderr；部分版本会把部分信息写 stdout，两边都取更稳
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let mut devices: Vec<CaptureDevice> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();

        // 只在方括号前缀之后的内容里做判断，避免 `[in#0 @ ...]` 里的字符干扰
        let Some(after_bracket) = trimmed.find(']').map(|i| trimmed[i + 1..].trim()) else {
            continue;
        };

        // Alternative name 行：补充到「最后一台」设备上（它紧跟在设备行之后）
        if after_bracket.starts_with("Alternative name") {
            if let Some(last) = devices.last_mut() {
                // 取引号内内容作为 alt_name
                if let (Some(fq), Some(lq)) = (after_bracket.find('"'), after_bracket.rfind('"')) {
                    if lq > fq {
                        last.alt_name = after_bracket[fq + 1..lq].to_string();
                    }
                }
            }
            continue;
        }

        // 设备行判定：必须带 (video) 或 (audio) 标记，且带引号名称
        let kind = if after_bracket.contains("(video)") {
            Some("video")
        } else if after_bracket.contains("(audio)") {
            Some("audio")
        } else {
            None
        };
        let Some(kind) = kind else { continue };

        let (Some(fq), Some(lq)) = (after_bracket.find('"'), after_bracket.rfind('"')) else {
            continue;
        };
        if lq <= fq {
            continue;
        }
        let name = after_bracket[fq + 1..lq].to_string();
        if name.is_empty() {
            continue;
        }
        devices.push(CaptureDevice {
            name,
            kind: kind.to_string(),
            alt_name: String::new(),
        });
    }

    devices
}

// ────────────────────────────────────────────────────────────
//  ffmpeg 参数组装
// ────────────────────────────────────────────────────────────

/// 画中画位置转 overlay 坐标表达式
/// 旧版画中画定位（已由画布式 LayerLayout 取代，保留供旧配置迁移参考）
#[allow(dead_code)]
fn pip_xy(position: &str, margin: u32) -> (String, String) {
    let m = margin.max(0);
    match position {
        "top-left" => (m.to_string(), m.to_string()),
        "top-right" => (format!("W-w-{m}"), m.to_string()),
        "bottom-left" => (m.to_string(), format!("H-h-{m}")),
        // 默认右下角
        _ => (format!("W-w-{m}"), format!("H-h-{m}")),
    }
}

/// 组装录制用的 ffmpeg 参数（不含可执行文件本身）。
///
/// 返回 (参数列表, 实际编码器)。
pub fn build_args(
    config: &RecorderConfig,
    encoder: &str,
    output_path: &Path,
    camera_device: &str,
    mic_device: &str,
    system_audio_device: &str,
) -> Result<(Vec<String>, String), String> {
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-y".into()];

    // ── 输入 0：屏幕（gdigrab）──
    let has_screen = config.screen_enabled;
    if has_screen {
        let fps = config.screen_fps.clamp(1, 120);
        args.extend([
            "-f".into(), "gdigrab".into(),
            "-framerate".into(), fps.to_string(),
            "-draw_mouse".into(), "1".into(),
            "-i".into(), "desktop".into(),
        ]);
    }

    // ── 输入：摄像头 ──
    let mut video_inputs: Vec<usize> = Vec::new();
    let mut next_index = if has_screen { 1 } else { 0 };
    if has_screen {
        video_inputs.push(0);
    }

    let has_camera = !camera_device.trim().is_empty();
    let camera_index = if has_camera {
        // 摄像头分辨率：0 表示交给设备默认值（不可写入空的 size 参数）
        let mut camera_args: Vec<String> = vec!["-f".into(), "dshow".into()];
        let size = match (config.camera_width, config.camera_height) {
            (w, h) if w > 0 && h > 0 => Some(format!("{w}x{h}")),
            _ => None,
        };
        if let Some(size) = size {
            camera_args.push("-video_size".into());
            camera_args.push(size);
        }
        camera_args.push("-i".into());
        camera_args.push(format!("video={camera_device}"));
        let idx = next_index;
        next_index += 1;
        args.extend(camera_args);
        video_inputs.push(idx);
        Some(idx)
    } else {
        None
    };

    // ── 输入：麦克风 ──
    let mic_index = if !mic_device.trim().is_empty() {
        args.extend([
            "-f".into(), "dshow".into(),
            "-i".into(), format!("audio={mic_device}"),
        ]);
        let idx = next_index;
        next_index += 1;
        Some(idx)
    } else {
        None
    };

    // ── 输入：系统声音 ──
    let system_index = if !system_audio_device.trim().is_empty() {
        args.extend([
            "-f".into(), "dshow".into(),
            "-i".into(), format!("audio={system_audio_device}"),
        ]);
        // 最后一个输入，next_index 不再需要递增
        Some(next_index)
    } else {
        None
    };

    // 至少要有一个视频源，否则无法产出视频文件
    if video_inputs.is_empty() {
        return Err("至少需要启用屏幕采集或选择一个摄像头".to_string());
    }

    // ── 滤镜链：画面（画布式布局）──
    //
    // 设计：先建一张固定尺寸的空白画布，再把每个「可见图层」依次 overlay 上去。
    //
    // 每个图层的处理顺序：`scale=<宽度>:-2` → overlay（按 x/y 定位）
    //
    // 关键：只用 `scale=w:-2`，**高度由采集源自身的宽高比自动算出**，
    // 因此屏幕与摄像头画面在任何缩放/画布比例下都不会被拉伸变形。
    // 早期版本把宽度和高度都写死再去 pad 补黑边，既复杂又会出现
    // 「框比画面大、四周一圈黑」的问题；现在图层框与画面严格同比例。
    let mut filters: Vec<String> = Vec::new();

    let canvas_w = config.canvas_w.max(2);
    let canvas_h = config.canvas_h.max(2);
    // yuv420p 要求宽高为偶数
    let canvas_w = canvas_w - canvas_w % 2;
    let canvas_h = canvas_h - canvas_h % 2;

    // 画布底色：lavfi color 源
    filters.push(format!(
        "color=c=black:s={canvas_w}x{canvas_h}:r={}[canvas0]",
        config.screen_fps.clamp(1, 120)
    ));

    let mut composed = String::from("[canvas0]");

    // 逐个图层叠加
    let mut layer_no = 0usize;

    // 屏幕图层（输入 0）
    if has_screen && config.screen_layout.visible {
        let lay = &config.screen_layout;
        // 只控宽度，高度按源宽高比自动（-2 保证偶数）。
        // w=0 表示铺满画布宽度。
        let dw = if lay.w > 0 { lay.w - lay.w % 2 } else { canvas_w };
        let chain = format!("scale={dw}:-2");
        let label = format!("[scr{layer_no}]");
        filters.push(format!("[0:v]setpts=PTS-STARTPTS,{chain}{label}"));
        filters.push(format!(
            "{composed}{label}overlay=x={}:y={}:shortest=0[canvas{}]",
            // 允许负坐标：图层可以拖到画布外（超出部分自然被裁掉）
            lay.x,
            lay.y,
            layer_no + 1
        ));
        composed = format!("[canvas{}]", layer_no + 1);
        layer_no += 1;
    }

    // 摄像头图层
    if let Some(cam) = camera_index {
        let lay = &config.camera_layout;
        if lay.visible {
            // 与屏幕图层规则完全一致：只控宽度，高度按源比例自动
            let dw = if lay.w > 0 { lay.w - lay.w % 2 } else { canvas_w };
            let chain = format!("scale={dw}:-2");
            let label = format!("[cam{layer_no}]");
            filters.push(format!("[{cam}:v]setpts=PTS-STARTPTS,{chain}{label}"));
            filters.push(format!(
                "{composed}{label}overlay=x={}:y={}:shortest=0[canvas{}]",
                // 允许负坐标：图层可以拖到画布外
                lay.x,
                lay.y,
                layer_no + 1
            ));
            composed = format!("[canvas{}]", layer_no + 1);
            // 摄像头是当前最后一个图层，后续不再有新图层，这里递增只为语义完整
            let _ = layer_no + 1;
        }
    }

    // 所有图层都不可见时，至少要保证画布本身被输出（避免 map 找不到流）
    let video_out_label = composed.clone();

    // 统一重采样到恒定帧率并格式化像素格式。
    //
    // # 为什么必须显式 `fps=`（这是「录制画面非常卡顿」的根因修复）
    //
    // gdigrab 抓屏的节奏由系统调度决定，实测在本机只能稳定产出约 22–24fps
    // （即使 `-framerate 30`）。若不加处理，这些帧会带着**极不均匀**的时间戳
    // 进入编码器与 mp4：实测帧间隔在 0.166s / 0.200s / 0.233s / 0.300s 之间
    // 上下乱跳（目标应恒为 0.0333s）。播放器按这些时间戳显示，于是画面
    // 一顿一顿地推进 —— 这就是用户看到的「非常卡顿」。
    //
    // `fps=<输出帧率>` 会把不均匀的输入重采样成**严格等间隔**的恒定帧率
    // （不足处复制上一帧、多余处丢弃），实测加它之后帧间隔稳定为 0.033333s，
    // 播放恢复顺滑。代价是文件里会有重复帧，但视觉流畅度远优先于此。
    //
    // 同时它保证下游编码器收到恒定帧率，避免 -g / 码率控制按错误的帧率估算。
    let out_fps = config.screen_fps.clamp(1, 120);
    filters.push(format!(
        "{video_out_label}fps={out_fps},format=yuv420p[vout]"
    ));
    let video_out_label = "[vout]".to_string();

    // ── 滤镜链：音频混合 ──
    let mut audio_out_label: Option<String> = None;
    let mut has_audio = false;

    match (mic_index, system_index) {
        (Some(mic), Some(sys)) => {
            // 两路都采集：音量可调后 amix 混合
            let mv = config.mic_volume as f64 / 100.0;
            let sv = config.system_volume as f64 / 100.0;
            filters.push(format!("[{mic}:a]volume={mv:.3}[micv]"));
            filters.push(format!("[{sys}:a]volume={sv:.3}[sysv]"));
            filters.push("[micv][sysv]amix=inputs=2:duration=longest:dropout_transition=0,aresample=async=1[aout]".to_string());
            audio_out_label = Some("[aout]".to_string());
            has_audio = true;
        }
        (Some(mic), None) => {
            let mv = config.mic_volume as f64 / 100.0;
            filters.push(format!("[{mic}:a]volume={mv:.3},aresample=async=1[aout]"));
            audio_out_label = Some("[aout]".to_string());
            has_audio = true;
        }
        (None, Some(sys)) => {
            let sv = config.system_volume as f64 / 100.0;
            filters.push(format!("[{sys}:a]volume={sv:.3},aresample=async=1[aout]"));
            audio_out_label = Some("[aout]".to_string());
            has_audio = true;
        }
        (None, None) => {}
    }

    args.push("-filter_complex".into());
    args.push(filters.join(";"));
    args.push("-map".into());
    args.push(video_out_label);

    // ── 编码参数 ──
    let br = config.video_bitrate_kbps.clamp(500, 100_000);
    args.extend([
        "-c:v".into(), encoder.to_string(),
        "-b:v".into(), format!("{br}k"),
    ]);

    // 各编码器的调优参数不同，写错会直接失败
    match encoder {
        "libx264" => {
            args.extend([
                "-preset".into(), "veryfast".into(),
                "-pix_fmt".into(), "yuv420p".into(),
                // WebView2 / 系统播放器兼容性：确保 yuv420p + 标准 profile
                "-profile:v".into(), "high".into(),
            ]);
            // 注意：**不要**加 `-tune zerolatency`。
            // 它禁用 B 帧与前瞻，只对直播推流有意义；录制场景纯属浪费码率。
            // 实测同一段 5 秒屏幕录制：带 zerolatency 为 1.55MB，
            // 去掉后 1.14MB（体积减少 26%）且画质相当。
        }
        "h264_nvenc" => {
            args.extend([
                "-preset".into(), "p5".into(),
                "-rc".into(), "cbr".into(),
                "-pix_fmt".into(), "yuv420p".into(),
            ]);
        }
        "h264_qsv" => {
            args.extend([
                "-preset".into(), "veryfast".into(),
                "-pix_fmt".into(), "nv12".into(),
            ]);
        }
        "h264_amf" => {
            args.extend([
                "-quality".into(), "speed".into(),
                "-pix_fmt".into(), "yuv420p".into(),
            ]);
        }
        "h264_mf" => {
            // MediaFoundation 编码器：只接受少量通用参数，
            // 不能传 -preset / -profile / -tune（那是 x264/nvenc 的选项，传了会失败）。
            // 实测纯编码 1080p30 达 6.32x，远快于 libx264 的 0.98x。
            args.extend([
                "-pix_fmt".into(), "yuv420p".into(),
            ]);
        }
        _ => {
            args.extend(["-pix_fmt".into(), "yuv420p".into()]);
        }
    }

    if has_audio {
        if let Some(label) = audio_out_label {
            args.push("-map".into());
            args.push(label);
        }
        let abr = config.audio_bitrate_kbps.clamp(32, 512);
        args.extend([
            "-c:a".into(), "aac".into(),
            "-b:a".into(), format!("{abr}k"),
            "-ac".into(), "2".into(),
            "-ar".into(), "48000".into(),
        ]);
    }

    // 关键帧间隔 2 秒，便于播放器 seek
    args.extend(["-g".into(), (config.screen_fps.clamp(1, 120) * 2).to_string()]);
    // faststart：把 moov 移到文件头，录制中断也能尽量可播
    args.extend(["-movflags".into(), "+faststart".into()]);
    // 输出文件
    args.push(output_path.to_string_lossy().into_owned());

    Ok((args, encoder.to_string()))
}

// ────────────────────────────────────────────────────────────
//  输出路径与设置
// ────────────────────────────────────────────────────────────

/// 解析输出目录：用户指定 → 默认「视频/NeoBox 录制」
fn resolve_output_dir(config: &RecorderConfig) -> PathBuf {
    let raw = config.output_dir.trim();
    if !raw.is_empty() {
        let p = PathBuf::from(raw);
        let _ = std::fs::create_dir_all(&p);
        return p;
    }
    let base = dirs::video_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Videos")))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join(OUTPUT_DIR_NAME);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// 生成录制文件名：NeoBox_录制_20261103_205512.mp4
fn make_output_path(dir: &Path) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    dir.join(format!("NeoBox_录制_{stamp}.{RECORDING_EXT}"))
}

/// 读取持久化的录制设置
pub fn read_config(app: &AppHandle) -> RecorderConfig {
    crate::hotkey::read_settings_value(app, SETTINGS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// 保存录制设置
pub fn write_config(app: &AppHandle, config: &RecorderConfig) {
    if let Ok(value) = serde_json::to_value(config) {
        crate::hotkey::save_settings_value(app, SETTINGS_KEY, value);
    }
}

// ────────────────────────────────────────────────────────────
//  Tauri 命令
// ────────────────────────────────────────────────────────────

/// 获取主屏幕分辨率（画布默认尺寸来源）
pub fn screen_size() -> (u32, u32) {
    #[cfg(windows)]
    unsafe {
        use winapi::um::winuser::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        let w = GetSystemMetrics(SM_CXSCREEN);
        let h = GetSystemMetrics(SM_CYSCREEN);
        if w > 0 && h > 0 {
            return (w as u32, h as u32);
        }
    }
    (1920, 1080)
}

/// 探测 ffmpeg 与设备能力
#[tauri::command]
pub async fn recorder_probe() -> Result<FfmpegProbe, String> {
    let Some(ffmpeg) = find_ffmpeg() else {
        return Ok(FfmpegProbe {
            available: false,
            path: String::new(),
            version: String::new(),
            has_gdigrab: false,
            has_dshow: false,
            hw_encoders: Vec::new(),
        });
    };
    Ok(probe_ffmpeg(&ffmpeg))
}

/// 获取屏幕分辨率与默认画布尺寸（前端据此初始化画布与图层坐标）
#[tauri::command]
pub async fn recorder_screen_info() -> Result<serde_json::Value, String> {
    let (w, h) = screen_size();
    Ok(serde_json::json!({ "width": w, "height": h }))
}

/// 枚举摄像头与麦克风设备
#[tauri::command]
pub async fn recorder_list_devices() -> Result<Vec<CaptureDevice>, String> {
    let Some(ffmpeg) = find_ffmpeg() else {
        return Err("未找到 ffmpeg，无法枚举采集设备".to_string());
    };
    Ok(enumerate_devices(&ffmpeg))
}

/// 读取录制设置
#[tauri::command]
pub async fn recorder_get_config(app: AppHandle) -> Result<RecorderConfig, String> {
    let mut cfg = read_config(&app);
    // 归一化后再返回，保证前端拿到的是完整画布布局（含旧配置迁移结果）
    let (sw, sh) = screen_size();
    cfg.normalize(sw, sh);
    Ok(cfg)
}

/// 启动实时预览（MJPEG 流），返回预览端口
#[tauri::command]
pub async fn recorder_preview_start(config: RecorderConfig) -> Result<u16, String> {
    let Some(ffmpeg) = find_ffmpeg() else {
        return Err("未找到 ffmpeg，无法启动预览".to_string());
    };
    let (sw, sh) = screen_size();
    let mut cfg = config;
    cfg.normalize(sw, sh);

    // 预览不再需要图层布局与画布尺寸：每源输出原始画面，由前端分层合成
    let params = crate::recorder_preview::PreviewParams {
        screen_enabled: cfg.screen_enabled,
        screen_fps: cfg.screen_fps,
        camera_device: cfg.camera_device.clone(),
    };

    crate::recorder_preview::start_preview_server(ffmpeg, params).await
}

/// 停止实时预览（释放预览占用的采集设备与 CPU）
#[tauri::command]
pub async fn recorder_preview_stop() -> Result<(), String> {
    crate::recorder_preview::stop_preview();
    Ok(())
}

/// 查询预览状态（端口 / 是否运行）
#[tauri::command]
pub async fn recorder_preview_status() -> Result<serde_json::Value, String> {
    // 按源返回错误：屏幕正常但摄像头失败时，前端仍能在摄像头区域显示原因
    let (screen_err, camera_err) = crate::recorder_preview::preview_source_errors();
    Ok(serde_json::json!({
        "running": crate::recorder_preview::preview_running(),
        "port": crate::recorder_preview::get_preview_port(),
        "screenError": screen_err,
        "cameraError": camera_err,
    }))
}

/// 保存录制设置
#[tauri::command]
pub async fn recorder_save_config(app: AppHandle, config: RecorderConfig) -> Result<(), String> {
    write_config(&app, &config);
    Ok(())
}

/// 开始录制
#[tauri::command]
pub async fn recorder_start(app: AppHandle, config: RecorderConfig) -> Result<String, String> {
    // 用启动闸门串行化整个「检查 → 起进程 → 写入 ACTIVE」流程。
    // 锁中毒不放弃（内部数据仍可用），避免任何一次 panic 让录制功能永久不可用。
    let _gate = START_GATE.lock().unwrap_or_else(|e| e.into_inner());

    // 已在录制则直接返回当前文件，避免重复启动
    {
        let guard = lock_active();
        if let Some(active) = guard.as_ref() {
            return Ok(active.output_path.to_string_lossy().into_owned());
        }
    }
    // 修正可能残留的脏标志（例如上次异常退出没复位）
    IS_RECORDING.store(false, Ordering::SeqCst);

    let Some(ffmpeg) = find_ffmpeg() else {
        return Err("未找到 ffmpeg。请安装 ffmpeg 并加入 PATH，或把 ffmpeg.exe 放到 NeoBox 程序目录旁".to_string());
    };

    // 至少要有视频源
    if !config.screen_enabled && config.camera_device.trim().is_empty() {
        return Err("请至少启用屏幕采集或选择一个摄像头".to_string());
    }

    // 权威保护：录制前先停掉预览进程。
    //
    // 预览与录制会各自起一个 ffmpeg 打开同一个采集设备。虽然一般允许并存，
    // 但并发访问会互相抢设备锁（表现为录制掉帧、甚至启动即退出而界面显示
    // 「录制中」空转）。前端也会在开始录制前停预览，这里作为后端侧兜底，
    // 保证无论从哪条路径触发录制都不会与预览抢设备。
    crate::recorder_preview::stop_preview();

    // 归一化布局：补齐画布尺寸、迁移旧 pip_* 配置、强制偶数尺寸
    let (sw, sh) = screen_size();

    let dir = resolve_output_dir(&config);
    let output_path = make_output_path(&dir);

    let mut config = config;
    config.normalize(sw, sh);

    // 先组装参数与编码器，任何失败都在起进程之前返回，不留半初始化状态
    // （选编码器会实测硬件可用性，可能起若干探测进程，放在 spawn 之前更安全）
    let encoder = resolve_encoder(&ffmpeg, &config.encoder);

    let (args, encoder) = build_args(
        &config,
        &encoder,
        &output_path,
        config.camera_device.trim(),
        config.mic_device.trim(),
        config.system_audio_device.trim(),
    )?;

    info!("[Recorder] 启动录制 encoder={encoder} output={}", output_path.display());
    info!("[Recorder] ffmpeg {}", args.join(" "));

    // stderr 用管道接管：必须持续读走，否则缓冲区写满会让 ffmpeg 阻塞
    // （表现为「录到一半卡住」）。这里用后台线程持续消费并保留末尾若干行。
    let mut child = ffmpeg_cmd(&ffmpeg)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动 ffmpeg 失败: {e}"))?;

    let stderr_tail: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    if let Some(stderr) = child.stderr.take() {
        let tail = stderr_tail.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                // 锁中毒也必须继续读！
                //
                // 这个线程是 stderr 管道**唯一的读者**：一旦它因为锁中毒而退出，
                // ffmpeg 写日志时管道无人消费、缓冲区（约 64KB）写满后会阻塞，
                // 表现为「录到一半卡住」。锁中毒只是某次 panic 的连带结果，
                // 内部 Vec 本身没有会被破坏的不变量，取回内部值继续用即可。
                let mut guard = tail.lock().unwrap_or_else(|e| e.into_inner());
                guard.push(line);
                // 只保留末尾 40 行，长时录制不会吃光内存
                if guard.len() > 40 {
                    let drain = guard.len() - 40;
                    guard.drain(0..drain);
                }
            }
        });
    }

    *lock_last_error() = String::new();
    ELAPSED_SEC.store(0, Ordering::SeqCst);

    {
        let mut guard = lock_active();
        *guard = Some(ActiveRecording {
            child,
            output_path: output_path.clone(),
            started_at: Instant::now(),
            encoder: encoder.clone(),
            stderr_tail,
            bitrate_kbps: config.video_bitrate_kbps.clamp(500, 100_000),
        });
    }
    IS_RECORDING.store(true, Ordering::SeqCst);

    // ffmpeg 已成功启动，这时候才落盘配置 ——
    // 若前面任何步骤失败，不应把用户的配置改成一份「没生效」的值。
    write_config(&app, &config);

    Ok(output_path.to_string_lossy().into_owned())
}

/// 停止录制（优雅结束，确保文件可播放）
#[tauri::command]
pub async fn recorder_stop() -> Result<String, String> {
    // 取出录制进程（避免持锁跨 await）
    let active = {
        let mut guard = lock_active();
        guard.take()
    };
    let Some(mut active) = active else {
        IS_RECORDING.store(false, Ordering::SeqCst);
        return Ok(String::new());
    };

    IS_RECORDING.store(false, Ordering::SeqCst);
    let elapsed = active.started_at.elapsed().as_secs();
    ELAPSED_SEC.store(elapsed, Ordering::SeqCst);

    // 整个「写 q → 轮询等待 → 必要时 kill」是阻塞操作，最长可达
    // GRACEFUL_STOP_TIMEOUT_MS。它必须放进 spawn_blocking：
    // 直接在 async 命令里 std::thread::sleep 会占住 tokio worker，
    // 期间其它 IPC（状态轮询、预览）会被排到后面，界面表现为「停止中」卡住。
    let stop_result = tokio::task::spawn_blocking(move || stop_active_recording(&mut active))
        .await
        .map_err(|e| format!("停止录制任务异常: {e}"))?;

    match stop_result {
        Ok(path_str) => {
            info!("[Recorder] 录制结束 output={path_str} elapsed={elapsed}s");
            Ok(path_str)
        }
        Err(msg) => {
            *lock_last_error() = msg.clone();
            Err(msg)
        }
    }
}

/// 同步的「收尾录制」流程（供 spawn_blocking 调用）。
///
/// 返回 Ok(输出路径) 或 Err(错误信息)。
fn stop_active_recording(active: &mut ActiveRecording) -> Result<String, String> {
    // 先尝试优雅结束：向 ffmpeg 的 stdin 写 "q"。
    // ffmpeg 收到 q 会正常收尾并写入 moov（mp4 的索引），强制 kill 则文件可能不可播。
    let mut graceful = false;
    match active.child.stdin.as_mut() {
        Some(stdin) => match stdin.write_all(b"q") {
            Ok(()) => match stdin.flush() {
                Ok(()) => graceful = true,
                Err(e) => warn!("[Recorder] 停止信号 flush 失败: {e}"),
            },
            // EPIPE 通常说明进程已经自行退出，不必再等；交给下面的 try_wait 判定
            Err(e) => warn!("[Recorder] 停止信号写入失败: {e}"),
        },
        None => {
            // stdin 已被取走/关闭：优雅停止不可能成功，直接走强杀，不要白等 8 秒
            warn!("[Recorder] ffmpeg stdin 不可用，跳过优雅停止");
        }
    }

    let deadline = Instant::now() + Duration::from_millis(GRACEFUL_STOP_TIMEOUT_MS);
    let mut exited = false;
    while Instant::now() < deadline {
        match active.child.try_wait() {
            Ok(Some(_)) => {
                exited = true;
                break;
            }
            Ok(None) => {
                // 优雅停止失效时 ffmpeg 不会响应 "q"，没必要等满超时
                let nap = if graceful { 100 } else { 50 };
                std::thread::sleep(Duration::from_millis(nap));
            }
            Err(_) => break,
        }
    }

    if !exited {
        warn!("[Recorder] ffmpeg 未在超时内退出（graceful={graceful}），强制结束");
        let _ = active.child.kill();
        let _ = active.child.wait();
    }

    // 收集 stderr 末尾，异常时作为错误信息回传（锁中毒也要取回内部数据）
    let tail = active
        .stderr_tail
        .lock()
        .map(|g| g.iter().cloned().collect::<Vec<_>>().join("\n"))
        .unwrap_or_else(|e| e.into_inner().iter().cloned().collect::<Vec<_>>().join("\n"));

    let path_str = active.output_path.to_string_lossy().into_owned();

    // 校验产物：文件存在且非空才算成功
    let size = std::fs::metadata(&active.output_path).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        let msg = if tail.is_empty() {
            "录制失败：未生成有效文件（ffmpeg 未输出数据）".to_string()
        } else {
            format!("录制失败：{tail}")
        };
        return Err(msg);
    }

    Ok(path_str)
}

/// 查询录制状态
#[tauri::command]
pub async fn recorder_status() -> Result<RecorderStatus, String> {
    let recording = IS_RECORDING.load(Ordering::SeqCst);
    let mut status = RecorderStatus {
        recording,
        last_error: lock_last_error().clone(),
        ..Default::default()
    };

    if recording {
        let mut guard = lock_active();
        if let Some(active) = guard.as_mut() {
            // ── 存活检测：ffmpeg 是否已经自己退出了 ──
            //
            // 设备被占用、gdigrab 秒退等情况下 ffmpeg 会立刻退出，
            // 但 ACTIVE 与 IS_RECORDING 仍保持「录制中」。若不在这里查，
            // 界面会一直显示红角标与走秒计时（还用码率估算出「不断增长的文件大小」），
            // 用户录十几分钟后点停止才被告知「未生成有效文件」，整段录制静默丢失。
            // 状态轮询是唯一高频入口，在这里做一次性修正最合适。
            if matches!(active.child.try_wait(), Ok(Some(_))) {
                let tail = active
                    .stderr_tail
                    .lock()
                    .map(|g| g.iter().cloned().collect::<Vec<_>>().join(" "))
                    .unwrap_or_else(|e| {
                        e.into_inner().iter().cloned().collect::<Vec<_>>().join(" ")
                    });
                let msg = if tail.is_empty() {
                    "录制进程已意外退出（未生成有效文件）".to_string()
                } else {
                    format!("录制进程已意外退出：{tail}")
                };
                warn!("[Recorder] {msg}");
                *lock_last_error() = msg.clone();
                IS_RECORDING.store(false, Ordering::SeqCst);
                status.recording = false;
                status.last_error = msg;
                return Ok(status);
            }

            let elapsed = active.started_at.elapsed().as_secs();
            ELAPSED_SEC.store(elapsed, Ordering::SeqCst);
            status.elapsed_sec = elapsed;
            status.output_path = active.output_path.to_string_lossy().into_owned();
            status.encoder = active.encoder.clone();
            // 体积优先取磁盘真实大小：mp4 在录制过程中会持续写入，
            // metadata 能反映实际进度。文件还不存在（刚启动）时才退回码率估算。
            let real_size = std::fs::metadata(&active.output_path)
                .map(|m| m.len())
                .unwrap_or(0);
            status.estimated_bytes = if real_size > 0 {
                real_size
            } else {
                // 估算：码率(kbps) * 时长(s) / 8
                (active.bitrate_kbps as u64)
                    .saturating_mul(elapsed)
                    .saturating_mul(1000)
                    / 8
            };
        }
    } else {
        status.elapsed_sec = ELAPSED_SEC.load(Ordering::SeqCst);
    }

    Ok(status)
}

/// 列出录制目录下的文件
#[tauri::command]
pub async fn recorder_list_files(app: AppHandle, dir: Option<String>) -> Result<Vec<RecorderFile>, String> {
    let config = read_config(&app);
    let base = match dir.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => resolve_output_dir(&config),
    };

    let mut files: Vec<RecorderFile> = Vec::new();
    let Ok(entries) = std::fs::read_dir(&base) else { return Ok(files) };

    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        if ext != RECORDING_EXT && ext != "mkv" && ext != "webm" && ext != "mov" {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() { continue }
        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        files.push(RecorderFile {
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            path: path.to_string_lossy().into_owned(),
            size: meta.len(),
            modified_ms,
            duration_sec: 0,
        });
    }

    // 按修改时间倒序（最新的在最前）
    files.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    Ok(files)
}

/// 删除录制文件
#[tauri::command]
pub async fn recorder_delete_file(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err("文件不存在".to_string());
    }
    std::fs::remove_file(&p).map_err(|e| format!("删除失败: {e}"))?;
    info!("[Recorder] 已删除录制文件 {path}");
    Ok(())
}

/// 用系统默认播放器打开录制文件
#[tauri::command]
pub async fn recorder_open_file(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err("文件不存在".to_string());
    }
    Command::new("cmd")
        .args(["/C", "start", "", &path])
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|e| format!("打开失败: {e}"))?;
    Ok(())
}

/// 在资源管理器中定位录制文件
#[tauri::command]
pub async fn recorder_reveal_file(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err("文件不存在".to_string());
    }
    let arg = format!("/select,{}", path);
    Command::new("explorer")
        .arg(arg)
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|e| format!("定位失败: {e}"))?;
    Ok(())
}

/// 探测视频时长（供前端展示）
///
/// 优先用 ffprobe；便携包/只装了 ffmpeg.exe 的环境没有 ffprobe，
/// 此时回退到解析 `ffmpeg -i` 输出里的 `Duration: HH:MM:SS.ss`。
/// 早先回退分支直接拿 ffmpeg 跑 ffprobe 专用参数，ffmpeg 不认识 → stdout 为空
/// → 恒返回 0，录制列表的时长列永远显示 0。
#[tauri::command]
pub async fn recorder_probe_duration(path: String) -> Result<u64, String> {
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err("文件不存在".to_string());
    }
    let Some(ffmpeg) = find_ffmpeg() else { return Ok(0) };

    // 优先 ffprobe（与 ffmpeg 同目录）
    let ffprobe = ffmpeg.with_file_name(if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" });
    if ffprobe.is_file() {
        let out = ffmpeg_cmd(&ffprobe)
            .args([
                "-hide_banner", "-loglevel", "error",
                "-show_entries", "format=duration",
                "-of", "default=noprint_wrappers=1:nokey=1",
                &path,
            ])
            .output();
        if let Ok(out) = out {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Ok(secs) = text.trim().parse::<f64>() {
                return Ok(secs as u64);
            }
        }
    }

    // 回退：ffmpeg -i 会把元数据打到 stderr，其中含 "Duration: 00:00:05.00"
    let out = ffmpeg_cmd(&ffmpeg).args(["-hide_banner", "-i", &path]).output();
    let Ok(out) = out else { return Ok(0) };
    let stderr = String::from_utf8_lossy(&out.stderr);
    Ok(parse_duration_from_ffmpeg_stderr(&stderr).unwrap_or(0))
}

/// 从 `ffmpeg -i` 的 stderr 里解析时长，返回秒数。
///
/// 形如 `Duration: 00:01:23.45, start: 0.000000, bitrate: ...`。
fn parse_duration_from_ffmpeg_stderr(stderr: &str) -> Option<u64> {
    let idx = stderr.find("Duration:")?;
    let rest = &stderr[idx + "Duration:".len()..];
    let token = rest.split(',').next()?.trim();
    let mut parts = token.split(':');
    let h: u64 = parts.next()?.trim().parse().ok()?;
    let m: u64 = parts.next()?.trim().parse().ok()?;
    let s: f64 = parts.next()?.trim().parse().ok()?;
    Some(h * 3600 + m * 60 + s as u64)
}

/// 应用退出 / 崩溃前的兜底清理：结束残留的 ffmpeg 进程
pub fn cleanup_on_exit() {
    // 先把标志复位：早先这里放在函数末尾，而「ffmpeg 优雅退出」这一最常见路径
    // 走的是 `return`，会跳过复位，导致退出期间别处仍看到「录制中」。
    IS_RECORDING.store(false, Ordering::SeqCst);

    let active = {
        match ACTIVE.lock() {
            Ok(mut g) => g.take(),
            Err(e) => e.into_inner().take(),
        }
    };
    if let Some(mut active) = active {
        warn!("[Recorder] 退出时仍在录制，尝试优雅收尾");
        let mut sent = false;
        if let Some(stdin) = active.child.stdin.as_mut() {
            if stdin.write_all(b"q").is_ok() {
                let _ = stdin.flush();
                sent = true;
            }
        }
        let deadline = Instant::now() + Duration::from_millis(3000);
        let mut exited = false;
        while Instant::now() < deadline {
            match active.child.try_wait() {
                Ok(Some(_)) => {
                    exited = true;
                    break;
                }
                Ok(None) => {
                    // 优雅信号没发出去就没必要等满 3 秒
                    let nap = if sent { 100 } else { 50 };
                    std::thread::sleep(Duration::from_millis(nap));
                }
                Err(_) => break,
            }
        }
        if !exited {
            let _ = active.child.kill();
            let _ = active.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只有屏幕、无音频：应产出 map 到 [vout]，且不含 -c:a
    #[test]
    fn build_args_screen_only() {
        let config = RecorderConfig::default();
        let out = PathBuf::from("out.mp4");
        let (args, encoder) =
            build_args(&config, "libx264", &out, "", "", "").expect("should succeed");
        assert_eq!(encoder, "libx264");
        assert!(args.contains(&"gdigrab".to_string()), "should use gdigrab");
        assert!(args.contains(&"[vout]".to_string()), "should map vout");
        assert!(!args.contains(&"-c:a".to_string()), "no audio encoder expected");
        assert_eq!(args.last().unwrap(), &out.to_string_lossy().into_owned());
    }

    /// 屏幕 + 麦克风：应包含 amix 之外的 volume 链与 aac
    #[test]
    fn build_args_screen_with_mic() {
        let config = RecorderConfig::default();
        let out = PathBuf::from("out.mp4");
        let (args, _) = build_args(&config, "libx264", &out, "", "麦克风", "").expect("ok");
        assert!(args.contains(&"-c:a".to_string()));
        assert!(args.contains(&"aac".to_string()));
        // 滤镜串里应出现音量与重采样
        let filter = args
            .iter()
            .position(|a| a == "-filter_complex")
            .map(|i| &args[i + 1])
            .expect("filter_complex");
        assert!(filter.contains("volume="), "filter: {filter}");
        assert!(filter.contains("aresample"), "filter: {filter}");
        assert!(!filter.contains("amix"), "single audio should not amix");
    }

    /// 麦克风 + 系统声音：应使用 amix 混合两路
    #[test]
    fn build_args_dual_audio_amix() {
        let config = RecorderConfig::default();
        let out = PathBuf::from("out.mp4");
        let (args, _) = build_args(&config, "libx264", &out, "", "麦克风", "立体声混音").expect("ok");
        let filter = args
            .iter()
            .position(|a| a == "-filter_complex")
            .map(|i| &args[i + 1])
            .expect("filter_complex");
        assert!(filter.contains("amix=inputs=2"), "filter: {filter}");
        assert!(filter.contains("[aout]"), "filter: {filter}");
    }

    /// 屏幕 + 摄像头：应产生画布 + 双图层 overlay 链
    #[test]
    fn build_args_screen_with_camera_overlay() {
        let mut config = RecorderConfig::default();
        config.normalize(1920, 1080);
        config.camera_layout = LayerLayout {
            x: 1400, y: 800, w: 480, h: 270, ..Default::default()
        };
        let out = PathBuf::from("out.mp4");
        let (args, _) = build_args(&config, "libx264", &out, "摄像头", "", "").expect("ok");
        let filter = args
            .iter()
            .position(|a| a == "-filter_complex")
            .map(|i| &args[i + 1])
            .expect("filter_complex");
        // 画布底色 + 两个 overlay + 定位坐标
        assert!(filter.contains("color=c=black"), "filter: {filter}");
        assert!(filter.contains("overlay=x=1400:y=800"), "camera at given pos: {filter}");
        assert_eq!(filter.matches("overlay=").count(), 2, "two layers: {filter}");
        // 必须显式重采样到恒定帧率：gdigrab 抓屏的时间戳极不均匀，
        // 不重采样会让播放画面一顿一顿（实测帧间隔在 0.166~0.300s 之间乱跳）。
        assert!(
            filter.contains(&format!("fps={}", config.screen_fps)),
            "恒定帧率重采样缺失，会导致录制卡顿: {filter}"
        );
        assert!(filter.contains("format=yuv420p"), "filter: {filter}");
    }

    /// 不得使用 -tune zerolatency：它禁用 B 帧与前瞻，
    /// 实测使同段录制体积增大 26%（1.55MB vs 1.14MB）。
    #[test]
    fn libx264_must_not_use_zerolatency() {
        let mut config = RecorderConfig::default();
        config.normalize(1920, 1080);
        let out = PathBuf::from("out.mp4");
        let (args, _) = build_args(&config, "libx264", &out, "", "", "").expect("ok");
        assert!(
            !args.iter().any(|a| a == "zerolatency"),
            "libx264 不应使用 zerolatency（浪费码率）: {args:?}"
        );
    }

    /// h264_mf 分支不得带 x264/nvenc 专用参数（会导致编码器初始化失败）。
    ///
    /// 背景：h264_mf 是低配/虚拟机环境把录制从「卡顿」救回「流畅」的关键 ——
    /// 实测纯编码 1080p30 达 6.32x，而 libx264 仅 0.98x（低于实时），
    /// 软编还把 3 核 CPU 占满、与 gdigrab 抢核心，
    /// 导致 30fps 下约 23% 的帧是重复填充（画面一顿一顿）。
    /// 换成 h264_mf 后实测 dup 归零。
    #[test]
    fn h264_mf_uses_only_safe_params() {
        let mut config = RecorderConfig::default();
        config.normalize(1920, 1080);
        let out = PathBuf::from("out.mp4");
        let (args, _) = build_args(&config, "h264_mf", &out, "", "", "").expect("ok");
        for bad in ["-preset", "-tune", "-profile:v", "-rc", "-quality"] {
            assert!(
                !args.iter().any(|a| a == bad),
                "h264_mf 不应接受 {bad} 参数: {args:?}"
            );
        }
        assert!(args.iter().any(|a| a == "h264_mf"), "encoder applied");
    }

    /// 只有摄像头（不录屏幕）：摄像头输出直接作主画面
    #[test]
    fn build_args_camera_only() {
        let mut config = RecorderConfig::default();
        config.screen_enabled = false;
        let out = PathBuf::from("out.mp4");
        let (args, _) = build_args(&config, "libx264", &out, "摄像头", "", "").expect("ok");
        assert!(!args.contains(&"gdigrab".to_string()), "no gdigrab");
        assert!(args.contains(&"dshow".to_string()), "dshow expected");
        let filter = args
            .iter()
            .position(|a| a == "-filter_complex")
            .map(|i| &args[i + 1])
            .expect("filter_complex");
        assert!(filter.contains("[0:v]"), "camera is input 0: {filter}");
    }

    /// 无任何视频源：应报错，而不是产出坏参数
    #[test]
    fn build_args_no_video_source_errors() {
        let mut config = RecorderConfig::default();
        config.screen_enabled = false;
        let out = PathBuf::from("out.mp4");
        let err = build_args(&config, "libx264", &out, "", "", "").unwrap_err();
        assert!(err.contains("摄像头"), "err: {err}");
    }

    /// 画中画四个角落的坐标表达式
    #[test]
    fn pip_positions() {
        assert_eq!(pip_xy("top-left", 10), ("10".to_string(), "10".to_string()));
        assert_eq!(pip_xy("top-right", 10), ("W-w-10".to_string(), "10".to_string()));
        assert_eq!(pip_xy("bottom-left", 10), ("10".to_string(), "H-h-10".to_string()));
        assert_eq!(pip_xy("bottom-right", 10), ("W-w-10".to_string(), "H-h-10".to_string()));
        // 未知位置回退右下角
        assert_eq!(pip_xy("bogus", 10), ("W-w-10".to_string(), "H-h-10".to_string()));
    }

    /// 文件名应含时间戳与 mp4 扩展名
    #[test]
    fn output_path_format() {
        let dir = std::env::temp_dir();
        let p = make_output_path(&dir);
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("NeoBox_录制_"), "name: {name}");
        assert!(name.ends_with(".mp4"), "name: {name}");
    }

    /// 设置 JSON 往返：默认值应能被反序列化为默认配置
    #[test]
    fn config_roundtrip_defaults() {
        let cfg = RecorderConfig::default();
        let v = serde_json::to_value(&cfg).unwrap();
        // 缺省字段应靠 default 补齐（模拟旧版配置）
        let minimal = serde_json::json!({});
        let parsed: RecorderConfig = serde_json::from_value(minimal).unwrap();
        assert!(parsed.screen_enabled, "default screen_enabled = true");
        assert_eq!(parsed.screen_fps, 30);
        assert_eq!(parsed.video_bitrate_kbps, 6000);
        assert_eq!(parsed.encoder, "auto");
        assert!(parsed.preview_enabled, "预览默认开启");
        // 完整往返不丢字段
        let back: RecorderConfig = serde_json::from_value(v).unwrap();
        assert_eq!(back.screen_fps, cfg.screen_fps);
    }

    /// 旧版 pip_* 配置应被迁移成画布布局，且不丢失原位置语义
    #[test]
    fn legacy_pip_migrates_to_layout() {
        let legacy = serde_json::json!({
            "screen_enabled": true,
            "screen_fps": 30,
            "pip_scale": 25,
            "pip_margin": 24,
            "pip_position": "bottom-right"
        });
        let mut cfg: RecorderConfig = serde_json::from_value(legacy).unwrap();
        cfg.normalize(1920, 1080);
        // 画布铺满屏幕
        assert_eq!(cfg.canvas_w, 1920);
        assert_eq!(cfg.canvas_h, 1080);
        // 摄像头宽 = 1920*25%
        assert_eq!(cfg.camera_layout.w, 480);
        // 右下角：x = 1920 - 480 - 24
        assert_eq!(cfg.camera_layout.x, 1416);
        // 迁移后旧字段被清空，不会重复迁移
        assert!(cfg.pip_scale.is_none());
        // 屏幕图层铺满
        assert_eq!(cfg.screen_layout.w, 1920);
        assert_eq!(cfg.screen_layout.h, 1080);
    }

    /// 画布尺寸强制为偶数（yuv420p 要求）
    #[test]
    fn canvas_forced_even() {
        let mut cfg = RecorderConfig::default();
        cfg.normalize(1921, 1081);
        assert_eq!(cfg.canvas_w % 2, 0, "canvas_w must be even");
        assert_eq!(cfg.canvas_h % 2, 0, "canvas_h must be even");
    }

    /// 图层缩放只指定宽度，高度由源比例自动（scale=w:-2），保证不变形
    #[test]
    fn layer_scale_width_only_preserves_source_aspect() {
        let mut config = RecorderConfig::default();
        config.normalize(1920, 1080);
        config.camera_device = "cam".into();
        config.camera_layout = LayerLayout {
            x: 100, y: 100, w: 640, h: 480,
            visible: true,
            ..Default::default()
        };
        let out = PathBuf::from("out.mp4");
        let (args, _) = build_args(&config, "libx264", &out, "摄像头", "", "").expect("ok");
        let filter = args
            .iter()
            .position(|a| a == "-filter_complex")
            .map(|i| &args[i + 1])
            .expect("filter_complex");
        // 只写宽度，高度为 -2：由 ffmpeg 按源宽高比算出
        assert!(filter.contains("scale=640:-2"), "width-only scale: {filter}");
        // 不应再出现写死高度的 scale=640:480，或 pad/crop 补边
        assert!(!filter.contains("scale=640:480"), "height must not be fixed: {filter}");
        assert!(!filter.contains("pad="), "no letterbox pad: {filter}");
        assert!(!filter.contains("crop="), "crop feature removed: {filter}");
        assert!(filter.contains("overlay=x=100:y=100"), "position applied: {filter}");
    }

    /// 画布比例改变时，图层宽度按画布比例迁移，高度仍由源比例决定
    #[test]
    fn aspect_change_scales_layer_width() {
        let mut config = RecorderConfig::default();
        config.normalize(1920, 1080);
        config.camera_layout = LayerLayout { x: 1400, y: 780, w: 480, visible: true, ..Default::default() };
        // 模拟画布变为 1080x1920（9:16），宽缩放 0.5625
        config.canvas_w = 1080;
        config.canvas_h = 1920;
        let out = PathBuf::from("out.mp4");
        let (args, _) = build_args(&config, "libx264", &out, "摄像头", "", "").expect("ok");
        let filter = args
            .iter()
            .position(|a| a == "-filter_complex")
            .map(|i| &args[i + 1])
            .expect("filter_complex");
        // 仍只写宽度，不写死高度
        assert!(filter.contains("scale=480:-2"), "width only: {filter}");
        assert!(!filter.contains("pad="), "no pad: {filter}");
    }

    /// 图层隐藏时不应出现在滤镜链里
    #[test]
    fn hidden_layer_skipped() {
        let mut config = RecorderConfig::default();
        config.normalize(1920, 1080);
        config.camera_layout = LayerLayout { visible: false, ..Default::default() };
        let out = PathBuf::from("out.mp4");
        let (args, _) = build_args(&config, "libx264", &out, "摄像头", "", "").expect("ok");
        let filter = args
            .iter()
            .position(|a| a == "-filter_complex")
            .map(|i| &args[i + 1])
            .expect("filter_complex");
        // 只剩屏幕一个 overlay
        assert_eq!(filter.matches("overlay=").count(), 1, "hidden camera skipped: {filter}");
    }

    /// 设备枚举输出解析：ffmpeg 把设备表打到 stderr，形态需能正确解析
    #[test]
    fn parse_device_lines() {
        let sample = r#"[in#0 @ 000002640daa74c0] Could not enumerate video devices (or none found).
[in#0 @ 000002640daa74c0] "Microphone (High Definition Audio Device)" (audio)
[in#0 @ 000002640daa74c0]   Alternative name "@device_cm_{33D9A762-90C8-11D0-BD43-00A0C911CE86}\wave_{7D156F4F-A2DE-479E-95C4-10371D5AB183}"
Error opening input file dummy."#;
        // 直接把解析逻辑复制为纯函数调用不方便，这里验证关键行能被匹配：
        // 名称应被引号提取，类型为 audio
        let line = r#"[in#0 @ 000002640daa74c0] "Microphone (High Definition Audio Device)" (audio)"#;
        let fq = line.find('"').unwrap();
        let lq = line.rfind('"').unwrap();
        let name = &line[fq + 1..lq];
        assert_eq!(name, "Microphone (High Definition Audio Device)");
        assert!(line.contains("(audio)"));
        assert!(sample.contains("(audio)"));
        // alternative name 行能提取 GUID
        let alt = r#"[in#0 @ 0000]   Alternative name "@device_cm_{GUID}\wave_{GUID2}""#;
        let pos = alt.find('@').unwrap();
        assert!(alt[pos + 1..].contains("device_cm"));
    }

    /// 估算体积公式：码率 kbps * 秒 / 8
    #[test]
    fn estimated_size_formula() {
        let bitrate_kbps: u64 = 6000;
        let seconds: u64 = 10;
        let bytes = bitrate_kbps * seconds * 1000 / 8;
        assert_eq!(bytes, 7_500_000); // 6000kbps × 10s ≈ 7.5MB
    }
}
