# Architecture — MNS

**Last updated**: 2026-04-22

---

## Component Map

```
┌─────────────────────────────────────────────────────┐
│                    main.rs                          │
│  CLI entry, command dispatch, async runtime (tokio) │
└──────────┬──────────────────────────────────────────┘
           │
    ┌──────┴──────┐
    ▼             ▼
┌────────┐  ┌──────────┐
│ cli.rs │  │config.rs │
│ clap   │  │ TOML I/O  │
│ derive │  └────┬─────┘
└────────┘       │
           ┌─────┴─────┐
           ▼           ▼
      ┌────────┐  ┌──────────┐
      │ db.rs  │  │sentiment.rs│
      │SQLite  │  │ CNN API    │
      │(txn)   │  └────┬────┘
      └────┬───┘       │
           │           │
           ▼           ▼
      ┌────────┐  ┌──────────┐
      │models.rs│  │strategy.rs│
      │ structs │  │ sell→buy  │
      └────┬───┘  │ +risk     │
           │       └────┬─────┘
           │            │
           ▼            ▼
      ┌──────────┐  ┌───────────┐
      │report.rs │  │backtest.rs │
      │ text +   │  │ 多资产回测 │
      │ net flow │  │ 参数优化   │
      └──────────┘  └───────────┘
```

## Module Responsibilities

| Module | Responsibility | Public API |
|--------|---------------|------------|
| `cli.rs` | Clap command definitions | `Cli`, `Commands`, `CashAction`, `Remove` |
| `config.rs` | TOML load/save, 5-zone threshold logic, dot-path get/set | `AppConfig::load()`, `save()`, `sentiment_zone()`, `buy_ratio_for()`, `sell_ratio_for()` |
| `db.rs` | SQLite CRUD (transactional, auto-create tables) | `Database::open()`, `get_cash_balance()`, `buy_position()`, `sell_position()`, `remove_position()`, `save_fear_greed_snapshot()` |
| `models.rs` | Data structures + return calculations | `Position`, `Transaction`; `annualized_return_with_min_days()`, `absolute_return()`, `market_value_or_cost()` |
| `quote.rs` | 自动价格获取（天天基金/Yahoo Finance） | `fetch_price(code, category)` async, `update_all_prices()` async, `PriceUpdate` |
| `sentiment.rs` | 恐贪指数获取（CNN API，股票市场） | `fetch_fear_greed_index()` async |
| `strategy.rs` | 策略引擎（sell→buy→risk 顺序） | `calculate_sell_suggestions()`, `calculate_buy_suggestions()`, `check_risk_warnings()` |
| `report.rs` | 报告生成（净操作+风险+预案） | `generate_report()`, `save_report()` |
| `backtest.rs` | 回测引擎（多资产+多配置对比） | `run_backtest()`, `run_multi_asset_backtest()`, `run_buy_and_hold()`, `print_comparison()` |
| `main.rs` | 命令分发，UI输出 | 所有 `cmd_*` 函数 |

## Data Flow (report command)

```
cmd_report()
  1. sentiment::fetch_fear_greed_index()   → score (0-100)
  2. db::Database::open()                  → cash balance, positions
  3. db::save_fear_greed_snapshot()        → persist daily snapshot
  4. strategy::calculate_sell()             → Vec<SellSuggestion>
  5. strategy::calculate_buy(sell_refs)    → BuySuggestion (available = cash + proceeds)
  6. strategy::check_risk_warnings()       → Vec<RiskWarning>
  7. report::generate_report()             → formatted text
  8. report::save_report()                 → write to reports/
```

**Key**: sell 先计算，buy 使用 sell 回收金额。

## Data Flow (backtest command)

```
cmd_backtest()
  1. load historical data from embedded CSV
  2. run_multi_asset_backtest() for 美股+红利低波+黄金
  3. run_buy_and_hold() for baseline
  4. run_backtest() with multiple config variants
  5. print_comparison() showing all results
```

## Database Schema

Located at `~/.mns/mns.db`:

- **cash** — single row: `balance`, `updated_at`
- **positions** — `asset_code` (unique), `asset_name`, `category`, `shares`, `cost_price`, `current_price`, `first_buy_date`
- **transactions** — `type`, `asset_code`, `shares`, `price`, `amount`, `tx_date`, `note`
- **fear_greed_snapshots** — daily FGI scores (one per day)

## Configuration (TOML)

Path: `~/.mns/config.toml`

Default configuration (保守配置):
```toml
[settings]
annualized_target_low = 10.0      # 年化10%开始减仓
annualized_target_high = 15.0    # 年化15%大笔减仓
min_holding_days = 45            # 最小持仓天数
min_absolute_profit_days = 120   # 绝对收益持仓天数
max_contrarian_weight = 2.0     # 逆势加仓上限
report_output_dir = "./reports"

[allocation]
us_stocks = 55.0                 # 美股占比
cn_stocks = 25.0                 # A股占比
counter_cyclical = 20.0          # 黄金对冲

[thresholds]
extreme_fear = 30.0              # 极度恐慌阈值
fear = 45.0
neutral = 55.0
greed = 70.0                     # 贪婪阈值

[buy_ratio]
extreme_fear = 60.0              # 极度恐慌60%仓位
fear = 35.0                      # 恐慌35%
neutral = 0.0                    # 中性不买
greed = 0.0
```

## Key Design Patterns

- **Weighted average cost** — `buy_position()` recalculates cost_price
- **Annualized return with min days** — N/A if < `min_holding_days`
- **Absolute return** — for long-term profit-taking
- **Contrarian buy distribution** — weight = `max(1.0, cost/current)`
- **Sentiment zone** — 5 zones from thresholds
- **Sell matrix** — zone × return level
- **Dual sell triggers** — annualized + absolute ≥ 30%
- **Transactional DB** — SQLite transactions
- **Sentiment-aware risk** — different advice per zone
- **Dot-path config** — `get_value("thresholds.fear")`
