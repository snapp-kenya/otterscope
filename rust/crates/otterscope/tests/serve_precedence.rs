//! `serve` configuration precedence: flags beat conflicting `OTTERSCOPE_*`
//! variables, which beat the config file, which beats defaults.
//!
//! The old end-to-end version of this test spawned the binary on two chosen
//! ports. The server now always binds port 80, which is privileged, so the HTTP
//! side (every surface under one base path, OTLP sharing the app) is covered by
//! `server_api.rs` and the server unit tests, which drive the router directly.
//! This file keeps the precedence contract testable without a socket.
#![forbid(unsafe_code)]

use std::path::PathBuf;

use otterscope::config::Config;

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_string()).collect()
}

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

#[test]
fn flags_beat_env_which_beats_the_config_file() {
    let dir = temp_dir("precedence");
    let config = dir.join("config.json");
    std::fs::write(
        &config,
        r#"{
            "db": "/file.db",
            "basePath": "/filebase",
            "readAuth": false,
            "ingestRate": 7,
            "ingestBurst": 3,
            "retention": "24h"
        }"#,
    )
    .expect("write config");

    let args = argv(&[
        "-config",
        config.to_str().expect("config path"),
        "-db",
        "/flag.db",
        "-read-auth",
        "-base-path",
        "/flagbase",
    ]);
    let cfg = Config::serve(&args, |key| match key {
        // The -config flag must win over the env var, so this path is never read.
        "OTTERSCOPE_CONFIG" => Some("/does/not/exist.json".to_string()),
        "OTTERSCOPE_DB" => Some("/env.db".to_string()),
        "OTTERSCOPE_READ_AUTH" => Some("0".to_string()),
        "OTTERSCOPE_BASE_PATH" => Some("/envbase".to_string()),
        "OTTERSCOPE_INGEST_RATE" => Some("9".to_string()),
        _ => None,
    })
    .expect("serve config");

    assert_eq!(cfg.db, "/flag.db", "flag beat env and file");
    assert!(cfg.read_auth, "flag beat env");
    assert_eq!(cfg.base_path, "/flagbase", "flag beat env and file");
    assert_eq!(cfg.ingest_rate, 9.0, "env beat the file");
    assert_eq!(cfg.ingest_burst, 3.0, "file value survives");
    assert_eq!(
        cfg.retention,
        std::time::Duration::from_secs(24 * 60 * 60),
        "file value survives"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn removed_listener_flags_are_rejected() {
    for flag in ["-listen", "-otlp"] {
        let err = Config::serve(&argv(&[flag, ":9999"]), |_| None).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("flag provided but not defined: {flag}"),
            "the port is not configuration"
        );
    }
}
