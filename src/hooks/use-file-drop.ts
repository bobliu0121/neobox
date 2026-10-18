/**
 * 原生文件拖入（Tauri WebView2）。
 *
 * WebView 内 HTML5 drop 拿不到本地绝对路径，因此统一用 Tauri 的
 * onDragDropEvent 获取路径；浏览器预览下退化为 HTML5 事件（仅用于显示拖入态）。
 */
import { useCallback, useEffect, useRef, useState, type DragEvent } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

interface UseFileDropOptions {
  /** 关闭时不监听（例如转换进行中） */
  enabled?: boolean;
}

export function useFileDrop(
  onDrop: (paths: string[]) => void | Promise<void>,
  options: UseFileDropOptions = {},
) {
  const { enabled = true } = options;
  const [isDragging, setIsDragging] = useState(false);
  const onDropRef = useRef(onDrop);
  onDropRef.current = onDrop;

  useEffect(() => {
    if (!enabled || !isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;

    getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "enter" || payload.type === "over") {
          setIsDragging(true);
        } else if (payload.type === "leave") {
          setIsDragging(false);
        } else if (payload.type === "drop") {
          setIsDragging(false);
          if (payload.paths.length > 0) {
            void onDropRef.current(payload.paths);
          }
        }
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch((error) => {
        console.error("[FileDrop] listen failed:", error);
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [enabled]);

  // 浏览器预览 / 原生事件不可用时：阻止浏览器直接打开文件，保持页面不跳转
  const dropProps = useCallback(
    () => ({
      onDragOver: (event: DragEvent) => {
        event.preventDefault();
        setIsDragging(true);
      },
      onDragLeave: () => setIsDragging(false),
      onDrop: (event: DragEvent) => {
        event.preventDefault();
        setIsDragging(false);
      },
    }),
    [],
  )();

  return { isDragging, dropProps };
}
