# `.nbmusic` 格式设计（v1.0）

> 状态：v1.0 已实现后端读写、播放接入与专用合成/编辑页面。
> 目标：为 NeoBox 音乐模块定义一种可携带多声道音频、封面、带时间戳与逐字进度的字幕、声道级/多音轨字幕、翻译、读音标注、内嵌字体与高潮定位的容器格式。

## 0. 实现状态（v1.0）

已落地 `src-tauri/src/nbmusic.rs`，并通过 `cargo check`、前端 `tsc && vite build`：

* `mux_nbmusic`：把多条音频、字幕轨、封面、字体、高潮标记写入 `.nbmusic`；音频复用现有 Matroska 准备的 AAC/MP3/FLAC/PCM 帧。
* `probe_nbmusic`：返回规格版本、标题/艺术家/专辑、时长、音频轨、字幕轨、声道绑定、读音方案、字体与高潮标记。
* 本地音乐导入：`.nbmusic` 已加入支持扩展名，导入时解封装主音频到播放缓存并提取字幕 LRC 与封面。
* 播放：主音频复用现有 `matroska::write_playable_audio`，AAC 再转 M4A；MKA 与 `.nbmusic` 共用播放缓存机制，但缓存键按格式隔离。
* 字幕：写入 `S_TEXT/NBMUSIC` 轨，轨道级 JSON 存在 `CodecPrivate`，每条 Cue 一个 `BlockGroup + BlockDuration`；支持逐字进度、翻译、读音 segments、样式和内嵌字体引用。
* 高潮：写入全局 Tag `NBMUSIC_CLIMAX`。
* 专用编辑页：`/nbmusic-editor`（内置工具「NBMusic 编辑」），支持新建工程、打开已有 `.nbmusic` 继续编辑、音频轨 / 字幕轨 / 字号附件 / 高潮编辑，并通过 `mux_nbmusic` 另存。
* 编辑回读：`load_nbmusic_project` 把内嵌音频/封面/字体解到临时目录并返回完整 Cue 结构；`parse_nbmusic_subtitle` 复用 MKA 的 SRT / VTT / ASS / SSA 解析器导入字幕，其中 ASS 的 `\k` / `\kf` / `\ko` / `\K` 会转换为逐字 `chars`（`\kf`/`\K` 为 `linear`，`\k`/`\ko` 为 `step`），导入含逐字数据的字幕轨会自动加入 `karaoke` 角色。SRT / VTT / ASS 的双行字幕会拆成 Cue 正文 + 内联 `translations`（第一行为歌词，第二行为翻译），字幕后端导出 LRC 时以 `\N` 连接两行，播放器与桌面歌词显示为原文/翻译两行。
* 当前限制：v1 暂不写 `SeekHead/Cues`；Opus 未纳入基线；字体只做内嵌与缓存，不做子集化；编辑页保存为“另存”语义，不直接覆盖源文件。

## 1. 设计目标

### 必选



* 至少一条音频轨。

* 支持多声道音频，例如 stereo、5.1、7.1、7.1.4、ambisonic。

* 支持多音轨，例如原唱、伴奏、不同语言配音。

* 支持同一音频轨内不同声道承载不同语言/内容，并为每个声道绑定独立字幕。

### 可选



* 封面。

* 带时间戳的字幕。

* 字幕逐字进度。

* 同一音频文件内不同声道的独立字幕，例如左声道中文、右声道日文；也支持多条独立音频轨各自的字幕。

* 翻译。

* 读音 / 音译标注，例如日语 `こんにちは` 标注为 “空你鸡哇”。

* 内嵌字体，用于保证字幕排版在不同设备上一致。

* 高潮定位，支持一个或多个时间点/区间，可标记段落类型、强度与来源。

### 非目标



* 不存储视频画面；封面只作为附件，不作为视频轨。

* 不做 DRM、加密或版权保护。

* 不承载可执行文件、脚本、HTML。

* 不替代通用 MKV；`.nbmusic` 是 NeoBox 的受控音乐 profile。

## 2. 总体方案

物理层复用 **Matroska / EBML**，逻辑层定义 NeoBox 专用 profile。

这样可以直接复用项目现有的 `src-tauri/src/matroska.rs`、MKA 合成 / 播放链路与音频引擎，同时获得：



* 多音轨、多声道封装与声道级字幕绑定；

* 封面附件；

* 内嵌字体与高潮标记的附件/Tag 承载；

* 字幕轨道；

* 时间轴、Cluster、Cues 索引与流式 seek；

* 标准 Matroska 工具的兼容性。

### 2.1 文件标识



| 项目         | 值                     |
| ---------- | --------------------- |
| 扩展名        | `.nbmusic`            |
| MIME       | `audio/x-nbmusic`     |
| 魔数         | `1A 45 DF A3`（EBML 头） |
| 时间基准       | 整数毫秒（1 ms）            |
| 字幕 CodecID | `S_TEXT/NBMUSIC`      |

### 2.2 两种 profile

**兼容模式（默认）**



* `DocType = "matroska"`

* 用强制 Tag `NBMUSIC_SPEC=1.0` 识别。

* 普通播放器通常仍能播放其中的音频；自定义字幕轨会被忽略或提示未知 codec。

**严格模式（可选）**



* `DocType = "nbmusic"`。

* 只有 NeoBox 解析，格式边界更干净。

* 适合内部归档、避免被外部工具误改。

## 3. 文件结构



```
EBML Header
└─ Segment
   ├─ SeekHead                 # 指向 Info / Tracks / Tags / Attachments / Cues
   ├─ Info                     # TimestampScale、Duration、标题、编码器
   ├─ Tracks
   │  ├─ Audio Track 1..N      # 必选，至少一条
   │  └─ Subtitle Track 1..N   # 可选，S_TEXT/NBMUSIC
   ├─ Tags                     # NBMUSIC 规范标识、全局元数据、轨道关联、高潮标记
   ├─ Attachments              # 封面、可选字体
   ├─ Clusters                 # 音频块 + 字幕 cue 块
   └─ Cues                     # 音频/字幕 seek 索引
```

