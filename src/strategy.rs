use crate::config::AppConfig;
use crate::models::Position;
use chrono::NaiveDate;

/// 目标类别没有任何可买标的时，买入腿会被清零并挂上这条原因。
///
/// report、计划页、结论卡都认这一条常量来判断「这条腿的钱买不到东西」，
/// 而不是各自去看 `items.is_empty()`——同一件事三种写法，迟早会漂
/// （2026-11-11 复盘：CLI 报「买入 ¥148,700」、卡面却无标的可买）。
///
/// 这里**只存原因**，不存「去哪儿补」。CLI 的下一步是 `mns add`，GUI 的
/// 下一步是持仓页的「添加标的」按钮——把去处写进引擎常量，就会出现
/// 计划页自己写着「去计划页补齐」（那页根本没有添加标的入口）。
pub const NO_INSTRUMENT_REASON: &str = "目标类别暂无可买标的";

#[derive(Debug)]
pub struct RiskWarning {
    pub asset_code: String,
    pub asset_name: String,
    pub loss_ratio: f64,
    pub advice: RiskAdvice,
}

#[derive(Debug)]
pub enum RiskAdvice {
    ConsiderBuyMore,  // 恐慌环境下浮亏，可能是加仓机会
    ReviewFundamentals, // 中性环境下浮亏，审视基本面
    UrgentReview,      // 贪婪环境下浮亏，需要紧急审视
}

/// 检查风险警告（浮亏超 20%）
/// 改进：结合市场情绪给出差异化建议
pub fn check_risk_warnings(config: &AppConfig, score: f64, positions: &[Position]) -> Vec<RiskWarning> {
    let mut warnings = Vec::new();
    for pos in positions {
        if pos.shares <= 0.0 || pos.cost_price <= 0.0 {
            continue;
        }
        if let Some(current) = pos.current_price {
            let ratio = current / pos.cost_price;
            if ratio < 0.8 {
                let advice = if score < config.thresholds.fear {
                    // 恐慌环境下浮亏，可能是加仓机会
                    RiskAdvice::ConsiderBuyMore
                } else if score < config.thresholds.neutral {
                    // 中性环境下浮亏，审视基本面
                    RiskAdvice::ReviewFundamentals
                } else {
                    // 贪婪环境下浮亏，需要紧急审视（别人赚钱你还在亏）
                    RiskAdvice::UrgentReview
                };
                warnings.push(RiskWarning {
                    asset_code: pos.asset_code.clone(),
                    asset_name: pos.asset_name.clone(),
                    loss_ratio: (1.0 - ratio) * 100.0,
                    advice,
                });
            }
        }
    }
    warnings
}

// ═══════════════════════════════════════════════════════════════
// 目标仓位 + 偏离带框架（与回测引擎同源）
//
// 设计锚点：始终对照"应该持有多少风险资产"，并只在偏离超过带宽时动作。
// 不用"花掉手上现金的百分之几"式买入——那会导致 (1) 连续恐慌期弹药
// 几何衰减；(2) 路径依赖——刚注资则买入金额被放大；(3) 无再平衡，
// 权重可无限漂移；(4) 仓位无上限。
// ═══════════════════════════════════════════════════════════════

/// 单腿调仓建议
#[derive(Debug)]
pub struct LegPlan {
    pub category: String,
    pub category_cn: String,
    /// 目标权重（占总资产 %）
    pub target_weight: f64,
    /// 当前权重（%）
    pub current_weight: f64,
    /// 偏离（百分点），正数为超配
    pub drift_pp: f64,
    /// 建议金额，正数买入、负数卖出、0 为不动作
    pub amount: f64,
    /// 不动作的原因（在带宽内、金额过小、未过最短持有期等）
    pub hold_reason: Option<String>,
    /// 该腿下的标的与建议分摊金额
    pub items: Vec<LegItem>,
}

#[derive(Debug)]
pub struct LegItem {
    pub asset_code: String,
    pub asset_name: String,
    pub amount: f64,
    pub note: Option<String>,
    /// 该标的最早买入至今的持有天数（卖出费用估算用；买入腿为 None）
    pub holding_days: Option<i64>,
}

#[derive(Debug)]
pub struct RebalancePlan {
    /// 风险资产目标总权重（%）
    pub target_risk_weight: f64,
    /// 风险资产当前总权重（%）
    pub current_risk_weight: f64,
    pub total_assets: f64,
    pub cash_balance: f64,
    pub band_pp: f64,
    pub legs: Vec<LegPlan>,
    /// 全部建议卖出金额合计（正数）
    pub total_sell: f64,
    /// 全部建议买入金额合计
    pub total_buy: f64,
    /// 现金不足导致买入被按比例缩减
    pub cash_constrained: bool,
}

impl RebalancePlan {
    pub fn net_direction(&self) -> &'static str {
        let net = self.total_buy - self.total_sell;
        if net > 0.01 {
            "净买入"
        } else if net < -0.01 {
            "净卖出"
        } else {
            "持仓不动"
        }
    }

    pub fn has_action(&self) -> bool {
        self.total_buy > 0.0 || self.total_sell > 0.0
    }

    /// 按当前费率估算本次调仓的总费用（买入费 + 卖出费 + 阶梯赎回费）。
    ///
    /// 赎回费按各标的**最早买入日**估算持有天数，因此长期定投账户会低估
    /// 近期买入份额的赎回费——只作为量级参考，不做精确对账。
    pub fn estimated_fees(&self, costs: &crate::config::Costs) -> f64 {
        let mut total = 0.0;
        for leg in &self.legs {
            for item in &leg.items {
                if item.amount > 0.0 {
                    total += item.amount * costs.buy_rate();
                } else if item.amount < 0.0 {
                    let days = item.holding_days.unwrap_or(0);
                    total += -item.amount * costs.total_sell_rate(days);
                }
            }
        }
        total
    }
}

