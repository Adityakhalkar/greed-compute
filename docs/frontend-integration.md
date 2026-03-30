# Frontend Integration Guide — greed-compute API

All endpoints are prefixed with `/v1`. Pass your API key as `x-api-key` header on every request.

---

## Session Lifecycle

```
POST /v1/session/create          → { session_id, expires_at, ... }
POST /v1/session/{id}/execute    → { stdout, result, plots, html, error, duration_ms }
DELETE /v1/session/{id}          → terminate session
GET  /v1/session/{id}/status     → { ttl_remaining, calls_used, active }
```

Sessions last **15 minutes** and auto-renew on every execute call.

---

## 1. Create Session

```http
POST /v1/session/create
Content-Type: application/json

{
    "checkpoint_id": "optional — restore saved state on startup",
    "packages": ["optional", "pip packages to pre-install"]
}
```

**Response:**
```json
{
    "session_id": "d320c002-...",
    "created_at": "2026-03-28T...",
    "expires_at": "2026-03-28T...",
    "install_output": null,
    "install_error": null,
    "restore_vars": ["x", "model", "df"],
    "restore_error": null
}
```

---

## 2. Execute Code

```http
POST /v1/session/{id}/execute
Content-Type: application/json

{ "code": "import pandas as pd\ndf = pd.DataFrame({'a':[1,2,3]})\ndf" }
```

**Response:**
```json
{
    "stdout": "print() output here\n",
    "result": "42",
    "error": null,
    "duration_ms": 13,
    "plots": ["<base64 PNG>"],
    "html": "<table class=\"dataframe\">...</table>"
}
```

### Render priority order

```
1. stdout   → plain text output from print()
2. result   → last expression value (Jupyter-style)
3. html     → DataFrame/Series as HTML table
4. plots[]  → matplotlib figures as base64 PNG
5. error    → full Python traceback
```

```js
function renderOutput(result, container) {
    if (result.stdout) {
        container.append(pre(result.stdout));
    }
    if (result.result !== null) {
        container.append(pre(result.result));
    }
    if (result.html) {
        const div = document.createElement('div');
        div.innerHTML = result.html;
        container.append(div);
    }
    if (result.plots?.length) {
        result.plots.forEach(b64 => {
            const img = document.createElement('img');
            img.src = `data:image/png;base64,${b64}`;
            img.style.maxWidth = '100%';
            container.append(img);
        });
    }
    if (result.error) {
        container.append(errorBlock(result.error));
    }
}
```

### DataFrame CSS

```css
.dataframe { border-collapse: collapse; font-size: 0.875rem; font-family: monospace; }
.dataframe thead tr { background-color: #f3f4f6; }
.dataframe th, .dataframe td { padding: 6px 12px; border: 1px solid #e5e7eb; text-align: right; }
.dataframe tbody tr:hover { background-color: #f9fafb; }
```

### Error CSS

```css
.error-traceback {
    background: #fef2f2;
    border-left: 3px solid #ef4444;
    color: #991b1b;
    padding: 8px 12px;
    font-size: 0.8rem;
    white-space: pre-wrap;
}
```

---

## 3. Streaming Execution (recommended)

```http
POST /v1/session/{id}/execute/stream
Content-Type: application/json

{ "code": "..." }
```

Returns SSE. Use this for all execution — it's strictly better than the regular endpoint since you get real-time output AND the same final result.

**Event types:**
- `{"type":"stream","data":"line\n"}` — print() output arriving in real-time
- `{"type":"result",...}` — final event, same shape as `/execute` response

```js
const resp = await fetch(`/v1/session/${sessionId}/execute/stream`, {
    method: 'POST',
    headers: { 'x-api-key': apiKey, 'Content-Type': 'application/json' },
    body: JSON.stringify({ code }),
});

const reader = resp.body.getReader();
const decoder = new TextDecoder();

while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    for (const line of decoder.decode(value).split('\n')) {
        if (!line.startsWith('data: ')) continue;
        const event = JSON.parse(line.slice(6));
        if (event.type === 'stream') {
            outputEl.innerText += event.data;         // real-time
        } else if (event.type === 'result') {
            renderOutput(event, outputEl);            // final result
        }
    }
}
```

---

## 4. Kernel Busy — HTTP 423

If a cell is still executing and the user runs another one, the API returns **HTTP 423** immediately.

