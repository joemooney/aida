// trace:STORY-HOME-096 | ai:claude
use super::*;
use crate::mailbox_view::tests::{mail, mail_repo};
use std::cell::RefCell;

const THREAD: &str = "01a185ba-0000-7000-8000-000000000002";

/// A queue that records what it was asked to send and answers with a script.
struct FakeQueue {
    sent: RefCell<Vec<String>>,
    answers: RefCell<Vec<Result<(), QueueError>>>,
}

impl FakeQueue {
    fn ok() -> Self {
        Self::answering(Vec::new())
    }
    fn answering(answers: Vec<Result<(), QueueError>>) -> Self {
        Self {
            sent: RefCell::new(Vec::new()),
            answers: RefCell::new(answers),
        }
    }
}

impl ThreadQueue for FakeQueue {
    fn submit(&self, thread: &str, message: &str) -> Result<(), QueueError> {
        assert_eq!(thread, THREAD);
        self.sent.borrow_mut().push(message.to_string());
        let mut answers = self.answers.borrow_mut();
        if answers.is_empty() {
            Ok(())
        } else {
            answers.remove(0)
        }
    }
}

fn inbox() -> Inbox {
    Inbox {
        handle: "codex-agent-02".into(),
        keys: vec!["codex-agent-02".into(), "implementer".into()],
        role: Some("implementer".into()),
    }
}

fn scan(unread: &[&str], acked: &[&str]) -> Scan {
    Scan {
        unread: unread.iter().map(|s| s.to_string()).collect(),
        acknowledged: acked.iter().map(|s| s.to_string()).collect(),
        partial: false,
        role_pending: 0,
    }
}

fn step(ledger: &mut Ledger, scan: Scan, now: f64, queue: &FakeQueue) -> TickReport {
    tick(ledger, scan, &inbox(), now, 5.0, queue, &mut |_| Ok(())).unwrap()
}

fn ledger() -> Ledger {
    Ledger::new(Path::new("/p"), THREAD, "codex-agent-02")
}

// New mail waits out the batch window, wakes once, never twice for the same
// ID, and a read prunes it from the ledger.
#[test]
fn story_home_096_batches_once_dedupes_and_prunes_on_read() {
    let queue = FakeQueue::ok();
    let mut l = ledger();
    let r = step(&mut l, scan(&["m1"], &[]), 100.0, &queue);
    assert!(!r.queue_submitted, "inside the batch window");
    let r = step(&mut l, scan(&["m1", "m2"], &[]), 106.0, &queue);
    assert!(r.queue_submitted);
    assert_eq!(r.ids, vec!["m1", "m2"]);
    let r = step(&mut l, scan(&["m1", "m2"], &[]), 200.0, &queue);
    assert!(!r.queue_submitted, "accepted IDs never wake again");
    assert_eq!(queue.sent.borrow().len(), 1);
    step(&mut l, scan(&[], &["m1"]), 300.0, &queue);
    assert_eq!(l.accepted, BTreeSet::from(["m2".to_string()]));
    assert!(l.accepted_at.contains_key("m2") && !l.accepted_at.contains_key("m1"));
}

// TASK-HOME-119: the notice names whose inbox it read.
#[test]
fn story_home_096_notice_names_the_recipient_inbox() {
    let queue = FakeQueue::ok();
    let mut l = ledger();
    step(&mut l, scan(&["m1"], &[]), 0.0, &queue);
    step(&mut l, scan(&["m1"], &[]), 10.0, &queue);
    let sent = queue.sent.borrow();
    assert!(
        sent[0].starts_with(
            "AIDA mail notification for codex-agent-02 (inbox: codex-agent-02, implementer;"
        ),
        "{}",
        sent[0]
    );
    assert!(sent[0].contains("m1") && !sent[0].contains("advisor"));
}

