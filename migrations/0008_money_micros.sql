-- Introduce deterministic integer money columns (micro-USD) while keeping
-- legacy REAL columns for compatibility during rollout.

ALTER TABLE budgets ADD COLUMN hard_limit_micros INTEGER;
ALTER TABLE budgets ADD COLUMN soft_limit_micros INTEGER;

UPDATE budgets
SET
    hard_limit_micros = CASE
        WHEN hard_limit_usd IS NULL THEN NULL
        ELSE CAST(ROUND(hard_limit_usd * 1000000.0) AS INTEGER)
    END,
    soft_limit_micros = CASE
        WHEN soft_limit_usd IS NULL THEN NULL
        ELSE CAST(ROUND(soft_limit_usd * 1000000.0) AS INTEGER)
    END;

ALTER TABLE request_usage ADD COLUMN cost_micros INTEGER NOT NULL DEFAULT 0;

UPDATE request_usage
SET cost_micros = CAST(ROUND(cost_usd * 1000000.0) AS INTEGER);

ALTER TABLE cost_ledger ADD COLUMN amount_micros INTEGER NOT NULL DEFAULT 0;
ALTER TABLE cost_ledger ADD COLUMN running_total_micros INTEGER NOT NULL DEFAULT 0;

UPDATE cost_ledger
SET
    amount_micros = CAST(ROUND(amount_usd * 1000000.0) AS INTEGER),
    running_total_micros = CAST(ROUND(running_total * 1000000.0) AS INTEGER);
