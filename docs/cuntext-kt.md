# KT: cuntext format — for new Claude Code session

## What is cuntext

`.cuntext` is a file format for LLM-native API documentation. It was designed and built during a greed-compute session. The core idea: existing doc formats (markdown, OpenAPI, llms.txt) were written for humans. Agents pay token costs to load them even though 80% of the content is irrelevant to the task at hand.

`.cuntext` is:
- **Goal-oriented** — index keyed by what the agent wants to do, not what endpoints exist
- **Two-level** — tiny `index.cuntext` (~200t) always loaded, fragments (~100-400t) loaded on demand
- **Static** — just files over HTTP, zero infrastructure
- **Dense** — no markdown decorators, no prose, pattern-matchable schema

## Repo

`https://github.com/Adityakhalkar/cuntext`

Current contents (just pushed):
```
README.md                                    ← overview + token comparison table
SPEC.md                                      ← formal spec v0.1
examples/greed-compute/index.cuntext         ← reference implementation index
examples/greed-compute/fragments/
  exec.cuntext
  checkpoint.cuntext
  swarm.cuntext
  workspace.cuntext
  billing.cuntext
  errors.cuntext
```

## The format — quick reference

### index.cuntext structure
```
{API-NAME} v{VERSION}
base={base_url}
auth=header:{Header-Name}
auth-setup={how to get a key}
format=json content-type=application/json

goals:
  {what agent wants to do}  → fragments/{name}.cuntext

quick-ref:
  {label}  {METHOD} {path}  body:{hint}

on-error: 401=... 404=... 429=...
errors: → fragments/errors.cuntext
```

### Fragment structure
```
{NAME} [{human description}]
deps={other fragments needed}

{operation}:
  {METHOD} /path
  body: {field:type, optional?:type}
  → {response shape}
  note: important caveat

ex:
  # inline example
```

### Type conventions
- `str`, `int`, `float`, `bool`, `any`, `[type]`, `type|null`
- `field?` = optional
- `→` = response
- `ex:` = example block
- `note:` = caveat
- `deps=` = what else the agent should load

## Key design decisions and why

**Goals over endpoints** — The insight: agents know what they *want to do*, not what your API *is*. Mapping `run-code → exec.cuntext` is more useful than listing `POST /sessions`.

**`include_str!` in greed-compute** — The `.cuntext` files are embedded into the Rust binary at compile time. This means they're served directly from the API with no file I/O. greed-compute serves them at:
- `GET /v1/cuntext/index.cuntext`
- `GET /v1/cuntext/fragments/:name`
- `GET /v1/llms.cuntext` (auto-discovery alias)

**`deps=` field** — Each fragment declares what other fragments an agent should also load. This lets agents do transitive loading: load `swarm.cuntext`, see `deps=index.cuntext exec.cuntext`, load those too.

## Agent effectiveness test results

Ran a test: gave a fresh LLM agent *only* `index.cuntext` + `exec.cuntext` (no other docs), asked it to write a full Python script (create session → install numpy → compute → checkpoint → close).

**Results:**
- Got base URL, auth header, content-type exactly right ✅
- Session create → execute → close flow: 100% correct ✅
- State persistence understood ✅
- Error handling pattern correct ✅
- Inferred checkpoint body `{"name": ...}` correctly from quick-ref hint alone ✅
- Flagged honestly that checkpoint fragment wasn't loaded (lazy loading working as designed) ✅

**Gaps found and fixed:**
- Added `auth-setup=` line to index (how to get an API key)
- Added `body:{name:str}` hint to checkpoint in quick-ref
- Added `on-error:` inline summary so agents don't need `errors.cuntext` for common cases

**Token caveat** — Total agent call was ~10k tokens, but only ~500 of those were the cuntext files. The rest was the agent's reasoning + code generation. Format is most valuable when agents are juggling 10+ tools simultaneously (each tool's docs compete for context window).

## Business analysis summary

- Token savings come out of the LLM provider's (OpenAI/Anthropic) bill, not greed-compute's
- greed-compute charges for execution, not documentation
- `.cuntext` reduces friction → more agents adopt → more executions → net positive for greed-compute
- If `.cuntext` becomes a standard: services without it are harder for agents to use = competitive moat for early adopters
- Most valuable for: multi-tool agents, tight context windows, repeated doc references

## What still needs to be done on cuntext repo

### Immediate
- [ ] The greed-compute `examples/` were copied from a branch — review them against the latest greed-compute main (SAW/workspace endpoints were added after the cuntext files were first written, `workspace.cuntext` should be verified)
- [ ] Add a `CONTRIBUTING.md` — how to write `.cuntext` files for your own API
- [ ] Add a second `examples/` implementation (even a simple fake API) to show the format is generic, not greed-compute-specific

### Short term
- [ ] `cuntext.org` or similar — the spec needs a home for discoverability
- [ ] Parser/validator — a small Python or JS library that validates a `.cuntext` file against the spec
- [ ] Auto-discovery convention — should `yourdomain.com/llms.cuntext` be the standard discovery path? (greed-compute implements this already)
- [ ] Version the spec properly — SPEC.md is v0.1, should have a changelog

### Open questions to think about
- Should fragments support `if-goal:` conditionals? (e.g., load extra context only if agent is doing X)
- Should there be a standard for streaming APIs vs request/response? (SSE is currently just noted as `[SSE]`)
- Can the format express pagination? (e.g., `GET /checkpoints?cursor=...`)
- Should `deps=` be load-order-aware or just advisory?

## How greed-compute uses cuntext

The `.cuntext` files live at `greed-compute/docs/cuntext/` and are embedded at compile time via Rust's `include_str!`. When you update the cuntext spec or examples, you need to:
1. Update `examples/greed-compute/` in the `cuntext` repo
2. Copy updated files to `greed-compute/docs/cuntext/`
3. Rebuild greed-compute

Eventually this should be automated (git submodule or a copy script in CI).

## Relevant prior art

- `llms.txt` (Jeremy Howard / Answer.AI, Sep 2024) — prose markdown at `/.llms.txt`. Better than HTML but not structured for machines. No lazy loading.
- OpenAPI/Swagger — complete but verbose (1,200+ lines for PetStore). Designed for code gen, not LLM context.
- MCP (Model Context Protocol) — requires a running server. Not static files.
- LlamaIndex — RAG at runtime, needs embedding infrastructure. Not portable.

cuntext fills the gap: structured for machines, static, zero-infrastructure, goal-oriented.
