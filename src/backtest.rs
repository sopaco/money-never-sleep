use crate::config::AppConfig;
use crate::models::Position;
use crate::strategy::{calculate_buy_suggestions, calculate_sell_suggestions, check_risk_warnings};
use chrono::{Datelike, NaiveDate};
use std::collections::HashMap;

/// 历史恐贪指数数据 (2016-2020.09 逐日数据)
const HISTORICAL_FGI_2016_2020: &str =
    include_str!("../.agents/skills/mns-backtest/data/fgi_2016_2020.csv");

/// 补充恐贪指数数据 (2020.10-2025.04 月度近似)
const SUPPLEMENTARY_FGI: &str =
    include_str!("../.agents/skills/mns-backtest/data/fgi_2020_2025.csv");

/// S&P 500 月度收盘价
const SP500_MONTHLY: &str = include_str!("../.agents/skills/mns-backtest/data/sp500_monthly.csv");

/// 回测配置
#[derive(Debug, Clone)]
pub struct BacktestConfig {
    /// 初始资金
    pub initial_cash: f64,
    /// 每年追加资金 (2月注入)
    pub annual_inflow: f64,
    /// 回测起始日期
    pub start_date: NaiveDate,
    /// 回测结束日期
    pub end_date: NaiveDate,
}

impl Default for BacktestConfig {
    fn default() -> Self {
        Self {
            initial_cash: 100_000.0,
            annual_inflow: 50_000.0,
            start_date: NaiveDate::from_ymd_opt(2016, 1, 4).unwrap(),
            end_date: NaiveDate::from_ymd_opt(2025, 4, 17).unwrap(),
        }
    }
}

/// 回测状态
#[derive(Debug, Clone)]
pub struct BacktestState {
    /// 现金余额
    pub cash: f64,
    /// 总投入资金
    pub total_inflow: f64,
    /// 持仓
    pub position: BacktestPosition,
    /// 交易记录
    pub trades: Vec<Trade>,
    /// 每日资产价值
    pub daily_values: Vec<DailyValue>,
    /// 上次注入年份
    last_inflow_year: i32,
}

impl BacktestState {
    pub fn new(initial_cash: f64) -> Self {
        Self {
            cash: initial_cash,
            total_inflow: initial_cash,
            position: BacktestPosition::default(),
            trades: Vec::new(),
            daily_values: Vec::new(),
            last_inflow_year: 0,
        }
    }

    /// 总资产价值
    pub fn total_value(&self, price: f64) -> f64 {
        self.cash + self.position.market_value(price)
    }
}

/// 回测持仓
#[derive(Debug, Clone, Default)]
pub struct BacktestPosition {
    pub shares: f64,
    pub cost_price: f64,
    pub first_buy_date: NaiveDate,
}

impl BacktestPosition {
    pub fn market_value(&self, current_price: f64) -> f64 {
        self.shares * current_price
    }
}

/// 交易记录
#[derive(Debug, Clone)]
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

/// 每日资产价值
#[derive(Debug, Clone)]
pub struct DailyValue {
    pub date: NaiveDate,
    pub fgi: f64,
    pub zone: String,
    pub sp500: f64,
    pub cash: f64,
    pub position_value: f64,
    pub total_value: f64,
}

