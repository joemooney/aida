use super::*;

fn sample_requirement(spec: &str, description: &str) -> Requirement {
    let mut req = Requirement::new("Needs decision".to_string(), description.to_string());
    req.spec_id = Some(spec.to_string());
    req.status = RequirementStatus::Approved;
    req.priority = RequirementPriority::Medium;
    req.req_type = RequirementType::Story;
    req
}

fn sample_request() -> aida_core::DecisionRequest {
    aida_core::DecisionRequest {
        question: "Promote or ship?".to_string(),
        choices: vec![
            aida_core::DecisionChoice {
                label: "Promote to EPIC".to_string(),
                consequence: "decompose first".to_string(),
                resolution: "tag:+epic-candidate".to_string(),
            },
            aida_core::DecisionChoice {
                label: "Ship as story".to_string(),
                consequence: "implement directly".to_string(),
                resolution: "status:approved".to_string(),
            },
        ],
        recommended: Some(1),
        rationale: None,
        answered: None,
        note: None,
        asked_at: None,
        answered_at: None,
    }
}

/// No active leases — the common case for the candidate-detection tests.
fn no_leases() -> HashSet<String> {
    HashSet::new()
}

#[test]
fn sweep_scope_excludes_low_priority_archive_and_drafts() {
    let all = Vec::new();

    let mut low = sample_requirement(
        "STORY-LOW",
        "Acceptance: decide whether this should be built.",
    );
    low.priority = RequirementPriority::Low;
    assert!(
        question_sweep_candidate(&low, &all, QuestionSweepScope::Backlog, &no_leases()).is_none()
    );

    let mut archived = sample_requirement(
        "STORY-ARCHIVE",
        "Acceptance: decide whether this should be built.",
    );
    archived.archived = true;
    assert!(
        question_sweep_candidate(&archived, &all, QuestionSweepScope::Backlog, &no_leases())
            .is_none()
    );

    let mut draft = sample_requirement(
        "STORY-DRAFT",
        "Acceptance: decide whether this should be built.",
    );
    draft.status = RequirementStatus::Draft;
    assert!(
        question_sweep_candidate(&draft, &all, QuestionSweepScope::Backlog, &no_leases()).is_none()
    );
}

#[test]
fn sweep_flags_human_decision_markers() {
    let req = sample_requirement(
        "STORY-DECIDE",
        "Acceptance: implement after operator decision needed on the design fork.",
    );
    let candidate =
        question_sweep_candidate(&req, &[], QuestionSweepScope::Backlog, &no_leases()).unwrap();
    assert_eq!(candidate.reason, "decision-marker text");
}

#[test]
fn sweep_skips_open_decision_request_for_idempotency() {
    let mut req = sample_requirement(
        "STORY-ASKED",
        "Acceptance: operator decision needed before implementation.",
    );
    req.decision_request = Some(sample_request());
    assert!(
        question_sweep_candidate(&req, &[], QuestionSweepScope::Backlog, &no_leases()).is_none()
    );
}

#[test]
fn sweep_skips_advisor_resolvable_forks() {
    let req = sample_requirement(
            "STORY-ADVISOR",
            "Acceptance: operator decision needed, but this is advisor-resolvable from a recorded principle.",
        );
    assert!(
        question_sweep_candidate(&req, &[], QuestionSweepScope::Backlog, &no_leases()).is_none()
    );
}

// BUG-495: the missing-acceptance flag is noise on types an agent never
// implements (vision/folder/meta/principle/term) and on specs already
// built or in-flight (Done/Completed, review:draft-only, or leased).
#[test]
fn sweep_missing_acceptance_excludes_non_implementable_types() {
    for ty in [
        RequirementType::Vision,
        RequirementType::Principle,
        RequirementType::Term,
    ] {
        // No acceptance section in the text, so the only thing keeping the
        // flag from firing is the type exclusion.
        let mut req = sample_requirement("X-NOIMPL", "Strategic note. No criteria spelled out.");
        req.req_type = ty.clone();
        assert!(
            question_sweep_candidate(&req, &[], QuestionSweepScope::Backlog, &no_leases())
                .is_none(),
            "{ty:?} should be excluded from the missing-acceptance flag"
        );
    }

    // Control: an implementable Story with no criteria still fires.
    let story = sample_requirement("STORY-NOACC", "Build the thing. No criteria spelled out.");
    let candidate =
        question_sweep_candidate(&story, &[], QuestionSweepScope::Backlog, &no_leases()).unwrap();
    assert_eq!(candidate.reason, "missing acceptance criteria");
}

