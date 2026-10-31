//! BUG-044: a headless implementer that parks on its own background run is
//! waited for and resumed by its launcher, bounded, instead of being reported
//! `no-pr`; a PR-less report names the uncommitted work left behind.
// trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude

use super::*;
use crate::standalone_implementer as si;

// ── Deciding to resume ────────────────────────────────────────────────────

#[test]
fn the_reported_parking_results_read_as_waiting_and_a_finish_does_not() {
    for text in [
        "Waiting on the aida-core test build.",
        "Both runs are in the background; I'll pick up results when they finish.",
        "The full test suite is still running.",
        "I'll continue once the build completes.",
        "cargo nextest is still running for aida-cli-lib",
    ] {
        assert!(says_waiting_on_background(text), "{text}");
    }
    for text in [
        "Opened PR #42; stopping.",
        "Punted: two valid schemas.",
        "All tests pass. PR is open.",
        "Stopping: waiting for the advisor to decide X",
        "Waiting for the advisor; all tests pass.",
        "The dev server is still running.",
        "Waiting on review of PR #42.",
        "",
    ] {
        assert!(!says_waiting_on_background(text), "{text}");
    }
}

#[test]
fn build_work_is_compilers_and_target_binaries_not_servers_or_shells() {
    for (comm, exe) in [
        ("cargo", "/home/u/.cargo/bin/cargo"),
        ("rustc", "rustc"),
        ("cargo-nextest", "cargo-nextest"),
        ("make", "make"),
        (
            "aida_cli_lib-3f",
            "/repo/target/agent/deps/aida_cli_lib-3f2a",
        ),
    ] {
        assert!(is_build_work(comm, exe), "{comm} {exe}");
    }
    for (comm, exe) in [
        ("aida", "/home/u/.local/bin/aida"),
        ("node", "/usr/bin/node"),
        ("bash", "/bin/bash"),
        ("sleep", "sleep"),
        ("claude", "claude"),
        // Long-lived daemons that inherit the marker do not hold the lane.
        ("sccache", "/home/u/.cargo/bin/sccache"),
        ("aida-server", "/repo/target/debug/aida-server"),
        ("aida", "/repo/target/agent/aida"),
    ] {
        assert!(!is_build_work(comm, exe), "{comm} {exe}");
    }
}

/// A lingering sccache-like daemon carrying the lane's marker is not live
/// background work: the lane is not held for it. The process snapshot is
/// injected, so the decision does not depend on the host's process table.
#[test]
fn a_lingering_marked_daemon_does_not_hold_the_lane() {
    let proc_ = |pid: u32, comm: &str, exe: &str| MarkedProcess {
        pid,
        comm: comm.to_string(),
        exe: exe.to_string(),
    };
    let daemons = vec![
        proc_(101, "sccache", "/home/u/.cargo/bin/sccache"),
        proc_(102, "aida-server", "/repo/target/debug/aida-server"),
    ];
    let live = build_work_among(daemons.clone());
    assert!(live.is_empty(), "the daemon held the lane: {live:?}");
    let hold = ReapHold::new(std::time::Duration::from_secs(60));
    assert!(!hold.check(!live.is_empty()));
    assert!(!hold.held());

    // The same snapshot plus a real compile does hold it, for the compile only.
    let mut busy = daemons;
    busy.push(proc_(103, "rustc", "/home/u/.rustup/bin/rustc"));
    let live = build_work_among(busy);
    assert_eq!(live.iter().map(|p| p.pid).collect::<Vec<_>>(), vec![103]);
}

#[test]
fn resume_reason_needs_a_final_result_a_resumable_vendor_and_budget() {
    let waiting = Some("Waiting on the test build.");
    let done = Some("Stopping here.");
    assert_eq!(
        resume_reason(0, true, done, true, false, false),
        Some(ResumeReason::LiveWork)
    );
    assert_eq!(
        resume_reason(0, true, done, false, true, false),
        Some(ResumeReason::HeldForWork)
    );
    assert_eq!(
        resume_reason(0, true, waiting, false, false, false),
        Some(ResumeReason::SaidWaiting)
    );
    assert_eq!(
        resume_reason(0, true, done, false, true, true),
        Some(ResumeReason::Terminated),
        "a hold that ran out means the reaper killed the run: never 'finished'"
    );
    assert_eq!(resume_reason(0, true, done, false, false, false), None);
    assert_eq!(
        resume_reason(0, true, None, true, true, false),
        None,
        "a run that logged no result crashed; it did not park"
    );
    assert_eq!(resume_reason(0, false, waiting, true, false, false), None);
    assert_eq!(
        resume_reason(MAX_AUTO_RESUMES, true, waiting, true, true, false),
        None
    );
    assert!(resume_reason(MAX_AUTO_RESUMES - 1, true, waiting, false, false, false).is_some());
}

