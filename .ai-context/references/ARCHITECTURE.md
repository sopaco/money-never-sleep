# Architecture — MNS

**Last updated**: 2026-04-20

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
      ┌────────┐  ┌─────────┐
      │ db.rs  │  │sentiment.rs│
      │SQLite  │  │ HTTP reqwest│
      │(txn)   │  └────┬────┘
      └────┬───┘  └────┬────┘
           │           │
           ▼           ▼
      ┌────────┐  ┌──────────┐
      │models.rs│  │strategy.rs│
      │ structs │  │ sell→buy  │
      └────┬───┘  │ +risk     │
           │       └────┬─────┘
           │            │
           ▼            ▼
      ┌──────────┐
      │report.rs │
      │ text +   │
      │ net flow │
      └──────────┘
```

## Module Responsibilities

| Module | Responsibility | Public API |
|--------|---------------|------------|
| `cli.rs` | Clap command definitions | `Cli`, `Commands`, `CashAction` |
| `config.rs` | TOML load/save, 5-zone threshold logic, dot-path get/set | `AppConfig::load()`, `save()`, `sentiment_zone()`, `buy_ratio_for()`, `sell_ratio_for()`, `get_value()`, `set_value()` |
| `db.rs` | SQLite CRUD (transactional, auto-create tables) | `Database::open()`, `get_cash_balance()`, `buy_position()`, `sell_position()`, `save_fear_greed_snapshot()`, etc. |
| `models.rs` | Data structures + return calculations | `Position`, `Transaction`, `FearGreedResponse`; `annualized_return_with_min_days()`, `absolute_return()`, `market_value_or_cost()` |
| `quote.rs` | 自动价格获取（天天基金/Yahoo Finance） | `fetch_price(code, category)` async, `update_all_prices()` async, `PriceUpdate` |
| `sentiment.rs` | CNN API fetch with custom headers | `fetch_fear_greed(config)` async |
| `strategy.rs` | Strategy engine (sell→buy→risk order) | `calculate_sell_suggestions()`, `calculate_buy_suggestions()`, `check_risk_warnings()` |
| `report.rs` | Report generation with net flow + risk + presets | `generate_report()`, `save_report()` |
| `main.rs` | Command dispatch, UI output | All `cmd_*` functions |

## Data Flow (report command)

```
cmd_report()
  1. sentiment::fetch_fear_greed()      → score, rating, prev values
  2. db::Database::open()               → cash balance, positions
  3. db::save_fear_greed_snapshot()     → persist daily snapshot
  4. strategy::calculate_sell()         → Vec<SellSuggestion> (with reason: AnnualizedHigh | AbsoluteProfit)
  5. strategy::calculate_buy(sell_refs) → BuySuggestion (available_cash = cash + sell_proceeds, contrarian weighting)
  6. strategy::check_risk_warnings()    → Vec<RiskWarning> (sentiment-aware: ConsiderBuyMore | ReviewFundamentals | UrgentReview)
  7. report::generate_report()          → formatted text (情绪+概览+持仓+卖出+买入+净操作+风险+预案)
  8. report::save_report()              → write to {report_output_dir}/YYYY-MM-DD.txt
```

**Key**: sell is computed first, so buy suggestions include sell proceeds.

## Database Schema

Located at `~/.mns/mns.db` (auto-created on first `Database::open()`):

- **cash** — single-row table (id=1): `balance`, `updated_at`
- **positions** — `asset_code` (unique), `asset_name`, `category`, `shares`, `cost_price`, `current_price`, `first_buy_date`
- **transactions** — `type` (buy/sell), `asset_code`, `shares`, `price`, `amount`, `tx_date`, `note`
- **fear_greed_snapshots** — daily FGI scores (one per day, DELETE+INSERT on re-fetch)

## Configuration (TOML)

Path: `~/.mns/config.toml`

Key sections:
- `[settings]` — `annualized_target_low/high`, `min_holding_days`, `report_output_dir`
- `[allocation]` — `us_stocks`, `cn_stocks`, `counter_cyclical` (must sum to 100)
- `[thresholds]` — `extreme_fear`, `fear`, `neutral`, `greed` score boundaries
- `[buy_ratio]` — cash deployment % per sentiment zone (5 zones incl. extreme_greed=0%)
- `[sell_ratio]` — profit-taking % per sentiment × return matrix (6 fields: 3 zones × 2 return levels + extreme_greed_below_target + neutral_target_high)
- `[api]` — CNN API URL

## Key Design Patterns

- **Weighted average cost** — `buy_position()` recalculates cost_price as `(old_total + new_amount) / new_shares`
- **Annualized return with min days** — `(current / cost) ^ (365 / holding_days) - 1`, N/A if < `min_holding_days`
- **Absolute return** — `(current - cost) / cost`, used for long-term profit-taking regardless of annualized
- **Contrarian buy distribution** — `distribute_amount_contrarian()` uses weight = `max(1.0, cost/current)`, favoring underwater positions
- **Sentiment zone** — 5 zones determined by `thresholds.*` in config
- **Sell matrix** — `sell_ratio_for(score, annualized_pct)` returns the % to sell based on 3×3 matrix (neutral/greed/extreme_greed × return level)
- **Dual sell triggers** — annualized return (primary) + absolute return ≥ 30% (secondary, for long-held positions)
- **Transactional DB** — `buy_position()` and `sell_position()` use SQLite transactions for atomicity
- **Sentiment-aware risk** — `check_risk_warnings()` gives different advice based on market zone (fear=buy opportunity, neutral=review, greed=urgent)
- **Dot-path config** — `get_value("thresholds.fear")` / `set_value("buy_ratio.extreme_fear", "50")` for CLI config management
