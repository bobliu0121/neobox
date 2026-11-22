import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactElement,
} from "react";
import {
  Badge,
  Box,
  Button,
  Checkbox,
  Divider,
  FormControl,
  FormHelperText,
  FormLabel,
  Grid,
  GridItem,
  Heading,
  HStack,
  IconButton,
  Image,
  Input,
  Tab,
  TabList,
  TabPanel,
  TabPanels,
  Tabs,
  Text,
  Textarea,
  VStack,
  Wrap,
  WrapItem,
  useColorModeValue,
  useToast,
} from "@chakra-ui/react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import {
  ArrowLeft,
  AudioLines,
  ChevronLeft,
  ChevronRight,
  Clock3,
  Disc3,
  FileAudio,
  FileText,
  FolderOpen,
  Image as ImageIcon,
  Info,
  Music2,
  Plus,
  Save,
  Sparkles,
  Trash2,
  Type,
  X,
} from "lucide-react";
import { LiquidGlassCard } from "@/components/special/liquid-glass-card";
import { useThemeColor } from "@/contexts/theme-color-context";
import { useAdaptiveTextColor } from "@/hooks/use-adaptive-text-color";
import { useFileDrop } from "@/hooks/use-file-drop";
import { FileDropOverlay } from "@/components/ui/file-drop-overlay";
import { hexToRgba } from "@/lib/color-utils";

// ─────────────────────────── 类型 ───────────────────────────

type ChannelProgram = {
  channels: number[];
  language?: string;
  title?: string;
  role?: string;
};

type AudioBinding = {
  trackUid: string;
  channels?: number[] | null;
  labels?: string[] | null;
};

type Translation = { lang: string; text: string };

type PronunciationSegment = {
  source: string;
  reading: string;
  startMs?: number;
  endMs?: number;
};

type Pronunciation = {
  scheme: string;
  lang?: string;
  text: string;
  segments?: PronunciationSegment[];
};

type Cue = {
  id?: string;
  startMs: number;
  endMs: number;
  text: string;
  chars?: Array<Record<string, unknown>>;
  pronunciations?: Pronunciation[];
  translations?: Translation[];
  style?: Record<string, unknown> | null;
};

type AudioTrack = {
  path: string;
  uid?: string;
  codec?: string;
  sampleRate?: number;
  channels: number;
  language: string;
  title: string;
  default: boolean;
  channelPrograms: ChannelProgram[];
};

type SubtitleTrack = {
  uid?: string;
  language: string;
  roles: string[];
  audioBindings: AudioBinding[];
  pronunciationScheme?: string | null;
  sourceSubtitleUid?: string | null;
  title: string;
  default: boolean;
  forced: boolean;
  offsetMs?: number | null;
  cues: Cue[];
};

type FontTrack = {
  path: string;
  family: string;
  style?: string | null;
  weight?: number | null;
  license?: string | null;
  subset?: string | null;
  fileName?: string;
  mime?: string;
};

type ClimaxMarker = {
  id?: string;
  trackUid?: string | null;
  startMs: number;
  endMs?: number | null;
  peakMs?: number | null;
  kind?: string | null;
  label?: string | null;
  strength?: number | null;
  source?: string | null;
  confidence?: number | null;
};

type Project = {
  sourcePath: string;
  spec: string;
  id: string;
  title: string;
  artist: string;
  album: string;
  durationMs: number;
  coverPath: string | null;
  audioTracks: AudioTrack[];
  subtitleTracks: SubtitleTrack[];
  fonts: FontTrack[];
  climaxes: ClimaxMarker[];
};

type MuxResult = {
  outputPath: string;
  durationMs: number;
  audioTrackCount: number;
  subtitleTrackCount: number;
  hasCover: boolean;
  fontCount: number;
  size: number;
};

// ─────────────────────────── 工具 ───────────────────────────

const AUDIO_EXT_RE = /\.(mp3|wav|ogg|m4a|mp4|flac|aac|opus|wma|aiff|ape|oga|mka|nbmusic)$/i;
const SUBTITLE_EXT_RE = /\.(srt|vtt|ass|ssa)$/i;
const IMAGE_EXT_RE = /\.(jpe?g|png|webp|gif)$/i;
const FONT_EXT_RE = /\.(ttf|otf|woff|woff2)$/i;

const ROLE_OPTIONS = ["original", "translation", "karaoke", "pronunciation"];
const SCHEME_OPTIONS = ["pinyin", "zhuyin", "romaji", "ipa", "kana", "zh-phonetic"];

function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() || path;
}

function stem(path: string): string {
  return baseName(path).replace(/\.[^.]+$/, "");
}

function emptyProject(): Project {
  return {
    sourcePath: "",
    spec: "1.0",
    id: "",
    title: "",
    artist: "",
    album: "",
    durationMs: 0,
    coverPath: null,
    audioTracks: [],
    subtitleTracks: [],
    fonts: [],
    climaxes: [],
  };
}

function nextUid(project: Project): string {
  const numeric = [
    ...project.audioTracks.map((track) => Number(track.uid)),
    ...project.subtitleTracks.map((track) => Number(track.uid)),
  ].filter((value) => Number.isFinite(value) && value > 0);
  return String(Math.max(0, ...numeric) + 1);
}

function createAudio(path: string, uid: string): AudioTrack {
  return {
    path,
    uid,
    codec: "",
    sampleRate: 0,
    channels: 2,
    language: "und",
    title: stem(path),
    default: false,
    channelPrograms: [],
  };
}

function createSubtitle(uid: string | number, audio: AudioTrack[]): SubtitleTrack {
  return {
    uid: String(uid),
    language: "und",
    roles: ["original"],
    audioBindings: audio
      .map((track) => track.uid)
      .filter((trackUid): trackUid is string => !!trackUid)
      .map((trackUid) => ({ trackUid, channels: null, labels: null })),
    pronunciationScheme: null,
    sourceSubtitleUid: null,
    title: "字幕 1",
    default: false,
    forced: false,
    offsetMs: 0,
    cues: [],
  };
}

function formatDuration(ms: number): string {
  if (!ms) return "--";
  const totalSeconds = Math.floor(ms / 1000);
  const h = Math.floor(totalSeconds / 3600);
  const m = Math.floor((totalSeconds % 3600) / 60);
  const s = totalSeconds % 60;
  return h > 0
    ? `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`
    : `${m}:${String(s).padStart(2, "0")}`;
}

function formatCueTime(ms: number): string {
  const safe = Math.max(0, Math.round(ms));
  const m = Math.floor(safe / 60000);
  const s = Math.floor((safe % 60000) / 1000);
  const milli = safe % 1000;
  return `${m}:${String(s).padStart(2, "0")}.${String(milli).padStart(3, "0")}`;
}

function parseCueTime(value: string): number | null {
  const raw = value.trim().replace(",", ".");
  if (!raw) return null;
  if (/^\d+(\.\d+)?$/.test(raw)) {
    const n = Number(raw);
    return Number.isFinite(n) ? Math.max(0, Math.round(n)) : null;
  }
  const parts = raw.split(":");
  let h = 0;
  let m = 0;
  let s = 0;
  if (parts.length === 3) {
    h = Number(parts[0]);
    m = Number(parts[1]);
    s = Number(parts[2]);
  } else if (parts.length === 2) {
    m = Number(parts[0]);
    s = Number(parts[1]);
  } else {
    return null;
  }
  if (![h, m, s].every(Number.isFinite)) return null;
  return Math.max(0, Math.round((h * 3600 + m * 60 + s) * 1000));
}

