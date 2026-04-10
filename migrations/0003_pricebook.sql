CREATE TABLE pricebook_entries (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    model_id        TEXT NOT NULL REFERENCES models(id),
    input_per_mtok  REAL NOT NULL,
    output_per_mtok REAL NOT NULL,
    effective_from  TEXT NOT NULL,
    effective_until TEXT,
    source          TEXT NOT NULL DEFAULT 'local'
);

CREATE INDEX idx_pricebook_model ON pricebook_entries(model_id, effective_from);
