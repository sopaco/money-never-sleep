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
      │SQLite  │  │ HTTP reqwest │
      └────┬───┘  └────┬────┘
           │           │
           ▼           ▼
      ┌────────┐  ┌──────────┐
      │models.rs│  │strategy.rs│
      │ structs │  │ buy/sell  │
      └────┬───┘  │  engine   │
           │       └────┬─────┘
           │            │
           ▼            ▼
      ┌────────┐  ┌──────────┐
      │report.rs│  │  (uses   │
      │ text    │  │ strategy)│
      │ generator│ └──────────┘
      └─────────┘
```

## Module Responsibilities

| Module | Responsibility | Public API |
|--------|---------------|------------|
| `cli.rs` | Clap command definitions | `Cli`, `Commands`, `CashAction` |
| `config.rs` | TOML load/save, threshold logic | `AppConfig::load()`, `save()`, `sentiment_zone()`, `buy_ratio_for()`, `sell_ratio_for()` |
| `db.rs` | SQLite CRUD | `Database::open()`, `get_cash_balance()`, `buy_position()`, `sell_position()`, etc. |
| `models.rs` | Data structures | `Position`, `Transaction`, `FearGreedSnapshot`, `FearGreedResponse` |
| `sentiment.rs` | CNN API fetch | `fetch_fear_greed(config)` async |
| `strategy.rs` | Strategy engine | `calculate_buy_suggestions()`, `calculate_sell_suggestions()`, `check_risk_warnings()` |
| `report.rs` | Report generation | `generate_report()`, `save_report()` |
| `main.rs` | Command dispatch | All `cmd_*` functions |

## Data Flow (report command)

```
cmd_report()
  1. sentiment::fetch_fear_greed()  → score, rating, prev values
  2. db::Database::open()            → cash balance, positions
  3. strategy::calculate_buy()      → BuySuggestion (amount + allocation)
  4. strategy::calculate_sell()      → Vec<SellSuggestion>
  5. strategy::check_risk_warnings() → Vec<RiskWarning>
  6. report::generate_report()       → formatted text string
  7. report::save_report()           → write to {report_output_dir}/YYYY-MM-DD.txt
```

## Database Schema

Located at `~/.mns/mns.db`:

- **cash** — single-row table: `balance`, `updated_at`
- **positions** — `asset_code` (unique), `asset_name`, `category`, `shares`, `cost_price`, `current_price`, `first_buy_date`
- **transactions** — `type` (buy/sell), `asset_code`, `shares`, `price`, `amount`, `tx_date`
- **fear_greed_snapshots** — daily FGI scores stored for history

## Configuration (TOML)

Path: `~/.mns/config.toml`

Key sections:
- `[settings]` — `annualized_target_low/high`, `report_output_dir`
- `[allocation]` — `us_stocks`, `cn_stocks`, `counter_cyclical` (must sum to 100)
- `[thresholds]` — `extreme_fear`, `fear`, `neutral`, `greed` score boundaries
- `[buy_ratio]` — cash deployment % per sentiment zone
- `[sell_ratio]` — profit-taking % per sentiment × annualized return matrix
- `[api]` — CNN API URL

## Key Design Patterns

- **No frontend** — pure CLI, text output is the API
- **Weighted average cost** — `buy_position()` recalculates cost_price as `(old_total + new_amount) / new_shares`
- **Annualized return** — `(current / cost) ^ (365 / holding_days) - 1`
- **Sentiment zone** — determined by `thresholds.*` in config, not hardcoded
- **Sell matrix** — `sell_ratio_for(score, annualized_pct)` returns the % to sell based on 2D matrix (zone × return)
