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
mns market              # 市场综合概况（指数 + 恐贪指数）
mns market-indices      # 全球主要指数
mns analyze AAPL        # 个股分析
mns backtest            # 策略回测（多配置对比）
```

## File Map

| File | Purpose |
|------|---------|
| `SKILL.md` | This file — entry point |
| `quote.rs` | 自动价格获取（天天基金/Yahoo Finance v8） |
| `market.rs` | 市场数据模块（全球指数/个股报价） |
| `sentiment.rs` | 恐贪指数获取（CNN API，股票市场） |
| `backtest.rs` | 回测引擎（多资产+多配置对比） |
| `references/PROJECT-ESSENCE.md` | What & why |
| `references/ARCHITECTURE.md` | Component relationships |
| `references/DECISIONS.md` | Design decisions |
| `DYNAMICS.md` | Active issues |
| `meta/MAINTENANCE.md` | How to update |

## Key Facts

- **Language**: Rust (edition 2024)
- **Database**: SQLite via `rusqlite` (bundled)
- **HTTP**: `reqwest` for APIs (CNN Fear & Greed, 天天基金, Yahoo Finance v8)
- **CLI**: `clap` v4 (derive mode)
- **Config**: TOML at `~/.mns/config.toml`
- **Data**: SQLite at `~/.mns/mns.db`
- **Reports**: `./reports/YYYY-MM-DD.txt`

## Architecture Summary

11 source modules, single binary, no frontend yet (future Svelte 5).

Data flow: `sentiment` → `db` → `strategy` (sell→buy→risk) → `report`

New: `market` module provides global indices and stock quotes via Yahoo Finance v8 API.

Default config: **保守配置** (US 55%, CN 25%, Gold 20%)

See `references/ARCHITECTURE.md` for details.

## CLI Commands (v0.5.9)

| Category | Commands |
|----------|----------|
| Core | `init`, `config`, `cash` |
| Portfolio | `portfolio`, `add`, `buy`, `sell`, `price`, `remove` |
| Reports | `sentiment`, `report`, `history`, `update-prices` |
| Market | `market`, `market-indices`, `analyze <symbol>` |
| Backtest | `backtest run`, `backtest params` |

## Market Data Features

- **9 Global Indices**: S&P 500, Dow Jones, NASDAQ, VIX, FTSE 100, DAX, Nikkei 225, 上证指数, 恒生指数
- **Stock Analysis**: Basic quote info via `analyze <symbol>`
- **Free APIs**: No authentication required (Yahoo Finance v8, CNN)
- **Rate Limits**: ~5 requests/minute, 500/day estimated