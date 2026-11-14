# MNS (Money Never Sleeps)

> 一个跑在本机的个人投资辅助工具：记录持仓和资金流水，按市场情绪给出目标仓位，帮你在该动的时候动、不该动的时候忍住。

MNS 有桌面应用和命令行两个入口，共用同一份本地数据和同一套规则。它不连接券商，不自动下单，也不预测市场。所有数据只存在你自己的电脑上。

如果你做的是中长期配置，愿意按周或按月检查一次仓位，并且希望事后能回看每次决定的依据，这个工具可能适合你。主要做短线交易的话，它的节奏可能不太合适。

## 为什么做它

投资里最难对付的往往不是市场。下跌时不敢买，上涨时舍不得卖，看到波动就想操作，这些都是很正常的反应，但放在长期配置里，最后常常变成更高的成本和更差的决策。另一个常见的问题是，过几个月回头看，已经说不清当时为什么买、为什么卖。

MNS 的做法是把流程固定下来。情绪高的时候少拿一点风险资产，情绪低的时候多拿一点；平时只在固定时间检查，偏离不大就不动；每次买卖都记在账上，报告留档，事后可以翻出来复盘。规则不一定让你赚得更多，但至少能让决策过程可重复，也能让情绪少参与一点。

## 它怎么工作

MNS 用 CNN 的 Fear & Greed 指数作为输入，把 0 到 100 的分数映射成一条五档目标仓位曲线。默认情况下，风险资产的目标仓位在 35% 到 85% 之间：市场越恐慌，允许的仓位越高；越贪婪，仓位越低。

每次检查时，程序会拿目标仓位和你的实际持仓比较。只有偏离超过带宽（默认 4 个百分点）的资产才会进入调仓计划，并且先卖后买，卖出回笼的钱计入买入预算。

有一点要说清楚：情绪分数只决定“允许持有多少风险资产”，不负责判断买点卖点，也没有止盈规则。真正让一笔交易成立的，是另外两件事：有新钱进来或需要取钱，或者某类资产偏离目标超过带宽。其他时候，报告通常会建议你什么都不做。

## 功能

### 每周一次的决策报告

日频的情绪数据对周频、月频的配置来说噪音太大，所以 MNS 在计算前会先做 7 天平滑，再要求新档位连续 3 天确认，最后只在决策日（默认周五）输出完整的调仓建议。

决策日给出买什么、卖什么、各多少；其他日子只出观察版报告，提醒你今天不用动手。如果确实需要在非决策日看建议，可以用 `mns report --decide` 强制生成。

每份报告都会存到 `~/.mns/reports/YYYY-MM-DD.txt`。时间久了，这些存档比记忆可靠。

### 仓位建议和再平衡

目标仓位按五档曲线计算，再根据偏离带生成具体的买卖计划。偏离在带宽内的资产保持不动，需要调整的资产按先卖后买排序，报告里会列出具体金额和份额，而不是只丢一句“该加仓了”。

### 月度定投分配

`mns plan --monthly 3000` 会根据当前情绪和目标仓位，把本月的新钱分配到各个资产腿。它只买不卖，作用是把储蓄率和执行节奏固定下来，不是用来增厚收益的。

`plan` 和 `guard` 都依赖本地情绪快照，第一次使用前先跑一次 `mns today`。

### 冲动买入前的冷静期

临时想加仓的时候，先跑一次 `mns guard`。如果市场处于过热区间，它会建议你延迟一个交易日，并把理由写下来再决定。

它不会拦住你，也不会改仓位。只是一个提醒：这笔操作是计划内的，还是情绪推着你做的。

### 本地账本

MNS 不连券商，买卖需要你在券商完成之后自己登记：

```bash
mns cash deposit 10000        # 外部入金，按 XIRR 口径记账
mns buy 018966 1000 1.234     # 登记一笔已成交的买入
mns sell 018966 500 1.456     # 登记一笔已成交的卖出
mns cash withdraw 2000        # 外部出金
```

