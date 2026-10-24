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
  // HTML 交互设置由后端保管（主窗口写入、本窗口读取）。
  //
  // 注意：不能用 localStorage —— 壁纸窗口是独立 WebView 页面，与主窗口的
  // localStorage 并不共享，读到的永远是空值，会导致设置页开关对壁纸无效。
  const [settings, setSettings] = useState(() => ({
    mouse: true,
    keyboard: false,
    audio: true,
  }));

  // 挂载时从后端拉取当前设置
  useEffect(() => {
    let disposed = false;
    (async () => {
      try {
        const s = await invoke<{ mouse: boolean; keyboard: boolean; audio: boolean }>(
          "get_html_interaction",
        );
        if (!disposed && s) setSettings({ mouse: s.mouse, keyboard: s.keyboard, audio: s.audio });
      } catch (e) {
        console.error("[Wallpaper] 读取 HTML 交互设置失败:", e);
      }
    })();
    return () => {
      disposed = true;
    };
  }, []);

  // 主窗口改动设置时实时同步（后端 set_html_interaction 会广播该事件）
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        const stop = await listen<{ mouse: boolean; keyboard: boolean; audio: boolean }>(
          "wallpaper:interaction",
          (event) => {
            const s = event.payload;
            if (s) setSettings({ mouse: s.mouse, keyboard: s.keyboard, audio: s.audio });
          },
        );
        if (disposed) stop();
        else unlisten = stop;
      } catch (e) {
        console.error("[Wallpaper] 监听交互设置失败:", e);
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
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

  // 交互设置变化时交给后端应用（仅 HTML 壁纸生效）。
  //
  // 后端 `set_html_interaction` 会同时处理：窗口穿透样式、祖先窗口启用、
  // 键盘焦点。这里保留一次兜底调用，覆盖「壁纸窗口比设置先就绪」的时序。
  useEffect(() => {
    if (!info || info.kind !== "html") return;
    invoke("set_wallpaper_interactive", {
      mouse: settings.mouse,
      keyboard: settings.keyboard,
    }).catch((e) => {
      console.error("[Wallpaper] 应用交互设置失败:", e);
    });
  }, [settings.mouse, settings.keyboard, info?.kind]);

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
