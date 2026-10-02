//! Tests for BUG-1681: the three `aida ps` reporting defects.
//!
//! (a) A spec that is DONE with an approving review (or whose branch rides an
//!     open integration PR) is waiting on the integrator, not stalled — and
//!     must never carry the resume/rebrief hint, because an agent that follows
//!     it restarts work that already landed.
//! (b) Fan-out attribution must name a spec only when a LIVE subagent's
//!     branch/worktree actually matches it — not every flag-only spec in the
//!     repo the moment any fan-out is alive.
//! (c) A fan-out row shows its OWN role (the generic subagent label when the
//!     harness recorded only its placeholder agent type), never the parent
//!     session's role scraped out of the host transcript.
//
// trace:BUG-1681 | ai:claude

use super::*;
use crate::dispatch_health_ps::{DispatchState, WorktreeGitProbe};

fn lease(id: &str, scope: &str, worktree: std::path::PathBuf) -> SessionLease {
    SessionLease {
        id: id.to_string(),
        scope: scope.to_string(),
        slug: scope.to_ascii_lowercase(),
        owner: "tester".into(),
        worktree_path: worktree,
        branch: format!("claude/{}", scope.to_ascii_lowercase()),
        started_at: chrono::Utc::now() - chrono::Duration::hours(6),
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
        interrupted_at: None,
        interrupted_reason: None,
    }
}

fn spec_entry(disp: &str, in_progress: bool) -> RunningWorkSpec {
    RunningWorkSpec {
        disp: disp.into(),
        agreed_id: Some(disp.into()),
        spec_id: Some(disp.into()),
        title: format!("{disp} work"),
        in_progress,
        orphan_excluded_type: false,
    }
}

fn running_work(
    specs: &[RunningWorkSpec],
    leases: &[SessionLease],
    live: &[process_probe::LiveSession],
    probe: WorktreeGitProbe,
) -> (Vec<PsRow>, Vec<PsOrphan>) {
    build_running_work(
        specs,
        leases,
        live,
        chrono::Utc::now(),
        move |_| probe.clone(),
        |_| None,
        |_| None,
        |_| None,
        |_| None,
        |_| MailIdentityStatus::Unknown,
        |_, _| SeatActivity::Unknown,
    )
}

// ── (a) done-and-approved work is awaiting integration, never stalled ──────

/// The live shape (2026-09-27): an implementer session whose spec was
/// completed, approved and swept into an integration batch hours ago. Its
/// process is gone and its worktree is clean, so the dispatch matrix reads
/// STALLED — and the row advertises `aida queue work <spec>`, which restarts
/// work that already shipped. With the integration standing known the row must
/// read `awaiting-integration` and offer nothing to run.
#[test]
fn bug_1681_done_and_approved_row_reads_awaiting_integration_not_stalled() {
    let specs = vec![spec_entry("TASK-1544", false)];
    let leases = vec![lease(
        "sess-landed",
        "TASK-1544",
        std::path::PathBuf::from("/nonexistent/wt-task-1544"),
    )];
    let (mut rows, _orphans) = running_work(&specs, &leases, &[], WorktreeGitProbe::default());
    assert_eq!(rows.len(), 1);

    let standing = dispatch_health_ps::integration_standing(
        /* spec_finished */ true,
        /* verdict_approves */ true,
        Some("codex/integrate-batch105"),
        "claude/task-1544",
        Some("https://github.com/o/r/pull/105#issuecomment-4484756008"),
    )
    .expect("a done spec with an approving review is awaiting integration");
    assert_eq!(standing.pr, Some(105));

    assert!(ps_apply_integration_standing(&mut rows[0], Some(&standing)));
    let d = rows[0].dispatch.as_ref().unwrap();
    assert_eq!(
        d.state,
        DispatchState::AwaitingIntegration,
        "done + approved is the integrator's turn, not a stalled session"
    );
    assert_eq!(d.state.label(), "awaiting-integration");
    let hint = d.hint.as_deref().expect("the row still explains itself");
    assert!(
        hint.contains("awaiting integration") && hint.contains("PR #105"),
        "the hint names the standing and the PR: {hint}"
    );
    for harmful in ["resume", "rebrief", "aida queue work", "salvage"] {
        assert!(
            !hint.contains(harmful),
            "a landed spec must not advertise {harmful:?}: {hint}"
        );
    }
}

