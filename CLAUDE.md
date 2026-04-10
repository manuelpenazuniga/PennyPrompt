# CLAUDE.md — PennyPrompt Development Guide

## Project Overview

PennyPrompt is a local reverse proxy (~15MB single Rust binary) that provides cost guardrails for AI agents. It sits between any OpenAI-compatible AI agent (OpenClaw, claw-code, Cursor, Codex) and LLM providers (Anthropic, OpenAI), providing atomic budget enforcement, runaway loop detection, pre-execution cost estimation, and forensic cost reporting.

**This file is the source of truth for all development work.** Read it fully before writing any code.

---

## Core Architecture

```
Agent ──→ Proxy Plane (:8585) ──→ Provider (Anthropic/OpenAI)
               │
          Normalize → Reserve (ledger) → Dispatch → Reconcile
               │            │
          Auto-detect    Budget check
          project/session  (atomic, SQLite tx)
               │
          Admin Plane (unix socket / :8586)
          Reports, budgets, health, events
```

### Key Design Decisions — DO NOT DEVIATE

1. **HTTP 402 for budget blocks, never 429.** Agents retry 429 (rate limit). 402 with `"retryable": false` in the body is correct for budget exhaustion.

2. **Two modes: `observe` and `guard`.** In `guard`, if the budget engine or SQLite fails, the request is BLOCKED (fail-closed). In `observe`, it passes but logs a `mode_failsafe` event. Never fail-open in guard mode.

3. **Atomic budgets via cost ledger.** Every request goes through: RESERVE (estimated cost, SQLite transaction) → DISPATCH → RECONCILE (actual cost). The reserve and budget check happen in the SAME SQLite transaction. This prevents concurrent requests from breaking the limit.

4. **Auto-attribution without custom headers.** Project is detected from git root of cwd. Session is grouped by time window (default 30min). The user gets useful reports from the first request without configuring anything.

5. **Pricebook is local and versioned.** `prices/*.toml` files with `effective_from`/`effective_until`. No scraping. No external API calls. Updated via `pennyprompt prices update` (downloads from GitHub repo).

6. **Proxy plane and admin plane are separated.** Proxy on `:8585`, admin on unix socket (or `:8586` with token). Never expose admin endpoints on the proxy port.

7. **UUIDv7 for all IDs.** Temporally sortable. Better for SQLite range queries than UUIDv4.

8. **TOML for config, never YAML.** Strict typing. No ambiguity.

---

## Tech Stack

### Rust Crates — Pin These Versions

```toml
# Core
tokio = { version = "1", features = ["full"] }
axum = "0.8"
reqwest = { version = "0.12", features = ["json", "stream", "rustls-tls"], default-features = false }
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"

# CLI
clap = { version = "4", features = ["derive"] }

# Observability
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }

# Utilities
uuid = { version = "1", features = ["v7"] }
chrono = { version = "0.4", features = ["serde"] }
tiktoken-rs = "0.6"
sha2 = "0.10"
bytes = "1"
futures-util = "0.3"
comfy-table = "7"
indicatif = "0.17"
anyhow = "1"
thiserror = "2"
async-trait = "0.1"
```

### What is NOT in the stack

- No Python, Node.js, or external runtime
- No Docker as a requirement
- No vector database, Redis, or external service
- No ML crates (prompt classification is the job of routers like NadirClaw)
- No scraping for pricing

---

## Workspace Structure

```
pennyprompt/
├── Cargo.toml                      # Workspace root — members = ["crates/*"]
├── rust-toolchain.toml             # Pin to stable
├── .cargo/
│   └── config.toml                 # Workspace-level cargo config
├── prices/
│   ├── anthropic.toml              # Pricebook: model, input_per_mtok, output_per_mtok, effective_from
│   ├── openai.toml
│   └── VERSION                     # ISO date of last update
├── presets/
│   ├── indie.toml                  # $30/mo, $10/day hard, guard, burn-rate $10/hr
│   ├── team.toml                   # $100/mo, $20/day hard, guard, burn-rate $15/hr
│   └── explore.toml                # $10/mo soft only, observe, burn-rate $5/hr
├── config/
│   └── default.toml                # Full reference config with all fields documented
├── migrations/
│   ├── 0001_projects_sessions.sql
│   ├── 0002_providers_models.sql
│   ├── 0003_pricebook.sql
│   ├── 0004_requests_usage.sql
│   ├── 0005_budgets.sql
│   ├── 0006_cost_ledger.sql
│   └── 0007_events.sql
├── crates/
│   ├── penny-types/
│   ├── penny-config/
│   ├── penny-store/
│   ├── penny-cost/
│   ├── penny-ledger/
│   ├── penny-budget/
│   ├── penny-detect/
│   ├── penny-providers/
│   ├── penny-proxy/
│   ├── penny-admin/
│   ├── penny-cli/
│   └── penny-observe/
├── tests/
│   ├── integration/
│   ├── fixtures/                   # Captured real payloads for testing
│   └── golden/                     # Snapshot tests for CLI output
├── docs/
│   ├── PennyPrompt-v2.md           # Full project specification
│   ├── INSTALL.md
│   ├── QUICKSTART.md
│   ├── CONFIG-REFERENCE.md
│   ├── ARCHITECTURE.md
│   └── PRICEBOOK.md
└── scripts/
    ├── install.sh
    └── release.sh
```

---

