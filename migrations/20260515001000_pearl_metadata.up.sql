-- Add pearl metadata needed by lorelei-lore and vector indexing.

ALTER TABLE pearls
  ADD COLUMN IF NOT EXISTS agent_id uuid NULL,
  ADD COLUMN IF NOT EXISTS tags text[] NOT NULL DEFAULT '{}'::text[],
  ADD COLUMN IF NOT EXISTS metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
  ADD COLUMN IF NOT EXISTS last_echoed_at timestamptz NULL;

CREATE INDEX IF NOT EXISTS pearls_agent_id_idx ON pearls (agent_id);
CREATE INDEX IF NOT EXISTS pearls_tags_gin_idx ON pearls USING gin (tags);
CREATE INDEX IF NOT EXISTS pearls_last_echoed_at_idx ON pearls (last_echoed_at);

