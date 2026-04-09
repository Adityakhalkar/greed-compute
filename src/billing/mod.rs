/// Enterprise billing — plan limits, Stripe integration, usage enforcement.

// ── Plan tiers ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PlanLimits {
    /// Max API requests per minute (sliding window, in-memory)
    pub requests_per_minute: u32,
    /// Max code executions per day (execute + stream + async)
    pub executions_per_day: u32,
    /// Max swarms submitted per day
    pub swarms_per_day: u32,
    /// Max concurrent sessions allowed at once
    pub concurrent_sessions: u32,
    /// Max single execution wall time in seconds
    pub max_execution_secs: u32,
    /// Max total checkpoint storage in bytes (u64::MAX = unlimited)
    pub checkpoint_storage_bytes: u64,
    /// How many days before a checkpoint is auto-deleted (0 = never)
    pub checkpoint_retention_days: u32,
}

impl PlanLimits {
    pub fn for_tier(tier: &str) -> Self {
        match tier {
            "pro" => Self {
                requests_per_minute: 300,
                executions_per_day: 5_000,
                swarms_per_day: 100,
                concurrent_sessions: 20,
                max_execution_secs: 120,
                checkpoint_storage_bytes: 5 * 1024 * 1024 * 1024,  // 5 GB
                checkpoint_retention_days: 30,
            },
            "enterprise" => Self {
                requests_per_minute: 2_000,
                executions_per_day: u32::MAX,
                swarms_per_day: u32::MAX,
                concurrent_sessions: 100,
                max_execution_secs: 600,
                checkpoint_storage_bytes: 50 * 1024 * 1024 * 1024, // 50 GB
                checkpoint_retention_days: 90,
            },
            // free / unknown
            _ => Self {
                requests_per_minute: 60,
                executions_per_day: 100,
                swarms_per_day: 5,
                concurrent_sessions: 3,
                max_execution_secs: 30,
                checkpoint_storage_bytes: 500 * 1024 * 1024,        // 500 MB
                checkpoint_retention_days: 7,
            },
        }
    }

    pub fn is_unlimited(&self, field: u32) -> bool {
        field == u32::MAX
    }
}

// ── Stripe API client ─────────────────────────────────────────────────────────

pub struct StripeClient {
    secret_key: String,
    http: reqwest::Client,
}

impl StripeClient {
    pub fn new(secret_key: String) -> Self {
        Self {
            secret_key,
            http: reqwest::Client::new(),
        }
    }

    /// Create or retrieve a Stripe customer for this API key.
    pub async fn create_customer(
        &self,
        api_key: &str,
        email: Option<&str>,
        name: Option<&str>,
    ) -> Result<String, String> {
        let mut form = vec![
            ("metadata[greed_api_key]".to_string(), api_key.to_string()),
        ];
        if let Some(e) = email { form.push(("email".into(), e.to_string())); }
        if let Some(n) = name  { form.push(("name".into(),  n.to_string())); }

        let resp = self.http
            .post("https://api.stripe.com/v1/customers")
            .basic_auth(&self.secret_key, Some(""))
            .form(&form)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        json.get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("Stripe error: {}", json))
    }

    /// Create a Checkout Session for a subscription.
    /// Returns the session URL to redirect the user to.
    pub async fn create_checkout_session(
        &self,
        stripe_customer_id: &str,
        price_id: &str,          // Stripe Price ID for the plan
        success_url: &str,
        cancel_url: &str,
        api_key: &str,
    ) -> Result<String, String> {
        let form = vec![
            ("customer".to_string(), stripe_customer_id.to_string()),
            ("mode".to_string(), "subscription".to_string()),
            ("line_items[0][price]".to_string(), price_id.to_string()),
            ("line_items[0][quantity]".to_string(), "1".to_string()),
            ("success_url".to_string(), success_url.to_string()),
            ("cancel_url".to_string(), cancel_url.to_string()),
            ("metadata[greed_api_key]".to_string(), api_key.to_string()),
            ("subscription_data[metadata][greed_api_key]".to_string(), api_key.to_string()),
        ];

        let resp = self.http
            .post("https://api.stripe.com/v1/checkout/sessions")
            .basic_auth(&self.secret_key, Some(""))
            .form(&form)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        json.get("url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("Stripe error: {}", json))
    }

    /// Create a Customer Portal session so users can manage their subscription.
    pub async fn create_portal_session(
        &self,
        stripe_customer_id: &str,
        return_url: &str,
    ) -> Result<String, String> {
        let form = vec![
            ("customer".to_string(), stripe_customer_id.to_string()),
            ("return_url".to_string(), return_url.to_string()),
        ];

        let resp = self.http
            .post("https://api.stripe.com/v1/billing_portal/sessions")
            .basic_auth(&self.secret_key, Some(""))
            .form(&form)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        json.get("url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("Stripe error: {}", json))
    }

    /// Verify a Stripe webhook signature and parse the event.
    pub fn verify_webhook(payload: &[u8], sig_header: &str, secret: &str) -> Result<serde_json::Value, String> {
        // Parse t= and v1= from the Stripe-Signature header
        let mut timestamp = "";
        let mut signature = "";
        for part in sig_header.split(',') {
            if let Some(t) = part.strip_prefix("t=") { timestamp = t; }
            if let Some(v) = part.strip_prefix("v1=") { signature = v; }
        }
        if timestamp.is_empty() || signature.is_empty() {
            return Err("Invalid Stripe-Signature header".into());
        }

        // HMAC-SHA256(secret, "{timestamp}.{payload}")
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        type HmacSha256 = Hmac<Sha256>;

        let signed_payload = format!("{}.{}", timestamp, std::str::from_utf8(payload).unwrap_or(""));
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| e.to_string())?;
        mac.update(signed_payload.as_bytes());
        let result = mac.finalize().into_bytes();
        let computed = hex::encode(result);

        if computed != signature {
            return Err("Webhook signature mismatch".into());
        }

        serde_json::from_slice(payload).map_err(|e| e.to_string())
    }
}
