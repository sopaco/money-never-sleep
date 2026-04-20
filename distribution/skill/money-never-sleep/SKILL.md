---
name: money-never-sleep
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
  "更新现金余额", "记录买入卖出", "查看恐贪指数", "投资组合再平衡", "MNS 报告"
license: MIT
compatibility: Requires Node.js runtime. Prebuilt binaries available for darwin-arm64, linux-x64, win-x64. Access to local SQLite database at ~/.mns/mns.db.
metadata:
  version: "0.5.5"
  author: "Sopaco"
  language: "rust"
  platform: "cli"
  binary_distribution: "npm"
  security_verified: "ClawHub Security Verified"
  source_code: "https://github.com/sopaco/money-never-sleep"
---

# MNS CLI 投资管理 Skill

## 概述

本 skill 为 autonomous agents 提供 MNS 逆向投资策略的 CLI 操作能力。MNS 是基于 CNN Fear & Greed Index 情绪指标的量化投资工具，通过 contrarian 策略在情绪极度恐慌时买入，极度贪婪时卖出，实现市场中性风格的长期投资。

### 安全认证

> **✓ ClawHub Security Verified** - 本 skill 已通过 ClawHub 安全认证审核。
>
> - **开源项目**: 源码公开托管于 [GitHub](https://github.com/sopaco/money-never-sleep)
> - **预编译二进制**: 通过 npm 分发，由 GitHub Actions 从源码自动构建
> - **无网络交易**: 不连接任何券商 API，所有数据存储在本地 SQLite 数据库
> - **无敏感权限**: 仅访问 `~/.mns/` 目录下的本地配置和数据库

## 核心能力

1. **投资组合管理**: 查看持仓、现金余额、资产配置、年化收益
2. **交易记录**: 记录买入/卖出操作，更新资产当前价格（手动输入，非自动抓取）
3. **策略建议**: 自动生成基于最新恐贪指数的买卖建议报告（仅建议，不执行交易）
4. **配置管理**: 查看和调整策略参数（阈值、买入/卖出比例、止盈线）
5. **历史查询**: 查看交易历史、价格更新记录

> **重要说明**: 本工具仅提供策略建议和记录功能，不连接任何券商 API。
> 用户需自行在券商平台执行交易后，通过 CLI 记录交易结果。

## 快速开始

### 安装（首次使用）

```bash
# 安装 mns CLI，npm 会根据当前平台自动选择并安装对应的预编译 binary
npm install -g @never-sleeps/mns-cli
```

### 初始化

```bash
# ⚠️ 警告：init 命令会创建或覆盖 ~/.mns/config.toml 和 ~/.mns/mns.db
# 如果已有数据，系统会提示确认后再覆盖
mns init
mns cash set 100000
```

### 添加资产到持仓池

```bash
mns add QQQ "纳指100" us_stocks
mns add SH600000 "浦发银行" cn_stocks
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
# 更新资产当前价格
mns price QQQ 460.00

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
mns config buy_ratio.fear 0.30
mns config sell_ratio.("greed","between") 0.20
```

## 数据存储

- **配置文件**: `~/.mns/config.toml`（TOML 格式）
- **数据库**: `~/.mns/mns.db`（SQLite，包含 cash、positions、transactions、snapshots 表）
- **报告输出**: `./reports/`（当前目录），或通过 `settings.report_output_dir` 配置

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
2. **绝对收益线**: 绝对收益 ≥ 30% 时也可考虑卖出（即使年化收益率不高）

卖出矩阵（按情绪区间和价格位置）：

| 情绪 | 价格位置 | 卖出比例 |
|------|---------|---------|
| Extreme Greed | 高于阻力位 | 50% |
| Extreme Greed | 在支撑阻力间 | 30% |
| Extreme Greed | 低于支撑位 | 20% |
| Greed | 高于阻力位 | 40% |
| Greed | 在支撑阻力间 | 20% |
| Greed | 低于支撑位 | 0% |
| Neutral/Fear | 任何位置 | 0% |

### 买入资金分配：Contrarian 权重

可用现金按 contrarian 权重分配到各资产：

- **权重公式**: `weight = max(1.0, cost_price / current_price)`
- **解释**: 浮亏资产（当前价格 < 成本价）获得更高权重，符合逆向抄底逻辑
- **浮盈资产**: 权重为 1.0（基线）

## 常见工作流

### 日常报告（Agent 每日执行）

```bash
# 1. 查看策略报告（包含买卖建议、现金预测、风险警告）
mns report

# 2. 根据建议执行买入（如有）
mns buy QQQ 50 445.00

# 3. 更新价格（如有新价格）
mns price QQQ 448.50

# 4. 记录卖出（如有）
mns sell QQQ 100 455.00

# 5. 查看最新持仓
mns portfolio
```

### 策略参数调优（Agent 负责优化）

```bash
# 查看当前买入比例
mns config buy_ratio

# 调整极端恐慌买入比例到 60%
mns config buy_ratio.extreme_fear 0.60

# 降低中性区间买入到 10%
mns config buy_ratio.neutral 0.10

# 提高极度贪婪时的卖出比例
mns config sell_ratio.("extreme_greed","above_high") 0.60

# 调整年化止盈线
mns config annualized_target_low 12.0
mns config annualized_target_high 18.0
```

### 新资产添加流程

```bash
# 1. 添加资产到池子
mns add TSLA "特斯拉" us_stocks

# 2. 初始建仓买入
mns buy TSLA 50 250.00

# 3. 记录当前价格（后续需要定期更新）
mns price TSLA 255.00
```

## 注意事项

- **价格更新**: 买入/卖出后必须调用 `price` 命令更新成本价到当前市值，否则持仓收益数据不准确
- **时间敏感性**: `report` 和 `sentiment` 命令通过 HTTP 获取实时 CNN Fear & Greed Index，需要网络访问
- **异步要求**: 这两个命令是异步的，agent 调用时需确保环境支持 async execution
- **Windows 编码**: PowerShell 默认 GBK 编码可能导致中文乱码，建议使用 UTF-8 终端或重定向输出到文件
- **年化收益计算**: 使用 `annualized = (current / cost) ^ (365 / holding_days) - 1`，短期持仓（< min_holding_days）的年化收益不显示
- **绝对收益止盈**: 当持仓绝对收益 ≥ 30% 时，即使年化收益率未达阈值也可能触发卖出建议

## 错误处理

- **网络错误**: `sentiment` 和 `report` 可能因 API 不可用失败，建议重试或使用缓存数据
- **数据库锁定**: 多进程并发操作 SQLite 会导致锁定，agent 应确保串行访问
- **配置错误**: 检查 `~/.mns/config.toml` 语法，使用 `config` 命令验证可访问的配置项

## 相关文件

- `references/` - 详细的技术参考文档（配置结构、数据库 schema、策略参数）
- `assets/` - 模板文件（如报告 HTML 模板）

## 开源信息

MNS (money-never-sleep) 是开源项目，源码托管于 GitHub：

**[https://github.com/sopaco/money-never-sleep](https://github.com/sopaco/money-never-sleep)**
