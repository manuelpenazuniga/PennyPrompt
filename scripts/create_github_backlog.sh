#!/usr/bin/env bash

set -euo pipefail

repo="${1:-}"

if [[ -z "$repo" ]]; then
  if git remote get-url origin >/dev/null 2>&1; then
    remote_url="$(git remote get-url origin)"
    case "$remote_url" in
      git@github.com:*)
        repo="${remote_url#git@github.com:}"
        repo="${repo%.git}"
        ;;
      https://github.com/*)
        repo="${remote_url#https://github.com/}"
        repo="${repo%.git}"
        ;;
      *)
        echo "Could not derive owner/repo from origin remote."
        echo "Usage: $0 owner/repo"
        exit 1
        ;;
    esac
  else
    echo "No origin remote found."
    echo "Usage: $0 owner/repo"
    exit 1
  fi
fi

gh auth status >/dev/null

ensure_label() {
  local name="$1"
  local color="$2"
  local description="$3"

  if gh label list --repo "$repo" --limit 200 --json name --jq '.[].name' | rg -x --fixed-strings "$name" >/dev/null 2>&1; then
    gh label edit "$name" --repo "$repo" --color "$color" --description "$description" >/dev/null
  else
    gh label create "$name" --repo "$repo" --color "$color" --description "$description" >/dev/null
  fi
}

milestone_number() {
  local title="$1"
  gh api "repos/$repo/milestones?state=all&per_page=100" --jq ".[] | select(.title == \"$title\") | .number" | head -n1
}

ensure_milestone() {
  local title="$1"
  local description="$2"

  if [[ -z "$(milestone_number "$title" || true)" ]]; then
    gh api "repos/$repo/milestones" --method POST -f title="$title" -f description="$description" >/dev/null
  fi
}

issue_exists() {
  local title="$1"
  gh issue list --repo "$repo" --state all --limit 200 --json title --jq '.[].title' | rg -x --fixed-strings "$title" >/dev/null 2>&1
}

create_issue() {
  local title="$1"
  local milestone_title="$2"
  local labels="$3"
  local body="$4"

  if issue_exists "$title"; then
    echo "Skipping existing issue: $title"
    return
  fi

  gh issue create \
    --repo "$repo" \
    --title "$title" \
    --milestone "$milestone_title" \
    --label "$labels" \
    --body "$body" >/dev/null

  echo "Created issue: $title"
}

ensure_label "epic" "5319e7" "Cross-cutting milestone or delivery umbrella"
ensure_label "phase:m1" "1d76db" "Foundation"
ensure_label "phase:m2" "0e8a16" "Proxy pass-through"
ensure_label "phase:m3" "fbca04" "Atomic budgets"
ensure_label "phase:m4" "d4c5f9" "Streaming and real providers"
ensure_label "phase:m5" "b60205" "Active protection"
ensure_label "phase:m6" "c2e0c6" "Alpha release"
ensure_label "area:types" "bfdadc" "Shared domain types"
ensure_label "area:config" "bfd4f2" "Configuration and presets"
ensure_label "area:store" "c5def5" "SQLite schema and repositories"
ensure_label "area:cost" "d4c5f9" "Pricing and estimation"
ensure_label "area:providers" "f9d0c4" "Provider adapters"
ensure_label "area:proxy" "fef2c0" "Proxy plane"
ensure_label "area:ledger" "f9d0c4" "Atomic cost ledger"
ensure_label "area:budget" "fbca04" "Budget evaluation"
ensure_label "area:admin" "c2e0c6" "Admin plane"
ensure_label "area:detect" "b60205" "Loop and burn-rate detection"
ensure_label "area:cli" "0e8a16" "CLI UX"
ensure_label "area:docs" "0052cc" "Documentation"
ensure_label "area:release" "5319e7" "Release engineering"
ensure_label "kind:test" "d93f0b" "Testing work"
ensure_label "kind:ci" "6f42c1" "CI or automation work"

