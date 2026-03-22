# greed-compute Architecture

**One-liner:** Code Interpreter for AI agents. Give any LLM the power to compute, not just chat.

---

## What This Is

A stateless, ephemeral compute service that lets AI agents execute Python code with ML libraries (numpy, pandas, torch, sklearn) via API or MCP. Agents submit code, get results back, sandbox is wiped.

Think: ChatGPT Code Interpreter, but as an API any agent can call.

## What This Is NOT

- Not a GPU compute platform (that's Modal)
- Not a generic sandbox (that's E2B/Daytona)
- Not a PyTorch replacement
- Not a notebook

---

## Why This Exists

Today, AI agents can think, search, read files, and call APIs. But they can't compute. They can't take a CSV and run pandas on it. They can't verify their own math. They can't train a model.

greed-compute is the missing tool.

### Who Pays

| Customer | What They Buy | Why |
|----------|-------------|-----|
| AI agent builders (LangChain, CrewAI, n8n) | MCP tool / REST API | Their agents need compute |
| SaaS companies embedding agents | White-label API | Add data analysis without building infra |
| Enterprise AI teams | Self-hosted | Agent compute inside their VPC |
| Deep-ML | Internal use | Agent-graded ML challenges |

### Competitive Position

| | E2B | Daytona | Modal | greed-compute |
|---|---|---|---|---|
| Focus | Generic sandbox | Speed | Heavy GPU | ML-aware agent compute |
| Isolation | Firecracker microVM | Docker | Container | Process sandbox |
| Cold start | 150ms | 27-90ms | sub-second | <100ms (warm pool) |
| ML libraries | Install yourself | Install yourself | Pre-built images | Pre-installed, optimized |
| Pricing | ~$0.05/hr | ~$0.067/hr | $30/mo + usage | $0.001/execution |
| GPU | No | No | Yes (expensive) | wgpu (future, any GPU) |
| MCP native | No | No | No | Yes |

---

## Architecture

### System Overview

```
┌─────────────────────────────────────────────────────┐
│              AI Agent (Claude, GPT, Llama)           │
│              via MCP tool or REST API                │
└───────────────────────┬─────────────────────────────┘
                        │
┌───────────────────────▼─────────────────────────────┐
│                   API Gateway                        │
│   Axum HTTP server (Rust)                            │
│   ┌──────────┐ ┌──────────┐ ┌────────────────────┐  │
│   │ Auth     │ │ Rate     │ │ Usage Tracking     │  │
│   │ (API key)│ │ Limiting │ │ (SQLite)           │  │
│   └──────────┘ └──────────┘ └────────────────────┘  │
└───────────────────────┬─────────────────────────────┘
                        │
┌───────────────────────▼─────────────────────────────┐
│               Session Orchestrator                   │
│   ┌──────────┐ ┌──────────┐ ┌────────────────────┐  │
│   │ Session  │ │ Warm     │ │ TTL Sweeper        │  │
│   │ Router   │ │ Pool     │ │ (kills expired)    │  │
│   └──────────┘ └──────────┘ └────────────────────┘  │
└───────────────────────┬─────────────────────────────┘
                        │
┌───────────────────────▼─────────────────────────────┐
│              Sandbox (per session)                    │
│   ┌──────────────────────────────────────────────┐   │
│   │  Isolated Python Process                      │   │
│   │  ┌────────────┐ ┌─────────────────────────┐  │   │
│   │  │ Pre-loaded │ │ Workspace (/tmp/session) │  │   │
│   │  │ Libraries  │ │ (uploads, outputs)       │  │   │
│   │  │ numpy      │ └─────────────────────────┘  │   │
│   │  │ pandas     │                               │   │
│   │  │ sklearn    │  Resource Limits:             │   │
│   │  │ matplotlib │  - Memory: 512MB cap          │   │
│   │  │ torch*     │  - CPU: 30s timeout           │   │
│   │  │ scipy      │  - Disk: 100MB per session    │   │
│   │  └────────────┘  - Network: disabled          │   │
│   └──────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────┘

* torch = greed-torch (lightweight, wgpu-backed in future)
```

### Three-Layer Design

#### Layer 1: API Gateway (Rust/Axum)

Handles auth, routing, rate limiting, usage tracking. Pure Rust, no Python dependency at this layer. This is what makes it fast — the HTTP overhead is microseconds, not milliseconds.

```
POST /v1/session/create          → { session_id, expires_at }
POST /v1/session/{id}/execute    → { stdout, result, error, duration_ms }
POST /v1/session/{id}/files      → upload file to workspace
GET  /v1/session/{id}/output/{f} → download output file
GET  /v1/session/{id}/status     → { active, ttl_remaining }
DELETE /v1/session/{id}          → terminate + wipe
POST /v1/admin/keys              → create API key
GET  /v1/health                  → server status
```

#### Layer 2: Session Orchestrator (Rust)

Manages the lifecycle of sandboxed Python processes.

**Warm Pool:** Pre-spawn N Python processes with libraries loaded. When a session is created, assign a warm process instead of cold-starting. Target: <100ms from `create_session` to first `execute`.

**Session State:** Variables persist between `execute` calls within a session (Jupyter-style). State dies when session terminates.

**TTL Sweeper:** Background task runs every 30s, kills sessions past their expiry. Wipes workspace directory.

**Concurrency:** DashMap for session lookup. Each session gets its own Python process — no GIL contention between sessions.

#### Layer 3: Sandbox (Python process)

Each session runs in an isolated Python subprocess with:

**Pre-installed libraries:**
- `numpy`, `scipy` — numerical computing
- `pandas` — data manipulation
- `matplotlib`, `seaborn` — visualization (base64-encoded PNG output)
- `scikit-learn` — ML models
- `torch` (greed-torch) — tensor ops, neural nets

**Security boundaries:**
- Separate OS process (not just a thread)
- Memory limit via `resource.setrlimit` (RLIMIT_AS)
- CPU timeout via `signal.alarm`
- No network access (network namespace isolation)
- No filesystem access outside `/workspace/{session_id}/`
- Restricted imports (blocked: subprocess, socket, shutil.rmtree, etc.)

**Communication:** Parent (Rust) to Child (Python) via stdin/stdout JSON protocol:
```json
→ {"type": "execute", "code": "import numpy as np\nprint(np.mean([1,2,3]))"}
← {"type": "result", "stdout": "2.0\n", "error": null, "duration_ms": 3}
```

This is simpler and more robust than PyO3 embedding. Each session is a process — if it crashes, only that session dies.

---

## The greed-torch Question

### MVP (Phase 1): No GPU needed

For 95% of agent workloads, CPU is enough:
- Data analysis with pandas/numpy
- sklearn model training on small datasets
- Basic tensor math
- Chart generation

Install real lightweight libraries. No polyfill. No custom torch. Just Python with batteries included.

### Phase 2: greed-torch (Rust-backed tensor library)

When we see demand for GPU-accelerated agent workloads, build greed-torch:

```
Python agent code
  → import greed_torch as torch  (or monkey-patch)
    → PyO3 bridge to Rust
      → Smart dispatch:
        Small ops (<1K elements) → ndarray (CPU)
        Large ops (>1K elements) → wgpu compute shaders (any GPU)
      → Results back to Python
```

Built on **Burn** or **CubeCL** — existing Rust ML frameworks that already have wgpu backends. Don't reinvent the tensor engine.

### Phase 3: Premium GPU tier

Offer GPU-accelerated sessions as a premium tier:
- Free: CPU-only, 30s timeout, 256MB
- Pro: wgpu GPU, 5min timeout, 1GB
- Enterprise: dedicated resources, custom limits

---

## MCP Server

The primary distribution channel. Package greed-compute as an MCP server so any MCP-compatible agent gets instant access.

```json
{
  "name": "greed-compute",
  "description": "Execute Python code with ML libraries. Data analysis, model training, tensor ops.",
  "tools": [
    {
      "name": "create_session",
      "description": "Create an ephemeral compute session with numpy, pandas, sklearn, and torch pre-installed. Returns session_id. Always call this first.",
      "input_schema": {
        "type": "object",
        "properties": {
          "ttl_seconds": {
            "type": "integer",
            "description": "Session lifetime in seconds. Default 300 (5 min).",
            "default": 300
          }
        }
      }
    },
    {
      "name": "execute_code",
      "description": "Run Python code in the session. Variables persist between calls (Jupyter-style). Returns stdout and any errors.",
      "input_schema": {
        "type": "object",
        "properties": {
          "session_id": { "type": "string" },
          "code": { "type": "string", "description": "Python code to execute" }
        },
        "required": ["session_id", "code"]
      }
    },
    {
      "name": "upload_file",
      "description": "Upload a file (CSV, JSON, etc.) to the session workspace for use in code. Content must be base64-encoded.",
      "input_schema": {
        "type": "object",
        "properties": {
          "session_id": { "type": "string" },
          "filename": { "type": "string" },
          "content_base64": { "type": "string" }
        },
        "required": ["session_id", "filename", "content_base64"]
      }
    },
    {
      "name": "download_file",
      "description": "Download an output file from the session workspace (charts, CSVs, models).",
      "input_schema": {
        "type": "object",
        "properties": {
          "session_id": { "type": "string" },
          "filename": { "type": "string" }
        },
        "required": ["session_id", "filename"]
      }
    },
    {
      "name": "terminate_session",
      "description": "End the session and wipe all files. Always call this when done.",
      "input_schema": {
        "type": "object",
        "properties": {
          "session_id": { "type": "string" }
        },
        "required": ["session_id"]
      }
    }
  ]
}
```

---

## Build Phases

### Phase 1 — MVP (current to 2 weeks)

- [x] Axum API server with session management
- [x] Python execution via PyO3
- [x] SQLite auth + API keys
- [x] TTL sweeper
- [ ] Switch from PyO3 to subprocess sandbox model
- [ ] Pre-install numpy, pandas, sklearn, matplotlib, scipy
- [ ] Warm process pool (pre-spawn 3-5 Python processes)
- [ ] Resource limits (memory, CPU, disk, network)
- [ ] Import restriction (block dangerous modules)
- [ ] File upload/download working end-to-end
- [ ] Rate limiting per API key tier
- [ ] Deploy to VPS

### Phase 2 — MCP + Distribution (2 weeks)

- [ ] Package as MCP server
- [ ] Test with Claude Desktop + Claude Code
- [ ] Test with OpenClaw
- [ ] Submit to MCP registries
- [ ] Landing page
- [ ] Deep-ML integration (first customer)

### Phase 3 — greed-torch + GPU (when demand justifies)

- [ ] Build greed-torch on Burn/CubeCL
- [ ] wgpu compute backend
- [ ] PyO3 bridge for Python interop
- [ ] GPU tier pricing
- [ ] Session pooling with GPU affinity

### Phase 4 — Scale

- [ ] Usage dashboard + Stripe billing
- [ ] Multi-node deployment
- [ ] Kubernetes operator for self-hosted enterprise
- [ ] SOC2 / compliance story

---

## Benchmarks to Track

| Metric | Target | Why |
|--------|--------|-----|
| Cold start (create to first exec) | <100ms (warm) | Agents won't wait. E2B is 150ms. Beat them. |
| Warm execution | <50ms overhead | The Python code runtime is the bottleneck, not us |
| Concurrent sessions | 20+ on 4GB VPS | Cost efficiency |
| Session cleanup | <10ms | tmpfs wipe |
| Memory per session | <150MB baseline | Density on cheap VPSes |
| API latency (non-exec) | <5ms | Rust advantage |

---

## Key Design Decisions

1. **Subprocess, not embedded Python.** PyO3 shares process space — one bad script can corrupt the host. Subprocess gives true isolation, crash safety, and resource limits for free via the OS.

2. **Warm pool, not cold start.** Pre-spawn Python processes with libraries loaded. Assign on session create. Recycle on terminate. Cold start becomes warm handoff.

3. **Python is the API.** Agents are trained on Python. Any other input format (DSL, JSON graph) adds friction and reduces adoption. Keep the interface familiar.

4. **CPU first, GPU later.** 95% of agent workloads don't need GPU. Ship fast, prove demand, add GPU when customers ask for it.

5. **MCP is the distribution.** Don't build a UI. Agents are the customers. MCP packaging is the go-to-market. Every Claude, Cursor, and OpenClaw user gets instant access.

6. **Per-execution pricing, not per-hour.** Agents make hundreds of quick calls, not long-running jobs. $0.001/execution aligns with usage patterns. E2B/Daytona charge per-hour, which penalizes bursty agent workflows.

7. **REST-first, agent-native.** Any agent that can make HTTP calls uses greed-compute directly (OpenClaw via web.fetch, LangChain via tool calling). MCP wrapper optional for Claude Desktop. No special SDK required.

---

## Prior Art & Research

### Academic Papers

**LLM-in-Sandbox: Elicits General Agentic Intelligence** (arxiv:2601.16206, Jan 2026)
- Closest academic validation of our approach
- Provides a lightweight Python interpreter with only numpy/scipy pre-installed, delegates tool acquisition to the model
- Key finding: strong LLMs spontaneously use the sandbox for non-code tasks — accessing external resources, using the filesystem for long-context management, executing scripts for formatting
- Implication for us: **consider relaxing the network block** for pro-tier sessions, since agents benefit from installing packages on-the-fly
- Enhanced capabilities via LLM-in-Sandbox Reinforcement Learning (using non-agentic data)

**RedCodeAgent** (ICLR 2026, openreview.net/pdf?id=IyIaAOihmZ)
- Automated red-teaming for code agents — tested OpenCodeInterpreter, ReAct, MetaGPT, Cursor, Codeium
- Found real vulnerabilities across all tested agents
- Implication for us: our import blocklist is necessary but insufficient. Need **defense in depth**: process isolation + syscall filtering + filesystem jail + resource limits. No single layer is enough.

**AgentSpec: Customizable Runtime Enforcement** (ICSE '26, April 2026)
- Runtime enforcement for safe/reliable LLM agents via customizable policies
- Implication for us: consider exposing a policy language so enterprise customers can define their own sandbox rules (allowed imports, network access, resource limits)

**Securing AI Agent Execution** (arxiv:2510.21236, Oct 2025)
- Comprehensive framework for agent security: the threat model now includes the workload itself (LLM-generated code may be adversarial due to prompt injection)
- Implication for us: assume ALL submitted code is potentially malicious, even from "trusted" agents

### Industry Systems

**E2B** (e2b.dev)
- Firecracker microVM-based, <5MB memory overhead per VM, <200ms boot
- Uses Dockerfile as build artifact to create snapshotted microVMs — restore-from-snapshot is their speed trick
- 200M+ sandboxes served as of 2026
- Strength: strongest isolation (hardware-level via KVM). Weakness: generic, no ML focus, per-hour pricing
- What we learn: snapshot-based warm pools are proven at scale

**Google Agent Sandbox** (Kubernetes SIG, github.com/kubernetes-sigs/agent-sandbox)
- Kubernetes CRD + warm pod pool orchestrator
- SandboxWarmPool maintains pre-warmed pods, claims ready instances on creation
- Sub-second latency, 90% improvement over cold starts
- What we learn: warm pool pattern is the industry standard. Our subprocess pool is the right approach at smaller scale.

**Daytona** (daytona.io)
- Docker containers, 27-90ms cold start (fastest in industry)
- Computer Use support for browser/desktop automation
- What we learn: raw speed matters, but they have no ML specialization

**llm-sandbox** (github.com/vndee/llm-sandbox)
- Open-source, lightweight Python sandbox library
- Docker/process isolation modes
- Closest open-source competitor to our approach
- What we learn: the basic idea exists as open source. Our differentiation must come from ML-awareness and the wgpu roadmap.

**Monty sandbox** (from Arize Phoenix research)
- 3-15 microsecond latency — 10,000x faster than subprocess
- Uses restricted Python AST execution, no OS isolation
- What we learn: for trusted environments, AST-level restriction is dramatically faster. Could be an option for internal/trusted agent tiers.

### Performance Benchmarks from Research

| Approach | Latency | Isolation Level | Density |
|----------|---------|-----------------|---------|
| Monty (AST restriction) | 3-15 us | None (same process) | Unlimited |
| Container pool (warm) | 1-5ms | Container kernel | High |
| MicroVM pool (Firecracker) | 1-2ms (warm) | Hardware (KVM) | Medium (~5MB/VM) |
| Subprocess (our approach) | ~10-50ms | Process-level | High (~150MB/session) |
| CPython WASM (wasmtime) | ~30ms p50 | WASM sandbox | Medium |
| Cold Firecracker boot | 125ms | Hardware (KVM) | N/A |
| Cold Docker | 200-500ms | Container kernel | N/A |

### What the Research Changes About Our Architecture

1. **Network access policy**: LLM-in-Sandbox shows agents benefit from installing packages on-the-fly. Consider a tiered model:
   - Free: no network (fully sandboxed)
   - Pro: allow pip install from a whitelist of safe packages
   - Enterprise: configurable network policy

2. **Defense in depth**: RedCodeAgent proves import blocklists alone are insufficient. Our target architecture should stack:
   - Process isolation (separate PID namespace)
   - Syscall filtering (seccomp-bpf)
   - Filesystem jail (chroot or bind mount)
   - Resource limits (cgroups/rlimit)
   - Import blocklist (Python-level)

3. **Warm pool is validated**: Google's 90% latency improvement and E2B's snapshot-restore approach both confirm our warm pool decision. The subprocess model is the right abstraction for our scale.

4. **Moat is in Phase 3**: The sandbox layer is table stakes (everyone has one). The real differentiation is greed-torch with wgpu — vendor-agnostic GPU compute for agent workloads. Nobody else is building this.
