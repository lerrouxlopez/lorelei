-- Lorelei Reef (Postgres) initial schema

create table if not exists runs (
  id uuid primary key,
  tenant_id text not null,
  agent_id text not null,
  goal text not null,
  status text not null,
  created_at timestamptz not null default now(),
  completed_at timestamptz null
);

create table if not exists currents (
  id uuid primary key,
  tenant_id text not null,
  run_id uuid not null references runs(id),
  agent_id text not null,
  event_type text not null,
  content jsonb not null,
  created_at timestamptz not null default now()
);

create index if not exists currents_tenant_run_created_at_idx
  on currents (tenant_id, run_id, created_at);

create table if not exists pearls (
  id uuid primary key,
  tenant_id text not null,
  agent_id text not null,
  pearl_type text not null,
  content text not null,
  source_current_id uuid null references currents(id),
  confidence real not null check (confidence >= 0.0 and confidence <= 1.0),
  importance real not null check (importance >= 0.0 and importance <= 1.0),
  tags text[] not null default '{}',
  created_at timestamptz not null default now(),
  last_echoed_at timestamptz null,
  deleted_at timestamptz null
);

-- Active pearls = not deleted
create index if not exists pearls_active_tenant_type_idx
  on pearls (tenant_id, pearl_type)
  where deleted_at is null;

create index if not exists pearls_deleted_at_idx
  on pearls (deleted_at)
  where deleted_at is not null;

create table if not exists shell_calls (
  id uuid primary key,
  tenant_id text not null,
  run_id uuid not null references runs(id),
  current_id uuid not null references currents(id),
  shell_name text not null,
  input jsonb not null,
  output jsonb null,
  status text not null,
  risk_level text not null,
  created_at timestamptz not null default now()
);

create index if not exists shell_calls_tenant_run_idx
  on shell_calls (tenant_id, run_id);

