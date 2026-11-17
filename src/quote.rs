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

fn parse_fund_price(value: Option<&serde_json::Value>) -> Option<f64> {
    match value? {
        serde_json::Value::String(text) => text.parse::<f64>().ok().filter(|&p| p > 0.0),
        other => other.as_f64().filter(|&p| p > 0.0),
    }
}

fn json_text(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(text)) => text.trim().to_owned(),
        // 匹配绑定直接拿到那个值，不必再 `value.unwrap()` 一次：后者在
        // 匹配被改动时会变成运行期 panic。
        Some(number @ serde_json::Value::Number(_)) => number.to_string(),
        _ => String::new(),
    }
}

/// 从东方财富移动端 API 取一只基金的最新条目（名称、净值、估值同源）。
async fn fetch_fund_entry(code: &str) -> Result<Option<serde_json::Value>> {
    let url = format!(
        "https://fundmobapi.eastmoney.com/FundMNewApi/FundMNFInfo?pageIndex=1&pageSize=1&plat=Android&appType=ttjj&product=EFund&Version=1&deviceid=1&Fcodes={}",
        code
    );

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (compatible; MNS/1.0)")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("构建 HTTP 客户端失败")?;

    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .header("Referer", "https://fund.eastmoney.com/")
        .send()
        .await
        .context("请求东方财富基金 API 失败")?;

    if !resp.status().is_success() {
        return Ok(None);
    }

    let json: serde_json::Value = resp.json().await.context("解析东方财富响应失败")?;

    if !json.get("Success").and_then(|v| v.as_bool()).unwrap_or(false) {
        return Ok(None);
    }

    Ok(json
        .get("Datas")
        .and_then(|d| d.as_array())
        .and_then(|arr| arr.first())
        .cloned())
}

/// 从东方财富移动端 API 获取基金净值/估值
async fn fetch_from_eastmoney_mobile(code: &str) -> Result<Option<f64>> {
    let Some(fund) = fetch_fund_entry(code).await? else {
        return Ok(None);
    };

    // 优先取估算净值，没有则取单位净值
    if let Some(price) = parse_fund_price(fund.get("GSZ")) {
        return Ok(Some(price));
    }

    Ok(parse_fund_price(fund.get("NAV")))
}

/// 基金搜索建议接口。`key` 是自由文本（代码或名称），必须 URL 编码。
const FUND_SEARCH_URL: &str =
    "https://fundsuggest.eastmoney.com/FundSearch/api/FundSearchAPI.ashx?m=1&key=";

/// 搜索结果里的一条基金候选。
///
/// 这是「添加标的」的输入，不是价格来源：净值只用于让用户确认是不是这只，
/// 成本永远来自用户填写的实际成交价。
#[derive(Debug, Clone, PartialEq)]
pub struct FundCandidate {
    pub code: String,
    pub name: String,
    /// 天天基金的 FUNDTYPE 文案，例如「指数型-股票」「QDII-商品」。
    pub fund_type: String,
    /// 基金公司。
    pub company: String,
    /// 场内（ETF/LOF 等要走证券账户）为 true。
    ///
    /// 只看 `ISBUY`/`MINSG` 是否为空，不看名称或代码前缀：场外联接基金
    /// 名称里同样带 ETF，按名字猜会把能买的标的拦在门外。
    pub exchange_traded: bool,
    /// 展示用单位净值（`DWJZ`）。
    pub nav: Option<f64>,
    /// 建议归属；None = 无法可靠判断，必须由用户自己选。
    pub suggested: Option<&'static str>,
    /// 建议依据或需要确认的原因，直接显示在候选行下方。
    pub hint: &'static str,
}

