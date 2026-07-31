mod backtest;
mod cli;
mod config;
mod db;
mod market;
mod metrics;
mod models;
mod quote;
mod report;
mod sentiment;
mod strategy;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{BacktestAction, CashAction, Commands};
use comfy_table::{Cell, Color, Table, modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL};
use config::AppConfig;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::Cli::parse();

    match cli.command {
        Commands::Init { force } => cmd_init(force)?,
        Commands::Config { key, value } => cmd_config(key, value)?,
        Commands::Cash { action } => match action {
            None => cmd_cash()?,
            Some(CashAction::Set { amount }) => cmd_cash_set(amount)?,
            Some(CashAction::Add { amount }) => cmd_cash_add(amount)?,
        },
        Commands::Portfolio => cmd_portfolio()?,
        Commands::Add {
            code,
            name,
            category,
        } => cmd_add(&code, &name, &category)?,
        Commands::Buy {
            code,
            shares,
            price,
        } => cmd_buy(&code, shares, price)?,
        Commands::Sell {
            code,
            shares,
            price,
        } => cmd_sell(&code, shares, price)?,
        Commands::Price { code, price } => cmd_price(&code, price)?,
        Commands::Remove { code } => cmd_remove(&code)?,
        Commands::Sentiment => cmd_sentiment().await?,
        Commands::Report => cmd_report().await?,
        Commands::History { limit } => cmd_history(limit)?,
        Commands::Backtest { action } => match action {
            None => cmd_backtest(None, None)?,
            Some(BacktestAction::Run { config, compare }) => cmd_backtest(config, compare)?,
            Some(BacktestAction::Validate { iterations, block }) => {
                cmd_backtest_validate(iterations, block)?
            }
            Some(BacktestAction::Params) => cmd_backtest_params()?,
        },
        Commands::UpdatePrices => cmd_update_prices().await?,
        Commands::Market => cmd_market().await?,
        Commands::MarketIndices => cmd_market_indices().await?,
        Commands::Analyze { symbol } => cmd_analyze(&symbol).await?,
    }

    Ok(())
}

