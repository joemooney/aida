//! Shared claim naming for MCP and session cleanup.
// trace:TASK-3429 | ai:codex
use std::path::{Path, PathBuf};

// trace:TASK-b6b1278890774ef993d2ebc327862470-271 | ai:claude
/// Filename a MCP `claim_task` lease writes to. Keyed by lower-cased spec_id
/// so two concurrent claims on the same spec target the same path — letting
/// `OpenOptions::create_new(true)` enforce single-winner semantics. Distinct
/// from the `<lease_id>.toml` shape `aida session start` uses, so the two
/// claim modes don't collide.
// Shared so `aida session end` can reap the sibling mcp-claim for the
// scope it just released (BUG-694). trace:BUG-694 | ai:claude
pub fn mcp_claim_path(dir: &Path, spec_id: &str) -> PathBuf {
    dir.join(format!("mcp-claim.{}.toml", spec_id.to_ascii_lowercase()))
}
