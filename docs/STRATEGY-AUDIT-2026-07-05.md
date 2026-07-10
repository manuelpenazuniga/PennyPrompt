# PennyPrompt — Strategic Audit: Progress & Differentiators

**Date:** 2026-07-05 · **Revision 1.1:** 2026-07-06 (adds §8 D7–D9, §10 adoption/GTM levers, extends roadmap B/C with issues `#230`–`#234`, and corrects claims against verified LiteLLM/MCP prior art)
**Branch audited:** `feat/m6-issue-202-run-orchestration` (post `v0.1.0-alpha.3`, alpha.4 in flight)
**Audit author:** assisted review against the real state of the code, not the marketing copy.
**Scope:** product progress, security, scalability, functionality, competitive landscape, current and proposed differentiators, and an actionable roadmap to grow adoption.

> Method note: every claim about product state was checked against `crates/`, `migrations/`, `prices/`, `presets/`, and the release docs. Wherever the README promises something the code does not yet deliver, it is explicitly flagged as a **gap**, because a promise/reality gap is exactly what blocks adoption.

---

## 0. Executive TL;DR

PennyPrompt is already a real product, not a prototype: ~19.5k lines of Rust, 12 crates with clean boundaries, 190 tests, an atomic ledger using `BEGIN IMMEDIATE`, integer money (micros), three published alpha releases, and working release automation. **The financial core is solid and defensible.**

But the product is positioned as "cost guardrails for AI agents that work with your agent without changing anything", and there are **two gaps that attack that central promise directly**:

1. **No native Anthropic ingress (`/v1/messages`).** The proxy only accepts the OpenAI format (`/v1/chat/completions`). OpenClaw/claw-code-style agents — the README's own primary target — speak the native Messages API. Today, pointing `ANTHROPIC_BASE_URL` at the proxy 404s on the route the agent actually calls.
2. **Anthropic prompt caching is not accounted.** Coding agents reuse huge contexts with caching; without reading `cache_read_input_tokens`/`cache_creation_input_tokens`, the reported cost is systematically wrong in exactly the flagship use case.

This audit's thesis: **PennyPrompt's moat is not "another LLM gateway" — that space is saturated (LiteLLM, Portkey, Helicone, OpenRouter). The moat is being the only *local-first, zero-dependency, autonomous-agent-aware* guardrail with atomic enforcement *before* the spend.** The two gaps above, plus the new differentiators detailed below, are what turn that moat into adoption.

---

## 1. The real pain we solve (maximum abstraction)

Going one level of abstraction above "controlling costs": the underlying pain has three layers.

**Layer 1 — Loss of control over a spend that became variable overnight.**
The founding event (the end of flat-rate pricing for 135k OpenClaw instances on 2026-04-04) turned a *fixed, predictable* cost into a *variable, invisible, potentially unbounded* one. An autonomous agent is a process that spends real money in a loop, with no human watching each iteration. It is the first time the individual developer has a local process that can burn $50 while they're at lunch.

**Layer 2 — Temporal information asymmetry.**
Cost is known *after* it is incurred. Provider dashboards are aggregate and delayed. The developer cannot answer three basic questions *at the moment they matter*:
- *Before:* "how much will this task cost me?" → no answer today.
- *During:* "is this getting out of hand right now?" → today they find out from the invoice.
- *After:* "where exactly did the money go?" → today they only see a total.

PennyPrompt exists to collapse that asymmetry: **estimate before, protect during, explain after.**

**Layer 3 — The agent is not a user, it is a loop.**
This is the key abstraction that separates PennyPrompt from every generic gateway. A traditional LLM gateway models *applications with users* (virtual keys, per-API-key rate limits, team tags). An autonomous agent models *a process that retries, compacts memory, and can enter failure loops*. An agent's characteristic economic failure — retrying the same failed tool 30 times — **has no analogue in the app world**, which is why generic gateways don't detect it. PennyPrompt treats the agent as what it is: a loop with a credit card.

> **The audience that feels this pain most acutely:** the indie developer and the small team (2-10) running autonomous coding agents locally, who were pushed off flat-rate pricing, and for whom standing up LiteLLM + PostgreSQL + Redis in the cloud is disproportionate. That is the adoption wedge.

