//! End-to-end tests for the database subcommands: they run the built binary
//! against throwaway SQLite files and inspect the store the command produced.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use flate2::Compression;
use flate2::write::GzEncoder;
use otterscope::store::{Filter, Scope, Store};

fn temp_dir(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "otterscope-cli-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn temp_db(name: &str) -> (PathBuf, String) {
    let dir = temp_dir(name);
    let db = dir.join("test.db").to_string_lossy().into_owned();
    (dir, db)
}

fn run(args: &[&str]) -> Output {
    run_with_env(args, &[])
}

fn run_with_env(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_otterscope"));
    command.env_clear().args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run otterscope")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        stdout(output),
        stderr(output)
    );
}

fn value_of<'a>(text: &'a str, prefix: &str) -> &'a str {
    text.lines()
        .find_map(|line| line.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("no line starting with {prefix:?} in:\n{text}"))
        .trim()
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("utf-8 path")
}

fn token_count(db: &str) -> usize {
    Store::open(db)
        .expect("open store")
        .list_tokens()
        .expect("tokens")
        .len()
}

#[test]
fn token_add_mints_a_working_token_and_list_never_shows_it() {
    let (_dir, db) = temp_db("token");
    let admin = run(&["token", "add", "ci-runner", "-admin", "-db", &db]);
    assert_success(&admin);
    let text = stdout(&admin);
    assert!(
        text.contains("admin token \"ci-runner\" created (id tok_"),
        "{text}"
    );
    assert!(text.contains("The token is not stored; this is the only time it is shown."));
    let admin_secret = value_of(&text, "token: ").to_string();

    let read = run(&["token", "add", "dashboard", "-db", &db]);
    assert_success(&read);
    let read_secret = value_of(&stdout(&read), "token: ").to_string();

    let store = Store::open(&db).expect("open store");
    let admin_token = store
        .authenticate_token(&admin_secret)
        .expect("authenticate")
        .expect("admin token authenticates");
    assert_eq!(admin_token.name, "ci-runner");
    assert_eq!(admin_token.scope, Scope::Admin);
    assert!(admin_token.last_used_at.is_some(), "last use is recorded");

    let read_token = store
        .authenticate_token(&read_secret)
        .expect("authenticate")
        .expect("read token authenticates");
    assert_eq!(read_token.name, "dashboard");
    assert_eq!(read_token.scope, Scope::Read);
    let admin_id = admin_token.id.clone();
    let read_id = read_token.id.clone();
    drop(store);

    let list = run(&["token", "list", "-db", &db]);
    assert_success(&list);
    let listed = stdout(&list);
    assert!(
        !listed.contains(&admin_secret),
        "list leaked a secret:\n{listed}"
    );
    assert!(
        !listed.contains(&read_secret),
        "list leaked a secret:\n{listed}"
    );
    assert!(listed.contains(&admin_id));
    assert!(listed.contains(&read_id));
    assert!(listed.contains("ci-runner"));
    assert!(listed.contains("dashboard"));
    assert!(listed.contains("admin"));
    assert!(listed.contains("read"));
    assert!(listed.contains("last used"));
}

#[test]
fn project_add_prints_a_key_that_resolves() {
    let (_dir, db) = temp_db("project");
    let added = run(&["project", "add", "checkout", "-db", &db]);
    assert_success(&added);
    let text = stdout(&added);
    assert!(text.contains("project \"checkout\" created"), "{text}");
    assert!(
        text.contains("export OTEL_EXPORTER_OTLP_HEADERS=\"Authorization=Bearer "),
        "{text}"
    );
    let key = value_of(&text, "ingest key: ").to_string();

    let store = Store::open(&db).expect("open store");
    assert_eq!(store.project_for_key(&key).as_deref(), Some("checkout"));
    drop(store);

    let list = run(&["project", "list", "-db", &db]);
    assert_success(&list);
    let listed = stdout(&list);
    assert!(listed.contains("checkout"), "{listed}");
    assert!(listed.contains(&key), "{listed}");
    // `project` cannot see how `serve` was started, so it must not promise that
    // keyless ingest is open; only OTTERSCOPE_READ_AUTH in its own environment
    // lets it report the explicit disabled wording.
    assert!(listed.contains("default"), "{listed}");
    assert!(
        listed.contains(
            "(no ingest key — keyless ingest depends on the server's -read-auth setting)"
        ),
        "{listed}"
    );
    assert!(!listed.contains("open ingest)"), "{listed}");
}

