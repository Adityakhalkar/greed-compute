# greed-compute

## The workbench AI agents never had.

---

## The Problem

AI is getting smarter by the day.

But there's a fundamental problem nobody talks about: **AI agents are stateless.**

Imagine hiring the world's best mathematician to solve a complex problem. They show up, work brilliantly for an hour — fill ten whiteboards with equations, train a model, load a dataset, build a pipeline. Then you step out for coffee. You come back. Every whiteboard is wiped clean. They remember nothing. Tomorrow you hire them again. Same thing. Every single time.

That is the reality of AI agents today.

- Claude solves half your problem → session ends → everything is gone
- You switch from Claude to GPT mid-task → start completely over, re-explain everything
- An agent runs Python code → next run, no variables, no data, nothing persists
- Two agents working on the same problem → can't share state or results
- An agent executes code on your machine → security nightmare, no isolation

The AI has the brain. But it has no workbench. No memory. No hands.

---

## The Solution

**greed-compute** is the persistent, secure, shareable compute layer that lives underneath AI agents.

Not a model. Not a framework. Not a wrapper.

The layer that all of them run on.

---

## How It Works

### Isolated Sandbox
Every agent gets a fully isolated Python interpreter wrapped in OS-level jail (nsjail). Network blocked. Memory capped. CPU limited. The agent can execute anything — your infrastructure is never at risk.

### Persistent Sessions
The Python interpreter stays alive between calls. Variables in memory. Imports cached. Trained models sitting ready. The agent picks up exactly where it left off — milliseconds later or days later.

### Checkpoint System — *CALF*
The entire interpreter state serializes to disk in one atomic operation. Every variable, every function, every trained model.

Think of it as **`git commit` for a running Python process.**

Restore it anywhere, anytime, in any session. This is the foundation that makes everything else possible.

### Agent MapReduce — *StAR*
Split any problem across N parallel agents. Each gets its own isolated environment forked from the same starting state. Results stream into a shared reducer the moment each worker finishes — not after the last straggler.

**The answer improves continuously. You never wait.**

### Universal MCP Server
Any AI model — Claude, GPT, Gemini, Codex, whatever ships next year — connects via one URL. No install. No SDK. One line of config. Every model gets the same compute primitives.

---

## The Innovations

These are original architectural patterns we invented. They don't exist anywhere else in agent infrastructure.

---

### CALF — Checkpoint-based Agent Logical Fork

**The insight:** if ten agents all need numpy loaded, a dataset preprocessed, and a model initialized — why do that work ten times?

With CALF, the expensive setup runs once in a template session. The interpreter state gets checkpointed. Every worker **forks** from that checkpoint — like `git clone` for a live Python process — in under 100ms.

```
Without CALF:  10 workers × 5s cold start = 50s before any work begins
With CALF:     1 template (5s) + 10 forks (100ms each) = 6s total
```

**~500x speedup per worker. Measured. Real.**

*E2B, Modal, Daytona — all cold-start every worker, every time. Nobody does this.*

---

### Speculative Session Hydration

**The insight:** while the template is running its setup code, N worker sessions can warm up in parallel.

By the time the template checkpoints its last line, all N workers are already alive and waiting — not starting, not loading, **waiting**. Cold-start variance collapses to zero. Every worker starts from the same state at the same moment.

```
Before:  template (5s) → workers cold-start (5s each) → work begins
After:   template (5s)
         └── workers warming in parallel
             → all ready when template finishes → work begins immediately
```

---

### StAR — Streaming Agent Reduce

**Traditional MapReduce:** wait for all workers → then reduce → get answer.

**StAR:** start reducing the moment the first worker finishes.

A persistent reducer session sits open. As each worker completes — whether it takes 10ms or 10 seconds — its result streams directly in. For any commutative operation (sum, average, ensemble, merge), you get a **progressively improving answer** while stragglers finish.

The final answer arrives exactly when the last worker does. Zero overhead after the last result.

---

### Coming: SAW — Shared Agent Workspace

**The biggest insight yet.**

What we call "AI context" is actually two separate things being conflated:

1. **Conversation history** — what was said, what was reasoned about
2. **Compute state** — what was actually *done*: variables, files, trained models

When you switch AI models mid-task, you only need to transfer #2. The compute state is the objective truth of where the work stands.

SAW makes the compute environment **model-agnostic**. Claude sets up a data pipeline, gets stuck. Codex picks up the exact same Python state — same variables, same loaded data, same trained models — in under a second. No re-explaining. No re-running.

> **Switch models, not workspaces.**

---

