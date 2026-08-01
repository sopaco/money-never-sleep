---
type: agent_context
project: money-never-sleep
title: Agent Architecture Context
source: .
---

## 架构设计

单二进制、单进程、无服务端。命令进入 → 处理函数直接调用各模块（无独立服务层、无 trait 抽象，函数式组织）。

| 层 | 容器/职责 | 关键依赖 |
|---|---|---|
| CLI 入口层 | `cli.rs` 定义 clap 命令树；`main.rs` 分发 17 个命令到处理函数 | clap 4 (derive) |
| 决策引擎层 | `strategy.rs` 计算买卖/风控/再平衡；`config.rs` 情绪区间→目标权重映射 | 纯函数 |
| 数据层 | `db.rs` SQLite 账本（现金/持仓/交易/价格历史/恐贪快照）；`models.rs` 领域模型 | rusqlite (bundled) |
| 数据采集层 | `sentiment.rs` 拉取 CNN 恐贪指数；`quote.rs` 拉价格（国内基金/美股分源）；`market.rs` 指数与个股报价 | reqwest (rustls), serde_json |
| 分析层 | `backtest.rs` 四引擎回测（内嵌 2016-2025 真实全收益月度数据）；`metrics.rs` XIRR/回撤/风险指标/自助抽样 | 离线 |
| 报告层 | `report.rs` 渲染文本报告并存盘 | comfy-table |

数据流方向：**采集层 → 决策引擎层（依赖 config + db）→ 报告层 → 落盘**；回测引擎**独立于账本**，复用 `config.rs` 的成本模型与策略规则。UI 全部为终端文本表。

## 模块地图

| 模块 | 职责 | 主要路径 |
|---|---|---|
| cli | clap 命令树：init/config/cash/portfolio/add/buy/sell/price/remove/sentiment/report/history/backtest/update-prices/market/market-indices/analyze | src/cli.rs |
| main | 命令处理函数、各命令编排（init/cash/portfolio/buy/sell/report/backtest 等） | src/main.rs |
| config | AppConfig 全量配置、TOML 载入/保存、校验（单调性/配置和=100%）、dot-path get/set、情绪区间/目标权重/三腿拆分/费用曲线 | src/config.rs |
| db | SQLite 账本：现金、持仓（加权平均成本、事务化买卖）、交易、price_history、fear_greed_snapshots | src/db.rs |
| models | Position（市值/年化/绝对收益）、Transaction、FearGreedSnapshot | src/models.rs |
| sentiment | CNN 恐贪指数拉取（重试/反爬 418）、JSON 解析、历史对照值、快照入库 | src/sentiment.rs |
| quote | 价格采集：国内基金（东方财富移动/天天基金）、美股（Yahoo）、按类别路由、update_all_prices | src/quote.rs |
| market | 全球指数/个股报价（Yahoo），供 market/market-indices/analyze | src/market.rs |
| strategy | 买入/卖出建议（逆向权重上限、卖出回款计入买入预算）、风险预警、目标仓位+偏离带再平衡计划、宽基识别 | src/strategy.rs |
| report | 渲染含【市场情绪/账户概览/调仓计划/净操作指引/目标仓位预案/信号口径】的报告并写入 reports/ | src/report.rs |
| backtest | 内嵌数据集解析、四引擎（目标仓位/旧框架/买入持有/买入持有+再平衡）、FIFO 批次卖出、阶梯赎回费、按月流入、回测 validate（block bootstrap+holdout） | src/backtest.rs |
| metrics | XIRR、最大回撤、Sharpe/Sortino/Calmar、下行偏差、block bootstrap + 分位数 | src/metrics.rs |
| distribution | Agent 操作手册 SKILL、各平台 npm 预编译包（darwin/linux/win x64） | distribution/ |

## 核心流程

**① 每日报告 `mns report`（决策闭环）**
1. 拉取 CNN 恐贪指数（失败内置重试），评分快照写入 `fear_greed_snapshots`
2. 情绪评分 → `config.sentiment_zone`/`target_weight_for` 得风险资产目标总权重，按 `sleeve_split` 拆三腿（美股/A股/逆周期）
3. 先算卖出建议（年化收益达标/绝对收益≥30% 双止盈，受最短持有天数与阶梯赎回费约束），卖出回款并入买入预算
4. 再算买入建议（按权重上限逆向加仓，深度浮亏个股排除、宽基指数例外）、风险预警
5. 生成 `RebalancePlan`：每腿实际 vs 目标权重，偏离超 `band_pp`（默认 4pp）才动作，现金不足按比例缩减
6. `generate_report` 渲染六章节并保存 `reports/YYYY-MM-DD.txt`；**"不动作"是正常健康输出**

