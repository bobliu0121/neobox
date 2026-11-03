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
} from "lucide-react";

// ── 与后端一致的类型定义 ──

interface CaptureDevice {
  name: string;
  kind: string;
  alt_name: string;
}

interface RecorderConfig {
  screen_enabled: boolean;
  screen_fps: number;
  camera_device: string;
  camera_width: number;
  camera_height: number;
  pip_scale: number;
  pip_margin: number;
  pip_position: string;
  mic_device: string;
  system_audio_device: string;
  mic_volume: number;
  system_volume: number;
  video_bitrate_kbps: number;
  audio_bitrate_kbps: number;
  encoder: string;
  output_dir: string;
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

const DEFAULT_CONFIG: RecorderConfig = {
  screen_enabled: true,
  screen_fps: 30,
  camera_device: "",
  camera_width: 0,
  camera_height: 0,
  pip_scale: 25,
  pip_margin: 24,
  pip_position: "bottom-right",
  mic_device: "",
  system_audio_device: "",
  mic_volume: 100,
  system_volume: 100,
  video_bitrate_kbps: 6000,
  audio_bitrate_kbps: 160,
  encoder: "auto",
  output_dir: "",
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
  }, [loadProbe, loadDevices, loadConfig, loadStatus]);

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

  /** 保存设置（静默，不打扰用户） */
  const persistConfig = useCallback(async (next: RecorderConfig) => {
    try {
      await invoke("recorder_save_config", { config: next });
    } catch (e) {
      console.error("recorder_save_config failed:", e);
    }
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
              onChange={(e) => updateConfig("camera_device", e.target.value)}
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
                <OptionRow label={`${t("recording.pipScale")} · ${config.pip_scale}%`} textColor={textColor}>
                  <Slider
                    flex={1}
                    min={10}
                    max={60}
                    step={1}
                    value={config.pip_scale}
                    onChange={(v) => updateConfig("pip_scale", v)}
                    isDisabled={status.recording}
                  >
                    <SliderTrack sx={sliderStyle(((config.pip_scale - 10) / 50) * 100)}>
                      <SliderFilledTrack bg="transparent" />
                    </SliderTrack>
                    <SliderThumb boxSize={3} bg={activeColor} />
                  </Slider>
                </OptionRow>
                <OptionRow label={t("recording.pipPosition")} textColor={textColor}>
                  <Select
                    size="xs"
                    w="130px"
                    value={config.pip_position}
                    onChange={(e) => updateConfig("pip_position", e.target.value)}
                    isDisabled={status.recording}
                    borderColor={borderColor}
                  >
                    <option value="bottom-right">{t("recording.posBottomRight")}</option>
                    <option value="bottom-left">{t("recording.posBottomLeft")}</option>
                    <option value="top-right">{t("recording.posTopRight")}</option>
                    <option value="top-left">{t("recording.posTopLeft")}</option>
                  </Select>
                </OptionRow>
                <OptionRow label={`${t("recording.pipMargin")} · ${config.pip_margin}px`} textColor={textColor}>
                  <Slider
                    flex={1}
                    min={0}
                    max={120}
                    step={4}
                    value={config.pip_margin}
                    onChange={(v) => updateConfig("pip_margin", v)}
                    isDisabled={status.recording}
                  >
                    <SliderTrack sx={sliderStyle((config.pip_margin / 120) * 100)}>
                      <SliderFilledTrack bg="transparent" />
                    </SliderTrack>
                    <SliderThumb boxSize={3} bg={activeColor} />
                  </Slider>
                </OptionRow>
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

        {/* ── 右：录制文件 ── */}
        <Box flex={1} minH="0" display="flex" flexDirection="column">
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
