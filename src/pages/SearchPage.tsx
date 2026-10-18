import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Badge,
  Box,
  Flex,
  HStack,
  IconButton,
  Input,
  Spinner,
  Switch,
  Text,
  Tooltip,
  useColorModeValue,
} from "@chakra-ui/react";
import {
  AlertCircle,
  Archive,
  ArrowDown,
  ArrowUp,
  Copy,
  ExternalLink,
  File,
  FileCode2,
  FileText,
  Film,
  Folder,
  FolderOpen,
  FolderSearch,
  HardDrive,
  Image as ImageIcon,
  Loader2,
  Music,
  Package,
  RefreshCw,
  Search,
  X,
  type LucideIcon,
} from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import { LiquidGlassCard } from "@/components/special/liquid-glass-card";
import { CustomSelect } from "@/components/special/custom-select";
import { useDynamicIsland } from "@/components/ui/dynamic-island";
import { useThemeColor } from "@/contexts/theme-color-context";
import { store } from "@/lib/store";
import {
  SEARCH_CUSTOM_EXT_KEY,
  SEARCH_FILTER_KEY,
  SEARCH_FILTER_PRESETS,
  SEARCH_INCLUDE_HIDDEN_KEY,
  SEARCH_SCOPE_KEY,
  SEARCH_SORT_DESC_KEY,
  SEARCH_SORT_KEY,
  buildSearchIndex,
  cancelSearchIndex,
  fileTypeOf,
  formatFileSize,
  formatSearchTime,
  getSearchDrives,
  getSearchFilter,
  getSearchStatus,
  openSearchItem,
  parseCustomExtensions,
  revealSearchItem,
  scopeRoots,
  searchFiles,
  type SearchDrive,
  type SearchItem,
  type SearchProgress,
  type SearchResponse,
  type SearchScope,
  type SearchSort,
  type SearchStatus,
} from "@/lib/file-search";

const TYPE_ICONS: Record<string, { icon: LucideIcon; color: string }> = {
  folder: { icon: Folder, color: "#eab308" },
  image: { icon: ImageIcon, color: "#22c55e" },
  video: { icon: Film, color: "#a855f7" },
  audio: { icon: Music, color: "#ec4899" },
  archive: { icon: Archive, color: "#f97316" },
  code: { icon: FileCode2, color: "#3b82f6" },
  document: { icon: FileText, color: "#60a5fa" },
  program: { icon: Package, color: "#14b8a6" },
  file: { icon: File, color: "#94a3b8" },
};

function TypeIcon({ item, size = 16 }: { item: SearchItem; size?: number }) {
  const type = fileTypeOf(item);
  const config = TYPE_ICONS[type] ?? TYPE_ICONS.file;
  const Icon = config.icon;
  return <Icon size={size} color={config.color} strokeWidth={2} />;
}

function HighlightedName({ name, query }: { name: string; query: string }) {
  const needle = query.trim();
  if (!needle) return <>{name}</>;
  const lowerName = name.toLowerCase();
  const index = lowerName.indexOf(needle.toLowerCase());
  if (index < 0) return <>{name}</>;
  return (
    <>
      {name.slice(0, index)}
      <Box as="mark" bg="transparent" color="inherit" fontWeight="bold">
        {name.slice(index, index + needle.length)}
      </Box>
      {name.slice(index + needle.length)}
    </>
  );
}

function shortenPath(path: string, max = 46): string {
  if (path.length <= max) return path;
  const parts = path.split("\\");
  let result = parts[parts.length - 1] || path;
  for (let i = parts.length - 2; i >= 0; i -= 1) {
    const next = `${parts[i]}\\${result}`;
    if (next.length > max) break;
    result = next;
  }
  return `…\\${result}`;
}