---

## 2. Progress state (what is built and how mature)

| Area | State | Evidence |
|------|-------|----------|
| Workspace / architecture | ✅ Solid | 12 crates, clean dependency graph (leaf `penny-types`/`penny-config`), 19.5k LOC |
| Financial core (ledger) | ✅ Solid | `reserve/reconcile/release`, `BEGIN IMMEDIATE`, concurrency tests |
| Money type | ✅ Solid | `Money(i64)` in micros — migrations 0008/0009 moved everything to integers. No float drift |
| Budgets + modes | ✅ Works | observe/guard, fail-closed in guard, soft/hard, day/week/month windows |
| Loop detection | ✅ Works | burn-rate, repeated tool failures, content similarity (sha256 of first 500 chars) |
| Provider adapters | 🟡 Partial | Anthropic + OpenAI + Mock. SSE streaming in both. Only 2 real providers |
| Pricebook | 🟡 Partial | Local, versioned; 7 Anthropic + 3 OpenAI models. No signed remote feed |
| Proxy ingress surface | 🔴 Gap | Only `/v1/chat/completions`. **No native Anthropic `/v1/messages`** |
| Prompt caching accounting | 🔴 Gap | Cache tokens not read → wrong cost for coding agents |
| Admin plane | 🟡 Intentional | Reports, budgets, health, event SSE. **No auth** (documented as local-only) |
| CLI | ✅ Rich | init, serve, estimate, run, report, budget, detect, tail, doctor, prices, config, dashboard |
| `serve --daemon` | ✅ New (alpha.4) | #201 |
| `run <agent>` orchestration | 🟡 Minimal | #202 — dry-run + `--execute` limited to agents honoring an OpenAI-compatible base URL |
| Release / CI | ✅ Mature | `cargo audit` as gate, checksums, multi-arch matrix, 3 published alphas |

**Reading:** the project executed M1–M6 with discipline. The debt is not in the core (the hard part, and it is well built) but in the **compatibility surface** and some **cost-accuracy details** which, paradoxically, are what the user *sees first*.

---

## 3. Competitive landscape — alternatives in other repos and where OpenClaw sits

The strategic mistake to avoid is competing in the wrong category. There are three categories and PennyPrompt should only fight in one.

### 3.1 Gateways/observability (the saturated category — do NOT compete head-on)

| Tool | What it is | Budget enforcement | Requirements | Orientation |
|------|-----------|--------------------|--------------|-------------|
| **LiteLLM** | Python proxy, 100+ LLMs | `max_budget` per key/user/team, multiple windows | **PostgreSQL + Redis** | Teams/cloud, virtual keys |
| **Portkey** | Full-stack LLMOps; open-source gateway (Apache 2.0, Mar 2026) | Budgets + guardrails, PII redaction, jailbreak detection | Self-host gateway + platform | Production/enterprise |
| **Helicone** | Observability + light proxy, open-source | Tracking + rate limiting | Self-host or SaaS | Logging/analytics. **Acquired by Mintlify 2026, maintenance mode** |
| **OpenRouter** | Hosted aggregator, 300+ models | — (5.5% fee) | None (SaaS) | Simplicity, one API key |

**Conclusion:** all of these model *applications with users* and almost all track spend *after* the call (or with soft limits). LiteLLM is the closest on budget features, but its enforcement is not a concurrency-proof atomic pre-dispatch reservation, and its operating cost (Python + Postgres + Redis) is disproportionate for an individual dev. **PennyPrompt loses if it tries to be "LiteLLM but in Rust". It wins by being the category next door.**

### 3.2 Routers (complementary — compose, don't compete)

NadirClaw and similar tools *pick the model*. PennyPrompt explicitly is **not** a router (the README says so, and it is correct). The natural chain is `Agent → NadirClaw → PennyPrompt → Provider`. This is an asset: no routing needs to be built, only clean integration with it.

### 3.3 Where OpenClaw sits (the host of the pain)