// A spawn failure retries with backoff; an unknown result pauses the row
// until an explicit recovery, which either records or re-offers the IDs.
#[test]
fn story_home_096_spawn_failure_backs_off_and_uncertainty_needs_recovery() {
    let queue = FakeQueue::answering(vec![Err(QueueError::NotCreated)]);
    let mut l = ledger();
    step(&mut l, scan(&["m1"], &[]), 0.0, &queue);
    let r = step(&mut l, scan(&["m1"], &[]), 10.0, &queue);
    assert!(!r.queue_submitted && l.inflight.is_none());
    assert_eq!(l.retry_after, 20.0);
    assert!(!step(&mut l, scan(&["m1"], &[]), 15.0, &queue).queue_submitted);
    assert!(step(&mut l, scan(&["m1"], &[]), 21.0, &queue).queue_submitted);

    let queue = FakeQueue::answering(vec![Err(QueueError::Uncertain)]);
    let mut l = ledger();
    step(&mut l, scan(&["m1"], &[]), 0.0, &queue);
    let r = step(&mut l, scan(&["m1"], &[]), 10.0, &queue);
    assert!(r.uncertain);
    assert_eq!(l.inflight.as_ref().unwrap().ids, vec!["m1"]);
    let mut retried = l.clone();
    assert!(recover(&mut l, true));
    assert!(l.accepted.contains("m1") && l.pending.is_empty() && l.inflight.is_none());
    assert!(recover(&mut retried, false));
    assert!(retried.pending.contains("m1") && retried.accepted.is_empty());
}

// A partial view and queued role mail each wake once per episode with no
// IDs; a rise in queued role mail wakes again.
#[test]
fn story_home_096_partial_and_role_mail_wake_once_per_episode() {
    let queue = FakeQueue::ok();
    let mut l = ledger();
    let partial = || Scan {
        partial: true,
        ..scan(&[], &[])
    };
    assert!(step(&mut l, partial(), 0.0, &queue).queue_submitted);
    assert!(!step(&mut l, partial(), 30.0, &queue).queue_submitted);
    step(&mut l, scan(&[], &[]), 60.0, &queue);
    assert!(
        step(&mut l, partial(), 90.0, &queue).queue_submitted,
        "new episode"
    );
    let role = |n| Scan {
        role_pending: n,
        ..scan(&[], &[])
    };
    assert!(step(&mut l, role(1), 120.0, &queue).queue_submitted);
    assert!(!step(&mut l, role(1), 150.0, &queue).queue_submitted);
    assert!(step(&mut l, role(2), 180.0, &queue).queue_submitted);
    let sent = queue.sent.borrow();
    assert!(sent
        .last()
        .unwrap()
        .contains("Mail for the implementer role is queued"));
}

// The ledger and the doctor's health record are private and round-trip.
#[test]
fn story_home_096_thread_state_is_private_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let state = ThreadState::open(dir.path(), THREAD).unwrap();
    assert!(state.load().unwrap().is_none());
    let mut l = ledger();
    l.accepted.insert("m1".into());
    state.write(&l, 5.0).unwrap();
    assert_eq!(state.load().unwrap(), Some(l));
    let thread_dir = dir.path().join(THREAD);
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&thread_dir), 0o700);
    assert_eq!(mode(&thread_dir.join("ledger.json")), 0o600);
    let health: serde_json::Value =
        serde_json::from_slice(&std::fs::read(thread_dir.join("health.json")).unwrap()).unwrap();
    assert_eq!(health["accepted_unacknowledged_count"], 1);
    assert_eq!(health["accepted_unknown_age_count"], 1);
    assert_eq!(health["recovery_required"], false);
}

