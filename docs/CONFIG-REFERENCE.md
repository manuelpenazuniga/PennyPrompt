# PennyPrompt Config Reference (Alpha)

This reference reflects the current runtime behavior in `penny-config`.

## Resolution Order

Config is resolved in this order (later overrides earlier):

1. `config/default.toml`
2. `presets/<name>.toml` (when `--preset` is used)
3. user config file (`PENNY_CONFIG` or `~/.config/pennyprompt/config.toml`)
4. selected `PENNY_*` environment overrides

## User Config Path

- explicit: `PENNY_CONFIG=/absolute/path/config.toml`
- default: `~/.config/pennyprompt/config.toml`

## Top-Level Sections

- `[server]`
- `[defaults]`
- `[attribution]`
- `[providers.anthropic]`
- `[providers.openai]`
- `[[budgets]]` (one or more)
- `[detect]`
- `[cleanup]`

## Section Reference

## `[server]`

- `bind` (`string`): proxy bind address (example `127.0.0.1:8585`)
- `admin_socket` (`string`): admin bind target
  - if value is `host:port`, admin binds TCP
  - otherwise admin binds a Unix socket path
- `database_path` (`string`): SQLite database path
- `mode` (`observe|guard`)

Operational note:

- `tail` / `detect` CLI commands use HTTP URLs and default to `http://127.0.0.1:8586`.
- If you keep `admin_socket` as a Unix path, start serve with `--admin-bind 127.0.0.1:8586` for those commands, or pass an explicit reachable admin URL.

## `[defaults]`

- `provider` (`string`): default provider id
- `model` (`string`): default model id

## `[attribution]`

- `auto_detect_project` (`bool`)
- `session_window_minutes` (`u32`, must be `> 0`)

## `[providers.<name>]`

- `enabled` (`bool`)
- `base_url` (`string`, valid URL required when enabled)
- `api_key_env` (`string`, required when enabled)
- `api_format` (`string`, required when enabled)

## `[[budgets]]`

- `scope_type` (`global|project|session`)
- `scope_id` (`string`, non-empty)
- `window_type` (`day|week|month|total`)
- `hard_limit_usd` (`number`, optional, must be `> 0`)
- `soft_limit_usd` (`number`, optional, must be `> 0`)
- `action_on_hard` (`string`, default `block`)
- `action_on_soft` (`string`, default `warn`)
- `preset_source` (`string`, optional)

Validation rule:

- if both are set: `soft_limit_usd <= hard_limit_usd`

## `[detect]`

- `enabled` (`bool`)
- `burn_rate_alert_usd_per_hour` (`number`)
- `loop_window_seconds` (`u64`)
- `loop_threshold_similar_requests` (`u32`)
- `loop_action` (`alert|pause`)

## `[cleanup]`

- `strip_ansi` (`bool`, default `true`): remove ANSI escape sequences from text payloads.
- `minify_json` (`bool`, default `false`): attempts to minify string fields that contain valid JSON; treat as experimental opt-in.

## Presets

Current preset files:

- `presets/indie.toml`
- `presets/team.toml`
- `presets/explore.toml`

Preset budgets are tagged internally with `preset:<name>` when applied.

## Environment Overrides (currently implemented)

- `PENNY_CONFIG`
- `PENNY_SERVER_BIND`
- `PENNY_SERVER_MODE` (`observe|guard`)
- `PENNY_DEFAULTS_PROVIDER`
- `PENNY_DEFAULTS_MODEL`
- `PENNY_ATTRIBUTION_AUTO_DETECT_PROJECT` (`true|false|1|0|yes|no|on|off`)
- `PENNY_ATTRIBUTION_SESSION_WINDOW_MINUTES` (integer)
- `PENNY_CLEANUP_STRIP_ANSI` (`true|false|1|0|yes|no|on|off`)
- `PENNY_CLEANUP_MINIFY_JSON` (`true|false|1|0|yes|no|on|off`)

## Example Minimal Config

```toml
[server]
bind = "127.0.0.1:8585"
admin_socket = "~/.local/share/pennyprompt/admin.sock"
database_path = "~/.local/share/pennyprompt/penny.db"
mode = "guard"

[defaults]
provider = "anthropic"
model = "claude-sonnet-4-6"

[attribution]
auto_detect_project = true
session_window_minutes = 30

[providers.anthropic]
enabled = true
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
api_format = "anthropic"

[providers.openai]
enabled = true
base_url = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
api_format = "openai"

[[budgets]]
scope_type = "global"
scope_id = "*"
window_type = "day"
hard_limit_usd = 10.0
action_on_hard = "block"
action_on_soft = "warn"

[detect]
enabled = true
burn_rate_alert_usd_per_hour = 10.0
loop_window_seconds = 120
loop_threshold_similar_requests = 8
loop_action = "pause"

[cleanup]
strip_ansi = true
minify_json = false
```
