use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{sse::{Event, Sse}, Redirect},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, convert::Infallible, sync::Arc};
use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, StreamExt};

use crate::db::AuthError;
use crate::AppState;

static WEBHOOK_CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
fn webhook_client() -> &'static reqwest::Client {
    WEBHOOK_CLIENT.get_or_init(reqwest::Client::new)
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/auth/github", get(github_oauth_redirect))
        .route("/auth/github/callback", get(github_oauth_callback))
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/health", get(health))
        .route("/session/create", post(create_session))
        .route("/session/{id}", delete(terminate_session))
        .route("/session/{id}/status", get(session_status))
        .route("/session/{id}/execute", post(execute_code))
        .route("/session/{id}/execute/stream", post(execute_code_stream))
        .route("/session/{id}/install", post(install_packages))
        .route("/session/{id}/execute/async", post(execute_code_async))
        .route("/session/{id}/jobs", get(list_session_jobs))
        .route("/jobs/{id}", get(get_job))
        .route("/session/{id}/checkpoint", post(create_checkpoint))
        .route("/session/{id}/restore/{checkpoint_id}", post(restore_checkpoint))
        .route("/checkpoints", get(list_checkpoints))
        .route("/checkpoints/{id}", delete(delete_checkpoint))
        .route("/session/{id}/files", post(upload_file))
        .route("/session/{id}/output/{filename}", get(read_file))
        .route("/usage", get(get_usage))
        .route("/admin/keys", post(create_api_key))
        .route("/admin/keys", get(list_api_keys))
        .route("/admin/keys/{key}/revoke", post(revoke_api_key))
        .route("/swarm", post(crate::api::swarm::create_swarm))
        .route("/swarm/{id}", get(crate::api::swarm::get_swarm))
        .route("/mcp", post(crate::api::mcp::mcp_handler))
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
    /// Restore a saved checkpoint into the new session immediately
    checkpoint_id: Option<String>,
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
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

    // Restore checkpoint if requested
    let mut restore_vars: Option<Vec<String>> = None;
    let mut restore_error: Option<String> = None;
    if let Some(ref checkpoint_id) = body.checkpoint_id {
        let api_key = api_key_from_headers(&headers);
        if let Some(key) = api_key {
            if let Some(record) = state.db.get_checkpoint(checkpoint_id, &key) {
                if let Some(s) = state.sessions.get_session(&session.session_id) {
                    let mut runtime = s.runtime.lock().await;
                    let (vars, err) = runtime.restore_checkpoint(&record.path).await;
                    restore_vars = Some(vars);
                    restore_error = err;
                }
            } else {
                restore_error = Some(format!("Checkpoint '{}' not found", checkpoint_id));
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
            "restore_vars": restore_vars,
            "restore_error": restore_error,
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

// ── Background Jobs ─────────────────────────────────────

#[derive(Deserialize)]
struct AsyncExecuteRequest {
    code: String,
    webhook_url: Option<String>,
}

async fn execute_code_async(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AsyncExecuteRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let api_key = api_key_from_headers(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let session = state.sessions.get_session(&id).ok_or(StatusCode::NOT_FOUND)?;

    let job_id = uuid::Uuid::new_v4().to_string();
    state.db.create_job(&job_id, &id, &api_key, &body.code, body.webhook_url.as_deref())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Spawn background task — runs code and updates job record when done
    let state_clone = state.clone();
    let job_id_clone = job_id.clone();
    let code = body.code.clone();
    let webhook_url = body.webhook_url.clone();

    tokio::spawn(async move {
        state_clone.db.set_job_running(&job_id_clone);

        let mut runtime = session.runtime.lock().await;
        session.touch();
        let result = runtime.execute(&code).await;
        drop(runtime);

        let result_str = result.result.as_deref();
        let error_str = result.error.as_deref();

        state_clone.db.set_job_done(
            &job_id_clone,
            &result.stdout,
            result_str,
            error_str,
            &result.plots,
            result.html.as_deref(),
            result.duration_ms as i64,
        );

        // Fire webhook if provided
        if let Some(url) = webhook_url {
            let payload = serde_json::json!({
                "job_id": job_id_clone,
                "status": if error_str.is_some() { "error" } else { "done" },
                "stdout": result.stdout,
                "result": result.result,
                "error": result.error,
                "plots": result.plots,
                "html": result.html,
                "duration_ms": result.duration_ms,
            });
            let _ = webhook_client().post(&url).json(&payload).send().await;
        }
    });

    Ok(Json(serde_json::json!({
        "job_id": job_id,
        "status": "queued",
    })))
}

async fn get_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let api_key = api_key_from_headers(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let job = state.db.get_job(&id, &api_key).ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(serde_json::to_value(job).unwrap()))
}

async fn list_session_jobs(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let api_key = api_key_from_headers(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let jobs = state.db.list_session_jobs(&id, &api_key);
    Ok(Json(serde_json::json!({ "jobs": jobs })))
}

// ── Checkpointing ───────────────────────────────────────

fn checkpoint_dir() -> std::path::PathBuf {
    let base = std::env::var("GREED_CHECKPOINT_DIR")
        .unwrap_or_else(|_| "/tmp/greed-compute/checkpoints".to_string());
    std::path::PathBuf::from(base)
}

fn api_key_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

#[derive(Deserialize)]
struct CreateCheckpointRequest {
    name: Option<String>,
}

async fn create_checkpoint(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateCheckpointRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let api_key = api_key_from_headers(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let session = state.sessions.get_session(&id).ok_or(StatusCode::NOT_FOUND)?;

    let checkpoint_id = uuid::Uuid::new_v4().to_string();
    let name = body.name.unwrap_or_else(|| format!("checkpoint-{}", &checkpoint_id[..8]));

    let dir = checkpoint_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{}.dill", checkpoint_id));
    let path_str = path.to_string_lossy().to_string();

    let mut runtime = session.runtime.try_lock().map_err(|_| StatusCode::from_u16(423).unwrap())?;
    let (size_bytes, error) = runtime.create_checkpoint(&path_str).await;

    if let Some(err) = error {
        return Ok(Json(serde_json::json!({ "error": err })));
    }

    state.db.create_checkpoint_record(&checkpoint_id, &api_key, &name, &path_str, size_bytes as i64)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "checkpoint_id": checkpoint_id,
        "name": name,
        "size_bytes": size_bytes,
    })))
}

async fn restore_checkpoint(
    State(state): State<Arc<AppState>>,
    Path((id, checkpoint_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let api_key = api_key_from_headers(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let session = state.sessions.get_session(&id).ok_or(StatusCode::NOT_FOUND)?;

    let record = state.db.get_checkpoint(&checkpoint_id, &api_key)
        .ok_or(StatusCode::NOT_FOUND)?;

    let mut runtime = session.runtime.try_lock().map_err(|_| StatusCode::from_u16(423).unwrap())?;
    let (vars, error) = runtime.restore_checkpoint(&record.path).await;

    Ok(Json(serde_json::json!({
        "restored": error.is_none(),
        "vars": vars,
        "error": error,
    })))
}

async fn list_checkpoints(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let api_key = api_key_from_headers(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let checkpoints = state.db.list_checkpoints(&api_key);
    Ok(Json(serde_json::json!({ "checkpoints": checkpoints })))
}

async fn delete_checkpoint(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let api_key = api_key_from_headers(&headers).ok_or(StatusCode::UNAUTHORIZED)?;

    // Get path before deleting the record so we can remove the file
    let record = state.db.get_checkpoint(&id, &api_key).ok_or(StatusCode::NOT_FOUND)?;
    let _ = std::fs::remove_file(&record.path);

    if state.db.delete_checkpoint_record(&id, &api_key) {
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
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

// ── Usage ────────────────────────────────────────────────

async fn get_usage(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    let key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let key_info = state.db.validate_api_key(key);
    let tier = key_info.as_ref().map(|k| k.tier.as_str()).unwrap_or("free");

    let (req_limit, storage_limit_mb, retention_days): (i64, i64, i64) = match tier {
        "pro"        => (i64::MAX, 5120, 30),
        "enterprise" => (i64::MAX, i64::MAX, 365),
        _            => (100, 50, 1),
    };

    let used_today   = state.db.get_usage_today(key);
    let storage_bytes = state.db.get_checkpoint_storage_bytes(key);
    let storage_mb   = storage_bytes / (1024 * 1024);

    Json(serde_json::json!({
        "plan": tier,
        "requests": {
            "used": used_today,
            "limit": req_limit,
            "remaining": (req_limit - used_today).max(0)
        },
        "storage": {
            "used_mb": storage_mb,
            "limit_mb": storage_limit_mb,
            "remaining_mb": (storage_limit_mb - storage_mb).max(0)
        },
        "checkpoint_retention_days": retention_days
    }))
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

// ── Auth ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct AuthRequest {
    email: String,
    password: String,
}

async fn register(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AuthRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if body.email.is_empty() || !body.email.contains('@') {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Invalid email" })));
    }
    if body.password.len() < 8 {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Password must be at least 8 characters" })));
    }

    match state.db.register_user(&body.email, &body.password) {
        Ok(api_key) => (StatusCode::CREATED, Json(serde_json::json!({
            "api_key": api_key,
            "email": body.email,
            "plan": "free",
            "message": "Account created. Save your API key — it won't be shown again."
        }))),
        Err(AuthError::EmailTaken) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "Email already registered" })),
        ),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Registration failed" })),
        ),
    }
}

async fn login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AuthRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.db.login_user(&body.email, &body.password) {
        Ok(api_key) => (StatusCode::OK, Json(serde_json::json!({
            "api_key": api_key,
            "email": body.email,
        }))),
        Err(AuthError::InvalidCredentials) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "Invalid email or password" })),
        ),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Login failed" })),
        ),
    }
}

