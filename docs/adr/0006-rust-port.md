# ADR-0006: Rust backend and Bun/React UI

Date: 2026-11-04 · Status: accepted · Amends: ADR-0001, ADR-0004

## Context
Otterscope's product constraint is operational weight, not a language: one self-contained binary, one SQLite file, no external services (ADR-0001). The Go implementation proved the product and the ingest model, but it carried avoidable costs. The pure-Go SQLite driver (`modernc.org/sqlite`) was roughly twice as slow as CGO sqlite3 on some workloads; the alternative, CGO, would have broken the easy static build. The garbage collector and runtime footprint set a floor on latency and memory during ingest, exactly where a single-writer SQLite store is most sensitive. And the SQLite driver, OTLP/protobuf decoding and HTTP stack were three separate dependency families to keep current.

Rust addresses those costs without changing the product constraint: `rusqlite` with the `bundled` feature compiles SQLite into the binary, so there is no pure-Go performance penalty and no CGO decision to make; ownership removes the GC and data-race class of bugs; `axum`/`tokio`/`prost` give the same HTTP, async and protobuf capabilities with compile-time checking. The result is still one binary to copy and run.

The frontend had a different problem. The Vite + React + TypeScript UI worked, but routing, URL state and server-state caching were hand-rolled, and visual primitives were ad hoc. Rebuilding on Bun + React 19 + TanStack Router/Query + Tailwind + shadcn-style components gives typed routes, a single query cache, accessible primitives and one build toolchain, while keeping the UI embedded in the binary. Bun removes the npm install/build/test split and matches the single-tool spirit of the backend.

The port is an implementation change, not a repositioning. ADR-0002 (OTLP ingest boundary), ADR-0003 (license) and ADR-0005 (self-host only) still hold.

## Decision

1. **Backend in Rust.** A Cargo workspace at `rust/` with one binary crate, `rust/crates/otterscope` (edition 2024). Stack: `axum` + `tokio` for HTTP, `rusqlite` (`bundled`) for SQLite, `prost` for OTLP protobuf, `rust-embed` for the UI, `reqwest` for the judge and webhook calls. Modules mirror the old packages: `model`, `store`, `ingest`, `server`, `alerts`, `mcp`, `pricing`, `evals`, `sample`, `config`, `basepath`, plus `args`.
2. **Bun/React UI.** `web/` uses Bun for install/scripts/tests, React 19, TanStack Router and Query, Tailwind and local shadcn-style components. `bun run build` writes `web/dist`, embedded at compile time.
3. **Preserved behaviour.** The SQLite schema and the append-only migration sequence are carried over unchanged, so an existing database opens without a conversion step. CLI flags, `OTTERSCOPE_*` environment variables and the flags > env > file > defaults precedence keep their Go spellings and semantics. The OTLP receiver, dialect coverage (OTel GenAI old + experimental, OpenInference, Vercel AI SDK, Claude Agent SDK/Code), ingest limits, token scopes, alerting, assertions and judge behaviour are ported rather than redesigned. The single-connection SQLite writer and its cursor rule remain.
4. **Deliberately changed JSON contract.** The HTTP API is redesigned for the new server and UI: `camelCase` fields, typed enums (`status`, step `kind`, token `scope`) instead of free strings, one `{"error": "…"}` failure envelope, and array envelopes that are always present. The contract is documented in `docs/api-rust.md`; this is a breaking change for Go-era API clients.
5. **MCP revision 2026-07-28 only.** The MCP server is the stateless revision: no `initialize` handshake, no `Mcp-Session-Id`; clients send `MCP-Protocol-Version`, `Mcp-Method` and `Mcp-Name`, with protocol metadata mirrored in `_meta`. Older revisions are rejected with `UnsupportedProtocolVersionError`. This is a breaking change for MCP clients pinned to an earlier session-based revision.
6. **No byte-compatibility promise.** Databases remain plain SQLite and remain schema-compatible through the migrations, but the Go and Rust builds do not promise byte-identical database files. Back up before switching, and do not run both binaries against one file.
7. **Build and test gates.** `bun install && bun run build` in `web/`, then `cargo build --release` in `rust/`; `cargo fmt`, `clippy` and `test`, plus `bun run lint`, `bun test` and `bun run build`, are the quality gate.

## Consequences
- API and MCP clients must be updated. The break is intentional and documented in `docs/api-rust.md`, `docs/mcp.md` and the changelog; there is no compatibility shim for the Go JSON shapes or for session-based MCP.
- The Go tree stays temporarily as the reference implementation. During that window a fix may need to land in both trees, or explicitly be declared Rust-only; the port epic tracks parity before the Go tree is removed.
- Rust adds toolchain and CI work: edition 2024, longer release builds, and a crate supply chain to review. `cargo audit`/`cargo deny` are not yet wired in; the dependency rule ("justify each crate") applies with more force than it did for Go modules.
- `rusqlite` compiles SQLite from C. That is what removes the pure-Go penalty and keeps the binary self-contained, but it means the build needs a C compiler and the binary links the platform C runtime. "Static" remains a deployment promise (one file), not a fully static musl claim.
- The performance and memory motivations are design targets, not published results. `docs/benchmarks.md` now marks its numbers as Go-era reference data and must be re-run against the Rust store before any speed claim appears in the README.
- ADR-0001's language choice and ADR-0004's Go-era packaging are amended by this ADR. The release workflow now builds four targets and a multi-arch Docker image; Homebrew, deb/rpm, signing and the fifth platform remain deferred, as recorded in ADR-0004's amendment and `docs/packaging.md`.
- ADR-0002, ADR-0003 and ADR-0005 are unaffected: the ingest boundary, the Apache-2.0 license and the no-hosted-offering rule still hold.
