# Release Process (Alpha)

This document defines the repeatable process for alpha releases.

## Release Gates

- Generic alpha manual checklist: [`docs/ALPHA-MANUAL-CHECKLIST.md`](./ALPHA-MANUAL-CHECKLIST.md)
- Targeted gate for current cut: [`docs/RELEASE_GATE_v0.1.0-alpha.2.md`](./RELEASE_GATE_v0.1.0-alpha.2.md)
- Release notes draft for current cut: [`docs/release-notes/v0.1.0-alpha.2.md`](./release-notes/v0.1.0-alpha.2.md)

## Scope

Current release automation builds and publishes `penny-cli` binaries for:

- Linux `x86_64-unknown-linux-gnu`
- Linux `aarch64-unknown-linux-gnu`
- macOS `x86_64-apple-darwin`
- macOS `aarch64-apple-darwin`

All artifacts are published as `.tar.gz` plus SHA-256 checksums.

## Workflow Trigger

Release workflow file:

```text
.github/workflows/release.yml
```

Trigger condition:

- push a tag that starts with `v` (example: `v0.1.0-alpha.1`)

Manual trigger is also available via `workflow_dispatch`.

## Cut a Release

1. Ensure `main` is green and synchronized.
2. Complete the active release gate checklist (`RELEASE_GATE_v0.1.0-alpha.2.md` for alpha.2).
3. Update `CHANGELOG.md` and finalize release notes draft.
4. Confirm release notes include resolved blocking issues and known limitations link.
5. Create and push a tag:

```bash
git switch main
git pull --ff-only origin main
git tag v0.1.0-alpha.2
git push origin v0.1.0-alpha.2
```

6. Wait for the `Release` workflow to finish.
7. Verify GitHub Release contains:
- 4 target archives (`penny-cli-vX.Y.Z-<target>.tar.gz`)
- 4 checksum files (`.sha256`)
- `CHECKSUMS.txt`

For other versions, replace the tag value and keep the same gate sequence.

## Artifact Verification

Download one artifact and verify:

```bash
shasum -a 256 -c penny-cli-v0.1.0-alpha.2-x86_64-unknown-linux-gnu.sha256
```

If `shasum` is unavailable, use `sha256sum -c`.

## Installer Path

Installer script:

```text
scripts/install.sh
```

Supported env vars:

- `PENNY_VERSION` (example: `v0.1.0-alpha.1`)
- `PENNY_INSTALL_DIR` (default: `~/.local/bin`)
- `PENNY_REPO` (default: `manuelpenazuniga/PennyPrompt`)

Usage:

```bash
curl -fsSL https://raw.githubusercontent.com/manuelpenazuniga/PennyPrompt/main/scripts/install.sh | sh
```

Pinned version:

```bash
curl -fsSL https://raw.githubusercontent.com/manuelpenazuniga/PennyPrompt/main/scripts/install.sh | PENNY_VERSION=v0.1.0-alpha.2 sh
```
