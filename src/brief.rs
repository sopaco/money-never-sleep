//! 托盘简报的数据契约。
//!
//! 托盘宿主不再常驻 AppShell，托盘上的数字只能向唯一的数据服务方
//! （`mns-butlerd`）要：宿主发 [`crate::ipc::Signal::RequestBrief`]，守护
//! 进程读一次账本快照，把本模块的结构体序列化回 [`crate::ipc::Signal::Brief`]。
//!
//! 这里只放**事实**，不放文案：句子怎么写、什么时候加「数据非当日」的
//! 提醒，仍归 GUI（托盘排版）。这样守护进程不依赖 `gui` 特性也能算出
//! 同一套数字，测试不启动图形界面也能验证。

use serde::{Deserialize, Serialize};

/// 一次托盘简报所需的全部数字。字段刻意保持扁平：收发两端都不需要再读
/// 数据库或配置文件。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BriefData {
    /// 快照日期，`YYYY-MM-DD`。与 `freshness` 一起显示，旧数字不能读成今天。
    pub as_of: String,
    /// `Freshness::summary()` 的成品句子（「报价 3 小时前」之类）。
    pub freshness: String,
    pub total_assets: f64,
    pub xirr: Option<f64>,
    /// 持仓市值 / 总资产，百分比。计划缺失时托盘用它表达风险仓位。
    pub risk_weight_pct: f64,
    pub plan: Option<PlanBrief>,
    pub signal: Option<SignalBrief>,
    /// 计划生成受阻的原因（缺情绪数据、金额未设置等）。
    pub plan_blocked: Option<String>,
    pub decision_label: String,
    /// 价格旧到不能支撑「偏离多少」的判断（与管家事实闸门同一口径）。
    pub price_stale: bool,
    /// 情绪快照距今天数；`None` = 没有情绪数据。
    pub fgi_age_days: Option<i64>,
    /// 快照读取失败时的原因。数字全为 0 时，宿主要先说清「读不到」，
    /// 不能让 0 读成「账户空了」。
    pub load_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanBrief {
    pub current_risk_pct: f64,
    pub target_risk_pct: f64,
    pub band_pp: f64,
    pub total_buy: f64,
    pub total_sell: f64,
    /// 计划里是否有达到最小金额的买卖腿（`PlanView::has_orders`）。
    pub has_orders: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignalBrief {
    pub effective_score: f64,
    pub effective_zone: String,
    pub confirmed: bool,
}

impl BriefData {
    /// 从一个完整快照提取简报。调用方负责加载快照（守护进程在
    /// `spawn_blocking` 里读，别阻塞 `select` 循环）。
    pub fn from_snapshot(snapshot: &crate::app::Snapshot) -> Self {
        Self {
            as_of: snapshot.as_of.format("%Y-%m-%d").to_string(),
            freshness: snapshot.freshness.summary(),
            total_assets: snapshot.total_assets,
            xirr: snapshot.xirr,
            risk_weight_pct: snapshot.risk_weight_pct(),
            plan: snapshot.plan.as_ref().map(|plan| PlanBrief {
                current_risk_pct: plan.current_risk_pct,
                target_risk_pct: plan.target_risk_pct,
                band_pp: plan.band_pp,
                total_buy: plan.total_buy,
                total_sell: plan.total_sell,
                has_orders: plan.has_orders(),
            }),
            signal: snapshot.signal.as_ref().map(|signal| SignalBrief {
                effective_score: signal.effective_score,
                effective_zone: signal.effective_zone.clone(),
                confirmed: signal.confirmed,
            }),
            plan_blocked: snapshot.plan_blocked.clone(),
            decision_label: snapshot.config.decision_label.clone(),
            price_stale: snapshot.freshness.price_is_stale(),
            fgi_age_days: snapshot.freshness.fgi_age_days,
            load_error: None,
        }
    }

    /// 读不到账本时的简报：把原因带回去，托盘先说「读取失败」。
    pub fn from_error(error: impl Into<String>) -> Self {
        Self {
            load_error: Some(error.into()),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_brief_carries_a_reason_and_no_numbers() {
        let brief = BriefData::from_error("账本不可读");
        assert_eq!(brief.load_error.as_deref(), Some("账本不可读"));
        assert_eq!(brief.total_assets, 0.0);
        assert!(brief.plan.is_none());
        assert!(brief.signal.is_none());
    }

    #[test]
    fn brief_round_trips_through_json() {
        let brief = BriefData {
            as_of: "2026-11-20".to_owned(),
            freshness: "报价 2 小时前".to_owned(),
            total_assets: 9758.61,
            xirr: Some(0.12),
            risk_weight_pct: 70.1,
            plan: Some(PlanBrief {
                current_risk_pct: 70.1,
                target_risk_pct: 60.0,
                band_pp: 4.0,
                total_buy: 3000.0,
                total_sell: 0.0,
                has_orders: true,
            }),
            signal: Some(SignalBrief {
                effective_score: 23.0,
                effective_zone: "恐惧".to_owned(),
                confirmed: true,
            }),
            plan_blocked: None,
            decision_label: "周五".to_owned(),
            price_stale: false,
            fgi_age_days: Some(0),
            load_error: None,
        };
        let json = serde_json::to_string(&brief).unwrap();
        assert_eq!(serde_json::from_str::<BriefData>(&json).unwrap(), brief);
    }
}
