//! 管家对话的 ACP 适配层。
//!
//! MNS 不持有推理端点，也不保存 API key：对话全部交给外部 ACP agent，
//! provider、模型与凭据留在 agent 自己的配置里（见 `agent` 模块开头的
//! 设计边界）。这一层只负责三件事：
//!
//! 1. 把账本压成一段可核对的上下文，连同人设写进首轮 goal；
//! 2. 把后续提问包成续问（ACP session 自己保留上下文）；
//! 3. 收拾 agent 的自由文本——保留 markdown、限长度、清掉机器残留、拒空答。
//!
//! 管家是自主人格（`docs/butler-persona-upgrade-2026-11-17.md`）：策略性
//! 话术（买卖点、择时倾向）**不再**被校验层否决，观点约束由人设里的
//! 「自我标注」承担；这里只守诚实性底线——数字同源（提示词要求 + 数字
//! 白名单在 `expression::adopt_*`）、不谎称已下单、不空答。这一点必须
//! 写进帮助文档。

use std::path::Path;

use chrono::Local;

use super::expression::money;
use super::orchestrator::AccountFacts;
use super::schedule;

/// ACP 回答的长度上限。会话是管家的主场，可以聊开一点，但超过这个数
/// 就不是「聊」而是贴报告了。
pub const MAX_TALK_CHARS: usize = 2000;

/// 首轮 goal：人设 + 账本事实 + 可查命令 + 说话方式 + 两个短示例。
///
/// ACP 的 `session/new` 没有 system prompt 注入位（`NewSessionRequest` 里
/// 那个字段是 UNSTABLE，且要求 agent 声明 `sessionCapabilities.systemPrompt`；
/// opencode 2.2.5 没有声明），所以人设只能写进第一条消息。后续提问复用
/// 同一个 session，不必重复。
///
/// 人设要点：像个真人管家在聊天，先结论后理由、默认短；观点是我的、可以
/// 直说（自主人格），账本数字是事实、只照抄（数据诚信），下单能力不存在、
/// 不谎称（能力诚实）。few-shot 比形容词管用：两个短例子直接示范长度与
/// 语气，避免模型把「自由发挥」理解成长篇报告。
pub fn opening_goal(question: &str, facts: &AccountFacts, cli: Option<&Path>) -> String {
    let anchored = format!("{question}\n\n{}", length_note(question));
    compose_goal("【用户的问题】", &anchored, facts, cli, "")
}

/// 主动开口的首轮 goal。用户没有提问，所以问题位换成一条「你想对他说什么」
/// 的任务说明，并把记忆与最近对话补进上下文。
///
/// 输出要过通知 banner 的口径（≤60 字、不带买卖点话术），所以这里的指令
/// 必须写死「一句话、只报事实或提醒」，而不是让 agent 自己发挥成长段落。
pub fn agent_init_goal(
    facts: &AccountFacts,
    cli: Option<&Path>,
    memory: &[String],
    recent: &[String],
) -> String {
    let mut context = String::new();
    if !memory.is_empty() {
        context.push_str("【你记得的事】\n");
        for line in memory {
            context.push_str(&format!("- {line}\n"));
        }
        context.push('\n');
    }
    if !recent.is_empty() {
        context.push_str("【最近的对话】\n");
        for line in recent {
            context.push_str(&format!("- {line}\n"));
        }
        context.push('\n');
    }
    compose_goal("【这次的任务】", AGENT_INIT_QUESTION, facts, cli, &context)
}

/// 主动开口的任务说明。它替代「用户的问题」那一节。
const AGENT_INIT_QUESTION: &str = "\
（这一次用户没有提问，是你主动开口。）挑一件此刻真正值得他知道的事，\
对他说一句话。这一句会作为系统通知横幅出现：只写这一句，不超过 55 字，\
不分行、不寒暄、不罗列、不要用 markdown；别照着上面的聊天示例展开，只要一句。\
只报事实或一句提醒，不给买卖建议、不预测涨跌、不承诺收益。\
如果确实没有值得说的事，只回「无」。";

