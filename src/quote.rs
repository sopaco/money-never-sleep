use crate::models::Position;
use anyhow::{Context, Result};

/// 价格更新结果
#[derive(Debug, Clone)]
pub struct PriceUpdate {
    pub asset_code: String,
    pub asset_name: String,
    pub old_price: Option<f64>,
    pub new_price: f64,
    pub source: String,
}

/// 股票报价信息（用于市场分析）
#[derive(Debug, Clone)]
pub struct StockQuote {
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub change: f64,
    pub change_percent: f64,
}

/// 从天天基金获取基金估值价格
async fn fetch_from_tiantian(code: &str) -> Result<Option<f64>> {
    let url = format!("http://fundgz.1234567.com.cn/js/{}.js", code);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("构建 HTTP 客户端失败")?;

    let resp = client
        .get(&url)
        .header("Accept", "*/*")
        .header("Referer", "https://fund.eastmoney.com/")
        .send()
        .await
        .context("请求天天基金 API 失败")?;

    if !resp.status().is_success() {
        return Ok(None);
    }

    let text = resp.text().await.context("读取响应失败")?;

    // 解析 JSONP 格式: jsonpgz({"fundcode":"020972","name":"...","gsz":"1.4395",...});
    if text.contains("jsonpgz(") {
        let start = text.find('(').unwrap_or(0) + 1;
        let end = text.rfind(')').unwrap_or(text.len());
        let json_str = &text[start..end];

        // 使用 serde_json 解析
        let json: serde_json::Value =
            serde_json::from_str(json_str).context("解析天天基金响应失败")?;

        // 优先取估算净值，没有则取单位净值
        if let Some(gsz) = json.get("gsz").and_then(|v| v.as_str()) {
            if let Ok(price) = gsz.parse::<f64>() {
                if price > 0.0 {
                    return Ok(Some(price));
                }
            }
        }

        if let Some(dwjz) = json.get("dwjz").and_then(|v| v.as_str()) {
            if let Ok(price) = dwjz.parse::<f64>() {
                if price > 0.0 {
                    return Ok(Some(price));
                }
            }
        }
    }

    Ok(None)
}

/// 从 Yahoo Finance 获取美股/ETF 价格
async fn fetch_from_yahoo(symbol: &str) -> Result<Option<f64>> {
    let url = format!(
        "https://query1.finance.yahoo.com/v8/finance/chart/{}?interval=1d&range=1d",
        symbol
    );

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("构建 HTTP 客户端失败")?;

    let resp = client
        .get(&url)
        .header("Accept", "application/json, text/plain, */*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Referer", "https://finance.yahoo.com/")
        .send()
        .await
        .context("请求 Yahoo Finance API 失败")?;

    if !resp.status().is_success() {
        return Ok(None);
    }

    let json: serde_json::Value = resp.json().await.context("解析 Yahoo Finance 响应失败")?;

    // 解析 Yahoo Finance v8 API 响应
    if let Some(result) = json.get("chart").and_then(|c| c.get("result"))
        && let Some(first) = result.as_array().and_then(|arr| arr.first())
        && let Some(meta) = first.get("meta")
        && let Some(price) = meta.get("regularMarketPrice").and_then(|v| v.as_f64())
        && price > 0.0
    {
        return Ok(Some(price));
    }

    Ok(None)
}

/// 判断代码类型并获取价格
///
/// 规则:
/// - 纯数字6位: 国内基金，使用天天基金接口
/// - 字母开头: 美股/ETF，使用 Yahoo Finance
/// - 其他: 尝试 Yahoo Finance
pub async fn fetch_price(code: &str, category: &str) -> Result<Option<f64>> {
    // 根据代码特征判断数据源
    let is_chinese_fund = code.len() == 6 && code.chars().all(|c| c.is_ascii_digit());

    if is_chinese_fund || category == "cn_stocks" {
        // 国内基金: 优先天天基金
        if let Some(price) = fetch_from_tiantian(code).await? {
            return Ok(Some(price));
        }
        // 如果天天基金没有数据，可能是 QDII 等特殊基金，尝试 Yahoo
        // QDII 在 Yahoo 可能有对应代码，但通常不完整
        Ok(None)
    } else {
        // 美股/ETF: 使用 Yahoo Finance
        fetch_from_yahoo(code).await
    }
}

