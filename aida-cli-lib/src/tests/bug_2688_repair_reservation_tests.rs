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
        &[],
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

fn fixture_heads(fx: &Fixture) -> ReworkHeads {
    let (base, head) = fetch_rework_heads(&fx.main, "main", "feat").unwrap();
    ReworkHeads {
        base_ref: "main".into(),
        base,
        head,
    }
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
// trace:BUG-b6b1278890774ef993d2ebc327862470-452.aca42931 | ai:codex
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
    assert!(format!("{error:#}").contains("--vendor codex --sandbox --fresh"));
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

// trace:BUG-b6b1278890774ef993d2ebc327862470-452.ac153421 | ai:codex
// trace:BUG-b6b1278890774ef993d2ebc327862470-452.Tests | ai:codex
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
            &[],
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
            &[],
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
        .preserve_for_fresh_pickup(&fx.main, &fixture_heads(&fx))
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
fn bug_2688_fresh_pickup_preserves_private_clone_when_origin_moves_without_a_journal() {
    if aida_testkit::test_env::run_path_isolated_test_in_scratch() {
        return;
    }
    let fx = fixture();
    let old_head = conflicting_pr(&fx);
    let retained = clone_path(&fx.main, "bug-2688").unwrap();
    retained_private_clone(&fx.main, &retained);
    assert!(host_recorded_clone(&fx.main, &retained).is_none());
    assert!(journal(&fx.main).is_empty());
    std::fs::write(retained.join("untracked"), "seat-only bytes").unwrap();
    let index = git_ok(&retained, &["ls-files", "--stage"]);
    let merge = git_ok(&retained, &["rev-parse", "MERGE_HEAD"]);
    let old_pin = pin_ref(&retained).unwrap();
    assert_eq!(git_ok(&fx.main, &["rev-parse", &old_pin]), old_head);

    git_ok(&fx.main, &["checkout", "-q", "feat"]);
    git_ok(&fx.main, &["commit", "--allow-empty", "-qm", "new PR head"]);
    let new_head = git_ok(&fx.main, &["rev-parse", "HEAD"]);
    git_ok(&fx.main, &["push", "-q", "origin", "feat"]);
    git_ok(&fx.main, &["checkout", "-q", "main"]);
    let mut metadata =
        verify_rework_metadata("feat", fixture_rework_metadata("feat", "main")).unwrap();
    metadata.head_sha.clone_from(&new_head);
    let heads = ReworkHeads::fetch(&fx.main, metadata, "feat").unwrap();
    assert_ne!(heads.head, old_head);
    heads.verify_origin(&fx.main, "feat").unwrap();

    // A pin at the new PR head cannot authenticate a marker recording H1.
    git_ok(&fx.main, &["update-ref", &old_pin, &new_head]);
    let error = archive_retained_rework_with(
        &fx.main,
        "BUG-2688",
        PR,
        "feat",
        &heads,
        || Ok(()),
        |_| Ok(()),
    )
    .unwrap_err();
    let diagnostic = format!("{error:#}");
    assert!(diagnostic.contains("no matching host pin or exact repair reservation"));
    assert!(diagnostic.contains(&format!("host pin is {new_head}")));
    assert!(diagnostic.contains("show-ref --verify"));
    assert!(diagnostic.contains("aida doctor check repair-journal"));
    assert_eq!(git_ok(&retained, &["ls-files", "--stage"]), index);
    assert_eq!(git_ok(&retained, &["rev-parse", "MERGE_HEAD"]), merge);
    assert!(journal(&fx.main).is_empty());
    git_ok(&fx.main, &["update-ref", &old_pin, &old_head]);

    let route = launch(HeadlessVendor::Codex, true);
    let archives = archive_retained_rework_with(
        &fx.main,
        route.scope,
        route.pr,
        route.branch,
        &heads,
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
    assert_eq!(git_ok(archive, &["rev-parse", "HEAD"]), old_head);
    assert_eq!(git_ok(archive, &["rev-parse", "MERGE_HEAD"]), merge);
    assert_eq!(git_ok(archive, &["ls-files", "--stage"]), index);
    assert_eq!(
        std::fs::read_to_string(archive.join("README.md")).unwrap(),
        "unfinished draft"
    );
    assert_eq!(git_ok(&fx.main, &["for-each-ref", &old_pin]), "");
    assert_eq!(
        std::fs::read_to_string(archive.join("untracked")).unwrap(),
        "seat-only bytes"
    );
    assert_eq!(
        git_ok(&fx.main, &["rev-parse", &pin_ref(archive).unwrap()]),
        old_head
    );
    assert!(host_recorded_clone(&fx.main, archive).is_none());
    assert!(journal(&fx.main).is_empty());

    create(&fx.main, &retained, "feat", &heads.head).unwrap();
    assert_eq!(git_ok(&retained, &["rev-parse", "HEAD"]), new_head);
    assert_eq!(git_ok(&fx.main, &["rev-parse", &old_pin]), new_head);
    assert!(!merging(&retained));
    assert!(merging(archive));
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
            .preserve_for_fresh_pickup(&fx.main, &fixture_heads(&fx))
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("lacks matching marker and object alternates"),
            "{error:#}"
        );
        assert!(format!("{error:#}").contains(&format!(
            "cat -- {}",
            crate::shell_quote(
                &retained
                    .join(".git/objects/info/alternates")
                    .to_string_lossy()
            )
        )));
        assert_eq!(git_ok(&retained, &["rev-parse", "MERGE_HEAD"]), merge);
        assert_eq!(git_ok(&retained, &["ls-files", "--stage"]), index);
        assert_eq!(git_ok(&other.main, &["rev-parse", &pin]), head);
        assert_eq!(git_ok(&fx.main, &["for-each-ref", &pin]), "");
    }
}

