import { useCallback, useEffect, useRef } from "react";

/**
 * 「仅在页面可见时运行」的 setInterval。
 *
 * 为什么需要它：本项目大量页面用 setInterval 轮询后端（IPC / PowerShell 采样），
 * 而 WebView 被隐藏（切到别的页面、窗口最小化到托盘、被别的窗口完全遮挡）时
 * 这些轮询毫无意义——用户看不到，却仍在持续唤醒 CPU、串行占用后端命令队列。
 * 另外 Chromium 对隐藏页面本来就会节流定时器，但仍会按 1s 级别唤醒，
 * 主动清掉定时器才是真正的零唤醒。
 *
 * 行为约定：
 * - 不可见（document.hidden）时不注册定时器，也不执行回调；
 * - 恢复可见时**立即执行一次**回调（把暂停期间错过的数据补上，
 *   否则用户切回来的第一秒看到的还是旧数据），然后重新开始计时；
 * - 回调函数用 ref 保存最新引用：调用方常写成内联箭头函数，若直接进依赖数组，
 *   每次 render 都会清掉旧 interval 重建新的，会让「延迟」被无限重置（永远不触发）。
 * - 首次挂载（页面本来可见）同样会立即执行一次，调用方不必再写一遍首拉。
 *
 * @param callback 每次到点执行的动作（可以是 async，返回值被忽略）
 * @param delay 间隔毫秒；<= 0 视为不启用
 * @param enabled 额外开关，false 时完全不跑（如「搜索框有内容时暂停进程列表刷新」）
 * @returns 稳定的手动触发函数：调用它立即执行一次最新回调，
 *          便于「刷新按钮」等场景复用同一份轮询逻辑（不受可见性/enabled 限制）
 */
export function useVisibleInterval(
  callback: () => void,
  delay: number,
  enabled = true,
): () => void {
  // 始终指向最新的 callback，避免调用方每次 render 都重建 interval
  const callbackRef = useRef(callback);
  callbackRef.current = callback;

  useEffect(() => {
    if (!enabled || delay <= 0) return;

    let timer: ReturnType<typeof setInterval> | null = null;

    const stop = () => {
      if (timer !== null) {
        clearInterval(timer);
        timer = null;
      }
    };

    const start = () => {
      stop();
      // 进入可见态先补一次，保证「切回来立刻是新数据」
      if (!document.hidden) {
        callbackRef.current();
        timer = setInterval(() => callbackRef.current(), delay);
      }
    };

    start();

    const onVisibilityChange = () => {
      if (document.hidden) {
        // 不可见：停表，零唤醒
        stop();
      } else {
        // 由隐藏转可见：立即刷新一次并重新计时
        start();
      }
    };

    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => {
      document.removeEventListener("visibilitychange", onVisibilityChange);
      stop();
    };
  }, [delay, enabled]);

  // 用 useCallback 固定身份：调用方把它放进依赖数组也不会每帧变化
  return useCallback(() => callbackRef.current(), []);
}
