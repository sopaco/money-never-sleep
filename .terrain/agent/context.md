---
type: agent_context
project: money-never-sleep
title: Agent Architecture Context
source: .
---

## 项目概览

MNS 是一套运行在本机的**个人投资决策 CLI 工具**（Rust 单二进制 `mns`），服务中长线逆向投资者。核心命题：把 CNN 恐贪指数（0–100）映射成风险资产**目标仓位**（越恐慌越高，35%–85%），再按偏离带（默认 4pp）给出买/卖/持有建议。**刻意不做自动交易**——`mns buy/sell` 只是把用户已在券商完成的交易记入本地账本。设计强调"诚实"：回测承认买入持有收益更高（年化 14.6% vs 12.8%），优势仅在风险调整后（Calmar 1.17 vs 0.98）。关键约束：无券商连接、离线可回测、报告先留档再决策（`reports/YYYY-MM-DD.txt`）。

## 架构设计

分层单二进制 CLI，无服务端、无后台进程，命令串行执行。

| 层 | 容器 | 职责 | 关键文件 |
|---|---|---|---|
| 交互层 | clap CLI | 命令定义与参数校验 | `src/cli.rs` |
| 入口层 | 命令分发 | 各 `cmd_*` 组装数据流 | `src/main.rs` |
| 决策层 | 策略引擎 | 情绪→目标权重→调仓计划 | `src/strategy.rs` |
| 决策层 | 报告生成 | 章节化文本 + 落盘 | `src/report.rs` |
| 数据层 | SQLite 账本 | 现金/持仓/交易/价格/情绪快照 | `src/db.rs`、`src/models.rs` |
| 配置层 | TOML 配置 | 参数+单调性校验+默认曲线 | `src/config.rs` |
| 数据源层 | 情绪抓取 | CNN 恐贪指数 API | `src/sentiment.rs` |
| 数据源层 | 行情抓取 | 天天基金/东财/Yahoo 多源 | `src/quote.rs`、`src/market.rs` |
| 验证层 | 回测引擎 | 4 引擎对比 + 样本外验证 | `src/backtest.rs`、`src/metrics.rs` |

主要依赖（`Cargo.toml`）：rusqlite、reqwest、clap、chrono、serde/toml、anyhow、comfy-table。单二进制，无运行时依赖；`distribution/` 下按平台打包为 npm 包。

## 模块地图

| Module | 责任 | 主要路径 |
|---|---|---|
| 入口/分发 | 约 20 个 `cmd_*`，串行装配数据流 | `src/main.rs` |
| CLI 定义 | 命令树 + clap 参数 | `src/cli.rs` |
| 配置 | `AppConfig`（Settings/Allocation/Thresholds/BuyRatio/SellRatio/Api/TargetWeight/Rebalance/Costs），TOML 读写，`validate()` 拒绝违反单调性的参数；路径 `~/.mns/config.toml`、`~/.mns/mns.db` | `src/config.rs` |
| 账本 | SQLite：`cash`、`positions`、`transactions`、`price_history`、`fear_greed_snapshots` | `src/db.rs` |
| 领域模型 | `Position`（收益率计算）、`Transaction`、`FearGreedSnapshot` | `src/models.rs` |
| 策略引擎 | `calculate_rebalance_plan` / `calculate_buy_suggestions` / `calculate_sell_suggestions` / `check_risk_warnings`；输出 `RebalancePlan`（legs、net_direction） | `src/strategy.rs` |
| 报告 | `generate_report`（六大章节）+ `save_report` 落盘 | `src/report.rs` |
| 情绪数据 | CNN API 抓取、解析、历史值抽取 | `src/sentiment.rs` |
| 行情数据 | 东财移动端/天天基金 JSONP/Yahoo 多源回退；`update_all_prices` | `src/quote.rs` |
| 市场行情 | 全球指数、个股报价（Yahoo） | `src/market.rs` |
| 风险指标 | XIRR、max_drawdown、Sharpe/Sortino/Calmar、block bootstrap、自研 RNG | `src/metrics.rs` |
| 回测 | 嵌入 2016–2025 数据集，`Engine` 4 引擎，`SignalConfig`（SentimentOnly/TrendTilt 锚），FIFO 批次，赎回费阶梯，holdout 验证 | `src/backtest.rs` |
| 发布/技能 | 平台 npm 包 + Agent 操作手册 SKILL | `distribution/` |

## 核心流程

