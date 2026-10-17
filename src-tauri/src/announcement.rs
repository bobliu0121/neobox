use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

const ANNOUNCEMENT_URL: &str = "https://gitee.com/muliuawa/neobox/raw/master/notice.json";
const CACHE_FILE_NAME: &str = "announcement_cache.json";
const CONNECT_TIMEOUT_SECS: u64 = 3;
const REQUEST_TIMEOUT_SECS: u64 = 5;
const MEMORY_CACHE_TTL_SECS: u64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Announcement {
    pub title: String,
    pub content: String,
    pub important: bool,
    pub create_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnnouncementResponse {
    pub version: u32,
    pub announce_list: Vec<Announcement>,
}

struct MemoryCache {
    data: Option<AnnouncementResponse>,
    fetched_at: Option<Instant>,
}

impl MemoryCache {
    fn new() -> Self {
        Self {
            data: None,
            fetched_at: None,
        }
    }

    fn get(&self) -> Option<AnnouncementResponse> {
        if let (Some(data), Some(fetched_at)) = (&self.data, self.fetched_at) {
            if fetched_at.elapsed() < Duration::from_secs(MEMORY_CACHE_TTL_SECS) {
                return Some(data.clone());
            }
        }
        None
    }

    fn set(&mut self, data: AnnouncementResponse) {
        self.data = Some(data);
        self.fetched_at = Some(Instant::now());
    }
}

static MEMORY_CACHE: OnceLock<Arc<RwLock<MemoryCache>>> = OnceLock::new();

fn get_memory_cache() -> Arc<RwLock<MemoryCache>> {
    MEMORY_CACHE
        .get_or_init(|| Arc::new(RwLock::new(MemoryCache::new())))
        .clone()
}

fn get_cache_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|p| p.join("NeoBox").join(CACHE_FILE_NAME))
}

fn save_to_cache(data: &AnnouncementResponse) {
    if let Some(cache_path) = get_cache_path() {
        if let Some(parent) = cache_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(data) {
            let _ = fs::write(&cache_path, json);
        }
    }
}

fn load_from_cache() -> Option<AnnouncementResponse> {
    if let Some(cache_path) = get_cache_path() {
        if cache_path.exists() {
            if let Ok(content) = fs::read_to_string(&cache_path) {
                if let Ok(data) = serde_json::from_str::<AnnouncementResponse>(&content) {
                    return Some(data);
                }
            }
        }
    }
    None
}

pub async fn fetch_announcements() -> Result<AnnouncementResponse, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let response = client
        .get(ANNOUNCEMENT_URL)
        .send()
        .await
        .map_err(|e| format!("Network request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("HTTP error: {}", response.status()));
    }

    let text = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;

    let data: AnnouncementResponse = serde_json::from_str(&text)
        .map_err(|e| format!("JSON parse error: {}", e))?;

    save_to_cache(&data);

    let cache = get_memory_cache();
    cache.write().await.set(data.clone());

    Ok(data)
}

#[tauri::command]
pub async fn get_announcements() -> AnnouncementResponse {
    {
        let cache = get_memory_cache();
        if let Some(data) = cache.read().await.get() {
            return data;
        };
    }

    match fetch_announcements().await {
        Ok(data) => data,
        Err(e) => {
            log::warn!("Failed to fetch announcements: {}, trying cache...", e);
            load_from_cache().unwrap_or_default()
        }
    }
}

#[tauri::command]
pub async fn get_important_announcements() -> Vec<Announcement> {
    let announcements = get_announcements().await;
    announcements
        .announce_list
        .into_iter()
        .filter(|a| a.important)
        .collect()
}
