use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::runtime::PythonRuntime;

const DEFAULT_TTL_SECS: i64 = 300; // 5 minutes
const SWEEPER_INTERVAL_SECS: u64 = 30;

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
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
        }
    }

    pub fn create_session(&self, ttl_secs: Option<i64>) -> SessionInfo {
        let session_id = Uuid::new_v4().to_string();
        let ttl = ttl_secs.unwrap_or(DEFAULT_TTL_SECS);
        let now = Utc::now();
        let expires_at = now + chrono::Duration::seconds(ttl);

        // Create temp workspace
        let workspace = std::env::temp_dir()
            .join("greed-compute")
            .join(&session_id);
        std::fs::create_dir_all(&workspace).unwrap();

        let info = SessionInfo {
            session_id: session_id.clone(),
            created_at: now,
            expires_at,
            calls_used: 0,
            workspace_path: workspace.to_string_lossy().to_string(),
        };

        let session = Arc::new(Session {
            info: info.clone(),
            runtime: Arc::new(Mutex::new(PythonRuntime::new())),
            workspace,
        });

        self.sessions.insert(session_id, session);
        info
    }

    pub fn get_session(&self, session_id: &str) -> Option<Arc<Session>> {
        self.sessions.get(session_id).map(|s| s.value().clone())
    }

    pub fn terminate_session(&self, session_id: &str) -> bool {
        if let Some((_, session)) = self.sessions.remove(session_id) {
            // Wipe workspace
            let _ = std::fs::remove_dir_all(&session.workspace);
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

    pub fn increment_calls(&self, _session_id: &str) {
        // TODO: track calls per session with AtomicU64
    }

    pub fn active_session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Background sweeper that kills expired sessions
    pub async fn run_sweeper(&self) {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(SWEEPER_INTERVAL_SECS)).await;

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
