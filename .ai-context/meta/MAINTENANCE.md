# Maintenance Guide — MNS AI Context

**Last updated**: 2026-04-21

---

## When to Update

| File | Update Trigger |
|------|---------------|
| `DYNAMICS.md` | Any new issue, blocker, or workaround discovered |
| `DECISIONS.md` | Any new design decision or parameter change |
| `ARCHITECTURE.md` | New module (e.g., `quote.rs`), changed data flow, new dependencies |
| `PROJECT-ESSENCE.md` | Mission change, new target user, dropped feature |
| `SKILL.md` | Only if activation rules change |

## How to Audit

1. Read `.ai-context/SKILL.md` entry conditions
2. Compare against current code — any new modules? Changed data flow?
3. Check `DYNAMICS.md` — any resolved issues to archive?
4. Check `DECISIONS.md` — any new parameters in `config.rs` that need documenting?

## Token Budget

Total should stay under ~4000 tokens across all files. If approaching limit:
- Trim `ARCHITECTURE.md` detail
- Move rarely-used details to code comments (link to file instead)
- Keep `DYNAMICS.md` lean — only current state

## File Locations

All in `d:\Workspace\toys\MNS\.ai-context\`

```
.ai-context/
├── SKILL.md                    ← Entry point (don't change purpose)
├── DYNAMICS.md                 ← Issues (update frequently)
└── references/
    ├── PROJECT-ESSENCE.md     ← Core identity (rarely changes)
    ├── ARCHITECTURE.md         ← Component map (monthly review)
    └── DECISIONS.md            ← Design rationale (per-change)

# New modules in src/: quote.rs (auto price fetch)
```

## Related Documentation

- `PRD.txt` — Full product spec (V2.1, 2026-04-19)
- `README.md` — User-facing quick start
- `.agents/skills/mns-backtest/SKILL.md` — Backtest skill (separate)
