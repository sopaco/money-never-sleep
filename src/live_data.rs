//! 实盘日频数据集（point-in-time）。
//!
//! 与 `backtest.rs` 内置的 2016–2025 月频**代理**序列严格区分：
//! 那里的三腿是 270042 / 510880 / 518880，与本模块用户实际持有的四只场外基金
//! 在指数、通道、时间覆盖上均不同。代理序列只保留作长周期参考，不得用于实盘结论。
//!
//! 数据集由 `.agents/skills/mns-backtest/data/build_live_dataset.py` 生成，
//! 每一行只包含当日收盘前真正可知的信息：
//! - FGI 取**严格早于**该日的最后一条已公布值（CNN 美股收盘后计算，北京次日才可得）
//! - 净值取不晚于该日的最后一条已披露值（QDII T+1/T+2 披露，缺口沿用前值）
//! 因此回测不可能"用当天情绪按当天净值同刻成交"，从数据层消除前视偏差。

use crate::config::AppConfig;
use anyhow::{Context, Result, bail};
use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 实盘持仓基金代码，顺序与 CSV 列一致
pub const CODES: [&str; 4] = ["018966", "539001", "007605", "020341"];

/// 各基金所属的三腿：0=纳指QDII，1=红利低波，2=黄金
pub const LEG_OF: [usize; 4] = [0, 0, 1, 2];

/// 腿内权重（纳指腿由两只 QDII 各半合成，来自实盘持仓市值比）
pub const LEG_IN_WEIGHT: [f64; 4] = [0.5, 0.5, 1.0, 1.0];

pub const LEG_NAMES: [&str; 3] = ["纳指QDII(018966+539001)", "300红利低波(007605)", "黄金(020341)"];

/// 通道映射由数据集类型 + 腿序号决定（见 `audit::channel`），此处不再重复定义。

