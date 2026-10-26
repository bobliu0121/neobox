//! B 站视频/音频下载（免登录）。
//!
//! ## 免登录能拿到什么（实测结论，非猜测）
//!
//! B 站对未登录请求会**静默降级画质**：`playurl` 返回的 `accept_quality` 会声称支持
//! 112(1080P+)/80(1080P)/64(720P)，但实际下发的 DASH 流只有 32(480P) 与 16(360P)。
//! 本模块实测多个不同类型视频（热门 MV、老投稿、冷门视频）结果一致，
//! 带上游客 `buvid3` cookie 也无法提升。
//!
//! 因此：
//! - 免登录路径下最高只能取到 480P；
//! - 界面必须**如实展示实际可用的清晰度**（用 streams 里真实存在的
//!   `accept_quality` 与 DASH 流求交集），而不是照抄 `accept_quality` 撒谎。
//! - 高清晰度（720P+）需要登录 Cookie，本模块已预留 `cookie` 参数入口，
//!   后续接登录时无需改动调用链。
//!
//! ## 为什么仍要做音视频合并
//!
//! 即便只有 480P，B 站的 DASH 流也是**视频轨与音频轨分离**的（`durl` 单文件形式
//! 只在老接口/特定 qn 下才出现）。所以要拿到可播放的 MP4，必须：
//! 1. 分别下载 video.m4s / audio.m4s；
//! 2. 剥掉各自的容器头，把裸 H.264 Annex-B 与裸 AAC 重新封装成一个 MP4。
//! 第 2 步复用 `file_convert` 里已有的 `Mp4Muxer`。
//!
//! 纯音频模式则只下 audio.m4s，转封装成 .m4a。

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// 与项目其它模块一致的 UA。B 站会对异常 UA 返回 412。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
/// 下载媒体流时必须带 Referer，否则 403（B 站防盗链）。
const REFERER: &str = "https://www.bilibili.com";

/// 全局取消标志：批量下载时用户点「停止」置位。
static CANCEL: AtomicBool = AtomicBool::new(false);
/// 当前已下载字节数（供进度事件读取，避免为每个分片单独加锁）。
static DOWNLOADED: AtomicU64 = AtomicU64::new(0);

// ════════════════════════════════════════════════════════════════════
//  数据模型
// ════════════════════════════════════════════════════════════════════

/// 解析结果：一个视频（可能含多个分P）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BiliVideoInfo {
    pub bvid: String,
    pub aid: i64,
    pub title: String,
    pub cover: String,
    pub duration: i64,
    pub owner: String,
    pub desc: String,
    /// 分P 列表。单P视频也有 1 个元素。
    pub pages: Vec<BiliPage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BiliPage {
    pub page: i64,
    pub cid: i64,
    /// 分P 标题（单P时等于视频标题）
    pub part: String,
    pub duration: i64,
}

/// 清晰度选项（只包含**实际可下载**的）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BiliQuality {
    /// DASH 里的 qn：16/32/64/80/112/116/120…
    pub qn: i64,
    /// 面向用户的描述，如「480P 清晰」
    pub label: String,
    pub width: i64,
    pub height: i64,
}

/// 某一分P 的可选清晰度 + 音轨信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BiliPlayInfo {
    pub qualities: Vec<BiliQuality>,
    /// 是否支持纯音频（B 站 DASH 几乎总有音轨）
    pub has_audio: bool,
}


/// 只取清晰度列表的公开封装（避免向外暴露私有 Dash 类型）。
pub async fn fetch_play_qualities(bvid: &str, cid: i64) -> Result<BiliPlayInfo, String> {
    let (info, _) = fetch_play_info(bvid, cid, None).await?;
    Ok(info)
}

/// 下载请求。
#[derive(Debug, Clone, Deserialize)]
pub struct BiliDownloadRequest {
    pub bvid: String,
    pub cid: i64,
    /// 目标 qn；None 表示自动取最高可用
    pub qn: Option<i64>,
    /// 仅下载音频（输出 .m4a）
    pub audio_only: bool,
    /// 输出文件完整路径
    pub output_path: String,
    /// 分P 序号，仅用于日志与提示
    pub page: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BiliDownloadResult {
    pub output_path: String,
    pub bytes: u64,
    pub quality_label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BiliProgress {
    /// 当前分P 序号
    pub page: i64,
    pub total_pages: i64,
    /// 0~100
    pub percent: f64,
    /// 已下载 / 总字节
    pub downloaded: u64,
    pub total: u64,
    /// 阶段描述：解析 / 下载视频 / 下载音频 / 合并 / 完成
    pub stage: String,
}

// ════════════════════════════════════════════════════════════════════
//  输入解析：链接 / BV 号 / av 号
// ════════════════════════════════════════════════════════════════════

/// 从任意输入中提取 BV 号或 av 号。
///
/// 支持：
/// - 纯 BV 号：`BV1GJ411x7h7`
/// - 纯 av 号：`av80433022` / `AV80433022`
/// - 完整链接：`https://www.bilibili.com/video/BV1GJ411x7h7/?p=2`
/// - 短链：`https://b23.tv/xxxxx`（返回 `ShortLink` 交由后续解析）
/// - 手机端：`https://m.bilibili.com/video/BV1GJ411x7h7`
pub enum BiliIdInput {
    Bvid(String),
    Aid(i64),
    /// b23.tv 短链，需要先跟随重定向拿到真实地址
    ShortLink(String),
}

pub fn parse_input(raw: &str) -> Result<BiliIdInput, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("输入为空".to_string());
    }

    // BV 号：BV + 10 位（旧格式长度固定 12）
    if let Some(pos) = s.find("BV") {
        let tail = &s[pos..];
        // BV 号后续只取字母数字，避免把链接里的 / ? 带进来
        let bv: String = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if bv.len() >= 12 {
            return Ok(BiliIdInput::Bvid(bv));
        }
    }