/// 回测结果
#[derive(Debug, Clone)]
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
            "    总收益率:             {:>8.2}%",
            self.total_return * 100.0
        );
        println!(
            "    年化收益率:           {:>8.2}%",
            self.annualized_return * 100.0
        );
        println!(
            "    最大回撤:             {:>8.2}%",
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
        for zone in &["极度恐慌", "恐慌", "中性", "贪婪"] {
            if let Some((count, amount)) = self.buy_by_zone.get(*zone) {
                println!("      {}: {:>4} 次, ¥{:>12.2}", zone, count, amount);
            }
        }
        println!();

        println!("  【按情绪区间 - 卖出】");
        for zone in &["中性", "贪婪", "极度贪婪"] {
            if let Some((count, amount)) = self.sell_by_zone.get(*zone) {
                println!("      {}: {:>4} 次, ¥{:>12.2}", zone, count, amount);
            }
        }
        println!();

        // 关键交易 (每年 Top 3)
        println!("  【关键交易】（每年 Top 3）");
        let mut trades_by_year: HashMap<i32, Vec<&Trade>> = HashMap::new();
        for trade in &self.trades {
            let year = trade.date.year();
            trades_by_year.entry(year).or_default().push(trade);
        }

        for year in 2016..=2025 {
            if let Some(year_trades) = trades_by_year.get(&year) {
                let mut sorted: Vec<_> = year_trades.iter().collect();
                sorted.sort_by(|a, b| {
                    b.amount
                        .partial_cmp(&a.amount)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                for trade in sorted.iter().take(3) {
                    let ann_str = match trade.ann_ret {
                        Some(r) => format!("{:.1}%", r * 100.0),
                        None => "-".to_string(),
                    };
                    println!(
                        "    {} {} {}(FGI:{:.0}), ¥{:>10.0} ({}, 年化{})",
                        trade.date,
                        trade.action,
                        trade.zone,
                        trade.fgi,
                        trade.amount,
                        trade.pct,
                        ann_str
                    );
                }
            }
        }
    }
}

/// 解析恐贪指数数据
fn parse_fgi_data(raw: &str) -> Vec<(NaiveDate, f64)> {
    let mut result = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() == 2 {
            if let Ok(date) = NaiveDate::parse_from_str(parts[0].trim(), "%Y-%m-%d") {
                if let Ok(score) = parts[1].trim().parse::<f64>() {
                    result.push((date, score));
                }
            }
        }
    }
    result
}

/// 解析 S&P 500 月度数据
fn parse_sp500_data(raw: &str) -> HashMap<String, f64> {
    let mut result = HashMap::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() == 2 {
            let month = parts[0].trim().to_string();
            if let Ok(price) = parts[1].trim().parse::<f64>() {
                result.insert(month, price);
            }
        }
    }
    result
}

/// 获取 S&P 500 价格 (月度匹配)
fn get_sp500_price(date: NaiveDate, sp500_data: &HashMap<String, f64>) -> Option<f64> {
    let month_key = format!("{:04}-{:02}", date.year(), date.month());
    sp500_data.get(&month_key).copied()
}

/// 获取情绪区间名称
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

/// 将回测持仓转换为 Position 对象
fn create_position(state: &BacktestState, price: f64, date: NaiveDate) -> Option<Position> {
    if state.position.shares <= 0.0 {
        return None;
    }
    Some(Position {
        id: 1,
        asset_code: "SPY".to_string(),
        asset_name: "S&P 500 ETF".to_string(),
        category: "us_stocks".to_string(),
        shares: state.position.shares,
        cost_price: state.position.cost_price,
        current_price: Some(price),
        first_buy_date: state.position.first_buy_date.format("%Y-%m-%d").to_string(),
        updated_at: date.format("%Y-%m-%d").to_string(),
    })
}