### 3.1 Info



* `TimestampScale = 1,000,000`（纳秒单位，即 1 ms）。

* `Duration` 为音频总时长。

* `Title`、`MuxingApp`、`WritingApp` 可选。

### 3.2 Tracks



* 音频轨：`TrackType = 2`。

* 字幕轨：`TrackType = 0x11`，`CodecID = S_TEXT/NBMUSIC`。

* 不允许视频轨；封面走 Attachments。

### 3.3 Tags



* 全局 Tag：规范版本、文件 UUID、封面关联、全局歌词偏移、内嵌字体清单、全局高潮标记、通用音乐元数据。

* 轨道级 Tag：声道布局、声道承载内容、字幕关联音频轨与声道、字幕角色、读音体系、翻译源轨。

### 3.4 Attachments



* 封面图片。

* 可选字体文件：TTF / OTF / WOFF / WOFF2，用于特殊字幕排版。

* 字体附件必须带字体名称、许可信息与可选子集范围；与 `NBMUSIC_FONTS` Tag 一一对应，播放器按 `fontFamily` 或 `fontAttachmentUid` 查找。

* 附件必须使用安全的文件名，禁止路径分隔符与 `..`。

### 3.5 Clusters



* 音频与字幕按时间交错写入。

* 建议每 1–5 秒一个 Cluster，兼顾 seek 与文件体积。

* 音频使用 SimpleBlock 或 BlockGroup；字幕使用 BlockGroup + `BlockDuration`。

### 3.6 Cues



* 音频 seek 索引。

* 字幕 Cue 也写入 Cues，支持按时间直接定位。

* 可选 fast-start：把 Cues、Tags、封面附件前移，通过 SeekHead 指向后置 Cluster。

## 4. 逻辑数据模型

逻辑模型用于描述语义。实际文件不要求存在一个完整的 `manifest.json`；各字段映射到 Matroska 元素、Tags、Attachments 与自定义字幕轨 JSON。

### 4.1 文档级



```
interface NBMusicDocument {
  spec: "nbmusic/1.0";
  id: string;                 // UUID，文件唯一标识
  timebase: "ms";
  durationMs: number;
  metadata: Metadata;
  audioTracks: AudioTrack[];  // 至少 1 条
  subtitleTracks?: SubtitleTrack[];
  covers?: Cover[];
  fonts?: FontAttachment[];
  climaxes?: ClimaxMarker[];
}
```

### 4.2 音频轨



```
interface AudioTrack {
  uid: string;                // Matroska TrackUID 的十进制字符串
  codec: "aac" | "mp3" | "flac" | "pcm_s16le" | "pcm_s24le" | "opus";
  sampleRate: number;         // 如 48000
  channels: number;           // 1..32
  channelLayout:
    | "mono"
    | "stereo"
    | "2.1"
    | "5.1"
    | "7.1"
    | "7.1.4"
    | "ambisonic"
    | `x-${string}`;
  channelOrder?: string;      // ambisonic 可用，如 "ACN"、"FuMa"
  channelPrograms?: ChannelProgram[]; // 可选：说明每个声道承载的内容/语言
  bitDepth?: number;          // PCM / FLAC 用
  language?: string;          // BCP 47，如 "zh-CN"、"en-US"
  title?: string;             // "原唱"、"伴奏"、"英文版"
  default?: boolean;
  startOffsetMs?: number;     // 相对时间轴 0 的起始偏移，通常为 0
  replayGain?: ReplayGain;
  flags?: ("original" | "instrumental" | "dub" | "acapella")[];
}

interface ChannelProgram {
  channels: number[];         // 1-based 声道序号，如 [1] 表示左声道
  language?: string;          // 该声道承载的语言
  title?: string;             // 如 "中文"、"日本語"
  role?: "main" | "dual-language" | "commentary" | `x-${string}`;
}
```

规则：



* `audioTracks.length >= 1`。

* 所有音轨共享同一条时间轴；时间 0 为第一条音频的首个采样。

* 所有 TrackUID 在文件内唯一。

* 每条音频轨的 `channels` 必须与 `channelLayout` 匹配。

* 多声道指单轨 5.1/7.1 等；多音轨指原唱、伴奏、不同语言等多条 Track，两者都支持。

* 同一条音频轨的不同声道可以承载不同内容，例如左声道中文、右声道日文；此时用 `channelPrograms` 描述声道内容，字幕通过 `audioBindings` 按声道绑定。

* `channelPrograms` 中的声道序号必须落在 `1..channels` 范围内；同一段音频的声道可以重叠声明，但语言/标题应保持一致。

* 若同一音频轨的声道语言不同，`AudioTrack.language` 可写 `"mul"` 或省略，具体语言放到 `channelPrograms`。

v1.0 编码基线：



| 编码   | 声道约束    | 说明                            |
| ---- | ------- | ----------------------------- |
| AAC  | 1–8 声道  | 5.1 / 7.1 常用；与现有 MKA 链路一致     |
| MP3  | 1–2 声道  | 仅 stereo /mono                |
| FLAC | 1–8 声道  | 无损；7.1 以内                     |
| PCM  | 1–32 声道 | 7.1.4、ambisonic 等超过 8 声道时必须使用 |
| Opus | 可选扩展    | 需要播放器先支持；不作为 v1.0 必需能力        |

### 4.3 字幕轨



