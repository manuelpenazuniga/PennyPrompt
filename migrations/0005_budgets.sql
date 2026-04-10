CREATE TABLE budgets (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    scope_type      TEXT NOT NULL,
    scope_id        TEXT NOT NULL,
    window_type     TEXT NOT NULL,
    hard_limit_usd  REAL,
    soft_limit_usd  REAL,
    action_on_hard  TEXT NOT NULL DEFAULT 'block',
    action_on_soft  TEXT NOT NULL DEFAULT 'warn',
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    preset_source   TEXT
);

CREATE INDEX idx_budgets_scope ON budgets(scope_type, scope_id, window_type);
