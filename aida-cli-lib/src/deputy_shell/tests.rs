//! Hermetic tests for the script deputy: every queue lives in a temp dir,
//! the clock, grant verdicts, liveness and the refresh minter are injected,
//! and the runner is driven in-process with test limits.
// trace:STORY-2338 | ai:claude

use super::attribution::{self, Probes};
use super::runner::{self, RunnerConfig};
use super::*;
use crate::deputy::worker::{no_mandate_stop, GrantState};
use crate::gate::seat_authority::{GrantCheck, GrantRejection, SeatGrant};
use chrono::Duration as CDuration;
use sha2::Digest;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

const LIVE_PID: u32 = 4242;
const LIVE_TOKEN: &str = "proc:4242@100";
const RUNNER_GRANT: &str = "11111111-0000-4000-8000-000000000001";

// trace:BUG-2468 | ai:codex
fn host_observer() -> aida_core::pid_observer::PidObserver {
    aida_core::pid_observer::PidObserver {
        namespace: Some(aida_core::process_probe::INITIAL_PID_NAMESPACE_INODE),
        isolated: false,
    }
}

// A successful spawn precedes exec's /proc cmdline update. Wait for the
// identity under test rather than depending on the parent's scheduling.
// trace:STORY-2593 | ai:codex
fn wait_for_legacy_runner(q: &Queue, child: &mut std::process::Child) -> RunnerStatus {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let status = q.runner_status_with(Path::new("/proc"), host_observer());
        if status == RunnerStatus::Legacy(child.id()) {
            return status;
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "bash exited before readiness"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "bash never became visible: {status:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

// trace:BUG-2468 | ai:codex
#[test]
fn runner_status_preserves_native_identity_checks_and_legacy_fallback() {
    let f = Fixture::new("t");
    let mut info = RunnerInfo {
        pid: std::process::id(),
        pid_start: Some(aida_core::liveness::process_start_identity(std::process::id()).unwrap()),
        tty: None,
        boot_id: host().boot_id,
        grant_prefix: "native".into(),
        submit_seats: vec!["advisor".into()],
        started_at: Utc::now(),
    };
    std::fs::write(f.q.runner_json(), serde_json::to_vec(&info).unwrap()).unwrap();
    assert_eq!(
        f.q.runner_status_with(Path::new("/proc"), host_observer()),
        RunnerStatus::Live(info.clone())
    );
    info.pid_start = Some("linux-starttime:0".into());
    std::fs::write(f.q.runner_json(), serde_json::to_vec(&info).unwrap()).unwrap();
    assert_eq!(
        f.q.runner_status_with(Path::new("/proc"), host_observer()),
        RunnerStatus::NotRunning
    );
    let proc = f.root().join("proc");
    std::fs::create_dir_all(proc.join("self")).unwrap();
    std::fs::create_dir_all(proc.join("4242")).unwrap();
    std::fs::write(proc.join("self/stat"), "readable").unwrap();
    std::fs::write(
        proc.join("4242/cmdline"),
        b"/bin/bash\0/aida-deputy-shell\0run\0t\0",
    )
    .unwrap();
    std::fs::write(f.q.root.join("runner.pid"), "4242\n").unwrap();
    assert_eq!(
        f.q.runner_status_with(&proc, host_observer()),
        RunnerStatus::Legacy(4242)
    );
}

// trace:BUG-2468 | ai:codex
#[test]
fn runner_pid_only_detects_a_live_bash_runner_without_grant_authority() {
    use std::process::{Command, Stdio};
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let f = Fixture::new("t");
    let script = f.root().join("aida-deputy-shell");
    std::fs::write(&script, "read -r line\n").unwrap();
    let mut child = Child(
        Command::new("/bin/bash")
            .arg(&script)
            .args(["run", "t"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    std::fs::write(f.q.root.join("runner.pid"), format!("{}\n", child.0.id())).unwrap();
    let status = wait_for_legacy_runner(&f.q, &mut child.0);
    assert_eq!(status, RunnerStatus::Legacy(child.0.id()));
    let mut out = Vec::new();
    crate::deputy_shell_cmd::write_runner_status(status, &mut out).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("runner: live"), "{out}");
    assert!(out.contains("legacy bash runner"), "{out}");
    assert!(
        f.q.live_runner().is_none(),
        "legacy evidence grants no native submit authority"
    );
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-897 | ai:codex
#[test]
fn legacy_runner_start_forms_and_submit_guidance_agree_with_list() {
    use std::process::{Command, Stdio};
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let f = Fixture::new("t");
    let script = f.root().join("aida-deputy-shell");
    // Keep bash alive without executing a real deputy or changing PATH.
    std::fs::write(&script, "read -r line\n").unwrap();
    let file = f.root().join("job.sh");
    std::fs::write(&file, "true\n").unwrap();
    let args = crate::deputy_shell_cmd::ShellSubmitArgs {
        name: "t".into(),
        file,
        timeout: "1m".into(),
        spec: None,
        wait: false,
    };
    for start in [vec!["run", "t"], vec!["t"]] {
        let mut child = Child(
            Command::new("/bin/bash")
                .arg(&script)
                .args(&start)
                .stdin(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        std::fs::write(f.q.root.join("runner.pid"), format!("{}\n", child.0.id())).unwrap();
        assert_eq!(
            wait_for_legacy_runner(&f.q, &mut child.0),
            RunnerStatus::Legacy(child.0.id()),
            "{start:?}"
        );
        let err = crate::deputy_shell_cmd::submit_with_runner_status(
            &f.q,
            &args,
            submitter(&advisor(), LIVE_TOKEN, f.root(), "s"),
            &["advisor".into()],
            &mut Vec::new(),
            || f.q.runner_status_with(Path::new("/proc"), host_observer()),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("legacy bash runner live"), "{err}");
        assert!(err.contains(&format!("pid {}", child.0.id())), "{err}");
        assert!(err.contains("aida-deputy-shell submit t <file>"), "{err}");
        assert!(err.contains("aida deputy shell run --name t"), "{err}");
        assert!(!err.contains("no script deputy"), "{err}");
        assert!(f.q.live_runner().is_none());
        assert!(f.q.queued_ids().is_empty());
        assert!(!f.q.socket_path().exists());
        assert!(f.q.job_ids().is_empty());
    }
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-905 | ai:codex
#[test]
fn submit_preserves_unknown_runner_reasons_without_queueing() {
    let f = Fixture::new("t");
    let file = f.root().join("job.sh");
    std::fs::write(&file, "true\n").unwrap();
    let args = crate::deputy_shell_cmd::ShellSubmitArgs {
        name: "t".into(),
        file,
        timeout: "1m".into(),
        spec: None,
        wait: false,
    };
    for status in [
        RunnerStatus::Unknown("cannot see host processes from this sandbox"),
        RunnerStatus::Unknown("cannot read runner process information"),
        RunnerStatus::Unknown("invalid runner.json"),
        RunnerStatus::NotRunning,
    ] {
        let unknown_reason = match &status {
            RunnerStatus::Unknown(why) => Some(*why),
            _ => None,
        };
        let mut out = Vec::new();
        let err = crate::deputy_shell_cmd::submit_with_runner_status(
            &f.q,
            &args,
            submitter(&advisor(), LIVE_TOKEN, f.root(), "s"),
            &["advisor".into()],
            &mut out,
            || status,
        )
        .unwrap_err()
        .to_string();
        if let Some(why) = unknown_reason {
            assert!(err.contains(&format!("UNKNOWN ({why})")), "{err}");
            assert!(err.contains("check from the host terminal"), "{err}");
            assert!(!err.contains("no script deputy"), "{err}");
        } else {
            assert!(err.contains("no script deputy"), "{err}");
            assert!(err.contains("aida deputy shell run --name t"), "{err}");
            assert!(!err.contains("UNKNOWN"), "{err}");
        }
        assert!(out.is_empty());
        assert!(f.q.queued_ids().is_empty());
        assert!(f.q.job_ids().is_empty());
        assert!(!f.q.socket_path().exists());
    }
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-897 | ai:codex
#[test]
fn legacy_runner_bare_names_do_not_match_other_verbs_or_invalid_start_names() {
    for name in ["t", "submit", "list", "status", "log", "wait", "_t", "-t"] {
        let f = Fixture::new(name);
        let proc = f.root().join("proc");
        std::fs::create_dir_all(proc.join("self")).unwrap();
        std::fs::create_dir_all(proc.join("4242")).unwrap();
        std::fs::write(proc.join("self/stat"), "readable").unwrap();
        std::fs::write(f.q.root.join("runner.pid"), "4242\n").unwrap();
        for prefix in [
            "aida-deputy-shell\0",
            "/bin/bash\0/home/u/aida-deputy-shell\0",
        ] {
            std::fs::write(proc.join("4242/cmdline"), format!("{prefix}{name}\0")).unwrap();
            assert_eq!(
                f.q.runner_status_with(&proc, host_observer()),
                if name == "t" {
                    RunnerStatus::Legacy(4242)
                } else {
                    RunnerStatus::NotRunning
                },
                "{prefix:?} {name}"
            );
            std::fs::write(proc.join("4242/cmdline"), format!("{prefix}run\0{name}\0")).unwrap();
            assert_eq!(
                f.q.runner_status_with(&proc, host_observer()),
                RunnerStatus::Legacy(4242),
                "explicit run {name}"
            );
        }
    }
}

// trace:BUG-2468 | ai:codex
#[test]
fn runner_status_handles_stale_wrong_name_unrelated_and_hidden_processes() {
    let f = Fixture::new("t");
    let proc = f.root().join("proc");
    std::fs::create_dir_all(proc.join("self")).unwrap();
    std::fs::write(proc.join("self/stat"), "readable").unwrap();
    let status = || f.q.runner_status_with(&proc, host_observer());
    assert_eq!(status(), RunnerStatus::NotRunning);
    std::fs::write(f.q.root.join("runner.pid"), "4242\n").unwrap();
    assert_eq!(status(), RunnerStatus::NotRunning);
    std::fs::write(f.q.runner_json(), "invalid json").unwrap();
    assert!(matches!(status(), RunnerStatus::Unknown(_)));
    std::fs::remove_file(f.q.runner_json()).unwrap();
    std::fs::create_dir(proc.join("4242")).unwrap();
    let cmd = proc.join("4242/cmdline");
    for args in [
        b"/bin/bash\0/home/u/bin/aida-deputy-shell\0run\0other\0".as_slice(),
        b"/bin/sleep\0aida-deputy-shell\0run\0t\0".as_slice(),
        b"/bin/bash\0-c\0aida-deputy-shell run t\0".as_slice(),
        b"/bin/bash\0/aida-deputy-shell\0other\0".as_slice(),
        b"/bin/bash\0/aida-deputy-shell\0submit\0t\0job.sh\0".as_slice(),
        b"".as_slice(),
    ] {
        std::fs::write(&cmd, args).unwrap();
        assert_eq!(status(), RunnerStatus::NotRunning);
    }
    std::fs::write(&cmd, b"/bin/bash\0/home/u/bin/aida-deputy-shell\0run\0t\0").unwrap();
    assert_eq!(status(), RunnerStatus::Legacy(4242));
    let sandbox = aida_core::pid_observer::PidObserver {
        isolated: true,
        ..host_observer()
    };
    let hidden = f.q.runner_status_with(&proc, sandbox);
    assert_eq!(
        hidden,
        RunnerStatus::Unknown("cannot see host processes from this sandbox")
    );
    let mut out = Vec::new();
    crate::deputy_shell_cmd::write_runner_status(hidden, &mut out).unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "runner: UNKNOWN (cannot see host processes from this sandbox)\n"
    );
    std::fs::remove_file(&cmd).unwrap();
    std::fs::create_dir(&cmd).unwrap();
    assert!(matches!(status(), RunnerStatus::Unknown(_)));
    std::fs::remove_file(proc.join("self/stat")).unwrap();
    assert!(matches!(status(), RunnerStatus::Unknown(_)));
}

fn host() -> Host {
    Host {
        hostname: "testhost".into(),
        boot_id: "boot-1".into(),
    }
}

fn grant(id: &str, seat: &str, binding: &[&str]) -> SeatGrant {
    let now = Utc::now();
    SeatGrant {
        id: id.into(),
        principal: "op".into(),
        subject: "op".into(),
        session_id: "grant-session".into(),
        seat: seat.into(),
        tty_issued_at: now,
        delegable_seats: vec![],
        parent_grant_id: None,
        issued_at: now,
        expires_at: now + CDuration::hours(24),
        revoked_at: None,
        binding: binding.iter().map(|s| s.to_string()).collect(),
        origin: None,
        launch_capability: None,
    }
}

fn key_token() -> String {
    format!("key:{}", "ab".repeat(32))
}

/// The world the verdict sees: known grants, rejected ids, live pids, and
/// per-session heartbeats, each from the registry row of one agent
/// (`(session, agent, at)`).
fn probes(
    grants: &[SeatGrant],
    rejected: &[(&str, GrantRejection)],
    heartbeats: &[(&str, &str, DateTime<Utc>)],
) -> Probes {
    let mut table: HashMap<String, GrantCheck> = grants
        .iter()
        .map(|g| (g.id.clone(), GrantCheck::Valid(g.clone())))
        .collect();
    for (id, why) in rejected {
        table.insert(
            id.to_string(),
            GrantCheck::Rejected {
                id: id.to_string(),
                seat: Some("advisor".into()),
                reason: why.clone(),
            },
        );
    }
    let beats: HashMap<(String, String), DateTime<Utc>> = heartbeats
        .iter()
        .map(|(s, agent, t)| ((s.to_string(), agent.to_string()), *t))
        .collect();
    Probes {
        grant: Arc::new(move |_root, id| {
            table.get(id).cloned().unwrap_or(GrantCheck::Rejected {
                id: id.to_string(),
                seat: None,
                reason: GrantRejection::Unreadable,
            })
        }),
        alive: Arc::new(|pid, _start| pid == LIVE_PID),
        heartbeat: Arc::new(move |_root, sid, g: &SeatGrant| {
            beats.get(&(sid.to_string(), g.subject.clone())).copied()
        }),
        host: host(),
    }
}

fn advisor() -> SeatGrant {
    grant(
        "22222222-0000-4000-8000-000000000002",
        "advisor",
        &[LIVE_TOKEN],
    )
}

/// A submitter as the runner's socket files it: with the attested peer
/// whose ancestry matched `token`.
fn submitter(g: &SeatGrant, token: &str, root: &Path, session: &str) -> Submitter {
    let mut s = attribution::submitter_from(
        g,
        &[token.to_string()],
        Some(session.into()),
        LIVE_PID,
        Some("100".into()),
        root,
        root,
        host(),
        None,
    );
    s.peer = Some(Peer {
        pid: LIVE_PID,
        uid: crate::deputy::current_uid(),
        start: Some("100".into()),
        matched: token.to_string(),
        attested: true,
    });
    s
}

fn job(q: &Queue, script: &str, sub: Submitter) -> ShellJob {
    ShellJob {
        schema: SCHEMA,
        id: crate::deputy::new_job_id(),
        deputy: q.name.clone(),
        created_at: Utc::now(),
        sha256: script_digest(script),
        script: script.into(),
        source: None,
        timeout_secs: 60,
        spec: None,
        submitter: sub,
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    q: Queue,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let q = queue_in(&dir.path().join("deputy-shell"), name).unwrap();
        q.ensure().unwrap();
        Self { dir, q }
    }
    fn root(&self) -> &Path {
        self.dir.path()
    }
    fn home(&self) -> PathBuf {
        let h = self.dir.path().join("home");
        std::fs::create_dir_all(&h).unwrap();
        h
    }
    fn submit(&self, script: &str) -> ShellJob {
        self.submit_as(
            script,
            submitter(&advisor(), LIVE_TOKEN, self.root(), "sess-a"),
        )
    }
    fn submit_as(&self, script: &str, sub: Submitter) -> ShellJob {
        let j = job(&self.q, script, sub);
        self.q.accept(&j).unwrap();
        j
    }
    /// A job already on file in `accepted/` (the runner's socket took it
    /// earlier), written without `accept`'s queue check so a test can damage
    /// the queue first and still have something to (not) claim.
    fn file_accepted_unchecked(&self, script: &str) -> ShellJob {
        let j = job(
            &self.q,
            script,
            submitter(&advisor(), LIVE_TOKEN, self.root(), "sess-a"),
        );
        crate::deputy::write_private_new(
            &self.q.accepted_job(&j.id),
            &serde_json::to_vec(&j).unwrap(),
        )
        .unwrap();
        j
    }
    fn cfg(&self, p: Probes) -> RunnerConfig {
        RunnerConfig {
            project_root: self.root().to_path_buf(),
            grant_id: RUNNER_GRANT.into(),
            grant: Arc::new(|_| GrantState::Valid {
                expires_at: Some(Utc::now() + CDuration::hours(20)),
            }),
            mint: Arc::new(|| anyhow::bail!("no minter in this test")),
            refresh: CDuration::hours(12),
            submit_seats: vec!["advisor".into()],
            keep_days: 30,
            mandate: no_mandate_stop(),
            probes: p,
            clock: system_clock(),
            poll: Duration::from_millis(20),
            kill_grace: Duration::from_secs(1),
            mail: false,
            terminal: None,
            job_home: self.home(),
            bash: "bash".into(),
            max_jobs: Some(1),
            max_rounds: Some(2000),
            serve_socket: false,
        }
    }
    fn default_cfg(&self) -> RunnerConfig {
        self.cfg(probes(&[advisor()], &[], &[]))
    }
    fn transcript(&self, id: &str) -> String {
        std::fs::read_to_string(self.q.transcript_path(id)).unwrap_or_default()
    }
    fn state(&self, id: &str) -> ShellState {
        self.q.read_state(id).expect("state.json")
    }
}

fn run(q: &Queue, cfg: &RunnerConfig) -> usize {
    runner::run(q, cfg, &AtomicBool::new(false)).unwrap()
}

fn events(q: &Queue) -> Vec<String> {
    q.read_audit().into_iter().map(|r| r.event).collect()
}

fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !cond() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

// ── 1. queue location, permissions, no /tmp ────────────────────────────────

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.acd7a7ee | ai:claude
#[test]
fn the_queue_lives_under_the_aida_home() {
    let home = crate::gate::kernel_deps::aida_home_dir().unwrap();
    assert_eq!(base_dir().unwrap(), home.join(".aida").join("deputy-shell"));
    assert_eq!(
        current_queue("2").unwrap().root,
        home.join(".aida").join("deputy-shell").join("2")
    );
    assert!(queue_in(Path::new("/x"), "../up").is_err());
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.acd7a7ee | ai:claude
#[cfg(unix)]
#[test]
fn queue_dirs_are_0700_and_a_loose_or_linked_dir_stops_claiming() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new("t");
    for d in [
        &f.q.base,
        &f.q.root,
        &f.q.accepted(),
        &f.q.jobs(),
        &f.q.acks(),
    ] {
        assert_eq!(mode(d), 0o700, "{}", d.display());
    }
    let loose = || std::fs::set_permissions(f.q.jobs(), std::fs::Permissions::from_mode(0o755));
    let tight = || std::fs::set_permissions(f.q.jobs(), std::fs::Permissions::from_mode(0o700));
    // A group/other bit at start: the runner refuses to start at all.
    loose().unwrap();
    assert!(f.q.check().is_err());
    assert!(runner::run(&f.q, &f.default_cfg(), &AtomicBool::new(false)).is_err());
    tight().unwrap();
    // The same mid-run: it stops claiming, and never repairs the mode.
    let q = f.q.clone();
    let mut cfg = f.default_cfg();
    cfg.max_rounds = Some(40);
    let runner = std::thread::spawn(move || runner::run(&q, &cfg, &AtomicBool::new(false)));
    wait_until("the runner to start", || f.q.runner_json().exists());
    loose().unwrap();
    // `accept` itself refuses a loose queue, so the socket could not file
    // a job now; one filed before the mode changed must still not run.
    assert!(f
        .q
        .accept(&job(
            &f.q,
            "true",
            submitter(&advisor(), LIVE_TOKEN, f.root(), "sess-a")
        ))
        .is_err());
    let j = f.file_accepted_unchecked("touch ran");
    assert_eq!(runner.join().unwrap().unwrap(), 0);
    assert!(f.q.accepted_job(&j.id).exists(), "the job must stay queued");
    assert_eq!(
        mode(&f.q.jobs()),
        0o755,
        "the runner must not repair the queue"
    );
    tight().unwrap();
    // accepted/ swapped for a symlink: refused too.
    let real = f.root().join("elsewhere");
    std::fs::rename(f.q.accepted(), &real).unwrap();
    std::os::unix::fs::symlink(&real, f.q.accepted()).unwrap();
    assert!(f.q.check().is_err());
    assert!(!f.home().join("ran").exists());
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.acd7a7ee | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.ac4ceb56 | ai:claude
#[test]
fn the_deputy_source_names_no_tmp_path_and_no_legacy_runme_file() {
    for (file, text) in [
        ("mod.rs", include_str!("mod.rs")),
        ("attribution.rs", include_str!("attribution.rs")),
        ("runner.rs", include_str!("runner.rs")),
        ("socket.rs", include_str!("socket.rs")),
        (
            "deputy_shell_cmd.rs",
            include_str!("../deputy_shell_cmd.rs"),
        ),
        (
            "deputy_shell_cmd_args.rs",
            include_str!("../deputy_shell_cmd_args.rs"),
        ),
    ] {
        assert!(
            !text.contains(concat!("/", "tmp")),
            "{file} names a /tmp path"
        );
        assert!(
            !text.contains(concat!("aida_", "runme")),
            "{file} names the legacy file"
        );
    }
}

/// Removes the planted legacy file however the test ends.
struct Planted(PathBuf);
impl Drop for Planted {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.acd7a7ee | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.ac4ceb56 | ai:claude
#[cfg(unix)]
#[test]
fn a_planted_legacy_tmp_script_is_never_read_or_run() {
    let name = format!("t{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let f = Fixture::new(&name);
    let marker = f.root().join("legacy-ran");
    let legacy = PathBuf::from(format!("{}/aida_runme{name}.sh", concat!("/", "tmp")));
    let body = format!("touch '{}'\n", marker.display());
    if std::fs::write(&legacy, &body).is_err() {
        eprintln!("skipping: cannot write {} here", legacy.display());
        return;
    }
    let guard = Planted(legacy.clone());
    let mut cfg = f.default_cfg();
    cfg.max_rounds = Some(10);
    assert_eq!(run(&f.q, &cfg), 0);
    assert!(!marker.exists(), "the legacy script ran");
    assert_eq!(
        std::fs::read_to_string(&legacy).unwrap(),
        body,
        "it was touched"
    );
    drop(guard);
    assert!(!legacy.exists(), "the drop guard removes the fixture");
}

// ── 2. the record, retention, role refresh ─────────────────────────────────

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.acf28460 | ai:claude
#[cfg(unix)]
#[test]
fn a_job_records_who_what_when_and_its_transcript() {
    let f = Fixture::new("t");
    let j = f.submit("printf hi; exit 0");
    assert_eq!(
        f.q.locate(&j.id).unwrap().unwrap().state.state,
        JobState::Queued
    );
    run(&f.q, &f.default_cfg());
    let rec = f.q.read_job(&j.id).unwrap();
    assert_eq!(rec, j);
    let g = advisor();
    assert_eq!(rec.submitter.grant_id.as_deref(), Some(g.id.as_str()));
    assert_eq!(rec.submitter.seat, "advisor");
    assert_eq!(rec.submitter.subject, g.subject);
    assert_eq!(rec.submitter.principal, g.principal);
    assert_eq!(rec.submitter.presenter_token.as_deref(), Some(LIVE_TOKEN));
    let bytes = std::fs::read(f.q.script_path(&j.id)).unwrap();
    assert_eq!(format!("{:x}", sha2::Sha256::digest(&bytes)), rec.sha256);
    let s = f.state(&j.id);
    assert_eq!(
        (s.state, s.rc, s.reason.as_deref()),
        (JobState::Done, Some(0), Some("ok"))
    );
    let (q_at, started, finished) = (s.queued_at, s.started_at.unwrap(), s.finished_at.unwrap());
    assert!(q_at <= started && started <= finished);
    assert_eq!(s.attribution.as_ref().unwrap().verdict, "ok");
    assert!(f.transcript(&j.id).contains("hi"));
    assert_eq!(
        events(&f.q),
        [
            "runner-start",
            "queued",
            "claimed",
            "started",
            "finished",
            "runner-stop"
        ]
    );
    for p in [
        f.q.job_json(&j.id),
        f.q.state_path(&j.id),
        f.q.script_path(&j.id),
    ] {
        assert_eq!(mode(&p), 0o600, "{}", p.display());
    }
}

fn finished_job(f: &Fixture, days_ago: i64, now: DateTime<Utc>) -> String {
    let j = job(
        &f.q,
        "true",
        submitter(&advisor(), LIVE_TOKEN, f.root(), "s"),
    );
    let dir = f.q.job_dir(&j.id);
    crate::deputy::create_private_dir(&dir).unwrap();
    crate::deputy::write_private_new(&f.q.job_json(&j.id), &serde_json::to_vec(&j).unwrap())
        .unwrap();
    let mut s = ShellState::queued(now - CDuration::days(days_ago));
    s.state = JobState::Done;
    s.rc = Some(1);
    s.finished_at = Some(now - CDuration::days(days_ago));
    f.q.write_state(&j.id, &s).unwrap();
    f.q.acknowledge(&j.id).unwrap();
    j.id
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.acf28460 | ai:claude
#[test]
fn retention_prunes_old_jobs_audits_them_and_never_truncates_the_audit() {
    let f = Fixture::new("t");
    let now = Utc::now();
    let old = finished_job(&f, 31, now);
    let kept = finished_job(&f, 29, now);
    f.q.audit_event(now, "runner-start", None, "x", |_| {});
    let before = std::fs::read_to_string(f.q.audit_path()).unwrap();
    assert_eq!(f.q.prune(now, 30), 1);
    assert!(!f.q.job_dir(&old).exists());
    assert!(!f.q.acknowledged(&old));
    assert!(f.q.job_dir(&kept).exists());
    let after = std::fs::read_to_string(f.q.audit_path()).unwrap();
    assert!(after.starts_with(&before), "earlier audit lines changed");
    let pruned: Vec<_> =
        f.q.read_audit()
            .into_iter()
            .filter(|r| r.event == "pruned")
            .collect();
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0].job.as_deref(), Some(old.as_str()));
    assert!(pruned[0].sha256.is_some() && pruned[0].rc == Some(1));
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.acf28460 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.Authority | ai:claude
#[cfg(unix)]
#[test]
fn the_grant_is_refreshed_between_jobs_on_a_clock() {
    let f = Fixture::new("t");
    let offset = Arc::new(AtomicI64::new(0));
    let first = f.submit("sleep 0.6; echo one");
    let second = f.submit(&format!(
        "echo \"grant=${}\"",
        crate::gate::seat_authority::GRANT_ENV
    ));
    let mut cfg = f.default_cfg();
    let o = Arc::clone(&offset);
    cfg.clock = Arc::new(move || Utc::now() + CDuration::seconds(o.load(Ordering::SeqCst)));
    let o = Arc::clone(&offset);
    cfg.grant = Arc::new(move |_| GrantState::Valid {
        expires_at: Some(
            Utc::now() + CDuration::seconds(o.load(Ordering::SeqCst)) + CDuration::hours(20),
        ),
    });
    cfg.refresh = CDuration::hours(1);
    let minted = Arc::new(AtomicUsize::new(0));
    let m = Arc::clone(&minted);
    cfg.mint = Arc::new(move || {
        m.fetch_add(1, Ordering::SeqCst);
        Ok("33333333-0000-4000-8000-000000000003".into())
    });
    cfg.max_jobs = Some(2);
    let q = f.q.clone();
    let id = first.id.clone();
    let bump = std::thread::spawn(move || {
        wait_until("the first job to start", || {
            q.read_state(&id)
                .is_some_and(|s| s.state == JobState::Running)
        });
        offset.store(2 * 3600, Ordering::SeqCst); // crosses --refresh mid-job
    });
    run(&f.q, &cfg);
    bump.join().unwrap();
    assert_eq!(minted.load(Ordering::SeqCst), 1);
    let first_state = f.state(&first.id);
    assert_eq!(
        first_state.rc,
        Some(0),
        "the job at the crossing was interrupted"
    );
    assert!(f.transcript(&first.id).contains("one"));
    assert!(f.transcript(&second.id).contains("grant=33333333-"));
    let ev = events(&f.q);
    let refreshed = ev.iter().position(|e| e == "role-refreshed").unwrap();
    let finished_first = ev.iter().position(|e| e == "finished").unwrap();
    assert!(finished_first < refreshed, "refreshed mid-job: {ev:?}");
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.acf28460 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.Authority | ai:claude
#[cfg(unix)]
#[test]
fn a_failed_refresh_stops_claiming() {
    let f = Fixture::new("t");
    let j = f.submit("touch ran");
    let mut cfg = f.default_cfg();
    // Under an hour left: the refresh is due at once, and it fails.
    cfg.grant = Arc::new(|_| GrantState::Valid {
        expires_at: Some(Utc::now() + CDuration::minutes(30)),
    });
    cfg.max_rounds = Some(10);
    assert_eq!(run(&f.q, &cfg), 0);
    assert!(f.q.accepted_job(&j.id).exists());
    let ev = events(&f.q);
    assert_eq!(
        ev.iter().filter(|e| *e == "role-refresh-failed").count(),
        1,
        "{ev:?}"
    );
    assert!(!ev.contains(&"claimed".to_string()));
}

// ── 3. exit codes and states ───────────────────────────────────────────────

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.ac930a33 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.ac4ceb56 | ai:claude
#[cfg(unix)]
#[test]
fn submit_wait_and_wait_return_the_jobs_exit_code() {
    let f = Fixture::new("t");
    let script = f.root().join("seven.sh");
    std::fs::write(&script, "exit 7\n").unwrap();
    // Through the real socket: the grant is bound to this test process.
    let me = self_grant();
    let q = f.q.clone();
    let mut cfg = f.cfg(real_probes(std::slice::from_ref(&me)));
    cfg.serve_socket = true;
    let runner = std::thread::spawn(move || run(&q, &cfg));
    wait_until("the runner to start", || f.q.live_runner().is_some());
    let args = crate::deputy_shell_cmd::ShellSubmitArgs {
        name: "t".into(),
        file: script,
        timeout: "1m".into(),
        spec: None,
        wait: true,
    };
    let mut out = Vec::new();
    let mut sub = submitter(&me, &me.binding[0], f.root(), "sess-a");
    sub.peer = None;
    let rc =
        crate::deputy_shell_cmd::submit(&f.q, &args, sub, &["advisor".into()], &mut out).unwrap();
    runner.join().unwrap();
    assert_eq!(rc, 7);
    let id = String::from_utf8(out)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();
    let mut out = Vec::new();
    assert_eq!(
        crate::deputy_shell_cmd::wait(&f.q, &id, None, &mut out).unwrap(),
        7
    );
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.Authority | ai:claude
#[test]
fn submit_refuses_a_seat_the_runner_does_not_allow() {
    let f = Fixture::new("t");
    let script = f.root().join("s.sh");
    std::fs::write(&script, "true\n").unwrap();
    let args = crate::deputy_shell_cmd::ShellSubmitArgs {
        name: "t".into(),
        file: script,
        timeout: "1m".into(),
        spec: None,
        wait: false,
    };
    let imp = grant(
        "44444444-0000-4000-8000-000000000004",
        "implementer",
        &[LIVE_TOKEN],
    );
    let err = crate::deputy_shell_cmd::submit(
        &f.q,
        &args,
        submitter(&imp, LIVE_TOKEN, f.root(), "s"),
        &["advisor".into()],
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("may not submit"), "{err}");
    assert!(f.q.queued_ids().is_empty());
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.ac930a33 | ai:claude
#[test]
fn wait_on_a_job_nobody_runs_times_out_with_75() {
    let f = Fixture::new("t");
    let j = f.submit("true");
    let mut out = Vec::new();
    let rc = crate::deputy_shell_cmd::wait(&f.q, &j.id, Some(Duration::from_millis(100)), &mut out)
        .unwrap();
    assert_eq!(rc, RC_WAIT_TIMEOUT);
    assert!(String::from_utf8(out).unwrap().contains("still queued"));
}

// trace:TASK-2832 | ai:codex
#[test]
fn wait_recovers_when_a_claim_publishes_its_state() {
    let f = Fixture::new("t");
    let j = f.submit("exit 7");
    let mut state = f.q.locate(&j.id).unwrap().unwrap().state;
    // Reproduce the runner's claim window without racing the actual runner:
    // job.json has moved, but state.json has not been published yet.
    std::fs::create_dir(f.q.job_dir(&j.id)).unwrap();
    std::fs::rename(f.q.accepted_job(&j.id), f.q.job_json(&j.id)).unwrap();
    assert!(f.q.locate(&j.id).unwrap().is_none());
    let q = f.q.clone();
    let id = j.id.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let mut out = Vec::new();
        let result = crate::deputy_shell_cmd::wait(&q, &id, None, &mut out);
        tx.send((result, out)).unwrap();
    });
    assert!(matches!(
        rx.recv_timeout(Duration::from_millis(150)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    state.state = JobState::Done;
    state.rc = Some(7);
    state.reason = Some("exit".into());
    std::fs::write(f.q.transcript_path(&j.id), "claim completed\n").unwrap();
    f.q.write_state(&j.id, &state).unwrap();
    let (result, out) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    waiter.join().unwrap();
    assert_eq!(result.unwrap(), 7);
    let out = String::from_utf8(out).unwrap();
    assert!(out.starts_with("claim completed\n"), "{out}");
    assert!(out.contains("done rc 7 (exit)"), "{out}");
}

// trace:TASK-2832 | ai:codex
#[test]
fn wait_refuses_an_interrupted_claim_after_the_grace_expires() {
    let f = Fixture::new("t");
    let j = f.submit("true");
    std::fs::create_dir(f.q.job_dir(&j.id)).unwrap();
    std::fs::rename(f.q.accepted_job(&j.id), f.q.job_json(&j.id)).unwrap();
    let grace = Duration::from_millis(300);
    // No caller timeout: the claim grace itself must bound this wait.
    let q = f.q.clone();
    let id = j.id.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let mut out = Vec::new();
        let begun = std::time::Instant::now();
        let result = crate::deputy_shell_cmd::wait_with_grace(&q, &id, None, grace, &mut out);
        let _ = tx.send((result, out, begun.elapsed()));
    });
    // Keep a regression that removes the grace bound from hanging the suite.
    let (result, out, elapsed) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    waiter.join().unwrap();
    let err = result.unwrap_err();
    assert!(elapsed >= grace, "refused before claim grace: {elapsed:?}");
    assert!(
        elapsed < grace + Duration::from_secs(1),
        "refusal exceeded claim grace margin: {elapsed:?}"
    );
    assert!(
        err.to_string().contains(&format!("no script job {}", j.id)),
        "{err}"
    );
    assert!(out.is_empty());
}

// trace:TASK-2832 | ai:codex
#[test]
fn wait_refuses_an_unknown_job_without_a_claim_directory() {
    let f = Fixture::new("t");
    let id = crate::deputy::new_job_id();
    let begun = std::time::Instant::now();
    let err = crate::deputy_shell_cmd::wait(&f.q, &id, None, &mut Vec::new()).unwrap_err();
    assert!(
        err.to_string().contains(&format!("no script job {id}")),
        "{err}"
    );
    assert!(begun.elapsed() < Duration::from_secs(10));
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.ac930a33 | ai:claude
// trace:TASK-2852 | ai:codex
#[cfg(unix)]
#[test]
fn status_tells_queued_running_done_and_cancelled_apart() {
    let f = Fixture::new("t");
    let fifo = f.root().join("gate.fifo");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: mkfifo on a path we own.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let j = f.submit(&format!("read x < '{}'; exit 0", fifo.display()));
    let status = |id: &str| {
        let mut out = Vec::new();
        crate::deputy_shell_cmd::status(&f.q, id, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    };
    assert!(status(&j.id).contains(": queued"));
    let q = f.q.clone();
    let cfg = f.default_cfg();
    let runner = std::thread::spawn(move || run(&q, &cfg));
    // Claim moves the accepted job before publishing state.json. Poll the
    // optional state through that window, then assert the strict status output.
    wait_until("running", || {
        f.q.read_state(&j.id)
            .is_some_and(|s| s.state == JobState::Running)
    });
    assert!(status(&j.id).contains(": running"));
    std::fs::write(&fifo, "go\n").unwrap();
    runner.join().unwrap();
    assert!(status(&j.id).contains(": done"));

    // Cancelled while queued: never runs.
    let queued = f.submit("touch never");
    f.q.request_cancel(&queued.id).unwrap();
    run(&f.q, &f.default_cfg());
    let s = f.state(&queued.id);
    assert_eq!((s.state, s.rc), (JobState::Cancelled, Some(RC_CANCELLED)));
    assert!(status(&queued.id).contains(": cancelled"));
    assert!(!f.home().join("never").exists());

    // Cancelled while running: its group is killed, rc 130.
    let long = f.submit("sleep 30");
    let q = f.q.clone();
    let cfg = f.default_cfg();
    let begun = std::time::Instant::now();
    let runner = std::thread::spawn(move || run(&q, &cfg));
    wait_until("the long job to run", || {
        f.q.read_state(&long.id)
            .is_some_and(|s| s.state == JobState::Running)
    });
    assert!(status(&long.id).contains(": running"));
    f.q.request_cancel(&long.id).unwrap();
    runner.join().unwrap();
    assert!(begun.elapsed() < Duration::from_secs(15));
    let s = f.state(&long.id);
    assert_eq!((s.state, s.rc), (JobState::Cancelled, Some(RC_CANCELLED)));
    assert!(!s.failed(), "a cancellation is not a failure");
}

// ── 4. authority ───────────────────────────────────────────────────────────

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.Authority | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.ac4ceb56 | ai:claude
#[cfg(unix)]
#[test]
fn every_unattributable_job_is_refused_with_its_code_and_never_runs() {
    let f = Fixture::new("t");
    let marks = f.root().join("marks");
    std::fs::create_dir_all(&marks).unwrap();
    let now = Utc::now();
    let adv = advisor();
    let dead = grant(
        "55555555-0000-4000-8000-000000000005",
        "advisor",
        &["proc:9999@1"],
    );
    let imp = grant(
        "66666666-0000-4000-8000-000000000006",
        "implementer",
        &[LIVE_TOKEN],
    );
    let key = key_token();
    let keyed = grant(
        "77777777-0000-4000-8000-000000000007",
        "advisor",
        &[key.as_str()],
    );
    // BUG-2419: LIVE_PID with a live start, but taken in another pid
    // namespace: it names some other process here and must not pass as
    // holder-live. // trace:STORY-2338 | ai:claude
    let foreign_token = "proc:4242@pidns-4026532999/100";
    let foreign = grant(
        "abababab-0000-4000-8000-00000000000a",
        "advisor",
        &[foreign_token],
    );
    let expired = "88888888-0000-4000-8000-000000000008";
    let revoked = "99999999-0000-4000-8000-000000000009";
    let p = probes(
        &[
            adv.clone(),
            dead.clone(),
            imp.clone(),
            keyed.clone(),
            foreign.clone(),
        ],
        &[
            (expired, GrantRejection::Expired { at: now }),
            (revoked, GrantRejection::Revoked { at: now }),
        ],
        &[
            ("fresh", "op", now - CDuration::minutes(1)),
            ("stale", "op", now - CDuration::minutes(20)),
            // Fresh, but the registry row is another agent's: it never
            // vouches for this grant, whatever session the job names.
            ("borrowed", "someone-else", now - CDuration::minutes(1)),
        ],
    );
    let root = f.root().to_path_buf();
    let base = || submitter(&adv, LIVE_TOKEN, &root, "s");
    let script = |n: &str| format!("touch '{}/{n}'", marks.display());
    let mut cases: Vec<(String, &str)> = Vec::new();
    let mut add = |label: &str, sub: Submitter, want: &'static str| {
        cases.push((f.submit_as(&script(label), sub).id, want));
    };
    let mut s = base();
    s.grant_id = None;
    add("no-grant", s, "refused:unattributed");
    let mut s = base();
    s.presenter_token = None;
    add("no-token", s, "refused:unattributed");
    let mut s = base();
    s.grant_id = Some("12121212-0000-4000-8000-000000000012".into());
    add("unknown-grant", s, "refused:grant");
    let mut s = base();
    s.grant_id = Some(expired.into());
    add("expired", s, "refused:grant");
    let mut s = base();
    s.grant_id = Some(revoked.into());
    add("revoked", s, "refused:grant");
    add(
        "dead-holder",
        submitter(&dead, "proc:9999@1", &root, "s"),
        "refused:not-live",
    );
    add(
        "foreign-pid-ns",
        submitter(&foreign, foreign_token, &root, "s"),
        "refused:not-live",
    );
    let mut s = base();
    s.host.boot_id = "boot-2".into();
    add("foreign-boot", s, "refused:foreign-host");
    add(
        "seat",
        submitter(&imp, LIVE_TOKEN, &root, "s"),
        "refused:seat",
    );
    let mut s = base();
    s.seat = "integrator".into();
    add("seat-lie", s, "refused:grant");
    add(
        "key-stale",
        submitter(&keyed, &key, &root, "stale"),
        "refused:not-live",
    );
    add(
        "key-borrowed-heartbeat",
        submitter(&keyed, &key, &root, "borrowed"),
        "refused:not-live",
    );
    let mut s = base();
    s.presenter_token = Some("proc:7777@5".into());
    add("token-not-bound", s, "refused:not-live");
    // STORY-2343: a record that never came through the socket (no attested
    // peer), or whose attested binding is not the grant's, never runs.
    let mut s = base();
    s.peer = None;
    add("no-peer", s, "refused:unattested");
    let mut s = base();
    if let Some(p) = s.peer.as_mut() {
        p.attested = false;
    }
    add("peer-not-attested", s, "refused:unattested");
    let mut s = base();
    if let Some(p) = s.peer.as_mut() {
        p.matched = "proc:7777@5".into();
    }
    add("peer-off-binding", s, "refused:unattested");
    // A sha256 that is not the script's, and an unknown field: refused:schema.
    let mut bad_sha = job(&f.q, &script("bad-sha"), base());
    bad_sha.sha256 = "0".repeat(64);
    f.q.accept(&bad_sha).unwrap();
    cases.push((bad_sha.id.clone(), "refused:schema"));
    let extra = job(&f.q, &script("extra"), base());
    let mut v = serde_json::to_value(&extra).unwrap();
    v["capability"] = serde_json::json!("let me in");
    crate::deputy::write_private_new(
        &f.q.accepted_job(&extra.id),
        &serde_json::to_vec(&v).unwrap(),
    )
    .unwrap();
    cases.push((extra.id.clone(), "refused:schema"));
    // A key-only grant with a fresh session heartbeat runs (Q5).
    let ok = f.submit_as(
        &script("key-fresh"),
        submitter(&keyed, &key, &root, "fresh"),
    );

    let mut cfg = f.cfg(p);
    cfg.max_jobs = Some(cases.len() + 1);
    run(&f.q, &cfg);
    for (id, want) in &cases {
        let s = f.state(id);
        assert_eq!(s.rc, Some(RC_REFUSED), "{id} {want}");
        assert_eq!(s.reason.as_deref(), Some(*want), "{id}");
        assert_eq!(s.state, JobState::Done);
        assert!(
            !f.q.script_path(id).exists(),
            "{want}: script written before the verdict"
        );
        assert!(f.transcript(id).contains("did not run"));
    }
    let ran: Vec<_> = std::fs::read_dir(&marks)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().into_string().unwrap())
        .collect();
    assert_eq!(
        ran,
        vec!["key-fresh".to_string()],
        "only the attributable job ran"
    );
    let s = f.state(&ok.id);
    assert_eq!(s.rc, Some(0));
    assert!(s
        .attribution
        .unwrap()
        .checks
        .contains(&"session-heartbeat".to_string()));
}

/// A registry row on disk, as the session's hooks write it.
fn registry_row(root: &Path, id: &str, session: &str, key: Option<&str>, at: DateTime<Utc>) {
    let dir = crate::agent_registry::agents_dir(root);
    std::fs::create_dir_all(&dir).unwrap();
    let mut row = toml::Table::new();
    let mut put = |k: &str, v: toml::Value| {
        row.insert(k.to_string(), v);
    };
    put("id", id.into());
    put("agent_type", "claude".into());
    put("pid", 1_593_772_i64.into());
    put("name", id.into());
    put("started_at", at.to_rfc3339().into());
    put("last_active_at", at.to_rfc3339().into());
    put("worktree_path", root.display().to_string().into());
    put("source", "session-start-hook".into());
    put("claude_session_id", session.into());
    if let Some(k) = key {
        put("grant_key", k.into());
    }
    let body = toml::to_string(&row).unwrap();
    std::fs::write(dir.join(format!("{id}.toml")), body).unwrap();
}

/// Q5 for the DEFAULT submitter: a direct advisor grant (`aida role enter`)
/// names the OS user as its subject, never a registry row's name, so the
/// heartbeat must be tied to the grant by its session key. Uses the real
/// [`attribution::session_heartbeat`] and the real hook writer.
// trace:STORY-2338 | ai:claude
#[test]
fn key_only_direct_grant_is_vouched_by_its_own_sessions_recorded_key() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let key = key_token();
    let other_key = format!("key:{}", "cd".repeat(32));
    let mut direct = grant("33333333-0000-4000-8000-000000000003", "advisor", &[&key]);
    direct.subject = "joe".into();
    direct.principal = "joe".into();
    let now = Utc::now();

    // The hook records the key it proved it holds; then the row vouches.
    registry_row(
        root,
        "claude-1593772",
        "sid-adv",
        None,
        now - CDuration::minutes(2),
    );
    assert_eq!(
        attribution::session_heartbeat(root, "sid-adv", &direct),
        None,
        "a row that never proved the key does not vouch for a direct grant"
    );
    crate::agent_registry::record_grant_key(root, "claude-1593772", Some(&key));
    assert!(attribution::session_heartbeat(root, "sid-adv", &direct).is_some());
    // Another session's row, even holding the key, never vouches for this one.
    assert_eq!(attribution::session_heartbeat(root, "sid-x", &direct), None);
    // A row holding a key the grant does not carry never vouches.
    registry_row(root, "claude-77", "sid-other", Some(&other_key), now);
    assert_eq!(
        attribution::session_heartbeat(root, "sid-other", &direct),
        None
    );
    // Dropping the grant clears the key, and the row stops vouching.
    crate::agent_registry::record_grant_key(root, "claude-1593772", None);
    assert_eq!(
        attribution::session_heartbeat(root, "sid-adv", &direct),
        None
    );
    crate::agent_registry::record_grant_key(root, "claude-1593772", Some(&key));

    // End to end through the verdict, no live process: the key-only
    // submitter runs on its own session's heartbeat, and is refused on a
    // session whose row holds another key.
    let mut p = probes(std::slice::from_ref(&direct), &[], &[]);
    p.heartbeat = Arc::new(attribution::session_heartbeat);
    let as_job = |sub: Submitter| ShellJob {
        schema: SCHEMA,
        id: crate::deputy::new_job_id(),
        deputy: "shell".into(),
        created_at: now,
        sha256: script_digest("true"),
        script: "true".into(),
        source: None,
        timeout_secs: 60,
        spec: None,
        submitter: sub,
    };
    let ok = as_job(submitter(&direct, &key, root, "sid-adv"));
    let checks = attribution::verdict(&ok, &["advisor".into()], &p, now).unwrap();
    assert!(
        checks.contains(&"session-heartbeat".to_string()),
        "{checks:?}"
    );
    let borrowed = as_job(submitter(&direct, &key, root, "sid-other"));
    assert_eq!(
        attribution::verdict(&borrowed, &["advisor".into()], &p, now)
            .unwrap_err()
            .code,
        "not-live"
    );
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.Authority | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.ac4ceb56 | ai:claude
#[test]
fn submit_side_capture_refuses_without_a_grant_and_inside_a_script_job() {
    let dir = tempfile::tempdir().unwrap();
    {
        let _env = crate::test_env::EnvVarsGuard::set(&[(
            JOB_ENV,
            "0190f0aa-0000-7000-8000-000000000001",
        )]);
        let err = attribution::capture(dir.path()).unwrap_err();
        assert!(
            err.to_string().contains("inside a script-deputy job"),
            "{err}"
        );
    }
    let _env = crate::test_env::EnvVarsGuard::set(&[(crate::gate::seat_authority::GRANT_ENV, "")]);
    let err = attribution::capture(dir.path()).unwrap_err();
    assert!(
        err.to_string()
            .contains("needs a live session with a seat grant"),
        "{err}"
    );
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.Authority | ai:claude
#[test]
fn neither_deputy_starts_inside_a_script_job() {
    let _env =
        crate::test_env::EnvVarsGuard::set(&[(JOB_ENV, "0190f0aa-0000-7000-8000-000000000001")]);
    let h = crate::deputy_cmd::HumanTerminal::ambient();
    assert!(h.in_deputy_job);
    let why = h.refusal(true).unwrap();
    assert!(why.contains("inside a deputy job"), "{why}");
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.Authority | ai:claude
#[cfg(unix)]
#[test]
fn an_exit_or_cd_in_a_job_moves_neither_the_loop_nor_the_next_job() {
    let f = Fixture::new("t");
    let a = f.submit("cd /; exit 3");
    let b = f.submit(
        "pwd; echo \"job=$AIDA_DEPUTY_SHELL_JOB\"; echo \"deputy-job=${AIDA_DEPUTY_JOB:-none}\"",
    );
    let mut cfg = f.default_cfg();
    cfg.max_jobs = Some(2);
    assert_eq!(run(&f.q, &cfg), 2);
    assert_eq!(f.state(&a.id).rc, Some(3));
    let out = f.transcript(&b.id);
    let home = std::fs::canonicalize(f.home()).unwrap();
    assert!(out.contains(home.to_str().unwrap()), "{out}");
    assert!(out.contains(&format!("job={}", b.id)), "{out}");
    assert!(
        out.contains("deputy-job=none"),
        "a script job must not carry the command-deputy marker: {out}"
    );
    assert_eq!(f.state(&b.id).rc, Some(0));
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.ac930a33 | ai:claude
#[cfg(unix)]
#[test]
fn a_timeout_kills_the_job_with_124() {
    let f = Fixture::new("t");
    let mut j = job(
        &f.q,
        "sleep 30",
        submitter(&advisor(), LIVE_TOKEN, f.root(), "s"),
    );
    j.timeout_secs = 1;
    f.q.accept(&j).unwrap();
    run(&f.q, &f.default_cfg());
    let s = f.state(&j.id);
    assert_eq!(
        (s.rc, s.reason.as_deref()),
        (Some(RC_TIMEOUT), Some("timeout"))
    );
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.acf28460 | ai:claude
#[cfg(unix)]
#[test]
fn a_job_left_running_by_a_dead_runner_is_orphaned_not_rerun() {
    let f = Fixture::new("t");
    let j = job(
        &f.q,
        "touch rerun",
        submitter(&advisor(), LIVE_TOKEN, f.root(), "s"),
    );
    crate::deputy::create_private_dir(&f.q.job_dir(&j.id)).unwrap();
    crate::deputy::write_private_new(&f.q.job_json(&j.id), &serde_json::to_vec(&j).unwrap())
        .unwrap();
    let mut s = ShellState::queued(Utc::now());
    s.state = JobState::Running;
    f.q.write_state(&j.id, &s).unwrap();
    let mut cfg = f.default_cfg();
    cfg.max_rounds = Some(3);
    run(&f.q, &cfg);
    let s = f.state(&j.id);
    assert_eq!(
        (s.state, s.rc, s.reason.as_deref()),
        (JobState::Done, Some(RC_CANCELLED), Some("orphaned"))
    );
    assert!(!f.home().join("rerun").exists());
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.Authority | ai:claude
#[cfg(unix)]
#[test]
fn a_paused_deputy_claims_nothing() {
    let f = Fixture::new("t");
    let j = f.submit("true");
    assert!(f.q.pause().unwrap());
    let mut cfg = f.default_cfg();
    cfg.max_rounds = Some(5);
    assert_eq!(run(&f.q, &cfg), 0);
    assert!(f.q.accepted_job(&j.id).exists());
    assert!(events(&f.q).contains(&"paused".to_string()));
    assert!(f.q.resume().unwrap());
    run(&f.q, &f.default_cfg());
    assert_eq!(f.state(&j.id).rc, Some(0));
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.Authority | ai:claude
#[cfg(unix)]
#[test]
fn a_second_runner_of_the_same_name_is_refused() {
    let f = Fixture::new("t");
    let _held = runner::acquire_lock(&f.q).unwrap();
    assert!(runner::run(&f.q, &f.default_cfg(), &AtomicBool::new(false)).is_err());
}

// ── 5. aida awaiting ───────────────────────────────────────────────────────

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.acbdfb5a | ai:claude
#[cfg(unix)]
#[test]
fn awaiting_lists_failed_script_jobs_beside_command_jobs_until_acked() {
    let f = Fixture::new("t");
    let other_root = f.root().join("other-project");
    std::fs::create_dir_all(&other_root).unwrap();
    let failed = f.submit("exit 4");
    let elsewhere = f.submit_as(
        "exit 5",
        submitter(&advisor(), LIVE_TOKEN, &other_root, "sess-a"),
    );
    let fine = f.submit("exit 0");
    let mut cfg = f.default_cfg();
    cfg.max_jobs = Some(3);
    run(&f.q, &cfg);
    let base = f.q.base.clone();
    let mine = unacked_failed_jobs_in(&base, f.root(), Some("sess-a"));
    assert_eq!(mine.len(), 1, "{mine:?}");
    assert_eq!(mine[0].id, failed.id);
    assert_eq!(mine[0].kind, crate::deputy::DeputyKind::Script);
    assert_eq!(mine[0].output, f.q.transcript_path(&failed.id));
    assert!(unacked_failed_jobs_in(&base, f.root(), Some("sess-b")).is_empty());
    assert_eq!(unacked_failed_jobs_in(&base, f.root(), None).len(), 1);
    assert_eq!(
        unacked_failed_jobs_in(&base, &other_root, None)[0].id,
        elsewhere.id
    );
    let _ = fine;

    let command = crate::deputy::FailedJob {
        deputy: "default".into(),
        id: "0190f0aa-0000-7000-8000-00000000abcd".into(),
        requester_session: Some("sess-a".into()),
        rc: 3,
        reason: "exit".into(),
        output: "/x/out/j.out".into(),
        kind: crate::deputy::DeputyKind::Command,
    };
    let report = crate::awaiting_you::AwaitingReport {
        deputy_failed: vec![command, mine[0].clone()],
        ..Default::default()
    };
    let mut human = Vec::new();
    report.render(true, &mut human).unwrap();
    let human = String::from_utf8(human).unwrap();
    assert!(human.contains("deputy command job"), "{human}");
    assert!(human.contains("deputy script job"), "{human}");
    assert!(
        human.contains(&format!("aida deputy shell ack {} --name t", failed.id)),
        "{human}"
    );
    assert!(human.contains("aida deputy ack 0190f0aa"), "{human}");
    let json = report.to_json().to_string();
    assert!(
        json.contains("\"kind\":\"script\"") && json.contains("\"kind\":\"command\""),
        "{json}"
    );

    // Another session may not silence it; its own session may.
    let args = |ids: Vec<String>| crate::deputy_shell_cmd::ShellAckArgs {
        ids,
        all: false,
        name: "t".into(),
    };
    let adv = advisor();
    let actor = crate::deputy_shell_cmd::ack_actor(Some(&adv), Some("sess-a"));
    assert_eq!(actor, "22222222 advisor");
    assert!(crate::deputy_shell_cmd::ack(
        &f.q,
        f.root(),
        &args(vec![failed.id.clone()]),
        Some("sess-b"),
        &actor,
        &mut Vec::new()
    )
    .is_err());
    crate::deputy_shell_cmd::ack(
        &f.q,
        f.root(),
        &args(vec![failed.id.clone()]),
        Some("sess-a"),
        &actor,
        &mut Vec::new(),
    )
    .unwrap();
    assert!(unacked_failed_jobs_in(&base, f.root(), None).is_empty());
    let acked: Vec<_> =
        f.q.read_audit()
            .into_iter()
            .filter(|r| r.event == "acked")
            .collect();
    assert_eq!(acked.len(), 1);
    assert_eq!(
        acked[0].actor, "22222222 advisor",
        "the AuditRow::actor form"
    );
    assert_eq!(
        crate::deputy_shell_cmd::ack_actor(None, None),
        OPERATOR_TTY,
        "the operator's own shell"
    );
    assert_eq!(
        crate::deputy_shell_cmd::ack_actor(None, Some("s")),
        "- unattributed"
    );
}

// ── 6. under a pty ─────────────────────────────────────────────────────────

/// The runner drives a real terminal: a test pty stands in for the
/// operator's, so the relay, raw mode and the job's own pty are all real.
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.ac4ceb56 | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn a_job_has_a_terminal_on_all_three_fds_and_its_output_reaches_both_sinks() {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    let f = Fixture::new("t");
    let j = f.submit("test -t 0 && test -t 1 && test -t 2 && echo TTY-OK");
    let (master, slave) = crate::deputy::worker::open_pty().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let mut operator = std::fs::File::from(master);
    let reader = std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        while let Ok(n) = operator.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock().unwrap().extend_from_slice(&buf[..n]);
        }
    });
    let mut cfg = f.default_cfg();
    cfg.terminal = Some(runner::Terminal {
        input: slave.as_raw_fd(),
        output: slave.as_raw_fd(),
    });
    run(&f.q, &cfg);
    assert_eq!(f.state(&j.id).rc, Some(0));
    assert!(
        f.transcript(&j.id).contains("TTY-OK"),
        "{}",
        f.transcript(&j.id)
    );
    wait_until("relayed output", || {
        String::from_utf8_lossy(&seen.lock().unwrap()).contains("TTY-OK")
    });
    let shown = String::from_utf8_lossy(&seen.lock().unwrap()).to_string();
    assert!(
        shown.contains(&j.sha256),
        "the script was not shown first: {shown}"
    );
    drop(slave);
    let _ = reader.join();
}

// ── 7. help text ───────────────────────────────────────────────────────────

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// trace:STORY-b6b1278890774ef993d2ebc327862470-334.Docs | ai:claude
#[test]
fn both_deputies_help_opens_with_the_same_two_kinds_paragraph() {
    use clap::CommandFactory;
    let mut cmd = crate::deputy_cmd::DeputyArgs::command();
    let top = squash(&cmd.render_long_help().to_string());
    let shell = squash(
        &cmd.find_subcommand_mut("shell")
            .unwrap()
            .render_long_help()
            .to_string(),
    );
    let start = top.find("Two kinds of deputy exist.").expect("deputy help");
    let end = top[start..].find("cannot express.").unwrap() + start + "cannot express.".len();
    let para = &top[start..end];
    assert!(
        shell.contains(para),
        "deputy shell --help lacks the paragraph:\n{shell}"
    );
    assert!(shell.contains("counts as you at the keyboard"));
    assert!(!top.contains("--allow-scripts"));
}

/// The advisor's standing orders route multi-step operator acts to
/// `submit` and link the one deputies page, which exists.
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.Docs | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.acd7a7ee | ai:claude
#[test]
fn the_advisor_standing_orders_deputy_line_submits_and_links_the_page() {
    let orders =
        include_str!("../../../aida-core/templates/.aida/discipline/standing-orders/advisor.md");
    let line = orders
        .split("\n- ")
        .find(|item| item.starts_with("Use the established deputies"))
        .expect("the deputy line");
    let line = squash(line);
    assert!(
        line.contains("aida deputy shell submit --file FILE --wait"),
        "{line}"
    );
    assert!(
        line.contains(
            "[Deputies: command vs script](https://github.com/joemooney/aida/blob/main/docs/advanced/deputies.md)"
        ),
        "the link must resolve from the template master, the generated mirror and an installed project alike: {line}"
    );
    let page = include_str!("../../../docs/advanced/deputies.md");
    assert!(page.starts_with("# Deputies: command vs script"));
}

// ── 8. who may submit, and the runner's own grant ──────────────────────────

/// SECURITY: the tracked config file never widens who may submit. A
/// committed `submit_seats` (any lane can merge one) is ignored; only the
/// operator's `--allow-seat` at the terminal adds a seat.
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.Authority | ai:claude
#[test]
fn a_committed_submit_seats_entry_cannot_widen_the_submitters() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(
        // Spelled in two halves so the BUG-1723 scan, which reads this file
        // as production source, does not count a test fixture as a reader.
        dir.path().join(".aida").join(concat!("config", ".toml")),
        "[deputy_shell]\nkeep_days = 7\nsubmit_seats = [\"advisor\", \"implementer\"]\n",
    )
    .unwrap();
    let c = load_config(dir.path());
    assert_eq!(c, ShellConfig { keep_days: 7 }, "only keep_days is read");
    assert_eq!(default_submit_seats(), vec!["advisor".to_string()]);
    assert_eq!(parse_config("not toml ["), ShellConfig::default());
    // What `run` hands the runner without --allow-seat: the advisor alone.
    let seats = crate::deputy_shell_cmd::submit_seats(&default_submit_seats(), &[]);
    assert_eq!(seats, vec!["advisor".to_string()]);
    // So an implementer's job is refused at submit and again at claim.
    let f = Fixture::new("t");
    let script = f.root().join("s.sh");
    std::fs::write(&script, "true\n").unwrap();
    let imp = grant(
        "45454545-0000-4000-8000-000000000045",
        "implementer",
        &[LIVE_TOKEN],
    );
    let args = crate::deputy_shell_cmd::ShellSubmitArgs {
        name: "t".into(),
        file: script,
        timeout: "1m".into(),
        spec: None,
        wait: false,
    };
    let sub = submitter(&imp, LIVE_TOKEN, f.root(), "s");
    assert!(
        crate::deputy_shell_cmd::submit(&f.q, &args, sub.clone(), &seats, &mut Vec::new()).is_err()
    );
    let refused = attribution::verdict(
        &job(&f.q, "true", sub),
        &seats,
        &probes(&[imp.clone()], &[], &[]),
        Utc::now(),
    )
    .unwrap_err();
    assert_eq!(refused.reason(), "refused:seat");
    // The operator's --allow-seat is the one widening.
    assert_eq!(
        crate::deputy_shell_cmd::submit_seats(
            &default_submit_seats(),
            &["implementer".into(), "advisor".into()]
        ),
        vec!["advisor".to_string(), "implementer".to_string()]
    );
}

/// `run` refuses without a valid grant and under any seat but the
/// advisor's; it starts under a valid advisor grant.
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.Authority | ai:claude
#[test]
fn run_refuses_without_a_grant_and_under_a_non_advisor_seat() {
    use crate::deputy_shell_cmd::runner_grant;
    let err = runner_grant(GrantCheck::Absent).unwrap_err().to_string();
    assert!(err.contains("refuses to start"), "{err}");
    assert!(err.contains("aida role enter advisor"), "{err}");
    let err = runner_grant(GrantCheck::Rejected {
        id: "88888888-0000-4000-8000-000000000008".into(),
        seat: Some("advisor".into()),
        reason: GrantRejection::Expired { at: Utc::now() },
    })
    .unwrap_err()
    .to_string();
    assert!(err.contains("refuses to start"), "{err}");
    let imp = grant(
        "66666666-0000-4000-8000-000000000006",
        "implementer",
        &[LIVE_TOKEN],
    );
    let err = runner_grant(GrantCheck::Valid(imp))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("runs under an advisor grant") && err.contains("holds a implementer grant"),
        "{err}"
    );
    let adv = advisor();
    assert_eq!(
        runner_grant(GrantCheck::Valid(adv.clone())).unwrap().id,
        adv.id
    );
}

/// The real refresh minter refuses where `aida role enter` does: inside an
/// AIDA-managed agent session it mints nothing.
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.Authority | ai:claude
#[test]
fn the_real_refresh_minter_refuses_an_agent_session() {
    let dir = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::set(&[("AIDA_AGENT_NAME", "drain-worker")]);
    let err = crate::gate::seat_authority::issue_runner_refresh(
        dir.path(),
        RUNNER_SEAT,
        vec!["implementer".into()],
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("AIDA-managed agent sessions cannot issue"),
        "{err}"
    );
}

/// A job dir left by an interrupted claim (no state.json) takes the refusal
/// for a colliding id, so `wait` returns 126 instead of "no script job".
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.ac930a33 | ai:claude
#[cfg(unix)]
#[test]
fn an_id_collision_is_filed_as_a_refused_job_that_wait_returns() {
    let f = Fixture::new("t");
    let j = f.submit("touch collided");
    crate::deputy::create_private_dir(&f.q.job_dir(&j.id)).unwrap();
    let mut cfg = f.default_cfg();
    cfg.max_rounds = Some(5);
    run(&f.q, &cfg);
    let s = f.state(&j.id);
    assert_eq!(
        (s.state, s.rc, s.reason.as_deref()),
        (JobState::Done, Some(RC_REFUSED), Some("refused:schema"))
    );
    assert!(!f.home().join("collided").exists());
    assert!(!f.q.accepted_job(&j.id).exists(), "filed aside");
    let mut out = Vec::new();
    assert_eq!(
        crate::deputy_shell_cmd::wait(&f.q, &j.id, Some(Duration::from_secs(5)), &mut out).unwrap(),
        RC_REFUSED
    );
    assert!(String::from_utf8(out).unwrap().contains("already in use"));
}

// ── STORY-2343: kernel-attested submit over the runner's socket ─────────────

/// An advisor grant bound to THIS test process, as `proc:<pid>@<start>`.
fn self_grant() -> SeatGrant {
    let pid = std::process::id();
    let start = aida_core::liveness::process_start_identity(pid).expect("own start identity");
    grant(
        "5a5a5a5a-0000-4000-8000-00000000005a",
        "advisor",
        &[format!("proc:{pid}@{start}").as_str()],
    )
}

/// Probes over real process liveness (the socket attests real processes).
fn real_probes(grants: &[SeatGrant]) -> Probes {
    let mut p = probes(grants, &[], &[]);
    p.alive =
        Arc::new(|pid, start| aida_core::liveness::process_identity_matches_strict(pid, start));
    p
}

/// A helper child that is reaped on drop, whatever the test does.
struct Reaped(std::process::Child);
impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

// trace:STORY-2343 | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn a_socket_submit_records_the_kernel_peer_and_a_peer_outside_the_grant_is_refused() {
    let f = Fixture::new("t");
    let me = self_grant();
    // A live process that is NOT an ancestor of this one.
    let child = Reaped(
        std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep"),
    );
    let cpid = child.0.id();
    let cstart = aida_core::liveness::process_start_identity(cpid).expect("child start");
    let other = grant(
        "6b6b6b6b-0000-4000-8000-00000000006b",
        "advisor",
        &[format!("proc:{cpid}@{cstart}").as_str()],
    );
    let server = socket::serve(
        &f.q,
        real_probes(&[me.clone(), other.clone()]),
        system_clock(),
    )
    .expect("serve the socket");
    assert_eq!(mode(&f.q.socket_path()), 0o600);

    // Under the grant: accepted, and the kernel's peer replaces whatever the
    // client claimed about itself.
    let mut sub = submitter(&me, &me.binding[0], f.root(), "s");
    sub.pid = 4242;
    sub.peer = Some(Peer {
        pid: 4242,
        uid: 0,
        start: None,
        matched: "proc:4242@100".into(),
        attested: true,
    });
    let j = job(&f.q, "true", sub);
    let id = socket::expect_accepted(
        &f.q,
        &socket::Request::Submit {
            job: Box::new(j.clone()),
        },
    )
    .expect("accepted");
    assert_eq!(id, j.id);
    let filed: ShellJob =
        serde_json::from_slice(&std::fs::read(f.q.accepted_job(&id)).unwrap()).unwrap();
    let peer = filed.submitter.peer.expect("an attested peer");
    assert_eq!(peer.pid, std::process::id());
    assert_eq!(peer.uid, crate::deputy::current_uid());
    assert_eq!(peer.matched, me.binding[0]);
    assert!(peer.attested);
    assert_eq!(filed.submitter.pid, std::process::id());

    // Naming a grant bound to an unrelated live process: refused, not filed.
    let j2 = job(
        &f.q,
        "true",
        submitter(&other, &other.binding[0], f.root(), "s"),
    );
    let resp = socket::request(
        &f.q,
        &socket::Request::Submit {
            job: Box::new(j2.clone()),
        },
    )
    .unwrap();
    assert!(!resp.ok);
    assert_eq!(resp.reason.as_deref(), Some("refused:not-under-grant"));
    assert!(!f.q.accepted_job(&j2.id).exists());
    // A grant that does not validate: refused too.
    let mut ghost = submitter(&me, &me.binding[0], f.root(), "s");
    ghost.grant_id = Some("7c7c7c7c-0000-4000-8000-00000000007c".into());
    let j3 = job(&f.q, "true", ghost);
    let resp = socket::request(&f.q, &socket::Request::Submit { job: Box::new(j3) }).unwrap();
    assert_eq!(resp.reason.as_deref(), Some("refused:grant"));
    assert!(
        events(&f.q)
            .iter()
            .filter(|e| *e == "submit-refused")
            .count()
            >= 2
    );

    // Cancel rides the socket as well; the runner files the request.
    socket::expect_accepted(&f.q, &socket::Request::Cancel { id: id.clone() }).unwrap();
    assert!(f.q.cancel_requested(&id));

    drop(server);
    assert!(
        !f.q.socket_path().exists(),
        "the socket goes with the runner"
    );
}

// trace:STORY-2343 | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn attest_compares_bindings_as_namespace_aware_tokens() {
    let cred = socket::PeerCred { pid: 30, uid: 1000 };
    let chain = [30, 20, 10];
    let alive = |pid: u32, _s: &str| pid != 99;
    let g = |b: &str| grant("8d8d8d8d-0000-4000-8000-00000000008d", "advisor", &[b]);
    // The launcher (an ancestor) is bound: accepted, by that token.
    assert_eq!(
        socket::attest(cred, 1000, &g("proc:20@7"), &chain, &alive).unwrap(),
        "proc:20@7"
    );
    // A session-leader binding counts the same way.
    assert!(socket::attest(cred, 1000, &g("sid:10@7"), &chain, &alive).is_ok());
    // Another uid, a pid off the chain, a dead holder, a key: refused.
    assert_eq!(
        socket::attest(cred, 1001, &g("proc:20@7"), &chain, &alive)
            .unwrap_err()
            .code,
        "foreign-uid"
    );
    let off_chain = [30, 20, 99];
    for (b, ch) in [
        ("proc:21@7", &chain),
        ("proc:99@7", &off_chain),
        ("key:abc", &chain),
    ] {
        assert_eq!(
            socket::attest(cred, 1000, &g(b), ch, &alive)
                .unwrap_err()
                .code,
            "not-under-grant",
            "{b}"
        );
    }
    // BUG-2419: the same pid bound in another pid namespace is another
    // process; it is never compared against this namespace's chain.
    assert_eq!(
        socket::attest(cred, 1000, &g("proc:20@pidns-4026532999/7"), &chain, &alive)
            .unwrap_err()
            .code,
        "not-under-grant"
    );
    // The real walk starts at the peer and reaches its parent.
    let me = std::process::id();
    let walked = socket::ancestry(me);
    assert_eq!(walked[0], me);
    assert!(walked.len() <= socket::MAX_ANCESTRY);
    if let Some(ppid) = socket::parent_pid(me).filter(|p| *p > 1) {
        assert_eq!(walked.get(1), Some(&ppid));
    }
}

// trace:STORY-2343 | ai:claude
#[cfg(unix)]
#[test]
fn a_file_dropped_where_submitters_used_to_write_is_never_claimed() {
    let f = Fixture::new("t");
    // The retired submitter-writable `incoming/` directory, holding a
    // well-formed, attested-looking job.
    let legacy = f.q.root.join("incoming");
    crate::deputy::create_private_dir(&legacy).unwrap();
    let j = job(
        &f.q,
        &format!("touch '{}'", f.home().join("ran").display()),
        submitter(&advisor(), LIVE_TOKEN, f.root(), "s"),
    );
    crate::deputy::write_private_new(
        &legacy.join(format!("{}.job", j.id)),
        &serde_json::to_vec(&j).unwrap(),
    )
    .unwrap();
    let mut cfg = f.default_cfg();
    cfg.max_rounds = Some(5);
    assert_eq!(run(&f.q, &cfg), 0);
    assert!(f.q.queued_ids().is_empty());
    assert!(!f.q.job_dir(&j.id).exists());
    assert!(!f.home().join("ran").exists());
    // ...but it does not vanish silently: the runner said so once at start.
    let noted: Vec<_> =
        f.q.read_audit()
            .into_iter()
            .filter(|r| r.event == "legacy-incoming-ignored")
            .collect();
    assert_eq!(noted.len(), 1, "one notice per runner start");
    let files = noted[0].detail.as_ref().unwrap()["files"].to_string();
    assert!(files.contains(&j.id), "{files}");
}

// trace:STORY-2343 | ai:claude
// trace:TASK-b6b1278890774ef993d2ebc327862470-905 | ai:codex
#[cfg(unix)]
#[test]
fn submit_without_a_live_runner_says_to_start_one_and_names_the_sandbox_opt_in() {
    let f = Fixture::new("t");
    let script = f.root().join("s.sh");
    std::fs::write(&script, "true\n").unwrap();
    let args = crate::deputy_shell_cmd::ShellSubmitArgs {
        name: "t".into(),
        file: script,
        timeout: "1m".into(),
        spec: None,
        wait: false,
    };
    let err = crate::deputy_shell_cmd::submit_with_runner_status(
        &f.q,
        &args,
        submitter(&advisor(), LIVE_TOKEN, f.root(), "s"),
        &["advisor".into()],
        &mut Vec::new(),
        || RunnerStatus::NotRunning,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("aida deputy shell run --name t"), "{err}");
    assert!(err.contains("first"), "{err}");
    assert!(f.q.queued_ids().is_empty());
    // No socket at all: the client says the same.
    let err = socket::request(&f.q, &socket::Request::Cancel { id: "x".into() })
        .unwrap_err()
        .to_string();
    assert!(err.contains("is running"), "{err}");
    // The sandboxed-client error names the one opt-in.
    assert!(socket::SANDBOX_OPT_IN.contains("excludedCommands"));
    assert!(socket::SANDBOX_OPT_IN.contains("aida deputy shell submit"));
}

// trace:STORY-2343 | ai:claude
#[cfg(unix)]
#[test]
fn a_stale_socket_is_replaced_under_the_lock_and_a_non_socket_is_refused() {
    let f = Fixture::new("t");
    {
        let _old = std::os::unix::net::UnixListener::bind(f.q.socket_path()).unwrap();
    }
    assert!(f.q.socket_path().exists(), "a dead runner's socket file");
    let server = socket::serve(&f.q, probes(&[], &[], &[]), system_clock()).expect("replaced");
    drop(server);
    std::fs::write(f.q.socket_path(), b"not a socket").unwrap();
    let err = socket::serve(&f.q, probes(&[], &[], &[]), system_clock())
        .err()
        .expect("refused");
    assert!(err.to_string().contains("not a socket"), "{err}");
    assert!(f.q.socket_path().is_file(), "never removed");
}

// trace:STORY-2343 | ai:claude
#[cfg(unix)]
#[test]
fn a_client_that_never_finishes_its_request_does_not_stall_other_requests() {
    let f = Fixture::new("t");
    let server = socket::serve(&f.q, probes(&[], &[], &[]), system_clock()).expect("serve");
    let cancel = || {
        socket::request(
            &f.q,
            &socket::Request::Cancel {
                id: crate::deputy::new_job_id(),
            },
        )
        .expect("a reply")
    };
    // One connection that sends nothing and never half-closes.
    let silent = std::os::unix::net::UnixStream::connect(f.q.socket_path()).unwrap();
    let begun = std::time::Instant::now();
    let resp = cancel();
    assert_eq!(resp.reason.as_deref(), Some("refused:schema"), "{resp:?}");
    assert!(
        begun.elapsed() < Duration::from_secs(3),
        "answered beside the silent client, took {:?}",
        begun.elapsed()
    );
    // Enough silent clients to fill every slot: the next is refused as busy
    // at once rather than queued behind them.
    let more: Vec<_> = (0..15)
        .map(|_| std::os::unix::net::UnixStream::connect(f.q.socket_path()).unwrap())
        .collect();
    let begun = std::time::Instant::now();
    let resp = cancel();
    assert_eq!(resp.reason.as_deref(), Some("refused:busy"), "{resp:?}");
    assert!(begun.elapsed() < Duration::from_secs(3));
    drop(more);
    drop(silent);
    drop(server);
}

// trace:TASK-2879 | ai:codex
#[test]
fn status_recovers_claim_window_and_bounds_interrupted_claims() {
    let f = Fixture::new("t");
    let j = f.submit("true");
    let state = f.q.locate(&j.id).unwrap().unwrap().state;
    std::fs::create_dir(f.q.job_dir(&j.id)).unwrap();
    std::fs::rename(f.q.accepted_job(&j.id), f.q.job_json(&j.id)).unwrap();
    let mut out = Vec::new();
    assert!(crate::deputy_shell_cmd::status_with_grace(
        &f.q,
        &j.id,
        Duration::from_millis(30),
        &mut out
    )
    .is_err());
    let q = f.q.clone();
    let id = j.id.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let result =
            crate::deputy_shell_cmd::status_with_grace(&q, &id, Duration::from_secs(2), &mut out);
        tx.send((result, out)).unwrap();
    });
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    f.q.write_state(&j.id, &state).unwrap();
    let (result, out) = rx.recv_timeout(Duration::from_secs(3)).unwrap();
    reader.join().unwrap();
    result.unwrap();
    assert!(String::from_utf8(out).unwrap().contains(": queued"));
}

// The fixed service operation shares the real kernel-attested socket; merely
// naming a valid advisor or its registration cannot replace the binding.
// trace:STORY-2571 | ai:codex
#[test]
fn story2571_service_socket_authenticates_rotation_and_refuses_stale_binding() {
    let f = Fixture::new("t");
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(
        f.root(),
        "advisor",
        &["advisor", "reviewer"],
    );
    let first = crate::gate::seat_authority::grant_check(f.root())
        .into_valid()
        .unwrap();
    let cfg = f.default_cfg();
    let host = crate::review_watch::service::HostAuthority::new(&cfg);
    let _server =
        socket::serve_service(&f.q, real_probes(&[]), system_clock(), Some(host)).unwrap();
    let send = |g: &SeatGrant, project: &Path| {
        socket::request(
            &f.q,
            &socket::Request::ReviewWatchRebind {
                project: project.to_path_buf(),
                grant: g.id.clone(),
            },
        )
        .unwrap()
    };
    assert!(send(&first, f.root()).ok);
    let next_id = crate::gate::seat_authority::test_support::mint_grant_for(
        f.root(),
        &first.subject,
        "advisor",
        &["advisor", "reviewer"],
    );
    let next = crate::gate::seat_authority::read_grant(&next_id).unwrap();
    assert!(send(&next, f.root()).ok);
    let stale = send(&first, f.root());
    assert!(!stale.ok);
    assert!(stale.why.unwrap().contains("stale"));
    assert!(!send(&next, &f.root().join("other")).ok);
    crate::gate::seat_authority::revoke_by_id(&next.id).unwrap();
    assert!(!send(&next, f.root()).ok);
}

// trace:BUG-2520 | ai:codex
#[test]
fn delegated_advisor_jobs_validate_operator_principal_and_chain() {
    use crate::gate::seat_authority as authority;
    let f = Fixture::new("t");
    let _ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(f.root(), "advisor", &["advisor"]);
    let parent = authority::grant_check(f.root()).into_valid().unwrap();
    let child = authority::issue_child_from(parent.clone(), "advisor", "claude-advisor-111", false)
        .unwrap();
    let foreign_id = authority::test_support::mint_grant_for(
        f.root(),
        "another-operator",
        "advisor",
        &["advisor"],
    );
    let foreign_parent = authority::read_grant(&foreign_id).unwrap();
    let foreign =
        authority::issue_child_from(foreign_parent, "advisor", "claude-advisor-222", false)
            .unwrap();
    let p = Probes::live();
    let enqueue = |g: &SeatGrant| {
        let mut s = submitter(g, &g.binding[0], f.root(), "session");
        s.host = p.host.clone();
        f.submit_as("exit 7", s)
    };
    let accepted = enqueue(&child);
    let refused = enqueue(&foreign);
    let mut cfg = f.cfg(p.clone());
    cfg.max_jobs = Some(2);
    run(&f.q, &cfg);
    assert_eq!(f.state(&accepted.id).rc, Some(7));
    assert_eq!(
        f.state(&refused.id).reason.as_deref(),
        Some("refused:grant")
    );
    assert!(!f.q.script_path(&refused.id).exists());
    assert!(f.transcript(&refused.id).contains("principal"));

    // A matching principal cannot rescue a revoked delegation chain.
    authority::revoke_by_id(&parent.id).unwrap();
    let revoked = enqueue(&child);
    run(&f.q, &f.cfg(p));
    assert_eq!(
        f.state(&revoked.id).reason.as_deref(),
        Some("refused:grant")
    );
    assert!(!f.q.script_path(&revoked.id).exists());
}
