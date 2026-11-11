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
fn a_no_pr_report_names_the_unshipped_work() {
    let (line, code) = si::report(
        &si::Outcome::NoPr,
        Some(0),
        Some(&Unshipped::UncommittedWork(12)),
    );
    assert_eq!(
        line,
        "implementer: no-pr reason=uncommitted-work uncommitted=12"
    );
    assert_eq!(code, si::NO_PR_EXIT_CODE);
    assert_eq!(
        si::report(
            &si::Outcome::NoPr,
            Some(0),
            Some(&Unshipped::UnpushedCommits)
        )
        .0,
        "implementer: no-pr reason=unpushed-commits"
    );
    assert_eq!(
        si::report(&si::Outcome::NoPr, Some(0), None).0,
        "implementer: no-pr"
    );
}

fn seat(head: Option<&str>, origin: OriginHead, uncommitted: usize) -> SeatState {
    SeatState {
        head: head.map(String::from),
        origin,
        uncommitted: Some(uncommitted),
        head_has_unpushed: Some(true),
    }
}

fn at(sha: &str) -> OriginHead {
    OriginHead::At(sha.to_string())
}

/// BUG-050: whether a run shipped is read from the seat's git: its `HEAD`
/// against `origin`'s branch and against the head it launched on (`a`).
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
#[test]
fn the_seat_state_says_whether_a_run_pushed_and_what_it_left() {
    let launch = Some("a");
    assert_eq!(seat(Some("b"), at("b"), 0).pushed_since(launch), Some(true));
    assert_eq!(
        seat(Some("a"), at("a"), 0).pushed_since(launch),
        Some(false)
    );
    assert_eq!(
        seat(Some("b"), at("a"), 0).pushed_since(launch),
        Some(false)
    );
    assert_eq!(
        seat(Some("b"), OriginHead::Absent, 0).pushed_since(launch),
        Some(false)
    );
    assert_eq!(
        seat(Some("b"), OriginHead::Unknown, 0).pushed_since(launch),
        None
    );
    assert_eq!(seat(None, at("b"), 0).pushed_since(launch), None);

    assert_eq!(
        seat(Some("a"), at("a"), 4).unshipped(launch),
        Some(Unshipped::UncommittedWork(4))
    );
    assert_eq!(
        seat(Some("b"), at("a"), 0).unshipped(launch),
        Some(Unshipped::UnpushedCommits)
    );
    assert_eq!(
        seat(Some("b"), OriginHead::Absent, 0).unshipped(launch),
        Some(Unshipped::UnpushedCommits)
    );
    assert_eq!(
        seat(Some("b"), at("a"), 2).unshipped(launch),
        Some(Unshipped::UncommittedWork(2)),
        "a dirty tree is named first"
    );
    assert_eq!(
        seat(Some("b"), at("b"), 3).unshipped(launch),
        None,
        "stray paths beside a push are not a stall"
    );
    assert_eq!(seat(Some("a"), at("a"), 0).unshipped(launch), None);
    assert_eq!(
        seat(Some("b"), OriginHead::Unknown, 0).unshipped(launch),
        None
    );
}

// trace:TASK-2938 | ai:codex
#[test]
fn unknown_reachability_falls_back_to_moved_and_known_non_push() {
    let launch_head = Some("a");
    for (head, origin, expected) in [
        (Some("b"), at("a"), Some(Unshipped::UnpushedCommits)),
        (
            Some("b"),
            OriginHead::Absent,
            Some(Unshipped::UnpushedCommits),
        ),
        (Some("a"), at("a"), None),
        (Some("b"), at("b"), None),
        (Some("b"), OriginHead::Unknown, None),
        (None, OriginHead::Absent, None),
    ] {
        let mut state = seat(head, origin, 0);
        state.head_has_unpushed = None;
        assert_eq!(state.unshipped(launch_head), expected, "{state:?}");
    }

    let launch_pr = (7, Some("a".to_string()));
    let launch = si::LaunchSeat {
        pr: Some(&launch_pr),
        head: launch_head,
    };
    let mut state = seat(Some("b"), at("a"), 0);
    state.head_has_unpushed = None;
    let (outcome, work) = si::settle_outcome(
        si::Outcome::PrOpened {
            pr: 7,
            head: Some("a".to_string()),
        },
        &launch,
        &state,
    );
    assert_eq!(outcome, si::Outcome::NoPr);
    assert_eq!(
        si::report(&outcome, Some(0), work.as_ref()),
        (
            "implementer: no-pr reason=unpushed-commits".to_string(),
            si::NO_PR_EXIT_CODE
        )
    );
}

