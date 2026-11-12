//! Next-head reviews must pass the real queue pickup a second time.
// trace:BUG-2457 | ai:codex
use super::tests::fixture;
use aida_core::db::DatabaseBackend as _;

// Keep the maintenance guard visible to the fixture ratchet in this file.
// trace:BUG-2457 | ai:codex
fn git_ok(repo: &std::path::Path, args: &[&str]) -> String {
    let output = crate::test_git::git_command()
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn queue_work(storage: &crate::Storage) -> anyhow::Result<()> {
    queue_work_launch(storage, true)
}

fn queue_work_launch(storage: &crate::Storage, no_launch: bool) -> anyhow::Result<()> {
    crate::queue_cmd::handle_queue_work(
        storage,
        None,
        crate::test_env::HERMETIC_TEST_USER,
        Some(if no_launch { "PR-2457" } else { "STORY-2457" }),
        None,
        false,
        /* no_launch */ no_launch,
        crate::queue_cmd::QueueWorkRoleInput::direct(Some("reviewer")),
        /* no_pull */ true,
        None,
        None,
        None,
        false,
        None,
        false,
        false,
        false,
        /* force_claim */ false,
        None,
        false,
        false,
        None,
        "claude",
        false,
        crate::phase_driver::AutonomyMode::Default,
        None,
        false,
        false,
        false,
        (!no_launch).then_some(crate::complexity_calibration::ComplexityLevel::High),
        None,
        (!no_launch).then_some(crate::effort_calibration::EffortBucket::OneDay),
        false,
        false,
        false,
        false,
        None,
    )
}

fn review_story(main: &std::path::Path) -> (crate::Storage, impl Fn(aida_core::RequirementStatus)) {
    // Keep the trusted fixture setting private even under a shared-write
    // runner umask; the allocation reader correctly rejects mode 0664.
    // trace:BUG-2498 | ai:codex
    use std::os::unix::fs::PermissionsExt;
    let store_root = main.join(".aida-store");
    crate::test_env::home_fixture_backend(&store_root).unwrap();
    let common = aida_core::allocation_authority::common_dir(&store_root).unwrap();
    std::fs::set_permissions(
        common.join(aida_core::allocation_authority::CONFIG_FILE),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let (storage, status) = super::orphan_reentry_tests::queued_scope(main, "STORY-2457");
    std::fs::write(
        main.join(".aida-store/registry/team.toml"),
        format!(
            "[members]\n\"{}\" = [\"implementer\", \"reviewer\"]\n",
            crate::test_env::HERMETIC_TEST_USER
        ),
    )
    .unwrap();
    let backend = crate::test_env::home_fixture_backend(&main.join(".aida-store")).unwrap();
    let mut store = backend.load().unwrap();
    store
        .requirements
        .iter_mut()
        .find(|r| r.spec_id.as_deref() == Some("STORY-2457"))
        .unwrap()
        .title = "Review PR-2457: next head".into();
    backend.save(&store).unwrap();
    let entry = storage
        .queue_list(crate::test_env::HERMETIC_TEST_USER, false)
        .unwrap()
        .remove(0);
    storage
        .queue_remove(&entry.user_id, &entry.requirement_id)
        .unwrap();
    storage
        .queue_add(aida_core::QueueEntry {
            for_role: Some("reviewer".into()),
            ..entry
        })
        .unwrap();
    (storage, status)
}

#[test]
fn bug_2457_two_heads_through_queue_pickup_preserve_holds_and_refuse_unsafe_trees() {
    let fx = fixture();
    let mut ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "reviewer", &[]);
    // Every holder of these fresh private fixtures is in this test's PID
    // namespace. Keep production's automatic-removal guard fail-closed.
    ambient.set_key("AIDA_TRUST_PID_NAMESPACE", "1");
    let gh = fx.root.join("fake-gh");
    crate::test_exec::write_executable(&gh, "#!/bin/sh\nexit 1\n");
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    git_ok(&fx.main, &["init", "-q", ".aida-store"]);
    git_ok(
        &fx.main.join(".aida-store"),
        &["config", "user.email", "test@example.com"],
    );
    git_ok(
        &fx.main.join(".aida-store"),
        &["config", "user.name", "AIDA Test"],
    );
    let (storage, status) = review_story(&fx.main);
    git_ok(
        &fx.main,
        &["push", "-q", "origin", "HEAD:refs/pull/2457/head"],
    );
    queue_work(&storage).unwrap();
    let lease = crate::list_leases(&fx.main).remove(0);
    let tree = lease.worktree_path.clone();
    let first_head = git_ok(&tree, &["rev-parse", "HEAD"]);

    // An outstanding lease retains the tree, even if the checkout is clean.
    status(aida_core::RequirementStatus::Approved);
    let err = queue_work(&storage).unwrap_err().to_string();
    assert!(err.contains("held by worktree"), "{err}");
    std::fs::remove_file(crate::lease_path(&fx.main, &lease.id)).unwrap();

    std::fs::write(tree.join("review-notes"), "keep me").unwrap();
    let err = queue_work(&storage).unwrap_err().to_string();
    assert!(err.contains("held by worktree"), "{err}");
    assert_eq!(
        std::fs::read_to_string(tree.join("review-notes")).unwrap(),
        "keep me"
    );
    std::fs::remove_file(tree.join("review-notes")).unwrap();

    let mut holder = std::process::Command::new("sleep")
        .arg("30")
        .current_dir(&tree)
        .spawn()
        .unwrap();
    let result = queue_work(&storage);
    holder.kill().unwrap();
    holder.wait().unwrap();
    assert!(result.unwrap_err().to_string().contains("held by worktree"));
    git_ok(&fx.main, &["worktree", "lock", tree.to_str().unwrap()]);
    assert!(queue_work(&storage)
        .unwrap_err()
        .to_string()
        .contains("held by worktree"));
    git_ok(&fx.main, &["worktree", "unlock", tree.to_str().unwrap()]);

    // A legacy hold must reach the primary ledger before the old tree goes.
    let markers = tree.join(".aida/merge-holds");
    std::fs::create_dir_all(&markers).unwrap();
    std::fs::write(markers.join("PR-9999"), "unrelated hold\n").unwrap();
    assert!(queue_work(&storage)
        .unwrap_err()
        .to_string()
        .contains("held by worktree"));
    assert!(markers.join("PR-9999").exists());
    std::fs::remove_file(markers.join("PR-9999")).unwrap();
    std::fs::write(markers.join("PR-2457"), "needs human review\n").unwrap();
    std::fs::write(fx.main.join("README.md"), "second head\n").unwrap();
    git_ok(&fx.main, &["commit", "-qam", "second head"]);
    git_ok(
        &fx.main,
        &["push", "-q", "origin", "HEAD:refs/pull/2457/head"],
    );
    let second_head = git_ok(&fx.main, &["rev-parse", "HEAD"]);
    assert_ne!(first_head, second_head);
    assert!(
        aida_core::worktree_holders::check_worktree_removal(&tree).is_ok(),
        "unexpected holder: {:?}",
        aida_core::worktree_holders::check_worktree_removal(&tree)
    );
    queue_work(&storage).unwrap();
    let lease = crate::list_leases(&fx.main).remove(0);
    assert_eq!(lease.worktree_path, tree);
    assert_eq!(git_ok(&tree, &["rev-parse", "HEAD"]), second_head);
    assert!(crate::gate::merge_hold_core::read_hold(&fx.main, 2457).is_some());
    assert!(!markers.join("PR-2457").exists());
}

// trace:BUG-2469 | ai:codex
// trace:BUG-2469.acec8dd7 | ai:codex
// trace:BUG-2469.ac5829b2 | ai:codex
// trace:BUG-2469.ac77bc7f | ai:codex
// Canonical spellings also serve criteria reports after ID promotion.
// trace:BUG-b6b1278890774ef993d2ebc327862470-235.acec8dd7 | ai:codex
// trace:BUG-b6b1278890774ef993d2ebc327862470-235.ac5829b2 | ai:codex
// trace:BUG-b6b1278890774ef993d2ebc327862470-235.ac77bc7f | ai:codex
// trace:TASK-b6b1278890774ef993d2ebc327862470-900 | ai:codex
#[test]
fn bug_2469_dead_review_reclaimed_only_when_new_head_covers_recorded_and_actual_heads() {
    let fx = fixture();
    let mut ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "reviewer", &[]);
    ambient.set_key("AIDA_TRUST_PID_NAMESPACE", "1");
    let gh = fx.root.join("fake-gh");
    crate::test_exec::write_executable(&gh, "#!/bin/sh\nexit 1\n");
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    git_ok(&fx.main, &["init", "-q", ".aida-store"]);
    git_ok(
        &fx.main.join(".aida-store"),
        &["config", "user.email", "test@example.com"],
    );
    git_ok(
        &fx.main.join(".aida-store"),
        &["config", "user.name", "AIDA Test"],
    );
    let (storage, status) = review_story(&fx.main);
    // The reviewed commits exist only under the forge ref, never refs/remotes.
    std::fs::write(fx.main.join("README.md"), "first PR head\n").unwrap();
    git_ok(&fx.main, &["commit", "-qam", "first PR head"]);
    git_ok(
        &fx.main,
        &["push", "-q", "origin", "HEAD:refs/pull/2457/head"],
    );
    queue_work(&storage).unwrap();
    let mut lease = crate::list_leases(&fx.main).remove(0);
    let tree = lease.worktree_path.clone();
    let first_head = git_ok(&tree, &["rev-parse", "HEAD"]);
    lease.pr_head_sha = Some(first_head.clone());
    lease.active_pid = Some(u32::MAX);
    lease.creator_pid = Some(u32::MAX);
    lease.active_pid_start_time = None;
    lease.creator_pid_start_time = None;
    lease.pid_namespace = crate::agent_registry::current_pid_namespace();
    let lease_file = crate::lease_path(&fx.main, &lease.id);
    // Reclaim reads ownership, not the legacy PID fields. Keep the fixture's
    // authoritative review record in the same state as each simulated owner.
    // trace:STORY-2325 | ai:codex
    let save = |l: &crate::SessionLease| {
        use crate::ownership::{Ownership, Phase, Witness, WitnessKind, WorkKey};
        let at = chrono::Utc::now()
            - chrono::Duration::days(if l.active_pid == Some(u32::MAX) { 2 } else { 0 });
        let key = WorkKey::Review {
            change: crate::ownership::dual::change(2457),
            head: first_head.clone(),
        };
        let mut holder = crate::ownership::dual::holder_for_lease(
            l,
            &crate::ownership::dual::WriterFacts::here(),
            at,
        );
        holder.witnesses = l
            .active_pid
            .map(|_| Witness {
                admission: None,
                kind: WitnessKind::Heartbeat { at, every_secs: 10 },
                recorded_at: at,
            })
            .into_iter()
            .collect();
        crate::ownership::store::OwnershipStore::for_project(&fx.main)
            .unwrap()
            .unwrap()
            .update(
                &key,
                "fixture",
                crate::ownership::store::Expect::Any,
                |_| {
                    Some((
                        Ownership {
                            key: key.clone(),
                            generation: 0,
                            phase: Phase::Reviewing {
                                head: first_head.clone(),
                            },
                            holder: Some(holder),
                        },
                        None,
                    ))
                },
            )
            .unwrap();
        std::fs::write(&lease_file, toml::to_string(l).unwrap()).unwrap();
    };
    let assert_retained_refusal = |reason: &str| {
        let before = std::fs::read(&lease_file).unwrap();
        let head = git_ok(&tree, &["rev-parse", "HEAD"]);
        let err = queue_work(&storage).unwrap_err().to_string();
        assert!(err.contains("held by worktree"), "{err}");
        assert!(err.contains("was not reclaimed"), "{err}");
        assert!(err.contains(reason), "{err}");
        assert_eq!(std::fs::read(&lease_file).unwrap(), before);
        assert_eq!(git_ok(&tree, &["rev-parse", "HEAD"]), head);
    };
    std::fs::write(&lease_file, toml::to_string(&lease).unwrap()).unwrap();
    assert!(lease.review_verb && !lease.claim_verb);
    assert!(
        crate::seat_clone::remote_missing_commits(&fx.main, &tree, false, &lease.branch).unwrap()
            > 0
    );
    let fetch_head = std::fs::read(fx.main.join(".git/FETCH_HEAD")).unwrap();
    crate::session_end::guard_remote_commits(&fx.main, &lease, false).unwrap();
    assert_eq!(
        std::fs::read(fx.main.join(".git/FETCH_HEAD")).unwrap(),
        fetch_head
    );
    assert_eq!(git_ok(&tree, &["rev-parse", "HEAD"]), first_head);
    // No admitted ownership record: branch takeover retains the seat even
    // though the independent remote-coverage guard can prove its commits safe.
    // trace:STORY-2325 | ai:codex
    assert!(matches!(
        crate::pr_lease_takeover::check_pr_lease(&fx.main, &lease, "missing-source-branch"),
        crate::pr_lease_takeover::PrLeaseCheck::NotProvable(_)
    ));
    save(&lease);
    assert!(matches!(
        crate::ownership::reader::lease_owner(&fx.main, &lease),
        crate::ownership::OwnerVerdict::Dead { .. }
    ));
    let mut implementer = lease.clone();
    implementer.review_verb = false;
    assert!(crate::session_end::guard_remote_commits(&fx.main, &implementer, false).is_err());
    implementer.review_verb = true;
    implementer.claim_verb = true;
    assert!(crate::session_end::guard_remote_commits(&fx.main, &implementer, false).is_err());

    std::fs::write(fx.main.join("README.md"), "next PR head\n").unwrap();
    git_ok(&fx.main, &["commit", "-qam", "next PR head"]);
    git_ok(
        &fx.main,
        &["push", "-q", "origin", "HEAD:refs/pull/2457/head"],
    );
    let next_head = git_ok(&fx.main, &["rev-parse", "HEAD"]);
    status(aida_core::RequirementStatus::Approved);

    // Live and unknown owners keep the tree and lease.
    lease.active_pid = Some(std::process::id());
    save(&lease);
    assert!(matches!(
        crate::ownership::reader::lease_owner(&fx.main, &lease),
        crate::ownership::OwnerVerdict::Alive { .. }
    ));
    assert!(queue_work(&storage).is_err());
    lease.active_pid = None;
    lease.creator_pid = None;
    lease.holder_record = None;
    save(&lease);
    assert!(matches!(
        crate::ownership::reader::lease_owner(&fx.main, &lease),
        crate::ownership::OwnerVerdict::Unknown { .. }
    ));
    assert!(queue_work(&storage).is_err());
    lease.active_pid = Some(u32::MAX);
    lease.creator_pid = Some(u32::MAX);
    save(&lease);

    std::fs::write(tree.join("pending"), "keep me").unwrap();
    assert!(queue_work(&storage).is_err());
    assert!(lease_file.exists());
    std::fs::remove_file(tree.join("pending")).unwrap();
    git_ok(&fx.main, &["worktree", "lock", tree.to_str().unwrap()]);
    assert!(queue_work(&storage).is_err());
    git_ok(&fx.main, &["worktree", "unlock", tree.to_str().unwrap()]);
    let mut holder = std::process::Command::new("sleep")
        .arg("30")
        .current_dir(&tree)
        .spawn()
        .unwrap();
    let result = queue_work(&storage);
    holder.kill().unwrap();
    holder.wait().unwrap();
    assert!(result.is_err());

    // Force-pushed unrelated heads, missing refs, missing metadata, and
    // review-local commits cannot authorize automatic retirement.
    git_ok(
        &fx.main,
        &[
            "push",
            "-q",
            "--force",
            "origin",
            "origin/main:refs/pull/2457/head",
        ],
    );
    assert_retained_refusal("recorded review head is not covered");
    assert!(crate::session_end::guard_remote_commits(&fx.main, &lease, false).is_err());
    git_ok(&fx.main, &["push", "-q", "origin", ":refs/pull/2457/head"]);
    assert_retained_refusal("cannot fetch review head");
    git_ok(
        &fx.main,
        &["push", "-q", "origin", "HEAD:refs/pull/2457/head"],
    );
    lease.pr_head_sha = None;
    save(&lease);
    // Without a head the review key cannot be resolved: retain as Unknown
    // before attempting the dead-review coverage checks.
    let before = std::fs::read(&lease_file).unwrap();
    assert!(matches!(
        crate::ownership::reader::lease_owner(&fx.main, &lease),
        crate::ownership::OwnerVerdict::Unknown { .. }
    ));
    assert!(queue_work(&storage)
        .unwrap_err()
        .to_string()
        .contains("held by worktree"));
    assert_eq!(std::fs::read(&lease_file).unwrap(), before);
    lease.pr_head_sha = Some(first_head.clone());
    save(&lease);
    std::fs::write(tree.join("local"), "unpublished review fix").unwrap();
    git_ok(&tree, &["add", "local"]);
    git_ok(&tree, &["commit", "-qm", "review local commit"]);
    assert_retained_refusal("review checkout has unpublished commits");
    assert!(crate::session_end::guard_remote_commits(&fx.main, &lease, false).is_err());
    git_ok(
        &tree,
        &["reset", "--hard", lease.pr_head_sha.as_deref().unwrap()],
    );
    assert!(lease_file.exists());
    lease.claim_verb = true;
    save(&lease);
    assert!(queue_work(&storage).is_err());
    lease.claim_verb = false;
    save(&lease);
    let mut duplicate = lease.clone();
    duplicate.id = "duplicate-review".into();
    let duplicate_file = crate::lease_path(&fx.main, &duplicate.id);
    std::fs::write(&duplicate_file, toml::to_string(&duplicate).unwrap()).unwrap();
    assert!(queue_work(&storage).is_err());
    assert!(lease_file.exists() && duplicate_file.exists());
    std::fs::remove_file(duplicate_file).unwrap();
    // Production's stalled state: Reviewing, exact session, no witnesses.
    // Neither an empty witness list nor an unrelated Closed record proves exit.
    // trace:BUG-2498 | ai:codex
    crate::ownership::dual::enable_for_test();
    crate::ownership::dual::lease_released(&fx.main, &lease.id, "review completed");
    assert!(matches!(
        crate::ownership::reader::lease_owner(&fx.main, &lease),
        crate::ownership::OwnerVerdict::Unknown { .. }
    ));
    assert!(!crate::ownership::reader::review_lease_ended(
        &fx.main, &lease
    ));
    assert!(crate::ownership::reader::journal_reclaim(&fx.main, &lease).is_err());
    lease.active_pid = None;
    lease.creator_pid = None;
    save(&lease);
    let record = crate::ownership::reader::lease_record(&fx.main, &lease)
        .unwrap()
        .unwrap();
    assert!(matches!(
        record.phase,
        crate::ownership::Phase::Reviewing { .. }
    ));
    assert!(record.holder.unwrap().witnesses.is_empty());
    assert!(!crate::ownership::reader::review_lease_ended(
        &fx.main, &lease
    ));
    // Observed production state: clean, pushed, witness-less, no verdict.
    // The real supervisor exit path must still record completion.
    // trace:BUG-2498 | ai:codex
    crate::queue_cmd::end_finished_review(&fx.main, "PR-2457", &tree, false);
    assert!(crate::ownership::reader::review_lease_ended(
        &fx.main, &lease
    ));
    // A mail-retention refusal may stamp the lease after completion; it must
    // preserve the binding while retaining all worker identity checks.
    let mut retained = lease.clone();
    retained.ending_retained_at = Some(chrono::Utc::now());
    retained.ending_retained_reason = Some("unread mail".into());
    std::fs::write(&lease_file, toml::to_string(&retained).unwrap()).unwrap();
    assert!(crate::ownership::reader::review_lease_ended(
        &fx.main, &retained
    ));
    save(&lease);
    // Contained writers refuse before touching host ownership evidence.
    // trace:TASK-3777 | ai:codex
    let before = crate::ownership::reader::lease_record(&fx.main, &lease)
        .unwrap()
        .unwrap();
    ambient.set_key("AIDA_CONTAINED", "1");
    assert!(crate::ownership::reader::review_finished(&fx.main, &lease)
        .unwrap_err()
        .to_string()
        .contains("contained seat"));
    ambient.unset_key("AIDA_CONTAINED");
    let after = crate::ownership::reader::lease_record(&fx.main, &lease)
        .unwrap()
        .unwrap();
    assert_eq!(after.generation, before.generation);
    std::fs::write(tree.join("pending"), "keep completed review notes").unwrap();
    let ended = crate::queue_cmd::end_finished_review(&fx.main, "PR-2457", &tree, true);
    assert!(ended[0].1.is_err());
    assert!(crate::ownership::reader::review_lease_ended(
        &fx.main, &lease
    ));
    assert!(queue_work(&storage).is_err());
    assert!(lease_file.exists());
    std::fs::remove_file(tree.join("pending")).unwrap();
    let mut holder = std::process::Command::new("sleep")
        .arg("30")
        .current_dir(&tree)
        .spawn()
        .unwrap();
    let result = queue_work(&storage);
    holder.kill().unwrap();
    holder.wait().unwrap();
    assert!(result.is_err());
    std::fs::write(tree.join("local"), "unpublished completed review fix").unwrap();
    git_ok(&tree, &["add", "local"]);
    git_ok(&tree, &["commit", "-qm", "completed review local commit"]);
    assert_retained_refusal("review checkout has unpublished commits");
    git_ok(&tree, &["reset", "--hard", &first_head]);
    let markers = tree.join(".aida/merge-holds");
    std::fs::create_dir_all(&markers).unwrap();
    std::fs::write(markers.join("PR-2457"), "review crashed with hold\n").unwrap();
    save(&lease); // the watcher sees the original witness-less Reviewing state
    std::fs::write(tree.join("pending"), "retain until supervisor finishes").unwrap();
    crate::review_watch::bug2498_review_worktree_release_allows_one_watch_setup_retry(
        &next_head,
        || queue_work(&storage),
        || {
            // Use the real supervisor completion path, not dual::lease_released.
            let ended = crate::queue_cmd::end_finished_review(&fx.main, "PR-2457", &tree, true);
            assert!(ended[0].1.is_err()); // dirty work is still retained
            assert!(lease_file.exists());
            assert!(crate::ownership::reader::review_lease_ended(
                &fx.main, &lease
            ));
            let mut replacement = lease.clone();
            replacement.id = "later-same-head-review".into();
            assert!(!crate::ownership::reader::review_lease_ended(
                &fx.main,
                &replacement
            ));
            replacement = lease.clone();
            replacement.active_pid = Some(std::process::id());
            assert!(!crate::ownership::reader::review_lease_ended(
                &fx.main,
                &replacement
            ));
            // Legacy recorded workers need a positive dead identity; a PID
            // without its start identity cannot borrow completion authority.
            // trace:TASK-3777 | ai:codex
            let mut child = std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .unwrap();
            let pid = child.id();
            let start = crate::process_probe::process_start_identity(pid).unwrap();
            child.kill().unwrap();
            child.wait().unwrap();
            replacement = lease.clone();
            replacement.holder_record = None;
            replacement.active_pid = Some(pid);
            replacement.active_pid_start_time = Some(start);
            save(&replacement);
            crate::ownership::reader::review_finished(&fx.main, &replacement).unwrap();
            assert!(crate::ownership::reader::review_lease_ended(
                &fx.main,
                &replacement
            ));
            replacement.active_pid = Some(std::process::id());
            replacement.active_pid_start_time = None;
            save(&replacement);
            crate::ownership::reader::review_finished(&fx.main, &replacement).unwrap();
            assert!(!crate::ownership::reader::review_lease_ended(
                &fx.main,
                &replacement
            ));
            replacement = lease.clone();
            replacement.active_pid = Some(std::process::id());
            // Even an otherwise matching receipt cannot override a recorded
            // live worker outside the review cwd.
            replacement.active_pid_start_time =
                crate::process_probe::process_start_identity(std::process::id());
            save(&replacement);
            crate::ownership::reader::review_finished(&fx.main, &replacement).unwrap();
            assert!(!crate::ownership::reader::review_lease_ended(
                &fx.main,
                &replacement
            ));
            save(&lease);
            // Superseding killed the supervisor before verdict collection.
            // Its watcher proves all recorded processes gone and closes the
            // captured lease before the failed next-head pickup retries.
            // trace:BUG-2498 | ai:codex
            crate::review_watch::bug2498_confirm_superseded(&fx.main, &lease);
            std::fs::remove_file(tree.join("pending")).unwrap();
        },
    );
    assert!(crate::gate::merge_hold_core::read_hold(&fx.main, 2457).is_some());
    assert!(!lease_file.exists());
    let new_lease = crate::list_leases(&fx.main).remove(0);
    assert_ne!(new_lease.id, lease.id);
    assert_eq!(new_lease.worktree_path, tree);
    assert_eq!(git_ok(&tree, &["rev-parse", "HEAD"]), next_head);
    assert!(git_ok(&fx.main, &["for-each-ref", "refs/aida/review-coverage"]).is_empty());
}

