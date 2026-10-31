//! Deputy spool, policy, floor and worker tests.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude

use super::policy::{Policy, TtyDecision};
use super::*;
use chrono::Duration as CDuration;

fn spool_in(dir: &tempfile::TempDir) -> Spool {
    let s = spool_for(dir.path(), "test").expect("name");
    s.ensure().expect("spool");
    s
}

fn requester(session: &str) -> Requester {
    Requester {
        session_id: Some(session.into()),
        role: Some("implementer".into()),
        pid: 1,
        ..Default::default()
    }
}

fn job(body: JobBody, serial: Option<&str>, timeout_secs: u64) -> JobRecord {
    JobRecord {
        id: new_job_id(),
        created_at: Utc::now(),
        requester: requester("sess-test"),
        serial: serial.map(str::to_string),
        timeout_secs,
        sha256: body.digest(),
        body,
        delegation: None,
        extra: Default::default(),
    }
}

fn sh(cmd: &str) -> JobBody {
    // The worker tests point `aida` at bash, so `aida -c CMD` is `bash -c CMD`.
    JobBody::Argv(vec!["aida".into(), "-c".into(), cmd.into()])
}

// ── spool: atomic submit, refusal table, exactly-once claim ─────────────────

#[test]
fn parallel_submits_never_clobber() {
    let dir = tempfile::tempdir().unwrap();
    let spool = spool_in(&dir);
    let handles: Vec<_> = (0..50)
        .map(|i| {
            let spool = spool.clone();
            std::thread::spawn(move || {
                spool
                    .submit(&job(sh(&format!("echo {i}")), None, 60))
                    .unwrap()
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(spool.list_ids("incoming").len(), 50);
}

#[test]
fn submit_refuses_an_existing_id() {
    let dir = tempfile::tempdir().unwrap();
    let spool = spool_in(&dir);
    let rec = job(sh("true"), None, 60);
    spool.submit(&rec).unwrap();
    assert!(spool.submit(&rec).is_err());
}

#[test]
fn a_job_is_claimed_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let spool = spool_in(&dir);
    let rec = job(sh("true"), None, 60);
    spool.submit(&rec).unwrap();
    let from = spool.job_path("incoming", &rec.id);
    let wins: usize = (0..4)
        .map(|_| {
            let (from, to) = (from.clone(), spool.job_path("running", &rec.id));
            std::thread::spawn(move || std::fs::rename(from, to).is_ok() as usize)
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .sum();
    // rename(2) of a vanished source fails, so one claimer wins (or, at most,
    // later renames find nothing to move).
    assert!(wins >= 1);
    assert!(!from.exists());
    assert!(spool.job_path("running", &rec.id).exists());
}

#[cfg(unix)]
#[test]
fn job_file_refusal_table() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let spool = spool_in(&dir);

    let ok = job(sh("true"), None, 60);
    let p = spool.submit(&ok).unwrap();
    assert!(read_job_file(&p, &ok.id).is_ok());

    let w = job(sh("true"), None, 60);
    let p = spool.submit(&w).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert_eq!(read_job_file(&p, &w.id), Err(JobFileRefusal::Writable));

    let l = job(sh("true"), None, 60);
    let p = spool.submit(&l).unwrap();
    std::fs::hard_link(&p, dir.path().join("extra-link")).unwrap();
    assert_eq!(read_job_file(&p, &l.id), Err(JobFileRefusal::Linked));

    let s = new_job_id();
    let target = dir.path().join("planted.json");
    std::fs::write(&target, b"{}").unwrap();
    let sp = spool.job_path("incoming", &s);
    std::os::unix::fs::symlink(&target, &sp).unwrap();
    assert_eq!(read_job_file(&sp, &s), Err(JobFileRefusal::NotRegular));

    let m = new_job_id();
    let mp = spool.job_path("incoming", &m);
    std::fs::write(&mp, b"not json").unwrap();
    std::fs::set_permissions(&mp, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        read_job_file(&mp, &m),
        Err(JobFileRefusal::Malformed(_))
    ));

    // A body edited after submit no longer matches its digest.
    let mut t = job(sh("true"), None, 60);
    t.body = sh("echo tampered");
    let tp = spool.submit(&t).unwrap();
    assert_eq!(read_job_file(&tp, &t.id), Err(JobFileRefusal::IdMismatch));
}

#[cfg(unix)]
#[test]
fn spool_open_to_group_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let spool = spool_in(&dir);
    std::fs::set_permissions(&spool.root, std::fs::Permissions::from_mode(0o770)).unwrap();
    assert!(spool.check_owned().is_err(), "the deputy stops claiming");
    std::fs::set_permissions(&spool.root, std::fs::Permissions::from_mode(0o700)).unwrap();
    for open in [
        spool.dir("incoming"),
        spool.dir("incoming").parent().unwrap().to_path_buf(),
    ] {
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o770)).unwrap();
        assert!(spool.check_owned().is_err(), "{}", open.display());
        assert!(
            spool.submit(&job(sh("true"), None, 60)).is_err(),
            "{}",
            open.display()
        );
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    assert!(spool.submit(&job(sh("true"), None, 60)).is_ok());
}

#[test]
fn ids_and_names_are_path_safe() {
    assert!(validate_id("../../etc/passwd").is_err());
    assert!(validate_id(&new_job_id()).is_ok());
    assert!(spool_for(std::path::Path::new("/x"), "../evil").is_err());
    // `incoming` names the shared submit directory, never a deputy.
    assert!(spool_for(std::path::Path::new("/x"), "incoming").is_err());
    let s = spool_for(std::path::Path::new("/x"), "ship").unwrap();
    assert_eq!(s.root, std::path::Path::new("/x/.aida/deputy/ship"));
    assert_eq!(
        s.dir("incoming"),
        std::path::Path::new("/x/.aida/deputy/incoming/ship")
    );
}

// ── the contained write boundary: submit side only ──────────────────────────

#[cfg(unix)]
#[test]
fn a_contained_seat_can_submit_but_cannot_write_running_or_the_policy() {
    use crate::contained_boundary::{
        contained_deputy_write_paths, prepare_deputy_incoming, sandbox_allow_write_entries,
    };
    let dir = tempfile::tempdir().unwrap();
    let main = std::fs::canonicalize(dir.path()).unwrap();
    prepare_deputy_incoming(&main).unwrap();
    let allowed = contained_deputy_write_paths(&main).unwrap();
    assert_eq!(allowed, vec![main.join(".aida/deputy/incoming")]);
    assert_eq!(sandbox_allow_write_entries(&allowed).len(), 1);

    let spool = spool_for(&main, DEFAULT_NAME).unwrap();
    spool.ensure().unwrap();
    let writable = |p: &std::path::Path| allowed.iter().any(|a| p.starts_with(a));
    let id = new_job_id();
    assert!(writable(&spool.job_path("incoming", &id)), "submit");
    assert!(writable(&spool.cancel_request_path(&id)), "cancel");
    for p in [
        spool.dir("running"),
        spool.job_path("running", &id),
        spool.tty_marker_path(&id),
        spool.waiting_marker_path(&id),
        spool.dir("done"),
        spool.dir("failed"),
        spool.out_path(&id),
        spool.policy_path(),
        spool.ledger_path(),
        spool.lock_path(),
    ] {
        assert!(!writable(&p), "{} must stay unwritable", p.display());
    }

    // The whole submit touches only the allowed path.
    let rec = job(sh("true"), None, 60);
    let written = spool.submit(&rec).unwrap();
    assert!(writable(&written));
    spool.request_cancel(&rec.id).unwrap();
    assert!(spool.cancel_requested(&rec.id));
}

#[cfg(unix)]
#[test]
fn a_symlinked_deputy_directory_is_never_bound() {
    let dir = tempfile::tempdir().unwrap();
    let main = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(main.join(".aida")).unwrap();
    let elsewhere = main.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, main.join(".aida/deputy")).unwrap();
    assert!(crate::contained_boundary::contained_deputy_write_paths(&main).is_err());
}

// ── policy ──────────────────────────────────────────────────────────────────

fn argv(s: &str) -> JobBody {
    JobBody::Argv(s.split_whitespace().map(str::to_string).collect())
}

#[test]
fn policy_prefix_ttl_single_use_and_grant_cap() {
    let now = Utc::now();
    let mut p = Policy::default();
    let e = p
        .allow(
            policy::parse_prefix("aida merge-hold clear"),
            3600,
            false,
            now,
        )
        .unwrap();
    assert!(matches!(
        p.decide(&argv("aida merge-hold clear 3120"), now, None),
        TtyDecision::Tty { ref entry, .. } if *entry == e.id
    ));
    assert_eq!(
        p.decide(&argv("aida merge-hold set 3120"), now, None),
        TtyDecision::NoTty
    );
    assert_eq!(
        p.decide(&argv("aida merge-hold"), now, None),
        TtyDecision::NoTty
    );
    // Expired.
    assert_eq!(
        p.decide(
            &argv("aida merge-hold clear 1"),
            now + CDuration::hours(2),
            None
        ),
        TtyDecision::NoTty
    );
    // The grant caps the entry's expiry.
    let grant = now + CDuration::minutes(5);
    match p.decide(&argv("aida merge-hold clear 1"), now, Some(grant)) {
        TtyDecision::Tty { expires_at, .. } => assert_eq!(expires_at, grant),
        other => panic!("expected tty, got {other:?}"),
    }
    assert_eq!(
        p.decide(
            &argv("aida merge-hold clear 1"),
            now,
            Some(now - CDuration::seconds(1))
        ),
        TtyDecision::NoTty
    );

    // Single use is consumed by the first match.
    let s = p
        .allow(policy::parse_prefix("aida pr ship 3120"), 3600, true, now)
        .unwrap();
    assert!(p.consume(&s.id, "job-1", now));
    assert_eq!(
        p.decide(&argv("aida pr ship 3120"), now, None),
        TtyDecision::NoTty
    );
}

