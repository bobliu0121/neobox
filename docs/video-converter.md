# 视频格式转换

路由：`/video-converter`（内置工具 → 视频转换）。后端：`src-tauri/src/video_convert.rs`。

## 支持格式

| 输出 | 说明 |
| --- | --- |
| GIF | 逐帧抽帧生成动图，可设置帧率、宽度、开始时间、截取时长 |
| PNG / JPG / WebP | 截取指定时间点的首帧静态图 |
| M4A / AAC | 提取视频中的音轨（系统 Media Foundation 编码） |
| WAV | 提取音轨并直接写 PCM，不依赖系统编码器 |

## 实现要点

- 支持把视频 / 音频文件直接拖到页面任意位置，自动填入输入路径。
- 转换任务运行在全局后台 store：点击转换后可立即切换到其他页面，任务不会中断；
  完成后通过灵动岛提示，返回页面仍能看到结果。
- 视频解码通过 Windows Media Foundation 的 Source Reader 完成。设置
  `MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING` 后，任意受系统支持的视频格式都会
  统一转成 RGB32；`MF_MT_FRAME_SIZE` 请求输出尺寸，若解码器不接受则回退到原始
  尺寸，再由 `image` crate 做软件缩放。
- 抽帧按样本时间戳（100ns）计算，而非按帧序号，因此可变帧率视频也能保持节奏。
- GIF 编码使用 `image::codecs::gif`，最多 1800 帧；UI 默认截取 10 秒、480px 宽，
  避免生成过大的文件。
- 不依赖外部 ffmpeg；部分精简版 Windows 若缺少 MP3 编码器，导出 MP3 可能失败，
  可改用 M4A / WAV。

## 限制

- 需要系统安装对应视频解码器（H.264 / H.265 / VP9 等通常由 Windows 自带，
  部分专业格式需额外安装解码器）。
- 动画壁纸 GIF 建议控制在 10–20 秒、480–720px 宽。
