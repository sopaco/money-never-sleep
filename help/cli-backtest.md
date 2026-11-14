# 回测与历史审计

## `mns backtest`：实盘审计

默认跑的是**实盘审计**：用和 `mns report` 同源的信号、真实基金净值、真实通道成本与结算规则，输出账户镜像和一组对照实验。

```bash
mns backtest update-data              # 重建本地日频数据集（需要 Python 3）
mns backtest                          # 完整审计
mns backtest mirror                   # 严格执行策略和你的真实流水对比
mns backtest asset --dataset hs300    # 单资产对照实验
```

| 子命令 | 用途 |
|---|---|
| `update-data` | 重建本地日频数据集 |
| （默认） | 完整审计 |
| `mirror` | 账户镜像：严格执行策略 vs 你的真实流水 |
| `asset` | 单资产对照实验 |
| `legacy` | 更早的月频引擎，已降级，只适合查历史 |

> [!WARNING]
> `mns backtest legacy` 是更早的月频引擎，已经降级，不要拿它指导实盘。

## 建议的审计节奏

每季度一次：

1. `mns backtest update-data` 更新数据集；
2. `mns backtest mirror` 对比「如果严格执行策略」和「实际操作」的差异。

差异大不代表你错了——策略允许你因为用钱而暂停——但持续偏大的差异值得想想原因。

## 给 AI Agent 用

仓库里带了两套面向 Agent 的 Skill：

| Skill | 用途 |
|---|---|
| `mns-advisor` | 处理「什么时候买、什么时候卖、为什么不动」：判定触发器、联网核验限购和费率、输出可审计的决策卡 |
| `mns-backtest` | 回测、数据集重建、参数对照和结果解读 |
