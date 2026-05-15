
# Lorelei: Deep Memory, Iron Safety.

Lorelei is a Rust-native autonomous agent framework built around persistent RAG memory, provider-agnostic LLMs, and containerized deployment.

- **The Lore** remembers.
- **Echo** retrieves.
- **The Song** reasons.
- **Siren** protects.
- **The Reef** contains.


## Binaries

- `lore` (from `crates/lorelei-cli`)
- `lorelei-harbor` (from `crates/lorelei-harbor`)

## Usage

### Install the CLI (host machine)

Install the CLI from the workspace root:

```bash
cargo install --path crates/lorelei-cli --bin lore
```

## Docker (Harbor + Postgres + Qdrant)

### 1) Configure

```bash
cp .env.example .env
cp lorelei.toml.example lorelei.toml

# set at least one provider key in `.env`:
#   OPENAI_API_KEY=...
#   ANTHROPIC_API_KEY=...
```

### 2) Start the Reef

```bash
docker compose up --build
curl http://localhost:8080/healthz
```

> Note: Harbor reads `DATABASE_URL` and `QDRANT_URL` from environment variables, not from `lorelei.toml`.

### 3) Run a request

```bash
# requires Harbor running (docker compose up)
lore ask "hello"
```

### Switching providers without rebuild

Provider/model selection is **server-side** (Harbor reads `lorelei.toml`).

To change providers without rebuilding Harbor:

1) Update `lorelei.toml`
2) Reload Harbor config

- `lore init --song-provider openai`
- or `curl -X POST http://localhost:8080/v1/config/reload`

Example `lorelei.toml` snippet:

```toml
[song]
provider = { name = "openai" }

[providers.openai]
kind = "openai-compatible"
base_url = "https://api.openai.com/v1"
model = "gpt-4.1-mini"
api_key = { source = "env", var = "OPENAI_API_KEY" }
```

## Observability

Lorelei uses `tracing` for structured logs.

- `RUST_LOG=info` (or `debug`, etc.) controls log level.
- `LORELEI_LOG_JSON=true` enables JSON log output (recommended for containers).
- `LORELEI_LOG_PROMPTS=true` allows logging full prompts (off by default; avoid in production).
