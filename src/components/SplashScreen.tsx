import { useAppStartup } from "@/contexts/app-startup-context";
import { useThemeColor } from "@/contexts/theme-color-context";
import { Box } from "@chakra-ui/react";
import { useEffect, useRef, useState } from "react";
import { store } from "@/lib/store";

/** 视频启动动画资源（public/splash/startup.mp4，H.264 + AAC，960x720，6.08s） */
const STARTUP_VIDEO = "/splash/startup.mp4";
/** 无自定义 LOGO 时的回退静态图 */
const DEFAULT_LOGO = "/logo/NeoBoxIcon.png";

/**
 * 启动画面：全屏视频动画 + 叠加渲染进度条。
 *
 * 设计要点（均已确认）：
 * 1. **视频铺满全屏**（object-fit: cover），底部叠加一条细进度条，不遮挡主视觉；
 * 2. **视频播完停在最后一帧**，继续等程序启动完成 —— 因为视频只有 6 秒，
 *    而真实启动可能更慢，直接结束会让用户看到一半动画突然消失；
 * 3. **启动很快时也等视频播完**：若程序在视频结束前就绪，动画继续播到自然结束
 *    再淡出，避免「秒开」时动画只闪一下就没了；
 * 4. **播放视频自带声音**：tauri.conf.json 已带 `--autoplay-policy=no-user-gesture-required`，
 *    所以带音频自动播放不会被 WebView2 拦截（已确认该参数存在）；
 * 5. 视频加载/解码失败时**自动回退到静态 LOGO**，不让启动画面变成纯黑。
 *
 * 消失条件（两者同时满足才淡出）：
 *   `startupReady`（程序启动完成） && `videoEnded`（视频播完）
 * 淡出动画结束后由本组件调用 `onFinish`，父级据此卸载自己 —— 而不是让父级
 * 直接按 `isStartupComplete` 卸载，否则「秒开」场景下视频会被瞬间切掉。
 */
interface SplashScreenProps {
  /** 启动任务是否已完成（来自 AppStartup context） */
  startupReady: boolean;
  /** 淡出动画结束后调用，父级据此卸载本组件 */
  onFinish: () => void;
}

