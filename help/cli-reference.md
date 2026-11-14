# CLI 命令参考

## 账本类

| 命令 | 作用 | 示例 |
|---|---|---|
| `mns init` | 初始化配置与数据库 | `mns init` |
| `mns add` | 登记新标的 | `mns add 018966 "纳指100ETF" us_stocks` |
| `mns remove` | 移除标的 | `mns remove 018966` |
| `mns buy` | 登记已成交的买入 | `mns buy 018966 1000 1.234` |
| `mns sell` | 登记已成交的卖出 | `mns sell 018966 500 1.456` |
| `mns cash deposit` | 外部入金（XIRR 口径） | `mns cash deposit 10000` |
| `mns cash withdraw` | 外部出金 | `mns cash withdraw 2000` |
| `mns cash set` | 纠错用，不应记入金 | `mns cash set 5000` |
| `mns price` | 手动更新某标的的价格 | `mns price 018966` |

## 报告与计划类

| 命令 | 作用 | 说明 |
|---|---|---|
| `mns today` | 刷价并生成今日报告 | 决策日给出完整调仓建议 |
| `mns report` | 生成报告 | `--decide` 强制按决策日口径 |
| `mns plan` | 定投分配 | `--monthly 3000`，只买不卖 |
| `mns guard` | 冲动冷静期 | 过热区间建议延迟一个交易日 |
| `mns market` | 市场概况 | 只读 |

## 绩效与流水类（只读）

| 命令 | 作用 |
|---|---|
| `mns portfolio` | 单只标的的持仓、成本和年化收益 |
| `mns performance` | 组合 XIRR 和实际最大回撤 |
| `mns history` | 翻看交易流水 |
| `mns analyze` | 账本分析 |

## 配置类

| 命令 | 作用 |
|---|---|
| `mns config params` | 查看全部可调参数和默认值 |
| `mns config set` | 修改单个参数 |

## 记账示例

```bash
mns cash deposit 10000        # 外部入金，按 XIRR 口径记账
mns buy 018966 1000 1.234     # 登记一笔已成交的买入
mns sell 018966 500 1.456     # 登记一笔已成交的卖出
mns cash withdraw 2000        # 外部出金
```

持仓、成本、现金和流水会保持一致，现金不足或卖出超过持仓会直接报错。`cash set` 只用来纠错，不应该拿来记入金。
