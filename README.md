<p align="center">
  <h1 align="center">PennyPrompt</h1>
  <p align="center"><strong>Cost guardrails for AI agents.</strong></p>
  <p align="center">
    Estimates before you spend. Protects while you spend. Explains after you spend.
  </p>
</p>

<p align="center">
  <a href="#quickstart">Quickstart</a> •
  <a href="#why">Why</a> •
  <a href="#features">Features</a> •
  <a href="#how-it-works">How It Works</a> •
  <a href="#configuration">Configuration</a> •
  <a href="#cli-reference">CLI</a> •
  <a href="#architecture">Architecture</a> •
  <a href="#contributing">Contributing</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/language-Rust-orange?style=flat-square" />
  <img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" />
  <img src="https://img.shields.io/badge/status-alpha-yellow?style=flat-square" />
</p>



---

Your AI agent just burned $47 debugging a typo. You didn't know until Tuesday.

**PennyPrompt is a local reverse proxy (~15MB, single binary, zero dependencies) that sits between your AI agent and your LLM provider.** It estimates costs before execution, enforces budgets with hard stops, detects runaway loops, and generates forensic reports — all without changing a single line in your existing workflow.

```
Your Agent ──→ PennyPrompt (:8585) ──→ Anthropic / OpenAI
                    │
                    ├── Budget check (atomic, with reservations)
                    ├── Loop detection (burn-rate, repeated failures)
                    ├── Cost accounting (per request, session, project)
                    └── SQLite (local, append-only ledger)
```

## Quickstart

```bash
# Install
curl -sSL https://get.pennyprompt.dev | sh

# Initialize with a preset
pennyprompt init --preset indie

# Point your agent to PennyPrompt
export ANTHROPIC_BASE_URL=http://localhost:8585/v1

# Start the proxy
pennyprompt serve

# That's it. Your agent works the same. You control the spend.
```

Your first cost report in under 10 minutes:

```bash
$ pennyprompt report summary --since 1d

  Total:       $4.23  (67 requests)
  Burn-rate:   $2.80/hr active
  Budget:      $4.23 / $10.00 day  ████░░░░░░ 42.3%

  By Model:
    claude-sonnet-4-6    $3.41  (80.7%)  48 reqs
    gpt-4.1              $0.82  (19.3%)  19 reqs

  By Project:
    webapp               $3.89  (92.0%)
    experiments          $0.34  ( 8.0%)
```

## Why

On April 4, 2026, Anthropic cut the flat-rate buffet for 135,000+ OpenClaw instances. A $20/month subscription that enabled ~$236 in real token usage became pay-per-token overnight. But the real problem isn't the price change — it's that **nobody knows what their agents actually cost until the bill arrives.**

A typical 4-hour coding session with an autonomous agent generates 15-40 model calls per task, each with 100K+ token context windows, silent retries, and memory compaction that triggers even more calls. The result: invisible, unpredictable spend.

PennyPrompt exists because:

- **You can't optimize what you can't see.** Provider dashboards show aggregate totals. PennyPrompt shows cost per project, per session, per request.
- **Alerts after the fact don't help.** PennyPrompt blocks the request *before* it breaks your budget. Atomically. With reservations that prevent concurrent requests from slipping through.
- **Agents loop.** A debugging agent retrying the same failed tool call 30 times in 2 minutes will burn $50+ before you notice. PennyPrompt detects it and pauses.
- **"How much will this cost?" has no answer today.** PennyPrompt estimates before you start, so you can decide if a task is worth the spend.

### What PennyPrompt is NOT

- **Not a router.** It doesn't pick models for you. Use [NadirClaw](https://github.com/NadirRouter/NadirClaw) for that — they compose well together.
- **Not an enterprise gateway.** No RBAC, no Kubernetes, no YAML labyrinth. Use LiteLLM or Kong for that.
- **Not a SaaS.** Your traffic never leaves your machine (except to the LLM provider you configured).
- **Not magic.** It doesn't rewrite your prompts or make decisions you can't audit.

