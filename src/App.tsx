import { Routes, Route, useLocation, Navigate } from "react-router-dom";
import { Box } from "@chakra-ui/react";
import { MainLayout } from "./components/ui/main-layout";
import HomePage from "./pages/HomePage";
import HardwarePage from "./pages/HardwarePage";
import OptimizePage from "./pages/OptimizePage";
import MemoryLimitPage from "./pages/MemoryLimitPage";
import MemoryCleanupPage from "./pages/MemoryCleanupPage";
import SettingsPage from "./pages/SettingsPage";
import DiskHealthPage from "./pages/DiskHealthPage";
import OverlayPanelPage from "./pages/OverlayPanelPage";
import BuiltinToolsPage from "./pages/BuiltinToolsPage";
import ResolutionConverterPage from "./pages/ResolutionConverterPage";
import ShaderCachePage from "./pages/ShaderCachePage";
import PowerManagementPage from "./pages/PowerManagementPage";
import StorageCleanPage from "./pages/StorageCleanPage";
import StartupManagerPage from "./pages/StartupManagerPage";
import ContextMenuManagerPage from "./pages/ContextMenuManagerPage";
import DownloadAcceleratorPage from "./pages/DownloadAcceleratorPage";
import SystemOptimizerPage from "./pages/SystemOptimizerPage";
import NetworkOptimizerPage from "./pages/NetworkOptimizerPage";
import PeripheralOptimizePage from "./pages/PeripheralOptimizePage";
import WindowsUpdatePage from "./pages/WindowsUpdatePage";
import DesktopLyricsPage from "./pages/DesktopLyricsPage";
import DynamicIslandWindowPage from "./pages/DynamicIslandWindowPage";
import VerticalOverlayPage from "./pages/VerticalOverlayPage";
import SensorMonitorPage from "./pages/SensorMonitorPage";
import RuntimeRepairPage from "./pages/RuntimeRepairPage";
import AppManagerPage from "./pages/AppManagerPage";
import VtxVirtualizationPage from "./pages/VtxVirtualizationPage";
import HiddenFeaturesPage from "./pages/HiddenFeaturesPage";
import AudioEqPage from "./pages/AudioEqPage";
import AutoClickerPage from "./pages/AutoClickerPage";
import CpuSchedulerPage from "./pages/CpuSchedulerPage";
import SpeedTestPage from "./pages/SpeedTestPage";
import CustomPage from "./pages/CustomPage";
import NvidiaRecordingPage from "./pages/NvidiaRecordingPage";
import TimeSyncPage from "./pages/TimeSyncPage";
import ScreenTestPage from "./pages/ScreenTestPage";
import WallpaperPage from "./pages/WallpaperPage";
import WallpaperWindowPage from "./pages/WallpaperWindowPage";
import BsodLogPage from "./pages/BsodLogPage";
import ImageConverterPage from "./pages/ImageConverterPage";
import AudioConverterPage from "./pages/AudioConverterPage";
import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

import { UpdateModal } from "./components/UpdateModal";
import { SplashScreen } from "./components/SplashScreen";
import { StartupAdModal } from "./components/ads/startup-ad-popup";
import { useAds } from "./hooks/use-ads";
import { useAppStartup } from "./contexts/app-startup-context";
import { MusicProvider } from "./contexts/music-context";
import MusicPage from "./pages/MusicPage";
import { ImportantAnnouncementModal } from "./components/ImportantAnnouncementModal";
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

  // 桌面灵动岛独立窗口：只负责渲染，状态真源在主窗口（见 IslandRemoteBridge）
  if (location.pathname === "/dynamic-island") {
    return <DynamicIslandWindowPage />;
  }

  // Desktop lyrics window: render standalone, no main layout
  if (location.pathname === "/desktop-lyrics") {
    return <DesktopLyricsPage />;
  }

  // 解锁按钮已内嵌到桌面歌词窗口,不再使用独立窗口

  // Vertical overlay window: standalone, no main layout
  if (location.pathname === "/vertical-overlay") {
    return <VerticalOverlayPage />;
  }

  // Sensor monitor window: standalone, no main layout
  if (location.pathname === "/sensor-monitor") {
    return <SensorMonitorPage />;
  }

  // Dynamic wallpaper window: standalone, transparent, no main layout
  if (location.pathname === "/wallpaper-window") {
    return <WallpaperWindowPage />;
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
              <Routes location={location}>
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
                </Routes>
          )}
        </MainLayout>

        <UpdateModal />
        <ImportantAnnouncementModal />
        <DynamicIslandHost />
        <AccelIslandBridge />
        <IslandRemoteBridge />
      </>
    </MusicProvider>
  );
}

export default App;
