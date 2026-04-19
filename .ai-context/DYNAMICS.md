# Dynamics — Active Issues & Constraints

**Last updated**: 2026-04-19

---

## Current Blockers / Known Issues

### 1. CNN API may be inaccessible in some network environments
**Severity**: Environmental
**Detail**: The CNN Fear & Greed API (`production.dataviz.cnn.io`) may timeout in certain network setups (tested: Windows PowerShell environment). The code is correct (200 response on actual API), but external network conditions can cause timeouts.
**Workaround**: None — this is an external API with no authentication. If it goes down permanently, need to find alternative data source.
**Reported**: 2026-04-19

---

## Recently Resolved

### 2. CLI subcommand parsing for `cash set/add`
**Resolved**: 2026-04-19
**Detail**: Initial design had `Cash` and `CashOp` as separate subcommands, causing `mns cash set` to fail. Fixed by using `Cash { action: Option<CashAction> }` pattern — `mns cash` alone shows balance, `mns cash set X` uses the subcommand.

---

## Constraints

- **No frontend yet** — PRD mentions Svelte 5 for future dashboard, but currently pure CLI only
- **Single-user only** — SQLite, no auth, no multi-portfolio support
- **Windows-first tested** — developed and tested on Windows PowerShell, though Rust code is cross-platform
