#!/bin/sh
set -eu

REPO="${PENNY_REPO:-manuelpenazuniga/PennyPrompt}"
INSTALL_DIR="${PENNY_INSTALL_DIR:-${HOME:-$PWD}/.local/bin}"
# The shipped binary is `pennyprompt`. Releases at or before the rename
# (v0.1.0-alpha.4 and older) shipped `penny-cli`; the installer falls back to
# that legacy asset name so pinned older tags stay installable.
BIN_NAME="pennyprompt"
LEGACY_BIN_NAME="penny-cli"

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: required command not found: $1" >&2
    exit 1
  fi
}

detect_os() {
  uname_s="$(uname -s)"
  case "$uname_s" in
    Linux) echo "unknown-linux-gnu" ;;
    Darwin) echo "apple-darwin" ;;
    *)
      echo "error: unsupported OS: $uname_s" >&2
      exit 1
      ;;
  esac
}

detect_arch() {
  uname_m="$(uname -m)"
  case "$uname_m" in
    x86_64|amd64) echo "x86_64" ;;
    arm64|aarch64) echo "aarch64" ;;
    *)
      echo "error: unsupported architecture: $uname_m" >&2
      exit 1
      ;;
  esac
}

resolve_version() {
  if [ -n "${PENNY_VERSION:-}" ]; then
    echo "$PENNY_VERSION"
    return
  fi

  latest_json="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest")"
  version="$(echo "$latest_json" | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
  if [ -z "$version" ]; then
    echo "error: unable to resolve latest release tag from GitHub API" >&2
    exit 1
  fi
  echo "$version"
}

verify_checksum() {
  checksum_file="$1"
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -c "$checksum_file"
    return
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c "$checksum_file"
    return
  fi
  echo "error: need shasum or sha256sum to verify checksums" >&2
  exit 1
}

main() {
  need_cmd curl
  need_cmd tar
  need_cmd mktemp

  os="$(detect_os)"
  arch="$(detect_arch)"
  target="${arch}-${os}"
  version="$(resolve_version)"

  base_url="https://github.com/${REPO}/releases/download/${version}"
  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "$tmp_dir"' EXIT INT TERM

  # Prefer the current `pennyprompt-*` asset. Fall back to the legacy
  # `penny-cli-*` asset for releases published before the rename.
  bin_name="$BIN_NAME"
  asset="${BIN_NAME}-${version}-${target}.tar.gz"
  checksum="${BIN_NAME}-${version}-${target}.sha256"
  if ! curl -fsSL "${base_url}/${asset}" -o "${tmp_dir}/${asset}" 2>/dev/null; then
    echo "note: ${asset} not found; falling back to legacy ${LEGACY_BIN_NAME} asset for ${version}"
    bin_name="$LEGACY_BIN_NAME"
    asset="${LEGACY_BIN_NAME}-${version}-${target}.tar.gz"
    checksum="${LEGACY_BIN_NAME}-${version}-${target}.sha256"
    curl -fsSL "${base_url}/${asset}" -o "${tmp_dir}/${asset}"
  fi
  curl -fsSL "${base_url}/${checksum}" -o "${tmp_dir}/${checksum}"

  echo "Installing ${bin_name} ${version} for ${target}"
  (
    cd "$tmp_dir"
    verify_checksum "$checksum"
    tar -xzf "$asset"
  )

  mkdir -p "$INSTALL_DIR"
  if command -v install >/dev/null 2>&1; then
    install -m 755 "${tmp_dir}/${bin_name}" "${INSTALL_DIR}/${bin_name}"
  else
    cp "${tmp_dir}/${bin_name}" "${INSTALL_DIR}/${bin_name}"
    chmod 755 "${INSTALL_DIR}/${bin_name}"
  fi

  # One-train compatibility: expose the legacy `penny-cli` command as a symlink
  # to `pennyprompt` (removed in beta.1). Skipped when installing a legacy asset,
  # which already installs `penny-cli` directly.
  if [ "$bin_name" = "$BIN_NAME" ]; then
    ln -sf "$BIN_NAME" "${INSTALL_DIR}/${LEGACY_BIN_NAME}"
    echo "note: created legacy '${LEGACY_BIN_NAME}' -> '${BIN_NAME}' symlink (deprecated, removed in beta.1)"
  fi

  echo "Installed to ${INSTALL_DIR}/${bin_name}"
  case ":$PATH:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
      echo "warning: ${INSTALL_DIR} is not in PATH. Add this line to your shell profile:"
      echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
      ;;
  esac
}

main "$@"
