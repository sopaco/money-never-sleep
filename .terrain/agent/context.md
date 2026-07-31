---
type: agent_context
project: money-never-sleep
title: Agent Architecture Context
source: .
---

## 项目概览

MNS（Money Never Sleeps）是面向个人投资者的**逆向投资决策助手**：本地单文件 CLI（Rust 静态二进制 v0.6.0）。不连接券商、不执行交易，只做「恐慌时提醒贪婪，贪婪时提醒恐慌」——拉取 CNN 恐贪指数 → 对照「目标仓位+偏离带」再平衡纪律 → 输出每日调仓建议（`mns report`），并用 2016–2025 真实全收益数据离线回测验证策略。

**消费方**：CLI 用户、Coding/Ask Agents。Agent 操作手册唯一权威：`distribution/skill/money-never-sleep/SKILL.md`。
**关键约束**：`mns buy/sell` 是登记已成交交易（记账非下单，无撤销）；报告是规则引擎机械输出、非投资建议；数字只能来自命令输出，不可编造。数据本地化 `~/.mns/`，无远程服务端。

## 架构设计

**四层分层架构**（单入口多出口，依赖倒置 + 适配器 + 策略模式）：

| 层 | 容器 | 职责 |
|---|---|---|
| 工具支持层 | `src/main.rs`、`src/cli.rs` | 启动、tokio 异步调度、clap 命令路由（17 顶级命令）、Fail-fast 初始化、anyhow 错误封装 |
| 基础设施层 | `config.rs`、`db.rs`、`models.rs`、`sentiment.rs`、`quote.rs`、`market.rs` | 配置加载/校验/读写、SQLite 事务持久化（5 表）、领域实体、外部行情 API 适配 |
| 核心业务层 | `strategy.rs`、`report.rs`、`backtest.rs`、`metrics.rs` | 调仓计划计算、日报编排渲染、离线回测引擎、金融绩效指标 |
| 外部系统 | CNN FGI、天天基金、Yahoo Finance、东方财富 | 情绪与价格数据源（仅出站只读） |

**关键模式**：依赖倒置（业务层依赖数据模型/函数签名，不碰 DB/HTTP）；外观（`main.rs` 统一调度）；适配器（`sentiment/quote` 封装外部 API 返回统一结构）；策略模式（`Engine` 枚举 + 配置驱动决策）；FIFO 批次 + 阶梯赎回费成本模型；嵌入式历史数据集（编译期内置 CSV，回测离线可跑）；事务性（`db.rs` 单事务保证账实一致）。

**数据流**：`main.rs` → 基础设施层取数 → `strategy.rs` 计算建议 → `report.rs` 渲染落盘；`backtest.rs` 独立闭环，不碰账本、不发网络。

## 模块地图

| 模块 | 职责 | 主要路径 |
|---|---|---|
| 系统入口 | init/config/init 化、异步调度、21 个 `cmd_*` 处理器 | `src/main.rs` |
| CLI 接口 | clap v4 derive 命令树（17 顶级命令）与参数解析 | `src/cli.rs` |
| 配置管理 | `AppConfig` 加载/校验/点路径读写（`mns config`）、目标权重曲线、再平衡/成本参数 | `src/config.rs` |
| 数据持久化 | SQLite 连接、原子事务、5 表 CRUD（cash/positions/transactions/price_history/fear_greed_snapshots）、按日去重 | `src/db.rs` |
| 数据模型 | `Position`/`Transaction`/`FearGreedSnapshot` 实体与收益率计算 | `src/models.rs` |
| 策略引擎 | 五区间情绪 → 目标权重、三腿调仓计划、买/卖建议、风险预警、亏损标的接飞刀过滤 | `src/strategy.rs` |
| 报告生成 | 中文日报渲染（comfy-table）、`save_report` 落盘 `reports/YYYY-MM-DD.txt` | `src/report.rs` |
| 回测引擎 | 4 引擎对比（目标仓位/旧框架/买入持有/买入持有+年度再平衡）、FIFO+阶梯赎回费、样本外验证+bootstrap | `src/backtest.rs` |
| 指标计算 | XIRR、最大回撤、均值/方差/下行偏差、Sharpe/Sortino/Calmar | `src/metrics.rs` |
| 情绪获取 | CNN FGI API 适配、历史值解析、快照入库 | `src/sentiment.rs` |
| 价格获取 | 天天基金 JSONP / Yahoo v8 / 东财 mobile 多源路由与全持仓自动更新 | `src/quote.rs` |
| 市场行情 | 全球指数、个股/ETF 报价、附加分析 | `src/market.rs` |

## 核心流程

**1. 每日报告（`mns report`）** — 需网络
1. 加载配置 + DB 现金/持仓
2. `sentiment::fetch_fear_greed_data()` 拉取恐贪指数（0–100），按日去重存快照
3. 情绪落五区间（极度恐慌<30/恐慌/中性/贪婪/极度贪婪≥70）→ `target_weight` 曲线得风险资产目标总权重（默认 85/75/60/45/35%）
4. 按 `allocation` 拆三条腿（us_stocks/cn_stocks/counter_cyclical），实际 vs 目标，偏离超带（默认 4pp）才建议且只补到目标
5. 风险预警 + 卖出建议（年化/绝对收益达阈值，最短持有 30 天）→ `generate_report` 渲染 → 写盘

