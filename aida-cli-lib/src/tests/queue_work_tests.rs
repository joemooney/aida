//! STORY-42: cover the pure-decision helpers used by `aida queue work`.
//! Side-effecting bits (session_start, exec) are integration-tested
//! by hand in the merge gate; these unit tests pin role inference,
//! prompt routing, scope derivation, and spec-id matching so future
//! refactors don't silently break the resolver.
//! trace:STORY-42 | ai:claude
use super::*;
use aida_core::{QueueEntry, Relationship, Requirement, RequirementType};
use uuid::Uuid;

/// Minimal real-git helper for the BUG-1515 tip-relation tests below —
/// `done_spec_outstanding_refusal` now shells out to `git` to place the
/// verdict's reviewed sha against the branch tip, so a fake sha in a
/// non-repo tempdir no longer exercises the real decision.
// trace:BUG-1515 | ai:claude
fn git(repo: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// trace:BUG-1515 | ai:claude
fn git_head(repo: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// BUG-1213 (round 2): the loop guard fires only when the IMMEDIATELY previous
/// findings block equals the new one. Two consecutive identical rounds (A, A)
/// recur; A → B → A does not — the last recorded block is B, so a third round
/// with A is progress, not a loop.
// trace:BUG-1213 | ai:claude
#[test]
fn findings_recur_only_when_the_last_block_is_identical() {
    use crate::queue_cmd::findings_recur_consecutively;
    let prefix = crate::review_verdict::FINDINGS_BLOCK_PREFIX;
    let a = format!("{prefix}PR #7):\nVerdict: RequestChanges\n- fix A");
    let b = format!("{prefix}PR #7):\nVerdict: RequestChanges\n- fix B");
    let base = chrono::Utc::now();
    let mk = |content: &str, secs: i64| {
        let mut c = aida_core::Comment::new("reviewer".to_string(), content.to_string());
        c.created_at = base + chrono::Duration::seconds(secs);
        c
    };
    let chatter = mk("[aida:proxy-note] unrelated comment", 5);
    assert!(!findings_recur_consecutively(&[], &a), "no history");
    assert!(
        findings_recur_consecutively(&[mk(&a, 1)], &a),
        "A, A recurs"
    );
    assert!(
        findings_recur_consecutively(&[mk(&a, 1), chatter.clone()], &a),
        "non-findings comments in between are ignored"
    );
    assert!(
        !findings_recur_consecutively(&[mk(&a, 1), mk(&b, 2)], &a),
        "A, B, A: the last block is B — not a loop"
    );
    assert!(
        findings_recur_consecutively(&[mk(&a, 1), mk(&b, 2), mk(&a, 3)], &a),
        "…but A, B, A, A is"
    );
    // Order is by comment time, not slice position.
    assert!(!findings_recur_consecutively(&[mk(&a, 9), mk(&b, 1)], &b));
}

/// BUG-1213 (round 3): the production pickup derives the round from the
/// spec's recorded findings blocks — a later re-drive says its ACTUAL round,
/// not a constant ROUND 2.
// trace:BUG-1213 | ai:claude
#[test]
fn rework_pickup_round_is_derived_from_recorded_findings_blocks() {
    use crate::queue_cmd::{derive_rework_queue_work_prompt, rework_round_from_comments};
    let prefix = crate::review_verdict::FINDINGS_BLOCK_PREFIX;
    let block = |n: usize| format!("{prefix}PR #9):\nVerdict: RequestChanges\n- item {n}");
    let mk = |content: String| aida_core::Comment::new("reviewer".to_string(), content);
    // No findings recorded yet (first rework about to be queued) → round 2.
    assert_eq!(rework_round_from_comments(&[]), 2);
    // One recorded block → the pickup after it is round 2; two → round 3.
    assert_eq!(rework_round_from_comments(&[mk(block(1))]), 2);
    let two = vec![
        mk(block(1)),
        mk("[aida:proxy-note] chatter".into()),
        mk(block(2)),
    ];
    assert_eq!(rework_round_from_comments(&two), 3);
    let e = resolved("BUG-1213", entry(Uuid::now_v7(), Some("implementer"), None));
    let plan = QueueWorkPlan {
        mode: QueueWorkMode::Item,
        entries: vec![e],
        scope: "BUG-1213".into(),
        review_target: None,
        anchor_display: "BUG-1213".into(),
        anchor_title: "title".into(),
    };
    let findings = block(2);
    // Exercise the history-aware production assembly path used by
    // `handle_queue_work`, rather than injecting a round into the formatter.
    let prompt =
        derive_rework_queue_work_prompt(&plan, "implementer", false, false, Some(&findings), &two);
    assert!(prompt.starts_with("ROUND 3 — ITEMS STILL OPEN"), "{prompt}");
    assert!(prompt.ends_with("/aida-pickup BUG-1213"), "{prompt}");
}

// trace:BUG-1213 | ai:codex
#[test]
fn rework_prompt_leads_with_round_and_authoritative_open_items() {
    let prompt = rework_pickup_prompt(
        "REVIEW FINDINGS TO ADDRESS:\n- add real CLI/MCP parity coverage",
        "/aida-pickup BUG-1213",
        4,
    );
    assert!(prompt.starts_with("ROUND 4 — ITEMS STILL OPEN (AUTHORITATIVE TASK):"));
    assert!(
        prompt.contains("previous round's commit is already on the PR branch and does not count")
    );
    assert!(prompt.ends_with("/aida-pickup BUG-1213"));
}

/// TASK-1291: round 1 makes every acceptance decision and every defect
/// explicit; later rounds retain the established compact review prompt.
// trace:TASK-1291 | ai:codex
#[test]
fn round_one_reviewer_prompt_requires_a_complete_sweep_only_once() {
    let first = reviewer_prompt_for_round(1291, 1);
    assert!(first.starts_with("/aida-review --pr 1291"));
    assert!(first.contains("enumerate EVERY acceptance criterion"));
    assert!(first.contains("one status line per criterion"));
    assert!(first.contains("list EVERY defect you find, not only the first"));
    assert!(first.contains("`findings` array must carry the complete"));

    assert_eq!(reviewer_prompt_for_round(1291, 2), "/aida-review --pr 1291");
    assert_eq!(reviewer_prompt_for_round(1291, 7), "/aida-review --pr 1291");
}

// trace:TASK-1291 | ai:codex
#[test]
fn reviewer_round_is_derived_from_all_recorded_findings_blocks() {
    let prefix = crate::review_verdict::FINDINGS_BLOCK_PREFIX;
    let mk = |content: String| aida_core::Comment::new("reviewer".to_string(), content);
    assert_eq!(review_round_from_comments(&[]), 1);
    assert_eq!(
        review_round_from_comments(&[
            mk(format!("{prefix}PR #12):\n- first batch")),
            mk("unrelated note".into()),
            mk(format!("{prefix}PR #12):\n- second batch")),
        ]),
        3
    );
}

/// TASK-1291: discussion that quotes or embeds the durable marker is not a
/// completed review and must not suppress the mandatory round-1 sweep.
// trace:TASK-1291 | ai:codex
#[test]
fn reviewer_round_ignores_embedded_or_quoted_findings_markers() {
    let prefix = crate::review_verdict::FINDINGS_BLOCK_PREFIX;
    let mk = |content: String| aida_core::Comment::new("reviewer".to_string(), content);

    assert_eq!(
        review_round_from_comments(&[
            mk(format!("Discussion mentions {prefix}PR #12) as an example")),
            mk(format!("> {prefix}PR #12):\n> quoted from another review")),
        ]),
        1
    );
}

/// TASK-1291: every durable-history consumer shares the exact same
/// prefix-at-byte-zero classification; quoted examples are neither latest
/// findings nor completed rework rounds.
// trace:TASK-1291 | ai:codex
#[test]
fn all_review_history_helpers_reject_noncanonical_marker_placement() {
    let prefix = crate::review_verdict::FINDINGS_BLOCK_PREFIX;
    let mk = |content: String| aida_core::Comment::new("reviewer".to_string(), content);
    let comments = vec![
        mk(format!("preamble: {prefix}PR #12):\nFindings:\n1. example")),
        mk(format!("> {prefix}PR #12):\n> quoted example")),
    ];

    assert!(latest_findings_block(&comments).is_none());
    assert_eq!(rework_round_from_comments(&comments), 2);
    assert_eq!(review_round_from_comments(&comments), 1);
}

fn req(spec_id: &str, agreed: Option<&str>, t: RequirementType) -> Requirement {
    let mut r = Requirement::new(spec_id.to_string(), String::new());
    r.spec_id = Some(spec_id.into());
    r.agreed_id = agreed.map(String::from);
    r.req_type = t;
    r
}

fn entry(req_id: Uuid, for_role: Option<&str>, for_scope: Option<&str>) -> QueueEntry {
    QueueEntry {
        user_id: "u".into(),
        requirement_id: req_id,
        position: 1,
        added_by: "u".into(),
        note: None,
        added_at: chrono::Utc::now(),
        for_role: for_role.map(String::from),
        for_scope: for_scope.map(String::from),
        for_session: None,
        added_by_machine: None,
    }
}

fn resolved(spec: &str, qe: QueueEntry) -> QueueWorkEntry {
    QueueWorkEntry {
        queue: qe,
        spec_id: spec.into(),
        status_at_plan: "Approved".into(),
    }
}

fn resolved_with_status(spec: &str, qe: QueueEntry, status: &str) -> QueueWorkEntry {
    QueueWorkEntry {
        queue: qe,
        spec_id: spec.into(),
        status_at_plan: status.into(),
    }
}

fn plan_with(mode: QueueWorkMode, scope: &str, entries: Vec<QueueWorkEntry>) -> QueueWorkPlan {
    let review_target = parse_review_scope(scope);
    QueueWorkPlan {
        mode,
        entries,
        scope: scope.into(),
        review_target,
        anchor_display: scope.into(),
        anchor_title: "anchor".into(),
    }
}

#[test]
fn rework_pr_head_lookup_selects_open_pr_head_branch() {
    let lookup = PrLookup::Found(OpenPrInfo {
        number: 1704,
        title: "Rework STORY-994".into(),
        url: "https://example.test/pr/1704".into(),
        head_branch: Some("story-994-pr1704".into()),
    });

    assert_eq!(
        rework_pr_head_branch_from_lookup(lookup),
        Some("story-994-pr1704".into())
    );
}

#[test]
fn rework_pr_head_lookup_ignores_missing_or_empty_branch() {
    let lookup = PrLookup::Found(OpenPrInfo {
        number: 1704,
        title: "Rework STORY-994".into(),
        url: "https://example.test/pr/1704".into(),
        head_branch: Some("   ".into()),
    });

    assert_eq!(rework_pr_head_branch_from_lookup(lookup), None);
    assert_eq!(rework_pr_head_branch_from_lookup(PrLookup::NoOpenPr), None);
}

#[test]
fn rework_pr_head_match_accepts_branch_for_same_spec() {
    assert!(rework_pr_head_matches_spec(
        "STORY-994",
        "story-994-pr1704",
        "Rework unrelated title",
        "",
    ));
}

#[test]
fn rework_pr_head_match_rejects_branch_for_different_spec() {
    assert!(!rework_pr_head_matches_spec(
        "STORY-1028",
        "story-1033-followup",
        "STORY-1028",
        "Body mentions STORY-1028, but the branch trailer owns STORY-1033.",
    ));
}

#[test]
fn rework_pr_head_match_allows_metadata_when_branch_has_no_spec_id() {
    assert!(rework_pr_head_matches_spec(
        "BUG-1074",
        "followup-polish",
        "[AI:codex] fix(queue): tighten rework reuse (BUG-1074)",
        "",
    ));
}

#[test]
fn in_progress_single_spec_pickup_wants_pr_head_branch() {
    let r = req("STORY-994", None, RequirementType::Story);
    let e = resolved_with_status(
        "STORY-994",
        entry(r.id, Some("implementer"), None),
        "In Progress",
    );
    let plan = plan_with(QueueWorkMode::Item, "STORY-994", vec![e]);

    assert!(queue_work_plan_wants_pr_head_branch(&plan));
}

#[test]
fn approved_or_review_pickup_does_not_use_pr_head_branch() {
    let r = req("STORY-994", None, RequirementType::Story);
    let approved = resolved("STORY-994", entry(r.id, Some("implementer"), None));
    let approved_plan = plan_with(QueueWorkMode::Item, "STORY-994", vec![approved]);
    assert!(!queue_work_plan_wants_pr_head_branch(&approved_plan));

    let review = resolved_with_status(
        "PR-1704",
        entry(Uuid::now_v7(), Some("reviewer"), None),
        "In Progress",
    );
    let review_plan = plan_with(QueueWorkMode::Item, "PR-1704", vec![review]);
    assert!(!queue_work_plan_wants_pr_head_branch(&review_plan));
}

/// Single-role cluster → that role, "cluster-derived", no warnings.
#[test]
fn role_uniform_cluster_picks_that_role() {
    let r = req("BUG-1", None, RequirementType::Bug);
    let e1 = resolved("BUG-1", entry(r.id, Some("implementer"), None));
    let e2 = resolved("BUG-2", entry(Uuid::now_v7(), Some("implementer"), None));
    let plan = plan_with(QueueWorkMode::Cluster, "EPIC-20", vec![e1, e2]);
    let (role, origin, warns) = infer_queue_work_role(&plan, None);
    assert_eq!(role, "implementer");
    assert_eq!(origin, "cluster-derived");
    assert!(warns.is_none());
}

/// Mixed-role cluster → majority wins + a warning about the minority.
#[test]
fn role_majority_wins_with_warning() {
    let mut entries = Vec::new();
    for _ in 0..3 {
        entries.push(resolved(
            "X",
            entry(Uuid::now_v7(), Some("implementer"), None),
        ));
    }
    entries.push(resolved("Y", entry(Uuid::now_v7(), Some("reviewer"), None)));
    let plan = plan_with(QueueWorkMode::Cluster, "EPIC-20", entries);
    let (role, origin, warns) = infer_queue_work_role(&plan, None);
    assert_eq!(role, "implementer");
    assert_eq!(origin, "cluster-derived");
    let warns = warns.expect("minority should warn");
    assert!(warns
        .iter()
        .any(|w| w.contains("reviewer") || w.contains("other role")));
}

/// Explicit --role override beats the cluster tally and the
/// scope default.
#[test]
fn role_override_wins() {
    let e = resolved("BUG-1", entry(Uuid::now_v7(), Some("implementer"), None));
    let plan = plan_with(QueueWorkMode::Item, "EPIC-20", vec![e]);
    let (role, origin, warns) = infer_queue_work_role(&plan, Some("architect"));
    assert_eq!(role, "architect");
    assert_eq!(origin, "--role flag");
    assert!(warns.is_none());
}

/// Empty cluster (no for_role on any entry) + PR-N scope → reviewer
// default. trace:STORY-42 | ai:claude
#[test]
fn role_scope_default_pr_is_reviewer() {
    let e = resolved("STORY-1", entry(Uuid::now_v7(), None, None));
    let plan = plan_with(QueueWorkMode::Cluster, "PR-11", vec![e]);
    let (role, origin, _) = infer_queue_work_role(&plan, None);
    assert_eq!(role, "reviewer");
    assert_eq!(origin, "scope-default");
}

/// Empty cluster + non-PR scope → implementer default.
#[test]
fn role_scope_default_non_pr_is_implementer() {
    let e = resolved("STORY-1", entry(Uuid::now_v7(), None, None));
    let plan = plan_with(QueueWorkMode::Cluster, "EPIC-20", vec![e]);
    let (role, origin, _) = infer_queue_work_role(&plan, None);
    assert_eq!(role, "implementer");
    assert_eq!(origin, "scope-default");
}

/// BUG-862: an implementer auto-complete drain must skip a reviewer-routed
/// queue head and select the next implementer-drivable item instead of
/// launching a doomed phase-1 implementer session for the review row.
// trace:BUG-862 | ai:codex
#[test]
fn auto_complete_head_skips_entries_routed_to_other_roles() {
    let candidates = vec![
        AutoCompleteHeadCandidate {
            id: "STORY-943".to_string(),
            status: RequirementStatus::Approved,
            for_role: Some("reviewer".to_string()),
            deferred: false,
            execution_mode: None,
            tags: Default::default(),
            blocked: None,
        },
        AutoCompleteHeadCandidate {
            id: "TASK-944".to_string(),
            status: RequirementStatus::Approved,
            for_role: Some("implementer".to_string()),
            deferred: false,
            execution_mode: None,
            tags: Default::default(),
            blocked: None,
        },
    ];

    let pick = pick_auto_complete_head_for_role(&candidates, "implementer")
        .expect("implementer drain should find the implementer-routed item");
    assert_eq!(pick.spec, "TASK-944");
    assert_eq!(
        pick.role_skipped,
        vec![("STORY-943".to_string(), "reviewer".to_string())]
    );
    assert!(pick.status_skipped.is_empty());
}

#[test]
fn auto_complete_head_skips_deferred_candidates() {
    let candidates = vec![
        AutoCompleteHeadCandidate {
            id: "TASK-1205".to_string(),
            status: RequirementStatus::Approved,
            for_role: Some("implementer".to_string()),
            deferred: true,
            execution_mode: None,
            tags: Default::default(),
            blocked: None,
        },
        AutoCompleteHeadCandidate {
            id: "TASK-1208".to_string(),
            status: RequirementStatus::Approved,
            for_role: Some("implementer".to_string()),
            deferred: false,
            execution_mode: None,
            tags: Default::default(),
            blocked: None,
        },
    ];

    let pick = pick_auto_complete_head_for_role(&candidates, "implementer")
        .expect("drain should skip deferred rows and pick the next drivable item");
    assert_eq!(pick.spec, "TASK-1208");
    assert_eq!(pick.deferred_skipped, vec!["TASK-1205".to_string()]);
    assert!(pick.status_skipped.is_empty());
    assert!(pick.role_skipped.is_empty());
}

// trace:STORY-1125 | ai:codex
#[test]
fn auto_complete_head_skips_release_tagged_candidates() {
    let candidates = vec![
        AutoCompleteHeadCandidate {
            id: "STORY-1125".to_string(),
            status: RequirementStatus::Approved,
            for_role: Some("implementer".to_string()),
            deferred: false,
            execution_mode: Some(aida_core::ExecutionMode::Drain),
            tags: ["release workflow meta-task aida:release".to_string()]
                .into_iter()
                .collect(),
            blocked: None,
        },
        AutoCompleteHeadCandidate {
            id: "TASK-1126".to_string(),
            status: RequirementStatus::Approved,
            for_role: Some("implementer".to_string()),
            deferred: false,
            execution_mode: Some(aida_core::ExecutionMode::Drain),
            tags: Default::default(),
            blocked: None,
        },
    ];

    let pick = pick_auto_complete_head_for_role(&candidates, "implementer")
        .expect("headless drain should skip release tasks and pick normal drainable work");

    assert_eq!(pick.spec, "TASK-1126");
    assert_eq!(pick.release_skipped, vec!["STORY-1125".to_string()]);
    assert!(pick.status_skipped.is_empty());
    assert!(pick.role_skipped.is_empty());
    assert!(pick.deferred_skipped.is_empty());
    assert!(pick.guided_or_operator_skipped.is_empty());
}

// trace:BUG-1120 | ai:codex
#[test]
fn auto_complete_head_skips_guided_operator_and_decide_candidates() {
    let candidates = vec![
        AutoCompleteHeadCandidate {
            id: "STORY-1120".to_string(),
            status: RequirementStatus::Approved,
            for_role: Some("implementer".to_string()),
            deferred: false,
            execution_mode: Some(aida_core::ExecutionMode::Guided),
            tags: Default::default(),
            blocked: None,
        },
        AutoCompleteHeadCandidate {
            id: "TASK-1121".to_string(),
            status: RequirementStatus::Approved,
            for_role: Some("implementer".to_string()),
            deferred: false,
            execution_mode: Some(aida_core::ExecutionMode::Operator),
            tags: Default::default(),
            blocked: None,
        },
        AutoCompleteHeadCandidate {
            id: "TASK-1122".to_string(),
            status: RequirementStatus::Approved,
            for_role: Some("implementer".to_string()),
            deferred: false,
            execution_mode: Some(aida_core::ExecutionMode::Decide),
            tags: Default::default(),
            blocked: None,
        },
        AutoCompleteHeadCandidate {
            id: "TASK-1123".to_string(),
            status: RequirementStatus::Approved,
            for_role: Some("implementer".to_string()),
            deferred: false,
            execution_mode: Some(aida_core::ExecutionMode::Drain),
            tags: Default::default(),
            blocked: None,
        },
    ];

    let pick = pick_auto_complete_head_for_role(&candidates, "implementer")
        .expect("drain should skip interactive/blocking modes and pick drainable work");

    assert_eq!(pick.spec, "TASK-1123");
    assert_eq!(
        pick.guided_or_operator_skipped,
        vec![
            ("STORY-1120".to_string(), aida_core::ExecutionMode::Guided),
            ("TASK-1121".to_string(), aida_core::ExecutionMode::Operator),
            ("TASK-1122".to_string(), aida_core::ExecutionMode::Decide),
        ]
    );
    assert!(pick.status_skipped.is_empty());
    assert!(pick.role_skipped.is_empty());
    assert!(pick.deferred_skipped.is_empty());
}

/// BUG-862 (reviewer finding, round 2): a drain launched from a DISPATCH
/// seat works the implementer lane. AIDA_SESSION_ROLE=advisor (or human)
/// must not become the drain's working role — that filtered out reviewer-
/// AND implementer-routed entries and reported "no drivable queued items".
/// An explicit --role override still wins.
// trace:BUG-862 | ai:claude
#[test]
fn effective_auto_complete_role_maps_dispatch_seats_to_implementer() {
    let _guard = crate::test_env::env_lock();
    for (seat, expect) in [
        ("advisor", "implementer"),
        ("human", "implementer"),
        ("dialog", "implementer"), // deprecated alias canonicalizes to advisor
        ("reviewer", "reviewer"),
        ("implementer", "implementer"),
    ] {
        std::env::set_var("AIDA_SESSION_ROLE", seat);
        assert_eq!(
            effective_auto_complete_role(None),
            expect,
            "session role {seat}"
        );
    }
    std::env::set_var("AIDA_SESSION_ROLE", "advisor");
    assert_eq!(
        effective_auto_complete_role(Some("reviewer")),
        "reviewer",
        "explicit override beats the dispatch-seat mapping"
    );
    std::env::remove_var("AIDA_SESSION_ROLE");
    assert_eq!(effective_auto_complete_role(None), "implementer");
}

fn queued_status_fixture(statuses: &[(&str, RequirementStatus)]) -> (tempfile::TempDir, Storage) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let backend = aida_core::GitBackend::new(&root).unwrap();
    let storage = Storage::new(&root);
    let mut store = aida_core::RequirementsStore::default();

    for (idx, (spec, status)) in statuses.iter().enumerate() {
        let mut r = aida_core::Requirement::new(format!("title for {spec}"), String::new());
        r.spec_id = Some((*spec).to_string());
        r.agreed_id = Some((*spec).to_string());
        r.status = status.clone();
        let req_id = r.id;
        store.requirements.push(r);
        storage
            .queue_add(aida_core::QueueEntry {
                user_id: "u".into(),
                requirement_id: req_id,
                position: ((idx + 1) * 1000) as i64,
                added_by: "u".into(),
                note: None,
                added_at: chrono::Utc::now(),
                for_role: Some("implementer".into()),
                for_scope: None,
                for_session: None,
                added_by_machine: None,
            })
            .unwrap();
    }
    backend.save(&store).unwrap();
    (dir, storage)
}

fn queued_status_fixture_for_user(
    statuses: &[(&str, RequirementStatus)],
    queue_user: &str,
    for_role: &str,
) -> (tempfile::TempDir, Storage) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let backend = aida_core::GitBackend::new(&root).unwrap();
    let storage = Storage::new(&root);
    let mut store = aida_core::RequirementsStore::default();

    for (idx, (spec, status)) in statuses.iter().enumerate() {
        let mut r = aida_core::Requirement::new(format!("title for {spec}"), String::new());
        r.spec_id = Some((*spec).to_string());
        r.agreed_id = Some((*spec).to_string());
        r.status = status.clone();
        let req_id = r.id;
        store.requirements.push(r);
        storage
            .queue_add(aida_core::QueueEntry {
                user_id: queue_user.into(),
                requirement_id: req_id,
                position: ((idx + 1) * 1000) as i64,
                added_by: queue_user.into(),
                note: None,
                added_at: chrono::Utc::now(),
                for_role: Some(for_role.into()),
                for_scope: None,
                for_session: None,
                added_by_machine: None,
            })
            .unwrap();
    }
    backend.save(&store).unwrap();
    (dir, storage)
}

