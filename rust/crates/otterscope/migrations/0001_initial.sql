-- Otterscope baseline schema (ADR-0008).
--
-- This one file replaces the 21 migrations that were transliterated from the
-- Go build. It is the only migration a fresh database ever applies; future
-- schema changes are new numbered files applied after it.
--
-- Standards:
--   * every ordinary table is STRICT (the FTS5 virtual table cannot be);
--   * enums are CHECK-constrained, booleans are INTEGER CHECK (x IN (0,1));
--   * ownership is enforced by foreign keys with ON DELETE CASCADE;
--   * times are unix nanoseconds in *_ns columns, durations end in _secs;
--   * child tables that are keyed by a project scope use (project, id)
--     primary keys so one project cannot overwrite another's data.
--
-- A database created by the old (pre-baseline) schema is refused by
-- Store::open rather than migrated in place. See
-- docs/adr/0008-sqlite-schema-redesign.md.

PRAGMA user_version = 1;

-- Instance metadata. One row, so an accidental second insert is a CHECK
-- failure instead of a silently duplicated fact.
CREATE TABLE instance (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    created_ns INTEGER NOT NULL
) STRICT;

INSERT INTO instance (id, created_ns)
VALUES (1, CAST(strftime('%s', 'now') AS INTEGER) * 1000000000);

-- Projects are the isolation and routing dimension. `name` is the natural
-- key used by the wire contract and by every child table; the integer `id`
-- exists only to keep project listings in creation order.
CREATE TABLE projects (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE,
    ingest_key TEXT NOT NULL UNIQUE,
    created_ns INTEGER NOT NULL
) STRICT;

-- The keyless project keeps zero-config ingest working.
INSERT INTO projects (name, ingest_key, created_ns) VALUES ('default', '', 0);

-- Runs (traces). Aggregates are derived from the run's steps at write time;
-- they are denormalized display data, not independent facts.
CREATE TABLE runs (
    project       TEXT NOT NULL REFERENCES projects (name) ON DELETE CASCADE,
    id            TEXT NOT NULL,
    service       TEXT NOT NULL DEFAULT '',
    agent_name    TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL CHECK (status IN ('running', 'ok', 'error')),
    start_ns      INTEGER NOT NULL,
    end_ns        INTEGER,
    input_tokens  INTEGER NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    output_tokens INTEGER NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    llm_calls     INTEGER NOT NULL DEFAULT 0 CHECK (llm_calls >= 0),
    tool_calls    INTEGER NOT NULL DEFAULT 0 CHECK (tool_calls >= 0),
    -- Comma-joined distinct request models / prompt identities. Display
    -- aggregates; normalized lists live on the steps.
    models        TEXT NOT NULL DEFAULT '',
    prompts       TEXT NOT NULL DEFAULT '',
    -- NULL means no known cost (unknown model or no LLM steps), never 0.
    cost_usd      REAL CHECK (cost_usd IS NULL OR cost_usd >= 0),
    cost_partial  INTEGER NOT NULL DEFAULT 0 CHECK (cost_partial IN (0, 1)),
    error         TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (project, id)
) STRICT;

