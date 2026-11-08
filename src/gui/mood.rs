//! Mood: the atmosphere the account is in right now.
//!
//! A dashboard that always wears the same blue tells the user nothing. The
//! palette here is derived from two facts the product already knows — where the
//! fear & greed index sits, and whether the book is up or down — and only ever
//! moves along a blue↔amber axis. Red and green stay reserved for money, so the
//! background can emote without competing with the numbers.

use crate::app::Snapshot;
use crate::gui::components::Tone;
use crate::gui::theme::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MoodKind {
    /// Fear: the plan is buying, the market is cheap.
    Opportunity,
    /// Middle of the range: nothing to prove.
    Balanced,
    /// Greed: the plan is trimming, the market is expensive.
    Caution,
}

#[derive(Clone, Copy)]
pub struct Mood {
    pub kind: MoodKind,
    /// Wash behind the whole page. Fades into `BG` before the first card row.
    pub tint: u32,
    /// Stronger wash for the hero panel.
    pub glow: u32,
    /// Decorative ink: arcs, auras, watermarks.
    pub ink: u32,
    /// Opaque pale version of `ink`, for pill backgrounds.
    ///
    /// Precomputed rather than derived from `Hsla` at the call site: converting
    /// a translucent colour back to a hex integer silently loses the alpha and
    /// produced the wrong hue.
    pub wash: u32,
    pub label: &'static str,
    /// One line that explains what the atmosphere means for the plan.
    pub note: &'static str,
}

impl Mood {
    fn of_kind(kind: MoodKind) -> Self {
        match kind {
            MoodKind::Opportunity => Self {
                kind,
                tint: 0xe9f1ff,
                glow: 0xd9e6ff,
                ink: 0x2f6fe0,
                wash: 0xe6eefd,
                label: "机会区间",
                note: "情绪偏冷，策略在分批买入；回撤容忍度可以高一些。",
            },
            MoodKind::Balanced => Self {
                kind,
                tint: 0xf1f5fc,
                glow: 0xe4ecfb,
                ink: ACCENT,
                wash: 0xeaf0fc,
                label: "中性区间",
                note: "情绪在常态区间，按月度计划执行，不需要额外动作。",
            },
            MoodKind::Caution => Self {
                kind,
                tint: 0xfdf3e4,
                glow: 0xfbe7c8,
                ink: 0xb4681a,
                wash: 0xfaeedb,
                label: "贪婪区间",
                note: "情绪偏热，策略在收缩仓位；新钱先停在现金里。",
            },
        }
    }

    /// The tone a metric tile should borrow so it does not fight the room.
    /// A blue watermark on an amber page reads like a leftover component.
    pub fn tile_tone(&self) -> Tone {
        match self.kind {
            MoodKind::Caution => Tone::Warning,
            _ => Tone::Accent,
        }
    }

    /// The book being underwater cools the atmosphere one step, because the
    /// user is more likely to need a calm screen than a cheerful one.
    pub fn resolve(data: Option<&Snapshot>) -> Self {
        let Some(data) = data else {
            return Self::of_kind(MoodKind::Balanced);
        };
        let score = data.signal.as_ref().map(|signal| signal.effective_score);
        let kind = match score {
            Some(score) if score <= 42.0 => MoodKind::Opportunity,
            Some(score) if score >= 62.0 => MoodKind::Caution,
            _ => MoodKind::Balanced,
        };
        let mut mood = Self::of_kind(kind);
        if data.holding_pnl() < -0.0001 && mood.kind == MoodKind::Caution {
            // Greed plus a losing book reads as a warning, not an invitation.
            mood.tint = 0xf6eef7;
            mood.glow = 0xf0e2f2;
            mood.ink = 0x8a4b8f;
            mood.wash = 0xf3e7f4;
            mood.label = "谨慎区间";
            mood.note = "情绪偏热且账户浮亏，先按兵不动，等下一次决策日。";
        }
        mood
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The palette must never borrow gain/loss colours: red and green belong to
    /// money, and a mood that looked like a profit would misread the account.
    #[test]
    fn mood_inks_stay_off_the_gain_loss_axis() {
        for kind in [MoodKind::Opportunity, MoodKind::Balanced, MoodKind::Caution] {
            let mood = Mood::of_kind(kind);
            assert_ne!(mood.ink, GAIN, "{kind:?} reuses the gain colour");
            assert_ne!(mood.ink, LOSS, "{kind:?} reuses the loss colour");
            assert_ne!(mood.wash, GAIN_SOFT);
            assert_ne!(mood.wash, LOSS_SOFT);
        }
    }

    /// Every mood needs a sentence: a tinted panel with no explanation is
    /// decoration without information.
    #[test]
    fn every_mood_explains_itself() {
        for kind in [MoodKind::Opportunity, MoodKind::Balanced, MoodKind::Caution] {
            let mood = Mood::of_kind(kind);
            assert!(!mood.note.is_empty());
            assert!(!mood.label.is_empty());
        }
    }

    #[test]
    fn unknown_state_falls_back_to_the_middle() {
        assert_eq!(Mood::resolve(None).kind, MoodKind::Balanced);
    }
}
