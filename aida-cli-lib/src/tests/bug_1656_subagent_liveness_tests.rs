//! Tests for BUG-1656: an Agent-tool subagent editing inside a leased
//! worktree must not be read as a dead session. Fresh dirty-file movement is
//! liveness (no salvage advice, no "abandoned" awaiting row), a stale spec
//! lease next to a live harness lease reads "possibly worked by a subagent",
//! and a subagent registering from a spec lease's worktree adopts that lease.
//
// trace:BUG-1656 | ai:claude

use super::*;
use crate::dispatch_health_ps::{
    dirty_movement_is_fresh, dispatch_state_with_movement, newest_mtime_age_secs, DispatchState,
    WorktreeGitProbe, DEFAULT_AWAITING_AGENT_GRACE_SECS, DEFAULT_DIRTY_MOVEMENT_FRESH_SECS,
    DEFAULT_STALLED_THRESHOLD_SECS,
};

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
        // trace:TASK-1518 | ai:codex — preserve this fixture's uninterrupted lease.
        interrupted_at: None,
        interrupted_reason: None,
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
        |_| None,
    )
}

// --- (1) dirty movement is liveness -------------------------------------------

#[test]
fn bug_1656_dead_pid_with_fresh_dirty_movement_is_moving_not_salvageable() {
    let worktree = tempfile::tempdir().unwrap();
    let state = dispatch_state_with_movement(
        Some(false),
        true,
        0,
        999,
        DEFAULT_STALLED_THRESHOLD_SECS,
        None,
        DEFAULT_AWAITING_AGENT_GRACE_SECS,
        /* dirty_movement_fresh */ true,
    );
    assert_eq!(state, DispatchState::Moving);
    let hint = crate::dispatch_health_ps::next_command_hint(
        state,
        worktree.path(),
        "claude/task-1",
        None,
        Some("TASK-1"),
        false,
    );
    assert!(
        hint.is_none(),
        "no salvage advice while the tree is changing: {hint:?}"
    );

    // Unknown liveness with fresh movement is Moving as well.
    let unknown = dispatch_state_with_movement(
        None,
        true,
        0,
        10,
        DEFAULT_STALLED_THRESHOLD_SECS,
        None,
        DEFAULT_AWAITING_AGENT_GRACE_SECS,
        true,
    );
    assert_eq!(unknown, DispatchState::Moving);
}

#[test]
fn bug_1656_dead_pid_with_stale_dirty_movement_stays_salvageable() {
    let state = dispatch_state_with_movement(
        Some(false),
        true,
        0,
        999,
        DEFAULT_STALLED_THRESHOLD_SECS,
        None,
        DEFAULT_AWAITING_AGENT_GRACE_SECS,
        /* dirty_movement_fresh */ false,
    );
    assert_eq!(state, DispatchState::Salvageable);
    // A clean tree never counts as moving on this axis.
    assert_eq!(
        dispatch_state_with_movement(
            Some(false),
            false,
            0,
            999,
            DEFAULT_STALLED_THRESHOLD_SECS,
            None,
            DEFAULT_AWAITING_AGENT_GRACE_SECS,
            true,
        ),
        DispatchState::Stalled
    );
}

#[test]
fn bug_1656_freshness_threshold() {
    assert!(dirty_movement_is_fresh(
        Some(0),
        DEFAULT_DIRTY_MOVEMENT_FRESH_SECS
    ));
    assert!(dirty_movement_is_fresh(
        Some(DEFAULT_DIRTY_MOVEMENT_FRESH_SECS - 1),
        DEFAULT_DIRTY_MOVEMENT_FRESH_SECS
    ));
    assert!(!dirty_movement_is_fresh(
        Some(DEFAULT_DIRTY_MOVEMENT_FRESH_SECS),
        DEFAULT_DIRTY_MOVEMENT_FRESH_SECS
    ));
    assert!(!dirty_movement_is_fresh(
        None,
        DEFAULT_DIRTY_MOVEMENT_FRESH_SECS
    ));
}

