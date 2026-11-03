# Changelog

Notable changes to Otterscope, newest first. Entries describe what changed
for someone running or connecting to an instance; the issue or PR in the
commit history has the rest.

## Unreleased

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

- Ingest rate limiting defaults to a finite 50 batches/sec per ingest key
  with a burst of 100, instead of unlimited. An exposed receiver was a free
  amplifier; `-ingest-rate 0` still turns the limit off explicitly.
- Touched runs are rederived in one grouped query per batch instead of one
  query per run, so a large accepted batch no longer queues behind its own
  rederivation.
- Tokens are listed by id, name, scope and last use. The secret is returned
  only at creation and stored only as a digest, so a database or a backup
  no longer hands out working credentials; rows written by an older release
  are hashed in place the first time the store opens.
- The container image runs as uid 65532 on a scratch base with `/data` as
  the only writable path, and keeps the binary's loopback defaults.
  `docker-compose.yml` publishes both ports on `127.0.0.1` and enables read
  authentication.

### Fixed

- A case collision in the frontend broke the production build on
  case-insensitive filesystems (macOS, Windows). The module is renamed.
- The run list and run detail report an error instead of hanging, and no
  longer truncate silently at 100 runs.
- The compare view debounces filter changes and ignores stale responses;
  timeline bars are clamped to the pane.
- Saved views now apply over the defaults instead of replacing them, so a
  view that does not name every filter keeps the others.
- Retention deletes the full-text index rows before the runs they index and
  vacuums afterwards, so deleted content stops being searchable and the
  file shrinks instead of keeping the text.
- `otterscope_db_size_bytes` includes the WAL and SHM files, which is where
  recent data actually lives in WAL mode.
- Dependency advisories in the frontend tree are cleared.

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
