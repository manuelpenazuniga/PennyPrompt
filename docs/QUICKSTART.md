# PennyPrompt Quickstart (Alpha)

Goal: get from zero to actionable local operator output in about 5-10 minutes.

## 1. Build the CLI

```bash
git clone https://github.com/manuelpenazuniga/PennyPrompt.git
cd PennyPrompt
cargo build --release -p penny-cli
```

Use one of these command styles in the rest of this guide:

- `./target/release/penny-cli <command>`
- `cargo run -p penny-cli -- <command>`

## 2. Initialize Configuration

```bash
./target/release/penny-cli init --preset indie
```

Available presets:

- `indie`
- `team`
- `explore`

If the file already exists and you want to overwrite:

```bash
./target/release/penny-cli init --preset indie --force
```

## 3. Set Provider Keys

```bash
export ANTHROPIC_API_KEY=...
export OPENAI_API_KEY=...
```

## 4. Import Local Pricebook

```bash
./target/release/penny-cli prices update
./target/release/penny-cli prices show --limit 20
```

## 5. Run Health and Config Checks

```bash
./target/release/penny-cli doctor
./target/release/penny-cli config --json
```

## 6. Try Core Operator Commands

Estimate:

```bash
./target/release/penny-cli estimate \
  --model claude-sonnet-4-6 \
  --context-files "src/**/*.rs" \
  --task-type multi-round
```

Budget overview:

```bash
./target/release/penny-cli budget list
```

Detect control plane:

```bash
./target/release/penny-cli detect status
```

Reports:

```bash
./target/release/penny-cli report summary --since 1d
./target/release/penny-cli report top --limit 20
```

## 7. Optional Live Monitoring (if admin plane is running)

```bash
./target/release/penny-cli tail --admin-url http://127.0.0.1:8586
```

Resume a paused session:

```bash
./target/release/penny-cli detect resume <session_id>
```

## Expected First Outcomes

- `doctor` shows config and DB status.
- `prices show` lists active models and rates.
- `estimate` returns min/max range and budget impact.
- `report` commands return either data or explicit "no usage rows" output.

If something is unclear, continue in:

- [INSTALL.md](./INSTALL.md)
- [CONFIG-REFERENCE.md](./CONFIG-REFERENCE.md)
- [LIMITATIONS.md](./LIMITATIONS.md)