```
interface AudioBinding {
  trackUid: string;           // 对应 AudioTrack.uid
  channels?: number[];        // 1-based 声道序号；省略表示整条音轨
  labels?: string[];          // 可选语义标签，如 ["L"]、["R"]、["C"]
}

interface SubtitleTrack {
  uid: string;
  audioBindings: AudioBinding[]; // 这条字幕对应哪些音轨/声道
  roles: SubtitleRole[];
  language: string;           // 字幕文本语言，BCP 47
  title?: string;
  default?: boolean;
  forced?: boolean;
  offsetMs?: number;          // 整轨字幕校正偏移，可正可负
  pronunciationScheme?: PronunciationScheme;
  sourceSubtitleUid?: string; // 翻译/读音轨指向源字幕轨
  cues: Cue[];
}

type SubtitleRole =
  | "original"       // 原文
  | "translation"    // 翻译
  | "pronunciation"  // 读音 / 音译 / 空耳
  | "karaoke"        // 逐字进度
  | "description"    // 画面描述 / SDH
  | `x-${string}`;
```

规则：



* 每条字幕轨的 `audioBindings` 必须非空，且引用的 `trackUid` 存在。

* `audioBindings[].channels` 使用 1-based 声道序号；省略时表示整条音轨。stereo 下 `[1]` 为左声道，`[2]` 为右声道，`[1,2]` 为双声道。

* `audioBindings[].channels` 的每个序号必须落在 `1..AudioTrack.channels` 范围内；`labels` 只作语义提示，不能替代序号。

* 同一条字幕轨可以服务多条音频轨或多个声道，例如原唱与伴奏共享歌词，或左右声道都要显示同一句翻译。

* 不同配音版本歌词不同，或同一音频轨左右声道内容不同，必须拆成不同字幕轨，用 `audioBindings` 区分。

* 同一音频轨的每种 role + language + 声道组合最多一个 `default`。

* 翻译轨的 `sourceSubtitleUid` 必须指向同文件内存在的字幕轨。

### 4.4 Cue（一行字幕）



```
interface Cue {
  id?: string;                // 轨内唯一；被翻译轨引用时必填
  startMs: number;
  endMs: number;              // 左闭右开 [startMs, endMs)
  text: string;
  chars?: CharSpan[];         // 可选逐字进度
  pronunciations?: Pronunciation[];
  translations?: InlineTranslation[];
  style?: CueStyle;
}
```

规则：



* `endMs > startMs`。

* 时间落在音频总时长范围内；允许尾部少量余量，由播放器裁剪。

* `id` 在单条字幕轨内唯一。

* 有 `chars` 时，逐字数据必须落在 Cue 区间内。

字幕样式：

```ts
interface CueStyle {
  fontFamily?: string;
  fontAttachmentUid?: string; // 优先使用指定内嵌字体
  fontWeight?: number;        // 100..900
  fontStyle?: "normal" | "italic";
  fontSizePx?: number;
  color?: string;             // #RRGGBB / #RRGGBBAA
  align?: "left" | "center" | "right";
  position?: "top" | "middle" | "bottom";
  marginPx?: number;
}
```

规则：

* `fontAttachmentUid` 存在时必须指向有效的 `FontAttachment`；否则按 `fontFamily` 匹配内嵌字体。
* 两者都不存在或字体缺失时，回退到系统字体，不阻断播放。
* 字体子集必须覆盖该 Cue 实际使用到的字符；缺字时允许回退。

### 4.5 逐字进度



```
interface CharSpan {
  text: string;               // 一个或多个字符
  startMs: number;
  endMs: number;
  mode?: "step" | "linear";   // 默认 step：到点整字高亮；linear：逐字扫过
  reading?: string;           // 一字一音时使用，如 "月" -> "yuè"
  progress?: ProgressPoint[]; // 可选，非匀速进度关键点
  confidence?: number;        // 0..100，自动对齐置信度
}

interface ProgressPoint {
  atMs: number;
  ratio: number;              // 0..1
}
```

规则：



* 有 `chars` 时，`chars[].text` 顺序拼接必须等于 `text`（空格、标点也算）。

* `chars` 内时间必须单调、不重叠，且落在 Cue 的 `[startMs, endMs)` 内。

* 不需要逐字时省略 `chars`，只保留行级时间戳。

* 逐字进度默认由 `startMs`、`endMs` 与 `mode` 推导。

* 需要渐强、停顿、拖长音时，用 `progress` 关键点描述非线性进度。

ASS 导入映射：`\kf` / `\K` → `mode: "linear"`，`\k` / `\ko` → `mode: "step"`；标签时长按 ASS 的厘秒单位换算为毫秒，标签对应的文本片段作为 `chars[].text`。

### 4.6 读音 / 音译

这里的 “音调” 已明确为**读音标注**，不是音乐音高。



```
type PronunciationScheme =
  | "pinyin"        // 拼音：yuè liàng
  | "zhuyin"        // 注音：ㄩㄝˋ ㄌㄧㄤˋ
  | "romaji"        // 罗马字：konnichiha
  | "ipa"           // 国际音标
  | "kana"          // 假名读音
  | "zh-phonetic"   // 中文谐音 / 空耳：空你鸡哇
  | `x-${string}`;

interface Pronunciation {
  scheme: PronunciationScheme;
  lang?: string;              // 读音文本语言，如 "zh-CN"
  text: string;               // 整行读音
  segments?: PronunciationSegment[];
}

interface PronunciationSegment {
  source: string;             // 原文片段，如 "こん"
  reading: string;            // 对应读音，如 "空"
  startMs?: number;
  endMs?: number;
}
```

说明：



* `zh-phonetic` 专门表示 “用汉字近似外语发音”，即 “空你鸡哇” 这类写法。

* `romaji` 表示标准罗马字，如 `konnichiha`。

* `ipa` 表示音标。

* `segments` 用于原文与读音非一字对一字的场景。

* 不需要逐段对齐时，只写 `text` 即可。

* 中文等一字一音场景，也可以直接用 `chars[].reading`。

校验规则：



* 有 `segments` 时，`segments[].source` 顺序拼接必须等于 `text`。

