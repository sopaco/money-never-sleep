# Architecture — MNS

**Last updated**: 2026-04-19

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
| `config.rs` | TOML load/save, 5-zone threshold logic | `AppConfig::load()`, `save()`, `sentiment_zone()`, `buy_ratio_for()`, `sell_ratio_for()` |
| `db.rs` | SQLite CRUD (transactional) | `Database::open()`, `get_cash_balance()`, `buy_position()`, `sell_position()`, etc. |
| `models.rs` | Data structures + return calc | `Position`, `Transaction`, `FearGreedResponse`; `annualized_return_with_min_days()`, `absolute_return()` |
| `sentiment.rs` | CNN API fetch | `fetch_fear_greed(config)` async |
| `strategy.rs` | Strategy engine (sell→buy→risk order) | `calculate_sell_suggestions()`, `calculate_buy_suggestions()`, `check_risk_warnings()` |
| `report.rs` | Report generation + net flow | `generate_report()`, `save_report()` |
| `main.rs` | Command dispatch | All `cmd_*` functions |

## Data Flow (report command)

```
cmd_report()
  1. sentiment::fetch_fear_greed()      → score, rating, prev values
  2. db::Database::open()               → cash balance, positions
  3. strategy::calculate_sell()         → Vec<SellSuggestion> (includes reason + absolute_return)
  4. strategy::calculate_buy(sell_refs) → BuySuggestion (available_cash = cash + sell_proceeds)
  5. strategy::check_risk_warnings()    → Vec<RiskWarning> (sentiment-aware advice)
  6. report::generate_report()          → formatted text + net operation guidance
  7. report::save_report()              → write to {report_output_dir}/YYYY-MM-DD.txt
```

**Key**: sell is computed first, so buy suggestions include sell proceeds.

## Database Schema

Located at `~/.mns/mns.db`:

- **cash** — single-row table: `balance`, `updated_at`
- **positions** — `asset_code` (unique), `asset_name`, `category`, `shares`, `cost_price`, `current_price`, `first_buy_date`
- **transactions** — `type` (buy/sell), `asset_code`, `shares`, `price`, `amount`, `tx_date`
- **fear_greed_snapshots** — daily FGI scores (one per day, overwrite on re-fetch)

## Configuration (TOML)

Path: `~/.mns/config.toml`

Key sections:
- `[settings]` — `annualized_target_low/high`, `min_holding_days`, `report_output_dir`
- `[allocation]` — `us_stocks`, `cn_stocks`, `counter_cyclical` (must sum to 100)
- `[thresholds]` — `extreme_fear`, `fear`, `neutral`, `greed` score boundaries
- `[buy_ratio]` — cash deployment % per sentiment zone (5 zones incl. extreme_greed=0%)
- `[sell_ratio]` — profit-taking % per sentiment × return matrix (includes `neutral_target_high`)
- `[api]` — CNN API URL

## Key Design Patterns

- **No frontend** — pure CLI, text output is the API
- **Weighted average cost** — `buy_position()` recalculates cost_price as `(old_total + new_amount) / new_shares`
- **Annualized return with min days** — `(current / cost) ^ (365 / holding_days) - 1`, N/A if < `min_holding_days`
- **Absolute return** — `(current - cost) / cost`, used for long-term profit-taking regardless of annualized
- **Contrarian buy distribution** — `distribute_amount_contrarian()` uses weight = `max(1.0, cost/current)`, favoring underwater positions
- **Sentiment zone** — 5 zones determined by `thresholds.*` in config
- **Sell matrix** — `sell_ratio_for(score, annualized_pct)` returns the % to sell based on 3×3 matrix (neutral/greed/extreme_greed × return level)
- **Transactional DB** — `buy_position()` and `sell_position()` use SQLite transactions for atomicity
- **Sentiment-aware risk** — `check_risk_warnings()` gives different advice based on market zone (fear=buy opportunity, neutral=review, greed=urgent)
