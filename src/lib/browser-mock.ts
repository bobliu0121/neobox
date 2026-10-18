// 浏览器开发模式下的 Tauri API 模拟。
// 必须在任何业务模块之前导入：App 等模块在顶层调用 getCurrentWindow()，
// 若模拟晚于这些模块求值，会在浏览器中抛出 metadata 相关异常导致白屏。
import { mockIPC, mockWindows, mockConvertFileSrc } from "@tauri-apps/api/mocks";

const NOW = Math.floor(Date.now() / 1000);
const mockSearchItems = [
  { name: "README.md", path: "C:\\Users\\Bobliu\\Desktop\\Git\\neobox\\README.md", isDir: false, size: 4210, modified: NOW - 1800, extension: "md", hidden: false },
  { name: "package.json", path: "C:\\Users\\Bobliu\\Desktop\\Git\\neobox\\package.json", isDir: false, size: 2104, modified: NOW - 7200, extension: "json", hidden: false },
  { name: "search.rs", path: "C:\\Users\\Bobliu\\Desktop\\Git\\neobox\\src-tauri\\src\\search.rs", isDir: false, size: 28412, modified: NOW - 600, extension: "rs", hidden: false },
  { name: "SearchPage.tsx", path: "C:\\Users\\Bobliu\\Desktop\\Git\\neobox\\src\\pages\\SearchPage.tsx", isDir: false, size: 31204, modified: NOW - 300, extension: "tsx", hidden: false },
  { name: "NeoBox-1.0.0-portable.zip", path: "C:\\Users\\Bobliu\\Downloads\\NeoBox-1.0.0-portable.zip", isDir: false, size: 50_974_224, modified: NOW - 86_400, extension: "zip", hidden: false },
  { name: "报告草稿.docx", path: "C:\\Users\\Bobliu\\Documents\\报告草稿.docx", isDir: false, size: 148_224, modified: NOW - 172_800, extension: "docx", hidden: false },
  { name: "桌面壁纸.png", path: "C:\\Users\\Bobliu\\Pictures\\壁纸\\桌面壁纸.png", isDir: false, size: 4_512_000, modified: NOW - 259_200, extension: "png", hidden: false },
  { name: "周杰伦 - 晴天.flac", path: "C:\\Users\\Bobliu\\Music\\周杰伦 - 晴天.flac", isDir: false, size: 32_512_000, modified: NOW - 500_000, extension: "flac", hidden: false },
  { name: "电影合集", path: "C:\\Users\\Bobliu\\Videos\\电影合集", isDir: true, size: null, modified: NOW - 900_000, extension: null, hidden: false },
  { name: "neobox", path: "C:\\Users\\Bobliu\\Desktop\\Git\\neobox", isDir: true, size: null, modified: NOW - 200, extension: null, hidden: false },
  { name: "settings.json", path: "C:\\Users\\Bobliu\\AppData\\Roaming\\NeoBox\\settings.json", isDir: false, size: 12_480, modified: NOW - 60, extension: "json", hidden: true },
  { name: "installer.msi", path: "C:\\Users\\Bobliu\\Downloads\\installer.msi", isDir: false, size: 88_312_000, modified: NOW - 604_800, extension: "msi", hidden: false },
  { name: "logo.svg", path: "C:\\Users\\Bobliu\\Desktop\\Git\\neobox\\public\\logo\\logo.svg", isDir: false, size: 8_120, modified: NOW - 43_200, extension: "svg", hidden: false },
  { name: "main.rs", path: "C:\\Users\\Bobliu\\Desktop\\Git\\neobox\\src-tauri\\src\\main.rs", isDir: false, size: 3_912, modified: NOW - 86_400, extension: "rs", hidden: false },
  { name: "系统日志.txt", path: "C:\\Windows\\Logs\\系统日志.txt", isDir: false, size: 1_024_000, modified: NOW - 3_600, extension: "txt", hidden: false },
  { name: "wallpaper.mp4", path: "C:\\Users\\Bobliu\\Videos\\wallpaper.mp4", isDir: false, size: 120_312_000, modified: NOW - 1_209_600, extension: "mp4", hidden: false },
];