## Crate Dependency Graph

```
penny-cli
  └─→ penny-proxy
        ├─→ penny-budget
        │     ├─→ penny-ledger
        │     │     ├─→ penny-cost
        │     │     │     ├─→ penny-types
        │     │     │     └─→ penny-config
        │     │     └─→ penny-store
        │     └─→ penny-detect
        │           └─→ penny-observe
        └─→ penny-providers
              └─→ penny-types

penny-admin (separate binary or feature)
  ├─→ penny-store
  ├─→ penny-budget
  └─→ penny-cost
```

**Rule: penny-types and penny-config have ZERO internal dependencies.** They are leaf crates. Everything flows down from penny-cli.

---

## Module Specifications

### penny-types

Shared types used across all crates. No business logic. No I/O.

Key types:
```rust
// Identifiers
pub type RequestId = String;    // UUIDv7
pub type SessionId = String;    // UUIDv7
pub type ProjectId = String;    // slug derived from git root or "default"

// Core structs
pub struct NormalizedRequest { id, project_id, session_id, model_requested, model_resolved, provider_id, messages: serde_json::Value, stream: bool, estimated_input_tokens, estimated_output_tokens, timestamp }
pub struct ProviderResponse { status: u16, body: ResponseBody, upstream_ms: u64 }
pub enum ResponseBody { Complete(serde_json::Value), Stream(Receiver<Bytes>) }
pub struct AccountedUsage { input_tokens, output_tokens, cost_usd, source: UsageSource, pricing_snapshot: serde_json::Value }
pub enum UsageSource { Provider, Estimated, Heuristic }

// Budget
pub struct Budget { id, scope_type: ScopeType, scope_id, window_type: WindowType, hard_limit_usd: Option<f64>, soft_limit_usd: Option<f64>, action_on_hard, action_on_soft }
pub enum ScopeType { Global, Project, Session }
pub enum WindowType { Day, Week, Month, Total }
pub enum RouteDecision { Allow { warnings }, Block { reason, detail: BudgetBlockDetail }, Failsafe { mode: Mode, reason } }
pub enum Mode { Observe, Guard }

// Ledger
pub struct LedgerEntry { id, request_id, entry_type: LedgerEntryType, budget_id, amount_usd, running_total, created_at }
pub enum LedgerEntryType { Reserve, Reconcile, Release }
pub enum Reservation { Granted { entries, remaining_by_budget }, Denied { budget, accumulated, limit, reason } }

// Detection
pub struct RequestDigest { model, input_tokens, cost_usd, tool_name: Option<String>, tool_succeeded: bool, content_hash: u64, timestamp }
pub enum DetectAlert { ToolLoop { tool_name, failure_count }, BurnRate { usd_per_hour, threshold }, ContentLoop { similar_count, window_seconds } }

// Cost estimation
pub struct CostRange { min_usd, max_usd, confidence: Confidence }
pub enum Confidence { High, Medium, Low }
pub enum TaskType { SinglePass, MultiRound, AgentTask }

// Events
pub struct Event { id, request_id: Option, session_id: Option, event_type: EventType, severity: Severity, detail: serde_json::Value, created_at }
pub enum EventType { BudgetCheck, BudgetBlock, BudgetWarn, Reserve, Reconcile, Release, LoopDetected, BurnRateAlert, SessionPaused, ModeFailsafe }
pub enum Severity { Info, Warn, Error, Critical }

// Errors
pub use thiserror for all error types.
// PennyError is the top-level error enum with variants per subsystem.
```

### penny-config

Loads and validates TOML config. Merges with env var overrides (`PENNY_` prefix). Loads presets. Returns strongly-typed `AppConfig`.

Key responsibilities:
- Load `~/.config/pennyprompt/config.toml` (or path from `PENNY_CONFIG`)
- Merge env var overrides: `PENNY_SERVER_BIND`, `PENNY_DEFAULTS_MODEL`, etc.
- Load presets from `presets/*.toml` when `--preset` is used
- Validate: required fields present, provider URLs parseable, budget limits > 0
- Load pricebook from `prices/*.toml` files

Exports: `AppConfig`, `ProviderConfig`, `BudgetConfig`, `DetectConfig`, `AttributionConfig`

### penny-store

SQLite repository layer. Uses sqlx with compile-time checked queries. Runs migrations on startup.

Key responsibilities:
- Run migrations from `migrations/` directory
- SQLite in WAL mode (set on connection)
- Repository traits: `ProjectRepo`, `SessionRepo`, `RequestRepo`, `BudgetRepo`, `LedgerRepo`, `EventRepo`, `PricebookRepo`
- All writes use transactions where atomicity matters
- Connection pool via sqlx::SqlitePool

**Important**: The `LedgerRepo::reserve` method MUST execute the budget check AND the ledger insert in the SAME SQLite transaction. This is the core atomicity guarantee.

### penny-cost

Pricing engine. Given (model, input_tokens, output_tokens) → cost in USD. Also provides token estimation via tiktoken-rs and cost range estimation.

Key functions:
- `calculate(model_id, input_tokens, output_tokens) → f64`
- `estimate_tokens(messages: &Value) → (input_est, output_est)`
- `estimate_range(model_id, context_tokens, task_type) → CostRange`
- `snapshot(model_id) → Value` — returns the pricebook entry used for audit trail

