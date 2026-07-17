use super::*;
use crate::cli::{Cli, Command, FindingsCommand};
use clap::Parser;

fn failure(phase: &str, detail: &str, hint: Option<&str>) -> aida_core::FailureReason {
    aida_core::FailureReason {
        phase: phase.to_string(),
        phase_index: 2,
        kind: "ci-red".to_string(),
        detail: detail.to_string(),
        recovery_hint: hint.map(|h| h.to_string()),
        shelved_by: None,
        shelved_at: chrono::Utc::now(),
    }
}

/// FIX 2: `aida why` / `aida status <spec>` inline the phase + detail + hint
/// the orchestrator recorded — the same three fields `aida findings list`
/// renders — instead of only redirecting to `aida findings list`.
#[test]
fn failure_reason_lines_inline_phase_detail_and_hint() {
    let fr = failure(
        "ci",
        "CI is red on the PR (2 tests failing)",
        Some("re-run CI / fix the failing test, then re-queue"),
    );
    let lines = failure_reason_lines(&fr);
    assert_eq!(lines.len(), 2, "phase+detail line plus a hint line");
    assert!(lines[0].contains("ci"), "phase named: {:?}", lines[0]);
    assert!(
        lines[0].contains("CI is red on the PR"),
        "detail inlined: {:?}",
        lines[0]
    );
    assert!(
        lines[1].contains("re-run CI / fix the failing test"),
        "recovery hint inlined: {:?}",
        lines[1]
    );
    // The whole rendering must not redirect away — it answers in place.
    assert!(
        !lines.iter().any(|l| l.contains("aida findings list")),
        "inlined reason must not redirect: {lines:?}"
    );
}

/// FIX 2: a failure with no recovery hint still inlines phase + detail (no
/// empty trailing hint line).
#[test]
fn failure_reason_lines_without_hint_is_one_line() {
    let fr = failure("build", "the workspace failed to compile", None);
    let lines = failure_reason_lines(&fr);
    assert_eq!(lines.len(), 1, "no hint → single line");
    assert!(lines[0].contains("build") && lines[0].contains("failed to compile"));
}

/// FIX 3: a Completed (terminal) spec carrying a dormant lease is framed as
/// CLEANUP — never "the In-Progress flag is orphaned", which contradicts the
/// Completed badge and reads as "is it done or not?".
#[test]
fn terminal_stale_lease_frames_as_cleanup_not_doubt() {
    let suffix = stale_cleanup_suffix("a1b2c3d4");
    assert!(
        suffix.contains("still attached"),
        "frames as housekeeping: {suffix}"
    );
    assert!(
        suffix.contains("aida session end a1b2c3d4"),
        "names the clear command: {suffix}"
    );
    assert!(
        !suffix.to_ascii_lowercase().contains("orphaned"),
        "no 'orphaned' framing on a terminal spec: {suffix}"
    );
    assert!(
        !suffix.contains("In-Progress"),
        "no 'In-Progress flag' contradiction on a Completed spec: {suffix}"
    );
}

/// FIX 3 regression guard: a NON-terminal STALE spec keeps the orphaned
/// In-Progress warning (the honest signal there).
#[test]
fn non_terminal_stale_lease_keeps_orphaned_warning() {
    let line = stale_orphaned_line("no live process", "3h 12m");
    assert!(line.contains("orphaned") && line.contains("In-Progress flag"));
}

/// FIX 8: bare `aida findings` parses (it used to be a hard clap error) and
/// resolves to the same view as `aida findings list`.
#[test]
fn bare_findings_parses_to_optional_subcommand() {
    let bare = Cli::try_parse_from(["aida", "findings"]).expect("bare findings must parse");
    assert!(
        matches!(bare.command, Command::Findings { cmd: None }),
        "bare `aida findings` defaults to no subcommand (→ list)"
    );
    let listed =
        Cli::try_parse_from(["aida", "findings", "list"]).expect("findings list must parse");
    assert!(matches!(
        listed.command,
        Command::Findings {
            cmd: Some(FindingsCommand::List { .. })
        }
    ));
}
