use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub settings: Settings,
    pub allocation: Allocation,
    pub thresholds: Thresholds,
    pub buy_ratio: BuyRatio,
    pub sell_ratio: SellRatio,
    pub api: ApiConfig,
    /// 风险资产目标总权重曲线（目标仓位框架）
    #[serde(default = "TargetWeight::default_curve")]
    pub target_weight: TargetWeight,
    /// 再平衡带宽与交易门槛
    #[serde(default)]
    pub rebalance: Rebalance,
    /// 交易成本与现金收益
    #[serde(default)]
    pub costs: Costs,
}

/// 各情绪区间下"风险资产合计"的目标权重（占总资产百分比）。
///
/// 这是仓位管理的锚：策略始终对照"我应该持有多少风险资产"，
/// 而非旧逻辑的"把手上现金花掉百分之几"。后者会导致弹药几何衰减
/// （连续恐慌期越到后面越没钱买）与路径依赖（刚注资则买入金额被放大）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetWeight {
    pub extreme_fear: f64,
    pub fear: f64,
    pub neutral: f64,
    pub greed: f64,
    pub extreme_greed: f64,
}

impl TargetWeight {
    pub fn default_curve() -> Self {
        Self {
            extreme_fear: 85.0,
            fear: 75.0,
            neutral: 60.0,
            greed: 45.0,
            extreme_greed: 35.0,
        }
    }
}

