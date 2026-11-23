---
type: agent_context
project: money-never-sleep
title: Agent Architecture Context
source: .
---

## 项目概览

MNS（Money Never Sleeps）是本机运行的个人逆情绪投资决策助手，服务中长线投资者。投资引擎只实现一次，集中在 Rust 库 `money_never_sleep`；`mns`（CLI）、`mns-gui`（桌面壳）、`mns-butler`（审计 CLI）、`mns-butlerd`（常驻守护进程）均为其呈现或编排层，不重复任何投资规则。关键约束：不做自动交易；全部可写数据固定在单一根 `~/.mns/`（与进程 cwd 无关）；管家「说不说」必须可确定性复算、不依赖模型；MNS 不持有推理端点与 API key，仅外接 ACP agent；情绪信号已被证伪为择时依据，通知与报告不得给出买卖点。

## 架构设计

| 层次 | 职责 | 主要依赖 |
|------|------|----------|
| 引擎层（`lib.rs`） | 配置、账本、策略、信号、情绪、行情、绩效、回测、报告、纪律、护栏、审计；唯一规则来源 | `rusqlite`、`reqwest`、`chrono` |
| 应用服务层（`app/`） | 无呈现的类型化服务（`Snapshot` / `Freshness`），CLI 与 GUI 共用 | 依赖引擎各模块 |
| 呈现层 | CLI（`main.rs` 仅解析参数与表格）、GPUI 桌面壳、帮助查看器 | `clap`、`comfy-table`、`gpui-kit`、`gpui-tray` |
| 管家子系统（`butler/`） | 确定性唤醒/开口判定、记忆、任务、会话、表达兜底、巡检排程与报告 | 只读 SQLite、小 JSON 状态 |
| 外部 Agent（`agent/`） | ACP runtime、prompt 契约、工作区 `AGENTS.md` 托管 | `agent-client-protocol` |

**进程模型**

| 进程/容器 | 角色 | 生命周期 |
|-----------|------|----------|
| `mns` | 终端命令入口，薄呈现层 | 短命 |
| `mns-gui` | 常驻宿主：托盘、菜单栏、Dock、通知投递、守护进程启停；二次启动只 poke | 常驻（无 AppShell 窗口） |
| `mns-gui --window [--page]` / `--window --butler [--notice <id>] [--ref <id>] [--ask <句>]` | 主窗 / 管家窗，独立单实例通道 | 关窗即退出（Accessory、无第二 Dock 图标） |
| `mns-butler` | 管家确定性内核审计入口 | 短命 |
| `mns-butlerd` | 事件驱动守护进程：只读账本、唯一会话写者、定时巡检执行者、发通知前判定 | 常驻（由 GUI 拉起） |
| `mns-help` | 独立 CHM 风格帮助查看器 | 短命 |

**跨进程通道**：`ipc.rs` 定义 NDJSON `Signal` 协议（行长上限 1MB），Unix 走 `SOCK_STREAM`、Windows 走命名管道 `\\.\pipe\mns-<app>-<用户>`；连接关闭即存活信号。宿主、主窗、管家窗各用独立 `ipc::listen` 通道做单实例锁。管家窗口与守护进程之间是双向长连接（上行 `Chat*`，下行 `ChatSnapshot`/`ChatDelta`），窗口断开即 detach。连接拆除是整条收摊：每连接出站队列（64 帧）满或写半边收摊即断，调用方只看到「连接没了」，重连后由 `ChatAttach` 重建全量快照；`Signal::Bye` 是主动告别，不必等断开。

## 模块地图

