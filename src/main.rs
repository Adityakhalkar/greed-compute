mod api;
mod db;
mod runtime;
mod sandbox;

use axum::{middleware, Router};
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::db::Database;
use crate::sandbox::SessionManager;

pub struct AppState {
    pub sessions: SessionManager,
    pub db: Database,
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

    // Start TTL sweeper — kills expired sessions + refills warm pool
    let sweep_sessions = sessions.clone();
    tokio::spawn(async move {
        sweep_sessions.run_sweeper().await;
    });

    let state = Arc::new(AppState { sessions, db });

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
