# Development

Lightweight, self-hosted observability + evals for AI agents. One self-contained
Rust binary, embedded SQLite, embedded web UI. Positioning: "Plausible for AI
agents" — the anti-ClickHouse-stack. Read [`roadmap.md`](roadmap.md) for
milestones and [`adr/`](adr/) for decisions already made (do not relitigate them
silently — write a new ADR to change one). ADR-0006 records the port from Go to
Rust and the Bun/React UI; the Rust workspace under `rust/` and the UI under
`web/` are the only implementations.

This guide is for anyone working on the code, human or otherwise. The operating
procedure for a change is [`workflow.md`](workflow.md).

## Architecture

- `rust/crates/otterscope/src/main.rs` — CLI entrypoint (`serve`, `version`,
  `sample`, `token`, `project`, `backup`, `renormalize`).
- `rust/crates/otterscope/src/cli.rs` — database subcommands and the config/flag
  plumbing they share with `serve`.
- `rust/crates/otterscope/src/args.rs` — Go-`flag`-compatible parser: one or two
  dashes, `-name=value` or `-name value`, boolean flags, stop at the first
  operand.
- `rust/crates/otterscope/src/config.rs` — effective `serve` config. Precedence
  is flags > `OTTERSCOPE_*` env > config file > defaults, with the same
  spellings as the original Go build.
- `rust/crates/otterscope/src/basepath.rs` — one mount prefix for the UI, API,
  `/mcp`, share links and OTLP receiver; requests outside it are 404, never
  another app's SPA.
- `rust/crates/otterscope/src/model.rs` — the domain: Run, Step, LlmCall,
  ToolCall, Message, Status, StepKind. Everything downstream depends on this,
  not on OTel types.
- `rust/crates/otterscope/src/ingest/` — OTLP/HTTP receiver (protobuf + JSON),
  per-key rate limiting, and GenAI-dialect normalization (OTel `gen_ai.*` old +
  experimental, OpenInference, Vercel AI SDK, Claude Agent SDK/Code).
  Normalization maps raw spans to `model`; this is the most churn-prone area, so
  quirks stay here behind the model boundary.
- `rust/crates/otterscope/src/store/` — SQLite persistence via `rusqlite`
  (`bundled`, so SQLite is compiled into the binary). Migrations live in
  `rust/crates/otterscope/migrations/`, are embedded with `include_str!`, and
  are sequential and append-only after the `0001_initial.sql` baseline. The
  baseline is `STRICT` with cascading foreign keys and `CHECK`-constrained
  enums (ADR-0008); a database from the old pre-baseline schema is refused,
  not upgraded. One connection behind a mutex.
- `rust/crates/otterscope/src/server/` — axum router: JSON API (contract in
  [`api-rust.md`](api-rust.md)), auth/guard, Prometheus metrics, SSE stream,
  judge evaluator, and the embedded UI (`rust-embed` reads `web/dist`).
- `rust/crates/otterscope/src/alerts/` — alert rules, the settle-then-notify
  watcher, and the webhook SSRF gate.
- `rust/crates/otterscope/src/mcp/` — read-only MCP over Streamable HTTP,
  stateless protocol revision `2026-07-28` only.
- `rust/crates/otterscope/src/pricing/` — maintained pricing table and cost math.
- `rust/crates/otterscope/src/evals/` — deterministic assertions and LLM-as-judge
  scoring.
- `rust/crates/otterscope/src/sample/` — seeded demo data.
- `web/` — Bun + React 19 + TanStack Router/Query + Tailwind + shadcn-style
  components, built by Vite. `bun run build` writes `web/dist`, which the server
  embeds; never serve it from disk in production.

## Hard rules

- **The binary stays single and self-contained.** No external services, no
  sidecar dependencies, no database server. SQLite and the UI are embedded;
  "static" means one file to deploy, not a fully static musl build. Any feature
  that "needs Redis/Postgres" needs a different design instead.
- **OTel types never leak past `src/ingest`.** Downstream code sees only
  `src/model`.
- **SQLite schema changes only via new migration files.** Never edit an existing
  migration. A database migrated by a newer version cannot be opened by an older
  one, so `Store::open` snapshots before applying pending migrations. A
  database from the old pre-baseline schema, or any database with an unknown
  migration history, is refused with an explicit message — do not add an
  in-place upgrade for it; start a new database instead
  ([ADR-0008](adr/0008-sqlite-schema-redesign.md)).
