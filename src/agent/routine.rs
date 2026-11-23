//! Prompt assembly for scheduled and guided runs.
//!
//! The split of duties is the point: MNS fetches data and states the question,
//! the agent only judges. Keeping the template here — not in the GUI — means
//! the desktop shell, the CLI and any future front end share one contract.

use std::path::Path;

/// Schema handed to the agent. Field names are part of the contract: the shell
/// renders them by name, so renaming one silently degrades the card.
const DECISION_SCHEMA: &str = r#"{
  "as_of": "YYYY-MM-DD",
  "decision_day": true,
  "verdict": "hold | adjust | wait",
  "headline": "一句话结论，不超过 40 字",
  "positions": [
    { "code": "007605", "name": "嘉实沪深300红利低波", "stance": "hold | trim | add", "note": "依据报告里的偏离或收益" }
  ],
  "risks": [
    { "level": "low | medium | high", "text": "今天需要留意的环境风险" }
  ],
  "actions": [
    { "code": "007605", "side": "buy | sell", "amount_cny": 0, "reason": "必须来自报告的偏离或现金流事件" }
  ],
  "questions": ["0–2 条：只有你知道、且答案会改变今天结论的事实缺口；没有就给 []"]
}"#;

/// Cap on how much report text goes into the prompt. A full report is a few
/// thousand characters; anything beyond this is duplicated detail that only
/// burns context and pushes the schema out of the model's attention.
const MAX_REPORT_CHARS: usize = 12_000;

/// Daily review prompt: holdings from the last close, environment today.
pub fn daily_review_prompt(date: &str, weekday: &str, report: &str, cli: Option<&Path>) -> String {
    let report = trim_report(report);
    let cli_note = match cli {
        Some(cli) => {
            let cli = cli.display();
            format!(
                "需要补充数据时，用这些只读命令：\n\
                 - `{cli} portfolio` 持仓与成本\n\
                 - `{cli} cash` 现金与流水\n\
                 - `{cli} performance` 实际 XIRR 与回撤\n\
                 - `{cli} guard` 买入冷静期\n\
                 - `{cli} plan --monthly <金额>` 定投分配\n\
                 - `{cli} --help` 全部命令\n\
                 不要用 find / ls 搜索 mns，也不要直接读 ~/.mns 下的数据库或配置。"
            )
        }
        None => {
            "本次未提供 mns CLI，只能依据下面的报告判断；数据不足时直接说明，不要推测。".to_owned()
        }
    };

    format!(
        "你是我的投资纪律助手。今天是 {date}（{weekday}）。\n\n\
         下面这段输出由 mns CLI 刚刚生成，是本次判断的唯一数据来源：\n\
         <mns-report>\n{report}\n</mns-report>\n\n\
         请完成三件事：\n\
         1. 昨日持仓表现：哪些标的偏离在扩大、哪些回到带宽内，现金是否够用。\n\
         2. 今日市场环境风险：情绪刻度处在哪个区间、数据是否新鲜、限购与费率等执行条件是否需要重新核验。\
         联网只用于核验能否按计划执行，不得用来判断涨跌或改写目标仓位。\n\
         3. 今天是否操作：按报告的目标仓位与偏离决定。不满足条件时明确写「今天不操作」，不要为了有产出而制造交易。\n\n\
         {cli_note}\n\n\
         【输出契约】\n\
         回答的最后必须输出一个 ```mns-decision 代码块，内容是下面这个 JSON（字段名固定，缺失也要保留键）：\n\
         {DECISION_SCHEMA}\n\n\
         约束：\n\
         - 金额、份额、日期一律照抄报告，不重算、不四舍五入、不凭记忆填写。\n\
         - verdict 只能是 hold / adjust / wait；没有触发条件就填 hold，并在 headline 写明「今天不操作」。\n\
         - questions 是例外通道，不是待办清单：最多 2 条，能不问就不问。\n\
         只问「只有用户本人知道、且答案会改变今天 verdict 或 actions」的封闭事实\
         （是否已成交 / 已入金、某笔限购是否解除、资金何时到账），\
         每条都必须能用一个「是/否、一个数字或一个日期」回答。\n\
         - 不要问观点、偏好、预测或复盘理由，也不要问报告里已经写明的事实；\
         想展开讨论的不要塞进 questions——用户会带这份巡检去管家窗口追问。\n\
         - 不确定但会影响结论的事实才写进 questions；不影响结论的不确定，在正文说明即可，不要占用 questions。\n\
         - 正文里不要向用户提问，也不要说「请回复 / 请告诉我」：巡检页没有对话入口。\n\
         需要用户确认的只放进 questions；展开讨论由用户自己去管家窗口发起。\n\
         - 工作目录可写，但不要写任何文件：结论只通过 mns-decision 块回传，MNS 负责保存与展示。"
    )
}

