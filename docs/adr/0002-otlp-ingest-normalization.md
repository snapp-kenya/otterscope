# ADR-0002: OTLP/HTTP ingestion with a dialect-normalization layer

Date: 2026-07-14 · Status: accepted

> Port note (2026-11-04): the paths in this ADR are updated to the Rust tree; the decision is unchanged (ADR-0006).

## Context
As of mid-2026 the OTel GenAI semantic conventions are **not stable**: the old `gen_ai.*` attribute format and the `gen_ai_latest_experimental` format coexist in the wild, and OpenInference (Arize) is the de-facto dialect for several agent frameworks (OpenAI Agents SDK, CrewAI). Frameworks emit natively (Pydantic AI, Vercel AI SDK, LangGraph) or via instrumentors (OpenLLMetry, OpenInference).

## Decision
- Single ingestion surface: **OTLP/HTTP** (`POST /v1/traces` on the single port-80 listener per ADR-0007, protobuf and JSON) — drop-in `OTEL_EXPORTER_OTLP_ENDPOINT` compatibility. No proprietary SDK required on day one.
- A normalization layer in `rust/crates/otterscope/src/ingest` maps all three dialects (OTel-old, OTel-experimental, OpenInference) into our own domain model (`rust/crates/otterscope/src/model`: Run, Step, LlmCall, ToolCall). OTel types never cross that boundary.
- Every dialect quirk discovered is captured as a real OTLP payload fixture in `rust/crates/otterscope/testdata/` with a test.

## Consequences
- We absorb the ecosystem's instability so users don't — this is deliberate, budgeted, ongoing work (the normalization layer is expected churn) and a durable moat.
- Raw span attributes are retained alongside normalized data so re-normalization after upstream changes is possible without data loss.
- OTLP/gRPC deferred until requested.
