//! GitHub Release 历史：主页「公告」卡片的数据源。
//!
//! 不再依赖第三方公告服务：直接读取 `bobliu0121/neobox` 的 Releases，
//! 以标签、标题、发布说明正文和发布时间展示。带 30 分钟内存缓存 + 磁盘缓存，
//! 离线时回退到上一次成功结果；首次启动且断网时返回内置的 1.0.0 条目。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

const RELEASES_URL: &str =
    "https://api.github.com/repos/bobliu0121/neobox/releases?per_page=30";
const CACHE_FILE_NAME: &str = "releases_cache.json";
const CACHE_VERSION: u32 = 1;
const CONNECT_TIMEOUT_SECS: u64 = 4;
const REQUEST_TIMEOUT_SECS: u64 = 8;
/// 内存缓存有效期：同一会话内避免反复打 GitHub API（未登录限流 60 次/小时）
const MEMORY_CACHE_TTL_SECS: u64 = 30 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    /// 版本标签，例如 `v1.0.0`
    pub tag: String,
    /// Release 标题（为空时回退标签）
    pub name: String,
    /// Markdown 发布说明
    pub body: String,
    /// ISO 8601 发布时间
    pub published_at: String,
    pub html_url: String,
    pub prerelease: bool,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    html_url: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReleaseCache {
    version: u32,
    fetched_at: u64,
    releases: Vec<ReleaseInfo>,
}

struct MemoryCache {
    releases: Option<Vec<ReleaseInfo>>,
    fetched_at: Option<Instant>,
}

impl MemoryCache {
    fn new() -> Self {
        Self {
            releases: None,
            fetched_at: None,
        }
    }

    fn get(&self) -> Option<Vec<ReleaseInfo>> {
        if let (Some(releases), Some(fetched_at)) = (&self.releases, self.fetched_at) {
            if fetched_at.elapsed() < Duration::from_secs(MEMORY_CACHE_TTL_SECS) {
                return Some(releases.clone());
            }
        }
        None
    }
}

static MEMORY_CACHE: OnceLock<Arc<RwLock<MemoryCache>>> = OnceLock::new();

fn get_memory_cache() -> Arc<RwLock<MemoryCache>> {
    MEMORY_CACHE
        .get_or_init(|| Arc::new(RwLock::new(MemoryCache::new())))
        .clone()
}

fn cache_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|p| p.join("NeoBox").join(CACHE_FILE_NAME))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn save_cache(releases: &[ReleaseInfo]) {
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let cache = ReleaseCache {
        version: CACHE_VERSION,
        fetched_at: now_secs(),
        releases: releases.to_vec(),
    };
    if let Ok(json) = serde_json::to_string(&cache) {
        let _ = fs::write(path, json);
    }
}

fn load_cache() -> Option<Vec<ReleaseInfo>> {
    let path = cache_path()?;
    let content = fs::read_to_string(path).ok()?;
    let cache: ReleaseCache = serde_json::from_str(&content).ok()?;
    if cache.version != CACHE_VERSION || cache.releases.is_empty() {
        return None;
    }
    Some(cache.releases)
}

fn fallback_release() -> ReleaseInfo {
    ReleaseInfo {
        tag: "v1.0.0".to_string(),
        name: "NeoBox 1.0.0".to_string(),
        body: "当前无法连接 GitHub 获取完整发布记录，可点击下方链接查看仓库 Releases。".to_string(),
        published_at: String::new(),
        html_url: "https://github.com/bobliu0121/neobox/releases".to_string(),
        prerelease: false,
    }
}

async fn fetch_releases() -> Result<Vec<ReleaseInfo>, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .user_agent("NeoBox/1.0 (+https://github.com/bobliu0121/neobox)")
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;

    let response = client
        .get(RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("请求 GitHub Releases 失败: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("GitHub API 返回 HTTP {}", response.status()));
    }

    let releases: Vec<GithubRelease> = response
        .json()
        .await
        .map_err(|e| format!("解析 Releases 失败: {e}"))?;

    let mapped: Vec<ReleaseInfo> = releases
        .into_iter()
        .filter(|release| !release.draft)
        .map(|release| {
            let name = release
                .name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| release.tag_name.clone());
            ReleaseInfo {
                tag: release.tag_name,
                name,
                body: release.body.unwrap_or_default(),
                published_at: release.published_at.unwrap_or_default(),
                html_url: release.html_url,
                prerelease: release.prerelease,
            }
        })
        .collect();

    if mapped.is_empty() {
        return Err("仓库还没有 Release".to_string());
    }
    save_cache(&mapped);
    Ok(mapped)
}

#[tauri::command]
pub async fn get_release_history() -> Vec<ReleaseInfo> {
    {
        let cache = get_memory_cache();
        let guard = cache.read().await;
        if let Some(releases) = guard.get() {
            return releases;
        }
    }

    match fetch_releases().await {
        Ok(releases) => {
            let cache = get_memory_cache();
            let mut guard = cache.write().await;
            guard.releases = Some(releases.clone());
            guard.fetched_at = Some(Instant::now());
            releases
        }
        Err(error) => {
            log::warn!("[Releases] 获取失败，回退缓存: {error}");
            let cached = load_cache();
            let releases = cached.unwrap_or_else(|| vec![fallback_release()]);
            let cache = get_memory_cache();
            let mut guard = cache.write().await;
            guard.releases = Some(releases.clone());
            guard.fetched_at = Some(Instant::now());
            releases
        }
    }
}