/// Write a registry row for this process under `main`'s registry.
fn register_self(main: &Path, handle: &str, mail_wake: Option<&str>) -> PathBuf {
    let pid = std::process::id();
    let id = format!("codex-{pid}");
    let start = aida_core::liveness::process_start_identity(pid).unwrap();
    let mut row = toml::Table::new();
    for (key, value) in [
        ("id", id.clone()),
        ("agent_type", "codex".into()),
        ("pid_start", start),
        ("name", handle.into()),
        ("role", "implementer".into()),
        ("native_session_id", THREAD.into()),
        ("started_at", "2026-10-29T00:00:00Z".into()),
        ("last_active_at", "2026-10-29T00:00:00Z".into()),
        ("worktree_path", main.to_string_lossy().into_owned()),
        ("source", "agent-launcher".into()),
    ] {
        row.insert(key.into(), toml::Value::String(value));
    }
    row.insert("pid".into(), toml::Value::Integer(i64::from(pid)));
    if let Some(wake) = mail_wake {
        row.insert("mail_wake".into(), toml::Value::String(wake.into()));
    }
    let path = crate::agent_registry::agents_dir(main).join(format!("{id}.toml"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, toml::to_string(&row).unwrap()).unwrap();
    path
}

/// A caller whose own verified audience holds `user` (and `role`).
fn own(user: &str, role: Option<&str>) -> ResolvedAudience {
    ResolvedAudience::from_facts(&crate::mailbox_view::unbound_facts(user, role, None))
}

fn options(r: &crate::mailbox_view::tests::MailRepo, state: &Path) -> RunOptions {
    RunOptions {
        project: r.main.clone(),
        store: r.store.clone(),
        state_root: state.to_path_buf(),
        codex: PathBuf::from("/nonexistent/codex"),
        interval: Duration::from_secs(1),
        batch_seconds: 0.0,
        once: true,
    }
}

// The watcher wakes an armed session for its own handle and role mail only,
// never advisor mail, and never a session that is not armed.
#[test]
fn story_home_096_run_wakes_armed_rows_for_their_own_inbox_only() {
    let r = mail_repo();
    let state = tempfile::tempdir().unwrap();
    for (id, to) in [
        ("to-handle", "codex-agent-02"),
        ("to-role", "implementer"),
        ("to-advisor", "advisor"),
    ] {
        crate::mailbox_store::write_message(&r.sib, &mail(id, to, 10, false)).unwrap();
    }
    let path = register_self(&r.main, "codex-agent-02", None);
    let queue = FakeQueue::ok();
    let opts = options(&r, state.path());
    let lock = WatchLock::acquire(state.path(), &r.main).unwrap();
    let watcher = own("operator", None);
    assert!(
        run_once(&opts, &lock, &watcher, &queue).is_empty(),
        "unarmed"
    );

    register_self(&r.main, "codex-agent-02", Some("armed"));
    let reports = run_once(&opts, &lock, &watcher, &queue);
    assert_eq!(reports.len(), 1, "{reports:?}");
    let tick = reports[0]
        .tick
        .as_ref()
        .unwrap_or_else(|| panic!("{reports:?}"));
    assert!(tick.queue_submitted);
    assert_eq!(tick.ids, vec!["to-handle", "to-role"]);
    assert!(!queue.sent.borrow()[0].contains("to-advisor"));

    // Only the watcher holding the lock reads the armed row's own inbox.
    let audience = crate::mailbox_wake::audience_for_recipient(
        &watcher,
        "codex-agent-02",
        Some((&lock, &path)),
    )
    .unwrap();
    assert_eq!(
        audience.keys().collect::<Vec<_>>(),
        vec!["codex-agent-02", "implementer"]
    );

    std::fs::write(
        &path,
        std::fs::read_to_string(&path)
            .unwrap()
            .replace("mail_wake = \"armed\"", "mail_wake = \"retired\""),
    )
    .unwrap();
    assert!(run_once(&opts, &lock, &watcher, &queue).is_empty());
    assert!(crate::mailbox_wake::audience_for_recipient(
        &watcher,
        "codex-agent-02",
        Some((&lock, &path))
    )
    .is_err());
    assert!(
        state.path().join(THREAD).join("ledger.json").exists(),
        "retiring keeps the ledger"
    );
}

#[test]
fn story_home_096_unit_quotes_specifiers_and_carries_its_marker() {
    let text = unit_text(
        Path::new("/home/u/100%$proj"),
        Path::new("/bin/aida"),
        Path::new("/bin/codex"),
    )
    .unwrap();
    assert!(text.starts_with(&format!("# {UNIT_MARKER}\n")));
    assert!(text.contains("\"/home/u/100%%$$proj\""));
    assert!(text.contains(
        "ExecStart=\"/bin/aida\" \"mailbox\" \"wake-run\" \"--project\" \"/home/u/100%%$$proj\" \
         \"--codex\" \"/bin/codex\""
    ));
    assert_eq!(
        unit_name(Path::new("/a")),
        format!("aida-mail-wake-{}.service", project_key(Path::new("/a")))
    );
}

// `wake-snapshot --recipient` narrows, never widens: another session's
// armed inbox is refused to any caller that does not hold the watcher lock,
// and a second watcher cannot take the lock.
#[test]
fn story_home_096_recipient_narrows_and_only_the_watcher_reads_an_armed_row() {
    use crate::mailbox_wake::audience_for_recipient;
    let r = mail_repo();
    let state = tempfile::tempdir().unwrap();
    let path = register_self(&r.main, "codex-agent-02", Some("armed"));
    let stranger = own("codex-agent-07", Some("reviewer"));
    let err = audience_for_recipient(&stranger, "codex-agent-02", None).unwrap_err();
    assert!(format!("{err:#}").contains("only the project's mail-wake watcher"));

    let lock = WatchLock::acquire(state.path(), &r.main).unwrap();
    assert!(
        WatchLock::acquire(state.path(), &r.main).is_err(),
        "one watcher"
    );
    // The watcher reads only the row that names the recipient, in its project.
    assert!(audience_for_recipient(&stranger, "codex-agent-03", Some((&lock, &path))).is_err());
    let elsewhere = tempfile::tempdir().unwrap();
    let foreign = elsewhere.path().join("codex-1.toml");
    std::fs::copy(&path, &foreign).unwrap();
    assert!(lock.armed_audience(&foreign, "codex-agent-02").is_err());
    assert!(audience_for_recipient(&stranger, "codex-agent-02", Some((&lock, &path))).is_ok());
}

// An agent asking for its OWN handle reads its own audience whether or not a
// row is armed, as before slice 1.
#[test]
fn story_home_096_own_handle_without_an_armed_row_reads_its_own_audience() {
    let r = mail_repo();
    register_self(&r.main, "codex-agent-02", None);
    let me = own("codex-agent-02", Some("implementer"));
    let audience =
        crate::mailbox_wake::audience_for_recipient(&me, "codex-agent-02", None).unwrap();
    assert_eq!(audience.keys().collect::<Vec<_>>(), vec!["codex-agent-02"]);
}

// `--recover` needs exactly one of `--accepted` / `--retry`.
#[test]
fn story_home_096_recover_requires_exactly_one_outcome() {
    use clap::Parser;
    #[derive(Parser)]
    struct Cli {
        #[clap(subcommand)]
        command: crate::cli::MailboxCommand,
    }
    let parse = |extra: &[&str]| {
        let mut argv = vec!["mailbox", "wake-run"];
        argv.extend_from_slice(extra);
        Cli::try_parse_from(argv).map(|_| ())
    };
    assert!(parse(&["--recover", THREAD]).is_err(), "bare --recover");
    assert!(parse(&["--recover", THREAD, "--accepted", "--retry"]).is_err());
    assert!(parse(&["--accepted"]).is_err(), "outcome without --recover");
    assert!(parse(&["--recover", THREAD, "--accepted"]).is_ok());
    assert!(parse(&["--recover", THREAD, "--retry"]).is_ok());
    assert!(parse(&["--once"]).is_ok());
}

// A recover waits for the watcher's in-progress step on that thread and
// applies on top of what the watcher wrote, so neither overwrites the other.
#[test]
fn story_home_096_recover_waits_for_the_watcher_thread_lock() {
    let root = tempfile::tempdir().unwrap();
    let mut held = ledger();
    held.inflight = Some(Receipt {
        ids: vec!["m1".into()],
        at: 1.0,
    });
    let watcher = ThreadState::open(root.path(), THREAD).unwrap();
    watcher.write(&ledger(), 1.0).unwrap();
    let state_root = root.path().to_path_buf();
    let recovering = std::thread::spawn(move || recover_thread(&state_root, THREAD, true).unwrap());
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !recovering.is_finished(),
        "recover waits for the thread lock"
    );
    // The watcher's step ends with an in-flight submission, then unlocks.
    watcher.write(&held, 2.0).unwrap();
    drop(watcher);
    assert!(
        recovering.join().unwrap(),
        "recover saw the watcher's write"
    );
    let after = ThreadState::open(root.path(), THREAD)
        .unwrap()
        .load()
        .unwrap()
        .unwrap();
    assert!(after.inflight.is_none());
    assert!(after.accepted.contains("m1"));
}

