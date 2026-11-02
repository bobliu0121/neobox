/**
 * 高潮点（highlight）检测 —— 边播边分析，取全曲能量峰值。
 *
 * 为什么不用现成接口：网易云 `/api/song/lyric/v1` 只返回 lrc/klyric/tlyric/romalrc，
 * 实测不存在 marker/climax/chorus/beat 等高潮字段（已探测确认），
 * 因此只能从音频能量曲线自行推导。
 *
 * 设计要点（务必遵守，本项目在 AudioContext 上踩过坑）：
 * 1. **绝不接管 <audio> 输出**：不使用 createMediaElementSource。
 *    项目内 `audio-spectrum.ts` 已注明「接管后 suspend 即整体静音」，
 *    进度条只是附加标记功能，不能影响收听。这里改用 `decodeAudioData`
 *    对**独立下载的副本**做离线分析，与播放链路完全隔离。
 * 2. **只在需要时分析一次**：每首歌按 songId 缓存，切歌不重复分析。
 * 3. **失败必须静默降级**：任何异常（网络/解码/内存）都返回空数组，
 *    进度条退回普通样式，绝不阻塞播放或抛错到 UI。
 * 4. 分析结果按时长归一化（0~1），用百分比定位，不依赖 duration 是否已就绪。
 */

/** 能量包络采样窗口（秒）。0.5s 足够分辨「段落级」高潮，又不会太碎 */
const WINDOW_SEC = 0.5;

/** 每首歌最多保留的高潮点数量（用户选择：1~3 个） */
const MAX_HIGHLIGHTS = 3;

/** 相邻高潮点的最小间隔（秒），避免几个点挤在一起 */
const MIN_GAP_SEC = 8;

/** 单个音频文件分析上限（字节）。超过则跳过，避免拖垮内存 */
const MAX_BYTES = 24 * 1024 * 1024;

/** songId -> 归一化高潮位置（0~1，升序）。null 表示分析过但不可用 */
const cache = new Map<string, number[]>();

/** 正在分析中的任务，避免同一首歌并发重复分析 */
const inflight = new Map<string, Promise<number[]>>();

/**
 * 从 AudioBuffer 计算归一化高潮位置。
 *
 * 算法：
 *  1. 按 WINDOW_SEC 切窗，算每窗 RMS 能量（只用单声道即可，足够判断强弱）
 *  2. 对能量序列做轻度平滑，抑制瞬时尖峰
 *  3. 取能量最高的若干窗口作为候选
 *  4. 按时长归一化、按最小间隔去重、限制数量
 */
export function computeHighlightsFromBuffer(buffer: AudioBuffer): number[] {
  const duration = buffer.duration;
  if (!Number.isFinite(duration) || duration <= 0) return [];

  const data = buffer.getChannelData(0);
  const sampleRate = buffer.sampleRate;
  const winSamples = Math.max(1, Math.floor(WINDOW_SEC * sampleRate));
  const winCount = Math.floor(data.length / winSamples);
  if (winCount < 4) return [];

  // 1) 每窗 RMS
  const rms = new Float32Array(winCount);
  for (let w = 0; w < winCount; w++) {
    const start = w * winSamples;
    let sum = 0;
    for (let i = start; i < start + winSamples; i++) {
      const v = data[i];
      sum += v * v;
    }
    rms[w] = Math.sqrt(sum / winSamples);
  }

  // 2) 3 点平滑（抑制单窗尖峰，保留段落级起伏）
  const smooth = new Float32Array(winCount);
  for (let w = 0; w < winCount; w++) {
    const a = rms[w - 1] ?? rms[w];
    const b = rms[w];
    const c = rms[w + 1] ?? rms[w];
    smooth[w] = (a + b + c) / 3;
  }

  // 3) 阈值过滤：排除静音与「伪峰」
  //
  // 实测发现（合成音频验证）：只取「相对最高的几个窗口」会把背景段的
  // 微小起伏也选成高潮点（静音音频甚至会选出 3 个点）。
  // 因此加两道闸：
  //   a) 绝对闸：全曲最大能量过低（几乎静音/纯朗读）→ 不认为有高潮
  //   b) 相对闸：候选点能量必须显著高于全曲均值，否则视为背景噪音
  let maxRms = 0;
  let sumRms = 0;
  for (let w = 0; w < winCount; w++) {
    if (smooth[w] > maxRms) maxRms = smooth[w];
    sumRms += smooth[w];
  }
  const meanRms = sumRms / winCount;

  // 几乎静音（或音量极低）时没有可信的高潮点
  if (maxRms < 1e-4) return [];

  // 候选必须同时超过：均值的 1.25 倍、且达到最大值的一定比例
  const relGate = meanRms * 1.25;
  const absGate = maxRms * 0.4;
  const gate = Math.max(relGate, absGate);

  // 4) 候选：能量最高的窗口（同一「峰」只取一次）
  const idxOrder = Array.from({ length: winCount }, (_, i) => i)
    .filter((w) => smooth[w] >= gate)
    .sort((a, b) => smooth[b] - smooth[a]);

  const picked: number[] = [];
  const minGapWin = Math.max(1, Math.round(MIN_GAP_SEC / WINDOW_SEC));

  for (const w of idxOrder) {
    // 跳过开头/结尾（通常是前奏和收尾，高潮极少出现在最边上）
    const t = w * WINDOW_SEC;
    if (t < duration * 0.08 || t > duration * 0.94) continue;

    // 与已选点保持最小间隔
    if (picked.some((p) => Math.abs(p * duration - t) < MIN_GAP_SEC)) continue;

    picked.push(t / duration);
    if (picked.length >= MAX_HIGHLIGHTS) break;
  }

  return picked.sort((a, b) => a - b);
}

