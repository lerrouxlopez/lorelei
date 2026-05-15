# Lorelei
**Lorelei: Deep Memory, Iron Safety.**

Lorelei is a Rust-native autonomous AI-agent framework focused on **safe autonomy** and **durable memory**: a single-agent loop that can reason, act through tightly-scoped tools, and persist long-term context via RAG-backed storage.

This repo currently contains **planning documents only** (no implementation code yet).

## The Song (What Lorelei Is)
- **Rust-first agent framework**: core types and execution loop designed for correctness and clarity.
- **RAG-backed persistent memory**: turn experiences into retrievable knowledge over time.
- **Dockerized local runtime**: reproducible, inspectable execution environment for development and testing.
- **Provider-agnostic LLM layer**: core types do not assume any one model vendor or API shape.
- **CLI**: `lore`

## The Reef (Non-Goals for v1)
- No unrestricted shell execution.
- No multi-agent orchestration until the single-agent loop is reliable.
- No provider-specific assumptions in core types.

## Vocabulary
Lorelei uses nautical metaphors as a shared design language:
- **The Lore**: persistent knowledge accumulated over time.
- **Pearl**: a compact, high-signal memory item (candidate for retrieval).
- **Echo**: a trace of an interaction/event (raw or lightly structured).
- **The Song**: the agent loop (plan → act → observe → remember).
- **The Reef**: memory store + retrieval index (durable substrate).
- **Current / Tide / Harbor / Shell / Siren**: terms reserved for runtime flow, boundaries, and interfaces.

## Documents
- `docs/ADR-0001-architecture.md` — baseline architecture decision.
- `docs/ROADMAP.md` — milestones and release slices.
- `docs/TESTING.md` — testing strategy (unit/integration/security).
- `docs/SECURITY_MODEL.md` — threat model and safety constraints.

## Branding
- Project name: **Lorelei**
- Tagline: **“Lorelei: Deep Memory, Iron Safety.”**
- CLI name: **`lore`**
- Logo source (local): `C:\Users\User\Documents\Docs\lorelei\logo.png` (TODO: vendor into repo when ready)

## License
TBD.