| 模块 | 职责 | 主要路径 |
|------|------|----------|
| 核心引擎库面 | 单一规则来源，导出全部引擎模块 | `src/lib.rs` |
| 应用服务 | 无呈现的类型化快照与新鲜度，供 CLI/GUI 共用 | `src/app/mod.rs` |
| 配置与账本 | `~/.mns` 路径解析、config.toml、SQLite（含只读连接） | `src/config.rs`、`src/db.rs`、`src/models.rs` |
| 策略与信号 | 目标仓位曲线、三腿再平衡、FGI 平滑/区间确认/决策日 | `src/strategy.rs`、`src/signal.rs` |
| 情绪与行情 | CNN 恐贪指数、基金/指数报价、市场概况 | `src/sentiment.rs`、`src/quote.rs`、`src/market.rs` |
| 报告与绩效 | 每日报告留档、XIRR/回撤、纪律复盘、买入冷静期 | `src/report.rs`、`src/metrics.rs`、`src/discipline.rs`、`src/guardrail.rs` |
| 回测与审计 | 月频代理回测、实盘 point-in-time 日频审计 | `src/backtest.rs`、`src/audit.rs`、`src/live_data.rs` |
| 桌面壳 | 六个页面、托盘/Dock/菜单、后台 worker | `src/gui/`、`src/gui/pages/` |
| 管家确定性内核 | 唤醒调度、开口判定、记忆、任务、状态落盘 | `src/butler/`（`schedule`/`orchestrator`/`memory`/`tasks`/`agenda`/`store`） |
| 管家会话与表达 | 会话唯一写者、意图路由模板、ACP 对话适配、巡检排程与报告存档 | `src/butler/`（`chat`/`expression`/`talk`/`review`/`session`） |
| 外部 Agent | ACP runtime、prompt 契约（巡检 prompt 的 `questions` 上限 0–2）、工作区契约托管 | `src/agent/`（`runner`/`routine`/`workspace`） |
| IPC、简报与通知 | 本地唤醒、托盘简报数据契约、系统通知送达 | `src/ipc.rs`、`src/brief.rs`、`src/notify.rs` |

## 核心流程

**1. 每日决策报告（`mns today` / `mns report`）**

1. 抓取并落库当日 CNN 恐贪指数快照（`fear_greed_snapshots`），批量更新持仓最新价；失败则降级为本地快照。
2. `signal::compute` 生成决策信号：最近 N 天平滑 + 连续区间确认 + 决策日过滤（默认周五，非决策日出观察版）。
3. `strategy::calculate_rebalance_plan` 计算三腿偏离、目标仓位与调仓指令，按偏离带（默认 ±4pp）与「先卖后买」排序；无可买标的时挂 `NO_INSTRUMENT_REASON`。
4. `guardrail` 仅在过热时给冷静期提示，不阻断交易、不改仓位。
5. 报告先留档 `reports/YYYY-MM-DD.txt` 再输出，含风险警告与「情绪信号贡献接近零」等边界说明。

**2. 管家主动开口（`mns-butlerd`）**

1. `agenda` 汇总定时巡检、每日巡检、任务、承诺、`agent-init` 重试五类唤醒源，取最早时刻睡到点；每日巡检时刻由 `review::schedule()`（纯函数，读 `agent/last-review` 与 `last-review-attempt.json`）给出，账本事件/状态变更由 GUI/CLI 经 `Signal::Wake` 立即叫醒。
2. `orchestrator` 用 `Database::open_read_only()` 从账本算 `AccountFacts`（与 CLI/GUI 同源 `Snapshot`），不读 `facts.json`；读库在阻塞线程池内完成，成败都写健康标记供托盘红灯。
3. `schedule` 两层判定：唤醒许可（`WakeSource` → `WakeVerdict`）与开口义务（`SpeakDecision`，多数唤醒应答「沉默」）。
4. `WakeSource::DailyReview` 不进确定性判定：到点先跑 `mns today --quiet` 刷价留档，再起一轮 ACP 复盘，报告写 `agent/reviews/<id>.json`、向会话追加一条消息、投一条 banner（文案不引用结论）；无人值守，权限请求一律拒绝、prompt 不提供 CLI 入口；当天最多 3 次尝试（失败按 30 分钟退避），`last-review` 只在成功时写。
5. 开口则写 `outbox.jsonl`（审计）与 `pending.jsonl`（投递），并入 `conversation.json`；经 `DeliverNotice` 交宿主，宿主按 `notices.jsonl` 去重后由 GUI 发系统通知。
6. 确定性内核无由头时可起一次 ACP `agent-init`（120 秒超时、≤60 字、无买卖话术）；失败按 15 分钟 → 1 小时 → 6 小时退避，runtime 每轮 `Shutdown`。

