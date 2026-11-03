//! STORY-2254 AC1: `aida ps` shows a detached Codex lane as running.
//!
//! The detached launcher writes the lane's lease into the MAIN checkout's
//! `.aida/sessions` and stamps the Codex worker as its `active_pid`. `aida ps`
//! run from a plain linked worktree (no `.aida/sessions` link) used to read
//! that worktree's own empty lease dir and report `running: 0`.
// trace:STORY-2254 | ai:claude
use super::*;
use std::path::Path;

fn git(dir: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The lease shape `agent new codex --bg` leaves after the supervisor stamps
/// its worker (`stamp_lease_detached_worker_at`).
fn detached_codex_lease(id: &str, pool: &Path, worker: u32, worker_start: &str) -> String {
    format!(
        r#"id = "{id}"
scope = "TASK-2881"
slug = "task-2881"
owner = "t@t"
worktree_path = {pool}
branch = "task-2881-2"
started_at = "{started}"
hostname = "test-host"
role = "implementer"
review_verb = false
claim_verb = false
child_grant_id = "72f655dc-78e8-4320-933b-8c10f9366e73"
active_pid = {worker}
active_pid_start_time = "{worker_start}"
"#,
        pool = aida_core::toml_quote::toml_string(&pool.to_string_lossy()),
        started = chrono::Utc::now().to_rfc3339(),
    )
}

#[cfg(target_os = "linux")]
#[test]
fn ps_from_a_linked_worktree_shows_a_live_detached_codex_lane_then_drops_it_on_exit() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let main = base.join("main");
    let linked = base.join("linked");
    let pool = base.join("pool");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::create_dir_all(&pool).unwrap();
    crate::test_git::commit_fixture(&main);
    git(&main, &["worktree", "add", "-q", linked.to_str().unwrap()]);
    // A plain linked worktree carries tracked `.aida/` files but no
    // `.aida/sessions` link — the probe run's shape.
    std::fs::create_dir_all(linked.join(".aida")).unwrap();

    let mut worker = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .expect("spawn stand-in worker");
    let pid = worker.id();
    let start = process_probe::process_start_identity(pid).expect("worker start identity");
    std::fs::create_dir_all(leases_dir(&main)).unwrap();
    std::fs::write(
        lease_path(&main, "01a19f91d315"),
        detached_codex_lease("01a19f91d315", &pool, pid, &start),
    )
    .unwrap();

    // The pre-fix reading: the linked worktree's own lease dir is empty.
    assert!(gather_running_work(&linked).0.is_empty());

    let root = ps_project_root(&linked);
    assert_eq!(root.canonicalize().unwrap(), main);
    let running = |root: &Path| -> Vec<PsRow> {
        gather_running_work(root)
            .0
            .into_iter()
            .filter(|r| !matches!(r.state, LeaseState::Stale))
            .collect()
    };
    let rows = running(&root);
    let row = rows
        .iter()
        .find(|r| r.lease.id == "01a19f91d315")
        .unwrap_or_else(|| {
            let ids: Vec<_> = rows.iter().map(|r| (&r.lease.id, r.state)).collect();
            panic!("the live detached lane is running: {ids:?}")
        });
    assert_eq!(row.state, LeaseState::Live);
    assert_eq!(row.pid, Some(pid));
    assert_eq!(row.role.as_deref(), Some("implementer"));
    assert_eq!(row.lease.scope, "TASK-2881");

    worker.kill().unwrap();
    worker.wait().unwrap();
    assert!(
        running(&root).iter().all(|r| r.lease.id != "01a19f91d315"),
        "an exited lane leaves running"
    );
}