impl Default for TargetWeight {
    fn default() -> Self {
        Self::default_curve()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rebalance {
    /// 偏离带（百分点）。实际权重与目标偏离不超过该值则不动作，
    /// 这是把交易频率压到"每月2-3次或更低"的主要机制。
    pub band_pp: f64,
    /// 单笔最小交易金额，避免产生无意义碎单
    pub min_trade_amount: f64,
    /// 卖出最短持有天数：早于此天数不建议卖出，用于规避惩罚性赎回费
    pub min_holding_days_for_sell: i64,
}

impl Default for Rebalance {
    fn default() -> Self {
        Self {
            band_pp: 4.0,
            min_trade_amount: 500.0,
            min_holding_days_for_sell: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Costs {
    /// 买入成本（申购费/佣金，%）
    pub buy_fee_pct: f64,
    /// 卖出固定成本（佣金/过户费，%），与阶梯赎回费叠加
    pub sell_fee_pct: f64,
    /// 闲置现金年化收益（货币基金）。旧回测假设为 0，系统性低估持有现金的表现。
    pub cash_annual_yield: f64,
    /// 阶梯赎回费：(持有天数上界, 费率%)，取第一个满足 days < 上界 的档位
    pub redemption_tiers: Vec<(i64, f64)>,
}

impl Default for Costs {
    fn default() -> Self {
        Self {
            buy_fee_pct: 0.12,
            sell_fee_pct: 0.05,
            cash_annual_yield: 2.0,
            // 国内基金常见阶梯：<7天惩罚性1.5%，<30天0.75%，<365天0.5%，<730天0.25%
            redemption_tiers: vec![(7, 1.5), (30, 0.75), (365, 0.5), (730, 0.25)],
        }
    }
}

impl Costs {
    /// 按持有天数返回赎回费率（小数，非百分数）
    pub fn redemption_rate(&self, holding_days: i64) -> f64 {
        for (limit, rate) in &self.redemption_tiers {
            if holding_days < *limit {
                return rate / 100.0;
            }
        }
        0.0
    }

    /// 卖出总费率（小数）＝ 固定卖出成本 + 阶梯赎回费
    pub fn total_sell_rate(&self, holding_days: i64) -> f64 {
        self.sell_fee_pct / 100.0 + self.redemption_rate(holding_days)
    }

    /// 买入费率（小数）
    pub fn buy_rate(&self) -> f64 {
        self.buy_fee_pct / 100.0
    }

    /// 现金在 `days` 天内的增长因子
    pub fn cash_growth(&self, days: i64) -> f64 {
        if days <= 0 {
            return 1.0;
        }
        (1.0 + self.cash_annual_yield / 100.0).powf(days as f64 / 365.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub annualized_target_low: f64,
    pub annualized_target_high: f64,
    pub min_holding_days: i64,
    pub min_absolute_profit_days: i64,
    pub max_contrarian_weight: f64,
    pub report_output_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Allocation {
    pub us_stocks: f64,
    pub cn_stocks: f64,
    pub counter_cyclical: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thresholds {
    pub extreme_fear: f64,
    pub fear: f64,
    pub neutral: f64,
    pub greed: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuyRatio {
    pub extreme_fear: f64,
    pub fear: f64,
    pub neutral: f64,
    pub greed: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SellRatio {
    pub extreme_greed_target_high: f64,
    pub extreme_greed_target_low: f64,
    pub extreme_greed_below_target: f64,
    pub greed_target_high: f64,
    pub greed_target_low: f64,
    pub neutral_target_high: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub fear_greed_url: String,
}

impl AppConfig {
    pub fn default_config() -> Self {
        // 防御配置 - 低回撤优先，逆向策略
        Self {
            settings: Settings {
                annualized_target_low: 10.0,   // 降低止盈门槛，更早锁定收益
                annualized_target_high: 15.0,  // 年化15%大笔减仓
                min_holding_days: 45,          // 更长持仓，避免短期波动
                min_absolute_profit_days: 120, // 绝对收益持仓天数更长
                max_contrarian_weight: 2.0,    // 降低逆势加仓权重
                report_output_dir: "./reports".to_string(),
            },
            allocation: Allocation {
                us_stocks: 55.0,        // 降低美股占比
                cn_stocks: 25.0,        // 提高红利低波稳健配置
                counter_cyclical: 20.0, // 提高黄金对冲比例
            },
            thresholds: Thresholds {
                extreme_fear: 30.0, // 更严格的极度恐慌阈值
                fear: 45.0,         // 恐慌阈值
                neutral: 55.0,      // 中性阈值
                greed: 70.0,        // 更早触发贪婪卖出
            },
            buy_ratio: BuyRatio {
                extreme_fear: 60.0, // 极度恐慌适度买入
                fear: 35.0,         // 恐慌保守买入
                neutral: 0.0,       // 中性不买
                greed: 0.0,         // 贪婪不买
            },
            sell_ratio: SellRatio {
                extreme_greed_target_high: 50.0,  // 极度贪婪减仓50%
                extreme_greed_target_low: 30.0,   // 极度贪婪低收益减仓30%
                extreme_greed_below_target: 20.0, // 极度贪婪未达标减仓20%
                greed_target_high: 40.0,          // 贪婪减仓40%
                greed_target_low: 25.0,           // 贪婪低收益减仓25%
                neutral_target_high: 15.0,        // 中性减仓15%
            },
            api: ApiConfig {
                fear_greed_url: "https://production.dataviz.cnn.io/index/fearandgreed/graphdata"
                    .to_string(),
            },
            target_weight: TargetWeight::default_curve(),
            rebalance: Rebalance::default(),
            costs: Costs::default(),
        }
    }

    pub fn config_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("无法获取用户主目录")?;
        Ok(home.join(".mns"))
    }

    pub fn config_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.toml"))
    }

    pub fn db_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("mns.db"))
    }

    pub fn load() -> Result<Self> {
        Self::load_from_path(&Self::config_path()?)
    }

    /// 从指定路径加载配置
    pub fn load_from_path<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let content = fs::read_to_string(path)
            .with_context(|| format!("读取配置文件失败: {}", path.display()))?;
        let config: AppConfig = toml::from_str(&content).with_context(|| "解析配置文件失败")?;
        config.validate()?;
        Ok(config)
    }

    /// 校验配置合法性
    pub fn validate(&self) -> Result<()> {
        let alloc_sum = self.allocation.us_stocks
            + self.allocation.cn_stocks
            + self.allocation.counter_cyclical;
        if (alloc_sum - 100.0).abs() > 0.01 {
            anyhow::bail!(
                "资产配置比例之和必须为 100%，当前: 美股{}% + A股{}% + 逆周期{}% = {}%",
                self.allocation.us_stocks,
                self.allocation.cn_stocks,
                self.allocation.counter_cyclical,
                alloc_sum
            );
        }
        if self.settings.min_holding_days < 0 {
            anyhow::bail!("最小持仓天数不能为负数: {}", self.settings.min_holding_days);
        }
        if self.settings.max_contrarian_weight < 1.0 {
            anyhow::bail!(
                "最大逆向权重不能小于 1.0: {}",
                self.settings.max_contrarian_weight
            );
        }
        let t = &self.thresholds;
        if !(t.extreme_fear < t.fear && t.fear < t.neutral && t.neutral < t.greed) {
            anyhow::bail!(
                "情绪阈值必须满足单调递增: extreme_fear({}) < fear({}) < neutral({}) < greed({})",
                t.extreme_fear,
                t.fear,
                t.neutral,
                t.greed
            );
        }
        let w = &self.target_weight;
        for (name, v) in [
            ("extreme_fear", w.extreme_fear),
            ("fear", w.fear),
            ("neutral", w.neutral),
            ("greed", w.greed),
            ("extreme_greed", w.extreme_greed),
        ] {
            if !(0.0..=100.0).contains(&v) {
                anyhow::bail!("target_weight.{} 必须在 0-100 之间: {}", name, v);
            }
        }
        // 逆向策略的核心约束：越恐慌目标仓位越高
        if !(w.extreme_fear >= w.fear
            && w.fear >= w.neutral
            && w.neutral >= w.greed
            && w.greed >= w.extreme_greed)
        {
            anyhow::bail!(
                "target_weight 必须随情绪升高而单调不增（逆向策略）: 极度恐慌{} ≥ 恐慌{} ≥ 中性{} ≥ 贪婪{} ≥ 极度贪婪{}",
                w.extreme_fear, w.fear, w.neutral, w.greed, w.extreme_greed
            );
        }
        if self.rebalance.band_pp < 0.0 {
            anyhow::bail!("rebalance.band_pp 不能为负: {}", self.rebalance.band_pp);
        }
        if self.costs.cash_annual_yield < 0.0 {
            anyhow::bail!(
                "costs.cash_annual_yield 不能为负: {}",
                self.costs.cash_annual_yield
            );
        }
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        let dir = Self::config_dir()?;
        fs::create_dir_all(&dir)?;
        let path = Self::config_path()?;
        let content = toml::to_string_pretty(self)?;
        fs::write(&path, content)?;
        Ok(())
    }

    /// 根据恐贪指数判断情绪区间
    pub fn sentiment_zone(&self, score: f64) -> &'static str {
        if score < self.thresholds.extreme_fear {
            "极度恐慌"
        } else if score < self.thresholds.fear {
            "恐慌"
        } else if score < self.thresholds.neutral {
            "中性"
        } else if score < self.thresholds.greed {
            "贪婪"
        } else {
            "极度贪婪"
        }
    }

    /// 风险资产目标总权重（%），由情绪区间决定。
    ///
    /// 与 `buy_ratio_for` 的本质区别：返回的是"应该持有多少"，
    /// 而非"应该花掉手上现金的多少"，因此无路径依赖、无弹药衰减。
    pub fn target_weight_for(&self, score: f64) -> f64 {
        let t = &self.target_weight;
        if score < self.thresholds.extreme_fear {
            t.extreme_fear
        } else if score < self.thresholds.fear {
            t.fear
        } else if score < self.thresholds.neutral {
            t.neutral
        } else if score < self.thresholds.greed {
            t.greed
        } else {
            t.extreme_greed
        }
    }

    /// 风险资产内部的三腿目标权重（归一化后占风险资产的比例）
    pub fn sleeve_split(&self) -> (f64, f64, f64) {
        let a = &self.allocation;
        let sum = a.us_stocks + a.cn_stocks + a.counter_cyclical;
        if sum <= 0.0 {
            return (0.0, 0.0, 0.0);
        }
        (
            a.us_stocks / sum,
            a.cn_stocks / sum,
            a.counter_cyclical / sum,
        )
    }

    /// 各腿占**总资产**的目标权重（小数）
    pub fn asset_target_weights(&self, score: f64) -> (f64, f64, f64) {
        let risk = self.target_weight_for(score) / 100.0;
        let (us, cn, cc) = self.sleeve_split();
        (risk * us, risk * cn, risk * cc)
    }

    /// 根据情绪区间获取买入比例（旧框架，保留用于对照回测）
    pub fn buy_ratio_for(&self, score: f64) -> f64 {
        if score < self.thresholds.extreme_fear {
            self.buy_ratio.extreme_fear
        } else if score < self.thresholds.fear {
            self.buy_ratio.fear
        } else if score < self.thresholds.neutral {
            self.buy_ratio.neutral
        } else if score < self.thresholds.greed {
            self.buy_ratio.greed
        } else {
            0.0 // 极度贪婪时暂停买入
        }
    }

    /// 根据情绪区间和年化收益获取卖出减仓比例
    pub fn sell_ratio_for(&self, score: f64, annualized: f64) -> f64 {
        if score >= self.thresholds.greed {
            // 极度贪婪
            if annualized >= self.settings.annualized_target_high {
                self.sell_ratio.extreme_greed_target_high
            } else if annualized >= self.settings.annualized_target_low {
                self.sell_ratio.extreme_greed_target_low
            } else {
                self.sell_ratio.extreme_greed_below_target
            }
        } else if score >= self.thresholds.neutral {
            // 贪婪
            if annualized >= self.settings.annualized_target_high {
                self.sell_ratio.greed_target_high
            } else if annualized >= self.settings.annualized_target_low {
                self.sell_ratio.greed_target_low
            } else {
                0.0
            }
        } else if score >= self.thresholds.fear {
            // 中性
            if annualized >= self.settings.annualized_target_high {
                self.sell_ratio.neutral_target_high
            } else {
                0.0
            }
        } else {
            0.0
        }
    }

    /// 用 dot path 获取/设置配置值
    pub fn get_value(&self, key: &str) -> Option<String> {
        match key {
            "settings.annualized_target_low" => {
                Some(self.settings.annualized_target_low.to_string())
            }
            "settings.annualized_target_high" => {
                Some(self.settings.annualized_target_high.to_string())
            }
            "settings.min_holding_days" => Some(self.settings.min_holding_days.to_string()),
            "settings.min_absolute_profit_days" => {
                Some(self.settings.min_absolute_profit_days.to_string())
            }
            "settings.max_contrarian_weight" => {
                Some(self.settings.max_contrarian_weight.to_string())
            }
            "settings.report_output_dir" => Some(self.settings.report_output_dir.clone()),
            "allocation.us_stocks" => Some(self.allocation.us_stocks.to_string()),
            "allocation.cn_stocks" => Some(self.allocation.cn_stocks.to_string()),
            "allocation.counter_cyclical" => Some(self.allocation.counter_cyclical.to_string()),
            "thresholds.extreme_fear" => Some(self.thresholds.extreme_fear.to_string()),
            "thresholds.fear" => Some(self.thresholds.fear.to_string()),
            "thresholds.neutral" => Some(self.thresholds.neutral.to_string()),
            "thresholds.greed" => Some(self.thresholds.greed.to_string()),
            "buy_ratio.extreme_fear" => Some(self.buy_ratio.extreme_fear.to_string()),
            "buy_ratio.fear" => Some(self.buy_ratio.fear.to_string()),
            "buy_ratio.neutral" => Some(self.buy_ratio.neutral.to_string()),
            "buy_ratio.greed" => Some(self.buy_ratio.greed.to_string()),
            "sell_ratio.extreme_greed_target_high" => {
                Some(self.sell_ratio.extreme_greed_target_high.to_string())
            }
            "sell_ratio.extreme_greed_target_low" => {
                Some(self.sell_ratio.extreme_greed_target_low.to_string())
            }
            "sell_ratio.extreme_greed_below_target" => {
                Some(self.sell_ratio.extreme_greed_below_target.to_string())
            }
            "sell_ratio.greed_target_high" => Some(self.sell_ratio.greed_target_high.to_string()),
            "sell_ratio.greed_target_low" => Some(self.sell_ratio.greed_target_low.to_string()),
            "sell_ratio.neutral_target_high" => {
                Some(self.sell_ratio.neutral_target_high.to_string())
            }
            "api.fear_greed_url" => Some(self.api.fear_greed_url.clone()),
            "target_weight.extreme_fear" => Some(self.target_weight.extreme_fear.to_string()),
            "target_weight.fear" => Some(self.target_weight.fear.to_string()),
            "target_weight.neutral" => Some(self.target_weight.neutral.to_string()),
            "target_weight.greed" => Some(self.target_weight.greed.to_string()),
            "target_weight.extreme_greed" => Some(self.target_weight.extreme_greed.to_string()),
            "rebalance.band_pp" => Some(self.rebalance.band_pp.to_string()),
            "rebalance.min_trade_amount" => Some(self.rebalance.min_trade_amount.to_string()),
            "rebalance.min_holding_days_for_sell" => {
                Some(self.rebalance.min_holding_days_for_sell.to_string())
            }
            "costs.buy_fee_pct" => Some(self.costs.buy_fee_pct.to_string()),
            "costs.sell_fee_pct" => Some(self.costs.sell_fee_pct.to_string()),
            "costs.cash_annual_yield" => Some(self.costs.cash_annual_yield.to_string()),
            _ => None,
        }
    }

    pub fn set_value(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "settings.annualized_target_low" => {
                self.settings.annualized_target_low = value.parse()?
            }
            "settings.annualized_target_high" => {
                self.settings.annualized_target_high = value.parse()?
            }
            "settings.min_holding_days" => self.settings.min_holding_days = value.parse()?,
            "settings.min_absolute_profit_days" => {
                self.settings.min_absolute_profit_days = value.parse()?
            }
            "settings.max_contrarian_weight" => {
                self.settings.max_contrarian_weight = value.parse()?
            }
            "settings.report_output_dir" => self.settings.report_output_dir = value.to_string(),
            "allocation.us_stocks" => self.allocation.us_stocks = value.parse()?,
            "allocation.cn_stocks" => self.allocation.cn_stocks = value.parse()?,
            "allocation.counter_cyclical" => self.allocation.counter_cyclical = value.parse()?,
            "thresholds.extreme_fear" => self.thresholds.extreme_fear = value.parse()?,
            "thresholds.fear" => self.thresholds.fear = value.parse()?,
            "thresholds.neutral" => self.thresholds.neutral = value.parse()?,
            "thresholds.greed" => self.thresholds.greed = value.parse()?,
            "buy_ratio.extreme_fear" => self.buy_ratio.extreme_fear = value.parse()?,
            "buy_ratio.fear" => self.buy_ratio.fear = value.parse()?,
            "buy_ratio.neutral" => self.buy_ratio.neutral = value.parse()?,
            "buy_ratio.greed" => self.buy_ratio.greed = value.parse()?,
            "sell_ratio.extreme_greed_target_high" => {
                self.sell_ratio.extreme_greed_target_high = value.parse()?
            }
            "sell_ratio.extreme_greed_target_low" => {
                self.sell_ratio.extreme_greed_target_low = value.parse()?
            }
            "sell_ratio.extreme_greed_below_target" => {
                self.sell_ratio.extreme_greed_below_target = value.parse()?
            }
            "sell_ratio.greed_target_high" => self.sell_ratio.greed_target_high = value.parse()?,
            "sell_ratio.greed_target_low" => self.sell_ratio.greed_target_low = value.parse()?,
            "sell_ratio.neutral_target_high" => {
                self.sell_ratio.neutral_target_high = value.parse()?
            }
            "api.fear_greed_url" => self.api.fear_greed_url = value.to_string(),
            "target_weight.extreme_fear" => self.target_weight.extreme_fear = value.parse()?,
            "target_weight.fear" => self.target_weight.fear = value.parse()?,
            "target_weight.neutral" => self.target_weight.neutral = value.parse()?,
            "target_weight.greed" => self.target_weight.greed = value.parse()?,
            "target_weight.extreme_greed" => self.target_weight.extreme_greed = value.parse()?,
            "rebalance.band_pp" => self.rebalance.band_pp = value.parse()?,
            "rebalance.min_trade_amount" => self.rebalance.min_trade_amount = value.parse()?,
            "rebalance.min_holding_days_for_sell" => {
                self.rebalance.min_holding_days_for_sell = value.parse()?
            }
            "costs.buy_fee_pct" => self.costs.buy_fee_pct = value.parse()?,
            "costs.sell_fee_pct" => self.costs.sell_fee_pct = value.parse()?,
            "costs.cash_annual_yield" => self.costs.cash_annual_yield = value.parse()?,
            _ => anyhow::bail!("未知的配置项: {}", key),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AppConfig {
        AppConfig::default_config()
    }

    #[test]
    fn 默认配置应通过校验() {
        assert!(cfg().validate().is_ok());
    }

    #[test]
    fn 阈值非单调应被拒绝() {
        let mut c = cfg();
        c.thresholds.fear = 80.0; // > neutral(55)
        assert!(c.validate().is_err());
    }

    #[test]
    fn 目标权重必须随情绪升高单调不增() {
        let mut c = cfg();
        c.target_weight.greed = 95.0; // 贪婪时反而比中性重仓
        assert!(c.validate().is_err());
    }

    #[test]
    fn 资产配置比例之和必须为百分之百() {
        let mut c = cfg();
        c.allocation.us_stocks = 60.0; // 60+25+20 = 105
        assert!(c.validate().is_err());
    }

    #[test]
    fn 目标权重曲线随情绪单调下降() {
        let c = cfg();
        let w: Vec<f64> = [10.0, 35.0, 50.0, 60.0, 80.0]
            .iter()
            .map(|s| c.target_weight_for(*s))
            .collect();
        assert_eq!(w, vec![85.0, 75.0, 60.0, 45.0, 35.0]);
        for pair in w.windows(2) {
            assert!(pair[0] >= pair[1], "目标权重应单调不增: {:?}", w);
        }
    }

    #[test]
    fn 三腿目标权重之和等于风险资产总权重() {
        let c = cfg();
        let (us, cn, cc) = c.asset_target_weights(20.0); // 极度恐慌 85%
        assert!((us + cn + cc - 0.85).abs() < 1e-9);
        // 内部比例应保持 55:25:20
        assert!((us / (us + cn + cc) - 0.55).abs() < 1e-9);
    }

    #[test]
    fn 阶梯赎回费按持有天数取档() {
        let c = cfg();
        assert!((c.costs.redemption_rate(3) - 0.015).abs() < 1e-9, "<7天 1.5%");
        assert!((c.costs.redemption_rate(20) - 0.0075).abs() < 1e-9, "<30天 0.75%");
        assert!((c.costs.redemption_rate(200) - 0.005).abs() < 1e-9, "<365天 0.5%");
        assert!((c.costs.redemption_rate(500) - 0.0025).abs() < 1e-9, "<730天 0.25%");
        assert_eq!(c.costs.redemption_rate(1000), 0.0, "长期持有免赎回费");
    }

    #[test]
    fn 卖出总费率叠加固定成本与赎回费() {
        let c = cfg();
        let expect = 0.05 / 100.0 + 0.015;
        assert!((c.costs.total_sell_rate(3) - expect).abs() < 1e-12);
    }

    #[test]
    fn 现金按年化收益增长() {
        let c = cfg();
        assert_eq!(c.costs.cash_growth(0), 1.0);
        let one_year = c.costs.cash_growth(365);
        assert!((one_year - 1.02).abs() < 1e-9, "2%年化: {}", one_year);
    }

    #[test]
    fn 旧配置文件缺少新字段仍可加载() {
        // 用户既有 config.toml 不含 target_weight/rebalance/costs
        let old = r#"
[settings]
annualized_target_low = 10.0
annualized_target_high = 15.0
min_holding_days = 45
min_absolute_profit_days = 120
max_contrarian_weight = 2.0
report_output_dir = "./reports"
[allocation]
us_stocks = 55.0
cn_stocks = 25.0
counter_cyclical = 20.0
[thresholds]
extreme_fear = 30.0
fear = 45.0
neutral = 55.0
greed = 70.0
[buy_ratio]
extreme_fear = 60.0
fear = 35.0
neutral = 0.0
greed = 0.0
[sell_ratio]
extreme_greed_target_high = 50.0
extreme_greed_target_low = 30.0
extreme_greed_below_target = 20.0
greed_target_high = 40.0
greed_target_low = 25.0
neutral_target_high = 15.0
[api]
fear_greed_url = "https://example.com"
"#;
        let c: AppConfig = toml::from_str(old).expect("旧配置应能解析");
        assert!(c.validate().is_ok());
        assert_eq!(c.target_weight.neutral, 60.0, "应回落到默认曲线");
        assert_eq!(c.costs.cash_annual_yield, 2.0);
    }
}
