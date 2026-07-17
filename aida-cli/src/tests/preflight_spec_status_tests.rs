use super::{
    dup_pickup_recheck, effective_force_claim_for_session_start, preflight_spec_status,
    preflight_spec_status_review_aware, session_start_status_bump_preconditions_met,
    DupPickupDecision, PreflightDecision, RequirementStatus,
};
use tempfile::TempDir;

// TASK-619: cross-machine duplicate-pickup re-check. trace:TASK-619 |
// ai:claude
#[test]
fn dup_pickup_refuses_when_approved_became_in_progress_under_us() {
    // Headline case: we planned a pickup on an Approved spec, pulled, and
    // it's now InProgress — another machine grabbed it. Refuse.
    let d = dup_pickup_recheck(
        "TASK-619",
        Some(&RequirementStatus::Approved),
        Some(&RequirementStatus::InProgress),
        false, // force_claim
        false, // orchestrator_corroborated
        false, // is_review
    );
    match d {
        DupPickupDecision::Refuse(m) => {
            assert!(m.contains("TASK-619"), "{m}");
            assert!(m.contains("force-claim"), "{m}");
            assert!(m.contains("machine"), "{m}");
        }
        other => panic!("expected Refuse, got {other:?}"),
    }
}

#[test]
fn dup_pickup_refuses_when_planned_became_done_or_completed() {
    // Shipped-elsewhere variants also count as claimed-under-us.
    assert!(matches!(
        dup_pickup_recheck(
            "STORY-1",
            Some(&RequirementStatus::Planned),
            Some(&RequirementStatus::Done),
            false,
            false,
            false,
        ),
        DupPickupDecision::Refuse(_)
    ));
    assert!(matches!(
        dup_pickup_recheck(
            "STORY-1",
            Some(&RequirementStatus::Approved),
            Some(&RequirementStatus::Completed),
            false,
            false,
            false,
        ),
        DupPickupDecision::Refuse(_)
    ));
}

#[test]
fn dup_pickup_proceeds_when_status_unchanged() {
    // No transition under us → nothing to refuse; the normal preflight runs.
    assert_eq!(
        dup_pickup_recheck(
            "TASK-619",
            Some(&RequirementStatus::Approved),
            Some(&RequirementStatus::Approved),
            false,
            false,
            false,
        ),
        DupPickupDecision::Proceed
    );
}

#[test]
fn dup_pickup_proceeds_when_already_in_progress_at_plan_time() {
    // A resume/force flow: the spec was already InProgress when we planned,
    // so a stable InProgress is NOT a cross-machine grab — leave it to the
    // existing preflight (and --force-claim / --resume), don't double-refuse.
    assert_eq!(
        dup_pickup_recheck(
            "TASK-619",
            Some(&RequirementStatus::InProgress),
            Some(&RequirementStatus::InProgress),
            false,
            false,
            false,
        ),
        DupPickupDecision::Proceed
    );
}

#[test]
fn dup_pickup_bypassed_by_force_claim_orchestrator_and_review() {
    // The deliberate-takeover / orchestrator-child / review escapes all
    // proceed even on an Approved → InProgress transition.
    for (force, orch, review) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        assert_eq!(
            dup_pickup_recheck(
                "TASK-619",
                Some(&RequirementStatus::Approved),
                Some(&RequirementStatus::InProgress),
                force,
                orch,
                review,
            ),
            DupPickupDecision::Proceed,
            "force={force} orch={orch} review={review}"
        );
    }
}

#[test]
fn dup_pickup_proceeds_when_status_unknown() {
    // Scope isn't a plain spec (cluster / PR) → no status to compare → no-op.
    assert_eq!(
        dup_pickup_recheck("EPIC-20", None, None, false, false, false),
        DupPickupDecision::Proceed
    );
    assert_eq!(
        dup_pickup_recheck(
            "TASK-1",
            Some(&RequirementStatus::Approved),
            None,
            false,
            false,
            false,
        ),
        DupPickupDecision::Proceed
    );
}

// BUG-436: a review session must be allowed to start against a Done /
// Completed spec (reviewing a PR is not implementing the spec); a non-review
// (implementer) session must still be refused (the BUG-379 re-implement
// guard stays intact). trace:BUG-436 | ai:claude
#[test]
fn review_session_allowed_on_done_spec() {
    assert_eq!(
        preflight_spec_status_review_aware(
            "TASK-639",
            Some(&RequirementStatus::Done),
            false,
            true, // is_review
        ),
        PreflightDecision::Allow,
        "reviewing a Done spec's PR is the normal pre-review state"
    );
}

#[test]
fn review_session_allowed_on_completed_spec() {
    assert_eq!(
        preflight_spec_status_review_aware(
            "TASK-639",
            Some(&RequirementStatus::Completed),
            false,
            true,
        ),
        PreflightDecision::Allow,
    );
}

