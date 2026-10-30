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
    let wake = mail_wake
        .map(|w| format!("mail_wake = \"{w}\"\n"))
        .unwrap_or_default();
    let path = crate::agent_registry::agents_dir(main).join(format!("{id}.toml"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        format!(
            "id = \"{id}\"\nagent_type = \"codex\"\npid = {pid}\npid_start = \"{start}\"\n\
             name = \"{handle}\"\nrole = \"implementer\"\nnative_session_id = \"{THREAD}\"\n\
             started_at = \"2026-10-29T00:00:00Z\"\nlast_active_at = \"2026-10-29T00:00:00Z\"\n\
             worktree_path = \"{}\"\nsource = \"agent-launcher\"\n{wake}",
            main.display()
        ),
    )
    .unwrap();
    path
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
    assert!(
        run_once(&options(&r, state.path()), &queue).is_empty(),
        "unarmed"
    );

    register_self(&r.main, "codex-agent-02", Some("armed"));
    let reports = run_once(&options(&r, state.path()), &queue);
    assert_eq!(reports.len(), 1, "{reports:?}");
    let tick = reports[0]
        .tick
        .as_ref()
        .unwrap_or_else(|| panic!("{reports:?}"));
    assert!(tick.queue_submitted);
    assert_eq!(tick.ids, vec!["to-handle", "to-role"]);
    assert!(!queue.sent.borrow()[0].contains("to-advisor"));

    // The verified recipient path of `wake-snapshot` reads the same inbox.
    let audience = crate::mailbox_wake::audience_for_recipient(&r.main, "codex-agent-02").unwrap();
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
    assert!(run_once(&options(&r, state.path()), &queue).is_empty());
    assert!(crate::mailbox_wake::audience_for_recipient(&r.main, "codex-agent-02").is_err());
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
    );
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
