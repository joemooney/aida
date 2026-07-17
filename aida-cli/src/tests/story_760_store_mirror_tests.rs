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