// Merge-gate keeps the authoritative object under the node-qualified
// `spec_id` while operators and queue surfaces use `agreed_id`. Batch pickup
// must resolve the queue UUID to that object instead of deriving a YAML path
// from the display id. trace:BUG-1264 | ai:codex
#[test]
fn batch_members_resolve_agreed_id_alias_to_origin_object() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let backend = aida_core::GitBackend::new(&root).unwrap();
    let storage = Storage::new(&root);

    let mut req = Requirement::new("aliased member".into(), String::new());
    req.spec_id = Some("TASK-1-140".into());
    req.agreed_id = Some("TASK-163".into());
    req.status = RequirementStatus::Approved;
    req.tags.insert("batch:alias-wave".into());
    let req_id = req.id;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    backend.save(&store).unwrap();
    storage
        .queue_add(entry(req_id, Some("implementer"), None))
        .unwrap();

    let members = resolve_batch_members(
        &storage,
        "role:implementer",
        "alias-wave",
        Some("implementer"),
    )
    .expect("alias-aware cross-user batch resolution");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].1, "TASK-163");
    assert_eq!(members[0].0.requirement_id, req_id);
}

// An orphaned queue UUID must be observable both to the operator and to event
// consumers; silently dropping it can falsely make a batch look drained.
// trace:BUG-1264 | ai:codex
#[test]
fn batch_members_report_and_emit_unresolvable_member() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    aida_core::GitBackend::new(&root).unwrap();
    let storage = Storage::new(&root);
    let missing_id = Uuid::now_v7();
    storage
        .queue_add(entry(missing_id, Some("implementer"), None))
        .unwrap();

    let mut diagnostic = Vec::new();
    let members = resolve_batch_members_with_context(
        &storage,
        "role:implementer",
        "missing-wave",
        Some("implementer"),
        Some(dir.path()),
        &mut diagnostic,
    )
    .expect("missing queue member is skipped, not fatal");

    assert!(members.is_empty());
    let diagnostic = String::from_utf8(diagnostic).unwrap();
    assert!(diagnostic.contains(&format!(
        "batch:missing-wave — skipping unresolvable member {missing_id}"
    )));
    let events = crate::events::read_all(dir.path());
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].spec.as_deref(),
        Some(missing_id.to_string()).as_deref()
    );
    assert!(events[0].kind.is_actionable());
    assert!(matches!(
        &events[0].kind,
        crate::events::EventKind::SpecSkipped { reason }
            if reason == "requirement id does not resolve to a stored object"
    ));
}

fn implementer_lease(scope: &str) -> SessionLease {
    SessionLease {
        id: "lease-1082".to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "codex@example.test".to_string(),
        worktree_path: std::path::PathBuf::from("/tmp/aida-bug-1082"),
        branch: scope.to_ascii_lowercase(),
        started_at: chrono::Utc::now(),
        hostname: "testhost".to_string(),
        role: Some("implementer".to_string()),
        creator_pid: None,
        creator_pid_start_time: None,
        active_pid: None,
        active_pid_start_time: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
        manual_enter_at: None,
    }
}

#[test]
fn leased_implementer_head_pins_assigned_spec_over_queue_head() {
    let (_dir, storage) = queued_status_fixture(&[
        ("STORY-1054", RequirementStatus::Approved),
        ("STORY-1051", RequirementStatus::InProgress),
    ]);
    let store = storage.load().unwrap();
    let entries = storage.queue_list("u", false).unwrap();
    let lease = implementer_lease("STORY-1051");

    let head = leased_implementer_queue_head(&entries, &store, "u", Some(&lease))
        .expect("implementer lease should pin the assigned spec");
    let req = store
        .requirements
        .iter()
        .find(|r| r.id == head.requirement_id)
        .unwrap();

    // trace:BUG-1082 | ai:codex
    assert_eq!(req.display_id(), "STORY-1051");
}

#[test]
fn leased_implementer_head_synthesizes_assignment_when_queue_shifted() {
    let (_dir, storage) = queued_status_fixture(&[
        ("STORY-1054", RequirementStatus::Approved),
        ("STORY-1051", RequirementStatus::InProgress),
    ]);
    let store = storage.load().unwrap();
    let queued: Vec<aida_core::QueueEntry> = storage
        .queue_list("u", false)
        .unwrap()
        .into_iter()
        .filter(|entry| {
            let req = store
                .requirements
                .iter()
                .find(|r| r.id == entry.requirement_id)
                .unwrap();
            req.display_id() != "STORY-1051"
        })
        .collect();
    let lease = implementer_lease("STORY-1051");

    let head = leased_implementer_queue_head(&queued, &store, "u", Some(&lease))
        .expect("assignment should survive a shifted/dequeued queue entry");
    let req = store
        .requirements
        .iter()
        .find(|r| r.id == head.requirement_id)
        .unwrap();

    assert_eq!(req.display_id(), "STORY-1051");
    assert_eq!(head.for_session.as_deref(), Some("lease-1082"));
}

#[test]
fn leased_implementer_head_does_not_resurrect_done_assignment() {
    let (_dir, storage) = queued_status_fixture(&[
        ("STORY-1136", RequirementStatus::Done),
        ("STORY-1137", RequirementStatus::Approved),
    ]);
    let store = storage.load().unwrap();
    let queued: Vec<aida_core::QueueEntry> = storage
        .queue_list("u", false)
        .unwrap()
        .into_iter()
        .filter(|entry| {
            let req = store
                .requirements
                .iter()
                .find(|r| r.id == entry.requirement_id)
                .unwrap();
            req.display_id() != "STORY-1136"
        })
        .collect();
    let lease = implementer_lease("STORY-1136");

    // trace:TASK-1-136 | ai:codex
    assert!(
        leased_implementer_queue_head(&queued, &store, "u", Some(&lease)).is_none(),
        "a Done lease scope should not be synthesized as the queue head"
    );
}

#[test]
fn fresh_pickup_policy_status_table_is_shared_by_surfaces() {
    use RequirementStatus::*;

    let mut store = aida_core::RequirementsStore::default();
    for status in [
        Draft,
        Approved,
        Planned,
        InProgress,
        Done,
        Completed,
        Rejected,
        Superseded,
        NeedsAttention,
    ] {
        let mut r = aida_core::Requirement::new(format!("status {status}"), String::new());
        r.spec_id = Some(format!("SPEC-{status:?}"));
        r.status = status.clone();
        store.requirements = vec![r.clone()];

        let expected = match status {
            Done => QueueFreshPickup::AwaitingMerge,
            Completed | Rejected | Superseded => QueueFreshPickup::Terminal(status.clone()),
            NeedsAttention => {
                QueueFreshPickup::Blocked(aida_core::pickability::BlockedReason::NeedsTriage)
            }
            _ => QueueFreshPickup::Pickable,
        };

        // trace:BUG-1017 | ai:codex
        for surface in [
            "queue list",
            "queue next",
            "queue work --dry-run",
            "queue work",
        ] {
            assert_eq!(
                queue_fresh_pickup_policy(&r, &store, false, None),
                expected,
                "{surface} must share status pickability for {status}"
            );
        }
    }
}

#[test]
fn fresh_pickup_policy_allows_needs_attention_only_with_force() {
    let mut store = aida_core::RequirementsStore::default();
    let mut r = aida_core::Requirement::new("punted".to_string(), String::new());
    r.spec_id = Some("BUG-1017".to_string());
    r.status = RequirementStatus::NeedsAttention;
    store.requirements.push(r.clone());

    assert!(matches!(
        queue_fresh_pickup_policy(&r, &store, false, None),
        QueueFreshPickup::Blocked(aida_core::pickability::BlockedReason::NeedsTriage)
    ));
    assert_eq!(
        queue_fresh_pickup_policy(&r, &store, true, None),
        QueueFreshPickup::Pickable
    );
}

#[test]
fn queue_work_head_skips_done_and_picks_next_fresh_item() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_SESSION_ROLE", "implementer");
    let (_dir, storage) = queued_status_fixture(&[
        ("BUG-1017", RequirementStatus::Done),
        ("BUG-1018", RequirementStatus::Approved),
    ]);

    let plan = resolve_queue_work_plan(&storage, "u", None, None, false, true, false, None)
        .expect("head pickup should skip Done and pick the next fresh item");

    assert_eq!(plan.anchor_display, "BUG-1018");
}

