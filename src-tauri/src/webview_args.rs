//! 全窗口统一的 WebView2 启动参数。
//!
//! WebView2 环境在每个进程内共享：不同窗口传入的 `additionalBrowserArgs`
//! 必须完全一致，否则新窗口会因环境参数冲突而创建失败。所有动态创建的
//! 窗口（竖排悬浮框、桌面灵动岛、传感器监控、桌面歌词、动态壁纸等）都
//! 从这里取同一份字符串，避免各写一份后悄悄漂移。
//!
//! 参数说明（抗闪烁四件套）：
//! - `CalculateNativeWinOcclusion` 关闭 Chromium 原生遮挡计算。该计算对
//!   置底/透明/悬浮窗口会误判“被遮挡”，从而挂起渲染，表现为动画卡顿或
//!   闪烁；动态壁纸正是一个长期“被遮挡”的置底窗口，必须关闭。
//! - 其余三个禁用后台渲染降频、后台计时器节流，保证失焦后 GIF 仍按帧率播放。
//! - `MediaSessionService` / `HardwareMediaKeyHandling` 关闭 Chromium 自带
//!   媒体会话，避免与 smtc.rs 注册的系统媒体会话重复。
pub const BROWSER_ARGS: &str = "--disable-features=MediaSessionService,HardwareMediaKeyHandling,msWebOOUI,msPdfOOUI,msSmartScreenProtection,msEdgeAutofill,msEdgeShopping,msEdgeWallet,CalculateNativeWinOcclusion --autoplay-policy=no-user-gesture-required --disable-background-networking --disable-client-side-phishing-detection --disable-component-update --disable-default-apps --disable-extensions --disable-sync --disable-backgrounding-occluded-windows --disable-renderer-backgrounding --disable-background-timer-throttling";