ensure_milestone "M1 Foundation" "Workspace, config, schema, pricebook, and pricing engine."
ensure_milestone "M2 Proxy Pass-Through" "Proxy plane with mock provider and auto-attribution."
ensure_milestone "M3 Atomic Budgets" "Ledger-backed budget enforcement and first cost reports."
ensure_milestone "M4 Streaming and Real Providers" "Anthropic, OpenAI, streaming, and admin plane."
ensure_milestone "M5 Active Protection" "Loop detection, burn-rate alerts, estimate, and live tail."
ensure_milestone "M6 Alpha Release" "CLI polish, docs, tests, and release artifacts."

create_issue "EPIC: M1 Foundation" "M1 Foundation" "epic,phase:m1" "$(cat <<'EOF'
Goal:
- Establish the Rust workspace and the minimum core subsystems required for all later milestones.

Scope:
- Workspace scaffold
- shared types
- config and presets
- SQLite store and migrations
- pricing engine and pricebook loader
- CI

Definition of done:
- `cargo test --workspace` passes
- presets load correctly
- at least six models resolve in the local pricebook
EOF
)"

create_issue "Scaffold Cargo workspace and crate layout" "M1 Foundation" "phase:m1,area:types" "$(cat <<'EOF'
Create the base Rust workspace and crate skeleton described in the spec.

Deliverables:
- root `Cargo.toml`
- `rust-toolchain.toml`
- crate directories and minimal `Cargo.toml` files
- empty `lib.rs` or `main.rs` stubs where appropriate

Acceptance criteria:
- `cargo check --workspace` passes
- crate graph matches the documented architecture
EOF
)"

create_issue "Implement penny-types shared domain model" "M1 Foundation" "phase:m1,area:types" "$(cat <<'EOF'
Implement the shared types crate with the request, response, budget, ledger, detect, and event types from the spec.

Deliverables:
- typed identifiers
- core structs and enums
- top-level error type
- serialization tests

Acceptance criteria:
- no business logic in `penny-types`
- round-trip serde tests cover key public types
EOF
)"

create_issue "Implement penny-config loader, presets, validation, env overrides" "M1 Foundation" "phase:m1,area:config" "$(cat <<'EOF'
Implement strongly typed config loading from TOML, plus preset and environment variable merging.

Deliverables:
- `AppConfig` and supporting config structs
- `indie`, `team`, and `explore` presets
- reference `config/default.toml`
- validation for URLs, enums, and budget limits

Acceptance criteria:
- config loads from TOML
- `PENNY_*` overrides are applied correctly
- invalid config fails with actionable errors
EOF
)"

create_issue "Implement penny-store migrations and repository layer" "M1 Foundation" "phase:m1,area:store" "$(cat <<'EOF'
Implement SQLite setup, migrations, and repository traits.

Deliverables:
- migrations 0001 through 0007
- WAL mode setup
- project, session, request, budget, event, and pricebook repositories

Acceptance criteria:
- migrations run on startup
- CRUD tests pass against in-memory SQLite
- repository signatures align with the development guide
EOF
)"

create_issue "Implement penny-cost pricing engine and pricebook loader" "M1 Foundation" "phase:m1,area:cost" "$(cat <<'EOF'
Implement cost calculation, token estimation, range estimation, and pricing snapshots.

Deliverables:
- local pricebook files for Anthropic and OpenAI
- pricebook import into SQLite
- cost calculation and estimate APIs

Acceptance criteria:
- pricing tests pass
- token estimation has documented fallbacks
- pricebook resolves at least six models
EOF
)"

create_issue "Add CI workflow for check, test, clippy, fmt" "M1 Foundation" "phase:m1,kind:ci" "$(cat <<'EOF'
Add baseline CI for the workspace.

Deliverables:
- GitHub Actions workflow
- `cargo check`
- `cargo test --workspace`
- `cargo clippy -- -D warnings`
- `cargo fmt -- --check`

Acceptance criteria:
- workflow runs on pushes and pull requests
- failures are actionable and deterministic
EOF
)"

create_issue "EPIC: M2 Proxy Pass-Through" "M2 Proxy Pass-Through" "epic,phase:m2" "$(cat <<'EOF'
Goal:
- Accept OpenAI-compatible requests at `:8585`, route to a mock provider, and persist request metadata with automatic project and session attribution.

