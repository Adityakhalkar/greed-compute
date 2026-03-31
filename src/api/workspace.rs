use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    db::WorkspaceRecord,
    runtime::PythonRuntime,
    AppState,
};

// ── Per-workspace live runtime ────────────────────────────────────────────────

pub struct WorkspaceRuntime {
    pub runtime: PythonRuntime,
    pub workspace_path: std::path::PathBuf,
}

/// Global live workspace runtimes: workspace_id → Mutex<WorkspaceRuntime>
/// The Mutex serialises execution so only one agent runs at a time per workspace.
pub type WorkspaceMap = Arc<DashMap<String, Arc<Mutex<WorkspaceRuntime>>>>;

// ── Request / response types ──────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateWorkspaceRequest {
    pub name: String,
}

#[derive(Deserialize)]
pub struct ExecuteInWorkspaceRequest {
    pub code: String,
}

#[derive(Deserialize)]
pub struct InviteRequest {
    pub api_key: String,
}

#[derive(Serialize)]
pub struct WorkspaceResponse {
    pub id: String,
    pub name: String,
    pub owner_api_key: String,
    pub created_at: String,
    pub last_accessed_at: String,
    pub live: bool,
}

impl From<(&WorkspaceRecord, bool)> for WorkspaceResponse {
    fn from((r, live): (&WorkspaceRecord, bool)) -> Self {
        Self {
            id: r.id.clone(),
            name: r.name.clone(),
            owner_api_key: r.owner_api_key.clone(),
            created_at: r.created_at.clone(),
            last_accessed_at: r.last_accessed_at.clone(),
            live,
        }
    }
}