// trace:BUG-2530 | ai:codex
#[test]
fn bug2530_concluded_pickup_preserves_checkout_queue_and_calibration() {
    let fx = fixture();
    let mut ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "reviewer", &[]);
    ambient.set_key("AIDA_TRUST_PID_NAMESPACE", "1");
    let gh = fx.root.join("fake-gh");
    crate::test_exec::write_executable(&gh, "#!/bin/sh\nexit 1\n");
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    git_ok(&fx.main, &["init", "-q", ".aida-store"]);
    let (storage, status) = review_story(&fx.main);
    git_ok(
        &fx.main,
        &["push", "-q", "origin", "HEAD:refs/pull/2457/head"],
    );
    queue_work(&storage).unwrap();
    let lease = crate::list_leases(&fx.main).remove(0);
    let tree = lease.worktree_path;
    std::fs::remove_file(crate::lease_path(&fx.main, &lease.id)).unwrap();
    status(aida_core::RequirementStatus::Approved);
    let user = crate::test_env::HERMETIC_TEST_USER;
    for row in storage.queue_list(user, false).unwrap() {
        storage.queue_remove(user, &row.requirement_id).unwrap();
    }
    let before = serde_json::to_value(storage.load().unwrap()).unwrap();
    let sha = git_ok(&tree, &["rev-parse", "HEAD"]);
    ambient.set_key("AIDA_TEST_PR_HEAD_SHA", &sha);
    let grant = crate::gate::seat_authority::test_support::mint_grant_for(
        &fx.main,
        user,
        "advisor",
        &["reviewer"],
    );
    ambient.set_key(crate::gate::seat_authority::GRANT_ENV, grant);
    let path = crate::gate::review_verdict::verdict_path(&fx.main, "PR-2457");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        serde_json::json!({"verdict":"request-changes", "reviewed_sha":sha}).to_string(),
    )
    .unwrap();
    let calibration = fx.main.join(".aida/complexity-calibration");
    let effort = fx.main.join(".aida/effort-calibration");
    // Remove setup's calibration so any attempted pickup write is visible.
    for dir in [&calibration, &effort] {
        if dir.exists() {
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
    let error = queue_work_launch(&storage, false).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("duplicate review is unnecessary"),
        "{error:#}"
    );
    assert!(tree.join(".git").exists());
    assert_eq!(git_ok(&tree, &["rev-parse", "HEAD"]), sha);
    assert!(storage.queue_list(user, false).unwrap().is_empty());
    assert_eq!(
        serde_json::to_value(storage.load().unwrap()).unwrap(),
        before
    );
    assert!(!calibration.exists() && !effort.exists());
}
