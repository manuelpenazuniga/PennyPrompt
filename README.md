<div align="center">

# 🪙 PennyPrompt

### Cost guardrails for AI agents

**Estimates before you spend. Protects while you spend. Explains after you spend.**

[![CI](https://github.com/manuelpenazuniga/PennyPrompt/actions/workflows/ci.yml/badge.svg)](https://github.com/manuelpenazuniga/PennyPrompt/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/manuelpenazuniga/PennyPrompt?include_prereleases&label=release)](https://github.com/manuelpenazuniga/PennyPrompt/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/built_with-Rust-orange.svg)](https://www.rust-lang.org/)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](docs/DEVELOPMENT_WORKFLOW.md)

[Quickstart](#-quickstart) •
[Why](#-why) •
[How It Compares](#%EF%B8%8F-how-it-compares) •
[Features](#-features) •
[How It Works](#%EF%B8%8F-how-it-works) •
[Configuration](#%EF%B8%8F-configuration) •
[Roadmap](#%EF%B8%8F-roadmap) •
[Contributing](#-contributing)

</div>

---

Your AI agent just burned **$47 debugging a typo**. You didn't find out until Tuesday.

Autonomous agents are loops with a credit card: 15–40 model calls per task, 100K+ token contexts, silent retries, memory compaction that triggers even more calls. Provider dashboards show you the damage *after* it's done.

**PennyPrompt is a local reverse proxy — one ~15MB binary, zero external services — that sits between your agent and your LLM provider** and gives you the three answers nobody else gives you *when they matter*:

- **Before:** *"How much will this task cost?"* → pre-execution estimation
- **During:** *"Is this getting out of hand right now?"* → atomic budget stops + runaway-loop detection
- **After:** *"Where exactly did the money go?"* → per-request, per-session, per-project forensics

```
Your Agent ──→ PennyPrompt (:8585) ──→ Anthropic / OpenAI
                    │
                    ├── Budget check   (atomic reservation — blocks BEFORE overspend)
                    ├── Loop detection (burn-rate, repeated failures, similarity)
                    ├── Cost accounting (per request / session / project)
                    └── SQLite         (local, append-only ledger — nothing leaves your machine)
```

No cloud. No PostgreSQL. No Redis. No YAML labyrinth. No telemetry.

## 🚀 Quickstart

```bash
# 1. Install (macOS / Linux, x86_64 & arm64)
curl -fsSL https://raw.githubusercontent.com/manuelpenazuniga/PennyPrompt/main/scripts/install.sh | sh

# 2. Initialize with a preset ($30/mo, $10/day hard stop, guard mode)
penny-cli init --preset indie

# 3. Import the local pricebook and check your setup
penny-cli prices update
penny-cli doctor

# 4. Start the proxy
penny-cli serve

# 5. Point your agent at it (OpenAI-compatible clients)
export OPENAI_BASE_URL=http://localhost:8585/v1
```

Your agent works exactly the same. You control the spend. First report in minutes:

```
$ penny-cli report summary --since 1d

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

> Full walkthrough: [docs/QUICKSTART.md](docs/QUICKSTART.md) · Install options: [docs/INSTALL.md](docs/INSTALL.md)

## 💡 Why

On April 4, 2026, flat-rate pricing ended for 135,000+ OpenClaw instances overnight. A $20/month subscription that quietly covered ~$236 of real token usage became pay-per-token. But the price change isn't the real problem — the real problem is that **nobody knows what their agents cost until the invoice arrives.**

PennyPrompt exists because:

- **You can't optimize what you can't see.** Provider dashboards show aggregate totals, delayed. PennyPrompt shows cost per project, per session, per request — live.
- **Alerts after the fact don't help.** PennyPrompt blocks the request *before* it breaks your budget. Atomically, with reservations that concurrent requests can't slip past.
- **Agents loop.** A debugging agent retrying the same failed tool call 30 times in 2 minutes burns $50+ before you notice. PennyPrompt detects the loop and pauses the session.
- **"How much will this cost?" has no answer today.** PennyPrompt estimates before you start, so you decide whether a task is worth the spend.

## ⚖️ How It Compares

The LLM-gateway category models *applications with users* — virtual keys, per-key limits, spend tracking after the call. PennyPrompt models something else: **an autonomous agent, which is a loop with a credit card** — and it runs local-first.

| | **PennyPrompt** | LiteLLM | Portkey | Helicone | OpenRouter |
|---|---|---|---|---|---|
| **Setup** | 1 binary, ~15MB | Python + PostgreSQL + Redis | Gateway + platform | Self-host or SaaS | SaaS only |
| **Budget enforcement** | **Atomic, *before* dispatch** | Per-key limits, post-hoc tracking | Budgets + guardrails | Tracking + rate limits | — (5.5% fee) |
| **Concurrency-safe hard stops** | ✅ Transactional reservation | ❌ | ❌ | ❌ | ❌ |
| **Agent-loop detection** | ✅ Burn-rate, tool failures, similarity | ❌ | ❌ | ❌ | ❌ |
| **Agent-friendly block semantics** | ✅ HTTP 402 `retryable:false` | 429-style | 429-style | — | — |
| **Zero-config attribution** | ✅ Git root + time window | Virtual keys | Metadata/tags | Custom headers | — |
| **Pre-execution estimation** | ✅ | ❌ | ❌ | ❌ | ❌ |
| **Data leaves your machine** | **Only the provider call** | Depends on deploy | Depends on tier | Depends on tier | Always |
| **Telemetry** | **None** | — | — | — | — |

*Different tools for different jobs — LiteLLM and Portkey are excellent gateways for teams running app traffic. PennyPrompt is the guardrail for the developer whose "user" is an autonomous agent on their own machine. Full competitive analysis: [docs/STRATEGY-AUDIT-2026-07-05.md](docs/STRATEGY-AUDIT-2026-07-05.md).*

### What PennyPrompt is NOT

- **Not a router.** It doesn't pick models for you. Use [NadirClaw](https://github.com/NadirRouter/NadirClaw) for that — they compose: `Agent → NadirClaw → PennyPrompt → Provider`.
- **Not an enterprise gateway.** No RBAC, no Kubernetes. Use LiteLLM or Kong for that.
- **Not a SaaS.** Your traffic never leaves your machine (except to the provider you configured).
- **Not magic.** It doesn't rewrite prompts or make decisions you can't audit.

## ✨ Features

### 🔒 Atomic Budget Enforcement

A **cost ledger with transactional reservations**: before dispatching, PennyPrompt reserves the estimated cost inside a SQLite transaction; after the response, it reconciles against actual cost. Concurrent requests cannot break your limit:

```
Request A: RESERVE $4 est. → ($45+$4=$49 of $50, OK)      → dispatch → RECONCILE $3.80
Request B: RESERVE $3 est. → ($49+$3=$52 of $50, BLOCKED) → HTTP 402
```

Blocks use **HTTP 402 (Payment Required)** with `"retryable": false` — agents auto-retry 429, but 402 tells them to stop and ask the human. Money is integer micros end-to-end: no floating-point drift, ever.

### 🎚️ Two Operating Modes

| Mode | Behavior | On DB/ledger failure |
|------|----------|----------------------|
| `observe` | Logs everything, never blocks | Request passes + `mode_failsafe` event |
| `guard` | Logs + blocks on hard limit | Request **blocked** (fail-closed) |

Start in `observe` to learn your costs. Flip to `guard` when you want protection.

### 🔄 Runaway Loop Detection

Three per-session heuristics, no ML, sub-millisecond:

- **Repeated tool failures** — same tool failing N times in M seconds
- **Abnormal burn-rate** — `$14.20/hr (threshold: $10/hr)`
- **Request similarity** — near-identical requests hammering the window

Actions: `alert` (log + `tail`) or `pause` (block the session until `penny-cli detect resume`).

### 🔮 Pre-Execution Cost Estimation

```
$ penny-cli estimate --model claude-sonnet-4-6 --context-files src/auth/

  claude-sonnet-4-6:  $0.12 – $0.45 (single pass) | $0.60 – $2.25 (agent task)
  claude-opus-4-7:    $0.35 – $1.25 (single pass) | $1.75 – $6.25 (agent task)
  claude-haiku-4-5:   $0.04 – $0.15 (single pass) | $0.20 – $0.75 (agent task)

  Budget: OK (day: $6.59 remaining of $10.00)
```

### 🏷️ Zero-Config Attribution

No custom headers, no virtual keys. **Project** is detected from the git root; **session** groups requests in a time window (default 30min). Useful reports from the very first request.

### 📺 Real-Time Monitoring

```
$ penny-cli tail

  [14:23:01] → sonnet  4,231 in / 892 out    $0.018  webapp/sess_x1
  [14:23:04] → sonnet  6,102 in / 1,203 out  $0.031  webapp/sess_x1
  [14:23:06] ⚠ BURN-RATE $14.20/hr (threshold: $10/hr) sess_x1
  [14:23:09] ⛔ BUDGET BLOCK  global/day: $10.12/$10.00  sess_x1
  [14:23:09] ← 402 budget_exceeded → client
```

Run `serve` with `--admin-bind 127.0.0.1:8586`, then `tail`, `detect`, and `dashboard` from another terminal.

### 🔐 Local-First & Private by Design

- Everything persists to a **local SQLite** file — reports work offline.
- API keys are read from env vars and **never persisted**.
- Prompts and responses are never sent anywhere except your configured provider.
- **Zero telemetry.** A tool that watches your spend must not watch you.

## ⚙️ How It Works

```
Request arrives (:8585)
  │
  ├─ Generate request_id (UUIDv7)
  ├─ Auto-detect project (git root) + session (time window)
  ├─ Map model → provider
  ├─ Estimate tokens (tiktoken-rs, per-model dispatch) + cost (pricebook)
  │
  ├─ RESERVE estimated cost in ledger (SQLite tx, BEGIN IMMEDIATE)
  │   ├─ accumulated + estimated > hard limit? → HTTP 402 (block)
  │   └─ OK → reservation created
  │
  ├─ DISPATCH to provider (Anthropic / OpenAI)
  │   ├─ Non-streaming: read full response
  │   └─ Streaming: forward SSE chunks immediately, accumulate in background
  │
  ├─ RECONCILE actual cost in ledger
  │   ├─ Usage from provider response (authoritative)
  │   └─ Fallback: tiktoken-rs estimation (tagged as estimated)
  │
  └─ DETECT loop / burn-rate patterns → alert or pause

  Proxy overhead: ~2–5ms (excluding upstream I/O)
```

Pricing lives in **local, versioned TOML pricebooks** (`prices/*.toml`) with effective dates. No scraping, no external pricing calls, works offline. Details: [docs/PRICEBOOK.md](docs/PRICEBOOK.md).

## 🔌 Compatibility

PennyPrompt works with any tool that speaks the OpenAI chat completions API:

| Agent | How to connect |
|-------|---------------|
| Codex | `OPENAI_BASE_URL=http://localhost:8585/v1` |
| Cursor | Settings → Models → Base URL → `http://localhost:8585/v1` |
| Continue | `config.json` → `apiBase` → `http://localhost:8585/v1` |
| OpenClaw / claw-code | Via OpenAI-compatible base URL today — native Anthropic ingress lands in alpha.5 ([#207](https://github.com/manuelpenazuniga/PennyPrompt/issues/207)) |
| NadirClaw | Chain: Agent → NadirClaw → PennyPrompt → Provider |
| curl / SDKs | `http://localhost:8585/v1/chat/completions` |

> **Current inbound contract (alpha):** the proxy accepts the **OpenAI-compatible** `POST /v1/chat/completions` surface. Native Anthropic ingress (`POST /v1/messages`) — letting Anthropic-native agents connect with zero translation — is the headline of the **alpha.5** train ([#207](https://github.com/manuelpenazuniga/PennyPrompt/issues/207)). All current constraints: [docs/LIMITATIONS.md](docs/LIMITATIONS.md).

## 🛠️ Configuration

Zero-friction start with presets:

```bash
penny-cli init --preset indie     # $30/mo, $10/day hard stop, guard mode
penny-cli init --preset team      # $100/mo, $20/day hard stop, guard mode
penny-cli init --preset explore   # $10/mo soft limit only, observe mode
```

Everything lives in one TOML file (`~/.config/pennyprompt/config.toml`):

```toml
[server]
bind = "127.0.0.1:8585"                 # Proxy plane
admin_socket = "127.0.0.1:8586"         # Admin plane (loopback only)
mode = "guard"                          # observe | guard

[providers.anthropic]
enabled = true
api_key_env = "ANTHROPIC_API_KEY"

[[budgets]]
scope_type = "global"
scope_id = "*"
window_type = "day"
hard_limit_usd = 10.0

[detect]
burn_rate_alert_usd_per_hour = 10.0
loop_action = "pause"                   # alert | pause
```

Every value can be overridden with `PENNY_`-prefixed env vars. Full reference — every field, preset, and default: [docs/CONFIG-REFERENCE.md](docs/CONFIG-REFERENCE.md).

## 💻 CLI Reference

```
penny-cli
├── init [--preset indie|team|explore]       Setup wizard
├── serve [--daemon] [--mock]                Start proxy + admin plane
├── estimate [--model M] [--context-files]   Pre-execution cost estimate
├── run <agent> [--execute] [--mock] -- ...  Launch an agent through the proxy
├── report summary|top                       Cost forensics
├── budget list|set|reset                    Manage budgets at runtime
├── detect status|resume                     Loop alerts & paused sessions
├── tail                                     Live request + cost stream
├── dashboard [--since] [--limit]            Snapshot KPIs
├── doctor                                   Full system diagnostics
├── prices show|update                       Pricebook management
└── config                                   Effective config (env resolved)
```

## 🏗️ Architecture

12 focused crates, clean dependency graph, the financial core isolated and exhaustively tested:

```
penny-types → penny-config → penny-store → penny-cost → penny-ledger
                                                             ↓
penny-cli → penny-proxy → penny-budget ─────────────────────┘
                 ↓              ↓
          penny-providers  penny-detect        penny-admin · penny-observe
```

| Choice | Why |
|--------|-----|
| Rust + tokio + axum | Single static binary, ~2–5ms proxy overhead |
| SQLite (WAL) + `BEGIN IMMEDIATE` | Atomic reservations, zero external services |
| Integer-micros `Money` | No floating-point drift in a financial ledger |
| UUIDv7 IDs | Temporally sortable, efficient range queries |
| TOML config | Strict typing, no YAML ambiguity |

Deep dive: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) · Proxy and admin planes are **separate by design** — exposing `:8585` never exposes admin endpoints.

## 🗺️ Roadmap

Roadmap source of truth: [docs/GITHUB_BACKLOG.md](docs/GITHUB_BACKLOG.md), driven by the [strategic audit](docs/STRATEGY-AUDIT-2026-07-05.md).

| Train | Theme | Highlights |
|-------|-------|-----------|
| ✅ `alpha.1`–`alpha.3` | Foundation → hardening | Proxy, atomic budgets, loop detection, streaming, real providers, release automation |
| 🔄 `alpha.4` | Operator usability | Serve daemon, `run` orchestration, installer smoke check ([#199](https://github.com/manuelpenazuniga/PennyPrompt/issues/199)) |
| ⏭️ `alpha.5` | **Compatibility & cost accuracy** | Native Anthropic `/v1/messages` ingress, prompt-cache cost accounting ([#225](https://github.com/manuelpenazuniga/PennyPrompt/issues/225)) |
| ⏭️ `alpha.6` | **Agent-aware moat** | Per-task budgets, human-approval circuit breaker, cost-feedback headers, invoice-parity benchmark, webhooks ([#226](https://github.com/manuelpenazuniga/PennyPrompt/issues/226)) |
| ⏭️ `beta.1` | **Scope expansion** | Gemini + local models (Ollama/vLLM) + OpenRouter, live TUI dashboard, MCP budget introspection, statusline, Homebrew ([#227](https://github.com/manuelpenazuniga/PennyPrompt/issues/227)) |
| ⏭️ `v1.0.0` | **Team, without betraying local-first** | Admin auth, optional PostgreSQL, durable detector state ([#228](https://github.com/manuelpenazuniga/PennyPrompt/issues/228)) |

**We track our gaps in public.** Known limitations are documented with dates and tracking issues in [docs/LIMITATIONS.md](docs/LIMITATIONS.md) and the backlog's honesty ledger — for a tool that guards your money, trust is the product.

## 🤝 Contributing

PennyPrompt is MIT licensed and contributions are welcome — provider adapters are a great entry point (a repeatable, well-bounded pattern with two reference implementations in the tree).

```bash
git clone https://github.com/manuelpenazuniga/PennyPrompt.git
cd PennyPrompt
cargo build --workspace
cargo test --workspace

# Run without API keys or real spend
cargo run -p penny-cli -- serve --mock
```

- Issue workflow & PR conventions: [docs/DEVELOPMENT_WORKFLOW.md](docs/DEVELOPMENT_WORKFLOW.md)
- Crate map for contributors: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- Release process: [docs/RELEASE.md](docs/RELEASE.md) · Changes: [CHANGELOG.md](CHANGELOG.md)

## 📄 License

MIT — see [LICENSE](LICENSE).

---

<div align="center">

**Every token has a price. Now you know it before you pay.**

⭐ If PennyPrompt saved you from a surprise invoice, a star helps others find it.

</div>