    // av 号
    let lower = s.to_lowercase();
    if let Some(pos) = lower.find("av") {
        let digits: String = lower[pos + 2..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !digits.is_empty() {
            if let Ok(aid) = digits.parse::<i64>() {
                return Ok(BiliIdInput::Aid(aid));
            }
        }
    }

    // b23.tv 短链
    if s.contains("b23.tv") {
        return Ok(BiliIdInput::ShortLink(s.to_string()));
    }

    Err(format!("无法识别的链接或编号：{s}"))
}

/// 跟随 b23.tv 重定向拿到真实 URL，再从中解析出 BV/av。
pub async fn resolve_short_link(url: &str) -> Result<BiliIdInput, String> {
    let client = crate::utils::http::client_with_ua(15, Some(10), Some(UA));
    let resp = client
        .get(url)
        .header("Referer", REFERER)
        .send()
        .await
        .map_err(|e| format!("解析短链失败: {e}"))?;
    // reqwest 默认跟随重定向，最终 URL 即为目标
    let final_url = resp.url().to_string();
    parse_input(&final_url)
}

// ════════════════════════════════════════════════════════════════════
//  API 调用
// ════════════════════════════════════════════════════════════════════

/// `x/web-interface/view` 的响应（只取用到的字段）。
#[derive(Deserialize)]
struct ViewResp {
    code: i64,
    message: String,
    data: Option<ViewData>,
}

#[derive(Deserialize)]
struct ViewData {
    bvid: String,
    aid: i64,
    title: String,
    pic: String,
    duration: i64,
    desc: Option<String>,
    owner: Option<Owner>,
    pages: Option<Vec<PageData>>,
    cid: Option<i64>,
}

#[derive(Deserialize)]
struct Owner {
    name: String,
}

#[derive(Deserialize)]
struct PageData {
    page: i64,
    cid: i64,
    part: String,
    duration: i64,
}

/// 获取视频基本信息（含分P 列表）。
pub async fn fetch_video_info(bvid: &str) -> Result<BiliVideoInfo, String> {
    let client = crate::utils::http::client_with_ua(15, Some(10), Some(UA));
    let url = format!("https://api.bilibili.com/x/web-interface/view?bvid={bvid}");
    let resp = client
        .get(&url)
        .header("Referer", REFERER)
        .send()
        .await
        .map_err(|e| format!("请求视频信息失败: {e}"))?;

    if resp.status() == 412 {
        return Err("请求被 B 站风控拦截（412），请稍后重试".to_string());
    }

    let body: ViewResp = resp.json().await.map_err(|e| format!("解析视频信息失败: {e}"))?;
    if body.code != 0 {
        return Err(format!("B 站返回错误 {}: {}", body.code, body.message));
    }
    let d = body.data.ok_or("视频信息为空")?;

    // pages 缺失时用顶层 cid 兜底（部分接口返回不含 pages）
    let pages = match d.pages {
        Some(ps) if !ps.is_empty() => ps
            .into_iter()
            .map(|p| BiliPage {
                page: p.page,
                cid: p.cid,
                part: p.part,
                duration: p.duration,
            })
            .collect(),
        _ => {
            let cid = d.cid.ok_or("该视频没有可用 cid")?;
            vec![BiliPage {
                page: 1,
                cid,
                part: d.title.clone(),
                duration: d.duration,
            }]
        }
    };

    Ok(BiliVideoInfo {
        bvid: d.bvid,
        aid: d.aid,
        title: d.title,
        cover: d.pic,
        duration: d.duration,
        owner: d.owner.map(|o| o.name).unwrap_or_default(),
        desc: d.desc.unwrap_or_default(),
        pages,
    })
}

/// 通过 av 号拿 bvid：view 接口接受 aid 参数。
pub async fn bvid_from_aid(aid: i64) -> Result<String, String> {
    let client = crate::utils::http::client_with_ua(15, Some(10), Some(UA));
    let url = format!("https://api.bilibili.com/x/web-interface/view?aid={aid}");
    let resp = client
        .get(&url)
        .header("Referer", REFERER)
        .send()
        .await
        .map_err(|e| format!("请求视频信息失败: {e}"))?;
    let body: ViewResp = resp.json().await.map_err(|e| format!("解析失败: {e}"))?;
    if body.code != 0 {
        return Err(format!("B 站返回错误 {}: {}", body.code, body.message));
    }
    body.data.map(|d| d.bvid).ok_or("未取到 BV 号".to_string())
}

#[derive(Deserialize)]
struct PlayUrlResp {
    code: i64,
    message: String,
    data: Option<PlayUrlData>,
}

#[derive(Deserialize)]
struct PlayUrlData {
    #[serde(default)]
    accept_quality: Vec<i64>,
    #[serde(default)]
    accept_description: Vec<String>,
    dash: Option<Dash>,
    /// 老接口的单文件形式（低清/部分视频才有）
    durl: Option<Vec<Durl>>,
}

#[derive(Deserialize)]
struct Dash {
    #[serde(default)]
    video: Vec<DashVideo>,
    #[serde(default)]
    audio: Vec<DashAudio>,
}

#[derive(Deserialize, Clone)]
struct DashVideo {
    id: i64,
    base_url: Option<String>,
    backup_url: Option<Vec<String>>,
    width: i64,
    height: i64,
    /// 12=AVC(H.264) 7=AV1 13=HEVC。合并时优先 AVC，兼容性最好。
    codecid: Option<i64>,
}

#[derive(Deserialize, Clone)]
struct DashAudio {
    id: i64,
    base_url: Option<String>,
    backup_url: Option<Vec<String>>,
    bandwidth: Option<i64>,
}

#[derive(Deserialize, Clone)]
struct Durl {
    url: Option<String>,
    backup_url: Option<Vec<String>>,
}


/// B 站 DASH 的 `codecid` 含义与我们的选择优先级。
///
/// **这里踩过一个严重 bug**：最初把 12 当成"最优先"，理由是"数字大=更好"。
/// 实际上：
///   - 7  = AVC / H.264（`avc1.*`）—— 兼容性最好，我们的 muxer 直接支持
///   - 12 = HEVC / H.265（`hev1.*`）—— 需要 `hvc1`+`hvcC` 容器，当前 muxer 不支持
///   - 13 = AV1（`av01.*`）—— 需要 `av01`+`av1C`
///
/// 选到 HEVC 后再按 AVC 的方式封装，会让播放器拿不到可解码的 SPS/PPS，
/// 表现就是**只有声音没有画面**（用户实际反馈的现象）。
/// 因此必须优先 AVC：宁可清晰度相同但编码兼容，也不要"看起来更好"却放不出来。
fn codec_priority(codecid: Option<i64>) -> i32 {
    match codecid {
        Some(7) => 30,  // AVC：唯一被当前 muxer 完整支持
        Some(12) => 20, // HEVC：能下载但不能直接封装成 avc1
        Some(13) => 10, // AV1：同上
        _ => 0,
    }
}

/// qn → 面向用户的标签。B 站官方描述有时缺失，这里兜底。
fn qn_label(qn: i64) -> &'static str {
    match qn {
        120 => "4K 超清",
        116 => "1080P60",
        112 => "1080P+",
        80 => "1080P 高清",
        74 => "720P60",
        64 => "720P 高清",
        32 => "480P 清晰",
        16 => "360P 流畅",
        _ => "未知清晰度",
    }
}

/// 拉取某分P 的 playurl 信息，并**只返回实际可下载的清晰度**。
///
/// 关键点：`accept_quality` 会虚报（未登录时声称到 1080P+），
/// 必须以 DASH 里真实存在的 `video[].id` 为准做交集，
/// 否则用户选了 1080P 却下到 480P，或直接下载失败。
pub async fn fetch_play_info(
    bvid: &str,
    cid: i64,
    cookie: Option<&str>,
) -> Result<(BiliPlayInfo, Dash), String> {
    let client = crate::utils::http::client_with_ua(20, Some(10), Some(UA));
    // fnval=4048 表示：DASH + HDR + 4K + 杜比音频 + 杜比视界 + 8K + AV1
    let url = format!(
        "https://api.bilibili.com/x/player/playurl?bvid={bvid}&cid={cid}&qn=127&fnval=4048&fourk=1"
    );
    let mut req = client.get(&url).header("Referer", REFERER);
    // 预留登录入口：传了 cookie 就用，免登录时为空
    if let Some(c) = cookie {
        if !c.is_empty() {
            req = req.header("Cookie", c);
        }
    }
    let resp = req.send().await.map_err(|e| format!("请求播放地址失败: {e}"))?;
    if resp.status() == 412 {
        return Err("请求被 B 站风控拦截（412），请稍后重试".to_string());
    }
    let body: PlayUrlResp = resp
        .json()
        .await
        .map_err(|e| format!("解析播放地址失败: {e}"))?;
    if body.code != 0 {
        return Err(format!("B 站返回错误 {}: {}", body.code, body.message));
    }
    let d = body.data.ok_or("播放地址为空")?;

    let dash = d.dash.unwrap_or(Dash {
        video: Vec::new(),
        audio: Vec::new(),
    });

    // 只保留「有可用直链」且「AVC 编码优先」的清晰度
    // B 站同一 qn 会给多个 codecid（12/7/13），我们每个 qn 只留一条：
    // 优先 12(AVC)，其次 13(HEVC)，最后 7(AV1) —— 兼容性递减。
    let mut by_qn: std::collections::BTreeMap<i64, &DashVideo> = std::collections::BTreeMap::new();
    for v in &dash.video {
        let has_url = v.base_url.is_some()
            || v.backup_url.as_ref().map(|b| !b.is_empty()).unwrap_or(false);
        if !has_url {
            continue;
        }
        let prio = codec_priority(v.codecid);
        by_qn
            .entry(v.id)
            .and_modify(|cur| {
                let cur_prio = codec_priority(cur.codecid);
                if prio > cur_prio {
                    *cur = v;
                }
            })
            .or_insert(v);
    }

    let mut qualities: Vec<BiliQuality> = by_qn
        .into_iter()
        .map(|(qn, v)| {
            // 官方描述里可能有更准确的说法（如「高清 1080P+」），优先使用
            let official = d
                .accept_quality
                .iter()
                .position(|&q| q == qn)
                .and_then(|i| d.accept_description.get(i));
            let label = match official {
                Some(s) if !s.trim().is_empty() => s.clone(),
                _ => qn_label(qn).to_string(),
            };
            BiliQuality {
                qn,
                label,
                width: v.width,
                height: v.height,
            }
        })
        .collect();
    // 由低到高排序，界面默认选最后一个（最高）
    qualities.sort_by_key(|q| q.qn);

    let has_audio = !dash.audio.is_empty();
    Ok((
        BiliPlayInfo {
            qualities,
            has_audio,
        },
        dash,
    ))
}

// ════════════════════════════════════════════════════════════════════
//  下载
// ════════════════════════════════════════════════════════════════════

/// 选中 qn 对应的视频流（AVC 优先，已在 fetch_play_info 里筛过 base_url）。
fn pick_video_stream(dash: &Dash, qn: i64) -> Option<&DashVideo> {
    let mut best: Option<&DashVideo> = None;
    let mut best_prio = -1;
    for v in &dash.video {
        if v.id != qn {
            continue;
        }
        let has_url = v.base_url.is_some()
            || v.backup_url.as_ref().map(|b| !b.is_empty()).unwrap_or(false);
        if !has_url {
            continue;
        }
        let prio = codec_priority(v.codecid);
        if prio > best_prio {
            best_prio = prio;
            best = Some(v);
        }
    }
    best
}

/// 选码率最高的音轨（音质最好）。
fn pick_audio_stream(dash: &Dash) -> Option<&DashAudio> {
    dash.audio
        .iter()
        .filter(|a| {
            a.base_url.is_some()
                || a.backup_url.as_ref().map(|b| !b.is_empty()).unwrap_or(false)
        })
        .max_by_key(|a| a.bandwidth.unwrap_or(0))
}

