//! Direct writer barrier coverage. trace:TASK-1981 | ai:codex
use super::*;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
enum Writer {
    Full,
    RemoteOnly,
    Owner,
    Name,
    Hijack,
    Unregister,
}

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("store");
    let bare = dir.path().join("origin.git");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&bare).unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "--bare", "-q", "-b", "aida-store"])
        .current_dir(&bare)
        .status()
        .unwrap()
        .success());
    init(&repo).unwrap();
    // trace:BUG-2017 | ai:codex — this writer-lock fixture explicitly models HOME.
    crate::allocation_authority::home_fixture_backend(&repo).unwrap();
    configure_user(&repo, "Test", "test@example.com").unwrap();
    for target in [&repo, &bare] {
        for (key, value) in [
            ("maintenance.auto", "false"),
            ("gc.auto", "0"),
            ("commit.gpgSign", "false"),
            ("rebase.autoStash", "false"),
        ] {
            git_test(target, &["config", key, value]);
        }
    }
    assert!(host_git_command(&repo)
        .args(["checkout", "-b", "aida-store"])
        .status()
        .unwrap()
        .success());
    assert!(host_git_command(&repo)
        .args(["remote", "add", "origin", bare.to_str().unwrap()])
        .status()
        .unwrap()
        .success());
    let registry_path = repo.join("registry/nodes.toml");
    let mut registry = crate::NodeRegistry::default();
    registry.register_specific_full(
        "n1".into(),
        1000,
        "old-host".into(),
        Some("old@example.com".into()),
        None,
    );
    registry.save(&registry_path).unwrap();
    add(&repo, &["registry/nodes.toml"]).unwrap();
    std::fs::write(repo.join("seed.txt"), "seed").unwrap();
    add(&repo, &["seed.txt"]).unwrap();
    commit(&repo, "seed node registry").unwrap();
    assert!(push(&repo, "origin", "aida-store").unwrap());
    (dir, repo, bare, registry_path)
}

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

fn run_writer(repo: &Path, writer: Writer) -> Result<()> {
    match writer {
        Writer::Full => register_node_full_identity(
            repo,
            Some("n2".into()),
            1000,
            "new-host",
            Some("new@example.com".into()),
            NodeIdentity::default(),
        )
        .map(|_| ()),
        Writer::RemoteOnly => register_node_remote_only(
            repo,
            "n2".into(),
            1000,
            "new-host",
            "new@example.com".into(),
        )
        .map(|_| ()),
        Writer::Owner => set_node_identity_field(repo, "n1", NodeIdentityField::Owner, "new-owner"),
        Writer::Name => set_node_identity_field(repo, "n1", NodeIdentityField::Name, "new-name"),
        Writer::Hijack => {
            hijack_node(repo, "n1", 1000, "new-host", Some("new@example.com".into())).map(|_| ())
        }
        Writer::Unregister => unregister_node(repo, "n1").and_then(|removed| {
            anyhow::ensure!(removed, "node was not removed");
            Ok(())
        }),
    }
}

