use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Duration, Utc};

use crate::alerts::Rule;
use crate::evals::{Assertion, Result as EvalResult};
use crate::model::{LlmCall, Message, Status, Step, StepKind, ToolCall};

use super::*;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("otterscope-store-{}-{unique}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct TestStore {
    store: Store,
    dir: TempDir,
    path: String,
}

impl TestStore {
    fn open() -> Self {
        let dir = TempDir::new();
        let path = dir.join("test.db");
        let path = path.to_str().unwrap().to_string();
        let store = Store::open(&path).unwrap();
        Self { store, dir, path }
    }
}

impl std::ops::Deref for TestStore {
    type Target = Store;

    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

fn ts(sec: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(sec, 0).unwrap()
}

fn sample_run(run_id: &str, base: i64) -> Vec<Step> {
    vec![
        Step {
            id: format!("{run_id}-root"),
            project: String::new(),
            run_id: run_id.to_string(),
            parent_id: String::new(),
            kind: StepKind::Agent,
            name: "invoke_agent support".to_string(),
            service: "support-agent".to_string(),
            agent_name: "support".to_string(),
            status: Status::Ok,
            start: ts(base),
            end: Some(ts(base + 10)),
            error: String::new(),
            llm: None,
            tool: None,
        },
        Step {
            id: format!("{run_id}-llm"),
            project: String::new(),
            run_id: run_id.to_string(),
            parent_id: format!("{run_id}-root"),
            kind: StepKind::Llm,
            name: "chat claude-sonnet-5".to_string(),
            service: String::new(),
            agent_name: String::new(),
            status: Status::Ok,
            start: ts(base + 1),
            end: Some(ts(base + 4)),
            error: String::new(),
            llm: Some(LlmCall {
                provider: "anthropic".to_string(),
                request_model: "claude-sonnet-5".to_string(),
                input_tokens: 800,
                output_tokens: 150,
                ..LlmCall::default()
            }),
            tool: None,
        },
        Step {
            id: format!("{run_id}-tool"),
            project: String::new(),
            run_id: run_id.to_string(),
            parent_id: format!("{run_id}-root"),
            kind: StepKind::Tool,
            name: "execute_tool get_ticket".to_string(),
            service: String::new(),
            agent_name: String::new(),
            status: Status::Ok,
            start: ts(base + 5),
            end: Some(ts(base + 6)),
            error: String::new(),
            llm: None,
            tool: Some(ToolCall {
                name: "get_ticket".to_string(),
                call_id: "call-1".to_string(),
                ..ToolCall::default()
            }),
        },
    ]
}

fn step_with_msg(id: &str, run_id: &str, out: &str, sec: i64) -> Vec<Step> {
    let base = ts(sec);
    vec![
        Step {
            id: format!("{run_id}-root"),
            project: "default".to_string(),
            run_id: run_id.to_string(),
            parent_id: String::new(),
            kind: StepKind::Agent,
            name: "invoke_agent a".to_string(),
            service: String::new(),
            agent_name: String::new(),
            status: Status::Ok,
            start: base,
            end: Some(base + Duration::seconds(1)),
            error: String::new(),
            llm: None,
            tool: None,
        },
        Step {
            id: id.to_string(),
            project: "default".to_string(),
            run_id: run_id.to_string(),
            parent_id: format!("{run_id}-root"),
            kind: StepKind::Llm,
            name: "chat m".to_string(),
            service: String::new(),
            agent_name: String::new(),
            status: Status::Ok,
            start: base,
            end: Some(base + Duration::seconds(1)),
            error: String::new(),
            llm: Some(LlmCall {
                request_model: "m".to_string(),
                output_messages: vec![Message {
                    role: "assistant".to_string(),
                    content: out.to_string(),
                }],
                ..LlmCall::default()
            }),
            tool: None,
        },
    ]
}

fn fts_row_count(st: &Store) -> i64 {
    st.with(|c| c.query_row("SELECT count(*) FROM steps_fts", [], |r| r.get(0)))
        .unwrap()
}

// ---------------------------------------------------------------------------
// Open / migrations
// ---------------------------------------------------------------------------

#[test]
fn migrations_apply_and_are_idempotent() {
    let st = TestStore::open();
    let applied: i64 = st
        .with(|c| c.query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(applied as usize, MIGRATIONS.len());

    let created_ns: i64 = st
        .with(|c| {
            c.query_row("SELECT created_ns FROM instance WHERE id = 1", [], |r| {
                r.get(0)
            })
        })
        .unwrap();
    assert!(created_ns > 0, "instance creation time is recorded");
    drop(st);

    let path = TempDir::new();
    let db = path.join("test.db");
    let db = db.to_str().unwrap();
    let first = Store::open(db).unwrap();
    drop(first);
    let second = Store::open(db).unwrap();
    let applied: i64 = second
        .with(|c| c.query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(applied as usize, MIGRATIONS.len());
}

// ---------------------------------------------------------------------------
// Schema standards (ADR-0008)
// ---------------------------------------------------------------------------

fn fts_match_count(st: &Store, term: &str) -> i64 {
    st.with(|c| {
        c.query_row(
            "SELECT count(*) FROM steps_fts WHERE steps_fts MATCH ?1",
            rusqlite::params![term],
            |r| r.get(0),
        )
    })
    .unwrap()
}

fn table_definition(st: &Store, name: &str) -> String {
    st.with(|c| {
        c.query_row(
            "SELECT coalesce(sql, '') FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [name],
            |r| r.get(0),
        )
    })
    .unwrap()
}

#[test]
fn baseline_is_one_migration_and_every_ordinary_table_is_strict() {
    assert_eq!(
        MIGRATIONS.len(),
        1,
        "the baseline replaced the 21 old files"
    );
    assert_eq!(MIGRATIONS[0].0, "0001_initial.sql");

    let st = TestStore::open();
    let tables: Vec<String> = st
        .with(|c| {
            let mut stmt = c.prepare(
                "SELECT name FROM sqlite_schema
                  WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                  ORDER BY name",
            )?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .unwrap();

    let ordinary: Vec<&String> = tables
        .iter()
        .filter(|name| !name.starts_with("steps_fts"))
        .collect();
    assert!(
        ordinary.len() >= 12,
        "expected the full baseline, got {ordinary:?}"
    );
    for name in ordinary {
        let sql = table_definition(&st, name);
        assert!(sql.contains("STRICT"), "{name} is not STRICT:\n{sql}");
    }
    // The FTS5 virtual table and its shadow tables cannot be STRICT; they are
    // the only exceptions and are never queried through the store API.
    assert!(tables.iter().any(|name| name == "steps_fts"));
    for name in &tables {
        if name.starts_with("steps_fts") {
            assert!(
                name == "steps_fts" || name.starts_with("steps_fts_"),
                "unexpected shadow table {name}"
            );
        }
    }
}

#[test]
fn strict_rejects_a_wrong_column_type() {
    let st = TestStore::open();
    let err = st
        .with(|c| {
            c.execute(
                "INSERT INTO runs (project, id, status, start_ns)
                 VALUES ('default', 'bad', 'ok', 'not-an-integer')",
                [],
            )
        })
        .unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("INTEGER column") || message.contains("cannot store TEXT"),
        "{message}"
    );
}

#[test]
fn check_constraints_reject_invalid_enums() {
    let st = TestStore::open();
    let long_hash = "a".repeat(64);
    let cases = [
        (
            "INSERT INTO runs (project, id, status, start_ns) VALUES ('default', 'x', 'bogus', 1)"
                .to_string(),
            "run status",
        ),
        (
            "INSERT INTO steps (project, id, run_id, kind, name, status, start_ns)
             VALUES ('default', 'x', 'no-run', 'bogus', 'n', 'ok', 1)"
                .to_string(),
            "step kind",
        ),
        (
            format!(
                "INSERT INTO tokens (id, token_hash, name, scope, created_ns)
                 VALUES ('t', '{long_hash}', 'n', 'root', 1)"
            ),
            "token scope",
        ),
        (
            "INSERT INTO alerts (project, name, type, threshold, webhook_url, created_ns)
             VALUES ('default', 'a', 'bogus', 1.0, 'https://example.com/h', 1)"
                .to_string(),
            "alert type",
        ),
        (
            "INSERT INTO assertions (project, name, type, created_ns)
             VALUES ('default', 'a', 'bogus', 1)"
                .to_string(),
            "assertion type",
        ),
        (
            "INSERT INTO audit_log (at_ns, action, entity) VALUES (1, 'update', 'project')"
                .to_string(),
            "audit action",
        ),
        (
            "INSERT INTO saved_views (name, params_json, created_ns) VALUES ('v', '{', 1)"
                .to_string(),
            "view params JSON",
        ),
        (
            "INSERT INTO runs (project, id, status, start_ns, cost_partial)
             VALUES ('default', 'x', 'ok', 1, 2)"
                .to_string(),
            "boolean",
        ),
    ];
    for (sql, label) in cases {
        let err = st.with(|c| c.execute(&sql, [])).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("CHECK constraint failed"),
            "{label} was not rejected by a CHECK: {message}"
        );
    }
}

#[test]
fn foreign_keys_are_enabled_and_cascade() {
    let st = TestStore::open();
    let on: i64 = st
        .with(|c| c.query_row("PRAGMA foreign_keys", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(on, 1);

    // Deleting a run removes its steps, verdicts and share links. The
    // assertion itself is independent and survives.
    st.upsert_steps(&sample_run("r1", 1000)).unwrap();
    let assertion = st
        .create_assertion(Assertion {
            name: "cascades".into(),
            r#type: "is_json".into(),
            enabled: true,
            ..Assertion::default()
        })
        .unwrap();
    st.save_assertion_results(
        "default",
        "r1",
        &[EvalResult {
            assertion_id: assertion.id,
            pass: true,
            ..EvalResult::default()
        }],
    )
    .unwrap();
    st.create_share("default", "r1").unwrap();
    st.with(|c| {
        c.execute(
            "DELETE FROM runs WHERE project = 'default' AND id = 'r1'",
            [],
        )
    })
    .unwrap();
    for (table, column, value) in [
        ("steps", "run_id", "r1"),
        ("assertion_results", "run_id", "r1"),
        ("shares", "run_id", "r1"),
    ] {
        let left: i64 = st
            .with(|c| {
                c.query_row(
                    &format!("SELECT count(*) FROM {table} WHERE {column} = ?1"),
                    [value],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(left, 0, "{table} did not cascade");
    }
    assert_eq!(st.list_assertions("default").unwrap().len(), 1);

    // Deleting a project cascades every project-scoped table.
    st.create_project("cascade").unwrap();
    let mut steps = sample_run("r-cascade", 2000);
    for step in &mut steps {
        step.project = "cascade".into();
    }
    st.upsert_steps(&steps).unwrap();
    st.create_assertion(Assertion {
        project: "cascade".into(),
        name: "a".into(),
        r#type: "is_json".into(),
        enabled: true,
        ..Assertion::default()
    })
    .unwrap();
    st.create_alert(Rule {
        project: "cascade".into(),
        name: "a".into(),
        r#type: "error_rate".into(),
        threshold: 0.5,
        window_secs: 3600,
        webhook_url: "https://example.com/hook".into(),
        enabled: true,
        ..Rule::default()
    })
    .unwrap();
    st.ingest_batch("cascade", b"raw", &[]).unwrap();
    st.with(|c| c.execute("DELETE FROM projects WHERE name = 'cascade'", []))
        .unwrap();
    for table in [
        "runs",
        "steps",
        "assertions",
        "alerts",
        "raw_batches",
        "assertion_results",
        "shares",
    ] {
        let left: i64 = st
            .with(|c| {
                c.query_row(
                    &format!("SELECT count(*) FROM {table} WHERE project = 'cascade'"),
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(left, 0, "{table} did not cascade from projects");
    }
}

#[test]
fn foreign_key_violations_are_rejected() {
    let st = TestStore::open();
    let err = st
        .with(|c| {
            c.execute(
                "INSERT INTO steps (project, id, run_id, kind, name, status, start_ns)
                 VALUES ('default', 'orphan', 'no-such-run', 'agent', 'n', 'ok', 1)",
                [],
            )
        })
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("foreign key"),
        "{err}"
    );

    let err = st
        .with(|c| {
            c.execute(
                "INSERT INTO shares (token, project, run_id, created_ns)
                 VALUES ('t', 'default', 'no-such-run', 1)",
                [],
            )
        })
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("foreign key"),
        "{err}"
    );
}

#[test]
fn typed_enum_reads_refuse_corrupt_rows() {
    let st = TestStore::open();
    // Bypass the CHECK constraints the way a corrupted file or a future
    // SQLite could: the read path must still reject the value, not coerce it.
    st.with(|c| {
        c.execute_batch(
            "PRAGMA ignore_check_constraints = ON;
             INSERT INTO runs (project, id, status, start_ns)
                 VALUES ('default', 'rotten-run', 'bogus', 1);
             INSERT INTO runs (project, id, status, start_ns)
                 VALUES ('default', 'rotten-step', 'ok', 1);
             INSERT INTO steps (project, id, run_id, kind, name, status, start_ns)
                 VALUES ('default', 'rotten', 'rotten-step', 'bogus', 'n', 'ok', 1);
             PRAGMA ignore_check_constraints = OFF;",
        )
    })
    .unwrap();

    let err = st.get_run("rotten-run").unwrap_err();
    assert!(format!("{err:#}").contains("invalid run status"), "{err:#}");
    let err = st.get_run("rotten-step").unwrap_err();
    assert!(format!("{err:#}").contains("invalid step kind"), "{err:#}");
}

#[test]
fn fts_triggers_stay_in_sync_on_insert_update_and_delete() {
    let st = TestStore::open();
    st.upsert_steps(&step_with_msg(
        "s1",
        "run-sync",
        "the original needle",
        1000,
    ))
    .unwrap();
    assert_eq!(fts_match_count(&st, "needle"), 1);
    assert_eq!(fts_row_count(&st), 2);

    // Re-delivering the step with different content is an UPDATE; the old
    // tokens must disappear and the new ones become searchable.
    st.upsert_steps(&step_with_msg(
        "s1",
        "run-sync",
        "a replacement token",
        1000,
    ))
    .unwrap();
    assert_eq!(fts_match_count(&st, "needle"), 0);
    assert_eq!(fts_match_count(&st, "replacement"), 1);

    // Deleting the run cascades to steps, and the delete trigger purges the
    // index with them.
    st.with(|c| {
        c.execute(
            "DELETE FROM runs WHERE project = 'default' AND id = 'run-sync'",
            [],
        )
    })
    .unwrap();
    assert_eq!(fts_row_count(&st), 0);
    assert_eq!(fts_match_count(&st, "replacement"), 0);
}

// ---------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------

#[test]
fn upsert_and_get_run() {
    let st = TestStore::open();
    st.upsert_steps(&sample_run("r1", 1000)).unwrap();

    let (run, steps) = st.get_run("r1").unwrap().expect("run r1");
    assert_eq!(run.status, Status::Ok);
    assert_eq!(run.service, "support-agent");
    assert_eq!(run.agent_name, "support");
    assert_eq!(run.input_tokens, 800);
    assert_eq!(run.output_tokens, 150);
    assert_eq!(run.llm_calls, 1);
    assert_eq!(run.tool_calls, 1);
    assert_eq!(run.start, ts(1000));
    assert_eq!(run.end, Some(ts(1010)));
    assert_eq!(steps.len(), 3);
    assert_eq!(
        steps[1].llm.as_ref().map(|l| l.request_model.as_str()),
        Some("claude-sonnet-5")
    );
    assert_eq!(
        steps[2].tool.as_ref().map(|t| t.name.as_str()),
        Some("get_ticket")
    );
}

#[test]
fn idempotent_redelivery() {
    let st = TestStore::open();
    let batch = sample_run("r1", 1000);
    for _ in 0..3 {
        st.upsert_steps(&batch).unwrap();
    }
    let (run, steps) = st.get_run("r1").unwrap().unwrap();
    assert_eq!(steps.len(), 3);
    assert_eq!(run.input_tokens, 800);
    assert_eq!(run.llm_calls, 1);
}

#[test]
fn running_until_root_arrives() {
    let st = TestStore::open();
    let batch = sample_run("r1", 1000);
    st.upsert_steps(&batch[1..]).unwrap();
    let (run, _) = st.get_run("r1").unwrap().unwrap();
    assert_eq!(run.status, Status::Running);

    st.upsert_steps(&batch[..1]).unwrap();
    let (run, _) = st.get_run("r1").unwrap().unwrap();
    assert_eq!(run.status, Status::Ok);
}

#[test]
fn error_status_wins() {
    let st = TestStore::open();
    let mut batch = sample_run("r1", 1000);
    batch[2].status = Status::Error;
    batch[2].error = "tool exploded".to_string();
    st.upsert_steps(&batch).unwrap();

    let (run, _) = st.get_run("r1").unwrap().unwrap();
    assert_eq!(run.status, Status::Error);
    assert_eq!(run.error, "tool exploded");
}

#[test]
fn list_runs_newest_first() {
    let st = TestStore::open();
    st.upsert_steps(&sample_run("old", 1000)).unwrap();
    st.upsert_steps(&sample_run("new", 2000)).unwrap();

    let runs = st.list_runs(&Filter::default(), 10, 0).unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].id, "new");
    assert_eq!(runs[1].id, "old");

    let page = st.list_runs(&Filter::default(), 1, 1).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].id, "old");

    assert_eq!(st.count_runs(&Filter::default()).unwrap(), 2);
    let filter = Filter {
        status: "ok".to_string(),
        ..Filter::default()
    };
    assert_eq!(st.count_runs(&filter).unwrap(), 2);
    let filter = Filter {
        status: "error".to_string(),
        ..Filter::default()
    };
    assert_eq!(st.count_runs(&filter).unwrap(), 0);
}

#[test]
fn get_run_missing_returns_none() {
    let st = TestStore::open();
    assert!(st.get_run("nope").unwrap().is_none());
    assert!(st.get_run_in_project("default", "nope").unwrap().is_none());
}

#[test]
fn list_runs_filters() {
    let st = TestStore::open();
    st.upsert_steps(&sample_run("r-ok", 1000)).unwrap();
    let mut err_run = sample_run("r-err", 2000);
    err_run[2].status = Status::Error;
    err_run[2].error = "boom".to_string();
    st.upsert_steps(&err_run).unwrap();

    let filter = Filter {
        status: "error".to_string(),
        ..Filter::default()
    };
    let runs = st.list_runs(&filter, 10, 0).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, "r-err");

    let filter = Filter {
        service: "support-agent".to_string(),
        ..Filter::default()
    };
    assert_eq!(st.list_runs(&filter, 10, 0).unwrap().len(), 2);

    let filter = Filter {
        model: "sonnet".to_string(),
        ..Filter::default()
    };
    assert_eq!(st.list_runs(&filter, 10, 0).unwrap().len(), 2);

    let filter = Filter {
        model: "son%net".to_string(),
        ..Filter::default()
    };
    assert_eq!(
        st.list_runs(&filter, 10, 0).unwrap().len(),
        0,
        "LIKE wildcards must be escaped"
    );

    let filter = Filter {
        since: Some(ts(1500)),
        ..Filter::default()
    };
    let runs = st.list_runs(&filter, 10, 0).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, "r-err");

    let filter = Filter {
        until: Some(ts(1500)),
        ..Filter::default()
    };
    let runs = st.list_runs(&filter, 10, 0).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, "r-ok");

    let filter = Filter {
        status: "ok".to_string(),
        model: "claude".to_string(),
        since: Some(ts(500)),
        ..Filter::default()
    };
    let runs = st.list_runs(&filter, 10, 0).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, "r-ok");
}

#[test]
fn prompt_aggregation_and_filter() {
    let st = TestStore::open();
    for (id, prompt, sec) in [("r-v2", "reply v2", 1000), ("r-v3", "reply v3", 2000)] {
        let mut steps = sample_run(id, sec);
        steps[1].llm.as_mut().unwrap().prompt = prompt.to_string();
        st.upsert_steps(&steps).unwrap();
    }

    let (run, _) = st.get_run("r-v2").unwrap().unwrap();
    assert_eq!(run.prompts, "reply v2");

    let filter = Filter {
        prompt: "v3".to_string(),
        ..Filter::default()
    };
    let runs = st.list_runs(&filter, 10, 0).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, "r-v3");

