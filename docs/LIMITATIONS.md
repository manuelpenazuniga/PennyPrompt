# Known Limitations (Alpha)

This list documents current constraints as of April 18, 2026.

## CLI / Product Surface

- `serve` is available in `penny-cli`, but daemon/background mode is not implemented yet.
- `run <agent>` currently emits deterministic dry-run launch plans; full process orchestration remains a follow-up.
- Some outputs are operator-focused and intentionally minimal (not final UX polish).

## Pricebook Update

- `prices update` currently imports from local repository TOML files.
- There is no remote signed feed sync in the current alpha branch.

## Runtime Topology

- Default setup assumes a local single-node SQLite database.
- Team/multi-node coordination is out of scope for current alpha.

## API/Control Plane Assumptions

- `tail` and `detect` control commands assume admin API availability over HTTP (default `http://127.0.0.1:8586` in CLI commands).
- If `serve` runs admin on a Unix socket path, use `--admin-bind 127.0.0.1:8586` (or equivalent TCP bind) for those commands.
- If admin plane is unavailable, related commands fail as expected.

## Data and Reporting

- Reports reflect local persisted usage only.
- Empty datasets produce explicit "no rows" style output (expected in fresh installs).

These limitations are intentional scope boundaries for current alpha sequencing.
