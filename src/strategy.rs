use crate::config::AppConfig;
use crate::models::Position;
use chrono::{Local, NaiveDate};

#[derive(Debug)]
pub struct BuySuggestion {
    pub total_amount: f64,
    pub us_amount: f64,
    pub cn_amount: f64,
    pub counter_amount: f64,
    pub details: Vec<BuyDetail>,
    pub excluded: Vec<ExcludedFromBuy>,
}

#[derive(Debug)]
pub struct BuyDetail {
    pub asset_code: String,
    pub asset_name: String,
    pub amount: f64,
}

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
