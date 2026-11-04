//! `otterscope` — the command line.
//!
//! Subcommands and flags keep the Go spelling and precedence exactly, because
//! operators script them: flags override `OTTERSCOPE_*` env, which overrides the
//! config file, which overrides built-in defaults.

use std::process::ExitCode;

use otterscope::{args, config, server, store};

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = argv.first() else {
        usage();
        return ExitCode::from(2);
    };

    let rest = &argv[1..];
    let result = match command.as_str() {
        "serve" => serve(rest),
        "version" => {
            println!("{}", otterscope::VERSION);
            Ok(())
        }
        "renormalize" | "backup" | "token" | "sample" | "project" => Err(anyhow::anyhow!(
            "{command} is not ported yet"
        )),
        other => {
            eprintln!("unknown command {other:?}");
            usage();
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("otterscope: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!(
        "usage: otterscope <command>

commands:
  serve [-config f.json]  start the ingest + UI server
  project add <name>      create a project and print its ingest key
  project list            list projects and their ingest keys
  sample                  seed demo data (services, runs, assertions)
  token add <name>        create a read-API token and print it (use -admin
                          for a token that may also mutate)
  token list              list tokens by id, scope and last use
  backup -o <file>        write a consistent snapshot of the database
  renormalize             replay stored raw batches through the current
                          normalizer and pricing table (backfill after upgrades)
  version                 print version"
    );
}

fn serve(argv: &[String]) -> anyhow::Result<()> {
    let parsed = args::parse(argv, &["read-auth"]).map_err(anyhow::Error::msg)?;
    let config_path = parsed
        .get("config")
        .map(str::to_string)
        .or_else(|| std::env::var("OTTERSCOPE_CONFIG").ok())
        .unwrap_or_default();

    let mut cfg = config::Config::load(&config_path, |k| std::env::var(k).ok())
        .map_err(anyhow::Error::msg)?;

    if parsed.get("listen").is_some() {
        cfg.listen = parsed.string("listen", &cfg.listen);
    }
    if parsed.get("otlp").is_some() {
        cfg.otlp = parsed.string("otlp", &cfg.otlp);
    }
    if parsed.get("db").is_some() {
        cfg.db = parsed.string("db", &cfg.db);
    }
    if parsed.get("pricing").is_some() {
        cfg.pricing = parsed.string("pricing", &cfg.pricing);
    }
    if parsed.get("judge-url").is_some() {
        cfg.judge_url = parsed.string("judge-url", &cfg.judge_url);
    }
    if let Some(v) = parsed.get("judge-models") {
        cfg.judge_models = config::parse_model_list(v);
    }
    if let Some(v) = parsed.get("retention") {
        cfg.retention = config::parse_duration(v).map_err(anyhow::Error::msg)?;
    }
    if let Some(v) = parsed.get("alert-interval") {
        cfg.alert_interval = config::parse_duration(v).map_err(anyhow::Error::msg)?;
    }
    cfg.read_auth = parsed.bool("read-auth", cfg.read_auth).map_err(anyhow::Error::msg)?;
    if let Some(v) = parsed.get("ingest-rate") {
        cfg.ingest_rate = v.parse()?;
    }
    if let Some(v) = parsed.get("ingest-burst") {
        cfg.ingest_burst = v.parse()?;
    }
    if let Some(v) = parsed.get("ingest-max-spans") {
        cfg.max_spans_per_batch = v.parse()?;
    }
    if let Some(v) = parsed.get("ingest-max-trace-ids") {
        cfg.max_trace_ids_per_batch = v.parse()?;
    }
    cfg.base_path = otterscope::basepath::normalize(&parsed.string("base-path", &cfg.base_path))
        .map_err(anyhow::Error::msg)?;

    tracing::info!(db = %cfg.db, listen = %cfg.listen, otlp = %cfg.otlp, "starting");
    let store = store::Store::open(&cfg.db)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let app = server::router(server::AppState { store });
        let listener = tokio::net::TcpListener::bind(&cfg.listen).await?;
        tracing::info!("ui on http://{}", listener.local_addr()?);
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await?;
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}