* `segments[].reading` 顺序拼接必须等于 `pronunciation.text`。

* 段时间必须单调、不重叠，并落在 Cue 区间内。

* 无 `segments` 时只做整行读音，不要求逐字对齐。

* `pronunciationScheme` 与 Cue 内每个 `Pronunciation.scheme` 不一致时，以 Cue 内的为准。

### 4.7 翻译



```
interface InlineTranslation {
  lang: string;
  text: string;
}
```

两种方式：



1. **推荐**：每种语言单独一条 `role: "translation"` 字幕轨，`sourceSubtitleUid` 指向原文轨，Cue 用 `refCueId` 对齐。

2. **轻量**：直接在原文 Cue 里写 `translations`，适合只有一两句翻译。

翻译轨 Cue：



```
interface TranslationCue extends Cue {
  refCueId?: string;          // 指向源字幕轨的 cue.id
  sourceSubtitleUid?: string;
}
```

翻译不是读音。翻译表达语义，读音表达发音；两者可以同时存在。

### 4.8 封面



```
interface Cover {
  attachmentUid: string;      // Matroska Attachment 的 FileUID
  purpose: "front" | "back" | "artist" | "other";
  mime: "image/jpeg" | "image/png" | "image/webp";
  width?: number;
  height?: number;
  sha256?: string;
}
```

规则：



* 最多一个 `front`；`back`、`artist` 可选。

* 单张 ≤ 20 MB，边长 ≤ 8192。

* 仅允许 JPEG / PNG / WebP。

* 附件文件名建议：`cover-front.jpg`、`cover-back.jpg`、`cover-artist.jpg`。

* 封面不是视频轨，不参与音频时间轴。

### 4.9 元数据



```
interface Metadata {
  title?: string;
  artists?: string[];
  album?: string;
  albumArtist?: string;
  trackNumber?: number;
  discNumber?: number;
  date?: string;
  genre?: string[];
  comment?: string;
  copyright?: string;
  language?: string;
  isrc?: string;
  bpm?: number;
  replayGain?: ReplayGain;
  lyricsOffsetMs?: number;
}

interface ReplayGain {
  trackGainDb?: number;
  trackPeak?: number;
  albumGainDb?: number;
  albumPeak?: number;
}
```

### 4.10 内嵌字体

```ts
interface FontAttachment {
  attachmentUid: string;      // Matroska Attachment FileUID
  fileName: string;           // 如 "NotoSansSC-Regular.otf"
  mime: "font/ttf" | "font/otf" | "font/woff" | "font/woff2";
  family: string;             // 字体族名，供 CueStyle.fontFamily 匹配
  style?: "normal" | "italic";
  weight?: number;            // 100..900
  license?: string;           // SPDX 标识或许可说明
  subset?: string;            // Unicode 范围，如 "U+3040-30FF,U+4E00-9FFF"
  sha256?: string;
}
```

规则：

- 内嵌字体可选；同一文件可包含多个字体族。
- 单个字体 ≤ 50 MB，全部字体总量 ≤ 100 MB。
- 仅允许 TTF / OTF / WOFF / WOFF2；禁止可执行文件和其他附件类型。
- `family`、`style`、`weight` 必须与实际字体元数据一致；`attachmentUid` 必须指向存在的附件。
- `license` 建议填写 SPDX 标识（如 `OFL-1.1`）或完整许可说明；分发前必须确认字体许可允许嵌入。
- `subset` 用于声明子集覆盖范围；省略表示完整字体。播放器必须检查子集是否覆盖实际字幕字符，缺字时回退。
- 播放器只应在内存中加载字体，不得自动安装到系统字体目录。
- 字体缺失或加载失败时回退系统字体，不阻断音频与字幕播放。

### 4.11 高潮定位

```ts
interface ClimaxMarker {
  id?: string;
  trackUid?: string;          // 省略 = 对所有音频轨生效
  startMs: number;            // 高潮区间起点；单点时即高潮点
  endMs?: number;             // 可选，高潮区间终点
  peakMs?: number;            // 最强点/跳转点；默认等于 startMs
  kind?:
    | "chorus"
    | "drop"
    | "solo"
    | "bridge"
    | "finale"
    | "highlight"
    | `x-${string}`;
  label?: string;             // 如 "副歌高潮"、"Drop"
  strength?: number;          // 0..100，用于排序/推荐
  source?: "manual" | "auto" | "imported" | `x-${string}`;
  confidence?: number;        // 0..100，自动识别置信度
}
```

规则：

- 高潮定位可选；一个文件可有 0..N 个标记。
- `startMs` 必须在 `0..durationMs` 内。
- 有 `endMs` 时 `endMs > startMs`；`peakMs` 必须落在 `[startMs, endMs]` 内。
- 无 `endMs` 时表示单点高潮，`peakMs` 默认等于 `startMs`。
- `trackUid` 省略表示全局；存在时必须指向有效音频轨。
- 同一作用域（同一 `trackUid` 或全局）内按 `startMs` 排序，标记区间不得重叠；不同作用域之间允许重叠。
- `strength` 用于多个候选的排序或“推荐高潮”；`source` / `confidence` 用于区分手工、导入与自动识别。
- 播放器应支持在时间轴显示高潮标记，并提供“跳到高潮”“上一处/下一处”操作。
- 高潮标记以 `NBMUSIC_CLIMAX` Tag 的 JSON 数组为准；可选镜像为 Matroska Chapters 供通用播放器显示，但镜像不参与 `.nbmusic` 语义判定。

## 5. 声道级与多音轨字幕关联

这里要区分两种“多路”：

1. **多 TrackEntry**：同一文件有原唱、伴奏、不同语言配音等多条音频轨。字幕用 `trackUid` 绑定。
2. **单 TrackEntry 多声道**：同一条音频轨的左/右声道承载不同内容，例如左声道中文、右声道日文。字幕用 `channels` 绑定到具体声道。

