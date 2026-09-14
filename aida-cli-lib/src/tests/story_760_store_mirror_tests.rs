use super::*;

fn write_config(root: &std::path::Path, body: &str) {
    let config_dir = root.join(".aida");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), body).unwrap();
}

// TASK-1096: mirror_remotes parses a string array.
#[test]
fn mirror_remotes_parses_array() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(
        tmp.path(),
        "[store.sync]\nmirror_remotes = [\"gitlab\", \"backup\"]\n",
    );
    let cfg = read_store_sync_config(tmp.path()).unwrap();
    assert_eq!(
        cfg.mirror_remotes,
        vec!["gitlab".to_string(), "backup".to_string()]
    );
}

// A bare string is tolerated as a single-remote shorthand.
#[test]
fn mirror_remotes_parses_bare_string() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[store.sync]\nmirror_remotes = \"gitlab\"\n");
    let cfg = read_store_sync_config(tmp.path()).unwrap();
    assert_eq!(cfg.mirror_remotes, vec!["gitlab".to_string()]);
}

// Absent key / absent section => empty (origin-only, unchanged behaviour).
#[test]
fn mirror_remotes_absent_is_empty() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[store.sync]\nauto_push = \"manual\"\n");
    assert!(read_store_sync_config(tmp.path())
        .unwrap()
        .mirror_remotes
        .is_empty());

    let tmp2 = tempfile::tempdir().unwrap();
    write_config(tmp2.path(), "[node]\nid = \"1\"\n");
    assert!(read_store_sync_config(tmp2.path())
        .unwrap()
        .mirror_remotes
        .is_empty());
}

// TASK-1227: failed mirror fan-out persists as remote-drift doctor input.
#[test]
fn mirror_fanout_failure_records_remote_drift_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join(".aida-store");
    std::fs::create_dir_all(&repo).unwrap();

    record_store_mirror_fanout_failure(
        tmp.path(),
        &repo,
        "aida-store",
        "gitlab",
        "authentication failed",
    );

    let failures = read_store_mirror_fanout_failures(tmp.path());
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].repo, repo.display().to_string());
    assert_eq!(failures[0].branch, "aida-store");
    assert_eq!(failures[0].remote, "gitlab");
    assert_eq!(failures[0].reason, "authentication failed");

    let findings = scan_store_mirror_fanout_failures(tmp.path());
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].category, "remote-drift");
    assert_eq!(
        findings[0].id,
        "remote-drift-mirror-fanout-gitlab-aida-store"
    );
    assert!(findings[0].summary.contains("authentication failed"));
    assert!(findings[0].action.contains("aida remote reconcile"));
    assert!(!findings[0].safe_heal);
}

// TASK-1227: a later successful mirror push clears the stale marker.
#[test]
fn mirror_fanout_success_clears_matching_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join(".aida-store");
    let other_repo = tmp.path().join("code");

    record_store_mirror_fanout_failure(tmp.path(), &repo, "aida-store", "gitlab", "failed");
    record_store_mirror_fanout_failure(tmp.path(), &other_repo, "main", "gitlab", "failed");

    clear_store_mirror_fanout_failure(tmp.path(), &repo, "aida-store", "gitlab");

    let failures = read_store_mirror_fanout_failures(tmp.path());
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].repo, other_repo.display().to_string());
    assert_eq!(failures[0].branch, "main");
}
