# PennyPrompt GitHub Backlog

This backlog translates the project roadmap into GitHub milestones, labels, and issues that can be created automatically.

Source documents:
- `README.md`
- `CLAUDE.md`
- `PennyPrompt-v2.md`

Current repository state:
- The repo is still in specification mode.
- There is no Rust workspace or crate scaffold yet.
- Alpha work should start from foundation, not from providers or UX polish.

## Non-Negotiable Design Constraints

These must stay fixed across all issues:

1. Budget blocks use HTTP `402`, never `429`.
2. `guard` mode is fail-closed if budget or SQLite fails.
3. The core accounting flow is `reserve -> dispatch -> reconcile`.
4. Budget reservation and budget check happen in the same SQLite transaction.
5. Project and session attribution should work without requiring custom headers.
6. Pricebooks are local versioned files, not scraped at runtime.
7. Proxy plane and admin plane stay separate.

## Milestone Plan

### M1 - Foundation

Goal:
- Workspace compiles.
- Config loads with presets and env overrides.
- SQLite schema exists.
- Pricing engine resolves at least six models.

Issues:
- EPIC: M1 Foundation
- Scaffold Cargo workspace and crate layout
- Implement `penny-types` shared domain model
- Implement `penny-config` loader, presets, validation, env overrides
- Implement `penny-store` migrations and repository layer
- Implement `penny-cost` pricing engine and pricebook loader
- Add CI workflow for check, test, clippy, fmt

Acceptance:
- `cargo test --workspace` passes.
- `indie` preset loads correctly.
- Pricebook resolves six or more models.

### M2 - Proxy Pass-Through

Goal:
- Proxy accepts OpenAI-compatible traffic and forwards to a mock provider.
- Requests are persisted.
- Project and session are auto-attributed.

Issues:
- EPIC: M2 Proxy Pass-Through
- Implement `MockProvider` with streaming and non-streaming fixtures
- Implement proxy server with `/v1/chat/completions` and `/v1/models`
- Implement normalization pipeline and request persistence
- Implement project/session auto-attribution
- Add temporary health endpoint

Acceptance:
- A `curl` request against `localhost:8585` returns a mock response.
- Rows are written to `projects`, `sessions`, `requests`, and `request_usage`.

### M3 - Atomic Budgets

Goal:
- Budgets are enforced atomically under concurrency.
- `observe` and `guard` modes behave correctly.
- Budget failures return non-retriable `402`.

Issues:
- EPIC: M3 Atomic Budgets
- Implement `penny-ledger` reserve, reconcile, and release
- Implement `penny-budget` evaluator and mode logic
- Integrate budget enforcement into proxy pipeline
- Seed budgets from config and presets
- Implement `report summary` CLI

Acceptance:
- Over-budget request returns `402`.
- Concurrent requests do not leak over the hard limit.
- Guard mode blocks on ledger or DB failure.

### M4 - Streaming and Real Providers

Goal:
- Anthropic and OpenAI work end-to-end.
- Streaming is forwarded in real time and accounted correctly.
- Admin plane exposes health, reports, budgets, and event SSE.

Issues:
- EPIC: M4 Streaming and Real Providers
- Implement Anthropic adapter
- Implement OpenAI adapter
- Implement streaming pass-through and post-stream accounting
- Implement admin plane endpoints and SSE stream
- Map upstream provider errors cleanly

Acceptance:
- Real agent traffic can run through `localhost:8585`.
- Streaming works without visible artifacts.
- Admin endpoints expose accurate cost data.

### M5 - Active Protection

Goal:
- PennyPrompt detects costly loops, abnormal burn-rate, and repeated failures.
- Estimation and live tailing are usable from the CLI.

Issues:
- EPIC: M5 Active Protection
- Implement `penny-detect` loop detector
- Integrate pause/resume session protection in proxy
- Implement `estimate` CLI and `/admin/estimate`
- Implement `tail` CLI over admin SSE
- Implement `detect status` and `detect resume`

Acceptance:
- Repeated similar requests trigger alert or pause.
- Burn-rate alerts appear in the live tail.
- `estimate` returns cost ranges with budget status.

### M6 - Alpha Release

Goal:
- New user reaches first useful report in less than ten minutes.
- CLI, docs, tests, and release artifacts are good enough for public alpha.

Issues:
- EPIC: M6 Alpha Release
- Finish CLI UX: `init`, `doctor`, `config`, `prices`, `budget`, `report top`
- Write install, quickstart, config, architecture, and pricebook docs
- Add integration and golden test coverage
- Add release automation, install script, and changelog

