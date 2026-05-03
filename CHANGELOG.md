# Changelog

All notable changes to PennyPrompt are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- Upcoming changes are tracked via issue-first workflow and merged through PRs.
- Alpha.2 release gate artifacts: targeted checklist, linked release process gate, and release notes document.

### Changed
- Observability startup precedence is now explicit: CLI flags (`--log-filter`, `--json-log`) override environment (`PENNY_LOG`/`RUST_LOG`, `PENNY_OBSERVE_JSON`), which still override built-in defaults. Backward-compatibility note: workflows relying on env vars to force logging behavior should stop passing conflicting CLI flags.

## [v0.1.0-alpha.2] - 2026-04-30 (release publication pending under #144)

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
