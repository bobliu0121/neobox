use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Contributor {
    pub name: String,
    pub avatar: String,
    pub role: String,
    #[serde(default)]
    pub bilibili: String,
    #[serde(default)]
    pub douyin: String,
}

async fn fetch_contributors() -> Result<Vec<Contributor>, reqwest::Error> {
    let url = "https://gitee.com/muliuawa/neobox/raw/master/contributors.json";

    // 复用全局连接池（5 秒超时档），避免每次请求重建 Client。
    let client = crate::utils::http::client_with_timeout(5);

    let resp = client.get(url).send().await?;
    let data = resp.json::<Vec<Contributor>>().await?;
    Ok(data)
}

#[tauri::command]
pub async fn get_contributors() -> Result<Vec<Contributor>, String> {
    fetch_contributors().await.map_err(|e| e.to_string())
}