#[test]
fn bug_1656_newest_mtime_age_skips_missing_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let fresh = tmp.path().join("fresh.rs");
    std::fs::write(&fresh, "x").unwrap();
    let missing = tmp.path().join("deleted.rs");
    let age = newest_mtime_age_secs(&[missing.clone(), fresh], std::time::SystemTime::now());
    assert!(age.is_some_and(|a| a < 5), "{age:?}");
    assert_eq!(
        newest_mtime_age_secs(&[missing], std::time::SystemTime::now()),
        None
    );
}

/// The real probe: a dirty git worktree reports the age of its newest
/// dirty file; a clean one reports `None`.
#[test]
fn bug_1656_probe_reports_newest_dirty_mtime_age() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.invalid"]);
    git(&["config", "user.name", "t"]);
    git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "-qm", "init"]);

    let clean = crate::dispatch_health_ps::probe_worktree(root);
    assert!(!clean.dirty);
    assert_eq!(clean.dirty_newest_mtime_age_secs, None);

    std::fs::write(root.join("a.txt"), "two\n").unwrap();
    std::fs::write(root.join("new.txt"), "untracked\n").unwrap();
    let dirty = crate::dispatch_health_ps::probe_worktree(root);
    assert!(dirty.dirty);
    assert!(
        dirty.dirty_newest_mtime_age_secs.is_some_and(|a| a < 60),
        "{:?}",
        dirty.dirty_newest_mtime_age_secs
    );
}

/// The acceptance test: a dead spec-lease pid plus fresh dirty-file mtimes
/// gives no salvage advice and no "abandoned" awaiting row.
#[test]
fn bug_1656_dead_lease_with_fresh_dirty_movement_gives_no_salvage_and_no_abandoned_row() {
    let specs = vec![in_progress_spec("TASK-1515", "being edited by a subagent")];
    let leases = vec![lease(
        "sess-dead",
        "TASK-1515",
        std::path::PathBuf::from("/nonexistent/bug-1656-wt-task-1515"),
    )];
    let probe = WorktreeGitProbe {
        dirty: true,
        ahead_of_main: 0,
        last_commit_subject: Some("wip".into()),
        dirty_newest_mtime_age_secs: Some(30),
        untracked_only: false,
    };
    let (rows, orphans) = running_work(&specs, &leases, &[], probe);

    let row = rows
        .iter()
        .find(|r| r.spec.as_deref() == Some("TASK-1515"))
        .unwrap();
    let dispatch = row
        .dispatch
        .as_ref()
        .expect("a worktree lease has a dispatch verdict");
    assert_eq!(dispatch.state, DispatchState::Moving);
    assert!(
        dispatch.hint.is_none(),
        "no salvage advice: {:?}",
        dispatch.hint
    );

    assert!(
        orphans.is_empty(),
        "a changing tree is not orphaned: {orphans:?}"
    );
    let items = orphaned_in_progress_items(orphans, |_| "last touched 1m ago".to_string());
    assert!(items.iter().all(|i| !i.abandoned));
    assert!(items.is_empty());
}

/// Without fresh movement the same fixture is the classic crashed session.
#[test]
fn bug_1656_dead_lease_with_old_dirty_diff_is_still_salvageable_and_abandoned() {
    let specs = vec![in_progress_spec("TASK-1516", "crashed for real")];
    let leases = vec![lease(
        "sess-dead2",
        "TASK-1516",
        std::path::PathBuf::from("/nonexistent/bug-1656-wt-task-1516"),
    )];
    let probe = WorktreeGitProbe {
        dirty: true,
        ahead_of_main: 0,
        last_commit_subject: Some("wip".into()),
        dirty_newest_mtime_age_secs: Some(3 * 60 * 60),
        untracked_only: false,
    };
    let (rows, orphans) = running_work(&specs, &leases, &[], probe);
    let row = rows
        .iter()
        .find(|r| r.spec.as_deref() == Some("TASK-1516"))
        .unwrap();
    assert_eq!(
        row.dispatch.as_ref().unwrap().state,
        DispatchState::Salvageable
    );
    assert_eq!(orphans.len(), 1);
    assert!(orphans[0].stale_lease);
    assert!(!orphans[0].possibly_subagent);
    let items = orphaned_in_progress_items(orphans, |_| "last touched 3h ago".to_string());
    assert_eq!(items.len(), 1);
    assert!(items[0].abandoned);
    assert!(!items[0].possibly_subagent);
}

