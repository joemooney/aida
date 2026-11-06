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
