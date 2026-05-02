mod api;
mod billing;
mod db;
mod runtime;
mod sandbox;

fn run_retention_cleanup(state: &std::sync::Arc<AppState>, checkpoint_dir: &std::path::Path) {
    // For each plan tier, delete checkpoints older than retention_days.
    // We check each key's tier individually so free/pro/enterprise get different windows.
    use crate::billing::PlanLimits;
    // Collect (id, api_key, path) for checkpoints that exceed their tier's retention window
    let free_expired  = state.db.list_expired_checkpoints(PlanLimits::for_tier("free").checkpoint_retention_days);
    let pro_expired   = state.db.list_expired_checkpoints(PlanLimits::for_tier("pro").checkpoint_retention_days);
    let ent_expired   = state.db.list_expired_checkpoints(PlanLimits::for_tier("enterprise").checkpoint_retention_days);

    // Build a set: only delete if the checkpoint's owner's tier matches the window
    let mut deleted = 0usize;
    for (id, api_key, path) in free_expired {
        let tier = state.db.validate_api_key(&api_key).map(|k| k.tier).unwrap_or_else(|| "free".into());
        if tier == "free" {
            let _ = std::fs::remove_file(&path);
            state.db.delete_checkpoint_by_id(&id);
            deleted += 1;
        }
    }
    for (id, api_key, path) in pro_expired {
        let tier = state.db.validate_api_key(&api_key).map(|k| k.tier).unwrap_or_else(|| "free".into());
        if tier == "pro" {
            let _ = std::fs::remove_file(&path);
            state.db.delete_checkpoint_by_id(&id);
            deleted += 1;
        }
    }
    for (id, api_key, path) in ent_expired {
        let tier = state.db.validate_api_key(&api_key).map(|k| k.tier).unwrap_or_else(|| "free".into());
        if tier == "enterprise" {
            let _ = std::fs::remove_file(&path);
            state.db.delete_checkpoint_by_id(&id);
            deleted += 1;
        }
    }

    if deleted > 0 {
        tracing::info!(deleted, "Retention cleanup: removed expired checkpoints");
    }
    let _ = checkpoint_dir; // used via paths in records
}

use axum::{middleware, Router};
use dashmap::DashMap;
use std::{collections::VecDeque, sync::Arc, time::Instant};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::api::workspace::WorkspaceMap;
use crate::db::Database;
use crate::sandbox::SessionManager;

pub struct AppState {
    pub sessions: SessionManager,
    pub db: Database,
    /// Sliding window rate limiter: api_key → timestamps of recent requests (last 60s)
    pub rate_windows: DashMap<String, VecDeque<Instant>>,
    /// Live workspace runtimes: workspace_id → locked PythonRuntime
    pub workspaces: WorkspaceMap,
    /// Whether token budget tracking is enabled server-wide.
    /// Set via GREED_TOKEN_TRACKING=false to disable. Default: true.
    pub token_tracking: bool,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "greed_compute=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let db = Database::new("greed-compute.db").expect("Failed to initialize database");
    db.migrate().expect("Failed to run database migrations");

    // Resolve worker path
    let worker_path = std::env::current_dir()
        .unwrap()
        .join("sandbox")
        .join("worker.py")
        .to_string_lossy()
        .to_string();

    // Resolve python path: prefer .venv/bin/python3, fall back to system python3
    let venv_python = std::env::current_dir()
        .unwrap()
        .join(".venv")
        .join("bin")
        .join("python3");
    let python_path = if venv_python.exists() {
        venv_python.to_string_lossy().to_string()
    } else {
        "python3".to_string()
    };

    tracing::info!(worker_path = %worker_path, python_path = %python_path, "Resolved Python paths");

    let sessions = SessionManager::new(worker_path, python_path);

    // Pre-fill warm pool at startup
    sessions.fill_warm_pool().await;

    // Pre-warm template pools in background (don't block startup)
    let sessions_arc = Arc::new(sessions);
    sessions_arc.spawn_template_warmup();
    let sessions = (*sessions_arc).clone();
    drop(sessions_arc);

    // Start TTL sweeper — kills expired sessions + refills warm pool
    let sweep_sessions = sessions.clone();
    tokio::spawn(async move {
        sweep_sessions.run_sweeper().await;
    });

    let token_tracking = std::env::var("GREED_TOKEN_TRACKING")
        .map(|v| v.to_lowercase() != "false" && v != "0")
        .unwrap_or(true);
    tracing::info!(token_tracking, "Token budget tracking");

    let state = Arc::new(AppState {
        sessions,
        db,
        rate_windows: DashMap::new(),
        workspaces: Arc::new(DashMap::new()),
        token_tracking,
    });

