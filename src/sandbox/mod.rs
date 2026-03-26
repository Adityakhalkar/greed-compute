use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::runtime::PythonRuntime;

const DEFAULT_TTL_SECS: i64 = 900; // 15 minutes — dedicated VPS, suits notebook workflows
const SWEEPER_INTERVAL_SECS: u64 = 30;
const WARM_POOL_SIZE: usize = 1;

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub calls_used: u64,
    pub workspace_path: String,
}

pub struct Session {
    pub info: SessionInfo,
    pub runtime: Arc<Mutex<PythonRuntime>>,
    pub workspace: PathBuf,
}

#[derive(Clone)]
pub struct SessionManager {
    sessions: Arc<DashMap<String, Arc<Session>>>,
    warm_pool: Arc<Mutex<Vec<PythonRuntime>>>,
    worker_path: String,
    python_path: String,
}

impl SessionManager {
    pub fn new(worker_path: String, python_path: String) -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            warm_pool: Arc::new(Mutex::new(Vec::with_capacity(WARM_POOL_SIZE))),
            worker_path,
            python_path,
        }
    }

    /// Pre-spawn workers into the warm pool. Call this at startup.
    pub async fn fill_warm_pool(&self) {
        let mut pool = self.warm_pool.lock().await;
        let needed = WARM_POOL_SIZE.saturating_sub(pool.len());

        if needed == 0 {
            return;
        }

        tracing::info!(needed, target = WARM_POOL_SIZE, "Filling warm pool");

        for i in 0..needed {
            // Warm workers use a shared temp workspace — will be reassigned on session create
            let warm_workspace = std::env::temp_dir()
                .join("greed-compute")
                .join("_warm_pool");
            let _ = std::fs::create_dir_all(&warm_workspace);

            match PythonRuntime::spawn(&warm_workspace, &self.worker_path, &self.python_path).await
            {
                Ok(runtime) => {
                    pool.push(runtime);
                    tracing::info!(worker = i + 1, "Warm worker spawned");
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to spawn warm worker");
                }
            }
        }

        tracing::info!(pool_size = pool.len(), "Warm pool ready");
    }

    /// Take a worker from the warm pool, or cold-spawn if pool is empty.
    async fn acquire_runtime(&self, workspace: &PathBuf) -> Result<PythonRuntime, String> {
        // Try warm pool first
        {
            let mut pool = self.warm_pool.lock().await;
            if let Some(mut runtime) = pool.pop() {
                // Clear any leftover state from previous use
                runtime.clear_state().await;
                tracing::info!(pool_remaining = pool.len(), "Assigned warm worker");
                return Ok(runtime);
            }
        }

        // Pool empty — cold spawn
        tracing::warn!("Warm pool empty, cold-spawning worker");
        PythonRuntime::spawn(workspace, &self.worker_path, &self.python_path).await
    }

    pub async fn create_session(&self, ttl_secs: Option<i64>) -> Result<SessionInfo, String> {
        let session_id = Uuid::new_v4().to_string();
        let ttl = ttl_secs.unwrap_or(DEFAULT_TTL_SECS);
        let now = Utc::now();
        let expires_at = now + chrono::Duration::seconds(ttl);

        let workspace = std::env::temp_dir()
            .join("greed-compute")
            .join(&session_id);
        std::fs::create_dir_all(&workspace)
            .map_err(|e| format!("Failed to create workspace directory: {}", e))?;

        let runtime = self.acquire_runtime(&workspace).await?;

        let info = SessionInfo {
            session_id: session_id.clone(),
            created_at: now,
            expires_at,
            calls_used: 0,
            workspace_path: workspace.to_string_lossy().to_string(),
        };

        let session = Arc::new(Session {
            info: info.clone(),
            runtime: Arc::new(Mutex::new(runtime)),
            workspace,
        });

        self.sessions.insert(session_id, session);
        Ok(info)
    }

    pub fn get_session(&self, session_id: &str) -> Option<Arc<Session>> {
        self.sessions.get(session_id).map(|s| s.value().clone())
    }

    pub fn terminate_session(&self, session_id: &str) -> bool {
        if let Some((_, session)) = self.sessions.remove(session_id) {
            let _ = std::fs::remove_dir_all(&session.workspace);
            // Worker process is killed by drop (kill_on_drop = true)
            tracing::info!(session_id, "Session terminated and workspace wiped");
            true
        } else {
            false
        }
    }

    pub fn get_session_status(&self, session_id: &str) -> Option<SessionStatus> {
        self.sessions.get(session_id).map(|s| {
            let remaining = (s.info.expires_at - Utc::now()).num_seconds().max(0);
            SessionStatus {
                active: true,
                ttl_remaining: remaining,
                calls_used: s.info.calls_used,
                session_id: s.info.session_id.clone(),
            }
        })
    }

    pub fn active_session_count(&self) -> usize {
        self.sessions.len()
    }

    pub async fn warm_pool_size(&self) -> usize {
        self.warm_pool.lock().await.len()
    }

    /// Background sweeper: kills expired sessions + refills warm pool
    pub async fn run_sweeper(&self) {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(SWEEPER_INTERVAL_SECS)).await;

            // Sweep expired sessions
            let now = Utc::now();
            let expired: Vec<String> = self
                .sessions
                .iter()
                .filter(|entry| entry.value().info.expires_at < now)
                .map(|entry| entry.key().clone())
                .collect();

            for session_id in &expired {
                self.terminate_session(session_id);
            }

            if !expired.is_empty() {
                tracing::info!(
                    count = expired.len(),
                    remaining = self.sessions.len(),
                    "Swept expired sessions"
                );
            }

            // Refill warm pool periodically
            let pool_size = self.warm_pool.lock().await.len();
            if pool_size < WARM_POOL_SIZE {
                self.fill_warm_pool().await;
            }
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct SessionStatus {
    pub active: bool,
    pub ttl_remaining: i64,
    pub calls_used: u64,
    pub session_id: String,
}
