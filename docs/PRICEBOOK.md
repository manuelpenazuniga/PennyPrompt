# Pricebook Guide (Alpha)

PennyPrompt pricing is sourced from local TOML files and imported into SQLite.

## Source Files

- `prices/anthropic.toml`
- `prices/openai.toml`

These files are version-controlled in this repository.

## Import and Refresh

Import both files into the local database:

```bash
penny-cli prices update
```

Show active entries:

```bash
penny-cli prices show --limit 20
```

(`cargo run -p penny-cli -- prices update` works too.)

## File Format

Top-level fields:

- `provider_id`
- `provider_name`
- `api_format`
- `source`

Per entry (`[[entries]]`):

- `model_id`
- `external_name`
- `display_name`
- `class`
- `input_per_mtok`
- `output_per_mtok`
- `effective_from`
- `effective_until` (optional)

Example:

```toml
provider_id = "anthropic"
provider_name = "Anthropic"
api_format = "anthropic"
source = "local"

[[entries]]
model_id = "claude-sonnet-4-6"
external_name = "claude-sonnet-4-6"
display_name = "Claude Sonnet 4.6"
class = "balanced"
input_per_mtok = 3.0
output_per_mtok = 15.0
effective_from = "2026-04-10T00:00:00Z"
```

## Effective-Window Semantics

At runtime, price resolution selects the latest entry where:

- `effective_from <= now`
- `effective_until IS NULL OR effective_until > now`

This enables non-destructive price evolution over time.

## How to Add/Change a Model

1. Edit the corresponding provider file under `prices/`.
2. Add a new `[[entries]]` block or update with a new `effective_from`.
3. Run `penny-cli prices update`.
4. Verify with `penny-cli prices show`.
5. Run tests for pricing-sensitive components.

## Notes

- This alpha flow is local-file based; no remote fetch is required.
- Estimation/reporting reproducibility depends on imported snapshot state.
