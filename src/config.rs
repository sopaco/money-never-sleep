use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub settings: Settings,
    pub allocation: Allocation,
    pub thresholds: Thresholds,
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
    /// 现金流（定投计划）
    #[serde(default)]
    pub cashflow: Cashflow,
    /// 风险档位。内部用 core_blend 把目标曲线向满仓收缩；
    /// `core_blend=0`（老配置缺字段）完全等价旧行为。
    #[serde(default)]
    pub profile: ProfileConfig,
}

/// 目标仓位曲线的过渡方式。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum WeightMode {
    /// 五档阶梯（旧行为）
    #[default]
    Step,
    /// 阈值锚点之间线性插值
    Linear,
}

/// 风险档位预设：不改变用户的曲线形状，只把曲线**向满仓收缩**的程度分档。
///
/// 目标权重 = `core_blend × 100 + (1-core_blend) × 原曲线(score)`。
/// `core_blend=0`（无档位）完全等价旧行为、择时摆幅最大；越大则底仓越高、
/// 摆幅越小。注意：档位抬高的是**常驻仓位**，不是"更稳健"——回测显示
/// 回撤最小的是无档位（曲线原样），档位带来的是更少交易与更平滑的行为。
/// 这是风险偏好旋钮，不是收益增强：主样本外仍不保证赢买入持有。
///
/// 变体命名用 `Base` 而非 `None`：避免与 `Option::None` 在 match 臂里混淆
/// （序列化 id 仍为 "none"，用户侧无感知）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RiskProfile {
    /// 无档位：曲线原样执行（core_blend=0），择时摆幅最大、常驻仓位最低
    Base,
    Steady,
    Balanced,
    Aggressive,
}

impl RiskProfile {
    pub const ALL: [RiskProfile; 4] = [Self::Base, Self::Steady, Self::Balanced, Self::Aggressive];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "raw" | "无档位" => Some(Self::Base),
            "steady" | "conservative" | "稳健" => Some(Self::Steady),
            "balanced" | "平衡" => Some(Self::Balanced),
            "aggressive" | "进取" | "激进" => Some(Self::Aggressive),
            "custom" | "自定义" => None,
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Base => "none",
            Self::Steady => "steady",
            Self::Balanced => "balanced",
            Self::Aggressive => "aggressive",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Base => "无档位",
            Self::Steady => "稳健",
            Self::Balanced => "平衡",
            Self::Aggressive => "进取",
        }
    }

    /// 曲线向满仓收缩的权重（0~1）。
    pub fn core_blend(self) -> f64 {
        match self {
            Self::Base => 0.0,
            Self::Steady => 0.4,
            Self::Balanced => 0.6,
            Self::Aggressive => 0.8,
        }
    }
}

/// 风险档位的用户可见部分：名字 + 内部收缩权重。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileConfig {
    /// steady / balanced / aggressive / custom
    #[serde(default = "ProfileConfig::default_name")]
    pub name: String,
    /// 内部参数：目标权重向满仓收缩的权重，0 = 不改动原曲线
    #[serde(default)]
    pub core_blend: f64,
}

impl ProfileConfig {
    fn default_name() -> String {
        "custom".to_string()
    }
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            name: Self::default_name(),
            core_blend: 0.0,
        }
    }
}

/// 各情绪区间下"风险资产合计"的目标权重（占总资产百分比）。
///
/// 这是仓位管理的锚：策略始终对照"我应该持有多少风险资产"，
/// 而非"把手上现金花掉百分之几"。后者会导致弹药几何衰减
/// （连续恐慌期越到后面越没钱买）与路径依赖（刚注资则买入金额被放大）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TargetWeight {
    pub extreme_fear: f64,
    pub fear: f64,
    pub neutral: f64,
    pub greed: f64,
    pub extreme_greed: f64,
    /// 档位之间是阶梯还是线性过渡。旧配置缺省为 step。
    #[serde(default)]
    pub mode: WeightMode,
}

impl TargetWeight {
    pub fn default_curve() -> Self {
        Self {
            extreme_fear: 85.0,
            fear: 75.0,
            neutral: 60.0,
            greed: 45.0,
            extreme_greed: 35.0,
            mode: WeightMode::Step,
        }
    }
}

