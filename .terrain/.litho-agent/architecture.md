## 架构研究报告

### 架构模式

MNS 是一个**分层单片式 CLI 应用**，可以把它想象成一家小型"决策咨询公司"：前台（CLI）接待你的指令，数据科（情绪与行情）负责出去收集市场情报，档案室（数据持久化）把所有信息归档，分析师（策略核心）基于情报与档案给出建议，秘书（报告生成）把建议写成工整的报告，而审计部门（回测引擎）定期用历史数据检验分析师的建议靠不靠谱。

这不是偶然的扁平结构，而是一种刻意为之的克制。作为一个个人工具，它不需要微服务、不需要消息队列、不需要复杂的依赖注入——**极简的模块边界 + 单一入口分发**已经足够。证据是 `main.rs` 作为唯一枢纽直接调用所有模块（main.rs:24-64），模块之间通过 `crate::` 引用保持轻量耦合，没有循环依赖、没有全局共享可变状态。

### 核心设计原则

**1. 单一口径（Single Source of Truth）——回测与实盘共用决策代码**

这是整个系统最关键的架构决策。回测引擎没有复制一份策略逻辑，而是直接复用 `strategy.rs` 的决策函数：`Engine::Legacy` 调用 `calculate_buy_suggestions`/`calculate_sell_suggestions`（backtest.rs:18, step_legacy backtest.rs:753-797），实盘报告调用 `calculate_rebalance_plan`（main.rs:379）。回测的 `rebalance_to_weights`（backtest.rs:687）与实盘的 `calculate_rebalance_plan`（strategy.rs:406）都共享 `config.target_weight_for`（config.rs:350）这条"情绪→目标仓位"映射。这保证了**回测结果与实盘建议严格一致**——不会出现"回测用的是一套逻辑、实盘用的另一套"这种系统性失真。

**2. 目标权重思维取代现金比例思维**

旧框架的买入逻辑是"花掉手上现金的百分之几"，这有两个致命缺陷：连续恐慌期弹药几何衰减（越买越没钱），以及路径依赖（刚注资则买入金额被放大）。新框架（config.rs:25-29 注释中明确记录了这个设计决策）始终对照"我应该持有多少风险资产"，只在实际权重偏离目标超过带宽时才动作。这是把逆向策略从"直觉"变成"可回测、可验证、无路径依赖的规则"的关键一步。

**3. 诚实是最高的数据质量要求**

系统在三个层面贯彻诚实：财务口径上，年化用 XIRR（现金流加权，metrics.rs:1-4 注释说明为什么简单年化失真）而非 `(期末/期初)^(1/n)`；成本模型上，计入买入费、按持有天数的 FIFO 阶梯赎回费、闲置现金货币基金收益（backtest.rs:5-13 设计要点）；对外披露上，README 明确写着"买入持有收益更高（14.60% vs 12.75%），本工具价值仅在风险调整后成立"。回测引擎还内置防数据造假校验：逐月跳变超 35% 即报"疑似数据断点"、整数行过多即报"疑似人工估填"（backtest.rs:1062-1087）。

**4. 本地优先、零远程状态**

所有状态都落在用户自己的机器上：配置在 `~/.mns/config.toml`，数据在 `~/.mns/mns.db`，回测数据编译期嵌入二进制。用户数据不出本机，工具离线可读，唯一的外部依赖是按需拉取的市场数据。

### 技术栈详情

技术选型围绕"个人金融工具"这一场景展开——需要可靠、轻量、跨平台、可长期维护，而不是追求极限性能或炫技：

| 层次/领域 | 技术选型 | 选择理由 |
|---------|---------|---------|
| 语言与运行时 | Rust edition 2024 + Tokio | 单一静态二进制、内存安全、跨平台（darwin/linux/windows）；Tokio 处理网络请求的并发等待 |
| CLI | clap 4（derive） | 声明式定义命令/参数/帮助，与 Rust 类型系统天然结合 |
| HTTP | reqwest 0.12 + rustls-tls | 成熟异步客户端；选 rustls 而非 OpenSSL，避免跨平台原生依赖编译问题 |
| 数据库 | rusqlite + bundled SQLite | 零配置、单文件、SQL 能力足够；`bundled` 特性免去系统级 SQLite 依赖 |
| 序列化 | serde + serde_json + toml | 配置用 TOML（人类可编辑），API 响应用 JSON（机器解析） |
| 日期时间 | chrono | Rust 生态标准日期库，回测需要精确的日期算术与排序 |
| 终端渲染 | comfy-table + unicode-width | 中文终端对齐需要按显示宽度而非字符数补齐（report.rs:12-19 专门处理） |
| 错误处理 | anyhow | 个人工具优先"快速失败 + 友好报错"，无需自定义错误类型 |

