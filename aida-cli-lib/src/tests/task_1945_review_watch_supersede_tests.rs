// Head-change supersede: real `sh`/`sleep` fixtures, each in a process group
// the test creates and registers with the signal guard. No LLM, no cargo.
// trace:TASK-1945 | ai:claude
use super::signal::{self, Signal};
use super::*;
use std::os::unix::process::CommandExt;
use std::time::{Duration, Instant};

const REPO: &str = "github:github.com/owner/repo";

fn fixture() -> (tempfile::TempDir, Store) {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path(), REPO).unwrap();
    (temp, store)
}
fn change(id: u64, head: u64) -> forge::Change {
    forge::Change {
        id,
        head: format!("{head:040x}"),
        base: "main".into(),
        author: "writer".into(),
        gitlab: false,
        overlap: Some(false),
    }
}
fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
// The module is Linux-only (see its `mod`); the item attributes repeat that
// for the portability ratchet, which scans this file alone.
#[cfg(target_os = "linux")]
fn running_pid(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat[stat.rfind(')').unwrap() + 1..]
            .split_whitespace()
            .next()
            != Some("Z")
    })
}

/// A shell script running as the leader of its own process group, standing
/// in for the dispatcher. Named pids it writes are its descendants.
struct Group {
    dir: tempfile::TempDir,
    child: Child,
    pgid: u32,
}
impl Group {
    fn spawn(script: &str, names: &[&str]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dispatcher");
        crate::test_exec::write_executable(&path, format!("#!/bin/sh\n{script}\n"));
        let child = Command::new(&path)
            .current_dir(dir.path())
            .stdin(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let pgid = child.id();
        signal::allow_test_group(pgid);
        let group = Self { dir, child, pgid };
        for name in names {
            until(name, || group.pid(name).is_some());
        }
        group
    }
    fn pid(&self, name: &str) -> Option<u32> {
        std::fs::read_to_string(self.dir.path().join(name))
            .ok()
            .and_then(|s| s.trim().parse().ok())
    }
    fn identity(&self, name: &str) -> Identity {
        let pid = if name == "dispatcher" {
            self.child.id()
        } else {
            self.pid(name).unwrap()
        };
        Identity::of(pid).unwrap()
    }
    fn terms(&self) -> usize {
        std::fs::read_to_string(self.dir.path().join("count"))
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }
    fn identity_alive(&self) -> bool {
        running_pid(self.child.id())
    }
    fn empty(&self) -> bool {
        signal::group_members(self.pgid).unwrap().is_empty()
    }
    fn kill_all(&self) {
        for pid in signal::group_members(self.pgid).unwrap_or_default() {
            signal::test_signal(pid, Signal::Kill);
        }
    }
}
impl Drop for Group {
    fn drop(&mut self) {
        self.kill_all();
        let _ = self.child.wait();
        until("fixture group exits", || self.empty());
        signal::forget_test_group(self.pgid);
    }
}

const TREE: &str = "sh -c 'sleep 60 & echo $! > reviewer; wait' & echo $! > worker; wait";
const LOOP: &str = "while :; do sleep 0.05; done";
const IGNORE_TERM: &str = "trap '' TERM; while :; do sleep 0.05; done";
const COUNT_TERM: &str = "trap 'echo t >> count' TERM; while :; do sleep 0.05; done";

fn reserve(store: &Store, c: &forge::Change) -> Attempt {
    match select(store, c, None, 2).unwrap() {
        Action::Dispatch(attempt) => attempt,
        other => panic!("expected dispatch: {other:?}"),
    }
}
fn edit(store: &Store, key: &str, f: impl FnOnce(&mut Attempt)) {
    store
        .transaction(key, |state| {
            f(&mut state.entries.get_mut(key).unwrap().attempts[0]);
            Ok(())
        })
        .unwrap();
}
/// A claimed review at `c.head` carried by the fixture's processes.
fn running(store: &Store, c: &forge::Change, group: &Group, names: &[&str], pgid: bool) {
    reserve(store, c);
    let dispatcher = group.identity("dispatcher");
    let worker = names.contains(&"worker").then(|| group.identity("worker"));
    let reviewer = names
        .contains(&"reviewer")
        .then(|| group.identity("reviewer"));
    edit(store, &c.key(), |a| {
        a.phase = Phase::Running;
        a.identity = Some(dispatcher.clone());
        a.dispatcher = Some(dispatcher);
        a.spawning = worker.is_some();
        a.worker = worker;
        a.reviewer_spawning = reviewer.is_some();
        a.reviewer = reviewer;
        a.pgid = pgid.then_some(group.pgid);
    });
}
fn pass(store: &Store, c: &forge::Change) -> Option<String> {
    supersede(store, c, || Ok(c.head.clone())).unwrap()
}
fn first(store: &Store, key: &str) -> Attempt {
    store.load().unwrap().entries[key].attempts[0].clone()
}
fn intent(store: &Store, key: &str, by: &forge::Change) {
    edit(store, key, |a| {
        a.phase = Phase::Superseding;
        a.supersede = Some(Supersede {
            by: by.head.clone(),
            requested_at: chrono::Utc::now(),
            term_at: None,
            kill_at: None,
            diagnostic: None,
        });
    });
}

// trace:TASK-1945 | ai:claude
#[test]
fn supersede_signals_recorded_group_and_admits_new_head() {
    let (_temp, store) = fixture();
    let group = Group::spawn(TREE, &["worker", "reviewer"]);
    let (h1, h2) = (change(1, 1), change(1, 2));
    running(&store, &h1, &group, &["worker", "reviewer"], true);
    let ids = ["dispatcher", "worker", "reviewer"].map(|n| group.identity(n));
    let held = pass(&store, &h2).expect("held until its processes exit");
    assert!(held.contains("superseded for head"), "{held}");
    let a = first(&store, "PR-1");
    assert_eq!(a.phase, Phase::Superseding);
    assert!(a.supersede.as_ref().unwrap().term_at.is_some());
    assert!(matches!(
        select(&store, &h2, None, 2).unwrap(),
        Action::Skip(_)
    ));
    until("all three exit on SIGTERM", || {
        ids.iter().all(|i| !i.alive())
    });
    assert_eq!(pass(&store, &h2), None);
    assert_eq!(first(&store, "PR-1").phase, Phase::Superseded);
    let admitted = match select(&store, &h2, None, 2).unwrap() {
        Action::Dispatch(attempt) => attempt,
        other => panic!("H2 not admitted: {other:?}"),
    };
    assert_eq!(admitted.head, h2.head);
    assert_eq!(first(&store, "PR-1").phase, Phase::Reserved);
}

// trace:TASK-1945 | ai:claude
#[test]
fn reused_pid_never_signalled() {
    let (_temp, store) = fixture();
    let group = Group::spawn(LOOP, &[]);
    let c = change(1, 1);
    running(&store, &c, &group, &[], false);
    // The recorded PID now names a different process.
    edit(&store, "PR-1", |a| {
        for i in [&mut a.identity, &mut a.dispatcher].into_iter().flatten() {
            i.start = "linux-starttime:1".into();
        }
    });
    assert_eq!(pass(&store, &change(1, 2)), None);
    assert_eq!(first(&store, "PR-1").phase, Phase::Superseded);
    std::thread::sleep(Duration::from_millis(200));
    assert!(running_pid(group.child.id()), "a reused PID was signalled");
}

// trace:TASK-1945 | ai:claude
#[test]
fn pgrp_mismatch_holds_without_signal() {
    let (_temp, store) = fixture();
    let group = Group::spawn(LOOP, &[]);
    // A real, now-empty process group number that is not the fixture's.
    let other = Group::spawn(LOOP, &[]);
    let empty_pgid = other.pgid;
    drop(other);
    let c = change(1, 1);
    running(&store, &c, &group, &[], true);
    edit(&store, "PR-1", |a| a.pgid = Some(empty_pgid));
    let held = pass(&store, &change(1, 2)).unwrap();
    assert!(held.contains("not the recorded"), "{held}");
    std::thread::sleep(Duration::from_millis(200));
    assert!(running_pid(group.child.id()));
    assert_eq!(first(&store, "PR-1").phase, Phase::Superseding);
    assert!(matches!(
        select(&store, &change(1, 2), None, 2).unwrap(),
        Action::Skip(_)
    ));
}

// trace:TASK-1945 | ai:claude
#[test]
fn term_ignoring_child_gets_kill_after_grace() {
    let (_temp, store) = fixture();
    let group = Group::spawn(IGNORE_TERM, &[]);
    let h2 = change(1, 2);
    running(&store, &change(1, 1), &group, &[], true);
    let started = Instant::now();
    assert!(pass(&store, &h2).is_some());
    if started.elapsed() < SUPERSEDE_GRACE {
        assert!(pass(&store, &h2).is_some());
        let receipt = first(&store, "PR-1").supersede.unwrap();
        assert!(receipt.kill_at.is_none(), "killed before the grace");
    }
    assert!(running_pid(group.child.id()), "TERM was ignored");
    assert!(matches!(
        select(&store, &h2, None, 2).unwrap(),
        Action::Skip(_)
    ));
    std::thread::sleep(SUPERSEDE_GRACE + Duration::from_millis(100));
    assert!(pass(&store, &h2).is_some());
    assert!(first(&store, "PR-1").supersede.unwrap().kill_at.is_some());
    until("SIGKILL ends the group", || group.empty());
    assert_eq!(pass(&store, &h2), None);
    assert!(matches!(
        select(&store, &h2, None, 2).unwrap(),
        Action::Dispatch(_)
    ));
}

// trace:TASK-1945 | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn superseding_blocks_new_descendants() {
    let (temp, store) = fixture();
    let c = change(1, 1);
    let attempt = reserve(&store, &c);
    store
        .transition("PR-1", &attempt.token, Phase::Running)
        .unwrap();
    edit(&store, "PR-1", |a| a.spawning = true);
    intent(&store, "PR-1", &change(1, 2));
    assert!(store.record_worker("PR-1", &attempt.token, None).is_err());
    assert!(store.record_reviewer("PR-1", &c.head, None).is_err());
    LAUNCH.with(|l| {
        *l.borrow_mut() = Some((
            temp.path().to_path_buf(),
            REPO.into(),
            "PR-1".into(),
            c.head.clone(),
        ))
    });
    let marker = temp.path().join("reviewer-ran");
    let status = reviewer_status(
        Command::new("/bin/sh").args(["-c", &format!("touch {}", marker.display())]),
    );
    LAUNCH.with(|l| *l.borrow_mut() = None);
    assert!(status.is_err());
    assert!(!marker.exists(), "a reviewer spawned after the intent");
    let a = first(&store, "PR-1");
    assert!(a.worker.is_none() && a.reviewer.is_none());
}

