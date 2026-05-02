use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::runtime::PythonRuntime;

const DEFAULT_TTL_SECS: i64 = 900; // 15 minutes
const SWEEPER_INTERVAL_SECS: u64 = 30;
const WARM_POOL_SIZE: usize = 3;

// ── Session templates ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SessionTemplate {
    DataScience,   // numpy pandas matplotlib scikit-learn scipy
    MachineLearning, // torch transformers datasets accelerate
    WebScraping,   // requests httpx beautifulsoup4 lxml
    Blank,         // nothing pre-installed (default)
}

impl SessionTemplate {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "data-science" | "data_science" | "datascience" => Some(Self::DataScience),
            "ml" | "machine-learning" | "machine_learning" => Some(Self::MachineLearning),
            "web" | "web-scraping" | "scraping" => Some(Self::WebScraping),
            "blank" | "" => Some(Self::Blank),
            _ => None,
        }
    }

    pub fn packages(&self) -> &[&str] {
        match self {
            Self::DataScience => &["numpy", "pandas", "matplotlib", "scikit-learn", "scipy"],
            Self::MachineLearning => &["torch", "transformers", "datasets", "accelerate"],
            Self::WebScraping => &["requests", "httpx", "beautifulsoup4", "lxml"],
            Self::Blank => &[],
        }
    }

    pub fn install_code(&self) -> Option<String> {
        let pkgs = self.packages();
        if pkgs.is_empty() { return None; }
        Some(format!(
            "import subprocess as _sp\n_sp.run(['pip','install','-q',{}], check=True)\ndel _sp",
            pkgs.iter().map(|p| format!("'{}'", p)).collect::<Vec<_>>().join(",")
        ))
    }

    pub fn name(&self) -> &str {
        match self {
            Self::DataScience => "data-science",
            Self::MachineLearning => "machine-learning",
            Self::WebScraping => "web-scraping",
            Self::Blank => "blank",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub calls_used: u64,
    pub workspace_path: String,
    pub template: Option<String>,
}

pub struct Session {
    pub info: SessionInfo,
    /// Unix timestamp (seconds). Updated atomically on every execute call.
    expires_at_secs: AtomicI64,
    /// Total execute calls made on this session.
    calls_used: AtomicU64,
    /// Running total of output_tokens across all execute calls this session.
    pub cumulative_output_tokens: AtomicU64,
    /// Running total of state_tokens (last reported value * call count proxy).
    /// We accumulate each call's state_tokens as a snapshot — the sum divided
    /// by calls_used gives the average state held while the session was live.
    pub cumulative_state_tokens: AtomicU64,
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

    /// Record token counts from one execute call.
    pub fn record_tokens(&self, output_tokens: u64, state_tokens: u64) {
        self.cumulative_output_tokens.fetch_add(output_tokens, Ordering::Relaxed);
        // Store the latest state snapshot (most recent is most accurate).
        self.cumulative_state_tokens.store(state_tokens, Ordering::Relaxed);
    }

    pub fn token_report(&self) -> TokenReport {
        let total_output = self.cumulative_output_tokens.load(Ordering::Relaxed);
        let peak_state = self.cumulative_state_tokens.load(Ordering::Relaxed);
        let calls = self.calls_used.load(Ordering::Relaxed).max(1);
        // Without sandbox: every call would need all output tokens + the full
        // state repeated each time. We use peak_state × calls as the proxy.
        let estimated_without_sandbox = total_output + peak_state * calls;
        TokenReport {
            total_output_tokens: total_output,
            peak_session_state_tokens: peak_state,
            estimated_tokens_without_sandbox: estimated_without_sandbox,
            net_token_savings: estimated_without_sandbox.saturating_sub(total_output),
            estimated_cost_savings_usd: (estimated_without_sandbox.saturating_sub(total_output) as f64) / 1_000_000.0 * 3.0,
        }
    }
}

#[derive(serde::Serialize)]
pub struct TokenReport {
    pub total_output_tokens: u64,
    pub peak_session_state_tokens: u64,
    pub estimated_tokens_without_sandbox: u64,
    pub net_token_savings: u64,
    /// Estimated USD savings at $3/1M tokens (claude-sonnet-4-6 input price)
    pub estimated_cost_savings_usd: f64,
}

#[derive(Clone)]
pub struct SessionManager {
    sessions: Arc<DashMap<String, Arc<Session>>>,
    warm_pool: Arc<Mutex<Vec<PythonRuntime>>>,
    /// Pre-warmed template pools: template name → ready runtimes with packages installed
    template_pools: Arc<DashMap<String, Arc<Mutex<Vec<PythonRuntime>>>>>,
    worker_path: String,
    python_path: String,
}

impl SessionManager {
    pub fn new(worker_path: String, python_path: String) -> Self {
        let template_pools: Arc<DashMap<String, Arc<Mutex<Vec<PythonRuntime>>>>> =
            Arc::new(DashMap::new());
        // Pre-create pool slots for each template
        for t in &[SessionTemplate::DataScience, SessionTemplate::MachineLearning, SessionTemplate::WebScraping] {
            template_pools.insert(t.name().to_string(), Arc::new(Mutex::new(Vec::new())));
        }
        Self {
            sessions: Arc::new(DashMap::new()),
            warm_pool: Arc::new(Mutex::new(Vec::with_capacity(WARM_POOL_SIZE))),
            template_pools,
            worker_path,
            python_path,
        }
    }

    /// Pre-warm one session per template at startup (background, don't block).
    pub fn spawn_template_warmup(self: &Arc<Self>) {
        for template in &[SessionTemplate::DataScience, SessionTemplate::MachineLearning, SessionTemplate::WebScraping] {
            let mgr = self.clone();
            let t = template.clone();
            tokio::spawn(async move {
                mgr.fill_template_pool(&t, 1).await;
            });
        }
    }

    async fn fill_template_pool(&self, template: &SessionTemplate, target: usize) {
        let pool_entry = match self.template_pools.get(template.name()) {
            Some(e) => e.clone(),
            None => return,
        };
        let current = pool_entry.lock().await.len();
        let needed = target.saturating_sub(current);
        if needed == 0 { return; }

        tracing::info!(template = template.name(), needed, "Pre-warming template pool");
        let workspace = std::env::temp_dir()
            .join("greed-compute")
            .join(format!("_template_{}", template.name()));
        let _ = std::fs::create_dir_all(&workspace);

        for _ in 0..needed {
            match PythonRuntime::spawn(&workspace, &self.worker_path, &self.python_path).await {
                Ok(mut runtime) => {
                    if let Some(code) = template.install_code() {
                        let res = runtime.execute(&code).await;
                        if res.error.is_some() {
                            tracing::warn!(template = template.name(), "Template install had error, discarding");
                            continue;
                        }
                    }
                    pool_entry.lock().await.push(runtime);
                    tracing::info!(template = template.name(), "Template worker ready");
                }
                Err(e) => tracing::error!(template = template.name(), error = %e, "Template spawn failed"),
            }
        }
    }

    async fn acquire_template_runtime(&self, template: &SessionTemplate, workspace: &PathBuf) -> Result<PythonRuntime, String> {
        if let Some(pool_entry) = self.template_pools.get(template.name()) {
            let mut pool = pool_entry.lock().await;
            if let Some(mut runtime) = pool.pop() {
                tracing::info!(template = template.name(), "Assigned pre-warmed template worker");
                // Wipe any leftover state but keep installed packages
                runtime.clear_state().await;
                // Refill pool in background
                let mgr_clone = SessionManager {
                    sessions: self.sessions.clone(),
                    warm_pool: self.warm_pool.clone(),
                    template_pools: self.template_pools.clone(),
                    worker_path: self.worker_path.clone(),
                    python_path: self.python_path.clone(),
                };
                let t = template.clone();
                tokio::spawn(async move { mgr_clone.fill_template_pool(&t, 1).await; });
                return Ok(runtime);
            }
        }
        // Pool empty — cold spawn + install
        tracing::warn!(template = template.name(), "Template pool empty, cold-spawning");
        let mut runtime = PythonRuntime::spawn(workspace, &self.worker_path, &self.python_path).await?;
        if let Some(code) = template.install_code() {
            runtime.execute(&code).await;
        }
        Ok(runtime)
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

    pub async fn create_session_for_key(&self, ttl_secs: Option<i64>, api_key: Option<String>, template: Option<SessionTemplate>) -> Result<SessionInfo, String> {
        self.create_session_inner(ttl_secs, api_key, template).await
    }

    pub async fn create_session(&self, ttl_secs: Option<i64>) -> Result<SessionInfo, String> {
        self.create_session_inner(ttl_secs, None, None).await
    }

    async fn create_session_inner(&self, ttl_secs: Option<i64>, api_key: Option<String>, template: Option<SessionTemplate>) -> Result<SessionInfo, String> {
        let session_id = Uuid::new_v4().to_string();
        let ttl = ttl_secs.unwrap_or(DEFAULT_TTL_SECS);
        let now = Utc::now();
        let expires_at = now + chrono::Duration::seconds(ttl);

        let workspace = std::env::temp_dir()
            .join("greed-compute")
            .join(&session_id);
        std::fs::create_dir_all(&workspace)
            .map_err(|e| format!("Failed to create workspace directory: {}", e))?;

        let runtime = match &template {
            Some(t) if *t != SessionTemplate::Blank => self.acquire_template_runtime(t, &workspace).await?,
            _ => self.acquire_runtime(&workspace).await?,
        };

        let info = SessionInfo {
            session_id: session_id.clone(),
            created_at: now,
            expires_at,
            calls_used: 0,
            workspace_path: workspace.to_string_lossy().to_string(),
            template: template.as_ref().map(|t| t.name().to_string()),
        };

        let session = Arc::new(Session {
            info: info.clone(),
            expires_at_secs: AtomicI64::new(expires_at.timestamp()),
            calls_used: AtomicU64::new(0),
            cumulative_output_tokens: AtomicU64::new(0),
            cumulative_state_tokens: AtomicU64::new(0),
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

    pub async fn template_pool_sizes(&self) -> std::collections::HashMap<String, usize> {
        let mut map = std::collections::HashMap::new();
        for entry in self.template_pools.iter() {
            map.insert(entry.key().clone(), entry.value().lock().await.len());
        }
        map
    }

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