// --- (3) possibly worked by a subagent ------------------------------------------

#[test]
fn bug_1656_stale_lease_next_to_live_harness_lease_says_possibly_subagent() {
    let harness_dir = tempfile::tempdir().unwrap();
    let specs = vec![in_progress_spec("BUG-1641", "worked by a subagent")];
    let leases = vec![
        lease(
            "sess-dead3",
            "BUG-1641",
            std::path::PathBuf::from("/nonexistent/bug-1656-wt-bug-1641"),
        ),
        lease(
            "sess-harness",
            worktree_lease::HARNESS_WORKTREE_SCOPE,
            harness_dir.path().to_path_buf(),
        ),
    ];
    let live = vec![process_probe::LiveSession {
        pid: std::process::id(),
        cwd: harness_dir.path().to_path_buf(),
        jsonl: None,
        stale_cwd: false,
    }];
    let (_rows, orphans) = running_work(&specs, &leases, &live, WorktreeGitProbe::default());
    let orphan = orphans
        .iter()
        .find(|o| o.spec == "BUG-1641")
        .expect("stale spec lease is listed");
    assert!(orphan.stale_lease);
    assert!(
        orphan.possibly_subagent,
        "a live harness lease in the repo flags it"
    );
    assert!(
        !orphan.likely_fanout,
        "a stale spec-scoped lease is never the fan-out framing"
    );

    let items = orphaned_in_progress_items(orphans, |_| "last touched 5m ago".to_string());
    assert_eq!(items.len(), 1);
    assert!(items[0].possibly_subagent);

    let report = awaiting_you::AwaitingReport {
        orphaned_in_progress: items,
        ..Default::default()
    };
    let mut buf = Vec::new();
    report.render(false, &mut buf).unwrap();
    let text = String::from_utf8(buf).unwrap();
    assert!(text.contains("possibly worked by a subagent"), "{text}");
    assert!(!text.contains("abandoned — lease died"), "{text}");
    let json = report.to_json();
    assert_eq!(json["orphaned_in_progress"][0]["possibly_subagent"], true);
}

#[test]
fn bug_1656_stale_lease_without_live_harness_lease_is_plain_abandoned() {
    let specs = vec![in_progress_spec("BUG-1642", "no subagent anywhere")];
    let leases = vec![lease(
        "sess-dead4",
        "BUG-1642",
        std::path::PathBuf::from("/nonexistent/bug-1656-wt-bug-1642"),
    )];
    let (_rows, orphans) = running_work(&specs, &leases, &[], WorktreeGitProbe::default());
    assert_eq!(orphans.len(), 1);
    assert!(orphans[0].stale_lease);
    assert!(!orphans[0].possibly_subagent);
}

// --- (2) adopting the spec lease ------------------------------------------------

// trace:BUG-1656 | ai:codex
fn adoption_lease_fixture(
    id: &str,
    scope: &str,
    slug: &str,
    worktree_path: &str,
    custom_key: bool,
) -> String {
    format!(
        "id = \"{id}\"\nscope = \"{scope}\"\nslug = \"{slug}\"\nowner = \"t\"\n\
         worktree_path = {}\nbranch = \"claude/task-77\"\n\
         started_at = \"2026-09-26T00:00:00Z\"\nhostname = \"h\"\n{}",
        aida_core::toml_quote::toml_string(worktree_path),
        if custom_key {
            "custom_key = \"kept\"\n"
        } else {
            ""
        },
    )
}

