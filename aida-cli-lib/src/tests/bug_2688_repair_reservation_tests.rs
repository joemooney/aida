//! BUG-2688: a conflict-repair reservation must not outlive the attempt that
//! made it. The loop observed on 2026-11-20: a conflict lane punts and leaves
//! its reservation with an unknown spawn outcome; the next reconcile is
//! refused on its route but reserves the head again first; every later
//! attempt is refused for "host journal evidence". These tests run that loop
//! against real Git seats and the real pickup rollback:
//!
//! - a route refusal reserves nothing and leaves the seat untouched;
//! - a launch that fails before a worker spawned releases its own reservation;
//! - a reservation whose lane is gone is found and released by the doctor heal.
// trace:BUG-2688 | ai:claude

use super::tests::{fixture, git_ok, Fixture};
use super::*;
use crate::queue_cmd::{
    prepare_and_route_rework_conflict, run_pickup_launch, AgentLaunchMark, ReworkConflictLaunch,
};
use crate::repair_journal::{RepairGuard, SpawnOutcome};
use crate::session::HeadlessVendor;
use anyhow::Result;
use std::path::{Path, PathBuf};

const PR: u64 = 4365;

/// Branch `feat` and `main` both rewrite README.md and are pushed, so merging
/// `main` into the PR head conflicts. Returns the PR head.
fn conflicting_pr(fx: &Fixture) -> String {
    git_ok(&fx.main, &["checkout", "-qb", "feat"]);
    std::fs::write(fx.main.join("README.md"), "lane\n").unwrap();
    git_ok(&fx.main, &["commit", "-qam", "lane"]);
    git_ok(&fx.main, &["push", "-q", "origin", "feat"]);
    let head = git_ok(&fx.main, &["rev-parse", "HEAD"]);
    git_ok(&fx.main, &["checkout", "-q", "main"]);
    std::fs::write(fx.main.join("README.md"), "main moved\n").unwrap();
    git_ok(&fx.main, &["commit", "-qam", "main moved"]);
    git_ok(&fx.main, &["push", "-q", "origin", "main"]);
    head
}

/// A fresh linked-worktree seat on the PR head.
fn seat(fx: &Fixture, name: &str) -> PathBuf {
    let path = fx.root.join(name);
    git_ok(
        &fx.main,
        &["worktree", "add", "-q", path.to_str().unwrap(), "feat"],
    );
    path
}

fn remove_seat(fx: &Fixture, seat: &Path) {
    git_ok(
        &fx.main,
        &["worktree", "remove", "--force", seat.to_str().unwrap()],
    );
    assert!(!seat.exists());
}

fn merging(seat: &Path) -> bool {
    crate::test_git::git_command()
        .arg("-C")
        .arg(seat)
        .args(["rev-parse", "-q", "--verify", "MERGE_HEAD"])
        .output()
        .unwrap()
        .status
        .success()
}

/// (lane, spawn outcome, reserved_at) of every journal entry.
fn journal(main: &Path) -> Vec<(String, SpawnOutcome, String)> {
    if !main.join(".aida/repair-journal.json").exists() {
        return Vec::new();
    }
    RepairGuard::lock(main)
        .unwrap()
        .entries()
        .iter()
        .map(|e| (e.lane_id.clone(), e.spawn.clone(), e.reserved_at.clone()))
        .collect()
}

fn launch(vendor: HeadlessVendor, contained: bool) -> ReworkConflictLaunch<'static> {
    ReworkConflictLaunch {
        vendor,
        resume: false,
        interactive: false,
        contained,
        scope: "BUG-2688",
        pr: PR,
        branch: "feat",
    }
}

/// The pickup's own order: route admission, host preparation, route.
fn reconcile(
    fx: &Fixture,
    seat: &Path,
    lane: &str,
    vendor: HeadlessVendor,
    contained: bool,
) -> Result<HeadlessVendor> {
    let mut prompt = String::from("/aida-pickup BUG-2688");
    let mut model = None;
    prepare_and_route_rework_conflict(
        &fx.main,
        launch(vendor, contained),
        &mut prompt,
        &mut model,
        |admit| {
            prepare_rework_merge_fixture_admitted(&fx.main, seat, "feat", false, PR, lane, admit)
        },
        |_, _| panic!("no conflict-only mail is sent"),
    )
}

/// A lane whose route is admitted prepares the conflict on its seat.
fn admitted_prepare(fx: &Fixture, seat: &Path, lane: &str) -> Result<Vec<String>> {
    prepare_rework_merge_fixture_admitted(&fx.main, seat, "feat", false, PR, lane, &|| Ok(()))
}

