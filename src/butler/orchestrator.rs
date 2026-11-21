//! 编排层：唤醒源 → 唤醒判定 → 产出开口候选 → 开口判定。
//!
//! 这一层的唯一职责是**串起来**，不重新定义任何口径：偏离、带宽、决策日
//! 全部来自 `crate::app::Snapshot`（与 CLI / GUI 同源），任务的「到点」来自
//! `TaskBoard::due`，记忆里的承诺来自 `Memory`。管家不自算一套数字，否则
//! 「GUI 说低配 6.2pp、管家说 5.8pp」就没人知道该信谁。
//!
//! 表达层不在这里：这里只产出**确定性模板文案**。模型可以改写措辞，但不能
//! 改事实；改坏了就由 `validate_notification` 打回模板。

use anyhow::Result;
use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};

use super::butler_dir;
use super::memory::{FactKind, Memory};
use super::schedule::{
    self, SpeakCandidate, SpeakDecision, SpeakLedger, SpeakPolicy, SpeakReason, WakeScheduler,
    WakeSource, WakeVerdict,
};
use super::store::{read_json, write_json};
use super::tasks::{ButlerTask, TaskBoard, TaskKind};

/// 一条腿的偏离，直接抄 `Snapshot.categories`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftRow {
    pub key: String,
    pub name: String,
    pub current_pct: f64,
    pub target_pct: f64,
    pub drift_pp: f64,
    /// 这条腿当前市值。用于「分析持仓」时把百分比落到钱上；旧版
    /// `facts.json` 没有这个字段时按 0 读，表达层会跳过金额。
    #[serde(default)]
    pub market_value: f64,
}

impl DriftRow {
    /// 超出带宽的绝对幅度；没超就是 0。开口判定只看这个，不看方向——
    /// 低配高配都值得提醒，但话术不同。
    pub fn excess_pp(&self, band_pp: f64) -> f64 {
        (self.drift_pp.abs() - band_pp).max(0.0)
    }
}

/// 决定层需要的账户事实。刻意做成扁平结构：给定这一份，`candidates` 的
/// 输出完全可复算，不读数据库、不联网、不调模型。
///
/// 这份结构由判定方就地计算（[`ledger_facts`]）：守护进程与审计 CLI 直接
/// 从只读快照提取，不再经文件交接。「管家只读账本」仍然成立——边界从
/// 「不读库」改成「连接不可写」。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AccountFacts {
    pub decision_day: bool,
    pub band_pp: f64,
    pub drift: Vec<DriftRow>,
    /// 今日入金合计（只算 `deposit`，不含期初校准）。
    pub new_cash_today: f64,
    /// 月度计划是否有可执行的下单（`PlanView::has_orders`）。
    pub plan_ready: bool,
    /// 今天反复查看的页面与次数（来自行为日志）。
    pub heavy_page: Option<(String, usize)>,
    /// 今天到期的承诺（来自记忆的 `Commitment`）。
    pub promises_due: Vec<String>,

    // ---- 供「分析持仓 / 念计划」使用的结构性事实 ----
    //
    // 这些字段只用来**复述**账本，不参与开口判定，所以全部带 `serde(default)`：
    // 旧版 `facts.json` 读进来是 0 / 空，表达层据此跳过对应句子，而不是把 0
    // 当成真实数字念出来。
    #[serde(default)]
    pub total_assets: f64,
    #[serde(default)]
    pub cash: f64,
    #[serde(default)]
    pub holdings_value: f64,
    #[serde(default)]
    pub positions_count: usize,
    /// 月度计划的逐腿金额（正=买，负=卖）；没有计划时为空。
    #[serde(default)]
    pub plan_lines: Vec<PlanLine>,
    /// 计划被拦下的原因（如现金不足），没有则为 `None`。
    #[serde(default)]
    pub plan_blocked: Option<String>,
    /// 下一个决策日（ISO 日期）。每天决策时为 `None`。
    #[serde(default)]
    pub next_decision_day: Option<String>,
    /// 账本快照日期（ISO 日期）。用来判断数据是不是今天，避免拿旧数字
    /// 冒充当天结论。
    #[serde(default)]
    pub data_date: Option<String>,
}

/// 计划里的一条腿。金额已由 `PlanView` 算好，这里只做搬运。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanLine {
    pub name: String,
    /// 正数买入，负数卖出。
    pub amount: f64,
}

/// `-0.0` 与 `0.0` 在浮点里相等，但格式化出来是「-0.0%」，读起来像欠了
/// 一点点仓位。管家的话是要念给人听的，这里把符号位抹平。
fn clean_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