#[test]
fn implementer_session_still_refused_on_done_spec() {
    // The re-implement guard must NOT regress for non-review sessions.
    assert!(matches!(
        preflight_spec_status_review_aware(
            "TASK-639",
            Some(&RequirementStatus::Done),
            false,
            false, // not a review
        ),
        PreflightDecision::Refuse(_)
    ));
}

#[test]
fn review_session_does_not_loosen_other_states() {
    // is_review only exempts Done/Completed; an Approved spec still bumps,
    // a Draft still refuses — the wrapper delegates for everything else.
    assert_eq!(
        preflight_spec_status_review_aware(
            "TASK-1",
            Some(&RequirementStatus::Approved),
            false,
            true,
        ),
        PreflightDecision::AllowAndBump,
    );
    assert!(matches!(
        preflight_spec_status_review_aware("TASK-1", Some(&RequirementStatus::Draft), false, true,),
        PreflightDecision::Refuse(_)
    ));
}

// trace:TASK-1-108 | ai:claude
#[test]
fn effective_force_claim_is_true_when_explicit() {
    // Operator passed --force-claim → honor it regardless of
    // orchestrator context.
    assert!(effective_force_claim_for_session_start(true, false));
    assert!(effective_force_claim_for_session_start(true, true));
}

// trace:TASK-1-108 | ai:claude
#[test]
fn effective_force_claim_is_true_when_orchestrator_corroborated() {
    // No --force-claim, but we're the orchestrator's phase-1 child.
    // The parent already bumped status — claim without bouncing.
    assert!(effective_force_claim_for_session_start(false, true));
}

// trace:TASK-1-108 | ai:claude
#[test]
fn effective_force_claim_is_false_in_interactive_session() {
    // Bare `aida session start` (no orchestrator, no --force-claim).
    // The InProgress-without-lease gate stays in place for safety —
    // a stale state from another machine could otherwise be silently
    // overwritten.
    assert!(!effective_force_claim_for_session_start(false, false));
}

#[test]
fn no_spec_match_just_allows() {
    // Scope isn't a SPEC-ID (path glob, EPIC name, etc.) — gate is a no-op.
    assert_eq!(
        preflight_spec_status("src/scaffolding/**", None, false),
        PreflightDecision::Allow
    );
    assert_eq!(
        preflight_spec_status("feature:auth", None, true),
        PreflightDecision::Allow
    );
}

#[test]
fn approved_bumps_to_in_progress() {
    // The headline case: a fresh Approved spec auto-bumps.
    assert_eq!(
        preflight_spec_status("BUG-379", Some(&RequirementStatus::Approved), false),
        PreflightDecision::AllowAndBump
    );
    // --force-claim doesn't change Approved's behavior.
    assert_eq!(
        preflight_spec_status("BUG-379", Some(&RequirementStatus::Approved), true),
        PreflightDecision::AllowAndBump
    );
}

#[test]
fn planned_allows_without_bumping() {
    assert_eq!(
        preflight_spec_status("STORY-99", Some(&RequirementStatus::Planned), false),
        PreflightDecision::Allow
    );
}

#[test]
fn done_refuses() {
    let d = preflight_spec_status("STORY-86", Some(&RequirementStatus::Done), false);
    match d {
        PreflightDecision::Refuse(m) => {
            assert!(m.contains("STORY-86"), "{m}");
            assert!(m.contains("Done") || m.contains("shipped"), "{m}");
            // STORY-729: the refuse names the reopen command, not just
            // "reopen the spec first".
            assert!(m.contains("aida edit STORY-86 --status approved"), "{m}");
        }
        other => panic!("expected Refuse, got {:?}", other),
    }
    // --force-claim does NOT override Done.
    assert!(matches!(
        preflight_spec_status("STORY-86", Some(&RequirementStatus::Done), true),
        PreflightDecision::Refuse(_)
    ));
}

#[test]
fn completed_refuses_even_with_force_claim() {
    assert!(matches!(
        preflight_spec_status("STORY-86", Some(&RequirementStatus::Completed), false),
        PreflightDecision::Refuse(_)
    ));
    assert!(matches!(
        preflight_spec_status("STORY-86", Some(&RequirementStatus::Completed), true),
        PreflightDecision::Refuse(_)
    ));
}

