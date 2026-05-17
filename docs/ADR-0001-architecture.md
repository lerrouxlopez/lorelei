# ADR-0001: Baseline Architecture (Single Agent, Persistent Memory)

- **Status**: Accepted
- **Date**: 2026-05-16
- **Decision owners**: Lorelei maintainers

## Context (The Harbor)
Lorelei is a Rust-native autonomous AI-agent framework. The goal is a reliable **single-agent** loop with:
- **Provider-agnostic** model access (no vendor assumptions in core types).
- **RAG-backed persistent memory** for long-term recall.
- A **Dockerized local runtime** for safe, reproducible execution.
- A clear security posture: **no unrestricted shell execution** and tight tool boundaries.

The architecture must support growth (plugins/tools, more memory backends, more model providers) without forcing premature multi-agent orchestration.

## Decision (The Current)
We adopt a layered architecture with strict boundaries:

1. **Core (The Shell)**: stable, provider-agnostic domain model and agent loop primitives.
   - Core defines the “Song” as an explicit state machine: plan → act → observe → remember.
   - Core owns tool contracts, safety policies, and canonical event/memory schemas.

2. **LLM Abstraction (The Siren Interface)**:
   - A trait-based interface for “generate / embed / moderate (optional)” capabilities.
   - Providers live outside core (adapters), so core types remain vendor-neutral.

3. **Memory (The Reef)**:
   - A persistent store for **Echoes** (event traces) and **Pearls** (curated memory items).
   - Retrieval is RAG-oriented: query → candidate fetch → rank → context assembly.
   - Memory backends are pluggable; core consumes a capability interface.

4. **Runtime (The Harbor)**:
   - A Dockerized local runtime that hosts sandboxed execution facilities for tools.
   - The runtime enforces security constraints (network egress policies, file boundaries, tool allowlists).

5. **CLI (lore)**:
   - Entry point that wires together configuration, providers, memory, and runtime.
   - Exposes minimal, composable commands to operate the agent loop and inspect memory artifacts.

## Rationale (Why This Tide)
- **Safety first**: separating core policy from runtime execution makes “no unrestricted shell” enforceable by design.
- **Provider neutrality**: using traits and adapters avoids “provider-shaped” core types.
- **Iterative delivery**: a single-agent loop can ship earlier; multi-agent is explicitly deferred.
- **Extensibility**: memory backends, tools, and providers can evolve independently behind interfaces.

## Consequences (What Washes Ashore)
### Positive
- Clear boundaries for testing (pure core vs. runtime integration).
- Memory becomes a first-class subsystem (Echo/Pearl lifecycle, retrieval invariants).
- Easier compliance and review: security model is centralized and explicit.

### Negative / Tradeoffs
- More upfront interface design (traits, DTOs, versioning).
- Some runtime features may feel “heavier” due to Dockerization and policy enforcement.

## Guardrails (Iron Safety)
- Tools are **capability-scoped** and **explicitly allowlisted**.
- The agent cannot access arbitrary OS shell commands in v1.
- Memory ingestion must track provenance (which tool/step produced which Echo/Pearl).
- Provider adapters must be isolated from core to prevent vendor coupling.

## Follow-ups
- Define the minimal v1 contracts for:
  - LLM inference + embedding
  - Tool execution envelope (inputs/outputs/errors/timeouts)
  - Memory API (write Echo/Pearl; retrieve Pearls; compaction policies)
- Specify configuration sources and precedence (file/env/flags).
