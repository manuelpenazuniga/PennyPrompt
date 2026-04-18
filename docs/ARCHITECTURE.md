# PennyPrompt Architecture (Alpha)

This document describes the current repository architecture and runtime boundaries.

## Workspace Layout

```text
crates/
  penny-types/      Shared domain types and enums
  penny-config/     Config loading, merge rules, validation
  penny-store/      SQLite repositories + migrations + WAL mode
  penny-cost/       Token estimation + pricebook pricing + estimation ranges
  penny-ledger/     Atomic reserve/reconcile/release ledger flow
  penny-budget/     Budget evaluation for observe/guard modes
  penny-detect/     Loop and burn-rate detection engine
  penny-providers/  Provider adapters and response normalization
  penny-proxy/      Request pipeline and provider dispatch path
  penny-admin/      Admin HTTP API and SSE events stream
  penny-cli/        Operator CLI interface
  penny-observe/    Tracing/logging support
```

## Data Model (SQLite)

Primary relational flow:

```text
projects -> sessions -> requests -> request_usage
                          |
                          +-> events

budgets
cost_ledger (append-only accounting rows)
pricebook_entries (versioned by effective window)
providers / models (routing metadata)
```

Key properties:

- `cost_ledger` is append-only.
- pricing is time-versioned with `effective_from/effective_until`.
- event history is persisted in `events`.

## Runtime Planes

## Proxy Plane

- Accepts OpenAI-compatible request shape.
- Normalizes request.
- Estimates cost.
- Runs budget evaluation (`observe` or `guard`).
- Dispatches to provider adapter.
- Reconciles usage and emits detect events.

## Admin Plane

- Exposes operational APIs:
  - health
  - report summary/top
  - budgets CRUD-like operations
  - estimate endpoint
  - detect status/resume
  - events SSE stream

## CLI Plane

- Operator interface for setup and inspection:
  - init, doctor, config
  - prices show/update
  - budget list/set/reset
  - estimate, report summary/top
  - detect status/resume
  - tail (SSE consumer)

## Core Flow (Request Lifecycle)

1. Request enters proxy.
2. Request is attributed (project/session context).
3. Estimated cost is computed from pricebook.
4. Ledger reserve executes atomically.
5. Provider call executes.
6. Actual usage reconciles back into ledger.
7. Events are persisted for budget/detect/provider outcomes.
8. CLI/admin read from the same local database.

## Error and Safety Model

- `guard` mode fails closed on critical budget/ledger lookup failures.
- `observe` mode logs warnings/failsafe events and allows traffic.
- budget hard blocks map to structured non-retryable responses.
- detect pause state can be resumed explicitly by operator command/API.

## Design Constraints (Current Alpha)

- local-first architecture (single-node SQLite default)
- deterministic local pricebook import path
- no external control plane dependency required for core operation

For known gaps and temporary alpha limitations, see [LIMITATIONS.md](./LIMITATIONS.md).
