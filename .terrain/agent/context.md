---
type: agent_context
project: money-never-sleep
title: Agent Architecture Context
source: .
---

## 项目概览
MNS 是一套运行在本地的**个人投资决策工具**（Rust）：提供 CLI 二进制 `mns`，并新增 feature-gated 的桌面壳 `mns-gui`（P0，`gui` feature；`cargo run --bin mns-gui --features gui`，不启用时 CLI 依赖与行为不变）。核心引擎编译为库 `money_never_sleep`，由 CLI、桌面壳与示例共享。核心命题：把 CNN 恐贪指数（0–100）映射成风险资产**目标仓位**（越恐慌越高，35%–85%），再按偏离带（默认 4pp）给出买/卖/持有建议。策略重构后强化“规避情绪化投资”：情绪信号只设定目标敞口，`guardrail` 仅在过热区给出冷静期提示/二次确认，**不阻断交易、也不替代决策**；`mns buy/sell` 与桌面记账仍只把用户已在券商完成的交易记入本地账本，**零券商连接**。验证链路强调可审计与同源：`mns backtest` 默认执行完整实盘审计（信号同源、T+lag 执行、通道成本、回撤双口径），并提供账户镜像/单资产对照；旧月频代理对比降级为 `legacy`，结论不可迁移到实盘。报告先留档再决策（`reports/YYYY-MM-DD.txt`）。

桌面壳“助手”页可调度外部 ACP Agent 执行**每日巡检**：MNS 用随应用分发的 `mns` CLI 生成报告简报并组装提示词（Agent 只做判断、不写文件），结论以 `mns-decision` JSON 块回传，由 MNS 解析、落盘并渲染；对话时间线与决策存档保存在 `~/.mns/agent/`，可只读回放。

桌面端在 Windows 上以窗口子系统发布（release 无控制台），`build.rs` 把 `assets/app.ico` 编入 exe 资源；`scripts/make-installer.ps1` 产出按用户安装的 Inno Setup 安装包与便携 ZIP，`mns.exe` 与 `mns-gui.exe` 同目录。支持登录自启动（`--tray` 只驻留托盘）。

## 架构设计
分层架构：CLI 串行命令 + 可选 GPUI 桌面壳，共享同一套账本/策略/审计引擎；无服务端、无券商连接。桌面壳的展示与交互通过 `app` 提供的 typed 只读/服务视图消费引擎；后台任务在 `gui::jobs` 与 `agent` ACP 运行时中执行。

| 层 | 容器 | 职责 | 关键文件 |
|---|---|---|---|
| 交互层 | clap CLI / GPUI Kit 桌面壳 | CLI 命令与参数；桌面窗口/托盘/页面（总览/持仓/计划/复盘/助手/设置）；登录自启动开关 | `src/cli.rs`、`src/bin/mns-gui.rs`、`src/gui/**` |
| 入口层 | 命令分发 / 库入口 | CLI `cmd_*` 装配（`use money_never_sleep::{...}`）；库导出 `app/agent/gui/discipline` 等模块；GUI 入口 `money_never_sleep::gui::run()` | `src/main.rs`、`src/lib.rs` |
| 应用服务层 | typed 视图/快照 | 供 CLI/GUI/助手共享的应用服务与快照（如 `Snapshot`/`PlanView`、`load_snapshot`），避免前端各自拼装账本；`Snapshot` 内置年度纪律复盘 | `src/app/mod.rs` |
| 助手层（`agent` feature） | 外部 ACP 运行时 | 通过 `agent-client-protocol` 驱动外部 ACP CLI（默认发现 `opencode`，追加 `acp` 子命令；兼容命令串/额外参数/模型内联配置）；巡检/追问提示词组装与 `mns-decision` 结论解析；MNS 不持有模型/推理端点/API Key，凭据留在外部 Agent | `src/agent/mod.rs`、`src/agent/routine.rs`、`src/agent/runner.rs` |
| 归档层（GUI） | 对话/决策存档 | 对话时间线与决策 JSON 的写入、只读回放、删除；巡检触发标记 `last-review` 防当日重复 | `src/gui/conversations.rs`、`src/gui/jobs.rs` |
| 引擎层 | 纯规则/纯统计 | 信号、再平衡与定投、冷静期护栏、**纪律复盘**、报告、审计；不感知前端 | `src/{signal,strategy,guardrail,discipline,report,audit,backtest}.rs` |
| 数据访问层 | SQLite 单写者 | 表初始化、upsert、按日/按起始日查询（含全年流水无上限读取） | `src/db.rs`、`src/models.rs` |
| 外部数据层 | HTTP 抓取 | 恐贪指数、基金/行情净值，多源回退 | `src/sentiment.rs`、`src/quote.rs`、`src/market.rs` |