Token estimation strategy:
1. Use tiktoken-rs with cl100k_base encoding (works for OpenAI and Anthropic)
2. For output estimation: `min(input_tokens * 0.3, 4096)` as default heuristic
3. If tiktoken-rs doesn't support the model's tokenizer: fallback to `chars / 4`
4. Always tag the estimation source: `Provider | Estimated | Heuristic`

### penny-ledger

Append-only cost ledger. The core of atomic budget enforcement.

Three operations:
- `reserve(request_id, budgets, estimated_cost) → Reservation` — Within a SQLite transaction: for each applicable budget, check if `running_total + estimated_cost > hard_limit`. If any budget would exceed: rollback, return `Denied`. If all pass: insert `Reserve` entries, update running_total, commit, return `Granted`.
- `reconcile(request_id, actual_cost)` — Insert `Reconcile` entry. Adjust running_total by `(actual_cost - reserved_cost)`.
- `release(request_id)` — Insert `Release` entry. Subtract reserved cost from running_total. Used when request fails/cancels before dispatch.

**Critical implementation detail**: The reserve method MUST use `BEGIN IMMEDIATE` transaction to prevent SQLITE_BUSY race conditions. With WAL mode and IMMEDIATE, only one writer at a time can reserve against a budget.

### penny-budget

Budget evaluation layer. Sits above penny-ledger. Handles mode logic (observe/guard) and soft limit warnings.

Key function:
```rust
fn evaluate(&self, request: &NormalizedRequest, estimated_cost: f64) -> Result<RouteDecision>
```

Logic:
1. Find all applicable budgets (global, project-specific, session-specific)
2. Call `ledger.reserve(...)` for hard limit check
3. If `Denied` and mode == `guard` → return `Block`
4. If `Denied` and mode == `observe` → return `Allow` with warning, log event
5. If DB error and mode == `guard` → return `Failsafe` (block), log `ModeFailsafe` event
6. If DB error and mode == `observe` → return `Failsafe` (allow), log `ModeFailsafe` event
7. Check soft limits → if exceeded, add warnings to `Allow`

### penny-detect

Runaway loop detector. Operates on a per-session sliding window of `RequestDigest` entries.

Implementation: in-memory `HashMap<SessionId, SessionWindow>` behind a `RwLock`. Fed after every request reconciliation.

Three heuristics (no ML, no embeddings):
1. **Tool failure repetition**: count requests with same `tool_name` and `tool_succeeded=false` in window → alert if count >= threshold
2. **Burn rate**: `total_cost_in_window / elapsed_hours` → alert if > threshold
3. **Content similarity**: count requests with same `content_hash` in window → alert if count >= threshold. Hash is computed as `sha2` of first 500 chars of the first user message.

Actions:
- `alert` → create `DetectAlert` event, emit via tracing. Visible in `pennyprompt tail`.
- `pause` → mark session as paused in memory. Subsequent requests to that session return HTTP 402 with `"reason": "session_paused_loop_detected"`. Resume via `pennyprompt detect resume <session_id>`.

### penny-providers

Adapter pattern. One adapter per provider. Normalizes request/response formats.

Trait:
```rust
#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    async fn send(&self, req: NormalizedRequest) -> Result<ProviderResponse>;
    fn provider_id(&self) -> &str;
    fn supports_model(&self, model: &str) -> bool;
}
```

Adapters to implement:
1. **MockProvider** — deterministic responses with configurable usage data. For testing.
2. **AnthropicAdapter** — translates to Anthropic Messages API format. Handles `x-api-key` header, `anthropic-version` header, content block format, streaming delta format.
3. **OpenAIAdapter** — native OpenAI format. Most direct pass-through.

Streaming: both adapters must handle SSE streaming. Forward chunks to client immediately via `tokio::sync::mpsc` channel. Accumulate content in background for token counting. Extract usage from final chunk (Anthropic: `message_delta` event with `usage`; OpenAI: last chunk with `usage` field) or fall back to tiktoken estimation.

### penny-proxy

Axum HTTP server. The proxy plane.

Endpoints:
- `POST /v1/chat/completions` — main handler
- `POST /v1/messages` — Anthropic native format
- `GET /v1/models` — list available models

Middleware stack (in order):
1. `request_id` — generate UUIDv7, add `X-Penny-Request-Id` to response
2. `tracing_span` — create tracing span for the request
3. `normalize` — extract model, estimate tokens, resolve provider, auto-detect project/session
4. Handler — budget check → dispatch → reconcile → detect feed

The main handler flow is the pipeline described in the architecture section. See `PennyPrompt-v2.md` section 15 for the detailed step-by-step.

### penny-admin

Admin plane. Separate from proxy. Binds to unix socket by default.

Endpoints:
- `GET /admin/health` — uptime, DB status, provider reachability, pricebook age
- `GET /admin/report/summary?project=&since=&until=&model=` — cost report
- `GET /admin/report/session/:id` — session detail
- `GET /admin/report/top?limit=N` — most expensive requests
- `GET /admin/budgets` — all budgets with current status (accumulated/limit/%)
- `POST /admin/budgets` — create/update budget at runtime
- `POST /admin/estimate` — route preview (cost estimate per model for a payload)
- `GET /admin/detect/status` — active alerts, paused sessions
- `POST /admin/detect/resume` — resume paused session
- `GET /admin/events` — SSE stream of events in real-time

### penny-cli

Clap CLI. All subcommands.