#[test]
fn sweep_missing_acceptance_excludes_built_or_held_specs() {
    // review:draft-only — work done, awaiting human review.
    let mut held = sample_requirement("STORY-HELD", "Build the thing. No criteria spelled out.");
    held.tags.insert("review:draft-only".to_string());
    assert!(
        question_sweep_candidate(&held, &[], QuestionSweepScope::All, &no_leases()).is_none(),
        "review:draft-only specs should be excluded"
    );

    // Active lease — work in-flight.
    let leased = sample_requirement("STORY-LEASED", "Build the thing. No criteria spelled out.");
    let mut leases = HashSet::new();
    leases.insert("story-leased".to_string());
    assert!(
        question_sweep_candidate(&leased, &[], QuestionSweepScope::Backlog, &leases).is_none(),
        "leased specs should be excluded"
    );

    // Sanity: the same spec WITHOUT a lease and not draft-only DOES fire,
    // proving the exclusions above are doing the work.
    let plain = sample_requirement("STORY-PLAIN", "Build the thing. No criteria spelled out.");
    let candidate =
        question_sweep_candidate(&plain, &[], QuestionSweepScope::Backlog, &no_leases()).unwrap();
    assert_eq!(candidate.reason, "missing acceptance criteria");

    // Done — work finished on a branch (built). Backlog scope already drops
    // Done, but assert the built-or-held guard holds directly too.
    let mut done = sample_requirement("STORY-DONE", "Build the thing. No criteria spelled out.");
    done.status = RequirementStatus::Done;
    assert!(is_built_or_held(&done, &no_leases()));
}

#[test]
fn sweep_formulates_story_522_decision_request() {
    let req = sample_requirement("STORY-FORM", "Acceptance: open question remains.");
    let candidate = QuestionSweepCandidate {
        reason: "decision-marker text".to_string(),
        kind: SweepKind::Clarify,
    };
    let request = formulate_sweep_decision_request(&req, &candidate);
    assert!(request.is_pending());
    assert_eq!(request.choices.len(), 2);
    assert_eq!(request.recommended, Some(1));
    assert!(request.question.contains("STORY-FORM"));
    // STORY-555: the park choice must use a tag the burndown gate
    // recognizes, else applying the answer would not actually park it.
    assert!(burndown::parking_tag(&["needs-decision".to_string()]).is_some());
    assert_eq!(request.choices[1].resolution, "tag:+needs-decision");
}

// trace:STORY-557 | ai:claude
#[test]
fn clarify_excludes_non_implementable_types() {
    for t in [
        RequirementType::Vision,
        RequirementType::Folder,
        RequirementType::Meta,
        RequirementType::Principle,
        RequirementType::Term,
    ] {
        let label = format!("{t:?}");
        let mut req = sample_requirement("VIS-1", "no acceptance here");
        req.req_type = t;
        assert!(is_clarify_excluded(&req), "{label} should be excluded");
    }
}

// trace:STORY-557 | ai:claude
#[test]
fn clarify_excludes_built_and_terminal_status() {
    for s in [
        RequirementStatus::Done,
        RequirementStatus::Completed,
        RequirementStatus::Rejected,
    ] {
        let label = format!("{s:?}");
        let mut req = sample_requirement("STORY-DONE", "no acceptance here");
        req.status = s;
        assert!(is_clarify_excluded(&req), "{label} should be excluded");
    }
}

// trace:STORY-557 | ai:claude
#[test]
fn clarify_excludes_held_for_review() {
    let mut req = sample_requirement("STORY-HELD", "no acceptance here");
    req.tags.insert("review:draft-only".to_string());
    assert!(is_clarify_excluded(&req));
}

// trace:STORY-557 | ai:claude
#[test]
fn clarify_includes_a_plain_underspecified_story() {
    // A workable story with no acceptance is NOT type/status/tag-excluded.
    let req = sample_requirement("STORY-THIN", "Implement the thing. No criteria yet.");
    assert!(!is_clarify_excluded(&req));
}

#[test]
fn parse_choice_splits_three_fields() {
    let c = parse_decision_choice("Reject|drop the spec|status:rejected").unwrap();
    assert_eq!(c.label, "Reject");
    assert_eq!(c.consequence, "drop the spec");
    assert_eq!(c.resolution, "status:rejected");
}

#[test]
fn parse_choice_trims_whitespace() {
    let c = parse_decision_choice("  A  |  does B  |  noop  ").unwrap();
    assert_eq!(c.label, "A");
    assert_eq!(c.consequence, "does B");
    assert_eq!(c.resolution, "noop");
}

