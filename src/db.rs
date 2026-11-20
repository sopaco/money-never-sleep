use anyhow::{Context, Result};
use chrono::Local;
use rusqlite::{Connection, params};

use crate::models::{Position, Transaction};

/// 最新一条恐贪快照（完整字段）。CNN 获取失败时的降级数据源。
/// 注：区间标签不存这里——降级时用当前配置从分数重算，保证与
/// 正常路径同口径（历史落库的 rating 可能来自旧阈值）。
#[derive(Debug, Clone)]
pub struct FearGreedSnapshot {
    pub date: chrono::NaiveDate,
    pub score: f64,
    pub previous_close: Option<f64>,
    pub previous_1_week: Option<f64>,
    pub previous_1_month: Option<f64>,
    pub previous_1_year: Option<f64>,
}

/// 外部现金流台账条目（C1）。买卖交易**不进此表**——那是组合内部的
/// 现金↔持仓转换；XIRR 只需要外部资金进出。
#[derive(Debug, Clone)]
pub struct CashFlow {
    /// 行 id。当前仅作排序锚（ORDER BY id DESC），保留为将来
    /// 纠错入口（如删除误记流水）的句柄。
    #[allow(dead_code)]
    pub id: i64,
    pub flow_date: chrono::NaiveDate,
    /// 有符号金额：入金/期初为正，出金为负，adjust 为新旧余额之差
    pub amount: f64,
    /// deposit / withdraw / opening / adjust
    pub kind: String,
    pub note: Option<String>,
}

pub struct Database {
    conn: Connection,
}

/// 多进程共库的底层调优：WAL + busy_timeout。
///
/// butlerd 常驻读、GUI / CLI 随手写的形态下，默认 rollback journal 会在
/// 读写相撞时直接抛 `SQLITE_BUSY`；WAL 允许一写多读并存，busy_timeout
/// 再兜住短暂的迁移窗口。`journal_mode` 是库文件的持久属性：升级后第一个
/// 可写连接一次性完成迁移，之后的设置是幂等 no-op，因此失败不阻断打开
/// （退回 rollback journal 时 busy_timeout 仍然挡着 BUSY）。
fn tune(conn: &Connection) {
    if let Err(error) = conn.busy_timeout(std::time::Duration::from_secs(5)) {
        eprintln!("数据库 busy_timeout 设置失败：{error}");
    }
    // journal_mode 的 PRAGMA 会回一行结果，`pragma_update` 不收带结果的
    // 语句，直接按查询读回。
    if let Err(error) = conn.query_row("PRAGMA journal_mode=WAL", [], |_row| Ok(())) {
        eprintln!("数据库 WAL 模式设置失败（沿用现有 journal 模式）：{error}");
    }
}

impl Database {
    pub fn open() -> Result<Self> {
        Self::open_at(&crate::config::AppConfig::db_path()?)
    }

    /// 只读连接：给「只判定、不记账」的调用方（管家守护进程的事实计算）。
    ///
    /// 访问边界钉在连接标志上，比约定可靠——守护进程想写也没有写路径。
    /// WAL 库的只读访问在「库被干净关闭（WAL 已 checkpoint）」或「有别的
    /// 连接开着」时成立；两样都不满足时打开失败，调用方按「暂无账本」
    /// 处理，下一次可写打开即恢复。
    pub fn open_read_only() -> Result<Self> {
        let path = crate::config::AppConfig::db_path()?;
        Self::open_read_only_at(&path)
    }