/// 按名称与 FUNDTYPE 推断这笔基金属于哪条腿。
///
/// 只做「建议 + 依据」，绝不静默改用户的类别选择：猜错的类别会让整条
/// 再平衡链路算错，代价远高于让用户多点一次。
pub fn category_hint(name: &str, fund_type: &str) -> (Option<&'static str>, &'static str) {
    let quoted = fund_type.contains("债券") || fund_type.contains("货币") || fund_type.contains("FOF");

    if quoted {
        return (None, "债券/货币/FOF 不属于这三条腿，请确认后再归。");
    }
    // 黄金股/矿业是股票敞口，不是商品腿。这条必须先于「名称含金」判断。
    if name.contains("黄金股") || name.contains("矿业") || (name.contains("黄金") && name.contains("股票")) {
        return (None, "黄金股/矿业属于股票敞口，不是商品腿。");
    }
    if fund_type.contains("QDII-商品") || (fund_type.contains("指数型-其他") && name.contains("金")) {
        return (Some("counter_cyclical"), "商品类（黄金/原油等）对应逆周期腿。");
    }
    if fund_type.contains("指数型-海外股票") {
        const CROSS_MARKET: [&str; 9] =
            ["全球", "海外", "亚太", "新兴", "恒生", "港股", "欧洲", "日本", "德国"];
        if CROSS_MARKET.iter().any(|word| name.contains(word)) {
            return (None, "跨市场指数，请确认是否归入美股腿。");
        }
        return (Some("us_stocks"), "海外股票指数，通常对应美股腿。");
    }
    if fund_type.contains("QDII") {
        return (None, "QDII 主动基金，跨市场，请确认归属。");
    }
    if fund_type.contains("指数型-股票") {
        const CN_MARKET: [&str; 7] =
            ["沪深", "中证", "上证", "深证", "红利", "科创", "创业"];
        if CN_MARKET.iter().any(|word| name.contains(word)) {
            return (Some("cn_stocks"), "A 股宽基/红利指数，对应 A 股腿。");
        }
        return (None, "股票指数但没识别出市场，请确认归属。");
    }
    if fund_type.contains("股票") || fund_type.contains("混合") {
        return (None, "主动基金，市场与行业不明确，请确认归属。");
    }
    if fund_type.contains("指数") {
        return (None, "其它指数类，请确认跟踪标的与市场。");
    }
    (None, "未能判断归属，请手动确认。")
}

/// 把搜索接口的原始响应解析成候选列表。
///
/// 股票、债券等非基金结果没有 `FundBaseInfo`，直接丢弃——「添加标的」只
/// 接受基金，把股票混进列表只会让用户选到之后刷不出净值。
fn parse_fund_candidates(json: &serde_json::Value) -> Vec<FundCandidate> {
    let mut out = Vec::new();
    let Some(rows) = json.get("Datas").and_then(|value| value.as_array()) else {
        return out;
    };

    for row in rows {
        let Some(base) = row.get("FundBaseInfo").filter(|value| value.is_object()) else {
            continue;
        };
        let code = {
            let code = json_text(base.get("FCODE"));
            if code.is_empty() { json_text(row.get("CODE")) } else { code }
        };
        let name = {
            let name = json_text(base.get("SHORTNAME"));
            if name.is_empty() { json_text(row.get("NAME")) } else { name }
        };
        if code.is_empty() || name.is_empty() {
            continue;
        }

        let fund_type = json_text(base.get("FTYPE"));
        let company = json_text(base.get("JJGS"));
        // ISBUY/MINSG 都为空 = 场内：场外基金两者至少有一个值。
        let exchange_traded =
            json_text(base.get("ISBUY")).is_empty() && json_text(base.get("MINSG")).is_empty();
        let nav = parse_fund_price(base.get("DWJZ"));
        let (suggested, hint) = category_hint(&name, &fund_type);

        out.push(FundCandidate {
            code,
            name,
            fund_type,
            company,
            exchange_traded,
            nav,
            suggested,
            hint,
        });

        // 接口最多返回 10 条，这里再兜一层，避免以后接口放大时列表失控。
        if out.len() >= 10 {
            break;
        }
    }

    out
}

/// 按代码或名称搜索基金。空关键词返回空列表，不打网络请求。
pub async fn search_funds(keyword: &str) -> Result<Vec<FundCandidate>> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (compatible; MNS/1.0)")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("构建 HTTP 客户端失败")?;

    let resp = client
        .get(format!("{FUND_SEARCH_URL}{}", percent_encode(keyword)))
        .header("Accept", "application/json")
        .header("Referer", "https://fund.eastmoney.com/")
        .send()
        .await
        .context("请求基金搜索接口失败")?;

    if !resp.status().is_success() {
        anyhow::bail!("基金搜索接口返回 HTTP {}", resp.status());
    }

    let json: serde_json::Value = resp.json().await.context("解析基金搜索响应失败")?;
    Ok(parse_fund_candidates(&json))
}