```
pennyprompt init [--preset indie|team|explore] [--provider PROVIDER]
pennyprompt serve [--daemon] [--mock]
pennyprompt estimate [--model MODEL] [--context-files GLOB]
pennyprompt report summary [--since DURATION] [--by project|model|session]
pennyprompt report session <SESSION_ID>
pennyprompt report top [--limit N]
pennyprompt budget list
pennyprompt budget set <SCOPE_TYPE:SCOPE_ID> <WINDOW> <LIMIT_USD>
pennyprompt budget reset <SCOPE_TYPE:SCOPE_ID> <WINDOW>
pennyprompt detect status
pennyprompt detect resume <SESSION_ID>
pennyprompt tail
pennyprompt doctor
pennyprompt prices show
pennyprompt prices update
pennyprompt config
pennyprompt version
```

Output formatting: use `comfy-table` for tables. Use Unicode box-drawing for report headers. Use colors for severity (green=ok, yellow=warn, red=error/block). Respect `NO_COLOR` env var.

### penny-observe

Tracing setup. Configures `tracing-subscriber` with env filter (`PENNY_LOG` or `RUST_LOG`). JSON output when `--json-log` flag is used. Structured fields: `request_id`, `session_id`, `project_id`, `model`, `cost_usd`, `event_type`.

---

## Database Schema

All migrations are in `migrations/` directory. They run automatically on startup via sqlx.

### 0001_projects_sessions.sql
```sql
CREATE TABLE projects (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    path        TEXT UNIQUE,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE sessions (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL REFERENCES projects(id),
    started_at  TEXT NOT NULL DEFAULT (datetime('now')),
    closed_at   TEXT,
    source      TEXT NOT NULL DEFAULT 'auto'
);
CREATE INDEX idx_sessions_project ON sessions(project_id, started_at);
```

### 0002_providers_models.sql
```sql
CREATE TABLE providers (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    base_url    TEXT NOT NULL,
    api_format  TEXT NOT NULL DEFAULT 'openai',
    enabled     INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE models (
    id              TEXT PRIMARY KEY,
    provider_id     TEXT NOT NULL REFERENCES providers(id),
    external_name   TEXT NOT NULL,
    display_name    TEXT NOT NULL,
    class           TEXT NOT NULL DEFAULT 'balanced'
);
```

### 0003_pricebook.sql
```sql
CREATE TABLE pricebook_entries (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    model_id        TEXT NOT NULL REFERENCES models(id),
    input_per_mtok  REAL NOT NULL,
    output_per_mtok REAL NOT NULL,
    effective_from  TEXT NOT NULL,
    effective_until TEXT,
    source          TEXT NOT NULL DEFAULT 'local'
);
CREATE INDEX idx_pricebook_model ON pricebook_entries(model_id, effective_from);
```

### 0004_requests_usage.sql
```sql
CREATE TABLE requests (
    id              TEXT PRIMARY KEY,
    session_id      TEXT REFERENCES sessions(id),
    project_id      TEXT NOT NULL REFERENCES projects(id),
    model_requested TEXT NOT NULL,
    model_used      TEXT NOT NULL,
    provider_id     TEXT NOT NULL,
    started_at      TEXT NOT NULL,
    completed_at    TEXT,
    status          TEXT NOT NULL DEFAULT 'pending',
    is_streaming    INTEGER NOT NULL DEFAULT 0,
    upstream_ms     INTEGER
);

CREATE TABLE request_usage (
    request_id          TEXT PRIMARY KEY REFERENCES requests(id),
    input_tokens        INTEGER NOT NULL,
    output_tokens       INTEGER NOT NULL,
    cost_usd            REAL NOT NULL,
    pricing_snapshot    TEXT NOT NULL,
    source              TEXT NOT NULL DEFAULT 'provider'
);
CREATE INDEX idx_requests_project ON requests(project_id, started_at);
CREATE INDEX idx_requests_session ON requests(session_id);
```

### 0005_budgets.sql
```sql
CREATE TABLE budgets (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    scope_type      TEXT NOT NULL,
    scope_id        TEXT NOT NULL,
    window_type     TEXT NOT NULL,
    hard_limit_usd  REAL,
    soft_limit_usd  REAL,
    action_on_hard  TEXT NOT NULL DEFAULT 'block',
    action_on_soft  TEXT NOT NULL DEFAULT 'warn',
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    preset_source   TEXT
);
CREATE INDEX idx_budgets_scope ON budgets(scope_type, scope_id, window_type);
```

### 0006_cost_ledger.sql
```sql
CREATE TABLE cost_ledger (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id      TEXT NOT NULL,
    entry_type      TEXT NOT NULL,
    budget_id       INTEGER NOT NULL REFERENCES budgets(id),
    amount_usd      REAL NOT NULL,
    running_total   REAL NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_ledger_budget ON cost_ledger(budget_id, created_at);
CREATE INDEX idx_ledger_request ON cost_ledger(request_id);
```

### 0007_events.sql
```sql
CREATE TABLE events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id  TEXT,
    session_id  TEXT,
    event_type  TEXT NOT NULL,
    severity    TEXT NOT NULL DEFAULT 'info',
    detail      TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_events_type ON events(event_type, created_at);
CREATE INDEX idx_events_session ON events(session_id, created_at);
```

---

## Development Roadmap — Step by Step

