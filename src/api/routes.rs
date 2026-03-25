use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/session/create", post(create_session))
        .route("/session/{id}", delete(terminate_session))
        .route("/session/{id}/status", get(session_status))
        .route("/session/{id}/execute", post(execute_code))
        .route("/session/{id}/files", post(upload_file))
        .route("/session/{id}/output/{filename}", get(read_file))
        .route("/admin/keys", post(create_api_key))
        .route("/admin/keys", get(list_api_keys))
        .route("/admin/keys/{key}/revoke", post(revoke_api_key))
}

// ── Health ──────────────────────────────────────────────

async fn health(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let pool_size = state.sessions.warm_pool_size().await;
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "active_sessions": state.sessions.active_session_count(),
        "warm_pool": pool_size,
    }))
}

// ── Session Management ──────────────────────────────────

#[derive(Deserialize)]
struct CreateSessionRequest {
    ttl_seconds: Option<i64>,
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateSessionRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.sessions.create_session(body.ttl_seconds).await {
        Ok(info) => {
            tracing::info!(session_id = %info.session_id, "Session created");
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "session_id": info.session_id,
                    "created_at": info.created_at,
                    "expires_at": info.expires_at,
                    "workspace_path": info.workspace_path,
                })),
            )
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to create session");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("Failed to create session: {}", e),
                })),
            )
        }
    }
}

async fn terminate_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if state.sessions.terminate_session(&id) {
        (
            StatusCode::OK,
            Json(serde_json::json!({ "deleted": true })),
        )
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "session not found" })),
        )
    }
}

async fn session_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match state.sessions.get_session_status(&id) {
        Some(status) => Ok(Json(serde_json::json!(status))),
        None => Err(StatusCode::NOT_FOUND),
    }
}

// ── Code Execution ──────────────────────────────────────

#[derive(Deserialize)]
struct ExecuteRequest {
    code: String,
}

#[derive(Serialize)]
struct ExecuteResponse {
    stdout: String,
    result: Option<String>,
    error: Option<String>,
    duration_ms: u64,
}

async fn execute_code(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ExecuteRequest>,
) -> Result<Json<ExecuteResponse>, StatusCode> {
    let session = state
        .sessions
        .get_session(&id)
        .ok_or(StatusCode::NOT_FOUND)?;

    let mut runtime = session.runtime.lock().await;
    let result = runtime.execute(&body.code).await;

    Ok(Json(ExecuteResponse {
        stdout: result.stdout,
        result: result.result,
        error: result.error,
        duration_ms: result.duration_ms,
    }))
}

// ── File Operations ─────────────────────────────────────

#[derive(Deserialize)]
struct UploadFileRequest {
    filename: String,
    content: String, // base64 encoded
}

async fn upload_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UploadFileRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let session = state
        .sessions
        .get_session(&id)
        .ok_or(StatusCode::NOT_FOUND)?;

    // Validate filename — prevent path traversal
    let filename = std::path::Path::new(&body.filename)
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or(StatusCode::BAD_REQUEST)?
        .to_string();

    let content = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &body.content,
    )
    .map_err(|_| StatusCode::BAD_REQUEST)?;

    let file_path = session.workspace.join(&filename);
    tokio::fs::write(&file_path, &content)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "uploaded": filename,
        "size_bytes": content.len(),
    })))
}

async fn read_file(
    State(state): State<Arc<AppState>>,
    Path((id, filename)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let session = state
        .sessions
        .get_session(&id)
        .ok_or(StatusCode::NOT_FOUND)?;

    // Validate filename
    let safe_filename = std::path::Path::new(&filename)
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or(StatusCode::BAD_REQUEST)?;

    let file_path = session.workspace.join(safe_filename);

    if !file_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    let content = tokio::fs::read(&file_path)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let encoded = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &content,
    );

    Ok(Json(serde_json::json!({
        "filename": safe_filename,
        "content_base64": encoded,
        "size_bytes": content.len(),
    })))
}

// ── Admin ───────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateKeyRequest {
    name: String,
    tier: Option<String>,
}

async fn create_api_key(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateKeyRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), StatusCode> {
    let tier = body.tier.unwrap_or_else(|| "free".to_string());
    let key = state
        .db
        .create_api_key(&body.name, &tier)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "api_key": key,
            "name": body.name,
            "tier": tier,
        })),
    ))
}

async fn list_api_keys(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let keys = state.db.list_api_keys();
    Json(serde_json::json!({ "keys": keys }))
}

async fn revoke_api_key(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if state.db.revoke_api_key(&key) {
        (StatusCode::OK, Json(serde_json::json!({ "revoked": true, "key": key })))
    } else {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "key not found" })))
    }
}
