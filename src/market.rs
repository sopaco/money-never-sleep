// src/market.rs - 市场数据获取模块

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use comfy_table::{Table, Cell, Color};
use crate::quote;

/// 市场报价数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketQuote {
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub change: f64,
    pub change_percent: f64,
}

/// 主要市场指数列表
const MARKET_INDICES: &[(&str, &str)] = &[
    ("^GSPC", "S&P 500"),
    ("^DJI", "Dow Jones"),
    ("^IXIC", "NASDAQ"),
    ("^VIX", "VIX 波动率"),
    ("^FTSE", "FTSE 100"),
    ("^GDAXI", "DAX"),
    ("^N225", "Nikkei 225"),
    ("000001.SS", "上证指数"),
    ("^HSI", "恒生指数"),
];

/// 从 quote::StockQuote 转换为 MarketQuote
impl From<quote::StockQuote> for MarketQuote {
    fn from(q: quote::StockQuote) -> Self {
        MarketQuote {
            symbol: q.symbol,
            name: q.name,
            price: q.price,
            change: q.change,
            change_percent: q.change_percent,
        }
    }
}

/// 获取主要市场指数
pub async fn fetch_market_indices() -> Result<Vec<MarketQuote>> {
    let mut quotes = Vec::new();
    let mut errors: Vec<(&str, String)> = Vec::new();

    for (symbol, default_name) in MARKET_INDICES {
        match quote::fetch_full_quote(symbol).await {
            Ok(q) => {
                let mut quote: MarketQuote = q.into();
                // 使用预定义的中文名称
                quote.name = default_name.to_string();
                quotes.push(quote);
            }
            Err(e) => {
                // 收集错误信息，稍后统一显示
                errors.push((*symbol, e.to_string()));
            }
        }
    }

    // 使用表格显示获取失败的指数
    if !errors.is_empty() {
        let mut error_table = Table::new();
        error_table
            .load_preset(comfy_table::presets::UTF8_FULL)
            .apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS)
            .set_header(vec![
                Cell::new("指数代码").fg(Color::Yellow),
                Cell::new("错误信息").fg(Color::Yellow),
            ]);

        for (symbol, error) in &errors {
            error_table.add_row(vec![
                Cell::new(symbol).fg(Color::Red),
                Cell::new(error).fg(Color::Red),
            ]);
        }

        println!("\n⚠️  部分指数获取失败:\n{}", error_table);
    }

    if quotes.is_empty() {
        bail!("无法获取任何市场指数数据");
    }

    Ok(quotes)
}

/// 获取单个股票报价
pub async fn fetch_stock_quote(symbol: &str) -> Result<MarketQuote> {
    let quote = quote::fetch_full_quote(symbol).await?;
    Ok(quote.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fetch_market_indices() {
        let quotes = fetch_market_indices().await;
        assert!(quotes.is_ok());
        let quotes = quotes.unwrap();
        assert!(!quotes.is_empty());
    }

    #[tokio::test]
    async fn test_fetch_stock_quote() {
        let quote = fetch_stock_quote("AAPL").await;
        assert!(quote.is_ok());
    }
}