- 依赖方向单向：交互/入口 → 应用服务 → 引擎 →（数据访问｜外部数据）；`discipline` 只消费 `db` 流水与 `config.backtest` 预算，不反向依赖 GUI。
- feature 开关：`agent`（ACP 运行时）、`gui`（自动包含 `agent`）、`gui-test`（headless 布局测试）。
- 平台适配：`gui::command` 统一构造派生进程（Windows 加 `CREATE_NO_WINDOW`，避免 `reg.exe`/`mns.exe` 闪黑框）；`gui::autostart` 只写系统登录项（Windows `HKCU\...\Run`、macOS LaunchAgents、Linux `~/.config/autostart/*.desktop`），不带常驻守护进程；`build.rs` 仅在 Windows 目标把 `assets/app.ico` 编入 exe 图标资源。

## 模块地图
| Module | 责任 | 主要路径 |
|---|---|---|
| 库入口/前端共享 | 导出核心模块；始终暴露 `app`/`discipline`；按 feature 暴露 `agent`/`gui` | `src/lib.rs`、`src/app/mod.rs` |
| CLI 入口/分发 | `cmd_*` 串行装配；消费 `money_never_sleep` 库；`report/today/plan/guard/performance`；backtest 默认审计及 `mirror/asset/update-data/validate-full/legacy/validate`；cash `deposit/withdraw/set` | `src/main.rs`、`src/cli.rs` |
| 桌面壳（P0，GUI） | GPUI Kit 窗口/托盘、页面路由（Dashboard/Portfolio/Plan/Performance/Agent/Settings）、后台 jobs、每日巡检调度（每日至多一次）、助手页二级面板（Trace/Settings/History）、主题/图表/情绪氛围、登录自启动与隐藏启动窗口 | `src/bin/mns-gui.rs`、`src/gui/mod.rs`、`src/gui/pages/*.rs`、`src/gui/{autostart,chart,components,conversations,jobs,mood,theme}.rs` |
| 外部助手运行时 | ACP 连接外部 `opencode` 等 CLI（兼容命令串/额外参数/模型名）；目标提交、会话内追问（复用 ACP 会话）、`mns-decision` 结构化结论、巡检提示词（`routine`）、随应用分发的 CLI 路径解析、权限策略（不托管模型/凭据） | `src/agent/mod.rs`、`src/agent/runner.rs`、`src/agent/routine.rs`、`examples/agent_smoke.rs` |
| 应用服务层 | typed 只读视图与服务（`Snapshot`/`PlanView`/`Freshness`/`Counts`/`RefreshOutcome`）；`Snapshot.discipline` 暴露年度纪律复盘 | `src/app/mod.rs` |
| 配置与存储 | `AppConfig`（风险档位/曲线/偏离带/成本/api/backtest 预算）；数据根解析；SQLite schema 与流水查询 | `src/config.rs`、`src/db.rs`、`src/models.rs` |
| 决策信号 | 7 天平滑 + 3 天区间确认 + 决策日判定与下次决策日；历史不足降级 | `src/signal.rs` |
| 再平衡/定投/护栏 | 目标权重+偏离带生成腿计划（先卖后买、缺标的清零挂原因）；月度定投分配；过热冷静期提示（不阻断） | `src/strategy.rs`、`src/guardrail.rs` |
| 纪律复盘 | 按流水 `note` 的结构化前缀（`计划外冷静：`/`过热冷静：`/未标记）分桶统计自然年笔数，对照年度交易预算；不推断未标记意图 | `src/discipline.rs` |
| 报告生成 | 中文分章节文本（TL;DR/情绪/调仓计划/无动作原因/披露），落盘 `reports/YYYY-MM-DD.txt` | `src/report.rs` |
| 实盘审计/回测/数据集 | 默认审计（信号同源、T+lag、通道成本、回撤双口径、账户镜像）、旧月频 `legacy`、point-in-time 日频数据集与指标 | `src/audit.rs`、`src/backtest.rs`、`src/live_data.rs`、`src/metrics.rs` |
| 情绪与行情 | CNN 恐贪抓取解析；基金/股票净值行情多源回退 | `src/sentiment.rs`、`src/quote.rs`、`src/market.rs` |

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
4. 纪律闭环：桌面录入买入触发冷静期时，理由带结构化前缀落库（`计划外冷静：`/`过热冷静：`）；`discipline::review` 读全年流水（`list_transactions_since`，无行数上限）分桶，`复盘`页纪律卡对照 `backtest.annual_trade_budget_min/max` 显示是否超预算，未标记记录如实写「未标记」而不推断意图。

