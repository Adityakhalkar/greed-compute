use rusqlite::{Connection, params};
use std::sync::Mutex;
use chrono::Utc;

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn new(path: &str) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn migrate(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS api_keys (
                key TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                tier TEXT NOT NULL DEFAULT 'free',
                created_at TEXT NOT NULL,
                is_active INTEGER NOT NULL DEFAULT 1
            );

            CREATE TABLE IF NOT EXISTS usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                api_key TEXT NOT NULL,
                endpoint TEXT NOT NULL,
                duration_ms INTEGER,
                timestamp TEXT NOT NULL,
                FOREIGN KEY (api_key) REFERENCES api_keys(key)
            );

            CREATE INDEX IF NOT EXISTS idx_usage_key ON usage(api_key);
            CREATE INDEX IF NOT EXISTS idx_usage_timestamp ON usage(timestamp);"
        )?;
        Ok(())
    }

    pub fn validate_api_key(&self, key: &str) -> Option<ApiKeyInfo> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT key, name, tier, is_active FROM api_keys WHERE key = ?1",
            params![key],
            |row| {
                Ok(ApiKeyInfo {
                    key: row.get(0)?,
                    name: row.get(1)?,
                    tier: row.get(2)?,
                    is_active: row.get(3)?,
                })
            },
        ).ok().filter(|info| info.is_active)
    }

    pub fn record_usage(&self, api_key: &str, endpoint: &str, duration_ms: i64) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "INSERT INTO usage (api_key, endpoint, duration_ms, timestamp) VALUES (?1, ?2, ?3, ?4)",
            params![api_key, endpoint, duration_ms, Utc::now().to_rfc3339()],
        );
    }

    pub fn get_usage_count(&self, api_key: &str, since: &str) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM usage WHERE api_key = ?1 AND timestamp >= ?2",
            params![api_key, since],
            |row| row.get(0),
        ).unwrap_or(0)
    }

    pub fn create_api_key(&self, name: &str, tier: &str) -> Result<String, rusqlite::Error> {
        let key = format!("greed_{}", uuid::Uuid::new_v4().to_string().replace("-", ""));
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO api_keys (key, name, tier, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![key, name, tier, Utc::now().to_rfc3339()],
        )?;
        Ok(key)
    }
}

#[derive(Debug, Clone)]
pub struct ApiKeyInfo {
    pub key: String,
    pub name: String,
    pub tier: String,
    pub is_active: bool,
}
