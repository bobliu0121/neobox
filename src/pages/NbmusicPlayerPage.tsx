import { useEffect } from "react";
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
    void useMusicStore.getState().init();
  }, []);

  if (!currentSong) {
    return (
      <Center h="100vh" w="100vw">
        <Spinner size="xl" />
      </Center>
    );
  }

  return <ExpandedPlayer onClose={() => {}} playerOnly />;
}
