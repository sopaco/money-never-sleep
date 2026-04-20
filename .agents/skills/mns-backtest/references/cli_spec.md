# MNS CLI 命令规范

## 工具概述

MNS (Market Neutral Strategist) 是一款个人逆向投资 CLI 工具，通过监控 CNN 恐贪指数，结合持仓成本与年化收益目标，每日生成买入/卖出建议，输出文本报告。

## 数据存储

- 配置: `~/.mns/config.toml`
- 数据库: `~/.mns/mns.db` (SQLite)
- 报告输出: `{report_output_dir}/{YYYY-MM-DD}.txt`

## 数据库 Schema

```sql
-- 现金余额 (单行表)
CREATE TABLE cash (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    balance REAL NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- 持仓
CREATE TABLE positions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    asset_code TEXT NOT NULL UNIQUE,
    asset_name TEXT NOT NULL,
    category TEXT NOT NULL,  -- us_stocks / cn_stocks / counter_cyclical
    shares REAL NOT NULL DEFAULT 0,
    cost_price REAL NOT NULL DEFAULT 0,
    current_price REAL,
    first_buy_date TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- 交易记录
CREATE TABLE transactions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    type TEXT NOT NULL CHECK (type IN ('buy', 'sell')),
    asset_code TEXT NOT NULL,
    shares REAL NOT NULL,
    price REAL NOT NULL,
    amount REAL NOT NULL,
    tx_date TEXT NOT NULL,
    note TEXT
);

-- 恐贪指数快照
CREATE TABLE fear_greed_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    score REAL NOT NULL,
    rating TEXT NOT NULL,
    snapshot_date TEXT NOT NULL,
    previous_close REAL,
    previous_1_week REAL,
    previous_1_month REAL,
    previous_1_year REAL,
    fetched_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

## CLI 命令列表

| 命令 | 说明 |
|------|------|
| `mns init [-f, --force]` | 初始化配置和数据库（已有数据会提示确认，--force跳过确认） |
| `mns config [key] [value]` | 查看/修改配置项 |
| `mns cash` | 查看现金余额 |
| `mns cash set <amount>` | 设置现金余额 |
| `mns cash add <amount>` | 增加现金 |
| `mns portfolio` | 查看持仓概览（含年化收益） |
| `mns add <code> <name> <category>` | 新增资产到持仓池 |
| `mns buy <code> <shares> <price>` | 记录买入 |
| `mns sell <code> <shares> <price>` | 记录卖出 |
| `mns price <code> [price]` | 更新资产当前价格 |
| `mns sentiment` | 查看当前恐贪指数 |
| `mns report` | 生成今日策略报告 |
| `mns history` | 查看交易历史 |

## 报告模板

```
=================================================================
  逆向投资助手 - 每日策略报告
  2026-04-19
=================================================================

【市场情绪】
  CNN 恐贪指数: 68.09 (贪婪)
  前日收盘: 62.20 | 周环比: 36.57 → 68.09

【账户概览】
  现金余额: ¥100,000.00
  持仓市值: ¥250,000.00
  总资产:   ¥350,000.00

  持仓明细:
  代码   名称      份额   成本价   现价    年化收益
  QQQ    纳指100   100    350.00  420.00  +18.5%
  512890 红利低波  1000   1.12    1.25    +11.2%
  GLD    黄金      50     170.00  165.00  -3.2%

【卖出建议】 ⚠ 市场贪婪，检查止盈
  ▸ QQQ — 年化 18.5% ≥ 15%
    建议: 减仓 40%，卖出 40 份，回收 ¥16,800.00

【买入建议】
  当前市场"贪婪"，建议暂停买入。

【资金分配预案】
  若市场回调至不同区间：
  · 恐慌   (< 45): 投入 ¥30,000 (30%)
  · 极度恐慌 (< 25): 投入 ¥50,000 (50%)

=================================================================
```

## 卖出决策二维矩阵

```
情绪区间 × 年化收益 → 减仓比例

              年化≥15%    10-15%     <10%
极度贪婪(>75)   50%        30%       20%
贪婪(55-74)    40%        20%       0%
中性(45-55)    30%        0%        0%
```

## 浮亏警告

若持仓浮亏超 20%（现价/成本价 < 0.8），标注风险警告，但不自动建议卖出——逆向策略下浮亏可能是加仓机会。
