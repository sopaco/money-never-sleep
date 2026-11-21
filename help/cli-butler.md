# CLI：管家审计（mns-butler）

`mns-butler` 不是第二个 MNS：簿记、策略与报告仍由 `mns` / `mns-gui` 负责。
它只暴露管家的确定性部分，用来回答两个问题：

1. 今天为什么说话 / 为什么没说话？
2. 这条通知、这条记忆、这个任务是谁授权的？

所有状态都在 `~/.mns/agent/butler/`，与 `mns` 共用同一个数据根。

桌面端还有一个常驻的 `mns-butlerd`：它替 GUI 做巡检判定（关掉主界面也不停），
开口写进 `pending.jsonl` 由 GUI 投递。它不轮询——算出下一次该醒的时刻就睡，
你的每条命令（`task add`、`remember`、`forget`、`mute`）都会顺便叫醒它重算。
手动跑 `wake` 会与它共用同一份状态，用于复算或调试；不要同时造两份「刚刚说过」
的记录。

`mns-butlerd` 需要 `agent` 特性构建（`cargo build --features agent --bin mns-butlerd`）：
确定性内核没有由头时，它会按需起一次外部 agent 做主动开口，回答过不了通知
口径就沉默，本轮不写任何记录。

## 常用命令

```bash
mns-butler status            # 唤醒状态、开关、记忆与任务概况
mns-butler wake              # 按当前账户做一次定时巡检判定
mns-butler wake --ledger ID  # 模拟一条账本事件（买入/卖出/入金/出金）
mns-butler tick              # 立即跑一次每周自整理（记忆合并 + 任务复盘）
mns-butler tasks             # 待执行与已归档任务
mns-butler task add "下个决策日核对计划" --at "2026-11-20 09:30"
mns-butler task add "提醒我喝水" --in 5m --user
mns-butler task add "到点陪我聊两句" --in 1h --talk
mns-butler task done <id>
mns-butler remember "周五补美股" --due 2026-11-20   # 写入一条事实
mns-butler memory            # 常驻事实（--archived 看归档）
mns-butler forget "等发工资补仓"   # 命中事实移入归档
mns-butler outbox            # 最近已授权但未送达的通知
mns-butler mute | unmute     # 关闭 / 恢复主动开口
```

## 对话：交给外部 agent

`mns-butler` 没有对话子命令，也不持有模型配置。管家窗口里的提问由外部 ACP
agent 回答（默认 `opencode acp`），provider、模型与密钥都在 agent 自己的
配置里，MNS 这边不保存。

要换 CLI 或模型，在桌面端设置页「外部 Agent」里改，写到 `~/.mns/gui.toml`。
agent 没装、没登录、超时，或回答超长不像话，这一条退回本地模板——判定、
开口与通知始终在本机，和对话走哪条路无关。

## 判定顺序

`wake` 依次做三件事，任何一步不成立就不会有通知：

1. 唤醒：定时巡检（间隔 ≥4 小时）或账本事件；
2. 候选：任务到点、偏离超带宽、新钱到账、承诺到期；
3. 开口：静音时段、去重、每日/每周上限、全局开关。**到点任务例外**——
   它是用户自己设的闹钟，不受开关、频控和静音约束，避免提醒被静默吞掉。

候选为空时，`mns-butlerd` 还会试一次 agent 主动开口（`agent-init`）；它走
同一本 ledger 的同一套额度，`status` 里能看到 `自主开口` 这个由头。

`status` 里的「近 7 日开口」就是第 3 步的结果记录，可直接复核。频控默认
每天 3 次、每周 10 次、去重 3 天、静音 22–08 点，可在
`mns config set butler.daily_cap 3` 一类命令或设置页里改，守护进程下次判定
即生效。

守护进程在跑时（`mns-butlerd`），`wake`、`mute`、`unmute` 会自动转交给它
执行，不在本地另跑一遍——两个进程各跑一遍会重复消耗频控额度，还会互相
覆盖 `state.json`。输出里会写明「已交由守护进程」。守护进程没在跑时才由
CLI 自己写文件。

## 文案边界

管家的会话是自主人格：观点、倾向、市场看法都允许，不查策略性话术。保留的
硬底线：非空、长度（agent 自由文本 ≤2000 字、模板 ≤300 字）、**不谎称能替
用户下单**（没有券商通道却说能下单会退回模板；「下单得你自己来」这类诚实
否定不算）。数字同源靠提示词约束，账本事实由 MNS 在首轮注入；agent 也可以自己用
`mns` 只读子命令查，拿到的数可能比摘要细，所以**不做逐项数字比对**，硬比对会把正确回答
误判成越界。提醒必须走 `mns-butler task add` 建到点任务；使用默认 opencode 时 MNS 只拦
会改账本的 mns 子命令和 `init`，换其他 ACP agent 时权限由该 agent 自己的配置决定。

通知 banner（≤60 字、≤2 行）仍走最严口径：含买卖点、择时、收益承诺的文案
一律退回模板——弹窗是打扰通道，与会话是两种礼貌。这条边界在
`src/butler/talk.rs` 与 `src/butler/schedule.rs` 里有测试。
