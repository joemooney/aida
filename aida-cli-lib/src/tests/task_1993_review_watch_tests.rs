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
        author: "writer".into(),
        gitlab: false,
        overlap: Some(overlap),
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

#[test]
fn task1993_pending_ci_dispatches_with_receipt_before_work() {
    let (_temp, store) = fixture();
    let value = serde_json::json!({"number": 1, "state": "OPEN", "isDraft": false,
        "headRefOid": "1".repeat(40), "author": {"login": "writer"},
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

#[test]
fn task1993_lost_completion_and_pid_reuse_remain_ambiguous() {
    let (_temp, store) = fixture();
    let c = change(1, 1, false);
    let attempt = dispatch(select(&store, &c, None, 2).unwrap());
    execute_reservation(
        &store,
        &c.key(),
        &attempt.token,
        |_| Ok(()),
        |_| anyhow::bail!("dispatcher died after work began"),
    )
    .unwrap_err();
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
    assert!(matches!(
        select(&store, &change(1, 2, false), None, 2).unwrap(),
        Action::Skip(_)
    ));
    assert_eq!(
        store.load().unwrap().entries[&c.key()].attempts[0].phase,
        Phase::Running
    );
}

#[test]
fn task1993_new_head_waits_without_signalling_then_obeys_two_round_cap() {
    let (_temp, store) = fixture();
    let first = change(1, 1, false);
    let a = dispatch(select(&store, &first, None, 2).unwrap());
    let second = change(1, 2, false);
    assert!(matches!(
        select(&store, &second, None, 2).unwrap(),
        Action::Skip(_)
    ));
    complete(&store, &first, &a);
    let b = dispatch(select(&store, &second, None, 2).unwrap());
    complete(&store, &second, &b);
    let Action::Skip(reason) = select(&store, &change(1, 3, false), None, 2).unwrap() else {
        panic!("third round dispatched")
    };
    assert!(reason.contains("two-round"));
}

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

#[test]
fn task1993_stale_notice_retry_is_idempotent_and_preserves_reader_state() {
    let (temp, store) = fixture();
    let c = change(1, 1, true);
    let Action::Notify(notice) = select(&store, &c, None, 2).unwrap() else {
        panic!("missing notice")
    };
    assert!(store.load().unwrap().entries[&c.key()].attempts.is_empty());
    deliver(temp.path(), &notice).unwrap();
    let path = aida_core::mailbox::local_mailbox_dir(temp.path())
        .join(format!("{}.json", notice.message.id));
    let mut archived = notice.message.clone();
    archived.archived = true;
    std::fs::write(&path, serde_json::to_vec(&archived).unwrap()).unwrap();
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

#[test]
fn task1993_strict_discovery_handles_gitlab_and_refuses_unknown_evidence() {
    let mut row = serde_json::json!({"iid": 4, "state": "opened", "draft": false,
        "sha": "a".repeat(40), "author": {"username": "writer"}});
    assert_eq!(
        forge::parse_change(&row, true).unwrap().unwrap().key(),
        "MR-4"
    );
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
fn process_dispatch_barrier(root: &Path, repo: &forge::Repository, change: &forge::Change) {
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
(root / 'queue-ready').write_text(str(os.getppid()))
deadline = time.monotonic() + 15
while not (root / 'queue-release').exists():
    assert time.monotonic() < deadline, 'process barrier timed out'
    time.sleep(0.01)
"#,
    );
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.current_dir(root).args([
        "--exact", "review_watch::tests::task1993_live_discovery_probes_both_forges_and_refuses_moving_or_unknown_heads",
        "--nocapture", "--test-threads=1",
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
            serde_json::json!({"iid":1,"state":"opened","draft":false,"sha":head,"author":{"username":"writer"}})
        } else {
            serde_json::json!({"number":1,"state":"OPEN","isDraft":false,"headRefOid":head,"author":{"login":"writer"},"statusCheckRollup":[{"status":"IN_PROGRESS"}]})
        };
        let list = temp.path().join("list.json");
        let view = temp.path().join("view.json");
        std::fs::write(&list, serde_json::to_vec(&vec![&row]).unwrap()).unwrap();
        std::fs::write(&view, serde_json::to_vec(&row).unwrap()).unwrap();
        let q = |p: &Path| crate::sh_single_quote(&p.to_string_lossy());
        crate::test_exec::write_executable(&bin.join("git"), &format!(
            "#!/bin/sh\nif [ \"$1 $2 $3\" = 'remote get-url origin' ]; then printf '%s\\n' '{origin}'; exit 0; fi\nif [ \"$1\" = fetch ]; then shift 5; exec /usr/bin/git fetch --no-write-fetch-head --no-tags -- '{}' \"$@\"; fi\nif [ -f '{}/bad-count' ] && [ \"$1\" = rev-list ]; then echo not-a-count; exit 0; fi\nexec /usr/bin/git \"$@\"\n", q(&hub), q(temp.path())));
        let script = if gitlab {
            format!("#!/bin/sh\n[ \"$GITLAB_HOST\" = gitlab.example.test ] || exit 81\n[ \"$1 $2 $3\" = 'api -X GET' ] || exit 82\ncase \"$4\" in projects/group%2Fsub%2Frepo/merge_requests) cat '{}' ;; projects/group%2Fsub%2Frepo/merge_requests/1) cat '{}' ;; *) exit 83 ;; esac\n", q(&list), q(&view))
        } else {
            format!("#!/bin/sh\n[ \"$GH_HOST\" = github.com ] || exit 81\n[ \"$1\" = pr ] || exit 82\ncase \"$2\" in list) cat '{}' ;; view) cat '{}' ;; *) exit 83 ;; esac\n", q(&list), q(&view))
        };
        crate::test_exec::write_executable(&bin.join(if gitlab { "glab" } else { "gh" }), &script);
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let _env = crate::test_env::AmbientGuard::hermetic_with_seat_and(
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
        std::env::set_var(crate::seat_authority::GRANT_ENV, parent);
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
        assert_eq!(repo.change(&root, 1).unwrap().head, head);
        process_dispatch_barrier(&root, &repo, &changes[0]);
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
        changes[0].head = renamed_head;
        std::fs::write(root.join("own.rs"), "overlapping main advance").unwrap();
        git_run(&root, &["commit", "-qam", "overlapping main advance"]);
        git_run(&root, &["push", "-q", "origin", "main"]);
        repo.probe(&root, &mut changes[0]).unwrap();
        assert_eq!(changes[0].overlap, Some(true));
        let args = ReviewWatchArgs {
            once: true,
            interval: 1,
            max_concurrent: 2,
            vendor: None,
            launch_reservation: None,
        };
        super::run(&args).unwrap();
        super::run(&args).unwrap();
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
    assert!(reopened.load().unwrap().entries["PR-2"].attempts.is_empty());
}

// trace:TASK-1993 | ai:codex
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