fn compose_goal(
    label: &str,
    question: &str,
    facts: &AccountFacts,
    cli: Option<&Path>,
    context: &str,
) -> String {
    let facts_text = facts_block(facts);
    let tools = tools_block(cli);
    format!(
        "你是用户的私人投资管家，一个像真人一样说话的自主智能体。这本账是你的地盘：\
         市场、计划、持仓、和钱有关的闲聊都可以谈，也可以有自己的观点和倾向。观点是你的，\
         直说并标明「我的看法」；账本数字是事实，只照抄下面的清单或命令输出。\n\n\
         【账本事实】\n\
         {facts_text}\n\n\
         {tools}\n\n\
         {context}\
         【怎么说】\n\
         - 先给结论，再给一两句理由；像面对面聊天，不像写报告。\n\
         - 默认 2–4 个短段落，全文约 300 字以内。只有用户明确说「展开」「细说」「详细讲讲」\
         才展开；没被要求就别写长。\n\
         - 一次最多点 2–3 个关键数字，别把所有腿、所有指标都念一遍；其余用一句话带过。\n\
         - 可以用简单的 markdown：**把关键数字加粗**、短列表、少量小标题；\
         不要表格、不要代码块、不要 JSON。\n\
         - 用中文回答。不要写「我先查一下」「接下来我会…」「已完成」「涉及命令」这类过程旁白，\
         也不要输出 mns-decision、action、effective 之类的内部标记。\n\
         - 给观点就自我标注；预测说不准就直说不准——可以给倾向和理由，不装确定。\n\
         - 不谎称已经下单或改了账：下单你做不了，动账的只有用户本人。\n\
         - 数字照抄上面的清单或命令输出，不要心算、不要四舍五入、不要凭记忆补。\n\
         - 数据不是今天的时候，先用一句话提醒先更新，再回答。\n\
         - 上面的事实够答就直接答，不要为了确认再跑一遍命令；只有确实缺数据时才查。\n\n\
         【示例】\n\
         用户：明天 A 股怎么看？\n\
         管家：我的看法是继续震荡，方向说不准。**这不影响你的计划**——按带宽走就行，\
         不用因为明天涨跌改主意。\n\n\
         用户：我该不该把黄金卖了？\n\
         管家：我的看法是别急着卖。黄金在你的组合里是对冲腿，不是收益来源，跌了不等于该砍。\
         真要动，等决策日看它有没有出带宽；现在卖只是被短期波动牵着走。\n\n\
         {label}\n\
         {question}"
    )
}

/// 续问。session 已经带着上一轮的事实与风格，重复一遍只会挤掉注意力。
pub fn follow_up(question: &str) -> String {
    format!(
        "{question}\n\n（继续上面的对话，沿用同一份账本事实与同一种说话方式。）\n{}",
        length_note(question)
    )
}

/// 贴着问题的长度提醒。
///
/// 首轮人设里已经写了「默认 300 字、要展开才展开」，但长会话里那条指令
/// 会被越推越远，模型容易回到写报告的习惯。把提醒放在问题后面最近的位置
/// 更有效——不是截断，只是提醒；用户说「展开」时同一条提醒改口，避免
/// 提示词和一个明确的「讲详细点」相互打架。
fn length_note(question: &str) -> String {
    if wants_detail(question) {
        "（用户要求展开：可以写得长一些，但仍先给结论，按要点分短段，不要凑字数。）".to_owned()
    } else {
        "（默认简短：先给结论，2–4 个短段，全文约 300 字以内；不要罗列所有指标。）".to_owned()
    }
}

/// 用户是否明确要求写长。只认显式的展开词，避免「详细仓位」这类名词误判。
fn wants_detail(question: &str) -> bool {
    const CUES: [&str; 7] = ["展开", "细说", "详细讲", "详细说", "多说", "讲讲", "具体说"];
    let text = question.trim();
    CUES.iter().any(|cue| text.contains(cue))
}

/// 校验 agent 的回答，返回可直接落盘的文本（markdown 保留）。
///
/// 只守诚实性底线：非空、限长、不谎称能替用户下单。观点不查——管家人格
/// 允许倾向性表述，策略性禁词表只留给通知 banner 路径（见模块注释）。
pub fn validate_talk(text: &str) -> Result<String, String> {
    let cleaned = normalize(text);
    if cleaned.is_empty() {
        return Err("agent 没有给出内容".to_owned());
    }
    let chars = cleaned.chars().count();
    if chars > MAX_TALK_CHARS {
        return Err(format!("回答 {chars} 字，超过 {MAX_TALK_CHARS} 字上限"));
    }
    if let Some(phrase) = schedule::claims_execution(&cleaned) {
        return Err(format!("谎称能替用户执行下单（「{phrase}」）"));
    }
    Ok(cleaned)
}