**① 每日报告（`mns update-prices` → `mns report`）**
1. 抓取 CNN 恐贪指数并解析出 score/rating/前日周月年值
2. 指数快照写入 `fear_greed_snapshots` 落库
3. 读入现金 + 持仓现价，调用策略引擎
4. 情绪 → 目标风险权重（按 `target_weight` 曲线，单调不增）
5. 按三类腿（美股/A股/逆周期）拆目标权重，逐腿比较实际 vs 目标、算漂移
6. 偏离超出带宽（默认 4pp）才触发动作；**先算卖出、卖出回笼资金计入买入预算**
7. 生成六章节报告并存档 `reports/YYYY-MM-DD.txt`

**② 交易记账（`mns add` → `mns buy/sell` → `mns portfolio`）**
1. 标的先入池（类别限 `us_stocks`/`cn_stocks`/`counter_cyclical`）
2. `buy`/`sell` 仅登记已成交交易，更新现金与成本（现金不足/超持即报错）
3. `portfolio`/`history` 只读复核；卖出受最短持有天数约束（规避惩罚性赎回费）

**③ 回测与验证（`mns backtest` [run|validate|params]）**
1. 加载内置真实全收益月度数据集（2016–2025，人民币计价含分红）
2. 按月推进 `State`：目标仓位/旧框架/买入持有/买入持有+年度再平衡 四引擎并行
3. FIFO 批次卖出 + 阶梯赎回费 + 现金利息 + 交易成本逐笔扣减
4. 汇总 XIRR/回撤/风险指标/交易频率并输出对比
5. validate：block bootstrap 收益分布 + holdout 样本外验证

## 技术选型

- **语言/版本**：Rust edition 2024，`cargo build --release` 出单静态二进制
- **CLI**：clap（子命令树，位置参数而非 `--limit` 等约定）
- **存储**：SQLite（rusqlite），单文件 `~/.mns/mns.db`，单写者串行
- **HTTP**：reqwest；带重试（CNN 可能反爬 418）
- **时间**：chrono，NaiveDate 处理持仓天数/回测月份
- **输出**：comfy-table 表格 + 中文章节标记文本
- **配置**：serde + toml，`~/.mns/config.toml`；旧配置缺字段自动取默认值
- **错误**：anyhow，统一 `Error: <中文原因>` + 非零退出码
- **发布**：平台化 npm 包（darwin-arm64/linux-x64/win-x64），技能打包进 `distribution/skill/`
- **测试**：内嵌 `#[cfg(test)]` 单测（策略单调性、FIFO、成本扣减、带宽交易频率等）

## 系统边界

| 边界 | 对象 | 说明 / 信任边界 |
|---|---|---|
| 外部 API | CNN Fear & Greed `production.dataviz.cnn.io/index/fearandgreed/graphdata` | 需网络，可 418 反爬；数据只读，落库前不过滤 |
| 外部 API | 天天基金 / 东财移动端行情 | 国内基金价格，多源回退；无对应数据时跳过单标的 |
| 外部 API | Yahoo Finance | `market`/`analyze` 依赖；部分网络 403 拦截，视为环境限制 |
| 本地存储 | SQLite 账本 + TOML 配置 | 完全可信、单写者；多进程并发会锁库 |
| 人机边界 | 用户已成交的交易 | `buy/sell` 是记账不是下单；误记无撤销命令，会污染全部下游数字 |
| 无边界 | 券商/银行 | **零连接**，从不发起真实交易 |

## 代码映射索引

| Concept | Location | Notes |
|---|---|---|
| 命令树/参数 | `src/cli.rs` | 全命令清单 |
| 配置结构与校验 | `src/config.rs` | 默认目标权重曲线 85/75/60/45/35；`validate()` 校验单调性 |
| 数据库表与 CRUD | `src/db.rs` | 5 张表，`init_tables` |
| 领域类型 | `src/models.rs` | Position 收益率计算 |
| 再平衡计划 | `src/strategy.rs` | RebalancePlan/建议/风险警告 |
| 报告生成 | `src/report.rs` | 六章节 + 落盘 |
| 恐贪指数抓取 | `src/sentiment.rs` | CNN 解析 |
| 价格多源回退 | `src/quote.rs` | 东财/天天/Yahoo |
| 市场行情 | `src/market.rs` | 指数 + 个股 |
| 风险指标 | `src/metrics.rs` | XIRR/回撤/Sharpe/Calmar/bootstrap |
| 回测引擎 | `src/backtest.rs` | 4 Engine、SignalConfig、holdout |
| 入口分发 | `src/main.rs` | cmd_* 装配 |
| 内嵌数据集 | `src/backtest.rs` | 2016–2025 真实全收益 |
| Agent 操作手册 | `distribution/skill/money-never-sleep/SKILL.md` | 硬约束/工作流/故障表 |
| 知识资产 | `.terrain/human/`、`.ai-context/`、`AGENTS.md` | 人类文档与 Agent 指南 |
```