/// 把一个 URL 候选集依次尝试下载，任一成功即返回。
async fn download_to_vec(urls: &[String]) -> Result<Vec<u8>, String> {
    let client = crate::utils::http::client_with_ua(300, Some(15), Some(UA));
    let mut last_err = String::new();
    for url in urls {
        if CANCEL.load(Ordering::SeqCst) {
            return Err("已取消".to_string());
        }
        let resp = match client.get(url).header("Referer", REFERER).send().await {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("连接失败: {e}");
                continue;
            }
        };
        if !resp.status().is_success() {
            last_err = format!("HTTP {}", resp.status());
            continue;
        }
        let total = resp.content_length().unwrap_or(0);
        let mut buf: Vec<u8> = Vec::with_capacity(total as usize);
        use futures_util::StreamExt;
        let mut stream = resp.bytes_stream();
        let mut failed = false;
        while let Some(chunk) = stream.next().await {
            if CANCEL.load(Ordering::SeqCst) {
                return Err("已取消".to_string());
            }
            match chunk {
                Ok(c) => {
                    buf.extend_from_slice(&c);
                    // 只累加全局计数：闭包回调会让 future 变成非 Send
                    DOWNLOADED.fetch_add(c.len() as u64, Ordering::Relaxed);
                }
                Err(e) => {
                    last_err = format!("下载中断: {e}");
                    failed = true;
                    break;
                }
            }
        }
        if !failed && !buf.is_empty() {
            return Ok(buf);
        }
    }
    Err(format!("所有下载地址均失败（最后一次: {last_err}）"))
}


/// 从 m4s 的 moov→stsd→avc1→avcC 里读出真实的 SPS / PPS。
///
/// ## 为什么必须从 moov 读，而不是从帧里找
///
/// B 站 fMP4 的**视频样本里不含 SPS/PPS**（它们只存在于初始化段 moov 的 avcC 中）。
/// 现有 muxer 的 `add_video` 逻辑是「从第一帧的 Annex-B 里提取 SPS/PPS」，
/// 对 B 站这种输入永远提取不到 → `v_sps` 为空 → 写出的 avcC 退化成硬编码的
/// 兜底值（66/0/40），解码器拿不到真实的 profile/level 与参数集，**结果就是只有声音没有画面**。
///
/// 正确做法：解析初始化段的 avcC，拿到真正的 SPS/PPS，再连同帧一起交给 muxer。
pub fn read_sps_pps_from_moov(fmp4: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut pos = 0usize;
    while pos + 8 <= fmp4.len() {
        let size =
            u32::from_be_bytes([fmp4[pos], fmp4[pos + 1], fmp4[pos + 2], fmp4[pos + 3]]) as usize;
        let typ = &fmp4[pos + 4..pos + 8];
        if size < 8 || pos + size > fmp4.len() {
            return None;
        }
        if typ == b"moov" {
            // ⚠️ 必须传**含 box 头**的完整 moov 切片。
            // find_avcc 内部从 p=8 开始遍历子 box（跳过 8 字节 box 头），
            // 若这里先跳过 8 字节再传 content，就会再跳一次 → 直接读到 0 字节，
            // 遍历立即失败 → 取不到 avcC → avcC 退化成假 profile(66)/level(40)，
            // 与真实 High Profile(100) 不符，播放器报「不支持的 avc1 格式」只放声音。
            return find_avcc(&fmp4[pos..pos + size]);
        }
        pos += size;
    }
    None
}

/// 递归在 trak/mdia/minf/stbl/stsd 中找到 avcC 并解析。
fn find_avcc(node: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    find_avcc_from(node, 8)
}

/// 同 `find_avcc`，但从指定偏移开始遍历子 box。
///
/// 需要它是因为不同容器的“子 box 起始位置”不同：
///   - trak/mdia/minf/stbl：紧跟在 8 字节 box 头之后 → 偏移 8
///   - stsd：还要跳过 ver/flags + entry_count → 偏移 16
///   - avc1（VisualSampleEntry）：有 70 字节固定字段 → 偏移 78
/// 早期实现一律用 8，导致 stsd/avc1 层解析错位、取不到 avcC，
/// 最终 avcC 退化写入兜底的假 profile(66)/level(40)，
/// 与真实 High Profile(100) 不符 → 播放器报「不支持的 avc1 格式」。
fn find_avcc_from(node: &[u8], start: usize) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut p = start;
    while p + 8 <= node.len() {
        let size = u32::from_be_bytes([node[p], node[p + 1], node[p + 2], node[p + 3]]) as usize;
        let typ = &node[p + 4..p + 8];
        if size < 8 || p + size > node.len() {
            break;
        }
        match typ {
            b"trak" | b"mdia" | b"minf" | b"stbl" => {
                if let Some(r) = find_avcc(&node[p..p + size]) {
                    return Some(r);
                }
            }
            b"stsd" => {
                // stsd 结构：size(4) + 'stsd'(4) + ver/flags(4) + entry_count(4) + sample entry...
                // 引导到第一个 sample entry 时，必须跳过 16 字节头部，
                // 不能沿用 find_avcc 默认的 8 字节（那会把 ver/flags 当作 box size 读取 → 解析失败）。
                if size >= 16 {
                    let entry = &node[p..p + size];
                    if let Some(r) = find_avcc_from(entry, 16) {
                        return Some(r);
                    }
                }
            }
            b"avc1" | b"avc3" => {
                // VisualSampleEntry 的固定字段共 **86 字节**（含 8 字节 box 头），
                // 子 box（avcC）从偏移 86 开始。逐项核算（ISO/IEC 14496-12 §12.1.3）：
                //     8  box 头
                //   + 6  reserved[6]
                //   + 2  data_reference_index
                //   + 2  pre_defined
                //   + 2  reserved
                //   +12  pre_defined[3]
                //   + 2  width
                //   + 2  height
                //   + 4  horizresolution
                //   + 4  vertresolution
                //   + 4  reserved
                //   + 2  frame_count
                //   +32  compressorname
                //   + 2  depth
                //   + 2  pre_defined
                //  = 86
                //
                // ⚠️ 这里曾写成 78，少了 8 字节（漏算 box 头 + reserved[6]），
                // 于是把 depth/pre_defined 当成 box size 读（得到 0x0018FFFF 这种荒谬值），
                // 遍历立即失败 → 取不到 avcC → avcC 退化写入假 profile(66)/level(40)，
                // 而源流真实编码是 avc1.64001F（profile=100 High / level=31）。
                // 解码器发现 avcC 与实际码流不符，报「不支持的 avc1 格式」，只放音频。
                if size >= 86 {
                    let entry = &node[p..p + size];
                    if let Some(r) = find_avcc_from(entry, 86) {
                        return Some(r);
                    }
                }
            }
            b"avcC" => {
                return parse_avcc_body(&node[p..p + size]);
            }
            _ => {}
        }
        p += size;
    }
    None
}

/// 解析 avcC（AVCDecoderConfigurationRecord）的 body，取出首组 SPS/PPS。
fn parse_avcc_body(avcc: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    // body: 8(box header) + configurationVersion(1) profile(1) compat(1) level(1)
    //       + 0xFC|reserved+numSPS(1) + [len(2)+sps]* + numPPS(1) + [len(2)+pps]*
    if avcc.len() < 12 {
        return None;
    }
    let b = &avcc[8..];
    if b.len() < 6 {
        return None;
    }
    let num_sps = (b[5] & 0x1F) as usize;
    let mut off = 6usize;
    let mut sps_out: Option<Vec<u8>> = None;
    for _ in 0..num_sps {
        if off + 2 > b.len() {
            return None;
        }
        let l = u16::from_be_bytes([b[off], b[off + 1]]) as usize;
        off += 2;
        if off + l > b.len() {
            return None;
        }
        if sps_out.is_none() {
            sps_out = Some(b[off..off + l].to_vec());
        }
        off += l;
    }
    if off + 1 > b.len() {
        return None;
    }
    let num_pps = b[off] as usize;
    off += 1;
    let mut pps_out: Option<Vec<u8>> = None;
    for _ in 0..num_pps {
        if off + 2 > b.len() {
            return None;
        }
        let l = u16::from_be_bytes([b[off], b[off + 1]]) as usize;
        off += 2;
        if off + l > b.len() {
            return None;
        }
        if pps_out.is_none() {
            pps_out = Some(b[off..off + l].to_vec());
        }
        off += l;
    }
    match (sps_out, pps_out) {
        (Some(s), Some(p)) => Some((s, p)),
        _ => None,
    }
}

