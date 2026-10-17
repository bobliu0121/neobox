//! Spotify 试听支持（官方 Web API）
//!
//! 范围（用户已确认：移植 Spotify「试听版」）：
//! - 配置：用户自备 Client ID + Client Secret（developer.spotify.com 免费创建应用）
//! - 认证：client_credentials（无需 Spotify 账号）→ access_token（内存缓存，官方有效期 1 小时）
//! - 搜索：GET /v1/search?type=track
//! - 播放：track.preview_url —— Spotify 官方仅提供 30 秒试听片段；
//!   完整播放需要用户 Spotify 账号登录 + Premium + Web Playback SDK，不在本实现范围。
//!
//! 参考：Mineradio (github.com/XxHuberrr/Mineradio-paused) spotify-api.js 的实现思路。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde_json::{json, Value};
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

use super::models::*;
use super::cookie;

const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const API_BASE: &str = "https://api.spotify.com/v1";
const DEFAULT_MARKET: &str = "US";
/// 官方 token 有效期 1 小时，提前 1 分钟刷新
const TOKEN_TTL: Duration = Duration::from_secs(3600 - 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 内存 token 缓存（access_token, 获取时刻）
static TOKEN_CACHE: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// 读取已保存的 Client ID / Secret（music-cookies.json 中 key=spotify_credentials）
fn read_credentials(app: &AppHandle) -> Option<(String, String)> {
    let store = app.store(cookie::STORE_FILE).ok()?;
    let val = store.get("spotify_credentials")?;
    let client_id = val.get("client_id")?.as_str()?.to_string();
    let client_secret = val.get("client_secret")?.as_str()?.to_string();
    if client_id.is_empty() || client_secret.is_empty() {
        None
    } else {
        Some((client_id, client_secret))
    }
}

/// client_credentials 流程换取 access_token
async fn fetch_access_token(app: &AppHandle) -> Result<String, String> {
    let (client_id, client_secret) = read_credentials(app)
        .ok_or_else(|| "Spotify 未配置：请先在登录面板填写 Client ID 与 Client Secret（developer.spotify.com 免费创建应用）".to_string())?;
    let basic = B64.encode(format!("{client_id}:{client_secret}"));
    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_URL)
        .header("Authorization", format!("Basic {basic}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&[("grant_type", "client_credentials")])
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("Spotify 认证请求失败：{e}"))?;
    let json: Value = resp
        .json()
        .await
        .map_err(|e| format!("Spotify 认证响应解析失败：{e}"))?;
    let token = json
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Spotify 认证失败：未返回 access_token（请检查 Client ID / Client Secret 是否正确）".to_string())?;
    Ok(token.to_string())
}

/// 获取 access_token（带缓存）
async fn get_access_token(app: &AppHandle) -> Result<String, String> {
    if let Some((token, at)) = TOKEN_CACHE.lock().unwrap().as_ref() {
        if at.elapsed() < TOKEN_TTL {
            return Ok(token.clone());
        }
    }
    let token = fetch_access_token(app).await?;
    *TOKEN_CACHE.lock().unwrap() = Some((token.clone(), Instant::now()));
    Ok(token)
}

/// 搜索单曲（/v1/search?type=track）
#[tauri::command]
pub async fn spotify_search(app: AppHandle, keywords: String, limit: Option<u32>) -> Result<Vec<Song>, String> {
    search_impl(&app, &keywords, limit.unwrap_or(30)).await
}

async fn search_impl(app: &AppHandle, keywords: &str, limit: u32) -> Result<Vec<Song>, String> {
    let token = get_access_token(app).await?;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{API_BASE}/search"))
        .header("Authorization", format!("Bearer {token}"))
        .query(&[
            ("q", keywords),
            ("type", "track"),
            ("limit", &limit.min(50).to_string()),
            ("market", DEFAULT_MARKET),
        ])
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("Spotify 搜索请求失败：{e}"))?;
    let json: Value = resp
        .json()
        .await
        .map_err(|e| format!("Spotify 搜索响应解析失败：{e}"))?;
    let items = json
        .pointer("/tracks/items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut songs = Vec::new();
    for t in items {
        let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        let id = t.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let preview = t.get("preview_url").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let artists: Vec<Artist> = t
            .get("artists")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|a| Artist {
                id: a.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()),
                mid: None,
                name: a.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                pic_url: None,
                music_size: None,
            })
            .collect();
        let artist = artists.first().map(|a| a.name.clone()).unwrap_or_default();
        let album = t.pointer("/album/name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let cover = t
            .pointer("/album/images/0/url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let duration = t.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0);

        songs.push(Song {
            provider: "spotify".into(),
            id,
            mid: None,
            media_mid: None,
            name,
            artist,
            artists,
            album,
            cover,
            duration,
            // Spotify 无「VIP 会员」付费墙概念：30 秒试听对所有用户免费
            fee: 0,
            // 有 preview_url 才可播放
            playable: !preview.is_empty(),
            language: 0,
            hash: None,
            album_id: None,
            album_audio_id: None,
            hq_hash: None,
            sq_hash: None,
            res_hash: None,
            qq_song_id: None,
            content_id: None,
        });
    }
    Ok(songs)
}

/// 取播放地址（/v1/tracks/{id} → preview_url）
#[tauri::command]
pub async fn spotify_song_url(app: AppHandle, id: String) -> Result<SongUrlResult, String> {
    song_url_impl(&app, &id).await
}

async fn song_url_impl(app: &AppHandle, id: &str) -> Result<SongUrlResult, String> {
    let token = get_access_token(app).await?;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{API_BASE}/tracks/{id}"))
        .header("Authorization", format!("Bearer {token}"))
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("Spotify 获取歌曲失败：{e}"))?;
    let json: Value = resp
        .json()
        .await
        .map_err(|e| format!("Spotify 歌曲响应解析失败：{e}"))?;
    let preview = json.get("preview_url").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if preview.is_empty() {
        return Ok(SongUrlResult {
            url: None,
            playable: false,
            trial: false,
            level: String::new(),
            quality: String::new(),
            br: 0,
            reason: Some("no_preview".into()),
            message: Some("该歌曲没有 30 秒试听片段（Spotify 官方限制），完整播放需在 Spotify 客户端登录 Premium 会员".into()),
            fee: None,
        });
    }
    Ok(SongUrlResult {
        url: Some(preview),
        playable: true,
        trial: true,
        level: String::new(),
        quality: "preview".into(),
        br: 0,
        reason: Some("spotify_preview".into()),
        message: Some("Spotify 官方试听片段（30 秒）".into()),
        fee: None,
    })
}