if (typeof window !== "undefined" && !("__TAURI_INTERNALS__" in window)) {
  const npmMockStore = new Map<string, unknown>();
  mockWindows("main");
  mockConvertFileSrc("windows");
  mockIPC(
    (cmd, args) => {
      switch (cmd) {
        case "plugin:store|load":
          return 1;
        case "plugin:store|get_store":
          return 1;
        case "plugin:store|get": {
          const a = args as Record<string, unknown>;
          const v = npmMockStore.get(a.key as string);
          return [v !== undefined ? v : null, v !== undefined];
        }
        case "plugin:store|set": {
          const a = args as Record<string, unknown>;
          npmMockStore.set(a.key as string, a.value);
          return;
        }
        case "plugin:store|has": {
          const a = args as Record<string, unknown>;
          return npmMockStore.has(a.key as string);
        }
        case "plugin:store|delete": {
          const a = args as Record<string, unknown>;
          npmMockStore.delete(a.key as string);
          return;
        }
        case "plugin:store|keys":
          return Array.from(npmMockStore.keys());
        case "plugin:store|clear":
          npmMockStore.clear();
          return;
        case "plugin:store|save":
        case "plugin:store|reload":
          return;
        case "get_system_locale":
          return "zh";
        case "get_system_username":
          return "Bobliu";
        case "get_music_files":
          return [];
        case "search_get_drives":
          return [
            {
              letter: "C:",
              label: "本地磁盘",
              total: 512_000_000_000,
              free: 186_000_000_000,
              fileSystem: "NTFS",
            },
          ];
        case "search_get_status":
        case "search_build_index":
          return {
            ready: true,
            indexing: false,
            entries: 128_432,
            dirs: 8_432,
            files: 120_000,
            roots: ["C:\\"],
            builtAt: Math.floor(Date.now() / 1000),
            elapsedMs: 3_200,
            scanned: 128_432,
            error: null,
          };
        case "search_cancel_index":
        case "search_open_item":
        case "search_reveal_item":
          return;
        case "search_files": {
          const a = args as { options?: { query?: string; extensions?: string[]; includeDirs?: boolean; includeFiles?: boolean; includeHidden?: boolean; sortBy?: string; sortDesc?: boolean } };
          const options = a.options ?? {};
          const keyword = (options.query ?? "").toLowerCase();
          const exts = (options.extensions ?? []).map((ext) => ext.toLowerCase());
          const items = mockSearchItems.filter((item) => {
            if (!options.includeFiles && !item.isDir) return false;
            if (!options.includeDirs && item.isDir) return false;
            if (!options.includeHidden && item.hidden) return false;
            if (exts.length > 0) {
              if (item.isDir) return false;
              if (!item.extension || !exts.includes(item.extension)) return false;
            }
            return item.name.toLowerCase().includes(keyword);
          });
          const sortBy = options.sortBy ?? "name";
          const direction = options.sortDesc ? -1 : 1;
          items.sort((left, right) => {
            const compare = (() => {
              if (sortBy === "size") return (left.size ?? 0) - (right.size ?? 0);
              if (sortBy === "modified") return (left.modified ?? 0) - (right.modified ?? 0);
              if (sortBy === "type") return (left.extension ?? "").localeCompare(right.extension ?? "");
              return left.name.localeCompare(right.name, "zh-CN");
            })();
            return compare === 0 ? left.name.localeCompare(right.name, "zh-CN") : compare * direction;
          });
          return {
            total: items.length,
            items: items.slice(0, 500),
            tookMs: 12,
            truncated: items.length > 500,
            indexEntries: 128_432,
          };
        }
        case "get_hardware":
          return null;
        case "update_overlay_settings":
        case "set_overlay_hotkey":
        case "minimize_to_tray":
        case "exit_app":
        case "install_update":
        case "delete_download_file":
        case "mark_pending_install":
        case "clear_pending_install":
        case "cancel_download":
        case "reset_download_cancel":
          return;
        case "download_update":
          return "downloaded.exe";
        case "get_current_wallpaper":
        case "get_dynamic_wallpaper":
          return null;
        case "set_dynamic_wallpaper": {
          const a = args as Record<string, unknown>;
          return { kind: "gif", path: (a.path as string) ?? "" };
        }
        case "set_wallpaper":
        case "stop_dynamic_wallpaper":
        case "wallpaper_window_ready":
          return;
        default:
          if (cmd.startsWith("plugin:")) return;
          console.warn("[Tauri Mock] Unhandled command:", cmd, args);
          return null;
      }
    },
    { shouldMockEvents: true },
  );
}
