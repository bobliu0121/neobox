import { useCallback, useMemo, useState } from "react";
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
  NumberInput,
  NumberInputField,
  NumberInputStepper,
  NumberIncrementStepper,
  NumberDecrementStepper,
  FormControl,
  FormLabel,
  Badge,
  useColorModeValue,
  useToast,
} from "@chakra-ui/react";
import { useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Clapperboard, FileInput, Save, RefreshCw, Film, Info } from "lucide-react";
import { LiquidGlassCard } from "@/components/special/liquid-glass-card";
import { useThemeColor } from "@/contexts/theme-color-context";
import { useAdaptiveTextColor } from "@/hooks/use-adaptive-text-color";
import { hexToRgba } from "@/lib/color-utils";

type VideoOutputFormat = {
  value: string;
  label: string;
  group: "image" | "audio" | "gif";
  extension: string;
};

const OUTPUT_FORMATS: VideoOutputFormat[] = [
  { value: "gif", label: "GIF（动图）", group: "gif", extension: "gif" },
  { value: "png", label: "PNG（首帧）", group: "image", extension: "png" },
  { value: "jpg", label: "JPG（首帧）", group: "image", extension: "jpg" },
  { value: "webp", label: "WebP（首帧）", group: "image", extension: "webp" },
  { value: "mp3", label: "MP3（音频）", group: "audio", extension: "mp3" },
  { value: "m4a", label: "M4A / AAC（音频）", group: "audio", extension: "m4a" },
  { value: "wav", label: "WAV（音频）", group: "audio", extension: "wav" },
];

interface ConvertResult {
  output_path: string;
  frames: number;
  width: number;
  height: number;
  duration_ms: number;
}

