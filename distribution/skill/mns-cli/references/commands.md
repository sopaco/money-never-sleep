# MNS CLI 命令参考

本文档详细描述 MNS CLI 的所有命令、参数和用法。

## 命令概览

| 命令 | 用途 | 异步 |
|------|------|------|
| `init` | 初始化和数据库 | 否 |
| `config` | 查看/修改配置 | 否 |
| `cash` | 现金余额管理 | 否 |
| `portfolio` | 查看持仓概览 | 否 |
| `add` | 新增资产到持仓池 | 否 |
| `buy` | 记录买入交易 | 否 |
| `sell` | 记录卖出交易 | 否 |
| `price` | 更新资产当前价格 | 否 |
| `sentiment` | 查看当前恐贪指数 | 是 |
| `report` | 生成策略报告 | 是 |
| `history` | 查看交易历史 | 否 |

---

## `init`

**语法**: `mns init`

**功能**: 创建默认配置文件 `~/.mns/config.toml` 和空的 SQLite 数据库 `~/.mns/mns.db`，同时创建报告输出目录。

**示例**:
```bash
npx @mns/cli-darwin-arm64 init
```

**输出**:
```
✓ 初始化完成
  配置文件: /home/user/.mns/config.toml
  数据库:   /home/user/.mns/mns.db
  报告目录: ./reports
```

**注意**: 
- 如果配置文件或数据库已存在，`init` 会覆盖它们
- 报告目录默认为当前目录下的 `reports/`，可通过配置修改

---

## `config`

**语法**: `mns config [KEY] [VALUE]`

**功能**: 查看或修改配置项。支持 dot-path 语法访问嵌套配置。

### 查看所有配置
```bash
npx @mns/cli-darwin-arm64 config
```

输出 TOML 格式的完整配置。

### 查看特定配置项
```bash
npx @mns/cli-darwin-arm64 config thresholds.fear
npx @mns/cli-darwin-arm64 config buy_ratio.extreme_fear
```

输出:
```
thresholds.fear = 45
buy_ratio.extreme_fear = 0.50
```

### 修改配置项
```bash
npx @mns/cli-darwin-arm64 config thresholds.greed 75
npx @mns/cli-darwin-arm64 config buy_ratio.fear 0.30
```

**支持的特殊语法**:
- 嵌套键: `thresholds.fear`
- 数组索引: `sell_ratio.("greed","between")`（注意引号和括号）

**示例 - 完整参数调优**:
```bash
# 调整买入比例
npx @mns/cli-darwin-arm64 config buy_ratio.extreme_fear 0.60
npx @mns/cli-darwin-arm64 config buy_ratio.fear 0.25
npx @mns/cli-darwin-arm64 config buy_ratio.neutral 0.10

# 调整卖出矩阵
npx @mns/cli-darwin-arm64 config sell_ratio.("extreme_greed","above_high") 0.60
npx @mns/cli-darwin-arm64 config sell_ratio.("greed","above_high") 0.50

# 调整止盈线
npx @mns/cli-darwin-arm64 config annualized_target_low 12.0
npx @mns/cli-darwin-arm64 config annualized_target_high 18.0

# 调整最小持有天数（避免短期年化收益失真）
npx @mns/cli-darwin-arm64 config min_holding_days 30
```

---

## `cash`

**语法**: `mns cash [ACTION]`

**功能**: 查看或设置现金余额。

### 查看余额
```bash
npx @mns/cli-darwin-arm64 cash
```

输出:
```
现金余额: ¥125430.50
```

### 设置现金余额
```bash
npx @mns/cli-darwin-arm64 cash set 100000
```

### 增加现金
```bash
npx @mns/cli-darwin-arm64 cash add 5000
```

**注意**:
- `set` 会覆盖当前余额
- `add` 在当前余额基础上增加
- 金额不能为负数

---

## `portfolio`

**语法**: `mns portfolio`

**功能**: 显示当前持仓概览，包括：
- 资产代码和名称
- 持仓份额和成本价
- 当前价格（需通过 `price` 命令更新）
- 持仓市值和盈亏
- 年化收益率（基于持有天数）

