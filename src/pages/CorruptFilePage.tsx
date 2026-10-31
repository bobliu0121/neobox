import { useState, useCallback, useEffect, useMemo } from "react";
import {
  Box,
  Text,
  Heading,
  VStack,
  HStack,
  IconButton,
  Button,
  Spinner,
  Input,
  NumberInput,
  NumberInputField,
  NumberInputStepper,
  NumberIncrementStepper,
  NumberDecrementStepper,
  Select,
  Switch,
  Divider,
  Badge,
  useColorModeValue,
  useToast,
  Tooltip,
} from "@chakra-ui/react";
import { useNavigate } from "react-router-dom";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import {
  ArrowLeft,
  FileInput,
  Save,
  RefreshCw,
  ShieldAlert,
  Hash,
  Dices,
  Info,
  Trash2,
} from "lucide-react";
import { LiquidGlassCard } from "@/components/special/liquid-glass-card";
import { useThemeColor } from "@/contexts/theme-color-context";
import { useAdaptiveTextColor } from "@/hooks/use-adaptive-text-color";
import { useFileDrop } from "@/hooks/use-file-drop";
import { FileDropOverlay } from "@/components/ui/file-drop-overlay";
import { hexToRgba } from "@/lib/color-utils";

/** 损坏方式：与后端 CorruptMode 的 serde(rename_all = "snake_case") 对应 */
type CorruptMode = "random" | "zero" | "ones";

/** 后端 corrupt_estimate 的返回结构 */
interface CorruptEstimate {
  file_size: number;
  range_start: number;
  range_end: number;
  span: number;
  corrupt_bytes: number;
  percent: number;
}

/** 后端 corrupt_file_random 的返回结构 */
interface CorruptResult {
  output_path: string;
  original_size: number;
  output_size: number;
  corrupted_bytes: number;
  range_start: number;
  range_end: number;
  seed: number;
  mode: string;
  percent: number;
  damaged_blocks: number;
  message: string;
}

/** 字节数格式化：大数值用 KB/MB 表示，便于理解损坏规模 */
function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(2)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

/**
 * 文件随机损坏页。
 *
 * 设计要点（都已与用户确认）：
 * - **只写新文件，永不覆盖原文件**：后端拒绝"输出=输入"的请求，页面也不提供就地覆盖开关。
 * - 六个参数全部可控：比例 / 精确字节数 / 起止偏移 / 随机种子 / 损坏方式 / 保大小。
 * - 生成结构化报告（损坏字节数、离散块数、种子），便于复现与对照修复工具的表现。
 */
