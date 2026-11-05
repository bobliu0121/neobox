import {
  Box,
  Flex,
  Text,
  Heading,
  HStack,
  VStack,
  Badge,
  Button,
  IconButton,
  Spinner,
  Tooltip,
  useColorModeValue,
  Select,
  Slider,
  SliderTrack,
  SliderFilledTrack,
  SliderThumb,
  Switch,
  Input,
  Alert,
  AlertIcon,
  AlertDescription,
  Divider,
} from "@chakra-ui/react";
import { useDynamicIsland } from "@/components/ui/dynamic-island";
import { useThemeColor } from "@/contexts/theme-color-context";
import { useAdaptiveTextColor } from "@/hooks/use-adaptive-text-color";
import { useTranslation } from "react-i18next";
import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useNavigate } from "react-router-dom";
import {
  ArrowLeft,
  Monitor,
  Camera,
  Mic,
  Volume2,
  Play,
  Square,
  RefreshCw,
  FolderOpen,
  Trash2,
  Film,
  Settings2,
  AlertTriangle,
  CheckCircle2,
  Move,
  Eye,
  EyeOff,
  Crop,
  RotateCcw,
} from "lucide-react";
import CanvasPreview, { type LayerLayout, heightForWidth } from "@/components/recorder/CanvasPreview";

// ── 与后端一致的类型定义 ──

interface CaptureDevice {
  name: string;
  kind: string;
  alt_name: string;
}

/**
 * 图层默认布局。
 *
 * 只有 x/y/w/visible：高度由采集源宽高比自动推导（见 CanvasPreview.heightForWidth），
 * 裁剪功能已整体移除。
 */
const DEFAULT_LAYER: LayerLayout = {
  x: 0, y: 0, w: 0, visible: true,
};

interface RecorderConfig {
  screen_enabled: boolean;
  screen_fps: number;
  camera_device: string;
  camera_width: number;
  camera_height: number;
  canvas_w: number;
  canvas_h: number;
  screen_layout: LayerLayout;
  camera_layout: LayerLayout;
  mic_device: string;
  system_audio_device: string;
  mic_volume: number;
  system_volume: number;
  video_bitrate_kbps: number;
  audio_bitrate_kbps: number;
  encoder: string;
  output_dir: string;
  preview_enabled: boolean;
}

interface RecorderStatus {
  recording: boolean;
  elapsed_sec: number;
  estimated_bytes: number;
  output_path: string;
  encoder: string;
  last_error: string;
}

interface RecorderFile {
  path: string;
  name: string;
  size: number;
  modified_ms: number;
  duration_sec: number;
}

interface FfmpegProbe {
  available: boolean;
  path: string;
  version: string;
  has_gdigrab: boolean;
  has_dshow: boolean;
  hw_encoders: string[];
}

/** 录制画面比例预设 */
const ASPECT_PRESETS: { label: string; w: number; h: number }[] = [
  { label: "16:9", w: 16, h: 9 },
  { label: "9:16", w: 9, h: 16 },
  { label: "1:1", w: 1, h: 1 },
  { label: "4:3", w: 4, h: 3 },
  { label: "4:5", w: 4, h: 5 },
  { label: "21:9", w: 21, h: 9 },
];

/**
 * 按比例算出画布尺寸，保持面积（对角线长度）大致不变，避免切比例后尺寸突变。
 *
 * area 保持不变时：w = sqrt(area * rw/rh)，h = w * rh/rw。
 * 两者都强制偶数（yuv420p 要求）。
 */
function canvasForAspect(ratioW: number, ratioH: number, refW: number, refH: number) {
  const area = Math.max(16, refW) * Math.max(16, refH);
  let w = Math.sqrt((area * ratioW) / ratioH);
  let h = w * (ratioH / ratioW);
  w = Math.max(2, Math.round(w));
  h = Math.max(2, Math.round(h));
  w -= w % 2;
  h -= h % 2;
  return { w, h };
}

/**
 * 画布尺寸变化时迁移图层布局。
 *
 * 规则：
 * 1. `w` 为 0 表示「铺满画布宽度」，**必须保持 0**。早先对 0 也套了
 *    `Math.max(2, ...)`，结果恢复默认（w=0）后再切比例，图层被"放大"成
 *    2×2 像素，录制出来整片黑 —— 这是「恢复默认后黑屏」的根因。
 * 2. 非 0 的宽度与坐标按画布缩放比例等比迁移，否则切比例后布局会错位。
 *
 * 高度不参与迁移：它由采集源宽高比自动推导，与画布尺寸无关。
 */
function migrateLayersForCanvas(layer: LayerLayout, sx: number, sy: number): LayerLayout {
  const scaleWidth = (v: number) => {
    if (v === 0) return 0; // 0 = 铺满画布宽度，保持 0
    const scaled = Math.round(v * sx);
    return Math.max(2, Math.round(scaled / 2) * 2);
  };
  return {
    ...layer,
    x: Math.round(layer.x * sx),
    y: Math.round(layer.y * sy),
    w: scaleWidth(layer.w),
  };
}

const DEFAULT_CONFIG: RecorderConfig = {
  screen_enabled: true,
  screen_fps: 30,
  camera_device: "",
  camera_width: 0,
  camera_height: 0,
  canvas_w: 0,
  canvas_h: 0,
  screen_layout: { ...DEFAULT_LAYER },
  camera_layout: { ...DEFAULT_LAYER },
  mic_device: "",
  system_audio_device: "",
  mic_volume: 100,
  system_volume: 100,
  video_bitrate_kbps: 6000,
  audio_bitrate_kbps: 160,
  encoder: "auto",
  output_dir: "",
  preview_enabled: true,
};

