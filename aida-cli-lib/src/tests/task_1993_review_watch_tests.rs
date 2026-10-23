// trace:TASK-1945 trace:TASK-1993 | ai:codex
use super::*;
use std::cell::Cell;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Barrier};

fn fixture() -> (tempfile::TempDir, Store) {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path(), "github:github.com/owner/repo").unwrap();
    (temp, store)
}
fn change(id: u64, head: u64, overlap: bool) -> forge::Change {
    forge::Change {
        id,
        head: format!("{head:040x}"),
        base: "main".into(),
        author: "writer".into(),
        gitlab: false,
        overlap: Some(overlap),
        behind: Some(u64::from(overlap)),
    }
}
fn dispatch(action: Action) -> Attempt {
    match action {
        Action::Dispatch(attempt) => attempt,
        other => panic!("expected dispatch: {other:?}"),
    }
}
fn complete(store: &Store, c: &forge::Change, attempt: &Attempt) {
    execute_reservation(store, &c.key(), &attempt.token, |_| Ok(()), |_| Ok(())).unwrap();
}
fn verdict_json(head: &str, verdict: &str) -> serde_json::Value {
    serde_json::json!({"reviewed_sha": head, "recorded_by": "reviewer seat", "verdict": verdict})
}
fn write_verdict(path: &Path, value: &serde_json::Value) {
    std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

// trace:TASK-1993.ac9ef854 | ai:claude
#[test]
fn task1993_pending_ci_dispatches_with_receipt_before_work() {
    let (_temp, store) = fixture();
    let value = serde_json::json!({"number": 1, "state": "OPEN", "isDraft": false,
        "headRefOid": "1".repeat(40), "baseRefName": "main", "author": {"login": "writer"},
        "statusCheckRollup": [{"status": "IN_PROGRESS"}]});
    let mut c = forge::parse_change(&value, false).unwrap().unwrap();
    // Base evidence is independent of pending CI; discovery supplies it.
    c.overlap = Some(false);
    let attempt = dispatch(select(&store, &c, Some("claude".into()), 2).unwrap());
    let worked = Cell::new(false);
    execute_reservation(
        &store,
        &c.key(),
        &attempt.token,
        |_| Ok(()),
        |_| {
            let state = store.load()?;
            let receipt = &state.entries[&c.key()].attempts[0];
            assert_eq!(receipt.phase, Phase::Running);
            assert_eq!(receipt.identity.as_ref(), Some(&Identity::current()?));
            worked.set(true);
            Ok(())
        },
    )
    .unwrap();
    assert!(worked.get());
    assert_eq!(
        store.load().unwrap().entries[&c.key()].attempts[0].phase,
        Phase::Completed
    );
    assert_eq!(
        queue_args(&c.key(), Some("claude")),
        [
            "queue",
            "work",
            "PR-1",
            "--role",
            "reviewer",
            "--no-human",
            "--fresh",
            "--vendor",
            "claude"
        ]
    );
}

// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_restart_and_duplicate_child_cannot_repeat_a_head() {
    let (temp, store) = fixture();
    let c = change(1, 1, false);
    let attempt = dispatch(select(&store, &c, None, 2).unwrap());
    let reopened = Store::open(temp.path(), &store.repository).unwrap();
    assert!(matches!(
        select(&reopened, &c, None, 2).unwrap(),
        Action::Skip(_)
    ));
    complete(&store, &c, &attempt);
    execute_reservation(
        &store,
        &c.key(),
        &attempt.token,
        |_| panic!("duplicate preflight"),
        |_| panic!("duplicate work"),
    )
    .unwrap_err();
    assert!(matches!(
        select(&reopened, &c, None, 2).unwrap(),
        Action::Skip(_)
    ));
}

// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_failed_preflight_reconciles_without_starting_review() {
    let (_temp, store) = fixture();
    let c = change(1, 1, false);
    let attempt = dispatch(select(&store, &c, None, 2).unwrap());
    execute_reservation(
        &store,
        &c.key(),
        &attempt.token,
        |_| anyhow::bail!("head moved"),
        |_| panic!("moving head must never start work"),
    )
    .unwrap_err();
    let next = change(1, 2, false);
    let retried = dispatch(select(&store, &next, None, 2).unwrap());
    assert_ne!(retried.token, attempt.token);
    assert_eq!(store.load().unwrap().entries[&c.key()].attempts.len(), 1);
    execute_reservation(
        &store,
        &c.key(),
        &attempt.token,
        |_| panic!("old ticket"),
        |_| panic!("old ticket"),
    )
    .unwrap_err();
}

// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_lost_completion_waits_while_live_and_frees_on_proven_exit() {
    let (_temp, store) = fixture();
    let c = change(1, 1, false);
    let attempt = dispatch(select(&store, &c, None, 2).unwrap());
    // The dispatcher claimed its receipt and has not recorded an outcome.
    store
        .transition(&c.key(), &attempt.token, Phase::Running)
        .unwrap();
    // While its exact process lives, a new head waits.
    assert!(matches!(
        select(&store, &change(1, 2, false), None, 2).unwrap(),
        Action::Skip(_)
    ));
    // A reused PID with another start identity is proof the dispatcher is
    // gone: it can no longer complete, and the slot is freed terminally.
    store
        .transaction(&c.key(), |state| {
            state.entries.get_mut(&c.key()).unwrap().attempts[0]
                .identity
                .as_mut()
                .unwrap()
                .start = "linux-starttime:0".into();
            Ok(())
        })
        .unwrap();
    assert!(store
        .transition(&c.key(), &attempt.token, Phase::Completed)
        .is_err());
    dispatch(select(&store, &change(1, 2, false), None, 2).unwrap());
    let state = store.load().unwrap();
    assert!(
        state.entries[&c.key()]
            .attempts
            .iter()
            .all(|a| a.head != c.head),
        "terminal attempt replaced by the new head"
    );
}

// trace:TASK-1993.ac1e5827 | ai:claude
#[test]
fn task1993_new_head_waits_without_signalling_then_obeys_two_round_cap() {
    let (temp, store) = fixture();
    let verdicts = temp.path().join(".aida/review-verdicts");
    std::fs::create_dir(&verdicts).unwrap();
    let mut rounds = Vec::new();
    // Each watcher review lands an ordinary request-changes verdict.
    let review = |c: &forge::Change, a: &Attempt, rounds: &mut Vec<serde_json::Value>| {
        execute_reservation(
            &store,
            &c.key(),
            &a.token,
            |_| Ok(()),
            |_| {
                let mut current = verdict_json(&c.head, "request-changes");
                current["rounds"] = serde_json::Value::Array(rounds.clone());
                write_verdict(&verdicts.join("PR-1.json"), &current);
                rounds.push(verdict_json(&c.head, "request-changes"));
                Ok(())
            },
        )
        .unwrap();
    };
    let first = change(1, 1, false);
    let a = dispatch(select(&store, &first, None, 2).unwrap());
    let second = change(1, 2, false);
    assert!(matches!(
        select(&store, &second, None, 2).unwrap(),
        Action::Skip(_)
    ));
    review(&first, &a, &mut rounds);
    let b = dispatch(select(&store, &second, None, 2).unwrap());
    review(&second, &b, &mut rounds);
    let Action::Skip(reason) = select(&store, &change(1, 3, false), None, 2).unwrap() else {
        panic!("third round dispatched")
    };
    assert!(reason.contains("two-round"));
}

// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_global_capacity_includes_ambiguous_reservations_and_configuration() {
    let (_temp, store) = fixture();
    let c = change(1, 1, false);
    let a = dispatch(select(&store, &c, None, 1).unwrap());
    assert!(select(&store, &change(2, 2, false), None, 2).is_err());
    assert!(matches!(
        select(&store, &change(2, 2, false), None, 1).unwrap(),
        Action::Skip(_)
    ));
    complete(&store, &c, &a);
    dispatch(select(&store, &change(2, 2, false), None, 2).unwrap());
    dispatch(select(&store, &change(3, 3, false), None, 2).unwrap());
    assert!(matches!(
        select(&store, &change(4, 4, false), None, 2).unwrap(),
        Action::Skip(_)
    ));
}

// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_concurrent_watchers_admit_only_one_change_head() {
    let (temp, store) = fixture();
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let path = temp.path().to_path_buf();
        let repository = store.repository.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            let other = Store::open(&path, &repository).unwrap();
            barrier.wait();
            match select(&other, &change(1, 1, false), None, 2) {
                Ok(Action::Dispatch(_)) => 1,
                Ok(Action::Skip(_)) => 0,
                Err(error)
                    if error.chain().any(|e| {
                        e.downcast_ref::<std::io::Error>()
                            .is_some_and(|e| e.kind() == std::io::ErrorKind::WouldBlock)
                    }) =>
                {
                    0
                }
                result => panic!("unexpected concurrent admission: {result:?}"),
            }
        }));
    }
    barrier.wait();
    assert_eq!(
        handles.into_iter().map(|h| h.join().unwrap()).sum::<u8>(),
        1
    );
    assert_eq!(store.load().unwrap().entries["PR-1"].attempts.len(), 1);
}