`AudioBinding` 同时覆盖这两种情况，结构见 4.3 节。

规则：

* stereo 下 `channels: [1]` 是左声道，`[2]` 是右声道，`[1,2]` 是双声道。
* 5.1/7.1 下声道序号按 `AudioTrack.channelLayout` / `channelOrder` 解释；`labels` 可写 `"L"`、`"R"`、`"C"`、`"LFE"` 等作为提示。
* 同一条字幕轨可以包含多个 `AudioBinding`，服务多条音轨或多个声道。
* 左右声道内容不同时，至少拆成两条 `original` 字幕轨，分别绑定 `[1]` 和 `[2]`。
* 若一句字幕要同时显示在左右声道，`channels` 省略或写 `[1,2]`。
* `AudioTrack.channelPrograms` 用于声明每个声道承载的语言/内容，播放器和导入器据此选择字幕。
* 播放器应支持按 `ChannelProgram` 选择或静音声道，并切换到该声道绑定的字幕轨。
* 同一音频轨的每种 role + language + 声道组合最多一个 `default`。

常用声道序号（1-based，未特别声明时按此解释）：

| `channelLayout` | 声道序号 |
| --- | --- |
| `mono` | 1 = M |
| `stereo` | 1 = L，2 = R |
| `2.1` | 1 = L，2 = R，3 = LFE |
| `5.1` | 1 = L，2 = R，3 = C，4 = LFE，5 = Ls，6 = Rs |
| `7.1` | 1 = L，2 = R，3 = C，4 = LFE，5 = Ls，6 = Rs，7 = Lb，8 = Rb |
| `7.1.4` | 1–8 同 7.1，9–12 = 高度声道 |
| `ambisonic` | 按 `channelOrder` 解释，默认 ACN |

若文件使用其他声道顺序，必须写 `channelOrder` 或 `AudioBinding.labels` 明确标注；播放器不得只凭声道数量猜测。

同一音频轨可以有：

* 1 条 `original`（可按声道拆成多条）；
* N 条 `translation`（不同语言/不同源声道）；
* 1 条 `pronunciation`（可按声道拆分）；
* 1 条 `karaoke`，可与 `original` 合并为同一条轨的多 role；
* N 条 `description`。

翻译轨 `sourceSubtitleUid` 必须存在；`refCueId` 必须指向源轨内存在的 Cue。读音轨可以独立存在，也可以把 `pronunciations` 直接挂在原文 Cue 上。

## 6. 封装细节

### 6.1 音频



* `TrackType = 2`。

* CodecID 映射：


  * AAC → `A_AAC`

  * MP3 → `A_MPEG/L3`

  * FLAC → `A_FLAC`

  * PCM → `A_PCM/INT/LIT`

  * Opus → `A_OPUS`（可选扩展）

* Audio 元素写入 `SamplingFrequency`、`Channels`、`BitDepth`。

* 声道布局写入 `NBMUSIC:CHANNEL_LAYOUT` Tag；ambisonic 可追加 `NBMUSIC:CHANNEL_ORDER`。

* 声道承载的语言/内容写入 `NBMUSIC:CHANNEL_PROGRAMS` Tag（JSON），供播放器选择字幕。

* v1.0 要求音频块不使用 lacing，降低解析复杂度，并与现有 MKA 解封装能力保持一致。

* 多声道 PCM 按采样点交错存储。

### 6.2 字幕



* CodecID：`S_TEXT/NBMUSIC`。

* `CodecPrivate` 存轨道级 JSON：



```
{
  "v": 1,
  "uid": "3",
  "audioBindings": [{ "trackUid": "1", "channels": [1] }],
  "roles": ["original", "karaoke"],
  "language": "zh-CN",
  "pronunciationScheme": "pinyin"
}
```



* 每个 Block 存一条 Cue JSON。

* Block 时间戳 = `startMs`。

* `BlockDuration = endMs - startMs`。

* 字幕文本统一 UTF-8。

* 可选 Matroska 标准 zlib 压缩，降低 JSON 体积。

* 每条字幕轨写入 Cues，支持按时间 seek。

* 单条 Cue 损坏不影响整轨；播放器可跳过并继续。

### 6.3 封面



* Matroska Attachment：


  * `FileName = cover-front.jpg`

  * `FileMimeType = image/jpeg`

  * `FileData = 原图字节`

  * `FileUID` 与 `Cover.attachmentUid` 对应

* `NBMUSIC:COVER` Tag 记录附件 UID、用途、宽高、SHA-256。

* 封面附件建议放在 Clusters 之前，方便播放器快速取图。

### 6.4 内嵌字体与高潮

字体：

- 以 Matroska Attachment 存储：`FileName`、`FileMimeType`、`FileData`、`FileUID`。
- `NBMUSIC_FONTS` Tag 存 `FontAttachment[]` 的 JSON（不含 `FileData`），用于把 `fontFamily`、`fontAttachmentUid` 映射到附件。
- 文件名建议：`font-notosanssc-regular-400.otf`。
- 播放器只在内存中加载字体，不安装到系统字体目录。
- 字体许可不允许嵌入时，不应写入附件；播放器缺失字体时回退系统字体。

高潮：

- `NBMUSIC_CLIMAX` Tag 存 `ClimaxMarker[]` 的 JSON。
- 全局标记写入全局 Tag；只针对某条音频轨的标记写入该轨的 Track Tag，并在 JSON 中保留 `trackUid`。
- 播放器加载后可在时间轴渲染标记，支持跳到 `peakMs`、上一处/下一处。
- 可选镜像为 Matroska Chapters；镜像仅供通用播放器显示，`.nbmusic` 以 `NBMUSIC_CLIMAX` 为准。

### 6.5 标签



