# greed-compute — 30-second demo

## Get a key
```bash
curl -s -X POST https://compute.deep-ml.com/v1/admin/keys \
  -H "Content-Type: application/json" \
  -d '{"name":"my-agent"}' | jq .key
```

## Run stateful Python in the cloud
```python
import requests

API = "https://compute.deep-ml.com/v1"
KEY = "your-key-here"
H   = {"X-API-Key": KEY, "Content-Type": "application/json"}

# Start a session — Python interpreter stays alive
s = requests.post(f"{API}/sessions", headers=H).json()
sid = s["session_id"]

# State persists across calls
requests.post(f"{API}/sessions/{sid}/execute", headers=H, json={"code": "x = 0"})
for i in range(5):
    r = requests.post(f"{API}/sessions/{sid}/execute", headers=H,
                      json={"code": f"x += {i}; x"})
    print(r.json()["result"])  # 0, 1, 3, 6, 10

# Save state — resume later, even after server restart
ck = requests.post(f"{API}/sessions/{sid}/checkpoint", headers=H,
                   json={"name": "my-progress"}).json()
print(ck["checkpoint_id"])  # restore anytime
```

## Start with data science stack pre-installed (~100ms, no pip install)
```python
s = requests.post(f"{API}/sessions", headers=H,
                  json={"template": "data-science"}).json()
sid = s["session_id"]

r = requests.post(f"{API}/sessions/{sid}/execute", headers=H, json={
    "code": "import numpy as np; np.random.randn(1000).mean()"
})
print(r.json()["result"])  # works instantly, numpy already there
```

## Parallel compute — map 10 workers, reduce results
```python
r = requests.post(f"{API}/swarm", headers=H, json={
    "template": "import numpy as np",
    "worker_fn": "result = float(np.sum(partition))",
    "partitions": [list(range(i, i+100)) for i in range(0, 1000, 100)],
    "reduce_fn": "total = sum(results)"
}).json()

print(r["reduce_result"])   # 499500
print(r["workers"][0]["duration_ms"])  # ~10ms per worker
```

## Share state between agents (any model)
```python
# Agent A creates a shared workspace
ws = requests.post(f"{API}/workspaces", headers=H,
                   json={"name": "research-42"}).json()

requests.post(f"{API}/workspaces/{ws['id']}/execute", headers=H,
              json={"code": "findings = []; model_accuracy = 0.0"})

# Invite Agent B (different API key)
requests.post(f"{API}/workspaces/{ws['id']}/invite", headers=H,
              json={"api_key": "agent-b-key"})

# Agent B (GPT-4, different key) reads shared state
r = requests.post(f"{API}/workspaces/{ws['id']}/execute",
                  headers={**H, "X-API-Key": "agent-b-key"},
                  json={"code": "len(findings)"})
print(r.json()["result"])  # 0 — same interpreter
```
