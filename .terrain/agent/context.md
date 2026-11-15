---
type: agent_context
project: money-never-sleep
title: Agent Architecture Context
source: .
---

## 项目概览
MNS（Money Never Sleeps，v0.8.0）是本机运行的个人投资决策工具（Rust），服务于中长线逆情绪投资者。三个二进制共享同一引擎库 `money_never_sleep`：CLI `mns`、GPUI 桌面壳 `mns-gui`（`gui` feature）、独立帮助中心 `mns-help`（CHM 式离线手册）。核心命题：把 CNN 恐贪指数（0–100）映射为风险资产目标仓位（越恐慌越高，35%–85%），再按**逐腿**偏离带（默认 ±4pp）给出买/卖/持有建议；情绪只设定敞口，已被证伪为择时依据。硬约束：零券商连接、零服务端、零模型/凭据托管；报告先留档再决策；`~/.mns/` 是唯一可写根，任何写入都经 `config::resolve_data_path()`，不依赖 cwd。

## 架构设计
| 层 | 容器 | 职责 | 关键路径 |
|---|---|---|---|
| 交互层 | clap CLI | 串行命令与参数、终端表格渲染 | `src/cli.rs`、`src/main.rs` |
| 交互层 | GPUI 桌面壳 | 窗口/托盘、六页路由、后台 jobs、登录自启动、单实例唤起 | `src/bin/mns-gui.rs`、`src/gui/**` |
| 交互层 | 帮助中心 | 左目录右正文、前进/后退、深链跳页、截图运行时加载 | `src/bin/mns-help.rs`、`src/help/**` |
| 入口层 | 库入口 | 导出核心模块；按 feature 暴露 `agent`/`gui`/`help`/`single_instance` | `src/lib.rs` |
| 应用服务层 | typed 视图 | `Snapshot`/`PlanView`/`Freshness` 等快照，避免前端各自拼账本；内置年度纪律复盘 | `src/app/mod.rs` |
| 助手层 | 外部 ACP 运行时 | 驱动外部 ACP CLI（默认 `opencode`，追加 `acp`），组装巡检/追问提示词，解析 `mns-decision` 结论 | `src/agent/{mod,runner,routine}.rs` |
| 引擎层 | 纯规则/纯统计 | 信号、再平衡/定投、护栏、纪律复盘、报告、审计 | `src/{signal,strategy,guardrail,discipline,report,audit,backtest}.rs` |
| 数据访问层 | SQLite 单写者 | schema 初始化、upsert、按日/按起始日查询 | `src/db.rs`、`src/models.rs` |
| 外部数据层 | HTTP 抓取 | 恐贪指数、基金/行情净值，多源回退 | `src/sentiment.rs`、`src/quote.rs`、`src/market.rs`、`src/live_data.rs` |

- 依赖方向单向：交互/入口 → 应用服务 → 引擎 →（数据访问｜外部数据）；`discipline` 只消费 `db` 流水与 `config.backtest` 预算。
- feature 门控链：`agent` → `gui`（隐含 `agent`）→ `gui-test`（headless 布局测试）；默认 feature 为空，CLI 依赖与行为不变。
- 帮助中心是**独立进程**，复用 `gui::theme`/`gui::components`（提升为 `pub(crate)` 共享），只读渲染，不持有任何投资规则；`help/content.rs` 用 `include_str!` 在编译期把 `help/*.md` 内嵌进二进制。
- 跨进程协作只有两种机制：Windows 命名互斥体 + 命名事件做单实例置前（非 Windows 恒返回 Primary）；文件型深链 `~/.mns/help-request`（temp + rename 原子写、mtime 判过期）。
- 平台适配集中在 `gui::command`（Windows `CREATE_NO_WINDOW`）、`gui::autostart`（只写系统登录项）、`build.rs`（仅 Windows 嵌 `assets/app.ico`）。