| Tag                                             | 说明              |
| ----------------------------------------------- | --------------- |
| `NBMUSIC_SPEC`                                  | 必选，`1.0`        |
| `NBMUSIC_ID`                                    | 文件 UUID         |
| `NBMUSIC_COVER`                                 | 封面附件 UID 与用途    |
| `NBMUSIC_CHANNEL_LAYOUT`                        | 轨道级声道布局         |
| `NBMUSIC_CHANNEL_ORDER`                         | ambisonic 等声道顺序 |
| `NBMUSIC_CHANNEL_PROGRAMS`                       | 声道承载的语言/内容      |
| `NBMUSIC_AUDIO_BINDINGS`                         | 字幕轨关联的音频轨与声道（JSON） |
| `NBMUSIC_FONTS`                                 | 内嵌字体清单（JSON）      |
| `NBMUSIC_CLIMAX`                                | 高潮定位标记（JSON）      |
| `NBMUSIC_SUBTITLE_ROLES`                        | 字幕轨角色           |
| `NBMUSIC_SOURCE_SUBTITLE`                       | 翻译 / 读音轨的源字幕轨   |
| `NBMUSIC_LYRICS_OFFSET_MS`                      | 全局歌词偏移          |
| `TITLE` / `ARTIST` / `ALBUM` / `DATE` / `GENRE` | 通用音乐元数据         |
| `ISRC` / `BPM` / `REPLAYGAIN_*`                 | 可选扩展元数据         |

## 7. 时间与同步



* 全文件统一整数毫秒。

* Matroska `TimestampScale = 1,000,000`（1 ms）。

* Cue 区间采用左闭右开 `[startMs, endMs)`，避免相邻字幕端点重复闪烁。

* 时间 0 = 第一条音频的首个采样。

* 音轨若有前导静音，用 `startOffsetMs` 表示，不把静音写入音频块。

* 偏移优先级：

1. 全局 `lyricsOffsetMs`（`NBMUSIC_LYRICS_OFFSET_MS`）

2. 单轨 `offsetMs`

3. 单 Cue 不设独立偏移；不鼓励逐 Cue 偏移。

* 播放器按 Cues seek；字幕按时间窗口按需加载。

* 逐字进度由 `startMs`、`endMs`、`mode` 计算；`progress` 关键点用于非线性进度。

## 8. 校验、安全与完整性

### 8.1 必选校验



1. 至少 1 条音频轨。
2. `NBMUSIC_SPEC` 存在且可识别。
3. 所有 TrackUID 唯一。
4. 字幕轨的 `audioBindings` 非空；每个 `trackUid` 存在，`channels` 序号落在 `1..AudioTrack.channels` 范围内。
5. `channelPrograms` 的声道序号合法，且不超出对应音频轨的声道数。
6. Cue 的 `endMs > startMs`。
7. `chars` 时间单调、不重叠，且落在 Cue 区间内。
8. `chars[].text` 拼接等于 `text`。
9. `segments[].source` 拼接等于 Cue `text`；`segments[].reading` 拼接等于 `pronunciation.text`。
10. 翻译轨引用有效。
11. `language` 使用 BCP 47。
12. `fonts` 的 `attachmentUid` 均存在，MIME 属于字体白名单，且不超过大小限制。
13. `climaxes` 的 `trackUid` 存在；`startMs`/`endMs`/`peakMs` 合法；同一作用域内不重叠。

### 8.2 安全限制



* 附件只允许图片 / 字体；禁止可执行文件、脚本、HTML。

* 附件文件名禁止路径分隔符与 `..`。

* 单条 Cue 文本 ≤ 64 KB。

* 单轨 `CodecPrivate` ≤ 1 MB。

* 封面 ≤ 20 MB。

* 单个字体 ≤ 50 MB，全部内嵌字体总量 ≤ 100 MB。

* 高潮标记数量建议 ≤ 1000 条，`NBMUSIC_CLIMAX` JSON ≤ 1 MB。

* 字幕按纯文本处理；样式只走白名单字段，不允许内嵌脚本。

* 解析器必须限制单个 Cluster、单条字幕、附件解压后的最大尺寸，防止压缩炸弹。

### 8.3 完整性



* 每个 Cluster 使用 Matroska 标准 CRC-32。

* 可选 `NBMUSIC_PAYLOAD_SHA256`：


  * 覆盖音频与字幕 Block 的原始字节；

  * 不包含该 Tag 自身；

  * 用于分发校验与损坏检测。

## 9. 版本与扩展



* `NBMUSIC_SPEC = "1.0"`。

* 读取方必须忽略未知字段；写入方不得改变已有字段语义。

* 扩展字段统一使用 `x-` 前缀；未知 role 也允许 `x-`。

* 字幕轨道 JSON 自带 `"v": 1`，可与文件级版本独立演进。

* 破坏性变更升主版本，例如 `2.0`；播放器遇到不支持的主版本应拒绝播放并提示。

* Matroska `DocTypeVersion` / `DocTypeReadVersion` 与文件级版本保持兼容。

## 10. 完整示例

### 10.1 轨道布局

同一个音频文件，stereo 左声道中文、右声道日文：

* Track 1：FLAC，stereo，48 kHz，default；声道内容：
  * 声道 1（左）：`zh-CN`，中文；
  * 声道 2（右）：`ja-JP`，日本語。
* Track 2：字幕，左声道原文 + 逐字，`roles=["original","karaoke"]`，绑定 Track 1 声道 1，`zh-CN`，读音体系 `pinyin`。
* Track 3：字幕，右声道原文 + 逐字，`roles=["original","karaoke"]`，绑定 Track 1 声道 2，`ja-JP`，读音体系 `zh-phonetic`。
* Track 4：字幕，左声道中文的英文翻译，`roles=["translation"]`，绑定 Track 1 声道 1，`en-US`，源字幕轨 Track 2。
* Track 5：字幕，右声道日文的英文翻译，`roles=["translation"]`，绑定 Track 1 声道 2，`en-US`，源字幕轨 Track 3。

