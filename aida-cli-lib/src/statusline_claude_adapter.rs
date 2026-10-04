//! Claude Code statusLine stdin JSON → [`ClientLiveFields`] (TASK-1479).
//!
//! Claude Code pipes a JSON snapshot to a command-backed `statusLine` on
//! every render (see `.claude/settings.json` → `statusLine.command`, wired
//! by `statusline_cmd::claude_statusline_block`). This adapter is a thin,
//! read-only parse: it pulls the fields this contract cares about and drops
//! everything else. It never re-derives the AIDA segment — that stays
//! `statusline_cmd::handle_statusline_command`'s job — and it never retains
//! or logs the raw payload (privacy contract: `statusline_contract`).
//!
//! Schema notes (fact-checked against Claude Code's statusline JSON schema
//! docs; every field below is independently optional/nullable in practice,
//! so every struct field here is `Option`):
//!
//! - `model.display_name` / `model.id` — model label. Prefer display_name.
//! - `context_window.used_percentage` / `.remaining_percentage` — one or
//!   both may be present; both may be `null` early in a session (before the
//!   first API call) or after `/compact`. We derive whichever the payload
//!   didn't send directly from the one it did.
//! - `worktree.branch` — the ONLY branch info in the payload, and only
//!   present in an active worktree session; Claude Code does not include
//!   git status/dirty state for the main repo at all (by design — the
//!   statusline command is expected to shell out to git itself if it wants
//!   that, which AIDA's own segment already does via the session lease).
//!   `vcs_dirty` is therefore always `None` from this adapter.
//! - There is no activity / current-tool field in the payload as of this
//!   writing — the JSON is a point-in-time snapshot, not a live activity
//!   feed. `activity` is therefore always `None` from this adapter; if
//!   Claude Code adds one later, wire it here (tolerant — an absent field
//!   degrades silently rather than erroring).
//!
//! Unknown top-level/nested keys (`cost`, `rate_limits`, `prompt_cache`,
//! `pr`, `vim`, `agent`, …) are ignored by construction: these structs have
//! no `deny_unknown_fields`, so serde drops anything not listed here.
// trace:TASK-1479 | ai:claude

use serde::Deserialize;

use crate::statusline_contract::ClientLiveFields;

#[derive(Deserialize, Default)]
struct ClaudeModel {
    display_name: Option<String>,
    id: Option<String>,
}

#[derive(Deserialize, Default)]
struct ClaudeContextWindow {
    used_percentage: Option<f64>,
    remaining_percentage: Option<f64>,
}

#[derive(Deserialize, Default)]
struct ClaudeWorktree {
    branch: Option<String>,
}

#[derive(Deserialize, Default)]
struct ClaudePayload {
    model: Option<ClaudeModel>,
    context_window: Option<ClaudeContextWindow>,
    worktree: Option<ClaudeWorktree>,
}