#[test]
fn a_terminated_run_is_resumed_saying_it_was_terminated_not_finished() {
    let prompt = resume_prompt(&ResumeReason::Terminated);
    assert!(prompt.contains("TERMINATED"), "{prompt}");
    assert!(prompt.contains("Rerun"), "{prompt}");
    assert!(!prompt.contains("has finished"), "{prompt}");
    for reason in [
        ResumeReason::LiveWork,
        ResumeReason::HeldForWork,
        ResumeReason::SaidWaiting,
    ] {
        assert_eq!(resume_prompt(&reason), RESUME_PROMPT);
    }
}

// ── The two ceiling paths ─────────────────────────────────────────────────

/// Ceiling path 1: the reaper hold runs out while the build is still live.
/// The hold lifts (so the reaper kills the tree) and records that it
/// expired, so the resume says TERMINATED.
#[test]
fn a_reap_hold_lifts_at_the_ceiling_and_records_that_it_expired() {
    let hold = ReapHold::new(std::time::Duration::from_millis(200));
    assert!(hold.check(true), "live work holds the reaper off");
    assert!(hold.held() && !hold.expired());
    std::thread::sleep(std::time::Duration::from_millis(250));
    assert!(!hold.check(true), "the ceiling lifts the hold");
    assert!(hold.expired(), "the work was still live at the ceiling");
    let terminated = std::cell::Cell::new(false);
    let settled = settle_background_work(
        std::time::Duration::from_secs(30),
        hold.expired(),
        Vec::new,
        || terminated.set(true),
    );
    assert_eq!(settled, Settled::Terminated);
    assert!(terminated.get(), "any survivor of the reaper is stopped");
    assert_eq!(
        resume_reason(
            0,
            true,
            Some("Stopping."),
            false,
            hold.held(),
            hold.expired()
        ),
        Some(ResumeReason::Terminated)
    );

    let quiet = ReapHold::new(std::time::Duration::from_millis(200));
    assert!(!quiet.check(false));
    assert!(!quiet.held() && !quiet.expired(), "no work, no hold");
}

/// Ceiling path 2: the vendor exited and its build outlives the launcher's
/// wait. The build is stopped and reported Terminated; work that ends in
/// time is Finished, and nothing is stopped.
#[test]
fn work_outliving_the_wait_ceiling_is_stopped_and_reported_terminated() {
    let cargo = || MarkedProcess {
        pid: 1,
        comm: "cargo".into(),
        exe: "cargo".into(),
    };
    let terminated = std::cell::Cell::new(false);
    let settled = settle_background_work(
        std::time::Duration::from_millis(300),
        false,
        || vec![cargo()],
        || terminated.set(true),
    );
    assert_eq!(settled, Settled::Terminated);
    assert!(terminated.get());

    let terminated = std::cell::Cell::new(false);
    let calls = std::cell::Cell::new(0);
    let settled = settle_background_work(
        std::time::Duration::from_secs(30),
        false,
        || {
            calls.set(calls.get() + 1);
            if calls.get() < 3 {
                vec![cargo()]
            } else {
                Vec::new()
            }
        },
        || terminated.set(true),
    );
    assert_eq!(settled, Settled::Finished);
    assert!(!terminated.get(), "finished work is not stopped");
}

/// The real stop: a marked build process outliving the ceiling is killed.
#[cfg(target_os = "linux")]
#[test]
fn terminate_background_work_stops_the_marked_build() {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .env(si::MARKER_ENV, "b044term01")
        .spawn()
        .unwrap();
    let pid = child.id();
    let live = || {
        marked_processes(si::MARKER_ENV, "b044term01")
            .into_iter()
            .filter(|p| p.pid == pid)
            .collect::<Vec<_>>()
    };
    assert_eq!(live().len(), 1);
    terminate_background_work(live);
    let status = child.wait().unwrap();
    assert!(!status.success(), "the build was killed: {status:?}");
}

