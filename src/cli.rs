use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "mns",
    version,
    about = "逆向投资助手 - Market Neutral Strategist"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// 初始化配置文件和数据库
    /// 使用 --force 跳过确认提示
    Init {
        /// 跳过确认，强制覆盖已有数据
        #[arg(short, long)]
        force: bool,
    },

    /// 查看/修改配置项
    Config {
        /// 配置项名称 (如 thresholds.fear)
        key: Option<String>,
        /// 配置项新值
        value: Option<String>,
    },

    /// 现金管理 (无子命令时查看余额)
    Cash {
        #[command(subcommand)]
        action: Option<CashAction>,
    },

    /// 查看持仓概览（含年化收益）
    Portfolio,

    /// 新增资产到持仓池
    Add {
        /// 资产代码 (如 QQQ)
        code: String,
        /// 资产名称 (如 "纳指100")
        name: String,
        /// 类别: us_stocks / cn_stocks / counter_cyclical
        category: String,
    },

    /// 记录买入
    Buy {
        /// 资产代码
        code: String,
        /// 买入份额
        shares: f64,
        /// 买入价格
        price: f64,
    },

    /// 记录卖出
    Sell {
        /// 资产代码
        code: String,
        /// 卖出份额
        shares: f64,
        /// 卖出价格
        price: f64,
    },

    /// 更新资产当前价格
    Price {
        /// 资产代码
        code: String,
        /// 当前价格 (省略则查看当前价格)
        price: Option<f64>,
    },

    /// 查看当前恐贪指数
    Sentiment,

    /// 生成今日策略报告
    Report,

    /// 查看交易历史
    History {
        /// 显示条数
        #[arg(default_value = "20")]
        limit: i64,
    },
}

#[derive(Subcommand)]
pub enum CashAction {
    /// 设置现金余额
    Set {
        /// 金额
        amount: f64,
    },
    /// 增加现金
    Add {
        /// 金额
        amount: f64,
    },
}
