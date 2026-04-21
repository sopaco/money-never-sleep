# Dynamics — Active Issues & Constraints

**Last updated**: 2026-04-22

---

## Current Blockers / Known Issues

### 1. 恐贪指数 API 网络可达性
**Severity**: Environmental
**Detail**: CNN API 在某些网络环境可能被反爬虫拦截（返回 418）。
**Workaround**: 代码已设置 User-Agent 模拟浏览器，若仍失败可使用代理。
**Reported**: 2026-04-21

### 2. 绝对收益阈值 (30%) 硬编码
**Severity**: Low
**Detail**: 长期止盈的30%绝对收益阈值未通过TOML配置。
**Workaround**: 直接编辑 `strategy.rs` 修改 `0.30` 常量。
**Reported**: 2026-04-19

### 3. 单一情绪指数驱动的多市场组合
**Severity**: Design limitation
**Detail**: CNN 恐贪指数仅反映美股情绪，用于A股和黄金可能产生次优信号。
**Workaround**: 用户需结合判断；逆周期资产设计上可能与美股情绪反向。
**Reported**: 2026-04-19

---

## Recently Added Features

### 策略参数优化 (2026-04-22)
**Detail**: 基于历史回测验证，默认配置为保守配置（低回撤优先）：
- 美股 55%（降低美股占比）
- A股 25%（红利低波稳健配置）
- 黄金 20%（提高对冲比例）
- 极度恐慌买入比例 60%
- 年化止盈目标 10%/15%
- 最小持仓天数 45天
- 预期年化 7-8%，回撤 13-18%

### CNN API 集成 (2026-04-21)
**Detail**: 直接调用 CNN Fear & Greed API：
- 获取股票市场恐贪指数（非 crypto）
- 设置 User-Agent 避免反爬虫拦截
- 移除 `finance-query` 依赖

### 自动更新资产价格 (2026-04-21)
**Detail**: `mns update-prices` 自动获取所有持仓价格：
- 国内基金：天天基金接口
- 美股/ETF：Yahoo Finance API
- 失败时跳过，继续处理其他资产

---

## Recently Resolved

### 编译警告清理 (2026-04-21)
**Resolved**: 清理所有编译警告
**Detail**: 移除无用代码、修复 Clippy 警告

### 恐贪指数数据源修正 (2026-04-21)
**Resolved**: 切换为 CNN 股票市场恐贪指数
**Detail**: 修复 alternative.me 返回 crypto 恐贪指数的问题，改为直接调用 CNN API 获取股票市场恐贪指数

### 回测数据文件引用错误 (2026-04-21)
**Resolved**: 修复 `include_str!` 引用已删除CSV的编译错误
**Detail**: 更新数据文件路径和解析逻辑

### Strategy threshold mismatch with PRD (2026-04-19)
**Resolved**: 修复买卖比例缺少"中性"区间的逻辑错误
**Detail**: 添加正确的5区和3区逻辑，新增 `neutral_target_high` 配置项

---

## Constraints

- **No frontend yet** — PRD mentions Svelte 5 for future dashboard
- **Single-user only** — SQLite, no auth, no multi-portfolio
- **Windows-first tested** — developed on Windows, cross-platform Rust
- **Existing config files lack new fields** — run `mns init` or manually add new fields
