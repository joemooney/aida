use super::*;

/// BUG-1523 (AC3): drive the three substrate-distinguishable cases through the
/// PRODUCTION path — [`build_running_work`] (the same orphan detection
/// `aida ps` uses) feeding [`orphaned_in_progress_items`] (the same mapping
/// `collect_awaiting_report_inner` uses for `aida awaiting`) — rather than
/// asserting only that a hand-built [`awaiting_you::OrphanedInProgressItem`]
/// renders. The pre-existing `awaiting_you.rs` test
/// (`orphaned_in_progress_renders_and_distinguishes_abandoned_from_not_yet_started`)
/// covers rendering only; this covers detection → emission.

fn lease(id: &str, scope: &str, worktree: std::path::PathBuf) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".into(),
        worktree_path: worktree,
        branch: scope.to_ascii_lowercase(),
        started_at: chrono::Utc::now(),
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

fn in_progress_spec(disp: &str, title: &str) -> RunningWorkSpec {
    RunningWorkSpec {
        disp: disp.into(),
        agreed_id: Some(disp.into()),
        spec_id: Some(format!("{disp}-001")),
        title: title.into(),
        in_progress: true,
        orphan_excluded_type: false,
    }
}

fn no_op_orphans(specs: &[RunningWorkSpec], leases: &[SessionLease]) -> Vec<PsOrphan> {
    let now = chrono::Utc::now();
    let (_rows, orphans) = build_running_work(
        specs,
        leases,
        &[],
        now,
        |_| dispatch_health_ps::WorktreeGitProbe::default(),
        |_| None,
        |_| None,
        |_| None,
        |_| None,
        |_| MailIdentityStatus::Unknown,
    );
    orphans
}

/// NO-LEASE case: an In-Progress spec with no spec-scoped lease at all. The
/// production detector ([`build_running_work`], via [`ps_orphan_verdict`])
/// classifies it flag-only (`stale_lease == false` — absent evidence, not
/// "stale", per PRIN-5), and the mapping must still emit it on the
/// `aida awaiting` surface with `abandoned == false`.
#[test]
fn no_lease_case_is_emitted_with_abandoned_false() {
    let specs = vec![in_progress_spec("BUG-9101", "no lease at all")];
    let orphans = no_op_orphans(&specs, &[]);

    let items = orphaned_in_progress_items(orphans, |_| "last touched 3h ago".to_string());

    assert_eq!(items.len(), 1, "the no-lease case must reach the report");
    assert_eq!(items[0].spec_id, "BUG-9101");
    assert_eq!(items[0].title, "no lease at all");
    assert!(
        !items[0].abandoned,
        "no lease at all is flag-only, not a crashed session"
    );
    assert_eq!(items[0].since_label, "last touched 3h ago");
}

/// EXITED-PROCESS case: an In-Progress spec whose spec-scoped lease exists but
/// whose worktree/pid no longer resolve to a live process — the crashed-session
/// signature `lease_state_for` classifies Stale. The production detector marks
/// it `stale_lease == true`, and the mapping must emit it with
/// `abandoned == true`.
#[test]
fn exited_process_case_is_emitted_with_abandoned_true() {
    let specs = vec![in_progress_spec("BUG-9102", "lease outlived its process")];
    let leases = vec![lease(
        "sess-dead",
        "BUG-9102",
        std::path::PathBuf::from("/nonexistent/bug-1523-dead-worktree"),
    )];
    let orphans = no_op_orphans(&specs, &leases);

    let items = orphaned_in_progress_items(orphans, |_| "last touched 2d ago".to_string());

    assert_eq!(
        items.len(),
        1,
        "the exited-process case must reach the report"
    );
    assert_eq!(items[0].spec_id, "BUG-9102");
    assert!(
        items[0].abandoned,
        "a lease whose process exited is a crashed/abandoned session"
    );
}

/// AC2's third distinguishable case — a lease held by a genuinely LIVE
/// process — must not be reported as orphaned at all (distinct from both
/// no-lease and exited-process).
#[test]
fn live_process_case_is_not_reported_as_orphaned() {
    let live_dir = tempfile::tempdir().unwrap();
    let specs = vec![in_progress_spec("BUG-9103", "genuinely being worked")];
    let leases = vec![lease(
        "sess-live",
        "BUG-9103",
        live_dir.path().to_path_buf(),
    )];
    let now = chrono::Utc::now();
    let (_rows, orphans) = build_running_work(
        &specs,
        &leases,
        &[process_probe::LiveSession {
            pid: std::process::id(),
            cwd: live_dir.path().to_path_buf(),
            jsonl: None,
            stale_cwd: false,
        }],
        now,
        |_| dispatch_health_ps::WorktreeGitProbe::default(),
        |_| None,
        |_| None,
        |_| None,
        |_| None,
        |_| MailIdentityStatus::Unknown,
    );

    let items = orphaned_in_progress_items(orphans, |_| "unused".to_string());

    assert!(
        items.is_empty(),
        "a spec with a live-process-backed lease is not an anomaly"
    );
}