#[test]
fn rejected_refuses() {
    let d = preflight_spec_status("TASK-1", Some(&RequirementStatus::Rejected), false);
    match d {
        PreflightDecision::Refuse(m) => {
            assert!(m.contains("Rejected"), "{m}");
            // STORY-729: the refuse now names the reopen command, where it
            // previously named no reopen/override path at all.
            assert!(m.contains("aida edit TASK-1 --status approved"), "{m}");
        }
        other => panic!("expected Refuse, got {:?}", other),
    }
    // No force-claim escape.
    assert!(matches!(
        preflight_spec_status("TASK-1", Some(&RequirementStatus::Rejected), true),
        PreflightDecision::Refuse(_)
    ));
}

#[test]
fn draft_refuses() {
    let d = preflight_spec_status("BUG-2", Some(&RequirementStatus::Draft), false);
    match d {
        PreflightDecision::Refuse(m) => {
            assert!(m.contains("Draft"), "{m}");
            assert!(m.contains("approved"), "{m}");
        }
        other => panic!("expected Refuse, got {:?}", other),
    }
    // No force-claim escape — Draft means "not ready", not "ambiguous".
    assert!(matches!(
        preflight_spec_status("BUG-2", Some(&RequirementStatus::Draft), true),
        PreflightDecision::Refuse(_)
    ));
}

#[test]
fn in_progress_without_force_claim_refuses() {
    let d = preflight_spec_status("BUG-379", Some(&RequirementStatus::InProgress), false);
    match d {
        PreflightDecision::Refuse(m) => {
            assert!(m.contains("In Progress") || m.contains("InProgress"), "{m}");
            assert!(m.contains("--force-claim"), "{m}");
        }
        other => panic!("expected Refuse, got {:?}", other),
    }
}

#[test]
fn in_progress_with_force_claim_warns_and_allows() {
    // TASK-1-108: warning is mechanism-neutral (works for both
    // explicit --force-claim AND orchestrator-corroborated auto-
    // claim). Assert the spec ID + the claim verb survived, not
    // the literal --force-claim string.
    match preflight_spec_status("BUG-379", Some(&RequirementStatus::InProgress), true) {
        PreflightDecision::AllowWithWarning(m) => {
            assert!(m.contains("BUG-379"), "{m}");
            assert!(m.contains("claim"), "{m}");
        }
        other => panic!("expected AllowWithWarning, got {:?}", other),
    }
}

#[test]
fn needs_attention_without_force_claim_refuses() {
    let d = preflight_spec_status("BUG-9", Some(&RequirementStatus::NeedsAttention), false);
    match d {
        PreflightDecision::Refuse(m) => {
            assert!(
                m.contains("NeedsAttention") || m.contains("Needs Attention"),
                "{m}"
            );
            assert!(m.contains("--force-claim"), "{m}");
        }
        other => panic!("expected Refuse, got {:?}", other),
    }
}

#[test]
fn needs_attention_with_force_claim_warns_and_allows() {
    assert!(matches!(
        preflight_spec_status("BUG-9", Some(&RequirementStatus::NeedsAttention), true),
        PreflightDecision::AllowWithWarning(_)
    ));
}

#[test]
fn status_bump_waits_for_worktree_and_lease() {
    let tmp = TempDir::new().unwrap();
    let worktree = tmp.path().join("worktree");
    let lease = tmp.path().join("lease.toml");

    assert!(
        !session_start_status_bump_preconditions_met(
            Some(&RequirementStatus::Approved),
            &worktree,
            &lease,
        ),
        "missing worktree + lease must not permit Approved -> In Progress"
    );

    std::fs::create_dir_all(&worktree).unwrap();
    assert!(
        !session_start_status_bump_preconditions_met(
            Some(&RequirementStatus::Approved),
            &worktree,
            &lease,
        ),
        "worktree without lease must not permit Approved -> In Progress"
    );

    std::fs::write(&lease, "id = \"test\"\n").unwrap();
    assert!(
        session_start_status_bump_preconditions_met(
            Some(&RequirementStatus::Approved),
            &worktree,
            &lease,
        ),
        "Approved -> In Progress is allowed only after worktree + lease exist"
    );
}

#[test]
fn status_bump_preconditions_ignore_non_approved_statuses() {
    let tmp = TempDir::new().unwrap();
    let worktree = tmp.path().join("worktree");
    let lease = tmp.path().join("lease.toml");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(&lease, "id = \"test\"\n").unwrap();

    for status in [
        RequirementStatus::Planned,
        RequirementStatus::InProgress,
        RequirementStatus::NeedsAttention,
        RequirementStatus::Done,
        RequirementStatus::Completed,
        RequirementStatus::Rejected,
        RequirementStatus::Draft,
    ] {
        assert!(
            !session_start_status_bump_preconditions_met(Some(&status), &worktree, &lease),
            "{status:?} must not be auto-bumped by session start"
        );
    }
    assert!(
        !session_start_status_bump_preconditions_met(None, &worktree, &lease),
        "non-spec scopes have no status to bump"
    );
}
