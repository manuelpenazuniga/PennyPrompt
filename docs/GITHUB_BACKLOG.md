# PennyPrompt GitHub Backlog

This backlog is the current issue and release-direction source for the alpha train.
It replaces the original M1-M6 scaffold backlog, which is now historical: the Rust
workspace, proxy, budget ledger, admin plane, CLI, docs, tests, and alpha release
automation all exist.

Current baseline:
- Branch: `main`
- Latest published release: `v0.1.0-alpha.3` published on 2026-06-20 as a GitHub prerelease.
- Capture date: 2026-06-20 after alpha.3 closure and alpha.4 issue creation.
- Active roadmap: `v0.1.0-alpha.4` operator usability.

Source of truth for this backlog:
- GitHub issue `#199`, plus alpha.4 child issues `#200`, `#201`, `#202`, and `#203`.
- Closed alpha.3 release epic `#186`, plus closed release blockers `#183`, `#184`, `#185`, `#189`, `#190`, and `#196`.
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

## Completed Release: `v0.1.0-alpha.3`

`v0.1.0-alpha.3` is published and its release epic is closed.

Closed goals:
- Preserve final release evidence.
- Ship the bounded alpha.3 hardening scope.
- Start the next roadmap from a fresh issue set after `#186` closed.

Completed alpha.3 scope:
- `#183` - `feat(cli): add --help descriptions to every subcommand`
- `#184` - `feat(cost): per-model tokenizer dispatch (Anthropic vs OpenAI families)`
- `#185` - `feat(proxy): structured tracing on request hot path (info spans + per-stage events)`
- `#189` - `security: refresh rustls-webpki and add cargo audit release gate`
- `#190` - `docs(security): align admin plane security contract with implementation`
- `#196` - `release: prepare v0.1.0-alpha.3 gate and notes`
- `#186` - `[Epic] v0.1.0-alpha.3 release scope`

Publication evidence:
- Tag: `v0.1.0-alpha.3`
- Release: https://github.com/manuelpenazuniga/PennyPrompt/releases/tag/v0.1.0-alpha.3
- Release run: `27873967227`
- CI-built artifacts: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`
- Local backfill: `x86_64-apple-darwin`, SHA-256 `582b1ecb273126fe089b57789d54d9e619bbf3382b83c1ed6a1d3c7ee741e6b6`
- `CHECKSUMS.txt` downloaded from GitHub Release and verified locally for all 4 archives.

## Current Release Direction: `v0.1.0-alpha.4`

`v0.1.0-alpha.4` is an operator-usability release. It should turn the now-hardened
local alpha into a more usable daily workflow without expanding the product into
team, dashboard, plugin, or remote-control-plane scope.

Goal:
- Make the local operator path usable beyond hardening.
- Start and stop the service predictably.
- Move `pennyprompt run <agent>` from dry-run planning toward a minimal real local execution flow.
- Add smoke coverage around the published installer path.

Out of scope for alpha.4:
- New providers.
- TUI/dashboard product work.
- Remote signed pricebook feed sync.
- PostgreSQL/team/multi-node mode.
- Plugin system.
- Broad docs/website redesign.
- Stable/non-prerelease promotion.

## Active Issue Set

### Epic

- `#199` - `[Epic] v0.1.0-alpha.4 operator usability scope`

Definition:
- Closes once the alpha.4 child issues are shipped or explicitly deferred, gate evidence is recorded, and the alpha.4 release is published.

### Roadmap / Release Direction

- `#200` - `docs(roadmap): refresh backlog for alpha.4`

Definition:
- Closes when this backlog reflects alpha.4 as the active roadmap and CI passes on the documentation PR.

### Planned Alpha.4 Implementation Scope

- `#201` - `feat(cli): add serve daemon/background mode`
- `#202` - `feat(cli): implement minimal pennyprompt run orchestration`
- `#203` - `test(release): add installer smoke check for latest prerelease`

## Alpha.4 Release Sequence

Current order:

1. [ ] Close `#200` to publish the alpha.4 roadmap baseline.
2. [ ] Close `#201` with a bounded `serve` daemon/background implementation.
3. [ ] Close `#202` with minimal local `pennyprompt run <agent>` orchestration.
4. [ ] Close `#203` with installer smoke coverage for a pinned prerelease.
5. [ ] Create alpha.4 release gate documentation.
6. [ ] Create alpha.4 release notes.
7. [ ] Bump workspace and `penny-cli` versions to `0.1.0-alpha.4`.
8. [ ] Update `CHANGELOG.md`.
9. [ ] Run the standard gate:
    - `cargo fmt --all -- --check`
    - `cargo check --workspace --locked`
    - `cargo test --workspace --locked`
    - `cargo clippy --workspace --all-targets --locked -- -D warnings`
    - `cargo audit --ignore RUSTSEC-2023-0071`
10. [ ] Tag and publish `v0.1.0-alpha.4`.
11. [ ] Verify release artifacts and checksums.
12. [ ] Close `#199` when final evidence lands.

## Alpha.3 Release Sequence

Final order:

1. [x] File and close security/audit blocker (`#189`).
2. [x] Close `#184` tokenizer dispatch.
3. [x] Close `#185` structured proxy tracing.
4. [x] Close `#183` CLI help descriptions.
5. [x] Align canonical admin security docs (`#190`).
6. [x] Bump workspace and `penny-cli` versions to `0.1.0-alpha.3`.
7. [x] Convert `CHANGELOG.md` `[Unreleased]` into `[v0.1.0-alpha.3] - 2026-06-20`.
8. [x] Add `docs/RELEASE_GATE_v0.1.0-alpha.3.md`.
9. [x] Add `docs/release-notes/v0.1.0-alpha.3.md`.
10. [x] Run the standard gate:
    - `cargo fmt --all -- --check`
    - `cargo check --workspace --locked`
    - `cargo test --workspace --locked`
    - `cargo clippy --workspace --all-targets --locked -- -D warnings`
    - `cargo audit --ignore RUSTSEC-2023-0071`
11. [x] Tag and publish `v0.1.0-alpha.3`.
12. [x] Verify release artifacts and checksums.
13. [x] Close `#186` when final evidence PR is merged.

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

Do not pull these into alpha.4:
- TUI/dashboard
- provider #3
- alert webhooks
- CSV/JSON export expansion beyond what alpha.4 implementation needs
- team mode or PostgreSQL
- plugin system
- Grafana/Prometheus/OTLP metrics
- remote pricebook feed
- admin authentication or non-loopback admin exposure

## Historical Backlog Status

The original M1-M6 plan is considered delivered for alpha scope:
- M1 Foundation: delivered.
- M2 Proxy pass-through: delivered.
- M3 Atomic budgets: delivered.
- M4 Streaming and real providers: delivered for alpha.
- M5 Active protection: delivered for alpha.
- M6 Alpha release: delivered through `v0.1.0-alpha.3`.

Future roadmap docs should start from the active alpha.4 operator-usability scope above,
not from the old scaffold issue list.