fn cmd_init(force: bool) -> Result<()> {
    use std::io::{self, Write};

    let config_path = AppConfig::config_path()?;
    let db_path = AppConfig::db_path()?;

    let config_exists = config_path.exists();
    let db_exists = db_path.exists();

    if (config_exists || db_exists) && !force {
        println!("⚠️  检测到已有数据：");
        if config_exists {
            println!("   配置文件: {}", config_path.display());
        }
        if db_exists {
            println!("   数据库:   {}", db_path.display());
        }
        println!();
        print!("继续将覆盖上述文件，数据将丢失。是否继续？[y/N]: ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        let input = input.trim().to_lowercase();
        if input != "y" && input != "yes" {
            println!("已取消初始化。");
            return Ok(());
        }
    }

    // 用户确认后，删除旧的数据库文件
    if db_exists {
        std::fs::remove_file(&db_path)
            .with_context(|| format!("删除数据库失败: {}", db_path.display()))?;
    }

    let config = AppConfig::default_config();
    config.save()?;

    let db = db::Database::open()?;
    drop(db);

    // 创建报告输出目录
    let report_dir = &config.settings.report_output_dir;
    std::fs::create_dir_all(report_dir)?;

    println!("✓ 初始化完成");
    println!("  配置文件: {}", config_path.display());
    println!("  数据库:   {}", db_path.display());
    println!("  报告目录: {}", report_dir);
    Ok(())
}

fn cmd_config(key: Option<String>, value: Option<String>) -> Result<()> {
    let mut config = AppConfig::load()?;

    match (key, value) {
        (None, None) => {
            // 显示全部配置
            let content = toml::to_string_pretty(&config)?;
            println!("{}", content);
        }
        (Some(k), None) => {
            // 显示某个配置项
            match config.get_value(&k) {
                Some(v) => println!("{} = {}", k, v),
                None => anyhow::bail!("未知的配置项: {}", k),
            }
        }
        (Some(k), Some(v)) => {
            // 修改配置项：校验通过后才落盘，避免写出无法加载的配置
            config.set_value(&k, &v)?;
            config.validate()?;
            config.save()?;
            println!("✓ {} = {}", k, v);
        }
        (None, Some(_)) => unreachable!(),
    }
    Ok(())
}

fn cmd_cash() -> Result<()> {
    let db = db::Database::open()?;
    let balance = db.get_cash_balance()?;
    println!("现金余额: ¥{:.2}", balance);
    Ok(())
}

fn cmd_cash_set(amount: f64) -> Result<()> {
    if amount < 0.0 {
        anyhow::bail!("现金余额不能为负数: {}", amount);
    }
    let db = db::Database::open()?;
    db.set_cash_balance(amount)?;
    println!("✓ 现金余额已设置为: ¥{:.2}", amount);
    Ok(())
}

fn cmd_cash_add(amount: f64) -> Result<()> {
    let db = db::Database::open()?;
    let new_balance = db.add_cash(amount)?;
    println!("✓ 已增加 ¥{:.2}，当前余额: ¥{:.2}", amount, new_balance);
    Ok(())
}

fn cmd_portfolio() -> Result<()> {
    let db = db::Database::open()?;
    let config = AppConfig::load()?;
    let positions = db.list_positions()?;
    let cash = db.get_cash_balance()?;

    if positions.is_empty() {
        println!("暂无持仓，使用 'mns add <code> <name> <category>' 添加资产");
        return Ok(());
    }

    let today = chrono::Local::now().date_naive();
    let min_days = config.settings.min_holding_days;
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS);
    table.set_header(vec![
        Cell::new("代码"),
        Cell::new("名称"),
        Cell::new("类别"),
        Cell::new("份额"),
        Cell::new("成本价"),
        Cell::new("现价"),
        Cell::new("市值"),
        Cell::new("年化收益"),
        Cell::new("绝对收益"),
    ]);

    let mut total_mv = 0.0;
    for pos in &positions {
        let mv = pos.market_value_or_cost();
        total_mv += mv;
        let ann = pos.annualized_return_with_min_days(&today, min_days);
        let ann_str = match ann {
            Some(r) => format!("{:+.1}%", r * 100.0),
            None => "N/A".to_string(),
        };
        let abs_str = match pos.absolute_return() {
            Some(r) => format!("{:+.1}%", r * 100.0),
            None => "N/A".to_string(),
        };
        let price_str = match pos.current_price {
            Some(p) => format!("{:.2}", p),
            None => "-".to_string(),
        };
        let category_cn = match pos.category.as_str() {
            "us_stocks" => "美股",
            "cn_stocks" => "A股",
            "counter_cyclical" => "逆周期",
            _ => &pos.category,
        };
        let mut ann_cell = Cell::new(&ann_str);
        if let Some(r) = ann {
            if r * 100.0 >= config.settings.annualized_target_high {
                ann_cell = ann_cell.fg(Color::Green);
            } else if r < 0.0 {
                ann_cell = ann_cell.fg(Color::Red);
            }
        }
        table.add_row(vec![
            Cell::new(&pos.asset_code),
            Cell::new(&pos.asset_name),
            Cell::new(category_cn),
            Cell::new(format!("{:.2}", pos.shares)),
            Cell::new(format!("{:.2}", pos.cost_price)),
            Cell::new(price_str),
            Cell::new(format!("¥{:.2}", mv)),
            ann_cell,
            Cell::new(&abs_str),
        ]);
    }

    println!("{}", table);
    println!("\n现金余额: ¥{:.2}", cash);
    println!("持仓市值: ¥{:.2}", total_mv);
    println!("总资产:   ¥{:.2}", cash + total_mv);
    Ok(())
}

fn cmd_add(code: &str, name: &str, category: &str) -> Result<()> {
    let valid_categories = ["us_stocks", "cn_stocks", "counter_cyclical"];
    if !valid_categories.contains(&category) {
        anyhow::bail!(
            "无效类别 '{}'，可选: {}",
            category,
            valid_categories.join(", ")
        );
    }
    let db = db::Database::open()?;
    db.add_position(code, name, category)?;
    println!("✓ 已添加资产: {} ({}) [{}]", code, name, category);
    Ok(())
}

