//! End-to-end proof that `serve` resolves flags above conflicting `OTTERSCOPE_*`
//! variables and mounts every surface under the one resolved prefix.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use reqwest::StatusCode;

fn temp_dir(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "otterscope-serve-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn binary() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_otterscope"));
    command.env_clear();
    command
}

fn run(args: &[&str]) -> Output {
    binary().args(args).output().expect("run otterscope")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn value_of<'a>(text: &'a str, prefix: &str) -> &'a str {
    text.lines()
        .find_map(|line| line.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("no line starting with {prefix:?} in:\n{text}"))
        .trim()
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("free port");
    listener.local_addr().expect("local addr").port()
}

struct Server {
    child: Child,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_serve(db: &Path, ui: u16, otlp: u16, envs: &[(&str, &str)]) -> Server {
    let mut command = binary();
    command
        .args([
            "serve",
            "-db",
            db.to_str().expect("db path"),
            "-listen",
            &format!("127.0.0.1:{ui}"),
            "-otlp",
            &format!("127.0.0.1:{otlp}"),
            "-read-auth",
            "-base-path",
            "/flagbase",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (key, value) in envs {
        command.env(key, value);
    }
    let child = command.spawn().expect("spawn serve");
    Server { child }
}

async fn wait_ready(client: &reqwest::Client, url: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(response) = client.get(url).send().await
            && response.status().is_success()
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "server did not become ready at {url}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn flags_win_over_conflicting_env_and_every_surface_shares_the_prefix() {
    let dir = temp_dir("precedence");
    let db = dir.join("test.db");
    let db_arg = db.to_str().expect("db path");

    let token_output = run(&["token", "add", "script", "-db", db_arg]);
    assert!(
        token_output.status.success(),
        "{}",
        String::from_utf8_lossy(&token_output.stderr)
    );
    let token = value_of(&stdout(&token_output), "token: ").to_string();
    let project_output = run(&["project", "add", "checkout", "-db", db_arg]);
    assert!(
        project_output.status.success(),
        "{}",
        String::from_utf8_lossy(&project_output.stderr)
    );
    let ingest_key = value_of(&stdout(&project_output), "ingest key: ").to_string();

    let ui = free_port();
    let mut otlp = free_port();
    while otlp == ui {
        otlp = free_port();
    }
    let _server = spawn_serve(
        &db,
        ui,
        otlp,
        &[
            ("OTTERSCOPE_READ_AUTH", "0"),
            ("OTTERSCOPE_BASE_PATH", "/envbase"),
        ],
    );

    let client = reqwest::Client::new();
    let ui_base = format!("http://127.0.0.1:{ui}");
    let otlp_base = format!("http://127.0.0.1:{otlp}");
    wait_ready(&client, &format!("{ui_base}/flagbase/healthz")).await;

    // -read-auth beat OTTERSCOPE_READ_AUTH=0: the API is closed.
    let response = client
        .get(format!("{ui_base}/flagbase/api/runs"))
        .send()
        .await
        .expect("api");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );

    let response = client
        .get(format!("{ui_base}/flagbase/api/runs"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("authorized api");
    assert_eq!(response.status(), StatusCode::OK);

    // -base-path beat OTTERSCOPE_BASE_PATH=/envbase for every mount.
    let response = client
        .get(format!("{ui_base}/flagbase/"))
        .send()
        .await
        .expect("ui");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html"))
    );
    for path in ["/envbase/", "/envbase/api/runs", "/envbase/mcp"] {
        let response = client
            .get(format!("{ui_base}{path}"))
            .send()
            .await
            .expect("outside prefix");
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }

    // MCP is mounted at the flag prefix and still takes the read token.
    let response = client
        .post(format!("{ui_base}/flagbase/mcp"))
        .send()
        .await
        .expect("mcp");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Ingest follows the same prefix and the same read-auth key requirement.
    let response = client
        .post(format!("{otlp_base}/flagbase/v1/traces"))
        .header("content-type", "application/json")
        .body(r#"{"resourceSpans":[]}"#)
        .send()
        .await
        .expect("ingest without key");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = client
        .post(format!("{otlp_base}/flagbase/v1/traces"))
        .header("content-type", "application/json")
        .bearer_auth(&ingest_key)
        .body(r#"{"resourceSpans":[]}"#)
        .send()
        .await
        .expect("ingest");
    assert_eq!(response.status(), StatusCode::OK);
    let response = client
        .post(format!("{otlp_base}/envbase/v1/traces"))
        .header("content-type", "application/json")
        .bearer_auth(&ingest_key)
        .body(r#"{"resourceSpans":[]}"#)
        .send()
        .await
        .expect("ingest outside prefix");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let _ = std::fs::remove_dir_all(dir);
}
