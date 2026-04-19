use anyhow::{Context, Result};
use chrono::Local;
use rusqlite::{params, Connection};

use crate::models::{FearGreedSnapshot, Position, Transaction};

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
            INSERT OR IGNORE INTO cash (id, balance) VALUES (1, 0);",
        )?;
        Ok(())
    }

    // ── Cash ──────────────────────────────────────

    pub fn get_cash_balance(&self) -> Result<f64> {
        let balance: f64 = self
            .conn
            .query_row("SELECT balance FROM cash WHERE id = 1", [], |row| row.get(0))?;
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

    pub fn buy_position(&self, code: &str, shares: f64, price: f64) -> Result<()> {
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
        let tx = self.conn.unchecked_transaction()
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
            "INSERT INTO transactions (type, asset_code, shares, price, amount, tx_date) VALUES ('buy', ?, ?, ?, ?, ?)",
            params![code, shares, price, amount, today],
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
            anyhow::bail!("卖出份额超出持有量: 持有 {:.2}, 欲卖 {:.2}", pos.shares, shares);
        }

        let actual_shares = shares.min(pos.shares); // 防止浮点误差
        let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let today = Local::now().format("%Y-%m-%d").to_string();
        let amount = actual_shares * price;
        let new_shares = pos.shares - actual_shares;

        let balance = self.get_cash_balance()?;

        // 使用事务保证原子性
        let tx = self.conn.unchecked_transaction()
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
        Ok(())
    }

    // ── Transactions ──────────────────────────────

    pub fn list_transactions(&self, limit: i64) -> Result<Vec<Transaction>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, type, asset_code, shares, price, amount, tx_date, note
             FROM transactions ORDER BY id DESC LIMIT ?",
        )?;
        let rows = stmt.query_map([limit], |row| {
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
        })?;
        let mut txs = Vec::new();
        for row in rows {
            txs.push(row?);
        }
        Ok(txs)
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

    #[allow(dead_code)]
    pub fn get_latest_snapshot(&self) -> Result<Option<FearGreedSnapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, score, rating, snapshot_date, previous_close, previous_1_week, previous_1_month, previous_1_year, fetched_at
             FROM fear_greed_snapshots ORDER BY id DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map([], |row| {
            Ok(FearGreedSnapshot {
                id: row.get(0)?,
                score: row.get(1)?,
                rating: row.get(2)?,
                snapshot_date: row.get(3)?,
                previous_close: row.get(4)?,
                previous_1_week: row.get(5)?,
                previous_1_month: row.get(6)?,
                previous_1_year: row.get(7)?,
                fetched_at: row.get(8)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }
}
