//! Agy (Antigravity CLI) live agent-state stdin JSON → [`ClientLiveFields`]
//! (TASK-1479).
//!
//! Antigravity's command-backed `statusLine` (wired by
//! `statusline_cmd::antigravity_statusline_fragment`) can pipe a live
//! agent-state JSON snapshot to the command on stdin, the same shape as
//! Claude Code's `statusLine` integration but for Antigravity's own runtime
//! (model, context usage, activity, VCS). **No Agy payload shape is
//! recorded anywhere in this repo** — checked `docs/agents/antigravity-*`,
//! `docs/competitive-analysis/**/*agy*`, and the SPIKE-27 architecture
//! inventory (`14-agy-architecture.md`) at TASK-1479 authoring time; none
//! of them capture an actual statusline JSON sample. This adapter is
//! therefore SPECULATIVE and TOLERANT by design:
//!
//! - every known field is optional; an absent field degrades to `None`,
//!   never an error;
//! - the adapter accepts multiple plausible key spellings per field (see
//!   below) so it has the best chance of matching whatever Agy actually
//!   sends, without needing a round of guess-and-fix once a real payload is
//!   observed;
//! - unknown fields are ignored by construction (no `deny_unknown_fields`).
//!
//! Field spellings this adapter tries, in priority order:
//!
//! | Live field | JSON paths tried |
//! |---|---|
//! | model | `model` (string), `model.name`, `model.display_name`, `model.id` |
//! | context_remaining_pct | `context.remaining_percent`, `context.remaining_pct`, derived from `context.used_percent`/`used_pct`, derived from `context.used_tokens`/`total_tokens` |
//! | activity | `activity` (string), `status` (string) |
//! | vcs_branch / vcs_dirty | `vcs.branch`+`vcs.dirty`, `git.branch`+`git.dirty` (either top-level key) |
//!
//! **Tripwire:** when a real Agy statusline payload is captured (a live
//! session, or documented upstream), fact-check this table against it the
//! way `statusline_claude_adapter` cross-checked Claude Code's schema docs,
//! and delete whichever guessed spellings turned out wrong.
// trace:TASK-1479 | ai:claude

use serde::Deserialize;
use serde_json::Value;

use crate::statusline_contract::ClientLiveFields;

#[derive(Deserialize, Default)]
struct AgyModel {
    name: Option<String>,
    display_name: Option<String>,
    id: Option<String>,
}

#[derive(Deserialize, Default)]
struct AgyContext {
    remaining_percent: Option<f64>,
    remaining_pct: Option<f64>,
    used_percent: Option<f64>,
    used_pct: Option<f64>,
    used_tokens: Option<f64>,
    total_tokens: Option<f64>,
}

#[derive(Deserialize, Default)]
struct AgyVcs {
    branch: Option<String>,
    dirty: Option<bool>,
}

#[derive(Deserialize, Default)]
struct AgyPayload {
    /// Accepted as either a bare string (`"model": "gemini-3-pro"`) or an
    /// object (`"model": {"name": "..."}`) — tolerant of either shape.
    model: Option<Value>,
    context: Option<AgyContext>,
    activity: Option<String>,
    status: Option<String>,
    vcs: Option<AgyVcs>,
    git: Option<AgyVcs>,
}

fn model_from_value(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(_) => {
            let m: AgyModel = serde_json::from_value(v.clone()).ok()?;
            m.name.or(m.display_name).or(m.id)
        }
        _ => None,
    }
}

fn context_remaining_pct(ctx: &AgyContext) -> Option<u8> {
    let pct = ctx
        .remaining_percent
        .or(ctx.remaining_pct)
        .or_else(|| ctx.used_percent.map(|u| 100.0 - u))
        .or_else(|| ctx.used_pct.map(|u| 100.0 - u))
        .or_else(|| match (ctx.used_tokens, ctx.total_tokens) {
            (Some(used), Some(total)) if total > 0.0 => Some(100.0 - (used / total * 100.0)),
            _ => None,
        })?;
    Some(pct.clamp(0.0, 100.0).round() as u8)
}

