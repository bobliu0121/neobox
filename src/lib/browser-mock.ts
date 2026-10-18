// 浏览器开发模式下的 Tauri API 模拟。
// 必须在任何业务模块之前导入：App 等模块在顶层调用 getCurrentWindow()，
// 若模拟晚于这些模块求值，会在浏览器中抛出 metadata 相关异常导致白屏。
import { mockIPC, mockWindows, mockConvertFileSrc } from "@tauri-apps/api/mocks";

const NOW = Math.floor(Date.now() / 1000);

// 本地音乐示例：用于浏览器预览「本地导入的歌单」子文件夹与本地搜索
const mockLocalFolders = [
  { id: "C:\\Music\\周杰伦", name: "周杰伦", path: "C:\\Music\\周杰伦", recursive: true, importedAt: NOW - 86_400 },
  { id: "C:\\Music\\OST", name: "OST", path: "C:\\Music\\OST", recursive: false, importedAt: NOW - 172_800 },
];
const mockLocalSongs = [
  { provider: "local", id: "C:\\Music\\周杰伦\\晴天.flac", name: "晴天", artist: "周杰伦", artists: [{ name: "周杰伦" }], album: "叶惠美", cover: "", duration: 269_000, fee: 0, playable: true, language: 0, hash: "C:\\Music\\周杰伦\\晴天.flac", _localPath: "C:\\Music\\周杰伦\\晴天.flac", _localFolderId: mockLocalFolders[0].id, _localHasLyric: true },
  { provider: "local", id: "C:\\Music\\周杰伦\\七里香.flac", name: "七里香", artist: "周杰伦", artists: [{ name: "周杰伦" }], album: "七里香", cover: "", duration: 299_000, fee: 0, playable: true, language: 0, hash: "C:\\Music\\周杰伦\\七里香.flac", _localPath: "C:\\Music\\周杰伦\\七里香.flac", _localFolderId: mockLocalFolders[0].id, _localHasLyric: true },
  { provider: "local", id: "C:\\Music\\周杰伦\\夜曲.flac", name: "夜曲", artist: "周杰伦", artists: [{ name: "周杰伦" }], album: "十一月的萧邦", cover: "", duration: 226_000, fee: 0, playable: true, language: 0, hash: "C:\\Music\\周杰伦\\夜曲.flac", _localPath: "C:\\Music\\周杰伦\\夜曲.flac", _localFolderId: mockLocalFolders[0].id, _localHasLyric: false },
  { provider: "local", id: "C:\\Music\\OST\\Main Theme.mka", name: "Main Theme", artist: "Local OST", artists: [], album: "Game OST", cover: "", duration: 185_000, fee: 0, playable: true, language: 0, hash: "C:\\Music\\OST\\Main Theme.mka", _localPath: "C:\\Music\\OST\\Main Theme.mka", _localPlayPath: "C:\\Users\\Bobliu\\AppData\\Local\\NeoBox\\cache\\playback\\abc.m4a", _localFolderId: mockLocalFolders[1].id, _localHasLyric: false },
  { provider: "local", id: "C:\\Music\\OST\\Battle.mka", name: "Battle", artist: "Local OST", artists: [], album: "Game OST", cover: "", duration: 205_000, fee: 0, playable: true, language: 0, hash: "C:\\Music\\OST\\Battle.mka", _localPath: "C:\\Music\\OST\\Battle.mka", _localPlayPath: "C:\\Users\\Bobliu\\AppData\\Local\\NeoBox\\cache\\playback\\def.m4a", _localFolderId: mockLocalFolders[1].id, _localHasLyric: false },
  { provider: "local", id: "C:\\Music\\single.mp3", name: "单曲导入示例", artist: "本地音乐", artists: [], album: "", cover: "", duration: 180_000, fee: 0, playable: true, language: 0, hash: "C:\\Music\\single.mp3", _localPath: "C:\\Music\\single.mp3", _localHasLyric: false },
];
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
          const key = a.key as string;
          if (!npmMockStore.has(key)) {
            if (key === "localSongs") return [mockLocalSongs, true];
            if (key === "localMusicFolders") return [mockLocalFolders, true];
            if (key === "wallpaper_folders") {
              return [
                [
                  { path: "C:\\Users\\Bobliu\\Pictures\\Wallpapers", recursive: true },
                  { path: "C:\\Users\\Bobliu\\Pictures\\Anime", recursive: false },
                ],
                true,
              ];
            }
          }
          const v = npmMockStore.get(key);
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
        case "scan_wallpaper_folder": {
          const a = args as { dir?: string; recursive?: boolean };
          const root = (a.dir ?? "C:\\Pictures").replace(/[\\/]+$/, "");
          const base = `${root}\\sample`;
          return [
            `${base}\\wallpaper-1.jpg`,
            `${base}\\wallpaper-2.png`,
            `${base}\\wallpaper-3.webp`,
          ];
        }
        case "get_release_history":
          return [
            {
              tag: "v1.1.0",
              name: "NeoBox 1.1.0",
              body: "## 音乐\n\n- 搜索不到时明确提示\n- 未登录也可红心，本地保存\n- MKA 双行字幕修复\n- MKA 可拖到 exe 打开\n\n## 导入与后台任务\n\n- 所有导入处支持拖入\n- 转换 / MKA 组合后台完成",
              published_at: "2026-10-18T11:30:00Z",
              html_url: "https://github.com/bobliu0121/neobox/releases/tag/v1.1.0",
              prerelease: false,
            },
            {
              tag: "v1.0.0",
              name: "NeoBox 1.0.0",
              body: "## 首个正式版本\n\n- 全新的文件搜索（类 Everything）\n- 音乐 / 壁纸 / 内置工具多项改进\n- 修复若干稳定性问题",
              published_at: "2026-10-18T08:00:00Z",
              html_url: "https://github.com/bobliu0121/neobox/releases/tag/v1.0.0",
              prerelease: false,
            },
            {
              tag: "v0.9.0",
              name: "NeoBox 0.9.0 (Beta)",
              body: "- 重写动态壁纸渲染\n- 支持 HTML 壁纸\n- 搜索支持按名称/日期/大小排序",
              published_at: "2026-10-01T08:00:00Z",
              html_url: "https://github.com/bobliu0121/neobox/releases/tag/v0.9.0",
              prerelease: true,
            },
          ];
        case "convert_video":
          return {
            output_path: "C:\\Videos\\demo_converted.gif",
            frames: 120,
            width: 480,
            height: 270,
            duration_ms: 10_000,
          };
        case "mux_mka":
          return {
            output_path: "C:\\Music\\demo.mka",
            audio_codec: "A_AAC",
            subtitle_codec: "S_TEXT/UTF8",
            has_cover: true,
            duration_ms: 215_000,
            size: 8_560_000,
          };
        case "path_is_directory": {
          const a = args as { path?: string };
          return !!a.path && !/\.[^.\\/]+$/.test(a.path);
        }
        case "read_image_as_data_url":
          // 1x1 透明 PNG，仅用于浏览器预览时走通拖入背景逻辑
          return "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
        case "read_text_file":
          return "CLASS1 : Effect Type\n9: Version\nPreview\n0: Double Params Flag\n1: Total number of elements\n";
        case "take_pending_open_mka":
          return null;
        default:
          if (cmd.startsWith("plugin:")) return;
          console.warn("[Tauri Mock] Unhandled command:", cmd, args);
          return null;
      }
    },
    { shouldMockEvents: true },
  );
}
