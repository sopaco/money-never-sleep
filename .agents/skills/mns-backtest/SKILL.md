---
name: mns-backtest
description: |
  This skill should be used when the user wants to run backtests on the MNS (Market Neutral
  Strategist) contrarian investment strategy, analyze historical CNN Fear & Greed Index data,
  compare strategy performance against buy-and-hold, or tune strategy parameters (thresholds,
  buy/sell ratios, profit-taking lines).

  Triggers include: "回测", "backtest", "策略回测", "参数调优", "历史表现",
  "逆向策略表现", "恐贪指数回测", "测试我的策略", "MNS 回测"
---

# MNS 逆向投资策略回测 Skill

## 概述

本 skill 提供 MNS 逆向投资策略的回测能力，基于 CNN Fear & Greed Index 历史数据和 S&P 500 价格数据，评估策略在 2016-2025 年间的表现，并与买入持有基准对比。

## 核心能力

1. **策略回测**: 运行完整的逆向策略回测（买入/卖出逻辑 + 年化止盈判断）
2. **基准对比**: 逆向策略 vs 买入持有，输出年化收益、最大回撤、交易次数等指标
3. **情绪分析**: 各恐贪指数区间下市场后续表现统计（3个月后收益、胜率）
4. **参数调优**: 修改 `scripts/backtest_runner.py` 中的策略参数后重跑，评估参数变化的影响

## 使用方式

### 回测执行

```bash
python .agents/skills/mns-backtest/scripts/backtest_runner.py
```

输出内容:
- 逆向策略收益概览（总收益、年化、最大回撤）
- 买入/卖出统计（按情绪区间分类）
- 关键交易明细（每年 Top 3）
- 策略对比表（逆向 vs 买入持有）
- 各情绪区间后续市场表现分析

### 参数调优

直接编辑 `scripts/backtest_runner.py` 顶部的策略参数：

```python
# 恐贪指数阈值
THRESHOLDS = {"extreme_fear": 25, "fear": 45, "neutral": 55, "greed": 75}

# 买入比例
BUY_RATIO = {"extreme_fear": 0.50, "fear": 0.30, "neutral": 0.20, "greed": 0.00}

# 卖出减仓矩阵
SELL_RATIO = {
    ("extreme_greed", "above_high"): 0.50,
    ("extreme_greed", "between"): 0.30,
    ("extreme_greed", "below_low"): 0.20,
    ("greed", "above_high"): 0.40,
    ("greed", "between"): 0.20,
    ("greed", "below_low"): 0.00,
    ("neutral", "above_high"): 0.30,
    ("neutral", "between"): 0.00,
    ("neutral", "below_low"): 0.00,
}

# 止盈线
ANNUALIZED_TARGET_LOW = 10.0
ANNUALIZED_TARGET_HIGH = 15.0
```

修改后重新运行脚本即可获得新的回测结果。

## 关键回测结论（已验证）

以下结论基于 2016-2025 回测，可作为参数调整的参考方向：

### 策略 vs 基准
| 指标 | 逆向策略 | 买入持有 |
|------|---------|---------|
| 年化收益 | 6.93% | 7.18% |
| 最大回撤 | 19.07% | 20.85% |

**结论**: 长牛市场中逆向策略略逊，但防御性更好。

### 各情绪区间市场后续表现（3个月）
| 情绪区间 | 均收益 | 胜率 |
|---------|-------|------|
| 极度恐慌 | +7.19% | 81.2% |
| 恐慌 | +8.66% | 95.6% |
| 中性 | +5.77% | 89.6% |
| 贪婪 | +4.20% | 74.9% |
| 极度贪婪 | -1.41% | 50.7% |

**结论**: 恐慌区间买入胜率和收益均远高于贪婪区间，逆向逻辑有效。

### 已知优化方向
- 中性区间买入过频（80次/147次），建议降至 10% 或 0%
- 极度恐慌买入可提升至 60-70%
- 极度贪婪时年化≥15%的减仓比例可提升至 60%

## 数据限制说明

- **2016-2020.09**: 逐日恐贪指数（高置信度，来源: GitHub 开源数据集）
- **2020.10-2025.04**: 月度近似值（低置信度，关键时点重建）
- **S&P 500**: 月度收盘价

回测结果仅供参考，实际交易需考虑手续费、滑点、税收等因素。

## 相关文件

- `scripts/backtest_runner.py` - 回测执行脚本
- `references/strategy_params.md` - 策略参数详解
- `references/cli_spec.md` - MNS CLI 命令规范
