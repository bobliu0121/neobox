import { useState, useCallback } from "react";
import {
  Box,
  Text,
  Heading,
  VStack,
  HStack,
  IconButton,
  Button,
  Select,
  Spinner,
  useColorModeValue,
  useToast,
} from "@chakra-ui/react";
import { useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { ArrowLeft, FileAudio, FileInput, Save, RefreshCw } from "lucide-react";
import { LiquidGlassCard } from "@/components/special/liquid-glass-card";
import { useThemeColor } from "@/contexts/theme-color-context";
import { useAdaptiveTextColor } from "@/hooks/use-adaptive-text-color";
import { useFileDrop } from "@/hooks/use-file-drop";
import { FileDropOverlay } from "@/components/ui/file-drop-overlay";
import { hexToRgba } from "@/lib/color-utils";

const AUDIO_FORMATS = ["wav", "mp3", "m4a"] as const;

export default function AudioConverterPage() {
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
  const [outputFormat, setOutputFormat] = useState<string>("mp3");
  const [outputPath, setOutputPath] = useState<string | null>(null);
  const [converting, setConverting] = useState(false);

  // 支持把音频文件直接拖到页面任意位置
  const handleDrop = useCallback((paths: string[]) => {
    const audio = paths.find((path) => /\.(wav|mp3|m4a|flac|aac|ogg|opus|wma)$/i.test(path));
    if (!audio) {
      toast({ title: "请拖入音频文件", status: "warning", duration: 3000 });
      return;
    }
    setInputPath(audio);
    setOutputPath(null);
  }, [toast]);
  const { isDragging, dropProps } = useFileDrop(handleDrop);

  const handlePickInput = useCallback(async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        multiple: false,
        title: t("audioConverter.selectInput"),
        filters: [
          { name: "音频文件", extensions: ["wav", "mp3", "m4a"] },
        ],
      });
      if (selected && typeof selected === "string") {
        setInputPath(selected);
        // 重置输出路径，避免沿用旧格式
        setOutputPath(null);
      }
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 3000 });
    }
  }, [t, toast]);

  const handlePickOutput = useCallback(async () => {
    if (!inputPath) return;
    const base = inputPath.replace(/\.[^.\\/]+$/, "");
    const defaultName = `${base}_converted.${outputFormat}`;
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const path = await save({
        defaultPath: defaultName,
        filters: [{ name: "音频文件", extensions: [outputFormat] }],
      });
      if (path) setOutputPath(path);
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 3000 });
    }
  }, [inputPath, outputFormat, toast]);

  const handleConvert = useCallback(async () => {
    if (!inputPath) {
      toast({ title: t("audioConverter.noInput"), status: "warning", duration: 2500 });
      return;
    }
    // 未手动选择输出位置时，自动保存到输入文件所在目录：
    // 原名去扩展名 + "_converted" + 新扩展名，避免覆盖原文件
    const target =
      outputPath ??
      inputPath.replace(/\.[^.\\/]+$/, "") + "_converted." + outputFormat;
    if (inputPath === target) {
      toast({ title: t("audioConverter.samePath"), status: "warning", duration: 2500 });
      return;
    }
    setConverting(true);
    try {
      await invoke("convert_audio", {
        inputPath,
        outputPath: target,
      });
      toast({ title: t("audioConverter.success"), status: "success", duration: 3000 });
      setInputPath(null);
      setOutputPath(null);
    } catch (e) {
      toast({ title: `${t("audioConverter.failed")}: ${String(e)}`, status: "error", duration: 5000 });
    } finally {
      setConverting(false);
    }
  }, [inputPath, outputPath, outputFormat, t, toast]);

  return (
    <Box pt={8} pb={8} {...dropProps}>
      <FileDropOverlay visible={isDragging} hint="松开即可导入音频" />
      <HStack justify="space-between" mb={6} flexWrap="wrap" gap={2}>
        <HStack>
          <IconButton
            aria-label={t("builtinTools.back")}
            icon={<ArrowLeft size={20} />}
            variant="ghost"
            onClick={() => navigate("/builtin-tools")}
            color={headingColor}
          />
          <Heading size="lg" color={adaptiveTitle.text} textShadow={adaptiveTitle.shadow}>
            <HStack spacing={2}>
              <FileAudio size={24} />
              <Text>{t("audioConverter.title")}</Text>
            </HStack>
          </Heading>
        </HStack>
      </HStack>

      <VStack spacing={4} align="stretch" maxW="720px">
        <LiquidGlassCard p={5}>
          <VStack spacing={4} align="stretch">
            {/* 输入文件 */}
            <Box>
              <Text fontSize="sm" fontWeight="bold" mb={2} color={headingColor}>
                {t("audioConverter.selectInput")}
              </Text>
              <HStack spacing={3}>
                <Button
                  leftIcon={<FileInput size={16} />}
                  onClick={handlePickInput}
                  bg={themeColorHex}
                  color="#fff"
                  _hover={{ bg: themeColorRgba(0.85) }}
                >
                  {t("audioConverter.selectInput")}
                </Button>
                <Text fontSize="sm" color={subTextColor} wordBreak="break-all">
                  {inputPath ?? t("audioConverter.inputHint")}
                </Text>
              </HStack>
            </Box>

            {/* 输出格式 */}
            <Box>
              <Text fontSize="sm" fontWeight="bold" mb={2} color={headingColor}>
                {t("audioConverter.outputFormat")}
              </Text>
              <Select
                value={outputFormat}
                onChange={(e) => {
                  setOutputFormat(e.target.value);
                  setOutputPath(null);
                }}
                maxW="200px"
              >
                {AUDIO_FORMATS.map((f) => (
                  <option key={f} value={f}>{f.toUpperCase()}</option>
                ))}
              </Select>
            </Box>

            {/* 输出位置 */}
            <Box>
              <Text fontSize="sm" fontWeight="bold" mb={2} color={headingColor}>
                {t("audioConverter.selectOutput")}
              </Text>
              <HStack spacing={3}>
                <Button
                  leftIcon={<Save size={16} />}
                  onClick={handlePickOutput}
                  variant="outline"
                  color={themeColorHex}
                >
                  {t("audioConverter.selectOutput")}
                </Button>
                <Text fontSize="sm" color={subTextColor} wordBreak="break-all">
                  {outputPath ?? "--"}
                </Text>
              </HStack>
              {!outputPath && (
                <Text fontSize="xs" color={subTextColor} mt={1}>
                  {t("audioConverter.autoSaveHint")}
                </Text>
              )}
            </Box>

            {/* 转换按钮 */}
            <HStack pt={2}>
              <Button
                leftIcon={converting ? <Spinner size="sm" /> : <RefreshCw size={16} />}
                onClick={handleConvert}
                isDisabled={converting}
                bg={themeColorHex}
                color="#fff"
                _hover={{ bg: themeColorRgba(0.85) }}
              >
                {converting ? t("audioConverter.converting") : t("audioConverter.convert")}
              </Button>
            </HStack>
          </VStack>
        </LiquidGlassCard>
      </VStack>
    </Box>
  );
}
