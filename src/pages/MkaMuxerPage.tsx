import { useCallback, useState, type ReactElement } from "react";
import {
  Box,
  Text,
  Heading,
  VStack,
  HStack,
  IconButton,
  Button,
  Input,
  Badge,
  FormControl,
  FormLabel,
  useColorModeValue,
  useToast,
} from "@chakra-ui/react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  ArrowLeft,
  Disc3,
  FileAudio,
  FileText,
  Image as ImageIcon,
  Save,
  Layers,
  Info,
  X,
} from "lucide-react";
import { LiquidGlassCard } from "@/components/special/liquid-glass-card";
import { useThemeColor } from "@/contexts/theme-color-context";
import { useAdaptiveTextColor } from "@/hooks/use-adaptive-text-color";
import { useFileDrop } from "@/hooks/use-file-drop";
import { FileDropOverlay } from "@/components/ui/file-drop-overlay";
import { useConversionStore } from "@/stores/conversion-store";
import { hexToRgba } from "@/lib/color-utils";

function formatBytes(bytes: number): string {
  if (!bytes) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function formatDuration(ms: number): string {
  if (!ms) return "--";
  const total = Math.round(ms / 1000);
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${seconds.toString().padStart(2, "0")}`;
}

export default function MkaMuxerPage() {
  const navigate = useNavigate();
  const { t } = useTranslation();
  const toast = useToast();
  const { config: themeConfig } = useThemeColor();
  const themeColorHex = themeConfig.primaryColor;
  const themeColorRgba = (opacity: number) => hexToRgba(themeColorHex, opacity);
  const adaptiveTitle = useAdaptiveTextColor();
  const headingColor = useColorModeValue("gray.800", "#ffffff");
  const subTextColor = useColorModeValue("gray.500", "#ffffff");

  const [audioPath, setAudioPath] = useState<string | null>(null);
  const [subtitlePath, setSubtitlePath] = useState<string | null>(null);
  const [coverPath, setCoverPath] = useState<string | null>(null);
  const [outputPath, setOutputPath] = useState<string | null>(null);
  const [title, setTitle] = useState("");

  // 组合任务放在全局 store：离开页面也会继续，结果不会丢失
  const mkaJob = useConversionStore((s) => s.mkaJob);
  const runMkaMux = useConversionStore((s) => s.runMkaMux);
  const clearMkaJob = useConversionStore((s) => s.clearMkaJob);
  const muxing = mkaJob?.status === "running";
  const result = mkaJob?.status === "done" ? mkaJob.result : null;
  const muxError = mkaJob?.status === "error" ? mkaJob.error : null;

  const pickFile = useCallback(
    async (
      kind: "audio" | "subtitle" | "cover",
      filters: { name: string; extensions: string[] }[],
    ) => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        multiple: false,
        title:
          kind === "audio"
            ? t("mkaMuxer.selectAudio")
            : kind === "subtitle"
            ? t("mkaMuxer.selectSubtitle")
            : t("mkaMuxer.selectCover"),
        filters,
      });
      if (typeof selected !== "string") return;
      if (kind === "audio") {
        setAudioPath(selected);
        setTitle((current) => current || selected.replace(/^.*[\\/]/, "").replace(/\.[^.]+$/, ""));
      } else if (kind === "subtitle") {
        setSubtitlePath(selected);
      } else {
        setCoverPath(selected);
      }
      clearMkaJob();
    },
    [t],
  );

  const handlePickAudio = useCallback(async () => {
    try {
      await pickFile("audio", [
        {
          name: t("mkaMuxer.audioFilter"),
          extensions: [
            "mp3", "flac", "wav", "m4a", "aac", "ogg", "opus", "wma", "aiff", "ape", "mka",
          ],
        },
        { name: t("mkaMuxer.allFiles"), extensions: ["*"] },
      ]);
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 3000 });
    }
  }, [pickFile, t, toast]);

  const handlePickSubtitle = useCallback(async () => {
    try {
      await pickFile("subtitle", [
        { name: t("mkaMuxer.subtitleFilter"), extensions: ["srt", "vtt", "ass", "ssa"] },
        { name: t("mkaMuxer.allFiles"), extensions: ["*"] },
      ]);
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 3000 });
    }
  }, [pickFile, t, toast]);

  const handlePickCover = useCallback(async () => {
    try {
      await pickFile("cover", [
        { name: t("mkaMuxer.coverFilter"), extensions: ["jpg", "jpeg", "png", "webp", "gif"] },
      ]);
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 3000 });
    }
  }, [pickFile, t, toast]);

  const handlePickOutput = useCallback(async () => {
    if (!audioPath) {
      toast({ title: t("mkaMuxer.noAudio"), status: "warning", duration: 2500 });
      return;
    }
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const path = await save({
        defaultPath: audioPath.replace(/\.[^.\\/]+$/, "") + ".mka",
        filters: [{ name: "Matroska Audio", extensions: ["mka"] }],
      });
      if (path) setOutputPath(path);
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 3000 });
    }
  }, [audioPath, t, toast]);

  const handleMux = useCallback(async () => {
    if (!audioPath) {
      toast({ title: t("mkaMuxer.noAudio"), status: "warning", duration: 2500 });
      return;
    }
    const target = outputPath ?? audioPath.replace(/\.[^.\\/]+$/, "") + ".mka";
    // 不 await：页面切走后 Promise 仍在全局 store 中继续执行
    void runMkaMux({
      audioPath,
      subtitlePath: subtitlePath ?? undefined,
      coverPath: coverPath ?? undefined,
      outputPath: target,
      title: title.trim() || undefined,
    });
  }, [audioPath, subtitlePath, coverPath, outputPath, title, runMkaMux]);

  // 拖入时按扩展名自动分配到音频 / 字幕 / 封面
  const handleDrop = useCallback(
    (paths: string[]) => {
      let audio: string | undefined;
      let subtitle: string | undefined;
      let cover: string | undefined;
      for (const path of paths) {
        if (/\.(mp3|flac|wav|m4a|aac|ogg|opus|wma|aiff|ape|mka)$/i.test(path)) audio ??= path;
        else if (/\.(srt|vtt|ass|ssa)$/i.test(path)) subtitle ??= path;
        else if (/\.(jpe?g|png|webp|gif|bmp)$/i.test(path)) cover ??= path;
      }
      if (!audio && !subtitle && !cover) {
        toast({ title: "请拖入音频、字幕或封面文件", status: "warning", duration: 3000 });
        return;
      }
      if (audio) {
        setAudioPath(audio);
        setTitle((current) => current || audio!.replace(/^.*[\\/]/, "").replace(/\.[^.]+$/, ""));
      }
      if (subtitle) setSubtitlePath(subtitle);
      if (cover) setCoverPath(cover);
      clearMkaJob();
    },
    [toast],
  );
  const { isDragging, dropProps } = useFileDrop(handleDrop);

  const fileRow = (    label: string,
    path: string | null,
    placeholder: string,
    icon: ReactElement,
    onPick: () => void,
    onClear?: () => void,
  ) => (
    <Box>
      <Text fontSize="sm" fontWeight="bold" mb={2} color={headingColor}>
        {label}
      </Text>
      <HStack spacing={3} flexWrap="wrap">
        <Button
          leftIcon={icon}
          onClick={onPick}
          variant="outline"
          borderColor={themeColorHex}
          color={themeColorHex}
          size="sm"
        >
          {placeholder}
        </Button>
        <Text fontSize="sm" color={path ? subTextColor : subTextColor} wordBreak="break-all" flex={1}>
          {path ?? "--"}
        </Text>
        {path && onClear && (
          <IconButton
            aria-label={t("mkaMuxer.clear")}
            icon={<X size={14} />}
            size="xs"
            variant="ghost"
            onClick={onClear}
            color={subTextColor}
          />
        )}
      </HStack>
    </Box>
  );

  return (
    <Box pt={8} pb={8} {...dropProps}>
      <FileDropOverlay visible={isDragging} hint="松开即可填入音频 / 字幕 / 封面" />
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
              <Layers size={24} />
              <Text>{t("mkaMuxer.title")}</Text>
            </HStack>
          </Heading>
        </HStack>
      </HStack>

      <VStack spacing={4} align="stretch" maxW="820px">
        <LiquidGlassCard p={5}>
          <VStack spacing={4} align="stretch">
            {fileRow(
              t("mkaMuxer.audio"),
              audioPath,
              t("mkaMuxer.selectAudio"),
              <FileAudio size={16} />,
              handlePickAudio,
              () => {
                setAudioPath(null);
                clearMkaJob();
              },
            )}
            {fileRow(
              t("mkaMuxer.subtitle"),
              subtitlePath,
              t("mkaMuxer.selectSubtitle"),
              <FileText size={16} />,
              handlePickSubtitle,
              () => {
                setSubtitlePath(null);
                clearMkaJob();
              },
            )}
            {fileRow(
              t("mkaMuxer.cover"),
              coverPath,
              t("mkaMuxer.selectCover"),
              <ImageIcon size={16} />,
              handlePickCover,
              () => {
                setCoverPath(null);
                clearMkaJob();
              },
            )}

            <FormControl>
              <FormLabel fontSize="sm" fontWeight="bold" color={headingColor}>
                {t("mkaMuxer.titleField")}
              </FormLabel>
              <Input
                size="sm"
                value={title}
                onChange={(event) => setTitle(event.target.value)}
                placeholder={t("mkaMuxer.titlePlaceholder")}
                maxW="420px"
              />
            </FormControl>

            <Box>
              <Text fontSize="sm" fontWeight="bold" mb={2} color={headingColor}>
                {t("mkaMuxer.output")}
              </Text>
              <HStack spacing={3} flexWrap="wrap">
                <Button
                  leftIcon={<Save size={16} />}
                  onClick={handlePickOutput}
                  variant="outline"
                  borderColor={themeColorHex}
                  color={themeColorHex}
                  size="sm"
                >
                  {t("mkaMuxer.selectOutput")}
                </Button>
                <Text fontSize="sm" color={subTextColor} wordBreak="break-all">
                  {outputPath ?? t("mkaMuxer.autoSaveHint")}
                </Text>
              </HStack>
            </Box>

            <HStack pt={2} spacing={3}>
              <Button
                leftIcon={<Disc3 size={16} />}
                onClick={handleMux}
                isLoading={muxing}
                loadingText={t("mkaMuxer.muxing")}
                bg={themeColorHex}
                color="#fff"
                _hover={{ bg: themeColorRgba(0.85) }}
              >
                {t("mkaMuxer.mux")}
              </Button>
            </HStack>

            {muxing && (
              <Text fontSize="xs" color={subTextColor}>
                组合在后台进行，可切换到其他页面，完成后会通过灵动岛提示。
              </Text>
            )}
            {muxError && !muxing && (
              <Text fontSize="xs" color="#e53e3e" wordBreak="break-all">
                {t("mkaMuxer.failed")}: {muxError}
              </Text>
            )}

            {result && (
              <Box p={3} borderRadius="md" bg={themeColorRgba(0.08)} border="1px solid" borderColor={themeColorRgba(0.25)}>
                <HStack spacing={2} mb={1} flexWrap="wrap">
                  <Text fontSize="sm" fontWeight="bold" color={headingColor}>
                    {t("mkaMuxer.resultTitle")}
                  </Text>
                  <Badge colorScheme="green" fontSize="2xs">
                    {t("mkaMuxer.done")}
                  </Badge>
                </HStack>
                <Text fontSize="xs" color={subTextColor} wordBreak="break-all">
                  {result.output_path}
                </Text>
                <HStack spacing={3} mt={1} flexWrap="wrap">
                  <Text fontSize="xs" color={subTextColor}>
                    {t("mkaMuxer.audioCodec")}: {result.audio_codec}
                  </Text>
                  {result.subtitle_codec && (
                    <Text fontSize="xs" color={subTextColor}>
                      {t("mkaMuxer.subtitleCodec")}: {result.subtitle_codec}
                    </Text>
                  )}
                  <Text fontSize="xs" color={subTextColor}>
                    {t("mkaMuxer.duration")}: {formatDuration(result.duration_ms)}
                  </Text>
                  <Text fontSize="xs" color={subTextColor}>
                    {formatBytes(result.size)}
                  </Text>
                  {result.has_cover && (
                    <Badge colorScheme="purple" fontSize="2xs">
                      {t("mkaMuxer.coverIncluded")}
                    </Badge>
                  )}
                </HStack>
              </Box>
            )}

            <HStack spacing={2} align="flex-start" color={subTextColor}>
              <Info size={14} style={{ marginTop: 2, flexShrink: 0 }} />
              <Text fontSize="xs">{t("mkaMuxer.hint")}</Text>
            </HStack>
          </VStack>
        </LiquidGlassCard>
      </VStack>
    </Box>
  );
}
