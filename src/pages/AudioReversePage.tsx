import { useState, useCallback } from "react";
import {
  Box,
  Text,
  Heading,
  VStack,
  HStack,
  IconButton,
  Button,
  Spinner,
  Badge,
  useColorModeValue,
  useToast,
} from "@chakra-ui/react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Rewind, FileInput, Save } from "lucide-react";
import { LiquidGlassCard } from "@/components/special/liquid-glass-card";
import { useThemeColor } from "@/contexts/theme-color-context";
import { useAdaptiveTextColor } from "@/hooks/use-adaptive-text-color";
import { useFileDrop } from "@/hooks/use-file-drop";
import { FileDropOverlay } from "@/components/ui/file-drop-overlay";
import { useConversionStore } from "@/stores/conversion-store";
import { ThemeSwitch } from "@/components/special/theme-switch";

/** 视频类输入：开启「仅提取音频」后，这类文件的输出要改成 .m4a */
const VIDEO_EXTS = ["mp4", "webm", "mov", "mkv", "avi", "m4v"];

function isVideoPath(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return VIDEO_EXTS.includes(ext);
}
import { hexToRgba } from "@/lib/color-utils";

export default function AudioReversePage() {
  const navigate = useNavigate();
  const { t } = useTranslation();
  const toast = useToast();
  const { getActiveColor, config: themeConfig } = useThemeColor();
  const themeColorHex = themeConfig.primaryColor;
  const themeColorRgba = (opacity: number) => hexToRgba(themeColorHex, opacity);
  const adaptiveTitle = useAdaptiveTextColor();
  const headingColor = useColorModeValue("gray.800", "#ffffff");
  const subTextColor = useColorModeValue("gray.500", "#ffffff");

  const [inputPath, setInputPath] = useState<string | null>(null);
  const [outputPath, setOutputPath] = useState<string | null>(null);
  /** 仅提取音频：对视频文件跳过画面，只把音轨倒放后输出 .m4a */
  const [audioOnly, setAudioOnly] = useState(false);

  /**
   * 实际输出后缀。
   * 开启「仅提取音频」且输入是视频时强制 .m4a —— 后端写的是 AAC-in-MP4，
   * 沿用 .mp4 会让系统按视频打开（黑屏）。其余情况保持输入文件原后缀。
   */
  const effectiveExt = audioOnly && inputPath && isVideoPath(inputPath)
    ? "m4a"
    : (inputPath?.split(".").pop() || "mp3");

  // 后台任务：页面切走也能继续，重新进入仍显示状态
  const reverseJob = useConversionStore((s) => s.reverseJob);
  const runReverseMedia = useConversionStore((s) => s.runReverseMedia);
  const processing = reverseJob?.status === "running";
  const reverseError = reverseJob?.status === "error" ? reverseJob.error : null;
  const reverseResult = reverseJob?.status === "done" ? reverseJob.result : null;

  const handleDrop = useCallback((paths: string[]) => {
    const file = paths.find((path) => /\.(wav|mp3|m4a|flac|aac|ogg|opus|wma|mp4|webm|mov|mkv|avi|m4v)$/i.test(path));
    if (!file) {
      toast({ title: "请拖入音频或视频文件", status: "warning", duration: 3000 });
      return;
    }
    setInputPath(file);
    setOutputPath(null);
  }, [toast]);
  const { isDragging, dropProps } = useFileDrop(handleDrop);

  const handlePickInput = useCallback(async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        multiple: false,
        title: "选择要倒放的文件",
        filters: [
          { name: "音频/视频", extensions: ["wav", "mp3", "m4a", "mp4", "webm", "mov", "mkv", "avi"] },
        ],
      });
      if (selected && typeof selected === "string") {
        setInputPath(selected);
        setOutputPath(null);
      }
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 3000 });
    }
  }, [toast]);

  const handlePickOutput = useCallback(async () => {
    if (!inputPath) return;
    const base = inputPath.replace(/\.[^.\\/]+$/, "");
    const defaultName = `${base}_reversed.${effectiveExt}`;
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
  }, [inputPath, effectiveExt, toast]);

  const handleProcess = useCallback(async () => {
    if (!inputPath) {
      toast({ title: "请先选择输入文件", status: "warning", duration: 2500 });
      return;
    }
    // 未手动选择输出位置时，自动保存到输入文件所在目录
    const target =
      outputPath ??
      inputPath.replace(/\.[^.\\/]+$/, "") + "_reversed." + effectiveExt;
    if (inputPath === target) {
      toast({ title: "输出文件不能和输入文件相同", status: "warning", duration: 2500 });
      return;
    }
    // 不 await：页面切走后 Promise 仍在全局 store 中继续执行
    void runReverseMedia({ inputPath, outputPath: target, audioOnly });
  }, [inputPath, outputPath, effectiveExt, audioOnly, runReverseMedia, toast]);

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
              <Rewind size={24} />
              <Text>音视频倒放</Text>
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

            {/* 仅提取音频：对视频输入跳过画面，只把音轨倒放后输出 .m4a */}
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
                    // 后缀可能变化（.mp4 → .m4a），清掉旧的手选输出路径避免扩展名不匹配
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
                leftIcon={processing ? <Spinner size="sm" /> : <Rewind size={16} />}
                onClick={handleProcess}
                isDisabled={processing}
                bg={themeColorHex}
                color="#fff"
                _hover={{ bg: themeColorRgba(0.85) }}
                size="lg"
              >
                {processing ? "处理中..." : "开始倒放"}
              </Button>
              {processing && (
                <Text fontSize="xs" color={subTextColor}>
                  任务在后台运行，可切换到其他页面，完成后会通过灵动岛提示。
                </Text>
              )}
              {reverseError && !processing && (
                <Text fontSize="xs" color="#e53e3e" wordBreak="break-all">
                  倒放失败: {reverseError}
                </Text>
              )}
            </HStack>

            {reverseResult && (
              <Box p={3} borderRadius="md" bg={themeColorRgba(0.08)} border="1px solid" borderColor={themeColorRgba(0.25)}>
                <HStack spacing={2} mb={1}>
                  <Rewind size={16} color={themeColorHex} />
                  <Text fontSize="sm" fontWeight="bold" color={headingColor}>
                    倒放完成
                  </Text>
                  <Badge colorScheme="green" fontSize="2xs">完成</Badge>
                </HStack>
                <Text fontSize="xs" color={subTextColor} wordBreak="break-all">
                  {reverseResult.output_path}
                </Text>
              </Box>
            )}
          </VStack>
        </LiquidGlassCard>
      </VStack>
    </Box>
  );
}