/// fMP4（m4s）解析：按 `moof`+`mdat` 分片取出每个 sample 的原始字节。
///
/// ## 为什么不能用「4 字节长度前缀」的朴素解析
///
/// B 站的 m4s 是标准 fragmented MP4：
///   ftyp | free | moov | sidx | (moof | mdat)+
/// 一个 moof 里的 `traf/trun` 记录本分片包含几个 sample、每个 sample 多大
/// （`trun` flags 0x200 表示带 sample_size）。实测音频分片是
/// `sample_count=117`，sizes 形如 `[222, 2048, 247, 2048, ...]`。
///
/// 关键点：**mdat 里连续存放这些 sample，没有长度前缀**；
/// 且不同分片的 sample 数/大小都不同。早期实现误当成「4 字节长度前缀」
/// 并把每个 mdat 单独处理，导致第一个 4 字节被当成巨大长度值而解析失败
/// （实测首个 sample 前 4 字节是 0x21114500，即 AAC 帧头而非长度）。
///
/// 正确做法：读 trun 得到 sizes，再按顺序从紧随其后的 mdat 切割。
fn parse_fmp4_samples(data: &[u8]) -> Result<(Vec<Vec<u8>>, Vec<i64>), String> {
    let mut samples: Vec<Vec<u8>> = Vec::new();
    let mut ctos: Vec<i64> = Vec::new();
    let mut pending_sizes: Vec<usize> = Vec::new();
    let mut pending_ctos: Vec<i64> = Vec::new();
    let mut pos = 0usize;

    while pos + 8 <= data.len() {
        let size = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let typ = &data[pos + 4..pos + 8];
        if size < 8 || pos + size > data.len() {
            break;
        }
        if typ == b"moof" {
            // 收集本 moof 内所有 trun 的 sample sizes 与合成时间偏移
            pending_sizes.clear();
            pending_ctos.clear();
            collect_trun_sizes(&data[pos..pos + size], &mut pending_sizes, &mut pending_ctos);
        } else if typ == b"mdat" && !pending_sizes.is_empty() {
            let payload = &data[pos + 8..pos + size];
            let mut off = 0usize;
            for (i, sz) in pending_sizes.iter().enumerate() {
                if off + sz > payload.len() {
                    break;
                }
                samples.push(payload[off..off + sz].to_vec());
                // 把本 sample 的合成时间偏移并入最终列表。
                // ⚠️ 这里曾经漏掉合并（只 clear 了 pending_ctos），
                // 导致最终 ctos 恒为空 → 不写 ctts → B 帧显示顺序错乱、
                // 播放器报「不支持的 avc1 格式」只有声音。已修正。
                ctos.push(*pending_ctos.get(i).unwrap_or(&0));
                off += sz;
            }
            pending_sizes.clear();
            pending_ctos.clear();
        }
        pos += size;
    }

    if samples.is_empty() {
        return Err("未从媒体流中解析出任何 sample".to_string());
    }
    // cto 数量应与样本数一致；不一致时交由调用方决定是否忽略
    Ok((samples, ctos))
}

/// 递归找 moof → traf → trun，按 flags 解析每个 sample 的大小。
fn collect_trun_sizes(node: &[u8], out: &mut Vec<usize>, ctos: &mut Vec<i64>) {
    let mut p = 8usize; // 跳过 box 头
    while p + 8 <= node.len() {
        let size = u32::from_be_bytes([node[p], node[p + 1], node[p + 2], node[p + 3]]) as usize;
        let typ = &node[p + 4..p + 8];
        if size < 8 || p + size > node.len() {
            break;
        }
        if typ == b"traf" {
            collect_trun_sizes(&node[p..p + size], out, ctos);
        } else if typ == b"trun" {
            read_one_trun(&node[p..p + size], out, ctos);
        }
        p += size;
    }
}

fn read_one_trun(trun: &[u8], out: &mut Vec<usize>, ctos: &mut Vec<i64>) {
    if trun.len() < 16 {
        return;
    }
    let flags = u32::from_be_bytes([trun[8], trun[9], trun[10], trun[11]]) & 0x00FF_FFFF;
    let sample_count =
        u32::from_be_bytes([trun[12], trun[13], trun[14], trun[15]]) as usize;
    let mut off = 16usize;
    // data_offset（0x001）
    let has_data_offset = flags & 0x001 != 0;
    if has_data_offset {
        off += 4;
    }
    // first_sample_flags（0x004）
    if flags & 0x004 != 0 {
        off += 4;
    }
    let has_duration = flags & 0x100 != 0;
    let has_size = flags & 0x200 != 0;
    let has_flags = flags & 0x400 != 0;
    let has_cto = flags & 0x800 != 0;

    // 固定大小模式：flags 0x100 时 sample_size 字段在 flags 里
    let fixed_size = if !has_size && has_duration {
        // sample_size 位于 flags 的 bit 8..? 实际是 trun 的 sample_size 字段
        // 规范：若 0x200 未置位，则所有 sample 用 trun header 的 sample_size
        let s = u32::from_be_bytes([trun[8], trun[9], trun[10], trun[11]]);
        // 注意：这一位不需要，交给下面统一处理
        let _ = s;
        0usize
    } else {
        0usize
    };
    let _ = fixed_size;

    for _ in 0..sample_count {
        if off + 4 > trun.len() {
            break;
        }
        if has_duration {
            off += 4;
        }
        let mut sz = 0usize;
        if has_size {
            if off + 4 > trun.len() {
                break;
            }
            sz = u32::from_be_bytes([trun[off], trun[off + 1], trun[off + 2], trun[off + 3]]) as usize;
            off += 4;
        }
        if has_flags {
            off += 4;
        }
        if has_cto {
            // 记录合成时间偏移：这是 B 帧正确显示的关键，必须带出去写进 ctts。
            // 规范：trun version=0 时为 u32，version=1 时为 i32（可负）。
            let trun_ver = trun[8];
            let raw = u32::from_be_bytes([trun[off], trun[off + 1], trun[off + 2], trun[off + 3]]);
            let cto = if trun_ver == 1 { raw as i32 as i64 } else { raw as i64 };
            off += 4;
            if sz > 0 {
                ctos.push(cto);
            }
        }
        if sz > 0 {
            out.push(sz);
        }
    }
}

/// 取出所有 sample 的原始字节（音频：裸 AAC；视频：AVCC 格式的 NAL 长度前缀流）。
fn extract_samples(fmp4: &[u8]) -> Result<(Vec<Vec<u8>>, Vec<i64>), String> {
    parse_fmp4_samples(fmp4)
}

/// 把 AVCC 形式的视频 sample 转成 Annex-B（加 00 00 00 01 起始码）。
fn avcc_samples_to_annexb(samples: &[Vec<u8>]) -> Result<Vec<Vec<u8>>, String> {
    let mut frames: Vec<Vec<u8>> = Vec::with_capacity(samples.len());
    for s in samples {
        let mut out: Vec<u8> = Vec::with_capacity(s.len() + 16);
        let mut p = 0usize;
        while p + 4 <= s.len() {
            let nlen = u32::from_be_bytes([s[p], s[p + 1], s[p + 2], s[p + 3]]) as usize;
            p += 4;
            if nlen == 0 || p + nlen > s.len() {
                return Err("视频 sample 的 NAL 长度前缀异常".to_string());
            }
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(&s[p..p + nlen]);
            p += nlen;
        }
        if out.is_empty() {
            return Err("视频 sample 未包含 NAL".to_string());
        }
        frames.push(out);
    }
    if frames.is_empty() {
        return Err("没有视频帧".to_string());
    }
    Ok(frames)
}

/// 取 AAC 裸帧：sample 本身就是裸 AAC（无 ADTS 头），直接返回。
fn aac_samples(samples: &[Vec<u8>]) -> Result<Vec<Vec<u8>>, String> {
    if samples.is_empty() {
        return Err("没有音频帧".to_string());
    }
    Ok(samples.to_vec())
}

/// 取消当前所有下载。
pub fn cancel_downloads() {
    CANCEL.store(true, Ordering::SeqCst);
}

/// 重置取消标志（每次新的批量任务开始前调用）。
pub fn reset_cancel() {
    CANCEL.store(false, Ordering::SeqCst);
}

// ════════════════════════════════════════════════════════════════════
//  下载主流程
// ════════════════════════════════════════════════════════════════════

