use super::*;

fn write_config(root: &std::path::Path, body: &str) {
    let config_dir = root.join(".aida");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), body).unwrap();
}

#[test]
fn missing_config_defaults_to_manual() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = read_store_sync_config(tmp.path()).unwrap();
    assert_eq!(cfg.auto_push, StoreAutoPushMode::Manual);
}

#[test]
fn parses_supported_auto_push_modes() {
    let cases = [
        ("manual", StoreAutoPushMode::Manual),
        ("session-end", StoreAutoPushMode::SessionEnd),
        ("per-write", StoreAutoPushMode::PerWrite),
        ("periodic", StoreAutoPushMode::Periodic),
    ];
    for (raw, expected) in cases {
        let tmp = tempfile::tempdir().unwrap();
        write_config(
                tmp.path(),
                &format!(
                    "[store.sync]\nauto_push = \"{raw}\"\nperiodic_threshold = 5\nperiodic_interval = \"30s\"\n"
                ),
            );
        let cfg = read_store_sync_config(tmp.path()).unwrap();
        assert_eq!(cfg.auto_push, expected);
        assert_eq!(cfg.periodic_threshold, Some(5));
        assert_eq!(cfg.periodic_interval.as_deref(), Some("30s"));
    }
}

#[test]
fn invalid_auto_push_reports_file_line_and_valid_values() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(
        tmp.path(),
        "[deployment]\nmode = \"distributed\"\n\n[store.sync]\nauto_push = \"always\"\n",
    );
    let err = read_store_sync_config(tmp.path()).unwrap_err().to_string();
    assert!(err.contains("config.toml:5"), "{err}");
    assert!(
        err.contains("manual, session-end, per-write, periodic"),
        "{err}"
    );
}

#[test]
fn allocation_retry_max_defaults_to_three() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = read_store_allocation_config(tmp.path()).unwrap();
    assert_eq!(cfg.retry_max, 3);
}

#[test]
fn allocation_retry_max_reads_store_allocation_section() {
    let tmp = tempfile::tempdir().unwrap();
    write_config(tmp.path(), "[store.allocation]\nretry_max = 7\n");
    let cfg = read_store_allocation_config(tmp.path()).unwrap();
    assert_eq!(cfg.retry_max, 7);
}

#[test]
fn allocation_retry_max_ignores_zero_and_negative_values() {
    for raw in ["0", "-2"] {
        let tmp = tempfile::tempdir().unwrap();
        write_config(
            tmp.path(),
            &format!("[store.allocation]\nretry_max = {raw}\n"),
        );
        let cfg = read_store_allocation_config(tmp.path()).unwrap();
        assert_eq!(cfg.retry_max, 3);
    }
}

#[test]
fn spec_id_collision_scan_detects_same_id_different_uuid() {
    let mut a = Requirement::new("local review story".to_string(), String::new());
    a.spec_id = Some("STORY-446".to_string());
    let mut b = Requirement::new("origin deps story".to_string(), String::new());
    b.spec_id = Some("story-446".to_string());
    let store = RequirementsStore {
        requirements: vec![a, b],
        ..RequirementsStore::new()
    };

    let collisions = find_spec_id_collisions(&store);
    assert_eq!(collisions.len(), 1);
    assert_eq!(collisions[0].spec_id, "STORY-446");
    assert_eq!(collisions[0].claimants.len(), 2);
}

#[test]
fn spec_id_collision_scan_ignores_single_claimant() {
    let mut req = Requirement::new("single".to_string(), String::new());
    req.spec_id = Some("TASK-1".to_string());
    let store = RequirementsStore {
        requirements: vec![req],
        ..RequirementsStore::new()
    };
    assert!(find_spec_id_collisions(&store).is_empty());
}

// BUG-701: the cache-backed path returns flat (spec_id, uuid, title) rows;
// `group_spec_id_collisions` reshapes them into the same `SpecIdCollision`s
// `find_spec_id_collisions` produces from a full store. trace:BUG-701
#[test]
fn group_spec_id_collisions_matches_full_store_grouping() {
    let u1 = Uuid::new_v4();
    let u2 = Uuid::new_v4();
    // Two distinct uuids claim FR-1 → a collision; a lone claimant does not.
    let rows = vec![
        ("FR-1".to_string(), u1, "first".to_string()),
        ("FR-1".to_string(), u2, "second".to_string()),
        ("BUG-2".to_string(), Uuid::new_v4(), "unique".to_string()),
    ];
    let collisions = group_spec_id_collisions(rows);
    assert_eq!(collisions.len(), 1);
    assert_eq!(collisions[0].spec_id, "FR-1");
    assert_eq!(collisions[0].claimants.len(), 2);
}

#[test]
fn group_spec_id_collisions_dedups_repeated_uuid_rows() {
    let u1 = Uuid::new_v4();
    // A single spec appearing twice under the same uuid is NOT a collision.
    let rows = vec![
        ("FR-1".to_string(), u1, "same".to_string()),
        ("FR-1".to_string(), u1, "same".to_string()),
    ];
    assert!(group_spec_id_collisions(rows).is_empty());
}

#[test]
fn collision_recovery_message_is_paste_ready() {
    let collision = SpecIdCollision {
        spec_id: "STORY-446".to_string(),
        claimants: vec![SpecIdClaimant {
            uuid: Uuid::new_v4(),
            title: "origin deps story".to_string(),
        }],
    };
    let msg = spec_id_collision_recovery_message(&[collision], std::path::Path::new("/tmp/s"));
    assert!(msg.contains("cd /tmp/s"), "{msg}");
    assert!(msg.contains("git status"), "{msg}");
    assert!(
        msg.contains("aida db check --collisions --show-conflict"),
        "{msg}"
    );
    assert!(msg.contains("Do not use `git rebase --skip`"), "{msg}");
}

#[test]
fn auto_push_failure_is_non_fatal_for_per_write_and_session_end() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join(".aida-store");
    std::fs::create_dir_all(&store).unwrap();
    for args in [
        &["init", "-b", "aida-store"][..],
        &["config", "user.email", "aida@example.test"],
        &["config", "user.name", "AIDA Test"],
        &[
            "remote",
            "add",
            "origin",
            "/definitely/missing/aida-store.git",
        ],
    ] {
        let status = std::process::Command::new("git")
            .current_dir(&store)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }

    std::fs::write(store.join("metadata.yaml"), "name: test\n").unwrap();
    write_config(tmp.path(), "[store.sync]\nauto_push = \"per-write\"\n");
    maybe_auto_push_store(&store, StoreAutoPushMode::PerWrite, "test-per-write");

    write_config(tmp.path(), "[store.sync]\nauto_push = \"session-end\"\n");
    std::fs::write(store.join("metadata.yaml"), "name: test2\n").unwrap();
    maybe_auto_push_store(&store, StoreAutoPushMode::SessionEnd, "test-session-end");
}
