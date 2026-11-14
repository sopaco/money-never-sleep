# MNS - Money Never Sleeps

> 个人投资台账、仓位纪律与实盘审计工具（Rust CLI）。2026-11-08 整改后版本。

---

## 一、定位

MNS 记录持仓与资金流水，按 CNN 恐贪指数把风险资产目标仓位约束在五档曲线内，并在偏离超过带宽时给出再平衡建议。它不预测收益、不自动下单、不阻断交易。

2026-11-08 的整改把默认 `mns backtest` 从「2016–2025 月频代理序列上的策略对比」换成「实盘四只基金 + 2016 至今日频长周期代理 + 与实盘同源信号 + 真实通道结算 + 账户镜像」的审计链路。审计结论如下：

| 检验 | 结果 |
|---|---|
| 三资产 10.8 年真实信号置换检验 | 第 63 分位，未过 80 分位门槛 → 择时无显著贡献 |
| 沪深300 单资产对照 | FGI 第 44 分位；本地 200 日均线逆向信号第 61 分位 |
| 常数仓位对照 | 70%~75% 常数仓位常等价或更优，且交易更少 |
| 账户镜像（2026-07-16 入场） | 严格执行 vs 实际执行期末净值差 ¥135.45；严格执行 32.3 次/年超出 10–20 次年度预算，实际流水 19.4 次/年；QDII 限购阻断 7 次 |
| 买入冷静期实验 | 四种过热定义的 20 日收益差与尾部概率置信区间均跨 0；自动否决买入在长期上行资产池上期望为负 |

结论：**不要把 MNS 当作择时或收益增强工具**。可验证的价值是纪律性的仓位约束、账户对账与执行复盘；情绪信号只作为「延迟一天 + 写明理由」的流程护栏，不作为买卖依据。完整推导见 [`docs/remediation-2026-11-08.md`](docs/remediation-2026-11-08.md)、[`docs/backtest-validation-review-2026-11-08.md`](docs/backtest-validation-review-2026-11-08.md)。

---

## 二、功能地图

| 分组 | 命令 | 作用 |
|---|---|---|
| 初始化与配置 | `init` / `config` / `config params` | 建立 `~/.mns/config.toml` 与 `mns.db`；查看/修改参数（写入前自动校验）；`config params` 列出全部可调项 |
| 账户台账 | `cash`（`deposit` / `withdraw` / `set`）、`add` / `remove`、`buy` / `sell`、`price`、`history`、`update-prices` | 资金进出、持仓与交易流水；`deposit` 是入金主通道，`set` 仅用于纠错 |
| 绩效视图 | `portfolio`、`performance` | 单标的年化/绝对收益；组合 XIRR、实际最大回撤、与回测口径对照（只读，零写入） |
| 行情与情绪 | `sentiment`、`market`、`analyze` | 抓取并保存恐贪快照、市场指数、个股报价 |
| 日常决策 | `report` / `today`、`plan`、`guard` | 决策日（默认周五）出决策版报告，其余天出观察版；`plan` 给只买不卖的月投分配；`guard` 给买入冷静期提示 |
| 实盘审计 | `backtest`（= `validate-full`）、`backtest mirror`、`backtest validate-full --iterations N`、`backtest update-data` | 置换/常数仓位/延迟/随机对照、36 月 holdout、牛熊分段、多折 walk-forward、账户镜像、三层数据重建与新鲜度 SLA |
| 对照实验 | `backtest asset --dataset hs300` | 用同一套引擎检验单一宽基；同时跑 A 股本地信号作对照 |
| 已降级 | `backtest legacy`、`backtest validate` | 旧月频代理引擎，仅作历史参考，结论不可迁移到实盘 |

配套 Python 脚本在 `.agents/skills/mns-backtest/data/`：`build_live_dataset.py`、`build_long_dataset.py`、`build_hs300_dataset.py` 生成三层数据，`guardrail_experiment.py` 复现冷静期实验。日常无需直接调用，`mns backtest update-data` 会统一调度。

**「什么时候买、什么时候卖」不在这套 CLI 里**，而在 [`.agents/skills/mns-advisor/`](.agents/skills/mns-advisor/) 这套面向 AGENT 的 SKILL 中：CLI 只做确定性计算（目标仓位、偏离、金额、台账），AGENT 负责判定交易触发器、联网核验限购/费率/结算、把结果翻译成可执行动作并留痕。情绪信号只决定仓位刻度，不决定买不买；交易合法性只来自现金流事件与配置偏离。**外部 Agent（只有 `mns` 命令、无本仓库源码）请使用通用分发版**，见文末文档表。

---

## 三、正确使用方式

### 1. 首次初始化

