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

## 2026-04: Gemini Perf/Readability Follow-ups (`#128`)

Source reviews:
- PR #125: https://github.com/manuelpenazuniga/PennyPrompt/pull/125#discussion_r3142804023
- PR #127: https://github.com/manuelpenazuniga/PennyPrompt/pull/127#discussion_r3142857157

### 1. Proxy SSE ANSI marker scan style (PR #125)

Status: tracked, non-blocking.

Decision:
- Current implementation is functionally correct and does not expose a
  correctness or security defect.
- Treat iterator/slice-style rewrite as readability/micro-perf polish.
- Apply opportunistically when touching nearby proxy cleanup code.

### 2. CLI `csv_escape` quoted wrapping path (PR #127)

Status: tracked, non-blocking.

Decision:
- Current implementation is functionally correct and covered by tests.
- `push_str` fast-path when `quote_count == 0` is a micro-optimization only.
- Apply opportunistically in a future CLI perf/readability touch.

### Close Criteria for `#128`

- Recommendation set remains documented and discoverable.
- No urgent correctness, security, or data-integrity risk is pending from these
  comments.

## 2026-05: Gemini Triage for PRs `#135`-`#140` (`#155`)

Source PRs:
- https://github.com/manuelpenazuniga/PennyPrompt/pull/135
- https://github.com/manuelpenazuniga/PennyPrompt/pull/136
- https://github.com/manuelpenazuniga/PennyPrompt/pull/137
- https://github.com/manuelpenazuniga/PennyPrompt/pull/138
- https://github.com/manuelpenazuniga/PennyPrompt/pull/139
- https://github.com/manuelpenazuniga/PennyPrompt/pull/140

This section is the canonical disposition log for recommendations raised during
that review wave.

### Disposition Ledger

1. Admin bind hostname/IP contract ambiguity (PRs #135/#136)
- Disposition: **Accepted / Fixed**
- Tracking: `#146` -> merged in [PR #159](https://github.com/manuelpenazuniga/PennyPrompt/pull/159)
- Notes: Contract is now explicit and tested for hostname, ip:port, unix path,
  and invalid bind strings.

2. Serve signal-shutdown short-circuit risk (PR #135)
- Disposition: **Accepted / Fixed**
- Tracking: `#147` -> merged in [PR #160](https://github.com/manuelpenazuniga/PennyPrompt/pull/160)
- Notes: Both plane outcomes are always awaited/evaluated with deterministic
  multi-failure reporting.

3. Pricebook guardrail timing/atomicity concerns (PR #137)
- Disposition: **Accepted / Fixed**
- Tracking: `#148` -> merged in [PR #161](https://github.com/manuelpenazuniga/PennyPrompt/pull/161)
- Notes: Guardrail moved pre-import, validation made deterministic and
  independent from incidental wall-clock behavior.

4. Doctor robustness gaps (timeouts, in-memory DSNs, datetime parsing) (PR #139)
- Disposition: **Accepted / Fixed**
- Tracking: `#149` -> merged in [PR #162](https://github.com/manuelpenazuniga/PennyPrompt/pull/162)
- Notes: Added tests and explicit timeout policy rationale.

5. Observe precedence surprise (env overriding explicit CLI flags) (PR #140)
- Disposition: **Accepted / Fixed**
- Tracking: `#150` -> merged in [PR #163](https://github.com/manuelpenazuniga/PennyPrompt/pull/163)
- Notes: Runtime precedence is now explicit and documented as
  `CLI explicit > env > defaults`.

6. PR/issue traceability should be policy, not convention (cross-cutting)
- Disposition: **Accepted / Fixed**
- Tracking: `#151` -> merged in [PR #164](https://github.com/manuelpenazuniga/PennyPrompt/pull/164)
- Notes: Added workflow doc rule, PR template, and CI linkage check with an
  explicit exception label.

7. Tail/detect topology defaults unclear for operators (cross-cutting docs)
- Disposition: **Accepted / Fixed**
- Tracking: `#152` -> merged in [PR #165](https://github.com/manuelpenazuniga/PennyPrompt/pull/165)
- Notes: Local default path is now clearly documented around loopback TCP admin
  connectivity.

8. Status snapshot drift after hardening sequence (cross-cutting docs)
- Disposition: **Accepted / Fixed**
- Tracking: `#153` -> merged in [PR #166](https://github.com/manuelpenazuniga/PennyPrompt/pull/166)
- Notes: New dated status snapshot with evidence-backed resolved/pending map.

9. Release-gate checklist command ambiguity (PR #138)
- Disposition: **Accepted / Fixed**
- Tracking: `#154` -> merged in [PR #167](https://github.com/manuelpenazuniga/PennyPrompt/pull/167)
- Notes: Runtime and checksum verification commands are now concrete and
  reproducible.

10. Release publication and artifact evidence completeness (post-gate execution)
- Disposition: **Deferred (Operational dependency)**
- Tracking: `#144` (open)
- Rationale: This is bounded by release workflow execution/publication state,
  not by unresolved code-level correctness in reviewed PRs.
- Revisit trigger: Close immediately once release artifacts + checksums are
  published and evidence-linked in the gate doc.

### Rejected / Not-a-Defect Clarifications

1. Model-ID validity false alarms in docs/pricebook review thread
- Disposition: **Rejected as defect**
- Rationale: PennyPrompt maintains canonical local model IDs in versioned
  pricebooks; IDs are validated through the guardrail/import pipeline and not
  by matching marketing naming conventions verbatim.
- Supporting fixes: [PR #137](https://github.com/manuelpenazuniga/PennyPrompt/pull/137),
  [PR #161](https://github.com/manuelpenazuniga/PennyPrompt/pull/161)
- Revisit trigger: open a blocker only if a canonical ID cannot be resolved at
  import/runtime or provider mapping fails in integration.

2. Mandatory no-exception linkage enforcement
- Disposition: **Rejected as policy default**
- Rationale: Strict issue-linkage enforcement is required, but a documented
  emergency/doc-only exception path is operationally necessary (`skip-issue-linkage`).
- Supporting fix: [PR #164](https://github.com/manuelpenazuniga/PennyPrompt/pull/164)
- Revisit trigger: if exception label misuse appears, tighten policy with
  additional CI constraints (e.g., maintainer-only label application).

### Close Criteria for `#155`

- Every recommendation cluster from PRs `#135`-`#140` is mapped to one of:
  fixed, deferred, or rejected.
- Deferred/rejected entries contain rationale and explicit revisit triggers.
- This section remains the single source for avoiding repeated triage churn on
  already-dispositioned Gemini recommendations.
