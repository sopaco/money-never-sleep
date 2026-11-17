//! 表达层的确定性底线。
//!
//! 管家是自主人格（`docs/butler-persona-upgrade-2026-11-17.md`）：主路是
//! 外接 ACP agent，可以聊观点、聊市场。这一层是**本地兜底**——agent 不可
//! 用、超时、答非所问时的退路，以及意图路由的入口。模板只做两件事：复述
//! 账本同源数字，接不住的问题如实说「这条我答不好，说说我有把握的部分」。
//!
//! 诚实性约束不因人格放开而撤：数字白名单（`adopt_*`）照旧，不谎称能下单
//! 照旧；不再有的是策略性话术审查——倾向性表述是管家的职责，不是越界。

use chrono::Local;

use super::orchestrator::AccountFacts;
use super::schedule;

/// 用户问题的意图。
///
/// 这是**确定性的关键词路由**，不是语义理解。宁可落到 `Unknown` 并如实说
/// 「没接住」，也不要猜错方向答非所问——用户问「今天有什么大事」却听到
/// 「你想核对哪一段」，比直接说不懂更伤信任。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// 「你能做什么」——能力边界。
    Capability,
    /// 「你是谁」——身份。
    Identity,
    /// 「分析持仓 / 我的仓位」——资产结构。
    Holdings,
    /// 「偏离多少 / 配置比例」——目标与现状。
    Drift,
    /// 「本月计划 / 要买多少」——逐腿金额。
    Plan,
    /// 「要不要买 / 该不该动」——动作判断（只给偏离，不给买卖点）。
    Action,
    /// 「哪天决策」。
    DecisionDay,
    /// 「今天有什么」——账本当天要点，不是外部新闻。
    Today,
    /// 「明天涨不涨」——明确拒答。
    Forecast,
    /// 没接住。
    Unknown,
}

/// 用户问「现在要不要动 / 买 / 卖」时的回答。
pub fn reply(question: &str, facts: &AccountFacts) -> String {
    reply_with_context(question, None, facts)
}

/// 带上一位用户问题。用户追问「那 A股呢」时，单看这一句没有任何意图词，
/// 但接着上一句的意图答才像在对话，而不是每次都从零开始。
///
/// 只在**当前句完全没命中**且**确实是追问句式**时才沿用上一句意图——
/// 宁可诚实说没接住，也不要凭一个「那」字猜错方向。
pub fn reply_with_context(question: &str, previous: Option<&str>, facts: &AccountFacts) -> String {
    let text = question.trim();
    if text.is_empty() {
        return valid_or(default_reply(facts), facts);
    }
    compose(intent_for(text, previous), text, facts)
}

/// 判定意图，带上一句的承接。
///
/// GUI 用它决定「这一问要不要交给外部 agent」——涨跌预测在本地就拒答，
/// 不必启一次会话。`reply_with_context` 内部走的也是同一条路。
pub fn intent_for(question: &str, previous: Option<&str>) -> Intent {
    let text = question.trim();
    if text.is_empty() {
        return Intent::Unknown;
    }
    let mut intent = classify(text);
    if intent == Intent::Unknown
        && let Some(previous) = previous
        && is_follow_up(text)
    {
        let carried = classify(previous);
        if !matches!(carried, Intent::Unknown | Intent::Forecast) {
            intent = carried;
        }
    }
    intent
}

/// 意图已经判定好、只要模板文案时用这个入口。
///
/// 两条路径共用它：一是本地兜底（agent 不可用、超时、越界），二是预测类
/// 问题的固定拒答。关键词路由只决定「用哪段模板」，不决定数字——数字始终
/// 从账本取，边界始终由 `validate_dialogue` 守。
pub fn reply_with_intent(intent: Intent, text: &str, facts: &AccountFacts) -> String {
    compose(intent, text, facts)
}