### 关键数据结构

理解这些类型就理解了 MNS 的决策闭环：

| 类型名 | 文件路径 | 用途 |
|-------|---------|------|
| `AppConfig` | src/config.rs:7 | 全部策略参数的容器，分 9 个子结构管理目标仓位/成本/阈值/配置 |
| `TargetWeight` | src/config.rs:31 | 情绪→风险资产目标权重的 5 档曲线（极度恐慌 85%→极度贪婪 35%） |
| `RebalancePlan` | src/strategy.rs:360 | 调仓计划的根类型：目标 vs 当前权重、每腿建议金额、现金约束标记 |
| `LegPlan` | src/strategy.rs:334 | 单条资产腿（美股/A股/黄金）的目标/当前权重、偏离、建议金额 |
| `Position` | src/models.rs:5 | 持仓快照：份额、成本价、现价、首次买入日 |
| `MonthRow` | src/backtest.rs:38 | 回测数据行：月末日期 + FGI + 三腿价格 |
| `BacktestResult` | src/backtest.rs:406 | 回测产出：XIRR、回撤、风险指标、交易列表、逐月序列 |
| `SignalConfig` | src/backtest.rs:268 | 信号配置：平滑窗口、锚点（情绪/趋势）、趋势参数、情绪倾斜量 |

### 核心接口/Trait/协议

MNS 没有使用 Rust trait 做多态抽象，而是用**函数签名 + 领域结构体**组织边界——对个人工具而言，引入 trait 抽象反而增加认知负担。关键的"接口"是跨模块调用点：

| 名称 | 实现数量 | 核心职责 |
|-----|---------|---------|
| `Database::open()` | 1（db.rs:12） | 统一入口打开/初始化 SQLite，所有命令共用 |
| `calculate_rebalance_plan()` | 1（strategy.rs:406） | 实盘调仓计划，被 report 与（概念上）回测共用 |
| `run()`（回测） | 1（backtest.rs:557） | 回测引擎主循环，参数化 engine/signal/config |
| `fetch_fear_greed_data()` | 1（sentiment.rs:57） | 恐贪指数获取（含 3 次重试） |
| `fetch_price()` | 1（quote.rs:178） | 按代码特征路由到数据源的统一价格入口 |
| `Engine` enum | 4 变体（backtest.rs:232） | 策略引擎类型化：TargetWeight/Legacy/BuyHold/BuyHoldRebalanced |
| `Anchor` enum | 2 变体（backtest.rs:259） | 信号锚点：SentimentOnly/TrendTilt |

### 架构决策记录（推断）

- **决策1**：回测引擎复用实盘策略函数，而不是复制一份回测专用逻辑。放弃了"回测独立实现"的清晰性，换来了**实盘与回测口径完全一致**——这是避免"回测好看、实盘拉胯"的根本保障。观察依据：backtest.rs:18 `use crate::strategy::{calculate_buy_suggestions, ...}`，step_legacy（backtest.rs:753）直接调用实盘买卖建议函数。
- **决策2**：用"目标权重 + 偏离带"替代"现金百分比"。放弃了旧框架的简单直觉，因为其在连续恐慌期存在弹药几何衰减与路径依赖问题（config.rs:25-29 注释明示）。换来了无路径依赖、有仓位上限、可再平衡的稳健框架。
- **决策3**：SQLite 单文件本地库 + 配置单文件 TOML，而不是引入完整 ORM/迁移框架。放弃了迁移工具链与类型安全 ORM，换来了零配置、开箱即用、schema 演进靠 `CREATE TABLE IF NOT EXISTS`（db.rs:24-73）的轻量（旧配置文件缺少新字段仍可加载，config.rs:636 有测试保障）。
- **决策4**：回测数据编译期 `include_str!` 嵌入（backtest.rs:24-29）。放弃了运行时读外部文件，换来二进制自包含、结果可复现；代价是更新数据需重新编译。
- **决策5**：XIRR 作为年化主口径（metrics.rs:18）。放弃了简单年化公式，因为在分批注资下它系统性低估真实收益（晚到的资金不应被当作全程投入）。
- **决策6**：Yahoo/Eastmoney/天天基金多源冗余 + 重试降级。放弃了单一数据源，因为免费金融 API 随时可能反爬或下线；sentiment.rs 对 CNN 418 反爬专门做了错误提示，quote.rs 对失败资产跳过不中断批量更新。