fn cmd_buy(code: &str, shares: f64, price: f64) -> Result<()> {
    let db = db::Database::open()?;
    let amount = shares * price;
    db.buy_position(code, shares, price)?;
    println!(
        "✓ 买入 {} {:.2} 份 @ ¥{:.2}，合计 ¥{:.2}",
        code, shares, price, amount
    );
    Ok(())
}

fn cmd_sell(code: &str, shares: f64, price: f64) -> Result<()> {
    let db = db::Database::open()?;
    let amount = shares * price;
    db.sell_position(code, shares, price)?;
    println!(
        "✓ 卖出 {} {:.2} 份 @ ¥{:.2}，合计 ¥{:.2}",
        code, shares, price, amount
    );
    Ok(())
}

fn cmd_price(code: &str, price: Option<f64>) -> Result<()> {
    let db = db::Database::open()?;
    match price {
        Some(p) => {
            db.update_price(code, p)?;
            println!("✓ {} 当前价格已更新为 ¥{:.2}", code, p);
        }
        None => {
            let pos = db.get_position(code)?;
            match pos {
                Some(p) => {
                    let cur = match p.current_price {
                        Some(v) => format!("¥{:.2}", v),
                        None => "未设置".to_string(),
                    };
                    println!("{} ({}) 当前价格: {}", p.asset_code, p.asset_name, cur);
                }
                None => anyhow::bail!("未找到资产: {}", code),
            }
        }
    }
    Ok(())
}

fn cmd_remove(code: &str) -> Result<()> {
    let db = db::Database::open()?;
    db.remove_position(code)?;
    Ok(())
}

async fn cmd_sentiment() -> Result<()> {
    let config = AppConfig::load()?;
    println!("正在获取恐贪指数...");

    // 使用配置的 API URL
    let url = &config.api.fear_greed_url;
    let data = sentiment::fetch_fear_greed_data(url).await?;
    let score_f64 = data.score as f64;
    let zone = config.sentiment_zone(score_f64);

    println!("恐贪指数: {} ({})", data.score, zone);

    // 保存快照（含历史数据）
    let db = db::Database::open()?;
    db.save_fear_greed_snapshot(
        score_f64,
        zone,
        data.previous_close,
        data.previous_1_week,
        data.previous_1_month,
        data.previous_1_year,
    )?;

    Ok(())
}

async fn cmd_report() -> Result<()> {
    let config = AppConfig::load()?;
    let db = db::Database::open()?;

    println!("正在获取恐贪指数...");

    // 使用配置的 API URL
    let url = &config.api.fear_greed_url;
    let data = sentiment::fetch_fear_greed_data(url).await?;
    let score_f64 = data.score as f64;
    let zone = config.sentiment_zone(score_f64);

    // 保存快照（含历史数据）
    db.save_fear_greed_snapshot(
        score_f64,
        zone,
        data.previous_close,
        data.previous_1_week,
        data.previous_1_month,
        data.previous_1_year,
    )?;

    println!("恐贪指数: {} ({})", data.score, zone);

    let cash = db.get_cash_balance()?;
    let positions = db.list_positions()?;

    // 策略计算：目标仓位 + 偏离带（与回测引擎同源）
    let today = chrono::Local::now().date_naive();
    let risk_warnings = strategy::check_risk_warnings(&config, score_f64, &positions);
    let plan =
        strategy::calculate_rebalance_plan(&config, score_f64, cash, &positions, today);

    // 生成报告
    let report = report::generate_report(
        &config,
        score_f64,
        zone,
        data.previous_close,
        data.previous_1_week,
        data.previous_1_month,
        data.previous_1_year,
        cash,
        &positions,
        &plan,
        &risk_warnings,
    )?;

    let filepath = report::save_report(&config, &report)?;
    println!("{}", report);
    println!("\n报告已保存至: {}", filepath);
    Ok(())
}

