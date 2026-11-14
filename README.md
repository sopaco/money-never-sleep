# MNS (Money Never Sleeps)

> A local investment companion: it keeps your positions and cash flows, maps market sentiment to a target exposure, and helps you act when it is warranted — and sit still when it is not.

MNS comes as a desktop app and a command-line tool. Both read the same local database and follow the same rules. It does not connect to a broker, place orders, or predict the market. Your data stays on your machine.

It is meant for people running a long-term allocation who are willing to check in weekly or monthly and want to look back later at why a decision was made. If you mainly trade short-term, the rhythm probably will not suit you.

## Why it exists

The hardest part of investing is usually not the market. You hesitate to buy after a fall, you are reluctant to sell into strength, and a bit of volatility makes you want to do something. Those reactions are normal, but in a long-term portfolio they tend to turn into extra costs and worse decisions. There is also the memory problem: a few months later, you may no longer be able to explain why you bought or sold.

MNS takes the opposite approach: it fixes the process. Hold a little less risk when sentiment is greedy and a little more when it is fearful. Check on a schedule, and leave things alone when the drift is small. Record every trade, archive every report, and review the decisions later. The rules will not necessarily make you more money, but they make the process repeatable and leave less room for impulse.

## How it works

MNS takes the CNN Fear & Greed Index as input and maps its 0–100 score onto a five-step target exposure curve. By default, risk assets may range from 35% to 85% of the portfolio: more fear allows more exposure, more greed allows less.

On each check, the program compares that target with what you actually hold. Only assets that drift outside the band (4 percentage points by default) enter the rebalance plan. Sells come before buys, and proceeds from sales count toward the buy budget.

One thing worth being clear about: the sentiment score only sets how much risk exposure you are allowed to hold. It does not pick entry or exit points, and there is no take-profit rule. A trade is justified by something else — new money arriving, money needed out, or an asset drifting beyond its band. Most of the time, the report will tell you to do nothing.

## What it does

### A weekly decision report

Daily sentiment is noisy for a portfolio that is checked weekly or monthly. Before computing anything, MNS smooths the score over 7 days, requires a new zone to hold for 3 consecutive days, and only produces a full rebalance recommendation on the decision day (Friday by default).

On that day it tells you what to buy, what to sell, and how much. On other days it produces an observation-only report that says no action is needed. If you really need a decision on another day, `mns report --decide` will generate one.

Every report is saved to `~/.mns/reports/YYYY-MM-DD.txt`. Over time, those files are more reliable than your memory.

### Position sizing and rebalancing

Target exposure comes from the five-step curve, and the drift band turns it into a concrete plan. Assets inside their bands are left alone; the rest are ordered sells first, then buys, with amounts and share counts spelled out. The report gives you amounts and share counts instead of a vague suggestion to "add some exposure."

### Monthly contributions

`mns plan --monthly 3000` spreads this month's new money across the allocation legs based on current sentiment and target exposure. It only buys; the point is to keep the savings rate and execution rhythm steady, not to generate extra return.

Both `plan` and `guard` read the local sentiment snapshot, so run `mns today` once before using them for the first time.

### A cooling-off period before impulse buys

When you feel like adding exposure, run `mns guard` first. If the market is overheated, it suggests waiting one trading day and writing down your reason before you decide.

It will not stop you or change your allocation. It just asks whether the trade is part of the plan or something the mood talked you into.

### A local ledger

MNS does not connect to a broker. After you complete a trade, you record it yourself:

```bash
mns cash deposit 10000        # external inflow, tracked on an XIRR basis
mns buy 018966 1000 1.234     # a completed buy
mns sell 018966 500 1.456     # a completed sell
mns cash withdraw 2000        # external outflow
```

Positions, cost basis, cash, and transaction history stay consistent. Insufficient cash or selling more than you hold fails immediately. `cash set` exists for corrections; do not use it as a deposit channel.

`mns portfolio` shows each position's size, cost, and annualized return. `mns performance` shows portfolio XIRR and actual maximum drawdown; it is read-only and writes nothing.

### Discipline review

The review page counts your trades per calendar year against an annual trade budget (10 to 20 by default), so you can see whether you have been trading too often. If a buy triggered the cooling-off prompt, the reason is stored with a "planned-off-plan" or "overheated" prefix, and the review separates those buckets. Unmarked records are reported as unmarked; the program does not guess your intent.

### Desktop app

The desktop app runs the same engine as the CLI, just with a different presentation. The dashboard shows net value and the asset curve, the portfolio page shows cost and P&L, the plan page shows this month's contribution, the review page shows performance and discipline, the assistant page connects an external AI agent, and settings covers parameters, theme, and launch at login.

With the tray running, you can bring the window back, start a review, stop a task, or quit. Launch at login writes the operating system's own login entry; it does not add a background daemon. The Windows release has no console window.

### AI assistant

The assistant page talks to an ACP-compatible coding agent you already have installed (`opencode` is discovered by default). MNS hands it the day's report as the single source of truth and asks for a structured conclusion.

MNS ships no model and stores no API key; models and credentials stay in your own agent configuration. Conclusions are archived as JSON under `~/.mns/agent/` for read-only replay. The agent does not write to your ledger or execute trades.

### Historical audit

`mns backtest` runs a live-account audit by default: it uses the same signal path as `mns report`, real fund NAVs, real channel costs, and settlement rules, then reports an account mirror and a set of control experiments.

```bash
mns backtest update-data              # rebuild local daily datasets (needs Python 3)
mns backtest                          # full audit
mns backtest mirror                   # strict strategy vs your actual fills
mns backtest asset --dataset hs300    # single-asset control experiment
```

