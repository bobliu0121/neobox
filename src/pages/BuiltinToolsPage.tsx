import { useState, useCallback } from "react";
import {
  Box,
  Heading,
  VStack,
  Flex,
  useColorModeValue,
} from "@chakra-ui/react";
import {
  Layout,
  Monitor,
  HardDrive,
  Volume2,
  MousePointerClick,
  MousePointer2,
  Gauge,
  Braces,
  ShieldCheck,
  FlaskConical,
  DownloadCloud,
  Video,
  Boxes,
  Clock,
  ScanLine,
  ScrollText,
  Image,
  FileAudio,
  Clapperboard,
  Layers,
  Rewind,
} from "lucide-react";
import { useTranslation } from "react-i18next";

import { ViewGrid } from "@/components/special/view-grid";
import { ViewList } from "@/components/special/view-list";
import { LayoutToggle, type LayoutMode } from "@/components/special/layout-toggle";
import { LiquidGlassCard } from "@/components/special/liquid-glass-card";
import { store } from "@/lib/store";
import type { ViewItem } from "@/components/special/view-types";
import { useAdaptiveTextColor } from "@/hooks/use-adaptive-text-color";

const STORE_KEY = "neobox_builtin_tools_order_v2";
const LS_KEY = "neobox_builtin_tools_order_v2";

/**
 * 内置工具默认顺序：按使用场景分组——
 * 视觉/显示 → 音频 → 视频 → 效率 → 网络 → 系统维护。
 * 用户仍可在页面上拖动自定义。
 */
const defaultTools: ViewItem[] = [
  { id: "overlay-panel", path: "/overlay-panel", icon: Layout, titleKey: "sidebar.overlayPanel", descriptionKey: "builtinTools.overlayPanelDesc", color: "#9B59B6" },
  { id: "resolution-converter", path: "/resolution-converter", icon: Monitor, titleKey: "sidebar.resolutionConverter", descriptionKey: "builtinTools.resolutionConverterDesc", color: "#4A90E2" },
  { id: "screen-test", path: "/screen-test", icon: ScanLine, titleKey: "sidebar.screenTest", descriptionKey: "builtinTools.screenTestDesc", color: "#38B2AC" },
  { id: "image-converter", path: "/image-converter", icon: Image, titleKey: "sidebar.imageConverter", descriptionKey: "builtinTools.imageConverterDesc", color: "#8B5CF6" },
  { id: "video-converter", path: "/video-converter", icon: Clapperboard, titleKey: "sidebar.videoConverter", descriptionKey: "builtinTools.videoConverterDesc", color: "#EC4899" },
  { id: "audio-eq", path: "/audio-eq", icon: Volume2, titleKey: "sidebar.audioEq", descriptionKey: "builtinTools.audioEqDesc", color: "#E74C3C" },
  { id: "audio-converter", path: "/audio-converter", icon: FileAudio, titleKey: "sidebar.audioConverter", descriptionKey: "builtinTools.audioConverterDesc", color: "#F59E0B" },
  { id: "audio-reverse", path: "/audio-reverse", icon: Rewind, titleKey: "audioReverse.title", descriptionKey: "audioReverse.desc", color: "#EF4444" },
  { id: "audio-speed", path: "/audio-speed", icon: Gauge, titleKey: "audioSpeed.title", descriptionKey: "audioSpeed.desc", color: "#8B5CF6" },
  { id: "mka-muxer", path: "/mka-muxer", icon: Layers, titleKey: "sidebar.mkaMuxer", descriptionKey: "builtinTools.mkaMuxerDesc", color: "#14B8A6", beta: true },
  { id: "nvidia-recording", path: "/nvidia-recording", icon: Video, titleKey: "sidebar.nvidiaRecording", descriptionKey: "builtinTools.nvidiaRecordingDesc", color: "#76B900" },
  { id: "autoclicker", path: "/autoclicker", icon: MousePointerClick, titleKey: "sidebar.autoclicker", descriptionKey: "builtinTools.autoclickerDesc", color: "#FF8C00" },
  { id: "context-menu", path: "/context-menu", icon: MousePointer2, titleKey: "sidebar.contextMenu", descriptionKey: "builtinTools.contextMenuDesc", color: "#3B82F6" },
  { id: "hidden-features", path: "/hidden-features", icon: FlaskConical, titleKey: "sidebar.hiddenFeatures", descriptionKey: "builtinTools.hiddenFeaturesDesc", color: "#E67E22", beta: true },
  { id: "download-accelerator", path: "/download-accelerator", icon: DownloadCloud, titleKey: "sidebar.downloadAccelerator", descriptionKey: "builtinTools.downloadAcceleratorDesc", color: "#00C896" },
  { id: "bilibili-download", path: "/bilibili-download", icon: DownloadCloud, titleKey: "sidebar.bilibiliDownload", descriptionKey: "builtinTools.bilibiliDownloadDesc", color: "#FB7299" },
  { id: "speedtest", path: "/speedtest", icon: Gauge, titleKey: "sidebar.speedtest", descriptionKey: "builtinTools.speedtestDesc", color: "#00B4D8" },
  { id: "disk-health", path: "/disk-health", icon: HardDrive, titleKey: "sidebar.diskHealth", descriptionKey: "builtinTools.diskHealthDesc", color: "#00B4D8" },
  { id: "runtime-repair", path: "/runtime-repair", icon: Braces, titleKey: "sidebar.runtimeRepair", descriptionKey: "builtinTools.runtimeRepairDesc", color: "#4A90E2" },
  { id: "app-manager", path: "/app-manager", icon: Boxes, titleKey: "sidebar.appManager", descriptionKey: "builtinTools.appManagerDesc", color: "#4A90E2" },
  { id: "time-sync", path: "/time-sync", icon: Clock, titleKey: "sidebar.timeSync", descriptionKey: "builtinTools.timeSyncDesc", color: "#00B4D8" },
  { id: "vtx-virtualization", path: "/vtx-virtualization", icon: ShieldCheck, titleKey: "sidebar.vtxVirtualization", descriptionKey: "builtinTools.vtxVirtualizationDesc", color: "#38A169" },
  { id: "bsod-log", path: "/bsod-log", icon: ScrollText, titleKey: "sidebar.bsodLog", descriptionKey: "builtinTools.bsodLogDesc", color: "#EF4444", beta: true },
];
function loadOrder(): string[] | null {
  try {
    const raw = localStorage.getItem(LS_KEY);
    if (raw) {
      const ids: string[] = JSON.parse(raw);
      if (Array.isArray(ids) && ids.length > 0) return ids;
    }
  } catch { /* ignore */ }
  return null;
}