/// 模板生成。所有入口最终都汇到这里。
///
/// 用户要「操作建议」而模板只有账面事实时，不再追加边界句（「买卖点我不
/// 给」）——事实本身就是兜底答案，agent 在线时观点由 agent 给。
fn compose(intent: Intent, text: &str, facts: &AccountFacts) -> String {
    let mut body = render(intent, facts);

    // 复合问题：「分析我的持仓，还有今天有什么大事」这类，主意图答完之后
    // 补一句当天要点。补的时候要**去掉主回答已经说过的**——把同一句偏离
    // 念两遍，比不补更糟。
    if intent != Intent::Today && intent != Intent::Forecast && mentions_today(text) {
        let joined = format!(
            "{body}{}",
            today_brief(
                facts,
                !matches!(intent, Intent::Holdings | Intent::Drift | Intent::Action),
                intent != Intent::Action,
            )
        );
        if schedule::validate_dialogue(&joined).is_ok() {
            body = joined;
        }
    }

    valid_or(body, facts)
}

/// 追问句式：短、且带指代或递进词。纯「1」这类不算——那是没听懂，
/// 不该被当成「接着上一句」。
fn is_follow_up(text: &str) -> bool {
    text.chars().count() <= 15
        && contains_any(text, &["那", "呢", "还有", "它", "这个", "再说", "具体"])
}

/// 意图分类。顺序即优先级：越具体的意图越先判，避免被「买」「今天」这类
/// 高频字抢走。
pub fn classify(question: &str) -> Intent {
    let text = question.trim();
    if contains_any(
        text,
        &[
            "能做什么",
            "能干什么",
            "会做什么",
            "你会什么",
            "有什么功能",
            "怎么用",
            "能帮我",
        ],
    ) {
        return Intent::Capability;
    }
    if contains_any(text, &["你是谁", "你是什么", "介绍一下", "你好", "在吗"]) {
        return Intent::Identity;
    }
    if contains_any(
        text,
        &[
            "涨",
            "跌",
            "预测",
            "后市",
            "行情",
            "走势",
            "推荐",
            "目标价",
            "买什么好",
            "抄底",
        ],
    ) {
        return Intent::Forecast;
    }
    if contains_any(
        text,
        &["计划", "定投", "下单", "买多少", "这周买", "本月买"],
    ) {
        return Intent::Plan;
    }
    if contains_any(
        text,
        &[
            "持仓",
            "仓位",
            "结构",
            "分析",
            "总资产",
            "资产",
            "多少钱",
            "本金",
        ],
    ) {
        return Intent::Holdings;
    }
    if contains_any(text, &["偏离", "配置", "比例", "权重", "带宽", "目标"]) {
        return Intent::Drift;
    }
    if contains_any(text, &["决策日", "哪天", "什么时候"]) {
        return Intent::DecisionDay;
    }
    if contains_any(
        text,
        &[
            "买",
            "卖",
            "加仓",
            "减仓",
            "操作",
            "要不要动",
            "该做",
            "动手",
        ],
    ) {
        return Intent::Action;
    }
    if mentions_today(text) {
        return Intent::Today;
    }
    Intent::Unknown
}

fn contains_any(text: &str, words: &[&str]) -> bool {
    words.iter().any(|word| text.contains(word))
}

fn mentions_today(text: &str) -> bool {
    contains_any(text, &["今天", "今日", "大事", "发生", "注意"])
}

fn render(intent: Intent, facts: &AccountFacts) -> String {
    match intent {
        Intent::Capability => capability_reply(),
        Intent::Identity => identity_reply(),
        Intent::Holdings => holdings_reply(facts),
        Intent::Drift => drift_reply(facts),
        Intent::Plan => plan_reply(facts),
        Intent::Action => action_reply(facts),
        Intent::DecisionDay => decision_reply(facts),
        Intent::Today => today_reply(facts),
        Intent::Forecast => forecast_reply(),
        Intent::Unknown => unknown_reply(facts),
    }
}

fn capability_reply() -> String {
    "账本我盯着：偏离、计划、持仓、决策日随口问。市场看法、操作倾向我也能聊——\
     那是我的看法，你拍板。数字我只念账本里的，不编；下单我帮不了，得你自己来。"
        .to_owned()
}

fn identity_reply() -> String {
    "我是你的私人投资管家，常驻本机，读这一本账。账面数字、市场看法、你交代的事，\
     都可以谈；观点是我的，决定是你的。"
        .to_owned()
}

