# PennyPrompt GitHub Backlog

This backlog is the current issue and release-direction source of truth for the
project. It supersedes the original M1-M6 scaffold backlog (now historical): the
Rust workspace, proxy, budget ledger, admin plane, CLI, docs, tests, and alpha
release automation all exist and are shipped through `v0.1.0-alpha.3`.

Current baseline:
- Branch: `main`
- Latest published release: `v0.1.0-alpha.3`, published 2026-06-20 as a GitHub prerelease.
- In-flight release train: `v0.1.0-alpha.4` (operator usability) — `#201` and `#202`
  merged; `#203` installer smoke check open.
- Capture date: 2026-07-05, after the strategic audit
  (`docs/STRATEGY-AUDIT-2026-07-05.md`) established the post-alpha.4 roadmap.
- Active forward roadmap: Phases A-D below (`v0.1.0-alpha.5` → `v1.0.0`).

Source of truth for this backlog:
- The strategic audit: `docs/STRATEGY-AUDIT-2026-07-05.md`.
- GitHub epics `#225` (alpha.5), `#226` (alpha.6), `#227` (beta.1), `#228` (v1.0.0),
  plus child issues `#207`-`#224`.
- In-flight alpha.4 epic `#199` and child `#203`.
- Closed alpha.3 epic `#186` and blockers `#183`, `#184`, `#185`, `#189`, `#190`, `#196`.
- Implementation reality in `crates/`.

Operator-facing marketing copy is not used as normative release evidence here.
Local `docs/status-*.md` snapshots are working notes only; any decision needed by
the public roadmap or release gates must be repeated in tracked docs or GitHub issues.

---

## Non-Negotiable Design Constraints

These constraints remain fixed unless a dedicated architecture decision changes them:

1. Budget blocks use HTTP `402`, never `429`.
2. `guard` mode is fail-closed if budget or SQLite accounting fails.
3. The core accounting flow remains `reserve -> dispatch -> reconcile`.
4. Budget reservation and budget check happen in one SQLite transaction (`BEGIN IMMEDIATE`).
5. Provider-reported usage wins over estimates during reconciliation.
6. Project and session attribution should work without custom headers.
7. Pricebooks are local versioned files (a signed remote feed is opt-in, Phase C).
8. Proxy plane and admin plane stay separate.
9. Admin plane is local-control-plane scope; TCP admin exposure must stay loopback-only
   until token authentication is implemented and tested (Phase D `#221`).
10. Money is integer micros (`Money(i64)`) — no floating-point accumulation of cost.
11. Local-first, single self-contained binary, zero required external services
    (SQLite embedded). Any team/multi-node backend is strictly opt-in.
12. Alpha/beta releases are prereleases until a stable cut intentionally changes maturity.

---

## Consolidated Differentiators (the narrative)

These are the traits that separate PennyPrompt from the LLM-gateway category
(LiteLLM, Portkey, Helicone, OpenRouter). They are already true in the code except
where marked. Every roadmap item below either **protects** or **deepens** one of these.
Full competitive analysis: `docs/STRATEGY-AUDIT-2026-07-05.md` §3-§4.

1. **Local-first, single 15MB binary, zero external dependencies.** No PostgreSQL,
   Redis, or Docker required. Traffic never leaves the machine except the provider call.
2. **Atomic enforcement *before* the spend.** RESERVE→DISPATCH→RECONCILE in one SQLite
   transaction; concurrency-safe hard stops, not after-the-fact tracking.
3. **HTTP 402 semantics tuned for agents.** `retryable:false` tells the agent to stop
   and ask a human, instead of the retry storm a 429 triggers.
4. **Agent-loop awareness.** Burn-rate, repeated tool-failure, and content-similarity
   detection model the agent as a loop — a failure mode the app-centric gateways do not have.
5. **Zero-config auto-attribution.** Project by git root, session by time window; useful
   reports from the first request with no virtual keys or custom headers.
6. **Pre-execution cost estimation.** Answers "what will this cost?" before you spend.
7. **Correct-by-design financial core.** Integer-micros money, append-only auditable ledger.

Differentiators to *deepen* next (roadmap): perfect native-agent compatibility (Phase A),
cache-accurate cost receipts (Phase A), human-in-the-loop circuit breaker and per-task
budgets (Phase B), explicit data-sovereignty and router composition (Phase C).