function formatBytes(bytes: number): string {
  if (!bytes) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function channelName(total: number, index: number): string {
  if (total === 2) return index === 1 ? "L" : "R";
  if (total === 6) {
    return ["FL", "FR", "C", "LFE", "SL", "SR"][index - 1] ?? `CH${index}`;
  }
  return `CH${index}`;
}

function defaultOutput(project: Project): string {
  if (project.sourcePath) {
    return project.sourcePath.replace(/\.nbmusic$/i, "") + ".edited.nbmusic";
  }
  const first = project.audioTracks[0]?.path;
  return first ? first.replace(/\.[^.\\/]+$/, "") + ".nbmusic" : "";
}

function makeCue(startMs = 0, endMs = 3000): Cue {
  return {
    id: `c${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`,
    startMs,
    endMs,
    text: "",
    chars: [],
    pronunciations: [],
    translations: [],
    style: null,
  };
}

// ─────────────────────────── 小组件 ───────────────────────────

function TimeInput({
  value,
  onChange,
  ariaLabel,
  width = "118px",
}: {
  value: number;
  onChange: (value: number) => void;
  ariaLabel: string;
  width?: string;
}) {
  const [text, setText] = useState(formatCueTime(value));
  useEffect(() => {
    setText(formatCueTime(value));
  }, [value]);
  return (
    <Input
      aria-label={ariaLabel}
      size="sm"
      w={width}
      value={text}
      fontFamily="mono"
      onChange={(event) => setText(event.target.value)}
      onBlur={() => {
        const parsed = parseCueTime(text);
        if (parsed == null) setText(formatCueTime(value));
        else onChange(parsed);
      }}
    />
  );
}

function JsonEditor({
  label,
  value,
  onChange,
}: {
  label: string;
  value: unknown;
  onChange: (value: unknown) => void;
}) {
  const initial = useMemo(() => {
    try {
      return JSON.stringify(value ?? null, null, 2);
    } catch {
      return "null";
    }
  }, [value]);
  const [text, setText] = useState(initial);
  const [invalid, setInvalid] = useState(false);
  useEffect(() => {
    setText(initial);
    setInvalid(false);
  }, [initial]);
  return (
    <FormControl>
      <FormLabel fontSize="xs" mb={1}>
        {label}
      </FormLabel>
      <Textarea
        value={text}
        onChange={(event) => {
          setText(event.target.value);
          setInvalid(false);
        }}
        onBlur={() => {
          try {
            onChange(text.trim() ? JSON.parse(text) : null);
            setInvalid(false);
          } catch {
            setInvalid(true);
          }
        }}
        minH="90px"
        fontSize="xs"
        fontFamily="mono"
        borderColor={invalid ? "red.400" : undefined}
      />
      <FormHelperText fontSize="2xs">
        {invalid ? "JSON 无效，修改未生效" : "失焦后应用，结构按 .nbmusic v1.0 规范"}
      </FormHelperText>
    </FormControl>
  );
}

function TagInput({
  values,
  onChange,
  suggestions,
}: {
  values: string[];
  onChange: (values: string[]) => void;
  suggestions: string[];
}) {
  const [draft, setDraft] = useState(values.join(", "));
  useEffect(() => {
    setDraft(values.join(", "));
  }, [values]);
  const commitDraft = () => {
    onChange(
      draft
        .split(",")
        .map((value) => value.trim())
        .filter(Boolean),
    );
  };
  return (
    <VStack align="stretch" spacing={2}>
      <Wrap spacing={1}>
        {suggestions.map((item) => {
          const active = values.includes(item);
          return (
            <WrapItem key={item}>
              <Button
                size="xs"
                variant={active ? "solid" : "outline"}
                onClick={() =>
                  onChange(
                    active ? values.filter((value) => value !== item) : [...values, item],
                  )
                }
              >
                {item}
              </Button>
            </WrapItem>
          );
        })}
      </Wrap>
      <Input
        size="xs"
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commitDraft}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            commitDraft();
            event.currentTarget.blur();
          }
        }}
        placeholder="自定义角色，用逗号分隔"
      />
    </VStack>
  );
}

function SectionTitle({ icon, title, description }: { icon: ReactElement; title: string; description?: string }) {
  return (
    <HStack spacing={2} mb={3}>
      {icon}
      <Box>
        <Text fontSize="sm" fontWeight="bold">
          {title}
        </Text>
        {description && (
          <Text fontSize="2xs" opacity={0.66}>
            {description}
          </Text>
        )}
      </Box>
    </HStack>
  );
}

function EmptyHint({ text }: { text: string }) {
  return (
    <Box py={6} px={3} borderRadius="lg" border="1px dashed" borderColor="whiteAlpha.300" textAlign="center">
      <Text fontSize="xs" opacity={0.6}>
        {text}
      </Text>
    </Box>
  );
}

// ─────────────────────────── 主页面 ───────────────────────────

