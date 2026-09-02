# MNS - Money Never Sleeps

> **AI 时代的逆向投资决策助手**  
> 克服人性弱点，系统化执行"在恐慌中贪婪，在贪婪中恐慌"

<p align="center">
    <a href="https://github.com/sopaco/money-never-sleep/tree/main/.terrain/human"><img alt="Litho Docs" src="https://img.shields.io/badge/Litho-Docs-green?logo=Gitbook&color=%23008a60"/></a>
    <a href="http://clawhub.ai/sopaco/money-never-sleep"><img alt="ClawHub" src="https://img.shields.io/badge/ClawHub-Certified-blue"/></a>
    <a href="https://github.com/openclaw/openclaw"><img alt="OpenClaw Compatible" src="https://img.shields.io/badge/OpenClaw-Compatible-brightgreen"/></a>
</p>

---

## 投资最大的敌人，不是市场，而是人性

恐惧时不敢买，贪婪时不愿卖——这是每个投资者的本能困境。MNS 将逆向投资策略数字化，用规则代替情绪，让数据驱动决策：

- 🎯 **极度恐慌时自动建议加仓**，而非恐慌抛售
- 💰 **年化收益达标时提醒止盈**，而非追涨杀跌  
- ⚠️ **持仓浮亏时智能预警**，而非视而不见
- 📊 **回测口径诚实披露**：风险调整后优于买入持有（Calmar 1.17 vs 0.98），但**收益低于**买入持有

---

## 核心优势

### 🧠 基于真实市场数据
集成 CNN Fear & Greed Index，实时感知市场情绪，自动保存历史快照用于回溯分析。

### 📈 经过验证的策略参数
基于 2016-2025 **真实全收益数据**（人民币计价、含分红）并**计入交易成本、阶梯赎回费与现金收益**回测。
默认配置：美股 55%、A股 25%、黄金 20%，风险资产目标权重随情绪在 35%~85% 之间调整。

| 策略 | 年化(XIRR) | 最大回撤 | Calmar | 年均交易 |
|---|---|---|---|---|
| 目标仓位(趋势锚) | 12.75% | 10.91% | **1.17** | 8.0 |
| 买入持有 | **14.60%** | 14.93% | 0.98 | 3.2 |

⚠️ **请先读这个**：买入持有在收益上领先约 1.9pp 年化。本工具的价值仅在风险调整后成立
（回撤低约 4pp）。且 2000-2002／2008 等长期熊市未被数据覆盖 —— 而"熊市保护"正是本策略的主要卖点，
该卖点尚未经检验。

**⚠️ 更重要的是——上表 12.75%/Calmar 1.17 是「趋势锚」的回测结果，但 `mns report` 实盘默认走的是「情绪锚」
（不做趋势判断，纯按恐贪指数映射目标仓位），实盘口径的样本内表现是 9.79%/Calmar 1.07。而样本外
(walk-forward) 验证显示，这个风险调整后优势本身也不稳健：**

| 样本外(2022-11 起) | 年化 | 最大回撤 | Calmar |
|---|---|---|---|
| 默认配置(情绪锚，即 `mns report` 实盘口径) | 7.91% | 9.18% | 0.86 |
| 默认配置(趋势锚，仅回测展示，未接入实盘) | 10.79% | 11.47% | 0.94 |
| 样本内调参最优(情绪锚) | 6.76% | 7.36% | 0.92 |
| **买入持有** | **12.23%** | 11.95% | **1.02** |

样本外买入持有在**收益和 Calmar 上同时反超**两种锚点配置——样本内的"风险调整后占优"未能外推到样本外。
bootstrap 分布（`mns backtest validate` 会打印）显示这一差异也落在统计噪声范围内，不具备显著性。
`mns backtest validate` 还会把 2018、2022（数据集内仅有的两段明显下跌年份）逐年拆开单独展示——
两种锚点在这两年确实比买入持有回撤更小，但这**不能替代 2000-2002／2008 这类多年期熊市压力测试**，
那类数据目前不可得，"熊市保护"仍是未充分验证的卖点。运行 `mns backtest validate` 可自行复现以上全部数字；
不要只看 `mns backtest` 默认展示的样本内趋势锚结果。

### 🔄 买卖互感知
卖出回收的现金自动计入买入预算，先算卖再算买，资金利用率最大化。

### 🛡️ 双重止盈机制
年化收益达标 OR 绝对收益 ≥30%，两种方式锁定利润，不错过长期持有的复利效应。

### 🎯 逆向加仓逻辑
浮亏越多，建议加仓越多（有权重上限），真正实现"别人恐惧我贪婪"。

---

## AI 时代的人机协作：OpenClaw + SKILL

MNS 专为 AI 辅助开发设计，内置两套知识系统：

| 系统 | 面向 | 作用 |
|------|------|------|
| `.agents/skills/` / `.claude/skills/` | AI Coding Agent | Skill 定义（如 `mns-backtest`），快速理解回测工作流 |
| `.terrain/agent/` | AI Coding Agent | 项目架构、模块划分、核心流程（Terrain 知识资产） |
| `.terrain/human/` | 人类开发者 | 项目概述、架构、工作流程、深入探索 |

### SKILL 协同效果

当你说出关键词（如"回测"、"backtest"），AI 会自动激活对应的 SKILL，瞬间获得完整的上下文理解：

- ✅ **无需解释项目背景**，AI 已经理解架构和约束
- ✅ **修改策略参数有保障**，AI 知道在哪里改、怎么改、不会破坏其他模块
- ✅ **添加新功能时自动遵循既有模式**，保持代码一致性
- ✅ **调试问题时快速定位根因**，AI 掌握完整的数据流和调用关系

> 💡 **一句话触发 SKILL**：对 AI 说"我想优化止盈参数"或"帮我回测这个策略"，AI 会自动读取
> [`.agents/skills/mns-backtest/SKILL.md`](.agents/skills/mns-backtest/SKILL.md) 并获得完整项目知识。

---

## 快速开始

```bash
# 安装
cargo build --release

# 初始化
mns init
mns cash set 100000

# 日常使用
mns add QQQ "纳指100" us_stocks
mns buy QQQ 50 380.00
mns update-prices
mns report          # 生成今日操作建议
mns backtest          # 四种策略对比（同一数据与成本模型）
mns backtest validate # 样本外验证 + bootstrap 收益分布 + holdout
```

---

## 文档导航

| 文档 | 说明 |
|------|------|
| [.terrain/human/](.terrain/human/) | 面向人类的友好文档：项目概述、架构、工作流程 |
| [.agents/skills/mns-backtest/SKILL.md](.agents/skills/mns-backtest/SKILL.md) | 回测 Skill：面向 AI 的操作步骤、参数说明、数据文件对照 |
| [AGENTS.md](AGENTS.md) | AI Coding Agent 工作指南 |

---

## 设计理念

```
恐惧 × 贪婪 = 亏损
规则 × 纪律 = 收益
```

MNS 不预测市场，不执行交易，只做一件事：**在关键时刻，给你系统化的决策依据**。

剩下的，由你决定。

---

**用 OpenClaw + MNS，让 AI 成为你的投资决策搭档。**