- **No breaking changes to the ingest endpoint** once released; agents in the
  wild must not lose data because we refactored.
- **The store runs on a SINGLE SQLite connection.** Never hold a rows cursor open
  while issuing another query/write on the same call path — it self-deadlocks.
  Drain and close cursors first.
- **Keep the JSON contract in one place.** A wire change updates
  [`api-rust.md`](api-rust.md), `web/src/api/contract.ts`, and the tests
  together. All JSON is `camelCase`; every failure is the one
  `{"error": "…"}` envelope.
- **CLI, env and config precedence are compatibility surface.** Flags override
  `OTTERSCOPE_*`, which overrides the config file, which overrides defaults. Do
  not rename a flag or variable to make the Rust code prettier.
- Dependencies are a liability: prefer std and the existing tokio/axum/rusqlite
  stack; justify each new crate in the PR/commit description.

## Development

- Rust 1.99.1 (pinned in `rust-toolchain.toml`, with rustfmt and clippy), Bun
  for `web/`.
- Build the UI first, then the binary: `cd web && bun install && bun run build`,
  then `cd rust && cargo build --release`. The binary is
  `rust/target/release/otterscope`.
- Build without the UI: `cargo build` (or `cargo build --release`) works because
  `build.rs` creates a placeholder `web/dist/index.html`; the API, OTLP receiver
  and `/mcp` run, the UI page says it was built without one.
- The whole gate is local (there is no CI; see below): `cargo fmt --all --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test` in `rust/`;
  `bun run lint`, `bun test`, `bun run build` in `web/`.
- Run locally: `cd rust && cargo run -- serve` (or
  `./target/release/otterscope serve`). The process always binds port 80, so it
  needs root or `CAP_NET_BIND_SERVICE`; UI, API, `/mcp` and OTLP ingest
  (`POST /v1/traces`) all share that one port. The supported way to run the
  whole deployment is k3d: [`deploy/k8s/deploy.sh`](../deploy/k8s/deploy.sh);
  see [`deploy/readme.md`](../deploy/readme.md).
- Readme screenshots are generated, not hand-taken: seed a db
  (`cargo run -- sample`), serve it (the k3d port-forward on 8080 is the easy
  way; there is no `-listen` knob), then
  `cd web && OTTERSCOPE_URL=http://localhost:8080 bun run screenshots`. The
  script defaults to `http://localhost:8324`, so pass `OTTERSCOPE_URL` unless
  the server is on that port. That script is the only consumer of the
  `playwright` devDependency — it was once deleted as "unused" (#124), so keep
  the script and the dependency together. `otterscope version` reports
  `OTTERSCOPE_VERSION` when set (the image build passes the git SHA as
  `--build-arg version`) and `CARGO_PKG_VERSION` otherwise, so bump the
  workspace version before regenerating screenshots from a plain cargo build.
- Test fixtures for ingest live in `rust/crates/otterscope/testdata/` as captured
  OTLP JSON/protobuf payloads from real frameworks — extend these whenever a new
  framework/dialect quirk is found.
- Any command whose exit code gates a decision (tests, builds, the local gate
  below) must not be piped through `tail`/`grep`/etc. without
  `set -o pipefail` — a pipe masks the failure and the gate silently passes (this
  has bitten twice: PR #21 locally, PR #30 before the port). Bound any command
  that can hang so a stalled test fails fast.
- **There is no CI.** CI was removed by operator decision: the repository holds
  no workflow files and no hosted CI service replaces them; do not re-add
  Actions or an equivalent. Every gate is run locally, from a clean tree, before
  merge:

  ```sh
  cd rust && cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test
  cd web && bun run lint && bun test && bun run build
  ```

  Do not merge if any of the six fails. A flaky failure is still a failure:
  re-run the command, then debug; there is no job to re-run. Deployment is
  Kubernetes via k3d: `deploy/k8s/deploy.sh` builds the image from the current
  commit, tags it with the short git SHA, imports it into the local cluster and
  applies the manifests. Nothing is pushed anywhere, and there is no packaging
  or release pipeline ([ADR-0009](adr/0009-kubernetes-k3d-deployment.md)).

## Workflow

Every unit of work follows [`workflow.md`](workflow.md) — read it before starting
any change. Summary: work is tracked (GitHub Issues are disabled; the port epic
is the draft PR), one branch per change, tests + dry-run verification before
merge, ADR for any architectural decision, roadmap kept truthful.
