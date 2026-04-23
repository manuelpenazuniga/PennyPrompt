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
