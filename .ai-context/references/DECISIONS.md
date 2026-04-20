# Design Decisions — MNS

**Last updated**: 2026-04-21

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
**Choice**: `annualized = (current_price / cost_price) ^ (365 / holding_days) - 1`, with `min_holding_days` threshold (default 30).
**Rationale**:
- Normalizes across positions held for different lengths
- Directly maps to the user's 10%-15% annual target
- Min days threshold prevents short-term noise from triggering extreme sell suggestions
**Trade-off**: Positions held < 30 days show N/A for annualized — but absolute return still available as fallback.

---

## Decision 7: Two-dimensional sell matrix (zone × return), 3×3

**Context**: Simple "sell when greedy" is too blunt. Need to consider both market sentiment AND whether the position has actually earned its target return.

**Matrix**:

```
              ≥15% annual   10-15%       <10%
Extreme Greed    50%          30%        20%
Greed            40%          20%         0% (hold)
Neutral          30%          0%         0% (hold)
Fear/EFear       0%           0%         0% (hold)
```

**Rationale**: Full 3-zone matrix (neutral/greed/extreme_greed) matches PRD spec. Fear zones never trigger sell — contrarian strategy holds through fear.
**Trade-off**: More parameters to tune. User can adjust via config.

---

## Decision 8: Absolute return ≥ 30% as secondary sell trigger

**Context**: A position held 5 years may have 50% absolute gain but only 8.4% annualized — below the 10% target. It should still be a candidate for profit-taking.
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
- Report shows net operation direction (net buy / net sell / hold)
- More realistic view of what the user would actually do today
**Trade-off**: Slightly more complex pipeline order (must compute sell before buy).

---

## Decision 11: 浮亏 > 20% triggers sentiment-aware warning, NOT automatic sell

**Context**: Contrarian strategy means buying during fear — positions may go negative before recovering.
**Choice**: Only warn, never auto-sell. Warning advice varies by market zone:
- Fear: "可能是加仓机会" (consider buying more)
- Neutral: "审视基本面" (review fundamentals)
- Greed: "紧急审视" (urgent review — market up but this position is down)
**Rationale**: Selling at -20% locks in losses. Different sentiment contexts require different responses.
**Trade-off**: If a position's fundamentals deteriorate, the user still needs to manually decide to exit.

---

## Decision 12: SQLite transactions for buy/sell operations

**Context**: Original `buy_position()` and `sell_position()` executed 3 separate SQL statements without transaction — crash between steps could corrupt data.
**Choice**: Wrap all DB operations in `unchecked_transaction()`.
**Rationale**: Atomicity guarantee — either all changes commit or none do.
**Trade-off**: Minimal performance impact for a single-user CLI tool.

---

## Decision 13: Multiple price data sources (Tian Tian Fund + Yahoo Finance)

**Context**: Needed a way to automatically update asset prices. Users hold both Chinese funds and US ETFs.
**Choice**: Use two data sources based on code pattern:
- 6-digit numeric codes → Tian Tian Fund (`fundgz.1234567.com.cn`)
- Letter codes → Yahoo Finance (`query1.finance.yahoo.com`)
**Rationale**:
- Tian Tian Fund is the de facto standard for Chinese fund data, free and reliable
- Yahoo Finance covers global markets including US ETFs
- Code pattern detection is simple and reliable
**Trade-off**: Some QDII funds may not have real-time estimates on Tian Tian Fund; users must manually update those.