// trace:TASK-1945 | ai:claude
#[test]
fn unrecorded_descendant_holds_while_group_live() {
    let (_temp, store) = fixture();
    // Queue work is spawning but unrecorded; it ignores TERM and outlives
    // the dispatcher.
    let group = Group::spawn(
        "sh -c \"trap '' TERM; while :; do sleep 0.05; done\" & echo $! > member; wait",
        &["member"],
    );
    let h2 = change(1, 2);
    running(&store, &change(1, 1), &group, &[], true);
    edit(&store, "PR-1", |a| a.spawning = true);
    let member = group.pid("member").unwrap();
    assert!(pass(&store, &h2).is_some());
    until("the dispatcher exits", || !group.identity_alive());
    let held = pass(&store, &h2).unwrap();
    assert!(held.contains("still running"), "{held}");
    assert!(running_pid(member));
    assert_eq!(first(&store, "PR-1").phase, Phase::Superseding);
    group.kill_all();
    until("the group empties", || group.empty());
    assert_eq!(pass(&store, &h2), None);
    assert_eq!(first(&store, "PR-1").phase, Phase::Superseded);
}

// trace:TASK-1945 | ai:claude
#[test]
fn restart_after_intent_is_idempotent() {
    let (temp, store) = fixture();
    let group = Group::spawn(COUNT_TERM, &[]);
    let h2 = change(1, 2);
    running(&store, &change(1, 1), &group, &[], true);
    intent(&store, "PR-1", &h2);
    drop(store);
    let reopen = || Store::open(temp.path(), REPO).unwrap();
    assert!(
        pass(&reopen(), &h2).is_some(),
        "the intent resumes with SIGTERM"
    );
    assert!(first(&reopen(), "PR-1")
        .supersede
        .unwrap()
        .term_at
        .is_some());
    // Under load the second pass may fall past the grace and kill the
    // fixture; let the trap record first.
    until("the trap records SIGTERM", || group.terms() >= 1);
    let _ = pass(&reopen(), &h2);
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(group.terms(), 1, "SIGTERM sent once across restarts");
    group.kill_all();
    until("the group empties", || group.empty());
    assert_eq!(pass(&reopen(), &h2), None);
    assert!(matches!(
        select(&reopen(), &h2, None, 2).unwrap(),
        Action::Dispatch(_)
    ));
    assert!(matches!(
        select(&reopen(), &h2, None, 2).unwrap(),
        Action::Skip(_)
    ));
    let state = reopen().load().unwrap();
    assert_eq!(
        state.entries["PR-1"]
            .attempts
            .iter()
            .filter(|a| a.head == h2.head)
            .count(),
        1
    );
    assert_eq!(group.terms(), 1);
}