/// 通知横幅用的单行纯文本。
///
/// 主动开口会同时进会话与系统通知：会话用 markdown 渲染，横幅不会。这里
/// 去掉强调符与标题标记并压成一行，避免用户在自己的锁屏上看到 `**`。
pub fn notification_text(text: &str) -> String {
    let normalized = normalize(text);
    normalized
        .lines()
        .map(|line| {
            let line = line.trim().trim_start_matches('#').trim_start();
            line.replace("**", "").replace("__", "").replace('`', "")
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// 渲染前清理：会话气泡显示的内容统一过一遍 `normalize`。
///
/// 落盘前新消息已经过 `validate_talk`，但升级前存下的旧回答不会回洗：第 10
/// 节实测的那条 1633 字消息，尾部仍有 `action:` / `effective:` 四行；
/// human 截图里更早的一条用的是 `<mns-decision>…</mns-decision>`。删改历史
/// 存储不值当，显示层每次都清一遍即可；`normalize` 幂等，新消息过一遍没有
/// 副作用。用户气泡不过这里——用户自己打的字不能改。
pub fn display_text(text: &str) -> String {
    normalize(text)
}

/// 校验前的整理：保留 markdown，只做空白规整与防御性清理。
///
/// 为什么不再剥 markdown：管家窗口现在用 `TextView::markdown` 渲染，
/// `**加粗**` 是排版而不是噪音。这里只处理两类「机器残留」：
///
/// 1. mns-decision 决策块（``` 围栏与 `<mns-decision>` 标签两种历史写法，
///    含流式未闭合的半截）——旧执行契约的机器通道；
/// 2. 行首的 ASCII `key:` 机器字段（`action:` / `effective:` /
///    `monthly_amount:` …）——实测模型会把执行契约模仿成正文尾巴。
///
/// 只认固定键名，中文「结论：」与英文普通句子不会被误伤。
fn normalize(text: &str) -> String {
    let without_decision = strip_decision_blocks(text);

    let mut lines: Vec<String> = Vec::new();
    for line in without_decision.lines() {
        let line = line.trim_end().to_owned();
        if is_machine_field(&line) {
            continue;
        }
        lines.push(line);
    }
    // 机器字段被拿走后，尾部可能留下连续空行，交给下面的折叠处理。

    let mut out: Vec<String> = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            if out.last().is_some_and(|last| !last.is_empty()) {
                out.push(String::new());
            }
            continue;
        }
        out.push(line);
    }
    while out.last().is_some_and(|line| line.is_empty()) {
        out.pop();
    }
    out.join("\n").trim().to_owned()
}

/// 正文里的机器字段：只匹配固定的 ASCII 键，避免误删正常句子。
fn is_machine_field(line: &str) -> bool {
    const KEYS: [&str; 9] = [
        "action",
        "actions",
        "effective",
        "monthly_amount",
        "monthly",
        "allocation",
        "allocations",
        "next_action",
        "executed_action",
    ];
    let trimmed = line.trim_start();
    let Some((key, _)) = trimmed.split_once(':') else {
        return false;
    };
    KEYS.contains(&key.trim())
}

/// 去掉 mns-decision 决策块，两种历史写法都认：
///
/// - ```` ```mns-decision ```` 围栏，闭合符是下一处 ```` ``` ````；
/// - `<mns-decision>…</mns-decision>`（更早的会话用尖括号，human 截图里
///   就是这一种）。
///
/// 流式输出里未闭合的半截连同后续文本一起丢掉：宁可少显示，也不能把机器
/// 字段留进气泡。
fn strip_decision_blocks(text: &str) -> String {
    const MARKERS: [(&str, &str); 2] =
        [("```mns-decision", "```"), ("<mns-decision>", "</mns-decision>")];

    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let found = MARKERS
            .iter()
            .filter_map(|(open, close)| rest.find(open).map(|at| (at, *open, *close)))
            .min_by_key(|(at, _, _)| *at);
        let Some((start, open, close)) = found else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after = &rest[start + open.len()..];
        match after.find(close) {
            Some(end) => {
                rest = after[end + close.len()..].trim_start_matches(['\r', '\n']);
            }
            None => rest = "",
        }
    }
    out
}