OpenClaw (and claw-code) is the **autonomous coding agent** that lives in the developer's terminal and suffered the pricing change. It is not a competitor: **it is the substrate PennyPrompt installs onto**. The strategic question is not "how do I beat OpenClaw?" but "how do I become the default layer every OpenClaw user installs on day 1?". That requires:
- Perfect native compatibility with how OpenClaw speaks (→ the `/v1/messages` gap).
- Cost accuracy on OpenClaw's real usage pattern (→ the prompt caching gap).
- Zero install friction (single binary — **we already have this, and it's huge**).

---

## 4. Our CURRENT differentiators (what already sets us apart)

These are real and already in the code. They must be protected and made legible in the message.

1. **Local-first, single binary, zero external dependencies.** ~15MB, embedded SQLite, no PostgreSQL/Redis/Docker. Against LiteLLM/Portkey this is a *friction* differentiator and a *privacy* one (traffic never leaves the machine except to the provider). For the indie dev it is the difference between "installed in 2 minutes" and "not installed".

2. **Atomic enforcement *before* the spend (ledger reservation).** RESERVE→DISPATCH→RECONCILE in one SQLite transaction with `BEGIN IMMEDIATE`. Most competitors account *after*; PennyPrompt blocks the N+1 request that would break the limit, correctly under concurrency. A *technical, verifiable* differentiator.

3. **HTTP 402 semantics designed for agents, not 429.** Agents retry 429; 402 `retryable:false` tells them "stop and ask the human". A small detail with enormous impact inside an autonomous loop. No generic gateway thinks about this.

4. **Agent-loop detection (burn-rate, tool failures, similarity).** A feature that *does not exist* in the gateway category because it comes from modeling the agent as a loop. The hardest differentiator to copy, because it requires thinking in agents, not apps.

5. **Auto-attribution without custom headers** (project by git root, session by time window). Useful reports from the first request, zero config. Competitors demand virtual keys or tags.

6. **Pre-execution estimation** ("how much will this cost?"). Rare in the market; answers the question *before* almost anyone else does.

7. **Financial core correct by design** (integer-micros money, auditable append-only ledger). Trust: when the product says "$4.23", it is $4.23.

---

## 5. Security findings

Ordered by relevance to real adoption/operation.

| # | Finding | Severity | Note |
|---|---------|----------|------|
| S1 | **Admin plane has no authentication.** No bearer/admin token (confirmed: zero auth references in `penny-admin`). | Medium (mitigated by design) | Already documented as local-only and loopback/unix-socket. Acceptable for alpha, but an **adoption ceiling** for the jump to "team". Any local process can read reports and **mutate budgets** via `POST /admin/budgets` → effectively disable the guardrail. |
| S2 | **No provider key management/rotation path.** API keys are read from env (`api_key_env`). Good (not persisted), but no rotation or scoping. | Low | Correct for alpha; documenting that keys never touch the DB is a *privacy selling point*. |
| S3 | **Dynamic SQL in reports** (group key / join variant). | Low (controlled) | Already audited: fragments come from enums, filters use bind params. Keep the guardrail; migrate to a query builder if it grows. |
| S4 | **`cargo audit` as a gate** already integrated (rustls-webpki refreshed). | ✅ Positive | Good hygiene. Keep the gate on every release. |
| S5 | **Payload cleanup / ANSI strip** in the proxy. | ✅ Positive | Reduces terminal-escape-injection surface in outputs the operator views in `tail`. |

**Highest-leverage security recommendation:** turn the absence of auth from a "limitation" into an *architecture decision with an exit door*: keep local-only by default, but design the admin token contract now (even before implementing) so "team mode" needs no redesign. Budget mutation via unauthenticated admin is the most concrete risk: a compromised agent that discovers the admin port can raise its own limit.

---

## 6. Scalability findings

| # | Finding | Impact | Recommendation |
|---|---------|--------|----------------|
| E1 | **`max_connections(1)` on the SQLite pool.** Serializes *all* operations, not only reservation writes. | Throughput ceiling under many concurrent agents/sessions. | Correct for local single-node consistency. To scale reads: separate a read pool (WAL allows concurrent readers) from the single writer. Measure before optimizing. |
| E2 | **In-memory loop detection** (`HashMap<SessionId, SessionWindow>` behind `RwLock`). | Non-persistent state: a restart loses windows and paused sessions. | Acceptable for alpha. Document that `detect resume` and pause state don't survive restart. For v1, consider a light snapshot. |
| E3 | **No explicit backpressure or inbound connection limit** on the proxy. | An agent opening many connections can saturate the single writer. | Add a concurrency limit (tower `ConcurrencyLimit`) and configurable upstream timeouts. |
| E4 | **One node, one SQLite file.** | Multi-machine / shared team not supported. | Already an alpha non-goal (correct). PostgreSQL is the v1 path for team, but **not before** exhausting the single-node market. |
| E5 | **Pricebook and reconciliation load fine**, but **streaming reconcile depends on estimation** when the provider sends no usage. | Degraded cost accuracy on streams without final usage. | Tied to the prompt caching gap (§7). Prioritize accuracy over throughput: it is the brand promise. |

**Reading:** *current* scalability is right for the target audience (local single-node). The strategic risk is not "doesn't scale to 1000 nodes" (not the market) but **presenting the product as team-ready too early**. Keep the message honest: "a local guardrail for your machine/small team".

---

## 7. Functional findings / product gaps (the ones that move adoption)

Ordered by impact on the central promise "works with your agent, zero changes".

### F1 — 🔴 No native Anthropic ingress (`/v1/messages`) — **gap #1**
The proxy router registers exactly three routes: `/v1/chat/completions`, `/v1/models`, `/internal/health`. There is no `/v1/messages`. The `AnthropicProvider` translates **output**, but **there is no input surface** for a client speaking the native Messages API. Since OpenClaw/claw-code (the declared primary target) speak native Anthropic, pointing `ANTHROPIC_BASE_URL=http://localhost:8585/v1` would make the agent hit `/v1/messages` → 404. **This contradicts the README compatibility table.** It is the highest-ROI fix in the entire backlog: without it, the zero-friction tagline is unfulfilled for the most important user.

### F2 — 🔴 No prompt caching accounting — **gap #2**
`cache_creation_input_tokens` and `cache_read_input_tokens` are not read (zero references in `penny-cost`/`penny-providers`/`penny-types`). Coding agents use prompt caching aggressively (reused repo context). A cached read costs ~10% of normal input and a cache write ~125%; ignoring them **materially over- or under-states real cost** in exactly the flagship flow. The brand is "when we say $X, it's $X" — this gap silently erodes it.

### F3 — 🟡 Narrow provider coverage
Only Anthropic + OpenAI. No Google/Gemini, no OpenRouter passthrough, no local (Ollama/vLLM). Many indie devs run local models or mix providers. Every missing provider is a segment that cannot adopt.

### F4 — 🟡 `run <agent>` still minimal
Dry-run + `--execute` limited to agents honoring an OpenAI-compatible base URL. The piece that turns PennyPrompt from "a proxy you configure" into "a wrapper you invoke" (`pennyprompt run openclaw -- ...`). High UX leverage, but correctly bounded for now.

### F5 — 🟡 No live dashboard (only textual `tail`)
`tail` is functional, but a TUI/panel is what creates the "aha moment" and the shareable screenshots (organic marketing). Correctly deferred, but it is an adoption multiplier.

### F6 — 🟢 No webhooks/outbound alerts
No way to notify Slack/Discord/desktop on a block or burn-rate alert. Devs don't live watching `tail`. Deferred, reasonable.

---

## 8. Proposed NEW differentiators (to grow adoption)

Each is chosen by one rule: **deepen the "agent-aware + local-first" moat, don't dilute it toward "another gateway".**

### D1 — Native Anthropic compatibility as a *headline feature* (solves F1)
Not just closing a bug: make it the message. "Point your OpenClaw at PennyPrompt and it works identically — no translation, no lost streaming or tool-use." *Perfect* compatibility with the market's #1 agent is itself a differentiator against gateways that force the OpenAI format.

### D2 — The agent's "cost receipt": cache accuracy as a flag (solves F2)
Be the **only** guardrail that correctly accounts Anthropic prompt caching. A report that breaks down fresh input vs cached input vs cache write vs output. For the coding-agent user this is *the* number nobody else gets right. Accuracy as differentiator, not as hygiene.

### D3 — "Circuit breaker with human approval" (deepens the loop differentiator)
Today: block (402) or pause session. New: when a task exceeds an estimated-cost threshold, **pause and request explicit approval** (desktop notification / CLI response) before continuing. Turns the passive guardrail into an *economic human-in-the-loop*. Nobody in the gateway category does this because nobody models "agent task" as a unit.

### D4 — Budget per *agent task*, not only per time window
Competitors budget per key/user/day. PennyPrompt can budget per **task** ("don't spend more than $2 solving this issue"), tied to the auto-detected session. It is the user's real mental unit for agents: "this feature cost $3". A conceptual differentiator that is hard to copy without the auto-attribution we already have.

### D5 — Privacy/data sovereignty as an explicit differentiator
Against SaaS gateways (OpenRouter/Portkey managed) and against Helicone (now in maintenance): "your prompt, your code, your cost — nothing leaves your machine except the provider call." For healthcare/finance/legal this is a hard requirement. Already true in the code; missing is making it a first-line message and perhaps a "no telemetry" attestation.

### D6 — Explicit composition with routers (NadirClaw) as the standard
Publish the canonical integration `Agent → Router → PennyPrompt → Provider` with *per-candidate-model* estimation. "The router picks the model; PennyPrompt tells you what each option costs and stops you if you exceed." Turns a potential competitor into a distribution channel.

### D7 — The cost-aware loop: from guard to sense organ *(added rev. 1.1 — the biggest strategic differentiator)*
Everything above treats the agent as something to *police* (block, pause, approve). The next conceptual leap is giving the agent the signal to **self-regulate**: knowing how much it has spent and how much remains, *before* hitting the wall — so it can pick a cheaper model, compact context, or stop on its own.

Two mechanisms, in order of friction:
1. **Response headers** (`X-Penny-Request-Cost-USD`, `X-Penny-Session-Cost-USD`, `X-Penny-Budget-Remaining-USD`, `X-Penny-Budget-Scope`) on every proxied response — zero integration (issue `#230`).
2. **MCP introspection server** (`pennyprompt mcp`): the agent asks `get_budget_status` / `estimate_cost` of the *same ledger that enforces* (issue `#232`).

**Prior art (verified — do not over-claim):** LiteLLM already exposes a per-response cost header (`x-litellm-response-cost`) and rate-limit-remaining headers; and standalone MCP spend *meters* exist. What does **not** exist is the closed loop with authority: *remaining budget in USD for the scopes an agent cares about (task/session), emitted by the same atomic ledger that will return the 402*. The number in the header is the number that blocks you. Passive meter ≠ introspectable guardrail. Combined with the 402 semantics and the approval flow (D3), this makes PennyPrompt the only piece that closes the perception→decision→enforcement circuit.

### D8 — Published accuracy proof: the invoice-parity benchmark *(added rev. 1.1)*
The brand is "when we say $X, it's $X" — but a claim without reproducible evidence is just marketing. A harness that runs a representative workload (streaming, tools, cache) against real providers and publishes the deviation between PennyPrompt-reported and provider-billed cost (**target: ≤1%**), re-runnable by third parties, turns accuracy into a *verifiable fact* and doubles as a permanent regression net for the accounting (issue `#231`). For a trust product, the proof **is** the marketing.

### D9 — Visibility embedded in the developer's workflow *(added rev. 1.1)*
`pennyprompt statusline`: a one-line segment (`$1.42 session · $6.20/hr · 62% day`, <50ms) embeddable in the Claude Code/OpenClaw status line, starship, or tmux (issue `#233`). The product's value stays on screen all day **and appears in every screenshot the user shares** — organic distribution embedded in the workflow, complementary to the TUI (`#218`).

---

## 9. Detailed roadmap toward the differentiators

Guiding principle: **first close the two gaps that break the central promise (F1, F2), then deepen the agent moat (D3, D4, D7), and only then expand scope (providers, team).** Expanding before closing the gaps is building on an unfulfilled promise.

### Phase A — "Fulfill the promise" (alpha.4 → alpha.5) · *adoption blocker*
Goal: the README compatibility table becomes literally true and the cost number becomes correct.

- **A1. Native Anthropic ingress `/v1/messages`** (closes F1 → D1).
  - New route in `build_router`. Messages→`NormalizedRequest` normalizer. Preserve native Anthropic SSE streaming (event: message_start/content_block_delta/message_delta/message_stop) and `tool_use`.
  - Integration test: native Messages request → mock → 200 with Anthropic shape, ledger reconciled.
  - Update the README so the OpenClaw claim is end-to-end verifiable.
- **A2. Prompt caching accounting** (closes F2 → D2).
  - Extend `AccountedUsage` and the pricebook with `cache_read`/`cache_write` rates. Read Anthropic usage fields (and OpenAI `prompt_tokens_details.cached_tokens`).
  - Reconcile uses the four categories. Reports break down fresh/cached input/output.
  - Calibration fixtures as in the tokenizer dispatch (`#184`).
- **A3. Concurrency limit + upstream timeout** (E3).
  - `tower` ConcurrencyLimit and configurable timeout. Saturation test.
- **A4. Installer smoke test** (#203, already in alpha.4). Close it.

**Phase exit:** an OpenClaw user installs, points, runs a real cache-heavy task, and the reported cost matches the provider invoice within a small margin. *That* is the credibility moment.

### Phase B — "Deepen the agent moat" (alpha.5 → beta) · *differentiation*
Goal: features the gateway category structurally does not have.

- **B1. Per-task/session budget** (D4). New `ScopeType::Task` tied to the auto-detected session; CLI `budget set task:<id>`; estimation consumes task budget.
- **B2. Circuit breaker with human approval** (D3). New `require_approval` action besides `alert`/`pause`. Desktop notification + resume via CLI. `ApprovalRequested` event.
- **B3. Real `pennyprompt run <agent>`** (F4). `run openclaw -- <args>` spins an ephemeral proxy, injects the base URL, attaches task attribution, tears down on exit. Turns the proxy into a wrapper.
- **B4. Webhooks/outbound alerts** (F6). Slack/Discord/desktop on block, burn-rate, approval. `[detect.webhooks]` config.
- **B5. Cost-feedback headers** (D7, `#230`) *(rev. 1.1)*. Request cost + remaining budget per scope on every response, emitted by the enforcing ledger. Cheap to build, opens the cost-aware loop.
- **B6. Invoice-parity benchmark** (D8, `#231`) *(rev. 1.1)*. Only meaningful after A1+A2; sequence it last in the train. Publishes the evidence for Phase A's exit criterion.

**Phase exit:** PennyPrompt does things LiteLLM/Portkey cannot do *by design*, not for lack of features — and accuracy stops being a claim and becomes a reproducible report.

### Phase C — "Expand scope without diluting" (beta → v1) · *growth*
- **C1. Providers** (F3): Gemini/Google, OpenRouter passthrough, local (Ollama/vLLM). Each opens a segment.
- **C2. Live TUI/dashboard** (F5). The organic-marketing multiplier (shareable screenshots).
- **C3. Signed remote pricebook feed.** Keep accuracy current without manual releases; signed so as not to break the "no scraping, no unverified external calls" model.
- **C4. Explicit privacy differentiator** (D5): "zero telemetry" audit, data sovereignty doc, perhaps attestation.
- **C5. Canonical router integration** (D6): NadirClaw recipes, multi-model estimation.
- **C6. MCP budget introspection server** (D7, `#232`) *(rev. 1.1)*. Closes the cost-aware loop: read-only, ≤5 tools, backed by the enforcement ledger.
- **C7. Embeddable statusline** (D9, `#233`) *(rev. 1.1)*. <50ms, graceful degradation, recipes for Claude Code/OpenClaw/starship/tmux.
- **C8. Distribution channels** (`#234`) *(rev. 1.1)*. Homebrew tap, `cargo-binstall`, one-page per-agent integration guides. The cheapest multiplier on everything else (see §10).

### Phase D — "Team without betraying local-first" (v1+) · *only if the single-node market is exhausted*
- **D-1. Admin plane auth** (S1): design the token contract *now* (Phase A, without implementing) to avoid a redesign here.
- **D-2. Optional PostgreSQL backend** (E4) behind the same store trait. SQLite remains the default.
- **D-3. Read pool separated from the writer** (E1).

**Golden rule for Phase D:** do not start until there is evidence of team demand. The death risk is not "we lack team mode" — it is "we diluted the local-first moat chasing enterprise before dominating the niche".

---

## 10. Adoption levers (go-to-market) — *added rev. 1.1*

The critical review of v1.0 of this document exposed its biggest hole: it was 100% product. But for an open-source project, **adoption = product × distribution × trust × visibility** — and three of the four factors were missing. Phases A–D build the product; this section builds the rest. Without it, a good product stalls at 30 stars.

### 10.1 Trust (the proof is the marketing)
For a *money guardrail*, trust is not a nice-to-have: it is the only buying reason. Levers:
- **Invoice-parity benchmark published per release** (`#231`, D8). "≤1% deviation — re-run it yourself" is worth more than any post.
- **Verifiable zero telemetry** (`#220`, D5): not just privacy but coherence — a product that watches your spend must not watch you.
- **Honesty ledger** in the backlog (already exists): gaps are published with a date and an issue, not hidden. Keeping it is a policy, not a document.
- Public `cargo audit` gate (already exists).

### 10.2 Distribution (channels, `#234`)
- **Homebrew tap + `cargo-binstall`**: the single binary is the friction differentiator; without `brew install` it is wasted on the macOS segment that dominates the target audience.
- **One-page integration guides per agent** (OpenClaw, claw-code, Cursor, Codex, Continue): the exact paste + a verification step. Each guide doubles as an indexable landing page ("openclaw cost limit", "cursor budget cap") — organic SEO with extremely high intent.
- **Lists and registries**: awesome-rust, awesome-llm; MCP registries once `#232` exists (every registry is a channel).

### 10.3 Daily visibility (the product shows itself)
- **Statusline** (`#233`, D9) and **TUI** (`#218`): live cost sits on the dev's screen all day and in every screenshot they share. Marketing embedded in the workflow, consistent with zero telemetry: we don't track users — users show us off.
- **Headers** (`#230`): the `X-Penny-*` prefix travels through third-party logs and debug output — the name propagates on its own.

### 10.4 Community (turn the adapter pattern into a quarry)
- **Provider adapters** (C1–C3) are the perfect `good-first-issue`: a repeatable, well-bounded pattern with two reference examples in the tree. Labeling and documenting "how to add a provider" turns gap F3 into a contributor quarry instead of our own backlog.
- Every per-agent guide (`#234`) ends with "your agent missing? PRs welcome" — the integration directory grows itself.

### 10.5 North-star metrics without telemetry
Consistent with D5: users are never instrumented. Measure with public signals:
- **Release downloads per version** (`gh api`), stars/week, public Homebrew tap analytics.
- **Issues/discussions opened by third parties** — the strongest real-adoption signal that exists for a local-first project.
- Suggested north star: **weekly release downloads** + **non-maintainer issues/month**. Ritual: monthly snapshot in `docs/status-*.md`.

### 10.6 Launch sequence tied to the release trains
The rule: **one big shot, and only when the promise is demonstrable.** Launching before Phase A would burn the only credibility shot (gap F1 would be the first comment in the thread).
- **alpha.5** — no promotion: it is a correction release. Only update guides/README.
- **alpha.6** — first technical content: "how atomic budget reservation works", "invoice-parity report #1". Builds authority, not traffic.
- **beta.1** — **the launch** (Show HN, r/LocalLLaMA, lobste.rs): with parity published, a cost-aware-loop demo (statusline + TUI GIF + a 402 saving money), `brew install` working, and five per-agent guides. Everything aligned in a single moment.
- **v1.0.0** — stability announcement; homebrew-core, winget/apt (the "serious" registries require non-prerelease).

### 10.7 Sustainability (one-line note)
GitHub Sponsors from now (zero friction). If monetization ever comes, it lives in the team tier (v1+), **never** as a gate on local features: free local-first *is* the moat, not the bait of a freemium.

---

## 11. Recommended priority (if only one thing per quarter)

1. **A1 + A2** (native Anthropic ingress + prompt caching). Without these, the central promise is unfulfilled for user #1. Everything else is secondary.
2. **B5 + B2 + B4** (cost-feedback headers + approval circuit breaker + alerts). The complete agent loop: perception, decision, enforcement. The purest "agent-aware" differentiator and the hardest to copy.
3. **B6 + C8** (parity benchmark + distribution channels). Demonstrable trust + minimal install friction = the preconditions of the beta.1 launch (§10.6).
4. **B3** (real `run`) + **C7/C2** (statusline + dashboard). UX and organic visibility.
5. **C1 providers + C6 MCP** to open segments and close the cost-aware loop, in observed-demand order.

---

## 12. Strategic risks and anti-goals

- **Risk #1 — Competing as a generic gateway.** If the roadmap drifts toward "LiteLLM features in Rust", we lose. The moat is agent + local-first, not 100-model coverage.
- **Risk #2 — Promising team/enterprise before dominating the niche.** Dilutes the message and the design. Keep scope honesty (the docs already do this well).
- **Risk #3 — Silently wrong accuracy** (F2). A cost guardrail that misreports cost loses its only reason to exist. Accuracy is the brand, not a feature.
- **Risk #4 — Promise/reality gap** (F1). The README promises compatibility the router does not deliver. Close the gap or adjust the promise; leave none open.

**Anti-goals to maintain** (already well defined in the backlog): not a router, not an enterprise gateway, not a SaaS, no price scraping, no admin exposure without auth beyond loopback.

---

## 13. One-sentence synthesis

> **PennyPrompt is not "another LLM gateway": it is the first cost guardrail that treats the autonomous agent as what it is — a local loop with a credit card — in a 15MB binary with zero dependencies. The moat already exists in the code. Adoption depends on three moves in order: (1) close the two gaps that break the central promise (native Anthropic ingress and cache accounting), (2) build what no generic gateway can copy — per-task budgets, a human-approval circuit breaker, and the cost-aware loop where the same ledger that blocks is the one that informs the agent — and (3) launch once, with accuracy proven by a reproducible parity benchmark and installation one command away.**

---

### Annex — Code evidence consulted

- Proxy routes: `crates/penny-proxy/src/lib.rs` (`build_router`, ~lines 281-285) — only 3 routes, no `/v1/messages` ingress.
- Adapters: `crates/penny-providers/src/lib.rs` — Anthropic/OpenAI/Mock; Anthropic translates output to `/v1/messages` (~272).
- Prompt caching: no references to `cache_read`/`cache_creation` in `penny-cost`/`penny-providers`/`penny-types`.
- SQLite pool: `crates/penny-store/src/lib.rs:106-114` — `max_connections(1)`, WAL, foreign_keys.
- Atomic ledger: `crates/penny-ledger/src/lib.rs:373-375` — `begin_with("BEGIN IMMEDIATE")`.
- Money: `crates/penny-types/src/lib.rs` — `Money(i64)` in micros.
- Admin auth: no bearer/token/auth references in `crates/penny-admin/`.
- Pricebook: `prices/anthropic.toml` (7 models), `prices/openai.toml` (3 models); no Gemini/local.
- Tests: 190 (`#[test]`/`#[tokio::test]`/`#[sqlx::test]`).

### Annex — Competitive sources

- [LiteLLM — Budgets & Rate Limits](https://docs.litellm.ai/docs/proxy/users) · [Virtual Keys](https://docs.litellm.ai/docs/proxy/virtual_keys) · [Spend Tracking](https://docs.litellm.ai/docs/proxy/cost_tracking)
- [LLM Gateway 2026: OpenRouter vs LiteLLM vs Portkey vs Helicone](https://klymentiev.com/blog/llm-gateway-guide)
- [Best LLM Gateways 2026 — Braintrust](https://www.braintrust.dev/articles/best-llm-gateways-2026)
- [7 Best OpenRouter Alternatives 2026](https://ofox.ai/blog/openrouter-alternatives-2026/)

Verified prior art for D7 (rev. 1.1):
- [LiteLLM — Response Headers](https://docs.litellm.ai/docs/proxy/response_headers) (`x-litellm-response-cost`, rate-limit remaining) — per-response cost exists; budget-remaining-USD per task/session from the enforcing ledger does not.
- [LLM Usage & Cost Tracker (MCP, Glama)](https://glama.ai/mcp/servers/zhaoyue722/llm-usage-mcp) and [Agent Budget Guard](https://earezki.com/ai-news/2026-03-02-i-built-an-mcp-server-so-my-ai-agent-can-track-its-own-spending/) — passive MCP meters exist; introspection backed by the guardrail that blocks does not.
