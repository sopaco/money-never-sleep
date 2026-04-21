# Project Essence — MNS

**Last updated**: 2026-04-21

---

## What

MNS (Money Never Sleeps, Market Neutral Strategist) is a personal CLI tool that monitors market sentiment (Fear & Greed Index), analyzes portfolio positions against return targets, and generates daily buy/sell recommendations. Now includes backtesting with optimized parameters.

## Why

逆向投资 (contrarian investing) — buy when fearful, sell when greedy. Most investors struggle with emotional discipline. This tool provides systematic, rules-based signals so the user makes decisions without panic.

## Who

个人投资者，需要一套系统化的投资决策辅助工具，包含历史回测验证策略有效性。

## What It Does

1. **Fetches** Fear & Greed Index via CNN API (股票市场，0-100)
2. **Reads** current cash balance, positions, and cost basis from SQLite
3. **Calculates** sell/buy suggestions based on:
   - Fear & Greed zone → whether/how much to sell or buy
   - Annualized return (with min holding days) vs 15%/22% targets → profit-taking
   - Absolute return ≥ 30% → long-term profit-taking
   - Contrarian buy weighting → underwater positions get more allocation
4. **Outputs** text report with sell/buy/risk/net operation guidance
5. **Auto-updates** prices via `mns update-prices`
6. **Backtests** strategy effectiveness with historical data (2016-2025)

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

## Optimized Configuration (Default)

Based on historical backtesting (2016-2025):
- **年化收益**: 8-9%
- **最大回撤**: 16-21%
- **收益/回撤比**: 0.42+

Key parameters:
- US stocks: 55% (reduced risk)
- CN stocks: 25% (dividend low-vol)
- Gold: 20% (enhanced hedge)
- Extreme fear buy: 60% of cash
- Annualized targets: 10% / 15%
- Min holding days: 45
