# Provider config examples

Provider definitions live in `lorelei.toml` under `[providers.<name>]`. Pick a default by setting:
- `[agent].default_provider`
- `[agent].default_embedding_provider`

## Mock (local / CI)

Use `configs/reef-mock.toml` as a working reference.

## OpenAI (OpenAI-compatible)

In `lorelei.toml`:

```toml
[agent]
default_provider = "openai"
default_embedding_provider = "openai"

[providers.openai]
kind = "openai-compatible"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
chat_model = "gpt-4o-mini"
embedding_model = "text-embedding-3-small"
```

In your environment:
- `OPENAI_API_KEY=...`

## Ollama (Compose profile)

Start Ollama in the Reef:
- `docker compose --profile ollama up -d`

In `lorelei.toml`:

```toml
[agent]
default_provider = "ollama"
default_embedding_provider = "ollama"

[providers.ollama]
kind = "local"
base_url = "http://ollama:11434/v1"
api_key_env = "LORELEI_LOCAL_API_KEY"
chat_model = "llama3.2:3b"
embedding_model = "nomic-embed-text"
```

In your environment (can be dummy):
- `LORELEI_LOCAL_API_KEY=replace-me`

