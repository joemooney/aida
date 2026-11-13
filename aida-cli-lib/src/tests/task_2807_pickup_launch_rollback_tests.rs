//! TASK-2807 (with TASK-2808 and TASK-2811): a `queue work` pickup's launch
//! runs under `run_pickup_launch` holding the real setup that
//! `session_start_with_receipt` created. A launch that fails before an agent
//! can start (a refused child-grant mint, any other pre-agent error) leaves
//! neither the worktree nor the lease; once an agent can have run, neither an
//! error nor a panic removes them.
// trace:TASK-2807 | ai:claude

use super::tests::fixture;
use crate::queue_cmd::{run_pickup_launch, AgentLaunchMark};
use std::path::{Path, PathBuf};

/// The setup a launching pickup creates for `scope`: worktree, branch,
/// lease and claim, with its rollback armed.
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
    )
    .unwrap();
    assert!(receipt.owns_setup(), "a launching pickup owns its setup");
    receipt
}

fn lease_tree(main: &Path, scope: &str) -> Option<(String, PathBuf)> {
    crate::list_leases(main)
        .into_iter()
        .find(|l| l.scope == scope)
        .map(|l| (l.id, l.worktree_path))
}

fn assert_gone(main: &Path, tree: &Path, scope: &str) {
    assert!(lease_tree(main, scope).is_none(), "the lease is removed");
    assert!(!tree.exists(), "the worktree is removed");
}

fn assert_kept(main: &Path, tree: &Path, scope: &str, lease_id: &str) {
    assert_eq!(
        lease_tree(main, scope).map(|(id, _)| id).as_deref(),
        Some(lease_id),
        "the lease is kept"
    );
    assert!(
        tree.join("agent-work.txt").exists(),
        "the agent's work is kept"
    );
}

/// A refused child-grant mint inside the launch rolls the setup back.
#[test]
fn a_refused_mint_rolls_back_the_worktree_and_lease() {
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let scope = "TASK-928071";
    let receipt = launching_setup(scope);
    let (_, tree) = lease_tree(&fx.main, scope).expect("setup minted a lease");
    assert!(tree.exists());

    let (result, rolled_back) = run_pickup_launch(&fx.main, receipt, |_agent| {
        crate::grant_keyring::FORCE_INSTALL_ERRNO.with(|e| e.set(Some(libc::EDQUOT)));
        crate::gate::seat_authority::preflight_sandboxed_child_key("implementer")?;
        unreachable!("the mint is refused");
    });

    let err = result.unwrap_err();
    assert!(
        crate::gate::seat_authority::ChildGrantRefused::is_in(&err),
        "{err:#}"
    );
    assert!(rolled_back);
    assert_gone(&fx.main, &tree, scope);
}

/// TASK-2808: a pre-agent failure that is not a grant refusal (here, a
/// failed exec) rolls back too.
#[test]
fn any_pre_agent_failure_rolls_back_the_worktree_and_lease() {
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let scope = "TASK-928072";
    let receipt = launching_setup(scope);
    let (_, tree) = lease_tree(&fx.main, scope).expect("setup minted a lease");

    let (result, rolled_back) = run_pickup_launch(&fx.main, receipt, |_agent| {
        anyhow::bail!("failed to exec codex: No such file or directory")
    });

    assert!(result.is_err());
    assert!(rolled_back);
    assert_gone(&fx.main, &tree, scope);
}

/// The reviewer's codex arm: `reviewer_status` on a `codex` that cannot be
/// spawned. The failed spawn launched nobody, so the setup is rolled back.
fn spawn_reviewer(program: &Path) -> anyhow::Result<std::process::ExitStatus> {
    use anyhow::Context as _;
    crate::review_watch::reviewer_status(&mut std::process::Command::new(program))
        .context("failed to spawn codex")
}

/// TASK-2808: a failed codex spawn (no child process) rolls back.
#[test]
fn a_failed_codex_spawn_rolls_back_the_worktree_and_lease() {
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let scope = "TASK-928076";
    let receipt = launching_setup(scope);
    let (_, tree) = lease_tree(&fx.main, scope).expect("setup minted a lease");
    let missing = fx.main.join("no-such-bin").join("codex");

    let (result, rolled_back) = run_pickup_launch(&fx.main, receipt, |_agent| {
        spawn_reviewer(&missing)?;
        unreachable!("the spawn fails");
    });

    let err = result.unwrap_err();
    assert!(
        format!("{err:#}").contains("failed to spawn codex"),
        "{err:#}"
    );
    assert!(rolled_back);
    assert_gone(&fx.main, &tree, scope);
}