/// 下载单个分P。返回产物路径与实际字节数。
///
/// 流程：
/// 1. 取 playurl（真实可用清晰度）
/// 2. 纯音频 → 只下 audio.m4s → 提取 AAC → 封装 .m4a
/// 3. 视频 → 下 video.m4s + audio.m4s → 各自提取裸帧 → remux 成 MP4
pub async fn download_one(
    req: &BiliDownloadRequest,
    total_pages: i64,
    app: Option<&tauri::AppHandle>,
    // 登录 cookie：为空表示免登录（最高 480P）；非空则能取到 720P/1080P
    cookie: Option<&str>,
) -> Result<BiliDownloadResult, String> {
    // 直接 emit 而不走闭包回调：`&mut dyn FnMut` 不是 Send，
    // 会污染整个 future 导致 Tauri 命令编译失败。
    // AppHandle 本身是 Send+Sync，直接持有引用即可。
    let report = |p: BiliProgress| {
        if let Some(a) = app {
            let _ = a.emit("bilibili:progress", p);
        }
    };
    // 1) 播放信息
    report(BiliProgress {
        page: req.page,
        total_pages,
        percent: 0.0,
        downloaded: 0,
        total: 0,
        stage: "解析播放地址".to_string(),
    });
    let (info, dash) = fetch_play_info(&req.bvid, req.cid, cookie).await?;

    if req.audio_only {
        let audio = pick_audio_stream(&dash)
            .ok_or("该视频没有可用音轨，无法提取音频")?;
        let urls = collect_urls(audio.base_url.as_deref(), audio.backup_url.as_deref());

        report(BiliProgress {
            page: req.page,
            total_pages,
            percent: 5.0,
            downloaded: 0,
            total: 0,
            stage: "下载音频".to_string(),
        });
        let bytes = download_to_vec(&urls).await?;

        report(BiliProgress {
            page: req.page,
            total_pages,
            percent: 80.0,
            downloaded: bytes.len() as u64,
            total: bytes.len() as u64,
            stage: "封装 m4a".to_string(),
        });

        let (a_samples_a, _) = extract_samples(&bytes)?;
        let frames = aac_samples(&a_samples_a)?;
        // 优先用 m4s 的 moov 里的真实参数；读不到再按 ADTS 头推断
        let (sr, ch) = read_audio_params_from_moov(&bytes)
            .unwrap_or_else(|| crate::file_convert::infer_audio_params(&frames));
        crate::file_convert::remux_aac_to_m4a(
            &req.output_path,
            &frames,
            sr,
            ch,
            None,
        )?;

        let size = std::fs::metadata(&req.output_path).map(|m| m.len()).unwrap_or(0);
        report(BiliProgress {
            page: req.page,
            total_pages,
            percent: 100.0,
            downloaded: size,
            total: size,
            stage: "完成".to_string(),
        });
        return Ok(BiliDownloadResult {
            output_path: req.output_path.clone(),
            bytes: size,
            quality_label: "仅音频".to_string(),
        });
    }

    // 2) 视频：确定清晰度
    if info.qualities.is_empty() {
        return Err("没有可下载的清晰度（可能该视频需要登录）".to_string());
    }
    let qn = match req.qn {
        Some(q) if info.qualities.iter().any(|x| x.qn == q) => q,
        // 用户指定了不可用的清晰度 → 退到最高可用，并让上层知道实际值
        _ => info.qualities.last().map(|q| q.qn).unwrap_or(32),
    };
    let vq = info
        .qualities
        .iter()
        .find(|q| q.qn == qn)
        .ok_or("未找到目标清晰度")?
        .clone();

    let vstream = pick_video_stream(&dash, qn).ok_or("未找到目标清晰的视频流")?;
    let vurls = collect_urls(vstream.base_url.as_deref(), vstream.backup_url.as_deref());

    // 3) 下载视频
    report(BiliProgress {
        page: req.page,
        total_pages,
        percent: 5.0,
        downloaded: 0,
        total: 0,
        stage: format!("下载视频（{}）", vq.label),
    });
    let vbytes = download_to_vec(&vurls).await?;

    // 4) 下载音频（有音轨时）
    let abytes = if let Some(a) = pick_audio_stream(&dash) {
        let aurls = collect_urls(a.base_url.as_deref(), a.backup_url.as_deref());
        report(BiliProgress {
            page: req.page,
            total_pages,
            percent: 55.0,
            downloaded: vbytes.len() as u64,
            total: 0,
            stage: "下载音频".to_string(),
        });
        Some(download_to_vec(&aurls).await?)
    } else {
        None
    };

    // 5) 合并
    report(BiliProgress {
        page: req.page,
        total_pages,
        percent: 85.0,
        downloaded: vbytes.len() as u64,
        total: 0,
        stage: "合并音视频".to_string(),
    });

    // 视频：sample → AVCC → Annex-B；音频：sample 即裸 AAC
    // 视频：sample → AVCC → Annex-B；同时取出合成时间偏移（B 帧重排序所需）
    let (v_samples, v_ctos) = extract_samples(&vbytes)?;
    let vframes = avcc_samples_to_annexb(&v_samples)?;
    let aframes = match &abytes {
        Some(a) => {
            let (a_samples_b, _) = extract_samples(a)?;
            aac_samples(&a_samples_b)?
        }
        None => Vec::new(),
    };
    // 音频参数：优先 m4s 的 moov，其次 ADTS 头，最后 44100/2 兜底
    let (sr, ch) = abytes
        .as_ref()
        .and_then(|a| read_audio_params_from_moov(a))
        .unwrap_or_else(|| crate::file_convert::infer_audio_params(&aframes));
    // 帧率：用 mvhd 总时长 ÷ 视频帧数推算真实值。
    // 写死 30 会让 25fps 视频的输出时长从 212s 变成 176.9s（实测踩过）。
    let (fps_num, fps_den) =
        read_video_fps(&vbytes, vframes.len()).unwrap_or((30, 1));

    // 从初始化段读真实 SPS/PPS：B 站 fMP4 的帧里不含参数集，
    // 读不到就会写出假 avcC，播放器表现为只有声音没有画面。
    let sps_pps = read_sps_pps_from_moov(&vbytes);
    if sps_pps.is_none() {
        log::warn!("[bilibili] 未能从 m4s 的 moov 读取 SPS/PPS，画面可能无法解码");
    }
    // 合成时间偏移换算：源流用 mdhd.timescale 刻度（实测 16000），
    // 而 muxer 的视频轨时间刻度是 1e7。必须换算，否则 B 帧显示时刻错乱。
    let src_timescale = find_mdhd_timescale_of_first_video(&vbytes).unwrap_or(16000) as i64;
    let ctos_1e7: Vec<i64> = if v_ctos.len() == vframes.len() {
        v_ctos
            .iter()
            .map(|&c| if src_timescale > 0 { c * 10_000_000 / src_timescale } else { c })
            .collect()
    } else {
        // 数量不匹配（解析异常）时退化为全 0：不写 ctts 也不至于产生错误的偏移
        Vec::new()
    };
    if !ctos_1e7.is_empty() {
        let nonzero = ctos_1e7.iter().filter(|&&c| c != 0).count();
        log::info!(
            "[bilibili] ctts: 共 {} 个样本，其中 {} 个非零偏移（源刻度 {} -> 1e7）",
            ctos_1e7.len(),
            nonzero,
            src_timescale
        );
    }

    crate::file_convert::remux_av_to_mp4(
        &req.output_path,
        &vframes,
        &aframes,
        vq.width as u32,
        vq.height as u32,
        fps_num,
        fps_den,
        sr,
        ch,
        None,
        sps_pps,
        &ctos_1e7,
    )?;

    let size = std::fs::metadata(&req.output_path).map(|m| m.len()).unwrap_or(0);
    report(BiliProgress {
        page: req.page,
        total_pages,
        percent: 100.0,
        downloaded: size,
        total: size,
        stage: "完成".to_string(),
    });
    Ok(BiliDownloadResult {
        output_path: req.output_path.clone(),
        bytes: size,
        quality_label: vq.label,
    })
}

/// 把 base_url 与 backup_url 合成候选列表。
fn collect_urls(base: Option<&str>, backups: Option<&[String]>) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    if let Some(b) = base {
        if !b.is_empty() {
            v.push(b.to_string());
        }
    }
    if let Some(list) = backups {
        for b in list {
            if !b.is_empty() && !v.contains(b) {
                v.push(b.clone());
            }
        }
    }
    v
}

/// 把连续 Annex-B 字节流切成「一帧一个元素」。
///
/// 切分依据：遇到 `00 00 00 01` / `00 00 01` 起始码即开新帧。
/// Mp4Muxer 要求逐个样本写入，因此必须切分。
fn split_h264_frames(annexb: &[u8]) -> Vec<Vec<u8>> {
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut i = 0usize;
    while i < annexb.len() {
        // 识别 3 或 4 字节起始码
        let is4 = i + 4 <= annexb.len()
            && annexb[i] == 0
            && annexb[i + 1] == 0
            && annexb[i + 2] == 0
            && annexb[i + 3] == 1;
        let is3 = i + 3 <= annexb.len()
            && annexb[i] == 0
            && annexb[i + 1] == 0
            && annexb[i + 2] == 1;
        if is4 || is3 {
            if !cur.is_empty() {
                frames.push(std::mem::take(&mut cur));
            }
            cur.extend_from_slice(&annexb[i..i + if is4 { 4 } else { 3 }]);
            i += if is4 { 4 } else { 3 };
            continue;
        }
        cur.push(annexb[i]);
        i += 1;
    }
    if !cur.is_empty() {
        frames.push(cur);
    }
    frames
}

/// AAC 裸流按「一帧一个元素」切分。
///
/// B 站 audio.m4s 的每个 sample 就是一帧，`extract_aac_raw` 已按 sample 边界
/// 逐段拼出连续数据，但拼接后丢失了边界 —— 这里用 ADTS 头识别帧边界；
/// 若数据无 ADTS 头（裸 AAC），则整体作为一帧交给 Muxer（MP4 允许大 sample，
/// 只是时间戳粒度变粗，仍可播放）。
fn split_aac_frames(raw: &[u8]) -> Vec<Vec<u8>> {
    // 先判断是否为 ADTS 流
    let looks_adts = raw.len() > 7 && raw[0] == 0xFF && (raw[1] & 0xF0) == 0xF0;
    if !looks_adts {
        return vec![raw.to_vec()];
    }
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut i = 0usize;
    while i + 7 <= raw.len() {
        if raw[i] != 0xFF || (raw[i + 1] & 0xF0) != 0xF0 {
            // 不同步就前移一字节重新找
            i += 1;
            continue;
        }
        // ADTS 帧长：13 位，跨第 3~5 字节
        let len = (((raw[i + 3] & 0x03) as usize) << 11)
            | ((raw[i + 4] as usize) << 3)
            | ((raw[i + 5] as usize) >> 5);
        if len < 7 || i + len > raw.len() {
            break;
        }
        // MP4 里的 AAC sample 不含 ADTS 头，去掉前 7 字节的头部
        frames.push(raw[i + 7..i + len].to_vec());
        i += len;
    }
    if frames.is_empty() {
        vec![raw.to_vec()]
    } else {
        frames
    }
}