#[test]
fn project_list_reports_keyless_ingest_disabled_under_read_auth() {
    let (_dir, db) = temp_db("project-auth");
    let output = run_with_env(
        &["project", "list", "-db", &db],
        &[("OTTERSCOPE_READ_AUTH", "1")],
    );
    assert_success(&output);
    let listed = stdout(&output);
    assert!(
        listed.contains("(keyless ingest disabled — an ingest key is required)"),
        "{listed}"
    );
    assert!(!listed.contains("open ingest"), "{listed}");
}

#[test]
fn unknown_flags_exit_two_with_go_wording() {
    for (args, flag) in [
        (vec!["token", "list", "-bogus=1"], "-bogus"),
        (vec!["serve", "-retentionn", "24h"], "-retentionn"),
        (vec!["serve", "-read_auth"], "-read_auth"),
    ] {
        let output = run(&args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}\nstdout:\n{}\nstderr:\n{}",
            stdout(&output),
            stderr(&output)
        );
        assert!(
            stderr(&output).contains(&format!("flag provided but not defined: {flag}")),
            "{args:?}\n{}",
            stderr(&output)
        );
    }
}

#[test]
fn invalid_flag_values_exit_two_with_go_wording() {
    for (args, message) in [
        (
            vec!["serve", "-read-auth=maybe"],
            "invalid boolean value \"maybe\" for -read-auth",
        ),
        (
            vec!["serve", "-retention", "10"],
            "invalid value \"10\" for flag -retention: time: missing unit in duration \"10\"",
        ),
        (
            vec!["serve", "-ingest-rate", "abc"],
            "invalid value \"abc\" for flag -ingest-rate: parse error",
        ),
        (
            vec!["serve", "-base-path", "/a//b"],
            "invalid value \"/a//b\" for flag -base-path",
        ),
    ] {
        let output = run(&args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}\nstdout:\n{}\nstderr:\n{}",
            stdout(&output),
            stderr(&output)
        );
        assert!(
            stderr(&output).contains(message),
            "{args:?}\nexpected {message:?} in:\n{}",
            stderr(&output)
        );
    }
}

#[test]
fn help_flags_print_usage_to_stdout_and_exit_zero() {
    for args in [
        vec!["-h"],
        vec!["--help"],
        vec!["serve", "-h"],
        vec!["serve", "--help"],
        vec!["token", "-help"],
        vec!["token", "add", "-h"],
        vec!["project", "list", "--help"],
        vec!["sample", "-h"],
        vec!["backup", "-h"],
        vec!["renormalize", "-h"],
        vec!["version", "--help"],
    ] {
        let output = run(&args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}\nstdout:\n{}\nstderr:\n{}",
            stdout(&output),
            stderr(&output)
        );
        assert!(
            stdout(&output).contains("usage: otterscope"),
            "{args:?}\n{}",
            stdout(&output)
        );
        assert!(
            stderr(&output).is_empty(),
            "{args:?} wrote to stderr:\n{}",
            stderr(&output)
        );
    }
}

#[test]
fn backup_writes_a_private_working_snapshot() {
    let (dir, db) = temp_db("backup");
    assert_success(&run(&["token", "add", "backup-me", "-db", &db]));
    let out = dir.join("snapshot.db");

    let backup = run(&["backup", "-o", path_str(&out), "-db", &db]);
    assert_success(&backup);
    assert_eq!(
        stdout(&backup),
        format!("backed up {db} to {}\n", out.display())
    );
    assert!(out.exists(), "backup file exists");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&out)
            .expect("backup metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "backup is owner-only");
    }

    let snapshot = Store::open(path_str(&out)).expect("open snapshot");
    assert_eq!(snapshot.list_tokens().expect("tokens").len(), 1);
    drop(snapshot);

    let missing = run(&["backup", "-db", &db]);
    assert!(!missing.status.success());
    assert!(
        stderr(&missing).contains("usage: otterscope backup -o <file> [-db path]"),
        "{}",
        stderr(&missing)
    );
}

