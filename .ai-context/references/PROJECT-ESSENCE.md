# Project Essence — MNS

**Last updated**: 2026-04-23

---

## What

MNS (Money Never Sleeps, Market Neutral Strategist) is a personal CLI tool that monitors market sentiment (Fear & Greed Index), analyzes portfolio positions against return targets, and generates daily buy/sell recommendations. Now includes market data features (global indices, stock analysis), backtesting with optimized parameters, and multi-asset portfolio support.

## Why

逆向投资 — buy when fearful, sell when greedy. Most investors struggle with emotional discipline. This tool provides systematic, rules-based signals so the user makes decisions without panic.

## Who

个人投资者，需要一套系统化的投资决策辅助工具，包含市场概览、历史回测验证策略有效性。

## What It Does

1. **Fetches** Fear & Greed Index via CNN API (股票市场，0-100)
2. **Reads** current cash balance, positions, and cost basis from SQLite
3. **Calculates** sell/buy suggestions based on:
   - Fear & Greed zone → whether/how much to sell or buy
   - Annualized return (with min holding days) vs 10%/15% targets → profit-taking
   - Absolute return ≥ 30% → long-term profit-taking
   - Contrarian buy weighting → underwater positions get more allocation
4. **Outputs** text report with sell/buy/risk/net operation guidance
5. **Auto-updates** prices via `mns update-prices`
6. **Shows market overview** via `mns market` (9 global indices + fear/greed)
7. **Analyzes individual stocks** via `mns analyze <symbol>`
8. **Backtests** strategy effectiveness with historical data (2016-2025)
9. **Compares** multiple config variants in single run

## What It Does NOT Do

- No push notifications (future: external programs consume reports)
- No actual trading (user executes trades manually)
- No frontend (future: Svelte 5 dashboard)
- No auto-price-update by default

## Core Design Principles

- **Single binary** — no runtime dependencies, portable
- **SQLite for state** — portable, zero-config, transactional
- **TOML for config** — human-editable
- **Text reports** — machine-readable, future-proof
- **Contrarian weighting** — buy favors underwater positions
- **Buy/sell aware** — sell proceeds flow into buy suggestions
- **Dual-criteria sell** — annualized + absolute return triggers
- **Sentiment-aware risk** — different advice per market zone
- **Optimized defaults** — conservative config: US 55%, CN 25%, Gold 20%
- **Free data sources** — Yahoo Finance v8, CNN API, 天天基金; no API keys

## New Features (v0.5.9)

### Market Data Commands
- `mns market` — Combined view: 9 global indices + Fear & Greed Index
- `mns market-indices` — Dedicated global indices query
- `mns analyze <symbol>` — Individual stock analysis (quote + valuation placeholder)

### Supported Market Indices
| Region | Indices |
|--------|---------|
| US | S&P 500, Dow Jones, NASDAQ, VIX |
| Europe | FTSE 100 (UK), DAX (Germany) |
| Asia | Nikkei 225 (Japan), 上证指数, 恒生指数 |

## Optimized Configuration (Default)

Based on historical backtesting (2016-2025):
- **年化收益**: 8-9% (逆向策略) vs 11% (买入持有)
- **最大回撤**: 21-28% (逆向策略) vs 15% (买入持有)

Key parameters (保守配置):
- US stocks: 55%
- CN stocks: 25%
- Gold: 20%
- Extreme fear buy: 60%
- Annualized targets: 10% / 15%
- Min holding days: 45
- Expected: 7-8% annual, 13-18% drawdown