// trace:TASK-1993.acb96f0d | ai:claude
#[test]
fn task1993_stale_notice_retry_is_idempotent_and_preserves_reader_state() {
    let (temp, store) = fixture();
    let c = change(1, 1, true);
    let Action::Notify(notice) = select(&store, &c, None, 2).unwrap() else {
        panic!("missing notice")
    };
    assert!(store.load().unwrap().entries[&c.key()].attempts.is_empty());
    let mailbox = aida_core::mailbox::local_mailbox_dir(temp.path());
    std::fs::create_dir(&mailbox).unwrap();
    let shared_directory_mode = 0o775;
    std::fs::set_permissions(
        &mailbox,
        std::fs::Permissions::from_mode(shared_directory_mode),
    )
    .unwrap();
    deliver(temp.path(), &notice).unwrap();
    let path = aida_core::mailbox::local_mailbox_dir(temp.path())
        .join(format!("{}.json", notice.message.id));
    let mut archived = notice.message.clone();
    archived.archived = true;
    std::fs::write(&path, serde_json::to_vec(&archived).unwrap()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o664)).unwrap();
    // Simulate a crash after delivery but before persisting sent=true.
    let Action::Notify(retried) = select(&store, &c, None, 2).unwrap() else {
        panic!("pending delivery lost")
    };
    deliver(temp.path(), &retried).unwrap();
    let kept: aida_core::mailbox::Message =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(kept.archived);
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );
    store
        .transaction(&c.key(), |state| {
            state
                .entries
                .get_mut(&c.key())
                .unwrap()
                .notices
                .get_mut(&c.head)
                .unwrap()
                .sent = true;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        select(&store, &c, None, 2).unwrap(),
        Action::Skip(_)
    ));
}

// trace:TASK-1993.acb96f0d | ai:claude
#[test]
fn task1993_notification_payload_drift_refuses_delivery() {
    let (_temp, store) = fixture();
    let c = change(1, 1, true);
    select(&store, &c, None, 2).unwrap();
    store
        .transaction(&c.key(), |state| {
            state
                .entries
                .get_mut(&c.key())
                .unwrap()
                .notices
                .get_mut(&c.head)
                .unwrap()
                .message
                .from = "advisor".into();
            Ok(())
        })
        .unwrap();
    assert!(select(&store, &c, None, 2).is_err());
}

// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_missing_corrupt_and_unsafe_state_refuse_reinitialization() {
    use std::os::unix::fs::symlink;
    let (_temp, store) = fixture();
    select(&store, &change(1, 1, false), None, 2).unwrap();
    let path = store.dir.path().join(STATE);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(store.load().is_err());
    std::fs::write(&path, b"{\"version\":999}").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(store.load().is_err());
    std::fs::remove_file(&path).unwrap();
    let outside = store.dir.path().join("outside");
    std::fs::write(&outside, &bytes).unwrap();
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&outside, &path).unwrap();
    assert!(store.load().is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::hard_link(&outside, &path).unwrap();
    assert!(store.load().is_err());
    std::fs::remove_file(&outside).unwrap();
    assert!(
        store.load().is_ok(),
        "the surviving singly linked regular state is valid"
    );
}

// trace:TASK-1993.ac4fdc2a | ai:claude
#[test]
fn task1993_strict_discovery_handles_gitlab_and_refuses_unknown_evidence() {
    let mut row = serde_json::json!({"iid": 4, "state": "opened", "draft": false,
        "sha": "a".repeat(40), "target_branch": "release/2", "author": {"username": "writer"}});
    let parsed = forge::parse_change(&row, true).unwrap().unwrap();
    assert_eq!(parsed.key(), "MR-4");
    // The change's own target branch is the base evidence (not origin/HEAD).
    assert_eq!(parsed.base, "release/2");
    for bad in [serde_json::Value::Null, "".into(), "--upload-pack=x".into()] {
        let mut unknown = row.clone();
        unknown["target_branch"] = bad;
        assert!(
            forge::parse_change(&unknown, true).is_err(),
            "unknown base must refuse"
        );
    }
    row["draft"] = true.into();
    assert!(forge::parse_change(&row, true).unwrap().is_none());
    row["draft"] = false.into();
    row["sha"] = "--help".into();
    assert!(forge::parse_change(&row, true).is_err());
    row.as_object_mut().unwrap().remove("draft");
    assert!(forge::parse_change(&row, true).is_err());
}

// Spawn the production child boundary in another test process, then hold its
// queue-work executable at a barrier while inspecting its persisted receipt.
// trace:TASK-1993 | ai:codex
fn process_dispatch_barrier(
    root: &Path,
    repo: &forge::Repository,
    change: &forge::Change,
    at_barrier: impl FnOnce(),
) {
    let store = Store::open(root, &repo.identity).unwrap();
    let attempt = dispatch(select(&store, change, Some("claude".into()), 2).unwrap());
    let ticket = root.join(".aida/task1993-process-ticket");
    std::fs::write(&ticket, format!("{}:{}", change.key(), attempt.token)).unwrap();
    let program = root.parent().unwrap().join("bin/queue-fixture");
    crate::test_exec::write_executable(
        &program,
        r#"#!/usr/bin/python3
import json, os, pathlib, sys, time
root = pathlib.Path.cwd()
state = json.loads((root / '.aida/review-watch/state.json').read_text())
key = sys.argv[3]
attempt = state['entries'][key]['attempts'][0]
assert attempt['phase'] == 'running'
assert attempt['identity']['pid'] == os.getppid()
assert attempt['identity']['start'] and attempt['identity']['boot']
assert sys.argv[1:] == ['queue', 'work', key, '--role', 'reviewer', '--no-human', '--fresh', '--vendor', 'claude']
for name in os.environ:
    assert not name.startswith(('AIDA_FROM_PR_', 'AIDA_AGENT_GATE_'))
    assert not name.startswith('AIDA_REVIEW_') or name in ['AIDA_REVIEW_WATCH_HEAD', 'AIDA_REVIEW_ROUND']
for name in ['GH_REPO', 'GLAB_REPO', 'GITLAB_REPO', 'GIT_DIR', 'GIT_WORK_TREE', 'GIT_COMMON_DIR', 'GIT_INDEX_FILE', 'GIT_NAMESPACE', 'GIT_CONFIG_COUNT', 'AIDA_GRADED_REVIEW_CONTEXT']:
    assert name not in os.environ, name
assert os.environ['AIDA_REVIEW_WATCH_HEAD'] == attempt['head']
assert os.environ['AIDA_REVIEW_ROUND'] == '1'
assert os.environ['GH_HOST'] in ['github.com', 'gitlab.example.test']
assert os.environ.get('AIDA_SESSION_GRANT')
# The dispatcher records this child before relying on it.
# trace:TASK-1993 | ai:claude
deadline = time.monotonic() + 10
while True:
    attempt = json.loads((root / '.aida/review-watch/state.json').read_text())['entries'][key]['attempts'][0]
    if 'worker' in attempt:
        break
    assert attempt.get('spawning'), attempt
    assert time.monotonic() < deadline, 'worker identity never recorded'
    time.sleep(0.01)
assert attempt['worker']['pid'] == os.getpid() and attempt['worker']['start']
(root / 'queue-ready').write_text(str(os.getppid()))
deadline = time.monotonic() + 15
while not (root / 'queue-release').exists():
    assert time.monotonic() < deadline, 'process barrier timed out'
    time.sleep(0.01)
"#,
    );
    let mut command = Command::new(crate::aida_bin::current_executable_path().unwrap());
    command.current_dir(root).args([
        "--exact", "--nocapture", "--test-threads=1",
        "review_watch::tests::task1993_live_discovery_probes_both_forges_and_refuses_moving_or_unknown_heads",
    ]);
    for name in [
        "AIDA_REVIEW_VERDICT_FILE",
        "AIDA_REVIEW_ROUND",
        "AIDA_AGENT_GATE_NAME",
        "AIDA_AGENT_GATE_ROLE",
        "AIDA_FROM_PR_REVIEW",
        "AIDA_FROM_PR_NUMBER",
        "AIDA_FROM_PR_HEAD_SHA",
        "AIDA_GRADED_REVIEW_CONTEXT",
        "GH_REPO",
        "GLAB_REPO",
        "GITLAB_REPO",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_NAMESPACE",
    ] {
        command.env(name, "unrelated-parent-context");
    }
    let mut child = command.spawn().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !root.join("queue-ready").exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("dispatcher exited before barrier: {status}");
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("queue barrier was never reached");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let state = store.load().unwrap();
    let receipt = &state.entries[&change.key()].attempts[0];
    assert_eq!(receipt.phase, Phase::Running);
    assert_eq!(receipt.identity.as_ref().unwrap().pid, child.id());
    assert!(matches!(
        select(&store, change, None, 2).unwrap(),
        Action::Skip(_)
    ));
    at_barrier();
    std::fs::write(root.join("queue-release"), "continue").unwrap();
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("dispatcher did not complete");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(child.wait().unwrap().success());
    assert_eq!(
        store.load().unwrap().entries[&change.key()].attempts[0].phase,
        Phase::Completed
    );
    std::fs::remove_file(ticket).unwrap();
}

