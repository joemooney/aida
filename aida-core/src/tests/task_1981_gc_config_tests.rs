//! Config transactions serialize; expensive GC does not occupy the writer lock.
//! trace:TASK-1981 | ai:codex
use super::*;
use std::time::{Duration, Instant};

fn config_barrier(opportunistic: bool) {
    let test = if opportunistic {
        "git_ops::task_1981_gc_config_tests::task_1981_opportunistic_gc_config_waits_for_writer"
    } else {
        "git_ops::task_1981_gc_config_tests::task_1981_gc_env_config_waits_for_writer"
    };
    if std::env::var_os("AIDA_TEST_GC_CONFIG_BARRIER_CHILD").is_none() {
        let global = tempfile::NamedTempFile::new().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture"])
            .env("AIDA_TEST_GC_CONFIG_BARRIER_CHILD", "1")
            .env("AIDA_STORE_GC_AUTO", "777")
            .env("AIDA_STORE_GC_AGGRESSIVE_PACKS", "off")
            .env("GIT_CONFIG_GLOBAL", global.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stdout}\n{stderr}");
        assert!(
            stdout.lines().any(|line| line == "running 1 test")
                && stdout.contains("test result: ok. 1 passed; 0 failed;"),
            "{stdout}\n{stderr}"
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("store");
    init(&repo).unwrap();
    configure_user(&repo, "Test", "test@example.com").unwrap();
    for (key, value) in [
        ("maintenance.auto", "false"),
        ("gc.auto", "0"),
        ("gc.autoDetach", "false"),
        ("commit.gpgSign", "false"),
    ] {
        assert!(git(&repo, &["config", key, value]).unwrap().success);
    }
    let before = crate::db::store_write_lock_contended_for(&repo);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut child = None;
    crate::db::with_store_write_lock(&repo, || {
        let contender = repo.clone();
        child = Some(std::thread::spawn(move || {
            if opportunistic {
                opportunistic_store_gc(&contender);
            } else {
                apply_store_gc_auto_from_env(&contender);
            }
            tx.send(()).unwrap();
        }));
        let started = Instant::now();
        while crate::db::store_write_lock_contended_for(&repo) == before {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "config never contended"
            );
            assert!(rx.try_recv().is_err(), "config escaped writer lock");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(git(&repo, &["config", "gc.auto"])?.stdout, "0");
        assert_eq!(git(&repo, &["config", "gc.autoDetach"])?.stdout, "false");
        // A peer config field written while the contender waits must survive.
        assert!(git(&repo, &["config", "aida.peer", "retained"])?.success);
        Ok(())
    })
    .unwrap();
    rx.recv_timeout(Duration::from_secs(20)).unwrap();
    child.unwrap().join().unwrap();
    assert_eq!(git(&repo, &["config", "gc.auto"]).unwrap().stdout, "777");
    assert_eq!(
        git(&repo, &["config", "gc.autoDetach"]).unwrap().stdout,
        "true"
    );
    assert_eq!(
        git(&repo, &["config", "aida.peer"]).unwrap().stdout,
        "retained"
    );
}

#[test]
fn task_1981_gc_env_config_waits_for_writer() {
    config_barrier(false);
}
#[test]
fn task_1981_opportunistic_gc_config_waits_for_writer() {
    config_barrier(true);
}
