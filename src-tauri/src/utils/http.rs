//! 全局共享的 HTTP 客户端。
//!
//! 背景：`reqwest::Client::new()` 每次调用都会创建一个**独立的连接池 + TLS 配置对象**。
//! 原先各模块在自己的函数里反复构造 Client（音乐搜索、歌词、广告、公告、统计上报等），
//! 结果是每发一次请求都要重新做 TCP 三次握手 + TLS 协商，连接复用完全失效。
//! 音乐搜索这类操作一次要打十几个接口，这部分开销按「请求数 × 握手耗时」直接叠加。
//!
//! 这里按**超时值**缓存客户端：
//! - 同一超时值 → 同一实例 → 共享连接池，keep-alive 生效；
//! - 不同超时值各自独立实例，**不改变任何调用方原有的超时语义**。
//!
//! `reqwest::Client` 内部持有 `Arc`，clone 不复制连接池；本模块返回 `&'static Client`，
//! 调用方直接引用即可，无需 clone。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// 常规请求默认超时（音乐接口偶发慢响应，给 15 秒）。
pub const DEFAULT_TIMEOUT_SECS: u64 = 15;

/// 默认客户端（15 秒超时）。
pub fn client() -> &'static reqwest::Client {
    client_with_timeout(DEFAULT_TIMEOUT_SECS)
}

/// 按超时秒数取共享客户端。
///
/// 内部按秒数精确缓存，因此不会改变调用方的超时语义。
pub fn client_with_timeout(secs: u64) -> &'static reqwest::Client {
    static CACHE: OnceLock<Mutex<HashMap<u64, &'static reqwest::Client>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    // 快路径：命中直接返回。这里用 std Mutex 而非 parking_lot，
    // 因为临界区只有一次哈希查找，且该方法只在请求发起时调用（不在渲染热循环里）。
    if let Ok(guard) = cache.lock() {
        if let Some(client) = guard.get(&secs) {
            return client;
        }
    }

    // 慢路径：构建并泄漏一个实例。客户端本身是全程序共享的长生命周期对象，
    // 每个超时值只会构建一次，泄漏量 = 不同超时值的个数（实际个位数）。
    let built: &'static reqwest::Client = Box::leak(Box::new(build(secs)));
    if let Ok(mut guard) = cache.lock() {
        // 并发下可能已有其它线程先插入同一超时值；保留先到者，避免重复的连接池。
        return *guard.entry(secs).or_insert(built);
    }
    built
}

/// 按超时**毫秒**取共享客户端。
///
/// 探测类请求常用 2.5 秒这类非整秒超时，若统一归并到秒档会改变超时语义，
/// 因此单独提供毫秒缓存。
///
/// ⚠️ **必须做档位量化**：调用方会传入按剩余时间动态算出的超时
/// （例如 QQ 音乐探测用 `800ms.min(剩余时间)`，每次循环取值都不同）。
/// 若按原始毫秒值做缓存键，每个不同取值都会 `Box::leak` 一个全新 Client
/// （连同它自己的连接池），一首首换歌探测就会持续泄漏，永不回收。
/// 因此这里把毫秒向上量化到 [`MS_GRANULARITY`] 的整数倍：
/// 缓存条目数被限制在 (上限 / 粒度) 个，且超时只会略变长、不会变短，
/// 既不破坏「超时不要过短」的语义，也杜绝无限增长。
pub fn client_with_timeout_ms(millis: u64) -> &'static reqwest::Client {
    /// 量化粒度（毫秒）。16 档覆盖 800ms 探测这类场景，误差 ≤ 粒度数。
    const MS_GRANULARITY: u64 = 50;
    /// 超过这个上限不再细分，直接按上限处理。
    const MS_MAX: u64 = 60_000;

    let quantized = millis.min(MS_MAX).div_ceil(MS_GRANULARITY) * MS_GRANULARITY;
    // 保证至少 1 个粒度，避免 0 超时导致请求立即失败。
    let quantized = quantized.max(MS_GRANULARITY);

    static CACHE: OnceLock<Mutex<HashMap<u64, &'static reqwest::Client>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Ok(guard) = cache.lock() {
        if let Some(client) = guard.get(&quantized) {
            return client;
        }
    }

    let built: &'static reqwest::Client = Box::leak(Box::new(
        reqwest::Client::builder()
            .timeout(Duration::from_millis(quantized))
            .build()
            .unwrap_or_else(|e| {
                log::warn!("[http] 共享客户端构建失败(timeout={quantized}ms)，退化为默认客户端: {e}");
                reqwest::Client::new()
            }),
    ));
    if let Ok(mut guard) = cache.lock() {
        return *guard.entry(quantized).or_insert(built);
    }
    built
}