/// A reviewer that spawned and then failed (no verdict) can have run: kept.
#[test]
fn a_spawned_reviewer_that_fails_keeps_the_worktree_and_lease() {
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let scope = "TASK-928077";
    let receipt = launching_setup(scope);
    let (lease_id, tree) = lease_tree(&fx.main, scope).expect("setup minted a lease");

    let (result, rolled_back) = run_pickup_launch(&fx.main, receipt, |_agent| {
        spawn_reviewer(Path::new("true"))?;
        std::fs::write(tree.join("agent-work.txt"), "uncommitted\n").unwrap();
        anyhow::bail!("the reviewer exited without a verdict")
    });

    assert!(result.is_err());
    assert!(!rolled_back);
    assert_kept(&fx.main, &tree, scope, &lease_id);
}

/// An error after an agent can have run (an implementer that punted, a
/// reviewer with no verdict) keeps its tree and lease.
#[test]
fn a_failure_after_the_agent_started_keeps_the_worktree_and_lease() {
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let scope = "TASK-928073";
    let receipt = launching_setup(scope);
    let (lease_id, tree) = lease_tree(&fx.main, scope).expect("setup minted a lease");

    let (result, rolled_back) = run_pickup_launch(&fx.main, receipt, |agent: &AgentLaunchMark| {
        agent.starting();
        std::fs::write(tree.join("agent-work.txt"), "uncommitted\n").unwrap();
        anyhow::bail!("the implementer exited without opening a PR")
    });

    assert!(result.is_err());
    assert!(!rolled_back);
    assert_kept(&fx.main, &tree, scope, &lease_id);
}

/// TASK-2807: a panic after the agent ran unwinds through a disarmed setup,
/// so its uncommitted work and lease survive; the panic still propagates.
#[test]
fn a_panic_after_the_agent_started_keeps_the_worktree_and_lease() {
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let scope = "TASK-928074";
    let receipt = launching_setup(scope);
    let (lease_id, tree) = lease_tree(&fx.main, scope).expect("setup minted a lease");

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_pickup_launch(&fx.main, receipt, |agent| {
            agent.starting();
            std::fs::write(tree.join("agent-work.txt"), "uncommitted\n").unwrap();
            panic!("a bug after the agent ran");
        })
    }));

    assert!(panicked.is_err(), "the panic propagates");
    assert_kept(&fx.main, &tree, scope, &lease_id);
}

/// A panic before any agent could start still rolls the setup back.
#[test]
fn a_panic_before_the_agent_started_rolls_back() {
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let scope = "TASK-928075";
    let receipt = launching_setup(scope);
    let (_, tree) = lease_tree(&fx.main, scope).expect("setup minted a lease");

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_pickup_launch(&fx.main, receipt, |_agent| {
            panic!("a bug before any agent");
        })
    }));

    assert!(panicked.is_err(), "the panic propagates");
    assert_gone(&fx.main, &tree, scope);
}

// Production no--role session setup must publish the same effective role that
// the local lease records, even from a reviewer/advisor shell.
// trace:STORY-2705 | ai:codex
#[test]
fn session_start_default_role_claim_defers_cross_host_review_implementer_gate() {
    use super::tests::git_ok;
    for inherited in ["", "advisor", "reviewer"] {
        let fx = fixture();
        let mut ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
        ambient.set_key("AIDA_SESSION_ROLE", inherited);
        let store = fx.main.join(".aida-store");
        std::fs::create_dir_all(&store).unwrap();
        git_ok(&store, &["init", "-q", "-b", "aida-store"]);
        git_ok(&store, &["config", "user.email", "test@example.com"]);
        git_ok(&store, &["config", "user.name", "AIDA Test"]);
        git_ok(&store, &["add", "-A"]);
        let seed = crate::test_git::git_command()
            .current_dir(&store)
            .args(["commit", "--allow-empty", "-qm", "seed"])
            .output()
            .unwrap();
        assert!(
            seed.status.success(),
            "{}",
            String::from_utf8_lossy(&seed.stderr)
        );
        let origin = fx.root.join("store-origin.git");
        git_ok(
            &fx.root,
            &[
                "clone",
                "-q",
                "--bare",
                store.to_str().unwrap(),
                origin.to_str().unwrap(),
            ],
        );
        git_ok(
            &store,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        let scope = "STORY-928079";
        let _receipt = launching_setup(scope);
        let mut claim = crate::coordination::list_claims(&store)
            .into_iter()
            .find(|c| c.scope == scope)
            .expect("production session wrote claim");
        assert_eq!(claim.agent, "implementer");
        let lease = crate::list_leases(&fx.main)
            .into_iter()
            .find(|l| l.scope == scope)
            .unwrap();
        assert_eq!(lease.role.as_deref(), Some(claim.agent.as_str()));
        // Observe the actual claim from another host, without a local lease.
        let observer = fx.root.join("observer");
        claim.host = "remote-fixture-host".into();
        let path = crate::coordination::claim_path(&observer.join(".aida-store"), scope);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, toml::to_string(&claim).unwrap()).unwrap();
        assert!(
            crate::review_implementer_gate::deferred_with_metadata(&observer, || Ok((
                format!("fix (Refs {scope})"),
                "other-branch".into()
            )))
            .unwrap()
            .is_some()
        );
    }
}
