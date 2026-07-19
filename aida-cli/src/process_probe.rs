//! Cross-platform process inspection for session liveness.
//!
//! BUG-677: the probe itself now lives in `aida_core::liveness` so `aida-tui`
//! can compute per-spec liveness in-process (no `aida ps` subprocess). This
//! module is a thin re-export that keeps the historical `crate::process_probe`
//! path — and the ~80 existing call sites — working unchanged. The moved code
//! (and its unit tests) is the SAME implementation; see `aida-core/src/liveness.rs`.
//!
//! trace:STORY-69 trace:BUG-677 | ai:claude

// Re-export the subset of the shared probe this crate references in code. The
// full probe API (uncached probe, jsonl helpers, RECENT_JSONL_WINDOW) lives in
// `aida_core::liveness` — reach for it there directly if a new caller needs it;
// mirroring an unused item here just trips the unused-import lint.
// trace:BUG-677 | ai:claude
pub use aida_core::liveness::{
    encode_cwd_for_projects, nearest_claude_ancestor_pid, pid_is_alive, probe_live_claude_sessions,
    walk_ancestor_pids, LiveSession,
};