#[test]
fn a_resume_rewrites_only_the_session_flag() {
    let args: Vec<String> = ["-p", "--model", "m", "--session-id", "sid", "prompt"]
        .map(String::from)
        .to_vec();
    let resumed = crate::session::resume_session_args(args, "sid").unwrap();
    assert_eq!(
        resumed,
        ["-p", "--model", "m", "--resume", "sid", "prompt"].map(String::from)
    );
    let err = crate::session::resume_session_args(vec!["-p".into()], "sid").unwrap_err();
    assert!(format!("{err:#}").contains("--session-id sid"), "{err:#}");
}

#[test]
fn resume_logs_are_fresh_files_beside_the_first() {
    let first = std::path::Path::new("/p/.aida/headless-logs/claude-x-sid.jsonl");
    assert_eq!(
        si::resume_log_path(first, 2),
        std::path::Path::new("/p/.aida/headless-logs/claude-x-sid-resume2.jsonl")
    );
}

#[test]
fn a_no_pr_report_names_the_uncommitted_paths() {
    let (line, code) = si::report(&si::Outcome::NoPr, Some(0), Some(12));
    assert_eq!(line, "implementer: no-pr uncommitted=12");
    assert_eq!(code, si::NO_PR_EXIT_CODE);
    assert_eq!(
        si::report(&si::Outcome::NoPr, Some(0), Some(0)).0,
        "implementer: no-pr"
    );
    assert_eq!(
        si::report(&si::Outcome::NoPr, Some(0), None).0,
        "implementer: no-pr"
    );
}

/// A lane whose child logged its final result but whose launcher is waiting
/// on that child's background run (to resume it) still holds its cap slot:
/// no other launch takes it.
#[test]
fn a_lane_waiting_on_its_own_background_work_still_counts_toward_the_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(".aida").join("standalone-implementers");
    std::fs::create_dir_all(&dir).unwrap();
    let log = tmp.path().join("parked.jsonl");
    std::fs::write(&log, "{\"type\":\"result\",\"is_error\":false}\n").unwrap();
    let slot = |pid: u32, spec: &str, waiting: bool| {
        std::fs::write(
            dir.join(format!("{pid}.json")),
            serde_json::json!({"pid": pid, "start": "t", "spec": spec, "log": log, "waiting": waiting})
                .to_string(),
        )
        .unwrap();
    };
    let alive = |_: u32, _: Option<&str>| true;
    slot(11, "STORY-1", true);
    let err = si::acquire_slot_with(tmp.path(), "STORY-2", 1, 500, alive)
        .err()
        .expect("the waiting lane keeps its slot");
    assert!(format!("{err:#}").contains("STORY-1"), "{err:#}");

    slot(11, "STORY-1", false);
    let guard = si::acquire_slot_with(tmp.path(), "STORY-2", 1, 500, alive)
        .expect("once it is not waiting, a finished lane frees its slot");
    drop(guard);
}

// ── Finding and waiting on the session's own work ─────────────────────────

#[cfg(target_os = "linux")]
#[test]
fn marked_processes_finds_only_the_launch_marker_value() {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .env(si::MARKER_ENV, "b044marked01")
        .spawn()
        .unwrap();
    let found = marked_processes(si::MARKER_ENV, "b044marked01");
    let other = marked_processes(si::MARKER_ENV, "b044marked02");
    let _ = child.kill();
    let _ = child.wait();
    let ours = found
        .iter()
        .find(|p| p.pid == child.id())
        .expect("the marked child");
    assert_eq!(ours.comm, "sleep");
    assert!(!is_build_work(&ours.comm, &ours.exe));
    assert!(!other.iter().any(|p| p.pid == child.id()));
}

