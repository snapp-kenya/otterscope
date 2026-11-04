# ADR-0001: Single static Go binary with embedded SQLite

Date: 2026-07-14 · Status: amended by ADR-0006 (2026-11-04)

> Implementation note: the decision below is still current, but its Go/`modernc.org/sqlite`/CGO details are historical. The current implementation is Rust with bundled SQLite; see "Current implementation" below.

## Context
The competitive research (July 2026) shows the market gap for AI-agent observability is specifically *operational weight*: Langfuse v3 requires Postgres + ClickHouse + Redis + S3; Opik, SigNoz, OpenLIT are ClickHouse-backed; LangSmith/Braintrust gate self-hosting behind enterprise contracts. The users we target run a few thousand to a few hundred thousand LLM calls per day — SQLite-scale, not ClickHouse-scale.

## Decision
- Go, compiled as a single static binary (CGO disabled). SQLite via `modernc.org/sqlite` (pure Go).
- One embedded database file (WAL mode). The web UI is embedded via `go:embed`.
- The product identity **is** this constraint: any feature whose design requires an external service gets redesigned or rejected.

## Consequences
- Deployment is `./otterscope serve` or one Docker container — our core differentiator.
- Analytics queries are bounded by SQLite; acceptable at target scale. If it becomes a real limit, revisit with a DuckDB/Parquet layer (deferred, see ROADMAP), never a mandatory server dependency.
- `modernc.org/sqlite` is slower than CGO sqlite3 (~2x on some workloads) — the price of static builds; benchmark before optimizing.

## Current implementation (2026-11-04)

The decision and its consequences still hold: one self-contained binary (one process), one embedded SQLite file, one embedded web UI, no external services. The implementation is now Rust (`rust/crates/otterscope`) with `rusqlite`'s `bundled` SQLite, `rust-embed` for the UI, and `axum`/`tokio` for HTTP (ADR-0006). The Go, `modernc.org/sqlite`, CGO-free and `go:embed` details in the body above describe the original implementation and are retained as history.

What "single binary" means now, precisely:

- One executable contains the server, the SQLite library and the built UI; there is no database server, asset directory or sidecar to deploy.
- It is **not fully static**. `rusqlite` compiles SQLite from C, so the build needs a C compiler, and a Linux build links glibc 2.39+ when built on the Ubuntu 24.04 baseline (2.36 on the Debian bookworm image build). The container image is the portable option.
- The relevant promise is one file to deploy and one SQLite file for data — not a fully static musl build.