**③ 实盘审计与验证（`mns backtest` 默认审计 → `mirror/asset/update-data/validate-full/legacy/validate`）**
1. `update-data` 用 Python 3 重建 point-in-time 日频 CSV/meta（抓取日、各腿起止、stale 天数），失败不静默覆盖既有数据。
2. 审计直接调用 `signal::compute` 与生产策略，按 `execution_lag_days` 成交、扣真实通道成本，输出实际/预期回撤双口径与账户镜像对照。
3. `legacy` 仅重跑旧月频代理对比，供历史归档；结论不迁移到实盘。

**④ 桌面壳与助手协作（P0，`mns-gui --features gui`）**
1. `src/bin/mns-gui` 启动库内 `gui::run()`；展示层从 `crate::app` 读取快照/计划/绩效等 typed 数据，投资规则仍由同一引擎模块裁决。
2. 启动与常驻：`run()` 先把 cwd 固定到 `~/.mns` 数据根；`autostart::launched_in_background` 识别 `--tray`/`--background`——登录自启动创建隐藏窗口只驻留托盘，手动启动正常显示。`launch_at_login` 开启时按当前 exe 路径刷新系统登录项（升级换目录后仍有效）。设置页开关先写系统、成功后才落盘，失败回滚。
3. 刷新、记账类操作与巡检简报通过 `gui::jobs` 后台执行；派生进程统一走 `gui::command`（Windows `CREATE_NO_WINDOW`）；托盘支持唤起主窗口/运行默认目标/立即巡检/停止任务/退出，并显示巡检计划状态。非 macOS 关闭窗口会真正销毁，`ensure_window` 从托盘按可见参数重建。
4. “助手”页通过 `agent` 运行时向外部 ACP CLI（默认 `opencode`，追加 `acp`）提交目标并消费 `AgentEvent` 时间线/权限请求；MNS 仅传入 CLI 路径与上下文，**不提供模型或凭据**。
5. 每日巡检：`maybe_start_daily` 每日最多触发一次（`~/.mns/agent/last-review` 标记，重启不重复）；jobs worker 以随应用分发的 `mns` 运行 `today --quiet`（或 `report`）生成简报——与终端输出字节一致；`routine::daily_review_prompt` 组装提示词（报告为唯一数据源、只读命令清单、`mns-decision` JSON 输出契约），以 `GoalSource::DailyReview` 提交。
6. 结论回传：每轮结束后 runner 从助手正文提取最后一个 `mns-decision` 围栏（仅认带标签围栏，非法 JSON 忽略）解析为 `DecisionReport`，GUI 渲染结论卡片并存档 `~/.mns/agent/runs/*.json`（原始 JSON 保留，容忍新字段）；会话时间线按对话写入 `conversations/`，“历史”面板只读回放或删除；Agent 全程不写文件。

