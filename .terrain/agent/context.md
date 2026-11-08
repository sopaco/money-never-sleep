---
type: agent_context
project: money-never-sleep
title: Agent Architecture Context
source: .
---

## 项目概览

MNS 是一套运行在本机的**个人投资决策工具**（Rust）：提供 CLI 二进制 `mns`，并新增 feature-gated 的桌面壳 `mns-gui`（P0，`gui` feature；`cargo run --bin mns-gui --features gui`，不启用时 CLI 依赖与行为不变）。核心引擎编译为库 `money_never_sleep`，由 CLI、桌面壳与示例共享。核心命题：把 CNN 恐贪指数（0–100）映射成风险资产**目标仓位**（越恐慌越高，35%–85%），再按偏离带（默认 4pp）给出买/卖/持有建议。策略重构后强化“规避情绪化投资”：情绪信号只设定目标敞口，`guardrail` 仅在过热区给出冷静期提示/二次确认，**不阻断交易、也不替代决策**；`mns buy/sell` 与桌面记账仍只把用户已在券商完成的交易记入本地账本，**零券商连接**。验证链路强调可审计与同源：`mns backtest` 默认执行完整实盘审计（信号同源、T+lag 执行、通道成本、回撤双口径），并提供账户镜像/单资产对照；旧月频代理对比降级为 `legacy`，结论不可迁移到实盘。报告先留档再决策（`reports/YYYY-MM-DD.txt`）。

## 架构设计

分层架构：CLI 串行命令 + 可选 GPUI 桌面壳，共享同一套账本/策略/审计引擎；无服务端、无券商连接。桌面壳的展示与交互通过 `app` 提供的 typed 只读/服务视图消费引擎；后台任务在 `gui::jobs` 与 `agent` ACP 运行时中执行。

| 层 | 容器 | 职责 | 关键文件 |
|---|---|---|---|
| 交互层 | clap CLI / GPUI Kit 桌面壳 | CLI 命令与参数；桌面窗口/托盘/页面（总览/持仓/计划/复盘/助手/设置） | `src/cli.rs`、`src/bin/mns-gui.rs`、`src/gui/**` |
| 入口层 | 命令分发 / 库入口 | CLI `cmd_*` 装配（`use money_never_sleep::{...}`）；库导出 `app/agent/gui` 等模块；GUI 入口 `money_never_sleep::gui::run()` | `src/main.rs`、`src/lib.rs` |
| 应用服务层 | typed 视图/快照 | 供 CLI/GUI/助手共享的应用服务与快照（如 `Snapshot`/`PlanView`、`load_snapshot`），避免前端各自拼装账本 | `src/app/mod.rs` |
| 助手层（`agent` feature） | 外部 ACP 运行时 | 通过 `agent-client-protocol` 驱动外部 ACP CLI（默认发现 `opencode`，追加 `acp` 子命令）；MNS 不持有模型/推理端点/API Key，凭据留在外部 Agent；命令/事件/权限策略/设置 | `src/agent/mod.rs`、`src/agent/runner.rs` |
| 决策层 | 策略引擎 | 情绪→目标权重→调仓计划 | `src/strategy.rs` |
| 决策层 | 信号处理 | 恐贪指数平滑、区间确认、决策日过滤（审计同源复用） | `src/signal.rs` |
| 决策层 | 冷静期护栏 | 过热区二次确认提示，不参与仓位/交易决策、不阻断 | `src/guardrail.rs` |
| 决策层 | 报告生成 | 决策/观察版章节化文本 + 落盘；可接入护栏提示 | `src/report.rs` |
| 审计/验证层 | 实盘审计引擎 | 同源信号、T+lag、通道成本、在途资金、回撤双口径 | `src/audit.rs` |
| 审计/验证层 | 数据集与镜像数据 | point-in-time 日频 bar/净值、数据新鲜度、账户镜像输入 | `src/live_data.rs` |
| 验证层 | 回测/对照引擎 | 历史对比/实验能力；`legacy` 月频代理（降级） | `src/backtest.rs`、`src/metrics.rs` |
| 数据层 | SQLite 账本 | 现金/持仓/交易/价格/情绪快照/资金流水等 | `src/db.rs`、`src/models.rs` |
| 配置层 | TOML 配置 | 参数+校验+护栏/通道/回测审计等配置 | `src/config.rs` |
| 数据源层 | 情绪抓取 | CNN 恐贪指数 API | `src/sentiment.rs` |
| 数据源层 | 行情抓取 | 天天基金/东财/Yahoo 多源 | `src/quote.rs`、`src/market.rs` |

