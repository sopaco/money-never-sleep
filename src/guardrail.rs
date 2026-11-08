//! 买入冷静期：情绪过热时的二次确认提示。
//!
//! 这个模块**不参与任何仓位或交易决策**。它只做三件事：提示当前处于过热区、
//! 建议延迟一个交易日、要求把买入理由记入流水供日后复盘。
//!
//! 为什么不直接否决买入（见 `docs/guardrail-experiment-2026-11-08.md`）：
//!   - "过热之后收益更差"在 2016-2026 三层数据上未通过任何显著性检验；
//!   - 让信号自动否决买入，在长期上行的三资产池上期望为负（13.1 万总投入
//!     下 -102 ~ -460 元），在长期横盘的沪深300 上为正，且对阈值非单调敏感；
//!   - 护栏的真实价值取决于"人本来会犯多大的错"，这是反事实，行情数据测不出。
//!
//! 因此把被证伪的预测能力，降级为不依赖预测能力的流程约束。

use crate::config::AppConfig;

/// 冷静期评估结果。`hot=false` 表示当前不处于过热区。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Advice {
    pub hot: bool,
    pub cooldown_days: u32,
    pub show_stats: bool,
}

/// 评估当前决策分数是否触发过热提示。配置关闭时返回 None。
pub fn evaluate(config: &AppConfig, effective_score: f64) -> Option<Advice> {
    if !config.guardrail.enabled {
        return None;
    }
    Some(Advice {
        hot: effective_score >= config.guardrail.hot_threshold,
        cooldown_days: config.guardrail.cooldown_days,
        show_stats: config.guardrail.show_stats,
    })
}

/// 生成提示文本。纯函数，便于回归测试与日报/plan 复用。
pub fn render(advice: Option<Advice>, effective_score: f64, hot_threshold: f64) -> String {
    let Some(a) = advice else {
        return String::new();
    };
    let mut s = String::new();
    if !a.hot {
        s.push_str(&format!(
            "  冷静期: 决策分数 {effective_score:.1} 低于过热阈值 {hot_threshold:.0}，按既定计划执行。\n"
        ));
        return s;
    }
    s.push_str("─────────────────────────────────────────────────────────────\n");
    s.push_str(&format!(
        "  ⚠ 买入冷静期 | 决策分数 {effective_score:.1} ≥ 过热阈值 {hot_threshold:.0}\n"
    ));
    s.push_str(&format!(
        "  建议延迟 {} 个交易日再执行；若属于计划外的临时追加买入，先写明理由。\n",
        a.cooldown_days
    ));
    s.push_str("  本工具不替你决定是否买入，也不会阻断交易。\n");
    if a.show_stats {
        s.push_str("  历史描述性统计（三资产 2016-2026，FGI≥75 后 60 交易日）：\n");
        s.push_str("    下跌概率 33.7% vs 全样本 25.9%，5% 左尾 -7.70%\n");
        s.push_str("    ⚠ 该差异未通过显著性检验（年块 bootstrap 95% CI 跨 0），不构成择时依据。\n");
        s.push_str("    ⚠ 自动否决买入在长期上行资产池上期望为负，故本功能只提示、不拦截。\n");
    }
    s.push_str("─────────────────────────────────────────────────────────────\n");
    s
}

/// 在 `mns plan` / `mns guard` 输出末尾打印提示。
pub fn print_advice(config: &AppConfig, effective_score: f64) {
    let text = render(
        evaluate(config, effective_score),
        effective_score,
        config.guardrail.hot_threshold,
    );
    if !text.is_empty() {
        println!();
        print!("{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AppConfig {
        AppConfig::default_config()
    }

    #[test]
    fn 过热阈值边界触发冷静期() {
        let c = cfg();
        let thr = c.guardrail.hot_threshold;
        assert!(!evaluate(&c, thr - 0.1).unwrap().hot);
        assert!(evaluate(&c, thr).unwrap().hot);
        assert!(evaluate(&c, 95.0).unwrap().hot);
    }

    #[test]
    fn 关闭后不产生任何提示() {
        let mut c = cfg();
        c.guardrail.enabled = false;
        assert_eq!(evaluate(&c, 99.0), None);
        assert!(render(None, 99.0, 75.0).is_empty());
    }

    #[test]
    fn 提示不得替用户做出买卖决定() {
        let c = cfg();
        let text = render(evaluate(&c, 90.0), 90.0, c.guardrail.hot_threshold);
        assert!(text.contains("不替你决定是否买入"), "必须声明不代做决策");
        assert!(text.contains("不会阻断交易"), "必须声明不拦截");
        assert!(text.contains("未通过显著性检验"), "必须披露统计不显著");
        assert!(!text.contains("建议不买"), "不得给出择时结论");
        assert!(!text.contains("禁止买入"), "不得给出阻断指令");
    }

    #[test]
    fn 非过热时只提示按计划执行() {
        let c = cfg();
        let text = render(evaluate(&c, 30.0), 30.0, c.guardrail.hot_threshold);
        assert!(text.contains("按既定计划执行"));
        assert!(!text.contains("延迟"));
    }

    #[test]
    fn 配置校验拒绝非法阈值() {
        let mut c = cfg();
        c.guardrail.hot_threshold = 120.0;
        assert!(c.validate().is_err());
        c.guardrail.hot_threshold = 75.0;
        c.guardrail.cooldown_days = 99;
        assert!(c.validate().is_err());
    }

    #[test]
    fn 参数读写映射可用() {
        let mut c = cfg();
        c.set_value("guardrail.hot_threshold", "80").unwrap();
        assert_eq!(c.guardrail.hot_threshold, 80.0);
        assert_eq!(c.get_value("guardrail.hot_threshold").as_deref(), Some("80"));
    }
}
