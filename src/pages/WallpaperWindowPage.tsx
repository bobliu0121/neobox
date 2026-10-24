import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { DynamicWallpaperInfo } from "@/lib/dynamic-wallpaper";

/**
 * 动态壁纸窗口页面（GIF / HTML / 视频），窗口本身是透明置底、鼠标穿透的。
 */
export default function WallpaperWindowPage() {
  const [info, setInfo] = useState<DynamicWallpaperInfo | null>(null);
  const [loaded, setLoaded] = useState(false);
  const videoRef = useRef<HTMLVideoElement>(null);
  // 读取壁纸交互设置（与壁纸管理页 localStorage 同步）
  const [settings, setSettings] = useState(() => ({
    mouse: localStorage.getItem("wp_html_mouse") !== "0",
    keyboard: localStorage.getItem("wp_html_keyboard") === "1",
    audio: localStorage.getItem("wp_html_audio") !== "0",
  }));

  // 监听 localStorage 变化（用户在壁纸管理页修改设置时同步更新）
  useEffect(() => {
    const handleStorage = (e: StorageEvent) => {
      if (e.key === "wp_html_audio" || e.key === "wp_html_mouse" || e.key === "wp_html_keyboard") {
        setSettings({
          mouse: localStorage.getItem("wp_html_mouse") !== "0",
          keyboard: localStorage.getItem("wp_html_keyboard") === "1",
          audio: localStorage.getItem("wp_html_audio") !== "0",
        });
      }
    };
    window.addEventListener("storage", handleStorage);
    return () => window.removeEventListener("storage", handleStorage);
  }, []);

  // 动态壁纸窗口必须整页透明：index.css 里 :root 的背景色会铺满整块画布，
  // 不覆盖的话置底窗口会变成一块不透明黑底，看起来就像“GIF 渲染失败”。
  useLayoutEffect(() => {
    document.documentElement.classList.add("wallpaper-window");
    return () => document.documentElement.classList.remove("wallpaper-window");
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    (async () => {
      try {
        const current = await invoke<DynamicWallpaperInfo | null>("get_dynamic_wallpaper");
        if (!disposed) setInfo(current);
      } catch (e) {
        console.error("[Wallpaper] 读取动态壁纸失败:", e);
      }

      try {
        const stop = await listen<DynamicWallpaperInfo>("wallpaper:changed", (event) => {
          setLoaded(false);
          setInfo(event.payload);
        });
        if (disposed) stop();
        else unlisten = stop;
      } catch (e) {
        console.error("[Wallpaper] 监听动态壁纸变更失败:", e);
      }
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // 内容加载失败/长时间无响应时兜底显示窗口，避免壁纸窗口永远隐藏。
  useEffect(() => {
    if (!info || loaded) return;
    const timer = setTimeout(() => setLoaded(true), 4000);
    return () => clearTimeout(timer);
  }, [info, loaded]);

  // 内容就绪后再显示窗口，避免透明窗口先露出一帧空白。
  useEffect(() => {
    if (!info || !loaded) return;
    invoke("wallpaper_window_ready").catch((e) => {
      console.error("[Wallpaper] 显示动态壁纸窗口失败:", e);
    });
  }, [info, loaded]);

  // 鼠标设置变化时动态切换窗口穿透样式（仅 HTML 壁纸生效）
  useEffect(() => {
    if (!info || info.kind !== "html") return;
    invoke("set_wallpaper_interactive", { interactive: settings.mouse }).catch(() => {});
  }, [settings.mouse, info?.kind]);

  // 音频设置变化时实时控制视频音量
  useEffect(() => {
    if (!info || info.kind !== "video") return;
    if (videoRef.current) {
      videoRef.current.muted = !settings.audio;
    }
  }, [settings.audio, info?.kind, loaded]);

  // 音频设置变化时控制 HTML iframe 内的媒体元素音量
  useEffect(() => {
    if (!info || info.kind !== "html") return;
    const muted = !settings.audio;
    // iframe 与主窗口同源（都是 asset://），可直接访问内部 DOM
    try {
      const iframe = document.querySelector("iframe");
      const doc = iframe?.contentDocument || iframe?.contentWindow?.document;
      if (doc) {
        doc.querySelectorAll("audio, video").forEach((el) => {
          (el as HTMLMediaElement).muted = muted;
        });
      }
    } catch {
      // 跨域时静默失败
    }
  }, [settings.audio, info?.kind, loaded]);

  if (!info) return null;

  // HTML 用后端本地服务地址（相对路径资源可用）；GIF 走 Tauri asset 协议
  const src = info.url || convertFileSrc(info.path);
  const fullscreen = {
    position: "fixed" as const,
    inset: 0,
    width: "100vw",
    height: "100vh",
    display: "block",
    border: "none",
    background: "transparent",
    userSelect: "none" as const,
  };

  if (info.kind === "html") {
    return (
      <iframe
        key={src}
        src={src}
        title=""
        allow="autoplay; fullscreen; encrypted-media; picture-in-picture"
        onLoad={() => setLoaded(true)}
        onError={() => setLoaded(true)}
        style={{ ...fullscreen, pointerEvents: settings.mouse ? "auto" : "none", colorScheme: "normal" }}
      />
    );
  }

  if (info.kind === "video") {
    return (
      <video
        ref={videoRef}
        key={src}
        src={src}
        autoPlay
        loop
        playsInline
        muted={!settings.audio}
        onCanPlay={() => setLoaded(true)}
        onError={() => {
          console.error("[Wallpaper] 视频加载失败:", info.path);
          setLoaded(true);
        }}
        style={{ ...fullscreen, objectFit: "cover", pointerEvents: "none" }}
      />
    );
  }

  return (
    <img
      key={src}
      src={src}
      alt=""
      draggable={false}
      onLoad={() => setLoaded(true)}
      onError={() => {
        console.error("[Wallpaper] GIF 加载失败:", info.path);
        setLoaded(true);
      }}
      style={{ ...fullscreen, objectFit: "cover", pointerEvents: "none" }}
    />
  );
}