```bash
cargo build --release
mns init                       # 已有数据会提示确认；--force 会覆盖
mns cash deposit 10000         # 外部入金，走 XIRR 口径；不要用 cash set 记入金
mns add 018966 "汇添富纳斯达克100ETF" us_stocks
mns buy 018966 <份额> <成交价>
mns config params              # 确认可调参数与默认值
```

资产类别只能是 `us_stocks` / `cn_stocks` / `counter_cyclical`。三腿内部比例由 `allocation.*` 控制。

### 2. 每周（默认周五决策日）

```bash
mns today                      # 先刷价格再出报告；价格获取失败也继续
mns report --decide            # 只在确有必要于非决策日获取建议时使用
mns performance                # 只读：看实际 XIRR / 实际回撤是否落在预期内
```

`report` 本身不刷价；`today` = 先批量刷价（失败也继续用旧价）再走**同一个** `cmd_report_inner`。除此之外两者完全同源：都会联网拉恐贪、写当日恐贪快照与资产快照、把报告存到 `reports/`，因此**都不是只读命令**。CNN 接口失败时回退本地快照：≤7 天照常出报告并标注滞后，>7 天强制观察版且 `--decide` 直接拒绝。`today --quiet` 只跳过过程日志与价格明细，报告全文仍会打印。报告含【口径披露】与年度交易预算（默认 10–20 次）。

### 3. 每月定投

```bash
mns plan --monthly 3000        # 当前 cashflow.monthly = 0，省略 --monthly 会报错
mns guard                      # 过热时只提示「延迟 1 个交易日 + 写明理由」
# 实际成交后补录台账
mns buy <代码> <份额> <价格>
mns history 20                 # 复盘买入理由与执行偏差
```

`plan` 与 `guard` 依赖本地恐贪快照，首次使用先跑一次 `mns report`。`plan` 只买不卖，不产生收益 alpha，作用是储蓄率与执行纪律。

### 4. 定期审计（季度或数据陈旧时）

```bash
mns backtest update-data       # 重建三层日频数据集，输出新鲜度 SLA（默认 30 天）
mns backtest                   # 完整审计：有效性对照 → holdout → 牛熊分段 → walk-forward → 账户镜像
mns backtest mirror            # 只看「严格执行策略 vs 实际流水」
mns backtest validate-full --iterations 500
```

读结论的顺序：先看置换检验分位是否过门槛；再看牛熊分段而非全区间；再看 walk-forward 改善是否 ≥0.5pp/年；最后看账户镜像的净值差、费用差、受阻次数。数据落后超过 SLA 会醒目告警。

### 5. 研究与对照

```bash
mns backtest asset --dataset hs300
python3 .agents/skills/mns-backtest/data/guardrail_experiment.py
```

单资产层只回答「换标的/换输入信号会怎样」，绝对收益不得与实盘层或长周期层互相引用。

---

## 四、必须遵守的边界

- **不要逐日跟单**。日频信号压缩到周频决策，年度预算 10–20 次；严格执行策略会到 32.3 次/年，已超预算上限。
- **不要把回测收益当实盘预期**。长周期层是代理标的（红利腿为价格指数、黄金腿为场内 ETF）；实盘结论只以账户镜像层为准。
- **不要用 `guard` 择时**。它不阻断交易、不改仓位、不给买卖结论；描述性统计（FGI≥75 后 60 日下跌概率 33.7% vs 25.9%）未通过显著性检验。
- **不要用 `legacy` / `validate` 的数字做决策**。两者是 2016–2025 月频代理、3 月平滑、月末成交、10 万起步 + 年注资 5 万，与真实账户不是同一策略。
- **实盘与回测同源**。改 `[target_weight]` / `[rebalance]` / `backtest.decision_interval` 会同时影响 `mns report` 与 `mns backtest`，不允许两条链路各设一套信号后互引结论。
- **参考预设不自动加载**。`.agents/skills/mns-backtest/data/config_*.toml` 七个文件只在 `mns backtest legacy --config <路径>` 显式指定时生效。
- **价格与净值口径**。审计按 point-in-time 对齐：FGI 严格早于交易日，净值不晚于交易日；T+1 执行、按通道 T+n 结算、在途资金不可复用、QDII ¥520/日限购。
- **未检验区间**。2000–2002 与 2008 型多年熊市没有数据支撑；实盘共同区间仅 34 个月。本工具不构成投资建议。

---

## 五、数据与配置

| 数据集 | 区间 | 标的 | 用途 |
|---|---|---|---|
| `live_daily.csv` | 2024-01-11 ~ 2026-11-06（682 行） | 018966 / 539001 / 007605 / 020341 真实全收益指数 + 单位净值 | 账户镜像、实盘结论 |
| `long_daily.csv` | 2016-01-04 ~ 2026-11-06（2637 行） | 539001 / H30269 中证红利低波价格指数 / 518880 | 穿越牛熊的稳健性检验 |
| `hs300_daily.csv` | 2016-01-04 ~ 2026-11-06（2633 行） | 510300 前复权（含分红） | 单资产对照实验 |

