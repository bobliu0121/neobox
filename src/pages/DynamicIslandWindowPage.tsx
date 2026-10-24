/**
 * 桌面灵动岛窗口页面（Tauri 窗口 label=dynamic-island，路由 /dynamic-island）
 *
 * 这个窗口只是「纯渲染器」：灵动岛的状态真源（提示队列、自动关闭计时、持久基线）
 * 全部留在主窗口的 dynamic-island store 里，本页面只做三件事：
 *   1. 接收主窗口推来的快照 island:snapshot → 交给同一套 DynamicIslandHost 渲染动画；
 *   2. 把悬停 / 点击 / 播控以 island:cmd、island:music-control 回传主窗口执行；
 *   3. 常驻置顶 + 空闲鼠标穿透：光标不在岛体上时整窗穿透，避免在屏幕顶部中央
 *      挡出一条永远点不到的区域。
 *
 * 已知视觉差异：透明桌面窗口里 CSS backdrop-filter 只能采样本 WebView 表面（背后是
 * 桌面），拿不到真实折射/模糊，因此岛体使用不透明胶囊底色，见 dynamic-island.tsx 的
 * islandGlass 判定。
 */
import { useEffect, useRef } from "react";
import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { availableMonitors, getCurrentWindow, PhysicalPosition, primaryMonitor } from "@tauri-apps/api/window";
import { useMusicStore } from "@/stores/music-store";
import type { Song } from "@/types/music";
import type { IslandSongLite } from "@/components/ui/island-remote-bridge";
import {
  applyRemoteSnapshot,
  clearRemoteSnapshot,
  notifyIslandIdle,
  ISLAND_HOST_DOM_ID,
  type RemoteIslandSnapshot,
} from "@/components/ui/dynamic-island";
import { DynamicIslandHost } from "@/components/ui/dynamic-island";

/** 必须与 Rust 侧 island_window.rs 的 inner_size 保持一致（逻辑像素） */
const WINDOW_LOGICAL_W = 348;
const WINDOW_LOGICAL_H = 176;
/** 光标进入岛体的判定边距（CSS px），离开时用它 + EXIT_EXTRA 形成回差，避免抖动 */
const HIT_MARGIN = 14;
const HIT_EXIT_EXTRA = 18;

/**
 * 穿透态兜底轮询间隔：穿透状态下窗口收不到任何 pointer/mouse 事件，DOM 完全不知道
 * 光标在哪儿，只能低频问一下后端。600ms 兼顾「光标飘进来后 ≤0.6s 就能点到」的手感
 * 与 IPC 成本（每个 tick 只查 1~2 次，稳定态 ≈1.7 次/秒，原先是 60ms tick ≈33~48 次/秒）。
 */
const FAILSAFE_POLL_MS = 600;
/**
 * 窗口外框物理坐标缓存的有效期。命中判定需要「光标物理坐标 − 窗口物理坐标」，
 * 而外框位置只由本文件的归位逻辑改变（归位后会主动刷新缓存），
 * 没必要每轮都向 Tauri 要一次；取 10s 是为了把平均 IPC 压在 ≤2 次/秒。
 */
const OUTER_POS_REFRESH_MS = 10000;
/** 岛窗口内部事件：内容发生变化，请立刻重新判定一次穿透（不用等兜底轮询） */
const ISLAND_WAKE_EVENT = "island-wake";
/** 本页面根元素 id：指针事件挂在它上面做「离开即恢复穿透」的主路径判定 */
const ISLAND_WINDOW_ROOT_ID = "neobox-island-window-root";
/**
 * 可交互态下的安全兜底间隔：只有「指针已经 2s 没有任何事件」时才会走到这里，
 * 再限流到 2s 一次，防止悬停静止时兜底把 IPC 又拉回高频（最坏 1 次/2s）。
 */
const SAFETY_CHECK_MS = 2000;
/** 窗口被判为隐藏时，每隔多少轮兜底轮询才真正跑一次（≈3s / 次） */
const HIDDEN_POLL_DIVISOR = 5;
/** pointermove 命中判定的节流间隔（ms）：pointermove 可达上百 Hz，不必每次都量布局 */
const MOVE_CHECK_THROTTLE_MS = 80;
/**
 * 显示器几何信息（主屏位置/尺寸）的重查间隔：只有改分辨率 / 插拔显示器才变，
 * 没必要每轮定位校正都重新枚举一遍（那是 2 次 IPC）
 */