export default function NbmusicEditorPage() {
  const navigate = useNavigate();
  const { t } = useTranslation();
  const toast = useToast();
  const { config: themeConfig } = useThemeColor();
  const themeColorHex = themeConfig.primaryColor;
  const themeColorRgba = (opacity: number) => hexToRgba(themeColorHex, opacity);
  const adaptiveTitle = useAdaptiveTextColor();
  const headingColor = useColorModeValue("gray.800", "#ffffff");
  const subTextColor = useColorModeValue("gray.500", "#ffffff");
  const fieldBg = useColorModeValue("whiteAlpha.700", "whiteAlpha.100");

  const [project, setProject] = useState<Project>(emptyProject);
  const [outputPath, setOutputPath] = useState("");
  const [activeTab, setActiveTab] = useState(0);
  const [activeSubtitle, setActiveSubtitle] = useState(0);
  const [activeCue, setActiveCue] = useState(0);
  const [loading, setLoading] = useState(false);
  const [muxing, setMuxing] = useState(false);
  const [result, setResult] = useState<MuxResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  const activeSubtitleTrack = project.subtitleTracks[activeSubtitle];
  const activeCueData = activeSubtitleTrack?.cues[activeCue];

  const coverSrc = project.coverPath ? convertFileSrc(project.coverPath) : "";

  const patchProject = useCallback((patch: Partial<Project>) => {
    setProject((current) => ({ ...current, ...patch }));
  }, []);

  const loadProject = useCallback(
    async (path: string) => {
      setLoading(true);
      setError(null);
      try {
        const loaded = await invoke<Project>("load_nbmusic_project", { path });
        setProject(loaded);
        setOutputPath(
          loaded.sourcePath.replace(/\.nbmusic$/i, "") + ".edited.nbmusic",
        );
        setActiveSubtitle(0);
        setActiveCue(0);
        setActiveTab(0);
        setResult(null);
        toast({
          title: "已载入 .nbmusic",
          description: `${loaded.audioTracks.length} 条音频 · ${loaded.subtitleTracks.length} 条字幕轨`,
          status: "success",
          duration: 3000,
        });
      } catch (e) {
        setError(String(e));
        toast({ title: "载入失败", description: String(e), status: "error", duration: 5000 });
      } finally {
        setLoading(false);
      }
    },
    [toast],
  );

  const handleOpen = useCallback(async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        multiple: false,
        title: "打开 .nbmusic 工程",
        filters: [
          { name: "NBMusic", extensions: ["nbmusic"] },
          { name: "所有文件", extensions: ["*"] },
        ],
      });
      if (typeof selected === "string") await loadProject(selected);
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 4000 });
    }
  }, [loadProject, toast]);

  const handleNew = useCallback(() => {
    if (project.audioTracks.length > 0 && !window.confirm("新建工程会清空当前编辑内容，继续？")) {
      return;
    }
    setProject(emptyProject());
    setOutputPath("");
    setResult(null);
    setError(null);
    setActiveTab(0);
    setActiveSubtitle(0);
    setActiveCue(0);
  }, [project.audioTracks.length]);

  const addAudioPaths = useCallback(
    async (paths: string[]) => {
      if (!paths.length) return;
      // 添加音频时顺手读取容器内嵌封面（MP3 / FLAC / M4A / MP4 等），
      // 工程还没有封面就自动填入，避免用户再手动找图。
      const covers = await Promise.all(
        paths.map(async (path) => {
          try {
            return await invoke<string | null>("extract_audio_cover", { path });
          } catch {
            return null;
          }
        }),
      );
      setProject((current) => {
        let uid = Number(nextUid(current));
        const hasDefault = current.audioTracks.some((track) => track.default);
        const added = paths.map((path, index) => {
          const track = createAudio(path, String(uid));
          track.default = !hasDefault && index === 0;
          uid += 1;
          return track;
        });
        return {
          ...current,
          coverPath: current.coverPath || covers.find((cover): cover is string => !!cover) || null,
          audioTracks: [...current.audioTracks, ...added],
          title: current.title || stem(paths[0]),
        };
      });
    },
    [],
  );

  const handleAddAudio = useCallback(async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        multiple: true,
        title: "选择音频文件",
        filters: [
          {
            name: "音频文件",
            extensions: [
              "mp3", "wav", "ogg", "m4a", "mp4", "flac", "aac", "opus", "wma", "aiff", "ape", "oga", "mka",
            ],
          },
          { name: "所有文件", extensions: ["*"] },
        ],
      });
      const paths = typeof selected === "string" ? [selected] : selected ?? [];
      await addAudioPaths(paths as string[]);
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 4000 });
    }
  }, [addAudioPaths, toast]);

  const removeAudio = useCallback((uid?: string) => {
    if (!uid) return;
    setProject((current) => {
      const remaining = current.audioTracks.filter((track) => track.uid !== uid);
      if (remaining.length > 0 && !remaining.some((track) => track.default)) {
        remaining[0] = { ...remaining[0], default: true };
      }
      return {
        ...current,
        audioTracks: remaining,
        subtitleTracks: current.subtitleTracks.map((track) => ({
          ...track,
          audioBindings: track.audioBindings.filter((binding) => binding.trackUid !== uid),
        })),
      };
    });
  }, []);

  const updateAudio = useCallback((index: number, patch: Partial<AudioTrack>) => {
    setProject((current) => ({
      ...current,
      audioTracks: current.audioTracks.map((track, i) => (i === index ? { ...track, ...patch } : track)),
    }));
  }, []);

  const addSubtitleBatch = useCallback(
    (items: Array<{ cues: Cue[]; title: string; language: string }>) => {
      if (!items.length) return;
      setProject((current) => {
        let uid = Number(nextUid(current));
        const nextTracks = [...current.subtitleTracks];
        const hadDefault = current.subtitleTracks.some((track) => track.default);
        items.forEach((item, index) => {
          const track = createSubtitle(uid, current.audioTracks);
          track.cues = item.cues;
          track.title = item.title;
          track.language = item.language;
          if (track.cues.some((cue) => cue.chars && cue.chars.length > 0)) {
            track.roles = track.roles.includes("karaoke")
              ? track.roles
              : [...track.roles, "karaoke"];
          }
          track.default = !hadDefault && index === 0;
          nextTracks.push(track);
          uid += 1;
        });
        return { ...current, subtitleTracks: nextTracks };
      });
      setActiveSubtitle(project.subtitleTracks.length + items.length - 1);
      setActiveCue(0);
    },
    [project.subtitleTracks.length],
  );

  const addSubtitle = useCallback(
    (cues: Cue[] = [], title = "新字幕轨", language = "und") => {
      addSubtitleBatch([{ cues, title, language }]);
    },
    [addSubtitleBatch],
  );

  const importSubtitle = useCallback(
    async (path: string) => {
      try {
        const cues = await invoke<Cue[]>("parse_nbmusic_subtitle", { path });
        addSubtitle(cues, stem(path), "und");
        toast({
          title: "字幕已导入",
          description: `${baseName(path)} · ${cues.length} 条`,
          status: "success",
          duration: 3000,
        });
      } catch (e) {
        toast({ title: "字幕导入失败", description: String(e), status: "error", duration: 5000 });
      }
    },
    [addSubtitle, toast],
  );

  const handleImportSubtitle = useCallback(async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        multiple: false,
        title: "导入字幕",
        filters: [
          { name: "字幕文件", extensions: ["srt", "vtt", "ass", "ssa"] },
          { name: "所有文件", extensions: ["*"] },
        ],
      });
      if (typeof selected === "string") await importSubtitle(selected);
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 4000 });
    }
  }, [importSubtitle, toast]);

  const removeSubtitle = useCallback((index: number) => {
    const nextTracks = project.subtitleTracks.filter((_, i) => i !== index);
    setProject((current) => ({ ...current, subtitleTracks: nextTracks }));
    setActiveSubtitle((current) => Math.max(0, Math.min(current, nextTracks.length - 1)));
    setActiveCue(0);
  }, [project.subtitleTracks]);

  const updateSubtitle = useCallback((index: number, patch: Partial<SubtitleTrack>) => {
    setProject((current) => ({
      ...current,
      subtitleTracks: current.subtitleTracks.map((track, i) => (i === index ? { ...track, ...patch } : track)),
    }));
  }, []);

  const updateCue = useCallback(
    (index: number, patch: Partial<Cue>) => {
      setProject((current) => ({
        ...current,
        subtitleTracks: current.subtitleTracks.map((track, trackIndex) => {
          if (trackIndex !== activeSubtitle) return track;
          return {
            ...track,
            cues: track.cues.map((cue, cueIndex) => (cueIndex === index ? { ...cue, ...patch } : cue)),
          };
        }),
      }));
    },
    [activeSubtitle],
  );

  const addCue = useCallback(() => {
    if (!activeSubtitleTrack) return;
    const last = activeSubtitleTrack.cues[activeSubtitleTrack.cues.length - 1];
    const cue = makeCue(last?.endMs ?? 0, (last?.endMs ?? 0) + 3000);
    const cues = [...activeSubtitleTrack.cues, cue];
    setProject((current) => ({
      ...current,
      subtitleTracks: current.subtitleTracks.map((track, trackIndex) =>
        trackIndex === activeSubtitle ? { ...track, cues } : track,
      ),
    }));
    setActiveCue(cues.length - 1);
  }, [activeSubtitle, activeSubtitleTrack]);

  const removeCue = useCallback(
    (index: number) => {
      if (!activeSubtitleTrack) return;
      const nextCues = activeSubtitleTrack.cues.filter((_, cueIndex) => cueIndex !== index);
      setProject((current) => ({
        ...current,
        subtitleTracks: current.subtitleTracks.map((track, trackIndex) =>
          trackIndex === activeSubtitle ? { ...track, cues: nextCues } : track,
        ),
      }));
      setActiveCue((current) => Math.max(0, Math.min(current, nextCues.length - 1)));
    },
    [activeSubtitle, activeSubtitleTrack],
  );

  const handlePickCover = useCallback(async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        multiple: false,
        title: "选择封面",
        filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp", "gif"] }],
      });
      if (typeof selected === "string") patchProject({ coverPath: selected });
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 4000 });
    }
  }, [patchProject, toast]);

  const handleAddFont = useCallback(async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        multiple: true,
        title: "选择内嵌字体",
        filters: [{ name: "字体", extensions: ["ttf", "otf", "woff", "woff2"] }],
      });
      const paths = typeof selected === "string" ? [selected] : selected ?? [];
      if (!paths.length) return;
      setProject((current) => ({
        ...current,
        fonts: [
          ...current.fonts,
          ...(paths as string[]).map((path) => ({
            path,
            family: stem(path),
            style: null,
            weight: null,
            license: null,
            subset: null,
            fileName: baseName(path),
            mime: "",
          })),
        ],
      }));
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 4000 });
    }
  }, [toast]);

  const handlePickOutput = useCallback(async () => {
    if (!project.audioTracks.length) {
      toast({ title: "请先添加音频", status: "warning", duration: 3000 });
      return;
    }
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const path = await save({
        defaultPath: outputPath || defaultOutput(project),
        filters: [{ name: "NBMusic", extensions: ["nbmusic"] }],
      });
      if (path) setOutputPath(path);
    } catch (e) {
      toast({ title: String(e), status: "error", duration: 4000 });
    }
  }, [outputPath, project, toast]);

  const handleMux = useCallback(async () => {
    if (!project.audioTracks.length) {
      toast({ title: "至少需要一条音频轨", status: "warning", duration: 3000 });
      return;
    }
    for (const track of project.subtitleTracks) {
      for (const cue of track.cues) {
        if (cue.endMs <= cue.startMs) {
          toast({
            title: "字幕时间不合法",
            description: `“${cue.text.slice(0, 24) || cue.id || "Cue"}” 的结束时间必须大于开始时间`,
            status: "error",
            duration: 5000,
          });
          return;
        }
      }
    }
    const target = outputPath || defaultOutput(project);
    if (!target) {
      toast({ title: "请选择输出路径", status: "warning", duration: 3000 });
      return;
    }
    const finalTarget = target.toLowerCase().endsWith(".nbmusic") ? target : `${target}.nbmusic`;
    if (
      project.sourcePath &&
      finalTarget.toLowerCase() === project.sourcePath.toLowerCase() &&
      !window.confirm("输出路径与源文件相同，将覆盖原 .nbmusic，继续？")
    ) {
      return;
    }
    setMuxing(true);
    setError(null);
    setResult(null);
    try {
      const response = await invoke<MuxResult>("mux_nbmusic", {
        options: {
          outputPath: finalTarget,
          id: project.id || undefined,
          title: project.title || undefined,
          artist: project.artist || undefined,
          album: project.album || undefined,
          coverPath: project.coverPath || undefined,
          audioTracks: project.audioTracks.map((track) => ({
            path: track.path,
            uid: track.uid,
            language: track.language || "und",
            title: track.title || undefined,
            default: track.default,
            channelPrograms: track.channelPrograms,
          })),
          subtitleTracks: project.subtitleTracks
            .filter((track) => track.cues.length > 0)
            .map((track) => ({
              uid: track.uid,
              language: track.language || "und",
              roles: track.roles,
              audioBindings: track.audioBindings.length
                ? track.audioBindings.map((binding) => ({
                    trackUid: binding.trackUid,
                    channels: binding.channels?.length ? binding.channels : undefined,
                    labels: binding.labels?.length ? binding.labels : undefined,
                  }))
                : undefined,
              pronunciationScheme: track.pronunciationScheme || undefined,
              sourceSubtitleUid: track.sourceSubtitleUid || undefined,
              title: track.title || undefined,
              default: track.default,
              forced: track.forced,
              offsetMs: track.offsetMs ?? 0,
              cues: track.cues.map((cue) => ({
                id: cue.id,
                startMs: cue.startMs,
                endMs: cue.endMs,
                text: cue.text,
                chars: cue.chars?.length ? cue.chars : undefined,
                pronunciations: cue.pronunciations?.length ? cue.pronunciations : undefined,
                translations: cue.translations?.length ? cue.translations : undefined,
                style: cue.style || undefined,
              })),
            })),
          fonts: project.fonts.map((font) => ({
            path: font.path,
            family: font.family,
            style: font.style || undefined,
            weight: font.weight ?? undefined,
            license: font.license || undefined,
            subset: font.subset || undefined,
          })),
          climaxes: project.climaxes,
        },
      });
      setResult(response);
      setOutputPath(finalTarget);
      toast({
        title: "合成完成",
        description: `${baseName(response.outputPath)} · ${formatBytes(response.size)}`,
        status: "success",
        duration: 4000,
      });
    } catch (e) {
      setError(String(e));
      toast({ title: "合成失败", description: String(e), status: "error", duration: 6000 });
    } finally {
      setMuxing(false);
    }
  }, [outputPath, project, toast]);

  const handleDrop = useCallback(
    async (paths: string[]) => {
      const nbmusic = paths.find((path) => /\.nbmusic$/i.test(path));
      if (nbmusic) {
        await loadProject(nbmusic);
        return;
      }
      const audio = paths.filter((path) => AUDIO_EXT_RE.test(path));
      const subtitles = paths.filter((path) => SUBTITLE_EXT_RE.test(path));
      const images = paths.filter((path) => IMAGE_EXT_RE.test(path));
      const fonts = paths.filter((path) => FONT_EXT_RE.test(path));
      if (audio.length) await addAudioPaths(audio);
      if (images.length) patchProject({ coverPath: images[0] });
      if (fonts.length) {
        setProject((current) => ({
          ...current,
          fonts: [
            ...current.fonts,
            ...fonts.map((path) => ({
              path,
              family: stem(path),
              style: null,
              weight: null,
              license: null,
              subset: null,
              fileName: baseName(path),
              mime: "",
            })),
          ],
        }));
      }
      if (subtitles.length) {
        const imported: Array<{ cues: Cue[]; title: string; language: string }> = [];
        for (const path of subtitles) {
          try {
            const cues = await invoke<Cue[]>("parse_nbmusic_subtitle", { path });
            imported.push({ cues, title: stem(path), language: "und" });
          } catch (e) {
            toast({
              title: `字幕导入失败：${baseName(path)}`,
              description: String(e),
              status: "error",
              duration: 5000,
            });
          }
        }
        if (imported.length) {
          addSubtitleBatch(imported);
          toast({
            title: "字幕已导入",
            description: `${imported.length} 条字幕轨 · ${imported.reduce((sum, item) => sum + item.cues.length, 0)} 条 Cue`,
            status: "success",
            duration: 3000,
          });
        }
      }
      if (!audio.length && !subtitles.length && !images.length && !fonts.length) {
        toast({ title: "未识别到可导入的文件", status: "warning", duration: 3000 });
      }
    },
    [addAudioPaths, addSubtitleBatch, loadProject, patchProject, toast],
  );
  const { isDragging, dropProps } = useFileDrop(handleDrop);

  const handleBindingToggle = useCallback(
    (trackIndex: number, trackUid: string, bound: boolean) => {
      const subtitle = project.subtitleTracks[trackIndex];
      if (!subtitle) return;
      const bindings = bound
        ? [...subtitle.audioBindings.filter((item) => item.trackUid !== trackUid), { trackUid, channels: null, labels: null }]
        : subtitle.audioBindings.filter((item) => item.trackUid !== trackUid);
      updateSubtitle(trackIndex, { audioBindings: bindings });
    },
    [project.subtitleTracks, updateSubtitle],
  );

  const handleBindingChannels = useCallback(
    (trackIndex: number, trackUid: string, channel: number) => {
      const subtitle = project.subtitleTracks[trackIndex];
      if (!subtitle) return;
      const binding = subtitle.audioBindings.find((item) => item.trackUid === trackUid);
      if (!binding) return;
      const current = binding.channels ?? [];
      const next = current.includes(channel)
        ? current.filter((item) => item !== channel)
        : [...current, channel].sort((a, b) => a - b);
      updateSubtitle(trackIndex, {
        audioBindings: subtitle.audioBindings.map((item) =>
          item.trackUid === trackUid ? { ...item, channels: next.length ? next : null } : item,
        ),
      });
    },
    [project.subtitleTracks, updateSubtitle],
  );

  const audioSummary = `${project.audioTracks.length} 音频 · ${project.subtitleTracks.length} 字幕轨 · ${
    project.subtitleTracks.reduce((sum, track) => sum + track.cues.length, 0)
  } Cue`;

  return (
    <Box pt={8} pb={8} {...dropProps}>
      <FileDropOverlay visible={isDragging} hint="松开即可导入 .nbmusic / 音频 / 字幕 / 封面 / 字体" />

      <HStack justify="space-between" mb={5} flexWrap="wrap" gap={2}>
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
              <Music2 size={24} />
              <Text>{t("nbmusicEditor.title")}</Text>
            </HStack>
          </Heading>
          <Badge colorScheme="teal" fontSize="2xs">
            v1.0
          </Badge>
        </HStack>
        <HStack spacing={2} flexWrap="wrap">
          <Button size="sm" variant="outline" leftIcon={<FolderOpen size={15} />} onClick={handleOpen} isLoading={loading}>
            {t("nbmusicEditor.open")}
          </Button>
          <Button size="sm" variant="ghost" leftIcon={<Plus size={15} />} onClick={handleNew}>
            {t("nbmusicEditor.new")}
          </Button>
          <Button
            size="sm"
            bg={themeColorHex}
            color="#fff"
            _hover={{ bg: themeColorRgba(0.85) }}
            leftIcon={<Save size={15} />}
            onClick={handleMux}
            isLoading={muxing}
            loadingText={t("nbmusicEditor.muxing")}
          >
            {t("nbmusicEditor.mux")}
          </Button>
        </HStack>
      </HStack>

      <VStack align="stretch" spacing={4} maxW="1180px">
        <LiquidGlassCard px={4} py={3}>
          <HStack justify="space-between" flexWrap="wrap" gap={3}>
            <VStack align="start" spacing={0}>
              <Text fontSize="sm" fontWeight="bold">
                {project.title || t("nbmusicEditor.untitled")}
              </Text>
              <Text fontSize="2xs" opacity={0.66} wordBreak="break-all">
                {project.sourcePath || t("nbmusicEditor.newHint")}
              </Text>
            </VStack>
            <HStack spacing={3} flexWrap="wrap">
              <Text fontSize="xs" opacity={0.72}>{audioSummary}</Text>
              {project.durationMs > 0 && (
                <Badge variant="subtle" colorScheme="purple" fontSize="2xs">
                  <Clock3 size={11} style={{ display: "inline", marginRight: 4 }} />
                  {formatDuration(project.durationMs)}
                </Badge>
              )}
              {project.id && (
                <Badge variant="subtle" fontSize="2xs" maxW="210px" overflow="hidden" textOverflow="ellipsis">
                  ID {project.id}
                </Badge>
              )}
            </HStack>
          </HStack>
        </LiquidGlassCard>

        <LiquidGlassCard p={4}>
          <Tabs index={activeTab} onChange={setActiveTab} variant="soft-rounded" size="sm" isLazy>
            <TabList flexWrap="wrap" gap={1}>
              <Tab><HStack spacing={1}><Disc3 size={14} /><Text>{t("nbmusicEditor.tabMeta")}</Text></HStack></Tab>
              <Tab><HStack spacing={1}><AudioLines size={14} /><Text>{t("nbmusicEditor.tabAudio")}</Text></HStack></Tab>
              <Tab><HStack spacing={1}><FileText size={14} /><Text>{t("nbmusicEditor.tabSubtitle")}</Text></HStack></Tab>
              <Tab><HStack spacing={1}><Type size={14} /><Text>{t("nbmusicEditor.tabFont")}</Text></HStack></Tab>
              <Tab><HStack spacing={1}><Sparkles size={14} /><Text>{t("nbmusicEditor.tabClimax")}</Text></HStack></Tab>
            </TabList>

            <TabPanels pt={4}>
              {/* 元信息 */}
              <TabPanel px={0}>
                <Grid templateColumns={{ base: "1fr", lg: "1.1fr 0.9fr" }} gap={5}>
                  <GridItem>
                    <VStack align="stretch" spacing={3}>
                      <SectionTitle icon={<Disc3 size={17} />} title={t("nbmusicEditor.metadata")} />
                      <FormControl>
                        <FormLabel fontSize="xs">{t("nbmusicEditor.titleField")}</FormLabel>
                        <Input size="sm" value={project.title} onChange={(e) => patchProject({ title: e.target.value })} placeholder={t("nbmusicEditor.titlePlaceholder")} />
                      </FormControl>
                      <Grid templateColumns={{ base: "1fr", sm: "1fr 1fr" }} gap={3}>
                        <FormControl>
                          <FormLabel fontSize="xs">{t("nbmusicEditor.artist")}</FormLabel>
                          <Input size="sm" value={project.artist} onChange={(e) => patchProject({ artist: e.target.value })} />
                        </FormControl>
                        <FormControl>
                          <FormLabel fontSize="xs">{t("nbmusicEditor.album")}</FormLabel>
                          <Input size="sm" value={project.album} onChange={(e) => patchProject({ album: e.target.value })} />
                        </FormControl>
                      </Grid>
                      <FormControl>
                        <FormLabel fontSize="xs">{t("nbmusicEditor.output")}</FormLabel>
                        <HStack>
                          <Input size="sm" value={outputPath} onChange={(e) => setOutputPath(e.target.value)} placeholder={t("nbmusicEditor.outputHint")} />
                          <Button size="sm" variant="outline" onClick={handlePickOutput}>{t("nbmusicEditor.choose")}</Button>
                        </HStack>
                      </FormControl>
                      <HStack spacing={3} flexWrap="wrap" pt={1}>
                        <Badge colorScheme="teal" fontSize="2xs">NBMUSIC_SPEC {project.spec || "1.0"}</Badge>
                        <Badge fontSize="2xs">{t("nbmusicEditor.idPreserve")}</Badge>
                      </HStack>
                    </VStack>
                  </GridItem>
                  <GridItem>
                    <VStack align="stretch" spacing={3}>
                      <SectionTitle icon={<ImageIcon size={17} />} title={t("nbmusicEditor.cover")} />
                      <Box
                        borderRadius="xl"
                        overflow="hidden"
                        border="1px solid"
                        borderColor="whiteAlpha.300"
                        bg={fieldBg}
                        minH="190px"
                        display="flex"
                        alignItems="center"
                        justifyContent="center"
                      >
                        {coverSrc ? (
                          <Image src={coverSrc} alt="cover" maxH="260px" objectFit="contain" w="full" />
                        ) : (
                          <VStack opacity={0.55}>
                            <ImageIcon size={34} />
                            <Text fontSize="xs">{t("nbmusicEditor.noCover")}</Text>
                          </VStack>
                        )}
                      </Box>
                      <HStack>
                        <Button size="sm" variant="outline" onClick={handlePickCover} leftIcon={<FolderOpen size={14} />}>
                          {t("nbmusicEditor.chooseCover")}
                        </Button>
                        {project.coverPath && (
                          <Button size="sm" variant="ghost" onClick={() => patchProject({ coverPath: null })} leftIcon={<X size={14} />}>
                            {t("nbmusicEditor.clear")}
                          </Button>
                        )}
                      </HStack>
                      <Text fontSize="2xs" opacity={0.6}>
                        {t("nbmusicEditor.coverHint")}
                      </Text>
                    </VStack>
                  </GridItem>
                </Grid>
              </TabPanel>

              {/* 音频 */}
              <TabPanel px={0}>
                <HStack justify="space-between" mb={3} flexWrap="wrap">
                  <SectionTitle icon={<AudioLines size={17} />} title={t("nbmusicEditor.audioTracks")} description={t("nbmusicEditor.audioHint")} />
                  <Button size="sm" variant="outline" leftIcon={<Plus size={14} />} onClick={handleAddAudio}>
                    {t("nbmusicEditor.addAudio")}
                  </Button>
                </HStack>
                {project.audioTracks.length === 0 ? (
                  <EmptyHint text={t("nbmusicEditor.noAudio")} />
                ) : (
                  <VStack align="stretch" spacing={3}>
                    {project.audioTracks.map((track, index) => (
                      <Box key={track.uid ?? index} p={3} borderRadius="lg" border="1px solid" borderColor="whiteAlpha.300" bg={fieldBg}>
                        <HStack justify="space-between" mb={2} flexWrap="wrap" gap={2}>
                          <HStack>
                            <FileAudio size={16} />
                            <Text fontSize="sm" fontWeight="bold">{track.title || baseName(track.path)}</Text>
                            {track.uid && <Badge fontSize="2xs">UID {track.uid}</Badge>}
                            {track.default && <Badge colorScheme="green" fontSize="2xs">{t("nbmusicEditor.defaultTrack")}</Badge>}
                          </HStack>
                          <IconButton aria-label="remove audio" icon={<Trash2 size={14} />} size="xs" variant="ghost" onClick={() => removeAudio(track.uid)} />
                        </HStack>
                        <Text fontSize="2xs" opacity={0.62} wordBreak="break-all" mb={2}>{track.path}</Text>
                        <Grid templateColumns={{ base: "1fr", md: "1.2fr 0.7fr 0.6fr 0.6fr" }} gap={2} mb={2}>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.trackTitle")}</FormLabel>
                            <Input size="xs" value={track.title} onChange={(e) => updateAudio(index, { title: e.target.value })} />
                          </FormControl>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.language")}</FormLabel>
                            <Input size="xs" value={track.language} onChange={(e) => updateAudio(index, { language: e.target.value })} placeholder="und / zh / ja" />
                          </FormControl>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.channels")}</FormLabel>
                            <Input
                              size="xs"
                              type="number"
                              min={1}
                              max={32}
                              value={track.channels}
                              onChange={(e) => updateAudio(index, { channels: Math.max(1, Number(e.target.value) || 1) })}
                            />
                          </FormControl>
                          <FormControl display="flex" alignItems="flex-end">
                            <Checkbox size="sm" isChecked={track.default} onChange={(e) => updateAudio(index, { default: e.target.checked })}>
                              {t("nbmusicEditor.defaultTrack")}
                            </Checkbox>
                          </FormControl>
                        </Grid>
                        <JsonEditor
                          label={t("nbmusicEditor.channelPrograms")}
                          value={track.channelPrograms}
                          onChange={(value) => updateAudio(index, { channelPrograms: Array.isArray(value) ? (value as ChannelProgram[]) : [] })}
                        />
                      </Box>
                    ))}
                  </VStack>
                )}
              </TabPanel>

              {/* 字幕 */}
              <TabPanel px={0}>
                <HStack justify="space-between" mb={3} flexWrap="wrap" gap={2}>
                  <SectionTitle icon={<FileText size={17} />} title={t("nbmusicEditor.subtitleTracks")} description={t("nbmusicEditor.subtitleHint")} />
                  <HStack>
                    <Button size="sm" variant="outline" leftIcon={<FolderOpen size={14} />} onClick={handleImportSubtitle}>
                      {t("nbmusicEditor.importSubtitle")}
                    </Button>
                    <Button size="sm" variant="outline" leftIcon={<Plus size={14} />} onClick={() => addSubtitle()}>
                      {t("nbmusicEditor.addSubtitle")}
                    </Button>
                  </HStack>
                </HStack>

                {project.subtitleTracks.length === 0 ? (
                  <EmptyHint text={t("nbmusicEditor.noSubtitle")} />
                ) : (
                  <VStack align="stretch" spacing={4}>
                    <Wrap spacing={2}>
                      {project.subtitleTracks.map((track, index) => (
                        <WrapItem key={track.uid ?? index}>
                          <Button
                            size="xs"
                            variant={index === activeSubtitle ? "solid" : "outline"}
                            onClick={() => {
                              setActiveSubtitle(index);
                              setActiveCue(0);
                            }}
                          >
                            {track.title || `Sub ${index + 1}`}
                            <Badge ml={2} fontSize="2xs">{track.cues.length}</Badge>
                          </Button>
                        </WrapItem>
                      ))}
                    </Wrap>

                    {activeSubtitleTrack && (
                      <Box p={3} borderRadius="lg" border="1px solid" borderColor="whiteAlpha.300" bg={fieldBg}>
                        <HStack justify="space-between" mb={3}>
                          <Text fontSize="sm" fontWeight="bold">{t("nbmusicEditor.trackSettings")}</Text>
                          <IconButton aria-label="remove subtitle" icon={<Trash2 size={14} />} size="xs" variant="ghost" onClick={() => removeSubtitle(activeSubtitle)} />
                        </HStack>
                        <Grid templateColumns={{ base: "1fr", md: "1.2fr .8fr .8fr .8fr" }} gap={3} mb={3}>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.trackTitle")}</FormLabel>
                            <Input size="xs" value={activeSubtitleTrack.title} onChange={(e) => updateSubtitle(activeSubtitle, { title: e.target.value })} />
                          </FormControl>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.language")}</FormLabel>
                            <Input size="xs" value={activeSubtitleTrack.language} onChange={(e) => updateSubtitle(activeSubtitle, { language: e.target.value })} placeholder="und / zh / ja" />
                          </FormControl>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.pronunciationScheme")}</FormLabel>
                            <Input
                              size="xs"
                              value={activeSubtitleTrack.pronunciationScheme ?? ""}
                              onChange={(e) => updateSubtitle(activeSubtitle, { pronunciationScheme: e.target.value || null })}
                              placeholder={SCHEME_OPTIONS.join(" / ")}
                            />
                          </FormControl>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.offsetMs")}</FormLabel>
                            <Input
                              size="xs"
                              type="number"
                              value={activeSubtitleTrack.offsetMs ?? 0}
                              onChange={(e) => updateSubtitle(activeSubtitle, { offsetMs: Number(e.target.value) || 0 })}
                            />
                          </FormControl>
                        </Grid>
                        <Grid templateColumns={{ base: "1fr", md: "1fr 1fr" }} gap={3} mb={3}>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.roles")}</FormLabel>
                            <TagInput
                              values={activeSubtitleTrack.roles}
                              suggestions={ROLE_OPTIONS}
                              onChange={(roles) => updateSubtitle(activeSubtitle, { roles })}
                            />
                          </FormControl>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.flags")}</FormLabel>
                            <HStack spacing={4} pt={1}>
                              <Checkbox size="sm" isChecked={activeSubtitleTrack.default} onChange={(e) => updateSubtitle(activeSubtitle, { default: e.target.checked })}>
                                {t("nbmusicEditor.defaultTrack")}
                              </Checkbox>
                              <Checkbox size="sm" isChecked={activeSubtitleTrack.forced} onChange={(e) => updateSubtitle(activeSubtitle, { forced: e.target.checked })}>
                                {t("nbmusicEditor.forced")}
                              </Checkbox>
                            </HStack>
                          </FormControl>
                        </Grid>

                        <Divider my={3} />
                        <SectionTitle icon={<AudioLines size={15} />} title={t("nbmusicEditor.audioBindings")} description={t("nbmusicEditor.bindingHint")} />
                        <VStack align="stretch" spacing={2}>
                          {project.audioTracks.map((audio) => {
                            const binding = activeSubtitleTrack.audioBindings.find((item) => item.trackUid === audio.uid);
                            return (
                              <Box key={audio.uid} p={2} borderRadius="md" border="1px solid" borderColor="whiteAlpha.200">
                                <HStack flexWrap="wrap" spacing={2}>
                                  <Checkbox
                                    size="sm"
                                    isChecked={!!binding}
                                    onChange={(e) => handleBindingToggle(activeSubtitle, audio.uid || "", e.target.checked)}
                                  >
                                    {audio.title || baseName(audio.path)}
                                  </Checkbox>
                                  <Badge fontSize="2xs" variant="subtle">{audio.channels}ch</Badge>
                                  {audio.uid && <Text fontSize="2xs" opacity={0.55}>UID {audio.uid}</Text>}
                                </HStack>
                                {binding && (
                                  <Wrap spacing={1} mt={2}>
                                    <WrapItem>
                                      <Button
                                        size="xs"
                                        variant={!binding.channels?.length ? "solid" : "outline"}
                                        onClick={() =>
                                          updateSubtitle(activeSubtitle, {
                                            audioBindings: activeSubtitleTrack.audioBindings.map((item) =>
                                              item.trackUid === audio.uid ? { ...item, channels: null } : item,
                                            ),
                                          })
                                        }
                                      >
                                        {t("nbmusicEditor.allChannels")}
                                      </Button>
                                    </WrapItem>
                                    {Array.from({ length: Math.max(1, audio.channels) }, (_, i) => i + 1).map((channel) => (
                                      <WrapItem key={channel}>
                                        <Button
                                          size="xs"
                                          variant={binding.channels?.includes(channel) ? "solid" : "ghost"}
                                          onClick={() => handleBindingChannels(activeSubtitle, audio.uid || "", channel)}
                                        >
                                          {channelName(audio.channels, channel)}
                                        </Button>
                                      </WrapItem>
                                    ))}
                                  </Wrap>
                                )}
                              </Box>
                            );
                          })}
                        </VStack>
                      </Box>
                    )}

                    {activeSubtitleTrack && (
                      <Grid templateColumns={{ base: "1fr", xl: "0.9fr 1.1fr" }} gap={4}>
                        <GridItem>
                          <HStack justify="space-between" mb={2}>
                            <SectionTitle icon={<FileText size={15} />} title={t("nbmusicEditor.cues")} description={`${activeSubtitleTrack.cues.length} Cue`} />
                            <Button size="xs" variant="outline" leftIcon={<Plus size={13} />} onClick={addCue}>
                              {t("nbmusicEditor.addCue")}
                            </Button>
                          </HStack>
                          <VStack align="stretch" spacing={1} maxH="520px" overflowY="auto">
                            {activeSubtitleTrack.cues.length === 0 ? (
                              <EmptyHint text={t("nbmusicEditor.noCues")} />
                            ) : (
                              activeSubtitleTrack.cues.map((cue, index) => (
                                <HStack
                                  key={cue.id ?? index}
                                  p={2}
                                  borderRadius="md"
                                  cursor="pointer"
                                  bg={index === activeCue ? themeColorRgba(0.16) : "transparent"}
                                  border="1px solid"
                                  borderColor={index === activeCue ? themeColorRgba(0.35) : "transparent"}
                                  _hover={{ bg: themeColorRgba(0.1) }}
                                  onClick={() => setActiveCue(index)}
                                  spacing={2}
                                >
                                  <Text fontSize="2xs" fontFamily="mono" opacity={0.7} minW="88px">
                                    {formatCueTime(cue.startMs)}
                                  </Text>
                                  <Text fontSize="xs" flex={1} noOfLines={1}>{cue.text || t("nbmusicEditor.emptyCue")}</Text>
                                  <IconButton
                                    aria-label="delete cue"
                                    icon={<Trash2 size={12} />}
                                    size="xs"
                                    variant="ghost"
                                    onClick={(event) => {
                                      event.stopPropagation();
                                      removeCue(index);
                                    }}
                                  />
                                </HStack>
                              ))
                            )}
                          </VStack>
                        </GridItem>
                        <GridItem>
                          {activeCueData ? (
                            <Box p={3} borderRadius="lg" border="1px solid" borderColor="whiteAlpha.300" bg={fieldBg}>
                              <HStack justify="space-between" mb={3}>
                                <HStack>
                                  <Text fontSize="sm" fontWeight="bold">{t("nbmusicEditor.cueDetail")}</Text>
                                  <Badge fontSize="2xs">{activeCue + 1} / {activeSubtitleTrack.cues.length}</Badge>
                                </HStack>
                                <HStack>
                                  <IconButton aria-label="prev cue" icon={<ChevronLeft size={14} />} size="xs" variant="ghost" disabled={activeCue <= 0} onClick={() => setActiveCue((v) => Math.max(0, v - 1))} />
                                  <IconButton aria-label="next cue" icon={<ChevronRight size={14} />} size="xs" variant="ghost" disabled={activeCue >= activeSubtitleTrack.cues.length - 1} onClick={() => setActiveCue((v) => Math.min(activeSubtitleTrack.cues.length - 1, v + 1))} />
                                </HStack>
                              </HStack>
                              <HStack mb={3}>
                                <FormControl>
                                  <FormLabel fontSize="2xs">{t("nbmusicEditor.start")}</FormLabel>
                                  <TimeInput value={activeCueData.startMs} onChange={(value) => updateCue(activeCue, { startMs: value })} ariaLabel="cue start" />
                                </FormControl>
                                <FormControl>
                                  <FormLabel fontSize="2xs">{t("nbmusicEditor.end")}</FormLabel>
                                  <TimeInput value={activeCueData.endMs} onChange={(value) => updateCue(activeCue, { endMs: value })} ariaLabel="cue end" />
                                </FormControl>
                                <Button
                                  size="xs"
                                  variant="ghost"
                                  alignSelf="flex-end"
                                  onClick={() => updateCue(activeCue, { endMs: activeCueData.startMs + 3000 })}
                                >
                                  +3s
                                </Button>
                              </HStack>
                              <FormControl mb={3}>
                                <FormLabel fontSize="2xs">{t("nbmusicEditor.cueText")}</FormLabel>
                                <Textarea
                                  size="sm"
                                  value={activeCueData.text}
                                  onChange={(e) => updateCue(activeCue, { text: e.target.value })}
                                  minH="84px"
                                  placeholder={t("nbmusicEditor.cueTextHint")}
                                />
                              </FormControl>

                              <FormControl mb={3}>
                                <HStack justify="space-between" mb={1}>
                                  <FormLabel fontSize="2xs" m={0}>{t("nbmusicEditor.translations")}</FormLabel>
                                  <Button
                                    size="xs"
                                    variant="ghost"
                                    leftIcon={<Plus size={12} />}
                                    onClick={() => updateCue(activeCue, { translations: [...(activeCueData.translations ?? []), { lang: "zh", text: "" }] })}
                                  >
                                    {t("nbmusicEditor.addTranslation")}
                                  </Button>
                                </HStack>
                                {(activeCueData.translations ?? []).map((item, index) => (
                                  <HStack key={index} mb={1}>
                                    <Input size="xs" w="74px" value={item.lang} onChange={(e) => {
                                      const translations = [...(activeCueData.translations ?? [])];
                                      translations[index] = { ...item, lang: e.target.value };
                                      updateCue(activeCue, { translations });
                                    }} />
                                    <Input size="xs" value={item.text} onChange={(e) => {
                                      const translations = [...(activeCueData.translations ?? [])];
                                      translations[index] = { ...item, text: e.target.value };
                                      updateCue(activeCue, { translations });
                                    }} />
                                    <IconButton aria-label="delete translation" icon={<X size={12} />} size="xs" variant="ghost" onClick={() => {
                                      updateCue(activeCue, { translations: (activeCueData.translations ?? []).filter((_, i) => i !== index) });
                                    }} />
                                  </HStack>
                                ))}
                                {(activeCueData.translations ?? []).length === 0 && <Text fontSize="2xs" opacity={0.55}>{t("nbmusicEditor.none")}</Text>}
                              </FormControl>

                              <FormControl mb={3}>
                                <HStack justify="space-between" mb={1}>
                                  <FormLabel fontSize="2xs" m={0}>{t("nbmusicEditor.pronunciations")}</FormLabel>
                                  <Button
                                    size="xs"
                                    variant="ghost"
                                    leftIcon={<Plus size={12} />}
                                    onClick={() => updateCue(activeCue, { pronunciations: [...(activeCueData.pronunciations ?? []), { scheme: "zh-phonetic", lang: activeSubtitleTrack.language || "ja", text: "" }] })}
                                  >
                                    {t("nbmusicEditor.addPronunciation")}
                                  </Button>
                                </HStack>
                                {(activeCueData.pronunciations ?? []).map((item, index) => (
                                  <HStack key={index} mb={1} align="flex-start">
                                    <Input size="xs" w="100px" value={item.scheme} onChange={(e) => {
                                      const pronunciations = [...(activeCueData.pronunciations ?? [])];
                                      pronunciations[index] = { ...item, scheme: e.target.value };
                                      updateCue(activeCue, { pronunciations });
                                    }} placeholder={SCHEME_OPTIONS.join(" / ")} />
                                    <Input size="xs" value={item.text} onChange={(e) => {
                                      const pronunciations = [...(activeCueData.pronunciations ?? [])];
                                      pronunciations[index] = { ...item, text: e.target.value };
                                      updateCue(activeCue, { pronunciations });
                                    }} placeholder={t("nbmusicEditor.pronunciationHint")} />
                                    <IconButton aria-label="delete pronunciation" icon={<X size={12} />} size="xs" variant="ghost" onClick={() => {
                                      updateCue(activeCue, { pronunciations: (activeCueData.pronunciations ?? []).filter((_, i) => i !== index) });
                                    }} />
                                  </HStack>
                                ))}
                                {(activeCueData.pronunciations ?? []).length === 0 && <Text fontSize="2xs" opacity={0.55}>{t("nbmusicEditor.none")}</Text>}
                              </FormControl>

                              {(activeCueData.chars ?? []).length > 0 && (
                                <Box mb={3} p={2} borderRadius="lg" border="1px solid" borderColor="whiteAlpha.300" bg={fieldBg}>
                                  <Text fontSize="2xs" opacity={0.7} mb={1}>
                                    逐字解析预览
                                  </Text>
                                  <Wrap spacing={1}>
                                    {(activeCueData.chars ?? []).map((char, index) => {
                                      const label = String(char.text ?? "");
                                      const start = Number(char.startMs ?? 0);
                                      const end = Number(char.endMs ?? 0);
                                      return (
                                        <WrapItem key={index}>
                                          <Badge
                                            px={1.5}
                                            py={0.5}
                                            borderRadius="md"
                                            fontSize="2xs"
                                            bg={themeColorRgba(0.14)}
                                            color={adaptiveTitle.text}
                                          >
                                            {label.trim() ? label : "∅"} · {Number.isFinite(start) ? start : 0}
                                            –{Number.isFinite(end) ? end : 0}ms
                                          </Badge>
                                        </WrapItem>
                                      );
                                    })}
                                  </Wrap>
                                </Box>
                              )}
                              <JsonEditor
                                label={t("nbmusicEditor.charSpans")}
                                value={activeCueData.chars ?? []}
                                onChange={(value) => updateCue(activeCue, { chars: Array.isArray(value) ? (value as Array<Record<string, unknown>>) : [] })}
                              />
                              <Box h={2} />
                              <JsonEditor
                                label={t("nbmusicEditor.styleJson")}
                                value={activeCueData.style ?? null}
                                onChange={(value) => updateCue(activeCue, { style: value && typeof value === "object" ? (value as Record<string, unknown>) : null })}
                              />
                            </Box>
                          ) : (
                            <EmptyHint text={t("nbmusicEditor.selectCue")} />
                          )}
                        </GridItem>
                      </Grid>
                    )}
                  </VStack>
                )}
              </TabPanel>

              {/* 字体 */}
              <TabPanel px={0}>
                <HStack justify="space-between" mb={3} flexWrap="wrap">
                  <SectionTitle icon={<Type size={17} />} title={t("nbmusicEditor.fonts")} description={t("nbmusicEditor.fontHint")} />
                  <Button size="sm" variant="outline" leftIcon={<Plus size={14} />} onClick={handleAddFont}>
                    {t("nbmusicEditor.addFont")}
                  </Button>
                </HStack>
                {project.fonts.length === 0 ? (
                  <EmptyHint text={t("nbmusicEditor.noFonts")} />
                ) : (
                  <VStack align="stretch" spacing={2}>
                    {project.fonts.map((font, index) => (
                      <Box key={`${font.path}-${index}`} p={3} borderRadius="lg" border="1px solid" borderColor="whiteAlpha.300" bg={fieldBg}>
                        <HStack justify="space-between" mb={2} flexWrap="wrap">
                          <HStack>
                            <Type size={15} />
                            <Text fontSize="sm" fontWeight="bold">{font.fileName || baseName(font.path)}</Text>
                            <Badge fontSize="2xs">{font.mime || "font"}</Badge>
                          </HStack>
                          <IconButton aria-label="remove font" icon={<Trash2 size={14} />} size="xs" variant="ghost" onClick={() => patchProject({ fonts: project.fonts.filter((_, i) => i !== index) })} />
                        </HStack>
                        <Text fontSize="2xs" opacity={0.6} wordBreak="break-all" mb={2}>{font.path}</Text>
                        <Grid templateColumns={{ base: "1fr", md: "1.1fr .7fr .5fr 1fr" }} gap={2}>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.fontFamily")}</FormLabel>
                            <Input size="xs" value={font.family} onChange={(e) => patchProject({ fonts: project.fonts.map((item, i) => i === index ? { ...item, family: e.target.value } : item) })} />
                          </FormControl>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.fontStyle")}</FormLabel>
                            <Input size="xs" value={font.style ?? ""} onChange={(e) => patchProject({ fonts: project.fonts.map((item, i) => i === index ? { ...item, style: e.target.value || null } : item) })} placeholder="Regular / Bold" />
                          </FormControl>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.fontWeight")}</FormLabel>
                            <Input size="xs" type="number" value={font.weight ?? ""} onChange={(e) => patchProject({ fonts: project.fonts.map((item, i) => i === index ? { ...item, weight: e.target.value ? Number(e.target.value) : null } : item) })} />
                          </FormControl>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.license")}</FormLabel>
                            <Input size="xs" value={font.license ?? ""} onChange={(e) => patchProject({ fonts: project.fonts.map((item, i) => i === index ? { ...item, license: e.target.value || null } : item) })} placeholder="OFL-1.1" />
                          </FormControl>
                        </Grid>
                      </Box>
                    ))}
                  </VStack>
                )}
              </TabPanel>

              {/* 高潮 */}
              <TabPanel px={0}>
                <HStack justify="space-between" mb={3} flexWrap="wrap">
                  <SectionTitle icon={<Sparkles size={17} />} title={t("nbmusicEditor.climaxes")} description={t("nbmusicEditor.climaxHint")} />
                  <Button
                    size="sm"
                    variant="outline"
                    leftIcon={<Plus size={14} />}
                    onClick={() => patchProject({
                      climaxes: [
                        ...project.climaxes,
                        { id: `climax-${project.climaxes.length + 1}`, startMs: 0, endMs: null, peakMs: null, kind: "peak", label: "", strength: 80, source: "manual", confidence: null },
                      ],
                    })}
                  >
                    {t("nbmusicEditor.addClimax")}
                  </Button>
                </HStack>
                {project.climaxes.length === 0 ? (
                  <EmptyHint text={t("nbmusicEditor.noClimaxes")} />
                ) : (
                  <VStack align="stretch" spacing={2}>
                    {project.climaxes.map((climax, index) => (
                      <Box key={climax.id ?? index} p={3} borderRadius="lg" border="1px solid" borderColor="whiteAlpha.300" bg={fieldBg}>
                        <HStack justify="space-between" mb={2}>
                          <Badge colorScheme="orange" fontSize="2xs">{climax.kind || "peak"}</Badge>
                          <IconButton aria-label="remove climax" icon={<Trash2 size={14} />} size="xs" variant="ghost" onClick={() => patchProject({ climaxes: project.climaxes.filter((_, i) => i !== index) })} />
                        </HStack>
                        <Grid templateColumns={{ base: "1fr", md: "1fr 1fr 1fr 1.3fr .6fr" }} gap={2}>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.start")}</FormLabel>
                            <TimeInput value={climax.startMs} onChange={(value) => patchProject({ climaxes: project.climaxes.map((item, i) => i === index ? { ...item, startMs: value } : item) })} ariaLabel="climax start" width="100%" />
                          </FormControl>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.peak")}</FormLabel>
                            <Input size="xs" type="number" value={climax.peakMs ?? ""} onChange={(e) => patchProject({ climaxes: project.climaxes.map((item, i) => i === index ? { ...item, peakMs: e.target.value ? Number(e.target.value) : null } : item) })} />
                          </FormControl>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.end")}</FormLabel>
                            <Input size="xs" type="number" value={climax.endMs ?? ""} onChange={(e) => patchProject({ climaxes: project.climaxes.map((item, i) => i === index ? { ...item, endMs: e.target.value ? Number(e.target.value) : null } : item) })} />
                          </FormControl>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.label")}</FormLabel>
                            <Input size="xs" value={climax.label ?? ""} onChange={(e) => patchProject({ climaxes: project.climaxes.map((item, i) => i === index ? { ...item, label: e.target.value || null } : item) })} />
                          </FormControl>
                          <FormControl>
                            <FormLabel fontSize="2xs">{t("nbmusicEditor.strength")}</FormLabel>
                            <Input size="xs" type="number" min={0} max={100} value={climax.strength ?? 80} onChange={(e) => patchProject({ climaxes: project.climaxes.map((item, i) => i === index ? { ...item, strength: Number(e.target.value) || 0 } : item) })} />
                          </FormControl>
                        </Grid>
                      </Box>
                    ))}
                  </VStack>
                )}
              </TabPanel>
            </TabPanels>
          </Tabs>
        </LiquidGlassCard>

        {muxing && (
          <LiquidGlassCard p={3}>
            <HStack spacing={2}>
              <Disc3 size={15} />
              <Text fontSize="xs">{t("nbmusicEditor.muxingHint")}</Text>
            </HStack>
          </LiquidGlassCard>
        )}

        {error && !muxing && (
          <Text fontSize="xs" color="red.400" wordBreak="break-all">{t("nbmusicEditor.failed")}: {error}</Text>
        )}

        {result && (
          <LiquidGlassCard p={4}>
            <HStack justify="space-between" mb={2} flexWrap="wrap">
              <HStack>
                <Text fontSize="sm" fontWeight="bold">{t("nbmusicEditor.result")}</Text>
                <Badge colorScheme="green" fontSize="2xs">{t("nbmusicEditor.done")}</Badge>
              </HStack>
              <Text fontSize="2xs" opacity={0.7}>{formatBytes(result.size)}</Text>
            </HStack>
            <Text fontSize="xs" wordBreak="break-all" mb={2}>{result.outputPath}</Text>
            <Wrap spacing={2}>
              <WrapItem><Badge fontSize="2xs">{t("nbmusicEditor.audioTracks")}: {result.audioTrackCount}</Badge></WrapItem>
              <WrapItem><Badge fontSize="2xs">{t("nbmusicEditor.subtitleTracks")}: {result.subtitleTrackCount}</Badge></WrapItem>
              <WrapItem><Badge fontSize="2xs">{formatDuration(result.durationMs)}</Badge></WrapItem>
              <WrapItem><Badge fontSize="2xs">{t("nbmusicEditor.fonts")}: {result.fontCount}</Badge></WrapItem>
              {result.hasCover && <WrapItem><Badge colorScheme="purple" fontSize="2xs">{t("nbmusicEditor.coverIncluded")}</Badge></WrapItem>}
            </Wrap>
          </LiquidGlassCard>
        )}

        <HStack spacing={2} align="flex-start" opacity={0.65}>
          <Info size={14} style={{ marginTop: 2, flexShrink: 0 }} />
          <Text fontSize="2xs">
            {t("nbmusicEditor.footerHint")}
          </Text>
        </HStack>
      </VStack>
    </Box>
  );
}