## Who It's For

### Students
Run a machine learning experiment today. Close your laptop. Come back tomorrow — your model is still trained, your variables still loaded, your results still there. greed-compute **auto-saves your session** when it expires. Restore it in under a second.

No more re-running three hours of training every morning.

### Researchers
Share a workspace with a collaborator. Both of you work in the same live Python environment — from different models, different machines, different time zones. The compute state is the shared ground truth.

### Startups
Replace expensive always-on infrastructure with on-demand agent compute. Pay for execution time, not idle servers. No DevOps. No containers to manage. An agent that can run code reliably is an agent that can ship products.

### Enterprises
Build multi-model workflows where each model does what it's best at:
- Claude for reasoning and planning
- Codex for implementation
- Gemini for analysis and summarization

All operating on **one shared persistent workspace**. Switch models mid-task without losing a single variable.

### AI Framework Builders
LangChain, CrewAI, AutoGen — any framework that ships next year. Plug in via MCP and get persistent, parallel, sandboxed compute for free. One URL.

---

## Enterprise Ready

| | Free | Pro | Enterprise |
|---|---|---|---|
| Requests/min | 60 | 300 | 2,000 |
| Executions/day | 100 | 5,000 | Unlimited |
| Swarms/day | 5 | 100 | Unlimited |
| Concurrent sessions | 3 | 20 | 100 |
| Max execution time | 30s | 120s | 600s |
| Checkpoint storage | 500 MB | 5 GB | 50 GB |
| Checkpoint retention | 7 days | 30 days | 90 days |
| Auto-save on expiry | ✅ | ✅ | ✅ |

Rate limiting is enforced by a **sliding window** per API key — no DB reads, zero latency overhead. Stripe-powered plan upgrades apply instantly via webhook.

---

## Security

- **OS-level isolation** via nsjail v3.4 — every worker runs in a separate Linux namespace
- **Network blocked by default** — agents cannot make outbound calls
- **Blocked dangerous imports** — `socket`, `subprocess`, `ctypes`, `threading` and more
- **Resource limits** — memory capped, CPU limited, file descriptor limits enforced
- **GPU-tier package blocking** — torch, tensorflow, jax blocked on CPU tier with clear error

---

## How to Get It

**Step 1: Get an API key**
```bash
curl -X POST https://compute.deep-ml.com/v1/admin/keys \
  -H "Content-Type: application/json" \
  -d '{"name": "my-agent", "tier": "free"}'
```

**Step 2: Connect any AI agent via MCP** *(one line of config)*
```json
{
  "type": "http",
  "url": "https://compute.deep-ml.com/v1/mcp?api_key=greed_..."
}
```

**Step 3: Or use the REST API directly**
```bash
# Create a persistent session
POST /v1/session/create

# Run code — state persists between calls
POST /v1/session/{id}/execute
{ "code": "import numpy as np\ndata = np.random.randn(1000, 10)" }

# Save state to disk
POST /v1/session/{id}/checkpoint
{ "name": "after-data-load" }

# Restore into a new session tomorrow
POST /v1/session/create
{ "checkpoint_id": "..." }

# Run a parallel agent swarm
POST /v1/swarm
{
  "template_code": "import numpy as np\nnp.random.seed(42)",
  "map_fn": "result = float(np.mean(partition))",
  "data": [[1,2,3], [4,5,6], [7,8,9]],
  "reduce_fn": "final = sum(r['result'] for r in results) / len(results)"
}
```

---

## The Stack

Built for reliability, not demos.

- **Rust + Axum** — async HTTP server, zero-cost concurrency
- **Python workers** — persistent interpreter processes, JSON protocol over stdin/stdout
- **nsjail** — OS-level sandboxing, production-grade
- **dill** — full Python interpreter state serialization
- **SQLite + WAL** — persistent jobs, checkpoints, usage events
- **DashMap** — lock-free in-memory rate limiting
- **Stripe** — usage metering and plan management

---

## The Vision

Right now, every AI tool is a brilliant mind in a room with no memory and no tools.

greed-compute gives AI agents a workbench that remembers.
A whiteboard that doesn't get wiped.
Hands that can actually build things.

The models are getting better every month. The infrastructure to run them reliably has barely moved. We're building the missing layer — the **persistent compute substrate** that makes AI agents reliable, collaborative, and actually useful for real work.

**The AI has the brain. We give it the workbench.**

---

*greed-compute — built by [deep-ml.com](https://deep-ml.com)*
*API: `https://compute.deep-ml.com`*
*Questions: reach out at deep-ml.com*