/// 保存 Client ID / Client Secret（等价于「登录/配置」）
#[tauri::command]
pub fn spotify_configure(app: AppHandle, client_id: String, client_secret: String) -> Result<LoginInfo, String> {
    configure_impl(&app, &client_id, &client_secret)
}

fn configure_impl(app: &AppHandle, client_id: &str, client_secret: &str) -> Result<LoginInfo, String> {
    let cid = client_id.trim();
    let cs = client_secret.trim();
    if cid.is_empty() || cs.is_empty() {
        return Err("Client ID 与 Client Secret 不能为空".into());
    }
    let store = app.store(cookie::STORE_FILE).map_err(|e| e.to_string())?;
    store.set("spotify_credentials", json!({ "client_id": cid, "client_secret": cs }));
    store.save().map_err(|e| format!("保存 Spotify 凭据失败：{e}"))?;
    // 凭据变更后强制刷新 token
    *TOKEN_CACHE.lock().unwrap() = None;
    Ok(spotify_status_impl(app))
}

/// 登录状态：配置了凭据即视为「已登录」（匿名可搜索/试听）
#[tauri::command]
pub fn spotify_status(app: AppHandle) -> LoginInfo {
    spotify_status_impl(&app)
}

pub fn spotify_status_impl(app: &AppHandle) -> LoginInfo {
    let configured = read_credentials(app).is_some();
    LoginInfo {
        provider: "spotify".into(),
        logged_in: configured,
        user_id: String::new(),
        nickname: if configured { "已配置 Spotify API".into() } else { String::new() },
        avatar: String::new(),
        vip_type: 0,
        vip_level: "preview".into(),
        is_vip: false,
        is_svip: false,
    }
}

/// 清除凭据（登出）
#[tauri::command]
pub fn spotify_logout(app: AppHandle) -> Result<(), String> {
    clear_impl(&app)
}

fn clear_impl(app: &AppHandle) -> Result<(), String> {
    let store = app.store(cookie::STORE_FILE).map_err(|e| e.to_string())?;
    store.delete("spotify_credentials");
    store.save().map_err(|e| format!("清除 Spotify 凭据失败：{e}"))?;
    *TOKEN_CACHE.lock().unwrap() = None;
    Ok(())
}
