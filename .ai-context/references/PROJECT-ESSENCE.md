# Project Essence — MNS

**Last updated**: 2026-04-21

---

## What

MNS (Money Never Sleeps，Market Neutral Strategist) is a personal CLI tool that monitors the CNN Fear & Greed Index, analyzes portfolio positions against return targets, and generates daily buy/sell recommendations as text reports.

## Why

逆向投资 (contrarian investing) — buy when fearful, sell when greedy. Most investors struggle with emotional discipline. This tool provides systematic, rules-based signals so the user makes decisions without panic.

## Who

个人投资者，不需要复杂的风控系统，只需要每日一份文本报告告诉"今天该怎么操作"。

## What It Does

1. **Fetches** CNN Fear & Greed Index (0-100 score + rating)
2. **Reads** current cash balance, positions, and cost basis from SQLite
3. **Calculates** sell/buy suggestions based on:
   - Fear & Greed zone → whether/how much to sell or buy
   - Annualized return (with min holding days threshold) vs 10%/15% targets → whether to take profit
   - Absolute return ≥ 30% → long-term profit-taking even if annualized is below target
   - Contrarian buy weighting → underwater positions get more allocation
4. **Outputs** a text report with sell suggestions, buy suggestions, risk warnings, net operation guidance, and allocation presets
5. **Auto-updates** asset prices via `mns update-prices` (Tian Tian Fund for CN funds, Yahoo Finance for US ETFs)

## What It Does NOT Do

- No push notifications (future: external programs consume the text report)
- No actual trading (user executes trades manually)
- No frontend (future: Svelte 5 dashboard)
- No auto-price update by default (user must run `mns update-prices` manually)

## Core Design Principles

- **Single binary** — no runtime dependencies, portable
- **SQLite for state** — portable, zero-config, transactional
- **TOML for config** — human-editable, no special tooling
- **Text reports** — machine-readable, future-proof, zero coupling with push systems
- **Contrarian weighting** — buy distribution favors underwater positions over winners
- **Buy/sell aware** — sell proceeds flow into buy suggestions for net operation guidance
- **Dual-criteria sell** — both annualized return and absolute return can trigger profit-taking
- **Sentiment-aware risk** — same -20% drawdown gets different advice based on market mood
