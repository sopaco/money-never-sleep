# MNS 策略参数参考

本文档详细说明 MNS 逆向投资策略的核心参数，包括阈值定义、买入/卖出比例配置、止盈线设置等。这些参数通过 `mns config` 命令动态调整。

---

## 配置层级

MNS 配置采用分层结构：

```toml
[database]
path = "~/.mns/mns.db"

[settings]
min_holding_days = 30          # 最小持有天数（年化收益计算用）
annualized_target_low = 10.0   # 年化止盈线下限
annualized_target_high = 15.0  # 年化止盈线上限
report_output_dir = "./reports"

[thresholds]
extreme_fear = 25      # 极度恐慌阈值
fear = 45              # 恐慌阈值
neutral = 55           # 中性阈值
greed = 75             # 贪婪阈值
extreme_greed = 100    # 极度贪婪阈值（通常固定为 100）

[buy_ratio]
extreme_fear = 0.50    # 极度恐慌时买入比例（可用现金的 50%）
fear = 0.30            # 恐慌时买入比例
neutral = 0.20         # 中性时买入比例
greed = 0.00           # 贪婪时不买入
# extreme_greed 默认 0.00（通常不买入）

[sell_ratio]
# 格式: ("情绪", "价格位置") = 卖出比例
# 价格位置定义:
#   above_high  - 当前价格高于阻力位（盈利状态）
#   between     - 价格在支撑位和阻力位之间
#   below_low   - 当前价格低于支撑位（浮亏状态）

"sell_ratio.(\"extreme_greed\",\"above_high\")" = 0.50
"sell_ratio.(\"extreme_greed\",\"between\")" = 0.30
"sell_ratio.(\"extreme_greed\",\"below_low\")" = 0.20
"sell_ratio.(\"greed\",\"above_high\")" = 0.40
"sell_ratio.(\"greed\",\"between\")" = 0.20
"sell_ratio.(\"greed\",\"below_low\")" = 0.00
# neutral/fear/extreme_fear 默认无卖出（值为 0.00）
```

---

## 恐贪指数区间划分

CNN Fear & Greed Index 是一个 0-100 的指数，0 表示极度恐慌（Extreme Fear），100 表示极度贪婪（Extreme Greed）。

### 默认区间配置

| 区间名称 | 指数范围 | 含义 | 买入比例 |
|---------|---------|------|---------|
| Extreme Fear | 0-25 | 市场极度恐慌，逆向买入信号强 | 50% |
| Fear | 25-45 | 市场恐慌，适度买入 | 30% |
| Neutral | 45-55 | 中性，小额买入或观望 | 20% |
| Greed | 55-75 | 市场贪婪，谨慎持有 | 0% |
| Extreme Greed | 75-100 | 市场极度贪婪，考虑减仓 | 0% |

### 调整阈值

通过修改 `thresholds` 可改变区间边界：

```bash
# 将贪婪阈值从 75 降至 70，使贪婪区间更早触发
mns config thresholds.greed 70

# 将极度恐慌阈值从 25 提升至 30，减少极端情况下的买入频率
mns config thresholds.extreme_fear 30
```

**注意**: 区间必须保持连续性，建议:
- `extreme_fear < fear < neutral < greed < extreme_greed`
- 通常 `extreme_greed` 固定为 100

---

## 买入比例矩阵 (`buy_ratio`)

定义在不同情绪区间下，用多少比例的可用现金进行买入。

### 参数说明

- **可用现金**: `cash_balance + 卖出回笼资金`
- **分配方式**: 按 contrarian 权重（浮亏资产权重更高）分配到各资产
- **单次买入上限**: 单个资产不超过该比例 × 总可用现金

### 默认配置

| 情绪区间 | 买入比例 | 适用场景 |
|---------|---------|---------|
| extreme_fear | 0.50 | 恐慌极致，重仓抄底 |
| fear | 0.30 | 恐慌情绪，积极建仓 |
| neutral | 0.20 | 中性市场，小额布局 |
| greed | 0.00 | 贪婪时暂停买入 |
| extreme_greed | 0.00 | 极度贪婪时完全不买入 |

### 调优示例

