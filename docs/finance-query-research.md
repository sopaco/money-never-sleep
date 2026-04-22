# Finance Query 项目调研报告

## 项目概述

**项目名称**: finance-query  
**仓库地址**: https://github.com/Verdenroz/finance-query  
**开源协议**: MIT  
**主要语言**: Rust (82.4%), Python (16.5%)  
**当前版本**: v2.4.3 (截至 2026-03-28)

Finance Query 是一个用 Rust 编写的金融数据查询工具，提供了多种使用方式：
- **Rust 库** - 可集成到其他 Rust 项目中
- **CLI 工具 (fq)** - 功能强大的命令行工具
- **HTTP 服务器** - 提供 REST API、WebSocket 和 GraphQL 接口
- **MCP 服务器** - 用于 AI 代理集成（如 Claude、Cursor、Windsurf 等）

同时提供免费的托管版本：**https://finance-query.com**

---

## 核心功能能力

### 1. 市场数据 (Market Data)

| 功能 | 描述 |
|------|------|
| 实时报价 | 获取股票、ETF、加密货币、货币等的实时价格 |
| 历史数据 | 获取 OHLCV（开高低收成交量）历史数据 |
| 实时流数据 | 通过 WebSocket 实时推送价格更新 |
| 市场摘要 | 指数、期货、债券、加密货币的市场概览 |
| 趋势股票 | 按地区查看热门股票 |
| 世界指数 | 全球市场指数数据 |
| 行业数据 | 11 个 GICS 行业的表现和龙头股 |

### 2. 公司信息 (Company Information)

| 功能 | 描述 |
|------|------|
| 公司详情 | 公司简介、行业、员工数等完整信息 |
| 财务报表 | 利润表、资产负债表、现金流量表 |
| 盈利数据 | 历史盈利和预期数据 |
| 新闻资讯 | 公司相关新闻报道 |
| 分析师评级 | 买入/持有/卖出建议 |
| 股东信息 | 机构持股、内部人士持股 |
| SEC EDGAR | 交互式 EDGAR 文件浏览器（TUI） |
| XBRL 数据 | 来自 EDGAR 的结构化财务数据 |
| 盈利电话会议记录 | 盈利电话会议的文字记录 |
| 评级历史 | 分析师升级/降级历史 |

### 3. 技术分析 (Technical Analysis)

| 功能 | 描述 |
|------|------|
| 40+ 技术指标 | RSI, MACD, SMA, EMA, Bollinger Bands, ATR, Stochastic, ADX, OBV, VWAP, CCI, Williams %R, StochRSI, Parabolic SAR, SuperTrend, MFI, Ichimoku, Donchian 等 |
| 策略回测 | 预设策略（swing、day、trend、mean-reversion、conservative、aggressive）或自定义策略回测 |

### 4. 期权与股息 (Options & Dividends)

| 功能 | 描述 |
|------|------|
| 期权链 | 交互式期权链浏览器（TUI） |
| 股息历史 | 股息支付历史和分析（CAGR、平均支付额） |
| 股票分割 | 股票分割历史 |

### 5. 工具功能 (Utilities)

| 功能 | 描述 |
|------|------|
| 符号搜索 | 按名称或关键词搜索股票代码 |
| 市场时间 | 市场开盘/收盘时间和状态 |
| 货币汇率 | 货币列表和汇率 |
| 价格提醒 | 价格提醒和桌面通知 |

### 6. 风险分析 (Risk Analytics)

| 功能 | 描述 |
|------|------|
| VaR | 95% 和 99% 置信度的在险价值 |
| 风险比率 | Sharpe、Sortino、Calmar 比率 |
| Beta | 相对市场的波动性 |
| 最大回撤 | 历史最大跌幅 |

### 7. 替代数据 (Alternative Data)