fn cmd_history(limit: i64) -> Result<()> {
    let db = db::Database::open()?;
    let txs = db.list_transactions(limit)?;

    if txs.is_empty() {
        println!("暂无交易记录");
        return Ok(());
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS);
    table.set_header(vec!["日期", "类型", "代码", "份额", "价格", "金额"]);

    for tx in &txs {
        let type_label = if tx.tx_type == "buy" {
            "买入"
        } else {
            "卖出"
        };
        table.add_row(vec![
            Cell::new(&tx.tx_date),
            Cell::new(type_label),
            Cell::new(&tx.asset_code),
            Cell::new(format!("{:.2}", tx.shares)),
            Cell::new(format!("{:.2}", tx.price)),
            Cell::new(format!("¥{:.2}", tx.amount)),
        ]);
    }

    println!("{}", table);
    Ok(())
}

fn cmd_backtest(config_path: Option<String>, compare: Option<String>) -> Result<()> {
    use backtest::{BacktestConfig, Engine, SignalConfig, print_comparison, print_report};

    println!("=================================================================");
    println!("   MNS 逆向投资策略回测");
    println!("=================================================================");
    println!();

    let rows = backtest::load_main();
    if rows.is_empty() {
        anyhow::bail!("回测数据集为空");
    }
    let bt_config = BacktestConfig::spanning(&rows, 100_000.0, 50_000.0);
    let signal = SignalConfig::default();

    println!(
        "[INFO] 回测期间: {} ~ {}  ({} 个月)",
        bt_config.start_date.format("%Y-%m"),
        bt_config.end_date.format("%Y-%m"),
        rows.len()
    );
    println!(
        "[INFO] 初始资金: {:.0}, 年度注资: {:.0}",
        bt_config.initial_cash, bt_config.annual_inflow
    );
    println!(
        "[INFO] 数据: 真实全收益序列(人民币,含分红) | 情绪信号: {} 月移动平均",
        signal.smooth_months
    );

    if let Some(paths) = compare {
        // 多配置对比：全部走同一数据与成本模型
        let mut results = Vec::new();
        for path in paths.split(',').map(|s| s.trim()) {
            let cfg = AppConfig::load_from_path(path)?;
            let mut r = backtest::run(&cfg, &bt_config, &signal, Engine::TargetWeight, &rows);
            r.name = std::path::Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("自定义")
                .to_string();
            results.push(r);
        }
        let base = AppConfig::load()?;
        results.push(backtest::run(&base, &bt_config, &signal, Engine::BuyHold, &rows));
        for r in &results {
            print_report(r);
        }
        print_comparison(&results);
        return Ok(());
    }

    let config = match config_path {
        Some(path) => AppConfig::load_from_path(&path)?,
        None => AppConfig::load()?,
    };
    println!(
        "[INFO] 资产配置: 美股{:.0}% / A股{:.0}% / 黄金{:.0}%  | 偏离带 {:.1}pp",
        config.allocation.us_stocks,
        config.allocation.cn_stocks,
        config.allocation.counter_cyclical,
        config.rebalance.band_pp
    );
    println!(
        "[INFO] 成本: 买入{:.2}% 卖出{:.2}%+阶梯赎回费 | 现金年化{:.1}%",
        config.costs.buy_fee_pct, config.costs.sell_fee_pct, config.costs.cash_annual_yield
    );

    // 全部跑在同一数据、同一成本模型上，保证对比公平
    let mut results = Vec::new();
    let mut trend = backtest::run(
        &config,
        &bt_config,
        &SignalConfig::trend_tilt(),
        Engine::TargetWeight,
        &rows,
    );
    trend.name = "目标仓位(趋势锚)".to_string();
    results.push(trend);
    let mut senti = backtest::run(&config, &bt_config, &signal, Engine::TargetWeight, &rows);
    senti.name = "目标仓位(情绪锚)".to_string();
    results.push(senti);
    for e in [Engine::Legacy, Engine::BuyHoldRebalanced, Engine::BuyHold] {
        results.push(backtest::run(&config, &bt_config, &signal, e, &rows));
    }

    for r in &results {
        print_report(r);
    }
    print_comparison(&results);

    println!("提示: 运行 `mns backtest validate` 查看样本外验证与收益分布。");
    println!();
    Ok(())
}

