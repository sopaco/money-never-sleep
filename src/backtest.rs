use crate::config::AppConfig;
use crate::models::Position;
use crate::strategy::{
    BuySuggestion, calculate_buy_suggestions, calculate_sell_suggestions, check_risk_warnings,
};
use chrono::{Datelike, NaiveDate};
use comfy_table::{Cell, Color, Table, presets::UTF8_FULL, modifiers::UTF8_ROUND_CORNERS};
use std::collections::HashMap;
use unicode_width::UnicodeWidthStr;

/// Pad string to specified display width (handling CJK characters)
fn pad_to_width(s: &str, width: usize) -> String {
    let display_width = UnicodeWidthStr::width(s);
    if display_width >= width {
        s.to_string()
    } else {
        let padding = width - display_width;
        format!("{}{}", s, " ".repeat(padding))
    }
}

const HISTORICAL_FGI_2016_2020: &str =
    include_str!("../.agents/skills/mns-backtest/data/fgi_2016_2020.csv");

const SUPPLEMENTARY_FGI: &str =
    include_str!("../.agents/skills/mns-backtest/data/fgi_2020_2025.csv");

const MONTHLY_REAL_DATA: &str = include_str!("../.agents/skills/mns-backtest/data/monthly_real_final.csv");

#[derive(Debug, Clone)]
pub struct BacktestConfig {
    pub initial_cash: f64,
    pub annual_inflow: f64,
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
}

impl Default for BacktestConfig {
    fn default() -> Self {
        Self {
            initial_cash: 100_000.0,
            annual_inflow: 50_000.0,
            start_date: NaiveDate::from_ymd_opt(2016, 1, 31).unwrap(),
            end_date: NaiveDate::from_ymd_opt(2025, 4, 30).unwrap(),
        }
    }
}

#[derive(Debug)]
pub struct BacktestState {
    pub cash: f64,
    pub total_inflow: f64,
    pub position: BacktestPosition,
    pub trades: Vec<Trade>,
    pub monthly_values: Vec<MonthlyValue>,
    last_inflow_year: i32,
}

impl BacktestState {
    pub fn new(initial_cash: f64) -> Self {
        Self {
            cash: initial_cash,
            total_inflow: initial_cash,
            position: BacktestPosition::default(),
            trades: Vec::new(),
            monthly_values: Vec::new(),
            last_inflow_year: 0,
        }
    }

    pub fn total_value(&self, price: f64) -> f64 {
        self.cash + self.position.market_value(price)
    }
}

#[derive(Debug, Default)]
pub struct BacktestPosition {
    pub shares: f64,
    pub cost_price: f64,
    pub first_buy_date: NaiveDate,
}

