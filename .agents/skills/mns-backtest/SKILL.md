---
name: mns-backtest
description: |
  This skill should be used when the user wants to run backtests on the MNS (Market Neutral
  Strategist) contrarian investment strategy, analyze historical CNN Fear & Greed Index data,
  compare strategy performance against buy-and-hold, or tune strategy parameters (thresholds,
  buy/sell ratios, profit-taking lines).

  Triggers include: "回测", "backtest", "策略回测", "参数调优", "历史表现",
  "逆向策略表现", "恐贪指数回测", "测试我的策略", "MNS 回测", "参数对比"
---

# MNS 逆向投资策略回测 Skill

## 概述

本 skill 提供 MNS 逆向投资策略的回测能力，**基于 MNS CLI 的真实策略引擎**，复用 `src/strategy.rs` 的完整逻辑，确保回测结果与实际使用效果一致。

## 核心能力

1. **策略回测**: 运行完整的逆向策略回测（买入/卖出逻辑 + 年化止盈判断 + 绝对止盈）
2. **基准对比**: 逆向策略 vs 买入持有，输出年化收益、最大回撤、交易次数等指标
3. **参数对比**: 对比不同参数配置的回测结果，支持多配置并行对比
4. **预设配置**: 内置激进、保守、无中性等预设配置，快速验证参数调整效果

## 使用方式

### 基本回测

```bash
mns backtest
```

默认运行参数对比模式，输出：
- 默认配置回测结果
- 激进配置（提高极度恐慌/恐慌买入比例）
- 保守配置（降低中性买入比例）
- 无中性配置（中性买入比例=0%）
- 买入持有基准

### 指定配置文件

```bash
mns backtest run --config my_config.toml
```

使用自定义配置文件运行回测，配置文件格式与 `~/.mns/config.toml` 相同。

### 多配置对比

```bash
mns backtest run --compare config1.toml,config2.toml,config3.toml
```

对比多个配置文件的回测结果，输出对比表格。

### 查看可调参数

```bash
mns backtest params
```

显示所有可调参数及其默认值。

## 参数说明

### 阈值参数 (thresholds)

| 参数 | 默认值 | 说明 |
|------|--------|------|
| extreme_fear | 25 | 极度恐慌阈值，FGI < 此值为极度恐慌 |
| fear | 45 | 恐慌阈值，FGI 在 [extreme_fear, fear) 区间为恐慌 |
| neutral | 55 | 中性阈值，FGI 在 [fear, neutral) 区间为中性 |
| greed | 75 | 贪婪阈值，FGI 在 [neutral, greed) 区间为贪婪，≥greed 为极度贪婪 |

### 买入比例 (buy_ratio)

| 参数 | 默认值 | 说明 |
|------|--------|------|
| extreme_fear | 50% | 极度恐慌时投入可用现金比例 |
| fear | 30% | 恐慌时投入可用现金比例 |
| neutral | 20% | 中性时投入可用现金比例 |
| greed | 0% | 贪婪时暂停买入 |

### 卖出比例 (sell_ratio)

根据情绪区间 + 年化收益决定减仓比例：

| 情绪区间 | 年化 ≥ 15% | 10% ≤ 年化 < 15% | 年化 < 10% |
|----------|-----------|-----------------|-----------|
| 极度贪婪 | 50% | 30% | 20% |
| 贪婪 | 40% | 20% | 0% |
| 中性 | 30% | 0% | 0% |

### 其他参数 (settings)

| 参数 | 默认值 | 说明 |
|------|--------|------|
| annualized_target_low | 10% | 低止盈线 |
| annualized_target_high | 15% | 高止盈线 |
| min_holding_days | 30 | 年化收益计算的最小持仓天数 |
| max_contrarian_weight | 2.0 | 逆向加权的最大权重上限 |

## 回测配置示例

创建自定义配置文件 `my_config.toml`：

```toml
[settings]
annualized_target_low = 10.0
annualized_target_high = 15.0
min_holding_days = 30
min_absolute_profit_days = 90
max_contrarian_weight = 2.0
report_output_dir = "./reports"

[allocation]
us_stocks = 50.0
cn_stocks = 35.0
counter_cyclical = 15.0

[thresholds]
extreme_fear = 25.0
fear = 45.0
neutral = 55.0
greed = 75.0

[buy_ratio]
extreme_fear = 60.0   # 提高极度恐慌买入比例
fear = 35.0
neutral = 0.0         # 关闭中性买入
greed = 0.0

[sell_ratio]
extreme_greed_target_high = 60.0
extreme_greed_target_low = 40.0
extreme_greed_below_target = 30.0
greed_target_high = 50.0
greed_target_low = 30.0
neutral_target_high = 30.0

[api]
fear_greed_url = "https://production.dataviz.cnn.io/index/fearandgreed/graphdata"
```

运行回测：

```bash
mns backtest run --config my_config.toml
```

## 关键回测结论（CLI 验证）

以下结论基于 2016-2025 回测（使用真实策略引擎）：

### 预设配置对比
| 配置 | 年化收益 | 总收益率 | 最大回撤 | 买入次数 | 卖出次数 |
|------|---------|---------|---------|---------|---------|
| 默认配置 | 5.93% | 70.74% | 17.74% | 83 | 16 |
| 激进配置 | 6.00% | 71.77% | 18.62% | 78 | 16 |
| 保守配置 | 5.60% | 65.83% | 16.88% | 84 | 16 |
| 无中性配置 | 5.31% | 61.70% | 16.75% | 41 | 15 |
| 买入持有 | 7.16% | 90.17% | 20.59% | 1 | 0 |

### 核心发现

1. **长牛市场中逆向策略略逊于买入持有**：年化收益差距约 1.2%

2. **防御性更好**：最大回撤比买入持有低约 3%

3. **无中性配置交易频率大幅降低**：从 83 次买入降到 41 次，但收益也相应降低

4. **激进配置效果最好**：提高极度恐慌买入比例到 70%，年化收益 6.00%

### 优化建议

- 中性区间买入过频但贡献有限，建议降至 10% 或 0%
- 极度恐慌买入可提升至 60-70%
- 极度贪婪时高年化的减仓比例可提升至 60%

## 数据说明

- **2016-2020.09**: 逐日恐贪指数（高置信度，来源: GitHub 开源数据集）
- **2020.10-2025.04**: 月度近似值（低置信度，关键时点重建）
- **S&P 500**: 月度收盘价

回测假设：
- 初始资金 ¥100,000
- 每年 2 月追加 ¥50,000
- 忽略手续费、滑点、税收

## 与 Python 脚本的区别

| 特性 | Python 脚本 (旧) | CLI 回测 (新) |
|------|----------------|--------------|
| 策略逻辑 | 简化版 | **完整版** |
| 绝对止盈 | ❌ | ✅ |
| 逆向加权 | ❌ | ✅ |
| 最小持仓天数 | ❌ | ✅ |
| 多资产配置 | ❌ | ✅ |
| 参数对比 | 手动修改 | **命令行支持** |

## 相关文件

- `src/backtest.rs` - 回测引擎模块
- `src/strategy.rs` - 策略核心逻辑
- `src/config.rs` - 配置管理
- `.agents/skills/mns-backtest/data/fgi_2016_2020.csv` - 2016-2020 逐日恐贪指数
- `.agents/skills/mns-backtest/data/fgi_2020_2025.csv` - 2020-2025 月度恐贪指数
- `.agents/skills/mns-backtest/data/sp500_monthly.csv` - S&P 500 月度收盘价