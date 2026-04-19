use crate::config::AppConfig;
use crate::models::Position;
use chrono::Local;

#[derive(Debug)]
pub struct BuySuggestion {
    pub total_amount: f64,
    pub us_amount: f64,
    pub cn_amount: f64,
    pub counter_amount: f64,
    pub details: Vec<BuyDetail>,
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
    pub annualized_return: f64,
    pub sell_ratio: f64,
    pub sell_shares: f64,
    pub sell_amount: f64,
}

#[derive(Debug)]
pub struct RiskWarning {
    pub asset_code: String,
    pub asset_name: String,
    pub loss_ratio: f64,
}

/// 计算买入建议
pub fn calculate_buy_suggestions(
    config: &AppConfig,
    score: f64,
    cash_balance: f64,
    positions: &[Position],
) -> BuySuggestion {
    let ratio = config.buy_ratio_for(score) / 100.0;
    let total_amount = cash_balance * ratio;

    let us_ratio = config.allocation.us_stocks / 100.0;
    let cn_ratio = config.allocation.cn_stocks / 100.0;
    let cc_ratio = config.allocation.counter_cyclical / 100.0;

    let us_amount = total_amount * us_ratio;
    let cn_amount = total_amount * cn_ratio;
    let counter_amount = total_amount * cc_ratio;

    // 按类别内持仓的市值比例分配
    let mut details = Vec::new();

    let us_positions: Vec<&Position> = positions.iter().filter(|p| p.category == "us_stocks").collect();
    let cn_positions: Vec<&Position> = positions.iter().filter(|p| p.category == "cn_stocks").collect();
    let cc_positions: Vec<&Position> = positions.iter().filter(|p| p.category == "counter_cyclical").collect();

    details.extend(distribute_amount(&us_positions, us_amount));
    details.extend(distribute_amount(&cn_positions, cn_amount));
    details.extend(distribute_amount(&cc_positions, counter_amount));

    BuySuggestion {
        total_amount,
        us_amount,
        cn_amount,
        counter_amount,
        details,
    }
}

fn distribute_amount(positions: &[&Position], total: f64) -> Vec<BuyDetail> {
    if positions.is_empty() || total <= 0.0 {
        return Vec::new();
    }
    if positions.len() == 1 {
        return vec![BuyDetail {
            asset_code: positions[0].asset_code.clone(),
            asset_name: positions[0].asset_name.clone(),
            amount: total,
        }];
    }
    // 按市值比例分配
    let total_mv: f64 = positions.iter().map(|p| p.market_value()).sum();
    if total_mv <= 0.0 {
        // 等额分配
        let per = total / positions.len() as f64;
        return positions
            .iter()
            .map(|p| BuyDetail {
                asset_code: p.asset_code.clone(),
                asset_name: p.asset_name.clone(),
                amount: per,
            })
            .collect();
    }
    positions
        .iter()
        .map(|p| BuyDetail {
            asset_code: p.asset_code.clone(),
            asset_name: p.asset_name.clone(),
            amount: total * (p.market_value() / total_mv),
        })
        .collect()
}

/// 计算卖出建议
pub fn calculate_sell_suggestions(
    config: &AppConfig,
    score: f64,
    positions: &[Position],
) -> Vec<SellSuggestion> {
    let today = Local::now().date_naive();
    let mut suggestions = Vec::new();

    for pos in positions {
        if pos.shares <= 0.0 {
            continue;
        }
        let current = match pos.current_price {
            Some(p) if p > 0.0 => p,
            _ => continue,
        };

        let ann_ret = match pos.annualized_return(&today) {
            Some(r) => r,
            None => continue,
        };

        let ratio = config.sell_ratio_for(score, ann_ret * 100.0) / 100.0;
        if ratio > 0.0 {
            let sell_shares = pos.shares * ratio;
            let sell_amount = sell_shares * current;
            suggestions.push(SellSuggestion {
                asset_code: pos.asset_code.clone(),
                asset_name: pos.asset_name.clone(),
                annualized_return: ann_ret,
                sell_ratio: ratio * 100.0,
                sell_shares,
                sell_amount,
            });
        }
    }
    suggestions
}

/// 检查风险警告（浮亏超 20%）
pub fn check_risk_warnings(positions: &[Position]) -> Vec<RiskWarning> {
    let mut warnings = Vec::new();
    for pos in positions {
        if pos.shares <= 0.0 || pos.cost_price <= 0.0 {
            continue;
        }
        if let Some(current) = pos.current_price {
            let ratio = current / pos.cost_price;
            if ratio < 0.8 {
                warnings.push(RiskWarning {
                    asset_code: pos.asset_code.clone(),
                    asset_name: pos.asset_name.clone(),
                    loss_ratio: (1.0 - ratio) * 100.0,
                });
            }
        }
    }
    warnings
}
