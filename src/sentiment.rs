//! 恐惧贪婪指数获取模块
//!
//! 使用 CNN API 获取股票市场恐贪指数

use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

/// 默认 CNN API 端点
pub const DEFAULT_CNN_API_URL: &str =
    "https://production.dataviz.cnn.io/index/fearandgreed/graphdata";

/// 请求超时时间（秒）
const REQUEST_TIMEOUT_SECS: u64 = 10;

/// 连接超时时间（秒）
const CONNECT_TIMEOUT_SECS: u64 = 5;

/// 最大重试次数
const MAX_RETRIES: u32 = 3;

/// 重试间隔（毫秒）
const RETRY_DELAY_MS: u64 = 500;

/// CNN Fear & Greed Index API 完整响应结构
#[derive(Debug, Deserialize)]
struct CnnResponse {
    fear_and_greed: FearGreed,
}

/// 当前恐贪指数数据
#[derive(Debug, Deserialize)]
struct FearGreed {
    score: f64,
    rating: String,
}

/// 恐贪指数完整数据（含历史）
#[derive(Debug, Clone)]
pub struct FearGreedData {
    /// 当前指数 (0-100)
    pub score: u8,
    /// CNN 原始评级 (英文，如 "Fear", "Greed")
    pub rating: String,
    /// 前日收盘值
    pub previous_close: Option<f64>,
    /// 一周前值
    pub previous_1_week: Option<f64>,
    /// 一月前值
    pub previous_1_month: Option<f64>,
    /// 一年前值
    pub previous_1_year: Option<f64>,
}

/// 获取恐惧贪婪指数（使用默认 URL）
///
/// 便捷函数，使用默认 CNN API 端点
pub async fn fetch_fear_greed_index() -> Result<u8> {
    fetch_fear_greed_index_with_url(DEFAULT_CNN_API_URL).await
}

/// 获取恐惧贪婪指数（指定 URL）
///
/// 数据来源：CNN Business Fear & Greed Index (股票市场，范围 0-100)
///
/// # Arguments
/// * `url` - CNN API 端点 URL
///
/// # Returns
/// * `Ok(u8)` - 恐贪指数 (0-100)
/// * `Err` - 网络错误或解析错误
pub async fn fetch_fear_greed_index_with_url(url: &str) -> Result<u8> {
    let data = fetch_fear_greed_data(url).await?;
    Ok(data.score)
}

/// 获取完整恐贪指数数据（含历史）
///
/// # Arguments
/// * `url` - CNN API 端点 URL
///
/// # Returns
/// * `Ok(FearGreedData)` - 完整数据结构
/// * `Err` - 网络错误或解析错误
pub async fn fetch_fear_greed_data(url: &str) -> Result<FearGreedData> {
    let client = build_client()?;

    let mut last_error = None;

    // 重试机制
    for attempt in 1..=MAX_RETRIES {
        match try_fetch(&client, url).await {
            Ok(data) => return Ok(data),
            Err(e) => {
                // 最后一次尝试不等待
                if attempt < MAX_RETRIES {
                    tokio::time::sleep(Duration::from_millis(RETRY_DELAY_MS)).await;
                }
                last_error = Some(e);
            }
        }
    }

    // 所有重试都失败
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("未知错误")))
}

/// 尝试获取数据（单次请求）
async fn try_fetch(client: &Client, url: &str) -> Result<FearGreedData> {
    let response = client
        .get(url)
        .header("Accept", "application/json")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Referer", "https://www.cnn.com/markets/fear-and-greed")
        .send()
        .await
        .context("请求 CNN API 失败，请检查网络连接")?;

    let status = response.status();
    if !status.is_success() {
        // 特殊处理反爬虫错误
        if status.as_u16() == 418 {
            anyhow::bail!("CNN API 拒绝请求（反爬虫拦截），请稍后重试或使用代理");
        }
        anyhow::bail!("CNN API 返回错误状态码: {}", status);
    }

    let text = response.text().await.context("读取响应内容失败")?;

    // 解析 JSON，提取历史数据
    parse_cnn_response(&text)
}

/// 构建 HTTP 客户端
fn build_client() -> Result<Client> {
    Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .build()
        .context("创建 HTTP 客户端失败")
}

/// 解析 CNN API 响应
fn parse_cnn_response(text: &str) -> Result<FearGreedData> {
    // 首先解析基本结构
    let cnn_data: CnnResponse = serde_json::from_str(text).context("解析 CNN API 响应失败")?;

    // 提取历史数据（CNN API 可能在根级别或其他位置提供）
    let previous_close = extract_historical_value(text, "previous_close");
    let previous_1_week = extract_historical_value(text, "previous_1_week");
    let previous_1_month = extract_historical_value(text, "previous_1_month");
    let previous_1_year = extract_historical_value(text, "previous_1_year");

    let score = cnn_data.fear_and_greed.score.clamp(0.0, 100.0) as u8;

    Ok(FearGreedData {
        score,
        rating: cnn_data.fear_and_greed.rating,
        previous_close,
        previous_1_week,
        previous_1_month,
        previous_1_year,
    })
}

/// 从 JSON 文本中提取历史值
fn extract_historical_value(json_text: &str, field: &str) -> Option<f64> {
    // 尝试在 JSON 中查找字段
    let pattern = format!("\"{}\":", field);
    if let Some(start) = json_text.find(&pattern) {
        let rest = &json_text[start + pattern.len()..];
        // 跳过空白
        let rest = rest.trim_start();
        // 提取数字
        let mut num_str = String::new();
        for c in rest.chars() {
            if c.is_ascii_digit() || c == '.' || c == '-' {
                num_str.push(c);
            } else if num_str.is_empty() && c.is_whitespace() {
                continue;
            } else {
                break;
            }
        }
        if let Ok(value) = num_str.parse::<f64>() {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fetch() {
        let result = fetch_fear_greed_index().await;
        assert!(result.is_ok());
        let score = result.unwrap();
        assert!(score <= 100);
        println!("CNN FGI: {}", score);
    }

    #[tokio::test]
    async fn test_fetch_full_data() {
        let result = fetch_fear_greed_data(DEFAULT_CNN_API_URL).await;
        assert!(result.is_ok());
        let data = result.unwrap();
        println!("Score: {}", data.score);
        println!("Rating: {}", data.rating);
        println!("Previous close: {:?}", data.previous_close);
        println!("Previous 1 week: {:?}", data.previous_1_week);
        println!("Previous 1 month: {:?}", data.previous_1_month);
        println!("Previous 1 year: {:?}", data.previous_1_year);
    }

    #[test]
    fn test_extract_historical() {
        let json = r#"{"fear_and_greed":{"score":45,"rating":"Fear"},"previous_close":42.5}"#;
        let value = extract_historical_value(json, "previous_close");
        assert_eq!(value, Some(42.5));
    }
}
