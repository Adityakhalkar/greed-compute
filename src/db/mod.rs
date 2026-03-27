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

            CREATE TABLE IF NOT EXISTS checkpoints (
                id TEXT PRIMARY KEY,
                api_key TEXT NOT NULL,
                name TEXT NOT NULL,
                path TEXT NOT NULL,
                created_at TEXT NOT NULL,
                size_bytes INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY (api_key) REFERENCES api_keys(key)
            );

            CREATE INDEX IF NOT EXISTS idx_usage_key ON usage(api_key);
            CREATE INDEX IF NOT EXISTS idx_usage_timestamp ON usage(timestamp);
            CREATE INDEX IF NOT EXISTS idx_checkpoints_key ON checkpoints(api_key);"
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

    pub fn revoke_api_key(&self, key: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "UPDATE api_keys SET is_active = 0 WHERE key = ?1",
            params![key],
        ).unwrap_or(0);
        rows > 0
    }

    pub fn list_api_keys(&self) -> Vec<ApiKeyInfo> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT key, name, tier, is_active FROM api_keys ORDER BY created_at DESC"
        ).unwrap();
        stmt.query_map([], |row| {
            Ok(ApiKeyInfo {
                key: row.get(0)?,
                name: row.get(1)?,
                tier: row.get(2)?,
                is_active: row.get(3)?,
            })
        }).unwrap().filter_map(|r| r.ok()).collect()
    }

    // ── Checkpoint CRUD ──────────────────────────────────────────────────────

    pub fn create_checkpoint_record(
        &self,
        id: &str,
        api_key: &str,
        name: &str,
        path: &str,
        size_bytes: i64,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO checkpoints (id, api_key, name, path, created_at, size_bytes) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, api_key, name, path, Utc::now().to_rfc3339(), size_bytes],
        )?;
        Ok(())
    }

    pub fn list_checkpoints(&self, api_key: &str) -> Vec<CheckpointInfo> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, api_key, name, path, created_at, size_bytes FROM checkpoints WHERE api_key = ?1 ORDER BY created_at DESC",
            )
            .unwrap();
        stmt.query_map(params![api_key], |row| {
            Ok(CheckpointInfo {
                id: row.get(0)?,
                api_key: row.get(1)?,
                name: row.get(2)?,
                path: row.get(3)?,
                created_at: row.get(4)?,
                size_bytes: row.get(5)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn get_checkpoint(&self, id: &str, api_key: &str) -> Option<CheckpointInfo> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, api_key, name, path, created_at, size_bytes FROM checkpoints WHERE id = ?1 AND api_key = ?2",
            params![id, api_key],
            |row| {
                Ok(CheckpointInfo {
                    id: row.get(0)?,
                    api_key: row.get(1)?,
                    name: row.get(2)?,
                    path: row.get(3)?,
                    created_at: row.get(4)?,
                    size_bytes: row.get(5)?,
                })
            },
        )
        .ok()
    }

    pub fn delete_checkpoint_record(&self, id: &str, api_key: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM checkpoints WHERE id = ?1 AND api_key = ?2",
            params![id, api_key],
        )
        .map(|rows| rows > 0)
        .unwrap_or(false)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ApiKeyInfo {
    pub key: String,
    pub name: String,
    pub tier: String,
    pub is_active: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckpointInfo {
    pub id: String,
    pub api_key: String,
    pub name: String,
    pub path: String,
    pub created_at: String,
    pub size_bytes: i64,
}
