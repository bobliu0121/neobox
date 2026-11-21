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

  if (!currentSong) {
    return (
      <Center h="100vh" w="100vw">
        <Spinner size="xl" />
      </Center>
    );
  }

  return <ExpandedPlayer onClose={() => {}} playerOnly />;
}
