# MNS - Market Neutral Strategist

逆向投资助手 CLI 工具，基于 CNN 恐贪指数监控市场情绪，结合持仓年化收益目标，每日生成买入/卖出建议。

## 安装

```bash
cargo build --release
# 二进制文件在 target/release/mns.exe (Windows) / target/release/mns (Linux/macOS)
```

## 快速开始

```bash
# 1. 初始化
mns init

# 2. 设置初始现金
mns cash set 100000

# 3. 添加资产（类别: us_stocks / cn_stocks / counter_cyclical）
mns add QQQ "纳指100" us_stocks
mns add 512890 "红利低波" cn_stocks
mns add GLD "黄金" counter_cyclical

# 4. 记录买入
mns buy QQQ 50 380.00

# 5. 更新当前价格
mns price QQQ 420.00

# 6. 查看持仓
mns portfolio

# 7. 查看恐贪指数
mns sentiment

# 8. 生成策略报告
mns report
```

## 命令列表

| 命令 | 说明 |
:|------|------|
| `mns init` | 初始化配置和数据库 |
| `mns config [key] [value]` | 查看/修改配置项（支持 dot-path 如 `thresholds.fear`） |
| `mns cash` | 查看现金余额 |
| `mns cash set <amount>` | 设置现金余额 |
| `mns cash add <amount>` | 增加现金（如年度资金流入） |
| `mns portfolio` | 查看持仓概览（含年化收益、绝对收益） |
| `mns add <code> <name> <category>` | 新增资产（类别: us_stocks / cn_stocks / counter_cyclical） |
| `mns buy <code> <shares> <price>` | 记录买入（自动更新加权平均成本，扣减现金） |
| `mns sell <code> <shares> <price>` | 记录卖出（自动更新份额，回收现金） |
| `mns price <code> [price]` | 查看/更新资产当前价格 |
| `mns sentiment` | 查看当前恐贪指数（并保存快照） |
| `mns report` | 生成今日策略报告（输出到终端和文件） |
| `mns history [limit]` | 查看交易历史（默认 20 条） |

## 配置

配置文件位于 `~/.mns/config.toml`，可通过 `mns config <key> <value>` 动态修改。

### settings

| 配置项 | 默认值 | 说明 |
|--------|--------|------|
| `annualized_target_low` | 10.0 | 年化止盈低线（%） |
| `annualized_target_high` | 15.0 | 年化止盈高线（%） |
| `min_holding_days` | 30 | 最小持仓天数（不足时不计算年化，避免短期失真） |
| `min_absolute_profit_days` | 90 | 绝对收益止盈最小持仓天数（防止短期波动触发止盈） |
| `max_contrarian_weight` | 2.0 | 逆向加权最大权重（防止向单亏损标的过度集中） |
| `report_output_dir` | `./reports` | 报告输出目录 |

### allocation

| 配置项 | 默认值 | 说明 |
|--------|--------|------|
| `us_stocks` | 50 | 美股占比（%） |
| `cn_stocks` | 35 | A股占比（%） |
| `counter_cyclical` | 15 | 逆周期资产占比（%） |

### thresholds

| 配置项 | 默认值 | 说明 |
|--------|--------|------|
| `extreme_fear` | 25 | 极度恐慌阈值（指数 < 25） |
| `fear` | 45 | 恐慌阈值（指数 < 45） |
| `neutral` | 55 | 中性阈值（指数 < 55） |
| `greed` | 75 | 贪婪阈值（指数 ≥ 75 为极度贪婪） |

### buy_ratio

| 区间 | 默认值 | 说明 |
|------|--------|------|
| 极度恐慌 | 50% | 极度恐慌时投入可用资金的 50% |
| 恐慌 | 30% | 恐慌时投入 30% |
| 中性 | 20% | 中性时投入 20% |
| 贪婪 | 0% | 贪婪时暂停买入 |
| 极度贪婪 | 0% | 极度贪婪时暂停买入 |

## 策略逻辑

### 年化收益计算

```
正收益: annualized = (current / cost) ^ (365 / days) - 1
负收益: annualized = (current / cost - 1) / years
```

负收益使用简单年化，避免复利公式将小幅亏损（如 -5%）严重放大为夸张的负年化（如 -46%）。

### 卖出建议

触发条件：恐贪指数 ≥ 45（中性及以上）。

**决策矩阵**：

| 市场情绪 | 年化 ≥ 15% | 10% ≤ 年化 < 15% | 年化 < 10% |
|----------|-----------|-----------------|-----------|
| 极度贪婪 (≥75) | 减仓 50% | 减仓 30% | 减仓 20% |
| 贪婪 (55-74) | 减仓 40% | 减仓 20% | 持有 |
| 中性 (45-54) | 减仓 30% | 持有 | 持有 |
| 恐慌/极度恐慌 | 持有 | 持有 | 持有 |

**绝对收益二级触发**：绝对收益 ≥ 30% 且持仓 ≥ 90 天时，在中性及以上环境触发止盈：
- 极度贪婪：减仓 15%
- 中性/贪婪：减仓 10%

卖出建议按绝对收益从高到低排序，优先卖出盈利最多的标的以锁定利润。

### 买入建议

1. 可用现金 = 现金余额 + 卖出回收金额（买卖互感知）
2. 建议投入金额 = 可用现金 × 区间买入比例
3. 按资产配置比例拆分到美股/A股/逆周期
4. 各类别内按逆向加权分配：
   - 权重 = min(max_contrarian_weight, max(1.0, cost / current))
   - 浮亏越多权重越高（逆向加仓），但有上限防止单标的过度集中
5. **风险联动**：浮亏 ≥ 30% 的标的被排除在逆向加仓之外

### 风险警告

持仓浮亏 ≥ 20% 时触发，根据市场情绪给出差异化建议：

| 市场情绪 | 建议 |
|----------|------|
| 恐慌及以下 | 可能是加仓机会——若基本面未恶化，可考虑逆向加仓 |
| 中性 | 审视基本面是否恶化 |
| 贪婪及以上 | 紧急审视——市场普涨时该标的逆势下跌，可能存在结构性问题 |

**注意**：风险警告仅提示不自动卖，且浮亏 ≥ 30% 的标的不会出现在买入建议中。

## 数据存储

- 配置: `~/.mns/config.toml`
- 数据库: `~/.mns/mns.db` (SQLite)
- 报告: `{report_output_dir}/{YYYY-MM-DD}.txt`
