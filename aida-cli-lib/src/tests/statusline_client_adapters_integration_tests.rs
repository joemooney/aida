//! End-to-end coverage for TASK-1479: representative per-client stdin
//! payloads, parsed by each adapter and combined with a stable AIDA segment
//! via the shared formatter — the same pipeline `handle_statusline_command`
//! runs when `--client` is passed. Exercises the acceptance-criteria
//! surface directly (formatting + fallback behavior per client) without
//! needing a live project/cache on disk.
// trace:TASK-1479 | ai:claude

use crate::statusline_agy_adapter::parse_agy_payload;
use crate::statusline_claude_adapter::parse_claude_payload;
use crate::statusline_contract::{format_combined, ClientLiveFields};

const AIDA_SEGMENT: &str = "aida · aida-project · role:implementer · @TASK-1479 · q:2";

#[test]
fn claude_representative_payload_combines_with_aida_segment() {
    let raw = r#"{
        "model": {"id": "claude-sonnet-4-5-20250929", "display_name": "Sonnet 4.5"},
        "context_window": {"used_percentage": 38.0, "remaining_percentage": 62.0},
        "worktree": {"name": "wt-task-1479", "branch": "claude/task-1479"}
    }"#;
    let live = parse_claude_payload(raw);
    let out = format_combined(AIDA_SEGMENT, Some(&live), 200);
    assert_eq!(
        out,
        "Sonnet 4.5 · ctx:62% · claude/task-1479 · aida · aida-project · role:implementer · @TASK-1479 · q:2"
    );
}

#[test]
fn agy_representative_payload_combines_with_aida_segment() {
    let raw = r#"{
        "model": {"name": "gemini-3-pro"},
        "context": {"remaining_percent": 80.0},
        "activity": "editing",
        "vcs": {"branch": "main", "dirty": true}
    }"#;
    let live = parse_agy_payload(raw);
    let out = format_combined(AIDA_SEGMENT, Some(&live), 200);
    assert_eq!(
        out,
        "gemini-3-pro · ctx:80% · editing · main* · aida · aida-project · role:implementer · @TASK-1479 · q:2"
    );
}

#[test]
fn claude_payload_with_no_context_or_branch_yields_model_only_live_segment() {
    // Fresh session, first render — Claude Code sends `context_window` with
    // both usage fields `null` (schema-documented pre-first-API-call state)
    // and no active worktree session.
    let raw = r#"{"model": {"display_name": "Sonnet 4.5"}, "context_window": {"used_percentage": null, "remaining_percentage": null}}"#;
    let live = parse_claude_payload(raw);
    let out = format_combined(AIDA_SEGMENT, Some(&live), 200);
    assert_eq!(
        out,
        "Sonnet 4.5 · aida · aida-project · role:implementer · @TASK-1479 · q:2"
    );
}

#[test]
fn agy_payload_with_nothing_usable_falls_back_to_aida_segment_alone() {
    // Garbage/unknown-shape payload — every known field absent.
    let raw = r#"{"unexpected_field": {"nested": true}}"#;
    let live = parse_agy_payload(raw);
    assert_eq!(live, ClientLiveFields::default());
    let out = format_combined(AIDA_SEGMENT, Some(&live), 200);
    assert_eq!(out, AIDA_SEGMENT);
}

#[test]
fn malformed_stdin_for_either_client_never_panics_and_falls_back() {
    for raw in ["", "not json", "{ broken", "null", "[1,2,3]"] {
        let claude_live = parse_claude_payload(raw);
        let agy_live = parse_agy_payload(raw);
        assert_eq!(
            claude_live,
            ClientLiveFields::default(),
            "claude raw={raw:?}"
        );
        assert_eq!(agy_live, ClientLiveFields::default(), "agy raw={raw:?}");
        assert_eq!(
            format_combined(AIDA_SEGMENT, Some(&claude_live), 200),
            AIDA_SEGMENT
        );
        assert_eq!(
            format_combined(AIDA_SEGMENT, Some(&agy_live), 200),
            AIDA_SEGMENT
        );
    }
}

#[test]
fn narrow_terminal_sheds_activity_for_both_clients_but_keeps_model_and_context() {
    let claude_live = parse_claude_payload(
        r#"{"model": {"display_name": "Sonnet 4.5"}, "context_window": {"remaining_percentage": 62.0}}"#,
    );
    let agy_live = parse_agy_payload(
        r#"{"model": "gemini-3-pro", "context": {"remaining_percent": 80.0}, "activity": "thinking"}"#,
    );
    // 55 columns: below NARROW_TERMINAL_COLUMNS (60) so shedding kicks in,
    // but wide enough that the shed (activity-free) segments still fit.
    // Claude has no activity to shed in the first place (schema has none);
    // Agy's `activity` sheds.
    let claude_out = format_combined("aida · role:implementer", Some(&claude_live), 55);
    assert_eq!(claude_out, "Sonnet 4.5 · ctx:62% · aida · role:implementer");
    let agy_out = format_combined("aida · role:implementer", Some(&agy_live), 55);
    assert_eq!(agy_out, "gemini-3-pro · ctx:80% · aida · role:implementer");
    assert!(!agy_out.contains("thinking"), "{agy_out:?}");
}

#[test]
fn no_client_flag_path_never_touches_live_fields() {
    // Mirrors handle_statusline_command's `client: None` branch: the AIDA
    // segment is used verbatim, with no adapter/formatter involvement.
    assert_eq!(format_combined(AIDA_SEGMENT, None, 80), AIDA_SEGMENT);
}
