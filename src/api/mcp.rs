/// MCP (Model Context Protocol) endpoint — streamable HTTP transport
///
/// Exposes greed-compute as tools for Claude Desktop, Cursor, and any
/// MCP client. Users add one URL to their config — no local install needed.
///
/// Usage in claude_desktop_config.json:
/// {
///   "mcpServers": {
///     "greed-compute": {
///       "type": "http",
///       "url": "http://168.144.22.192/v1/mcp?api_key=greed_..."
///     }
///   }
/// }

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};

use crate::AppState;

// ── JSON-RPC helpers ──────────────────────────────────────────────────────────

fn ok(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: &Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn text(s: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": s.into() }] })
}

fn tool_err(msg: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": msg.into() }], "isError": true })
}

// ── Tool definitions ──────────────────────────────────────────────────────────

fn tool_list() -> Value {
    json!({ "tools": [
        {
            "name": "create_session",
            "description": "Create a new Python execution session. Returns session_id. Sessions last 15 minutes and auto-renew on each execution.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "checkpoint_id": { "type": "string", "description": "Optional: restore a saved checkpoint on startup." },
                    "packages": { "type": "array", "items": { "type": "string" }, "description": "Optional: pip install these packages before the session is ready." }
                }
            }
        },
        {
            "name": "execute_code",
            "description": "Execute Python code in a session. Supports numpy, pandas, matplotlib, sklearn, scipy. Returns stdout, last-expression value (Jupyter-style), plots, HTML DataFrames, and full tracebacks.",
            "inputSchema": {
                "type": "object",
                "required": ["session_id", "code"],
                "properties": {
                    "session_id": { "type": "string" },
                    "code": { "type": "string", "description": "Python code to execute." }
                }
            }
        },
        {
            "name": "install_packages",
            "description": "pip install packages into a session. GPU-heavy libraries (torch, tensorflow, jax) are blocked.",
            "inputSchema": {
                "type": "object",
                "required": ["session_id", "packages"],
                "properties": {
                    "session_id": { "type": "string" },
                    "packages": { "type": "array", "items": { "type": "string" } }
                }
            }
        },
        {
            "name": "submit_job",
            "description": "Submit long-running code as a background job. Returns job_id immediately. Use get_job to poll for results.",
            "inputSchema": {
                "type": "object",
                "required": ["session_id", "code"],
                "properties": {
                    "session_id": { "type": "string" },
                    "code": { "type": "string" },
                    "webhook_url": { "type": "string", "description": "Optional URL to POST results to when done." }
                }
            }
        },
        {
            "name": "get_job",
            "description": "Poll the status and result of a background job.",
            "inputSchema": {
                "type": "object",
                "required": ["job_id"],
                "properties": {
                    "job_id": { "type": "string" }
                }
            }
        },
        {
            "name": "session_status",
            "description": "Get session TTL remaining (seconds), calls used, and active status.",
            "inputSchema": {
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": { "type": "string" }
                }
            }
        },
        {
            "name": "terminate_session",
            "description": "Terminate a session and free its resources.",
            "inputSchema": {
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": { "type": "string" }
                }
            }
        },
        {
            "name": "create_checkpoint",
            "description": "Save the current session state (all variables, functions, imports) to a named checkpoint that persists across sessions.",
            "inputSchema": {
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": { "type": "string" },
                    "name": { "type": "string" }
                }
            }
        },
        {
            "name": "restore_checkpoint",
            "description": "Restore a saved checkpoint into a running session.",
            "inputSchema": {
                "type": "object",
                "required": ["session_id", "checkpoint_id"],
                "properties": {
                    "session_id": { "type": "string" },
                    "checkpoint_id": { "type": "string" }
                }
            }
        },
        {
            "name": "list_checkpoints",
            "description": "List all saved checkpoints for this API key.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "delete_checkpoint",
            "description": "Delete a checkpoint and its stored file.",
            "inputSchema": {
                "type": "object",
                "required": ["checkpoint_id"],
                "properties": {
                    "checkpoint_id": { "type": "string" }
                }
            }
        }
    ]})
}

// ── Tool dispatch ─────────────────────────────────────────────────────────────

