# Release Process (Alpha)

This document defines the repeatable process for alpha releases.

## Release Gates

- Generic alpha manual checklist: [`docs/ALPHA-MANUAL-CHECKLIST.md`](./ALPHA-MANUAL-CHECKLIST.md)
- Targeted gate for current cut: [`docs/RELEASE_GATE_v0.1.0-alpha.3.md`](./RELEASE_GATE_v0.1.0-alpha.3.md)
- Release notes for current cut: [`docs/release-notes/v0.1.0-alpha.3.md`](./release-notes/v0.1.0-alpha.3.md)
- Release history reconciliation note: [`docs/release-audit/2026-04-30-release-history-reconciliation.md`](./release-audit/2026-04-30-release-history-reconciliation.md)

## Scope

Current release automation builds and publishes `penny-cli` binaries for:

- Linux `x86_64-unknown-linux-gnu`
- Linux `aarch64-unknown-linux-gnu`
- macOS `aarch64-apple-darwin`

When a release tag is pushed and the workflow succeeds, artifacts are published as `.tar.gz` plus SHA-256 checksums.

Intel macOS (`x86_64-apple-darwin`) is not part of the default CI release matrix because the `macos-13` runner has repeatedly blocked alpha publication. If an Intel-Mac artifact is required for a cut, use the local backfill procedure below and disclose the artifact provenance in the release notes.

## Workflow Trigger

Release workflow file:

```text
.github/workflows/release.yml
```

Trigger condition:

- push a tag that starts with `v` (example: `v0.1.0-alpha.3`)

Manual trigger is also available via `workflow_dispatch`.

## Cut a Release

1. Ensure `main` is green and synchronized.
2. Complete the active release gate checklist (`RELEASE_GATE_v0.1.0-alpha.3.md` for alpha.3).
3. Update `CHANGELOG.md` and finalize release notes.
4. Confirm release notes include resolved blocking issues and known limitations link.
5. Create and push a tag:

```bash
git switch main
git pull --ff-only origin main
git tag v0.1.0-alpha.3
git push origin v0.1.0-alpha.3
```

6. Wait for the `Release` workflow to finish.
7. Verify GitHub Release contains:
- 3 CI target archives (`penny-cli-vX.Y.Z-<target>.tar.gz`)
- 3 checksum files (`.sha256`)
- `CHECKSUMS.txt`

If an Intel-Mac artifact is backfilled for the release, verify the Release contains 4 archives, 4 checksum files, and an updated `CHECKSUMS.txt`.

For other versions, replace the tag value and keep the same gate sequence.

## Artifact Verification

Download one artifact and verify:

```bash
shasum -a 256 -c penny-cli-v0.1.0-alpha.3-x86_64-unknown-linux-gnu.sha256
```

If `shasum` is unavailable, use `sha256sum -c`.

## Installer Path

Installer script:

```text
scripts/install.sh
```

Supported env vars:

- `PENNY_VERSION` (example: `v0.1.0-alpha.3`)
- `PENNY_INSTALL_DIR` (default: `~/.local/bin`)
- `PENNY_REPO` (default: `manuelpenazuniga/PennyPrompt`)

Usage:

```bash
curl -fsSL https://raw.githubusercontent.com/manuelpenazuniga/PennyPrompt/main/scripts/install.sh | sh
```

Pinned version:

```bash
curl -fsSL https://raw.githubusercontent.com/manuelpenazuniga/PennyPrompt/main/scripts/install.sh | PENNY_VERSION=v0.1.0-alpha.3 sh
```

## Release Maturity Policy

Alpha cuts are published as GitHub **Pre-release** by design. The `release.yml` workflow sets `prerelease: true` for the `softprops/action-gh-release` step, which means alpha tags do not receive the GitHub `Latest` badge and `GET /releases/latest` returns 404 until a non-alpha tag is published. This is intentional: the badge implies stability we do not yet claim. Promotion of a future stable cut to `Latest` will be handled either by removing the prerelease flag in `release.yml` for non-prerelease tags or by editing the published Release with `gh release edit <tag> --prerelease=false --latest`.

## Operational Fallback: Local Build + Upload

Intel macOS is intentionally outside the default release matrix. If the project needs an `x86_64-apple-darwin` artifact for a specific cut, it can be backfilled locally and uploaded to the same Release. Apple Silicon hosts can produce `x86_64-apple-darwin` natively because the Apple toolchain ships a multi-arch SDK; no `osxcross` or `cargo-zigbuild` is needed.

The procedure used for `v0.1.0-alpha.2` (#181) and `v0.1.0-alpha.3`:

```bash
# 1. Sync main, then check out the exact tag commit (so Cargo.lock and source match).
git switch main && git pull --ff-only origin main
git checkout v0.1.0-alpha.2

# 2. Add the target the first time.
rustup target add x86_64-apple-darwin

# 3. Build with --locked so the lockfile from the tag is enforced.
cargo build --release -p penny-cli --target x86_64-apple-darwin --locked

# 4. Package exactly as release.yml does.
VERSION=v0.1.0-alpha.2
TARGET=x86_64-apple-darwin
ASSET="penny-cli-${VERSION}-${TARGET}"
mkdir -p dist
cp "target/${TARGET}/release/penny-cli" "dist/penny-cli"
tar -C dist -czf "dist/${ASSET}.tar.gz" penny-cli
( cd dist && shasum -a 256 "${ASSET}.tar.gz" > "${ASSET}.sha256" )
rm -f dist/penny-cli

# 5. Upload artifacts to the existing GitHub Release.
gh release upload "${VERSION}" --repo manuelpenazuniga/PennyPrompt \
  "dist/${ASSET}.tar.gz" "dist/${ASSET}.sha256"

# 6. Regenerate aggregated CHECKSUMS.txt (alphabetical-by-filename).
mkdir -p dist-checksums
gh release download "${VERSION}" --repo manuelpenazuniga/PennyPrompt \
  --pattern '*.sha256' --dir dist-checksums --clobber
( cd dist-checksums && cat *.sha256 > CHECKSUMS.txt )
gh release upload "${VERSION}" --repo manuelpenazuniga/PennyPrompt \
  --clobber dist-checksums/CHECKSUMS.txt

# 7. Verify end-to-end from a fresh temp dir.
VD=$(mktemp -d) && cd "$VD"
gh release download "${VERSION}" --repo manuelpenazuniga/PennyPrompt \
  --pattern "penny-cli-${VERSION}-${TARGET}.*"
shasum -a 256 -c "penny-cli-${VERSION}-${TARGET}.sha256"
```

Locally backfilled artifacts have a different host/SDK provenance from CI-built artifacts; this difference must be disclosed in the corresponding `docs/release-notes/<tag>.md` file under an `### Artifact provenance` section.
