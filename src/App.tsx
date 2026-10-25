import { Routes, Route, useLocation, Navigate } from "react-router-dom";
import { Box } from "@chakra-ui/react";
import { MainLayout } from "./components/ui/main-layout";
import HomePage from "./pages/HomePage";
// 除 HomePage 外的路由页面全部改为懒加载（定义集中在 routes/lazy-pages.ts）：
// 之前这 46 个页面被静态导入，导致启动时 WebView2 必须先解析完 MusicPage / SettingsPage
// 这类数千行的重型模块才能画出第一帧；改成按需 import 后，首屏只解析真正用到的代码。
import {
  HardwarePage,
  SearchPage,
  OptimizePage,
  MemoryLimitPage,
  MemoryCleanupPage,
  SettingsPage,
  DiskHealthPage,
  OverlayPanelPage,
  BuiltinToolsPage,
  ResolutionConverterPage,
  ShaderCachePage,
  PowerManagementPage,
  StorageCleanPage,
  StartupManagerPage,
  ContextMenuManagerPage,
  DownloadAcceleratorPage,
  SystemOptimizerPage,
  NetworkOptimizerPage,
  PeripheralOptimizePage,
  WindowsUpdatePage,
  DesktopLyricsPage,
  DynamicIslandWindowPage,
  VerticalOverlayPage,
  SensorMonitorPage,
  RuntimeRepairPage,
  AppManagerPage,
  VtxVirtualizationPage,
  HiddenFeaturesPage,
  AudioEqPage,
  AutoClickerPage,
  CpuSchedulerPage,
  SpeedTestPage,
  CustomPage,
  NvidiaRecordingPage,
  TimeSyncPage,
  ScreenTestPage,
  WallpaperPage,
  WallpaperWindowPage,
  BsodLogPage,
  ImageConverterPage,
  AudioConverterPage,
  AudioReversePage,
  AudioSpeedPage,
  VideoConverterPage,
  MkaMuxerPage,
  BilibiliDownloadPage,
  MusicPage,
  preloadMusicPage,
} from "./routes/lazy-pages";
import {
  RouteErrorBoundary,
  RouteFallback,
  TransparentWindowFallback,
  WindowFallback,
} from "./routes/route-fallback";
import { Suspense, useState, useEffect, useCallback, useRef } from "react";
import { useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useMusicStore } from "./stores/music-store";

import { UpdateModal } from "./components/UpdateModal";
import { SplashScreen } from "./components/SplashScreen";
import { StartupAdModal } from "./components/ads/startup-ad-popup";
import { useAds } from "./hooks/use-ads";
import { useAppStartup } from "./contexts/app-startup-context";
import { MusicProvider } from "./contexts/music-context";

import { DynamicIslandHost } from "./components/ui/dynamic-island";
import { AccelIslandBridge } from "./components/ui/accel-island-bridge";
import { IslandRemoteBridge } from "./components/ui/island-remote-bridge";
import { store } from "./lib/store";
import {
  DYNAMIC_WALLPAPER_STORE_KEY,
  dynamicWallpaperKind,
  setDynamicWallpaper,
  type DynamicWallpaperInfo,
} from "./lib/dynamic-wallpaper";

/** 启动完成后展示一次开屏广告弹窗（ads 为空时不显示任何内容） */
function StartupAdHost() {
  const { splash } = useAds();
  return <StartupAdModal ads={splash} />;
}

/**
 * 仅主窗口需要执行的启动副作用。
 *
 * 放在独立组件里而不是 App 内部：App 对各个独立窗口有早返回，
 * 主窗口专用的 Hook 如果写在早返回之后会违反 Hooks 调用顺序。
 */
