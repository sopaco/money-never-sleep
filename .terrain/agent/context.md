---
type: agent_context
project: money-never-sleep
title: Agent Architecture Context
source: .
---

## 项目概览

MNS 是一个面向个人投资者的**逆向投资决策助手**（Rust CLI）。它不做交易、不预测市场，只做一件事：拉取 CNN 恐贪指数等情绪信号，对照"目标仓位 + 偏离带"框架，计算每日买入/卖出建议并生成报告，帮助用户"在恐慌时贪婪、在贪婪时恐慌"。决策闭环由 `mns report` 一条命令触发；历史表现由编译期嵌入的 2016–2025 月度全收益数据回测审计，诚实披露"收益低于买入持有，价值仅在风险调整后成立"。完全本地、单用户、单文件 SQLite（`~/.mns/mns.db`）+ TOML 配置（`~/.mns/config.toml`），17 个 CLI 命令，无 GUI/服务端/多用户。

## 架构设计

分层单片式 CLI：`main.rs` 作为唯一枢纽分发 17 个命令，模块间通过 `crate::` 源码级耦合（共享同一二进制与数据文件），无进程隔离、无全局可变状态。依赖方向：**通用域/支撑域在下，核心域在上**。

| 层 | 容器/模块 | 说明 |
|----|-----------|------|
| 入口 | `main.rs` + `cli.rs` | clap 命令定义 + 全部命令处理器（唯一枢纽，`#[tokio::main]`） |
| 核心域 | `strategy.rs` / `backtest.rs` / `report.rs` / `sentiment.rs` + `quote.rs` + `market.rs` | 调仓决策、历史模拟、报告渲染、情绪与行情获取 |
| 支撑域 | `db.rs` / `metrics.rs` / `config.rs` | SQLite 持久化、绩效指标、配置系统 |
| 通用域 | `models.rs` | 纯数据模型（Position/Transaction/FearGreedSnapshot） |

关键设计决策：① 回测与实盘**共用策略决策代码**（单一口径，避免"回测好看实盘拉胯"）；② 以**目标权重 + 偏离带**取代旧"现金比例"框架（消除路径依赖与弹药几何衰减）；③ 幂等建表 + `#[serde(default)]` 兼容，不引入 ORM/迁移；④ 回测数据 `include_str!` 编译期嵌入（可复现）；⑤ XIRR 作为年化主口径（现金流加权）；⑥ 多源冗余 + 重试降级（CNN 3 次重试、东财→天天 JSONP 兜底）。

## 模块地图

| 模块 | 职责 | 主要路径 |
|------|------|----------|
| 策略核心 | 目标仓位调仓计划（RebalancePlan/LegPlan）、风险警告、逆向加权分摊 | `src/strategy.rs` |
| 回测引擎 | 4 种引擎（目标仓位/旧框架/买入持有±再平衡）、2 种信号锚点（情绪/趋势）、FIFO 成本模拟 | `src/backtest.rs` |
| 报告生成 | 每日策略报告渲染（comfy-table，中文宽度对齐）与落盘 | `src/report.rs` |
| 情绪与行情 | CNN 恐贪指数、资产价格（东财/天天/Yahoo）、全球指数 | `src/sentiment.rs`, `src/quote.rs`, `src/market.rs` |
| 数据持久化 | 现金/持仓/交易/价格历史/情绪快照的 SQLite 读写与事务 | `src/db.rs` |
| 绩效指标 | XIRR、最大回撤、Sharpe/Sortino/Calmar、block bootstrap | `src/metrics.rs` |
| 配置系统 | 策略参数、目标权重曲线、成本模型、`validate()` 校验 | `src/config.rs` |
| CLI 分发 | clap 命令树 + 17 个命令处理器 | `src/cli.rs`, `src/main.rs` |
| 数据模型 | Position/Transaction/FearGreedSnapshot 纯数据 | `src/models.rs` |

## 核心流程

**① 每日报告 `mns report`（核心价值流）**
1. 拉取 CNN 恐贪指数（内置 3 次重试 + 418 反爬处理）
2. 保存情绪快照入库（同一天保留最新）
3. `check_risk_warnings` 找出浮亏超 20% 持仓，按情绪环境给差异化建议
4. `calculate_rebalance_plan` 计算目标 vs 当前权重，偏离超带宽才动作，先卖后买、现金受限时按比例缩减
5. `generate_report` 渲染 8 信息块报告，终端打印 + 存 `reports/{date}.txt`