#[test]
fn queue_work_explicit_done_refuses_with_awaiting_merge_hint() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_SESSION_ROLE", "implementer");
    let (_dir, storage) = queued_status_fixture(&[("BUG-1017", RequirementStatus::Done)]);

    let err = resolve_queue_work_plan(
        &storage,
        "u",
        Some("BUG-1017"),
        None,
        false,
        true,
        false,
        None,
    )
    .expect_err("explicit Done pickup must refuse fresh work")
    .to_string();

    assert!(err.contains("Done"), "error should name Done: {err}");
    assert!(
        err.contains("awaiting merge"),
        "error should hint merge path: {err}"
    );
    assert!(
        err.contains("--from-pr") || err.contains("integrate"),
        "error should route away from fresh pickup: {err}"
    );
}

#[test]
fn queue_work_explicit_needs_attention_requires_force() {
    let _env = crate::test_env::EnvVarGuard::set("AIDA_SESSION_ROLE", "implementer");
    let (_dir, storage) = queued_status_fixture(&[("BUG-1017", RequirementStatus::NeedsAttention)]);

    let err = resolve_queue_work_plan(
        &storage,
        "u",
        Some("BUG-1017"),
        None,
        false,
        true,
        false,
        None,
    )
    .expect_err("NeedsAttention pickup must refuse without force")
    .to_string();
    assert!(
        err.contains("needs-triage"),
        "error should name triage: {err}"
    );

    let plan = resolve_queue_work_plan(
        &storage,
        "u",
        Some("BUG-1017"),
        None,
        false,
        false,
        true,
        None,
    )
    .expect("force should allow a deliberate NeedsAttention claim");
    assert_eq!(plan.anchor_display, "BUG-1017");
}

#[test]
fn orchestrated_reviewer_can_pick_current_implementer_routed_spec() {
    let token = uuid::Uuid::now_v7().to_string();
    let (dir, storage) = queued_status_fixture_for_user(
        &[("BUG-1106", RequirementStatus::InProgress)],
        "role:implementer",
        "implementer",
    );
    crate::drain_state::DrainState::new_single("BUG-1106", &token, false)
        .write(dir.path())
        .unwrap();
    let store = storage.load().unwrap();
    let req = store
        .requirements
        .iter()
        .find(|r| r.display_id() == "BUG-1106")
        .unwrap();
    let mut visible_entries = storage.queue_list("reviewer-user", false).unwrap();

    assert!(orchestrator_authorizes_explicit_queue_pickup_with_token(
        &storage, "BUG-1106", req, &token
    ));
    merge_current_spec_queue_entries(&storage, &mut visible_entries, req.id);

    // trace:BUG-1106 | ai:codex
    let entry = visible_entries
        .iter()
        .find(|entry| entry.requirement_id == req.id)
        .expect("orchestrator-authorized merge should surface the queued row");
    assert_eq!(entry.user_id, "role:implementer");
    assert_eq!(entry.for_role.as_deref(), Some("implementer"));
}

/// Reviewer role + PR scope → `/aida-review --pr N`.
#[test]
fn prompt_reviewer_pr_passes_number() {
    let e = resolved("STORY-X", entry(Uuid::now_v7(), Some("reviewer"), None));
    let plan = plan_with(QueueWorkMode::Cluster, "PR-11", vec![e]);
    assert_eq!(
        derive_queue_work_prompt(&plan, "reviewer", false, false, None),
        reviewer_prompt_for_round(11, 1)
    );
}

#[test]
fn prompt_agent_gate_reuses_review_verdict_skill_for_custom_role() {
    let _guard = crate::test_env::env_lock();
    let old_name = std::env::var("AIDA_AGENT_GATE_NAME").ok();
    let old_role = std::env::var("AIDA_AGENT_GATE_ROLE").ok();
    std::env::set_var("AIDA_AGENT_GATE_NAME", "security-review");
    std::env::set_var("AIDA_AGENT_GATE_ROLE", "security-reviewer");
    let e = resolved("STORY-X", entry(Uuid::now_v7(), Some("reviewer"), None));
    let plan = plan_with(QueueWorkMode::Cluster, "PR-11", vec![e]);

    let prompt = derive_queue_work_prompt(&plan, "security-reviewer", false, false, None);

    match old_name {
        Some(v) => std::env::set_var("AIDA_AGENT_GATE_NAME", v),
        None => std::env::remove_var("AIDA_AGENT_GATE_NAME"),
    }
    match old_role {
        Some(v) => std::env::set_var("AIDA_AGENT_GATE_ROLE", v),
        None => std::env::remove_var("AIDA_AGENT_GATE_ROLE"),
    }
    assert!(prompt.starts_with("/aida-review --pr 11"), "{prompt}");
    assert!(prompt.contains("Agent gate: security-review"), "{prompt}");
    assert!(prompt.contains("Gate role: security-reviewer"), "{prompt}");
}

/// Reviewer role + non-PR scope → bare `/aida-review`.
#[test]
fn prompt_reviewer_non_pr_is_bare() {
    let e = resolved("STORY-X", entry(Uuid::now_v7(), Some("reviewer"), None));
    let plan = plan_with(QueueWorkMode::Cluster, "EPIC-20", vec![e]);
    assert_eq!(
        derive_queue_work_prompt(&plan, "reviewer", false, false, None),
        "/aida-review"
    );
}

/// Implementer + item mode → `/aida-pickup <ID>` (focus directive).
#[test]
fn prompt_implementer_item_passes_focus() {
    let e = resolved("BUG-83", entry(Uuid::now_v7(), Some("implementer"), None));
    let plan = QueueWorkPlan {
        mode: QueueWorkMode::Item,
        entries: vec![e],
        scope: "EPIC-20".into(),
        review_target: None,
        anchor_display: "BUG-83".into(),
        anchor_title: "title".into(),
    };
    assert_eq!(
        derive_queue_work_prompt(&plan, "implementer", false, false, None),
        "/aida-pickup BUG-83"
    );
}

/// TASK-1276: a groomed spike rides the normal PR lifecycle, but phase 1 must
/// receive the research/report contract instead of treating it as code work.
#[test]
fn prompt_spike_item_names_research_lane_and_report_contract() {
    let e = resolved("SPIKE-82", entry(Uuid::now_v7(), Some("implementer"), None));
    let plan = QueueWorkPlan {
        mode: QueueWorkMode::Item,
        entries: vec![e],
        scope: "SPIKE-82".into(),
        review_target: None,
        anchor_display: "SPIKE-82".into(),
        anchor_title: "night-shift report".into(),
    };
    let prompt = derive_queue_work_prompt(&plan, "implementer", false, false, None);
    assert!(prompt.starts_with("/aida-pickup SPIKE-82"), "{prompt}");
    assert!(prompt.contains("Research-lane contract"), "{prompt}");
    assert!(prompt.contains("docs/spikes/<date>-<slug>.md"), "{prompt}");
    assert!(prompt.contains("open a PR"), "{prompt}");
    assert!(prompt.contains("Do not make or apply"), "{prompt}");
}

/// STORY-1226: a seat's due `[schedule]` jobs lead the pickup prompt, and the
/// pickup line stays last; an empty block leaves the prompt untouched.
// trace:STORY-1226 | ai:claude
#[test]
fn pickup_prompt_leads_with_due_jobs_for_role() {
    let _guard = crate::test_env::env_lock();
    std::env::remove_var("AIDA_AGENT_GATE_NAME");
    let e = resolved("TASK-1226", entry(Uuid::now_v7(), Some("advisor"), None));
    let plan = QueueWorkPlan {
        mode: QueueWorkMode::Item,
        entries: vec![e],
        scope: "TASK-1226".into(),
        review_target: None,
        anchor_display: "TASK-1226".into(),
        anchor_title: "title".into(),
    };
    let due = vec![crate::maintenance_schedule::DueJob {
        name: "mailbox-triage".into(),
        kind: crate::maintenance_schedule::JobKind::Seat,
        seats: vec!["advisor".into()],
        schedule: "every 30m".into(),
        reason: "every 30m".into(),
        prompt: Some("triage the mailbox".into()),
        command: None,
        last_run: None,
        last_by: None,
        due_since: None,
        failure: None,
    }];
    let block = crate::maintenance_schedule::render_due_jobs_block(&due, "advisor");
    let base = derive_queue_work_prompt(&plan, "advisor", false, false, None);
    let prompt = prepend_due_jobs(base.clone(), &block);
    assert!(
        prompt.starts_with("DUE JOBS (seat: advisor):\n"),
        "{prompt}"
    );
    assert!(
        prompt.contains("- mailbox-triage (every 30m, never run) → triage the mailbox"),
        "{prompt}"
    );
    assert!(prompt.contains("aida schedule done <job>"), "{prompt}");
    assert!(
        prompt.ends_with(&base),
        "pickup line must stay last: {prompt}"
    );
    // Nothing due → unchanged.
    assert_eq!(prepend_due_jobs(base.clone(), ""), base);
    // An agent gate never gets the block.
    std::env::set_var("AIDA_AGENT_GATE_NAME", "security");
    assert_eq!(prepend_due_jobs(base.clone(), &block), base);
    std::env::remove_var("AIDA_AGENT_GATE_NAME");
}

/// BUG-814: a rework pickup with a blocking review verdict must lead with the
/// findings before `/aida-pickup`, so the implementer treats the review as the
/// acceptance delta instead of silently passing the same commit back.
// trace:BUG-814 | ai:codex
#[test]
fn prompt_implementer_item_leads_with_review_findings() {
    let e = resolved("BUG-814", entry(Uuid::now_v7(), Some("implementer"), None));
    let plan = QueueWorkPlan {
        mode: QueueWorkMode::Item,
        entries: vec![e],
        scope: "BUG-814".into(),
        review_target: None,
        anchor_display: "BUG-814".into(),
        anchor_title: "title".into(),
    };
    let findings = "REVIEW FINDINGS TO ADDRESS (PR #1637):\nFindings:\n1. fix the prompt";
    let prompt = derive_queue_work_prompt(&plan, "implementer", false, false, Some(findings));
    // BUG-1213: the findings now sit under the round header ("ROUND N — ITEMS
    // STILL OPEN"), but they still lead the prompt and precede the pickup.
    assert!(prompt.starts_with("ROUND "), "{prompt}");
    let findings_at = prompt.find(findings).expect("findings present");
    let pickup_at = prompt.find("/aida-pickup").expect("pickup present");
    assert!(
        findings_at < pickup_at,
        "findings must precede the pickup: {prompt}"
    );
    assert!(prompt.ends_with("/aida-pickup BUG-814"), "{prompt}");
}

/// BUG-814: orchestrated reviews record PR-keyed verdict files, while rework is
/// invoked with the spec id. The lookup must bridge that key mismatch when the
/// reviewer summary/findings name the spec.
// trace:BUG-814 | ai:codex
#[test]
fn rework_findings_lookup_reads_pr_keyed_verdict_for_spec() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida").join("review-verdicts")).unwrap();
    std::fs::write(
        root.join(".aida")
            .join("review-verdicts")
            .join("PR-1637.json"),
        r#"{
            "verdict":"RequestChanges",
            "summary":"BUG-814 still hides review findings",
            "comment_url":"https://github.com/o/r/pull/1637#issuecomment-1",
            "findings":["BUG-814 prompt omits the RequestChanges text"]
        }"#,
    )
    .unwrap();

    let block = rework_findings_block_for_spec(root, "BUG-814", "BUG-814")
        .expect("PR-keyed verdict should be found");
    assert!(block.contains("REVIEW FINDINGS TO ADDRESS (PR #1637)"));
    assert!(block.contains("1. BUG-814 prompt omits the RequestChanges text"));
}

/// TASK-1191: a lone unrelated blocking verdict must not be attributed to the
/// reworked spec just because it is the only verdict file on disk.
// trace:TASK-1191 | ai:codex
#[test]
fn rework_findings_lookup_ignores_lone_unrelated_blocking_verdict() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida").join("review-verdicts")).unwrap();
    std::fs::write(
        root.join(".aida")
            .join("review-verdicts")
            .join("PR-1638.json"),
        r#"{
            "verdict":"RequestChanges",
            "summary":"BUG-999 still needs changes",
            "comment_url":"https://github.com/o/r/pull/1638#issuecomment-1",
            "findings":["BUG-999 prompt omits the RequestChanges text"]
        }"#,
    )
    .unwrap();

    let block = rework_findings_block_for_spec(root, "TASK-1191", "TASK-1191");
    assert!(block.is_none(), "{block:?}");
}

/// Implementer + cluster/head → `/aida-pickup --auto-first`
/// (manifest carries the context; STORY-42 pre-flight is the consent
// point so the skill skips its own confirm). trace:TASK-86 | ai:claude
#[test]
fn prompt_implementer_cluster_is_auto_first() {
    let e = resolved("BUG-83", entry(Uuid::now_v7(), Some("implementer"), None));
    let plan = plan_with(QueueWorkMode::Cluster, "EPIC-20", vec![e]);
    assert_eq!(
        derive_queue_work_prompt(&plan, "implementer", false, false, None),
        "/aida-pickup --auto-first"
    );
}

/// Implementer + head mode → also `/aida-pickup --auto-first`.
/// The no-arg invocation explicitly opts into queue-driven flow, so
/// the confirm is the same friction-without-value as cluster mode.
// trace:TASK-86 | ai:claude
#[test]
fn prompt_implementer_head_is_auto_first() {
    let e = resolved("BUG-83", entry(Uuid::now_v7(), Some("implementer"), None));
    let plan = plan_with(QueueWorkMode::Head, "EPIC-20", vec![e]);
    assert_eq!(
        derive_queue_work_prompt(&plan, "implementer", false, false, None),
        "/aida-pickup --auto-first"
    );
}

/// STORY-265: plan-only mode runs /aida-plan (not /aida-pickup) with the
/// item focus, so the session writes a plan instead of implementing.
#[test]
fn prompt_plan_only_runs_aida_plan() {
    let e = resolved("BUG-83", entry(Uuid::now_v7(), Some("implementer"), None));
    let plan = QueueWorkPlan {
        mode: QueueWorkMode::Item,
        entries: vec![e],
        scope: "EPIC-20".into(),
        review_target: None,
        anchor_display: "BUG-83".into(),
        anchor_title: "title".into(),
    };
    assert_eq!(
        derive_queue_work_prompt(&plan, "implementer", true, false, None),
        "/aida-plan BUG-83"
    );
}

/// STORY-735: guided keystone mode runs /aida-guided-implement (the
/// structured decision dialog) with the spec focus, not /aida-pickup.
#[test]
fn prompt_guided_runs_guided_implement() {
    let e = resolved("STORY-7", entry(Uuid::now_v7(), Some("implementer"), None));
    let plan = QueueWorkPlan {
        mode: QueueWorkMode::Item,
        entries: vec![e],
        scope: "EPIC-20".into(),
        review_target: None,
        anchor_display: "STORY-7".into(),
        anchor_title: "title".into(),
    };
    assert_eq!(
        derive_queue_work_prompt(&plan, "implementer", false, true, None),
        "/aida-guided-implement STORY-7"
    );
}

/// STORY-265: plan-only defaults the permission mode to `plan` (read-only),
/// overriding env/config/bypass — but an explicit --permission-mode wins.
#[test]
fn plan_only_defaults_permission_to_plan_but_flag_wins() {
    // no explicit flag: plan-only forces read-only `plan` even with bypass on
    let (m, o) = resolve_queue_work_permission_mode(None, Some("auto"), None, true, true);
    assert_eq!(m.as_deref(), Some("plan"));
    assert!(o.contains("plan-only"));
    // explicit --permission-mode flag still wins over plan-only
    let (m, _) = resolve_queue_work_permission_mode(Some("acceptEdits"), None, None, false, true);
    assert_eq!(m.as_deref(), Some("acceptEdits"));
}

/// Title shape "Review PR-11: …" overrides for_scope and parent
/// EPIC — review-mode session for PR-11.
#[test]
fn scope_review_title_wins_over_for_scope() {
    let mut review = req("STORY-9", None, RequirementType::Story);
    review.title = "Review PR-11: clean up sync flow".into();
    let qe = entry(review.id, Some("reviewer"), Some("EPIC-99"));
    let (scope, target) = derive_scope_from_entry(&qe, &review);
    assert_eq!(scope, "PR-11");
    assert!(target.is_some());
}

/// for_scope wins even for a child story (its parent-epic relationship is
/// no longer consulted for the session scope — BUG-431 #1).
#[test]
fn scope_for_scope_beats_parent_epic() {
    let mut bug = req("BUG-1", None, RequirementType::Bug);
    bug.relationships.push(Relationship {
        rel_type: RelationshipType::Child,
        target_id: Uuid::now_v7(), // a parent epic, irrelevant to scope now
        created_at: Some(chrono::Utc::now()),
        created_by: Some("t".into()),
    });
    let qe = entry(bug.id, Some("implementer"), Some("EPIC-21"));
    let (scope, _) = derive_scope_from_entry(&qe, &bug);
    assert_eq!(scope, "EPIC-21");
}