// trace:TASK-1945 | ai:claude
#[test]
fn concurrent_watchers_signal_once() {
    let (temp, store) = fixture();
    let group = Group::spawn(COUNT_TERM, &[]);
    let h2 = change(1, 2);
    running(&store, &change(1, 1), &group, &[], true);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let watchers: Vec<_> = (0..2)
        .map(|_| {
            let (root, barrier, h2) = (temp.path().to_path_buf(), barrier.clone(), h2.clone());
            std::thread::spawn(move || {
                let store = Store::open(&root, REPO).unwrap();
                barrier.wait();
                match supersede(&store, &h2, || Ok(h2.head.clone())) {
                    Ok(_) => {}
                    Err(error) => assert!(is_busy(&error), "{error:#}"),
                }
            })
        })
        .collect();
    for watcher in watchers {
        watcher.join().unwrap();
    }
    let receipt = first(&store, "PR-1").supersede.unwrap();
    assert!(receipt.term_at.is_some());
    // A loser that waited out the grace on the lock claims SIGKILL instead,
    // which may end the fixture before its trap writes; never a second TERM.
    if receipt.kill_at.is_none() {
        until("the trap records SIGTERM", || group.terms() >= 1);
    }
    std::thread::sleep(Duration::from_millis(150));
    assert!(group.terms() <= 1, "two watchers signalled twice");
}