/// BUG-050: a rework round starts with its PR open on the head the seat
/// launched on (`a`). The forge's PR head is not trusted alone: right after
/// a push it still reports `a`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-050.ac9d20b3 | ai:claude
#[test]
fn a_rework_round_on_its_old_pr_head_is_settled_from_the_seat() {
    use si::Outcome::{NoPr, PrOpened};
    let pr = |head: &str| PrOpened {
        pr: 7,
        head: Some(head.to_string()),
    };
    let launch_pr = (7, Some("a".to_string()));
    let rework = si::LaunchSeat {
        pr: Some(&launch_pr),
        head: Some("a"),
    };
    // Pushed, forge lagging: pr-opened on the seat's pushed head.
    let pushed = seat(Some("b"), at("b"), 0);
    assert!(!rework.is_stale(&pr("a"), &pushed));
    assert_eq!(
        si::settle_outcome(pr("a"), &rework, &pushed),
        (pr("b"), None)
    );
    // Committed, not pushed.
    let unpushed = seat(Some("b"), at("a"), 0);
    assert!(rework.is_stale(&pr("a"), &unpushed));
    assert_eq!(
        si::settle_outcome(pr("a"), &rework, &unpushed),
        (NoPr, Some(Unshipped::UnpushedCommits))
    );
    // Left uncommitted.
    let dirty = seat(Some("a"), at("a"), 4);
    let (settled, unshipped) = si::settle_outcome(pr("a"), &rework, &dirty);
    assert_eq!(
        si::report(&settled, Some(0), unshipped.as_ref()),
        (
            "implementer: no-pr reason=uncommitted-work uncommitted=4".to_string(),
            si::NO_PR_EXIT_CODE
        )
    );
    // Clean and unmoved: the round had nothing to change.
    let clean = seat(Some("a"), at("a"), 0);
    assert_eq!(
        si::settle_outcome(pr("a"), &rework, &clean),
        (pr("a"), None)
    );
    // Git cannot tell (no origin): fall back to the forge's head.
    let unknown = seat(Some("a"), OriginHead::Unknown, 4);
    assert!(rework.is_stale(&pr("a"), &unknown));
    assert!(!rework.is_stale(&pr("c"), &unknown));
    // A PR this run opened is never stale.
    let fresh = si::LaunchSeat {
        pr: None,
        head: Some("a"),
    };
    assert!(!fresh.is_stale(&pr("a"), &unpushed));
    assert_eq!(
        si::settle_outcome(NoPr, &fresh, &unpushed),
        (NoPr, Some(Unshipped::UnpushedCommits))
    );
}

