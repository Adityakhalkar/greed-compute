/// Enterprise billing API handlers.
///
/// POST /v1/billing/checkout   — create Stripe Checkout session (subscribe to pro/enterprise)
/// POST /v1/billing/portal     — create Customer Portal session (manage subscription)
/// POST /v1/billing/webhook    — receive Stripe webhook events
/// GET  /v1/usage              — current usage vs plan limits

use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::AppState;
use crate::billing::{PlanLimits, StripeClient};

fn stripe_client() -> Option<StripeClient> {
    std::env::var("STRIPE_SECRET_KEY").ok().map(StripeClient::new)
}

fn stripe_webhook_secret() -> String {
    std::env::var("STRIPE_WEBHOOK_SECRET").unwrap_or_default()
}

/// Map Stripe Price IDs to plan tiers.
/// Set STRIPE_PRICE_PRO and STRIPE_PRICE_ENTERPRISE env vars.
fn price_to_plan(price_id: &str) -> Option<&'static str> {
    let pro = std::env::var("STRIPE_PRICE_PRO").unwrap_or_default();
    let ent = std::env::var("STRIPE_PRICE_ENTERPRISE").unwrap_or_default();
    if !pro.is_empty() && price_id == pro   { return Some("pro"); }
    if !ent.is_empty() && price_id == ent   { return Some("enterprise"); }
    None
}

fn api_key_from_headers(headers: &HeaderMap) -> Option<String> {
    headers.get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

// ── POST /v1/billing/checkout ─────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CheckoutRequest {
    pub plan: String,              // "pro" | "enterprise"
    pub email: Option<String>,
    pub success_url: Option<String>,
    pub cancel_url: Option<String>,
}

