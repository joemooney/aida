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

use super::tests::{fixture, Fixture};
use super::*;
use crate::queue_cmd::{
    admit_rework_conflict_launch, apply_rework_conflict_route, rework_seat_for, run_pickup_launch,
    AgentLaunchMark, ReworkConflictLaunch,
};
use crate::repair_journal::{RepairGuard, SpawnOutcome};
use crate::session::HeadlessVendor;
use anyhow::Result;
use std::path::{Path, PathBuf};

fn git_ok(root: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

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
    apply_rework_conflict_route(
        launch(vendor, contained),
        &mut prompt,
        |admit| {
            prepare_rework_merge_fixture_admitted(&fx.main, seat, "feat", false, PR, lane, admit)
        },
        admit_rework_conflict_launch,
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
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &fx.main,
        "implementer",
        &[],
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );

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

#[test]
fn bug_2688_replacement_pickup_pins_remote_for_new_and_resumed_conversations() {
    for resume in [false, true] {
        let fx = fixture();
        let head = conflicting_pr(&fx);
        let removed = seat(&fx, "removed-seat");
        remove_seat(&fx, &removed);
        assert!(!fx.main.join(".aida/repair-journal.json").exists());
        git_ok(&fx.main, &["branch", "-f", "feat", &format!("{head}^")]);
        let replacement = seat(&fx, "replacement");
        assert_ne!(git_ok(&replacement, &["rev-parse", "HEAD"]), head);

        let mut launch = launch(HeadlessVendor::Codex, true);
        launch.resume = resume;
        let mut prompt = String::new();
        let admission_count = std::cell::Cell::new(0);
        let vendor = apply_rework_conflict_route(
            launch,
            &mut prompt,
            |admit| {
                prepare_rework_merge_fixture_for_seat(
                    &fx.main,
                    &replacement,
                    "feat",
                    rework_seat_for(false, false),
                    PR,
                    "lane-1",
                    admit,
                )
            },
            |facts| {
                assert_eq!(facts.resume, resume);
                admission_count.set(admission_count.get() + 1);
                Ok(())
            },
            |_, _| panic!("no conflict-only mail"),
        )
        .unwrap();
        assert_eq!(vendor, HeadlessVendor::Codex);
        assert_eq!(admission_count.get(), 1);
        assert!(prompt.contains("README.md"));
        assert!(merging(&replacement));
        assert_eq!(git_ok(&replacement, &["rev-parse", "HEAD"]), head);

        // A genuinely retained merge resumes through the same route using its
        // verified adoption and journal, irrespective of the conversation.
        let admitted = std::cell::Cell::new(0);
        apply_rework_conflict_route(
            launch,
            &mut prompt,
            |admit| {
                prepare_rework_merge_fixture_for_seat(
                    &fx.main,
                    &replacement,
                    "feat",
                    rework_seat_for(true, false),
                    PR,
                    "lane-1",
                    admit,
                )
            },
            |_| {
                admitted.set(admitted.get() + 1);
                Ok(())
            },
            |_, _| panic!("no conflict-only mail"),
        )
        .unwrap();
        assert_eq!(admitted.get(), 1);
    }
}

#[test]
fn bug_2688_fresh_pickup_refused_route_keeps_retained_seat_and_reservation() {
    let fx = fixture();
    conflicting_pr(&fx);
    let retained = seat(&fx, "retained");
    assert_eq!(
        admitted_prepare(&fx, &retained, "retained-lane").unwrap(),
        ["README.md"]
    );
    let journal_path = fx.main.join(".aida/repair-journal.json");
    let before = std::fs::read(&journal_path).unwrap();
    let merge = git_ok(&retained, &["rev-parse", "MERGE_HEAD"]);
    let index = git_ok(&retained, &["ls-files", "--stage"]);
    let branch = git_ok(&fx.main, &["rev-parse", "refs/heads/feat"]);

    let error = launch(HeadlessVendor::Claude, true)
        .preserve_for_fresh_pickup(&fx.main, "main")
        .unwrap_err();
    assert!(
        error
            .downcast_ref::<crate::repair_journal::UnvalidatedRepairPublication>()
            .is_some(),
        "{error:#}"
    );
    assert_eq!(git_ok(&retained, &["rev-parse", "MERGE_HEAD"]), merge);
    assert_eq!(git_ok(&retained, &["ls-files", "--stage"]), index);
    assert_eq!(git_ok(&fx.main, &["rev-parse", "refs/heads/feat"]), branch);
    assert_eq!(std::fs::read(journal_path).unwrap(), before);
    assert!(!std::fs::read_dir(&fx.root).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("retained.retained-")));
}

