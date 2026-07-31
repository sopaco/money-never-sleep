---
type: agent_context
project: money-never-sleep
title: Agent Architecture Context
source: .
---

## 项目概览

MNS（Money Never Sleeps / Market Neutral Strategist）是面向个人投资者的**逆向投资决策助手**：本地单文件 CLI（Rust 静态二进制），不连接券商、不执行交易，只做「恐慌时提醒贪婪，贪婪时提醒恐慌」。核心价值是把「目标仓位 + 偏离带」的再平衡纪律固化为规则引擎：拉取 CNN 恐贪指数 → 对照目标权重曲线 → 输出每类资产的调仓建议，生成每日中文报告（`mns report`），并用 2016–2025 真实数据回测验证策略。

**消费方**：CLI 用户、Coding/Ask Agents（操作手册权威来源为 `distribution/skill/money-never-sleep/SKILL.md`）。
**关键约束**：`mns buy/sell` 是记账而非下单；报告输出是规则引擎机械结果，非投资建议；数字只能来自命令输出，不可编造。数据本地化于 `~/.mns/`，无远程服务端。

## 架构设计

**四层分层架构**（单入口、多出口，松耦合）：

| 层 | 容器 | 职责 |
|---|---|---|
| 工具支持层 | `main.rs`、`cli.rs` | 程序启动、clap 命令解析、Fail-fast 初始化、命令路由 |
| 基础设施层 | `config.rs`、`db.rs`、`models.rs`、`sentiment.rs`、`quote.rs`、`market.rs` | 配置加载/校验、SQLite 事务持久化、外部 API 适配、价格获取 |
| 核心业务层 | `strategy.rs`、`report.rs`、`backtest.rs`、`metrics.rs` | 策略计算、报告编排、回测验证、绩效指标 |
| 外部系统 | CNN FGI API、天天基金、Yahoo Finance | 市场情绪与价格数据源 |

**关键设计模式**：依赖倒置（模块依赖数据模型/函数签名，可 mock）；外观模式（`cli.rs` 统一入口隐藏内部复杂度）；适配器模式（`sentiment.rs`/`quote.rs` 封装外部 API，返回统一结构）；策略模式（配置驱动决策权重）；事务性（`db.rs` 原子事务保证账实一致）；嵌入式历史数据（`include_str!` 编译期编入 CSV）。

**数据流方向**：`cli.rs` 依赖基础设施层取数 → 传给 `strategy.rs` 计算建议 → `report.rs` 渲染输出；`strategy.rs` 仅输出建议，不碰 DB、不发 HTTP。

## 模块地图

| 模块 | 职责 | 主要路径 |
|---|---|---|
| 系统入口 | 初始化（配置/DB/报告目录）、异步调度、错误封装 | `src/main.rs` |
| CLI 接口 | clap v4 命令树定义（17 命令）与参数解析 | `src/cli.rs` |
| 配置管理 | `AppConfig` 加载/校验/点路径查询/设置，API 端点与策略参数 | `src/config.rs` |
| 数据持久化 | SQLite 连接、事务、4 表 CRUD（cash/positions/transactions/fear_greed_snapshots） | `src/db.rs` |
| 数据模型 | 核心实体与业务计算方法 | `src/models.rs` |
| 策略引擎 | 目标仓位+偏离带调仓计划、买/卖建议、风险预警、接飞刀过滤 | `src/strategy.rs` |
| 报告生成 | 中文日报生成（comfy-table）与存盘 `reports/YYYY-MM-DD.txt` | `src/report.rs` |
| 回测引擎 | 多资产回测、买入持有基准、参数对比、样本外验证 | `src/backtest.rs` |
| 指标计算 | XIRR、最大回撤、均值/方差（回测与报告共用） | `src/metrics.rs` |
| 情绪获取 | CNN 恐贪指数 API 适配与历史值解析、快照入库 | `src/sentiment.rs` |
| 价格获取 | 天天基金/Yahoo/Eastmoney 多源适配与自动更新 | `src/quote.rs` |
| 市场行情 | 全球 9 大指数、个股/ETF 报价 | `src/market.rs` |

## 核心流程

**1. 每日报告生成（`mns report`）** — 需网络
1. 加载配置 `AppConfig`，打开 DB（现金、持仓）
2. `sentiment::fetch_fear_greed_index()` 拉取恐贪指数（0–100），`save_fear_greed_snapshot()` 按日去重入库
3. 情绪落入五区间（极度恐慌<30/恐慌/中性/贪婪/极度贪婪≥70）→ 查 `target_weight` 曲线得风险资产目标总权重
4. 按配置拆到三条腿（us_stocks/cn_stocks/counter_cyclical），实际权重与目标权重比较，偏离超带（默认 4pp）才建议，且只补到目标为止
5. `check_risk_warnings()` 输出风险预警 → `report::generate_report()` 渲染 → `save_report()` 写盘