**示例输出**:
```
┌───────┬────────────┬──────────┬──────────┬─────────────┬─────────────┬────────────┐
│ 代码  │   名称     │  份额    │ 成本价   │ 当前价      │  市值       │ 年化收益   │
├───────┼────────────┼──────────┼──────────┼─────────────┼─────────────┼────────────┤
│ QQQ   │ 纳指100    │ 150.0    │ 450.00   │ 460.50      │ 69075.00    │  +8.2%     │
│ SH600 │ 浦发银行   │ 500.0    │ 12.30    │ 12.80       │ 6400.00     │ +15.3%     │
└───────┴────────────┴──────────┴──────────┴─────────────┴─────────────┴────────────┘

总市值: ¥75475.00
现金余额: ¥24685.50
总资产: ¥100160.50
```

**注意事项**:
- 年化收益计算公式: `(current / cost) ^ (365 / holding_days) - 1`
- 如果持有天数小于配置的 `min_holding_days`，年化收益显示为 `N/A`
- 当前价格需要定期通过 `price` 命令更新

---

## `add`

**语法**: `mns add <CODE> <NAME> <CATEGORY>`

**功能**: 新增资产到持仓池，后续可通过 `buy` 命令买入。

**参数**:
- `CODE`: 资产代码（如 `QQQ`, `SH600000`, `AAPL`）
- `NAME`: 资产名称（任意描述性字符串）
- `CATEGORY`: 类别，预设可选值：
  - `us_stocks` - 美股
  - `cn_stocks` - A股
  - `counter_cyclical` - 逆周期资产

**示例**:
```bash
npx @mns/cli-darwin-arm64 add QQQ "纳指100" us_stocks
npx @mns/cli-darwin-arm64 add SH600000 "浦发银行" cn_stocks
npx @mns/cli-darwin-arm64 add GLD "黄金ETF" counter_cyclical
```

**注意**:
- 资产代码在数据库中必须唯一
- 添加后立即用 `buy` 记录初始买入

---

## `buy`

**语法**: `mns buy <CODE> <SHARES> <PRICE>`

**功能**: 记录买入交易，增加持仓份额。

**参数**:
- `CODE`: 资产代码（必须已通过 `add` 添加）
- `SHARES`: 买入份额（支持小数，如股票可支持碎股）
- `PRICE`: 买入单价（元/股）

**示例**:
```bash
# 买入 100 股 QQQ，单价 450.50 元
npx @mns/cli-darwin-arm64 buy QQQ 100 450.50

# 买入 50 份黄金 ETF，单价 180.00 元
npx @mns/cli-darwin-arm64 buy GLD 50 180.00
```

**行为**:
- 现金余额相应减少
- 持仓成本价重新计算（加权平均）
- 交易记录存入 `transactions` 表

---

## `sell`

**语法**: `mns sell <CODE> <SHARES> <PRICE>`

**功能**: 记录卖出交易，减少持仓份额。

**参数**:
- `CODE`: 资产代码
- `SHARES`: 卖出份额（不能超过当前持仓）
- `PRICE`: 卖出单价

**示例**:
```bash
# 卖出 30 股 QQQ，单价 455.00 元
npx @mns/cli-darwin-arm64 sell QQQ 30 455.00

# 清仓某资产
npx @mns/cli-darwin-arm64 sell SH600000 500 13.20
```

**行为**:
- 现金余额相应增加
- 持仓份额减少，成本价保持不变（剩余份额）
- 交易记录存入 `transactions` 表

---

## `price`

**语法**: `mns price <CODE> [PRICE]`

**功能**: 查看或更新资产的当前价格。

### 查看当前价格
```bash
npx @mns/cli-darwin-arm64 price QQQ
```

输出:
```
QQQ (纳指100): ¥460.50
```

### 更新当前价格
```bash
npx @mns/cli-darwin-arm64 price QQQ 460.50
```

**注意**:
- 每次更新价格会更新对应持仓的 `current_price` 和 `current_at` 字段
- 不更新价格会导致 `portfolio` 和 `report` 显示的市值不准确
- 建议每日运行 `report` 前先更新价格，或编写脚本自动更新

---

## `sentiment`

**语法**: `mns sentiment`

**功能**: 从 CNN 官网获取最新的 Fear & Greed Index 数据。

**示例**:
```bash
npx @mns/cli-darwin-arm64 sentiment
```

输出:
```
CNN Fear & Greed Index: 42 (Fear)
Previous Close: 38
1 Week Ago: 35
1 Month Ago: 28
```

**注意**:
- 此命令是异步的，需要网络访问
- 如果 API 不可用，可能返回错误
- 数据来自 `https://cnn.com/data/api/ Fear & Greed` 端点

---

## `report`

**语法**: `mns report`

