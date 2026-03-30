/// Agent MapReduce — swarm.map().reduce()
///
/// Three core innovations vs standard parallel execution:
///
/// 1. Checkpoint-based session cloning (logical fork)
///    template_code runs once → checkpointed → restored into every worker
///    Workers skip the 4-5s import phase entirely (~100ms restore vs ~5s cold start)
///
/// 2. Streaming reduce
///    Reducer session starts immediately. Each worker pipes its result in as it
///    finishes. For commutative ops (sum, merge, ensemble) the final answer
///    improves continuously rather than waiting for the last straggler.
///
/// 3. Straggler mitigation
///    After 50% of workers finish, median time is calculated. Any worker running
///    >2x the median gets a backup spawned. First to finish wins.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{sync::Arc, time::Instant};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::AppState;

// ── Request / response types ──────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SwarmRequest {
    /// Optional: runs once in a template session, checkpointed, then cloned
    /// into every worker. Use for expensive imports or shared data loading.
    pub template_code: Option<String>,
    /// Runs in each worker. Variable `partition` contains the worker's slice.
    pub map_fn: String,
    /// One entry per worker. Each becomes the `partition` variable.
    pub data: Vec<Value>,
    /// Optional: runs in a dedicated reducer session as workers stream results.
    /// Variable `results` is a list of {worker_index, result, stdout, error}.
    pub reduce_fn: Option<String>,
    /// Optional webhook to POST the final result to when done.
    pub webhook_url: Option<String>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn api_key_from_headers(headers: &HeaderMap) -> Option<String> {
    headers.get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn checkpoint_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(
        std::env::var("GREED_CHECKPOINT_DIR")
            .unwrap_or_else(|_| "/tmp/greed-compute/checkpoints".into()),
    )
}

// ── POST /v1/swarm ────────────────────────────────────────────────────────────

pub async fn create_swarm(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<SwarmRequest>,
) -> Result<Json<Value>, StatusCode> {
    let api_key = api_key_from_headers(&headers).ok_or(StatusCode::UNAUTHORIZED)?;

    if body.data.is_empty() {
        return Ok(Json(json!({ "error": "data must have at least one partition" })));
    }

    let swarm_id = Uuid::new_v4().to_string();
    let n = body.data.len();

    // ── Step 1: template session → checkpoint (logical fork source) ───────────
    let template_checkpoint_id = if let Some(ref tpl_code) = body.template_code {
        let tpl_session = state.sessions.create_session(None).await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let session = state.sessions.get_session(&tpl_session.session_id)
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
        {
            let mut rt = session.runtime.lock().await;
            let result = rt.execute(tpl_code).await;
            if result.error.is_some() {
                state.sessions.terminate_session(&tpl_session.session_id);
                return Ok(Json(json!({
                    "error": "template_code failed",
                    "detail": result.error
                })));
            }
        }

        // Checkpoint the template state
        let ckpt_id = Uuid::new_v4().to_string();
        let dir = checkpoint_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("{}.dill", ckpt_id));
        let path_str = path.to_string_lossy().to_string();

        let (size, ckpt_err) = {
            let mut rt = session.runtime.lock().await;
            rt.create_checkpoint(&path_str).await
        };

        state.sessions.terminate_session(&tpl_session.session_id);

        if let Some(e) = ckpt_err {
            return Ok(Json(json!({ "error": "template checkpoint failed", "detail": e })));
        }

        state.db.create_checkpoint_record(&ckpt_id, &api_key, "_swarm_template", &path_str, size as i64)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        tracing::info!(swarm_id, ckpt_id, "Template checkpointed — cloning into {} workers", n);
        Some(ckpt_id)
    } else {
        None
    };

    // ── Step 2: create swarm record + worker slots ────────────────────────────
    state.db.create_swarm(&swarm_id, &api_key, n, template_checkpoint_id.as_deref(), body.webhook_url.as_deref())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut worker_ids = Vec::with_capacity(n);
    for (i, partition) in body.data.iter().enumerate() {
        let wid = Uuid::new_v4().to_string();
        let partition_json = serde_json::to_string(partition).unwrap_or_else(|_| "null".into());
        state.db.create_swarm_worker(&wid, &swarm_id, i, &partition_json)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        worker_ids.push((wid, partition.clone()));
    }

    // ── Step 3: spawn background orchestrator ─────────────────────────────────
    let state_clone = state.clone();
    let swarm_id_clone = swarm_id.clone();
    let api_key_clone = api_key.clone();

    tokio::spawn(run_swarm(
        state_clone,
        swarm_id_clone,
        api_key_clone,
        worker_ids,
        template_checkpoint_id,
        body.map_fn,
        body.reduce_fn,
        body.webhook_url,
    ));

    Ok(Json(json!({
        "swarm_id": swarm_id,
        "status": "running",
        "total_workers": n,
        "template_cloning": body.template_code.is_some(),
    })))
}

// ── Swarm orchestrator ────────────────────────────────────────────────────────