    // Grace-checkpoint + retention cleanup task
    let grace_state = state.clone();
    tokio::spawn(async move {
        let checkpoint_dir = std::path::PathBuf::from(
            std::env::var("GREED_CHECKPOINT_DIR")
                .unwrap_or_else(|_| "/tmp/greed-compute/checkpoints".into()),
        );
        let _ = std::fs::create_dir_all(&checkpoint_dir);
        let mut last_retention = std::time::Instant::now();

        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

            // ── Grace checkpoint expired sessions ─────────────────────────
            let expired = grace_state.sessions.drain_expired();
            if !expired.is_empty() {
                tracing::info!(count = expired.len(), "Processing expired sessions");
            }
            for (session_id, session) in expired {
                let calls = session.calls_used();
                if calls > 0 {
                    if let Some(ref key) = session.api_key {
                        let tier = grace_state.db.validate_api_key(key)
                            .map(|k| k.tier)
                            .unwrap_or_else(|| "free".into());
                        let limits = crate::billing::PlanLimits::for_tier(&tier);
                        let used = grace_state.db.checkpoint_storage_used(key);
                        if used < limits.checkpoint_storage_bytes {
                            let ckpt_id = uuid::Uuid::new_v4().to_string();
                            let path = checkpoint_dir.join(format!("{}.dill", ckpt_id));
                            let path_str = path.to_string_lossy().to_string();
                            if let Ok(mut rt) = session.runtime.try_lock() {
                                let (size, err) = rt.create_checkpoint(&path_str).await;
                                if err.is_none() && size > 0 {
                                    let name = format!("autosave-{}", &session_id[..8]);
                                    let _ = grace_state.db.create_checkpoint_record(
                                        &ckpt_id, key, &name, &path_str, size as i64,
                                    );
                                    tracing::info!(session_id, ckpt_id, size,
                                        "Grace checkpoint on expiry");
                                }
                            }
                        }
                    }
                }
                let _ = std::fs::remove_dir_all(&session.workspace);
            }

            // ── Hourly retention cleanup ──────────────────────────────────
            if last_retention.elapsed() >= std::time::Duration::from_secs(3600) {
                last_retention = std::time::Instant::now();
                run_retention_cleanup(&grace_state, &checkpoint_dir);
            }
        }
    });

    // Grace-checkpoint + retention cleanup task
    let grace_state = state.clone();
    tokio::spawn(async move {
        let checkpoint_dir = std::path::PathBuf::from(
            std::env::var("GREED_CHECKPOINT_DIR")
                .unwrap_or_else(|_| "/tmp/greed-compute/checkpoints".into()),
        );
        let _ = std::fs::create_dir_all(&checkpoint_dir);
        let mut last_retention = std::time::Instant::now();

        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

            // ── Grace checkpoint expired sessions ─────────────────────────
            let expired = grace_state.sessions.drain_expired();
            if !expired.is_empty() {
                tracing::info!(count = expired.len(), "Processing expired sessions");
            }
            for (session_id, session) in expired {
                let calls = session.calls_used();
                if calls > 0 {
                    if let Some(ref key) = session.api_key {
                        let tier = grace_state.db.validate_api_key(key)
                            .map(|k| k.tier)
                            .unwrap_or_else(|| "free".into());
                        let limits = crate::billing::PlanLimits::for_tier(&tier);
                        let used = grace_state.db.checkpoint_storage_used(key);
                        if used < limits.checkpoint_storage_bytes {
                            let ckpt_id = uuid::Uuid::new_v4().to_string();
                            let path = checkpoint_dir.join(format!("{}.dill", ckpt_id));
                            let path_str = path.to_string_lossy().to_string();
                            if let Ok(mut rt) = session.runtime.try_lock() {
                                let (size, err) = rt.create_checkpoint(&path_str).await;
                                if err.is_none() && size > 0 {
                                    let name = format!("autosave-{}", &session_id[..8]);
                                    let _ = grace_state.db.create_checkpoint_record(
                                        &ckpt_id, key, &name, &path_str, size as i64,
                                    );
                                    tracing::info!(session_id, ckpt_id, size,
                                        "Grace checkpoint on expiry");
                                }
                            }
                        }
                    }
                }
                let _ = std::fs::remove_dir_all(&session.workspace);
            }

            // ── Hourly retention cleanup ──────────────────────────────────
            if last_retention.elapsed() >= std::time::Duration::from_secs(3600) {
                last_retention = std::time::Instant::now();
                run_retention_cleanup(&grace_state, &checkpoint_dir);
            }
        }
    });

    let app = Router::new()
        .nest("/v1", api::routes::router())
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            api::auth::auth_middleware,
        ))
        .with_state(state);

    let addr = "0.0.0.0:8080";
    tracing::info!("greed-compute listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