fn retained_private_clone(main: &Path, path: &Path) {
    create(main, path, "feat", "origin/feat").unwrap();
    let base = git_ok(main, &["rev-parse", "origin/main"]);
    let merge = crate::test_git::git_command()
        .current_dir(path)
        .args([
            "-c",
            "user.name=AIDA Test",
            "-c",
            "user.email=test@example.com",
            "merge",
            "--no-edit",
            &base,
        ])
        .output()
        .unwrap();
    assert!(!merge.status.success(), "{merge:?}");
    assert!(merging(path), "{merge:?}");
    std::fs::write(path.join("README.md"), "unfinished draft").unwrap();
}

#[test]
fn bug_2688_fresh_pickup_preserves_project_private_clone_without_a_lease() {
    let fx = fixture();
    let head = conflicting_pr(&fx);
    let retained = clone_path(&fx.main, "bug-2688").unwrap();
    retained_private_clone(&fx.main, &retained);
    assert!(host_recorded_clone(&fx.main, &retained).is_none());
    let index = git_ok(&retained, &["ls-files", "--stage"]);
    let merge = git_ok(&retained, &["rev-parse", "MERGE_HEAD"]);
    let old_pin = pin_ref(&retained).unwrap();

    let route = launch(HeadlessVendor::Codex, true);
    let archives = archive_retained_rework_with(
        &fx.main,
        route.scope,
        route.pr,
        route.branch,
        &head,
        || admit_rework_conflict_launch(route),
        |path| {
            assert_eq!(path, retained);
            Ok(())
        },
    )
    .unwrap();
    let [archive] = archives.as_slice() else {
        panic!("expected one archive, got {archives:?}");
    };
    assert!(!retained.exists());
    assert_eq!(git_ok(archive, &["rev-parse", "MERGE_HEAD"]), merge);
    assert_eq!(git_ok(archive, &["ls-files", "--stage"]), index);
    assert_eq!(
        std::fs::read_to_string(archive.join("README.md")).unwrap(),
        "unfinished draft"
    );
    assert_eq!(git_ok(&fx.main, &["for-each-ref", &old_pin]), "");
    assert_eq!(
        git_ok(&fx.main, &["rev-parse", &pin_ref(archive).unwrap()]),
        head
    );
    assert!(host_recorded_clone(&fx.main, archive).is_none());
}

#[test]
fn bug_2688_fresh_pickup_refuses_wrong_main_or_unverified_private_clone() {
    let fx = fixture();
    conflicting_pr(&fx);
    let other = fixture();
    let head = conflicting_pr(&other);
    let retained = clone_path(&fx.main, "bug-2688").unwrap();
    retained_private_clone(&other.main, &retained);
    let pin = pin_ref(&retained).unwrap();
    let merge = git_ok(&retained, &["rev-parse", "MERGE_HEAD"]);
    let index = git_ok(&retained, &["ls-files", "--stage"]);
    let marker_path = retained.join(MARKER_RELATIVE);
    let mut marker: SeatCloneMarker =
        toml::from_str(&std::fs::read_to_string(&marker_path).unwrap()).unwrap();

    // Even if the marker is then rewritten to name this project, the
    // alternate still points at the other project's objects.
    for marker_main in [&other.main, &fx.main] {
        marker.main.clone_from(marker_main);
        std::fs::write(&marker_path, toml::to_string(&marker).unwrap()).unwrap();
        let error = launch(HeadlessVendor::Codex, true)
            .preserve_for_fresh_pickup(&fx.main, "main")
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("lacks matching marker and object alternates"),
            "{error:#}"
        );
        assert_eq!(git_ok(&retained, &["rev-parse", "MERGE_HEAD"]), merge);
        assert_eq!(git_ok(&retained, &["ls-files", "--stage"]), index);
        assert_eq!(git_ok(&other.main, &["rev-parse", &pin]), head);
        assert_eq!(git_ok(&fx.main, &["for-each-ref", &pin]), "");
    }
}