/// Parse Agy's (speculative — see module docs) live agent-state stdin JSON
/// into the shared live-fields shape. Malformed/non-JSON input degrades to
/// `ClientLiveFields::default()` rather than erroring — same hot-path
/// contract as the Claude adapter — and the raw input is never echoed back.
// trace:TASK-1479 | ai:claude
pub fn parse_agy_payload(raw: &str) -> ClientLiveFields {
    let payload: AgyPayload = match serde_json::from_str(raw) {
        Ok(p) => p,
        Err(_) => return ClientLiveFields::default(),
    };

    let model = payload
        .model
        .as_ref()
        .and_then(model_from_value)
        .filter(|s| !s.trim().is_empty());

    let context_remaining_pct = payload.context.as_ref().and_then(context_remaining_pct);

    let activity = payload
        .activity
        .or(payload.status)
        .filter(|s| !s.trim().is_empty());

    let (vcs_branch, vcs_dirty) = payload
        .vcs
        .or(payload.git)
        .map(|v| (v.branch.filter(|s| !s.trim().is_empty()), v.dirty))
        .unwrap_or((None, None));

    ClientLiveFields {
        model,
        context_remaining_pct,
        activity,
        vcs_branch,
        vcs_dirty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_representative_full_payload_object_model() {
        let raw = r#"{
            "model": {"name": "gemini-3-pro"},
            "context": {"remaining_percent": 71.0},
            "activity": "editing",
            "vcs": {"branch": "main", "dirty": true}
        }"#;
        let live = parse_agy_payload(raw);
        assert_eq!(live.model.as_deref(), Some("gemini-3-pro"));
        assert_eq!(live.context_remaining_pct, Some(71));
        assert_eq!(live.activity.as_deref(), Some("editing"));
        assert_eq!(live.vcs_branch.as_deref(), Some("main"));
        assert_eq!(live.vcs_dirty, Some(true));
    }

    #[test]
    fn parses_bare_string_model() {
        let live = parse_agy_payload(r#"{"model": "gemini-3-pro"}"#);
        assert_eq!(live.model.as_deref(), Some("gemini-3-pro"));
    }

    #[test]
    fn derives_remaining_pct_from_used_percent_alt_spelling() {
        let live = parse_agy_payload(r#"{"context": {"used_pct": 30.0}}"#);
        assert_eq!(live.context_remaining_pct, Some(70));
    }

    #[test]
    fn derives_remaining_pct_from_token_counts() {
        let live =
            parse_agy_payload(r#"{"context": {"used_tokens": 50000, "total_tokens": 200000}}"#);
        assert_eq!(live.context_remaining_pct, Some(75));
    }

    #[test]
    fn status_is_accepted_as_activity_alias() {
        let live = parse_agy_payload(r#"{"status": "thinking"}"#);
        assert_eq!(live.activity.as_deref(), Some("thinking"));
        // `activity` wins when both are present.
        let live = parse_agy_payload(r#"{"activity": "editing", "status": "thinking"}"#);
        assert_eq!(live.activity.as_deref(), Some("editing"));
    }

    #[test]
    fn git_key_is_accepted_as_vcs_alias() {
        let live = parse_agy_payload(r#"{"git": {"branch": "feature-x", "dirty": false}}"#);
        assert_eq!(live.vcs_branch.as_deref(), Some("feature-x"));
        assert_eq!(live.vcs_dirty, Some(false));
    }

    #[test]
    fn unknown_fields_are_ignored_not_errors() {
        let raw =
            r#"{"session_uuid": "xyz", "extra_nested": {"whatever": 1}, "model": "gemini-3-pro"}"#;
        let live = parse_agy_payload(raw);
        assert_eq!(live.model.as_deref(), Some("gemini-3-pro"));
    }

    #[test]
    fn empty_or_malformed_payload_degrades_to_default() {
        assert_eq!(parse_agy_payload(""), ClientLiveFields::default());
        assert_eq!(parse_agy_payload("not json"), ClientLiveFields::default());
        assert_eq!(parse_agy_payload("{}"), ClientLiveFields::default());
        assert_eq!(
            parse_agy_payload("{ malformed"),
            ClientLiveFields::default()
        );
    }

    #[test]
    fn blank_activity_and_branch_are_treated_as_absent() {
        let raw = r#"{"activity": "   ", "vcs": {"branch": "  "}}"#;
        let live = parse_agy_payload(raw);
        assert_eq!(live.activity, None);
        assert_eq!(live.vcs_branch, None);
    }
}