**3. 桌面 GUI 数据流**

1. 宿主常驻管理托盘、菜单栏、Dock、通知与守护进程；点 Dock/二次启动经 `Signal::Open`/`OpenPage` 置前或拉起窗口。
2. 窗口只呈现：所有数字走 `app::Snapshot`；SQLite 读写、网络刷新、文件写入统一交给 `jobs` worker 线程，不阻塞 GPUI 主线程。
3. 托盘简报走 `Signal::RequestBrief` → `butlerd` → `Signal::Brief`（`brief::BriefData`），宿主只排版；管家健康随 `Brief` 回传，异常时亮红灯并换带红点 Dock 图标。
4. 通知点击带 `notice_id` 回宿主，记入 `notices.jsonl` 后拉起管家窗并定位消息。
5. 巡检页只读回放，接力不搬正文：`Signal::ButlerRef { id, question }` → 宿主转给已开的管家窗，或拉起 `--window butler --ref <id> [--ask <句>]`；窗口按 id 读标题与正文，把结论里的第一条「待确认」预填进空输入框（不覆盖已有输入）。会话内 `@` 引用最近巡检时 IPC 只传 `{id,title}`，守护进程按 id 读正文（最多 3 份、各自截断）。

**4. 回测与审计链**

1. `mns backtest update-data` 重建 point-in-time 日频数据集：FGI 取严格早于当日的已公布值，净值取不晚于当日的已披露值，消除前视偏差。
2. `audit` 复用 `signal::compute`（与报告同源），按通道建模 T+n 结算、赎回阶梯与限购，回撤采用剔除资金流的复合净值主口径。
3. `mirror` / `asset` / `validate-full` 输出审计结论；旧月频 `legacy` 已降级，结论不可迁移到实盘。

## 技术选型

- Rust edition 2024；一个 lib + 多个 bin，feature 门控：`agent`、`gui`、`gui-test`。
- CLI：`clap` 4（derive）；异步：`tokio` 1（full）；序列化：`serde` / `serde_json` / `toml`。
- 数据：`rusqlite`（bundled，WAL + `busy_timeout`）、`chrono`、`dirs`；表格 `comfy-table`、`unicode-width`；错误 `anyhow`。
- 网络：`reqwest` 0.12（rustls-tls，无默认 TLS）；行情源为 CNN FGI 与东方财富基金 API。
- 桌面：`gpui-kit` 0.9 + `gpui-tray` 0.3 + `gpui-pre` 0.3（Fluent 风格、原生托盘）；`gui-test` 开启 `gpui-pre/test-support` 做无头布局测试。
- AI：`agent-client-protocol` 5（process 特性），首个目标 `opencode acp`；不存 API key，可选自由文本 `model`。
- 平台通知：macOS `objc2-user-notifications`；Windows 用 `windows` 0.62 WinRT Toast + `AppUserModelID` 快捷方式 + `mns:` 协议回跳。
- 打包：Windows Inno Setup 6 安装包与便携 ZIP；macOS DMG（`MNS.app` 内含 `mns` 与 `mns-butlerd`）。

## 系统边界

