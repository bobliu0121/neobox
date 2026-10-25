import { useState, useCallback, useEffect } from "react";
import {
  Box,
  Text,
  Heading,
  VStack,
  HStack,
  IconButton,
  Button,
  Input,
  Textarea,
  Badge,
  Spinner,
  Progress,
  Select,
  Checkbox,
  Image,
  Tooltip,
  useColorModeValue,
  useToast,
  Alert,
  AlertIcon,
} from "@chakra-ui/react";
import { useNavigate } from "react-router-dom";
import { ArrowLeft, Download, Link2, Film, Music, X, Square } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { LiquidGlassCard } from "@/components/special/liquid-glass-card";
import { useThemeColor } from "@/contexts/theme-color-context";
import { useAdaptiveTextColor } from "@/hooks/use-adaptive-text-color";
import { hexToRgba } from "@/lib/color-utils";

/* ────────────────────────── 类型（与后端 serde 对齐）────────────────────────── */

interface BiliPage {
  page: number;
  cid: number;
  part: string;
  duration: number;
}

interface BiliVideoInfo {
  bvid: string;
  aid: number;
  title: string;
  cover: string;
  duration: number;
  owner: string;
  desc: string;
  pages: BiliPage[];
}

interface BiliQuality {
  qn: number;
  label: string;
  width: number;
  height: number;
}

interface BiliPlayInfo {
  qualities: BiliQuality[];
  has_audio: boolean;
}

interface BiliProgress {
  page: number;
  total_pages: number;
  percent: number;
  downloaded: number;
  total: number;
  stage: string;
}

interface BiliItemResult {
  index: number;
  page: number;
  ok: boolean;
  path?: string;
  quality?: string;
  error?: string;
}

/** 解析后的条目：视频信息 + 该视频当前选择的状态 */
interface ParsedEntry {
  info: BiliVideoInfo;
  /** 已解析出的清晰度（按分P缓存：cid -> 清晰度列表） */
  qualitiesByCid: Record<number, BiliQuality[]>;
  /** 选中的分P（cid 集合） */
  selectedCids: number[];
  /** 选中的清晰度 qn（按 cid） */
  qnByCid: Record<number, number>;
  /** 展开/折叠 */
  expanded: boolean;
  error?: string;
}

const VIDEO_EXTS_AUDIO = "m4a";
const VIDEO_EXT = "mp4";

