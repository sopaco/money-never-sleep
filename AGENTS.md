# AGENTS.md — MNS Project

> This file tells **AI coding agents** how to work in this project.

---

## First Step: Read Project Knowledge

**Before doing anything else**, read the `.ai-context/` directory:

```bash
cat .ai-context/SKILL.md
cat .ai-context/references/PROJECT-ESSENCE.md
cat .ai-context/references/ARCHITECTURE.md
cat .ai-context/DYNAMICS.md
```

These files contain the project's architecture, design decisions, and active issues. They are **the primary source of truth** for understanding the project quickly.

## Project Overview

- **Type**: Personal CLI investment tool (Rust)
- **Binary**: `target/release/mns.exe` (Windows) / `target/release/mns` (Linux/macOS)
- **Data**: `~/.mns/config.toml` + `~/.mns/mns.db`
- **Stack**: Rust edition 2021, SQLite (rusqlite), reqwest, clap, chrono, comfy-table

## Standard Workflow

### Build
```bash
cargo build --release
```

### Run
```bash
# All commands go through the mns binary
mns init
mns cash set 100000
mns portfolio
mns report
```

### Test a change
```bash
cargo build --release
# Test the specific command you changed
mns portfolio
mns history
```

## Key Files

| File | Purpose |
|------|---------|
| `src/main.rs` | Entry point + all command handlers (`cmd_*` functions) |
| `src/cli.rs` | Clap command/argument definitions |
| `src/config.rs` | TOML config load/save + strategy threshold logic |
| `src/db.rs` | SQLite operations (cash, positions, transactions, snapshots) |
| `src/models.rs` | Data structs (`Position`, `Transaction`, `FearGreedResponse`) |
| `src/sentiment.rs` | CNN Fear & Greed API fetch (async) |
| `src/strategy.rs` | Buy/sell calculation engine |
| `src/report.rs` | Text report generation |
| `PRD.txt` | Full product spec (source of truth for features) |
| `.codebuddy/skills/mns-backtest/SKILL.md` | Backtest skill — activates on "回测" / "backtest" |

## Important Conventions

### Async runtime
`sentiment.rs` uses async (`reqwest`). All `cmd_*` functions that call it must be `async`. `main()` uses `#[tokio::main]`.

### Configuration dot-path notation
Use dot-path strings like `thresholds.fear`, `buy_ratio.extreme_fear` — these match the TOML structure exactly.

### Data flow convention
`sentiment` → `db` → `strategy` → `report`. Never skip steps in the report pipeline.

### Annualized return formula
`annualized = (current / cost) ^ (365 / holding_days) - 1`
- `Position::annualized_return_with_min_days(today, min_days)` — uses `settings.min_holding_days` threshold to avoid short-term distortion
- `Position::annualized_return(today)` — no minimum days, kept as general API
- `Position::absolute_return()` — simple `(current - cost) / cost`, used for long-term profit-taking
- `config::sell_ratio_for()` expects percentage (e.g. `18.5`)

### Strategy engine pipeline
`strategy` module computes in this order:
1. `calculate_sell_suggestions()` — sell first
2. `calculate_buy_suggestions()` — buy uses `available_cash = cash + sell_proceeds` (buy/sell aware)
3. `check_risk_warnings()` — risk warnings with sentiment-aware advice

### Buy distribution: contrarian weighting
`distribute_amount_contrarian()` assigns more funds to underwater positions:
- Weight = `max(1.0, cost_price / current_price)` — losing positions get higher weight
- Winning positions get weight 1.0 (baseline)
- This aligns with contrarian strategy (buy more when price is below cost)

### Sell decision: dual-criteria
Sell suggestions consider both:
1. **Annualized return** (with min holding days threshold) → PRD matrix
2. **Absolute return** ≥ 30% → long-term profit-taking even if annualized is low

### Error handling
Use `anyhow::Result<()>` for command handlers. Use `anyhow::bail!("message")` for user-facing errors.

### Windows encoding
PowerShell uses GBK by default. If printing to terminal causes mojibake, wrap with:
```rust
use std::io::Write;
println!("{}", text); // usually fine with UTF-8 terminal
```

## Common Tasks

### Adding a new command
1. Add variant to `Commands` enum in `cli.rs`
2. Add `cmd_*` function in `main.rs`
3. Match in `main()` match block

### Changing strategy thresholds
Edit `src/config.rs` — `buy_ratio_for()` and `sell_ratio_for()` contain all threshold logic. No other file needs changing.

### Adding a database field
1. Add to `models.rs` struct
2. Add to `db.rs` SQL queries
3. Run `mns init` to recreate schema (or manually ALTER TABLE in sqlite3)

### Running backtests
Use the skill: say "回测" or "backtest" to activate the mns-backtest skill.

## Constraints

- **Do not** add push notifications — the tool outputs text reports only
- **Do not** add frontend code yet — PRD specifies Svelte 5 for future phase
- **Do not** commit `target/` directory to git
- **Do not** modify `.ai-context/` without also updating `meta/MAINTENANCE.md`

## Architecture Decision Boundaries

If asked to change any of these, re-read `references/DECISIONS.md` first:

- SQLite chosen over JSON/CSV → reason in DECISIONS.md
- Text reports over push → reason in DECISIONS.md
- Weighted average cost for positions → reason in DECISIONS.md
- 2D sell matrix (zone × return) → reason in DECISIONS.md
- 浮亏警告 only, no auto-sell → reason in DECISIONS.md