/** 字节数格式化 */
function formatBytes(n: number): string {
  if (n <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

/** 秒数格式化为 时:分:秒 */
function formatDuration(sec: number): string {
  const s = Math.max(0, Math.floor(sec));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = s % 60;
  const pad = (x: number) => String(x).padStart(2, "0");
  return `${pad(h)}:${pad(m)}:${pad(r)}`;
}

/** 时间戳格式化 */
function formatTime(ms: number): string {
  if (!ms) return "";
  const d = new Date(ms);
  const pad = (x: number) => String(x).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export default function RecordingPage() {
  const navigate = useNavigate();
  const { t } = useTranslation();
  const { config: themeConfig, getActiveColor, getHoverColor } = useThemeColor();
  const toast = useDynamicIsland("wrench");

  const activeColor = getActiveColor();
  const textColor = useColorModeValue("gray.800", "#f0f0f0");
  const subTextColor = useColorModeValue("gray.500", "#cccccc");
  const cardBg = useColorModeValue("white", "rgba(30,30,30,0.6)");
  const borderColor = useColorModeValue("gray.200", "rgba(255,255,255,0.12)");
  const trackBg = useColorModeValue("gray.200", "rgba(255,255,255,0.16)");
  const dividerColor = useColorModeValue("gray.100", "rgba(255,255,255,0.08)");
  const adaptiveTitle = useAdaptiveTextColor();

  // ── 状态 ──
  const [config, setConfig] = useState<RecorderConfig>(DEFAULT_CONFIG);
  const [devices, setDevices] = useState<CaptureDevice[]>([]);
  const [probe, setProbe] = useState<FfmpegProbe | null>(null);
  /**
   * 摄像头采集源的原始宽高比（宽/高）。
   *
   * 图层高度必须按源比例推导，否则画面会被拉伸变形。
   * 摄像头未指定分辨率时设备默认值未知，先用 16:9 作为占位；
   * 若用户填了 camera_width/camera_height 则用真实比例。
   */
  const cameraAspect = useMemo(() => {
    const w = config.camera_width;
    const h = config.camera_height;
    if (w > 0 && h > 0) return w / h;
    return 16 / 9;
  }, [config.camera_width, config.camera_height]);
  /** 各源预览的失败原因（屏幕/摄像头分别记录，避免"整体成功但某路失败"被静默） */
  const [screenPreviewError, setScreenPreviewError] = useState("");
  const [cameraPreviewError, setCameraPreviewError] = useState("");
  const [status, setStatus] = useState<RecorderStatus>({
    recording: false,
    elapsed_sec: 0,
    estimated_bytes: 0,
    output_path: "",
    encoder: "",
    last_error: "",
  });
  const [files, setFiles] = useState<RecorderFile[]>([]);
  const [loadingDevices, setLoadingDevices] = useState(false);
  const [loadingFiles, setLoadingFiles] = useState(false);
  const [starting, setStarting] = useState(false);
  const [stopping, setStopping] = useState(false);
  const pollRef = useRef<number | null>(null);
  // 画布预览相关状态
  const [screenInfo, setScreenInfo] = useState({ width: 1920, height: 1080 });
  const [previewRunning, setPreviewRunning] = useState(false);
  const [previewPort, setPreviewPort] = useState(0);
  /** 预览的明确状态：idle 未启动 / starting 启动中 / running 运行中 / error 失败 */
  const [previewState, setPreviewState] = useState<"idle" | "starting" | "running" | "error">("idle");
  /** 预览失败原因（供画布区展示，避免"一片黑却不知道为什么"） */
  const [previewError, setPreviewError] = useState("");
  // 预览流加时间戳，强制 <img> 重新连接（MJPEG 流被读干后无法续读）
  const [previewStamp, setPreviewStamp] = useState(() => Date.now());
  const [activeLayer, setActiveLayer] = useState<"screen" | "camera" | null>(null);
  // 画布宽高的输入框草稿（失焦/回车才提交，避免边输入边重建）
  const [canvasWInput, setCanvasWInput] = useState("");
  const [canvasHInput, setCanvasHInput] = useState("");
  // 最新 config / status 的 ref：供按关键字段触发的 effect 读取，避免闭包陈旧
  const configRef = useRef(config);
  const statusRef = useRef(status);
  useEffect(() => {
    configRef.current = config;
  }, [config]);
  useEffect(() => {
    statusRef.current = status;
  }, [status]);
  // 画布尺寸变化时同步输入框显示值（切换比例预设后要能立刻看到新宽高）
  useEffect(() => {
    const w = config.canvas_w || screenInfo.width;
    const h = config.canvas_h || screenInfo.height;
    setCanvasWInput(String(w));
    setCanvasHInput(String(h));
  }, [config.canvas_w, config.canvas_h, screenInfo.width, screenInfo.height]);

  const cameras = useMemo(() => devices.filter((d) => d.kind === "video"), [devices]);
  const mics = useMemo(() => devices.filter((d) => d.kind === "audio"), [devices]);

  const scrollbarSx = {
    "&::-webkit-scrollbar": { width: "6px" },
    "&::-webkit-scrollbar-track": { background: "transparent" },
    "&::-webkit-scrollbar-thumb": {
      background: activeColor,
      borderRadius: "3px",
      minHeight: "40px",
    },
  } as const;

  /** 拉取 ffmpeg 能力探测 */
  const loadProbe = useCallback(async () => {
    try {
      const result = await invoke<FfmpegProbe>("recorder_probe");
      setProbe(result);
    } catch (e) {
      console.error("recorder_probe failed:", e);
      setProbe(null);
    }
  }, []);

  /** 拉取采集设备 */
  const loadDevices = useCallback(async () => {
    setLoadingDevices(true);
    try {
      const result = await invoke<CaptureDevice[]>("recorder_list_devices");
      setDevices(result ?? []);
    } catch (e) {
      console.error("recorder_list_devices failed:", e);
      setDevices([]);
    } finally {
      setLoadingDevices(false);
    }
  }, []);

  /** 读取已保存设置 */
  const loadConfig = useCallback(async () => {
    try {
      const saved = await invoke<RecorderConfig>("recorder_get_config");
      if (saved) {
        setConfig({ ...DEFAULT_CONFIG, ...saved });
      }
    } catch (e) {
      console.error("recorder_get_config failed:", e);
    }
  }, []);

  /** 拉取录制状态 */
  const loadStatus = useCallback(async () => {
    try {
      const s = await invoke<RecorderStatus>("recorder_status");
      setStatus(s);
    } catch (e) {
      console.error("recorder_status failed:", e);
    }
  }, []);

  /** 拉取录制文件列表 */
  const loadFiles = useCallback(async () => {
    setLoadingFiles(true);
    try {
      const list = await invoke<RecorderFile[]>("recorder_list_files", {
        dir: config.output_dir.trim() || null,
      });
      setFiles(list ?? []);
    } catch (e) {
      console.error("recorder_list_files failed:", e);
      setFiles([]);
    } finally {
      setLoadingFiles(false);
    }
  }, [config.output_dir]);

  // 首次加载
  useEffect(() => {
    void loadProbe();
    void loadDevices();
    void loadConfig();
    void loadStatus();
    // 屏幕分辨率：作为画布默认尺寸
    void invoke<{ width: number; height: number }>("recorder_screen_info")
      .then((info) => {
        if (info && info.width > 0) setScreenInfo(info);
      })
      .catch((e) => console.error("recorder_screen_info failed:", e));
  }, [loadProbe, loadDevices, loadConfig, loadStatus]);

  // 离开页面时停止预览，避免残留采集进程占用设备与 CPU
  useEffect(() => {
    return () => {
      void invoke("recorder_preview_stop").catch(() => {});
    };
  }, []);

  // 配置加载完成后拉文件列表
  useEffect(() => {
    void loadFiles();
  }, [loadFiles]);

  // 录制中轮询状态（1 秒一次）
  useEffect(() => {
    if (status.recording) {
      pollRef.current = window.setInterval(() => {
        void loadStatus();
      }, 1000);
    } else if (pollRef.current !== null) {
      window.clearInterval(pollRef.current);
      pollRef.current = null;
    }
    return () => {
      if (pollRef.current !== null) {
        window.clearInterval(pollRef.current);
        pollRef.current = null;
      }
    };
  }, [status.recording, loadStatus]);

  /**
   * 保存设置（静默，不打扰用户）。
   *
   * 带 300ms 防抖：拖拽/缩放图层时每个 mousemove 都会产生一次配置变化，
   * 若每次都 invoke 落盘，一次拖拽会打出几十次 IPC 与 settings.json 写入，
   * 拖拽会明显卡顿、且磁盘被反复刷。防抖后只在停手 300ms 落盘一次。
   */
  const persistTimerRef = useRef<number | null>(null);
  const persistConfig = useCallback(async (next: RecorderConfig) => {
    if (persistTimerRef.current !== null) {
      window.clearTimeout(persistTimerRef.current);
    }
    persistTimerRef.current = window.setTimeout(() => {
      persistTimerRef.current = null;
      void invoke("recorder_save_config", { config: next }).catch((e) => {
        console.error("recorder_save_config failed:", e);
      });
    }, 300);
  }, []);
  // 离开页面时把待写入的配置立刻落盘，避免防抖窗口内的改动丢失
  useEffect(() => {
    return () => {
      if (persistTimerRef.current !== null) {
        window.clearTimeout(persistTimerRef.current);
        persistTimerRef.current = null;
        void invoke("recorder_save_config", { config: configRef.current }).catch(() => {});
      }
    };
  }, []);

  /** 修改配置并持久化 */
  const updateConfig = useCallback(
    <K extends keyof RecorderConfig>(key: K, value: RecorderConfig[K]) => {
      setConfig((prev) => {
        const next = { ...prev, [key]: value };
        void persistConfig(next);
        return next;
      });
    },
    [persistConfig]
  );

  /** 更新某个图层布局（拖拽/缩放/裁剪后由画布组件回调） */
  const updateLayer = useCallback(
    (key: "screen_layout" | "camera_layout", layout: LayerLayout) => {
      setConfig((prev) => {
        const next = { ...prev, [key]: layout };
        void persistConfig(next);
        return next;
      });
    },
    [persistConfig]
  );

  /**
   * 选择/切换摄像头。
   *
   * 摄像头的图层规则与屏幕**完全一致**（w/h=0 表示铺满画布、可拖拽缩放裁剪、
   * 切比例等比迁移）。但若首次启用时也按「铺满画布」来，摄像头会整片盖住屏幕，
   * 用户第一眼只会看到摄像头画面、以为屏幕没生效。
   * 因此首次选中时自动给一个合理初始尺寸（画布宽 1/4、按 4:3 估高、右下角留边距），
   * 之后用户把它拖成什么样就保持什么样（含清零语义）。
   */
  const handleCameraDeviceChange = useCallback(
    (device: string) => {
      setConfig((prev) => {
        const next: RecorderConfig = { ...prev, camera_device: device };
        // 首次启用（原来没选设备，现在选了）且摄像头布局仍为初始状态时，给初始宽度
        const wasEmpty = prev.camera_device.trim() === "";
        const layerUntouched =
          prev.camera_layout.w === 0 &&
          prev.camera_layout.x === 0 &&
          prev.camera_layout.y === 0;
        if (device.trim() !== "" && wasEmpty && layerUntouched) {
          const cw = prev.canvas_w || screenInfo.width;
          const ch = prev.canvas_h || screenInfo.height;
          const camW = Math.max(160, Math.round(cw / 4 / 2) * 2);
          // 高度按**摄像头源比例**推导（不是随意估 4:3），保证初始就不变形
          const camH = heightForWidth(camW, cameraAspect);
          const margin = Math.round(Math.min(cw, ch) * 0.03);
          next.camera_layout = {
            ...prev.camera_layout,
            w: camW,
            x: Math.max(0, cw - camW - margin),
            y: Math.max(0, ch - camH - margin),
          };
        }
        void persistConfig(next);
        return next;
      });
    },
    [persistConfig, screenInfo.width, screenInfo.height, cameraAspect]
  );

  /**
   * 恢复默认配置。
   *
   * 只重置**布局与采集参数**，保留输出目录、码率、编码器等「用户偏好」：
   * 一次误点就把码率和输出路径也清掉会很恼人。
   * 设备选择（摄像头/麦克风）也保留，因为那通常与硬件环境绑定。
   */
  const handleRestoreDefaults = useCallback(() => {
    setConfig((prev) => {
      // 画布尺寸必须来自可靠的屏幕尺寸：screenInfo 若还没加载好（0 或异常小），
      // 直接用它会把画布设成极小值，录制出来整片黑。这里做下限保护。
      const safeW = screenInfo.width >= 320 ? screenInfo.width : 1920;
      const safeH = screenInfo.height >= 240 ? screenInfo.height : 1080;
      const cw = safeW - (safeW % 2);
      const ch = safeH - (safeH % 2);
      // 摄像头若已选设备，给与「首次启用」相同的默认小窗，避免恢复默认后它盖住屏幕；
      // 未选设备时保持全 0（不参与合成）。
      const hasCamera = prev.camera_device.trim() !== "";
      const camW = Math.max(160, Math.round(cw / 4 / 2) * 2);
      // 高度按摄像头源比例推导（保证不变形）
      const camH = heightForWidth(camW, cameraAspect);
      const margin = Math.round(Math.min(cw, ch) * 0.03);
      const next: RecorderConfig = {
        ...prev,
        // 采集参数
        screen_enabled: DEFAULT_CONFIG.screen_enabled,
        screen_fps: DEFAULT_CONFIG.screen_fps,
        camera_width: DEFAULT_CONFIG.camera_width,
        camera_height: DEFAULT_CONFIG.camera_height,
        mic_volume: DEFAULT_CONFIG.mic_volume,
        system_volume: DEFAULT_CONFIG.system_volume,
        // 画布与布局：canvas 用有效屏幕尺寸
        canvas_w: cw,
        canvas_h: ch,
        // 屏幕图层 w=0 → 铺满画布宽度
        screen_layout: { ...DEFAULT_LAYER },
        // 摄像头图层：有设备时给初始宽度（高度按源比例自动），规则与屏幕一致
        camera_layout: hasCamera
          ? {
              ...DEFAULT_LAYER,
              w: camW,
              x: Math.max(0, cw - camW - margin),
              y: Math.max(0, ch - camH - margin),
            }
          : { ...DEFAULT_LAYER },
      };
      void persistConfig(next);
      return next;
    });
    setActiveLayer(null);
    toast({ title: "已恢复默认布局", status: "info", duration: 2000 });
  }, [persistConfig, screenInfo.width, screenInfo.height, cameraAspect, toast]);

  /**
   * 切换录制画面比例。
   *
   * 只改变**画布**尺寸，图层内容按画布缩放比例等比迁移：
   * - 图层 w/h 为 0（自动铺满）时保持 0，因此屏幕图层会跟着新画布自动铺满
   * - 已手动定位的图层按比例跟随，不会跑到画布外
   */
  const handleAspectChange = useCallback(
    (ratioW: number, ratioH: number) => {
      setConfig((prev) => {
        const refW = prev.canvas_w || screenInfo.width;
        const refH = prev.canvas_h || screenInfo.height;
        const { w, h } = canvasForAspect(ratioW, ratioH, refW, refH);
        const sx = w / Math.max(1, refW);
        const sy = h / Math.max(1, refH);
        const next: RecorderConfig = {
          ...prev,
          canvas_w: w,
          canvas_h: h,
          screen_layout: migrateLayersForCanvas(prev.screen_layout, sx, sy),
          camera_layout: migrateLayersForCanvas(prev.camera_layout, sx, sy),
        };
        void persistConfig(next);
        return next;
      });
    },
    [persistConfig, screenInfo.width, screenInfo.height]
  );

  /** 自定义画布宽高的直接输入 */
  const handleCanvasSizeChange = useCallback(
    (w: number, h: number) => {
      setConfig((prev) => {
        const refW = prev.canvas_w || screenInfo.width;
        const refH = prev.canvas_h || screenInfo.height;
        // 强制偶数（yuv420p 要求），最小 64
        const nw = Math.max(64, w - (w % 2));
        const nh = Math.max(64, h - (h % 2));
        const sx = nw / Math.max(1, refW);
        const sy = nh / Math.max(1, refH);
        const next: RecorderConfig = {
          ...prev,
          canvas_w: nw,
          canvas_h: nh,
          screen_layout: migrateLayersForCanvas(prev.screen_layout, sx, sy),
          camera_layout: migrateLayersForCanvas(prev.camera_layout, sx, sy),
        };
        void persistConfig(next);
        return next;
      });
    },
    [persistConfig, screenInfo.width, screenInfo.height]
  );

  /** 查询各源预览状态并记下失败原因（供界面按源提示） */
  const refreshPreviewSourceErrors = useCallback(async () => {
    try {
      const st = await invoke<{
        running: boolean;
        port: number;
        screenError: string;
        cameraError: string;
      }>("recorder_preview_status");
      setScreenPreviewError(st.screenError ?? "");
      setCameraPreviewError(st.cameraError ?? "");
    } catch (e) {
      console.error("recorder_preview_status failed:", e);
    }
  }, []);

  /** 启动实时预览 */
  const startPreview = useCallback(async () => {
    setPreviewState("starting");
    try {
      const port = await invoke<number>("recorder_preview_start", { config });
      setPreviewPort(port);
      setPreviewRunning(true);
      setPreviewState("running");
      setPreviewError("");
      // 新流需要新的时间戳参数，否则浏览器可能复用已断的旧连接
      setPreviewStamp(Date.now());
      // 立刻查一次按源状态：屏幕成功但摄像头失败时也要把原因显示出来
      void refreshPreviewSourceErrors();
    } catch (e) {
      const msg = String(e ?? "");
      console.error("recorder_preview_start failed:", msg);
      setPreviewRunning(false);
      // 空源属于配置问题，静默置为 idle；其它错误才提示
      if (msg.includes("没有可见的图层") || msg.includes("没有可预览的采集源")) {
        setPreviewState("idle");
        setPreviewError("");
      } else {
        setPreviewState("error");
        setPreviewError(msg.slice(0, 160));
        toast({ title: `预览启动失败：${msg.slice(0, 120)}`, status: "warning", duration: 4000 });
      }
    }
  }, [config, toast, refreshPreviewSourceErrors]);

  /** 停止实时预览 */
  const stopPreview = useCallback(async () => {
    try {
      await invoke("recorder_preview_stop");
    } catch (e) {
      console.error("recorder_preview_stop failed:", e);
    }
    setPreviewRunning(false);
    setPreviewState("idle");
    setPreviewError("");
  }, []);

  /** 切换预览开关（关掉就释放采集设备与 CPU） */
  const handleTogglePreview = useCallback(
    (enabled: boolean) => {
      updateConfig("preview_enabled", enabled);
      if (enabled) {
        void startPreview();
      } else {
        void stopPreview();
      }
    },
    [updateConfig, startPreview, stopPreview]
  );

  /**
   * 只有「采集源」变化时才需要重启预览进程。
   *
   * # 为什么不再监听图层布局
   *
   * 预览现在由前端分层合成（每个源一路流 + CSS 定位），
   * 拖拽/缩放/裁剪只是改 CSS，浏览器下一帧就重绘 —— 真正的实时。
   * 因此图层布局变化**不应该**重启 ffmpeg：那会导致拖动时画面反复黑掉重建。
   *
   * 只有换摄像头、开关屏幕源、改帧率这类「采集参数」变化，才必须重启流进程。
   *
   * 判定用 preview_enabled 而非 previewRunning：后者在预览失败后为 false，
   * 会让 effect 永远提前 return，导致"开着预览却一直显示关闭"。
   */
  useEffect(() => {
    if (!config.preview_enabled) return;
    // 录制中不动预览，避免抢占采集设备影响录制质量
    if (statusRef.current.recording) return;

    const timer = window.setTimeout(() => {
      const latest = configRef.current;
      if (!latest.preview_enabled) return;
      if (statusRef.current.recording) return;
      setPreviewState("starting");
      void invoke<number>("recorder_preview_start", { config: latest })
        .then((port) => {
          setPreviewPort(port);
          setPreviewRunning(true);
          setPreviewState("running");
          // 新请求必须带新的时间戳，否则 <img> 会复用已断开的旧连接
          setPreviewStamp(Date.now());
        })
        .catch((e) => {
          const msg = String(e ?? "");
          console.error("preview start failed:", msg);
          setPreviewRunning(false);
          if (msg.includes("没有可预览的采集源") || msg.includes("请启用屏幕")) {
            setPreviewState("idle");
            setPreviewError("");
          } else {
            setPreviewState("error");
            setPreviewError(msg.slice(0, 160));
          }
        });
    }, 350);

    return () => window.clearTimeout(timer);
  }, [
    config.preview_enabled,
    config.screen_enabled,
    config.screen_fps,
    config.camera_device,
  ]);

  /** 开始录制 */
  const handleStart = useCallback(async () => {
    if (starting || status.recording) return;
    setStarting(true);
    try {
      const path = await invoke<string>("recorder_start", { config });
      toast({ title: `开始录制：${path.split(/[\\/]/).pop()}`, status: "success", duration: 3000 });
      await loadStatus();
    } catch (e) {
      const msg = String(e ?? "");
      toast({ title: `启动录制失败：${msg.slice(0, 160)}`, status: "error", duration: 5000 });
    } finally {
      setStarting(false);
    }
  }, [config, starting, status.recording, toast, loadStatus]);

  /** 停止录制 */
  const handleStop = useCallback(async () => {
    if (stopping || !status.recording) return;
    setStopping(true);
    try {
      const path = await invoke<string>("recorder_stop");
      if (path) {
        toast({ title: "录制已保存", status: "success", duration: 3000 });
      }
      await loadStatus();
      await loadFiles();
    } catch (e) {
      const msg = String(e ?? "");
      toast({ title: `停止失败：${msg.slice(0, 160)}`, status: "error", duration: 5000 });
      await loadStatus();
    } finally {
      setStopping(false);
    }
  }, [stopping, status.recording, toast, loadStatus, loadFiles]);

  /** 删除文件 */
  const handleDelete = useCallback(
    async (file: RecorderFile) => {
      try {
        await invoke("recorder_delete_file", { path: file.path });
        toast({ title: "已删除", status: "info", duration: 2000 });
        await loadFiles();
      } catch (e) {
        toast({ title: `删除失败：${String(e).slice(0, 120)}`, status: "error", duration: 3000 });
      }
    },
    [toast, loadFiles]
  );

  /** 打开文件 */
  const handleOpen = useCallback(
    async (file: RecorderFile) => {
      try {
        await invoke("recorder_open_file", { path: file.path });
      } catch (e) {
        toast({ title: `打开失败：${String(e).slice(0, 120)}`, status: "error", duration: 3000 });
      }
    },
    [toast]
  );

  /** 定位文件 */
  const handleReveal = useCallback(
    async (file: RecorderFile) => {
      try {
        await invoke("recorder_reveal_file", { path: file.path });
      } catch (e) {
        toast({ title: `定位失败：${String(e).slice(0, 120)}`, status: "error", duration: 3000 });
      }
    },
    [toast]
  );

  /** 刷新全部 */
  const handleRefresh = useCallback(async () => {
    await loadProbe();
    await loadDevices();
    await loadStatus();
    await loadFiles();
  }, [loadProbe, loadDevices, loadStatus, loadFiles]);

  /** 滑块的渐变填充样式 */
  const sliderStyle = (pct: number) => ({
    background: `linear-gradient(to right, ${activeColor} 0%, ${activeColor} ${pct}%, ${trackBg} ${pct}%, ${trackBg} 100%)`,
    borderRadius: "3px",
    height: "6px",
  });

  const ffmpegMissing = probe !== null && !probe.available;

  return (
    <Box pt={8} flex={1} minH="0" display="flex" flexDirection="column">
      {/* 页头 */}
      <Flex w="full" justify="space-between" align="center" mb={6} flexShrink={0}>
        <HStack>
          <IconButton
            aria-label={t("builtinTools.back")}
            icon={<ArrowLeft size={20} />}
            variant="ghost"
            onClick={() => navigate("/")}
            color={adaptiveTitle.text}
          />
          <Heading size="lg" color={adaptiveTitle.text} textShadow={adaptiveTitle.shadow}>
            {t("recording.title")}
          </Heading>
          {status.recording && (
            <Badge colorScheme="red" fontSize="xs" px={2} py={1} borderRadius="md">
              <HStack spacing={1}>
                <Box w="6px" h="6px" borderRadius="full" bg="red.500" />
                <Text>{t("recording.recording")}</Text>
              </HStack>
            </Badge>
          )}
        </HStack>
        <HStack spacing={3}>
          <LiquidGlassButtonFallback
            icon={<RefreshCw size={16} />}
            label={t("recording.refresh")}
            onClick={() => void handleRefresh()}
            disabled={status.recording}
            activeColor={activeColor}
            textColor={textColor}
          />
        </HStack>
      </Flex>

      {/* ffmpeg 缺失提示 */}
      {ffmpegMissing && (
        <Alert status="warning" mb={4} borderRadius="lg" flexShrink={0}>
          <AlertIcon />
          <AlertDescription fontSize="sm">{t("recording.ffmpegMissing")}</AlertDescription>
        </Alert>
      )}

      {/* ffmpeg 已就绪但缺采集设备支持 */}
      {probe?.available && !probe.has_gdigrab && !probe.has_dshow && (
        <Alert status="warning" mb={4} borderRadius="lg" flexShrink={0}>
          <AlertIcon />
          <AlertDescription fontSize="sm">{t("recording.noCaptureSupport")}</AlertDescription>
        </Alert>
      )}

      {/* 主体：左配置 / 右文件列表 */}
      <Flex gap={6} flex={1} minH="0">
        {/* ── 左：采集配置 ── */}
        <Box flex="0 0 420px" minH="0" overflowY="auto" pr={1} sx={scrollbarSx}>
          {/* 录制控制卡 */}
          <Box
            bg={cardBg}
            border="1px solid"
            borderColor={borderColor}
            borderRadius="xl"
            p={5}
            mb={4}
            backdropFilter="blur(12px)"
          >
            <HStack justify="space-between" mb={4}>
              <HStack spacing={2}>
                <Film size={18} color={activeColor} />
                <Text fontWeight="bold" color={textColor} fontSize="md">
                  {t("recording.control")}
                </Text>
              </HStack>
              <Text fontSize="sm" color={subTextColor} fontFamily="monospace">
                {formatDuration(status.elapsed_sec)}
              </Text>
            </HStack>

            {status.recording ? (
              <VStack align="stretch" spacing={3}>
                <HStack spacing={2} fontSize="sm" color={subTextColor}>
                  <Box w="8px" h="8px" borderRadius="full" bg="red.500" />
                  <Text>{formatBytes(status.estimated_bytes)}</Text>
                  {status.encoder && (
                    <Badge fontSize="2xs" colorScheme="purple">
                      {status.encoder}
                    </Badge>
                  )}
                </HStack>
                <Button
                  w="full"
                  colorScheme="red"
                  leftIcon={<Square size={16} />}
                  onClick={() => void handleStop()}
                  isLoading={stopping}
                  loadingText={t("recording.stopping")}
                >
                  {t("recording.stop")}
                </Button>
              </VStack>
            ) : (
              <Button
                w="full"
                bg={activeColor}
                color="white"
                _hover={{ bg: getHoverColor() }}
                leftIcon={<Play size={16} />}
                onClick={() => void handleStart()}
                isLoading={starting}
                loadingText={t("recording.starting")}
                isDisabled={ffmpegMissing}
              >
                {t("recording.start")}
              </Button>
            )}

            {status.last_error && !status.recording && (
              <Text mt={3} fontSize="xs" color="red.400" wordBreak="break-all">
                {status.last_error}
              </Text>
            )}
          </Box>

          {/* 屏幕采集 */}
          <SectionCard
            icon={<Monitor size={18} color={activeColor} />}
            title={t("recording.screen")}
            cardBg={cardBg}
            borderColor={borderColor}
            textColor={textColor}
            mb={4}
          >
            <OptionRow label={t("recording.enableScreen")} textColor={textColor}>
              <Switch
                isChecked={config.screen_enabled}
                onChange={(e) => updateConfig("screen_enabled", e.target.checked)}
                colorScheme="blue"
                isDisabled={status.recording}
              />
            </OptionRow>
            {config.screen_enabled && (
              <OptionRow label={`${t("recording.fps")} · ${config.screen_fps}`} textColor={textColor}>
                <Slider
                  flex={1}
                  min={10}
                  max={60}
                  step={1}
                  value={config.screen_fps}
                  onChange={(v) => updateConfig("screen_fps", v)}
                  isDisabled={status.recording}
                >
                  <SliderTrack sx={sliderStyle(((config.screen_fps - 10) / 50) * 100)}>
                    <SliderFilledTrack bg="transparent" />
                  </SliderTrack>
                  <SliderThumb boxSize={3} bg={activeColor} />
                </Slider>
              </OptionRow>
            )}
            {config.screen_enabled && (
              <>
                <OptionRow label={t("recording.layerVisible")} textColor={textColor}>
                  <Switch
                    size="sm"
                    isChecked={config.screen_layout.visible}
                    onChange={(e) =>
                      updateLayer("screen_layout", { ...config.screen_layout, visible: e.target.checked })
                    }
                    colorScheme="blue"
                    isDisabled={status.recording}
                  />
                </OptionRow>
                {activeLayer === "screen" && (
                  <LayerNumericEditor
                    layer={config.screen_layout}
                    onChange={(l) => updateLayer("screen_layout", l)}
                    textColor={textColor}
                    subTextColor={subTextColor}
                    borderColor={borderColor}
                    activeColor={activeColor}
                    disabled={status.recording}
                    label={t("recording.layerPosition")}
                  />
                )}
              </>
            )}
          </SectionCard>

          {/* 摄像头 */}
          <SectionCard
            icon={<Camera size={18} color={activeColor} />}
            title={t("recording.camera")}
            cardBg={cardBg}
            borderColor={borderColor}
            textColor={textColor}
            mb={4}
            extra={
              loadingDevices ? (
                <Spinner size="xs" color={activeColor} />
              ) : (
                <Badge fontSize="2xs" colorScheme="gray">
                  {cameras.length}
                </Badge>
              )
            }
          >
            <Select
              size="sm"
              value={config.camera_device}
              onChange={(e) => handleCameraDeviceChange(e.target.value)}
              isDisabled={status.recording}
              mb={3}
              borderColor={borderColor}
            >
              <option value="">{t("recording.noCamera")}</option>
              {cameras.map((d) => (
                <option key={d.name} value={d.name}>
                  {d.name}
                </option>
              ))}
            </Select>

            {config.camera_device && (
              <>
                <OptionRow label={t("recording.layerVisible")} textColor={textColor}>
                  <Switch
                    size="sm"
                    isChecked={config.camera_layout.visible}
                    onChange={(e) =>
                      updateLayer("camera_layout", { ...config.camera_layout, visible: e.target.checked })
                    }
                    colorScheme="blue"
                    isDisabled={status.recording}
                  />
                </OptionRow>
                <Text fontSize="2xs" color={subTextColor} mt={2} lineHeight="tall">
                  {t("recording.layoutHint")}
                </Text>
                {activeLayer === "camera" && (
                  <LayerNumericEditor
                    layer={config.camera_layout}
                    onChange={(l) => updateLayer("camera_layout", l)}
                    textColor={textColor}
                    subTextColor={subTextColor}
                    borderColor={borderColor}
                    activeColor={activeColor}
                    disabled={status.recording}
                    label={t("recording.layerPosition")}
                  />
                )}
              </>
            )}
          </SectionCard>

          {/* 麦克风 */}
          <SectionCard
            icon={<Mic size={18} color={activeColor} />}
            title={t("recording.microphone")}
            cardBg={cardBg}
            borderColor={borderColor}
            textColor={textColor}
            mb={4}
          >
            <Select
              size="sm"
              value={config.mic_device}
              onChange={(e) => updateConfig("mic_device", e.target.value)}
              isDisabled={status.recording}
              mb={3}
              borderColor={borderColor}
            >
              <option value="">{t("recording.noMic")}</option>
              {mics.map((d) => (
                <option key={d.name} value={d.name}>
                  {d.name}
                </option>
              ))}
            </Select>
            {config.mic_device && (
              <OptionRow label={`${t("recording.volume")} · ${config.mic_volume}%`} textColor={textColor}>
                <Slider
                  flex={1}
                  min={0}
                  max={200}
                  step={5}
                  value={config.mic_volume}
                  onChange={(v) => updateConfig("mic_volume", v)}
                  isDisabled={status.recording}
                >
                  <SliderTrack sx={sliderStyle((config.mic_volume / 200) * 100)}>
                    <SliderFilledTrack bg="transparent" />
                  </SliderTrack>
                  <SliderThumb boxSize={3} bg={activeColor} />
                </Slider>
              </OptionRow>
            )}
          </SectionCard>

          {/* 系统声音 */}
          <SectionCard
            icon={<Volume2 size={18} color={activeColor} />}
            title={t("recording.systemAudio")}
            cardBg={cardBg}
            borderColor={borderColor}
            textColor={textColor}
            mb={4}
            extra={
              <Tooltip label={t("recording.systemAudioHint")} placement="top" hasArrow>
                <Box>
                  <AlertTriangle size={14} color={subTextColor} />
                </Box>
              </Tooltip>
            }
          >
            <Select
              size="sm"
              value={config.system_audio_device}
              onChange={(e) => updateConfig("system_audio_device", e.target.value)}
              isDisabled={status.recording}
              mb={2}
              borderColor={borderColor}
            >
              <option value="">{t("recording.noSystemAudio")}</option>
              {mics.map((d) => (
                <option key={`sys-${d.name}`} value={d.name}>
                  {d.name}
                </option>
              ))}
            </Select>
            <Text fontSize="xs" color={subTextColor} lineHeight="tall">
              {t("recording.systemAudioHint")}
            </Text>
            {config.system_audio_device && (
              <OptionRow label={`${t("recording.volume")} · ${config.system_volume}%`} textColor={textColor} mt={2}>
                <Slider
                  flex={1}
                  min={0}
                  max={200}
                  step={5}
                  value={config.system_volume}
                  onChange={(v) => updateConfig("system_volume", v)}
                  isDisabled={status.recording}
                >
                  <SliderTrack sx={sliderStyle((config.system_volume / 200) * 100)}>
                    <SliderFilledTrack bg="transparent" />
                  </SliderTrack>
                  <SliderThumb boxSize={3} bg={activeColor} />
                </Slider>
              </OptionRow>
            )}
          </SectionCard>

          {/* 高级设置 */}
          <SectionCard
            icon={<Settings2 size={18} color={activeColor} />}
            title={t("recording.advanced")}
            cardBg={cardBg}
            borderColor={borderColor}
            textColor={textColor}
            mb={4}
          >
            <OptionRow label={t("recording.encoder")} textColor={textColor}>
              <Select
                size="xs"
                w="150px"
                value={config.encoder}
                onChange={(e) => updateConfig("encoder", e.target.value)}
                isDisabled={status.recording}
                borderColor={borderColor}
              >
                <option value="auto">{t("recording.encoderAuto")}</option>
                <option value="libx264">libx264 (CPU)</option>
                <option value="h264_nvenc">NVENC (NVIDIA)</option>
                <option value="h264_qsv">QSV (Intel)</option>
                <option value="h264_amf">AMF (AMD)</option>
              </Select>
            </OptionRow>
            <OptionRow label={`${t("recording.videoBitrate")} · ${config.video_bitrate_kbps}kbps`} textColor={textColor}>
              <Slider
                flex={1}
                min={1000}
                max={20000}
                step={500}
                value={config.video_bitrate_kbps}
                onChange={(v) => updateConfig("video_bitrate_kbps", v)}
                isDisabled={status.recording}
              >
                <SliderTrack sx={sliderStyle(((config.video_bitrate_kbps - 1000) / 19000) * 100)}>
                  <SliderFilledTrack bg="transparent" />
                </SliderTrack>
                <SliderThumb boxSize={3} bg={activeColor} />
              </Slider>
            </OptionRow>
            <OptionRow label={t("recording.audioBitrate")} textColor={textColor}>
              <Select
                size="xs"
                w="110px"
                value={config.audio_bitrate_kbps}
                onChange={(e) => updateConfig("audio_bitrate_kbps", Number(e.target.value))}
                isDisabled={status.recording}
                borderColor={borderColor}
              >
                <option value={96}>96 kbps</option>
                <option value={128}>128 kbps</option>
                <option value={160}>160 kbps</option>
                <option value={192}>192 kbps</option>
                <option value={256}>256 kbps</option>
                <option value={320}>320 kbps</option>
              </Select>
            </OptionRow>
            <VStack align="stretch" spacing={1} mt={2}>
              <Text fontSize="xs" color={subTextColor}>
                {t("recording.outputDir")}
              </Text>
              <Input
                size="sm"
                fontSize="xs"
                placeholder={t("recording.outputDirPlaceholder")}
                value={config.output_dir}
                onChange={(e) => updateConfig("output_dir", e.target.value)}
                isDisabled={status.recording}
                borderColor={borderColor}
              />
            </VStack>
          </SectionCard>

          {/* ffmpeg 信息 */}
          {probe?.available && (
            <Box fontSize="xs" color={subTextColor} px={1} pb={4} wordBreak="break-all" lineHeight="tall">
              <HStack spacing={1} mb={1}>
                <CheckCircle2 size={12} color={activeColor} />
                <Text color={textColor} fontWeight="medium">
                  {t("recording.ffmpegReady")}
                </Text>
              </HStack>
              <Text opacity={0.85}>{probe.version}</Text>
              <Text opacity={0.7}>{probe.path}</Text>
              <HStack spacing={2} mt={1}>
                {probe.has_gdigrab && <Badge fontSize="2xs">gdigrab</Badge>}
                {probe.has_dshow && <Badge fontSize="2xs">dshow</Badge>}
                {probe.hw_encoders.map((e) => (
                  <Badge key={e} fontSize="2xs" colorScheme="purple">
                    {e}
                  </Badge>
                ))}
              </HStack>
            </Box>
          )}
        </Box>

        {/* ── 右：画布预览 + 录制文件 ── */}
        <Box flex={1} minH="0" display="flex" flexDirection="column">
          {/* 实时预览 */}
          <Box mb={4} flexShrink={0}>
            <HStack justify="space-between" mb={2}>
              <HStack spacing={2}>
                <Move size={16} color={activeColor} />
                <Text fontWeight="bold" fontSize="sm" color={textColor}>
                  {t("recording.canvas")}
                </Text>
                {previewRunning && (
                  <Badge fontSize="2xs" colorScheme="green">
                    LIVE
                  </Badge>
                )}
              </HStack>
              <HStack spacing={3}>
                <HStack spacing={1.5} fontSize="xs" color={subTextColor}>
                  <Text>{t("recording.preview")}</Text>
                  <Switch
                    size="sm"
                    isChecked={config.preview_enabled}
                    onChange={(e) => handleTogglePreview(e.target.checked)}
                    colorScheme="blue"
                  />
                </HStack>
                <Tooltip label={t("recording.restoreDefaults")} hasArrow>
                  <IconButton
                    aria-label={t("recording.restoreDefaults")}
                    icon={<RotateCcw size={15} />}
                    size="xs"
                    variant="ghost"
                    color={subTextColor}
                    isDisabled={status.recording}
                    onClick={handleRestoreDefaults}
                  />
                </Tooltip>
              </HStack>
            </HStack>

            {/* 录制画面比例 */}
            <HStack spacing={2} mb={2} flexWrap="wrap" rowGap={2} align="center">
              <Text fontSize="xs" color={subTextColor} flexShrink={0}>
                {t("recording.aspect")}
              </Text>
              {ASPECT_PRESETS.map((p) => {
                const active =
                  config.canvas_w > 0 &&
                  Math.abs(config.canvas_w / Math.max(1, config.canvas_h) - p.w / p.h) < 0.02;
                return (
                  <Button
                    key={p.label}
                    size="xs"
                    px={2.5}
                    height="22px"
                    fontSize="2xs"
                    variant={active ? "solid" : "outline"}
                    bg={active ? activeColor : "transparent"}
                    color={active ? "white" : subTextColor}
                    borderColor={active ? activeColor : borderColor}
                    isDisabled={status.recording}
                    onClick={() => handleAspectChange(p.w, p.h)}
                    _hover={{ borderColor: activeColor, color: active ? "white" : textColor }}
                  >
                    {p.label}
                  </Button>
                );
              })}
              <HStack spacing={1} ml={2}>
                <Input
                  size="xs"
                  w="62px"
                  h="22px"
                  px={1.5}
                  fontSize="2xs"
                  textAlign="center"
                  value={canvasWInput}
                  onChange={(e) => setCanvasWInput(e.target.value)}
                  onBlur={() => handleCanvasSizeChange(Number(canvasWInput) || config.canvas_w, config.canvas_h)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      handleCanvasSizeChange(Number(canvasWInput) || config.canvas_w, config.canvas_h);
                    }
                  }}
                  isDisabled={status.recording}
                  borderColor={borderColor}
                  color={textColor}
                />
                <Text fontSize="2xs" color={subTextColor}>×</Text>
                <Input
                  size="xs"
                  w="62px"
                  h="22px"
                  px={1.5}
                  fontSize="2xs"
                  textAlign="center"
                  value={canvasHInput}
                  onChange={(e) => setCanvasHInput(e.target.value)}
                  onBlur={() => handleCanvasSizeChange(config.canvas_w, Number(canvasHInput) || config.canvas_h)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      handleCanvasSizeChange(config.canvas_w, Number(canvasHInput) || config.canvas_h);
                    }
                  }}
                  isDisabled={status.recording}
                  borderColor={borderColor}
                  color={textColor}
                />
              </HStack>
            </HStack>

            <CanvasPreview
              canvasW={config.canvas_w || screenInfo.width}
              canvasH={config.canvas_h || screenInfo.height}
              screenLayout={config.screen_layout}
              cameraLayout={config.camera_layout}
              screenEnabled={config.screen_enabled}
              cameraDevice={config.camera_device}
              screenStreamUrl={
                previewRunning && previewPort > 0
                  ? `http://127.0.0.1:${previewPort}/preview/screen?t=${previewStamp}`
                  : ""
              }
              cameraStreamUrl={
                previewRunning && previewPort > 0
                  ? `http://127.0.0.1:${previewPort}/preview/camera?t=${previewStamp}`
                  : ""
              }
              activeLayer={activeLayer}
              onActiveLayerChange={setActiveLayer}
              onScreenLayoutChange={(l) => updateLayer("screen_layout", l)}
              onCameraLayoutChange={(l) => updateLayer("camera_layout", l)}
              disabled={status.recording}
              previewState={previewState}
              previewError={previewError}
              previewEnabled={config.preview_enabled}
              screenPreviewError={screenPreviewError}
              cameraPreviewError={cameraPreviewError}
              screenAspect={
                (config.canvas_w || screenInfo.width) /
                Math.max(1, config.canvas_h || screenInfo.height)
              }
              cameraAspect={cameraAspect}
            />
          </Box>

          <HStack justify="space-between" mb={3} flexShrink={0}>
            <HStack spacing={2}>
              <Film size={18} color={activeColor} />
              <Text fontWeight="bold" fontSize="md" color={textColor}>
                {t("recording.myRecordings")}
              </Text>
              <Badge fontSize="2xs">{files.length}</Badge>
            </HStack>
            <IconButton
              aria-label={t("recording.refresh")}
              icon={loadingFiles ? <Spinner size="xs" /> : <RefreshCw size={15} />}
              size="sm"
              variant="ghost"
              onClick={() => void loadFiles()}
              isDisabled={loadingFiles}
            />
          </HStack>

          <Box flex={1} minH="0" overflowY="auto" pr={1} sx={scrollbarSx}>
            {loadingFiles && files.length === 0 ? (
              <Flex align="center" justify="center" h="120px">
                <Spinner size="md" color={activeColor} thickness="3px" />
              </Flex>
            ) : files.length === 0 ? (
              <Flex align="center" justify="center" h="160px">
                <VStack spacing={3}>
                  <FolderOpen size={40} color={subTextColor} />
                  <Text fontSize="sm" color={subTextColor}>
                    {t("recording.noFiles")}
                  </Text>
                </VStack>
              </Flex>
            ) : (
              <VStack align="stretch" spacing={2}>
                {files.map((f) => (
                  <Box
                    key={f.path}
                    bg={cardBg}
                    border="1px solid"
                    borderColor={borderColor}
                    borderRadius="lg"
                    p={3}
                    backdropFilter="blur(8px)"
                    _hover={{ borderColor: activeColor }}
                    transition="border-color 0.15s"
                  >
                    <HStack justify="space-between" align="start">
                      <VStack align="start" spacing={0.5} flex={1} minW={0}>
                        <Text fontSize="sm" color={textColor} noOfLines={1} fontWeight="medium">
                          {f.name}
                        </Text>
                        <HStack spacing={3} fontSize="2xs" color={subTextColor}>
                          <Text>{formatBytes(f.size)}</Text>
                          <Text>{formatTime(f.modified_ms)}</Text>
                          {f.duration_sec > 0 && <Text>{formatDuration(f.duration_sec)}</Text>}
                        </HStack>
                      </VStack>
                      <HStack spacing={1}>
                        <Tooltip label={t("recording.play")} hasArrow>
                          <IconButton
                            aria-label={t("recording.play")}
                            icon={<Play size={14} />}
                            size="xs"
                            variant="ghost"
                            onClick={() => void handleOpen(f)}
                          />
                        </Tooltip>
                        <Tooltip label={t("recording.reveal")} hasArrow>
                          <IconButton
                            aria-label={t("recording.reveal")}
                            icon={<FolderOpen size={14} />}
                            size="xs"
                            variant="ghost"
                            onClick={() => void handleReveal(f)}
                          />
                        </Tooltip>
                        <Tooltip label={t("recording.delete")} hasArrow>
                          <IconButton
                            aria-label={t("recording.delete")}
                            icon={<Trash2 size={14} />}
                            size="xs"
                            variant="ghost"
                            color="red.400"
                            onClick={() => void handleDelete(f)}
                          />
                        </Tooltip>
                      </HStack>
                    </HStack>
                  </Box>
                ))}
              </VStack>
            )}
          </Box>
        </Box>
      </Flex>
    </Box>
  );
}