#[test]
fn bug_2688_explicit_fresh_archives_the_retained_linked_merge() {
    if aida_testkit::test_env::run_path_isolated_test_in_scratch() {
        return;
    }
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
        &fixture_heads(&fx),
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
        &fixture_heads(&fx),
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

// trace:BUG-2688 | ai:codex
#[test]
fn bug_2688_matching_private_clone_metadata_without_host_proof_is_preserved() {
    if aida_testkit::test_env::run_path_isolated_test_in_scratch() {
        return;
    }
    let fx = fixture();
    let head = conflicting_pr(&fx);
    let retained = clone_path(&fx.main, "bug-2688").unwrap();
    retained_private_clone(&fx.main, &retained);
    git_ok(
        &fx.main,
        &["update-ref", "-d", &pin_ref(&retained).unwrap()],
    );
    assert!(host_recorded_clone(&fx.main, &retained).is_none());
    assert_eq!(clone_marker_main(&retained), Some(fx.main.clone()));
    let index = git_ok(&retained, &["ls-files", "--stage"]);

    let error = archive_retained_rework_with(
        &fx.main,
        "BUG-2688",
        PR,
        "feat",
        &fixture_heads(&fx),
        || Ok(()),
        |_| Ok(()),
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("no matching host pin or exact repair reservation"));
    assert_eq!(git_ok(&retained, &["ls-files", "--stage"]), index);
    assert!(merging(&retained));
    assert!(journal(&fx.main).is_empty());
    assert_eq!(git_ok(&fx.main, &["for-each-ref", PIN_PREFIX]), "");

    // An exact journal generation supplies the missing host binding.
    let base = git_ok(&retained, &["rev-parse", "MERGE_HEAD"]);
    RepairGuard::lock(&fx.main)
        .unwrap()
        .reserve(crate::repair_journal::RepairEntry {
            key: crate::repair_journal::RepairKey {
                repository: fx.main.clone(),
                pr: PR,
                original_head: head.clone(),
            },
            dispatched_by: "direct-rework".into(),
            reserved_at: chrono::Utc::now().to_rfc3339(),
            lane_id: "retained-lane".into(),
            tree_path: retained.clone(),
            branch: "feat".into(),
            base,
            spawn: SpawnOutcome::Unknown,
            original_conflict_paths: vec!["README.md".into()],
            regeneration_success: None,
            regenerated_tree: None,
            preparation_success: None,
            validation: None,
            refusal_notice: None,
        })
        .unwrap();
    let archive = archive_retained_rework_with(
        &fx.main,
        "BUG-2688",
        PR,
        "feat",
        &fixture_heads(&fx),
        || Ok(()),
        |_| Ok(()),
    )
    .unwrap()
    .pop()
    .unwrap();
    assert!(!retained.exists());
    assert!(merging(&archive));
    assert_eq!(git_ok(&archive, &["ls-files", "--stage"]), index);
    assert!(host_recorded_clone(&fx.main, &archive).is_none());
}

#[test]
fn bug_2688_fresh_pickup_refuses_head_movement_without_replacing_its_pin() {
    if aida_testkit::test_env::run_path_isolated_test_in_scratch() {
        return;
    }
    let fx = fixture();
    let head = conflicting_pr(&fx);
    let retained = seat(&fx, "retained");
    assert_eq!(
        admitted_prepare(&fx, &retained, "retained-lane").unwrap(),
        ["README.md"]
    );
    let index = git_ok(&retained, &["ls-files", "--stage"]);
    let merge = git_ok(&retained, &["rev-parse", "MERGE_HEAD"]);
    let mut metadata =
        verify_rework_metadata("feat", fixture_rework_metadata("feat", "main")).unwrap();
    metadata.head_sha.clone_from(&head);
    let heads = ReworkHeads::fetch(&fx.main, metadata.clone(), "feat").unwrap();
    let mut retargeted = metadata.clone();
    retargeted.base_ref = "integration".into();
    assert!(heads.require_same_change(&retargeted).is_err());

    let archive = archive_retained_rework_with(
        &fx.main,
        "BUG-2688",
        PR,
        "feat",
        &heads,
        || Ok(()),
        |_| Ok(()),
    )
    .unwrap()
    .pop()
    .unwrap();

    // Move origin after archival, with no new forge observation available.
    git_ok(&fx.main, &["push", "--force", "origin", "main:feat"]);
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &fx.main,
        "implementer",
        &[],
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    // Force the remote-only placement path to distinguish the pinned commit
    // from the newer origin/feat seen by ordinary session setup.
    git_ok(&fx.main, &["branch", "-D", "feat"]);
    let scope = "TASK-926881";
    let receipt = crate::session_start::session_start_with_receipt(
        scope,
        Some("feat"),
        Some(crate::session_start::SessionBase::PinnedRework(&heads.head)),
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
        false,
        Some(false),
        true,
        None,
        false,
        true,
        crate::session_start::PathReentry::None,
        Some(Some((PR, "feat".into()))),
        false,
    )
    .unwrap();
    let (lane, fresh) = lease_tree(&fx.main, scope);
    assert_eq!(git_ok(&fresh, &["rev-parse", "HEAD"]), head);
    let (result, rolled_back) = run_pickup_launch(&fx.main, receipt, |_| {
        prepare_rework_merge(
            &fx.main,
            &fresh,
            "feat",
            ReworkSeat::Fresh,
            PR,
            &lane,
            scope,
            Some(&heads),
            &|| panic!("admission must not repeat"),
        )
        .map(|_| ())
    });
    let error = result.unwrap_err();
    let diagnostic = format!("{error:#}");
    assert!(
        diagnostic.contains("origin refs/heads/feat no longer matches pinned"),
        "{diagnostic}"
    );
    let inspect_origin = format!(
        "`git -C {} ls-remote --refs origin refs/heads/main refs/heads/feat`",
        crate::shell_quote(&fx.main.to_string_lossy()),
    );
    assert!(diagnostic.contains(&inspect_origin), "{diagnostic}");
    assert!(rolled_back);
    assert!(!fresh.exists());
    assert_eq!(git_ok(&archive, &["rev-parse", "MERGE_HEAD"]), merge);
    assert_eq!(git_ok(&archive, &["ls-files", "--stage"]), index);
    assert!(journal(&fx.main).iter().all(|entry| entry.0 != lane));

    // The same movement before the first fetch refuses the stale forge observation.
    let error = ReworkHeads::fetch(&fx.main, metadata, "feat")
        .err()
        .unwrap();
    let diagnostic = format!("{error:#}");
    assert!(diagnostic.contains("differs from the observed PR head"));
    assert!(diagnostic.contains("ls-remote --refs origin refs/heads/feat"));
}

#[test]
fn bug_2688_divergent_linked_branch_recovery_preserves_tip_and_frees_branch() {
    let fx = fixture();
    let head = conflicting_pr(&fx);
    let retained = seat(&fx, "local-divergence");
    git_ok(&retained, &["reset", "--hard", "HEAD^"]);
    git_ok(
        &retained,
        &["commit", "--allow-empty", "-qm", "unpublished history"],
    );
    let local = git_ok(&retained, &["rev-parse", "HEAD"]);

    let error = admitted_prepare(&fx, &retained, "divergent-lane").unwrap_err();
    let diagnostic = format!("{error:#}");
    let command = diagnostic.split('`').nth(1).unwrap();
    let backup = command.split_whitespace().last().unwrap();
    assert!(command.contains(" branch -m -- "));
    git_ok(&fx.main, &["branch", "-m", "--", "feat", backup]);
    assert_eq!(
        git_ok(&retained, &["symbolic-ref", "--short", "HEAD"]),
        backup
    );
    assert_eq!(git_ok(&retained, &["rev-parse", "HEAD"]), local);
    assert!(journal(&fx.main).is_empty());
    git_ok(&fx.main, &["branch", "feat", "origin/feat"]);
    let replacement = seat(&fx, "replacement");
    assert_eq!(git_ok(&replacement, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        admitted_prepare(&fx, &replacement, "replacement").unwrap(),
        ["README.md"]
    );
}

#[test]
fn bug_2688_unobservable_merge_names_the_pinned_probe_and_keeps_state() {
    let fx = fixture();
    let head = conflicting_pr(&fx);
    let base = git_ok(&fx.main, &["rev-parse", "origin/main"]);
    let tree = seat(&fx, "probe");
    let execute = |args: &[&str]| {
        if args[0] == "merge-tree" {
            anyhow::bail!("injected merge-tree failure");
        }
        Ok(git_ok(&tree, args))
    };
    let error = prepare_host_repair_with(
        &fx.main,
        &tree,
        PR,
        "probe",
        &execute,
        "feat",
        false,
        |_| panic!("unexpected publication"),
        |_, _| panic!("unexpected regeneration"),
        || Ok((base.clone(), head.clone())),
    )
    .unwrap_err();
    let diagnostic = format!("{error:#}");
    assert!(diagnostic.contains("no reservation created"));
    assert!(diagnostic.contains(&format!(
        "merge-tree --write-tree --name-only -z {} {}",
        crate::shell_quote(&head),
        crate::shell_quote(&base)
    )));
    let probe = crate::test_git::git_command()
        .current_dir(&fx.main)
        .args([
            "merge-tree",
            "--write-tree",
            "--name-only",
            "-z",
            &head,
            &base,
        ])
        .output()
        .unwrap();
    assert_eq!(probe.status.code(), Some(1), "{probe:?}");
    assert!(
        repair_merge_tree_paths(std::str::from_utf8(&probe.stdout).unwrap())
            .unwrap()
            .contains(&"README.md".into())
    );
    assert!(!merging(&tree));
    assert!(journal(&fx.main).is_empty());
    assert_eq!(
        admitted_prepare(&fx, &tree, "probe").unwrap(),
        ["README.md"]
    );
}

fn pinned_setup(
    scope: &str,
    head: &str,
    contained: bool,
) -> crate::session_start::SessionStartReceipt {
    crate::session_start::session_start_with_receipt(
        scope,
        Some("feat"),
        Some(crate::session_start::SessionBase::PinnedRework(head)),
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
        false,
        Some(false),
        true,
        None,
        contained,
        true,
        crate::session_start::PathReentry::None,
        Some(Some((PR, "feat".into()))),
        false,
    )
    .unwrap()
}

#[test]
fn bug_2688_fresh_pickup_reuses_branch_of_absent_registration_without_pruning() {
    if aida_testkit::test_env::run_path_isolated_test_in_scratch() {
        return;
    }
    let fx = fixture();
    let head = conflicting_pr(&fx);
    let scope = "TASK-926881";
    let old = seat(&fx, "aida-task-926881");
    git_ok(&old, &["commit", "--allow-empty", "-qm", "unpublished"]);
    let tip = git_ok(&old, &["rev-parse", "HEAD"]);
    let git_dir = git_ok(&old, &["rev-parse", "--absolute-git-dir"]);
    let index = std::fs::read(Path::new(&git_dir).join("index")).unwrap();
    let merge = git_ok(&fx.main, &["rev-parse", "origin/main"]);
    std::fs::write(Path::new(&git_dir).join("MERGE_HEAD"), format!("{merge}\n")).unwrap();
    let lock = crate::worktree_lock::acquire(&fx.main, &old, "advisor-a").unwrap();
    std::fs::remove_dir_all(&old).unwrap();

    let heads = fixture_heads(&fx);
    let route = launch(HeadlessVendor::Codex, true);
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &fx.main,
        "implementer",
        &[],
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    crate::queue_cmd::inspect_queue_work_branch(&fx.main, Some("feat"), None, false, false, true)
        .unwrap();
    let error = route
        .preserve_for_fresh_pickup(&fx.main, &heads)
        .unwrap_err();
    assert!(format!("{error:#}").contains("locked by advisor-a"));
    assert!(lock.exists());
    assert_eq!(git_ok(&fx.main, &["rev-parse", "feat"]), tip);
    assert!(crate::worktree_lock::release(&fx.main, &old).unwrap());
    assert!(route
        .preserve_for_fresh_pickup(&fx.main, &heads)
        .unwrap()
        .is_empty());
    let receipt = pinned_setup(scope, &heads.head, false);
    assert!(receipt.owns_setup());
    let (lane, replacement) = lease_tree(&fx.main, scope);
    assert_ne!(replacement, old);
    assert_eq!(git_ok(&replacement, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        std::fs::read(Path::new(&git_dir).join("index")).unwrap(),
        index
    );
    assert_eq!(
        std::fs::read_to_string(Path::new(&git_dir).join("MERGE_HEAD"))
            .unwrap()
            .trim(),
        merge
    );
    assert_eq!(
        std::fs::read_to_string(Path::new(&git_dir).join("HEAD"))
            .unwrap()
            .trim(),
        tip
    );
    assert!(git_ok(&fx.main, &["worktree", "list", "--porcelain"]).contains("prunable"));
    assert!(crate::lease_path(&fx.main, &lane).exists());
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-452.acc34a6d | ai:codex
#[test]
fn bug_2688_fresh_session_setup_preserves_unpublished_local_tips() {
    if aida_testkit::test_env::run_path_isolated_test_in_scratch() {
        return;
    }
    for (contained, diverged) in [(false, false), (false, true), (true, false), (true, true)] {
        let fx = fixture();
        let head = conflicting_pr(&fx);
        let fork = if diverged {
            format!("{head}^")
        } else {
            head.clone()
        };
        git_ok(&fx.main, &["checkout", "-qb", "local-only", &fork]);
        git_ok(
            &fx.main,
            &["commit", "--allow-empty", "-qm", "private commit"],
        );
        let tip = git_ok(&fx.main, &["rev-parse", "HEAD"]);
        git_ok(&fx.main, &["branch", "-f", "feat", &tip]);
        git_ok(&fx.main, &["checkout", "-q", "main"]);
        git_ok(&fx.main, &["branch", "-D", "local-only"]);

        let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            &fx.main,
            "implementer",
            &[],
            &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
        );
        let scope = "TASK-926881";
        let receipt = pinned_setup(scope, &head, contained);
        assert!(receipt.owns_setup());
        let (lane, tree) = lease_tree(&fx.main, scope);
        assert_eq!(git_ok(&tree, &["rev-parse", "HEAD"]), head);
        assert_eq!(git_ok(&fx.main, &["rev-parse", "origin/feat"]), head);
        if contained {
            assert_eq!(git_ok(&fx.main, &["rev-parse", "feat"]), tip);
            assert_eq!(
                git_ok(&fx.main, &["rev-parse", &pin_ref(&tree).unwrap()]),
                head
            );
        } else {
            assert_eq!(git_ok(&fx.main, &["rev-parse", "feat"]), head);
            assert_eq!(
                git_ok(
                    &fx.main,
                    &[
                        "for-each-ref",
                        "--format=%(objectname)",
                        "refs/heads/aida-recovery/"
                    ]
                ),
                tip
            );
        }
        assert!(crate::lease_path(&fx.main, &lane).exists());
    }
}

#[test]
fn bug_2688_fresh_clean_route_archives_old_conflict_and_respects_advisor_lock() {
    if aida_testkit::test_env::run_path_isolated_test_in_scratch() {
        return;
    }
    let fx = fixture();
    let old = conflicting_pr(&fx);
    let retained = seat(&fx, "retained");
    assert_eq!(
        admitted_prepare(&fx, &retained, "old-lane").unwrap(),
        ["README.md"]
    );
    let merge = git_ok(&retained, &["rev-parse", "MERGE_HEAD"]);
    let index = git_ok(&retained, &["ls-files", "--stage"]);
    std::fs::write(retained.join("draft"), "keep me").unwrap();

    git_ok(
        &fx.main,
        &["commit", "--allow-empty", "-qm", "resolved PR branch"],
    );
    let head = git_ok(&fx.main, &["rev-parse", "HEAD"]);
    git_ok(&fx.main, &["push", "-q", "--force", "origin", "HEAD:feat"]);
    let heads = fixture_heads(&fx);
    assert_eq!(heads.head, head);
    let route = launch(HeadlessVendor::Claude, true);
    assert!(admit_rework_conflict_launch(route).is_err());
    let lock = crate::worktree_lock::acquire(&fx.main, &retained, "advisor-a").unwrap();
    let lock_bytes = std::fs::read(&lock).unwrap();
    let old_journal = journal(&fx.main);
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &fx.main,
        "implementer",
        &[],
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    let error = route
        .preserve_for_fresh_pickup(&fx.main, &heads)
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("locked by advisor-a"),
        "{error:#}"
    );
    assert_eq!(std::fs::read(&lock).unwrap(), lock_bytes);
    assert_eq!(journal(&fx.main), old_journal);
    assert_eq!(git_ok(&retained, &["ls-files", "--stage"]), index);
    assert!(crate::worktree_lock::release(&fx.main, &retained).unwrap());

    let archives = route.preserve_for_fresh_pickup(&fx.main, &heads).unwrap();
    let [archive] = archives.as_slice() else {
        panic!("expected one archive: {archives:?}");
    };
    assert_eq!(git_ok(archive, &["rev-parse", "HEAD"]), old);
    assert_eq!(git_ok(archive, &["rev-parse", "MERGE_HEAD"]), merge);
    assert_eq!(git_ok(archive, &["ls-files", "--stage"]), index);
    assert_eq!(
        std::fs::read_to_string(archive.join("draft")).unwrap(),
        "keep me"
    );
    let scope = "TASK-926881";
    let receipt = pinned_setup(scope, &head, false);
    assert!(receipt.owns_setup());
    let (lane, fresh) = lease_tree(&fx.main, scope);
    assert!(prepare_rework_merge_fixture_for_seat(
        &fx.main,
        &fresh,
        "feat",
        ReworkSeat::Fresh,
        PR,
        &lane,
        &|| admit_rework_conflict_launch(route),
    )
    .unwrap()
    .is_empty());
    assert!(crate::lease_path(&fx.main, &lane).exists());
}

#[test]
fn bug_2688_adoption_rejects_another_pr_or_new_base_without_touching_merge() {
    let fx = fixture();
    let head = conflicting_pr(&fx);
    let retained = seat(&fx, "retained");
    let lane = "original-lane";
    assert_eq!(
        admitted_prepare(&fx, &retained, lane).unwrap(),
        ["README.md"]
    );
    let merge = git_ok(&retained, &["rev-parse", "MERGE_HEAD"]);
    let index = git_ok(&retained, &["ls-files", "--stage"]);
    let old_journal = journal(&fx.main);
    let refuse = |pr| {
        let error = prepare_rework_merge_fixture_for_seat(
            &fx.main,
            &retained,
            "feat",
            ReworkSeat::Adopted,
            pr,
            lane,
            &|| panic!("stale adoption must refuse before admission"),
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("aida queue work"),
            "{error:#}"
        );
        assert!(format!("{error:#}").contains("--fresh"), "{error:#}");
        assert_eq!(git_ok(&retained, &["rev-parse", "HEAD"]), head);
        assert_eq!(git_ok(&retained, &["rev-parse", "MERGE_HEAD"]), merge);
        assert_eq!(git_ok(&retained, &["ls-files", "--stage"]), index);
        assert_eq!(journal(&fx.main), old_journal);
    };
    refuse(PR + 1);
    git_ok(
        &fx.main,
        &["commit", "--allow-empty", "-qm", "base advanced"],
    );
    git_ok(&fx.main, &["push", "-q", "origin", "main"]);
    refuse(PR);
}

#[test]
fn bug_2688_conflict_merge_failure_preserves_git_error_ignored_files_and_lease() {
    if aida_testkit::test_env::run_path_isolated_test_in_scratch() {
        return;
    }
    let fx = fixture();
    let head = conflicting_pr(&fx);
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &fx.main,
        "implementer",
        &[],
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    let scope = "TASK-926881";
    let receipt = pinned_setup(scope, &head, false);
    let (lane, tree) = lease_tree(&fx.main, scope);
    let lease = crate::lease_path(&fx.main, &lane);
    let lease_bytes = std::fs::read(&lease).unwrap();
    let exclude = git_ok(
        &tree,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "info/exclude",
        ],
    );
    let existing_excludes = std::fs::read_to_string(&exclude).unwrap();
    std::fs::write(exclude, format!("{existing_excludes}\nobstruction\n")).unwrap();
    std::fs::write(tree.join("obstruction"), "keep these ignored bytes").unwrap();
    assert_eq!(git_ok(&tree, &["status", "--porcelain"]), "");
    let before = git_ok(&tree, &["ls-files", "--stage"]);
    // Use a real Git failure before MERGE_HEAD. merge-tree reports the
    // conflict, while the actual merge reads branch-specific strategy options.
    let strategy = "missing-rework-strategy";
    git_ok(
        &tree,
        &[
            "config",
            "branch.feat.mergeOptions",
            &format!("-s {strategy}"),
        ],
    );
    let (result, rolled_back) = run_pickup_launch(&fx.main, receipt, |_| {
        prepare_rework_merge_fixture_for_seat(
            &fx.main,
            &tree,
            "feat",
            ReworkSeat::Fresh,
            PR,
            &lane,
            &|| Ok(()),
        )
        .map(|_| ())
    });
    assert!(!rolled_back);
    assert_eq!(std::fs::read(lease).unwrap(), lease_bytes);
    let error = result.unwrap_err();
    let diagnostic = format!("{error:#}");
    assert!(diagnostic.contains(strategy), "{diagnostic}");
    assert!(
        diagnostic.contains("Could not find merge strategy"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("--fresh"), "{diagnostic}");
    assert!(!merging(&tree));
    assert_eq!(git_ok(&tree, &["ls-files", "--stage"]), before);
    assert_eq!(
        std::fs::read_to_string(tree.join("obstruction")).unwrap(),
        "keep these ignored bytes"
    );
    assert_eq!(journal(&fx.main).len(), 1);
}

#[cfg(target_os = "linux")]
#[test]
fn bug_2688_missing_pin_recovery_preserves_the_clone_and_makes_room_for_pickup() {
    if aida_testkit::test_env::run_path_isolated_test_in_scratch() {
        return;
    }
    let fx = fixture();
    let head = conflicting_pr(&fx);
    let scope = "TASK-926881";
    let retained = clone_path(&fx.main, &crate::slugify(scope)).unwrap();
    retained_private_clone(&fx.main, &retained);
    let pin = pin_ref(&retained).unwrap();
    git_ok(&fx.main, &["update-ref", "-d", &pin, &head]);
    let merge = git_ok(&retained, &["rev-parse", "MERGE_HEAD"]);
    let index = git_ok(&retained, &["ls-files", "--stage"]);
    let heads = fixture_heads(&fx);
    let mut route = launch(HeadlessVendor::Codex, true);
    route.scope = scope;
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &fx.main,
        "implementer",
        &[],
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    let error = route
        .preserve_for_fresh_pickup(&fx.main, &heads)
        .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("host pin is missing"), "{message}");
    let command = message
        .split('`')
        .find(|part| part.starts_with("mv -T --no-clobber -- "))
        .unwrap();
    assert!(std::process::Command::new("sh")
        .args(["-c", command])
        .status()
        .unwrap()
        .success());
    let archive = std::fs::read_dir(&fx.root)
        .unwrap()
        .map(|entry| entry.unwrap())
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("retained-seat-")
        })
        .unwrap()
        .path();
    assert!(!retained.exists());
    assert_eq!(git_ok(&archive, &["rev-parse", "MERGE_HEAD"]), merge);
    assert_eq!(git_ok(&archive, &["ls-files", "--stage"]), index);
    assert_eq!(git_ok(&fx.main, &["for-each-ref", &pin]), "");
    assert!(route
        .preserve_for_fresh_pickup(&fx.main, &heads)
        .unwrap()
        .is_empty());
    let receipt = pinned_setup(scope, &heads.head, true);
    assert!(receipt.owns_setup());
    let (lane, fresh) = lease_tree(&fx.main, scope);
    assert_eq!(git_ok(&fresh, &["rev-parse", "HEAD"]), head);
    assert!(crate::lease_path(&fx.main, &lane).exists());
    assert_eq!(git_ok(&archive, &["ls-files", "--stage"]), index);
}

