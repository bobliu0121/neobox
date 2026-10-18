import { invoke } from "@tauri-apps/api/core";

export interface SearchDrive {
  letter: string;
  label: string;
  total: number;
  free: number;
  fileSystem: string;
}

export interface SearchStatus {
  ready: boolean;
  indexing: boolean;
  entries: number;
  dirs: number;
  files: number;
  roots: string[];
  builtAt: number;
  elapsedMs: number;
  scanned: number;
  error: string | null;
}

export interface SearchProgress {
  indexing: boolean;
  scanned: number;
  message: string;
  done: boolean;
  error: string | null;
}

export interface SearchItem {
  name: string;
  path: string;
  isDir: boolean;
  size: number | null;
  modified: number | null;
  extension: string | null;
  hidden: boolean;
}

export interface SearchResponse {
  total: number;
  items: SearchItem[];
  tookMs: number;
  truncated: boolean;
  indexEntries: number;
}

export interface SearchOptions {
  query: string;
  extensions: string[];
  includeDirs: boolean;
  includeFiles: boolean;
  includeHidden: boolean;
  caseSensitive: boolean;
  maxResults: number;
}

export type SearchScope =
  | { mode: "all" }
  | { mode: "path"; path: string };

export const SEARCH_SCOPE_KEY = "neobox_file_search_scope";
export const SEARCH_FILTER_KEY = "neobox_file_search_filter";
export const SEARCH_INCLUDE_HIDDEN_KEY = "neobox_file_search_include_hidden";
export const SEARCH_CUSTOM_EXT_KEY = "neobox_file_search_custom_ext";

export interface SearchFilterPreset {
  key: string;
  includeDirs: boolean;
  includeFiles: boolean;
  extensions: string[];
}

const preset = (
  key: string,
  extensions: string[],
  includeDirs = false,
  includeFiles = true,
): SearchFilterPreset => ({ key, extensions, includeDirs, includeFiles });

export const SEARCH_FILTER_PRESETS: SearchFilterPreset[] = [
  preset("all", [], true, true),
  preset("folders", [], true, false),
  preset(
    "documents",
    "doc docx xls xlsx ppt pptx pdf txt md rtf csv odt ods epub".split(" "),
  ),
  preset(
    "images",
    "jpg jpeg png gif bmp webp svg ico tif tiff heic avif psd ai raw cr2 nef".split(" "),
  ),
  preset(
    "videos",
    "mp4 mkv avi mov wmv flv webm m4v mpg mpeg ts rmvb 3gp".split(" "),
  ),
  preset(
    "audio",
    "mp3 flac wav aac ogg m4a wma ape opus aiff mid".split(" "),
  ),
  preset(
    "archives",
    "zip rar 7z tar gz bz2 xz iso cab zst".split(" "),
  ),
  preset(
    "code",
    "rs ts tsx js jsx mjs cjs py java c cpp h hpp cs go php rb swift kt lua html htm css scss less json xml yml yaml toml ini sh bat ps1 sql vue svelte".split(
      " ",
    ),
  ),
  preset("programs", "exe msi msix appx bat cmd com lnk".split(" ")),
  preset("custom", []),
];

export function getSearchFilter(key: string): SearchFilterPreset {
  return SEARCH_FILTER_PRESETS.find((item) => item.key === key) ?? SEARCH_FILTER_PRESETS[0];
}

export function parseCustomExtensions(input: string): string[] {
  return Array.from(
    new Set(
      input
        .split(/[\s,;，；]+/)
        .map((item) => item.trim().replace(/^\./, "").toLowerCase())
        .filter(Boolean),
    ),
  );
}

export const getSearchDrives = () => invoke<SearchDrive[]>("search_get_drives");

export const getSearchStatus = () => invoke<SearchStatus>("search_get_status");

export const buildSearchIndex = (roots: string[] | null) =>
  invoke<SearchStatus>("search_build_index", { roots });

export const cancelSearchIndex = () => invoke<void>("search_cancel_index");

export const searchFiles = (options: SearchOptions) =>
  invoke<SearchResponse>("search_files", { options });

export const openSearchItem = (path: string) =>
  invoke<void>("search_open_item", { path });

export const revealSearchItem = (path: string) =>
  invoke<void>("search_reveal_item", { path });

export function scopeRoots(scope: SearchScope): string[] | null {
  return scope.mode === "path" ? [scope.path] : null;
}

export function formatFileSize(bytes: number | null): string {
  if (bytes === null || bytes === undefined) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 100 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
}

export function formatSearchTime(seconds: number | null): string {
  if (!seconds) return "—";
  const date = new Date(seconds * 1000);
  const now = Date.now();
  const diff = now - date.getTime();
  if (diff < 86_400_000 && diff >= 0) {
    return date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  }
  return date.toLocaleDateString(undefined, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  });
}

export function fileTypeOf(item: SearchItem): string {
  if (item.isDir) return "folder";
  const ext = (item.extension ?? "").toLowerCase();
  if (!ext) return "file";
  if (["jpg", "jpeg", "png", "gif", "bmp", "webp", "svg", "ico", "tif", "tiff", "heic", "avif", "psd"].includes(ext)) return "image";
  if (["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "ts"].includes(ext)) return "video";
  if (["mp3", "flac", "wav", "aac", "ogg", "m4a", "wma", "ape", "opus"].includes(ext)) return "audio";
  if (["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "iso", "cab", "zst"].includes(ext)) return "archive";
  if (["exe", "msi", "msix", "appx", "bat", "cmd", "com", "lnk"].includes(ext)) return "program";
  if (["rs", "ts", "tsx", "js", "jsx", "py", "java", "c", "cpp", "h", "cs", "go", "php", "html", "css", "json", "xml", "yml", "yaml", "toml", "sh", "ps1", "sql"].includes(ext)) return "code";
  if (["doc", "docx", "xls", "xlsx", "ppt", "pptx", "pdf", "txt", "md", "rtf", "csv", "epub"].includes(ext)) return "document";
  return "file";
}