/// Follow-up prompt. The ACP session already carries the plan, the tool history
/// and the constraints from the first turn, so re-stating them would waste
/// context and invite the agent to re-plan from scratch.
pub fn followup_prompt(question: &str) -> String {
    format!(
        "{question}\n\n\
         （继续当前任务。沿用上一轮的数据与约束；如果结论发生变化，重新输出一个 ```mns-decision 块，\
         没有变化就不用重复。）"
    )
}

fn trim_report(report: &str) -> String {
    let report = report.trim();
    if report.chars().count() <= MAX_REPORT_CHARS {
        return report.to_owned();
    }
    let head: String = report.chars().take(MAX_REPORT_CHARS).collect();
    format!("{head}\n…（报告过长，已截断）")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_review_carries_data_cli_and_the_output_contract() {
        let prompt = daily_review_prompt(
            "2026-11-09",
            "周一",
            "现金余额 ¥2919.40",
            Some(Path::new("/Applications/MNS.app/Contents/MacOS/mns")),
        );
        assert!(prompt.contains("2026-11-09"));
        assert!(
            prompt.contains("现金余额 ¥2919.40"),
            "the report is the data source"
        );
        assert!(prompt.contains("/Applications/MNS.app/Contents/MacOS/mns"));
        assert!(
            prompt.contains("mns-decision"),
            "the schema block must be demanded"
        );
        assert!(prompt.contains("\"verdict\""));
        assert!(prompt.contains("不要写任何文件"));
    }

    #[test]
    fn questions_are_a_rare_exception_channel_not_a_chat_entry() {
        let prompt = daily_review_prompt("2026-11-09", "周一", "报告", None);
        assert!(
            prompt.contains("最多 2 条"),
            "the cap must be stated, otherwise a model fills it"
        );
        assert!(
            prompt.contains("能不问就不问"),
            "zero questions must be the preferred default"
        );
        assert!(
            prompt.contains("管家窗口"),
            "depth belongs to the butler, not the review card"
        );
        assert!(
            prompt.contains("不要问观点、偏好、预测"),
            "open-ended advice questions must be excluded"
        );
        assert!(
            prompt.contains("报告里已经写明的事实"),
            "facts the report already answers are not questions"
        );
        assert!(
            !prompt.contains("不确定的事实写进 questions"),
            "the old blanket rule invited a list of every uncertainty"
        );
        assert!(
            prompt.contains("巡检页没有对话入口"),
            "prose must not ask the user to answer in a page that cannot reply"
        );
    }

    #[test]
    fn missing_cli_degrades_to_report_only() {
        let prompt = daily_review_prompt("2026-11-09", "周一", "报告", None);
        assert!(prompt.contains("未提供 mns CLI"));
        assert!(!prompt.contains("`--help`"));
    }

    #[test]
    fn long_reports_are_trimmed() {
        let long = "行".repeat(MAX_REPORT_CHARS + 50);
        let prompt = daily_review_prompt("2026-11-09", "周一", &long, None);
        assert!(prompt.contains("已截断"));
        assert!(prompt.chars().count() < MAX_REPORT_CHARS + 2_000);
    }

    #[test]
    fn followup_keeps_the_session_contract_short() {
        let prompt = followup_prompt("那黄金这一腿为什么不动？");
        assert!(prompt.starts_with("那黄金这一腿为什么不动？"));
        assert!(prompt.contains("沿用上一轮"));
        assert!(prompt.contains("mns-decision"));
    }
}