#[derive(Debug, Clone)]
pub struct Day {
    pub date: NaiveDate,
    /// None / -1 表示该日无可用已公布 FGI（CNN 仅提供约一年历史）
    pub fgi: Option<f64>,
    pub fgi_age_days: i32,
    /// 四只基金的全收益净值指数（回测用）
    pub nav: [f64; 4],
    /// 四只基金的真实单位净值（账户镜像按「份额 × 净值」估值用）
    pub unit_nav: [f64; 4],
    /// 距各自最近一个已披露净值日的天数（0 = 当日已披露）
    pub nav_stale_days: [i32; 4],
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DatasetMeta {
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub calendar_range: Vec<String>,
    #[serde(default)]
    pub calendar_observations: usize,
    #[serde(default)]
    pub strategy_ready_range: Vec<String>,
    #[serde(default)]
    pub strategy_ready_observations: usize,
    #[serde(default)]
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Freshness {
    pub last_date: NaiveDate,
    pub age_days: i64,
    pub sla_days: u32,
}

impl Freshness {
    pub fn ok(&self) -> bool {
        self.age_days >= 0 && self.age_days <= self.sla_days as i64
    }
    pub fn banner(&self) -> String {
        if self.ok() {
            format!(
                "数据新鲜度 OK：末日 {}，落后 {} 天（SLA {} 天）",
                self.last_date, self.age_days, self.sla_days
            )
        } else {
            format!(
                "⚠️ 数据已陈旧：末日 {}，落后 {} 天（SLA {} 天）。请运行 `mns backtest update-data`",
                self.last_date, self.age_days, self.sla_days
            )
        }
    }
}

#[derive(Debug, Clone)]
pub struct LiveDataset {
    pub days: Vec<Day>,
    pub meta: DatasetMeta,
    pub path: PathBuf,
}

impl LiveDataset {
    /// 解析数据路径：绝对路径优先，其次当前工作目录、仓库根、~/.mns/。
    /// `data_path` 是相对仓库根的配置值，用编译期 MANIFEST_DIR 兜底，
    /// 保证从任意目录调用 `mns` 都能找到同一份数据。
    pub fn resolve_path(config: &AppConfig) -> PathBuf {
        let raw = Path::new(&config.backtest.data_path);
        if raw.is_absolute() {
            return raw.to_path_buf();
        }
        let candidates = [
            raw.to_path_buf(),
            Path::new(env!("CARGO_MANIFEST_DIR")).join(raw),
            dirs::home_dir()
                .unwrap_or_default()
                .join(".mns")
                .join(raw.file_name().unwrap_or_default()),
        ];
        for p in &candidates {
            if p.exists() {
                return p.clone();
            }
        }
        candidates[1].clone()
    }

    pub fn load(config: &AppConfig) -> Result<Self> {
        let path = Self::resolve_path(config);
        let text = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "读取实盘数据集失败: {}\n请先运行 `mns backtest update-data` 生成数据集",
                path.display()
            )
        })?;
        let days = Self::parse(&text)?;
        if days.is_empty() {
            bail!("实盘数据集为空: {}", path.display());
        }
        let meta_path = path.with_file_name(
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| format!("{s}_meta.json"))
                .unwrap_or_else(|| "live_daily_meta.json".to_string()),
        );
        let meta: DatasetMeta = std::fs::read_to_string(&meta_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        // 结构校验：日期严格升序、净值恒正、无重复
        for w in days.windows(2) {
            if w[1].date <= w[0].date {
                bail!("实盘数据集日期非严格升序: {} -> {}", w[0].date, w[1].date);
            }
        }
        for d in &days {
            for (i, v) in d.nav.iter().enumerate() {
                if !v.is_finite() || *v <= 0.0 {
                    bail!("{} 的 {} 净值非法: {}", d.date, CODES[i], v);
                }
            }
        }
        Ok(Self { days, meta, path })
    }

    fn parse(text: &str) -> Result<Vec<Day>> {
        let mut out = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("date,") {
                continue;
            }
            let p: Vec<&str> = line.split(',').collect();
            if p.len() < 7 {
                continue;
            }
            let Ok(date) = NaiveDate::parse_from_str(p[0], "%Y-%m-%d") else {
                continue;
            };
            let Ok(raw_fgi) = p[1].parse::<f64>() else {
                continue;
            };
            let fgi = if raw_fgi < 0.0 { None } else { Some(raw_fgi) };
            let fgi_age_days = p[2].parse::<i32>().unwrap_or(-1);
            let mut nav = [0.0; 4];
            let mut ok = true;
            for i in 0..4 {
                match p[3 + i].parse::<f64>() {
                    Ok(v) if v > 0.0 => nav[i] = v,
                    _ => ok = false,
                }
            }
            if !ok {
                continue;
            }
            let mut stale = [0_i32; 4];
            if p.len() > 7 {
                for (i, s) in p[7].split('|').take(4).enumerate() {
                    stale[i] = s.parse().unwrap_or(0);
                }
            }
            // 末 4 列为真实单位净值（账户镜像用）。旧格式缺列时退化为 0，
            // 由调用方检测并拒绝在该数据集上做账户镜像。
            let mut unit = [0.0; 4];
            if p.len() >= 12 {
                for i in 0..4 {
                    unit[i] = p[8 + i].parse().unwrap_or(0.0);
                }
            }
            out.push(Day {
                date,
                fgi,
                fgi_age_days,
                nav,
                unit_nav: unit,
                nav_stale_days: stale,
            });
        }
        Ok(out)
    }

    /// FGI 可用且连续的交易日区间（策略可验证段）。
    pub fn strategy_days(&self) -> Vec<&Day> {
        self.days.iter().filter(|d| d.fgi.is_some()).collect()
    }

    pub fn freshness(&self, sla_days: u32) -> Result<Freshness> {
        let last = self.days.last().context("数据集无行")?.date;
        let today = chrono::Local::now().date_naive();
        Ok(Freshness { last_date: last, age_days: (today - last).num_days(), sla_days })
    }

    /// 把四只基金合成为三条腿的净值指数，并以 `start_idx` 为起点归一到 1.0。
    ///
    /// 四只基金的"起点 1.0"各自对应不同成立日，不能跨基金直接加权；
    /// 必须先在共同验证区间起点归一，再按腿内初始权重持有（份额固定，
    /// 不假定每日再平衡）。
    pub fn leg_series(&self, start_idx: usize) -> Vec<[f64; 3]> {
        let base = self.days.get(start_idx).map(|d| d.nav).unwrap_or([1.0; 4]);
        self.days[start_idx..]
            .iter()
            .map(|d| {
                let mut legs = [0.0; 3];
                for i in 0..4 {
                    legs[LEG_OF[i]] += LEG_IN_WEIGHT[i] * (d.nav[i] / base[i]);
                }
                legs
            })
            .collect()
    }

    /// 触发三个数据集的重建（调用仓库内 Python 脚本，单一实现避免两套抓取逻辑）。
    pub fn update_all(config: &AppConfig) -> Result<String> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".agents/skills/mns-backtest/data");
        let mut text = String::new();
        for (script, out) in [
            ("build_live_dataset.py", Self::resolve_path(config)),
            ("build_long_dataset.py", LongDataset::resolve_path(config)),
            ("build_hs300_dataset.py", LongDataset::path_for("hs300")),
        ] {
            let path = dir.join(script);
            if !path.exists() {
                bail!("找不到数据构建脚本: {}", path.display());
            }
            text.push_str(&format!("===== {script} =====\n"));
            text.push_str(&Self::run_script(&path, &out)?);
        }
        Ok(text)
    }

    pub fn update(config: &AppConfig) -> Result<String> {
        Self::update_all(config)
    }

    fn run_script(script: &Path, out: &Path) -> Result<String> {
        let output = Command::new("python3")
            .arg(script)
            .arg("--out")
            .arg(out)
            .output()
            .with_context(|| "执行 python3 失败（需要 Python 3 与网络访问）")?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !output.status.success() {
            bail!("数据更新失败:\n{}", text);
        }
        Ok(text)
    }
}

