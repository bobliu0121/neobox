import React, { useState, useCallback, useEffect, useRef } from "react";
import {
  Box,
  Text,
  Heading,
  VStack,
  HStack,
  IconButton,
  Button,
  Switch,
  NumberInput,
  NumberInputField,
  NumberInputStepper,
  NumberIncrementStepper,
  NumberDecrementStepper,
  SimpleGrid,
  Spinner,
  Image,
  Tag,
  TagLabel,
  TagCloseButton,
  Badge,
  useColorModeValue,
  useToast,
} from "@chakra-ui/react";
import { useNavigate } from "react-router-dom";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import {
  ArrowLeft,
  Images,
  FolderOpen,
  FolderPlus,
  RefreshCw,
  Play,
  ListMusic,
  Trash2,
  Pencil,
  Film,
  FileCode2,
  MonitorPlay,
  Square,
} from "lucide-react";
import { LiquidGlassCard } from "@/components/special/liquid-glass-card";
import { useThemeColor } from "@/contexts/theme-color-context";
import { useBackground } from "@/contexts/background-context";
import { store } from "@/lib/store";
import {
  DYNAMIC_WALLPAPER_STORE_KEY,
  dynamicWallpaperKind,
  getDynamicWallpaper,
  setDynamicWallpaper,
  stopDynamicWallpaper,
  type DynamicWallpaperInfo,
  type DynamicWallpaperKind,
} from "@/lib/dynamic-wallpaper";
import WallpaperEditor from "./WallpaperEditor";

const WALLPAPER_HISTORY_KEY = "wallpaper_history";
const WALLPAPER_ROTATE_KEY = "wallpaper_rotate_enabled";
const WALLPAPER_INTERVAL_KEY = "wallpaper_rotate_interval_minutes";
const WALLPAPER_PLAYLIST_KEY = "wallpaper_playlist";
const WALLPAPER_FOLDERS_KEY = "wallpaper_folders";

interface HistoryItem {
  path: string;
  time: number;
}

interface PlaylistItem {
  path: string;
  durationMinutes: number;
}

interface CtxMenuState {
  x: number;
  y: number;
  path: string;
}

function dedupe(paths: string[]): string[] {
  return Array.from(new Set(paths));
}

/** BETA 标记：GIF / HTML 动态壁纸仍是实验性功能。 */
function BetaTag() {
  return (
    <Badge colorScheme="purple" variant="solid" fontSize="2xs" px={1.5} borderRadius="sm">
      BETA
    </Badge>
  );
}

/** 懒加载容器：只在元素进入可视区域后才渲染子内容 */
function LazyMount({ children }: { children: React.ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries[0].isIntersecting) {
          setVisible(true);
          io.disconnect();
        }
      },
      { rootMargin: "300px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, []);
  return (
    <div ref={ref} style={{ width: "100%", aspectRatio: "16 / 9", overflow: "hidden" }}>
      {visible ? children : null}
    </div>
  );
}

/** HTML 壁纸缩略图：懒加载 iframe，整页缩放 */
const HtmlThumb = React.memo(function HtmlThumb({ path }: { path: string }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [scale, setScale] = useState(0.2);
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      const w = el.clientWidth;
      setScale(w / 1920);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return (
    <div ref={containerRef} style={{ width: "100%", height: "100%", background: "#1a202c", overflow: "hidden", position: "relative" }}>
      <div
        style={{
          position: "absolute",
          top: 0,
          left: 0,
          width: "1920px",
          height: "1080px",
          transform: `scale(${scale})`,
          transformOrigin: "top left",
        }}
      >
        <iframe
          src={convertFileSrc(path)}
          style={{ width: "1920px", height: "1080px", border: "none", pointerEvents: "none" }}
          sandbox="allow-scripts allow-same-origin"
          loading="lazy"
        />
      </div>
    </div>
  );
});