const MONITOR_INFO_REFRESH_MS = 60000;

/** 窗口外框物理位置缓存（模块级，不进 React state：只给 IPC 判定用，进 state 会白白重渲染） */
const outerPosCache = { x: 0, y: 0, at: 0 };

interface IslandMusicTrack {
  song: IslandSongLite | null;
  isPlaying: boolean;
  durationSec: number;
  positionSec: number;
}

interface IslandMusicProgress {
  positionSec: number;
  isPlaying: boolean;
}

/**
 * 音乐岛的进度来源占位：主窗口把 currentTime 推过来后写入这里并派发 timeupdate，
 * MusicIslandContent 沿用「监听 audio 元素 timeupdate」的原逻辑即可，无需为跨窗口改造。
 */
class IslandFakeAudio extends EventTarget {
  currentTime = 0;
  duration = 0;
}
const fakeAudio = new IslandFakeAudio();

/**
 * 进度插值基准：主窗口 500ms 推一次进度，岛窗口每 200ms 自行往前推，
 * 否则进度条与时间会明显卡顿甚至不动。
 */
const progressBase = { pos: 0, at: 0, playing: false };

function advanceProgress(pos: number, playing: boolean) {
  progressBase.pos = pos;
  progressBase.at = performance.now();
  progressBase.playing = playing;
  fakeAudio.currentTime = pos;
  fakeAudio.dispatchEvent(new Event("timeupdate"));
}

/**
 * 本窗口的播控全部替换为跨窗口指令：岛窗口没有 <audio>、没有播放队列，
 * 真正的播放动作必须由主窗口的 music-store 执行（见 IslandRemoteBridge）。
 * 首次渲染前同步安装，确保 MusicIslandContent 拿到的是替换后的动作。
 */
function installIslandStoreShim() {
  useMusicStore.setState({
    audioRef: fakeAudio as unknown as HTMLAudioElement,
    togglePlay: () => {
      void emit("island:music-control", { action: "play-pause" });
    },
    prevTrack: () => {
      void emit("island:music-control", { action: "prev" });
    },
    nextTrack: () => {
      void emit("island:music-control", { action: "next" });
    },
    seekTo: (time: number) => {
      // 本地立即钉到目标位置，等主窗口真的跳过去，避免松手瞬间回弹
      advanceProgress(time, progressBase.playing);
      void emit("island:music-control", { action: "seek", value: time });
    },
    externalControl: (action: "play-pause" | "prev" | "next" | "seek", valueMs?: number) => {
      void emit("island:music-control", { action: `external-${action}`, value: valueMs });
    },
  });
  // 封面走主窗口同一套本地代理（同 origin、同 WebView2 数据目录，可直接加载）
  void invoke<number>("cmd_get_proxy_port")
    .then((port) => useMusicStore.setState({ proxyPort: port }))
    .catch(() => {});
}