// trace:TASK-1945 | ai:claude
#[test]
fn dispatcher_terminal_write_keeps_superseding() {
    let (_temp, store) = fixture();
    let c = change(1, 1);
    let attempt = reserve(&store, &c);
    store
        .transition("PR-1", &attempt.token, Phase::Running)
        .unwrap();
    intent(&store, "PR-1", &change(1, 2));
    store
        .transition_with(
            "PR-1",
            &attempt.token,
            Phase::Exited,
            Some("queue work exited signal: 15".into()),
        )
        .unwrap();
    store
        .transition("PR-1", &attempt.token, Phase::Completed)
        .unwrap();
    let a = first(&store, "PR-1");
    assert_eq!(a.phase, Phase::Superseding);
    assert_eq!(
        a.diagnostic.as_deref(),
        Some("queue work exited signal: 15")
    );
    assert!(active(&a), "a terminal write freed the slot");
    assert!(store
        .transition("PR-1", &attempt.token, Phase::Running)
        .is_err());
}

// trace:TASK-1945 | ai:claude
#[test]
fn superseding_holds_capacity() {
    let (_temp, store) = fixture();
    match select(&store, &change(1, 1), None, 1).unwrap() {
        Action::Dispatch(_) => {}
        other => panic!("{other:?}"),
    }
    intent(&store, "PR-1", &change(1, 2));
    let Action::Skip(reason) = select(&store, &change(2, 3), None, 1).unwrap() else {
        panic!("PR-2 admitted over a superseding slot");
    };
    assert!(reason.contains("capacity"), "{reason}");
    assert!(reason.contains("superseded for head"), "{reason}");
}