| 功能 | 描述 |
|------|------|
| FRED 数据 | 美联储经济数据（FEDFUNDS、CPI、GDP、UNRATE 等） |
| 国债收益率 | 美国国债收益率曲线（1个月-30年） |
| 加密货币 | CoinGecko 前 N 名加密货币（无需 API key） |
| RSS 源 | Bloomberg、WSJ、FT、SEC、MarketWatch 等的 RSS/Atom 源 |

---

## 技术架构

### 整体架构

```
┌─────────────────────────────────────────────────────┐
│                  finance-query                       │
│                  (核心库 - Core Library)              │
└──────────────┬──────────────┬───────────────────────┘
               │              │
       ┌───────▼────────┐    ┌▼──────────────────┐
       │  CLI (fq)      │    │  Server            │
       │  命令行工具     │    │  HTTP/WebSocket     │
       └────────────────┘    │  GraphQL            │
                             └──────┬──────────────┘
                                    │
                             ┌──────▼──────────────┐
                             │  MCP Server          │
                             │  AI 代理集成          │
                             └─────────────────────┘
```

### 核心组件

1. **finance-query** (核心库)
   - 提供所有金融数据查询的核心逻辑
   - 异步 API 设计
   - 支持可选功能：dataframe、indicators、fred、crypto、rss、risk

2. **finance-query-cli** (命令行工具)
   - 丰富的 TUI（终端用户界面）
   - 支持多种输出格式：table、json、csv
   - 交互式图表和期权链浏览器

3. **finance-query-server** (HTTP 服务器)
   - REST API（v2 版本）
   - WebSocket 实时流数据
   - GraphQL 查询语言
   - Redis 缓存支持
   - 基于速率限制

4. **finance-query-mcp** (MCP 服务器)
   - 36 个 MCP 工具
   - 支持 HTTP 和 stdio 传输
   - 兼容 Claude、Cursor、Windsurf、Zed、Continue 等

5. **finance-query-derive** (过程宏)
   - 用于 Polars DataFrame 集成的过程宏
   - 简化数据结构转换

### 技术栈

| 技术选型 | 说明 |
|----------|------|
| **语言** | Rust (edition 2021) - 高性能、内存安全 |
| **异步运行时** | Tokio - 高性能异步 I/O |
| **数据处理** | Polars - 高性能 DataFrame 库 |
| **Web 框架** | Axum - 异步 Web 框架 |
| **数据库/缓存** | Redis - 可选的缓存层 |
| **序列化** | Serde - JSON/序列化支持 |
| **CLI 框架** | Clap - 命令行参数解析 |
| **TUI 框架** | Ratatui - 终端用户界面 |
| **容器化** | Docker + Docker Compose |
| **反向代理** | Caddy/Nginx |

---

## 实现原理

### 1. 数据源

Finance Query 的数据主要来自 Yahoo Finance 的公开数据：
- 通过爬取和分析 Yahoo Finance 网页获取数据
- 不需要 API key（除了可选的 FRED 数据）
- SEC EDGAR 数据来自官方 API（需要提供联系邮箱）

### 2. 异步设计

所有网络请求都是异步的，使用 Tokio 运行时：
- 单个符号请求：`Ticker::builder("AAPL").build().await?`
- 批量请求：`Tickers::builder(vec!["AAPL", "MSFT"]).build().await?`
- WebSocket 流：实时推送价格更新

### 3. 缓存策略

服务器支持 Redis 缓存：
- 根据市场时间智能设置 TTL（生存时间）
- 市场开盘时：短 TTL（如 1 分钟）
- 市场关闭时：长 TTL（如 1 小时）
- 可通过 `--no-default-features` 禁用

### 4. 速率限制

基于 Governor 的速率限制：
- 默认：60 请求/分钟
- 可通过环境变量 `RATE_LIMIT_PER_MINUTE` 配置
- 防止滥用和保护服务器资源

### 5. 技术指标计算

使用 Rust 实现的纯计算：
- 从历史 OHLCV 数据计算
- 支持 42+ 种指标
- 高性能，无需外部依赖

