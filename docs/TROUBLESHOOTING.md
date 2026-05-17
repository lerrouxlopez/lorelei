# Troubleshooting

## `502 provider_error` calling Ollama from Harbor

If Harbor runs in Docker, `127.0.0.1` points at the Harbor container, not your host.

Fix:
- Use the compose hostname: `http://ollama:11434/v1`

## `Denied by Siren` for `save_pearl` / tools

- Ensure `siren.allow_shell_execution = true` when you want the agent to save memories.
- If you only want answers (no tools/memory), run:
  - `lore ask --no-memory --progress false "..."` (or set `siren.allow_shell_execution=false`)

## `invalid planner JSON` / `invalid candidate JSON`

Some local models wrap JSON in extra text or code fences. Lorelei attempts to repair and extract JSON, but if you still see this:
- Try a different model
- Reduce temperature on your provider (if supported)
- Use the mock provider to validate your Reef plumbing

## Slow responses

`lore ask` runs multiple steps (retrieve → plan → answer → memory). With local models this can take a while.

Tips:
- Use `--no-memory` for quick answers.
- Enable progress output (default) to see phases.

