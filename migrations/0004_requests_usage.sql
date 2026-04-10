CREATE TABLE requests (
    id              TEXT PRIMARY KEY,
    session_id      TEXT REFERENCES sessions(id),
    project_id      TEXT NOT NULL REFERENCES projects(id),
    model_requested TEXT NOT NULL,
    model_used      TEXT NOT NULL,
    provider_id     TEXT NOT NULL,
    started_at      TEXT NOT NULL,
    completed_at    TEXT,
    status          TEXT NOT NULL DEFAULT 'pending',
    is_streaming    INTEGER NOT NULL DEFAULT 0,
    upstream_ms     INTEGER
);

CREATE TABLE request_usage (
    request_id          TEXT PRIMARY KEY REFERENCES requests(id),
    input_tokens        INTEGER NOT NULL,
    output_tokens       INTEGER NOT NULL,
    cost_usd            REAL NOT NULL,
    pricing_snapshot    TEXT NOT NULL,
    source              TEXT NOT NULL DEFAULT 'provider'
);

CREATE INDEX idx_requests_project ON requests(project_id, started_at);
CREATE INDEX idx_requests_session ON requests(session_id);