#[test]
fn bug_1656_adoption_fixture_paths_round_trip() {
    for path in [
        r#"C:\Users\Runner\AppData\Local\Temp\wt-task-77"#,
        "worktree with spaces/wt-task-77",
        "worktree with \"quotes\"/wt-task-77",
    ] {
        for (id, scope, slug, custom_key) in [
            ("aaaa11112222", "TASK-77", "task-77", true),
            (
                "bbbb33334444",
                "harness-worktree",
                "harness-worktree",
                false,
            ),
        ] {
            let body = adoption_lease_fixture(id, scope, slug, path, custom_key);
            let parsed: toml::Value = toml::from_str(&body).unwrap();
            assert_eq!(parsed["worktree_path"].as_str(), Some(path), "{body}");
        }
    }
}

#[test]
fn bug_1656_subagent_adopts_only_the_spec_lease_of_its_own_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("wt-task-77");
    std::fs::create_dir_all(&wt).unwrap();
    let other = tmp.path().join("wt-other");
    std::fs::create_dir_all(&other).unwrap();
    assert!(subagent_adopts_lease("TASK-77", &wt, &wt));
    assert!(!subagent_adopts_lease("TASK-77", &other, &wt));
    assert!(!subagent_adopts_lease(
        worktree_lease::HARNESS_WORKTREE_SCOPE,
        &wt,
        &wt
    ));
    assert!(!subagent_adopts_lease("not-a-spec", &wt, &wt));
    assert!(!subagent_adopts_lease(
        "TASK-77",
        std::path::Path::new(""),
        &wt
    ));
}

#[test]
fn bug_1656_adoption_stamps_harness_pid_and_keeps_unknown_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let wt = root.join("wt-task-77");
    std::fs::create_dir_all(&wt).unwrap();
    let sessions = root.join(".aida").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let worktree_path = wt.to_str().unwrap();
    let body = adoption_lease_fixture("aaaa11112222", "TASK-77", "task-77", worktree_path, true);
    std::fs::write(sessions.join("aaaa11112222.toml"), &body).unwrap();
    // A harness lease for the same dir is never the adoption target.
    let harness = adoption_lease_fixture(
        "bbbb33334444",
        "harness-worktree",
        "harness-worktree",
        worktree_path,
        false,
    );
    std::fs::write(sessions.join("bbbb33334444.toml"), &harness).unwrap();

    // No harness pid: nothing adopted.
    assert_eq!(adopt_spec_lease_for_subagent(root, &wt, None), None);
    // A different cwd: nothing adopted.
    assert_eq!(
        adopt_spec_lease_for_subagent(root, &root.join("elsewhere"), Some(std::process::id())),
        None
    );

    let pid = std::process::id();
    let adopted = adopt_spec_lease_for_subagent(root, &wt, Some(pid));
    assert_eq!(adopted.as_deref(), Some("aaaa11112222"));
    let after = std::fs::read_to_string(sessions.join("aaaa11112222.toml")).unwrap();
    assert!(after.contains(&format!("active_pid = {pid}")), "{after}");
    assert!(after.contains("adopted_by_subagent_at = "), "{after}");
    assert!(after.contains("custom_key = \"kept\""), "{after}");
    let harness_after = std::fs::read_to_string(sessions.join("bbbb33334444.toml")).unwrap();
    assert!(!harness_after.contains("active_pid"), "{harness_after}");

    // The adopted lease now reads Live through the ordinary lease machinery.
    let parsed: SessionLease = toml::from_str(&after).unwrap();
    assert_eq!(parsed.active_pid, Some(pid));
    assert_eq!(
        lease_state_for(&parsed, &[], chrono::Utc::now()),
        LeaseState::Live
    );

    // Already-live active_pid is left alone (idempotent, no re-stamp).
    let again = adopt_spec_lease_for_subagent(root, &wt, Some(pid));
    assert_eq!(again, None);
}

// --- porcelain parsing and the recency filter -----------------------------------

/// A committed single-file repo for the porcelain probes.
fn one_file_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.invalid"]);
    git(&["config", "user.name", "t"]);
    git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    git(&["add", "a.txt"]);
    git(&["commit", "-qm", "init"]);
    tmp
}