fn holdings_reply(facts: &AccountFacts) -> String {
    if facts.drift.is_empty() {
        return "账本里还没有持仓分类，分析无从下手。先记一笔入金和持仓，我才有数可核对。"
            .to_owned();
    }

    let mut parts: Vec<String> = Vec::new();
    // 总资产为 0 时宁可不提，也不念一句「总资产 ¥0」。
    if facts.total_assets > 0.0 {
        parts.push(format!(
            "总资产 {}，其中持仓 {}、现金 {}，共 {} 个标的",
            money(facts.total_assets),
            money(facts.holdings_value),
            money(facts.cash),
            facts.positions_count
        ));
    }

    let rows: Vec<String> = facts
        .drift
        .iter()
        .take(3)
        .map(|row| {
            format!(
                "{} {:.1}%（目标 {:.1}%）",
                row.name, row.current_pct, row.target_pct
            )
        })
        .collect();
    parts.push(rows.join("；"));

    let tail = match facts
        .widest_drift()
        .filter(|row| row.excess_pp(facts.band_pp) > 0.0)
    {
        Some(row) => format!(
            "偏离最大的是{}，{:.1}pp 超出 ±{:.1}pp 带宽",
            row.name,
            row.drift_pp.abs(),
            facts.band_pp
        ),
        None => format!("各腿都在 ±{:.1}pp 带宽内，结构没走样", facts.band_pp),
    };
    parts.push(tail);

    format!("{}。{}。", parts[0], parts[1..].join("。"))
}

fn drift_reply(facts: &AccountFacts) -> String {
    if facts.drift.is_empty() {
        return "还没有持仓，偏离无从谈起。先记一笔入金和持仓，我才有数可核对。".to_owned();
    }
    let rows: Vec<String> = facts
        .drift
        .iter()
        .take(3)
        .map(|row| {
            format!(
                "{} {:.1}%（目标 {:.1}%）",
                row.name, row.current_pct, row.target_pct
            )
        })
        .collect();
    let tail = match facts
        .widest_drift()
        .filter(|row| row.excess_pp(facts.band_pp) > 0.0)
    {
        Some(row) => format!(
            "超出 ±{:.1}pp 带宽的是{}，偏离 {:.1}pp。",
            facts.band_pp,
            row.name,
            row.drift_pp.abs()
        ),
        None => format!("都在 ±{:.1}pp 带宽内。", facts.band_pp),
    };
    format!("{}。{tail}", rows.join("；"))
}

fn plan_reply(facts: &AccountFacts) -> String {
    if let Some(blocked) = &facts.plan_blocked {
        return format!("本月计划被拦下了：{blocked}。这是计划页的结论，不是我另算的。");
    }
    if facts.plan_lines.is_empty() {
        return "本月计划没有可执行的下单：各腿都在带宽内，或者还没到该动的时候。".to_owned();
    }
    let rows: Vec<String> = facts
        .plan_lines
        .iter()
        .take(4)
        .map(|line| {
            let action = if line.amount >= 0.0 { "买" } else { "卖" };
            format!("{} {action} {}", line.name, money(line.amount.abs()))
        })
        .collect();
    format!(
        "本月计划 {} 笔：{}。按不按计划走由你定，下单得你自己来。",
        facts.plan_lines.len(),
        rows.join("；")
    )
}

fn action_reply(facts: &AccountFacts) -> String {
    let over = facts
        .widest_drift()
        .filter(|row| row.excess_pp(facts.band_pp) > 0.0);
    if let Some(row) = over {
        let direction = if row.drift_pp > 0.0 {
            "高配"
        } else {
            "低配"
        };
        let when = if facts.decision_day {
            "今天是决策日"
        } else {
            "今天不是决策日"
        };
        return format!(
            "{when}，{}{direction} {:.1}pp，超出 ±{:.1}pp 带宽。账面就是这些；怎么处理你定，下单我帮不了。",
            row.name,
            row.drift_pp.abs(),
            facts.band_pp
        );
    }
    if facts.new_cash_today > 0.5 && facts.plan_ready {
        return format!(
            "新钱 {} 已到账，本月计划有可执行的下单。计划页有逐腿金额，按不按计划走由你定。",
            money(facts.new_cash_today)
        );
    }
    "眼下账面没有要动的事：各腿在带宽内，也没有新钱触发计划。要不要等下一个决策日，你定。"
        .to_owned()
}

fn decision_reply(facts: &AccountFacts) -> String {
    if facts.decision_day {
        return "今天是决策日。要看的就两件：偏离有没有超带宽、计划有没有可执行的下单。".to_owned();
    }
    match &facts.next_decision_day {
        Some(day) => format!(
            "今天不是决策日，下一个是 {}。中间不催你动手。",
            short_date(day)
        ),
        None => "每天都是决策日，所以每天都按同一套口径看偏离和计划。".to_owned(),
    }
}