/// 决策日判定：与 `signal.rs` 共用 `rebalance.decision_weekday`，
/// 再叠加回测设置里的决策周期，杜绝实盘/回测两条链路各用一套信号。
/// `next` 为下一根 bar 的日期（末根为 None）。Monthly 取每月最后一个交易日。
pub fn is_decision_day(config: &AppConfig, date: NaiveDate, next: Option<NaiveDate>) -> bool {
    let wd = date.weekday().num_days_from_monday() as u32 + 1;
    match config.backtest.decision_interval {
        crate::config::DecisionInterval::Daily => true,
        crate::config::DecisionInterval::Weekly => {
            // 用户配置 decision_weekday=0（每天）时，Weekly 必须回退到每周五；
            // 否则「恢复周频决策日保护」形同虚设，仍会产生每日信号。
            let target = if config.rebalance.decision_weekday == 0 {
                5
            } else {
                config.rebalance.decision_weekday
            };
            target == wd
        }
        crate::config::DecisionInterval::Monthly => match next {
            Some(n) => n.month() != date.month() || n.year() != date.year(),
            None => true,
        },
    }
}

// ───────────────────── 长周期代理数据集（2016-今） ─────────────────────

/// 长周期数据集的单日记录。三腿均为可比代理标的，不是实盘持仓。
#[derive(Debug, Clone)]
pub struct LongDay {
    pub date: NaiveDate,
    pub fgi: Option<f64>,
    pub fgi_age_days: i32,
    pub prices: [f64; 3],
    pub stale_days: [i32; 3],
    /// `archive`（2021-02 前第三方冻结存档，整数分值）/ `cnn_regen`（CNN 再生成，浮点）
    pub fgi_source: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct LongMeta {
    /// 实验数据集名称；为空时 to_bars 使用默认长周期标签
    #[serde(default)]
    pub label: String,
    /// 单资产实验数据集：调用方应把 allocation 覆盖为单腿
    #[serde(default)]
    pub single_asset: bool,
    #[serde(default)]
    pub purpose: String,
    #[serde(default)]
    pub calendar_range: Vec<String>,
    #[serde(default)]
    pub calendar_observations: usize,
    #[serde(default)]
    pub strategy_ready_range: Vec<String>,
    #[serde(default)]
    pub strategy_ready_observations: usize,
    #[serde(default)]
    pub limitations: Vec<String>,
    #[serde(default)]
    pub regimes: Vec<Regime>,
    /// 实验数据集的附加诊断（相关系数、分档收益等），仅用于展示
    #[serde(default)]
    pub diagnostics: serde_json::Value,
    /// 各牛熊段的买入持有涨跌，用于核验分段是否合理
    #[serde(default)]
    pub regime_buy_hold_check: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Regime {
    pub name: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone)]
pub struct LongDataset {
    pub days: Vec<LongDay>,
    pub meta: LongMeta,
    pub path: PathBuf,
}

impl LongDataset {
    pub fn resolve_path(_config: &AppConfig) -> PathBuf {
        Self::path_for("long")
    }

    fn path_for(stem: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".agents/skills/mns-backtest/data")
            .join(format!("{stem}_daily.csv"))
    }

