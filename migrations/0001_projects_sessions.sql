CREATE TABLE projects (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    path        TEXT UNIQUE,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE sessions (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL REFERENCES projects(id),
    started_at  TEXT NOT NULL DEFAULT (datetime('now')),
    closed_at   TEXT,
    source      TEXT NOT NULL DEFAULT 'auto'
);

CREATE INDEX idx_sessions_project ON sessions(project_id, started_at);
