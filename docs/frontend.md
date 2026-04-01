# Frontend Integration Guide

## Base URL
```
https://compute.deep-ml.com/v1
```

---

## Authentication

### Regular users
All user-facing endpoints require:
```
X-API-Key: gc-...
Content-Type: application/json
```

### Admin UI
Admin endpoints require a separate header:
```
X-Admin-Key: <value of GREED_ADMIN_KEY env var on VPS>
Content-Type: application/json
```

Set `GREED_ADMIN_KEY` in `/etc/systemd/system/greed-compute.service`:
```ini
[Service]
Environment=GREED_ADMIN_KEY=your-secret-here
```

---

## Admin endpoints (UI only, X-Admin-Key required)

### Create API key
```
POST /admin/keys
Body: { "name": "user@example.com" }
→ { "key": "gc-...", "name": "...", "tier": "free", "created_at": "..." }
```

### List API keys
```
GET /admin/keys
→ [{ "key", "name", "tier", "is_active", "created_at" }]
```

### Revoke API key
```
POST /admin/keys/{key}/revoke
→ { "revoked": true, "key": "..." }
```

---

## User-facing endpoints (X-API-Key required)

### Sessions
```
POST   /sessions                          create session
       body: { ttl_seconds?, template?, checkpoint_id? }
       template: "data-science" | "machine-learning" | "web-scraping" | "blank"
       → { session_id, expires_at, template }

POST   /session/{id}/execute              run code
       body: { code: str }
       → { stdout, result, error, plots, duration_ms }

POST   /session/{id}/execute/stream       streaming SSE
       body: { code: str }
       → SSE: { type: "stdout"|"result"|"error", data }

GET    /session/{id}/status               session info
       → { active, ttl_remaining, calls_used }

DELETE /session/{id}                      terminate session
```

### Checkpoints
```
POST   /session/{id}/checkpoint           save state
       body: { name: str }
       → { checkpoint_id, size_bytes, expires_in_days, storage_used_mb, storage_limit_mb }

GET    /checkpoints                       list checkpoints
POST   /session/{id}/restore/{checkpoint_id}  restore checkpoint → new session
DELETE /checkpoints/{id}                  delete checkpoint
```

### Swarm
```
POST   /swarm
       body: {
         template: str,       setup code run once
         worker_fn: str,      code per partition (receives `partition`)
         partitions: [...],   array of inputs
         reduce_fn: str       code after all workers (receives `results`)
       }
       → { swarm_id, workers: [...], reduce_result, total_duration_ms }

GET    /swarm/{id}            get swarm status/result
```

### Workspaces (SAW)
```
POST   /workspaces                        create workspace
       body: { name: str }
       → { id, name, owner_api_key, live }

GET    /workspaces                        list your workspaces
GET    /workspaces/{id}                   workspace detail + members
POST   /workspaces/{id}/execute           run code in shared state
       body: { code: str }
       → { stdout, result, error, duration_ms }
POST   /workspaces/{id}/invite            invite member (owner only)
       body: { api_key: str }
DELETE /workspaces/{id}/members/{key}     remove member (owner only)
DELETE /workspaces/{id}                   delete workspace (owner only)
```

### Usage & Billing
```
GET    /usage
       → { plan, requests: { used, limit, remaining }, storage: { used_mb, limit_mb }, checkpoint_retention_days }

POST   /billing/checkout
       body: { plan: "pro"|"enterprise", success_url, cancel_url }
       → { checkout_url }   ← redirect user here for Stripe payment

POST   /billing/portal
       body: { return_url }
       → { portal_url }     ← redirect user here to manage subscription
```

---

## Plan tiers

| | Free | Pro | Enterprise |
|---|---|---|---|
| Rate limit | 60 rpm | 600 rpm | unlimited |
| Storage | 500 MB | 5 GB | 50 GB |
| Retention | 7 days | 30 days | 90 days |
| Price | $0 | $49/mo | $299/mo |

---

## Error codes

| Code | Meaning | UI action |
|---|---|---|
| 401 | Missing/invalid API key | Show login |
| 403 | Wrong admin key | Show error |
| 404 | Session/checkpoint expired | Prompt recreate |
| 429 | Rate limit hit | Show upgrade banner |
| 507 | Storage quota full | Show storage usage + upgrade |
| 500 | Python execution error | Show traceback in output |

---

## Templates available

Show these as options on session create:

| Value | Label | Includes |
|---|---|---|
| `blank` | Blank | Nothing pre-installed |
| `data-science` | Data Science | numpy, pandas, matplotlib, scikit-learn, scipy |
| `machine-learning` | Machine Learning | torch, transformers, datasets, accelerate |
| `web-scraping` | Web Scraping | requests, httpx, beautifulsoup4, lxml |

---

## Public endpoints (no auth)

```
GET /health                        → { status, version, warm_pool, template_pools }
GET /cuntext/index.cuntext         agent-readable API manifest
GET /cuntext/fragments/{name}      agent-readable fragments
GET /llms.cuntext                  alias for index.cuntext
```
