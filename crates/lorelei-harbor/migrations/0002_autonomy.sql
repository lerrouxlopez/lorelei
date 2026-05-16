-- Autonomous tasks + approval requests (bounded autonomy)

create table if not exists autonomous_tasks (
  id uuid primary key,
  tenant_id text not null,
  agent_id text not null,
  prompt text not null,
  status text not null,
  schedule_kind text not null,
  schedule_at text not null,
  next_run_at timestamptz not null,
  last_run_at timestamptz null,
  locked_until timestamptz null,
  locked_by uuid null,
  last_error text null,
  consecutive_failures int not null default 0,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists autonomous_tasks_due_idx
  on autonomous_tasks (tenant_id, status, next_run_at);

create table if not exists task_run_links (
  task_id uuid not null references autonomous_tasks(id),
  run_id uuid not null references runs(id),
  created_at timestamptz not null default now(),
  primary key (task_id, run_id)
);

create table if not exists approvals (
  id uuid primary key,
  tenant_id text not null,
  agent_id text not null,
  task_id uuid null references autonomous_tasks(id),
  run_id uuid not null references runs(id),
  tool text not null,
  input jsonb not null,
  risk_level text not null,
  approval_prompt text not null,
  state text not null,
  created_at timestamptz not null default now(),
  decided_at timestamptz null
);

create index if not exists approvals_tenant_state_created_at_idx
  on approvals (tenant_id, state, created_at);

create unique index if not exists approvals_unique_per_run_tool_idx
  on approvals (run_id, tool)
  where state = 'pending';