#[test]
fn bug_2688_fresh_pickup_refuses_non_utf8_registration_before_mutation() {
    use std::os::unix::ffi::OsStrExt;

    let fx = fixture();
    let head = conflicting_pr(&fx);
    let retained = seat(&fx, "retained");
    let name = std::ffi::OsStr::from_bytes(b"non-utf8-\xff");
    let moved = fx.root.join(name);
    let status = crate::test_git::git_command()
        .current_dir(&fx.main)
        .args(["worktree", "move"])
        .arg(&retained)
        .arg(&moved)
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::write(moved.join("draft"), "preserve this").unwrap();
    crate::queue_cmd::inspect_queue_work_branch(
        &fx.main,
        Some("unrelated"),
        None,
        false,
        false,
        true,
    )
    .unwrap();
    let error = crate::queue_cmd::inspect_queue_work_branch(
        &fx.main,
        Some("feat"),
        None,
        false,
        false,
        true,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("non-UTF-8"));
    let error = launch(HeadlessVendor::Codex, true)
        .preserve_for_fresh_pickup(&fx.main, &fixture_heads(&fx))
        .unwrap_err();
    assert!(format!("{error:#}").contains("non-UTF-8"));
    assert_eq!(
        std::fs::read_to_string(moved.join("draft")).unwrap(),
        "preserve this"
    );
    assert_eq!(git_ok(&fx.main, &["rev-parse", "feat"]), head);
}

