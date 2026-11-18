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
use super::tasks::{ButlerTask, TaskBoard};

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
/// 这份结构同时是 GUI 与守护进程之间的契约文件（`agent/butler/facts.json`）：
/// 守护进程不碰 `mns.db`，只读 GUI 算好的事实，所以「管家只读账本」是结构
/// 保证而不是策略约定。
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
        rows.push(candidate(
            SpeakReason::TaskDue,
            format!("task:{}", task.id),
            format!("到点了：{}", task.intent),
            "到点了，按你自己定的那件事检查一下。",
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
}

/// 把一条已授权的通知放进出站队列。写失败只影响送达，不影响判定。
pub fn enqueue_notice(at: DateTime<Local>, candidate: &SpeakCandidate) -> Result<()> {
    let notice = Notice {
        at,
        reason: candidate.reason,
        trigger: candidate.trigger.clone(),
        draft: candidate.draft.clone(),
    };
    let line = serde_json::to_string(&notice)?;
    super::store::append_line(&outbox_path(), &line)
}

/// 管家的判断事实。GUI 每次巡检算完就落一份，守护进程只读这一份——
/// 它不打开 `mns.db`，所以「只读账本」是结构保证。
pub fn facts_path() -> std::path::PathBuf {
    butler_dir().join("facts.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FactsFile {
    at: DateTime<Local>,
    facts: AccountFacts,
}

/// 写入当前事实。失败只影响守护进程本轮能不能判断，不影响 GUI。
pub fn write_facts(at: DateTime<Local>, facts: &AccountFacts) -> Result<()> {
    write_json(
        &facts_path(),
        &FactsFile {
            at,
            facts: facts.clone(),
        },
    )
}

/// 读回事实与它的生成时间。调用方必须检查新鲜度：拿十分钟前的数字开口，
/// 等于用一个已经不存在的仓位提醒用户。
pub fn read_facts() -> Result<Option<(DateTime<Local>, AccountFacts)>> {
    Ok(read_json::<FactsFile>(&facts_path())?.map(|file| (file.at, file.facts)))
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
    };
    let line = serde_json::to_string(&notice)?;
    super::store::append_line(&pending_path(), &line)
}

/// 从字节偏移读走新的待投递条目，并返回新的偏移。
///
/// 用偏移而不是「读完就清空」：守护进程可能在 GUI 读与清之间追加，清空会
/// 丢掉那一条。文件每天最多长三行，不值得为它做压缩。
pub fn take_pending(offset: u64) -> Result<(u64, Vec<Notice>)> {
    use std::io::{Read, Seek, SeekFrom};
    let path = pending_path();
    let Ok(mut file) = std::fs::File::open(&path) else {
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
        .filter_map(|line| serde_json::from_str::<Notice>(line).ok())
        .collect();
    Ok((consumed, notices))
}

/// 已消费到的偏移。GUI 独占这个文件。
pub fn pending_offset_path() -> std::path::PathBuf {
    butler_dir().join("pending.offset")
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

        let due = self.tasks.due(now);
        let rows = candidates(facts, &due, now);
        let decision = schedule::decide(&rows, now, &self.state.policy, &self.state.ledger);
        if let SpeakDecision::Speak(row) = &decision {
            self.state.ledger.record(now, row);
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
        assert_eq!(read_init_records_at(&path, 99).len(), 6, "limit 大于总数时全读");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