### 6. 回测引擎

策略回测包含：
- 预设策略（swing、day、trend 等）
- 自定义策略参数
- 性能指标计算（Sharpe 比率、最大回撤等）

---

## 安装与使用

### 方式一：使用托管 API（推荐）

免费托管版本：**https://finance-query.com**

```bash
# 获取报价
curl "https://finance-query.com/v2/quote/AAPL"

# 实时流数据
wscat -c "wss://finance-query.com/v2/stream"

# GraphQL 游乐场
open "https://finance-query.com/graphql"
```

### 方式二：CLI 工具

#### 安装

**预编译二进制文件（推荐）：**

```bash
# Linux/macOS
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Verdenroz/finance-query/releases/latest/download/finance-query-cli-installer.sh | sh

# Windows
powershell -c "irm https://github.com/Verdenroz/finance-query/releases/latest/download/finance-query-cli-installer.ps1 | iex"
```

**从源码安装：**

```bash
git clone https://github.com/Verdenroz/finance-query
cd finance-query/finance-query-cli
cargo install --path .
```

#### 使用示例

```bash
# 获取报价
fq quote AAPL MSFT GOOGL

# 实时流数据
fq stream AAPL TSLA NVDA

# 交互式图表
fq chart AAPL -r 6mo

# 技术指标
fq indicator AAPL --indicator rsi:14

# 策略回测
fq backtest AAPL --preset swing

# 市场仪表盘
fq dashboard

# 价格提醒
fq alerts add AAPL price-above:200
fq alerts watch

# SEC EDGAR 文件
fq edgar AAPL
fq facts AAPL

# 期权链
fq options AAPL

# 财务报表
fq financials AAPL
```

### 方式三：Rust 库

#### 添加依赖

```toml
[dependencies]
finance-query = "2.3"

# 或带额外功能
finance-query = { version = "2.3", features = ["dataframe", "indicators", "fred", "crypto", "rss", "risk"] }
```

#### 使用示例

**单个符号：**

```rust
use finance_query::Ticker;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ticker = Ticker::builder("AAPL").logo().build().await?;
    let quote = ticker.quote().await?;

    if let Some(price) = quote.regular_market_price.as_ref().and_then(|v| v.raw) {
        println!("AAPL: ${:.2}", price);
    }
    Ok(())
}
```

**批量操作：**

```rust
use finance_query::{Tickers, Interval, TimeRange};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let tickers = Tickers::builder(vec!["AAPL", "MSFT", "GOOGL"]).logo().build().await?;
    let response = tickers.quotes().await?;

    for (symbol, quote) in &response.quotes {
        if let Some(price) = quote.regular_market_price.as_ref().and_then(|v| v.raw) {
            println!("{}: ${:.2}", symbol, price);
        }
    }
    Ok(())
}
```

**SEC EDGAR 文件：**

```rust
use finance_query::edgar;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    edgar::init("your.email@example.com")?;

    // 解析代码到 CIK
    let cik = edgar::resolve_cik("AAPL").await?;

    // 获取文件历史
    let submissions = edgar::submissions(cik).await?;
    if let Some(name) = &submissions.name {
        println!("Company: {}", name);
    }

    // 获取 XBRL 财务数据
    let facts = edgar::company_facts(cik).await?;
    if let Some(us_gaap) = facts.facts.get("us-gaap") {
        if let Some(revenue) = us_gaap.0.get("Revenues") {
            if let Some(usd) = revenue.units.get("USD") {
                for point in usd.iter().take(3) {
                    if let (Some(fy), Some(val)) = (point.fy, point.val) {
                        println!("FY {}: ${}", fy, val);
                    }
                }
            }
        }
    }
    Ok(())
}
```

### 方式四：HTTP 服务器

#### 本地运行

```bash
git clone https://github.com/Verdenroz/finance-query.git
cd finance-query
make serve  # 编译并运行 v2 服务器
```