/// BUG-739: legacy rows auto-stamped with the generic Claude harness lease
/// must not route queue-work into the shared harness checkout.
#[test]
fn scope_ignores_legacy_harness_worktree_for_scope() {
    let bug = req("BUG-739", None, RequirementType::Bug);
    let qe = entry(
        bug.id,
        Some("implementer"),
        Some(worktree_lease::HARNESS_WORKTREE_SCOPE),
    );
    let (scope, target) = derive_scope_from_entry(&qe, &bug);
    assert_eq!(scope, "BUG-739");
    assert!(target.is_none());
}

/// BUG-431 #1: no for_scope → a child story scopes to its OWN id, NOT the
/// parent epic. Previously this fell back to the parent EPIC, so every
/// same-epic story in a drain contended for one epic scope (worktree +
/// branch collision, sibling lease-block, multi-spec PR). Each sibling
/// must get its own scope so the drain progresses without contention.
#[test]
fn scope_child_story_does_not_inherit_parent_epic() {
    let mut bug = req("BUG-1", None, RequirementType::Bug);
    // A child-of-epic relationship — the exact shape that used to pull the
    // session scope up to the parent epic.
    bug.relationships.push(Relationship {
        rel_type: RelationshipType::Child,
        target_id: Uuid::now_v7(),
        created_at: Some(chrono::Utc::now()),
        created_by: Some("t".into()),
    });
    let qe = entry(bug.id, Some("implementer"), None);
    let (scope, _) = derive_scope_from_entry(&qe, &bug);
    assert_eq!(
        scope, "BUG-1",
        "a child story's session must scope to its own id, not its parent epic"
    );
}

/// No for_scope and no parent EPIC → falls back to req's own
/// display id.
#[test]
fn scope_falls_back_to_own_id() {
    let bug = req("BUG-1", None, RequirementType::Bug);
    let qe = entry(bug.id, Some("implementer"), None);
    let (scope, _) = derive_scope_from_entry(&qe, &bug);
    assert_eq!(scope, "BUG-1");
}

/// BUG-739: queue-add's implicit cwd lease routing must skip the generic
/// harness scope while keeping explicit `--scope harness-worktree` meaningful.
#[test]
fn queue_add_scope_routing_skips_implicit_harness_scope() {
    let lease = lease_for("harness", worktree_lease::HARNESS_WORKTREE_SCOPE, 1);
    assert_eq!(
        queue_add_for_scope_routing(false, None, None, Some(&lease)),
        None
    );
    assert_eq!(
        queue_add_for_scope_routing(
            false,
            Some(worktree_lease::HARNESS_WORKTREE_SCOPE),
            None,
            Some(&lease),
        )
        .as_deref(),
        Some(worktree_lease::HARNESS_WORKTREE_SCOPE)
    );
}

#[test]
fn queue_add_scope_routing_keeps_real_implicit_scope() {
    let lease = lease_for("task", "TASK-1156", 1);
    assert_eq!(
        queue_add_for_scope_routing(false, None, None, Some(&lease)).as_deref(),
        Some("TASK-1156")
    );
}

#[test]
fn queue_add_hidden_view_reason_rejects_parked_specs() {
    let mut deferred = req("TASK-1135", None, RequirementType::Task);
    deferred.deferred = true;
    assert_eq!(
        queue_add_hidden_view_reason(&deferred),
        Some("deferred"),
        "deferred specs are parked outside the default queue projection"
    );

    let mut archived = req("TASK-1136", None, RequirementType::Task);
    archived.archived = true;
    assert_eq!(
        queue_add_hidden_view_reason(&archived),
        Some("archived"),
        "archived specs are hidden from the default queue projection"
    );

    let active = req("TASK-1137", None, RequirementType::Task);
    assert_eq!(queue_add_hidden_view_reason(&active), None);
}

#[test]
fn fresh_pickup_policy_skips_deferred_specs() {
    let mut deferred = req("TASK-1205", None, RequirementType::Task);
    deferred.status = RequirementStatus::Approved;
    deferred.deferred = true;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(deferred.clone());

    let policy = queue_fresh_pickup_policy(&deferred, &store, false, None);
    assert_eq!(policy, QueueFreshPickup::Deferred);
    assert_eq!(
        queue_fresh_pickup_reason_label(&policy).as_deref(),
        Some("deferred — skipped")
    );
}

// trace:BUG-1120 | ai:codex
#[test]
fn drain_pickup_policy_skips_guided_operator_and_decide_specs() {
    let mut store = aida_core::RequirementsStore::default();
    for mode in [
        aida_core::ExecutionMode::Guided,
        aida_core::ExecutionMode::Operator,
        aida_core::ExecutionMode::Decide,
    ] {
        let mut r = req(&format!("TASK-{mode:?}"), None, RequirementType::Task);
        r.status = RequirementStatus::Approved;
        r.execution_mode = Some(mode);
        store.requirements = vec![r.clone()];

        let policy = queue_drain_pickup_policy(&r, &store, false, None);
        assert_eq!(policy, QueueFreshPickup::NeedsGuidedOrOperatorSession(mode));
        assert!(queue_fresh_pickup_reason_label(&policy)
            .expect("mode skip has a label")
            .contains("needs guided/operator session"));
        assert!(queue_fresh_pickup_reason_label(&policy)
            .expect("mode skip has a label")
            .contains("aida derisk <ID>"));
        assert_eq!(
            queue_fresh_pickup_policy(&r, &store, false, None),
            QueueFreshPickup::Pickable
        );
    }
}

// trace:STORY-1125 | ai:codex
#[test]
fn drain_pickup_policy_skips_release_meta_tasks() {
    let mut release = req("STORY-1125", None, RequirementType::Story);
    release.status = RequirementStatus::Approved;
    release.execution_mode = Some(aida_core::ExecutionMode::Drain);
    release
        .tags
        .insert("release workflow meta-task aida:release".to_string());
    let mut store = aida_core::RequirementsStore::default();
    store.requirements = vec![release.clone()];

    let policy = queue_drain_pickup_policy(&release, &store, false, None);
    assert_eq!(policy, QueueFreshPickup::NeedsReleaseOperatorSession);
    assert!(queue_fresh_pickup_reason_label(&policy)
        .expect("release skip has a label")
        .contains("/aida-release"));
    assert_eq!(
        queue_fresh_pickup_policy(&release, &store, false, None),
        QueueFreshPickup::Pickable,
        "release meta-tasks stay queueable for guided/operator pickup"
    );
}

// trace:BUG-1120 | ai:codex
#[test]
fn explicit_auto_complete_guard_names_guided_path() {
    let msg = guarded_execution_mode_drain_message("BUG-1120", aida_core::ExecutionMode::Guided);
    assert!(msg.contains("skipped BUG-1120"));
    assert!(msg.contains("needs guided/operator session"));
    assert!(msg.contains("aida queue work BUG-1120 --guided"));
    assert!(msg.contains("aida do BUG-1120"));
}

/// spec_matches walks uuid, spec_id (case-insensitive), and
// agreed_id (case-insensitive). trace:STORY-42 | ai:claude
#[test]
fn spec_matches_covers_uuid_and_ids() {
    let mut r = req("BUG-1-099", Some("BUG-42"), RequirementType::Bug);
    r.id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
    assert!(spec_matches(&r, "11111111-1111-1111-1111-111111111111"));
    assert!(spec_matches(&r, "BUG-1-099"));
    assert!(spec_matches(&r, "bug-1-099")); // case-insensitive
    assert!(spec_matches(&r, "BUG-42")); // agreed_id
    assert!(spec_matches(&r, "bug-42"));
    assert!(!spec_matches(&r, "BUG-99"));
}

#[test]
fn queue_work_identity_preserves_node_qualified_spec_id() {
    let r = req("TASK-1-127", Some("TASK-151"), RequirementType::Task);
    assert!(spec_matches(&r, "TASK-1-127"));
    assert!(spec_matches(&r, "TASK-151"));

    let (scope, review_target) = derive_scope_from_req_id(&r);
    assert_eq!(scope, "TASK-1-127");
    assert_eq!(review_target, None);

    let resolved = build_resolved_entry(entry(r.id, Some("implementer"), None), &r);
    assert_eq!(resolved.spec_id, "TASK-1-127");
}

/// BUG-1244: queue rows carry routing only; pickup status is always joined
/// from the current requirement. A stale prior `Done` plan must not suppress a
/// spec an operator reset to Approved for fresh work.
// trace:BUG-1244 | ai:codex
#[test]
fn resolved_queue_entry_uses_current_spec_status_after_reopen() {
    let mut r = req("BUG-1244", None, RequirementType::Bug);
    r.set_status_from_str("Approved");
    let resolved = build_resolved_entry(entry(r.id, Some("implementer"), None), &r);
    assert_eq!(resolved.status_at_plan, "Approved");
}

/// BUG-366: the "awaiting review" hint must be an unambiguous reviewer
/// pickup, not a bare `aida queue work PR-N` that invites implementer-drain
// flags the PR-N path can't resolve. trace:BUG-366 | ai:claude
#[test]
fn review_pickup_hint_names_reviewer_role() {
    assert_eq!(
        review_pickup_hint(250),
        "aida queue work PR-250 --for reviewer"
    );
}

fn lease_for(id: &str, scope: &str, age_secs: i64) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_lowercase(),
        owner: "u".into(),
        worktree_path: std::path::PathBuf::from(format!("/tmp/wt-{}", id)),
        branch: format!("br-{}", id),
        started_at: chrono::Utc::now() - chrono::Duration::seconds(age_secs),
        hostname: "h".into(),
        role: Some("implementer".into()),
        creator_pid: None,
        creator_pid_start_time: None,
        active_pid: None,
        active_pid_start_time: None,
        cargo_target_dir: None,
        parent_project_root: None,
        pr_head_sha: None,
        pr_base_sha: None,
        pr_base_ref: None,
        zen_intent_token: None,
        escalated_to_human: None,
        parent_branch: None,
        parent_branch_sha: None,
        review_verb: false,
        claim_verb: false,
        manual_enter_at: None,
    }
}

// No leases → no conflict. trace:TASK-81 | ai:claude
#[test]
fn lease_conflict_empty() {
    assert!(find_scope_lease_conflict(&[], "TASK-81").is_none());
}

// Lease on a different scope → no conflict. trace:TASK-81 | ai:claude
#[test]
fn lease_conflict_mismatched_scope() {
    let leases = vec![lease_for("aaaa", "EPIC-20", 10)];
    assert!(find_scope_lease_conflict(&leases, "TASK-81").is_none());
}

/// Exact scope match → that lease is the conflict.
// trace:TASK-81 | ai:claude
#[test]
fn lease_conflict_exact_match() {
    let leases = vec![lease_for("aaaa", "TASK-81", 10)];
    let got = find_scope_lease_conflict(&leases, "TASK-81").unwrap();
    assert_eq!(got.id, "aaaa");
}

/// Case-insensitive scope match — `aida queue work task-81` should
// still detect a lease owning `TASK-81`. trace:TASK-81 | ai:claude
#[test]
fn lease_conflict_case_insensitive() {
    let leases = vec![lease_for("aaaa", "TASK-81", 10)];
    let got = find_scope_lease_conflict(&leases, "task-81").unwrap();
    assert_eq!(got.id, "aaaa");
}

/// Multiple leases on the same scope → freshest (smallest age) wins,
/// so `session_end` targets the live one rather than a stale ghost.
// trace:TASK-81 | ai:claude
#[test]
fn lease_conflict_picks_freshest() {
    let leases = vec![
        lease_for("oldold", "TASK-81", 600),
        lease_for("freshh", "TASK-81", 10),
    ];
    let got = find_scope_lease_conflict(&leases, "TASK-81").unwrap();
    assert_eq!(got.id, "freshh");
}

/// --permission-mode flag beats everything else (incl. the bypass knob).
// trace:TASK-84 trace:STORY-495 | ai:claude
#[test]
fn permission_mode_flag_wins() {
    let (m, o) = resolve_queue_work_permission_mode(
        Some("plan"),
        Some("auto"),
        Some("default"),
        true,
        false,
    );
    assert_eq!(m.as_deref(), Some("plan"));
    assert_eq!(o, "--permission-mode flag");
}

/// AIDA_PERMISSION_MODE env wins over config + the bypass knob.
// trace:TASK-84 trace:STORY-495 | ai:claude
#[test]
fn permission_mode_env_beats_config() {
    let (m, o) =
        resolve_queue_work_permission_mode(None, Some("auto"), Some("default"), true, false);
    assert_eq!(m.as_deref(), Some("auto"));
    assert_eq!(o, "AIDA_PERMISSION_MODE env");
}

/// config.toml [behavior] beats the bypass knob.
// trace:TASK-84 trace:STORY-495 | ai:claude
#[test]
fn permission_mode_config_beats_worktree_default() {
    let (m, o) = resolve_queue_work_permission_mode(None, None, Some("acceptEdits"), true, false);
    assert_eq!(m.as_deref(), Some("acceptEdits"));
    assert_eq!(o, ".aida/config.toml");
}

/// STORY-495: the `[agents] bypass` knob (no other overrides) →
// bypassPermissions. trace:STORY-495 | ai:claude
#[test]
fn permission_mode_bypass_knob_injects_bypass() {
    let (m, o) = resolve_queue_work_permission_mode(None, None, None, true, false);
    assert_eq!(m.as_deref(), Some("bypassPermissions"));
    assert_eq!(o, "[agents] bypass knob");
}

/// STORY-495: faithful default — no flag, no env, no config, knob off →
/// native (None), so no `--permission-mode` is injected.
// trace:STORY-495 | ai:claude
#[test]
fn permission_mode_faithful_default_is_native() {
    let (m, o) = resolve_queue_work_permission_mode(None, None, None, false, false);
    assert_eq!(m, None);
    assert_eq!(o, "native (faithful default)");
}

/// Empty string from flag/env/config is treated as absent (so an empty
/// shell variable doesn't accidentally pin a mode); with the knob on the
// resolution falls through to the bypass knob. trace:TASK-84 trace:STORY-495 | ai:claude
#[test]
fn permission_mode_empty_strings_are_ignored() {
    let (m, o) = resolve_queue_work_permission_mode(Some(""), Some(""), Some(""), true, false);
    assert_eq!(m.as_deref(), Some("bypassPermissions"));
    assert_eq!(o, "[agents] bypass knob");
}

// --- AutonomyMode (STORY-287) -----------------------------------------

/// No flags → the human is driving; every prompt pauses.
// trace:STORY-287 | ai:claude
#[test]
fn autonomy_mode_default_when_no_flags() {
    assert_eq!(resolve_autonomy_mode(false, false), AutonomyMode::Default);
}

/// `--zen` alone → advisor-on-standby mode.
// trace:STORY-287 | ai:claude
#[test]
fn autonomy_mode_zen_flag_alone() {
    assert_eq!(resolve_autonomy_mode(true, false), AutonomyMode::Zen);
}

/// `--no-human` alone → the headless drain mode.
// trace:STORY-287 | ai:claude
#[test]
fn autonomy_mode_no_human_alone() {
    assert_eq!(resolve_autonomy_mode(false, true), AutonomyMode::NoHuman);
}

/// Precedence: `--no-human --zen` resolves to `NoHuman` — the stronger
/// mode wins (the dispatch also warns and clears `AIDA_ZEN`).
// trace:STORY-287 | ai:claude
#[test]
fn autonomy_mode_no_human_beats_zen() {
    assert_eq!(resolve_autonomy_mode(true, true), AutonomyMode::NoHuman);
}

// --- AutonomyMode::resolve_run (ADR-7 / ADR-10) -----------------------
// The in-flight typed resolution that replaces the bare `AIDA_ZEN` re-read
// inside run_auto_complete. Same precedence as resolve_autonomy_mode, but
// expressed over the typed `--no-human` mode + the zen-INTENT-TOKEN
// presence (not a bare env read), so a leaked AIDA_ZEN is leak-resistant.

/// No `--no-human`, no zen token → the human is driving. (ADR-10)
#[test]
fn resolve_run_default_when_nothing_set() {
    assert_eq!(
        AutonomyMode::resolve_run(None, false),
        AutonomyMode::Default
    );
}

/// Zen-intent token present (and no `--no-human`) → a supervised zen run
/// under ADR-10.
#[test]
fn resolve_run_zen_when_token_present() {
    let m = AutonomyMode::resolve_run(None, true);
    assert_eq!(m, AutonomyMode::Zen);
    assert!(m.is_zen());
}

