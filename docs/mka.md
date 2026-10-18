# MKA 合成与播放

路由：`/mka-muxer`（内置工具 → MKA 合成）。后端：`src-tauri/src/matroska.rs`。

MKA 是基于 Matroska 容器的纯音频文件，可同时携带字幕与封面。NeoBox 不依赖
ffmpeg，自行写入最小可用的 EBML / Matroska 结构。

## 合成

输入：

- 音频（必选）：任意常见音频格式；
- 字幕（可选）：SRT / WebVTT / ASS / SSA；
- 封面（可选）：PNG / JPEG / GIF / WebP；
- 标题（可选）。

处理流程：

1. 音频优先用 Windows Media Foundation 解码并转码为 ADTS AAC（`A_AAC`），
   解析 ADTS 帧后逐帧写入，簇内时间码来自 AAC 1024 采样/帧。
2. 输入本身就是 MP3 时直接解析 MP3 帧并按 `A_MPEG/L3` 复制，避免二次有损。
3. 字幕按格式映射为 Matroska 轨道：
   - SRT → `S_TEXT/UTF8`
   - WebVTT → `S_TEXT/WEBVTT`
   - ASS → `S_TEXT/ASS`，CodecPrivate 保存脚本头，事件按
     `ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,Text` 写入
   - SSA → `S_TEXT/SSA`
4. 封面作为 Matroska Attachment 写入（`cover.jpg` / `cover.png`），
   foobar2000、MPC-HC、VLC 等播放器可直接显示。
5. 写入 Info（时长、TimecodeScale）、Tracks、Attachments、Clusters；
   音频用 SimpleBlock，字幕用 BlockGroup + BlockDuration。

## 在音乐模块打开 MKA

音乐模块导入时会识别 `.mka`：

1. `demux_mka` 解析 Tracks / Clusters / Attachments，提取音频帧与封面；
2. 根据编码写出可播放文件：AAC → ADTS 再封装 M4A，MP3 → 直接拼接帧，
   FLAC → 重建 FLAC 文件，PCM → 包装 WAV；
3. 播放缓存位于应用缓存目录的 `playback/`，以「路径 + 大小 + 修改时间」为键，
   同一文件不会重复转码；
4. 封面写入缓存；歌词优先读取 MKA 同目录的同名 `.lrc` / `.txt`，
   没有时解析 MKA 内嵌字幕轨道并转为 LRC（`[mm:ss.xx]文本`），在音乐播放器的
   歌词面板与桌面歌词中按时间显示。同一条字幕里的多行文本（双行字幕）会用字面量
   `\N` 连接，前端还原为两行显示，不再拼成一行。字幕转换结果缓存在
   `playback/<key>.lyrics-v2.lrc`，首次播放后不再重复解析整个容器。

## 拖到 exe 打开

把 `.mka` 文件直接拖到 `neobox.exe` 上即可打开：便携版通过命令行参数识别路径，
安装版同时注册 `.mka` 文件关联。若 NeoBox 已在运行，会由单实例机制把路径转交给
主窗口，自动导入本地歌库并开始播放。

## 限制

- 解封装只支持无 lacing 的 SimpleBlock / BlockGroup；带 lacing 的音频块会明确报错。
- 从 MKA 播放目前支持 AAC / MP3 / FLAC / PCM；Opus 等编码会提示不支持。
- 合成时会先整体读入音频帧，超大文件（数小时）会占用较多内存。