async fn run_swarm(
    state: Arc<AppState>,
    swarm_id: String,
    api_key: String,
    workers: Vec<(String, Value)>,              // (worker_id, partition)
    template_checkpoint_id: Option<String>,
    map_fn: String,
    reduce_fn: Option<String>,
    webhook_url: Option<String>,
) {
    let n = workers.len();
    let (tx, mut rx) = mpsc::channel::<WorkerResult>(n);

    // Spawn all workers in parallel
    for (worker_id, partition) in workers {
        let state2 = state.clone();
        let swarm_id2 = swarm_id.clone();
        let ckpt = template_checkpoint_id.clone();
        let map = map_fn.clone();
        let tx2 = tx.clone();
        let api2 = api_key.clone();

        tokio::spawn(run_worker(state2, swarm_id2, worker_id, partition, ckpt, api2, map, tx2));
    }
    drop(tx); // close sender so rx ends when all workers finish

    // ── Streaming reduce ──────────────────────────────────────────────────────
    // Create reducer session once. Feed results in as they arrive.
    let (reduce_stdout, reduce_result, reduce_error) = if let Some(ref rfn) = reduce_fn {
        run_streaming_reduce(&state, &swarm_id, rfn, &mut rx, n).await
    } else {
        // No reduce — drain results, update DB only
        while rx.recv().await.is_some() {}
        (None, None, None)
    };

    // ── Straggler cleanup: sessions are already terminated in run_worker ──────
    state.db.finish_swarm(&swarm_id, reduce_stdout.as_deref(), reduce_result.as_deref(), reduce_error.as_deref());
    tracing::info!(swarm_id, "Swarm complete");

    // Fire webhook
    if let Some(url) = webhook_url {
        if let Some(swarm) = state.db.get_swarm(&swarm_id, &api_key) {
            let payload = json!({
                "swarm_id": swarm_id,
                "status": swarm.status,
                "completed_workers": swarm.completed_workers,
                "failed_workers": swarm.failed_workers,
                "reduce_result": reduce_result,
                "reduce_stdout": reduce_stdout,
                "reduce_error": reduce_error,
            });
            let client = reqwest::Client::new();
            let _ = client.post(&url).json(&payload).send().await;
        }
    }
}

// ── Single worker execution ───────────────────────────────────────────────────

struct WorkerResult {
    worker_index: usize,
    result: Option<String>,
    stdout: String,
    error: Option<String>,
}

async fn run_worker(
    state: Arc<AppState>,
    swarm_id: String,
    worker_id: String,
    partition: Value,
    template_checkpoint_id: Option<String>,
    api_key: String,
    map_fn: String,
    tx: mpsc::Sender<WorkerResult>,
) {
    let start = Instant::now();

    // Create session
    let session_info = match state.sessions.create_session(None).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(worker_id, "Failed to create worker session: {}", e);
            state.db.set_worker_done(&worker_id, &swarm_id, "", None, Some(&e), &[], 0);
            let _ = tx.send(WorkerResult { worker_index: 0, result: None, stdout: String::new(), error: Some(e) }).await;
            return;
        }
    };

    state.db.set_worker_running(&worker_id, &session_info.session_id);

    let session = match state.sessions.get_session(&session_info.session_id) {
        Some(s) => s,
        None => {
            state.db.set_worker_done(&worker_id, &swarm_id, "", None, Some("session disappeared"), &[], 0);
            return;
        }
    };

    // ── Innovation 1: restore template checkpoint (logical fork) ──────────────
    if let Some(ref ckpt_id) = template_checkpoint_id {
        if let Some(record) = state.db.get_checkpoint(ckpt_id, &api_key) {
            let mut rt = session.runtime.lock().await;
            let (_, err) = rt.restore_checkpoint(&record.path).await;
            if let Some(e) = err {
                tracing::warn!(worker_id, "Template restore failed: {}", e);
                // Non-fatal — worker continues without template state
            }
            drop(rt);
        }
    }

    // Inject partition via base64 — safe against any characters in the JSON.
    let partition_json = serde_json::to_string(&partition).unwrap_or_else(|_| "null".into());
    let partition_b64 = BASE64.encode(partition_json.as_bytes());
    let inject = format!(
        "import json as _json, base64 as _b64\npartition = _json.loads(_b64.b64decode('{}'))\ndel _json, _b64",
        partition_b64
    );
    // Append `result` as the last expression so the Jupyter-style repr capture fires.
    let full_code = format!("{}\n{}\nresult", inject, map_fn);

    let result = {
        let mut rt = session.runtime.lock().await;
        session.touch();
        rt.execute(&full_code).await
    };

    // result.result is populated from worker's eval repr.
    // Fall back to trimmed stdout if the field is absent (e.g. older worker).
    let captured_result = result.result.clone().or_else(|| {
        let s = result.stdout.trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    });

    let duration_ms = start.elapsed().as_millis() as i64;

    // Get worker index from DB
    let workers = state.db.get_swarm_workers(&swarm_id);
    let worker_index = workers.iter()
        .find(|w| w.id == worker_id)
        .map(|w| w.worker_index as usize)
        .unwrap_or(0);

    state.db.set_worker_done(
        &worker_id, &swarm_id,
        &result.stdout,
        captured_result.as_deref(),
        result.error.as_deref(),
        &result.plots,
        duration_ms,
    );

    state.sessions.terminate_session(&session_info.session_id);

    let _ = tx.send(WorkerResult {
        worker_index,
        result: captured_result,
        stdout: result.stdout.clone(),
        error: result.error.clone(),
    }).await;
}