/// BUG-050: the continuation a parked lane is resumed with opens with the
/// fixed instruction.
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-050.ac9cf711 | ai:claude
#[test]
fn the_resume_prompt_opens_with_the_fixed_continuation() {
    assert!(RESUME_PROMPT.starts_with(
        "Your background work has finished or must be run in the foreground now; continue and \
         finish the task."
    ));
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
        self.run_vendor(stub_body, crate::session::HeadlessVendor::Claude, false)
    }

    fn run_vendor(
        &self,
        stub_body: &str,
        vendor: crate::session::HeadlessVendor,
        contained: bool,
    ) -> anyhow::Result<()> {
        self.run_mode(stub_body, vendor, contained, 0)
    }

    fn run_mode(
        &self,
        stub_body: &str,
        vendor: crate::session::HeadlessVendor,
        contained: bool,
        mode: u8,
    ) -> anyhow::Result<()> {
        let mock = self.path("mock-claude.sh");
        // Resumes issue a real delegated child grant. The fake Codex vendor
        // acknowledges its model-free preflight separately from the child
        // invocation whose exit/publication this fixture observes.
        // trace:STORY-2475 | ai:codex
        let preflight = if mode >= 2 && vendor == crate::session::HeadlessVendor::Codex {
            "for arg in \"$@\"; do\n\
             if [ \"$arg\" = sandbox ]; then\n\
             test -n \"$AIDA_SESSION_GRANT\" || exit 42\n\
             test \"$AIDA_SESSION_ROLE\" = implementer || exit 43\n\
             echo 'seat_grant_valid=true expected_seat=true'\n\
             exit 0\nfi\ndone\n"
        } else {
            ""
        };
        crate::test_exec::write_executable(
            &mock,
            format!(
                "#!/bin/sh\n{preflight}n=$(cat \"{count}\" 2>/dev/null || echo 0); n=$((n+1)); echo $n > \"{count}\"\n\
                 for a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"{args}.$n\"\n{stub_body}\n",
                count = self.path("count").display(),
                args = self.path("args").display(),
            ),
        );
        let home = self.path("home");
        let mut env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_HOME", Some(home.to_str().unwrap())),
            ("HOME", Some(home.to_str().unwrap())),
            ("AIDA_AGENT_CMD", Some(mock.to_str().unwrap())),
            ("AIDA_HEADLESS_VENDOR", None),
            ("AIDA_CONTAINED", None),
            ("AIDA_OS_WRAP", None),
            ("AIDA_TEE_HEADLESS", Some("0")),
            (crate::gate::seat_authority::GRANT_ENV, None),
        ]);
        if mode >= 2 {
            let subject = "story-2475-fixture";
            env.set_key("AIDA_USER", subject);
            env.set_key("AIDA_SESSION_ROLE", "advisor");
            let parent = crate::gate::seat_authority::test_support::mint_grant_for(
                &self.worktree,
                subject,
                "advisor",
                &["implementer"],
            );
            env.set_key(crate::gate::seat_authority::GRANT_ENV, parent);
        }
        if mode >= 3 {
            use crate::auto_complete::{self as ac, PhaseDriver};
            let lease_path = crate::lease_path(&self.project, &self.lease);
            let mut lease = std::fs::read_to_string(&lease_path)?;
            lease.push_str("vendor_session_id = \"fixture-thread\"\n");
            std::fs::write(&lease_path, lease)?;
            let end = self.path("end-session");
            crate::test_exec::write_executable(
                &end,
                format!("#!/bin/sh\nrm -f '{}'\n", lease_path.display()),
            );
            // The advisor may leave the old status in place on an empty resume.
            // trace:TASK-3749 | ai:codex
            if mode == 3 {
                crate::test_env::enroll_synthetic_home(&self.project)?;
                let mut req = aida_core::Requirement::new("Resume".into(), String::new());
                req.spec_id = Some("STORY-9".into());
                req.status = aida_core::RequirementStatus::NeedsAttention;
                aida_core::object_store::write_object(
                    &self.project.join(".aida-store/objects"),
                    &req,
                )?;
                assert_eq!(
                    crate::phase_gates::spec_status(&self.project, "STORY-9"),
                    Some(aida_core::RequirementStatus::NeedsAttention)
                );
            }
            let mut real = crate::RealPhaseDriver::new(
                self.project.clone(),
                "STORY-9".into(),
                "test".into(),
                None,
                true,
                None,
                crate::AutonomyMode::Default,
                "token".into(),
                false,
                false,
                false,
                false,
                ac::LifecycleSkip::default(),
                ac::AutoCompleteVariant::Full,
            );
            real.aida_exe = end;
            real.contained = contained;
            real.implementer_session = Some("019e0000-0000-7000-8000-000000000044".into());
            real.implementer_vendor = Some(vendor);
            real.implementer_lease = Some(self.lease.clone());
            real.implementer_worktree = Some(self.worktree.clone());
            real.branch = Some("claude/story-9".into());
            let mut forge = crate::forge::fake::RecordingForge::new();
            forge.open_for_branch = match mode {
                4 => crate::forge::ChangeLookup::Found(crate::forge::ChangeRef {
                    id: 4242,
                    url: "https://example.invalid/4242".into(),
                    branch: "claude/story-9".into(),
                    base: "main".into(),
                    title: None,
                }),
                5 => crate::forge::ChangeLookup::Unreachable("network unavailable".into()),
                10 => crate::forge::ChangeLookup::CliFailed("malformed response".into()),
                _ => crate::forge::ChangeLookup::NoChange,
            };
            // Non-receipt resumes retain their pre-existing failure classification.
            // trace:TASK-3608 | ai:codex
            if mode == 11 {
                forge.open_for_spec =
                    crate::forge::ChangeLookup::CliFailed("malformed response".into());
            }
            if mode == 12 {
                real.branch = None;
            }
            real.forge_factory = Some(forge.factory());
            // Exercise the real advisor-resume method inside final orchestration.
            // Only the later CI/review/merge phases are outside this fixture.
            struct ResumeDriver(crate::RealPhaseDriver);
            impl PhaseDriver for ResumeDriver {
                fn run_implementer(&mut self) -> Result<ac::ImplementerOutcome, ac::PhaseFailure> {
                    Ok(ac::ImplementerOutcome::Punted {
                        reason: "decision".into(),
                    })
                }
                fn run_advisor(&mut self) -> Result<ac::AdvisorOutcome, ac::PhaseFailure> {
                    Ok(ac::AdvisorOutcome::Resolved {
                        answer: "continue".into(),
                        reasoning: "test decision".into(),
                    })
                }
                fn resume_implementer(
                    &mut self,
                    answer: &str,
                ) -> Result<ac::ImplementerOutcome, ac::PhaseFailure> {
                    self.0.resume_implementer(answer)
                }
                fn finish_ci(&mut self) -> Result<(), ac::PhaseFailure> {
                    Ok(())
                }
                fn run_reviewer(&mut self) -> Result<ac::ReviewerOutcome, ac::PhaseFailure> {
                    panic!("through-CI only")
                }
                fn merge(&mut self) -> Result<(), ac::PhaseFailure> {
                    panic!("through-CI only")
                }
                fn pull(&mut self) -> Result<(), ac::PhaseFailure> {
                    panic!("through-CI only")
                }
                fn build(&mut self) -> Result<(), ac::PhaseFailure> {
                    panic!("through-CI only")
                }
                fn hint_context(&self) -> ac::HintContext {
                    self.0.hint_context()
                }
            }
            let mut d = ResumeDriver(real);
            let result = ac::orchestrate(
                &mut d,
                "STORY-9",
                ac::AutoCompleteVariant::ThroughCi,
                true,
                ac::EscalateMode::Blocks,
            );
            match mode {
                3 => {
                    assert!(result.no_change, "{result:?}");
                    // Follow the real resumed result through drain persistence.
                    // trace:TASK-3608 | ai:codex
                    crate::drain_state::DrainState::new_single("STORY-9", "test", false)
                        .write(&self.project)
                        .unwrap();
                    crate::drain_state::set_member_result(&self.project, "STORY-9", &result, None);
                    let persisted = crate::drain_state::DrainState::read(&self.project).unwrap();
                    assert_eq!(
                        persisted.members[0].state,
                        crate::drain_state::STATE_NO_CHANGE
                    );
                    assert!(persisted.members[0].finished_at.is_some());

                    assert_eq!(result.exit_code, 0);
                    assert!(result.shipped_spec_id.is_none());
                    assert!(
                        !lease_path.exists(),
                        "no-change releases the seat immediately"
                    );
                }
                4 => {
                    assert!(!result.no_change, "{result:?}");
                    assert!(result.failure.is_none(), "{result:?}");
                    assert_eq!(d.0.pr_number, Some(4242));
                }
                5 | 10 => {
                    assert!(result.inconclusive_reason.is_some(), "{result:?}");
                    assert!(!result.no_change);
                }
                11 | 12 => {
                    let failure = result.failure.as_ref().expect("ordinary resume failure");
                    assert_eq!(
                        failure.kind,
                        if mode == 11 {
                            ac::FailureKind::Failed
                        } else {
                            ac::FailureKind::NoPr
                        },
                        "{result:?}"
                    );
                    assert!(!result.no_change);
                    assert!(result.inconclusive_reason.is_none());
                }
                6 | 7 => {
                    let failure = result.failure.as_ref().expect("failed publication/child");
                    assert_eq!(failure.kind, ac::FailureKind::Failed, "{result:?}");
                    assert!(failure.kind.is_shelvable());
                    assert!(!result.no_change);
                }
                8 => assert!(result.punt_reason.is_some(), "{result:?}"),
                9 => assert!(result.held_reason.is_some(), "{result:?}"),
                _ => unreachable!(),
            }
            assert!(!crate::ship_from_seat::take_no_change(
                &self.project,
                &self.lease
            ));
            return Ok(());
        }
        let grant = crate::gate::seat_authority::HeadlessChildGrant::Baseline {
            cause: "test".into(),
        };
        if mode > 0 {
            let tee = crate::headless_tee::TeeOptions {
                enabled: false,
                label: None,
            };
            let log = self.path("child.jsonl");
            let status = if mode == 1 {
                crate::session::run_vendor_headless_to_exit(
                    Some(&self.project),
                    vendor,
                    "/aida-pickup STORY-9",
                    "019e0000-0000-7000-8000-000000000044",
                    &log,
                    &tee,
                    contained,
                    Some(&self.lease),
                    "implementer",
                    &grant,
                    Some((&self.worktree, "claude/story-9")),
                    true,
                )?
            } else {
                crate::queue_cmd::run_queue_resume_to_exit(
                    Some(&self.project),
                    &crate::session::HeadlessResume {
                        vendor,
                        session_id: "019e0000-0000-7000-8000-000000000044",
                        resume_id: "fixture-thread",
                    },
                    "/aida-pickup STORY-9",
                    &log,
                    &self.worktree,
                    &tee,
                    contained,
                    "implementer",
                    (&self.project, "STORY-9", &self.lease),
                    true,
                )?
            };
            anyhow::ensure!(status.success(), "mock child failed");
            return Ok(());
        }
        si::run_for_test(si::Launch {
            project_root: &self.project,
            vendor,
            prompt: "/aida-pickup STORY-9",
            session_id: "019e0000-0000-7000-8000-000000000044",
            spec: "STORY-9",
            lease_id: &self.lease,
            branch: "claude/story-9",
            contained,
            child_grant: &grant,
            host_main: contained.then_some(self.project.as_path()),
        })
    }

    /// [`Fixture::run`] against a mock forge whose every PR lookup on the
    /// run's branch answers `pr`.
    fn run_with_pr(
        &self,
        pr: Option<(u64, Option<String>)>,
        stub_body: &str,
    ) -> anyhow::Result<()> {
        si::TEST_PR.with(|p| *p.borrow_mut() = Some(pr));
        let result = self.run(stub_body);
        si::TEST_PR.with(|p| *p.borrow_mut() = None);
        result
    }

    /// `git` in `dir`, with an identity; its trimmed stdout.
    fn git(&self, dir: &std::path::Path, args: &[&str]) -> String {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Put the seat on the lease branch with one commit, pushed to a bare
    /// `origin`: a rework round's seat at launch. Returns that head.
    fn with_origin(&self) -> String {
        let origin = self.path("origin.git");
        self.git(
            self.tmp.path(),
            &["init", "-q", "--bare", origin.to_str().unwrap()],
        );
        let wt = &self.worktree;
        self.git(wt, &["checkout", "-q", "-b", "claude/story-9"]);
        self.git(wt, &["commit", "-q", "--allow-empty", "-m", "round 1"]);
        self.git(wt, &["remote", "add", "origin", origin.to_str().unwrap()]);
        self.git(wt, &["push", "-q", "origin", "claude/story-9"]);
        self.git(wt, &["rev-parse", "HEAD"])
    }

    /// The mock vendor's shell for `git <args>` in the seat, with an identity.
    fn seat_git(&self, args: &str) -> String {
        format!(
            "git -C \"{}\" -c user.name=t -c user.email=t@t {args}",
            self.worktree.display()
        )
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
// trace:BUG-b6b1278890774ef993d2ebc327862470-050.ac9cf711 | ai:claude
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

/// BUG-050: a rework round starts with its PR open on its launch head. A
/// session that leaves work uncommitted and ends its turn while its build
/// runs is resumed (the open PR is not a push), and the stall is reported
/// `no-pr reason=uncommitted-work`, not `pr-opened` with the old head.
#[cfg(target_os = "linux")]
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-050.ac9d20b3 | ai:claude
#[test]
fn a_rework_round_parked_on_its_old_pr_head_is_resumed() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = Fixture::new("b050rework01");
    let head = fx.with_origin();
    let cargo = fx.path("bin").join("cargo");
    std::fs::create_dir_all(cargo.parent().unwrap()).unwrap();
    let running = fx.path("cargo-running");
    crate::test_exec::write_executable(
        &cargo,
        format!("#!/bin/sh\ntouch \"{}\"\nsleep 2\n", running.display()),
    );
    let err = fx
        .run_with_pr(
            Some((7, Some(head))),
            &format!(
                "if [ $n -eq 1 ]; then\n  echo wip > \"{wt}/wip.rs\"\n  \"{cargo}\" &\n  \
                 while [ ! -f \"{running}\" ]; do sleep 0.05; done\n  \
                 echo '{{\"type\":\"result\",\"is_error\":false,\"result\":\"Stopping for now.\"}}'\n  exit 0\nfi\n\
                 echo '{{\"type\":\"result\",\"is_error\":false,\"result\":\"Out of time.\"}}'",
                wt = fx.worktree.display(),
                cargo = cargo.display(),
                running = running.display(),
            ),
        )
        .expect_err("a stalled rework round is not pr-opened");
    assert_eq!(
        crate::exit_code_for_error(&err),
        si::NO_PR_EXIT_CODE,
        "{err:#}"
    );
    assert_eq!(fx.invocations(), 2, "resumed although its PR was open");
    assert!(fx.args(2).contains(&"--resume".to_string()));
    let text = format!("{err:#}");
    assert!(text.contains("after 1 automatic resume(s)"), "{text}");
    assert!(text.contains("1 uncommitted path(s)"), "{text}");
}

/// BUG-050: a rework round that committed but did not push is not shipped,
/// even though its PR is open: `no-pr reason=unpushed-commits`.
#[cfg(target_os = "linux")]
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
#[test]
fn a_rework_round_that_committed_but_did_not_push_is_not_shipped() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = Fixture::new("b050rework02");
    let head = fx.with_origin();
    let err = fx
        .run_with_pr(
            Some((7, Some(head))),
            &format!(
                "{}\necho '{{\"type\":\"result\",\"is_error\":false,\"result\":\"Committed.\"}}'",
                fx.seat_git("commit -q --allow-empty -m 'round 2'")
            ),
        )
        .expect_err("an unpushed round is not pr-opened");
    assert_eq!(
        crate::exit_code_for_error(&err),
        si::NO_PR_EXIT_CODE,
        "{err:#}"
    );
    assert_eq!(fx.invocations(), 1, "it did not park");
    assert!(
        format!("{err:#}").contains("commits it did not push"),
        "{err:#}"
    );
}