#[test]
fn script_jobs_never_get_a_terminal_and_bypass_flags_are_refused() {
    let now = Utc::now();
    let mut p = Policy::default();
    p.allow(policy::parse_prefix("aida"), 3600, false, now)
        .unwrap();
    assert_eq!(
        p.decide(
            &JobBody::Script("aida merge-hold clear 1".into()),
            now,
            None
        ),
        TtyDecision::NoTty
    );
    assert_eq!(
        p.decide(&argv("aida pr ship 1 --force"), now, None),
        TtyDecision::RefuseBypass("--force".into())
    );
    assert_eq!(
        p.decide(&argv("aida pr ship 1 --no-verify=true"), now, None),
        TtyDecision::RefuseBypass("--no-verify=true".into())
    );
    assert_eq!(
        p.decide(&argv("aida pr ship 1 --override-stale-approval"), now, None),
        TtyDecision::RefuseBypass("--override-stale-approval".into())
    );
    assert!(p
        .allow(policy::parse_prefix("aida pr ship --force"), 60, false, now)
        .is_err());
    assert!(p.allow(vec![], 60, false, now).is_err());
}

// ── single-use entries fail closed ──────────────────────────────────────────

#[cfg(unix)]
#[test]
fn a_single_use_entry_fails_closed_when_its_use_cannot_be_recorded() {
    use crate::deputy::worker::claim_terminal;
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let spool = spool_in(&dir);
    let now = Utc::now();
    let mut pol = Policy::default();
    let e = pol
        .allow(policy::parse_prefix("aida -c"), 600, true, now)
        .unwrap();
    policy::save(&spool, &pol).unwrap();
    let decide = || policy::load(&spool).unwrap().decide(&sh("true"), now, None);
    assert!(matches!(
        decide(),
        TtyDecision::Tty {
            single_use: true,
            ..
        }
    ));

    // Save fails: a directory squats on the policy's temp path.
    let squat = spool.root.join(".deputy.policy.tmp");
    std::fs::create_dir(&squat).unwrap();
    assert_eq!(
        claim_terminal(&spool, "t", &new_job_id(), decide(), now),
        None
    );
    let after = policy::load(&spool).unwrap();
    assert!(after.entries[0].consumed_at.is_none(), "entry was not used");
    std::fs::remove_dir(&squat).unwrap();

    // Reload fails: the policy file is unreadable as a policy.
    let d = decide();
    std::fs::write(spool.policy_path(), b"not json").unwrap();
    std::fs::set_permissions(spool.policy_path(), std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(claim_terminal(&spool, "t", &new_job_id(), d, now), None);

    // With a healthy policy the entry is used exactly once.
    policy::save(&spool, &pol).unwrap();
    let job_id = new_job_id();
    assert_eq!(
        claim_terminal(&spool, "t", &job_id, decide(), now).map(|(entry, _)| entry),
        Some(e.id.clone())
    );
    let used = policy::load(&spool).unwrap();
    assert_eq!(
        used.entries[0].consumed_by.as_deref(),
        Some(job_id.as_str())
    );
    assert_eq!(decide(), TtyDecision::NoTty);
    // A decision taken before the use cannot be replayed either.
    let stale = TtyDecision::Tty {
        entry: e.id,
        expires_at: now + CDuration::minutes(5),
        single_use: true,
    };
    assert_eq!(claim_terminal(&spool, "t", &new_job_id(), stale, now), None);
}

#[test]
fn deny_removes_by_id_or_prefix() {
    let now = Utc::now();
    let mut p = Policy::default();
    let a = p
        .allow(policy::parse_prefix("aida a"), 60, false, now)
        .unwrap();
    p.allow(policy::parse_prefix("aida b"), 60, false, now)
        .unwrap();
    assert_eq!(p.deny(&a.id), 1);
    assert_eq!(p.deny("aida b"), 1);
    assert!(p.entries.is_empty());
}

// ── Q6: the integrity floor under a deputy job ──────────────────────────────

#[cfg(unix)]
#[test]
fn floor_needs_a_live_marker_for_this_job() {
    use crate::gate::kernel_deps::deputy_job_floor_authorized_from as job_floor_authorized_from;
    let dir = tempfile::tempdir().unwrap();
    let spool = spool_in(&dir);
    let root = spool.root.to_string_lossy().to_string();
    let now = Utc::now();
    let id = new_job_id();

    assert!(job_floor_authorized_from(None, None, now), "outside a job");
    assert!(!job_floor_authorized_from(Some(&id), None, now), "no spool");
    assert!(
        !job_floor_authorized_from(Some(&id), Some(&root), now),
        "no marker"
    );

    write_tty_marker(
        &spool,
        &bound_marker(&spool, &id, now + CDuration::minutes(10)),
    )
    .unwrap();
    assert!(job_floor_authorized_from(Some(&id), Some(&root), now));
    assert!(
        !job_floor_authorized_from(Some(&id), Some(&root), now + CDuration::hours(1)),
        "expired marker"
    );
    let other = new_job_id();
    assert!(!job_floor_authorized_from(Some(&other), Some(&root), now));
    assert!(!job_floor_authorized_from(Some("../x"), Some(&root), now));
}

/// A live marker bound to `running/<id>.job` (created if absent), as the
/// worker writes it at claim.
#[cfg(unix)]
fn bound_marker(spool: &Spool, id: &str, expires_at: DateTime<Utc>) -> TtyMarker {
    let job_file = spool.job_path("running", id);
    if !job_file.exists() {
        write_private_new(&job_file, b"{}").unwrap();
    }
    let md = std::fs::symlink_metadata(&job_file).unwrap();
    let (job_dev, job_ino) = crate::gate::kernel_deps::deputy_file_identity(&md);
    TtyMarker {
        job: id.to_string(),
        entry: "e1".into(),
        expires_at,
        job_dev,
        job_ino,
    }
}

/// Ruling test (c): a marker authorizes only the job file it was written
/// for. Another job id never matches it, and neither does a different file
/// put at the same `running/<id>.job` (a forged resubmission of the id).
#[cfg(unix)]
#[test]
fn a_marker_for_job_a_never_authorizes_job_b() {
    use crate::gate::kernel_deps::{
        deputy_job_floor_authorized_from as authorized, deputy_running_job_path,
    };
    let dir = tempfile::tempdir().unwrap();
    let spool = spool_in(&dir);
    let root = spool.root.to_string_lossy().to_string();
    let now = Utc::now();
    let a = new_job_id();
    assert_eq!(
        deputy_running_job_path(&spool.root, &a),
        spool.job_path("running", &a),
        "the floor reads the file the worker claims"
    );
    write_tty_marker(
        &spool,
        &bound_marker(&spool, &a, now + CDuration::minutes(10)),
    )
    .unwrap();
    assert!(authorized(Some(&a), Some(&root), now));

    // Job B under its own id, with A's marker copied to B's name.
    let b = new_job_id();
    write_private_new(&spool.job_path("running", &b), b"{}").unwrap();
    assert!(!authorized(Some(&b), Some(&root), now), "B has no marker");
    let mut copied = bound_marker(&spool, &a, now + CDuration::minutes(10));
    copied.job = b.clone();
    write_tty_marker(&spool, &copied).unwrap();
    assert!(
        !authorized(Some(&b), Some(&root), now),
        "A's marker, renamed for B, is bound to A's file"
    );

    // Job B's file put over A's id: A's marker no longer authorizes anyone.
    let forged = spool.dir("running").join(".forged");
    write_private_new(&forged, b"{}").unwrap();
    std::fs::rename(&forged, spool.job_path("running", &a)).unwrap();
    assert!(
        !authorized(Some(&a), Some(&root), now),
        "a different file under A's id does not inherit A's marker"
    );
    // A marker without the binding (the pre-fix format) fails closed.
    std::fs::remove_file(spool.tty_marker_path(&a)).unwrap();
    let legacy = serde_json::json!({
        "job": a, "entry": "e1", "expires_at": now + CDuration::minutes(10)
    });
    write_private_new(&spool.tty_marker_path(&a), legacy.to_string().as_bytes()).unwrap();
    assert!(!authorized(Some(&a), Some(&root), now), "unbound marker");
}

/// Pins the human-terminal probes to a TTY for this thread (the BUG-1618
/// seam), restoring the previous ambient on drop.
struct TerminalAmbient(Option<crate::test_ambient::Ambient>);

impl TerminalAmbient {
    fn pin(root: &std::path::Path) -> Self {
        Self(crate::test_ambient::replace(Some(
            crate::test_ambient::Ambient {
                project_root: root.to_path_buf(),
                stdin_is_terminal: true,
                stdout_is_terminal: true,
                stderr_is_terminal: true,
            },
        )))
    }
}

impl Drop for TerminalAmbient {
    fn drop(&mut self) {
        crate::test_ambient::replace(self.0.take());
    }
}

/// The gate itself, not only its input: with a human terminal on stdin,
/// `has_integrity_floor_authority` still refuses a process that carries a
/// deputy job's environment unless the deputy's marker for that job is live.
#[cfg(unix)]
#[test]
fn integrity_floor_refuses_a_deputy_job_without_a_live_marker() {
    use crate::gate::role_authority::has_integrity_floor_authority;
    let dir = tempfile::tempdir().unwrap();
    let spool = spool_in(&dir);
    let root = spool.root.to_string_lossy().to_string();
    let id = new_job_id();
    let _terminal = TerminalAmbient::pin(dir.path());

    {
        let _env = crate::test_env::EnvVarsGuard::apply(&[(JOB_ENV, None), (SPOOL_ENV, None)]);
        assert!(has_integrity_floor_authority(), "a human outside any job");
    }

    let _env =
        crate::test_env::EnvVarsGuard::apply(&[(JOB_ENV, Some(&id)), (SPOOL_ENV, Some(&root))]);
    assert!(
        !has_integrity_floor_authority(),
        "a deputy job on a terminal with no marker holds no floor"
    );
    write_tty_marker(
        &spool,
        &bound_marker(&spool, &new_job_id(), Utc::now() + CDuration::minutes(10)),
    )
    .unwrap();
    assert!(
        !has_integrity_floor_authority(),
        "another job's marker does not count"
    );
    write_tty_marker(
        &spool,
        &bound_marker(&spool, &id, Utc::now() - CDuration::seconds(1)),
    )
    .unwrap();
    assert!(!has_integrity_floor_authority(), "an expired marker");
    write_tty_marker(
        &spool,
        &bound_marker(&spool, &id, Utc::now() + CDuration::minutes(10)),
    )
    .unwrap();
    assert!(
        has_integrity_floor_authority(),
        "the policy-authorized job holds the floor"
    );
}

// ── start-path refusals ─────────────────────────────────────────────────────

#[test]
fn start_refuses_without_a_human_terminal_or_the_shell_function() {
    use crate::deputy_cmd::HumanTerminal;
    let ok = HumanTerminal {
        ttys: [true; 3],
        agent: false,
        headless: false,
        in_deputy_job: false,
        via_wrapper: true,
        wrapper_marker: Some("role,session,eval-block,deputy".into()),
    };
    let with = |f: &dyn Fn(&mut HumanTerminal)| {
        let mut h = ok.clone();
        f(&mut h);
        h
    };
    assert!(ok.refusal(true).is_none());
    for i in 0..3 {
        assert!(
            with(&|h| h.ttys[i] = false).refusal(true).is_some(),
            "tty {i}"
        );
    }
    assert!(with(&|h| h.agent = true).refusal(true).is_some(), "agent");
    assert!(
        with(&|h| h.headless = true).refusal(true).is_some(),
        "headless"
    );

    // Inside a deputy job: neither start nor allow/deny.
    let job = with(&|h| h.in_deputy_job = true);
    assert!(job.refusal(true).unwrap().contains("deputy job"));
    assert!(job.refusal(false).unwrap().contains("deputy job"));

    // A bare binary, with no shell function at all.
    let bare = with(&|h| {
        h.via_wrapper = false;
        h.wrapper_marker = None;
    })
    .refusal(true)
    .expect("bare binary refused");
    assert!(bare.contains("aida deputy") && bare.contains("shell-init"));
    // An older shell function that predates `deputy` gets the update hint.
    let stale = with(&|h| {
        h.via_wrapper = false;
        h.wrapper_marker = Some("role,session,dev,eval-block,zsh-localopts".into());
    })
    .refusal(true)
    .expect("stale wrapper refused");
    assert!(stale.contains("predates") && stale.contains("shell-init --install"));

    // allow/deny need the terminal, not the shell function.
    assert!(with(&|h| h.via_wrapper = false).refusal(false).is_none());
}

#[test]
fn the_installed_shell_function_advertises_deputy() {
    let helpers = crate::dev_cmd::SHELL_HELPERS;
    let line = helpers
        .lines()
        .find(|l| l.starts_with("export AIDA_SHELL_WRAPPER="))
        .expect("capability line");
    assert!(
        crate::shell_eval::marker_has_cap(
            Some(
                line.trim_start_matches("export AIDA_SHELL_WRAPPER=")
                    .trim_matches('\'')
            ),
            WRAPPER_CAP
        ),
        "{line}"
    );
    assert!(
        helpers.contains("\"deputy \"*"),
        "the deputy arm routes the call"
    );
}

// ── worker fixtures (real child processes) ──────────────────────────────────

#[cfg(unix)]
mod worker_fixtures {
    use super::*;
    use crate::deputy::worker::{self, GrantState, WorkerConfig};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    fn cfg(max_jobs: usize) -> WorkerConfig {
        WorkerConfig {
            name: "test".into(),
            workers: 2,
            allow_exec: vec![],
            allow_scripts: false,
            tty_idle: Duration::from_secs(2),
            poll: Duration::from_millis(50),
            grant: Arc::new(|| GrantState::Valid {
                expires_at: Some(Utc::now() + CDuration::hours(1)),
            }),
            mandate: worker::no_mandate_stop(),
            mail_root: None,
            max_jobs: Some(max_jobs),
            aida_program: "bash".into(),
        }
    }

    fn run(spool: &Spool, cfg: &WorkerConfig) {
        let stop = AtomicBool::new(false);
        worker::run(spool, cfg, &stop).unwrap();
    }

    fn out(spool: &Spool, id: &str) -> String {
        std::fs::read_to_string(spool.out_path(id)).unwrap_or_default()
    }

    #[test]
    fn exit_in_a_job_does_not_stop_the_loop() {
        let dir = tempfile::tempdir().unwrap();
        let spool = spool_in(&dir);
        let a = job(sh("echo before; exit 3"), None, 30);
        spool.submit(&a).unwrap();
        let b = job(sh("echo hi"), None, 30);
        spool.submit(&b).unwrap();
        run(&spool, &cfg(2));

        let ra = spool.read_result(&a.id).unwrap();
        assert_eq!(ra.rc, 3);
        assert!(spool.job_path("failed", &a.id).exists());
        assert!(out(&spool, &a.id).contains("before"));
        let rb = spool.read_result(&b.id).unwrap();
        assert_eq!(rb.rc, 0);
        assert!(spool.job_path("done", &b.id).exists());
        assert_eq!(out(&spool, &b.id).trim(), "hi");
        assert!(!rb.tty);

        // AC5: every row of these jobs names the requester session.
        let ledger = std::fs::read_to_string(spool.ledger_path()).unwrap();
        let rows: Vec<LedgerRow> = ledger
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let finished: Vec<_> = rows.iter().filter(|r| r.event == "finished").collect();
        assert_eq!(finished.len(), 2);
        for r in finished {
            assert_eq!(
                r.requester.as_ref().unwrap().session_id.as_deref(),
                Some("sess-test")
            );
            assert!(r.sha256.is_some());
        }
    }

    #[test]
    fn timeout_kills_the_job() {
        let dir = tempfile::tempdir().unwrap();
        let spool = spool_in(&dir);
        let a = job(sh("sleep 30"), None, 1);
        spool.submit(&a).unwrap();
        let t = std::time::Instant::now();
        run(&spool, &cfg(1));
        assert!(t.elapsed() < Duration::from_secs(10));
        let r = spool.read_result(&a.id).unwrap();
        assert_eq!(r.rc, RC_TIMEOUT);
        assert!(r.reason.starts_with("timeout"));
    }

    #[test]
    fn serial_class_runs_one_at_a_time_while_others_run_in_parallel() {
        let dir = tempfile::tempdir().unwrap();
        let spool = spool_in(&dir);
        let s1 = job(sh("sleep 1"), Some("ship"), 30);
        spool.submit(&s1).unwrap();
        let s2 = job(sh("sleep 1"), Some("ship"), 30);
        spool.submit(&s2).unwrap();
        let q = job(sh("sleep 1"), None, 30);
        spool.submit(&q).unwrap();
        run(&spool, &cfg(3));
        let r1 = spool.read_result(&s1.id).unwrap();
        let r2 = spool.read_result(&s2.id).unwrap();
        let rq = spool.read_result(&q.id).unwrap();
        assert!(
            r2.started_at.unwrap() >= r1.finished_at,
            "ship jobs overlapped"
        );
        assert!(
            rq.started_at.unwrap() < r1.finished_at,
            "quick job waited on ship"
        );
    }

    #[test]
    fn only_a_policy_matched_job_gets_a_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let spool = spool_in(&dir);
        let now = Utc::now();
        let mut pol = Policy::default();
        pol.allow(policy::parse_prefix("aida -c"), 600, false, now)
            .unwrap();
        policy::save(&spool, &pol).unwrap();

        let probe = "[ -t 0 ] && echo STDIN_TTY || echo STDIN_NOTTY; \
                     [ -f \"$AIDA_DEPUTY_SPOOL/running/$AIDA_DEPUTY_JOB.tty\" ] && echo MARKER || echo NOMARKER; \
                     ( : < /dev/tty ) 2>/dev/null && echo DEVTTY || echo NODEVTTY";
        let matched = job(sh(probe), None, 30);
        spool.submit(&matched).unwrap();
        // Same command, different argv shape: not matched, so no terminal.
        let unmatched = job(
            JobBody::Argv(vec![
                "aida".into(),
                "--norc".into(),
                "-c".into(),
                probe.into(),
            ]),
            None,
            30,
        );
        spool.submit(&unmatched).unwrap();
        run(&spool, &cfg(2));

        let m = out(&spool, &matched.id);
        assert!(m.contains("STDIN_TTY"), "matched job output: {m}");
        assert!(m.contains("MARKER") && !m.contains("NOMARKER"), "{m}");
        assert!(spool.read_result(&matched.id).unwrap().tty);
        assert!(
            !spool.tty_marker_path(&matched.id).exists(),
            "marker removed after the job"
        );

        let u = out(&spool, &unmatched.id);
        assert!(u.contains("STDIN_NOTTY"), "unmatched job output: {u}");
        assert!(u.contains("NOMARKER"), "{u}");
        assert!(
            u.contains("NODEVTTY"),
            "a detached job must not reach a terminal: {u}"
        );
        assert!(!spool.read_result(&unmatched.id).unwrap().tty);
    }

    /// The idle window is display only (advisor ruling 04:24): a quiet
    /// terminal job shows as waiting on a prompt in `status` and keeps
    /// running; only its own timeout stops it.
    #[test]
    fn a_quiet_terminal_job_shows_as_waiting_and_only_its_timeout_stops_it() {
        let dir = tempfile::tempdir().unwrap();
        let spool = spool_in(&dir);
        let mut pol = Policy::default();
        pol.allow(policy::parse_prefix("aida -c"), 600, false, Utc::now())
            .unwrap();
        policy::save(&spool, &pol).unwrap();
        let a = job(
            sh("printf 'Proceed? '; read answer; echo got $answer"),
            None,
            4,
        );
        spool.submit(&a).unwrap();
        let (s, id) = (spool.clone(), a.id.clone());
        let watcher = std::thread::spawn(move || {
            for _ in 0..80 {
                if s.waiting(&id).is_some() {
                    let args = crate::deputy_cmd::DeputyIdArgs {
                        id: id.clone(),
                        name: "test".into(),
                    };
                    let mut buf = Vec::new();
                    crate::deputy_cmd::status(&s, &args, &mut buf).unwrap();
                    return Some(String::from_utf8(buf).unwrap());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            None
        });
        let mut c = cfg(1);
        c.tty_idle = Duration::from_secs(1);
        let t = std::time::Instant::now();
        run(&spool, &c);
        let shown = watcher.join().unwrap().expect("never shown as waiting");
        assert!(shown.contains("running"), "{shown}");
        assert!(shown.contains("waiting on a prompt"), "{shown}");

        let r = spool.read_result(&a.id).unwrap();
        assert_eq!(r.rc, RC_TIMEOUT, "the job's own timeout governs: {r:?}");
        assert!(r.reason.starts_with("timeout"), "{}", r.reason);
        assert!(
            t.elapsed() >= Duration::from_secs(4),
            "stopped before its timeout"
        );
        assert!(out(&spool, &a.id).contains("Proceed?"));
        assert!(spool.waiting(&a.id).is_none(), "waiting marker cleared");
        let ledger = std::fs::read_to_string(spool.ledger_path()).unwrap();
        assert!(ledger.contains("\"waiting\""), "{ledger}");
    }

    #[test]
    fn a_single_use_job_runs_without_a_terminal_when_its_use_cannot_be_saved() {
        let dir = tempfile::tempdir().unwrap();
        let spool = spool_in(&dir);
        let mut pol = Policy::default();
        pol.allow(policy::parse_prefix("aida -c"), 600, true, Utc::now())
            .unwrap();
        policy::save(&spool, &pol).unwrap();
        std::fs::create_dir(spool.root.join(".deputy.policy.tmp")).unwrap();
        let a = job(
            sh("[ -t 0 ] && echo STDIN_TTY || echo STDIN_NOTTY"),
            None,
            30,
        );
        spool.submit(&a).unwrap();
        run(&spool, &cfg(1));
        assert!(out(&spool, &a.id).contains("STDIN_NOTTY"));
        assert!(!spool.read_result(&a.id).unwrap().tty);
        assert!(policy::load(&spool).unwrap().entries[0]
            .consumed_at
            .is_none());
    }

    #[test]
    fn refusals_land_in_failed_with_a_ledger_row() {
        let dir = tempfile::tempdir().unwrap();
        let spool = spool_in(&dir);
        let mut pol = Policy::default();
        pol.allow(policy::parse_prefix("aida -c"), 600, false, Utc::now())
            .unwrap();
        policy::save(&spool, &pol).unwrap();
        let bypass = job(
            JobBody::Argv(vec![
                "aida".into(),
                "-c".into(),
                "true".into(),
                "--force".into(),
            ]),
            None,
            30,
        );
        spool.submit(&bypass).unwrap();
        let script = job(JobBody::Script("echo hi".into()), None, 30);
        spool.submit(&script).unwrap();
        let exec = job(
            JobBody::Argv(vec!["/bin/echo".into(), "hi".into()]),
            None,
            30,
        );
        spool.submit(&exec).unwrap();
        run(&spool, &cfg(3));
        for (j, want) in [
            (&bypass, "refused:bypass-flag"),
            (&script, "refused:script-jobs-disabled"),
            (&exec, "refused:exec-not-allowed"),
        ] {
            let r = spool.read_result(&j.id).unwrap();
            assert_eq!(r.rc, RC_REFUSED);
            assert!(r.reason.starts_with(want), "{}", r.reason);
            assert!(spool.job_path("failed", &j.id).exists());
        }
        let ledger = std::fs::read_to_string(spool.ledger_path()).unwrap();
        assert_eq!(ledger.matches("\"refused\"").count(), 3);
    }

    /// A forged job reusing `id` (an agent can write `incoming/`): same id,
    /// a body that would leave `forged-ran` behind if it ever ran.
    fn forged(spool: &Spool, id: &str) -> JobRecord {
        let mut rec = job(sh("touch \"$AIDA_DEPUTY_SPOOL/forged-ran\""), None, 30);
        rec.id = id.to_string();
        spool.submit(&rec).unwrap();
        rec
    }

    fn duplicate_rows(spool: &Spool, id: &str) -> usize {
        std::fs::read_to_string(spool.ledger_path())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<LedgerRow>(l).ok())
            .filter(|r| {
                r.job == id
                    && r.event == "refused"
                    && r.reason
                        .as_deref()
                        .is_some_and(|w| w.starts_with("refused:duplicate-id"))
            })
            .count()
    }

    fn duplicates_filed(spool: &Spool, id: &str) -> usize {
        std::fs::read_dir(spool.dir("failed"))
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(&format!("{id}.duplicate-"))
            })
            .count()
    }

    /// Ruling test (a): a submission reusing a RUNNING job's id is refused;
    /// the running job's record and terminal marker survive and still
    /// authorize it, and the forgery never runs.
    #[test]
    fn a_duplicate_of_a_running_id_is_refused_and_its_marker_survives() {
        use crate::gate::kernel_deps::deputy_job_floor_authorized_from as authorized;
        let dir = tempfile::tempdir().unwrap();
        let spool = spool_in(&dir);
        let mut pol = Policy::default();
        pol.allow(policy::parse_prefix("aida -c"), 600, false, Utc::now())
            .unwrap();
        policy::save(&spool, &pol).unwrap();
        let real = job(sh("sleep 3; echo real-done"), None, 30);
        spool.submit(&real).unwrap();
        let (s, id) = (spool.clone(), real.id.clone());
        let attacker = std::thread::spawn(move || {
            let root = s.root.to_string_lossy().to_string();
            for _ in 0..100 {
                if s.tty_marker_path(&id).exists() {
                    forged(&s, &id);
                    for _ in 0..100 {
                        if duplicate_rows(&s, &id) == 1 {
                            return Some((
                                s.tty_marker_path(&id).exists(),
                                authorized(Some(&id), Some(&root), Utc::now()),
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            None
        });
        run(&spool, &cfg(2));
        let (marker_kept, still_authorized) =
            attacker.join().unwrap().expect("duplicate never refused");
        assert!(marker_kept, "the running job's marker was deleted");
        assert!(still_authorized, "the running job lost its terminal floor");

        let r = spool.read_result(&real.id).unwrap();
        assert_eq!(r.rc, 0, "{r:?}");
        assert!(r.tty);
        assert!(out(&spool, &real.id).contains("real-done"));
        let kept = read_job_file(&spool.job_path("done", &real.id), &real.id).unwrap();
        assert_eq!(kept.sha256, real.sha256, "the real record was replaced");
        assert!(!spool.root.join("forged-ran").exists(), "the forgery ran");
        assert!(!spool.job_path("incoming", &real.id).exists());
        assert_eq!(duplicates_filed(&spool, &real.id), 1);
        assert!(!spool.result_path("failed", &real.id).exists());
    }

    /// Ruling test (b): a submission reusing a DONE job's id is refused and
    /// the finished job's record and result are untouched.
    #[test]
    fn a_duplicate_of_a_done_id_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let spool = spool_in(&dir);
        let real = job(sh("echo real"), None, 30);
        spool.submit(&real).unwrap();
        run(&spool, &cfg(1));
        assert_eq!(spool.read_result(&real.id).unwrap().rc, 0);

        forged(&spool, &real.id);
        run(&spool, &cfg(1));
        assert_eq!(duplicate_rows(&spool, &real.id), 1);
        assert_eq!(duplicates_filed(&spool, &real.id), 1);
        assert!(!spool.root.join("forged-ran").exists(), "the forgery ran");
        let kept = read_job_file(&spool.job_path("done", &real.id), &real.id).unwrap();
        assert_eq!(kept.sha256, real.sha256);
        let r = spool.read_result(&real.id).unwrap();
        assert_eq!((r.rc, r.reason.as_str()), (0, "ok"));
        assert!(!spool.result_path("failed", &real.id).exists());
        assert_eq!(
            spool.locate(&real.id).unwrap().map(|(s, _)| s),
            Some("done")
        );

        // A cancel request beside the forgery does not file it over the
        // real record either.
        forged(&spool, &real.id);
        spool.request_cancel(&real.id).unwrap();
        run(&spool, &cfg(1));
        assert_eq!(duplicate_rows(&spool, &real.id), 2);
        assert!(!spool.result_path("failed", &real.id).exists());
        assert_eq!(spool.read_result(&real.id).unwrap().rc, 0);
    }

    #[test]
    fn scripts_and_allowed_execs_run_when_the_operator_allows_them() {
        let dir = tempfile::tempdir().unwrap();
        let spool = spool_in(&dir);
        let script = job(JobBody::Script("echo from-script; exit 0".into()), None, 30);
        spool.submit(&script).unwrap();
        let echo = std::fs::canonicalize("/bin/echo").unwrap();
        let exec = job(
            JobBody::Argv(vec![echo.to_string_lossy().to_string(), "from-exec".into()]),
            None,
            30,
        );
        spool.submit(&exec).unwrap();
        let mut c = cfg(2);
        c.allow_scripts = true;
        c.allow_exec = vec![echo];
        run(&spool, &c);
        assert!(out(&spool, &script.id).contains("from-script"));
        assert!(out(&spool, &exec.id).contains("from-exec"));
    }

    #[test]
    fn orphans_are_failed_not_rerun() {
        let dir = tempfile::tempdir().unwrap();
        let spool = spool_in(&dir);
        let a = job(sh("touch \"$AIDA_DEPUTY_SPOOL/ran\""), None, 30);
        spool.submit(&a).unwrap();
        std::fs::rename(
            spool.job_path("incoming", &a.id),
            spool.job_path("running", &a.id),
        )
        .unwrap();
        run(&spool, &cfg(0));
        let r = spool.read_result(&a.id).unwrap();
        assert!(r.reason.starts_with("orphaned"));
        assert!(spool.job_path("failed", &a.id).exists());
        assert!(!spool.root.join("ran").exists());
    }

    #[test]
    fn an_invalid_grant_stops_claiming() {
        let dir = tempfile::tempdir().unwrap();
        let spool = spool_in(&dir);
        let a = job(sh("true"), None, 30);
        spool.submit(&a).unwrap();
        let mut c = cfg(1);
        c.grant = Arc::new(|| GrantState::Invalid("expired".into()));
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = Arc::clone(&stop);
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            s2.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        worker::run(&spool, &c, &stop).unwrap();
        t.join().unwrap();
        assert!(
            spool.job_path("incoming", &a.id).exists(),
            "job was claimed without a grant"
        );
    }

    #[test]
    fn a_second_deputy_with_the_same_name_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let spool = spool_in(&dir);
        let _held = worker::acquire_lock(&spool).unwrap();
        assert!(worker::acquire_lock(&spool).is_err());
    }

    #[test]
    fn a_cancel_request_stops_a_running_job() {
        let dir = tempfile::tempdir().unwrap();
        let spool = spool_in(&dir);
        let a = job(sh("sleep 30"), None, 60);
        spool.submit(&a).unwrap();
        let s = spool.clone();
        let id = a.id.clone();
        let t = std::thread::spawn(move || {
            for _ in 0..100 {
                if s.job_path("running", &id).exists() {
                    s.request_cancel(&id).unwrap();
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        run(&spool, &cfg(1));
        t.join().unwrap();
        let r = spool.read_result(&a.id).unwrap();
        assert_eq!(r.rc, RC_CANCELLED);
        assert_eq!(r.reason, "cancelled");
        assert!(!spool.cancel_requested(&a.id), "request cleared");
    }

    // ── AC4: the commands a session uses ───────────────────────────────────

    mod commands {
        use super::*;
        use crate::deputy_cmd::{self, DeputyArgs, DeputyCommand};
        use clap::Parser;

        fn parse(args: &[&str]) -> DeputyCommand {
            DeputyArgs::try_parse_from(std::iter::once("deputy").chain(args.iter().copied()))
                .unwrap()
                .command
                .unwrap()
        }

        fn text(buf: Vec<u8>) -> String {
            String::from_utf8(buf).unwrap()
        }

        #[test]
        fn submit_wait_returns_the_jobs_rc_and_output_path_then_status_log_list() {
            let dir = tempfile::tempdir().unwrap();
            let spool = spool_in(&dir);
            let s = spool.clone();
            let worker = std::thread::spawn(move || run(&s, &cfg(1)));

            let DeputyCommand::Submit(a) = parse(&[
                "submit",
                "--name",
                "test",
                "--wait",
                "--",
                "aida",
                "-c",
                "echo hello-from-job; exit 3",
            ]) else {
                panic!("not a submit")
            };
            let mut buf = Vec::new();
            let rc = deputy_cmd::submit(&spool, &a, requester("sess-cmd"), &mut buf).unwrap();
            worker.join().unwrap();
            let shown = text(buf);
            let id = shown.lines().next().unwrap().trim().to_string();
            validate_id(&id).unwrap();
            assert_eq!(rc, 3, "{shown}");
            assert!(shown.contains("hello-from-job"), "{shown}");
            assert!(shown.contains("rc 3"), "{shown}");
            let out_path = spool.out_path(&id).display().to_string();
            assert!(shown.contains(&out_path), "{shown}");

            let DeputyCommand::Status(st) = parse(&["status", &id, "--name", "test"]) else {
                panic!("not a status")
            };
            let mut buf = Vec::new();
            deputy_cmd::status(&spool, &st, &mut buf).unwrap();
            let shown = text(buf);
            assert!(shown.contains(&format!("job {id}: failed")), "{shown}");
            assert!(shown.contains("rc:        3"), "{shown}");
            assert!(shown.contains("session sess-cmd"), "{shown}");
            assert!(shown.contains(&out_path), "{shown}");

            let DeputyCommand::Log(lg) = parse(&["log", &id, "--name", "test"]) else {
                panic!("not a log")
            };
            let mut buf = Vec::new();
            deputy_cmd::log(&spool, &lg, &mut buf).unwrap();
            assert_eq!(text(buf).trim(), "hello-from-job");

            let DeputyCommand::List(ls) = parse(&["list", "--name", "test"]) else {
                panic!("not a list")
            };
            let mut buf = Vec::new();
            deputy_cmd::list(&spool, &ls, &mut buf).unwrap();
            let shown = text(buf);
            assert!(shown.contains("failed (1):"), "{shown}");
            assert!(shown.contains("incoming (0):"), "{shown}");
            assert!(shown.contains(&id) && shown.contains("rc 3"), "{shown}");

            let DeputyCommand::List(only) = parse(&["list", "--name", "test", "--state", "done"])
            else {
                panic!("not a list")
            };
            let mut buf = Vec::new();
            deputy_cmd::list(&spool, &only, &mut buf).unwrap();
            assert_eq!(text(buf).trim(), "done (0):");
        }

        #[test]
        fn submit_wait_returns_zero_for_a_clean_job() {
            let dir = tempfile::tempdir().unwrap();
            let spool = spool_in(&dir);
            let s = spool.clone();
            let worker = std::thread::spawn(move || run(&s, &cfg(1)));
            let DeputyCommand::Submit(a) = parse(&[
                "submit", "--name", "test", "--wait", "--", "aida", "-c", "echo ok",
            ]) else {
                panic!("not a submit")
            };
            let mut buf = Vec::new();
            let rc = deputy_cmd::submit(&spool, &a, requester("s"), &mut buf).unwrap();
            worker.join().unwrap();
            assert_eq!(rc, 0);
            assert!(text(buf).contains("rc 0 (ok)"));
        }

        #[test]
        fn cancel_a_queued_job_then_a_finished_one() {
            let dir = tempfile::tempdir().unwrap();
            let spool = spool_in(&dir);
            let DeputyCommand::Submit(a) = parse(&[
                "submit",
                "--name",
                "test",
                "--",
                "aida",
                "-c",
                "touch \"$AIDA_DEPUTY_SPOOL/never-ran\"",
            ]) else {
                panic!("not a submit")
            };
            let mut buf = Vec::new();
            assert_eq!(
                deputy_cmd::submit(&spool, &a, requester("s"), &mut buf).unwrap(),
                0
            );
            let id = text(buf).trim().to_string();

            let DeputyCommand::Cancel(c) = parse(&["cancel", &id, "--name", "test"]) else {
                panic!("not a cancel")
            };
            let mut buf = Vec::new();
            deputy_cmd::cancel(&spool, &c, &mut buf).unwrap();
            assert!(text(buf).contains("cancel requested"));
            let DeputyCommand::Status(st) = parse(&["status", &id, "--name", "test"]) else {
                panic!("not a status")
            };
            let mut buf = Vec::new();
            deputy_cmd::status(&spool, &st, &mut buf).unwrap();
            let shown = text(buf);
            assert!(
                shown.contains("incoming") && shown.contains("cancel:    requested"),
                "{shown}"
            );

            run(&spool, &cfg(1));
            let r = spool.read_result(&id).unwrap();
            assert_eq!(r.rc, RC_CANCELLED);
            assert_eq!(r.reason, "cancelled before it ran");
            assert!(spool.job_path("failed", &id).exists());
            assert!(!spool.root.join("never-ran").exists());

            let mut buf = Vec::new();
            deputy_cmd::cancel(&spool, &c, &mut buf).unwrap();
            assert!(text(buf).contains("already failed"));
            let unknown = crate::deputy_cmd::DeputyIdArgs {
                id: new_job_id(),
                name: "test".into(),
            };
            assert!(deputy_cmd::cancel(&spool, &unknown, &mut Vec::new()).is_err());
        }

        #[test]
        fn submit_without_a_deputy_set_up_says_so() {
            let dir = tempfile::tempdir().unwrap();
            let spool = spool_for(dir.path(), "test").unwrap();
            let DeputyCommand::Submit(a) =
                parse(&["submit", "--name", "test", "--", "aida", "status"])
            else {
                panic!("not a submit")
            };
            let err = deputy_cmd::submit(&spool, &a, requester("s"), &mut Vec::new())
                .unwrap_err()
                .to_string();
            assert!(err.contains("aida deputy"), "{err}");
        }
    }

    // ── slice 2: awaiting, mail, pause, stop at grant/mandate end, hooks ────
    // trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
    mod slice2 {
        use super::*;
        use crate::deputy_cmd::{self, DeputyArgs, DeputyCommand};
        use clap::Parser;

        fn parse(args: &[&str]) -> DeputyCommand {
            DeputyArgs::try_parse_from(std::iter::once("deputy").chain(args.iter().copied()))
                .unwrap()
                .command
                .unwrap()
        }

        use crate::deputy_cmd::HumanTerminal;

        /// A person at their own terminal: every TTY, no agent, no drain.
        fn human() -> HumanTerminal {
            HumanTerminal {
                ttys: [true; 3],
                agent: false,
                headless: false,
                in_deputy_job: false,
                via_wrapper: true,
                wrapper_marker: None,
            }
        }

        fn job_for(body: JobBody, session: &str) -> JobRecord {
            let mut j = job(body, None, 30);
            j.requester = requester(session);
            j
        }

        fn ledger_has(spool: &Spool, event: &str, needle: &str) -> bool {
            std::fs::read_to_string(spool.ledger_path())
                .unwrap_or_default()
                .lines()
                .filter_map(|l| serde_json::from_str::<LedgerRow>(l).ok())
                .any(|r| {
                    r.event == event
                        && (needle.is_empty() || r.reason.as_deref().unwrap_or("").contains(needle))
                })
        }

        fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
            for _ in 0..200 {
                if f() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            panic!("timed out waiting for {what}");
        }

        // AC1: failed jobs are listed (id, requester session, rc, log path)
        // until acknowledged; a session sees only its own.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155.ac85effd | ai:claude
        #[test]
        fn failed_jobs_list_until_acknowledged() {
            let dir = tempfile::tempdir().unwrap();
            let spool = spool_in(&dir);
            let a = job_for(sh("exit 3"), "sess-a");
            let ok = job_for(sh("true"), "sess-a");
            let b = job_for(sh("exit 4"), "sess-b");
            for j in [&a, &ok, &b] {
                spool.submit(j).unwrap();
            }
            run(&spool, &cfg(3));

            let mine = unacked_failed_jobs(dir.path(), Some("sess-a"));
            assert_eq!(mine.len(), 1, "{mine:?}");
            assert_eq!(mine[0].id, a.id);
            assert_eq!(mine[0].rc, 3);
            assert_eq!(mine[0].deputy, "test");
            assert_eq!(mine[0].requester_session.as_deref(), Some("sess-a"));
            assert_eq!(mine[0].output, spool.out_path(&a.id));
            assert_eq!(unacked_failed_jobs(dir.path(), None).len(), 2);

            let DeputyCommand::Ack(args) = parse(&["ack", "--name", "test", &a.id]) else {
                panic!("not an ack")
            };
            let mut buf = Vec::new();
            deputy_cmd::ack(dir.path(), &args, Some("sess-a"), &mut buf).unwrap();
            assert!(String::from_utf8(buf).unwrap().contains("acknowledged"));
            assert!(unacked_failed_jobs(dir.path(), Some("sess-a")).is_empty());
            assert_eq!(unacked_failed_jobs(dir.path(), None).len(), 1);

            // A done job has nothing to acknowledge.
            let DeputyCommand::Ack(args) = parse(&["ack", "--name", "test", &ok.id]) else {
                panic!("not an ack")
            };
            let mut buf = Vec::new();
            deputy_cmd::ack(dir.path(), &args, None, &mut buf).unwrap();
            assert!(String::from_utf8(buf).unwrap().contains("not failed"));

            // --all takes only the reading session's failures.
            let DeputyCommand::Ack(args) = parse(&["ack", "--name", "test", "--all"]) else {
                panic!("not an ack")
            };
            deputy_cmd::ack(dir.path(), &args, Some("sess-b"), &mut Vec::new()).unwrap();
            assert!(unacked_failed_jobs(dir.path(), None).is_empty());
        }

        // AC1: inside a session, an explicit id another session submitted
        // is not acknowledged; the operator's shell (no session) can.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155.ac85effd | ai:claude
        #[test]
        fn a_session_cannot_ack_another_sessions_failure() {
            let dir = tempfile::tempdir().unwrap();
            let spool = spool_in(&dir);
            let b = job_for(sh("exit 4"), "sess-b");
            spool.submit(&b).unwrap();
            run(&spool, &cfg(1));
            let DeputyCommand::Ack(args) = parse(&["ack", "--name", "test", &b.id]) else {
                panic!("not an ack")
            };
            let mut buf = Vec::new();
            let err = deputy_cmd::ack(dir.path(), &args, Some("sess-a"), &mut buf).unwrap_err();
            assert!(err.to_string().contains("another session"), "{err}");
            assert!(
                String::from_utf8(buf)
                    .unwrap()
                    .contains("submitted by sess-b"),
                "the refusal names the owner"
            );
            assert_eq!(
                unacked_failed_jobs(dir.path(), None).len(),
                1,
                "still listed"
            );
            assert_eq!(unacked_failed_jobs(dir.path(), Some("sess-b")).len(), 1);

            deputy_cmd::ack(dir.path(), &args, Some("sess-b"), &mut Vec::new()).unwrap();
            assert!(unacked_failed_jobs(dir.path(), None).is_empty());

            // The operator's shell may acknowledge anyone's.
            let c = job_for(sh("exit 5"), "sess-c");
            spool.submit(&c).unwrap();
            run(&spool, &cfg(1));
            let DeputyCommand::Ack(args) = parse(&["ack", "--name", "test", &c.id]) else {
                panic!("not an ack")
            };
            deputy_cmd::ack(dir.path(), &args, None, &mut Vec::new()).unwrap();
            assert!(unacked_failed_jobs(dir.path(), None).is_empty());
        }

        // AC1: the awaiting report renders the channel in every view.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155.ac85effd | ai:claude
        #[test]
        fn awaiting_lists_a_failed_deputy_job() {
            let report = crate::awaiting_you::AwaitingReport {
                deputy_failed: vec![FailedJob {
                    deputy: "default".into(),
                    id: "0190f0aa-0000-7000-8000-00000000abcd".into(),
                    requester_session: Some("sess-a".into()),
                    rc: 3,
                    reason: "exit".into(),
                    output: "/x/out/j.out".into(),
                }],
                ..Default::default()
            };
            let json = report.to_json().to_string();
            assert!(
                json.contains("\"rc\":3")
                    && json.contains("sess-a")
                    && json.contains("/x/out/j.out"),
                "{json}"
            );
            let agent = report.render_agent_view();
            assert!(
                agent.contains("deputy_failed") && agent.contains("/x/out/j.out"),
                "{agent}"
            );
            let mut human = Vec::new();
            report.render(true, &mut human).unwrap();
            let human = String::from_utf8(human).unwrap();
            assert!(
                human.contains("rc 3")
                    && human.contains("sess-a")
                    && human.contains("aida deputy ack"),
                "{human}"
            );
        }

        // AC2: every finished job mails its requester rc and log path.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155.ac12a1ab | ai:claude
        #[test]
        fn a_finished_job_mails_its_requester() {
            let dir = tempfile::tempdir().unwrap();
            let spool = spool_in(&dir);
            let mut ok = job_for(sh("true"), "sess-a");
            ok.requester.mail_to = Some("lane-x".into());
            let mut bad = job_for(sh("exit 3"), "sess-a");
            bad.requester.mail_to = Some("lane-x".into());
            let mut anon = job_for(sh("true"), "sess-c");
            anon.requester.role = None;
            for j in [&ok, &bad, &anon] {
                spool.submit(j).unwrap();
            }
            let mut c = cfg(3);
            c.mail_root = Some(dir.path().to_path_buf());
            run(&spool, &c);

            let msgs = crate::mailbox_store::read_local_messages(dir.path()).unwrap();
            assert_eq!(msgs.len(), 2, "{msgs:?}");
            for (j, rc) in [(&ok, 0), (&bad, 3)] {
                let m = msgs
                    .iter()
                    .find(|m| m.body.contains(&j.id))
                    .unwrap_or_else(|| panic!("no mail for {}", j.id));
                assert!(
                    matches!(&m.to, aida_core::mailbox::Recipient::Agent(to) if to == "lane-x"),
                    "{:?}",
                    m.to
                );
                assert!(m.body.contains(&format!("rc {rc}")), "{}", m.body);
                assert!(
                    m.body
                        .contains(&spool.out_path(&j.id).display().to_string()),
                    "{}",
                    m.body
                );
                assert_eq!(m.body.contains("aida deputy ack"), rc != 0, "{}", m.body);
            }
            assert!(ledger_has(&spool, "mailed", "no requester mail identity"));
        }

        // AC3: a paused deputy claims nothing; the running job finishes.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155.acd360f8 | ai:claude
        #[test]
        fn a_paused_deputy_claims_nothing_and_running_jobs_finish() {
            let dir = tempfile::tempdir().unwrap();
            let spool = spool_in(&dir);
            let a = job(sh("sleep 1"), None, 30);
            spool.submit(&a).unwrap();
            let s = spool.clone();
            let worker = std::thread::spawn(move || run(&s, &cfg(2)));
            wait_until("job a running", || {
                spool.job_path("running", &a.id).exists()
            });

            let mut buf = Vec::new();
            deputy_cmd::pause(&spool, &mut buf).unwrap();
            assert!(String::from_utf8(buf).unwrap().contains("paused"));
            let b = job(sh("true"), None, 30);
            spool.submit(&b).unwrap();
            wait_until("job a done", || spool.job_path("done", &a.id).exists());
            std::thread::sleep(Duration::from_millis(500));
            assert!(
                spool.job_path("incoming", &b.id).exists(),
                "a paused deputy claimed a job"
            );
            let mut listing = Vec::new();
            deputy_cmd::list(
                &spool,
                &crate::deputy_cmd::DeputyListArgs {
                    name: "test".into(),
                    state: None,
                    limit: 5,
                },
                &mut listing,
            )
            .unwrap();
            assert!(String::from_utf8(listing).unwrap().contains("PAUSED"));

            deputy_cmd::resume(&spool, &mut Vec::new()).unwrap();
            worker.join().unwrap();
            assert!(spool.job_path("done", &b.id).exists());
            assert!(ledger_has(&spool, "paused", ""));
            assert!(ledger_has(&spool, "resumed", ""));
        }

        // AC3: resume hands claim authority back, so without a human
        // terminal (a non-TTY or contained caller) it refuses and the pause
        // marker stays.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155.acd360f8 | ai:claude
        #[test]
        fn resume_refuses_without_a_human_terminal_and_stays_paused() {
            let dir = tempfile::tempdir().unwrap();
            let spool = spool_in(&dir);
            deputy_cmd::pause(&spool, &mut Vec::new()).unwrap();
            for ctx in [
                HumanTerminal {
                    ttys: [false; 3],
                    ..human()
                },
                HumanTerminal {
                    agent: true,
                    ..human()
                },
                HumanTerminal {
                    headless: true,
                    ..human()
                },
            ] {
                let refusal = ctx.refusal(false);
                assert!(refusal.is_some(), "{ctx:?} should be refused");
                let mut buf = Vec::new();
                let err = deputy_cmd::resume_checked(&spool, refusal, &mut buf).unwrap_err();
                assert!(err.to_string().contains("resume refuses"), "{err}");
                assert!(buf.is_empty());
                assert!(spool.paused().is_some(), "the pause marker was removed");
            }
            let mut buf = Vec::new();
            deputy_cmd::resume_checked(&spool, human().refusal(false), &mut buf).unwrap();
            assert!(String::from_utf8(buf).unwrap().contains("resumed"));
            assert!(spool.paused().is_none());
        }

        // AC3/AC4: resuming a deputy whose grant expired while it was paused
        // gives it nothing to claim under: it stops instead.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155.acd360f8 | ai:claude
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155.ac65c866 | ai:claude
        #[test]
        fn a_resumed_deputy_whose_grant_expired_claims_nothing() {
            let dir = tempfile::tempdir().unwrap();
            let spool = spool_in(&dir);
            deputy_cmd::pause(&spool, &mut Vec::new()).unwrap();
            let a = job(sh("true"), None, 30);
            spool.submit(&a).unwrap();
            let mut c = cfg(0);
            c.max_jobs = None;
            let expires = Utc::now() + CDuration::milliseconds(600);
            c.grant = Arc::new(move || GrantState::Valid {
                expires_at: Some(expires),
            });
            let s = spool.clone();
            let worker = std::thread::spawn(move || run(&s, &c));
            wait_until("grant expiry", || Utc::now() > expires);
            deputy_cmd::resume(&spool, &mut Vec::new()).unwrap();
            wait_until("deputy exit", || worker.is_finished());
            worker.join().unwrap();
            assert!(
                spool.job_path("incoming", &a.id).exists(),
                "a resumed deputy claimed under an expired grant"
            );
            assert!(ledger_has(&spool, "stopped", "grant expired"));
        }

        // A pause marker that cannot be parsed still pauses (fails closed).
        #[test]
        fn an_unreadable_pause_marker_still_pauses() {
            let dir = tempfile::tempdir().unwrap();
            let spool = spool_in(&dir);
            std::fs::write(spool.paused_path(), b"not json").unwrap();
            assert!(spool.paused().is_some());
            assert!(spool.resume().unwrap());
            assert!(spool.paused().is_none());
        }

        fn mandate(expires_in_ms: i64) -> crate::gate::mandate::Mandate {
            let now = Utc::now();
            serde_json::from_value(serde_json::json!({
                "id": "m-test",
                "principal": "joe",
                "seat": "mandate",
                "scope": "ship-held",
                "issued_at": now,
                "expires_at": now + CDuration::milliseconds(expires_in_ms),
            }))
            .unwrap()
        }

        // AC4: the real mandate tie-in, over an injected mandate reader.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155.ac65c866 | ai:claude
        #[test]
        fn mandate_probe_stops_at_expiry_or_stop_unless_kept() {
            use crate::deputy_cmd::mandate_probe_with;
            use crate::gate::mandate::MandateState;
            use std::sync::atomic::{AtomicUsize, Ordering};

            // No mandate at start: never stops for one.
            let p = mandate_probe_with(false, Utc::now(), Arc::new(|| MandateState::None));
            assert_eq!(p(), None);

            // --keep-after-mandate: never stops for the mandate, even past
            // its expiry.
            let p = mandate_probe_with(
                true,
                Utc::now(),
                Arc::new(|| MandateState::Active(mandate(100))),
            );
            assert_eq!(p(), None);
            std::thread::sleep(Duration::from_millis(150));
            assert_eq!(p(), None);

            // Active at start, expiring: stops once the clock passes it
            // and a re-read shows it was not renewed.
            let p = mandate_probe_with(
                false,
                Utc::now(),
                Arc::new({
                    let m = mandate(300);
                    move || MandateState::Active(m.clone())
                }),
            );
            assert_eq!(p(), None);
            std::thread::sleep(Duration::from_millis(400));
            let why = p().expect("an expired mandate stops the deputy");
            assert!(why.contains("mandate expired"), "{why}");

            // Active at start, then stopped: the next read reports it.
            let reads = Arc::new(AtomicUsize::new(0));
            let r2 = Arc::clone(&reads);
            let m = mandate(60_000);
            let p = mandate_probe_with(
                false,
                Utc::now(),
                Arc::new(move || {
                    if r2.fetch_add(1, Ordering::SeqCst) == 0 {
                        MandateState::Active(m.clone())
                    } else {
                        MandateState::Stopped(m.clone())
                    }
                }),
            );
            let why = p().expect("a stopped mandate stops the deputy");
            assert!(why.contains("was stopped"), "{why}");
            // Cached: the next call within 10s does not re-read.
            assert!(p().is_some());
            assert_eq!(reads.load(Ordering::SeqCst), 2);
        }

        // STORY-166 AC5: a renewal (same id, later expiry) keeps a running
        // deputy claiming past the expiry it started under.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-166.ac446b4b | ai:claude
        #[test]
        fn mandate_probe_follows_a_renewal_past_the_old_expiry() {
            use crate::deputy_cmd::mandate_probe_with;
            use crate::gate::mandate::MandateState;
            use std::sync::Mutex;

            let current = Arc::new(Mutex::new(mandate(300)));
            let reader = Arc::clone(&current);
            let p = mandate_probe_with(
                false,
                Utc::now(),
                Arc::new(move || MandateState::Active(reader.lock().unwrap().clone())),
            );
            assert_eq!(p(), None);
            // Renewed in place: same id, expiry an hour out.
            current.lock().unwrap().expires_at = Utc::now() + CDuration::hours(1);
            std::thread::sleep(Duration::from_millis(400));
            assert_eq!(
                p(),
                None,
                "a renewed mandate still holds past the old expiry"
            );

            // A different mandate replacing it does not count as a renewal.
            let other = Arc::new(Mutex::new(mandate(300)));
            let reader = Arc::clone(&other);
            let p = mandate_probe_with(
                false,
                Utc::now(),
                Arc::new(move || MandateState::Active(reader.lock().unwrap().clone())),
            );
            {
                let mut m = other.lock().unwrap();
                m.id = "m-other".into();
                m.expires_at = Utc::now() + CDuration::hours(1);
            }
            std::thread::sleep(Duration::from_millis(400));
            let why = p().expect("a different mandate does not extend this run");
            assert!(why.contains("mandate expired"), "{why}");
        }

        // AC4: a short-TTL grant: the running job finishes, nothing more is
        // claimed, and the deputy exits by itself with a `stopped` row.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155.ac65c866 | ai:claude
        #[test]
        fn a_short_ttl_grant_stops_the_deputy() {
            let dir = tempfile::tempdir().unwrap();
            let spool = spool_in(&dir);
            let a = job(sh("sleep 2"), None, 30);
            spool.submit(&a).unwrap();
            let mut c = cfg(0);
            c.max_jobs = None; // only the grant can end this run
            let expires = Utc::now() + CDuration::milliseconds(1200);
            c.grant = Arc::new(move || GrantState::Valid {
                expires_at: Some(expires),
            });
            let s = spool.clone();
            let worker = std::thread::spawn(move || run(&s, &c));
            wait_until("job a running", || {
                spool.job_path("running", &a.id).exists()
            });
            wait_until("grant expiry", || Utc::now() > expires);
            let b = job(sh("true"), None, 30);
            spool.submit(&b).unwrap();
            wait_until("deputy exit", || worker.is_finished());
            worker.join().unwrap();
            assert!(
                spool.job_path("done", &a.id).exists(),
                "the running job finished"
            );
            assert!(
                spool.job_path("incoming", &b.id).exists(),
                "a job was claimed after the grant expired"
            );
            assert!(ledger_has(&spool, "stopped", "grant expired"));
        }

        // AC4: an invalid grant also ends the run (slice 1 idled instead).
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155.ac65c866 | ai:claude
        #[test]
        fn an_invalid_grant_exits_without_claiming() {
            let dir = tempfile::tempdir().unwrap();
            let spool = spool_in(&dir);
            let a = job(sh("true"), None, 30);
            spool.submit(&a).unwrap();
            let mut c = cfg(0);
            c.max_jobs = None;
            c.grant = Arc::new(|| GrantState::Invalid("revoked".into()));
            run(&spool, &c);
            assert!(spool.job_path("incoming", &a.id).exists());
            assert!(ledger_has(&spool, "stopped", "revoked"));
        }

        // AC4: the mandate's end stops claiming the same way.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155.ac65c866 | ai:claude
        #[test]
        fn a_mandate_end_stops_the_deputy() {
            let dir = tempfile::tempdir().unwrap();
            let spool = spool_in(&dir);
            let a = job(sh("sleep 1"), None, 30);
            spool.submit(&a).unwrap();
            let ended = Arc::new(AtomicBool::new(false));
            let e2 = Arc::clone(&ended);
            let mut c = cfg(0);
            c.max_jobs = None;
            c.mandate = Arc::new(move || {
                e2.load(std::sync::atomic::Ordering::SeqCst)
                    .then(|| "the mandate expired".to_string())
            });
            let s = spool.clone();
            let worker = std::thread::spawn(move || run(&s, &c));
            wait_until("job a running", || {
                spool.job_path("running", &a.id).exists()
            });
            ended.store(true, std::sync::atomic::Ordering::SeqCst);
            let b = job(sh("true"), None, 30);
            spool.submit(&b).unwrap();
            wait_until("deputy exit", || worker.is_finished());
            worker.join().unwrap();
            assert!(spool.job_path("done", &a.id).exists());
            assert!(spool.job_path("incoming", &b.id).exists());
            assert!(ledger_has(&spool, "stopped", "mandate expired"));
        }

        // AC5: any field outside the allow-list (top level or requester) is
        // refused, never dropped: a capability under an unknown name fails
        // closed, both at the hook and through the worker.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155.acdce2cd | ai:claude
        #[test]
        fn unknown_job_fields_are_refused_not_dropped() {
            use crate::deputy::hooks::{self, HookVerdict};
            let dir = tempfile::tempdir().unwrap();
            let spool = spool_in(&dir);

            // Round-trips through the on-disk form, so the parser keeps it.
            let mut j = job(sh("true"), None, 30);
            j.extra
                .insert("capability".into(), serde_json::json!({"token": "x"}));
            let back: JobRecord = serde_json::from_slice(&serde_json::to_vec(&j).unwrap()).unwrap();
            assert_eq!(hooks::unknown_field_names(&back), vec!["capability"]);
            assert!(
                matches!(hooks::check(&spool, &back), HookVerdict::Refuse(r) if r.starts_with("refused:unknown-field capability"))
            );
            let mut r = job(sh("true"), None, 30);
            r.requester
                .extra
                .insert("tty_policy".into(), serde_json::json!("allow"));
            assert!(
                matches!(hooks::unknown_fields(&r), HookVerdict::Refuse(why) if why.contains("requester.tty_policy"))
            );
            assert_eq!(
                hooks::unknown_fields(&job(sh("true"), None, 30)),
                HookVerdict::Pass
            );

            let mut top = job(sh("touch \"$AIDA_DEPUTY_SPOOL/ran-top\""), None, 30);
            top.extra.insert("grant".into(), serde_json::json!("abc"));
            let mut nested = job(sh("touch \"$AIDA_DEPUTY_SPOOL/ran-nested\""), None, 30);
            nested
                .requester
                .extra
                .insert("capability".into(), serde_json::json!("abc"));
            spool.submit(&top).unwrap();
            spool.submit(&nested).unwrap();
            run(&spool, &cfg(2));
            for (j, field) in [(&top, "grant"), (&nested, "requester.capability")] {
                let res = spool.read_result(&j.id).unwrap();
                assert!(
                    res.reason
                        .starts_with(&format!("refused:unknown-field {field}")),
                    "{}",
                    res.reason
                );
            }
            assert!(!spool.root.join("ran-top").exists());
            assert!(!spool.root.join("ran-nested").exists());
        }

        // AC5: the slice-3 hooks refuse what they cannot judge.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155.acdce2cd | ai:claude
        #[test]
        fn slice3_hooks_refuse_instead_of_failing_open() {
            use crate::deputy::hooks::{self, HookVerdict};
            let dir = tempfile::tempdir().unwrap();
            let spool = spool_in(&dir);
            let plain = job(sh("true"), None, 30);
            assert_eq!(hooks::check(&spool, &plain), HookVerdict::Pass);
            let mut delegated = job(sh("true"), None, 30);
            delegated.delegation = Some(serde_json::json!({"grant": "abc"}));
            assert!(
                matches!(hooks::submitter_delegation(&delegated), HookVerdict::Refuse(r) if r.starts_with("refused:delegation-unverifiable"))
            );
            std::fs::write(spool.command_policy_path(), b"{}").unwrap();
            assert!(
                matches!(hooks::check(&spool, &plain), HookVerdict::Refuse(r) if r.starts_with("refused:command-policy-unsupported"))
            );
        }

        // trace:STORY-B6B1278890774EF993D2EBC327862470-155.acdce2cd | ai:claude
        #[test]
        fn the_worker_refuses_delegated_jobs_command_policies_and_unknown_policy_rules() {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let spool = spool_in(&dir);
            let mut delegated = job(sh("touch \"$AIDA_DEPUTY_SPOOL/ran-1\""), None, 30);
            delegated.delegation = Some(serde_json::json!({"cap": "x"}));
            spool.submit(&delegated).unwrap();
            run(&spool, &cfg(1));
            let r = spool.read_result(&delegated.id).unwrap();
            assert!(
                r.reason.starts_with("refused:delegation-unverifiable"),
                "{}",
                r.reason
            );

            // A policy carrying a rule this deputy does not know.
            let p = spool.policy_path();
            std::fs::write(&p, br#"{"entries": [], "deny": ["aida pr ship"]}"#).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
            let j = job(sh("touch \"$AIDA_DEPUTY_SPOOL/ran-2\""), None, 30);
            spool.submit(&j).unwrap();
            run(&spool, &cfg(1));
            let r = spool.read_result(&j.id).unwrap();
            assert!(
                r.reason.starts_with("refused:policy-unreadable"),
                "{}",
                r.reason
            );
            std::fs::remove_file(&p).unwrap();

            // A command policy file this deputy cannot enforce.
            std::fs::write(spool.command_policy_path(), b"aida status\n").unwrap();
            let k = job(sh("touch \"$AIDA_DEPUTY_SPOOL/ran-3\""), None, 30);
            spool.submit(&k).unwrap();
            run(&spool, &cfg(1));
            let r = spool.read_result(&k.id).unwrap();
            assert!(
                r.reason.starts_with("refused:command-policy-unsupported"),
                "{}",
                r.reason
            );
            for n in 1..=3 {
                assert!(!spool.root.join(format!("ran-{n}")).exists(), "job {n} ran");
            }
        }
    }
}