/// `--no-human` wins over a zen token (the stronger mode) — symmetric with
/// resolve_autonomy_mode's precedence. (ADR-10)
#[test]
fn resolve_run_no_human_beats_zen_token() {
    assert_eq!(
        AutonomyMode::resolve_run(Some(auto_complete::NoHumanMode::Both), true),
        AutonomyMode::NoHuman
    );
    assert_eq!(
        AutonomyMode::resolve_run(Some(auto_complete::NoHumanMode::ReviewerOnly), false),
        AutonomyMode::NoHuman
    );
}

/// BUG-237 leak-resistance preserved: NO zen token (a leaked `AIDA_ZEN=1`
/// carries none) → NOT recorded as a zen run. (ADR-10 / BUG-237)
#[test]
fn resolve_run_no_token_is_not_zen() {
    let m = AutonomyMode::resolve_run(None, false);
    assert!(!m.is_zen());
}

/// ADR-10: the phase driver CARRIES the resolved-once typed autonomy mode
/// as a field (symmetric with `no_human`), and the engine's zen predicate
/// (`is_zen_run`) reads that carried field — NOT a bare `AIDA_ZEN` env
/// read. This is the drain-state zen-stamping source in `run_auto_complete`.
/// The three modes round-trip through construction unchanged, so the
/// autonomy behavior is byte-identical to the pre-ADR-10 env-derived read.
// trace:ADR-10 | ai:claude
#[test]
fn phase_driver_carries_autonomy_mode_and_is_zen_run_reads_the_field() {
    fn build(mode: AutonomyMode) -> RealPhaseDriver {
        RealPhaseDriver::new(
            std::env::temp_dir().join(format!("aida-adr10-{}", uuid::Uuid::now_v7())),
            "ADR-10".to_string(),
            "test-queue".to_string(),
            None,
            false,
            // `no_human` is independent of the carried autonomy mode here —
            // the field is the SOLE source `is_zen_run` consults.
            None,
            mode,
            "test-run".to_string(),
            false,
            false,
            false,
            false,
            auto_complete::LifecycleSkip::none(),
            auto_complete::AutoCompleteVariant::Full,
        )
    }
    // The field is stored verbatim, and `is_zen_run()` reads it.
    assert_eq!(build(AutonomyMode::Zen).autonomy_mode, AutonomyMode::Zen);
    assert!(build(AutonomyMode::Zen).is_zen_run());
    assert!(!build(AutonomyMode::Default).is_zen_run());
    assert!(!build(AutonomyMode::NoHuman).is_zen_run());
    // Guard against a future refactor that silently re-reads the env: the
    // field wins regardless of what `AIDA_ZEN` is set to in the process.
    // (No env mutation here — asserting the field is self-contained.)
    assert_eq!(
        build(AutonomyMode::Default).autonomy_mode,
        AutonomyMode::Default
    );
}

// --- resolve_drain_alias (TASK-578) -----------------------------------

/// `--drain` off is a pure identity map — the operator's flags pass through
// untouched. trace:TASK-578 | ai:claude
#[test]
fn drain_alias_off_is_identity() {
    let r = resolve_drain_alias(
        false,
        Some("through-ci"),
        Some("reviewer-only"),
        Some(3),
        10,
    );
    assert_eq!(r.auto_complete.as_deref(), Some("through-ci"));
    assert_eq!(r.no_human.as_deref(), Some("reviewer-only"));
    assert_eq!(r.max, Some(3));

    let empty = resolve_drain_alias(false, None, None, None, 10);
    assert_eq!(empty.auto_complete, None);
    assert_eq!(empty.no_human, None);
    assert_eq!(empty.max, None);
}

/// Bare `--drain` expands to the full headless drain bounded by the queue
// size. trace:TASK-578 | ai:claude
#[test]
fn drain_alias_bare_expands_to_full_headless_queue_sized() {
    let r = resolve_drain_alias(true, None, None, None, 7);
    assert_eq!(r.auto_complete.as_deref(), Some("full"));
    assert_eq!(r.no_human.as_deref(), Some("both"));
    assert_eq!(r.max, Some(7));
}

/// An unknown / empty queue size falls back to the spec's `--max 99`.
// trace:TASK-578 | ai:claude
#[test]
fn drain_alias_unknown_queue_size_falls_back_to_99() {
    let r = resolve_drain_alias(true, None, None, None, 0);
    assert_eq!(r.max, Some(99));
}

/// Explicit flags always win over the `--drain` defaults — the alias never
// overwrites an operator-supplied value. trace:TASK-578 | ai:claude
#[test]
fn drain_alias_explicit_flags_override_defaults() {
    let r = resolve_drain_alias(
        true,
        Some("through-merge"),
        Some("reviewer-only"),
        Some(2),
        50,
    );
    assert_eq!(r.auto_complete.as_deref(), Some("through-merge"));
    assert_eq!(r.no_human.as_deref(), Some("reviewer-only"));
    assert_eq!(r.max, Some(2));
}

/// Mixed: `--drain --max 3` keeps the explicit cap but still defaults the
// autonomy fields. trace:TASK-578 | ai:claude
#[test]
fn drain_alias_partial_override_keeps_other_defaults() {
    let r = resolve_drain_alias(true, None, None, Some(3), 50);
    assert_eq!(r.auto_complete.as_deref(), Some("full"));
    assert_eq!(r.no_human.as_deref(), Some("both"));
    assert_eq!(r.max, Some(3));
}

// --- TASK-306: --no-human kickoff gate --------------------------------

/// STORY-276: the gate keys purely off acknowledgement — ack'd → proceed
/// silently, otherwise the banner + prompt. `both` is no longer rejected
/// (the headless implementer ships); it is acknowledged like any mode.
// trace:TASK-306, STORY-276
#[test]
fn no_human_gate_keys_off_acknowledgement() {
    assert_eq!(classify_no_human_gate(false), NoHumanGate::NeedsAck);
    assert_eq!(classify_no_human_gate(true), NoHumanGate::Acknowledged);
}

/// STORY-276: the scope line differs by mode — `both` names the headless
/// implementer + the punt safety net; `reviewer-only` says phase 1 stays
// interactive. trace:STORY-276
#[test]
fn no_human_scope_line_differs_by_mode() {
    let both = no_human_scope_line(auto_complete::NoHumanMode::Both);
    assert!(both.contains("both"), "{both}");
    assert!(both.contains("punts"), "{both}");
    let reviewer = no_human_scope_line(auto_complete::NoHumanMode::ReviewerOnly);
    assert!(
        reviewer.contains("reviewer phase runs headless"),
        "{reviewer}"
    );
    assert!(reviewer.contains("interactive"), "{reviewer}");
}

// trace:BUG-740 | ai:codex
#[test]
fn non_tty_interactive_implementer_refuses_before_side_effects() {
    let err = non_tty_interactive_implementer_preflight(None, false, true, false).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("aida do needs a terminal"), "{msg}");
    assert!(msg.contains("--no-human=both"), "{msg}");

    let err = non_tty_interactive_implementer_preflight(
        Some(auto_complete::NoHumanMode::ReviewerOnly),
        true,
        false,
        false,
    )
    .unwrap_err();
    assert!(err.to_string().contains("interactive implementer"), "{err}");
}

// trace:BUG-740 | ai:codex
#[test]
fn non_tty_preflight_allows_headless_implementer_or_headless_env() {
    assert_eq!(
        non_tty_interactive_implementer_preflight(
            Some(auto_complete::NoHumanMode::Both),
            false,
            false,
            false,
        )
        .unwrap(),
        Some(auto_complete::NoHumanMode::Both)
    );
    assert_eq!(
        non_tty_interactive_implementer_preflight(None, false, false, true).unwrap(),
        Some(auto_complete::NoHumanMode::Both)
    );
    assert_eq!(
        non_tty_interactive_implementer_preflight(None, true, true, false).unwrap(),
        None
    );
}

// --- TASK-306: orchestrator-context statusline badge ------------------

/// A corroborated phase-1 session shows the phase index, its name, and
// the always-present pause cue. trace:TASK-306
#[test]
fn orchestrator_badge_shows_phase_and_name() {
    let b = OrchestratorBadge::build(Some(1), None);
    assert_eq!(b.phase, "auto:1/6 implementer");
    assert!(b.no_human.is_none());
    assert_eq!(b.pause, "pause-here");
}

/// The `--no-human` scope is folded in when the env var is present.
// trace:TASK-306
#[test]
fn orchestrator_badge_includes_no_human_scope() {
    let b = OrchestratorBadge::build(Some(3), Some("reviewer-only"));
    assert_eq!(b.phase, "auto:3/6 reviewer");
    assert_eq!(b.no_human.as_deref(), Some("no-human:reviewer-only"));
}

/// Defensive fallback: a missing phase env var renders `?`; an
// out-of-range index keeps the number but drops the name. trace:TASK-306
#[test]
fn orchestrator_badge_falls_back_when_phase_unknown() {
    assert_eq!(OrchestratorBadge::build(None, None).phase, "auto:?/6");
    assert_eq!(OrchestratorBadge::build(Some(9), None).phase, "auto:9/6");
}