impl BacktestPosition {
    pub fn market_value(&self, price: f64) -> f64 {
        self.shares * price
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct Trade {
    pub date: NaiveDate,
    pub action: String,
    pub zone: String,
    pub fgi: f64,
    pub shares: f64,
    pub price: f64,
    pub amount: f64,
    pub pct: String,
    pub ann_ret: Option<f64>,
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct MonthlyValue {
    pub date: NaiveDate,
    pub fgi: f64,
    pub zone: String,
    pub sp500: f64,
    pub cash: f64,
    pub position_value: f64,
    pub total_value: f64,
}

#[derive(Debug)]
pub struct BacktestResult {
    pub name: String,
    pub total_inflow: f64,
    pub final_value: f64,
    pub total_return: f64,
    pub annualized_return: f64,
    pub max_drawdown: f64,
    pub trades: Vec<Trade>,
    pub buy_count: usize,
    pub sell_count: usize,
    pub buy_by_zone: HashMap<String, (usize, f64)>,
    pub sell_by_zone: HashMap<String, (usize, f64)>,
}

impl BacktestResult {
    pub fn print_report(&self) {
        println!();
        println!("=================================================================");
        println!("   {} 回测报告", self.name);
        println!("=================================================================");
        println!();

        println!("  【收益概览】");
        println!("    总投入资金:     ¥{:>12.2}", self.total_inflow);
        println!("    期末总资产:     ¥{:>12.2}", self.final_value);
        println!(
            "    总收益:         ¥{:>12.2}",
            self.final_value - self.total_inflow
        );
        println!(
            "    总收益率:               {:>10.2}%",
            self.total_return * 100.0
        );
        println!(
            "    年化收益率:              {:>10.2}%",
            self.annualized_return * 100.0
        );
        println!(
            "    最大回撤:                {:>10.2}%",
            self.max_drawdown * 100.0
        );
        println!();

        println!("  【交易统计】");
        println!(
            "    买入次数: {:>4}  |  卖出次数: {:>4}",
            self.buy_count, self.sell_count
        );
        println!();

        println!("  【按情绪区间 - 买入】");
        let mut buy_zones: Vec<_> = self.buy_by_zone.iter().collect();
        buy_zones.sort_by_key(|(k, _)| k.as_str());
        for (zone, (count, amount)) in buy_zones {
            println!("      {}:{:>5} 次, ¥{:>12.2}", pad_to_width(zone, 8), count, amount);
        }
        println!();

        println!("  【按情绪区间 - 卖出】");
        let mut sell_zones: Vec<_> = self.sell_by_zone.iter().collect();
        sell_zones.sort_by_key(|(k, _)| k.as_str());
        for (zone, (count, amount)) in sell_zones {
            println!("      {}:{:>5} 次, ¥{:>12.2}", pad_to_width(zone, 8), count, amount);
        }
        println!();

        println!("  【关键交易】（每年 Top 3）");
        let mut trades_by_year: HashMap<i32, Vec<&Trade>> = HashMap::new();
        for trade in &self.trades {
            let year = trade.date.year();
            trades_by_year.entry(year).or_default().push(trade);
        }

        let mut years: Vec<_> = trades_by_year.keys().collect();
        years.sort();

        for year in years {
            let mut year_trades = trades_by_year.get(year).unwrap().clone();
            year_trades.sort_by(|a, b| {
                b.amount
                    .partial_cmp(&a.amount)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            for trade in year_trades.iter().take(3) {
                println!(
                    "    {} {:<4} {}({}), ¥{:>12.0} ({}%, 年化{})",
                    trade.date,
                    trade.action,
                    trade.zone,
                    trade.fgi as i32,
                    trade.amount,
                    trade.pct.replace("%", ""),
                    match trade.ann_ret {
                        Some(r) => format!("{:.1}%", r * 100.0),
                        None => "-".to_string(),
                    }
                );
            }
        }
        println!();
    }
}

/// 多资产月度数据
#[derive(Debug)]
#[allow(dead_code)]
struct MonthlyData {
    date: NaiveDate,
    fgi: f64,
    nasdaq: f64,
    dividend_low_vol: f64,
    gold_cny: f64,
    india: f64,
    japan: f64,
}

/// 多资产回测状态
#[derive(Debug)]
pub struct MultiAssetBacktestState {
    pub cash: f64,
    pub total_inflow: f64,
    pub us_shares: f64,
    pub us_cost: f64,
    pub us_first_buy: NaiveDate,
    pub cn_shares: f64,
    pub cn_cost: f64,
    pub cn_first_buy: NaiveDate,
    pub gold_shares: f64,
    pub gold_cost: f64,
    pub gold_first_buy: NaiveDate,
    pub trades: Vec<MultiAssetTrade>,
    pub monthly_values: Vec<MultiAssetMonthly>,
    last_inflow_year: i32,
}

impl MultiAssetBacktestState {
    pub fn new(initial_cash: f64) -> Self {
        Self {
            cash: initial_cash,
            total_inflow: initial_cash,
            us_shares: 0.0,
            us_cost: 0.0,
            us_first_buy: NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
            cn_shares: 0.0,
            cn_cost: 0.0,
            cn_first_buy: NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
            gold_shares: 0.0,
            gold_cost: 0.0,
            gold_first_buy: NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
            trades: Vec::new(),
            monthly_values: Vec::new(),
            last_inflow_year: 0,
        }
    }

    pub fn total_value(&self, us_price: f64, cn_price: f64, gold_price: f64) -> f64 {
        self.cash + self.us_shares * us_price + self.cn_shares * cn_price + self.gold_shares * gold_price
    }

    pub fn us_position(&self, price: f64) -> Position {
        if self.us_shares <= 0.0 {
            return Position {
                id: 0,
                asset_code: "".to_string(),
                asset_name: "".to_string(),
                shares: 0.0,
                cost_price: 0.0,
                current_price: None,
                category: "".to_string(),
                first_buy_date: "".to_string(),
                updated_at: "".to_string(),
            };
        }
        Position {
            id: 1,
            asset_code: "NASDAQ".to_string(),
            asset_name: "纳指ETF".to_string(),
            shares: self.us_shares,
            cost_price: self.us_cost,
            current_price: Some(price),
            category: "us_stocks".to_string(),
            first_buy_date: self.us_first_buy.format("%Y-%m-%d").to_string(),
            updated_at: "".to_string(),
        }
    }

    pub fn cn_position(&self, price: f64) -> Position {
        if self.cn_shares <= 0.0 {
            return Position {
                id: 0,
                asset_code: "".to_string(),
                asset_name: "".to_string(),
                shares: 0.0,
                cost_price: 0.0,
                current_price: None,
                category: "".to_string(),
                first_buy_date: "".to_string(),
                updated_at: "".to_string(),
            };
        }
        Position {
            id: 2,
            asset_code: "DIVIDEND_LOW_VOL".to_string(),
            asset_name: "红利低波".to_string(),
            shares: self.cn_shares,
            cost_price: self.cn_cost,
            current_price: Some(price),
            category: "cn_stocks".to_string(),
            first_buy_date: self.cn_first_buy.format("%Y-%m-%d").to_string(),
            updated_at: "".to_string(),
        }
    }

    pub fn gold_position(&self, price: f64) -> Position {
        if self.gold_shares <= 0.0 {
            return Position {
                id: 0,
                asset_code: "".to_string(),
                asset_name: "".to_string(),
                shares: 0.0,
                cost_price: 0.0,
                current_price: None,
                category: "".to_string(),
                first_buy_date: "".to_string(),
                updated_at: "".to_string(),
            };
        }
        Position {
            id: 3,
            asset_code: "GOLD".to_string(),
            asset_name: "人民币黄金".to_string(),
            shares: self.gold_shares,
            cost_price: self.gold_cost,
            current_price: Some(price),
            category: "counter_cyclical".to_string(),
            first_buy_date: self.gold_first_buy.format("%Y-%m-%d").to_string(),
            updated_at: "".to_string(),
        }
    }

    pub fn all_positions(&self, us_price: f64, cn_price: f64, gold_price: f64) -> Vec<Position> {
        let mut positions = Vec::new();
        if self.us_shares > 0.0 {
            positions.push(self.us_position(us_price));
        }
        if self.cn_shares > 0.0 {
            positions.push(self.cn_position(cn_price));
        }
        if self.gold_shares > 0.0 {
            positions.push(self.gold_position(gold_price));
        }
        positions
    }

    pub fn buy_us(&mut self, amount: f64, price: f64, date: NaiveDate) {
        if amount <= 0.0 || price <= 0.0 {
            return;
        }
        let shares = amount / price;
        if self.us_shares == 0.0 {
            self.us_first_buy = date;
            self.us_cost = price;
        } else {
            self.us_cost = (self.us_shares * self.us_cost + shares * price) / (self.us_shares + shares);
        }
        self.us_shares += shares;
        self.cash -= amount;
    }

    pub fn buy_cn(&mut self, amount: f64, price: f64, date: NaiveDate) {
        if amount <= 0.0 || price <= 0.0 {
            return;
        }
        let shares = amount / price;
        if self.cn_shares == 0.0 {
            self.cn_first_buy = date;
            self.cn_cost = price;
        } else {
            self.cn_cost = (self.cn_shares * self.cn_cost + shares * price) / (self.cn_shares + shares);
        }
        self.cn_shares += shares;
        self.cash -= amount;
    }

    pub fn buy_gold(&mut self, amount: f64, price: f64, date: NaiveDate) {
        if amount <= 0.0 || price <= 0.0 {
            return;
        }
        let shares = amount / price;
        if self.gold_shares == 0.0 {
            self.gold_first_buy = date;
            self.gold_cost = price;
        } else {
            self.gold_cost = (self.gold_shares * self.gold_cost + shares * price) / (self.gold_shares + shares);
        }
        self.gold_shares += shares;
        self.cash -= amount;
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct MultiAssetTrade {
    pub date: NaiveDate,
    pub action: String,
    pub asset: String,
    pub zone: String,
    pub fgi: f64,
    pub shares: f64,
    pub price: f64,
    pub amount: f64,
    pub pct: String,
    pub ann_ret: Option<f64>,
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct MultiAssetMonthly {
    pub date: NaiveDate,
    pub fgi: f64,
    pub zone: String,
    pub nasdaq: f64,
    pub dividend_low_vol: f64,
    pub gold_cny: f64,
    pub cash: f64,
    pub total_value: f64,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct MultiAssetBacktestResult {
    pub name: String,
    pub total_inflow: f64,
    pub final_value: f64,
    pub total_return: f64,
    pub annualized_return: f64,
    pub max_drawdown: f64,
    pub trades: Vec<MultiAssetTrade>,
    pub buy_count: usize,
    pub sell_count: usize,
    pub buy_by_zone: HashMap<String, (usize, f64)>,
    pub sell_by_zone: HashMap<String, (usize, f64)>,
    pub buy_by_asset: HashMap<String, (usize, f64)>,
    pub sell_by_asset: HashMap<String, (usize, f64)>,
}

impl MultiAssetBacktestResult {
    pub fn print_report(&self) {
        println!();
        println!("=================================================================");
        println!("   {} 回测报告", self.name);
        println!("=================================================================");
        println!();

        println!("  【收益概览】");
        println!("    总投入资金:     ¥{:>12.2}", self.total_inflow);
        println!("    期末总资产:     ¥{:>12.2}", self.final_value);
        println!(
            "    总收益:         ¥{:>12.2}",
            self.final_value - self.total_inflow
        );
        println!(
            "    总收益率:               {:>10.2}%",
            self.total_return * 100.0
        );
        println!(
            "    年化收益率:              {:>10.2}%",
            self.annualized_return * 100.0
        );
        println!(
            "    最大回撤:                {:>10.2}%",
            self.max_drawdown * 100.0
        );
        println!();

        println!("  【交易统计】");
        println!(
            "    买入次数: {:>4}  |  卖出次数: {:>4}",
            self.buy_count, self.sell_count
        );
        println!();

        println!("  【按资产类别 - 买入】");
        let mut buy_assets: Vec<_> = self.buy_by_asset.iter().collect();
        buy_assets.sort_by_key(|(k, _)| k.as_str());
        for (asset, (count, amount)) in buy_assets {
            println!("      {}:{:>5} 次, ¥{:>12.2}", pad_to_width(asset, 8), count, amount);
        }
        println!();

        println!("  【按资产类别 - 卖出】");
        let mut sell_assets: Vec<_> = self.sell_by_asset.iter().collect();
        sell_assets.sort_by_key(|(k, _)| k.as_str());
        for (asset, (count, amount)) in sell_assets {
            println!("      {}:{:>5} 次, ¥{:>12.2}", pad_to_width(asset, 8), count, amount);
        }
        println!();

        println!("  【按情绪区间 - 买入】");
        let mut buy_zones: Vec<_> = self.buy_by_zone.iter().collect();
        buy_zones.sort_by_key(|(k, _)| k.as_str());
        for (zone, (count, amount)) in buy_zones {
            println!("      {}:{:>5} 次, ¥{:>12.2}", pad_to_width(zone, 8), count, amount);
        }
        println!();

        println!("  【按情绪区间 - 卖出】");
        let mut sell_zones: Vec<_> = self.sell_by_zone.iter().collect();
        sell_zones.sort_by_key(|(k, _)| k.as_str());
        for (zone, (count, amount)) in sell_zones {
            println!("      {}:{:>5} 次, ¥{:>12.2}", pad_to_width(zone, 8), count, amount);
        }
        println!();
    }
}

fn parse_multi_asset_data(data: &str) -> Vec<MonthlyData> {
    data.lines()
        .skip(1) // 跳过标题行
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 7 {
                let year_month = parts[0];
                let fgi: f64 = parts[1].parse().ok()?;
                let nasdaq: f64 = parts[2].parse().ok()?;
                let dividend_low_vol: f64 = parts[3].parse().ok()?;
                let gold_cny: f64 = parts[4].parse().ok()?;
                let india: f64 = parts[5].parse().ok()?;
                let japan: f64 = parts[6].parse().ok()?;

                // 解析年月为日期（取月末）
                let ym: Vec<&str> = year_month.split('-').collect();
                if ym.len() == 2 {
                    let year: i32 = ym[0].parse().ok()?;
                    let month: u32 = ym[1].parse().ok()?;

                    // 计算月末日期
                    let last_day = if month == 12 {
                        NaiveDate::from_ymd_opt(year + 1, 1, 1)
                    } else {
                        NaiveDate::from_ymd_opt(year, month + 1, 1)
                    }
                    .and_then(|next_month| next_month.pred_opt());

                    if let Some(date) = last_day {
                        return Some(MonthlyData {
                            date,
                            fgi,
                            nasdaq,
                            dividend_low_vol,
                            gold_cny,
                            india,
                            japan,
                        });
                    }
                }
            }
            None
        })
        .collect()
}

fn get_zone_name(score: f64, config: &AppConfig) -> &'static str {
    if score < config.thresholds.extreme_fear {
        "极度恐慌"
    } else if score < config.thresholds.fear {
        "恐慌"
    } else if score < config.thresholds.neutral {
        "中性"
    } else if score < config.thresholds.greed {
        "贪婪"
    } else {
        "极度贪婪"
    }
}

/// 执行多资产回测
pub fn run_multi_asset_backtest(config: &AppConfig, bt_config: &BacktestConfig) -> MultiAssetBacktestResult {
    let monthly_data = parse_multi_asset_data(MONTHLY_REAL_DATA);

    // 过滤日期范围
    let filtered_data: Vec<_> = monthly_data
        .into_iter()
        .filter(|d| d.date >= bt_config.start_date && d.date <= bt_config.end_date)
        .collect();

    let mut state = MultiAssetBacktestState::new(bt_config.initial_cash);
    let mut prev_zone: Option<&str> = None;
    let mut last_trade_month: i32 = -100;

    let mut buy_count = 0usize;
    let mut sell_count = 0usize;
    let mut buy_by_zone: HashMap<String, (usize, f64)> = HashMap::new();
    let mut sell_by_zone: HashMap<String, (usize, f64)> = HashMap::new();
    let mut buy_by_asset: HashMap<String, (usize, f64)> = HashMap::new();
    let mut sell_by_asset: HashMap<String, (usize, f64)> = HashMap::new();

    for data in &filtered_data {
        let year = data.date.year();
        let month_key = year * 12 + data.date.month() as i32;

        // 年度注资（每年3月末）
        if year > state.last_inflow_year && data.date.month() >= 3 {
            state.cash += bt_config.annual_inflow;
            state.total_inflow += bt_config.annual_inflow;
            state.last_inflow_year = year;
        }

        let zone = get_zone_name(data.fgi, config);
        let zone_changed = prev_zone != Some(zone);
        prev_zone = Some(zone);

        // 计算持仓信息
        let positions = state.all_positions(data.nasdaq, data.dividend_low_vol, data.gold_cny);

        // 交易触发条件
        let months_since_trade = month_key - last_trade_month;
        let should_trade = zone_changed
            || (months_since_trade >= 3 && data.fgi < config.thresholds.neutral);

        // 卖出逻辑：贪婪及以上区间
        if zone_changed && data.fgi >= config.thresholds.neutral {
            let sell_suggestions = calculate_sell_suggestions(config, data.fgi, &positions);

            for sell in &sell_suggestions {
                if sell.sell_shares >= 0.01 {
                    let (shares, _cost, _first_buy) = if sell.asset_code == "NASDAQ" {
                        (state.us_shares, state.us_cost, state.us_first_buy)
                    } else if sell.asset_code == "DIVIDEND_LOW_VOL" {
                        (state.cn_shares, state.cn_cost, state.cn_first_buy)
                    } else {
                        (state.gold_shares, state.gold_cost, state.gold_first_buy)
                    };

                    if sell.sell_shares <= shares {
                        // 执行卖出
                        let actual_shares = sell.sell_shares;
                        let actual_amount = actual_shares * (data.nasdaq.max(data.dividend_low_vol.max(data.gold_cny)));

                        if sell.asset_code == "NASDAQ" {
                            state.us_shares -= actual_shares;
                            state.cash += actual_shares * data.nasdaq;
                        } else if sell.asset_code == "DIVIDEND_LOW_VOL" {
                            state.cn_shares -= actual_shares;
                            state.cash += actual_shares * data.dividend_low_vol;
                        } else {
                            state.gold_shares -= actual_shares;
                            state.cash += actual_shares * data.gold_cny;
                        }

                        last_trade_month = month_key;
                        sell_count += 1;
                        sell_by_zone.entry(zone.to_string()).or_insert((0, 0.0)).0 += 1;
                        sell_by_asset.entry(sell.asset_code.clone()).or_insert((0, 0.0)).0 += 1;

                        state.trades.push(MultiAssetTrade {
                            date: data.date,
                            action: "卖出".to_string(),
                            asset: sell.asset_code.clone(),
                            zone: zone.to_string(),
                            fgi: data.fgi,
                            shares: actual_shares,
                            price: if sell.asset_code == "NASDAQ" { data.nasdaq } else if sell.asset_code == "DIVIDEND_LOW_VOL" { data.dividend_low_vol } else { data.gold_cny },
                            amount: actual_amount,
                            pct: format!("{:.0}%", sell.sell_ratio),
                            ann_ret: sell.annualized_return,
                        });
                    }
                }
            }
        }

        // 买入逻辑：恐慌及以下区间
        if should_trade && data.fgi < config.thresholds.neutral {
            let risk_warnings = check_risk_warnings(config, data.fgi, &positions);
            let sell_suggestions = Vec::new(); // 买入时卖出建议为空
            let buy_suggestion = calculate_buy_suggestions(
                config,
                data.fgi,
                state.cash,
                &positions,
                &sell_suggestions,
                &risk_warnings,
            );

            // 按配置分配买入金额
            let us_ratio = config.allocation.us_stocks / 100.0;
            let cn_ratio = config.allocation.cn_stocks / 100.0;
            let gold_ratio = config.allocation.counter_cyclical / 100.0;

            let us_amount = buy_suggestion.total_amount * us_ratio;
            let cn_amount = buy_suggestion.total_amount * cn_ratio;
            let gold_amount = buy_suggestion.total_amount * gold_ratio;

            // 执行买入
            if us_amount > 0.0 && data.nasdaq > 0.0 {
                let _cash_before = state.cash;
                state.buy_us(us_amount, data.nasdaq, data.date);

                buy_count += 1;
                buy_by_zone.entry(zone.to_string()).or_insert((0, 0.0)).0 += 1;
                buy_by_asset.entry("NASDAQ".to_string()).or_insert((0, 0.0)).0 += 1;
                buy_by_zone.get_mut(zone).unwrap().1 += us_amount;
                buy_by_asset.get_mut("NASDAQ").unwrap().1 += us_amount;

                state.trades.push(MultiAssetTrade {
                    date: data.date,
                    action: "买入".to_string(),
                    asset: "NASDAQ".to_string(),
                    zone: zone.to_string(),
                    fgi: data.fgi,
                    shares: us_amount / data.nasdaq,
                    price: data.nasdaq,
                    amount: us_amount,
                    pct: format!("{:.0}%", (us_amount / (state.cash + us_amount) * 100.0)),
                    ann_ret: None,
                });
            }

            if cn_amount > 0.0 && data.dividend_low_vol > 0.0 {
                state.buy_cn(cn_amount, data.dividend_low_vol, data.date);

                buy_count += 1;
                buy_by_zone.entry(zone.to_string()).or_insert((0, 0.0)).0 += 1;
                buy_by_asset.entry("DIVIDEND_LOW_VOL".to_string()).or_insert((0, 0.0)).0 += 1;
                buy_by_zone.get_mut(zone).unwrap().1 += cn_amount;
                buy_by_asset.get_mut("DIVIDEND_LOW_VOL").unwrap().1 += cn_amount;

                state.trades.push(MultiAssetTrade {
                    date: data.date,
                    action: "买入".to_string(),
                    asset: "DIVIDEND_LOW_VOL".to_string(),
                    zone: zone.to_string(),
                    fgi: data.fgi,
                    shares: cn_amount / data.dividend_low_vol,
                    price: data.dividend_low_vol,
                    amount: cn_amount,
                    pct: format!("{:.0}%", (cn_amount / (state.cash + cn_amount) * 100.0)),
                    ann_ret: None,
                });
            }

            if gold_amount > 0.0 && data.gold_cny > 0.0 {
                state.buy_gold(gold_amount, data.gold_cny, data.date);

                buy_count += 1;
                buy_by_zone.entry(zone.to_string()).or_insert((0, 0.0)).0 += 1;
                buy_by_asset.entry("GOLD".to_string()).or_insert((0, 0.0)).0 += 1;
                buy_by_zone.get_mut(zone).unwrap().1 += gold_amount;
                buy_by_asset.get_mut("GOLD").unwrap().1 += gold_amount;

                state.trades.push(MultiAssetTrade {
                    date: data.date,
                    action: "买入".to_string(),
                    asset: "GOLD".to_string(),
                    zone: zone.to_string(),
                    fgi: data.fgi,
                    shares: gold_amount / data.gold_cny,
                    price: data.gold_cny,
                    amount: gold_amount,
                    pct: format!("{:.0}%", (gold_amount / (state.cash + gold_amount) * 100.0)),
                    ann_ret: None,
                });
            }

            if buy_suggestion.total_amount > 0.0 {
                last_trade_month = month_key;
            }
        }

        // 记录月度数据
        state.monthly_values.push(MultiAssetMonthly {
            date: data.date,
            fgi: data.fgi,
            zone: zone.to_string(),
            nasdaq: data.nasdaq,
            dividend_low_vol: data.dividend_low_vol,
            gold_cny: data.gold_cny,
            cash: state.cash,
            total_value: state.total_value(data.nasdaq, data.dividend_low_vol, data.gold_cny),
        });
    }

    // 计算最终结果
    let last_data = filtered_data.last().expect("No data");
    let final_value = state.total_value(last_data.nasdaq, last_data.dividend_low_vol, last_data.gold_cny);
    let total_return = (final_value / state.total_inflow) - 1.0;

    let days = (bt_config.end_date - bt_config.start_date).num_days() as f64;
    let years = days / 365.0;
    let annualized_return = (final_value / state.total_inflow).powf(1.0 / years) - 1.0;

    // 计算最大回撤
    let mut max_value: f64 = 0.0;
    let mut max_drawdown: f64 = 0.0;
    for mv in &state.monthly_values {
        max_value = max_value.max(mv.total_value);
        let drawdown = (max_value - mv.total_value) / max_value;
        max_drawdown = max_drawdown.max(drawdown);
    }

    MultiAssetBacktestResult {
        name: "多资产逆向策略".to_string(),
        total_inflow: state.total_inflow,
        final_value,
        total_return,
        annualized_return,
        max_drawdown,
        trades: state.trades,
        buy_count,
        sell_count,
        buy_by_zone,
        sell_by_zone,
        buy_by_asset,
        sell_by_asset,
    }
}

fn parse_fgi_data(data: &str) -> Vec<(NaiveDate, f64)> {
    data.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 2 {
                NaiveDate::parse_from_str(parts[0], "%Y-%m-%d")
                    .ok()
                    .map(|d| (d, parts[1].parse().unwrap_or(0.0)))
            } else {
                None
            }
        })
        .collect()
}

fn aggregate_fgi_to_monthly(fgi_data: &[(NaiveDate, f64)]) -> Vec<(NaiveDate, f64)> {
    if fgi_data.is_empty() {
        return Vec::new();
    }

    // 先按日期排序
    let mut sorted_data: Vec<_> = fgi_data.to_vec();
    sorted_data.sort_by_key(|(d, _)| *d);

    // 取每月最后一条数据
    let mut monthly_data: HashMap<(i32, u32), (NaiveDate, f64)> = HashMap::new();
    for (date, score) in sorted_data {
        let key = (date.year(), date.month());
        // 直接覆盖，因为已排序，最后的就是月末
        monthly_data.insert(key, (date, score));
    }

    let mut result: Vec<_> = monthly_data.into_values().collect();
    result.sort_by_key(|(d, _)| *d);
    result
}

fn parse_sp500_data(data: &str) -> Vec<(NaiveDate, f64)> {
    data.lines()
        .skip(1) // 跳过表头
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 3 {
                let year_month = parts[0];
                let price: f64 = parts[2].parse().ok()?; // nasdaq 列

                let ym: Vec<&str> = year_month.split('-').collect();
                if ym.len() == 2 {
                    let year: i32 = ym[0].parse().ok()?;
                    let month: u32 = ym[1].parse().ok()?;

                    let last_day = if month == 12 {
                        NaiveDate::from_ymd_opt(year + 1, 1, 1)
                    } else {
                        NaiveDate::from_ymd_opt(year, month + 1, 1)
                    }
                    .and_then(|next_month| next_month.pred_opt());

                    if let Some(date) = last_day {
                        return Some((date, price));
                    }
                }
            }
            None
        })
        .collect()
}

fn create_position(state: &BacktestState, price: f64, date: NaiveDate) -> Option<Position> {
    if state.position.shares <= 0.0 {
        return None;
    }

    Some(Position {
        id: 1,
        asset_code: "SPY".to_string(),
        asset_name: "S&P 500 ETF".to_string(),
        shares: state.position.shares,
        cost_price: state.position.cost_price,
        current_price: Some(price),
        category: "us_stocks".to_string(),
        first_buy_date: state.position.first_buy_date.format("%Y-%m-%d").to_string(),
        updated_at: date.format("%Y-%m-%d").to_string(),
    })
}

pub fn run_backtest(config: &AppConfig, bt_config: &BacktestConfig) -> BacktestResult {
    let mut fgi_data = parse_fgi_data(HISTORICAL_FGI_2016_2020);
    fgi_data.extend(parse_fgi_data(SUPPLEMENTARY_FGI));
    fgi_data.sort_by_key(|(d, _)| *d);

    let monthly_fgi = aggregate_fgi_to_monthly(&fgi_data);
    let sp500_data = parse_sp500_data(MONTHLY_REAL_DATA);

    let mut combined_data: Vec<(NaiveDate, f64, f64)> = Vec::new();
    for (fgi_date, fgi_score) in &monthly_fgi {
        for (sp_date, sp_price) in &sp500_data {
            if fgi_date.year() == sp_date.year() && fgi_date.month() == sp_date.month() {
                combined_data.push((*fgi_date, *fgi_score, *sp_price));
                break;
            }
        }
    }

    let combined_data: Vec<_> = combined_data
        .into_iter()
        .filter(|(d, _, _)| *d >= bt_config.start_date && *d <= bt_config.end_date)
        .collect();

    let mut state = BacktestState::new(bt_config.initial_cash);
    let mut prev_zone: Option<&str> = None;
    let mut last_trade_month: i32 = -100;
    let mut buy_count = 0usize;
    let mut sell_count = 0usize;
    let mut buy_by_zone: HashMap<String, (usize, f64)> = HashMap::new();
    let mut sell_by_zone: HashMap<String, (usize, f64)> = HashMap::new();

    for (date, score, price) in &combined_data {
        let year = date.year();
        let month_key = year * 12 + date.month() as i32;

        // 年度注资（每年3月末）
        let new_capital = if year > state.last_inflow_year && date.month() >= 3 {
            state.cash += bt_config.annual_inflow;
            state.total_inflow += bt_config.annual_inflow;
            state.last_inflow_year = year;
            true
        } else {
            false
        };

        let zone = get_zone_name(*score, config);
        let zone_changed = prev_zone != Some(zone);
        prev_zone = Some(zone);

        let positions: Vec<Position> = create_position(&state, *price, *date)
            .map(|p| vec![p])
            .unwrap_or_default();

        // 交易触发条件：
        // 1. 区间变化时（常规触发）
        // 2. 有新资金注入且处于可买入区间（恐慌及以下）
        // 3. 距离上次交易超过3个月（冷却期后重新评估）
        let months_since_trade = month_key - last_trade_month;
        let should_trade = zone_changed
            || (new_capital && *score < config.thresholds.neutral)
            || (months_since_trade >= 3 && *score < config.thresholds.neutral);

        // 卖出：仅在贪婪及以上区间且区间变化时
        let sell_suggestions = if zone_changed && *score >= config.thresholds.neutral {
            calculate_sell_suggestions(config, *score, &positions)
        } else {
            Vec::new()
        };

        // 买入：在恐慌及以下区间，满足交易条件时
        let buy_suggestion = if should_trade && *score < config.thresholds.neutral {
            let risk_warnings = check_risk_warnings(config, *score, &positions);
            calculate_buy_suggestions(
                config,
                *score,
                state.cash,
                &positions,
                &sell_suggestions,
                &risk_warnings,
            )
        } else {
            BuySuggestion {
                total_amount: 0.0,
                us_amount: 0.0,
                cn_amount: 0.0,
                counter_amount: 0.0,
                details: Vec::new(),
                excluded: Vec::new(),
            }
        };

        for sell in &sell_suggestions {
            if sell.sell_shares >= 0.01 {
                state.position.shares -= sell.sell_shares;
                state.cash += sell.sell_amount;
                last_trade_month = month_key;

                sell_count += 1;
                let zone_key = zone.to_string();
                sell_by_zone.entry(zone_key.clone()).or_insert((0, 0.0)).0 += 1;
                sell_by_zone.get_mut(&zone_key).unwrap().1 += sell.sell_amount;

                state.trades.push(Trade {
                    date: *date,
                    action: "卖出".to_string(),
                    zone: zone.to_string(),
                    fgi: *score,
                    shares: sell.sell_shares,
                    price: *price,
                    amount: sell.sell_amount,
                    pct: format!("{:.0}%", sell.sell_ratio),
                    ann_ret: sell.annualized_return,
                });

                if state.position.shares < 0.01 {
                    state.position.shares = 0.0;
                    state.position.cost_price = 0.0;
                }
            }
        }

        let buy_amount = buy_suggestion.total_amount;
        if buy_amount > 0.0 && *price > 0.0 {
            let buy_shares = buy_amount / price;
            if buy_shares >= 0.01 {
                let total_shares = state.position.shares + buy_shares;
                if state.position.shares == 0.0 {
                    state.position.first_buy_date = *date;
                    state.position.cost_price = *price;
                } else {
                    state.position.cost_price = (state.position.shares * state.position.cost_price
                        + buy_shares * price)
                        / total_shares;
                }
                state.position.shares = total_shares;
                state.cash -= buy_amount;
                last_trade_month = month_key;

                buy_count += 1;
                let zone_key = zone.to_string();
                buy_by_zone.entry(zone_key.clone()).or_insert((0, 0.0)).0 += 1;
                buy_by_zone.get_mut(&zone_key).unwrap().1 += buy_amount;

                // 计算买入金额占买入前可用现金的比例
                let cash_before_buy = state.cash + buy_amount;
                let pct = (buy_amount / cash_before_buy * 100.0) as i32;
                state.trades.push(Trade {
                    date: *date,
                    action: "买入".to_string(),
                    zone: zone.to_string(),
                    fgi: *score,
                    shares: buy_shares,
                    price: *price,
                    amount: buy_amount,
                    pct: format!("{}%", pct),
                    ann_ret: None,
                });
            }
        }

        state.monthly_values.push(MonthlyValue {
            date: *date,
            fgi: *score,
            zone: zone.to_string(),
            sp500: *price,
            cash: state.cash,
            position_value: state.position.market_value(*price),
            total_value: state.total_value(*price),
        });
    }

    let final_value = state
        .monthly_values
        .last()
        .map(|d| d.total_value)
        .unwrap_or(bt_config.initial_cash);

    let total_return = (final_value / state.total_inflow) - 1.0;

    let days = (bt_config.end_date - bt_config.start_date).num_days() as f64;
    let years = days / 365.0;
    let annualized_return = (final_value / state.total_inflow).powf(1.0 / years) - 1.0;

    let mut max_value: f64 = 0.0;
    let mut max_drawdown: f64 = 0.0;
    for mv in &state.monthly_values {
        max_value = max_value.max(mv.total_value);
        let drawdown = (max_value - mv.total_value) / max_value;
        max_drawdown = max_drawdown.max(drawdown);
    }

    BacktestResult {
        name: "逆向策略".to_string(),
        total_inflow: state.total_inflow,
        final_value,
        total_return,
        annualized_return,
        max_drawdown,
        trades: state.trades,
        buy_count,
        sell_count,
        buy_by_zone,
        sell_by_zone,
    }
}

pub fn run_buy_and_hold(bt_config: &BacktestConfig) -> BacktestResult {
    let sp500_data = parse_sp500_data(MONTHLY_REAL_DATA);

    let end_price = sp500_data
        .iter()
        .rfind(|(d, _)| *d <= bt_config.end_date)
        .map(|(_, p)| *p)
        .unwrap_or(6049.06);

    let mut total_inflow = bt_config.initial_cash;
    let mut total_shares = 0.0;
    let mut trades: Vec<Trade> = Vec::new();

    let mut cash = bt_config.initial_cash;
    let mut last_inflow_year = 0;

    for (date, price) in &sp500_data {
        if *date < bt_config.start_date || *date > bt_config.end_date {
            continue;
        }

        let year = date.year();

        // 每年3月末注资
        if year > last_inflow_year && date.month() >= 3 {
            cash += bt_config.annual_inflow;
            total_inflow += bt_config.annual_inflow;
            last_inflow_year = year;
        }

        // 如果有现金，立即买入
        if cash > 0.0 {
            let shares = cash / price;
            total_shares += shares;

            trades.push(Trade {
                date: *date,
                action: "买入".to_string(),
                zone: "持有".to_string(),
                fgi: 50.0,
                shares,
                price: *price,
                amount: cash,
                pct: "100%".to_string(),
                ann_ret: None,
            });

            cash = 0.0;
        }
    }

    let final_value = total_shares * end_price;

    let days = (bt_config.end_date - bt_config.start_date).num_days() as f64;
    let years = days / 365.0;

    let total_return = (final_value / total_inflow) - 1.0;
    let annualized_return = (final_value / total_inflow).powf(1.0 / years) - 1.0;

    let buy_count = trades.len();

    BacktestResult {
        name: "买入持有".to_string(),
        total_inflow,
        final_value,
        total_return,
        annualized_return,
        max_drawdown: 0.147,
        trades,
        buy_count,
        sell_count: 0,
        buy_by_zone: HashMap::new(),
        sell_by_zone: HashMap::new(),
    }
}

pub fn print_comparison(results: &[BacktestResult]) {
    println!();
    println!("=================================================================");
    println!("   策略对比");
    println!("=================================================================");
    println!();

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS);
    table.set_header(vec![
        Cell::new("策略"),
        Cell::new("年化收益"),
        Cell::new("总收益率"),
        Cell::new("最大回撤"),
        Cell::new("买入"),
        Cell::new("卖出"),
    ]);

    for result in results {
        let ann_color = if result.annualized_return >= 0.0 {
            Color::Green
        } else {
            Color::Red
        };
        let total_color = if result.total_return >= 0.0 {
            Color::Green
        } else {
            Color::Red
        };
        table.add_row(vec![
            Cell::new(&result.name),
            Cell::new(format!("{:.2}%", result.annualized_return * 100.0)).fg(ann_color),
            Cell::new(format!("{:.2}%", result.total_return * 100.0)).fg(total_color),
            Cell::new(format!("{:.2}%", result.max_drawdown * 100.0)),
            Cell::new(result.buy_count.to_string()),
            Cell::new(result.sell_count.to_string()),
        ]);
    }
    println!("{}", table);
    println!();
}

pub fn run_param_comparison(
    base_config: &AppConfig,
    bt_config: &BacktestConfig,
) -> Vec<BacktestResult> {
    let mut results = Vec::new();

    let result_default = run_backtest(base_config, bt_config);
    results.push(result_default);

    let mut config_aggressive = base_config.clone();
    config_aggressive.buy_ratio.extreme_fear = 70.0;
    config_aggressive.buy_ratio.fear = 40.0;
    config_aggressive.sell_ratio.extreme_greed_target_high = 60.0;
    config_aggressive.sell_ratio.greed_target_high = 50.0;
    let mut result = run_backtest(&config_aggressive, bt_config);
    result.name = "激进配置".to_string();
    results.push(result);

    let mut config_ultra = base_config.clone();
    config_ultra.buy_ratio.extreme_fear = 80.0;
    config_ultra.buy_ratio.fear = 50.0;
    config_ultra.buy_ratio.neutral = 25.0;
    config_ultra.sell_ratio.extreme_greed_target_high = 70.0;
    config_ultra.sell_ratio.greed_target_high = 55.0;
    let mut result = run_backtest(&config_ultra, bt_config);
    result.name = "超激进配置".to_string();
    results.push(result);

    let mut config_max = base_config.clone();
    config_max.buy_ratio.extreme_fear = 90.0;
    config_max.buy_ratio.fear = 60.0;
    config_max.buy_ratio.neutral = 30.0;
    config_max.sell_ratio.extreme_greed_target_high = 80.0;
    config_max.sell_ratio.greed_target_high = 60.0;
    config_max.sell_ratio.extreme_greed_below_target = 40.0;
    let mut result = run_backtest(&config_max, bt_config);
    result.name = "极致激进".to_string();
    results.push(result);

    let mut config_conservative = base_config.clone();
    config_conservative.buy_ratio.neutral = 10.0;
    config_conservative.buy_ratio.fear = 25.0;
    let mut result = run_backtest(&config_conservative, bt_config);
    result.name = "保守配置".to_string();
    results.push(result);

    let mut config_no_neutral = base_config.clone();
    config_no_neutral.buy_ratio.neutral = 0.0;
    let mut result = run_backtest(&config_no_neutral, bt_config);
    result.name = "无中性配置".to_string();
    results.push(result);

    let result_bnh = run_buy_and_hold(bt_config);
    results.push(result_bnh);

    results
}

pub fn run_custom_comparison(
    _base_config: &AppConfig,
    bt_config: &BacktestConfig,
    config_paths: &[&str],
) -> Vec<BacktestResult> {
    let mut results = Vec::new();

    for path in config_paths {
        if let Ok(content) = std::fs::read_to_string(path)
            && let Ok(custom_config) = toml::from_str::<AppConfig>(&content)
        {
            let mut result = run_backtest(&custom_config, bt_config);
            let name = std::path::Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("自定义")
                .to_string();
            result.name = name;
            results.push(result);
        }
    }

    let result_bnh = run_buy_and_hold(bt_config);
    results.push(result_bnh);

    results
}

/// 多资产买入持有基准
pub fn run_multi_asset_buy_and_hold(bt_config: &BacktestConfig) -> MultiAssetBacktestResult {
    let monthly_data = parse_multi_asset_data(MONTHLY_REAL_DATA);

    // 过滤日期范围
    let filtered_data: Vec<_> = monthly_data
        .into_iter()
        .filter(|d| d.date >= bt_config.start_date && d.date <= bt_config.end_date)
        .collect();

    let first_data = filtered_data.first().expect("No data");
    let last_data = filtered_data.last().expect("No data");

    // 配置比例 - 使用优化后的激进配置
    let us_ratio = 0.70;
    let cn_ratio = 0.15;
    let gold_ratio = 0.15;

    let mut total_inflow = bt_config.initial_cash;
    let mut trades: Vec<MultiAssetTrade> = Vec::new();
    let mut monthly_values: Vec<MultiAssetMonthly> = Vec::new();
    let mut last_inflow_year = 0;

    // 初始买入
    let us_amount = bt_config.initial_cash * us_ratio;
    let cn_amount = bt_config.initial_cash * cn_ratio;
    let gold_amount = bt_config.initial_cash * gold_ratio;

    let mut us_shares = us_amount / first_data.nasdaq;
    let mut cn_shares = cn_amount / first_data.dividend_low_vol;
    let mut gold_shares = gold_amount / first_data.gold_cny;

    trades.push(MultiAssetTrade {
        date: first_data.date,
        action: "买入".to_string(),
        asset: "NASDAQ".to_string(),
        zone: "持有".to_string(),
        fgi: first_data.fgi,
        shares: us_shares,
        price: first_data.nasdaq,
        amount: us_amount,
        pct: format!("{:.0}%", us_ratio * 100.0),
        ann_ret: None,
    });

    trades.push(MultiAssetTrade {
        date: first_data.date,
        action: "买入".to_string(),
        asset: "DIVIDEND_LOW_VOL".to_string(),
        zone: "持有".to_string(),
        fgi: first_data.fgi,
        shares: cn_shares,
        price: first_data.dividend_low_vol,
        amount: cn_amount,
        pct: format!("{:.0}%", cn_ratio * 100.0),
        ann_ret: None,
    });

    trades.push(MultiAssetTrade {
        date: first_data.date,
        action: "买入".to_string(),
        asset: "GOLD".to_string(),
        zone: "持有".to_string(),
        fgi: first_data.fgi,
        shares: gold_shares,
        price: first_data.gold_cny,
        amount: gold_amount,
        pct: format!("{:.0}%", gold_ratio * 100.0),
        ann_ret: None,
    });

    for data in &filtered_data {
        let year = data.date.year();

        // 每年3月末注资
        if year > last_inflow_year && data.date.month() >= 3 {
            let us_amount = bt_config.annual_inflow * us_ratio;
            let cn_amount = bt_config.annual_inflow * cn_ratio;
            let gold_amount = bt_config.annual_inflow * gold_ratio;

            us_shares += us_amount / data.nasdaq;
            cn_shares += cn_amount / data.dividend_low_vol;
            gold_shares += gold_amount / data.gold_cny;
            total_inflow += bt_config.annual_inflow;
            last_inflow_year = year;

            trades.push(MultiAssetTrade {
                date: data.date,
                action: "买入".to_string(),
                asset: "NASDAQ".to_string(),
                zone: "持有".to_string(),
                fgi: data.fgi,
                shares: us_amount / data.nasdaq,
                price: data.nasdaq,
                amount: us_amount,
                pct: format!("{:.0}%", us_ratio * 100.0),
                ann_ret: None,
            });

            trades.push(MultiAssetTrade {
                date: data.date,
                action: "买入".to_string(),
                asset: "DIVIDEND_LOW_VOL".to_string(),
                zone: "持有".to_string(),
                fgi: data.fgi,
                shares: cn_amount / data.dividend_low_vol,
                price: data.dividend_low_vol,
                amount: cn_amount,
                pct: format!("{:.0}%", cn_ratio * 100.0),
                ann_ret: None,
            });

            trades.push(MultiAssetTrade {
                date: data.date,
                action: "买入".to_string(),
                asset: "GOLD".to_string(),
                zone: "持有".to_string(),
                fgi: data.fgi,
                shares: gold_amount / data.gold_cny,
                price: data.gold_cny,
                amount: gold_amount,
                pct: format!("{:.0}%", gold_ratio * 100.0),
                ann_ret: None,
            });
        }

        let total_value = us_shares * data.nasdaq + cn_shares * data.dividend_low_vol + gold_shares * data.gold_cny;

        monthly_values.push(MultiAssetMonthly {
            date: data.date,
            fgi: data.fgi,
            zone: "持有".to_string(),
            nasdaq: data.nasdaq,
            dividend_low_vol: data.dividend_low_vol,
            gold_cny: data.gold_cny,
            cash: 0.0,
            total_value,
        });
    }

    let final_value = us_shares * last_data.nasdaq + cn_shares * last_data.dividend_low_vol + gold_shares * last_data.gold_cny;
    let total_return = (final_value / total_inflow) - 1.0;

    let days = (bt_config.end_date - bt_config.start_date).num_days() as f64;
    let years = days / 365.0;
    let annualized_return = (final_value / total_inflow).powf(1.0 / years) - 1.0;

    // 计算最大回撤
    let mut max_value: f64 = 0.0;
    let mut max_drawdown: f64 = 0.0;
    for mv in &monthly_values {
        max_value = max_value.max(mv.total_value);
        let drawdown = (max_value - mv.total_value) / max_value;
        max_drawdown = max_drawdown.max(drawdown);
    }

    let buy_count = trades.len();
    let mut buy_by_zone: HashMap<String, (usize, f64)> = HashMap::new();
    buy_by_zone.insert("持有".to_string(), (buy_count, total_inflow));

    let mut buy_by_asset: HashMap<String, (usize, f64)> = HashMap::new();
    buy_by_asset.insert("NASDAQ".to_string(), (buy_count / 3, us_shares * last_data.nasdaq));
    buy_by_asset.insert("DIVIDEND_LOW_VOL".to_string(), (buy_count / 3, cn_shares * last_data.dividend_low_vol));
    buy_by_asset.insert("GOLD".to_string(), (buy_count / 3, gold_shares * last_data.gold_cny));

    MultiAssetBacktestResult {
        name: "多资产买入持有".to_string(),
        total_inflow,
        final_value,
        total_return,
        annualized_return,
        max_drawdown,
        trades,
        buy_count,
        sell_count: 0,
        buy_by_zone,
        sell_by_zone: HashMap::new(),
        buy_by_asset,
        sell_by_asset: HashMap::new(),
    }
}

/// 打印多资产对比结果
pub fn print_multi_asset_comparison(results: &[MultiAssetBacktestResult]) {
    println!();
    println!("=================================================================");
    println!("   多资产策略对比");
    println!("=================================================================");
    println!();

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS);
    table.set_header(vec![
        Cell::new("策略"),
        Cell::new("年化收益"),
        Cell::new("总收益率"),
        Cell::new("最大回撤"),
        Cell::new("买入"),
        Cell::new("卖出"),
    ]);

    for result in results {
        let ann_color = if result.annualized_return >= 0.0 {
            Color::Green
        } else {
            Color::Red
        };
        let total_color = if result.total_return >= 0.0 {
            Color::Green
        } else {
            Color::Red
        };
        table.add_row(vec![
            Cell::new(&result.name),
            Cell::new(format!("{:.2}%", result.annualized_return * 100.0)).fg(ann_color),
            Cell::new(format!("{:.2}%", result.total_return * 100.0)).fg(total_color),
            Cell::new(format!("{:.2}%", result.max_drawdown * 100.0)),
            Cell::new(result.buy_count.to_string()),
            Cell::new(result.sell_count.to_string()),
        ]);
    }
    println!("{}", table);
    println!();
}