// ── 小部件 ──

interface SectionCardProps {
  icon: React.ReactNode;
  title: string;
  cardBg: string;
  borderColor: string;
  textColor: string;
  mb?: number;
  extra?: React.ReactNode;
  children: React.ReactNode;
}

/** 分区卡片 */
function SectionCard({ icon, title, cardBg, borderColor, textColor, mb, extra, children }: SectionCardProps) {
  return (
    <Box
      bg={cardBg}
      border="1px solid"
      borderColor={borderColor}
      borderRadius="xl"
      p={4}
      mb={mb}
      backdropFilter="blur(12px)"
    >
      <HStack justify="space-between" mb={3}>
        <HStack spacing={2}>
          {icon}
          <Text fontWeight="bold" color={textColor} fontSize="sm">
            {title}
          </Text>
        </HStack>
        {extra}
      </HStack>
      <Divider borderColor={borderColor} mb={3} />
      {children}
    </Box>
  );
}

interface OptionRowProps {
  label: string;
  textColor: string;
  mt?: number;
  children: React.ReactNode;
}

/** 一行「标签 + 控件」 */
function OptionRow({ label, textColor, mt, children }: OptionRowProps) {
  return (
    <HStack justify="space-between" spacing={3} mt={mt} mb={2}>
      <Text fontSize="xs" color={textColor} flexShrink={0}>
        {label}
      </Text>
      {children}
    </HStack>
  );
}