/// BUG-050: a rework round that pushed is shipped even while the forge
/// still reports the PR's old head (it lags a push by seconds).
#[cfg(target_os = "linux")]
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
#[test]
fn a_pushed_rework_round_is_shipped_while_the_forge_lags() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = Fixture::new("b050rework03");
    let head = fx.with_origin();
    fx.run_with_pr(
        Some((7, Some(head))),
        &format!(
            "{}\n{}\necho '{{\"type\":\"result\",\"is_error\":false,\"result\":\"Pushed.\"}}'",
            fx.seat_git("commit -q --allow-empty -m 'round 2'"),
            fx.seat_git("push -q origin claude/story-9")
        ),
    )
    .expect("a pushed round is pr-opened");
    assert_eq!(fx.invocations(), 1);
}

/// BUG-050: the `--auto-complete` drain's phase-1 implementer runs in the
/// same resume loop: a run that parks is resumed with the continuation, and
/// a run that punted or held is done.
#[cfg(unix)]
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
#[test]
fn the_drain_implementer_gets_the_same_resume_safety_net() {
    use std::os::unix::process::ExitStatusExt as _;
    let tmp = tempfile::tempdir().unwrap();
    let hold = tmp.path().join("hold.json");
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        (crate::punt::SIGNAL_FILE_ENV, None),
        (
            crate::punt::HOLD_SIGNAL_FILE_ENV,
            Some(hold.to_str().unwrap()),
        ),
    ]);
    let drain = |first: &'static str| {
        let calls = std::cell::RefCell::new(Vec::new());
        let log = tmp.path().join(format!("drain-{}.jsonl", first.len()));
        crate::session::drain_lane_with_resume(
            "b050drain01",
            tmp.path(),
            "claude/story-9",
            "/aida-pickup STORY-9",
            &log,
            |prompt, log, resume, _hold| {
                calls.borrow_mut().push((prompt.to_string(), resume));
                let text = if calls.borrow().len() == 1 {
                    first
                } else {
                    "Stopped."
                };
                std::fs::write(
                    log,
                    format!("{{\"type\":\"result\",\"is_error\":false,\"result\":\"{text}\"}}\n"),
                )
                .unwrap();
                Ok(std::process::ExitStatus::from_raw(0))
            },
        )
        .unwrap();
        calls.into_inner()
    };
    assert_eq!(
        drain("Waiting on the cargo test build."),
        vec![
            ("/aida-pickup STORY-9".to_string(), false),
            (RESUME_PROMPT.to_string(), true),
        ]
    );
    std::fs::write(&hold, "{}").unwrap();
    assert_eq!(
        drain("Held; waiting on the cargo test build."),
        vec![("/aida-pickup STORY-9".to_string(), false)],
        "a held lane is done"
    );
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

