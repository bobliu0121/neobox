import { useState, useCallback, useEffect, useRef } from "react";
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
  RefreshCw,
  Play,
  ListMusic,
  Trash2,
  Pencil,
} from "lucide-react";
import { LiquidGlassCard } from "@/components/special/liquid-glass-card";
import { useThemeColor } from "@/contexts/theme-color-context";
import { useAdaptiveTextColor } from "@/hooks/use-adaptive-text-color";
import { hexToRgba } from "@/lib/color-utils";
import { store } from "@/lib/store";
import WallpaperEditor from "./WallpaperEditor";

const WALLPAPER_HISTORY_KEY = "wallpaper_history";
const WALLPAPER_ROTATE_KEY = "wallpaper_rotate_enabled";
const WALLPAPER_INTERVAL_KEY = "wallpaper_rotate_interval_minutes";
const WALLPAPER_PLAYLIST_KEY = "wallpaper_playlist";

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

export default function WallpaperPage() {
  const navigate = useNavigate();
  const { t } = useTranslation();
  const toast = useToast();
  const { getActiveColor, config: themeConfig } = useThemeColor();
  const themeColorHex = themeConfig.primaryColor;
  const headingColor = useColorModeValue("gray.800", "#ffffff");
  const subTextColor = useColorModeValue("gray.500", "#ffffff");
  const menuBg = useColorModeValue("white", "gray.800");
  const menuBorder = useColorModeValue("gray.200", "gray.600");

  const [folders, setFolders] = useState<string[]>([]);
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
  }, []);

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

  // 设置壁纸并记录历史（GIF 使用动态壁纸窗口，其他格式走系统 API）
  const applyWallpaper = useCallback(
    async (path: string, index: number, showToast = true) => {
      setSettingIndex(index);
      try {
        const ext = path.split(".").pop()?.toLowerCase();
        if (ext === "gif") {
          await invoke("set_gif_wallpaper", { path });
        } else {
          await invoke("stop_gif_wallpaper");
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
    [t, toast],
  );

  // 轮换定时器：优先使用循环播放列表（每项独立时长），否则按统一间隔轮换全部壁纸
  useEffect(() => {
    if (!rotateEnabled) return;
    let timer: ReturnType<typeof setTimeout> | null = null;
    if (playlist.length > 0) {
      const item = playlist[playlistIndex % playlist.length];
      if (item) void applyWallpaper(item.path, -1, false);
      timer = setTimeout(() => setTick((x) => x + 1), (item?.durationMinutes || 10) * 60 * 1000);
    } else if (wallpapers.length > 1) {
      rotateIndexRef.current = (rotateIndexRef.current + 1) % wallpapers.length;
      const next = wallpapers[rotateIndexRef.current];
      if (next) void applyWallpaper(next, rotateIndexRef.current, false);
      timer = setTimeout(() => setTick((x) => x + 1), rotateInterval * 60 * 1000);
    }
    return () => {
      if (timer) clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rotateEnabled, playlist, playlistIndex, tick, wallpapers, rotateInterval]);

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
  const handleImportFolder = useCallback(async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        directory: true,
        multiple: false,
        title: t("wallpaper.selectFolderTitle"),
      });
      if (typeof selected !== "string") return;
      if (folders.includes(selected)) {
        toast({ title: t("wallpaper.folderExists"), status: "info", duration: 2000 });
        return;
      }
      setLoading(true);
      const files = await invoke<string[]>("scan_wallpaper_folder", { dir: selected });
      setFolders((prev) => [...prev, selected]);
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

  // 移除已导入文件夹并重扫剩余文件夹
  const handleRemoveFolder = useCallback(
    async (folder: string) => {
      const rest = folders.filter((f) => f !== folder);
      setFolders(rest);
      setLoading(true);
      try {
        let merged: string[] = [];
        for (const f of rest) {
          const files = await invoke<string[]>("scan_wallpaper_folder", { dir: f });
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
                onClick={handleImportFolder}
                isLoading={loading}
              >
                {t("wallpaper.importFolder")}
              </Button>
              <HStack ml="auto" spacing={2}>
                <Text fontSize="sm" color={subTextColor}>
                  {t("wallpaper.rotate")}
                </Text>
                <Switch size="sm" isChecked={rotateEnabled} onChange={handleRotateToggle} />
                <NumberInput
                  size="sm"
                  width="110px"
                  min={1}
                  max={1440}
                  value={rotateInterval}
                  onChange={handleIntervalChange}
                  isDisabled={!rotateEnabled}
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
              </HStack>
            </HStack>
            {folders.length > 0 && (
              <HStack flexWrap="wrap" spacing={2}>
                {folders.map((f) => (
                  <Tag key={f} size="md" colorScheme="blue" variant="subtle">
                    <TagLabel maxW="240px" isTruncated>
                      {f}
                    </TagLabel>
                    <TagCloseButton onClick={() => handleRemoveFolder(f)} />
                  </Tag>
                ))}
              </HStack>
            )}
          </VStack>
        </LiquidGlassCard>

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
                  <Image
                    src={convertFileSrc(item.path)}
                    alt=""
                    boxSize="48px"
                    objectFit="cover"
                    borderRadius="md"
                    fallback={<Box boxSize="48px" bg="gray.600" borderRadius="md" />}
                  />
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
                  <Image
                    src={convertFileSrc(path)}
                    alt=""
                    w="100%"
                    aspectRatio={16 / 9}
                    objectFit="cover"
                    fallback={<Box w="100%" aspectRatio={16 / 9} bg="gray.600" />}
                  />
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
                  <Image
                    src={convertFileSrc(h.path)}
                    alt=""
                    w="100%"
                    aspectRatio={16 / 9}
                    objectFit="cover"
                    fallback={<Box w="100%" aspectRatio={16 / 9} bg="gray.600" />}
                  />
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
          bg={menuBg}
          borderRadius="md"
          boxShadow="lg"
          p={1}
          spacing={0}
          align="stretch"
          minW="200px"
          border="1px solid"
          borderColor={menuBorder}
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