#[test]
fn waiting_ends_when_the_work_does_or_at_the_ceiling() {
    let calls = std::cell::Cell::new(0);
    let marked = || MarkedProcess {
        pid: 1,
        comm: "cargo".into(),
        exe: "cargo".into(),
    };
    let finished = wait_for_background_work(std::time::Duration::from_secs(30), || {
        calls.set(calls.get() + 1);
        if calls.get() < 2 {
            vec![marked()]
        } else {
            Vec::new()
        }
    });
    assert!(finished);
    assert_eq!(calls.get(), 2);
    let started = std::time::Instant::now();
    assert!(!wait_for_background_work(
        std::time::Duration::from_millis(300),
        || vec![marked()]
    ));
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

// ── End to end through the launcher, with a mock vendor ───────────────────

#[cfg(target_os = "linux")]
struct Fixture {
    tmp: tempfile::TempDir,
    project: std::path::PathBuf,
    worktree: std::path::PathBuf,
    lease: String,
}

#[cfg(target_os = "linux")]
impl Fixture {
    fn new(lease: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(tmp.path().join("home").join(".aida")).unwrap();
        std::fs::create_dir_all(project.join(".aida")).unwrap();
        // pure-git: the PR lookup stays local.
        std::fs::write(
            project.join(".aida/config.toml"),
            "[forge]\nprovider = \"pure-git\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        let out = crate::test_git::git_command()
            .args(["init", "-q"])
            .arg(&worktree)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let lease_file = crate::lease_path(&project, lease);
        std::fs::create_dir_all(lease_file.parent().unwrap()).unwrap();
        let mut table: toml::Table = toml::from_str(&format!(
            "id = \"{lease}\"\nscope = \"STORY-9\"\nslug = \"story-9\"\nowner = \"tester\"\n\
             branch = \"claude/story-9\"\nstarted_at = \"2026-10-30T00:00:00Z\"\n\
             hostname = \"h\"\nrole = \"implementer\"\n"
        ))
        .unwrap();
        table.insert(
            "worktree_path".into(),
            toml::Value::String(worktree.to_string_lossy().into_owned()),
        );
        std::fs::write(&lease_file, toml::to_string_pretty(&table).unwrap()).unwrap();
        Fixture {
            tmp,
            project,
            worktree,
            lease: lease.to_string(),
        }
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.tmp.path().join(name)
    }

    /// Run the launcher against `stub_body` (the mock vendor's shell body;
    /// `$n` is its 1-based invocation, its argv lands in `args.<n>`).
    fn run(&self, stub_body: &str) -> anyhow::Result<()> {
        let mock = self.path("mock-claude.sh");
        crate::test_exec::write_executable(
            &mock,
            format!(
                "#!/bin/sh\nn=$(cat \"{count}\" 2>/dev/null || echo 0); n=$((n+1)); echo $n > \"{count}\"\n\
                 for a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"{args}.$n\"\n{stub_body}\n",
                count = self.path("count").display(),
                args = self.path("args").display(),
            ),
        );
        let home = self.path("home");
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_HOME", Some(home.to_str().unwrap())),
            ("HOME", Some(home.to_str().unwrap())),
            ("AIDA_AGENT_CMD", Some(mock.to_str().unwrap())),
            ("AIDA_HEADLESS_VENDOR", None),
            ("AIDA_CONTAINED", None),
            ("AIDA_OS_WRAP", None),
            ("AIDA_TEE_HEADLESS", Some("0")),
            (crate::gate::seat_authority::GRANT_ENV, None),
        ]);
        let grant = crate::gate::seat_authority::HeadlessChildGrant::Baseline {
            cause: "test".into(),
        };
        si::run(si::Launch {
            project_root: &self.project,
            vendor: crate::session::HeadlessVendor::Claude,
            prompt: "/aida-pickup STORY-9",
            session_id: "019e0000-0000-7000-8000-000000000044",
            spec: "STORY-9",
            lease_id: &self.lease,
            branch: "claude/story-9",
            contained: false,
            child_grant: &grant,
        })
    }

    fn invocations(&self) -> u32 {
        std::fs::read_to_string(self.path("count"))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    fn args(&self, n: u32) -> Vec<String> {
        std::fs::read_to_string(self.path(&format!("args.{n}")))
            .unwrap()
            .lines()
            .map(String::from)
            .collect()
    }
}

/// The 038d shape: the vendor ends its turn with a build still running.
/// The launcher waits for the build, then resumes the same session, which
/// finishes (here by holding its PR).
#[cfg(target_os = "linux")]
#[test]
fn a_run_parked_on_its_own_build_is_waited_for_and_resumed() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = Fixture::new("b044resume01");
    let cargo = fx.path("bin").join("cargo");
    std::fs::create_dir_all(cargo.parent().unwrap()).unwrap();
    let built = fx.path("built");
    let running = fx.path("cargo-running");
    crate::test_exec::write_executable(
        &cargo,
        format!(
            "#!/bin/sh\ntouch \"{}\"\nsleep 2\ntouch \"{}\"\n",
            running.display(),
            built.display()
        ),
    );
    // The vendor ends its turn only once its background cargo has exec'd
    // (a loaded CI runner can otherwise exit before the fork becomes cargo).
    let err = fx
        .run(&format!(
            "if [ $n -eq 1 ]; then\n  \"{cargo}\" &\n  \
             while [ ! -f \"{running}\" ]; do sleep 0.05; done\n  \
             echo '{{\"type\":\"result\",\"is_error\":false,\"result\":\"Stopping for now.\"}}'\n  exit 0\nfi\n\
             [ -f \"{built}\" ] && touch \"{saw}\"\n\
             printf '%s' '{{\"spec\":\"STORY-9\",\"branch\":\"claude/story-9\"}}' > \"$AIDA_HOLD_SIGNAL_FILE\"\n\
             echo '{{\"type\":\"result\",\"is_error\":false,\"result\":\"held\"}}'",
            cargo = cargo.display(),
            running = running.display(),
            built = built.display(),
            saw = fx.path("saw-build").display(),
        ))
        .expect_err("a held run is not an open PR");
    assert_eq!(
        crate::exit_code_for_error(&err),
        si::HELD_EXIT_CODE,
        "{err:#}"
    );
    assert_eq!(fx.invocations(), 2, "one launch, one resume");
    assert!(
        fx.path("saw-build").exists(),
        "the resume started only after the background build finished"
    );
    let first = fx.args(1);
    assert!(first.contains(&"--session-id".to_string()), "{first:?}");
    let second = fx.args(2);
    let at = second
        .iter()
        .position(|a| a == "--resume")
        .expect("--resume");
    assert_eq!(second[at + 1], "019e0000-0000-7000-8000-000000000044");
    assert!(!second.contains(&"--session-id".to_string()), "{second:?}");
    assert_eq!(second.last().map(String::as_str), Some(RESUME_PROMPT));
}

