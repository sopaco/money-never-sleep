---
type: agent_context
project: money-never-sleep
title: Agent Architecture Context
source: .
---

## 项目概览
MNS 是一套运行在本地的**个人投资决策工具**（Rust）：提供 CLI 二进制 `mns`，并新增 feature-gated 的桌面壳 `mns-gui`（P0，`gui` feature；`cargo run --bin mns-gui --features gui`，不启用时 CLI 依赖与行为不变）。核心引擎编译为库 `money_never_sleep`，由 CLI、桌面壳与示例共享。核心命题：把 CNN 恐贪指数（0–100）映射成风险资产**目标仓位**（越恐慌越高，35%–85%），再按偏离带（默认 4pp）给出买/卖/持有建议。策略重构后强化“规避情绪化投资”：情绪信号只设定目标敞口，`guardrail` 仅在过热区给出冷静期提示/二次确认，**不阻断交易、也不替代决策**；`mns buy/sell` 与桌面记账仍只把用户已在券商完成的交易记入本地账本，**零券商连接**。验证链路强调可审计与同源：`mns backtest` 默认执行完整实盘审计（信号同源、T+lag 执行、通道成本、回撤双口径），并提供账户镜像/单资产对照；旧月频代理对比降级为 `legacy`，结论不可迁移到实盘。报告先留档再决策（`reports/YYYY-MM-DD.txt`）。

桌面壳“助手”页可调度外部 ACP Agent 执行**每日巡检**：MNS 用随应用分发的 `mns` CLI 生成报告简报并组装提示词（Agent 只做判断、不写文件），结论以 `mns-decision` JSON 块回传，由 MNS 解析、落盘并渲染；对话时间线与决策存档保存在 `~/.mns/agent/`，可只读回放。
## 架构设计
分层架构：CLI 串行命令 + 可选 GPUI 桌面壳，共享同一套账本/策略/审计引擎；无服务端、无券商连接。桌面壳的展示与交互通过 `app` 提供的 typed 只读/服务视图消费引擎；后台任务在 `gui::jobs` 与 `agent` ACP 运行时中执行。

| 层 | 容器 | 职责 | 关键文件 |
|---|---|---|---|
| 交互层 | clap CLI / GPUI Kit 桌面壳 | CLI 命令与参数；桌面窗口/托盘/页面（总览/持仓/计划/复盘/助手/设置） | `src/cli.rs`、`src/bin/mns-gui.rs`、`src/gui/**` |
| 入口层 | 命令分发 / 库入口 | CLI `cmd_*` 装配（`use money_never_sleep::{...}`）；库导出 `app/agent/gui` 等模块；GUI 入口 `money_never_sleep::gui::run()` | `src/main.rs`、`src/lib.rs` |
| 应用服务层 | typed 视图/快照 | 供 CLI/GUI/助手共享的应用服务与快照（如 `Snapshot`/`PlanView`、`load_snapshot`），避免前端各自拼装账本 | `src/app/mod.rs` |
| 助手层（`agent` feature） | 外部 ACP 运行时 | 通过 `agent-client-protocol` 驱动外部 ACP CLI（默认发现 `opencode`，追加 `acp` 子命令；兼容命令串/额外参数/模型内联配置）；巡检/追问提示词组装与 `mns-decision` 结论解析；MNS 不持有模型/推理端点/API Key，凭据留在外

