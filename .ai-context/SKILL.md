# AI Context — MNS Project

> **Activation triggers** — Read this directory when:
> - Starting a coding session in `money-never-sleep`
> - User asks about project structure, architecture, or decisions
> - Implementing new features or debugging
> - User says "create ai-context", "setup project knowledge"
>
> **Do NOT read** if `.ai-context/SKILL.md` was read in the last 24h and no major changes were made.

---

## Quick Start

```bash
# Build
cargo build --release

# Initialize (如有数据会提示确认，使用 --force 跳过)
mns init

# Common workflow
mns cash set 100000
mns add QQQ "纳指100" us_stocks
mns buy QQQ 100 350.00
mns remove QQQ          # 删除资产（慎用）
mns update-prices       # 自动更新所有资产价格
mns portfolio
mns report
mns backtest            # 策略回测（使用保守配置）
```

## File Map

| File | Purpose |
|------|---------|
| `SKILL.md` | This file — entry point |
| `quote.rs` | 自动价格获取（天天基金/Yahoo Finance） |
| `sentiment.rs` | 恐贪指数获取（CNN API，股票市场） |
| `references/PROJECT-ESSENCE.md` | What & why |
| `references/ARCHITECTURE.md` | Component relationships |
| `references/DECISIONS.md` | Design decisions |
| `DYNAMICS.md` | Active issues |
| `meta/MAINTENANCE.md` | How to update |

## Key Facts

- **Language**: Rust (edition 2024)
- **Database**: SQLite via `rusqlite` (bundled)
- **HTTP**: `reqwest` for APIs (CNN Fear & Greed Index, 天天基金, Yahoo Finance)
- **CLI**: `clap` v4 (derive mode)
- **Config**: TOML at `~/.mns/config.toml`
- **Data**: SQLite at `~/.mns/mns.db`
- **Reports**: `./reports/YYYY-MM-DD.txt`

## Architecture Summary

8 source modules, single binary, no frontend yet (future Svelte 5).

Data flow: `sentiment` → `db` → `strategy` (sell→buy→risk) → `report`

Default config: **保守配置** (US 55%, CN 25%, Gold 20%)

See `references/ARCHITECTURE.md` for details.
