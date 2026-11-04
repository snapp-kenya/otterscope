# Changelog

Notable changes to Otterscope, newest first. Entries describe what changed
for someone running or connecting to an instance; the issue or PR in the
commit history has the rest.

## Unreleased

### Breaking

- The SQLite schema is redesigned from a single `STRICT` baseline
  (`0001_initial.sql`) instead of the 21 migrations inherited from the Go
  build: real column types, `CHECK`-constrained enums, cascading foreign keys
  and an external-content FTS5 index. **A database created by an older
  Otterscope is refused**, with an error telling the operator to start a new
  database (or restore a backup) and re-ingest; there is no in-place upgrade.
  The JSON API, OTLP ingest, MCP, CLI flags and port remain unchanged. See
  [ADR-0008](docs/adr/0008-sqlite-schema-redesign.md).
- The server now has **one HTTP listener, always on port 80**. The UI, JSON
  API, `/mcp`, share links and OTLP ingest (`POST /v1/traces`) all live there;
  there is no separate OTLP port. `-listen`, `-otlp`, `OTTERSCOPE_LISTEN`,
  `OTTERSCOPE_OTLP` and the `listen`/`otlp` config-file keys are removed, and a
  config file that still contains them fails to parse rather than silently
  ignoring the value. Deployments that published 8317/4318 must publish port 80
  instead; the host-side port is a deployment choice (`-p 8080:80`, Istio,
  NodePort, …). Docker lets the non-root image bind 80 because its runtime sets
  `net.ipv4.ip_unprivileged_port_start=0`; Kubernetes/containerd and native
  runs need `CAP_NET_BIND_SERVICE`.
  TLS is not this service's job.
- The HTTP JSON API is redesigned for the Rust server: `camelCase` fields,
  typed enums for run status, step kind and token scope, one
  `{"error": "…"}` envelope for every failure, and response arrays that are
  always present. Clients built against the Go JSON shapes need updating;
  [docs/api-rust.md](docs/api-rust.md) is the contract.
- MCP now speaks protocol revision `2026-07-28` only, the stateless revision.
  The `initialize` handshake and `Mcp-Session-Id` are gone; clients pinned to
  an older revision are rejected with `UnsupportedProtocolVersionError`. See
  [docs/mcp.md](docs/mcp.md).

### Added

- A configurable base path (`-base-path`, `OTTERSCOPE_BASE_PATH`, or
  `"basePath"` in the config file) that serves the UI, API, `/mcp`, share
  links and the OTLP receiver under one URL prefix. The prefix is stripped
  once at the edge, and a request outside it is a 404 rather than the SPA,
  so a misrouted path cannot land on another application's index. The
  frontend is built with the same value and builds every API URL through one
  base-aware helper.
- Read and admin token scopes. `otterscope token add <name>` mints a read
  token; `-admin` mints one that may also mutate. Mutating routes — tokens,
  assertions, saved views, alerts, on-demand evaluation — require the admin
  scope. Tokens that predate the change migrate to read-only, so an upgrade
  cannot widen one silently.
- Per-batch ingest caps of 10,000 spans and 1,000 distinct trace ids
  (`-ingest-max-spans`, `-ingest-max-trace-ids`,
  `maxSpansPerBatch`/`maxTraceIdsPerBatch`). An oversized batch is refused
  whole, before normalization: a 20,000-span batch used to hold the single
  SQLite writer for 80 seconds and is now refused in 7 ms.

### Changed

- Deployment is Kubernetes via k3d only. `deploy/k8s/deploy.sh` builds the
  image from the current commit, tags it with the short git SHA, imports it
  into the local cluster and applies the manifests. The SHA is passed as
  `--build-arg version` and is what `otterscope version` and `/healthz`
  report. No image is pushed anywhere; see
  [ADR-0009](docs/adr/0009-kubernetes-k3d-deployment.md).
- Documentation and the container build file are renamed to lowercase:
  `readme.md`, `changelog.md`, `license`, `deploy/readme.md`,
  `docs/frameworks/readme.md`, `docs/roadmap.md`, `docs/workflow.md` and
  `dockerfile`. The Cargo manifests keep their required `Cargo.toml`/
  `Cargo.lock` names. All links and build commands point at the new names;
  file contents are unchanged.
- The backend is ported from Go to Rust (axum, rusqlite with bundled SQLite,
  prost, rust-embed). The binary stays single and self-contained with the UI
  embedded. CLI flags, `OTTERSCOPE_*` variables and the flags > env > file >
  defaults precedence keep their old spellings. The SQLite schema was
  redesigned after the port (ADR-0008); future schema changes remain new
  numbered migration files.
- Builds now come from the Rust workspace after the frontend is built:
  `cd web && bun install && bun run build`, then `cd rust && cargo build
  --release`. The old Go build and its installation path are retired.
- The container image built from `dockerfile` is the only artifact. Packaging
  and release channels are withdrawn by operator decision: no archives, no
  checksums, no signing, no GitHub Releases, no GHCR image and no
  package-manager distribution (ADR-0004 amendment,
  [docs/adr/0004-release-channels.md](docs/adr/0004-release-channels.md)).
  Deployment is that image, built locally and imported into the k3d cluster
  (ADR-0009).
- The web UI is rebuilt on Bun + React 19 + TanStack Router/Query + Tailwind +
  shadcn-style components. `bun test` replaces vitest, `bun run lint` keeps
  oxlint, and `bun run build` produces the `web/dist` the server embeds.
- Ingest rate limiting defaults to a finite 50 batches/sec per ingest key
  with a burst of 100, instead of unlimited. An exposed receiver was a free
  amplifier; `-ingest-rate 0` still turns the limit off explicitly.
