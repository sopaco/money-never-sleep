# Design Decisions — MNS

**Last updated**: 2026-04-19

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

## Decision 6: Annualized return as the sell trigger metric

**Context**: Needed a way to determine if a position has "earned enough" to take profit.
**Choice**: `annualized = (current_price / cost_price) ^ (365 / holding_days) - 1`
**Rationale**:
- Normalizes across positions held for different lengths
- Directly maps to the user's 10%-15% annual target
- Hard to game (market-beating short-term gains still show low annualization if held briefly)
**Trade-off**: Breaks down for very short holding periods (< 30 days) — results flagged as N/A.

---

## Decision 7: Two-dimensional sell matrix (zone × return)

**Context**: Simple "sell when greedy" is too blunt. Need to consider both market sentiment AND whether the position has actually earned its target return.

**Matrix**:

```
              ≥15% annual   10-15%       <10%
Extreme Greed    50%          30%        20%
Greed            40%          20%         0% (hold)
Neutral          30%          0%         0% (hold)
```

**Rationale**: Backtested against 2016-2025 data — this combination outperformed simple fear/greed binary.
**Trade-off**: More parameters to tune. User can adjust via config.

---

## Decision 8:浮亏 > 20% triggers warning, NOT automatic sell

**Context**: Contrarian strategy means buying during fear — positions may go negative before recovering.
**Choice**: Only warn, never auto-sell.
**Rationale**: Selling at -20% locks in losses. The tool's whole purpose is to help user resist panic selling.
**Trade-off**: If a position's fundamentals deteriorate, the user still needs to manually decide to exit.
