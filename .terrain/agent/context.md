---
type: agent_context
project: money-never-sleep
title: Agent Architecture Context
source: .
---

## 项目概览


MNS（Money Never Sleeps，v0.9.0）是本机运行的个人投资决策工具（Rust），服务于中长线逆情绪投资者。五个二进制共享同一引擎库 `money_never_sleep`：CLI `mns`、GPUII have enough evidence to update the document. Producing the full revised markdown now.

## 项目概览


MNS（Money Never Sleeps，v0.9.0）是本机运行的个人投资决策工具（Rust），服务于中长线逆情绪投资者。五个二进制共享同一引擎库 `money_never_sleep`：CLI `mns`、GPUI 桌面壳 `mns-gui`（`gui` feature）、独立帮助中心 `mns-help`、管家审计 CLI `mns-butler`、常驻守护 `mns-butlerd`（`agent` feature）。核心命题：把 CNN 恐贪指数（0–100）映射为风险资产目标仓位（越恐慌越高，35%–85%），再按**逐腿**偏离带（默认 ±4pp）给出买/卖/持有建议；情绪只设定敞口，已被证伪为择时依据。v0.9 增补「管家」：常驻人格，只在唤醒源（定时巡检/账本事件/任务与承诺到期/agent-init 失败重试）成立时评估是否开口，「说不说」全程确定性、可离线复算。硬约束：零券商连接、零服务端、零模型/凭据托管；报告先留档再决策；`~/.mns/` 是唯一可写根，任何写入都经 `config::resolve_data_path()`，不依赖 cwd。

## 架构设计


| 层 | 容器 | 职责 | 关键路径 |
|---|---|---|---|
| 交互层 | clap CLI | 串行命令与参数、终端表格渲染 | `src/cli.rs`、`src/main.rs` |
| 交互层 | GPUI 桌面壳 | 窗口/托盘、六页路由、后台 jobs、登录自启动、二次启动唤起、`--butler` 直开管家窗 | `src/bin/mns-gui.rs`、`src/gui/**` |
| 交互层 | 管家独立窗口 | 人设化对话、流式回答、记忆意图解析、任务与记忆面板、60s 事实重算 | `src/gui/butler.rs` |
| 交互层 | 帮助中心 | 左目录右正文、前进/后退、深链跳页、截图运行时加载 | `src/bin/mns-help.rs`、`src/help/**` |
| 交互层 | 管家 CLI/守护进程 | 审计式复算；事件驱动常驻、关窗后继续判定、长连接回告 GUI | `src/bin/mns-butler.rs`、`src/bin/mns-butlerd.rs` |
| 入口层 | 库入口 | 导出核心模块；常驻 `app`/`butler`/`ipc`/`notify`，按 feature 暴露 `agent`/`gui`/`help`/`single_instance` | `src/lib.rs` |
| 应用服务层 | typed 视图 | `Snapshot`/`PlanView`/`Freshness` 等快照，避免前端各自拼账本；内置年度纪律复盘 | `src/app/mod.rs` |
| 管家层 | 确定性内核 | 唤醒调度、候选生成、开口闸门、发言台账、记忆/任务/会话、模板兜底、跨进程写锁 | `src/butler/**` |
| 助手层 | 外部 ACP 运行时 | 驱动外部 ACP CLI（默认 `opencode`，追加 `acp`），按 `Profile` 写入工作区契约并组装巡检/对话/主动开口提示词 | `src/agent/{mod,runner,routine,workspace}.rs` |
| 引擎层 | 纯规则/纯统计 | 信号、再平衡/定投、护栏、纪律复盘、报告、审计 | `src/{signal,strategy,guardrail,discipline,report,audit,backtest}.rs` |
| 数据访问层 | SQLite 单写者 | schema 初始化、upsert、按日/按起始日查询 | `src/db.rs`、`src/models.rs` |
| 外部数据层 | HTTP 抓取 | 恐贪指数、基金/行情净值，多源回退 | `src/sentiment.rs`、`src/quote.rs`、`src/market.rs`、`src/live_data.rs` |