主要依赖（`Cargo.toml`）：rusqlite、reqwest、clap、chrono、serde/toml、anyhow、comfy-table；feature `agent` 增加 `agent-client-protocol`（`process`）；feature `gui`（含 `agent`）增加 `gpui-kit 0.9.0`、`gpui-tray 0.3.0`（`gpui-kit` 后端，共享 `gpui-pre`）。库 + `[[bin]] mns` / `[[bin]] mns-gui`（`required-features=["gui"]`）；`distribution/` 下按平台打包 npm 包，并提供 Agent skills。

## 模块地图

| Module | 责任 | 主要路径 |
|---|---|---|
| 库入口/前端共享 | 导出核心模块；始终暴露 `app`；按 feature 暴露 `agent`/`gui` | `src/lib.rs`、`src/app/mod.rs` |
| CLI 入口/分发 | `cmd_*` 串行装配；消费 `money_never_sleep` 库；`report/today/plan/guard/performance`；backtest 默认审计及 `mirror/asset/update-data/validate-full/legacy/validate`；cash `deposit/withdraw/set` | `src/main.rs`、`src/cli.rs` |
| 桌面壳（P0，GUI） | GPUI Kit 窗口/托盘、页面路由（Dashboard/Portfolio/Plan/Performance/Agent/Settings）、后台 jobs、主题/图表/情绪氛围 | `src/bin/mns-gui.rs`、`src/gui/mod.rs`、`src/gui/pages/*.rs`、`src/gui/{chart,components,jobs,mood,theme}.rs` |
| 外部助手运行时 | ACP 连接外部 `opencode` 等 CLI；目标提交、事件流、权限策略（不托管模型/凭据） | `src/agent/mod.rs`、`src/agent/runner.rs`、`examples/agent_smoke.rs` |
| 配置 | `AppConfig`（Settings/Allocation/Thresholds/BuyRatio/SellRatio/Api/TargetWeight/Rebalance/Costs/Cashflow/Profile 等）；`guardrail`、通道/回测/审计相关配置与校验；`~/.mns/config.toml` | `src/config.rs` |
| 账本 | SQLite：`cash`、`positions`、`transactions`、`fear_greed_snapshots`、资金流水等；情绪历史读取 | `src/db.rs`、`src/models.rs` |
| 信号处理 | `DecisionSignal`：平滑、区间确认、决策日；历史不足降级；供实盘与审计同源复用 | `src/signal.rs` |
| 策略引擎 | 再平衡计划/买卖建议/风险警告；`RebalancePlan` | `src/strategy.rs` |
| 冷静期护栏 | `evaluate/render/print_advice`；过热阈值提示与统计免责声明；不改仓、不阻断 | `src/guardrail.rs` |
| 报告 | 决策版/观察版生成 + 落盘；结合有效分数与护栏提示 | `src/report.rs` |
| 实盘审计 | 同源信号、T+lag、通道成本、在途资金、回撤双口径；账户镜像/策略有效性/分段验证 | `src/audit.rs`、`src/live_data.rs` |
| 回测/对照 | `Engine`/对比实验与 `legacy` 月频代理（降级，不可迁移实盘）；`metrics` 供回测与绩效 | `src/backtest.rs`、`src/metrics.rs` |
| 情绪/行情 | CNN 恐贪指数；东财/天天/Yahoo 多源价格与市场行情 | `src/sentiment.rs`、`src/quote.rs`、`src/market.rs` |
| 打包/技能/图标 | 平台 npm 包；Agent 操作/决策/回测技能；桌面图标生成与 DMG 打包脚本 | `distribution/`、`.agents/skills/{mns-advisor,mns-backtest,gpui-component-skill}/`、`scripts/make-dmg.sh`、`scripts/make-icons.swift`、`assets/*` |

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