// ════════════════════════════════════════════════════════════════════
//  登录（B 站官方二维码登录）
// ════════════════════════════════════════════════════════════════════
//
// 为什么要做登录：未登录时 B 站只下发 480P（见文件头说明）。
// 登录后可拿到 720P/1080P，这也是用户提出该需求的直接原因。
//
// 采用**官方 passport 二维码登录**（与 B 站网页端同一套接口）：
//   1. GET  generate -> 拿到 qrcode_key + 二维码 URL（前端用 qrcode.react 绘制）
//   2. GET  poll     -> 轮询扫码状态；成功后响应里带 Set-Cookie
//   3. 保存 cookie   -> 复用 music_api::cookie 的持久化（与音乐模块同一套存储）
//
// 不采用"内嵌浏览器窗口"方案：那需要用户手动登录并抓 cookie，
// 且窗口参数必须与 tauri.conf.json 逐字一致否则 WebView2 会创建失败，
// 官方扫码接口更简单可靠，也不涉及跨域/签名问题。

/// 二维码登录状态。
#[derive(Debug, Clone, Serialize)]
pub struct BiliLoginStatus {
    /// 0=成功 1=等待扫码 2=已扫码待确认 3=已过期（与 B 站接口 code 对齐）
    pub code: i64,
    pub message: String,
    /// 是否已登录（cookie 里含 SESSDATA 且有效）
    pub logged_in: bool,
}

/// 登录信息（成功后回给前端展示）。
#[derive(Debug, Clone, Serialize)]
pub struct BiliLoginInfo {
    pub mid: i64,
    pub uname: String,
    pub face: String,
}

#[derive(Deserialize)]
struct QrGenResp {
    code: i64,
    message: String,
    data: Option<QrGenData>,
}

#[derive(Deserialize)]
struct QrGenData {
    url: String,
    qrcode_key: String,
}

#[derive(Deserialize)]
struct QrPollResp {
    code: i64,
    message: String,
    data: Option<QrPollData>,
}

#[derive(Deserialize)]
struct QrPollData {
    url: Option<String>,
    refresh_token: Option<String>,
    timestamp: Option<i64>,
    /// 接口自身的状态码：0 成功 / 86101 未扫码 / 86090 已扫码未确认 / 86038 已失效
    code: i64,
    message: Option<String>,
}

/// 生成登录二维码。返回 (qrcode_key, 二维码内容 URL)。
pub async fn login_qrcode() -> Result<(String, String), String> {
    let client = crate::utils::http::client_with_ua(15, Some(10), Some(UA));
    let resp = client
        .get("https://passport.bilibili.com/x/passport-login/web/qrcode/generate")
        .header("Referer", REFERER)
        .send()
        .await
        .map_err(|e| format!("获取登录二维码失败: {e}"))?;
    let body: QrGenResp = resp.json().await.map_err(|e| format!("解析二维码失败: {e}"))?;
    if body.code != 0 {
        return Err(format!("B 站返回错误 {}: {}", body.code, body.message));
    }
    let d = body.data.ok_or("二维码数据为空")?;
    Ok((d.qrcode_key, d.url))
}

/// 轮询扫码结果。成功时返回 (cookie 字符串, 登录信息)。
pub async fn login_poll(qrcode_key: &str) -> Result<(Option<String>, BiliLoginStatus), String> {
    let client = crate::utils::http::client_with_ua(15, Some(10), Some(UA));
    let url = format!(
        "https://passport.bilibili.com/x/passport-login/web/qrcode/poll?qrcode_key={qrcode_key}"
    );
    let resp = client
        .get(&url)
        .header("Referer", REFERER)
        .send()
        .await
        .map_err(|e| format!("轮询登录状态失败: {e}"))?;

    // 关键：登录成功时 cookie 在响应头 Set-Cookie 里，而不是 body。
    // reqwest 未启用 cookie store，需自行拼接。
    let cookies: Vec<String> = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|s| s.split(';').next().map(|x| x.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect();

    let body: QrPollResp = resp.json().await.map_err(|e| format!("解析轮询结果失败: {e}"))?;
    if body.code != 0 {
        return Err(format!("B 站返回错误 {}: {}", body.code, body.message));
    }
    let d = body.data.ok_or("轮询数据为空")?;

    let mut status = BiliLoginStatus {
        code: d.code,
        message: d.message.clone().unwrap_or_default(),
        logged_in: false,
    };
    if d.code == 0 {
        // 拼接 cookie：优先用 Set-Cookie 里的（含 SESSDATA）
        let cookie = cookies
            .iter()
            .filter(|c| {
                let lower = c.to_lowercase();
                lower.starts_with("sessdata=")
                    || lower.starts_with("bili_jct=")
                    || lower.starts_with("dedeuserid=")
                    || lower.starts_with("buvid3=")
            })
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        if !cookie.to_lowercase().contains("sessdata=") {
            return Err("登录成功但未获取到 SESSDATA，请重试".to_string());
        }
        status.logged_in = true;
        return Ok((Some(cookie), status));
    }
    Ok((None, status))
}

/// 用 cookie 校验登录态并取用户信息。cookie 无效时返回 Err。
pub async fn login_check(cookie: &str) -> Result<BiliLoginInfo, String> {
    if !cookie_has_login(cookie) {
        return Err("未登录".to_string());
    }
    let client = crate::utils::http::client_with_ua(15, Some(10), Some(UA));
    let resp = client
        .get("https://api.bilibili.com/x/web-interface/nav")
        .header("Referer", REFERER)
        .header("Cookie", cookie)
        .send()
        .await
        .map_err(|e| format!("校验登录态失败: {e}"))?;
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("解析失败: {e}"))?;
    let code = v.get("code").and_then(|x| x.as_i64()).unwrap_or(-1);
    if code != 0 {
        // -101 表示未登录/cookie 失效
        let msg = v
            .get("message")
            .and_then(|x| x.as_str())
            .unwrap_or("登录态无效");
        return Err(format!("登录态无效（{code}: {msg}）"));
    }
    let d = v.get("data").ok_or("用户信息为空")?;
    let is_login = d.get("isLogin").and_then(|x| x.as_bool()).unwrap_or(false);
    if !is_login {
        return Err("登录态已失效，请重新登录".to_string());
    }
    Ok(BiliLoginInfo {
        mid: d.get("mid").and_then(|x| x.as_i64()).unwrap_or(0),
        uname: d
            .get("uname")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        face: d
            .get("face")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

/// cookie 里是否含 SESSDATA（B 站登录态凭据）。
pub fn cookie_has_login(cookie: &str) -> bool {
    cookie
        .to_lowercase()
        .contains("sessdata=")
}

/// 从持久化存储读取 cookie（复用音乐模块的 provider 存储).
pub fn load_saved_cookie(app: &tauri::AppHandle) -> String {
    crate::music_api::cookie::load_cookie(app, "bilibili").unwrap_or_default()
}


// ════════════════════════════════════════════════════════════════════
//  Tauri 命令
// ════════════════════════════════════════════════════════════════════

use tauri::{AppHandle, Emitter};


/// 生成登录二维码：返回 { qrcode_key, qr_url }。
/// 前端用 qrcode.react 把 qr_url 画成二维码给用户扫。
#[tauri::command]
pub async fn bilibili_login_qrcode() -> Result<serde_json::Value, String> {
    let (key, url) = login_qrcode().await?;
    Ok(serde_json::json!({ "qrcode_key": key, "qr_url": url }))
}

/// 轮询扫码结果。登录成功时自动保存 cookie 并返回用户信息。
#[tauri::command]
pub async fn bilibili_login_poll(app: AppHandle, qrcode_key: String) -> Result<serde_json::Value, String> {
    let (cookie, status) = login_poll(&qrcode_key).await?;
    if let Some(c) = cookie {
        // 复用音乐模块的持久化（同一套加密存储，provider=bilibili）
        crate::music_api::cookie::save_cookie(&app, "bilibili", &c)
            .map_err(|e| format!("保存登录凭据失败: {e}"))?;
        let info = login_check(&c).await.unwrap_or(BiliLoginInfo {
            mid: 0,
            uname: String::new(),
            face: String::new(),
        });
        return Ok(serde_json::json!({
            "code": status.code,
            "message": status.message,
            "logged_in": true,
            "mid": info.mid,
            "uname": info.uname,
            "face": info.face,
        }));
    }
    Ok(serde_json::json!({
        "code": status.code,
        "message": status.message,
        "logged_in": false,
    }))
}

/// 查询当前登录状态与用户信息。
#[tauri::command]
pub async fn bilibili_login_status(app: AppHandle) -> Result<serde_json::Value, String> {
    let cookie = load_saved_cookie(&app);
    if cookie.is_empty() {
        return Ok(serde_json::json!({ "logged_in": false }));
    }
    match login_check(&cookie).await {
        Ok(info) => Ok(serde_json::json!({
            "logged_in": true,
            "mid": info.mid,
            "uname": info.uname,
            "face": info.face,
        })),
        Err(e) => Ok(serde_json::json!({ "logged_in": false, "error": e })),
    }
}

/// 退出登录（清除凭据）。
#[tauri::command]
pub async fn bilibili_logout(app: AppHandle) -> Result<(), String> {
    crate::music_api::cookie::clear_cookie(&app, "bilibili")
        .map_err(|e| format!("清除登录凭据失败: {e}"))
}

/// 带登录态获取清晰度：登录后应能看到 720P/1080P。
#[tauri::command]
pub async fn bilibili_qualities_authed(
    app: AppHandle,
    bvid: String,
    cid: i64,
) -> Result<BiliPlayInfo, String> {
    let cookie = load_saved_cookie(&app);
    let (info, _) = fetch_play_info(&bvid, cid, if cookie.is_empty() { None } else { Some(&cookie) }).await?;
    Ok(info)
}

/// 解析输入（链接/BV/av），返回视频信息。不做下载。
#[tauri::command]
pub async fn bilibili_resolve(input: String) -> Result<BiliVideoInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // 短链需要先跟随重定向；同步上下文里用 block_on 跑 async 解析
        let id = tauri::async_runtime::block_on(async {
            match parse_input(&input)? {
                BiliIdInput::ShortLink(u) => resolve_short_link(&u).await,
                other => Ok(other),
            }
        })?;
        let bvid = tauri::async_runtime::block_on(async {
            match id {
                BiliIdInput::Bvid(b) => Ok(b),
                BiliIdInput::Aid(a) => bvid_from_aid(a).await,
                BiliIdInput::ShortLink(_) => Err("短链解析失败".to_string()),
            }
        })?;
        tauri::async_runtime::block_on(fetch_video_info(&bvid))
    })
    .await
    .map_err(|e| format!("解析任务异常: {e}"))?
}