- 依赖方向单向：交互/入口 → 应用服务 → 引擎 →（数据访问｜外部数据）；`discipline` 只消费 `db` 流水与 `config.backtest` 预算；`butler` 只消费 `app::Snapshot` 派生的事实，不直接读库。
- feature 门控链：`agent` → `gui`（隐含 `agent`）→ `gui-test`（headless 布局测试）；`butler`/`ipc`/`notify` 常驻编译；`mns-butler` 无 feature 要求，`mns-butlerd` 需 `agent`。
- 工作区契约是助手层的新底座：`agent::workspace` 只写 MNS 自有的 `~/.mns/agent/**`，把常驻规则放进 `AGENTS.md` 的受管块（`<!-- mns:begin v1 -->`…`<!-- mns:end -->`，块外是用户文本），并给 `CLAUDE.md`/`GEMINI.md` 补一行 `@AGENTS.md` 导入；同时安装 `mns-account`/`mns-butler` 两个技能到 `.agents/skills` 与 `.claude/skills`。想强制更新，只要改动内容不同，否则保持 mtime 稳定、整块用临时文件 + rename 替换。用户自选 `working_dir` 一律不写，改由 prompt 兜底全部规则。
- 管家是两层分离：**唤醒**（permission to think，仅定时巡检/账本事件/任务与承诺到期/agent-init 重试）与**开口**（duty to speak，多数唤醒的正确答案是沉默），两层都是纯函数 + 可持久化状态，模型不参与；表达层（ACP）与通知层只是消费者。
- 管家已不再只是只认可事实的聊天框：它读取的是同一个工作区契约（Butler Profile 版）中的 `mns-butler` 技能，可以只读查询账户、并把用户口头提醒真的落成 `mns-butler task add` 任务；护栏靠 opencode 注入的 deny 规则（`OPENCODE_CONFIG_CONTENT`）尽力限制（best-effort，非沙箱）。
- 帮助中心是**独立进程**，复用 `gui::theme`/`gui::components`（提升为 `pub(crate)` 共享），只读渲染，不持有任何投资规则；`help/content.rs` 用 `include_str!` 在编译期把 `help/*.md` 内嵌进二进制。
- 跨进程协作有三种机制：本地 IPC 通道（Unix `<数据目录>/run/<app>.sock`，Windows `\\.\pipe\mns-<app>-<用户名>`，NDJSON 一行一条，支持一次性 `poke` 与双向长连接；**绑定失败即已有实例**、**连接断开即对方死亡**）；Windows 命名互斥体 + 命名事件（可携带二次启动意图：主窗/管家窗）；文件型深链 `~/.mns/help-request`（temp + rename 原子写、mtime 判过期）。
- 平台适配集中在 `gui::command`（Windows `CREATE_NO_WINDOW`）、`gui::autostart`（只写系统登录项）、`notify`（macOS `UNUserNotificationCenter`）、`build.rs`（仅 Windows 给 `mns-gui`/`mns-help`/`mns-butlerd` 嵌 `app`/`help`/`butler` 三套 `.ico` 资源；CLI `mns`/`mns-butler` 保留系统默认图标）。

## 模块地图