#### Docker 部署

```bash
# 单服务器
docker build -t finance-query-server -f server/Dockerfile .
docker run -p 8000:8000 finance-query-server

# 完整部署（v1 + v2 + Redis + Nginx）
make docker-compose
```

#### 配置

创建 `.env` 文件：

```bash
PORT=8000
RUST_LOG=info
REDIS_URL=redis://localhost:6379  # 可选
RATE_LIMIT_PER_MINUTE=60         # 可选，默认 60
EDGAR_EMAIL=you@example.com      # EDGAR 端点必需
```

#### REST API 示例

```bash
# 单符号
curl "http://localhost:8000/v2/quote/AAPL"
curl "http://localhost:8000/v2/chart/AAPL?range=1mo&interval=1d"
curl "http://localhost:8000/v2/options/AAPL"

# 批量
curl "http://localhost:8000/v2/quotes?symbols=AAPL,MSFT,GOOGL"

# 市场数据
curl "http://localhost:8000/v2/market-summary"
curl "http://localhost:8000/v2/trending"
curl "http://localhost:8000/v2/indices"

# EDGAR
curl "http://localhost:8000/v2/edgar/cik/AAPL"
curl "http://localhost:8000/v2/edgar/facts/AAPL"

# WebSocket
wscat -c "ws://localhost:8000/v2/stream"
```

### 方式五：MCP 服务器（AI 集成）

#### 托管实例（推荐）

添加到你的 MCP 客户端配置：

**Claude Code** (`.mcp.json`):

```json
{
  "mcpServers": {
    "finance-query": {
      "type": "http",
      "url": "https://finance-query.com/mcp"
    }
  }
}
```

**Cursor** (`.cursor/mcp.json`):

```json
{
  "mcpServers": {
    "finance-query": {
      "type": "http",
      "url": "https://finance-query.com/mcp"
    }
  }
}
```

**Windsurf** (`~/.codeium/windsurf/mcp_config.json`):

```json
{
  "mcpServers": {
    "finance-query": {
      "type": "http",
      "url": "https://finance-query.com/mcp"
    }
  }
}
```

#### 自托管 MCP

**Docker（推荐）：**

```bash
docker run -p 3000:3000 \
  -e MCP_TRANSPORT=http \
  -e EDGAR_EMAIL=your@email.com \
  -e FRED_API_KEY=your_fred_api_key \
  ghcr.io/verdenroz/finance-query/mcp:latest
```

**从源码运行：**

```bash
# stdio（默认）
cargo run -p finance-query-mcp --quiet

# HTTP
MCP_TRANSPORT=http cargo run -p finance-query-mcp --quiet
```

#### 环境变量

| 变量 | 必需 | 描述 |
|------|------|------|
| `EDGAR_EMAIL` | 可选 | 启用 `get_edgar_*` 工具。SEC 要求在 User-Agent 中提供联系邮箱。 |
| `FRED_API_KEY` | 可选 | 启用 `get_fred_series`。在 fred.stlouisfed.org 免费申请。`get_treasury_yields` 无需 key。 |

#### 可用的 MCP 工具（36 个）

- **报价**: `get_quote`, `get_quotes`, `get_recommendations`, `get_splits`
- **图表**: `get_chart`, `get_charts`, `get_spark`
- **基本面**: `get_financials`, `get_batch_financials`, `get_holders`, `get_analysis`
- **技术指标**: `get_indicators`, `get_batch_indicators`
- **期权**: `get_options`
- **股息**: `get_dividends`, `get_batch_dividends`
- **市场数据**: `get_market_summary`, `get_market_hours`, `get_trending`, `get_indices`, `get_fear_and_greed`, `get_sector`, `get_industry`
- **搜索**: `search`, `lookup`, `screener`, `get_news`, `get_feeds`, `get_transcripts`
- **风险分析**: `get_risk`
- **加密货币**: `get_crypto_coins`
- **FRED**: `get_fred_series`, `get_treasury_yields`
- **SEC EDGAR**: `get_edgar_facts`, `get_edgar_submissions`, `get_edgar_search`

