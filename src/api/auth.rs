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

    // ── Daily credit quota check (execute + swarm endpoints) ─────────────────
    // 1 credit = 1 second of execution. Checked before execution so the user
    // gets a clear error instead of a surprise at the end.
    let is_compute = path.ends_with("/execute")
        || path.ends_with("/execute/async")
        || path.ends_with("/execute/stream")
        || path == "/v1/swarm";

    if is_compute && !limits.is_unlimited(limits.credits_per_day) {
        let credits_used = state.db.get_credits_used_today(&key);
        if credits_used >= limits.credits_per_day {
            let plan = crate::billing::PlanLimits::tier_display(&key_info.tier);
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "error": "Daily credit limit reached",
                    "plan": plan,
                    "credits_used": credits_used,
                    "credits_limit": limits.credits_per_day,
                    "resets": "midnight UTC",
                    "upgrade": "https://compute.deep-ml.com/dashboard"
                })),
            ).into_response());
        }
    }

    Ok(next.run(request).await)
}
