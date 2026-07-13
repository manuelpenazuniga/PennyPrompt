-- Record prompt-cache token counts per request so cost reports can break usage
-- down into fresh input / cached read / cache write / output. `input_tokens`
-- continues to hold the fresh (non-cached) input count. Additive only.

ALTER TABLE request_usage ADD COLUMN cache_read_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE request_usage ADD COLUMN cache_creation_tokens INTEGER NOT NULL DEFAULT 0;
