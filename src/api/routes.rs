use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::sse::{Event, Sse},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, sync::Arc};
use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, StreamExt};

use crate::AppState;

static WEBHOOK_CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
fn webhook_client() -> &'static reqwest::Client {
    WEBHOOK_CLIENT.get_or_init(reqwest::Client::new)
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/session/create", post(create_session))
        .route("/session/{id}", delete(terminate_session))
        .route("/session/{id}/status", get(session_status))
        .route("/session/{id}/execute", post(execute_code))
        .route("/session/{id}/execute/stream", post(execute_code_stream))
        .route("/session/{id}/token-report", get(session_token_report))
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
        .route("/admin/keys", post(create_api_key))
        .route("/admin/keys", get(list_api_keys))
        .route("/admin/keys/{key}/revoke", post(revoke_api_key))
        .route("/swarm", post(crate::api::swarm::create_swarm))
        .route("/swarm/{id}", get(crate::api::swarm::get_swarm))
        .route("/mcp", post(crate::api::mcp::mcp_handler))
        // ── Enterprise billing ─────────────────────────────────────────────
        .route("/billing/checkout", post(crate::api::billing::create_checkout))
        .route("/billing/portal", post(crate::api::billing::create_portal))
        .route("/billing/webhook", post(crate::api::billing::stripe_webhook))
        .route("/usage", get(crate::api::billing::get_usage))
        // ── SAW: Shared Agent Workspaces ───────────────────────────────────
        .route("/workspaces", post(crate::api::workspace::create_workspace))
        .route("/workspaces", get(crate::api::workspace::list_workspaces))
        .route("/workspaces/{id}", get(crate::api::workspace::get_workspace))
        .route("/workspaces/{id}", delete(crate::api::workspace::delete_workspace))
        .route("/workspaces/{id}/execute", post(crate::api::workspace::execute_in_workspace))
        .route("/workspaces/{id}/invite", post(crate::api::workspace::invite_member))
        .route("/workspaces/{id}/members/{member_key}", delete(crate::api::workspace::kick_member))
        // ── cuntext: LLM-native API discovery ─────────────────────────────
        .route("/cuntext/index.cuntext", get(cuntext_index))
        .route("/cuntext/fragments/{name}", get(cuntext_fragment))
        .route("/llms.cuntext", get(cuntext_index))
        // ── GitHub OAuth ──────────────────────────────────────────────
        .route("/auth/github", get(github_oauth_start))
        .route("/auth/github/callback", get(github_oauth_callback))
}

// ── Health ──────────────────────────────────────────────

async fn health(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let pool_size = state.sessions.warm_pool_size().await;
    let template_pools = state.sessions.template_pool_sizes().await;
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "active_sessions": state.sessions.active_session_count(),
        "warm_pool": pool_size,
        "template_pools": template_pools,
        "templates": ["data-science", "machine-learning", "web-scraping", "blank"],
    }))
}

// ── Session Management ──────────────────────────────────

