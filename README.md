# MNS - Money Never Sleeps

> **AI-Powered Contrarian Investment Decision Assistant**  
> Overcome human weaknesses, systematically execute "buy in fear, sell in greed"

<p align="center">
    <a href="https://github.com/sopaco/money-never-sleep/tree/main/litho.docs"><img alt="Litho Docs" src="https://img.shields.io/badge/Litho-Docs-green?logo=Gitbook&color=%23008a60"/></a>
    <a href="http://clawhub.ai/sopaco/money-never-sleep"><img alt="ClawHub" src="https://img.shields.io/badge/ClawHub-Certified-blue"/></a>
    <a href="https://github.com/openclaw/openclaw"><img alt="OpenClaw Compatible" src="https://img.shields.io/badge/OpenClaw-Compatible-brightgreen"/></a>
</p>

---

## The biggest enemy of investing is not the market, but human nature

Fear prevents buying when prices drop, greed prevents selling when prices rise—this is the instinctive dilemma every investor faces. MNS digitizes contrarian investment strategies, replacing emotion with rules, letting data drive decisions:

- 🎯 **Automatically suggest buying during extreme fear**, not panic selling
- 💰 **Remind to take profit when annualized returns meet targets**, not chasing highs
- ⚠️ **Smart alerts when positions are underwater**, not ignoring losses
- 📊 **Backtest-validated strategy effectiveness**, 8-9% annualized return, 16-21% max drawdown

---

## Core Advantages

### 🧠 Based on Real Market Data
Integrated CNN Fear & Greed Index, real-time market sentiment sensing, automatically saves historical snapshots for retrospective analysis.

### 📈 Validated Strategy Parameters
Optimized based on 2016-2025 historical backtesting, default conservative allocation (US stocks 55%, CN stocks 25%, Gold 20%), return/drawdown ratio 0.42+.

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
| `.ai-context/` | AI Coding Agent | Quickly understand project architecture, design decisions, active issues |
| `litho.docs/` | Human Developers | Project overview, workflows, deep dives |

### SKILL Synergy Effect

When you speak keywords (like "backtest"), AI automatically activates the corresponding SKILL, instantly gaining complete contextual understanding:

- ✅ **No need to explain project background**, AI already understands architecture and constraints
- ✅ **Safe strategy parameter modifications**, AI knows where and how to change without breaking other modules
- ✅ **Automatically follows existing patterns when adding features**, maintaining code consistency
- ✅ **Quickly locate root causes when debugging**, AI has complete data flow and call relationships

> 💡 **One sentence triggers SKILL**: Tell OpenClaw "I want to optimize profit-taking parameters" or "help me backtest this strategy", AI will automatically read `.ai-context/SKILL.md` and gain complete project knowledge.

---

## Quick Start

```bash
# Install
cargo build --release

# Initialize
mns init
mns cash set 100000

# Daily usage
mns add QQQ "Nasdaq 100" us_stocks
mns buy QQQ 50 380.00
mns update-prices
mns report          # Generate today's action suggestions
mns backtest        # Strategy backtest
```

---

## Documentation Navigation

| Documentation | Description |
|---------------|-------------|
| [litho.docs/](litho.docs/) | Human-friendly docs: project overview, architecture, workflows |
| [.ai-context/](.ai-context/) | AI context: architecture decisions, design constraints, active issues |
| [AGENTS.md](AGENTS.md) | OpenClaw AI Agent working guidelines |

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