#[test]
fn parse_choice_resolution_may_contain_no_extra_pipes() {
    // splitn(3) keeps everything after the 2nd pipe in `resolution`, so a
    // compound token survives intact.
    let c = parse_decision_choice("Approve|ready|status:approved;tag:+ready").unwrap();
    assert_eq!(c.resolution, "status:approved;tag:+ready");
}

#[test]
fn parse_choice_rejects_too_few_fields() {
    assert!(parse_decision_choice("just a label").is_err());
    assert!(parse_decision_choice("label|consequence").is_err());
}

#[test]
fn parse_choice_rejects_empty_fields() {
    assert!(parse_decision_choice("|consequence|res").is_err());
    assert!(parse_decision_choice("label||res").is_err());
    assert!(parse_decision_choice("label|consequence|").is_err());
}

#[test]
fn resolve_index_handles_numbers_and_keywords() {
    let req = sample_request();
    assert_eq!(resolve_choice_index(&req, "1").unwrap(), 0);
    assert_eq!(resolve_choice_index(&req, "2").unwrap(), 1);
    // default / recommended → the 0-based recommended index.
    assert_eq!(resolve_choice_index(&req, "default").unwrap(), 1);
    assert_eq!(resolve_choice_index(&req, "recommended").unwrap(), 1);
    // out of range / non-numeric → error.
    assert!(resolve_choice_index(&req, "0").is_err());
    assert!(resolve_choice_index(&req, "3").is_err());
    assert!(resolve_choice_index(&req, "nope").is_err());
}

#[test]
fn record_answer_flips_pending_to_answered() {
    let mut req = sample_request();
    assert!(req.is_pending());
    let idx = record_answer(&mut req, "1").unwrap();
    assert_eq!(idx, 0);
    assert_eq!(req.answered, Some(0));
    assert!(req.answered_at.is_some());
    assert!(!req.is_pending());
    // A second answer is refused.
    assert!(record_answer(&mut req, "2").is_err());
}

// TASK-791: a counter-proposal note rides ALONGSIDE the chosen option as
// pure data — it never changes which index is recorded.
#[test]
fn note_is_recorded_alongside_the_pick() {
    let mut req = sample_request();
    let idx = record_answer(&mut req, "1").unwrap();
    // Mirror questions_answer_one's note-attachment step.
    let note = Some("name it list-claude-sessions");
    if let Some(n) = note.map(str::trim).filter(|n| !n.is_empty()) {
        req.note = Some(n.to_string());
    }
    assert_eq!(idx, 0, "the picked index is unaffected by the note");
    assert_eq!(req.answered, Some(0));
    assert_eq!(req.note.as_deref(), Some("name it list-claude-sessions"));
}

// TASK-791: a blank/whitespace note is dropped (pure-pick path stays clean).
#[test]
fn blank_note_is_not_recorded() {
    let mut req = sample_request();
    record_answer(&mut req, "1").unwrap();
    let note = Some("   ");
    if let Some(n) = note.map(str::trim).filter(|n| !n.is_empty()) {
        req.note = Some(n.to_string());
    }
    assert_eq!(req.note, None);
}

#[test]
fn record_answer_default_keyword_uses_recommended() {
    let mut req = sample_request();
    let idx = record_answer(&mut req, "default").unwrap();
    assert_eq!(idx, 1);
    assert_eq!(req.answered, Some(1));
}

#[test]
fn confirm_default_sets_answered_to_recommended() {
    let mut req = sample_request();
    let idx = confirm_default(&mut req).unwrap();
    assert_eq!(idx, 1);
    assert_eq!(req.answered, Some(1));
    assert!(req.answered_at.is_some());
    assert!(!req.is_pending());
}

#[test]
fn confirm_default_skips_when_no_recommendation() {
    let mut req = sample_request();
    req.recommended = None;
    assert!(confirm_default(&mut req).is_none());
    assert!(req.is_pending(), "no default → left pending");
}

#[test]
fn confirm_default_skips_already_answered() {
    let mut req = sample_request();
    req.answered = Some(0);
    assert!(confirm_default(&mut req).is_none());
    assert_eq!(req.answered, Some(0), "existing answer untouched");
}

// =============================================================
// STORY-555: the slice-2 resolution applier + unpark evaluation.
// Pure functions over in-memory Requirements — no backend.
// trace:STORY-555 | ai:claude
// =============================================================

#[test]
fn apply_token_noop_changes_nothing() {
    let mut req = sample_requirement("STORY-N", "body");
    let applied = apply_resolution_token(&mut req, "noop", "Proceed", "stays");
    assert!(!applied.is_disposition);
    assert!(!applied.keeps_parked);
    assert!(req.tags.is_empty());
    assert_eq!(req.status, RequirementStatus::Approved);
}