    pub fn load(config: &AppConfig) -> Result<Self> {
        Self::load_named(config, "long")
    }

    /// 加载同格式的实验数据集（如 `hs300_daily.csv`）。CSV 与 meta 结构与长周期层一致，
    /// 因而可直接送进同一套审计引擎，保证信号/成本/执行/回撤口径完全可比。
    pub fn load_named(config: &AppConfig, stem: &str) -> Result<Self> {
        let path = if stem == "long" {
            Self::resolve_path(config)
        } else {
            Self::path_for(stem)
        };
        let text = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "读取数据集失败: {}\n请运行对应的数据构建脚本或 `mns backtest update-data`",
                path.display()
            )
        })?;
        let mut days = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("date,") {
                continue;
            }
            let p: Vec<&str> = line.split(',').collect();
            if p.len() < 7 {
                continue;
            }
            let Ok(date) = NaiveDate::parse_from_str(p[0], "%Y-%m-%d") else {
                continue;
            };
            let raw = p[1].parse::<f64>().unwrap_or(-1.0);
            let mut prices = [0.0; 3];
            let mut ok = true;
            for i in 0..3 {
                match p[3 + i].parse::<f64>() {
                    Ok(v) if v > 0.0 => prices[i] = v,
                    _ => ok = false,
                }
            }
            if !ok {
                continue;
            }
            let mut stale = [0_i32; 3];
            for (i, s) in p[6].split('|').take(3).enumerate() {
                stale[i] = s.parse().unwrap_or(0);
            }
            days.push(LongDay {
                date,
                fgi: if raw < 0.0 { None } else { Some(raw) },
                fgi_age_days: p[2].parse().unwrap_or(-1),
                prices,
                stale_days: stale,
                fgi_source: p.get(7).unwrap_or(&"").to_string(),
            });
        }
        if days.is_empty() {
            bail!("长周期数据集为空: {}", path.display());
        }
        for w in days.windows(2) {
            if w[1].date <= w[0].date {
                bail!("长周期数据集日期非严格升序: {} -> {}", w[0].date, w[1].date);
            }
        }
        let mp = path.with_file_name(format!("{stem}_daily_meta.json"));
        let meta: LongMeta = std::fs::read_to_string(mp)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Ok(Self { days, meta, path })
    }

    pub fn freshness(&self, sla_days: u32) -> Result<Freshness> {
        let last = self.days.last().context("长周期数据集无行")?.date;
        let today = chrono::Local::now().date_naive();
        Ok(Freshness { last_date: last, age_days: (today - last).num_days(), sla_days })
    }
}

// ───────────────────── 引擎统一输入 ─────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatasetKind {
    /// 实盘四只基金，用于账户镜像
    Live,
    /// 2016-今三腿代理，用于穿越牛熊的策略检验
    Long,
}

/// 回测/审计引擎的唯一输入格式。两个数据集先转换为 Bar，再进同一套引擎，
/// 保证「信号、成本、回撤口径」在长周期验证与实盘镜像中完全一致。
#[derive(Debug, Clone)]
pub struct Bar {
    pub date: NaiveDate,
    pub fgi: Option<f64>,
    pub fgi_age_days: i32,
    pub prices: [f64; 3],
    /// 仅实盘层提供：四只基金真实单位净值，供账户镜像按份额估值
    pub unit_nav: Option<[f64; 4]>,
}

#[derive(Debug, Clone)]
pub struct BarSeries {
    pub kind: DatasetKind,
    pub label: String,
    pub bars: Vec<Bar>,
    pub caveats: Vec<String>,
}

impl LiveDataset {
    /// 从 `start_idx` 起转换为引擎输入，三腿按区间起点归一。
    pub fn to_bars(&self, start_idx: usize) -> BarSeries {
        let legs = self.leg_series(start_idx);
        let bars = self.days[start_idx..]
            .iter()
            .enumerate()
            .map(|(i, d)| Bar {
                date: d.date,
                fgi: d.fgi,
                fgi_age_days: d.fgi_age_days,
                prices: legs[i],
                unit_nav: Some(d.unit_nav),
            })
            .collect();
        BarSeries {
            kind: DatasetKind::Live,
            label: "实盘镜像（018966/539001/007605/020341 真实净值）".to_string(),
            bars,
            caveats: vec![
                "共同区间起点由最晚成立的 020341（2024-01-11）决定，仅 34 个月".to_string(),
                "CNN 日频 FGI 仅覆盖近一年，故本层策略可验证段约 240 个交易日".to_string(),
            ],
        }
    }
}