// Exercise the actual standalone launcher after a mock Codex child exits;
// the existing commit seam observes the publication call on the host thread.
// trace:STORY-2475 | ai:codex
#[cfg(target_os = "linux")]
#[test]
fn story_2475_all_codex_lane_exits_publish_after_child_exit() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    use crate::session::HeadlessVendor;
    use crate::ship_from_seat::{with_post_exit_step, PostExitCommit};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    for mode in [0, 1, 2] {
        for (vendor, contained, count) in [
            (HeadlessVendor::Codex, true, 1),
            (HeadlessVendor::Codex, false, 0),
            (HeadlessVendor::Claude, false, 0),
        ] {
            let fx = Fixture::new("c0ffee247501");
            fx.git(&fx.project, &["init", "-q", "-b", "main"]);
            std::fs::write(
                fx.project.join(".aida/config.toml"),
                "[forge]\nprovider = \"pure-git\"\n[contained.vendors]\ncodex = \"native\"\n",
            )
            .unwrap();
            fx.git(&fx.project, &["add", ".aida/config.toml"]);
            fx.git(&fx.project, &["commit", "-qm", "trusted native policy"]);
            fx.git(
                &fx.project,
                &["update-ref", "refs/remotes/origin/main", "HEAD"],
            );
            let _ambient = crate::seat_clone::LauncherMainPin::new(&fx.project);

            let observed = Arc::new(AtomicUsize::new(0));
            let calls = observed.clone();
            let marker = fx.path("child-exited");
            let child = format!("touch '{}'\n", marker.display());
            // Reproduce the CI refusal: resumes validate policy in the seat,
            // so an unrelated empty repository must not inherit the host opt-in.
            // trace:STORY-2475 | ai:codex
            if mode >= 2 && vendor == HeadlessVendor::Codex && contained {
                let error = fx
                    .run_mode(&child, vendor, contained, mode)
                    .expect_err("an unprovisioned seat has no trusted native policy");
                assert!(
                    format!("{error:#}").contains("cannot run an AIDA-contained session"),
                    "{error:#}"
                );
                assert!(!marker.exists(), "the refused resume must not launch");
            }
            // A real seated clone carries origin/main and its trusted policy.
            // Keep the policy check intact and repair the fixture for every case.
            std::fs::remove_dir_all(&fx.worktree).unwrap();
            fx.git(
                &fx.project,
                &[
                    "clone",
                    "-q",
                    fx.project.to_str().unwrap(),
                    fx.worktree.to_str().unwrap(),
                ],
            );
            fx.git(&fx.worktree, &["checkout", "-qb", "claude/story-9"]);
            let check_marker = marker.clone();
            let expected_tree = fx.worktree.clone();
            si::TEST_PR.with(|p| *p.borrow_mut() = Some(None));
            let result = with_post_exit_step(
                Box::new(move |_, tree, spec, lane, clean| {
                    assert!(check_marker.exists(), "publication must follow child exit");
                    assert_eq!(tree, expected_tree);
                    assert_eq!(spec, "STORY-9");
                    assert_eq!(lane, "c0ffee247501");
                    assert!(clean);
                    calls.fetch_add(1, Ordering::SeqCst);
                    PostExitCommit::NotASeat
                }),
                || fx.run_mode(&child, vendor, contained, mode),
            );
            si::TEST_PR.with(|p| *p.borrow_mut() = None);
            assert_eq!(
                result.is_ok(),
                mode != 0,
                "mode={mode} vendor={vendor:?} contained={contained}: {result:?}"
            );
            assert!(marker.exists(), "mock child must have run: mode={mode} vendor={vendor:?} contained={contained}: {result:?}");
            assert_eq!(
                observed.load(Ordering::SeqCst),
                count,
                "mode={mode} vendor={vendor:?} contained={contained}: {result:?}"
            );
        }
    }
}