/// The 2026-11-20 loop, end to end.
#[test]
fn bug_2688_refused_route_reserves_nothing_and_a_gone_lane_heals_without_a_hand_edit() {
    let fx = fixture();
    let head = conflicting_pr(&fx);

    // (1) A conflict lane prepares, then punts: its seat is removed and its
    // reservation stays behind with an unknown spawn outcome.
    let first = seat(&fx, "seat-1");
    assert_eq!(
        admitted_prepare(&fx, &first, "lane-1").unwrap(),
        ["README.md"]
    );
    assert!(merging(&first));
    remove_seat(&fx, &first);
    let stuck = journal(&fx.main);
    assert_eq!(stuck.len(), 1);
    assert_eq!(
        (stuck[0].0.as_str(), &stuck[0].1),
        ("lane-1", &SpawnOutcome::Unknown)
    );

    // The next attempt is refused, and the refusal names the holder and the
    // one command that releases it. Its own seat is left as it was found.
    let second = seat(&fx, "seat-2");
    let blocked = format!(
        "{:#}",
        admitted_prepare(&fx, &second, "lane-2").unwrap_err()
    );
    assert!(blocked.contains("lane-1"), "{blocked}");
    assert!(
        blocked.contains("aida doctor heal repair-journal --yes"),
        "{blocked}"
    );
    assert!(!merging(&second));
    assert_eq!(
        journal(&fx.main),
        stuck,
        "a refused reserve changes nothing"
    );

    // (2) No hand edit: the doctor finds the reservation of the gone lane and
    // releases it under the journal lock, with a backup.
    let findings = crate::repair_journal::scan(&fx.main);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].safe_heal);
    assert!(findings[0].id.contains(&head), "{findings:?}");
    let before_heal = std::fs::read(fx.main.join(".aida/repair-journal.json")).unwrap();
    let backup = crate::repair_journal::heal_dead_lane_reservation(&fx.main, &findings[0].id)
        .unwrap()
        .expect("a gone lane's reservation is released");
    let backups: Vec<_> = std::fs::read_dir(fx.main.join(".aida"))
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("repair-journal.backup-"))
        })
        .collect();
    assert_eq!(backups, [backup]);
    assert_eq!(std::fs::read(&backups[0]).unwrap(), before_heal);
    let released = journal(&fx.main);
    assert_eq!(
        (released[0].0.as_str(), &released[0].1),
        ("lane-1", &SpawnOutcome::ProvenDead)
    );
    assert!(crate::repair_journal::scan(&fx.main).is_empty());

    // (3) A reconcile on a route that may not publish the merge is refused
    // with the typed route refusal itself, BEFORE any reservation: the journal
    // is byte-identical and the seat has no merge and no changes.
    let journal_bytes = std::fs::read(fx.main.join(".aida/repair-journal.json")).unwrap();
    for (vendor, contained, route) in [
        (
            HeadlessVendor::Claude,
            true,
            "self-published contained claude lane",
        ),
        (
            HeadlessVendor::Codex,
            false,
            "self-published uncontained codex lane",
        ),
    ] {
        let error = reconcile(&fx, &second, "lane-3", vendor, contained).unwrap_err();
        let refusal = error
            .downcast_ref::<crate::repair_journal::UnvalidatedRepairPublication>()
            .unwrap_or_else(|| panic!("untyped refusal: {error:#}"));
        assert_eq!(refusal.route, route);
        let text = format!("{error:#}");
        assert!(text.contains("contained Codex lane"), "{text}");
        assert!(!text.contains("host journal evidence"), "{text}");
        assert_eq!(
            std::fs::read(fx.main.join(".aida/repair-journal.json")).unwrap(),
            journal_bytes,
            "a route refusal must not touch the repair journal"
        );
        assert!(!merging(&second));
        assert!(git_ok(&second, &["status", "--porcelain"]).is_empty());
        assert_eq!(git_ok(&second, &["rev-parse", "HEAD"]), head);
    }

    // (4) The next admitted attempt is no longer refused: it reserves the
    // head for its own lane and retains the conflict for the worker.
    assert_eq!(
        admitted_prepare(&fx, &second, "lane-4").unwrap(),
        ["README.md"]
    );
    assert!(merging(&second));
    let reserved = journal(&fx.main);
    assert_eq!(reserved.len(), 1);
    assert_eq!(
        (reserved[0].0.as_str(), &reserved[0].1),
        ("lane-4", &SpawnOutcome::Unknown)
    );
    // Its seat still exists, so the doctor does not offer to release it.
    assert!(crate::repair_journal::scan(&fx.main).is_empty());
}