    let filter = Filter {
        prompt: "reply".to_string(),
        ..Filter::default()
    };
    assert_eq!(st.list_runs(&filter, 10, 0).unwrap().len(), 2);

    let filter = Filter {
        prompt: "v2".to_string(),
        ..Filter::default()
    };
    assert_eq!(st.get_stats(&filter).unwrap().runs, 1);
}

#[test]
fn audit_log() {
    let st = TestStore::open();
    st.create_project("prod").unwrap();
    let assertion = st
        .create_assertion(Assertion {
            name: "n".to_string(),
            r#type: "is_json".to_string(),
            enabled: true,
            ..Assertion::default()
        })
        .unwrap();
    assert!(st.delete_assertion(assertion.id).unwrap());

    let entries = st.list_audit(10);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].action, "delete");
    assert_eq!(entries[0].entity, "assertion");
    assert_eq!(entries[2].action, "create");
    assert_eq!(entries[2].entity, "project");
    assert_eq!(entries[2].detail, "prod");
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

#[test]
fn get_stats() {
    let st = TestStore::open();
    for (i, id) in ["a", "b"].iter().enumerate() {
        st.upsert_steps(&sample_run(id, 1000 + i as i64 * 100))
            .unwrap();
    }
    let mut bad = sample_run("c", 2000);
    bad[1].status = Status::Error;
    bad[1].error = "boom".to_string();
    st.upsert_steps(&bad).unwrap();

    let assertion = st
        .create_assertion(Assertion {
            name: "n".to_string(),
            r#type: "is_json".to_string(),
            enabled: true,
            ..Assertion::default()
        })
        .unwrap();
    st.save_assertion_results(
        "default",
        "a",
        &[EvalResult {
            assertion_id: assertion.id,
            pass: true,
            ..EvalResult::default()
        }],
    )
    .unwrap();
    st.save_assertion_results(
        "default",
        "b",
        &[EvalResult {
            assertion_id: assertion.id,
            pass: false,
            ..EvalResult::default()
        }],
    )
    .unwrap();

    let stats = st.get_stats(&Filter::default()).unwrap();
    assert_eq!(stats.runs, 3);
    assert_eq!(stats.errors, 1);
    assert_eq!(stats.avg_duration_ms, 10000.0);
    assert_eq!(stats.p50_duration_ms, 10000);
    assert_eq!(stats.p95_duration_ms, 10000);
    assert_eq!(stats.input_tokens, 2400);
    assert_eq!(stats.assertion_rates.len(), 1);
    assert_eq!(stats.assertion_rates[0].pass, 1);
    assert_eq!(stats.assertion_rates[0].fail, 1);

    let filter = Filter {
        status: "error".to_string(),
        ..Filter::default()
    };
    let stats = st.get_stats(&filter).unwrap();
    assert_eq!(stats.runs, 1);
    assert_eq!(stats.errors, 1);
    assert!(stats.assertion_rates.is_empty());

    let filter = Filter {
        service: "nothing".to_string(),
        ..Filter::default()
    };
    let stats = st.get_stats(&filter).unwrap();
    assert_eq!(stats.runs, 0);
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

#[test]
fn create_token_stores_only_digest() {
    let st = TestStore::open();
    let token = st.create_token("ci", Scope::Read).unwrap();
    assert_eq!(token.token.len(), 32);

    let (id, hash, scope): (String, String, String) = st
        .with(|c| {
            c.query_row(
                "SELECT id, token_hash, scope FROM tokens WHERE id = ?",
                [&token.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
        })
        .unwrap();
    assert_eq!(id, token.id);
    assert_eq!(scope, "read");
    assert_eq!(hash, super::tokens::token_hash(&token.token));
    assert_ne!(hash, token.token);

    let found: i64 = st
        .with(|c| {
            c.query_row(
                "SELECT count(*) FROM tokens WHERE id = ? OR token_hash = ? OR name = ?",
                [&token.token, &token.token, &token.token],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(found, 0);
}

#[test]
fn authenticate_token() {
    let st = TestStore::open();
    let read = st.create_read_token("script").unwrap();
    let admin = st.create_token("operator", Scope::Admin).unwrap();

    let got = st.authenticate_token(&read.token).unwrap().unwrap();
    assert_eq!(got.id, read.id);
    assert_eq!(got.name, "script");
    assert_eq!(got.scope, Scope::Read);

    let got = st.authenticate_token(&admin.token).unwrap().unwrap();
    assert_eq!(got.scope, Scope::Admin);

    assert!(st.authenticate_token("nope").unwrap().is_none());
    assert!(st.authenticate_token("").unwrap().is_none());
    assert!(
        st.authenticate_token(&format!("{}0", read.token))
            .unwrap()
            .is_none()
    );
}

#[test]
fn last_used_is_recorded() {
    let st = TestStore::open();
    let token = st.create_read_token("dashboard").unwrap();
    assert!(st.list_tokens().unwrap()[0].last_used_at.is_none());

    let before = Utc::now() - Duration::seconds(1);
    assert!(st.authenticate_token(&token.token).unwrap().is_some());

    let tokens = st.list_tokens().unwrap();
    assert_eq!(tokens.len(), 1);
    let last_used = tokens[0].last_used_at.expect("last used recorded");
    assert!(last_used >= before);
}

#[test]
fn delete_token_by_id() {
    let st = TestStore::open();
    let token = st.create_read_token("gone").unwrap();
    assert!(st.delete_token(&token.id).unwrap());
    assert!(st.authenticate_token(&token.token).unwrap().is_none());
    assert!(!st.delete_token(&token.id).unwrap());
}

/// The 21 migration names the pre-redesign build recorded. A database that
/// carries them is exactly what the new binary must refuse.
const LEGACY_MIGRATIONS: &[&str] = &[
    "0001_meta.sql",
    "0002_runs_steps.sql",
    "0003_raw_batches.sql",
    "0004_llm_token_details.sql",
    "0005_run_models.sql",
    "0006_step_detail.sql",
    "0007_cost.sql",
    "0008_service_idx.sql",
    "0009_projects.sql",
    "0010_assertions.sql",
    "0011_project_scoped_keys.sql",
    "0012_alerts.sql",
    "0013_fts.sql",
    "0014_shares.sql",
    "0015_saved_views.sql",
    "0016_prompt.sql",
    "0017_read_tokens.sql",
    "0018_audit.sql",
    "0019_assertion_results_project.sql",
    "0020_alert_settle.sql",
    "0021_token_scopes.sql",
];

fn write_legacy_database(path: &str) -> usize {
    let raw = rusqlite::Connection::open(path).unwrap();
    raw.execute_batch(
        "CREATE TABLE schema_migrations (
             name TEXT PRIMARY KEY,
             applied_at TEXT NOT NULL DEFAULT (datetime('now')));
         CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE runs (
             project TEXT NOT NULL DEFAULT 'default',
             id TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'running',
             start_ns INTEGER NOT NULL,
             PRIMARY KEY (project, id));
         INSERT INTO meta (key, value) VALUES ('created_at', '2026-01-01T00:00:00Z');
         INSERT INTO runs (project, id, status, start_ns)
             VALUES ('default', 'old-run', 'ok', 1000);",
    )
    .unwrap();
    for name in LEGACY_MIGRATIONS {
        raw.execute("INSERT INTO schema_migrations (name) VALUES (?)", [name])
            .unwrap();
    }
    LEGACY_MIGRATIONS.len()
}

#[test]
fn legacy_database_is_refused_with_a_clear_message() {
    let dir = TempDir::new();
    let path = dir.join("legacy.db");
    let path = path.to_str().unwrap().to_string();
    assert_eq!(write_legacy_database(&path), 21);

    let err = match Store::open(&path) {
        Ok(_) => panic!("a pre-redesign database must be refused"),
        Err(err) => err,
    };
    let message = format!("{err:#}");
    assert!(message.contains("new database"), "{message}");
    assert!(message.contains("different schema"), "{message}");
    assert!(message.contains("0001_meta.sql"), "{message}");
    assert!(message.contains("0008-sqlite-schema-redesign"), "{message}");
    assert_eq!(
        backup_files(&path).len(),
        0,
        "refusing a legacy database must not write a snapshot"
    );

    // Nothing was migrated: the old tables and rows are still the only ones.
    let raw = rusqlite::Connection::open(&path).unwrap();
    let migrations: i64 = raw
        .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(migrations, 21);
    let runs: i64 = raw
        .query_row("SELECT count(*) FROM runs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(runs, 1);
    for new_table in [
        "steps",
        "steps_fts",
        "assertion_results",
        "shares",
        "tokens",
    ] {
        let found: i64 = raw
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = ?",
                [new_table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(found, 0, "{new_table} must not be created by a refusal");
    }
}

#[test]
fn tables_without_migration_history_are_refused() {
    let dir = TempDir::new();
    let path = dir.join("foreign.db");
    let path = path.to_str().unwrap();
    {
        let raw = rusqlite::Connection::open(path).unwrap();
        raw.execute_batch("CREATE TABLE runs (id TEXT PRIMARY KEY) STRICT")
            .unwrap();
    }
    let err = match Store::open(path) {
        Ok(_) => panic!("an unowned database must be refused"),
        Err(err) => err,
    };
    let message = format!("{err:#}");
    assert!(message.contains("no migration history"), "{message}");
    assert!(message.contains("new database"), "{message}");
}

// ---------------------------------------------------------------------------
// Projects / sweep / isolation
// ---------------------------------------------------------------------------

#[test]
fn projects() {
    let st = TestStore::open();
    assert_eq!(st.project_for_key(""), Some("default".to_string()));

    let created = st.create_project("prod").unwrap();
    assert_eq!(created.ingest_key.len(), 32);
    assert_eq!(
        st.project_for_key(&created.ingest_key),
        Some("prod".to_string())
    );
    assert!(st.project_for_key("bogus").is_none());
    assert!(st.create_project("prod").is_err());

    assert_eq!(st.list_projects().unwrap().len(), 2);
}

#[test]
fn sweep_removes_old_runs() {
    let st = TestStore::open();
    st.upsert_steps(&sample_run("old", 1000)).unwrap();
    st.upsert_steps(&sample_run("new", 5000)).unwrap();

    assert_eq!(st.sweep(ts(3000)).unwrap(), 1);
    let runs = st.list_runs(&Filter::default(), 10, 0).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, "new");

    let steps: i64 = st
        .with(|c| {
            c.query_row("SELECT count(*) FROM steps WHERE run_id = 'old'", [], |r| {
                r.get(0)
            })
        })
        .unwrap();
    assert_eq!(steps, 0);
}

#[test]
fn cross_project_isolation() {
    let st = TestStore::open();
    let mut alpha = sample_run("shared-trace", 1000);
    for step in &mut alpha {
        step.project = "alpha".to_string();
    }
    st.upsert_steps(&alpha).unwrap();

    let mut beta = sample_run("shared-trace", 5000);
    for step in &mut beta {
        step.project = "beta".to_string();
        if let Some(llm) = &mut step.llm {
            llm.input_tokens = 999;
        }
    }
    st.upsert_steps(&beta).unwrap();

    let alpha_runs = st
        .list_runs(
            &Filter {
                project: "alpha".to_string(),
                ..Filter::default()
            },
            10,
            0,
        )
        .unwrap();
    let beta_runs = st
        .list_runs(
            &Filter {
                project: "beta".to_string(),
                ..Filter::default()
            },
            10,
            0,
        )
        .unwrap();
    assert_eq!(alpha_runs.len(), 1);
    assert_eq!(beta_runs.len(), 1);
    assert_eq!(alpha_runs[0].input_tokens, 800);
    assert_eq!(beta_runs[0].input_tokens, 999);

    let (_, steps) = st.get_run("shared-trace").unwrap().unwrap();
    assert_eq!(steps.len(), 3);
    let (_, alpha_steps) = st
        .get_run_in_project("alpha", "shared-trace")
        .unwrap()
        .unwrap();
    assert!(alpha_steps.iter().all(|s| s.project == "alpha"));
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn assert_mode(path: &Path, want: u32) {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, want, "{} mode = {mode:04o}", path.display());
}

#[cfg(unix)]
#[test]
fn open_creates_private_files() {
    let dir = TempDir::new();
    let data = dir.path().join("data");
    let path = data.join("otter.db");
    let store = Store::open(path.to_str().unwrap()).unwrap();
    store.upsert_steps(&sample_run("r1", 1000)).unwrap();

    assert_mode(&data, 0o700);
    assert_mode(&path, 0o600);
    for suffix in ["-wal", "-shm"] {
        assert_mode(&PathBuf::from(format!("{}{suffix}", path.display())), 0o600);
    }
}

#[cfg(unix)]
#[test]
fn open_tightens_existing_wide_files() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new();
    let data = dir.path().join("data");
    let path = data.join("otter.db");
    let path_str = path.to_str().unwrap().to_string();

    {
        let store = Store::open(&path_str).unwrap();
        store.upsert_steps(&sample_run("r1", 1000)).unwrap();
    }
    for p in [
        path.clone(),
        PathBuf::from(format!("{path_str}-wal")),
        PathBuf::from(format!("{path_str}-shm")),
    ] {
        if p.exists() {
            fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        }
    }
    fs::set_permissions(&data, fs::Permissions::from_mode(0o755)).unwrap();

    let store = Store::open(&path_str).unwrap();
    store.upsert_steps(&sample_run("r2", 2000)).unwrap();

    assert_mode(&data, 0o700);
    assert_mode(&path, 0o600);
    for suffix in ["-wal", "-shm"] {
        assert_mode(&PathBuf::from(format!("{path_str}{suffix}")), 0o600);
    }
}

#[cfg(unix)]
#[test]
fn tighten_dir_skips_shared_locations() {
    use std::os::unix::fs::PermissionsExt;

    for dir in [std::env::temp_dir(), PathBuf::from("/")] {
        let before = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        super::backup::tighten_dir(&dir).unwrap();
        let after = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(after, before);
    }

    let own = TempDir::new();
    fs::set_permissions(own.path(), fs::Permissions::from_mode(0o755)).unwrap();
    super::backup::tighten_dir(own.path()).unwrap();
    assert_mode(own.path(), 0o700);
}

// ---------------------------------------------------------------------------
// Full-text search
// ---------------------------------------------------------------------------

#[test]
fn full_text_search() {
    let st = TestStore::open();
    st.upsert_steps(&step_with_msg(
        "s1",
        "run-ship",
        "Your order A-1042 has shipped today",
        1000,
    ))
    .unwrap();
    st.upsert_steps(&step_with_msg(
        "s2",
        "run-refund",
        "I have processed your refund",
        2000,
    ))
    .unwrap();

    let query = |q: &str| Filter {
        query: q.to_string(),
        ..Filter::default()
    };
    let runs = st.list_runs(&query("shipped"), 10, 0).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, "run-ship");

    let runs = st.list_runs(&query("refund"), 10, 0).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, "run-refund");

    assert!(st.list_runs(&query("A-1042"), 10, 0).is_ok());
    assert_eq!(st.list_runs(&query("A-1042"), 10, 0).unwrap().len(), 1);
    assert!(st.list_runs(&query("say \"hi\""), 10, 0).is_ok());

    st.upsert_steps(&step_with_msg(
        "s1",
        "run-ship",
        "Your order A-1042 has shipped today",
        1000,
    ))
    .unwrap();
    assert_eq!(st.list_runs(&query("shipped"), 10, 0).unwrap().len(), 1);

    assert_eq!(st.get_stats(&query("refund")).unwrap().runs, 1);
}

#[test]
fn sweep_purges_full_text() {
    let st = TestStore::open();
    st.upsert_steps(&step_with_msg(
        "s1",
        "run-old",
        "Your order A-1042 has shipped today",
        1000,
    ))
    .unwrap();
    st.upsert_steps(&step_with_msg(
        "s2",
        "run-new",
        "I have processed your refund",
        5000,
    ))
    .unwrap();
    assert_eq!(fts_row_count(&st), 4);

    assert_eq!(st.sweep(ts(3000)).unwrap(), 1);
    assert_eq!(fts_row_count(&st), 2);
    let query = |q: &str| Filter {
        query: q.to_string(),
        ..Filter::default()
    };
    assert_eq!(st.list_runs(&query("shipped"), 10, 0).unwrap().len(), 0);
    assert_eq!(st.list_runs(&query("refund"), 10, 0).unwrap().len(), 1);

    assert_eq!(st.sweep(ts(9000)).unwrap(), 1);
    assert_eq!(fts_row_count(&st), 0);
    for q in ["shipped", "refund", "A-1042"] {
        assert_eq!(st.list_runs(&query(q), 10, 0).unwrap().len(), 0);
    }
    assert_eq!(st.sweep(ts(9000)).unwrap(), 0);
}

#[test]
fn sweep_purges_orphaned_full_text() {
    let st = TestStore::open();
    st.upsert_steps(&step_with_msg(
        "s1",
        "run-old",
        "Your order A-1042 has shipped today",
        1000,
    ))
    .unwrap();
    st.with(|c| {
        c.execute_batch(
            "DELETE FROM steps WHERE run_id = 'run-old';
             DELETE FROM runs WHERE id = 'run-old'",
        )
    })
    .unwrap();
    // The delete triggers purge the FTS rows with the steps; a standalone FTS
    // table used to need a sweep-time cleanup pass for this.
    assert_eq!(fts_row_count(&st), 0);

    // A hand-written orphan (the only way one can now exist) is removed by the
    // rebuild the sweep runs after VACUUM. `count(*)` on an external-content
    // table counts content rows, so MATCH is what proves the orphan is there.
    st.with(|c| {
        c.execute(
            "INSERT INTO steps_fts (rowid, name) VALUES (999, 'orphaned-message')",
            [],
        )
    })
    .unwrap();
    assert_eq!(fts_match_count(&st, "orphaned"), 1);
    st.upsert_steps(&step_with_msg(
        "s1",
        "run-old",
        "Your order A-1042 has shipped today",
        1000,
    ))
    .unwrap();

    st.sweep(ts(9000)).unwrap();
    assert_eq!(fts_row_count(&st), 0);
    assert_eq!(fts_match_count(&st, "orphaned"), 0);
    assert_eq!(fts_match_count(&st, "shipped"), 0);
    let filter = Filter {
        query: "shipped".to_string(),
        ..Filter::default()
    };
    assert_eq!(st.list_runs(&filter, 10, 0).unwrap().len(), 0);
}

#[test]
fn sweep_reclaims_file_space() {
    let st = TestStore::open();
    let marker = "A-1042-ZQX";
    st.upsert_steps(&step_with_msg(
        "s1",
        "run-old",
        &format!("Your order {marker} has shipped today"),
        1000,
    ))
    .unwrap();
    assert!(
        file_contains(marker, &[&st.path, &format!("{}-wal", st.path)]),
        "setup: marker not on disk"
    );

    st.sweep(ts(9000)).unwrap();
    assert!(
        !file_contains(marker, &[&st.path, &format!("{}-wal", st.path)]),
        "swept message text still present in the database files"
    );
}

fn file_contains(needle: &str, paths: &[&str]) -> bool {
    for path in paths {
        match fs::read(path) {
            Ok(bytes) => {
                if bytes.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                    return true;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => panic!("read {path}: {err}"),
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

fn stat_size(path: &Path) -> i64 {
    fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0)
}

#[test]
fn metrics_includes_wal() {
    let st = TestStore::open();
    st.with(|c| c.execute_batch("PRAGMA wal_autocheckpoint = 0"))
        .unwrap();

    let out = "tool output for ticket A-1042 ".repeat(200);
    for i in 0..40 {
        let run_id = format!("run-{i}");
        let base = 1000 + i;
        let steps = vec![
            Step {
                id: format!("{run_id}-root"),
                project: "default".to_string(),
                run_id: run_id.clone(),
                parent_id: String::new(),
                kind: StepKind::Agent,
                name: "invoke_agent a".to_string(),
                service: String::new(),
                agent_name: String::new(),
                status: Status::Ok,
                start: ts(base),
                end: Some(ts(base + 1)),
                error: String::new(),
                llm: None,
                tool: None,
            },
            Step {
                id: format!("{run_id}-llm"),
                project: "default".to_string(),
                run_id: run_id.clone(),
                parent_id: format!("{run_id}-root"),
                kind: StepKind::Llm,
                name: "chat m".to_string(),
                service: String::new(),
                agent_name: String::new(),
                status: Status::Ok,
                start: ts(base),
                end: Some(ts(base + 1)),
                error: String::new(),
                llm: Some(LlmCall {
                    request_model: "m".to_string(),
                    output_messages: vec![Message {
                        role: "assistant".to_string(),
                        content: out.clone(),
                    }],
                    ..LlmCall::default()
                }),
                tool: None,
            },
        ];
        st.upsert_steps(&steps).unwrap();
    }

    let path = Path::new(&st.path);
    let main_size = stat_size(path);
    let wal_size = stat_size(&PathBuf::from(format!("{}-wal", st.path)));
    let shm_size = stat_size(&PathBuf::from(format!("{}-shm", st.path)));
    assert!(wal_size > 0, "expected a non-empty WAL");
    assert!(
        wal_size > main_size,
        "WAL {wal_size} B vs main {main_size} B"
    );

    let metrics = st.metrics().unwrap();
    assert_eq!(metrics.db_size_bytes, main_size + wal_size + shm_size);
    assert!(metrics.db_size_bytes > main_size);
    assert_eq!(metrics.runs, 40);
    assert_eq!(metrics.runs_ok, 40);
    assert_eq!(metrics.steps, 80);
    assert_eq!(metrics.projects, 1);
}

#[test]
fn db_size_in_memory_is_zero() {
    for path in ["", ":memory:"] {
        assert_eq!(super::metrics::db_size_bytes(path), 0);
    }
    let store = Store::open(":memory:").unwrap();
    assert_eq!(store.metrics().unwrap().db_size_bytes, 0);
}

// ---------------------------------------------------------------------------
// Backup
// ---------------------------------------------------------------------------

#[test]
fn backup_produces_valid_copy() {
    let st = TestStore::open();
    st.upsert_steps(&sample_run("r1", 1000)).unwrap();
    let out = TempDir::new();
    let dest = out.join("backup.db");
    let dest = dest.to_str().unwrap();
    st.backup(dest).unwrap();
    drop(st);

    let restored = Store::open(dest).unwrap();
    let runs = restored.list_runs(&Filter::default(), 10, 0).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, "r1");
}

#[test]
fn unknown_migration_history_is_refused_without_a_snapshot() {
    let dir = TempDir::new();
    let path = dir.join("up.db");
    let path = path.to_str().unwrap().to_string();

    let st = Store::open(&path).unwrap();
    st.upsert_steps(&sample_run("r1", 1000)).unwrap();
    drop(st);

    // A database from a build with a migration this one does not know: newer
    // schema, or a foreign one. Refuse before writing anything, including a
    // pre-migration snapshot (a refusal is not an upgrade).
    {
        let raw = rusqlite::Connection::open(&path).unwrap();
        raw.execute(
            "INSERT INTO schema_migrations (name, applied_ns) VALUES ('9999_future.sql', 0)",
            [],
        )
        .unwrap();
    }

    let err = match Store::open(&path) {
        Ok(_) => panic!("a database with an unknown migration must be refused"),
        Err(err) => err,
    };
    let message = format!("{err:#}");
    assert!(message.contains("new database"), "{message}");
    assert!(message.contains("9999_future.sql"), "{message}");
    assert_eq!(backup_files(&path).len(), 0, "refusal must not snapshot");

    // The refused database is untouched: the unknown marker and the user's
    // data are still there, and no other migration ran.
    let raw = rusqlite::Connection::open(&path).unwrap();
    let runs: i64 = raw
        .query_row("SELECT count(*) FROM runs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(runs, 1);
    let fake: i64 = raw
        .query_row(
            "SELECT count(*) FROM schema_migrations WHERE name = '9999_future.sql'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fake, 1);
    let applied: i64 = raw
        .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(applied, 2, "refusal must not record a new migration");
}

#[test]
fn no_backup_on_fresh_init() {
    let dir = TempDir::new();
    let path = dir.join("fresh.db");
    let path = path.to_str().unwrap();
    let st = Store::open(path).unwrap();
    drop(st);
    assert_eq!(backup_files(path).len(), 0);
}

#[cfg(unix)]
#[test]
fn backup_file_is_private() {
    let st = TestStore::open();
    let dest = st.dir.join("snapshot.db");
    st.backup(dest.to_str().unwrap()).unwrap();
    assert_mode(&dest, 0o600);
}

#[test]
fn pre_migration_backups_prune_to_newest() {
    let st = TestStore::open();
    st.upsert_steps(&sample_run("r1", 1000)).unwrap();

    let mut newest = String::new();
    for _ in 0..3 {
        let before = backup_files(&st.path);
        st.backup_before_migrate().unwrap();
        let after = backup_files(&st.path);
        newest = new_backup(&before, &after);
        assert!(!newest.is_empty(), "backup created no new snapshot");
        assert_eq!(after.len(), 1, "want exactly one backup: {after:?}");
        // Nanosecond-stamped names make each snapshot distinct even when the
        // filesystem's mtime resolution is coarse.
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    let backups = backup_files(&st.path);
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0], newest);
    #[cfg(unix)]
    assert_mode(Path::new(&backups[0]), 0o600);

    let raw = rusqlite::Connection::open(&backups[0]).unwrap();
    let runs: i64 = raw
        .query_row("SELECT count(*) FROM runs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(runs, 1);
}

fn backup_files(db_path: &str) -> Vec<String> {
    let db = Path::new(db_path);
    let dir = db.parent().unwrap_or(Path::new("."));
    let name = db.file_name().unwrap().to_string_lossy();
    let prefix = format!("{name}.pre-migrate-");
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let file_name = entry.file_name().to_string_lossy().to_string();
        if file_name.starts_with(&prefix) && file_name.ends_with(".bak") {
            out.push(entry.path().to_string_lossy().to_string());
        }
    }
    out.sort();
    out
}

fn new_backup(before: &[String], after: &[String]) -> String {
    after
        .iter()
        .find(|path| !before.contains(path))
        .cloned()
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Alerts / assertions / shares / views / raw ingest
// ---------------------------------------------------------------------------

#[test]
fn alerts_round_trip() {
    let st = TestStore::open();
    let created = st
        .create_alert(Rule {
            name: "errors".to_string(),
            r#type: "error_rate".to_string(),
            threshold: 0.5,
            window_secs: 3600,
            webhook_url: "https://example.com/hook".to_string(),
            enabled: true,
            ..Rule::default()
        })
        .unwrap();
    assert!(created.id > 0);
    assert_eq!(created.project, "default");

    assert_eq!(st.list_alerts("").unwrap().len(), 1);
    assert_eq!(st.list_alerts("default").unwrap().len(), 1);
    assert_eq!(st.list_alerts("other").unwrap().len(), 0);
    assert_eq!(st.list_enabled_alerts().unwrap().len(), 1);

    st.set_alert_firing(created.id, true).unwrap();
    let alert = &st.list_alerts("").unwrap()[0];
    assert!(alert.firing);
    assert_eq!(alert.pending_since_ns, 0);

    st.set_alert_pending(created.id, 12345).unwrap();
    let alert = &st.list_alerts("").unwrap()[0];
    assert_eq!(alert.pending_since_ns, 12345);

    st.set_alert_firing(created.id, false).unwrap();
    let alert = &st.list_alerts("").unwrap()[0];
    assert!(!alert.firing);
    assert_eq!(alert.pending_since_ns, 0);

    assert!(st.delete_alert(created.id).unwrap());
    assert!(!st.delete_alert(created.id).unwrap());
}

#[test]
fn assertions_round_trip() {
    let st = TestStore::open();
    let assertion = st
        .create_assertion(Assertion {
            name: "json".to_string(),
            r#type: "is_json".to_string(),
            config: "{}".to_string(),
            enabled: true,
            ..Assertion::default()
        })
        .unwrap();
    assert!(assertion.id > 0);
    assert_eq!(assertion.project, "default");
    assert_eq!(st.list_assertions("").unwrap().len(), 1);
    assert_eq!(st.list_assertions("default").unwrap().len(), 1);
    assert_eq!(st.list_assertions("other").unwrap().len(), 0);

    st.upsert_steps(&sample_run("run-1", 1000)).unwrap();
    st.save_assertion_results(
        "default",
        "run-1",
        &[EvalResult {
            assertion_id: assertion.id,
            pass: true,
            detail: "ok".to_string(),
            ..EvalResult::default()
        }],
    )
    .unwrap();
    let results = st.results_for_run("default", "run-1").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "json");
    assert!(results[0].pass);
    assert!(st.results_for_run("other", "run-1").unwrap().is_empty());

    assert!(st.delete_assertion(assertion.id).unwrap());
    assert!(st.results_for_run("default", "run-1").unwrap().is_empty());
    assert!(!st.delete_assertion(assertion.id).unwrap());
}

#[test]
fn shares_round_trip() {
    let st = TestStore::open();
    st.upsert_steps(&sample_run("run-1", 1000)).unwrap();
    let token = st.create_share("default", "run-1").unwrap();
    assert_eq!(token.len(), 32);
    assert_eq!(
        st.resolve_share(&token).unwrap(),
        Some(("default".to_string(), "run-1".to_string()))
    );
    assert_eq!(st.shares_for_run("default", "run-1").unwrap().len(), 1);
    assert!(st.delete_share(&token).unwrap());
    assert!(st.resolve_share(&token).unwrap().is_none());
    assert!(!st.delete_share(&token).unwrap());
}

#[test]
fn saved_views_round_trip() {
    let st = TestStore::open();
    let params = serde_json::json!({"status": "error"});
    let view = st.create_saved_view("errors", &params).unwrap();
    assert!(view.id > 0);
    assert_eq!(view.name, "errors");
    assert_eq!(view.params, params);

    let views = st.list_saved_views().unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].params, params);
    assert!(st.delete_saved_view(view.id).unwrap());
    assert!(!st.delete_saved_view(view.id).unwrap());
}

#[test]
fn raw_ingest_batches_round_trip() {
    let st = TestStore::open();
    let steps = sample_run("r1", 1000);
    assert_eq!(st.ingest_batch("default", b"raw-1", &steps).unwrap(), 0);

    let mut seen = Vec::new();
    st.each_raw_batch(|project, payload| {
        seen.push((project.to_string(), payload.to_vec()));
        Ok(())
    })
    .unwrap();
    assert_eq!(seen, vec![("default".to_string(), b"raw-1".to_vec())]);

    // Re-delivery is idempotent for steps but still records the raw batch.
    assert_eq!(st.ingest_batch("default", b"raw-2", &steps).unwrap(), 0);

    // A conflicting service on every span is refused, and keeps no raw copy.
    let mut forged = sample_run("r1", 1000);
    for step in &mut forged {
        step.service = "other".to_string();
    }
    assert_eq!(
        st.ingest_batch("default", b"raw-3", &forged).unwrap(),
        forged.len()
    );
    let mut count = 0;
    st.each_raw_batch(|_, _| {
        count += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn duplicate_writes_are_conflicts() {
    let st = TestStore::open();
    let assertion = Assertion {
        name: "dup".to_string(),
        r#type: "is_json".to_string(),
        enabled: true,
        ..Assertion::default()
    };
    st.create_assertion(assertion.clone()).unwrap();
    let err = st.create_assertion(assertion).unwrap_err();
    assert_eq!(err.downcast::<Error>().unwrap(), Error::Conflict);

    let params = serde_json::json!({});
    st.create_saved_view("dup", &params).unwrap();
    let err = st.create_saved_view("dup", &params).unwrap_err();
    assert_eq!(err.downcast::<Error>().unwrap(), Error::Conflict);

    let rule = Rule {
        name: "dup".to_string(),
        r#type: "error_rate".to_string(),
        threshold: 0.5,
        window_secs: 3600,
        webhook_url: "https://example.com/hook".to_string(),
        enabled: true,
        ..Rule::default()
    };
    st.create_alert(rule.clone()).unwrap();
    let err = st.create_alert(rule).unwrap_err();
    assert_eq!(err.downcast::<Error>().unwrap(), Error::Conflict);
}

#[test]
fn raw_batch_callback_may_write() {
    let st = TestStore::open();
    st.ingest_batch("default", b"raw-1", &[]).unwrap();

    let mut pages = 0;
    st.each_raw_batch(|_, _| {
        pages += 1;
        // The store's single connection must be free here: a cursor held open
        // across the callback would deadlock this write.
        st.upsert_steps(&sample_run("r1", 1000))?;
        Ok(())
    })
    .unwrap();

    assert_eq!(pages, 1);
    assert_eq!(st.list_runs(&Filter::default(), 10, 0).unwrap().len(), 1);
}
