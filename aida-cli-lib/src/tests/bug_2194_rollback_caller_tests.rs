//! Non-node CLI CAS callers keep a failed rollback observable and stop.
//! A local pre-push hook commits a peer and dirties a tracked file before the
//! push fails, so `rollback_commit_if_head_matches` must refuse the rebase.
//! trace:BUG-2194 | ai:claude
//! trace:BUG-2194.ac498808 BUG-2194.ac67e854 BUG-2194.ac0ab426 BUG-2194.ac727a60 | ai:claude
use super::*;
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn git_test(repo: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(repo)
        .args([
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "-c",
            "commit.gpgSign=false",
        ])
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

struct Fixture {
    _dir: tempfile::TempDir,
    project: PathBuf,
    store: PathBuf,
    bare: PathBuf,
    branch: String,
}

// The store directory is deliberately not named `.aida-store`: only a
// non-store fixture runs the test's own hooks (store trees pin hooksPath).
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    let store = project.join("store");
    let bare = dir.path().join("origin.git");
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/config.toml"),
        "[id_format]\ncounter_scope = \"per_type\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(&bare).unwrap();
    git_test(&bare, &["init", "--bare", "-q"]);
    aida_core::git_ops::init(&store).unwrap();
    aida_core::git_ops::configure_user(&store, "Test", "test@example.com").unwrap();
    for target in [&store, &bare] {
        for (key, value) in [
            ("maintenance.auto", "false"),
            ("gc.auto", "0"),
            ("commit.gpgSign", "false"),
            ("rebase.autoStash", "false"),
        ] {
            git_test(target, &["config", key, value]);
        }
    }
    git_test(&store, &["remote", "add", "origin", bare.to_str().unwrap()]);
    // This fixture explicitly models the allocation root (HOME).
    let common = aida_core::allocation_authority::common_dir(&store).unwrap();
    std::fs::write(
        common.join(aida_core::allocation_authority::CONFIG_FILE),
        "version=1\nauthoritative=true\n",
    )
    .unwrap();
    let mut nodes = aida_core::NodeRegistry::default();
    nodes.nodes.push(aida_core::node::NodeRegistryEntry {
        id: "n1".into(),
        user_id: 1,
        hostname: "host".into(),
        email: None,
        clone_path: None,
        name: Some("node-1".into()),
        user: Some("tester".into()),
        registered: chrono::Utc::now(),
        retired_at: None,
    });
    nodes.save(&store.join("registry/nodes.toml")).unwrap();
    aida_core::NodeConfig {
        node_id: "n1".into(),
        user_id: 1,
        hostname: "host".into(),
        email: None,
        name: Some("node-1".into()),
        user: Some("tester".into()),
        registered_at: chrono::Utc::now(),
    }
    .save(&store.join(".aida/node.toml"))
    .unwrap();
    std::fs::write(store.join("seed.txt"), "seed\n").unwrap();
    aida_core::git_ops::add(&store, &["registry", "seed.txt"]).unwrap();
    aida_core::git_ops::commit(&store, "seed store").unwrap();
    let branch = aida_core::git_ops::current_branch(&store).unwrap();
    assert!(aida_core::git_ops::push(&store, "origin", &branch).unwrap());
    Fixture {
        _dir: dir,
        project,
        store,
        bare,
        branch,
    }
}

#[derive(Clone, Copy)]
enum Failure {
    /// The remote rejects the push (`push_exact` → `Ok(false)`).
    Rejected,
    /// The local pre-push hook fails (`push_exact` → `Err`).
    Errored,
}