async fn dispatch(
    state: &Arc<AppState>,
    api_key: &str,
    tool: &str,
    args: &Value,
) -> Value {
    let empty = Value::Object(Default::default());

    match tool {
        "create_session" => {
            let ttl = args.get("ttl_seconds").and_then(|v| v.as_i64());
            let packages = args.get("packages")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect::<Vec<_>>());
            let checkpoint_id = args.get("checkpoint_id").and_then(|v| v.as_str()).map(|s| s.to_string());

            let session = match state.sessions.create_session(ttl).await {
                Ok(s) => s,
                Err(e) => return tool_err(format!("Failed to create session: {}", e)),
            };

            // Pre-install packages
            if let Some(pkgs) = &packages {
                if !pkgs.is_empty() {
                    if let Some(s) = state.sessions.get_session(&session.session_id) {
                        let mut runtime = s.runtime.lock().await;
                        runtime.install_packages(pkgs).await;
                    }
                }
            }

            // Restore checkpoint
            let mut restore_info = String::new();
            if let Some(ckpt_id) = &checkpoint_id {
                if let Some(record) = state.db.get_checkpoint(ckpt_id, api_key) {
                    if let Some(s) = state.sessions.get_session(&session.session_id) {
                        let mut runtime = s.runtime.lock().await;
                        let (vars, err) = runtime.restore_checkpoint(&record.path).await;
                        if let Some(e) = err {
                            restore_info = format!("\nWarning: checkpoint restore failed: {}", e);
                        } else {
                            restore_info = format!("\nRestored checkpoint '{}': {} vars ({})",
                                record.name, vars.len(), vars.join(", "));
                        }
                    }
                } else {
                    restore_info = format!("\nWarning: checkpoint '{}' not found", ckpt_id);
                }
            }

            text(format!("Session created.\nsession_id: {}\nexpires_at: {}{}",
                session.session_id, session.expires_at, restore_info))
        }

        "execute_code" => {
            let session_id = match args.get("session_id").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return tool_err("Missing session_id"),
            };
            let code = match args.get("code").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return tool_err("Missing code"),
            };

            let session = match state.sessions.get_session(session_id) {
                Some(s) => s,
                None => return tool_err(format!("Session '{}' not found or expired", session_id)),
            };

            let mut runtime = match session.runtime.try_lock() {
                Ok(r) => r,
                Err(_) => return tool_err("Session is busy — another execution is running"),
            };
            session.touch();
            let result = runtime.execute(code).await;
            drop(runtime);

            let mut parts = Vec::new();
            if !result.stdout.is_empty() {
                parts.push(format!("stdout:\n{}", result.stdout.trim_end()));
            }
            if let Some(r) = &result.result {
                parts.push(format!("result: {}", r));
            }
            if result.html.is_some() {
                parts.push("[DataFrame — HTML table in html field]".into());
            }
            if !result.plots.is_empty() {
                parts.push(format!("[{} plot(s) captured]", result.plots.len()));
            }
            if let Some(e) = &result.error {
                parts.push(format!("error:\n{}", e));
            }
            if parts.is_empty() {
                parts.push("(no output)".into());
            }
            parts.push(format!("duration: {}ms", result.duration_ms));

            text(parts.join("\n\n"))
        }

        "install_packages" => {
            let session_id = match args.get("session_id").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return tool_err("Missing session_id"),
            };
            let packages: Vec<String> = match args.get("packages").and_then(|v| v.as_array()) {
                Some(arr) => arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect(),
                None => return tool_err("Missing packages"),
            };

            let session = match state.sessions.get_session(session_id) {
                Some(s) => s,
                None => return tool_err("Session not found"),
            };

            let mut runtime = session.runtime.lock().await;
            let (stdout, error, _) = runtime.install_packages(&packages).await;
            drop(runtime);

            if let Some(e) = error {
                tool_err(format!("Install failed: {}\n{}", e, stdout))
            } else {
                text(format!("Installed: {}\n\n{}", packages.join(", "), stdout.trim()))
            }
        }

        "submit_job" => {
            let session_id = match args.get("session_id").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return tool_err("Missing session_id"),
            };
            let code = match args.get("code").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return tool_err("Missing code"),
            };
            let webhook_url = args.get("webhook_url").and_then(|v| v.as_str());

            let session = match state.sessions.get_session(session_id) {
                Some(s) => s,
                None => return tool_err("Session not found"),
            };

            let job_id = uuid::Uuid::new_v4().to_string();
            if let Err(e) = state.db.create_job(&job_id, session_id, api_key, code, webhook_url) {
                return tool_err(format!("Failed to create job: {}", e));
            }

            let state_clone = state.clone();
            let job_id_clone = job_id.clone();
            let code_owned = code.to_string();
            let webhook_owned = webhook_url.map(|s| s.to_string());

            tokio::spawn(async move {
                state_clone.db.set_job_running(&job_id_clone);
                let mut runtime = session.runtime.lock().await;
                session.touch();
                let result = runtime.execute(&code_owned).await;
                drop(runtime);
                state_clone.db.set_job_done(
                    &job_id_clone, &result.stdout,
                    result.result.as_deref(), result.error.as_deref(),
                    &result.plots, result.html.as_deref(), result.duration_ms as i64,
                );
                if let Some(url) = webhook_owned {
                    let payload = json!({
                        "job_id": job_id_clone, "status": "done",
                        "stdout": result.stdout, "error": result.error,
                    });
                    let client = reqwest::Client::new();
                    let _ = client.post(&url).json(&payload).send().await;
                }
            });

            text(format!("Job submitted.\njob_id: {}\nstatus: queued\n\nUse get_job to poll for results.", job_id))
        }

        "get_job" => {
            let job_id = match args.get("job_id").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return tool_err("Missing job_id"),
            };
            match state.db.get_job(job_id, api_key) {
                Some(job) => {
                    let mut parts = vec![
                        format!("job_id: {}", job.id),
                        format!("status: {}", job.status),
                    ];
                    if let Some(s) = &job.stdout { if !s.is_empty() { parts.push(format!("stdout:\n{}", s.trim_end())); } }
                    if let Some(e) = &job.error { parts.push(format!("error:\n{}", e)); }
                    if let Some(ms) = job.duration_ms { parts.push(format!("duration: {}ms", ms)); }
                    text(parts.join("\n\n"))
                }
                None => tool_err(format!("Job '{}' not found", job_id)),
            }
        }

        "session_status" => {
            let session_id = match args.get("session_id").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return tool_err("Missing session_id"),
            };
            match state.sessions.get_session_status(session_id) {
                Some(status) => text(format!(
                    "session_id: {}\nactive: {}\nttl_remaining: {}s\ncalls_used: {}",
                    status.session_id, status.active, status.ttl_remaining, status.calls_used
                )),
                None => tool_err(format!("Session '{}' not found or expired", session_id)),
            }
        }

        "terminate_session" => {
            let session_id = match args.get("session_id").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return tool_err("Missing session_id"),
            };
            if state.sessions.terminate_session(session_id) {
                text(format!("Session '{}' terminated.", session_id))
            } else {
                tool_err(format!("Session '{}' not found", session_id))
            }
        }

        "create_checkpoint" => {
            let session_id = match args.get("session_id").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return tool_err("Missing session_id"),
            };
            let session = match state.sessions.get_session(session_id) {
                Some(s) => s,
                None => return tool_err("Session not found"),
            };

            let checkpoint_id = uuid::Uuid::new_v4().to_string();
            let name = args.get("name").and_then(|v| v.as_str())
                .unwrap_or(&format!("checkpoint-{}", &checkpoint_id[..8]))
                .to_string();

            let dir = std::path::PathBuf::from(
                std::env::var("GREED_CHECKPOINT_DIR").unwrap_or_else(|_| "/tmp/greed-compute/checkpoints".into())
            );
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join(format!("{}.dill", checkpoint_id));
            let path_str = path.to_string_lossy().to_string();

            let mut runtime = match session.runtime.try_lock() {
                Ok(r) => r,
                Err(_) => return tool_err("Session is busy"),
            };
            let (size, error) = runtime.create_checkpoint(&path_str).await;
            drop(runtime);

            if let Some(e) = error {
                return tool_err(format!("Checkpoint failed: {}", e));
            }
            if let Err(e) = state.db.create_checkpoint_record(&checkpoint_id, api_key, &name, &path_str, size as i64) {
                return tool_err(format!("Failed to save checkpoint record: {}", e));
            }
            text(format!("Checkpoint saved.\ncheckpoint_id: {}\nname: {}\nsize: {} bytes", checkpoint_id, name, size))
        }

        "restore_checkpoint" => {
            let session_id = match args.get("session_id").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return tool_err("Missing session_id"),
            };
            let checkpoint_id = match args.get("checkpoint_id").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return tool_err("Missing checkpoint_id"),
            };

            let record = match state.db.get_checkpoint(checkpoint_id, api_key) {
                Some(r) => r,
                None => return tool_err(format!("Checkpoint '{}' not found", checkpoint_id)),
            };
            let session = match state.sessions.get_session(session_id) {
                Some(s) => s,
                None => return tool_err("Session not found"),
            };

            let mut runtime = match session.runtime.try_lock() {
                Ok(r) => r,
                Err(_) => return tool_err("Session is busy"),
            };
            let (vars, error) = runtime.restore_checkpoint(&record.path).await;
            drop(runtime);

            if let Some(e) = error {
                tool_err(format!("Restore failed: {}", e))
            } else {
                text(format!("Restored '{}': {} vars loaded ({})", record.name, vars.len(), vars.join(", ")))
            }
        }

        "list_checkpoints" => {
            let checkpoints = state.db.list_checkpoints(api_key);
            if checkpoints.is_empty() {
                text("No checkpoints saved yet.")
            } else {
                let lines: Vec<String> = checkpoints.iter().map(|c| {
                    format!("• {} — {} ({} bytes, {})", c.name, c.id, c.size_bytes, c.created_at)
                }).collect();
                text(format!("{} checkpoint(s):\n\n{}", checkpoints.len(), lines.join("\n")))
            }
        }

        "delete_checkpoint" => {
            let checkpoint_id = match args.get("checkpoint_id").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return tool_err("Missing checkpoint_id"),
            };
            if let Some(record) = state.db.get_checkpoint(checkpoint_id, api_key) {
                let _ = std::fs::remove_file(&record.path);
            }
            if state.db.delete_checkpoint_record(checkpoint_id, api_key) {
                text(format!("Checkpoint '{}' deleted.", checkpoint_id))
            } else {
                tool_err(format!("Checkpoint '{}' not found", checkpoint_id))
            }
        }

        _ => tool_err(format!("Unknown tool: {}", tool)),
    }
}