`mns backtest legacy` is the older monthly engine. It has been demoted; treat it as historical reference, not guidance for live decisions.

## A week with MNS

| When | What you do | Command |
|---|---|---|
| First time | Initialize, deposit, register existing positions | `mns init` → `mns cash deposit` → `mns add` |
| Every Friday | Refresh prices and generate the decision report | `mns today` |
| Payday | Allocate the monthly contribution, record fills afterward | `mns plan --monthly 3000` → `mns buy` |
| Impulse buy | Check the cooling-off hint | `mns guard` |
| Quarterly | Refresh data and compare against the account mirror | `mns backtest update-data` → `mns backtest mirror` |
| Anytime | Review performance, transactions, market overview | `mns performance` / `mns history` / `mns market` |
| Daily | Open the desktop app | `mns-gui` |

## Install

Build the CLI from source:

```bash
cargo build --release
./target/release/mns --version
```

Build the desktop app. On macOS this produces a `.app` and a `.dmg`:

```bash
cargo build --release --features gui
./scripts/make-dmg.sh
# Output: target/dist/MNS.app, target/dist/MNS-<version>-<arch>.dmg
```

On Windows, install [Inno Setup 6](https://jrsoftware.org/isinfo.php) first:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\make-installer.ps1
# Output: target/dist/MNS-<version>-win-x64-setup.exe and a portable ZIP
```

The installer is per-user, needs no admin rights, and does not change `PATH`. The desktop app and the `mns` CLI sit in the same directory at the same version.

You can also install a prebuilt CLI through npm:

```bash
npm install -g @never-sleeps/mns-cli
```

It supports macOS Apple Silicon, Linux x64, and Windows x64. The npm package may lag behind the repository; build from source if you need the latest features.

First run:

```bash
mns init                        # create ~/.mns/config.toml and mns.db
mns cash deposit 10000          # deposit
mns add 018966 "Nasdaq 100 feeder" us_stocks
mns buy 018966 1000 1.234       # record a completed buy
mns config params               # list all tunable parameters and defaults
mns today                       # refresh prices and generate today's report
```

There are three asset classes: `us_stocks` (US broad market), `cn_stocks` (China A-share broad market), and `counter_cyclical` (gold and similar). Their internal weights are controlled by `allocation.*`.

Everything lives under `~/.mns/`:

| Path | Contents |
|---|---|
| `config.toml` | Risk profile, exposure curve, drift band, costs |
| `mns.db` | Positions, transactions, cash, sentiment snapshots |
| `reports/` | Archived daily reports |
| `agent/` | AI conversations and decision records |
| `gui.toml` | Desktop settings |

Backup is just copying that directory, and moving to another machine is the same. MNS uploads nothing.

## Boundaries

- No broker connection and no order placement. `buy` and `sell` only record fills; you execute the trade yourself.
- No price targets and no return promises. Historical backtests are not live expectations.
- Do not trade the daily signal. Decisions are compressed to a weekly rhythm, and the annual trade budget is only 10 to 20 by default. Trading every day burns both costs and discipline.
- Sentiment does not decide whether to trade. It only sets how much risk exposure is allowed.
- Cooling-off hints and trade budgets are prompts, not blocks.
- `report` and `today` write local sentiment snapshots and report files; they are not read-only. `performance`, `history`, and `portfolio` are.
- The final decision, and its consequences, remain yours.

## For AI agents

Two agent-facing skills ship with the repository:

| Skill | Purpose |
|---|---|
| [`mns-advisor`](.agents/skills/mns-advisor/SKILL.md) | Handles "when to buy, when to sell, why not to act": trigger detection, live verification of purchase limits and fees, auditable decision cards |
| [`mns-backtest`](.agents/skills/mns-backtest/SKILL.md) | Backtests, dataset rebuilds, parameter comparisons, and result interpretation |

To hand the workflow to an external agent, use the distributable [`distribution/skills/mns-advisor.zip`](distribution/skills/mns-advisor.zip). It assumes `mns` is on `PATH`, with no source tree or datasets, and only teaches the agent which commands to call and how to read their output.

Agents should prefer the `mns` binary shipped with the app (on macOS, `MNS.app/Contents/MacOS/mns`) so a stale copy on `PATH` cannot produce a different picture of the ledger.

## Under the hood

MNS is written in Rust 2024. The CLI builds into a single executable with no runtime or database service. The ledger is a single SQLite file, configuration is TOML, and everything stays local — no cloud sync, no telemetry.

The desktop shell is built on GPUI with a system tray, not a wrapped web page. Reports, the desktop app, and historical audits share the same signal and strategy code, so what you see is what gets tested.

Quotes and sentiment data have multiple sources with fallbacks. Once sentiment snapshots are stored, smoothing and zone confirmation work offline, even if an API is temporarily unavailable.

External agents connect over ACP. MNS hosts no model, inference endpoint, or API key.

## Documentation

- [`README_zh.md`](README_zh.md): Chinese README
- [`AGENTS.md`](AGENTS.md): working agreement for AI coding agents
- [`.terrain/human/`](.terrain/human/): project overview, architecture, and workflows
- [`.agents/skills/mns-advisor/SKILL.md`](.agents/skills/mns-advisor/SKILL.md): live buy/sell decision orchestration
- [`.agents/skills/mns-backtest/SKILL.md`](.agents/skills/mns-backtest/SKILL.md): backtesting and data contracts

## License

[MIT](LICENSE)
