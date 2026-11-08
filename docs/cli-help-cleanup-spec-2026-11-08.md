# CLI 帮助文本清理与去冗余 — 产品规格

> 日期：2026-11-08 · 提出人：product · 状态：待 coder 评估可行性
> 背景：用户反馈 —— backtest 中出现"旧策略/旧框架"，且各 -h 对默认策略配置、参数效果解释不足。
> 原则：**MNS 是新应用，无历史用户包袱**。所有兼容旧版的路径、"旧命令/旧框架"文案一律清除；一件事一个入口；-h 要写清默认值、单位与效果，但保持每行简短（-h 不是教程）。

---

## P0 清除旧框架（用户明确要求）

### P0-1 删除 `cash add` 子命令
- 现状：`mns cash add` 帮助文案自带 `[旧命令]` 标签，语义与 `cash deposit` 重复，且台账记 `adjust`（XIRR 口径差）。
- 动作：删除 `CashAction::Add`（src/cli.rs）、main.rs 中对应分支（约 301 行 C1 注释处）。
- 验收：`mns cash --help` 无 add；`mns cash add 100` 报错。

### P0-2 回测移除旧框架引擎
- 现状：`mns backtest` 默认对比包含 `旧框架(现金比例)`（`Engine::Legacy`）；`backtest params` 输出【旧框架参数】块。
- 动作：
  - 删除 `Engine::Legacy`、`step_legacy()`（src/backtest.rs）及其专属测试（如"旧框架在连续恐慌期会越买越少"，backtest.rs ~1286）。
  - 默认对比变为 4 条：目标仓位(趋势锚) / 目标仓位(情绪锚) / 买入持有+年度再平衡 / 买入持有。
  - strategy.rs 中 `calculate_buy_suggestions` / `calculate_sell_suggestions` 仅被 `step_legacy` 调用 → 一并删除。**注意 `check_risk_warnings` 被实盘路径（main.rs ~983）使用，必须保留。**
- 验收：`mns backtest` 输出无"旧框架"行；cargo test 全绿。

### P0-3 删除旧框架配置参数
- `buy_ratio.*` / `sell_ratio.*`：删除 config.rs 中 `BuyRatio`/`SellRatio` 结构、字段与 `buy_ratio_for`/`sell_ratio_for`。
- `backtest params` 输出删除【旧框架参数】块（main.rs ~1595-1598）。
- **特例 `settings.annualized_target_low/high`**：不再驱动任何策略行为（旧止盈语义），但仍被 portfolio/report 用于年化收益单元格着色（main.rs ~396、report.rs ~357），且 config 默认注释"年化15%大笔减仓"已失真误导。
  - 产品决策：**方案 a（推荐）**——保留参数，从【旧框架参数】移出，归入"显示"组，注释改为"仅用于报告/持仓中年化收益着色阈值，不驱动交易"；**方案 b**——彻底删除，着色改固定阈值。倾向 a，少 churn。

---

## P1 命令去重

### P1-1 `mns backtest` 与 `backtest run` 二合一
- 现状：bare `mns backtest` 与 `mns backtest run` 走同一函数 `cmd_backtest(None, None)`，同一功能两个入口。
- 动作：删除 `BacktestAction::Run` 子命令，`--config`/`--compare` 提升到 `mns backtest` 顶层。
- 新用法：`mns backtest` / `mns backtest --config my.toml` / `mns backtest --compare a.toml,b.toml` / `mns backtest validate`。bare 行为不变（多策略对比）。
- 验收：`mns backtest run` 报错；`mns backtest --config x.toml` 等价旧 run。

### P1-2 `backtest params` → `config params`（产品倾向迁移，请 coder 评估）
- 理由：params 列出的参数（thresholds / target_weight / rebalance / costs / allocation / cashflow.monthly / profile）**绝大多数驱动实盘行为**，不只是回测。挂在 backtest 下是语义错位；挂到 config 家族（看/改/列清单）更自然，且 `mns config --help` 会自然展示它。
- 迁移项：cli 结构调整、`cmd_backtest_params` 改挂 config、两处 SKILL.md 与 README 的引用同步更新。
- 若 coder 评估后认为迁移成本高，可降级为：保留 `backtest params` 但在 `mns config --help` 里加交叉引用（不推荐，保留两个认知入口本身就是本次要消除的问题）。

### P1-3 删除 `market-indices`
- 现状：`cmd_market_indices` 与 `cmd_market` 的指数部分为**同一实现、同一表格**（main.rs 1664-1742），纯子集重复。
- `market` 在 FGI 获取失败时已优雅降级，"只想看指数"没有硬需求。
- 保留 `sentiment`（有落快照副作用，是信号链路一部分），但其 -h 必须写明副作用（见 P2-6）。
- 验收：`mns market-indices` 报错；`mns market` 不受影响。

