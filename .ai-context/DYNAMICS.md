# Dynamics — Active Issues & Constraints

**Last updated**: 2026-04-21

---

## Current Blockers / Known Issues

### 1. CNN API may be inaccessible in some network environments
**Severity**: Environmental
**Detail**: The CNN Fear & Greed API (`production.dataviz.cnn.io`) may timeout in certain network setups (tested: Windows PowerShell environment). The code is correct (200 response on actual API), but external network conditions can cause timeouts.
**Workaround**: None — this is an external API with no authentication. If it goes down permanently, need to find alternative data source.
**Reported**: 2026-04-19

### 2. Absolute return threshold (30%) is hardcoded
**Severity**: Low
**Detail**: The 30% absolute return threshold for long-term profit-taking is not configurable via TOML. Should be added to `[settings]` as `absolute_profit_target`.
**Workaround**: Edit `strategy.rs` directly to change the `0.30` constant.
**Reported**: 2026-04-19

### 3. Single sentiment index for multi-market portfolio
**Severity**: Design limitation
**Detail**: CNN Fear & Greed reflects US market sentiment only. Using it to drive buy/sell for A-shares and counter-cyclical assets may produce suboptimal signals (US fear ≠ China fear). No alternative data source implemented.
**Workaround**: User interprets suggestions with judgment; counter-cyclical assets may move inversely to US sentiment by design.
**Reported**: 2026-04-19

### 4. QDII基金估值数据可能缺失
**Severity**: Low
**Detail**: 天天基金接口对部分QDII基金（如暂停申购的016668）不提供实时估值数据，`update-prices` 命令会跳过这类资产。
**Workaround**: 用户需手动使用 `mns price <code> <价格>` 更新此类基金价格。
**Reported**: 2026-04-21

---

## Recently Added Features

### 自动更新资产价格 (`mns update-prices`)
**Added**: 2026-04-21
**Detail**: 新增 `mns update-prices` 命令，自动获取所有持仓资产的当前价格：
- 国内基金（6位数字代码）：使用天天基金接口 `fundgz.1234567.com.cn`
- 美股/ETF（字母代码）：使用 Yahoo Finance API
- 失败时跳过该资产，继续处理其他资产
- 显示更新结果表格（代码、名称、原价格、新价格、来源）

---

## Recently Resolved

### 4. Strategy threshold mismatch with PRD
**Resolved**: 2026-04-19
**Detail**: `buy_ratio_for()` and `sell_ratio_for()` were missing the "Neutral" sentiment zone — scores in 45-55 range were incorrectly bucketed into Greed/Fear. Fixed by adding proper 5-zone and 3-zone logic respectively. Added `sell_ratio.neutral_target_high` config field.

### 5. Short-term annualized return distortion
**Resolved**: 2026-04-19
**Detail**: Positions held < 30 days could show extreme annualized returns (e.g., 1-day 1% gain → 3678% annualized), triggering aggressive sell suggestions. Fixed by adding `min_holding_days` threshold (default 30) — positions below threshold show N/A for annualized.

### 6. Buy distribution "winner-take-more" effect
**Resolved**: 2026-04-19
**Detail**: `distribute_amount()` used market-value weighting, giving more funds to already-winning positions. Replaced with `distribute_amount_contrarian()` using weight = `max(1.0, cost/current)` — underwater positions get higher allocation.

### 7. Buy/sell independent computation
**Resolved**: 2026-04-19
**Detail**: Buy suggestions didn't account for sell proceeds. Pipeline now computes sell first, then passes sell proceeds to buy calculation. Report includes "净操作指引" (net operation guidance).

### 8. Non-transactional DB operations
**Resolved**: 2026-04-19
**Detail**: `buy_position()` and `sell_position()` executed 3 SQL statements without transaction. Now wrapped in `unchecked_transaction()` for atomicity. Also added input validation (non-negative cash, positive shares/price).

### 9. Duplicate daily snapshots
**Resolved**: 2026-04-19
**Detail**: Running `mns report` multiple times per day created duplicate `fear_greed_snapshots` rows. Now uses DELETE+INSERT to keep only the latest snapshot per day.

---

## Constraints

- **No frontend yet** — PRD mentions Svelte 5 for future dashboard, but currently pure CLI only
- **Single-user only** — SQLite, no auth, no multi-portfolio support
- **Windows-first tested** — developed and tested on Windows PowerShell, though Rust code is cross-platform
- **Existing config files lack new fields** — users with pre-existing `config.toml` must run `mns init` or manually add `min_holding_days` and `neutral_target_high`
