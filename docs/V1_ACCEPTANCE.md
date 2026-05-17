# v1 Acceptance

This document defines the v1 acceptance checklist and the commands to validate it.

**Rule:** Do not mark v1 complete if any **memory isolation**, **Siren**, or **deletion-exclusion** test fails.

## Quick run (automated)

Runs the feasible checks using the **mock provider** by default:
- `bash scripts/v1_acceptance.sh`

## Scenarios (manual + automated)

### 1) Fresh Docker start succeeds
- `cp configs/reef-mock.toml lorelei.toml`
- `cp .env.example .env`
- `docker compose up --build -d`
- `curl -fsS http://localhost:8080/healthz`
- `curl -fsS http://localhost:8080/readyz`

### 2) `lore doctor` passes
- `docker compose exec -T harbor lore doctor`

### 3) Manual Pearl save works
- `docker compose exec -T harbor lore memo "v1: my name is Eddie"`

### 4) Echo retrieves the Pearl
- `docker compose exec -T harbor lore echo "Eddie" --top-k 5`

### 5) `lore ask` uses retrieved memory in final answer
With the mock provider, the answer should include `Using memory:` when EchoHits exist.
- `docker compose exec -T harbor lore ask "What is my name?" --progress false`

### 6) Reflection stores a durable preference
- `docker compose exec -T harbor lore ask "remember that I prefer tea" --progress false`
- `docker compose exec -T harbor lore pearls --pearl-type preference`

### 7) Temporary fact is rejected by reflection
- `docker compose exec -T harbor lore ask "remember todo: call mom tomorrow" --progress false`
- Verify the temporary item does **not** appear in `lore pearls`.

### 8) High-risk Shell requires approval
Trigger a deterministic shell call via the mock provider:
- `docker compose exec -T harbor lore ask "LORELEI_TEST_CALL_SHELL=forget_pearl pearl_id=<uuid>" --progress false`
- Expect: `Approval required` and an approval appears in:
  - `docker compose exec -T harbor lore approvals`

### 9) Low-risk Shell executes automatically
- `docker compose exec -T harbor lore ask "LORELEI_TEST_CALL_SHELL=noop" --progress false`
- Expect: run succeeds and does not require approvals.

### 10) Scheduled autonomous task runs and creates a run
- `docker compose exec -T harbor lore task add "Say ok." --daily --at <UTC_HH:MM>`
- Wait until `<UTC_HH:MM>` passes, then:
  - `docker compose exec -T worker lorelei-harbor worker --once`
- Verify a run was created (currents exist for the task run).

### 11) Provider abstraction works (mock + one real/local provider)
- Mock: `configs/reef-mock.toml`
- Real/local: `docs/PROVIDERS.md` (OpenAI-compatible, Ollama profile, etc.)

### 12) Tenant isolation test passes
- Attempts to access a run/pearl from the wrong tenant should be rejected/empty.

### 13) Deleted Pearl exclusion test passes
- Save a Pearl, delete it, then verify Echo does not return it.

### 14) Document ingestion + search (if included in v1)
- `docker compose exec -T harbor lore docs ingest docs/ADR-0001-architecture.md`
- `docker compose exec -T harbor lore docs search architecture --top-k 3`

### 15) Logs contain run_id but not secrets
- Harbor logs should include `run_id=...` lines.
- Ensure API keys are not printed (keep `LORELEI_LOG_PROMPTS` disabled).

### 16) Docker image builds and smoke test passes
- `docker build -f docker/Dockerfile -t lorelei-reef:local .`
- `docker build -f docker/Dockerfile.lore -t lorelei-cli:local .`
- `bash scripts/smoke_reef.sh`