fn today_reply(facts: &AccountFacts) -> String {
    // 这里最容易编：用户问「今天有什么大事」，默认期待的是外部新闻。管家
    // 没有新闻源，也不该假装有，先说清边界再给账本侧的事实。
    format!(
        "外面今天有什么新闻，我不知道，也不编。{}",
        today_brief(facts, true, true)
    )
}

/// 账本当天的要点，一到两句。
///
/// `with_drift` / `with_decision` 用来避免和主回答重复：复合问题里主回答
/// 已经念过偏离，这里就不该再念一遍。
fn today_brief(facts: &AccountFacts, with_drift: bool, with_decision: bool) -> String {
    let mut bits: Vec<String> = Vec::new();
    if with_decision {
        bits.push(
            if facts.decision_day {
                "今天是决策日"
            } else {
                "今天不是决策日"
            }
            .to_owned(),
        );
    }
    if with_drift {
        match facts
            .widest_drift()
            .filter(|row| row.excess_pp(facts.band_pp) > 0.0)
        {
            Some(row) => bits.push(format!(
                "{}偏离 {:.1}pp，超出 ±{:.1}pp 带宽",
                row.name,
                row.drift_pp.abs(),
                facts.band_pp
            )),
            None => bits.push(format!("各腿都在 ±{:.1}pp 带宽内", facts.band_pp)),
        }
    }

    let mut additions: Vec<String> = Vec::new();
    if facts.new_cash_today > 0.5 {
        additions.push(format!("今日入金 {}", money(facts.new_cash_today)));
    }
    if !facts.promises_due.is_empty() {
        additions.push(format!("{} 件承诺今天到期", facts.promises_due.len()));
    }
    if let Some((page, views)) = &facts.heavy_page {
        additions.push(format!("你已看了{page}页 {views} 次"));
    }
    if stale(facts) {
        additions.push("数据不是今天的，建议先更新".to_owned());
    }
    if additions.is_empty() && !with_drift {
        additions.push("没有入金，也没有到期承诺".to_owned());
    }
    bits.extend(additions);

    format!("账本这边：{}。", bits.join("，"))
}

fn forecast_reply() -> String {
    // 模板是 agent 不在时的兜底：它没有市场数据，诚实的说法是「说不准」，
    // 再把有把握的账面递上去——不是拒答，是只答有把握的部分。
    "涨跌我说不准，不装确定。有把握的在账面：你现在偏离多少、计划怎么排，\
     这两个随时问；想听我展开聊，等 agent 在线再说。"
        .to_owned()
}

fn unknown_reply(facts: &AccountFacts) -> String {
    format!(
        "这句我本地没接住。账本、计划、偏离、市场看法都能聊，换种问法试试。{}",
        today_brief(facts, true, true)
    )
}

fn default_reply(facts: &AccountFacts) -> String {
    format!("我在。{}", today_brief(facts, true, true))
}

fn fallback(facts: &AccountFacts) -> String {
    format!(
        "我在。{}你想问哪一段，我照实说。",
        today_brief(facts, true, true)
    )
}

/// 文案过不了校验时不能退回一句固定话——那正是「复读机」的来源。退到带
/// 当天状态的兜底句，至少还在回答「现在怎么样」。
fn valid_or(body: String, facts: &AccountFacts) -> String {
    if schedule::validate_dialogue(&body).is_ok() {
        return body;
    }
    let degraded = fallback(facts);
    if schedule::validate_dialogue(&degraded).is_ok() {
        degraded
    } else {
        "我在。".to_owned()
    }
}

/// 金额带千分位。管家念数字是给人听的，`¥12340` 得在脑子里点逗号。
///
/// 对 `talk` 也开放：交给外部 agent 的账本上下文用同一份格式，避免同一个
/// 金额在模板与对话里长得不一样。
pub fn money(value: f64) -> String {
    let rounded = value.round() as i64;
    let digits = rounded.abs().to_string();
    let mut out = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if rounded < 0 {
        format!("-¥{out}")
    } else {
        format!("¥{out}")
    }
}

