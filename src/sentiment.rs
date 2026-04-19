use anyhow::{Context, Result};
use crate::config::AppConfig;
use crate::models::FearGreedResponse;

pub async fn fetch_fear_greed(config: &AppConfig) -> Result<FearGreedResponse> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/135.0.0.0 Safari/537.36")
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let resp = client
        .get(&config.api.fear_greed_url)
        .header("Accept", "application/json, text/plain, */*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Referer", "https://www.cnn.com/markets/fear-and-greed")
        .send()
        .await
        .context("请求 CNN 恐贪指数 API 失败")?;

    if !resp.status().is_success() {
        let status = resp.status();
        anyhow::bail!("API 请求失败，状态码: {}", status);
    }

    let data: FearGreedResponse = resp
        .json()
        .await
        .context("解析 CNN 恐贪指数响应失败")?;

    Ok(data)
}