Follow this order exactly. Each phase builds on the previous. Do not skip ahead.

### Phase 1 — Week 1: Foundation (types, config, store, cost)

**Goal**: Workspace compiles. Config loads. Database has schema. Pricing engine works.

#### Step 1.1: Scaffold workspace
- Create root `Cargo.toml` with `[workspace]` and all member crates
- Create `rust-toolchain.toml` pinning stable Rust
- Create every crate directory with minimal `Cargo.toml` and `lib.rs`
- Verify: `cargo check --workspace` passes (all crates are empty stubs)

#### Step 1.2: penny-types
- Define all types listed in the Module Specifications section above
- Use `#[derive(Debug, Clone, Serialize, Deserialize)]` on all types
- Define `PennyError` enum with `thiserror`
- NO business logic. Only type definitions and trivial constructors.
- Write tests: serialization round-trips for all key types

#### Step 1.3: penny-config
- Define `AppConfig` struct with all config sections
- Implement TOML loading with `toml::from_str`
- Implement env var override: iterate struct fields, check for `PENNY_` prefixed env vars
- Implement preset loading: read from `presets/` directory, merge into AppConfig
- Create `presets/indie.toml`, `presets/team.toml`, `presets/explore.toml`
- Create `config/default.toml` with full reference (all fields documented with comments)
- Implement validation: non-empty provider URL, budget limits > 0, valid enum values
- Write tests: load valid config, load with env override, load preset, reject invalid config

#### Step 1.4: penny-store
- Set up sqlx with SQLite
- Implement migration runner: reads `migrations/*.sql` files and applies them in order
- Set WAL mode on connection: `PRAGMA journal_mode=WAL;`
- Implement repository traits:
  - `ProjectRepo`: `upsert_by_path(path) → ProjectId`, `get_by_path(path) → Option<Project>`
  - `SessionRepo`: `create(project_id) → SessionId`, `find_active(project_id, window_minutes) → Option<SessionId>`, `close(session_id)`
  - `RequestRepo`: `insert(request) → ()`, `update_status(id, status)`, `insert_usage(usage)`
  - `BudgetRepo`: `list_applicable(scope_type, scope_id, window_type) → Vec<Budget>`, `upsert(budget)`, `list_all() → Vec<Budget>`
  - `LedgerRepo`: see penny-ledger section. This is the most critical repo.
  - `EventRepo`: `insert(event)`, `list(filters) → Vec<Event>`
  - `PricebookRepo`: `get_price(model_id, date) → Option<PricebookEntry>`, `import(entries)`
- Write tests: CRUD for each repo using in-memory SQLite (`:memory:`)

#### Step 1.5: penny-cost
- Implement `PricingEngine` struct that holds a reference to `PricebookRepo`
- `calculate(model_id, input_tokens, output_tokens)`: look up price, compute `(input * input_per_mtok / 1_000_000) + (output * output_per_mtok / 1_000_000)`
- `estimate_tokens(messages: &Value)`: use tiktoken-rs cl100k_base. Count tokens in all message content fields. Output estimate: `min(input * 0.3, 4096)`.
- `snapshot(model_id)`: return the pricebook entry as JSON for audit trail
- `estimate_range(model_id, context_tokens, task_type)`: SinglePass = 1 round, range ±30%. MultiRound = 3 rounds, range ±50%. AgentTask = 5 rounds, range ±100%.
- Create `prices/anthropic.toml` and `prices/openai.toml` with current pricing
- Write tests: pricing calculation, token estimation accuracy, range estimation

#### Step 1.6: CI
- Create `.github/workflows/ci.yml`
- Jobs: `cargo check`, `cargo test --workspace`, `cargo clippy -- -D warnings`, `cargo fmt -- --check`
- Run on push and PR

**Milestone M1**: `cargo test --workspace` passes. Config loads with preset. Pricebook resolves 6+ models. All repos pass CRUD tests.

---

### Phase 2 — Week 2: Proxy Pass-Through + Auto-Attribution

**Goal**: Request enters on :8585, exits to mock provider, response returns to client. Project and session auto-detected.

#### Step 2.1: penny-providers (mock only)
- Implement `MockProvider` that returns deterministic responses
- Response includes `usage` field with configurable token counts
- Support both streaming (SSE) and non-streaming modes
- Mock generates realistic-looking response bodies

#### Step 2.2: penny-proxy (basic)
- Set up axum server binding to `127.0.0.1:8585`
- Implement `POST /v1/chat/completions` handler
- Middleware: request_id (UUIDv7 → `X-Penny-Request-Id` header)
- Normalize incoming request: extract model name, messages, stream flag
- Map model name → provider via config
- Estimate tokens using penny-cost
- Dispatch to mock provider
- Forward response to client
- Persist request + usage in SQLite via penny-store
- Implement `GET /v1/models` returning configured models

#### Step 2.3: Auto-attribution
- **Project detection**: 
  1. Check `X-Penny-Project` header (explicit override)
  2. Check `X-Penny-Cwd` header → find git root → derive project slug
  3. Use cwd of the PennyPrompt process itself → find git root
  4. Fallback to `"default"`
  - Git root detection: walk up from cwd looking for `.git` directory
  - Project slug: last component of git root path, lowercased, sanitized
  - Upsert project in DB on first sight
- **Session detection**:
  1. Check `X-Penny-Session` header (explicit override)
  2. Find active session for this project within `session_window_minutes` (default 30)
  3. If none found, create new session