/// 批量更新所有持仓的价格
pub async fn update_all_prices(positions: &[Position]) -> Result<Vec<PriceUpdate>> {
    let mut updates = Vec::new();

    for pos in positions {
        match fetch_price(&pos.asset_code, &pos.category).await {
            Ok(Some(new_price)) => {
                updates.push(PriceUpdate {
                    asset_code: pos.asset_code.clone(),
                    asset_name: pos.asset_name.clone(),
                    old_price: pos.current_price,
                    new_price,
                    source: if pos.asset_code.len() == 6
                        && pos.asset_code.chars().all(|c| c.is_ascii_digit())
                    {
                        "天天基金".to_string()
                    } else {
                        "Yahoo Finance".to_string()
                    },
                });
            }
            Ok(None) => {
                // 接口无数据，跳过该资产
                eprintln!(
                    "⚠ 无法获取 {} ({}) 的价格数据，已跳过",
                    pos.asset_code, pos.asset_name
                );
            }
            Err(e) => {
                // 请求失败，跳过该资产，继续处理其他
                eprintln!(
                    "⚠ 获取 {} ({}) 价格失败: {}，已跳过",
                    pos.asset_code, pos.asset_name, e
                );
            }
        }
    }

    Ok(updates)
}

/// 获取单个股票的完整报价信息（用于市场分析）
///
/// # Arguments
/// * `symbol` - 股票代码（如 AAPL, ^GSPC）
///
/// # Returns
/// * `Ok(StockQuote)` - 完整报价信息
/// * `Err` - 获取失败
pub async fn fetch_full_quote(symbol: &str) -> Result<StockQuote> {
    // 使用 Yahoo Finance v8 API 获取完整报价
    let url = format!(
        "https://query1.finance.yahoo.com/v8/finance/chart/{}?interval=1d&range=2d",
        symbol
    );

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("构建 HTTP 客户端失败")?;

    let resp = client
        .get(&url)
        .header("Accept", "application/json, text/plain, */*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Referer", "https://finance.yahoo.com/")
        .send()
        .await
        .context("请求 Yahoo Finance API 失败")?;

    if !resp.status().is_success() {
        anyhow::bail!("Yahoo Finance API 返回 HTTP {}", resp.status());
    }

    let json: serde_json::Value = resp.json().await.context("解析 Yahoo Finance 响应失败")?;

    // 解析响应
    let chart = json.get("chart").context("响应缺少 chart 字段")?;
    let result = chart
        .get("result")
        .and_then(|r| r.as_array())
        .and_then(|arr| arr.first())
        .context("响应缺少 result 数据")?;

    let meta = result.get("meta").context("响应缺少 meta 字段")?;

    let price = meta
        .get("regularMarketPrice")
        .and_then(|v| v.as_f64())
        .filter(|&p| p > 0.0)
        .context("缺少有效的价格数据")?;

    // Yahoo v8 chart API 对指数使用 chartPreviousClose，而非 previousClose
    let previous_close = meta
        .get("chartPreviousClose")
        .and_then(|v| v.as_f64())
        .or_else(|| meta.get("previousClose").and_then(|v| v.as_f64()))
        .or_else(|| {
            // 从 indicators 的历史收盘价中取前一天数据
            result
                .get("indicators")
                .and_then(|i| i.get("quote"))
                .and_then(|q| q.as_array())
                .and_then(|arr| arr.first())
                .and_then(|q| q.get("close"))
                .and_then(|c| c.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.as_f64())
                .filter(|&p| p > 0.0 && p != price)
        })
        .unwrap_or(price);

    let symbol_name = meta
        .get("symbol")
        .and_then(|v| v.as_str())
        .unwrap_or(symbol);

    let name = meta
        .get("shortName")
        .and_then(|v| v.as_str())
        .unwrap_or(symbol_name)
        .to_string();

    let change = price - previous_close;
    let change_percent = if previous_close > 0.0 {
        (change / previous_close) * 100.0
    } else {
        0.0
    };

    Ok(StockQuote {
        symbol: symbol_name.to_string(),
        name,
        price,
        change,
        change_percent,
    })
}