/// A clean seat can advance to newer main without producing any local commits.
// trace:BUG-2464 | ai:codex
#[test]
fn bug_2464_fast_forward_to_remote_main_is_not_unpushed_work() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {:?}", out);
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    git(&["commit", "-q", "--allow-empty", "-m", "launch"]);
    let launch_head = git(&["rev-parse", "HEAD"]);
    git(&["branch", "seat"]);
    git(&["commit", "-q", "--allow-empty", "-m", "new remote main"]);
    let remote_head = git(&["rev-parse", "HEAD"]);
    let remote = tempfile::tempdir().unwrap();
    git(&["init", "-q", "--bare", remote.path().to_str().unwrap()]);
    git(&["remote", "add", "origin", remote.path().to_str().unwrap()]);
    git(&["push", "-q", "origin", "main"]);
    assert_eq!(git(&["rev-parse", "origin/main"]), remote_head);
    git(&["checkout", "-q", "seat"]);
    git(&["merge", "--ff-only", "origin/main"]);
    let mut state = SeatState::read(root, "seat");
    assert_eq!(state.origin, OriginHead::Absent);
    assert_ne!(state.head.as_deref(), Some(launch_head.as_str()));
    assert_eq!(state.uncommitted, Some(0));
    assert_eq!(state.head_has_unpushed, Some(false));
    assert_eq!(state.unshipped(Some(&launch_head)), None);
    let launch = si::LaunchSeat {
        pr: None,
        head: Some(&launch_head),
    };
    let (outcome, work) = si::settle_outcome(
        si::Outcome::Punted {
            reason: "design fork".into(),
        },
        &launch,
        &state,
    );
    assert_eq!(
        si::report(&outcome, Some(0), work.as_ref()).0,
        "implementer: punted reason=design fork"
    );
    assert_eq!(
        si::settle_outcome(si::Outcome::NoPr, &launch, &state),
        (si::Outcome::NoPr, None)
    );

    git(&["commit", "-q", "--allow-empty", "-m", "local work"]);
    state.head = seat_head(root);
    state.head_has_unpushed = head_has_unpushed_commits(root);
    assert_eq!(state.head_has_unpushed, Some(true));
    assert_eq!(
        state.unshipped(Some(&launch_head)),
        Some(Unshipped::UnpushedCommits)
    );
    // A different remote also establishes that the commit was published.
    git(&[
        "update-ref",
        "refs/remotes/backup/seat",
        state.head.as_deref().unwrap(),
    ]);
    state.head_has_unpushed = head_has_unpushed_commits(root);
    assert_eq!(state.head_has_unpushed, Some(false));
    assert_eq!(state.unshipped(Some(&launch_head)), None);
    git(&["update-ref", "-d", "refs/remotes/backup/seat"]);
    state.head_has_unpushed = None;
    // trace:TASK-2938 | ai:codex
    assert_eq!(
        state.unshipped(Some(&launch_head)),
        Some(Unshipped::UnpushedCommits),
        "unreadable reachability falls back to the moved, unpushed head"
    );
    assert_eq!(head_has_unpushed_commits(&root.join("missing")), None);
}

