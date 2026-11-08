---
name: mns-backtest
description: |
  This skill should be used when the user wants to run backtests on the MNS (Market Neutral
  Strategist) contrarian investment strategy, analyze historical CNN Fear & Greed Index data,
  compare strategy performance against buy-and-hold, or tune strategy parameters.

  Triggers include: "回测", "backtest", "策略回测", "参数调优", "历史表现",
  "逆向策略表现", "恐贪指数回测", "测试我的策略", "MNS 回测", "参数对比"
---

# MNS 逆向投资策略回测 Skill

> 2026-11-08 整改后版本。旧的 2016–2025 月频代理对比已降级为 `mns backtest legacy`，
> **其结论不可迁移到实盘**：标的、指数、通道、信号口径、注资假设均与真实账户不同。
> 评估依据见 `docs/backtest-validation-review-2026-11-08.md` 与 `docs/remediation-2026-11-08.md`。

## 操作步骤

### 1. 构建与数据

```bash
cargo build --release
mns backtest update-data      # 重建两层日频数据集；需 Python 3 与网络
```

`update-data` 会同时刷新实盘层与长周期层，并输出数据新鲜度 SLA 校验结果。
数据集落后超过 `backtest.freshness_sla_days`（默认 30 天）时，`mns backtest` 会输出醒目告警。

### 2. 运行

```bash
mns backtest                 # 默认：完整实盘审计
mns backtest mirror          # 只看账户镜像（严格执行 vs 实际执行）
mns backtest validate-full   # 只看统计检验，可加 --iterations 500
mns backtest legacy          # 已降级：旧月频代理对比（--config / --compare 仅在此生效）
mns config params            # 全部可调参数（含 [backtest] / [channels]）
```

默认审计链路依次输出：数据与口径 → 策略有效性对照（含信号时序置换检验）→
最近 36 月连续复利 holdout → 牛熊分段 → 滚动多折 walk-forward → 账户镜像 → 边界声明。

### 3. 参数调优

```bash
mns config
mns config backtest.decision_interval weekly
mns config rebalance.band_pp 6
```

生效配置始终来自 `~/.mns/config.toml`。`.agents/skills/mns-backtest/data/config_*.toml`
七个文件是**参考预设，不会被任何命令自动加载**；只能通过 `mns backtest legacy --config <路径>`
显式指定。它们只含 `[target_weight]` / `[rebalance]` 等参数，不含 `[backtest]` / `[channels]`，
缺省部分会回落到 `AppConfig::default_config()`。

⚠️ 实盘建议与默认回测共用 `[target_weight]` + `[rebalance]` + `[backtest.decision_interval]`，
不允许两条链路各设一套信号后互相引用结论。

## 数据说明（两层，不可混用）

两层数据集均由 point-in-time as-of 对齐生成：每一行的 FGI 取**严格早于**该交易日的
最后一条已公布值（CNN 美股收盘后计算，北京时间次日才可得），净值取不晚于当日的
最后一条已披露值（QDII 净值 T+1/T+2 披露，缺口沿用前值）。因此不存在
「用当日情绪按当日净值同刻成交」的前视偏差。

| 数据集 | 区间 | 标的 | 用途 |
|---|---|---|---|
| `live_daily.csv` | 2024-01-11 ~ 今（682 行） | 实盘四只场外基金 018966 / 539001 / 007605 / 020341 的真实全收益指数 + 单位净值 | 账户镜像、实盘结论 |
| `long_daily.csv` | 2016-01-04 ~ 今（2637 行） | 三腿长历史代理：539001 / H30269 中证红利低波**价格指数** / 518880 黄金 ETF | 穿越牛熊的稳健性检验 |

生成脚本分别为 `build_live_dataset.py` 与 `build_long_dataset.py`，元数据为
同目录下的 `*_meta.json`（含抓取时间、各腿起止、口径限制、牛熊分段定义）。

