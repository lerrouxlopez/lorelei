-- Lorelei initial schema (PostgreSQL).

-- Domain enums
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'pearl_type') THEN
    CREATE TYPE pearl_type AS ENUM ('memory', 'note', 'insight');
  END IF;
END
$$;

CREATE TABLE IF NOT EXISTS runs (
  id uuid PRIMARY KEY,
  tenant_id uuid NOT NULL,

  started_at timestamptz NOT NULL,
  ended_at timestamptz NULL,

  metadata jsonb NOT NULL DEFAULT '{}'::jsonb,

  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS runs_tenant_id_idx ON runs (tenant_id);
CREATE INDEX IF NOT EXISTS runs_created_at_idx ON runs (created_at);

CREATE TABLE IF NOT EXISTS currents (
  id uuid PRIMARY KEY,
  tenant_id uuid NOT NULL,
  run_id uuid NOT NULL REFERENCES runs(id),

  kind text NOT NULL,
  content jsonb NOT NULL DEFAULT '{}'::jsonb,

  confidence double precision NULL,
  importance double precision NULL,

  created_at timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE currents
  ADD CONSTRAINT currents_confidence_range CHECK (confidence IS NULL OR (confidence >= 0.0 AND confidence <= 1.0)),
  ADD CONSTRAINT currents_importance_range CHECK (importance IS NULL OR (importance >= 0.0 AND importance <= 1.0));

CREATE INDEX IF NOT EXISTS currents_tenant_id_idx ON currents (tenant_id);
CREATE INDEX IF NOT EXISTS currents_run_id_idx ON currents (run_id);
CREATE INDEX IF NOT EXISTS currents_created_at_idx ON currents (created_at);

CREATE TABLE IF NOT EXISTS pearls (
  id uuid PRIMARY KEY,
  tenant_id uuid NOT NULL,
  run_id uuid NOT NULL REFERENCES runs(id),

  pearl_type pearl_type NOT NULL,
  content text NOT NULL,

  confidence double precision NULL,
  importance double precision NULL,

  created_at timestamptz NOT NULL DEFAULT now(),
  deleted_at timestamptz NULL
);

ALTER TABLE pearls
  ADD CONSTRAINT pearls_confidence_range CHECK (confidence IS NULL OR (confidence >= 0.0 AND confidence <= 1.0)),
  ADD CONSTRAINT pearls_importance_range CHECK (importance IS NULL OR (importance >= 0.0 AND importance <= 1.0));

CREATE INDEX IF NOT EXISTS pearls_tenant_id_idx ON pearls (tenant_id);
CREATE INDEX IF NOT EXISTS pearls_run_id_idx ON pearls (run_id);
CREATE INDEX IF NOT EXISTS pearls_pearl_type_idx ON pearls (pearl_type);
CREATE INDEX IF NOT EXISTS pearls_created_at_idx ON pearls (created_at);
CREATE INDEX IF NOT EXISTS pearls_deleted_at_idx ON pearls (deleted_at);

CREATE TABLE IF NOT EXISTS shell_calls (
  id uuid PRIMARY KEY,
  tenant_id uuid NOT NULL,
  run_id uuid NOT NULL REFERENCES runs(id),

  input jsonb NOT NULL,
  output jsonb NULL,

  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS shell_calls_tenant_id_idx ON shell_calls (tenant_id);
CREATE INDEX IF NOT EXISTS shell_calls_run_id_idx ON shell_calls (run_id);
CREATE INDEX IF NOT EXISTS shell_calls_created_at_idx ON shell_calls (created_at);

