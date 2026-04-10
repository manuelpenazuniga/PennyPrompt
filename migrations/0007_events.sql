CREATE TABLE events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id  TEXT,
    session_id  TEXT,
    event_type  TEXT NOT NULL,
    severity    TEXT NOT NULL DEFAULT 'info',
    detail      TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_events_type ON events(event_type, created_at);
CREATE INDEX idx_events_session ON events(session_id, created_at);
