# PennyPrompt Alpha Installation

This document describes alpha installation paths for PennyPrompt.

## 1. Prerequisites

- macOS or Linux shell environment
- `curl`, `tar`, and `shasum` or `sha256sum`
- Rust toolchain (stable) with `cargo` (only required for source builds)

## 2. Quick Install from GitHub Release (`curl | sh`)

Install latest alpha release:

```bash
curl -fsSL https://raw.githubusercontent.com/manuelpenazuniga/PennyPrompt/main/scripts/install.sh | sh
```

Install specific version:

```bash
curl -fsSL https://raw.githubusercontent.com/manuelpenazuniga/PennyPrompt/main/scripts/install.sh | PENNY_VERSION=v0.1.0-alpha.1 sh
```

By default the binary is installed to:

```text
~/.local/bin/penny-cli
```

Override install location:

```bash
curl -fsSL https://raw.githubusercontent.com/manuelpenazuniga/PennyPrompt/main/scripts/install.sh | PENNY_INSTALL_DIR=/usr/local/bin sh
```

## 3. Build from Source (Alternative)

Check toolchain:

```bash
rustc --version
cargo --version
```

Clone and build:

```bash
git clone https://github.com/manuelpenazuniga/PennyPrompt.git
cd PennyPrompt
cargo build --release -p penny-cli
```

The binary is generated at:

```text
target/release/penny-cli
```

## 4. Run Commands

You can either:

- run the binary directly:

```bash
./target/release/penny-cli doctor
```

- or run from source:

```bash
cargo run -p penny-cli -- doctor
```

Optional convenience alias:

```bash
alias pp='cargo run -p penny-cli --'
pp doctor
```

Launcher preview (dry-run):

```bash
./target/release/penny-cli run codex
```

## 5. Configure Initial Settings

Create a local config from a preset:

```bash
./target/release/penny-cli init --preset indie
```

Default config target:

```text
~/.config/pennyprompt/config.toml
```

You can override config path with:

```bash
export PENNY_CONFIG=/absolute/path/to/config.toml
```

## 6. Seed Pricebook and Verify

```bash
./target/release/penny-cli prices update
./target/release/penny-cli doctor
./target/release/penny-cli config --json
```

## 7. API Key Environment Variables

Set keys for providers you plan to use:

```bash
export ANTHROPIC_API_KEY=...
export OPENAI_API_KEY=...
```

`doctor` reports whether these keys are present.

## Troubleshooting

- `config already exists`: rerun `init` with `--force`.
- `HOME not set`: define `HOME` or use `PENNY_CONFIG`.
- `no active pricebook entries`: run `prices update`.
- DB connectivity errors: verify `server.database_path` and file permissions.
- `unsupported OS/architecture`: install script currently supports Linux/macOS on x86_64 and arm64.