| Module | 责任 | 主要路径 |
|---|---|---|
| 库入口/feature 导出 | 常驻 `app`/`butler`/`ipc`/`notify`；feature 门控 `agent`/`gui`/`help`/`single_instance` | `src/lib.rs` |
| CLI 命令树与分发 | `cmd_*` 串行装配：`report/today/plan/guard/performance`；backtest 默认审计 + `mirror/asset/update-data/validate-full/legacy/validate`；cash `deposit/withdraw/set`；`butler.*` 参数校验 | `src/cli.rs`、`src/main.rs` |
| 应用服务层 | typed 只读视图与服务、`BuyGuardrail::build_note` 前缀生成 | `src/app/mod.rs` |
| 桌面壳（`gui`） | GPUI 窗口/托盘、页面路由（总览/持仓/计划/复盘/助手/设置）、jobs、每日巡检调度、IPC 监听、`--butler` 意图、隐藏启动与窗口重建 | `src/bin/mns-gui.rs`、`src/gui/mod.rs`、`src/gui/pages/*.rs`、`src/gui/jobs.rs` |
| 管家窗口（`gui`） | 独立 `ButlerView`：流式正文、`TurnState` 防串话、记忆意图拦截（`handle_memory`）、任务/记忆面板、打开即交接事实 | `src/gui/butler.rs` |
| 管家确定性内核 | 候选、开口闸门、发言台账、记忆上限/归档、任务板（Check/Talk、Butler/User 来源）、下一次唤醒、会话 200 条、行为日志、原子 JSON、`writer_lock` 跨进程写锁 | `src/butler/**` |
| 管家进程与审计 CLI | 事件驱动常驻循环、GUI 长连接回告、`outbox.jsonl`/`pending.jsonl`、`agent-init` 与退避重试、`status/wake/tick/remember/forget/memory/tasks/task add/done/outbox/mute/unmute` | `src/bin/mns-butlerd.rs`、`src/bin/mns-butler.rs` |
| 助手运行时 | ACP 连接/会话/追问、权限答案、时间线、`mns-decision` 解析、`Profile` 决定的契约与权限集 | `src/agent/runner.rs`、`src/agent/mod.rs`、`src/agent/routine.rs` |
| 工作区契约 | 受管 `AGENTS.md` 块、`CLAUDE.md`/`GEMINI.md` 导入、`mns-account`/`mns-butler` 技能安装、只在 `~/.mns/agent/**` 生效 | `src/agent/workspace.rs` |
| 本地 IPC 与通知 | `Signal`/`Event`/`listen`/`connect`/`Client`/`poke_daemon`、macOS 横幅与点击回传、Windows 单实例 | `src/ipc.rs`、`src/notify.rs`、`src/single_instance.rs` |
| 帮助中心（`gui`） | `mns-help` 入口、目录树/正文、键盘与菜单绑定、macOS 菜单改名、深链请求；自研 Markdown 子集解析与 GPUI 渲染、`ImageCache` | `src/bin/mns-help.rs`、`src/help/*.rs`、`src/gui/help_links.rs` |
| 配置与存储 | `AppConfig`（曲线/档位/带宽/成本/回测预算/`[butler]` 频控）、`zone_target_weights`、数据根解析、SQLite schema | `src/config.rs`、`src/db.rs`、`src/models.rs` |
| 信号/策略/纪律/报告/审计 | 7 天平滑 + 3 天区间确认 + 决策日；逐腿带宽再平衡；冷静期护栏；年度交易预算复盘；中文章节报告；实盘审计与 point-in-time 数据集 | `src/{signal,strategy,guardrail,discipline,report,audit,backtest,live_data,metrics}.rs` |

## 核心流程


**① 日常决策与观察（`update-prices` → `report`/`today`；`plan`；`guard`）**
1. 抓取 CNN 恐贪指数，解析 score/rating，当日快照落 `fear_greed_snapshots`。
2. `signal::compute` 读历史算有效决策分数（平滑、区间确认、决策日判定），历史不足自动降级。
3. 决策日或 `--decide`：用有效分数驱动策略，按**每腿**偏离与带宽生成计划（先卖后买，卖出回笼计入买入预算；缺标的清零并挂原因）。
4. 报告分决策版/观察版存档 `reports/YYYY-MM-DD.txt`；`plan`/`guard` 只读展示，护栏仅提示不阻断。

**② 记账与纪律闭环**
1. 外部资金进出用 `cash deposit/withdraw`（XIRR 口径）；`cash set` 仅纠错。
2. `add/buy/sell` 只登记已在券商成交的交易并更新现金/成本；误记会污染下游。
3. 桌面录入买入触发冷静期时，理由带结构化前缀（`计划外冷静：`/`过热冷静：`）落库。
4. `discipline::review` 读全年流水（无行数上限）分桶，复盘页对照 `backtest.annual_trade_budget_min/max`；未标记记录如实标注，不推断意图。

