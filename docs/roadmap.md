# Roadmap

Dates assume start 2026-07-14. The milestones below were tracked as GitHub milestones; GitHub Issues are now disabled on this repository, so the live tracker is this file plus the draft port epic (PR #1, `rewrite/rust-port`). This file states *intent* — when reality diverges, update this file in the same PR that diverges.

## M0 — Foundation (week of Jul 14)
Repo, docs, CI, scaffold. `otterscope serve` starts, opens SQLite, serves a health endpoint and a placeholder UI.

## M1 — Ingest core (Jul 14 – Jul 27)
The moat. OTLP/HTTP receiver (protobuf + JSON) on the server's single port-80 listener, at `POST /v1/traces`; normalization of OTel GenAI spans (old + `gen_ai_latest_experimental` dialects) and OpenInference into the domain model (Run → Step → LLMCall/ToolCall); SQLite schema + migrations; captured real-framework payloads as fixtures (Vercel AI SDK, Pydantic AI, OpenAI Agents SDK via OpenInference). Exit criterion: point a real instrumented agent at Otterscope and its runs land correctly in the DB.

## M2 — Run explorer UI (Jul 27 – Aug 10)
Bun + React 19 UI (TanStack Router/Query, Tailwind, shadcn-style components) embedded in the binary: runs list (status, duration, model, tokens, cost, error badge), run detail with step timeline, tool-loop visibility, LLM call inspector (messages in/out), live tail. Exit criterion: debugging a misbehaving agent via the UI is genuinely faster than reading logs.

## M3 — Cost, search, projects (Aug 10 – Aug 24)
Model-pricing table (maintained, overridable) → per-call/per-run cost; filter/search (model, status, time range, attribute); multiple projects with ingest API keys; retention config. Includes first debt-repayment pass.

## M4 — Evals v1 (Aug 24 – Sep 14)
Assertions (contains/regex/JSON-schema/latency/cost thresholds) and LLM-as-judge scoring over captured runs; score storage on the run; comparison view (this week vs last, version A vs B). Exit criterion: "did my prompt change make things worse?" answerable in-product.

## M5 — Ship it (Sep 14 – Sep 28)
Manual release process per ADR-0004, retargeted to the Rust workspace: `scripts/release.sh` builds the native archives on matching hosts (linux amd64/arm64, darwin arm64, windows amd64), and the multi-arch Docker image is built and pushed manually to GHCR (distroless base), plus a `docker-compose.yml` example; install docs; demo instance with sample data; readme polish with screenshots/GIF; docs for each supported framework. There is no CI; the gate and the release steps are documented in [`development.md`](development.md) and `docs/packaging.md`. Homebrew, deb/rpm, darwin amd64, windows arm64 and signing remain deferred (`docs/packaging.md`). Launch: Show HN + r/selfhosted + awesome-selfhosted PR.

The release process is exercised with `v0.0.x` pre-releases so packaging is proven before launch, not bolted on at the end.

## Later / explicitly deferred
- DuckDB/Parquet analytics sidecar for large volumes (ADR-0002 keeps SQLite until proven insufficient)
- OTLP/gRPC receiver (HTTP covers the ecosystem; gRPC on demand)
- Prompt management, multi-user auth/RBAC

## Never
Hosted/managed Otterscope, paid tiers, usage metering, seat fees. Otterscope is software you run; see ADR-0005.
