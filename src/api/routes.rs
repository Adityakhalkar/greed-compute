use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, Sse},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, sync::Arc};
use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, StreamExt};

use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/session/create", post(create_session))
        .route("/session/{id}", delete(terminate_session))
        .route("/session/{id}/status", get(session_status))
        .route("/session/{id}/execute", post(execute_code))
        .route("/session/{id}/execute/stream", post(execute_code_stream))
        .route("/session/{id}/install", post(install_packages))
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
    /// Packages to pip install before the session is ready
    packages: Option<Vec<String>>,
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateSessionRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let session = match state.sessions.create_session(body.ttl_seconds).await {
        Ok(info) => info,
        Err(e) => {
            tracing::error!(error = %e, "Failed to create session");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to create session: {}", e) })),
            );
        }
    };

    // Pre-install packages if requested
    let mut install_output = None;
    let mut install_error = None;
    if let Some(packages) = &body.packages {
        if !packages.is_empty() {
            tracing::info!(session_id = %session.session_id, packages = ?packages, "Pre-installing packages");
            if let Some(s) = state.sessions.get_session(&session.session_id) {
                let mut runtime = s.runtime.lock().await;
                let (stdout, error, _) = runtime.install_packages(packages).await;
                install_output = Some(stdout);
                install_error = error;
            }
        }
    }

    tracing::info!(session_id = %session.session_id, "Session created");
    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "session_id": session.session_id,
            "created_at": session.created_at,
            "expires_at": session.expires_at,
            "workspace_path": session.workspace_path,
            "install_output": install_output,
            "install_error": install_error,
        })),
    )
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
    /// Base64-encoded PNG images from matplotlib plt.show() calls
    plots: Vec<String>,
    /// HTML table when the last expression was a DataFrame or Series
    html: Option<String>,
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

    // Non-blocking try_lock — if the session is already executing, return 423
    // immediately instead of queuing. Matches Jupyter's "kernel busy" behavior.
    let mut runtime = session.runtime.try_lock().map_err(|_| StatusCode::from_u16(423).unwrap())?;
    let result = runtime.execute(&body.code).await;

    // Renew TTL on every execute — keeps active notebook sessions alive
    session.touch();

    Ok(Json(ExecuteResponse {
        stdout: result.stdout,
        result: result.result,
        error: result.error,
        duration_ms: result.duration_ms,
        plots: result.plots,
        html: result.html,
    }))
}

async fn execute_code_stream(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ExecuteRequest>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let session = state.sessions.get_session(&id).ok_or(StatusCode::NOT_FOUND)?;

    // 423 if already executing — check then immediately release so the spawned task can lock
    if session.runtime.try_lock().is_err() {
        return Err(StatusCode::from_u16(423).unwrap());
    }

    let (tx, rx) = mpsc::channel::<String>(64);

    let session_clone = session.clone();
    let code = body.code.clone();

    tokio::spawn(async move {
        let mut runtime = session_clone.runtime.lock().await;
        let result = runtime.execute_streaming(&code, tx.clone()).await;
        session_clone.touch();
        // Send the final result as the last SSE event
        let final_json = serde_json::json!({
            "type": "result",
            "stdout": result.stdout,
            "error": result.error,
            "duration_ms": result.duration_ms,
            "plots": result.plots,
            "html": result.html,
        });
        let _ = tx.send(final_json.to_string()).await;
    });

    let stream = ReceiverStream::new(rx).map(|line| {
        Ok::<Event, Infallible>(Event::default().data(line))
    });

    Ok(Sse::new(stream))
}

// ── Package Installation ─────────────────────────────────

#[derive(Deserialize)]
struct InstallRequest {
    packages: Vec<String>,
}

async fn install_packages(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<InstallRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let session = state.sessions.get_session(&id).ok_or(StatusCode::NOT_FOUND)?;

    let mut runtime = session.runtime.try_lock().map_err(|_| StatusCode::from_u16(423).unwrap())?;
    let (stdout, error, duration_ms) = runtime.install_packages(&body.packages).await;

    Ok(Json(serde_json::json!({
        "stdout": stdout,
        "error": error,
        "duration_ms": duration_ms,
    })))
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
