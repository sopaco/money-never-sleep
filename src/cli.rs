use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "mns",
    version,
    about = "逆情绪投资助手 - Money Never Sleeps（Market Neutral Strategist）",
    after_help = "策略速览: 恐贪指数 → 目标风险仓位（默认 35%~85%，五档）→ 偏离带再平衡（默认 ±4pp）
决策节奏: 默认每周五决策日（rebalance.decision_weekday 可改），其余天出观察版
快速上手: mns init → mns cash deposit 10000 → mns add QQQ 纳指100 us_stocks → mns today
调参清单: mns config params（全部可调参数、默认值与效果说明）"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// 初始化 ~/.mns/config.toml 与 ~/.mns/mns.db（默认平衡档；已有数据会提示确认）
    /// 使用 --force 跳过确认提示
    Init {
        /// 跳过确认，强制覆盖已有数据
        #[arg(short, long)]
        force: bool,
    },

    /// 无参数=输出完整配置(TOML)；KEY=查看；KEY VALUE=修改（校验通过后落盘）
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
        /// 配置项名称，点路径（如 thresholds.fear；清单见 `mns config params`）
        key: Option<String>,
        /// 配置项新值
        value: Option<String>,
    },

    /// 现金管理 (无子命令时查看余额与最近流水)
    Cash {
        #[command(subcommand)]
        action: Option<CashAction>,
    },

    /// 查看持仓概览（含年化收益）；组合级 XIRR/回撤见 performance
    Portfolio,

    /// 组合绩效仪表盘：XIRR / 实际最大回撤 / 对照策略预期（只读，零写入）；单标的角度见 portfolio
    Performance,

    /// 新增资产到持仓池
    Add {
        /// 资产代码 (如 QQQ)
        code: String,
        /// 资产名称 (如 "纳指100")
        name: String,
        /// 类别: us_stocks(美股宽基) / cn_stocks(A股宽基) / counter_cyclical(逆周期·黄金等)；
        /// 三腿内部比例由 allocation.* 控制
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

    /// 查看/更新单个资产当前价（批量自动更新见 update-prices）
    Price {
        /// 资产代码
        code: String,
        /// 当前价格 (省略则查看当前价格)
        price: Option<f64>,
    },

    /// 删除资产
    Remove {
        /// 资产代码
        code: String,
    },

    /// 获取 CNN 恐贪指数并保存当日快照（历史快照用于信号平滑与区间确认）
    Sentiment,

    /// 生成今日策略报告（决策日→决策版含调仓指令；其余天→观察版仅提示不动手。
    /// 决策日默认周五，`rebalance.decision_weekday` 可改）
    Report {
        /// 非决策日仍强制生成决策版报告（含调仓指令）
        #[arg(long)]
        decide: bool,
    },

    /// 一键：先更新价格，再生成今日报告（价格失败也继续）；--decide 同 report
    Today {
        /// 非决策日仍强制生成决策版报告（含调仓指令）
        #[arg(long)]
        decide: bool,
        /// 只输出结论与报告路径，不打印价格明细
        #[arg(long)]
        quiet: bool,
    },

    /// 定投计划：按当前情绪与目标仓位给出本月各腿买入金额（只买不卖）
    Plan {
        /// 每月投入金额（省略则读取 cashflow.monthly 配置）
        #[arg(long)]
        monthly: Option<f64>,
    },

    /// 查看交易流水（买入/卖出/入金/出金/纠错，新→旧）
    History {
        /// 显示最近 N 条（默认 20）
        #[arg(default_value = "20")]
        limit: i64,
    },

    /// 实盘审计：真实标的日频回测 + 策略有效性检验 + 账户镜像（默认命令）
    #[command(
        after_help = "默认链路使用实盘四只基金(018966/539001/007605/020341)与 2016-今长周期代理数据，
信号与 mns report 同源（7天平滑 + 3天确认 + 决策周期），按真实通道建模 T+n 结算与限购。
旧的 2016-2025 月频代理对比已降级为 `backtest legacy`，其标的与信号口径均不同于实盘，
结论不可迁移到实盘决策。"
    )]
    Backtest {
        #[command(subcommand)]
        action: Option<BacktestAction>,
        /// 使用指定 config.toml（省略则用 ~/.mns/config.toml 当前配置）
        #[arg(short, long)]
        config: Option<String>,
        /// 对比多个配置文件（仅 legacy 子命令使用）
        #[arg(long)]
        compare: Option<String>,
    },

    /// 自动拉取全部持仓最新价（mns today 也会先做这一步）
    UpdatePrices,

    /// 市场综合概况（指数、恐贪指数）
    Market,

    /// 个股报价查询（价格、涨跌幅）
    Analyze {
        /// 股票代码 (如 AAPL, MSFT)
        symbol: String,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// 列出全部可调参数（默认值与效果说明）
    Params,
}

#[derive(Subcommand)]
pub enum CashAction {
    /// 设置现金余额（纠错工具；会记一条 adjust 流水保证台账可重建）
    Set {
        /// 金额
        amount: f64,
    },
    /// 外部入金——资金进入组合的主通道（XIRR 口径）
    Deposit {
        /// 金额（正数）
        amount: f64,
        /// 备注（如来源/用途）
        #[arg(long)]
        note: Option<String>,
    },
    /// 外部出金——资金离开组合
    Withdraw {
        /// 金额（正数）
        amount: f64,
        /// 备注（如用途）
        #[arg(long)]
        note: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum BacktestAction {
    /// 账户镜像：严格执行策略 vs 数据库真实交易流水（仅实盘层）
    Mirror,
    /// 重建实盘与长周期日频数据集（抓取行情与情绪，含新鲜度校验）
    UpdateData,
    /// 完整有效性检验：置换/常数仓位/延迟/随机对照 + 牛熊分段 + 多折 walk-forward
    ValidateFull {
        /// 置换检验次数（覆盖 backtest.permutation_iterations）
        #[arg(long)]
        iterations: Option<usize>,
    },
    /// 【已降级】旧的 2016-2025 月频代理对比，结论不可迁移到实盘
    Legacy,
    /// 【已降级】旧版单一切分 walk-forward + 分块 bootstrap + 13 月 holdout
    Validate {
        /// bootstrap 重采样次数（默认 2000）
        #[arg(long, default_value = "2000")]
        iterations: usize,
        /// 分块长度（月，默认 6）
        #[arg(long, default_value = "6")]
        block: usize,
    },
}
