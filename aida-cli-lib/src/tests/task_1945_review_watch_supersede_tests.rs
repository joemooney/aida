// Head-change supersede: real `sh`/`sleep` fixtures, each in a process group
// the test creates and registers with the signal guard. No LLM, no cargo.
// trace:TASK-1945 | ai:claude
use super::signal::{self, Signal};
use super::*;
use crate::process_retry::RetryEtxtbsy;
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
        behind: Some(0),
        head_branch: None,
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
            .spawn_retrying_etxtbsy()
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
        pid_namespace: crate::agent_registry::current_pid_namespace(),
        recorded_at: None,
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
    // `aida ps` renders the same slots through its own JSON and TOON paths.
    let json = crate::ps_integrate::ps_review_watch_held_json(&held);
    assert_eq!(json[0]["change"], "PR-1");
    assert!(
        json[0]["cause"].as_str().unwrap().contains("ENOSYS"),
        "{json}"
    );
    let toon = crate::ps_integrate::ps_review_watch_held_toon(&held);
    assert!(
        toon.contains("review_watch_held") && toon.contains("ENOSYS"),
        "{toon}"
    );
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

// A held cause names only processes still running: a member that exited
// drops out on the next pass. trace:TASK-1945 | ai:claude
#[test]
fn hold_cause_drops_an_exited_member() {
    let (_temp, store) = fixture();
    let group = Group::spawn(LOOP, &[]);
    let stranger = || {
        Command::new("sleep")
            .arg("60")
            .process_group(group.pgid as i32)
            .spawn()
            .unwrap()
    };
    let (mut gone, mut stays) = (stranger(), stranger());
    let h2 = change(1, 2);
    running(&store, &change(1, 1), &group, &[], true);
    assert!(pass(&store, &h2).is_some());
    until("the dispatcher exits", || !group.identity_alive());
    let diagnostic = || first(&store, "PR-1").supersede.unwrap().diagnostic.unwrap();
    let _ = pass(&store, &h2);
    for pid in [gone.id(), stays.id()] {
        assert!(
            diagnostic().contains(&format!("process {pid} ")),
            "{}",
            diagnostic()
        );
    }
    signal::test_signal(gone.id(), Signal::Kill);
    gone.wait().unwrap();
    let held = pass(&store, &h2).unwrap();
    assert!(
        !diagnostic().contains(&format!("process {} ", gone.id())),
        "{}",
        diagnostic()
    );
    assert!(held.contains(&format!("process {} ", stays.id())), "{held}");
    signal::test_signal(stays.id(), Signal::Kill);
    stays.wait().unwrap();
}

fn bug_2425_foreign_identity(recorded_at: chrono::DateTime<chrono::Utc>) -> Identity {
    Identity {
        pid_namespace: Some(crate::agent_registry::current_pid_namespace().unwrap_or(0) ^ 1),
        recorded_at: Some(recorded_at),
        pid: u32::MAX - 1,
        start: "linux-starttime:1".into(),
        boot: boot_id().unwrap(),
    }
}

// A foreign pid namespace is not evidence of death: inside its backstop the
// process keeps its slot and is never signalled.
// trace:BUG-2425 | ai:codex
#[test]
fn bug_2425_foreign_watch_process_retains_capacity_and_refuses_signals() {
    let identity = bug_2425_foreign_identity(chrono::Utc::now());
    assert!(
        identity.alive(),
        "an invisible process must retain watch capacity"
    );
    for sig in [None, Some(signal::Signal::Term), Some(signal::Signal::Kill)] {
        let holds = signal::signal_attempt(&[&identity], Some(1), Some(&identity), sig);
        assert_eq!(holds.len(), 1);
        assert!(holds[0].contains("foreign pidns"));
    }
}

// The backstop is independent evidence: past its TTL, or after a reboot, a
// foreign-namespace process is gone, frees its slot and is not held.
// trace:BUG-2425 | ai:claude
#[test]
fn bug_2425_foreign_watch_process_past_its_backstop_is_released() {
    let now = chrono::Utc::now();
    let expired = bug_2425_foreign_identity(now - chrono::Duration::hours(5));
    assert!(
        !expired.alive(),
        "an expired foreign record must not wedge its slot"
    );
    let held = signal::signal_attempt(&[&expired], Some(1), Some(&expired), None);
    assert!(
        held.iter().all(|h| !h.contains("foreign pidns")),
        "{held:?}"
    );
    let fresh = bug_2425_foreign_identity(now);
    assert!(!fresh.within_foreign_backstop(now, Some("another-boot")));
    assert!(fresh.within_foreign_backstop(now, Some(fresh.boot.as_str())));
}