// trace:BUG-2488 | ai:codex
#[cfg(target_os = "linux")]
#[test]
fn bug_2488_noncontained_queue_resume_mirrors_hold_after_real_child_exit() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = Fixture::new("c0ffee248808");
    let _ambient = crate::seat_clone::LauncherMainPin::new(&fx.project);
    let host_marker = crate::punt::hold_signal_path(&fx.project, "STORY-9");
    let (_, old_hold) = si::provision_signals(&fx.project, "STORY-9").unwrap();
    crate::punt::write_hold_signal(
        &old_hold,
        &crate::punt::HoldSignal {
            spec: "STORY-9".into(),
            branch: "old-attempt".into(),
            reason: None,
            spec_key: None,
        },
    )
    .unwrap();
    fx.run_mode(
        r#"test -n "$AIDA_HOLD_SIGNAL_FILE" || exit 41
printf '%s\n' '{"spec":"STORY-9","branch":"claude/story-9","reason":"new hold"}' > "$AIDA_HOLD_SIGNAL_FILE"
"#,
        crate::session::HeadlessVendor::Codex,
        false,
        2,
    ).unwrap();
    assert_eq!(fx.invocations(), 1);
    let hold = crate::punt::read_hold_signal(&host_marker).expect("resume hold reaches host");
    assert_eq!(hold.reason.as_deref(), Some("new hold"));
    assert!(crate::queue_cmd::held_resume_reentry_allowed(
        true,
        &aida_core::RequirementStatus::Done,
        true,
    ));
}