**2. 交易记账（`mns buy/sell/add/price`）** — 本地原子事务
1. `add` 先建持仓池条目（份额 0），`buy/sell` 校验现金充足/份额足够
2. 单个 SQLite 事务内：更新 positions（加权平均成本重算）→ 更新 cash → 追加 transactions 审计记录 → commit
3. 注意：这是**登记用户已在券商完成的交易**，不是下单，不可据此「执行」建议

**3. 自动价格更新（`mns update-prices`）** — 需网络
1. `list_positions()` 取全部持仓
2. 按代码格式路由：6 位数字 → 天天基金；字母 → Yahoo Finance（Eastmoney mobile 为国内基金回退源）
3. 逐个 `fetch_price()` → `update_position_price()`，单资产失败跳过继续，输出更新表

**4. 策略回测（`mns backtest`）** — 纯离线，不碰账本
1. 加载嵌入式 CSV（恐贪指数 + 纳指/红利低波/黄金月度全收益数据）
2. `run_multi_asset_backtest()` 跑目标仓位策略 vs `run_buy_and_hold()` 基准
3. `run_backtest()` 多组配置变体对比，`backtest validate` 做样本外验证 + 收益分布（`--iterations/--block` 可调）

## 技术选型

- **语言/构建**：Rust 1.92+，edition 2024，cargo 单一静态二进制（无运行时依赖），跨平台（darwin/linux/win）
- **CLI**：clap v4（derive）+ tokio 全特性异步运行时
- **HTTP**：reqwest 0.12（rustls-tls，无 OpenSSL 依赖）
- **配置**：TOML + serde 序列化到 `AppConfig`
- **持久化**：rusqlite 0.39（bundled SQLite），ACID 事务
- **时间**：chrono（含 serde feature）
- **输出**：comfy-table 渲染 ASCII 表格，unicode-width 对齐
- **错误处理**：anyhow + Context，用户可读中文错误
- **辅助**：dirs 定位 `~/.mns/`，serde_json 解析 API
- **打包**：npm 化分平台包（`distribution/bin-*`），Agent 手册独立 skill（`distribution/skill/`）

## 系统边界

**外部 API（仅出站读取，无认证，未校验响应可信度）**
- CNN Fear & Greed API：`https://production.dataviz.cnn.io/index/fearandgreed/graphdata` — 股票市场恐贪指数；需浏览器 UA + Referer 头规避 418 反爬
- 天天基金：`http://fundgz.1234567.com.cn/js/{code}.js` — 6 位国内基金净值
- Yahoo Finance v8：`https://query1.finance.yahoo.com/v8/finance/chart/{symbol}` — 美股/ETF/指数（部分网络环境不可用）
- Eastmoney mobile：国内基金回退数据源

**本地存储（信任边界）**
- `~/.mns/config.toml` — 策略参数/API 端点/DB 与报告路径；加载时校验（如分配和≠100% 拒绝）
- `~/.mns/mns.db` — SQLite 4 表：cash、positions（asset_code 唯一）、transactions、fear_greed_snapshots（按日去重）；事务保障一致性
- `./reports/YYYY-MM-DD.txt` — 每日中文报告

**信任与安全约束**：完全不连接券商、不执行交易；`buy/sell` 纯记账且无撤销命令；外部价格直接采信（仅解析校验）；配置即策略，规则可改不须改码；回测局限诚实披露（买入持有年化优于策略，价值仅在风险调整后）。

## 代码映射索引

| 概念 | 位置 | 备注 |
|---|---|---|
| CLI 入口与命令路由 | `src/main.rs` | 17 个 `cmd_*`，初始化/调度 |
| 命令树定义 | `src/cli.rs` | clap derive |
| `AppConfig` 配置模型与校验 | `src/config.rs` | `TargetWeight`/`Rebalance`/`Costs` 等 |
| SQLite 连接与 CRUD | `src/db.rs` | 事务、加权平均成本 |
| 领域实体 | `src/models.rs` | Position/Transaction/FearGreedSnapshot |
| 调仓计划与买卖建议 | `src/strategy.rs` | `RebalancePlan`/`LegPlan`/`RiskWarning` |
| 报告生成 | `src/report.rs` | `generate_report`/`save_report` |
| 回测引擎 | `src/backtest.rs` | 嵌入式月度数据集 |
| 金融指标 | `src/metrics.rs` | xirr/max_drawdown/mean/stddev |
| 恐贪指数适配 | `src/sentiment.rs` | CNN API + 历史值提取 |
| 价格多源适配 | `src/quote.rs` | `fetch_price` 按代码路由 |
| 市场指数 | `src/market.rs` | `fetch_market_indices` |
| Agent 操作手册（权威） | `distribution/skill/money-never-sleep/SKILL.md` | 命令语法与策略语义唯一权威 |
| 人类可读架构文档 | `litho.docs/` | 架构概览/边界/工作流等 |
| 分平台二进制打包 | `distribution/bin-*/` | npm 包装 |