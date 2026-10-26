//! BUG-1934: seat grants are bound to the process tree that was issued them.
//! An exported grant id is honoured only for a presenter that runs under one
//! of the grant's bound processes; at ship the attestation must carry the
//! matched binding.
// trace:BUG-1934 | ai:claude

use super::*;
use crate::gate::review_authority::{with_identity_source, IdentitySource, ProcEntry};
use crate::test_env::EnvVarsGuard;

const SUBJECT: &str = "bug-1934-fixture-user";
const FOREIGN: &str = "proc:999999@linux-starttime:1";

/// Hold the env lock for the whole test with `AIDA_USER` pinned.
fn pinned_env() -> EnvVarsGuard {
    let mut env = EnvVarsGuard::snapshot(&["AIDA_USER", GRANT_ENV, "AIDA_SESSION_ID", "AIDA_HOME"]);
    env.set_key("AIDA_USER", SUBJECT);
    env.unset_key("AIDA_SESSION_ID");
    env
}

fn set_binding(id: &str, binding: &[&str]) {
    let binding: Vec<String> = binding.iter().map(|s| s.to_string()).collect();
    update_grant(
        id,
        |g| {
            g.binding = binding.clone();
            true
        },
        |g| g.binding == binding,
    )
    .unwrap();
}

fn proc(pid: u32, name: &str, start: &str) -> ProcEntry {
    ProcEntry {
        pid,
        name: name.into(),
        cmd: vec![name.into()],
        start: Some(start.into()),
    }
}

/// A presenter: `aida` (5000) under `claude` (4300) under the shell the
/// grant was entered in (4242), in POSIX session 4200.
fn presenter_source() -> IdentitySource {
    IdentitySource {
        session_ids: vec!["copyable-session-id".into()],
        chain: vec![
            proc(5000, "aida", "linux-starttime:500"),
            proc(4300, "claude", "linux-starttime:430"),
            proc(4242, "bash", "linux-starttime:424"),
        ],
        session_leader: Some(proc(4200, "", "linux-starttime:420")),
    }
}

#[test]
fn grant_refused_when_presenter_identity_does_not_overlap_binding() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env();
    let id = test_support::mint_grant_for(root, SUBJECT, "reviewer", &[]);
    set_binding(&id, &[FOREIGN]);
    env.set_key(GRANT_ENV, &id);

    let check = grant_check(root);
    assert!(
        matches!(
            &check,
            GrantCheck::Rejected {
                reason: GrantRejection::PresenterNotBound { .. },
                ..
            }
        ),
        "{check:?}"
    );
    assert!(current_grant(root).is_none());
    let sentence = check.rejection_sentence().unwrap();
    assert!(sentence.contains("bound to process 999999"), "{sentence}");
    assert!(
        sentence.contains("this process does not run under it"),
        "{sentence}"
    );
    // A copied session id is no binding: it is as copyable as the grant.
    with_identity_source(presenter_source(), || {
        assert!(current_grant(root).is_none());
    });
}

#[test]
fn grant_accepted_for_bound_session() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env();
    let id = test_support::mint_grant_for(root, SUBJECT, "reviewer", &[]);
    // Bound to the issuing shell only, which sits ABOVE the harness.
    set_binding(&id, &["proc:4242@linux-starttime:424"]);
    env.set_key(GRANT_ENV, &id);

    assert!(current_grant(root).is_none(), "not under the shell");
    with_identity_source(presenter_source(), || {
        let grant = current_grant(root).expect("a process under the bound shell holds it");
        assert_eq!(grant.seat, "reviewer");
        // The recorder identity stops at the harness, which is why grant
        // binding uses the presenter's full ancestry instead.
        let recorder = IdentitySource::live().recorder_tokens();
        assert!(!recorder.iter().any(|t| t.starts_with("proc:4242@")));
    });
    // Bound to the terminal session's leader: any process in the session.
    set_binding(&id, &["sid:4200@linux-starttime:420"]);
    with_identity_source(presenter_source(), || {
        assert!(current_grant(root).is_some());
    });
}