export default function CorruptFilePage() {
  const navigate = useNavigate();
  const { t } = useTranslation();
  const toast = useToast();
  const { config: themeConfig } = useThemeColor();
  const themeColorHex = themeConfig.primaryColor;
  const themeColorRgba = (opacity: number) => hexToRgba(themeColorHex, opacity);
  const adaptiveTitle = useAdaptiveTextColor();
  const headingColor = useColorModeValue("gray.800", "#ffffff");
  const subTextColor = useColorModeValue("gray.500", "#ffffff");
  const warnColor = useColorModeValue("red.600", "red.300");

  // ── 状态 ────────────────────────────────────────────────────────
  const [inputPath, setInputPath] = useState<string | null>(null);
  // 参数模式：按比例 / 按精确字节数
  const [sizeMode, setSizeMode] = useState<"ratio" | "bytes">("ratio");
  const [ratioPercent, setRatioPercent] = useState(5);
  const [byteCount, setByteCount] = useState(1024);
  // 损坏范围：留空表示不限制（用整个文件）
  const [rangeStart, setRangeStart] = useState("");
  const [rangeEnd, setRangeEnd] = useState("");
  // 随机种子：留空表示每次随机；填入则结果可复现
  const [seedText, setSeedText] = useState("");
  const [mode, setMode] = useState<CorruptMode>("random");
  const [keepSize, setKeepSize] = useState(true);
  // 预估结果（后端计算，避免前端重复实现一套边界逻辑）
  const [estimate, setEstimate] = useState<CorruptEstimate | null>(null);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<CorruptResult | null>(null);

  // ── 选文件 ──────────────────────────────────────────────────────
  const handlePickInput = useCallback(async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        multiple: false,
        directory: false,
        title: t("corruptFile.selectInput"),
      });
      if (typeof selected !== "string") return;
      setInputPath(selected);
      setResult(null);
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 3000 });
    }
  }, [t, toast]);

  // 支持拖入任意文件（不限制类型，与用户选择一致）
  const handleDrop = useCallback((paths: string[]) => {
    const first = paths[0];
    if (!first) return;
    setInputPath(first);
    setResult(null);
  }, []);
  const { isDragging, dropProps } = useFileDrop(handleDrop);

  // ── 预估（参数变化时自动刷新）────────────────────────────────────
  useEffect(() => {
    if (!inputPath) {
      setEstimate(null);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const est = await invoke<CorruptEstimate>("corrupt_estimate", {
          path: inputPath,
          ratio: sizeMode === "ratio" ? ratioPercent / 100 : null,
          bytes: sizeMode === "bytes" ? Math.max(0, Math.round(byteCount)) : null,
          start: rangeStart.trim() === "" ? null : Math.max(0, Number(rangeStart)),
          end: rangeEnd.trim() === "" ? null : Math.max(0, Number(rangeEnd)),
        });
        if (!cancelled) setEstimate(est);
      } catch {
        if (!cancelled) setEstimate(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [inputPath, sizeMode, ratioPercent, byteCount, rangeStart, rangeEnd]);

  // ── 执行损坏 ────────────────────────────────────────────────────
  const handleRun = useCallback(async () => {
    if (!inputPath) {
      toast({ title: t("corruptFile.noInput"), status: "warning", duration: 2500 });
      return;
    }
    setRunning(true);
    setResult(null);
    try {
      const seed = seedText.trim() === "" ? null : Number(seedText);
      if (seed !== null && (!Number.isFinite(seed) || seed < 0)) {
        toast({ title: t("corruptFile.seedInvalid"), status: "warning", duration: 2500 });
        return;
      }
      const r = await invoke<CorruptResult>("corrupt_file_random", {
        req: {
          path: inputPath,
          output: null, // 后端自动生成 xxx_corrupted.ext，绝不覆盖原文件
          ratio: sizeMode === "ratio" ? ratioPercent / 100 : null,
          bytes: sizeMode === "bytes" ? Math.max(0, Math.round(byteCount)) : null,
          start: rangeStart.trim() === "" ? null : Math.max(0, Number(rangeStart)),
          end: rangeEnd.trim() === "" ? null : Math.max(0, Number(rangeEnd)),
          seed,
          mode,
          keep_size: keepSize,
        },
      });
      setResult(r);
      // 回填实际使用的种子，方便用户用同一参数复现
      setSeedText(String(r.seed));
      toast({ title: t("corruptFile.success"), status: "success", duration: 3000 });
    } catch (e) {
      toast({
        title: `${t("corruptFile.failed")}: ${String(e)}`,
        status: "error",
        duration: 6000,
      });
    } finally {
      setRunning(false);
    }
  }, [
    inputPath,
    seedText,
    sizeMode,
    ratioPercent,
    byteCount,
    rangeStart,
    rangeEnd,
    mode,
    keepSize,
    t,
    toast,
  ]);

  // 预估摘要文案（放在 useMemo 里，避免每次渲染重复拼接）
  const estimateText = useMemo(() => {
    if (!estimate) return null;
    return t("corruptFile.estimateSummary", {
      fileSize: formatBytes(estimate.file_size),
      bytes: estimate.corrupt_bytes.toLocaleString(),
      percent: estimate.percent.toFixed(2),
      start: estimate.range_start.toLocaleString(),
      end: estimate.range_end.toLocaleString(),
    });
  }, [estimate, t]);

  return (
    <Box pt={8} pb={8} {...dropProps}>
      <FileDropOverlay visible={isDragging} hint={t("corruptFile.dropHint")} />
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
              <ShieldAlert size={24} />
              <Text>{t("corruptFile.title")}</Text>
            </HStack>
          </Heading>
        </HStack>
      </HStack>

      <VStack spacing={4} align="stretch" maxW="760px">
        {/* 安全提示：明确说明不会动原文件 */}
        <LiquidGlassCard p={4}>
          <HStack align="start" spacing={2}>
            <Info size={18} color={themeColorHex} style={{ flexShrink: 0, marginTop: 2 }} />
            <Text fontSize="sm" color={subTextColor}>
              {t("corruptFile.safetyNote")}
            </Text>
          </HStack>
        </LiquidGlassCard>

        <LiquidGlassCard p={5}>
          <VStack spacing={5} align="stretch">
            {/* 源文件 */}
            <Box>
              <Text fontSize="sm" fontWeight="bold" mb={2} color={headingColor}>
                {t("corruptFile.selectInput")}
              </Text>
              <HStack spacing={3} flexWrap="wrap">
                <Button
                  leftIcon={<FileInput size={16} />}
                  onClick={handlePickInput}
                  bg={themeColorHex}
                  color="#fff"
                  _hover={{ bg: themeColorRgba(0.85) }}
                >
                  {t("corruptFile.selectInput")}
                </Button>
                {inputPath && (
                  <IconButton
                    aria-label={t("corruptFile.clear")}
                    icon={<Trash2 size={16} />}
                    size="sm"
                    variant="ghost"
                    onClick={() => {
                      setInputPath(null);
                      setResult(null);
                      setEstimate(null);
                    }}
                  />
                )}
              </HStack>
              <Text fontSize="sm" color={subTextColor} mt={2} wordBreak="break-all">
                {inputPath ?? t("corruptFile.inputHint")}
              </Text>
            </Box>

            <Divider />

            {/* 损坏规模 */}
            <Box>
              <Text fontSize="sm" fontWeight="bold" mb={2} color={headingColor}>
                {t("corruptFile.scaleTitle")}
              </Text>
              <HStack spacing={4} flexWrap="wrap">
                <HStack spacing={2}>
                  <Select
                    width="150px"
                    value={sizeMode}
                    onChange={(e) => setSizeMode(e.target.value as "ratio" | "bytes")}
                    size="sm"
                  >
                    <option value="ratio">{t("corruptFile.byRatio")}</option>
                    <option value="bytes">{t("corruptFile.byBytes")}</option>
                  </Select>
                </HStack>
                {sizeMode === "ratio" ? (
                  <NumberInput
                    size="sm"
                    width="150px"
                    min={0.01}
                    max={100}
                    step={0.5}
                    precision={2}
                    value={ratioPercent}
                    onChange={(_, v) => setRatioPercent(Number.isFinite(v) ? v : 0.01)}
                  >
                    <NumberInputField />
                    <NumberInputStepper>
                      <NumberIncrementStepper />
                      <NumberDecrementStepper />
                    </NumberInputStepper>
                  </NumberInput>
                ) : (
                  <NumberInput
                    size="sm"
                    width="170px"
                    min={1}
                    value={byteCount}
                    onChange={(_, v) => setByteCount(Number.isFinite(v) ? v : 1)}
                  >
                    <NumberInputField />
                    <NumberInputStepper>
                      <NumberIncrementStepper />
                      <NumberDecrementStepper />
                    </NumberInputStepper>
                  </NumberInput>
                )}
                <Text fontSize="sm" color={subTextColor}>
                  {sizeMode === "ratio" ? "%" : t("corruptFile.bytesUnit")}
                </Text>
              </HStack>
              {estimateText && (
                <Text fontSize="xs" color={subTextColor} mt={2}>
                  {estimateText}
                </Text>
              )}
            </Box>

            {/* 损坏范围 */}
            <Box>
              <Text fontSize="sm" fontWeight="bold" mb={2} color={headingColor}>
                {t("corruptFile.rangeTitle")}
              </Text>
              <HStack spacing={3} flexWrap="wrap">
                <Input
                  size="sm"
                  width="150px"
                  placeholder={t("corruptFile.startPlaceholder")}
                  value={rangeStart}
                  onChange={(e) => setRangeStart(e.target.value.replace(/[^\d]/g, ""))}
                />
                <Text fontSize="sm" color={subTextColor}>~</Text>
                <Input
                  size="sm"
                  width="150px"
                  placeholder={t("corruptFile.endPlaceholder")}
                  value={rangeEnd}
                  onChange={(e) => setRangeEnd(e.target.value.replace(/[^\d]/g, ""))}
                />
              </HStack>
              <Text fontSize="xs" color={subTextColor} mt={1}>
                {t("corruptFile.rangeHint")}
              </Text>
            </Box>

            {/* 损坏方式 + 种子 */}
            <HStack spacing={6} flexWrap="wrap" align="flex-start">
              <Box>
                <Text fontSize="sm" fontWeight="bold" mb={2} color={headingColor}>
                  {t("corruptFile.modeTitle")}
                </Text>
                <Select
                  size="sm"
                  width="220px"
                  value={mode}
                  onChange={(e) => setMode(e.target.value as CorruptMode)}
                >
                  <option value="random">{t("corruptFile.modeRandom")}</option>
                  <option value="zero">{t("corruptFile.modeZero")}</option>
                  <option value="ones">{t("corruptFile.modeOnes")}</option>
                </Select>
              </Box>
              <Box>
                <HStack mb={2} spacing={2}>
                  <Dices size={15} color={themeColorHex} />
                  <Text fontSize="sm" fontWeight="bold" color={headingColor}>
                    {t("corruptFile.seedTitle")}
                  </Text>
                </HStack>
                <Tooltip label={t("corruptFile.seedHint")} placement="top" hasArrow>
                  <Input
                    size="sm"
                    width="220px"
                    placeholder={t("corruptFile.seedPlaceholder")}
                    value={seedText}
                    onChange={(e) => setSeedText(e.target.value.replace(/[^\d]/g, ""))}
                  />
                </Tooltip>
              </Box>
            </HStack>

            {/* 保持大小 */}
            <HStack spacing={3}>
              <Switch
                size="sm"
                isChecked={keepSize}
                onChange={(e) => setKeepSize(e.target.checked)}
              />
              <Text fontSize="sm" color={subTextColor}>
                {t("corruptFile.keepSize")}
              </Text>
            </HStack>

            <Divider />

            {/* 执行 */}
            <HStack>
              <Button
                leftIcon={running ? <Spinner size="sm" /> : <RefreshCw size={16} />}
                onClick={handleRun}
                isDisabled={running || !inputPath}
                bg={themeColorHex}
                color="#fff"
                _hover={{ bg: themeColorRgba(0.85) }}
              >
                {running ? t("corruptFile.running") : t("corruptFile.run")}
              </Button>
              <Text fontSize="xs" color={warnColor}>
                {t("corruptFile.irreversibleHint")}
              </Text>
            </HStack>
          </VStack>
        </LiquidGlassCard>

        {/* 结果报告：用于对照修复工具的表现，也便于复现 */}
        {result && (
          <LiquidGlassCard p={5}>
            <VStack align="stretch" spacing={3}>
              <HStack>
                <Hash size={16} color={themeColorHex} />
                <Text fontSize="md" fontWeight="semibold" color={headingColor}>
                  {t("corruptFile.reportTitle")}
                </Text>
                <Badge colorScheme="green" fontSize="2xs">
                  {t("corruptFile.done")}
                </Badge>
              </HStack>
              <Box fontSize="sm" color={subTextColor}>
                <Text wordBreak="break-all">
                  <b>{t("corruptFile.outputPath")}:</b> {result.output_path}
                </Text>
                <Text>
                  <b>{t("corruptFile.fileSize")}:</b> {formatBytes(result.original_size)}
                  {" → "}
                  {formatBytes(result.output_size)}
                </Text>
                <Text>
                  <b>{t("corruptFile.corruptedBytes")}:</b>{" "}
                  {result.corrupted_bytes.toLocaleString()} ({result.percent.toFixed(2)}%)
                </Text>
                <Text>
                  <b>{t("corruptFile.damagedBlocks")}:</b> {result.damaged_blocks.toLocaleString()}
                </Text>
                <Text>
                  <b>{t("corruptFile.usedRange")}:</b>{" "}
                  {result.range_start.toLocaleString()} ~ {result.range_end.toLocaleString()}
                </Text>
                <Text>
                  <b>{t("corruptFile.usedSeed")}:</b> {result.seed}
                </Text>
                <Text mt={2}>{result.message}</Text>
              </Box>
              <HStack>
                <Button
                  size="sm"
                  variant="outline"
                  leftIcon={<Save size={14} />}
                  onClick={async () => {
                    try {
                      const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
                      await revealItemInDir(result.output_path);
                    } catch (e) {
                      toast({ title: String(e), status: "error", duration: 3000 });
                    }
                  }}
                >
                  {t("corruptFile.reveal")}
                </Button>
              </HStack>
            </VStack>
          </LiquidGlassCard>
        )}
      </VStack>
    </Box>
  );
}
