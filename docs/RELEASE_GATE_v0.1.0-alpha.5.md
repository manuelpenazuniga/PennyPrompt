# Release Gate: v0.1.0-alpha.5

Objective gate for publishing `v0.1.0-alpha.5` (Phase A — compatibility & cost accuracy).

Status: **prepared, not yet published.** Pre-tag gates (scope, version, quality, audit,
runtime smoke, docs) are complete on the release-prep branch; the tag-push, artifact, and
publish gates are pending the human-triggered release.

## 1. Scope Lock

- [x] `#207` merged: native Anthropic `/v1/messages` ingress (merge `6906bbd`).
- [x] `#208` merged: prompt-cache cost accounting (merge `f79689d`).
- [x] `#209` merged: inbound concurrency limit and upstream timeout (merge `8f23e42`).
- [x] `#236` merged: binary rename `penny-cli` → `pennyprompt` with compat shim (merge `86881da`).
- [x] `#210` merged: compatibility/limitation docs flipped to verified (merge `27be64d`).
- [x] Epic `#225` scope complete.

## 2. Version Gate

- [x] Workspace package version is `0.1.0-alpha.5`.
- [x] `crates/penny-cli/Cargo.toml` version is `0.1.0-alpha.5`.
- [x] `pennyprompt --version` reports `pennyprompt 0.1.0-alpha.5` from a release build.

Evidence:
- `cargo build --release -p penny-cli --locked` passed on 2026-07-12.
- `./target/release/pennyprompt --version` returned `pennyprompt 0.1.0-alpha.5`.
- The release binary is named `pennyprompt` (via `[[bin]]`), crate/package name unchanged.

## 3. Workspace Quality Gate

Run on the release candidate branch before merge, and again from `main` before tagging if any
code changes land after this gate.

- [x] `cargo fmt --all -- --check`
- [x] `cargo check --workspace --locked`
- [x] `cargo test --workspace --locked`
- [x] `cargo clippy --workspace --all-targets --locked -- -D warnings`

Evidence:
- Executed locally on 2026-07-12 with an isolated `HOME` (config tests require a clean `HOME`):
  `HOME="$(mktemp -d)" RUSTUP_HOME=... CARGO_HOME=... cargo test --workspace --locked` — all passed.
- Each child PR (`#237`–`#241`) passed the same gate in GitHub Actions on its branch before merge.
- Full `cargo test --workspace --locked` remains a CI/main requirement where loopback binds are permitted.

## 4. Security Audit Gate

- [x] `cargo audit --ignore RUSTSEC-2023-0071`

Evidence:
- Ran on 2026-07-12: exit 0. `RUSTSEC-2023-0071` remains ignored intentionally (`rsa` is present through
  lockfile metadata for sqlx optional MySQL support, not the active normal dependency graph).
- One advisory warning is surfaced but non-blocking (`RUSTSEC-2026-0190`, `anyhow` unsoundness) and the
  same audit command runs in CI.

## 5. Runtime Acceptance Smoke Tests

Use isolated local state.

Reference command sequence:

```bash
PENNY_RELEASE_HOME="$(mktemp -d)"
mkdir -p "$PENNY_RELEASE_HOME/.local/share/pennyprompt"

cargo build --release -p penny-cli --locked

HOME="$PENNY_RELEASE_HOME" ./target/release/pennyprompt --version
HOME="$PENNY_RELEASE_HOME" ./target/release/pennyprompt init --preset indie --force
HOME="$PENNY_RELEASE_HOME" ./target/release/pennyprompt prices update
HOME="$PENNY_RELEASE_HOME" ./target/release/pennyprompt --json-log serve --mock --admin-bind 127.0.0.1:8586 &
curl -fsS http://127.0.0.1:8586/admin/health
curl -fsS -X POST http://127.0.0.1:8585/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"claude-sonnet-4-6","messages":[{"role":"user","content":"hi"}]}'
curl -fsS -X POST http://127.0.0.1:8585/v1/messages \
  -H 'content-type: application/json' -H 'anthropic-version: 2023-06-01' \
  -d '{"model":"claude-sonnet-4-6","max_tokens":128,"messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}]}'
HOME="$PENNY_RELEASE_HOME" ./target/release/pennyprompt report summary --since 1d --by model
```