/// C1: every grant minted before this change is `Unbound` and refuses, and
/// the refusal names both sanctioned fixes.
#[test]
fn legacy_grant_without_binding_is_refused_and_names_the_fix() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env();
    let id = test_support::mint_grant_for(root, SUBJECT, "reviewer", &[]);
    set_binding(&id, &[]);
    let raw = std::fs::read_to_string(grant_dir().unwrap().join(format!("{id}.json"))).unwrap();
    assert!(
        !raw.contains("binding"),
        "a legacy record has no binding key"
    );
    env.set_key(GRANT_ENV, &id);

    let check = grant_check(root);
    assert!(
        matches!(
            &check,
            GrantCheck::Rejected {
                reason: GrantRejection::Unbound,
                ..
            }
        ),
        "{check:?}"
    );
    let sentence = check.rejection_sentence().unwrap();
    assert!(
        sentence.contains("issued before grants were bound"),
        "{sentence}"
    );
    assert!(
        sentence.contains("`eval \"$(aida role enter reviewer)\"`"),
        "{sentence}"
    );
    assert!(
        sentence.contains("`aida role enter reviewer --bind-pid <harness pid>`"),
        "{sentence}"
    );
    // doctor reports the reason instead of a silent "no role".
    // (A host with yama ptrace_scope 0 adds its own finding; ignore it.)
    let findings: Vec<_> = doctor_scan(root)
        .into_iter()
        .filter(|f| !f.id.ends_with(":ptrace-scope"))
        .collect();
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].category, DOCTOR_CATEGORY);
    assert_eq!(findings[0].summary, sentence);
    // Round 3: the action steers to re-entering in the agent's terminal, and
    // never offers --bind-pid as the fix for a (possibly sandboxed) session.
    let action = &findings[0].action;
    assert!(
        action.contains("`eval \"$(aida role enter reviewer)\"`"),
        "{action}"
    );
    assert!(
        action.contains("start the agent session from that terminal"),
        "{action}"
    );
    assert!(!action.contains("harness pid for --bind-pid"), "{action}");
    assert!(
        action.contains("only an already-running session's unsandboxed"),
        "{action}"
    );
    // ... and `role status --json` shows the binding verdict.
    let status = serde_json::to_value(grant_status(root)).unwrap();
    assert_eq!(status["grant"]["valid"], false);
    assert_eq!(status["grant"]["binding_ok"], false);
    assert_eq!(status["grant"]["bound_to"], "no process");
}

#[test]
fn doctor_and_status_are_silent_or_valid_for_a_bound_grant() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env();
    let id = test_support::mint_grant_for(root, SUBJECT, "reviewer", &[]);
    env.set_key(GRANT_ENV, &id);
    assert!(doctor_scan(root)
        .iter()
        .all(|f| f.id.ends_with(":ptrace-scope")));
    let status = serde_json::to_value(grant_status(root)).unwrap();
    assert_eq!(status["grant"]["valid"], true);
    assert_eq!(status["grant"]["binding_ok"], true);
    assert_eq!(status["grant"]["via"], "proc");
    assert_eq!(
        status["grant"]["binding"],
        serde_json::json!([own_process_token().unwrap()])
    );
    with_identity_source(presenter_source(), || {
        let status = serde_json::to_value(grant_status(root)).unwrap();
        assert_eq!(status["harness_pid"], 4300);
        // TASK-2159: the transient shell a command runs in is never offered
        // for --bind-pid. trace:TASK-2159 | ai:claude
        assert!(status.get("shell_pid").is_none(), "{status}");
    });
}

