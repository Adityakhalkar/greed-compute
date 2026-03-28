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

            CREATE TABLE IF NOT EXISTS jobs (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                api_key TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'queued',
                code TEXT NOT NULL,
                stdout TEXT,
                result TEXT,
                error TEXT,
                plots TEXT,
                html TEXT,
                webhook_url TEXT,
                created_at TEXT NOT NULL,
                started_at TEXT,
                finished_at TEXT,
                duration_ms INTEGER
            );

            CREATE INDEX IF NOT EXISTS idx_usage_key ON usage(api_key);
            CREATE INDEX IF NOT EXISTS idx_usage_timestamp ON usage(timestamp);
            CREATE INDEX IF NOT EXISTS idx_checkpoints_key ON checkpoints(api_key);
            CREATE INDEX IF NOT EXISTS idx_jobs_session ON jobs(session_id);
            CREATE INDEX IF NOT EXISTS idx_jobs_key ON jobs(api_key);"
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

    // ── Jobs CRUD ────────────────────────────────────────────────────────────

    pub fn create_job(
        &self,
        id: &str,
        session_id: &str,
        api_key: &str,
        code: &str,
        webhook_url: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO jobs (id, session_id, api_key, status, code, webhook_url, created_at)
             VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?6)",
            params![id, session_id, api_key, code, webhook_url, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn set_job_running(&self, id: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE jobs SET status = 'running', started_at = ?1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), id],
        );
    }

    pub fn set_job_done(
        &self,
        id: &str,
        stdout: &str,
        result: Option<&str>,
        error: Option<&str>,
        plots: &[String],
        html: Option<&str>,
        duration_ms: i64,
    ) {
        let conn = self.conn.lock().unwrap();
        let status = if error.is_some() { "error" } else { "done" };
        let plots_json = serde_json::to_string(plots).unwrap_or_else(|_| "[]".to_string());
        let _ = conn.execute(
            "UPDATE jobs SET status = ?1, stdout = ?2, result = ?3, error = ?4,
             plots = ?5, html = ?6, duration_ms = ?7, finished_at = ?8 WHERE id = ?9",
            params![
                status, stdout, result, error, plots_json, html,
                duration_ms, Utc::now().to_rfc3339(), id
            ],
        );
    }

    pub fn get_job(&self, id: &str, api_key: &str) -> Option<JobRecord> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, session_id, api_key, status, code, stdout, result, error,
             plots, html, webhook_url, created_at, started_at, finished_at, duration_ms
             FROM jobs WHERE id = ?1 AND api_key = ?2",
            params![id, api_key],
            job_from_row,
        ).ok()
    }

    pub fn list_session_jobs(&self, session_id: &str, api_key: &str) -> Vec<JobRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, api_key, status, code, stdout, result, error,
             plots, html, webhook_url, created_at, started_at, finished_at, duration_ms
             FROM jobs WHERE session_id = ?1 AND api_key = ?2 ORDER BY created_at DESC",
        ).unwrap();
        stmt.query_map(params![session_id, api_key], job_from_row)
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ApiKeyInfo {
    pub key: String,
    pub name: String,
    pub tier: String,
    pub is_active: bool,
}

fn job_from_row(row: &rusqlite::Row) -> rusqlite::Result<JobRecord> {
    let plots_json: Option<String> = row.get(8)?;
    let plots = plots_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default();
    Ok(JobRecord {
        id: row.get(0)?,
        session_id: row.get(1)?,
        api_key: row.get(2)?,
        status: row.get(3)?,
        code: row.get(4)?,
        stdout: row.get(5)?,
        result: row.get(6)?,
        error: row.get(7)?,
        plots,
        html: row.get(9)?,
        webhook_url: row.get(10)?,
        created_at: row.get(11)?,
        started_at: row.get(12)?,
        finished_at: row.get(13)?,
        duration_ms: row.get(14)?,
    })
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct JobRecord {
    pub id: String,
    pub session_id: String,
    pub api_key: String,
    pub status: String,
    pub code: String,
    pub stdout: Option<String>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub plots: Vec<String>,
    pub html: Option<String>,
    pub webhook_url: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub duration_ms: Option<i64>,
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
