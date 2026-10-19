import { invoke } from "@tauri-apps/api/core";

/** 动态壁纸类型：GIF 逐帧播放；HTML 以完整网页渲染；视频循环播放。 */
export type DynamicWallpaperKind = "gif" | "html" | "video";

export interface DynamicWallpaperInfo {
  kind: DynamicWallpaperKind;
  path: string;
  /** HTML 壁纸的本地服务地址（由后端启动的 127.0.0.1 静态服务器提供） */
  url?: string;
}

/** 持久化 key：应用重启后据此恢复上次的动态壁纸。 */
export const DYNAMIC_WALLPAPER_STORE_KEY = "wallpaper_dynamic_active";

/** 根据扩展名判断文件是否可作为动态壁纸，以及使用哪种渲染方式。 */
export function dynamicWallpaperKind(path: string): DynamicWallpaperKind | null {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  if (ext === "gif") return "gif";
  if (ext === "html" || ext === "htm") return "html";
  if (["mp4", "webm", "mov", "mkv", "avi", "m4v"].includes(ext)) return "video";
  return null;
}

/** 应用动态壁纸（后端负责创建/复用桌面壁纸层窗口）。 */
export async function setDynamicWallpaper(path: string): Promise<DynamicWallpaperInfo> {
  return invoke<DynamicWallpaperInfo>("set_dynamic_wallpaper", { path });
}

/** 停止动态壁纸并销毁壁纸窗口。 */
export async function stopDynamicWallpaper(): Promise<void> {
  await invoke("stop_dynamic_wallpaper");
}

/** 查询当前动态壁纸（窗口重建、页面恢复时使用）。 */
export async function getDynamicWallpaper(): Promise<DynamicWallpaperInfo | null> {
  return invoke<DynamicWallpaperInfo | null>("get_dynamic_wallpaper");
}

/** 壁纸页面首帧就绪后通知后端显示窗口，避免透明窗口先闪黑/白底。 */
export async function notifyWallpaperWindowReady(): Promise<void> {
  await invoke("wallpaper_window_ready");
}
