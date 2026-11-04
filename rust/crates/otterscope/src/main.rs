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
        "sample" => sample(rest),
        "renormalize" | "backup" | "token" | "project" => {
            Err(anyhow::anyhow!("{command} is not ported yet"))
        }
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

fn sample(argv: &[String]) -> anyhow::Result<()> {
    let parsed = args::parse(argv, &[]).map_err(anyhow::Error::msg)?;
    let config_path = parsed
        .get("config")
        .map(str::to_string)
        .or_else(|| std::env::var("OTTERSCOPE_CONFIG").ok())
        .unwrap_or_default();
    let cfg = config::Config::load(&config_path, |k| std::env::var(k).ok())
        .map_err(anyhow::Error::msg)?;
    let db = parsed.string("db", &cfg.db);
    let runs = parsed.int("runs", 60).map_err(anyhow::Error::msg)?;

    let store = store::Store::open(&db)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(otterscope::sample::seed(&store, runs.max(0) as usize))?;
    println!("seeded {runs} sample runs into {db}");
    Ok(())
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
    cfg.read_auth = parsed
        .bool("read-auth", cfg.read_auth)
        .map_err(anyhow::Error::msg)?;
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
    let store = std::sync::Arc::new(store::Store::open(&cfg.db)?);

    let mut prices = otterscope::pricing::Table::default();
    if !cfg.pricing.is_empty() {
        let data = std::fs::read(&cfg.pricing)
            .map_err(|err| anyhow::anyhow!("read pricing overrides {}: {err}", cfg.pricing))?;
        prices.merge_json(&data)?;
        tracing::info!(file = %cfg.pricing, "pricing overrides loaded");
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let options = server::Options::from_config(&cfg);
        let judge = otterscope::evals::Endpoint {
            base_url: options.judge_url.clone(),
            key: options.judge_key.clone(),
            models: options.judge_models.clone(),
        };
        let hub = server::Hub::new();
        let evaluator = server::Evaluator::start(store.clone(), judge);
        let sink = std::sync::Arc::new(server::StoreSink::new(
            store.clone(),
            prices,
            evaluator.clone(),
            hub.clone(),
        ));
        let app = server::router_with(
            server::AppState {
                store: store.clone(),
            },
            options,
            hub,
        );

        let resolve_store = store.clone();
        let keyless_allowed = !cfg.read_auth;
        let resolve: otterscope::ingest::KeyResolver = std::sync::Arc::new(move |key: &str| {
            if key.is_empty() {
                keyless_allowed.then(|| "default".to_string())
            } else {
                resolve_store.project_for_key(key)
            }
        });
        let otlp_options = otterscope::ingest::Options {
            base_path: cfg.base_path.clone(),
            rate_per_sec: cfg.ingest_rate,
            burst: cfg.ingest_burst,
            require_key: cfg.read_auth,
            max_spans: cfg.max_spans_per_batch,
            max_trace_ids: cfg.max_trace_ids_per_batch,
        };
        let otlp_app = otterscope::ingest::router(otlp_options, sink, resolve);

        let ui_listener = tokio::net::TcpListener::bind(&cfg.listen).await?;
        let otlp_listener = tokio::net::TcpListener::bind(&cfg.otlp).await?;
        tracing::info!(
            ui = %ui_listener.local_addr()?,
            otlp = %otlp_listener.local_addr()?,
            "listening"
        );

        let mut watcher = if cfg.alert_interval > std::time::Duration::ZERO {
            let mut watcher = otterscope::alerts::Watcher::new(store.clone(), cfg.alert_interval)?;
            watcher.start();
            Some(watcher)
        } else {
            None
        };

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = shutdown_tx.send(true);
        });
        let mut ui_shutdown = shutdown_rx.clone();
        let mut otlp_shutdown = shutdown_rx;
        tokio::try_join!(
            axum::serve(ui_listener, app).with_graceful_shutdown(async move {
                let _ = ui_shutdown.changed().await;
            }),
            axum::serve(otlp_listener, otlp_app).with_graceful_shutdown(async move {
                let _ = otlp_shutdown.changed().await;
            }),
        )?;

        evaluator.stop().await;
        if let Some(watcher) = watcher.as_mut() {
            watcher.stop().await;
        }
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}
