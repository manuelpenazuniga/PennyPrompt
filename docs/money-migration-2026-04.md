# Money Hardening Migration Notes (Issue #45)

## What changed
- Introduced deterministic `Money` type in `penny-types` using integer micro-USD precision (`1 USD = 1_000_000 micros`).
- Accounting-critical structs now use `Money` instead of `f64`:
  - budgets (`hard_limit_usd`, `soft_limit_usd`)
  - ledger entries (`amount_usd`, `running_total`)
  - reservation/block details (`accumulated_usd`, `limit_usd`, `remaining_usd`)
  - accounted usage (`cost_usd`)
- Budget/ledger math now uses exact integer arithmetic.

## Database compatibility strategy
- Added migration `0008_money_micros.sql`.
- Added migration `0008_money_micros.sql`.
- Added migration `0009_pricebook_micros.sql`.
- New deterministic columns were introduced and backfilled from legacy `REAL` columns:
  - `budgets.hard_limit_micros`, `budgets.soft_limit_micros`
  - `request_usage.cost_micros`
  - `cost_ledger.amount_micros`, `cost_ledger.running_total_micros`
  - `pricebook_entries.input_per_mtok_micros`, `pricebook_entries.output_per_mtok_micros`
- Existing `REAL` columns remain during transition for compatibility and diagnostics.
- Repositories now read/write the `*_micros` columns for accounting logic, and still dual-write legacy `REAL` columns where relevant.

## Operational notes
- Existing databases are migrated in-place via SQLx migration runner at startup.
- New environments receive all migrations, including `0008`, automatically.
- For forensic checks, `*_micros` values are now the source of truth.