export default function VideoConverterPage() {
  const navigate = useNavigate();
  const { t } = useTranslation();
  const toast = useToast();
  const { config: themeConfig } = useThemeColor();
  const themeColorHex = themeConfig.primaryColor;
  const themeColorRgba = (opacity: number) => hexToRgba(themeColorHex, opacity);
  const adaptiveTitle = useAdaptiveTextColor();
  const headingColor = useColorModeValue("gray.800", "#ffffff");
  const subTextColor = useColorModeValue("gray.500", "#ffffff");

  const [inputPath, setInputPath] = useState<string | null>(null);
  const [outputFormat, setOutputFormat] = useState("gif");
  const [outputPath, setOutputPath] = useState<string | null>(null);
  const [fps, setFps] = useState(12);
  const [width, setWidth] = useState(480);
  const [startSeconds, setStartSeconds] = useState(0);
  const [durationSeconds, setDurationSeconds] = useState(10);
  const [converting, setConverting] = useState(false);
  const [result, setResult] = useState<ConvertResult | null>(null);

  const selectedFormat = useMemo(
    () => OUTPUT_FORMATS.find((item) => item.value === outputFormat) ?? OUTPUT_FORMATS[0],
    [outputFormat],
  );
  const isGif = selectedFormat.group === "gif";

  const handlePickInput = useCallback(async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        multiple: false,
        title: t("videoConverter.selectInput"),
        filters: [
          {
            name: "视频 / 音频",
            extensions: [
              "mp4", "mkv", "mov", "avi", "webm", "flv", "wmv", "m4v", "ts", "m2ts", "mpg", "mpeg",
              "mp3", "wav", "flac", "aac", "m4a", "ogg", "opus", "wma",
            ],
          },
          { name: "所有文件", extensions: ["*"] },
        ],
      });
      if (typeof selected === "string") {
        setInputPath(selected);
        setOutputPath(null);
        setResult(null);
      }
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 3000 });
    }
  }, [t, toast]);

  const defaultOutputPath = useCallback(
    (source: string, extension: string) =>
      source.replace(/\.[^.\\/]+$/, "") + `_converted.${extension}`,
    [],
  );

  const handlePickOutput = useCallback(async () => {
    if (!inputPath) return;
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const path = await save({
        defaultPath: defaultOutputPath(inputPath, selectedFormat.extension),
        filters: [{ name: selectedFormat.label, extensions: [selectedFormat.extension] }],
      });
      if (path) setOutputPath(path);
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 3000 });
    }
  }, [inputPath, selectedFormat, defaultOutputPath, toast]);

  const handleConvert = useCallback(async () => {
    if (!inputPath) {
      toast({ title: t("videoConverter.noInput"), status: "warning", duration: 2500 });
      return;
    }
    const target = outputPath ?? defaultOutputPath(inputPath, selectedFormat.extension);
    if (inputPath === target) {
      toast({ title: t("videoConverter.samePath"), status: "warning", duration: 2500 });
      return;
    }
    setConverting(true);
    setResult(null);
    try {
      const converted = await invoke<ConvertResult>("convert_video", {
        inputPath,
        outputPath: target,
        format: outputFormat,
        fps,
        width,
        startSeconds,
        durationSeconds: selectedFormat.group === "image" ? 0 : durationSeconds,
      });
      setResult(converted);
      toast({ title: t("videoConverter.success"), status: "success", duration: 3000 });
    } catch (e) {
      toast({
        title: `${t("videoConverter.failed")}: ${String(e)}`,
        status: "error",
        duration: 6000,
        isClosable: true,
      });
    } finally {
      setConverting(false);
    }
  }, [
    inputPath,
    outputPath,
    outputFormat,
    fps,
    width,
    startSeconds,
    durationSeconds,
    selectedFormat,
    defaultOutputPath,
    t,
    toast,
  ]);

  return (
    <Box pt={8} pb={8}>
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
              <Clapperboard size={24} />
              <Text>{t("videoConverter.title")}</Text>
            </HStack>
          </Heading>
        </HStack>
      </HStack>

      <VStack spacing={4} align="stretch" maxW="820px">
        <LiquidGlassCard p={5}>
          <VStack spacing={4} align="stretch">
            <Box>
              <Text fontSize="sm" fontWeight="bold" mb={2} color={headingColor}>
                {t("videoConverter.selectInput")}
              </Text>
              <HStack spacing={3} flexWrap="wrap">
                <Button
                  leftIcon={<FileInput size={16} />}
                  onClick={handlePickInput}
                  bg={themeColorHex}
                  color="#fff"
                  _hover={{ bg: themeColorRgba(0.85) }}
                >
                  {t("videoConverter.selectInput")}
                </Button>
                <Text fontSize="sm" color={subTextColor} wordBreak="break-all">
                  {inputPath ?? t("videoConverter.inputHint")}
                </Text>
              </HStack>
            </Box>

            <FormControl>
              <FormLabel fontSize="sm" fontWeight="bold" color={headingColor}>
                {t("videoConverter.outputFormat")}
              </FormLabel>
              <Select
                value={outputFormat}
                maxW="300px"
                onChange={(event) => {
                  setOutputFormat(event.target.value);
                  setOutputPath(null);
                  setResult(null);
                }}
              >
                {OUTPUT_FORMATS.map((item) => (
                  <option key={item.value} value={item.value}>
                    {item.label}
                  </option>
                ))}
              </Select>
            </FormControl>

            {isGif && (
              <HStack spacing={4} flexWrap="wrap" align="flex-end">
                <FormControl width="150px">
                  <FormLabel fontSize="sm" color={headingColor}>
                    {t("videoConverter.fps")}
                  </FormLabel>
                  <NumberInput size="sm" min={1} max={30} value={fps} onChange={(_, value) => setFps(Number(value) || 12)}>
                    <NumberInputField />
                    <NumberInputStepper>
                      <NumberIncrementStepper />
                      <NumberDecrementStepper />
                    </NumberInputStepper>
                  </NumberInput>
                </FormControl>
                <FormControl width="170px">
                  <FormLabel fontSize="sm" color={headingColor}>
                    {t("videoConverter.width")}
                  </FormLabel>
                  <NumberInput size="sm" min={64} max={1920} step={16} value={width} onChange={(_, value) => setWidth(Number(value) || 480)}>
                    <NumberInputField />
                    <NumberInputStepper>
                      <NumberIncrementStepper />
                      <NumberDecrementStepper />
                    </NumberInputStepper>
                  </NumberInput>
                </FormControl>
                <FormControl width="150px">
                  <FormLabel fontSize="sm" color={headingColor}>
                    {t("videoConverter.start")}
                  </FormLabel>
                  <NumberInput size="sm" min={0} value={startSeconds} onChange={(_, value) => setStartSeconds(Number(value) || 0)}>
                    <NumberInputField />
                    <NumberInputStepper>
                      <NumberIncrementStepper />
                      <NumberDecrementStepper />
                    </NumberInputStepper>
                  </NumberInput>
                </FormControl>
                <FormControl width="150px">
                  <FormLabel fontSize="sm" color={headingColor}>
                    {t("videoConverter.duration")}
                  </FormLabel>
                  <NumberInput size="sm" min={1} max={600} value={durationSeconds} onChange={(_, value) => setDurationSeconds(Number(value) || 10)}>
                    <NumberInputField />
                    <NumberInputStepper>
                      <NumberIncrementStepper />
                      <NumberDecrementStepper />
                    </NumberInputStepper>
                  </NumberInput>
                </FormControl>
              </HStack>
            )}

            <Box>
              <Text fontSize="sm" fontWeight="bold" mb={2} color={headingColor}>
                {t("videoConverter.selectOutput")}
              </Text>
              <HStack spacing={3} flexWrap="wrap">
                <Button leftIcon={<Save size={16} />} onClick={handlePickOutput} variant="outline" color={themeColorHex}>
                  {t("videoConverter.selectOutput")}
                </Button>
                <Text fontSize="sm" color={subTextColor} wordBreak="break-all">
                  {outputPath ?? t("videoConverter.autoSaveHint")}
                </Text>
              </HStack>
            </Box>

            <HStack pt={2} spacing={3}>
              <Button
                leftIcon={converting ? <Spinner size="sm" /> : <RefreshCw size={16} />}
                onClick={handleConvert}
                isDisabled={converting}
                bg={themeColorHex}
                color="#fff"
                _hover={{ bg: themeColorRgba(0.85) }}
              >
                {converting ? t("videoConverter.converting") : t("videoConverter.convert")}
              </Button>
              {converting && (
                <Text fontSize="xs" color={subTextColor}>
                  {t("videoConverter.convertingHint")}
                </Text>
              )}
            </HStack>

            {result && (
              <Box p={3} borderRadius="md" bg={themeColorRgba(0.08)} border="1px solid" borderColor={themeColorRgba(0.25)}>
                <HStack spacing={2} mb={1}>
                  <Film size={16} color={themeColorHex} />
                  <Text fontSize="sm" fontWeight="bold" color={headingColor}>
                    {t("videoConverter.resultTitle")}
                  </Text>
                  <Badge colorScheme="green" fontSize="2xs">
                    {t("videoConverter.done")}
                  </Badge>
                </HStack>
                <Text fontSize="xs" color={subTextColor} wordBreak="break-all">
                  {result.output_path}
                </Text>
                <HStack spacing={4} mt={1} flexWrap="wrap">
                  {result.frames > 0 && (
                    <Text fontSize="xs" color={subTextColor}>
                      {t("videoConverter.frames", { count: result.frames })}
                    </Text>
                  )}
                  {result.width > 0 && (
                    <Text fontSize="xs" color={subTextColor}>
                      {result.width} × {result.height}
                    </Text>
                  )}
                  {result.duration_ms > 0 && (
                    <Text fontSize="xs" color={subTextColor}>
                      {(result.duration_ms / 1000).toFixed(1)} s
                    </Text>
                  )}
                </HStack>
              </Box>
            )}

            <HStack spacing={2} align="flex-start" color={subTextColor}>
              <Info size={14} style={{ marginTop: 2, flexShrink: 0 }} />
              <Text fontSize="xs">{t("videoConverter.hint")}</Text>
            </HStack>
          </VStack>
        </LiquidGlassCard>
      </VStack>
    </Box>
  );
}
