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

## 操作步骤

### 1. 构建

```bash
cargo build --release
```

### 2. 运行回测

```bash
# 默认行为：先输出多资产回测（美股+红利低波+黄金）对比，再输出单资产（纳指）参数对比
mns backtest

# 使用自定义配置文件（仅单资产回测）
mns backtest run --config path/to/config.toml

# 多配置文件对比（仅单资产回测）
mns backtest run --compare config1.toml,config2.toml

# 查看可调参数说明（含当前默认值）
mns backtest params
```

### 3. 参数调优

查看当前配置：
```bash
mns config
```

修改单个参数：
```bash
mns config buy_ratio.extreme_fear 70
mns config thresholds.fear 40
```

使用预设配置文件（位于 `.agents/skills/mns-backtest/data/`）：

| 配置文件 | 特点 |
|---------|------|
| `config_defensive.toml` | 防御配置（低回撤，更长持仓，与 `AppConfig::default_config()` 的 `target_weight` 曲线一致——**是当前默认**） |
| `config_balanced.toml` | 均衡配置（三资产等权，风险权重曲线更平缓） |
| `config_circuit_breaker.toml` | 熔断抄底（极度恐慌区间风险权重更高、偏离带更宽） |
| `config_swing.toml` | 波段操作（偏离带更窄，交易更频繁） |
| `config_extreme_contrarian.toml` | 极致逆向（风险权重区间跨度最大） |
| `config_value.toml` | 价值导向（阈值更极端，只在真正的极端行情动仓） |
| `config_historical_aggressive.toml` | 历史激进配置（⚠️ 过拟合风险，仅供研究参考） |

```bash
mns backtest run --config .agents/skills/mns-backtest/data/config_defensive.toml
```

⚠️ **重要**：驱动回测与 `mns report` 实盘建议的是 `[target_weight]`（风险资产目标权重曲线，
见 `src/config.rs` 的 `TargetWeight`）+ `[rebalance]`（偏离带）。**`[buy_ratio]`/`[sell_ratio]` 段只
被"旧框架"（`Engine::Legacy`，回测对比里的"旧框架(现金比例)"一行）和 `calculate_buy_suggestions`/
`calculate_sell_suggestions` 使用，`mns report` 走的是 `calculate_rebalance_plan`，完全不读这两段**。
上述 7 个预设文件都**已补齐 `[target_weight]` 段**（历史版本缺失，改 `buy_ratio`/`sell_ratio` 对 `mns
report` 和默认回测引擎没有任何效果）；若只想对比"旧框架"，才需要关注 `buy_ratio`/`sell_ratio`。

## 数据说明

回测使用嵌入式数据（`include_str!` 编译进二进制，见 `src/backtest.rs` 顶部）：

- `monthly_total_return.csv` — 主序列：多资产月度全收益数据（纳指/红利低波/人民币金价，含分红，2016-2025）
- `monthly_total_return_holdout.csv` — 独立 holdout 区块，从未参与调参，`mns backtest validate` 用它做最终检验
- `fgi_2016_2020.csv` / `fgi_2020_2025.csv` — 恐贪指数原始数据，经 `build_dataset.py` 加工进上述月度序列，不会被 `include_str!` 直接编译进二进制

**注意**：数据更新需要修改 CSV 文件（必要时先跑 `build_dataset.py` 重新生成月度序列）并重新
`cargo build --release`。

## 核心逻辑文件

- `src/backtest.rs` — 回测引擎；核心函数是 `run()`（单次回测，按 `Engine` 枚举选择目标仓位/旧框架/买入持有等
  引擎）、`print_report()`、`print_comparison()`；CLI 入口在 `src/main.rs` 的 `cmd_backtest` /
  `cmd_backtest_validate` / `cmd_backtest_params`，对应 `mns backtest run|validate|params`
- `src/strategy.rs` — 实盘策略核心：`calculate_rebalance_plan`（`target_weight` 驱动，`mns report` 与
  `Engine::TargetWeight` 回测同源）、`calculate_buy_suggestions`/`calculate_sell_suggestions`（`buy_ratio`/
  `sell_ratio` 驱动，仅供"旧框架"对比使用）
- `src/config.rs` — 参数结构定义，`AppConfig::default_config()` 是当前防御配置默认值（`target_weight`
  曲线与 `config_defensive.toml` 一致）

## 解读结果

运行后输出对比表（年化收益、总收益率、最大回撤、买卖次数），关注：

1. **vs 买入持有**：策略年化是否接近基准——历史区间内通常是**跑输**买入持有的，工具的价值主张仅在
   Calmar（风险调整后）成立，且该优势在 `mns backtest validate` 的样本外验证里并不稳健，务必一并看
   walk-forward、bootstrap、holdout 三段输出，不要只看 `mns backtest` 默认展示的样本内数字
2. **最大回撤**：回撤越低说明策略防御性越强
3. **调整 `[target_weight]`**：中性区间权重设低会明显降低仓位波动；`buy_ratio`/`sell_ratio` 对默认引擎和
   `mns report` 无效，只影响"旧框架"对比行

策略价值在于**纪律性**而非超额收益——帮助克服情绪化决策，在极端市场（恐慌/贪婪）时强制执行买卖信号。