## Features

### Budget Enforcement (Atomic)

PennyPrompt uses a **cost ledger with transactional reservations**. Before dispatching a request, it reserves the estimated cost in a SQLite transaction. After the response, it reconciles against the actual cost. This prevents concurrent requests from breaking your budget:

```
Request A: RESERVE $4 est. → ($45+$4=$49 of $50, OK) → dispatch → RECONCILE $3.80
Request B: RESERVE $3 est. → ($49+$3=$52 of $50, BLOCKED) → HTTP 402
```

Budgets are enforced with **HTTP 402 (Payment Required)**, not 429. Agents auto-retry 429 (rate limit). 402 with `"retryable": false` tells the agent to stop and ask the human.

### Two Operating Modes

| Mode | Behavior | DB/Ledger Failure |
|------|----------|-------------------|
| `observe` | Logs everything, never blocks | Request passes + `mode_failsafe` event |
| `guard` | Logs + blocks on hard limit | Request blocked (fail-closed) |

Start with `observe` to understand your costs. Switch to `guard` when you're ready for protection.

### Runaway Loop Detection

PennyPrompt monitors each session for three patterns:

- **Repeated tool failures** — same tool, same error, N times in M seconds
- **Abnormal burn-rate** — "$14.20/hr (threshold: $10/hr)"
- **Request similarity** — N requests with similar content/tokens in a time window

Configurable actions: `alert` (log + show in `tail`) or `pause` (block session until `pennyprompt detect resume`).

### Pre-Execution Cost Estimation

```bash
$ pennyprompt estimate --model claude-sonnet-4-6 --context-files src/auth/

  claude-sonnet-4-6:  $0.12 – $0.45 (single pass) | $0.60 – $2.25 (agent task)
  claude-opus-4-1:    $0.35 – $1.25 (single pass) | $1.75 – $6.25 (agent task)
  claude-haiku-4:     $0.04 – $0.15 (single pass) | $0.20 – $0.75 (agent task)

  Budget: OK (day: $6.59 remaining of $10.00)
```

### Auto-Attribution

No custom headers needed. PennyPrompt auto-detects:

- **Project** — from the git root of the current working directory
- **Session** — groups requests within a configurable time window (default: 30min)

Your reports are useful from the first request, zero configuration.

### Real-Time Monitoring

```bash
$ pennyprompt tail

  [14:23:01] → sonnet  4,231 in / 892 out   $0.018  webapp/sess_x1
  [14:23:04] → sonnet  6,102 in / 1,203 out  $0.031  webapp/sess_x1
  [14:23:06] ⚠ BURN-RATE $14.20/hr (threshold: $10/hr) sess_x1
  [14:23:09] ⛔ BUDGET BLOCK  global/day: $10.12/$10.00  sess_x1
  [14:23:09] ← 402 budget_exceeded → client
```

## How It Works

```
Request arrives (:8585)
  │
  ├─ Generate request_id (UUIDv7)
  ├─ Auto-detect project (git root) + session (time window)
  ├─ Map model → provider
  ├─ Estimate tokens (tiktoken-rs) + cost (pricebook)
  │
  ├─ RESERVE estimated cost in ledger (SQLite transaction)
  │   ├─ accumulated + estimated > hard_limit? → 402 (block)
  │   └─ OK → reservation created
  │
  ├─ DISPATCH to provider (Anthropic / OpenAI)
  │   ├─ Non-streaming: read full response
  │   └─ Streaming: forward SSE chunks, accumulate in background
  │
  ├─ RECONCILE actual cost in ledger
  │   ├─ Usage from provider response (preferred)
  │   └─ Fallback: tiktoken-rs estimation
  │
  └─ DETECT loop/burn-rate patterns → alert or pause

  Overhead: ~2-5ms (excluding upstream I/O)
```

## Configuration

### Presets (Zero-Friction Start)

