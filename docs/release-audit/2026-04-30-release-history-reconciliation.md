# Release History Reconciliation (2026-04-30)

Issue: #143

## Purpose

Reconcile documented alpha release history with observable GitHub artifacts.

## Observed GitHub State (2026-04-30)

The following checks were executed against the repository:

- `gh release list --limit 20`
- `gh run list --workflow release.yml --limit 20`
- `git ls-remote --tags origin`

Observed output at audit time: no rows returned for releases, release workflow runs, or remote tags.

## Reconciliation Decision

1. `v0.1.0-alpha.1` remains recorded in `CHANGELOG.md` as an internal milestone cut date.
2. `v0.1.0-alpha.1` is not treated as a publicly verifiable GitHub Release artifact unless tag/release assets are later published.
3. Release and installation docs must avoid wording that implies a published artifact exists when none is observable.
4. Canonical public release history is defined by observable GitHub tags + GitHub Releases.

## Follow-up Execution

- #144 will execute and evidence the `v0.1.0-alpha.2` gate.
- #145 will finalize alpha.2 notes and changelog release entry.

