# Changelog

All notable changes to PennyPrompt are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

No unreleased changes.

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