**② 记账 `mns buy/sell`（用户已成交后登记）**
1. 校验：标的已 add、份额/价格为正、现金足够（买）或份额不超持有（卖）
2. 单一 SQLite 事务内：更新持仓（买入加权平均成本价；卖出减份额）→ 更新现金 → 插入 transactions
3. `mns buy/sell` 严禁依据 report 建议调用——会静默污染现金、成本价、持有天数与后续所有建议

**③ 价格刷新 `mns update-prices`**
1. 遍历全部持仓，按类别路由：国内基金 → 东方财富/天天基金净值，美股 → Yahoo
2. 更新 `current_price` 并写入 `price_history`（按 asset_code+date 去重 upsert）；单标的失败跳过不中断

**④ 回测 `mns backtest`（离线，不动账本）**
1. 解析内嵌月度数据集（2016-2025 真实全收益，三腿价格+FGI）为主集+holdout
2. 依次跑四引擎，均复用同一成本模型（买/卖费、阶梯赎回费、现金年化收益）与 `SignalConfig`（情绪锚/趋势锚）
3. `finalize` 出 XIRR/最大回撤/Calmar/交易频率，`print_comparison` 横向对比
4. `backtest validate` 用 block bootstrap 给收益分布 + holdout 样本外验证
5. 转述结论必须带限定：收益不敌买入持有、优势在风险调整、FGI 边际贡献近零、长期熊市未覆盖

## 技术选型

- **语言/工具链**：Rust edition 2024；`cargo build --release` 产单静态二进制（无运行时依赖）
- **CLI**：clap 4（derive）17 子命令
- **异步/网络**：tokio 1 + reqwest 0.12（default-features off，rustls-tls，无 openssl）
- **数据**：rusqlite 0.39 bundled（零编译期系统依赖）、toml 1.1、chrono 0.4（serde）
- **序列化**：serde / serde_json
- **输出**：comfy-table 7、unicode-width（中文对齐）
- **错误**：anyhow + 非零退出码 + `Error: 中文原因`；配置校验在写入前拒绝
- **分发**：cargo 交叉编译 + npm 包装（`@never-sleeps/mns-cli`），darwin-arm64/linux-x64/win-x64 预编译包
- **测试**：模块内 `#[cfg(test)]` 单元测试（配置校验、FIFO、成本模型、指标、数据集完整性），无集成测试框架
- **回测**：内嵌数据集（离线可用），block bootstrap 用自实现 xorshift RNG

## 系统边界

- **外部 API（拉取，只读）**：CNN 恐贪指数 `production.dataviz.cnn.io/fearandgreed/graphdata`（可能 418 反爬，有重试）；Yahoo Finance（美股报价/全球指数/analyze，部分网络 403 被拦截，属环境限制）；东方财富/天天基金（国内基金净值）。**无任何写向外部**的调用
- **本地持久化**：`~/.mns/config.toml`、`~/.mns/mns.db`（SQLite 单文件，多进程并发写会锁库，命令须串行）、`./reports/`
- **信任边界**：不连券商、不下单、无多用户；外部价格/指数视为账本记价依据；恐贪指数是策略唯一情绪输入
- **故障模式**：网络失败不得用旧指数伪装当日数据；个别标的抓价失败可手工 `mns price`；金额/份额错误靠 `portfolio`/`history` 核对（无撤销命令）

## 代码映射索引

| 概念 | 位置 | 备注 |
|---|---|---|
| 命令树定义 | src/cli.rs | clap derive；backtest 子命令 Run/Validate/Params |
| 命令分发/编排 | src/main.rs | cmd_* 处理函数 |
| 配置模型/默认曲线/校验/dot-path | src/config.rs | `target_weight_for`/`asset_target_weights`/`validate`/`get_value`/`set_value` |
| 账本存取 | src/db.rs | `Database`；买卖为事务化；`init_tables` 建 5 张表 |
| 领域模型 | src/models.rs | Position/Transaction/FearGreedSnapshot |
| 恐贪指数采集 | src/sentiment.rs | `fetch_fear_greed_data`/`parse_cnn_response` |
| 价格采集（多源路由） | src/quote.rs | eastmoney/tiantian/yahoo；`fetch_price(code, category)` |
| 指数/个股报价 | src/market.rs | 依赖 Yahoo，环境受限 |
| 建议与再平衡引擎 | src/strategy.rs | buy/sell/risk/rebalance 计算 + 测试用例 |
| 报告渲染/落盘 | src/report.rs | `generate_report`/`save_report` |
| 回测引擎与数据集 | src/backtest.rs | `Engine`/`SignalConfig`/`State`/FIFO 批次卖出/validate |
| 风险指标/自助抽样 | src/metrics.rs | xirr/max_drawdown/risk_metrics/block_bootstrap |
| Agent 操作手册 | distribution/skill/money-never-sleep/SKILL.md | 硬约束、命令速查、故障模式 |
| npm 分发 | distribution/bin-{darwin-arm64,linux-x64,win-x64}/ | 平台预编译包 |
| 依赖清单 | Cargo.toml | 版本 0.6.0，edition 2024 |