/** 壁纸缩略图：懒加载，HTML/视频不自动播放 */
const WallpaperThumb = React.memo(function WallpaperThumb({ path }: { path: string }) {
  const kind = dynamicWallpaperKind(path);
  if (kind === "html") {
    return (
      <LazyMount>
        <HtmlThumb path={path} />
      </LazyMount>
    );
  }
  if (kind === "video") {
    return (
      <LazyMount>
        <div style={{ width: "100%", height: "100%", background: "black", overflow: "hidden" }}>
          <video
            src={convertFileSrc(path)}
            preload="metadata"
            muted
            playsInline
            style={{ width: "100%", height: "100%", objectFit: "cover", pointerEvents: "none" }}
          />
        </div>
      </LazyMount>
    );
  }
  return (
    <LazyMount>
      <img
        src={convertFileSrc(path)}
        alt=""
        style={{ width: "100%", height: "100%", objectFit: "cover", display: "block" }}
        loading="lazy"
      />
    </LazyMount>
  );
});

export default function WallpaperPage() {
  const navigate = useNavigate();
  const { t } = useTranslation();
  const toast = useToast();
  const { config: themeConfig } = useThemeColor();
  const { liquidGlassEnabled, liquidGlassBlur } = useBackground();
  const themeColorHex = themeConfig.primaryColor;
  const headingColor = useColorModeValue("gray.800", "#ffffff");
  const subTextColor = useColorModeValue("gray.500", "#ffffff");
  const menuBg = useColorModeValue("white", "gray.800");
  const menuBorder = useColorModeValue("gray.200", "gray.600");

  interface WallpaperFolder {
  path: string;
  recursive: boolean;
}

const [folders, setFolders] = useState<WallpaperFolder[]>([]);
  const [wallpapers, setWallpapers] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [history, setHistory] = useState<HistoryItem[]>([]);
  const [rotateEnabled, setRotateEnabled] = useState(false);
  const [rotateInterval, setRotateInterval] = useState(10);
  const [settingIndex, setSettingIndex] = useState<number | null>(null);
  const [playlist, setPlaylist] = useState<PlaylistItem[]>([]);
  const [playlistIndex, setPlaylistIndex] = useState(0);
  const [tick, setTick] = useState(0);
  const [ctxMenu, setCtxMenu] = useState<CtxMenuState | null>(null);
  const [editingPath, setEditingPath] = useState<string | null>(null);
  const [dynamic, setDynamic] = useState<DynamicWallpaperInfo | null>(null);
  const [dynamicBusy, setDynamicBusy] = useState(false);
  // HTML 壁纸交互设置（持久化到 localStorage）
  const [htmlMouse, setHtmlMouse] = useState(() => localStorage.getItem("wp_html_mouse") !== "0");
  const [htmlKeyboard, setHtmlKeyboard] = useState(() => localStorage.getItem("wp_html_keyboard") === "1");
  const [htmlAudio, setHtmlAudio] = useState(() => localStorage.getItem("wp_html_audio") !== "0");
  const rotateIndexRef = useRef(0);

  // 读取持久化状态
  useEffect(() => {
    (async () => {
      const [hist, rot, interval, pl] = await Promise.all([
        store.get<HistoryItem[]>(WALLPAPER_HISTORY_KEY),
        store.get<boolean>(WALLPAPER_ROTATE_KEY),
        store.get<number>(WALLPAPER_INTERVAL_KEY),
        store.get<PlaylistItem[]>(WALLPAPER_PLAYLIST_KEY),
      ]);
      if (Array.isArray(hist)) setHistory(hist);
      if (typeof rot === "boolean") setRotateEnabled(rot);
      if (typeof interval === "number" && interval > 0) setRotateInterval(interval);
      if (Array.isArray(pl)) setPlaylist(pl);
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // folders 持久化到 store（settings.json）+ 恢复时重新扫描壁纸
  useEffect(() => {
    (async () => {
      try {
        const saved = await store.get<WallpaperFolder[]>(WALLPAPER_FOLDERS_KEY);
        if (Array.isArray(saved) && saved.length > 0) {
          setFolders(saved);
          return;
        }
      } catch {}
      // 兼容旧版 localStorage
      try {
        const raw = localStorage.getItem("wp_folders");
        if (raw) {
          const old = JSON.parse(raw);
          if (Array.isArray(old) && old.length > 0) {
            setFolders(old);
            await store.set(WALLPAPER_FOLDERS_KEY, old);
            await store.save();
          }
        }
      } catch {}
    })();
  }, []);

  // folders 变化时持久化 + 重新扫描壁纸
  useEffect(() => {
    // 先保存到 localStorage（即时生效）
    localStorage.setItem("wp_folders", JSON.stringify(folders));
    // 再保存到 store（持久化到 settings.json）
    (async () => {
      try {
        await store.set(WALLPAPER_FOLDERS_KEY, folders);
        await store.save();
      } catch {}
    })();
    if (folders.length === 0) {
      setWallpapers([]);
      return;
    }
    (async () => {
      setLoading(true);
      try {
        let merged: string[] = [];
        for (const f of folders) {
          const files = await invoke<string[]>("scan_wallpaper_folder", { dir: f.path, recursive: f.recursive });
          merged = [...merged, ...files];
        }
        setWallpapers(dedupe(merged));
      } catch {
        // 扫描失败不阻塞
      } finally {
        setLoading(false);
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [folders]);

  // 进入页面时读取系统当前壁纸（可反映其他应用/Windows 设置中的修改），补入历史
  useEffect(() => {
    (async () => {
      try {
        const current = await invoke<string | null>("get_current_wallpaper");
        if (current) {
          setHistory((prev) => {
            const next = [{ path: current, time: Date.now() }, ...prev.filter((h) => h.path !== current)].slice(0, 12);
            store.set(WALLPAPER_HISTORY_KEY, next).then(() => store.save());
            return next;
          });
        }
      } catch {
        // 读取失败不阻塞页面
      }
    })();
  }, []);

  // 读取当前动态壁纸（后端是唯一真源，重启后由 App 恢复）
  useEffect(() => {
    getDynamicWallpaper()
      .then(setDynamic)
      .catch(() => {
        // 查询失败不影响页面其它功能
      });
  }, []);

  // 动态壁纸选择状态需要持久化，供下次启动恢复
  const persistDynamic = useCallback((info: DynamicWallpaperInfo | null) => {
    store.set(DYNAMIC_WALLPAPER_STORE_KEY, info).then(() => store.save());
  }, []);

  // 设置壁纸并记录历史（GIF / HTML 走动态壁纸窗口，其他格式走系统 API）
  const applyWallpaper = useCallback(
    async (path: string, index: number, showToast = true) => {
      setSettingIndex(index);
      try {
        const kind = dynamicWallpaperKind(path);
        if (kind) {
          const info = await setDynamicWallpaper(path);
          setDynamic(info);
          persistDynamic(info);
        } else {
          await stopDynamicWallpaper();
          setDynamic(null);
          persistDynamic(null);
          await invoke("set_wallpaper", { path });
        }
        setHistory((prev) => {
          const next = [{ path, time: Date.now() }, ...prev.filter((h) => h.path !== path)].slice(0, 12);
          store.set(WALLPAPER_HISTORY_KEY, next).then(() => store.save());
          return next;
        });
        if (showToast) {
          toast({ title: t("wallpaper.applied"), status: "success", duration: 2000 });
        }
      } catch (e) {
        toast({ title: String(e), status: "error", duration: 3000 });
      } finally {
        setSettingIndex(null);
      }
    },
    [t, toast, persistDynamic],
  );

  // 通过文件选择器直接挑一个 GIF / HTML 作为动态壁纸
  const handlePickDynamic = useCallback(
    async (kind: DynamicWallpaperKind) => {
      try {
        const { open } = await import("@tauri-apps/plugin-dialog");
        const selected = await open({
          directory: false,
          multiple: false,
          title: t(kind === "gif" ? "wallpaper.dynamicGif" : kind === "video" ? "wallpaper.dynamicVideo" : "wallpaper.dynamicHtml"),
          filters:
            kind === "gif"
              ? [{ name: "GIF", extensions: ["gif"] }]
              : kind === "video"
              ? [{ name: "视频", extensions: ["mp4", "webm", "mov", "mkv", "avi", "m4v"] }]
              : [{ name: "HTML", extensions: ["html", "htm"] }],
        });
        if (typeof selected !== "string") return;
        setDynamicBusy(true);
        const info = await setDynamicWallpaper(selected);
        setDynamic(info);
        persistDynamic(info);
        // 记录到历史
        setHistory((prev) => {
          const next = [{ path: selected, time: Date.now() }, ...prev.filter((h) => h.path !== selected)].slice(0, 12);
          store.set(WALLPAPER_HISTORY_KEY, next).then(() => store.save());
          return next;
        });
        toast({ title: t("wallpaper.dynamicApplied"), status: "success", duration: 2500 });
      } catch (e) {
        toast({ title: String(e), status: "error", duration: 3000 });
      } finally {
        setDynamicBusy(false);
      }
    },
    [t, toast, persistDynamic],
  );

  // 停止动态壁纸（恢复系统壁纸）
  const handleStopDynamic = useCallback(async () => {
    try {
      await stopDynamicWallpaper();
      setDynamic(null);
      persistDynamic(null);
      toast({ title: t("wallpaper.dynamicStopped"), status: "info", duration: 2000 });
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 3000 });
    }
  }, [t, toast, persistDynamic]);

  // 轮换定时器：按 playlist 中每张壁纸的时长切换
  useEffect(() => {
    if (!rotateEnabled) return;
    if (playlist.length === 0) return;

    let timer: ReturnType<typeof setTimeout> | null = null;
    const item = playlist[playlistIndex % playlist.length];
    if (item) {
      void applyWallpaper(item.path, -1, false);
      timer = setTimeout(() => setPlaylistIndex((i) => (i + 1) % playlist.length), (item.durationMinutes || 10) * 60 * 1000);
    }
    return () => {
      if (timer) clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rotateEnabled, playlist, playlistIndex]);

  // 保存轮换设置
  const persistRotate = useCallback((enabled: boolean, interval: number) => {
    store.set(WALLPAPER_ROTATE_KEY, enabled).then(() => store.save());
    store.set(WALLPAPER_INTERVAL_KEY, interval).then(() => store.save());
  }, []);

  const handleRotateToggle = useCallback(
    (e: { target: { checked: boolean } }) => {
      const enabled = e.target.checked;
      setRotateEnabled(enabled);
      persistRotate(enabled, rotateInterval);
      setPlaylistIndex(0);
      if (!enabled) rotateIndexRef.current = 0;
    },
    [persistRotate, rotateInterval],
  );

  const handleIntervalChange = useCallback(
    (_: string, value: number) => {
      const v = Math.max(1, Math.min(1440, Math.round(value) || 1));
      setRotateInterval(v);
      persistRotate(rotateEnabled, v);
    },
    [persistRotate, rotateEnabled],
  );

  // 导入壁纸文件夹（可多次导入，合并结果）
  const importFolderPath = useCallback(async (selected: string, recursive: boolean) => {
    if (folders.some((f) => f.path === selected)) {
      toast({ title: t("wallpaper.folderExists"), status: "info", duration: 2000 });
      return;
    }
    setLoading(true);
    try {
      const files = await invoke<string[]>("scan_wallpaper_folder", { dir: selected, recursive });
      setFolders((prev) => [...prev, { path: selected, recursive }]);
      setWallpapers((prev) => dedupe([...prev, ...files]));
      rotateIndexRef.current = 0;
      if (files.length === 0) {
        toast({ title: t("wallpaper.noImages"), status: "warning", duration: 2500 });
      } else {
        toast({ title: t("wallpaper.loadedCount", { count: files.length }), status: "success", duration: 2500 });
      }
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 3000 });
    } finally {
      setLoading(false);
    }
  }, [t, toast, folders]);

  const handleImportFolder = useCallback(async (recursive: boolean) => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        directory: true,
        multiple: false,
        title: recursive ? t("wallpaper.selectFolderRecursive") : t("wallpaper.selectFolderTitle"),
      });
      if (typeof selected !== "string") return;
      await importFolderPath(selected, recursive);
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 3000 });
    }
  }, [t, toast, importFolderPath]);

  // 移除已导入文件夹并重扫剩余文件夹
  const handleRemoveFolder = useCallback(
    async (folder: WallpaperFolder) => {
      const rest = folders.filter((f) => f.path !== folder.path);
      setFolders(rest);
      setLoading(true);
      try {
        let merged: string[] = [];
        for (const f of rest) {
          const files = await invoke<string[]>("scan_wallpaper_folder", { dir: f.path, recursive: f.recursive });
          merged = dedupe([...merged, ...files]);
        }
        setWallpapers(merged);
        rotateIndexRef.current = 0;
      } catch (e) {
        toast({ title: String(e), status: "error", duration: 3000 });
      } finally {
        setLoading(false);
      }
    },
    [folders, toast],
  );

  // 右键：添加到循环播放列表
  const addToPlaylist = useCallback(
    (path: string) => {
      setPlaylist((prev) => {
        if (prev.some((p) => p.path === path)) return prev;
        const next = [...prev, { path, durationMinutes: rotateInterval }];
        store.set(WALLPAPER_PLAYLIST_KEY, next).then(() => store.save());
        return next;
      });
      setCtxMenu(null);
      toast({ title: t("wallpaper.addedToPlaylist"), status: "success", duration: 2000 });
    },
    [rotateInterval, t, toast],
  );

  // 右键：从列表移除（不删除磁盘文件）
  const removeWallpaper = useCallback(
    (path: string) => {
      setWallpapers((prev) => prev.filter((p) => p !== path));
      setPlaylist((prev) => {
        const next = prev.filter((p) => p.path !== path);
        store.set(WALLPAPER_PLAYLIST_KEY, next).then(() => store.save());
        return next;
      });
      setPlaylistIndex(0);
      setCtxMenu(null);
      toast({ title: t("wallpaper.removedFromList"), status: "info", duration: 2000 });
    },
    [t, toast],
  );

  // 播放列表项时长修改
  const updatePlaylistDuration = useCallback((path: string, value: number) => {
    const v = Math.max(1, Math.min(1440, Math.round(value) || 1));
    setPlaylist((prev) => {
      const next = prev.map((p) => (p.path === path ? { ...p, durationMinutes: v } : p));
      store.set(WALLPAPER_PLAYLIST_KEY, next).then(() => store.save());
      return next;
    });
  }, []);

  // 播放列表项移除
  const removeFromPlaylist = useCallback((path: string) => {
    setPlaylist((prev) => {
      const next = prev.filter((p) => p.path !== path);
      store.set(WALLPAPER_PLAYLIST_KEY, next).then(() => store.save());
      return next;
    });
    setPlaylistIndex(0);
  }, []);

  // 编辑器保存回调
  const handleEditorSaved = useCallback((newPath?: string) => {
    if (newPath) {
      setWallpapers((prev) => (prev.includes(newPath) ? prev : [...prev, newPath]));
    }
  }, []);

  return (
    <Box minH="100vh" p={6} onClick={() => setCtxMenu(null)} onContextMenu={() => setCtxMenu(null)}>
      <HStack mb={5} spacing={3}>
        <IconButton
          aria-label="back"
          icon={<ArrowLeft />}
          size="sm"
          variant="ghost"
          onClick={() => navigate("/")}
        />
        <Heading as="h1" size="lg" color={headingColor}>
          {t("wallpaper.title")}
        </Heading>
      </HStack>

      <VStack spacing={5} align="stretch">
        {/* 操作区：导入文件夹 + 已导入文件夹列表 + 轮换开关 */}
        <LiquidGlassCard px={4} py={4}>
          <VStack align="stretch" spacing={3}>
            <HStack flexWrap="wrap" spacing={3}>
              <Button
                leftIcon={<FolderOpen />}
                colorScheme="blue"
                size="sm"
                onClick={() => handleImportFolder(false)}
                isLoading={loading}
              >
                {t("wallpaper.importFolder")}
              </Button>
              <Button
                leftIcon={<FolderPlus />}
                colorScheme="blue"
                variant="outline"
                size="sm"
                onClick={() => handleImportFolder(true)}
                isLoading={loading}
              >
                {t("wallpaper.importFolderRecursive")}
              </Button>
              <HStack ml="auto" spacing={2}>
                <Text fontSize="sm" color={subTextColor}>
                  {t("wallpaper.rotate")}
                </Text>
                <Switch size="sm" isChecked={rotateEnabled} onChange={handleRotateToggle} />
                <Text fontSize="xs" color={subTextColor}>
                  {t("wallpaper.playlist")}
                </Text>
              </HStack>
            </HStack>
            {folders.length > 0 && (
              <HStack flexWrap="wrap" spacing={2}>
                {folders.map((f) => (
                  <Tag key={f.path} size="md" colorScheme="blue" variant="subtle">
                    <TagLabel maxW="240px" isTruncated title={f.path}>
                      {f.path}
                    </TagLabel>
                    <Badge fontSize="2xs" colorScheme={f.recursive ? "purple" : "gray"} ml={1}>
                      {f.recursive ? "含子目录" : "仅当前"}
                    </Badge>
                    <TagCloseButton onClick={() => handleRemoveFolder(f)} />
                  </Tag>
                ))}
              </HStack>
            )}
          </VStack>
        </LiquidGlassCard>

        {/* 动态壁纸（BETA）：GIF 逐帧播放 / HTML 以完整网页渲染 */}
        <LiquidGlassCard px={4} py={4}>
          <VStack align="stretch" spacing={3}>
            <HStack spacing={2} flexWrap="wrap">
              <MonitorPlay size={18} color={themeColorHex} />
              <Text fontSize="md" fontWeight="semibold" color={headingColor}>
                {t("wallpaper.dynamicTitle")}
              </Text>
              <BetaTag />
              {dynamic && (
                <Badge colorScheme="green" variant="subtle" fontSize="xs">
                  {t("wallpaper.dynamicActive")}
                </Badge>
              )}
            </HStack>
            <HStack spacing={2} flexWrap="wrap">
              <Button
                size="sm"
                leftIcon={<Film />}
                rightIcon={<BetaTag />}
                colorScheme="purple"
                variant="outline"
                isLoading={dynamicBusy}
                onClick={() => handlePickDynamic("gif")}
              >
                {t("wallpaper.dynamicGif")}
              </Button>
              <Button
                size="sm"
                leftIcon={<FileCode2 />}
                rightIcon={<BetaTag />}
                colorScheme="purple"
                variant="outline"
                isLoading={dynamicBusy}
                onClick={() => handlePickDynamic("html")}
              >
                {t("wallpaper.dynamicHtml")}
              </Button>
              <Button
                size="sm"
                leftIcon={<MonitorPlay />}
                rightIcon={<BetaTag />}
                colorScheme="purple"
                variant="outline"
                isLoading={dynamicBusy}
                onClick={() => handlePickDynamic("video")}
              >
                {t("wallpaper.dynamicVideo")}
              </Button>
              {dynamic && (
                <Button size="sm" leftIcon={<Square />} colorScheme="red" variant="ghost" onClick={handleStopDynamic}>
                  {t("wallpaper.dynamicStop")}
                </Button>
              )}
            </HStack>
            {/* HTML/视频壁纸交互设置 */}
            <HStack spacing={4} mt={2} fontSize="xs" flexWrap="wrap">
              <HStack spacing={1}>
                <Text color={subTextColor}>鼠标</Text>
                <Switch size="sm" isChecked={htmlMouse} onChange={(e) => { setHtmlMouse(e.target.checked); localStorage.setItem("wp_html_mouse", e.target.checked ? "1" : "0"); }} />
              </HStack>
              <HStack spacing={1}>
                <Text color={subTextColor}>键盘</Text>
                <Switch size="sm" isChecked={htmlKeyboard} onChange={(e) => { setHtmlKeyboard(e.target.checked); localStorage.setItem("wp_html_keyboard", e.target.checked ? "1" : "0"); }} />
              </HStack>
              <HStack spacing={1}>
                <Text color={subTextColor}>音频</Text>
                <Switch size="sm" isChecked={htmlAudio} onChange={(e) => { setHtmlAudio(e.target.checked); localStorage.setItem("wp_html_audio", e.target.checked ? "1" : "0"); }} />
              </HStack>
            </HStack>
            <Text fontSize="xs" color={subTextColor} isTruncated title={dynamic?.path}>
              {dynamic ? `${dynamic.kind.toUpperCase()} · ${dynamic.path}` : t("wallpaper.dynamicHint")}
            </Text>
          </VStack>
        </LiquidGlassCard>

        {/* 空列表提示 */}
        {rotateEnabled && playlist.length === 0 && (
          <LiquidGlassCard px={4} py={3}>
            <Text fontSize="sm" color="orange.300" textAlign="center">
              轮换已开启，请右键点击壁纸选择"加入轮换列表"
            </Text>
          </LiquidGlassCard>
        )}

        {/* 循环播放列表 */}
        {playlist.length > 0 && (
          <LiquidGlassCard px={4} py={4}>
            <HStack mb={3}>
              <ListMusic size={18} color={themeColorHex} />
              <Text fontSize="md" fontWeight="semibold" color={headingColor}>
                {t("wallpaper.playlist")}
              </Text>
            </HStack>
            <VStack spacing={2} align="stretch">
              {playlist.map((item) => (
                <HStack key={item.path} spacing={3}>
                  {dynamicWallpaperKind(item.path) === "html" ? (
                    <Box
                      boxSize="48px"
                      bg="purple.900"
                      borderRadius="md"
                      display="flex"
                      alignItems="center"
                      justifyContent="center"
                      color="purple.200"
                      flexShrink={0}
                    >
                      <FileCode2 size={20} />
                    </Box>
                  ) : (
                    <Image
                      src={convertFileSrc(item.path)}
                      alt=""
                      boxSize="48px"
                      objectFit="cover"
                      borderRadius="md"
                      fallback={<Box boxSize="48px" bg="gray.600" borderRadius="md" />}
                    />
                  )}
                  <Text fontSize="xs" color={subTextColor} maxW="260px" isTruncated flex="1">
                    {item.path}
                  </Text>
                  <NumberInput
                    size="xs"
                    width="90px"
                    min={1}
                    max={1440}
                    value={item.durationMinutes}
                    onChange={(_, v) => updatePlaylistDuration(item.path, v)}
                  >
                    <NumberInputField />
                    <NumberInputStepper>
                      <NumberIncrementStepper />
                      <NumberDecrementStepper />
                    </NumberInputStepper>
                  </NumberInput>
                  <Text fontSize="xs" color={subTextColor}>
                    {t("wallpaper.intervalUnit")}
                  </Text>
                  <IconButton
                    size="xs"
                    icon={<Trash2 />}
                    aria-label="remove"
                    variant="ghost"
                    onClick={() => removeFromPlaylist(item.path)}
                  />
                </HStack>
              ))}
            </VStack>
          </LiquidGlassCard>
        )}

        {/* 壁纸网格 */}
        {wallpapers.length > 0 && (
          <LiquidGlassCard px={4} py={4}>
            <HStack mb={3}>
              <Images size={18} color={themeColorHex} />
              <Text fontSize="md" fontWeight="semibold" color={headingColor}>
                {t("wallpaper.list")}
              </Text>
              <Text fontSize="xs" color={subTextColor}>
                ({wallpapers.length})
              </Text>
            </HStack>
            <SimpleGrid columns={{ base: 3, md: 4, lg: 6 }} spacing={3}>
              {wallpapers.map((path, index) => (
                <Box
                  key={path}
                  position="relative"
                  borderRadius="lg"
                  overflow="hidden"
                  border="2px solid"
                  borderColor={settingIndex === index ? themeColorHex : "transparent"}
                  cursor="pointer"
                  onClick={() => applyWallpaper(path, index)}
                  onContextMenu={(e) => {
                    e.stopPropagation();
                    e.preventDefault();
                    setCtxMenu({ x: e.clientX, y: e.clientY, path });
                  }}
                  _hover={{ borderColor: themeColorHex }}
                >
                  <WallpaperThumb path={path} />
                  {dynamicWallpaperKind(path) === "gif" && (
                    <Box position="absolute" top={1} right={1}>
                      <BetaTag />
                    </Box>
                  )}
                  {settingIndex === index && (
                    <Box
                      position="absolute"
                      inset={0}
                      display="flex"
                      alignItems="center"
                      justifyContent="center"
                      bg="blackAlpha.500"
                    >
                      <Spinner size="sm" color="white" />
                    </Box>
                  )}
                </Box>
              ))}
            </SimpleGrid>
            <Button
              mt={4}
              size="sm"
              leftIcon={<RefreshCw />}
              variant="outline"
              onClick={() => {
                rotateIndexRef.current = 0;
                setWallpapers([...wallpapers]);
              }}
            >
              {t("wallpaper.refresh")}
            </Button>
          </LiquidGlassCard>
        )}

        {/* 历史记录 */}
        {history.length > 0 && (
          <LiquidGlassCard px={4} py={4}>
            <HStack mb={3}>
              <Play size={16} color={themeColorHex} />
              <Text fontSize="md" fontWeight="semibold" color={headingColor}>
                {t("wallpaper.history")}
              </Text>
            </HStack>
            <SimpleGrid columns={{ base: 4, md: 6, lg: 8 }} spacing={3}>
              {history.map((h) => (
                <Box
                  key={h.path + h.time}
                  position="relative"
                  borderRadius="md"
                  overflow="hidden"
                  border="2px solid"
                  borderColor="transparent"
                  cursor="pointer"
                  onClick={() => applyWallpaper(h.path, history.indexOf(h))}
                  onContextMenu={(e) => {
                    e.stopPropagation();
                    e.preventDefault();
                    setCtxMenu({ x: e.clientX, y: e.clientY, path: h.path });
                  }}
                  _hover={{ borderColor: themeColorHex }}
                >
                  <WallpaperThumb path={h.path} />
                </Box>
              ))}
            </SimpleGrid>
          </LiquidGlassCard>
        )}

        {wallpapers.length === 0 && (
          <LiquidGlassCard px={6} py={8} textAlign="center">
            <Text color={subTextColor} fontSize="sm">
              {t("wallpaper.emptyHint")}
            </Text>
          </LiquidGlassCard>
        )}
      </VStack>

      {/* 右键菜单 */}
      {ctxMenu && (
        <VStack
          position="fixed"
          top={ctxMenu.y}
          left={ctxMenu.x}
          zIndex={1401}
          bg={liquidGlassEnabled ? useColorModeValue("rgba(255,255,255,0.25)", "rgba(0,0,0,0.25)") : menuBg}
          sx={liquidGlassEnabled ? { backdropFilter: `blur(${liquidGlassBlur}px) saturate(1.3)`, WebkitBackdropFilter: `blur(${liquidGlassBlur}px) saturate(1.3)` } : undefined}
          borderRadius="md"
          boxShadow="lg"
          p={1}
          spacing={0}
          align="stretch"
          minW="200px"
          border="1px solid"
          borderColor={liquidGlassEnabled ? useColorModeValue("rgba(255,255,255,0.2)", "rgba(255,255,255,0.1)") : menuBorder}
          onContextMenu={(e) => {
            e.preventDefault();
            e.stopPropagation();
          }}
        >
          <Button
            size="sm"
            variant="ghost"
            justifyContent="flex-start"
            leftIcon={<ListMusic size={14} />}
            onClick={() => addToPlaylist(ctxMenu.path)}
          >
            {t("wallpaper.menuAddToPlaylist")}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            justifyContent="flex-start"
            leftIcon={<Pencil size={14} />}
            onClick={() => {
              setEditingPath(ctxMenu.path);
              setCtxMenu(null);
            }}
          >
            {t("wallpaper.menuEdit")}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            justifyContent="flex-start"
            color="red.300"
            leftIcon={<Trash2 size={14} />}
            onClick={() => removeWallpaper(ctxMenu.path)}
          >
            {t("wallpaper.menuRemove")}
          </Button>
        </VStack>
      )}

      {/* 图片编辑器 */}
      {editingPath && (
        <WallpaperEditor
          path={editingPath}
          onClose={() => setEditingPath(null)}
          onSaved={handleEditorSaved}
        />
      )}
    </Box>
  );
}
