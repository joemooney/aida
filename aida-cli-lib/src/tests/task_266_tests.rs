use super::{
    auto_complete_queue_add_args, auto_failure_bug_matches, increment_auto_failure_attempts,
};

fn failure_bug(status: aida_core::RequirementStatus, kind: &str) -> aida_core::Requirement {
    let mut req = aida_core::Requirement::new(
        "auto-complete failure: phase 3 (reviewer) on STORY-943".to_string(),
        format!("Failure kind: `{kind}`\n\n## Recurrence\n\nAttempts: 1\nLatest recurrence: old"),
    );
    req.spec_id = Some("BUG-854".to_string());
    req.req_type = aida_core::RequirementType::Bug;
    req.status = status;
    req.tags.insert("auto-complete".to_string());
    req.tags.insert("failure-3".to_string());
    req.tags.insert("auto-drafted".to_string());
    req.tags.insert("reviewer".to_string());
    req
}

// TASK-1564 retired the `aida add` subprocess that filed a Draft BUG for a
// phase failure — the record is now a comment on the parent spec — so the
// `ID:` stdout parser those three tests covered no longer exists. The dedupe
// and recurrence coverage below still applies: it guards the legacy
// auto-drafted-BUG absorb path, which TASK-1564 deliberately left in place.

#[test]
fn auto_complete_preflight_queue_add_disables_cwd_scope_derivation() {
    // BUG-352: `aida queue work <SPEC> --auto-complete` auto-queues
    // an explicit spec as standalone work. It must not inherit a
    // stale/misattributed cwd lease via queue-add's normal scope
    // derivation path.
    assert_eq!(
        auto_complete_queue_add_args("TASK-488"),
        vec![
            "queue",
            "add",
            "TASK-488",
            "--for",
            "implementer",
            "--no-scope"
        ]
    );
}

#[test]
fn auto_failure_dedup_matches_open_draft_by_spec_phase_and_kind() {
    let req = failure_bug(aida_core::RequirementStatus::Draft, "request-changes");
    assert!(auto_failure_bug_matches(
        &req,
        "STORY-943",
        3,
        "reviewer",
        "request-changes"
    ));
    assert!(
        !auto_failure_bug_matches(&req, "STORY-944", 3, "reviewer", "request-changes"),
        "different source spec must not absorb"
    );
    assert!(
        !auto_failure_bug_matches(&req, "STORY-943", 2, "ci", "request-changes"),
        "different phase must file a second record"
    );
    assert!(
        !auto_failure_bug_matches(&req, "STORY-943", 3, "reviewer", "ci-red"),
        "different failure class must file a second record"
    );
}

#[test]
fn auto_failure_dedup_window_is_draft_or_approved_only() {
    let approved = failure_bug(aida_core::RequirementStatus::Approved, "request-changes");
    assert!(auto_failure_bug_matches(
        &approved,
        "STORY-943",
        3,
        "reviewer",
        "request-changes"
    ));

    let rejected = failure_bug(aida_core::RequirementStatus::Rejected, "request-changes");
    assert!(
        !auto_failure_bug_matches(&rejected, "STORY-943", 3, "reviewer", "request-changes"),
        "rejected records stop absorbing, so the next failure files fresh"
    );

    let completed = failure_bug(aida_core::RequirementStatus::Completed, "request-changes");
    assert!(
        !auto_failure_bug_matches(&completed, "STORY-943", 3, "reviewer", "request-changes"),
        "completed records stop absorbing, so the next failure files fresh"
    );
}

#[test]
fn recurring_auto_failure_increments_attempts_and_latest_timestamp() {
    let once =
        "Failure kind: `request-changes`\n\n## Recurrence\n\nAttempts: 1\nLatest recurrence: old";
    let twice = increment_auto_failure_attempts(once, "2026-09-06T12:00:00Z");
    assert!(twice.contains("Attempts: 2"));
    assert!(twice.contains("Latest recurrence: 2026-09-06T12:00:00Z"));

    let thrice = increment_auto_failure_attempts(&twice, "2026-09-06T12:05:00Z");
    assert!(thrice.contains("Attempts: 3"));
    assert!(thrice.contains("Latest recurrence: 2026-09-06T12:05:00Z"));
}

#[test]
fn recurring_auto_failure_adds_counter_to_legacy_draft() {
    let legacy = "Failure kind: `request-changes`";
    let updated = increment_auto_failure_attempts(legacy, "2026-09-06T12:00:00Z");
    assert!(updated.contains("## Recurrence"));
    assert!(updated.contains("Attempts: 2"));
    assert!(updated.contains("Latest recurrence: 2026-09-06T12:00:00Z"));
}
