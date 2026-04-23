# Design Decisions — MNS

**Last updated**: 2026-04-23

---

## Decision 1: Rust as the primary language

**Context**: PRD specified Rust + Svelte 5 from the start.
**Choice**: Stick with Rust for the CLI binary.
**Rationale**: No runtime, single static binary, excellent SQLite/HTTP crates, fast execution.
**Trade-off**: Longer compile times, more verbose than Python/Go for prototyping.

---

## Decision 2: SQLite over JSON or CSV for state

**Context**: Needed persistent state for cash, positions, transactions.
**Choice**: SQLite via `rusqlite` (bundled).
**Rationale**:
- Zero-config, portable
- Structured queries vs flat files
- Built-in transaction support
- No server needed (personal tool)
**Trade-off**: Not great for multi-user, but fine for single-user CLI.

---

## Decision 3: TOML for configuration over YAML/JSON

**Context**: User-editable config with nested sections.
**Choice**: TOML via `toml` crate.
**Rationale**: Native Rust support, cleaner than JSON for nested config, user-friendly.
**Trade-off**: Not as widely used as YAML in DevOps contexts, but fine for a Rust CLI tool.

---

## Decision 4: Text reports over push notifications

**Context**: User said "no push, just text report to a directory, other programs will call it".
**Choice**: Reports saved as `reports/YYYY-MM-DD.txt`.
**Rationale**:
- Maximum flexibility — any program can read .txt
- No vendor lock-in (no WeChat/Telegram dependency)
- Human-readable, debuggable
- Zero infrastructure
**Trade-off**: User must check the file manually or have another program poll the directory.

---

## Decision 5: Weighted average cost for position updates

