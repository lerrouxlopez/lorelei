-- Documents + chunk storage (text/markdown v1)

create table if not exists documents (
  id uuid primary key,
  tenant_id text not null,
  agent_id text not null,
  title text not null,
  source_uri text not null,
  mime_type text not null,
  checksum text not null,
  created_at timestamptz not null default now(),
  deleted_at timestamptz null
);

create unique index if not exists documents_unique_active_checksum_idx
  on documents (tenant_id, agent_id, checksum)
  where deleted_at is null;

create index if not exists documents_tenant_created_at_idx
  on documents (tenant_id, created_at);

create table if not exists document_chunks (
  id uuid primary key,
  document_id uuid not null references documents(id),
  tenant_id text not null,
  agent_id text not null,
  chunk_index int not null,
  content text not null,
  token_estimate int not null,
  created_at timestamptz not null default now()
);

create index if not exists document_chunks_tenant_doc_idx
  on document_chunks (tenant_id, document_id, chunk_index);

create index if not exists document_chunks_tenant_agent_created_at_idx
  on document_chunks (tenant_id, agent_id, created_at);