fn category_cn(category: &str) -> &'static str {
    match category {
        "us_stocks" => "美股",
        "cn_stocks" => "A股",
        "counter_cyclical" => "逆周期(黄金)",
        _ => "其他",
    }
}

/// 生成目标仓位调仓计划。
///
/// `today` 用于判断最短持有天数（规避国内基金惩罚性赎回费窗口）。
pub fn calculate_rebalance_plan(
    config: &AppConfig,
    score: f64,
    cash_balance: f64,
    positions: &[Position],
    today: NaiveDate,
) -> RebalancePlan {
    let categories = ["us_stocks", "cn_stocks", "counter_cyclical"];
    // 各腿占总资产的目标权重（已含风险资产总权重与腿内比例）
    let (tu, tc, tcc) = config.asset_target_weights(score);
    let leg_target = [tu, tc, tcc];
    let risk_w = config.target_weight_for(score) / 100.0;
    let band = config.rebalance.band_pp;
    let min_trade = config.rebalance.min_trade_amount;
    let min_hold = config.rebalance.min_holding_days_for_sell;

    // 空持仓时 .sum() 的浮点累加起点会留下 -0.0，"{:.1}" 打印成 "-0.0%"——
    // 加 0.0 归一化符号（-0.0 + 0.0 == 0.0，与 report.rs 账户概览同款处理）
    let holdings: f64 = positions.iter().map(|p| p.market_value_or_cost()).sum::<f64>() + 0.0;
    let total = cash_balance + holdings;

    let mut legs: Vec<LegPlan> = Vec::new();
    let mut total_sell = 0.0;
    let mut buy_needs: Vec<(usize, f64)> = Vec::new();

    for (idx, cat) in categories.iter().enumerate() {
        let members: Vec<&Position> = positions.iter().filter(|p| p.category == *cat).collect();
        // 同上：空腿 -0.0 归一化，避免腿表格"当前 -0.0%"
        let current: f64 = members.iter().map(|p| p.market_value_or_cost()).sum::<f64>() + 0.0;
        let target_value = total * leg_target[idx];
        let target_weight = leg_target[idx] * 100.0;
        let current_weight = if total > 0.0 { current / total * 100.0 } else { 0.0 };
        let drift_pp = current_weight - target_weight;
        let diff = current - target_value;

        let mut plan = LegPlan {
            category: cat.to_string(),
            category_cn: category_cn(cat).to_string(),
            target_weight,
            current_weight,
            drift_pp,
            amount: 0.0,
            hold_reason: None,
            items: Vec::new(),
        };

        if drift_pp.abs() <= band {
            plan.hold_reason = Some(format!("偏离 {:+.1}pp 在带宽 ±{:.1}pp 内", drift_pp, band));
        } else if diff.abs() < min_trade {
            plan.hold_reason = Some(format!("金额 ¥{:.0} 低于最小交易额", diff.abs()));
        } else if diff > 0.0 {
            // 超配 → 卖出。仅卖出已过最短持有期的标的
            let sellable: Vec<&&Position> = members
                .iter()
                .filter(|p| holding_days(p, today) >= min_hold)
                .collect();
            if sellable.is_empty() {
                plan.hold_reason = Some(format!(
                    "需减仓 ¥{:.0}，但持仓均未满 {} 天，卖出将触发惩罚性赎回费",
                    diff, min_hold
                ));
            } else {
                // 腿内等比例卖出：只对已过最短持有期的标的按**市值占比**分摊，
                // 不按浮盈排序——"优先卖盈利最多的"是处置效应，长期会系统性砍掉赢家。
                let sellable_mv: f64 = sellable.iter().map(|p| p.market_value_or_cost()).sum();
                let mut remaining = diff;
                if sellable_mv > 0.0 {
                    let ratio = (diff / sellable_mv).min(1.0);
                    for p in &sellable {
                        let mv = p.market_value_or_cost();
                        let take = (mv * ratio).min(remaining);
                        if take > 0.0 {
                            let days = holding_days(p, today);
                            plan.items.push(LegItem {
                                asset_code: p.asset_code.clone(),
                                asset_name: p.asset_name.clone(),
                                amount: -take,
                                note: Some(format!("持有{}天", days)),
                                holding_days: Some(days),
                            });
                            remaining -= take;
                        }
                    }
                }
                let sold = diff - remaining.max(0.0);
                plan.amount = -sold;
                total_sell += sold;
                if remaining > 1.0 {
                    plan.hold_reason =
                        Some(format!("剩余 ¥{:.0} 因可卖持仓不足未能减仓", remaining));
                }
            }
        } else {
            // 低配 → 买入，稍后受现金总额约束
            buy_needs.push((idx, -diff));
        }
        legs.push(plan);
    }

    // 买入预算 = 现金 + 卖出回收
    let available = cash_balance + total_sell;
    let need_sum: f64 = buy_needs.iter().map(|(_, v)| v).sum();
    let budget = available.min(need_sum);
    let cash_constrained = need_sum > available + 0.01;
    let mut total_buy = 0.0;

    for (idx, need) in &buy_needs {
        let amount = if need_sum > 0.0 { budget * (need / need_sum) } else { 0.0 };
        let leg = &mut legs[*idx];
        if amount < min_trade {
            leg.hold_reason = Some(if cash_constrained {
                format!("需加仓 ¥{:.0}，可用资金不足", need)
            } else {
                format!("金额 ¥{:.0} 低于最小交易额", amount)
            });
            continue;
        }

        // 腿内分摊：排除高浮亏个股，宽基指数不排除
        let members: Vec<&Position> = positions
            .iter()
            .filter(|p| p.category == leg.category)
            .collect();
        let items = split_within_leg(&members, amount, config.settings.max_contrarian_weight);
        if items.is_empty() {
            // 腿级金额算得出来，不代表买得到：该类别一个标的都没有（空池，
            // 或用户刚加的新类别）。这个数报出去就是一条无法执行的建议，
            // 还会混进 report 的净买入总额，让 CLI 与结论卡各说各话。
            // 清零 + 挂原因，三处界面自动同源。
            leg.amount = 0.0;
            leg.hold_reason = Some(NO_INSTRUMENT_REASON.to_owned());
            continue;
        }
        leg.amount = amount;
        leg.items = items;
        total_buy += amount;

        if cash_constrained {
            leg.hold_reason = Some(format!("需 ¥{:.0}，受可用资金限制按比例缩减", need));
        }
    }

    RebalancePlan {
        target_risk_weight: risk_w * 100.0,
        current_risk_weight: if total > 0.0 { holdings / total * 100.0 } else { 0.0 },
        total_assets: total,
        cash_balance,
        band_pp: band,
        legs,
        total_sell,
        total_buy,
        cash_constrained,
    }
}