生效配置始终来自 `~/.mns/config.toml`。新增的 `[backtest]`（审计设置）、`[channels]`（按通道费率/结算/限购）、`[guardrail]`（冷静期）与原有 `[target_weight]` / `[rebalance]` 共用一份配置；`mns config <键> <值>` 校验通过后才落盘。参数清单以 `mns config params` 为准。

---

## 六、文档

| 文档 | 内容 |
|---|---|
| [`docs/backtest-validation-review-2026-11-08.md`](docs/backtest-validation-review-2026-11-08.md) | 整改前的问题清单与证据 |
| [`docs/remediation-2026-11-08.md`](docs/remediation-2026-11-08.md) | 整改落地、三层数据、逐条验收 |
| [`docs/hs300-experiment-2026-11-08.md`](docs/hs300-experiment-2026-11-08.md) | 沪深300 单资产实验 |
| [`docs/guardrail-experiment-2026-11-08.md`](docs/guardrail-experiment-2026-11-08.md) | 买入冷静期显著性实验 |
| [`docs/product-review-2026-11-08.md`](docs/product-review-2026-11-08.md) | 产品视角评审（含已撤回结论的更正） |
| [`.agents/skills/mns-backtest/SKILL.md`](.agents/skills/mns-backtest/SKILL.md) | 回测 Skill：数据契约、口径、结果解读 |
| [`.agents/skills/mns-advisor/SKILL.md`](.agents/skills/mns-advisor/SKILL.md) | 实盘买卖决策编排 Skill：三类触发器、AGENT 五步工作流、联网核验清单、CLI 契约与证据边界 |
| [`.agents/skills/mns-advisor/references/buy-sell-playbook.md`](.agents/skills/mns-advisor/references/buy-sell-playbook.md) | 「何时买 / 何时卖 / 何时不动」的完整叙事与判定链 |
| [`distribution/skills/mns-advisor.zip`](distribution/skills/mns-advisor.zip) | **通用分发版**（与本仓库版同名但不同内容）：假定 `mns` 已在 PATH、无源码、无数据集，只教 Agent 调用命令与解读输出；跨平台，含 macOS/Linux 的 bash 与 Windows 的 PowerShell 两版只读采集脚本。安装位置 `~/.workbuddy/skills/mns-advisor/` |
| [`docs/gui-p0.md`](docs/gui-p0.md) | 桌面端 P0：GPUI Kit 窗口/托盘、ACP 外接 `opencode` 自主运行的验证结论、已知取舍与未验证边界 |
| [`docs/gui-design.md`](docs/gui-design.md) | 桌面端视觉系统：色彩令牌、字号阶梯、情绪氛围映射、图标的四种用法与产物管线 |
| [`docs/gui-release.md`](docs/gui-release.md) | 打包与分发：`make-dmg.sh` 的四个步骤、ad-hoc / Developer ID / 公证三档签名、架构与已知边界 |

桌面端仍在 P0，入口为 `cargo run --bin mns-gui --features gui`；不启用该 feature 时 CLI 依赖与行为完全不变。外接 Agent 只需配置 CLI 路径，模型与凭据留在 `opencode` 自身配置中。

要一个能双击安装的版本，执行 `./scripts/make-dmg.sh`，产物在 `target/dist/`（`.app` 与 `.dmg`）。签名、公证与分发边界见 [`docs/gui-release.md`](docs/gui-release.md)。

Windows 下用 PowerShell 构建安装包（需先装 Inno Setup 6：`winget install -e --id JRSoftware.InnoSetup`）：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\make-installer.ps1
```

产物在 `target/dist/`：`MNS-<version>-win-x64-setup.exe` 与 `MNS-<version>-win-x64.zip`。安装包按用户安装到 `%LOCALAPPDATA%\Programs\MNS`，不弹 UAC，包含开始菜单快捷方式与可选桌面快捷方式；不修改系统 PATH。

界面语言见 [`docs/gui-design.md`](docs/gui-design.md)：红绿只表示盈亏，情绪沿蓝↔琥珀轴影响背景与装饰；图标不再与文字并排，而是作为角落水印或按钮徽记。应用图标与托盘图标由 `swift scripts/make-icons.swift` 从 `assets/app_icon.png` 一次生成（`.icns` / `.ico` / PNG / 裸 RGBA）。

当前验收状态：`cargo test` 116 passed / 0 failed / 3 ignored，`cargo check` 0 warning；三层数据集末日均 2026-11-06。