…
## 模块地图
| Module | 责任 | 主要路径 |
|---|---|---|
| 库入口/前端共享 | 导出核心模块；始终暴露 `app`；按 feature 暴露 `agent`/`gui` | `src/lib.rs`、`src/app/mod.rs` |
| CLI 入口/分发 | `cmd_*` 串行装配；消费 `money_never_sleep` 库；`report/today/plan/guard/performance`；backtest 默认审计及 `mirror/asset/update-data/validate-full/legacy/validate`；cash `deposit/withdraw/set` | `src/main.rs`、`src/cli.rs` |
| 桌面壳（P0，GUI） | GPUI Kit 窗口/托盘、页面路由（Dashboard/Portfolio/Plan/Performance/Agent/Settings）、后台 jobs、每日巡检调度（每日至多一次）、主题/图表/情绪氛围 | `src/bin/mns-gui.rs`、`src/gui/mod.rs`、`src/gui/pages/*.rs`、`src/gui/{chart,components,conversations,jobs,mood,theme}.rs` |
| 外部助手运行时 | ACP 连接外部 `opencode` 等 CLI（兼容命令串/额外参数/模型名）；目标提交、会话内追问（复用 ACP 会话）、`mns-decision` 结构化结论、巡检提示词（`routine`）、随应用分发的 CLI 路径解析、权限策略（不托管模型/凭据） | `src/agent/mod.rs`、`src/a

…
## 核心流程
**① 日常决策与观察（`mns update-prices` → `mns report` / `mns today`；`mns plan`；`mns guard`）**
1. 抓取 CNN 恐贪指数，解析 score/rating 等；当日快照写入 `fear_greed_snapshots`。
2. 读取近期历史，由 `signal::compute` 计算决策信号（平滑、区间确认、决策日）；历史不足降级。
3. 决策日或 `--decide`：用**有效决策分数**驱动策略引擎，按目标权重/偏离带生成再平衡计划（先卖后买，卖出回笼计入买入预算）；生成决策版报告并存档 `reports/YYYY-MM-DD.txt`。非决策日未强制时输出观察版。
4. `mns plan` 输出定投分配（依赖本地恐贪快照）；`mns guard` 只读展示冷静期/过热提示。护栏在过热时建议延迟并要求记录理由，**不阻断交易**；情绪信号不作为择时 alpha。

**② 资金与交易记账（`mns cash deposit/withdraw` → `mns add/buy/sell` → `mns portfolio/performance/history`）**
1. 外部资金进出使用 `cash deposit/withdraw`（XIRR 口径资金流）；`cash set` 仅用于纠错，不代表外部资金流。
2. 标的先入池；`buy/sell` 仅登记已成交交易并更新现金/成本（现金不足/超持报错，卖出受持有期约束）——记账非下单，误记会污染下游。
3. `portfolio/history` 复核账本；`performance` 只读输出组合 XIRR/实际回撤等绩效视图。

**③ 实盘审计与验证（`mns back

…
## 技术选型
- **语言/版本**：Rust edition 2024；库 `money-never_sleep` + CLI/GUI 双二进制；`cargo build --release` 构建 CLI
- **CLI**：clap（子命令树）
- **桌面 UI（`gui` feature）**：`gpui-kit 0.9.0` + `gpui-tray 0.3.0`（`gpui-kit`/`menu-state`，与 `gpui-pre` 统一）；P0 窗口/托盘/页面
- **外部助手（`agent` feature）**：`agent-client-protocol 5.0`（`process`，v1 schema），ACP over stdio 驱动外部 CLI（如 `opencode acp`）；启动配置兼容命令串（如 `npx opencode`）、额外参数与模型名内联配置；助手/GUI 设置（CLI 路径、模型、权限策略、巡检时间）持久化于 `~/.mns/gui.toml`（temp+rename 原子写）
- **存储**：SQLite（rusqlite），单文件 `~/.mns/mns.db`，单写者串行
- **HTTP**：reqwest；CNN 抓取含重试/反爬容错
- **时间**：chrono（NaiveDate；日频 bar/决策日/披露时序）
- **输出**：comfy-table + 中文章节文本；报告落盘；桌面可视化组件
- **配置**：serde + toml，`~/.mns/config.toml`；兼容默认值与校验；助手设置（CLI 路径/模型/权限策略等）持久化于 `~/.mns/gui.toml`
- **错误**：anyhow，统一 `Error: <中文原因>` +

…
## 系统边界
| 边界 | 对象 | 说明 / 信任边界 |
|---|---|---|
| 外部 API | CNN Fear & Greed `production.dataviz.cnn.io/index/fearandgreed/graphdata` | 需网络，可 418；只读，供决策与数据集构建参考 |
| 外部 API | 天天基金 / 东财行情 | 基金/净值/价格来源，多源回退；缺失时跳过单标的 |
| 外部 API | Yahoo Finance | `market`/`analyze` 等依赖；可能 403，视为环境限制 |
| 外部进程（ACP） | `opencode` 等 ACP 兼容 CLI（`agent` feature） | MNS 仅配置/发现 CLI 路径（支持命令串/额外参数/模型名）并通过 stdio 发指令，巡检/追问提示词由 MNS 组装；**不持有模型、推理端点或 API Key**，provider/凭据由外部 Agent 自管；权限策略由 MNS/用户约束目标执行；结论只经 `mns-decision` 块回传，Agent 不写文件 |
| 数据集构建 | `update-data` + Python 3 + 网络 | 重建 point-in-time 日频 CSV/meta；失败不应静默污染既有数据 |
| 本地存储 | SQLite 账本 + TOML 配置 + 助手/GUI 设置（`~/.mns/gui.toml`）+ 对话/决策归档（`~/.mns/agent/conversations|runs|last-review`）+ 报告/数据集/图标与打包产物 | 本地可信、单写者；并发写会锁库；`cash set` 为纠错通道；运行时产物统一解析到 `~/.mns` 数据