function saveOrder(ids: string[]) {
  try {
    localStorage.setItem(LS_KEY, JSON.stringify(ids));
    store.set(STORE_KEY, ids).then(() => store.save());
  } catch { /* ignore */ }
}

function applyOrder(allTools: ViewItem[], orderIds: string[]): ViewItem[] {
  const map = new Map(allTools.map((t) => [t.id, t]));
  const ordered: ViewItem[] = [];
  for (const id of orderIds) {
    const tool = map.get(id);
    if (tool) {
      ordered.push(tool);
      map.delete(id);
    }
  }
  // Append any new tools not in the saved order
  for (const tool of map.values()) {
    ordered.push(tool);
  }
  return ordered;
}

export default function BuiltinToolsPage() {
  const { t } = useTranslation();
  const [layoutMode, setLayoutMode] = useState<LayoutMode>("grid");

  const [tools, setTools] = useState<ViewItem[]>(() => {
    const saved = loadOrder();
    if (saved) return applyOrder(defaultTools, saved);
    return defaultTools;
  });

  const handleReorder = useCallback((newTools: ViewItem[]) => {
    setTools(newTools);
    saveOrder(newTools.map((t) => t.id));
  }, []);

  const headingColor = useColorModeValue("gray.900", "#ffffff");
  const adaptiveTitle = useAdaptiveTextColor();

  const content = (
    <VStack align="start" spacing={6}>
      <Flex w="full" justify="space-between" align="center">
        <Heading size="lg" color={adaptiveTitle.text} textShadow={adaptiveTitle.shadow}>
          {t("builtinTools.title")}
        </Heading>
        <LiquidGlassCard display="inline-flex" p={1} boxShadow="sm">
          <LayoutToggle mode={layoutMode} onChange={setLayoutMode} />
        </LiquidGlassCard>
      </Flex>
      {layoutMode === "grid" ? (
        <ViewGrid tools={tools} onReorder={handleReorder} />
      ) : (
        <ViewList tools={tools} onReorder={handleReorder} />
      )}
    </VStack>
  );

  return (
    <Box pt={8}>
      {content}
    </Box>
  );
}