/// 取带 `connect_timeout` 的共享客户端（用于需要单独限制连接阶段的接口）。
///
/// 连接超时和整体超时都会影响连接池配置，故按 (整体秒数, 连接秒数) 二元组缓存。
pub fn client_for(timeout_secs: u64, connect_timeout_secs: Option<u64>) -> &'static reqwest::Client {
    client_with_ua(timeout_secs, connect_timeout_secs, None)
}

/// 取带自定义 `User-Agent` 的共享客户端。
///
/// **为什么必须保留 UA**：`reqwest` **默认不发送 User-Agent 头**。
/// 之前把各模块的 `Client::builder()` 统一替换成共享客户端时，顺手丢掉了
/// `.user_agent(...)`，导致 GitHub API（强制要求 UA）直接返回 403，
/// 「检查更新」与「公告」接口全部失效，只能回退到本地缓存。
/// 因此这里把 UA 纳入缓存键：同 UA 复用连接池，不同 UA 各自独立实例。
pub fn client_with_ua(
    timeout_secs: u64,
    connect_timeout_secs: Option<u64>,
    user_agent: Option<&str>,
) -> &'static reqwest::Client {
    type Key = (u64, Option<u64>, Option<String>);
    static CACHE: OnceLock<Mutex<HashMap<Key, &'static reqwest::Client>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key: Key = (
        timeout_secs,
        connect_timeout_secs,
        user_agent.map(|s| s.to_string()),
    );

    if let Ok(guard) = cache.lock() {
        if let Some(client) = guard.get(&key) {
            return client;
        }
    }

    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .connect_timeout(Duration::from_secs(connect_timeout_secs.unwrap_or(10)));
    if let Some(ua) = user_agent {
        builder = builder.user_agent(ua);
    }

    let built: &'static reqwest::Client = Box::leak(Box::new(builder.build().unwrap_or_else(|e| {
        log::warn!("[http] 共享客户端构建失败(timeout={timeout_secs}s): {e}");
        reqwest::Client::new()
    })));
    if let Ok(mut guard) = cache.lock() {
        return *guard.entry(key).or_insert(built);
    }
    built
}

fn build(secs: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(secs))
        .build()
        .unwrap_or_else(|e| {
            // 构建失败极罕见（通常是 TLS 后端初始化问题）。
            // 退化为默认客户端而不是 panic：网络功能降级好过整个程序崩溃。
            log::warn!("[http] 共享客户端构建失败(timeout={secs}s)，退化为默认客户端: {e}");
            reqwest::Client::new()
        })
}

/// 流式下载用客户端：不设整体超时（大文件不能按总时长掐断），
/// 但限制连接阶段，避免连接阶段无限挂起。
///
/// 注意：reqwest 0.11 没有 `read_timeout`（该 API 是 0.12 才加入），
/// 因此这里只用 `connect_timeout`；响应停滞由各调用方自己的读循环保护。
pub fn download_client() -> &'static reqwest::Client {
    static DOWNLOAD: OnceLock<reqwest::Client> = OnceLock::new();
    DOWNLOAD.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|e| {
                log::warn!("[http] 下载客户端构建失败，退化为默认客户端: {e}");
                reqwest::Client::new()
            })
    })
}