```bash
pennyprompt init --preset indie     # $30/mo, $10/day, guard mode, burn-rate alerts
pennyprompt init --preset team      # $100/mo, $20/day, guard mode, loop detection
pennyprompt init --preset explore   # $10/mo soft limit only, observe mode
```

### Full Configuration

```toml
# ~/.config/pennyprompt/config.toml

[server]
bind = "127.0.0.1:8585"               # Proxy plane
admin_socket = "~/.local/share/pennyprompt/admin.sock"
database_path = "~/.local/share/pennyprompt/penny.db"
mode = "guard"                          # observe | guard

[defaults]
provider = "anthropic"
model = "claude-sonnet-4-6"

[attribution]
auto_detect_project = true              # Uses git root
session_window_minutes = 30

[providers.anthropic]
enabled = true
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
api_format = "anthropic"

[providers.openai]
enabled = true
base_url = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
api_format = "openai"

[[budgets]]
scope_type = "global"
scope_id = "*"
window_type = "month"
hard_limit_usd = 30.0
soft_limit_usd = 20.0

[[budgets]]
scope_type = "global"
scope_id = "*"
window_type = "day"
hard_limit_usd = 10.0

[detect]
enabled = true
burn_rate_alert_usd_per_hour = 10.0
loop_window_seconds = 120
loop_threshold_similar_requests = 8
loop_action = "pause"                   # alert | pause
```

All config values can be overridden via environment variables with the `PENNY_` prefix:

```bash
PENNY_SERVER_BIND=0.0.0.0:8585
PENNY_DEFAULTS_MODEL=claude-opus-4-1
```

`pennyprompt serve` bind behavior:

- Proxy plane binds to `server.bind` (default `127.0.0.1:8585`).
- Admin plane reads `server.admin_socket`.
- If `admin_socket` is `host:port`, admin binds TCP.
- Otherwise admin binds a Unix socket path (supports `~` expansion).
- You can override at runtime with `--proxy-bind` and `--admin-bind`.

### Pricebook

Pricing is stored locally in versioned TOML files (`prices/anthropic.toml`, `prices/openai.toml`). No scraping. No external API calls. Works offline.

```bash
pennyprompt prices show            # Current prices
pennyprompt prices update          # Import bundled local pricebook files
```

## CLI Reference

```
pennyprompt
├── init [--preset indie|team|explore]     Setup wizard
├── serve [--mock] [--proxy-bind] [--admin-bind]  Start proxy + admin
├── estimate [--model M] [--context-files]  Pre-execution cost estimate
├── run <agent> [--json]                    Launcher dry-run plan
├── report
│   ├── summary [--since] [--by project|model|session]
│   └── top [--limit N]                     Most expensive requests
├── budget
│   ├── list                                Active budgets + status
│   ├── set <scope> <window> <limit>        Create/update budget
│   └── reset <scope> <window>              Reset a budget window
├── detect
│   ├── status                              Active alerts, paused sessions
│   └── resume <session_id>                 Resume paused session
├── tail                                    Live request + cost stream
├── doctor                                  System diagnostics
├── prices
│   ├── show                                Current pricebook
│   └── update                              Import bundled local pricebook files
├── config                                  Show effective config
└── dashboard [--since] [--limit]           Snapshot KPIs by project/model
```

## Architecture

### Crate Structure

```
crates/
├── penny-types/          Shared types, enums, error types
├── penny-config/         TOML loader, validation, presets, env overrides
├── penny-store/          SQLite repositories (sqlx, WAL mode)
├── penny-cost/           Pricing engine, token estimation (tiktoken-rs)
├── penny-ledger/         Cost ledger: reserve / reconcile / release
├── penny-budget/         Budget evaluation, enforcement, observe/guard modes
├── penny-detect/         Runaway loop detection, burn-rate monitoring
├── penny-providers/      Provider adapters (Anthropic, OpenAI, mock)
├── penny-proxy/          Axum HTTP proxy, middleware pipeline
├── penny-admin/          Admin plane: reports, budgets, health, events SSE
├── penny-cli/            Clap CLI: all subcommands
└── penny-observe/        Tracing, structured logging
```

