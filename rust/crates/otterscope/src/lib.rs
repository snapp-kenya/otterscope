//! Otterscope — lightweight, self-hosted observability and evals for AI agents.
//!
//! Rust port of the Go implementation. Modules mirror the original `internal/`
//! packages one-to-one so the Go tree stays legible as the reference while the
//! port is in progress: `model`, `store`, `ingest`, `server`, `config`,
//! `basepath`, plus `pricing`, `evals`, `alerts`, `mcp` and `sample`.

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
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