function MainWindowEffects() {
  const navigate = useNavigate();
  const lastOpenMkaRef = useRef<{ path: string; at: number } | null>(null);

  // 把 .mka 拖到 exe（或通过文件关联启动）后：导入本地歌库、立即播放并打开播放器
  const openMkaFile = useCallback(async (path: string) => {
    const key = path.toLowerCase();
    const last = lastOpenMkaRef.current;
    if (last && last.path === key && Date.now() - last.at < 1500) return;
    lastOpenMkaRef.current = { path: key, at: Date.now() };
    try {
      const store = useMusicStore.getState();
      await store.importLocalSongs([path]);
      const updated = useMusicStore.getState();
      const song = updated.localSongs.find(
        (item) => (item._localPath || "").toLowerCase() === key
      );
      if (!song) {
        console.warn("[OpenMKA] imported but song not found:", path);
        return;
      }
      // 先进入音乐页并等待音频元素挂载，否则 playSong 会因 audioRef 为空直接返回
      // MusicPage 现在是懒加载：先把 chunk 取回来再 navigate，避免「取 chunk + 挂载」两段耗时
      // 把后面的 2 秒等待预算挤爆（冷启动机械盘上尤其明显）。
      void preloadMusicPage().catch(() => {});
      navigate("/music", { state: { expandPlayer: true } });
      for (let i = 0; i < 40 && !useMusicStore.getState().audioRef; i += 1) {
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
      await useMusicStore.getState().playSong(song, [song]);
    } catch (e) {
      console.error("[OpenMKA] open failed:", e);
      lastOpenMkaRef.current = null;
    }
  }, [navigate]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await listen<string>("open-mka-file", (event) => {
          if (typeof event.payload === "string") void openMkaFile(event.payload);
        });
        // 处理启动时命令行参数（拖到 exe 打开）
        const pending = await invoke<string | null>("take_pending_open_mka");
        if (!disposed && pending) await openMkaFile(pending);
      } catch (e) {
        console.error("[OpenMKA] listener setup failed:", e);
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [openMkaFile]);

  // 开机自启(--autostart)模式：后端已离屏预热加载本窗口，前端初始化完成后隐藏到托盘，
  // 复用 minimize_to_tray 正确更新后端可见性并触发 EcoQoS。
  useEffect(() => {
    (async () => {
      try {
        const autostart = await invoke<boolean>("is_autostart_mode");
        if (autostart) await invoke("minimize_to_tray");
      } catch (e) {
        console.error("autostart hide check failed:", e);
      }
    })();
  }, []);

  // 恢复上次使用的动态壁纸（GIF / HTML）。延迟到启动流程稳定后再创建置底窗口，
  // 避免与开屏动画/主窗口首帧抢 WebView2 资源。
  useEffect(() => {
    let cancelled = false;
    const timer = setTimeout(async () => {
      try {
        const saved = await store.get<DynamicWallpaperInfo>(DYNAMIC_WALLPAPER_STORE_KEY);
        if (cancelled || !saved?.path || !dynamicWallpaperKind(saved.path)) return;
        await setDynamicWallpaper(saved.path);
      } catch (e) {
        console.error("恢复动态壁纸失败:", e);
      }
    }, 1500);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, []);

  return null;
}

function App() {
  const { isStartupComplete } = useAppStartup();
  const location = useLocation();

  // 独立窗口页同样懒加载：主窗口的启动流程不解析它们，每个窗口只付自己那份 chunk 的代价。
  // fallback 按窗口底色区分——透明窗口不能画 Spinner（见 route-fallback.tsx 的说明）。
  // 桌面灵动岛独立窗口：只负责渲染，状态真源在主窗口（见 IslandRemoteBridge）
  if (location.pathname === "/dynamic-island") {
    return (
      <Suspense fallback={<TransparentWindowFallback />}>
        <DynamicIslandWindowPage />
      </Suspense>
    );
  }

  // Desktop lyrics window: render standalone, no main layout
  if (location.pathname === "/desktop-lyrics") {
    return (
      <Suspense fallback={<TransparentWindowFallback />}>
        <DesktopLyricsPage />
      </Suspense>
    );
  }

  // 解锁按钮已内嵌到桌面歌词窗口,不再使用独立窗口

  // Vertical overlay window: standalone, no main layout
  if (location.pathname === "/vertical-overlay") {
    return (
      <Suspense fallback={<TransparentWindowFallback />}>
        <VerticalOverlayPage />
      </Suspense>
    );
  }

  // Sensor monitor window: standalone, no main layout
  if (location.pathname === "/sensor-monitor") {
    return (
      <Suspense fallback={<WindowFallback />}>
        <SensorMonitorPage />
      </Suspense>
    );
  }

  // Dynamic wallpaper window: standalone, transparent, no main layout
  if (location.pathname === "/wallpaper-window") {
    return (
      <Suspense fallback={<TransparentWindowFallback />}>
        <WallpaperWindowPage />
      </Suspense>
    );
  }

  return (
    <MusicProvider>
      <>
        <MainWindowEffects />
        {!isStartupComplete && <SplashScreen />}
        {isStartupComplete && <StartupAdHost />}
        <MainLayout>
          {/* 主页常驻挂载：路由切换只隐藏、不卸载，避免每次回到主页重新加载硬件信息与快捷启动 */}
          <Box display={location.pathname === "/" ? "block" : "none"}>
            <HomePage />
          </Box>
          {location.pathname !== "/" && (
            // Suspense 只包住内容区，不包 MainLayout：取页面 chunk 期间标题栏与侧边栏仍然在位，
            // 用户看到的是「框架 + 内容区转圈」，而不是整窗退回白屏。
            // 占位高度固定（route-fallback.tsx），chunk 到达后不会出现内容塌陷再撑开的跳动。
            // ErrorBoundary 在 Suspense 外层：某个页面 chunk 取失败时只坏内容区，
            // 侧边栏仍能切走（没有边界会卸载整棵树，变成整窗白屏且无法自救）。
            <RouteErrorBoundary area="该页面">
              <Suspense fallback={<RouteFallback />}>
                <Routes location={location}>
                  <Route
                    path="/search"
                    element={
                      <SearchPage />
                    }
                  />
                  <Route
                    path="/hardware"
                    element={
                      <HardwarePage />
                    }
                  />
                  <Route
                    path="/builtin-tools"
                    element={
                      <BuiltinToolsPage />
                    }
                  />
                  <Route
                    path="/wallpaper"
                    element={
                      <WallpaperPage />
                    }
                  />
                  <Route
                    path="/optimization"
                    element={
                      <OptimizePage />
                    }
                  />
                  <Route
                    path="/optimize"
                    element={
                      <OptimizePage />
                    }
                  />
                  <Route
                    path="/optimize/memory-cleanup"
                    element={
                      <MemoryCleanupPage />
                    }
                  />
                  <Route
                    path="/optimize/memory-limit"
                    element={
                      <MemoryLimitPage />
                    }
                  />
                  <Route
                    path="/settings"
                    element={
                      <SettingsPage />
                    }
                  />
                  <Route
                    path="/autoclicker"
                    element={
                      <AutoClickerPage />
                    }
                  />
                  <Route
                    path="/disk-health"
                    element={
                      <DiskHealthPage />
                    }
                  />
                  <Route
                    path="/overlay-panel"
                    element={
                      <OverlayPanelPage />
                    }
                  />
                  <Route
                    path="/resolution-converter"
                    element={
                      <ResolutionConverterPage />
                    }
                  />
                  <Route
                    path="/optimize/shader-cache"
                    element={
                      <ShaderCachePage />
                    }
                  />
                  <Route
                    path="/optimize/power-management"
                    element={
                      <PowerManagementPage />
                    }
                  />
                  <Route
                    path="/optimize/storage-clean"
                    element={
                      <StorageCleanPage />
                    }
                  />
                  <Route
                    path="/optimize/startup-manager"
                    element={
                      <StartupManagerPage />
                    }
                  />
                  <Route
                    path="/system-optimizer"
                    element={
                      <SystemOptimizerPage />
                    }
                  />
                  {/* 旧路径（原优化页子页）重定向到独立导航页 */}
                  <Route
                    path="/optimize/system-optimizer"
                    element={
                      <Navigate to="/system-optimizer" replace />
                    }
                  />
                  <Route
                    path="/optimize/network-optimizer"
                    element={
                      <NetworkOptimizerPage />
                    }
                  />
                  <Route
                    path="/optimize/peripheral-optimize"
                    element={
                      <PeripheralOptimizePage />
                    }
                  />
                  <Route
                    path="/optimize/windows-update"
                    element={
                      <WindowsUpdatePage />
                    }
                  />
                  <Route
                    path="/optimize/cpu-scheduler"
                    element={
                      <CpuSchedulerPage />
                    }
                  />
                  <Route
                    path="/audio-eq"
                    element={
                      <AudioEqPage />
                    }
                  />
                  <Route
                    path="/music"
                    element={
                      <MusicPage />
                    }
                  />
                  <Route
                    path="/custom"
                    element={
                      <CustomPage />
                    }
                  />
                  <Route
                    path="/speedtest"
                    element={
                      <SpeedTestPage />
                    }
                  />
                  <Route
                    path="/runtime-repair"
                    element={
                      <RuntimeRepairPage />
                    }
                  />
                  <Route
                    path="/app-manager"
                    element={
                      <AppManagerPage />
                    }
                  />
                  <Route
                    path="/vtx-virtualization"
                    element={
                      <VtxVirtualizationPage />
                    }
                  />
                  <Route
                    path="/hidden-features"
                    element={
                      <HiddenFeaturesPage />
                    }
                  />
                  <Route
                    path="/context-menu"
                    element={
                      <ContextMenuManagerPage />
                    }
                  />
                  <Route
                    path="/download-accelerator"
                    element={
                      <DownloadAcceleratorPage />
                    }
                  />
                  <Route
                    path="/nvidia-recording"
                    element={
                      <NvidiaRecordingPage />
                    }
                  />
                  <Route
                    path="/time-sync"
                    element={
                      <TimeSyncPage />
                    }
                  />
                  <Route
                    path="/screen-test"
                    element={
                      <ScreenTestPage />
                    }
                  />
                  <Route
                    path="/bsod-log"
                    element={
                      <BsodLogPage />
                    }
                  />
                  <Route
                    path="/image-converter"
                    element={
                      <ImageConverterPage />
                    }
                  />
                  <Route
                    path="/audio-converter"
                    element={
                      <AudioConverterPage />
                    }
                  />
                  <Route
                    path="/audio-reverse"
                    element={
                      <AudioReversePage />
                    }
                  />
                  <Route
                    path="/audio-speed"
                    element={
                      <AudioSpeedPage />
                    }
                  />
                  <Route
                    path="/video-converter"
                    element={
                      <VideoConverterPage />
                    }
                  />
                  <Route
                    path="/mka-muxer"
                    element={
                      <MkaMuxerPage />
                    }
                  />
                  <Route
                    path="/bilibili-download"
                    element={
                      <BilibiliDownloadPage />
                    }
                  />
                </Routes>
              </Suspense>
            </RouteErrorBoundary>
          )}
        </MainLayout>

        <UpdateModal />
        <DynamicIslandHost />
        <AccelIslandBridge />
        <IslandRemoteBridge />
      </>
    </MusicProvider>
  );
}

export default App;
