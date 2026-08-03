---
type: agent_context
project: money-never-sleep
title: Agent Architecture Context
source: .
---

## 项目概览

MNS（Money Never Sleeps）是一个运行在本机的**逆情绪投资决策助手**，Rust 编写的单文件 CLI（`mns`，v0.6.0，edition 2024）。它抓取 CNN 恐贪指数，按"越恐慌仓位越高、越贪婪仓位越低"的目标权重曲线计算应持有的风险仓位，对照本地账本生成再平衡调仓建议，并留档为文本报告。核心性格是**诚实**：回测主动承认买入持有收益更高（年化 14.6% vs 12.8%）、恐贪指数边际贡献接近零，且明确标注策略未覆盖长熊市。工具刻意**不做自动交易**，只当参谋——`mns buy/sell` 是记账而非下单。消费方为人类用户与 AI Agent（`distribution/skill`）。数据与配置存于 `~/.mns/`，报告存于 `./reports/`。

## 架构设计

单进程本地 CLI，无网络服务端；三层结构 + 纯离线回测。

| 层 | 说明 |
|---|---|
| 命令层 | `main.rs` 按 `cmd_*` 函数分发 CLI 子命令；`cli.rs` 用 clap derive 定义 `Commands` 枚举与参数 |
| 领域层 | 情绪采集（`sentiment.rs`）、行情采集（`quote.rs`/`market.rs`）、策略引擎（`strategy.rs`）、报告生成（`report.rs`）、回测（`backtest.rs`）、指标（`metrics.rs`）、配置（`config.rs`） |
| 持久层 | `db.rs`（rusqlite 账本）+ `models.rs`（领域结构体）+ `config.toml` |

依赖方向：命令层 → 领域层 → 持久层；领域层仅通过 `models`/`config` 共享结构体，模块间无环形依赖。策略引擎输出纯数据（`RebalancePlan`），报告层只做渲染。

主要内部依赖：
- `main.rs` 调用 `db::Database`、`sentiment`、`strategy::calculate_rebalance_plan`、`report::generate_report`
- `strategy.rs` 依赖 `config::AppConfig`、`models::Position`
- `backtest.rs` 依赖 `config::Costs`、`models::Position`、`metrics`
- `quote.rs`/`market.rs` 经 `reqwest` 访问外部数据源

## 模块地图

| Module | Responsibility | Primary paths |
|---|---|---|
| CLI 入口 | 子命令分发（`cmd_init`/`cmd_report`/`cmd_buy`…）、参数解析 | `src/main.rs`, `src/cli.rs` |
| 配置 | `AppConfig` 全部策略参数（阈值、三腿配置、目标权重曲线、偏离带、费用、费率 URL）；读写 `~/.mns/config.toml`，校验单调性 | `src/config.rs` |
| 本地账本 | SQLite：现金、持仓、交易流水、价格历史、恐贪快照；买卖为记账（原子事务、加权成本、校验现金/份额） | `src/db.rs`, `src/models.rs` |
| 情绪采集 | 抓取并解析 CNN 恐贪指数及前日/周/月/年同比，内置重试 | `src/sentiment.rs` |
| 行情采集 | 按类别路由数据源（国内基金 EastMoney/天天基金、美股 Yahoo），单条失败跳过；市场指数与个股报价 | `src/quote.rs`, `src/market.rs` |
| 策略引擎 | 目标权重 vs 实际权重、偏离带、买入/卖出建议、风险警告、逆周期加仓、先卖后买 | `src/strategy.rs` |
| 报告生成 | 渲染含固定章节的报告并写入 `reports/YYYY-MM-DD.txt`（先留档再决策） | `src/report.rs` |
| 回测 | 内置 2016–2025 月度数据集，四引擎对比、趋势/情绪锚、FIFO 批次成本、样本外验证、参数清单 | `src/backtest.rs` |
| 绩效指标 | XIRR、最大回撤、Sharpe/Sortino/Calmar、下侧偏差、block bootstrap、确定性 RNG | `src/metrics.rs` |
| 分发 | 跨平台 npm 包（darwin-arm64 / linux-x64 / win-x64）、Agent 操作手册 | `distribution/` |

## 核心流程

**1. 每日策略报告（`mns report`）**
1. 经 `sentiment.rs` 抓取当日恐贪指数与历史同比，`db.save_fear_greed_snapshot` 落库（同日覆盖）。
2. 读入现金余额与全部持仓。
3. `strategy::calculate_rebalance_plan`：情绪→风险资产目标权重→按三腿配置拆分→逐腿对比实际权重与目标、判定偏离带与动作（买/卖/持有），生成 `RebalancePlan` 与风险警告。
4. `report::generate_report` 渲染六大章节并 `save_report` 到 `reports/YYYY-MM-DD.txt`；无动作是正常健康状态。