**③ 实盘审计与验证（`mns backtest` 默认；`mirror`/`asset`/`update-data`/`legacy`）**
1. 默认 `mns backtest`：加载日频数据集与配置，运行**完整实盘审计**（同源 `signal::compute`、T 日信号/T+lag 成交、通道分离成本与在途资金、剔除资金流复合净值与含资金流账户净值双口径，日频回撤）。
2. `mirror`：严格执行规则 vs 真实流水的账户镜像对照；`asset --dataset ...`：单资产/数据集对照实验。
3. `update-data`：重建实盘与长周期日频数据集（需网络与 Python 3，point-in-time 约束避免前视偏差）。
4. `legacy`：保留旧月频代理策略对比，已降级，**结论不可迁移到实盘**。

**④ 桌面壳与助手协作（P0，`mns-gui --features gui`）**
1. `src/bin/mns-gui` 启动库内 `gui::run()`；展示层从 `crate::app` 读取快照/计划/绩效等 typed 数据，投资规则仍由同一引擎模块裁决。
2. 刷新、记账类操作通过 `gui::jobs` 后台执行；托盘支持唤起主窗口/运行默认目标/停止任务/退出。
3. “助手”页通过 `agent` 运行时向外部 ACP CLI（默认 `opencode`，追加 `acp`）提交目标并消费 `AgentEvent` 时间线/权限请求；MNS 仅传入 CLI 路径与上下文，**不提供模型或凭据**。

## 技术选型

- **语言/版本**：Rust edition 2024；库 `money-never_sleep` + CLI/GUI 双二进制；`cargo build --release` 构建 CLI
- **CLI**：clap（子命令树）
- **桌面 UI（`gui` feature）**：`gpui-kit 0.9.0` + `gpui-tray 0.3.0`（`gpui-kit`/`menu-state`，与 `gpui-pre` 统一）；P0 窗口/托盘/页面
- **外部助手（`agent` feature）**：`agent-client-protocol 5.0`（`process`），ACP over stdio 驱动外部 CLI（如 `opencode acp`）
- **存储**：SQLite（rusqlite），单文件 `~/.mns/mns.db`，单写者串行
- **HTTP**：reqwest；CNN 抓取含重试/反爬容错
- **时间**：chrono（NaiveDate；日频 bar/决策日/披露时序）
- **输出**：comfy-table + 中文章节文本；报告落盘；桌面可视化组件
- **配置**：serde + toml，`~/.mns/config.toml`；兼容默认值与校验；助手设置（CLI 路径/权限策略等）持久化
- **错误**：anyhow，统一 `Error: <中文原因>` + 非零退出码
- **数据构建**：`.agents/skills/mns-backtest/data/*.py`（Python 3）生成 `hs300/live/long_daily.csv` 与 meta（`update-data` 调用）
- **打包/发布**：平台化 npm 包；`scripts/make-icons.swift` 生成图标资产（`.icns/.ico/PNG/RGBA`）；`scripts/make-dmg.sh` 产出 `.app/.dmg`（签名/公证边界见 `docs/gui-release.md`）；`distribution/skill/` 与 `distribution/skills/*.zip`
- **测试**：`#[cfg(test)]` 单测覆盖策略/护栏/审计等关键逻辑；`examples/agent_smoke.rs`（`--features agent`）验证 ACP 链路

## 系统边界