- Touched runs are rederived in one grouped query per batch instead of one
  query per run, so a large accepted batch no longer queues behind its own
  rederivation.
- Tokens are listed by id, name, scope and last use. The secret is returned
  only at creation and stored only as a digest, so a database or a backup no
  longer hands out working credentials. The `read_tokens` table became
  `tokens` because it also holds admin-scope tokens.
- The container image runs as uid 65532 on a distroless base with `/data` as
  the only writable path, and keeps the binary's loopback defaults. It is built
  and imported into the local k3d cluster by `deploy/k8s/deploy.sh`.

### Removed

- `docker-compose.yml` is deleted. k3d is the only deployment target; the
  readme quick start, deploy docs and MCP examples use `deploy.sh` and a
  port-forward instead.
- `docs/launch/` is deleted: the Show HN, awesome-selfhosted,
  digitalocean-community and r/selfhosted drafts described a public product
  launch that is not this deployment.
- Registry and push instructions are removed from the manifests and docs. The
  Deployment and CronJob use the `otterscope:dev` placeholder that `deploy.sh`
  replaces with the git SHA, and `imagePullPolicy: IfNotPresent`.
- The `.github/` directory is removed entirely: no GitHub Actions workflows and
  no issue templates. No CI service replaces it — the six quality gates are run
  locally before merge. There is no release pipeline either: deployment is the
  locally built image imported into the k3d cluster (ADR-0009). The gate itself
  is documented in [docs/development.md](docs/development.md).
- `docs/packaging.md` and `scripts/release.sh` are deleted. With no archives to
  build, the archive script and the packaging document had no consumer, and
  `scripts/` is gone with them.
- Release channels are withdrawn by operator decision (2026-11-04, ADR-0004
  amendment): no archives, checksums, signing, GitHub Releases, GHCR image or
  package-manager distribution. The only artifact is the container image, built
  locally and imported into the k3d cluster; nothing is pushed. Deployment is
  described in [deploy/readme.md](deploy/readme.md).
- `CLAUDE.md` is replaced by [docs/development.md](docs/development.md), which
  keeps the architecture map, the hard rules, the development commands and the
  no-CI gate, written without vendor-specific tooling references.
- The frozen Go implementation (`cmd/`, `internal/`, `go.mod`, `go.sum`,
  `web/embed.go`) is deleted; the Rust workspace is now the only implementation.
  The Go-era SQLite schema was replaced by the baseline described under
  "Breaking"; databases in that old format are refused.

### Fixed

- A case collision in the frontend broke the production build on
  case-insensitive filesystems (macOS, Windows). The module is renamed.
- The run list and run detail report an error instead of hanging, and no
  longer truncate silently at 100 runs.
- The compare view debounces filter changes and ignores stale responses;
  timeline bars are clamped to the pane.
- Saved views now apply over the defaults instead of replacing them, so a
  view that does not name every filter keeps the others.
- Saving or deleting a view and creating, editing or deleting an alert no
  longer dead-ends with `403 admin token required` for a read-token user:
  every admin-scoped mutation now asks once for an admin token and retries,
  as share minting and revocation already did.
- Retention deletes the full-text index rows before the runs they index and
  vacuums afterwards, so deleted content stops being searchable and the
  file shrinks instead of keeping the text.
- `otterscope_db_size_bytes` includes the WAL and SHM files, which is where
  recent data actually lives in WAL mode.
- Dependency advisories in the frontend tree are cleared.
- `serve` exits 2 (Go's flag-parse code) for an invalid `-read-auth`,
  `-retention`, `-ingest-rate`, `-base-path` or similar flag value instead of
  reporting a runtime failure with exit 1. `-h`/`-help` now prints each
  command's usage to stdout and exits 0; unknown flags still exit 2.
- Starting a root-built image with `-base-path` logs a prominent warning that
  the embedded UI was built for another base, instead of silently serving a UI
  that calls the wrong paths. The API, MCP and OTLP receiver still start.
- `project list` no longer claims keyless ingest is open when the CLI cannot
  see the server's `-read-auth` setting; the explicit disabled wording remains
  when `OTTERSCOPE_READ_AUTH` is visible in its own environment.

### Security

- Origin and Host checks cover the whole API, not only `/mcp`, so a
  malicious web page cannot drive-by-write to a local instance. Host is
  checked separately because a DNS-rebound request arrives with no Origin.
- Writes must be `application/json`. A cross-site form post, the one write
  shape a browser sends without a preflight, can no longer be parsed as a
  command.
- Responses carry a strict CSP (`default-src 'self'`), `nosniff` and
  `no-referrer`.
- Both listeners set read-header, read and idle timeouts and a 64 KiB
  header cap. The write timeout is deliberately unset so a long-lived SSE
  stream is not cut mid-connection.
- `/metrics` sits behind the token check: run counts, project count and
  database size are as sensitive as `/api/stats`.
- A foreign service can no longer overwrite an existing span by id. The
  span is rejected and reported in OTLP partial success, so a leaked ingest
  key cannot rewrite another service's trace.
- Ingest requires an ingest key whenever read authentication is on; the
  keyless `default` project is closed.
- Webhook URLs are validated on creation and again on every POST, refusing
  loopback, link-local and private ranges, including through redirects.
  Stored URLs are redacted on read.
- An unbounded evaluate call is refused. A call is capped at 500 runs and
  100 judge calls by default, with skipped counts reported, and the judge
  model must come from the configured `-judge-models` allow-list.
- The database, its WAL and SHM siblings are created 0600, in a directory the
  store creates as 0700, and existing wide files are tightened on open.
  Snapshots are 0600, and only the newest pre-migration backup is kept.