fn facts_block(facts: &AccountFacts) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("- 今天：{}", Local::now().format("%Y-%m-%d")));
    match &facts.data_date {
        Some(date) if date == &Local::now().format("%Y-%m-%d").to_string() => {
            lines.push(format!("- 账本数据日期：{date}（当天）"));
        }
        Some(date) => lines.push(format!("- 账本数据日期：{date}（**不是今天**）")),
        None => lines.push("- 账本数据日期：未知".to_owned()),
    }

    if facts.positions_count == 0 && facts.total_assets <= 0.0 {
        lines.push("- 账本还没有持仓，也没有入金记录".to_owned());
    } else {
        lines.push(format!(
            "- 总资产 {}：持仓 {}、现金 {}，共 {} 个标的",
            money(facts.total_assets),
            money(facts.holdings_value),
            money(facts.cash),
            facts.positions_count
        ));
    }

    if facts.decision_day {
        lines.push("- 今天是决策日".to_owned());
    } else if let Some(next) = &facts.next_decision_day {
        lines.push(format!("- 今天不是决策日，下一个决策日 {next}"));
    } else {
        lines.push("- 今天不是决策日".to_owned());
    }

    if !facts.drift.is_empty() {
        lines.push(format!(
            "- 各腿配置与目标（偏离带宽 ±{:.1}pp，超出会标出）：",
            facts.band_pp
        ));
        for row in facts.drift.iter().take(10) {
            let over = row.excess_pp(facts.band_pp) > 0.0;
            let sign = if row.drift_pp >= 0.0 { "+" } else { "" };
            lines.push(format!(
                "  · {} 当前 {:.1}% / 目标 {:.1}%（{sign}{:.1}pp{}）",
                row.name,
                row.current_pct,
                row.target_pct,
                row.drift_pp,
                if over { "，超出带宽" } else { "" }
            ));
        }
    }

    if facts.plan_lines.is_empty() {
        lines.push("- 本月计划：没有可执行的下单".to_owned());
    } else {
        let items: Vec<String> = facts
            .plan_lines
            .iter()
            .map(|line| {
                format!(
                    "{} {}{}",
                    line.name,
                    if line.amount >= 0.0 {
                        "买入 "
                    } else {
                        "卖出 "
                    },
                    money(line.amount.abs())
                )
            })
            .collect();
        lines.push(format!("- 本月计划：{}", items.join("；")));
    }
    if let Some(reason) = &facts.plan_blocked {
        lines.push(format!("- 计划被拦下：{reason}"));
    }

    lines.push(format!("- 今日入金：{}", money(facts.new_cash_today)));
    if !facts.promises_due.is_empty() {
        lines.push(format!(
            "- 今天到期的承诺：{}",
            facts.promises_due.join("；")
        ));
    }
    if let Some((page, views)) = &facts.heavy_page {
        lines.push(format!("- 今天已经看了{page} {views} 次"));
    }
    lines.join("\n")
}