```js
if (response.status === 423) {
    showKernelBusyIndicator();
    setTimeout(() => executeCell(code), 1000); // retry after 1s
}
```

---

## 5. Package Installation

```http
POST /v1/session/{id}/install
Content-Type: application/json

{ "packages": ["seaborn", "xgboost==2.0.3"] }
```

**Response:**
```json
{
    "stdout": "Successfully installed seaborn-0.13.2...",
    "error": null,
    "packages": ["seaborn", "xgboost==2.0.3"]
}
```

GPU libraries (torch, tensorflow, jax, etc.) are blocked and return an explanatory error. Installs can take up to 2 minutes — show a loading indicator.

---

## 6. Background Jobs

For long-running code (model training, data processing) — submit async, poll for result.

```http
POST /v1/session/{id}/execute/async
Content-Type: application/json

{
    "code": "...",
    "webhook_url": "https://your-app.com/webhook"   ← optional
}
```

**Response (immediate):**
```json
{ "job_id": "998520de-...", "status": "queued" }
```

**Poll:**
```http
GET /v1/jobs/{job_id}
```

**Job statuses:** `queued` → `running` → `done` / `error`

**Done response:**
```json
{
    "id": "998520de-...",
    "status": "done",
    "stdout": "0\n1\n2\n",
    "result": null,
    "error": null,
    "plots": [],
    "html": null,
    "duration_ms": 3001,
    "created_at": "...",
    "started_at": "...",
    "finished_at": "..."
}
```

**Webhook payload** (POST to your URL when done):
```json
{
    "job_id": "...",
    "status": "done",
    "stdout": "...",
    "result": "...",
    "error": null,
    "plots": [],
    "html": null,
    "duration_ms": 3001
}
```

**List all jobs for a session:**
```http
GET /v1/session/{id}/jobs
```

---

## 7. Checkpoints

Save and restore Python interpreter state (variables, functions, imported libraries) across sessions.

**Save:**
```http
POST /v1/session/{id}/checkpoint
Content-Type: application/json

{ "name": "after-preprocessing" }
```

```json
{ "checkpoint_id": "d5bab72d-...", "name": "after-preprocessing", "size_bytes": 14820 }
```

**Restore into a running session:**
```http
POST /v1/session/{id}/restore/{checkpoint_id}
```

```json
{ "restored": true, "vars": ["df", "model", "X_train", "y_train"], "error": null }
```

**Restore on session create** (most common pattern — start a new session pre-loaded):
```http
POST /v1/session/create
{ "checkpoint_id": "d5bab72d-..." }
```

**List:**
```http
GET /v1/checkpoints
```

**Delete:**
```http
DELETE /v1/checkpoints/{id}
```

### Recommended UI pattern

Show a "Save state" button in the notebook toolbar. On click:
1. `POST /session/{id}/checkpoint` with a name prompt
2. Store `checkpoint_id` in the notebook config
3. On next load, `POST /session/create` with `checkpoint_id` to resume instantly

---

## 8. Session Status

```http
GET /v1/session/{id}/status
```

```json
{
    "active": true,
    "ttl_remaining": 847,
    "calls_used": 12,
    "session_id": "..."
}
```

Use `ttl_remaining` to show an idle warning (e.g. warn when < 60s). Don't show a countdown — the TTL resets on every execute, so it only counts down during genuine inactivity.

---

## API Reference Summary

| Method | Path | Description |
|--------|------|-------------|
| POST | `/v1/session/create` | Create session |
| DELETE | `/v1/session/{id}` | Terminate session |
| GET | `/v1/session/{id}/status` | Session info + TTL |
| POST | `/v1/session/{id}/execute` | Execute code (blocking) |
| POST | `/v1/session/{id}/execute/stream` | Execute code (SSE streaming) |
| POST | `/v1/session/{id}/execute/async` | Submit background job |
| GET | `/v1/session/{id}/jobs` | List jobs for session |
| GET | `/v1/jobs/{id}` | Get job status/result |
| POST | `/v1/session/{id}/install` | pip install packages |
| POST | `/v1/session/{id}/checkpoint` | Save session state |
| POST | `/v1/session/{id}/restore/{checkpoint_id}` | Restore checkpoint into session |
| GET | `/v1/checkpoints` | List all checkpoints |
| DELETE | `/v1/checkpoints/{id}` | Delete checkpoint |