**③ 审计与验证**
1. `backtest update-data` 用 Python 3 重建 point-in-time 日频 CSV/meta（抓取日、各腿起止、stale 天数），失败不静默覆盖。
2. 默认审计直接调用生产 `signal::compute` 与策略，按 `execution_lag_days` 成交、扣通道成本、输出实际/预期回撤双口径与账户镜像。
3. `mirror`/`asset` 做账户镜像与单资产对照；`legacy` 仅历史归档，结论不迁移到实盘。

**④ 桌面壳、助手巡检与帮助中心**
1. `mns-gui` 先占单实例锁与本地通道：二次启动按意图（默认置前、`--butler` 直开管家窗口）通知已有实例后退出；Windows 侧由专用线程阻塞在命名事件上等待，不做轮询。cwd 固定到 `~/.mns`，`--tray`/`--background` 隐藏启动只驻留托盘。
2. 页面从 `app` 读 typed 快照；刷新/记账/巡检走 `gui::jobs`，派生进程统一走 `gui::command`（Windows 无黑框）。
3. `maybe_start_daily` 每日最多一次（`agent/last-review`）：用随包 `mns today --quiet` 生成简报，`routine::daily_review_prompt` 组装提示词（报告为唯一数据源、只读命令、`mns-decision` 输出契约）提交 ACP；每轮结束提取最后一个带标签围栏解析为 `DecisionReport`，渲染卡片并存档。
4. 帮助入口：页面顶栏（"这一屏怎么用"）与"数字来历说不清"的卡片（"这个数怎么来"）各挂一个 `?`，slug 集中在 `help_links` 并被测试校验。
5. 点击 `?` 先写深链请求文件，再按子进程句柄决定：无窗口则启动 `mns-help --page <slug>`，有窗口则靠 watcher 翻页并置前；`mns-help` 取自身单实例锁，`QuitMode::LastWindowClosed`，关窗即退出以便下次可重开。
6. 正文在编译期内嵌，截图运行时从 exe 同级 `help/`（macOS 为 `Contents/Resources/help`）按目录层级向上搜索 5 级，缺失时渲染带预期路径的虚线占位。

**⑤ 助手会话与工作区契约**
1. `run_session` 先 `resolve_cwd`：配置为空时落在固定工作区 `~/.mns/agent/workspace`（绝不取进程 cwd）。
2. 由 `GoalSource` 推出 `Profile`：`Conversation` → `Butler`，`Manual`/`DailyReview` → `Task`；Profile 同时决定工作区契约文本与权限集。
3. `workspace::bootstrap` 仅当 cwd 在 `~/.mns/agent/**` 之下才写入 `AGENTS.md` 受管块、`CLAUDE.md`/`GEMINI.md` 的 `@AGENTS.md` 导入，以及两个技能；返回 false（用户自选目录/写入失败）时 prompt 保留完整规则，并在时间线说明「改用 prompt 兜底」。
4. `launch_config` 追加 `acp`、拆分含空格的命令行、注入 `OPENCODE_CONFIG_CONTENT`（模型 + 账本写命令 deny 规则）；同时把 `mns` 绝对路径与 PATH 交给子进程，避免 agent 用 `find` 找 CLI 或直接 `sqlite3` 开库。