/// `read_behavior_permission_mode` parses `[behavior]
/// permission_mode = "..."` and ignores other sections.
// trace:TASK-84 | ai:claude
#[test]
fn read_behavior_permission_mode_parses_value() {
    let tmp = std::env::temp_dir().join(format!(
        "aida-task84-config-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let aida = tmp.join(".aida");
    std::fs::create_dir_all(&aida).unwrap();
    std::fs::write(
        aida.join("config.toml"),
        "[id_format]\npolicy = \"node-aware-only\"\n\n[behavior]\npermission_mode = \"auto\"\n",
    )
    .unwrap();
    let got = read_behavior_permission_mode(&tmp);
    assert_eq!(got.as_deref(), Some("auto"));
    let _ = std::fs::remove_dir_all(&tmp);
}

// Missing config file → None. trace:TASK-84 | ai:claude
#[test]
fn read_behavior_permission_mode_missing_is_none() {
    let tmp = std::env::temp_dir().join(format!(
        "aida-task84-noconf-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    assert!(read_behavior_permission_mode(&tmp).is_none());
    let _ = std::fs::remove_dir_all(&tmp);
}

/// `review_title_matches` accepts the canonical "Review PR-N: ..."
/// shape, leading whitespace, and is case-insensitive on the prefix
// but exact on the number. trace:TASK-85 | ai:claude
#[test]
fn review_title_matches_canonical() {
    assert!(review_title_matches(
        "Review PR-14: shave this yak",
        ReviewForge::GitHub,
        14
    ));
    assert!(review_title_matches(
        "  Review PR-14: leading space",
        ReviewForge::GitHub,
        14
    ));
    assert!(review_title_matches(
        "review pr-14: lowercase prefix",
        ReviewForge::GitHub,
        14
    ));
    assert!(review_title_matches(
        "Review MR-7: gitlab works",
        ReviewForge::GitLab,
        7
    ));
}

/// Reject titles that aren't review stories, that name a different
// number, or that name the wrong forge. trace:TASK-85 | ai:claude
#[test]
fn review_title_matches_rejects_mismatches() {
    // Different PR number.
    assert!(!review_title_matches(
        "Review PR-15: nope",
        ReviewForge::GitHub,
        14
    ));
    // Different forge.
    assert!(!review_title_matches(
        "Review PR-14: github not gitlab",
        ReviewForge::GitLab,
        14
    ));
    // Title doesn't start with Review.
    assert!(!review_title_matches(
        "Fixing PR-14",
        ReviewForge::GitHub,
        14
    ));
    // PR-14 mentioned but not as review.
    assert!(!review_title_matches(
        "Follow-up to PR-14",
        ReviewForge::GitHub,
        14
    ));
    // Substring number — `PR-140` must not match PR-14.
    assert!(!review_title_matches(
        "Review PR-140: longer number",
        ReviewForge::GitHub,
        14
    ));
}

/// `format_review_label` produces the human-facing label that error
// messages use. trace:TASK-85 | ai:claude
#[test]
fn format_review_label_shapes() {
    assert_eq!(format_review_label(ReviewForge::GitHub, 14), "PR-14");
    assert_eq!(format_review_label(ReviewForge::GitLab, 7), "MR-7");
}

/// Quote-aware inline-comment stripping for TOML lines.
// trace:TASK-84 | ai:claude
#[test]
fn strip_toml_inline_comment_basics() {
    // No comment → unchanged.
    assert_eq!(strip_toml_inline_comment(""), "");
    assert_eq!(strip_toml_inline_comment("key = \"v\""), "key = \"v\"");
    // Trailing comment stripped.
    assert_eq!(
        strip_toml_inline_comment("key = \"v\" # trailing"),
        "key = \"v\" "
    );
    // Whole-line comment.
    assert_eq!(strip_toml_inline_comment("# only"), "");
    // `#` inside a double-quoted string preserved.
    assert_eq!(
        strip_toml_inline_comment("key = \"hash #inside\""),
        "key = \"hash #inside\""
    );
    // `#` inside a single-quoted string preserved.
    assert_eq!(
        strip_toml_inline_comment("key = 'hash #inside'"),
        "key = 'hash #inside'"
    );
    // Quote followed by `#` outside any string → stripped.
    assert_eq!(
        strip_toml_inline_comment("key = \"v\"#tight"),
        "key = \"v\""
    );
}

/// `read_behavior_permission_mode` honors inline TOML comments
// (regression for ultrareview bug_002). trace:TASK-84 | ai:claude
#[test]
fn read_behavior_permission_mode_strips_inline_comment() {
    let tmp = std::env::temp_dir().join(format!(
        "aida-task84-inline-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let aida = tmp.join(".aida");
    std::fs::create_dir_all(&aida).unwrap();
    std::fs::write(
        aida.join("config.toml"),
        "[behavior]\npermission_mode = \"auto\"  # default for autonomous runs\n",
    )
    .unwrap();
    let got = read_behavior_permission_mode(&tmp);
    assert_eq!(got.as_deref(), Some("auto"));
    let _ = std::fs::remove_dir_all(&tmp);
}

/// `[behavior]` section absent → None, even when other sections exist.
// trace:TASK-84 | ai:claude
#[test]
fn read_behavior_permission_mode_section_absent() {
    let tmp = std::env::temp_dir().join(format!(
        "aida-task84-noseq-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let aida = tmp.join(".aida");
    std::fs::create_dir_all(&aida).unwrap();
    std::fs::write(aida.join("config.toml"), "[id_format]\npolicy = \"x\"\n").unwrap();
    assert!(read_behavior_permission_mode(&tmp).is_none());
    let _ = std::fs::remove_dir_all(&tmp);
}

// --- TASK-217: status-aware not-queued error message ---

fn make_req_status(spec: &str, t: RequirementType, s: RequirementStatus) -> aida_core::Requirement {
    let mut r = req(spec, Some(spec), t);
    r.status = s;
    r
}

#[test]
fn not_queued_approved_suggests_add_and_work() {
    let r = make_req_status(
        "STORY-86",
        RequirementType::Story,
        RequirementStatus::Approved,
    );
    let msg = format_queue_work_not_queued_error("STORY-86", &r, Some("implementer"));
    assert!(msg.contains("isn't queued"), "msg: {msg}");
    assert!(msg.contains("Approved"), "msg: {msg}");
    assert!(
        msg.contains("aida queue add STORY-86 --for implementer"),
        "msg: {msg}"
    );
    assert!(msg.contains("aida queue work STORY-86"), "msg: {msg}");
}

#[test]
fn not_queued_planned_suggests_promote_to_approved() {
    let r = make_req_status(
        "STORY-86",
        RequirementType::Story,
        RequirementStatus::Planned,
    );
    let msg = format_queue_work_not_queued_error("STORY-86", &r, Some("implementer"));
    assert!(msg.contains("Planned"), "msg: {msg}");
    assert!(
        msg.contains("aida edit STORY-86 --status approved"),
        "msg: {msg}"
    );
    assert!(msg.contains("aida queue add STORY-86"), "msg: {msg}");
}

#[test]
fn not_queued_in_progress_warns_lease_lost() {
    let r = make_req_status(
        "STORY-86",
        RequirementType::Story,
        RequirementStatus::InProgress,
    );
    let msg = format_queue_work_not_queued_error("STORY-86", &r, Some("implementer"));
    assert!(msg.contains("In Progress"), "msg: {msg}");
    assert!(msg.contains("lease may have been lost"), "msg: {msg}");
    assert!(msg.contains("aida queue list --all"), "msg: {msg}");
}

#[test]
fn not_queued_done_suggests_rework_verb() {
    let r = make_req_status("STORY-86", RequirementType::Story, RequirementStatus::Done);
    let msg = format_queue_work_not_queued_error("STORY-86", &r, Some("implementer"));
    assert!(msg.contains("Done"), "msg: {msg}");
    assert!(
        msg.contains("aida queue rework STORY-86 --work"),
        "msg: {msg}"
    );
    // BUG-236: the suggestion must run verbatim for any Done spec, so
    // it stays `--work` (fresh session) — never `--work --resume`,
    // which bounces when the spec has no recorded claude session.
    assert!(!msg.contains("--resume"), "msg: {msg}");
    assert!(msg.contains("auto-bump"), "msg: {msg}");
    // TASK-240: a Done spec's PR author gets a merge-now path, routed via
    // `aida show` (which prints the PR number) so the command is honest.
    assert!(msg.contains("gh pr merge"), "msg: {msg}");
    assert!(msg.contains("aida show STORY-86"), "msg: {msg}");
}

#[test]
fn not_queued_completed_suggests_force_reopen() {
    let r = make_req_status(
        "STORY-86",
        RequirementType::Story,
        RequirementStatus::Completed,
    );
    let msg = format_queue_work_not_queued_error("STORY-86", &r, Some("implementer"));
    assert!(msg.contains("Completed"), "msg: {msg}");
    assert!(msg.contains("already shipped"), "msg: {msg}");
    assert!(
        msg.contains("aida edit STORY-86 --status in-progress --force"),
        "msg: {msg}"
    );
}

#[test]
fn not_queued_rejected_suggests_force_reopen() {
    let r = make_req_status(
        "STORY-86",
        RequirementType::Story,
        RequirementStatus::Rejected,
    );
    let msg = format_queue_work_not_queued_error("STORY-86", &r, Some("implementer"));
    assert!(msg.contains("Rejected"), "msg: {msg}");
    assert!(
        msg.contains("aida edit STORY-86 --status approved --force"),
        "msg: {msg}"
    );
}

#[test]
fn not_queued_container_uses_cluster_message() {
    let r = make_req_status(
        "EPIC-23",
        RequirementType::Epic,
        RequirementStatus::InProgress,
    );
    let msg = format_queue_work_not_queued_error("EPIC-23", &r, Some("implementer"));
    // Containers get a different shape — focus on inspecting + adding
    // children rather than the leaf status-aware recovery.
    assert!(msg.contains("no queued children"), "msg: {msg}");
    assert!(msg.contains("aida queue list --tree"), "msg: {msg}");
    assert!(msg.contains("aida list --parent EPIC-23"), "msg: {msg}");
}

#[test]
fn not_queued_falls_back_when_role_unknown() {
    let r = make_req_status(
        "STORY-86",
        RequirementType::Story,
        RequirementStatus::Approved,
    );
    let msg = format_queue_work_not_queued_error("STORY-86", &r, None);
    assert!(msg.contains("--for <role>"), "msg: {msg}");
}

/// BUG-226: `--quiet` parses on `aida queue work` and defaults off, so
/// a standalone reviewer prints its end-of-command summary unless the
/// caller opts out.
#[test]
fn queue_work_quiet_flag_parses() {
    let on = Cli::try_parse_from([
        "aida", "queue", "work", "PR-65", "--role", "reviewer", "--quiet",
    ])
    .expect("--quiet parses");
    let off = Cli::try_parse_from(["aida", "queue", "work", "PR-65", "--role", "reviewer"])
        .expect("no --quiet parses");
    let quiet_of = |c: &Cli| match &c.command {
        Command::Queue(QueueCommand::Work { quiet, .. }) => *quiet,
        _ => panic!("expected queue work command"),
    };
    assert!(quiet_of(&on), "--quiet should set quiet=true");
    assert!(!quiet_of(&off), "quiet defaults to false");
}

/// TASK-560: --resume + --auto-complete must now PARSE (the clap conflict
/// was lifted) so the handler can reject it with a helpful message instead
// of clap's terse "cannot be used with". trace:TASK-560
#[test]
fn queue_work_resume_plus_auto_complete_parses_for_handler_rejection() {
    let cli = Cli::try_parse_from([
        "aida",
        "queue",
        "work",
        "STORY-465",
        "--resume",
        "--auto-complete",
    ])
    .expect("--resume + --auto-complete should parse (handler rejects, not clap)");
    match cli.command {
        Command::Queue(QueueCommand::Work {
            resume,
            auto_complete,
            ..
        }) => {
            assert!(resume.is_some() && auto_complete.is_some());
        }
        other => panic!("expected queue work command, got {other:?}"),
    }
}

/// TASK-560: the conflict message fires only for the pair, and carries the
// WHY + both recovery paths. trace:TASK-560
#[test]
fn resume_autocomplete_conflict_message_explains_and_recovers() {
    assert!(resume_autocomplete_conflict_message(false, true).is_none());
    assert!(resume_autocomplete_conflict_message(true, false).is_none());
    assert!(resume_autocomplete_conflict_message(false, false).is_none());
    let msg = resume_autocomplete_conflict_message(true, true).expect("pair conflicts");
    assert!(msg.contains("FRESH"), "explains why: {msg}");
    assert!(
        msg.contains("--resume alone"),
        "names the continue path: {msg}"
    );
    assert!(
        msg.contains("aida session end"),
        "names the fresh-drain path: {msg}"
    );
}

#[test]
fn queue_work_force_claim_flag_parses() {
    let cli = Cli::try_parse_from(["aida", "queue", "work", "TASK-559", "--force-claim"])
        .expect("--force-claim parses on queue work");
    match cli.command {
        Command::Queue(QueueCommand::Work { force_claim, .. }) => {
            assert!(force_claim, "--force-claim should set force_claim=true");
        }
        other => panic!("expected queue work command, got {other:?}"),
    }
}

// BUG-311: the orchestrator's phase-1 `aida queue work` subprocess must
// carry `--steal` when the outer drain was invoked with it. Without
// threading, the inner subprocess's dormant-lease guard sees `steal=false`
// and bails with the canned "pass --steal" message — exactly the
// symptom this BUG reported. These tests pin the argv contract so a
// refactor cannot silently drop the flag again. trace:BUG-311 | ai:claude

#[test]
fn implementer_phase_args_threads_steal() {
    let args = build_implementer_phase_args(
        "BUG-311",
        "0192f1c8-aaaa-7000-8000-000000000001",
        true,
        false,
        None,
        None,
        false,
        None,
    );
    assert!(
        args.iter().any(|a| a == "--steal"),
        "--steal must be threaded to phase 1 when the outer drain set it; got {:?}",
        args
    );
    assert_eq!(args[0], "queue");
    assert_eq!(args[1], "work");
    assert_eq!(args[2], "BUG-311");
    assert_eq!(args[3], "--session-id");
    assert_eq!(args[4], "0192f1c8-aaaa-7000-8000-000000000001");
}

#[test]
fn implementer_phase_args_omits_steal_by_default() {
    let args =
        build_implementer_phase_args("BUG-311", "uuid", false, false, None, None, false, None);
    assert!(
        args.iter().all(|a| a != "--steal"),
        "--steal must not appear when the outer drain did not pass it; got {:?}",
        args
    );
}

#[test]
fn implementer_phase_args_threads_no_human_and_permission() {
    let args = build_implementer_phase_args(
        "TASK-1",
        "uuid",
        true,
        false,
        None,
        None,
        true,
        Some("bypassPermissions"),
    );
    assert!(args.iter().any(|a| a == "--steal"));
    assert!(args.iter().any(|a| a == "--no-human"));
    let i = args
        .iter()
        .position(|a| a == "--permission-mode")
        .expect("--permission-mode threaded");
    assert_eq!(
        args.get(i + 1).map(String::as_str),
        Some("bypassPermissions")
    );
}

#[test]
fn implementer_phase_args_threads_force_claim() {
    let args =
        build_implementer_phase_args("TASK-559", "uuid", false, true, None, None, false, None);
    assert!(
        args.iter().any(|a| a == "--force-claim"),
        "--force-claim must be threaded to phase 1 when the outer drain set it; got {:?}",
        args
    );
    assert!(
        args.iter().all(|a| a != "--steal"),
        "--force-claim should not imply --steal; got {:?}",
        args
    );
}

#[test]
fn implementer_retry_args_thread_branch_and_existing_path_without_steal() {
    let path = std::path::Path::new("/tmp/aida-story-993");
    let args = build_implementer_phase_args(
        "STORY-993",
        "uuid",
        false,
        true,
        Some("story-993"),
        Some(path),
        true,
        None,
    );

    assert!(args.iter().any(|a| a == "--force-claim"));
    assert!(args.iter().all(|a| a != "--steal"), "{args:?}");
    assert_eq!(
        args.windows(2)
            .find(|w| w[0] == "--branch")
            .map(|w| w[1].as_str()),
        Some("story-993")
    );
    assert_eq!(
        args.windows(2)
            .find(|w| w[0] == "--path")
            .map(|w| w[1].as_str()),
        Some("/tmp/aida-story-993")
    );
}

#[test]
fn auto_complete_phase1_status_promotes_only_not_started_statuses() {
    assert_eq!(
        auto_complete_phase1_target_status(&RequirementStatus::Draft),
        Some(RequirementStatus::InProgress)
    );
    assert_eq!(
        auto_complete_phase1_target_status(&RequirementStatus::Approved),
        Some(RequirementStatus::InProgress)
    );
    assert_eq!(
        auto_complete_phase1_target_status(&RequirementStatus::Planned),
        Some(RequirementStatus::InProgress)
    );
    assert_eq!(
        auto_complete_phase1_target_status(&RequirementStatus::InProgress),
        None
    );
    assert_eq!(
        auto_complete_phase1_target_status(&RequirementStatus::NeedsAttention),
        None
    );
    assert_eq!(
        auto_complete_phase1_target_status(&RequirementStatus::Completed),
        None
    );
    assert_eq!(
        auto_complete_phase1_target_status(&RequirementStatus::Rejected),
        None
    );
}

#[test]
fn prepare_auto_complete_phase1_status_flips_approved_before_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("requirements.yaml");
    let storage = Storage::new(&path);
    let mut req = aida_core::Requirement::new("early punt".to_string(), String::new());
    req.spec_id = Some("BUG-369".to_string());
    req.status = RequirementStatus::Approved;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    storage.save(&store).unwrap();

    let changed = prepare_auto_complete_phase1_status(&storage, "BUG-369")
        .expect("phase-1 status preparation should succeed");

    assert_eq!(
        changed,
        Some(("BUG-369".to_string(), RequirementStatus::Approved))
    );
    let updated = storage.load().unwrap();
    let req = updated.get_requirement_by_spec_id("BUG-369").unwrap();
    assert_eq!(req.status, RequirementStatus::InProgress);
}

#[test]
fn auto_complete_head_names_sibling_role_queue_and_honors_role_override() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let backend = aida_core::GitBackend::new(&root).unwrap();
    let storage = Storage::new(&root);

    let mut req = aida_core::Requirement::new("route me".to_string(), String::new());
    req.spec_id = Some("BUG-795".to_string());
    req.status = RequirementStatus::Approved;
    let req_id = req.id;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    backend.save(&store).unwrap();
    storage
        .queue_add(aida_core::QueueEntry {
            user_id: "u".into(),
            requirement_id: req_id,
            position: 1000,
            added_by: "u".into(),
            note: None,
            added_at: chrono::Utc::now(),
            for_role: Some("implementer".into()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        })
        .unwrap();

    // trace:BUG-795 | ai:codex
    let err = resolve_auto_complete_head(&storage, "u", Some("advisor"))
        .expect_err("advisor queue should be empty when item is routed to implementer")
        .to_string();
    for needle in [
        "queue is empty for advisor",
        "1 item(s) routed for:implementer",
        "aida queue work --auto-complete --role implementer",
    ] {
        assert!(
            err.contains(needle),
            "empty-role error missing `{needle}`:\n{err}"
        );
    }

    let picked = resolve_auto_complete_head(&storage, "u", Some("implementer"))
        .expect("--role implementer should select the implementer-routed head");
    assert_eq!(picked, "BUG-795");
}

/// TASK-547 acceptance: smart-default auto-queues an Approved-but-not-queued
/// spec for the current role when resolving the queue work plan, unless `--strict` is set.
// trace:TASK-547 | ai:antigravity
#[test]
fn resolve_queue_work_plan_auto_queues_when_not_strict() {
    // BUG-1195: this test mutates AIDA_SESSION_ROLE — serialize with the other
    // env-mutating tests instead of racing them.
    let _guard = crate::test_env::env_lock();
    let prior_role = std::env::var("AIDA_SESSION_ROLE").ok();
    std::env::remove_var("AIDA_SESSION_ROLE");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let backend = aida_core::GitBackend::new(&root).unwrap();
    let storage = Storage::new(&root);
    let mut req = aida_core::Requirement::new("backlog item".to_string(), String::new());
    req.spec_id = Some("BUG-376".to_string());
    req.status = RequirementStatus::Approved;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    backend.save(&store).unwrap();

    // 1. With strict = true, it must refuse and error out with status-aware error message
    let res = resolve_queue_work_plan(
        &storage,
        "test-user",
        Some("BUG-376"),
        None,
        true,
        false,
        false,
        None,
    );
    assert!(res.is_err());
    let err = res.unwrap_err().to_string();
    assert!(
        err.contains("`BUG-376` isn't queued"),
        "expected not queued error, got: {err}"
    );

    // TASK-1053: a DRY RUN on the same Approved-but-unqueued spec must
    // resolve the very same Item plan WITHOUT persisting the auto-queue —
    // the queue stays empty afterwards. trace:TASK-1053 | ai:claude
    let res = resolve_queue_work_plan(
        &storage,
        "test-user",
        Some("BUG-376"),
        None,
        false,
        true,
        false,
        None,
    )
    .expect("dry-run should resolve a plan without persisting");
    assert_eq!(res.mode, QueueWorkMode::Item);
    assert_eq!(res.anchor_display, "BUG-376");
    let entries = storage.queue_list("test-user", false).unwrap();
    assert!(
        entries.is_empty(),
        "dry-run must not persist the auto-queue, found: {entries:?}"
    );

    // 2. With strict = false (real run), it must automatically queue it and return a successful plan
    let res = resolve_queue_work_plan(
        &storage,
        "test-user",
        Some("BUG-376"),
        None,
        false,
        false,
        false,
        None,
    )
    .expect("auto-queue should succeed and return plan");
    assert_eq!(res.mode, QueueWorkMode::Item);
    assert_eq!(res.anchor_display, "BUG-376");

    // Verify it was added to the queue
    let entries = storage.queue_list("test-user", false).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].for_role.as_deref(), Some("implementer"));
    if let Some(role) = prior_role {
        std::env::set_var("AIDA_SESSION_ROLE", role);
    }
}

fn queue_review_story(storage: &Storage, root: &std::path::Path) {
    let backend = aida_core::GitBackend::new(root).unwrap();
    let mut review =
        aida_core::Requirement::new("Review PR-457: throwaway".to_string(), String::new());
    review.spec_id = Some("STORY-901".to_string());
    review.status = RequirementStatus::Approved;
    let review_id = review.id;
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(review);
    backend.save(&store).unwrap();
    storage
        .queue_add(aida_core::QueueEntry {
            user_id: "u".into(),
            requirement_id: review_id,
            position: 0,
            added_by: "u".into(),
            note: None,
            added_at: chrono::Utc::now(),
            for_role: Some("reviewer".into()),
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        })
        .unwrap();
}

/// STORY-501: the dispatch gate's signal — `queued_review_story_for_pr`
/// detects a queued "Review PR-N" story (so the dispatch DEFERS PR→spec
// resolution to the reviewer pickup). trace:STORY-501 | ai:claude
#[test]
fn queued_review_story_for_pr_detects_queued_story() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let storage = Storage::new(&root);
    queue_review_story(&storage, &root);
    assert!(queued_review_story_for_pr(&storage, "u", ReviewForge::GitHub, 457).is_found());
    assert!(!queued_review_story_for_pr(&storage, "u", ReviewForge::GitHub, 999).is_found());
    // BUG-1195 / BUG-1193: the story was queued by user `u` (a sibling
    // worktree session); the drain's reviewer child looks it up as
    // `role:implementer`. The role-routed entry must be found through the
    // same role fallback the plan-builder uses — this was the "no pickable
    // review story" shelve.
    assert_eq!(
        queued_review_story_for_pr(&storage, "role:implementer", ReviewForge::GitHub, 457),
        ReviewStoryLookup::Found("STORY-901".to_string())
    );
    assert!(matches!(
        queued_review_story_for_pr(&storage, "role:implementer", ReviewForge::GitHub, 999),
        ReviewStoryLookup::NoTitleMatch { .. }
    ));
}

/// STORY-501: with a "Review PR-N" story queued, resolve_queue_work_plan
/// routes `PR-N` to the reviewer (review_target set → `/aida-review --pr N`)
/// — the path the dispatch gate now lets run instead of resolving
// PR→backing-spec into an implementer pickup. trace:STORY-501 | ai:claude
#[test]
fn resolve_queue_work_plan_pr_n_with_review_story_routes_to_reviewer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let storage = Storage::new(&root);
    queue_review_story(&storage, &root);
    let plan = resolve_queue_work_plan(
        &storage,
        "u",
        Some("PR-457"),
        None,
        false,
        false,
        false,
        None,
    )
    .expect("PR-N with a queued review story resolves to a plan");
    assert!(
        plan.review_target.is_some(),
        "PR-N pickup must set review_target so it routes to the reviewer"
    );
    assert_eq!(
        derive_queue_work_prompt(&plan, "reviewer", false, false, None),
        reviewer_prompt_for_round(457, 1)
    );
}

/// BUG-1195 (round 2): the advisor's shell repro. The review story was queued
/// by user `u` and routed to `reviewer`; the pickup runs as a DIFFERENT user
/// with the shell's `AIDA_SESSION_ROLE=advisor`. A PR-N scope must read the
/// reviewer route regardless of that env role, so the story is found.
#[test]
fn resolve_queue_work_plan_pr_n_finds_reviewer_routed_story_across_users_and_env_role() {
    let _guard = crate::test_env::env_lock();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let storage = Storage::new(&root);
    queue_review_story(&storage, &root);
    let prev = std::env::var_os("AIDA_SESSION_ROLE");
    std::env::set_var("AIDA_SESSION_ROLE", "advisor");
    let plan = resolve_queue_work_plan(
        &storage,
        "role:implementer",
        Some("PR-457"),
        None,
        false,
        true,
        false,
        Some("reviewer"),
    );
    // Also without any --role: the PR scope alone implies the reviewer route.
    let plan_no_hint = resolve_queue_work_plan(
        &storage,
        "role:implementer",
        Some("PR-457"),
        None,
        false,
        true,
        false,
        None,
    );
    match prev {
        Some(v) => std::env::set_var("AIDA_SESSION_ROLE", v),
        None => std::env::remove_var("AIDA_SESSION_ROLE"),
    }
    let plan = plan.expect("PR-N resolves through the reviewer route for another user");
    assert!(plan.review_target.is_some());
    assert_eq!(plan.anchor_display, "STORY-901");
    assert!(plan_no_hint
        .expect("PR scope implies reviewer")
        .review_target
        .is_some());
    // A missing story still bails, and the bail names the dropped filter.
    let err = resolve_queue_work_plan(
        &storage,
        "u",
        Some("PR-999"),
        None,
        false,
        true,
        false,
        None,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("no queued review story for PR-999"), "{err}");
    assert!(err.contains("none is a review story"), "{err}");
}

/// TASK-630 (BUG-250 criterion 5): the held-state re-entry decision is a pure
/// function, so it can be exercised exhaustively with no Storage, worktree,
/// or launcher. A deliberate PR-hold parks the spec Done + dequeued with a
/// marker; `--resume` against that combination is the ONLY case that may
/// re-enter. Every other state, a missing marker, or a non-resume invocation
// must NOT — those keep the existing recovery hints. trace:TASK-630 | ai:claude
#[test]
fn held_resume_reentry_only_for_resume_done_and_marked() {
    use RequirementStatus::*;

    // The one re-enterable combination: explicit --resume, Done status,
    // hold marker present.
    assert!(
        held_resume_reentry_allowed(true, &Done, true),
        "resume + Done + hold marker is the deliberate-hold re-entry case"
    );

    // No --resume → never re-enter (a plain `queue work <spec>` keeps its
    // status-aware not-queued hint).
    assert!(
        !held_resume_reentry_allowed(false, &Done, true),
        "without --resume a held Done spec is not auto-re-entered"
    );

    // Marker absent → not a deliberate hold; leave it to the Done hint
    // (rework / wait-for-merge).
    assert!(
        !held_resume_reentry_allowed(true, &Done, false),
        "no hold marker → not a deliberate hold, no re-entry"
    );

    // Held re-entry is Done-specific: a hold marker against any other status
    // must not unlock resume (defensive — a held spec is always Done).
    for status in [
        Draft,
        Approved,
        Planned,
        InProgress,
        Completed,
        Rejected,
        NeedsAttention,
    ] {
        assert!(
            !held_resume_reentry_allowed(true, &status, true),
            "held re-entry must be Done-only; {status:?} must not re-enter"
        );
    }
}

/// TASK-630: a held-spec resume plan is an Item-mode pickup anchored on the
/// spec itself (its own id is the lease scope — the implementer worktree the
/// dormant session lives in), with exactly one entry. This is what lets the
/// rest of `handle_queue_work` resume the session unchanged.
// trace:TASK-630 | ai:claude
#[test]
fn held_resume_plan_is_item_scoped_to_the_spec() {
    let mut req = aida_core::Requirement::new("held work".to_string(), String::new());
    req.spec_id = Some("STORY-306".to_string());
    req.status = RequirementStatus::Done;

    let plan = held_resume_plan(&req, "test-user");

    assert_eq!(plan.mode, QueueWorkMode::Item);
    assert_eq!(plan.entries.len(), 1, "exactly the held spec, no cluster");
    assert_eq!(plan.anchor_display, "STORY-306");
    assert_eq!(
        plan.scope, "STORY-306",
        "scope is the spec's own id (its implementer worktree)"
    );
    assert!(
        plan.review_target.is_none(),
        "a held implementer spec is not a PR-review pickup"
    );
    assert_eq!(plan.entries[0].spec_id, "STORY-306");
}

/// BUG-311 acceptance: when `--steal`'s internal `session_end` fails, the
/// inner subprocess's error must name the lease + actual reason — not
/// the canned "pass --steal" message. anyhow's `{:#}` collapses the
/// chain inline; the `--steal`-prefixed map_err in `handle_queue_work`
/// guarantees the lease id + reason are in the primary line.
// trace:BUG-311 | ai:claude
#[test]
fn steal_session_end_failure_surfaces_actual_reason_not_canned_message() {
    let lease_id = "019e4dec-abcd-7000-8000-000000000000";
    let short = &lease_id[..8];
    let underlying = anyhow::anyhow!("worktree has uncommitted changes — pass --force to discard");
    let wrapped: anyhow::Error = anyhow::anyhow!(
        "--steal could not end lease {}: {:#} \
             (resolve manually with `aida session end {} --force` to discard, \
             or commit/stash the worktree's changes first, then re-run)",
        short,
        underlying,
        short,
    );

    let primary = format!("{}", wrapped);
    assert!(
        primary.contains(&format!("--steal could not end lease {}", short)),
        "primary line must name the lease the steal targeted; got: {}",
        primary
    );
    assert!(
        primary.contains("worktree has uncommitted changes"),
        "primary line must include the actual session_end reason inline; got: {}",
        primary
    );
    assert!(
            !primary.contains("pass --steal to end that session first"),
            "BUG-311: must NOT emit the canned `pass --steal` message when --steal IS already in play; got: {}",
            primary
        );
}

/// BUG-1568: the pipelined batch child is itself a `queue work
/// --auto-complete`, so it takes the drain lock at its top while the parent
/// batch drive still holds it. Without `AIDA_DRAIN_BORROW` it is refused with
/// "a drain is already running (pid <parent>)", and the batch driver reports
/// that refusal as a phase failure of the member — so the drain stops, ships
/// nothing, and blames an innocent spec. Measured before the fix: 221
/// consecutive zero-ship drains over 17.5 hours.
///
/// Asserted on the ENV rather than the argv on purpose. The flag is invisible
/// in the arguments, so the argv-shaped routing guardrails could not see it go
/// missing — which is how this shipped.
// trace:BUG-1568 | ai:claude
#[test]
fn pipelined_batch_child_borrows_the_parent_drain_lock() {
    let env = crate::pipelined_child_env(std::path::Path::new("/w/x/result.json"));
    let get = |key: &str| env.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str());

    assert_eq!(
        get("AIDA_DRAIN_BORROW"),
        Some("1"),
        "the pipelined child must borrow the parent's drain lock rather than \
         acquire its own; env was {env:?}"
    );

    // FORCE must NOT be set. A borrowed guard neither rewrites nor releases the
    // parent's lock; FORCE overwrites it, which is the pre-BUG-748 behaviour
    // that left the parent running without a live lock.
    assert!(
        !env.iter().any(|(k, _)| *k == "AIDA_DRAIN_FORCE"),
        "borrow, not force — force overwrites the parent's lock: {env:?}"
    );

    // The pre-existing contract stays intact.
    assert_eq!(get("AIDA_PIPELINED_BATCH_CHILD"), Some("1"));
    assert_eq!(get("AIDA_PIPELINED_RESULT_PATH"), Some("/w/x/result.json"));
}

/// BUG-1570: a pipelined child that ends without reporting a drive outcome may
/// never have started the spec at all. Attributing that to the SPEC's CI phase
/// is what let a drain-level lock refusal spend 17.5 hours being blamed on an
/// innocent spec — the batch summary read "drain stopped at BUG-1462 (phase 2
/// failed)" and every reader went to debug BUG-1462.
// trace:BUG-1570 | ai:claude
#[test]
fn a_child_that_reported_nothing_is_not_blamed_on_the_spec() {
    let result = crate::pipelined_child_reported_nothing("BUG-1462", Some(1));

    let failure = result
        .failure
        .as_ref()
        .expect("an unreported child exit must carry a failure explanation");

    // The load-bearing assertion: this is an orchestrator fault, so it is
    // un-shelvable and the spec is not parked for something it did not do.
    assert_eq!(
        failure.kind,
        crate::auto_complete::FailureKind::Internal,
        "an unreported child is an orchestrator-layer fault, not the spec's CI"
    );

    assert!(
        failure.reason.contains("BUG-1462"),
        "the operator needs to know which child: {}",
        failure.reason
    );
    assert!(
        failure.reason.contains("NOT a phase failure"),
        "the message must actively deny the attribution that misled everyone: {}",
        failure.reason
    );
    assert!(
        failure.reason.contains("drain lock"),
        "a refused drain lock is the case this exists for; name it: {}",
        failure.reason
    );

    // A signal-terminated child has no code and must still explain itself.
    let signalled = crate::pipelined_child_reported_nothing("BUG-1462", None);
    let reason = &signalled.failure.as_ref().unwrap().reason;
    assert!(
        reason.contains("signal"),
        "a child killed by a signal must say so rather than print a bare None: {reason}"
    );
}

/// BUG-1570, the other direction. The test above proves the classifier REJECTS
/// the old behaviour (blaming the spec's CI for a child that said nothing). On
/// its own that is mutation-checked one way only: it stays green if the fix
/// flattened EVERY child outcome into `Internal`, which would destroy the
/// genuine CI-red signal and stop a batch drain dead — `Internal` is
/// un-shelvable, so EPIC-28 could no longer continue past a red member.
///
/// So this pins the complementary case: a child that DID report a drive outcome
/// — it ran the spec, CI came back red, it already shelved the spec into
/// NeedsAttention and exited `DRIVE_EXIT_SHELVED` — still travels the shelvable
/// path, with its shelve attributed to the spec's CI phase exactly as before.
///
/// Note on what crosses the process boundary: the sidecar format written by
/// `write_pipelined_child_result_sidecar` carries `shelved: bool` and
/// `failed_phase`, not the `FailureKind`, so the parent never sees the literal
/// `FailureKind::CiRed` the child computed. At this seam the CI-red signal IS
/// the ci-phase `shelved_reason` — that is the thing that must survive, and the
/// thing an always-`Internal` regression would destroy.
// trace:BUG-1570 | ai:claude
#[test]
fn a_child_that_reported_a_real_ci_failure_still_travels_the_shelvable_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sidecar_path = dir.path().join("BUG-1462-7.json");

    // The bytes a CI-red child leaves behind: shelved at phase 2 (CI), exiting
    // with the phase's 1-based index. Shape mirrors
    // `write_pipelined_child_result_sidecar`.
    std::fs::write(
        &sidecar_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "exit_code": crate::auto_complete::Phase::Ci.index(),
            "failed_phase": crate::auto_complete::Phase::Ci.index(),
            "punt_reason": serde_json::Value::Null,
            "shipped_spec_id": serde_json::Value::Null,
            "escalation": serde_json::Value::Null,
            "inconclusive_reason": serde_json::Value::Null,
            "shelved": true,
            "held_reason": serde_json::Value::Null,
        }))
        .expect("serialize sidecar"),
    )
    .expect("write sidecar");

    let result = crate::classify_pipelined_child_outcome(
        "BUG-1462",
        Some(crate::auto_complete::DRIVE_EXIT_SHELVED),
        false,
        || crate::read_pipelined_child_result_sidecar(&sidecar_path),
    );

    // The load-bearing assertion, and the exact inverse of the test above: this
    // child DID report an outcome, so it must NOT be rewritten into an
    // orchestrator-layer fault.
    assert!(
        !matches!(
            result.failure.as_ref().map(|f| f.kind),
            Some(crate::auto_complete::FailureKind::Internal)
        ),
        "a child that reported a real CI failure must not be relabelled an \
         orchestrator fault: {:?}",
        result.failure
    );

    // The spec stays parked in NeedsAttention, attributed to its CI phase — so
    // EPIC-28 shelves this member and the batch drain carries on.
    let shelved = result
        .shelved_reason
        .as_ref()
        .expect("a real CI failure must still shelve the spec into NeedsAttention");
    assert_eq!(
        shelved.phase,
        crate::auto_complete::Phase::Ci.slug(),
        "the shelve must still be attributed to the spec's CI phase"
    );
    assert_eq!(
        result.failed_phase,
        Some(crate::auto_complete::Phase::Ci),
        "the failed phase must survive the sidecar round trip"
    );

    // And the un-shelvable kind the sibling test asserts for really is
    // un-shelvable, so the two situations cannot both be "keep going".
    assert!(
        !crate::auto_complete::FailureKind::Internal.is_shelvable(),
        "Internal must stay un-shelvable, or the distinction is cosmetic"
    );

    // A shelved child that left NO sidecar falls back to the same shelvable
    // path rather than to the unreported-child fault.
    let no_sidecar = crate::classify_pipelined_child_outcome(
        "BUG-1462",
        Some(crate::auto_complete::DRIVE_EXIT_SHELVED),
        false,
        || None,
    );
    assert!(
        no_sidecar.shelved_reason.is_some(),
        "a DRIVE_EXIT_SHELVED child with no sidecar is still a shelve, not an \
         orchestrator fault"
    );

    // Through the SAME seam, a child that reported nothing DOES get the
    // orchestrator-fault treatment. Asserting both directions here is what
    // makes the pair able to tell the two situations apart, rather than each
    // test separately tolerating a classifier that labels everything alike.
    let unreported = crate::classify_pipelined_child_outcome("BUG-1462", Some(1), false, || None);
    assert_eq!(
        unreported.failure.as_ref().map(|f| f.kind),
        Some(crate::auto_complete::FailureKind::Internal),
        "an unreported child must still be an orchestrator-layer fault: {:?}",
        unreported.failure
    );
    assert!(
        unreported.shelved_reason.is_none(),
        "an orchestrator fault must not park the spec it may never have reached"
    );
}