#[test]
fn bug_2688_dirty_replacement_and_missing_host_identity_name_corrective_commands() {
    if aida_testkit::test_env::run_path_isolated_test_in_scratch() {
        return;
    }
    let fx = fixture();
    conflicting_pr(&fx);
    let tree = seat(&fx, "seat");
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &fx.main,
        "implementer",
        &[],
        &[("GIT_CONFIG_GLOBAL", Some("/dev/null"))],
    );
    for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
        git_ok(&fx.main, &["config", "--unset", key]);
        let error = admitted_prepare(&fx, &tree, "lane").unwrap_err();
        let diagnostic = format!("{error:#}");
        assert!(
            diagnostic.contains(&format!("missing host {key}")),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains(&format!("config {key} <value>")),
            "{diagnostic}"
        );
        git_ok(&fx.main, &["config", key, value]);
    }
    std::fs::write(tree.join("draft"), "keep dirty").unwrap();
    let error = admitted_prepare(&fx, &tree, "lane").unwrap_err();
    let diagnostic = format!("{error:#}");
    assert!(
        diagnostic.contains("stash push --include-untracked"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("aida queue work") && diagnostic.contains("--fresh"),
        "{diagnostic}"
    );
    assert_eq!(
        std::fs::read_to_string(tree.join("draft")).unwrap(),
        "keep dirty"
    );
    assert!(!merging(&tree));
    assert!(journal(&fx.main).is_empty());
}