/// Q3a: binding matches are strict — a match GRANTS authority, so a missing
/// start or a scheme mismatch refuses rather than matching.
#[test]
fn binding_match_is_strict() {
    use crate::gate::review_authority::binding_tokens_match as m;
    assert!(m("proc:42@linux-starttime:7", "proc:42@linux-starttime:7"));
    assert!(m("sid:42@linux-starttime:7", "proc:42@linux-starttime:7"));
    assert!(m("shell:42@linux-starttime:7", "proc:42@linux-starttime:7"));
    assert!(!m("proc:42@linux-starttime:7", "proc:42@linux-starttime:8"));
    assert!(!m("proc:42@linux-starttime:7", "proc:43@linux-starttime:7"));
    assert!(
        !m("proc:42@", "proc:42@"),
        "no start identity never matches"
    );
    assert!(!m("proc:42@linux-starttime:7", "proc:42@"));
    assert!(!m(
        "proc:42@2026-10-10T00:00:00Z",
        "proc:42@linux-starttime:7"
    ));
    assert!(!m("proc:1@linux-starttime:7", "proc:1@linux-starttime:7"));
    assert!(!m("session:abc", "session:abc"), "env ids never bind");
    assert!(!m("lease:abc", "lease:abc"));
    // The lenient refusal matcher would have accepted the scheme mismatch.
    assert!(crate::gate::review_authority::tokens_match(
        "proc:42@2026-10-10T00:00:00Z",
        "proc:42@linux-starttime:7"
    ));
}

/// C2: an exec'd (or spawned) worker presents the child grant as the
/// launcher process it was minted bound to; a detached worker is refused
/// until the launcher stamps its pid.
#[test]
fn child_grant_bound_to_launcher_and_detached_worker_after_stamp() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env();
    let advisor = test_support::mint_grant_for(root, SUBJECT, "advisor", &["reviewer"]);
    env.set_key(GRANT_ENV, &advisor);
    let child = issue_child(root, "reviewer", SUBJECT).unwrap();
    // The launcher's process, then (where the keyring works, SKETCH v2) the
    // key the launcher holds for its worker.
    assert_eq!(child.binding[0], own_process_token().unwrap());
    assert!(
        child.binding[1..].iter().all(|b| b.starts_with("key:")),
        "{:?}",
        child.binding
    );
    let minted = child.binding.len();

    // Exec launch: the worker IS the minting process.
    env.set_key(GRANT_ENV, &child.id);
    assert_eq!(current_seat(root).as_deref(), Some("reviewer"));

    // Detached launch: the worker's ancestry no longer holds the launcher.
    let worker = vec!["proc:777777@linux-starttime:77".to_string()];
    let record = read_grant(&child.id).unwrap();
    assert!(matches!(
        presenter_rejection(&record, &worker),
        Some(GrantRejection::PresenterNotBound { .. })
    ));
    bind_grant_token(&child.id, &worker[0]).unwrap();
    let record = read_grant(&child.id).unwrap();
    assert_eq!(presenter_rejection(&record, &worker), None);
    assert_eq!(matched_binding(&record, &worker), worker);
    // Stamping is idempotent.
    bind_grant_token(&child.id, &worker[0]).unwrap();
    assert_eq!(read_grant(&child.id).unwrap().binding.len(), minted + 1);
    // A pid with no readable start identity cannot be stamped.
    bind_grant_to_pid(&child.id, u32::MAX - 1).expect_err("no such process");
}

/// C4: concurrent stamps (and a revocation) on one grant all land.
#[test]
fn concurrent_binding_stamps_never_lose_a_pid() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env();
    let id = test_support::mint_grant_for(root, SUBJECT, "reviewer", &[]);
    env.set_key(GRANT_ENV, &id);
    let tokens = |lane: u32| -> Vec<String> {
        (0..15)
            .map(|i| format!("proc:{}@linux-starttime:{i}", 10_000 + lane * 100 + i))
            .collect()
    };
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let workers: Vec<_> = (0..3)
        .map(|lane| {
            let (id, barrier, mine) = (id.clone(), barrier.clone(), tokens(lane));
            std::thread::spawn(move || {
                barrier.wait();
                for t in &mine {
                    bind_grant_token(&id, t).unwrap();
                }
            })
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }
    let binding = read_grant(&id).unwrap().binding;
    for lane in 0..3 {
        for t in tokens(lane) {
            assert!(binding.contains(&t), "lost {t}");
        }
    }
    assert!(binding.contains(&own_process_token().unwrap()));
    // A revocation racing a stamp is never written back over.
    let stamp = {
        let id = id.clone();
        std::thread::spawn(move || bind_grant_token(&id, "proc:20000@linux-starttime:1").unwrap())
    };
    assert!(revoke_by_id(&id).unwrap());
    stamp.join().unwrap();
    let after = read_grant(&id).unwrap();
    assert!(after.revoked_at.is_some(), "revocation survives the stamp");
    assert!(after
        .binding
        .contains(&"proc:20000@linux-starttime:1".to_string()));
}