/// The 041c shape: the session keeps saying it is waiting on a build. It is
/// resumed at most [`MAX_AUTO_RESUMES`] times, then reported `no-pr` naming
/// the uncommitted work it left.
#[cfg(target_os = "linux")]
#[test]
fn a_run_that_keeps_parking_is_resumed_a_bounded_number_of_times() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = Fixture::new("b044resume02");
    std::fs::write(fx.worktree.join("half-done.rs"), "fn x() {}\n").unwrap();
    let err = fx
        .run(
            "echo '{\"type\":\"result\",\"is_error\":false,\"result\":\"Waiting on the aida-core test build.\"}'",
        )
        .expect_err("a PR-less run");
    assert_eq!(
        crate::exit_code_for_error(&err),
        si::NO_PR_EXIT_CODE,
        "{err:#}"
    );
    assert_eq!(fx.invocations(), 1 + MAX_AUTO_RESUMES);
    let text = format!("{err:#}");
    assert!(text.contains("after 3 automatic resume(s)"), "{text}");
    assert!(text.contains("1 uncommitted path(s)"), "{text}");
}

/// A run that ends its turn without parking is reported as before: no
/// resume.
#[cfg(target_os = "linux")]
#[test]
fn a_run_that_did_not_park_is_not_resumed() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = Fixture::new("b044resume03");
    let err = fx
        .run("echo '{\"type\":\"result\",\"is_error\":false,\"result\":\"I could not finish.\"}'")
        .expect_err("a PR-less run");
    assert_eq!(crate::exit_code_for_error(&err), si::NO_PR_EXIT_CODE);
    assert_eq!(fx.invocations(), 1);
    assert!(!format!("{err:#}").contains("uncommitted"), "{err:#}");
}
