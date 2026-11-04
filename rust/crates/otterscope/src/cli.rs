//! Database subcommands ported from `cmd/otterscope/main.go`: `token`,
//! `project`, `backup` and `renormalize`, plus the config/flag plumbing they
//! share with `serve`.

use anyhow::Context;
use chrono::{DateTime, SecondsFormat, Utc};

use otterscope::{args, config, ingest, pricing, server, store};

/// Resolve the effective configuration the way every subcommand does:
/// `-config`, else `OTTERSCOPE_CONFIG`, then the file, then `OTTERSCOPE_*` env
/// vars. Explicit flags override the result afterwards.
pub fn load_config(argv: &[String]) -> anyhow::Result<(config::Config, String)> {
    let mut path = scan_arg(argv, "config").unwrap_or_default();
    if path.is_empty() {
        path = std::env::var("OTTERSCOPE_CONFIG").unwrap_or_default();
    }
    let cfg =
        config::Config::load(&path, |key| std::env::var(key).ok()).map_err(anyhow::Error::msg)?;
    Ok((cfg, path))
}

/// The value of `-name`/`--name` in `argv`, including the `-name=value` form.
fn scan_arg(argv: &[String], name: &str) -> Option<String> {
    let short = format!("-{name}");
    let long = format!("--{name}");
    let short_eq = format!("-{name}=");
    let long_eq = format!("--{name}=");
    for (i, arg) in argv.iter().enumerate() {
        if *arg == short || *arg == long {
            return argv.get(i + 1).cloned();
        }
        if let Some(value) = arg.strip_prefix(&short_eq) {
            return Some(value.to_string());
        }
        if let Some(value) = arg.strip_prefix(&long_eq) {
            return Some(value.to_string());
        }
    }
    None
}

fn open_store(db: &str) -> anyhow::Result<store::Store> {
    store::Store::open(db).context("open store")
}

pub fn token(argv: &[String]) -> anyhow::Result<()> {
    let Some(sub) = argv.first() else {
        anyhow::bail!("usage: otterscope token [add <name> | list] [-db path]");
    };
    if sub != "add" && sub != "list" {
        anyhow::bail!("unknown subcommand {sub:?}");
    }
    let (cfg, _) = load_config(argv)?;
    let mut rest = &argv[1..];
    let mut name = String::new();
    if sub == "add" {
        match rest.first() {
            Some(candidate) if !candidate.starts_with('-') => {
                name = candidate.clone();
                rest = &rest[1..];
            }
            _ => anyhow::bail!("usage: otterscope token add <name> [-admin] [-db path]"),
        }
    }
    let parsed = args::parse(rest, &["config", "db"], &["admin"])?;
    let db = parsed.string("db", &cfg.db);
    let store = open_store(&db)?;

    if sub == "add" {
        let scope = if parsed.bool("admin", false)? {
            store::Scope::Admin
        } else {
            store::Scope::Read
        };
        let token = store.create_token(&name, scope)?;
        println!(
            "{} token {:?} created (id {})",
            token.scope.as_str(),
            token.name,
            token.id
        );
        println!("token: {}", token.token);
        println!();
        println!("use it as:");
        println!(
            "  curl -H \"Authorization: Bearer {}\" http://localhost/api/runs",
            token.token
        );
        println!();
        println!("The token is not stored; this is the only time it is shown.");
    } else {
        for token in store.list_tokens()? {
            let used = token
                .last_used_at
                .map(rfc3339)
                .unwrap_or_else(|| "never".to_string());
            println!(
                "{:<20} {:<20} {:<6} created {}, last used {}",
                token.id,
                token.name,
                token.scope.as_str(),
                rfc3339(token.created_at),
                used
            );
        }
    }
    Ok(())
}

pub fn project(argv: &[String]) -> anyhow::Result<()> {
    let Some(sub) = argv.first() else {
        anyhow::bail!("usage: otterscope project [add <name> | list] [-db path]");
    };
    if sub != "add" && sub != "list" {
        anyhow::bail!("unknown subcommand {sub:?}");
    }
    let (cfg, _) = load_config(argv)?;
    let mut rest = &argv[1..];
    let mut name = String::new();
    if sub == "add" {
        match rest.first() {
            Some(candidate) if !candidate.starts_with('-') => {
                name = candidate.clone();
                rest = &rest[1..];
            }
            _ => anyhow::bail!("usage: otterscope project add <name> [-db path]"),
        }
    }
    let parsed = args::parse(rest, &["config", "db"], &[])?;
    let db = parsed.string("db", &cfg.db);
    let store = open_store(&db)?;

    if sub == "add" {
        let project = store.create_project(&name)?;
        println!("project {:?} created", project.name);
        println!("ingest key: {}", project.ingest_key);
        println!();
        println!("agents send it via:");
        println!(
            "  export OTEL_EXPORTER_OTLP_HEADERS=\"Authorization=Bearer {}\"",
            project.ingest_key
        );
    } else {
        for project in store.list_projects()? {
            let key = if project.ingest_key.is_empty() {
                if cfg.read_auth {
                    "(keyless ingest disabled — an ingest key is required)"
                } else {
                    "(no ingest key — keyless ingest depends on the server's -read-auth setting)"
                }
            } else {
                project.ingest_key.as_str()
            };
            println!("{:<20} {}", project.name, key);
        }
    }
    Ok(())
}

pub fn backup(argv: &[String]) -> anyhow::Result<()> {
    let (cfg, _) = load_config(argv)?;
    let parsed = args::parse(argv, &["config", "db", "o"], &[])?;
    let db = parsed.string("db", &cfg.db);
    let Some(out) = parsed.get("o").filter(|out| !out.is_empty()) else {
        anyhow::bail!("usage: otterscope backup -o <file> [-db path]");
    };
    // Create the destination directory so a scheduled backup can target
    // /data/backups/... on a fresh volume without a shell or init container.
    // VACUUM INTO refuses a missing parent, and the distroless image has no
    // mkdir.
    if let Some(parent) = std::path::Path::new(out)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create backup directory {}", parent.display()))?;
    }
    let store = open_store(&db)?;
    store.backup(out)?;
    println!("backed up {db} to {out}");
    Ok(())
}

pub fn renormalize(argv: &[String]) -> anyhow::Result<()> {
    let (cfg, _) = load_config(argv)?;
    let parsed = args::parse(argv, &["config", "db", "pricing"], &[])?;
    let db = parsed.string("db", &cfg.db);
    let pricing_path = parsed.string("pricing", &cfg.pricing);

    let mut prices = pricing::Table::default();
    if !pricing_path.is_empty() {
        let data = std::fs::read(&pricing_path)
            .map_err(|err| anyhow::anyhow!("read pricing overrides {}: {err}", pricing_path))?;
        prices.merge_json(&data)?;
    }

    let store = open_store(&db)?;
    let mut replayed = 0usize;
    store.each_raw_batch(|project, raw| {
        let mut steps = ingest::renormalize_batch(raw).context("decode raw batch")?;
        server::price_steps(&mut steps, &prices);
        for step in &mut steps {
            step.project = project.to_string();
        }
        store.upsert_steps(&steps)?;
        replayed += 1;
        Ok(())
    })?;
    println!("replayed {replayed} raw batches through the current normalizer and pricing table");
    Ok(())
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}