    fn open_at(path: &std::path::Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("打开数据库失败: {}", path.display()))?;
        tune(&conn);
        let db = Self { conn };
        db.init_tables()?;
        db.ensure_cash_flow_opening()?;
        Ok(db)
    }

    fn open_read_only_at(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("打开数据库失败（只读）: {}", path.display()))?;
        // 只读连接同样可能撞上短暂锁（如另一进程正在迁移 journal 模式）：
        // 等待而不是立刻报错。
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .context("设置 busy_timeout 失败")?;
        Ok(Self { conn })
    }

    fn init_tables(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cash (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                balance REAL NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS positions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                asset_code TEXT NOT NULL UNIQUE,
                asset_name TEXT NOT NULL,
                category TEXT NOT NULL,
                shares REAL NOT NULL DEFAULT 0,
                cost_price REAL NOT NULL DEFAULT 0,
                current_price REAL,
                first_buy_date TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS transactions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                type TEXT NOT NULL CHECK (type IN ('buy', 'sell')),
                asset_code TEXT NOT NULL,
                shares REAL NOT NULL,
                price REAL NOT NULL,
                amount REAL NOT NULL,
                tx_date TEXT NOT NULL,
                note TEXT
            );
            CREATE TABLE IF NOT EXISTS fear_greed_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                score REAL NOT NULL,
                rating TEXT NOT NULL,
                snapshot_date TEXT NOT NULL,
                previous_close REAL,
                previous_1_week REAL,
                previous_1_month REAL,
                previous_1_year REAL,
                fetched_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS cash_flows (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                flow_date TEXT NOT NULL,
                amount REAL NOT NULL,
                kind TEXT NOT NULL CHECK (kind IN ('deposit', 'withdraw', 'opening', 'adjust')),
                note TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS portfolio_snapshots (
                snap_date TEXT NOT NULL PRIMARY KEY,
                cash REAL NOT NULL,
                holdings_value REAL NOT NULL,
                total_value REAL NOT NULL,
                fear_score REAL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS price_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                asset_code TEXT NOT NULL,
                price REAL NOT NULL,
                price_date TEXT NOT NULL,
                UNIQUE(asset_code, price_date)
            );
            INSERT OR IGNORE INTO cash (id, balance) VALUES (1, 0);",
        )?;
        Ok(())
    }

    /// C1 台账启用：首次打开（cash_flows 为空）时写入一条 opening 流水 =
    /// 当前余额，作为 XIRR 的起点。历史不可重建，note 如实标注。
    /// 新库余额为 0 也写——标记台账启用日。CREATE TABLE IF NOT EXISTS 原地
    /// 升级，用户已有数据零接触。
    fn ensure_cash_flow_opening(&self) -> Result<()> {
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM cash_flows", [], |row| row.get(0))?;
        if count > 0 {
            return Ok(());
        }
        let balance = self.get_cash_balance()?;
        let today = Local::now().date_naive();
        self.add_cash_flow(
            today,
            balance,
            "opening",
            Some("台账启用，此前的余额历史不可重建"),
        )?;
        Ok(())
    }

    // ── Cash ──────────────────────────────────────

    pub fn get_cash_balance(&self) -> Result<f64> {
        let balance: f64 =
            self.conn
                .query_row("SELECT balance FROM cash WHERE id = 1", [], |row| {
                    row.get(0)
                })?;
        Ok(balance)
    }

    pub fn set_cash_balance(&self, amount: f64) -> Result<()> {
        if amount < 0.0 {
            anyhow::bail!("现金余额不能为负数: {}", amount);
        }
        let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        self.conn.execute(
            "UPDATE cash SET balance = ?, updated_at = ? WHERE id = 1",
            params![amount, now],
        )?;
        Ok(())
    }

    pub fn add_cash(&self, amount: f64) -> Result<f64> {
        if amount <= 0.0 {
            anyhow::bail!("增加现金金额必须为正数: {}", amount);
        }
        let balance = self.get_cash_balance()?;
        let new_balance = balance + amount;
        self.set_cash_balance(new_balance)?;
        Ok(new_balance)
    }

    // ── Cash Flows 台账（C1）─────────────────────

    /// 记一条外部现金流。买卖交易的现金影响走 transactions，不进此表。
    pub fn add_cash_flow(
        &self,
        date: chrono::NaiveDate,
        amount: f64,
        kind: &str,
        note: Option<&str>,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO cash_flows (flow_date, amount, kind, note) VALUES (?, ?, ?, ?)",
                params![date.format("%Y-%m-%d").to_string(), amount, kind, note],
            )
            .with_context(|| format!("写入现金流失败: {} {}", kind, amount))?;
        Ok(())
    }

    /// 最近 n 条流水，新→旧排序。
    pub fn list_cash_flows(&self, limit: i64) -> Result<Vec<CashFlow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, flow_date, amount, kind, note FROM cash_flows
             ORDER BY id DESC LIMIT ?",
        )?;
        let rows = stmt.query_map(params![limit], Self::cash_flow_from_row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// (id, flow_date, amount, kind, note) 列序 → CashFlow。
    fn cash_flow_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CashFlow> {
        let date_str: String = row.get(1)?;
        let date = chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                0, rusqlite::types::Type::Text, Box::new(e),
            ))?;
        Ok(CashFlow {
            id: row.get(0)?,
            flow_date: date,
            amount: row.get(2)?,
            kind: row.get(3)?,
            note: row.get(4)?,
        })
    }

    // ── Portfolio Snapshots 每日总资产（C3）──────

    /// 当日总资产快照（upsert：同日多次运行取最后一次——"当日终态"语义，
    /// 回撤测量的标准口径）。不回填历史，自启用日起积累。
    /// fear_score 记当日决策分数（降级时即降级所用的分数）。
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_portfolio_snapshot(
        &self,
        date: chrono::NaiveDate,
        cash: f64,
        holdings_value: f64,
        total_value: f64,
        fear_score: Option<f64>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO portfolio_snapshots (snap_date, cash, holdings_value, total_value, fear_score, updated_at)
             VALUES (?, ?, ?, ?, ?, datetime('now'))
             ON CONFLICT(snap_date) DO UPDATE SET
                cash = excluded.cash,
                holdings_value = excluded.holdings_value,
                total_value = excluded.total_value,
                fear_score = excluded.fear_score,
                updated_at = excluded.updated_at",
            params![
                date.format("%Y-%m-%d").to_string(),
                cash,
                holdings_value,
                total_value,
                fear_score
            ],
        )?;
        Ok(())
    }

    /// 资产曲线的追踪起点（最早快照日期）。表为空时返回 None。
    pub fn portfolio_tracking_since(&self) -> Result<Option<chrono::NaiveDate>> {
        let since: Option<String> = self.conn
            .query_row("SELECT MIN(snap_date) FROM portfolio_snapshots", [], |row| {
                row.get(0)
            })
            .unwrap_or(None);
        Ok(since.and_then(|s| chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()))
    }

    /// 全部外部现金流（升序），XIRR 的输入。
    pub fn all_cash_flows(&self) -> Result<Vec<CashFlow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, flow_date, amount, kind, note FROM cash_flows ORDER BY flow_date, id",
        )?;
        let rows = stmt.query_map([], Self::cash_flow_from_row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 每日总资产序列（snap_date 升序, total_value）——实际回撤的输入。
    pub fn portfolio_snapshot_series(&self) -> Result<Vec<(chrono::NaiveDate, f64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT snap_date, total_value FROM portfolio_snapshots ORDER BY snap_date",
        )?;
        let rows = stmt.query_map([], |row| {
            let date_str: String = row.get(0)?;
            let date = chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                    0, rusqlite::types::Type::Text, Box::new(e),
                ))?;
            Ok((date, row.get::<_, f64>(1)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 某标的价格历史的 (最早日, 最新日)。无记录返回 None——C4 就绪进度用。
    /// MIN/MAX 在零行时返回 NULL（聚合恒有一行），故列按可空读取。
    pub fn price_history_span(&self, code: &str) -> Result<Option<(chrono::NaiveDate, chrono::NaiveDate)>> {
        let row: (Option<String>, Option<String>) = self.conn
            .query_row(
                "SELECT MIN(price_date), MAX(price_date) FROM price_history WHERE asset_code = ?",
                params![code],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or((None, None));
        Ok(match row {
            (Some(min), Some(max)) => {
                match (
                    chrono::NaiveDate::parse_from_str(&min, "%Y-%m-%d"),
                    chrono::NaiveDate::parse_from_str(&max, "%Y-%m-%d"),
                ) {
                    (Ok(lo), Ok(hi)) => Some((lo, hi)),
                    _ => None,
                }
            }
            _ => None,
        })
    }

    // ── Positions ─────────────────────────────────

    pub fn add_position(&self, code: &str, name: &str, category: &str) -> Result<()> {
        let today = Local::now().format("%Y-%m-%d").to_string();
        let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        self.conn.execute(
            "INSERT INTO positions (asset_code, asset_name, category, shares, cost_price, first_buy_date, updated_at)
             VALUES (?, ?, ?, 0, 0, ?, ?)",
            params![code, name, category, today, now],
        ).with_context(|| format!("新增资产失败，代码 {} 可能已存在", code))?;
        Ok(())
    }

    pub fn list_positions(&self) -> Result<Vec<Position>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, asset_code, asset_name, category, shares, cost_price, current_price, first_buy_date, updated_at
             FROM positions ORDER BY category, asset_code",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Position {
                id: row.get(0)?,
                asset_code: row.get(1)?,
                asset_name: row.get(2)?,
                category: row.get(3)?,
                shares: row.get(4)?,
                cost_price: row.get(5)?,
                current_price: row.get(6)?,
                first_buy_date: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;
        let mut positions = Vec::new();
        for row in rows {
            positions.push(row?);
        }
        Ok(positions)
    }

    pub fn get_position(&self, code: &str) -> Result<Option<Position>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, asset_code, asset_name, category, shares, cost_price, current_price, first_buy_date, updated_at
             FROM positions WHERE asset_code = ?",
        )?;
        let mut rows = stmt.query_map([code], |row| {
            Ok(Position {
                id: row.get(0)?,
                asset_code: row.get(1)?,
                asset_name: row.get(2)?,
                category: row.get(3)?,
                shares: row.get(4)?,
                cost_price: row.get(5)?,
                current_price: row.get(6)?,
                first_buy_date: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn buy_position(&self, code: &str, shares: f64, price: f64, note: Option<&str>) -> Result<()> {
        if shares <= 0.0 {
            anyhow::bail!("买入份额必须为正数");
        }
        if price <= 0.0 {
            anyhow::bail!("买入价格必须为正数");
        }

        let pos = self
            .get_position(code)?
            .with_context(|| format!("未找到资产: {}", code))?;

        let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let today = Local::now().format("%Y-%m-%d").to_string();
        let amount = shares * price;

        let balance = self.get_cash_balance()?;
        if balance < amount {
            anyhow::bail!("现金余额不足: 当前 ¥{:.2}, 需要 ¥{:.2}", balance, amount);
        }

        let old_total = pos.shares * pos.cost_price;
        let new_shares = pos.shares + shares;
        let new_cost_price = if new_shares > 0.0 {
            (old_total + amount) / new_shares
        } else {
            price
        };

        let first_buy_date = if pos.shares == 0.0 {
            today.clone()
        } else {
            pos.first_buy_date.clone()
        };

        let tx = self
            .conn
            .unchecked_transaction()
            .with_context(|| "开启事务失败")?;

        tx.execute(
            "UPDATE positions SET shares = ?, cost_price = ?, current_price = ?, first_buy_date = ?, updated_at = ? WHERE asset_code = ?",
            params![new_shares, new_cost_price, price, first_buy_date, now, code],
        )?;

        tx.execute(
            "UPDATE cash SET balance = ?, updated_at = ? WHERE id = 1",
            params![balance - amount, now],
        )?;

        tx.execute(
            // note 只由 GUI 冷静期护栏写入（计划外冷静：/过热冷静：前缀）；
            // CLI 记买入传 None，保持既有行为不变。
            "INSERT INTO transactions (type, asset_code, shares, price, amount, tx_date, note) VALUES ('buy', ?, ?, ?, ?, ?, ?)",
            params![code, shares, price, amount, today, note],
        )?;

        tx.commit().with_context(|| "提交事务失败")?;

        Ok(())
    }

    pub fn sell_position(&self, code: &str, shares: f64, price: f64) -> Result<()> {
        if shares <= 0.0 {
            anyhow::bail!("卖出份额必须为正数");
        }
        if price <= 0.0 {
            anyhow::bail!("卖出价格必须为正数");
        }

        let pos = self
            .get_position(code)?
            .with_context(|| format!("未找到资产: {}", code))?;

        if shares > pos.shares + 1e-6 {
            anyhow::bail!(
                "卖出份额超出持有量: 持有 {:.2}, 欲卖 {:.2}",
                pos.shares,
                shares
            );
        }

        let actual_shares = shares.min(pos.shares); // 防止浮点误差
        let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let today = Local::now().format("%Y-%m-%d").to_string();
        let amount = actual_shares * price;
        let new_shares = pos.shares - actual_shares;

        let balance = self.get_cash_balance()?;

        let tx = self
            .conn
            .unchecked_transaction()
            .with_context(|| "开启事务失败")?;

        tx.execute(
            "UPDATE positions SET shares = ?, current_price = ?, updated_at = ? WHERE asset_code = ?",
            params![new_shares, price, now, code],
        )?;

        tx.execute(
            "UPDATE cash SET balance = ?, updated_at = ? WHERE id = 1",
            params![balance + amount, now],
        )?;

        tx.execute(
            "INSERT INTO transactions (type, asset_code, shares, price, amount, tx_date) VALUES ('sell', ?, ?, ?, ?, ?)",
            params![code, actual_shares, price, amount, today],
        )?;

        tx.commit().with_context(|| "提交事务失败")?;

        Ok(())
    }

    pub fn update_price(&self, code: &str, price: f64) -> Result<()> {
        let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let rows = self.conn.execute(
            "UPDATE positions SET current_price = ?, updated_at = ? WHERE asset_code = ?",
            params![price, now, code],
        )?;
        if rows == 0 {
            anyhow::bail!("未找到资产: {}", code);
        }
        // C4 价格历史积累：任何价格落库（update-prices 批量 / mns price 手动）
        // 同价 upsert 进 price_history，趋势锚的 12 个月倒计时由此开始走。
        // 同日重跑覆盖而非重复（当日终态语义，与 portfolio_snapshots 一致）。
        let today = Local::now().format("%Y-%m-%d").to_string();
        self.conn.execute(
            "INSERT INTO price_history (asset_code, price, price_date) VALUES (?, ?, ?)
             ON CONFLICT(asset_code, price_date) DO UPDATE SET price = excluded.price",
            params![code, price, today],
        )?;
        Ok(())
    }

    pub fn remove_position(&self, code: &str) -> Result<()> {
        let rows = self
            .conn
            .execute("DELETE FROM positions WHERE asset_code = ?", params![code])?;
        if rows == 0 {
            anyhow::bail!("未找到资产: {}", code);
        }
        println!("已删除资产: {}", code);
        Ok(())
    }

    // ── Transactions ──────────────────────────────

    pub fn list_transactions(&self, limit: i64) -> Result<Vec<Transaction>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, type, asset_code, shares, price, amount, tx_date, note
             FROM transactions ORDER BY id DESC LIMIT ?",
        )?;
        let rows = stmt.query_map([limit], transaction_from_row)?;
        let mut txs = Vec::new();
        for row in rows {
            txs.push(row?);
        }
        Ok(txs)
    }

    /// 读取 `from`（含，`YYYY-MM-DD`）之后的所有交易，按日期升序。
    ///
    /// 纪律复盘要按年统计「今年操作了几次」，`list_transactions` 的 limit
    /// 是行数，取 12 条只够看最近记录，会把全年笔数算少。这里不设上限，
    /// 也不改动 `list_transactions` 的语义——总览页的活动卡依赖那 12 条。
    pub fn list_transactions_since(&self, from: &str) -> Result<Vec<Transaction>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, type, asset_code, shares, price, amount, tx_date, note
             FROM transactions WHERE tx_date >= ? ORDER BY tx_date ASC, id ASC",
        )?;
        let rows = stmt.query_map([from], transaction_from_row)?;
        let mut txs = Vec::new();
        for row in rows {
            txs.push(row?);
        }
        Ok(txs)
    }

    /// 读取最近 `days` 天的恐贪快照 (日期, 分数)，按日期**升序**。
    ///
    /// 供 `signal::compute` 做平滑与区间确认；同一天只保留最新一条
    /// （写入端已按天 upsert）。
    pub fn recent_fear_greed_scores(&self, days: i64) -> Result<Vec<(chrono::NaiveDate, f64)>> {
        let cutoff = (Local::now().date_naive() - chrono::Duration::days(days.max(1)))
            .format("%Y-%m-%d")
            .to_string();
        let mut stmt = self.conn.prepare(
            "SELECT snapshot_date, score FROM fear_greed_snapshots
             WHERE snapshot_date > ? ORDER BY snapshot_date ASC",
        )?;
        let rows = stmt
            .query_map(params![cutoff], |row| {
                let date_str: String = row.get(0)?;
                let date = chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                        0, rusqlite::types::Type::Text, Box::new(e),
                    ))?;
                let score: f64 = row.get(1)?;
                Ok((date, score))
            })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 最新一条恐贪快照（含展示字段），供 CNN 获取失败时降级回退（W4）。
    pub fn latest_fear_greed_snapshot(&self) -> Result<Option<FearGreedSnapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT snapshot_date, score, rating, previous_close, previous_1_week,
                    previous_1_month, previous_1_year
             FROM fear_greed_snapshots
             ORDER BY snapshot_date DESC, id DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![], |row| {
            let date_str: String = row.get(0)?;
            let date = chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                    0, rusqlite::types::Type::Text, Box::new(e),
                ))?;
            Ok(FearGreedSnapshot {
                date,
                score: row.get(1)?,
                previous_close: row.get(3)?,
                previous_1_week: row.get(4)?,
                previous_1_month: row.get(5)?,
                previous_1_year: row.get(6)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    // ── Fear & Greed Snapshots ────────────────────

    pub fn save_fear_greed_snapshot(
        &self,
        score: f64,
        rating: &str,
        previous_close: Option<f64>,
        previous_1_week: Option<f64>,
        previous_1_month: Option<f64>,
        previous_1_year: Option<f64>,
    ) -> Result<()> {
        let today = Local::now().format("%Y-%m-%d").to_string();
        let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        // 同一天只保留最新快照：先删除当天已有记录，再插入
        self.conn.execute(
            "DELETE FROM fear_greed_snapshots WHERE snapshot_date = ?",
            params![today],
        )?;
        self.conn.execute(
            "INSERT INTO fear_greed_snapshots (score, rating, snapshot_date, previous_close, previous_1_week, previous_1_month, previous_1_year, fetched_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![score, rating, today, previous_close, previous_1_week, previous_1_month, previous_1_year, now],
        )?;
        Ok(())
    }
}

/// `transactions` 的行映射，`list_transactions*` 共用，避免两处列序漂移。
fn transaction_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Transaction> {
    Ok(Transaction {
        id: row.get(0)?,
        tx_type: row.get(1)?,
        asset_code: row.get(2)?,
        shares: row.get(3)?,
        price: row.get(4)?,
        amount: row.get(5)?,
        tx_date: row.get(6)?,
        note: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn seeded() -> Database {
        let db = Database {
            conn: Connection::open_in_memory().expect("in-memory db"),
        };
        db.init_tables().expect("schema");
        for (date, note) in [
            ("2025-12-31", Some("计划外冷静：去年")),
            ("2026-01-05", Some("计划外冷静：追高")),
            ("2026-03-09", Some("过热冷静：按计划分两批")),
            ("2026-03-09", None),
            ("2026-11-11", None),
        ] {
            db.conn
                .execute(
                    "INSERT INTO transactions (type, asset_code, shares, price, amount, tx_date, note)
                     VALUES ('buy', '007605', 1.0, 1.0, 1.0, ?, ?)",
                    params![date, note],
                )
                .expect("insert");
        }
        db
    }

    #[test]
    fn 按起始日期查询不设行数上限且保留备注() {
        let txs = seeded().list_transactions_since("2026-01-01").unwrap();
        // 2025 的记录被挡掉，2026 的 4 笔全部返回（远少于 list_transactions
        // 常见的 10/12 条窗口也不会被截断）。
        assert_eq!(txs.len(), 4);
        assert!(txs.iter().all(|t| t.tx_date.as_str() >= "2026-01-01"));
        assert_eq!(
            txs[0].note.as_deref(),
            Some("计划外冷静：追高"),
            "日期升序时第一条应是新年最早的一笔"
        );
        assert_eq!(txs[3].note, None, "空备注不能被写坏");
        assert_eq!(txs[3].tx_date, "2026-11-11");
    }

    #[test]
    fn 纪律复盘能从数据库流水直接分桶() {
        let txs = seeded().list_transactions_since("2026-01-01").unwrap();
        let review = crate::discipline::review(
            2026,
            10,
            20,
            txs.into_iter().filter_map(|t| {
                NaiveDate::parse_from_str(&t.tx_date, "%Y-%m-%d")
                    .ok()
                    .map(|d| (d, t.note))
            }),
        );
        assert_eq!(review.total, 4);
        assert_eq!(review.out_of_plan, 1);
        assert_eq!(review.hot_calm, 1);
        assert_eq!(review.unmarked, 2);
        assert_eq!(
            review.last_trade,
            NaiveDate::from_ymd_opt(2026, 11, 11)
        );
    }

    /// 文件库的测试脚手架：`open()` 绑定真实账户路径，测试必须落临时目录
    /// （与 `butler::store` 的测试同一约定）。
    fn scratch_db(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mns-db-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir.join("mns.db")
    }

    #[test]
    fn 可写连接打开后库处于_wal模式() {
        let path = scratch_db("wal");
        let db = Database::open_at(&path).expect("打开临时库");
        let mode: String = db
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("读 journal_mode");
        assert_eq!(mode.to_lowercase(), "wal", "journal_mode 应为 WAL");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn 只读连接能读但不能写() {
        let path = scratch_db("readonly");
        let db = Database::open_at(&path).expect("先建库");
        db.conn
            .execute(
                "INSERT INTO transactions (type, asset_code, shares, price, amount, tx_date)
                 VALUES ('buy', '007605', 1.0, 1.0, 1.0, '2026-11-19')",
                [],
            )
            .expect("可写连接插入");

        // 干净关闭（drop 即 checkpoint + 删 -wal/-shm）之后只读打开仍可读。
        drop(db);
        let reader = Database::open_read_only_at(&path).expect("只读打开");
        let count: i64 = reader
            .conn
            .query_row("SELECT COUNT(*) FROM transactions", [], |row| row.get(0))
            .expect("只读查询");
        assert_eq!(count, 1);
        let denied = reader.conn.execute(
            "INSERT INTO transactions (type, asset_code, shares, price, amount, tx_date)
             VALUES ('buy', '007605', 1.0, 1.0, 1.0, '2026-11-20')",
            [],
        );
        assert!(denied.is_err(), "只读连接必须拒绝写入");
        drop(reader);

        // 库不存在时只读打开失败：调用方按「暂无账本」处理，而不是拿到空库。
        let missing = scratch_db("missing");
        assert!(Database::open_read_only_at(&missing).is_err());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// P1 前置的并发契约：WAL 下「butlerd 正在读、GUI/CLI 正在写」不互相
    /// 报错。读方持续读，写方连续插入，双方都必须成功——这正是分期计划里
    /// 「GUI/CLI 写入时 butlerd 正在算 facts」并发测试的库层等价（SQLite
    /// 对同进程多连接与多进程采用同一套文件锁语义）。
    #[test]
    fn 写入进行中读取不报_busy() {
        let path = scratch_db("concurrent");
        let writer = Database::open_at(&path).expect("可写连接");
        writer
            .conn
            .execute(
                "INSERT INTO transactions (type, asset_code, shares, price, amount, tx_date)
                 VALUES ('buy', '007605', 1.0, 1.0, 1.0, '2026-11-19')",
                [],
            )
            .expect("写入第一批");

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let start = barrier.clone();
        let read_path = path.clone();
        let reader_handle = std::thread::spawn(move || {
            let conn = Connection::open_with_flags(
                &read_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .expect("读者连接");
            conn.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
            start.wait();
            for _ in 0..50 {
                let count: i64 = conn
                    .query_row("SELECT COUNT(*) FROM transactions", [], |row| row.get(0))
                    .expect("读取不得因并发写入报错");
                assert!(count >= 1);
            }
        });

        barrier.wait();
        for round in 0..50 {
            writer
                .conn
                .execute(
                    "INSERT INTO transactions (type, asset_code, shares, price, amount, tx_date)
                     VALUES ('buy', '007605', 1.0, 1.0, 1.0, '2026-11-20')",
                    [],
                )
                .unwrap_or_else(|error| panic!("第 {round} 轮写入不得因并发读取报错：{error}"));
        }
        reader_handle.join().expect("读者线程退出");
        drop(writer);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
