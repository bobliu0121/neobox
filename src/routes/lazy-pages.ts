/**
 * 路由页面懒加载容器。
 *
 * 为什么集中在这里，而不是在 App.tsx 里逐个 lazy()：
 * - App.tsx 顶部还留着一批「必须常驻」的静态导入（HomePage、MainLayout、各种 Bridge/Provider），
 *   46 条 lazy 定义混进去之后，真正的首屏依赖会变得难以辨认；
 * - 独立窗口页（灵动岛 / 桌面歌词 / 悬浮框 / 传感器 / 壁纸窗）和主窗口路由页共用同一份模块缓存，
 *   集中定义能保证同一个页面只有一个 import 入口，不会因为两处写法不同而重复拉包。
 *
 * 硬性约束：只有「首屏不需要」的页面可以放进来。
 * HomePage 必须继续在 App.tsx 里静态导入 —— 它是常驻挂载的（切路由只 display 隐藏、不卸载），
 * 一旦改成 lazy，每次回到主页都要重新拉 chunk 并重挂载，反而比现在更慢。
 */
import { lazy } from "react";

export const HardwarePage = lazy(() => import("@/pages/HardwarePage"));
export const SearchPage = lazy(() => import("@/pages/SearchPage"));
export const OptimizePage = lazy(() => import("@/pages/OptimizePage"));
export const MemoryLimitPage = lazy(() => import("@/pages/MemoryLimitPage"));
export const MemoryCleanupPage = lazy(() => import("@/pages/MemoryCleanupPage"));
export const SettingsPage = lazy(() => import("@/pages/SettingsPage"));
export const DiskHealthPage = lazy(() => import("@/pages/DiskHealthPage"));
export const OverlayPanelPage = lazy(() => import("@/pages/OverlayPanelPage"));
export const BuiltinToolsPage = lazy(() => import("@/pages/BuiltinToolsPage"));
export const ResolutionConverterPage = lazy(() => import("@/pages/ResolutionConverterPage"));
export const ShaderCachePage = lazy(() => import("@/pages/ShaderCachePage"));
export const PowerManagementPage = lazy(() => import("@/pages/PowerManagementPage"));
export const StorageCleanPage = lazy(() => import("@/pages/StorageCleanPage"));
export const StartupManagerPage = lazy(() => import("@/pages/StartupManagerPage"));
export const ContextMenuManagerPage = lazy(() => import("@/pages/ContextMenuManagerPage"));
export const DownloadAcceleratorPage = lazy(() => import("@/pages/DownloadAcceleratorPage"));
export const SystemOptimizerPage = lazy(() => import("@/pages/SystemOptimizerPage"));
export const NetworkOptimizerPage = lazy(() => import("@/pages/NetworkOptimizerPage"));
export const PeripheralOptimizePage = lazy(() => import("@/pages/PeripheralOptimizePage"));
export const WindowsUpdatePage = lazy(() => import("@/pages/WindowsUpdatePage"));
export const DesktopLyricsPage = lazy(() => import("@/pages/DesktopLyricsPage"));
export const DynamicIslandWindowPage = lazy(() => import("@/pages/DynamicIslandWindowPage"));
export const VerticalOverlayPage = lazy(() => import("@/pages/VerticalOverlayPage"));
export const SensorMonitorPage = lazy(() => import("@/pages/SensorMonitorPage"));
export const RuntimeRepairPage = lazy(() => import("@/pages/RuntimeRepairPage"));
export const AppManagerPage = lazy(() => import("@/pages/AppManagerPage"));
export const VtxVirtualizationPage = lazy(() => import("@/pages/VtxVirtualizationPage"));
export const HiddenFeaturesPage = lazy(() => import("@/pages/HiddenFeaturesPage"));
export const AudioEqPage = lazy(() => import("@/pages/AudioEqPage"));
export const AutoClickerPage = lazy(() => import("@/pages/AutoClickerPage"));
export const CpuSchedulerPage = lazy(() => import("@/pages/CpuSchedulerPage"));
export const SpeedTestPage = lazy(() => import("@/pages/SpeedTestPage"));
export const CustomPage = lazy(() => import("@/pages/CustomPage"));
export const NvidiaRecordingPage = lazy(() => import("@/pages/NvidiaRecordingPage"));
export const TimeSyncPage = lazy(() => import("@/pages/TimeSyncPage"));
export const ScreenTestPage = lazy(() => import("@/pages/ScreenTestPage"));
export const WallpaperPage = lazy(() => import("@/pages/WallpaperPage"));
export const WallpaperWindowPage = lazy(() => import("@/pages/WallpaperWindowPage"));
export const BsodLogPage = lazy(() => import("@/pages/BsodLogPage"));
export const ImageConverterPage = lazy(() => import("@/pages/ImageConverterPage"));
export const AudioConverterPage = lazy(() => import("@/pages/AudioConverterPage"));
export const AudioReversePage = lazy(() => import("@/pages/AudioReversePage"));
export const AudioSpeedPage = lazy(() => import("@/pages/AudioSpeedPage"));
export const VideoConverterPage = lazy(() => import("@/pages/VideoConverterPage"));
export const MkaMuxerPage = lazy(() => import("@/pages/MkaMuxerPage"));
export const CorruptFilePage = lazy(() => import("@/pages/CorruptFilePage"));
export const BilibiliDownloadPage = lazy(() => import("@/pages/BilibiliDownloadPage"));
export const MusicPage = lazy(() => import("@/pages/MusicPage"));