## 模块地图
| Module | 责任 | 主要路径 |
|---|---|---|
| 库入口/feature 导出 | 常驻 `app`/`discipline`；feature 门控 `agent`/`gui`/`help`/`single_instance` | `src/lib.rs` |
| CLI 命令树与分发 | `cmd_*` 串行装配：`report/today/plan/guard/performance`；backtest 默认审计 + `mirror/asset/update-data/validate-full/legacy/validate`；cash `deposit/withdraw/set` | `src/cli.rs`、`src/main.rs` |
| 应用服务层 | typed 只读视图与服务、`BuyGuardrail::build_note` 前缀生成 | `src/app/mod.rs` |
| 桌面壳（`gui`） | GPUI 窗口/托盘、页面路由（总览/持仓/计划/复盘/助手/设置）、jobs、每日巡检调度、隐藏启动与窗口重建 | `src/bin/mns-gui.rs`、`src/gui/mod.rs`、`src/gui/pages/*.rs`、`src/gui/jobs.rs` |
| 帮助中心（`gui`） | `mns-help` 入口、目录树/正文、键盘与菜单绑定、macOS 菜单改名 | `src/bin/mns-help.rs`、`src/help/{mod,content,macos}.rs` |
| 帮助内容管线 | 自研 Markdown 子集解析（标题/列表/GFM 表格/代码块/`[!TYPE]` 提示块/图片）与 GPUI 渲染、`ImageCache` 截图缓存 | `src/help/markdown.rs`、`src/help/render.rs` |
| 帮助深链 | 请求文件写入、400ms `Watcher` 轮询、slug→页解析 | `src/help/deeplink.rs`、`src/gui/help_links.rs` |
| 单实例守卫 | `acquire`/`wakeup_requested`；Primary 轮询唤起窗口 | `src/single_instance.rs` |
| 外部助手运行时 | 设置持久化、事件模型、`PermissionPolicy`、`DecisionReport`、巡检提示词、随包 CLI 路径解析 | `src/agent/{mod,runner,routine}.rs`、`examples/agent_smoke.rs` |
| 会话/决策归档 | 一对话一 JSON 时间线、只读回放、`agent/runs/*.json`、`last-review` 标记 | `src/gui/conversations.rs` |
| 配置与存储 | `AppConfig`（曲线/档位/带宽/成本/回测预算）、`zone_target_weights`、数据根解析、SQLite schema | `src/config.rs`、`src/db.rs`、`src/models.rs` |
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
1. `mns-gui` 先取单实例锁（重复启动仅通知 Primary 置前后退出），把 cwd 固定到 `~/.mns`，`--tray`/`--background` 隐藏启动只驻留托盘。
2. 页面从 `app` 读 typed 快照；刷新/记账/巡检走 `gui::jobs`，派生进程统一走 `gui::command`（Windows 无黑框）。
3. `maybe_start_daily` 每日最多一次（`agent/last-review`）：用随包 `mns today --quiet` 生成简报，`routine::daily_review_prompt` 组装提示词（报告为唯一数据源、只读命令、`mns-decision` 输出契约）提交 ACP；每轮结束提取最后一个带标签围栏解析为 `DecisionReport`，渲染卡片并存档。
4. 帮助入口：页面顶栏（"这一屏怎么用"）与"数字来历说不清"的卡片（"这个数怎么来"）各挂一个 `?`，slug 集中在 `help_links` 并被测试校验。
5. 点击 `?` 先写深链请求文件，再按子进程句柄决定：无窗口则启动 `mns-help --page <slug>`，有窗口则靠 watcher 翻页并置前；`mns-help` 取自身单实例锁，`QuitMode::LastWindowClosed`，关窗即退出以便下次可重开。
6. 正文在编译期内嵌，截图运行时从 exe 同级 `help/`（macOS 为 `Contents/Resources/help`）按目录层级向上搜索 5 级，缺失时渲染带预期路径的虚线占位。

## 技术选型
- **语言**：Rust edition 2024；包 `money-never-sleep`，库 `money_never_sleep`，三个二进制（`mns` / `mns-gui` / `mns-help`）。
- **CLI**：clap 4（derive 子命令树）。
- **桌面 UI（`gui`）**：`gpui-kit 0.9.0` + `gpui-tray 0.3.0`（`gpui-kit`/`menu-state`，统一到 `gpui-pre`）；`gui-test` 打开 `gpui-pre/test-support` 做 headless 布局断言。
- **外部助手（`agent`）**：`agent-client-protocol 5.0`（`process`，ACP over stdio）；权限策略三档（自主运行/保护写入/逐项确认）。
- **帮助中心**：自研 Markdown 子集解析器（不引依赖，语法可预期）+ GPUI 渲染；仅 macOS 用 `objc2` 重设前台菜单名。
- **存储**：rusqlite（bundled）单文件 `~/.mns/mns.db`；`serde + toml` 管 `config.toml` 与 `gui.toml`（temp+rename 原子写）。
- **网络/时间/输出**：reqwest（rustls）、chrono（`NaiveDate` 日频口径）、comfy-table + unicode-width（中英混排列宽）。
- **错误**：anyhow，统一 `Error: <中文原因>`，CLI 非零退出。
- **打包**：`scripts/make-dmg.sh`（三可执行同版本入 `.app`，截图进 `Contents/Resources` 以过 codesign）、`scripts/make-installer.ps1` + `scripts/installer/mns.iss`（按用户安装、无 UAC、不改 PATH、附帮助中心快捷方式）、`scripts/make-help-diagrams.sh` 生成 `help/diagrams/*.svg`、`make-icons.swift` 生成图标。