#[test]
fn bug_2688_explicit_fresh_archives_the_retained_linked_merge() {
    let fx = fixture();
    let head = conflicting_pr(&fx);
    let retained = seat(&fx, "retained");
    let branch_tip = git_ok(&retained, &["rev-parse", "HEAD"]);
    std::fs::write(retained.join("lane-only"), "before merge").unwrap();
    git_ok(&retained, &["add", "lane-only"]);
    git_ok(&retained, &["commit", "-qm", "unpublished tip"]);
    let old_head = git_ok(&retained, &["rev-parse", "HEAD"]);
    assert_ne!(old_head, branch_tip);
    let merge = crate::test_git::git_command()
        .current_dir(&retained)
        .args(["merge", "--no-edit", "origin/main"])
        .output()
        .unwrap();
    // The fixture's origin/main is refreshed by its push.
    assert!(!merge.status.success(), "{merge:?}");
    assert!(merging(&retained));
    std::fs::write(retained.join("README.md"), "unfinished resolution").unwrap();
    std::fs::write(retained.join("untracked"), "untracked").unwrap();
    let exclude = git_ok(
        &retained,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "info/exclude",
        ],
    );
    std::fs::write(exclude, "ignored\n").unwrap();
    std::fs::write(retained.join("ignored"), "ignored bytes").unwrap();
    let index = git_ok(&retained, &["ls-files", "--stage"]);
    let merge_head = git_ok(&retained, &["rev-parse", "MERGE_HEAD"]);

    let (_, fetched) = fetch_rework_heads(&fx.main, "main", "feat").unwrap();
    assert_eq!(fetched, head);
    let blocked = archive_retained_rework_with(
        &fx.main,
        "BUG-2688",
        PR,
        "feat",
        &fetched,
        || Ok(()),
        |path| {
            assert_eq!(path, retained);
            anyhow::bail!("holder found")
        },
    )
    .unwrap_err();
    assert!(format!("{blocked:#}").contains("holder found"));
    assert_eq!(git_ok(&retained, &["ls-files", "--stage"]), index);

    let archive = archive_retained_rework_with(
        &fx.main,
        "BUG-2688",
        PR,
        "feat",
        &fetched,
        || Ok(()),
        |_| Ok(()),
    )
    .unwrap()
    .pop()
    .unwrap();
    assert!(!retained.exists());
    assert_eq!(git_ok(&archive, &["rev-parse", "HEAD"]), old_head);
    assert_eq!(git_ok(&archive, &["rev-parse", "MERGE_HEAD"]), merge_head);
    assert_eq!(git_ok(&archive, &["ls-files", "--stage"]), index);
    for (name, body) in [
        ("README.md", "unfinished resolution"),
        ("untracked", "untracked"),
        ("ignored", "ignored bytes"),
    ] {
        assert_eq!(std::fs::read_to_string(archive.join(name)).unwrap(), body);
    }

    let fresh = seat(&fx, "retained");
    assert_eq!(
        prepare_rework_merge_fixture_for_seat(
            &fx.main,
            &fresh,
            "feat",
            rework_seat_for(false, true),
            PR,
            "lane-1",
            &|| Ok(()),
        )
        .unwrap(),
        ["README.md"]
    );
    assert_eq!(git_ok(&fresh, &["rev-parse", "HEAD"]), head);
    assert!(merging(&fresh));
    assert!(merging(&archive));
}