Track 1 的 `channelPrograms`：

```json
[
  { "channels": [1], "language": "zh-CN", "title": "中文" },
  { "channels": [2], "language": "ja-JP", "title": "日本語" }
]
```

### 10.2 左右声道字幕轨头

左声道（Track 2）：

```json
{
  "v": 1,
  "uid": "2",
  "audioBindings": [{ "trackUid": "1", "channels": [1] }],
  "roles": ["original", "karaoke"],
  "language": "zh-CN",
  "pronunciationScheme": "pinyin"
}
```

右声道（Track 3）：

```json
{
  "v": 1,
  "uid": "3",
  "audioBindings": [{ "trackUid": "1", "channels": [2] }],
  "roles": ["original", "karaoke"],
  "language": "ja-JP",
  "pronunciationScheme": "zh-phonetic"
}
```

### 10.3 日语读音示例

以下 Cue 属于 Track 3（右声道日文）：

原文 `こんにちは`，中文谐音 `空你鸡哇`，罗马字 `konnichiha`，英文翻译 `Hello`：



```
{
  "id": "c1",
  "startMs": 0,
  "endMs": 2400,
  "text": "こんにちは",

  "pronunciations": [
    {
      "scheme": "zh-phonetic",
      "lang": "zh-CN",
      "text": "空你鸡哇",
      "segments": [
        { "source": "こん", "reading": "空", "startMs": 0,    "endMs": 600  },
        { "source": "に",   "reading": "你", "startMs": 600,  "endMs": 1200 },
        { "source": "ち",   "reading": "鸡", "startMs": 1200, "endMs": 1800 },
        { "source": "は",   "reading": "哇", "startMs": 1800, "endMs": 2400 }
      ]
    },
    {
      "scheme": "romaji",
      "text": "konnichiha"
    }
  ],

  "translations": [
    { "lang": "en", "text": "Hello" }
  ]
}
```

只显示整行读音时：



```
{
  "text": "こんにちは",
  "pronunciations": [
    { "scheme": "zh-phonetic", "text": "空你鸡哇" }
  ]
}
```

### 10.4 中文逐字示例

以下 Cue 属于 Track 2（左声道中文）：



```
{
  "id": "c2",
  "startMs": 2400,
  "endMs": 4000,
  "text": "月亮",
  "chars": [
    {
      "text": "月",
      "startMs": 2400,
      "endMs": 3200,
      "mode": "linear",
      "reading": "yuè"
    },
    {
      "text": "亮",
      "startMs": 3200,
      "endMs": 4000,
      "mode": "linear",
      "reading": "liàng"
    }
  ]
}
```

### 10.5 翻译轨

左声道中文的英文翻译（Track 4）：

```json
{
  "v": 1,
  "uid": "4",
  "audioBindings": [{ "trackUid": "1", "channels": [1] }],
  "roles": ["translation"],
  "language": "en-US",
  "sourceSubtitleUid": "2"
}
```

右声道日文的英文翻译（Track 5）：

```json
{
  "v": 1,
  "uid": "5",
  "audioBindings": [{ "trackUid": "1", "channels": [2] }],
  "roles": ["translation"],
  "language": "en-US",
  "sourceSubtitleUid": "3"
}
```

左声道 Cue（对应 Track 2 的 `c2`）：

```json
{
  "refCueId": "c2",
  "startMs": 2400,
  "endMs": 4000,
  "text": "The moon"
}
```

右声道 Cue（对应 Track 3 的 `c1`）：

```json
{
  "refCueId": "c1",
  "startMs": 0,
  "endMs": 2400,
  "text": "Hello"
}
```

### 10.6 内嵌字体与高潮标记

字体清单（`NBMUSIC_FONTS`）：

```json
[
  {
    "attachmentUid": "100",
    "fileName": "font-notosanssc-regular-400.otf",
    "mime": "font/otf",
    "family": "Noto Sans SC",
    "style": "normal",
    "weight": 400,
    "license": "OFL-1.1",
    "subset": "U+3000-303F,U+3040-30FF,U+4E00-9FFF",
    "sha256": "..."
  }
]
```

字幕样式引用：

```json
{
  "fontFamily": "Noto Sans SC",
  "fontAttachmentUid": "100",
  "fontWeight": 400,
  "fontSizePx": 42,
  "color": "#FFFFFF",
  "align": "center",
  "position": "bottom"
}
```

高潮标记（`NBMUSIC_CLIMAX`）：

```json
[
  {
    "id": "climax-1",
    "trackUid": "1",
    "startMs": 185000,
    "endMs": 205000,
    "peakMs": 192000,
    "kind": "chorus",
    "label": "副歌高潮",
    "strength": 92,
    "source": "manual"
  },
  {
    "id": "climax-2",
    "startMs": 260000,
    "peakMs": 267000,
    "kind": "finale",
    "label": "结尾高潮",
    "strength": 85,
    "source": "auto",
    "confidence": 76
  }
]
```

## 11. 方案对比



| 方案                           | 问题                                                    |
| ---------------------------- | ----------------------------------------------------- |
| ZIP + `manifest.json` + 音频文件 | 无法流式播放；seek 需要解压；音频与字幕时间轴难对齐；适合归档，不适合播放器              |
| 普通 MKA + SRT/ASS             | 行级字幕够用，但逐字进度、多语言翻译、读音标注、轨道稳定关联都表达不了；ASS 的 `\k` 只是文本标签 |
| MP4 自定义 box                  | 能做，但生态与工具链不如 Matroska；项目已有 Matroska 解析器，重复造轮子         |
| 全新二进制容器                      | 需要自己解决流式、seek、索引、附件、兼容性，成本和风险最高                       |

## 12. 实现落点（当前状态）