/**
 * 预取音乐页 chunk。
 *
 * 为什么需要它：把 .mka 拖到 exe 打开时，MainWindowEffects 会先 navigate("/music")，
 * 再最多等 2 秒让 MusicPage 挂载后把 <audio> 写进 store（audioRef），超时就放弃播放。
 * 页面改成懒加载后这 2 秒预算里多了一次网络/磁盘取 chunk，冷启动机械硬盘上有风险，
 * 所以这里在 navigate 之前先把模块拉下来（与 lazy 指向同一模块，命中模块缓存后不重复请求）。
 */
export const preloadMusicPage = () => import("@/pages/MusicPage");

/**
 * 路由 → 页面 chunk 预取函数。
 *
 * 用途：侧边栏在鼠标悬停/聚焦时提前把目标页 chunk 取进模块缓存，
 * 使点击导航时 Suspense 直接命中、不出现占位闪烁。
 *
 * 为什么单独维护一张表：`React.lazy` 的 loader 无法在外部读取，
 * 所以只能把「路由 → import()」再声明一遍。为避免与上方 lazy 定义漂移，
 * 两者必须指向同一个模块路径（同一模块被 import 两次只会请求一次）。
 * 未收录的路径（例如未来新增页）直接静默跳过，不影响导航。
 */
const ROUTE_PRELOADERS: Record<string, () => Promise<unknown>> = {
  "/music": () => import("@/pages/MusicPage"),
  "/search": () => import("@/pages/SearchPage"),
  "/hardware": () => import("@/pages/HardwarePage"),
  "/builtin-tools": () => import("@/pages/BuiltinToolsPage"),
  "/wallpaper": () => import("@/pages/WallpaperPage"),
  "/optimization": () => import("@/pages/OptimizePage"),
  "/system-optimizer": () => import("@/pages/SystemOptimizerPage"),
  "/optimize/network-optimizer": () => import("@/pages/NetworkOptimizerPage"),
  "/optimize/storage-clean": () => import("@/pages/StorageCleanPage"),
  "/optimize/startup-manager": () => import("@/pages/StartupManagerPage"),
  "/optimize/power-management": () => import("@/pages/PowerManagementPage"),
  "/optimize/shader-cache": () => import("@/pages/ShaderCachePage"),
  "/optimize/memory-cleanup": () => import("@/pages/MemoryCleanupPage"),
  "/optimize/memory-limit": () => import("@/pages/MemoryLimitPage"),
  "/optimize/peripheral-optimize": () => import("@/pages/PeripheralOptimizePage"),
  "/optimize/windows-update": () => import("@/pages/WindowsUpdatePage"),
  "/optimize/cpu-scheduler": () => import("@/pages/CpuSchedulerPage"),
  "/settings": () => import("@/pages/SettingsPage"),
  "/disk-health": () => import("@/pages/DiskHealthPage"),
  "/overlay-panel": () => import("@/pages/OverlayPanelPage"),
  "/resolution-converter": () => import("@/pages/ResolutionConverterPage"),
  "/audio-eq": () => import("@/pages/AudioEqPage"),
  "/speedtest": () => import("@/pages/SpeedTestPage"),
  "/custom": () => import("@/pages/CustomPage"),
  "/runtime-repair": () => import("@/pages/RuntimeRepairPage"),
  "/app-manager": () => import("@/pages/AppManagerPage"),
  "/vtx-virtualization": () => import("@/pages/VtxVirtualizationPage"),
  "/hidden-features": () => import("@/pages/HiddenFeaturesPage"),
  "/context-menu": () => import("@/pages/ContextMenuManagerPage"),
  "/download-accelerator": () => import("@/pages/DownloadAcceleratorPage"),
  "/nvidia-recording": () => import("@/pages/NvidiaRecordingPage"),
  "/time-sync": () => import("@/pages/TimeSyncPage"),
  "/screen-test": () => import("@/pages/ScreenTestPage"),
  "/bsod-log": () => import("@/pages/BsodLogPage"),
  "/image-converter": () => import("@/pages/ImageConverterPage"),
  "/audio-converter": () => import("@/pages/AudioConverterPage"),
  "/audio-reverse": () => import("@/pages/AudioReversePage"),
  "/audio-speed": () => import("@/pages/AudioSpeedPage"),
  "/video-converter": () => import("@/pages/VideoConverterPage"),
  "/mka-muxer": () => import("@/pages/MkaMuxerPage"),
  "/corrupt-file": () => import("@/pages/CorruptFilePage"),
  "/bilibili-download": () => import("@/pages/BilibiliDownloadPage"),
  "/autoclicker": () => import("@/pages/AutoClickerPage"),
};

/** 已触发过的预取路径：避免同一按钮反复 hover 时重复调用（幂等但多余） */
const prefetched = new Set<string>();

/**
 * 预取指定路由的页面 chunk。找不到映射或已预取过则什么都不做。
 * 失败一律吞掉：预取只是优化，绝不能因为它抛错而影响导航。
 */
export function preloadRouteChunk(path: string): void {
  const loader = ROUTE_PRELOADERS[path];
  if (!loader || prefetched.has(path)) return;
  prefetched.add(path);
  loader().catch(() => {
    // 预取失败不记录到「已预取」，允许下次 hover 重试
    prefetched.delete(path);
  });
}
