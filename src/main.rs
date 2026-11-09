// The engine is a library so the GUI can call it directly instead of parsing
// CLI output. This binary keeps only argument parsing and terminal rendering.
#[allow(unused_imports)]
use money_never_sleep::{
    audit, backtest, cli, config, db, guardrail, live_data, market, metrics, models, quote, report,
    sentiment, signal, strategy,
};

use anyhow::{Context, Result};
use clap::Parser;
use cli::{BacktestAction, CashAction, Commands, ConfigAction};
use comfy_table::{Cell, Color, Table, modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL};
use config::AppConfig;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::Cli::parse();

    match cli.command {
        Commands::Init { force } => cmd_init(force)?,
        Commands::Config { action, key, value } => match action {
            None => cmd_config(key, value)?,
            Some(ConfigAction::Params) => cmd_config_params()?,
        },
        Commands::Cash { action } => match action {
            None => cmd_cash()?,
            Some(CashAction::Set { amount }) => cmd_cash_set(amount)?,
            Some(CashAction::Deposit { amount, note }) => cmd_cash_deposit(amount, note)?,
            Some(CashAction::Withdraw { amount, note }) => cmd_cash_withdraw(amount, note)?,
        },
        Commands::Portfolio => cmd_portfolio()?,
        Commands::Performance => cmd_performance()?,
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
        Commands::Report { decide } => cmd_report(decide).await?,
        Commands::Today { decide, quiet } => cmd_today(decide, quiet).await?,
        Commands::Plan { monthly } => cmd_plan(monthly)?,
        Commands::Guard => cmd_guard()?,
        Commands::History { limit } => cmd_history(limit)?,
        Commands::Backtest {
            action,
            config,
            compare,
        } => match action {
            None => cmd_backtest_audit(config)?,
            Some(BacktestAction::Mirror) => cmd_backtest_mirror(config)?,
            Some(BacktestAction::Asset { dataset }) => cmd_backtest_asset(config, &dataset)?,
            Some(BacktestAction::UpdateData) => cmd_backtest_update_data()?,
            Some(BacktestAction::ValidateFull { iterations }) => {
                cmd_backtest_validate_full(config, iterations)?
            }
            Some(BacktestAction::Legacy) => cmd_backtest_legacy(config, compare)?,
            Some(BacktestAction::Validate { iterations, block }) => {
                cmd_backtest_validate(iterations, block)?
            }
        },
        Commands::UpdatePrices => cmd_update_prices().await?,
        Commands::Market => cmd_market().await?,
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

    let mut config = AppConfig::default_config();
    config.apply_profile(config::RiskProfile::Balanced);
    config.save()?;

    let db = db::Database::open()?;
    drop(db);

    // 创建报告输出目录
    let report_dir = config.report_dir();
    std::fs::create_dir_all(&report_dir)?;

    println!("✓ 初始化完成");
    println!("  配置文件: {}", config_path.display());
    println!("  数据库:   {}", db_path.display());
    println!("  报告目录: {}", report_dir.display());
    println!(
        "  风险档位: {}（中性目标 {:.0}%，用 `mns config profile <none|steady|balanced|aggressive>` 切换）",
        config.profile_label(),
        config.effective_neutral_weight()
    );
    println!(
        "  档位语义: 档位抬高常驻仓位并收窄择时摆幅（不降低风险）；追求最小回撤可选 none（曲线原样）"
    );
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
            let old_config = config.clone();
            config.set_value(&k, &v)?;
            config.validate()?;
            config.save()?;
            println!("✓ {} = {}", k, v);
            // 档位类改动会立即重锚目标仓位——切换前预告将引发的调仓规模
            // （带宽/平滑/确认管不到这个盲区），见 P1-5。
            if k == "profile" || k == "profile.core_blend" {
                print_profile_switch_preview(&old_config, &config);
            }
        }
        (None, Some(_)) => unreachable!(),
    }
    Ok(())
}

/// `mns config profile` 切换后的调仓影响预告（W5）。
///
/// 用最新本地恐贪快照算决策分数，对比新旧档位的目标仓位与当前实际权重，
/// 预告"下次决策日预计需要买/卖多少"。本地无数据时降级为一句说明，
/// 不阻塞配置修改本身。
fn print_profile_switch_preview(old: &AppConfig, new: &AppConfig) {
    let attempted = (|| -> Result<()> {
        let db = db::Database::open()?;
        let history = db.recent_fear_greed_scores(90)?;
        let Some((&(snap_date, latest), _)) = history.split_last() else {
            println!("  （本地无恐贪快照，跳过调仓影响预告——运行一次 mns report 后可得）");
            return Ok(());
        };
        // split_last 的 rest 是切片；signal::compute 需要 Vec（不含"今天"的升序历史）
        let rest: money_never_sleep::signal::ScoreHistory = history[..history.len() - 1].to_vec();
        let today = chrono::Local::now().date_naive();
        let signal = signal::compute(new, latest, &rest, today);
        let cash = db.get_cash_balance()?;
        let positions = db.list_positions()?;
        let impact =
            strategy::profile_switch_impact(old, new, signal.effective_score, cash, &positions);

        if impact.current_risk <= 0.0 && cash <= 0.0 {
            println!("  （账户未注资，跳过调仓影响预告）");
            return Ok(());
        }
        println!(
            "  ▸ 档位切换预告（快照 {}，决策分数 {:.1}）:",
            snap_date.format("%Y-%m-%d"),
            signal.effective_score
        );
        println!(
            "    目标风险仓位: {:.1}% → {:.1}%（当前实际 {:.1}%）",
            impact.old_target, impact.new_target, impact.current_risk
        );
        if impact.in_band {
            println!(
                "    下次决策日预计无需调仓（偏离 {:+.1}pp 在带宽 ±{:.1}pp 内）",
                impact.drift_pp, new.rebalance.band_pp
            );
        } else {
            println!(
                "    下次决策日预计需{}约 ¥{:.0}（偏离 {:+.1}pp；实际执行还受各腿带宽/最短持有期约束）",
                impact.direction, impact.est_amount, impact.drift_pp
            );
        }
        Ok(())
    })();
    if let Err(e) = attempted {
        println!("  （调仓影响预告不可用: {}）", e);
    }
}

fn cmd_cash() -> Result<()> {
    let db = db::Database::open()?;
    let balance = db.get_cash_balance()?;
    println!("现金余额: ¥{:.2}", balance);

    // C1：余额 + 最近 5 条外部流水，让"钱从哪来"一眼可查
    let flows = db.list_cash_flows(5)?;
    if flows.is_empty() {
        println!("（暂无流水；外部资金进出用 mns cash deposit / withdraw）");
    } else {
        println!("\n最近流水:");
        let mut table = Table::new();
        table
            .load_preset(UTF8_FULL)
            .apply_modifier(UTF8_ROUND_CORNERS);
        table.set_header(vec![
            Cell::new("日期"),
            Cell::new("类型"),
            Cell::new("金额"),
            Cell::new("备注"),
        ]);
        for f in &flows {
            table.add_row(vec![
                Cell::new(f.flow_date.format("%Y-%m-%d").to_string()),
                Cell::new(flow_kind_cn(&f.kind)),
                Cell::new(format!("{:+.2}", f.amount)),
                Cell::new(f.note.clone().unwrap_or_default()),
            ]);
        }
        for line in table.to_string().lines() {
            println!("  {}", line);
        }
    }
    Ok(())
}

/// 台账类型 → 中文（展示用）。
fn flow_kind_cn(kind: &str) -> &'static str {
    match kind {
        "deposit" => "入金",
        "withdraw" => "出金",
        "opening" => "期初",
        "adjust" => "调整",
        _ => "未知",
    }
}