### Tech Stack

| Layer | Choice | Why |
|-------|--------|-----|
| Runtime | tokio | Async standard. Required by axum, reqwest, sqlx. |
| HTTP Server | axum 0.8+ | Composable extractors, native tokio, type-safe. |
| HTTP Client | reqwest | TLS, streaming, connection pooling. |
| Database | SQLite (sqlx, WAL) | Local-first. Zero external deps. Atomic transactions. |
| Config | TOML (serde) | Strict typing. No YAML ambiguity. |
| CLI | clap 4 (derive) | Ergonomic. Completion generation. |
| Token counting | tiktoken-rs | OpenAI-compatible tokenizer. Fast path. |
| IDs | UUIDv7 | Temporally sortable. Better SQLite range queries. |
| Logging | tracing | Structured, filterable, span-aware. |

### Data Model

```
projects ──1:N──→ sessions ──1:N──→ requests ──1:1──→ request_usage
    │                                    │
    │                                    └──1:N──→ events
    └──1:N──→ budgets

cost_ledger (append-only): reserve → reconcile → release entries
pricebook_entries: versioned, with effective_from/effective_until
```

### Proxy Plane vs Admin Plane

| Plane | Bind | Purpose |
|-------|------|---------|
| Proxy | `127.0.0.1:8585` | Agent traffic. OpenAI-compatible API. |
| Admin | Unix socket (default) or `:8586` with token | Reports, budgets, health, events. |

Separated by design. Exposing `:8585` to the network never exposes admin endpoints.

## Compatibility

PennyPrompt works with any tool that speaks the OpenAI chat completions API:

| Agent | How to connect |
|-------|---------------|
| OpenClaw | `ANTHROPIC_BASE_URL=http://localhost:8585/v1` |
| claw-code | `ANTHROPIC_BASE_URL=http://localhost:8585/v1` |
| Cursor | Settings → Models → Base URL → `http://localhost:8585/v1` |
| Codex | `OPENAI_BASE_URL=http://localhost:8585/v1` |
| Continue | `config.json` → apiBase → `http://localhost:8585/v1` |
| NadirClaw | Chain: Agent → NadirClaw → PennyPrompt → Provider |
| curl | `curl http://localhost:8585/v1/chat/completions ...` |

## Roadmap

- [x] Project specification (v2)
- [ ] **Alpha** (6 weeks) — Proxy, atomic budgets, loop detection, estimation, reports, presets
- [ ] **Post-alpha** — Launcher (`pennyprompt run <agent>`), TUI dashboard, payload cleanup, webhooks
- [ ] **v1.0** — Team mode, PostgreSQL backend, plugin system, Grafana/Prometheus metrics

See [PennyPrompt-v2.md](PennyPrompt-v2.md) for the full project specification.

## Contributing

PennyPrompt is MIT licensed. Contributions welcome.

```bash
# Clone and build
git clone https://github.com/your-user/pennyprompt.git
cd pennyprompt
cargo build

# Run tests
cargo test --workspace

# Run with mock provider
cargo run -- serve --mock
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the crate dependency map.
For the day-to-day issue workflow used in this repo, see [docs/DEVELOPMENT_WORKFLOW.md](docs/DEVELOPMENT_WORKFLOW.md).

Alpha documentation set:

- [docs/INSTALL.md](docs/INSTALL.md)
- [docs/QUICKSTART.md](docs/QUICKSTART.md)
- [docs/CONFIG-REFERENCE.md](docs/CONFIG-REFERENCE.md)
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- [docs/PRICEBOOK.md](docs/PRICEBOOK.md)
- [docs/LIMITATIONS.md](docs/LIMITATIONS.md)
- [docs/RELEASE.md](docs/RELEASE.md)
- [CHANGELOG.md](CHANGELOG.md)

## License

MIT — see [LICENSE](LICENSE).

---

<p align="center">
  <strong>Every token has a price. Now you know it before you pay.</strong>
</p>