// Exercise the production discovery/probe argv against fake forge CLIs and
// real local Git refs, rather than returning a preclassified overlap boolean.
// trace:TASK-1993.ac4fdc2a | ai:claude
// trace:TASK-1993.ac9ef854 | ai:claude
// trace:TASK-1993.acb96f0d | ai:claude
// trace:TASK-1993.acab7994 | ai:claude
#[test]
fn task1993_live_discovery_probes_both_forges_and_refuses_moving_or_unknown_heads() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let cwd = std::env::current_dir().unwrap();
    if let Ok(ticket) = std::fs::read_to_string(cwd.join(".aida/task1993-process-ticket")) {
        let repo = forge::Repository::load(&cwd).unwrap();
        child_with_program(
            &cwd,
            &repo,
            &ticket,
            &cwd.parent().unwrap().join("bin/queue-fixture"),
        )
        .unwrap();
        return;
    }
    for gitlab in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let hub = temp.path().join("hub.git");
        let bin = temp.path().join("bin");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&bin).unwrap();
        let git_run = |cwd: &Path, args: &[&str]| -> String {
            let out = Command::new("/usr/bin/git")
                .args(crate::test_git::git_command().get_args())
                .current_dir(cwd)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().into()
        };
        git_run(
            temp.path(),
            &["init", "-q", "--bare", hub.to_str().unwrap()],
        );
        git_run(&root, &["init", "-q", "-b", "main"]);
        git_run(&root, &["config", "user.name", "fixture"]);
        git_run(&root, &["config", "user.email", "fixture@test"]);
        std::fs::write(root.join("own.rs"), "initial").unwrap();
        git_run(&root, &["add", "."]);
        git_run(&root, &["commit", "-q", "-m", "initial"]);
        git_run(&root, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(root.join("own.rs"), "feature").unwrap();
        git_run(&root, &["commit", "-qam", "feature"]);
        let head = git_run(&root, &["rev-parse", "HEAD"]);
        git_run(&root, &["checkout", "-q", "main"]);
        std::fs::write(root.join("other.rs"), "main advance").unwrap();
        git_run(&root, &["add", "."]);
        git_run(&root, &["commit", "-q", "-m", "unrelated main advance"]);
        git_run(&root, &["remote", "add", "origin", hub.to_str().unwrap()]);
        git_run(
            &root,
            &[
                "push",
                "-q",
                "origin",
                "main",
                "feature:refs/pull/1/head",
                "feature:refs/merge-requests/1/head",
            ],
        );
        git_run(
            &root,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        let origin = if gitlab {
            "https://gitlab.example.test/group/sub/repo.git"
        } else {
            "https://github.com/owner/repo.git"
        };
        let row = if gitlab {
            serde_json::json!({"iid":1,"state":"opened","draft":false,"sha":head,"target_branch":"main","author":{"username":"writer"}})
        } else {
            serde_json::json!({"number":1,"state":"OPEN","isDraft":false,"headRefOid":head,"baseRefName":"main","author":{"login":"writer"},"statusCheckRollup":[{"status":"IN_PROGRESS"}]})
        };
        let list = temp.path().join("list.json");
        let view = temp.path().join("view.json");
        std::fs::write(&list, serde_json::to_vec(&vec![&row]).unwrap()).unwrap();
        std::fs::write(&view, serde_json::to_vec(&row).unwrap()).unwrap();
        let q = |p: &Path| crate::sh_single_quote(&p.to_string_lossy());
        // Every Git argv is logged so the credential test can inspect it; an
        // origin-url file overrides the reported origin. trace:TASK-1993 | ai:claude
        crate::test_exec::write_executable(&bin.join("git"), &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{tmp}/argv.log'\nif [ \"$1 $2 $3\" = 'remote get-url origin' ]; then if [ -f '{tmp}/origin-url' ]; then cat '{tmp}/origin-url'; else printf '%s\\n' '{origin}'; fi; exit 0; fi\nif [ \"$1\" = fetch ]; then shift 5; exec /usr/bin/git -c maintenance.auto=false -c gc.auto=0 fetch --no-write-fetch-head --no-tags -- '{hub}' \"$@\"; fi\nif [ -f '{tmp}/bad-count' ] && [ \"$1\" = rev-list ]; then echo not-a-count; exit 0; fi\nexec /usr/bin/git -c maintenance.auto=false -c gc.auto=0 \"$@\"\n", hub = q(&hub), tmp = q(temp.path())));
        let script = if gitlab {
            format!("#!/bin/sh\n[ \"$GITLAB_HOST\" = gitlab.example.test ] || exit 81\n[ \"$1 $2 $3\" = 'api -X GET' ] || exit 82\ncase \"$4\" in projects/group%2Fsub%2Frepo/merge_requests) cat '{}' ;; projects/group%2Fsub%2Frepo/merge_requests/1) cat '{}' ;; *) exit 83 ;; esac\n", q(&list), q(&view))
        } else {
            format!("#!/bin/sh\n[ \"$GH_HOST\" = github.com ] || exit 81\n[ \"$1\" = pr ] || exit 82\ncase \"$2\" in list) cat '{}' ;; view) cat '{}' ;; *) exit 83 ;; esac\n", q(&list), q(&view))
        };
        crate::test_exec::write_executable(&bin.join(if gitlab { "glab" } else { "gh" }), &script);
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let mut env = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            &root,
            "advisor",
            &["reviewer"],
            &[
                ("PATH", Some(path.as_str())),
                ("HOME", Some(temp.path().to_str().unwrap())),
                ("AIDA_HOME", Some(temp.path().to_str().unwrap())),
                ("GH_REPO", Some("wrong/repo")),
                ("GLAB_REPO", Some("wrong/repo")),
                ("AIDA_BIN", None),
                ("AIDA_AGENT_CMD", None),
            ],
        );
        // HOME is pinned by the same held environment guard. Reissue this
        // fixture's parent in that final home before testing the real runner.
        let parent = crate::seat_authority::test_support::mint_grant_for(
            &root,
            crate::test_env::HERMETIC_TEST_USER,
            "advisor",
            &["reviewer"],
        );
        env.set_key(crate::seat_authority::GRANT_ENV, parent); // trace:TASK-1993 | ai:claude
        let repo = forge::Repository::load(&root).unwrap();
        let mut changes = repo.discover(&root).unwrap();
        assert_eq!(changes.len(), 1);
        assert!(changes[0].overlap.is_none());
        repo.probe(&root, &mut changes[0]).unwrap();
        assert_eq!(
            changes[0].overlap,
            Some(false),
            "a stale non-overlapping head remains reviewable"
        );
        // Base evidence is the change's own target branch: the same head
        // overlaps a release branch that touched its file.
        // trace:TASK-1993 | ai:claude
        // Forge-independent checks run once, in the GitHub pass.
        if !gitlab {
            git_run(&root, &["checkout", "-q", "-b", "release", "main~1"]);
            std::fs::write(root.join("own.rs"), "release fix").unwrap();
            git_run(&root, &["commit", "-qam", "release fix"]);
            git_run(&root, &["push", "-q", "origin", "release"]);
            git_run(&root, &["checkout", "-q", "main"]);
            changes[0].base = "release".into();
            repo.probe(&root, &mut changes[0]).unwrap();
            assert_eq!(
                changes[0].overlap,
                Some(true),
                "probe ignored the change's base branch"
            );
            changes[0].base = "main".into();
            repo.probe(&root, &mut changes[0]).unwrap();
            assert_eq!(changes[0].overlap, Some(false));
        }
        // A credentialed origin never reaches git argv (BUG-2181): the probe
        // fetches by remote name through the credential gate.
        // trace:TASK-1993 | ai:claude
        if !gitlab {
            let secret = "SECRET-TOKEN-1993";
            std::fs::write(
                temp.path().join("origin-url"),
                format!("https://oauth2:{secret}@github.com/owner/repo.git\n"),
            )
            .unwrap();
            let credentialed = forge::Repository::load(&root).unwrap();
            assert_eq!(credentialed.identity, repo.identity);
            std::fs::write(temp.path().join("argv.log"), "").unwrap();
            let mut probed = changes[0].clone();
            credentialed.probe(&root, &mut probed).unwrap();
            let argv = std::fs::read_to_string(temp.path().join("argv.log")).unwrap();
            assert!(
                argv.contains("fetch --no-write-fetch-head --no-tags -- origin"),
                "{argv}"
            );
            assert!(
                !argv.contains(secret),
                "credential leaked into git argv: {argv}"
            );
            std::fs::remove_file(temp.path().join("origin-url")).unwrap();
        }
        assert_eq!(repo.change(&root, 1).unwrap().head, head);
        // The reviewer launch boundary: outside a watch launch it is inert;
        // inside one it revalidates head and reservation before any review.
        // trace:TASK-1993 | ai:claude
        if !gitlab {
            assert!(reviewer_launch_prompt(&root, 1, "P").unwrap().is_none());
            env.set_key(WATCH_HEAD, &head);
            let refused = reviewer_launch_prompt(&root, 1, "P").unwrap_err();
            assert!(
                format!("{refused:#}").contains("missing watcher reservation"),
                "{refused:#}"
            );
            env.set_key(WATCH_HEAD, "f".repeat(40));
            let refused = reviewer_launch_prompt(&root, 1, "P").unwrap_err();
            assert!(
                format!("{refused:#}").contains("head changed"),
                "{refused:#}"
            );
            env.unset_key(WATCH_HEAD);
        }
        process_dispatch_barrier(&root, &repo, &changes[0], || {
            env.set_key(WATCH_HEAD, &head);
            let prompt = reviewer_launch_prompt(&root, 1, "/aida-review --pr 1")
                .unwrap()
                .expect("a reserved watch launch gets the watch prompt");
            assert!(prompt.starts_with("/aida-review --pr 1"));
            assert!(prompt.contains("do not run Cargo"));
            assert!(prompt.contains("Never merge or update the branch"));
            env.unset_key(WATCH_HEAD);
        });
        std::fs::write(temp.path().join("bad-count"), "").unwrap();
        assert!(
            repo.probe(&root, &mut changes[0]).is_err(),
            "unknown count must never become zero behind"
        );
        std::fs::remove_file(temp.path().join("bad-count")).unwrap();
        changes[0].head = "f".repeat(40);
        assert!(
            repo.probe(&root, &mut changes[0]).is_err(),
            "moving head must not dispatch"
        );
        changes[0].head = head;
        // Renames must include their source in the overlap set.
        git_run(&root, &["checkout", "-q", "feature"]);
        git_run(&root, &["mv", "own.rs", "renamed.rs"]);
        git_run(&root, &["commit", "-qm", "rename feature path"]);
        let renamed_head = git_run(&root, &["rev-parse", "HEAD"]);
        git_run(
            &root,
            &[
                "push",
                "-q",
                "origin",
                "feature:refs/pull/1/head",
                "feature:refs/merge-requests/1/head",
            ],
        );
        git_run(&root, &["checkout", "-q", "main"]);
        let mut renamed_row = row.clone();
        renamed_row[if gitlab { "sha" } else { "headRefOid" }] = renamed_head.clone().into();
        std::fs::write(&list, serde_json::to_vec(&vec![&renamed_row]).unwrap()).unwrap();
        std::fs::write(&view, serde_json::to_vec(&renamed_row).unwrap()).unwrap();
        changes[0].head = renamed_head.clone();
        // A real queue-work process refusing (here a held reviewer lease) is
        // an observed exit: recorded terminally with the reason from its log.
        // trace:TASK-1993 | ai:claude
        {
            let store = Store::open(&root, &repo.identity).unwrap();
            let attempt = dispatch(select(&store, &changes[0], None, 2).unwrap());
            let key = changes[0].key();
            let logs = Directory::open(&store.dir.path().join("logs")).unwrap();
            logs.file(&format!("{key}-{}.log", attempt.token), true)
                .unwrap();
            let refusing = temp.path().join("bin/queue-refusing");
            crate::test_exec::write_executable(
                &refusing,
                r#"#!/bin/sh
for log in .aida/review-watch/logs/*.log; do echo "reviewer lease is already held by session 01a15dfa" >> "$log"; done
exit 3
"#,
            );
            let error =
                child_with_program(&root, &repo, &format!("{key}:{}", attempt.token), &refusing)
                    .unwrap_err();
            assert!(
                format!("{error:#}").contains("lease is already held"),
                "{error:#}"
            );
            let recorded = &store.load().unwrap().entries[&key].attempts[0];
            assert_eq!(recorded.phase, Phase::Exited);
            assert!(recorded
                .diagnostic
                .as_deref()
                .unwrap()
                .contains("lease is already held"));
            assert!(store
                .load()
                .unwrap()
                .entries
                .values()
                .flat_map(|e| &e.attempts)
                .all(|a| !active(a)));
        }
        std::fs::write(root.join("own.rs"), "overlapping main advance").unwrap();
        git_run(&root, &["commit", "-qam", "overlapping main advance"]);
        git_run(&root, &["push", "-q", "origin", "main"]);
        repo.probe(&root, &mut changes[0]).unwrap();
        assert_eq!(changes[0].overlap, Some(true));
        env.set_key(WATCH_HEAD, &renamed_head);
        assert!(
            reviewer_launch_prompt(&root, 1, "P").is_err(),
            "an overlap appearing before the vendor launch refuses the review"
        );
        env.unset_key(WATCH_HEAD);
        let args = ReviewWatchArgs {
            once: true,
            interval: 1,
            max_concurrent: 2,
            vendor: None,
            release: None,
            launch_reservation: None,
        };
        // The watcher resolves its real cwd through sanitized Git, not the
        // test-only ambient root used by authority helpers.
        std::env::set_current_dir(&root).unwrap();
        super::run(&args).unwrap();
        super::run(&args).unwrap();
        std::env::set_current_dir(&cwd).unwrap();
        let messages = aida_core::mailbox::read_local_messages(&root).unwrap();
        assert_eq!(
            messages.len(),
            1,
            "actual watcher passes must send one stale-head notice"
        );
        assert_eq!(messages[0].from, "review-watch");
        assert!(
            git_run(
                &root,
                &[
                    "for-each-ref",
                    "--format=%(refname)",
                    "refs/aida/review-watch/"
                ]
            )
            .is_empty(),
            "private probe refs must be cleaned"
        );
    }
}

// trace:TASK-1993.ac4fdc2a | ai:claude
#[test]
fn task1993_review_subcommand_parser_enforces_bounded_settings() {
    use clap::Parser;
    let parsed = crate::review_cmd::ReviewArgs::try_parse_from([
        "review", "watch", "--once", "--vendor", "claude",
    ])
    .unwrap();
    let Some(crate::ReviewCommand::Watch(args)) = parsed.cmd else {
        panic!("watch was not routed through the review registry")
    };
    assert!(args.once);
    assert_eq!(args.max_concurrent, 2);
    assert_eq!(args.vendor.as_deref(), Some("claude"));
    assert!(crate::review_cmd::ReviewArgs::try_parse_from([
        "review",
        "watch",
        "--max-concurrent",
        "3"
    ])
    .is_err());
    assert!(
        crate::review_cmd::ReviewArgs::try_parse_from(["review", "watch", "--interval", "0"])
            .is_err()
    );
}

// trace:TASK-1993 | ai:codex
// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_existing_and_mixed_review_rounds_share_the_cap() {
    let (temp, store) = fixture();
    let path = temp.path().join(".aida/review-verdicts");
    std::fs::create_dir(&path).unwrap();
    let record = |head: u64| serde_json::json!({"reviewed_sha": format!("{head:040x}"), "recorded_by":"manual-reviewer", "verdict":"request-changes"});
    let write = |value: serde_json::Value| {
        let file = path.join("PR-1.json");
        std::fs::write(&file, serde_json::to_vec(&value).unwrap()).unwrap();
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
    };
    write(record(1));
    assert!(matches!(
        select(&store, &change(1, 1, false), None, 2).unwrap(),
        Action::Skip(_)
    ));
    let second = dispatch(select(&store, &change(1, 2, false), None, 2).unwrap());
    store
        .transition("PR-1", &second.token, Phase::Running)
        .unwrap();
    assert_eq!(store.round_for("PR-1", &second).unwrap(), 2);
    // A manual/drain second round lands before queue/vendor launch.
    let mut two = record(3);
    two["rounds"] = serde_json::json!([record(1)]);
    write(two);
    assert!(store.round_for("PR-1", &second).is_err());
    store
        .transition("PR-1", &second.token, Phase::Failed)
        .unwrap();
    let Action::Skip(reason) = select(&store, &change(1, 4, false), None, 2).unwrap() else {
        panic!("third review dispatched");
    };
    assert!(reason.contains("two-round"));
    // A fresh watcher also sees both historical rounds.
    let reopened = Store::open(temp.path(), &store.repository).unwrap();
    assert!(matches!(
        select(&reopened, &change(1, 5, false), None, 2).unwrap(),
        Action::Skip(_)
    ));
    let mut prior = record(7);
    prior["rounds"] = serde_json::json!([record(6)]);
    let file = path.join("PR-2.json");
    std::fs::write(&file, serde_json::to_vec(&prior).unwrap()).unwrap();
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        select(&reopened, &change(2, 8, false), None, 2).unwrap(),
        Action::Skip(_)
    ));
    assert!(!reopened.load().unwrap().entries.contains_key("PR-2"));
}

// trace:TASK-1993 | ai:codex
// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_history_deduplicates_archives_and_refuses_unsafe_evidence() {
    let (temp, store) = fixture();
    let verdicts = temp.path().join(".aida/review-verdicts");
    let archive = verdicts.join("PR-1");
    std::fs::create_dir_all(&archive).unwrap();
    let body = serde_json::to_vec(&serde_json::json!({"reviewed_sha":"a".repeat(40), "recorded_by":"reviewer", "verdict":"approved"})).unwrap();
    for file in [
        verdicts.join("PR-1.json"),
        archive.join(format!("{}.json", "a".repeat(40))),
    ] {
        std::fs::write(&file, &body).unwrap();
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert_eq!(store.history("PR-1").unwrap().keys.len(), 1);
    std::fs::remove_file(verdicts.join("PR-1.json")).unwrap();
    assert_eq!(store.history("PR-1").unwrap().keys.len(), 1);
    std::fs::rename(&archive, verdicts.join("kept")).unwrap();
    std::os::unix::fs::symlink(verdicts.join("kept"), &archive).unwrap();
    assert!(store.history("PR-1").is_err());
}

// trace:TASK-1993 | ai:codex
// trace:TASK-1993.acb96f0d | ai:claude
#[test]
fn task1993_file_directory_collisions_are_stale_overlap() {
    let paths = |names: &[&str]| names.iter().map(|s| s.to_string()).collect();
    assert!(forge::paths_overlap(
        &paths(&["config"]),
        &paths(&["config/settings"])
    ));
    assert!(forge::paths_overlap(
        &paths(&["config/settings"]),
        &paths(&["config"])
    ));
    assert!(!forge::paths_overlap(
        &paths(&["config"]),
        &paths(&["config-backup/settings"])
    ));
}

// trace:TASK-1993 | ai:codex
// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_canonical_history_permissions_and_reset_preserve_consumed_rounds() {
    let (temp, store) = fixture();
    // Directory search bits model the canonical layout, not executable fixtures.
    let shared_directory_mode = 0o775;
    std::fs::set_permissions(
        temp.path().join(".aida"),
        std::fs::Permissions::from_mode(shared_directory_mode),
    )
    .unwrap();
    let store = Store::open(temp.path(), &store.repository).unwrap();
    let verdicts = temp.path().join(".aida/review-verdicts");
    std::fs::create_dir(&verdicts).unwrap();
    std::fs::set_permissions(
        &verdicts,
        std::fs::Permissions::from_mode(shared_directory_mode),
    )
    .unwrap();
    let path = verdicts.join("PR-1.json");
    let body = |head: u64| {
        serde_json::json!({"reviewed_sha": format!("{head:040x}"),
        "recorded_by":"manual-reviewer", "verdict":"request-changes"})
        .to_string()
    };
    for mode in [0o644, 0o664] {
        crate::review_verdict::write_verdict_atomic(&path, &body(1)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(store.history("PR-1").unwrap().keys.len(), 1);
    }
    // Current-only legacy evidence has no archive. Production admission must
    // persist it before the ordinary queue-work reset deletes this file.
    let second = change(1, 2, false);
    let attempt = dispatch(select(&store, &second, None, 2).unwrap());
    assert_eq!(store.retained("PR-1").unwrap().len(), 1);
    crate::queue_cmd::clear_standalone_reviewer_verdict(&path);
    assert!(!path.exists());
    execute_reservation(
        &store,
        "PR-1",
        &attempt.token,
        |a| {
            assert_eq!(store.round_for("PR-1", a)?, 2);
            Ok(())
        },
        |_| {
            crate::review_verdict::write_verdict_atomic(&path, &body(2))?;
            Ok(())
        },
    )
    .unwrap();
    let reopened = Store::open(temp.path(), &store.repository).unwrap();
    let Action::Skip(reason) = select(&reopened, &change(1, 3, false), None, 2).unwrap() else {
        panic!("third head bypassed the historical two-round cap after verdict reset")
    };
    assert!(reason.contains("two-round"));
    // Canonical history accepts ordinary writer modes; runtime state stays private.
    std::fs::set_permissions(
        store.dir.path().join(STATE),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(store.load().is_err());
}

// trace:TASK-1993 | ai:codex
// trace:TASK-1993.ac4fdc2a | ai:claude
#[test]
fn task1993_initial_root_ignores_ambient_git_routing() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let requested = temp.path().join("requested");
    let other = temp.path().join("other");
    for root in [&requested, &other] {
        std::fs::create_dir(root).unwrap();
        assert!(Command::new("/usr/bin/git")
            .args(crate::test_git::git_command().get_args())
            .args(["init", "-q"])
            .arg(root)
            .status()
            .unwrap()
            .success());
    }
    let mut env =
        crate::test_env::EnvVarsGuard::snapshot(&["GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR"]);
    env.set_key("GIT_DIR", other.join(".git"));
    env.set_key("GIT_WORK_TREE", &other);
    env.set_key("GIT_COMMON_DIR", other.join(".git"));
    assert_eq!(
        forge::main_root(&requested).unwrap(),
        requested.canonicalize().unwrap()
    );
    assert_ne!(
        forge::main_root(&requested).unwrap(),
        other.canonicalize().unwrap()
    );
}

// Queue work's deterministic refusals (STORY-281 stale-overlap bail, TASK-480
// intermediate-only gate, a reviewer lease already held, the launch-boundary
// revalidation) are observed exits: terminal, capacity freed, never relaunched
// for that head, and a later head still dispatches.
// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_observed_queue_work_refusals_are_terminal_and_free_capacity() {
    for (id, refusal) in [
        (
            1,
            "refusing review: PR-1 is behind main and overlaps changed files (STORY-281)",
        ),
        (
            2,
            "refusing review: PR-2 contains only intermediate commits (TASK-480)",
        ),
        (
            3,
            "reviewer lease for PR-3 is already held by session 01a15dfa",
        ),
        (
            4,
            "review-watch head changed before reviewer launch; no review started",
        ),
    ] {
        let (_temp, store) = fixture();
        let c = change(id, 1, false);
        let attempt = dispatch(select(&store, &c, None, 1).unwrap());
        let error = execute_reservation(
            &store,
            &c.key(),
            &attempt.token,
            |_| Ok(()),
            |_| anyhow::bail!("queue work exited exit status: 1: {refusal}"),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains(refusal));
        let recorded = &store.load().unwrap().entries[&c.key()].attempts[0];
        assert_eq!(recorded.phase, Phase::Exited);
        assert!(recorded.diagnostic.as_deref().unwrap().contains(refusal));
        let Action::Skip(reason) = select(&store, &c, None, 1).unwrap() else {
            panic!("an observed refusal relaunched the same head");
        };
        assert!(
            reason.contains("exited") && reason.contains(refusal),
            "{reason}"
        );
        // The single global slot is free again for another change...
        let other = dispatch(select(&store, &change(id + 10, 1, false), None, 1).unwrap());
        complete(&store, &change(id + 10, 1, false), &other);
        // ...and a later head of the refused change dispatches.
        dispatch(select(&store, &change(id, 2, false), None, 1).unwrap());
    }
}

// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_state_stays_bounded_far_past_one_thousand_changes() {
    let (_temp, store) = fixture();
    // One live review holds the only slot; 1500 skipped changes
    // persist nothing.
    let live = change(1, 1, false);
    dispatch(select(&store, &live, None, 1).unwrap());
    for id in 2..=1500 {
        assert!(matches!(
            select(&store, &change(id, id, false), None, 1).unwrap(),
            Action::Skip(_)
        ));
    }
    assert_eq!(store.load().unwrap().entries.len(), 1);
    let open: BTreeSet<String> = ["PR-1".to_string()].into();
    assert_eq!(store.prune(&open).unwrap(), 0);
    // Reviewed changes that close are pruned; the live attempt survives even
    // when its change leaves the open set. One transaction seeds 1199
    // completed receipts so the test measures pruning, not fsync throughput.
    let (_temp, store) = fixture();
    let live = dispatch(select(&store, &change(1, 1, false), None, 2).unwrap());
    store
        .transition("PR-1", &live.token, Phase::Running)
        .unwrap();
    let me = Identity::current().unwrap();
    store
        .transaction("PR-1", |state| {
            for id in 2..=1200u64 {
                state.entries.insert(
                    format!("PR-{id}"),
                    Entry {
                        attempts: vec![Attempt {
                            head: format!("{id:040x}"),
                            token: uuid::Uuid::new_v4().to_string(),
                            vendor: None,
                            phase: Phase::Completed,
                            identity: Some(me.clone()),
                            diagnostic: None,
                            dispatcher: None,
                            spawning: false,
                            worker: None,
                            reviewer_spawning: false,
                            reviewer: None,
                            pgid: None,
                            supersede: None,
                        }],
                        ..Default::default()
                    },
                );
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(store.load().unwrap().entries.len(), 1200);
    assert_eq!(
        store.prune(&BTreeSet::from(["PR-7".to_string()])).unwrap(),
        1198
    );
    assert_eq!(store.prune(&BTreeSet::new()).unwrap(), 1);
    let state = store.load().unwrap();
    assert_eq!(state.entries.keys().collect::<Vec<_>>(), ["PR-1"]);
    dispatch(select(&store, &change(5000, 1, false), None, 2).unwrap());
}

// The round is the ordinary one (TASK-1291): only verdicts that block done
// (the durable findings blocks) consume it, so an approval followed by main
// merges gets a round-1 full-sweep re-review, and two blocking rounds cap.
// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_approved_head_merged_with_main_is_rereviewed_at_round_one() {
    let (temp, store) = fixture();
    let verdicts = temp.path().join(".aida/review-verdicts");
    std::fs::create_dir(&verdicts).unwrap();
    let path = verdicts.join("PR-1.json");
    let head = |n: u64| format!("{n:040x}");
    write_verdict(&path, &verdict_json(&head(1), "approved"));
    let mut history = vec![verdict_json(&head(1), "approved")];
    for n in [2, 3] {
        let c = change(1, n, false);
        let a = dispatch(select(&store, &c, None, 2).unwrap());
        execute_reservation(
            &store,
            "PR-1",
            &a.token,
            |a| {
                assert_eq!(store.round_for("PR-1", a)?, 1, "approval consumed a round");
                Ok(())
            },
            |_| {
                let mut current = verdict_json(&c.head, "approved");
                current["rounds"] = serde_json::Value::Array(history.clone());
                write_verdict(&path, &current);
                Ok(())
            },
        )
        .unwrap();
        history.push(verdict_json(&c.head, "approved"));
    }
    assert_eq!(
        crate::queue_cmd::reviewer_prompt_for_round(1, 1).contains("ROUND 1 FULL-SWEEP"),
        true
    );
    // Blocking reviews advance the round and cap at two.
    for (n, expected) in [(4, 1), (5, 2)] {
        let c = change(1, n, false);
        let a = dispatch(select(&store, &c, None, 2).unwrap());
        execute_reservation(
            &store,
            "PR-1",
            &a.token,
            |a| {
                assert_eq!(store.round_for("PR-1", a)?, expected);
                Ok(())
            },
            |_| {
                let mut current = verdict_json(&c.head, "request-changes");
                current["rounds"] = serde_json::Value::Array(history.clone());
                write_verdict(&path, &current);
                Ok(())
            },
        )
        .unwrap();
        history.push(verdict_json(&c.head, "request-changes"));
    }
    let Action::Skip(reason) = select(&store, &change(1, 6, false), None, 2).unwrap() else {
        panic!("third blocking round dispatched");
    };
    assert!(reason.contains("two-round"));
}

// trace:TASK-1993.ac4fdc2a | ai:claude
#[test]
fn task1993_continuous_mode_continues_past_change_errors_and_stops_on_corrupt_state() {
    let (_temp, store) = fixture();
    let c = change(1, 1, true);
    select(&store, &c, None, 2).unwrap();
    store
        .transaction(&c.key(), |state| {
            let notice = state.entries.get_mut(&c.key()).unwrap();
            notice.notices.get_mut(&c.head).unwrap().message.from = "advisor".into();
            Ok(())
        })
        .unwrap();
    let drift = select(&store, &c, None, 2).unwrap_err();
    assert!(
        !stops_watcher(&drift, false),
        "one change's drift stopped the daemon"
    );
    assert!(
        stops_watcher(&drift, true),
        "--once still reports the refusal"
    );
    // Other changes still dispatch.
    dispatch(select(&store, &change(2, 2, false), None, 2).unwrap());
    let path = store.dir.path().join(STATE);
    std::fs::write(&path, b"{\"version\":999}").unwrap();
    let corrupt = select(&store, &change(3, 3, false), None, 2).unwrap_err();
    assert!(
        stops_watcher(&corrupt, false),
        "corrupt state must stop the watcher"
    );
}

// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_capacity_refusal_names_holders_and_recovery() {
    let (_temp, store) = fixture();
    let a = dispatch(select(&store, &change(1, 1, false), None, 1).unwrap());
    let Action::Skip(reason) = select(&store, &change(2, 2, false), None, 1).unwrap() else {
        panic!("capacity exceeded");
    };
    for needle in [
        "PR-1 at",
        &a.token,
        "Reserved",
        "aida session leases --all",
        "review-watch/logs",
    ] {
        assert!(reason.contains(needle), "{needle} missing from {reason}");
    }
}

// S1 has no process signalling, merge, branch update or Cargo execution; the
// reviewer it launches is told the same.
// trace:TASK-1993.acab7994 | ai:claude
#[test]
fn task1993_watcher_never_signals_merges_or_runs_cargo() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    // Readiness and the commit CI reader too: the report names update and
    // ship commands for the advisor but never runs one. trace:TASK-1945 | ai:claude
    for file in [
        "review_watch.rs",
        "review_watch_forge.rs",
        "review_watch_ready.rs",
        "forge_commit_ci.rs",
    ] {
        let text = std::fs::read_to_string(src.join(file)).unwrap();
        for forbidden in [
            "libc::kill",
            ".kill()",
            "killpg",
            "nix::sys::signal",
            "\"merge\"",
            "\"update-branch\"",
            "\"push\"",
            "\"rebase\"",
            "--override",
            "merge_change",
            "\"cargo\"",
            "\"ship\"",
            // trace:TASK-1993 | ai:claude
            "PR_SET_PDEATHSIG",
            "SIGTERM",
            "prctl",
            "pre_exec",
        ] {
            assert!(!text.contains(forbidden), "{file} contains {forbidden}");
        }
    }
    for policy in ["do not run Cargo", "Never merge or update the branch"] {
        assert!(REVIEWER_POLICY.contains(policy));
    }
    assert!(!queue_args("PR-1", Some("claude"))
        .iter()
        .any(|a| a.contains("merge")));
}

// A dispatcher can exit before claiming its receipt: its grant refused
// (validate_child_delegation runs before the reservation branch), the store
// unusable (Store::open), or the claim timing out under lock contention. Any
// watcher, including one restarted after the spawning parent is gone, sees
// the recorded dispatcher is gone and frees the slot terminally.
// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_dispatcher_lost_before_claim_frees_its_reservation() {
    for trigger in [
        "error: the session grant expired; a reviewer seat cannot be delegated",
        "Error: unsafe review-watch directory .aida/review-watch",
        "Error: review-watch lock busy: PR-1.lock",
    ] {
        let (temp, store) = fixture();
        let c = change(1, 1, false);
        let attempt = dispatch(select(&store, &c, None, 1).unwrap());
        // A real dispatcher process, identified while alive, then gone.
        let mut dispatcher = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let identity = Identity::of(dispatcher.id()).unwrap();
        assert!(identity.alive());
        store
            .transaction("PR-1", |state| {
                state.entries.get_mut("PR-1").unwrap().attempts[0].dispatcher =
                    Some(identity.clone());
                Ok(())
            })
            .unwrap();
        let logs = Directory::open(&store.dir.path().join("logs")).unwrap();
        let mut log = logs
            .file(&format!("PR-1-{}.log", attempt.token), true)
            .unwrap();
        writeln!(log, "{trigger}").unwrap();
        // While it lives the reservation holds the only slot.
        assert!(matches!(
            select(&store, &change(2, 2, false), None, 1).unwrap(),
            Action::Skip(_)
        ));
        dispatcher.kill().unwrap();
        dispatcher.wait().unwrap();
        assert!(!identity.alive());
        // A fresh watcher (no parent bookkeeping) reconciles and dispatches.
        let restarted = Store::open(temp.path(), &store.repository).unwrap();
        dispatch(select(&restarted, &change(2, 2, false), None, 1).unwrap());
        let recorded = &restarted.load().unwrap().entries["PR-1"].attempts[0];
        assert_eq!(recorded.phase, Phase::Exited);
        assert!(
            recorded.diagnostic.as_deref().unwrap().contains(trigger),
            "{recorded:?}"
        );
        let Action::Skip(reason) = select(&restarted, &c, None, 1).unwrap() else {
            panic!("the lost head relaunched");
        };
        assert!(reason.contains("exited"));
    }
    // The parent's own reaping records an observed early exit directly.
    let (_temp, store) = fixture();
    let attempt = dispatch(select(&store, &change(1, 1, false), None, 1).unwrap());
    mark_unclaimed_exit(&store, "PR-1", &attempt.token, "exit status: 1").unwrap();
    assert_eq!(
        store.load().unwrap().entries["PR-1"].attempts[0].phase,
        Phase::Exited
    );
    dispatch(select(&store, &change(2, 2, false), None, 1).unwrap());
}

// The triggers above really do leave the reservation unclaimed.
// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_unclaimed_triggers_never_start_work() {
    let (temp, store) = fixture();
    let attempt = dispatch(select(&store, &change(1, 1, false), None, 2).unwrap());
    {
        let _held = store.dir.lock("PR-1.lock").unwrap();
        let other = Store::open(temp.path(), &store.repository).unwrap();
        assert!(execute_reservation(
            &other,
            "PR-1",
            &attempt.token,
            |_| panic!("preflight"),
            |_| panic!("work")
        )
        .is_err());
    }
    assert_eq!(
        store.load().unwrap().entries["PR-1"].attempts[0].phase,
        Phase::Reserved
    );
    // An unsafe watcher directory (here a symlink) makes Store::open refuse.
    let dir = temp.path().join(".aida/review-watch");
    let moved = temp.path().join("moved-review-watch");
    std::fs::rename(&dir, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &dir).unwrap();
    assert!(Store::open(temp.path(), &store.repository).is_err());
    std::fs::remove_file(&dir).unwrap();
    std::fs::rename(&moved, &dir).unwrap();
}

// Rounds survive pruning: a draft toggle or close/reopen cannot reset the
// two-round cap, even for legacy current-only verdicts that queue work clears.
// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_pruning_keeps_consumed_rounds() {
    let (temp, store) = fixture();
    let verdicts = temp.path().join(".aida/review-verdicts");
    std::fs::create_dir(&verdicts).unwrap();
    let path = verdicts.join("PR-1.json");
    write_verdict(
        &path,
        &verdict_json(&format!("{:040x}", 1), "request-changes"),
    );
    let second = change(1, 2, false);
    let a = dispatch(select(&store, &second, None, 2).unwrap());
    crate::queue_cmd::clear_standalone_reviewer_verdict(&path);
    execute_reservation(
        &store,
        "PR-1",
        &a.token,
        |_| Ok(()),
        |_| {
            write_verdict(&path, &verdict_json(&second.head, "request-changes"));
            Ok(())
        },
    )
    .unwrap();
    // A later pass observes the second blocking verdict...
    assert!(matches!(
        select(&store, &second, None, 2).unwrap(),
        Action::Skip(_)
    ));
    // ...then queue work's reset clears it and the change leaves the open set.
    crate::queue_cmd::clear_standalone_reviewer_verdict(&path);
    store.prune(&BTreeSet::new()).unwrap();
    assert!(!store.load().unwrap().entries.contains_key("PR-1"));
    let Action::Skip(reason) = select(&store, &change(1, 3, false), None, 2).unwrap() else {
        panic!("a pruned change dispatched a third blocking round");
    };
    assert!(reason.contains("two-round"), "{reason}");
}

// The watch revalidation runs before the destructive verdict reset.
// trace:TASK-1993.ac9ef854 | ai:claude
#[test]
fn task1993_launch_revalidation_precedes_verdict_reset() {
    let source = include_str!("../queue_cmd.rs");
    let check = source
        .find("crate::review_watch::reviewer_launch_context(&project_root, *pr)?;")
        .expect("watch revalidation in handle_queue_work");
    let clear = source
        .find("clear_standalone_reviewer_verdict(verdict_path);")
        .expect("verdict reset in handle_queue_work");
    assert!(
        check < clear,
        "the verdict is cleared before the watch launch is revalidated"
    );
}

// A terminal receipt waits through lock contention like the claim does; one
// whose lock stays busy past the deadline leaves the attempt Running, and the
// next pass frees it once the dispatcher's exact process is gone.
// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_terminal_receipt_lock_timeout_is_recovered_next_pass() {
    let hold = |store: &Store, millis: u64| {
        let lock = store.dir.lock("PR-1.lock").unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(millis));
            drop(lock);
        })
    };
    // Brief contention: the completion still lands.
    let (_temp, store) = fixture();
    let a = dispatch(select(&store, &change(1, 1, false), None, 1).unwrap());
    let mut holder = None;
    execute_reservation(
        &store,
        "PR-1",
        &a.token,
        |_| Ok(()),
        |_| {
            holder = Some(hold(&store, 400));
            Ok(())
        },
    )
    .unwrap();
    holder.unwrap().join().unwrap();
    assert_eq!(
        store.load().unwrap().entries["PR-1"].attempts[0].phase,
        Phase::Completed
    );
    // Contention past the deadline: the receipt is lost, the attempt stays
    // Running, and the slot is still held while the dispatcher lives.
    let (_temp, store) = fixture();
    let a = dispatch(select(&store, &change(1, 1, false), None, 1).unwrap());
    let mut holder = None;
    let lost = execute_reservation(
        &store,
        "PR-1",
        &a.token,
        |_| Ok(()),
        |_| {
            holder = Some(hold(&store, 2500));
            Ok(())
        },
    );
    assert!(lost.is_err(), "the completion should have timed out");
    holder.unwrap().join().unwrap();
    assert_eq!(
        store.load().unwrap().entries["PR-1"].attempts[0].phase,
        Phase::Running
    );
    assert!(matches!(
        select(&store, &change(2, 2, false), None, 1).unwrap(),
        Action::Skip(_)
    ));
    // The dispatcher then exits (here: its receipt names a process now gone).
    let mut gone = Command::new("/bin/sleep").arg("30").spawn().unwrap();
    let identity = Identity::of(gone.id()).unwrap();
    gone.kill().unwrap();
    gone.wait().unwrap();
    store
        .transaction("PR-1", |state| {
            state.entries.get_mut("PR-1").unwrap().attempts[0].identity = Some(identity.clone());
            Ok(())
        })
        .unwrap();
    dispatch(select(&store, &change(2, 2, false), None, 1).unwrap());
    let recorded = &store.load().unwrap().entries["PR-1"].attempts[0];
    assert_eq!(recorded.phase, Phase::Exited);
    assert!(
        recorded
            .diagnostic
            .as_deref()
            .unwrap()
            .contains("after claiming"),
        "{recorded:?}"
    );
}

// Notices name the change's base. A pending notice recorded with the earlier
// default-branch wording keeps it (same ID, idempotent retry), and a sent one
// is never re-sent, so the wording change re-sends nothing.
// trace:TASK-1993.acb96f0d | ai:claude
#[test]
fn task1993_stale_notice_names_the_base_without_resending_old_notices() {
    let (_temp, store) = fixture();
    let mut c = change(1, 1, true);
    c.base = "release/2".into();
    let Action::Notify(notice) = select(&store, &c, None, 2).unwrap() else {
        panic!("missing notice");
    };
    assert!(
        notice.message.body.contains("`release/2`"),
        "{}",
        notice.message.body
    );
    assert!(!notice.message.body.contains("default branch"));
    // A notice pending from before the wording change is re-delivered as is.
    let old = change(2, 2, true);
    // Recorded before recipients were mapped: addressed to the forge login.
    // trace:TASK-1955 | ai:claude (TASK-1-410)
    let legacy = stale_message_for(&store.repository, "PR-2", &old, None, &old.author);
    assert_eq!(
        legacy.id,
        stale_message(&store.repository, "PR-2", &old, "advisor").id
    );
    for sent in [false, true] {
        store
            .transaction("PR-2", |state| {
                state
                    .entries
                    .entry("PR-2".into())
                    .or_default()
                    .notices
                    .insert(
                        old.head.clone(),
                        Notice {
                            message: legacy.clone(),
                            sent,
                        },
                    );
                Ok(())
            })
            .unwrap();
        match select(&store, &old, None, 2).unwrap() {
            Action::Notify(retry) if !sent => {
                assert_eq!(retry.message.body, legacy.body);
                assert_eq!(retry.message.id, legacy.id);
            }
            Action::Skip(reason) if sent => assert!(reason.contains("already notified")),
            other => panic!("unexpected action for sent={sent}: {other:?}"),
        }
    }
}

// The review is carried by the reviewer process queue work launches, which
// can outlive both queue work and the dispatcher. Real processes model the
// tree: the slot is held while the reviewer lives and freed once it exits,
// and a reviewer never recorded keeps the slot held, named as ambiguous.
// trace:TASK-1993.ac55aab8 | ai:claude
// trace:TASK-1993.ac1e5827 | ai:claude
#[test]
fn task1993_orphaned_reviewer_keeps_its_slot_until_it_exits() {
    let temp = tempfile::tempdir().unwrap();
    let pid_of = |name: &str| -> u32 {
        let path = temp.path().join(name);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(pid) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| s.trim().parse().ok())
            {
                return pid;
            }
            assert!(std::time::Instant::now() < deadline, "{name} never started");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    };
    let gone = |identity: &Identity| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while identity.alive() {
            assert!(
                std::time::Instant::now() < deadline,
                "process {} did not exit",
                identity.pid
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    };
    let kill = |pid: u32| {
        assert!(Command::new("/bin/kill")
            .args(["-9", &pid.to_string()])
            .status()
            .unwrap()
            .success());
    };
    // dispatcher -> queue work -> reviewer, as real processes.
    let tree = temp.path().join("dispatcher");
    crate::test_exec::write_executable(
        &tree,
        &format!(
            "#!/bin/sh\nsh -c 'sleep 60 & echo $! > {dir}/reviewer; wait' & echo $! > {dir}/queue; wait\n",
            dir = temp.path().display()
        ),
    );
    let mut dispatcher = Command::new(&tree).spawn().unwrap();
    let dispatcher_id = Identity::of(dispatcher.id()).unwrap();
    let queue_id = Identity::of(pid_of("queue")).unwrap();
    let reviewer_id = Identity::of(pid_of("reviewer")).unwrap();
    let running = |store: &Store, reviewer: Option<Identity>| {
        let a = dispatch(select(store, &change(1, 1, false), None, 1).unwrap());
        store
            .transaction("PR-1", |state| {
                let a = &mut state.entries.get_mut("PR-1").unwrap().attempts[0];
                a.phase = Phase::Running;
                a.identity = Some(dispatcher_id.clone());
                a.spawning = true;
                a.worker = Some(queue_id.clone());
                a.reviewer_spawning = true;
                a.reviewer = reviewer.clone();
                Ok(())
            })
            .unwrap();
        a
    };
    let (_store_temp, store) = fixture();
    running(&store, Some(reviewer_id.clone()));
    // The dispatcher and queue work die; the reviewer is orphaned but alive.
    dispatcher.kill().unwrap();
    dispatcher.wait().unwrap();
    kill(queue_id.pid);
    gone(&dispatcher_id);
    gone(&queue_id);
    assert!(reviewer_id.alive());
    assert!(matches!(
        select(&store, &change(2, 2, false), None, 1).unwrap(),
        Action::Skip(_)
    ));
    assert!(matches!(
        select(&store, &change(1, 2, false), None, 1).unwrap(),
        Action::Skip(_)
    ));
    assert_eq!(
        store.load().unwrap().entries["PR-1"].attempts[0].phase,
        Phase::Running
    );
    // The reviewer exits: only now is the slot freed.
    kill(reviewer_id.pid);
    gone(&reviewer_id);
    dispatch(select(&store, &change(2, 2, false), None, 1).unwrap());
    assert_eq!(
        store.load().unwrap().entries["PR-1"].attempts[0].phase,
        Phase::Exited
    );
    // A reviewer marked spawning but never recorded stays held as ambiguous.
    let (_store_temp, store) = fixture();
    let a = running(&store, None);
    let Action::Skip(reason) = select(&store, &change(2, 2, false), None, 1).unwrap() else {
        panic!("an unrecorded reviewer freed its slot");
    };
    assert!(
        reason.contains(&a.token) && reason.contains("never recorded"),
        "{reason}"
    );
    assert!(
        reason.contains("no `queue work PR-1` or reviewer process for it remains"),
        "{reason}"
    );
    assert_eq!(
        store.load().unwrap().entries["PR-1"].attempts[0].phase,
        Phase::Running
    );
}

// Queue work records the reviewer it launches through the production spawn
// helper; outside a watch launch the helper is the ordinary blocking run.
// trace:TASK-1993.ac55aab8 | ai:claude
#[test]
fn task1993_queue_work_records_the_reviewer_it_launches() {
    let (temp, store) = fixture();
    let c = change(1, 1, false);
    dispatch(select(&store, &c, None, 2).unwrap());
    store
        .transaction("PR-1", |state| {
            let a = &mut state.entries.get_mut("PR-1").unwrap().attempts[0];
            a.phase = Phase::Running;
            a.identity = Some(Identity::current()?);
            a.spawning = true;
            Ok(())
        })
        .unwrap();
    LAUNCH.with(|l| *l.borrow_mut() = None);
    let exit = || Command::new("/bin/true");
    assert!(reviewer_status(&mut exit()).unwrap().success());
    assert!(store.load().unwrap().entries["PR-1"].attempts[0]
        .reviewer
        .is_none());
    LAUNCH.with(|l| {
        *l.borrow_mut() = Some((
            temp.path().to_path_buf(),
            store.repository.clone(),
            "PR-1".into(),
            c.head.clone(),
        ))
    });
    let status = reviewer_status(&mut exit());
    LAUNCH.with(|l| *l.borrow_mut() = None);
    assert!(status.unwrap().success());
    let attempt = &store.load().unwrap().entries["PR-1"].attempts[0];
    assert!(attempt.reviewer_spawning);
    let reviewer = attempt
        .reviewer
        .as_ref()
        .expect("reviewer identity recorded");
    assert_ne!(reviewer.pid, std::process::id());
    assert_eq!(
        attempt.worker.as_ref(),
        Some(&Identity::current().unwrap()),
        "queue work recorded itself"
    );
}

/// TASK-2064: a change keeps a notice only for its current head, so a run
/// of stale heads can no longer grow to the 256-notice cap.
// trace:TASK-1955 | ai:claude (TASK-2064)
#[test]
fn notices_are_kept_for_the_current_head_only() {
    let (_temp, store) = fixture();
    assert!(matches!(
        select(&store, &change(1, 1, true), None, 2).unwrap(),
        Action::Notify(_)
    ));
    assert!(matches!(
        select(&store, &change(1, 2, true), None, 2).unwrap(),
        Action::Notify(_)
    ));
    let state = store.load().unwrap();
    let heads: Vec<_> = state.entries["PR-1"].notices.keys().cloned().collect();
    assert_eq!(heads, vec![change(1, 2, true).head]);
}

/// PR-2789 note (b): retained rounds are capped at the two the round cap
/// needs, so the next read's <=16 check can never fail.
// trace:TASK-1955 | ai:claude (TASK-2064)
#[test]
fn retained_rounds_are_capped_on_write() {
    let (_temp, store) = fixture();
    let keys = (1..=20u64).map(|n| {
        (
            Some(format!("{n:040x}")),
            "reviewer seat".to_string(),
            "request-changes".to_string(),
            Vec::new(),
        )
    });
    store.retain("PR-1", keys).unwrap();
    assert_eq!(store.retained("PR-1").unwrap().len(), RETAINED_ROUNDS);
}

/// TASK-2064: dispatcher logs no recorded attempt names are pruned.
// trace:TASK-1955 | ai:claude (TASK-2064)
#[test]
fn prune_removes_logs_no_attempt_names() {
    let (_temp, store) = fixture();
    let c = change(1, 1, false);
    let attempt = dispatch(select(&store, &c, None, 2).unwrap());
    let logs = Directory::open(&store.dir.path().join("logs")).unwrap();
    logs.file(&format!("PR-1-{}.log", attempt.token), true)
        .unwrap();
    logs.file("PR-9-00000000-0000-4000-8000-000000000000.log", true)
        .unwrap();
    store.prune(&BTreeSet::from(["PR-1".to_string()])).unwrap();
    let names: Vec<String> = std::fs::read_dir(logs.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec![format!("PR-1-{}.log", attempt.token)]);
}

/// TASK-1-412: `--release` frees an exited head for a retry, and refuses
/// while a recorded process for the change is alive.
// trace:TASK-1955 | ai:claude (TASK-1-412)
#[test]
fn release_retries_an_exited_head_and_refuses_a_live_one() {
    let (_temp, store) = fixture();
    let c = change(1, 1, false);
    let attempt = dispatch(select(&store, &c, None, 2).unwrap());
    store
        .transaction("PR-1", |state| {
            let a = &mut state.entries.get_mut("PR-1").unwrap().attempts[0];
            a.phase = Phase::Exited;
            a.dispatcher = Some(Identity::current()?);
            Ok(())
        })
        .unwrap();
    let Action::Skip(reason) = select(&store, &c, None, 2).unwrap() else {
        panic!("an exited head is not relaunched")
    };
    assert!(
        reason.contains("aida review watch --release PR-1"),
        "{reason}"
    );
    assert!(
        store.release("PR-1").is_err(),
        "a live recorded process refuses the release"
    );
    store
        .transaction("PR-1", |state| {
            state.entries.get_mut("PR-1").unwrap().attempts[0].dispatcher = None;
            Ok(())
        })
        .unwrap();
    assert_eq!(store.release("PR-1").unwrap(), 1);
    let again = dispatch(select(&store, &c, None, 2).unwrap());
    assert_ne!(again.token, attempt.token);
}

/// TASK-1-415: the reviewer launch reuses the launch queue work already
/// revalidated in this process; no forge or repository is consulted again.
// trace:TASK-1955 | ai:claude (TASK-1-415)
#[test]
fn second_launch_context_reuses_the_first_revalidation() {
    let temp = tempfile::tempdir().unwrap();
    let head = format!("{:040x}", 5);
    let _env = crate::test_env::EnvVarsGuard::apply(&[(WATCH_HEAD, Some(head.as_str()))]);
    LAUNCH.with(|launch| {
        *launch.borrow_mut() = Some((
            temp.path().to_path_buf(),
            "github:github.com/owner/repo".into(),
            "PR-5".into(),
            head.clone(),
        ))
    });
    // The temp dir is no repository: a fresh revalidation would fail.
    assert!(reviewer_launch_context(temp.path(), 5).unwrap());
    assert!(reviewer_launch_context(temp.path(), 6).is_err());
    LAUNCH.with(|launch| *launch.borrow_mut() = None);
}

/// TASK-1-413: probe refs whose prober is gone are swept; a live prober's
/// refs are kept.
// trace:TASK-1955 | ai:claude (TASK-1-413)
#[test]
fn leaked_probe_refs_are_swept_and_live_ones_kept() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "s",
    ]);
    let head = git(&["rev-parse", "HEAD"]);
    let me = std::process::id();
    let uuid = "0b7c6a52-1f3e-4c1d-9a7e-5d2f8c3b4a10";
    // A legacy pid-less UUID whose first group is all digits, equal to a
    // live pid: it must not read as that pid's probe (TASK-2121).
    let legacy_digits = format!("{me:08}-1f3e-4c1d-9a7e-5d2f8c3b4a10");
    let now = chrono::Utc::now().timestamp();
    let old = |name: &str| {
        git(&["update-ref", name, &head]);
        let path = root.join(git(&["rev-parse", "--git-path", name]));
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
            .unwrap();
    };
    let live_old = format!("refs/aida/review-watch/{me}-{uuid}/base");
    old(&live_old);
    old(&format!("refs/aida/review-watch/4000000001-{uuid}/base"));
    old("refs/aida/review-watch/0b7c6a52-1f3e-4c1d-9a7e-5d2f8c3b4a10/head");
    old(&format!("refs/aida/review-watch/{legacy_digits}/head"));
    old(&format!(
        "refs/aida/review-watch/4000000001-t{}-{uuid}/base",
        now - 3600
    ));
    // Fresh refs are kept whatever their pid says: a legacy one by its loose
    // ref mtime, a timestamped one from another pid namespace by its name.
    let fresh_legacy = "refs/aida/review-watch/1c2d3e4f-1f3e-4c1d-9a7e-5d2f8c3b4a10/head";
    let fresh_foreign = format!("refs/aida/review-watch/4000000001-t{now}-{uuid}/base");
    git(&["update-ref", fresh_legacy, &head]);
    git(&["update-ref", &fresh_foreign, &head]);

    assert_eq!(forge::sweep_probe_refs(root), 4);
    let mut left: Vec<String> = git(&[
        "for-each-ref",
        "--format=%(refname)",
        "refs/aida/review-watch/",
    ])
    .lines()
    .map(String::from)
    .collect();
    left.sort();
    let mut want = vec![live_old, fresh_legacy.to_string(), fresh_foreign];
    want.sort();
    assert_eq!(left, want);
}

/// TASK-2121: probe-name parsing never reads a legacy UUID's digit group as
/// a pid, and the current name carries its creation time.
// trace:TASK-1955 | ai:claude (TASK-2121)
#[test]
fn probe_ref_names_parse_pid_and_creation_time_only_from_their_own_shapes() {
    use forge::{parse_probe_name, ProbeName};
    let uuid = "0b7c6a52-1f3e-4c1d-9a7e-5d2f8c3b4a10";
    assert_eq!(
        parse_probe_name(&format!("42-t1700000000-{uuid}")),
        ProbeName {
            pid: Some(42),
            created: Some(1_700_000_000)
        }
    );
    assert_eq!(
        parse_probe_name(&format!("42-{uuid}")),
        ProbeName {
            pid: Some(42),
            created: None
        }
    );
    for legacy in [uuid, "12345678-1f3e-4c1d-9a7e-5d2f8c3b4a10", "42-live", "x"] {
        assert_eq!(
            parse_probe_name(legacy),
            ProbeName {
                pid: None,
                created: None
            },
            "{legacy}"
        );
    }
}

/// TASK-2176 (2): a probe ref dated in the future is not "young" forever; past
/// the pid check it is swept like any old timestamped ref.
// trace:TASK-1955 | ai:claude (TASK-2176)
#[test]
fn a_future_dated_probe_ref_is_not_kept_forever() {
    use forge::{probe_ref_in_use, ProbeName};
    let now = 1_800_000_000;
    let named = |created| ProbeName {
        pid: Some(1),
        created: Some(created),
    };
    let alive = |_| true;
    assert!(
        probe_ref_in_use(&named(now - 5), now, None, alive),
        "young ref kept"
    );
    assert!(
        probe_ref_in_use(&named(now), now, None, alive),
        "a ref made now is kept"
    );
    assert!(
        !probe_ref_in_use(&named(now - 3_600), now, None, alive),
        "old ref swept"
    );
    assert!(
        !probe_ref_in_use(&named(now + 86_400), now, None, alive),
        "a future-dated ref is swept"
    );
    // A future loose-ref mtime on a pid-less legacy ref is swept too.
    let legacy = ProbeName {
        pid: None,
        created: None,
    };
    assert!(!probe_ref_in_use(&legacy, now, Some(now + 86_400), alive));
    assert!(probe_ref_in_use(&legacy, now, Some(now - 1), alive));
}

/// TASK-1-410 / TASK-2109: a stale-head notice goes to the AIDA identity the
/// author's forge login names, in its stored spelling, and a pending notice
/// survives a roster change before it is sent.
// trace:TASK-1955 | ai:claude (TASK-2109)
#[test]
fn stale_notice_maps_the_forge_login_and_survives_a_roster_change() {
    let (temp, store) = fixture();
    let registry = temp.path().join(".aida-store").join("registry");
    std::fs::create_dir_all(&registry).unwrap();
    std::fs::write(
        registry.join("team.toml"),
        "[members]\nJoe = [\"advisor\"]\n",
    )
    .unwrap();
    let mut c = change(1, 1, true);
    c.author = "joe".into();
    let Action::Notify(notice) = select(&store, &c, None, 2).unwrap() else {
        panic!("missing notice")
    };
    assert_eq!(
        notice.message.to,
        aida_core::mailbox::Recipient::Agent("Joe".into())
    );

    // The roster drops Joe before the notice is sent: the pending notice is
    // still delivered as recorded, not refused as drifted.
    std::fs::remove_file(registry.join("team.toml")).unwrap();
    let Action::Notify(retry) = select(&store, &c, None, 2).unwrap() else {
        panic!("pending notice lost after a roster change")
    };
    assert_eq!(retry.message.id, notice.message.id);
    assert_eq!(retry.message.to, notice.message.to);

    // An unmapped login goes to the advisor.
    let mut stranger = change(2, 2, true);
    stranger.author = "octocat".into();
    let Action::Notify(n) = select(&store, &stranger, None, 2).unwrap() else {
        panic!("missing notice")
    };
    assert_eq!(
        n.message.to,
        aida_core::mailbox::Recipient::Agent("advisor".into())
    );
}

/// TASK-2118: a forge login registered only as an alias of an identity is
/// mailed at that identity's stored spelling, not at the alias.
// trace:TASK-1955 | ai:claude (TASK-2118)
#[test]
fn stale_notice_resolves_an_aliased_forge_login_to_its_canonical_identity() {
    let (temp, store) = fixture();
    let registry = temp.path().join(".aida-store").join("registry");
    std::fs::create_dir_all(&registry).unwrap();
    std::fs::write(
        registry.join("team.toml"),
        "[members]\nJoe = [\"advisor\"]\n",
    )
    .unwrap();
    std::fs::write(
        registry.join("aliases.toml"),
        "[aliases]\njoemooney = \"joe\"\n",
    )
    .unwrap();
    let mut c = change(1, 1, true);
    c.author = "joemooney".into();
    let Action::Notify(notice) = select(&store, &c, None, 2).unwrap() else {
        panic!("missing notice")
    };
    assert_eq!(
        notice.message.to,
        aida_core::mailbox::Recipient::Agent("Joe".into())
    );
}
