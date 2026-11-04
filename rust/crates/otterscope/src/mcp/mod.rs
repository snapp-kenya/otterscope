//! MCP server over HTTP (port of `internal/mcp`).
//!
//! Exposes Otterscope's read-only data as MCP tools over a stateless Streamable
//! HTTP endpoint (protocol revision 2026-07-28), so an agent can query its own
//! traces. The store is reached through [`McpStore`], which
//! [`crate::store::Store`] implements.

mod server;
mod tools;
mod views;

pub use server::{Handler, HttpReply, PROTOCOL_VERSION, SERVER_NAME, SUPPORTED_VERSIONS};
pub use tools::{McpStore, ToolDef, run_tool, tool_defs};