#[test]
fn backup_creates_a_missing_output_directory() {
    // The Kubernetes CronJob writes to /data/backups/..., and the distroless
    // image has neither a shell nor mkdir. The requested path's parent must be
    // created by the binary itself.
    let (dir, db) = temp_db("backup-nested");
    let out = dir.join("backups").join("snapshot.db");
    assert!(!out.parent().expect("parent").exists());

    let backup = run(&["backup", "-o", path_str(&out), "-db", &db]);
    assert_success(&backup);
    assert!(out.exists(), "backup file exists");
    assert!(out.parent().expect("parent").is_dir());
}

#[test]
fn renormalize_noops_on_a_fresh_db_and_replays_a_seeded_batch() {
    let (_dir, db) = temp_db("renormalize");

    let fresh = run(&["renormalize", "-db", &db]);
    assert_success(&fresh);
    assert_eq!(
        stdout(&fresh),
        "replayed 0 raw batches through the current normalizer and pricing table\n"
    );

    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("pydantic_ai_chat.pb");
    let payload = std::fs::read(&fixture).expect("fixture");
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut encoder, &payload).expect("gzip fixture");
    let raw = encoder.finish().expect("gzip fixture");
    {
        let store = Store::open(&db).expect("open store");
        store
            .ingest_batch("checkout", &raw, &[])
            .expect("store raw batch");
    }

    let replay = run(&["renormalize", "-db", &db]);
    assert_success(&replay);
    assert_eq!(
        stdout(&replay),
        "replayed 1 raw batches through the current normalizer and pricing table\n"
    );

    let store = Store::open(&db).expect("open store");
    let metrics = store.metrics().expect("metrics");
    assert_eq!(metrics.runs, 1);
    assert_eq!(metrics.steps, 3);
    let trace_id = "5b8efff798038103d269b633813fc60c";
    let (_run, steps) = store
        .get_run_in_project("checkout", trace_id)
        .expect("run lookup")
        .expect("project stamped on the replayed run");
    assert_eq!(steps.len(), 3);
    assert_eq!(
        store
            .list_runs(&Filter::default(), 10, 0)
            .expect("runs")
            .len(),
        1
    );
}

#[test]
fn config_file_and_db_flag_keep_the_serve_precedence() {
    let (dir, _) = temp_db("config");
    let config_db = dir.join("from-config.db");
    let env_db = dir.join("from-env.db");
    let flag_db = dir.join("from-flag.db");
    let config = dir.join("config.json");
    std::fs::write(&config, format!("{{\"db\": {:?}}}", path_str(&config_db)))
        .expect("write config");
    let config_arg = path_str(&config);

    let added = run(&["token", "add", "from-config", "-config", config_arg]);
    assert_success(&added);
    assert_eq!(token_count(path_str(&config_db)), 1);

    let added = run_with_env(
        &["token", "add", "from-env", "-config", config_arg],
        &[("OTTERSCOPE_DB", path_str(&env_db))],
    );
    assert_success(&added);
    assert_eq!(token_count(path_str(&env_db)), 1);
    assert_eq!(token_count(path_str(&config_db)), 1);

    let added = run_with_env(
        &["token", "add", "from-flag", "-db", path_str(&flag_db)],
        &[
            ("OTTERSCOPE_CONFIG", config_arg),
            ("OTTERSCOPE_DB", path_str(&env_db)),
        ],
    );
    assert_success(&added);
    assert_eq!(token_count(path_str(&flag_db)), 1);
    assert_eq!(token_count(path_str(&env_db)), 1);
}