fn cmd_cash_set(amount: f64) -> Result<()> {
    if amount < 0.0 {
        anyhow::bail!("现金余额不能为负数: {}", amount);
    }
    let db = db::Database::open()?;
    let old = db.get_cash_balance()?;
    db.set_cash_balance(amount)?;
    // C1：set 是纠错工具，但每次纠错都记 adjust 流水（带 原值→新值），
    // 保证余额历史可以从台账重建；无变化则不记（避免噪声）
    let delta = amount - old;
    if delta.abs() >= 0.005 {
        let today = chrono::Local::now().date_naive();
        db.add_cash_flow(
            today,
            delta,
            "adjust",
            Some(&format!("余额纠错: ¥{:.2} → ¥{:.2}", old, amount)),
        )?;
        println!(
            "✓ 现金余额已设置为: ¥{:.2}（已记调整流水 {:+.2}）",
            amount, delta
        );
    } else {
        println!("✓ 现金余额已设置为: ¥{:.2}", amount);
    }
    Ok(())
}

/// C1：外部入金——资金进入组合的主通道（XIRR 口径的资金流入）。
fn cmd_cash_deposit(amount: f64, note: Option<String>) -> Result<()> {
    if amount <= 0.0 {
        anyhow::bail!("入金金额必须为正数: {}", amount);
    }
    let db = db::Database::open()?;
    let new_balance = db.add_cash(amount)?;
    let today = chrono::Local::now().date_naive();
    db.add_cash_flow(today, amount, "deposit", note.as_deref())?;
    println!("✓ 入金 ¥{:.2}，当前余额: ¥{:.2}", amount, new_balance);
    Ok(())
}

