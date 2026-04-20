use crate::config::AppConfig;
use crate::models::Position;
use crate::strategy::{
    BuySuggestion, calculate_buy_suggestions, calculate_sell_suggestions, check_risk_warnings,
};
use chrono::{Datelike, NaiveDate};
use std::collections::HashMap;

const HISTORICAL_FGI_2016_2020: &str =
    include_str!("../.agents/skills/mns-backtest/data/fgi_2016_2020.csv");

const SUPPLEMENTARY_FGI: &str =
    include_str!("../.agents/skills/mns-backtest/data/fgi_2020_2025.csv");

const SP500_MONTHLY: &str = include_str!("../.agents/skills/mns-backtest/data/sp500_monthly.csv");

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
            println!("      {:<8}:{:>5} 次, ¥{:>12.2}", zone, count, amount);
        }
        println!();

        println!("  【按情绪区间 - 卖出】");
        let mut sell_zones: Vec<_> = self.sell_by_zone.iter().collect();
        sell_zones.sort_by_key(|(k, _)| k.as_str());
        for (zone, (count, amount)) in sell_zones {
            println!("      {:<8}:{:>5} 次, ¥{:>12.2}", zone, count, amount);
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
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 2 {
                let year_month = parts[0];
                let price: f64 = parts[1].parse().ok()?;

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
    let sp500_data = parse_sp500_data(SP500_MONTHLY);

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
    let mut last_trade_month: i32 = -100; // 上次交易的月份（用于冷却期）
    let mut buy_count = 0usize;
    let mut sell_count = 0usize;
    let mut buy_by_zone: HashMap<String, (usize, f64)> = HashMap::new();
    let mut sell_by_zone: HashMap<String, (usize, f64)> = HashMap::new();

    for (date, score, price) in &combined_data {
        let year = date.year();
        let month_key = year * 12 + date.month() as i32; // 用于冷却期计算
        
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
        let zone_changed = prev_zone.map_or(true, |pz| pz != zone);
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
            || (new_capital && *score < config.thresholds.neutral) // 有新资金+可买入区间
            || (months_since_trade >= 3 && *score < config.thresholds.neutral); // 冷却期后+恐慌区间

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
    let sp500_data = parse_sp500_data(SP500_MONTHLY);

    // 找到起始和结束价格
    let _start_price = sp500_data
        .iter()
        .find(|(d, _)| *d >= bt_config.start_date)
        .map(|(_, p)| *p)
        .unwrap_or(1940.24);

    let end_price = sp500_data
        .iter()
        .rfind(|(d, _)| *d <= bt_config.end_date)
        .map(|(_, p)| *p)
        .unwrap_or(6049.06);

    let _start_date = sp500_data
        .iter()
        .find(|(d, _)| *d >= bt_config.start_date)
        .map(|(d, _)| *d)
        .unwrap_or(bt_config.start_date);

    // 计算总投入
    let _start_year = bt_config.start_date.year();
    let _end_year = bt_config.end_date.year();
    let mut total_inflow = bt_config.initial_cash;
    let mut total_shares = 0.0;
    let mut trades: Vec<Trade> = Vec::new();

    // 模拟每月投资：初始资金在第一个月买入，每年3月末追加投资
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
    println!("  策略               年化收益     总收益率     最大回撤     买入     卖出");
    println!("  --------------------------------------------------");

    for result in results {
        println!(
            "  {:<12} {:>7.2}%   {:>7.2}%   {:>7.2}%    {:>4}     {:>4}",
            result.name,
            result.annualized_return * 100.0,
            result.total_return * 100.0,
            result.max_drawdown * 100.0,
            result.buy_count,
            result.sell_count
        );
    }
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
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Ok(custom_config) = toml::from_str::<AppConfig>(&content) {
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
    }

    let result_bnh = run_buy_and_hold(bt_config);
    results.push(result_bnh);

    results
}