/// Regression: a single unstaged ` M` edit is the FIRST (and only) porcelain
/// record, whose leading space a trimmed read used to strip — cutting the
/// path to `.txt` so the recency check never fired.
#[test]
fn bug_1656_single_unstaged_modify_is_recent_movement() {
    let tmp = one_file_repo();
    let root = tmp.path();
    std::fs::write(root.join("a.txt"), "two\n").unwrap();
    assert_eq!(
        crate::dispatch_health_ps::dirty_paths(root),
        vec![root.join("a.txt")]
    );
    let probe = crate::dispatch_health_ps::probe_worktree(root);
    assert!(probe.dirty);
    assert!(
        probe.dirty_newest_mtime_age_secs.is_some_and(|a| a < 60),
        "{:?}",
        probe.dirty_newest_mtime_age_secs
    );
    assert!(dirty_movement_is_fresh(
        probe.dirty_newest_mtime_age_secs,
        DEFAULT_DIRTY_MOVEMENT_FRESH_SECS
    ));
}

/// A single unstaged ` D` delete: the path parses intact, but a gone file has
/// no mtime, so it is dirty yet never "recent" — the crashed-session
/// salvage verdict is kept.
#[test]
fn bug_1656_single_unstaged_delete_is_dirty_but_not_recent() {
    let tmp = one_file_repo();
    let root = tmp.path();
    std::fs::remove_file(root.join("a.txt")).unwrap();
    assert_eq!(
        crate::dispatch_health_ps::dirty_paths(root),
        vec![root.join("a.txt")]
    );
    let probe = crate::dispatch_health_ps::probe_worktree(root);
    assert!(probe.dirty);
    assert_eq!(probe.dirty_newest_mtime_age_secs, None);
}

#[test]
fn bug_1656_parse_porcelain_z_handles_leading_space_and_renames() {
    use crate::dispatch_health_ps::parse_porcelain_z;
    let out = " M a.txt\0R  new name.rs\0old name.rs\0?? dir/u.txt\0 D gone.rs\0";
    assert_eq!(
        parse_porcelain_z(out),
        vec![
            (" M".to_string(), "a.txt".to_string()),
            ("R ".to_string(), "new name.rs".to_string()),
            ("??".to_string(), "dir/u.txt".to_string()),
            (" D".to_string(), "gone.rs".to_string()),
        ]
    );
    assert!(parse_porcelain_z("").is_empty());
}

#[test]
fn bug_1656_swap_and_hidden_untracked_files_do_not_count_as_movement() {
    use crate::dispatch_health_ps::counts_toward_recent_movement as counts;
    // Tracked changes always count, whatever their name.
    assert!(counts(" M", "src/lib.rs"));
    assert!(counts("M ", ".github/ci.yml"));
    assert!(counts("R ", "b.rs"));
    // Ordinary untracked files count.
    assert!(counts("??", "src/new.rs"));
    // Editor swap / backup / lock names never count.
    assert!(!counts("??", "src/.lib.rs.swp"));
    assert!(!counts("??", "src/lib.rs~"));
    assert!(!counts("??", "src/.#lib.rs"));
    // Anything under a hidden directory, or a hidden file, never counts.
    assert!(!counts("??", ".codegraph/index.db"));
    assert!(!counts("??", "sub/.cache/x"));
    assert!(!counts("??", ".envrc"));
    // Ignored never counts.
    assert!(!counts("!!", "target/x"));
}

#[test]
fn bug_1656_untracked_swap_file_alone_is_not_recent_movement() {
    let tmp = one_file_repo();
    let root = tmp.path();
    std::fs::write(root.join(".a.txt.swp"), "swap").unwrap();
    std::fs::create_dir_all(root.join(".codegraph")).unwrap();
    std::fs::write(root.join(".codegraph").join("idx"), "x").unwrap();
    let probe = crate::dispatch_health_ps::probe_worktree(root);
    assert!(probe.dirty, "untracked files still make the tree dirty");
    assert_eq!(probe.dirty_newest_mtime_age_secs, None);
}

#[test]
fn bug_1656_future_mtime_is_unknown_not_recent() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("future.rs");
    std::fs::write(&f, "x").unwrap();
    // "now" an hour before the file was written: its mtime is in the future.
    let past_now = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    assert_eq!(newest_mtime_age_secs(&[f], past_now), None);
}
