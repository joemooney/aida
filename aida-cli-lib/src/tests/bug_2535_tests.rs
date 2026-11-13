// trace:BUG-2535 | ai:codex
use super::run_post_ship_sweep_bounded;
use std::{
    process::Command,
    time::{Duration, Instant},
};

#[test]
fn bug_2535_sweep_success_and_failure_are_preserved() {
    let mut ok = Command::new("sh");
    ok.args(["-c", "exit 0"]);
    assert!(run_post_ship_sweep_bounded(ok, Duration::from_secs(2), locked_file()).is_ok());
    let mut failed = Command::new("sh");
    failed.args(["-c", "exit 7"]);
    let error =
        run_post_ship_sweep_bounded(failed, Duration::from_secs(2), locked_file()).unwrap_err();
    assert!(error.to_string().contains("exited with"), "{error}");
}

#[test]
fn bug_2535_stalled_scan_and_separate_group_descendant_stop_at_budget() {
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("survived");
    let ready = temp.path().join("ready");
    let mut cmd = Command::new("sh");
    // Model a probe that creates a fresh process group, then would mutate
    // after the sweep has timed out. All paths are positional arguments.
    cmd.args([
        "-c",
        r#"setsid sh -c 'touch "$2"; sleep 2; touch "$1"' sh "$1" "$2" & wait"#,
        "sh",
    ])
    .arg(&marker)
    .arg(&ready);
    let start = Instant::now();
    let error =
        run_post_ship_sweep_bounded(cmd, Duration::from_millis(500), locked_file()).unwrap_err();
    assert!(error.to_string().contains("budget"), "{error}");
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(
        ready.exists(),
        "descendant must have started before timeout"
    );
    std::thread::sleep(Duration::from_secs(2));
    assert!(!marker.exists(), "timed-out descendant continued cleanup");
}
fn locked_file() -> std::fs::File {
    use fs2::FileExt;
    let lock = tempfile::tempfile().unwrap();
    lock.lock_exclusive().unwrap();
    lock
}

#[test]
fn bug_2535_termination_wait_is_bounded_and_reaper_retains_lock() {
    use crate::process_retry::RetryEtxtbsy;
    use fs2::FileExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cleanup.lock");
    let lock = std::fs::File::create(&path).unwrap();
    lock.lock_exclusive().unwrap();
    let contender = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    let child = Command::new("sleep")
        .arg("0.5")
        .spawn_retrying_etxtbsy()
        .unwrap();
    let start = Instant::now();
    // Model a pending SIGKILL: termination cannot yet be observed.
    super::reap_sweep_bounded(child, lock, Duration::from_millis(20), |_| Ok(None));
    assert!(start.elapsed() < Duration::from_millis(300));
    assert!(contender.try_lock_exclusive().is_err());
    let deadline = Instant::now() + Duration::from_secs(3);
    while contender.try_lock_exclusive().is_err() {
        assert!(
            Instant::now() < deadline,
            "reaper never released lock after exit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn bug_2535_direct_entry_refuses_even_while_cleanup_lock_is_held() {
    use fs2::FileExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cleanup.lock");
    let lock = std::fs::File::create(&path).unwrap();
    for held in [false, true] {
        if held {
            lock.lock_exclusive().unwrap();
        }
        assert!(super::verify_sweep_lock(&path).is_err());
    }
}

#[test]
fn bug_2535_verified_handoff_retains_serialization() {
    use fs2::FileExt;
    const CHILD: &str = "AIDA_2535_HANDOFF_TEST";
    if let Some(path) = std::env::var_os(CHILD) {
        let result = super::verify_sweep_lock(std::path::Path::new(&path));
        if std::env::var_os("AIDA_2535_REFUSE").is_some() {
            assert!(
                result.is_err(),
                "accepted an unlocked or unrelated descriptor"
            );
        } else {
            let lock = result.unwrap();
            let contender = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            assert!(contender.try_lock_exclusive().is_err());
            drop(lock);
        }
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cleanup.lock");
    let unrelated = temp.path().join("unrelated.lock");
    std::fs::File::create(&unrelated).unwrap();
    for mode in ["valid", "unlocked", "unrelated"] {
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        if mode != "unlocked" {
            lock.lock_exclusive().unwrap();
        }
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "pr_cmd::bug_2535_tests::bug_2535_verified_handoff_retains_serialization",
            "--nocapture",
        ])
        .env(
            CHILD,
            if mode == "unrelated" {
                &unrelated
            } else {
                &path
            },
        );
        if mode != "valid" {
            cmd.env("AIDA_2535_REFUSE", "1");
        }
        run_post_ship_sweep_bounded(cmd, Duration::from_secs(5), lock).unwrap();
        let contender = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        // Parallel tests may be between fork and exec with an inherited
        // CLOEXEC descriptor. Require eventual release within a fixed bound.
        let deadline = Instant::now() + Duration::from_secs(3);
        while contender.try_lock_exclusive().is_err() {
            assert!(
                Instant::now() < deadline,
                "{mode}: cleanup lock leaked after sweep exit"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