pub async fn create_checkout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CheckoutRequest>,
) -> Result<Json<Value>, StatusCode> {
    let api_key = api_key_from_headers(&headers).ok_or(StatusCode::UNAUTHORIZED)?;

    if !matches!(body.plan.as_str(), "builder" | "pro" | "scale" | "enterprise") {
        return Ok(Json(json!({ "error": "plan must be 'builder' or 'scale'" })));
    }

    let stripe = stripe_client().ok_or_else(|| {
        tracing::warn!("STRIPE_SECRET_KEY not set");
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let price_env = match body.plan.as_str() {
        "builder" | "pro"        => "STRIPE_PRICE_PRO",
        "scale"   | "enterprise" => "STRIPE_PRICE_ENTERPRISE",
        _                        => unreachable!(),
    };
    let price_id = std::env::var(price_env).map_err(|_| {
        tracing::warn!("{} not set", price_env);
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    // Get or create Stripe customer
    let customer_id = if let Some(sc) = state.db.get_stripe_customer(&api_key) {
        sc.stripe_customer_id
    } else {
        let key_info = state.db.validate_api_key(&api_key).ok_or(StatusCode::UNAUTHORIZED)?;
        stripe.create_customer(&api_key, body.email.as_deref(), Some(&key_info.name))
            .await
            .map_err(|e| {
                tracing::error!("Stripe create_customer failed: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?
    };

    let base = std::env::var("GREED_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:8080".into());
    let success = body.success_url.unwrap_or_else(|| format!("{}/billing/success", base));
    let cancel  = body.cancel_url.unwrap_or_else(|| format!("{}/billing/cancel", base));

    let checkout_url = stripe
        .create_checkout_session(&customer_id, &price_id, &success, &cancel, &api_key)
        .await
        .map_err(|e| {
            tracing::error!("Stripe checkout failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(json!({
        "checkout_url": checkout_url,
        "plan": body.plan,
    })))
}

// ── POST /v1/billing/portal ───────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PortalRequest {
    pub return_url: Option<String>,
}

pub async fn create_portal(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PortalRequest>,
) -> Result<Json<Value>, StatusCode> {
    let api_key = api_key_from_headers(&headers).ok_or(StatusCode::UNAUTHORIZED)?;

    let stripe = stripe_client().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    let sc = state.db.get_stripe_customer(&api_key)
        .ok_or_else(|| {
            // No billing record yet
            StatusCode::NOT_FOUND
        })?;

    let base = std::env::var("GREED_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:8080".into());
    let return_url = body.return_url.unwrap_or_else(|| format!("{}/billing", base));

    let portal_url = stripe
        .create_portal_session(&sc.stripe_customer_id, &return_url)
        .await
        .map_err(|e| {
            tracing::error!("Stripe portal failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(json!({ "portal_url": portal_url })))
}

// ── POST /v1/billing/webhook ──────────────────────────────────────────────────

pub async fn stripe_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let sig = headers.get("stripe-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let secret = stripe_webhook_secret();
    if secret.is_empty() {
        tracing::warn!("STRIPE_WEBHOOK_SECRET not set — skipping signature verification");
    }

    let event = if secret.is_empty() {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    } else {
        match StripeClient::verify_webhook(&body, sig, &secret) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("Webhook signature failed: {}", e);
                return StatusCode::BAD_REQUEST;
            }
        }
    };

    let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
    tracing::info!(event_type, "Stripe webhook received");

    match event_type {
        // Subscription created or renewed
        "customer.subscription.created" | "customer.subscription.updated" => {
            handle_subscription_change(&state, &event, "active");
        }
        // Subscription cancelled or payment failed
        "customer.subscription.deleted" => {
            handle_subscription_change(&state, &event, "cancelled");
        }
        "invoice.payment_failed" => {
            let customer_id = event
                .pointer("/data/object/customer")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if let Some(sc) = state.db.get_stripe_customer_by_stripe_id(customer_id) {
                tracing::warn!(api_key = %sc.api_key, "Payment failed — keeping plan active with grace period");
            }
        }
        _ => {}
    }

    StatusCode::OK
}

fn handle_subscription_change(state: &Arc<AppState>, event: &Value, status: &str) {
    let sub = match event.pointer("/data/object") {
        Some(s) => s,
        None => return,
    };

    let customer_id = sub.get("customer").and_then(|v| v.as_str()).unwrap_or("");
    let sub_id = sub.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let api_key = sub.pointer("/metadata/greed_api_key")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Determine plan from the subscribed price
    let plan = sub
        .pointer("/items/data/0/price/id")
        .and_then(|v| v.as_str())
        .and_then(price_to_plan)
        .unwrap_or("free");

    let effective_plan = if status == "cancelled" { "free" } else { plan };

    if !api_key.is_empty() && !customer_id.is_empty() {
        state.db.upsert_stripe_customer(
            api_key,
            customer_id,
            Some(sub_id),
            effective_plan,
            status,
        );
        tracing::info!(api_key, plan = effective_plan, status, "Plan updated via webhook");
    }
}

// ── GET /v1/usage ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct UsageQuery {
    pub date: Option<String>,
}

pub async fn get_usage(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<UsageQuery>,
) -> Result<Json<Value>, StatusCode> {
    let api_key = api_key_from_headers(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let key_info = state.db.validate_api_key(&api_key).ok_or(StatusCode::UNAUTHORIZED)?;

    let date = q.date.unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    let _usage = state.db.get_daily_usage(&api_key, &date);
    let limits = PlanLimits::for_tier(&key_info.tier);
    let stripe = state.db.get_stripe_customer(&api_key);

    let credits_used = state.db.get_credits_used_today(&api_key);
    let credits_limit = if limits.is_unlimited(limits.credits_per_day) {
        Value::Null
    } else {
        json!(limits.credits_per_day)
    };
    let credits_remaining = if limits.is_unlimited(limits.credits_per_day) {
        Value::Null
    } else {
        json!((limits.credits_per_day as i64 - credits_used as i64).max(0))
    };

    Ok(Json(json!({
        "date": date,
        "plan": PlanLimits::tier_display(&key_info.tier),
        "billing_status": stripe.as_ref().map(|s| s.status.as_str()).unwrap_or("none"),
        "credits": {
            "used": credits_used,
            "limit": credits_limit,
            "remaining": credits_remaining,
            "resets": "midnight UTC",
        },
        "storage": {
            "used_mb": state.db.checkpoint_storage_used(&api_key) / (1024 * 1024),
            "limit_mb": if limits.checkpoint_storage_bytes == u64::MAX { Value::Null } else { json!(limits.checkpoint_storage_bytes / (1024 * 1024)) },
            "retention_days": limits.checkpoint_retention_days,
        },
        "limits": {
            "requests_per_minute": limits.requests_per_minute,
            "concurrent_sessions": limits.concurrent_sessions,
            "max_execution_secs": limits.max_execution_secs,
        }
    })))
}
