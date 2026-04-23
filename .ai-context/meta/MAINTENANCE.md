# Maintenance Guide — MNS AI Context

**Last updated**: 2026-04-23

> **2026-04-23**: 投资策略文档对齐修复：重命名"最优配置"为"历史激进配置"，统一配置命名为"防御配置"，修正卖出矩阵表格，精简SKILL内容
> **2026-04-23**: 完成项目一致性review：修正SKILL.md和strategy.md中的参数表格，添加mns-backtest预设配置详细说明，更新.ai-context中的回测验证数据
> **2026-04-23**: 新增市场数据功能 (v0.5.10)：market.rs 模块、market/analyze/market-indices 命令、Yahoo Finance v8 API；更新 DECISIONS.md 添加 Decision 17
> **2026-04-22**: 验证默认配置为防御配置 (US 55%, CN 25%, Gold 20%)，修正 .ai-context 与代码一致
> **2026-04-21**: 切换恐贪指数数据源从 alternative.me (crypto) 到 CNN (stock market) (sentiment.rs, Cargo.toml, .ai-context/*)

---

## When to Update

| File | Update Trigger |
|------|----------------|
| `DYNAMICS.md` | Any new issue, blocker, or workaround discovered |
| `DECISIONS.md` | Any new design decision or parameter change |
| `ARCHITECTURE.md` | New module, changed data flow, new dependencies |
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

All in `~/.mns/.ai-context/`

```
.ai-context/
├── SKILL.md                    ← Entry point (don't change purpose)
├── DYNAMICS.md                 ← Issues (update frequently)
└── references/
    ├── PROJECT-ESSENCE.md     ← Core identity (rarely changes)
    ├── ARCHITECTURE.md         ← Component map (monthly review)
    └── DECISIONS.md            ← Design rationale (per-change)
```

## Related Documentation

- `PRD.txt` — Full product spec
- `README.md` — User-facing quick start
- `.agents/skills/mns-backtest/SKILL.md` — Backtest skill
- `.agents/skills/mns-backtest/STRATEGY_OPTIMIZATION.md` — Optimization results