**⑥ 管家：唤醒 → 开口 → 送达**
1. GUI 把 `Snapshot` 提取为 `AccountFacts`（逐腿偏离、带宽、决策日、今日入金、计划可执行、承诺到期），写入 `agent/butler/facts.json`，并经 IPC 唤醒守护进程；记账、加任务、改记忆同样 poke。
2. `mns-butlerd` 做唤醒判定（定时巡检间隔夹到 ≥4h、账本事件、任务/承诺到期、agent-init 失败重试），不轮询：`agenda::next` 取五类变体中最早时刻睡过去，单次睡眠 ≤30min 以免系统休眠漂移。
3. 候选只认三类由头：任务到点、可执行的下一步（决策日超带宽 / 新钱到账且计划有单）、承诺到期；模板文案先过通知口径，过不了退兜底。
4. 开口闸门按固定顺序：全局开关 → 静音时段 → 去重（同一 trigger N 天）→ 每日/每周上限；未通过记 `SilentBecause`，`status` 可复算。
5. 通过则写 `outbox.jsonl`（审计）与 `pending.jsonl`（投递），并经常驻长连接发 `Pending` 让 GUI 把消息并进会话（`Hello` 握手、断线自动重连、退出前发 `Bye`）；**系统通知只由 GUI 送达**（macOS 拒绝 helper 可执行文件）。
6. 候选为空时守护进程可按需起一次外部 ACP `agent-init`（120s 硬超时、与上次会话间隔 ≥12h、共用同一份额度）；失败按 15min → 1h → 6h 退避并记 `initlog.jsonl`，同时把重试时刻排进唤醒表；回答须过 banner 口径，否则本轮沉默且不写记录。
7. 窗口对话走 `butler::talk`：首轮人设 + 账本事实 + 记忆与最近对话写进 goal，后续 `Ask` 复用同一 session；agent 失败/超时/越界退回 `expression` 本地模板。管家可给倾向性观点（自我标注），但数字同源、不谎称能下单。
8. 「记住…/忘掉…」在消息进入 agent **之前**由 `gui::butler::handle_memory` 用 `memory::parse_intent` 确定性解析并落盘（含「记过了」「对上了多条」等回执），不进普通问答，也不占 ACP 额度。
9. 到点任务分两种：`Check` 复读「到点检查什么」，`Talk` 只送一句问候；`TaskOrigin::User` 的提醒跳过措辞校验与每日额度，但同一 `key` 仍只允许一个任务。提醒能否送达取决于 MNS 是否开着（托盘也算）及静音/开关/额度。
10. `mns-butler` 只读/改正状态文件；守护进程在跑时 `wake`/`mute`/`unmute`/`task add` 转交给它，避免两个进程重复消耗额度并互覆 `state.json`；三者写状态时统一持 `writers.lock`。

## 技术选型


- **语言**：Rust edition 2024；包 `money-never-sleep` 0.9.0，库 `money_never_sleep`，五个二进制（`mns` / `mns-gui` / `mns-help` / `mns-butler` / `mns-butlerd`）。
- **CLI**：clap 4（derive 子命令树）；`mns-butler` 复用同一风格，输出面向审计。
- **桌面 UI（`gui`）**：`gpui-kit 0.9.0` + `gpui-tray 0.3.0`（`gpui-kit`/`menu-state`，统一到 `gpui-pre`）；`gui-test` 打开 `gpui-pre/test-support` 做 headless 布局断言；管家头像/纸纹用 `include_bytes!` + `OnceLock` 只解码一次。
- **外部助手（`agent`）**：`agent-client-protocol 5.0`（`process`，ACP over stdio）；权限策略三档（自主运行/保护写入/逐项确认）；`GoalSource` 区分手动会话、每日巡检、管家对话与主动开口；工作区契约靠 `AGENTS.md`（ACP 无稳定 system-prompt 槽，`initialize` 不声明 `sessionCapabilities.systemPrompt`），已对照 `opencode acp` 2.2.5 验证。
- **管家内核**：纯 Rust 确定性实现（chrono 本地时间、serde_json 小文件 temp+rename、JSONL 追加），不依赖网络与模型；三个写者用 `writers.lock` 文件锁 + 带 pid 的临时名做原子替换；系统通知仅 macOS 用 `objc2` + `objc2-user-notifications` + `block2`。
- **本地 IPC**：tokio + NDJSON over Unix `SOCK_STREAM` / Windows 命名管道，不引第三方 IPC 库；行长上限 64KB；`Client` 长连接以断开/`Bye` 作为存活信号，无心跳轮询。
- **帮助中心**：自研 Markdown 子集解析器（不引依赖，语法可预期）+ GPUI 渲染；仅 macOS 用 `objc2` 重设前台菜单名；新增「管家」「管家审计」两页。
- **存储**：rusqlite（bundled）单文件 `~/.mns/mns.db`；`serde + toml` 管 `config.toml` 与 `gui.toml`（temp+rename 原子写）；管家状态独立小 JSON/JSONL。
- **网络/时间/输出**：reqwest（rustls）、chrono（`NaiveDate` 日频口径）、comfy-table + unicode-width（中英混排列宽）。
- **错误**：anyhow，统一 `Error: <中文原因>`，CLI 非零退出。
- **打包**：`scripts/make-dmg.sh`（五可执行同版本入 `.app`，截图进 `Contents/Resources` 以过 codesign）、`scripts/make-installer.ps1` + `scripts/installer/mns.iss`（按用户安装、无 UAC、不改 PATH、附帮助中心快捷方式）、`scripts/make-help-diagrams.sh` 生成 `help/diagrams/*.svg`、`make-icons.swift` 生成图标、`make-texture.py` 生成管家纸纹与头像资源；`build.rs` 在 Windows 上按二进制嵌 `app`/`help`/`butler` 三套 ID-1 图标。