/// `2026-11-21` → `11-21`。完整年份对「下一个决策日」是噪音。
fn short_date(iso: &str) -> &str {
    iso.get(5..).unwrap_or(iso)
}

/// 快照不是今天：提醒先更新，而不是拿旧数字冒充当天结论。
fn stale(facts: &AccountFacts) -> bool {
    facts
        .data_date
        .as_deref()
        .is_some_and(|date| date != Local::now().date_naive().to_string())
}

/// 主动开口的文案本体（已由 `SpeakCandidate` 校验过）。留一个入口，方便
/// 未来把 ACP 改写结果与模板做对比审计。
pub fn notice_line(draft: &str) -> &str {
    draft
}

/// 采纳外部表达层（ACP）改写过的通知文案。
///
/// 表达层只允许改措辞，不允许改事实：任何数字都必须与模板一致，且不能
/// 引入模板里没有的数字。过不了校验就退回模板——宁可句子笨一点，也不能
/// 让管家说出账本里没有的数。
pub fn adopt_notice(draft: &str, rewritten: &str) -> String {
    // 通知走完整的 banner 口径（长度 + 行数 + 禁用话术）。不能只查
    // `notification_violation`：禁词检查藏在 `validate_notification` 里，
    // 少查一层 banner 就会跟着会话一起松绑（有过一次回归，测试拦住了）。
    if schedule::validate_notification(rewritten).is_err() {
        return draft.to_owned();
    }
    adopt(draft, rewritten)
}

/// 同上，用于会话回复（300 字上限）。
pub fn adopt_dialogue(draft: &str, rewritten: &str) -> String {
    adopt(draft, rewritten)
}

fn adopt(draft: &str, rewritten: &str) -> String {
    if schedule::validate_dialogue(rewritten).is_err() {
        return draft.to_owned();
    }
    if numbers(draft) != numbers(rewritten) {
        return draft.to_owned();
    }
    rewritten.trim().to_owned()
}

/// 把文本里的数字抽成有序列表。只比较「出现了哪些数」，不比较位置：
/// 「偏离 6.2pp」写成「6.2pp 的偏离」是措辞变化，不是事实变化。
fn numbers(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() || (ch == '.' && !current.is_empty()) {
            current.push(ch);
        } else if !current.is_empty() {
            found.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        found.push(current);
    }
    found.sort();
    found
}