/// 生成月度定投计划：**只买不卖**。
///
/// 分配规则：
/// - 有新钱先补低配腿，按缺口占比重分配；
/// - 所有腿都达标后，剩余金额按目标权重分配（维持配置，而不是追高）。
///
/// 可执行性：目标类别没有任何标的时，该腿金额清零并挂
/// [`NO_INSTRUMENT_REASON`]——`total_buy` 只累计**可执行**金额，因此可能小于
/// 月投入。缺口不挪给别的腿（否则等于拿缺标的类别的预算去追另一类，配置会漂），
/// 由调用方提示「留在现金」。标的齐备时各腿之和仍等于月投入。
pub fn calculate_monthly_plan(
    config: &AppConfig,
    score: f64,
    cash_balance: f64,
    positions: &[Position],
    monthly: f64,
) -> RebalancePlan {
    let categories = ["us_stocks", "cn_stocks", "counter_cyclical"];
    let (tu, tc, tcc) = config.asset_target_weights(score);
    let leg_target = [tu, tc, tcc];
    let risk_w = config.target_weight_for(score) / 100.0;
    // 同上：空持仓时 -0.0 归一化，避免计划行"当前 -0.0%"
    let holdings: f64 = positions.iter().map(|p| p.market_value_or_cost()).sum::<f64>() + 0.0;
    // 新钱到账后的总资产口径
    let total = cash_balance + holdings + monthly;

    let mut legs: Vec<LegPlan> = Vec::new();
    let mut deficits = Vec::new();
    let mut deficit_sum = 0.0;

    for (idx, cat) in categories.iter().enumerate() {
        let members: Vec<&Position> = positions.iter().filter(|p| p.category == *cat).collect();
        // 同上：空腿 -0.0 归一化，避免腿表格"当前 -0.0%"
        let current: f64 = members.iter().map(|p| p.market_value_or_cost()).sum::<f64>() + 0.0;
        let target_value = total * leg_target[idx];
        let current_weight = if total > 0.0 { current / total * 100.0 } else { 0.0 };
        let deficit = (target_value - current).max(0.0);
        deficit_sum += deficit;
        deficits.push(deficit);
        legs.push(LegPlan {
            category: cat.to_string(),
            category_cn: category_cn(cat).to_string(),
            target_weight: leg_target[idx] * 100.0,
            current_weight,
            drift_pp: current_weight - leg_target[idx] * 100.0,
            amount: 0.0,
            hold_reason: None,
            items: Vec::new(),
        });
    }

    let mut total_buy = 0.0;
    for (idx, leg) in legs.iter_mut().enumerate() {
        let amount = if monthly <= 0.0 {
            0.0
        } else if deficit_sum >= monthly {
            monthly * deficits[idx] / deficit_sum
        } else {
            // 缺口全部补齐，剩余按目标权重分配
            let share = if risk_w > 0.0 { leg_target[idx] / risk_w } else { 0.0 };
            deficits[idx] + (monthly - deficit_sum) * share
        };

        if amount <= 0.0 {
            leg.hold_reason = Some("本月无需买入".to_string());
            continue;
        }
        let members: Vec<&Position> = positions
            .iter()
            .filter(|p| p.category == leg.category)
            .collect();
        let items = split_within_leg(&members, amount, config.settings.max_contrarian_weight);
        if items.is_empty() {
            // 与再平衡同一条根因（R1 应修③）：这条腿的钱买不到东西，报出去
            // 就是一条无法执行的建议。清零 + 挂原因，合计只算可执行金额。
            // 缺口不挪给别的腿——那等于拿缺标的类别的预算去追另一类，配置会漂。
            leg.hold_reason = Some(NO_INSTRUMENT_REASON.to_owned());
            continue;
        }
        if deficits[idx] <= 0.0 {
            leg.hold_reason = Some("已达标，按目标权重分配新钱".to_string());
        }
        leg.amount = amount;
        leg.items = items;
        total_buy += amount;
    }

    RebalancePlan {
        target_risk_weight: risk_w * 100.0,
        current_risk_weight: if total > 0.0 { holdings / total * 100.0 } else { 0.0 },
        total_assets: total,
        cash_balance,
        band_pp: config.rebalance.band_pp,
        legs,
        total_sell: 0.0,
        total_buy,
        cash_constrained: false,
    }
}

fn holding_days(p: &Position, today: NaiveDate) -> i64 {
    NaiveDate::parse_from_str(&p.first_buy_date, "%Y-%m-%d")
        .ok()
        .map(|d| (today - d).num_days())
        .unwrap_or(0)
}

