# MNS - Money Never Sleeps

> **AI-Powered Contrarian Investment Decision Assistant**  
> Overcome human weaknesses, systematically execute "buy in fear, sell in greed"

<p align="center">
    <a href="https://github.com/sopaco/money-never-sleep/tree/main/.terrain/human"><img alt="Litho Docs" src="https://img.shields.io/badge/Litho-Docs-green?logo=Gitbook&color=%23008a60"/></a>
    <a href="http://clawhub.ai/sopaco/money-never-sleep"><img alt="ClawHub" src="https://img.shields.io/badge/ClawHub-Certified-blue"/></a>
    <a href="https://github.com/openclaw/openclaw"><img alt="OpenClaw Compatible" src="https://img.shields.io/badge/OpenClaw-Compatible-brightgreen"/></a>
</p>

---

> **Status update (2026-11-08)**: the default `mns backtest` is now a live-account audit
> (real fund NAVs, point-in-time signals, channel-level settlement, account mirror), not the
> 2016–2025 monthly-proxy strategy comparison described in the tables below. Those legacy
> numbers are kept for history only and **must not be used for live decisions** — the old
> engine is `mns backtest legacy`. Validation shows the sentiment signal has no statistically
> significant timing contribution (63rd percentile in a permutation test, below the 80th
> percentile threshold); constant 70–75% exposure is often equivalent or better with fewer
> trades. See [`docs/remediation-2026-11-08.md`](docs/remediation-2026-11-08.md) and the
> Chinese README for the current feature map and correct usage.

## The biggest enemy of investing is not the market, but human nature

Fear prevents buying when prices drop, greed prevents selling when prices rise—this is the instinctive dilemma every investor faces. MNS digitizes contrarian investment strategies, replacing emotion with rules, letting data drive decisions:

- 🎯 **Automatically suggest buying during extreme fear**, not panic selling
- ⚠️ **Warn when a position is more than 20% underwater**, with wording that depends on sentiment
- ⚠️ **Smart alerts when positions are underwater**, not ignoring losses
- 📊 **Honest backtest reporting**: better risk-adjusted than buy-and-hold (Calmar 1.17 vs 0.98), but **lower returns**

---

## Core Advantages

### 🧠 Based on Real Market Data
Integrated CNN Fear & Greed Index, real-time market sentiment sensing, automatically saves historical snapshots for retrospective analysis.

### 📈 Legacy Backtest Numbers (2026-11-08: demoted, do not use for live decisions)
The tables in this section come from the **old monthly proxy engine** (`mns backtest legacy`):
2016-2025 monthly bars, 3-month smoothing, month-end execution, ¥100k start plus ¥50k/year inflow.
They do not match the live account or the current signal path. Current evidence and usage are in
[`docs/remediation-2026-11-08.md`](docs/remediation-2026-11-08.md) and
[`.agents/skills/mns-backtest/SKILL.md`](.agents/skills/mns-backtest/SKILL.md).

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

### 🌊 Daily Signal Smoothing (noise reduction for long-term investors)

A daily Fear & Greed score is weekly-frequency noise driving a monthly-to-yearly portfolio.
The live decision path therefore applies three filters (see `src/signal.rs`, configured under
`[rebalance]` in `~/.mns/config.toml`):

| Key | Default | Meaning |
|---|---|---|
| `signal_smooth_days` | `7` | Decision score = mean of the last N daily scores (raw score is display-only) |
| `zone_confirm_days` | `3` | A zone switch takes effect only after the smoothed score holds the new zone for N consecutive days; otherwise the previous zone stays |
| `decision_weekday` | `5` | `mns report` outputs a full "decision report" only on that weekday (1=Mon…7=Sun, 0=daily); other days output an observation-only report; `mns report --decide` forces a decision report |

The old in-sample smoothing comparison (14.8 → 10.2 → 6.4 trades/year) has been **retracted**:
it used a non-active config with monthly signals and does not reproduce under the active
configuration. Do not cite it as evidence. Daily snapshots are stored locally
(`fear_greed_snapshots` in SQLite), so smoothing works offline and survives CNN API changes.

### 🔄 Buy/Sell Awareness
Cash recovered from selling automatically counts toward buying budget, calculate sell first then buy, maximizing capital utilization.

### 🛡️ No profit-taking rules in the current decision path
The "dual profit-taking" description in older docs is obsolete. `mns report` does not sell to
lock in profits: target weights depend only on the sentiment score, never on an individual
position's P&L. The only return-related behaviour is that `mns portfolio` highlights a position
whose annualized return crosses `settings.annualized_target_high`, and the decision report
shows a risk warning for positions more than 20% underwater.

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
mns cash deposit 10000       # external inflow (XIRR basis); `cash set` is a correction tool only
mns add 018966 "Nasdaq 100 feeder" us_stocks
mns buy 018966 <shares> <price>
mns config params            # full list of tunable parameters

# Weekly (default decision day: Friday)
mns today                    # refresh prices + report; observation-only on other days
mns performance              # read-only XIRR / actual max drawdown

# Monthly
mns plan --monthly 3000      # buy-only DCA allocation; --monthly is required if cashflow.monthly = 0
mns guard                    # cooling-off hint: delay 1 trading day + write down the reason
mns history 20               # review executions

# Periodic audit
mns backtest update-data     # rebuild 3 daily datasets + freshness SLA (default 30 days)
mns backtest                 # full live audit: controls, holdout, regimes, walk-forward, mirror
mns backtest mirror          # strict strategy vs actual transactions only
mns backtest asset --dataset hs300   # single-asset control experiment

# Demoted, do not use for live decisions
mns backtest legacy          # old 2016-2025 monthly proxy comparison
mns backtest validate        # old single-split walk-forward + bootstrap
```

`mns guard` is a process guardrail, not a timing signal: it does not block trades, change
weights or issue buy/sell instructions. See [`docs/guardrail-experiment-2026-11-08.md`](docs/guardrail-experiment-2026-11-08.md).

### Risk profiles

`steady` / `balanced` / `aggressive` shrink the target-weight curve toward
full investment (`core_blend` 0.4 / 0.6 / 0.8); they do **not** rewrite your
curve anchors. A profile is a risk-preference knob, not alpha: the strategy
still does not guarantee beating buy-and-hold out of sample. Old configs
without the field default to `core_blend = 0`, i.e. exactly the previous
behavior.

---

## Documentation Navigation

| Documentation | Description |
|---------------|-------------|
| [.terrain/human/](.terrain/human/) | Human-friendly docs: project overview, architecture, workflows |
| [.agents/skills/mns-backtest/SKILL.md](.agents/skills/mns-backtest/SKILL.md) | Backtest skill: AI-facing steps, parameters, data-file reference |
| [README_zh.md](README_zh.md) | Current feature map and correct usage (Chinese, post-remediation) |
| [docs/remediation-2026-11-08.md](docs/remediation-2026-11-08.md) | Live-audit remediation, dataset contracts, acceptance results |
| [docs/guardrail-experiment-2026-11-08.md](docs/guardrail-experiment-2026-11-08.md) | Cooling-off guardrail significance experiment |
| [docs/hs300-experiment-2026-11-08.md](docs/hs300-experiment-2026-11-08.md) | CSI 300 single-asset control experiment |
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