fn exercise_writer(writer: Writer) {
    let (_dir, repo, bare, registry_path) = fixture();
    let mut registry = crate::NodeRegistry::load(&registry_path).unwrap();
    let before_head = head_sha(&repo).unwrap();
    let before_registry = std::fs::read(&registry_path).unwrap();
    let before_index = std::fs::read(repo.join(".git/index")).unwrap();
    let contended = crate::db::store_write_lock_contended_for(&repo);
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let mut child = None;
    crate::db::with_store_write_lock(&repo, || {
        let contender = repo.clone();
        child = Some(std::thread::spawn(move || {
            let result = run_writer(&contender, writer);
            done_tx.send(result).unwrap();
        }));
        let started = Instant::now();
        while crate::db::store_write_lock_contended_for(&repo) == contended {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "{writer:?} never contended"
            );
            assert!(
                done_rx.try_recv().is_err(),
                "{writer:?} escaped writer lock"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(head_sha(&repo)?, before_head);
        assert_eq!(std::fs::read(&registry_path)?, before_registry);
        assert_eq!(std::fs::read(repo.join(".git/index"))?, before_index);
        assert!(!repo.join(".aida/node.toml").exists());
        // The waiting writer must reload, preserving a peer's registry edit.
        registry.register_specific_full("n99".into(), 1000, "peer-host".into(), None, None);
        registry.save(&registry_path)?;
        add(&repo, &["registry/nodes.toml"])?;
        commit(&repo, "peer edit while writer is waiting")?;
        Ok(())
    })
    .unwrap();
    done_rx
        .recv_timeout(Duration::from_secs(20))
        .unwrap()
        .unwrap();
    child.unwrap().join().unwrap();
    let saved = crate::NodeRegistry::load(&registry_path).unwrap();
    assert!(saved.is_registered("n99"), "{writer:?} lost peer's node");
    let node = saved.nodes.iter().find(|n| n.id == "n1");
    match writer {
        Writer::Full => {
            assert!(saved.is_registered("n2"));
            assert_eq!(
                crate::NodeConfig::load(&repo.join(".aida/node.toml"))
                    .unwrap()
                    .node_id,
                "n2"
            );
        }
        Writer::RemoteOnly => {
            assert!(saved.is_registered("n2"));
            assert!(!repo.join(".aida/node.toml").exists());
        }
        Writer::Owner => assert_eq!(node.unwrap().user.as_deref(), Some("new-owner")),
        Writer::Name => assert_eq!(node.unwrap().name.as_deref(), Some("new-name")),
        Writer::Hijack => {
            assert_eq!(node.unwrap().hostname, "new-host");
            assert_eq!(
                crate::NodeConfig::load(&repo.join(".aida/node.toml"))
                    .unwrap()
                    .node_id,
                "n1"
            );
        }
        Writer::Unregister => assert!(node.is_none()),
    }
    let remote_head = host_git_command(&bare)
        .args(["rev-parse", "refs/heads/aida-store"])
        .output()
        .unwrap();
    assert!(remote_head.status.success());
    assert_eq!(
        String::from_utf8(remote_head.stdout).unwrap().trim(),
        head_sha(&repo).unwrap()
    );
}

#[test]
fn task_1981_remote_only_registration_waits_and_reloads() {
    exercise_writer(Writer::RemoteOnly);
}
#[test]
fn task_1981_node_owner_waits_and_reloads() {
    exercise_writer(Writer::Owner);
}
#[test]
fn task_1981_node_name_waits_and_reloads() {
    exercise_writer(Writer::Name);
}
#[test]
fn task_1981_node_hijack_waits_and_reloads() {
    exercise_writer(Writer::Hijack);
}
#[test]
fn task_1981_node_unregister_waits_and_reloads() {
    exercise_writer(Writer::Unregister);
}

#[test]
fn task_1981_full_registration_waits_and_reloads() {
    exercise_writer(Writer::Full);
}

fn exercise_rollback_error(writer: Writer) {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, repo, bare, _registry_path) = fixture();
    let remote_before = git_test(&bare, &["rev-parse", "refs/heads/aida-store"]);
    // Only this non-store fixture may install arbitrary hooks. The local
    // pre-push creates a later peer commit and dirties a tracked file before
    // the remote rejects. Remote hooks never create local Git objects in
    // receive-pack's quarantine. Autostash is explicitly disabled above.
    let pre_push = repo.join(".git/hooks/pre-push");
    std::fs::write(&pre_push, "#!/bin/sh\nset -eu\necho peer > peer.txt\ngit -c maintenance.auto=false -c gc.auto=0 -c commit.gpgSign=false add peer.txt\ngit -c maintenance.auto=false -c gc.auto=0 -c commit.gpgSign=false commit -q -m peer\necho dirty > seed.txt\ngit rev-parse HEAD > .git/peer-head\necho reached >> .git/push-count\n").unwrap();
    std::fs::set_permissions(&pre_push, std::fs::Permissions::from_mode(0o755)).unwrap();
    let reject = bare.join("hooks/pre-receive");
    std::fs::write(
        &reject,
        "#!/bin/sh\necho task-1981-remote-refusal >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&reject, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = run_writer(&repo, writer).unwrap_err();
    assert!(
        format!("{error:#}").contains("failed without conflict"),
        "{writer:?}: {error:#}"
    );
    let peer = std::fs::read_to_string(repo.join(".git/peer-head")).unwrap();
    assert_eq!(head_sha(&repo).unwrap(), peer.trim());
    assert_eq!(
        std::fs::read_to_string(repo.join("seed.txt")).unwrap(),
        "dirty\n"
    );
    assert_eq!(git_test(&repo, &["show", "HEAD:peer.txt"]), "peer");
    assert_eq!(
        git_test(&bare, &["rev-parse", "refs/heads/aida-store"]),
        remote_before
    );
    assert_eq!(
        std::fs::read_to_string(repo.join(".git/push-count")).unwrap(),
        "reached\n",
        "must stop rather than retry after rollback error"
    );
    assert!(!repo.join(".aida/node.toml").exists());
    assert!(!repo.join(".git/rebase-merge").exists());
    assert!(!repo.join(".git/rebase-apply").exists());
}

#[test]
fn task_1981_full_preserves_rollback_error() {
    exercise_rollback_error(Writer::Full);
}
#[test]
fn task_1981_remoteonly_preserves_rollback_error() {
    exercise_rollback_error(Writer::RemoteOnly);
}
#[test]
fn task_1981_owner_preserves_rollback_error() {
    exercise_rollback_error(Writer::Owner);
}
#[test]
fn task_1981_name_preserves_rollback_error() {
    exercise_rollback_error(Writer::Name);
}
#[test]
fn task_1981_hijack_preserves_rollback_error() {
    exercise_rollback_error(Writer::Hijack);
}
#[test]
fn task_1981_unregister_preserves_rollback_error() {
    exercise_rollback_error(Writer::Unregister);
}