- Add project_id and session_id to the normalized request

#### Step 2.4: Health endpoint
- `GET /internal/health` (temporary on proxy port; will move to admin plane in week 4)
- Returns: uptime, DB connection status, configured providers

**Milestone M2**: `curl -X POST localhost:8585/v1/chat/completions -d '{"model":"claude-sonnet-4-6","messages":[{"role":"user","content":"hello"}]}'` returns mock response. Row exists in `requests`, `request_usage`, `sessions`, `projects`.

---

### Phase 3 — Week 3: Atomic Budget Enforcement

**Goal**: Cost ledger works. Requests blocked in guard mode when budget exceeded. Concurrent requests don't break limits.

#### Step 3.1: penny-ledger
- Implement `CostLedger` struct
- `reserve(request_id, budgets, estimated_cost) → Reservation`:
  ```
  BEGIN IMMEDIATE;
  FOR each budget:
    SELECT running_total FROM cost_ledger WHERE budget_id = ? ORDER BY id DESC LIMIT 1;
    new_total = running_total + estimated_cost;
    IF new_total > budget.hard_limit_usd:
      ROLLBACK;
      RETURN Denied { budget, running_total, hard_limit };
    INSERT INTO cost_ledger (request_id, entry_type='reserve', budget_id, amount_usd=estimated_cost, running_total=new_total);
  COMMIT;
  RETURN Granted { entries, remaining_by_budget };
  ```
- `reconcile(request_id, actual_cost)`:
  ```
  FOR each reserve entry for this request_id:
    diff = actual_cost - reserved_cost;
    SELECT latest running_total for budget;
    new_total = running_total + diff;
    INSERT INTO cost_ledger (request_id, entry_type='reconcile', budget_id, amount_usd=diff, running_total=new_total);
  ```
- `release(request_id)`:
  ```
  FOR each reserve entry for this request_id:
    SELECT latest running_total for budget;
    new_total = running_total - reserved_cost;
    INSERT INTO cost_ledger (request_id, entry_type='release', budget_id, amount_usd=-reserved_cost, running_total=new_total);
  ```

**CRITICAL**: Use `BEGIN IMMEDIATE` for the reserve transaction. This acquires a RESERVED lock immediately, serializing concurrent reserves.

#### Step 3.2: penny-budget
- Implement `BudgetEvaluator`
- `evaluate(request, estimated_cost) → RouteDecision`:
  1. Load applicable budgets from store
  2. Check budget window: is the current time within the budget's window? Compute window start (day=midnight, week=Monday, month=1st)
  3. Call `ledger.reserve(...)`
  4. Apply mode logic (observe/guard) and failsafe behavior
  5. Check soft limits → add warnings
  6. Record events for all decisions

#### Step 3.3: Integrate into proxy pipeline
- After normalization, before dispatch: call `budget.evaluate(...)`
- If `Block` → return HTTP 402 with structured JSON body:
  ```json
  {
    "error": {
      "type": "budget_exceeded",
      "retryable": false,
      "message": "Budget 'global / day' exceeded: $10.23 of $10.00 limit",
      "budget": { "scope": "global:*", "window": "day", "accumulated_usd": 10.23, "limit_usd": 10.0, "resets_at": "2026-04-10T00:00:00Z" },
      "suggestion": "pennyprompt budget set global:* day 15"
    }
  }
  ```
- If `Failsafe` with mode=guard → return same 402 with `"type": "budget_engine_failure"`
- After response: call `ledger.reconcile(...)` with actual cost
- On dispatch failure: call `ledger.release(...)`

#### Step 3.4: Budget seeding from config
- On startup: read `[[budgets]]` from config → upsert into DB
- Preset budgets are tagged with `preset_source` field

#### Step 3.5: CLI report
- `pennyprompt report summary --since 7d`: query requests + usage, aggregate by model and project
- Use comfy-table for output formatting

**Milestone M3**: Budget of $1.00 configured. Send requests until exceeded. Request N+1 returns 402. Concurrent test (3 simultaneous requests, $0.50 each, $1.00 budget) → only 2 pass. `report summary` shows correct totals.

---

### Phase 4 — Week 4: Streaming + Real Providers + Admin Plane

**Goal**: Works with real Anthropic and OpenAI APIs. Streaming works. Admin plane separated.

#### Step 4.1: Anthropic adapter
- Translate NormalizedRequest to Anthropic Messages API format:
  - URL: `{base_url}/v1/messages`
  - Headers: `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json`
  - Body: `{ model, messages, max_tokens, stream }`
  - Message format translation: OpenAI `{"role","content"}` → Anthropic `{"role","content"}` (similar but content blocks differ)
- Handle response: extract `usage.input_tokens`, `usage.output_tokens` from response body
- Handle streaming: Anthropic uses `event: message_start`, `event: content_block_delta`, `event: message_delta` (contains usage), `event: message_stop`

#### Step 4.2: OpenAI adapter
- Most direct pass-through
- URL: `{base_url}/v1/chat/completions`
- Headers: `Authorization: Bearer {key}`, `Content-Type: application/json`
- Handle streaming: `data: {"choices":[{"delta":{"content":"..."}}]}` chunks. Usage in final chunk (or missing — fallback to estimation)

