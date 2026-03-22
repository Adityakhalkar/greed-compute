use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{Duration, Utc};
use serde_json::json;
use std::sync::Arc;

use crate::AppState;

/// Per-tier rate limits (requests per hour)
fn tier_limit(tier: &str) -> Option<i64> {
    match tier {
        "free" => Some(100),
        "pro" => Some(5000),
        "enterprise" => None, // unlimited
        _ => Some(100),       // unknown tiers default to free
    }
}

/// Auth middleware — validates X-API-Key header and enforces per-tier rate limits
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, Response> {
    // Skip auth for health check and admin key creation
    if request.uri().path() == "/v1/health" || request.uri().path() == "/v1/admin/keys" {
        return Ok(next.run(request).await);
    }

    let api_key = request
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let key = match api_key {
        Some(k) => k,
        None => {
            return Err((StatusCode::UNAUTHORIZED, Json(json!({"error": "Missing API key"}))).into_response());
        }
    };

    let key_info = match state.db.validate_api_key(&key) {
        Some(info) => info,
        None => {
            return Err((StatusCode::UNAUTHORIZED, Json(json!({"error": "Invalid API key"}))).into_response());
        }
    };

    // Rate limit check (skip for enterprise)
    if let Some(limit) = tier_limit(&key_info.tier) {
        let one_hour_ago = (Utc::now() - Duration::hours(1)).to_rfc3339();
        let usage_count = state.db.get_usage_count(&key, &one_hour_ago);

        if usage_count >= limit {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "error": "Rate limit exceeded",
                    "tier": key_info.tier,
                    "limit": limit,
                    "window": "1 hour"
                })),
            )
                .into_response());
        }
    }

    // Record this request's usage
    let endpoint = request.uri().path().to_string();
    state.db.record_usage(&key, &endpoint, 0);

    Ok(next.run(request).await)
}