```bash
# 策略1: 极度贪婪时也可能小幅买入（左侧布局）
mns config buy_ratio.extreme_greed 0.10

# 策略2: 中性区间也暂停买入，只在明确的恐慌区间出手
mns config buy_ratio.neutral 0.00

# 策略3: 极端恐慌时全仓出击
mns config buy_ratio.extreme_fear 0.70
```

---

## 卖出比例矩阵 (`sell_ratio`)

定义在特定情绪区间和价格位置下的卖出比例。卖出逻辑基于 **双准则**:

1. **年化收益止盈**: 当持仓年化收益率达到 `annualized_target_high` 且在对应情绪区间
2. **绝对收益**: 当持仓绝对收益 ≥ 30% 时，也可考虑卖出（即使年化收益未达阈值）

### 价格位置定义

- `above_high`: 当前价格高于某个阻力位（通常意味着盈利）
- `between`: 价格在支撑位和阻力位之间（中性区域）
- `below_low`: 当前价格低于支撑位（浮亏状态）

实际实现中，"价格位置" 是通过比较当前价格与成本价来判断的：
- `above_high`: 当前价格显著高于成本（如 +10% 以上）
- `between`: 价格在成本附近浮动（如 -10% ~ +10%）
- `below_low`: 当前价格显著低于成本（如 -10% 以下）

### 默认卖出矩阵

| 情绪区间 | above_high | between | below_low |
|---------|-----------|---------|-----------|
| extreme_greed | 50% | 30% | 20% |
| greed | 40% | 20% | 0% |
| neutral | 0% | 0% | 0% |
| fear | 0% | 0% | 0% |
| extreme_fear | 0% | 0% | 0% |

**解读**:
- 只在 `Greed` 和 `Extreme Greed` 区间考虑卖出
- 极度贪婪时卖出更激进（above_high 卖出 50%）
- 浮亏状态（below_low）即使在贪婪区间也只部分卖出或持有

### 调优示例

```bash
# 极度贪婪且大幅盈利时，卖出 70%
mns config sell_ratio.("extreme_greed","above_high") 0.70

# 贪婪区间小幅盈利时也卖出 30%
mns config sell_ratio.("greed","between") 0.30

# 中性区间也开始部分止盈（保守策略）
mns config sell_ratio.("neutral","above_high") 0.20
```

---

## 止盈线参数 (`settings`)

### `annualized_target_low` (默认 10.0)

年化收益率的下限阈值。当持仓年化收益 ≥ 此值且处于可卖出的情绪区间时，触发部分卖出建议。

**单位**: 百分比（0-100）

### `annualized_target_high` (默认 15.0)

年化收益率的上限阈值。当持仓年化收益 ≥ 此值时，建议卖出比例达到最大。

**调优示例**:

```bash
# 保守止盈：年化 8% 就开始卖
mns config annualized_target_low 8.0
mns config annualized_target_high 12.0

# 激进止盈：年化 20% 才开始卖
mns config annualized_target_low 15.0
mns config annualized_target_high 25.0
```

### `min_holding_days` (默认 30)

最小持有天数。持仓天数小于此值时，不计算年化收益率（显示为 N/A），避免短期波动误导。

**建议**: 
- 短线策略可降至 7-15 天
- 长线投资可提升至 60-90 天

---

## 买入资金分配: Contrarian 权重

MNS 的买入资金不是平均分配到各资产，而是采用 **逆向权重**:

```
weight_i = max(1.0, cost_price_i / current_price_i)
```

### 权重计算示例

假设可用现金 100,000 元，有两个资产可供买入:

| 资产 | 成本价 | 当前价 | 浮亏/浮盈 | 权重计算 | 权重 |
|------|--------|--------|----------|---------|------|
| QQQ  | 450.00 | 460.50 | +2.3%    | max(1.0, 450/460.5) = 1.0 | 1.0 |
| SH600| 12.30  | 11.80  | -4.1%    | max(1.0, 12.3/11.8) = 1.043 | 1.043 |

**分配结果**:
- QQQ: `100000 × (1.0 / 2.043) × buy_ratio(fear) = 100000 × 0.489 × 0.30 = 14,670 元`
- SH600: `100000 × (1.043 / 2.043) × buy_ratio(fear) = 100000 × 0.511 × 0.30 = 15,330 元`