/// The standing is evidence-gated: no approval means no claim, and an approval
/// alone (spec still open, reviewed on the session's own branch) is an ordinary
/// in-flight review, not integration.
#[test]
fn bug_1681_integration_standing_requires_approved_plus_integration_evidence() {
    // Not approved → no standing, whatever else is true.
    assert!(dispatch_health_ps::integration_standing(
        true,
        false,
        Some("codex/integrate-batch105"),
        "claude/task-1",
        None
    )
    .is_none());
    // Approved but the spec is still open and the review was recorded on this
    // session's own branch → nothing says the work is in an integration batch.
    assert!(dispatch_health_ps::integration_standing(
        false,
        true,
        Some("claude/task-1"),
        "claude/task-1",
        None
    )
    .is_none());
    // Approved + spec done → awaiting integration even with no PR known.
    let done = dispatch_health_ps::integration_standing(true, true, None, "claude/task-1", None)
        .expect("done + approved");
    assert_eq!(done.pr, None);
    assert_eq!(done.batch_branch, None);
    // Approved on a DIFFERENT (integration) branch → the branch rides a batch.
    let batched = dispatch_health_ps::integration_standing(
        false,
        true,
        Some("codex/integrate-batch105"),
        "claude/task-1",
        None,
    )
    .expect("reviewed on an integration branch");
    assert_eq!(
        batched.batch_branch.as_deref(),
        Some("codex/integrate-batch105")
    );
}

/// The override is scoped to the two "nothing is moving" readings. A dirty
/// worktree still outranks it: that diff is at risk however finished the spec
/// is, so the salvage hint must survive.
#[test]
fn bug_1681_integration_standing_never_masks_salvageable_or_moving() {
    let standing =
        dispatch_health_ps::integration_standing(true, true, None, "claude/task-1", None)
            .expect("done + approved");
    assert_eq!(
        dispatch_health_ps::apply_integration(DispatchState::Salvageable, Some(&standing)),
        DispatchState::Salvageable
    );
    assert_eq!(
        dispatch_health_ps::apply_integration(DispatchState::Moving, Some(&standing)),
        DispatchState::Moving
    );
    assert_eq!(
        dispatch_health_ps::apply_integration(DispatchState::Stalled, Some(&standing)),
        DispatchState::AwaitingIntegration
    );
    assert_eq!(
        dispatch_health_ps::apply_integration(DispatchState::Stopped, Some(&standing)),
        DispatchState::AwaitingIntegration
    );
    // No standing → every state is left exactly as classified.
    assert_eq!(
        dispatch_health_ps::apply_integration(DispatchState::Stalled, None),
        DispatchState::Stalled
    );
}

// ── (b) fan-out attribution must match a live subagent ────────────────────

/// A live fan-out working something else must not be credited with every
/// flag-only In-Progress spec in the repo. The observed false positive named
/// a spec no subagent had ever touched.
#[test]
fn bug_1681_fanout_attribution_needs_a_matching_live_subagent() {
    let harness_dir = tempfile::tempdir().unwrap();
    let specs = vec![spec_entry("STORY-1425", true)];
    let mut harness = lease(
        "sess-fanout",
        worktree_lease::HARNESS_WORKTREE_SCOPE,
        harness_dir.path().to_path_buf(),
    );
    // A fan-out building something entirely different.
    harness.branch = "claude/bug-9999".into();
    let live = vec![process_probe::LiveSession {
        pid: std::process::id(),
        cwd: harness_dir.path().to_path_buf(),
        jsonl: None,
        stale_cwd: false,
    }];
    let (_rows, orphans) = running_work(&specs, &[harness], &live, WorktreeGitProbe::default());
    let orphan = orphans
        .iter()
        .find(|o| o.spec == "STORY-1425")
        .expect("a flag-only In-Progress spec is still listed");
    assert!(
        !orphan.likely_fanout,
        "no live subagent's branch or worktree names this spec — do not guess"
    );
}

/// The true positive still holds: when a live fan-out's branch names the spec,
/// the informational framing stays.
#[test]
fn bug_1681_fanout_attribution_holds_when_the_subagent_names_the_spec() {
    let harness_dir = tempfile::tempdir().unwrap();
    let specs = vec![spec_entry("STORY-1425", true)];
    let mut harness = lease(
        "sess-fanout-match",
        worktree_lease::HARNESS_WORKTREE_SCOPE,
        harness_dir.path().to_path_buf(),
    );
    harness.branch = "claude/story-1425".into();
    let live = vec![process_probe::LiveSession {
        pid: std::process::id(),
        cwd: harness_dir.path().to_path_buf(),
        jsonl: None,
        stale_cwd: false,
    }];
    let (_rows, orphans) = running_work(&specs, &[harness], &live, WorktreeGitProbe::default());
    let orphan = orphans
        .iter()
        .find(|o| o.spec == "STORY-1425")
        .expect("a flag-only In-Progress spec is still listed");
    assert!(
        orphan.likely_fanout,
        "a live subagent on this spec's branch is the fan-out framing"
    );
}

