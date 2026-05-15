DROP INDEX IF EXISTS pearls_last_echoed_at_idx;
DROP INDEX IF EXISTS pearls_tags_gin_idx;
DROP INDEX IF EXISTS pearls_agent_id_idx;

ALTER TABLE pearls
  DROP COLUMN IF EXISTS last_echoed_at,
  DROP COLUMN IF EXISTS metadata,
  DROP COLUMN IF EXISTS tags,
  DROP COLUMN IF EXISTS agent_id;

