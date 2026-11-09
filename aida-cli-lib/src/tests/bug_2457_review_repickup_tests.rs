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
    crate::queue_cmd::handle_queue_work(
        storage,
        None,
        crate::test_env::HERMETIC_TEST_USER,
        Some("PR-2457"),
        None,
        false,
        /* no_launch */ true,
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
        None,
        None,
        None,
        false,
        false,
        false,
        false,
        None,
    )
}

fn review_story(main: &std::path::Path) -> (crate::Storage, impl Fn(aida_core::RequirementStatus)) {
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
    let markers = tree.join(".aida/merge-holds");
    std::fs::create_dir_all(&markers).unwrap();
    std::fs::write(markers.join("PR-2457"), "review crashed with hold\n").unwrap();
    queue_work(&storage).unwrap();
    assert!(crate::gate::merge_hold_core::read_hold(&fx.main, 2457).is_some());
    assert!(!lease_file.exists());
    let new_lease = crate::list_leases(&fx.main).remove(0);
    assert_ne!(new_lease.id, lease.id);
    assert_eq!(new_lease.worktree_path, tree);
    assert_eq!(git_ok(&tree, &["rev-parse", "HEAD"]), next_head);
    assert!(git_ok(&fx.main, &["for-each-ref", "refs/aida/review-coverage"]).is_empty());
}
