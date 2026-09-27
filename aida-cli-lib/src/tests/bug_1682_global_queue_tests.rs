//! Cross-process regression fixtures for global queue membership/persistence.
//! Every scenario and worker re-executes the fenced lib-test binary; no parent
//! HOME mutation, production CLI test flag, or operator queue access is needed.
// trace:BUG-1682 | ai:codex

use super::*;
use aida_core::DatabaseBackend;
use std::collections::BTreeSet;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CHILD: &str = "global_queue::tests::bug_1682_process_child";
const ROLE: &str = "implementer";
const DEADLINE: Duration = Duration::from_secs(120);

struct Process(Child, Duration);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Process {
    fn success(&mut self) {
        let end = Instant::now() + self.1;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "child failed: {status}");
                return;
            }
            assert!(Instant::now() < end, "child deadline expired");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn kill(&mut self) {
        self.0.kill().unwrap();
        self.0.wait().unwrap();
    }
}

fn wait(path: &Path) {
    let end = Instant::now() + DEADLINE;
    while !path.exists() {
        assert!(
            Instant::now() < end,
            "waiting for {} timed out",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn mark(path: impl AsRef<Path>) {
    std::fs::write(path, "ready").unwrap();
}

fn control() -> PathBuf {
    PathBuf::from(std::env::var_os("BUG_1682_CONTROL").unwrap())
}

// Observation/pause only, compiled out of production. Hooks never alter the
// mutation algorithm; filesystem fault fixtures use the ordinary atomic helper.
pub(super) fn observe(stage: &str, _path: &Path) {
    let Some(dir) = std::env::var_os("BUG_1682_CONTROL") else {
        return;
    };
    let dir = PathBuf::from(dir);
    mark(dir.join(stage));
    if std::env::var("BUG_1682_PAUSE").as_deref() == Ok(stage) {
        wait(&dir.join("release"));
    }
    if stage == "before_publish" && std::env::var("BUG_1682_PANIC").is_ok() {
        panic!("injected unwind before publication");
    }
}

fn spawn(home: &Path, action: &str, index: usize, pause: &str) -> (Process, PathBuf) {
    let ctl = home.join(format!("control-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&ctl).unwrap();
    let fence = std::env::var_os("AIDA_LIB_TEST_REAL_HOMES").expect("inherited real-home fence");
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", CHILD, "--nocapture", "--test-threads=1"])
        .current_dir(home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("AIDA_LIB_TEST_REAL_HOMES", &fence)
        .env("BUG_1682_EXPECTED_FENCE", &fence)
        .env("BUG_1682_HOME", home)
        .env("BUG_1682_ACTION", action)
        .env("BUG_1682_INDEX", index.to_string())
        .env("BUG_1682_CONTROL", &ctl)
        .env("BUG_1682_PAUSE", pause)
        .env_remove("BUG_1682_PANIC")
        .env("AIDA_USER", "queue-test")
        .stdin(Stdio::null());
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("AIDA_SESSION_") {
            cmd.env_remove(key);
        }
    }
    for key in [
        "AIDA_HOME",
        "AIDA_TEST_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ] {
        cmd.env_remove(key);
    }
    cmd.env("AIDA_SESSION_ROLE", "advisor");
    let timeout = if matches!(
        action,
        "adds" | "sidecar" | "mixed" | "readers" | "killed" | "errors" | "callers" | "fence"
    ) {
        Duration::from_secs(600)
    } else {
        DEADLINE
    };
    (Process(cmd.spawn().unwrap(), timeout), ctl)
}

fn scenario(name: &str) {
    let home = tempfile::tempdir().unwrap();
    let (mut child, _) = spawn(home.path(), name, 0, "");
    child.success();
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("BUG_1682_HOME").unwrap())
}

fn entry(id: usize, project: usize) -> GlobalQueueEntry {
    GlobalQueueEntry {
        requirement_id: uuid::Uuid::from_u128(id as u128),
        project_root: home().join(format!("project-{project}")),
        project_name: format!("project-{project}"),
        spec_id: Some(format!("TASK-{id}")),
        agreed_id: None,
        title: None,
        position: id as i64,
        added_by: "fixture".into(),
        added_at: Utc::now(),
        note: None,
        for_role: ROLE.into(),
    }
}

fn identities() -> BTreeSet<(uuid::Uuid, PathBuf)> {
    let entries = load(ROLE).unwrap();
    let set: BTreeSet<_> = entries
        .iter()
        .map(|e| (e.requirement_id, e.project_root.clone()))
        .collect();
    assert_eq!(entries.len(), set.len(), "duplicate upsert keys");
    set
}

fn expected(ids: impl IntoIterator<Item = (usize, usize)>) -> BTreeSet<(uuid::Uuid, PathBuf)> {
    ids.into_iter()
        .map(|(id, p)| {
            let e = entry(id, p);
            (e.requirement_id, e.project_root)
        })
        .collect()
}

fn group(action: &str, count: usize) -> Vec<Process> {
    let children: Vec<_> = (0..count).map(|i| spawn(&home(), action, i, "")).collect();
    for (_, ctl) in &children {
        wait(&ctl.join("ready"));
    }
    for (_, ctl) in &children {
        mark(ctl.join("start"));
    }
    children.into_iter().map(|(child, _)| child).collect()
}

fn barrier() {
    mark(control().join("ready"));
    wait(&control().join("start"));
}

fn adds() {
    for seeded in [false, true] {
        let path = queue_path(ROLE).unwrap();
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        if seeded {
            add(ROLE, entry(999, 9)).unwrap();
        }
        let mut children = group("writer", 6);
        for child in &mut children {
            child.success();
        }
        let mut want: Vec<_> = (0..6)
            .flat_map(|i| (0..20).map(move |j| (i * 20 + j, i)))
            .collect();
        if seeded {
            want.push((999, 9));
        }
        assert_eq!(identities(), expected(want));
    }
}

fn sidecar() {
    add(ROLE, entry(999, 9)).unwrap();
    let sidecar = queue_path(ROLE).unwrap().with_extension("lock");
    std::fs::write(&sidecar, "permanent sidecar").unwrap();
    let (mut holder, hctl) = spawn(&home(), "one", 1, "before_publish");
    wait(&hctl.join("before_publish"));
    let (mut waiter, wctl) = spawn(&home(), "one", 2, "");
    wait(&wctl.join("before_lock"));
    // Completion of another role is an independent progress handshake while
    // the same-role holder is still confirmed inside its critical section.
    let (mut other, _) = spawn(&home(), "other_role", 0, "");
    other.success();
    assert!(!wctl.join("after_lock").exists());
    assert!(waiter.0.try_wait().unwrap().is_none());
    mark(hctl.join("release"));
    holder.success();
    waiter.success();
    assert_eq!(identities(), expected([(999, 9), (1, 0), (2, 0)]));
    assert_eq!(
        std::fs::read_to_string(sidecar).unwrap(),
        "permanent sidecar"
    );
}

fn mixed() {
    add(ROLE, entry(999, 9)).unwrap();
    for i in 0..6 {
        add(ROLE, entry(100 + i, i)).unwrap();
    }
    let mut children = group("mixed_worker", 6);
    for child in &mut children {
        child.success();
    }
    let mut want = vec![(999, 9), (777, 0)];
    want.extend((0..6).map(|i| (200 + i, i)));
    want.extend((0..6).map(|i| (888, i)));
    assert_eq!(identities(), expected(want));
    let shared = load(ROLE)
        .unwrap()
        .into_iter()
        .find(|e| e.requirement_id == uuid::Uuid::from_u128(777))
        .unwrap();
    assert!((0..6).any(|i| shared.note == Some(i.to_string())));
    assert!(remove(
        ROLE,
        &uuid::Uuid::from_u128(888),
        Some(&entry(888, 2).project_root)
    )
    .unwrap());
    assert!(!identities().contains(&(uuid::Uuid::from_u128(888), entry(888, 2).project_root)));
    assert_eq!(
        load(ROLE)
            .unwrap()
            .iter()
            .filter(|e| e.requirement_id == uuid::Uuid::from_u128(888))
            .count(),
        5
    );
    assert!(remove(ROLE, &uuid::Uuid::from_u128(888), None).unwrap());
    assert!(!load(ROLE)
        .unwrap()
        .iter()
        .any(|e| e.requirement_id == uuid::Uuid::from_u128(888)));
    let path = queue_path(ROLE).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    // A publication observation proves no-op removal did not rewrite even
    // byte-identical YAML (mtime alone is too coarse on some filesystems).
    let observation = control().join("before_publish");
    std::fs::remove_file(&observation).unwrap();
    assert!(!remove(ROLE, &uuid::Uuid::from_u128(123456), None).unwrap());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    assert!(!observation.exists());
}

fn readers() {
    add(ROLE, entry(999, 9)).unwrap();
    let (mut reader, ctl) = spawn(&home(), "reader", 0, "");
    wait(&ctl.join("ready"));
    let mut writers = group("writer", 6);
    for writer in &mut writers {
        writer.success();
    }
    mark(ctl.join("stop"));
    reader.success();
    let reads: usize = std::fs::read_to_string(ctl.join("reads"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(reads > 1);
    assert_eq!(identities().len(), 121);
}

fn killed() {
    for seeded in [false, true] {
        for stage in ["before_publish", "after_publish"] {
            let path = queue_path(ROLE).unwrap();
            if path.exists() {
                std::fs::remove_file(&path).unwrap();
            }
            if seeded {
                add(ROLE, entry(999, 9)).unwrap();
            }
            let (mut child, ctl) = spawn(&home(), "one", 1, stage);
            wait(&ctl.join(stage));
            let sidecar = path.with_extension("lock");
            assert!(sidecar.is_file());
            // A killed writer may leave a staging file; readers ignore it.
            let orphan = path.with_extension("tmp.dead.fixture");
            std::fs::write(&orphan, "incomplete: [").unwrap();
            child.kill();
            let (mut next, _) = spawn(&home(), "one", 2, "");
            next.success();
            let mut want = vec![(2, 0)];
            if seeded {
                want.push((999, 9));
            }
            if stage == "after_publish" {
                want.push((1, 0));
            }
            assert_eq!(identities(), expected(want));
            assert!(sidecar.is_file());
            assert_eq!(std::fs::read_to_string(&orphan).unwrap(), "incomplete: [");
        }
    }
}

fn errors() {
    let path = queue_path(ROLE).unwrap();
    assert!(load(ROLE).unwrap().is_empty());
    assert!(!remove(ROLE, &uuid::Uuid::nil(), None).unwrap());
    assert!(!path.exists());
    std::fs::write(&path, "[]\n").unwrap();
    assert!(load(ROLE).unwrap().is_empty());
    add(ROLE, entry(999, 9)).unwrap();
    let committed = std::fs::read(&path).unwrap();
    for bad in ["[", "", " \n ", "null", "~", "{}", "text", "[{}]"] {
        std::fs::write(&path, bad).unwrap();
        for error in [
            load(ROLE).unwrap_err(),
            add(ROLE, entry(1, 0)).unwrap_err(),
            remove(ROLE, &uuid::Uuid::nil(), None).unwrap_err(),
        ] {
            let msg = format!("{error:#}");
            assert!(
                msg.contains(ROLE) && msg.contains(&path.display().to_string()),
                "{msg}"
            );
        }
        assert_eq!(std::fs::read(&path).unwrap(), bad.as_bytes());
        std::fs::write(&path, &committed).unwrap();
        let (mut next, _) = spawn(&home(), "one", 999, "");
        next.success();
    }
    // Invalid UTF-8 is a deterministic read_atomic failure, even as root.
    std::fs::write(&path, [0xff]).unwrap();
    assert!(add(ROLE, entry(1, 0)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), [0xff]);
    std::fs::write(&path, &committed).unwrap();
    let lock = path.with_extension("lock");
    std::fs::remove_file(&lock).unwrap();
    std::fs::create_dir(&lock).unwrap();
    assert!(format!("{:#}", add(ROLE, entry(1, 0)).unwrap_err()).contains("lock"));
    assert_eq!(std::fs::read(&path).unwrap(), committed);
    std::fs::remove_dir(&lock).unwrap();

    // Exercise the actual helper's staging and rename failures without chmod,
    // a helper rewrite, or relying on a platform's permissions for open files.
    let guard = QueueMutation::acquire(ROLE).unwrap();
    let backup_dir = path.parent().unwrap().join("saved");
    std::fs::create_dir(&backup_dir).unwrap();
    let backup = backup_dir.join("committed");
    std::fs::rename(&path, &backup).unwrap();
    std::fs::create_dir(&path).unwrap(); // replacement cannot rename over a directory
    assert!(guard.save(&[entry(1, 0)]).is_err());
    assert_eq!(std::fs::read(&backup).unwrap(), committed);
    assert!(!std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().contains(".tmp.")));
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(&backup, &path).unwrap();
    drop(guard);

    // Staging target is a directory for every possible sequence used in this
    // fresh child. Failed cleanup cannot remove a directory; assert it only
    // for the rename case above, where cleanup can succeed.
    let mut blockers = Vec::new();
    for seq in 0..1024 {
        let p = path.with_extension(format!("tmp.{}.{}", std::process::id(), seq));
        std::fs::create_dir(&p).unwrap();
        blockers.push(p);
    }
    assert!(add(ROLE, entry(1, 0)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), committed);
    for p in blockers {
        std::fs::remove_dir(p).unwrap();
    }
    // This is a single-test child, so changing this test-only hook cannot
    // race another test or change a production home resolver.
    std::env::set_var("BUG_1682_PANIC", "1");
    assert!(std::panic::catch_unwind(|| add(ROLE, entry(1, 0))).is_err());
    std::env::remove_var("BUG_1682_PANIC");
    assert_eq!(std::fs::read(&path).unwrap(), committed);
    let (mut next, _) = spawn(&home(), "one", 2, "");
    next.success();
    assert!(identities().contains(&(uuid::Uuid::from_u128(2), entry(2, 0).project_root)));
}

fn callers() {
    use crate::autopilot_audit::{apply_reversal, ReversalPlan, ReversalStep};
    let project = home().join("project");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    let store_path = project.join(".aida-store");
    let backend = aida_core::GitBackend::new(&store_path)
        .unwrap()
        .with_auto_commit(false);
    let mut req = aida_core::Requirement::new("fixture".into(), String::new());
    req.spec_id = Some("TASK-1".into());
    let mut store = aida_core::RequirementsStore::default();
    store.requirements.push(req);
    backend.save(&store).unwrap();
    let storage = crate::Storage::new(&store_path);
    let path = queue_path(ROLE).unwrap();
    let corrupt = "broken: [";
    std::fs::write(&path, corrupt).unwrap();
    for top in [false, true] {
        let cmd = crate::QueueCommand::Add {
            id: "TASK-1".into(),
            top,
            bottom: false,
            user: None,
            note: None,
            r#for: Some(ROLE.into()),
            scope: None,
            for_session: None,
            no_scope: true,
            global: true,
            force: false,
        };
        let error =
            crate::queue_cmd::handle_queue_command(&cmd, &storage, &store_path).unwrap_err();
        assert!(
            format!("{error:#}").contains("Failed to read global queue"),
            "{error:#}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), corrupt);
    }
    let cmd = crate::QueueCommand::Remove {
        id: "TASK-1".into(),
        user: None,
        global: true,
        r#for: Some(ROLE.into()),
    };
    let error = crate::queue_cmd::handle_queue_command(&cmd, &storage, &store_path).unwrap_err();
    assert!(
        format!("{error:#}").contains("Failed to read global queue"),
        "{error:#}"
    );
    let plan = ReversalPlan {
        target: "test".into(),
        spec_id: "TASK-1".into(),
        complete: true,
        steps: vec![ReversalStep::QueueAdd {
            spec_id: "TASK-1".into(),
            role: ROLE.into(),
        }],
    };
    let error = apply_reversal(&project, &plan, false).unwrap_err();
    assert!(
        format!("{error:#}").contains("Failed to read global queue"),
        "{error:#}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), corrupt);
    assert!(
        !control().join("before_lock").exists(),
        "pre-read must fail before mutation"
    );
}

#[test]
fn bug_1682_process_child() {
    let Ok(action) = std::env::var("BUG_1682_ACTION") else {
        return;
    };
    // Every re-exec, including nested workers, checks the exact same HOME and
    // real-home fence before even asking queue_path to create directories.
    assert_eq!(
        std::env::var_os("AIDA_LIB_TEST_REAL_HOMES"),
        std::env::var_os("BUG_1682_EXPECTED_FENCE")
    );
    assert_eq!(crate::home_dir().unwrap(), home());
    crate::test_home::assert_hermetic(&home());
    assert_eq!(
        queue_path(ROLE).unwrap(),
        home().join(".aida/queue/implementer.yaml")
    );
    let i: usize = std::env::var("BUG_1682_INDEX").unwrap().parse().unwrap();
    match action.as_str() {
        "adds" => adds(),
        "sidecar" => sidecar(),
        "mixed" => mixed(),
        "readers" => readers(),
        "killed" => killed(),
        "errors" => errors(),
        "callers" => callers(),
        "fence" => {
            let (mut child, _) = spawn(&home(), "one", 0, "");
            child.success();
        }
        "one" => add(ROLE, entry(i, 0)).unwrap(),
        "other_role" => add("reviewer", entry(i, 0)).unwrap(),
        "writer" => {
            barrier();
            for j in 0..20 {
                add(ROLE, entry(i * 20 + j, i)).unwrap();
            }
        }
        "mixed_worker" => {
            barrier();
            assert!(remove(
                ROLE,
                &uuid::Uuid::from_u128((100 + i) as u128),
                Some(&entry(100 + i, i).project_root)
            )
            .unwrap());
            add(ROLE, entry(200 + i, i)).unwrap();
            add(ROLE, entry(888, i)).unwrap();
            for _ in 0..10 {
                let mut shared = entry(777, 0);
                shared.note = Some(i.to_string());
                add(ROLE, shared).unwrap();
            }
        }
        "reader" => {
            let end = Instant::now() + DEADLINE;
            let mut count = 0;
            loop {
                assert!(identities()
                    .contains(&(uuid::Uuid::from_u128(999), entry(999, 9).project_root)));
                count += 1;
                if count == 1 {
                    mark(control().join("ready"));
                }
                if control().join("stop").exists() {
                    break;
                }
                assert!(Instant::now() < end, "reader deadline");
            }
            std::fs::write(control().join("reads"), count.to_string()).unwrap();
        }
        _ => panic!("unknown action {action}"),
    }
}

#[test]
fn bug_1682_cross_process_adds_preserve_exact_identity_set() {
    scenario("adds");
}
#[test]
fn bug_1682_sidecar_blocks_before_read_and_roles_are_independent() {
    scenario("sidecar");
}
#[test]
fn bug_1682_mixed_mutations_preserve_scope_and_upsert_uniqueness() {
    scenario("mixed");
}
#[test]
fn bug_1682_readers_observe_complete_seeded_snapshots() {
    scenario("readers");
}
#[test]
fn bug_1682_killed_holder_releases_lock_and_preserves_publication_boundary() {
    scenario("killed");
}
#[test]
fn bug_1682_invalid_documents_and_io_failures_preserve_committed_bytes() {
    scenario("errors");
}
#[test]
fn bug_1682_mutation_callers_propagate_load_errors() {
    scenario("callers");
}
#[test]
fn bug_1682_child_home_is_fenced() {
    scenario("fence");
}