export default function SearchPage() {
  const { t } = useTranslation();
  const toast = useDynamicIsland("search");
  const { getActiveColor, getHoverColor } = useThemeColor();
  const titleColor = useColorModeValue("gray.800", "#ffffff");
  const subColor = useColorModeValue("gray.500", "#9ca3af");
  const bodyColor = useColorModeValue("gray.700", "#e5e7eb");
  const rowBorder = useColorModeValue("rgba(0,0,0,0.05)", "rgba(255,255,255,0.05)");
  const tableHead = useColorModeValue("rgba(0,0,0,0.03)", "rgba(255,255,255,0.03)");
  const inputBg = useColorModeValue("rgba(255,255,255,0.55)", "rgba(255,255,255,0.04)");
  const activeColor = getActiveColor();

  const [drives, setDrives] = useState<SearchDrive[]>([]);
  const [scope, setScope] = useState<SearchScope>({ mode: "all" });
  const [status, setStatus] = useState<SearchStatus | null>(null);
  const [progress, setProgress] = useState<SearchProgress | null>(null);
  const [buildError, setBuildError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);

  const [query, setQuery] = useState("");
  const [debounced, setDebounced] = useState("");
  const [filterKey, setFilterKey] = useState("all");
  const [customExt, setCustomExt] = useState("");
  const [includeHidden, setIncludeHidden] = useState(true);
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [sortBy, setSortBy] = useState<SearchSort>("name");
  const [sortDesc, setSortDesc] = useState(false);

  const [response, setResponse] = useState<SearchResponse | null>(null);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [selectedIndex, setSelectedIndex] = useState(-1);
  const [menu, setMenu] = useState<{ x: number; y: number; item: SearchItem } | null>(null);

  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const sequenceRef = useRef(0);
  const autoStartedRef = useRef(false);
  const statusRef = useRef<SearchStatus | null>(null);
  statusRef.current = status;

  const scopeOptions = useMemo(() => {
    const options = [
      { value: "all", label: t("fileSearch.allDrives") },
      ...drives.map((drive) => ({
        value: `${drive.letter}\\`,
        label: `${drive.letter}  ${drive.label}`,
      })),
    ];
    if (scope.mode === "path" && !options.some((option) => option.value === scope.path)) {
      options.splice(1, 0, { value: scope.path, label: shortenPath(scope.path, 32) });
    }
    options.push({ value: "__folder__", label: t("fileSearch.pickFolder") });
    return options;
  }, [drives, scope, t]);

  const filterOptions = useMemo(
    () =>
      SEARCH_FILTER_PRESETS.map((preset) => ({
        value: preset.key,
        label: t(`fileSearch.format.${preset.key}`),
      })),
    [t],
  );

  const sortOptions = useMemo(
    () => [
      { value: "name", label: t("fileSearch.sort.name") },
      { value: "modified", label: t("fileSearch.sort.modified") },
      { value: "type", label: t("fileSearch.sort.type") },
      { value: "size", label: t("fileSearch.sort.size") },
    ],
    [t],
  );

  const isIndexing = Boolean(status?.indexing || progress?.indexing);
  const indexReady = Boolean(status?.ready);
  const items = response?.items ?? [];

  const persist = useCallback((key: string, value: unknown) => {
    store
      .set(key, value)
      .then(() => store.save())
      .catch((error) => console.error("[Search] 保存设置失败:", error));
  }, []);

  const startBuild = useCallback(async (roots: string[] | null) => {
    setBuildError(null);
    setSearchError(null);
    setResponse(null);
    try {
      const next = await buildSearchIndex(roots);
      setStatus(next);
    } catch (error) {
      setBuildError(String(error));
    }
  }, []);

  // 切换范围/重建时，如果旧索引还在建立，先取消并等待线程退出再开始新任务
  const rebuild = useCallback(
    async (roots: string[] | null) => {
      if (statusRef.current?.indexing) {
        try {
          await cancelSearchIndex();
        } catch (error) {
          console.error("[Search] 取消索引失败:", error);
        }
        for (let attempt = 0; attempt < 60; attempt += 1) {
          await new Promise((resolve) => setTimeout(resolve, 100));
          try {
            const next = await getSearchStatus();
            setStatus(next);
            if (!next.indexing) break;
          } catch {
            break;
          }
        }
      }
      await startBuild(roots);
    },
    [startBuild],
  );

  // 读取设置、磁盘列表与索引状态
  useEffect(() => {
    let disposed = false;
    (async () => {
      try {
        const [
          savedScope,
          savedFilter,
          savedHidden,
          savedCustom,
          savedSort,
          savedSortDesc,
          driveList,
          currentStatus,
        ] = await Promise.all([
            store.get<SearchScope>(SEARCH_SCOPE_KEY),
            store.get<string>(SEARCH_FILTER_KEY),
            store.get<boolean>(SEARCH_INCLUDE_HIDDEN_KEY),
            store.get<string>(SEARCH_CUSTOM_EXT_KEY),
            store.get<string>(SEARCH_SORT_KEY),
            store.get<boolean>(SEARCH_SORT_DESC_KEY),
            getSearchDrives(),
            getSearchStatus(),
          ]);
        if (disposed) return;
        setDrives(driveList);
        if (savedScope?.mode === "path" && savedScope.path) {
          setScope({ mode: "path", path: savedScope.path });
        } else if (savedScope?.mode === "all") {
          setScope({ mode: "all" });
        }
        if (savedFilter && SEARCH_FILTER_PRESETS.some((preset) => preset.key === savedFilter)) {
          setFilterKey(savedFilter);
        }
        if (typeof savedHidden === "boolean") setIncludeHidden(savedHidden);
        if (typeof savedCustom === "string") setCustomExt(savedCustom);
        if (savedSort === "name" || savedSort === "modified" || savedSort === "type" || savedSort === "size") {
          setSortBy(savedSort);
        }
        if (typeof savedSortDesc === "boolean") setSortDesc(savedSortDesc);
        setStatus(currentStatus);
        setLoaded(true);
      } catch (error) {
        console.error("[Search] 初始化失败:", error);
        if (!disposed) setLoaded(true);
      }
    })();
    return () => {
      disposed = true;
    };
  }, []);

  // 首次进入且没有索引时自动建立
  useEffect(() => {
    if (!loaded || autoStartedRef.current) return;
    if (status?.ready || status?.indexing) return;
    autoStartedRef.current = true;
    void rebuild(scopeRoots(scope));
  }, [loaded, scope, rebuild, startBuild, status?.indexing, status?.ready]);

  // 索引进度事件
  useEffect(() => {
    let disposed = false;
    const unlisten = listen<SearchProgress>("search:progress", (event) => {
      if (disposed) return;
      setProgress(event.payload);
      if (event.payload.done) {
        getSearchStatus()
          .then((next) => {
            if (!disposed) setStatus(next);
          })
          .catch(() => {});
        if (event.payload.error) {
          setBuildError(event.payload.error);
        } else {
          setBuildError(null);
        }
      }
    });
    return () => {
      disposed = true;
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  // 输入防抖
  useEffect(() => {
    const timer = setTimeout(() => setDebounced(query), 130);
    return () => clearTimeout(timer);
  }, [query]);

  // 执行搜索
  useEffect(() => {
    if (!indexReady) return;
    const keyword = debounced.trim();
    if (!keyword) {
      setResponse(null);
      setSearching(false);
      return;
    }
    const preset = getSearchFilter(filterKey);
    const extensions =
      filterKey === "custom" ? parseCustomExtensions(customExt) : preset.extensions;
    const sequence = ++sequenceRef.current;
    let disposed = false;
    setSearching(true);
    setSearchError(null);

    const timer = setTimeout(async () => {
      try {
        const result = await searchFiles({
          query: keyword,
          extensions,
          includeDirs: preset.includeDirs,
          includeFiles: preset.includeFiles,
          includeHidden,
          caseSensitive,
          maxResults: 500,
          sortBy,
          sortDesc,
        });
        if (disposed || sequence !== sequenceRef.current) return;
        setResponse(result);
        setSelectedIndex(result.items.length > 0 ? 0 : -1);
      } catch (error) {
        if (disposed || sequence !== sequenceRef.current) return;
        setSearchError(String(error));
        setResponse(null);
        setSelectedIndex(-1);
      } finally {
        if (!disposed && sequence === sequenceRef.current) setSearching(false);
      }
    }, 40);

    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [
    caseSensitive,
    customExt,
    debounced,
    filterKey,
    includeHidden,
    indexReady,
    sortBy,
    sortDesc,
    status?.builtAt,
  ]);

  // 选中项滚动到可视区域
  useEffect(() => {
    if (selectedIndex < 0) return;
    const container = listRef.current;
    if (!container) return;
    const row = container.querySelector<HTMLElement>(`[data-index="${selectedIndex}"]`);
    row?.scrollIntoView({ block: "nearest" });
  }, [selectedIndex]);

  // 右键菜单关闭
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    window.addEventListener("mousedown", close);
    window.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
    };
  }, [menu]);

  // Ctrl+F 聚焦搜索框
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
        event.preventDefault();
        inputRef.current?.focus();
        inputRef.current?.select();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const applyScope = useCallback(
    (next: SearchScope) => {
      setScope(next);
      persist(SEARCH_SCOPE_KEY, next);
      autoStartedRef.current = true;
      setQuery("");
      setDebounced("");
      setResponse(null);
      void rebuild(scopeRoots(next));
    },
    [persist, rebuild],
  );

  const handleScopeSelect = useCallback(
    (value: string) => {
      if (value === "__folder__") {
        openDialog({
          directory: true,
          multiple: false,
          title: t("fileSearch.chooseFolder"),
        })
          .then((selected) => {
            if (typeof selected === "string" && selected) {
              applyScope({ mode: "path", path: selected });
            }
          })
          .catch((error) => console.error("[Search] 选择目录失败:", error));
        return;
      }
      if (value === "all") {
        applyScope({ mode: "all" });
      } else {
        applyScope({ mode: "path", path: value });
      }
    },
    [applyScope, t],
  );

  const handleOpen = useCallback(
    async (item: SearchItem) => {
      try {
        await openSearchItem(item.path);
      } catch (error) {
        toast({
          title: t("fileSearch.openFailed"),
          description: String(error),
          status: "error",
          duration: 2600,
        });
      }
    },
    [t, toast],
  );

  const handleReveal = useCallback(
    async (item: SearchItem) => {
      try {
        await revealSearchItem(item.path);
      } catch (error) {
        toast({
          title: t("fileSearch.openFailed"),
          description: String(error),
          status: "error",
          duration: 2600,
        });
      }
    },
    [t, toast],
  );

  const handleCopyPath = useCallback(
    async (item: SearchItem) => {
      try {
        await navigator.clipboard.writeText(item.path);
        toast({ title: t("fileSearch.copied"), status: "success", duration: 1400 });
      } catch (error) {
        console.error("[Search] 复制失败:", error);
      }
    },
    [t, toast],
  );

  const handleInputKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      if (items.length === 0) return;
      setSelectedIndex((prev) => (prev + 1 >= items.length ? 0 : prev + 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      if (items.length === 0) return;
      setSelectedIndex((prev) => (prev <= 0 ? items.length - 1 : prev - 1));
    } else if (event.key === "Enter") {
      event.preventDefault();
      const item = selectedIndex >= 0 ? items[selectedIndex] : items[0];
      if (!item) return;
      if (event.ctrlKey || event.metaKey) void handleReveal(item);
      else void handleOpen(item);
    } else if (event.key === "Escape") {
      event.preventDefault();
      setQuery("");
      setDebounced("");
    }
  };

  const indexEntries = status?.entries ?? 0;
  const indexingCount = progress?.scanned ?? status?.scanned ?? 0;
  const scopeLabel =
    scope.mode === "all"
      ? `${t("fileSearch.scopeAll")}${drives.length ? ` (${drives.map((d) => d.letter).join(" ")})` : ""}`
      : shortenPath(scope.path, 40);
  const isEmpty = indexReady && debounced.trim().length > 0 && !searching && items.length === 0;

  return (
    <Flex direction="column" gap={3} pb={2}>
      <Box>
        <HStack spacing={2}>
          <FolderSearch size={22} color={activeColor} />
          <Text fontSize="xl" fontWeight="bold" color={titleColor}>
            {t("fileSearch.title")}
          </Text>
          <Badge fontSize="10px" colorScheme="green" borderRadius="full" px={2}>
            BETA
          </Badge>
        </HStack>
        <Text fontSize="sm" color={bodyColor} opacity={0.75} mt={1}>
          {t("fileSearch.subtitle")}
        </Text>
      </Box>

      {/* 搜索框 */}
      <LiquidGlassCard p={0} overflow="hidden">
        <HStack px={4} py={3} spacing={3}>
          <Search size={20} color={activeColor} />
          <Input
            ref={inputRef}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={handleInputKeyDown}
            placeholder={t("fileSearch.placeholder")}
            variant="unstyled"
            fontSize="lg"
            color={bodyColor}
            _placeholder={{ color: subColor }}
            autoFocus
            spellCheck={false}
          />
          {query && (
            <IconButton
              aria-label={t("fileSearch.clear")}
              icon={<X size={16} />}
              size="xs"
              variant="ghost"
              color={subColor}
              onClick={() => {
                setQuery("");
                setDebounced("");
                inputRef.current?.focus();
              }}
            />
          )}
          <Box
            px={2.5}
            py={1}
            borderRadius="md"
            bg={inputBg}
            border="1px solid"
            borderColor={rowBorder}
            flexShrink={0}
          >
            {isIndexing ? (
              <HStack spacing={1.5}>
                <Loader2 size={12} className="animate-spin" color={activeColor} />
                <Text fontSize="xs" color={subColor} sx={{ fontVariantNumeric: "tabular-nums" }}>
                  {indexingCount.toLocaleString()}
                </Text>
              </HStack>
            ) : (
              <HStack spacing={1.5}>
                <HardDrive size={12} color={subColor} />
                <Text fontSize="xs" color={subColor} sx={{ fontVariantNumeric: "tabular-nums" }}>
                  {indexEntries.toLocaleString()}
                </Text>
              </HStack>
            )}
          </Box>
        </HStack>
      </LiquidGlassCard>

      {/* 过滤工具栏 */}
      <LiquidGlassCard px={3} py={2}>
        <HStack spacing={2} flexWrap="wrap" rowGap={2}>
        <HStack spacing={1.5}>
          <Text fontSize="xs" color={bodyColor} flexShrink={0}>
            {t("fileSearch.scope")}
          </Text>
          <CustomSelect
            value={scope.mode === "all" ? "all" : scope.path}
            onChange={handleScopeSelect}
            options={scopeOptions}
            width="220px"
          />
        </HStack>
        <HStack spacing={1.5}>
          <Text fontSize="xs" color={bodyColor} flexShrink={0}>
            {t("fileSearch.formatLabel")}
          </Text>
          <CustomSelect
            value={filterKey}
            onChange={(value) => {
              setFilterKey(value);
              persist(SEARCH_FILTER_KEY, value);
            }}
            options={filterOptions}
            width="120px"
          />
        </HStack>
        <HStack spacing={1.5}>
          <Text fontSize="xs" color={bodyColor} flexShrink={0}>
            {t("fileSearch.sortLabel")}
          </Text>
          <CustomSelect
            value={sortBy}
            onChange={(value) => {
              const next = value as SearchSort;
              setSortBy(next);
              persist(SEARCH_SORT_KEY, next);
            }}
            options={sortOptions}
            width="112px"
          />
          <Tooltip label={sortDesc ? t("fileSearch.sortDesc") : t("fileSearch.sortAsc")}>
            <IconButton
              aria-label={sortDesc ? t("fileSearch.sortDesc") : t("fileSearch.sortAsc")}
              icon={sortDesc ? <ArrowDown size={14} /> : <ArrowUp size={14} />}
              size="sm"
              variant="ghost"
              onClick={() => {
                const next = !sortDesc;
                setSortDesc(next);
                persist(SEARCH_SORT_DESC_KEY, next);
              }}
            />
          </Tooltip>
        </HStack>
        {filterKey === "custom" && (
          <Input
            value={customExt}
            onChange={(event) => {
              setCustomExt(event.target.value);
              persist(SEARCH_CUSTOM_EXT_KEY, event.target.value);
            }}
            placeholder={t("fileSearch.customPlaceholder")}
            size="sm"
            w="240px"
            borderRadius="md"
            bg={inputBg}
            borderColor={rowBorder}
            fontSize="xs"
          />
        )}
        <HStack spacing={1.5}>
          <Switch
            size="sm"
            isChecked={includeHidden}
            onChange={(event) => {
              setIncludeHidden(event.target.checked);
              persist(SEARCH_INCLUDE_HIDDEN_KEY, event.target.checked);
            }}
          />
          <Text fontSize="xs" color={bodyColor} whiteSpace="nowrap">
            {t("fileSearch.includeHidden")}
          </Text>
        </HStack>
        <HStack spacing={1.5}>
          <Switch
            size="sm"
            isChecked={caseSensitive}
            onChange={(event) => setCaseSensitive(event.target.checked)}
          />
          <Text fontSize="xs" color={bodyColor} whiteSpace="nowrap">
            {t("fileSearch.caseSensitive")}
          </Text>
        </HStack>
        <Box flex={1} />
        {isIndexing ? (
          <Tooltip label={t("fileSearch.cancel")}>
            <IconButton
              aria-label={t("fileSearch.cancel")}
              icon={<X size={15} />}
              size="sm"
              variant="ghost"
              onClick={() => {
                void cancelSearchIndex();
              }}
            />
          </Tooltip>
        ) : (
          <Tooltip label={t("fileSearch.rebuild")}>
            <IconButton
              aria-label={t("fileSearch.rebuild")}
              icon={<RefreshCw size={15} />}
              size="sm"
              variant="ghost"
              onClick={() => {
                autoStartedRef.current = true;
                void rebuild(scopeRoots(scope));
              }}
            />
          </Tooltip>
        )}
        </HStack>
      </LiquidGlassCard>

      {buildError && !isIndexing && (
        <HStack
          px={3}
          py={2}
          borderRadius="md"
          bg="rgba(239,68,68,0.12)"
          color="#ef4444"
          spacing={2}
        >
          <AlertCircle size={14} />
          <Text fontSize="xs" flex={1} noOfLines={2}>
            {buildError}
          </Text>
          <IconButton
            aria-label={t("fileSearch.retry")}
            icon={<RefreshCw size={13} />}
            size="xs"
            variant="ghost"
            color="#ef4444"
            onClick={() => {
              autoStartedRef.current = true;
              void rebuild(scopeRoots(scope));
            }}
          />
        </HStack>
      )}

      {/* 结果列表 */}
      <LiquidGlassCard
        p={0}
        overflow="hidden"
        display="flex"
        flexDirection="column"
        flex={1}
        minH="320px"
        h="calc(100vh - 300px)"
      >
        <Flex
          px={4}
          py={2}
          bg={tableHead}
          borderBottom="1px solid"
          borderColor={rowBorder}
          fontSize="xs"
          color={subColor}
          fontWeight="semibold"
        >
          <Box w="34%" minW="160px">
            {t("fileSearch.name")}
          </Box>
          <Box flex={1}>{t("fileSearch.path")}</Box>
          <Box w="90px" textAlign="right">
            {t("fileSearch.size")}
          </Box>
          <Box w="128px" textAlign="right">
            {t("fileSearch.modified")}
          </Box>
        </Flex>

        <Box ref={listRef} flex={1} overflowY="auto" minH="240px">
          {isIndexing ? (
            <Flex direction="column" align="center" justify="center" h="100%" minH="240px" gap={3}>
              <Spinner size="lg" color={activeColor} />
              <Text fontSize="sm" color={bodyColor}>
                {progress?.message || t("fileSearch.building")}
              </Text>
              <Text fontSize="xs" color={subColor} sx={{ fontVariantNumeric: "tabular-nums" }}>
                {t("fileSearch.scanned", { count: indexingCount.toLocaleString() })}
              </Text>
            </Flex>
          ) : !indexReady ? (
            <Flex direction="column" align="center" justify="center" h="100%" minH="240px" gap={3}>
              <HardDrive size={36} color={subColor} />
              <Text fontSize="sm" color={bodyColor}>
                {t("fileSearch.noIndex")}
              </Text>
              <HStack
                as="button"
                px={4}
                py={2}
                borderRadius="md"
                bg={activeColor}
                color="#fff"
                fontSize="sm"
                fontWeight="medium"
                onClick={() => {
                  autoStartedRef.current = true;
                  void rebuild(scopeRoots(scope));
                }}
              >
                <FolderSearch size={15} />
                {t("fileSearch.buildIndex")}
              </HStack>
            </Flex>
          ) : !debounced.trim() ? (
            <Flex direction="column" align="center" justify="center" h="100%" minH="240px" gap={3}>
              <Search size={38} color={subColor} opacity={0.7} />
              <Text fontSize="sm" color={bodyColor}>
                {t("fileSearch.emptyHint")}
              </Text>
              <Text fontSize="xs" color={subColor} maxW="520px" textAlign="center">
                {t("fileSearch.emptyTip")}
              </Text>
            </Flex>
          ) : isEmpty ? (
            <Flex direction="column" align="center" justify="center" h="100%" minH="240px" gap={2}>
              <Search size={32} color={subColor} />
              <Text fontSize="sm" color={bodyColor}>
                {t("fileSearch.noResults")}
              </Text>
              {searchError && (
                <Text fontSize="xs" color="#ef4444">
                  {searchError}
                </Text>
              )}
            </Flex>
          ) : (
            items.map((item, index) => {
              const selected = index === selectedIndex;
              return (
                <Flex
                  key={`${item.path}-${index}`}
                  data-index={index}
                  px={4}
                  py={2}
                  align="center"
                  cursor="pointer"
                  borderBottom="1px solid"
                  borderColor={rowBorder}
                  bg={selected ? getHoverColor(true) : "transparent"}
                  _hover={{ bg: getHoverColor(true) }}
                  onClick={() => setSelectedIndex(index)}
                  onDoubleClick={() => void handleOpen(item)}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    setSelectedIndex(index);
                    setMenu({ x: event.clientX, y: event.clientY, item });
                  }}
                >
                  <HStack w="34%" minW="160px" spacing={2} pr={3}>
                    <TypeIcon item={item} />
                    <Text fontSize="sm" color={bodyColor} noOfLines={1} title={item.name}>
                      <HighlightedName name={item.name} query={query} />
                    </Text>
                  </HStack>
                  <Text flex={1} fontSize="xs" color={subColor} noOfLines={1} title={item.path}>
                    {item.path}
                  </Text>
                  <Text
                    w="90px"
                    textAlign="right"
                    fontSize="xs"
                    color={subColor}
                    sx={{ fontVariantNumeric: "tabular-nums" }}
                    flexShrink={0}
                  >
                    {item.isDir ? "—" : formatFileSize(item.size)}
                  </Text>
                  <Text
                    w="128px"
                    textAlign="right"
                    fontSize="xs"
                    color={subColor}
                    sx={{ fontVariantNumeric: "tabular-nums" }}
                    flexShrink={0}
                  >
                    {formatSearchTime(item.modified)}
                  </Text>
                </Flex>
              );
            })
          )}
        </Box>

        <Flex
          px={4}
          py={2}
          borderTop="1px solid"
          borderColor={rowBorder}
          bg={tableHead}
          align="center"
          gap={3}
          fontSize="xs"
          color={subColor}
        >
          <Text>
            {searching
              ? t("fileSearch.searching")
              : t("fileSearch.results", { count: (response?.total ?? 0).toLocaleString() })}
          </Text>
          {response?.truncated && (
            <Text color="#f59e0b">
              {t("fileSearch.truncated", { count: 500 })}
            </Text>
          )}
          {response && <Text>{t("fileSearch.took", { ms: response.tookMs })}</Text>}
          <Text display={{ base: "none", xl: "block" }} noOfLines={1} opacity={0.9}>
            · {scopeLabel}
            {indexReady && status
              ? ` · ${t("fileSearch.indexStats", {
                  files: status.files.toLocaleString(),
                  dirs: status.dirs.toLocaleString(),
                })}`
              : ""}
          </Text>
          {searchError && <Text color="#ef4444">{searchError}</Text>}
          <Box flex={1} />
          <HStack spacing={1} display={{ base: "none", lg: "flex" }}>
            <ArrowUp size={11} />
            <ArrowDown size={11} />
            <Text>{t("fileSearch.hintNavigate")}</Text>
            <Text>·</Text>
            <Text>{t("fileSearch.hintOpen")}</Text>
            <Text>·</Text>
            <Text>{t("fileSearch.hintReveal")}</Text>
          </HStack>
        </Flex>
      </LiquidGlassCard>

      {/* 右键菜单 */}
      {menu && (
        <Box
          position="fixed"
          left={`${Math.min(menu.x, window.innerWidth - 200)}px`}
          top={`${Math.min(menu.y, window.innerHeight - 140)}px`}
          zIndex={1000}
          onMouseDown={(event) => event.stopPropagation()}
        >
          <LiquidGlassCard p={1} minW="180px" boxShadow="xl">
            <Flex
              as="button"
              w="100%"
              px={3}
              py={2}
              borderRadius="md"
              align="center"
              gap={2}
              fontSize="sm"
              color={bodyColor}
              _hover={{ bg: getHoverColor(true) }}
              onClick={() => {
                void handleOpen(menu.item);
                setMenu(null);
              }}
            >
              <ExternalLink size={14} />
              {t("fileSearch.open")}
            </Flex>
            <Flex
              as="button"
              w="100%"
              px={3}
              py={2}
              borderRadius="md"
              align="center"
              gap={2}
              fontSize="sm"
              color={bodyColor}
              _hover={{ bg: getHoverColor(true) }}
              onClick={() => {
                void handleReveal(menu.item);
                setMenu(null);
              }}
            >
              <FolderOpen size={14} />
              {t("fileSearch.reveal")}
            </Flex>
            <Flex
              as="button"
              w="100%"
              px={3}
              py={2}
              borderRadius="md"
              align="center"
              gap={2}
              fontSize="sm"
              color={bodyColor}
              _hover={{ bg: getHoverColor(true) }}
              onClick={() => {
                void handleCopyPath(menu.item);
                setMenu(null);
              }}
            >
              <Copy size={14} />
              {t("fileSearch.copyPath")}
            </Flex>
          </LiquidGlassCard>
        </Box>
      )}
    </Flex>
  );
}
