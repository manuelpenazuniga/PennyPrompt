# Release Gate: v0.1.0-alpha.2

Objective gate for publishing `v0.1.0-alpha.2` after P0 onboarding/contract fixes.

Execution note (updated 2026-05-15):
- Release run: https://github.com/manuelpenazuniga/PennyPrompt/actions/runs/25174516142
- Three of four matrix targets completed successfully in attempt-3.
- `x86_64-apple-darwin` (`macos-13`) stayed queued indefinitely (`run_attempt=3`, `runner_id=null`) and was bypassed by the partial-success publish path landed in #176.
- The Intel-Mac artifact was backfilled locally on Apple Silicon from the same tag commit (`41d662c`) and uploaded via `gh release upload` (tracked in #181).
- Result: GitHub Release `v0.1.0-alpha.2` now carries 4 `.tar.gz` archives, 4 `.sha256` files, and an aggregated `CHECKSUMS.txt`.
- The release remains marked as `prerelease: true` per `release.yml` policy (no GitHub `Latest` badge is expected on alpha cuts).

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

Reference command sequence (copy/paste):

```bash
# terminal A
penny-cli serve --mock --admin-bind 127.0.0.1:8586

# terminal B
curl -fsS http://127.0.0.1:8586/admin/health
penny-cli tail --admin-url http://127.0.0.1:8586 --once --limit 20
```

Non-default scenario:
- If admin is started on a different TCP bind (for example `--admin-bind 127.0.0.1:9595`),
  every `tail` / `detect` command must pass the matching URL (`--admin-url http://127.0.0.1:9595`).

- [x] `penny-cli init --preset indie` succeeds.
- [x] `penny-cli prices update` succeeds and reports validated default models.
- [x] `penny-cli prices show --limit 20` shows active entries with expected models (`claude-opus-4-7`, `claude-sonnet-4-6`, `claude-haiku-4-5`, `gpt-4.1` at minimum).
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
- [x] Release workflow (`.github/workflows/release.yml`) produced artifacts for all four targets (3 via CI, 1 via local Apple Silicon backfill, all uploaded to the same GitHub Release):
  - [x] `x86_64-unknown-linux-gnu` (CI, `ubuntu-24.04`)
  - [x] `aarch64-unknown-linux-gnu` (CI, `ubuntu-24.04-arm`)
  - [x] `x86_64-apple-darwin` (local Apple Silicon backfill from tag commit `41d662c`; tracked in #181)
  - [x] `aarch64-apple-darwin` (CI, `macos-14`)

Evidence:
- PR CI (latest merged PR #168):
  - https://github.com/manuelpenazuniga/PennyPrompt/actions/runs/25528067731/job/74928101251
  - https://github.com/manuelpenazuniga/PennyPrompt/actions/runs/25528055962/job/74928067956
- Release run:
  - https://github.com/manuelpenazuniga/PennyPrompt/actions/runs/25174516142
  - attempt-3 job `aarch64-apple-darwin` (success): https://github.com/manuelpenazuniga/PennyPrompt/actions/runs/25174516142/job/74928756279
  - attempt-3 job `aarch64-unknown-linux-gnu` (success): https://github.com/manuelpenazuniga/PennyPrompt/actions/runs/25174516142/job/74928756259
  - attempt-3 job `x86_64-unknown-linux-gnu` (success): https://github.com/manuelpenazuniga/PennyPrompt/actions/runs/25174516142/job/74928756014
  - attempt-3 job `x86_64-apple-darwin` (queued/bypassed via #176): https://github.com/manuelpenazuniga/PennyPrompt/actions/runs/25174516142/job/74928756062
- Local backfill (Intel-Mac):
  - Built from tag commit `41d662c` with `cargo build --release -p penny-cli --target x86_64-apple-darwin --locked`.
  - `file penny-cli` reports `Mach-O 64-bit executable x86_64`.
  - Published SHA-256: `8a02e74fbd7d89730dbb145769d3b4fb9da4650ed8d71c58e728e14fa06d6c6e`.
  - Provenance is intentionally distinct from the CI-built artifacts (different toolchain host, different SDK timestamps); checksum integrity remains verifiable end-to-end.

## 6. Artifact Verification Gate

- [x] Release has 4 `.tar.gz` artifacts and 4 `.sha256` files.
- [x] `CHECKSUMS.txt` present and complete (4 entries, alphabetical-by-filename, regenerated after the local backfill).
- [x] At least one target checksum verified locally with a concrete command (Intel-Mac artifact verified end-to-end after upload).

Reference checksum commands (macOS/Linux):

```bash
# from a clean temp dir
gh release download v0.1.0-alpha.2 \
  --repo manuelpenazuniga/PennyPrompt \
  --pattern 'penny-cli-v0.1.0-alpha.2-x86_64-unknown-linux-gnu.tar.gz' \
  --pattern 'penny-cli-v0.1.0-alpha.2-x86_64-unknown-linux-gnu.sha256'

shasum -a 256 -c penny-cli-v0.1.0-alpha.2-x86_64-unknown-linux-gnu.sha256
# fallback when shasum is unavailable:
# sha256sum -c penny-cli-v0.1.0-alpha.2-x86_64-unknown-linux-gnu.sha256
```

Status:
- Cleared on 2026-05-15. `gh release view v0.1.0-alpha.2` returns the published Release with all 4 archives, 4 `.sha256` files, and aggregated `CHECKSUMS.txt`.

## 7. Release Notes Gate

Before publishing, prepare notes from `docs/release-notes/v0.1.0-alpha.2.md`.

- [x] Notes include explicit references to resolved P0 issues: `#129`, `#130`, `#131`.
- [x] Notes include link to known limitations: `docs/LIMITATIONS.md`.
- [x] Notes include validation statement (`fmt/check/test/clippy` passed).

Evidence:
- Notes finalized in #145: https://github.com/manuelpenazuniga/PennyPrompt/pull/157
- Notes file: `docs/release-notes/v0.1.0-alpha.2.md`

## 8. Publish Decision

- [x] All required boxes above are checked.
- [x] Remaining non-blocking items are tracked in open issues (alpha.3 backlog framing in `docs/status-2026-05-14.md`).
- [x] Tag pushed as `v0.1.0-alpha.2`.

Status:
- Gate cleared on 2026-05-15. Release is published with `prerelease: true` (intentional alpha policy) and contains all four target archives. Provenance for the Intel-Mac artifact is documented in #181 and disclosed in `docs/release-notes/v0.1.0-alpha.2.md`.
