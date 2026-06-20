# PennyPrompt GitHub Backlog

This backlog is the current issue and release-direction source for the alpha train.
It replaces the original M1-M6 scaffold backlog, which is now historical: the Rust
workspace, proxy, budget ledger, admin plane, CLI, docs, tests, and alpha.2 release
automation all exist.

Current baseline:
- Branch: `main`
- Latest published release: `v0.1.0-alpha.2` published on 2026-05-15 as a GitHub prerelease.
- Open PRs at the 2026-06-19 audit: none.
- Active roadmap: `v0.1.0-alpha.3` hardening release.

Source of truth for this backlog:
- GitHub issues `#183`, `#184`, `#185`, and `#186`.
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
- Close the implementation hardening issues.
- Fix release blockers discovered during the 2026-06-19 audit.
- Cut and publish `v0.1.0-alpha.3`.

### Required Child Issues

1. `#184` - `feat(cost): per-model tokenizer dispatch (Anthropic vs OpenAI families)`

Why it matters:
- `penny-cost` currently estimates all textual input with `cl100k_base`.
- Anthropic-family estimates can be materially low.
- Low estimates bias `estimate` output and budget reservations.

Acceptance:
- `TokenizerKind` or equivalent exists.
- Active shipped pricebook models resolve to a non-heuristic tokenizer class.
- Unknown models use a safe heuristic fallback.
- Tests cover OpenAI, Anthropic, and fallback behavior.
- `docs/TECHNICAL_NOTES.md` records the Anthropic tokenizer decision.

2. `#185` - `feat(proxy): structured tracing on request hot path (info spans + per-stage events)`

Why it matters:
- Runtime accounting exists, but the proxy hot path has too little structured log evidence.
- Operators need request, model, provider, cost, latency, and outcome fields in JSON logs.

Acceptance:
- Successful requests emit structured `received`, `completed`, and `reconciled` events.
- Error paths emit one structured error event with request context.
- Proxy tests assert the structured field set.
- Manual `--json-log` evidence is attached to the PR.

3. `#183` - `feat(cli): add --help descriptions to every subcommand`

Why it matters:
- First-run CLI help is still too bare for alpha users.
- This is a low-risk UX improvement that makes the release more credible.

Acceptance:
- Root help and nested help groups show useful descriptions.
- Golden/snapshot tests cover root and at least one nested group.
- No command behavior changes.

## New Release Blockers From 2026-06-19 Audit

These should be filed as GitHub issues before starting the alpha.3 implementation queue.

### Security: Refresh TLS Dependency / Add Audit Gate

Proposed issue title:
- `security: refresh rustls-webpki and add cargo audit release gate`

Why it matters:
- `Cargo.lock` currently resolves `rustls-webpki 0.103.11`.
- RustSec advisories published in April 2026 identify affected versions fixed in newer `rustls-webpki` releases.
- The project uses `reqwest` with `rustls-tls`, so TLS verification dependencies are release-critical.

Required work:
- Update the lockfile so `rustls-webpki` resolves to a fixed version.
- Install or wire `cargo-audit` in the release gate path.
- Document audit output in the alpha.3 gate.

Acceptance:
- `cargo audit` passes or all advisories are explicitly documented as non-applicable.
- `cargo test --workspace --locked` passes after the lockfile update.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` passes.

### Docs: Admin Security Contract Alignment

Proposed issue title:
- `docs(security): align admin plane security contract with implementation`

Why it matters:
- The admin plane currently exposes local operational endpoints without token authentication.
- The valid alpha contract is loopback TCP or Unix socket, not authenticated network admin.
- Release docs should not imply authentication that does not exist.

Required work:
- Ensure canonical docs describe admin as local-only unless auth is implemented.
- Do not add authentication to alpha.3 unless the release scope is intentionally expanded.
- Keep `docs/CONFIG-REFERENCE.md`, `docs/LIMITATIONS.md`, and release notes consistent.

Acceptance:
- Canonical docs do not claim admin token auth.
- Admin bind examples stay on `127.0.0.1` or Unix socket paths.
- Alpha.3 release notes include the local-admin limitation.

## Alpha.3 Release Sequence

Recommended order:

1. File the two new blocker issues listed above.
2. Patch TLS dependency and add audit-gate evidence.
3. Close `#184` tokenizer dispatch.
4. Close `#185` structured proxy tracing.
5. Close `#183` CLI help descriptions.
6. Align canonical admin security docs.
7. Bump workspace and `penny-cli` versions to `0.1.0-alpha.3`.
8. Convert `CHANGELOG.md` `[Unreleased]` into `[v0.1.0-alpha.3] - YYYY-MM-DD`.
9. Add `docs/RELEASE_GATE_v0.1.0-alpha.3.md`.
10. Add `docs/release-notes/v0.1.0-alpha.3.md`.
11. Run the standard gate:
    - `cargo fmt --all -- --check`
    - `cargo check --workspace --locked`
    - `cargo test --workspace --locked`
    - `cargo clippy --workspace --all-targets --locked -- -D warnings`
    - `cargo audit`
12. Tag and publish `v0.1.0-alpha.3`.
13. Verify release artifacts and checksums.

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
