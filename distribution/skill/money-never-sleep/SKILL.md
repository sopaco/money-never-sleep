---
name: money-never-sleep
version: 0.5.7
description: |
  MNS (Money Never Sleep, Market Neutral Strategist) CLI skill for autonomous agents. Provides investment
  portfolio tracking, market sentiment analysis using CNN Fear & Greed Index, and strategy
  suggestion report generation. Use when managing a contrarian investment portfolio,
  generating buy/sell suggestions based on market sentiment thresholds, or producing
  daily strategy reports.

  NOTE: This tool provides strategy suggestions only - it does NOT connect to any broker APIs
  or execute actual trades. All trades must be manually executed by users on their brokerage platforms
  and then recorded via CLI commands.

  Triggers include: "管理投资组合", "生成策略建议", "获取市场报告", "查看持仓收益",
  "更新现金余额", "记录买入卖出", "查看恐贪指数", "投资组合再平衡", "MNS 报告", "回测策略"
license: MIT
compatibility: Requires Node.js 18+ or Bun.
metadata:
  {
    "openclaw":
      {
        "requires": { "anyBins": ["npx", "bunx", "npm", "bun"] },
        "source": "https://github.com/sopaco/money-never-sleep",
        "homepage": "https://github.com/sopaco/money-never-sleep",
        "author": "Sopaco",
        "os": ["darwin", "linux", "win32"]
      }
  }
  
---

# MNS CLI 投资管理 Skill

## 概述

本 skill 提供 MNS 逆向投资策略的 CLI 操作能力。MNS 是基于 CNN Fear & Greed Index 情绪指标的量化投资工具，通过 contrarian 策略在情绪极度恐慌时买入，极度贪婪时卖出，实现市场中性风格的长期投资。

### 安全声明

