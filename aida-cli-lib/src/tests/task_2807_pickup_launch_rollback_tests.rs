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
        /* done_rework_admitted */ false,
    )
    .unwrap();
    assert!(receipt.owns_setup(), "a launching pickup owns its setup");
    receipt
}

/// A shared `.aida-store` for `fx.main` with a bare `origin`, so session
/// start writes and pushes a real shared claim. Returns (store, origin).
fn distributed_store(fx: &super::tests::Fixture) -> (PathBuf, PathBuf) {
    use super::tests::git_ok;
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
    (store, origin)
}

/// Whether `scope`'s shared claim stands in the local store.
fn local_claim(store: &Path, scope: &str) -> bool {
    crate::coordination::list_claims(store)
        .iter()
        .any(|claim| claim.scope == scope)
}

/// Whether `scope`'s shared claim stands on the store's `origin`.
fn origin_claim(store: &Path, origin: &Path, scope: &str) -> bool {
    let path = crate::coordination::claim_path(store, scope);
    let rel = path.strip_prefix(store).unwrap().to_str().unwrap();
    !super::tests::git_ok(origin, &["ls-tree", "--name-only", "aida-store", "--", rel]).is_empty()
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
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
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
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
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
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
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
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
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
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
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

/// BUG-2697: the drain launch marks its agent before host admission runs. A
/// memory-floor (or any other admission) refusal marked as before-spawn still
/// launched nobody, so the setup (lease, tree and claim) is rolled back; an
/// unmarked error after the mark keeps it.
// trace:BUG-2697 | ai:claude
#[test]
fn bug_2697_an_admission_refusal_after_the_mark_rolls_back() {
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let scope = "TASK-926970";
    let receipt = launching_setup(scope);
    let (_, tree) = lease_tree(&fx.main, scope).expect("setup minted a lease");

    let (result, rolled_back) = run_pickup_launch(&fx.main, receipt, |agent: &AgentLaunchMark| {
        agent.starting();
        Err(crate::lane_admission::refused_before_spawn(anyhow::anyhow!(
            "host memory headroom is below the lane floor"
        ))
        .context("could not start the headless implementer"))
    });

    let error = result.unwrap_err();
    assert!(
        format!("{error:#}").contains("below the lane floor"),
        "{error:#}"
    );
    assert!(rolled_back, "an admission refusal launched nobody");
    assert_gone(&fx.main, &tree, scope);
    assert!(!crate::queue_cmd::launch_refused_before_agent(
        &Err(anyhow::anyhow!("the implementer exited")),
        true
    ));
}

/// BUG-2697 (ADR-76): the real drain launch boundary. A pickup whose setup
/// (`session_start_with_receipt`) owns the rollback guard reaches
/// `run_vendor_headless_to_exit`, whose host admission refuses at the memory
/// floor before any agent spawns. The shared claim it pushed is released,
/// locally and on the store's origin, together with the lease and tree. It
/// stays behind if that launcher stops marking its admission errors as
/// before-spawn or if the setup stops owning the claim.
// trace:BUG-2697 | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn bug_2697_a_memory_floor_refusal_at_the_drain_launcher_releases_the_shared_claim() {
    use crate::lane_admission::{TestHost, TEST_HOST};
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let (store, origin) = distributed_store(&fx);
    let scope = "TASK-926972";
    let receipt = launching_setup(scope);
    let (lease_id, tree) = lease_tree(&fx.main, scope).expect("setup minted a lease");
    assert!(local_claim(&store, scope), "the setup holds a shared claim");
    assert!(origin_claim(&store, &origin, scope), "the claim was pushed");
    let branch = super::tests::git_ok(&tree, &["branch", "--show-current"]);
    // The disk floor is a separate preflight; keep it out of the way.
    let config = fx.main.join(".aida/config.toml");
    let mut body = std::fs::read_to_string(&config).unwrap_or_default();
    body.push_str("\n[doctor.disk_headroom]\nmin_free_gib = 1\n");
    std::fs::write(&config, body).unwrap();
    TEST_HOST.with(|h| {
        *h.borrow_mut() = Some(TestHost {
            max_concurrent_cold_starts: 1,
            min_memory_headroom_gib: 4,
            prefix: TestHost::prefix(),
            memory_gib: std::rc::Rc::new(std::cell::Cell::new(1)),
            on_wait: Box::new(|| panic!("a free slot does not queue")),
        })
    });
    let grant = crate::gate::seat_authority::HeadlessChildGrant::Baseline {
        cause: "bug-2697 fixture".into(),
    };
    let tee = crate::headless_tee::TeeOptions {
        enabled: false,
        label: None,
    };
    let log = fx.root.join("child.jsonl");

    let (result, rolled_back) = run_pickup_launch(&fx.main, receipt, |agent: &AgentLaunchMark| {
        // The drain marks its agent before the launcher's admission runs.
        agent.starting();
        crate::session::run_vendor_headless_to_exit(
            Some(&fx.main),
            crate::session::HeadlessVendor::Claude,
            &format!("/aida-pickup {scope}"),
            "019e0000-0000-7000-8000-000000002697",
            &log,
            &tee,
            false,
            Some(&lease_id),
            "implementer",
            &grant,
            Some((&tree, &branch)),
            false,
            None,
        )
        .map(|_| unreachable!("the launch is refused before the agent"))
    });
    TEST_HOST.with(|h| h.borrow_mut().take());

    let error = result.unwrap_err();
    assert!(
        format!("{error:#}").contains("below the 4 GiB floor"),
        "{error:#}"
    );
    assert!(rolled_back, "the refused launch started no agent");
    assert_gone(&fx.main, &tree, scope);
    assert!(!local_claim(&store, scope), "the local claim is released");
    assert!(
        !origin_claim(&store, &origin, scope),
        "the shared claim is released on origin"
    );
}