impl AccountFacts {
    /// 从 GUI / CLI 同一个 `Snapshot` 提取事实。
    pub fn from_snapshot(snapshot: &crate::app::Snapshot, now: DateTime<Local>) -> Self {
        let drift = snapshot
            .categories
            .iter()
            .map(|row| DriftRow {
                key: row.key.clone(),
                name: row.name.clone(),
                current_pct: clean_zero(row.current_pct),
                target_pct: clean_zero(row.target_pct),
                drift_pp: clean_zero(row.drift_pp),
                market_value: clean_zero(row.market_value),
            })
            .collect();

        let today = now.date_naive();
        let new_cash_today: f64 = snapshot
            .flows
            .iter()
            .filter(|flow| flow.date == today && flow.kind == "deposit")
            .map(|flow| flow.amount)
            .sum();

        // 只搬有实际金额的腿：`has_orders` 用的是同一个 1 元门槛，否则会
        // 念出一串「买入 ¥0」。
        let plan_lines: Vec<PlanLine> = snapshot
            .plan
            .as_ref()
            .map(|plan| {
                plan.legs
                    .iter()
                    .filter(|leg| leg.amount.abs() >= 1.0)
                    .map(|leg| PlanLine {
                        name: leg.category_cn.clone(),
                        amount: leg.amount,
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self {
            decision_day: schedule::is_decision_day(now, snapshot.config.decision_weekday),
            band_pp: snapshot.config.band_pp,
            drift,
            new_cash_today,
            plan_ready: snapshot.plan.as_ref().is_some_and(|plan| plan.has_orders()),
            heavy_page: None,
            promises_due: Vec::new(),
            total_assets: clean_zero(snapshot.total_assets),
            cash: clean_zero(snapshot.cash),
            holdings_value: clean_zero(snapshot.holdings_value),
            positions_count: snapshot.positions.len(),
            plan_lines,
            plan_blocked: snapshot.plan_blocked.clone(),
            next_decision_day: snapshot.next_decision_day.map(|day| day.to_string()),
            data_date: Some(snapshot.as_of.to_string()),
        }
    }

    /// 从记忆补齐「今天到期的承诺」。
    pub fn with_promises(mut self, memory: &Memory, today: NaiveDate) -> Self {
        self.promises_due = memory
            .active_facts(today)
            .into_iter()
            .filter(|fact| fact.kind == FactKind::Commitment)
            .filter(|fact| fact.expires_at == Some(today))
            .map(|fact| fact.text.clone())
            .collect();
        self
    }

    /// 偏离最大的一条腿（只看绝对幅度）。
    pub fn widest_drift(&self) -> Option<&DriftRow> {
        self.drift
            .iter()
            .max_by(|a, b| a.drift_pp.abs().total_cmp(&b.drift_pp.abs()))
    }
}

/// 就地读取账本，提取管家事实（不含承诺——承诺在记忆文件里，由持有
/// butler 状态的判定方合并）。
///
/// 这是「谁判定谁计算」的入口：守护进程与审计 CLI 共用同一条路径，事实
/// 不再经 `facts.json` 交接，也就不存在「GUI 没开就没事实」的空窗。账本
/// 用只读连接读取，调用方没有写路径。
///
/// 阻塞调用（全库读取 + 指标计算）：异步上下文里调用须放阻塞线程池
/// （butlerd 走 `spawn_blocking`），同步 CLI 直接调用即可。
pub fn ledger_facts(now: DateTime<Local>) -> AccountFacts {
    ledger_facts_report(now).0
}

/// 同 [`ledger_facts`]，但把账本读取失败的原因带出来。
///
/// 守护进程是唯一写健康状态的地方：失败不能只落在无人看的 stdout 上
/// （GUI 关着时没人盯着终端），托盘要能从 `health.json` 看到红灯。
pub fn ledger_facts_report(now: DateTime<Local>) -> (AccountFacts, Option<String>) {
    let mut error = None;
    let mut facts = match crate::app::load_snapshot_read_only() {
        Ok(snapshot) => {
            let mut facts = AccountFacts::from_snapshot(&snapshot, now);
            apply_price_gate(&mut facts, snapshot.freshness.price_age_hours);
            facts
        }
        Err(cause) => {
            // 账户未初始化 / 暂不可读不等于「账户是空的」：退化为默认事实，
            // 只保留与数字无关的由头（到点任务、承诺）。失败原因交给调用方
            // 落状态，别只打一行日志。
            let message = format!("{cause:#}");
            eprintln!("管家事实：账本快照读取失败（{message}），本轮按空账户判定");
            error = Some(message);
            AccountFacts::default()
        }
    };

    // 行为画像：同一天同一页面看 ≥5 次才算高频。它不是开口由头，只进事实，
    // 供管家在会话里回答「今天看了几次」。
    let since = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| naive.and_local_timezone(Local).single())
        .unwrap_or(now);
    let window = super::events::summarize(&super::events::read_all().unwrap_or_default(), since);
    if let Some((page, views)) = window.page_views.iter().max_by_key(|(_, views)| **views)
        && window.is_heavy_checking(page)
    {
        facts.heavy_page = Some((super::events::page_label(page).to_owned(), *views));
    }
    (facts, error)
}

/// 守护进程写下的账本健康状态。托盘与 `mns-butler status` 只读它：
/// 只读连接在极端场景（崩溃留下未 checkpoint 的 `-wal`、又没有别的连接）
/// 会打不开库——那时事实退化为空账户，用户必须能在托盘看到「账本读取失败」，
/// 而不是以为账户真的空了。
///
/// 唯一写者是 `mns-butlerd`；其他进程只读。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub at: DateTime<Local>,
    pub ledger_ok: bool,
    pub error: Option<String>,
}

pub fn health_path() -> std::path::PathBuf {
    super::butler_dir().join("health.json")
}

pub fn record_health(ledger_ok: bool, error: Option<String>) -> Health {
    let health = Health {
        at: Local::now(),
        ledger_ok,
        error,
    };
    // 简报请求与巡检可能在不同阻塞线程上同时落盘：默认锁按进程/机器互斥，
    // 避免两个写者争同一个临时文件。
    let _guard = super::writer_lock();
    if let Err(cause) = super::store::write_json(&health_path(), &health) {
        eprintln!("管家健康状态写入失败：{cause:#}");
    }
    health
}

pub fn read_health() -> Option<Health> {
    let text = std::fs::read_to_string(health_path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// 价格过旧时砍掉依赖估值的判断（偏离、计划可执行）：旧价格不足以支撑
/// 「偏离多少」。
///
/// 当日入金、决策日与行为画像不依赖价格，且 [`ledger_facts`] 总是按当前
/// 时刻计算，无需降级。抽象出来是为了把「旧价格不说偏离」钉成可测行为。
fn apply_price_gate(facts: &mut AccountFacts, price_age_hours: Option<i64>) {
    if price_too_old(price_age_hours) {
        facts.drift.clear();
        facts.plan_ready = false;
    }
}

/// 价格是否旧到不能支撑「偏离多少」的判断（≥24 小时）。
///
/// 与 [`crate::app::Freshness::price_is_stale`] 的差别在空账户：没有持仓就
/// 没有「价格年龄」可言，不该因此砍掉计划可执行——纯现金账户的定投计划
/// 不依赖任何价格。
fn price_too_old(price_age_hours: Option<i64>) -> bool {
    price_age_hours.is_some_and(|hours| hours >= 24)
}

/// 产出一轮开口候选。顺序由 `SpeakReason` 的排序保证，这里只负责收集。
///
/// 由头只有三类：任务到点、有可执行的下一步、承诺到期。
///
/// 曾经的「高频查看 + 市场异动」已移除：项目只采集 FGI 与四只基金净值，
/// 没有指数级行情，无法诚实地判断「市场动了多少」。拿基金净值冒充市场、
/// 或只看高频查看就开口（会变成催促），两种替代都更糟。行为日志仍记录
/// 高频查看，只作为会话里的上下文，不再作为开口理由。
pub fn candidates(
    facts: &AccountFacts,
    due_tasks: &[ButlerTask],
    now: DateTime<Local>,
) -> Vec<SpeakCandidate> {
    let today = now.date_naive();
    let mut rows = Vec::new();

    for task in due_tasks {
        // Check 回显用户要求检查什么；Talk 是用户明确要管家到点开口，
        // 送一句问候而不是把指令原文念回去。两者都只有一句话。
        let (draft, fallback) = match task.kind {
            TaskKind::Check => (
                format!("到点了：{}", task.intent),
                "到点了，按你自己定的那件事检查一下。",
            ),
            TaskKind::Talk => ("到点了，我在。想聊两句吗？".to_owned(), "到点了，我在。"),
        };
        rows.push(candidate(
            SpeakReason::TaskDue,
            format!("task:{}", task.id),
            draft,
            fallback,
        ));
    }

    if facts.decision_day
        && let Some(row) = facts.widest_drift()
        && row.excess_pp(facts.band_pp) > 0.0
    {
        let direction = if row.drift_pp > 0.0 {
            "高配"
        } else {
            "低配"
        };
        let draft = format!(
            "今天决策日，{}占比 {:.1}%（{direction} {:.1}pp），超出 ±{:.1}pp 带宽，要处理吗",
            row.name,
            row.current_pct,
            row.drift_pp.abs(),
            facts.band_pp
        );
        // 触发键不带日期：未处理的偏离是「同一件事」，七天内只说一次；
        // 每天换一个键等于每天催一遍。
        rows.push(candidate(
            SpeakReason::ActionableStep,
            format!("drift:{}", row.key),
            draft,
            "今天决策日，有腿超出目标带宽，点开看看要不要处理。",
        ));
    }

    if facts.new_cash_today > 0.5 && facts.plan_ready {
        rows.push(candidate(
            SpeakReason::ActionableStep,
            format!("cash:{today}"),
            format!(
                "新钱到账 ¥{:.0}，月度计划有可执行的下单，今天可以走一遍",
                facts.new_cash_today
            ),
            "新钱到账了，月度计划可以执行，点开看一眼。",
        ));
    }

    for promise in &facts.promises_due {
        let short = truncate_chars(promise, 24);
        rows.push(candidate(
            SpeakReason::PromiseDue,
            format!("promise:{today}:{short}"),
            format!("你之前说「{short}」，今天到期，还按原计划吗"),
            "你之前有一件事今天到期，还按原计划吗。",
        ));
    }

    rows
}

/// 构造候选；模板文案自己先过一遍硬约束。过不了就退到兜底文案——
/// 兜底也必须能过校验，否则表达层会拿到一条本就不该发的内容。
fn candidate(
    reason: SpeakReason,
    trigger: String,
    draft: String,
    fallback: &str,
) -> SpeakCandidate {
    let draft = if schedule::validate_notification(&draft).is_ok() {
        draft
    } else {
        debug_assert!(
            schedule::validate_notification(fallback).is_ok(),
            "兜底文案本身不合规：{fallback}"
        );
        fallback.to_owned()
    };
    SpeakCandidate {
        reason,
        trigger,
        draft,
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// 需要持久化的管家状态（记忆与任务各自落盘，见 `Memory::save` /
/// `TaskBoard::save`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ButlerState {
    pub scheduler: WakeScheduler,
    pub policy: SpeakPolicy,
    pub ledger: SpeakLedger,
    /// 上次每周自整理（记忆合并 + 任务复盘）。`None` = 从未整理。
    pub last_maintenance: Option<DateTime<Local>>,
    /// 上次尝试 agent 主动开口的时间（无论最后说没说）。
    ///
    /// 成功开口由 ledger 记录与频控约束；这个字段防的是「agent 每次都说无」
    /// 时每 4 小时起一次外部会话——尝试本身也要有节奏。
    #[serde(default)]
    pub last_agent_init: Option<DateTime<Local>>,
    /// agent 主动开口的连续失败次数。
    ///
    /// 失败（起不来、超时、回答过不了校验）按 [`agent_init_backoff`]
    /// 退避重试，而不是静默 12 小时；成功开口或模型正常说「无」不改变
    /// 这个计数——「无」是合法回答，不是故障。
    #[serde(default)]
    pub agent_init_failures: u32,
}

/// agent-init 与上一次会话往来之间至少隔这么久（常规节奏，无失败时）。
///
/// 用户刚聊完，管家不该自己起话头；模型连续说「无」时也不该每轮巡检
/// 都花一次外部会话。
pub const AGENT_INIT_MIN_IDLE_HOURS: i64 = 12;

/// agent-init 失败后的重试节奏：15 分钟 → 1 小时 → 6 小时封顶。
///
/// 失败一次就沉默 12 小时，用户看到的是「管家好几天不说话」；CLI 路径
/// 配错这类问题修好后也该快点恢复。成功开口（`failures == 0`）回到常规
/// 的 [`AGENT_INIT_MIN_IDLE_HOURS`]。
pub fn agent_init_backoff(failures: u32) -> chrono::Duration {
    match failures {
        0 => chrono::Duration::hours(AGENT_INIT_MIN_IDLE_HOURS),
        1 => chrono::Duration::minutes(15),
        2 => chrono::Duration::hours(1),
        _ => chrono::Duration::hours(6),
    }
}

/// agent-init 的决策日志：`agent/butler/initlog.jsonl`，每次尝试一行。
///
/// 守护进程是唯一写者（只追加）。`mns-butler status` 读它回答「昨天
/// 8 点为什么没说话」——失败原因只打 stderr 的话，GUI 拉起的守护进程
/// 谁也看不见。
pub fn initlog_path() -> std::path::PathBuf {
    butler_dir().join("initlog.jsonl")
}

/// 一轮 agent-init 尝试的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentInitOutcome {
    /// 开口了：已过校验并记入 ledger。
    Spoke,
    /// 模型说「无」或额度已满：正常沉默，不算失败。
    Silent,
    /// 起不来 / 超时 / 回答没过校验：计入退避。
    Failed,
}

impl AgentInitOutcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Spoke => "开口",
            Self::Silent => "沉默",
            Self::Failed => "失败",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInitRecord {
    pub at: DateTime<Local>,
    pub outcome: AgentInitOutcome,
    /// 失败原因或沉默原因（「模型说无」「额度已满」）。开口时为空。
    #[serde(default)]
    pub detail: Option<String>,
}

/// 追加一条 agent-init 尝试记录。失败只影响审计，不影响本轮判定。
pub fn append_init_record(record: &AgentInitRecord) -> Result<()> {
    append_init_record_at(&initlog_path(), record)
}

/// 路径可注入版本：测试用 scratch 目录，不碰真实账户数据。
pub fn append_init_record_at(path: &std::path::Path, record: &AgentInitRecord) -> Result<()> {
    let line = serde_json::to_string(record)?;
    super::store::append_line(path, &line)
}

/// 读最近 `limit` 条尝试记录（旧的先被丢）。文件不存在或损坏行跳过。
pub fn read_init_records(limit: usize) -> Vec<AgentInitRecord> {
    read_init_records_at(&initlog_path(), limit)
}

pub fn read_init_records_at(path: &std::path::Path, limit: usize) -> Vec<AgentInitRecord> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => return Vec::new(),
    };
    let mut records: Vec<AgentInitRecord> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let keep = records.len().saturating_sub(limit);
    records.drain(..keep);
    records
}

/// 自整理周期（天）。到期后在下一次成功唤醒时顺带执行，不另起定时器。
pub const MAINTENANCE_INTERVAL_DAYS: i64 = 7;

/// 通知出站队列：管家判定「该说」之后写一行，通知层（tray / 系统通知）
/// 只负责送达，不再判断该不该说。
pub fn outbox_path() -> std::path::PathBuf {
    butler_dir().join("outbox.jsonl")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notice {
    pub at: DateTime<Local>,
    pub reason: SpeakReason,
    pub trigger: String,
    pub draft: String,
    /// 稳定通知 id：落盘时生成一次，之后不可变。旧版 pending 行没有这个
    /// 字段，读取时用 `notice_id_for` 补出来——同样的 `at + trigger` 得到
    /// 同样的 id，重读不会漂移。
    #[serde(default)]
    pub notice_id: String,
}

impl Notice {
    /// 旧行补 id；已经是新格式的行原样保留。
    pub fn ensure_id(&mut self) {
        if self.notice_id.is_empty() {
            self.notice_id = notice_id_for(self.at, &self.trigger);
        }
    }
}

/// 由 `at + trigger` 派生稳定 id。
///
/// 同一事件（同一毫秒 + 同一 trigger）得到同一个 id；一天最多三条开口，
/// 毫秒时间戳足够区分。这里用 FNV-1a 而不是 `std::hash`：后者不保证跨
/// 进程/跨版本稳定，而旧 pending 行补出的 id 必须每次一致。
pub fn notice_id_for(at: DateTime<Local>, trigger: &str) -> String {
    let seed = format!("{}|{trigger}", at.timestamp_millis());
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in seed.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("n{}-{:08x}", at.timestamp_millis(), hash as u32)
}

/// 把一条已授权的通知放进出站队列。写失败只影响送达，不影响判定。
pub fn enqueue_notice(at: DateTime<Local>, candidate: &SpeakCandidate) -> Result<()> {
    let notice = Notice {
        at,
        reason: candidate.reason,
        trigger: candidate.trigger.clone(),
        draft: candidate.draft.clone(),
        notice_id: notice_id_for(at, &candidate.trigger),
    };
    let line = serde_json::to_string(&notice)?;
    super::store::append_line(&outbox_path(), &line)
}

/// 守护进程判定的开口。GUI 读走之后合并进会话存档（`conversation.json`
/// 始终只有 GUI 一个写者）。
pub fn pending_path() -> std::path::PathBuf {
    butler_dir().join("pending.jsonl")
}

/// 追加一条待合并的开口。与 `outbox` 内容相同，区别是消费方不同：
/// `outbox` 是审计（只追加不清），`pending` 是投递（GUI 消费后推进 offset）。
pub fn enqueue_pending(at: DateTime<Local>, candidate: &SpeakCandidate) -> Result<()> {
    let notice = Notice {
        at,
        reason: candidate.reason,
        trigger: candidate.trigger.clone(),
        draft: candidate.draft.clone(),
        notice_id: notice_id_for(at, &candidate.trigger),
    };
    let line = serde_json::to_string(&notice)?;
    super::store::append_line(&pending_path(), &line)
}

/// 从字节偏移读走新的待投递条目，并返回新的偏移。
///
/// 用偏移而不是「读完就清空」：守护进程可能在 GUI 读与清之间追加，清空会
/// 丢掉那一条。文件每天最多长三行，不值得为它做压缩。
pub fn take_pending(offset: u64) -> Result<(u64, Vec<Notice>)> {
    take_pending_at(&pending_path(), offset)
}

/// 路径可注入版本：测试用 scratch 文件，不碰真实账户目录。
pub fn take_pending_at(path: &std::path::Path, offset: u64) -> Result<(u64, Vec<Notice>)> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        // 还没有人开口过：offset 归零，不是错误。
        return Ok((0, Vec::new()));
    };
    let len = file.metadata()?.len();
    if offset > len {
        // 文件被外部截断过，只能从头再读；去重交给会话层。
        return Ok((0, Vec::new()));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    let consumed = offset + buf.len() as u64;
    let notices = buf
        .lines()
        .filter(|line| !line.trim().is_empty())
        // 坏行跳过（不报错）；旧行缺 id 时在这里补，交付链路只认非空 id。
        .filter_map(|line| serde_json::from_str::<Notice>(line).ok())
        .map(|mut notice| {
            notice.ensure_id();
            notice
        })
        .collect();
    Ok((consumed, notices))
}

/// 已消费到的偏移。P4 起消费者是 `mns-butlerd`：它把条目并进会话、投给
/// 宿主，全部成功才推进这里。
pub fn pending_offset_path() -> std::path::PathBuf {
    butler_dir().join("pending.offset")
}

/// 已消费到的 `pending.jsonl` 偏移。丢了就当 0，重读会去重。
pub fn read_pending_offset() -> u64 {
    std::fs::read_to_string(pending_offset_path())
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

/// 推进偏移。调用方负责保证「先落会话、投递成功，再推进」的顺序——
/// 反过来写的话，投递失败等于永久丢失：偏移已经越过它了。
pub fn write_pending_offset(offset: u64) {
    let _ = super::store::write_json(&pending_offset_path(), &offset);
}

/// 通知送达/点击的审计记录。只追加，不改写；`notice_id` 是关联键。
///
/// 与 `outbox.jsonl` 分开：outbox 记「管家说了什么」，这里记「系统通知真的
/// 送到/被点了吗」。两个问题不能互相冒充。
pub fn notices_path() -> std::path::PathBuf {
    butler_dir().join("notices.jsonl")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeEventKind {
    Delivered,
    Clicked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeEvent {
    pub event: NoticeEventKind,
    pub notice_id: String,
    /// 通知本身的产生时间（不是送达/点击时间）。
    #[serde(default)]
    pub notice_at: Option<DateTime<Local>>,
    pub recorded_at: DateTime<Local>,
    #[serde(default)]
    pub trigger: Option<String>,
    #[serde(default)]
    pub draft: Option<String>,
}

pub fn read_notice_events() -> Vec<NoticeEvent> {
    read_notice_events_at(&notices_path())
}

pub fn read_notice_events_at(path: &std::path::Path) -> Vec<NoticeEvent> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// 记一条「已送达」。同一 id 只记一次：宿主重启后重读旧 pending 时不重复
/// 弹横幅，也不重复落审计。
pub fn record_notice_delivered(notice: &Notice) -> Result<bool> {
    record_notice_delivered_at(&notices_path(), notice)
}

pub fn record_notice_delivered_at(path: &std::path::Path, notice: &Notice) -> Result<bool> {
    let already = read_notice_events_at(path).iter().any(|event| {
        event.event == NoticeEventKind::Delivered && event.notice_id == notice.notice_id
    });
    if already {
        return Ok(false);
    }
    let event = NoticeEvent {
        event: NoticeEventKind::Delivered,
        notice_id: notice.notice_id.clone(),
        notice_at: Some(notice.at),
        recorded_at: Local::now(),
        trigger: Some(notice.trigger.clone()),
        draft: Some(notice.draft.clone()),
    };
    let line = serde_json::to_string(&event)?;
    super::store::append_line(path, &line)?;
    Ok(true)
}

/// 记一条「被点击」。同一 id 第二次点击返回 `false`，调用方据此跳过重复
/// 注入；审计里也只留第一次。
pub fn record_notice_clicked(notice_id: &str) -> Result<bool> {
    record_notice_clicked_at(&notices_path(), notice_id)
}

pub fn record_notice_clicked_at(path: &std::path::Path, notice_id: &str) -> Result<bool> {
    let already = read_notice_events_at(path)
        .iter()
        .any(|event| event.event == NoticeEventKind::Clicked && event.notice_id == notice_id);
    if already {
        return Ok(false);
    }
    let event = NoticeEvent {
        event: NoticeEventKind::Clicked,
        notice_id: notice_id.to_owned(),
        notice_at: None,
        recorded_at: Local::now(),
        trigger: None,
        draft: None,
    };
    let line = serde_json::to_string(&event)?;
    super::store::append_line(path, &line)?;
    Ok(true)
}

/// 按 id 取送达记录。点击时窗口用它带回通知原文与产生时间——`--notice`
/// 只传一个 id，界面不能只显示「你点了一条通知」。
pub fn delivered_notice(notice_id: &str) -> Option<NoticeEvent> {
    delivered_notice_at(&notices_path(), notice_id)
}

pub fn delivered_notice_at(path: &std::path::Path, notice_id: &str) -> Option<NoticeEvent> {
    read_notice_events_at(path)
        .into_iter()
        .rfind(|event| event.event == NoticeEventKind::Delivered && event.notice_id == notice_id)
}

/// 一次唤醒的完整结果。
#[derive(Debug, Clone, PartialEq)]
pub struct WakeReport {
    pub wake: WakeVerdict,
    /// `None` = 没被唤醒，压根没做开口判定（不是「判了但沉默」）。
    pub speak: Option<SpeakDecision>,
}

/// 管家确定性内核的整体句柄。表达层与通知层是它的消费者。
#[derive(Debug, Clone, Default)]
pub struct Butler {
    pub state: ButlerState,
    pub memory: Memory,
    pub tasks: TaskBoard,
}

impl ButlerState {
    pub fn path() -> std::path::PathBuf {
        butler_dir().join("state.json")
    }

    pub fn load() -> Result<Self> {
        Ok(read_json(&Self::path())?.unwrap_or_default())
    }

    pub fn save(&self) -> Result<()> {
        write_json(&Self::path(), self)
    }
}

impl Butler {
    pub fn load() -> Result<Self> {
        let mut state = ButlerState::load()?;
        // 频控数值以用户配置为准；开关（enabled）仍归 `state.json`，因为
        // 它是「能不能说」的意图，不是额度。配置缺失或损坏时保持
        // `state.json` 里的值，不让一个坏文件把管家变成哑巴。
        if let Ok(config) = crate::config::AppConfig::load() {
            let policy = &config.butler;
            state.policy.daily_cap = policy.daily_cap.max(1);
            state.policy.weekly_cap = policy.weekly_cap.max(state.policy.daily_cap);
            state.policy.dedupe_days = policy.dedupe_days.max(1);
            state.policy.quiet_start_hour = policy.quiet_start_hour.min(23);
            state.policy.quiet_end_hour = policy.quiet_end_hour.min(24);
        }
        Ok(Self {
            state,
            memory: Memory::load()?,
            tasks: TaskBoard::load()?,
        })
    }

    /// 落盘 state + memory + tasks 三个文件。**调用方必须已持有写锁**
    /// （[`crate::butler::writer_lock`]）：本函数自己不加锁，「load → 改 →
    /// save」必须是一个临界区，跨进程互斥由最外层负责。
    ///
    /// 不要在持锁路径里再嵌套调用会自己拿锁的接口（如 `Memory::update`）——
    /// 同进程两次 `writer_lock()` 打开的是不同文件描述符，flock 会互相等满
    /// 2 秒超时后降级为无锁写入。
    pub fn save(&self) -> Result<()> {
        self.state.save()?;
        self.memory.save()?;
        self.tasks.save()
    }

    /// 只写状态文件。`state.json` 有三个潜在写者（GUI 巡检、守护进程、
    /// CLI），只改开关或去重记录时不该顺手重写记忆和任务——那会把另一个
    /// 进程刚写进去的条目覆盖回旧值。
    pub fn save_state(&self) -> Result<()> {
        self.state.save()
    }

    pub fn save_memory(&self) -> Result<()> {
        self.memory.save()
    }

    pub fn save_tasks(&self) -> Result<()> {
        self.tasks.save()
    }

    /// 收到一次唤醒请求。返回值里的 `speak` 既可能是一条通知，也可能是
    /// 「判过、决定不说」以及原因——两种都要能回答「今天为什么没动静」。
    pub fn wake(
        &mut self,
        source: &WakeSource,
        now: DateTime<Local>,
        facts: &AccountFacts,
    ) -> WakeReport {
        let wake = self.state.scheduler.request(source, now);
        if wake != WakeVerdict::Wake {
            return WakeReport { wake, speak: None };
        }

        self.maintain_if_due(now);

        let mut due = self.tasks.due(now);
        // 被这条唤醒源点名的那条任务排最前：多条同时到点时，先把「叫醒
        // 管家的那一条」处理掉，别让另一条同优先级任务把它挤成幽灵。
        if let WakeSource::Task { id } = source {
            due.sort_by_key(|task| task.id != *id);
        }
        let rows = candidates(facts, &due, now);
        let decision = schedule::decide(&rows, now, &self.state.policy, &self.state.ledger);
        match &decision {
            SpeakDecision::Speak(row) => {
                self.state.ledger.record(now, row);
                // 到点任务说出去就算处理完：留着会在待办里挂 24 小时，
                // 用户看到「还没执行完」，GUI 也会每分钟重试同一条。
                if let Some(id) = row.trigger.strip_prefix("task:") {
                    self.tasks.complete(id, now);
                }
            }
            // 同一任务在崩溃重放时已经说过：补做完成，别留幽灵。
            SpeakDecision::Silent(super::schedule::SilentBecause::Duplicate) => {
                if let WakeSource::Task { id } = source {
                    self.tasks.complete(id, now);
                }
            }
            _ => {}
        }
        // 只保留最近 30 天的开口记录：频控只需要一周，留 30 天足够审计。
        self.state.ledger.trim(now, 30);

        WakeReport {
            wake,
            speak: Some(decision),
        }
    }

    /// 每周自整理是否到期。首次运行也要整理一次，但空状态整理是幂等的。
    pub fn maintenance_due(&self, now: DateTime<Local>) -> bool {
        self.state
            .last_maintenance
            .is_none_or(|last| now - last >= chrono::Duration::days(MAINTENANCE_INTERVAL_DAYS))
    }

    /// 自整理：记忆只合并不删除，任务只收敛不丢审计。失败不阻断唤醒。
    fn maintain_if_due(&mut self, now: DateTime<Local>) {
        if !self.maintenance_due(now) {
            return;
        }
        self.memory.compact(now.date_naive());
        self.tasks.review(now);
        self.state.scheduler.trim_seen(200);
        self.state.last_maintenance = Some(now);
    }
}

/// 账本事件 → 唤醒源。没有 id 的事件（旧版本 GUI 写的日志）用时间戳兜底，
/// 保证同一行日志被重复读取时仍然只唤醒一次。
pub fn ledger_wake(event: &super::events::BehaviorEvent) -> Option<WakeSource> {
    if event.kind != super::events::kind::TRADE_RECORDED {
        return None;
    }
    let stamp = event.at.timestamp_millis();
    Some(WakeSource::Ledger {
        id: format!("trade:{stamp}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::butler::tasks::TaskVerdict;
    use chrono::TimeZone;

    fn at(day: u32, hour: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 11, day, hour, 0, 0)
            .single()
            .unwrap()
    }

    fn drift(key: &str, name: &str, current: f64, target: f64) -> DriftRow {
        DriftRow {
            key: key.to_owned(),
            name: name.to_owned(),
            current_pct: current,
            target_pct: target,
            drift_pp: current - target,
            market_value: 0.0,
        }
    }

    fn facts() -> AccountFacts {
        AccountFacts {
            decision_day: true,
            band_pp: 4.0,
            drift: vec![
                drift("us_stocks", "美股", 41.2, 35.0),
                drift("cn_stocks", "A股", 30.0, 32.0),
            ],
            ..AccountFacts::default()
        }
    }

    #[test]
    fn 负零不会念成负号() {
        assert_eq!(format!("{:.1}", clean_zero(-0.0)), "0.0");
        assert_eq!(clean_zero(-0.04), -0.04);
    }

    /// 旧价格不说偏离：价格 ≥24h 没更新时，依赖估值的判断全部砍掉。
    #[test]
    fn 价格过旧时砍掉偏离与计划可执行() {
        let mut stale = facts();
        stale.plan_ready = true;
        stale.new_cash_today = 5000.0;
        apply_price_gate(&mut stale, Some(24));
        assert!(stale.drift.is_empty(), "旧价格不能支撑「偏离多少」");
        assert!(!stale.plan_ready, "旧估值下的计划不该当成可执行");
        // 不依赖价格的判断原样保留：当日入金是台账事实，决策日按当日计算。
        assert!(stale.decision_day);
        assert_eq!(stale.new_cash_today, 5000.0);

        let mut fresh = facts();
        fresh.plan_ready = true;
        apply_price_gate(&mut fresh, Some(23));
        assert_eq!(fresh.drift.len(), 2, "新鲜价格保留偏离");
        assert!(fresh.plan_ready);
    }

    /// 空账户没有「价格年龄」：纯现金账户的定投计划不依赖任何价格。
    #[test]
    fn 空账户不因缺价格砍掉计划() {
        let mut cash_only = facts();
        cash_only.plan_ready = true;
        apply_price_gate(&mut cash_only, None);
        assert_eq!(cash_only.drift.len(), 2);
        assert!(cash_only.plan_ready, "无持仓时 price_age 为 None，不该降级");
    }

    #[test]
    fn 决策日偏离超带宽时给出可执行下一步() {
        let rows = candidates(&facts(), &[], at(20, 9));
        let row = rows
            .iter()
            .find(|row| row.reason == SpeakReason::ActionableStep)
            .expect("应有可执行下一步");
        assert!(row.trigger.contains("us_stocks"), "应挑偏离最大的腿");
        assert!(row.draft.contains("美股"));
        assert!(row.draft.contains("6.2pp"), "实际 {}", row.draft);
        assert!(schedule::validate_notification(&row.draft).is_ok());
    }

    #[test]
    fn 偏离都在带宽内就不打扰() {
        let mut facts = facts();
        facts.drift = vec![drift("us_stocks", "美股", 36.0, 35.0)];
        let rows = candidates(&facts, &[], at(20, 9));
        assert!(rows.is_empty(), "实际 {rows:?}");
    }

    #[test]
    fn 非决策日不因偏离开口() {
        let mut facts = facts();
        facts.decision_day = false;
        let rows = candidates(&facts, &[], at(19, 9));
        // 当前实现里决策日只作为 AccountFacts 的事实位；非决策日不应再产生
        // 偏离候选。这里直接断言没有 ActionableStep。
        assert!(
            !rows
                .iter()
                .any(|row| row.reason == SpeakReason::ActionableStep)
        );
    }

    #[test]
    fn 任务到点优先于偏离提醒() {
        let mut tasks = TaskBoard::default();
        let TaskVerdict::Accepted { id } = tasks.propose(
            ButlerTask::new(
                "nonfarm:2026-12-04",
                "到点检查当天是否有计划外操作",
                at(20, 8),
                at(16, 9),
            ),
            at(16, 9),
        ) else {
            panic!("任务应被接受");
        };
        let due = tasks.due(at(20, 9));
        let rows = candidates(&facts(), &due, at(20, 9));
        match schedule::decide(
            &rows,
            at(20, 9),
            &SpeakPolicy::default(),
            &SpeakLedger::default(),
        ) {
            SpeakDecision::Speak(row) => {
                assert_eq!(row.reason, SpeakReason::TaskDue);
                assert_eq!(row.trigger, format!("task:{id}"));
            }
            other => panic!("应当在任务到点时开口，实际 {other:?}"),
        }
    }

    #[test]
    fn 到点任务开口后立刻完成而不是留成幽灵() {
        let mut butler = Butler::default();
        let TaskVerdict::Accepted { id } = butler.tasks.propose(
            ButlerTask::new("t", "到点检查当天是否有计划外操作", at(20, 9), at(20, 8)),
            at(20, 8),
        ) else {
            panic!("任务应被接受");
        };
        let report = butler.wake(
            &WakeSource::Task { id: id.clone() },
            at(20, 9),
            &AccountFacts::default(),
        );
        assert!(matches!(report.speak, Some(SpeakDecision::Speak(_))));
        assert!(
            butler.tasks.pending().is_empty(),
            "说出去的任务不能继续挂在待办里"
        );
        assert_eq!(butler.tasks.archived.len(), 1);
        assert_eq!(
            butler.tasks.archived[0].state,
            crate::butler::TaskState::Done
        );
    }

    #[test]
    fn 到点问候用问候语而不是复读任务原文() {
        let mut board = TaskBoard::default();
        let TaskVerdict::Accepted { id } = board.propose_as(
            ButlerTask::new("talk", "到点主动打个招呼", at(20, 9), at(20, 8))
                .with_kind(TaskKind::Talk),
            at(20, 8),
            crate::butler::tasks::TaskOrigin::User,
        ) else {
            panic!("任务应被接受");
        };
        let due = board.due(at(20, 9));
        let rows = candidates(&AccountFacts::default(), &due, at(20, 9));
        let row = rows
            .iter()
            .find(|row| row.trigger == format!("task:{id}"))
            .expect("应有到点候选");
        assert!(row.draft.contains("我在"), "实际 {}", row.draft);
        assert!(
            !row.draft.contains("主动打个招呼"),
            "不该复读任务原文，实际 {}",
            row.draft
        );
        assert!(schedule::validate_notification(&row.draft).is_ok());
    }

    #[test]
    fn 高频查看不再单独构成开口由头() {
        // 只看高频查看就开口会变成催促；而项目没有指数级行情，无法判断
        // 「市场动了多少」。所以这条由头整体移除，行为日志只留作上下文。
        let mut facts = facts();
        facts.drift.clear();
        facts.heavy_page = Some(("总览".to_owned(), 6));

        let rows = candidates(&facts, &[], at(18, 9));
        assert!(
            rows.is_empty(),
            "没有任务/可执行下一步/到期承诺时应当沉默，实际 {rows:?}"
        );
    }

    #[test]
    fn 承诺到期才提醒() {
        let mut memory = Memory::default();
        memory.remember(
            crate::butler::memory::Fact::new(FactKind::Commitment, "周五补美股", at(13, 9))
                .expires_on(NaiveDate::from_ymd_opt(2026, 11, 20).unwrap()),
        );
        let today = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();

        let facts = AccountFacts::default().with_promises(&memory, today);
        assert_eq!(facts.promises_due, vec!["周五补美股".to_owned()]);

        let rows = candidates(&facts, &[], at(20, 9));
        let row = rows
            .iter()
            .find(|row| row.reason == SpeakReason::PromiseDue)
            .expect("到期承诺应开口");
        assert!(schedule::validate_notification(&row.draft).is_ok());

        let early = AccountFacts::default()
            .with_promises(&memory, NaiveDate::from_ymd_opt(2026, 11, 19).unwrap());
        assert!(early.promises_due.is_empty(), "没到期不该提前催");
    }

    #[test]
    fn 开口后同一事件进入去重() {
        let mut butler = Butler::default();
        let source = WakeSource::Scheduled;
        let first = butler.wake(&source, at(20, 9), &facts());
        assert!(matches!(first.speak, Some(SpeakDecision::Speak(_))));

        let second = butler.wake(&source, at(21, 9), &facts());
        assert_eq!(
            second.speak,
            Some(SpeakDecision::Silent(schedule::SilentBecause::Duplicate))
        );
    }

    #[test]
    fn 每周自整理在唤醒时顺带执行() {
        let mut butler = Butler::default();
        butler.memory.facts.push(crate::butler::memory::Fact::new(
            FactKind::Timeline,
            "发薪日 15 号",
            at(10, 9),
        ));
        butler.memory.facts.push(crate::butler::memory::Fact::new(
            FactKind::Timeline,
            "发薪日15号",
            at(10, 9),
        ));
        butler.state.last_maintenance = Some(at(1, 9));

        let report = butler.wake(&WakeSource::Scheduled, at(20, 9), &AccountFacts::default());
        assert_eq!(report.wake, WakeVerdict::Wake);
        assert_eq!(butler.memory.facts.len(), 1, "同义事实应被合并");
        assert_eq!(butler.state.last_maintenance, Some(at(20, 9)));
        assert!(!butler.maintenance_due(at(26, 9)));
        assert!(butler.maintenance_due(at(27, 9)), "满七天即到期");
    }

    #[test]
    fn 未被唤醒时不做开口判定() {
        let mut butler = Butler::default();
        assert_eq!(
            butler
                .wake(&WakeSource::Scheduled, at(20, 9), &facts())
                .wake,
            WakeVerdict::Wake
        );
        let report = butler.wake(&WakeSource::Scheduled, at(20, 10), &facts());
        assert!(matches!(report.wake, WakeVerdict::TooSoon { .. }));
        assert!(report.speak.is_none(), "没唤醒就不该判开不开口");
    }

    #[test]
    fn 失败退避从十五分钟起步六小时封顶() {
        use chrono::Duration;
        // 没有失败：常规节奏 12 小时。
        assert_eq!(agent_init_backoff(0), Duration::hours(12));
        // 失败 1 次 15 分钟、2 次 1 小时、之后一直 6 小时。
        assert_eq!(agent_init_backoff(1), Duration::minutes(15));
        assert_eq!(agent_init_backoff(2), Duration::hours(1));
        assert_eq!(agent_init_backoff(3), Duration::hours(6));
        assert_eq!(agent_init_backoff(99), Duration::hours(6));
    }

    #[test]
    fn 决策日志追加后能按条数读回() {
        use super::{
            AgentInitOutcome, AgentInitRecord, append_init_record_at, read_init_records_at,
        };

        let dir = std::env::temp_dir().join(format!("mns-butler-initlog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("initlog.jsonl");

        for index in 0..5 {
            append_init_record_at(
                &path,
                &AgentInitRecord {
                    at: at(20, 9),
                    outcome: if index % 2 == 0 {
                        AgentInitOutcome::Failed
                    } else {
                        AgentInitOutcome::Silent
                    },
                    detail: Some(format!("第 {index} 次")),
                },
            )
            .unwrap();
        }
        append_init_record_at(
            &path,
            &AgentInitRecord {
                at: at(21, 9),
                outcome: AgentInitOutcome::Spoke,
                detail: None,
            },
        )
        .unwrap();

        let last = read_init_records_at(&path, 3);
        assert_eq!(last.len(), 3, "只应保留最近几条");
        assert_eq!(last[0].outcome, AgentInitOutcome::Silent, "第 4 次是沉默");
        assert_eq!(last[2].outcome, AgentInitOutcome::Spoke, "最后一条是开口");
        assert_eq!(last[2].detail, None);
        assert_eq!(
            read_init_records_at(&path, 99).len(),
            6,
            "limit 大于总数时全读"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 通知id由时间与触发派生且稳定() {
        let first = notice_id_for(at(20, 9), "task:abc");
        assert_eq!(first, notice_id_for(at(20, 9), "task:abc"), "同源必须同 id");
        assert_ne!(
            first,
            notice_id_for(at(20, 10), "task:abc"),
            "时间不同则不同"
        );
        assert_ne!(
            first,
            notice_id_for(at(20, 9), "task:def"),
            "触发不同则不同"
        );
        assert!(
            first.starts_with('n') && first.contains('-'),
            "实际 {first}"
        );
    }

    #[test]
    fn 旧pending行读取时补id且坏行跳过() {
        let dir = std::env::temp_dir().join(format!("mns-butler-pending-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pending.jsonl");
        let old = serde_json::json!({
            "at": at(20, 9),
            "reason": "TaskDue",
            "trigger": "task:old",
            "draft": "旧格式通知"
        });
        std::fs::write(
            &path,
            format!("{}\nnot-json\n", serde_json::to_string(&old).unwrap()),
        )
        .unwrap();

        let (consumed, notices) = take_pending_at(&path, 0).unwrap();
        assert_eq!(notices.len(), 1, "坏行应跳过而不是报错");
        assert_eq!(consumed, std::fs::metadata(&path).unwrap().len());
        assert_eq!(notices[0].notice_id, notice_id_for(at(20, 9), "task:old"));
        assert_eq!(
            take_pending_at(&path, consumed).unwrap().1.len(),
            0,
            "偏移之后没有新行"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 送达与点击按id各只记一次且能读回原文() {
        let dir = std::env::temp_dir().join(format!("mns-butler-notices-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("notices.jsonl");
        let notice = Notice {
            at: at(20, 9),
            reason: SpeakReason::TaskDue,
            trigger: "task:x".to_owned(),
            draft: "该看计划了".to_owned(),
            notice_id: notice_id_for(at(20, 9), "task:x"),
        };

        assert!(record_notice_delivered_at(&path, &notice).unwrap());
        assert!(
            !record_notice_delivered_at(&path, &notice).unwrap(),
            "同一 id 的送达只记一次"
        );
        assert!(record_notice_clicked_at(&path, &notice.notice_id).unwrap());
        assert!(
            !record_notice_clicked_at(&path, &notice.notice_id).unwrap(),
            "同一 id 的点击只记一次"
        );

        let events = read_notice_events_at(&path);
        assert_eq!(events.len(), 2, "一次送达 + 一次点击");
        let delivered = delivered_notice_at(&path, &notice.notice_id).expect("应能读回送达记录");
        assert_eq!(delivered.draft.as_deref(), Some("该看计划了"));
        assert_eq!(delivered.notice_at, Some(at(20, 9)));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
