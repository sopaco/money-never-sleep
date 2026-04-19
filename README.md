# MNS - Market Neutral Strategist

逆向投资助手 CLI 工具，基于 CNN 恐贪指数监控市场情绪，结合持仓年化收益目标，每日生成买入/卖出建议。

## 安装

```bash
cargo build --release
# 二进制文件在 target/release/mns
```

## 快速开始

```bash
# 1. 初始化
mns init

# 2. 设置初始现金
mns cash set 100000

# 3. 添加资产
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
|------|------|
| `mns init` | 初始化配置和数据库 |
| `mns config [key] [value]` | 查看/修改配置项 |
| `mns cash` | 查看现金余额 |
| `mns cash set <amount>` | 设置现金余额 |
| `mns cash add <amount>` | 增加现金 |
| `mns portfolio` | 查看持仓概览 |
| `mns add <code> <name> <category>` | 新增资产 |
| `mns buy <code> <shares> <price>` | 记录买入 |
| `mns sell <code> <shares> <price>` | 记录卖出 |
| `mns price <code> [price]` | 查看/更新资产价格 |
| `mns sentiment` | 查看当前恐贪指数 |
| `mns report` | 生成今日策略报告 |
| `mns history [limit]` | 查看交易历史 |

## 配置

配置文件位于 `~/.mns/config.toml`，可调整：
- 止盈目标年化收益率（低线/高线）
- 资产配置比例（美股/A股/逆周期）
- 恐贪指数阈值
- 买入/卖出比例

## 数据存储

- 配置: `~/.mns/config.toml`
- 数据库: `~/.mns/mns.db` (SQLite)
- 报告: `{report_output_dir}/{YYYY-MM-DD}.txt`
