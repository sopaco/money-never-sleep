use chrono::NaiveDate;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Position {
    pub id: i64,
    pub asset_code: String,
    pub asset_name: String,
    pub category: String,
    pub shares: f64,
    pub cost_price: f64,
    pub current_price: Option<f64>,
    pub first_buy_date: String,
    pub updated_at: String,
}

impl Position {
    /// 持仓市值，无现价时返回 None
    #[allow(dead_code)]
    pub fn market_value(&self) -> Option<f64> {
        self.current_price.map(|p| p * self.shares)
    }

    /// 持仓市值，无现价时使用成本价估算（用于总资产等不可缺失场景）
    pub fn market_value_or_cost(&self) -> f64 {
        self.current_price
            .map(|p| p * self.shares)
            .unwrap_or(self.cost_price * self.shares)
    }

    /// 年化收益（无最小天数限制），保留作为通用 API
    #[allow(dead_code)]
    pub fn annualized_return(&self, today: &NaiveDate) -> Option<f64> {
        self.annualized_return_with_min_days(today, 0)
    }

    /// 年化收益计算，可指定最小持仓天数门槛
    /// 持仓天数不足门槛时不返回年化值，避免短期收益被放大失真
    /// 正收益：使用复利公式 (current/cost)^(1/years) - 1
    /// 负收益：使用简单年化 (current/cost - 1) / years，避免复利公式对亏损的过度放大
    pub fn annualized_return_with_min_days(&self, today: &NaiveDate, min_days: i64) -> Option<f64> {
        let cost = self.cost_price;
        let current = self.current_price?;
        if cost <= 0.0 || current <= 0.0 {
            return None;
        }
        let first = NaiveDate::parse_from_str(&self.first_buy_date, "%Y-%m-%d").ok()?;
        let days = (*today - first).num_days();
        if days <= 0 {
            return None;
        }
        if days < min_days {
            return None;
        }
        let years = days as f64 / 365.0;
        let ratio = current / cost;
        if ratio >= 1.0 {
            // 正收益：复利公式
            Some(ratio.powf(1.0 / years) - 1.0)
        } else {
            // 负收益：简单年化，避免 (0.95)^12 - 1 ≈ -46% 的失真
            Some((ratio - 1.0) / years)
        }
    }

    /// 绝对收益率 (不考虑时间)
    pub fn absolute_return(&self) -> Option<f64> {
        let cost = self.cost_price;
        let current = self.current_price?;
        if cost <= 0.0 {
            return None;
        }
        Some((current - cost) / cost)
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Transaction {
    pub id: i64,
    pub tx_type: String,
    pub asset_code: String,
    pub shares: f64,
    pub price: f64,
    pub amount: f64,
    pub tx_date: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FearGreedSnapshot {
    pub id: i64,
    pub score: f64,
    pub rating: String,
    pub snapshot_date: String,
    pub previous_close: Option<f64>,
    pub previous_1_week: Option<f64>,
    pub previous_1_month: Option<f64>,
    pub previous_1_year: Option<f64>,
    pub fetched_at: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct FearGreedResponse {
    pub fear_and_greed: FearGreedData,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[allow(dead_code)]
pub struct FearGreedData {
    pub score: f64,
    pub rating: String,
    pub timestamp: String,
    #[serde(default)]
    pub previous_close: Option<f64>,
    #[serde(default)]
    pub previous_1_week: Option<f64>,
    #[serde(default)]
    pub previous_1_month: Option<f64>,
    #[serde(default)]
    pub previous_1_year: Option<f64>,
}