// ── Main handler ──────────────────────────────────────────────────────────────

pub async fn mcp_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> (StatusCode, Json<Value>) {
    // Accept api_key from header or query param
    let api_key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| params.get("api_key").cloned())
        .unwrap_or_default();

    let msg: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(json!({
            "jsonrpc": "2.0", "id": null,
            "error": { "code": -32700, "message": "Parse error" }
        }))),
    };

    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");

    // Notifications (no id) — acknowledge with 202
    if msg.get("id").is_none() && method == "notifications/initialized" {
        return (StatusCode::ACCEPTED, Json(json!({})));
    }

    let result = match method {
        "initialize" => ok(&id, json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "greed-compute", "version": "0.1.0" }
        })),

        "tools/list" => ok(&id, tool_list()),

        "tools/call" => {
            if api_key.is_empty() {
                return (StatusCode::UNAUTHORIZED, Json(err(&id, -32001, "Missing API key — pass as x-api-key header or ?api_key= query param")));
            }
            if state.db.validate_api_key(&api_key).is_none() {
                return (StatusCode::UNAUTHORIZED, Json(err(&id, -32001, "Invalid API key")));
            }

            let params = msg.get("params").unwrap_or(&Value::Null);
            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let arguments = params.get("arguments").unwrap_or(&Value::Null);

            let tool_result = dispatch(&state, &api_key, tool_name, arguments).await;
            ok(&id, tool_result)
        }

        _ => err(&id, -32601, &format!("Method not found: {}", method)),
    };

    (StatusCode::OK, Json(result))
}