#[test]
fn issue_direct_binds_parent_shell_and_session_leader() {
    let caller = IdentitySource {
        session_ids: Vec::new(),
        chain: vec![
            proc(100, "aida", "linux-starttime:10"),
            proc(90, "bash", "linux-starttime:9"),
        ],
        session_leader: Some(proc(80, "", "linux-starttime:8")),
    };
    assert_eq!(
        direct_issue_binding(&caller).unwrap(),
        vec![
            "proc:90@linux-starttime:9".to_string(),
            "sid:80@linux-starttime:8".to_string()
        ]
    );
    // The shell leads its own session: one token.
    let mut leader_shell = caller.clone();
    leader_shell.session_leader = Some(proc(90, "", "linux-starttime:9"));
    assert_eq!(
        direct_issue_binding(&leader_shell).unwrap(),
        vec!["proc:90@linux-starttime:9".to_string()]
    );
    // An unreadable shell start identity mints nothing.
    let mut unreadable = caller.clone();
    unreadable.chain[1].start = None;
    let err = direct_issue_binding(&unreadable).unwrap_err().to_string();
    assert!(err.contains("no grant was issued"), "{err}");
}

/// Q1: `--bind-pid` accepts a live same-user harness or shell in (or below)
/// the caller's own terminal session and nothing broader.
#[test]
fn bind_pid_target_rules() {
    let caller = IdentitySource {
        session_ids: Vec::new(),
        chain: vec![
            proc(100, "aida", "linux-starttime:10"),
            proc(90, "zsh", "linux-starttime:9"),
            proc(70, "tmux: server", "linux-starttime:7"),
            proc(60, "systemd", "linux-starttime:6"),
        ],
        session_leader: Some(proc(90, "", "linux-starttime:9")),
    };
    let target = |pid: u32, name: &str, uid: u32| BindTarget {
        entry: proc(pid, name, &format!("linux-starttime:{pid}")),
        uid: Some(uid),
    };
    let ok = |pid: u32, t: &BindTarget| check_bind_target(pid, Some(t), &caller, Some(1000));
    let err = |pid: u32, t: Option<&BindTarget>| {
        check_bind_target(pid, t, &caller, Some(1000))
            .unwrap_err()
            .to_string()
    };

    let claude = target(4300, "claude", 1000);
    assert_eq!(ok(4300, &claude).unwrap(), "proc:4300@linux-starttime:4300");
    assert!(
        ok(5500, &target(5500, "-bash", 1000)).is_ok(),
        "login shell"
    );
    assert!(ok(5600, &target(5600, "codex", 1000)).is_ok());
    assert!(
        ok(90, &target(90, "zsh", 1000)).is_ok(),
        "own session leader"
    );

    assert!(err(1, Some(&target(1, "systemd", 0))).contains("pid 1"));
    assert!(err(4300, None).contains("no running process"));
    assert!(err(4300, Some(&target(4300, "claude", 0))).contains("does not belong"));
    assert!(err(4400, Some(&target(4400, "python3", 1000))).contains("neither an agent harness"));
    // BUG-b6b1278890774ef993d2ebc327862470-002: Claude Code's versioned
    // install runs with the version string as its process name.
    let mut versioned = target(4500, "2.1.310", 1000);
    versioned.entry.cmd = vec![
        "/home/u/.local/share/claude/versions/2.1.310".into(),
        "--session-id".into(),
    ];
    assert!(ok(4500, &versioned).is_ok());
    assert!(err(4600, Some(&target(4600, "2.1.310", 1000))).contains("neither an agent harness"));
    assert!(
        err(70, Some(&target(70, "bash", 1000))).contains("above this terminal session"),
        "an ancestor above the caller's session"
    );
    let mut no_start = claude.clone();
    no_start.entry.start = None;
    assert!(err(4300, Some(&no_start)).contains("start identity"));
    assert!(
        check_bind_target(4300, Some(&claude), &caller, None).is_err(),
        "an unknown caller uid refuses"
    );
}