export default function DynamicIslandWindowPage() {
  const shimInstalled = useRef(false);
  if (!shimInstalled.current) {
    shimInstalled.current = true;
    installIslandStoreShim();
  }

  // 是否有内容正在显示（无内容时永远穿透，不必询问光标）
  const hasContentRef = useRef(false);
  // 当前穿透状态，避免重复调用 set_ignore_cursor_events
  const throughRef = useRef(true);
  // 左键按住中的时间戳：拖动进度条时绝不能中途开启穿透，否则 mouseup 会丢到
  // 下方窗口、拖动状态永远不重置（带 5s 上限，即使漏事件也不会永久挡下点击）
  const mouseDownAtRef = useRef(0);
  // 穿透活动锁：窗口非穿透期间收到 mousemove 就续期（锁定期内禁止切回穿透）。
  // 解决悬停展开动画（岛体 200→320px）中 60ms 轮询的滞后矩形判定导致
  // 「穿透翻转 ↔ notifyIslandIdle ↔ collapse/expand」循环 → 文字反复消失出现。
  // 穿透开启时窗口收不到 mousemove，锁不会误触发；光标真正离开后锁 400ms 过期恢复判定
  const lockThroughUntilRef = useRef(0);

  useEffect(() => {
    const down = () => {
      mouseDownAtRef.current = Date.now();
    };
    const up = () => {
      mouseDownAtRef.current = 0;
    };
    window.addEventListener("mousedown", down);
    window.addEventListener("mouseup", up);
    window.addEventListener("blur", up);
    return () => {
      window.removeEventListener("mousedown", down);
      window.removeEventListener("mouseup", up);
      window.removeEventListener("blur", up);
    };
  }, []);

  // 穿透活动锁：只在非穿透期间（窗口收得到鼠标事件）才会触发，光标在窗内移动即续期
  useEffect(() => {
    const onMove = () => {
      lockThroughUntilRef.current = Date.now() + 400;
    };
    window.addEventListener("mousemove", onMove);
    return () => window.removeEventListener("mousemove", onMove);
  }, []);

  // 强制整窗背景透明：index.css 的 :root{background-color} 会填充 WebView2 导致穿透失效
  useEffect(() => {
    const html = document.documentElement;
    const body = document.body;
    const root = document.getElementById("root");
    const prev = [html.style.background, body.style.background, root?.style.background ?? ""];
    html.style.background = "transparent";
    body.style.background = "transparent";
    if (root) root.style.background = "transparent";
    return () => {
      html.style.background = prev[0];
      body.style.background = prev[1];
      if (root) root.style.background = prev[2];
    };
  }, []);

  // ── 闪烁诊断（验证后可移除）──
  // 1) 渲染停摆探测：正常 60fps 帧间隔 ~16ms，出现 >500ms 间隔 = 渲染被挂起
  //    （Chromium 遮挡误判/后台节流），这正是"不碰也闪、动画抽搐"的直接证据
  // 2) visibilitychange 监控：visibilityState 翻转 = 遮挡计算误判的铁证
  useEffect(() => {
    let last = performance.now();
    let raf = 0;
    const tick = (now: number) => {
      const delta = now - last;
      if (delta > 500) {
        console.warn(`[IslandWindow] 渲染停摆 ${Math.round(delta)}ms @ ${new Date().toISOString()}`);
      }
      last = now;
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    const onVis = () => {
      console.warn(`[IslandWindow] visibility -> ${document.visibilityState} @ ${new Date().toISOString()}`);
    };
    document.addEventListener("visibilitychange", onVis);
    return () => {
      cancelAnimationFrame(raf);
      document.removeEventListener("visibilitychange", onVis);
    };
  }, []);

  // 定位主屏顶部中央；每 10s 兜底校正一次（改分辨率 / 插拔显示器后自动归位）
  useEffect(() => {
    const win = getCurrentWindow();
    let disposed = false;
    // 目标位置缓存：显示器枚举结果只在分辨率/插拔变化时才需要重查，
    // 之前每轮都 availableMonitors + primaryMonitor + outerPosition（3 次 IPC）
    let target: { x: number; y: number } | null = null;
    let targetAt = 0;

    const computeTarget = async (): Promise<{ x: number; y: number } | null> => {
      // 主屏优先；取不到（极端驱动/虚拟屏）时回退到枚举出的第一个显示器
      const all = await availableMonitors();
      const primary = (await primaryMonitor().catch(() => null)) ?? all[0] ?? null;
      if (!primary) return null;
      const dpr = window.devicePixelRatio || 1;
      const wPhys = Math.round(WINDOW_LOGICAL_W * dpr);
      return {
        x: primary.position.x + Math.max(0, Math.round((primary.size.width - wPhys) / 2)),
        y: primary.position.y,
      };
    };

    const place = async () => {
      try {
        if (disposed) return;
        const now = performance.now();
        // 显示器几何信息 60s 才重查一次；其余轮次只用 1 次 IPC 比对当前位置
        if (!target || now - targetAt > MONITOR_INFO_REFRESH_MS) {
          const t = await computeTarget();
          if (disposed || !t) return;
          target = t;
          targetAt = now;
        }
        const cur = await win.outerPosition().catch(() => null);
        if (cur && Math.abs(cur.x - target.x) <= 1 && Math.abs(cur.y - target.y) <= 1) {
          // 位置已正确：同步穿透判定用的外框缓存（免费拿到最新坐标）
          outerPosCache.x = cur.x;
          outerPosCache.y = cur.y;
          outerPosCache.at = now;
          return;
        }
        await win.setPosition(new PhysicalPosition(target.x, target.y));
        // 归位后立刻刷新穿透判定用的外框缓存，避免兜底轮询拿旧坐标算错命中
        outerPosCache.x = target.x;
        outerPosCache.y = target.y;
        outerPosCache.at = performance.now();
        window.dispatchEvent(new Event(ISLAND_WAKE_EVENT));
      } catch (e) {
        console.error("[IslandWindow] 定位失败:", e);
      }
    };

    void place();
    // 位置校正每 10s 一轮（1 次 IPC）：改分辨率 / 插拔显示器后 10s 内自动归位，
    // 相比原先 2s 一轮的 3 次 IPC（≈1.5 次/秒）降到 ≈0.1 次/秒。
    // 不能「隐藏就彻底不跑」：本窗口存在 visibilityState 误报的历史，
    // 一旦误报成 hidden，位置就永远不再校正（换分辨率后岛会跑到屏幕中间），
    // 所以隐藏时每 4 轮（约 40s）仍校正一次，保留自愈能力。
    let placeSkips = 0;
    const timer = setInterval(() => {
      if (document.hidden) {
        placeSkips += 1;
        if (placeSkips % 4 !== 0) return;
      } else {
        placeSkips = 0;
      }
      void place();
    }, 10000);
    return () => {
      disposed = true;
      clearInterval(timer);
    };
  }, []);

  // 接收快照与音乐数据；监听注册完成后才发 island:ready（带退避重发，解决主窗口先发我还没听的问题）
  useEffect(() => {
    const unlisten: UnlistenFn[] = [];
    const cleanups: Array<() => void> = [];
    let cancelled = false;

    (async () => {
      const onSnapshot = await listen<RemoteIslandSnapshot>("island:snapshot", (e) => {
        hasContentRef.current = Boolean(e.payload?.item);
        applyRemoteSnapshot(e.payload);
        // 内容有无直接决定「是否需要询问光标」，立刻让穿透判定重跑一次，
        // 否则新提示出现后要等最多 FAILSAFE_POLL_MS 才能被点到
        window.dispatchEvent(new Event(ISLAND_WAKE_EVENT));
      });
      const onTrack = await listen<IslandMusicTrack>("island:music-track", (e) => {
        const { song, isPlaying, durationSec, positionSec } = e.payload;
        const full: Song | null = song
          ? {
              provider: "island",
              id: song.id,
              name: song.name,
              artist: song.artist,
              artists: song.artists.map((n) => ({ name: n })),
              album: "",
              cover: song.cover,
              duration: song.duration || durationSec || 0,
              fee: 0,
              playable: true,
              language: 0,
            }
          : null;
        fakeAudio.duration = full?.duration ?? 0;
        useMusicStore.setState({
          currentSong: full,
          isPlaying,
          duration: durationSec || full?.duration || 0,
        });
        // 换曲快照自带当前进度，作为本地插值的基准（positionSec 缺失时不覆盖基准）
        if (typeof positionSec === "number") advanceProgress(positionSec, isPlaying);
      });
      const onProgress = await listen<IslandMusicProgress>("island:music-progress", (e) => {
        advanceProgress(e.payload.positionSec, e.payload.isPlaying);
        useMusicStore.setState({ isPlaying: e.payload.isPlaying });
      });
      // 开关关掉时自己立刻退场：不依赖 Rust 的 destroy 时序（销毁可能被拖慢或阻止），
      // 否则桌面上会留一个旧快照的岛、而主窗口内又已经不再推新内容
      const retire = () => {
        hasContentRef.current = false;
        clearRemoteSnapshot();
        void getCurrentWindow().hide().catch(() => {});
      };
      const onSetting = await listen<{ enabled?: boolean }>("island:setting-changed", (e) => {
        if (e.payload?.enabled === false) retire();
      });
      const onClosed = await listen("island:closed", retire);
      if (cancelled) {
        onSnapshot();
        onTrack();
        onProgress();
        onSetting();
        onClosed();
        return;
      }
      unlisten.push(onSnapshot, onTrack, onProgress, onSetting, onClosed);

      // 就绪：请求主窗口把当前快照与音乐数据全量推一份（重建窗口时靠它拿到歌曲数据）
      void emit("island:ready", {});
      // 保活心跳：主窗口的 remoteMode 靠它维持（主窗口重载 / StrictMode 双挂载会丢掉
      // 一次性 ready，那样提示又会退回主窗口内显示）。只证活、不拉数据。
      // 主窗口侧的心跳存活判定阈值是 3s（island-remote-bridge 的 syncAlive），
      // 这里取 2s 仍然有 1s 余量，同时把这条常驻 IPC 从 1 次/秒降到 0.5 次/秒
      const heartbeat = setInterval(() => {
        if (cancelled) {
          clearInterval(heartbeat);
          return;
        }
        void emit("island:alive", {});
      }, 2000);
      cleanups.push(() => clearInterval(heartbeat));
    })();

    return () => {
      cancelled = true;
      unlisten.forEach((fn) => fn());
      cleanups.forEach((fn) => fn());
      void emit("island:closed", {});
    };
  }, []);

  // 进度本地插值：两次推送之间持续往前推，否则进度条/时间会停在上一帧。
  // 这里**故意**保留普通 setInterval 而不用 useVisibleInterval：
  // 1) 它不涉及 IPC，只是往 fakeAudio 派发 timeupdate（成本极低）；
  // 2) 本窗口存在 visibilityState 误报的历史（见上方闪烁诊断），若因误报停表，
  //    进度条会永久冻结在这种「看不见的可见窗口」里，属于可见的 UI 损坏。
  // 未播放时回调首行即返回，暂停状态下没有任何实际工作量。
  useEffect(() => {
    const timer = setInterval(() => {
      if (!progressBase.playing) return;
      const pos = progressBase.pos + (performance.now() - progressBase.at) / 1000;
      fakeAudio.currentTime = pos;
      fakeAudio.dispatchEvent(new Event("timeupdate"));
    }, 200);
    return () => clearInterval(timer);
  }, []);

  /**
   * 空闲鼠标穿透：只有光标落在岛体（含边距）上时才收事件，其余一律穿透到下方窗口。
   *
   * 为什么不再 60ms 轮询：穿透判定每轮要两次跨进程 IPC（光标位置 + 窗口外框），
   * 60ms 一轮意味着每秒 33~48 次 IPC，只为回答「鼠标在不在岛上」。
   * 现在改为两套机制并用：
   *   1) 主路径 = DOM 指针事件：窗口处于「可交互」态时能收到 pointermove，
   *      用 client 坐标 + 岛体 rect（含 HIT_EXIT_EXTRA 回差）即时判定离开，零 IPC；
   *      指针离开窗口（pointerleave）也立刻恢复穿透，避免整窗长期挡住下层点击。
   *   2) 兜底 = 600ms 低频轮询：穿透态下 WebView 收不到任何指针事件，
   *      只能问一次后端光标位置；光标一进命中区就切回可交互，之后交给 1) 接管。
   * 稳态 IPC：穿透态约 1.7 次/秒（每 10s 才额外查一次窗口外框），可交互态 0 次/秒。
   */
  useEffect(() => {
    const win = getCurrentWindow();
    const root = document.getElementById(ISLAND_WINDOW_ROOT_ID);
    let disposed = false;
    let busy = false;
    // 最近一次 DOM 指针活动时间：用于识别「可交互态卡住」（指针已离开但没收到 leave）
    let lastDomActivityAt = 0;
    // 可交互态下兜底查询的限流时间戳：悬停静止时不能被 600ms 定时器带回高频 IPC
    let lastSafetyCheckAt = 0;
    // 隐藏时跳过的轮数：用于把隐藏态的轮询降频而不彻底停摆（见 tick 内注释）
    let hiddenSkips = 0;
    // pointermove 命中判定的节流时间戳（避免高频事件里反复量布局）
    let lastMoveCheckAt = 0;

    const setThrough = async (ignore: boolean) => {
      if (throughRef.current === ignore) return;
      // 开启穿透前补一次「光标已离开」：否则悬停中直接穿透，岛窗口的 mouseleave
      // 永远不会触发 → 主窗口只 hold 不 extend → 提示永不自动收起
      if (ignore) notifyIslandIdle();
      throughRef.current = ignore;
      try {
        await invoke("dynamic_island_set_click_through", { ignore });
      } catch {
        // 回退：直接走 Tauri 窗口 API（命令要求窗口已存在，销毁瞬间可能取不到）
        try {
          await win.setIgnoreCursorEvents(ignore);
        } catch {
          /* ignore */
        }
      }
    };

    /** 拖动中锁定为可交互：拖动进度条时绝不能中途开启穿透，否则 mouseup 丢给下层窗口 */
    const dragging = () =>
      mouseDownAtRef.current !== 0 && Date.now() - mouseDownAtRef.current < 5000;

    /**
     * 物理坐标命中判定（需要 IPC）。外框位置走 OUTER_POS_REFRESH_MS 缓存：
     * 它只在归位逻辑里变，每轮都去问 Tauri 纯属浪费；光标位置每次都要最新的，
     * 所以只有它必须每轮查。需要刷新外框时两者并行发起（不能串行，串行会让单轮耗时翻倍）。
     */
    const hitTestByCursor = async (): Promise<boolean | null> => {
      const el = document.getElementById(ISLAND_HOST_DOM_ID);
      if (!el) return null;
      const dpr = window.devicePixelRatio || 1;
      const r = el.getBoundingClientRect();
      const margin = (throughRef.current ? HIT_MARGIN : HIT_MARGIN + HIT_EXIT_EXTRA) * dpr;
      const now = performance.now();
      let cursor: { x: number; y: number };
      let posX: number;
      let posY: number;
      if (now - outerPosCache.at > OUTER_POS_REFRESH_MS) {
        const [c, p] = await Promise.all([
          invoke<{ x: number; y: number }>("get_cursor_position"),
          win.outerPosition(),
        ]);
        cursor = c;
        outerPosCache.x = p.x;
        outerPosCache.y = p.y;
        outerPosCache.at = now;
        posX = p.x;
        posY = p.y;
      } else {
        cursor = await invoke<{ x: number; y: number }>("get_cursor_position");
        posX = outerPosCache.x;
        posY = outerPosCache.y;
      }
      return (
        cursor.x >= posX + r.left * dpr - margin &&
        cursor.x <= posX + r.right * dpr + margin &&
        cursor.y >= posY + r.top * dpr - margin &&
        cursor.y <= posY + r.bottom * dpr + margin
      );
    };

    const tick = async () => {
      if (disposed || busy) return;
      // 窗口隐藏时降频而不是完全停摆：本窗口历史上出现过 document.visibilityState
      // 误报（见上方闪烁诊断），若隐藏就永久停轮询，会卡在穿透态 → 岛永远点不到。
      // 因此隐藏时每 HIDDEN_POLL_DIVISOR 轮才真正跑一次（≈3s 一次），
      // IPC 相比可见态下降约 83%，同时保留自我纠错能力。
      if (document.hidden) {
        hiddenSkips += 1;
        if (hiddenSkips % HIDDEN_POLL_DIVISOR !== 0) return;
      } else {
        hiddenSkips = 0;
      }
      busy = true;
      try {
        if (dragging()) {
          // 拖动中：锁定为可交互，不做光标判定（也不花 IPC）
          await setThrough(false);
          return;
        }
        if (!hasContentRef.current) {
          // 没有内容时永远穿透，根本不必问光标（省掉全部 IPC）
          await setThrough(true);
          return;
        }
        if (Date.now() < lockThroughUntilRef.current) {
          // 活动锁定期：光标刚在窗内活动过（悬停/展开动画中），强制保持可交互。
          // 动画里 getBoundingClientRect 每帧变化，滞后的矩形判定在岛体边缘极易
          // 翻转穿透引发 collapse/expand 循环，所以锁定期内一律不判
          await setThrough(false);
          return;
        }
        if (!throughRef.current) {
          // 已可交互：离开判定交给 DOM 指针事件（零 IPC）。
          // 只有「长时间没有任何指针事件」才兜底查一次（例如事件链异常断裂），
          // 且兜底本身也限流到 SAFETY_CHECK_MS，避免悬停静止时又退回高频 IPC
          if (performance.now() - lastDomActivityAt < 2000) return;
          if (performance.now() - lastSafetyCheckAt < SAFETY_CHECK_MS) return;
          lastSafetyCheckAt = performance.now();
        }
        const inside = await hitTestByCursor();
        if (inside === null) {
          await setThrough(true);
          return;
        }
        await setThrough(!inside);
      } catch {
        /* 窗口正在销毁 / 光标查询失败：下一轮再试 */
      } finally {
        busy = false;
      }
    };

    /** DOM 侧离开判定：client 坐标与 rect 同为 CSS px，因此边距不乘 dpr */
    const isOutsideIsland = (clientX: number, clientY: number): boolean => {
      const el = document.getElementById(ISLAND_HOST_DOM_ID);
      if (!el) return true;
      const r = el.getBoundingClientRect();
      const m = HIT_MARGIN + HIT_EXIT_EXTRA; // 离开用更大的回差，避免边缘来回抖动
      return (
        clientX < r.left - m ||
        clientX > r.right + m ||
        clientY < r.top - m ||
        clientY > r.bottom + m
      );
    };

    // 主路径：指针在窗内移动（只有非穿透时收得到）。命中岛体什么都不做，
    // 离开岛体（含回差）立刻恢复穿透——一次 IPC 都不用。
    // 做 80ms 节流：pointermove 可能上百 Hz，而 getBoundingClientRect 在悬停动画
    // 期间取的是变化中的布局，没必要每个事件都算一遍；80ms 的滞后肉眼无感。
    // 这里**故意不看** lockThroughUntilRef：那个锁是给旧的 60ms 滞后判定兜底的，
    // 而 DOM 事件拿到的是「此刻」的真实矩形 + 32px 回差，不存在滞后问题；
    // 若也受锁约束，反而会因为每个 mousemove 都在续期锁而永远判不出离开。
    const onPointerMove = (e: PointerEvent) => {
      lastDomActivityAt = performance.now();
      if (throughRef.current || disposed) return;
      if (!hasContentRef.current || dragging()) return;
      const now = performance.now();
      if (now - lastMoveCheckAt < MOVE_CHECK_THROTTLE_MS) return;
      lastMoveCheckAt = now;
      if (isOutsideIsland(e.clientX, e.clientY)) void setThrough(true);
    };

    // 指针离开窗口表面：无论是否在回差带内都要恢复穿透，否则整窗会继续吞掉
    // 下层窗口的点击（整窗可交互时连透明区域也算命中）
    const onPointerLeave = () => {
      lastDomActivityAt = performance.now();
      if (disposed || !hasContentRef.current || dragging()) return;
      void setThrough(true);
    };

    // 指针进入窗口/岛窗口内部唤醒信号：立刻重算一次（不用等 600ms 兜底）
    const onWake = () => {
      lastDomActivityAt = performance.now();
      void tick();
    };

    root?.addEventListener("pointermove", onPointerMove);
    root?.addEventListener("pointerleave", onPointerLeave);
    root?.addEventListener("pointerenter", onWake);
    window.addEventListener(ISLAND_WAKE_EVENT, onWake);

    // 初始一律穿透：启动瞬间光标就算正好在顶部中央，也不该先挡住下面的点击。
    // 这里**绕过** setThrough 的「状态相同就跳过」短路：throughRef 初值就是 true，
    // 走 setThrough(true) 会直接 return，导致窗口实际仍可交互、顶部中央一直挡住点击，
    // 直到光标第一次进出岛体才修正。显式下发一次，让真实状态与 throughRef 对齐。
    throughRef.current = true;
    void (async () => {
      try {
        await invoke("dynamic_island_set_click_through", { ignore: true });
      } catch {
        await win.setIgnoreCursorEvents(true).catch(() => {});
      }
      // 外框缓存预热：失败也无所谓，后续 hitTest 会自行刷新
      try {
        const p = await win.outerPosition();
        outerPosCache.x = p.x;
        outerPosCache.y = p.y;
        outerPosCache.at = performance.now();
      } catch {
        /* ignore */
      }
    })();

    // ref 已绑定，立刻按当前状态判一次（下面定时器第一次触发在 600ms 后，
    // 这里先跑一次让首帧就拿到正确状态）
    void tick();

    /**
     * 兜底轮询调度：600ms 一次（隐藏时 tick 内部自降频到约 3s 一次）。
     * 这里没有直接套 useVisibleInterval，是因为该窗口历史上出现过
     * document.visibilityState 误报（见上方闪烁诊断代码），一旦被判为 hidden
     * 就再也不会切回非穿透 → 岛永远点不到。所以保底定时器始终在跑，
     * 由 tick 内部决定「这一轮要不要真的做」，兼顾省电与自我恢复。
     */
    const pollTimer = setInterval(() => void tick(), FAILSAFE_POLL_MS);

    return () => {
      disposed = true;
      clearInterval(pollTimer);
      root?.removeEventListener("pointermove", onPointerMove);
      root?.removeEventListener("pointerleave", onPointerLeave);
      root?.removeEventListener("pointerenter", onWake);
      window.removeEventListener(ISLAND_WAKE_EVENT, onWake);
    };
  }, []);

  return (
    <div
      id={ISLAND_WINDOW_ROOT_ID}
      style={{
        width: "100vw",
        height: WINDOW_LOGICAL_H,
        background: "transparent",
        overflow: "hidden",
      }}
    >
      <DynamicIslandHost />
    </div>
  );
}
