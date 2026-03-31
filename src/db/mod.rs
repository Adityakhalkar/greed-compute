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

            -- SAW: Shared Agent Workspaces
            CREATE TABLE IF NOT EXISTS workspaces (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                owner_api_key TEXT NOT NULL,
                checkpoint_path TEXT,
                created_at TEXT NOT NULL,
                last_accessed_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS workspace_members (
                workspace_id TEXT NOT NULL,
                api_key TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'member',
                added_at TEXT NOT NULL,
                PRIMARY KEY (workspace_id, api_key),
                FOREIGN KEY (workspace_id) REFERENCES workspaces(id)
            );

            CREATE INDEX IF NOT EXISTS idx_workspaces_owner ON workspaces(owner_api_key);
            CREATE INDEX IF NOT EXISTS idx_workspace_members_key ON workspace_members(api_key);

            -- Enterprise: detailed per-event usage tracking
            CREATE TABLE IF NOT EXISTS usage_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                api_key TEXT NOT NULL,
                event_type TEXT NOT NULL,
                session_id TEXT,
                swarm_id TEXT,
                duration_ms INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            -- Enterprise: daily aggregated counters (fast limit checks)
            CREATE TABLE IF NOT EXISTS daily_usage (
                api_key TEXT NOT NULL,
                date TEXT NOT NULL,
                exec_count INTEGER NOT NULL DEFAULT 0,
                total_duration_ms INTEGER NOT NULL DEFAULT 0,
                swarm_count INTEGER NOT NULL DEFAULT 0,
                install_count INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (api_key, date)
            );

            -- Enterprise: Stripe customer mapping
            CREATE TABLE IF NOT EXISTS stripe_customers (
                api_key TEXT PRIMARY KEY,
                stripe_customer_id TEXT NOT NULL,
                stripe_subscription_id TEXT,
                plan TEXT NOT NULL DEFAULT 'free',
                status TEXT NOT NULL DEFAULT 'active',
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_usage_events_key ON usage_events(api_key);
            CREATE INDEX IF NOT EXISTS idx_usage_events_created ON usage_events(created_at);
            CREATE INDEX IF NOT EXISTS idx_daily_usage_key ON daily_usage(api_key);
            "
        )?;

        // Additive column migrations — ALTER TABLE errors are silently ignored
        // because the column may already exist from a previous migration run.
        let _ = conn.execute_batch("ALTER TABLE api_keys ADD COLUMN stripe_customer_id TEXT;");
        let _ = conn.execute_batch("ALTER TABLE api_keys ADD COLUMN stripe_subscription_id TEXT;");

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

    // ── Enterprise: usage events ─────────────────────────────────────────────

    pub fn record_usage_event(
        &self,
        api_key: &str,
        event_type: &str,
        session_id: Option<&str>,
        swarm_id: Option<&str>,
        duration_ms: i64,
    ) {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "INSERT INTO usage_events (api_key, event_type, session_id, swarm_id, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![api_key, event_type, session_id, swarm_id, duration_ms],
        );
        // Upsert daily aggregation
        let _ = conn.execute(
            "INSERT INTO daily_usage (api_key, date, exec_count, total_duration_ms, swarm_count, install_count)
             VALUES (?1, ?2,
                 CASE WHEN ?3 IN ('execute','stream_execute','async_execute') THEN 1 ELSE 0 END,
                 ?4,
                 CASE WHEN ?3 = 'swarm' THEN 1 ELSE 0 END,
                 CASE WHEN ?3 = 'install' THEN 1 ELSE 0 END
             )
             ON CONFLICT(api_key, date) DO UPDATE SET
                 exec_count = exec_count + CASE WHEN ?3 IN ('execute','stream_execute','async_execute') THEN 1 ELSE 0 END,
                 total_duration_ms = total_duration_ms + ?4,
                 swarm_count = swarm_count + CASE WHEN ?3 = 'swarm' THEN 1 ELSE 0 END,
                 install_count = install_count + CASE WHEN ?3 = 'install' THEN 1 ELSE 0 END",
            params![api_key, today, event_type, duration_ms],
        );
    }

    pub fn get_daily_usage(&self, api_key: &str, date: &str) -> DailyUsage {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT exec_count, total_duration_ms, swarm_count, install_count
             FROM daily_usage WHERE api_key = ?1 AND date = ?2",
            params![api_key, date],
            |row| Ok(DailyUsage {
                exec_count: row.get(0)?,
                total_duration_ms: row.get(1)?,
                swarm_count: row.get(2)?,
                install_count: row.get(3)?,
            }),
        ).unwrap_or_default()
    }

    // ── Enterprise: Stripe ───────────────────────────────────────────────────

    pub fn upsert_stripe_customer(
        &self,
        api_key: &str,
        stripe_customer_id: &str,
        stripe_subscription_id: Option<&str>,
        plan: &str,
        status: &str,
    ) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "INSERT INTO stripe_customers (api_key, stripe_customer_id, stripe_subscription_id, plan, status, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))
             ON CONFLICT(api_key) DO UPDATE SET
                 stripe_customer_id = ?2,
                 stripe_subscription_id = ?3,
                 plan = ?4,
                 status = ?5,
                 updated_at = datetime('now')",
            params![api_key, stripe_customer_id, stripe_subscription_id, plan, status],
        );
        // Also upgrade the api_key tier
        let _ = conn.execute(
            "UPDATE api_keys SET tier = ?1 WHERE key = ?2",
            params![plan, api_key],
        );
    }

    pub fn get_stripe_customer(&self, api_key: &str) -> Option<StripeCustomer> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT api_key, stripe_customer_id, stripe_subscription_id, plan, status, updated_at
             FROM stripe_customers WHERE api_key = ?1",
            params![api_key],
            |row| Ok(StripeCustomer {
                api_key: row.get(0)?,
                stripe_customer_id: row.get(1)?,
                stripe_subscription_id: row.get(2)?,
                plan: row.get(3)?,
                status: row.get(4)?,
                updated_at: row.get(5)?,
            }),
        ).ok()
    }

    pub fn get_stripe_customer_by_stripe_id(&self, stripe_customer_id: &str) -> Option<StripeCustomer> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT api_key, stripe_customer_id, stripe_subscription_id, plan, status, updated_at
             FROM stripe_customers WHERE stripe_customer_id = ?1",
            params![stripe_customer_id],
            |row| Ok(StripeCustomer {
                api_key: row.get(0)?,
                stripe_customer_id: row.get(1)?,
                stripe_subscription_id: row.get(2)?,
                plan: row.get(3)?,
                status: row.get(4)?,
                updated_at: row.get(5)?,
            }),
        ).ok()
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

    /// Total bytes used by all checkpoints for this API key.
    pub fn checkpoint_storage_used(&self, api_key: &str) -> u64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM checkpoints WHERE api_key = ?1",
            params![api_key],
            |row| row.get::<_, i64>(0),
        ).unwrap_or(0) as u64
    }

    /// Return all checkpoints older than `retention_days` days, across all keys,
    /// grouped with their paths so the caller can delete the files.
    pub fn list_expired_checkpoints(&self, retention_days: u32) -> Vec<(String, String, String)> {
        let conn = self.conn.lock().unwrap();
        let cutoff = format!("-{} days", retention_days);
        let mut stmt = conn.prepare(
            "SELECT c.id, c.api_key, c.path FROM checkpoints c
             JOIN api_keys k ON c.api_key = k.key
             WHERE c.created_at < datetime('now', ?1)
             ORDER BY c.created_at ASC"
        ).unwrap();
        stmt.query_map(params![cutoff], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        }).unwrap().filter_map(|r| r.ok()).collect()
    }

    /// Delete checkpoint record by id only (used by retention cleanup — no key check).
    pub fn delete_checkpoint_by_id(&self, id: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute("DELETE FROM checkpoints WHERE id = ?1", params![id]);
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

    // ── Workspace CRUD ────────────────────────────────────────────────────────

    pub fn create_workspace(&self, id: &str, name: &str, owner_api_key: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO workspaces (id, name, owner_api_key, created_at, last_accessed_at) VALUES (?1, ?2, ?3, ?4, ?4)",
            params![id, name, owner_api_key, now],
        )?;
        // Owner is also a member
        conn.execute(
            "INSERT INTO workspace_members (workspace_id, api_key, role, added_at) VALUES (?1, ?2, 'owner', ?3)",
            params![id, owner_api_key, now],
        )?;
        Ok(())
    }

    pub fn get_workspace(&self, id: &str) -> Option<WorkspaceRecord> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, name, owner_api_key, checkpoint_path, created_at, last_accessed_at FROM workspaces WHERE id = ?1",
            params![id],
            workspace_from_row,
        ).ok()
    }

    pub fn list_workspaces(&self, api_key: &str) -> Vec<WorkspaceRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT w.id, w.name, w.owner_api_key, w.checkpoint_path, w.created_at, w.last_accessed_at
             FROM workspaces w
             JOIN workspace_members m ON w.id = m.workspace_id
             WHERE m.api_key = ?1
             ORDER BY w.last_accessed_at DESC"
        ).unwrap();
        stmt.query_map(params![api_key], workspace_from_row)
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    pub fn can_access_workspace(&self, workspace_id: &str, api_key: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM workspace_members WHERE workspace_id = ?1 AND api_key = ?2",
            params![workspace_id, api_key],
            |_| Ok(true),
        ).unwrap_or(false)
    }

    pub fn is_workspace_owner(&self, workspace_id: &str, api_key: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT 1 FROM workspaces WHERE id = ?1 AND owner_api_key = ?2",
            params![workspace_id, api_key],
            |_| Ok(true),
        ).unwrap_or(false)
    }

    pub fn add_workspace_member(&self, workspace_id: &str, api_key: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO workspace_members (workspace_id, api_key, role, added_at) VALUES (?1, ?2, 'member', ?3)",
            params![workspace_id, api_key, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn remove_workspace_member(&self, workspace_id: &str, api_key: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM workspace_members WHERE workspace_id = ?1 AND api_key = ?2 AND role != 'owner'",
            params![workspace_id, api_key],
        ).map(|n| n > 0).unwrap_or(false)
    }

    pub fn list_workspace_members(&self, workspace_id: &str) -> Vec<WorkspaceMember> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT api_key, role, added_at FROM workspace_members WHERE workspace_id = ?1 ORDER BY added_at"
        ).unwrap();
        stmt.query_map(params![workspace_id], |row| {
            Ok(WorkspaceMember { api_key: row.get(0)?, role: row.get(1)?, added_at: row.get(2)? })
        }).unwrap().filter_map(|r| r.ok()).collect()
    }

    pub fn update_workspace_checkpoint(&self, id: &str, checkpoint_path: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE workspaces SET checkpoint_path = ?1, last_accessed_at = ?2 WHERE id = ?3",
            params![checkpoint_path, Utc::now().to_rfc3339(), id],
        );
    }

    pub fn touch_workspace(&self, id: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE workspaces SET last_accessed_at = ?1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), id],
        );
    }

    pub fn delete_workspace(&self, id: &str, owner_api_key: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "DELETE FROM workspaces WHERE id = ?1 AND owner_api_key = ?2",
            params![id, owner_api_key],
        ).unwrap_or(0);
        if n > 0 {
            let _ = conn.execute("DELETE FROM workspace_members WHERE workspace_id = ?1", params![id]);
        }
        n > 0
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

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct DailyUsage {
    pub exec_count: i64,
    pub total_duration_ms: i64,
    pub swarm_count: i64,
    pub install_count: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StripeCustomer {
    pub api_key: String,
    pub stripe_customer_id: String,
    pub stripe_subscription_id: Option<String>,
    pub plan: String,
    pub status: String,
    pub updated_at: String,
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkspaceRecord {
    pub id: String,
    pub name: String,
    pub owner_api_key: String,
    pub checkpoint_path: Option<String>,
    pub created_at: String,
    pub last_accessed_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkspaceMember {
    pub api_key: String,
    pub role: String,
    pub added_at: String,
}

fn workspace_from_row(row: &rusqlite::Row) -> rusqlite::Result<WorkspaceRecord> {
    Ok(WorkspaceRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        owner_api_key: row.get(2)?,
        checkpoint_path: row.get(3)?,
        created_at: row.get(4)?,
        last_accessed_at: row.get(5)?,
    })
}