impl Default for TargetWeight {
    fn default() -> Self {
        Self::default_curve()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Cashflow {
    /// 每月定投金额（元）。`mns plan` 的默认值。
    #[serde(default)]
    pub monthly: f64,
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
    /// 决策信号平滑窗口（天）：报告的决策分数 = 最近 N 天恐贪值均值，
    /// 而非当日值。1 = 不平滑（旧行为）。见 `src/signal.rs`。
    #[serde(default = "Rebalance::default_smooth_days")]
    pub signal_smooth_days: u32,
    /// 区间切换确认天数：平滑值需连续 N 天处于新区间才生效，
    /// 未确认时沿用旧区间（防 whipsaw）。1 = 不确认。
    #[serde(default = "Rebalance::default_zone_confirm_days")]
    pub zone_confirm_days: u32,
    /// 决策日（每周出决策版报告的日子）：0 = 每天决策；1..=7 = 周一..周日。
    /// 非决策日 `mns report` 只出观察版，不产生买卖建议。
    #[serde(default = "Rebalance::default_decision_weekday")]
    pub decision_weekday: u32,
}

impl Default for Rebalance {
    fn default() -> Self {
        Self {
            band_pp: 4.0,
            min_trade_amount: 500.0,
            min_holding_days_for_sell: 30,
            signal_smooth_days: Self::default_smooth_days(),
            zone_confirm_days: Self::default_zone_confirm_days(),
            decision_weekday: Self::default_decision_weekday(),
        }
    }
}

impl Rebalance {
    fn default_smooth_days() -> u32 {
        7
    }
    fn default_zone_confirm_days() -> u32 {
        3
    }
    /// 0 = 每天决策；5 = 每周五
    fn default_decision_weekday() -> u32 {
        5
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
    /// 年化收益达标线（%）：持仓表/报告中年化收益 ≥ 此值时绿色标记
    pub annualized_target_high: f64,
    pub min_holding_days: i64,
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
pub struct ApiConfig {
    pub fear_greed_url: String,
}

/// 在 4 个阈值锚点之间线性插值；低于首锚点取首值，高于末锚点取尾部值。
fn interp_target(score: f64, pts: &[(f64, f64); 4], tail: f64) -> f64 {
    if score <= pts[0].0 {
        return pts[0].1;
    }
    if score >= pts[3].0 {
        return tail;
    }
    for w in pts.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        if score < x1 {
            let t = if x1 > x0 { (score - x0) / (x1 - x0) } else { 0.0 };
            return y0 + t * (y1 - y0);
        }
    }
    tail
}

impl AppConfig {
    pub fn default_config() -> Self {
        // 防御配置 - 低回撤优先，逆向策略
        Self {
            settings: Settings {
                annualized_target_high: 15.0, // 年化15%在持仓表中绿色标记
                min_holding_days: 45,          // 年化收益需至少持仓这么多天才显示
                max_contrarian_weight: 2.0,    // 逆向加仓时单标的最大相对权重
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
            api: ApiConfig {
                fear_greed_url: "https://production.dataviz.cnn.io/index/fearandgreed/graphdata"
                    .to_string(),
            },
            target_weight: TargetWeight::default_curve(),
            rebalance: Rebalance::default(),
            costs: Costs::default(),
            cashflow: Cashflow::default(),
            profile: ProfileConfig::default(),
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
        if !(0.0..=1.0).contains(&self.profile.core_blend) {
            anyhow::bail!(
                "profile.core_blend 必须在 0-1 之间: {}",
                self.profile.core_blend
            );
        }
        if self.rebalance.signal_smooth_days > 60 {
            anyhow::bail!(
                "rebalance.signal_smooth_days 过大（>60 天平滑已无意义）: {}",
                self.rebalance.signal_smooth_days
            );
        }
        if self.rebalance.zone_confirm_days == 0 || self.rebalance.zone_confirm_days > 30 {
            anyhow::bail!(
                "rebalance.zone_confirm_days 应在 1-30 之间: {}",
                self.rebalance.zone_confirm_days
            );
        }
        if self.rebalance.decision_weekday > 7 {
            anyhow::bail!(
                "rebalance.decision_weekday 应为 0(每天) 或 1-7(周一..周日): {}",
                self.rebalance.decision_weekday
            );
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
        let step = if score < self.thresholds.extreme_fear {
            t.extreme_fear
        } else if score < self.thresholds.fear {
            t.fear
        } else if score < self.thresholds.neutral {
            t.neutral
        } else if score < self.thresholds.greed {
            t.greed
        } else {
            t.extreme_greed
        };
        let base = match t.mode {
            WeightMode::Step => step,
            WeightMode::Linear => {
                // 锚点取在阈值上：阈值以下沿用该档权重，阈值之间线性过渡，
                // 两端截断。这样极端区间语义不变，只消除阈值处的跳变。
                let pts = [
                    (self.thresholds.extreme_fear, t.extreme_fear),
                    (self.thresholds.fear, t.fear),
                    (self.thresholds.neutral, t.neutral),
                    (self.thresholds.greed, t.greed),
                ];
                interp_target(score, &pts, t.extreme_greed)
            }
        };
        // 风险档位：把曲线向满仓收缩（core_blend=0 时完全等价旧行为）
        let blend = self.profile.core_blend.clamp(0.0, 1.0);
        blend * 100.0 + (1.0 - blend) * base
    }

    /// 一键套用风险档位：只调整曲线向满仓的收缩程度，不改用户自己的曲线锚点、
    /// 配置比例与成本。
    pub fn apply_profile(&mut self, profile: RiskProfile) {
        self.profile = ProfileConfig {
            name: profile.id().to_string(),
            core_blend: profile.core_blend(),
        };
    }

    /// 报告展示用档位名。
    pub fn profile_label(&self) -> &'static str {
        RiskProfile::ALL
            .iter()
            .find(|p| p.id() == self.profile.name)
            .map(|p| p.label())
            .unwrap_or("自定义")
    }

    /// “中性”区间的实际目标仓位（已含档位收缩），用于展示。
    pub fn effective_neutral_weight(&self) -> f64 {
        let mid = (self.thresholds.fear + self.thresholds.neutral) / 2.0;
        self.target_weight_for(mid)
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

    /// 用 dot path 获取/设置配置值
    pub fn get_value(&self, key: &str) -> Option<String> {
        match key {
            "settings.annualized_target_high" => {
                Some(self.settings.annualized_target_high.to_string())
            }
            "settings.min_holding_days" => Some(self.settings.min_holding_days.to_string()),
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
            "rebalance.signal_smooth_days" => Some(self.rebalance.signal_smooth_days.to_string()),
            "rebalance.zone_confirm_days" => Some(self.rebalance.zone_confirm_days.to_string()),
            "rebalance.decision_weekday" => Some(self.rebalance.decision_weekday.to_string()),
            "costs.buy_fee_pct" => Some(self.costs.buy_fee_pct.to_string()),
            "costs.sell_fee_pct" => Some(self.costs.sell_fee_pct.to_string()),
            "costs.cash_annual_yield" => Some(self.costs.cash_annual_yield.to_string()),
            "target_weight.mode" => Some(match self.target_weight.mode {
                WeightMode::Step => "step".to_string(),
                WeightMode::Linear => "linear".to_string(),
            }),
            "cashflow.monthly" => Some(self.cashflow.monthly.to_string()),
            "profile" => Some(self.profile_label().to_string()),
            "profile.core_blend" => Some(self.profile.core_blend.to_string()),
            _ => None,
        }
    }

    pub fn set_value(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "settings.annualized_target_high" => {
                self.settings.annualized_target_high = value.parse()?
            }
            "settings.min_holding_days" => self.settings.min_holding_days = value.parse()?,
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
            "api.fear_greed_url" => self.api.fear_greed_url = value.to_string(),
            "target_weight.extreme_fear" => self.target_weight.extreme_fear = value.parse()?,
            "target_weight.fear" => self.target_weight.fear = value.parse()?,
            "target_weight.neutral" => self.target_weight.neutral = value.parse()?,
            "target_weight.greed" => self.target_weight.greed = value.parse()?,
            "target_weight.extreme_greed" => self.target_weight.extreme_greed = value.parse()?,
            "rebalance.band_pp" => self.rebalance.band_pp = value.parse()?,
            "rebalance.min_trade_amount" => self.rebalance.min_trade_amount = value.parse()?,
            "rebalance.min_holding_days_for_sell" => {
                self.rebalance.min_holding_days_for_sell = value.parse()?;
            }
            "rebalance.signal_smooth_days" => self.rebalance.signal_smooth_days = value.parse()?,
            "rebalance.zone_confirm_days" => self.rebalance.zone_confirm_days = value.parse()?,
            "rebalance.decision_weekday" => self.rebalance.decision_weekday = value.parse()?,
            "costs.buy_fee_pct" => self.costs.buy_fee_pct = value.parse()?,
            "costs.sell_fee_pct" => self.costs.sell_fee_pct = value.parse()?,
            "costs.cash_annual_yield" => self.costs.cash_annual_yield = value.parse()?,
            "cashflow.monthly" => self.cashflow.monthly = value.parse()?,
            "target_weight.mode" => {
                self.target_weight.mode = match value.trim().to_ascii_lowercase().as_str() {
                    "step" | "阶梯" => WeightMode::Step,
                    "linear" | "线性" => WeightMode::Linear,
                    other => anyhow::bail!("未知的过渡方式: {}（可选 step/linear）", other),
                };
                // 手工改曲线参数后档位标签不再可信；mode 不影响数值，保留 profile。
            }
            "profile" => {
                let v = value.trim();
                if v.eq_ignore_ascii_case("custom") || v == "自定义" {
                    self.profile = ProfileConfig::default();
                } else {
                    let p = RiskProfile::parse(v).ok_or_else(|| {
                        anyhow::anyhow!(
                            "未知风险档位: {}（可选 none/steady/balanced/aggressive/custom 或 无档位/稳健/平衡/进取）",
                            value
                        )
                    })?;
                    self.apply_profile(p);
                }
            }
            "profile.core_blend" => {
                self.profile.core_blend = value.parse()?;
                self.profile.name = "custom".to_string();
            }
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
    fn 风险档位可解析中英文别名() {
        assert_eq!(RiskProfile::parse("steady"), Some(RiskProfile::Steady));
        assert_eq!(RiskProfile::parse("平衡"), Some(RiskProfile::Balanced));
        assert_eq!(RiskProfile::parse("进取"), Some(RiskProfile::Aggressive));
        assert_eq!(RiskProfile::parse("aggressive"), Some(RiskProfile::Aggressive));
        assert_eq!(RiskProfile::parse("none"), Some(RiskProfile::Base));
        assert_eq!(RiskProfile::parse("无档位"), Some(RiskProfile::Base));
        assert_eq!(RiskProfile::parse("raw"), Some(RiskProfile::Base));
        assert_eq!(RiskProfile::parse("unknown"), None);
    }

    #[test]
    fn 无档位保持曲线原样且摆幅最大() {
        let mut c = cfg();
        c.apply_profile(RiskProfile::Base);
        assert_eq!(c.profile.name, "none");
        assert_eq!(c.profile_label(), "无档位");
        assert!(c.profile.core_blend.abs() < 1e-12);
        // 曲线原样：中性锚 = target_weight.neutral，而非向满仓收缩后的值
        assert!((c.target_weight_for(50.0) - c.target_weight.neutral).abs() < 1e-9);
        // 摆幅 = 极端区间权重差，无档位时最大
        let swing =
            c.target_weight_for(0.0) - c.target_weight_for(100.0);
        assert!((swing - (85.0 - 35.0)).abs() < 1e-9, "无档位摆幅应为曲线原样: {}", swing);
        assert!(c.validate().is_ok());
    }

    #[test]
    fn 套用档位只改收缩权重不改用户曲线() {
        let mut c = cfg();
        let base = c.target_weight.clone();
        for p in RiskProfile::ALL {
            c.apply_profile(p);
            assert_eq!(c.profile_label(), p.label());
            assert_eq!(c.profile.name, p.id());
            assert_eq!(c.profile.core_blend, p.core_blend());
            assert_eq!(c.target_weight, base, "档位不应改写用户的曲线锚点");
            assert!(c.validate().is_ok());
        }
    }

    #[test]
    fn 自定义档位显示自定义且不改曲线() {
        let mut c = cfg();
        c.apply_profile(RiskProfile::Balanced);
        let before = c.target_weight_for(50.0);
        c.set_value("profile", "custom").unwrap();
        assert_eq!(c.profile_label(), "自定义");
        assert_eq!(c.profile.core_blend, 0.0);
        assert!(c.target_weight_for(50.0) < before, "取消档位后应回到原始曲线");
    }

    #[test]
    fn config命令可设置无档位预设() {
        let mut c = cfg();
        c.apply_profile(RiskProfile::Steady);
        let before = c.target_weight_for(50.0);
        c.set_value("profile", "none").unwrap();
        assert_eq!(c.profile.name, "none");
        assert_eq!(c.profile_label(), "无档位");
        assert!(c.profile.core_blend.abs() < 1e-12);
        assert!(
            c.target_weight_for(50.0) < before,
            "无档位应回到曲线原样（更低的中性仓位）"
        );
        // 中文别名同样可达
        c.set_value("profile", "无档位").unwrap();
        assert_eq!(c.profile.name, "none");
        // get_value 应回显档位标签
        assert_eq!(c.get_value("profile").unwrap(), "无档位");
    }

    #[test]
    fn 档位把曲线向满仓收缩() {
        let mut c = cfg();
        c.apply_profile(RiskProfile::Balanced); // core 0.6
        // 默认曲线中性 60 → 60 + 0.6*(100-60) = 84
        assert!((c.target_weight_for(50.0) - 84.0).abs() < 1e-9);
        // 极端贪婪 35 → 35 + 0.6*65 = 74
        assert!((c.target_weight_for(90.0) - 74.0).abs() < 1e-9);
    }

    #[test]
    fn 线性插值在阈值两侧连续() {
        let mut c = cfg();
        c.apply_profile(RiskProfile::Balanced);
        c.target_weight.mode = WeightMode::Linear;
        // 阈值 45 两侧
        let lo = c.target_weight_for(44.9);
        let hi = c.target_weight_for(45.1);
        assert!((lo - hi).abs() < 0.5, "阈值处应连续: {} vs {}", lo, hi);
        // 两端截断后的档位值（已含 core 0.6 收缩）
        assert!((c.target_weight_for(10.0) - 94.0).abs() < 1e-9);
        assert!((c.target_weight_for(90.0) - 74.0).abs() < 1e-9);
        // 中段落在恐慌/中性两档之间
        let mid = c.target_weight_for(50.0);
        assert!(mid < 90.0 && mid > 84.0, "50 分应落在恐慌/中性之间: {}", mid);
    }

    #[test]
    fn 线性插值在39_9与40_1之间连续() {
        let mut c = cfg();
        c.thresholds.extreme_fear = 40.0;
        c.thresholds.fear = 50.0;
        c.apply_profile(RiskProfile::Balanced);
        c.target_weight.mode = WeightMode::Linear;
        let lo = c.target_weight_for(39.9);
        let hi = c.target_weight_for(40.1);
        assert!((lo - hi).abs() < 0.5, "39.9 vs 40.1 应连续: {} vs {}", lo, hi);
    }

    #[test]
    fn 旧格式配置仍可加载_缺失字段回落默认值() {
        // 含已删除的旧框架键（buy_ratio/sell_ratio/annualized_target_low 等）：
        // serde 忽略未知字段，老配置文件无需迁移即可继续使用
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
        assert_eq!(c.rebalance.signal_smooth_days, 7, "旧配置回落到默认平滑窗口");
        assert_eq!(c.rebalance.zone_confirm_days, 3, "旧配置回落到默认确认天数");
        assert_eq!(c.rebalance.decision_weekday, 5, "旧配置回落到默认决策日(周五)");
        assert_eq!(c.target_weight.mode, WeightMode::Step, "旧配置回落到阶梯模式");
        assert!(c.profile.core_blend.abs() < 1e-9, "旧配置缺字段应等价旧行为");
        assert_eq!(c.profile.name, "custom");
        assert_eq!(c.profile_label(), "自定义");
    }
}