// Arming or retiring is the session's own act or the operator's, as with
// `agent stop`: another agent is refused.
#[test]
fn story_home_096_wake_arm_is_the_owner_or_the_operator_only() {
    use crate::agent_launch::require_wake_authority;
    let r = mail_repo();
    register_self(&r.main, "codex-agent-02", None);
    let (_, row) = crate::agent_registry::load_active_entries(&r.main)
        .into_iter()
        .next()
        .unwrap();
    let no_human = || -> Result<()> { bail!("not a direct terminal") };
    assert!(require_wake_authority(&row, Some("codex-agent-02"), no_human).is_ok());
    let err = require_wake_authority(&row, Some("codex-agent-07"), no_human).unwrap_err();
    assert!(format!("{err:#}").contains("only the session itself or the operator"));
    assert!(require_wake_authority(&row, None, no_human).is_err());
    assert!(require_wake_authority(&row, Some("codex-agent-07"), || Ok(())).is_ok());
}

// trace:BUG-2397 | ai:codex
#[test]
fn bug_2397_working_directory_is_literal_and_exec_keeps_argv_escaping() {
    let text = unit_text(
        Path::new("/home/joe/ai/aida"),
        Path::new("/bin/true"),
        Path::new("/bin/true"),
    )
    .unwrap();
    assert!(text.contains("\nWorkingDirectory=/home/joe/ai/aida\n"));
    let text = unit_text(
        Path::new("/home/u/a space/100%$proj"),
        Path::new("/bin/true"),
        Path::new("/bin/true"),
    )
    .unwrap();
    assert!(text.contains("\nWorkingDirectory=/home/u/a space/100%%$proj\n"));
    assert!(text.contains("\"/home/u/a space/100%%$$proj\""));
    for path in ["relative", "/p\n[Service]", "/p\r", "/p\t", "/p ", "/p\\"] {
        assert!(working_directory(Path::new(path)).is_err(), "{path:?}");
    }
}