-- Steps (spans). A step cannot exist without its run, so the foreign key is a
-- composite of the run's scope + id and deletes cascade. It is deferred so a
-- batch can insert steps before its aggregate run row is re-derived in the
-- same transaction.
CREATE TABLE steps (
    project               TEXT NOT NULL,
    id                    TEXT NOT NULL,
    run_id                TEXT NOT NULL,
    parent_id             TEXT NOT NULL DEFAULT '',
    kind                  TEXT NOT NULL CHECK (kind IN ('agent', 'llm', 'tool', 'generic')),
    name                  TEXT NOT NULL,
    service               TEXT NOT NULL DEFAULT '',
    agent_name            TEXT NOT NULL DEFAULT '',
    status                TEXT NOT NULL CHECK (status IN ('running', 'ok', 'error')),
    start_ns              INTEGER NOT NULL,
    end_ns                INTEGER,
    error                 TEXT NOT NULL DEFAULT '',
    provider              TEXT NOT NULL DEFAULT '',
    request_model         TEXT NOT NULL DEFAULT '',
    response_model        TEXT NOT NULL DEFAULT '',
    input_tokens          INTEGER NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    output_tokens         INTEGER NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    cache_read_tokens     INTEGER NOT NULL DEFAULT 0 CHECK (cache_read_tokens >= 0),
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0 CHECK (cache_creation_tokens >= 0),
    reasoning_tokens      INTEGER NOT NULL DEFAULT 0 CHECK (reasoning_tokens >= 0),
    tool_name             TEXT NOT NULL DEFAULT '',
    tool_call_id          TEXT NOT NULL DEFAULT '',
    -- Kind-specific JSON payload (messages for LLM steps, arguments/result
    -- for tool steps). Never filtered in SQL; read whole by the inspector.
    detail_json           TEXT NOT NULL DEFAULT ''
                              CHECK (detail_json = '' OR json_valid(detail_json)),
    cost_usd              REAL CHECK (cost_usd IS NULL OR cost_usd >= 0),
    prompt                TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (project, id),
    FOREIGN KEY (project, run_id) REFERENCES runs (project, id)
        ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE INDEX steps_run_start_idx ON steps (project, run_id, start_ns);

-- Full-text search over step content. External content: the FTS index stores
-- only the inverted index, and the triggers below are the only write path, so
-- it cannot drift. `detail_json` is indexed as text, which covers message and
-- tool payload words. The sweep rebuilds the index after VACUUM (VACUUM may
-- renumber rowids of tables without an INTEGER PRIMARY KEY).
CREATE VIRTUAL TABLE steps_fts USING fts5(
    name,
    provider,
    request_model,
    tool_name,
    tool_call_id,
    detail_json,
    prompt,
    content = 'steps',
    content_rowid = 'rowid',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER steps_fts_ai AFTER INSERT ON steps BEGIN
    INSERT INTO steps_fts (rowid, name, provider, request_model, tool_name,
                           tool_call_id, detail_json, prompt)
    VALUES (new.rowid, new.name, new.provider, new.request_model, new.tool_name,
            new.tool_call_id, new.detail_json, new.prompt);
END;

CREATE TRIGGER steps_fts_ad AFTER DELETE ON steps BEGIN
    INSERT INTO steps_fts (steps_fts, rowid, name, provider, request_model,
                           tool_name, tool_call_id, detail_json, prompt)
    VALUES ('delete', old.rowid, old.name, old.provider, old.request_model,
            old.tool_name, old.tool_call_id, old.detail_json, old.prompt);
END;

CREATE TRIGGER steps_fts_au AFTER UPDATE ON steps BEGIN
    INSERT INTO steps_fts (steps_fts, rowid, name, provider, request_model,
                           tool_name, tool_call_id, detail_json, prompt)
    VALUES ('delete', old.rowid, old.name, old.provider, old.request_model,
            old.tool_name, old.tool_call_id, old.detail_json, old.prompt);
    INSERT INTO steps_fts (rowid, name, provider, request_model, tool_name,
                           tool_call_id, detail_json, prompt)
    VALUES (new.rowid, new.name, new.provider, new.request_model, new.tool_name,
            new.tool_call_id, new.detail_json, new.prompt);
END;

-- Raw ingested OTLP batches, retained for renormalization. A batch can span
-- several runs, so it belongs to a project, not to a run.
CREATE TABLE raw_batches (
    -- AUTOINCREMENT, not a bare rowid alias: `renormalize` pages by
    -- `id > last_seen`, so ids must never be reused after a retention delete.
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    received_ns INTEGER NOT NULL,
    project     TEXT NOT NULL REFERENCES projects (name) ON DELETE CASCADE,
    payload     BLOB NOT NULL
) STRICT;

CREATE INDEX raw_batches_received_idx ON raw_batches (received_ns);
CREATE INDEX raw_batches_project_idx ON raw_batches (project);

-- Assertions, scoped to a project.
CREATE TABLE assertions (
    id         INTEGER PRIMARY KEY,
    project    TEXT NOT NULL REFERENCES projects (name) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    type       TEXT NOT NULL CHECK (type IN (
                   'contains', 'not_contains', 'regex', 'is_json',
                   'max_latency_ms', 'max_cost_usd', 'llm_judge')),
    config     TEXT NOT NULL DEFAULT '',
    enabled    INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_ns INTEGER NOT NULL,
    UNIQUE (project, name),
    -- Required as the parent key for assertion_results' composite FK.
    UNIQUE (project, id)
) STRICT;

-- Per-run verdicts. Both parents cascade: deleting a run or an assertion
-- removes its results, which is why the store no longer needs application
-- level cleanup.
CREATE TABLE assertion_results (
    project      TEXT NOT NULL,
    run_id       TEXT NOT NULL,
    assertion_id INTEGER NOT NULL,
    pass         INTEGER NOT NULL CHECK (pass IN (0, 1)),
    detail       TEXT NOT NULL DEFAULT '',
    evaluated_ns INTEGER NOT NULL,
    PRIMARY KEY (project, run_id, assertion_id),
    FOREIGN KEY (project, run_id) REFERENCES runs (project, id) ON DELETE CASCADE,
    FOREIGN KEY (project, assertion_id) REFERENCES assertions (project, id)
        ON DELETE CASCADE
) STRICT;

CREATE INDEX assertion_results_assertion_idx ON assertion_results (assertion_id, pass);

-- Alert rules and their watcher-owned firing state.
CREATE TABLE alerts (
    id               INTEGER PRIMARY KEY,
    project          TEXT NOT NULL REFERENCES projects (name) ON DELETE CASCADE,
    name             TEXT NOT NULL,
    type             TEXT NOT NULL CHECK (type IN (
                         'error_rate', 'cost', 'p95_latency', 'assertion_fail_rate')),
    threshold        REAL NOT NULL CHECK (threshold >= 0),
    window_secs      INTEGER NOT NULL DEFAULT 3600 CHECK (window_secs > 0),
    config           TEXT NOT NULL DEFAULT '',
    webhook_url      TEXT NOT NULL,
    enabled          INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    firing           INTEGER NOT NULL DEFAULT 0 CHECK (firing IN (0, 1)),
    last_fired_ns    INTEGER,
    settle_secs      INTEGER NOT NULL DEFAULT 0 CHECK (settle_secs >= 0),
    -- NULL means nothing is pending, so a timestamp of 0 never has to mean
    -- "not a timestamp".
    pending_since_ns INTEGER,
    created_ns       INTEGER NOT NULL,
    UNIQUE (project, name)
) STRICT;

CREATE INDEX alerts_enabled_idx ON alerts (enabled, id);
CREATE INDEX alerts_firing_idx ON alerts (firing) WHERE firing = 1;

-- Opt-in public read-only links to a run.
CREATE TABLE shares (
    token      TEXT PRIMARY KEY,
    project    TEXT NOT NULL,
    run_id     TEXT NOT NULL,
    created_ns INTEGER NOT NULL,
    FOREIGN KEY (project, run_id) REFERENCES runs (project, id) ON DELETE CASCADE
) STRICT;

CREATE INDEX shares_run_idx ON shares (project, run_id);

-- Named saved filter/search combinations. These are instance-wide: the JSON
-- contract has no project field, so neither does the table.
CREATE TABLE saved_views (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    params_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(params_json)),
    created_ns  INTEGER NOT NULL
) STRICT;

-- Bearer tokens. Only the SHA-256 digest is stored; `read_tokens` from the
-- old schema was renamed because admin-scope tokens live here too.
CREATE TABLE tokens (
    id           TEXT PRIMARY KEY,
    token_hash   TEXT NOT NULL UNIQUE CHECK (length(token_hash) = 64),
    name         TEXT NOT NULL,
    scope        TEXT NOT NULL DEFAULT 'read' CHECK (scope IN ('read', 'admin')),
    created_ns   INTEGER NOT NULL,
    last_used_ns INTEGER
) STRICT;

CREATE INDEX tokens_created_idx ON tokens (created_ns);
CREATE INDEX tokens_last_used_idx ON tokens (last_used_ns);

-- Change history: what mutated, and when.
CREATE TABLE audit_log (
    id     INTEGER PRIMARY KEY,
    at_ns  INTEGER NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('create', 'delete')),
    entity TEXT NOT NULL CHECK (entity IN (
               'assertion', 'alert', 'project', 'token', 'view', 'share')),
    detail TEXT NOT NULL DEFAULT ''
) STRICT;

CREATE INDEX audit_log_at_idx ON audit_log (at_ns DESC);

-- Run list filters: newest-first, per project, per status and per service,
-- plus the unfiltered newest-first listing. Model/prompt filters are substring
-- matches (LIKE '%x%'), which no B-tree can serve; they are kept for contract
-- compatibility and documented as unindexed in ADR-0008.
CREATE INDEX runs_start_idx ON runs (start_ns DESC);
CREATE INDEX runs_project_start_idx ON runs (project, start_ns DESC);
CREATE INDEX runs_status_start_idx ON runs (status, start_ns DESC);
CREATE INDEX runs_service_start_idx ON runs (service, start_ns DESC);
