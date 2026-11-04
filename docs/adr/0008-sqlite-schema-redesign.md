# 0008 — SQLite schema redesign

- Status: accepted
- Date: 2026-11-04
- Supersedes: the database-compatibility part of [ADR-0006](0006-rust-port.md)

## Context

The Rust port carried over the Go implementation's SQLite schema verbatim: 21
sequential migrations, most of them `ALTER TABLE`/table-rebuild fixes for a
shape that had grown by accretion. ADR-0006 deliberately preserved that schema
so an existing database opened without a conversion step. That constraint is
now gone: the Go implementation has been removed and the project is
pre-release, so there is no installed base whose database must keep working.

The inherited shape had real problems:

- no `STRICT` tables, so a column's declared type was advisory;
- almost no foreign keys, so orphaned steps, verdicts and share links were
  possible and retention needed hand-written cleanup;
- run status, step kind, token scope, alert type and assertion type were
  unconstrained `TEXT`, and reads silently coerced an unknown value to a
  default variant;
- names had drifted (`meta`, `read_tokens` after admin tokens existed,
  `shared_runs`, `steps.detail`, `steps_fts` as a standalone table);
- `end_ns`/`pending_since_ns`/`last_fired_ns` used `0` as "not set", which is
  indistinguishable from an actual timestamp;
- the standalone FTS5 table was written by hand in two places (ingest and
  backfill) and had to be cleaned up by the retention sweep.

## Decision

Replace the 21 migrations with **one baseline**, `0001_initial.sql`, and adopt
the following standards for it and for all future migrations:

1. **`STRICT` tables.** Every ordinary table is `STRICT` with `INTEGER`, `TEXT`,
   `REAL` or `BLOB` columns and no `ANY`. FTS5 virtual tables cannot be
   `STRICT`; that is the only exception.
2. **Enforcing foreign keys.** `PRAGMA foreign_keys=ON` (already set) plus
   `ON DELETE CASCADE` where the child cannot exist without the parent:
   steps → runs, assertion results → runs and → assertions, shares → runs,
   raw batches → projects, runs/assertions/alerts → projects. The steps FK is
   `DEFERRABLE INITIALLY DEFERRED` so a batch can write steps before the
   aggregate run row is derived in the same transaction. `SET NULL` is not
   used because no relation in this domain allows an orphan.
3. **`CHECK` constraints for every enum**, plus `INTEGER CHECK (x IN (0,1))`
   for booleans: run status, step kind, token scope, alert type, assertion
   type, audit action/entity, and the boolean columns.
4. **`NOT NULL` by default**, with a nullable column only where absence is a
   real domain state (`end_ns`, `cost_usd`, `last_used_ns`, `last_fired_ns`,
   `pending_since_ns`). Timestamps are unix nanoseconds in `*_ns` columns;
   durations carry `_secs`; `steps.detail` became `detail_json` and
   `saved_views.params` became `params_json` so the name matches the content.
5. **Naming.** Plural snake_case tables; `projects.name` is the natural key
   used by the wire contract and child tables; `read_tokens` became `tokens`
   (admin tokens live there too), `shared_runs` became `shares`, `meta` became
   the single-row `instance`, and `schema_migrations.applied_at` became
   `applied_ns`.
6. **Real uniqueness.** `UNIQUE` constraints back every 409: project name,
   saved-view name, token hash, share token, assertion name per project and
   alert name per project. Application-level "does it exist" checks are gone;
   the store maps `UNIQUE constraint failed` to `store::Error::Conflict`.
7. **Indexes that match the queries.** Newest-first, per-project, per-status
   and per-service run listings; step lookup by run; assertion-result joins;
   enabled/firing alerts; token digest, creation and last use. The step search
   is an FTS5 **external-content** table with insert/update/delete triggers, so
   it cannot drift from `steps` and a cascaded step delete purges its index row.
   The sweep rebuilds the index after `VACUUM`, which is allowed to renumber
   rowids on tables without an `INTEGER PRIMARY KEY`.
8. **Typed enums at the store boundary.** `Status`, `StepKind`, `Scope`,
   `AlertKind` and `AssertionKind` implement `as_str`/`TryFrom<&str>`. The store
   decodes constrained columns through them and refuses to coerce a corrupt row
   into a default variant. The public store/DTO field types stay as they were
   where the wire contract depends on them.

### Legacy refusal

A database created by the pre-redesign build must not be half-migrated.
`Store::open` inspects `sqlite_schema` and the recorded migration names before
writing anything:

- no user tables → fresh database, apply the baseline;
- every recorded migration is known and all are applied → open;
- a known prefix with pending work → snapshot (as before), then apply;
- any unknown migration name, tables without a history, or tables with an empty
  history → refuse with a message that names the old migrations, says the
  database cannot be opened by this build, and tells the operator to start a
  new database (or restore a backup). The refusal does not create a snapshot,
  because a refusal is not an upgrade.

### Deliberate exceptions

- `saved_views` stays instance-wide. The JSON contract has no project field
  for a view and the existing name uniqueness is global, so there is no
  project foreign key to add without changing the contract.
- `runs.models` / `runs.prompts` remain comma-joined display aggregates
  derived from steps. They are not an independent list and normalizing them
  would not change any query.
- The `model`/`prompt` run filters are `LIKE '%needle%'` substring matches.
  No B-tree can serve a leading wildcard, and a trigram FTS index would change
  matching for needles shorter than three characters, so these two filters
  remain unindexed by design rather than adding an index that cannot be used.
- `assertions.type`, `alerts.type` and `Assertion.r#type` keep the wire spelling
  `type`; the typed variants live in `AssertionKind`/`AlertKind` and are used
  for every bind and read.

## Consequences

- **Breaking change.** A database from the old implementation is refused, not
  upgraded. Operators must start a new database and re-ingest; the error
  message says so. This is acceptable because no compatibility promise remains
  (ADR-0006) and the project is pre-release.
- The JSON API, OTLP ingest, MCP semantics, CLI flags and the port-80 listener
  are unchanged. This is an internal storage redesign.
- `Store::open` still snapshots before applying a *pending* migration added
  after the baseline. Future schema changes remain new numbered files; the
  baseline itself is never edited.
- SQLite now enforces invariants the application used to check (or forgot to).
  A write that violates one fails at the database, which is the intended
  fail-closed behaviour.
- Typed reads turn a corrupt row into an error instead of silently reporting
  `running`/`generic`/`read`.
- The FTS index is maintained by triggers only. The ingest path no longer
  writes it, so there is one write path and no backfill to keep in step.
- A missing `end_ns` is now `NULL` instead of `0`. Stats treat it as a
  zero-length run, matching what the JSON API already reported for
  `durationMs`; the old sentinel made a running run's duration negative.
- `raw_batches.id` keeps `AUTOINCREMENT`. `renormalize` pages by
  `id > last_seen`, so ids must not be reused after a retention delete.
