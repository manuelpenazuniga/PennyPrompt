# PennyPrompt Alpha Installation

This document describes the current alpha installation path for this repository.

## 1. Prerequisites

- macOS or Linux shell environment
- Rust toolchain (stable) with `cargo`
- SQLite support (already included via `sqlx` + bundled SQLite driver)

Check toolchain:

```bash
rustc --version
cargo --version
```

## 2. Clone and Build

```bash
git clone https://github.com/manuelpenazuniga/PennyPrompt.git
cd PennyPrompt
cargo build --release -p penny-cli
```

The binary is generated at:

```text
target/release/penny-cli
```

## 3. Run Commands

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

## 4. Configure Initial Settings

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

## 5. Seed Pricebook and Verify

```bash
./target/release/penny-cli prices update
./target/release/penny-cli doctor
./target/release/penny-cli config --json
```

## 6. API Key Environment Variables

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
