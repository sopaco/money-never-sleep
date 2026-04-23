# MNS - Money Never Sleeps

> **AI 时代的逆向投资决策助手**  
> 克服人性弱点，系统化执行"在恐慌中贪婪，在贪婪中恐慌"

<p align="center">
    <a href="https://github.com/sopaco/money-never-sleep/tree/main/litho.docs"><img alt="Litho Docs" src="https://img.shields.io/badge/Litho-Docs-green?logo=Gitbook&color=%23008a60"/></a>
    <a href="http://clawhub.ai/sopaco/money-never-sleep"><img alt="ClawHub" src="https://img.shields.io/badge/ClawHub-Certified-blue"/></a>
    <a href="https://github.com/openclaw/openclaw"><img alt="OpenClaw Compatible" src="https://img.shields.io/badge/OpenClaw-Compatible-brightgreen"/></a>
</p>

---

## 投资最大的敌人，不是市场，而是人性

恐惧时不敢买，贪婪时不愿卖——这是每个投资者的本能困境。MNS 将逆向投资策略数字化，用规则代替情绪，让数据驱动决策：

- 🎯 **极度恐慌时自动建议加仓**，而非恐慌抛售
- 💰 **年化收益达标时提醒止盈**，而非追涨杀跌  
- ⚠️ **持仓浮亏时智能预警**，而非视而不见
- 📊 **历史回测验证策略有效性**，年化收益 8-9%，最大回撤 16-21%

---

## 核心优势

### 🧠 基于真实市场数据
集成 CNN Fear & Greed Index，实时感知市场情绪，自动保存历史快照用于回溯分析。

### 📈 经过验证的策略参数
基于 2016-2025 历史数据回测优化，默认防御配置（美股 55%、A股 25%、黄金 20%），收益/回撤比 0.42+。

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
| `.ai-context/` | AI Coding Agent | 快速理解项目架构、设计决策、活跃问题 |
| `litho.docs/` | 人类开发者 | 项目概述、工作流程、深入探索 |

### SKILL 协同效果

当你说出关键词（如"回测"、"backtest"），AI 会自动激活对应的 SKILL，瞬间获得完整的上下文理解：

- ✅ **无需解释项目背景**，AI 已经理解架构和约束
- ✅ **修改策略参数有保障**，AI 知道在哪里改、怎么改、不会破坏其他模块
- ✅ **添加新功能时自动遵循既有模式**，保持代码一致性
- ✅ **调试问题时快速定位根因**，AI 掌握完整的数据流和调用关系

> 💡 **一句话触发 SKILL**：对 OpenClaw 说"我想优化止盈参数"或"帮我回测这个策略"，AI 会自动读取 `.ai-context/SKILL.md` 并获得完整项目知识。

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
mns backtest        # 策略回测
```

---

## 文档导航

| 文档 | 说明 |
|------|------|
| [litho.docs/](litho.docs/) | 面向人类的友好文档：项目概述、架构、工作流程 |
| [.ai-context/](.ai-context/) | 面向 AI 的上下文：架构决策、设计约束、活跃问题 |
| [AGENTS.md](AGENTS.md) | OpenClaw AI Agent 工作指南 |

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
