-- Extend pricebook entries with deterministic micro-USD rates while keeping
-- legacy REAL columns for backward compatibility during rollout.

ALTER TABLE pricebook_entries ADD COLUMN input_per_mtok_micros INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pricebook_entries ADD COLUMN output_per_mtok_micros INTEGER NOT NULL DEFAULT 0;

UPDATE pricebook_entries
SET
    input_per_mtok_micros = CAST(ROUND(input_per_mtok * 1000000.0) AS INTEGER),
    output_per_mtok_micros = CAST(ROUND(output_per_mtok * 1000000.0) AS INTEGER);