**功能**: 生成今日策略报告，包含：
1. 当前恐贪指数和情绪判断
2. 买入建议（基于可用现金和 contrarian 分配）
3. 卖出建议（基于年化收益和绝对收益）
4. 持仓摘要和风险警告（浮亏过大）
5. 现金预测（按建议执行后）

**示例**:
```bash
npx @mns/cli-darwin-arm64 report
```

**输出节选**:
```
╔═══════════════════════════════════════════════════════════════╗
║           MNS 逆向投资策略报告 - 2025-06-16                  ║
╚═══════════════════════════════════════════════════════════════╝

📊 市场情绪 (CNN Fear & Greed)
┌─────────────────────────────────────┐
│ 当前指数: 42 (Fear)                  │
│ 状态解读: 市场处于恐慌区间，建议逐步  │
│         买入                          │
└─────────────────────────────────────┘

💰 买入建议 (可用现金: ¥25000)
┌─────────────────────────────────────┐
│ QQQ (Fear 区间权重: 1.2)             │
│  建议买入: ¥12000 (50 股 @ ¥240)    │
│                                     │
│ SH600000 (Fear 区间权重: 1.5)        │
│  建议买入: ¥13000 (1042 股 @ ¥12.5) │
└─────────────────────────────────────┘

⚠️  卖出建议
┌─────────────────────────────────────┐
│ 暂无满足卖出条件的持仓              │
└─────────────────────────────────────┘

🚨 风险警告
┌─────────────────────────────────────┐
│ [无]                                │
└─────────────────────────────────────┘
```

**注意**:
- 异步命令，需要网络获取恐贪指数
- 报告同时保存到报告目录（默认 `./reports/`）
- 买入/卖出建议仅供参考，agent 需根据实际执行顺序调整

---

## `history`

**语法**: `mns history [--limit N]`

**功能**: 查看最近的交易历史。

**参数**:
- `--limit N`: 显示条数，默认 20，最大 100

**示例**:
```bash
# 查看最近 20 条
npx @mns/cli-darwin-arm64 history

# 查看最近 50 条
npx @mns/cli-darwin-arm64 history --limit 50
```

**输出**:
```
最近交易记录 (按时间倒序):
┌─────────────────┬───────┬────────┬──────┬────────┬──────────┐
│ 时间            │ 类型  │ 资产   │ 份额 │ 价格   │ 金额     │
├─────────────────┼───────┼────────┼──────┼────────┼──────────┤
│ 2025-06-15 10:3│ buy   │ QQQ    │ 50   │ 448.50 │ 22425.00 │
│ 2025-06-14 14:2│ sell  │ SH600  │ 100  │ 13.00  │ 1300.00  │
│ 2025-06-14 09:1│ price │ QQQ    │ -    │ 445.00 │ -        │
└─────────────────┴───────┴────────┴──────┴────────┴──────────┘
```

**类型说明**:
- `buy` - 买入交易
- `sell` - 卖出交易
- `price` - 价格更新记录

---

## 异步命令说明

`sentiment` 和 `report` 是异步命令，调用方式略有不同：

### 在 Shell 中直接调用
```bash
npx @mns/cli-darwin-arm64 sentiment
npx @mns/cli-darwin-arm64 report
```

### 在 Node.js/TypeScript 中调用
```javascript
import { spawn } from 'child_process';

const result = spawn('npx', [
  '@mns/cli-darwin-arm64',
  'report'
], { shell: true });

result.stdout.on('data', data => console.log(data.toString()));
result.stderr.on('data', data => console.error(data.toString()));
```

### 在 Python 中调用
```python
import subprocess

result = subprocess.run(
    ['npx', '@mns/cli-darwin-arm64', 'report'],
    capture_output=True,
    text=True
)
print(result.stdout)
if result.returncode != 0:
    print(f"Error: {result.stderr}")
```

---

## 通用 Exit Code

| Code | 含义 |
|------|------|
| 0    | 成功 |
| 1    | 通用错误（配置错误、参数错误、数据库错误等） |
| 2    | 网络错误（仅影响 `sentiment`/`report`） |

---

## 平台适配

根据目标平台选择相应的 npm 包名：

| 平台 | npm 包名 |
|------|----------|
| macOS (Apple Silicon) | `@mns/cli-darwin-arm64` |
| Linux (x64) | `@mns/cli-linux-x64` |
| Windows (x64) | `@mns/cli-win-x64` |

**示例使用方式**:
```bash
# 在 agent skill 中动态选择平台
npx @mns/cli-${PLATFORM} report
# 其中 PLATFORM 为 darwin-arm64 / linux-x64 / win-x64
```

