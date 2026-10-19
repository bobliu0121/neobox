use std::time::Duration;

use reqwest::header::{COOKIE, REFERER, USER_AGENT};
use serde_json::{json, Value};

use super::crypto::{build_eapi_header, encrypt_eapi_payload, encrypt_weapi_payload};
use super::models::*;

const NETEASE_USER_AGENT: &str = "Mozilla/5.0 (Linux; Android 9; PCT-AL10) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/70.0.3538.64 HuaweiBrowser/10.0.3.311 Mobile Safari/537.36";
const EAPI_BASE: &str = "https://interface3.music.163.com/eapi";

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("Failed to build HTTP client")
}

/// 发送普通 POST 请求 (带 Cookie, 不加密)
/// 参考 NeteaseCloudMusicApi / Mineradio 的请求方式
async fn post_api(
    client: &reqwest::Client,
    api_path: &str,
    params: &[(&str, &str)],
    cookie: &str,
) -> Result<Value, String> {
    let url = format!("https://music.163.com/api{}", &api_path[4..]); // /api/xxx -> https://music.163.com/api/xxx

    let resp = client
        .post(&url)
        .header(USER_AGENT, NETEASE_USER_AGENT)
        .header(REFERER, "https://music.163.com/")
        .header(COOKIE, cookie)
        .form(params)
        .send()
        .await
        .map_err(|e| format!("POST {api_path} failed: {e}"))?;

    let text = resp.text().await.map_err(|e| format!("Failed to read response: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("Failed to parse JSON from {api_path}: {e}"))
}

/// 发送普通 GET 请求 (带 Cookie, 不加密)
async fn get_api(
    client: &reqwest::Client,
    api_path: &str,
    params: &[(&str, &str)],
    cookie: &str,
) -> Result<Value, String> {
    let url = format!("https://music.163.com/api{}", &api_path[4..]);

    let resp = client
        .get(&url)
        .header(USER_AGENT, NETEASE_USER_AGENT)
        .header(REFERER, "https://music.163.com/")
        .header(COOKIE, cookie)
        .query(params)
        .send()
        .await
        .map_err(|e| format!("GET {api_path} failed: {e}"))?;

    let text = resp.text().await.map_err(|e| format!("Failed to read response: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("Failed to parse JSON from {api_path}: {e}"))
}

/// EAPI 请求结果 (包含 JSON 和 Set-Cookie)
struct EapiResult {
    json: Value,
    cookies: String, // 从 Set-Cookie 头提取的 cookie 字符串
}

/// 发送 EAPI 请求 (捕获 Set-Cookie)
async fn post_eapi_full(
    client: &reqwest::Client,
    api_path: &str,
    payload: serde_json::Map<String, Value>,
    user_cookie: &str,
) -> Result<EapiResult, String> {
    let url = format!("{EAPI_BASE}{}", &api_path[4..]); // 去掉 /api 前缀拼接

    let mut full_payload = payload;
    let header = build_eapi_header();
    full_payload.insert(
        "header".into(),
        Value::String(serde_json::to_string(&header).map_err(|e| e.to_string())?),
    );

    let payload_text = serde_json::to_string(&full_payload).map_err(|e| e.to_string())?;
    let encrypted = encrypt_eapi_payload(api_path, &payload_text)?;

    // 合并 header cookie 和用户 cookie
    let header_cookie = header
        .iter()
        .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("")))
        .collect::<Vec<_>>()
        .join("; ");

    let full_cookie = if user_cookie.is_empty() {
        header_cookie
    } else {
        format!("{header_cookie}; {user_cookie}")
    };

    let resp = client
        .post(&url)
        .header(USER_AGENT, NETEASE_USER_AGENT)
        .header(REFERER, "https://music.163.com/")
        .header(COOKIE, &full_cookie)
        .form(&[("params", encrypted.as_str())])
        .send()
        .await
        .map_err(|e| format!("EAPI request failed: {e}"))?;

    // 提取 Set-Cookie 头中的所有 cookie
    let cookies: Vec<String> = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|s| {
            // 取每条 Set-Cookie 的第一部分 (key=value)
            let part = s.split(';').next()?.trim();
            if part.is_empty() {
                None
            } else {
                Some(part.to_string())
            }
        })
        .collect();
    let cookie_str = cookies.join("; ");

    let text = resp
        .text()
        .await
        .map_err(|e| format!("Failed to read EAPI response: {e}"))?;

    let json = serde_json::from_str(&text)
        .map_err(|e| format!("Failed to parse EAPI response: {e}"))?;

    Ok(EapiResult { json, cookies: cookie_str })
}

/// 发送 EAPI 请求 (仅返回 JSON)
async fn post_eapi(
    client: &reqwest::Client,
    api_path: &str,
    payload: serde_json::Map<String, Value>,
    user_cookie: &str,
) -> Result<Value, String> {
    post_eapi_full(client, api_path, payload, user_cookie)
        .await
        .map(|r| r.json)
}

/// 网易云 weapi 桌面端 User-Agent (参考 NeteaseCloudMusicApi)
const NETEASE_WEAPI_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36";

/// 发送 weapi 加密请求 (每日推荐歌单等需要 weapi 的接口)
/// /api/v1/xxx -> https://music.163.com/weapi/v1/xxx
async fn post_weapi(
    client: &reqwest::Client,
    api_path: &str,
    payload: serde_json::Map<String, Value>,
    cookie: &str,
) -> Result<Value, String> {
    let url = format!("https://music.163.com/weapi{}", &api_path[4..]);

    let mut full = payload;
    // 从 cookie 提取 csrf_token（点赞/发评论等写操作必须与 cookie 中的 csrf 匹配）
    let cookie_map = super::cookie::parse_cookie_string(cookie);
    let csrf = cookie_map
        .get("__csrf")
        .or_else(|| cookie_map.get("csrf_token"))
        .cloned()
        .unwrap_or_default();
    full.insert("csrf_token".into(), json!(csrf));
    let payload_text = serde_json::to_string(&full)
        .map_err(|e| format!("Failed to serialize weapi payload: {e}"))?;
    let (params, enc_sec_key) = encrypt_weapi_payload(&payload_text);

    let resp = client
        .post(&url)
        .header(USER_AGENT, NETEASE_WEAPI_USER_AGENT)
        .header(REFERER, "https://music.163.com/")
        .header(COOKIE, cookie)
        .form(&[("params", params.as_str()), ("encSecKey", enc_sec_key.as_str())])
        .send()
        .await
        .map_err(|e| format!("POST {api_path} failed: {e}"))?;

    let text = resp
        .text()
        .await
        .map_err(|e| format!("Failed to read weapi response: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("Failed to parse JSON from {api_path}: {e}"))
}

fn map_artists(raw: &[Value]) -> Vec<Artist> {
    raw.iter()
        .filter_map(|a| {
            let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name.is_empty() {
                None
            } else {
                Some(Artist {
                    id: a
                        .get("id")
                        .and_then(|v| v.as_i64())
                        .map(|n| n.to_string()),
                    mid: a.get("mid").and_then(|v| v.as_str()).map(|s| s.to_string()),
                    name: name.to_string(),
                    pic_url: a
                        .get("picUrl")
                        .or_else(|| a.get("img1v1Url"))
                        .or_else(|| a.get("cover"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    music_size: a.get("musicSize").and_then(|v| v.as_i64()),
                })
            }
        })
        .collect()
}

fn map_song_record(s: &Value) -> Song {
    let artists_raw = s
        .get("ar")
        .or_else(|| s.get("artists"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let artists = map_artists(&artists_raw);

    let album = s.get("al").or_else(|| s.get("album")).unwrap_or(&Value::Null);
    Song {
        provider: "netease".into(),
        id: s.get("id")
            .and_then(|v| v.as_i64())
            .map(|n| n.to_string())
            .or_else(|| s.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .unwrap_or_default(),
        mid: None,
        media_mid: None,
        name: s.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        artist: artists.iter().map(|a| a.name.clone()).collect::<Vec<_>>().join(" / "),
        artists,
        album: album
            .get("name")
            .or_else(|| album.get("title"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        cover: album
            .get("picUrl")
            .or_else(|| album.get("coverUrl"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        duration: s
            .get("dt")
            .or_else(|| s.get("duration"))
            .and_then(|v| v.as_i64())
            .map(|n| n as u64)
            .unwrap_or(0),
        fee: s.get("fee").and_then(|v| v.as_i64()).map(|n| n as i32).unwrap_or(0),
        playable: true,
        language: s.get("language").and_then(|v| v.as_i64()).map(|n| n as i32).unwrap_or(0),
        ..Default::default()
    }
}

/// 搜索歌曲
/// 网易云明文 cloudsearch 接口已强制登录（匿名返回 50000005），
/// 改用 weapi 加密搜索接口 /api/search/get，未登录（空 cookie）亦可搜索。
pub async fn search(keywords: &str, limit: u32, cookie: &str) -> Result<Vec<Song>, String> {
    let client = build_client();

    let mut payload = serde_json::Map::new();
    payload.insert("s".into(), json!(keywords));
    payload.insert("type".into(), json!(1));
    payload.insert("limit".into(), json!(limit));
    payload.insert("offset".into(), json!(0));
    payload.insert("total".into(), json!(true));

    let result: Value = post_weapi(&client, "/api/search/get", payload, cookie).await?;

    let code = result.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    log::info!("[NetEase] weapi search response code={code}");
    if code != 200 {
        return Err(format!("网易云搜索失败: code={code}"));
    }

    let songs = result
        .get("result")
        .and_then(|r| r.get("songs"))
        .and_then(|s| s.as_array())
        .ok_or("No songs in search result")?;

    let mapped: Vec<Song> = songs.iter().map(map_song_record).collect();

    // 补齐缺失封面
    let missing: Vec<String> = mapped
        .iter()
        .filter(|s| s.cover.is_empty())
        .map(|s| s.id.clone())
        .collect();

    if !missing.is_empty() {
        if let Ok(details) = song_detail(&client, &missing, cookie).await {
            let id_to_pic: std::collections::HashMap<String, String> = details
                .iter()
                .map(|s| (s.id.clone(), s.cover.clone()))
                .collect();
            return Ok(mapped
                .iter()
                .map(|s| {
                    if s.cover.is_empty() {
                        Song {
                            cover: id_to_pic.get(&s.id).cloned().unwrap_or_default(),
                            ..s.clone()
                        }
                    } else {
                        s.clone()
                    }
                })
                .collect());
        }
    }

    Ok(mapped)
}

/// 歌曲详情 (批量)
pub async fn song_detail(
    client: &reqwest::Client,
    ids: &[String],
    cookie: &str,
) -> Result<Vec<Song>, String> {
    // 构建 c 参数: [{"id": 123}, {"id": 456}]
    let c_value: Value = Value::Array(
        ids.iter()
            .map(|id| json!({ "id": id.parse::<i64>().unwrap_or(0) }))
            .collect(),
    );
    let c_str = serde_json::to_string(&c_value).unwrap_or_default();

    let result = post_api(client, "/api/v3/song/detail", &[("c", &c_str), ("csrf_token", "")], cookie).await?;

    let songs = result
        .get("songs")
        .and_then(|s| s.as_array())
        .ok_or("No songs in detail result")?;

    Ok(songs.iter().map(map_song_record).collect())
}

/// 获取播放地址 (带音质降级)
pub async fn song_url(id: &str, preferred_quality: &str, cookie: &str) -> Result<SongUrlResult, String> {
    let client = build_client();
    let candidates = quality_candidates_from(preferred_quality);
    let mut trial_fallback: Option<SongUrlResult> = None;

    for q in &candidates {
        let level = q.level.as_str();
        // ids 参数需要 JSON 数组格式: ["12345"]
        let ids_json = format!("[\"{id}\"]");
        let params: [(&str, &str); 4] = [
            ("ids", &ids_json),
            ("level", level),
            ("encodeType", "flac"),
            ("csrf_token", ""),
        ];

        match post_api(&client, "/api/song/enhance/player/url/v1", &params, cookie).await {
            Ok(result) => {
                let data = result
                    .get("data")
                    .and_then(|d| d.as_array())
                    .and_then(|arr| arr.first())
                    .cloned()
                    .unwrap_or(Value::Null);

                let url = data.get("url").and_then(|u| u.as_str()).unwrap_or("");
                let free_trial = data.get("freeTrialInfo").is_some();
                let br = data.get("br").and_then(|b| b.as_u64()).unwrap_or(0);
                let fee = data.get("fee").and_then(|f| f.as_i64()).map(|n| n as i32);

                if !url.is_empty() && !free_trial {
                    return Ok(SongUrlResult {
                        url: Some(url.to_string()),
                        playable: true,
                        trial: false,
                        level: q.level.clone(),
                        quality: q.label.clone(),
                        br,
                        fee,
                        ..Default::default()
                    });
                }

                if !url.is_empty() && free_trial && trial_fallback.is_none() {
                    trial_fallback = Some(SongUrlResult {
                        url: Some(url.to_string()),
                        playable: true,
                        trial: true,
                        level: q.level.clone(),
                        quality: q.label.clone(),
                        br,
                        fee,
                        message: Some("仅试听片段".into()),
                        ..Default::default()
                    });
                }
            }
            Err(_e) => {
                // 继续尝试下一个音质
            }
        }
    }

    if let Some(fallback) = trial_fallback {
        return Ok(fallback);
    }

    // VIP 歌曲无法获取完整 URL：尝试旧版接口（/api/song/enhance/player/url），
    // 旧版接口对未登录用户有时返回标准音质完整 URL（128k），相当于"免费试听"权益。
    // 前端会提示"看广告播放"，用户看完广告后调用此路径获取。
    let ids_json = format!("[\"{id}\"]");
    let params: [(&str, &str); 3] = [
        ("ids", &ids_json),
        ("br", "128000"),
        ("csrf_token", ""),
    ];
    if let Ok(result) = post_api(&client, "/api/song/enhance/player/url", &params, cookie).await {
        let data = result
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|arr| arr.first())
            .cloned()
            .unwrap_or(Value::Null);
        let url = data.get("url").and_then(|u| u.as_str()).unwrap_or("");
        if !url.is_empty() {
            return Ok(SongUrlResult {
                url: Some(url.to_string()),
                playable: true,
                trial: false,
                level: "standard".into(),
                quality: "标准".into(),
                br: 128000,
                fee: data.get("fee").and_then(|f| f.as_i64()).map(|n| n as i32),
                ..Default::default()
            });
        }
    }

    Ok(SongUrlResult {
        playable: false,
        reason: Some("url_unavailable".into()),
        message: Some("无法获取播放地址，可能需要登录或 VIP".into()),
        ..Default::default()
    })
}

/// 二维码登录 - 获取 key
pub async fn login_qr_key(cookie: &str) -> Result<String, String> {
    let client = build_client();
    let mut payload = serde_json::Map::new();
    payload.insert("type".into(), json!(1));
    payload.insert("csrf_token".into(), json!(""));

    let result = post_eapi(&client, "/api/login/qrcode/unikey", payload, cookie).await?;
    result
        .get("unikey")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or("Failed to get QR key".into())
}

/// 二维码登录 - 生成二维码 URL
/// 网易云 EAPI 不返回 qrimg，需要用 key 拼接 URL 在前端生成 QR 码
pub async fn login_qr_create(key: &str, _cookie: &str) -> Result<String, String> {
    // 返回扫码登录 URL，前端用 qrcode 库渲染
    Ok(format!("https://music.163.com/login?codekey={key}"))
}

/// 二维码登录 - 检查状态 (捕获 Set-Cookie)
pub async fn login_qr_check(key: &str, cookie: &str) -> Result<QrCheckResult, String> {
    let client = build_client();
    let mut payload = serde_json::Map::new();
    payload.insert("key".into(), json!(key));
    payload.insert("csrf_token".into(), json!(""));

    let eapi_result = post_eapi_full(&client, "/api/login/qrcode/client/login", payload, cookie).await?;
    let result = &eapi_result.json;

    let code = result
        .get("code")
        .and_then(|v| v.as_i64())
        .map(|n| n as i32)
        .unwrap_or(0);
    let message = result
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let nickname = result
        .get("nickname")
        .or_else(|| result.get("profile").and_then(|p| p.get("nickname")))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let avatar = result
        .get("avatarUrl")
        .or_else(|| result.get("profile").and_then(|p| p.get("avatarUrl")))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // code 803 = 登录成功
    // Cookie 在 Set-Cookie 响应头中，不在 JSON body 中
    let extracted_cookie = if code == 803 {
        if !eapi_result.cookies.is_empty() {
            Some(eapi_result.cookies.clone())
        } else {
            // 尝试从 JSON body 提取 (有些版本会返回)
            result
                .get("cookie")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        }
    } else {
        None
    };

    Ok(QrCheckResult {
        code,
        message,
        cookie: extracted_cookie,
        nickname,
        avatar,
    })
}

/// 获取登录状态和用户信息
/// 参考 Mineradio: 用普通 GET 请求 (带 Cookie), 不用 EAPI 加密
pub async fn login_status(cookie: &str) -> Result<LoginInfo, String> {
    if cookie.is_empty() || !super::cookie::netease_cookie_has_login(cookie) {
        return Ok(LoginInfo {
            provider: "netease".into(),
            ..Default::default()
        });
    }

    let client = build_client();

    // 优先用 /api/w/nuser/account/get (GET, 带 Cookie)
    let url = "https://music.163.com/api/w/nuser/account/get";
    let resp = client
        .get(url)
        .header(USER_AGENT, NETEASE_USER_AGENT)
        .header(REFERER, "https://music.163.com/")
        .header(COOKIE, cookie)
        .send()
        .await
        .map_err(|e| format!("login_status request failed: {e}"))?;

    let text = resp.text().await.map_err(|e| format!("Failed to read response: {e}"))?;
    let result: Value = serde_json::from_str(&text).map_err(|e| format!("Failed to parse JSON: {e}"))?;

    let code = result.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    log::info!("[NetEase] login_status GET response code={code}");

    // 参考 Mineradio: data.profile || body.profile, data.account || body.account
    let data = result.get("data").unwrap_or(&result);
    let profile = data
        .get("profile")
        .or_else(|| result.get("profile"))
        .or_else(|| data.get("account").and_then(|a| a.get("profile")))
        .or_else(|| result.get("account").and_then(|a| a.get("profile")))
        .cloned()
        .unwrap_or(Value::Null);

    if profile.is_null() {
        log::warn!("[NetEase] login_status: profile is null, response: {result}");
        return Ok(LoginInfo {
            provider: "netease".into(),
            logged_in: false,
            ..Default::default()
        });
    }

    // 参考 Mineradio normalizeLoginInfo: userId 必须存在才算登录
    let user_id = profile
        .get("userId")
        .or_else(|| profile.get("user_id"))
        .or_else(|| profile.get("id"))
        .and_then(|v| v.as_i64())
        .map(|n| n.to_string())
        .unwrap_or_default();

    if user_id.is_empty() {
        log::warn!("[NetEase] login_status: no userId in profile, profile: {profile}");
        return Ok(LoginInfo {
            provider: "netease".into(),
            logged_in: false,
            ..Default::default()
        });
    }

    let vip_type = profile
        .get("vipType")
        .and_then(|v| v.as_i64())
        .map(|n| n as i32)
        .unwrap_or(0);
    let is_vip = vip_type >= 1;
    let is_svip = vip_type >= 10;

    let nickname = profile
        .get("nickname")
        .or_else(|| profile.get("userName"))
        .and_then(|v| v.as_str())
        .unwrap_or("网易云用户")
        .to_string();

    log::info!("[NetEase] login_status success: userId={user_id}, nickname={nickname}");

    Ok(LoginInfo {
        provider: "netease".into(),
        logged_in: true,
        user_id,
        nickname,
        avatar: profile
            .get("avatarUrl")
            .or_else(|| profile.get("avatar"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        vip_type,
        vip_level: if is_svip {
            "svip".into()
        } else if is_vip {
            "vip".into()
        } else {
            "none".into()
        },
        is_vip,
        is_svip,
    })
}

/// 获取用户歌单
pub async fn user_playlist(uid: &str, cookie: &str) -> Result<Vec<Playlist>, String> {
    let client = build_client();
    // 用 GET 请求, uid 作为 query 参数
    let limit_str = "100".to_string();
    let offset_str = "0".to_string();
    let result = get_api(&client, "/api/user/playlist", &[
        ("uid", uid),
        ("limit", &limit_str),
        ("offset", &offset_str),
        ("csrf_token", ""),
    ], cookie).await?;

    let code = result.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    let keys = result.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>().join(",")).unwrap_or_default();
    log::info!("[NetEase] user_playlist response code={code}, keys={keys}");

    // 普通 API 可能返回 playlist 数组, 也可能直接是数组
    let playlists = result
        .get("playlist")
        .and_then(|p| p.as_array())
        .or_else(|| result.as_array())
        .ok_or_else(|| format!("No playlist in result, code={code}, keys={keys}"))?;

    Ok(playlists
        .iter()
        .map(|pl| Playlist {
            provider: "netease".into(),
            id: pl.get("id").and_then(|v| v.as_i64()).map(|n| n.to_string()).unwrap_or_default(),
            name: pl.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            cover: pl.get("coverImgUrl").or_else(|| pl.get("picUrl")).and_then(|v| v.as_str()).unwrap_or("").to_string(),
            track_count: pl.get("trackCount").and_then(|v| v.as_u64()).map(|n| n as u32).unwrap_or(0),
            creator: pl
                .get("creator")
                .and_then(|c| c.get("nickname"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            subscribed: pl.get("subscribed").and_then(|v| v.as_bool()).unwrap_or(false),
        })
        .collect())
}

/// 获取歌单元信息（仅元数据，不加载曲目）
pub async fn playlist_detail(id: &str, cookie: &str) -> Result<Playlist, String> {
    let client = build_client();
    let n_str = "0".to_string();
    let s_str = "0".to_string();
    let detail = post_api(&client, "/api/v6/playlist/detail", &[
        ("id", id),
        ("n", &n_str),
        ("s", &s_str),
        ("csrf_token", ""),
    ], cookie).await?;

    let pl = detail.get("playlist").ok_or("No playlist in result")?;
    Ok(Playlist {
        provider: "netease".into(),
        id: pl.get("id").and_then(|v| v.as_i64()).map(|n| n.to_string()).unwrap_or_default(),
        name: pl.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        cover: pl.get("coverImgUrl").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        track_count: pl.get("trackCount").and_then(|v| v.as_u64()).map(|n| n as u32).unwrap_or(0),
        creator: pl.get("creator").and_then(|c| c.get("nickname")).and_then(|v| v.as_str()).unwrap_or("").to_string(),
        ..Default::default()
    })
}

/// 获取歌单元数据 + 全部 trackIds（不加载曲目详情，用于前端分页）
pub async fn playlist_info_with_track_ids(id: &str, cookie: &str) -> Result<(Playlist, Vec<String>), String> {
    let client = build_client();
    let n_str = "1000".to_string();
    let s_str = "0".to_string();
    let detail = post_api(&client, "/api/v6/playlist/detail", &[
        ("id", id),
        ("n", &n_str),
        ("s", &s_str),
        ("csrf_token", ""),
    ], cookie).await?;

    let pl = detail.get("playlist").ok_or("No playlist in result")?;
    let playlist = Playlist {
        provider: "netease".into(),
        id: pl.get("id").and_then(|v| v.as_i64()).map(|n| n.to_string()).unwrap_or_default(),
        name: pl.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        cover: pl.get("coverImgUrl").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        track_count: pl.get("trackCount").and_then(|v| v.as_u64()).map(|n| n as u32).unwrap_or(0),
        creator: pl.get("creator").and_then(|c| c.get("nickname")).and_then(|v| v.as_str()).unwrap_or("").to_string(),
        ..Default::default()
    };

    let track_ids: Vec<String> = pl
        .get("trackIds")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.get("id").and_then(|id| id.as_i64()).map(|n| n.to_string()))
        .collect();

    Ok((playlist, track_ids))
}

/// 获取歌单前 50 首曲目
pub async fn playlist_tracks(id: &str, cookie: &str) -> Result<(Playlist, Vec<Song>), String> {
    let (pl, track_ids) = playlist_info_with_track_ids(id, cookie).await?;
    if track_ids.is_empty() {
        return Ok((pl, vec![]));
    }
    let end = 50.min(track_ids.len());
    let client = build_client();
    let songs = song_detail(&client, &track_ids[..end], cookie).await.unwrap_or_default();
    Ok((pl, songs))
}

/// 加载歌单后继续加更多曲目（指定 range）
pub async fn playlist_tracks_range(id: &str, start: usize, count: usize, cookie: &str) -> Result<Vec<Song>, String> {
    let (_, track_ids) = playlist_info_with_track_ids(id, cookie).await?;
    if track_ids.is_empty() || start >= track_ids.len() {
        return Ok(vec![]);
    }
    let client = build_client();
    let end = (start + count).min(track_ids.len());
    let range_ids: Vec<String> = track_ids[start..end].to_vec();
    let songs = song_detail(&client, &range_ids, cookie).await.unwrap_or_default();
    Ok(songs)
}

/// 获取喜欢列表 — 优先从"我喜欢的音乐"歌单取 trackIds
pub async fn likelist(uid: &str, cookie: &str) -> Result<Vec<String>, String> {
    let client = build_client();

    // 1. 获取用户歌单，找到"我喜欢的音乐"（特殊歌单，id 格式为 uid）
    let limit_str = "200".to_string();
    let offset_str = "0".to_string();
    let pl_result = get_api(&client, "/api/user/playlist", &[
        ("uid", uid),
        ("limit", &limit_str),
        ("offset", &offset_str),
        ("csrf_token", ""),
    ], cookie).await?;

    let playlists = pl_result
        .get("playlist")
        .and_then(|p| p.as_array())
        .ok_or("No playlists in result")?;

    // 找到"我喜欢的音乐"歌单（网易云特殊歌单，specialType=5 或名称匹配）
    let liked_pl = playlists.iter().find(|pl| {
        let special = pl.get("specialType").and_then(|v| v.as_i64()).unwrap_or(0);
        special == 5
    }).or_else(|| {
        playlists.iter().find(|pl| {
            let name = pl.get("name").and_then(|v| v.as_str()).unwrap_or("");
            name == "我喜欢的音乐" || name.contains("喜欢")
        })
    }).or_else(|| {
        // 兜底：用 uid 作为歌单 id
        playlists.iter().find(|pl| {
            pl.get("id").and_then(|v| v.as_i64()).map(|n| n.to_string() == uid).unwrap_or(false)
        })
    });

    let liked_id = if let Some(pl) = liked_pl {
        pl.get("id").and_then(|v| v.as_i64()).map(|n| n.to_string()).unwrap_or_default()
    } else {
        uid.to_string()
    };

    log::info!("[NetEase] liked playlist id: {liked_id}");

    // 2. 获取歌单详情取 trackIds（仅元数据，不加载曲目详情）
    let detail = post_api(&client, "/api/v6/playlist/detail", &[
        ("id", &liked_id),
        ("n", "0"),
        ("s", "0"),
        ("csrf_token", ""),
    ], cookie).await?;

    let pl = detail.get("playlist").ok_or("No playlist in detail")?;
    let ids: Vec<String> = pl
        .get("trackIds")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|v| {
            v.get("id").and_then(|id| id.as_i64()).map(|n| n.to_string())
        })
        .collect();

    log::info!("[NetEase] likelist loaded {} songs from playlist", ids.len());
    Ok(ids)
}

/// 红心/取消红心
pub async fn like(id: &str, like: bool, cookie: &str) -> Result<(), String> {
    let client = build_client();
    let like_str = if like { "true" } else { "false" };
    let result = post_api(&client, "/api/song/like", &[
        ("trackId", id),
        ("like", like_str),
        ("csrf_token", ""),
    ], cookie).await?;
    let code = result.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
    if code == 200 {
        Ok(())
    } else {
        Err(format!("Like failed with code: {code}"))
    }
}

/// 收藏/取消收藏歌单
pub async fn playlist_subscribe(id: &str, subscribe: bool, cookie: &str) -> Result<(), String> {
    let client = build_client();
    let csrf = super::cookie::parse_cookie_string(cookie)
        .get("__csrf")
        .cloned()
        .unwrap_or_default();
    let api = if subscribe { "/api/playlist/subscribe" } else { "/api/playlist/unsubscribe" };
    let result = post_api(&client, api, &[
        ("id", id),
        ("csrf_token", &csrf),
    ], cookie).await?;
    let code = result.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
    if code == 200 {
        Ok(())
    } else {
        Err(format!("Playlist subscribe failed with code: {code}"))
    }
}

/// 获取歌词
pub async fn lyric(id: &str, cookie: &str) -> Result<Lyrics, String> {
    let client = build_client();
    let result = post_api(&client, "/api/song/lyric/v1", &[
        ("id", id),
        ("cp", "false"),
        ("lv", "0"),
        ("kv", "0"),
        ("tv", "0"),
        ("rv", "0"),
        ("yv", "0"),
        ("ytv", "0"),
        ("yrv", "0"),
        ("csrf_token", ""),
    ], cookie).await?;

    let lyric = result
        .get("lrc")
        .and_then(|l| l.get("lyric"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let translation = result
        .get("tlyric")
        .and_then(|l| l.get("lyric"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let roma = result
        .get("romalrc")
        .and_then(|l| l.get("lyric"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let yrc = result
        .get("yrc")
        .and_then(|l| l.get("lyric"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    Ok(Lyrics {
        lyric,
        translation,
        roma,
        yrc,
    })
}

/// 推荐歌单
pub async fn personalized(cookie: &str) -> Result<Vec<Playlist>, String> {
    let client = build_client();
    let limit_str = "30".to_string();
    let result = post_api(&client, "/api/personalized/playlist", &[
        ("limit", &limit_str),
        ("csrf_token", ""),
    ], cookie).await?;

    let playlists = result.get("result").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    Ok(playlists
        .iter()
        .map(|pl| Playlist {
            provider: "netease".into(),
            id: pl.get("id").and_then(|v| v.as_i64()).map(|n| n.to_string()).unwrap_or_default(),
            name: pl.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            cover: pl.get("picUrl").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            track_count: pl.get("trackCount")
                .and_then(|v| v.as_u64().or_else(|| v.as_i64().map(|n| n as u64)).or_else(|| v.as_f64().map(|n| n as u64)))
                .map(|n| n as u32)
                .unwrap_or(0),
            creator: pl.get("creator").and_then(|c| c.get("nickname")).and_then(|v| v.as_str())
                .or_else(|| pl.get("copywriter").and_then(|v| v.as_str()))
                .unwrap_or("").to_string(),
            ..Default::default()
        })
        .collect())
}

/// 每日推荐歌曲
pub async fn recommend_songs(cookie: &str) -> Result<Vec<Song>, String> {
    let client = build_client();
    let result = get_api(&client, "/api/v3/discovery/recommend/songs", &[
        ("csrf_token", ""),
    ], cookie).await?;

    let songs = result
        .get("data")
        .and_then(|d| d.get("dailySongs"))
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(songs.iter().map(map_song_record).collect())
}

/// 每日推荐歌单 (recommend/resource, weapi 加密)
/// 返回当天为用户推荐的歌单列表，需要登录态 cookie
pub async fn recommend_resource(cookie: &str) -> Result<Vec<Playlist>, String> {
    let client = build_client();
    let result = post_weapi(
        &client,
        "/api/v1/discovery/recommend/resource",
        serde_json::Map::new(),
        cookie,
    ).await?;

    let playlists = result.get("recommend").and_then(|v| v.as_array()).cloned()
        .or_else(|| result.get("data").and_then(|v| v.as_array()).cloned())
        .unwrap_or_default();

    Ok(playlists
        .iter()
        .filter_map(|pl| {
            let id = pl.get("id").and_then(|v| v.as_i64())
                .or_else(|| pl.get("resourceId").and_then(|v| v.as_i64()))
                .map(|n| n.to_string())
                .unwrap_or_default();
            if id.is_empty() {
                return None;
            }
            // creator 可能是字符串，也可能是 { nickname } 对象
            let creator = pl.get("creator").map(|c| {
                if let Some(s) = c.as_str() {
                    s.to_string()
                } else if let Some(nick) = c.get("nickname").and_then(|v| v.as_str()) {
                    nick.to_string()
                } else if let Some(copy) = c.get("copywriter").and_then(|v| v.as_str()) {
                    copy.to_string()
                } else {
                    String::new()
                }
            }).unwrap_or_default();
            Some(Playlist {
                provider: "netease".into(),
                id,
                name: pl.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                cover: pl.get("picUrl").and_then(|v| v.as_str())
                    .or_else(|| pl.get("coverImgUrl").and_then(|v| v.as_str()))
                    .unwrap_or("").to_string(),
                track_count: pl.get("trackCount")
                    .and_then(|v| v.as_u64().or_else(|| v.as_i64().map(|n| n as u64)).or_else(|| v.as_f64().map(|n| n as u64)))
                    .map(|n| n as u32)
                    .unwrap_or(0),
                creator,
                ..Default::default()
            })
        })
        .collect())
}

/// 相似歌曲 (心动模式核心, 参考 NeteaseCloudMusicApi simi/song: /weapi/v1/discovery/simiSong)
/// 根据当前歌曲 id 返回口味相似的歌曲, 用于心跳模式动态续播
pub async fn simi_song(id: &str, limit: u32, cookie: &str) -> Result<Vec<Song>, String> {
    let client = build_client();
    let mut payload = serde_json::Map::new();
    payload.insert("songid".into(), json!(id));
    payload.insert("limit".into(), json!(limit));
    payload.insert("offset".into(), json!(0));

    let result = post_weapi(&client, "/api/v1/discovery/simiSong", payload, cookie).await?;
    let code = result.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 200 {
        return Err(format!("simi_song failed: code={code}"));
    }

    let songs = result
        .get("songs")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(songs.iter().map(map_song_record).collect())
}

/// 搜索歌单 (weapi type=1000；明文 cloudsearch 已强制登录，匿名不可用)
pub async fn playlist_search(keywords: &str, limit: u32, cookie: &str) -> Result<Vec<Playlist>, String> {
    let client = build_client();
    let mut payload = serde_json::Map::new();
    payload.insert("s".into(), json!(keywords));
    payload.insert("type".into(), json!(1000));
    payload.insert("limit".into(), json!(limit));
    payload.insert("offset".into(), json!(0));
    payload.insert("total".into(), json!(true));

    let result: Value = post_weapi(&client, "/api/search/get", payload, cookie).await?;

    log::info!("[NetEase] weapi playlist_search response code={}", result.get("code").and_then(|v| v.as_i64()).unwrap_or(-1));

    let playlists_raw = result
        .get("result")
        .and_then(|r| r.get("playlists"))
        .and_then(|a| a.as_array())
        .ok_or("No playlists in search result")?;

    let playlists: Vec<Playlist> = playlists_raw
        .iter()
        .map(|pl| Playlist {
            provider: "netease".into(),
            id: pl.get("id").and_then(|v| v.as_i64()).map(|n| n.to_string()).unwrap_or_default(),
            name: pl.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            cover: pl.get("coverImgUrl").or_else(|| pl.get("picUrl")).and_then(|v| v.as_str()).unwrap_or("").to_string(),
            track_count: pl.get("trackCount")
                .and_then(|v| v.as_u64().or_else(|| v.as_i64().map(|n| n as u64)).or_else(|| v.as_f64().map(|n| n as u64)))
                .map(|n| n as u32)
                .unwrap_or(0),
            creator: pl
                .get("creator")
                .and_then(|c| c.get("nickname"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            ..Default::default()
        })
        .collect();

    Ok(playlists)
}

/// 搜索歌手 (weapi type=100；明文 cloudsearch 已强制登录，匿名不可用)
pub async fn artist_search(keywords: &str, limit: u32, cookie: &str) -> Result<Vec<Artist>, String> {
    let client = build_client();
    let mut payload = serde_json::Map::new();
    payload.insert("s".into(), json!(keywords));
    payload.insert("type".into(), json!(100));
    payload.insert("limit".into(), json!(limit));
    payload.insert("offset".into(), json!(0));
    payload.insert("total".into(), json!(true));

    let result: Value = post_weapi(&client, "/api/search/get", payload, cookie).await?;

    log::info!("[NetEase] weapi artist_search response code={}", result.get("code").and_then(|v| v.as_i64()).unwrap_or(-1));

    let artists_raw = result
        .get("result")
        .and_then(|r| r.get("artists"))
        .and_then(|a| a.as_array())
        .ok_or("No artists in search result")?;

    let artists = map_artists(artists_raw);

    Ok(artists)
}

/// 获取歌手热门歌曲（参考 Mineradio：先用 EAPI artist/songs，失败回退 artist/top/song）
pub async fn artist_songs(
    artist_id: &str,
    limit: u32,
    offset: u32,
    cookie: &str,
) -> Result<Vec<Song>, String> {
    let client = build_client();

    // 主策略：EAPI POST /api/artist/songs
    let mut payload = serde_json::Map::new();
    payload.insert("id".into(), json!(artist_id));
    payload.insert("limit".into(), json!(limit));
    payload.insert("offset".into(), json!(offset));
    payload.insert("order".into(), json!("hot"));

    let result = post_eapi(&client, "/api/artist/songs", payload, cookie).await?;
    log::info!("[NetEase] artist_songs EAPI response code={}", result.get("code").and_then(|v| v.as_i64()).unwrap_or(-1));

    let songs_raw = result
        .get("songs")
        .or_else(|| result.get("data").and_then(|d| d.get("songs")))
        .or_else(|| result.get("hotSongs"))
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();

    // 回退策略：如果 artist/songs 返回空，尝试 artist/top/song
    let songs_raw = if songs_raw.is_empty() {
        log::info!("[NetEase] artist_songs empty, trying artist/top/song fallback");
        let mut fallback_payload = serde_json::Map::new();
        fallback_payload.insert("id".into(), json!(artist_id));

        match post_eapi(&client, "/api/artist/top/song", fallback_payload, cookie).await {
            Ok(fb) => {
                log::info!("[NetEase] artist/top/song response code={}", fb.get("code").and_then(|v| v.as_i64()).unwrap_or(-1));
                fb.get("songs")
                    .or_else(|| fb.get("data").and_then(|d| d.get("songs")))
                    .and_then(|s| s.as_array())
                    .cloned()
                    .unwrap_or_default()
            }
            Err(e) => {
                log::warn!("[NetEase] artist/top/song fallback failed: {e}");
                vec![]
            }
        }
    } else {
        songs_raw
    };

    if songs_raw.is_empty() {
        return Err("No songs in artist result".into());
    }

    let mut mapped: Vec<Song> = songs_raw.iter().map(map_song_record).collect();

    // 歌手歌曲 API 返回的歌曲可能缺少封面，用 song_detail 补齐
    let missing: Vec<String> = mapped
        .iter()
        .filter(|s| s.cover.is_empty())
        .map(|s| s.id.clone())
        .collect();

    if !missing.is_empty() {
        if let Ok(details) = song_detail(&client, &missing, cookie).await {
            let id_to_pic: std::collections::HashMap<String, String> = details
                .iter()
                .map(|s| (s.id.clone(), s.cover.clone()))
                .collect();
            mapped = mapped
                .iter()
                .map(|s| {
                    if s.cover.is_empty() {
                        Song {
                            cover: id_to_pic.get(&s.id).cloned().unwrap_or_default(),
                            ..s.clone()
                        }
                    } else {
                        s.clone()
                    }
                })
                .collect();
        }
    }

    Ok(mapped)
}

// ═══════════════════════════════════════════════════════
//  评论系统 (参考 NeteaseCloudMusicApi)
// ═══════════════════════════════════════════════════════

/// 解析单条评论（含 beReplied 回复拼接）
fn map_comment(c: &Value) -> Comment {
    let user = c.get("user").unwrap_or(&Value::Null);
    let content = c.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let reply = c
        .get("beReplied")
        .and_then(|b| b.as_array())
        .and_then(|arr| arr.first())
        .and_then(|r| r.get("content"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let final_content = if reply.is_empty() { content } else { format!("{content} // 回复: {reply}") };

    Comment {
        comment_id: c.get("commentId").and_then(|v| v.as_i64()).unwrap_or(0),
        content: final_content,
        time: c.get("time").and_then(|v| v.as_i64()).unwrap_or(0),
        liked_count: c.get("likedCount").and_then(|v| v.as_i64()).unwrap_or(0),
        liked: c.get("liked").and_then(|v| v.as_bool()).unwrap_or(false),
        user_id: user.get("userId").and_then(|v| v.as_i64()).unwrap_or(0),
        nickname: user.get("nickname").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        avatar: user.get("avatarUrl").or_else(|| user.get("avatar")).and_then(|v| v.as_str()).unwrap_or("").to_string(),
    }
}

/// 获取歌曲评论（分页，需登录获取完整列表）
pub async fn song_comments(id: &str, page: u32, page_size: u32, cookie: &str) -> Result<CommentPage, String> {
    let client = build_client();
    let offset = page.saturating_sub(1) * page_size;

    let mut payload = serde_json::Map::new();
    payload.insert("rid".into(), json!(id));
    payload.insert("limit".into(), json!(page_size));
    payload.insert("offset".into(), json!(offset));

    let api_path = format!("/api/v1/resource/comments/R_SO_4_{id}");
    let result = post_weapi(&client, &api_path, payload, cookie).await?;

    let code = result.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    log::info!("[NetEase] song_comments id={id} code={code} total={}", result.get("total").and_then(|v| v.as_i64()).unwrap_or(-1));
    if code != 200 {
        return Err(format!("song_comments failed: code={code}, msg={}", result.get("message").and_then(|v| v.as_str()).unwrap_or("")));
    }

    let total = result.get("total").and_then(|v| v.as_i64()).unwrap_or(0);
    let comments: Vec<Comment> = result
        .get("comments")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(map_comment)
        .collect();
    let hot_comments: Vec<Comment> = result
        .get("hotComments")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(map_comment)
        .collect();

    let loaded = comments.len() as i64 + hot_comments.len() as i64;
    let has_more = (offset as i64 + page_size as i64) < total && loaded >= page_size as i64;

    Ok(CommentPage {
        total,
        has_more,
        comments,
        hot_comments,
    })
}

/// 发表评论（需登录）
pub async fn send_comment(id: &str, content: &str, cookie: &str) -> Result<(), String> {
    let client = build_client();
    let mut payload = serde_json::Map::new();
    payload.insert("threadId".into(), json!(format!("R_SO_4_{id}")));
    payload.insert("content".into(), json!(content));

    let result = post_weapi(&client, "/api/resource/comments/add", payload, cookie).await?;
    let code = result.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code == 200 {
        Ok(())
    } else {
        let msg = result.get("message").and_then(|v| v.as_str()).unwrap_or("unknown error");
        Err(format!("send_comment failed: code={code}, msg={msg}"))
    }
}

// ═══════════════════════════════════════════════════════
//  歌手扩展: 简介 / 专辑 / MV / 专辑详情
// ═══════════════════════════════════════════════════════

/// 获取歌手简介 (参考 NeteaseCloudMusicApi artist_desc: /api/artist/introduction)
pub async fn artist_detail(id: &str, cookie: &str) -> Result<ArtistDetail, String> {
    let client = build_client();
    let mut payload = serde_json::Map::new();
    payload.insert("id".into(), json!(id));

    let result = post_weapi(&client, "/api/artist/introduction", payload, cookie).await?;
    let code = result.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 200 {
        return Err(format!("artist_detail failed: code={code}"));
    }

    let artist = result.get("artist").unwrap_or(&Value::Null);
    Ok(ArtistDetail {
        id: id.to_string(),
        name: artist.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        brief_desc: artist
            .get("briefDesc")
            .or_else(|| result.get("briefDesc"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

/// 获取歌手专辑列表 (参考 NeteaseCloudMusicApi artist_album: /api/artist/albums/{id})
pub async fn artist_albums(id: &str, limit: u32, offset: u32, cookie: &str) -> Result<Vec<Album>, String> {
    let client = build_client();
    let mut payload = serde_json::Map::new();
    payload.insert("limit".into(), json!(limit));
    payload.insert("offset".into(), json!(offset));
    payload.insert("total".into(), json!(true));

    let api_path = format!("/api/artist/albums/{id}");
    let result = post_weapi(&client, &api_path, payload, cookie).await?;
    let code = result.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 200 {
        return Err(format!("artist_albums failed: code={code}"));
    }

    let albums_raw = result
        .get("hotAlbums")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(albums_raw
        .iter()
        .map(|a| Album {
            id: a.get("id").and_then(|v| v.as_i64()).map(|n| n.to_string()).unwrap_or_default(),
            name: a.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            cover: a.get("picUrl").or_else(|| a.get("cover")).and_then(|v| v.as_str()).unwrap_or("").to_string(),
            publish_time: a.get("publishTime").and_then(|v| v.as_i64()).unwrap_or(0),
            song_count: a.get("size").and_then(|v| v.as_u64()).map(|n| n as u32).unwrap_or(0),
            artist_name: a
                .get("artist")
                .and_then(|ar| ar.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        })
        .collect())
}

/// 获取歌手 MV 列表 (参考 NeteaseCloudMusicApi artist_mv: /api/artist/mvs)
pub async fn artist_mvs(id: &str, limit: u32, offset: u32, cookie: &str) -> Result<Vec<Mv>, String> {
    let client = build_client();
    let mut payload = serde_json::Map::new();
    payload.insert("artistId".into(), json!(id));
    payload.insert("limit".into(), json!(limit));
    payload.insert("offset".into(), json!(offset));
    payload.insert("total".into(), json!(true));

    let result = post_weapi(&client, "/api/artist/mvs", payload, cookie).await?;
    let code = result.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 200 {
        return Err(format!("artist_mvs failed: code={code}"));
    }

    let mvs_raw = result
        .get("mvs")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(mvs_raw
        .iter()
        .map(|m| Mv {
            id: m.get("id").and_then(|v| v.as_i64()).map(|n| n.to_string()).unwrap_or_default(),
            name: m.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            cover: m.get("coverUrl").or_else(|| m.get("imgurl")).or_else(|| m.get("imgUrl")).and_then(|v| v.as_str()).unwrap_or("").to_string(),
            duration: m.get("duration").and_then(|v| v.as_i64()).map(|n| n as u64).unwrap_or(0),
            play_count: m.get("playCount").and_then(|v| v.as_i64()).unwrap_or(0),
            artist_name: m
                .get("artist")
                .and_then(|ar| ar.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        })
        .collect())
}

/// 获取专辑详情 + 歌曲列表 (参考 NeteaseCloudMusicApi album: /api/v1/album/{id})
pub async fn album_detail(id: &str, cookie: &str) -> Result<(Album, Vec<Song>), String> {
    let client = build_client();
    let api_path = format!("/api/v1/album/{id}");
    let result = post_weapi(&client, &api_path, serde_json::Map::new(), cookie).await?;
    let code = result.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 200 {
        return Err(format!("album_detail failed: code={code}"));
    }

    let album_raw = result.get("album").unwrap_or(&Value::Null);
    let album = Album {
        id: id.to_string(),
        name: album_raw.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        cover: album_raw.get("picUrl").or_else(|| album_raw.get("cover")).and_then(|v| v.as_str()).unwrap_or("").to_string(),
        publish_time: album_raw.get("publishTime").and_then(|v| v.as_i64()).unwrap_or(0),
        song_count: album_raw.get("size").and_then(|v| v.as_u64()).map(|n| n as u32).unwrap_or(0),
        artist_name: album_raw
            .get("artist")
            .and_then(|ar| ar.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    };

    let songs_raw = result
        .get("songs")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let songs: Vec<Song> = songs_raw.iter().map(map_song_record).collect();

    Ok((album, songs))
}

/// 获取 MV 播放地址 (参考 NeteaseCloudMusicApi mv_url: /api/song/enhance/play/mv/url)
pub async fn mv_url(id: &str, resolution: u32, cookie: &str) -> Result<String, String> {
    let client = build_client();
    let mut payload = serde_json::Map::new();
    payload.insert("id".into(), json!(id));
    payload.insert("r".into(), json!(resolution));

    let result = post_weapi(&client, "/api/song/enhance/play/mv/url", payload, cookie).await?;
    let code = result.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 200 {
        return Err(format!("mv_url failed: code={code}"));
    }

    let url = result
        .get("data")
        .and_then(|d| d.get("url"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if url.is_empty() {
        Err("MV 播放地址为空，可能需要登录或该 MV 不可用".into())
    } else {
        Ok(url)
    }
}