/// The ship gate's grant re-validation requires the presented binding.
#[test]
fn grant_authorized_at_requires_a_matching_presented_binding() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = pinned_env();
    let id = test_support::mint_grant_for(root, SUBJECT, "reviewer", &[]);
    let own = vec![own_process_token().unwrap()];
    let now = Utc::now();
    assert!(grant_authorized_at(root, &id, "reviewer", now, &own));
    assert!(!grant_authorized_at(root, &id, "reviewer", now, &[]));
    assert!(!grant_authorized_at(
        root,
        &id,
        "reviewer",
        now,
        &[FOREIGN.to_string()]
    ));
    set_binding(&id, &[]);
    assert!(
        !grant_authorized_at(root, &id, "reviewer", now, &own),
        "an unbound grant authorizes nothing"
    );
}

/// Q3b: an advisor seat is live only when its turn stamp came from a process
/// the grant is bound to.
#[test]
fn advisor_live_requires_the_stamped_binding() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env();
    // advisor_live scans every grant in the home: use a private one.
    let home = tempfile::tempdir().unwrap();
    env.set_key("AIDA_HOME", &home.path().display().to_string());
    let id = test_support::mint_grant_for(root, SUBJECT, crate::gate::mandate::MANDATE_SEAT, &[]);
    env.set_key(GRANT_ENV, &id);
    let now = Utc::now();
    set_binding(&id, &[FOREIGN]);
    crate::mandate_drive::stamp_grant_turn(Some(&id), now);
    assert!(
        !crate::mandate_drive::advisor_live(root, now),
        "a stamp from outside the binding is not a live seat"
    );
    set_binding(&id, &[&own_process_token().unwrap()]);
    crate::mandate_drive::stamp_grant_turn(Some(&id), now);
    assert!(crate::mandate_drive::advisor_live(root, now));
}

/// TASK-2210: a turn from a presenter the grant is not bound to (a daemon
/// lane holding a leaked grant id) leaves the bound session's stamp alone,
/// so the advisor seat stays live instead of flapping off.
// trace:TASK-1955 | ai:claude (TASK-2210)
#[test]
fn an_unbound_presenter_does_not_overwrite_the_bound_turn_stamp() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env();
    let home = tempfile::tempdir().unwrap();
    env.set_key("AIDA_HOME", &home.path().display().to_string());
    let id = test_support::mint_grant_for(root, SUBJECT, crate::gate::mandate::MANDATE_SEAT, &[]);
    env.set_key(GRANT_ENV, &id);
    set_binding(&id, &["proc:4242@linux-starttime:424"]);
    let now = Utc::now();
    with_identity_source(presenter_source(), || {
        crate::mandate_drive::stamp_grant_turn(Some(&id), now)
    });
    assert!(crate::mandate_drive::advisor_live(root, now));
    let daemon_lane = IdentitySource {
        session_ids: vec!["daemon-session".into()],
        chain: vec![
            proc(7000, "aida", "linux-starttime:700"),
            proc(6800, "claude-daemon", "linux-starttime:680"),
        ],
        session_leader: Some(proc(6800, "claude-daemon", "linux-starttime:680")),
    };
    let later = now + chrono::Duration::minutes(1);
    with_identity_source(daemon_lane, || {
        crate::mandate_drive::stamp_grant_turn(Some(&id), later)
    });
    assert!(
        crate::mandate_drive::advisor_live(root, later),
        "the unbound presenter's turn must not clear the bound stamp"
    );
}

#[test]
fn bg_launch_refuses_a_seat_it_cannot_bind() {
    let err = crate::gate::seat_authority::bg_seat_refusal("reviewer")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("`--bg` cannot launch a `reviewer` session"),
        "{err}"
    );
    assert!(err.contains("aida role enter reviewer"), "{err}");
    assert!(err.contains("`--bind-pid` would serve only"), "{err}");
    crate::gate::seat_authority::bg_seat_refusal("implementer").unwrap();
}

