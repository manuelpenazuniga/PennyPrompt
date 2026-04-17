# PennyPrompt Development Workflow

This is the working protocol we use to keep delivery, traceability, and review quality consistent issue by issue.

## Goals

- Keep one logical change per branch and per PR.
- Keep history easy to audit from issue to commit to merge.
- Catch regressions before PR with a fixed local validation set.
- Preserve roadmap order unless a blocking bug requires reprioritization.

## Issue Selection Rule

1. Identify the parent epic issue.
2. Pick the next concrete issue that unblocks downstream work.
3. If a review finds a functional bug, create/fix that as priority before continuing the next milestone issue.
4. If feedback is optimization/debt, map it to an existing issue or create a dedicated technical issue.

## Branch and PR Rule

1. Start from synced `main`.
2. Create a new branch per issue.
3. Keep scope strict to that issue only.
4. Open one PR per branch.
5. Merge PR, then sync `main` before starting the next issue.

Recommended branch naming:

- `feat/m<phase>-issue-<number>-<short-name>`
- `fix/m<phase>-issue-<number>-<short-name>`
- `chore/m<phase>-issue-<number>-<short-name>` for non-feature maintenance.

## Standard Execution Steps

1. Sync base:
```bash
git switch main
git pull --ff-only
```
2. Create issue branch:
```bash
git switch -c feat/m3-issue-18-budget-enforcement
```
3. Implement only the selected issue scope.
4. Run local validation:
```bash
cargo fmt --all
cargo test -p <affected-crate>
cargo check --workspace
```
5. Stage only intended files and commit with issue reference.
6. Push and open PR.
7. Review bot/human comments and classify:
- Must-fix now (functional correctness/security/data integrity).
- Covered by existing backlog issue.
- New technical debt issue.
8. Merge PR.
9. Sync main again before next issue.

## Commit Message Pattern

Use conventional prefix + area + concise outcome + issue id.

Examples:

- `feat(ledger): implement atomic reserve/reconcile/release flow (#16)`
- `fix(budget): allow observe mode on budget denial and avoid duplicate warn events (#17)`
- `docs(backlog): record AI review follow-ups and issue mapping (#53)`

## Local Safety Checks Before Commit

- `git status --short` shows only expected files.
- No accidental files like `.DS_Store` staged.
- Tests/checks are green for touched crates plus workspace check.
- Branch is not `main`.

## Review Handling Policy

- High-priority functional alerts: fix before continuing roadmap.
- Medium/low optimization alerts: track in technical issues when not blocking.
- Keep accounting paths on deterministic money representation (`Money`) and avoid reintroducing raw `f64` in persisted/comparative budget or ledger logic.

## End-of-Issue Output Checklist

At issue close, always provide:

1. Parent issue and concrete issue implemented.
2. What was changed at high level.
3. Validation commands executed and status.
4. Exact `git add` command.
5. Recommended commit message.

## Recovery When Session Context Is Lost

1. Check sync and branch state:
```bash
git branch --show-current
git status --short
git fetch --all --prune
git rev-parse main
git rev-parse origin/main
```
2. Check latest merged PRs and open issues:
```bash
gh pr list --state merged --limit 20
gh issue list --state open --limit 200
```
3. Resume from the next issue in roadmap order, unless a higher-priority bug fix is open.