#### Step 4.3: Streaming support in proxy
- For streaming requests:
  1. Start forwarding SSE chunks to client immediately (low latency)
  2. In background: accumulate all chunks, extract content
  3. On stream end (`data: [DONE]`):
     - Extract usage from final chunk if available
     - Otherwise: count tokens of accumulated content with tiktoken-rs
     - Reconcile ledger with actual cost
- Use `tokio::sync::mpsc` channel: adapter sends chunks, proxy handler receives and forwards
- Handle stream interruption: reconcile with partial estimate + `status: 'incomplete'`

#### Step 4.4: Admin plane
- Set up separate axum server on unix socket
- Move health endpoint from proxy to admin
- Implement report endpoints: `/admin/report/summary`, `/admin/report/session/:id`, `/admin/report/top`
- Implement budget endpoints: `GET /admin/budgets`, `POST /admin/budgets`
- Implement SSE events endpoint: `GET /admin/events`

#### Step 4.5: Error handling
- Provider timeout → 504 to client + event
- Provider 5xx → propagate status + event
- Provider 429 (rate limit) → propagate 429 to client (this is the provider's rate limit, not ours)
- Parse error in response → 502 + event

**Milestone M4 (Alpha Candidate)**: Point OpenClaw at localhost:8585. Complete 10 real tasks. Report shows correct costs. Streaming works cleanly. Admin endpoints return valid data.

---

### Phase 5 — Week 5: Loop Detection + Route Preview + Burn-Rate

**Goal**: The three active protection features that solve the most acute pain.

#### Step 5.1: penny-detect
- Implement `LoopDetector` struct with `HashMap<SessionId, SessionWindow>` behind `RwLock`
- `SessionWindow`: `VecDeque<RequestDigest>` with max window size (time-based eviction)
- `feed(digest) → Option<DetectAlert>`: run three heuristic checks after every request
- `is_session_paused(session_id) → bool`: check paused set
- `resume_session(session_id)`: remove from paused set, record event
- Content hash: `sha2::Sha256` of first 500 chars of first user message content, truncated to u64

#### Step 5.2: Integrate detection into proxy
- After `ledger.reconcile(...)`: create `RequestDigest`, call `detector.feed(...)`
- If alert returned: record event in DB, emit tracing event
- If pause action: add session to paused set
- Before budget check: if session is paused → return 402 with `"type": "session_paused_loop_detected"`

#### Step 5.3: Burn-rate
- Part of `penny-detect`: calculate `window_total_cost / elapsed_hours`
- Compare against `detect.burn_rate_alert_usd_per_hour` from config
- Alert includes: current rate, threshold, session_id, window duration

#### Step 5.4: Route preview / estimation
- CLI: `pennyprompt estimate --model sonnet --context-files src/auth/*.rs`
  - Glob files, count tokens, estimate cost range per configured model
  - Show budget status after estimated spend
- Admin API: `POST /admin/estimate` with `{ model, context_tokens, estimated_rounds, task_type }`
- Return: `{ estimates: [{ model, min_usd, max_usd, confidence, budget_status }] }`

#### Step 5.5: pennyprompt tail
- CLI command that connects to admin SSE endpoint (`/admin/events`)
- Formats events in real-time with colors:
  - Requests: `→ model  in/out  $cost  project/session`
  - Warnings: `⚠ BURN-RATE $X/hr (threshold: $Y/hr)`
  - Blocks: `⛔ BUDGET BLOCK scope: $used/$limit`
  - Loops: `🔄 LOOP DETECTED tool:X failed N times in Ys`

#### Step 5.6: detect CLI
- `pennyprompt detect status`: list active alerts and paused sessions
- `pennyprompt detect resume <session_id>`: resume + record event

**Milestone M5**: Simulate loop (script sending 15 identical requests in 30s). PennyPrompt detects and pauses. `tail` shows alerts. `detect resume` works. `estimate` shows ranges with budget status.

---

### Phase 6 — Week 6: Polish + Alpha Release

**Goal**: Publishable binary. Documented. Tested. Usable by new user in <10 minutes.

#### Step 6.1: CLI polish
- `pennyprompt init --preset indie`: interactive wizard, generates config, detects API keys in env
- `pennyprompt doctor`: config, DB, providers (ping test), pricebook age, budgets, mode, ports
- `pennyprompt config`: show effective config with env overrides resolved
- `pennyprompt prices show`: formatted table of current pricebook
- `pennyprompt prices update`: download latest from GitHub repo
- `pennyprompt budget list`: all budgets with current accumulated/limit/%
- `pennyprompt budget set`: create or update
- `pennyprompt budget reset`: reset window accumulation
- `pennyprompt report top --limit 5`: most expensive requests

#### Step 6.2: Documentation
- `docs/INSTALL.md`: Linux (binary, cargo install), macOS (binary, cargo install, homebrew later)
- `docs/QUICKSTART.md`: 0 to report in 5 minutes, step by step
- `docs/CONFIG-REFERENCE.md`: every field, every preset, every env var, every default
- `docs/ARCHITECTURE.md`: crate map, data flow, contributor guide
- `docs/PRICEBOOK.md`: how it works, how to update, how to add custom models
- `README.md`: the marketing-grade README (already created)

#### Step 6.3: Testing
- Run full integration test suite
- Run golden tests for all CLI outputs
- Manual acceptance: follow the 12-step checklist from v2 spec
- Test with real OpenClaw session (10+ tasks)
- Test with real claw-code session
- Verify streaming with both Anthropic and OpenAI

#### Step 6.4: Release build
- Cross-compile: `cargo build --release --target x86_64-unknown-linux-gnu`
- Targets: linux-x86_64, linux-aarch64, macos-x86_64, macos-aarch64
- Create `scripts/install.sh` that detects platform and downloads correct binary
- Create GitHub Release with binaries + checksums
- Write CHANGELOG.md

**Milestone M6 (Alpha Release)**: GitHub repo public. Binary downloadable. New user follows QUICKSTART.md → sees first report in <10 minutes.

---

## Coding Standards

### Error Handling
- Use `anyhow::Result` for application-level functions
- Use `thiserror` for library-level error types in each crate
- Never use `.unwrap()` in production code. Use `.expect("reason")` only for truly impossible states, with a clear message explaining why.
- All errors must be actionable: the user should know what to do when they see one

### Testing
- Unit tests go in the same file as the implementation (`#[cfg(test)] mod tests`)
- Integration tests go in `tests/integration/`
- Golden tests: compare CLI output against `tests/golden/*.txt` files
- Use `sqlx::test` attribute for database tests (automatic in-memory DB)
- Aim for >80% coverage on core crates (penny-ledger, penny-budget, penny-cost, penny-detect)

### Formatting
- `cargo fmt` on every commit
- `cargo clippy -- -D warnings` must pass
- No `#[allow(clippy::...)]` without a comment explaining why

### Git
- Conventional commits: `feat:`, `fix:`, `test:`, `docs:`, `refactor:`, `chore:`
- One logical change per commit
- Feature branches off `main`

### Logging
- Use `tracing` macros: `tracing::info!()`, `tracing::warn!()`, `tracing::error!()`
- Always include structured fields: `request_id`, `session_id`, `model`, `cost_usd` where applicable
- Never log API keys or full request bodies (PII risk)
- Log at `info` level: request start/end, budget decisions, alerts
- Log at `debug` level: token estimation details, pricebook lookups
- Log at `warn` level: soft limit exceeded, estimation fallback, pricebook age warning
- Log at `error` level: provider errors, budget engine failures, DB errors

### Performance Targets
- Proxy overhead p50: < 5ms
- Proxy overhead p95: < 15ms
- Proxy overhead p99: < 50ms
- Budget check (including SQLite tx): < 2ms
- Token estimation: < 1ms
- Loop detection check: < 0.5ms

---

## Key Implementation Warnings

### SQLite Concurrency
- ALWAYS use WAL mode: `PRAGMA journal_mode=WAL;`
- ALWAYS use `BEGIN IMMEDIATE` for reserve transactions (prevents SQLITE_BUSY on concurrent writes)
- Keep transactions SHORT — read running_total, compare, insert, commit. No network I/O inside transactions.
- Use a single `SqlitePool` shared across the application. sqlx handles connection pooling.

### Streaming
- Forward SSE chunks to client AS SOON AS received. Do not buffer the entire response.
- Accumulate content in background for token counting ONLY.
- If the stream is interrupted (client disconnects, timeout): reconcile with partial estimate, mark request as `incomplete`.
- Anthropic streaming format differs from OpenAI. Each adapter MUST handle its own SSE format.

### Budget Window Calculation
- `day` window: midnight UTC of current day
- `week` window: Monday midnight UTC of current week
- `month` window: 1st midnight UTC of current month
- To calculate accumulated spend for a window: `SELECT COALESCE(SUM(amount_usd), 0.0) FROM cost_ledger WHERE budget_id = ? AND entry_type IN ('reserve', 'reconcile') AND created_at >= ?`
- Actually, prefer using `running_total` from the latest ledger entry for O(1) lookup instead of SUM.

### Auto-Attribution Edge Cases
- If the cwd has no `.git` directory (and no parent does either): use project `"default"`
- If two different repos have the same directory name: include parent directory too → `parent-child`
- Session window: if a request arrives 31 minutes after the last one (with 30min window), it starts a new session. The old session is NOT closed — it's closed lazily when queried or on next startup.

---

## Files to Never Modify Carelessly

- `migrations/*.sql` — Schema changes must be additive (new tables, new columns with defaults). Never rename or remove columns in existing migrations.
- `prices/*.toml` — These are the source of truth for cost calculations. Changes affect all future cost reports.
- `presets/*.toml` — These define the UX for new users. Changes affect first impressions.
- `penny-types/src/lib.rs` — Type changes cascade through every crate. Be deliberate.
- `penny-ledger` — This is the financial core. Any bug here means incorrect budget enforcement. Test exhaustively.

---

## Quick Reference: Running During Development

```bash
# Build everything
cargo build --workspace

# Run all tests
cargo test --workspace

# Run with mock provider (no real API calls)
cargo run -- serve --mock

# Run with real providers (needs API keys in env)
ANTHROPIC_API_KEY=sk-ant-... cargo run -- serve

# Test a request
curl -s http://localhost:8585/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"claude-sonnet-4-6","messages":[{"role":"user","content":"say hi"}]}' | jq .

# Check cost report
cargo run -- report summary --since 1h

# Run specific crate tests
cargo test -p penny-ledger
cargo test -p penny-budget
cargo test -p penny-detect

# Clippy
cargo clippy --workspace -- -D warnings

# Format
cargo fmt --all
```