- [x] `pennyprompt --version` reports alpha.5.
- [x] `init --preset indie --force` succeeds.
- [x] `prices update` succeeds (`imported_entries: 10`, `validated_default_models: 2`).
- [x] `doctor` succeeds in isolated HOME (config/database OK, 10 active pricebook models).
- [x] `serve --mock --admin-bind 127.0.0.1:8586` starts proxy and admin planes.
- [x] `admin/health` returns `200`.
- [x] OpenAI-ingress chat completion (`/v1/chat/completions`) returns `200` with the OpenAI shape.
- [x] **Native Anthropic ingress (`/v1/messages`) returns `200` with the native Anthropic shape** (`type: message`, text content block, `usage.input_tokens`/`output_tokens`/`cache_*`).
- [x] **Native Anthropic streaming (`/v1/messages` `stream: true`) emits the native Anthropic SSE sequence** (`event: message_start`, `content_block_*`, no `[DONE]`).
- [x] `report summary` renders the cache breakdown columns (`input_tokens` / `cache_read` / `cache_write` / `output_tokens`) with a reconciled cost.

Evidence:
- Executed locally on 2026-07-12 from `chore/release-alpha5-prep` with an isolated `$PENNY_RELEASE_HOME`.
- `/v1/messages` non-stream returned `{"type":"message","role":"assistant","content":[{"type":"text",...}],"usage":{"input_tokens":120,"output_tokens":48,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}`.
- `/v1/messages` streaming returned ordered `event: message_start` → `content_block_start` → … native Anthropic SSE.
- `report summary --by model` showed `input_tokens/cache_read/cache_write/output_tokens/cost_usd` columns with 3 requests, 360 input, 144 output, `$0.003240`.
- TCP-bind behavior is also covered by CI workspace tests in a loopback-capable environment.

## 6. Docs Consistency Gate

- [x] `CHANGELOG.md` contains `v0.1.0-alpha.5` notes before tag push.
- [x] `docs/release-notes/v0.1.0-alpha.5.md` exists.
- [x] README compatibility table and `docs/LIMITATIONS.md` reflect the implemented native Anthropic ingress and cache-accurate cost (`#210`).
- [x] `docs/CONFIG-REFERENCE.md` documents `[server].max_inflight_requests` and `[server].upstream_timeout_ms` (`#209`).
- [x] `docs/PRICEBOOK.md` documents the `cache_read_per_mtok` / `cache_write_per_mtok` fields (`#208`).
- [x] Installer, release workflow, and docs use the `pennyprompt` binary name with the legacy-asset fallback (`#236`).

## 7. CI and Release Workflow Gate

- [x] Release-prep PR CI is green.
- [ ] Tag `v0.1.0-alpha.5` is pushed from updated `main` (human-triggered).
- [ ] Release workflow builds the three supported CI targets with `pennyprompt-*` asset names.
- [ ] `x86_64-apple-darwin` backfilled locally from the same tag commit if the Intel runner blocks.

## 8. Artifact Verification Gate

After the Release workflow publishes, verify at least one downloaded artifact checksum.

```bash
gh release download v0.1.0-alpha.5 \
  --repo manuelpenazuniga/PennyPrompt \
  --pattern 'pennyprompt-v0.1.0-alpha.5-x86_64-unknown-linux-gnu.tar.gz' \
  --pattern 'pennyprompt-v0.1.0-alpha.5-x86_64-unknown-linux-gnu.sha256'

shasum -a 256 -c pennyprompt-v0.1.0-alpha.5-x86_64-unknown-linux-gnu.sha256
```

- [ ] Release has target archives and per-target `.sha256` files (`pennyprompt-*` naming).
- [ ] `CHECKSUMS.txt` is present.
- [ ] All target checksums verify locally.
- [ ] A pinned pre-rename tag (`PENNY_VERSION=v0.1.0-alpha.3`) still installs via the legacy `penny-cli-*` asset fallback.

## 9. Publish Decision

- [x] All pre-tag release-prep boxes above are checked.
- [ ] `main` is synchronized with `origin/main` at the release commit.
- [ ] `v0.1.0-alpha.5` tag pushed.
- [ ] GitHub Release verified (prerelease, per alpha policy).
- [ ] Epic `#225` closed when the final release-documentation PR merges.
