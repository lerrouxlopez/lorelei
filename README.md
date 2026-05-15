
# Lorelei : Deep Memory, Iron Safety.**

Lorelei is a Rust-native autonomous agent framework built around persistent RAG memory, provider-agnostic LLMs, and containerized deployment.

- **The Lore** remembers.
- **Echo** retrieves.
- **The Song** reasons.
- **Siren** protects.
- **The Reef** contains.


## Binaries

- `lore` (from `crates/lorelei-cli`)
- `lorelei-harbor` (from `crates/lorelei-harbor`)

## Install + `lori` alias

Install the CLI from the workspace root:

```bash
cargo install --path crates/lorelei-cli --bin lore
```

Create an alias named `lori`:

- macOS/Linux:

  ```bash
  ln -sf "$(command -v lore)" "$HOME/.local/bin/lori"
  ```

- Windows (PowerShell):

  ```powershell
  Set-Alias lori lore
  ```

## Docker (Harbor + Postgres + Qdrant)

```bash
cp .env.example .env
cp lorelei.toml.example lorelei.toml
docker compose up --build
curl http://localhost:8080/healthz
```

> Note: Harbor reads `DATABASE_URL` and `QDRANT_URL` from environment variables, not from `lorelei.toml`.