/// BUG-2697: a claim push that errors after the remote accepted it still
/// returns the exact claim. A start without the launch rollback that then
/// fails before its lease is written releases that claim on origin.
// trace:BUG-2697 | ai:claude
#[cfg(unix)]
#[test]
fn bug_2697_an_errored_claim_push_is_released_when_an_unrolled_start_fails() {
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let (store, origin) = distributed_store(&fx);
    // One-shot: the next push lands on origin, then its transport fails.
    // The receive-pack argument is the remote repository path.
    let wrapper = fx.root.join("flaky-receive-pack");
    crate::test_exec::write_executable(
        &wrapper,
        "#!/bin/sh\ngit receive-pack \"$1\"\nrc=$?\n\
         if [ -e \"$1/fail-next-push\" ]; then rm -f \"$1/fail-next-push\"; exit 1; fi\n\
         exit $rc\n",
    );
    std::fs::write(origin.join("fail-next-push"), "").unwrap();
    super::tests::git_ok(
        &store,
        &[
            "config",
            "remote.origin.receivepack",
            wrapper.to_str().unwrap(),
        ],
    );
    let scope = "TASK-926973";

    crate::seat_clone::failpoint::fail_before_lease(true);
    let result = crate::session_start::session_start_with_receipt(
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
        /* rollback_failed_setup */ false,
        crate::session_start::PathReentry::None,
        None,
        /* done_rework_admitted */ false,
    );
    crate::seat_clone::failpoint::fail_before_lease(false);

    let error = result.err().expect("injected setup failure");
    assert!(
        format!("{error:#}").contains("injected failure before the lease write"),
        "{error:#}"
    );
    assert!(
        !origin.join("fail-next-push").exists(),
        "the claim push errored"
    );
    let subjects = super::tests::git_ok(&origin, &["log", "--format=%s", "aida-store"]);
    assert!(
        subjects.contains(&format!("claim lease {scope}")),
        "origin accepted the claim before the push errored: {subjects}"
    );
    assert!(
        !origin_claim(&store, &origin, scope),
        "the failed start released its claim on origin: {subjects}"
    );
    assert!(lease_tree(&fx.main, scope).is_none());
}

/// TASK-2807: a panic after the agent ran unwinds through a disarmed setup,
/// so its uncommitted work and lease survive; the panic still propagates.
#[test]
fn a_panic_after_the_agent_started_keeps_the_worktree_and_lease() {
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
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
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
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
    for inherited in ["", "advisor", "reviewer"] {
        let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
        let fx = fixture();
        let mut ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
        ambient.set_key("AIDA_SESSION_ROLE", inherited);
        let (store, _origin) = distributed_store(&fx);
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

// trace:BUG-2548 | ai:codex
#[test]
fn bug_2548_refused_review_removes_the_actual_lease_file() {
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
    let fx = fixture();
    let mut ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "reviewer", &[]);
    let gh = fx.root.join("fake-gh");
    crate::test_exec::write_executable(&gh, "#!/bin/sh\nexit 1\n");
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    assert!(crate::test_git::git_command()
        .arg("-C")
        .arg(&fx.main)
        .args(["push", "-q", "origin", "HEAD:refs/pull/2548/head"])
        .status()
        .unwrap()
        .success());
    let scope = "PR-2548";
    let receipt = launching_setup(scope);
    let (id, tree) = lease_tree(&fx.main, scope).unwrap();
    let path = crate::lease_path(&fx.main, &id);
    assert!(path.is_file());
    let (result, rolled_back) = run_pickup_launch(&fx.main, receipt, |_| {
        anyhow::bail!("reviewer spawn refused")
    });
    assert!(result.is_err());
    assert!(rolled_back);
    assert!(
        !path.exists(),
        "rollback must unlink the actual lease record"
    );
    assert_gone(&fx.main, &tree, scope);
}

// trace:BUG-2548 | ai:codex
#[test]
fn bug_2548_retained_owner_cannot_report_successful_rollback() {
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let scope = "TASK-928078";
    let mut receipt = launching_setup(scope);
    let (id, tree) = lease_tree(&fx.main, scope).unwrap();
    receipt.require_owner(|| false);
    let (result, rolled_back) = run_pickup_launch(&fx.main, receipt, |_| {
        anyhow::bail!("launch refused after ownership moved")
    });
    assert!(!rolled_back, "retained setup is not a successful rollback");
    assert!(format!("{:#}", result.unwrap_err()).contains("retained lease"));
    assert!(crate::lease_path(&fx.main, &id).is_file());
    assert!(tree.exists());
}