* `src-tauri/src/nbmusic.rs`（已实现）：
  * serde 数据模型、EBML 读写、`mux_nbmusic_file` / `demux_nbmusic` / `probe_nbmusic_file`；
  * `S_TEXT/NBMUSIC` 字幕轨、Cue JSON、逐字进度、翻译、读音 segments；
  * 封面 / 字体 Attachments、`NBMUSIC_FONTS`、`NBMUSIC_CLIMAX`；
  * `extract_playable_audio`、LRC 导出、封面缓存。

* `src-tauri/src/matroska.rs`（已复用）：
  * `prepare_audio` 提供音频帧；
  * `write_playable_audio` 提供播放封装；
  * EBML 解析辅助函数开放给 `nbmusic.rs`。

* `src-tauri/src/music.rs`（已接入）：
  * `.nbmusic` 加入支持扩展名；
  * 导入时解封装主音频、读取标题/时长、提取字幕 LRC 与封面；
  * 播放缓存按 `nbmusic` / `mka` 隔离。

* 前端：
  * 音乐导入文件过滤器与拖拽识别已支持 `.nbmusic`；
  * 搜索文件类型已识别为音频；
  * 内置工具新增 `/nbmusic-editor`，支持拖入 `.nbmusic` / 音频 / 字幕 / 封面 / 字体，元信息、音频轨、字幕轨、声道绑定、Cue、翻译/读音、逐字 JSON、字体与高潮均可编辑；
  * 打开已有 `.nbmusic` 时通过 `load_nbmusic_project` 回读完整工程，保存时保留原 `NBMUSIC_ID` 与轨道 UID；
  * 字幕导入通过 `parse_nbmusic_subtitle` 复用 SRT / VTT / ASS / SSA 解析。

* 双击打开 `.nbmusic`：
  * 后端 `audio_path_from_args` 识别 `.mka` / `.nbmusic` 路径，经 `PENDING_OPEN_AUDIO` 暂存并通过 `open-audio-file` 事件交给前端；单实例运行时由主实例接管第二次启动的参数。
  * 前端收到后调用音乐模块的 `importLocalSongs`，导入完成即播放，并导航到 `/music` 并展开播放器。
  * 便携版启动时由 `file_association::ensure_nbmusic_open_with` 写入 HKCU：`.nbmusic` 的 `OpenWithProgids`、`NeoBox.nbmusic` ProgID、`Applications\neobox.exe` 的 `SupportedTypes`。只把 NeoBox 加入「打开方式」候选，不抢占默认关联；用户在资源管理器中确认后即可双击直接打开。
  * 双击 `.nbmusic` / `.mka` 会进入**音乐专属模式**：后端置位 `MUSIC_ONLY_MODE`，只启动音乐 API、音频代理、播放控制热键、SMTC 与媒体键；监控子进程、硬件记录、悬浮框轮询、内存清理、CPU 调度、连点器、桌面歌词热键、外部播放器轮询和托盘均不初始化。主窗口关闭即退出进程。
  * 音乐专属模式的界面只渲染展开播放器本体与窗口控制：隐藏侧边栏、全局搜索、启动动画、开屏广告、主页、灵动岛桥接，以及播放器左上角的「收起/返回音乐首页」按钮；搜索、歌单和音乐首页不会出现，无法从播放器切回音乐搜索界面。
  * 纯播放器默认启用桌面歌词，并隐藏播放队列、音质选择、歌词颜色/字号等音乐首页相关按钮，只保留播放控制、音量与桌面歌词开关。播放模式固定为单曲循环，主播放器与桌面歌词控制栏都不显示列表/单曲/随机模式切换按钮。
  * 桌面歌词窗口恢复旧分辨率/旧缩放下的越界坐标时，会在显示后按物理像素钳制回当前显示器工作区，避免窗口实际已打开但停在屏幕外导致“点按钮没反应”。
  * `.nbmusic` / `.mka` 启动不注册单实例插件：已经打开播放器时再次双击文件，会启动一个独立进程和独立播放器窗口，多个窗口/多首歌可同时存在，互不接管播放队列。
  * 编辑页添加 MP3 / FLAC / M4A / MP4 等音频时，会读取容器内嵌封面并自动填入工程封面；MP4 / M4A / MOV 的内嵌 `covr` 封面在 lofty 读取失败时由 ISO BMFF box 解析器兜底提取。
  * 提权重启（UAC）会原样保留命令行参数，避免文件路径在二次启动时丢失。
  * 安装版同时在 `tauri.conf.json` 的 `bundle.fileAssociations` 声明 `.nbmusic`（`audio/x-nbmusic`）。

* 后续可补：
  * 逐字高亮与读音/翻译分行渲染的实时预览；
  * 内嵌字体加载与缺字回退；
  * 高潮标记时间轴与跳转；
  * 5.1 / 7.1、长歌词 seek、损坏 Cue、UTF-8 边界等自动化测试。

## 13. 已确认与待定

### 已确认



* “音调” 指**读音 / 音译标注**，不是音乐音高。

* 字段统一使用 `pronunciation`，不再使用 `tone`。

* 支持 `pinyin`、`zhuyin`、`romaji`、`ipa`、`kana`、`zh-phonetic`。

* “多音轨字幕”包含同一音频文件内不同声道的字幕，例如左声道中文、右声道日文；字幕用 `audioBindings` 按 `trackUid + channels` 绑定。

* 封面、字幕、翻译、读音、内嵌字体、高潮定位均为可选。

* 至少一条音频轨为必选。

### 待定



* 普通播放器是否需要直接播放音频。需要则默认 `DocType="matroska"` 兼容模式；不需要则可用 `DocType="nbmusic"` 严格模式。

* Opus 是否纳入 v1.0 基线，还是作为后续扩展。

* 内嵌字体的子集化策略、默认回退字体与许可校验细节。

* 封面是否允许多张非 front 图，以及总大小上限。

* 是否需要章节（Chapters）与音频波形索引。