持仓、成本、现金和流水会保持一致，现金不足或卖出超过持仓会直接报错。`cash set` 只用来纠错，不应该拿来记入金。

`mns portfolio` 看单只标的的持仓、成本和年化收益，`mns performance` 看组合 XIRR 和实际最大回撤。后者是只读的，不会写库。

### 纪律复盘

复盘页会按自然年统计你的交易笔数，对照年度交易预算（默认 10 到 20 次），看看今年是不是动得太频繁。买入时如果触发过冷静期，理由会带着“计划外冷静”或“过热冷静”的前缀存进流水，复盘时能分开看。没有标记的记录就如实算作未标记，程序不会去猜你的意图。

### 桌面端

桌面应用和命令行是同一套引擎，只是换了种呈现方式。总览页看净值和资产曲线，持仓页看成本和盈亏，计划页看本月定投，复盘页看绩效和纪律，助手页用来接外部 AI Agent，设置页管参数、主题和开机自启动。

托盘常驻之后，可以快速唤起窗口、立即巡检、停止任务或退出。开机自启动写的是系统自身的登录项，不会额外常驻一个后台进程。Windows 的发布版没有控制台窗口。

### AI 助手

桌面端的助手页通过 ACP 协议调用你已经安装的编码 Agent（默认会找 `opencode`）。MNS 把当天的报告作为唯一数据源交给它，让它按固定格式给出结论。

MNS 本身不内置模型，也不保存 API Key，模型和凭据始终留在你自己的 Agent 配置里。结论会以 JSON 存档到 `~/.mns/agent/`，可以只读回放。Agent 不会写你的账本，也不会执行交易。

### 历史审计

`mns backtest` 默认跑的是实盘审计：用和 `mns report` 同源的信号、真实基金净值、真实通道成本与结算规则，输出账户镜像和一组对照实验。

```bash
mns backtest update-data              # 重建本地日频数据集（需要 Python 3）
mns backtest                          # 完整审计
mns backtest mirror                   # 严格执行策略和你的真实流水对比
mns backtest asset --dataset hs300    # 单资产对照实验
```

`mns backtest legacy` 是更早的月频引擎，已经降级，只适合查历史，不要拿它指导实盘。

## 一周怎么用

| 时间 | 做什么 | 命令 |
|---|---|---|
| 第一次 | 初始化、入金、登记已有持仓 | `mns init` → `mns cash deposit` → `mns add` |
| 每周五 | 刷价并生成决策报告 | `mns today` |
| 发薪日 | 分配定投金额，成交后补录 | `mns plan --monthly 3000` → `mns buy` |
| 临时想加仓 | 看一眼冷静期提示 | `mns guard` |
| 每季度 | 更新数据，和账户镜像对一次 | `mns backtest update-data` → `mns backtest mirror` |
| 平时 | 看绩效、翻流水、看市场概况 | `mns performance` / `mns history` / `mns market` |
| 日常 | 打开桌面端 | `mns-gui` |

## 安装

从源码构建 CLI：

```bash
cargo build --release
./target/release/mns --version
```

构建桌面应用。macOS 会产出 `.app` 和 `.dmg`：

```bash
cargo build --release --features gui
./scripts/make-dmg.sh
# 产物在 target/dist/：MNS.app、MNS-<version>-<arch>.dmg
```

