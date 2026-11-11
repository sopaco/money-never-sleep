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

impl Database {
    pub fn open() -> Result<Self> {
        let path = crate::config::AppConfig::db_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)
            .with_context(|| format!("打开数据库失败: {}", path.display()))?;
        let db = Self { conn };
        db.init_tables()?;
        db.ensure_cash_flow_opening()?;
        Ok(db)
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

        // 先检查现金余额
        let balance = self.get_cash_balance()?;
        if balance < amount {
            anyhow::bail!("现金余额不足: 当前 ¥{:.2}, 需要 ¥{:.2}", balance, amount);
        }

        // 更新持仓：加权平均成本
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

        // 使用事务保证原子性
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

        // 使用事务保证原子性
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
}