/// With no earlier reservation at all, a refused route leaves no journal.
#[test]
fn bug_2688_first_route_refusal_creates_no_reservation() {
    let fx = fixture();
    let head = conflicting_pr(&fx);
    let tree = seat(&fx, "seat-1");
    let error = reconcile(&fx, &tree, "lane-1", HeadlessVendor::Claude, true).unwrap_err();
    assert!(
        error
            .downcast_ref::<crate::repair_journal::UnvalidatedRepairPublication>()
            .is_some(),
        "{error:#}"
    );
    assert!(journal(&fx.main).is_empty());
    assert!(!merging(&tree));
    assert_eq!(git_ok(&tree, &["rev-parse", "HEAD"]), head);
    // A clean rework on the same route has no conflict to refuse: the route
    // check is asked only once a conflict is observed.
    git_ok(&fx.main, &["checkout", "-qb", "clean", "origin/main"]);
    git_ok(&fx.main, &["push", "-q", "origin", "clean"]);
    git_ok(&fx.main, &["checkout", "-q", "main"]);
    let asked = std::cell::Cell::new(false);
    let clean = fx.root.join("seat-clean");
    git_ok(
        &fx.main,
        &["worktree", "add", "-q", clean.to_str().unwrap(), "clean"],
    );
    let files = prepare_rework_merge_fixture_admitted(
        &fx.main,
        &clean,
        "clean",
        false,
        PR,
        "lane-2",
        &|| {
            asked.set(true);
            anyhow::bail!("must not be asked")
        },
    )
    .unwrap();
    assert!(files.is_empty());
    assert!(!asked.get());
}

/// The setup a launching pickup creates for `scope`, with its rollback armed.
fn launching_setup(scope: &str) -> crate::session_start::SessionStartReceipt {
    let receipt = crate::session_start::session_start_with_receipt(
        scope,
        None,
        None,
        false,
        None,
        None,
        "auto",
        false,
        None,
        false,
        None,
        None,
        false,
        None,
        /* force_claim */ false,
        /* use_pool */ Some(false),
        /* self_claim */ true,
        /* review_coexistence */ None,
        /* contained_seat */ false,
        /* rollback_failed_setup */ true,
        crate::session_start::PathReentry::None,
        None,
        /* done_rework_admitted */ false,
    )
    .unwrap();
    assert!(receipt.owns_setup(), "a launching pickup owns its setup");
    receipt
}

fn lease_tree(main: &Path, scope: &str) -> (String, PathBuf) {
    crate::list_leases(main)
        .into_iter()
        .find(|l| l.scope == scope)
        .map(|l| (l.id, l.worktree_path))
        .expect("setup minted a lease")
}

fn reserve_for(main: &Path, lane: &str, tree: &Path) -> crate::repair_journal::RepairKey {
    let key = crate::repair_journal::RepairKey {
        repository: main.into(),
        pr: PR,
        original_head: "a".repeat(40),
    };
    RepairGuard::lock(main)
        .unwrap()
        .reserve(crate::repair_journal::RepairEntry {
            key: key.clone(),
            dispatched_by: "direct-rework".into(),
            reserved_at: chrono::Utc::now().to_rfc3339(),
            lane_id: lane.into(),
            tree_path: tree.into(),
            branch: "feat".into(),
            base: "b".repeat(40),
            spawn: SpawnOutcome::Unknown,
            original_conflict_paths: vec!["README.md".into()],
            regeneration_success: None,
            regenerated_tree: None,
            preparation_success: None,
            validation: None,
            refusal_notice: None,
        })
        .unwrap();
    key
}

/// A launch that reserved the conflict and then failed before any worker
/// spawned is rolled back, and its own reservation is recorded proven dead:
/// the retry reserves the same head. Once a worker can have run, the
/// reservation is the worker's and stays.
#[test]
fn bug_2688_pre_spawn_failure_releases_the_launch_own_reservation() {
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);

    let scope = "TASK-926881";
    let receipt = launching_setup(scope);
    let (lane, tree) = lease_tree(&fx.main, scope);
    let (result, rolled_back) = run_pickup_launch(&fx.main, receipt, |_agent| {
        reserve_for(&fx.main, &lane, &tree);
        anyhow::bail!("pickup brief could not be delivered")
    });
    assert!(result.is_err());
    assert!(rolled_back);
    assert!(!tree.exists());
    let entries = journal(&fx.main);
    assert_eq!(entries.len(), 1);
    assert_eq!(
        (entries[0].0.as_str(), &entries[0].1),
        (lane.as_str(), &SpawnOutcome::ProvenDead)
    );

    // The retry is a new lane on a new setup and reserves the same head.
    let receipt = launching_setup(scope);
    let (retry_lane, retry_tree) = lease_tree(&fx.main, scope);
    assert_ne!(retry_lane, lane);
    let (result, rolled_back) = run_pickup_launch(&fx.main, receipt, |agent: &AgentLaunchMark| {
        reserve_for(&fx.main, &retry_lane, &retry_tree);
        agent.starting();
        anyhow::bail!("the worker punted")
    });
    assert!(result.is_err());
    assert!(!rolled_back, "a worker can have run: the setup is kept");
    let entries = journal(&fx.main);
    assert_eq!(entries.len(), 1);
    assert_eq!(
        (entries[0].0.as_str(), &entries[0].1),
        (retry_lane.as_str(), &SpawnOutcome::Unknown),
        "a spawned worker's reservation is not released by the launcher"
    );
    // Its lease and seat still stand, so the doctor keeps it too.
    assert!(crate::repair_journal::scan(&fx.main).is_empty());
}
