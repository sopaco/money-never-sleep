---
name: mns-advisor
description: |
  This skill should be used when an AI agent helps a real investor decide whether to buy,
  sell, or hold using the MNS (Money Never Sleeps) CLI — monthly contributions, weekly
  rebalance checks, unplanned impulse buys, checking whether an order is actually executable,
  or reviewing execution discipline. It combines deterministic MNS calculations with
  web-verified fund/channel facts and produces an auditable decision record. The sentiment
  score sets the target exposure only; it never decides whether to trade. This skill does not
  predict markets, produce timing alpha, or place orders.

  Triggers include: "什么时候买", "什么时候卖", "该不该买", "这周要不要加仓", "定投怎么分配",
  "帮我看一下报告", "限购还能买吗", "复盘我的操作", "buy or sell", "should I buy",
  "rebalance decision", "mns guard", "mns plan", "MNS 实盘决策"

  Not for backtests, parameter tuning, dataset rebuilds, or significance testing — use the
  `mns-backtest` skill for those.
agent_created: true
---

# MNS 实盘买卖决策编排

> **作用范围**：本仓库专用，供能读取源码、数据集与整改报告的 Agent 使用，含实现细节与项目内证据，**不可分发给外部 Agent**。
> 面向「环境里只有 `mns` 命令、没有本仓库源码」的通用 Agent，改用用户级 `~/.workbuddy/skills/mns-advisor/`（可分发包 `distribution/skills/mns-advisor.zip`）。两者同名、内容不同，不要混用或互相引用。

## 定位

把「何时买、何时卖」当作**授权问题**处理，而不是预测问题：什么事件允许动手、动手前核验什么、什么情况下必须不动。MNS CLI 负责确定性计算，本 skill 负责事件判定、联网事实核验、解释与留痕。决策权始终属于用户。

不在 CLI 中新增「择时 / 买卖点」接口。2016–2026 的审计已否定情绪信号的择时贡献（见 `references/evidence-boundary.md`）；再提供一个看似能择时的命令，等于让被证伪的能力以接口形式复活。AGENT 恰好补上 CLI 缺的三件事：核验会过期的现实约束、把计算结果翻译成可执行动作、识别计划外的行为偏差。

## 决策叙事（先内化，再执行）

一句话：**交易合法性只来自「现金流事件」和「配置偏离」，不来自对市场的判断。**

| 触发器 | 判定 | 买 | 卖 | 依赖命令 |
|---|---|---|---|---|
| A 现金流事件 | 新钱到账 / 需要用钱 | 按缺口分配买入 | 出金或减仓 | `plan` / `cash withdraw` |
| B 状态偏离 | 决策日（默认周五）检查 | 某腿低配超带宽 | 某腿超配超带宽且过最短持有期 | `report` / `today` |
| C 计划外起意 | 用户临时想动 | 延迟 1 个交易日 + 写理由 | 同左 | `guard` |

判定顺序 A → B → C。C 不是独立买入理由，只是对 A / B 之外行为的流程约束。

情绪分数的唯一职责是**刻度**：决定「允许持有多少风险资产」，不决定「今天买不买」，也不产生止盈。当前决策路径没有任何止盈规则。

多数周的正确答案是「不操作」。详细触发器、卖出专章、不动作清单与节奏见 `references/buy-sell-playbook.md`。

## 工作流

0. **判定触发器**：先确认属于 A / B / C / 复盘中的哪一类。信息不足时先向用户提问，不要用命令代替提问。
1. **只读采集**：运行 `scripts/collect_state.sh`（可加 `--monthly <金额>`、`--mirror`）。禁止在此阶段调用 `today` / `report` / `update-prices` / `buy` / `sell` / `cash deposit` / `cash withdraw` —— 这些会写快照、价格或台账。命令契约见 `references/cli-contract.md`。
2. **联网核验**：按 `references/live-verification.md` 核验限购、费率、结算、标的状态与交易日历，并与 `mns config` 的静态假设逐项比对。联网信息的合法用途是「这笔交易能否按计划执行、成本是否变了」，不是「该不该买」。
3. **用 MNS 计算**：买入金额取 `mns plan`，再平衡金额取决策日 `mns report`；不自行重算目标仓位或金额，不把核验结果折算成「修正后的仓位」。`plan` 与 `report` 金额不同时按分母差异解释（`plan` 含本月新钱），不调和成一个数。
4. **输出决策卡**：按 `references/agent-workflow.md` 的字段输出；每条结论标注来源（MNS 计算 / 联网事实与日期 / 推断 / 用户陈述），并明确写出「本周是否操作」。
5. **留痕与复盘**：实际成交后经用户确认再用 `mns buy` / `mns sell` 登记；用 `assets/decision-record.md` 记录理由；复盘对照纪律执行率而非收益率。

## 硬约束

1. 不预测涨跌、不给目标价或点位、不说「现在是买点 / 卖点」。
2. 不因新闻、研报、社交情绪改变买卖方向，不覆盖 `effective_score`。
3. 不用 `guard` 否决交易，不把 FGI 描述为 alpha。
4. 不擅自修改 `[target_weight]` / `[rebalance]` 以「优化收益」，不引用 `backtest legacy` / `validate` 的数字作实盘依据。
5. 不自动下单，不代替用户执行 `buy` / `sell` / `cash deposit` / `cash withdraw`。
6. 不用 `mns backtest` 的收益数字作实盘预期。
7. 不制造交易：无触发器、偏离在带宽内、数据过期、年度预算用尽、非决策日 → 输出「不操作」。
8. 不混用长周期代理回测与实盘账户镜像；不承诺收益，标注不确定处。

完整禁用话术与证据数字见 `references/evidence-boundary.md`。

## 资源

| 文件 | 何时读取 |
|---|---|
| `references/buy-sell-playbook.md` | 判定买卖触发器、解释「为何不动」、回答「何时卖」 |
| `references/agent-workflow.md` | 执行 5 步、写决策卡、复盘 |
| `references/live-verification.md` | 联网核验前 |
| `references/cli-contract.md` | 调用任何 mns 命令前，确认只读性与输出字段 |
| `references/evidence-boundary.md` | 输出结论前核对证据与措辞 |
| `scripts/collect_state.sh` | Step 1 只读采集 |
| `assets/decision-record.md` | Step 5 留痕模板 |

## 典型入口

- 「这个月发工资了，3000 怎么买」→ A 类 → 只读采集 + `plan --monthly 3000` + 限购核验 → 决策卡。
- 「看到纳指连涨几天，想追加 2 万」→ C 类 → `guard` + 延迟 1 日 + 写理由，不否决也不鼓励。
- 「这周要调仓吗」→ B 类 → 先确认是否决策日；非决策日直接答「不操作」并给出下一个决策日。
- 「我上个月是不是乱动了」→ 复盘 → `history` + `backtest mirror` + 决策记录比对。
