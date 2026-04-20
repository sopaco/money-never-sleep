# MNS 策略回测报告

## 回测期间
2016年1月 - 2025年4月（约9年）

## 回测假设
- 初始资金：¥100,000
- 年度注资：¥50,000（每年3月末）
- 总投入：¥600,000
- 数据频率：月度
- 忽略交易成本、滑点、税收

## 策略对比结果

| 配置 | 年化收益 | 总收益率 | 最大回撤 | 买入次数 | 卖出次数 |
|------|----------|----------|----------|----------|----------|
| 买入持有 | 7.29% | 91.68% | 14.70% | 11 | 0 |
| **熔断抄底** | **6.17%** | **74.00%** | 19.91% | 30 | 14 |
| 极致逆向 | 5.88% | 69.73% | 19.20% | 22 | 15 |
| 默认配置 | 5.57% | 65.14% | 17.95% | 38 | 8 |
| 价值投资 | 5.48% | 63.78% | 17.73% | 18 | 9 |
| 波段交易 | 5.13% | 58.84% | 19.38% | 44 | 16 |

## 最优配置：熔断抄底 (config_circuit_breaker.toml)

```toml
[settings]
annualized_target_low = 10.0
annualized_target_high = 15.0
min_holding_days = 14
min_absolute_profit_days = 30
max_contrarian_weight = 4.0

[allocation]
us_stocks = 60.0
cn_stocks = 25.0
counter_cyclical = 15.0

[thresholds]
extreme_fear = 35.0
fear = 48.0
neutral = 52.0
greed = 68.0

[buy_ratio]
extreme_fear = 90.0
fear = 60.0
neutral = 0.0
greed = 0.0

[sell_ratio]
extreme_greed_target_high = 60.0
extreme_greed_target_low = 40.0
extreme_greed_below_target = 25.0
greed_target_high = 45.0
greed_target_low = 30.0
neutral_target_high = 15.0
```

## 核心发现

1. **逆向策略在长牛市场中略逊于买入持有**
   - 年化收益差距约1.1%（6.17% vs 7.29%）
   - 但逆向策略有情绪套利能力，在震荡市中可能表现更好

2. **熔断抄底配置表现最佳**
   - 放宽极度恐慌阈值（35 vs 25）捕获更多抄底机会
   - 极度恐慌时90%仓位买入
   - 提高逆向权重上限（4.0）让浮亏标的获得更多加仓

3. **中性区间买入效果有限**
   - 波段配置在中性区间也买入，但收益反而最低
   - 建议中性买入比例设为0%

4. **交易频率与收益的关系**
   - 极致逆向（22次买入）vs 波段（44次买入）
   - 前者收益更高（69.73% vs 58.84%）
   - 过度交易可能侵蚀收益

## 回测代码修复

### 修复内容

1. **FGI月度聚合逻辑**
   - 原代码只比较日期天数，可能选错月末数据
   - 修复：先排序再取每月最后一条

2. **交易触发条件**
   - 原代码仅在区间变化时触发，错过机会
   - 修复：增加冷却期机制，3个月后可重新评估

3. **买入百分比计算**
   - 原代码显示比例不正确
   - 修复：使用买入前现金作为基准

### 修改文件
- `src/backtest.rs`: 核心回测逻辑

## 使用方法

```bash
# 运行默认参数对比
mns backtest

# 使用最优配置
mns backtest run --config .agents/skills/mns-backtest/data/config_circuit_breaker.toml

# 对比多个配置
mns backtest run --compare config1.toml,config2.toml,config3.toml
```

## 数据文件

- `monthly_multi_asset.csv`: 2016-2025月度数据（FGI、纳斯达克、沪深300、人民币黄金）
- `fgi_2016_2020.csv`: 逐日恐贪指数（高置信度）
- `fgi_2020_2025.csv`: 月度恐贪指数
- `hs300_monthly.csv`: 沪深300历史数据

## 结论

逆向策略在美股长牛市场中收益略低于买入持有，但提供了：
1. 更低的单笔风险（分批建仓）
2. 情绪套利能力（恐慌买入、贪婪卖出）
3. 适合有持续现金流入的投资者（定投增强）

建议在实际使用时：
- 采用熔断抄底配置
- 中性区间不买入
- 极度恐慌时果断重仓（80-90%）
- 设置合理冷却期（3个月）避免过度交易
