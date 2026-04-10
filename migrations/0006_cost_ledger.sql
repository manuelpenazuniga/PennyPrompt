CREATE TABLE cost_ledger (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id      TEXT NOT NULL,
    entry_type      TEXT NOT NULL,
    budget_id       INTEGER NOT NULL REFERENCES budgets(id),
    amount_usd      REAL NOT NULL,
    running_total   REAL NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_ledger_budget ON cost_ledger(budget_id, created_at);
CREATE INDEX idx_ledger_request ON cost_ledger(request_id);
