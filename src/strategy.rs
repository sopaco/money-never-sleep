use crate::config::AppConfig;
use crate::models::Position;
use chrono::{Local, NaiveDate};

/// 旧框架的建议结构。仍被 `Engine::Legacy` 对照回测使用，
/// 但已不再驱动 `mns report`（改由 `RebalancePlan`）。
#[allow(dead_code)]
#[derive(Debug)]
pub struct BuySuggestion {
    pub total_amount: f64,
    pub us_amount: f64,
    pub cn_amount: f64,
    pub counter_amount: f64,
    pub details: Vec<BuyDetail>,
    pub excluded: Vec<ExcludedFromBuy>,
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct BuyDetail {
    pub asset_code: String,
    pub asset_name: String,
    pub amount: f64,
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct SellSuggestion {
    pub asset_code: String,
    pub asset_name: String,
    pub annualized_return: Option<f64>,
    pub absolute_return: f64,
    pub sell_ratio: f64,
    pub sell_shares: f64,
    pub sell_amount: f64,
    pub reason: SellReason,
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum SellReason {
    AnnualizedHigh,      // 年化收益达标
    AbsoluteProfit,      // 绝对收益足够，长期持有获利了结
}

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

/// 计算卖出建议后回收的现金总额
fn total_sell_proceeds(suggestions: &[SellSuggestion]) -> f64 {
    suggestions.iter().map(|s| s.sell_amount).sum()
}

/// 买入建议中标记因高浮亏被排除加仓的标的
#[allow(dead_code)]
#[derive(Debug)]
pub struct ExcludedFromBuy {
    pub asset_code: String,
    pub asset_name: String,
    pub loss_ratio: f64,
    pub reason: String,
}

/// 计算买入建议
/// sell_proceeds: 卖出建议预计回收的现金，用于实现买卖互感知
/// risk_warnings: 风险警告列表，高浮亏标的将被排除加仓
pub fn calculate_buy_suggestions(
    config: &AppConfig,
    score: f64,
    cash_balance: f64,
    positions: &[Position],
    sell_suggestions: &[SellSuggestion],
    risk_warnings: &[RiskWarning],
) -> BuySuggestion {
    // 买入可用现金 = 当前现金 + 卖出回收
    let sell_proceeds = total_sell_proceeds(sell_suggestions);
    let available_cash = cash_balance + sell_proceeds;

    let ratio = config.buy_ratio_for(score) / 100.0;
    let total_amount = available_cash * ratio;

    let us_ratio = config.allocation.us_stocks / 100.0;
    let cn_ratio = config.allocation.cn_stocks / 100.0;
    let cc_ratio = config.allocation.counter_cyclical / 100.0;

    let us_amount = total_amount * us_ratio;
    let cn_amount = total_amount * cn_ratio;
    let counter_amount = total_amount * cc_ratio;

    // 按逆向加权分配：浮亏越多获得越多资金
    // 高浮亏标的（≥30%）排除加仓，避免"越亏越买"的风险
    let excluded: Vec<ExcludedFromBuy> = risk_warnings
        .iter()
        .filter(|w| w.loss_ratio >= 30.0)
        .map(|w| ExcludedFromBuy {
            asset_code: w.asset_code.clone(),
            asset_name: w.asset_name.clone(),
            loss_ratio: w.loss_ratio,
            reason: "浮亏≥30%，暂停逆向加仓以防基本面恶化".to_string(),
        })
        .collect();
    let excluded_codes: Vec<String> = excluded.iter().map(|e| e.asset_code.clone()).collect();

    let mut details = Vec::new();

    let us_positions: Vec<&Position> = positions.iter().filter(|p| p.category == "us_stocks").collect();
    let cn_positions: Vec<&Position> = positions.iter().filter(|p| p.category == "cn_stocks").collect();
    let cc_positions: Vec<&Position> = positions.iter().filter(|p| p.category == "counter_cyclical").collect();

    let max_weight = config.settings.max_contrarian_weight;

    details.extend(distribute_amount_contrarian(&us_positions, us_amount, max_weight, &excluded_codes));
    details.extend(distribute_amount_contrarian(&cn_positions, cn_amount, max_weight, &excluded_codes));
    details.extend(distribute_amount_contrarian(&cc_positions, counter_amount, max_weight, &excluded_codes));

    BuySuggestion {
        total_amount,
        us_amount,
        cn_amount,
        counter_amount,
        details,
        excluded,
    }
}

/// 逆向加权分配：浮亏/低估的标的获得更多资金
/// 权重 = min(max_weight, max(1.0, cost_price / current_price))，即浮亏越多权重越高但有上限
/// 若所有持仓都浮盈，则等额分配
/// excluded_codes: 因高浮亏被排除加仓的标的代码列表
fn distribute_amount_contrarian(positions: &[&Position], total: f64, max_weight: f64, excluded_codes: &[String]) -> Vec<BuyDetail> {
    if positions.is_empty() || total <= 0.0 {
        return Vec::new();
    }

    // 过滤掉被排除的标的
    let eligible: Vec<&&Position> = positions
        .iter()
        .filter(|p| !excluded_codes.contains(&p.asset_code))
        .collect();

    if eligible.is_empty() {
        return Vec::new();
    }
    if eligible.len() == 1 {
        return vec![BuyDetail {
            asset_code: eligible[0].asset_code.clone(),
            asset_name: eligible[0].asset_name.clone(),
            amount: total,
        }];
    }

    // 计算逆向权重：浮亏的标获得更高权重，但有上限防止过度集中
    let weights: Vec<f64> = eligible
        .iter()
        .map(|p| {
            match p.current_price {
                Some(cur) if cur > 0.0 && p.cost_price > 0.0 => {
                    // 浮亏时 cost/cur > 1，浮盈时 < 1，取 max(1.0, ...) 保证浮盈标的也有基础权重
                    // 限制最大权重防止单标的过度集中
                    (p.cost_price / cur).max(1.0).min(max_weight)
                }
                _ => 1.0, // 无现价时给予等额权重
            }
        })
        .collect();

    let total_weight: f64 = weights.iter().sum();
    if total_weight <= 0.0 {
        // 等额分配兜底
        let per = total / eligible.len() as f64;
        return eligible
            .iter()
            .map(|p| BuyDetail {
                asset_code: p.asset_code.clone(),
                asset_name: p.asset_name.clone(),
                amount: per,
            })
            .collect();
    }

    eligible
        .iter()
        .zip(weights.iter())
        .map(|(p, w)| BuyDetail {
            asset_code: p.asset_code.clone(),
            asset_name: p.asset_name.clone(),
            amount: total * (w / total_weight),
        })
        .collect()
}

/// 计算卖出建议
/// 改进：
/// 1. 使用最小持仓天数门槛，避免短期年化失真触发卖出
/// 2. 增加绝对收益考量：长期持有绝对收益超30%也可止盈
/// 3. 中性区间按PRD矩阵补齐
pub fn calculate_sell_suggestions(
    config: &AppConfig,
    score: f64,
    positions: &[Position],
) -> Vec<SellSuggestion> {
    let today = Local::now().date_naive();
    let min_days = config.settings.min_holding_days;
    let min_abs_days = config.settings.min_absolute_profit_days;
    let mut suggestions = Vec::new();

    for pos in positions {
        if pos.shares <= 0.0 {
            continue;
        }
        let current = match pos.current_price {
            Some(p) if p > 0.0 => p,
            _ => continue,
        };

        // 计算持仓天数
        let holding_days = NaiveDate::parse_from_str(&pos.first_buy_date, "%Y-%m-%d")
            .ok()
            .map(|d| (today - d).num_days())
            .unwrap_or(0);

        // 年化收益（含最小天数门槛）
        let ann_ret = pos.annualized_return_with_min_days(&today, min_days);

        // 绝对收益（不受天数限制）
        let abs_ret = pos.absolute_return().unwrap_or(0.0);

        // 判断是否触发卖出
        let (ratio, reason) = if let Some(ann) = ann_ret {
            // 年化收益有效，按矩阵判断
            let r = config.sell_ratio_for(score, ann * 100.0) / 100.0;
            if r > 0.0 {
                (r, SellReason::AnnualizedHigh)
            } else if abs_ret >= 0.30 && holding_days >= min_abs_days {
                // 年化不达标但绝对收益≥30%且持仓足够长（长期持有获利），在贪婪及以上环境减仓
                if score >= config.thresholds.neutral {
                    (0.20, SellReason::AbsoluteProfit)
                } else {
                    (0.0, SellReason::AnnualizedHigh) // 不触发
                }
            } else {
                (0.0, SellReason::AnnualizedHigh) // 不触发
            }
        } else if abs_ret >= 0.30 && holding_days >= min_abs_days {
            // 年化无效（持仓不足门槛天数），但绝对收益≥30%且持仓足够长
            // 根据情绪区间差异化减仓：极度贪婪更多，中性较少
            if score >= config.thresholds.greed {
                (0.15, SellReason::AbsoluteProfit)
            } else if score >= config.thresholds.neutral {
                (0.10, SellReason::AbsoluteProfit)
            } else {
                continue;
            }
        } else {
            continue;
        };

        if ratio > 0.0 {
            let sell_shares = pos.shares * ratio;
            let sell_amount = sell_shares * current;
            suggestions.push(SellSuggestion {
                asset_code: pos.asset_code.clone(),
                asset_name: pos.asset_name.clone(),
                annualized_return: ann_ret,
                absolute_return: abs_ret,
                sell_ratio: ratio * 100.0,
                sell_shares,
                sell_amount,
                reason,
            });
        }
    }
    // 按绝对收益从高到低排序：优先卖出收益最高的标的以锁定利润
    suggestions.sort_by(|a, b| b.absolute_return.partial_cmp(&a.absolute_return).unwrap_or(std::cmp::Ordering::Equal));
    suggestions
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
// 目标仓位 + 偏离带框架（新，与回测引擎同源）
//
// 旧逻辑的问题：买入是"现金的百分比"、卖出是"份额的百分比"，两者没有共同锚点，
// 导致 (1) 连续恐慌期弹药几何衰减；(2) 路径依赖——刚注资则买入金额被放大；
// (3) 无再平衡，权重可无限漂移；(4) 仓位无上限。
// 新逻辑始终对照"应该持有多少"，并只在偏离超过带宽时动作。
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

    let holdings: f64 = positions.iter().map(|p| p.market_value_or_cost()).sum();
    let total = cash_balance + holdings;

    let mut legs: Vec<LegPlan> = Vec::new();
    let mut total_sell = 0.0;
    let mut buy_needs: Vec<(usize, f64)> = Vec::new();

    for (idx, cat) in categories.iter().enumerate() {
        let members: Vec<&Position> = positions.iter().filter(|p| p.category == *cat).collect();
        let current: f64 = members.iter().map(|p| p.market_value_or_cost()).sum();
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
                // 优先减仓绝对收益最高者，锁定利润
                let mut ranked: Vec<&&Position> = sellable.clone();
                ranked.sort_by(|a, b| {
                    b.absolute_return()
                        .unwrap_or(0.0)
                        .partial_cmp(&a.absolute_return().unwrap_or(0.0))
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let mut remaining = diff;
                for p in ranked {
                    if remaining <= 0.0 {
                        break;
                    }
                    let mv = p.market_value_or_cost();
                    let take = remaining.min(mv);
                    if take > 0.0 {
                        plan.items.push(LegItem {
                            asset_code: p.asset_code.clone(),
                            asset_name: p.asset_name.clone(),
                            amount: -take,
                            note: Some(format!("持有{}天", holding_days(p, today))),
                        });
                        remaining -= take;
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
        leg.amount = amount;
        total_buy += amount;

        // 腿内分摊：排除高浮亏个股，宽基指数不排除
        let members: Vec<&Position> = positions
            .iter()
            .filter(|p| p.category == leg.category)
            .collect();
        leg.items = split_within_leg(&members, amount, config.settings.max_contrarian_weight);
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

fn holding_days(p: &Position, today: NaiveDate) -> i64 {
    NaiveDate::parse_from_str(&p.first_buy_date, "%Y-%m-%d")
        .ok()
        .map(|d| (today - d).num_days())
        .unwrap_or(0)
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
    fn 空仓且极度恐慌时应建议买到目标仓位() {
        let c = AppConfig::default_config();
        let p = calculate_rebalance_plan(&c, 20.0, 100_000.0, &[], today());
        assert_eq!(p.target_risk_weight, 85.0);
        assert_eq!(p.current_risk_weight, 0.0);
        // 无持仓标的，腿内无可分摊对象，但腿级金额应已给出
        let total: f64 = p.legs.iter().map(|l| l.amount).sum();
        assert!((total - 85_000.0).abs() < 1.0, "应建议买入约8.5万, 实际{}", total);
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
    fn 极度贪婪应减仓且优先卖出收益最高者() {
        let c = AppConfig::default_config();
        // 满仓风险资产，极度贪婪目标仅 35%
        let positions = vec![
            pos("A", "纳指ETF", "us_stocks", 1000.0, 1.0, 2.0, "2020-01-01"), // +100%
            pos("B", "纳指联接", "us_stocks", 1000.0, 1.0, 1.1, "2020-01-01"), // +10%
        ];
        let p = calculate_rebalance_plan(&c, 90.0, 0.0, &positions, today());
        assert!(p.total_sell > 0.0);
        let us = p.legs.iter().find(|l| l.category == "us_stocks").unwrap();
        assert!(!us.items.is_empty());
        assert_eq!(us.items[0].asset_code, "A", "应先卖绝对收益最高的标的");
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
        // 美股昨日刚买入（卖出被最短持有期封锁），其余两腿空缺，现金很少
        // → 需买入金额将超过可用资金
        let positions = vec![pos(
            "A", "纳指ETF", "us_stocks", 100_000.0, 1.0, 1.0, "2026-07-30",
        )];
        let p = calculate_rebalance_plan(&c, 50.0, 10_000.0, &positions, today());
        assert_eq!(p.total_sell, 0.0, "美股未满持有期不应卖出");
        assert!(p.cash_constrained, "应标记为受资金约束");
        let buy: f64 = p.legs.iter().map(|l| l.amount.max(0.0)).sum();
        assert!(buy <= 10_000.0 + 0.01, "买入不应超过可用现金: {}", buy);
    }

    #[test]
    fn 卖出回收资金应计入买入预算() {
        let c = AppConfig::default_config();
        // 美股严重超配、其余为零，现金为零 → 卖美股的钱应能买其他腿
        let positions = vec![pos(
            "A", "纳指ETF", "us_stocks", 100_000.0, 1.0, 1.0, "2020-01-01",
        )];
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
