import { useState, useCallback, useRef } from "react";
import {
  Box,
  Text,
  Heading,
  VStack,
  HStack,
  IconButton,
  Button,
  Slider,
  SliderTrack,
  SliderFilledTrack,
  SliderThumb,
  Input,
  Badge,
  Spinner,
  useColorModeValue,
  useToast,
} from "@chakra-ui/react";
import { useNavigate } from "react-router-dom";
import { ArrowLeft, Gauge, FileInput, Save } from "lucide-react";
import { LiquidGlassCard } from "@/components/special/liquid-glass-card";
import { useThemeColor } from "@/contexts/theme-color-context";
import { useAdaptiveTextColor } from "@/hooks/use-adaptive-text-color";
import { useFileDrop } from "@/hooks/use-file-drop";
import { FileDropOverlay } from "@/components/ui/file-drop-overlay";
import { useConversionStore } from "@/stores/conversion-store";
import { ThemeSwitch } from "@/components/special/theme-switch";
import { hexToRgba } from "@/lib/color-utils";

const SPEED_MIN = 0.01;
const SPEED_MAX = 100;
/** 滑块按对数刻度：log10(0.01)=-2 … log10(100)=2，1x 正居中，x 与 1/x 对称 */
const LOG_MIN = Math.log10(SPEED_MIN);
const LOG_MAX = Math.log10(SPEED_MAX);

/** 视频类输入：开启「仅提取音频」后，这类文件的输出要改成 .m4a */
const VIDEO_EXTS = ["mp4", "webm", "mov", "mkv", "avi", "m4v"];

function isVideoPath(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return VIDEO_EXTS.includes(ext);
}

/** 把倍速值格式化为简洁文本（去多余零） */
function formatSpeed(s: number): string {
  const rounded = Math.round(s * 1000) / 1000;
  return String(rounded);
}

