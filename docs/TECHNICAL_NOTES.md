# PennyPrompt Technical Notes

This file tracks non-blocking technical recommendations that should remain visible
without stopping milestone delivery.

## 2026-04: Gemini Non-Blocking Recommendations (`#94`)

Source reviews:
- PR #70
- PR #82
- PR #84

### 1. Docs heading hierarchy

Status: tracked, low priority.

Decision:
- Keep incremental cleanup as docs are touched by future functional work.
- Do not open a dedicated refactor pass unless readability regressions appear.

### 2. Installer prerequisites consistency

Status: addressed in this issue.

Decision:
- Keep install docs aligned with actual installer script dependencies.
- `mktemp` is now listed in `docs/INSTALL.md` prerequisites.

### 3. Dynamic SQL construction style

Status: tracked with guardrails in place.

Decision:
- Current dynamic SQL interpolation is constrained to trusted enum-controlled
  fragments (group key and join variant), not raw user input.
- Query parameters remain bound (`?` placeholders) for all external filters.
- If summary query complexity grows, migrate to a query-builder style in a future
  dedicated hardening issue.

### Close Criteria for `#94`

- This recommendation set remains documented and discoverable.
- Blocking risks are not present in current implementation scope.
- Future hardening can be tracked independently without stalling roadmap flow.

## 2026-04: Detect Resume Consistency Policy (`#110`)

Context:
- Gemini review on PR #97 flagged ambiguity in resume semantics when event
  persistence fails.

Selected policy:
- `detect resume` is **best-effort persist after in-memory resume**.
- We prioritize unblocking the paused session in-memory first.
- Event persistence is attempted immediately after and reported explicitly.

API contract:
- `resumed: true` means the in-memory pause state was cleared.
- `persisted` indicates whether the `session_resumed` event was written.
- `consistency.mode = best_effort_resume_then_persist`
- `consistency.event_persistence_guarantee = best_effort`
- `warning` is present when persistence fails.
