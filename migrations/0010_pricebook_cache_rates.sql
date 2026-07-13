-- Extend pricebook entries with prompt-cache micro-USD rates. Columns are
-- nullable: a NULL rate means the model has no dedicated cache pricing, in which
-- case cache tokens are billed at the standard input rate (logged at debug).
-- Additive only; existing columns are left untouched.

ALTER TABLE pricebook_entries ADD COLUMN cache_read_per_mtok_micros INTEGER;
ALTER TABLE pricebook_entries ADD COLUMN cache_write_per_mtok_micros INTEGER;