// This only parses temporary fixtures; it never loads a unit into a manager.
// trace:BUG-2397 | ai:codex
#[test]
fn bug_2397_generated_units_pass_systemd_analyze_when_available() {
    if which("systemd-analyze").is_none() {
        eprintln!("SKIP systemd-analyze is unavailable");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for project in ["/home/joe/ai/aida", "/home/u/a space/100%$proj"] {
        let path = dir.path().join("mail-wake.service");
        std::fs::write(
            &path,
            unit_text(
                Path::new(project),
                Path::new("/bin/true"),
                Path::new("/bin/true"),
            )
            .unwrap(),
        )
        .unwrap();
        let output = std::process::Command::new("systemd-analyze")
            .args(["verify", "--man=no"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

// All manager calls go to a stub under a fake HOME in a one-test child.
// Failed installs must return before printing their success announcement.
// trace:BUG-2397 | ai:codex
#[test]
fn bug_2397_install_failures_and_runtime_status_use_stub_manager() {
    if let Some(output) = crate::test_env::run_path_isolated_test_output() {
        assert!(!String::from_utf8_lossy(&output.stdout).contains("installed aida-mail-wake-"));
        return;
    }
    let r = mail_repo();
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let stub = bin.join("systemctl");
    let log = home.path().join("calls");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let mut env = crate::test_env::EnvVarsGuard::set(&[
        ("HOME", home.path().to_str().unwrap()),
        ("XDG_CONFIG_HOME", home.path().to_str().unwrap()),
        ("PATH", &path),
    ]);
    let write_stub = |body: &str| {
        std::fs::write(
            &stub,
            format!("#!/bin/sh\necho \"$*\" >> '{}'\n{}\n", log.display(), body),
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
    };
    for (fail, expected) in [("daemon-reload", 1), ("enable", 2), ("is-active", 3)] {
        std::fs::write(&log, "").unwrap();
        write_stub(&format!(
            "if [ \"$2\" = '{fail}' ]; then exit 9; fi\nexit 0"
        ));
        let error = service(
            &r.main,
            crate::mailbox_cmd::WakeServiceAction::Install,
            Some(Path::new("/bin/true")),
            false,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains(fail), "{error:#}");
        assert_eq!(
            std::fs::read_to_string(&log).unwrap().lines().count(),
            expected
        );
        assert!(
            unit_path(&unit_name(&r.main)).is_file(),
            "file presence survives activation failure"
        );
    }
    write_stub("exit 0");
    install_unit(
        &unit_path(&unit_name(&r.main)),
        &unit_name(&r.main),
        "fixture",
    )
    .unwrap();
    for (active, sub, result, running, failed) in [
        ("active", "running", "success", true, false),
        ("inactive", "dead", "success", false, false),
        ("failed", "failed", "exit-code", false, true),
    ] {
        write_stub(&format!("printf 'LoadState=loaded\\nActiveState={active}\\nSubState={sub}\\nResult={result}\\n'"));
        let runtime = service_runtime(&unit_name(&r.main));
        assert_eq!(runtime["running"], running);
        assert_eq!(runtime["failed"], failed);
        assert_eq!(runtime["result"], result);
    }
    write_stub("exit 7");
    let runtime = service_runtime(&unit_name(&r.main));
    assert!(runtime["running"].is_null());
    assert!(runtime["error"].as_str().unwrap().contains("show"));
    std::fs::remove_file(&stub).unwrap();
    env.set_key("PATH", &bin);
    let error = systemctl(&["daemon-reload"]).unwrap_err();
    assert!(format!("{error:#}").contains("systemctl --user daemon-reload"));
}

// End to end for a hand-started Codex seat (`aida session start` then
// `codex`): its SessionStart hook's row arms, mail to its handle queues
// exactly one notification, and ending the session retires the row.
// trace:STORY-2256 | ai:claude
#[test]
fn story_2256_hand_started_codex_session_arms_wakes_once_and_retires() {
    let r = mail_repo();
    let state = tempfile::tempdir().unwrap();
    crate::mailbox_store::write_message(&r.sib, &mail("to-handle", "codex-agent-02", 10, false))
        .unwrap();
    // The row as a hand-started session's own SessionStart hook writes it:
    // no launcher, no mail-wake decision, the hook's exact thread id.
    let path = register_self(&r.main, "codex-agent-02", None);
    let mut row: toml::Table = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    row.remove("native_session_id");
    row.insert(
        "claude_session_id".into(),
        toml::Value::String(THREAD.into()),
    );
    row.insert(
        "source".into(),
        toml::Value::String("session-start-hook".into()),
    );
    std::fs::write(&path, toml::to_string(&row).unwrap()).unwrap();
    let entry: AgentRegistryEntry =
        toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(entry.mail_wake, None);

    let installs = std::cell::Cell::new(0);
    let armed = crate::mail_wake::arm_session(
        &r.main,
        &entry,
        None,
        || None,
        |_| {
            installs.set(installs.get() + 1);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(armed.state, MailWake::Armed);
    assert_eq!(installs.get(), 1, "the hook installs the missing service");
    let on_disk: AgentRegistryEntry =
        toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(on_disk.mail_wake, Some(MailWake::Armed));
    assert_eq!(row_thread(&on_disk).as_deref(), Some(THREAD));

    let queue = FakeQueue::ok();
    let opts = options(&r, state.path());
    let lock = WatchLock::acquire(state.path(), &r.main).unwrap();
    let watcher = own("operator", None);
    let reports = run_once(&opts, &lock, &watcher, &queue);
    assert_eq!(reports.len(), 1, "{reports:?}");
    let tick = reports[0]
        .tick
        .as_ref()
        .unwrap_or_else(|| panic!("{reports:?}"));
    assert!(tick.queue_submitted);
    assert_eq!(tick.ids, vec!["to-handle"]);
    run_once(&opts, &lock, &watcher, &queue);
    assert_eq!(queue.sent.borrow().len(), 1, "exactly one notification");

    let ended = crate::agent_registry::mark_agent_ended(&r.main, "codex", std::process::id())
        .unwrap()
        .unwrap();
    assert_eq!(ended.mail_wake, Some(MailWake::Retired));
    assert!(run_once(&opts, &lock, &watcher, &queue).is_empty());
    assert_eq!(queue.sent.borrow().len(), 1);
}