Definition of done:
- mock round-trip works with `curl`
- rows are written for project, session, request, and usage
EOF
)"

create_issue "Implement MockProvider for deterministic integration tests" "M2 Proxy Pass-Through" "phase:m2,area:providers" "$(cat <<'EOF'
Implement the test-only provider adapter used for early integration work.

Deliverables:
- deterministic non-streaming responses
- deterministic streaming responses
- configurable token usage payloads

Acceptance criteria:
- provider responses are stable across runs
- both streaming and non-streaming paths are exercised in tests
EOF
)"

create_issue "Implement proxy server and OpenAI-compatible endpoints" "M2 Proxy Pass-Through" "phase:m2,area:proxy" "$(cat <<'EOF'
Implement the initial proxy server and public API endpoints.

Deliverables:
- bind on `127.0.0.1:8585`
- `POST /v1/chat/completions`
- `GET /v1/models`
- request ID header generation

Acceptance criteria:
- valid requests pass through to the mock provider
- invalid requests fail clearly
EOF
)"

create_issue "Implement normalization pipeline and SQLite request persistence" "M2 Proxy Pass-Through" "phase:m2,area:proxy,area:store" "$(cat <<'EOF'
Normalize inbound requests and persist enough metadata for later accounting.

Deliverables:
- model extraction
- stream flag handling
- token estimation hook
- request and usage persistence

Acceptance criteria:
- request records are created consistently
- request IDs and timestamps are traceable end-to-end
EOF
)"

create_issue "Implement project and session auto-attribution" "M2 Proxy Pass-Through" "phase:m2,area:proxy" "$(cat <<'EOF'
Implement automatic project and session attribution without requiring custom headers.

Deliverables:
- git root detection
- project slug derivation
- session window grouping
- explicit header override support

Acceptance criteria:
- same project within the time window reuses the session
- missing git root falls back cleanly to `default`
EOF
)"

create_issue "Add temporary health endpoint" "M2 Proxy Pass-Through" "phase:m2,area:proxy" "$(cat <<'EOF'
Add a temporary internal health endpoint before the admin plane exists.

Deliverables:
- DB status
- uptime
- configured provider visibility

Acceptance criteria:
- endpoint can support local smoke testing until admin plane is ready
EOF
)"

create_issue "EPIC: M3 Atomic Budgets" "M3 Atomic Budgets" "epic,phase:m3" "$(cat <<'EOF'
Goal:
- Budget enforcement is atomic, concurrent-safe, and mode-aware.

Definition of done:
- over-budget requests return non-retriable `402`
- concurrent requests do not overspend
- guard mode blocks on budget subsystem failure
EOF
)"

create_issue "Implement penny-ledger atomic reservation flow" "M3 Atomic Budgets" "phase:m3,area:ledger,area:store" "$(cat <<'EOF'
Implement reserve, reconcile, and release around an append-only cost ledger.

Deliverables:
- `BEGIN IMMEDIATE` reserve flow
- running totals per budget
- reconcile diff entries
- release for failed or cancelled requests

Acceptance criteria:
- reserve and budget check happen in one transaction
- concurrent reserve tests show no overspend
EOF
)"

create_issue "Implement penny-budget evaluator and observe/guard modes" "M3 Atomic Budgets" "phase:m3,area:budget" "$(cat <<'EOF'
Implement budget selection, window logic, soft warnings, and mode-specific fail behavior.

Deliverables:
- applicable budget lookup
- day, week, month, total window handling
- observe and guard mode routing
- event recording for allow, warn, block, failsafe

Acceptance criteria:
- guard mode is fail-closed
- observe mode logs failsafe but permits traffic
EOF
)"

create_issue "Integrate budget enforcement and structured 402 error bodies" "M3 Atomic Budgets" "phase:m3,area:proxy,area:budget" "$(cat <<'EOF'
Integrate budget evaluation into the proxy request lifecycle and return stable error payloads.

Deliverables:
- pre-dispatch reserve
- post-response reconcile
- release on dispatch failure
- structured `402` with `retryable: false`

