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

- **Language**: Rust (edition 2024) | **Database**: SQLite | **CLI**: `clap` v4
- **Config**: `~/.mns/config.toml` | **Data**: `~/.mns/mns.db` | **Reports**: `./reports/`
- **Default**: 防御配置 (US 55%, CN 25%, Gold 20%)

## CLI Commands

| Category | Commands |
|----------|----------|
| Core | `init`, `config`, `cash` |
| Portfolio | `portfolio`, `add`, `buy`, `sell`, `price`, `remove` |
| Reports | `sentiment`, `report`, `history`, `update-prices` |
| Market | `market`, `market-indices`, `analyze <symbol>` |
| Backtest | `backtest run`, `backtest params` |