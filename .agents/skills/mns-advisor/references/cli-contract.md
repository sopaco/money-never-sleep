# MNS CLI 契约（供 AGENT 调用前核对）

二进制：`mns`（或项目内 `target/release/mns`）。生效配置：`~/.mns/config.toml`，台账：`~/.mns/mns.db`。全部命令以 `mns --help` 与 `src/cli.rs` 为准。

## 只读 / 写入分类

| 命令 | 副作用 | 前置条件 | 关键输出 |
|---|---|---|---|
| `config`、`config params`、`config <键>` | 只读 | 无 | 完整 TOML / 参数清单 |
| `cash`、`portfolio`、`performance`、`history N` | 只读 | 无 | 余额、持仓、组合 XIRR / 实际回撤、流水 |
| `price <代码>`（不带值） | 只读 | 标的存在 | 当前价 |
| `guard` | 只读 | 本地有 FGI 快照 | 决策分数、是否过热、冷静期提示 |
| `plan --monthly <金额>` | 只读 | 本地有 FGI 快照；金额 >0 | 三腿买入金额与标的分配 |
| `backtest mirror` | 只读 | 三层 CSV 存在 | 数据新鲜度横幅、严格执行 vs 实际流水 |
| `backtest asset --dataset hs300`、`backtest validate-full` | 只读 | 三层 CSV 存在 | 对照实验 / 统计检验 |
| `market`、`analyze <代码>` | 联网只读 | 网络 | 指数、个股报价 |
| `report`、`report --decide` | **写**：FGI 快照、当日资产快照、报告文件 | 网络或本地快照 | 决策版 / 观察版报告 |
| `today` | **写**：上述 + 全部持仓最新价 | 网络 | 刷价 + 报告 |
| `update-prices`、`sentiment` | **写**：价格 / 快照 | 网络 | 更新结果 |
| `buy`、`sell`、`cash deposit`、`cash withdraw`、`cash set`、`add`、`remove`、`price <代码> <值>` | **写**：台账 | 用户确认实际成交 | 流水记录 |
| `backtest update-data` | **写**：三层 CSV 与 meta | 网络 + Python 3 | 重建结果 |
| `init`、`init --force` | **写**：可能覆盖配置与 DB | 用户确认 | 初始化结果 |

采集阶段只跑第一组；`report` / `today` 只在 Step 3 确需决策版且用户知情后运行。

## 输出字段释义

| 字段 | 含义 | 常见误读 |
|---|---|---|
| 今日 FGI / 原始分区 | CNN 当日值，仅展示 | 误当作买卖信号 |
| 决策分数 `effective_score` | 7 天平滑 + 3 天区间确认后的值 | 误以为等于当日 FGI |
| 「区间切换未确认」 | 决策沿用旧区间及其平滑值 | 误以为信号失灵 |
| 决策日 / 观察日 | `rebalance.decision_weekday`，当前 5=周五 | 观察日误用 `--decide` 强行取建议 |
| 偏离 `drift_pp` | 当前权重 − 目标权重（百分点） | 与「收益率」混淆 |
| `hold_reason` | 在带宽内 / 金额过小 / 未过最短持有期 / 资金不足 | 忽略后误判「策略没给建议」 |
| `cash_constrained` | 现金 + 卖出回收不足以补全部缺口，按比例缩减 | 误以为需要额外入金 |
| 年度交易预算 | 本年已操作笔数 / 10–20 次 | 忽略超限提示继续逐日跟单 |
| 【口径披露】 | 日报与回测同源、长周期层为代理标的 | 把代理回测收益当实盘预期 |

`plan` 的分母 = 现金 + 持仓 + 本月新钱；`report` 的分母 = 现金 + 持仓。两者目标权重百分比相同、金额不同，属正常。

## 关键配置键

| 键 | 当前值 | 作用 |
|---|---|---|
| `target_weight.*` / `target_weight.mode` | 88/72/62/48/28，step | 情绪 → 目标风险仓位曲线 |
| `profile.core_blend` | 0.4（steady） | 目标 = blend×100 + (1−blend)×曲线；抬高常驻仓位，不降低风险 |
| `allocation.*` | 45/35/20 | 三腿内部比例 |
| `thresholds.*` | 20/40/55/75 | 五档分区边界 |
| `rebalance.band_pp` | 6.0 | 偏离带 |
| `rebalance.min_trade_amount` | 500 | 最小交易额 |
| `rebalance.min_holding_days_for_sell` | 60 | 卖出最短持有期 |
| `rebalance.signal_smooth_days` / `zone_confirm_days` / `decision_weekday` | 7 / 3 / 5 | 信号三级过滤 |
| `backtest.annual_trade_budget_min/max` | 10 / 20 | 年度交易预算与日报披露 |
| `backtest.freshness_sla_days` | 30 | 三层数据新鲜度告警阈值 |
| `channels.*` | T+7 / T+3 / T+1、¥520 日限购 | 结算与限购假设，需联网核验 |
| `guardrail.hot_threshold` / `cooldown_days` | 75 / 1 | 冷静期提示，不否决交易 |
| `cashflow.monthly` | 0.0 | `plan` 默认月投；为 0 时必须带 `--monthly` |

## 已知坑

1. `mns plan` 不带 `--monthly` 会报错（当前 `cashflow.monthly = 0`），与版本无关。
2. `guard` / `plan` 不联网，依赖本地 FGI 快照；新环境先跑一次 `mns report` 并说明其写副作用。
3. `mns report` 会 upsert 当日资产快照（`performance` 与实际回撤的数据源），不要为「看一眼」反复运行。
4. `--decide` 在情绪快照 >7 天时直接拒绝，这是刻意保护，不要寻找绕过方式。
5. `cash set` 是纠错工具，记 adjust 流水；外部入金必须用 `cash deposit`，否则 XIRR 口径失真。
6. `mns backtest`（默认）会跑完整审计与置换检验，不是收益展示；`legacy` / `validate` 的数字不可迁移到实盘。
7. 价格陈旧 ≥1 天时报告顶部有警告，先 `update-prices` 再谈金额。
8. `[target_weight]` / `[rebalance]` / `backtest.decision_interval` 实盘与回测同源，改一处等于改两处。
