//! BUG-776 (related UX issue): `aida session end <id>` without `--yes` must
//! never look like it ran while leaving the lease behind. With no TTY there is
//! nobody to answer `Continue? [y/N]`, so the command fails loudly (non-zero)
//! and says the lease survives — a silent no-op is what orphans leases.
// trace:BUG-776 | ai:claude

use super::{session_end_confirm_action, session_end_confirm_refusal, SessionEndConfirm};
use std::path::Path;

#[test]
fn non_interactive_without_yes_refuses_instead_of_no_op() {
    assert_eq!(
        session_end_confirm_action(false, false, false),
        SessionEndConfirm::RefuseNonInteractive
    );
}

#[test]
fn yes_or_force_skips_confirmation_even_headless() {
    assert_eq!(
        session_end_confirm_action(true, false, false),
        SessionEndConfirm::Skip
    );
    assert_eq!(
        session_end_confirm_action(false, true, false),
        SessionEndConfirm::Skip
    );
    // A TTY with a skip flag still skips — the flag is the answer.
    assert_eq!(
        session_end_confirm_action(true, false, true),
        SessionEndConfirm::Skip
    );
}

#[test]
fn interactive_without_yes_still_prompts() {
    assert_eq!(
        session_end_confirm_action(false, false, true),
        SessionEndConfirm::Prompt
    );
}

#[test]
fn refusal_message_says_the_session_was_not_ended_and_names_the_lease() {
    let msg = session_end_confirm_refusal(
        "019f87a59abb",
        Path::new("/p/.aida/sessions/019f87a59abb.toml"),
    );
    assert!(
        msg.contains("was NOT ended"),
        "refusal must be unambiguous that nothing happened: {msg}"
    );
    assert!(
        msg.contains("still held"),
        "refusal must say the lease survives: {msg}"
    );
    assert!(
        msg.contains("/p/.aida/sessions/019f87a59abb.toml"),
        "refusal must name the surviving lease file: {msg}"
    );
    assert!(
        msg.contains("--yes"),
        "refusal must name the flag that unblocks it: {msg}"
    );
}
