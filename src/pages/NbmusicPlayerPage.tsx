import { useEffect, useRef } from "react";
import { Center, Spinner } from "@chakra-ui/react";
import { useMusicStore } from "@/stores/music-store";
import { ExpandedPlayer } from "./MusicPage";

/**
 * 双击 `.nbmusic` 时使用的纯播放器页。
 *
 * 只渲染展开播放器本体，没有搜索、歌单、侧边栏，也没有“收起”按钮；
 * 当前歌曲还没导入完成时显示一个加载圈，导入完成后由播放器接管整个窗口。
 */
export default function NbmusicPlayerPage() {
  const currentSong = useMusicStore((s) => s.currentSong);
  const autoOpenedLyricsRef = useRef(false);

  // 纯播放器不挂载 MusicPage，但 openAudioFile / playSong 依赖 store 里的 audioRef。
  // 这里必须自己创建并注册音频元素，否则启动时会一直等不到 audioRef，卡在加载页。
  useEffect(() => {
    const state = useMusicStore.getState();
    const hadAudio = !!state.audioRef;
    const audio = state.audioRef ?? new Audio();

    if (!audio.crossOrigin) audio.crossOrigin = "anonymous";
    if (audio.preload !== "metadata") audio.preload = "metadata";

    if (!hadAudio) {
      audio.addEventListener("ended", () => {
        useMusicStore.getState().nextTrack();
      });
    }

    state.setAudioRef(audio);
    void (async () => {
      await useMusicStore.getState().init();
      // nbmusic 纯播放器固定单曲循环：设置初始化可能读回上次的列表/随机模式，
      // 必须在 init 完成后再锁定一次，并通知桌面歌词隐藏模式切换按钮。
      useMusicStore.setState({ playMode: "one", singleLoopOnly: true });
    })();
  }, []);

  // 双击 .nbmusic 打开的纯播放器默认启用桌面歌词，且只在本次启动自动打开一次。
  // 不能依赖 !desktopLyricsVisible 反复触发：否则用户点播放器按钮关闭后，
  // 该 effect 会立刻又把窗口打开，表现为按钮点了没反应。
  useEffect(() => {
    if (!currentSong || autoOpenedLyricsRef.current) return;
    autoOpenedLyricsRef.current = true;
    if (!useMusicStore.getState().desktopLyricsVisible) {
      void useMusicStore.getState().setDesktopLyricsVisible(true);
    }
  }, [currentSong]);

  if (!currentSong) {
    return (
      <Center h="100vh" w="100vw">
        <Spinner size="xl" />
      </Center>
    );
  }

  return <ExpandedPlayer onClose={() => {}} playerOnly />;
}