---

## Phase Label Scheme

The `phase:mX` labels continue the original M1-M6 milestone scheme into the forward roadmap:

| Label | Release train | Theme |
|-------|---------------|-------|
| `phase:m6` | `v0.1.0-alpha.1`…`alpha.4` | Alpha release + operator usability |
| `phase:m7` | `v0.1.0-alpha.5` | Compatibility & cost accuracy (Phase A) |
| `phase:m8` | `v0.1.0-alpha.6` | Agent-aware moat (Phase B) |
| `phase:m9` | `v0.1.0-beta.1` | Scope expansion (Phase C) |
| `phase:m10` | `v1.0.0` | Team & scale (Phase D) |

---

## Forward Roadmap (post-alpha.4)

Sequencing principle: **close the two gaps that break the core promise first (Phase A),
then deepen the agent-aware moat (Phase B), then expand scope (Phase C), and only then
pursue team/scale (Phase D).** Expanding before the promise is fulfilled builds on an
unfulfilled promise. Rationale: `docs/STRATEGY-AUDIT-2026-07-05.md` §9-§11.

### Phase A — `v0.1.0-alpha.5` — Compatibility & cost accuracy · Epic `#225`

Blocker release. Make the headline promise ("works with your agent, zero changes")
literally true, and make reported cost correct for the flagship coding-agent workload.

- [ ] `#207` — feat(proxy): native Anthropic `/v1/messages` ingress
- [ ] `#208` — feat(cost): prompt caching cost accounting (cache read/write tokens)
- [ ] `#209` — feat(proxy): inbound concurrency limit and upstream timeout
- [ ] `#210` — docs: align compatibility and limitation claims with implemented ingress

Exit: a native Anthropic (OpenClaw-style) client completes a real streamed, tool-using
task with cache-accurate cost matching the provider invoice; README compatibility table
verified end-to-end; standard gate green; version/CHANGELOG/notes/gate updated.

### Phase B — `v0.1.0-alpha.6` — Agent-aware moat · Epic `#226`

Build what the generic gateway category structurally cannot.

- [ ] `#211` — feat(budget): per-task budget scope tied to auto-detected session
- [ ] `#212` — feat(detect): human-in-the-loop circuit breaker (`require_approval`)
- [ ] `#213` — feat(cli): `pennyprompt run <agent>` real orchestration
- [ ] `#214` — feat(detect): outbound alert webhooks and desktop notifications

Exit: `run --task-budget` gives a hard per-run cap; approval threshold pauses and
requires explicit approval; alerts reach a webhook/desktop with no sensitive payload and
no hot-path impact.

### Phase C — `v0.1.0-beta.1` — Scope expansion · Epic `#227`

Widen reach and sharpen positioning without diluting the moat.

- [ ] `#215` — feat(providers): Google Gemini adapter
- [ ] `#216` — feat(providers): local model support (Ollama/vLLM, OpenAI-compatible)
- [ ] `#217` — feat(providers): OpenRouter passthrough adapter
- [ ] `#218` — feat(cli): live TUI dashboard
- [ ] `#219` — feat(cost): signed remote pricebook feed sync
- [ ] `#220` — docs: data-sovereignty positioning and router composition guide

Exit: Gemini + local models route/stream/reconcile; live TUI dashboard renders and
degrades gracefully; `prices update --remote` verifies a signed feed atomically;
sovereignty + router docs published; README differentiators consolidated.

### Phase D — `v1.0.0` — Team without betraying local-first · Epic `#228`

Only start with evidence of team demand. SQLite stays the default; every team feature opt-in.

- [ ] `#221` — feat(admin): admin-plane token authentication
- [ ] `#222` — feat(store): optional PostgreSQL backend behind store trait
- [ ] `#223` — feat(store): separate read pool from single writer
- [ ] `#224` — feat(detect): persist detector state across restart

Entry gate: documented demand from real teams, or a concrete adopter blocked only by
single-node limits. Exit: admin auth enables safe non-loopback exposure; full suite
(incl. ledger concurrency) passes on PostgreSQL with SQLite still default; reads no longer
serialise behind the writer; paused/awaiting-approval sessions survive restart; `v1.0.0`
promotion is an intentional maturity change (no longer prerelease).

---

## Standard Release Gate

