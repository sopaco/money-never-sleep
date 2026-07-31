# 预处理报告

## 项目基本信息
- **项目名称**：MNS - Money Never Sleeps（逆情绪投资助手 / Market Neutral Strategist）
- **版本**：0.6.0（Cargo.toml:3）
- **项目类型**：个人 CLI 投资决策工具（命令行走查系统，非自动交易）
- **主要编程语言**：Rust（edition 2024）
- **核心框架/运行时**：Tokio（异步运行时）、clap（CLI 解析）、rusqlite（SQLite）

## 技术栈
- **运行时**：Rust (edition 2024)，`cargo build --release` 生成 `target/release/mns`
- **Web框架**：无（纯 CLI）
- **数据库**：SQLite（rusqlite bundled，`~/.mns/mns.db`）
- **LLM/AI集成**：无（AI 辅助开发是外部工作流，工具本身不调用 LLM）
- **主要依赖库**：
  - clap 4（derive 宏定义 CLI）
  - tokio 1（异步运行时，网络请求）
  - reqwest 0.12（HTTP 客户端，rustls-tls）
  - serde / serde_json（配置与 JSON 解析）
  - toml 1.1（配置解析）
  - rusqlite 0.39（SQLite，bundled）
  - chrono 0.4（日期时间）
  - comfy-table 7（终端表格渲染）
  - dirs 6（用户目录定位）
  - anyhow（错误处理）
  - unicode-width 0.2（中英文宽度对齐）

## 目录结构摘要
```
money-never-sleep/
├── src/                          # 全部业务源码（11 个平铺 .rs 文件）
│   ├── main.rs                   # 入口 + 命令分发 + 全部命令处理器
│   ├── cli.rs                    # clap CLI 定义（Commands/CashAction/BacktestAction）
│   ├── config.rs                 # AppConfig 配置结构与校验
│   ├── db.rs                     # SQLite 持久化层（Database）
│   ├── models.rs                 # 领域数据结构（Position/Transaction/FearGreedSnapshot）
│   ├── strategy.rs               # 逆向策略核心（调仓计划/风险警告）
│   ├── report.rs                 # 每日策略报告生成
│   ├── sentiment.rs              # CNN 恐贪指数获取
│   ├── quote.rs                  # 资产价格获取（东方财富/天天基金/Yahoo）
│   ├── market.rs                 # 市场指数概览
│   ├── metrics.rs                # 绩效与风险指标（XIRR/回撤/Sharpe/Calmar）
│   └── backtest.rs               # 回测引擎（编译期嵌入 CSV 数据）
├── .ai-context/                  # AI 开发上下文（SKILL 体系）
├── .agents/skills/mns-backtest/  # 回测技能 + 数据 CSV
├── distribution/                 # 多平台发布打包
├── reports/                      # 每日报告输出目录（默认 ./reports）
├── litho.docs/                   # 已有的人类文档（早期版本）
├── Cargo.toml / Cargo.lock
└── README.md / README_zh.md
```

## 识别到的核心模块（候选领域模块）
按 `src/` 下 11 个源文件平铺结构 + DDD 语义分组：

| 领域模块 | 源文件 | 一句话职责 |
|---------|--------|-----------|
| 策略核心 | `src/strategy.rs` | 逆向策略决策：目标仓位、调仓计划、风险警告 |
| 回测引擎 | `src/backtest.rs` | 用嵌入真实数据模拟策略历史表现 |
| 绩效指标 | `src/metrics.rs` | XIRR/最大回撤/Sharpe/Sortino/Calmar/bootstrap |
| 报告生成 | `src/report.rs` | 把策略输出渲染成终端友好+落盘的每日报告 |
| 情绪与行情 | `src/sentiment.rs`+`src/quote.rs`+`src/market.rs` | CNN 恐贪指数 + 资产/指数价格获取 |
| 数据持久化 | `src/db.rs` | SQLite 存取现金/持仓/交易/价格历史/情绪快照 |
| 配置系统 | `src/config.rs` | 策略参数、目标仓位曲线、成本模型的声明与校验 |
| CLI与命令分发 | `src/cli.rs`+`src/main.rs` | clap 定义 + 17 个命令的处理器 |
| 数据模型 | `src/models.rs` | Position/Transaction/FearGreedSnapshot 结构 |

## 关键文件清单
- 入口文件：`src/main.rs`（`#[tokio::main] async fn main`，main.rs:20）
- 核心抽象：
  - `AppConfig`（config.rs:7）— 全局配置
  - `Database`（db.rs:7）— 数据持久化门面
  - `RebalancePlan` / `LegPlan`（strategy.rs:360/334）— 调仓计划核心类型
  - `BacktestConfig` / `SignalConfig` / `Engine`（backtest.rs:200/268/232）— 回测配置抽象
  - `RiskMetrics` / `CashFlow`（metrics.rs:132/10）— 指标类型
- 关键函数：
  - `calculate_rebalance_plan`（strategy.rs:406）— 每日建议入口
  - `run`（backtest.rs:557）— 回测引擎主循环
  - `generate_report`（report.rs:23）— 报告渲染
  - `fetch_fear_greed_data`（sentiment.rs:57）— 情绪输入
  - `update_all_prices`（quote.rs:197）— 价格批量更新

## 依赖关系摘要
- `main.rs` 依赖所有模块（命令分发枢纽）
- `backtest.rs` 依赖 `config/metrics/models/strategy`
- `report.rs` 依赖 `config/models/strategy`
- `strategy.rs` 依赖 `config/models`
- `db.rs` 依赖 `config/models`
- `market.rs` 依赖 `quote`
- `main.rs` 依赖 `config/db/strategy/report/sentiment/quote/market/backtest/metrics/cli/models`
- 数据流方向：`sentiment/quote`（外部输入）→ `db`（持久化）→ `strategy`（决策）→ `report`（输出）；`backtest` 旁路复用 `strategy` + `metrics` 做历史验证

## README 核心内容
- 定位：AI 时代的逆向投资决策助手，克服人性弱点，"buy in fear, sell in greed"
- 核心卖点：恐贪指数实时感知、参数经过 2016-2025 真实全收益数据回测、买卖互感知、双重止盈、逆向加仓
- **诚实披露**：买入持有年化 14.60% 优于策略 12.75%（XIRR），但策略 Calmar 1.17 vs 0.98（回撤低 ~4pp）；FGI 在趋势锚存在时边际贡献趋近于零；2000-2002/2008 熊市未被覆盖
- 设计理念：MNS 不预测市场、不执行交易，只提供决策支持

## 注意事项
- 无 ORM、无迁移文件；schema 由 `db.rs:init_tables` 内联 `CREATE TABLE IF NOT EXISTS` 管理
- 回测数据通过 `include_str!` 编译进二进制（backtest.rs:24-29），更新数据需重新编译
- 项目自带 `litho.docs/`（旧版人类文档）与 `.ai-context/`（AI 上下文），本次生成的 `.terrain/human/` 是 Terrain 流水线的新产出
- 测试全部为 Rust 单元测试（`cargo test`），用中文函数名；已确认 `cargo build --release` 通过
