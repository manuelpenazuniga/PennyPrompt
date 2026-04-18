# Alpha Manual Checklist

Use this checklist before cutting an alpha release candidate.

## Environment

- [ ] Fresh clone in a clean directory.
- [ ] Rust toolchain available (`rustc`, `cargo`).
- [ ] No pre-existing PennyPrompt user config (or intentionally reset).

## Build and Test Gate

- [ ] `cargo fmt --all` clean.
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings` passes.
- [ ] `cargo test --workspace --locked` passes.

## First-Time User Path

- [ ] Build CLI:
  - [ ] `cargo build --release -p penny-cli`
- [ ] Initialize config:
  - [ ] `penny-cli init --preset indie`
- [ ] Verify doctor output is actionable:
  - [ ] `penny-cli doctor`
- [ ] Verify effective config visibility:
  - [ ] `penny-cli config --json`

## Pricing and Budget Ops

- [ ] Import pricebook:
  - [ ] `penny-cli prices update`
- [ ] List active pricebook entries:
  - [ ] `penny-cli prices show --limit 20`
- [ ] List budgets:
  - [ ] `penny-cli budget list`
- [ ] Set and reset a test budget:
  - [ ] `penny-cli budget set ...`
  - [ ] `penny-cli budget reset ...`

## Reporting and Estimation

- [ ] Estimate command returns range + confidence:
  - [ ] `penny-cli estimate --model claude-sonnet-4-6 --context-files "src/**/*.rs"`
- [ ] Summary report runs:
  - [ ] `penny-cli report summary --since 1d`
- [ ] Top report runs:
  - [ ] `penny-cli report top --limit 20`

## Detection Control Plane

- [ ] Detect status returns operator-readable state:
  - [ ] `penny-cli detect status`
- [ ] Detect resume command responds correctly for paused session:
  - [ ] `penny-cli detect resume <session_id>`

## Event Streaming

- [ ] Tail attaches to admin SSE and prints near-real-time events:
  - [ ] `penny-cli tail --admin-url http://127.0.0.1:8586`
- [ ] `NO_COLOR=1` disables ANSI formatting in tail output.

## Documentation Gate

- [ ] `docs/INSTALL.md` is accurate.
- [ ] `docs/QUICKSTART.md` is accurate.
- [ ] `docs/CONFIG-REFERENCE.md` matches implemented fields.
- [ ] `docs/ARCHITECTURE.md` matches current crate boundaries.
- [ ] `docs/PRICEBOOK.md` matches current import/update flow.
- [ ] `docs/LIMITATIONS.md` reflects known alpha constraints.

## Release Readiness Decision

- [ ] All critical checklist items pass.
- [ ] Remaining gaps are explicitly tracked in GitHub issues.
- [ ] RC can be shared with alpha users.