…
## 代码映射索引
| Concept | Location | Notes |
|---|---|---|
| 库入口/feature 导出 | `src/lib.rs` | `app` 常驻；`agent`/`gui` 由 feature 控制；`money_never_sleep` 供 bins/示例复用 |
| CLI 命令树/参数 | `src/cli.rs` | Cash 子命令；`Guard`/`Performance`；`BacktestAction`（默认审计、Mirror/Asset/UpdateData/ValidateFull/Legacy/Validate） |
| CLI 入口分发 | `src/main.rs` | `use money_never_sleep::{...}`；`cmd_guard`/`cmd_performance`/`cmd_cash_deposit|withdraw`/`cmd_backtest_*` 等串行装配；`today --quiet` 输出供桌面巡检简报复用 |
| 应用快照/服务 | `src/app/mod.rs` | `Snapshot`/`PlanView`/`load_snapshot` 等 typed 视图，供 CLI/GUI 共享 |
| 桌面壳入口与页面 | `src/bin/mns-gui.rs`、`src/gui/mod.rs`、`src/gui/pages/{dashboard,portfolio,plan,performance,agent,settings}.rs` | P0 窗口/托盘、jobs、每日巡检调度、chart/components/mood/theme/conversations |
| 外部 ACP 助手 | `src/agent/

…
## 项目
概览

MNS 是一套运行在本机的**个人投资
决策工具
**（Rust）：提供 CLI 二进制 `mns`，并新增 feature
-gated 的桌面壳 `mns-gui`（P0，`gui` feature；`cargo run --bin mns-gui --features gui`，不启用时 CLI 依赖与行为不变）。核心引擎编译为库 `money_never_sleep`，由 CLI、桌面壳与示例共享。核心命题：把 CNN 恐贪指数（0–100）映射成风险资产**目标仓位**（越恐慌越高，35%–85%），再按偏离带（默认 4pp）给出买/卖/持有建议。策略重构后强化“规避情绪化投资”：情绪信号只设定目标敞口，`guardrail` 仅在过热区给出冷静期提示/二次确认，**不阻断交易、也不替代决策**；`mns buy/sell` 与桌面记账仍只把用户已在券商完成的交易记入本地账本，**零券商连接**。验证链路强调可审计与同源：`mns backtest` 默认执行完整实盘审计（信号同源、T+lag 执行、通道成本、回撤双口径），并提供账户镜像/单资产对照；旧月频代理对比降级为 `legacy`，结论不可迁移到实盘。报告先留档再决策（`reports/YYYY-MM-DD.txt`）。

桌面壳的“助手”页可驱动外部 ACP Agent 执行**每日巡检**：MNS 用随应用分发的 `mns` CLI 生成报告简报并组装提示词（Agent 只做判断、不写文件），结论以 

```mns
-decision JSON 块回传，由 MNS 解析、落盘并渲染；对话时间线与决策存档保存在 `~/.mns/agent/`，可只读回放。
```

Hmm
 — the
 fenced code block 

``

…
## 架构设计
table
 changes
:

Row 助手层:
| 助手层（`agent` feature） | 外部 ACP 运行时 | 通过 `agent-client-protocol` 驱动外部 ACP CLI（默认发现 `opencode`，追加 `acp` 子命令）；巡检/追问提示词组装（`routine`）与 `mns-decision` 结论解析；MNS 不持有模型/推理端点/API Key，凭据留在外部 Agent；命令/事件/权限策略/设置（`~/.mns/gui.toml`） | `src/agent/mod.rs`、`src/agent/routine.rs`、`src/agent/runner.rs` |

Add row
 after 助手层:
| 助手归档 | 会话与决策存档 | 对话时间线（一对话一 JSON）与 `mns-decision` 决策存档的写入/回放/删除，均在 jobs worker 线程执行 | `src/gui/conversations.rs` |

Hmm
, which
 layer? It's
 part
 of
 GUI shell but
 serves the
 assistant. I'll label
 it
 助手层 or
 交互层
? Let me put
 it
 under
 助手层 as
 "助手层（`gui`）"
 — hmm. Actually
 keep
 it
 simple
: put
 in
 the
 桌面壳 domain
. Since
 conversations.rs is
 in gui
, I'll add
 to
 the
 桌面壳 row
 description
 instead
 of
 a
 separate
 

…
## 模块地图
changes
:

桌面壳 row:
| 桌面壳（P0，GUI） | GPUI Kit 窗口/托盘、页面路由（Dashboard/Portfolio/Plan/Performance/Agent/Settings）、后台 jobs、每日巡检调度、主题/图表/情绪氛围 | `src/bin/mns-gui.rs`、`src/gui/mod.rs`、`src/gui/pages/*.rs`、`src/gui/{chart,components,conversations,jobs,mood,theme}.rs` |

外部助手运行
时 row:
| 外部助手运行时 | ACP 连接外部 `opencode` 等 CLI（兼容命令串/额外参数/模型内联配置）；目标提交与会话内追问、`mns-decision` 结论解析、每日巡检提示词（`routine`）；CLI 路径解析（随应用分发的 `mns`）；权限策略（不托管模型/凭据） | `src/agent/mod.rs`、`src/agent/runner.rs`、`src/agent/routine.rs`、`examples/agent_smoke.rs` |

Add
 new
 row
 (after
 外部助手运行
时):
| 助手会话/决策归档 | 对话时间线与结构化结论的落盘/回放/删除（`~/.mns/agent/conversations|runs`），巡检触发标记（last-review）防重复 | `src/gui/conversations.rs`、`src/gui/jobs.rs` |

配置 row: add
 app_data_dir? The
 baseline: "|
 配置 |
 `AppConfig`（Settings/Alloc

…
## 核心流程
④ rewrite
:

**④ 桌面壳与助手协作（P0，`mns-gui --features gui`）**
1. `src/bin/mns-gui` 启动库内 `gui::run()`；展示层从 `crate::app` 读取快照/计划/绩效等 typed 数据，投资规则仍由同一引擎模块裁决。
2. 刷新、记账类操作与巡检简报通过 `gui::jobs` 后台执行；托盘支持唤起主窗口/运行默认目标/立即巡检/停止任务/退出，并显示巡检计划状态。
3. “助手”页通过 `agent` 运行时向外部 ACP CLI（默认 `opencode`，追加 `acp`）提交目标并消费 `AgentEvent` 时间线/权限请求；MNS 仅传入 CLI 路径与上下文，**不提供模型或凭据**。
4. 每日巡检：`maybe_start_daily` 每日最多触发一次（`~/.mns/agent/last-review` 标记，重启不重复）；jobs worker 以随应用分发的 `mns` 运行 `today --quiet`（或 `report`）生成简报——与终端输出字节一致；`routine::daily_review_prompt` 组装提示词（报告为唯一数据源、只读命令清单、`mns-decision` JSON 输出契约），以 `GoalSource::DailyReview` 提交。
5. 结论回传：每轮结束后 runner 从助手正文提取最后一个 `mns-decision` 围栏（仅认带标签围栏，非法 JSON 忽略）解析为 `DecisionReport`，GUI 渲染结论卡片并存档 `~/.mns/agent/runs/*.json`（原始 JSON 保留，容忍新字段）；Agent 全程不写文件。
6

…
## 技术选型
changes
:

- 外部助手 row: "**外部助手（`agent` feature）**：`agent-client-protocol 5.0`（`process`，v1 schema），ACP over stdio 驱动外部 CLI（如 `opencode acp`）；启动配置兼容命令串（如 `npx opencode`）、额外参数与模型名内联配置，`acp` 子命令只追加一次；设置（CLI/模型/权限策略/巡检时间）持久化于 `~/.mns/gui.toml`（temp+rename 原子写）"

- 打包/发布 row: extend
 make-dmg: "`.app` 内同时携带 `mns` 与 `mns-gui`（同版本；CLI 以默认 feature 构建，与独立分发构建一致）".

- 测试 row: "`examples/agent_smoke.rs`（`--features agent`）验证 ACP 链路（含 `TurnFinished`/`Decision` 事件模型）" — fine.


#
## 系统边界
changes
:

- 外部进程
（ACP） row: "MNS 仅配置/发现 CLI 路径（支持命令串如 `npx opencode`、额外参数、模型名）并通过 stdio 发指令，巡检/追问提示词由 MNS 组装；**不持有推理端点或 API Key**，provider/凭据由外部 Agent 自管；权限策略由 MNS/用户约束目标执行；结论仅经 `mns-decision` 块回传，Agent 不写文件"
- 本地存储 row: "SQLite 账本 + TOML 配置 + 助手/GUI 设置（`~/.mns/gui.toml`）+ 对话/决策归档（`~/.mns/agent/`）+ 报告/数据集/图标与打包产物 | 本地可信、单写者；并发写会锁库；`cash set` 为纠错通道；运行时产物统一解析到 `~/.mns` 数据根（与 cwd 无关）"
- 人机边界 row: extend
 with
 助手结论不自动执行.


#
## 代码映射索引
changes
:

- CLI 入
口分发
 row: add
 `cmd_today`（`--decide/--quiet`）
? Keep
 baseline text
 + small
 addition
. Baseline: "|
 CLI 入
口分发 | `src/main.rs` | `use money_never_sleep::{...}`；`cmd_guard`/`cmd_performance`/`cmd_cash_deposit|withdraw`/`cmd_backtest_*` 等串行装配
 |" → add
 "；`cmd_today`（`--quiet` 供巡检简报复用）".
- 桌面壳入口与页面 row: Location
 add
 `src/gui/conversations.rs`
 and
 jobs
; Notes
 add
 每日巡检/会话归档
.
- 外部 ACP 助手 row: rewrite
 Notes
.
- Add
 row: 助手归档
/巡检调度
.

Baseline rows to preserve verbatim
 (untouched): 库入口/feature 导出, CLI 命令树/参数, 应用快照
/服务, 配置与校验
 (mostly
, but
 config.rs changed — add
 app_data_dir note), 数据库/流水, 领域类型, 决策信号, 再平衡计划, 冷静期护栏, 报告生成, 实盘审计引擎, 日频数据集, 回测/legacy 与指标, 情绪/行情, 打包/图标/技能.


Wait
 — config row: should
 I change
 it? config.rs had
 80 lin

…
## 项目概览
MNS 是一套运行在本地的**个人投资决策工具**（Rust）：提供 CLI 二进制 `mns`，并新增 feature-gated 的桌面壳 `mns-gui`（P0，`gui` feature；`cargo run --bin mns-gui --features gui`，不启用时 CLI 依赖与行为不变）。核心引擎编译为库 `money_never_sleep`，由 CLI、桌面壳与示例共享。核心命题：把 CNN 恐贪指数（0–100）映射成风险资产**目标仓位**（越恐慌越高，35%–85%），再按偏离带（默认 4pp）给出买/卖/持有建议。策略重构后强化“规避情绪化投资”：情绪信号只设定目标敞口，`guardrail` 仅在过热区给出冷静期提示/二次确认，**不阻断交易、也不替代决策**；`mns buy/sell` 与桌面记账仍只把用户已在券商完成的交易记入本地账本，**零券商连接**。验证链路强调可审计与同源：`mns backtest` 默认执行完整实盘审计（信号同源、T+lag 执行、通道成本、回撤双口径），并提供账户镜像/单资产对照；旧月频代理对比降级为 `legacy`，结论不可迁移到实盘。报告先留档再决策（`reports/YYYY-MM-DD.txt`）。

桌面壳“助手”页可调度外部 ACP Agent 执行**每日巡检**：MNS 用随应用分发的 `mns` CLI 生成报告简报并组装提示词（Agent 只做判断、不写文件），结论以 `mns-decision` JSON 块回传，由 MNS 解析、落盘并渲染；对话时间线与决策存档保存在 `~/.mns/agent/`，可只读回放。
## 架构设计
分层架构：CLI 串行命令 + 可选 GPUI 桌面壳，共享同一套账本/策略/审计引擎；无服务端、无券商连接。桌面壳的展示与交互通过 `app` 提供的 typed 只读/服务视图消费引擎；后台任务在 `gui::jobs` 与 `agent` ACP 运行时中执行。

| 层 | 容器 | 职责 | 关键文件 |
|---|---|---|---|
| 交互层 | clap CLI / GPUI Kit 桌面壳 | CLI 命令与参数；桌面窗口/托盘/页面（总览/持仓/计划/复盘/助手/设置） | `src/cli.rs`、`src/bin/mns-gui.rs`、`src/gui/**` |
| 入口层 | 命令分发 / 库入口 | CLI `cmd_*` 装配（`use money_never_sleep::{...}`）；库导出 `app/agent/gui` 等模块；GUI 入口 `money_never_sleep::gui::run()` | `src/main.rs`、`src/lib.rs` |
| 应用服务层 | typed 视图/快照 | 供 CLI/GUI/助手共享的应用服务与快照（如 `Snapshot`/`PlanView`、`load_snapshot`），避免前端各自拼装账本 | `src/app/mod.rs` |
| 助手层（`agent` feature） | 外部 ACP 运行时 | 通过 `agent-client-protocol` 驱动外部 ACP CLI（默认发现 `opencode`，追加 `acp` 子命令；兼容命令串/额外参数/模型内联配置）；巡检/追问提示词组装与 `mns-decision` 结论解析；MNS 不持有模型/推理端点/API Key，凭据留在外

…
## 模块地图
| Module | 责任 | 主要路径 |
|---|---|---|
| 库入口/前端共享 | 导出核心模块；始终暴露 `app`；按 feature 暴露 `agent`/`gui` | `src/lib.rs`、`src/app/mod.rs` |
| CLI 入口/分发 | `cmd_*` 串行装配；消费 `money_never_sleep` 库；`report/today/plan/guard/performance`；backtest 默认审计及 `mirror/asset/update-data/validate-full/legacy/validate`；cash `deposit/withdraw/set` | `src/main.rs`、`src/cli.rs` |
| 桌面壳（P0，GUI） | GPUI Kit 窗口/托盘、页面路由（Dashboard/Portfolio/Plan/Performance/Agent/Settings）、后台 jobs、每日巡检调度（每日至多一次）、主题/图表/情绪氛围 | `src/bin/mns-gui.rs`、`src/gui/mod.rs`、`src/gui/pages/*.rs`、`src/gui/{chart,components,conversations,jobs,mood,theme}.rs` |
| 外部助手运行时 | ACP 连接外部 `opencode` 等 CLI（兼容命令串/额外参数/模型名）；目标提交、会话内追问（复用 ACP 会话）、`mns-decision` 结构化结论、巡检提示词（`routine`）、随应用分发的 CLI 路径解析、权限策略（不托管模型/凭据） | `src/agent/mod.rs`、`src/a

…
## 核心流程
**① 日常决策与观察（`mns update-prices` → `mns report` / `mns today`；`mns plan`；`mns guard`）**
1. 抓取 CNN 恐贪指数，解析 score/rating 等；当日快照写入 `fear_greed_snapshots`。
2. 读取近期历史，由 `signal::compute` 计算决策信号（平滑、区间确认、决策日）；历史不足降级。
3. 决策日或 `--decide`：用**有效决策分数**驱动策略引擎，按目标权重/偏离带生成再平衡计划（先卖后买，卖出回笼计入买入预算）；生成决策版报告并存档 `reports/YYYY-MM-DD.txt`。非决策日未强制时输出观察版。
4. `mns plan` 输出定投分配（依赖本地恐贪快照）；`mns guard` 只读展示冷静期/过热提示。护栏在过热时建议延迟并要求记录理由，**不阻断交易**；情绪信号不作为择时 alpha。

**② 资金与交易记账（`mns cash deposit/withdraw` → `mns add/buy/sell` → `mns portfolio/performance/history`）**
1. 外部资金进出使用 `cash deposit/withdraw`（XIRR 口径资金流）；`cash set` 仅用于纠错，不代表外部资金流。
2. 标的先入池；`buy/sell` 仅登记已成交交易并更新现金/成本（现金不足/超持报错，卖出受持有期约束）——记账非下单，误记会污染下游。
3. `portfolio/history` 复核账本；`performance` 只读输出组合 XIRR/实际回撤等绩效视图。

**③ 实盘审计与验证（`mns back

…
## 技术选型
- **语言/版本**：Rust edition 2024；库 `money-never_sleep` + CLI/GUI 双二进制；`cargo build --release` 构建 CLI
- **CLI**：clap（子命令树）
- **桌面 UI（`gui` feature）**：`gpui-kit 0.9.0` + `gpui-tray 0.3.0`（`gpui-kit`/`menu-state`，与 `gpui-pre` 统一）；P0 窗口/托盘/页面
- **外部助手（`agent` feature）**：`agent-client-protocol 5.0`（`process`，v1 schema），ACP over stdio 驱动外部 CLI（如 `opencode acp`）；启动配置兼容命令串（如 `npx opencode`）、额外参数与模型名内联配置；助手/GUI 设置（CLI 路径、模型、权限策略、巡检时间）持久化于 `~/.mns/gui.toml`（temp+rename 原子写）
- **存储**：SQLite（rusqlite），单文件 `~/.mns/mns.db`，单写者串行
- **HTTP**：reqwest；CNN 抓取含重试/反爬容错
- **时间**：chrono（NaiveDate；日频 bar/决策日/披露时序）
- **输出**：comfy-table + 中文章节文本；报告落盘；桌面可视化组件
- **配置**：serde + toml，`~/.mns/config.toml`；兼容默认值与校验；助手设置（CLI 路径/模型/权限策略等）持久化于 `~/.mns/gui.toml`
- **错误**：anyhow，统一 `Error: <中文原因>` +

…
## 系统边界
| 边界 | 对象 | 说明 / 信任边界 |
|---|---|---|
| 外部 API | CNN Fear & Greed `production.dataviz.cnn.io/index/fearandgreed/graphdata` | 需网络，可 418；只读，供决策与数据集构建参考 |
| 外部 API | 天天基金 / 东财行情 | 基金/净值/价格来源，多源回退；缺失时跳过单标的 |
| 外部 API | Yahoo Finance | `market`/`analyze` 等依赖；可能 403，视为环境限制 |
| 外部进程（ACP） | `opencode` 等 ACP 兼容 CLI（`agent` feature） | MNS 仅配置/发现 CLI 路径（支持命令串/额外参数/模型名）并通过 stdio 发指令，巡检/追问提示词由 MNS 组装；**不持有模型、推理端点或 API Key**，provider/凭据由外部 Agent 自管；权限策略由 MNS/用户约束目标执行；结论只经 `mns-decision` 块回传，Agent 不写文件 |
| 数据集构建 | `update-data` + Python 3 + 网络 | 重建 point-in-time 日频 CSV/meta；失败不应静默污染既有数据 |
| 本地存储 | SQLite 账本 + TOML 配置 + 助手/GUI 设置（`~/.mns/gui.toml`）+ 对话/决策归档（`~/.mns/agent/conversations|runs|last-review`）+ 报告/数据集/图标与打包产物 | 本地可信、单写者；并发写会锁库；`cash set` 为纠错通道；运行时产物统一解析到 `~/.mns` 数据

…
## 代码映射索引
| Concept | Location | Notes |
|---|---|---|
| 库入口/feature 导出 | `src/lib.rs` | `app` 常驻；`agent`/`gui` 由 feature 控制；`money_never_sleep` 供 bins/示例复用 |
| CLI 命令树/参数 | `src/cli.rs` | Cash 子命令；`Guard`/`Performance`；`BacktestAction`（默认审计、Mirror/Asset/UpdateData/ValidateFull/Legacy/Validate） |
| CLI 入口分发 | `src/main.rs` | `use money_never_sleep::{...}`；`cmd_guard`/`cmd_performance`/`cmd_cash_deposit|withdraw`/`cmd_backtest_*` 等串行装配；`today --quiet` 输出供桌面巡检简报复用 |
| 应用快照/服务 | `src/app/mod.rs` | `Snapshot`/`PlanView`/`load_snapshot` 等 typed 视图，供 CLI/GUI 共享 |
| 桌面壳入口与页面 | `src/bin/mns-gui.rs`、`src/gui/mod.rs`、`src/gui/pages/{dashboard,portfolio,plan,performance,agent,settings}.rs` | P0 窗口/托盘、jobs、每日巡检调度、chart/components/mood/theme/conversations |
| 外部 ACP 助手 | `src/agent/

…