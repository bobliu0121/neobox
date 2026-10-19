import { useEffect, useLayoutEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { DynamicWallpaperInfo } from "@/lib/dynamic-wallpaper";

/**
 * 动态壁纸窗口页面（GIF / HTML），窗口本身是透明置底、鼠标穿透的。
 *
 * - GIF：`<img>` + `object-fit: cover` 铺满整块屏幕；
 * - HTML：全屏 `<iframe>` 加载 `http://asset.localhost/...`。
 *
 * 这里坚持用 iframe 而不是让窗口顶层导航到 asset URL：WebView2 的
 * `WebResourceRequested` 过滤器不覆盖顶层文档导航，Tauri 的 asset 协议
 * 拦不到这次请求，页面会是空白。iframe 属于子资源请求，能正常命中协议处理器。
 */
export default function WallpaperWindowPage() {
  const [info, setInfo] = useState<DynamicWallpaperInfo | null>(null);
  const [loaded, setLoaded] = useState(false);

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
        style={{ ...fullscreen, pointerEvents: "none", colorScheme: "normal" }}
      />
    );
  }

  if (info.kind === "video") {
    return (
      <video
        key={src}
        src={src}
        autoPlay
        loop
        playsInline
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
