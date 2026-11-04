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
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