Windows 需要先安装 [Inno Setup 6](https://jrsoftware.org/isinfo.php)：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\make-installer.ps1
# 产物在 target/dist/：MNS-<version>-win-x64-setup.exe 和便携 ZIP
```

安装包按用户安装，不需要管理员权限，也不会改系统 `PATH`。桌面应用和 `mns` CLI 放在同一目录，版本一致。

也可以直接用 npm 安装预编译的 CLI：

```bash
npm install -g @never-sleeps/mns-cli
```

支持 macOS Apple Silicon、Linux x64 和 Windows x64。npm 包的版本可能比仓库慢一些，需要最新功能时还是从源码构建。

第一次运行：

```bash
mns init                        # 建立 ~/.mns/config.toml 和 mns.db
mns cash deposit 10000          # 入金
mns add 018966 "汇添富纳斯达克100ETF" us_stocks
mns buy 018966 1000 1.234       # 登记一笔已成交的买入
mns config params               # 查看全部可调参数和默认值
mns today                       # 刷价并生成今日报告
```

资产类别只有三种：`us_stocks`（美股宽基）、`cn_stocks`（A 股宽基）、`counter_cyclical`（黄金等逆周期资产）。三腿内部的占比由 `allocation.*` 控制。

所有运行数据都在 `~/.mns/` 下：

| 路径 | 内容 |
|---|---|
| `config.toml` | 风险档位、仓位曲线、偏离带、成本等设置 |
| `mns.db` | 持仓、交易流水、现金、情绪快照 |
| `reports/` | 每日报告存档 |
| `agent/` | AI 对话和决策记录 |
| `gui.toml` | 桌面端设置 |

备份就是复制这个目录，换机器时一起带走就行。MNS 不会上传任何数据。

## 一些边界

- 不连券商，不下单。`buy` 和 `sell` 只负责记账，交易要你自己完成。
- 不预测点位，也不承诺收益。历史回测不能当成实盘预期。
- 不建议逐日跟单。日频信号已经压缩到周频决策，年度交易预算默认只有 10 到 20 次，频繁操作会让成本和纪律同时失控。
- 情绪分数不决定买不买，只决定允许持有多少风险资产。
- 冷静期提示、交易预算都只是提示，不会阻断你的操作。
- `report` 和 `today` 会写入本地情绪快照和报告，不是只读命令。`performance`、`history`、`portfolio` 才是只读的。
- 最终决策和后果都由你自己承担。

## 给 AI Agent 用

仓库里带了两套面向 Agent 的 Skill：

| Skill | 用途 |
|---|---|
| [`mns-advisor`](.agents/skills/mns-advisor/SKILL.md) | 处理“什么时候买、什么时候卖、为什么不动”：判定触发器、联网核验限购和费率、输出可审计的决策卡 |
| [`mns-backtest`](.agents/skills/mns-backtest/SKILL.md) | 回测、数据集重建、参数对照和结果解读 |

如果要把决策流程交给外部 Agent，可以用通用分发版 [`distribution/skills/mns-advisor.zip`](distribution/skills/mns-advisor.zip)。它假定 `mns` 已经在 PATH 上，没有源码和数据集，只教 Agent 调用命令、解读输出。

Agent 应优先使用随应用分发的 `mns`（macOS 下是 `MNS.app/Contents/MacOS/mns`），避免 PATH 上的旧版本读出不一样的账本。

## 技术实现

MNS 用 Rust 2024 写成，CLI 编译出来是一个单文件可执行程序，不需要运行时和数据库服务。账本是单文件 SQLite，配置用 TOML，全部留在本机，没有云同步，也没有遥测。

桌面端基于 GPUI，配系统托盘，不是套壳网页。报告、桌面端和历史审计共用同一套信号与策略代码，避免出现“展示一套、回测另一套”的情况。

行情和情绪数据都做了多源回退。情绪快照落库之后，平滑和确认可以离线计算，不依赖接口每次都能连上。

外部 Agent 通过 ACP 协议接入，MNS 不托管模型、推理端点或 API Key。

## 文档

- [`README.md`](README.md)：English README
- [`AGENTS.md`](AGENTS.md)：AI Coding Agent 的工作约定
- [`.terrain/human/`](.terrain/human/)：项目概述、架构和工作流
- [`.agents/skills/mns-advisor/SKILL.md`](.agents/skills/mns-advisor/SKILL.md)：实盘买卖决策编排
- [`.agents/skills/mns-backtest/SKILL.md`](.agents/skills/mns-backtest/SKILL.md)：回测与数据契约

## License

[MIT](LICENSE)
