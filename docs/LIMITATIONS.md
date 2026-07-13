# Known Limitations (Alpha)

This list documents current constraints as of July 12, 2026.

## Inbound API Surface

- The proxy accepts the **OpenAI-compatible** `POST /v1/chat/completions` surface plus
  `GET /v1/models`.
- **Resolved (July 12, 2026, `#207`):** native Anthropic ingress (`POST /v1/messages`) is now
  implemented — Anthropic-native agents (OpenClaw, claw-code, Claude-family SDKs) connect with
  `ANTHROPIC_BASE_URL=http://localhost:8585` and zero translation, including streaming and tool
  use. The response shape returned matches the ingress contract (Anthropic in → Anthropic out).

## Cost Accuracy

- **Resolved (July 12, 2026, `#208`):** prompt-cache tokens are now included in cost accounting.
  Provider `cache_read_input_tokens` / `cache_creation_input_tokens` (Anthropic) and
  `prompt_tokens_details.cached_tokens` (OpenAI) are read from both non-streaming and streaming
  responses and priced with dedicated cache-read / cache-write rates, so reported cost matches
  the provider invoice on cache-heavy agent workloads. Models without cache rates fall back to
  the standard input rate (logged at debug), never dropping tokens.
- Streaming reconciliation falls back to token estimation when the provider omits a final usage
  payload; provider-reported usage is preferred when present.

## CLI / Product Surface

- `serve` is available in `penny-cli` for foreground and local background operation.
- `run <agent>` supports deterministic dry-run launch plans and a minimal `--execute` path.
- `run --execute` is limited to local agents that honor OpenAI-compatible `/v1` base URL environment variables; native agent protocols are outside the current alpha scope.
- Some outputs are operator-focused and intentionally minimal (not final UX polish).

## Pricebook Update

- `prices update` currently imports from local repository TOML files.
- There is no remote signed feed sync in the current alpha branch.

## Runtime Topology

- Default setup assumes a local single-node SQLite database.
- Team/multi-node coordination is out of scope for current alpha.

## API/Control Plane Assumptions

- Admin APIs have no bearer token or admin-token authentication in the current alpha; treat the admin plane as local-only.
- Use a Unix socket or loopback TCP for admin. Do not expose admin binds to LAN or public networks.
- `tail` and `detect` client commands are HTTP-only today (default `http://127.0.0.1:8586`).
- Native unix-socket client connectivity for those commands is not implemented; expose admin over loopback TCP when using them.
- If admin plane is unavailable, related commands fail as expected.

## Data and Reporting

- Reports reflect local persisted usage only.
- Empty datasets produce explicit "no rows" style output (expected in fresh installs).

These limitations are intentional scope boundaries for current alpha sequencing.