/// 运行回测 (使用真实策略引擎)
pub fn run_backtest(config: &AppConfig, bt_config: &BacktestConfig) -> BacktestResult {
    // 解析数据
    let mut fgi_data = parse_fgi_data(HISTORICAL_FGI_2016_2020);
    fgi_data.extend(parse_fgi_data(SUPPLEMENTARY_FGI));
    fgi_data.sort_by_key(|(d, _)| *d);

    // 过滤日期范围
    let fgi_data: Vec<_> = fgi_data
        .into_iter()
        .filter(|(d, _)| *d >= bt_config.start_date && *d <= bt_config.end_date)
        .collect();

    let sp500_data = parse_sp500_data(SP500_MONTHLY);

    // 初始化状态
    let mut state = BacktestState::new(bt_config.initial_cash);
    let mut prev_zone: Option<&str> = None;

    // 统计
    let mut buy_count = 0usize;
    let mut sell_count = 0usize;
    let mut buy_by_zone: HashMap<String, (usize, f64)> = HashMap::new();
    let mut sell_by_zone: HashMap<String, (usize, f64)> = HashMap::new();

    // 遍历每一天
    for (date, score) in &fgi_data {
        let price = match get_sp500_price(*date, &sp500_data) {
            Some(p) => p,
            None => continue,
        };

        // 年度注资 (每年2月)
        let year = date.year();
        if year > state.last_inflow_year && date.month() >= 2 {
            state.cash += bt_config.annual_inflow;
            state.total_inflow += bt_config.annual_inflow;
            state.last_inflow_year = year;
        }

        let zone = get_zone_name(*score, config);
        let zone_changed = prev_zone.map_or(true, |pz| pz != zone);
        prev_zone = Some(zone);

        // 创建当前持仓的 Position 对象
        let positions: Vec<Position> = create_position(&state, price, *date)
            .map(|p| vec![p])
            .unwrap_or_default();

        // ===== 调用真实策略引擎 =====
        // 1. 计算卖出建议
        let sell_suggestions = if zone_changed {
            calculate_sell_suggestions(config, *score, &positions)
        } else {
            Vec::new()
        };

        // 2. 检查风险警告
        let risk_warnings = check_risk_warnings(config, *score, &positions);

        // 3. 计算买入建议 (使用卖出回收资金)
        let buy_suggestion = if zone_changed {
            calculate_buy_suggestions(
                config,
                *score,
                state.cash,
                &positions,
                &sell_suggestions,
                &risk_warnings,
            )
        } else {
            crate::strategy::BuySuggestion {
                total_amount: 0.0,
                us_amount: 0.0,
                cn_amount: 0.0,
                counter_amount: 0.0,
                details: Vec::new(),
                excluded: Vec::new(),
            }
        };

        // ===== 执行卖出 =====
        for sell in &sell_suggestions {
            if sell.sell_shares >= 0.01 {
                state.position.shares -= sell.sell_shares;
                state.cash += sell.sell_amount;

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
                    price,
                    amount: sell.sell_amount,
                    pct: format!("{}%", sell.sell_ratio as i32),
                    ann_ret: sell.annualized_return,
                });

                // 清仓后重置
                if state.position.shares < 0.01 {
                    state.position.shares = 0.0;
                    state.position.cost_price = 0.0;
                }
            }
        }

        // ===== 执行买入 =====
        // 在回测中，我们只买入 SPY (单一资产)
        let buy_amount = buy_suggestion.us_amount; // 使用美股分配金额
        if buy_amount > 0.0 && price > 0.0 {
            let buy_shares = buy_amount / price;
            if buy_shares >= 0.01 {
                // 更新成本 (加权平均)
                let total_shares = state.position.shares + buy_shares;
                if state.position.shares == 0.0 {
                    state.position.first_buy_date = *date;
                    state.position.cost_price = price;
                } else {
                    state.position.cost_price = (state.position.shares * state.position.cost_price
                        + buy_shares * price)
                        / total_shares;
                }
                state.position.shares = total_shares;
                state.cash -= buy_amount;

                buy_count += 1;
                let zone_key = zone.to_string();
                buy_by_zone.entry(zone_key.clone()).or_insert((0, 0.0)).0 += 1;
                buy_by_zone.get_mut(&zone_key).unwrap().1 += buy_amount;

                state.trades.push(Trade {
                    date: *date,
                    action: "买入".to_string(),
                    zone: zone.to_string(),
                    fgi: *score,
                    shares: buy_shares,
                    price,
                    amount: buy_amount,
                    pct: format!(
                        "{}%",
                        (buy_amount / (state.cash + buy_amount) * 100.0) as i32
                    ),
                    ann_ret: None,
                });
            }
        }

        // 记录每日价值
        state.daily_values.push(DailyValue {
            date: *date,
            fgi: *score,
            zone: zone.to_string(),
            sp500: price,
            cash: state.cash,
            position_value: state.position.market_value(price),
            total_value: state.total_value(price),
        });
    }

    // 计算回测指标
    let final_value = state
        .daily_values
        .last()
        .map(|d| d.total_value)
        .unwrap_or(bt_config.initial_cash);
    let total_return = (final_value / state.total_inflow) - 1.0;

    // 年化收益
    let days = (bt_config.end_date - bt_config.start_date).num_days() as f64;
    let years = days / 365.0;
    let annualized_return = (final_value / state.total_inflow).powf(1.0 / years) - 1.0;

    // 最大回撤
    let mut max_value: f64 = 0.0;
    let mut max_drawdown: f64 = 0.0;
    for dv in &state.daily_values {
        max_value = max_value.max(dv.total_value);
        let drawdown = (max_value - dv.total_value) / max_value;
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

/// 运行买入持有基准
pub fn run_buy_and_hold(bt_config: &BacktestConfig) -> BacktestResult {
    let mut fgi_data = parse_fgi_data(HISTORICAL_FGI_2016_2020);
    fgi_data.extend(parse_fgi_data(SUPPLEMENTARY_FGI));
    fgi_data.sort_by_key(|(d, _)| *d);

    let fgi_data: Vec<_> = fgi_data
        .into_iter()
        .filter(|(d, _)| *d >= bt_config.start_date && *d <= bt_config.end_date)
        .collect();

    let sp500_data = parse_sp500_data(SP500_MONTHLY);

    let mut cash = bt_config.initial_cash;
    let mut shares = 0.0;
    let mut total_inflow = bt_config.initial_cash;
    let mut last_inflow_year = 0;
    let mut daily_values = Vec::new();
    let mut first_invested = false;

    for (date, score) in &fgi_data {
        let price = match get_sp500_price(*date, &sp500_data) {
            Some(p) => p,
            None => continue,
        };

        // 年度注资
        let year = date.year();
        if year > last_inflow_year && date.month() >= 2 {
            shares += bt_config.annual_inflow / price;
            total_inflow += bt_config.annual_inflow;
            last_inflow_year = year;
        }

        // 初始投资
        if !first_invested && cash > 0.0 {
            shares = cash / price;
            cash = 0.0;
            first_invested = true;
        }

        daily_values.push(DailyValue {
            date: *date,
            fgi: *score,
            zone: String::new(),
            sp500: price,
            cash: 0.0,
            position_value: shares * price,
            total_value: shares * price,
        });
    }

    let final_value = daily_values
        .last()
        .map(|d| d.total_value)
        .unwrap_or(bt_config.initial_cash);
    let total_return = (final_value / total_inflow) - 1.0;

    let days = (bt_config.end_date - bt_config.start_date).num_days() as f64;
    let years = days / 365.0;
    let annualized_return = (final_value / total_inflow).powf(1.0 / years) - 1.0;

    // 最大回撤
    let mut max_value: f64 = 0.0;
    let mut max_drawdown: f64 = 0.0;
    for dv in &daily_values {
        max_value = max_value.max(dv.total_value);
        let drawdown = (max_value - dv.total_value) / max_value;
        max_drawdown = max_drawdown.max(drawdown);
    }

    BacktestResult {
        name: "买入持有".to_string(),
        total_inflow,
        final_value,
        total_return,
        annualized_return,
        max_drawdown,
        trades: Vec::new(),
        buy_count: 1,
        sell_count: 0,
        buy_by_zone: HashMap::new(),
        sell_by_zone: HashMap::new(),
    }
}

/// 打印对比报告
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

/// 运行参数对比回测
pub fn run_param_comparison(
    base_config: &AppConfig,
    bt_config: &BacktestConfig,
) -> Vec<BacktestResult> {
    let mut results = Vec::new();

    // 默认配置
    let result_default = run_backtest(base_config, bt_config);
    results.push(result_default);

    // 激进配置：提高极度恐慌/恐慌买入比例
    let mut config_aggressive = base_config.clone();
    config_aggressive.buy_ratio.extreme_fear = 70.0;
    config_aggressive.buy_ratio.fear = 40.0;
    let mut result = run_backtest(&config_aggressive, bt_config);
    result.name = "激进配置".to_string();
    results.push(result);

    // 保守配置：降低中性买入
    let mut config_conservative = base_config.clone();
    config_conservative.buy_ratio.neutral = 10.0;
    config_conservative.buy_ratio.fear = 25.0;
    let mut result = run_backtest(&config_conservative, bt_config);
    result.name = "保守配置".to_string();
    results.push(result);

    // 无中性买入配置
    let mut config_no_neutral = base_config.clone();
    config_no_neutral.buy_ratio.neutral = 0.0;
    let mut result = run_backtest(&config_no_neutral, bt_config);
    result.name = "无中性配置".to_string();
    results.push(result);

    // 买入持有基准
    let result_bnh = run_buy_and_hold(bt_config);
    results.push(result_bnh);

    results
}

/// 运行自定义配置对比
pub fn run_custom_comparison(
    configs: Vec<(String, AppConfig)>,
    bt_config: &BacktestConfig,
) -> Vec<BacktestResult> {
    let mut results = Vec::new();

    for (name, config) in configs {
        let mut result = run_backtest(&config, bt_config);
        result.name = name;
        results.push(result);
    }

    // 添加买入持有基准
    let result_bnh = run_buy_and_hold(bt_config);
    results.push(result_bnh);

    results
}
