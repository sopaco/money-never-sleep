# Dynamics — Active Issues & Constraints

**Last updated**: 2026-04-23

---

## Current Blockers / Known Issues

### 1. 恐贪指数 API 网络可达性
**Severity**: Environmental
**Detail**: CNN API 在某些网络环境可能被反爬虫拦截（返回 418）。
**Workaround**: 代码已设置 User-Agent 模拟浏览器，若仍失败可使用代理。
**Reported**: 2026-04-21
**Status**: Mitigated

### 2. 绝对收益阈值 (30%) 硬编码
**Severity**: Low
**Detail**: 长期止盈的30%绝对收益阈值未通过TOML配置。
**Workaround**: 直接编辑 `strategy.rs` 修改 `0.30` 常量。
**Reported**: 2026-04-19
**Status**: Accepted limitation

### 3. 单一情绪指数驱动的多市场组合
**Severity**: Design limitation
**Detail**: CNN 恐贪指数仅反映美股情绪，用于A股和黄金可能产生次优信号。
**Workaround**: 用户需结合判断；逆周期资产设计上可能与美股情绪反向。
**Reported**: 2026-04-19
**Status**: Documented design decision

### 4. Yahoo Finance API 速率限制
**Severity**: Low
**Detail**: 免费 API 有速率限制（约 5次/分钟，500次/天估计）。
**Workaround**: 避免短时间内频繁调用 market-indices 或 analyze 命令。
**Reported**: 2026-04-23
**Status**: Monitoring

---

## Recently Added Features

### 市场数据功能 (v0.5.9) — 2026-04-23
**Detail**: 新增市场数据获取模块和三个命令：
- `mns market` — 综合市场概况（9个全球指数 + 恐贪指数）
- `mns market-indices` — 专门查看全球主要指数
- `mns analyze <symbol>` — 个股基础报价分析
- 新增 `market.rs` 模块，使用 Yahoo Finance v8 API
- 支持 9 个全球主要指数：S&P 500, Dow Jones, NASDAQ, VIX, FTSE 100, DAX, Nikkei 225, 上证指数, 恒生指数

### 策略参数优化 (2026-04-22)
**Detail**: 基于历史回测验证，默认配置为防御配置（低回撤优先）：
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

### 编译警告清理 (2026-04-23)
**Resolved**: 清理所有编译警告
**Detail**: 移除未使用的 `fetch_quotes`, `fetch_market_summary`, `MarketSummary`，以及 `quote.rs` 中未使用的 `Deserialize` 导入

### 恐贪指数数据源修正 (2026-04-21)
**Resolved**: 切换为 CNN 股票市场恐贪指数
**Detail**: 修复 alternative.me 返回 crypto 恐贪指数的问题，改为直接调用 CNN API 获取股票市场恐贪指数

### 回测数据文件引用错误 (2026-04-21)
**Resolved**: 修复 `include_str!` 引用已删除CSV的编译错误
**Detail**: 更新数据文件路径和解析逻辑

### Strategy threshold mismatch with PRD (2026-04-19)
**Resolved**: 修复买卖比例缺少"中性"区间的逻辑错误
**Detail**: 添加正确的5区和3区逻辑，新增 `neutral_target_high` 配置项

### 文档参数不一致 (2026-04-23)
**Resolved**: 修正SKILL.md和strategy.md中的参数表格，与实际代码和backtest验证结论一致
**Detail**: 
- 修正 `buy_ratio` 表格：极度恐慌 50%→60%，恐慌 30%→35%，中性 20%→0%
- 修正 `sell_ratio` 表格：贪婪低收益 20%→25%，中性高收益 30%→15%
- 修正恐贪指数范围：极度恐慌 0-25→FGI<30，贪婪 75-100→FGI≥70
- 修正commands.md示例输出：buy_ratio.extreme_fear = 50.0→60.0
- 在mns-backtest/SKILL.md添加详细的预设配置参数说明表

### 投资策略文档对齐问题 (2026-04-23)
**Resolved**: 统一配置命名，修正卖出矩阵，精简SKILL内容
**Detail**: 
- 重命名"最优配置"为"历史激进配置"，添加免责声明：历史最优≠未来最优
- 统一配置命名：默认配置→防御配置（全文档一致）
- 修正DECISIONS.md卖出矩阵：≥22%/15-22%/<15% → ≥15%/10-15%/<10%（与代码一致）
- 补充PROJECT-ESSENCE.md策略价值说明：纪律性而非收益最大化
- 精简mns-backtest/SKILL.md：删除冗余的详细参数表格（50+行）
- 精简.ai-context/SKILL.md：移除重复的CLI命令完整列表
- 精简AGENTS.md：移除CLI完整列表，指向SKILL.md
- 更新AGENTS.md Rust版本：2021→2024

---

## Constraints

- **No frontend yet** — PRD mentions Svelte 5 for future dashboard
- **Single-user only** — SQLite, no auth, no multi-portfolio
- **Windows-first tested** — developed on Windows, cross-platform Rust
- **Existing config files lack new fields** — run `mns init` or manually add new fields
- **Free API rate limits** — Yahoo Finance ~5/min, 500/day estimated; CNN ~5/min
- **15-20 min quote delay** — Free data sources have inherent delay
- **No API keys required** — All data sources are free but rate-limited

---

## Upcoming Considerations

- [ ] Add `--json` output format for market commands
- [ ] Consider adding PE Ratio to `analyze` command
- [ ] Expand market indices (Russell 2000, etc.)
- [ ] Add sector ETF tracking (XLK, XLF, etc.)
```
