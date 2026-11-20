// trace:TASK-3780 | ai:codex
use super::*;

fn allow(emails: &[&str]) -> Vec<String> {
    emails.iter().map(|s| s.to_string()).collect()
}

#[test]
fn no_allowlist_means_no_findings() {
    assert!(identity_hygiene_findings(&[], Some("anyone@anywhere.com"), None).is_empty());
}

#[test]
fn allowlisted_email_and_wired_hook_are_silent() {
    let hook = crate::identity_gate::identity_pre_push_hook_script();
    let findings = identity_hygiene_findings(
        &allow(&["good@example.org"]),
        Some("good@example.org"),
        Some(&hook),
    );
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn the_gate_embedded_in_the_mirror_hook_counts_as_wired() {
    let hook = crate::remote_create::mirror_pre_push_hook_script();
    let findings = identity_hygiene_findings(
        &allow(&["good@example.org"]),
        Some("good@example.org"),
        Some(&hook),
    );
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn non_allowlisted_email_and_missing_hook_are_both_reported() {
    let findings = identity_hygiene_findings(
        &allow(&["good@example.org"]),
        Some("joe@work.example.com"),
        None,
    );
    let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "git-user-email-not-allowlisted",
            "identity-pre-push-hook-missing"
        ]
    );
    assert!(findings.iter().all(|f| !f.safe_heal));
}

#[test]
fn unset_user_email_is_reported() {
    let findings = identity_hygiene_findings(&allow(&["good@example.org"]), None, None);
    assert_eq!(findings[0].id, "git-user-email-not-allowlisted");
    assert!(findings[0].summary.contains("unset"));
}

#[test]
fn a_custom_hook_running_the_gate_counts_as_wired() {
    let hook = "#!/bin/sh\nrefs=$(cat)\nprintf '%s\\n' \"$refs\" | \
                    aida identity check-push \"$1\" || exit 1\n";
    let findings = identity_hygiene_findings(
        &allow(&["good@example.org"]),
        Some("good@example.org"),
        Some(hook),
    );
    assert!(findings.is_empty());
}