- **外部 API**：CNN Fear & Greed（情绪主源，失败降级本地快照表）；东方财富 `fundmobapi`（基金净值/估值）与行情接口（指数、个股）；均经 `reqwest` 访问，超时与重试有硬上限。
- **本地数据**：`~/.mns/config.toml` 与 `~/.mns/mns.db` 是账户事实唯一来源；CLI/GUI 可写，`mns-butlerd` 仅持只读连接（`open_read_only`），无写路径。相对配置路径一律相对 `app_data_dir()` 解析，禁止相对 cwd 写入。
- **文件根**：`~/.mns/` 为唯一可写根；`reports/`（每日报告）、`agent/reviews/`（巡检报告唯一存储）、`agent/workspace/`、`agent/runs/`、`agent/butler/`（state/memory/tasks/outbox/pending/conversation/health）、`agent/last-review` 与 `agent/last-review-attempt.json`（巡检去重与当日退避重试）以及 `agent/events.jsonl`。
- **第三方运行时**：外部 ACP agent CLI（如 opencode）由用户提供路径，provider/模型/凭据在其自身配置；MNS 只传 prompt 与可选 model 字符串。工作区契约只写 `~/.mns/agent/**` 的受管块。
- **信任边界**：管家判定与通知口径不依赖模型输出；模型文案须过数字白名单与 `validate_notification`，不得谎称下单；会话可给倾向性观点并自我标注，通知 banner 走最严话术。巡检 prompt 的 `questions`「待确认」是例外通道（0–2 条、只问会改变结论的封闭事实；正文不得反问用户）；无人值守的定时巡检拒绝一切权限请求。守护进程不发系统通知（macOS 拒绝 helper），投递只能由宿主完成。行为事件严格本地、无遥测；不使用 launchd/cron/计划任务做调度。

## 代码映射索引

| 概念 | 位置 | 说明 |
|------|------|------|
| CLI 命令定义 | `src/cli.rs` | clap `Commands` / `BacktestAction` |
| CLI 分发与表格呈现 | `src/main.rs` | 薄层，规则不在此 |
| 库模块面 | `src/lib.rs` | 唯一引擎来源，feature 门控 |
| 类型化应用服务 | `src/app/mod.rs` | `Snapshot`、`Freshness` |
| 配置与数据路径 | `src/config.rs` | `app_data_dir`、`resolve_data_path` |
| SQLite 账本 | `src/db.rs` | `open_read_only`、WAL 调优 |
| 目标仓位与再平衡 | `src/strategy.rs` | `calculate_rebalance_plan` |
| 决策信号 | `src/signal.rs` | 平滑、区间确认、决策日 |
| 恐贪指数获取 | `src/sentiment.rs` | CNN API |
| 基金/市场报价 | `src/quote.rs`、`src/market.rs` | 东方财富 |
| 报告留档 | `src/report.rs` | `reports/` 先留档再决策 |
| XIRR 与回撤 | `src/metrics.rs` | 现金流加权主口径 |
| 实盘审计与数据集 | `src/audit.rs`、`src/live_data.rs` | point-in-time、通道成本 |
| 桌面壳与页面 | `src/gui/mod.rs`、`src/gui/pages/` | 六页 + 托盘 + worker |
| 管家编排/调度 | `src/butler/orchestrator.rs`、`src/butler/schedule.rs` | 确定性判定 |
| 巡检排程与存档 | `src/butler/review.rs` | `schedule`、`last-review`、`ReviewAttempt` |
| 巡检页与管家接力 | `src/gui/pages/review.rs`、`src/gui/mod.rs` | `Signal::ButlerRef`、`--ref`/`--ask` |
| 巡检 prompt 契约 | `src/agent/routine.rs` | `daily_review_prompt`、`questions` |
| 管家会话与表达 | `src/butler/chat.rs`、`src/butler/talk.rs`、`src/butler/expression.rs` | 会话唯一写者 |
| 本地进程通道 | `src/ipc.rs` | `Signal` NDJSON 协议 |
| 系统通知 | `src/notify.rs` | macOS / Windows 送达 |
| ACP 运行时与契约 | `src/agent/runner.rs`、`src/agent/routine.rs`、`src/agent/workspace.rs` | 不持有 API key |

更细的实现签名与代码切片见 `.terrain/agent/repomix.md`，符号关系用 CodeGraph CLI 查询。