/// 查询某分P 的可选清晰度（真实可下载的）。
#[tauri::command]
pub async fn bilibili_qualities(bvid: String, cid: i64) -> Result<BiliPlayInfo, String> {
    let (info, _) = fetch_play_info(&bvid, cid, None).await?;
    Ok(info)
}

/// 取消所有下载。
#[tauri::command]
pub fn bilibili_cancel() {
    cancel_downloads();
}

/// 批量下载。逐项执行，全程通过 `bilibili:progress` 事件上报进度。
///
/// 返回每一项的结果（成功含路径与大小，失败含原因）。
#[tauri::command]
pub async fn bilibili_download(
    app: AppHandle,
    requests: Vec<BiliDownloadRequest>,
) -> Result<Vec<Result<BiliDownloadResult, String>>, String> {
    if requests.is_empty() {
        return Err("没有要下载的任务".to_string());
    }
    reset_cancel();
    let total = requests.len() as i64;
    let mut results: Vec<Result<BiliDownloadResult, String>> = Vec::with_capacity(requests.len());

    for (idx, req) in requests.iter().enumerate() {
        if CANCEL.load(Ordering::SeqCst) {
            results.push(Err("已取消".to_string()));
            continue;
        }
        let cookie = load_saved_cookie(&app);
        let r = download_one(req, total, Some(&app), if cookie.is_empty() { None } else { Some(&cookie) }).await;
        // 逐项上报，让界面能立即标记该分P 完成/失败
        let _ = app.emit(
            "bilibili:item",
            serde_json::json!({
                "index": idx,
                "page": req.page,
                "ok": r.is_ok(),
                "path": r.as_ref().ok().map(|x| x.output_path.clone()),
                "quality": r.as_ref().ok().map(|x| x.quality_label.clone()),
                "error": r.as_ref().err().cloned(),
            }),
        );
        results.push(r);
    }

    Ok(results)
}
//  从 m4s 的 moov 读取真实媒体参数
// ════════════════════════════════════════════════════════════════════

/// 从 fMP4 读视频帧率。
///
/// ## 为什么不能用 stts
///
/// B 站的 m4s 是 fragmented MP4，**moov 里的 stts 是空的**
/// （实测 timescale=16000 而 stts.entry_count=0），时长信息分散在
/// 各 moof 的 trun/tfdt 中。早期实现去读 stts 首项 delta，
/// 实际读到的是后续 box 的类型字符串（0x7374616E = "nats"），
/// 于是算出荒谬的帧率 —— 实测 25fps 的视频被当成 30fps，
/// 输出时长从 212s 变成 176.9s，音画严重不同步。
///
/// ## 正确做法
///
/// `mvhd` 的 timescale/duration 是完整总时长（fMP4 也会写），
/// 而视频帧总数可以数 trun 里的 sample_count 累加。
///   fps = 总帧数 / 总时长
/// 再约分成整数分数交给 muxer。这样对 25 / 29.97 / 30 / 60 都成立。
fn read_video_fps(fmp4: &[u8], total_samples: usize) -> Option<(u32, u32)> {
    // 两条路径，按可靠性排序：
    //
    // A) trex.default_sample_duration —— 首选。
    //    B 站的 AVC 流 mvhd.duration 是 **0**（fragmented MP4 常见），
    //    但 moov→mvex→trex 里的 default_sample_duration 是真实帧时长。
    //    实测 timescale=16000、default_sample_duration=640 → 恰好 25.0 fps。
    //    早期版本只读 mvhd，AVC 流上 duration=0 导致回退到 30fps，
    //    于是 25fps 视频的时长从 212s 变成 176.9s（音画不同步）。
    //
    // B) mvhd.duration / 总帧数 —— 兜底，适用于写了 duration 的流。
    if let Some(fps) = fps_from_trex(fmp4) {
        return Some(fps);
    }
    let dur = read_mvhd_duration_secs(fmp4)?;
    if dur <= 0.0 || total_samples == 0 {
        return None;
    }
    normalize_fps(total_samples as f64 / dur)
}

/// 从 moov→mvex→trex 读 (timescale / default_sample_duration)。
fn fps_from_trex(fmp4: &[u8]) -> Option<(u32, u32)> {
    let scale = find_mdhd_timescale_of_first_video(fmp4)?;
    let dsd = find_trex_default_duration(fmp4)?;
    if dsd == 0 {
        return None;
    }
    // 注意：mdhd.timescale / trex.default_sample_duration 得到的是**帧率本身**
    // （例如 16000/640 = 25），而不是要原样传给 muxer 的 (num, den)。
    // muxer 会用 `1e7 * den / num` 作为 stts 的单帧时长（100ns 刻度），
    // 所以必须把这里得到的比值**约分成真实帧率**再传，
    // 否则会把 16000/640 直接塞进去 → stts delta=625 → 时长算成 0.33s。
    let fps = scale as f64 / dsd as f64;
    normalize_fps(fps)
}

/// 在 moov 里找 mvex→trex，取 default_sample_duration。
fn find_trex_default_duration(fmp4: &[u8]) -> Option<u32> {
    let moov = find_top_box(fmp4, b"moov")?;
    find_trex_in_moov(&moov)
}

fn find_trex_in_moov(moov: &[u8]) -> Option<u32> {
    let mut p = 8usize;
    while p + 8 <= moov.len() {
        let size = u32::from_be_bytes([moov[p], moov[p + 1], moov[p + 2], moov[p + 3]]) as usize;
        let typ = &moov[p + 4..p + 8];
        if size < 8 || p + size > moov.len() {
            break;
        }
        if typ == b"mvex" {
            // mvex 内找 trex
            let mut q = 8usize;
            let area = &moov[p..p + size];
            while q + 8 <= area.len() {
                let s2 =
                    u32::from_be_bytes([area[q], area[q + 1], area[q + 2], area[q + 3]]) as usize;
                let t2 = &area[q + 4..q + 8];
                if s2 < 8 || q + s2 > area.len() {
                    break;
                }
                if t2 == b"trex" {
                    // trex box 布局（实测校准，早期版本算错了 4 字节）：
                    //   b[0..4]   size
                    //   b[4..8]   'trex'
                    //   b[8..12]  version+flags
                    //   b[12..16] track_ID
                    //   b[16..20] default_sample_description_index
                    //   b[20..24] default_sample_duration   <-- 要的就是这个
                    //   b[24..28] default_sample_size
                    //   b[28..32] default_sample_flags
                    //
                    // 之前取 b[16..20]（sdi），把「描述索引 1」当成帧时长，
                    // 于是 fps 被算成 mdhd_ts/1 = 16000 → stts delta=625 → 时长 0.33s。
                    let b = &area[q..q + s2];
                    if b.len() >= 24 {
                        return Some(u32::from_be_bytes([b[20], b[21], b[22], b[23]]));
                    }
                }
                q += s2;
            }
        }
        p += size;
    }
    None
}

/// 取第一个**视频** trak 的 mdhd.timescale。
fn find_mdhd_timescale_of_first_video(fmp4: &[u8]) -> Option<u32> {
    let moov = find_top_box(fmp4, b"moov")?;
    let mut p = 8usize;
    while p + 8 <= moov.len() {
        let size = u32::from_be_bytes([moov[p], moov[p + 1], moov[p + 2], moov[p + 3]]) as usize;
        let typ = &moov[p + 4..p + 8];
        if size < 8 || p + size > moov.len() {
            break;
        }
        if typ == b"trak" {
            let trak = &moov[p..p + size];
            if trak_is_video(trak) {
                if let Some(ts) = find_mdhd_timescale(trak) {
                    if ts > 0 {
                        return Some(ts);
                    }
                }
            }
        }
        p += size;
    }
    None
}