// ── GitHub OAuth ──────────────────────────────────────────

async fn github_oauth_redirect() -> Redirect {
    let client_id = std::env::var("GITHUB_CLIENT_ID").unwrap_or_default();
    let url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&scope=user:email",
        client_id
    );
    Redirect::to(&url)
}

async fn github_oauth_callback(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Redirect, (StatusCode, Json<serde_json::Value>)> {
    let code = params.get("code").cloned().unwrap_or_default();
    if code.is_empty() {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "Missing code"}))));
    }

    let client_id     = std::env::var("GITHUB_CLIENT_ID").unwrap_or_default();
    let client_secret = std::env::var("GITHUB_CLIENT_SECRET").unwrap_or_default();
    let frontend_url  = std::env::var("FRONTEND_URL")
        .unwrap_or_else(|_| "https://greed-compute-ui.vercel.app".to_string());

    let http = webhook_client();

    // Exchange code for access token
    let token_resp = http
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .json(&serde_json::json!({
            "client_id": client_id,
            "client_secret": client_secret,
            "code": code,
        }))
        .send()
        .await
        .map_err(|_| (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": "GitHub unreachable"}))))?;

    let token_json: serde_json::Value = token_resp.json().await
        .map_err(|_| (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": "Bad token response"}))))?;

    let access_token = token_json["access_token"].as_str()
        .unwrap_or("")
        .to_string();
    if access_token.is_empty() {
        return Err((StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "GitHub auth failed"}))));
    }

    // Fetch GitHub user info
    let user_resp = http
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("User-Agent", "greed-compute")
        .send()
        .await
        .map_err(|_| (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": "GitHub API unreachable"}))))?;

    let user_json: serde_json::Value = user_resp.json().await
        .map_err(|_| (StatusCode::BAD_GATEWAY, Json(serde_json::json!({"error": "Bad user response"}))))?;

    let github_id    = user_json["id"].as_i64().map(|n| n.to_string()).unwrap_or_default();
    let github_login = user_json["login"].as_str().unwrap_or("unknown").to_string();

    // Check account age — block accounts < 30 days old
    if let Some(created_at) = user_json["created_at"].as_str() {
        if let Ok(created) = chrono::DateTime::parse_from_rfc3339(created_at) {
            let age_days = (chrono::Utc::now() - created.with_timezone(&chrono::Utc)).num_days();
            if age_days < 30 {
                let redirect = format!("{}/auth/error?reason=account_too_new", frontend_url);
                return Ok(Redirect::to(&redirect));
            }
        }
    }

    // Fetch primary email
    let email_resp = http
        .get("https://api.github.com/user/emails")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("User-Agent", "greed-compute")
        .send()
        .await;

    let email = if let Ok(resp) = email_resp {
        let emails: serde_json::Value = resp.json().await.unwrap_or_default();
        emails.as_array()
            .and_then(|arr| arr.iter().find(|e| e["primary"].as_bool() == Some(true)))
            .and_then(|e| e["email"].as_str())
            .unwrap_or(&github_login)
            .to_string()
    } else {
        github_login.clone()
    };

    // Upsert user
    match state.db.upsert_github_user(&github_id, &github_login, &email) {
        Ok((api_key, _is_new)) => {
            let redirect = format!(
                "{}/dashboard?key={}&login={}",
                frontend_url, api_key, github_login
            );
            Ok(Redirect::to(&redirect))
        }
        Err(_) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Failed to create account"})),
        )),
    }
}