fn tools_block(cli: Option<&Path>) -> String {
    match cli {
        Some(cli) => {
            let cli = cli.display();
            format!(
                "【可以查的只读命令】\n\
                 - `{cli} portfolio` 持仓、成本与市值\n\
                 - `{cli} cash` 现金与流水\n\
                 - `{cli} performance` 实际收益与回撤\n\
                 - `{cli} plan --monthly <金额>` 定投分配\n\
                 - `{cli} guard` 买入冷静期\n\
                 只跑这几个命令；不要读 ~/.mns 下的数据库或配置，也不要写任何文件。"
            )
        }
        None => {
            "【没有可用的命令行】只依据上面的账本事实回答；不够就直说不够，不要推测。".to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::butler::orchestrator::{DriftRow, PlanLine};

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

    fn rich() -> AccountFacts {
        AccountFacts {
            decision_day: false,
            band_pp: 4.0,
            drift: vec![row("美股", 41.2, 35.0), row("A股", 30.0, 32.0)],
            new_cash_today: 5000.0,
            plan_ready: true,
            heavy_page: None,
            promises_due: vec!["补仓".to_owned()],
            total_assets: 128_400.0,
            cash: 22_200.0,
            holdings_value: 106_200.0,
            positions_count: 6,
            plan_lines: vec![PlanLine {
                name: "沪深300".to_owned(),
                amount: 3200.0,
            }],
            plan_blocked: None,
            next_decision_day: Some("2026-11-20".to_owned()),
            data_date: Some(Local::now().format("%Y-%m-%d").to_string()),
        }
    }

    #[test]
    fn 首轮把人设事实与风格一起交出去() {
        let goal = opening_goal("现在偏离多少", &rich(), None);
        assert!(goal.contains("私人投资管家"), "人设要立自主人格");
        assert!(goal.contains("像真人一样说话"), "要的是聊天，不是报告");
        assert!(goal.contains("我的看法"), "观点要自我标注");
        assert!(goal.contains("先给结论"), "回答顺序是验收项");
        assert!(goal.contains("300 字以内"), "默认长度要写死");
        assert!(goal.contains("展开"), "只有明确要求才允许写长");
        assert!(goal.contains("【示例】"), "few-shot 比形容词有效");
        assert!(goal.contains("128,400"), "总资产要进上下文");
        assert!(goal.contains("美股 当前 41.2% / 目标 35.0%"));
        assert!(goal.contains("超出带宽"));
        assert!(goal.contains("不谎称已经下单"), "能力诚实是硬底线");
        assert!(goal.contains("照抄"), "数字同源是硬底线");
        assert!(goal.contains("现在偏离多少"));
        assert!(goal.contains("只依据上面的账本事实回答"));
    }

    #[test]
    fn 空账本不念零也不假装有持仓() {
        let goal = opening_goal("我有什么", &AccountFacts::default(), None);
        assert!(goal.contains("还没有持仓"));
        assert!(!goal.contains("总资产 ¥0"), "零不该被念成一个数字");
    }

    #[test]
    fn 有命令行时给出绝对路径() {
        let cli = std::path::Path::new("/usr/local/bin/mns");
        let goal = opening_goal("现金多少", &rich(), Some(cli));
        assert!(goal.contains("/usr/local/bin/mns portfolio"));
        assert!(goal.contains("不要写任何文件"));
    }

    #[test]
    fn 续问不重复整段事实() {
        let text = follow_up("那 A 股呢");
        assert!(text.contains("那 A 股呢"));
        assert!(!text.contains("总资产"), "续问不该再贴一遍事实");
        assert!(text.contains("约 300 字以内"), "续问也要重申默认长度");
    }

    #[test]
    fn 默认短而说展开才允许写长() {
        let short = opening_goal("现在偏离多少", &rich(), None);
        assert!(short.contains("默认简短"), "默认要贴一条短提醒");
        assert!(!short.contains("用户要求展开"));

        for cue in ["展开说说", "细说一下美股", "详细讲讲"] {
            let long = opening_goal(cue, &rich(), None);
            assert!(
                long.contains("用户要求展开"),
                "「{cue}」是明确的展开要求，提示词不该还在压长度"
            );
            assert!(!long.contains("默认简短"));
        }
    }

    #[test]
    fn 展开提醒不误伤普通名词() {
        // 细节 / 详细仓位 不是「要求写长」的指令，否则默认长度就被绕过。
        let goal = opening_goal("详细仓位比例是多少", &rich(), None);
        assert!(goal.contains("默认简短"));
    }

    #[test]
    fn 主动开口把记忆与最近对话带进上下文() {
        let goal = agent_init_goal(
            &rich(),
            None,
            &["周五补美股".to_owned()],
            &["用户：最近怎么样".to_owned()],
        );
        assert!(goal.contains("你主动开口"), "要说明这次不是用户提问");
        assert!(goal.contains("周五补美股"), "记忆要进上下文");
        assert!(goal.contains("用户：最近怎么样"), "最近对话要进上下文");
        assert!(goal.contains("不超过 55 字"), "必须约束成一句话");
        assert!(goal.contains("不要用 markdown"), "横幅是纯文本通道");
        assert!(goal.contains("只回「无」"), "没话说要允许沉默");
    }

    #[test]
    fn 策略性措辞不再整条被拦() {
        // 管家是自主人格：倾向性表述是它的职责，不是越界。会话校验只守
        // 诚实性底线（非空、限长、不谎称能下单）。
        let text = validate_talk("我的看法是现在可以加仓美股，目标价到 45% 也不奇怪。").unwrap();
        assert!(text.contains("我的看法"));
    }

    #[test]
    fn 谎称能下单仍被拦而诚实否定放行() {
        let err = validate_talk("已经帮你下单了，明天到账。").unwrap_err();
        assert!(err.contains("执行下单"), "实际 {err}");
        let ok = validate_talk("这个我没法帮你下单，下单得你自己来。").unwrap();
        assert!(ok.contains("没法帮你下单"));
    }

    #[test]
    fn 超长回答被拦下() {
        let long = "账".repeat(MAX_TALK_CHARS + 1);
        assert!(validate_talk(&long).is_err());
    }

    #[test]
    fn 正常回答保留markdown并规整空白() {
        let text = validate_talk("## 偏离\n\n**美股** 41.2%，超出 6.2pp。\n\n\n- 第一条\n- 第二条").unwrap();
        assert!(text.contains("**美股**"), "窗口用 markdown 渲染，不该剥掉强调");
        assert!(text.contains("## 偏离"), "小标题保留，渲染层负责排版");
        assert!(text.contains("- 第一条"));
        assert!(!text.contains("\n\n\n"), "连续空行要折叠");
    }

    #[test]
    fn 尾部的机器字段被清掉() {
        // 实测输出：正文之后跟着执行契约里的字段。它们会原样出现在气泡里，
        // 必须清掉；中文「结论：」不能被误伤。
        let raw = "\
结论：两腿出带宽。\n\
action: adjust_monthly_plan\n\
effective: 2026-11-20\n\
monthly_amount: 2000\n\
allocation: 美股 45%";
        let text = validate_talk(raw).unwrap();
        assert_eq!(text, "结论：两腿出带宽。");
    }

    #[test]
    fn 决策围栏被剔除包括未闭合的半截() {
        let closed = validate_talk("今天不动。\n\n```mns-decision\n{\"a\":1}\n```\n").unwrap();
        assert_eq!(closed, "今天不动。");
        let streamed = validate_talk("今天不动。\n\n```mns-decision\n{\"a\":").unwrap();
        assert_eq!(streamed, "今天不动。");
    }

    #[test]
    fn 尖括号决策块也被剔除() {
        // 更早的会话写法（human 截图那条）：正文之后跟 XML 风格的块。
        let raw = "\
红利低波不卖。\n\n\
<mns-decision>\n\
结论: 11-20 维持原计划\n\
倾向: 欠配的两条股票腿该补\n\
待你决定: 两只纳指是否合并\n\
</mns-decision>";
        let text = validate_talk(raw).unwrap();
        assert_eq!(text, "红利低波不卖。");
        assert!(!text.contains("结论"));

        // 流式未闭合：块后的内容也一起丢掉。
        let streamed = validate_talk("就这些。\n\n<mns-decision>\n结论: 待定").unwrap();
        assert_eq!(streamed, "就这些。");
    }

    #[test]
    fn 正文里提到action不被当成机器字段() {
        let text = validate_talk("你说的 action 字段我不写了。").unwrap();
        assert!(text.contains("action 字段"), "整行不是 key:value 就保留");
    }

    #[test]
    fn 通知文本去掉markdown并压成一行() {
        let text = notification_text("**美股** 偏离 6.2pp。\n- 今天不是决策日。");
        assert_eq!(text, "美股 偏离 6.2pp。 - 今天不是决策日。");
        assert!(!text.contains("**"));
        assert!(!text.contains('\n'));
    }

    #[test]
    fn 显示层清理旧消息且对新消息幂等() {
        // 第 10 节实测的旧回答：正文尾巴仍带执行契约字段。
        let legacy = "\
结论：两腿出带宽。\n\n\
action: adjust_monthly_plan\n\
effective: 2026-11-20\n\
monthly_amount: 2000\n\
allocation: 美股 45%";
        let shown = display_text(legacy);
        assert_eq!(shown, "结论：两腿出带宽。");

        // human 截图里的旧消息：XML 风格决策块同样在显示层清掉。
        let xml = "红利低波不卖。\n\n<mns-decision>\n结论: 维持原计划\n</mns-decision>";
        assert_eq!(display_text(xml), "红利低波不卖。");

        // 新消息已经过 validate_talk，再过一次不该有任何变化。
        let fresh = validate_talk("**美股** 偏离 6.2pp。\n\n- 等决策日再看").unwrap();
        assert_eq!(display_text(&fresh), fresh);
        assert_eq!(display_text(&display_text(&fresh)), fresh, "必须是幂等的");
    }

    #[test]
    fn 空输出被拦下() {
        assert!(validate_talk("   \n  ").is_err());
    }
}