## 系统边界


| 边界 | 对象 | 说明 / 信任边界 |
|---|---|---|
| 外部 API | CNN Fear & Greed `production.dataviz.cnn.io/index/fearandgreed/graphdata` | 只读、需网络、可 418；决策与数据集共同来源 |
| 外部 API | 天天基金 / 东财 / Yahoo Finance | 净值与价格多源回退；单标的缺失时跳过，Yahoo 可能 403（视为环境限制） |
| 外部进程 | ACP 兼容 CLI（默认 `opencode acp`） | MNS 只提供 CLI 路径、可选模型名、工作目录与权限策略；**不持有模型/端点/API Key**；助手页走严格护栏，管家会话是自主人格（可给倾向性观点、自我标注），通知 banner 仍走最严口径；账本写命令的 deny 规则经 `OPENCODE_CONFIG_CONTENT` 注入，仅对 opencode 生效 |
| 本地进程 | 随包 `mns`、`mns-butler`、`mns-help`、`mns-butlerd`、`reg.exe`、`open`/`xdg-open` | 由 `gui::command` 派生；仅写用户级 HKCU；守护进程由 GUI 拉起/停止，崩溃由 IPC 断连即时发现 |
| 本地 IPC | `~/.mns/run/{mns-gui,mns-butlerd}.sock`、Windows 命名管道 | NDJSON ≤64KB/行；socket 目录 0700，只有本用户能连；绑定失败即已有实例；GUI↔守护为长连接（`Hello`/`Bye`），**不发系统通知、不读库** |
| 系统通知 | macOS `UNUserNotificationCenter` | 只有 app 主可执行文件能授权送达；守护进程只判定，GUI 负责发送；点击经通道回传打开管家；Windows/Linux 暂走托盘 |
| 跨进程 IPC | `~/.mns/help-request`、命名互斥体/事件（含二次启动意图） | 原子写 + mtime 判定，失败模式是"停在当前页"而非窗口打不开；单实例锁按登录会话隔离（`Local\`） |
| 工作区写入 | `~/.mns/agent/workspace`、`~/.mns/agent/**` 下的 `AGENTS.md`/`CLAUDE.md`/`GEMINI.md`/`.agents/skills`/`.claude/skills` | MNS 只写自有目录；受管块之外视为用户文本；用户自选 `working_dir` 绝不写入，改为 prompt 兜底；契约是约定不是沙箱 |
| 本地存储 | SQLite 账本、`config.toml`、`gui.toml`、`agent/{workspace,conversations,runs,last-review}`、`agent/butler/{state,memory,tasks,facts,writers.lock,outbox,pending,initlog}`、`agent/events.jsonl`、`reports/`、`help-request`、系统登录项 | 本地可信单写者；管家事实只经 `facts.json` 交接（守护进程无 `rusqlite`/`app` 依赖）；`outbox/pending/initlog` 为 JSONL 追加；删除 `agent/butler/` 即"忘掉一切" |
| 数据集构建 | `update-data` + Python 3 + 网络 | 写 CSV/meta；失败不得静默污染既有数据 |
| 人机边界 | 用户 / 券商 | MNS 只记账与建议，不下单、不阻断；管家的记忆/任务/通知同样不自动执行交易，提醒必须真的建任务、看到「已接受」才可声张，责任在用户 |

## 代码映射索引


| Concept | Location | Notes |
|---|---|---|
| 库入口/feature 导出 | `src/lib.rs` | `app`/`butler`/`ipc`/`notify` 常驻；`agent`/`gui`/`help`/`single_instance` 按 feature |
| CLI 命令树与分发 | `src/cli.rs`、`src/main.rs` | `CashAction`、`BacktestAction`、`ConfigAction`（校验 `butler.*`）；`today --quiet` 供巡检复用 |
| 应用快照/服务 | `src/app/mod.rs` | `Snapshot`/`PlanView`/`load_snapshot`；管家事实的唯一来源 |
| 决策信号 | `src/signal.rs` | `compute`（平滑/区间确认/决策日）、`next_decision_day`；生产与审计同源 |
| 再平衡计划与报告 | `src/strategy.rs`、`src/report.rs` | 逐腿偏离带、先卖后买、`NO_INSTRUMENT_REASON`；报告"按腿判定"与残余偏离 |
| 配置/档位目标 | `src/config.rs` | `zone_target_weights()`、`effective_neutral_weight()`、`ButlerConfig`（日/周上限、去重、静音） |
| 纪律复盘 | `src/discipline.rs`、`src/gui/pages/performance.rs` | 前缀常量 + `bucket_of`；`discipline_card` 对照年度预算 |
| 审计/数据集 | `src/audit.rs`、`src/backtest.rs`、`src/live_data.rs`、`src/metrics.rs` | 实盘审计、legacy 对照、point-in-time CSV 契约与指标 |
| 桌面壳入口 | `src/bin/mns-gui.rs`、`src/gui/mod.rs` | `run()`、`ensure_window`、IPC 监听、`--butler` 意图、`open_help_center`、托盘/菜单（含管家入口与停止守护） |
| 助手运行时 | `src/agent/runner.rs`、`src/agent/mod.rs` | `start_runtime`、`run_session`、`resolve_cwd`、`compose_goal`（任务/对话双契约）、`extract_decision`、`launch_config` + `agent_config_env`（模型与 deny 规则） |
| 工作区契约 | `src/agent/workspace.rs` | `Profile{Task,Butler}`、`Binaries`、`bootstrap`（受管块/兼容导入/技能安装，`write_if_changed` 保 mtime） |
| 管家内核 | `src/butler/{orchestrator,schedule,tasks,memory,agenda,session,events,store}.rs` | `AccountFacts`/`candidates`；`WakeScheduler`/`SpeakPolicy`/`SpeakLedger`；任务 `Check`/`Talk` + `Butler`/`User`、活跃 10、日新建 2、记忆 100/归档 200、会话 200；`WakeSource` 五变体；原子 JSON + `writer_lock` |
| 管家表达与兜底 | `src/butler/{talk,expression}.rs` | 首轮人设 + 账本事实 + 记忆/最近对话、`Intent` 关键词路由、`adopt_*` 数字白名单、拒空答 |
| 管家进程/CLI | `src/bin/mns-butler.rs`、`src/bin/mns-butlerd.rs` | 审计子命令（status/wake/tick/remember/memory/forget/tasks/task/outbox/mute/unmute）；事件驱动循环、`facts.json` 年龄闸、GUI 长连接、`agent-init` 退避重试 |
| 本地 IPC/通知/单实例 | `src/ipc.rs`、`src/notify.rs`、`src/single_instance.rs` | `Signal`/`Event`/`Client`/`poke_daemon`；macOS 横幅与点击回传；命名互斥体 + 命名事件 + `Intent` |
| 帮助中心/内容/深链 | `src/bin/mns-help.rs`、`src/help/{mod,content,markdown,render,macos,deeplink}.rs`、`src/gui/help_links.rs` | `--page <slug>`、`include_str!` 内嵌页、`ImageCache` 搜索根、`HelpLink{slug,tip}` |
| 打包/图标/素材 | `build.rs`、`scripts/{make-dmg.sh,make-installer.ps1,installer/mns.iss,make-help-diagrams.sh,make-texture.py}` | 五可执行 + `help/` 资源 + 管家素材；Windows 按二进制嵌 app/help/butler 图标 |