## 系统边界
| 边界 | 对象 | 说明 / 信任边界 |
|---|---|---|
| 外部 API | CNN Fear & Greed `production.dataviz.cnn.io/index/fearandgreed/graphdata` | 只读、需网络、可 418；决策与数据集共同来源 |
| 外部 API | 天天基金 / 东财 / Yahoo Finance | 净值与价格多源回退；单标的缺失时跳过，Yahoo 可能 403（视为环境限制） |
| 外部进程 | ACP 兼容 CLI（默认 `opencode acp`） | MNS 只提供 CLI 路径、可选模型名、工作目录与权限策略；**不持有模型/端点/API Key**，结论只经 `mns-decision` 块回传，Agent 不写文件 |
| 本地进程 | `reg.exe`、随包 `mns`、`mns-help`、`open`/`xdg-open` | 由 `gui::command` 派生；仅写用户级 HKCU；帮助中心作为子进程启动，句柄存活即视为"已开着" |
| 跨进程 IPC | `~/.mns/help-request`、命名互斥体/事件 | 原子写 + mtime 判定，失败模式是"停在当前页"而非窗口打不开；单实例锁按登录会话隔离（`Local\`） |
| 本地存储 | SQLite 账本、`config.toml`、`gui.toml`、`agent/{conversations,runs,last-review}`、`reports/`、`help-request`、系统登录项 | 本地可信单写者；并发写锁库；`cash set` 为唯一纠错通道 |
| 数据集构建 | `update-data` + Python 3 + 网络 | 写 CSV/meta；失败不得静默污染既有数据 |
| 人机边界 | 用户 / 券商 | MNS 只记账与建议，不下单、不阻断；护栏、纪律复盘与助手结论都不自动执行，责任在用户 |

## 代码映射索引
| Concept | Location | Notes |
|---|---|---|
| 库入口/feature 导出 | `src/lib.rs` | `app`/`discipline` 常驻；`agent`/`gui`/`help`/`single_instance` 按 feature |
| CLI 命令树 | `src/cli.rs` | `CashAction`、`BacktestAction`（默认审计 + 6 个子命令）、`ConfigAction` |
| CLI 分发与渲染 | `src/main.rs` | `cmd_*` 装配；`plan` 并排打印"今日档/中性档"目标；`today --quiet` 供巡检复用 |
| 应用快照/服务 | `src/app/mod.rs` | `Snapshot`/`PlanView`/`load_snapshot`；`Snapshot.discipline` |
| 决策信号 | `src/signal.rs` | `compute`（平滑/区间确认/决策日）、`next_decision_day`；生产与审计同源 |
| 再平衡计划 | `src/strategy.rs` | 逐腿偏离带判定、先卖后买、`risk_weight_after_rebalance`、缺标的清零挂原因 |
| 报告口径 | `src/report.rs` | 【风险档位】中性档 vs 今日档、偏离带"按腿判定"说明、执行后残余偏离提示 |
| 配置/档位目标 | `src/config.rs` | `zone_target_weights()` 取五档锚点、`effective_neutral_weight()` 不随 `mode` 漂移 |
| 纪律复盘 | `src/discipline.rs`、`src/gui/pages/performance.rs` | 前缀常量 + `bucket_of`；`discipline_card` 对照年度预算 |
| 审计/数据集 | `src/audit.rs`、`src/backtest.rs`、`src/live_data.rs`、`src/metrics.rs` | 实盘审计、legacy 对照、point-in-time CSV 契约与指标 |
| 桌面壳入口 | `src/bin/mns-gui.rs`、`src/gui/mod.rs` | `run()`、`ensure_window`、`spawn_instance_watcher`、`open_help_center`、托盘/菜单动作 |
| 帮助中心入口 | `src/bin/mns-help.rs`、`src/help/mod.rs` | `help::run()`、`--page <slug>`、键位与 `QuitMode::LastWindowClosed` |
| 帮助内容与渲染 | `src/help/{content,markdown,render,macos}.rs` | `include_str!` 内嵌页、自研解析器、`ImageCache` 搜索根、macOS 菜单改名 |
| 帮助深链/链接表 | `src/help/deeplink.rs`、`src/gui/help_links.rs` | `request_at`/`Watcher`；`HelpLink{slug,tip}` 与 `ALL` 测试清单 |
| 单实例守卫 | `src/single_instance.rs` | `acquire`/`Acquired`/`wakeup_requested`；Windows 互斥体实现，其他平台空实现 |
| 打包/图标 | `build.rs`、`scripts/{make-dmg.sh,make-installer.ps1,installer/mns.iss,make-help-diagrams.sh}` | 三可执行同目录 + `help/` 截图资源；Windows 图标编入 exe |