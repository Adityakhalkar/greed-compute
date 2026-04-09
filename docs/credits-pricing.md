# greed-compute: Credits & Pricing System

## Overview

We moved from a per-execution model to a **credit-based system** where:

> **1 credit = 1 second of execution time**

Credits reset daily at midnight UTC. Idle sessions consume zero credits.

---

## Tiers

| Tier | Price | Credits/day | Sessions | Storage | Retention | RPM |
|------|-------|-------------|----------|---------|-----------|-----|
| **hobby** | Free | 50 | 2 | 200 MB | 3 days | 60 |
| **builder** | $20/mo | 500 | 10 | 5 GB | 30 days | 300 |
| **scale** | $49/mo | Unlimited | 100 | 50 GB | 90 days | 2,000 |

### Tier name aliases (backward compat)
- `free` → `hobby`
- `pro` → `builder`
- `enterprise` → `scale`

Existing Stripe subscriptions using old tier names continue to work.

---

## Why credits over per-execution?

- A 100ms print statement and a 30-second ML training run shouldn't cost the same
- Forgotten/leaked sessions (LLM hallucination) cost zero — they just expire via TTL
- Familiar mental model — developers already think in tokens/credits from LLM APIs
- Internally tracked as `total_duration_ms` (already in DB), converted to credits on read

---

## API Changes

### GET /v1/usage — response shape changed

**Before:**
```json
{
  "plan": "free",
  "usage": {
    "executions": { "used": 12, "limit": 100, "remaining": 88 },
    "swarms": { "used": 1, "limit": 5, "remaining": 4 }
  }
}
```

**After:**
```json
{
  "plan": "hobby",
  "billing_status": "none",
  "date": "2026-04-09",
  "credits": {
    "used": 12,
    "limit": 50,
    "remaining": 38,
    "resets": "midnight UTC"
  },
  "storage": {
    "used_mb": 0,
    "limit_mb": 200,
    "retention_days": 3
  },
  "limits": {
    "requests_per_minute": 60,
    "concurrent_sessions": 2,
    "max_execution_secs": 30
  }
}
```

### 429 when credits exhausted

```json
{
  "error": "Daily credit limit reached",
  "plan": "hobby",
  "credits_used": 50,
  "credits_limit": 50,
  "resets": "midnight UTC",
  "upgrade": "https://compute.deep-ml.com/dashboard"
}
```

### POST /v1/billing/checkout — plan names changed

```json
{ "plan": "builder" }   // was "pro"
{ "plan": "scale" }     // was "enterprise"
```

---

## Stripe Setup

Using the same Stripe account as Deep-ML. Create two new products:

1. **greed-compute Builder** → $20/month recurring → copy `price_xxx` as `STRIPE_PRICE_PRO`
2. **greed-compute Scale** → $49/month recurring → copy `price_xxx` as `STRIPE_PRICE_ENTERPRISE`

Env vars on VPS (env var names unchanged):
```
STRIPE_SECRET_KEY=sk_live_xxx
STRIPE_PRICE_PRO=price_xxx        # Builder product price ID
STRIPE_PRICE_ENTERPRISE=price_xxx # Scale product price ID
STRIPE_WEBHOOK_SECRET=whsec_xxx
```

Webhook endpoint in Stripe dashboard:
- URL: `https://compute.deep-ml.com/v1/billing/webhook`
- Events: `checkout.session.completed`, `customer.subscription.updated`, `customer.subscription.deleted`

---

## Frontend Changes Required (greed-compute-ui)

The dashboard reads `/v1/usage`. Update to use new response shape:

```ts
// Before
data.usage.executions.used
data.usage.executions.remaining
data.plan  // was "free"/"pro"/"enterprise"

// After
data.credits.used
data.credits.remaining
data.plan  // now "hobby"/"builder"/"scale"
```

Display on dashboard: `"X credits remaining today"` — resets midnight UTC.

Upgrade button: `POST /v1/billing/checkout` with `{ plan: "builder" }` or `{ plan: "scale" }`.

---

## Files Changed

| File | Change |
|------|--------|
| `src/billing/mod.rs` | Replaced `executions_per_day`/`swarms_per_day` with `credits_per_day`. Added `tier_display()`. Renamed tiers. |
| `src/db/mod.rs` | Added `get_credits_used_today()` — ceil(total_duration_ms / 1000) |
| `src/api/auth.rs` | Replaced exec count check with credit check |
| `src/api/billing.rs` | `/usage` returns credits instead of executions/swarms |
| `docs/cuntext/index.cuntext` | Updated tier info in LLM-facing discovery |
| `docs/cuntext/fragments/billing.cuntext` | Full rewrite for credit model |
| `docs/cuntext/fragments/errors.cuntext` | 429 now has two cases: rate-limit vs credits-exhausted |
