# Release Gate: v0.1.0-alpha.3

Objective gate for publishing `v0.1.0-alpha.3` after the alpha.3 hardening scope.

Status: pre-tag candidate. This document records the release-prep gate; publish/artifact verification remains pending until the tag workflow completes.

## 1. Scope Lock

- [x] `#183` merged: CLI help descriptions for root and nested subcommands.
- [x] `#184` merged: per-model tokenizer dispatch for OpenAI, Anthropic, and fallback models.
- [x] `#185` merged: structured proxy tracing on the request hot path.
- [x] `#189` merged: TLS verification dependency refresh and `cargo audit` CI gate.
- [x] `#190` merged: admin-plane security contract aligned with current local-only implementation.
- [x] `#196` opened for release-prep docs/version work.

## 2. Version Gate

- [x] Workspace package version is `0.1.0-alpha.3`.
- [x] `crates/penny-cli/Cargo.toml` version is `0.1.0-alpha.3`.
- [x] `pennyprompt --version` reports `pennyprompt 0.1.0-alpha.3` from a release build.

Evidence:
- `cargo build --release -p penny-cli --locked` passed on 2026-06-20.
- `HOME="$PENNY_RELEASE_HOME" ./target/release/penny-cli --version` returned `pennyprompt 0.1.0-alpha.3`.

## 3. Workspace Quality Gate

Run on the release candidate branch before merge, and again from `main` before tagging if any code changes land after this gate.

- [x] `cargo fmt --all -- --check`
- [x] `cargo check --workspace --locked`
- [ ] `cargo test --workspace --locked`
- [x] `cargo clippy --workspace --all-targets --locked -- -D warnings`

Evidence:
- Executed locally on 2026-06-20 from `release-alpha3-prep`.
- `cargo test --workspace --locked` was attempted but the restricted sandbox rejected local TCP binds with `Operation not permitted`.
- Follow-up command passed for all non-bind tests:
  - `cargo test --workspace --locked -- --skip check_admin_bind_readiness_accepts_ephemeral_tcp_bind --skip serve_mock_starts_proxy_and_admin_and_shuts_down_cleanly --skip anthropic_error_payload_is_mapped --skip anthropic_non_stream_is_mapped_to_openai_shape --skip anthropic_streaming_sse_is_rewritten_with_usage_chunk --skip openai_error_payload_is_mapped --skip openai_http_429_and_503_are_passthrough --skip openai_non_stream_forwards_payload_and_auth_header --skip openai_parse_failure_is_mapped_to_502 --skip openai_stream_can_arrive_without_usage_for_estimation_fallback --skip openai_stream_preserves_usage_when_present --skip openai_timeout_is_mapped_to_504`
- Full `cargo test --workspace --locked` remains a CI/main pre-tag requirement where loopback binds are permitted.

## 4. Security Audit Gate

- [x] `cargo audit --ignore RUSTSEC-2023-0071`

Evidence:
- `RUSTSEC-2023-0071` remains ignored intentionally because `rsa` is present through lockfile metadata for sqlx optional MySQL support, not the active normal dependency graph.
- CI runs the same audit command.
- Local sandbox run used the already-present advisory DB without network fetch:
  - DB path: `$CARGO_AUDIT_DB`
  - DB revision: `776615bd`
  - DB commit date: `2026-06-18T13:58:33+02:00`
  - Command: `cargo audit --db "$CARGO_AUDIT_DB" --no-fetch --no-yanked --ignore RUSTSEC-2023-0071`

## 5. Runtime Acceptance Smoke Tests

Use isolated local state.

Reference command sequence:

```bash
PENNY_RELEASE_HOME="$(mktemp -d)"
mkdir -p "$PENNY_RELEASE_HOME/.local/share/pennyprompt"

cargo build --release -p penny-cli --locked

HOME="$PENNY_RELEASE_HOME" ./target/release/penny-cli --version
HOME="$PENNY_RELEASE_HOME" ./target/release/penny-cli init --preset indie --force
HOME="$PENNY_RELEASE_HOME" ./target/release/penny-cli prices update
HOME="$PENNY_RELEASE_HOME" ./target/release/penny-cli --json-log serve --mock --admin-bind 127.0.0.1:8586
curl -fsS http://127.0.0.1:8586/admin/health
curl -fsS -X POST http://127.0.0.1:8585/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"claude-sonnet-4-6","messages":[{"role":"user","content":"hi"}]}'
HOME="$PENNY_RELEASE_HOME" ./target/release/penny-cli tail --admin-url http://127.0.0.1:8586 --once --limit 20
```

- [x] `pennyprompt --version` reports alpha.3.
- [x] `init --preset indie --force` succeeds.
- [x] `prices update` succeeds and validates default models.
- [x] `doctor` succeeds in isolated HOME.
- [ ] `serve --mock --admin-bind 127.0.0.1:8586` starts proxy and admin planes.
- [ ] `admin/health` returns `200`.
- [ ] Proxy chat completion path returns `200`.
- [ ] `tail --once` can consume admin events.
- [ ] `--json-log` emits structured proxy events.

Evidence:
- Executed locally on 2026-06-20 from `release-alpha3-prep` with an isolated `$PENNY_RELEASE_HOME`.
- `init --preset indie --force` created `$PENNY_RELEASE_HOME/.config/pennyprompt/config.toml`.
- `prices update` output: `imported_entries: 10`, `validated_default_models: 2`.
- `doctor` exited successfully with config/database OK, 10 active pricebook models, and expected missing provider API keys in isolated local state.
- TCP bind smoke remains pending because the restricted sandbox rejects local listener creation with `Operation not permitted`.

## 6. Docs Consistency Gate

- [x] `CHANGELOG.md` contains `v0.1.0-alpha.3` notes before tag push.
- [x] `docs/release-notes/v0.1.0-alpha.3.md` exists.
- [x] `docs/RELEASE.md` points the active gate and notes links at alpha.3.
- [x] `docs/GITHUB_BACKLOG.md` reflects `#196` as release prep and `#186` as the remaining publish epic.
- [x] `docs/CONFIG-REFERENCE.md` and `docs/LIMITATIONS.md` document admin as local-only and unauthenticated in the current alpha.
- [x] README documents admin TCP as loopback-only and states bearer/admin-token auth is not implemented yet.

## 7. CI and Release Workflow Gate

- [ ] Release-prep PR CI is green.
- [ ] Tag `v0.1.0-alpha.3` is pushed from updated `main`.
- [ ] Release workflow publishes at least the minimum artifact set required by `.github/workflows/release.yml`.

Evidence:
- Pending PR and tag workflow.

## 8. Artifact Verification Gate

After the Release workflow publishes, verify at least one downloaded artifact checksum.

Reference checksum commands:

```bash
gh release download v0.1.0-alpha.3 \
  --repo manuelpenazuniga/PennyPrompt \
  --pattern 'penny-cli-v0.1.0-alpha.3-x86_64-unknown-linux-gnu.tar.gz' \
  --pattern 'penny-cli-v0.1.0-alpha.3-x86_64-unknown-linux-gnu.sha256'

shasum -a 256 -c penny-cli-v0.1.0-alpha.3-x86_64-unknown-linux-gnu.sha256
```

- [ ] Release has target archives and per-target `.sha256` files.
- [ ] `CHECKSUMS.txt` is present.
- [ ] At least one target checksum verifies locally.

Evidence:
- Pending tag workflow.

## 9. Publish Decision

- [ ] All pre-tag release-prep boxes above are checked.
- [ ] `main` is synchronized with `origin/main`.
- [ ] `v0.1.0-alpha.3` tag pushed.
- [ ] GitHub Release verified.
- [ ] `#186` closed after artifact verification.