#[test]
fn bug_2688_fresh_pickup_waits_for_recorded_worker_without_a_lease() {
    let fx = fixture();
    conflicting_pr(&fx);
    let retained = seat(&fx, "retained");
    let lane = "worker-only";
    assert_eq!(
        admitted_prepare(&fx, &retained, lane).unwrap(),
        ["README.md"]
    );
    let before = journal(&fx.main);
    assert!(!crate::lease_path(&fx.main, lane).exists());
    let lanes = fx.main.join(".aida/lanes");
    std::fs::create_dir_all(&lanes).unwrap();
    let pid = std::process::id();
    let record = serde_json::json!({
        "lease_id": lane,
        "spec": "BUG-2688",
        "vendor": "codex",
        "pid": pid,
        "pid_start_time": crate::process_probe::process_start_identity(pid).unwrap(),
        "pid_namespace": aida_core::pid_observer::current_pid_namespace().unwrap(),
        "log_path": fx.main.join(".aida/headless-logs/fixture"),
        "started_at": chrono::Utc::now(),
    });
    std::fs::write(lanes.join("headless-worker-only.json"), record.to_string()).unwrap();
    let error = launch(HeadlessVendor::Codex, true)
        .preserve_for_fresh_pickup(&fx.main, &fixture_heads(&fx))
        .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("recorded worker"), "{message}");
    assert!(
        message.contains("after the worker exits retry"),
        "{message}"
    );
    assert!(message.contains("--fresh"), "{message}");
    assert!(!message.contains("session end"), "{message}");
    assert!(merging(&retained));
    assert_eq!(journal(&fx.main), before);
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-452.acc34a6d | ai:codex
#[test]
fn bug_2688_live_worker_without_journal_keeps_seat() {
    let fx = fixture();
    conflicting_pr(&fx);
    let retained = seat(&fx, "retained");
    admitted_prepare(&fx, &retained, "previous").unwrap();
    std::fs::remove_file(fx.main.join(".aida/repair-journal.json")).unwrap();
    let lanes = fx.main.join(".aida/lanes");
    std::fs::create_dir_all(&lanes).unwrap();
    let pid = std::process::id();
    let record = serde_json::json!({
        "lease_id":"current", "spec":"BUG-2688", "vendor":"codex", "pid":pid,
        "pid_start_time":crate::process_probe::process_start_identity(pid).unwrap(),
        "pid_namespace":aida_core::pid_observer::current_pid_namespace().unwrap(),
        "seat_path":retained, "log_path":fx.root.join("log"), "started_at":chrono::Utc::now()
    });
    std::fs::write(lanes.join("headless-current.json"), record.to_string()).unwrap();
    let error = archive_retained_rework_with(
        &fx.main,
        "BUG-2688",
        PR,
        "feat",
        &fixture_heads(&fx),
        || Ok(()),
        |_| Ok(()),
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("recorded worker"));
    assert!(merging(&retained));
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-452.acc34a6d | ai:codex
#[test]
fn bug_2688_fresh_retries_removed_unregistered_reserved_seat() {
    if aida_testkit::test_env::run_path_isolated_test_in_scratch() {
        return;
    }
    let fx = fixture();
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &fx.main,
        "implementer",
        &[],
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    conflicting_pr(&fx);
    let old = seat(&fx, "removed");
    admitted_prepare(&fx, &old, "old").unwrap();
    remove_seat(&fx, &old);
    let route = launch(HeadlessVendor::Codex, true);
    let heads = fixture_heads(&fx);
    crate::queue_cmd::inspect_queue_work_branch(&fx.main, Some("feat"), None, false, false, true)
        .unwrap();
    let archived = route.preserve_for_fresh_pickup(&fx.main, &heads).unwrap();
    assert!(archived.is_empty());
    assert_eq!(journal(&fx.main)[0].1, SpawnOutcome::ProvenDead);
    let scope = "TASK-926881";
    let receipt = pinned_setup(scope, &heads.head, false);
    let (lane, replacement) = lease_tree(&fx.main, scope);
    let (result, rolled_back) = run_pickup_launch(&fx.main, receipt, |_| {
        let paths = prepare_rework_merge_fixture_for_seat(
            &fx.main,
            &replacement,
            "feat",
            ReworkSeat::Fresh,
            PR,
            &lane,
            &|| admit_rework_conflict_launch(route),
        )?;
        assert_eq!(paths, ["README.md"]);
        Ok(())
    });
    result.unwrap();
    assert!(!rolled_back);
    assert!(merging(&replacement));
    assert_eq!(git_ok(&replacement, &["rev-parse", "HEAD"]), heads.head);
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-452.acc34a6d | ai:codex
#[test]
fn bug_2688_reused_path_refuses_foreign_registration() {
    let fx = fixture();
    conflicting_pr(&fx);
    let old = seat(&fx, "retained");
    std::fs::remove_dir_all(&old).unwrap();
    let other = fixture();
    conflicting_pr(&other);
    git_ok(
        &other.main,
        &["worktree", "add", old.to_str().unwrap(), "feat"],
    );
    admitted_prepare(&other, &old, "other").unwrap();
    let error = launch(HeadlessVendor::Codex, true)
        .preserve_for_fresh_pickup(&fx.main, &fixture_heads(&fx))
        .unwrap_err();
    assert!(format!("{error:#}").contains("does not match host registration"));
    assert!(merging(&old));
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-452.aca42931 | ai:codex
#[test]
fn bug_2688_malformed_merge_names_inspection_command() {
    let fx = fixture();
    conflicting_pr(&fx);
    let old = seat(&fx, "retained");
    let path = git_ok(
        &old,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "MERGE_HEAD",
        ],
    );
    std::fs::write(&path, "invalid").unwrap();
    let err = linked_rework_has_merge(&old).unwrap_err();
    assert!(format!("{err:#}").contains(&format!("od -c -- {}", crate::shell_quote(&path))));
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-452.aca42931 | ai:codex
#[test]
fn bug_2688_clean_archive_probe_does_not_admit_conflicting_preparation() {
    if aida_testkit::test_env::run_path_isolated_test_in_scratch() {
        return;
    }
    let fx = fixture();
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &fx.main,
        "implementer",
        &[],
        &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
    );
    conflicting_pr(&fx);
    let old = seat(&fx, "retained");
    admitted_prepare(&fx, &old, "old").unwrap();
    let attr = fx.main.join(".git/info/attributes");
    std::fs::write(&attr, "README.md merge=ours\n").unwrap();
    git_ok(&fx.main, &["config", "merge.ours.driver", "true"]);
    let route = launch(HeadlessVendor::Claude, true);
    let heads = fixture_heads(&fx);
    let archive = route.preserve_for_fresh_pickup(&fx.main, &heads).unwrap();
    assert_eq!(archive.len(), 1);
    // Model the preparation configuration without the host-only attributes.
    // Both Git probes still operate on the same fetched commit pair.
    std::fs::remove_file(attr).unwrap();
    let replacement = seat(&fx, "replacement");
    let err = apply_rework_conflict_route(
        route,
        &mut String::new(),
        &archive,
        |admit| {
            prepare_rework_merge_fixture_for_seat(
                &fx.main,
                &replacement,
                "feat",
                ReworkSeat::Fresh,
                PR,
                "new",
                admit,
            )
        },
        admit_rework_conflict_launch,
        |_, _| panic!("unexpected notification"),
    )
    .unwrap_err();
    assert!(
        err.downcast_ref::<crate::repair_journal::UnvalidatedRepairPublication>()
            .is_some(),
        "{err:#}"
    );
    assert!(!merging(&replacement));
    assert_eq!(journal(&fx.main)[0].1, SpawnOutcome::ProvenDead);
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-452.aca42931 | ai:codex
#[test]
fn bug_2688_ignored_collision_refuses_before_advance_or_merge() {
    if aida_testkit::test_env::run_path_isolated_test_in_scratch() {
        return;
    }
    for (advance, conflict) in [(true, false), (false, false), (false, true)] {
        let fx = fixture();
        let seed = git_ok(&fx.main, &["rev-parse", "HEAD"]);
        if conflict {
            conflicting_pr(&fx);
        } else {
            git_ok(&fx.main, &["checkout", "-qb", "feat"]);
            git_ok(&fx.main, &["commit", "--allow-empty", "-qm", "feature"]);
            if !advance {
                git_ok(&fx.main, &["checkout", "-q", "main"]);
            }
        }
        std::fs::write(fx.main.join("incoming"), "tracked").unwrap();
        git_ok(&fx.main, &["add", "incoming"]);
        git_ok(&fx.main, &["commit", "-qm", "incoming"]);
        git_ok(&fx.main, &["push", "-q", "origin", "feat", "main"]);
        git_ok(&fx.main, &["checkout", "-q", "main"]);
        let heads = fixture_heads(&fx);
        let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            &fx.main,
            "implementer",
            &[],
            &[("AIDA_TRUST_PID_NAMESPACE", Some("1"))],
        );
        let scope = "TASK-926881";
        let receipt = pinned_setup(scope, &heads.head, false);
        let (lane, tree) = lease_tree(&fx.main, scope);
        if advance {
            git_ok(&tree, &["reset", "--hard", &seed]);
        }
        let lease = crate::lease_path(&fx.main, &lane);
        let lease_bytes = std::fs::read(&lease).unwrap();
        let exclude = git_ok(
            &tree,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "info/exclude",
            ],
        );
        let existing_excludes = std::fs::read_to_string(&exclude).unwrap();
        std::fs::write(exclude, format!("{existing_excludes}\nincoming\n")).unwrap();
        std::fs::write(tree.join("incoming"), "private bytes").unwrap();
        assert_eq!(git_ok(&tree, &["status", "--porcelain"]), "");
        let (result, rolled_back) = run_pickup_launch(&fx.main, receipt, |_| {
            prepare_rework_merge_fixture_for_seat(
                &fx.main,
                &tree,
                "feat",
                ReworkSeat::Fresh,
                PR,
                &lane,
                &|| Ok(()),
            )
            .map(|_| ())
        });
        let err = result.unwrap_err();
        assert!(
            err.downcast_ref::<PreserveReworkSeat>().is_some(),
            "{err:#}"
        );
        assert!(
            format!("{err:#}").contains("ignored path \"incoming\""),
            "{err:#}"
        );
        assert!(journal(&fx.main).is_empty());
        assert!(!merging(&tree));
        assert!(!rolled_back);
        assert_eq!(std::fs::read(lease).unwrap(), lease_bytes);
        assert_eq!(
            std::fs::read(tree.join("incoming")).unwrap(),
            b"private bytes"
        );
    }
}
