# Security Model (Iron Safety)

Lorelei’s security posture is intentionally conservative. The goal is safe autonomy: tools are explicit, boundaries are enforceable, and memory is treated as untrusted input at retrieval time.

## Scope
### In scope (v1)
- Single-agent loop execution with a constrained tool system.
- Persistent memory (Echoes/Pearls) with retrieval for RAG.
- Dockerized local runtime for tool execution isolation.
- Provider-agnostic model interface with adapters.

### Out of scope (v1)
- Unrestricted shell execution.
- Multi-agent orchestration.
- “Trust me” provider-specific behaviors in core.

## Threat Model (The Reef Is Untrusted)
### Primary threats
- **Prompt injection** via retrieved Lore (malicious instructions embedded in Pearls/Echoes).
- **Tool abuse**: coercing the agent to call tools with dangerous parameters.
- **Data exfiltration**: leaking secrets through model calls, logs, or memory writes.
- **Boundary escapes**: path traversal, container escape attempts, network egress misuse.
- **Supply chain risk**: unvetted tool plugins, unpinned images/dependencies.

### Assumptions
- The LLM is not trusted to enforce policy; it can be manipulated.
- Retrieved memory is not trusted; it must be treated like untrusted input.
- The runtime is a boundary: code must assume hostile prompts and hostile retrieved context.

## Security Controls
### Tool allowlists (The Shell)
- Tools must be explicitly registered and allowlisted.
- Each tool declares:
  - input schema constraints (size, types, allowed paths/URLs)
  - output schema constraints (size, redaction policy)
  - timeouts and resource limits
- Default-deny: unregistered tools are not callable.

### No unrestricted shell (The Harbor)
- v1 must not provide “run arbitrary command” functionality.
- If any execution is supported later, it must be sandboxed and capability-scoped.

### Dockerized runtime isolation (The Harbor)
- Tools execute in a container boundary (where applicable).
- Filesystem boundaries are explicit (workspace mounts are scoped and read/write is controlled).
- Optional network egress policy to support offline or allowlisted modes.

### Memory hygiene (The Reef)
- Provenance recorded for every Echo/Pearl (source, timestamp, correlation).
- Retrieval-time policy:
  - mark retrieved items as untrusted context
  - strip or annotate imperative instructions unless explicitly opted-in
- Redaction/filters to avoid persisting secrets by default.

### Secrets handling
- Support `.env` / environment injection for runtime secrets, but:
  - do not persist secrets into memory by default
  - redact secrets from logs and traces where feasible

## Operational Guidance (The Harbor Master)
- Prefer running Lorelei in an environment with minimal credentials.
- Use least-privilege tokens for any provider adapters.
- Keep Docker images pinned and updated.

## Reporting and Response
- Security issues should be reported privately (process TBD).
- Public issues should avoid including secrets or exploit details until triaged.

