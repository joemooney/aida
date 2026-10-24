use super::*;

fn direct_human_refusal() -> anyhow::Error {
    anyhow::anyhow!(
        "role grants require a human at an interactive TTY; run `aida role enter` from your terminal"
    )
}

#[test]
fn grant_without_agent_identity_names_the_identity_variables() {
    let msg = format!(
        "{:#}",
        review_record_attribution_refusal(direct_human_refusal(), Some("advisor"))
    );
    assert!(msg.contains("seat grant was found"), "{msg}");
    assert!(msg.contains("no agent identity is set"), "{msg}");
    assert!(msg.contains("AIDA_AGENT_TYPE"), "{msg}");
    assert!(msg.contains("AIDA_AGENT_NAME"), "{msg}");
    assert!(!msg.contains("aida role enter"), "{msg}");
    // Agent output mode turns the first backticked `aida ...` command in
    // an error into a `help:` retry line; re-running the same command
    // without an identity would fail the same way, so none may be offered.
    assert_eq!(crate::extract_aida_suggestion(&msg), None, "{msg}");
    assert!(
        !msg.contains("BUG-"),
        "internal IDs must stay out of user text: {msg}"
    );
}

#[test]
fn no_grant_keeps_the_existing_role_enter_guidance() {
    let msg = format!(
        "{:#}",
        review_record_attribution_refusal(direct_human_refusal(), None)
    );
    assert_eq!(
        msg,
        "aida review record attribution fallback: role grants require a human at an \
         interactive TTY; run `aida role enter` from your terminal"
    );
}
