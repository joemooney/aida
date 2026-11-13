#![recursion_limit = "256"]
//! AIDA MCP stdio transport.
// trace:TASK-3429 | ai:codex
// The shared descriptor helper stays below this crate (advisor amendment).
pub use aida_cli_lib::mcp_schema;
mod serve;
pub use serve::run_mcp_serve;
mod mcp;
pub use mcp::*;