Every release train closes with the same gate:

```bash
cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo audit --ignore RUSTSEC-2023-0071
```

Then: bump workspace + `penny-cli` version, convert `CHANGELOG.md` `[Unreleased]` into
the tagged section, add `docs/RELEASE_GATE_<tag>.md` and `docs/release-notes/<tag>.md`,
tag, publish as prerelease (until v1.0.0), and verify artifacts + checksums.

Known local verification caveats:
- Tests that bind loopback ports may fail inside restricted sandboxes; they pass when
  loopback binding is permitted.
- Config tests can be affected by a real user config if `HOME` is not isolated. Use a
  clean `HOME` or CI runner. Recommended shape:

```bash
HOME="$(mktemp -d)" \
RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" \
cargo test --workspace --locked
```

---

## In-Flight: `v0.1.0-alpha.4` — Operator usability · Epic `#199`

Operator-usability release. Turns the hardened local alpha into a usable daily workflow
without expanding into team/dashboard/plugin/remote-control-plane scope.

- [x] `#200` — docs(roadmap): refresh backlog for alpha.4
- [x] `#201` — feat(cli): add serve daemon/background mode
- [x] `#202` — feat(cli): implement minimal `pennyprompt run` orchestration
- [ ] `#203` — test(release): add installer smoke check for latest prerelease

Closes once `#203` ships or is explicitly deferred, gate evidence is recorded, and the
alpha.4 release is published. `run <agent>` graduates from minimal orchestration to a real
local wrapper in Phase B `#213`.

---

## Completed Release: `v0.1.0-alpha.3` (Hardening)

Published 2026-06-20; epic `#186` closed. Delivered scope: `#183` CLI help, `#184`
per-model tokenizer dispatch, `#185` structured proxy tracing, `#189` rustls-webpki
refresh + `cargo audit` gate, `#190` admin security contract docs, `#196` gate/notes.

Publication evidence:
- Tag: `v0.1.0-alpha.3` — https://github.com/manuelpenazuniga/PennyPrompt/releases/tag/v0.1.0-alpha.3
- Release run: `27873967227`
- CI artifacts: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`
- Local backfill: `x86_64-apple-darwin`, SHA-256 `582b1ecb273126fe089b57789d54d9e619bbf3382b83c1ed6a1d3c7ee741e6b6`
- `CHECKSUMS.txt` verified locally for all 4 archives.

---

## Known Gaps Tracked as Roadmap (honesty ledger)

These are real gaps between current marketing narrative and shipped behaviour. They are
tracked, not hidden, and doc hygiene (`#210`) keeps user-facing claims honest until the
code lands.

| Gap | Impact | Tracked by |
|-----|--------|-----------|
| No native Anthropic `/v1/messages` ingress (OpenAI-compatible inbound only) | Native Anthropic agents (the primary target) cannot use the proxy without an OpenAI-compatible base URL | `#207`, docs `#210` |
| Prompt-cache tokens not accounted | Reported cost is systematically off on cache-heavy agent workloads | `#208`, docs `#210` |
| Admin plane has no authentication | Any local process reaching the admin port can read reports and mutate budgets (mitigated: documented local-only) | `#221` |
| Detector state is in-memory | Paused/awaiting-approval sessions do not survive a `serve` restart | `#224` |
| `max_connections(1)` serialises reads | Reporting/health reads block behind the writer under load | `#223`, `#209` |
| Provider coverage = Anthropic + OpenAI only | Gemini/local/OpenRouter users excluded | `#215`, `#216`, `#217` |

---

## Deferred Parking Lot

Do not pull these forward without an explicit decision:
- Plugin system.
- Grafana/Prometheus/OTLP metrics export.
- Full RBAC/SSO, managed/hosted SaaS, Kubernetes.
- Email/SMS/PagerDuty native integrations (generic webhook `#214` covers relays).
- Web UI (the dashboard is terminal-native by design, `#218`).

---

## Historical Backlog Status

The original M1-M6 plan is delivered for alpha scope:
- M1 Foundation, M2 Proxy pass-through, M3 Atomic budgets, M4 Streaming + real providers,
  M5 Active protection, M6 Alpha release: all delivered through `v0.1.0-alpha.3`.

Forward roadmap docs should start from the Phase A-D scope above, not from the old
scaffold issue list.