Acceptance:
- Public alpha can be installed and used quickly.
- Multi-platform release artifacts exist.
- Alpha checklist is green.

## Label Taxonomy

Recommended labels:
- `epic`
- `phase:m1`
- `phase:m2`
- `phase:m3`
- `phase:m4`
- `phase:m5`
- `phase:m6`
- `area:types`
- `area:config`
- `area:store`
- `area:cost`
- `area:providers`
- `area:proxy`
- `area:ledger`
- `area:budget`
- `area:admin`
- `area:detect`
- `area:cli`
- `area:docs`
- `area:release`
- `kind:test`
- `kind:ci`

## Proposed Alpha Issue Set

### M1

1. EPIC: M1 Foundation
2. Scaffold Cargo workspace and crate layout
3. Implement `penny-types` shared domain model
4. Implement `penny-config` loader, presets, validation, env overrides
5. Implement `penny-store` migrations and repository layer
6. Implement `penny-cost` pricing engine and pricebook loader
7. Add CI workflow for check, test, clippy, fmt

### M2

8. EPIC: M2 Proxy Pass-Through
9. Implement `MockProvider` for deterministic integration tests
10. Implement proxy server and OpenAI-compatible endpoints
11. Implement normalization pipeline and SQLite request persistence
12. Implement project and session auto-attribution
13. Add temporary health endpoint

### M3

14. EPIC: M3 Atomic Budgets
15. Implement `penny-ledger` atomic reservation flow
16. Implement `penny-budget` evaluator and observe/guard modes
17. Integrate budget enforcement and structured `402` error bodies
18. Seed budgets from config and presets
19. Implement `report summary` CLI

### M4

20. EPIC: M4 Streaming and Real Providers
21. Implement Anthropic provider adapter
22. Implement OpenAI provider adapter
23. Implement streaming pass-through and reconciliation
24. Implement admin plane endpoints and event SSE
25. Map upstream provider errors and incomplete streams

### M5

26. EPIC: M5 Active Protection
27. Implement `penny-detect` heuristics and pause lifecycle
28. Integrate loop protection into proxy request flow
29. Implement `estimate` CLI and admin estimate API
30. Implement live `tail` CLI over SSE
31. Implement `detect status` and `detect resume`

### M6

32. EPIC: M6 Alpha Release
33. Finish operator-focused CLI commands and setup wizard
34. Write alpha docs set
35. Add integration suite, golden tests, and manual alpha checklist
36. Add release automation, install script, and changelog

## Post-Alpha Parking Lot

Do not create these until alpha scope is stable:
- `pennyprompt run <agent>`
- Payload cleanup
- TUI dashboard
- Provider #3
- Alert webhooks
- CSV and JSON export
- Team mode or PostgreSQL
- Plugin system
- Grafana or Prometheus metrics

## Technical Review Follow-Ups

Review source:
- Gemini Code Assist comments on merged PRs `#38` through `#51` (review pass on 2026-04-12).

Actioned as dedicated issues:
- [#45](https://github.com/manuelpenazuniga/PennyPrompt/issues/45) — Harden money representation before M3/M4.
- [#52](https://github.com/manuelpenazuniga/PennyPrompt/issues/52) — Harden proxy health and error surfaces before M3 integration.

Mapped to existing planned issues (no new issue needed):
- Streaming memory/latency behavior and reconciliation gaps → [#24](https://github.com/manuelpenazuniga/PennyPrompt/issues/24).
- Upstream error mapping and incomplete stream handling → [#26](https://github.com/manuelpenazuniga/PennyPrompt/issues/26).
- Budget-enforcement error contracts and structured 402 responses → [#18](https://github.com/manuelpenazuniga/PennyPrompt/issues/18).

Technical annotations to revisit (future hardening, currently no dedicated issue):
- `penny-store`: revisit pool sizing/query patterns and slug collision risk once M3 load/concurrency tests exist.
- `penny-config`: improve cross-platform path handling (`HOME`/tilde resolution) before alpha packaging.
- `penny-cost`: revisit historical pricing query ergonomics and non-blocking import path as performance tuning.

## Automation

Use [scripts/create_github_backlog.sh](/Volumes/MacMiniExt/dev/OpenSource%20Projects/PennyPrompt/PennyPrompt/scripts/create_github_backlog.sh) to create the milestones, labels, and alpha issues once `gh` is authenticated.
