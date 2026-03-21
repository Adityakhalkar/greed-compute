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

    let sessions = SessionManager::new();

    // Start TTL sweeper — kills expired sessions every 30s
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
