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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub annualized_target_low: f64,
    pub annualized_target_high: f64,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub fear_greed_url: String,
}

impl AppConfig {
    pub fn default_config() -> Self {
        Self {
            settings: Settings {
                annualized_target_low: 10.0,
                annualized_target_high: 15.0,
                report_output_dir: "./reports".to_string(),
            },
            allocation: Allocation {
                us_stocks: 50.0,
                cn_stocks: 35.0,
                counter_cyclical: 15.0,
            },
            thresholds: Thresholds {
                extreme_fear: 25.0,
                fear: 45.0,
                neutral: 55.0,
                greed: 75.0,
            },
            buy_ratio: BuyRatio {
                extreme_fear: 50.0,
                fear: 30.0,
                neutral: 20.0,
                greed: 0.0,
            },
            sell_ratio: SellRatio {
                extreme_greed_target_high: 50.0,
                extreme_greed_target_low: 30.0,
                extreme_greed_below_target: 20.0,
                greed_target_high: 40.0,
                greed_target_low: 20.0,
            },
            api: ApiConfig {
                fear_greed_url: "https://production.dataviz.cnn.io/index/fearandgreed/graphdata"
                    .to_string(),
            },
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
        let path = Self::config_path()?;
        let content = fs::read_to_string(&path)
            .with_context(|| format!("读取配置文件失败: {}", path.display()))?;
        toml::from_str(&content).with_context(|| "解析配置文件失败")
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
            "Extreme Fear"
        } else if score < self.thresholds.fear {
            "Fear"
        } else if score < self.thresholds.neutral {
            "Neutral"
        } else if score < self.thresholds.greed {
            "Greed"
        } else {
            "Extreme Greed"
        }
    }

    /// 根据情绪区间获取买入比例
    pub fn buy_ratio_for(&self, score: f64) -> f64 {
        if score < self.thresholds.extreme_fear {
            self.buy_ratio.extreme_fear
        } else if score < self.thresholds.fear {
            self.buy_ratio.fear
        } else if score < self.thresholds.neutral {
            self.buy_ratio.neutral
        } else {
            self.buy_ratio.greed
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
        } else {
            0.0
        }
    }

    /// 用 dot path 获取/设置配置值
    pub fn get_value(&self, key: &str) -> Option<String> {
        match key {
            "settings.annualized_target_low" => Some(self.settings.annualized_target_low.to_string()),
            "settings.annualized_target_high" => Some(self.settings.annualized_target_high.to_string()),
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
            "sell_ratio.extreme_greed_target_high" => Some(self.sell_ratio.extreme_greed_target_high.to_string()),
            "sell_ratio.extreme_greed_target_low" => Some(self.sell_ratio.extreme_greed_target_low.to_string()),
            "sell_ratio.extreme_greed_below_target" => Some(self.sell_ratio.extreme_greed_below_target.to_string()),
            "sell_ratio.greed_target_high" => Some(self.sell_ratio.greed_target_high.to_string()),
            "sell_ratio.greed_target_low" => Some(self.sell_ratio.greed_target_low.to_string()),
            "api.fear_greed_url" => Some(self.api.fear_greed_url.clone()),
            _ => None,
        }
    }

    pub fn set_value(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "settings.annualized_target_low" => self.settings.annualized_target_low = value.parse()?,
            "settings.annualized_target_high" => self.settings.annualized_target_high = value.parse()?,
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
            "sell_ratio.extreme_greed_target_high" => self.sell_ratio.extreme_greed_target_high = value.parse()?,
            "sell_ratio.extreme_greed_target_low" => self.sell_ratio.extreme_greed_target_low = value.parse()?,
            "sell_ratio.extreme_greed_below_target" => self.sell_ratio.extreme_greed_below_target = value.parse()?,
            "sell_ratio.greed_target_high" => self.sell_ratio.greed_target_high = value.parse()?,
            "sell_ratio.greed_target_low" => self.sell_ratio.greed_target_low = value.parse()?,
            "api.fear_greed_url" => self.api.fear_greed_url = value.to_string(),
            _ => anyhow::bail!("未知的配置项: {}", key),
        }
        Ok(())
    }
}
