# PennyPrompt GitHub Backlog

This backlog is the current issue and release-direction source for the alpha train.
It replaces the original M1-M6 scaffold backlog, which is now historical: the Rust
workspace, proxy, budget ledger, admin plane, CLI, docs, tests, and alpha.2 release
automation all exist.

Current baseline:
- Branch: `main`
- Latest published release: `v0.1.0-alpha.2` published on 2026-05-15 as a GitHub prerelease.
- Capture date: 2026-06-20 after `#183`, `#184`, `#185`, `#189`, and `#190` closed.
- Active roadmap: final `v0.1.0-alpha.3` release-prep and publish gate.

Source of truth for this backlog:
- GitHub issues `#186` and `#196`, plus closed release blockers `#183`, `#184`, `#185`, `#189`, and `#190`.
- `docs/status-2026-05-07.md`.
- `docs/CONFIG-REFERENCE.md`.
- `docs/RELEASE.md`.
- `docs/TECHNICAL_NOTES.md`.
- Implementation reality in `crates/`.

Operator-facing marketing copy is not used as normative release evidence here.
Local `docs/status-*.md` snapshots are working notes only; any decision needed by
public roadmap or release gates must be repeated in tracked docs or GitHub issues.

## Non-Negotiable Design Constraints

These constraints remain fixed unless a dedicated architecture decision changes them:

1. Budget blocks use HTTP `402`, never `429`.
2. `guard` mode is fail-closed if budget or SQLite accounting fails.
3. The core accounting flow remains `reserve -> dispatch -> reconcile`.
4. Budget reservation and budget check happen in one SQLite transaction.
5. Provider-reported usage wins over estimates during reconciliation.
6. Project and session attribution should work without custom headers.
7. Pricebooks are local versioned files for the current alpha train.
8. Proxy plane and admin plane stay separate.
9. Admin plane is local-control-plane scope; TCP admin exposure must stay loopback-only unless authentication is implemented and tested.
10. Alpha releases are prereleases until a stable cut intentionally changes release maturity.

## Current Release Direction: `v0.1.0-alpha.3`

`v0.1.0-alpha.3` is a hardening release, not a feature release.

Goal:
- Make the already-shipped alpha more correct, auditable, secure, and releasable.
- Avoid new product surface area unless it is required to close a release blocker.

Out of scope for alpha.3:
- `serve` daemon/background mode.
- Full `pennyprompt run <agent>` orchestration.
- New providers.
- TUI/dashboard product work.
- Remote signed pricebook feed sync.
- PostgreSQL/team/multi-node mode.
- Broad docs/website redesign.

## Active Issue Set

### Epic

- `#186` - `[Epic] v0.1.0-alpha.3 release scope`

Definition:
- Cut and publish `v0.1.0-alpha.3`.
- Close once version bump, changelog, release gate, release notes, tag, GitHub Release, and artifact verification are complete.

### Release Prep

- `#196` - `release: prepare v0.1.0-alpha.3 gate and notes`

Definition:
- Prepare version bump, changelog, release gate, release notes, and release-process docs.
- Close through the release-prep PR before tagging.

### Closed Alpha.3 Implementation Scope

- `#183` - `feat(cli): add --help descriptions to every subcommand`
- `#184` - `feat(cost): per-model tokenizer dispatch (Anthropic vs OpenAI families)`
- `#185` - `feat(proxy): structured tracing on request hot path (info spans + per-stage events)`
- `#189` - `security: refresh rustls-webpki and add cargo audit release gate`
- `#190` - `docs(security): align admin plane security contract with implementation`

### Remaining Release Work After `#196`

- Merge the release-prep PR after CI confirms the full gate in a loopback-capable environment.
- Tag `v0.1.0-alpha.3` from updated `main`.
- Publish the GitHub Release through `.github/workflows/release.yml`.
- Verify artifacts and checksums.
- Close `#186`.

## Alpha.3 Release Sequence

Current order:

1. [x] File and close security/audit blocker (`#189`).
2. [x] Close `#184` tokenizer dispatch.
3. [x] Close `#185` structured proxy tracing.
4. [x] Close `#183` CLI help descriptions.
5. [x] Align canonical admin security docs (`#190`).
6. [x] Bump workspace and `penny-cli` versions to `0.1.0-alpha.3`.
7. [x] Convert `CHANGELOG.md` `[Unreleased]` into `[v0.1.0-alpha.3] - 2026-06-20`.
8. [x] Add `docs/RELEASE_GATE_v0.1.0-alpha.3.md`.
9. [x] Add `docs/release-notes/v0.1.0-alpha.3.md`.
10. [ ] Run the standard gate:
    - `cargo fmt --all -- --check`
    - `cargo check --workspace --locked`
    - `cargo test --workspace --locked`
    - `cargo clippy --workspace --all-targets --locked -- -D warnings`
    - `cargo audit --ignore RUSTSEC-2023-0071`
11. [ ] Tag and publish `v0.1.0-alpha.3`.
12. [ ] Verify release artifacts and checksums.
13. [ ] Close `#186`.

## Release Gate Notes

Known local verification caveats:
- Tests that bind loopback ports may fail inside restricted sandboxes. They pass when loopback binding is permitted.
- Config tests can be affected by a real user config if `HOME` is not isolated. Release gate commands should use a clean HOME or CI runner environment.

Recommended local command shape:

```bash
HOME="$(mktemp -d)" \
RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" \
cargo test --workspace --locked
```

Adjust `RUSTUP_HOME` and `CARGO_HOME` to existing toolchain paths when running offline.

## Deferred Parking Lot

Do not pull these into alpha.3:
- daemon/background mode
- full `run <agent>` orchestration
- payload cleanup expansion beyond current behavior
- TUI/dashboard
- provider #3
- alert webhooks
- CSV/JSON export expansion
- team mode or PostgreSQL
- plugin system
- Grafana/Prometheus/OTLP metrics
- remote pricebook feed

## Historical Backlog Status

The original M1-M6 plan is considered delivered for alpha scope:
- M1 Foundation: delivered.
- M2 Proxy pass-through: delivered.
- M3 Atomic budgets: delivered.
- M4 Streaming and real providers: delivered for alpha.
- M5 Active protection: delivered for alpha.
- M6 Alpha release: delivered through `v0.1.0-alpha.2`.

Future roadmap docs should start from the active alpha.3 hardening scope above, not from the old scaffold issue list.