// trace:TASK-1945 | ai:claude
#[test]
fn release_refused_while_superseding_process_alive() {
    let (_temp, store) = fixture();
    let group = Group::spawn(LOOP, &[]);
    running(&store, &change(1, 1), &group, &[], true);
    intent(&store, "PR-1", &change(1, 2));
    assert!(store.release("PR-1").is_err());
    group.kill_all();
    until("the group empties", || group.empty());
    assert_eq!(store.release("PR-1").unwrap(), 1);
}

// trace:TASK-1945 | ai:claude
#[test]
fn supersede_touches_no_lease_or_worktree() {
    let (temp, store) = fixture();
    let files = [
        ".aida/sessions/reviewer-lease.json",
        ".aida/merge-holds/PR-1",
        ".aida/review-verdicts/PR-1.json",
        "worktrees/aida-pr-1/src/lib.rs",
    ];
    for (i, file) in files.iter().enumerate() {
        let path = temp.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // The verdict file must stay readable history: an approval of an
        // unrelated head consumes no round.
        let body = if file.ends_with(".json") && file.contains("verdicts") {
            serde_json::json!({"reviewed_sha": format!("{:040x}", 9), "recorded_by": "reviewer seat", "verdict": "approve"}).to_string()
        } else {
            format!("fixture {i}\n")
        };
        std::fs::write(&path, body).unwrap();
    }
    let snapshot = || {
        files
            .iter()
            .map(|f| std::fs::read(temp.path().join(f)).unwrap())
            .collect::<Vec<_>>()
    };
    let before = snapshot();
    let group = Group::spawn(TREE, &["worker", "reviewer"]);
    let h2 = change(1, 2);
    running(&store, &change(1, 1), &group, &["worker", "reviewer"], true);
    assert!(pass(&store, &h2).is_some());
    until("the review exits", || {
        group.empty() || !group.identity_alive()
    });
    until("confirmed", || pass(&store, &h2).is_none());
    assert_eq!(first(&store, "PR-1").phase, Phase::Superseded);
    assert_eq!(snapshot(), before);
}

// trace:TASK-1945 | ai:claude
#[test]
fn legacy_state_without_pgid_loads() {
    let (temp, store) = fixture();
    let group = Group::spawn("sleep 60 & echo $! > member; wait", &["member"]);
    let h2 = change(1, 2);
    running(&store, &change(1, 1), &group, &[], false);
    let raw = std::fs::read_to_string(temp.path().join(".aida/review-watch/state.json")).unwrap();
    assert!(!raw.contains("pgid") && !raw.contains("supersede"), "{raw}");
    let store = Store::open(temp.path(), REPO).unwrap();
    store.load().unwrap();
    assert!(pass(&store, &h2).is_some());
    until("the dispatcher exits", || !group.identity_alive());
    let member = group.pid("member").unwrap();
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        running_pid(member),
        "no group sweep without a recorded pgid"
    );
    assert_eq!(pass(&store, &h2), None);
    assert_eq!(first(&store, "PR-1").phase, Phase::Superseded);
}

// trace:TASK-1945 | ai:claude
#[test]
fn unprovable_group_member_not_signalled() {
    let (_temp, store) = fixture();
    let group = Group::spawn(LOOP, &[]);
    // In the group, but a child of the test process, not of the review.
    let mut stranger = Command::new("sleep")
        .arg("60")
        .process_group(group.pgid as i32)
        .spawn()
        .unwrap();
    let h2 = change(1, 2);
    running(&store, &change(1, 1), &group, &[], true);
    assert!(pass(&store, &h2).is_some());
    until("the dispatcher exits", || !group.identity_alive());
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        running_pid(stranger.id()),
        "an unprovable member was signalled"
    );
    let held = pass(&store, &h2).unwrap();
    assert!(held.contains("cannot be proved ours"), "{held}");
    assert_eq!(first(&store, "PR-1").phase, Phase::Superseding);
    signal::test_signal(stranger.id(), Signal::Kill);
    stranger.wait().unwrap();
}

