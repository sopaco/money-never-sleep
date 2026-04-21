//! 恐惧贪婪指数获取模块
//!
//! 使用 finance-query crate 获取数据

use anyhow::{Context, Result};
use finance_query::finance;

/// 获取恐惧贪婪指数
///
/// 数据来源：alternative.me (0-100)
pub async fn fetch_fear_greed_index() -> Result<u8> {
    let fg = finance::fear_and_greed().await
        .context("获取恐惧贪婪指数失败")?;
    Ok(fg.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fetch() {
        let result = fetch_fear_greed_index().await;
        assert!(result.is_ok());
        println!("FGI: {}", result.unwrap());
    }
}