fn cmd_backtest_validate(iterations: usize, block: usize) -> Result<()> {
    use backtest::{BacktestConfig, Engine, SignalConfig};

    let config = AppConfig::load()?;
    let rows = backtest::load_main();
    if rows.len() < 48 {
        anyhow::bail!("数据不足以做样本外验证");
    }
    let signal = SignalConfig::default();

    println!("=================================================================");
    println!("   样本外验证");
    println!("=================================================================");
    println!();

    // ── 1) walk-forward：前 60% 调参，后 40% 验证 ──
    let split = rows.len() * 60 / 100;
    let (ins, oos) = rows.split_at(split);
    println!(
        "【Walk-forward】样本内 {} ~ {} ({}个月) → 样本外 {} ~ {} ({}个月)",
        ins[0].date.format("%Y-%m"),
        ins[ins.len() - 1].date.format("%Y-%m"),
        ins.len(),
        oos[0].date.format("%Y-%m"),
        oos[oos.len() - 1].date.format("%Y-%m"),
        oos.len()
    );
    println!();

    let bt_ins = BacktestConfig::spanning(ins, 100_000.0, 50_000.0);
    let bt_oos = BacktestConfig::spanning(oos, 100_000.0, 50_000.0);

    // 网格：整体仓位水平 × 偏离带
    let mut best: Option<(f64, f64, f64)> = None; // (calmar, scale, band)
    let mut grid = Vec::new();
    for scale in [0.85_f64, 1.0, 1.15] {
        for band in [2.0_f64, 4.0, 6.0, 8.0] {
            let mut c = config.clone();
            c.target_weight.extreme_fear = (config.target_weight.extreme_fear * scale).min(100.0);
            c.target_weight.fear = (config.target_weight.fear * scale).min(100.0);
            c.target_weight.neutral = (config.target_weight.neutral * scale).min(100.0);
            c.target_weight.greed = (config.target_weight.greed * scale).min(100.0);
            c.target_weight.extreme_greed = (config.target_weight.extreme_greed * scale).min(100.0);
            c.rebalance.band_pp = band;
            if c.validate().is_err() {
                continue;
            }
            let r = backtest::run(&c, &bt_ins, &signal, Engine::TargetWeight, ins);
            grid.push((scale, band, r.xirr, r.max_drawdown, r.risk.calmar));
            if best.map(|b| r.risk.calmar > b.0).unwrap_or(true) {
                best = Some((r.risk.calmar, scale, band));
            }
        }
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_header(vec!["仓位缩放", "偏离带", "样本内年化", "样本内回撤", "Calmar"]);
    for (scale, band, xirr, dd, calmar) in &grid {
        table.add_row(vec![
            Cell::new(format!("{:.2}x", scale)),
            Cell::new(format!("{:.1}pp", band)),
            Cell::new(format!("{:.2}%", xirr * 100.0)),
            Cell::new(format!("{:.2}%", dd * 100.0)),
            Cell::new(format!("{:.2}", calmar)),
        ]);
    }
    println!("{}", table);

    let (_, best_scale, best_band) = best.expect("网格为空");
    println!(
        "
样本内最优: 仓位缩放 {:.2}x, 偏离带 {:.1}pp",
        best_scale, best_band
    );

    let mut tuned = config.clone();
    tuned.target_weight.extreme_fear = (config.target_weight.extreme_fear * best_scale).min(100.0);
    tuned.target_weight.fear = (config.target_weight.fear * best_scale).min(100.0);
    tuned.target_weight.neutral = (config.target_weight.neutral * best_scale).min(100.0);
    tuned.target_weight.greed = (config.target_weight.greed * best_scale).min(100.0);
    tuned.target_weight.extreme_greed = (config.target_weight.extreme_greed * best_scale).min(100.0);
    tuned.rebalance.band_pp = best_band;

    let oos_tuned = backtest::run(&tuned, &bt_oos, &signal, Engine::TargetWeight, oos);
    let oos_default = backtest::run(&config, &bt_oos, &signal, Engine::TargetWeight, oos);
    let oos_trend = backtest::run(
        &config,
        &bt_oos,
        &SignalConfig::trend_tilt(),
        Engine::TargetWeight,
        oos,
    );
    let oos_bh = backtest::run(&config, &bt_oos, &signal, Engine::BuyHold, oos);

    println!("\n【样本外表现】样本内调出的参数是否还能打？");
    let mut t2 = Table::new();
    t2.load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_header(vec!["参数来源", "样本外年化", "样本外回撤", "Calmar"]);
    for (name, r) in [
        ("样本内最优(情绪锚)", &oos_tuned),
        ("默认配置(情绪锚)", &oos_default),
        ("默认配置(趋势锚)", &oos_trend),
        ("买入持有", &oos_bh),
    ] {
        t2.add_row(vec![
            Cell::new(name),
            Cell::new(format!("{:.2}%", r.xirr * 100.0)),
            Cell::new(format!("{:.2}%", r.max_drawdown * 100.0)),
            Cell::new(format!("{:.2}", r.risk.calmar)),
        ]);
    }
    println!("{}", t2);
    if oos_tuned.risk.calmar < oos_default.risk.calmar {
        println!("⚠ 样本内最优参数在样本外劣于默认配置 —— 典型的过拟合信号。");
    }

    // ── 2) 分块 bootstrap：给出分布而非单点 ──
    println!("
【Bootstrap 分布】{} 次重采样, 分块 {} 个月", iterations, block);
    let bt_full = BacktestConfig::spanning(&rows, 100_000.0, 50_000.0);
    let mut t3 = Table::new();
    t3.load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_header(vec![
            "策略", "年化 P5", "年化 P50", "年化 P95", "回撤 P50", "回撤 P95",
        ]);
    for (label, sc, engine) in [
        ("目标仓位(趋势锚)", SignalConfig::trend_tilt(), Engine::TargetWeight),
        ("目标仓位(情绪锚)", signal, Engine::TargetWeight),
        ("买入持有", signal, Engine::BuyHold),
    ] {
        let r = backtest::run(&config, &bt_full, &sc, engine, &rows);
        let samples =
            metrics::block_bootstrap(&r.returns(), block, iterations, 12.0, 20260731);
        if samples.is_empty() {
            continue;
        }
        let mut cagrs: Vec<f64> = samples.iter().map(|s| s.0).collect();
        let mut dds: Vec<f64> = samples.iter().map(|s| s.1).collect();
        cagrs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        dds.sort_by(|a, b| a.partial_cmp(b).unwrap());
        t3.add_row(vec![
            Cell::new(label),
            Cell::new(format!("{:.2}%", metrics::percentile(&cagrs, 0.05) * 100.0)),
            Cell::new(format!("{:.2}%", metrics::percentile(&cagrs, 0.50) * 100.0)),
            Cell::new(format!("{:.2}%", metrics::percentile(&cagrs, 0.95) * 100.0)),
            Cell::new(format!("{:.2}%", metrics::percentile(&dds, 0.50) * 100.0)),
            Cell::new(format!("{:.2}%", metrics::percentile(&dds, 0.95) * 100.0)),
        ]);
    }
    println!("{}", t3);
    println!("单条历史路径上的年化差异若落在上述分布的宽度之内，就不具备统计显著性。");

    // ── 3) holdout 区块 ──
    let hold = backtest::load_holdout();
    if hold.len() >= 6 {
        println!(
            "
【Holdout 区块】{} ~ {} ({}个月, 从未参与任何调参)",
            hold[0].date.format("%Y-%m"),
            hold[hold.len() - 1].date.format("%Y-%m"),
            hold.len()
        );
        let bt_h = BacktestConfig::spanning(&hold, 100_000.0, 0.0);
        let mut t4 = Table::new();
        t4.load_preset(UTF8_FULL)
            .apply_modifier(UTF8_ROUND_CORNERS)
            .set_header(vec!["策略", "区间收益", "最大回撤"]);
        for (label, sc, engine) in [
            ("目标仓位(趋势锚)", SignalConfig::trend_tilt(), Engine::TargetWeight),
            ("目标仓位(情绪锚)", signal, Engine::TargetWeight),
            ("买入持有", signal, Engine::BuyHold),
        ] {
            let r = backtest::run(&config, &bt_h, &sc, engine, &hold);
            t4.add_row(vec![
                Cell::new(label),
                Cell::new(format!("{:+.2}%", r.total_return * 100.0)),
                Cell::new(format!("{:.2}%", r.max_drawdown * 100.0)),
            ]);
        }
        println!("{}", t4);
    }

    println!();
    println!("【口径与局限】");
    println!("  · FGI 数据自 2016 年起，2000-2002、2008 等长期熊市未被覆盖，");
    println!("    而「熊市回撤保护」恰是本策略的主要卖点，该卖点因此未经检验。");
    println!("  · 区间为美股强牛市 + 长期低利率，结论不应外推到其他 regime。");
    println!("  · 主序列与 holdout 之间存在约 3.5 个月空档（数据源限制），故分块回测。");
    println!();
    Ok(())
}

fn cmd_backtest_params() -> Result<()> {
    println!("可调参数列表:");
    println!();
    println!("  【目标仓位曲线】风险资产占总资产的目标权重(%)，逆向策略要求随情绪升高单调不增");
    println!("    target_weight.extreme_fear     极度恐慌 (默认: 85)");
    println!("    target_weight.fear             恐慌     (默认: 75)");
    println!("    target_weight.neutral          中性     (默认: 60)");
    println!("    target_weight.greed            贪婪     (默认: 45)");
    println!("    target_weight.extreme_greed    极度贪婪 (默认: 35)");
    println!();
    println!("  【再平衡】");
    println!("    rebalance.band_pp                    偏离带(百分点)，越大交易越少 (默认: 4)");
    println!("    rebalance.min_trade_amount           最小交易金额 (默认: 500)");
    println!("    rebalance.min_holding_days_for_sell  卖出最短持有天数，规避惩罚性赎回费 (默认: 30)");
    println!();
    println!("  【成本与现金】");
    println!("    costs.buy_fee_pct           买入费率% (默认: 0.12)");
    println!("    costs.sell_fee_pct          卖出固定费率% (默认: 0.05)");
    println!("    costs.cash_annual_yield     闲置现金年化% (默认: 2.0)");
    println!("    (阶梯赎回费需直接编辑 config.toml 的 costs.redemption_tiers)");
    println!();
    println!("  【情绪阈值】决定区间划分");
    println!("    thresholds.extreme_fear    极度恐慌阈值 (默认: 30)");
    println!("    thresholds.fear            恐慌阈值 (默认: 45)");
    println!("    thresholds.neutral         中性阈值 (默认: 55)");
    println!("    thresholds.greed           贪婪阈值 (默认: 70)");
    println!();
    println!("  【资产配置】风险资产内部比例，三者之和须为 100");
    println!("    allocation.us_stocks / cn_stocks / counter_cyclical");
    println!();
    println!("  【旧框架参数】仅供 `旧框架(现金比例)` 对照回测使用，已不驱动 mns report");
    println!("    buy_ratio.* / sell_ratio.* / settings.annualized_target_*");
    println!();
    println!("用法示例:");
    println!("  mns backtest                           # 四种策略对比（同一数据与成本模型）");
    println!("  mns backtest validate                  # 样本外验证 + bootstrap 分布");
    println!("  mns backtest run --config my.toml       # 使用指定配置文件");
    println!("  mns backtest run --compare a.toml,b.toml # 对比多个配置");
    println!("  mns config rebalance.band_pp 6          # 放宽偏离带以减少交易");
    Ok(())
}

async fn cmd_update_prices() -> Result<()> {
    let db = db::Database::open()?;
    let positions = db.list_positions()?;

    if positions.is_empty() {
        println!("没有资产，请先使用 'mns add' 添加资产");
        return Ok(());
    }

    println!("正在更新 {} 个资产的价格...\n", positions.len());

    let updates = quote::update_all_prices(&positions).await?;

    if updates.is_empty() {
        println!("未能更新任何资产价格");
        return Ok(());
    }

    // 更新数据库并显示结果
    println!(
        "{:<10} {:<20} {:>12} {:>12} {:>8}",
        "代码", "名称", "原价格", "新价格", "来源"
    );
    println!("{}", "-".repeat(66));

    for update in &updates {
        // 更新数据库
        db.update_price(&update.asset_code, update.new_price)?;

        let old = update
            .old_price
            .map(|p| format!("{:.4}", p))
            .unwrap_or("-".to_string());
        let display_name = if update.asset_name.chars().count() > 18 {
            update.asset_name.chars().take(18).collect()
        } else {
            update.asset_name.clone()
        };
        println!(
            "{:<10} {:<20} {:>12} {:>12} {:>8}",
            update.asset_code,
            &display_name,
            old,
            format!("{:.4}", update.new_price),
            update.source
        );
    }

    println!();
    println!("✓ 已更新 {} 个资产价格", updates.len());

    Ok(())
}

/// 市场综合概况（指数 + 恐贪指数）
async fn cmd_market() -> Result<()> {
    println!("📊 市场综合概况\n");

    // 获取指数数据
    println!("正在获取全球主要指数...");
    let indices = market::fetch_market_indices().await?;

    // 显示指数表格
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_header(vec!["指数", "价格", "涨跌", "涨跌幅"]);

    for quote in &indices {
        let change_color = if quote.change >= 0.0 {
            Color::Green
        } else {
            Color::Red
        };

        table.add_row(vec![
            Cell::new(&format!("{} {}", quote.symbol, quote.name)),
            Cell::new(&format!("{:.2}", quote.price)),
            Cell::new(&format!("{:+.2}", quote.change)).fg(change_color),
            Cell::new(&format!("{:+.2}%", quote.change_percent)).fg(change_color),
        ]);
    }

    println!("{}", table);

    // 获取恐贪指数
    println!("\n正在获取 CNN Fear & Greed Index...");
    let config = AppConfig::load()?;
    let url = &config.api.fear_greed_url;
    match sentiment::fetch_fear_greed_data(url).await {
        Ok(data) => {
            let zone = config.sentiment_zone(data.score as f64);
            println!("📊 恐贪指数: {} ({})", data.score, zone);
        }
        Err(e) => {
            println!("⚠️  获取恐贪指数失败: {}", e);
        }
    }

    Ok(())
}

/// 全球主要指数查询
async fn cmd_market_indices() -> Result<()> {
    println!("📈 全球主要指数\n");

    let indices = market::fetch_market_indices().await?;

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_header(vec!["指数", "价格", "涨跌", "涨跌幅"]);

    for quote in &indices {
        let change_color = if quote.change >= 0.0 {
            Color::Green
        } else {
            Color::Red
        };

        table.add_row(vec![
            Cell::new(&format!("{} {}", quote.symbol, quote.name)),
            Cell::new(&format!("{:.2}", quote.price)),
            Cell::new(&format!("{:+.2}", quote.change)).fg(change_color),
            Cell::new(&format!("{:+.2}%", quote.change_percent)).fg(change_color),
        ]);
    }

    println!("{}", table);

    Ok(())
}

/// 个股基础分析
async fn cmd_analyze(symbol: &str) -> Result<()> {
    println!("📊 分析: {}\n", symbol);

    // 获取报价数据
    let quote = market::fetch_stock_quote(symbol).await?;

    // 显示基础报价信息
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_header(vec!["项目", "数值"]);

    table.add_row(vec![Cell::new("股票代码"), Cell::new(&quote.symbol)]);
    table.add_row(vec![Cell::new("名称"), Cell::new(&quote.name)]);
    table.add_row(vec![Cell::new("当前价格"), Cell::new(&format!("{:.2}", quote.price))]);

    let change_color = if quote.change >= 0.0 {
        Color::Green
    } else {
        Color::Red
    };

    table.add_row(vec![
        Cell::new("涨跌"),
        Cell::new(&format!("{:+.2}", quote.change)).fg(change_color),
    ]);
    table.add_row(vec![
        Cell::new("涨跌幅"),
        Cell::new(&format!("{:+.2}%", quote.change_percent)).fg(change_color),
    ]);

    println!("{}", table);

    Ok(())
}
