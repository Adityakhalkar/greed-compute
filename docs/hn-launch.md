# Show HN: greed-compute — checkpoint Python state like git, fork for parallel agents

---

**Title:** Show HN: greed-compute – stateful Python sessions for AI agents with checkpoint/fork

---

**Post body:**

I built greed-compute because I kept hitting the same wall building AI agents: every time an agent needed to run Python, I had to cold-start a fresh interpreter. Import numpy, load the model, re-run setup — over and over. For a swarm of 10 agents running the same template, that's 10x the cold start.

The core idea: treat a Python interpreter like a git repo. Checkpoint it, fork it, resume it.

**What it does:**

- **Persistent sessions** — Python interpreter stays alive between API calls. State (variables, imports, installed packages) persists across calls.
- **CALF (Checkpoint-based Agent Logical Fork)** — checkpoint a session, then fork N workers from it in parallel. Each worker gets the same state (imports, loaded models, data) without repeating the setup. ~500x speedup on warm starts vs cold-spawning.
- **Session templates** — pre-warmed sessions with data-science (numpy/pandas/sklearn), ML (torch/transformers), or web (requests/beautifulsoup) stacks. Ready in <100ms.
- **Swarm / map-reduce** — run one template, fan out to N parallel workers with different data partitions, reduce results. Workers complete in 6-22ms.
- **Shared Agent Workspace (SAW)** — multiple agents, any model (Claude, GPT-4, Llama), share one Python interpreter. Agent A sets a variable, Agent B reads it. State serializes automatically so concurrent writes don't corrupt it.
- **Checkpoints** — serialize full Python interpreter state (via dill) to disk. Restore into a new session anytime. Useful for overnight training runs, long computations, or "save progress" patterns.

**The numbers (real, on a single VPS):**
- Cold start: ~5.3 seconds
- CALF fork (warm): ~10ms
- Swarm workers (3 parallel): 6ms / 11ms / 22ms
- Checkpoint save: ~200ms

**Quick example:**
```python
import requests
API = "https://compute.deep-ml.com/v1"
H = {"X-API-Key": "your-key", "Content-Type": "application/json"}

# Start data-science session — numpy/pandas/sklearn pre-installed
s = requests.post(f"{API}/sessions", headers=H,
                  json={"template": "data-science"}).json()
sid = s["session_id"]

# Run computations — state persists
requests.post(f"{API}/sessions/{sid}/execute", headers=H,
              json={"code": "import pandas as pd; df = pd.DataFrame({'x': range(100)})"})
r = requests.post(f"{API}/sessions/{sid}/execute", headers=H,
              json={"code": "df.describe()"})
print(r.json()["result"])  # full describe output, df still in memory
```

**Free tier:** 60 req/min, 500MB checkpoint storage, 7-day retention.

**What I'm looking for:** feedback from people building AI agents or running ML workloads. What's broken about your current setup? What would make this useful to you?

API: https://compute.deep-ml.com
Docs: https://compute.deep-ml.com/v1/cuntext/index.cuntext

---

**Tags to target:** AI, Python, infrastructure, agents, LLM

---

# Reddit post (r/MachineLearning, r/LangChain, r/LocalLLaMA)

**Title:** I built a stateful Python execution API for AI agents — checkpoint interpreter state like git, fork for parallel workers

**Body:**

Been building AI agent infrastructure and got tired of this pattern:
```
agent needs python → cold start → import torch (45s) → load model (30s) → run 2 lines → done → repeat
```

Built greed-compute to solve it. Core idea: Python interpreter as a first-class persistent object.

**The thing that surprised me most:** using dill to serialize Python interpreter state, you can "fork" a running session like git clone. Run your expensive setup once, checkpoint it, then spawn N parallel workers all starting from that checkpoint. We call this CALF (Checkpoint-based Agent Logical Fork).

Real benchmark on a single VPS:
- Without CALF: 3 workers × 5.3s cold start = 15.9s total
- With CALF: template once + 3 workers = 10ms each = basically instant

Also built SAW (Shared Agent Workspace) — multiple agents share one interpreter. Claude sets `results = []`, GPT-4 can read `results`. The workspace auto-checkpoints after every write so state survives restarts.

**Free to try:** https://compute.deep-ml.com

Would love feedback from anyone building agentic systems — what does your current compute layer look like?

---

# Twitter/X thread

1/ I built greed-compute: checkpoint a Python interpreter like git, fork it for parallel agents.

Here's why it's 500x faster than cold-starting:

2/ The problem: every AI agent that runs Python starts fresh. Import torch. Load model. Run setup. 45 seconds. Every. Single. Time.

If you have 10 agents running the same setup: 10 × 45s = 7.5 minutes wasted before any real work.

3/ The fix: CALF (Checkpoint-based Agent Logical Fork).

Run your setup once → checkpoint the interpreter state → fork N workers from that checkpoint.

Each worker starts with torch already imported, model already loaded. ~10ms warm start.

4/ Real numbers:
- Cold start: 5.3s
- CALF fork: 10ms
- 3 parallel workers: 6ms / 11ms / 22ms

5/ Also built SAW (Shared Agent Workspace):

Multiple agents — Claude, GPT-4, Llama, whatever — share one Python interpreter.

Agent A sets `results = []`. Agent B appends to it. State serializes automatically. Switch models not workspaces.

6/ And session templates — pre-warmed sessions with stacks ready:
- `"data-science"` → numpy pandas sklearn matplotlib
- `"machine-learning"` → torch transformers datasets
- `"web-scraping"` → requests httpx beautifulsoup

<100ms to a ready interpreter, no pip install.

7/ Free tier available now.
API: https://compute.deep-ml.com
Docs: https://compute.deep-ml.com/v1/cuntext/index.cuntext

What does your AI agent compute layer look like? Reply 👇