**Context**: User can buy the same asset multiple times at different prices.
**Choice**: `cost_price = (old_shares × old_cost + new_shares × new_price) / (old_shares + new_shares)`
**Rationale**: Standard portfolio accounting method, IRS-approved for tax lots.
**Trade-off**: Loses individual lot information (not needed for this tool's purpose).

---

## Decision 6: Annualized return as the primary sell trigger, with min holding days guard

**Context**: Needed a way to determine if a position has "earned enough" to take profit.
**Choice**: `annualized = (current_price / cost_price) ^ (365 / holding_days) - 1`, with `min_holding_days` threshold (default 21).
**Rationale**:
- Normalizes across positions held for different lengths
- Directly maps to the user's 15%-22% annual target
- Min days threshold prevents short-term noise
**Trade-off**: Positions held < 21 days show N/A for annualized — but absolute return still available.

---

## Decision 7: Two-dimensional sell matrix (zone × return), 3×3

**Context**: Simple "sell when greedy" is too blunt. Need to consider both market sentiment AND whether the position has actually earned its target return.

**Matrix** (对应 `config.rs` 默认值):

```
              ≥15% annual   10-15%      <10%
Extreme Greed    50%          30%        20%
Greed            40%          25%        hold
Neutral          15%          hold       hold
Fear/EFear       hold         hold       hold
```

**Rationale**: Full 3-zone matrix matches PRD spec. Fear zones never trigger sell — contrarian strategy holds through fear.
**Trade-off**: More parameters to tune. User can adjust via config.

---

## Decision 8: Absolute return ≥ 30% as secondary sell trigger

**Context**: A position held 5 years may have 50% absolute gain but only 8.4% annualized — below the 15% target. It should still be a candidate for profit-taking.
**Choice**: If absolute return ≥ 30%, trigger sell in greedy environments even if annualized is below target.
**Rationale**: Long-term positions with substantial unrealized gains deserve protection regardless of annualized rate.
**Trade-off**: Hardcoded 30% threshold (could be made configurable).

---

## Decision 9: Contrarian buy distribution over market-value-weighted

**Context**: Original code distributed buy funds proportional to market value — winners got more, losers got less.
**Choice**: Use contrarian weighting: `weight = max(1.0, cost_price / current_price)`.
**Rationale**:
- Underwater positions get more funds (higher weight), aligning with contrarian philosophy
- Market-value weighting created a "winner-take-more" effect contradicting the strategy
- Equal minimum weight (1.0) ensures winning positions still get some allocation
**Trade-off**: May over-allocate to fundamentally broken positions — user must review risk warnings.

---

## Decision 10: Sell-first, buy-second pipeline with proceeds awareness

**Context**: Original code computed buy and sell independently — same day could show "buy ¥50k" and "sell 40%" with no connection.
**Choice**: Compute sell suggestions first, then pass sell proceeds into buy calculation.
**Rationale**:
- Available cash for buying = current balance + sell proceeds
- Report shows net operation direction
- More realistic view of what the user would actually do today
**Trade-off**: Slightly more complex pipeline order (must compute sell before buy).

---

## Decision 11: 浮亏 > 20% triggers sentiment-aware warning, NOT automatic sell

**Context**: Contrarian strategy means buying during fear — positions may go negative before recovering.
**Choice**: Only warn, never auto-sell. Warning advice varies by market zone:
- Fear: "可能是加仓机会" (consider buying more)
- Neutral: "审视基本面" (review fundamentals)
- Greed: "紧急审视" (urgent review)
**Rationale**: Selling at -20% locks in losses. Different sentiment contexts require different responses.
**Trade-off**: If a position's fundamentals deteriorate, the user still needs to manually decide to exit.

---

## Decision 12: SQLite transactions for buy/sell operations

**Context**: Original `buy_position()` and `sell_position()` executed 3 separate SQL statements without transaction — crash between steps could corrupt data.
**Choice**: Wrap all DB operations in `unchecked_transaction()`.
**Rationale**: Atomicity guarantee — either all changes commit or none do.
**Trade-off**: Minimal performance impact for a single-user CLI tool.

---

## Decision 13: CNN API for stock market Fear & Greed Index

**Context**: Need stock market Fear & Greed Index, not crypto sentiment. alternative.me provides crypto Fear & Greed Index, which is not appropriate for stock portfolio decisions.
**Choice**: Direct HTTP request to CNN API (`https://production.dataviz.cnn.io/index/fearandgreed/graphdata`).
**Rationale**:
- CNN API provides stock market sentiment (not crypto)
- Free, no authentication required
- Real-time updates
- Direct control over request headers
**Trade-off**: CNN API may block requests (418 error), requires proper User-Agent header to simulate browser request.

---

## Decision 14: Defensive configuration as default

**Context**: Original default config was too aggressive, leading to higher drawdowns. After backtesting, conservative config provides better risk-adjusted returns.
**Choice**: Default to defensive config (US 55%, CN 25%, Gold 20%) based on backtesting.
**Rationale**:
- Lower drawdown priority: 13-18% vs 23-28% for aggressive
- Better risk-adjusted returns: similar 7-8% annual with lower volatility
- Users who prefer higher returns can manually switch to aggressive config
- 保守配置（中性小额买入）仍可手动启用
- Longer holding periods (45 days) reduce short-term noise
**Trade-off**: Lower absolute returns vs aggressive, but better sleep quality.

---

## Decision 15: Backtest embedded historical data

**Context**: Need historical data for strategy validation without external dependencies.
**Choice**: Embed CSV data in binary via `include_str!` macro.
**Rationale**:
- Zero runtime dependencies for backtest
- Data travels with binary
- No network calls during backtest
**Trade-off**: Larger binary size, data updates require recompilation.

---

## Decision 16: Multiple backtest config variants

**Context**: Need to compare different strategy parameters to find optimal configuration.
**Choice**: Support loading multiple config files for comparison backtests.
**Rationale**:
- Systematic parameter exploration
- Clear comparison of risk/return tradeoffs
- Repeatable experiments
**Trade-off**: More complex backtest module.

---

## Decision 17: Yahoo Finance v8 API for market data

**Context**: Need reliable, free market data for global indices and stock quotes.
**Choice**: Use Yahoo Finance v8 API (`query1.finance.yahoo.com/v8/finance/chart`).
**Rationale**:
- Free, no authentication required
- Covers global indices (US, Europe, Asia)
- Real-time-ish quotes (15-20 min delay)
- Well-documented API structure
**Trade-off**:
- Rate limits (~5 requests/min, 500/day estimated)
- 15-20 minute quote delay
- May require proper User-Agent headers to avoid blocking
- API is unofficial and may change without notice