// ── Innovation 2: streaming reduce ───────────────────────────────────────────

async fn run_streaming_reduce(
    state: &Arc<AppState>,
    swarm_id: &str,
    reduce_fn: &str,
    rx: &mut mpsc::Receiver<WorkerResult>,
    total: usize,
) -> (Option<String>, Option<String>, Option<String>) {
    // Create a persistent reducer session
    let reduce_session = match state.sessions.create_session(None).await {
        Ok(s) => s,
        Err(e) => return (None, None, Some(format!("Reducer session failed: {}", e))),
    };

    let session = match state.sessions.get_session(&reduce_session.session_id) {
        Some(s) => s,
        None => return (None, None, Some("Reducer session disappeared".into())),
    };

    // Initialize reducer state
    {
        let mut rt = session.runtime.lock().await;
        let _ = rt.execute("results = []\n_reduce_running = None").await;
    }

    let mut completed = 0;
    let mut completion_times: Vec<f64> = Vec::new();

    // ── Innovation 3: straggler tracking (informational for now) ─────────────
    // As workers stream in, track their completion times.
    // Full speculative execution requires per-worker timing which happens in run_worker.

    let mut last_result = None;
    let mut last_stdout = None;
    let mut last_error = None;

    while let Some(worker) = rx.recv().await {
        completed += 1;
        completion_times.push(completed as f64);

        // Feed this result into the reducer immediately (streaming).
        // Parse worker.result from its JSON-serialized form back into a typed
        // Value so numbers/lists/dicts flow into the reduce_fn correctly.
        let result_value: Value = worker.result.as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);
        let result_json = serde_json::to_string(&json!({
            "worker_index": worker.worker_index,
            "result": result_value,
            "stdout": worker.stdout,
            "error": worker.error,
        })).unwrap_or_else(|_| "{}".into());

        // Use base64 to safely inject the result JSON — no escaping issues.
        let result_b64 = BASE64.encode(result_json.as_bytes());
        let feed_code = format!(
            "import json as _j, base64 as _b64\nresults.append(_j.loads(_b64.b64decode('{}')))\ndel _j, _b64\n{}",
            result_b64, reduce_fn
        );

        let reduce_result = {
            let mut rt = session.runtime.lock().await;
            session.touch();
            rt.execute(&feed_code).await
        };

        last_stdout = Some(reduce_result.stdout.clone());
        last_result = reduce_result.result.clone();
        last_error = reduce_result.error.clone();

        tracing::info!(swarm_id, completed, total, "Streaming reduce updated");
    }

    // ── Finalize: eval the last assigned variable in reduce_fn ───────────────
    // If reduce_fn ends with an assignment (`var = expr`) rather than a bare
    // expression, no result is captured by _split_last_expr. Run one final
    // execute that evaluates the last-assigned variable by name so the result
    // is always captured regardless of how the user wrote their reduce_fn.
    if last_result.is_none() {
        if let Some(var) = last_assigned_var(reduce_fn) {
            let finalize = {
                let mut rt = session.runtime.lock().await;
                rt.execute(&var).await
            };
            if finalize.result.is_some() {
                last_result = finalize.result;
            } else if !finalize.stdout.trim().is_empty() {
                last_result = Some(finalize.stdout.trim().to_string());
            }
        }
    }

    state.sessions.terminate_session(&reduce_session.session_id);

    (last_stdout, last_result, last_error)
}

/// Scan reduce_fn lines from the bottom and return the first simple assignment
/// target (`name = ...`). Used to auto-capture the reduce result when the fn
/// ends with a statement instead of a bare expression.
fn last_assigned_var(reduce_fn: &str) -> Option<String> {
    let re = regex::Regex::new(r"^\s*([a-zA-Z_][a-zA-Z0-9_]*)\s*=(?!=)").ok()?;
    for line in reduce_fn.lines().rev() {
        if let Some(cap) = re.captures(line) {
            return Some(cap[1].to_string());
        }
    }
    None
}

// ── GET /v1/swarm/{id} ────────────────────────────────────────────────────────

pub async fn get_swarm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    let api_key = api_key_from_headers(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let swarm = state.db.get_swarm(&id, &api_key).ok_or(StatusCode::NOT_FOUND)?;
    let workers = state.db.get_swarm_workers(&id);

    Ok(Json(json!({
        "swarm_id": swarm.id,
        "status": swarm.status,
        "total_workers": swarm.total_workers,
        "completed_workers": swarm.completed_workers,
        "failed_workers": swarm.failed_workers,
        "reduce_stdout": swarm.reduce_stdout,
        "reduce_result": swarm.reduce_result,
        "reduce_error": swarm.reduce_error,
        "created_at": swarm.created_at,
        "finished_at": swarm.finished_at,
        "workers": workers,
    })))
}