impl LongDataset {
    pub fn to_bars(&self) -> BarSeries {
        let bars = self.days.iter().map(|d| Bar {
            date: d.date,
            fgi: d.fgi,
            fgi_age_days: d.fgi_age_days,
            prices: d.prices,
            unit_nav: None,
        }).collect();
        // 口径限制以 meta.limitations 为唯一来源，避免与手写条目重复
        let mut caveats = Vec::new();
        if !self.meta.purpose.is_empty() {
            caveats.push(format!("用途：{}", self.meta.purpose));
        }
        caveats.extend(self.meta.limitations.iter().cloned());
        BarSeries {
            kind: DatasetKind::Long,
            label: if self.meta.label.is_empty() {
                "长周期代理（2016-今，穿越牛熊）".to_string()
            } else {
                self.meta.label.clone()
            },
            bars,
            caveats,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> &'static str {
        "# comment\n\
         date,fgi,fgi_age_days,018966,539001,007605,020341,nav_stale_days,018966_unit,539001_unit,007605_unit,020341_unit\n\
         2026-01-05,-1,-1,1.0,1.0,1.0,1.0,0|0|0|0,1.0,2.0,1.2,1.9\n\
         2026-01-06,30.0,1,1.1,1.2,0.9,1.05,0|1|0|0,1.1,2.4,1.1,2.0\n\
         2026-01-07,25.0,1,1.2,1.1,1.0,1.1,0|0|0|0,1.2,2.2,1.2,2.1\n"
    }

    #[test]
    fn 解析应跳过注释表头与缺失FGI() {
        let days = LiveDataset::parse(sample()).unwrap();
        assert_eq!(days.len(), 3);
        assert_eq!(days[0].fgi, None);
        assert_eq!(days[1].fgi, Some(30.0));
        assert_eq!(days[1].nav_stale_days[1], 1);
    }

    #[test]
    fn FGI缺失日不进入策略验证段() {
        let ds = LiveDataset {
            days: LiveDataset::parse(sample()).unwrap(),
            meta: DatasetMeta::default(),
            path: PathBuf::new(),
        };
        assert_eq!(ds.strategy_days().len(), 2);
    }

    #[test]
    fn 三腿合成应归一到区间起点() {
        let ds = LiveDataset {
            days: LiveDataset::parse(sample()).unwrap(),
            meta: DatasetMeta::default(),
            path: PathBuf::new(),
        };
        let legs = ds.leg_series(1);
        assert!((legs[0][0] - 1.0).abs() < 1e-9, "起点纳指腿应为 1.0");
        assert!((legs[0][1] - 1.0).abs() < 1e-9);
        assert!((legs[0][2] - 1.0).abs() < 1e-9);
        // 第二日纳指腿 = 0.5*(1.2/1.1) + 0.5*(1.1/1.2)
        let expect = 0.5 * (1.2 / 1.1) + 0.5 * (1.1 / 1.2);
        assert!((legs[1][0] - expect).abs() < 1e-9);
    }

    #[test]
    fn 单资产实验数据集按名称加载且三腿同值() {
        let cfg = AppConfig::default_config();
        let Ok(ds) = LongDataset::load_named(&cfg, "hs300") else { return };
        assert!(ds.meta.single_asset, "hs300 应标记为单资产实验数据集");
        assert!(!ds.days.is_empty());
        assert!(!ds.meta.label.is_empty(), "实验数据集应有自己的标签");
        for d in ds.days.iter().take(50) {
            assert!((d.prices[0] - d.prices[1]).abs() < 1e-9);
            assert!((d.prices[0] - d.prices[2]).abs() < 1e-9);
        }
    }

    #[test]
    fn 真实数据集应覆盖实盘入场日且无前视() {
        let cfg = AppConfig::default_config();
        let Ok(ds) = LiveDataset::load(&cfg) else { return };
        let entry = NaiveDate::from_ymd_opt(2026, 7, 16).unwrap();
        assert!(ds.days.iter().any(|d| d.date >= entry), "数据集必须覆盖真实入场日");
        // 任何可用 FGI 都必须严格早于其所在交易日
        assert!(ds.days.iter().all(|d| d.fgi.is_none() || d.fgi_age_days >= 1),
                "FGI 必须至少滞后 1 天，否则存在前视偏差");
    }
}
