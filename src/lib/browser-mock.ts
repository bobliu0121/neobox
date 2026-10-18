// 浏览器开发模式下的 Tauri API 模拟。
// 必须在任何业务模块之前导入：App 等模块在顶层调用 getCurrentWindow()，
// 若模拟晚于这些模块求值，会在浏览器中抛出 metadata 相关异常导致白屏。
import { mockIPC, mockWindows, mockConvertFileSrc } from "@tauri-apps/api/mocks";

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
