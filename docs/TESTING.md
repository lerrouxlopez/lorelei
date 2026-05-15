# Testing Strategy (Echoes You Can Trust)

Lorelei aims for “iron safety”: predictable behavior, explicit boundaries, and testable contracts. This document defines the initial testing approach before implementation begins.

## Test Layers
### Unit tests (The Shell)
- Pure logic in core: state transitions for the Song, schema validation, policy checks.
- Deterministic inputs/outputs; avoid network and filesystem.

### Contract tests (The Siren)
- Provider-agnostic LLM interface conformance:
  - Error mapping and retry classification
  - Streaming vs. non-streaming semantics (if supported)
  - Token/length limit handling (as abstract constraints)

### Integration tests (The Harbor)
- Run the CLI against a local runtime (Docker) with a mock provider and a real memory backend.
- Validate tool allowlists, timeouts, and boundary enforcement.

### Memory tests (The Reef)
- Echo/Pearl lifecycle:
  - Ingestion correctness (provenance, timestamps, correlation IDs)
  - Retrieval quality invariants (rank stability, dedupe, max-context budgeting)
  - Migration tests for schema/version changes

### Security tests (Iron Safety)
- Prompt injection scenarios (malicious “instructions” inside retrieved Lore).
- Tool misuse attempts (attempts to escape the allowlist, path traversal, oversized inputs).
- Secrets handling (ensure `.env`/secrets never get written into memory by default).

## What We Will Automate
- `cargo test` for unit + integration tests (as they exist).
- Linting and formatting gates (`cargo fmt`, `cargo clippy`) once code arrives.
- Containerized CI job for runtime integration tests (Docker required).

## What We Will Not Do Initially
- Fuzzing by default (can be added once interfaces stabilize).
- Multi-agent simulation (explicitly out of scope until single-agent is reliable).

## Test Data Principles
- Avoid real secrets; use synthetic fixtures.
- Prefer small, human-readable fixtures for memory (Echoes/Pearls).
- Keep “golden” outputs stable by normalizing timestamps/IDs where appropriate.