**2. 交易记账（`mns add/buy/sell/cash`）** — 本地原子事务
1. `add` 建持仓池条目（份额 0），`buy/sell` 校验现金充足/份额足够，`cash add` 注资
2. 单事务内：更新 positions 成本 → 更新 cash → 追加 transactions → commit
3. 语义：登记用户已在券商完成的交易，不是下单；无撤销命令，错误记账须人工纠正

**3. 自动价格更新（`mns update-prices`）** — 需网络
1. `list_positions()` 取全部持仓
2. 按代码格式路由：6 位数字 → 天天基金（东财 mobile 回退）；字母 → Yahoo v8
3. 逐个 `fetch_price()` → `update_position_price()`（含 price_history 记录），单资产失败跳过继续

**4. 策略回测（`mns backtest` / `backtest validate`）** — 纯离线
1. 加载嵌入式月度数据（恐贪指数 + 纳指/红利低波/黄金，2016-01–2025-04，默认本金 10 万 + 年流入 5 万）
2. 4 引擎同数据同成本模型对比（目标仓位+偏离带 / 旧现金比例框架 / 买入持有 / 买入持有+年度再平衡）
3. `backtest validate` 样本外 holdout + bootstrap 收益分布（`--iterations/--block`），`backtest params` 列可调参数

## 技术选型

- **语言/构建**：Rust edition 2024，单一静态二进制（无运行时依赖），跨 darwin/linux/win，`.cargo/config.toml`
- **CLI**：clap v4（derive）+ tokio 全特性异步运行时
- **HTTP**：reqwest 0.12（rustls-tls，无 OpenSSL），浏览器 UA + Referer 反爬规避
- **配置**：TOML + serde → `AppConfig`，写入前校验（如权重曲线单调性、分配和=100%）
- **持久化**：rusqlite 0.39（bundled），SQLite ACID 事务，加权平均成本 + FIFO
- **时间**：chrono（serde feature）；**输出**：comfy-table + unicode-width
- **错误处理**：anyhow + Context，用户可读中文错误；**路径**：dirs 定位 `~/.mns/`
- **打包分发**：npm 分平台预编译包（`distribution/bin-*`），Agent 手册独立 skill（`distribution/skill/`）

## 系统边界

**外部 API（仅出站只读，无认证）**
- CNN Fear & Greed：`production.dataviz.cnn.io/index/fearandgreed/graphdata` — 恐贪指数；需浏览器 UA + Referer 规避 418
- 天天基金：`fundgz.1234567.com.cn/js/{code}.js` — 6 位国内基金净值（JSONP）
- Yahoo Finance v8：`query1.finance.yahoo.com/v8/finance/chart/{symbol}` — 美股/ETF/指数（部分网络环境不可用）
- 东方财富 mobile — 国内基金回退源

**本地存储（信任边界）**
- `~/.mns/config.toml` — 策略参数/API 端点/路径；加载与写入双校验，非法值拒绝写入
- `~/.mns/mns.db` — SQLite 5 表；`price_history` 按 (asset_code, price_date) 唯一，`fear_greed_snapshots` 按日去重
- `reports/YYYY-MM-DD.txt` — 每日中文报告存档

**信任与安全约束**：零券商连接、零交易执行；外部价格直接采信（仅解析校验）；配置即策略（改配置不改码）；回测局限诚实披露——买入持有年化（14.60%）优于策略（12.75%），策略价值仅在风险调整后（回撤 10.9% vs 14.9%，Calmar 1.17 vs 0.98），且 2000–2002/2008 熊市未覆盖。

## 代码映射索引

| 概念 | 位置 | 备注 |
|---|---|---|
| 入口与命令路由 | `src/main.rs` | 21 个 `cmd_*`，init/config/调度 |
| 命令树定义 | `src/cli.rs` | clap derive，17 顶级命令 |
| 配置模型与校验 | `src/config.rs` | `TargetWeight`/`Rebalance`/`Costs`/`Thresholds` 等 |
| SQLite 连接与 CRUD | `src/db.rs` | 5 表 init_tables、事务、去重快照 |
| 领域实体 | `src/models.rs` | Position/Transaction 与年化/绝对收益计算 |
| 调仓计划与建议 | `src/strategy.rs` | `RebalancePlan`/`BuySuggestion`/`SellSuggestion`/`RiskWarning` |
| 报告渲染与存盘 | `src/report.rs` | `generate_report`/`save_report` |
| 回测引擎 | `src/backtest.rs` | `Engine` 枚举、`SignalConfig`、嵌入式月度数据 |
| 金融指标 | `src/metrics.rs` | `xirr`/`max_drawdown`/`risk_metrics` |
| 恐贪指数适配 | `src/sentiment.rs` | CNN 拉取/解析/历史值提取 |
| 价格多源适配 | `src/quote.rs` | `fetch_price` 按代码路由、`update_all_prices` |
| 市场行情 | `src/market.rs` | `fetch_market_indices`/`fetch_stock_quote` |
| Agent 操作手册（权威） | `distribution/skill/money-never-sleep/SKILL.md` | 命令语法与策略语义唯一权威 |
| 打包与分发 | `distribution/bin-*/` | npm 分平台预编译包 |
| 人类可读文档 | `litho.docs/`、`.terrain/human/` | 架构/流程/边界/数据库/回测详解 |
| Agent 参考材料 | `.ai-context/SKILL.md`、`.ai-context/DYNAMICS.md` | 历史工程上下文与动力学文档 |