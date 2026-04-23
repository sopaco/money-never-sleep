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
# 多配置对比（默认行为）
mns backtest

# 使用自定义配置文件
mns backtest run --config path/to/config.toml

# 多配置文件对比
mns backtest run --compare config1.toml,config2.toml

# 查看可调参数说明
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
| `config_defensive.toml` | 防御配置（低回撤，中性不买，**当前默认**） |
| `config_balanced.toml` | 均衡配置 |
| `config_circuit_breaker.toml` | 熔断机制 |
| `config_swing.toml` | 波段操作 |
| `config_extreme_contrarian.toml` | 极致逆向 |
| `config_value.toml` | 价值导向 |

```bash
mns backtest run --config .agents/skills/mns-backtest/data/config_defensive.toml
```

## 数据说明

回测使用嵌入式数据（`include_str!` 编译进二进制）：

- `fgi_2016_2020.csv` — 逐日 CNN 恐贪指数（高置信度）
- `fgi_2020_2025.csv` — 月度近似恐贪指数（低置信度）
- `monthly_real_final.csv` — 多资产月度价格（纳指/红利低波/人民币金价）

**注意**：数据更新需要修改 CSV 文件并重新 `cargo build --release`。

## 核心逻辑文件

- `src/backtest.rs` — 回测引擎，`run_backtest()`、`run_multi_asset_backtest()`、`run_param_comparison()`
- `src/strategy.rs` — 策略核心，回测复用实际策略逻辑，结果与实盘一致
- `src/config.rs` — 参数结构定义，`AppConfig::default_config()` 为防御配置默认值

## 解读结果

运行后输出对比表（年化收益、总收益率、最大回撤、买卖次数），关注：

1. **vs 买入持有**：策略年化是否接近基准
2. **最大回撤**：回撤越低说明策略防御性越强
3. **买入次数**：中性区间 buy_ratio 设为 0 会明显降低频率

策略价值在于**纪律性**而非超额收益——帮助克服情绪化决策，在极端市场（恐慌/贪婪）时强制执行买卖信号。
