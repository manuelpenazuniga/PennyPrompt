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

## 6. Start Proxy + Admin Planes

Run `serve` in terminal A and keep it running:

```bash
./target/release/penny-cli serve --admin-bind 127.0.0.1:8586
```

This is the recommended default topology for local operator workflows because `tail` and `detect` commands default to `http://127.0.0.1:8586`.

If you want a fully local smoke test without provider keys:

```bash
./target/release/penny-cli serve --mock --admin-bind 127.0.0.1:8586
```

To run the same local topology in the background:

```bash
./target/release/penny-cli serve --daemon --mock --admin-bind 127.0.0.1:8586
./target/release/penny-cli serve --status
./target/release/penny-cli serve --stop
```

The default background pid/log files live next to the user config:
`~/.config/pennyprompt/serve.pid` and `~/.config/pennyprompt/serve.log`.

## 7. Try Core Operator Commands (terminal B)

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

Live monitoring:

```bash
./target/release/penny-cli tail
```

Resume a paused session:

```bash
./target/release/penny-cli detect resume <session_id>
```

## 8. Launcher Execution

Preview launcher attribution and runtime wiring:

```bash
./target/release/penny-cli run codex
./target/release/penny-cli run codex --json
```

Execute a local agent command through a temporary PennyPrompt proxy:

```bash
./target/release/penny-cli run codex --execute -- --help
```

Smoke the launcher without provider credentials:

```bash
./target/release/penny-cli run sh --execute --mock -- -c 'echo "$OPENAI_BASE_URL"'
```

Notes:

- without `--execute`, `run` remains a deterministic dry-run plan
- `--execute` starts a per-run loopback proxy, sets `OPENAI_BASE_URL` / `OPENAI_API_BASE`, and launches the agent process
- the alpha.4 launcher contract is OpenAI-compatible `/v1` agent traffic; native Anthropic CLI protocol routing is not claimed
- use `--project-id` / `--session-id` to override detected defaults
- process startup failures are reported by `pennyprompt run`; budget and provider failures are returned through the proxy to the agent

## Expected First Outcomes

- `doctor` shows config and DB status.
- `prices show` lists active models and rates.
- `serve` starts both proxy and admin without missing-command errors.
- `estimate` returns min/max range and budget impact.
- `report` commands return either data or explicit "no usage rows" output.

If something is unclear, continue in:

- [INSTALL.md](./INSTALL.md)
- [CONFIG-REFERENCE.md](./CONFIG-REFERENCE.md)
- [LIMITATIONS.md](./LIMITATIONS.md)