> - **源码公开**: 所有源码公开托管于 [GitHub](https://github.com/sopaco/money-never-sleep)
> - **无网络交易**: 不连接任何券商 API，所有数据存储在本地 SQLite 数据库
> - **无敏感权限**: 无需使用也不自动读取用户任何金融账户配置

## 核心能力

1. **投资组合管理**: 查看持仓、现金余额、资产配置、年化收益
2. **交易记录**: 记录买入/卖出操作，更新资产当前价格（手动输入或自动获取）
3. **策略建议**: 自动生成基于最新恐贪指数的买卖建议报告（仅建议，不执行交易）
4. **配置管理**: 查看和调整策略参数（阈值、买入/卖出比例、止盈线）
5. **历史查询**: 查看交易历史、价格更新记录
6. **策略回测**: 基于历史数据验证策略参数表现

> **重要说明**: 本工具仅提供策略建议和记录功能，不连接任何券商 API。
> 用户需自行在券商平台执行交易后，通过 CLI 记录交易结果。

## 快速开始

### 安装

```bash
# 通过 npm 安装（推荐）
npm install -g @never-sleeps/mns-cli

# 或通过 bun 安装
bun install -g @never-sleeps/mns-cli

# 或直接使用 npx（无需安装）
npx @never-sleeps/mns-cli --help
```

### 初始化

```bash
# 初始化配置文件和数据库
# 如果已有数据，会提示确认后再覆盖
mns init

# 使用 --force 跳过确认直接覆盖
mns init --force

# 设置初始现金
mns cash set 100000
```

### 添加资产到持仓池

```bash
mns add QQQ "纳指100" us_stocks
mns add SH600000 "浦发银行" cn_stocks
mns add GLD "黄金ETF" counter_cyclical
```

### 记录买入交易

```bash
mns buy QQQ 100 450.50
mns buy SH600000 500 12.30
```

### 查看持仓和策略建议

```bash
# 查看当前持仓（含年化收益）
mns portfolio

# 获取今日策略报告（基于最新恐贪指数）
mns report

# 查看当前恐贪指数
mns sentiment
```

### 更新价格和查看历史

```bash
# 手动更新单个资产价格
mns price QQQ 460.00

# 自动更新所有资产价格（需要网络）
mns update-prices

# 查看最近交易历史
mns history --limit 50

# 查看现金余额
mns cash
```

### 配置管理

```bash
# 查看所有配置
mns config

# 查看特定配置项（支持 dot-path 语法）
mns config thresholds.fear
mns config buy_ratio.extreme_fear

# 修改配置项（策略参数）
mns config thresholds.greed 75
mns config buy_ratio.fear 30.0
mns config sell_ratio.extreme_greed_target_high 60.0
```

### 策略回测

```bash
# 查看可调参数列表
mns backtest params

# 运行默认配置回测
mns backtest run

# 使用自定义配置回测
mns backtest run --config path/to/config.toml

# 对比多个配置
mns backtest run --compare config1.toml,config2.toml
```

## 数据存储

- **配置文件**: `~/.mns/config.toml`
- **数据库**: `~/.mns/mns.db`
- **报告输出**: `./reports/`（可通过 `settings.report_output_dir` 配置）

## 策略逻辑详解

### 情绪驱动的买入决策

买入比例基于恐贪指数区间：

| 恐贪区间 | 指数范围 | 买入比例 | 逻辑 |
|---------|---------|---------|------|
| Extreme Fear | 0-25 | 50% (默认) | 极度恐慌，重仓买入 |
| Fear | 25-45 | 30% (默认) | 恐慌，适度买入 |
| Neutral | 45-55 | 20% (默认) | 中性，小额买入 |
| Greed | 55-75 | 0% (默认) | 贪婪，不买入 |
| Extreme Greed | 75-100 | 0% (默认) | 极度贪婪，不买入 |

### 卖出决策（双准则）

卖出建议综合考虑：
1. **年化收益止盈**: 基于持有天数计算年化收益率，对照卖出矩阵
2. **绝对收益线**: 绝对收益 ≥ 30% 且持仓 ≥ 90 天时也可考虑卖出

卖出矩阵（按情绪区间和收益档位）：

| 情绪区间 | target_high | target_low | below_target |
|---------|-------------|------------|--------------|
| Extreme Greed | 50% | 30% | 20% |
| Greed | 40% | 20% | 0% |
| Neutral | 30% | 0% | 0% |
| Fear/Extreme Fear | 0% | 0% | 0% |

> 注：target_high = 年化收益 ≥ annualized_target_high（默认15%），target_low = 年化收益 ≥ annualized_target_low（默认10%）

### 买入资金分配：Contrarian 权重

可用现金按 contrarian 权重分配到各资产：

- **权重公式**: `weight = min(max_weight, max(1.0, cost_price / current_price))`
- **解释**: 浮亏资产（当前价格 < 成本价）获得更高权重，符合逆向抄底逻辑
- **上限控制**: `max_contrarian_weight`（默认 2.0）防止过度集中单一标的
- **浮盈资产**: 权重为 1.0（基线）

### 风险警告机制

当持仓浮亏 ≥ 20% 时触发风险警告，建议根据市场情绪差异化处理：

| 情绪环境 | 建议操作 |
|---------|---------|
| Extreme Fear/Fear | 可能是加仓机会 |
| Neutral | 审视基本面 |
| Greed/Extreme Greed | 紧急审视（别人赚钱你还在亏） |

## 常见工作流

### 日常报告（Agent 每日执行）

```bash
# 1. 自动更新所有资产价格
mns update-prices

# 2. 查看策略报告
mns report

# 3. 根据建议执行买入（如有）
mns buy QQQ 50 445.00

# 4. 记录卖出（如有）
mns sell QQQ 100 455.00

# 5. 查看最新持仓
mns portfolio
```

### 策略参数调优

```bash
# 查看当前买入比例
mns config buy_ratio

# 调整极端恐慌买入比例到 60%
mns config buy_ratio.extreme_fear 60.0

# 降低中性区间买入到 10%
mns config buy_ratio.neutral 10.0

# 调整卖出矩阵
mns config sell_ratio.extreme_greed_target_high 70.0

# 调整止盈线
mns config settings.annualized_target_low 12.0
mns config settings.annualized_target_high 18.0

# 调整逆向权重上限（防止单标的过度集中）
mns config settings.max_contrarian_weight 1.5
```

### 新资产添加流程

```bash
# 1. 添加资产到池子
mns add TSLA "特斯拉" us_stocks

# 2. 初始建仓买入
mns buy TSLA 50 250.00

# 3. 更新当前价格
mns price TSLA 255.00
```

## 配置参数

完整的配置参数说明请参考 `references/strategy.md`，其中包含：
- 所有配置项的默认值速查表
- 策略参数的详细解释
- 买入/卖出比例矩阵说明

命令使用方法请参考 `references/commands.md`。

## 注意事项

- **价格更新**: 买入/卖出后建议调用 `price` 或 `update-prices` 命令更新价格，确保持仓收益数据准确
- **时间敏感性**: `report` 和 `sentiment` 命令通过 HTTP 获取实时 CNN Fear & Greed Index，需要网络访问
- **异步要求**: 这两个命令是异步的，agent 调用时需确保环境支持 async execution
- **Windows 编码**: PowerShell 默认 GBK 编码可能导致中文乱码，建议使用 UTF-8 终端或重定向输出到文件
- **年化收益计算**: 使用 `annualized = (current / cost) ^ (365 / holding_days) - 1`，持仓天数 < min_holding_days 时不显示
- **绝对收益止盈**: 持仓绝对收益 ≥ 30% 且天数 ≥ min_absolute_profit_days 时，即使年化收益率未达阈值也可能触发卖出

## 错误处理

- **网络错误**: `sentiment` 和 `report` 可能因 API 不可用失败，建议重试或使用缓存数据
- **数据库锁定**: 多进程并发操作 SQLite 会导致锁定，agent 应确保串行访问

## 相关文件

- `references/commands.md` - 完整命令参考
- `references/strategy.md` - 策略参数详解

## 开源信息

MNS (money-never-sleep) 是开源项目，源码托管于 GitHub：

**[https://github.com/sopaco/money-never-sleep](https://github.com/sopaco/money-never-sleep)**