#[derive(Deserialize)]
struct CreateSessionRequest {
    ttl_seconds: Option<i64>,
    /// Pre-built template: "data-science", "machine-learning", "web-scraping", "blank"
    template: Option<String>,
    /// Packages to pip install before the session is ready
    packages: Option<Vec<String>>,
    /// Restore a saved checkpoint into the new session immediately
    checkpoint_id: Option<String>,
    /// Override server-wide token tracking for this session.
    /// Omit to inherit the server default (GREED_TOKEN_TRACKING env var).
    token_tracking: Option<bool>,
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateSessionRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    use crate::sandbox::SessionTemplate;
    let api_key = headers.get("x-api-key").and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    let template = body.template.as_deref()
        .and_then(SessionTemplate::from_str)
        .unwrap_or(SessionTemplate::Blank);
    let token_tracking = body.token_tracking.unwrap_or(state.token_tracking);
    let session = match state.sessions.create_session_for_key(body.ttl_seconds, api_key, Some(template), token_tracking).await {
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
struct ExecuteTokenInfo {
    output_tokens: u64,
    session_state_tokens: u64,
    estimated_context_tokens_without_sandbox: u64,
    cumulative_session_savings: u64,
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
    /// Present only when token tracking is enabled for this session.
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens: Option<ExecuteTokenInfo>,
}

async fn execute_code(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
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
    drop(runtime);

    // Token tracking: conditionally accumulate and expose.
    let tokens = if session.token_tracking {
        session.record_tokens(result.output_tokens, result.state_tokens);
        let report = session.token_report();
        Some(ExecuteTokenInfo {
            output_tokens: result.output_tokens,
            session_state_tokens: result.state_tokens,
            estimated_context_tokens_without_sandbox: result.output_tokens + result.state_tokens,
            cumulative_session_savings: report.net_token_savings,
        })
    } else {
        None
    };

    // Renew TTL on every execute — keeps active notebook sessions alive
    session.touch();

    // Record usage event — token columns are 0 when tracking is disabled
    let (out_tok, state_tok) = if session.token_tracking {
        (result.output_tokens, result.state_tokens)
    } else {
        (0, 0)
    };
    if let Some(key) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        state.db.record_usage_event(key, "execute", Some(&id), None, result.duration_ms as i64, out_tok, state_tok);
    }

    Ok(Json(ExecuteResponse {
        stdout: result.stdout,
        result: result.result,
        error: result.error,
        duration_ms: result.duration_ms,
        plots: result.plots,
        html: result.html,
        tokens,
    }))
}

async fn session_token_report(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<crate::sandbox::TokenReport>, StatusCode> {
    let session = state.sessions.get_session(&id).ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(session.token_report()))
}

async fn execute_code_stream(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
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
    let state_clone = state.clone();
    let api_key = headers.get("x-api-key").and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    let session_id = id.clone();

    tokio::spawn(async move {
        let mut runtime = session_clone.runtime.lock().await;
        let result = runtime.execute_streaming(&code, tx.clone()).await;

        let (out_tok, state_tok) = if session_clone.token_tracking {
            session_clone.record_tokens(result.output_tokens, result.state_tokens);
            (result.output_tokens, result.state_tokens)
        } else {
            (0, 0)
        };
        session_clone.touch();

        if let Some(key) = api_key {
            state_clone.db.record_usage_event(&key, "stream_execute", Some(&session_id), None, result.duration_ms as i64, out_tok, state_tok);
        }
        // Send the final result as the last SSE event
        let mut final_val = serde_json::json!({
            "type": "result",
            "stdout": result.stdout,
            "error": result.error,
            "duration_ms": result.duration_ms,
            "plots": result.plots,
            "html": result.html,
        });
        if session_clone.token_tracking {
            let report = session_clone.token_report();
            final_val["tokens"] = serde_json::json!({
                "output_tokens": result.output_tokens,
                "session_state_tokens": result.state_tokens,
                "estimated_context_tokens_without_sandbox": result.output_tokens + result.state_tokens,
                "cumulative_session_savings": report.net_token_savings,
            });
        }
        let final_json = final_val;
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
    use crate::billing::PlanLimits;

    let api_key = api_key_from_headers(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let session = state.sessions.get_session(&id).ok_or(StatusCode::NOT_FOUND)?;

    // ── Storage quota check ───────────────────────────────────────────────────
    let key_info = state.db.validate_api_key(&api_key).ok_or(StatusCode::UNAUTHORIZED)?;
    let limits = PlanLimits::for_tier(&key_info.tier);
    let used = state.db.checkpoint_storage_used(&api_key);
    if used >= limits.checkpoint_storage_bytes {
        let used_mb = used / (1024 * 1024);
        let limit_mb = limits.checkpoint_storage_bytes / (1024 * 1024);
        return Ok(Json(serde_json::json!({
            "error": "Checkpoint storage quota exceeded",
            "used_mb": used_mb,
            "limit_mb": limit_mb,
            "plan": key_info.tier,
            "fix": "Delete old checkpoints or upgrade your plan",
            "upgrade": "https://compute.deep-ml.com/billing"
        })));
    }

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

    let retention_days = limits.checkpoint_retention_days;
    Ok(Json(serde_json::json!({
        "checkpoint_id": checkpoint_id,
        "name": name,
        "size_bytes": size_bytes,
        "expires_in_days": retention_days,
        "storage_used_mb": (used + size_bytes) / (1024 * 1024),
        "storage_limit_mb": limits.checkpoint_storage_bytes / (1024 * 1024),
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

// ── cuntext handlers ──────────────────────────────────────────────────────────

use axum::response::Response;
use axum::http::header;

const CUNTEXT_INDEX: &str = include_str!("../../docs/cuntext/index.cuntext");

async fn cuntext_index() -> Response {
    axum::response::Response::builder()
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .body(axum::body::Body::from(CUNTEXT_INDEX))
        .unwrap()
}

// ── GitHub OAuth ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct OAuthStartQuery {
    redirect_uri: Option<String>,
}

#[derive(Deserialize)]
struct OAuthCallbackQuery {
    code: String,
    state: Option<String>,
}

#[derive(Deserialize)]
struct GitHubTokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct GitHubUser {
    login: String,
}

async fn github_oauth_start(
    axum::extract::Query(query): axum::extract::Query<OAuthStartQuery>,
) -> axum::response::Redirect {
    let client_id = std::env::var("GITHUB_CLIENT_ID").unwrap_or_default();
    let callback = format!(
        "{}/v1/auth/github/callback",
        std::env::var("GREED_BASE_URL").unwrap_or_else(|_| "http://localhost:8080".into())
    );

    // If a CLI redirect_uri is provided, pass it as the OAuth state parameter
    // so we can redirect back to the CLI after auth
    let state_param = query.redirect_uri.unwrap_or_default();

    let url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope=read:user&state={}",
        client_id, callback, state_param
    );
    axum::response::Redirect::temporary(&url)
}

async fn github_oauth_callback(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<OAuthCallbackQuery>,
) -> axum::response::Redirect {
    let frontend_url = std::env::var("FRONTEND_URL")
        .unwrap_or_else(|_| "http://localhost:3000".into());

    let client_id = std::env::var("GITHUB_CLIENT_ID").unwrap_or_default();
    let client_secret = std::env::var("GITHUB_CLIENT_SECRET").unwrap_or_default();

    let token_res = match reqwest::Client::new()
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .json(&serde_json::json!({
            "client_id": client_id,
            "client_secret": client_secret,
            "code": query.code,
        }))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("GitHub token exchange failed: {}", e);
            return axum::response::Redirect::temporary(
                &format!("{}/auth/error?reason=github_error", frontend_url)
            );
        }
    };

    let token: GitHubTokenResponse = match token_res.json().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("Failed to parse GitHub token response: {}", e);
            return axum::response::Redirect::temporary(
                &format!("{}/auth/error?reason=github_error", frontend_url)
            );
        }
    };

    let user: GitHubUser = match reqwest::Client::new()
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {}", token.access_token))
        .header("User-Agent", "greed-compute")
        .send()
        .await
    {
        Ok(r) => match r.json().await {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("Failed to parse GitHub user: {}", e);
                return axum::response::Redirect::temporary(
                    &format!("{}/auth/error?reason=github_error", frontend_url)
                );
            }
        },
        Err(e) => {
            tracing::error!("GitHub user API failed: {}", e);
            return axum::response::Redirect::temporary(
                &format!("{}/auth/error?reason=github_error", frontend_url)
            );
        }
    };

    let api_key = state.db.find_or_create_key_for_github(&user.login);
    tracing::info!(login = %user.login, "GitHub OAuth login");

    // If state contains a CLI redirect_uri (starts with http://localhost), redirect there
    // Otherwise redirect to the frontend dashboard
    let redirect_base = match &query.state {
        Some(s) if s.starts_with("http://localhost") => s.to_string(),
        _ => format!("{}/dashboard", frontend_url),
    };

    let separator = if redirect_base.contains('?') { '&' } else { '?' };
    axum::response::Redirect::temporary(
        &format!("{}{}key={}&login={}", redirect_base, separator, api_key, user.login)
    )
}

async fn cuntext_fragment(Path(name): Path<String>) -> Response {
    if name.contains('/') || name.contains("..") || !name.ends_with(".cuntext") {
        return axum::response::Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(axum::body::Body::from("not found"))
            .unwrap();
    }
    let content: Option<&'static str> = match name.as_str() {
        "exec.cuntext"        => Some(include_str!("../../docs/cuntext/fragments/exec.cuntext")),
        "checkpoint.cuntext"  => Some(include_str!("../../docs/cuntext/fragments/checkpoint.cuntext")),
        "swarm.cuntext"       => Some(include_str!("../../docs/cuntext/fragments/swarm.cuntext")),
        "workspace.cuntext"   => Some(include_str!("../../docs/cuntext/fragments/workspace.cuntext")),
        "billing.cuntext"     => Some(include_str!("../../docs/cuntext/fragments/billing.cuntext")),
        "errors.cuntext"      => Some(include_str!("../../docs/cuntext/fragments/errors.cuntext")),
        _ => None,
    };
    match content {
        Some(body) => axum::response::Response::builder()
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .header(header::CACHE_CONTROL, "public, max-age=3600")
            .body(axum::body::Body::from(body))
            .unwrap(),
        None => axum::response::Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(axum::body::Body::from("not found"))
            .unwrap(),
    }
}