**注意**: 这些包是预编译的二进制分发，安装时需要解除 Node.js 的 npm 包限制（`--ignore-scripts` 或使用 `npx` 直接运行）。

---

## 配置文件结构参考

完整的配置结构（`~/.mns/config.toml`）:

```toml
[database]
path = "~/.mns/mns.db"

[settings]
min_holding_days = 30
annualized_target_low = 10.0
annualized_target_high = 15.0
report_output_dir = "./reports"

[thresholds]
extreme_fear = 25
fear = 45
neutral = 55
greed = 75
extreme_greed = 100

[buy_ratio]
extreme_fear = 0.50
fear = 0.30
neutral = 0.20
greed = 0.00

[sell_ratio]
# 格式: sell_ratio.("情绪", "价格位置") = 比例
# 情绪: extreme_greed, greed, neutral, fear, extreme_fear
# 价格位置: above_high, between, below_low
"sell_ratio.(\"extreme_greed\",\"above_high\")" = 0.50
"sell_ratio.(\"extreme_greed\",\"between\")" = 0.30
"sell_ratio.(\"extreme_greed\",\"below_low\")" = 0.20
"sell_ratio.(\"greed\",\"above_high\")" = 0.40
"sell_ratio.(\"greed\",\"between\")" = 0.20
```

---

## 数据库 Schema 参考

主要表结构：

### `cash`
- `id` INTEGER PRIMARY KEY
- `balance` REAL NOT NULL
- `updated_at` TIMESTAMP

### `positions`
- `id` INTEGER PRIMARY KEY
- `code` TEXT NOT NULL UNIQUE
- `name` TEXT NOT NULL
- `category` TEXT NOT NULL
- `shares` REAL NOT NULL
- `cost_price` REAL NOT NULL
- `current_price` REAL NOT NULL
- `updated_at` TIMESTAMP
- `created_at` TIMESTAMP

### `transactions`
- `id` INTEGER PRIMARY KEY
- `type` TEXT NOT NULL ('buy' | 'sell' | 'price')
- `code` TEXT NOT NULL
- `shares` REAL
- `price` REAL
- `amount` REAL
- `created_at` TIMESTAMP

### `snapshots` (用于数据导出和审计)
- `id` INTEGER PRIMARY KEY
- `data` TEXT (JSON 格式的快照)

---

## 错误信息速查

| 错误信息 | 原因 | 解决方案 |
|----------|------|----------|
| `未知的配置项` | 配置路径错误或不存在 | 用 `config` 查看有效的配置项 |
| `现金余额不能为负数` | `cash set` 参数 < 0 | 检查金额参数 |
| `资产代码不存在` | `buy`/`sell`/`price` 使用未添加的代码 | 先用 `add` 添加资产 |
| `卖出份额超过持仓` | `sell` 参数 > 当前持仓 | 检查持仓数量 |
| `API error: ...` | CNN API 不可用 | 稍后重试或检查网络 |
| `Database is locked` | 并发访问数据库 | 确保同一时间只有一个进程操作 |
| `配置文件解析错误` | `config.toml` 语法错误 | 检查 TOML 语法 |

---

## 常见问题

### Q: 如何批量导入历史交易？
A: 暂无批量导入命令。可编写脚本依次调用 `buy`、`sell`、`price`，或直接操作 SQLite 数据库。

### Q: 年化收益显示为 `N/A`？
A: 持仓天数小于 `min_holding_days`（默认 30 天）时不计算年化收益，避免短期波动误导。

### Q: 如何查看详细的卖出建议计算过程？
A: 查看 `report` 输出中的 "卖出建议" 部分，或读取数据库中的 `positions` 表计算 `annualized_return`。

### Q: 怎样将报告导出为 HTML？
A: 当前报告为纯文本格式。可将输出重定向到文件后用第三方工具转换，或自行编写 wrapper 脚本生成 HTML。

### Q: 能否在 Windows PowerShell 中正确显示中文？
A: PowerShell 默认 GBK 编码可能导致乱码。解决方案：
```powershell
# 使用 UTF-8 编码
chcp 65001
# 或在命令前设置
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new()
```

---

## 版本历史

### v0.5.0 (当前)
- 完整的 CLI 命令集
- 异步 CNN Fear & Greed Index 获取
- dot-path 配置管理
- contrarian 买入分配
- 双准则卖出决策
- 多平台二进制分发（npm）

---

**最后更新**: 2025-06-16