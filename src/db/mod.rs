use rusqlite::{Connection, params};
use std::sync::Mutex;
use chrono::Utc;

#[derive(Debug)]
pub enum AuthError {
    EmailTaken,
    InvalidCredentials,
    Internal,
}

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
            CREATE INDEX IF NOT EXISTS idx_jobs_key ON jobs(api_key);

            CREATE TABLE IF NOT EXISTS swarms (
                id TEXT PRIMARY KEY,
                api_key TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'running',
                total_workers INTEGER NOT NULL,
                completed_workers INTEGER NOT NULL DEFAULT 0,
                failed_workers INTEGER NOT NULL DEFAULT 0,
                template_checkpoint_id TEXT,
                reduce_stdout TEXT,
                reduce_result TEXT,
                reduce_error TEXT,
                webhook_url TEXT,
                created_at TEXT NOT NULL,
                finished_at TEXT
            );

            CREATE TABLE IF NOT EXISTS swarm_workers (
                id TEXT PRIMARY KEY,
                swarm_id TEXT NOT NULL,
                worker_index INTEGER NOT NULL,
                session_id TEXT,
                status TEXT NOT NULL DEFAULT 'pending',
                partition TEXT NOT NULL,
                stdout TEXT,
                result TEXT,
                error TEXT,
                plots TEXT,
                duration_ms INTEGER,
                started_at TEXT,
                finished_at TEXT,
                FOREIGN KEY (swarm_id) REFERENCES swarms(id)
            );

            CREATE INDEX IF NOT EXISTS idx_swarms_key ON swarms(api_key);
            CREATE INDEX IF NOT EXISTS idx_swarm_workers_swarm ON swarm_workers(swarm_id);

            CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                email TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                api_key TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY (api_key) REFERENCES api_keys(key)
            );

            CREATE UNIQUE INDEX IF NOT EXISTS idx_users_email ON users(email);"
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

    // ── Swarm CRUD ────────────────────────────────────────────────────────────

    pub fn create_swarm(
        &self, id: &str, api_key: &str, total: usize,
        template_checkpoint_id: Option<&str>, webhook_url: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO swarms (id, api_key, total_workers, template_checkpoint_id, webhook_url, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, api_key, total as i64, template_checkpoint_id, webhook_url, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn create_swarm_worker(
        &self, id: &str, swarm_id: &str, index: usize, partition_json: &str,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO swarm_workers (id, swarm_id, worker_index, partition) VALUES (?1, ?2, ?3, ?4)",
            params![id, swarm_id, index as i64, partition_json],
        )?;
        Ok(())
    }

    pub fn set_worker_running(&self, id: &str, session_id: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE swarm_workers SET status='running', session_id=?1, started_at=?2 WHERE id=?3",
            params![session_id, Utc::now().to_rfc3339(), id],
        );
    }

    pub fn set_worker_done(
        &self, id: &str, swarm_id: &str,
        stdout: &str, result: Option<&str>, error: Option<&str>,
        plots: &[String], duration_ms: i64,
    ) {
        let conn = self.conn.lock().unwrap();
        let status = if error.is_some() { "error" } else { "done" };
        let plots_json = serde_json::to_string(plots).unwrap_or_else(|_| "[]".into());
        let _ = conn.execute(
            "UPDATE swarm_workers SET status=?1, stdout=?2, result=?3, error=?4,
             plots=?5, duration_ms=?6, finished_at=?7 WHERE id=?8",
            params![status, stdout, result, error, plots_json, duration_ms, Utc::now().to_rfc3339(), id],
        );
        if error.is_some() {
            let _ = conn.execute(
                "UPDATE swarms SET failed_workers = failed_workers + 1 WHERE id=?1", params![swarm_id],
            );
        } else {
            let _ = conn.execute(
                "UPDATE swarms SET completed_workers = completed_workers + 1 WHERE id=?1", params![swarm_id],
            );
        }
    }

    pub fn finish_swarm(
        &self, id: &str,
        reduce_stdout: Option<&str>, reduce_result: Option<&str>, reduce_error: Option<&str>,
    ) {
        let conn = self.conn.lock().unwrap();
        let status = if reduce_error.is_some() { "error" } else { "done" };
        let _ = conn.execute(
            "UPDATE swarms SET status=?1, reduce_stdout=?2, reduce_result=?3,
             reduce_error=?4, finished_at=?5 WHERE id=?6",
            params![status, reduce_stdout, reduce_result, reduce_error, Utc::now().to_rfc3339(), id],
        );
    }

    pub fn get_swarm(&self, id: &str, api_key: &str) -> Option<SwarmRecord> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, api_key, status, total_workers, completed_workers, failed_workers,
             template_checkpoint_id, reduce_stdout, reduce_result, reduce_error,
             webhook_url, created_at, finished_at FROM swarms WHERE id=?1 AND api_key=?2",
            params![id, api_key],
            |row| Ok(SwarmRecord {
                id: row.get(0)?, api_key: row.get(1)?, status: row.get(2)?,
                total_workers: row.get(3)?, completed_workers: row.get(4)?,
                failed_workers: row.get(5)?, template_checkpoint_id: row.get(6)?,
                reduce_stdout: row.get(7)?, reduce_result: row.get(8)?,
                reduce_error: row.get(9)?, webhook_url: row.get(10)?,
                created_at: row.get(11)?, finished_at: row.get(12)?,
            }),
        ).ok()
    }

    pub fn get_swarm_workers(&self, swarm_id: &str) -> Vec<SwarmWorkerRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, swarm_id, worker_index, session_id, status, partition,
             stdout, result, error, plots, duration_ms, started_at, finished_at
             FROM swarm_workers WHERE swarm_id=?1 ORDER BY worker_index",
        ).unwrap();
        stmt.query_map(params![swarm_id], |row| {
            let plots_json: Option<String> = row.get(9)?;
            let plots = plots_json.as_deref()
                .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                .unwrap_or_default();
            Ok(SwarmWorkerRecord {
                id: row.get(0)?, swarm_id: row.get(1)?, worker_index: row.get(2)?,
                session_id: row.get(3)?, status: row.get(4)?, partition: row.get(5)?,
                stdout: row.get(6)?, result: row.get(7)?, error: row.get(8)?,
                plots, duration_ms: row.get(10)?, started_at: row.get(11)?,
                finished_at: row.get(12)?,
            })
        }).unwrap().filter_map(|r| r.ok()).collect()
    }

    // ── User Auth ────────────────────────────────────────────────────────────

    pub fn register_user(&self, email: &str, password: &str) -> Result<String, AuthError> {
        // Check if email already taken
        {
            let conn = self.conn.lock().unwrap();
            let exists: bool = conn.query_row(
                "SELECT COUNT(*) FROM users WHERE email = ?1",
                params![email],
                |row| row.get::<_, i64>(0),
            ).unwrap_or(0) > 0;
            if exists {
                return Err(AuthError::EmailTaken);
            }
        }

        let hash = bcrypt::hash(password, bcrypt::DEFAULT_COST)
            .map_err(|_| AuthError::Internal)?;

        let user_id = uuid::Uuid::new_v4().to_string();
        let api_key = format!("gc_{}", uuid::Uuid::new_v4().to_string().replace("-", ""));
        let now = Utc::now().to_rfc3339();

        let conn = self.conn.lock().unwrap();
        // Create the api_key record first
        conn.execute(
            "INSERT INTO api_keys (key, name, tier, created_at) VALUES (?1, ?2, 'free', ?3)",
            params![api_key, email, now],
        ).map_err(|_| AuthError::Internal)?;
        // Create the user
        conn.execute(
            "INSERT INTO users (id, email, password_hash, api_key, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![user_id, email, hash, api_key, now],
        ).map_err(|_| AuthError::Internal)?;

        Ok(api_key)
    }

    pub fn login_user(&self, email: &str, password: &str) -> Result<String, AuthError> {
        let conn = self.conn.lock().unwrap();
        let (hash, api_key): (String, String) = conn.query_row(
            "SELECT password_hash, api_key FROM users WHERE email = ?1",
            params![email],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).map_err(|_| AuthError::InvalidCredentials)?;

        let valid = bcrypt::verify(password, &hash).map_err(|_| AuthError::Internal)?;
        if !valid {
            return Err(AuthError::InvalidCredentials);
        }
        Ok(api_key)
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
pub struct SwarmRecord {
    pub id: String,
    pub api_key: String,
    pub status: String,
    pub total_workers: i64,
    pub completed_workers: i64,
    pub failed_workers: i64,
    pub template_checkpoint_id: Option<String>,
    pub reduce_stdout: Option<String>,
    pub reduce_result: Option<String>,
    pub reduce_error: Option<String>,
    pub webhook_url: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SwarmWorkerRecord {
    pub id: String,
    pub swarm_id: String,
    pub worker_index: i64,
    pub session_id: Option<String>,
    pub status: String,
    pub partition: String,
    pub stdout: Option<String>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub plots: Vec<String>,
    pub duration_ms: Option<i64>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
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