// A noop (or any no-op resolution) must NOT claim to bind a decision into
// ## Acceptance — that produced the contradictory "no spec change (noop)" +
// "bound the decision into ## Acceptance" printout. trace:TASK-884
#[test]
fn apply_token_noop_does_not_bind_acceptance() {
    let mut req = sample_requirement("STORY-N", "body");
    let applied = apply_resolution_token(&mut req, "noop", "Proceed", "stays");
    assert!(
        !applied.binds_acceptance,
        "noop is a no-op; it must not bind into ## Acceptance"
    );
}

#[test]
fn apply_token_unknown_does_not_bind_acceptance() {
    let mut req = sample_requirement("STORY-N", "body");
    let applied = apply_resolution_token(&mut req, "garbage-token", "x", "y");
    assert!(
        !applied.binds_acceptance,
        "an unrecognized token mutates nothing and must not bind"
    );
}

#[test]
fn apply_token_redundant_tag_add_does_not_bind() {
    let mut req = sample_requirement("STORY-N", "body");
    req.tags.insert("already-here".to_string());
    let applied = apply_resolution_token(&mut req, "tag:+already-here", "x", "y");
    assert!(
        !applied.binds_acceptance,
        "adding a tag that already exists is a no-op and must not bind"
    );
}

#[test]
fn apply_token_real_tag_add_binds_acceptance() {
    let mut req = sample_requirement("STORY-N", "body");
    let applied = apply_resolution_token(&mut req, "tag:+chose-option-a", "x", "y");
    assert!(
        applied.binds_acceptance,
        "a genuine tag refinement binds into ## Acceptance"
    );
}

#[test]
fn apply_disposition_does_not_bind_acceptance() {
    let mut req = sample_requirement("STORY-N", "body");
    let applied = apply_resolution_token(&mut req, "disposition:reject", "Reject", "no");
    assert!(
        !applied.binds_acceptance,
        "a disposition is not a design refinement and must not bind"
    );
}

#[test]
fn apply_token_add_parking_tag_keeps_parked() {
    let mut req = sample_requirement("STORY-N", "body");
    let applied = apply_resolution_token(&mut req, "tag:+needs-decision", "Park", "held");
    assert!(req.tags.contains("needs-decision"));
    assert!(
        applied.keeps_parked,
        "a recognized parking tag keeps it parked"
    );
}

#[test]
fn apply_token_remove_tag_is_case_insensitive() {
    let mut req = sample_requirement("STORY-N", "body");
    req.tags.insert("Needs-Design-Signoff".to_string());
    let applied = apply_resolution_token(&mut req, "tag:-needs-design-signoff", "x", "y");
    assert!(req.tags.is_empty(), "removed despite casing");
    assert!(!applied.keeps_parked);
}

#[test]
fn apply_token_status_rejected_keeps_parked() {
    let mut req = sample_requirement("STORY-N", "body");
    let applied = apply_resolution_token(&mut req, "status:rejected", "x", "y");
    assert_eq!(req.status, RequirementStatus::Rejected);
    assert!(applied.keeps_parked, "a rejected spec is not queue-worthy");
}

#[test]
fn apply_token_unknown_records_only() {
    let mut req = sample_requirement("STORY-N", "body");
    req.status = RequirementStatus::Approved;
    let applied = apply_resolution_token(&mut req, "garbage-token", "x", "y");
    assert_eq!(req.status, RequirementStatus::Approved, "no mutation");
    assert!(req.tags.is_empty());
    // BUG-726: a freeform directive is recorded and LEFT PARKED — never
    // silently auto-queued (that was the bug that mis-queued disposition answers).
    assert!(applied.effects[0].contains("directive"));
    assert!(
        applied.keeps_parked,
        "an unrecognized directive must not queue the spec"
    );
}

#[test]
fn apply_token_bare_defer_parks_out_of_queue() {
    // BUG-726: a bare `defer` answer defers the spec (parked), not queues it.
    let mut req = sample_requirement("STORY-N", "body");
    req.status = RequirementStatus::Approved;
    assert!(!req.deferred);
    let applied = apply_resolution_token(&mut req, "defer", "Defer", "wait for demand");
    assert!(req.deferred, "bare `defer` must set the deferred flag");
    assert!(applied.keeps_parked, "a deferred spec is not queue-worthy");
}

