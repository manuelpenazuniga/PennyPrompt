# Release Gate: v0.1.0-alpha.2

Objective gate for publishing `v0.1.0-alpha.2` after P0 onboarding/contract fixes.

## 1. Scope Lock (P0 Closure)

- [ ] `#129` merged: `serve` command starts proxy + admin lifecycle from `penny-cli`.
- [ ] `#130` merged: README/operator docs aligned with actual command surface.
- [ ] `#131` merged: pricebooks refreshed and preset defaults resolvable.

## 2. Workspace Quality Gate

Run on release candidate branch (or `main` immediately before tagging):

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo check --workspace --locked`
- [ ] `cargo test --workspace --locked`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`

## 3. Runtime Acceptance Smoke Tests

Use a fresh shell and clean local config where possible.

- [ ] `penny-cli init --preset indie` succeeds.
- [ ] `penny-cli prices update` succeeds and reports validated default models.
- [ ] `penny-cli prices show --limit 20` shows active entries with expected models (`claude-sonnet-4-6`, `claude-haiku-4`, `gpt-4.1` at minimum).
- [ ] `penny-cli serve --mock --admin-bind 127.0.0.1:8586` starts both planes.
- [ ] `curl -fsS http://127.0.0.1:8586/admin/health` returns `200`.
- [ ] Proxy request path works against `http://127.0.0.1:8585/v1/chat/completions`.
- [ ] `penny-cli tail` can consume admin events when admin is bound on TCP.

## 4. Docs Consistency Gate

- [ ] `README.md` quickstart and CLI reference match the current binary.
- [ ] `docs/QUICKSTART.md` includes real `serve` flow.
- [ ] `docs/CONFIG-REFERENCE.md` reflects current admin bind semantics.
- [ ] `docs/LIMITATIONS.md` reflects current deferred behavior.
- [ ] `CHANGELOG.md` contains `v0.1.0-alpha.2` notes before tag push.

## 5. CI and Release Workflow Gate

- [ ] Latest PR to `main` has CI check `Check, Test, Clippy, Fmt` green.
- [ ] Release workflow (`.github/workflows/release.yml`) succeeded for all targets:
  - [ ] `x86_64-unknown-linux-gnu`
  - [ ] `aarch64-unknown-linux-gnu`
  - [ ] `x86_64-apple-darwin`
  - [ ] `aarch64-apple-darwin`

## 6. Artifact Verification Gate

- [ ] Release has 4 `.tar.gz` artifacts and 4 `.sha256` files.
- [ ] `CHECKSUMS.txt` present and complete.
- [ ] At least one target checksum verified locally (`shasum -a 256 -c ...`).

## 7. Release Notes Gate

Before publishing, prepare notes from `docs/release-notes/v0.1.0-alpha.2.md`.

- [ ] Notes include explicit references to resolved P0 issues: `#129`, `#130`, `#131`.
- [ ] Notes include link to known limitations: `docs/LIMITATIONS.md`.
- [ ] Notes include validation statement (`fmt/check/test/clippy` passed).

## 8. Publish Decision

- [ ] All required boxes above are checked.
- [ ] Remaining non-blocking items are tracked in open issues.
- [ ] Tag pushed as `v0.1.0-alpha.2`.
