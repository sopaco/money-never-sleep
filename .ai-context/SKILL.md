# AI Context — MNS Project

> **Activation triggers** — Read this directory when:
> - Starting a coding session in `d:\Workspace\toys\MNS`
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

# Initialize
mns init

# Common workflow
mns cash set 100000
mns add QQQ "纳指100" us_stocks
mns buy QQQ 100 350.00
mns price QQQ 420.00
mns portfolio
mns report
```

## File Map

| File | Purpose |
|------|---------|
| `SKILL.md` | This file — entry point |
| `references/PROJECT-ESSENCE.md` | What & why |
| `references/ARCHITECTURE.md` | Component relationships |
| `references/DECISIONS.md` | Design decisions |
| `DYNAMICS.md` | Active issues |
| `meta/MAINTENANCE.md` | How to update |

## Key Facts

- **Language**: Rust (edition 2021)
- **Database**: SQLite via `rusqlite` (bundled)
- **HTTP**: `reqwest` for CNN Fear & Greed API
- **CLI**: `clap` v4 (derive mode)
- **Config**: TOML at `~/.mns/config.toml`
- **Data**: SQLite at `~/.mns/mns.db`
- **Reports**: `./reports/YYYY-MM-DD.txt`

## Architecture Summary

8 source modules, single binary, no frontend yet (future Svelte 5).

Data flow: `sentiment` → `db` → `strategy` (sell→buy→risk) → `report`

See `references/ARCHITECTURE.md` for details.