/**
 * 下载并分析一首歌的高潮点。
 *
 * @param songId 缓存键（用 provider+songId 组合，避免跨平台撞 id）
 * @param audioUrl 可直接访问的音频地址（本地代理地址，同源无 CORS 问题）
 * @returns 归一化位置数组；任何失败返回 []
 */
export async function detectHighlights(
  songId: string,
  audioUrl: string,
): Promise<number[]> {
  if (!songId || !audioUrl) return [];

  const cached = cache.get(songId);
  if (cached) return cached;

  const running = inflight.get(songId);
  if (running) return running;

  const task = (async (): Promise<number[]> => {
    try {
      // 用 fetch 拿字节流（本地代理返回真实音频，同源无 CORS 限制）
      const resp = await fetch(audioUrl);
      if (!resp.ok) {
        console.warn(`[Highlights] fetch ${resp.status} -> 跳过分析`);
        return [];
      }

      const contentLength = Number(resp.headers.get("Content-Length") || 0);
      if (contentLength && contentLength > MAX_BYTES) {
        console.warn(
          `[Highlights] 文件过大 ${(contentLength / 1048576).toFixed(1)}MB -> 跳过`,
        );
        return [];
      }

      const bytes = await resp.arrayBuffer();
      if (bytes.byteLength > MAX_BYTES) {
        console.warn("[Highlights] 实际体积超出上限 -> 跳过");
        return [];
      }

      // 离线解码：独立的 OfflineAudioContext，不碰播放用的 AudioContext
      const Ctor =
        window.OfflineAudioContext ??
        (window as unknown as { webkitOfflineAudioContext?: typeof OfflineAudioContext })
          .webkitOfflineAudioContext;
      if (!Ctor) {
        console.warn("[Highlights] 无 OfflineAudioContext -> 跳过");
        return [];
      }

      // 采样率/长度在 decodeAudioData 前不影响解码，给个最小可用配置
      const offline = new Ctor(1, 1, 44100);
      const buffer = await offline.decodeAudioData(bytes);
      const result = computeHighlightsFromBuffer(buffer);

      console.info(
        `[Highlights] ${songId} 分析完成 -> ${result.length} 个点 ` +
          `(${result.map((v) => `${(v * 100).toFixed(1)}%`).join(", ")})`,
      );
      return result;
    } catch (e) {
      // 静默降级：分析失败不影响播放
      console.warn("[Highlights] 分析失败 -> 跳过:", e);
      return [];
    } finally {
      inflight.delete(songId);
    }
  })();

  inflight.set(songId, task);
  const result = await task;
  cache.set(songId, result);
  return result;
}

/** 读取已缓存的高潮点（未分析过返回 undefined，用来区分「没分析」与「分析了但为空」） */
export function getCachedHighlights(songId: string): number[] | undefined {
  return cache.get(songId);
}

/** 清空缓存（切歌列表切换等场景可调用，一般不需要） */
export function clearHighlightCache() {
  cache.clear();
}