#[derive(Serialize)]
pub struct ExecuteResponse {
    pub stdout: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Get or boot a live workspace runtime.
/// If the workspace has a saved checkpoint, restore from it.
/// Otherwise start a fresh Python runtime.
async fn get_or_boot(
    workspace_id: &str,
    checkpoint_path: Option<&str>,
    workspaces: &WorkspaceMap,
    worker_path: &str,
    python_path: &str,
) -> Result<Arc<Mutex<WorkspaceRuntime>>, String> {
    if let Some(live) = workspaces.get(workspace_id) {
        return Ok(live.clone());
    }

    // Boot a new runtime
    let ws_path = std::env::temp_dir()
        .join("greed-compute")
        .join("workspaces")
        .join(workspace_id);
    std::fs::create_dir_all(&ws_path).map_err(|e| e.to_string())?;

    let mut runtime = PythonRuntime::spawn(&ws_path, worker_path, python_path)
        .await
        .map_err(|e| e.to_string())?;

    // Restore from checkpoint if one exists
    if let Some(ckpt) = checkpoint_path {
        if std::path::Path::new(ckpt).exists() {
            let restore_code = format!(
                "import dill as _dill\nwith open({:?}, 'rb') as _f:\n    _ns = _dill.load(_f)\nglobals().update(_ns)\ndel _dill, _f, _ns",
                ckpt
            );
            runtime.execute(&restore_code).await;
            tracing::info!(workspace_id, ckpt, "Workspace restored from checkpoint");
        }
    }

    let entry = Arc::new(Mutex::new(WorkspaceRuntime { runtime, workspace_path: ws_path }));
    workspaces.insert(workspace_id.to_string(), entry.clone());
    Ok(entry)
}

/// Save workspace state to a checkpoint file and update DB.
async fn persist(
    workspace_id: &str,
    rt_lock: &Arc<Mutex<WorkspaceRuntime>>,
    checkpoint_dir: &std::path::Path,
    state: &Arc<AppState>,
) {
    let ckpt_path = checkpoint_dir.join(format!("ws-{}.dill", workspace_id));
    let path_str = ckpt_path.to_string_lossy().to_string();

    let save_code = format!(
        "import dill as _dill\nwith open({:?}, 'wb') as _f:\n    _dill.dump(dict(globals()), _f)\ndel _dill, _f",
        path_str
    );

    let mut rt = rt_lock.lock().await;
    rt.runtime.execute(&save_code).await;
    drop(rt);

    state.db.update_workspace_checkpoint(workspace_id, &path_str);
}

fn checkpoint_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(
        std::env::var("GREED_CHECKPOINT_DIR")
            .unwrap_or_else(|_| "/tmp/greed-compute/checkpoints".into()),
    )
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// POST /v1/workspaces
pub async fn create_workspace(
    State(state): State<Arc<AppState>>,
    axum::Extension(api_key): axum::Extension<String>,
    Json(body): Json<CreateWorkspaceRequest>,
) -> impl IntoResponse {
    let id = Uuid::new_v4().to_string();
    match state.db.create_workspace(&id, &body.name, &api_key) {
        Ok(_) => {
            let record = state.db.get_workspace(&id).unwrap();
            let live = state.workspaces.contains_key(&id);
            Json(WorkspaceResponse::from((&record, live))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// GET /v1/workspaces
pub async fn list_workspaces(
    State(state): State<Arc<AppState>>,
    axum::Extension(api_key): axum::Extension<String>,
) -> impl IntoResponse {
    let records = state.db.list_workspaces(&api_key);
    let resp: Vec<WorkspaceResponse> = records
        .iter()
        .map(|r| {
            let live = state.workspaces.contains_key(&r.id);
            WorkspaceResponse::from((r, live))
        })
        .collect();
    Json(resp)
}

/// GET /v1/workspaces/:id
pub async fn get_workspace(
    State(state): State<Arc<AppState>>,
    axum::Extension(api_key): axum::Extension<String>,
    Path(workspace_id): Path<String>,
) -> impl IntoResponse {
    if !state.db.can_access_workspace(&workspace_id, &api_key) {
        return (StatusCode::NOT_FOUND, "workspace not found").into_response();
    }
    let Some(record) = state.db.get_workspace(&workspace_id) else {
        return (StatusCode::NOT_FOUND, "workspace not found").into_response();
    };
    let members = state.db.list_workspace_members(&workspace_id);
    let live = state.workspaces.contains_key(&workspace_id);

    #[derive(Serialize)]
    struct DetailResponse {
        #[serde(flatten)]
        workspace: WorkspaceResponse,
        members: Vec<crate::db::WorkspaceMember>,
    }

    Json(DetailResponse {
        workspace: WorkspaceResponse::from((&record, live)),
        members,
    })
    .into_response()
}

/// POST /v1/workspaces/:id/execute
pub async fn execute_in_workspace(
    State(state): State<Arc<AppState>>,
    axum::Extension(api_key): axum::Extension<String>,
    Path(workspace_id): Path<String>,
    Json(body): Json<ExecuteInWorkspaceRequest>,
) -> impl IntoResponse {
    if !state.db.can_access_workspace(&workspace_id, &api_key) {
        return (StatusCode::NOT_FOUND, "workspace not found").into_response();
    }
    let Some(record) = state.db.get_workspace(&workspace_id) else {
        return (StatusCode::NOT_FOUND, "workspace not found").into_response();
    };

    let worker_path = state.sessions.worker_path();
    let python_path = state.sessions.python_path();
    let ckpt_dir = checkpoint_dir();
    let _ = std::fs::create_dir_all(&ckpt_dir);

    let rt_lock = match get_or_boot(
        &workspace_id,
        record.checkpoint_path.as_deref(),
        &state.workspaces,
        &worker_path,
        &python_path,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };

    let start = std::time::Instant::now();
    let exec_result = {
        let mut rt = rt_lock.lock().await;
        rt.runtime.execute(&body.code).await
    };
    let duration_ms = start.elapsed().as_millis() as u64;

    // Auto-persist after every write
    persist(&workspace_id, &rt_lock, &ckpt_dir, &state).await;
    state.db.touch_workspace(&workspace_id);

    let result_value: Option<serde_json::Value> = exec_result
        .result
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());

    Json(ExecuteResponse {
        stdout: exec_result.stdout,
        result: result_value,
        error: exec_result.error,
        duration_ms,
    })
    .into_response()
}

/// POST /v1/workspaces/:id/invite
pub async fn invite_member(
    State(state): State<Arc<AppState>>,
    axum::Extension(api_key): axum::Extension<String>,
    Path(workspace_id): Path<String>,
    Json(body): Json<InviteRequest>,
) -> impl IntoResponse {
    if !state.db.is_workspace_owner(&workspace_id, &api_key) {
        return (StatusCode::FORBIDDEN, "only the owner can invite members").into_response();
    }
    // Validate the invited key exists
    if state.db.validate_api_key(&body.api_key).is_none() {
        return (StatusCode::BAD_REQUEST, "invited api_key not found").into_response();
    }
    match state.db.add_workspace_member(&workspace_id, &body.api_key) {
        Ok(_) => Json(serde_json::json!({"invited": body.api_key})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// DELETE /v1/workspaces/:id/members/:member_key
pub async fn kick_member(
    State(state): State<Arc<AppState>>,
    axum::Extension(api_key): axum::Extension<String>,
    Path((workspace_id, member_key)): Path<(String, String)>,
) -> impl IntoResponse {
    if !state.db.is_workspace_owner(&workspace_id, &api_key) {
        return (StatusCode::FORBIDDEN, "only the owner can remove members").into_response();
    }
    if state.db.remove_workspace_member(&workspace_id, &member_key) {
        Json(serde_json::json!({"removed": member_key})).into_response()
    } else {
        (StatusCode::NOT_FOUND, "member not found or is owner").into_response()
    }
}

/// DELETE /v1/workspaces/:id
pub async fn delete_workspace(
    State(state): State<Arc<AppState>>,
    axum::Extension(api_key): axum::Extension<String>,
    Path(workspace_id): Path<String>,
) -> impl IntoResponse {
    if !state.db.is_workspace_owner(&workspace_id, &api_key) {
        return (StatusCode::FORBIDDEN, "only the owner can delete a workspace").into_response();
    }
    // Kill live runtime if present
    if let Some((_, rt_lock)) = state.workspaces.remove(&workspace_id) {
        let rt = rt_lock.lock().await;
        // workspace_path cleanup
        let _ = std::fs::remove_dir_all(&rt.workspace_path);
        drop(rt);
    }
    // Remove checkpoint file
    if let Some(record) = state.db.get_workspace(&workspace_id) {
        if let Some(ckpt) = record.checkpoint_path {
            let _ = std::fs::remove_file(&ckpt);
        }
    }
    if state.db.delete_workspace(&workspace_id, &api_key) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        (StatusCode::NOT_FOUND, "workspace not found").into_response()
    }
}
