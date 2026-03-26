# Frontend Changes Required for greed-compute API Updates

## What Changed in the API Response

The `/v1/session/{id}/execute` endpoint now returns additional fields:

```json
{
    "stdout": "...",
    "result": null,
    "error": null,
    "duration_ms": 13,
    "plots": ["<base64 PNG string>", "..."],
    "html": "<table border=\"1\" class=\"dataframe\">...</table>"
}
```

---

## 1. HTML DataFrame Output

**Field:** `html: string | null`

When the last expression in a code cell is a DataFrame or Series, the API returns an HTML table string in the `html` field instead of a text repr in `stdout`.

**Frontend logic:**

```js
if (result.html) {
    // Render as HTML table
    outputElement.innerHTML = result.html;
} else if (result.stdout) {
    // Render as plain text
    outputElement.innerText = result.stdout;
}
```

**Recommended styling** — the table has `class="dataframe"`, add CSS:

```css
.dataframe {
    border-collapse: collapse;
    font-size: 0.875rem;
    font-family: monospace;
}
.dataframe thead tr {
    background-color: #f3f4f6;
}
.dataframe th, .dataframe td {
    padding: 6px 12px;
    border: 1px solid #e5e7eb;
    text-align: right;
}
.dataframe tbody tr:hover {
    background-color: #f9fafb;
}
```

---

## 2. Matplotlib Plot Output

**Field:** `plots: string[]` (array of base64-encoded PNG strings)

When user calls `plt.show()` or creates a figure without calling `plt.show()`, plots are captured and returned as base64 PNGs. There can be multiple plots per execution.

**Frontend logic:**

```js
if (result.plots && result.plots.length > 0) {
    result.plots.forEach(b64 => {
        const img = document.createElement('img');
        img.src = `data:image/png;base64,${b64}`;
        img.style.maxWidth = '100%';
        outputElement.appendChild(img);
    });
}
```

---

## 3. Error Display (Full Traceback)

**Field:** `error: string | null`

Errors now return the full Python traceback with line numbers, not just the exception message. Display in a styled pre block:

```js
if (result.error) {
    outputElement.innerHTML = `<pre class="error-traceback">${escapeHtml(result.error)}</pre>`;
}
```

```css
.error-traceback {
    background-color: #fef2f2;
    border-left: 3px solid #ef4444;
    color: #991b1b;
    padding: 8px 12px;
    font-size: 0.8rem;
    white-space: pre-wrap;
    word-break: break-word;
}
```

---

## 4. Render Priority Order

A single execution can have stdout, html, plots, and/or an error. Render in this order:

```
1. stdout        → plain text / print() output
2. html          → DataFrame/Series table (replaces stdout repr)
3. plots[]       → images, one per capture
4. error         → red traceback block
```

---

## 5. Session TTL Update

Sessions now last **15 minutes** (up from 2 minutes). If you have any frontend session expiry warnings or timers, update them accordingly.

The `expires_at` field in the create session response is the source of truth:

```json
{
    "session_id": "...",
    "created_at": "2026-03-26T...",
    "expires_at": "2026-03-26T..."
}
```