/// Arm the hooks and return the remote tip before the caller runs.
fn arm(fx: &Fixture, failure: Failure) -> String {
    let exit = match failure {
        Failure::Rejected => "",
        Failure::Errored => "exit 1\n",
    };
    let pre_push = fx.store.join(".git/hooks/pre-push");
    std::fs::write(
        &pre_push,
        format!(
            "#!/bin/sh\nset -eu\necho peer > peer.txt\n\
             git -c maintenance.auto=false -c gc.auto=0 -c commit.gpgSign=false add peer.txt\n\
             git -c maintenance.auto=false -c gc.auto=0 -c commit.gpgSign=false commit -q -m peer\n\
             echo dirty > seed.txt\ngit rev-parse HEAD > .git/peer-head\n\
             echo reached >> .git/push-count\n{exit}"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&pre_push, std::fs::Permissions::from_mode(0o755)).unwrap();
    if matches!(failure, Failure::Rejected) {
        let reject = fx.bare.join("hooks/pre-receive");
        std::fs::write(&reject, "#!/bin/sh\necho bug-2194-refusal >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&reject, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    git_test(
        &fx.bare,
        &["rev-parse", &format!("refs/heads/{}", fx.branch)],
    )
}

fn assert_preserved(fx: &Fixture, remote_before: &str, error: anyhow::Error, what: &str) {
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("failed without conflict"),
        "{what}: {rendered}"
    );
    assert!(rendered.contains("rollback failed"), "{what}: {rendered}");
    let peer = std::fs::read_to_string(fx.store.join(".git/peer-head")).unwrap();
    assert_eq!(
        aida_core::git_ops::head_sha(&fx.store).unwrap(),
        peer.trim(),
        "{what}: peer HEAD must survive"
    );
    assert_eq!(git_test(&fx.store, &["show", "HEAD:peer.txt"]), "peer");
    assert_eq!(
        std::fs::read_to_string(fx.store.join("seed.txt")).unwrap(),
        "dirty\n",
        "{what}: dirty tracked file must survive"
    );
    assert_eq!(
        git_test(
            &fx.bare,
            &["rev-parse", &format!("refs/heads/{}", fx.branch)]
        ),
        remote_before,
        "{what}: origin must be unchanged"
    );
    assert_eq!(
        std::fs::read_to_string(fx.store.join(".git/push-count")).unwrap(),
        "reached\n",
        "{what}: must stop rather than retry after the rollback error"
    );
    assert!(!fx.store.join(".git/rebase-merge").exists());
    assert!(!fx.store.join(".git/rebase-apply").exists());
}

#[test]
fn bug_2194_block_claim_stops_on_rollback_error() {
    let fx = fixture();
    let before = arm(&fx, Failure::Rejected);
    let error = handle_block_command(
        &BlockCommand::Claim {
            r#type: "FR".into(),
            size: 50,
        },
        &fx.store,
    )
    .unwrap_err();
    assert_preserved(&fx, &before, error, "block claim");
}

#[test]
fn bug_2194_auto_allocate_stops_on_rollback_error() {
    let fx = fixture();
    let before = arm(&fx, Failure::Rejected);
    let error = auto_allocate_block_inner(
        &fx.store,
        "n1",
        "host",
        None,
        "FR",
        50,
        BlockClaimReason::OnAcquire,
    )
    .unwrap_err();
    assert_preserved(&fx, &before, error, "auto allocate");
}

#[test]
fn bug_2194_schedule_ledger_write_stops_on_rollback_error() {
    let fx = fixture();
    let before = arm(&fx, Failure::Rejected);
    let error = crate::schedule_ledger::write_cas_opts(&fx.store, "bug-2194-job", true, |l| {
        l.note = Some("tick".into())
    })
    .unwrap_err();
    assert_preserved(&fx, &before, error, "schedule ledger write");
}

#[test]
fn bug_2194_schedule_ledger_batch_stops_on_rollback_error() {
    let fx = fixture();
    let before = arm(&fx, Failure::Rejected);
    let mut ledger = crate::schedule_ledger::JobLedger::new("bug-2194-job");
    ledger.note = Some("tick".into());
    let next = BTreeMap::from([("bug-2194-job".to_string(), ledger)]);
    let error =
        crate::schedule_ledger::write_batch_cas_opts(&fx.store, &BTreeMap::new(), &next, true)
            .unwrap_err();
    assert_preserved(&fx, &before, error, "schedule ledger batch");
}

fn acquire_lock(fx: &Fixture) -> Result<crate::coordination::LockAcquireOutcome> {
    crate::coordination::acquire_lock_claim(
        &fx.store,
        crate::coordination::LockKind::Drain,
        &fx.project,
        "bug-2194",
        60,
        false,
    )
}

#[test]
fn bug_2194_lock_claim_rejected_push_stops_on_rollback_error() {
    let fx = fixture();
    let before = arm(&fx, Failure::Rejected);
    let error = acquire_lock(&fx).unwrap_err();
    assert_preserved(&fx, &before, error, "lock claim (rejected)");
}

// A push error normally degrades to Unavailable (proceed local-only); with a
// failed rollback the claim commit is still local, so it must be an error.
#[test]
fn bug_2194_lock_claim_push_error_is_not_unavailable_after_rollback_error() {
    let fx = fixture();
    let before = arm(&fx, Failure::Errored);
    let error = acquire_lock(&fx).unwrap_err();
    assert_preserved(&fx, &before, error, "lock claim (push error)");
}