/// TASK-2158: `aida review record` driven end to end with an exported grant
/// whose binding names another process tree: refused with the binding
/// sentence, and no verdict file is written.
// trace:TASK-2158 | ai:claude
#[test]
fn review_record_refuses_a_presenter_not_bound_grant() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut env = pinned_env();
    let id = test_support::mint_grant_for(root, SUBJECT, "reviewer", &[]);
    set_binding(&id, &[FOREIGN]);
    env.set_key(GRANT_ENV, &id);
    let err = crate::handle_review_record_at(
        root.to_path_buf(),
        "BUG-1",
        "approved",
        Some("0123456789abcdef0123456789abcdef01234567"),
        Some("some-branch"),
        None,
        &[],
        &[],
        None,
    )
    .expect_err("a grant presented outside its process tree must not record a verdict");
    let message = format!("{err:#}");
    assert!(message.starts_with("refused: review record."), "{message}");
    assert!(
        message.contains(&id[..8]) && !message.contains(&id),
        "{message}"
    );
    assert!(
        message.contains("it is bound to process 999999, and this process does not run under it"),
        "{message}"
    );
    assert!(message.contains("aida role enter reviewer"), "{message}");
    let verdicts = root.join(".aida").join("review-verdicts");
    let written: Vec<_> = std::fs::read_dir(&verdicts)
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    assert!(written.is_empty(), "no verdict may be written: {written:?}");
}

/// Round 2: a bare `aida role enter` whose stdout is the terminal would
/// print the session-key secret; it refuses before minting, naming the eval
/// form. `--bind-pid` prints no secret and is unaffected by this check.
// trace:BUG-1934 | ai:claude
#[test]
fn role_enter_refuses_to_print_the_secret_to_a_terminal() {
    if !crate::grant_keyring::available() {
        eprintln!("SKIP: no session keyring, so no secret is minted");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HEADLESS", None),
        ("AIDA_AGENT_NAME", None),
        ("AIDA_AGENT_TYPE", None),
        ("AIDA_USER", Some("human-operator")),
        ("AIDA_HOME", Some(dir.path().to_str().unwrap())),
    ]);
    let prev = crate::test_ambient::replace(Some(crate::test_ambient::Ambient {
        project_root: dir.path().to_path_buf(),
        stdin_is_terminal: true,
        stdout_is_terminal: true,
        stderr_is_terminal: true,
    }));
    let err = issue_direct(dir.path(), "reviewer", Vec::new(), None, None)
        .map(|_| ())
        .unwrap_err()
        .to_string();
    crate::test_ambient::replace(prev);
    assert!(
        err.contains("eval \"$(aida role enter reviewer)\""),
        "{err}"
    );
    let grants = dir.path().join(".aida").join("session-grants");
    assert!(
        std::fs::read_dir(&grants).map_or(true, |mut d| d.next().is_none()),
        "no grant may be minted"
    );
}

/// Round 2: revoking a grant removes its record lock, and the sweep removes
/// the locks of expired or vanished grants while keeping a live one's.
// trace:BUG-1934 | ai:claude
#[test]
fn revoke_removes_the_grant_lock_and_sweeps_stale_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _env = pinned_env();
    let live = test_support::mint_grant_for(root, SUBJECT, "reviewer", &[]);
    let gone = test_support::mint_grant_for(root, SUBJECT, "reviewer", &[]);
    bind_grant_token(&live, &own_process_token().unwrap()).unwrap();
    bind_grant_token(&gone, &own_process_token().unwrap()).unwrap();
    let locks = grant_dir()
        .unwrap()
        .parent()
        .unwrap()
        .join("session-grant-locks");
    let lock = |id: &str| locks.join(format!("{id}.lock"));
    std::fs::write(lock(&gone), "").unwrap();
    std::fs::write(lock("00000000-0000-4000-8000-00000000dead"), "").unwrap();
    assert!(revoke_by_id(&gone).unwrap());
    assert!(!lock(&gone).exists(), "the revoked grant's lock is removed");
    assert!(
        !lock("00000000-0000-4000-8000-00000000dead").exists(),
        "a lock without a grant is swept"
    );
    assert!(lock(&live).exists(), "a live grant's lock stays");
}
