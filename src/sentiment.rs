//! 恐惧贪婪指数获取模块
//!
//! 使用 CNN API 获取股票市场恐贪指数

use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

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
}

/// 恐贪指数完整数据（含历史）
#[derive(Debug, Clone)]
pub struct FearGreedData {
    /// 当前指数 (0-100)
    pub score: u8,
    /// 前日收盘值
    pub previous_close: Option<f64>,
    /// 一周前值
    pub previous_1_week: Option<f64>,
    /// 一月前值
    pub previous_1_month: Option<f64>,
    /// 一年前值
    pub previous_1_year: Option<f64>,
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
        anyhow::bail!(status_error_message(status.as_u16()));
    }

    let text = response.text().await.context("读取响应内容失败")?;

    // 解析 JSON，提取历史数据
    parse_cnn_response(&text)
}

/// 把 HTTP 错误状态码翻译成用户可读的降级提示。
///
/// 抽成独立函数以便单元测试覆盖"网络异常/被反爬拦截时如何降级"这条路径，
/// 不需要真实发起请求或引入 mock HTTP 服务器依赖。
fn status_error_message(status: u16) -> String {
    if status == 418 {
        "CNN API 拒绝请求（反爬虫拦截），请稍后重试或使用代理".to_string()
    } else {
        format!("CNN API 返回错误状态码: {}", status)
    }
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
    // 首先解析基本结构（score 是唯一确定不会变动位置的字段）
    let cnn_data: CnnResponse = serde_json::from_str(text).context("解析 CNN API 响应失败")?;

    // 历史字段（previous_close 等）在 CNN API 里出现的层级不完全固定，
    // 用结构化 JSON 树递归查找，而不是对原始字符串做逐字符扫描——
    // 后者对 "xprevious_close_foo": 这种子串命中、科学计数法、字段顺序变化都不健壮。
    let root: serde_json::Value = serde_json::from_str(text).context("解析 CNN API 响应失败")?;
    let previous_close = find_number_field(&root, "previous_close");
    let previous_1_week = find_number_field(&root, "previous_1_week");
    let previous_1_month = find_number_field(&root, "previous_1_month");
    let previous_1_year = find_number_field(&root, "previous_1_year");

    let score = cnn_data.fear_and_greed.score.clamp(0.0, 100.0) as u8;

    Ok(FearGreedData {
        score,
        previous_close,
        previous_1_week,
        previous_1_month,
        previous_1_year,
    })
}

/// 在任意深度的 JSON 树中递归查找第一个键名等于 `field` 且值为数字的字段。
///
/// CNN API 把这些历史字段放在 `fear_and_greed` 之下还是根级别，观察到会随接口版本变化，
/// 因此不假设固定层级，做一次深度优先搜索；命中即返回，找不到返回 None（而非报错），
/// 因为这些历史字段是展示性的，不应让主流程（获取当前 score）失败。
fn find_number_field(value: &serde_json::Value, field: &str) -> Option<f64> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(n) = map.get(field).and_then(|v| v.as_f64()) {
                return Some(n);
            }
            map.values().find_map(|v| find_number_field(v, field))
        }
        serde_json::Value::Array(items) => items.iter().find_map(|v| find_number_field(v, field)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CNN_API_URL: &str = "https://production.dataviz.cnn.io/index/fearandgreed/graphdata";

    // 需要真实网络，默认跳过：cargo test -- --ignored
    #[tokio::test]
    #[ignore]
    async fn test_fetch_full_data() {
        let result = fetch_fear_greed_data(CNN_API_URL).await;
        assert!(result.is_ok());
        let data = result.unwrap();
        println!("Score: {}", data.score);
        println!("Previous close: {:?}", data.previous_close);
        println!("Previous 1 week: {:?}", data.previous_1_week);
        println!("Previous 1 month: {:?}", data.previous_1_month);
        println!("Previous 1 year: {:?}", data.previous_1_year);
    }

    #[test]
    fn test_find_number_field_root_level() {
        let json = r#"{"fear_and_greed":{"score":45,"rating":"Fear"},"previous_close":42.5}"#;
        let root: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(find_number_field(&root, "previous_close"), Some(42.5));
    }

    #[test]
    fn test_find_number_field_nested_level() {
        // 字段藏在 fear_and_greed 对象内部而不是根级别，也应该能找到
        let json = r#"{"fear_and_greed":{"score":45,"rating":"Fear","previous_close":42.5}}"#;
        let root: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(find_number_field(&root, "previous_close"), Some(42.5));
    }

    #[test]
    fn test_find_number_field_missing_returns_none() {
        let json = r#"{"fear_and_greed":{"score":45,"rating":"Fear"}}"#;
        let root: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(find_number_field(&root, "previous_close"), None);
    }

    #[test]
    fn test_find_number_field_ignores_substring_keys() {
        // 旧的字符串扫描实现会被 "not_previous_close_at_all" 这种子串误命中；
        // 基于 JSON 树的实现按精确键名匹配，不会误判。
        let json = r#"{"fear_and_greed":{"score":45},"not_previous_close_at_all":99.0}"#;
        let root: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(find_number_field(&root, "previous_close"), None);
    }

    #[test]
    fn test_parse_cnn_response_missing_history_still_succeeds() {
        // 历史字段缺失时，主流程（score/rating）仍应正常返回，不能因为展示性字段缺失而报错
        let json = r#"{"fear_and_greed":{"score":45,"rating":"Fear"}}"#;
        let data = parse_cnn_response(json).expect("缺失历史字段不应导致解析失败");
        assert_eq!(data.score, 45);
        assert_eq!(data.previous_close, None);
    }

    #[test]
    fn test_parse_cnn_response_malformed_json_errors() {
        assert!(parse_cnn_response("not json at all").is_err());
    }

    #[test]
    fn test_status_error_message_anti_bot() {
        let msg = status_error_message(418);
        assert!(msg.contains("反爬虫"));
    }

    #[test]
    fn test_status_error_message_generic() {
        let msg = status_error_message(500);
        assert!(msg.contains("500"));
    }
}
