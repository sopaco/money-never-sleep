# MNS 数据能力说明

## 概述

MNS CLI 集成多种金融数据源，提供全面的市场分析、个股研究和投资组合管理能力。本文档详细说明各数据源的能力范围、更新频率和使用限制。

## 数据源概览

| 数据源 | 类型 | 覆盖范围 | 更新频率 | 主要用途 |
|--------|------|----------|----------|----------|
| Yahoo Finance | 免费 | 全球股票、ETF、指数 | 15-20 分钟延迟 | 报价、历史数据、基本面 |
| CNN Fear & Greed | 免费 | 美股市场情绪 | 每日 | 恐贪指数 |
| Alpha Vantage | 混合 | 全球股票、外汇 | 实时/延迟 | 技术指标、基本面 |
| IEX Cloud | 混合 | 美股为主 | 实时 | 实时报价、基本面 |

## 数据能力矩阵

### 1. 市场数据

#### 1.1 指数数据
- **覆盖指数**：
  - 美股：S&P 500, NASDAQ, DOW JONES, RUSSELL 2000
  - 国际：FTSE 100, DAX, NIKKEI 225, HSI, SSE
  - VIX 波动率指数

- **数据字段**：
  ```
  - price: 当前价格
  - change: 涨跌点数
  - change_pct: 涨跌幅百分比
  - open: 开盘价
  - high: 日内最高
  - low: 日内最低
  - volume: 成交量
  - timestamp: 数据时间戳
  ```

- **更新频率**：15-20 分钟延迟（免费源）

#### 1.2 行业数据
- **行业分类**：基于 GICS 标准（11 个一级行业）
  - Technology（科技）
  - Healthcare（医疗保健）
  - Financials（金融）
  - Energy（能源）
  - Consumer Discretionary（非必需消费）
  - Consumer Staples（必需消费）
  - Industrials（工业）
  - Materials（原材料）
  - Utilities（公用事业）
  - Real Estate（房地产）
  - Communication Services（通信服务）

- **数据字段**：
  ```
  - sector_name: 行业名称
  - change_pct: 行业整体涨跌幅
  - leading_stock: 领涨股
  - lagging_stock: 领跌股
  - volume_change: 成交量变化
  ```

#### 1.3 恐贪指数
- **数据源**：CNN Fear & Greed Index
- **范围**：0-100
  - 0-24: Extreme Fear（极度恐惧）
  - 25-44: Fear（恐惧）
  - 45-55: Neutral（中性）
  - 56-75: Greed（贪婪）
  - 76-100: Extreme Greed（极度贪婪）

- **构成指标**（7 个维度）：
  1. Market Momentum（市场动量）
  2. Stock Price Strength（股价强度）
  3. Stock Price Breadth（股价广度）
  4. Put and Call Options（看涨看跌期权）
  5. Market Volatility（市场波动率 VIX）
  6. Safe Haven Demand（避险需求）
  7. Junk Bond Demand（垃圾债券需求）

- **更新频率**：每日收盘后更新

### 2. 个股数据

#### 2.1 实时报价
- **数据字段**：
  ```
  - symbol: 股票代码
  - price: 当前价格
  - bid: 买一价
  - ask: 卖一价
  - bid_size: 买一量
  - ask_size: 卖一量
  - volume: 成交量
  - avg_volume: 平均成交量（30日）
  - market_cap: 市值
  - pe_ratio: 市盈率
  - week52_high: 52周最高
  - week52_low: 52周最低
  ```

#### 2.2 历史数据
- **时间范围**：最长 10 年历史数据
- **数据频率**：日线（OHLCV）
- **调整方式**：后复权（考虑分红、拆股）

- **数据字段**：
  ```
  - date: 日期
  - open: 开盘价
  - high: 最高价
  - low: 最低价
  - close: 收盘价
  - adj_close: 复权收盘价
  - volume: 成交量
  ```

#### 2.3 基本面数据

##### 估值指标
```
- pe_ratio: 市盈率（P/E）
- pb_ratio: 市净率（P/B）
- ps_ratio: 市销率（P/S）
- ev_ebitda: 企业价值/EBITDA
- peg_ratio: PEG 比率
- dividend_yield: 股息率
- earnings_yield: 收益率
```

##### 财务指标
```
- revenue: 营业收入
- revenue_growth: 营收增长率
- gross_margin: 毛利率
- operating_margin: 营业利润率
- net_margin: 净利润率
- roe: 净资产收益率
- roa: 总资产收益率
- debt_to_equity: 资产负债率
- current_ratio: 流动比率
- quick_ratio: 速动比率
```

##### 现金流指标
```
- operating_cash_flow: 经营现金流
- free_cash_flow: 自由现金流
- capex: 资本支出
- dividend_per_share: 每股股利
```

#### 2.4 技术指标

详见 [TECHNICAL_INDICATORS.md](TECHNICAL_INDICATORS.md)

支持的指标类型：
- 趋势指标：MA, EMA, MACD
- 动量指标：RSI, Stochastic, CCI
- 波动率指标：Bollinger Bands, ATR
- 成交量指标：OBV, VWAP
- 支撑阻力：Pivot Points, Fib Retracement