// C1: no pidfd (ENOSYS) writes the intent and holds without signalling.
// trace:TASK-1945 | ai:claude
#[test]
fn enosys_holds_and_never_signals() {
    let (_temp, store) = fixture();
    let group = Group::spawn(LOOP, &[]);
    running(&store, &change(1, 1), &group, &[], true);
    signal::NO_PIDFD.with(|c| c.set(true));
    let held = pass(&store, &change(1, 2));
    signal::NO_PIDFD.with(|c| c.set(false));
    let held = held.unwrap();
    assert!(held.contains("ENOSYS"), "{held}");
    std::thread::sleep(Duration::from_millis(150));
    assert!(running_pid(group.child.id()));
    let a = first(&store, "PR-1");
    assert_eq!(a.phase, Phase::Superseding);
    assert!(a.supersede.unwrap().term_at.is_some());
}

// C2: the guard refuses any process outside a test-created group.
// trace:TASK-1945 | ai:claude
#[test]
#[should_panic(expected = "was not created by the test")]
fn test_signal_guard_refuses_foreign_processes() {
    signal::test_signal(std::process::id(), Signal::Term);
}

// C3: start ticks compare on the same boot only.
// trace:TASK-1945 | ai:claude
#[test]
fn start_order_compares_ticks_on_one_boot() {
    let anchor = Identity {
        pid: 10,
        start: "linux-starttime:100".into(),
        boot: "boot-a".into(),
    };
    assert!(signal::started_not_before(100, "boot-a", &anchor));
    assert!(signal::started_not_before(101, "boot-a", &anchor));
    assert!(!signal::started_not_before(99, "boot-a", &anchor));
    assert!(!signal::started_not_before(500, "boot-b", &anchor));
    let legacy = Identity {
        start: "2026-10-21T00:00:00Z".into(),
        ..anchor
    };
    assert!(!signal::started_not_before(500, "boot-a", &legacy));
}

// C4: the hold cause is visible without reading stderr.
// trace:TASK-1945 | ai:claude
#[test]
fn held_slot_cause_shows_in_capacity_and_ps() {
    let (temp, store) = fixture();
    reserve(&store, &change(1, 1));
    intent(&store, "PR-1", &change(1, 2));
    edit(&store, "PR-1", |a| {
        a.supersede.as_mut().unwrap().diagnostic = Some("pidfd_open is unavailable (ENOSYS)".into())
    });
    let held = held_slots(temp.path());
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].0, "PR-1");
    assert!(held[0].1.contains("ENOSYS"), "{}", held[0].1);
    assert!(held[0].1.contains("aida review watch --release PR-1"));
    let holders = capacity_holders(&store.load().unwrap());
    assert!(holders.contains("ENOSYS"), "{holders}");
    let ps =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ps_integrate.rs"))
            .unwrap();
    assert!(ps.contains("crate::review_watch::held_slots(&project_root)"));
    assert!(ps.contains("\"review_watch_held\""));
}

// A supersede in progress still finishes when the change moves back to the
// superseded head, and that head can then be reviewed again.
// trace:TASK-1945 | ai:claude
#[test]
fn superseding_finishes_when_the_head_returns() {
    let (_temp, store) = fixture();
    let group = Group::spawn(LOOP, &[]);
    let h1 = change(1, 1);
    running(&store, &h1, &group, &[], true);
    intent(&store, "PR-1", &change(1, 2));
    group.kill_all();
    until("the group empties", || group.empty());
    assert!(matches!(
        select(&store, &h1, None, 2).unwrap(),
        Action::Skip(_)
    ));
    assert_eq!(pass(&store, &h1), None);
    assert_eq!(first(&store, "PR-1").phase, Phase::Superseded);
    assert!(matches!(
        select(&store, &h1, None, 2).unwrap(),
        Action::Dispatch(_)
    ));
}
