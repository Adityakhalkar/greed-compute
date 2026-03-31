use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::runtime::PythonRuntime;

const DEFAULT_TTL_SECS: i64 = 900; // 15 minutes — dedicated VPS, suits notebook workflows
const SWEEPER_INTERVAL_SECS: u64 = 30;
const WARM_POOL_SIZE: usize = 3;

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
    /// Unix timestamp (seconds). Updated atomically on every execute call.
    expires_at_secs: AtomicI64,
    /// Total execute calls made on this session.
    calls_used: AtomicU64,
    pub runtime: Arc<Mutex<PythonRuntime>>,
    pub workspace: PathBuf,
    /// API key that owns this session — used for grace checkpointing on expiry.
    pub api_key: Option<String>,
}

impl Session {
    /// Reset the TTL to DEFAULT_TTL_SECS from now. Called on every execute.
    pub fn touch(&self) {
        let new_expiry = Utc::now().timestamp() + DEFAULT_TTL_SECS;
        self.expires_at_secs.store(new_expiry, Ordering::Relaxed);
        self.calls_used.fetch_add(1, Ordering::Relaxed);
    }

    pub fn expires_at(&self) -> DateTime<Utc> {
        let ts = self.expires_at_secs.load(Ordering::Relaxed);
        DateTime::from_timestamp(ts, 0).unwrap_or_else(Utc::now)
    }

    pub fn calls_used(&self) -> u64 {
        self.calls_used.load(Ordering::Relaxed)
    }
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

    pub async fn create_session_for_key(&self, ttl_secs: Option<i64>, api_key: Option<String>) -> Result<SessionInfo, String> {
        self.create_session_inner(ttl_secs, api_key).await
    }

    pub async fn create_session(&self, ttl_secs: Option<i64>) -> Result<SessionInfo, String> {
        self.create_session_inner(ttl_secs, None).await
    }

    async fn create_session_inner(&self, ttl_secs: Option<i64>, api_key: Option<String>) -> Result<SessionInfo, String> {
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
            expires_at_secs: AtomicI64::new(expires_at.timestamp()),
            calls_used: AtomicU64::new(0),
            runtime: Arc::new(Mutex::new(runtime)),
            workspace,
            api_key,
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
            let remaining = (s.expires_at() - Utc::now()).num_seconds().max(0);
            SessionStatus {
                active: true,
                ttl_remaining: remaining,
                calls_used: s.calls_used(),
                session_id: s.info.session_id.clone(),
            }
        })
    }

    pub fn active_session_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn worker_path(&self) -> String { self.worker_path.clone() }
    pub fn python_path(&self) -> String { self.python_path.clone() }

    pub async fn warm_pool_size(&self) -> usize {
        self.warm_pool.lock().await.len()
    }

    /// Drain expired sessions from the map and return them.
    /// Caller is responsible for grace-checkpointing before dropping.
    pub fn drain_expired(&self) -> Vec<(String, Arc<Session>)> {
        let now = Utc::now();
        let expired_ids: Vec<String> = self
            .sessions
            .iter()
            .filter(|entry| entry.value().expires_at() < now)
            .map(|entry| entry.key().clone())
            .collect();

        expired_ids.into_iter()
            .filter_map(|id| self.sessions.remove(&id).map(|(k, v)| (k, v)))
            .collect()
    }

    /// Background sweeper: refills warm pool (expiry is handled by grace-checkpoint task)
    pub async fn run_sweeper(&self) {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(SWEEPER_INTERVAL_SECS)).await;
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
