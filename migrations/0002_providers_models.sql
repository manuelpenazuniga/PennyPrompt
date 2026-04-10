CREATE TABLE providers (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    base_url    TEXT NOT NULL,
    api_format  TEXT NOT NULL DEFAULT 'openai',
    enabled     INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE models (
    id              TEXT PRIMARY KEY,
    provider_id     TEXT NOT NULL REFERENCES providers(id),
    external_name   TEXT NOT NULL,
    display_name    TEXT NOT NULL,
    class           TEXT NOT NULL DEFAULT 'balanced'
);
