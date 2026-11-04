# ADR-0002: OTLP/HTTP ingestion with a dialect-normalization layer

Date: 2026-07-14 · Status: accepted

> Port note: the paths below are Go-era. The layer now lives in `rust/crates/otterscope/src/ingest`, the domain in `rust/crates/otterscope/src/model`, and the fixtures in `rust/crates/otterscope/testdata` (ADR-0006). The decision itself is unchanged.

## Context
As of mid-2026 the OTel GenAI semantic conventions are **not stable**: the old `gen_ai.*` attribute format and the `gen_ai_latest_experimental` format coexist in the wild, and OpenInference (Arize) is the de-facto dialect for several agent frameworks (OpenAI Agents SDK, CrewAI). Frameworks emit natively (Pydantic AI, Vercel AI SDK, LangGraph) or via instrumentors (OpenLLMetry, OpenInference).

## Decision
- Single ingestion surface: **OTLP/HTTP** (`:4318`, protobuf and JSON) — drop-in `OTEL_EXPORTER_OTLP_ENDPOINT` compatibility. No proprietary SDK required on day one.
- A normalization layer in `internal/ingest` maps all three dialects (OTel-old, OTel-experimental, OpenInference) into our own domain model (`internal/model`: Run, Step, LLMCall, ToolCall). OTel types never cross that boundary.
- Every dialect quirk discovered is captured as a real OTLP payload fixture in `internal/ingest/testdata/` with a test.

## Consequences
- We absorb the ecosystem's instability so users don't — this is deliberate, budgeted, ongoing work (the normalization layer is expected churn) and a durable moat.
- Raw span attributes are retained alongside normalized data so re-normalization after upstream changes is possible without data loss.
- OTLP/gRPC deferred until requested.
