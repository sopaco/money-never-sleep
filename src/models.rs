use chrono::NaiveDate;

#[derive(Debug, Clone)]
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
    pub fn market_value(&self) -> f64 {
        self.current_price.unwrap_or(0.0) * self.shares
    }

    pub fn annualized_return(&self, today: &NaiveDate) -> Option<f64> {
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
        let years = days as f64 / 365.0;
        Some((current / cost).powf(1.0 / years) - 1.0)
    }
}

#[derive(Debug, Clone)]
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