/// Parse Claude Code's statusLine stdin JSON into the shared live-fields
/// shape. Malformed/non-JSON input degrades to
/// `ClientLiveFields::default()` (no live segment) rather than erroring —
/// this runs on a hot path that must never fail the statusline render, and
/// the raw input is never echoed back (parse errors are discarded, not
/// surfaced).
// trace:TASK-1479 | ai:claude
pub fn parse_claude_payload(raw: &str) -> ClientLiveFields {
    let payload: ClaudePayload = match serde_json::from_str(raw) {
        Ok(p) => p,
        Err(_) => return ClientLiveFields::default(),
    };

    let model = payload.model.as_ref().and_then(|m| {
        m.display_name
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| m.id.clone().filter(|s| !s.trim().is_empty()))
    });

    let context_remaining_pct = payload.context_window.as_ref().and_then(|cw| {
        cw.remaining_percentage
            .or_else(|| cw.used_percentage.map(|used| 100.0 - used))
            .map(|pct| pct.clamp(0.0, 100.0).round() as u8)
    });

    let vcs_branch = payload
        .worktree
        .as_ref()
        .and_then(|w| w.branch.clone())
        .filter(|s| !s.trim().is_empty());

    ClientLiveFields {
        model,
        context_remaining_pct,
        // No activity/current-tool field in the payload (schema-verified);
        // no dirty-state field for the main repo either.
        activity: None,
        vcs_branch,
        vcs_dirty: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_representative_full_payload() {
        let raw = r#"{
            "hook_event_name": "Status",
            "session_id": "abc123",
            "cwd": "/home/joe/ai/aida",
            "model": {"id": "claude-sonnet-4-5-20250929", "display_name": "Sonnet 4.5"},
            "workspace": {"current_dir": "/home/joe/ai/aida", "project_dir": "/home/joe/ai/aida"},
            "version": "2.1.260",
            "context_window": {
                "total_input_tokens": 76000,
                "context_window_size": 200000,
                "used_percentage": 38.0,
                "remaining_percentage": 62.0,
                "current_usage": {"input_tokens": 76000}
            },
            "cost": {"total_cost_usd": 0.42},
            "worktree": {"name": "wt-task-1479", "branch": "claude/task-1479"}
        }"#;
        let live = parse_claude_payload(raw);
        assert_eq!(live.model.as_deref(), Some("Sonnet 4.5"));
        assert_eq!(live.context_remaining_pct, Some(62));
        assert_eq!(live.vcs_branch.as_deref(), Some("claude/task-1479"));
        assert_eq!(live.activity, None);
        assert_eq!(live.vcs_dirty, None);
    }

    #[test]
    fn derives_remaining_pct_from_used_pct_when_only_used_is_sent() {
        let raw = r#"{"context_window": {"used_percentage": 25.0}}"#;
        let live = parse_claude_payload(raw);
        assert_eq!(live.context_remaining_pct, Some(75));
    }

    #[test]
    fn missing_optional_fields_degrade_to_none() {
        // Minimal payload — most fields absent, `context_window.*` null
        // (pre-first-API-call state per schema docs).
        let raw = r#"{"model": {"id": "claude-sonnet-4-5"}, "context_window": {"used_percentage": null, "remaining_percentage": null}}"#;
        let live = parse_claude_payload(raw);
        assert_eq!(live.model.as_deref(), Some("claude-sonnet-4-5"));
        assert_eq!(live.context_remaining_pct, None);
        assert_eq!(live.vcs_branch, None);
    }

    #[test]
    fn absent_worktree_and_context_window_yield_default_live_fields() {
        let raw = r#"{"session_id": "abc", "cwd": "/repo"}"#;
        let live = parse_claude_payload(raw);
        assert_eq!(live, ClientLiveFields::default());
    }

    #[test]
    fn unknown_top_level_and_nested_fields_are_ignored_not_errors() {
        let raw = r#"{
            "rate_limits": {"five_hour": {"used_percentage": 12.0}},
            "prompt_cache": {"warm": true, "hit_ratio": 0.9},
            "pr": {"number": 42, "url": "https://example.invalid/42"},
            "vim": {"mode": "insert"},
            "agent": {"name": "reviewer"},
            "model": {"display_name": "Sonnet 4.5"}
        }"#;
        let live = parse_claude_payload(raw);
        assert_eq!(live.model.as_deref(), Some("Sonnet 4.5"));
    }

    #[test]
    fn malformed_json_degrades_to_default_not_a_panic_or_error() {
        assert_eq!(
            parse_claude_payload("not json at all"),
            ClientLiveFields::default()
        );
        assert_eq!(parse_claude_payload(""), ClientLiveFields::default());
        assert_eq!(parse_claude_payload("{"), ClientLiveFields::default());
    }

    #[test]
    fn blank_display_name_falls_back_to_id() {
        let raw = r#"{"model": {"display_name": "   ", "id": "claude-sonnet-4-5"}}"#;
        let live = parse_claude_payload(raw);
        assert_eq!(live.model.as_deref(), Some("claude-sonnet-4-5"));
    }
}