## 技术选型
- **语言/版本**：Rust edition 2024；库 `money-never_sleep` + CLI/GUI 双二进制；`cargo build --release` 构建 CLI
- **CLI**：clap（子命令树）
- **桌面 UI（`gui` feature）**：`gpui-kit 0.9.0` + `gpui-tray 0.3.0`（`gpui-kit`/`menu-state`，与 `gpui-pre` 统一）；P0 窗口/托盘/页面；`gui-test` 用 `gpui-pre/test-support` 做 headless 布局测试
- **平台适配（Windows）**：`mns-gui` release 声明 `windows_subsystem = "windows"`（debug 保留控制台）；`build.rs` + `winresource`（仅 Windows build-dependency）把 `assets/app.ico` 编成 exe 的 `RT_GROUP_ICON`（GPUI 用 `LoadImageW` 取 exe 图标，`WindowOptions::icon` 仅 X11 生效）；子进程统一 `CREATE_NO_WINDOW`
- **外部助手（`agent` feature）**：`agent-client-protocol 5.0`（`process`，v1 schema），ACP over stdio 驱动外部 CLI（如 `opencode acp`）；启动配置兼容命令串（如 `npx opencode`）、额外参数与模型名内联配置；助手/GUI 设置（CLI 路径、模型、权限策略、巡检时间、`launch_at_login`）持久化于 `~/.mns/gui.toml`（temp+rename 原子写）
- **开机自启动**：写操作系统自身登录入口而非自建守护进程——Windows `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`（`reg add/delete`）、macOS `~/Library/LaunchAgents/com.mns.app.plist`、Linux `~/.config/autostart/mns.desktop`
- **存储**：SQLite（rusqlite，bundled），单文件 `~/.mns/mns.db`，单写者串行
- **HTTP**：reqwest（rustls）；CNN 抓取含重试/反爬容错
- **时间**：chrono（NaiveDate；日频 bar/决策日/披露时序/纪律复盘自然年）
- **输出**：comfy-table + 中文章节文本（`unicode-width` 对齐中文列宽）；报告落盘；桌面可视化组件
- **配置**：serde + toml，`~/.mns/config.toml`；兼容默认值与校验；助手设置（CLI 路径/模型/权限策略等）持久化于 `~/.mns/gui.toml`
- **错误**：anyhow，统一 `Error: <中文原因>` 前缀，CLI 以非零码退出
- **测试**：单元测试内联各模块（含纪律分桶/预算边界用例）；`examples/agent_smoke.rs`（`--features agent`）验证 ACP 链路
- **打包/发布**：macOS 用 `scripts/make-dmg.sh` 产出 `.app`/DMG（同版本携带 `mns` 与 `mns-gui`），图标由 `scripts/make-icons.swift` 生成；Windows 用 `scripts/make-installer.ps1` + `scripts/installer/mns.iss`（Inno Setup 6，按用户装到 `%LOCALAPPDATA%\Programs\MNS`，无 UAC、不改 PATH，含开始菜单/可选桌面快捷方式）产出 `target/dist/MNS-<version>-win-x64-setup.exe` 与便携 ZIP；`distribution/` 提供各平台 bin 包与 `money-never-sleep` 技能分发

## 系统边界
| 边界 | 对象 | 说明 / 信任边界 |
|---|---|---|
| 外部 API | CNN Fear & Greed `production.dataviz.cnn.io/index/fearandgreed/graphdata` | 需网络，可 418；只读，供决策与数据集构建参考 |
| 外部 API | 天天基金 / 东财行情 | 基金/净值/价格来源，多源回退；缺失时跳过单标的 |
| 外部 API | Yahoo Finance | `market`/`analyze` 等依赖；可能 403，视为环境限制 |
| 外部进程（ACP） | `opencode` 等 ACP 兼容 CLI（`agent` feature） | MNS 仅配置/发现 CLI 路径（支持命令串/额外参数/模型名）并通过 stdio 发指令，巡检/追问提示词由 MNS 组装；**不持有模型、推理端点或 API Key**，provider/凭据由外部 Agent 自管；权限策略由 MNS/用户约束目标执行；结论只经 `mns-decision` 块回传，Agent 不写文件 |
| 外部进程（本地） | `reg.exe`（登录项）、随包 `mns.exe`、`open`/`xdg-open` | 由 `gui::command` 派生；Windows 隐藏控制台窗口；只写用户级 HKCU，不需管理员 |
| 数据集构建 | `update-data` + Python 3 + 网络 | 重建 point-in-time 日频 CSV/meta；失败不应静默污染既有数据 |
| 本地存储 | SQLite 账本 + TOML 配置 + 助手/GUI 设置（`~/.mns/gui.toml`）+ 对话/决策归档（`~/.mns/agent/conversations|runs|last-review`）+ 系统登录项 + 报告/数据集/图标与打包产物 | 本地可信、单写者；并发写会锁库；`cash set` 为纠错通道；运行时产物统一解析到 `~/.mns` 数据根（与 cwd 无关） |
| 人机边界 | 用户 / 券商 | MNS 只记账与给建议，不下单、不阻断；护栏与纪律复盘仅为流程提示，助手结论也不自动执行；决策与责任在用户 |