Acceptance criteria:
- error payload includes scope, window, accumulated, limit, and reset data
- proxy does not return `429` for local budget blocks
EOF
)"

create_issue "Seed budgets from config and presets" "M3 Atomic Budgets" "phase:m3,area:config,area:budget" "$(cat <<'EOF'
Load and upsert budgets from the active config and preset source.

Deliverables:
- budget seeding on startup
- preset source tagging
- idempotent upsert behavior

Acceptance criteria:
- seeded budgets are visible to the evaluator immediately
- preset budgets can be overridden safely
EOF
)"

create_issue "Implement report summary CLI" "M3 Atomic Budgets" "phase:m3,area:cli" "$(cat <<'EOF'
Add the first operator-facing cost report.

Deliverables:
- `report summary --since`
- aggregation by project and model
- human-readable table output

Acceptance criteria:
- totals match the recorded usage rows
- output is stable enough for golden tests later
EOF
)"

create_issue "EPIC: M4 Streaming and Real Providers" "M4 Streaming and Real Providers" "epic,phase:m4" "$(cat <<'EOF'
Goal:
- Replace mock-only routing with real Anthropic and OpenAI support, preserve streaming behavior, and expose admin APIs.

Definition of done:
- real providers work
- streaming is accounted accurately
- admin plane is separate from proxy plane
EOF
)"

create_issue "Implement Anthropic provider adapter" "M4 Streaming and Real Providers" "phase:m4,area:providers" "$(cat <<'EOF'
Implement request and response translation for Anthropic.

Deliverables:
- message API payload mapping
- auth and version headers
- usage extraction
- Anthropic SSE event handling

Acceptance criteria:
- non-streaming and streaming flows both work against Anthropic
EOF
)"

create_issue "Implement OpenAI provider adapter" "M4 Streaming and Real Providers" "phase:m4,area:providers" "$(cat <<'EOF'
Implement the native OpenAI provider adapter.

Deliverables:
- request forwarding
- auth header injection
- usage extraction from normal and streaming responses

Acceptance criteria:
- adapter preserves expected OpenAI-compatible behavior
- estimation fallback is used when final usage is absent
EOF
)"

create_issue "Implement streaming pass-through and reconciliation" "M4 Streaming and Real Providers" "phase:m4,area:proxy,area:ledger" "$(cat <<'EOF'
Add low-latency SSE forwarding and end-of-stream accounting.

Deliverables:
- immediate chunk forwarding
- background accumulation for token counting
- reconcile on `[DONE]`
- incomplete stream handling

Acceptance criteria:
- client receives streaming without full buffering
- interrupted streams are marked incomplete and still accounted for
EOF
)"

create_issue "Implement admin plane endpoints and event SSE" "M4 Streaming and Real Providers" "phase:m4,area:admin" "$(cat <<'EOF'
Build the admin plane as a separate service surface.

Deliverables:
- health endpoint
- report endpoints
- budget endpoints
- event SSE endpoint

Acceptance criteria:
- admin APIs are not exposed on the proxy port
- report and budget data match the SQLite store
EOF
)"

create_issue "Map upstream provider errors and incomplete streams" "M4 Streaming and Real Providers" "phase:m4,area:proxy,area:providers" "$(cat <<'EOF'
Normalize provider-side error handling for operator clarity.

Deliverables:
- timeout to `504`
- provider `5xx` passthrough with event logging
- provider `429` passthrough
- parse failure to `502`

Acceptance criteria:
- operator can distinguish provider failures from PennyPrompt budget failures
EOF
)"

create_issue "EPIC: M5 Active Protection" "M5 Active Protection" "epic,phase:m5" "$(cat <<'EOF'
Goal:
- Add loop detection, burn-rate monitoring, pre-execution estimate, and live event tailing.

Definition of done:
- looping sessions can alert or pause
- estimate and tail are usable from the CLI
EOF
)"

create_issue "Implement penny-detect heuristics and pause lifecycle" "M5 Active Protection" "phase:m5,area:detect" "$(cat <<'EOF'
Implement the in-memory detection engine and session pause support.

