# Lorelei Roadmap (The Tide)

This roadmap defines staged slices. Dates are intentionally omitted until implementation starts.

## North Star (The Song)
- A **reliable single-agent loop** with safe tools and durable, RAG-backed memory.
- A **Dockerized local runtime** that is reproducible and auditable.
- A **provider-agnostic LLM layer** in core types.
- A CLI that makes memory inspectable (Echo/Pearl lifecycle) and operations scriptable.

## v0 (Harbor Setup)
- Repository structure and planning docs (ADR, testing, security, roadmap).
- Configuration model draft (what must be configurable vs. compiled defaults).
- Decide initial persistence targets (local files vs. embedded DB) as an ADR.

## v0.1 (First Current: Minimal Single-Agent Loop)
- Core “Song” loop skeleton: plan → act → observe → remember (no multi-agent).
- Tool contract and allowlist policy (no shell; no arbitrary code execution).
- Minimal CLI (`lore`) to run a single session and print traces.

## v0.2 (The Reef: Memory MVP)
- Persistent storage for Echoes and Pearls.
- Embedding + retrieval pipeline behind interfaces.
- CLI commands to inspect memory contents and provenance.

## v0.3 (Dockerized Runtime MVP)
- Local Docker runtime that enforces isolation for tool execution.
- Explicit filesystem boundaries and optional network egress policy.
- Integration tests that run end-to-end inside the container.

## v0.4 (Provider-Agnostic LLM Adapters)
- Adapter layer for at least one provider (implementation detail), without provider assumptions in core.
- Conformance test suite for provider adapters (golden behaviors and error mapping).

## v1.0 (Safe Autonomy Baseline)
- Single-agent loop reliability (timeouts, retries, deterministic tracing).
- Stable memory APIs and migration/versioning story for stored Lore.
- Security model implemented as code-level enforcement + documented operational guidance.

## Explicit Non-Goals (The Reef’s Edge)
- No unrestricted shell execution in v1.
- No multi-agent orchestration until the single-agent loop is reliable.
- No provider-specific assumptions in core types.