## 代码映射索引
| Concept | Location | Notes |
|---|---|---|
| 库入口/feature 导出 | `src/lib.rs` | `app`/`discipline` 常驻；`agent`/`gui` 由 feature 控制；`money_never_sleep` 供 bins/示例复用 |
| CLI 命令树/参数 | `src/cli.rs` | Cash 子命令；`Guard`/`Performance`；`BacktestAction`（默认审计、Mirror/Asset/UpdateData/ValidateFull/Legacy/Validate） |
| CLI 入口分发 | `src/main.rs` | `use money_never_sleep::{...}`；`cmd_guard`/`cmd_performance`/`cmd_cash_deposit|withdraw`/`cmd_backtest_*` 等串行装配；`today --quiet` 输出供桌面巡检简报复用 |
| 应用快照/服务 | `src/app/mod.rs` | `Snapshot`/`PlanView`/`load_snapshot` 等 typed 视图；`Snapshot.discipline` 字段；`BuyGuardrail::build_note` 生成前缀 |
| 纪律复盘 | `src/discipline.rs` | 前缀常量、`bucket_of`（只认开头）、`YearReview`/`review`/`over_budget`；纯统计不推断意图 |
| 桌面壳入口与页面 | `src/bin/mns-gui.rs`、`src/gui/mod.rs`、`src/gui/pages/{dashboard,portfolio,plan,performance,agent,settings}.rs` | `windows_subsystem`、`command()`（`CREATE_NO_WINDOW`）、`window_hidden`/`ensure_window`、jobs、每日巡检调度、助手二级面板 |
| 复盘页纪律卡 | `src/gui/pages/performance.rs` | `discipline_card`：分桶与年度预算措辞（等于上限仍算预算内） |
| 图表/组件/自启动 | `src/gui/chart.rs`、`src/gui/components.rs`、`src/gui/autostart.rs` | 面积/曲线与卡片组件；`BACKGROUND_ARG`/`launched_in_background`/`is_enabled`/`set_enabled`（系统登录项） |
| 外部 ACP 助手 | `src/agent/mod.rs`、`src/agent/runner.rs`、`src/agent/routine.rs` | 设置（含 `launch_at_login`）、事件模型、`DecisionReport`、巡检提示词与 CLI 路径解析（`MNS_BIN` + exe 同目录 + `PATH` 前置） |
| 会话/决策归档 | `src/gui/conversations.rs` | 一对话一 JSON；回放只读、删除在 jobs worker 执行 |
| 配置与路径解析 | `src/config.rs` | `app_data_dir`/`resolve_data_path`；`backtest.annual_trade_budget_min/max` 供纪律卡对照 |
| 数据库/流水 | `src/db.rs` | schema、upsert；`list_transactions_since` 不限行数（`list_transactions` 仍取 12 条） |
| 决策信号 | `src/signal.rs` | `compute`（平滑/区间确认/决策日）、`next_decision_day`；生产与审计同源 |
| 再平衡计划 | `src/strategy.rs` | 目标权重+偏离带、先卖后买、`NO_INSTRUMENT_REASON` 缺标的清零 |
| 护栏/报告/审计 | `src/guardrail.rs`、`src/report.rs`、`src/audit.rs` | 冷静期提示；决策版/观察版报告与披露；默认实盘审计与 `legacy` 对照 |
| Windows 图标与打包 | `build.rs`、`scripts/make-installer.ps1`、`scripts/installer/mns.iss` | `winresource` 编入 exe 图标；Inno Setup 按用户安装 + 便携 ZIP，`mns.exe` 与 `mns-gui.exe` 同目录 |