Deliverables:
- tool failure repetition heuristic
- content similarity heuristic
- burn-rate heuristic
- paused session tracking and resume

Acceptance criteria:
- thresholds are configurable
- pause and resume events are recorded
EOF
)"

create_issue "Integrate loop protection into proxy request flow" "M5 Active Protection" "phase:m5,area:proxy,area:detect" "$(cat <<'EOF'
Integrate detection results into the request lifecycle.

Deliverables:
- feed detector after reconcile
- short-circuit paused sessions before budget evaluation
- persist detect events

Acceptance criteria:
- paused session returns `402` with a specific reason
- detector integration does not add large request overhead
EOF
)"

create_issue "Implement estimate CLI and admin estimate API" "M5 Active Protection" "phase:m5,area:cli,area:admin,area:cost" "$(cat <<'EOF'
Implement route preview and cost range estimation.

Deliverables:
- file-glob context estimation in CLI
- task type range selection
- admin estimate endpoint
- budget impact summary

Acceptance criteria:
- estimate returns min, max, confidence, and budget status
- results are reproducible from the pricebook snapshot
EOF
)"

create_issue "Implement live tail CLI over SSE" "M5 Active Protection" "phase:m5,area:cli,area:admin" "$(cat <<'EOF'
Build the operator live-view command on top of admin event SSE.

Deliverables:
- request line formatting
- burn-rate warning formatting
- block formatting
- loop formatting

Acceptance criteria:
- live tail reflects the SSE stream in near real time
- output respects `NO_COLOR`
EOF
)"

create_issue "Implement detect status and detect resume" "M5 Active Protection" "phase:m5,area:cli,area:detect" "$(cat <<'EOF'
Expose detection state and recovery commands in the CLI.

Deliverables:
- active alert listing
- paused session listing
- resume command

Acceptance criteria:
- resume clears the paused session
- operator can see why a session was paused
EOF
)"

create_issue "EPIC: M6 Alpha Release" "M6 Alpha Release" "epic,phase:m6" "$(cat <<'EOF'
Goal:
- Reach public alpha quality for installation, operation, and release.

Definition of done:
- first-time user can install quickly
- docs and tests are release-grade
- release artifacts exist for target platforms
EOF
)"

create_issue "Finish operator-focused CLI commands and setup wizard" "M6 Alpha Release" "phase:m6,area:cli" "$(cat <<'EOF'
Finish the remaining CLI commands required for a usable alpha.

Deliverables:
- `init`
- `doctor`
- `config`
- `prices show`
- `prices update`
- `budget list`
- `budget set`
- `budget reset`
- `report top`

Acceptance criteria:
- commands provide actionable output
- setup flow can get a new user to first report quickly
EOF
)"

create_issue "Write alpha docs set" "M6 Alpha Release" "phase:m6,area:docs" "$(cat <<'EOF'
Create the supporting documentation needed for alpha.

Deliverables:
- `INSTALL.md`
- `QUICKSTART.md`
- `CONFIG-REFERENCE.md`
- `ARCHITECTURE.md`
- `PRICEBOOK.md`
- documented limitations

Acceptance criteria:
- docs are sufficient for a new user to install and use PennyPrompt
EOF
)"

create_issue "Add integration suite, golden tests, and manual alpha checklist" "M6 Alpha Release" "phase:m6,kind:test" "$(cat <<'EOF'
Expand test coverage to support release confidence.

Deliverables:
- integration tests for proxy, ledger, detect, and reports
- golden snapshots for CLI and error payloads
- manual alpha checklist execution

Acceptance criteria:
- integration suite is green
- golden outputs are deterministic
- acceptance checklist is documented and run
EOF
)"

create_issue "Add release automation, install script, and changelog" "M6 Alpha Release" "phase:m6,area:release" "$(cat <<'EOF'
Prepare alpha distribution artifacts.

Deliverables:
- multi-target release builds
- install script for `curl | sh`
- release checksums
- changelog

Acceptance criteria:
- alpha binaries exist for Linux and macOS on x86_64 and arm64
- release process is documented and repeatable
EOF
)"

echo "GitHub backlog creation complete for $repo"
