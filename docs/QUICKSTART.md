# Quickstart (The Reef)

This repo ships a Docker Compose “Reef” that runs:
- Postgres (durable store)
- Qdrant (vector index)
- Harbor (HTTP API)
- Worker (background autonomy loop)

## 1) Prereqs
- Docker + Docker Compose v2

## 2) Start with the mock provider (recommended)

From repo root:

1. Create local config + env:
   - `cp configs/reef-mock.toml lorelei.toml`
   - `cp .env.example .env`

2. Start The Reef:
   - `docker compose up --build -d`

3. Wait for readiness:
   - `curl -fsS http://localhost:8080/healthz`
   - `curl -fsS http://localhost:8080/readyz`

4. Use the CLI (inside the container):
   - `docker compose exec -T harbor lore memo "hello reef"`
   - `docker compose exec -T harbor lore echo "hello reef"`
   - `docker compose exec -T harbor lore ask "Say ok." --no-memory --progress false`

## 3) Optional: local Ollama profile

Enable Ollama services:
- `docker compose --profile ollama up --build -d`

Then configure your provider to point at the compose host:
- In `lorelei.toml`, set `[providers.ollama].base_url = "http://ollama:11434/v1"`
- Set `[agent].default_provider = "ollama"` and `default_embedding_provider = "ollama"`

## 4) Smoke test

Run:
- `bash scripts/smoke_reef.sh`

