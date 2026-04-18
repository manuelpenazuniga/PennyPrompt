# Known Limitations (Alpha)

This list documents current constraints as of April 18, 2026.

## CLI / Product Surface

- `serve` lifecycle orchestration is not fully consolidated in `penny-cli` yet.
- Some outputs are operator-focused and intentionally minimal (not final UX polish).

## Pricebook Update

- `prices update` currently imports from local repository TOML files.
- There is no remote signed feed sync in the current alpha branch.

## Runtime Topology

- Default setup assumes a local single-node SQLite database.
- Team/multi-node coordination is out of scope for current alpha.

## API/Control Plane Assumptions

- `tail` and `detect` control commands assume admin API availability (default `http://127.0.0.1:8586` in CLI commands).
- If admin plane is unavailable, related commands fail as expected.

## Data and Reporting

- Reports reflect local persisted usage only.
- Empty datasets produce explicit "no rows" style output (expected in fresh installs).

These limitations are intentional scope boundaries for current alpha sequencing.