**2. 登记已成交交易（`mns buy/sell`）**
1. 校验标的已 `add`、现金余额/持有份额充足。
2. 在单个 `unchecked_transaction` 内：更新持仓（份额 + 加权成本价 + 首买日期）、更新现金、写入 `transactions` 流水。
3. 历史记录可经 `mns portfolio`/`mns history` 复核；不可撤销。

**3. 行情更新（`mns update-prices`）**
1. 按 `category` 选择数据源：`cn_stocks`/`counter_cyclical` 走 EastMoney 移动端 → 天天基金 JSONP 兜底；美股走 Yahoo。
2. 每条价格经 `db.update_price` 写入 `positions.current_price` 并记入 `price_history`（同日去重）。
3. 单条失败跳过继续，报告前应刷新价格。

**4. 策略回测（`mns backtest`）**
1. 解析内置月度数据集（FGI + 三腿价格）为 `MonthRow`。
2. 平滑 FGI，计算目标风险权重（情绪锚或趋势锚 ± 情绪倾斜）。
3. 按月模拟：三腿按目标权重与偏离带再平衡，卖出按 FIFO 批次、计阶梯赎回费与最短持有天数，现金计息。
4. 汇总指标（XIRR/回撤/Sharpe 等）对比四种引擎；`backtest validate` 用 block bootstrap 做样本外验证与收益分布。

## 技术选型

- **语言/构建**：Rust edition 2024，单静态二进制，MIT；无运行时依赖。
- **CLI**：clap 4（derive）；输出用 comfy-table + unicode-width。
- **网络**：tokio 1 + reqwest 0.12（rustls-tls，无默认 TLS），JSONP/JSON 抓取。
- **序列化**：serde / serde_json；配置用 toml。
- **存储**：rusqlite 0.39（bundled SQLite）单文件账本；chrono 处理日期与 XIRR。
- **错误处理**：anyhow，中文错误信息 + 非零退出码。
- **分发**：`distribution/` 下三平台 npm 预编译包（`@never-sleeps/mns-cli`），含 OpenClaw/Agent skill 打包。

## 系统边界

| 边界 | 说明 |
|---|---|
| CNN 恐贪指数 API | `https://production.dataviz.cnn.io/index/fearandgreed/graphdata`；需网络，可能 418 反爬，内置重试 |
| Yahoo Finance | `market`/`market-indices`/`analyze` 及美股报价依赖；部分网络环境 403 属环境限制 |
| EastMoney / 天天基金 | 国内基金价格 JSONP 数据源，含移动端接口与兜底链路 |
| 本地 SQLite | `~/.mns/mns.db`；串行调用，防锁；`mns init` 会清库（有确认） |
| 本地配置 | `~/.mns/config.toml`；旧配置缺新字段自动取默认值，非法值校验拒绝写入 |
| 信任边界 | 不连接券商、不自动交易；`buy/sell` 仅记账，错误登记会污染全部下游数值；报告为规则引擎机械输出而非投资建议；回测结论不可外推为收益预期 |

## 代码映射索引

| Concept | Location | Notes |
|---|---|---|
| 命令分发 | `src/main.rs` | `cmd_*` 21 个子命令处理 |
| CLI 枚举/参数 | `src/cli.rs` | clap derive `Commands`/`CashAction`/`BacktestAction` |
| 策略配置 | `src/config.rs` | `AppConfig`+`TargetWeight`/`Rebalance`/`Costs`/`ApiConfig`，含默认曲线与校验 |
| 本地账本 | `src/db.rs` | `Database`：现金/持仓/交易/价格历史/恐贪快照 |
| 领域结构体 | `src/models.rs` | `Position`（年化/绝对收益）、`Transaction`、`FearGreedSnapshot` |
| 策略引擎 | `src/strategy.rs` | `calculate_rebalance_plan`、`RebalancePlan`、买/卖建议、风险警告 |
| 报告渲染 | `src/report.rs` | `generate_report`、`save_report`，六大章节 |
| 情绪采集 | `src/sentiment.rs` | `fetch_fear_greed_data`、CNN 响应解析 |
| 价格数据源 | `src/quote.rs` | EastMoney/天天基金/Yahoo 多源回退，`update_all_prices` |
| 市场/个股行情 | `src/market.rs` | `fetch_market_indices`、`fetch_stock_quote` |
| 回测引擎 | `src/backtest.rs` | `Engine`×`Anchor`、FIFO 批次 `Leg`/`Lot`、`run`、holdout |
| 风险指标 | `src/metrics.rs` | `xirr`、`max_drawdown`、`risk_metrics`、`block_bootstrap` |
| Agent 操作手册 | `distribution/skill/money-never-sleep/SKILL.md` | 命令速查、硬约束、故障模式（权威行为文档） |
| 内置回测数据 | `src/backtest.rs` | `load_main`/`load_holdout` 月度 FGI+价格数据集 |