// Advisor ruling (PR #3298 round 4): `recorded_at` is observation metadata,
// not identity. Two reads of the same process taken at different instants
// must compare equal, or every self-identity check in the dispatcher fails.
// trace:BUG-2425 | ai:claude
#[test]
fn bug_2425_identity_equality_ignores_recorded_at() {
    let first = Identity::current().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let second = Identity::current().unwrap();
    assert_ne!(first.recorded_at, second.recorded_at);
    assert_eq!(first, second);
    let mut other_process = second.clone();
    other_process.pid ^= 1;
    assert_ne!(first, other_process);
}

// trace:TASK-3365 | ai:codex
#[test]
fn task_3365_watch_shared_identity_reuse_and_missing_evidence() {
    use aida_core::proc_identity::{DeadReason, Verdict};
    let current = Identity::current().unwrap();
    assert_eq!(current.verdict(), Verdict::Alive);
    let mut reused = current.clone();
    reused.start = format!(
        "linux-starttime:{}",
        current.process_identity().unwrap().starttime + 1
    );
    assert_eq!(reused.verdict(), Verdict::Dead(DeadReason::Reused));
    assert!(!reused.alive());
    let mut incomplete = current.clone();
    incomplete.start = "malformed".into();
    assert!(matches!(incomplete.verdict(), Verdict::ForeignUnknown(_)));
    assert!(
        incomplete.alive(),
        "unreadable identity must retain capacity"
    );
    let mut legacy = current.clone();
    legacy.pid_namespace = None;
    legacy.recorded_at = None;
    assert!(matches!(legacy.verdict(), Verdict::ForeignUnknown(_)));
    assert!(legacy.alive());
    let encoded = serde_json::to_string(&current).unwrap();
    assert_eq!(serde_json::from_str::<Identity>(&encoded).unwrap(), current);
}

// trace:TASK-3365 | ai:codex
#[test]
fn task_3365_legacy_capacity_uses_existing_reboot_and_ttl_backstop() {
    let mut child = Command::new("/bin/true").spawn().unwrap();
    let mut legacy = Identity::current().unwrap();
    legacy.pid = child.id();
    child.wait().unwrap();
    legacy.pid_namespace = None;
    legacy.recorded_at =
        Some(chrono::Utc::now() - FOREIGN_IDENTITY_TTL - chrono::Duration::seconds(1));
    assert!(
        !legacy.alive(),
        "dead legacy receipt must expire by existing TTL"
    );
    legacy.recorded_at = None;
    legacy.boot = "11111111-1111-4111-8111-111111111111".into();
    assert!(
        !legacy.alive(),
        "reboot frees even an undated legacy receipt"
    );
    legacy.boot = boot_id().unwrap();
    legacy.start = "unparseable".into();
    legacy.recorded_at =
        Some(chrono::Utc::now() - FOREIGN_IDENTITY_TTL - chrono::Duration::seconds(1));
    assert!(
        !legacy.alive(),
        "malformed start evidence also uses the backstop"
    );
}

// trace:TASK-3365 | ai:codex
#[test]
fn task_3365_caller_capture_refusal_is_recoverable() {
    for why in [
        "masked process view",
        "unverified procfs mount",
        "procfs uses ancestor coordinates",
        "hidepid",
    ] {
        let diagnostic = Identity::current_from(Err(why.into()))
            .unwrap_err()
            .to_string();
        assert!(diagnostic.contains(why));
        assert!(diagnostic.contains("retry from a host"));
        assert!(diagnostic.contains("existing reservations remain held"));
    }
}

// An orphan remains in the review's group, but adoption by a subreaper
// outside the recorded tree supplies no signal authority.
// trace:TASK-3365 | ai:codex
#[test]
fn task_3365_subreaper_adoption_does_not_prove_group_ancestry() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    // SAFETY: this exact-test child alone changes its own subreaper state.
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
        0
    );
    let mut group = Group::spawn(
        "sleep 60 & echo $! > orphan; while [ ! -f release ]; do sleep 0.01; done",
        &["orphan"],
    );
    let anchor = group.identity("dispatcher");
    let orphan = group.pid("orphan").unwrap();
    std::fs::write(group.dir.path().join("release"), "go").unwrap();
    group.child.wait().unwrap();
    until("orphan adopted by subreaper", || {
        std::fs::read_to_string(format!("/proc/{orphan}/status")).is_ok_and(|s| {
            s.lines().any(|line| {
                line.strip_prefix("PPid:")
                    .is_some_and(|p| p.trim() == std::process::id().to_string())
            })
        })
    });
    let held = signal::signal_attempt(
        &[&anchor],
        Some(group.pgid),
        Some(&anchor),
        Some(Signal::Kill),
    );
    assert!(
        held.iter().any(|s| s.contains("cannot be proved ours")),
        "{held:?}"
    );
    assert!(running_pid(orphan), "adopted group member was signalled");
    group.kill_all();
    // SAFETY: reap only the adopted fixture child, after the guarded kill.
    assert_eq!(
        unsafe { libc::waitpid(orphan as i32, std::ptr::null_mut(), 0) },
        orphan as i32
    );
}