/// BUG-1515: a `Done` spec whose recorded review verdict is a still-live
/// refusal (RequestChanges/Rejected, never closed by a later merge) must
/// classify as `AwaitingRework`, not `AwaitingMerge` — the drain, `aida
/// queue work`, and every surface that reads `queue_fresh_pickup_policy`
/// must stop reading a rejected round as merge-ready.
// trace:BUG-1515 | ai:claude
#[test]
fn done_spec_with_outstanding_refusal_is_awaiting_rework_not_awaiting_merge() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(
        root,
        &["init", "--initial-branch=claude/bug-1515", "--quiet"],
    );
    git(root, &["commit", "--allow-empty", "-m", "root", "--quiet"]);
    let head = git_head(root);

    let mut store = aida_core::RequirementsStore::default();
    let mut r = Requirement::new("refused round".to_string(), String::new());
    r.spec_id = Some("BUG-1515".to_string());
    r.status = RequirementStatus::Done;
    store.requirements.push(r.clone());

    // No verdict recorded yet: behaves exactly like pre-BUG-1515.
    assert_eq!(
        crate::queue_cmd::queue_fresh_pickup_policy(&r, &store, false, Some(root)),
        crate::queue_cmd::QueueFreshPickup::AwaitingMerge,
        "a Done spec with no recorded verdict is still plain awaiting-merge"
    );

    // A LIVE refusal (never closed by a merge), still pinned to the current
    // branch tip, flips the classification.
    crate::review_verdict::record_verdict(
        root,
        "BUG-1515",
        Some("request-changes"),
        Some(&head),
        Some("claude/bug-1515"),
        Some("needs another round"),
        &["fix the thing".to_string()],
        "reviewer",
    )
    .unwrap();
    let policy = crate::queue_cmd::queue_fresh_pickup_policy(&r, &store, false, Some(root));
    assert_eq!(
        policy,
        crate::queue_cmd::QueueFreshPickup::AwaitingRework,
        "an outstanding refusal still at the tip must read as rework, not merge-ready"
    );
    let reason = crate::queue_cmd::queue_fresh_pickup_reason_label(&policy).unwrap();
    assert!(
        reason.contains("REWORK"),
        "the hint must say REWORK, not awaiting merge: {reason}"
    );
    assert!(
        !reason.contains("--from-pr") && !reason.contains("integrate"),
        "the hint must not route a refused spec through a shipping verb: {reason}"
    );
    assert!(
        reason.contains("aida queue rework"),
        "the hint must name the working two-command recovery route: {reason}"
    );

    // An APPROVED verdict recorded on top (a later round that passed) is not
    // a refusal at all — back to plain awaiting-merge.
    git(
        root,
        &["commit", "--allow-empty", "-m", "approved round", "--quiet"],
    );
    let approved_head = git_head(root);
    crate::review_verdict::record_verdict(
        root,
        "BUG-1515",
        Some("approved"),
        Some(&approved_head),
        Some("claude/bug-1515"),
        Some("looks good"),
        &[],
        "reviewer",
    )
    .unwrap();
    assert_eq!(
        crate::queue_cmd::queue_fresh_pickup_policy(&r, &store, false, Some(root)),
        crate::queue_cmd::QueueFreshPickup::AwaitingMerge,
        "an approval overwriting the refusal must read as plain awaiting-merge again"
    );
}

