mod backtest;
mod cli;
mod config;
mod db;
mod market;
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
            // 修改配置项
            config.set_value(&k, &v)?;
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

    // 策略计算（先算风险警告，再算买入建议以实现联动）
    let sell_suggestions = strategy::calculate_sell_suggestions(&config, score_f64, &positions);
    let risk_warnings = strategy::check_risk_warnings(&config, score_f64, &positions);
    let buy_suggestion = strategy::calculate_buy_suggestions(
        &config,
        score_f64,
        cash,
        &positions,
        &sell_suggestions,
        &risk_warnings,
    );

    // 生成报告
    let report = report::generate_report(
        &config,
        score_f64,
        zone,
        None,
        None,
        None,
        None,
        cash,
        &positions,
        &buy_suggestion,
        &sell_suggestions,
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
    use backtest::{
        BacktestConfig, print_comparison, print_multi_asset_comparison, run_backtest,
        run_buy_and_hold, run_custom_comparison, run_multi_asset_backtest,
        run_multi_asset_buy_and_hold, run_param_comparison,
    };

    println!("=================================================================");
    println!("   MNS 逆向投资策略回测");
    println!("=================================================================");
    println!();

    let bt_config = BacktestConfig::default();
    println!(
        "[INFO] 回测期间: {} ~ {}",
        bt_config.start_date.format("%Y-%m-%d"),
        bt_config.end_date.format("%Y-%m-%d")
    );
    println!(
        "[INFO] 初始资金: {:.0}, 年度注资: {:.0}",
        bt_config.initial_cash, bt_config.annual_inflow
    );
    println!();

    // 运行多资产回测
    println!("[INFO] 运行多资产回测（美股+红利低波+黄金）...");
    let config = AppConfig::load()?;
    let multi_result = run_multi_asset_backtest(&config, &bt_config);
    multi_result.print_report();

    // 多资产买入持有基准
    println!("[INFO] 运行多资产买入持有基准...");
    let multi_bnh_result = run_multi_asset_buy_and_hold(&bt_config);
    multi_bnh_result.print_report();

    // 打印多资产对比
    print_multi_asset_comparison(&[multi_result, multi_bnh_result]);
    println!();

    if let Some(paths) = compare {
        // 多配置对比模式
        let path_list: Vec<&str> = paths.split(',').map(|s| s.trim()).collect();
        let base_config = AppConfig::default_config();

        let results = run_custom_comparison(&base_config, &bt_config, &path_list);

        for result in &results {
            result.print_report();
        }
        print_comparison(&results);
    } else if let Some(path) = config_path {
        // 单配置模式
        let config = if std::path::Path::new(&path).exists() {
            AppConfig::load_from_path(&path)?
        } else {
            anyhow::bail!("配置文件不存在: {}", path);
        };

        let result = run_backtest(&config, &bt_config);
        result.print_report();

        // 买入持有基准
        let result_bnh = run_buy_and_hold(&bt_config);
        print_comparison(&[result, result_bnh]);
    } else {
        // 默认配置 + 参数对比模式
        let config = AppConfig::load()?;
        let results = run_param_comparison(&config, &bt_config);

        for result in &results {
            result.print_report();
        }
        print_comparison(&results);
    }

    Ok(())
}

fn cmd_backtest_params() -> Result<()> {
    println!("可调参数列表:");
    println!();
    println!("  【阈值参数】");
    println!("    thresholds.extreme_fear    极度恐慌阈值 (默认: 30)");
    println!("    thresholds.fear            恐慌阈值 (默认: 45)");
    println!("    thresholds.neutral         中性阈值 (默认: 55)");
    println!("    thresholds.greed           贪婪阈值 (默认: 70)");
    println!();
    println!("  【买入比例】");
    println!("    buy_ratio.extreme_fear     极度恐慌买入比例 (默认: 60%)");
    println!("    buy_ratio.fear             恐慌买入比例 (默认: 35%)");
    println!("    buy_ratio.neutral          中性买入比例 (默认: 0%)");
    println!("    buy_ratio.greed            贪婪买入比例 (默认: 0%)");
    println!();
    println!("  【卖出比例】");
    println!("    sell_ratio.extreme_greed_target_high   极度贪婪+高年化卖出 (默认: 50%)");
    println!("    sell_ratio.extreme_greed_target_low    极度贪婪+中年化卖出 (默认: 30%)");
    println!("    sell_ratio.extreme_greed_below_target  极度贪婪+低年化卖出 (默认: 20%)");
    println!("    sell_ratio.greed_target_high           贪婪+高年化卖出 (默认: 40%)");
    println!("    sell_ratio.greed_target_low            贪婪+低年化卖出 (默认: 25%)");
    println!("    sell_ratio.neutral_target_high         中性+高年化卖出 (默认: 15%)");
    println!();
    println!("  【其他参数】");
    println!("    settings.annualized_target_low   低止盈线 (默认: 10%)");
    println!("    settings.annualized_target_high  高止盈线 (默认: 15%)");
    println!("    settings.min_holding_days        最小持仓天数 (默认: 45)");
    println!("    settings.max_contrarian_weight   最大逆向权重 (默认: 2.0)");
    println!();
    println!("用法示例:");
    println!("  mns backtest                           # 运行默认参数对比");
    println!("  mns backtest --config my_config.toml   # 使用指定配置文件");
    println!("  mns backtest --compare a.toml,b.toml    # 对比多个配置");
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
