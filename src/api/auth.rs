use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::{sync::Arc, time::{Duration, Instant}};

use crate::AppState;
use crate::billing::PlanLimits;

/// Auth + rate limiting middleware.
///
/// - Validates x-api-key
/// - Enforces per-minute sliding window rate limit (in-memory, zero DB reads)
/// - Blocks when daily execution quota is exceeded (checked on execute/swarm only)
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, Response> {
    let path = request.uri().path().to_string();

    // Health check — no auth
    if path == "/v1/health" {
        return Ok(next.run(request).await);
    }

    // Stripe webhook — no auth (verified by HMAC signature inside handler)
    if path == "/v1/billing/webhook" {
        return Ok(next.run(request).await);
    }

    // cuntext files — public, no auth
    if path.starts_with("/v1/cuntext/") || path == "/v1/llms.cuntext" {
        return Ok(next.run(request).await);
    }

    // GitHub OAuth — public, no auth
    if path.starts_with("/v1/auth/") {
        return Ok(next.run(request).await);
    }

    // Admin endpoints — require GREED_ADMIN_KEY, not a regular API key
    if path.starts_with("/v1/admin/") {
        let admin_key = std::env::var("GREED_ADMIN_KEY").unwrap_or_default();
        if admin_key.is_empty() {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "Admin access not configured"})),
            ).into_response());
        }
        let provided = request
            .headers()
            .get("x-admin-key")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if provided != admin_key {
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({"error": "Invalid admin key"})),
            ).into_response());
        }
        return Ok(next.run(request).await);
    }

    let api_key = request
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let key = match api_key {
        Some(k) => k,
        None => return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Missing x-api-key header"})),
        ).into_response()),
    };

    let key_info = match state.db.validate_api_key(&key) {
        Some(info) => info,
        None => return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Invalid API key"})),
        ).into_response()),
    };

    let limits = PlanLimits::for_tier(&key_info.tier);

    // ── Per-minute sliding window rate limit (in-memory) ─────────────────────
    {
        let now = Instant::now();
        let window = Duration::from_secs(60);
        let mut entry = state.rate_windows.entry(key.clone()).or_default();
        // Drop timestamps older than 60s
        while entry.front().map(|t: &Instant| now.duration_since(*t) > window).unwrap_or(false) {
            entry.pop_front();
        }
        if entry.len() as u32 >= limits.requests_per_minute {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "error": "Rate limit exceeded",
                    "plan": key_info.tier,
                    "limit": limits.requests_per_minute,
                    "window": "60s",
                    "upgrade": "https://compute.deep-ml.com/billing"
                })),
            ).into_response());
        }
        entry.push_back(now);
    }

    // ── Daily quota check for execute / swarm endpoints ───────────────────────
    let is_execute = path.ends_with("/execute") || path.ends_with("/execute/async") || path.ends_with("/execute/stream");
    let is_swarm   = path == "/v1/swarm";

    if is_execute && !limits.is_unlimited(limits.executions_per_day) {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let usage = state.db.get_daily_usage(&key, &today);
        if usage.exec_count >= limits.executions_per_day as i64 {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "error": "Daily execution quota exceeded",
                    "plan": key_info.tier,
                    "limit": limits.executions_per_day,
                    "used": usage.exec_count,
                    "resets": "midnight UTC",
                    "upgrade": "https://compute.deep-ml.com/billing"
                })),
            ).into_response());
        }
    }

    if is_swarm && !limits.is_unlimited(limits.swarms_per_day) {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let usage = state.db.get_daily_usage(&key, &today);
        if usage.swarm_count >= limits.swarms_per_day as i64 {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "error": "Daily swarm quota exceeded",
                    "plan": key_info.tier,
                    "limit": limits.swarms_per_day,
                    "used": usage.swarm_count,
                    "resets": "midnight UTC",
                    "upgrade": "https://compute.deep-ml.com/billing"
                })),
            ).into_response());
        }
    }

    Ok(next.run(request).await)
}
