# Changelog

All notable changes to PennyPrompt are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [v0.1.0-alpha.5] - 2026-07-12

Compatibility & cost accuracy (Phase A). Makes the headline promise — "works with
your agent, zero changes" — literally true for Anthropic-native agents, and makes
reported cost correct on the flagship cache-heavy coding-agent workload.

### Added
- Native Anthropic Messages ingress: `POST /v1/messages` accepts native Anthropic-format requests (preserving `system`, `tools`, and `tool_use`/`tool_result` content blocks) and returns the native Anthropic response shape. Streaming forwards the native Anthropic SSE event sequence unmodified while accumulating usage for reconciliation. Anthropic-native agents (OpenClaw, claw-code, Claude-family SDKs) connect with `ANTHROPIC_BASE_URL=http://localhost:8585` and zero translation. The shared budget/attribution/detect pipeline is factored so both ingress formats run through one core (`#207`).
- Prompt-cache cost accounting: cache-read and cache-write tokens are read from Anthropic (`cache_read_input_tokens`/`cache_creation_input_tokens`, non-stream and streaming `message_start`) and OpenAI (`prompt_tokens_details.cached_tokens`) usage and priced with dedicated cache-read/cache-write rates. `report summary` (CLI and admin) breaks usage into fresh input / cache read / cache write / output; totals reconcile to the ledger. Models without cache rates bill cache tokens at the input rate (logged at debug), never dropped (`#208`).
- Inbound concurrency limit `[server].max_inflight_requests` (default 64) and configurable upstream timeout `[server].upstream_timeout_ms` (default 60000). A provider that does not respond in time yields HTTP 504 with a `provider_timeout` event and releases its reservation (net-zero charge), preserving reserve → dispatch → reconcile (`#209`).
- Strategic audit `docs/STRATEGY-AUDIT-2026-07-05.md` (rev. 1.1): progress audit, security/scalability/functional findings, competitive analysis, consolidated differentiators, the cost-aware loop and invoice-parity direction, and the go-to-market track; forward roadmap tracked as GitHub epics `#225`-`#228` and child issues `#207`-`#224`, `#230`-`#234`.

### Changed
- The shipped binary is now `pennyprompt` (was `penny-cli`); the crate/package name is unchanged. A `penny-cli` compatibility symlink and one-line deprecation notice ship for one train (removed in beta.1); the installer and release artifacts use `pennyprompt-*` naming, with a legacy-asset fallback so pinned pre-rename tags stay installable (`#236`).
- Additive migrations `0010`/`0011`: nullable pricebook cache-rate columns and per-request cache-token columns; `input_tokens` now records fresh (non-cached) input. `prices/anthropic.toml` and `prices/openai.toml` carry cache rates for all listed models (`#208`).
- README compatibility table and `docs/LIMITATIONS.md` flipped to verified now that native Anthropic ingress and cache accounting have landed; README roadmap points at `docs/GITHUB_BACKLOG.md` as the single source of truth (`#210`).
- `docs/GITHUB_BACKLOG.md` rewritten around the forward roadmap with a consolidated differentiators section, the `phase:m7`-`m10` label scheme, and an honesty ledger of known gaps. README revamped: comparison table vs LiteLLM/Portkey/Helicone/OpenRouter, real install command, CI/release badges, phased roadmap table, privacy section, and contributor on-ramp.
- `docs/CONFIG-REFERENCE.md` documents the new `[server].max_inflight_requests` and `[server].upstream_timeout_ms` fields (`#209`).

## [v0.1.0-alpha.3] - 2026-06-20

### Added
- CLI help text now includes descriptions for root and nested subcommands (`#183`).
- Proxy hot-path tracing now emits structured `proxy.request`, `proxy.budget`, `proxy.completion`, `proxy.ledger`, and `proxy.error` events for JSON-log operators (`#185`).
- CI now runs `cargo audit` as part of the standard gate, with the non-applicable `rsa` advisory documented inline (`#189`).
- Alpha.3 release gate and release notes documents for the final pre-tag checklist (`#196`).

### Changed
- `penny-cost` now dispatches token estimation by model family instead of using one OpenAI tokenizer path for every model (`#184`).
- TLS verification dependencies were refreshed, including `rustls-webpki`, before the alpha.3 cut (`#189`).
- Admin-plane security docs now describe the actual alpha contract: local-only Unix socket or loopback TCP, with no bearer/admin-token auth claim (`#190`).
- Observability startup precedence is now explicit: CLI flags (`--log-filter`, `--json-log`) override environment (`PENNY_LOG`/`RUST_LOG`, `PENNY_OBSERVE_JSON`), which still override built-in defaults. Backward-compatibility note: workflows relying on env vars to force logging behavior should stop passing conflicting CLI flags.

## [v0.1.0-alpha.2] - 2026-04-30

### Added
- `penny-cli serve` runtime lifecycle with proxy/admin startup and graceful shutdown flow (`#129`).
- Operator/docs command-surface alignment for serve/admin/tail/detect semantics (`#130`).
- Refreshed local pricebooks with default-model resolvability guardrail (`#131`).
- Tracing bootstrap via `penny-observe` integrated into CLI startup path (`#134`).
- Release gate and notes artifacts for auditable alpha.2 publication workflow (`#132`).

### Changed
- Release/install documentation now avoids unverifiable artifact publication claims and links to explicit reconciliation notes (`#143`).

## [v0.1.0-alpha.1] - 2026-04-18 (internal milestone cut; public GitHub Release artifacts not yet reconciled)

### Added
- Core M1-M5 foundation: config loading, store/migrations, pricing engine, proxy plane, atomic budget/ledger flow, detect heuristics, and operator CLI commands.
- M6 docs set: install, quickstart, config reference, architecture, pricebook, and limitations.
- Integration and golden coverage for admin, proxy, ledger, and CLI event formatting.
- Release automation workflow for macOS/Linux on x86_64 and arm64 with per-artifact SHA-256 checksums.
- `scripts/install.sh` bootstrap installer for `curl | sh` installs from GitHub Releases.
