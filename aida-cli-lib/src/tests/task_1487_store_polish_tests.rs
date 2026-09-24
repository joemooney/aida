//! TASK-1487: store-resolution polish from the TASK-1486 review.
//! - `unattached_distributed_root` agrees with the main resolver's BUG-433
//!   shape detection (`.aida-store/objects` present, no config, no branch).
//! - `command_triggers_per_write_auto_push` recognizes the tracker
//!   subcommands that actually write the local store (a non-dry-run
//!   `jira`/`github pull`), and nothing else.
//! - `bulk_import_via_writer` stamps oplog entries with an attached
//!   dispenser's node id instead of defaulting to "0".
// trace:TASK-1487 | ai:claude

use super::*;
use std::fs;
use std::process::Command as StdCommand;
use tempfile::TempDir;

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = StdCommand::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git spawn")
        .status
        .success();
    assert!(ok, "git {args:?} failed");
}

fn init_repo(root: &std::path::Path) {
    fs::create_dir_all(root).unwrap();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "t@example.com"]);
    git(root, &["config", "user.name", "t"]);
    fs::write(root.join("README.md"), "hi\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "init"]);
}

/// BUG-433 shape: the store is physically attached (`.aida-store/objects`
/// exists) but neither `.aida/config.toml` nor an `aida-store` branch is
/// present — e.g. a session worktree forked from a commit that predates the
/// committed scaffolding. `unattached_distributed_root` must recognize this
/// project as distributed, the same way the main resolver's `distributed_root`
/// computation does via `attached_store_present`, instead of missing it and
/// letting `aida init --refresh` fall through to a stale legacy store.
// trace:TASK-1487 | ai:claude
#[test]
fn unattached_distributed_root_recognizes_bug_433_shape() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("proj");
    init_repo(&root);
    fs::create_dir_all(root.join(".aida-store/objects")).unwrap();

    assert!(
        !branch_exists_anywhere(&root, "aida-store"),
        "fixture must not have an aida-store branch"
    );
    assert!(
        !root.join(".aida/config.toml").exists(),
        "fixture must not declare distributed mode"
    );

    assert_eq!(
        unattached_distributed_root(&root).as_deref(),
        Some(root.as_path()),
        "a physically-attached store must be recognized even with no config \
         and no aida-store branch"
    );
}

/// A plain git repo with none of the three distributed signals stays
/// unrecognized — the BUG-433 shape check must not turn every git repo into
/// a "distributed project".
// trace:TASK-1487 | ai:claude
#[test]
fn unattached_distributed_root_stays_none_without_any_distributed_signal() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("proj");
    init_repo(&root);
    assert!(unattached_distributed_root(&root).is_none());
}

/// Only a non-dry-run tracker pull writes the local store (via
/// `bulk_import_via_writer`); every other tracker subcommand only reads the
/// store or talks to the remote tracker, so auto-push must not fire for
/// those.
// trace:TASK-1487 | ai:claude
#[test]
fn per_write_auto_push_fires_only_for_non_dry_run_tracker_pulls() {
    assert!(command_triggers_per_write_auto_push(&Command::Jira(
        JiraCommand::Pull {
            jql: None,
            limit: 50,
            dry_run: false,
        }
    )));
    assert!(!command_triggers_per_write_auto_push(&Command::Jira(
        JiraCommand::Pull {
            jql: None,
            limit: 50,
            dry_run: true,
        }
    )));
    assert!(command_triggers_per_write_auto_push(&Command::Github(
        GitHubCommand::Pull {
            labels: None,
            open_only: true,
            limit: 50,
            dry_run: false,
        }
    )));
    assert!(!command_triggers_per_write_auto_push(&Command::Github(
        GitHubCommand::Pull {
            labels: None,
            open_only: true,
            limit: 50,
            dry_run: true,
        }
    )));
    // Read-only / remote-only tracker subcommands never trigger it.
    assert!(!command_triggers_per_write_auto_push(&Command::Jira(
        JiraCommand::List {
            jql: None,
            limit: 20,
        }
    )));
    assert!(!command_triggers_per_write_auto_push(&Command::Jira(
        JiraCommand::Push {
            id: "TASK-1".to_string(),
        }
    )));
}

/// Seed an existing oplog whose log-level node id is still the unregistered
/// default ("0") — the realistic state of a store that already has history
/// from before a node id was claimed. `GitBackend::record_op` only ever
/// upgrades `log.node_id` away from exactly `"0"`, so a brand-new (never
/// written) oplog — whose `OpLog::default()` node id is `""`, not `"0"` —
/// would mask the bug this test targets.
// trace:TASK-1487 | ai:claude
fn seed_oplog_at_node_zero(store_dir: &std::path::Path) {
    let oplog_path = store_dir.join("oplog.yaml");
    aida_core::oplog::OpLog::new("0".to_string())
        .save(&oplog_path)
        .unwrap();
}

/// The core of the item-4 bug: `bulk_import_via_writer` used to build its own
/// fresh `GitBackend` and ignore any dispenser the caller's `Storage` carried,
/// so every tracker-imported requirement's oplog entry was stamped node_id
/// "0" regardless of this clone's real node id. Attaching the dispenser via
/// `Storage::with_dispenser` must now reach the oplog.
// trace:TASK-1487 | ai:claude
#[test]
fn bulk_import_via_writer_uses_the_storages_dispenser_node_id() {
    use aida_core::models::DispenserHandle;
    use aida_core::{IdMode, MemoryDispenser};
    use std::sync::Arc;

    let tmp = TempDir::new().unwrap();
    let store_dir = tmp.path().join("aida-store");
    std::fs::create_dir_all(&store_dir).unwrap();
    seed_oplog_at_node_zero(&store_dir);

    let dispenser = Arc::new(MemoryDispenser::new(IdMode::Distributed {
        node_id: "7".to_string(),
    }));
    let storage = Storage::new(&store_dir).with_dispenser(DispenserHandle(dispenser));

    let req = Requirement::new("Imported issue".to_string(), "from a tracker".to_string());
    let n = bulk_import_via_writer(&storage, "feat(jira)", std::iter::once(req)).unwrap();
    assert_eq!(n, 1);

    let oplog_path = store_dir.join("oplog.yaml");
    let log = aida_core::oplog::OpLog::load(&oplog_path).expect("oplog written");
    assert_eq!(
        log.node_id, "7",
        "bulk_import_via_writer must stamp the oplog with the attached \
         dispenser's node id, not leave the node_id-\"0\" default in place"
    );
}

/// Without an attached dispenser, the historical default (node_id "0" is
/// never upgraded) is unchanged — this is a targeted fix, not a behavior
/// change for every other `Storage` caller.
// trace:TASK-1487 | ai:claude
#[test]
fn bulk_import_via_writer_leaves_node_zero_unchanged_without_a_dispenser() {
    let tmp = TempDir::new().unwrap();
    let store_dir = tmp.path().join("aida-store");
    std::fs::create_dir_all(&store_dir).unwrap();
    seed_oplog_at_node_zero(&store_dir);
    let storage = Storage::new(&store_dir);

    let req = Requirement::new("Imported issue".to_string(), "from a tracker".to_string());
    bulk_import_via_writer(&storage, "feat(jira)", std::iter::once(req)).unwrap();

    let oplog_path = store_dir.join("oplog.yaml");
    let log = aida_core::oplog::OpLog::load(&oplog_path).expect("oplog written");
    assert_eq!(log.node_id, "0");
}