| 边界 | 对象 | 说明 / 信任边界 |
|---|---|---|
| 外部 API | CNN Fear & Greed `production.dataviz.cnn.io/index/fearandgreed/graphdata` | 需网络，可 418；只读，供决策与数据集构建参考 |
| 外部 API | 天天基金 / 东财行情 | 基金/净值/价格来源，多源回退；缺失时跳过单标的 |
| 外部 API | Yahoo Finance | `market`/`analyze` 等依赖；可能 403，视为环境限制 |
| 外部进程（ACP） | `opencode` 等 ACP 兼容 CLI（`agent` feature） | MNS 仅配置/发现 CLI 路径并通过 stdio 发指令；**不持有模型、推理端点或 API Key**，provider/凭据由外部 Agent 自管；权限策略由 MNS/用户约束目标执行 |
| 数据集构建 | `update-data` + Python 3 + 网络 | 重建 point-in-time 日频 CSV/meta；失败不应静默污染既有数据 |
| 本地存储 | SQLite 账本 + TOML 配置 + 助手/GUI 设置 + 报告/数据集/图标与打包产物 | 本地可信、单写者；并发写会锁库；`cash set` 为纠错通道 |
| 桌面环境 | 窗口/托盘、`assets/*`、`scripts/make-dmg.sh`/`make-icons.swift` | P0 桌面壳与分发工件；签名/公证/架构边界见 `docs/gui-*.md` |
| 人机边界 | 用户已成交交易与资金流 | `buy/sell/deposit/withdraw` 及桌面记账均为记账非下单；护栏/建议不阻断执行，责任在用户 |
| 无边界 | 券商/银行 | **零连接**，从不发起真实交易 |

## 代码映射索引

| Concept | Location | Notes |
|---|---|---|
| 库入口/feature 导出 | `src/lib.rs` | `app` 常驻；`agent`/`gui` 由 feature 控制；`money_never_sleep` 供 bins/示例复用 |
| CLI 命令树/参数 | `src/cli.rs` | Cash 子命令；`Guard`/`Performance`；`BacktestAction`（默认审计、Mirror/Asset/UpdateData/ValidateFull/Legacy/Validate） |
| CLI 入口分发 | `src/main.rs` | `use money_never_sleep::{...}`；`cmd_guard`/`cmd_performance`/`cmd_cash_deposit|withdraw`/`cmd_backtest_*` 等串行装配 |
| 应用快照/服务 | `src/app/mod.rs` | `Snapshot`/`PlanView`/`load_snapshot` 等 typed 视图，供 CLI/GUI 共享 |
| 桌面壳入口与页面 | `src/bin/mns-gui.rs`、`src/gui/mod.rs`、`src/gui/pages/{dashboard,portfolio,plan,performance,agent,settings}.rs` | P0 窗口/托盘、jobs、chart/components/mood/theme |
| 外部 ACP 助手 | `src/agent/mod.rs`、`src/agent/runner.rs`、`examples/agent_smoke.rs` | `start_runtime`/`AgentCommand`/`AgentEvent`/`PermissionPolicy`；外接 `opencode acp`；MNS 不持有凭据 |
| 配置与校验 | `src/config.rs` | 目标权重/再平衡/成本/通道/护栏/回测审计等；TOML 读写 |
| 数据库/流水 | `src/db.rs` | 现金、持仓、交易、情绪快照、资金流水与历史查询 |
| 领域类型 | `src/models.rs` | Position、Transaction |
| 决策信号（同源） | `src/signal.rs` | `compute`/`DecisionSignal`；实盘与审计共用 |
| 再平衡计划 | `src/strategy.rs` | RebalancePlan/建议/风险警告 |
| 冷静期护栏 | `src/guardrail.rs` | `evaluate`/`render`/`print_advice`；提示不阻断 |
| 报告生成 | `src/report.rs` | 决策/观察版 + 落盘 |
| 实盘审计引擎 | `src/audit.rs` | 同源信号、T+lag、通道成本、在途资金、回撤双口径、镜像/分段 |
| 日频数据集 | `src/live_data.rs`、`.agents/skills/mns-backtest/data/*.py`、`.agents/skills/mns-backtest/data/*_daily.csv|*_meta.json` | `BarSeries`/point-in-time；`update-data` 构建 CSV/meta |
| 回测/legacy 与指标 | `src/backtest.rs`、`src/metrics.rs` | 历史引擎、`legacy` 月频代理（降级）、XIRR/回撤 |
| 情绪/行情 | `src/sentiment.rs`、`src/quote.rs`、`src/market.rs` | CNN、天天/东财/Yahoo |
| 打包/图标/技能 | `Cargo.toml`、`scripts/make-dmg.sh`、`scripts/make-icons.swift`、`assets/*`、`.agents/skills/mns-advisor/`、`.agents/skills/mns-backtest/`、`distribution/` | feature/bin 定义、DMG/图标、实盘决策与回测技能、平台分发 |