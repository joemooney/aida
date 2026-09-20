// BUG-1295: the orchestrator's phase-3 auto-rebase driver used to recover a
// typed failure cause (`FailureKind::StaleBaseConflict` /
// `StaleBaseRefused`) by substring-matching the prose a sibling `aida pr
// rebase` subprocess printed. Six independently-worded bail sites all had
// to keep agreeing on wording for that to keep working, and nothing
// enforced it — a reword silently downgraded the failure to an untyped one
// and dropped the `--interactive` recovery recipe attached to the typed
// arm. These tests pin the fix: classification now rides on the
// subprocess's EXIT CODE (`pr_rebase::REBASE_EXIT_CODE_*`), so a message
// can be reworded to anything — even text sharing zero substrings with the
// old triggers — without changing the classification.
// trace:BUG-1295 | ai:claude

use super::*;

/// Deliberately shares NO substring with any of the six old prose triggers
/// ("force-push refused", "Force-pushing would DROP", "rebase aborted due
/// to conflicts", "rebase hit" + "conflict", "rebase hit conflicts and was
/// aborted — resolve manually"). If classification ever regresses back to
/// matching text, a message like this is exactly what would silently fall
/// through to the untyped arm.
const REWORDED_CONFLICT_MESSAGE: &str =
    "the automatic reconciliation could not complete unattended";
const REWORDED_REFUSED_MESSAGE: &str =
    "declining to publish — origin moved in a way we cannot safely overwrite";

fn assert_shares_no_old_trigger_substring(message: &str) {
    for banned in [
        "force-push refused",
        "Force-pushing would DROP",
        "rebase aborted due to conflicts",
        "rebase hit",
        "conflict",
        "aborted",
    ] {
        assert!(
            !message
                .to_ascii_lowercase()
                .contains(&banned.to_ascii_lowercase()),
            "test fixture message {message:?} accidentally contains the old \
             trigger substring {banned:?} — pick different wording so this \
             test actually proves classification is text-independent"
        );
    }
}

// trace:BUG-1295 | ai:claude
#[test]
fn reworded_conflict_message_still_classifies_as_conflict() {
    assert_shares_no_old_trigger_substring(REWORDED_CONFLICT_MESSAGE);

    // Producer side: the bail site builds the error from the reworded prose.
    let err = pr_rebase::rebase_conflict_error(REWORDED_CONFLICT_MESSAGE);
    // `Display` still carries the human message unchanged.
    assert_eq!(format!("{err}"), REWORDED_CONFLICT_MESSAGE);
    // `main_entry` picks the exit code from the typed signal, not the text.
    let code = exit_code_for_error(&err);
    assert_eq!(code, pr_rebase::REBASE_EXIT_CODE_CONFLICT);

    // Consumer side: `attempt_phase3_auto_rebase` classifies the finished
    // subprocess by that exit code alone.
    assert_eq!(
        classify_rebase_subprocess_exit(Some(code)),
        Some(auto_complete::FailureKind::StaleBaseConflict),
        "a reworded conflict message must still classify as StaleBaseConflict \
         (and therefore still carry the `--interactive` recovery recipe) — \
         if this fails, classification has regressed to matching prose again"
    );
}

// trace:BUG-1295 | ai:claude
#[test]
fn reworded_refused_message_still_classifies_as_refused() {
    assert_shares_no_old_trigger_substring(REWORDED_REFUSED_MESSAGE);

    let err = pr_rebase::rebase_refused_error(REWORDED_REFUSED_MESSAGE);
    assert_eq!(format!("{err}"), REWORDED_REFUSED_MESSAGE);
    let code = exit_code_for_error(&err);
    assert_eq!(code, pr_rebase::REBASE_EXIT_CODE_REFUSED);

    assert_eq!(
        classify_rebase_subprocess_exit(Some(code)),
        Some(auto_complete::FailureKind::StaleBaseRefused),
        "a reworded force-push-refusal message must still classify as \
         StaleBaseRefused — a safety refusal must never silently downgrade \
         to a generic failure"
    );
}

// trace:BUG-1295 | ai:claude
#[test]
fn conflict_and_refused_exit_codes_are_distinct() {
    // The two typed classes must map to different codes, and neither may
    // collide with the generic error code (1) every other failure uses.
    assert_ne!(
        pr_rebase::REBASE_EXIT_CODE_CONFLICT,
        pr_rebase::REBASE_EXIT_CODE_REFUSED
    );
    assert_ne!(pr_rebase::REBASE_EXIT_CODE_CONFLICT, 1);
    assert_ne!(pr_rebase::REBASE_EXIT_CODE_REFUSED, 1);
}

// trace:BUG-1295 | ai:claude
#[test]
fn unrecognized_exit_codes_fall_back_to_untyped() {
    // A generic anyhow error (exit 1), a clean exit (0), an unrelated
    // nonzero code, and a signal-death `None` must all fall through to the
    // untyped arm — only the two reserved codes classify.
    assert_eq!(classify_rebase_subprocess_exit(Some(1)), None);
    assert_eq!(classify_rebase_subprocess_exit(Some(0)), None);
    assert_eq!(classify_rebase_subprocess_exit(Some(42)), None);
    assert_eq!(classify_rebase_subprocess_exit(None), None);
}

// trace:BUG-1295 | ai:claude
#[test]
fn non_rebase_errors_keep_the_historical_exit_code() {
    // Any ordinary `anyhow::bail!`/`anyhow!` error elsewhere in the CLI must
    // keep exiting 1 — only the two typed rebase-failure sentinels change
    // the exit code.
    let err = anyhow::anyhow!("some unrelated command failed");
    assert_eq!(exit_code_for_error(&err), 1);
}