/// 取顶层某个 box 的内容（含 box 头）。
fn find_top_box(data: &[u8], name: &[u8; 4]) -> Option<Vec<u8>> {
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let size =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let typ = &data[pos + 4..pos + 8];
        if size < 8 || pos + size > data.len() {
            return None;
        }
        if typ == name {
            return Some(data[pos..pos + size].to_vec());
        }
        pos += size;
    }
    None
}




/// 把浮点帧率对齐到常见帧率，避免浮点误差导致时长漂移。
fn normalize_fps(fps: f64) -> Option<(u32, u32)> {
    if fps <= 0.0 {
        return None;
    }
    for (num, den) in [
        (24000u32, 1001u32),
        (30000, 1001),
        (25, 1),
        (30, 1),
        (24, 1),
        (60, 1),
        (50, 1),
        (48, 1),
        (15, 1),
        (120, 1),
    ] {
        let v = num as f64 / den as f64;
        if (fps - v).abs() < 0.35 {
            return Some((num, den));
        }
    }
    Some((fps.round().max(1.0) as u32, 1))
}

/// 读 mvhd 的总时长（秒）。
fn read_mvhd_duration_secs(fmp4: &[u8]) -> Option<f64> {
    let mut pos = 0usize;
    while pos + 8 <= fmp4.len() {
        let size =
            u32::from_be_bytes([fmp4[pos], fmp4[pos + 1], fmp4[pos + 2], fmp4[pos + 3]]) as usize;
        let typ = &fmp4[pos + 4..pos + 8];
        if size < 8 || pos + size > fmp4.len() {
            return None;
        }
        if typ == b"moov" {
            let end = pos + size;
            let mut p = pos + 8;
            while p + 8 <= end {
                let s2 =
                    u32::from_be_bytes([fmp4[p], fmp4[p + 1], fmp4[p + 2], fmp4[p + 3]]) as usize;
                let t2 = &fmp4[p + 4..p + 8];
                if s2 < 8 || p + s2 > end {
                    break;
                }
                if t2 == b"mvhd" {
                    let b = &fmp4[p..p + s2];
                    if b.len() < 40 {
                        return None;
                    }
                    let ver = b[8];
                    let (ts, du) = if ver == 1 {
                        // ver/flags(4) + ctime(8) + mtime(8) + timescale(4) + duration(8)
                        if b.len() < 8 + 4 + 8 + 8 + 4 + 8 {
                            return None;
                        }
                        (
                            u32::from_be_bytes([b[8 + 4 + 8 + 8], b[8 + 4 + 8 + 9], b[8 + 4 + 8 + 10], b[8 + 4 + 8 + 11]]) as u64,
                            u64::from_be_bytes([
                                b[8 + 4 + 8 + 8 + 4],
                                b[8 + 4 + 8 + 8 + 5],
                                b[8 + 4 + 8 + 8 + 6],
                                b[8 + 4 + 8 + 8 + 7],
                                b[8 + 4 + 8 + 8 + 8],
                                b[8 + 4 + 8 + 8 + 9],
                                b[8 + 4 + 8 + 8 + 10],
                                b[8 + 4 + 8 + 8 + 11],
                            ]),
                        )
                    } else {
                        // ver/flags(4) + ctime(4) + mtime(4) + timescale(4) + duration(4)
                        if b.len() < 8 + 4 + 4 + 4 + 4 + 4 {
                            return None;
                        }
                        (
                            u32::from_be_bytes([b[20], b[21], b[22], b[23]]) as u64,
                            u32::from_be_bytes([b[24], b[25], b[26], b[27]]) as u64,
                        )
                    };
                    if ts == 0 {
                        return None;
                    }
                    return Some(du as f64 / ts as f64);
                }
                p += s2;
            }
            return None;
        }
        pos += size;
    }
    None
}

/// 在 trak 内找 hdlr 的 handler_type，判断是否视频轨。
fn trak_is_video(trak: &[u8]) -> bool {
    let mut p = 8usize;
    while p + 8 <= trak.len() {
        let size = u32::from_be_bytes([trak[p], trak[p + 1], trak[p + 2], trak[p + 3]]) as usize;
        let typ = &trak[p + 4..p + 8];
        if size < 8 || p + size > trak.len() {
            break;
        }
        if typ == b"mdia" {
            return mdia_is_video(&trak[p..p + size]);
        }
        p += size;
    }
    false
}

fn mdia_is_video(mdia: &[u8]) -> bool {
    let mut p = 8usize;
    while p + 8 <= mdia.len() {
        let size = u32::from_be_bytes([mdia[p], mdia[p + 1], mdia[p + 2], mdia[p + 3]]) as usize;
        let typ = &mdia[p + 4..p + 8];
        if size < 8 || p + size > mdia.len() {
            break;
        }
        if typ == b"hdlr" {
            // hdlr: version/flags(4) + pre_defined(4) + handler_type(4)
            if p + 20 <= mdia.len() {
                return &mdia[p + 16..p + 20] == b"vide";
            }
        }
        p += size;
    }
    false
}

fn find_mdhd_timescale(node: &[u8]) -> Option<u32> {
    let mut p = 8usize;
    while p + 8 <= node.len() {
        let size = u32::from_be_bytes([node[p], node[p + 1], node[p + 2], node[p + 3]]) as usize;
        let typ = &node[p + 4..p + 8];
        if size < 8 || p + size > node.len() {
            break;
        }
        if typ == b"mdia" {
            return find_mdhd_timescale(&node[p..p + size]);
        }
        if typ == b"mdhd" {
            let body = &node[p..p + size];
            if body.len() < 12 {
                return None;
            }
            let ver = body[8];
            // v0: ver/flags(4)+ctime(4)+mtime(4)+timescale(4) → off = 8+4+4+4 = 20
            // v1: ver/flags(4)+ctime(8)+mtime(8)+timescale(4) → off = 8+4+8+8 = 28
            let off = if ver == 1 { 8 + 4 + 8 + 8 } else { 8 + 4 + 4 + 4 };
            if off + 4 <= body.len() {
                return Some(u32::from_be_bytes([
                    body[off],
                    body[off + 1],
                    body[off + 2],
                    body[off + 3],
                ]));
            }
            return None;
        }
        p += size;
    }
    None
}


/// 从 fMP4 的 moov 里读音频采样率与声道数。
/// mdhd.timescale 即采样率；声道数从 mp4a sample entry 的 channelcount 读。
pub fn read_audio_params_from_moov(fmp4: &[u8]) -> Option<(u32, u32)> {
    let mut pos = 0usize;
    while pos + 8 <= fmp4.len() {
        let size =
            u32::from_be_bytes([fmp4[pos], fmp4[pos + 1], fmp4[pos + 2], fmp4[pos + 3]]) as usize;
        let typ = &fmp4[pos + 4..pos + 8];
        if size < 8 || pos + size > fmp4.len() {
            return None;
        }
        if typ == b"moov" {
            let end = pos + size;
            let mut p = pos + 8;
            while p + 8 <= end {
                let s2 =
                    u32::from_be_bytes([fmp4[p], fmp4[p + 1], fmp4[p + 2], fmp4[p + 3]]) as usize;
                let t2 = &fmp4[p + 4..p + 8];
                if s2 < 8 || p + s2 > end {
                    break;
                }
                if t2 == b"trak" {
                    let trak = &fmp4[p..p + s2];
                    if !trak_is_video(trak) {
                        // 采样率（mdhd.timescale）是关键：写错会让整条音轨时长按比例偏移
                        // （实测把 48000 当 44100 用，212s 的视频音频轨变成 231s）。
                        // 声道数读不到时按 2 兜底 —— 不能因为声道数失败就丢掉采样率。
                        if let Some(ts) = find_mdhd_timescale(trak) {
                            let ch = find_mp4a_channelcount(trak).unwrap_or(2).max(1);
                            return Some((ts, ch));
                        }
                    }
                }
                p += s2;
            }
            return None;
        }
        pos += size;
    }
    None
}

/// 在 trak 内找 stsd → mp4a 的 channelcount。
fn find_mp4a_channelcount(node: &[u8]) -> Option<u32> {
    let mut p = 8usize;
    while p + 8 <= node.len() {
        let size = u32::from_be_bytes([node[p], node[p + 1], node[p + 2], node[p + 3]]) as usize;
        let typ = &node[p + 4..p + 8];
        if size < 8 || p + size > node.len() {
            break;
        }
        if typ == b"stbl" {
            return find_mp4a_channelcount(&node[p..p + size]);
        }
        if typ == b"stsd" {
            let body = &node[p..p + size];
            let mut q = 16usize;
            while q + 8 <= body.len() {
                let es =
                    u32::from_be_bytes([body[q], body[q + 1], body[q + 2], body[q + 3]]) as usize;
                let et = &body[q + 4..q + 8];
                if es < 8 || q + es > body.len() {
                    break;
                }
                if et == b"mp4a" {
                    // mp4a sample entry:
                    // 8(box) + 6(reserved) + 2(data_ref_idx) + 8(ver/rev/vendor) + 2(channelcount)
                    let cc_off = q + 8 + 6 + 2 + 8;
                    if cc_off + 2 <= body.len() {
                        return Some(u16::from_be_bytes([body[cc_off], body[cc_off + 1]]) as u32);
                    }
                }
                q += es;
            }
            return None;
        }
        p += size;
    }
    None
}