### P1-4 明确保留的"看似重复"项（不删，防止误伤）
- `today` = update-prices + report：主流程封装，帮助文案已写明。
- `price CODE`（省略参数=查看）：已文档化，保留。
- `sentiment` vs `market`：一个写库一个只读，语义不同。

---

## P2 帮助文本补全（每命令给出目标文案）

### P2-1 主命令 `mns -h` 追加 after_help（4 行速览）
```
策略速览: 恐贪指数 → 目标风险仓位（默认 35%~85%，五档）→ 偏离带再平衡（默认 ±4pp）
决策节奏: 默认每周五决策日（rebalance.decision_weekday 可改），其余天出观察版
快速上手: mns init → mns cash deposit 10000 → mns add QQQ 纳指100 us_stocks → mns today
调参清单: mns config params（全部可调参数、默认值与效果说明）
```
数字来自 config 默认值——文案请用 `format!` 从默认常量生成，避免漂移；如实现不便可写"默认"字样。

### P2-2 各子命令 -h 目标文案
| 命令 | 现文案问题 | 目标文案 |
|---|---|---|
| `report` | "决策日/决策版"无解释 | 生成今日策略报告（决策日→决策版含调仓指令；其余天→观察版仅提示不动手。决策日默认周五，`rebalance.decision_weekday` 可改） |
| `report --decide` | — | 非决策日仍强制生成决策版报告（含调仓指令） |
| `today` | 同上 | 一键：先更新价格，再生成今日报告（价格失败也继续）；--decide 同 report |
| `add CATEGORY` | 类别无解释 | 类别: us_stocks(美股宽基) / cn_stocks(A股宽基) / counter_cyclical(逆周期·黄金等)；三腿内部比例由 allocation.* 控制 |
| `plan` | 输出内容不明 | 按当前情绪与目标仓位给出本月各腿买入金额（只买不卖）；月投入省略时读 cashflow.monthly |
| `analyze` | **承诺"估值"但实现只有报价**（诚实性缺陷） | 个股报价查询（价格、涨跌幅）。删去"估值"二字；补实现不在本次范围 |
| `sentiment` | 隐藏副作用 | 获取 CNN 恐贪指数并保存当日快照（历史快照用于信号平滑与区间确认） |
| `config` | 用法三态不明 | 无参数=输出完整配置(TOML)；KEY=查看；KEY VALUE=修改（校验通过后落盘）。可调参数清单见 `mns config params` |
| `init` | 不知创建什么 | 初始化 ~/.mns/config.toml 与 ~/.mns/mns.db（默认平衡档；已有数据会提示确认） |
| `history` | 只说"交易历史" | 查看交易流水（买入/卖出/入金/出金/纠错，新→旧） |
| `update-prices` | 无交叉引用 | 自动拉取全部持仓最新价（mns today 也会先做这一步） |
| `price` | 无交叉引用 | 查看/更新单个资产当前价（批量自动更新见 update-prices） |
| `portfolio` | 无交叉引用 | 查看持仓概览（含年化收益）；组合级 XIRR/回撤见 performance |
| `performance` | 无交叉引用 | 组合绩效仪表盘：XIRR / 实际最大回撤 / 对照策略预期（只读，零写入）；单标的角度见 portfolio |
| `backtest` | — | 回测当前配置 vs 买入持有等基准（同一数据与成本模型）；样本外验证见 `backtest validate` |
| `backtest validate` | 已够清楚 | 保持 |
| `cash set/deposit/withdraw` | 已够清楚 | 保持 |

---

## 验收标准（全量）

1. `mns --help` 与所有子命令 `-h` 的**用户可见输出**中不再出现"旧"字样（源码注释不在此列）。
2. `mns cash add 100`、`mns backtest run`、`mns market-indices` 均报错；等价新路径全部可用。
3. `mns backtest` 输出无"旧框架(现金比例)"行，对比表仍完整（4 策略）。
4. 参数清单（迁移后 `mns config params`）无【旧框架参数】块，annualized_target 按方案 a 归组并改注释。
5. `cargo build --release` + `cargo test` 全绿；被删路径的测试一并清理。
6. 文档同步：`.agents/skills/mns-backtest/SKILL.md`（backtest run → backtest）、`distribution/skill/money-never-sleep/SKILL.md`（cash add → cash deposit、market-indices 删除）、README 如有引用一并更新。
7. 主 -h 的速览数字与 config 默认值一致（人工核对一次）。

## 范围外（本次不做）
- analyze 补估值实现（另行立项）
- report/sentiment 等运行时输出的文案（本次只管 -h 与命令结构）
- quote.rs 天天基金旧接口（数据源降级链，与"旧策略"无关）