/// BUG-1515 (blocker 1): a refusal recorded against an OLD head — the normal
/// shape after a rework round (refusal, new commits pushed, `queue done`
/// again) — must NOT keep reading as `AwaitingRework` just because it was
/// never explicitly closed. Its reviewed sha is no longer the branch tip, so
/// `awaiting_you::classify_pr_review` already treats it as re-review-ready
/// (a `Moved` refusal); `queue_fresh_pickup_policy` must agree and fall back
/// to `AwaitingMerge` rather than telling the operator "REWORK NEEDED" for
/// work that was, in fact, already reworked.
// trace:BUG-1515 | ai:claude
#[test]
fn stale_refusal_on_an_old_head_is_not_classified_awaiting_rework() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(
        root,
        &["init", "--initial-branch=claude/bug-1515", "--quiet"],
    );
    git(root, &["commit", "--allow-empty", "-m", "root", "--quiet"]);
    let old_head = git_head(root);

    crate::review_verdict::record_verdict(
        root,
        "BUG-1515",
        Some("request-changes"),
        Some(&old_head),
        Some("claude/bug-1515"),
        Some("needs another round"),
        &["fix the thing".to_string()],
        "reviewer",
    )
    .unwrap();

    // Rework happened: new commits landed past the reviewed sha, but the
    // refusal file was never explicitly closed.
    git(root, &["commit", "--allow-empty", "-m", "fix", "--quiet"]);

    let mut store = aida_core::RequirementsStore::default();
    let mut r = Requirement::new("reworked round".to_string(), String::new());
    r.spec_id = Some("BUG-1515".to_string());
    r.status = RequirementStatus::Done;
    store.requirements.push(r.clone());

    let policy = crate::queue_cmd::queue_fresh_pickup_policy(&r, &store, false, Some(root));
    assert_eq!(
        policy,
        crate::queue_cmd::QueueFreshPickup::AwaitingMerge,
        "a refusal pinned to an OLD head, with new commits since, needs \
         re-review — not another REWORK NEEDED round: {policy:?}"
    );
}

/// BUG-1515 / BUG-1529: a refusal that was explicitly CLOSED by a later merge
/// (`closed_by_merge`) is history, not a live obstruction — it must not
/// resurrect as `AwaitingRework` on some other Done spec that happens to
/// reuse the same verdict file id space.
// trace:BUG-1515 | ai:claude
#[test]
fn closed_refusal_does_not_count_as_outstanding() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    crate::review_verdict::record_verdict(
        root,
        "BUG-1515",
        Some("request-changes"),
        Some("deadbeef"),
        Some("claude/bug-1515"),
        Some("needs another round"),
        &["fix the thing".to_string()],
        "reviewer",
    )
    .unwrap();
    // Close it the way BUG-1529's merge-close path does.
    let path = crate::review_verdict::verdict_path(root, "BUG-1515");
    let body = std::fs::read_to_string(&path).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&body).unwrap();
    value["closed_by_merge"] = serde_json::Value::String("abc1234".to_string());
    crate::review_verdict::write_verdict_atomic(&path, &value.to_string()).unwrap();

    let mut store = aida_core::RequirementsStore::default();
    let mut r = Requirement::new("closed refusal".to_string(), String::new());
    r.spec_id = Some("BUG-1515".to_string());
    r.status = RequirementStatus::Done;
    store.requirements.push(r.clone());

    assert_eq!(
        crate::queue_cmd::queue_fresh_pickup_policy(&r, &store, false, Some(root)),
        crate::queue_cmd::QueueFreshPickup::AwaitingMerge,
        "a CLOSED refusal must not resurrect as rework"
    );
}

// --- BUG-1608: queue-wide drain honours BlockedBy + the failure budget -------

/// BUG-1608 fixture: a real git-backed store with `STORY-52` (prerequisite,
/// status `prereq_status`), `NFR-56` (Approved, `BlockedBy → STORY-52`), and
/// optionally an independent Approved `TASK-60`, all queued for the
/// implementer in that order.
fn bug_1608_fixture(
    prereq_status: RequirementStatus,
    with_independent: bool,
) -> (tempfile::TempDir, Storage) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("aida-store");
    let backend = aida_core::GitBackend::new(&root).unwrap();
    let storage = Storage::new(&root);

    let mut prereq = Requirement::new("prerequisite".to_string(), String::new());
    prereq.spec_id = Some("STORY-52".to_string());
    prereq.status = prereq_status;
    let mut dependent = Requirement::new("dependent".to_string(), String::new());
    dependent.spec_id = Some("NFR-56".to_string());
    dependent.status = RequirementStatus::Approved;
    dependent.relationships.push(Relationship {
        rel_type: aida_core::RelationshipType::BlockedBy,
        target_id: prereq.id,
        created_at: None,
        created_by: None,
    });
    let mut reqs = vec![prereq, dependent];
    if with_independent {
        let mut independent = Requirement::new("independent".to_string(), String::new());
        independent.spec_id = Some("TASK-60".to_string());
        independent.status = RequirementStatus::Approved;
        reqs.push(independent);
    }
    let ids: Vec<Uuid> = reqs.iter().map(|r| r.id).collect();
    let mut store = aida_core::RequirementsStore::default();
    store.requirements = reqs;
    backend.save(&store).unwrap();
    for (i, id) in ids.into_iter().enumerate() {
        storage
            .queue_add(QueueEntry {
                user_id: "u".into(),
                requirement_id: id,
                position: 1000 * (i as i64 + 1),
                added_by: "u".into(),
                note: None,
                added_at: chrono::Utc::now(),
                for_role: Some("implementer".into()),
                for_scope: None,
                for_session: None,
                added_by_machine: None,
            })
            .unwrap();
    }
    (dir, storage)
}

fn bug_1608_set_status(storage: &Storage, spec: &str, status: RequirementStatus) {
    storage
        .update_atomically(|s| {
            if let Some(r) = s
                .requirements
                .iter_mut()
                .find(|r| r.spec_id.as_deref() == Some(spec))
            {
                r.status = status.clone();
            }
        })
        .unwrap();
}

fn bug_1608_real_driver(storage: &Storage) -> RealNextNDriver<'_> {
    RealNextNDriver {
        storage,
        user_id: "u".to_string(),
        role_override: Some("implementer".to_string()),
        variant: auto_complete::AutoCompleteVariant::Full,
        json: true,
        permission_mode: None,
        no_human: None,
        escalate_mode: auto_complete::EscalateMode::Blocks,
        steal: false,
        force_claim: false,
        allow_stale_base: false,
        no_auto_rebase: false,
        token_meter: None,
        role_skipped: Vec::new(),
        seen_role_skips: std::collections::HashSet::new(),
        pipeline_depth: 1,
        pipelined_children: std::collections::HashMap::new(),
        pipelined_result_paths: std::collections::HashMap::new(),
        next_pipelined_handle: 1,
    }
}

/// Drives the REAL queue-wide head resolver ([`RealNextNDriver::next_head`])
/// and simulates each member's lifecycle as real status transitions in the
/// store: `shelve` specs go In Progress → Needs Attention (a phase-2 shelve),
/// everything else In Progress → Completed.
struct Bug1608Driver<'a> {
    real: RealNextNDriver<'a>,
    shelve: Vec<&'static str>,
    runs: Vec<String>,
}

impl auto_complete::BatchDriver for Bug1608Driver<'_> {
    fn next_head(&mut self) -> Option<String> {
        self.real.next_head()
    }

    fn run_spec(&mut self, spec: &str) -> auto_complete::OrchestrationResult {
        self.runs.push(spec.to_string());
        let storage = self.real.storage;
        bug_1608_set_status(storage, spec, RequirementStatus::InProgress);
        if self.shelve.contains(&spec) {
            bug_1608_set_status(storage, spec, RequirementStatus::NeedsAttention);
            let mut result = auto_complete::OrchestrationResult::failed(auto_complete::Phase::Ci);
            result.shelved_reason = Some(aida_core::FailureReason {
                phase: "ci".to_string(),
                phase_index: 2,
                kind: "failed".to_string(),
                detail: "no usable origin".to_string(),
                recovery_hint: None,
                shelved_by: None,
                shelved_at: chrono::Utc::now(),
            });
            return result;
        }
        bug_1608_set_status(storage, spec, RequirementStatus::Completed);
        auto_complete::OrchestrationResult::ok()
    }
}

fn bug_1608_drain(
    storage: &Storage,
    shelve: Vec<&'static str>,
    max_failures: Option<usize>,
) -> (auto_complete::BatchDrainResult, Vec<String>) {
    let mut driver = Bug1608Driver {
        real: bug_1608_real_driver(storage),
        shelve,
        runs: Vec::new(),
    };
    let mut result = auto_complete::drain_batch(&mut driver, Some(99), max_failures);
    // Same composition `handle_auto_complete_next_n` performs.
    result.skipped.extend(driver.real.role_skipped);
    (result, driver.runs)
}

/// BUG-1608 acceptance (the observed incident): queue-wide drain with
/// `--max-failures 1`, STORY-52 shelves in phase 2 — the drain stops before a
/// second launch; NFR-56 (BlockedBy STORY-52) is never started.
// trace:BUG-1608 | ai:claude
#[test]
fn bug_1608_queue_wide_drain_max_failures_one_stops_after_first_shelve() {
    let (_dir, storage) = bug_1608_fixture(RequirementStatus::Approved, true);
    let (result, runs) = bug_1608_drain(&storage, vec!["STORY-52"], Some(1));
    assert_eq!(runs, vec!["STORY-52"], "no second spec may launch");
    assert_eq!(result.shelved, vec!["STORY-52"]);
    assert_eq!(result.exit_code, auto_complete::DRIVE_EXIT_HARD_FAIL);
    assert!(matches!(
        result.outcome,
        auto_complete::BatchDrainOutcome::Failed(auto_complete::Phase::Ci)
    ));
}

/// BUG-1608: with budget to spare, the dependent of a shelved spec is skipped
/// (and reported with its blocker) in the same drain, while independent work
/// continues.
// trace:BUG-1608 | ai:claude
#[test]
fn bug_1608_dependent_of_shelved_spec_is_skipped_in_same_drain() {
    let (_dir, storage) = bug_1608_fixture(RequirementStatus::Approved, true);
    let (result, runs) = bug_1608_drain(&storage, vec!["STORY-52"], Some(5));
    assert_eq!(runs, vec!["STORY-52", "TASK-60"]);
    assert!(!runs.iter().any(|s| s == "NFR-56"));
    assert_eq!(result.shelved, vec!["STORY-52"]);
    assert_eq!(result.shipped, vec!["TASK-60"]);
    assert_eq!(
        result.skipped,
        vec![(
            "NFR-56".to_string(),
            "blocked-by STORY-52 (Needs Attention)".to_string()
        )]
    );
    assert_eq!(result.exit_code, auto_complete::DRIVE_EXIT_SHELVED);
}

/// BUG-1608: Done is not Completed — a dependent of a Done prerequisite (e.g.
/// merged-pending-verification, or a pushed branch) is not pickable, through
/// either the queue-wide resolver or the single-head pickup.
// trace:BUG-1608 | ai:claude
#[test]
fn bug_1608_dependent_of_done_but_not_completed_prereq_is_not_picked() {
    let (_dir, storage) = bug_1608_fixture(RequirementStatus::Done, false);
    let (pick, _role, blocked) = resolve_next_n_head(&storage, "u", Some("implementer"));
    assert!(pick.is_none(), "NFR-56 must not be picked: {pick:?}");
    assert_eq!(
        blocked,
        vec![(
            "NFR-56".to_string(),
            "blocked-by STORY-52 (Done)".to_string()
        )]
    );
    let err = resolve_auto_complete_head(&storage, "u", Some("implementer"))
        .expect_err("single-head pickup must refuse the blocked dependent");
    assert!(err.to_string().contains("nothing to drive"), "{err}");

    // Positive control: once the prerequisite is Completed, the edge is met.
    bug_1608_set_status(&storage, "STORY-52", RequirementStatus::Completed);
    let (pick, _role, blocked) = resolve_next_n_head(&storage, "u", Some("implementer"));
    assert_eq!(pick.map(|p| p.spec), Some("NFR-56".to_string()));
    assert!(blocked.is_empty());
}

/// BUG-1608 / PRIN-5: a `BlockedBy` edge whose target cannot be resolved
/// (dangling — dependency state unknown) fails closed: not picked.
// trace:BUG-1608 | ai:claude
#[test]
fn bug_1608_unknown_dependency_state_fails_closed() {
    let (_dir, storage) = bug_1608_fixture(RequirementStatus::Approved, false);
    storage
        .update_atomically(|s| {
            if let Some(r) = s
                .requirements
                .iter_mut()
                .find(|r| r.spec_id.as_deref() == Some("NFR-56"))
            {
                r.relationships[0].target_id = Uuid::now_v7();
            }
        })
        .unwrap();
    bug_1608_set_status(&storage, "STORY-52", RequirementStatus::Completed);
    let (pick, _role, blocked) = resolve_next_n_head(&storage, "u", Some("implementer"));
    assert!(pick.is_none(), "unknown dependency must not be picked");
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0].0, "NFR-56");
}

/// TASK-1490: the drain preview's member list must apply the same
/// `aida_core::pickability::pickability()` verdict dispatch uses (BUG-1608),
/// not a status-only view. NFR-56 (Approved, `BlockedBy → STORY-52` while
/// STORY-52 is only Approved, not Completed) must not appear as a preview
/// member — it must show up as skipped, with its blocker — even though
/// `next 99` has ample room and NFR-56's status alone is drivable.
// trace:TASK-1490 | ai:claude
#[test]
fn drain_preview_reports_blocked_dependent_as_skipped_not_a_member() {
    let (_dir, storage) = bug_1608_fixture(RequirementStatus::Approved, true);
    let (members, skipped) =
        crate::queue_cmd::drain_preview_head_members(&storage, "u", Some("implementer"), 99)
            .expect("preview resolves");

    let member_ids: Vec<&str> = members.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(
        member_ids,
        vec!["STORY-52", "TASK-60"],
        "the blocked dependent NFR-56 must not be listed as a member: {member_ids:?}"
    );
    assert_eq!(
        skipped,
        vec![(
            "NFR-56".to_string(),
            "blocked-by STORY-52 (Approved)".to_string()
        )],
        "NFR-56 must be reported skipped, with its blocker"
    );

    // Positive control: once the prerequisite is Completed, the dependent
    // becomes a member and drops out of skipped. STORY-52 itself is no
    // longer status-drivable once Completed, so it drops off the preview too
    // — the same status filter `auto_complete_head_drivable` always applied.
    bug_1608_set_status(&storage, "STORY-52", RequirementStatus::Completed);
    let (members, skipped) =
        crate::queue_cmd::drain_preview_head_members(&storage, "u", Some("implementer"), 99)
            .expect("preview resolves");
    let member_ids: Vec<&str> = members.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(member_ids, vec!["NFR-56", "TASK-60"]);
    assert!(skipped.is_empty());
}
