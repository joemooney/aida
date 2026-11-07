//! STORY-2254 AC1: `aida ps` shows a detached Codex lane as running.
//!
//! The detached launcher writes the lane's lease into the MAIN checkout's
//! `.aida/sessions` and stamps the Codex worker as its `active_pid`. `aida ps`
//! run from a plain linked worktree (no `.aida/sessions` link) used to read
//! that worktree's own empty lease dir and report `running: 0`.
// trace:STORY-2254 | ai:claude
use super::*;
use std::path::Path;
// trace:BUG-2476 | ai:codex

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
    let mut body = format!(
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
    );
    if let Some(namespace) = aida_core::pid_observer::current_pid_namespace() {
        body.push_str(&format!("pid_namespace = {namespace}\n"));
    }
    body
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

/// Exercise the cooperative spawn -> detached stamp -> dual ownership -> judge
/// path, including reparenting after the launch helper exits.
// trace:STORY-2537 trace:BUG-2458 | ai:codex
#[cfg(target_os = "linux")]
#[test]
fn proc_identity_detached_stamp_tracks_reparented_worker_until_dead() {
    use aida_core::proc_identity::{HolderRecord, Verdict};
    use std::os::unix::process::CommandExt;
    const ROOT_ENV: &str = "AIDA_TEST_DETACHED_HOLDER_ROOT";
    const GRANT: &str = "72f655dc-78e8-4320-933b-8c10f9366e73";
    if let Some(root) = std::env::var_os(ROOT_ENV) {
        let mut command = std::process::Command::new("/bin/sleep");
        command.arg("60");
        // SAFETY: setsid has no memory/allocator interactions after fork.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let pending = aida_core::proc_identity::prepare_command(&mut command).unwrap();
        let mut worker = command.spawn().unwrap();
        let holder = pending.finish(&mut worker);
        assert_eq!(holder.validate().unwrap().pid, worker.id());
        let start = crate::process_probe::process_start_identity(worker.id()).unwrap();
        crate::session::stamp_lease_detached_worker_with_holder_at(
            Path::new(&root),
            "detached-holder",
            GRANT,
            worker.id(),
            &start,
            &holder,
        )
        .unwrap();
        // Deliberately leave the independently session-led worker to the parent
        // test's cleanup. Child::drop neither signals nor waits for it.
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let sessions = root.join(".aida/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("detached-holder.toml"),
        detached_codex_lease(
            "detached-holder",
            root,
            std::process::id(),
            &crate::process_probe::process_start_identity(std::process::id()).unwrap(),
        )
        .replace(
            &format!("active_pid = {}", std::process::id()),
            "active_pid = 4194303",
        ),
    )
    .unwrap();
    let name = "ps_integrate::story_2254_ps_detached_codex_tests::proc_identity_detached_stamp_tracks_reparented_worker_until_dead";
    let mut launcher = std::process::Command::new(crate::aida_bin::running_executable_fallback())
        .args(["--exact", name, "--nocapture"])
        .env(ROOT_ENV, root)
        .spawn()
        .unwrap();
    let launcher_pid = launcher.id();
    let status = launcher.wait().unwrap();
    let lease: crate::SessionLease =
        toml::from_str(&std::fs::read_to_string(sessions.join("detached-holder.toml")).unwrap())
            .unwrap();
    let holder: HolderRecord =
        serde_json::from_str(lease.holder_record.as_deref().unwrap()).unwrap();
    let pid = holder.validate().unwrap().pid;
    struct Cleanup(HolderRecord);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if self.0.observe() == Ok(Verdict::Alive) {
                // SAFETY: the observed sleeping worker is private to this fixture.
                unsafe {
                    libc::kill(self.0.validate().unwrap().pid as i32, libc::SIGKILL);
                }
            }
        }
    }
    let _cleanup = Cleanup(holder.clone());
    assert!(status.success());
    assert_eq!(lease.active_pid, Some(pid));
    assert_ne!(pid, launcher_pid);
    let facts = aida_core::process_probe::ProbeSource::system()
        .facts(pid)
        .unwrap();
    assert_ne!(facts.ppid, launcher_pid, "worker must be reparented");
    assert_eq!(unsafe { libc::getsid(pid as i32) }, pid as i32);
    assert_eq!(
        crate::leases::lease_process_verdict(&lease).unwrap(),
        Verdict::Alive
    );
    let writer = crate::ownership::dual::WriterFacts::here();
    let ownership = crate::ownership::dual::holder_for_lease(&lease, &writer, chrono::Utc::now());
    assert_eq!(ownership.witnesses.len(), 1);
    assert_eq!(
        ownership.witnesses[0]
            .admission
            .as_ref()
            .unwrap()
            .validate()
            .unwrap()
            .pid,
        pid
    );
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !matches!(
        crate::leases::lease_process_verdict(&lease),
        Ok(Verdict::Dead(_))
    ) {
        assert!(
            std::time::Instant::now() < deadline,
            "exited worker did not read Dead"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let source = aida_core::process_probe::ProbeSource::system();
    let probe = |id: &aida_core::process_probe::ProcessIdentity| source.probe(id);
    let host = crate::ownership::HostFacts {
        node_id: aida_core::proc_identity::local_node_id(),
        host: lease.hostname.clone(),
        boot: aida_core::local_boot_id(),
        pid_ns: lease.pid_namespace,
        probe: &probe,
    };
    assert!(matches!(
        crate::ownership::judge_owner(&ownership, &host, chrono::Utc::now()),
        crate::ownership::OwnerVerdict::Dead { .. }
    ));
    assert!(matches!(
        crate::leases::lease_state_for(&lease, &[], chrono::Utc::now()),
        aida_core::liveness::LeaseState::Stale
    ));
}
