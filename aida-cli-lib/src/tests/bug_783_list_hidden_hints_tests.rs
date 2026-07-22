//! BUG-783: the default `aida list` view must not footer the archived /
//! deferred hidden-count nudges.
//!
//! The operator ran `aida list open` and got two trailing lines on EVERY
//! invocation — "(138 archived hidden — pass --all or --archived to see them)"
//! and "(41 deferred hidden — …)". On an explicit open-work request those are
//! pure noise: the answer to "show me open work" is the open work.
//!
//! These tests pin the contract:
//!
//! - the default view emits NO archived/deferred footer,
//! - the lens hints it does NOT own (closed history, accepted decisions) still
//!   print, because those explain why a spec the user knows exists is absent,
//! - `[list] show_hidden_hints = true` brings the tier nudges back,
//! - and the explicit `--archived` / `--deferred` / `--all` views are
//!   structurally unaffected (they clear the default lens, so both tier counts
//!   arrive as 0 and nothing is suppressed that was ever going to render).
//!
//! trace:BUG-783 | ai:claude

use super::*;

fn write_config(root: &std::path::Path, body: &str) {
    let config_dir = root.join(".aida");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), body).unwrap();
}

// --- The config knob. ---

/// No config file at all → suppressed. The safe default is quiet.
// trace:BUG-783 | ai:claude
#[test]
fn hidden_hints_default_off_when_no_config() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(!read_list_show_hidden_hints(tmp.path()));
}

/// Config file present but the key absent → suppressed.
// trace:BUG-783 | ai:claude
#[test]
fn hidden_hints_default_off_when_key_absent() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[archive]\nauto_after_days = 30\n");
    assert!(!read_list_show_hidden_hints(tmp.path()));
}

/// The explicit opt-in.
// trace:BUG-783 | ai:claude
#[test]
fn hidden_hints_opt_in_reads_true() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[list]\nshow_hidden_hints = true\n");
    assert!(read_list_show_hidden_hints(tmp.path()));
}

/// An explicit `false` is the same as the default (so writing the key out with
/// its default value is a no-op, not a surprise).
// trace:BUG-783 | ai:claude
#[test]
fn hidden_hints_explicit_false_stays_off() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[list]\nshow_hidden_hints = false\n");
    assert!(!read_list_show_hidden_hints(tmp.path()));
}

/// Garbage TOML must not break `aida list` — fall back to the quiet default.
// trace:BUG-783 | ai:claude
#[test]
fn hidden_hints_unparseable_config_falls_back_to_off() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[list\nshow_hidden_hints = ");
    assert!(!read_list_show_hidden_hints(tmp.path()));
}

// --- The footer itself. ---

/// THE acceptance criterion: the default open view, with archived and deferred
/// rows genuinely hidden, prints nothing about either tier.
// trace:BUG-783 | ai:claude
#[test]
fn default_view_prints_no_archived_or_deferred_footer() {
    let lines = list_hidden_hint_lines(false, 0, 138, 41, 0);
    assert!(
        lines.is_empty(),
        "default view must be quiet, got: {lines:?}"
    );
}

/// The lens hints are a different family and keep printing: they explain why a
/// spec the user KNOWS exists is missing from the rows they just asked for.
// trace:BUG-783 trace:STORY-723 trace:BUG-781 | ai:claude
#[test]
fn default_view_keeps_closed_and_accepted_decision_lens_hints() {
    let lines = list_hidden_hint_lines(false, 12, 138, 41, 3);
    assert_eq!(lines.len(), 2, "got: {lines:?}");
    assert!(lines[0].contains("12 closed hidden"));
    assert!(lines[1].contains("3 accepted decisions hidden"));
    assert!(!lines.iter().any(|l| l.contains("archived hidden")));
    assert!(!lines.iter().any(|l| l.contains("deferred hidden")));
}

/// With the knob on, the pre-BUG-783 footer comes back verbatim, in the same
/// order, with the same flag names.
// trace:BUG-783 | ai:claude
#[test]
fn opt_in_restores_the_archived_and_deferred_footer() {
    let lines = list_hidden_hint_lines(true, 0, 138, 41, 0);
    assert_eq!(lines.len(), 2, "got: {lines:?}");
    assert!(lines[0].contains("138 archived hidden — pass --all or --archived to see them"));
    assert!(lines[1].contains("41 deferred hidden — pass --all or --deferred to see them"));
}

/// Render order with everything on: closed, archived, deferred, accepted.
// trace:BUG-783 | ai:claude
#[test]
fn opt_in_render_order_is_stable() {
    let lines = list_hidden_hint_lines(true, 12, 138, 41, 3);
    assert_eq!(lines.len(), 4, "got: {lines:?}");
    assert!(lines[0].contains("closed hidden"));
    assert!(lines[1].contains("archived hidden"));
    assert!(lines[2].contains("deferred hidden"));
    assert!(lines[3].contains("accepted decision"));
}

/// `--archived` / `--deferred` / `--all` clear the default lens: the handler
/// hands both tier counts down as 0 (nothing is hidden), so the footer is empty
/// whichever way the knob is set — the flags' own output is untouched.
// trace:BUG-783 | ai:claude
#[test]
fn explicit_tier_views_are_unaffected_by_the_suppression() {
    for opted_in in [false, true] {
        let lines = list_hidden_hint_lines(opted_in, 0, 0, 0, 0);
        assert!(
            lines.is_empty(),
            "explicit tier view must have an empty footer (opted_in={opted_in}), got: {lines:?}"
        );
    }
}

/// A zero count never renders a "(0 … hidden)" line on either axis.
// trace:BUG-783 | ai:claude
#[test]
fn zero_counts_render_nothing() {
    assert!(list_hidden_hint_lines(true, 0, 0, 0, 0).is_empty());
    assert!(list_hidden_hint_lines(false, 0, 0, 0, 0).is_empty());
}

/// Singular/plural on the accepted-decision line survives the refactor.
// trace:BUG-783 trace:BUG-781 | ai:claude
#[test]
fn accepted_decision_line_pluralizes() {
    let one = list_hidden_hint_lines(false, 0, 0, 0, 1);
    assert!(one[0].contains("1 accepted decision hidden"), "{one:?}");
    let many = list_hidden_hint_lines(false, 0, 0, 0, 2);
    assert!(many[0].contains("2 accepted decisions hidden"), "{many:?}");
}

/// The BUG-684 empty-state signpost still depends on the real tier counts — a
/// filtered-empty project must not be told to file its first spec. The handler
/// therefore keeps computing both counts when the row set is empty, even with
/// the footer suppressed; this pins the decision function that consumes them.
// trace:BUG-783 trace:BUG-684 | ai:claude
#[test]
fn empty_state_signpost_still_respects_hidden_tier_counts() {
    assert!(empty_list_hint_line(0, 0, 0, 0).is_some());
    assert!(empty_list_hint_line(0, 138, 0, 0).is_none());
    assert!(empty_list_hint_line(0, 0, 41, 0).is_none());
}
