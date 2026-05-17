# Lorelei v1 Release Notes

## Features
- **The Reef (Docker Compose runtime)**: Postgres + Qdrant + Harbor + Worker.
- **Harbor HTTP API**: `/v1/runs`, Pearls, Echo retrieval, Documents, Tasks, Approvals.
- **Siren policy gate**: deterministic allow/deny/approval flow for tool usage.
- **Persistent memory**: Pearls stored in Postgres and indexed in Qdrant.
- **CLI (`lore`)**: doctor, ask, memo, echo, pearls, forget, docs, tasks, approvals.

## Known limitations
- **Task scheduling**: v1 supports daily schedules only; tasks are time-based and may not run immediately in tests.
- **Local model variability**: some providers may return malformed JSON; Lorelei includes repair logic but outputs can still vary by model.
- **Performance**: a single `lore ask` may include multiple sequential steps (retrieve → plan → answer → memory).

## Security notes
- **Tooling is gated**: medium/high risk tools require explicit policy checks (and approvals for high-risk).
- **Secrets should not be logged**: keep prompt logging disabled (do not enable `LORELEI_LOG_PROMPTS` in production).
- **Tenant scoped data**: Pearls, Echo, and docs are tenant-scoped; acceptance includes a tenant isolation check.

## Provider support status
- **Mock provider**: supported (recommended for CI / smoke / acceptance).
- **Ollama (OpenAI-compatible local)**: supported via compose `--profile ollama`.
- **OpenAI-compatible providers**: supported via `[providers.<name>] kind = "openai-compatible"` (requires keys).

## Next roadmap
- Faster/streaming UX improvements (partial results, structured progress).
- More robust “structured output” mode per provider.
- Expanded scheduling options (one-shot / interval schedules).
- Multi-agent / multi-tenant operational tooling and dashboards.