**② 交易记录 `mns buy/sell`**
校验（买入查现金、卖出查超持）→ 单 SQLite 事务内三写（持仓 + 现金 + 交易流水）→ 提交。保证账目原子一致。

**③ 策略回测 `mns backtest` + `validate`**
加载编译期嵌入数据 → 3 月平滑 FGI → 按锚点算目标权重序列 → 逐月模拟（现金计息/年度注资/先卖后买）→ XIRR 与风险指标统计 → 默认跑 5 变体对比。`validate`：walk-forward 切分 → 网格调参（Calmar 最优）→ 样本外对比（检出过拟合）→ 2000 次分块 bootstrap 给 P5/P50/P95。

**④ 批量价格更新 `mns update-prices`**
列出持仓 → 按代码特征路由数据源（6 位数字→天天基金/东财，字母→Yahoo）逐个串行请求 → 单资产失败仅警告不中断 → 回写现价并累积价格历史。

## 技术选型

- **语言/运行时**：Rust edition 2024 + Tokio（异步仅用于网络，串行 await 防打爆反爬）
- **CLI**：clap 4（derive 声明命令树）
- **HTTP**：reqwest 0.12 + rustls-tls（免 OpenSSL 跨平台依赖）
- **存储**：rusqlite 0.39（`bundled`）+ TOML 配置（serde + serde_json + toml）
- **日期**：chrono；**终端渲染**：comfy-table + unicode-width（中文宽度对齐）
- **错误处理**：anyhow + `with_context` 中文报错
- **分发**：三平台二进制 npm 包（`distribution/bin-*`）+ Agent 技能 `distribution/skill/money-never-sleep/SKILL.md`

## 系统边界

**外部 API（不可信数据源，需容错解析）**
- CNN 恐贪指数 graphdata API（`src/sentiment.rs`，配置可换）
- Yahoo Finance v8 chart（美股/ETF/指数，`src/quote.rs`, `src/market.rs`）
- 东方财富移动端 API + 天天基金 JSONP（国内基金净值，降级链）

**本地文件（信任域：单用户自持）**
- `~/.mns/config.toml` — 用户可编辑，载入时 `validate()` 校验（分配和=100%、阈值单调、目标权重单调不增）
- `~/.mns/mns.db` — SQLite：`cash`/`positions`/`transactions`/`price_history`/`fear_greed_snapshots` 五表
- `./reports/{date}.txt` — 报告存档

**明确不做什么**：不连接券商/不自动下单；不做涨跌预测；无多用户/多账户；无 GUI/Web 服务。

## 代码映射索引

| 概念 | 位置 | 备注 |
|------|------|------|
| CLI 命令定义 | `src/cli.rs` | 17 命令 + cash/backtest 子命令树 |
| 命令分发枢纽 | `src/main.rs` | `cmd_*` 处理器全部在此 |
| 配置系统/默认值/校验 | `src/config.rs` | `target_weight_for` 是情绪→目标仓位映射 |
| 数据模型 | `src/models.rs` | Position/Transaction/FearGreedSnapshot |
| SQLite 持久化 | `src/db.rs` | 幂等建表 + 事务化买卖 |
| 调仓决策/风险警告 | `src/strategy.rs` | `calculate_rebalance_plan` 为核心入口 |
| 恐贪指数获取 | `src/sentiment.rs` | 3 次重试 + 418 处理 |
| 价格获取 | `src/quote.rs` | 东财/天天/Yahoo 路由与降级 |
| 市场指数 | `src/market.rs` | 全球指数 + 个股报价 |
| 报告渲染 | `src/report.rs` | 8 信息块 + 落盘 |
| 回测引擎 | `src/backtest.rs` | 数据内嵌 + FIFO 成本 + 信号锚点 |
| 绩效指标 | `src/metrics.rs` | XIRR/回撤/Sharpe/Sortino/Calmar/bootstrap |
| 分发/打包 | `distribution/` | 三平台 npm 包 |
| Agent 技能 | `distribution/skill/money-never-sleep/SKILL.md` | 命令与调参参考 |