/// The same rule for the stale-lease side (BUG-1656's "possibly worked by a
/// subagent"): a live fan-out elsewhere in the repo says nothing about THIS
/// spec's crashed lease.
#[test]
fn bug_1681_possibly_subagent_needs_the_subagent_in_this_worktree() {
    let harness_dir = tempfile::tempdir().unwrap();
    let spec_worktree = std::path::PathBuf::from("/nonexistent/wt-bug-1641");
    let specs = vec![spec_entry("BUG-1641", true)];
    let mut harness = lease(
        "sess-fanout-elsewhere",
        worktree_lease::HARNESS_WORKTREE_SCOPE,
        harness_dir.path().to_path_buf(),
    );
    harness.branch = "claude/bug-9999".into();
    let leases = vec![
        lease("sess-dead", "BUG-1641", spec_worktree.clone()),
        harness,
    ];
    let live = vec![process_probe::LiveSession {
        pid: std::process::id(),
        cwd: harness_dir.path().to_path_buf(),
        jsonl: None,
        stale_cwd: false,
    }];
    let (_rows, orphans) = running_work(&specs, &leases, &live, WorktreeGitProbe::default());
    let orphan = orphans.iter().find(|o| o.spec == "BUG-1641").unwrap();
    assert!(orphan.stale_lease);
    assert!(
        !orphan.possibly_subagent,
        "a fan-out in an unrelated worktree does not explain this dead lease"
    );
}

// ── (c) a fan-out row carries its own role ────────────────────────────────

/// The harness records only its generic Agent-tool placeholder as the lease
/// role, and the lease's pid is the PARENT claude process (a subagent executes
/// inside it), so the transcript scan resolves the PARENT's role — `advisor`
/// for an advisor-led fan-out. The row must say what it is (a subagent)
/// instead of borrowing its host's identity.
#[test]
fn bug_1681_fanout_row_shows_subagent_role_not_the_parents() {
    let tmp = tempfile::tempdir().unwrap();
    let parent_root = tmp.path().join("repo");
    let wt = tmp.path().join(".claude/worktrees/agent-abc123");
    std::fs::create_dir_all(&parent_root).unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    let jsonl = tmp.path().join("parent-session.jsonl");

    let mut l = lease("sess-sub", worktree_lease::HARNESS_WORKTREE_SCOPE, wt);
    l.role = Some(tail_cmd::HARNESS_AGENT_TYPE.into());
    l.active_pid = Some(std::process::id());
    let live = vec![process_probe::LiveSession {
        pid: std::process::id(),
        cwd: parent_root,
        jsonl: Some(jsonl.clone()),
        stale_cwd: false,
    }];

    let (rows, _) = build_running_work(
        &[],
        &[l],
        &live,
        chrono::Utc::now(),
        |_| WorktreeGitProbe::default(),
        |_| None,
        |_| None,
        |path| (path == jsonl).then(|| "advisor".to_string()),
        |_| None,
        |_| MailIdentityStatus::Unknown,
        |_, _| SeatActivity::Unknown,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].role.as_deref(),
        Some("subagent"),
        "a fan-out row must not inherit the parent session's role"
    );
    assert_eq!(
        rows[0].lease_role.as_deref(),
        Some(tail_cmd::HARNESS_AGENT_TYPE),
        "the recorded provenance is preserved"
    );
}

/// A real recorded role (the agent type the fan-out was dispatched AS) still
/// wins — the placeholder is the only case that falls back to the generic
/// subagent label.
#[test]
fn bug_1681_dispatch_recorded_role_wins_over_the_subagent_label() {
    assert_eq!(
        ps_display_role(Some("implementer"), None, Some("advisor")).as_deref(),
        Some("implementer")
    );
    assert_eq!(
        ps_display_role(Some(tail_cmd::HARNESS_AGENT_TYPE), None, Some("advisor")).as_deref(),
        Some("subagent")
    );
    // TASK-153's stable manifest join is keyed by THIS lease, so it still
    // outranks the generic label.
    assert_eq!(
        ps_display_role(
            Some(tail_cmd::HARNESS_AGENT_TYPE),
            Some("advisor"),
            Some("reviewer")
        )
        .as_deref(),
        Some("advisor")
    );
    // No recorded role at all: the derived signals are all there is.
    assert_eq!(
        ps_display_role(None, None, Some("reviewer")).as_deref(),
        Some("reviewer")
    );
}