/// 供界面显示的一句话画像：当前最需要看的一条偏离，没有则说明带宽内。
pub fn headline(facts: &AccountFacts) -> String {
    match facts
        .widest_drift()
        .filter(|row| row.excess_pp(facts.band_pp) > 0.0)
    {
        Some(row) => format!(
            "{}偏离 {:.1}pp（带宽 ±{:.1}pp）",
            row.name,
            row.drift_pp.abs(),
            facts.band_pp
        ),
        None => "各腿都在目标带宽内".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::butler::orchestrator::DriftRow;

    fn row(name: &str, current: f64, target: f64) -> DriftRow {
        DriftRow {
            key: name.to_owned(),
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
            drift: vec![row("美股", 41.2, 35.0), row("A股", 30.0, 32.0)],
            ..AccountFacts::default()
        }
    }

    /// 一张有总资产、有计划、有到期承诺的完整账本。
    fn rich_facts() -> AccountFacts {
        AccountFacts {
            decision_day: false,
            band_pp: 4.0,
            drift: vec![row("美股", 41.2, 35.0), row("A股", 30.0, 32.0)],
            total_assets: 128_400.0,
            holdings_value: 106_200.0,
            cash: 22_200.0,
            positions_count: 6,
            plan_lines: vec![
                crate::butler::orchestrator::PlanLine {
                    name: "A股".to_owned(),
                    amount: 2_000.0,
                },
                crate::butler::orchestrator::PlanLine {
                    name: "美股".to_owned(),
                    amount: 1_500.0,
                },
            ],
            next_decision_day: Some("2026-11-21".to_owned()),
            data_date: Some(Local::now().date_naive().to_string()),
            ..AccountFacts::default()
        }
    }

    #[test]
    fn 问偏离时复述同源数字并点名超带宽的腿() {
        let reply = reply("现在偏离多少", &facts());
        assert!(reply.contains("美股 41.2%"));
        assert!(reply.contains("6.2pp"));
        assert!(schedule::validate_dialogue(&reply).is_ok(), "实际 {reply}");
    }

    #[test]
    fn 问要不要动时不装能下单也不编观点() {
        let reply = reply("今天要不要加仓", &facts());
        // 兜底模板没有观点逻辑，也不谎称能执行：把账面递上去，决定权交还。
        assert!(reply.contains("怎么处理你定"));
        assert!(reply.contains("下单我帮不了"));
        assert!(schedule::validate_dialogue(&reply).is_ok(), "实际 {reply}");
    }

    #[test]
    fn 空账户如实说没有数字() {
        let reply = reply("现在配置怎么样", &AccountFacts::default());
        assert!(reply.contains("还没有持仓"));
    }

    #[test]
    fn 非决策日不催人动手() {
        let mut facts = facts();
        facts.decision_day = false;
        let reply = reply("要不要操作", &facts);
        assert!(reply.contains("不是决策日"));
        assert!(schedule::validate_dialogue(&reply).is_ok());
    }

    #[test]
    fn 兜底文案本身合规() {
        assert!(schedule::validate_dialogue(&fallback(&facts())).is_ok());
        assert!(schedule::validate_dialogue(&headline(&facts())).is_ok());
    }

    #[test]
    fn 问能做什么时答能力与诚实底线而不是反问() {
        let reply = reply("你都能做什么", &rich_facts());
        assert!(reply.contains("偏离"), "应列出能核的事，实际 {reply}");
        assert!(reply.contains("我的看法"), "自主人格要声明观点归属，实际 {reply}");
        assert!(reply.contains("下单我帮不了"), "不谎称能执行，实际 {reply}");
        assert!(!reply.contains("你想核对哪一段"), "不能退回开场白");
        assert_eq!(classify("你都能做什么"), Intent::Capability);
    }

    #[test]
    fn 问持仓时念出总资产与结构() {
        let reply = reply("分析我的持仓", &rich_facts());
        assert!(reply.contains("¥128,400"), "应念总资产，实际 {reply}");
        assert!(reply.contains("¥22,200"), "应念现金，实际 {reply}");
        assert!(reply.contains("6.2pp"), "应点名超带宽的腿，实际 {reply}");
        assert!(schedule::validate_dialogue(&reply).is_ok());
    }

    #[test]
    fn 问今天有什么时先声明不编外部新闻() {
        let reply = reply("今天都有什么大事发生", &rich_facts());
        assert!(reply.contains("不知道"), "必须承认没有新闻源，实际 {reply}");
        assert!(reply.contains("不编"));
        assert!(reply.contains("账本这边"), "要给出账本侧的事实");
    }

    #[test]
    fn 复合问题主意图答完再补一句今日要点() {
        let reply = reply(
            "分析我的持仓，给出未来操作建议，还有今天都有什么大事发生",
            &rich_facts(),
        );
        assert!(
            reply.contains("¥128,400"),
            "主意图应是持仓分析，实际 {reply}"
        );
        assert!(reply.contains("账本这边"), "还应补今日要点，实际 {reply}");
        assert!(schedule::validate_dialogue(&reply).is_ok());
    }

    #[test]
    fn 问涨跌时只答有把握的部分而不是硬拒() {
        let reply = reply("明天美股会涨吗", &rich_facts());
        assert_eq!(classify("明天美股会涨吗"), Intent::Forecast);
        assert!(reply.contains("说不准"), "模板没有市场数据，要如实说不准");
        assert!(reply.contains("偏离"), "说完没把握的，递上有把握的，实际 {reply}");
        assert!(schedule::validate_dialogue(&reply).is_ok());
    }

    #[test]
    fn 没接住的问题带状态而不是同一句反问() {
        let answer = reply("1", &rich_facts());
        assert_eq!(classify("1"), Intent::Unknown);
        assert!(answer.contains("没接住"));
        assert!(answer.contains("账本这边"), "要让用户知道现在的状态");
        // 不同账本状态下兜底句不应完全相同——固定句就是复读机。
        let other = reply("1", &facts());
        assert_ne!(answer, other, "兜底句应随账本状态变化");
    }

    #[test]
    fn 问计划时念逐腿金额() {
        let reply = reply("本月计划要买多少", &rich_facts());
        assert_eq!(classify("本月计划要买多少"), Intent::Plan);
        assert!(reply.contains("A股 买 ¥2,000"), "实际 {reply}");
        assert!(reply.contains("美股 买 ¥1,500"), "实际 {reply}");
        assert!(schedule::validate_dialogue(&reply).is_ok());
    }

    #[test]
    fn 问决策日时给出下一个日期() {
        let reply = reply("下次哪天决策", &rich_facts());
        assert!(reply.contains("11-21"), "实际 {reply}");
    }

    #[test]
    fn 追问沿用上一句意图而不是掉兜底() {
        let facts = rich_facts();
        assert_eq!(classify("那A股呢"), Intent::Unknown, "单看这句没有意图词");
        let follow_up = reply_with_context("那A股呢", Some("分析我的持仓"), &facts);
        assert!(
            follow_up.contains("¥128,400"),
            "应沿用持仓意图，实际 {follow_up}"
        );
        assert!(schedule::validate_dialogue(&follow_up).is_ok());
    }

    #[test]
    fn 纯数字不被当成追问硬套上一句() {
        let facts = rich_facts();
        let answer = reply_with_context("1", Some("分析我的持仓"), &facts);
        assert!(answer.contains("没接住"), "实际 {answer}");
    }

    #[test]
    fn 给定意图时跳过关键词直接生成() {
        let facts = rich_facts();
        // 这句话本身没有「持仓」二字，关键词会落到 Unknown；给定意图后
        // 应生成持仓分析。
        let answer = reply_with_intent(Intent::Holdings, "它怎么样", &facts);
        assert!(answer.contains("¥128,400"), "实际 {answer}");
        assert!(schedule::validate_dialogue(&answer).is_ok());
    }

    #[test]
    fn 千分位按三位一组() {
        assert_eq!(money(0.0), "¥0");
        assert_eq!(money(999.4), "¥999");
        assert_eq!(money(1_000.0), "¥1,000");
        assert_eq!(money(1_234_567.8), "¥1,234,568");
        assert_eq!(money(-2_000.0), "-¥2,000");
    }

    #[test]
    fn 旧版事实文件缺少新字段时按零读而不是报错() {
        let old = r#"{"decision_day":true,"band_pp":4.0,"drift":[],"new_cash_today":0.0,
            "plan_ready":false,"heavy_page":null,"market_move_pct":null,"promises_due":[]}"#;
        let facts: AccountFacts = serde_json::from_str(old).expect("旧文件必须还能读");
        assert_eq!(facts.total_assets, 0.0);
        assert!(facts.plan_lines.is_empty());
        // 总资产为 0 时不能念「总资产 ¥0」。
        let reply = reply("分析持仓", &facts);
        assert!(!reply.contains("¥0"), "实际 {reply}");
    }

    #[test]
    fn 只改措辞的改写会被采纳() {
        let draft = "逆周期偏离 18.0pp，超出 ±4.0pp 带宽。要不要处理由你定；我不替你下单。";
        let rewritten = "逆周期这条腿比目标低了 18.0pp（带宽 ±4.0pp）。动不动你定，我不下单。";
        assert_eq!(
            adopt_notice(draft, rewritten),
            rewritten,
            "数字一致即可采纳"
        );
    }

    #[test]
    fn 改写引入或修改数字一律退回模板() {
        let draft = "逆周期偏离 18.0pp，超出 ±4.0pp 带宽。要不要处理由你定；我不替你下单。";
        let invented = "逆周期偏离 18.0pp，带宽 ±4.0pp，建议补 3 万。";
        assert_eq!(
            adopt_notice(draft, invented),
            draft,
            "出现了模板没有的数字，说明表达层在编事实"
        );
        let changed = "逆周期偏离 12.0pp，超出 ±4.0pp 带宽。";
        assert_eq!(adopt_notice(draft, changed), draft);
    }

    #[test]
    fn 改写越界或超长也退回模板() {
        let draft = "逆周期偏离 18.0pp，超出 ±4.0pp 带宽。要不要处理由你定；我不替你下单。";
        assert_eq!(
            adopt_notice(draft, "偏离 18.0pp，±4.0pp，建议买入补仓。"),
            draft
        );
        let long = format!("{draft}{draft}");
        assert_eq!(adopt_notice(draft, &long), draft, "通知超 60 字必须退回");
        assert_eq!(adopt_dialogue(draft, ""), draft, "空改写等于没改");
    }
}
