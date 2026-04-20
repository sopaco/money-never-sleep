mod cli;
mod config;
mod db;
mod models;
mod report;
mod sentiment;
mod strategy;

use anyhow::Result;
use clap::Parser;
use cli::{CashAction, Commands};
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
        Commands::Sentiment => cmd_sentiment().await?,
        Commands::Report => cmd_report().await?,
        Commands::History { limit } => cmd_history(limit)?,
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

async fn cmd_sentiment() -> Result<()> {
    let config = AppConfig::load()?;
    println!("正在获取 CNN 恐贪指数...");
    let data = sentiment::fetch_fear_greed(&config).await?;

    let zone = config.sentiment_zone(data.fear_and_greed.score);
    println!("CNN 恐贪指数: {:.2} ({})", data.fear_and_greed.score, zone);
    if let Some(pc) = data.fear_and_greed.previous_close {
        println!("前日收盘: {:.2}", pc);
    }
    if let Some(pw) = data.fear_and_greed.previous_1_week {
        println!("周环比: {:.2} → {:.2}", pw, data.fear_and_greed.score);
    }
    if let Some(pm) = data.fear_and_greed.previous_1_month {
        println!("月环比: {:.2} → {:.2}", pm, data.fear_and_greed.score);
    }
    if let Some(py) = data.fear_and_greed.previous_1_year {
        println!("年同比: {:.2} → {:.2}", py, data.fear_and_greed.score);
    }

    // 保存快照
    let db = db::Database::open()?;
    db.save_fear_greed_snapshot(
        data.fear_and_greed.score,
        zone,
        data.fear_and_greed.previous_close,
        data.fear_and_greed.previous_1_week,
        data.fear_and_greed.previous_1_month,
        data.fear_and_greed.previous_1_year,
    )?;

    Ok(())
}

async fn cmd_report() -> Result<()> {
    let config = AppConfig::load()?;
    let db = db::Database::open()?;

    println!("正在获取 CNN 恐贪指数...");
    let data = sentiment::fetch_fear_greed(&config).await?;
    let score = data.fear_and_greed.score;
    let rating = config.sentiment_zone(score);

    // 保存快照
    db.save_fear_greed_snapshot(
        score,
        rating,
        data.fear_and_greed.previous_close,
        data.fear_and_greed.previous_1_week,
        data.fear_and_greed.previous_1_month,
        data.fear_and_greed.previous_1_year,
    )?;

    let cash = db.get_cash_balance()?;
    let positions = db.list_positions()?;

    // 策略计算（先算风险警告，再算买入建议以实现联动）
    let sell_suggestions = strategy::calculate_sell_suggestions(&config, score, &positions);
    let risk_warnings = strategy::check_risk_warnings(&config, score, &positions);
    let buy_suggestion = strategy::calculate_buy_suggestions(
        &config,
        score,
        cash,
        &positions,
        &sell_suggestions,
        &risk_warnings,
    );

    // 生成报告
    let report = report::generate_report(
        &config,
        score,
        rating,
        data.fear_and_greed.previous_close,
        data.fear_and_greed.previous_1_week,
        data.fear_and_greed.previous_1_month,
        data.fear_and_greed.previous_1_year,
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
