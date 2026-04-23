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

## 概述

本 skill 提供 MNS 逆向投资策略的回测能力，**基于 MNS CLI 的真实策略引擎**，复用 `src/strategy.rs` 的完整逻辑，确保回测结果与实际使用效果一致。

## 使用方式

```bash
# 基本回测（多配置对比）
mns backtest

# 使用自定义配置
mns backtest run --config my_config.toml

# 多配置对比
mns backtest run --compare config1.toml,config2.toml

# 查看可调参数
mns backtest params
```

## 关键回测结论（2016-2025）

### 预设配置对比

| 配置 | 年化收益 | 总收益率 | 最大回撤 | 买入次数 | 卖出次数 |
|------|---------|---------|---------|---------|---------|
| 防御配置（默认） | 8.87% | 119.49% | 23.10% | 38 | 12 |
| 激进配置 | 9.20% | 125.67% | 24.99% | 38 | 12 |
| 超激进配置 | 9.42% | 130.08% | 26.50% | 38 | 13 |
| 极致激进 | 9.58% | 133.19% | 27.56% | 38 | 12 |
| 保守配置 | 8.41% | 111.06% | 21.09% | 38 | 12 |
| 无中性配置 | 8.20% | 107.33% | 21.09% | 22 | 12 |
| 买入持有 | 10.97% | 161.91% | 14.70% | 11 | 0 |

### 核心发现

1. **逆向策略收益低于买入持有**：年化差距约 2.1%
2. **防御性反而更差**：最大回撤比买入持有高约 8%
3. **无中性配置降低交易频率**：买入从 38 次降到 22 次

### 策略价值说明

**既然策略不如买入持有，为什么还要用？**

逆向策略的核心价值是**纪律性**而非收益最大化：
- 帮助用户克服"恐慌时不敢买、贪婪时不愿卖"的人性弱点
- 在极端市场提供系统性信号（如 2020 年熔断、2022 年熊市）
- 强制执行买卖纪律，避免情绪化决策

适合人群：
- 容易受市场情绪影响的投资者
- 需要外部信号辅助决策的投资者
- 希望系统化管理仓位纪律的投资者

## 预设配置说明

回测提供以下预设配置（位于 `.agents/skills/mns-backtest/data/`）：

| 配置文件 | 特点 | 适用场景 |
|---------|------|---------|
| `config_defensive.toml` | 低回撤优先，中性不买入 | 稳健型投资者（默认） |
| `config_circuit_breaker.toml` | 熔断机制，极端行情暂停 | 高波动市场 |
| `config_balanced.toml` | 三资产均衡配置 | 分散风险 |
| `config_swing.toml` | 波段操作，高抛低吸 | 短期交易 |
| `config_extreme_contrarian.toml` | 极端逆向，恐慌重仓 | 风险承受能力强 |
| `config_value.toml` | 价值导向，基本面筛选 | 长线投资者 |

**注意**：历史表现最好的配置不代表未来最优，过度拟合风险需警惕。

## 数据说明

- **2016-2020.09**：逐日恐贪指数（高置信度）
- **2020.10-2025.04**：月度近似值（低置信度）
- **S&P 500**：月度收盘价

回测假设：
- 初始资金 ¥100,000
- 每年 2 月追加 ¥50,000
- 忽略手续费、滑点、税收

## 相关文件

- `src/backtest.rs` - 回测引擎
- `src/strategy.rs` - 策略核心逻辑
- `data/*.toml` - 预设配置文件