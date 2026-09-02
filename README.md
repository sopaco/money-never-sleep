# MNS - Money Never Sleeps

> **AI-Powered Contrarian Investment Decision Assistant**  
> Overcome human weaknesses, systematically execute "buy in fear, sell in greed"

<p align="center">
    <a href="https://github.com/sopaco/money-never-sleep/tree/main/.terrain/human"><img alt="Litho Docs" src="https://img.shields.io/badge/Litho-Docs-green?logo=Gitbook&color=%23008a60"/></a>
    <a href="http://clawhub.ai/sopaco/money-never-sleep"><img alt="ClawHub" src="https://img.shields.io/badge/ClawHub-Certified-blue"/></a>
    <a href="https://github.com/openclaw/openclaw"><img alt="OpenClaw Compatible" src="https://img.shields.io/badge/OpenClaw-Compatible-brightgreen"/></a>
</p>

---

## The biggest enemy of investing is not the market, but human nature

Fear prevents buying when prices drop, greed prevents selling when prices rise—this is the instinctive dilemma every investor faces. MNS digitizes contrarian investment strategies, replacing emotion with rules, letting data drive decisions:

- 🎯 **Automatically suggest buying during extreme fear**, not panic selling
- 💰 **Remind to take profit when annualized returns meet targets**, not chasing highs
- ⚠️ **Smart alerts when positions are underwater**, not ignoring losses
- 📊 **Honest backtest reporting**: better risk-adjusted than buy-and-hold (Calmar 1.17 vs 0.98), but **lower returns**

---

## Core Advantages

### 🧠 Based on Real Market Data
Integrated CNN Fear & Greed Index, real-time market sentiment sensing, automatically saves historical snapshots for retrospective analysis.

### 📈 Validated Strategy Parameters
Backtested on 2016-2025 **real total-return data** (CNY-denominated, dividends included), **net of transaction costs, tiered redemption fees and cash yield**.
Default allocation: US 55%, CN 25%, Gold 20%; target risk-asset weight moves between 35% and 85% with sentiment.

| Strategy | Annualized (XIRR) | Max Drawdown | Calmar | Trades/yr |
|---|---|---|---|---|
| Target-weight (trend anchor) | 12.75% | 10.91% | **1.17** | 8.0 |
| Buy & hold | **14.60%** | 14.93% | 0.98 | 3.2 |

⚠️ **Read this first**: buy-and-hold beats this strategy by ~1.9pp annualized. The tool's value
holds only on a risk-adjusted basis (~4pp lower drawdown), and prolonged bear markets
(2000-2002, 2008) are not covered by the available data — yet "bear-market protection" is
precisely this strategy's main selling point, so that claim remains untested.

**⚠️ More importantly — the 12.75%/Calmar 1.17 above is the "trend anchor" backtest result, but
`mns report`'s live default is the "sentiment anchor" (no trend judgment, maps the Fear & Greed
score straight to a target weight); its in-sample figures are 9.79%/Calmar 1.07. Out-of-sample
(walk-forward) validation shows even that risk-adjusted edge is not robust:**

| Out-of-sample (from 2022-11) | Annualized | Max Drawdown | Calmar |
|---|---|---|---|
| Default config (sentiment anchor — `mns report`'s live behavior) | 7.91% | 9.18% | 0.86 |
| Default config (trend anchor — backtest-only, not wired into live report) | 10.79% | 11.47% | 0.94 |
| In-sample-tuned best (sentiment anchor) | 6.76% | 7.36% | 0.92 |
| **Buy & hold** | **12.23%** | 11.95% | **1.02** |

Out-of-sample, buy-and-hold beats both anchor configs on **both return and Calmar** — the
in-sample "risk-adjusted edge" does not extrapolate. The bootstrap distribution (printed by
`mns backtest validate`) shows this gap sits well inside statistical noise. `mns backtest
validate` also breaks out 2018 and 2022 — the only two clearly-down years in the dataset — year
by year; both anchors do show a smaller drawdown than buy-and-hold in those two years, but that
**cannot substitute for a multi-year bear-market stress test** like 2000-2002 or 2008 — that data
isn't available, so "bear-market protection" remains an under-tested claim. Run `mns backtest
validate` yourself to reproduce every number above — don't rely only on the in-sample
trend-anchor numbers `mns backtest` shows by default.

### 🔄 Buy/Sell Awareness
Cash recovered from selling automatically counts toward buying budget, calculate sell first then buy, maximizing capital utilization.

### 🛡️ Dual Profit-Taking Mechanism
Annualized return target OR absolute return ≥30%, two ways to lock in profits, not missing long-term compounding effects.

### 🎯 Contrarian Buying Logic
The more underwater, the more suggested to buy (with weight cap), truly achieving "be greedy when others are fearful".

---

## AI-Era Human-Machine Collaboration: OpenClaw + SKILL

MNS is designed for AI-assisted development, with two built-in knowledge systems:

| System | For | Purpose |
|--------|-----|---------|
| `.agents/skills/` / `.claude/skills/` | AI Coding Agent | Skill definitions (e.g. `mns-backtest`) for the backtest workflow |
| `.terrain/agent/` | AI Coding Agent | Project architecture, module boundaries, core flows (Terrain knowledge assets) |
| `.terrain/human/` | Human Developers | Project overview, architecture, workflows, deep dives |

### SKILL Synergy Effect

When you speak keywords (like "backtest"), AI automatically activates the corresponding SKILL, instantly gaining complete contextual understanding:

- ✅ **No need to explain project background**, AI already understands architecture and constraints
- ✅ **Safe strategy parameter modifications**, AI knows where and how to change without breaking other modules
- ✅ **Automatically follows existing patterns when adding features**, maintaining code consistency
- ✅ **Quickly locate root causes when debugging**, AI has complete data flow and call relationships

> 💡 **One sentence triggers SKILL**: Tell your AI "I want to optimize profit-taking parameters" or "help me backtest this strategy", and it will automatically read
> [`.agents/skills/mns-backtest/SKILL.md`](.agents/skills/mns-backtest/SKILL.md) and gain complete project knowledge.

---

## Quick Start

```bash
# Install
cargo build --release
or
cargo build --release --target x86_64-unknown-linux-musl

# Initialize
mns init
mns cash set 100000

# Daily usage
mns add QQQ "Nasdaq 100" us_stocks
mns buy QQQ 50 380.00
mns update-prices
mns report          # Generate today's action suggestions
mns backtest          # Compare 4 strategies (same data & cost model)
mns backtest validate # Out-of-sample validation + bootstrap + holdout
```

---

## Documentation Navigation

| Documentation | Description |
|---------------|-------------|
| [.terrain/human/](.terrain/human/) | Human-friendly docs: project overview, architecture, workflows |
| [.agents/skills/mns-backtest/SKILL.md](.agents/skills/mns-backtest/SKILL.md) | Backtest skill: AI-facing steps, parameters, data-file reference |
| [AGENTS.md](AGENTS.md) | AI Coding Agent working guidelines |

---

## Design Philosophy

```
Fear × Greed = Loss
Rules × Discipline = Profit
```

MNS doesn't predict markets, doesn't execute trades, it does one thing: **at critical moments, provide systematic decision support**.

The rest, is up to you.

---

**With OpenClaw + MNS, let AI be your investment decision partner.**