interface FallbackBtnProps {
  icon: React.ReactNode;
  label: string;
  onClick: () => void;
  disabled?: boolean;
  activeColor: string;
  textColor: string;
}

interface LayerNumericEditorProps {
  layer: LayerLayout;
  onChange: (layer: LayerLayout) => void;
  textColor: string;
  subTextColor: string;
  borderColor: string;
  activeColor: string;
  disabled?: boolean;
  /** 面板标题（由调用方传入已翻译文本） */
  label: string;
}

/**
 * 图层精确数值编辑（拖拽之外的精确控制方式）。
 *
 * 只有 X / Y / W 三个字段：高度由采集源宽高比自动推导，裁剪功能已移除，
 * 所以不再提供 H 与裁剪项，避免用户设出一个会拉伸画面的高度。
 * 输入框失焦或回车才提交，避免边输入边刷新导致光标跳动。
 */
function LayerNumericEditor({
  layer,
  onChange,
  textColor,
  subTextColor,
  borderColor,
  disabled,
  label,
}: LayerNumericEditorProps) {
  const fields: { key: keyof LayerLayout; label: string; min: number; max: number }[] = [
    { key: "x", label: "X", min: -10000, max: 10000 },
    { key: "y", label: "Y", min: -10000, max: 10000 },
    { key: "w", label: "W", min: 2, max: 10000 },
  ];

  const commit = (key: keyof LayerLayout, raw: string) => {
    const num = Number(raw);
    if (!Number.isFinite(num)) return;
    const meta = fields.find((f) => f.key === key);
    if (!meta) return;
    const val = Math.max(meta.min, Math.min(meta.max, Math.round(num)));
    onChange({ ...layer, [key]: val } as LayerLayout);
  };

  return (
    <Box mt={3} p={2} border="1px solid" borderColor={borderColor} borderRadius="md">
      <Text fontSize="2xs" color={subTextColor} mb={2}>
        {label}
      </Text>
      <Box display="grid" gridTemplateColumns="repeat(4, 1fr)" gap={1.5}>
        {fields.map((f) => (
          <VStack key={f.key} align="stretch" spacing={0.5}>
            <Text fontSize="2xs" color={subTextColor} textAlign="center">
              {f.label}
            </Text>
            <Input
              size="xs"
              px={1}
              textAlign="center"
              fontSize="2xs"
              defaultValue={String(layer[f.key] ?? 0)}
              onBlur={(e) => commit(f.key, e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") commit(f.key, (e.target as HTMLInputElement).value);
              }}
              isDisabled={disabled}
              borderColor={borderColor}
              color={textColor}
            />
          </VStack>
        ))}
      </Box>
    </Box>
  );
}

/** 轻量玻璃按钮（避免额外依赖） */
function LiquidGlassButtonFallback({ icon, label, onClick, disabled, activeColor, textColor }: FallbackBtnProps) {
  return (
    <Button
      size="sm"
      variant="outline"
      leftIcon={icon as React.ReactElement}
      onClick={onClick}
      isDisabled={disabled}
      borderColor={activeColor}
      color={textColor}
      _hover={{ bg: `${activeColor}22` }}
    >
      {label}
    </Button>
  );
}