/// 档位切换的量化影响（供 `mns config profile` 切换前预告，纯计算便于测试）。
///
/// 切换档位会立即重锚目标仓位——这是产品自身"反冲击"机制（带宽/平滑/确认）
/// 管不到的盲区，用户应在按下开关前看到它将引发的调仓规模。
#[derive(Debug)]
pub struct ProfileSwitchImpact {
    /// 旧档位在当前决策分数下的风险资产目标权重（%）
    pub old_target: f64,
    /// 新档位在当前决策分数下的风险资产目标权重（%）
    pub new_target: f64,
    /// 当前实际风险资产权重（%）
    pub current_risk: f64,
    /// 新目标与当前权重的偏离（百分点，正数需买入）
    pub drift_pp: f64,
    /// 偏离是否在新带宽内（无需动作）
    pub in_band: bool,
    /// "买入" / "卖出" / "不动作"
    pub direction: &'static str,
    /// 预计调仓金额（补齐到目标，与 calculate_rebalance_plan 同口径）
    pub est_amount: f64,
}

pub fn profile_switch_impact(
    old: &AppConfig,
    new: &AppConfig,
    score: f64,
    cash: f64,
    positions: &[Position],
) -> ProfileSwitchImpact {
    let old_target = old.target_weight_for(score);
    let new_target = new.target_weight_for(score);
    // 空持仓时 f64 的 .sum() 返回 -0.0，"{:.1}" 会打印成 "-0.0%"——加 0.0 归一化
    // （与 report.rs 的账户概览同款处理）
    let holdings: f64 = positions.iter().map(|p| p.market_value_or_cost()).sum::<f64>() + 0.0;
    let total = cash + holdings;
    let current_risk = if total > 0.0 { holdings / total * 100.0 } else { 0.0 };
    let drift_pp = new_target - current_risk;
    let band = new.rebalance.band_pp;
    let in_band = total <= 0.0 || drift_pp.abs() <= band;
    let est_amount = if in_band { 0.0 } else { drift_pp.abs() / 100.0 * total };
    ProfileSwitchImpact {
        old_target,
        new_target,
        current_risk,
        drift_pp,
        in_band,
        direction: if in_band {
            "不动作"
        } else if drift_pp > 0.0 {
            "买入"
        } else {
            "卖出"
        },
        est_amount,
    }
}

/// 腿内分摊：逆向加权（浮亏者多分），但对**个股**的深度浮亏做排除，
/// 宽基指数/ETF 不排除——指数下跌 30% 正是逆向策略应加仓的时点，
/// 旧逻辑把"个股基本面恶化"的规则错误地套用到了宽基上。
fn split_within_leg(members: &[&Position], amount: f64, max_weight: f64) -> Vec<LegItem> {
    if members.is_empty() || amount <= 0.0 {
        return Vec::new();
    }
    let eligible: Vec<&&Position> = members
        .iter()
        .filter(|p| {
            let deep_loss = p
                .absolute_return()
                .map(|r| r <= -0.30)
                .unwrap_or(false);
            !(deep_loss && !is_broad_index(&p.asset_name, &p.asset_code))
        })
        .collect();
    let pool: Vec<&&Position> = if eligible.is_empty() {
        members.iter().collect()
    } else {
        eligible
    };

    let weights: Vec<f64> = pool
        .iter()
        .map(|p| match p.current_price {
            Some(cur) if cur > 0.0 && p.cost_price > 0.0 => {
                (p.cost_price / cur).max(1.0).min(max_weight)
            }
            _ => 1.0,
        })
        .collect();
    let sum: f64 = weights.iter().sum();
    pool.iter()
        .zip(&weights)
        .map(|(p, w)| LegItem {
            asset_code: p.asset_code.clone(),
            asset_name: p.asset_name.clone(),
            amount: if sum > 0.0 { amount * (w / sum) } else { amount / pool.len() as f64 },
            note: None,
            holding_days: None,
        })
        .collect()
}