**长周期层必须代理的原因**：实盘四只基金中最晚成立的 020341 只有 2024-01 起的净值，
共同区间仅 34 个月且只有单一 regime。为覆盖 2016 年至今的四段牛熊，只能改用可比标的。
由此产生的口径差异必须在结论中声明：

- 纳指腿用 539001 单只代表（018966 成立于 2023-08，无法回溯到 2016）
- 红利腿用 H30269 价格指数，而实盘 007605 跟踪的是 930740 沪深300红利低波；
  后者 2018-12 才有数据。价格指数不含股息，会低估该腿绝对收益，但策略与基准
  同口径，**相对**比较仍成立
- 黄金腿用场内 ETF 518880 价格，实盘 020341 是其场外联接 E 份额
- FGI 来自 `whit3rabbit/fear-greed-data` 合并存档（2011 起日频），2021-02 前后
  存在整数/浮点**精度**断点，非数值断点

未覆盖 2000-2002 互联网泡沫与 2008 型多年熊市，这两段仍属未经检验区间。

旧的 `monthly_total_return.csv` / `monthly_total_return_holdout.csv` 仍由
`build_dataset.py` 生成，但只服务 `legacy` 命令，标的是 270042 / 510880 / 518880，
信号是 3 月平滑 + 月末决策，不得用于实盘决策。

## 核心逻辑文件

- `src/audit.rs` — **主验证链路**：`build_signal_series()` 直接调用 `signal::compute`
  （同源 7 天平滑 + 3 天确认 + 决策日）；`run_audit()` 实现 T+lag 执行、按通道 T+n
  结算、在途资金不可复用、每日限购、双口径回撤；`run_actual()` 用真实交易流水
  净额回放账户；`run_validation_suite()` 跑四个对照与 ≥300 次置换检验；
  `run_regimes()` / `run_walk_forward()` 做牛熊分段与滚动多折验证
- `src/live_data.rs` — 两层数据集的加载、校验、新鲜度 SLA、滚动更新与统一 `BarSeries` 转换
- `src/config.rs` — `[backtest]` 审计设置与 `[channels]` 按通道成本；实盘赎回阶梯按
  007605 真实规则（<7日 1.5% / 7–365日 0.5% / 365–730日 0.25% / ≥730日 0）
- `src/backtest.rs` — 已降级的月频代理引擎；保留双口径回撤、执行滞后与注资/信号
  驱动交易分解，仅供长周期参考
- `src/signal.rs` — 实盘决策信号；实盘与回测共用同一函数

## 解读结果

0. **先看置换检验判定**。真实信号若未进入随机时序分布前 `significance_percentile`
   （默认 80）分位，报告明确写「择时无显著贡献」，此时收益差异主要由平均风险仓位
   解释，不应据此宣称择时 alpha。旧版文档曾引用打印型测试输出的
   「14.8→10.2 次/年」论证平滑设计有效——那组数字来自**非生效配置**且使用月频信号，
   与生效配置下 3.4 次/年差 3–4 倍，已删除，不得再作为证据。
1. **看牛熊分段而非只看全区间**。全区间结论容易被单一长牛主导；分段表把
   2018Q4 / 2020 / 2022 / 2023-2025 牛市 / 2026 回调拆开，检验「低回撤」卖点是否
   只在牛市里换来跑输。
2. **看 walk-forward 的改善幅度**。折数更优不等于有价值；报告同时给出平均年化
   改善，低于 0.5pp/年即不建议据此调参。
3. **看账户镜像**。严格执行与真实流水的净值差、费用差、受阻次数（QDII 限购 /
   在途资金未到账）才是可执行的反馈。实盘层只有 34 个月共同区间，结论强度有限。
4. **口径披露**。日报（`mns report`）与回测同源信号，但长周期层是代理标的；
   报告末尾的【口径披露】列明这一差异。

策略可验证的价值是**纪律性的仓位约束**，不是择时 alpha。本工具不构成投资建议。