export function SplashScreen({ startupReady, onFinish }: SplashScreenProps) {
  const { startupProgress } = useAppStartup();
  const { getActiveColor } = useThemeColor();
  const primaryColor = getActiveColor();

  const videoRef = useRef<HTMLVideoElement | null>(null);
  // 视频是否已就绪（可以播放）
  const [videoReady, setVideoReady] = useState(false);
  // 视频是否已播完
  const [videoEnded, setVideoEnded] = useState(false);
  // 视频是否**确定不可用**（解码失败 / 源缺失 / 自动播放被硬拦）
  // 必须与「还在加载」区分开：加载中不能提前淡出，否则「等视频播完」就失效了
  const [videoFailed, setVideoFailed] = useState(false);
  // 是否已开始淡出
  const [fading, setFading] = useState(false);
  // 防止 onFinish 重复调用
  const finishedRef = useRef(false);
  // 视频最多等待时长（兜底）：超过这个时间仍未就绪，视为不可用，
  // 防止磁盘/解码异常时启动画面无限等待。留足余量让 6s 视频能完整播完。
  const VIDEO_GUARD_MS = 12000;

  // 视频播放状态的就绪/结束由 video 事件驱动，见下方 JSX 上的 onLoadedData/onEnded

  // 同步从 localStorage 读取自定义 LOGO，首帧即显示
  const [logoSrc, setLogoSrc] = useState<string>(() => {
    try {
      return localStorage.getItem("neobox_splash_logo") || DEFAULT_LOGO;
    } catch {
      return DEFAULT_LOGO;
    }
  });

  /**
   * 消失判定：启动完成 **且** 视频播完才淡出。
   *
   * 三种情况的处理：
   * - 视频正常：等 `videoEnded`（播完）才淡出，即使启动早已完成 → 动画完整
   * - 视频确定不可用（`videoFailed`）：只等启动完成，立即淡出，不空等
   * - 视频还在加载（既未 ready 也未 failed）：**继续等**，不能提前淡出
   *
   * 之前的写法 `videoEnded || !videoReady` 是错的：视频尚在加载时 `!videoReady`
   * 为 true，会在启动完成的瞬间就淡出，正好破坏了「启动快也要等视频播完」的需求。
   */
  useEffect(() => {
    if (finishedRef.current) return;
    if (!startupReady) return;

    const canFinish = videoEnded || videoFailed;
    if (!canFinish) return;

    finishedRef.current = true;
    setFading(true);
    console.info(
      `[Splash] 淡出开始 (startupReady=${startupReady} videoEnded=${videoEnded} videoFailed=${videoFailed})`,
    );
    // 与 CSS opacity 过渡时长保持一致（400ms），结束后通知父级卸载
    const timer = window.setTimeout(() => {
      console.info("[Splash] 淡出完成 -> onFinish");
      onFinish();
    }, 420);
    return () => window.clearTimeout(timer);
  }, [startupReady, videoEnded, videoFailed, onFinish]);

  /**
   * 视频兜底超时：超过 VIDEO_GUARD_MS 仍未就绪，按「不可用」处理。
   * 只影响异常场景（文件缺失/解码器故障），正常启动不会走到这里。
   */
  useEffect(() => {
    if (videoReady || videoFailed) return;
    const t = window.setTimeout(() => {
      console.warn(`[Splash] video not ready after ${VIDEO_GUARD_MS}ms -> treat as failed`);
      setVideoFailed(true);
    }, VIDEO_GUARD_MS);
    return () => window.clearTimeout(t);
  }, [videoReady, videoFailed]);

  useEffect(() => {
    try {
      if (localStorage.getItem("neobox_splash_logo")) return;
    } catch {
      /* ignore */
    }
    (async () => {
      const customLogo = await store.get<string>("neobox_splash_logo");
      if (customLogo) setLogoSrc(customLogo);
    })();
  }, []);

  // 启动播放：显式 play() 并把错误吞掉（回退由 videoReady/videoEnded 控制）
  useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    // 从视频开头播放，保证每次启动都完整呈现
    try {
      v.currentTime = 0;
    } catch {
      /* ignore */
    }
    void v.play().catch((e) => {
      // 自动播放被拦截：标记失败并回退到静态 LOGO 分支，
      // 避免「等视频播完」永远等不到而卡住启动画面
      console.warn("[Splash] video play failed:", e);
      setVideoFailed(true);
    });
  }, []);

  return (
    <Box
      w="100vw"
      h="100vh"
      className="neobox-splash"
      position="fixed"
      top="0"
      left="0"
      zIndex="9999"
      overflow="hidden"
      bg="#000"
      data-tauri-drag-region
      // 淡出由 fading 控制（启动完成 && 视频播完之后才开始），
      // 不再用 startupProgress>=100 —— 那样「秒开」时视频会被瞬间切断
      opacity={fading ? 0 : 1}
      transition="opacity 0.4s ease-out"
      willChange="opacity"
    >
      {/* ── 全屏视频层 ───────────────────────────────────────────────
          视频就绪后淡入覆盖静态 LOGO；播完停在最后一帧（不暂停隐藏）。 */}
      <video
        ref={videoRef}
        src={STARTUP_VIDEO}
        // 静音开关交由系统策略：conf 已放开 autoplay 限制，这里不强制 muted
        autoPlay
        playsInline
        preload="auto"
        // 播完保留最后一帧：不 loop、不移除元素
        onLoadedData={() => setVideoReady(true)}
        onCanPlay={() => setVideoReady(true)}
        onEnded={() => setVideoEnded(true)}
        onError={() => {
          // 解码失败：明确标记不可用，回退到静态 LOGO，并且不再等视频播完
          console.warn("[Splash] video error, falling back to static logo");
          setVideoFailed(true);
        }}
        style={{
          position: "absolute",
          inset: 0,
          width: "100%",
          height: "100%",
          objectFit: "cover",
          pointerEvents: "none",
          // 未就绪时透明，让静态 LOGO 兜底可见
          opacity: videoReady ? 1 : 0,
          transition: "opacity 0.35s ease-out",
        }}
      />

      {/* ── 静态 LOGO 兜底层 ─────────────────────────────────────────
          仅在视频未就绪（加载中/解码失败/自动播放被拦）时显示。 */}
      {!videoReady && (
        <Box
          position="absolute"
          inset={0}
          display="flex"
          alignItems="center"
          justifyContent="center"
          data-tauri-drag-region
        >
          <Box
            as="img"
            src={logoSrc}
            alt="NeoBox Logo"
            maxH="46vh"
            maxW="60vw"
            objectFit="contain"
            draggable={false}
            // 轻微呼吸感，避免静态画面看起来像卡死
            sx={{ animation: "neoboxSplashBreath 2.4s ease-in-out infinite" }}
          />
        </Box>
      )}

      {/* ── 底部叠加进度条 ───────────────────────────────────────────
          细线风格、半透明底轨，叠在视频画面上但不抢占视觉。 */}
      <Box
        position="absolute"
        bottom="6%"
        left="50%"
        transform="translateX(-50%)"
        // 视觉宽度适度加宽（比原来 160px 更接近全屏动画的比例）
        w={{ base: "60%", md: "42%" }}
        maxW="420px"
        zIndex="10"
        pointerEvents="none"
      >
        <Box
          bg="rgba(255,255,255,0.18)"
          h="3px"
          borderRadius="full"
          overflow="hidden"
          boxShadow="0 0 12px rgba(0,0,0,0.45)"
        >
          <Box
            h="100%"
            bg={primaryColor}
            w={`${Math.min(100, Math.max(0, startupProgress))}%`}
            transition="width 200ms ease-out"
            willChange="width"
            // 进度条自身带一点辉光，叠加在深色视频上更清晰
            boxShadow={`0 0 8px ${primaryColor}`}
          />
        </Box>
        {/* 视频播完后显示当前百分比（此时动画已结束，用户需要知道还在等什么）。
            淡出期间不再显示，避免和「正在消失」的画面打架。 */}
        {videoEnded && !fading && (
          <Box
            textAlign="center"
            mt={2}
            fontSize="11px"
            letterSpacing="0.08em"
            color="rgba(255,255,255,0.55)"
          >
            {Math.min(99, Math.round(startupProgress))}%
          </Box>
        )}
      </Box>

      {/* 呼吸动画 keyframes（静态 LOGO 兜底时使用） */}
      <style>{`
        @keyframes neoboxSplashBreath {
          0%, 100% { transform: scale(1); opacity: 0.92; }
          50% { transform: scale(1.035); opacity: 1; }
        }
      `}</style>
    </Box>
  );
}