function fmtDuration(sec: number): string {
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  const s = sec % 60;
  return h > 0
    ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`
    : `${m}:${String(s).padStart(2, "0")}`;
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

/** 去掉 Windows 文件名非法字符 */
function sanitize(name: string): string {
  return name.replace(/[\\/:*?"<>|]/g, "_").slice(0, 120);
}

export default function BilibiliDownloadPage() {
  const navigate = useNavigate();
  const toast = useToast();
  const { getActiveColor, config: themeConfig } = useThemeColor();
  const themeColorHex = themeConfig.primaryColor;
  const themeColorRgba = (opacity: number) => hexToRgba(themeColorHex, opacity);
  const adaptiveTitle = useAdaptiveTextColor();
  const headingColor = useColorModeValue("gray.800", "#ffffff");
  const subTextColor = useColorModeValue("gray.500", "#ffffff");
  const inputBg = useColorModeValue("white", "#1a1a1a");

  const [text, setText] = useState("");
  const [parsing, setParsing] = useState(false);
  const [entries, setEntries] = useState<ParsedEntry[]>([]);

  const [outputDir, setOutputDir] = useState<string>("");
  const [audioOnly, setAudioOnly] = useState(false);

  const [downloading, setDownloading] = useState(false);
  const [progress, setProgress] = useState<BiliProgress | null>(null);
  const [results, setResults] = useState<BiliItemResult[]>([]);

  /* ────────── 进度事件订阅 ────────── */
  useEffect(() => {
    let disposed = false;
    let unlistenProgress: UnlistenFn | undefined;
    let unlistenItem: UnlistenFn | undefined;
    (async () => {
      try {
        const p = await listen<BiliProgress>("bilibili:progress", (e) => {
          if (!disposed) setProgress(e.payload);
        });
        if (disposed) p();
        else unlistenProgress = p;
        const it = await listen<BiliItemResult>("bilibili:item", (e) => {
          if (!disposed) setResults((prev) => [...prev, e.payload]);
        });
        if (disposed) it();
        else unlistenItem = it;
      } catch (e) {
        console.error("订阅下载进度失败:", e);
      }
    })();
    return () => {
      disposed = true;
      unlistenProgress?.();
      unlistenItem?.();
    };
  }, []);

  /** 解析文本里所有链接/BV 号 */
  const handleParse = useCallback(async () => {
    const lines = text
      .split(/[\s,;]+/)
      .map((s) => s.trim())
      .filter(Boolean);
    if (lines.length === 0) {
      toast({ title: "请先粘贴视频链接或 BV 号", status: "warning", duration: 2500 });
      return;
    }
    setParsing(true);
    const next: ParsedEntry[] = [];
    for (const raw of lines) {
      try {
        const info = await invoke<BiliVideoInfo>("bilibili_resolve", { input: raw });
        // 同名视频去重
        if (next.some((x) => x.info.bvid === info.bvid)) continue;
        next.push({
          info,
          qualitiesByCid: {},
          selectedCids: info.pages.length === 1 ? [info.pages[0].cid] : info.pages.map((p) => p.cid),
          qnByCid: {},
          expanded: false,
        });
      } catch (e) {
        next.push({
          info: {
            bvid: "",
            aid: 0,
            title: raw,
            cover: "",
            duration: 0,
            owner: "",
            desc: "",
            pages: [],
          },
          qualitiesByCid: {},
          selectedCids: [],
          qnByCid: {},
          expanded: false,
          error: String(e),
        });
      }
    }
    setEntries(next);
    setParsing(false);
    setResults([]);
    setProgress(null);
  }, [text, toast]);

  /** 展开某视频时按需拉取每个分P 的清晰度 */
  const loadQualities = useCallback(
    async (index: number) => {
      const entry = entries[index];
      if (!entry || entry.info.pages.length === 0) return;
      const updates: Record<number, BiliQuality[]> = { ...entry.qualitiesByCid };
      for (const p of entry.info.pages) {
        if (updates[p.cid]) continue;
        try {
          const info = await invoke<BiliPlayInfo>("bilibili_qualities", {
            bvid: entry.info.bvid,
            cid: p.cid,
          });
          updates[p.cid] = info.qualities;
        } catch {
          updates[p.cid] = [];
        }
      }
      setEntries((prev) =>
        prev.map((e, i) => {
          if (i !== index) return e;
          // 默认选最高清晰度
          const qnByCid = { ...e.qnByCid };
          for (const [cid, list] of Object.entries(updates)) {
            const c = Number(cid);
            if (!qnByCid[c] && list.length > 0) {
              qnByCid[c] = list[list.length - 1].qn;
            }
          }
          return { ...e, qualitiesByCid: updates, qnByCid };
        }),
      );
    },
    [entries],
  );

  /** 选择输出目录 */
  const handlePickDir = useCallback(async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const dir = await open({ directory: true, multiple: false, title: "选择保存目录" });
      if (typeof dir === "string") setOutputDir(dir);
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 3000 });
    }
  }, [toast]);

  /** 开始下载 */
  const handleDownload = useCallback(async () => {
    if (entries.length === 0) {
      toast({ title: "请先解析链接", status: "warning", duration: 2500 });
      return;
    }
    // 组装请求：每个被选中的分P 一项
    const requests: {
      bvid: string;
      cid: number;
      qn: number | null;
      audio_only: boolean;
      output_path: string;
      page: number;
    }[] = [];
    const dir = outputDir || "";
    for (const entry of entries) {
      if (entry.error || entry.info.pages.length === 0) continue;
      for (const p of entry.info.pages) {
        if (!entry.selectedCids.includes(p.cid)) continue;
        const qn = entry.qnByCid[p.cid] ?? null;
        const ext = audioOnly ? VIDEO_EXTS_AUDIO : VIDEO_EXT;
        // 多P视频文件名带上分P 序号，避免同名覆盖
        const multi = entry.info.pages.length > 1;
        const base = sanitize(entry.info.title) + (multi ? `_P${p.page}` : "");
        const path = dir ? `${dir}\\${base}.${ext}` : `${base}.${ext}`;
        requests.push({
          bvid: entry.info.bvid,
          cid: p.cid,
          qn,
          audio_only: audioOnly,
          output_path: path,
          page: p.page,
        });
      }
    }
    if (requests.length === 0) {
      toast({ title: "没有选中任何分P", status: "warning", duration: 2500 });
      return;
    }
    if (!dir) {
      toast({ title: "请先选择保存目录", status: "warning", duration: 2500 });
      return;
    }
    setDownloading(true);
    setResults([]);
    setProgress(null);
    try {
      await invoke("bilibili_download", { requests });
      toast({ title: "下载任务结束", status: "success", duration: 3000 });
    } catch (e) {
      toast({ title: `下载失败: ${e}`, status: "error", duration: 5000 });
    } finally {
      setDownloading(false);
    }
  }, [entries, outputDir, audioOnly, toast]);

  const handleCancel = useCallback(async () => {
    try {
      await invoke("bilibili_cancel");
      toast({ title: "已请求取消", status: "info", duration: 2000 });
    } catch (e) {
      console.error(e);
    }
  }, [toast]);

  const removeEntry = (idx: number) =>
    setEntries((prev) => prev.filter((_, i) => i !== idx));

  const totalSelected = entries.reduce(
    (n, e) => n + e.selectedCids.length,
    0,
  );

  return (
    <Box pt={8} pb={8}>
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
              <Film size={24} />
              <Text>B 站视频/音频下载</Text>
            </HStack>
          </Heading>
        </HStack>
      </HStack>

      <VStack spacing={4} align="stretch" maxW="860px">
        {/* 免登录说明 */}
        <Alert status="info" borderRadius="lg" bg={themeColorRgba(0.08)}>
          <AlertIcon color={themeColorHex} />
          <Box fontSize="sm">
            无需登录 B 站。未登录时 B 站只对第三方下发到
            <Text as="span" fontWeight="bold"> 480P </Text>
            清晰度（实测多个视频均如此，高清晰度需要账号），本页面只展示实际可下载的清晰度。
          </Box>
        </Alert>

        {/* 输入区 */}
        <LiquidGlassCard p={5}>
          <VStack spacing={3} align="stretch">
            <HStack justify="space-between">
              <Text fontSize="sm" fontWeight="bold" color={headingColor}>
                视频链接 / BV 号（每行一个，支持批量）
              </Text>
              <Text fontSize="xs" color={subTextColor}>
                支持 BV 号、av 号、完整链接、b23.tv 短链
              </Text>
            </HStack>
            <Textarea
              value={text}
              onChange={(e) => setText(e.target.value)}
              placeholder={"https://www.bilibili.com/video/BV1GJ411x7h7\nBV1xx411c7mD"}
              rows={4}
              bg={inputBg}
              fontFamily="monospace"
              fontSize="sm"
              isDisabled={parsing || downloading}
            />
            <HStack spacing={3}>
              <Button
                leftIcon={parsing ? <Spinner size="sm" /> : <Link2 size={16} />}
                onClick={handleParse}
                isDisabled={parsing || downloading}
                bg={themeColorHex}
                color="#fff"
                _hover={{ bg: themeColorRgba(0.85) }}
              >
                {parsing ? "解析中..." : "解析"}
              </Button>
              <Checkbox
                isChecked={audioOnly}
                onChange={(e) => setAudioOnly(e.target.checked)}
                isDisabled={downloading}
              >
                <HStack spacing={1}>
                  <Music size={14} />
                  <Text fontSize="sm">仅下载音频（.m4a）</Text>
                </HStack>
              </Checkbox>
            </HStack>
          </VStack>
        </LiquidGlassCard>

        {/* 解析结果 */}
        {entries.length > 0 && (
          <LiquidGlassCard p={5}>
            <VStack spacing={3} align="stretch">
              <Text fontSize="sm" fontWeight="bold" color={headingColor}>
                解析结果（{entries.filter((e) => !e.error).length} 个有效）
              </Text>
              {entries.map((entry, idx) => (
                <Box
                  key={`${entry.info.bvid}-${idx}`}
                  border="1px solid"
                  borderColor={themeColorRgba(0.25)}
                  borderRadius="lg"
                  p={3}
                >
                  {entry.error ? (
                    <HStack justify="space-between">
                      <Text fontSize="sm" color="#e53e3e" wordBreak="break-all">
                        {entry.info.title}：{entry.error}
                      </Text>
                      <IconButton
                        aria-label="移除"
                        icon={<X size={14} />}
                        size="xs"
                        variant="ghost"
                        onClick={() => removeEntry(idx)}
                      />
                    </HStack>
                  ) : (
                    <VStack align="stretch" spacing={2}>
                      <HStack justify="space-between" align="flex-start">
                        <HStack spacing={3} align="flex-start">
                          {entry.info.cover && (
                            <Image
                              src={entry.info.cover}
                              alt=""
                              boxSize="72px"
                              objectFit="cover"
                              borderRadius="md"
                              referrerPolicy="no-referrer"
                              flexShrink={0}
                            />
                          )}
                          <Box minW={0}>
                            <Text fontSize="sm" fontWeight="bold" color={headingColor} noOfLines={2}>
                              {entry.info.title}
                            </Text>
                            <Text fontSize="xs" color={subTextColor}>
                              {entry.info.owner} · {fmtDuration(entry.info.duration)} ·{" "}
                              {entry.info.pages.length} 个分P
                            </Text>
                          </Box>
                        </HStack>
                        <HStack spacing={1}>
                          <Button
                            size="xs"
                            variant="ghost"
                            onClick={() => {
                              const willExpand = !entry.expanded;
                              setEntries((prev) =>
                                prev.map((e, i) =>
                                  i === idx ? { ...e, expanded: willExpand } : e,
                                ),
                              );
                              if (willExpand) void loadQualities(idx);
                            }}
                          >
                            {entry.expanded ? "收起" : "选择分P/清晰度"}
                          </Button>
                          <IconButton
                            aria-label="移除"
                            icon={<X size={14} />}
                            size="xs"
                            variant="ghost"
                            onClick={() => removeEntry(idx)}
                          />
                        </HStack>
                      </HStack>

                      {entry.expanded && (
                        <VStack align="stretch" spacing={1} mt={1}>
                          {entry.info.pages.map((p) => {
                            const qs = entry.qualitiesByCid[p.cid];
                            const checked = entry.selectedCids.includes(p.cid);
                            return (
                              <HStack
                                key={p.cid}
                                spacing={2}
                                px={2}
                                py={1}
                                borderRadius="md"
                                bg={checked ? themeColorRgba(0.06) : undefined}
                              >
                                <Checkbox
                                  isChecked={checked}
                                  onChange={(e) => {
                                    setEntries((prev) =>
                                      prev.map((en, i) => {
                                        if (i !== idx) return en;
                                        const sel = e.target.checked
                                          ? [...en.selectedCids, p.cid]
                                          : en.selectedCids.filter((c) => c !== p.cid);
                                        return { ...en, selectedCids: sel };
                                      }),
                                    );
                                  }}
                                />
                                <Text fontSize="xs" minW="48px" color={subTextColor}>
                                  {entry.info.pages.length > 1 ? `P${p.page}` : "单P"}
                                </Text>
                                <Text fontSize="xs" flex={1} noOfLines={1} color={headingColor}>
                                  {p.part}
                                </Text>
                                <Text fontSize="xs" color={subTextColor} minW="52px">
                                  {fmtDuration(p.duration)}
                                </Text>
                                {qs === undefined ? (
                                  <Spinner size="xs" />
                                ) : qs.length === 0 ? (
                                  <Badge colorScheme="red" fontSize="2xs">
                                    无可用清晰度
                                  </Badge>
                                ) : audioOnly ? (
                                  <Badge colorScheme="purple" fontSize="2xs">
                                    仅音频
                                  </Badge>
                                ) : (
                                  <Select
                                    size="xs"
                                    width="150px"
                                    value={entry.qnByCid[p.cid] ?? qs[qs.length - 1].qn}
                                    onChange={(e) => {
                                      const qn = Number(e.target.value);
                                      setEntries((prev) =>
                                        prev.map((en, i) =>
                                          i === idx
                                            ? {
                                                ...en,
                                                qnByCid: { ...en.qnByCid, [p.cid]: qn },
                                              }
                                            : en,
                                        ),
                                      );
                                    }}
                                  >
                                    {qs.map((q) => (
                                      <option key={q.qn} value={q.qn}>
                                        {q.label} ({q.width}×{q.height})
                                      </option>
                                    ))}
                                  </Select>
                                )}
                              </HStack>
                            );
                          })}
                        </VStack>
                      )}
                    </VStack>
                  )}
                </Box>
              ))}
            </VStack>
          </LiquidGlassCard>
        )}

        {/* 下载设置与操作 */}
        <LiquidGlassCard p={5}>
          <VStack spacing={3} align="stretch">
            <HStack spacing={3} flexWrap="wrap">
              <Button leftIcon={<Download size={16} />} onClick={handlePickDir} variant="outline">
                选择保存目录
              </Button>
              <Text fontSize="sm" color={subTextColor} isTruncated maxW="480px">
                {outputDir || "未选择（必须选择后才能下载）"}
              </Text>
            </HStack>

            <HStack spacing={3} flexWrap="wrap">
              <Button
                leftIcon={downloading ? <Spinner size="sm" /> : <Download size={16} />}
                onClick={handleDownload}
                isDisabled={downloading || totalSelected === 0}
                bg={themeColorHex}
                color="#fff"
                _hover={{ bg: themeColorRgba(0.85) }}
                size="lg"
              >
                {downloading ? "下载中..." : `开始下载（${totalSelected} 项）`}
              </Button>
              {downloading && (
                <Button
                  leftIcon={<Square size={14} />}
                  onClick={handleCancel}
                  variant="outline"
                  colorScheme="red"
                >
                  停止
                </Button>
              )}
            </HStack>

            {/* 进度 */}
            {progress && (
              <Box>
                <HStack justify="space-between" mb={1}>
                  <Text fontSize="xs" color={subTextColor}>
                    {progress.stage} · 第 {progress.page}/{progress.total_pages} 项
                  </Text>
                  <Text fontSize="xs" color={subTextColor}>
                    {progress.downloaded > 0 && fmtBytes(progress.downloaded)}
                  </Text>
                </HStack>
                <Progress
                  value={progress.percent}
                  size="sm"
                  borderRadius="full"
                  colorScheme="purple"
                />
              </Box>
            )}

            {/* 逐项结果 */}
            {results.length > 0 && (
              <VStack align="stretch" spacing={1} maxH="200px" overflowY="auto">
                {results.map((r) => (
                  <HStack key={`${r.index}-${r.page}`} spacing={2} fontSize="xs">
                    <Text minW="48px" color={subTextColor}>
                      P{r.page}
                    </Text>
                    {r.ok ? (
                      <>
                        <Badge colorScheme="green" fontSize="2xs">
                          完成
                        </Badge>
                        {r.quality && (
                          <Badge colorScheme="purple" fontSize="2xs">
                            {r.quality}
                          </Badge>
                        )}
                        <Text color={subTextColor} flex={1} noOfLines={1}>
                          {r.path}
                        </Text>
                      </>
                    ) : (
                      <>
                        <Badge colorScheme="red" fontSize="2xs">
                          失败
                        </Badge>
                        <Text color="#e53e3e" flex={1} noOfLines={2}>
                          {r.error}
                        </Text>
                      </>
                    )}
                  </HStack>
                ))}
              </VStack>
            )}
          </VStack>
        </LiquidGlassCard>
      </VStack>
    </Box>
  );
}
