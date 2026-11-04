# Otterscope — agent working guide

Lightweight, self-hosted observability + evals for AI agents. One self-contained Rust binary, embedded SQLite, embedded web UI. Positioning: "Plausible for AI agents" — the anti-ClickHouse-stack. Read `docs/ROADMAP.md` for milestones and `docs/adr/` for decisions already made (do not relitigate them silently — write a new ADR to change one). ADR-0006 records the port from Go to Rust and the Bun/React UI; the Rust workspace under `rust/` and the UI under `web/` are the only implementations.

## Architecture

- `rust/crates/otterscope/src/main.rs` — CLI entrypoint (`serve`, `version`, `sample`, `token`, `project`, `backup`, `renormalize`).
- `rust/crates/otterscope/src/cli.rs` — database subcommands and the config/flag plumbing they share with `serve`.
- `rust/crates/otterscope/src/args.rs` — Go-`flag`-compatible parser: one or two dashes, `-name=value` or `-name value`, boolean flags, stop at the first operand.
- `rust/crates/otterscope/src/config.rs` — effective `serve` config. Precedence is flags > `OTTERSCOPE_*` env > config file > defaults, with the same spellings as the original Go build.
- `rust/crates/otterscope/src/basepath.rs` — one mount prefix for the UI, API, `/mcp`, share links and OTLP receiver; requests outside it are 404, never another app's SPA.
- `rust/crates/otterscope/src/model.rs` — the domain: Run, Step, LlmCall, ToolCall, Message, Status, StepKind. Everything downstream depends on this, not on OTel types.
- `rust/crates/otterscope/src/ingest/` — OTLP/HTTP receiver (protobuf + JSON), per-key rate limiting, and GenAI-dialect normalization (OTel `gen_ai.*` old + experimental, OpenInference, Vercel AI SDK, Claude Agent SDK/Code). Normalization maps raw spans to `model`; this is the most churn-prone area, so quirks stay here behind the model boundary.
- `rust/crates/otterscope/src/store/` — SQLite persistence via `rusqlite` (`bundled`, so SQLite is compiled into the binary). Migrations live in `rust/crates/otterscope/migrations/`, are embedded with `include_str!`, and are sequential and append-only. One connection behind a mutex.
- `rust/crates/otterscope/src/server/` — axum router: JSON API (contract in `docs/api-rust.md`), auth/guard, Prometheus metrics, SSE stream, judge evaluator, and the embedded UI (`rust-embed` reads `web/dist`).
- `rust/crates/otterscope/src/alerts/` — alert rules, the settle-then-notify watcher, and the webhook SSRF gate.
- `rust/crates/otterscope/src/mcp/` — read-only MCP over Streamable HTTP, stateless protocol revision `2026-07-28` only.
- `rust/crates/otterscope/src/pricing/` — maintained pricing table and cost math.
- `rust/crates/otterscope/src/evals/` — deterministic assertions and LLM-as-judge scoring.
- `rust/crates/otterscope/src/sample/` — seeded demo data.
- `web/` — Bun + React 19 + TanStack Router/Query + Tailwind + shadcn-style components, built by Vite. `bun run build` writes `web/dist`, which the server embeds; never serve it from disk in production.

## Hard rules

- **The binary stays single and self-contained.** No external services, no sidecar dependencies, no database server. SQLite and the UI are embedded; "static" means one file to deploy, not a fully static musl build. Any feature that "needs Redis/Postgres" needs a different design instead.
- **OTel types never leak past `src/ingest`.** Downstream code sees only `src/model`.
- **SQLite schema changes only via new migration files.** Never edit an existing migration. A database migrated by a newer version cannot be opened by an older one, so `Store::open` snapshots before applying pending migrations.
- **No breaking changes to the ingest endpoint** once released; agents in the wild must not lose data because we refactored.
- **The store runs on a SINGLE SQLite connection.** Never hold a rows cursor open while issuing another query/write on the same call path — it self-deadlocks. Drain and close cursors first.
- **Keep the JSON contract in one place.** A wire change updates `docs/api-rust.md`, `web/src/api/contract.ts`, and the tests together. All JSON is `camelCase`; every failure is the one `{"error": "…"}` envelope.
- **CLI, env and config precedence are compatibility surface.** Flags override `OTTERSCOPE_*`, which overrides the config file, which overrides defaults. Do not rename a flag or variable to make the Rust code prettier.
- Dependencies are a liability: prefer std and the existing tokio/axum/rusqlite stack; justify each new crate in the PR/commit description.

## Development

- Rust 1.99.1 (pinned in `rust-toolchain.toml`, with rustfmt and clippy), Bun for `web/`.
- Build the UI first, then the binary: `cd web && bun install && bun run build`, then `cd rust && cargo build --release`. The binary is `rust/target/release/otterscope`.
- Build without the UI: `cargo build` (or `cargo build --release`) works because `build.rs` creates a placeholder `web/dist/index.html`; the API, OTLP receiver and `/mcp` run, the UI page says it was built without one.
- Test: `cd rust && cargo test` · `cd web && bun test`. Lint/format: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cd web && bun run lint`.
- Run locally: `cd rust && cargo run -- serve` (or `./target/release/otterscope serve`). The process always binds port 80, so it needs root, `CAP_NET_BIND_SERVICE` on Linux, or the container; UI, API, `/mcp` and OTLP ingest (`POST /v1/traces`) all share that one port.
- README screenshots are generated, not hand-taken: seed a db (`cargo run -- sample`), serve it on the fixed port 80 (a container published as `-p 8080:80` is the easy way; there is no `-listen` knob), then `cd web && OTERSCOPE_URL=http://localhost:8080 bun run screenshots`. The script defaults to `http://localhost:8324`, so pass `OTERSCOPE_URL` unless the server is on host port 80. That script is the only consumer of the `playwright` devDependency — it was once deleted as "unused" (#124), so keep the script and the dependency together. The version comes from `Cargo.toml` (`CARGO_PKG_VERSION`), so bump the workspace version before regenerating release screenshots.
- Test fixtures for ingest live in `rust/crates/otterscope/testdata/` as captured OTLP JSON/protobuf payloads from real frameworks — extend these whenever a new framework/dialect quirk is found.
- Any command whose exit code gates a decision (tests, builds, `gh pr checks`, `gh run watch`) must not be piped through `tail`/`grep`/etc. without `set -o pipefail` — a pipe masks the failure and the gate silently passes (this has bitten twice: PR #21 locally, PR #30 at merge). Bound any command that can hang so a stalled test fails fast.
- CI check failures aren't always code failures — registry/proxy flakes happen (PR #30). Re-run the failed job (`gh run rerun <id> --failed`) before debugging, but never merge on a red check regardless of suspected cause. CI is `.github/workflows/rust.yml`; releases come from `.github/workflows/release.yml`.

## Workflow

Every unit of work follows `docs/WORKFLOW.md` — read it before starting any change. Summary: work is tracked (GitHub Issues are disabled; the port epic is the draft PR), one branch per change, tests + dry-run verification before merge, ADR for any architectural decision, roadmap kept truthful.