// Real resume routing, child launch and receipt classification through the
// advisor branch of orchestrate; later phases stop at the through-CI boundary.
// trace:TASK-3608 | ai:codex
#[cfg(target_os = "linux")]
#[test]
fn task_3608_real_resume_receipts_precedence_and_forge_errors() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    use crate::ship_from_seat::{
        record_queue_exit, with_post_exit_step, PostExitCommit, QueueExitOutcome,
    };
    for mode in 3..=12 {
        let fx = Fixture::new("task3608resume");
        fx.git(&fx.project, &["init", "-q", "-b", "main"]);
        std::fs::write(
            fx.project.join(".aida/config.toml"),
            "[forge]\nprovider = \"pure-git\"\n[contained.vendors]\ncodex = \"native\"\n",
        )
        .unwrap();
        fx.git(&fx.project, &["add", ".aida/config.toml"]);
        fx.git(&fx.project, &["commit", "-qm", "native policy"]);
        fx.git(
            &fx.project,
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
        );
        std::fs::remove_dir_all(&fx.worktree).unwrap();
        fx.git(
            &fx.project,
            &[
                "clone",
                "-q",
                fx.project.to_str().unwrap(),
                fx.worktree.to_str().unwrap(),
            ],
        );
        fx.git(&fx.worktree, &["checkout", "-qb", "claude/story-9"]);
        let _pin = crate::seat_clone::LauncherMainPin::new(&fx.project);
        let main = fx.project.clone();
        let lease = fx.lease.clone();
        let marker = fx.path("child-exited");
        let expected_marker = marker.clone();
        let child = format!(
            "touch '{}'\nexit {}",
            marker.display(),
            if mode == 7 { 1 } else { 0 }
        );
        with_post_exit_step(
            Box::new(move |_, _, _, _, _| {
                assert!(expected_marker.exists());
                if mode == 6 {
                    return PostExitCommit::Failed("bundle export refused".into());
                }
                // Inject the host no-change observation using the production writer,
                // leaving the real queue-resume route to deliver it to the driver.
                if mode < 11 {
                    record_queue_exit(&main, &lease, true, true, || Ok(QueueExitOutcome::NoChange))
                        .unwrap();
                }
                if mode == 8 {
                    crate::punt::write_signal(
                        &crate::punt::signal_path(&main, "STORY-9"),
                        &crate::punt::PuntSignal {
                            spec: "STORY-9".into(),
                            category: aida_core::PuntCategory::DesignFork,
                            detail: "decision".into(),
                            lean: None,
                        },
                    )
                    .unwrap();
                }
                if mode == 9 {
                    crate::punt::write_hold_signal(
                        &crate::punt::hold_signal_path(&main, "STORY-9"),
                        &crate::punt::HoldSignal {
                            spec: "STORY-9".into(),
                            branch: "claude/story-9".into(),
                            reason: Some("manual gate".into()),
                            spec_key: None,
                        },
                    )
                    .unwrap();
                }
                PostExitCommit::NotASeat
            }),
            || fx.run_mode(&child, crate::session::HeadlessVendor::Codex, true, mode),
        )
        .unwrap();
        assert!(marker.exists());
    }
}