### 3. 投资组合数据

#### 3.1 持仓数据
- **存储位置**：SQLite 本地数据库（`~/.mns/mns.db`）
- **数据字段**：
  ```
  - symbol: 股票代码
  - shares: 持股数量
  - cost_price: 成本价
  - purchase_date: 买入日期
  - current_price: 当前价格
  - market_value: 持仓市值
  - unrealized_pnl: 浮动盈亏
  - unrealized_pnl_pct: 浮动盈亏百分比
  - annualized_return: 年化收益率
  - holding_days: 持有天数
  ```

#### 3.2 交易记录
- **记录类型**：
  - BUY: 买入
  - SELL: 卖出
  - DIVIDEND: 分红
  - SPLIT: 拆股

- **数据字段**：
  ```
  - transaction_id: 交易ID
  - symbol: 股票代码
  - transaction_type: 交易类型
  - date: 交易日期
  - shares: 股数
  - price: 成交价格
  - amount: 成交金额
  - commission: 手续费
  - notes: 备注
  ```

#### 3.3 历史快照
- **快照频率**：每日收盘后自动保存
- **用途**：计算历史收益、回撤、Sharpe 比率等

- **数据字段**：
  ```
  - snapshot_date: 快照日期
  - total_value: 组合总市值
  - cash_balance: 现金余额
  - positions: 持仓快照（JSON）
  - daily_pnl: 当日盈亏
  - benchmark_value: 基准指数值（SPY）
  ```

### 4. 策略数据

#### 4.1 建议生成依据

MNS 基于以下数据生成交易建议：

1. **恐贪指数状态** → 确定市场情绪区域
2. **持仓年化收益率** → 决定是否卖出
3. **持仓浮亏程度** → 决定是否补仓
4. **可用现金** → 确定买入金额上限
5. **技术指标信号** → 确认买卖时机

#### 4.2 策略阈值配置

配置文件路径：`~/.mns/config.toml`

```toml
[thresholds]
# 恐贪指数区域划分
fear = 45
extreme_fear = 25
greed = 55
extreme_greed = 75

[buy_ratio]
# 各区域的买入比例（占总现金百分比）
extreme_fear = 30.0
fear = 20.0
neutral = 10.0
greed = 5.0
extreme_greed = 0.0

[sell_ratio]
# 各区域的卖出比例（占持仓百分比）
extreme_fear = 0.0
fear = 0.0
neutral = 20.0
greed = 30.0
extreme_greed = 50.0

[settings]
min_holding_days = 30    # 最小持有天数（用于年化收益计算）
risk_free_rate = 0.05    # 无风险利率（5%）
```

## 数据延迟与限制

### 免费数据源限制

| 限制项 | 具体限制 | 影响 |
|--------|----------|------|
| 报价延迟 | 15-20 分钟 | 无法获取实时价格 |
| API 调用限制 | 每分钟 5 次，每天 500 次 | 批量查询需控制频率 |
| 历史数据 | 单次最多 1000 条 | 长周期需分批获取 |
| 并发限制 | 单连接顺序请求 | 无法并行大批量查询 |

### 数据准确性说明

1. **价格数据**：延迟报价可能与实际成交价有差异
2. **基本面数据**：更新依赖财报发布周期（季度/年度）
3. **技术指标**：基于历史数据计算，存在滞后性
4. **恐贪指数**：每日更新，盘中无实时数据

### 建议使用方式

1. **避免高频查询**：设置合理的查询间隔（≥1分钟）
2. **缓存历史数据**：本地存储已查询的数据
3. **交易日盘后查询**：确保数据已更新
4. **重要决策验证**：使用多个数据源交叉验证

## 数据缓存策略

MNS 实现以下缓存机制减少 API 调用：

### 内存缓存（运行时）
```
缓存项: 实时报价
TTL: 60 秒
刷新: 每次查询自动检查过期
```

### 文件缓存（持久化）
```
缓存项: 历史数据、基本面数据
路径: ~/.mns/cache/
格式: JSON 文件
TTL: 24 小时（历史数据）、7 天（基本面）
```

### 快照缓存
```
缓存项: 每日组合快照
路径: ~/.mns/mns.db (SQLite)
触发: 每日收盘后首次运行
```

## 扩展数据源

### 未来计划支持

1. **付费数据源**
   - Bloomberg API（实时报价、专业级数据）
   - Refinitiv（全球市场、另类数据）
   - FactSet（深度基本面数据）

2. **另类数据**
   - 社交媒体情绪（Twitter, Reddit）
   - 新闻情感分析
   - 卫星图像数据
   - 信用卡消费数据

3. **加密货币**
   - CoinGecko API
   - Binance API
   - 链上数据

## 数据字段完整参考

详见命令文档 [commands.md](commands.md)：
- 市场数据字段（market 命令）
- 个股数据字段（analyze 命令）
- 组合数据字段（portfolio 命令）

---

**更新日期**: 2024-01
**版本**: 1.0.0