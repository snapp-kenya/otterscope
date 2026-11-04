//! Otterscope — lightweight, self-hosted observability and evals for AI agents.
//!
//! The crate is organised by responsibility: `model`, `store`, `ingest`,
//! `server`, `config` and `basepath`, plus `pricing`, `evals`, `alerts`, `mcp`
//! and `sample`. It was ported from the original Go implementation, which is no
//! longer in the tree.

pub mod alerts;
pub mod args;
pub mod basepath;
pub mod config;
pub mod evals;
pub mod ingest;
pub mod mcp;
pub mod model;
pub mod pricing;
pub mod sample;
pub mod server;
pub mod store;

/// Version reported by `otterscope version` and the API.
///
/// The container build sets `OTTERSCOPE_VERSION` from `--build-arg version`
/// (the short git SHA in deploy/k8s/deploy.sh); a plain cargo build reports
/// the crate version from `Cargo.toml`.
pub const VERSION: &str = match option_env!("OTTERSCOPE_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};