---

## 适用场景

### 1. 个人投资者
- 获取实时股票报价和历史数据
- 技术指标分析和策略回测
- 监控投资组合和设置价格提醒
- 查看财务报表和分析师评级

### 2. 量化交易
- 批量获取历史数据用于模型训练
- 计算 40+ 技术指标
- 策略回测和性能评估
- 风险分析（VaR、Sharpe 比率等）

### 3. 金融应用开发
- 作为 Rust 库集成到应用中
- 通过 REST API 提供 HTTP 服务
- WebSocket 实时数据推送
- GraphQL 灵活查询

### 4. AI 金融助手
- MCP 服务器集成到 Claude、Cursor 等 AI 工具
- 让 AI 能够查询实时金融数据
- 构建智能投资顾问

### 5. 数据分析
- 导出 CSV 格式用于 Excel 分析
- Polars DataFrame 集成用于 Python 分析
- SEC EDGAR 数据用于财务研究

---

## 优势与限制

### 优势

1. **完全开源** - MIT 协议，可自由使用和修改
2. **高性能** - Rust 实现，性能远超 Python 替代品
3. **多接口** - 库、CLI、HTTP、MCP 多种使用方式
4. **无需 API Key** - 大部分功能无需注册（FRED 除外）
5. **免费托管** - 提供 finance-query.com 免费服务
6. **功能丰富** - 覆盖报价、技术分析、回测、风险分析等
7. **AI 友好** - MCP 服务器支持 AI 代理集成
8. **实时数据** - WebSocket 支持实时流数据

### 限制

1. **数据源单一** - 主要依赖 Yahoo Finance，可能存在限制
2. **非官方 API** - 爬取 Yahoo 网页，可能被限制访问
3. **延迟问题** - 免费数据可能有一定的延迟
4. **可靠性** - 不适合生产环境的关键交易系统
5. **历史数据** - 某些数据的历史范围有限
6. **国际市场** - 主要覆盖美股，其他市场支持有限

---

## 与 MNS 项目的关联

### 相似之处

1. **语言选择** - 都使用 Rust 实现
2. **CLI 工具** - 都提供命令行工具（`mns` vs `fq`）
3. **投资场景** - 都面向个人投资者
4. **技术分析** - 都涉及技术指标和策略

### 互补关系

Finance Query 可以作为 MNS 的数据源：
- 提供实时报价和历史数据
- 提供技术指标计算
- 提供 SEC EDGAR 财务数据

### 集成建议

1. **作为依赖库**
   ```toml
   [dependencies]
   finance-query = { version = "2.3", features = ["indicators"] }
   ```
   
2. **作为 HTTP 服务**
   - 本地运行 finance-query-server
   - 通过 REST API 获取数据
   - 避免重复造轮子

3. **作为 MCP 服务器**
   - 集成到 AI 开发流程
   - 让 AI 助手能够查询实时金融数据

---

## 总结

Finance Query 是一个功能丰富、设计精良的金融数据查询工具。它提供了多种使用方式（库、CLI、HTTP、MCP），覆盖了从基础报价到技术分析、风险分析的完整链路。

对于 MNS 项目，Finance Query 可以作为：
1. **学习参考** - 学习 Rust 金融工具的设计模式
2. **数据源** - 直接集成获取金融数据
3. **功能扩展** - 借鉴其技术指标和回测功能

推荐优先使用其托管 API（finance-query.com）或作为库集成，避免重复开发数据获取模块。

---

## 参考链接

- **GitHub 仓库**: https://github.com/Verdenroz/finance-query
- **官方文档**: https://verdenroz.github.io/finance-query/
- **Rust Docs**: https://docs.rs/finance-query/
- **Crates.io**: https://crates.io/crates/finance-query
- **托管 API**: https://finance-query.com