/// 只编码查询值需要的字符。关键词是中文或 6 位代码，RFC 3986 的
/// unreserved 集合之外的字节全部转义即可，不需要引入编码库。
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 从天天基金 JSONP API 获取基金估值价格（旧接口，部分网络环境已不可用）
async fn fetch_from_tiantian_jsonp(code: &str) -> Result<Option<f64>> {
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

        let json: serde_json::Value =
            serde_json::from_str(json_str).context("解析天天基金响应失败")?;

        if let Some(price) = parse_fund_price(json.get("gsz")) {
            return Ok(Some(price));
        }

        if let Some(price) = parse_fund_price(json.get("dwjz")) {
            return Ok(Some(price));
        }
    }

    Ok(None)
}

/// 从天天基金获取基金估值价格
async fn fetch_from_tiantian(code: &str) -> Result<Option<f64>> {
    if let Some(price) = fetch_from_eastmoney_mobile(code).await? {
        return Ok(Some(price));
    }
    fetch_from_tiantian_jsonp(code).await
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_keeps_codes_and_escapes_chinese() {
        assert_eq!(percent_encode("007605"), "007605");
        assert_eq!(percent_encode("红利"), "%E7%BA%A2%E5%88%A9");
    }

    /// 五条真实样本（2026-11-16 实测接口返回），锁定建议类别的口径。
    #[test]
    fn category_hint_matches_real_fund_samples() {
        assert_eq!(
            category_hint("嘉实沪深300红利低波动ETF联接A", "指数型-股票").0,
            Some("cn_stocks")
        );
        assert_eq!(
            category_hint("广发纳斯达克100ETF联接人民币(QDII)A", "指数型-海外股票").0,
            Some("us_stocks")
        );
        assert_eq!(
            category_hint("华安黄金ETF联接A", "指数型-其他").0,
            Some("counter_cyclical")
        );
        assert_eq!(category_hint("华夏恒生ETF联接A", "指数型-海外股票").0, None);
        assert_eq!(category_hint("中证黄金产业股票指数A", "指数型-股票").0, None);
    }

    #[test]
    fn category_hint_never_guesses_bonds_or_active_funds() {
        for (name, fund_type) in [
            ("易方达中债新综合债券A", "债券型-长债"),
            ("华夏货币A", "货币型"),
            ("某养老目标FOF", "FOF"),
            ("华夏成长混合", "混合型-灵活"),
            ("摩根欧洲动力策略股票(QDII)A", "QDII-普通股票"),
        ] {
            let (suggested, hint) = category_hint(name, fund_type);
            assert_eq!(suggested, None, "{name} 不该被自动归类");
            assert!(!hint.is_empty(), "{name} 必须给出原因");
        }
    }

    #[test]
    fn parse_fund_candidates_drops_non_funds_and_reads_exchange_flag() {
        let json = serde_json::json!({"Datas": [
            {
                "CODE": "518880",
                "NAME": "黄金ETF华安",
                "FundBaseInfo": {
                    "FCODE": "518880",
                    "SHORTNAME": "黄金ETF华安",
                    "FTYPE": "指数型-其他",
                    "JJGS": "华安基金",
                    "DWJZ": 7.9583,
                    "ISBUY": "",
                    "MINSG": ""
                }
            },
            {
                "CODE": "000216",
                "NAME": "华安黄金ETF联接A",
                "FundBaseInfo": {
                    "FCODE": "000216",
                    "SHORTNAME": "华安黄金ETF联接A",
                    "FTYPE": "指数型-其他",
                    "JJGS": "华安基金",
                    "DWJZ": "2.8944",
                    "ISBUY": "1",
                    "MINSG": 10.0
                }
            },
            { "CODE": "600519", "NAME": "贵州茅台", "FundBaseInfo": null }
        ]});

        let rows = parse_fund_candidates(&json);
        assert_eq!(rows.len(), 2, "股票结果没有 FundBaseInfo，应被丢弃");
        assert!(rows[0].exchange_traded, "ISBUY/MINSG 皆空 = 场内");
        assert!(!rows[1].exchange_traded, "有申购状态 = 场外");
        assert_eq!(rows[0].nav, Some(7.9583));
        assert_eq!(rows[1].nav, Some(2.8944));
        assert_eq!(rows[1].company, "华安基金");
        assert!(rows.iter().all(|row| row.suggested == Some("counter_cyclical")));
    }
}