export default function AudioSpeedPage() {
  const navigate = useNavigate();
  const toast = useToast();
  const { getActiveColor, config: themeConfig } = useThemeColor();
  const themeColorHex = themeConfig.primaryColor;
  const themeColorRgba = (opacity: number) => hexToRgba(themeColorHex, opacity);
  const adaptiveTitle = useAdaptiveTextColor();
  const headingColor = useColorModeValue("gray.800", "#ffffff");
  const subTextColor = useColorModeValue("gray.500", "#ffffff");

  const [inputPath, setInputPath] = useState<string | null>(null);
  const [outputPath, setOutputPath] = useState<string | null>(null);
  const [speed, setSpeed] = useState(1.0);
  /** 仅提取音频：对视频文件跳过画面，只把音轨变速后输出 .m4a */
  const [audioOnly, setAudioOnly] = useState(false);
  const speedInputRef = useRef<HTMLInputElement>(null);

  /**
   * 实际输出后缀。
   * - 开启「仅提取音频」且输入是视频 → 强制 .m4a：
   *   后端写入的是 AAC-in-MP4，用 .m4a 才能让播放器/资源管理器正确识别为音频；
   *   若沿用 .mp4，系统可能按视频打开导致黑屏。
   * - 其余情况保持输入文件原有的后缀（音频变速仍是原来的容器）。
   */
  const effectiveExt = audioOnly && inputPath && isVideoPath(inputPath)
    ? "m4a"
    : (inputPath?.split(".").pop() || "mp3");

  // 后台任务：页面切走也能继续，重新进入仍显示状态
  const speedJob = useConversionStore((s) => s.speedJob);
  const runSpeedMedia = useConversionStore((s) => s.runSpeedMedia);
  const clearSpeedJob = useConversionStore((s) => s.clearSpeedJob);
  const processing = speedJob?.status === "running";
  const speedError = speedJob?.status === "error" ? speedJob.error : null;
  const speedResult = speedJob?.status === "done" ? speedJob.result : null;

  /** 应用新的倍速值（同时同步输入框文本，不改输入框焦点） */
  const applySpeed = useCallback((s: number) => {
    const clamped = Math.min(SPEED_MAX, Math.max(SPEED_MIN, s));
    setSpeed(clamped);
    if (speedInputRef.current) {
      speedInputRef.current.value = formatSpeed(clamped);
    }
  }, []);

  const handleDrop = useCallback((paths: string[]) => {
    const file = paths.find((path) => /\.(wav|mp3|m4a|flac|aac|ogg|opus|wma|mp4|webm|mov|mkv|avi|m4v)$/i.test(path));
    if (!file) {
      toast({ title: "请拖入音频或视频文件", status: "warning", duration: 3000 });
      return;
    }
    setInputPath(file);
    setOutputPath(null);
    clearSpeedJob();
  }, [toast, clearSpeedJob]);
  const { isDragging, dropProps } = useFileDrop(handleDrop);

  const handlePickInput = useCallback(async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        multiple: false,
        title: "选择要调整速度的文件",
        filters: [
          { name: "音频/视频", extensions: ["wav", "mp3", "m4a", "mp4", "webm", "mov", "mkv", "avi"] },
        ],
      });
      if (selected && typeof selected === "string") {
        setInputPath(selected);
        setOutputPath(null);
        clearSpeedJob();
      }
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 3000 });
    }
  }, [toast, clearSpeedJob]);

  const handlePickOutput = useCallback(async () => {
    if (!inputPath) return;
    const base = inputPath.replace(/\.[^.\\/]+$/, "");
    const defaultName = `${base}_speed${formatSpeed(speed)}.${effectiveExt}`;
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const path = await save({
        defaultPath: defaultName,
        filters: [{ name: "输出文件", extensions: [effectiveExt] }],
      });
      if (path) setOutputPath(path);
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 3000 });
    }
  }, [inputPath, speed, effectiveExt, toast]);

  const handleProcess = useCallback(async () => {
    if (!inputPath) {
      toast({ title: "请先选择输入文件", status: "warning", duration: 2500 });
      return;
    }
    const speedLabel = formatSpeed(speed);
    // 手选路径优先，但要保证后缀与当前模式一致：
    // 开启「仅提取音频」后产物是 AAC-in-MP4；若用户此前手选了 .mp4，
    // 系统会按视频打开导致黑屏 —— 这里强制纠正扩展名为 .m4a。
    const pickedOutput = outputPath && effectiveExt === "m4a" && !/\.m4a$/i.test(outputPath)
      ? outputPath.replace(/\.[^.\/]+$/, "") + ".m4a"
      : outputPath;
    const target =
      pickedOutput ??
      inputPath.replace(/\.[^.\\/]+$/, "") + "_speed" + speedLabel + "." + effectiveExt;
    if (inputPath === target) {
      toast({ title: "输出文件不能和输入文件相同", status: "warning", duration: 2500 });
      return;
    }
    // 不 await：页面切走后 Promise 仍在全局 store 中继续执行
    void runSpeedMedia({ inputPath, outputPath: target, speed, audioOnly });
  }, [inputPath, outputPath, speed, effectiveExt, audioOnly, runSpeedMedia, toast]);

  return (
    <Box pt={8} pb={8} {...dropProps}>
      <FileDropOverlay visible={isDragging} hint="松开即可导入音频/视频" />
      <HStack justify="space-between" mb={6} flexWrap="wrap" gap={2}>
        <HStack>
          <IconButton
            aria-label="返回"
            icon={<ArrowLeft size={20} />}
            variant="ghost"
            onClick={() => navigate("/builtin-tools")}
            color={headingColor}
          />
          <Heading size="lg" color={adaptiveTitle.text} textShadow={adaptiveTitle.shadow}>
            <HStack spacing={2}>
              <Gauge size={24} />
              <Text>音视频加速/减速</Text>
            </HStack>
          </Heading>
        </HStack>
      </HStack>

      <VStack spacing={4} align="stretch" maxW="720px">
        <LiquidGlassCard p={5}>
          <VStack spacing={4} align="stretch">
            <Box>
              <Text fontSize="sm" fontWeight="bold" mb={2} color={headingColor}>
                输入文件
              </Text>
              <HStack spacing={3}>
                <Button
                  leftIcon={<FileInput size={16} />}
                  onClick={handlePickInput}
                  bg={themeColorHex}
                  color="#fff"
                  _hover={{ bg: themeColorRgba(0.85) }}
                >
                  选择文件
                </Button>
                <Text fontSize="sm" color={subTextColor} isTruncated maxW="400px">
                  {inputPath || "未选择"}
                </Text>
              </HStack>
            </Box>

            <Box>
              <HStack justify="space-between" mb={2}>
                <Text fontSize="sm" fontWeight="bold" color={headingColor}>
                  速度
                </Text>
                <HStack spacing={2}>
                  <Text fontSize="sm" color={subTextColor}>
                    手动输入：
                  </Text>
                  <Input
                    ref={speedInputRef}
                    size="sm"
                    width="110px"
                    defaultValue="1"
                    inputMode="decimal"
                    aria-label="手动输入倍速"
                    onChange={(e) => {
                      // 完全非受控：不重写输入框内容，光标永不被外部状态打断
                      const n = parseFloat(e.target.value);
                      if (!Number.isNaN(n) && n > 0) {
                        setSpeed(Math.min(SPEED_MAX, Math.max(SPEED_MIN, n)));
                      }
                    }}
                    onBlur={() => {
                      if (speedInputRef.current) {
                        speedInputRef.current.value = formatSpeed(speed);
                      }
                    }}
                  />
                  <Text fontSize="sm" color={subTextColor}>
                    x
                  </Text>
                </HStack>
              </HStack>
              <Slider
                min={LOG_MIN}
                max={LOG_MAX}
                step={0.001}
                value={Math.log10(speed)}
                onChange={(v) => applySpeed(Math.pow(10, v))}
                focusThumbOnChange={false}
                colorScheme="blue"
              >
                <SliderTrack>
                  <SliderFilledTrack />
                </SliderTrack>
                <SliderThumb />
              </Slider>
              <HStack justify="space-between" mt={1}>
                <Text fontSize="xs" color={subTextColor}>0.01x (1/100 慢速)</Text>
                <Text fontSize="xs" fontWeight="bold" color={headingColor}>1x (正常)</Text>
                <Text fontSize="xs" color={subTextColor}>100x (极速)</Text>
              </HStack>
            </Box>

            {/* 仅提取音频：对视频输入跳过画面，只把音轨变速后输出 .m4a（体积小、处理快） */}
            <Box>
              <HStack justify="space-between" align="flex-start">
                <Box>
                  <Text fontSize="sm" fontWeight="bold" color={headingColor}>
                    仅提取音频（不输出画面）
                  </Text>
                  <Text fontSize="xs" color={subTextColor} mt={1}>
                    {inputPath && isVideoPath(inputPath)
                      ? `视频将只处理音轨，输出为 .m4a（当前：${effectiveExt}）`
                      : "开启后对视频只处理音轨并输出 .m4a；音频文件不受影响"}
                  </Text>
                  {audioOnly && inputPath && isVideoPath(inputPath) && (
                    <Text fontSize="xs" color="#d69e2e" mt={1}>
                      若该视频本身不含音轨，处理会直接报错提示。
                    </Text>
                  )}
                </Box>
                <ThemeSwitch
                  isChecked={audioOnly}
                  onChange={(e) => {
                    setAudioOnly(e.target.checked);
                    // 后缀可能变化（.mp4 → .m4a），旧的手选输出路径要清掉，避免扩展名不匹配
                    setOutputPath(null);
                  }}
                  size="lg"
                  flexShrink={0}
                />
              </HStack>
            </Box>

            <Box>
              <Text fontSize="sm" fontWeight="bold" mb={2} color={headingColor}>
                输出位置（可选）
              </Text>
              <HStack spacing={3}>
                <Button
                  leftIcon={<Save size={16} />}
                  onClick={handlePickOutput}
                  variant="outline"
                >
                  选择输出
                </Button>
                <Text fontSize="sm" color={subTextColor} isTruncated maxW="400px">
                  {outputPath || "默认输出到同目录"}
                </Text>
              </HStack>
            </Box>

            <HStack pt={2} spacing={3} flexWrap="wrap">
              <Button
                leftIcon={processing ? <Spinner size="sm" /> : <Gauge size={16} />}
                onClick={handleProcess}
                isDisabled={processing}
                bg={themeColorHex}
                color="#fff"
                _hover={{ bg: themeColorRgba(0.85) }}
                size="lg"
              >
                {processing ? "处理中..." : "开始处理"}
              </Button>
              {processing && (
                <Text fontSize="xs" color={subTextColor}>
                  任务在后台运行，可切换到其他页面，完成后会通过灵动岛提示。
                </Text>
              )}
              {speedError && !processing && (
                <Text fontSize="xs" color="#e53e3e" wordBreak="break-all">
                  处理失败: {speedError}
                </Text>
              )}
            </HStack>

            {speedResult && (
              <Box p={3} borderRadius="md" bg={themeColorRgba(0.08)} border="1px solid" borderColor={themeColorRgba(0.25)}>
                <HStack spacing={2} mb={1}>
                  <Gauge size={16} color={themeColorHex} />
                  <Text fontSize="sm" fontWeight="bold" color={headingColor}>
                    处理完成
                  </Text>
                  <Badge colorScheme="green" fontSize="2xs">完成</Badge>
                </HStack>
                <Text fontSize="xs" color={subTextColor} wordBreak="break-all">
                  {speedResult.output_path}
                </Text>
                {speedResult.warning && (
                  <Text fontSize="xs" color="#d69e2e" mt={1} wordBreak="break-all">
                    {speedResult.warning}
                  </Text>
                )}
              </Box>
            )}
          </VStack>
        </LiquidGlassCard>
      </VStack>
    </Box>
  );
}