/// C1：外部出金——资金离开组合（XIRR 口径的资金流出）。
fn cmd_cash_withdraw(amount: f64, note: Option<String>) -> Result<()> {
    if amount <= 0.0 {
        anyhow::bail!("出金金额必须为正数: {}", amount);
    }
    let db = db::Database::open()?;
    let balance = db.get_cash_balance()?;
    if amount > balance + 0.005 {
        anyhow::bail!(
            "出金超过现金余额（当前 ¥{:.2}，要取出 ¥{:.2}）。若资金在持仓里，\
             请先用 mns sell 落账再取",
            balance,
            amount
        );
    }
    db.set_cash_balance(balance - amount)?;
    let today = chrono::Local::now().date_naive();
    db.add_cash_flow(today, -amount, "withdraw", note.as_deref())?;
    println!("✓ 出金 ¥{:.2}，当前余额: ¥{:.2}", amount, balance - amount);
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

/// C2 组合仪表盘：只读性能视图，零写入（常用路径保持轻量，此为单独入口）。
///
/// 章节与数据源：
/// 1. 组合 XIRR —— cash_flows 全部外部流水 + 当日终值（金额加权，资金
///    何时到账才算数）；metrics::xirr 本就支持不规则日期流，只需把台账
///    的「入金为正」翻转成 XIRR 的「投入为负」。
/// 2. 实际最大回撤 —— portfolio_snapshots 的 total_value 日序列（C3 积累）。
/// 3. 对照策略预期 —— 参考值现场计算（当前配置 × 内置数据集，单引擎秒级），
///    不硬编码回测数字——数字会过期（与 W1 同一决策）。
/// 4. 资产曲线摘要 —— 快照序列的期初/当前/峰值/谷值。
/// 5. 趋势锚就绪进度（C4）—— price_history 每腿最长净值历史天数。
fn cmd_performance() -> Result<()> {
    use backtest::{BacktestConfig, Engine, SignalConfig};

    let db = db::Database::open()?;
    let config = AppConfig::load()?;
    let positions = db.list_positions()?;
    let cash = db.get_cash_balance()?;
    let today = chrono::Local::now().date_naive();
    // 终值与账户概览同口径（无现价时退回成本）
    let holdings_value: f64 = positions
        .iter()
        .map(|p| p.market_value_or_cost())
        .sum::<f64>()
        + 0.0;
    let terminal = cash + holdings_value;

    println!("═════════════════════════════════════════════════");
    println!(" 组合绩效仪表盘（只读） {}", today.format("%Y-%m-%d"));
    println!("═════════════════════════════════════════════════");

    // ── 1. 组合 XIRR ─────────────────────────────
    println!("\n【组合 XIRR（金额加权）】");
    let flows = db.all_cash_flows()?;
    let ledger: Vec<(chrono::NaiveDate, f64)> =
        flows.iter().map(|f| (f.flow_date, f.amount)).collect();
    let net_inflow: f64 = flows.iter().map(|f| f.amount).sum::<f64>() + 0.0;
    let flow_days = flows
        .first()
        .map(|f| (today - f.flow_date).num_days())
        .unwrap_or(0);
    if flows.is_empty() {
        println!("  暂无台账记录");
    } else {
        println!(
            "  外部净流入: ¥{:.2}（{} 笔，自 {} 起，追踪 {} 天）",
            net_inflow,
            flows.len(),
            flows.first().unwrap().flow_date.format("%Y-%m-%d"),
            flow_days
        );
        println!("  当前总资产: ¥{:.2}", terminal);
        let xirr_flows = metrics::xirr_flows_from_ledger(&ledger, terminal, today);
        match metrics::xirr(&xirr_flows) {
            Some(x) => {
                println!("  XIRR: {:+.2}%", x * 100.0);
                if flow_days < 30 {
                    println!("  （追踪 {} 天，样本过短，仅供参考）", flow_days);
                }
            }
            None => println!("  XIRR: 暂无法计算（无符号相反的现金流——余额与市值均为 0？）"),
        }
    }

    // ── 2. 实际最大回撤 ─────────────────────────
    println!("\n【实际最大回撤】（基于每日总资产快照）");
    let series = db.portfolio_snapshot_series()?;
    let curve_days = series
        .first()
        .map(|(d, _)| (today - *d).num_days())
        .unwrap_or(0);
    let actual_dd = metrics::drawdown_summary(&series);
    match (&actual_dd, series.last()) {
        (Some(s), Some(&(last_date, last_value))) => {
            println!(
                "  最大回撤: {:.2}%（峰值 ¥{:.2} @ {} → 谷值 ¥{:.2} @ {}）",
                s.max_dd * 100.0,
                s.peak_value,
                s.peak_date.format("%Y-%m-%d"),
                s.trough_value,
                s.trough_date.format("%Y-%m-%d")
            );
            if last_value < s.all_time_peak && s.all_time_peak > 0.0 {
                println!(
                    "  当前距峰值: {:.2}%（全期峰值 ¥{:.2} @ {}，当前 ¥{:.2} @ {}）",
                    (s.all_time_peak - last_value) / s.all_time_peak * 100.0,
                    s.all_time_peak,
                    s.all_time_peak_date.format("%Y-%m-%d"),
                    last_value,
                    last_date.format("%Y-%m-%d")
                );
            } else {
                println!("  当前即全期峰值（¥{:.2}）", last_value);
            }
            println!(
                "  （{} 个数据点，自 {} 起积累）",
                series.len(),
                series.first().unwrap().0.format("%Y-%m-%d")
            );
        }
        _ => println!("  暂无足够数据点（需 ≥2 天快照；mns report / mns today 每日积累）"),
    }

    // ── 3. 对照策略预期 ─────────────────────────
    // 参考值现场计算：单引擎 + 当前配置（与实盘一致的情绪锚 SignalConfig::default），
    // 不跑对比矩阵。回测口径：初始 10 万 + 年注资 5 万（与 mns backtest 相同）。
    println!("\n【对照策略预期】（参考值现场计算，非硬编码）");
    let rows = backtest::load_main();
    if rows.is_empty() {
        println!("  回测数据集为空，无法生成参考值");
    } else {
        let bt = BacktestConfig::spanning(&rows, 100_000.0, 50_000.0);
        let bt_result = backtest::run(
            &config,
            &bt,
            &SignalConfig::default(),
            Engine::TargetWeight,
            &rows,
        );
        let dd_str = match &actual_dd {
            Some(s) => format!("{:.2}%", s.max_dd * 100.0),
            None => "暂无数据".to_string(),
        };
        let xirr_str = metrics::xirr(&metrics::xirr_flows_from_ledger(&ledger, terminal, today))
            .map(|x| format!("{:+.2}%", x * 100.0))
            .unwrap_or_else(|| "暂无法计算".to_string());
        println!(
            "  回测口径: {} ~ {}（当前配置，情绪锚目标仓位引擎）",
            bt.start_date.format("%Y-%m"),
            bt.end_date.format("%Y-%m")
        );
        println!(
            "  实际最大回撤 {} vs 回测 {:.2}%",
            dd_str,
            bt_result.max_drawdown * 100.0
        );
        println!(
            "  实际 XIRR    {} vs 回测 {:.2}%",
            xirr_str,
            bt_result.xirr * 100.0
        );
        if curve_days >= 90 {
            match &actual_dd {
                Some(s) if s.max_dd <= bt_result.max_drawdown + 1e-9 => {
                    println!("  判定: 在策略预期内（实际回撤未超回测口径）")
                }
                Some(_) => {
                    println!("  判定: 偏离预期——实际回撤超过回测口径，建议核对执行纪律与数据记录")
                }
                None => println!("  判定: 数据不足（无快照序列）"),
            }
        } else {
            println!(
                "  （追踪 {} 天 < 90 天，对照仅供参考，不做判定）",
                curve_days
            );
        }
        println!("  （回测为历史区间模拟，实际环境可能不同；不做统计显著性声明）");
    }

    // ── 4. 资产曲线摘要 ─────────────────────────
    println!("\n【资产曲线摘要】");
    if series.is_empty() {
        println!("  暂无快照（mns report / mns today 每日积累）");
    } else {
        let first = series.first().unwrap();
        let last = series.last().unwrap();
        let mut peak = series[0];
        let mut trough = series[0];
        for &p in &series {
            if p.1 > peak.1 {
                peak = p;
            }
            if p.1 < trough.1 {
                trough = p;
            }
        }
        println!(
            "  期初: ¥{:.2}（{}） → 当前: ¥{:.2}（{}）",
            first.1,
            first.0.format("%Y-%m-%d"),
            last.1,
            last.0.format("%Y-%m-%d")
        );
        println!(
            "  峰值: ¥{:.2}（{}） · 谷值: ¥{:.2}（{}）",
            peak.1,
            peak.0.format("%Y-%m-%d"),
            trough.1,
            trough.0.format("%Y-%m-%d")
        );
    }

    // ── 5. 趋势锚就绪进度（C4）───────────────────
    // 每腿取该腿内历史最长的净值天数；达标不自动切换信号锚——切换是独立的
    // 产品决策（需先对照验证），此处只积累 + 报告。
    println!("\n【趋势锚就绪进度】");
    let legs = [
        ("us_stocks", "美股"),
        ("cn_stocks", "A股"),
        ("counter_cyclical", "逆周期(黄金)"),
    ];
    let mut all_ready = true;
    for (cat, name) in legs {
        let members: Vec<&money_never_sleep::models::Position> =
            positions.iter().filter(|p| p.category == cat).collect();
        if members.is_empty() {
            println!("  · {}: —（无持仓）", name);
            all_ready = false;
            continue;
        }
        // 腿级聚合：取该腿内 span 最长的成员
        let mut best_days: i64 = -1;
        let mut best_start: Option<chrono::NaiveDate> = None;
        for p in &members {
            if let Some((lo, hi)) = db.price_history_span(&p.asset_code)? {
                let days = (hi - lo).num_days();
                if days > best_days {
                    best_days = days;
                    best_start = Some(lo);
                }
            }
        }
        match best_start {
            Some(lo) => {
                let days = best_days.max(0);
                println!(
                    "  · {}: {}/365 天（{} 起）",
                    name,
                    days,
                    lo.format("%Y-%m-%d")
                );
                if days < 365 {
                    all_ready = false;
                }
            }
            None => {
                println!("  · {}: 0/365 天（无本地价格记录）", name);
                all_ready = false;
            }
        }
    }
    if all_ready {
        println!("  ✓ 已就绪，等待产品决策启用（切换信号锚前需先对照验证）");
    } else {
        println!("  （自 C4 上线日起积累，不回填历史）");
    }

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

/// 汇总本月已发生的交易笔数与按现行费率估算的费用。
///
/// 卖出费按 30 天持有期估算（实际可能更低或更高），只用于报告量级参考。
fn month_activity(db: &db::Database, config: &AppConfig) -> Result<report::MonthActivity> {
    let txs = db.list_transactions(500)?;
    let now = chrono::Local::now();
    let prefix = now.format("%Y-%m").to_string();
    let year_prefix = now.format("%Y").to_string();
    let mut activity = report::MonthActivity::default();
    for tx in txs.iter().filter(|t| t.tx_date.starts_with(&prefix)) {
        activity.trades += 1;
        let amount = tx.amount.abs();
        activity.estimated_fees += if tx.tx_type == "buy" {
            amount * config.costs.buy_rate()
        } else {
            amount * config.costs.total_sell_rate(30)
        };
    }
    // 年度交易预算披露：把日频信号压缩为可执行建议，避免逐日跟单突破 10-20 次/年
    activity.year_trades = txs
        .iter()
        .filter(|t| t.tx_date.starts_with(&year_prefix))
        .count();
    Ok(activity)
}

async fn cmd_report(force_decide: bool) -> Result<()> {
    // 与 `mns today` 同口径的价格时效：report 本身不刷价，直接读库里的 updated_at
    let db = db::Database::open()?;
    let age = price_age_days(&db.list_positions()?);
    cmd_report_inner(force_decide, false, age).await
}

/// 一键：先刷新价格（失败也继续），再出报告。
async fn cmd_today(decide: bool, quiet: bool) -> Result<()> {
    let db = db::Database::open()?;
    let positions = db.list_positions()?;
    let mut update_err: Option<String> = None;

    if positions.is_empty() {
        if !quiet {
            println!("没有资产，报告将只包含现金。");
        }
    } else {
        if !quiet {
            println!("正在更新 {} 个资产的价格...", positions.len());
        }
        match quote::update_all_prices(&positions).await {
            Ok(updates) => {
                if updates.is_empty() {
                    update_err = Some("未获取到任何资产的最新价格".to_string());
                }
                for u in &updates {
                    if let Err(e) = db.update_price(&u.asset_code, u.new_price) {
                        update_err = Some(format!("{} 写库失败: {}", u.asset_code, e));
                    }
                }
                if !quiet {
                    println!("✓ 已更新 {} 个资产价格", updates.len());
                }
            }
            Err(e) => update_err = Some(e.to_string()),
        }
    }

    if let Some(err) = &update_err {
        println!("⚠ 价格更新失败：{}（继续使用本地旧价）", err);
    }

    let age = price_age_days(&db.list_positions()?);
    cmd_report_inner(decide, quiet, age).await
}

/// 本地价格距今最旧的天数（无持仓返回 None）。
fn price_age_days(positions: &[models::Position]) -> Option<i64> {
    let now = chrono::Local::now().naive_local();
    positions
        .iter()
        .filter_map(|p| {
            chrono::NaiveDateTime::parse_from_str(&p.updated_at, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|dt| (now - dt).num_days().max(0))
        })
        .max()
}

/// 降级快照的可用期（天）。与 `signal_smooth_days = 7` 同语义：一周内的
/// 恐贪分数对月度级调仓决策仍具代表性；超过即视为失效数据，不得驱动决策。
const DEGRADED_SNAPSHOT_MAX_AGE_DAYS: i64 = 7;

async fn cmd_report_inner(
    force_decide: bool,
    quiet: bool,
    price_age_days: Option<i64>,
) -> Result<()> {
    let config = AppConfig::load()?;
    let db = db::Database::open()?;

    if !quiet {
        println!("正在获取恐贪指数...");
    }

    // 使用配置的 API URL；获取失败时降级到本地最新快照（W4）：
    // CNN 418 反爬是文档已知的常态风险，不能让整个报告随之失败——
    // 最需要看报告的日子往往正是数据源最不稳定的日子。
    let url = &config.api.fear_greed_url;
    let today = chrono::Local::now().date_naive();
    // 降级状态：(快照距今天数, 快照日期)。None = 数据新鲜。
    let mut degraded: Option<(i64, chrono::NaiveDate)> = None;
    let data = match sentiment::fetch_fear_greed_data(url).await {
        Ok(d) => d,
        Err(err) => {
            let snap = db.latest_fear_greed_snapshot()?.ok_or_else(|| {
                anyhow::anyhow!(
                    "恐贪指数获取失败（{}），且本地无历史快照可回退——\
                         请检查网络后重试",
                    err
                )
            })?;
            degraded = Some(((today - snap.date).num_days(), snap.date));
            sentiment::FearGreedData {
                score: snap.score.round().clamp(0.0, 100.0) as u8,
                previous_close: snap.previous_close,
                previous_1_week: snap.previous_1_week,
                previous_1_month: snap.previous_1_month,
                previous_1_year: snap.previous_1_year,
            }
        }
    };
    // W4 分级降级（7 天分界，产品 2026-11-08 拍板）：
    // - 快照年龄 ≤ 7 天：数据仍有代表性，决策流照常——决策日照常出决策版
    //   （顶部带警告）；--decide 亦允许；
    // - 快照年龄 > 7 天：数据失效，强制只出观察版；--decide 拒绝执行并明确
    //   报错——一个月前的恐贪分数不该驱动真实调仓。
    let snapshot_expired = degraded
        .map(|(age, _)| age > DEGRADED_SNAPSHOT_MAX_AGE_DAYS)
        .unwrap_or(false);
    if force_decide && snapshot_expired {
        let (age, snap_date) = degraded.expect("snapshot_expired 蕴含 degraded 为 Some");
        anyhow::bail!(
            "恐贪指数获取失败，且本地快照已过期（{} 的数据，距今 {} 天 > {} 天上限）：\
             --decide 已拒绝，失效数据不驱动买卖建议。\
             请先修复网络后重跑；不带 --decide 仍会输出观察报告",
            snap_date.format("%Y-%m-%d"),
            age,
            DEGRADED_SNAPSHOT_MAX_AGE_DAYS
        );
    }
    let today_score = data.score as f64;
    let today_zone = config.sentiment_zone(today_score);
    if degraded.is_none() {
        // 只在拿到新鲜数据时落库；降级数据若落在今天之前的日期，
        // 写入会把旧数据误标为今天，污染历史
        db.save_fear_greed_snapshot(
            today_score,
            today_zone,
            data.previous_close,
            data.previous_1_week,
            data.previous_1_month,
            data.previous_1_year,
        )?;
    }

    // 决策信号：平滑 + 区间确认 + 决策日过滤（详见 src/signal.rs）；
    // 当日原始值仅展示，不驱动仓位，避免周级噪声引起操作干扰
    let mut history = db.recent_fear_greed_scores(90)?;
    if degraded.is_some() {
        // 降级时最新快照将作为"今天"的替身传入 compute；
        // 先从历史里摘出，避免同一天被计两次
        history.pop();
    }
    let mut signal = signal::compute(&config, today_score, &history, today);

    // 降级说明：按快照年龄分级措辞，写进信号说明与报告横幅
    let degraded_banner = degraded.map(|(age, snap_date)| {
        let snap_note = if snapshot_expired {
            format!(
                "恐贪指数获取失败，本地快照已过期（{} 天前，{} 的数据，超过 {} 天可用期）；\
                 今日强制只出观察报告，不产生买卖建议；--decide 在数据过期时不可用，\
                 请先修复网络后重跑",
                age,
                snap_date.format("%Y-%m-%d"),
                DEGRADED_SNAPSHOT_MAX_AGE_DAYS
            )
        } else {
            format!(
                "恐贪指数获取失败，已回退本地快照（{} 天前，{} 的数据，在 {} 天可用期内）；\
                 决策流程照常，请留意情绪数据滞后",
                age,
                snap_date.format("%Y-%m-%d"),
                DEGRADED_SNAPSHOT_MAX_AGE_DAYS
            )
        };
        signal.note.push_str(&format!("；{}", snap_note));
        snap_note
    });

    // 控制台与报告体同口径：--decide 强制决策时明确标注，
    // 避免控制台"信号说明"仍写'只出观察报告'而随后却出了决策报告
    let forced_decide = force_decide && !signal.decision_day;
    let display_note = if forced_decide {
        format!("{}（--decide 强制决策）", signal.note)
    } else {
        signal.note.clone()
    };
    if !quiet {
        println!(
            "恐贪指数: {} ({}) | 决策分数: {} ({})",
            data.score, today_zone, signal.effective_score, signal.effective_zone
        );
        println!("信号说明: {}", display_note);
        println!(
            "今日报告类型: {} 平滑 (样本 {}/{})",
            signal.report_kind, signal.samples_used, config.rebalance.signal_smooth_days
        );
    }

    let cash = db.get_cash_balance()?;
    let positions = db.list_positions()?;

    // C3 每日总资产快照：当日终态（同日重跑覆盖），实际回撤/收益曲线的数据源。
    // fear_score 记当日决策分数（降级时即降级所用的分数）；字段与账户概览同口径。
    let holdings_value: f64 = positions
        .iter()
        .map(|p| p.market_value_or_cost())
        .sum::<f64>()
        + 0.0;
    db.upsert_portfolio_snapshot(
        today,
        cash,
        holdings_value,
        cash + holdings_value,
        Some(signal.effective_score),
    )?;
    let tracking_since = db.portfolio_tracking_since()?;

    // 快照过期：无论是否决策日，一律观察版（force_decide 组合已在上方被拒绝）；
    // 其余情况（新鲜数据，或 ≤7 天的可用降级）走正常决策流
    let decide_today = !snapshot_expired && (signal.decision_day || force_decide);

    let report = if decide_today {
        // 策略计算：目标仓位 + 偏离带（与回测引擎同源），用决策分数而非当日值
        let decide_note = display_note;
        let risk_warnings =
            strategy::check_risk_warnings(&config, signal.effective_score, &positions);
        let plan = strategy::calculate_rebalance_plan(
            &config,
            signal.effective_score,
            cash,
            &positions,
            today,
        );
        let activity = month_activity(&db, &config)?;

        report::generate_report(
            &config,
            signal.effective_score,
            &signal.effective_zone,
            signal.raw_score,
            &signal.raw_zone,
            &decide_note,
            data.previous_close,
            data.previous_1_week,
            data.previous_1_month,
            data.previous_1_year,
            cash,
            &positions,
            &plan,
            &risk_warnings,
            &activity,
            price_age_days,
            tracking_since,
        )?
    } else {
        // 非决策日：只出观察报告，不给买卖建议
        report::generate_observation_report(
            &config,
            &signal,
            cash,
            &positions,
            price_age_days,
            tracking_since,
        )?
    };

    // 降级横幅置顶：陈旧数据警告必须在报告第一眼可见的位置
    let report = match &degraded_banner {
        Some(note) => format!("⚠ {}\n\n{}", note, report),
        None => report,
    };

    let filepath = report::save_report(&config, &report)?;
    println!("{}", report);
    println!("\n报告已保存至: {}", filepath);
    Ok(())
}

fn cmd_plan(monthly_arg: Option<f64>) -> Result<()> {
    let config = AppConfig::load()?;
    let db = db::Database::open()?;

    let monthly = monthly_arg.unwrap_or(config.cashflow.monthly);
    if monthly <= 0.0 {
        anyhow::bail!(
            "未设置月投入金额：用 `mns plan --monthly 3000` 或 `mns config cashflow.monthly 3000` 设置"
        );
    }

    let history = db.recent_fear_greed_scores(90)?;
    let latest = history
        .last()
        .map(|(_, s)| *s)
        .ok_or_else(|| anyhow::anyhow!("本地没有恐贪快照，请先运行一次 `mns report`"))?;
    let today = chrono::Local::now().date_naive();
    let signal = signal::compute(&config, latest, &history, today);
    let score = signal.effective_score;

    let cash = db.get_cash_balance()?;
    let positions = db.list_positions()?;
    let plan = strategy::calculate_monthly_plan(&config, score, cash, &positions, monthly);

    println!("=================================================================");
    println!("   定投计划：每月 ¥{:.0}（只买不卖）", monthly);
    println!("=================================================================");
    println!();
    println!(
        "风险档位: {} | 决策分数 {:.1} | 目标风险仓位 {:.0}%（含本月新钱）",
        config.profile_label(),
        score,
        plan.target_risk_weight
    );
    println!();

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS);
    table.set_header(vec![
        Cell::new("腿"),
        Cell::new("目标权重"),
        Cell::new("当前权重"),
        Cell::new("偏离"),
        Cell::new("本月买入"),
    ]);
    for leg in &plan.legs {
        table.add_row(vec![
            Cell::new(&leg.category_cn),
            Cell::new(format!("{:.0}%", leg.target_weight)),
            Cell::new(format!("{:.0}%", leg.current_weight)),
            Cell::new(format!("{:+.1}pp", leg.drift_pp)),
            Cell::new(format!("¥{:.0}", leg.amount)),
        ]);
    }
    table.add_row(vec![
        Cell::new("合计"),
        Cell::new(""),
        Cell::new(""),
        Cell::new(""),
        Cell::new(format!("¥{:.0}", plan.total_buy)),
    ]);
    println!("{}", table);
    println!();

    if plan.legs.iter().all(|l| l.items.is_empty()) {
        println!("持仓池为空，先用 `mns add <代码> <名称> <类别>` 添加标的。");
        return Ok(());
    }

    println!("标的分配:");
    for leg in &plan.legs {
        if leg.items.is_empty() {
            continue;
        }
        println!("  {}（¥{:.0}）", leg.category_cn, leg.amount);
        for item in &leg.items {
            println!(
                "    {} {}  ¥{:.0}",
                item.asset_code, item.asset_name, item.amount
            );
        }
    }

    let fees = plan.estimated_fees(&config.costs);
    println!();
    println!(
        "合计 ¥{:.0}（费用前）| 预计申购费约 ¥{:.0}（按现行费率估算）",
        plan.total_buy, fees
    );
    println!("提示: 执行后用 `mns buy <代码> <份额> <价格>` 记录，未买完的钱留在现金里下月继续。");
    println!("定投不产生收益 alpha，提升的是储蓄率与执行纪律。");
    guardrail::print_advice(&config, score);
    Ok(())
}

/// 买入冷静期：只读当前情绪状态，给出二次确认提示，不改动任何仓位。
fn cmd_guard() -> Result<()> {
    let config = AppConfig::load()?;
    let db = db::Database::open()?;
    let history = db.recent_fear_greed_scores(90)?;
    let latest = history
        .last()
        .map(|(_, s)| *s)
        .ok_or_else(|| anyhow::anyhow!("本地没有恐贪快照，请先运行一次 `mns report`"))?;
    let today = chrono::Local::now().date_naive();
    let signal = signal::compute(&config, latest, &history, today);

    println!("=================================================================");
    println!("   买入冷静期（流程约束，非择时信号）");
    println!("=================================================================");
    println!();
    println!(
        "最新恐贪快照 {:.1} | 有效决策分数 {:.1} | 过热阈值 {:.0} | 分区 {}",
        latest,
        signal.effective_score,
        config.guardrail.hot_threshold,
        config.sentiment_zone(signal.effective_score)
    );
    guardrail::print_advice(&config, signal.effective_score);
    println!();
    println!("执行买入时把理由记进流水（`mns history` 可复盘），本工具不替你做决定。");
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

fn load_audit_config(path: Option<String>) -> Result<AppConfig> {
    match path {
        Some(p) => AppConfig::load_from_path(&p),
        None => AppConfig::load(),
    }
}

/// 默认 `mns backtest`：完整实盘审计链路。
fn cmd_backtest_audit(config_path: Option<String>) -> Result<()> {
    cmd_backtest_validate_full(config_path, None)
}

/// 账户镜像：严格执行策略 vs 数据库真实交易流水。
fn cmd_backtest_mirror(config_path: Option<String>) -> Result<()> {
    let config = load_audit_config(config_path)?;
    let live = live_data::LiveDataset::load(&config)?;
    let long = live_data::LongDataset::load(&config)?;
    audit::print_data_banner(&config, &live, &long);
    run_mirror(&config, &live)
}

fn run_mirror(config: &AppConfig, live: &live_data::LiveDataset) -> Result<()> {
    let entry = chrono::NaiveDate::parse_from_str(&config.backtest.entry_date, "%Y-%m-%d")
        .context("backtest.entry_date 解析失败")?;
    let start_idx = live.days.partition_point(|d| d.date < entry);
    if start_idx >= live.days.len() {
        anyhow::bail!("实盘数据集未覆盖入场日 {}", entry);
    }
    let bars = live.to_bars(start_idx);
    let sig = audit::build_signal_series(config, &bars.bars);
    let mut opt = audit::AuditOptions::spanning("严格执行策略", &bars, config);
    opt.initial_cash = config.backtest.initial_cash;
    let strict = audit::run_audit(config, &bars, &sig, &opt)?;
    let db = db::Database::open()?;
    let actual = audit::run_actual(config, &bars, &db)?;
    audit::print_mirror(&strict, &actual, config);
    Ok(())
}

/// 重建两层日频数据集。
fn cmd_backtest_update_data() -> Result<()> {
    let config = load_audit_config(None)?;
    println!("正在重建实盘与长周期日频数据集（需要网络与 Python 3）...");
    let text = live_data::LiveDataset::update_all(&config)?;
    println!("{text}");
    let live = live_data::LiveDataset::load(&config)?;
    let long = live_data::LongDataset::load(&config)?;
    if let Ok(f) = live.freshness(config.backtest.freshness_sla_days) {
        println!("实盘层  {}", f.banner());
    }
    if let Ok(f) = long.freshness(config.backtest.freshness_sla_days) {
        println!("长周期层 {}", f.banner());
    }
    if let Ok(hs) = live_data::LongDataset::load_named(&config, "hs300") {
        if let Ok(f) = hs.freshness(config.backtest.freshness_sla_days) {
            println!("沪深300 实验 {}", f.banner());
        }
    }
    Ok(())
}

/// 完整有效性检验 + 牛熊分段 + 多折 walk-forward + holdout + 账户镜像。
fn cmd_backtest_validate_full(
    config_path: Option<String>,
    iterations: Option<usize>,
) -> Result<()> {
    let mut config = load_audit_config(config_path)?;
    if let Some(n) = iterations {
        config.backtest.permutation_iterations = n;
    }
    let live = live_data::LiveDataset::load(&config)?;
    let long = live_data::LongDataset::load(&config)?;
    audit::print_data_banner(&config, &live, &long);

    // ── 长周期代理层：穿越牛熊的策略有效性 ──
    let long_bars = long.to_bars();
    let sig = audit::build_signal_series(&config, &long_bars.bars);
    let mut opt = audit::AuditOptions::spanning("情绪信号（实盘同源）", &long_bars, &config);
    // 长周期对照统一本金，便于与买入持有同尺度比较（实盘镜像仍用真实 1 万本金）
    opt.initial_cash = 100_000.0;
    println!(
        "\n  长周期层统一本金 ¥100,000（仅用于相对比较）；账户镜像使用真实本金 ¥{:.0}",
        config.backtest.initial_cash
    );

    let suite = audit::run_validation_suite(&config, &long_bars, &sig, &opt)?;
    audit::print_validation_suite(&suite, &config);

    // holdout：末尾连续 36 个月，与主序列共用同一复利净值，不再各自归一化
    let end = long_bars.bars.len() - 1;
    let hold_from = long_bars.bars[end].date - chrono::Duration::days(365 * 3);
    let hs = long_bars.bars.partition_point(|b| b.date < hold_from);
    let mut ho = opt.clone();
    ho.start_idx = hs;
    ho.end_idx = end;
    ho.name = "最近 36 月 holdout".to_string();
    let hold_real = audit::run_audit(&config, &long_bars, &sig, &ho)?;
    let mut ho_bh = ho.clone();
    ho_bh.name = "holdout 买入持有".to_string();
    ho_bh.variant = audit::SignalVariant::Constant(1.0);
    ho_bh.buy_and_hold = true;
    let hold_bh = audit::run_audit(&config, &long_bars, &sig, &ho_bh)?;
    audit::print_result_table(
        &[&hold_real, &hold_bh],
        "holdout ≥36 月（连续复利，代理层）",
    );

    let regimes = long.meta.regimes.clone();
    if !regimes.is_empty() {
        let rows = audit::run_regimes(&config, &long_bars, &sig, &opt, &regimes);
        audit::print_regimes(&rows);
    }
    let folds = audit::run_walk_forward(&config, &long_bars, &sig, &opt);
    if !folds.is_empty() {
        audit::print_folds(&folds, &config);
    }

    // ── 实盘层：账户镜像 ──
    run_mirror(&config, &live)?;

    println!("\n═══ 边界声明 ═══════════════════════════════════════════════════════");
    for c in &long_bars.caveats {
        println!("  · {c}");
    }
    println!("  · 本报告为工程与方法验证，不构成投资建议。");
    Ok(())
}

/// 因果性本地逆向分值：只用 t 及之前的收盘价算 200 日均线偏离。
/// 分数语义与 FGI 一致（低分 = 超跌/恐慌 → 加仓；高分 = 超涨/贪婪 → 减仓）。
/// 替换输入后仍走 `signal::compute` 的同一套平滑/确认/决策逻辑，
/// 因此与 FGI 组的差异只来自分值本身，不来自执行口径。
fn local_reversal_scores(bars: &[live_data::Bar], window: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(bars.len());
    let mut sum = 0.0_f64;
    for i in 0..bars.len() {
        let p = bars[i].prices[0];
        sum += p;
        if i >= window {
            sum -= bars[i - window].prices[0];
        }
        let count = (i + 1).min(window);
        let ma = sum / count as f64;
        let dev = p / ma - 1.0;
        out.push((50.0 + dev * 200.0).clamp(0.0, 100.0));
    }
    out
}

/// 单资产对照实验：同一套引擎、同一套信号/成本/执行/回撤口径，
/// 检验 MNS 逆向择时框架放到单一宽基（默认沪深300）上的效果。
/// 额外加一组 A 股本地信号，用来区分「FGI 这个输入不合适」与「框架本身无效」。
fn cmd_backtest_asset(config_path: Option<String>, dataset: &str) -> Result<()> {
    let mut config = load_audit_config(config_path)?;
    let ds = live_data::LongDataset::load_named(&config, dataset)?;
    if ds.days.is_empty() {
        anyhow::bail!("实验数据集为空: {dataset}");
    }
    // 单资产：把三腿权重压到第一腿，严格等价于「标的 × 仓位 + 现金」
    if ds.meta.single_asset {
        config.allocation.us_stocks = 1.0;
        config.allocation.cn_stocks = 0.0;
        config.allocation.counter_cyclical = 0.0;
    }
    let bars = ds.to_bars();

    println!("\n═══ 单资产对照实验：{dataset} ══════════════════════════════════════════");
    if let Ok(f) = ds.freshness(config.backtest.freshness_sla_days) {
        println!("  数据 {}", f.banner());
    }
    println!(
        "  区间 {} ~ {}，{} 个交易日",
        bars.bars
            .first()
            .map(|b| b.date.to_string())
            .unwrap_or_default(),
        bars.bars
            .last()
            .map(|b| b.date.to_string())
            .unwrap_or_default(),
        bars.bars.len()
    );
    println!(
        "  标的 {}；通道 场内 ETF T+1；本金 ¥100,000，无外部注资",
        bars.label
    );
    println!("  口径 与 `mns backtest` 完全一致：7 天平滑 + 3 天确认 + 周频决策、");
    println!("       T+1 执行、同一成本模型、同一日频净值与回撤口径");
    println!("  ⚠ 单资产对照实验，结论不可迁移到三资产实盘配置；不构成投资建议");

    let mut opt = audit::AuditOptions::spanning("情绪信号(FGI)", &bars, &config);
    opt.initial_cash = 100_000.0;

    println!("\n── A. 原策略：CNN 恐贪指数 FGI ──────────────────────────────────────");
    let sig = audit::build_signal_series(&config, &bars.bars);
    let suite = audit::run_validation_suite(&config, &bars, &sig, &opt)?;
    audit::print_validation_suite(&suite, &config);

    println!("\n── B. 仅替换输入信号：沪深300 自身 200 日均线偏离（逆向） ──────────────");
    println!("   平滑/确认/决策日/成本/执行与 A 组完全相同，差异只来自分值本身");
    let local_raw = local_reversal_scores(&bars.bars, 200);
    let local_sig = audit::build_signal_series_with_raw(&config, &bars.bars, &local_raw);
    let mut local_opt = opt.clone();
    local_opt.name = "本地趋势逆向信号".to_string();
    let local_suite = audit::run_validation_suite(&config, &bars, &local_sig, &local_opt)?;
    audit::print_validation_suite(&local_suite, &config);

    // holdout：末尾连续 36 个月，与主序列共用同一复利净值
    let end = bars.bars.len() - 1;
    let hold_from = bars.bars[end].date - chrono::Duration::days(365 * 3);
    let hs = bars.bars.partition_point(|b| b.date < hold_from);
    let mut ho = opt.clone();
    ho.start_idx = hs;
    ho.end_idx = end;
    ho.name = "最近 36 月 holdout".to_string();
    let hold_real = audit::run_audit(&config, &bars, &sig, &ho)?;
    let mut ho_bh = ho.clone();
    ho_bh.name = "holdout 买入持有".to_string();
    ho_bh.variant = audit::SignalVariant::Constant(1.0);
    ho_bh.buy_and_hold = true;
    let hold_bh = audit::run_audit(&config, &bars, &sig, &ho_bh)?;
    audit::print_result_table(&[&hold_real, &hold_bh], "holdout ≥36 月（连续复利）");

    if !ds.meta.regimes.is_empty() {
        println!("  分段按 A 股市场共识日历划定（非算法识别），各段买入持有涨跌见下方诊断");
        let rows = audit::run_regimes(&config, &bars, &sig, &opt, &ds.meta.regimes);
        audit::print_regimes(&rows);
    }

    println!("\n═══ 数据诊断 ═══════════════════════════════════════════════════════");
    let num = |k: &str| ds.meta.diagnostics.get(k).and_then(|v| v.as_f64());
    if let (Some(tot), Some(ann), Some(dd)) = (
        num("price_total_return_pct"),
        num("price_annualized_pct"),
        num("price_max_drawdown_pct"),
    ) {
        println!("  标的全期 {tot:+.2}% / 年化 {ann:+.2}% / 日频最大回撤 {dd:.2}%");
    }
    if let (Some(c), Some(n)) = (
        num("fgi_vs_forward_20d_corr"),
        num("fgi_vs_forward_20d_pairs"),
    ) {
        println!("  FGI 与 A 股未来 20 日收益相关系数 {c:.4}（n={:.0}）", n);
    }
    if let Some(obj) = ds
        .meta
        .diagnostics
        .get("forward_20d_return_by_fgi_bucket")
        .and_then(|v| v.as_object())
    {
        println!("  各 FGI 分档的未来 20 日平均收益：");
        for k in ["<20", "20-40", "40-55", "55-75", ">=75"] {
            if let Some(v) = obj.get(k) {
                let avg = v
                    .get("avg_fwd_20d_pct")
                    .and_then(|x| x.as_f64())
                    .unwrap_or(f64::NAN);
                let n = v.get("samples").and_then(|x| x.as_f64()).unwrap_or(0.0);
                println!("    {k:>6}  n={n:>4.0}  {avg:+.3}%");
            }
        }
    }
    if let Some(arr) = ds.meta.regime_buy_hold_check.as_array() {
        println!("  各牛熊段买入持有：");
        for v in arr {
            let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
            let r = v
                .get("buy_hold_pct")
                .and_then(|x| x.as_f64())
                .unwrap_or(f64::NAN);
            println!("    {name:<24} {r:+7.2}%");
        }
    }

    println!("\n═══ 边界声明 ═══════════════════════════════════════════════════════");
    for c in &bars.caveats {
        println!("  · {c}");
    }
    println!("  · 本报告为工程与方法验证，不构成投资建议。");
    Ok(())
}

fn cmd_backtest_legacy(config_path: Option<String>, compare: Option<String>) -> Result<()> {
    use backtest::{BacktestConfig, Engine, SignalConfig, print_comparison, print_report};

    println!("=================================================================");
    println!("   MNS 逆向投资策略回测【已降级：长周期代理演示】");
    println!("=================================================================");
    println!("⚠️  本命令使用 2016-01~2025-04 的月频代理数据（270042/510880/518880），");
    println!("    信号为 3 月平滑 + 月末决策，与实盘 7 天平滑 + 日频决策不是同一策略；");
    println!("    假设 10 万起步 + 每年 5 万注资，与真实账户（1 万、无后续注资）不符。");
    println!("    结果仅用于长周期参考，不得用于实盘决策。实盘请用 `mns backtest`。");
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
        results.push(backtest::run(
            &base,
            &bt_config,
            &signal,
            Engine::BuyHold,
            &rows,
        ));
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
    for e in [Engine::BuyHoldRebalanced, Engine::BuyHold] {
        results.push(backtest::run(&config, &bt_config, &signal, e, &rows));
    }

    for r in &results {
        print_report(r);
    }
    print_comparison(&results);

    // 风险档位三档并列 × 阶梯/线性过渡：同一数据/成本/带宽
    let mut profile_results = Vec::new();
    for p in config::RiskProfile::ALL {
        for (mode, mode_label) in [
            (config::WeightMode::Step, "阶梯"),
            (config::WeightMode::Linear, "线性"),
        ] {
            let mut c = config.clone();
            c.apply_profile(p);
            c.target_weight.mode = mode;
            let mut r = backtest::run(&c, &bt_config, &signal, Engine::TargetWeight, &rows);
            r.name = format!(
                "{}·{}（中性 {:.0}%）",
                p.label(),
                mode_label,
                c.effective_neutral_weight()
            );
            profile_results.push(r);
        }
    }
    profile_results.push(backtest::run(
        &config,
        &bt_config,
        &signal,
        Engine::BuyHold,
        &rows,
    ));
    backtest::print_profile_comparison(&profile_results);

    // 定投频率：同样的年投入，年频 vs 月频
    let mut annual_bt = bt_config.clone();
    annual_bt.inflow_freq = backtest::InflowFreq::Annual;
    let mut annual = backtest::run(&config, &annual_bt, &signal, Engine::TargetWeight, &rows);
    annual.name = "年频注资（每年3月后）".to_string();
    let mut monthly_bt = bt_config.clone();
    monthly_bt.inflow_freq = backtest::InflowFreq::Monthly;
    let mut monthly = backtest::run(&config, &monthly_bt, &signal, Engine::TargetWeight, &rows);
    monthly.name = "月频定投（年额/12）".to_string();
    backtest::print_inflow_comparison(&[annual, monthly]);

    println!("提示: 运行 `mns backtest validate` 查看样本外验证与收益分布。");
    println!();
    Ok(())
}

/// 计算某个策略结果在指定日历年内的表现：(该年复合收益率, 该年内最大回撤)。
///
/// 收益率用剔除资金流影响的月度 `period_return` 复合得到；最大回撤基于该年内的
/// `total_value` 序列（峰值只在年内滚动，不带入年初之前的高点），用于逐年拆开看
/// 策略在真实下跌年份的表现，而不是只看整段回测区间的平均 Calmar。
/// 若数据集不覆盖该年份则返回 None。
fn year_window_stats(r: &backtest::BacktestResult, year: i32) -> Option<(f64, f64)> {
    use chrono::Datelike;
    let months: Vec<&backtest::Monthly> =
        r.monthly.iter().filter(|m| m.date.year() == year).collect();
    if months.is_empty() {
        return None;
    }
    let compounded = months
        .iter()
        .fold(1.0_f64, |acc, m| acc * (1.0 + m.period_return))
        - 1.0;
    let mut peak = months[0].total_value;
    let mut max_dd = 0.0_f64;
    for m in &months {
        if m.total_value > peak {
            peak = m.total_value;
        }
        if peak > 0.0 {
            let dd = (peak - m.total_value) / peak;
            if dd > max_dd {
                max_dd = dd;
            }
        }
    }
    Some((compounded, max_dd))
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

    if config.profile.core_blend > 0.0 {
        println!(
            "[INFO] 风险档位 {} 已启用（core_blend {:.1}）：调参网格只缩放卫星曲线，档位收缩保持不变。",
            config.profile_label(),
            config.profile.core_blend
        );
        println!();
    }

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
        .set_header(vec![
            "仓位缩放",
            "偏离带",
            "样本内年化",
            "样本内回撤",
            "Calmar",
        ]);
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
    tuned.target_weight.extreme_greed =
        (config.target_weight.extreme_greed * best_scale).min(100.0);
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
    println!(
        "
【Bootstrap 分布】{} 次重采样, 分块 {} 个月",
        iterations, block
    );
    let bt_full = BacktestConfig::spanning(&rows, 100_000.0, 50_000.0);
    let mut t3 = Table::new();
    t3.load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_header(vec![
            "策略",
            "年化 P5",
            "年化 P50",
            "年化 P95",
            "回撤 P50",
            "回撤 P95",
        ]);
    for (label, sc, engine) in [
        (
            "目标仓位(趋势锚)",
            SignalConfig::trend_tilt(),
            Engine::TargetWeight,
        ),
        ("目标仓位(情绪锚)", signal, Engine::TargetWeight),
        ("买入持有", signal, Engine::BuyHold),
    ] {
        let r = backtest::run(&config, &bt_full, &sc, engine, &rows);
        let samples = metrics::block_bootstrap(&r.returns(), block, iterations, 12.0, 20260731);
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

    // ── 2.5) 挑战期子区间：不要只看整段区间的平均 Calmar，看策略在真实下跌年份的逐段表现 ──
    println!(
        "
【挑战期子区间】数据覆盖内的两次显著下跌年份，逐年拆开看（不是整段区间的平均值）"
    );
    let mut t25 = Table::new();
    t25.load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_header(vec![
            "策略",
            "2018 年内收益",
            "2018 年内最大回撤",
            "2022 年内收益",
            "2022 年内最大回撤",
        ]);
    for (label, sc, engine) in [
        (
            "目标仓位(趋势锚)",
            SignalConfig::trend_tilt(),
            Engine::TargetWeight,
        ),
        ("目标仓位(情绪锚)", signal, Engine::TargetWeight),
        ("买入持有", signal, Engine::BuyHold),
    ] {
        let r = backtest::run(&config, &bt_full, &sc, engine, &rows);
        let y2018 = year_window_stats(&r, 2018);
        let y2022 = year_window_stats(&r, 2022);
        t25.add_row(vec![
            Cell::new(label),
            Cell::new(
                y2018
                    .map(|(ret, _)| format!("{:.2}%", ret * 100.0))
                    .unwrap_or_else(|| "无数据".into()),
            ),
            Cell::new(
                y2018
                    .map(|(_, dd)| format!("{:.2}%", dd * 100.0))
                    .unwrap_or_else(|| "无数据".into()),
            ),
            Cell::new(
                y2022
                    .map(|(ret, _)| format!("{:.2}%", ret * 100.0))
                    .unwrap_or_else(|| "无数据".into()),
            ),
            Cell::new(
                y2022
                    .map(|(_, dd)| format!("{:.2}%", dd * 100.0))
                    .unwrap_or_else(|| "无数据".into()),
            ),
        ]);
    }
    println!("{}", t25);
    println!(
        "注意：2018/2022 是本数据集(2016起)里仅有的两段明显下跌年份，均处于长期低利率+美股结构性牛市\n\
         的大背景下，不能替代 2000-2002／2008 这类多年期熊市压力测试——那类数据当前不可得，\n\
         该项验证仍然缺失（见下方【口径与局限】）。"
    );

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
            (
                "目标仓位(趋势锚)",
                SignalConfig::trend_tilt(),
                Engine::TargetWeight,
            ),
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

fn cmd_config_params() -> Result<()> {
    println!("可调参数列表（`mns config <键> <值>` 修改，改后立即生效）:");
    println!();
    println!("  【目标仓位曲线】风险资产占总资产的目标权重(%)，逆向策略要求随情绪升高单调不增");
    println!("    target_weight.extreme_fear     极度恐慌 (默认: 85)");
    println!("    target_weight.fear             恐慌     (默认: 75)");
    println!("    target_weight.neutral          中性     (默认: 60)");
    println!("    target_weight.greed            贪婪     (默认: 45)");
    println!("    target_weight.extreme_greed    极度贪婪 (默认: 35)");
    println!();
    println!("  【再平衡与信号】（实盘与回测共用，不允许两条链路各设一套）");
    println!("    rebalance.band_pp                    偏离带(百分点)，越大交易越少 (默认: 4)");
    println!("    rebalance.min_trade_amount           最小交易金额 (默认: 500)");
    println!("    rebalance.min_holding_days_for_sell  卖出最短持有天数 (默认: 30)");
    println!("    rebalance.signal_smooth_days         决策信号平滑天数 (默认: 7)");
    println!("    rebalance.zone_confirm_days          区间切换确认天数 (默认: 3)");
    println!("    rebalance.decision_weekday           决策日 0=每天 / 1-7=周一..周日 (默认: 5)");
    println!();
    println!("  【风险档位与现金流】");
    println!(
        "    profile                     无档位/稳健/平衡/进取/custom（core_blend 0/0.4/0.6/0.8；档位抬高常驻仓位，不降低风险）"
    );
    println!("    target_weight.mode          阶梯 step 或线性 linear 过渡");
    println!("    cashflow.monthly            `mns plan` 默认月投入金额");
    println!();
    println!("  【成本与现金】");
    println!("    costs.buy_fee_pct           买入费率% (默认: 0.12)");
    println!("    costs.sell_fee_pct          卖出固定费率%，场外基金为 0 (默认: 0.0)");
    println!("    costs.cash_annual_yield     闲置现金年化% (默认: 1.3)");
    println!("    (阶梯赎回费与按通道结算参数需直接编辑 config.toml 的 [costs] / [channels])");
    println!();
    println!("  【实盘审计】`mns backtest` 默认链路");
    println!("    backtest.data_path                   实盘日频数据集路径");
    println!("    backtest.freshness_sla_days          数据新鲜度 SLA 天数 (默认: 30)");
    println!("    backtest.initial_cash                账户期初本金 (默认: 10000)");
    println!("    backtest.entry_date                  真实入场日 YYYY-MM-DD");
    println!("    backtest.annual_trade_budget_min/max 年度交易预算 (默认: 10 / 20)");
    println!("    backtest.decision_interval           daily / weekly / monthly (默认: weekly)");
    println!("    backtest.execution_lag_days          信号到成交的滞后天数，须 >=1 (默认: 1)");
    println!("    backtest.permutation_iterations      置换检验次数，须 >=100 (默认: 300)");
    println!("    backtest.significance_percentile     择时显著性分位门槛 (默认: 80)");
    println!("    [channels.*] qdii_feeder / cn_feeder / on_exchange_etf 各自的费率、");
    println!("                 赎回阶梯、T+n 结算、每日限购、滑点与代理跟踪损耗");
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
    println!("  【显示】不驱动交易，仅改变报告/持仓表展示");
    println!("    settings.annualized_target_high  年化收益 ≥ 此值(%)时绿色标记 (默认: 15)");
    println!();
    println!("  【买入冷静期】只做减速与留痕，不阻断交易、不改仓位");
    println!("    guardrail.enabled          是否启用过热提示 (默认: true)");
    println!("    guardrail.hot_threshold    触发提示的 FGI 分数 (默认: 75)");
    println!("    guardrail.cooldown_days    建议延迟的交易日数 (默认: 1)");
    println!("    guardrail.show_stats       显示历史描述性统计（未通过显著性检验）");
    println!("    注: 自动否决买入在长期上行资产池上期望为负，故本功能不拦截。");
    println!("        详见 docs/guardrail-experiment-2026-11-08.md");
    println!();
    println!("用法示例:");
    println!(
        "  mns backtest                           # 实盘审计：有效性检验 + 牛熊分段 + 账户镜像"
    );
    println!("  mns backtest mirror                    # 只看严格执行 vs 实际执行");
    println!("  mns backtest asset --dataset hs300     # 单资产对照实验（含 A 股本地信号对照）");
    println!("  mns backtest update-data               # 重建三层日频数据集（含新鲜度校验）");
    println!("  mns backtest validate-full             # 只看统计检验（可 --iterations 覆盖次数）");
    println!(
        "  mns backtest legacy                    # 已降级：2016-2025 月频代理对比，不可用于实盘"
    );
    println!("  mns guard                              # 查看当前是否处于买入冷静期");
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
    table.add_row(vec![
        Cell::new("当前价格"),
        Cell::new(&format!("{:.2}", quote.price)),
    ]);

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
