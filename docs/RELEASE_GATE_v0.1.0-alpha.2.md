# Release Gate: v0.1.0-alpha.2

Objective gate for publishing `v0.1.0-alpha.2` after P0 onboarding/contract fixes.

Execution note (2026-04-30):
- Release run: https://github.com/manuelpenazuniga/PennyPrompt/actions/runs/25174516142
- Current blocker: `Build x86_64-apple-darwin` is still `queued`; remaining targets completed successfully.

## 1. Scope Lock (P0 Closure)

- [x] `#129` merged: `serve` command starts proxy + admin lifecycle from `penny-cli`.
- [x] `#130` merged: README/operator docs aligned with actual command surface.
- [x] `#131` merged: pricebooks refreshed and preset defaults resolvable.

## 2. Workspace Quality Gate

Run on release candidate branch (or `main` immediately before tagging):

- [x] `cargo fmt --all -- --check`
- [x] `cargo check --workspace --locked`
- [x] `cargo test --workspace --locked`
- [x] `cargo clippy --workspace --all-targets --locked -- -D warnings`

Evidence:
- Executed locally on 2026-04-30 from `chore/postalpha-issue-144-release-gate-evidence`.
- All commands completed successfully.

## 3. Runtime Acceptance Smoke Tests

Use a fresh shell and clean local config where possible.

- [x] `penny-cli init --preset indie` succeeds.
- [x] `penny-cli prices update` succeeds and reports validated default models.
- [x] `penny-cli prices show --limit 20` shows active entries with expected models (`claude-sonnet-4-6`, `claude-haiku-4`, `gpt-4.1` at minimum).
- [x] `penny-cli serve --mock --admin-bind 127.0.0.1:8586` starts both planes.
- [x] `curl -fsS http://127.0.0.1:8586/admin/health` returns `200`.
- [x] Proxy request path works against `http://127.0.0.1:8585/v1/chat/completions`.
- [x] `penny-cli tail --admin-url http://127.0.0.1:8586 --once --limit 20` can consume admin events when admin is bound on TCP.

Evidence:
- Executed with isolated HOME + explicit DB path:
  - `HOME=/var/folders/.../tmp.eLQl7iJO39`
  - `--database /var/folders/.../tmp.eLQl7iJO39/penny.db`
- `admin/health` response: `{\"status\":\"ok\",...}`
- proxy completion response returned mock payload with usage block.
- `tail --once` emitted request event line.

## 4. Docs Consistency Gate

- [x] `README.md` quickstart and CLI reference match the current binary.
- [x] `docs/QUICKSTART.md` includes real `serve` flow.
- [x] `docs/CONFIG-REFERENCE.md` reflects current admin bind semantics.
- [x] `docs/LIMITATIONS.md` reflects current deferred behavior.
- [x] `CHANGELOG.md` contains `v0.1.0-alpha.2` notes before tag push.

Evidence:
- `#145` merged: https://github.com/manuelpenazuniga/PennyPrompt/pull/157
- `#143` merged: https://github.com/manuelpenazuniga/PennyPrompt/pull/156

## 5. CI and Release Workflow Gate

- [x] Latest PR to `main` has CI check `Check, Test, Clippy, Fmt` green.
- [ ] Release workflow (`.github/workflows/release.yml`) succeeded for all targets:
  - [x] `x86_64-unknown-linux-gnu`
  - [x] `aarch64-unknown-linux-gnu`
  - [ ] `x86_64-apple-darwin` (pending runner allocation; job still queued)
  - [x] `aarch64-apple-darwin`

Evidence:
- PR CI (latest merged PR #157):
  - https://github.com/manuelpenazuniga/PennyPrompt/actions/runs/25172036308/job/73793828345
  - https://github.com/manuelpenazuniga/PennyPrompt/actions/runs/25172021516/job/73793778027
- Release run:
  - https://github.com/manuelpenazuniga/PennyPrompt/actions/runs/25174516142
  - job `aarch64-apple-darwin`: https://github.com/manuelpenazuniga/PennyPrompt/actions/runs/25174516142/job/73802809753
  - job `aarch64-unknown-linux-gnu`: https://github.com/manuelpenazuniga/PennyPrompt/actions/runs/25174516142/job/73802809839
  - job `x86_64-unknown-linux-gnu`: https://github.com/manuelpenazuniga/PennyPrompt/actions/runs/25174516142/job/73802809900
  - job `x86_64-apple-darwin` (queued): https://github.com/manuelpenazuniga/PennyPrompt/actions/runs/25174516142/job/73802809807

## 6. Artifact Verification Gate

- [ ] Release has 4 `.tar.gz` artifacts and 4 `.sha256` files.
- [ ] `CHECKSUMS.txt` present and complete.
- [ ] At least one target checksum verified locally (`shasum -a 256 -c ...`).

Status:
- Blocked until release workflow fully completes and publishes release assets.

## 7. Release Notes Gate

Before publishing, prepare notes from `docs/release-notes/v0.1.0-alpha.2.md`.

- [x] Notes include explicit references to resolved P0 issues: `#129`, `#130`, `#131`.
- [x] Notes include link to known limitations: `docs/LIMITATIONS.md`.
- [x] Notes include validation statement (`fmt/check/test/clippy` passed).

Evidence:
- Notes finalized in #145: https://github.com/manuelpenazuniga/PennyPrompt/pull/157
- Notes file: `docs/release-notes/v0.1.0-alpha.2.md`

## 8. Publish Decision

- [ ] All required boxes above are checked.
- [x] Remaining non-blocking items are tracked in open issues.
- [x] Tag pushed as `v0.1.0-alpha.2`.

Status:
- Gate execution is in progress. Final completion depends on `x86_64-apple-darwin` release job leaving queued state and successful publish of release assets.