/// 粗略判断是否为宽基指数/ETF（用于区分"指数下跌"与"个股基本面恶化"）
pub fn is_broad_index(name: &str, code: &str) -> bool {
    const KEYWORDS: [&str; 14] = [
        "指数", "ETF", "etf", "沪深300", "中证", "上证", "创业板", "科创",
        "纳斯达克", "纳指", "标普", "500", "红利", "联接",
    ];
    KEYWORDS.iter().any(|k| name.contains(k) || code.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 7, 31).unwrap()
    }

    fn pos(code: &str, name: &str, cat: &str, shares: f64, cost: f64, cur: f64, first: &str) -> Position {
        Position {
            id: 1,
            asset_code: code.to_string(),
            asset_name: name.to_string(),
            category: cat.to_string(),
            shares,
            cost_price: cost,
            current_price: Some(cur),
            first_buy_date: first.to_string(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn 空池不给无法执行的买入金额() {
        let c = AppConfig::default_config();
        let p = calculate_rebalance_plan(&c, 20.0, 100_000.0, &[], today());
        assert_eq!(p.target_risk_weight, 85.0);
        assert_eq!(p.current_risk_weight, 0.0);
        // assert_eq 抓不住 -0.0（IEEE 里 -0.0 == 0.0），必须显式查符号：
        // 空持仓时 .sum() 的浮点累加会留下 -0.0，"{:.1}" 打印成 "-0.0%"
        assert!(
            p.current_risk_weight.is_sign_positive(),
            "当前权重符号应为正零，实际 {:?}",
            p.current_risk_weight
        );
        assert!(
            p.legs.iter().all(|l| l.current_weight.is_sign_positive()),
            "腿级当前权重符号应为正零（表格里不得出现 -0.0%）"
        );
        // 一个标的都没有时，腿级目标金额不是"可执行建议"：清零并说明原因，
        // 否则 report 的净买入总额会含一笔买不到的钱。
        assert_eq!(p.total_buy, 0.0);
        assert!(!p.has_action());
        assert!(p.legs.iter().all(|l| l.amount == 0.0));
        assert!(
            p.legs
                .iter()
                .all(|l| l.hold_reason.as_deref() == Some(NO_INSTRUMENT_REASON)),
            "空池的腿必须挂上「暂无可买标的」原因"
        );
    }

    #[test]
    fn 只有部分类别有标的时只为有标的的腿报金额() {
        // 标的存在、份额还是 0（刚 add 未买）也算可执行：不能把「还没买」
        // 误判成「买不到」。另外两条没有标的的腿必须清零，不进总额。
        let c = AppConfig::default_config();
        let positions = vec![pos("QQQ", "纳指ETF", "us_stocks", 0.0, 0.0, 1.0, "2020-01-01")];
        let p = calculate_rebalance_plan(&c, 20.0, 100_000.0, &positions, today());

        let us = p.legs.iter().find(|l| l.category == "us_stocks").unwrap();
        assert!(us.amount > 0.0, "有标的的腿应给出金额");
        assert!(!us.items.is_empty(), "有标的的腿应分摊到具体标的");
        assert!(us.hold_reason.is_none());

        for leg in p.legs.iter().filter(|l| l.category != "us_stocks") {
            assert_eq!(leg.amount, 0.0, "{}腿没有标的，不应报金额", leg.category_cn);
            assert_eq!(leg.hold_reason.as_deref(), Some(NO_INSTRUMENT_REASON));
        }
        // 总额只含有标的的腿
        let legs_sum: f64 = p.legs.iter().map(|l| l.amount).sum();
        assert!((p.total_buy - legs_sum).abs() < 0.01);
        assert!(p.total_buy < 85_000.0, "无标的的腿不应计入总额");
    }

    #[test]
    fn 偏离在带宽内应不动作() {
        let c = AppConfig::default_config();
        // 中性目标 60%，构造刚好 60% 风险资产
        let positions = vec![
            pos("QQQ", "纳指ETF", "us_stocks", 330.0, 1.0, 1.0, "2020-01-01"),
            pos("DIV", "红利ETF", "cn_stocks", 150.0, 1.0, 1.0, "2020-01-01"),
            pos("AU", "黄金ETF", "counter_cyclical", 120.0, 1.0, 1.0, "2020-01-01"),
        ];
        let p = calculate_rebalance_plan(&c, 50.0, 400.0, &positions, today());
        assert!(!p.has_action(), "应无动作, legs={:?}", p.legs);
        assert!(p.legs.iter().all(|l| l.hold_reason.is_some()));
    }

    #[test]
    fn 极度贪婪应减仓且按市值等比例卖出() {
        let c = AppConfig::default_config();
        // 满仓风险资产，极度贪婪目标仅 35%
        let positions = vec![
            pos("A", "纳指ETF", "us_stocks", 1000.0, 1.0, 2.0, "2020-01-01"), // 市值 2000, +100%
            pos("B", "纳指联接", "us_stocks", 1000.0, 1.0, 1.1, "2020-01-01"), // 市值 1100, +10%
        ];
        let p = calculate_rebalance_plan(&c, 90.0, 0.0, &positions, today());
        assert!(p.total_sell > 0.0);
        let us = p.legs.iter().find(|l| l.category == "us_stocks").unwrap();
        assert_eq!(us.items.len(), 2, "两只都应被卖出，而不是只砍赢家: {:?}", us.items);
        // 卖出金额应与市值成比例（2000 : 1100）
        let a = us.items.iter().find(|i| i.asset_code == "A").unwrap();
        let b = us.items.iter().find(|i| i.asset_code == "B").unwrap();
        let ratio = (-a.amount) / (-b.amount);
        assert!(
            (ratio - 2000.0 / 1100.0).abs() < 0.02,
            "应按市值等比例卖出，实际比例 {:.3}",
            ratio
        );
    }

    #[test]
    fn 浮盈差异大时仍按市值比例卖出() {
        let mut c = AppConfig::default_config();
        c.rebalance.band_pp = 0.0;
        c.rebalance.min_holding_days_for_sell = 0;
        // 同为美股，A 翻三倍、B 只涨 5%，目标权重把美股压到低配
        let positions = vec![
            pos("A", "纳指ETF", "us_stocks", 1000.0, 1.0, 3.0, "2020-01-01"), // 3000
            pos("B", "标普ETF", "us_stocks", 1000.0, 1.0, 1.05, "2020-01-01"), // 1050
        ];
        let p = calculate_rebalance_plan(&c, 90.0, 0.0, &positions, today());
        let us = p.legs.iter().find(|l| l.category == "us_stocks").unwrap();
        let a = us.items.iter().find(|i| i.asset_code == "A").unwrap();
        let b = us.items.iter().find(|i| i.asset_code == "B").unwrap();
        let ratio = (-a.amount) / (-b.amount);
        assert!(
            (ratio - 3000.0 / 1050.0).abs() < 0.02,
            "收益率差距不应影响卖出比例，实际 {:.3}",
            ratio
        );
    }

    #[test]
    fn 未满最短持有期的标的不参与等比例卖出() {
        let mut c = AppConfig::default_config();
        c.rebalance.min_holding_days_for_sell = 30;
        let positions = vec![
            pos("OLD", "纳指ETF", "us_stocks", 1000.0, 1.0, 2.0, "2020-01-01"),
            pos("NEW", "纳指联接", "us_stocks", 1000.0, 1.0, 2.0, "2026-07-30"),
        ];
        let p = calculate_rebalance_plan(&c, 90.0, 0.0, &positions, today());
        let us = p.legs.iter().find(|l| l.category == "us_stocks").unwrap();
        assert_eq!(us.items.len(), 1);
        assert_eq!(us.items[0].asset_code, "OLD");
        assert!(us.items[0].holding_days.unwrap() >= 30);
    }

    #[test]
    fn 费用估算包含买入费与阶梯赎回费() {
        let c = AppConfig::default_config();
        let positions = vec![
            pos("A", "纳指ETF", "us_stocks", 1000.0, 1.0, 2.0, "2020-01-01"),
            pos("B", "红利ETF", "cn_stocks", 100.0, 1.0, 1.0, "2026-07-15"),
        ];
        let p = calculate_rebalance_plan(&c, 90.0, 5_000.0, &positions, today());
        let fees = p.estimated_fees(&c.costs);
        assert!(fees > 0.0, "有交易就应有费用估算");
        // 买入费 0.12%、A 已持有多年（免赎回费，只剩卖出 0.05%）
        let expected_buy = p.total_buy * 0.0012;
        let expected_sell_a = 2000.0f64.min(p.total_sell) * 0.0005;
        assert!(
            (fees - (expected_buy + expected_sell_a)).abs() < 1.0,
            "费用 {:.2} 与预期 {:.2} 不符",
            fees,
            expected_buy + expected_sell_a
        );
    }

    #[test]
    fn 定投计划各腿金额之和等于月投入() {
        let c = AppConfig::default_config();
        let positions = vec![
            pos("QQQ", "纳指ETF", "us_stocks", 100.0, 1.0, 1.0, "2020-01-01"),
            pos("DIV", "红利ETF", "cn_stocks", 100.0, 1.0, 1.0, "2020-01-01"),
            pos("GLD", "黄金ETF", "counter_cyclical", 100.0, 1.0, 1.0, "2020-01-01"),
        ];
        let p = calculate_monthly_plan(&c, 50.0, 1_000.0, &positions, 3_000.0);
        let sum: f64 = p.legs.iter().map(|l| l.amount).sum();
        assert!((sum - 3000.0).abs() < 0.01, "各腿之和应等于月投入: {}", sum);
        assert!(p.legs.iter().all(|l| l.amount >= 0.0), "定投只买不卖");
        assert_eq!(p.total_sell, 0.0);
        assert!((p.total_buy - 3000.0).abs() < 0.01);
        // 低配腿应分到更多钱
        let us = p.legs.iter().find(|l| l.category == "us_stocks").unwrap();
        let cc = p.legs.iter().find(|l| l.category == "counter_cyclical").unwrap();
        assert!(us.amount > cc.amount);
    }

    #[test]
    fn 定投缺标的的腿不给金额且合计只算可执行部分() {
        // 产品 B 批：计划页是用户「照着买」的主路径。只持有美股基金时，
        // 逆周期腿算得出 ¥101 却一条标的都没有——这个数不能报，否则用户
        // 拿着钱买不到东西，同屏的结论卡却说「该类别暂无可买标的」。
        let c = AppConfig::default_config();
        let positions = vec![pos(
            "QQQ", "纳指ETF", "us_stocks", 100.0, 1.0, 1.0, "2020-01-01",
        )];
        let monthly = 3_000.0;
        let p = calculate_monthly_plan(&c, 22.0, 1_000.0, &positions, monthly);

        let us = p.legs.iter().find(|l| l.category == "us_stocks").unwrap();
        assert!(us.amount > 0.0, "有标的的腿应给金额");
        assert!(!us.items.is_empty());

        for leg in p.legs.iter().filter(|l| l.category != "us_stocks") {
            assert_eq!(leg.amount, 0.0, "{}腿没有标的，不应报金额", leg.category_cn);
            assert_eq!(
                leg.hold_reason.as_deref(),
                Some(NO_INSTRUMENT_REASON),
                "{}腿应挂上补齐标的的指引",
                leg.category_cn
            );
            assert!(leg.items.is_empty());
        }
        let legs_sum: f64 = p.legs.iter().map(|l| l.amount).sum();
        assert!((p.total_buy - legs_sum).abs() < 0.01, "合计应等于可执行腿之和");
        assert!(
            p.total_buy < monthly,
            "合计应小于月投入（缺口留在现金）: {}",
            p.total_buy
        );
    }

    #[test]
    fn 定投在所有腿达标后按目标权重分配() {
        let c = AppConfig::default_config();
        // 极度贪婪时目标仅 35%，现金充裕 → 所有腿都无需补缺
        let positions = vec![
            pos("QQQ", "纳指ETF", "us_stocks", 40_000.0, 1.0, 1.0, "2020-01-01"),
            pos("DIV", "红利ETF", "cn_stocks", 40_000.0, 1.0, 1.0, "2020-01-01"),
            pos("GLD", "黄金ETF", "counter_cyclical", 40_000.0, 1.0, 1.0, "2020-01-01"),
        ];
        let p = calculate_monthly_plan(&c, 90.0, 1_000_000.0, &positions, 3_000.0);
        let sum: f64 = p.legs.iter().map(|l| l.amount).sum();
        assert!((sum - 3000.0).abs() < 0.01, "各腿之和应等于月投入: {}", sum);
    }

    #[test]
    fn 未满最短持有期不应建议卖出() {
        let mut c = AppConfig::default_config();
        c.rebalance.min_holding_days_for_sell = 30;
        // 昨天刚买入，极度贪婪本应减仓
        let positions = vec![pos("A", "纳指ETF", "us_stocks", 1000.0, 1.0, 2.0, "2026-07-30")];
        let p = calculate_rebalance_plan(&c, 90.0, 0.0, &positions, today());
        assert_eq!(p.total_sell, 0.0, "应因惩罚性赎回费窗口而不卖");
        let us = p.legs.iter().find(|l| l.category == "us_stocks").unwrap();
        assert!(us.hold_reason.as_ref().unwrap().contains("惩罚性"));
    }

    #[test]
    fn 买入受可用现金约束时应按比例缩减() {
        let c = AppConfig::default_config();
        // 美股昨日刚买入（卖出被最短持有期封锁），其余两腿低配，现金很少
        // → 需买入金额将超过可用资金。
        // A股/逆周期各放一个份额为 0 的标的（刚 add 未买，可执行）：否则这两
        // 条腿会被「暂无可买标的」清零，`buy <= 10_000` 就成了一恒真断言，
        // 测不到「按比例缩减」这件事。
        let positions = vec![
            pos("A", "纳指ETF", "us_stocks", 100_000.0, 1.0, 1.0, "2026-07-30"),
            pos("DIV", "红利ETF", "cn_stocks", 0.0, 0.0, 1.0, "2020-01-01"),
            pos("GLD", "黄金ETF", "counter_cyclical", 0.0, 0.0, 1.0, "2020-01-01"),
        ];
        let p = calculate_rebalance_plan(&c, 50.0, 10_000.0, &positions, today());
        assert_eq!(p.total_sell, 0.0, "美股未满持有期不应卖出");
        assert!(p.cash_constrained, "应标记为受资金约束");
        let buy: f64 = p.legs.iter().map(|l| l.amount.max(0.0)).sum();
        assert!(
            (buy - 10_000.0).abs() < 1.0,
            "可用现金应被足额分配（按缺口比例缩减），实际买入 {:.2}",
            buy
        );
        assert!((p.total_buy - buy).abs() < 0.01, "总额应等于各腿之和");
        // 每条有标的的腿都应有可执行明细，且都挂了缩减原因
        for leg in p.legs.iter().filter(|l| l.amount > 0.0) {
            assert!(!leg.items.is_empty(), "{}腿应有可执行明细", leg.category_cn);
            assert!(
                leg.hold_reason.as_deref().unwrap_or("").contains("受可用资金限制"),
                "{}腿应披露缩减原因",
                leg.category_cn
            );
        }
    }

    #[test]
    fn 标的齐备时应买到目标仓位() {
        // happy path：空仓 + 三腿各有一个标的（份额 0，刚 add 未买）→ 钱应
        // 按腿目标权重分完，且总额等于目标风险仓位对应的金额。
        let c = AppConfig::default_config();
        let positions = vec![
            pos("QQQ", "纳指ETF", "us_stocks", 0.0, 0.0, 1.0, "2020-01-01"),
            pos("DIV", "红利ETF", "cn_stocks", 0.0, 0.0, 1.0, "2020-01-01"),
            pos("GLD", "黄金ETF", "counter_cyclical", 0.0, 0.0, 1.0, "2020-01-01"),
        ];
        let p = calculate_rebalance_plan(&c, 20.0, 100_000.0, &positions, today());
        assert_eq!(p.target_risk_weight, 85.0);
        assert!(!p.cash_constrained, "现金充足不应缩减");
        assert!(
            (p.total_buy - 85_000.0).abs() < 1.0,
            "应买到目标仓位约 8.5 万，实际 {:.2}",
            p.total_buy
        );
        let legs_sum: f64 = p.legs.iter().map(|l| l.amount).sum();
        assert!((legs_sum - p.total_buy).abs() < 0.01);

        // 各腿金额应按腿目标权重分配（目标权重已是占总资产的百分比）
        let total_assets = p.total_assets;
        for leg in &p.legs {
            let expected = total_assets * leg.target_weight / 100.0;
            assert!(
                (leg.amount - expected).abs() < 1.0,
                "{}腿应约 ¥{:.2}，实际 ¥{:.2}",
                leg.category_cn,
                expected,
                leg.amount
            );
            assert!(!leg.items.is_empty(), "{}腿应有可执行明细", leg.category_cn);
            assert!(leg.hold_reason.is_none(), "{}腿不应有阻碍原因", leg.category_cn);
        }
    }

    #[test]
    fn 卖出回收资金应计入买入预算() {
        let c = AppConfig::default_config();
        // 美股严重超配、现金为零 → 卖美股的钱应能买其他腿。
        // A股给一个份额为 0 的标的（刚 add 未买），否则该腿没有可买对象，
        // 引擎会按「暂无可买标的」清零，这笔钱就无处可去。
        let positions = vec![
            pos("A", "纳指ETF", "us_stocks", 100_000.0, 1.0, 1.0, "2020-01-01"),
            pos("DIV", "红利ETF", "cn_stocks", 0.0, 0.0, 1.0, "2020-01-01"),
        ];
        let p = calculate_rebalance_plan(&c, 50.0, 0.0, &positions, today());
        assert!(p.total_sell > 0.0, "美股应减仓");
        assert!(p.total_buy > 0.0, "卖出回收应能支持其他腿买入");
        assert!(
            p.total_buy <= p.total_sell + 0.01,
            "买入不应超过回收金额: buy={} sell={}",
            p.total_buy,
            p.total_sell
        );
    }

    #[test]
    fn 宽基指数深度浮亏不应被排除加仓() {
        // 这是对旧规则的修正：指数跌30%正是该买的时候
        let members = vec![pos("510880", "红利ETF", "cn_stocks", 100.0, 2.0, 1.2, "2020-01-01")];
        let refs: Vec<&Position> = members.iter().collect();
        let items = split_within_leg(&refs, 10_000.0, 2.0);
        assert_eq!(items.len(), 1, "宽基指数应仍可加仓");
        assert!((items[0].amount - 10_000.0).abs() < 1e-6);
    }

    #[test]
    fn 个股深度浮亏应被排除加仓() {
        let members = vec![
            pos("600519", "某白酒股份", "cn_stocks", 100.0, 2.0, 1.2, "2020-01-01"), // -40%
            pos("510880", "红利ETF", "cn_stocks", 100.0, 1.0, 1.0, "2020-01-01"),
        ];
        let refs: Vec<&Position> = members.iter().collect();
        let items = split_within_leg(&refs, 10_000.0, 2.0);
        assert_eq!(items.len(), 1, "个股深亏应被排除");
        assert_eq!(items[0].asset_code, "510880");
    }

    #[test]
    fn 宽基识别() {
        assert!(is_broad_index("嘉实沪深300红利低波动ETF联接A", "007605"));
        assert!(is_broad_index("华安纳斯达克100", "040046"));
        assert!(is_broad_index("红利ETF华泰柏瑞", "510880"));
        assert!(!is_broad_index("某白酒股份", "600519"));
    }

    #[test]
    fn 目标仓位应随情绪单调不增() {
        let c = AppConfig::default_config();
        let mut prev = f64::INFINITY;
        for score in [10.0, 35.0, 50.0, 60.0, 80.0] {
            let p = calculate_rebalance_plan(&c, score, 100_000.0, &[], today());
            assert!(p.target_risk_weight <= prev, "情绪{}目标仓位不应升高", score);
            prev = p.target_risk_weight;
        }
    }

    #[test]
    fn 档位切换预告_更进取档位应预告买入() {
        let mut old = AppConfig::default_config();
        old.apply_profile(crate::config::RiskProfile::Steady);
        let mut new = old.clone();
        new.apply_profile(crate::config::RiskProfile::Aggressive);
        // 恐慌期（score 30，默认曲线 fear=75）：
        // steady(0.4) 目标 = 40+0.6×75 = 85；aggressive(0.8) 目标 = 40+0.2×75 = 95
        let positions = vec![
            pos("A", "纳指ETF", "us_stocks", 500.0, 1.0, 1.0, "2020-01-01"),
            pos("B", "红利ETF", "cn_stocks", 300.0, 1.0, 1.0, "2020-01-01"),
            pos("C", "黄金ETF", "counter_cyclical", 200.0, 1.0, 1.0, "2020-01-01"),
        ]; // 风险资产 1000 + 现金 1000 → current 50%
        let impact = profile_switch_impact(&old, &new, 30.0, 1_000.0, &positions);
        assert!((impact.old_target - 85.0).abs() < 1e-9);
        assert!((impact.new_target - 95.0).abs() < 1e-9);
        assert!((impact.current_risk - 50.0).abs() < 1e-9);
        assert_eq!(impact.direction, "买入");
        assert!((impact.est_amount - 0.45 * 2000.0).abs() < 1e-9);
    }

    #[test]
    fn 档位切换预告_偏离在带宽内不动作() {
        let mut c = AppConfig::default_config();
        c.apply_profile(crate::config::RiskProfile::Steady);
        let mut new = c.clone();
        new.apply_profile(crate::config::RiskProfile::Balanced);
        // score 90（极度贪婪，曲线 extreme_greed=35）：
        // steady(0.4) 目标 = 40+0.6×35 = 61%；balanced(0.6) 目标 = 60+0.4×35 = 74%
        // current 构造为 72% → 新档位偏离 +2pp < 带宽 4pp → 无需动作
        let positions = vec![
            pos("A", "纳指ETF", "us_stocks", 720.0, 1.0, 1.0, "2020-01-01"),
            pos("B", "红利ETF", "cn_stocks", 0.0, 1.0, 1.0, "2020-01-01"),
            pos("C", "黄金ETF", "counter_cyclical", 0.0, 1.0, 1.0, "2020-01-01"),
        ]; // 风险 720 + 现金 280 → current 恰为 72%
        let impact = profile_switch_impact(&c, &new, 90.0, 280.0, &positions);
        assert!((impact.new_target - 74.0).abs() < 1e-9);
        assert!(impact.in_band, "偏离 {:+.1}pp 应在带宽内", impact.drift_pp);
        assert_eq!(impact.direction, "不动作");
        assert_eq!(impact.est_amount, 0.0);
    }

    #[test]
    fn 档位切换预告_切到无档位在贪婪期预告卖出() {
        let mut old = AppConfig::default_config();
        old.apply_profile(crate::config::RiskProfile::Steady);
        let mut new = old.clone();
        new.apply_profile(crate::config::RiskProfile::Base);
        // score 90：steady 目标 61%，无档位目标 = 曲线原样 35%
        let positions = vec![
            pos("A", "纳指ETF", "us_stocks", 650.0, 1.0, 1.0, "2020-01-01"),
            pos("B", "红利ETF", "cn_stocks", 390.0, 1.0, 1.0, "2020-01-01"),
            pos("C", "黄金ETF", "counter_cyclical", 260.0, 1.0, 1.0, "2020-01-01"),
        ]; // 风险 1300 + 现金 700 → current 65%
        let impact = profile_switch_impact(&old, &new, 90.0, 700.0, &positions);
        assert!((impact.old_target - 61.0).abs() < 1e-9);
        assert!((impact.new_target - 35.0).abs() < 1e-9, "无档位贪婪目标应为曲线原样 35");
        assert_eq!(impact.direction, "卖出");
        assert!((impact.est_amount - 0.30 * 2000.0).abs() < 1e-9);
    }

    #[test]
    fn 无路径依赖_相同权重下建议与现金规模无关() {
        // 旧框架下"买入=现金的百分比"，刚注资会放大买入金额；
        // 新框架的目标是权重，故同一权重状态下建议方向应一致。
        let c = AppConfig::default_config();
        let mk = |scale: f64| {
            vec![
                pos("A", "纳指ETF", "us_stocks", 550.0 * scale, 1.0, 1.0, "2020-01-01"),
                pos("B", "红利ETF", "cn_stocks", 250.0 * scale, 1.0, 1.0, "2020-01-01"),
                pos("C", "黄金ETF", "counter_cyclical", 200.0 * scale, 1.0, 1.0, "2020-01-01"),
            ]
        };
        // 两个规模不同但权重结构相同的组合（风险资产 100%，目标 60%）
        // 规模取足够大，避免最小交易额门槛干扰比例比较
        let p1 = calculate_rebalance_plan(&c, 50.0, 0.0, &mk(100.0), today());
        let p2 = calculate_rebalance_plan(&c, 50.0, 0.0, &mk(1000.0), today());
        // 卖出占总资产比例应相同
        let r1 = p1.total_sell / p1.total_assets;
        let r2 = p2.total_sell / p2.total_assets;
        assert!((r1 - r2).abs() < 1e-9, "{} vs {}", r1, r2);
    }
}