#[test]
fn apply_token_bare_disposition_verbs_work_without_prefix() {
    // BUG-726: bare disposition verbs resolve without the `disposition:` prefix.
    let mut req = sample_requirement("STORY-N", "body");
    req.status = RequirementStatus::Approved;
    let applied = apply_resolution_token(&mut req, "reject", "Reject", "stale");
    assert_eq!(req.status, RequirementStatus::Rejected);
    assert!(applied.keeps_parked, "a rejected spec is not queue-worthy");

    let mut req2 = sample_requirement("STORY-M", "body");
    req2.tags.insert("needs-design".to_string());
    let applied2 = apply_resolution_token(&mut req2, "queue", "Approve+queue", "go");
    assert!(applied2.is_disposition);
    assert!(!applied2.keeps_parked, "an approved spec is queue-worthy");
}

#[test]
fn apply_disposition_approve_clears_all_gates() {
    let mut req = sample_requirement("STORY-N", "body");
    req.tags.insert("needs-design-signoff".to_string());
    req.tags.insert("operator-action".to_string());
    req.tags.insert("keep-me".to_string());
    let applied = apply_resolution_token(&mut req, "disposition:approve-to-ready", "Approve", "go");
    assert!(applied.is_disposition);
    assert!(!applied.keeps_parked);
    assert!(!req.tags.contains("needs-design-signoff"));
    assert!(!req.tags.contains("operator-action"));
    assert!(req.tags.contains("keep-me"), "non-gating tag survives");
}

#[test]
fn apply_disposition_reject_sets_status_and_holds() {
    let mut req = sample_requirement("STORY-N", "body");
    let applied = apply_resolution_token(&mut req, "disposition:reject", "Reject", "no");
    assert_eq!(req.status, RequirementStatus::Rejected);
    assert!(applied.is_disposition);
    assert!(applied.keeps_parked);
}

#[test]
fn apply_disposition_keep_parked_records_why_open() {
    let mut req = sample_requirement("STORY-N", "body");
    let before = req.comments.len();
    let applied = apply_resolution_token(&mut req, "disposition:keep-parked", "Keep", "later");
    assert!(applied.keeps_parked);
    assert_eq!(req.comments.len(), before + 1);
    assert!(req.comments.last().unwrap().content.contains("why-open"));
}

#[test]
fn acceptance_refinement_appends_into_existing_section() {
    let body = "Intro.\n\n## Acceptance\n\n1. Does a thing.\n\n## Notes\n\ntail";
    let out = append_resolved_to_acceptance(body, "- Resolved: chose A");
    // The refinement lands inside Acceptance, before the Notes heading.
    let accept = out.find("## Acceptance").unwrap();
    let resolved = out.find("- Resolved: chose A").unwrap();
    let notes = out.find("## Notes").unwrap();
    assert!(accept < resolved && resolved < notes, "{out}");
}

#[test]
fn acceptance_refinement_creates_section_when_absent() {
    let out = append_resolved_to_acceptance("Just a description.", "- Resolved: chose B");
    assert!(out.contains("## Acceptance"));
    assert!(out.contains("- Resolved: chose B"));
}

#[test]
fn unpark_ready_when_decision_answered_and_unblocked() {
    let mut req = sample_requirement("STORY-N", "body");
    // No parking tags, not an epic, no blockers, answered decision.
    let mut dr = sample_request();
    dr.answered = Some(0);
    req.decision_request = Some(dr);
    let all = vec![req.clone()];
    assert_eq!(evaluate_unpark(&req, &all), burndown::Pickability::Ready);
}

#[test]
fn unpark_reports_remaining_parking_tag() {
    let mut req = sample_requirement("STORY-N", "body");
    req.tags.insert("deferred:post-stability".to_string());
    let all = vec![req.clone()];
    match evaluate_unpark(&req, &all) {
        burndown::Pickability::Parked(reason) => assert!(reason.contains("deferred")),
        other => panic!("expected Parked, got {other:?}"),
    }
}

#[test]
fn sweep_synthesizes_disposition_for_tag_parked_spec() {
    let mut req = sample_requirement("STORY-PARK", "Has acceptance criteria, fully specified.");
    req.tags.insert("needs-design-signoff".to_string());
    let all = vec![req.clone()];
    let candidate =
        question_sweep_candidate(&req, &all, QuestionSweepScope::All, &no_leases()).unwrap();
    assert!(matches!(candidate.kind, SweepKind::Disposition(_)));
    let dr = formulate_sweep_decision_request(&req, &candidate);
    assert_eq!(dr.choices.len(), 3, "approve / reject / keep");
    assert_eq!(dr.recommended, None, "a disposition needs an explicit call");
    assert_eq!(dr.choices[0].resolution, "disposition:approve-to-ready");
}