浮亏资产获得更高权重，符合 "越跌越买" 的逆向逻辑。

---

## 卖出决策双准则详解

### 准则1：年化收益率 + 情绪区间

当同时满足以下条件时，触发基于年化收益的卖出建议:

1. 当前情绪区间在 `sell_ratio` 矩阵中定义了卖出比例（如 `greed` 或 `extreme_greed`）
2. 持仓年化收益率 ≥ `annualized_target_low`
3. 根据年化收益率所处的区间（low/high）和情绪区间，查出对应的卖出比例

**卖出比例插值**:
```rust
// 伪代码
if annualized < annualized_target_low:
    ratio = 0.0
else if annualized >= annualized_target_high:
    ratio = sell_ratio[emotion][price_position]
else:
    // 线性插值
    t = (annualized - low) / (high - low)
    ratio = t * sell_ratio[emotion][price_position]
```

### 准则2：绝对收益 ≥ 30%

当持仓满足以下条件时，即使年化收益率未达阈值，也建议卖出:

```
absolute_return = (current_price - cost_price) / cost_price
if absolute_return >= 0.30:
    建议卖出比例 = sells 矩阵中对应值（根据情绪和价格位置）
```

**目的**: 锁定长期盈利，避免 "坐过山车"。

---

## 完整参数调优工作流

### 1. 查看当前配置

```bash
# 查看所有配置
mns config

# 查看特定分组
mns config buy_ratio
mns config sell_ratio
mns config thresholds
```

### 2. 基于回测结果调整

参考 `回测结果`（见 `backtest_runner.py` 输出）调整:

- 如果回测显示 "中性区间买入过多"，降低 `buy_ratio.neutral` 到 0.10 或 0.00
- 如果回测显示 "极度恐慌时收益高但仓位不足"，提升 `buy_ratio.extreme_fear` 到 0.60-0.70
- 如果回测显示 "极度贪婪时卖出不够及时"，提升 `sell_ratio.("extreme_greed","above_high")` 到 0.60-0.70

### 3. 小步快跑验证

每次只调整 1-2 个参数，观察后续 `report` 输出和实际持仓表现。

```bash
# 调整 - 降低中性买入
mns config buy_ratio.neutral 0.10

# 观察 1 周后的 report 变化
# 如果感觉买入频率下降明显，可继续微调
```

### 4. 保存参数快照

将满意配置备份到文件:

```bash
mns config > config_backup_2025-06-16.toml
```

---

## 参数命名空间说明

### dot-path 语法

MNS 支持 Rust TOML crate 的 dot-path 访问:

```bash
# 访问 thresholds.fear
mns config thresholds.fear

# 访问 sell_ratio 的元组键
mns config sell_ratio.("greed","above_high")
```

### 特殊字符转义

当键名包含引号或括号时，需转义:

```bash
# 正确的写法（外层引号，内层转义）
mns config 'sell_ratio.("greed","above_high")' 0.50
```

在 shell 中，建议用单引号包裹整个参数以避免转义问题。

---

## 风险提示

- **参数调优需结合实际市场环境**: 回测结果是历史数据，不代表未来表现
- **避免频繁调整**: 每次调整后至少观察 1-2 个月的实盘表现
- **极端行情测试**: 回测中已包含 2020 年疫情、2022 年熊市等极端情况，但实际黑天鹅可能更严重

---

## 进阶话题

### 动态阈值调整（未来方向）

当前阈值为静态配置。未来可考虑:
- 基于 VIX 指数动态调整 `thresholds`
- 根据账户总资产规模调整 `buy_ratio`（规模越大，单次买入比例越低）
- 基于持仓集中度调整 `sell_ratio`（单一资产占比过高时优先卖出）

### 多因子情绪模型

当前仅使用 CNN Fear & Greed Index。可扩展:
- 加入 A 股情绪指标（沪深 300 波动率、涨跌停家数等）
- 加入技术指标（RSI、MACD）作为补充过滤器

---

**文档版本**: v0.5.6 | **更新日期**: 2026-04-20
