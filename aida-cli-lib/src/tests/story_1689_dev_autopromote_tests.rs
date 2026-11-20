//! STORY-1689: `aida dev build --if-main-green`. Every test runs against
//! temp dirs and fake steps (a fake forge verdict, a fake git, a fake
//! spawn and fake build steps); nothing fetches, builds or launches.
// trace:STORY-1689 | ai:claude

use super::*;
use crate::dev_build::{PromotionMeta, RetentionRerecord, PROMOTED_FILE};
use crate::forge::CommitCi;
use std::fs;

const LIVE_SHA: &str = "1111111111111111111111111111111111111111";
const TARGET: &str = "2222222222222222222222222222222222222222";

struct Fixture {
    _tmp: tempfile::TempDir,
    layout: DevLayout,
    paths: Paths,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let layout = DevLayout {
        repo: root.join("repo"),
        target_dir: root.join("repo").join("target"),
        live_dir: root.join("share").join("live"),
        link_dir: root.join("bin"),
    };
    fs::create_dir_all(layout.repo.join(".git")).unwrap();
    fs::create_dir_all(&layout.live_dir).unwrap();
    let paths = Paths {
        log: root.join("home").join("dev-autopromote.log"),
        build_logs: root.join("home").join("dev-autopromote"),
    };
    let f = Fixture {
        _tmp: tmp,
        layout,
        paths,
    };
    set_live(&f, LIVE_SHA, Some(canonical(&f.layout.repo)));
    f
}

fn set_live(f: &Fixture, sha: &str, repo: Option<String>) {
    let meta = PromotionMeta {
        sha: sha.into(),
        allow_dirty: false,
        promoted_at: "2026-10-22T00:00:00Z".into(),
        repo,
        ci_comparison: None,
    };
    fs::write(
        f.layout.live_dir.join(PROMOTED_FILE),
        serde_json::to_string(&meta).unwrap(),
    )
    .unwrap();
}

fn clean_main() -> CheckoutState {
    CheckoutState {
        branch: Some("main".into()),
        has_upstream: true,
        head: Some(LIVE_SHA.into()),
        ..Default::default()
    }
}

/// Fake tick steps: every call is recorded by name.
struct Fake {
    probes: Vec<CheckoutState>,
    fetch: Result<(), String>,
    target: String,
    ancestor: Result<bool, String>,
    ci: Result<CommitCi, String>,
    /// Per-commit CI, overriding `ci` (BUG-2687).
    ci_by: std::collections::HashMap<String, Result<CommitCi, String>>,
    /// origin/main's first-parent line, newest first (BUG-2687).
    line: Vec<String>,
    /// HEAD (BUG-2687).
    head: String,
    /// The commit expected at the fast-forward and the launch.
    expect: String,
    /// `(ancestor, descendant)` pairs that are NOT ancestry, overriding
    /// `ancestor` (BUG-2687).
    not_ancestor: Vec<(String, String)>,
    wave: Result<(), String>,
    ff: Result<(), String>,
    spawn: Result<(Option<u32>, String), String>,
    alive: bool,
    calls: Vec<String>,
}

impl Fake {
    fn green() -> Self {
        Self {
            probes: vec![clean_main()],
            fetch: Ok(()),
            target: TARGET.into(),
            ancestor: Ok(true),
            ci: Ok(CommitCi::Green),
            ci_by: Default::default(),
            line: vec![TARGET.into(), LIVE_SHA.into()],
            head: LIVE_SHA.into(),
            expect: TARGET.into(),
            not_ancestor: Vec::new(),
            wave: Ok(()),
            ff: Ok(()),
            spawn: Ok((Some(4242), "detached pid 4242".into())),
            alive: true,
            calls: Vec::new(),
        }
    }

    fn called(&self, name: &str) -> bool {
        self.calls.iter().any(|c| c == name)
    }
}

fn err<T>(r: &Result<T, String>) -> Result<T>
where
    T: Clone,
{
    r.clone().map_err(|e| anyhow::anyhow!("{e}"))
}

impl AutopromoteSteps for Fake {
    fn probe(&mut self, _repo: &Path) -> Result<CheckoutState> {
        self.calls.push("probe".into());
        let state = if self.probes.len() > 1 {
            self.probes.remove(0)
        } else {
            self.probes[0].clone()
        };
        Ok(state)
    }
    fn fetch(&mut self, _repo: &Path) -> Result<()> {
        self.calls.push("fetch".into());
        err(&self.fetch)
    }
    fn rev_parse(&mut self, _repo: &Path, rev: &str) -> Result<String> {
        self.calls.push(format!("rev-parse {rev}"));
        Ok(if rev == "HEAD" {
            self.head.clone()
        } else {
            self.target.clone()
        })
    }
    fn first_parent(&mut self, _repo: &Path, tip: &str, max: usize) -> Result<Vec<String>> {
        assert_eq!(tip, self.target, "the walk starts at origin/main");
        assert_eq!(max, GREEN_SEARCH);
        self.calls.push("first-parent".into());
        Ok(self.line.iter().take(max).cloned().collect())
    }
    fn is_ancestor(&mut self, _repo: &Path, a: &str, d: &str) -> Result<bool> {
        self.calls.push("is-ancestor".into());
        if self.not_ancestor.iter().any(|(x, y)| x == a && y == d) {
            return Ok(false);
        }
        err(&self.ancestor)
    }
    fn commit_ci(&mut self, _repo: &Path, sha: &str) -> Result<CommitCi> {
        self.calls.push("ci".into());
        self.calls.push(format!("ci {sha}"));
        err(self.ci_by.get(sha).unwrap_or(&self.ci))
    }
    fn wave_guard(&mut self, _repo: &Path, _binary: &Path) -> Result<()> {
        self.calls.push("wave".into());
        err(&self.wave)
    }
    fn ff_only(&mut self, _repo: &Path, sha: &str) -> Result<()> {
        assert_eq!(sha, self.expect);
        self.calls.push("ff".into());
        err(&self.ff)
    }
    fn spawn_child(
        &mut self,
        _repo: &Path,
        sha: &str,
        _log: &Path,
    ) -> Result<(Option<u32>, String)> {
        assert_eq!(sha, self.expect);
        self.calls.push("spawn".into());
        err(&self.spawn)
    }
    fn pid_alive(&mut self, _pid: u32) -> bool {
        self.calls.push("pid-alive".into());
        self.alive
    }
}

fn tick(f: &Fixture, fake: &mut Fake) -> TickOutcome {
    run_tick(&f.layout, &f.paths, fake, Instant::now() + TICK_BUDGET).unwrap()
}

#[test]
fn checkout_build_lock_skips_tick_before_fetch() {
    let f = fixture();
    let _lock = crate::dev_build::try_checkout_build_lock(&f.layout.repo)
        .unwrap()
        .unwrap();
    let mut fake = Fake::green();
    assert!(matches!(
        tick(&f, &mut fake),
        TickOutcome::Skipped(Skip::InFlight(why)) if why.contains("build in progress")
    ));
    assert!(fake.calls.is_empty(), "{:?}", fake.calls);
}

fn skip_reason(outcome: &TickOutcome) -> &'static str {
    match outcome {
        TickOutcome::Skipped(skip) => skip.reason(),
        other => panic!("expected a skip, got {other:?}"),
    }
}

/// A skip never fast-forwards and never launches a build.
fn assert_untouched(fake: &Fake) {
    assert!(!fake.called("ff"), "{:?}", fake.calls);
    assert!(!fake.called("spawn"), "{:?}", fake.calls);
}

fn put_state(f: &Fixture, phase: Phase, pid: Option<u32>, reported: bool, at: &str) {
    write_state(
        &f.layout.live_dir,
        &State {
            sha: TARGET.into(),
            phase,
            pid,
            log: f.paths.build_logs.join("b.log").display().to_string(),
            reported,
            at: at.into(),
            error: None,
        },
    )
    .unwrap();
}

// --- the promote path -----------------------------------------------------

// trace:STORY-1689.ac3e63c0 | ai:claude
#[test]
fn green_main_fast_forwards_then_launches_and_records_running() {
    let f = fixture();
    let mut fake = Fake::green();
    let outcome = tick(&f, &mut fake);
    assert_eq!(
        outcome,
        TickOutcome::Launched {
            sha: TARGET.into(),
            how: "detached pid 4242".into(),
            passed_over: Vec::new(),
        }
    );
    let ff = fake.calls.iter().position(|c| c == "ff").unwrap();
    let spawn = fake.calls.iter().position(|c| c == "spawn").unwrap();
    let ci = fake.calls.iter().position(|c| c == "ci").unwrap();
    assert!(ci < ff && ff < spawn, "{:?}", fake.calls);
    // C1: the checkout is re-probed after the CI read, right before the ff.
    assert_eq!(fake.calls[ff - 1], "probe", "{:?}", fake.calls);
    let state = read_state(&f.layout.live_dir).unwrap();
    assert_eq!(state.sha, TARGET);
    assert_eq!(state.phase, Phase::Running);
    assert_eq!(state.pid, Some(4242));
    assert!(state
        .log
        .starts_with(&f.paths.build_logs.display().to_string()));
}

// --- each skip reason -----------------------------------------------------

#[test]
fn not_live_source_skips_before_any_fetch() {
    for repo in [None, Some("/somewhere/else".to_string())] {
        let f = fixture();
        set_live(&f, LIVE_SHA, repo);
        let mut fake = Fake::green();
        assert_eq!(skip_reason(&tick(&f, &mut fake)), "not-live-source");
        assert!(fake.calls.is_empty(), "{:?}", fake.calls);
    }
    let f = fixture();
    fs::remove_file(f.layout.live_dir.join(PROMOTED_FILE)).unwrap();
    let mut fake = Fake::green();
    assert_eq!(skip_reason(&tick(&f, &mut fake)), "not-live-source");
    assert!(fake.calls.is_empty(), "{:?}", fake.calls);
}

// trace:STORY-1689.ac3e63c0 | ai:claude
#[test]
fn worktree_off_main_and_dirty_skip_before_any_fetch() {
    let cases = [
        (
            CheckoutState {
                is_linked_worktree: true,
                ..clean_main()
            },
            "worktree",
        ),
        (
            CheckoutState {
                branch: Some("feature".into()),
                ..clean_main()
            },
            "off-main",
        ),
        (
            CheckoutState {
                branch: None,
                ..clean_main()
            },
            "off-main",
        ),
        (
            CheckoutState {
                tracked_changes: vec!["src/lib.rs".into()],
                ..clean_main()
            },
            "dirty",
        ),
        // C1: an untracked file counts as dirty too.
        (
            CheckoutState {
                untracked: 1,
                ..clean_main()
            },
            "dirty",
        ),
    ];
    for (state, reason) in cases {
        let f = fixture();
        let mut fake = Fake::green();
        fake.probes = vec![state];
        assert_eq!(skip_reason(&tick(&f, &mut fake)), reason);
        assert!(!fake.called("fetch"), "{reason}: {:?}", fake.calls);
        assert_untouched(&fake);
    }
}

#[test]
fn fetch_failure_skips() {
    let f = fixture();
    let mut fake = Fake::green();
    fake.fetch = Err("could not resolve host".into());
    assert_eq!(skip_reason(&tick(&f, &mut fake)), "fetch-failed");
    assert!(!fake.called("ci"));
    assert_untouched(&fake);
}

// trace:STORY-1689.ac3e63c0 | ai:claude
#[test]
fn unchanged_skips_without_reading_ci_and_is_never_logged() {
    let f = fixture();
    let mut fake = Fake::green();
    fake.target = LIVE_SHA.into();
    let outcome = tick(&f, &mut fake);
    assert_eq!(outcome, TickOutcome::Skipped(Skip::Unchanged));
    assert!(!fake.called("ci"));
    assert_untouched(&fake);
    log_outcome(&f.paths.log, &outcome).unwrap();
    assert!(!f.paths.log.exists());
}

#[test]
fn a_failed_sha_is_not_rebuilt() {
    let f = fixture();
    put_state(&f, Phase::Failed, None, true, "2026-10-22T00:00:00Z");
    let mut fake = Fake::green();
    assert_eq!(skip_reason(&tick(&f, &mut fake)), "failed-sha");
    assert!(!fake.called("ci"));
    assert_untouched(&fake);
}

// trace:STORY-1689.ac3e63c0 | ai:claude
#[test]
fn non_fast_forward_skips() {
    let f = fixture();
    let mut fake = Fake::green();
    fake.ancestor = Ok(false);
    assert_eq!(skip_reason(&tick(&f, &mut fake)), "non-ff");
    assert!(!fake.called("ci"));
    assert_untouched(&fake);

    let f = fixture();
    let mut fake = Fake::green();
    fake.ff = Err("Not possible to fast-forward".into());
    assert_eq!(skip_reason(&tick(&f, &mut fake)), "non-ff");
    assert!(!fake.called("spawn"));
    assert!(read_state(&f.layout.live_dir).is_none());
}

// trace:STORY-1689.ac3e63c0 | ai:claude
#[test]
fn ci_that_is_not_green_skips() {
    let cases = [
        (
            Ok(CommitCi::Pending(vec!["CI (lib-0)".into()])),
            "ci-pending",
        ),
        (Ok(CommitCi::Failed(vec!["CI (lib-0)".into()])), "ci-red"),
        (Ok(CommitCi::NoChecks), "ci-unknown"),
        (
            Ok(CommitCi::MissingRequired("Build (ubuntu-latest)".into())),
            "ci-unknown",
        ),
        (Err("gh api failed: HTTP 502".to_string()), "ci-unknown"),
    ];
    for (ci, reason) in cases {
        let f = fixture();
        let mut fake = Fake::green();
        fake.ci = ci;
        assert_eq!(skip_reason(&tick(&f, &mut fake)), reason);
        assert!(!fake.called("wave"));
        assert_untouched(&fake);
    }
}

#[test]
fn a_live_wave_pinned_to_the_binary_skips_and_never_waits() {
    let f = fixture();
    let mut fake = Fake::green();
    fake.wave = Err("refusing to replace the binary pinned by live wave w1".into());
    assert_eq!(skip_reason(&tick(&f, &mut fake)), "wave-pinned");
    assert_untouched(&fake);
}

#[test]
fn a_checkout_dirtied_during_the_checks_is_not_fast_forwarded() {
    let f = fixture();
    let mut fake = Fake::green();
    fake.probes = vec![
        clean_main(),
        CheckoutState {
            untracked: 2,
            ..clean_main()
        },
    ];
    assert_eq!(skip_reason(&tick(&f, &mut fake)), "dirty");
    assert!(fake.called("ci"));
    assert_untouched(&fake);
}

// --- single flight --------------------------------------------------------

#[test]
fn a_held_lock_is_in_flight() {
    let f = fixture();
    let _held = try_lock(&f.layout.live_dir, Duration::ZERO)
        .unwrap()
        .unwrap();
    let mut fake = Fake::green();
    assert_eq!(skip_reason(&tick(&f, &mut fake)), "in-flight");
    assert!(fake.calls.is_empty(), "{:?}", fake.calls);
}

#[test]
fn a_running_build_with_a_live_pid_is_in_flight() {
    let f = fixture();
    put_state(
        &f,
        Phase::Running,
        Some(77),
        false,
        &chrono::Utc::now().to_rfc3339(),
    );
    let mut fake = Fake::green();
    assert_eq!(skip_reason(&tick(&f, &mut fake)), "in-flight");
    assert!(!fake.called("probe"));
    assert_untouched(&fake);
}

#[test]
fn an_unclaimed_unit_is_in_flight_only_within_its_grace() {
    let f = fixture();
    put_state(
        &f,
        Phase::Running,
        None,
        false,
        &chrono::Utc::now().to_rfc3339(),
    );
    let mut fake = Fake::green();
    assert_eq!(skip_reason(&tick(&f, &mut fake)), "in-flight");

    let f = fixture();
    let old = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    put_state(&f, Phase::Running, None, false, &old);
    let mut fake = Fake::green();
    assert!(matches!(tick(&f, &mut fake), TickOutcome::Failure { .. }));
}

// --- deferred failure report ----------------------------------------------

#[test]
fn a_failed_build_is_reported_once_with_its_log_tail() {
    let f = fixture();
    fs::create_dir_all(&f.paths.build_logs).unwrap();
    let mut log = String::new();
    for i in 0..60 {
        log.push_str(&format!("line {i}\n"));
    }
    log.push_str("error[E0425]: cannot find value `x`\n");
    fs::write(f.paths.build_logs.join("b.log"), log).unwrap();
    put_state(&f, Phase::Failed, None, false, "2026-10-22T00:00:00Z");

    let mut fake = Fake::green();
    let TickOutcome::Failure { sha, message } = tick(&f, &mut fake) else {
        panic!("expected the failure to be reported");
    };
    assert_eq!(sha, TARGET);
    assert!(
        fake.calls.is_empty(),
        "reporting runs no checks: {:?}",
        fake.calls
    );
    // C2: the routed line (the last one) carries the tail, not the head.
    let last = message.lines().last().unwrap();
    assert!(last.contains("error[E0425]"), "{last}");
    // The routed line names the commit and the full log path too.
    assert!(last.contains(TARGET), "{last}");
    assert!(
        last.contains(&f.paths.build_logs.join("b.log").display().to_string()),
        "{last}"
    );
    assert!(
        last.contains("line 59") && !last.contains("line 5 |"),
        "{last}"
    );
    assert!(message.contains("b.log"));
    assert!(read_state(&f.layout.live_dir).unwrap().reported);

    // Exactly once: the next tick does not report it again, and does not
    // rebuild the failed SHA either.
    let mut fake = Fake::green();
    assert_eq!(skip_reason(&tick(&f, &mut fake)), "failed-sha");
}

#[test]
fn a_build_whose_process_died_is_reported_as_failed() {
    let f = fixture();
    let recent = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
    put_state(&f, Phase::Running, Some(77), false, &recent);
    let mut fake = Fake::green();
    fake.alive = false;
    let TickOutcome::Failure { message, .. } = tick(&f, &mut fake) else {
        panic!("expected a failure report");
    };
    assert!(
        message.contains("ended without recording a result"),
        "{message}"
    );
    let state = read_state(&f.layout.live_dir).unwrap();
    assert_eq!((state.phase, state.reported), (Phase::Failed, true));
}

#[test]
fn a_running_record_past_the_build_time_limit_fails_even_if_its_pid_is_reused() {
    let f = fixture();
    // The pid is "alive" (an unrelated process reused it), but the record is
    // older than any build unit can run.
    let old = (chrono::Utc::now() - chrono::Duration::hours(3)).to_rfc3339();
    put_state(&f, Phase::Running, Some(77), false, &old);
    let mut fake = Fake::green();
    let TickOutcome::Failure { message, .. } = tick(&f, &mut fake) else {
        panic!("expected a failure report");
    };
    assert!(
        message.contains("outlived the build time limit"),
        "{message}"
    );
    assert!(!fake.called("pid-alive"), "{:?}", fake.calls);
    let state = read_state(&f.layout.live_dir).unwrap();
    assert_eq!((state.phase, state.reported), (Phase::Failed, true));
}

#[test]
fn a_launch_failure_is_reported_at_once() {
    let f = fixture();
    let mut fake = Fake::green();
    fake.spawn = Err("no such file".into());
    let TickOutcome::Failure { message, .. } = tick(&f, &mut fake) else {
        panic!("expected a failure report");
    };
    assert!(message.contains("could not be launched"), "{message}");
    assert!(read_state(&f.layout.live_dir).unwrap().reported);
}

// --- the log --------------------------------------------------------------

#[test]
fn a_repeated_skip_is_logged_once_and_a_new_reason_again() {
    let f = fixture();
    let pending = TickOutcome::Skipped(Skip::CiPending("CI on x is still running".into()));
    log_outcome(&f.paths.log, &pending).unwrap();
    log_outcome(&f.paths.log, &pending).unwrap();
    log_outcome(&f.paths.log, &TickOutcome::Skipped(Skip::Unchanged)).unwrap();
    let red = TickOutcome::Skipped(Skip::CiRed("CI on x failed".into()));
    log_outcome(&f.paths.log, &red).unwrap();
    log_outcome(
        &f.paths.log,
        &TickOutcome::Launched {
            sha: TARGET.into(),
            how: "unit u".into(),
            passed_over: Vec::new(),
        },
    )
    .unwrap();
    log_outcome(&f.paths.log, &pending).unwrap();
    let body = fs::read_to_string(&f.paths.log).unwrap();
    let kinds: Vec<&str> = body
        .lines()
        .map(|l| l.split_once(' ').unwrap().1)
        .map(|rest| rest.split_once(": ").map_or(rest, |(kind, _)| kind))
        .collect();
    assert_eq!(
        kinds,
        vec![
            "skip ci-pending",
            "skip ci-red",
            &format!("launch {TARGET}"),
            "skip ci-pending"
        ]
    );
}

#[test]
fn ci_skip_maps_only_green_to_build() {
    assert_eq!(ci_skip(TARGET, Ok(CommitCi::Green)), None);
    assert_eq!(
        ci_skip(TARGET, Err(anyhow::anyhow!("boom")))
            .unwrap()
            .reason(),
        "ci-unknown"
    );
}

// --- the child ------------------------------------------------------------

/// Fake build steps: a clean `main` at `head`, a build that writes both
/// binaries (or fails), a passing smoke test.
struct FakeBuild {
    head: String,
    fail_build: bool,
    /// The checkout gains a tracked change once the build has run.
    dirtied_by_build: bool,
    built: bool,
    /// Commits origin/main has that HEAD lacks, before / after the build
    /// (BUG-2687: origin/main moves while the build runs).
    behind: (usize, usize),
}

impl FakeBuild {
    fn at(head: &str, fail_build: bool) -> Self {
        Self {
            head: head.into(),
            fail_build,
            dirtied_by_build: false,
            built: false,
            behind: (0, 0),
        }
    }
}

impl BuildSteps for FakeBuild {
    fn probe(&mut self, _repo: &Path) -> Result<CheckoutState> {
        let mut state = CheckoutState {
            head: Some(self.head.clone()),
            ..clean_main()
        };
        if self.built && self.dirtied_by_build {
            state.tracked_changes = vec!["Cargo.lock".into()];
        }
        state.behind = if self.built {
            self.behind.1
        } else {
            self.behind.0
        };
        Ok(state)
    }
    fn guard(&mut self, _repo: &Path, _binary: &Path, _after_wave: bool) -> Result<()> {
        Ok(())
    }
    fn path_aida(&mut self) -> Option<PathBuf> {
        None
    }
    fn clean_full(&mut self, _layout: &DevLayout) -> Result<()> {
        Ok(())
    }
    fn build(&mut self, layout: &DevLayout) -> Result<()> {
        self.built = true;
        if self.fail_build {
            bail!("cargo build failed");
        }
        for name in crate::dev_build::LIVE_BINARIES {
            let built = layout.built(name);
            fs::create_dir_all(built.parent().unwrap()).unwrap();
            fs::write(built, format!("new-{name}")).unwrap();
        }
        Ok(())
    }
    fn smoke(&mut self, _repo: &Path, _aida: &Path, _server: &Path) -> Result<()> {
        Ok(())
    }
    fn version_sha(&mut self, _aida: &Path) -> Option<String> {
        Some(self.head[..12].to_string())
    }
    fn restart_mcp(&mut self, _layout: &DevLayout) -> Result<()> {
        Ok(())
    }
    fn restart_mail_wake(&mut self, _out: &mut dyn std::io::Write) -> Result<()> {
        Ok(())
    }
    fn rerecord_retention(&mut self, _layout: &DevLayout) -> RetentionRerecord {
        RetentionRerecord::NothingRecorded
    }
    fn confirm(&mut self, _prompt: &str) -> bool {
        false
    }
}

#[test]
fn the_child_records_promoted_through_the_unchanged_pipeline() {
    let f = fixture();
    put_state(
        &f,
        Phase::Running,
        None,
        false,
        &chrono::Utc::now().to_rfc3339(),
    );
    let mut build = FakeBuild::at(TARGET, false);
    run_child(&f.layout, TARGET, 99, &mut build, &mut Vec::new()).unwrap();
    let state = read_state(&f.layout.live_dir).unwrap();
    assert_eq!((state.phase, state.pid), (Phase::Promoted, Some(99)));
    let meta = read_promotion_meta(&f.layout.live_dir).unwrap();
    assert_eq!(meta.sha, TARGET);
    // The record names its checkout, so the next tick passes not-live-source.
    assert_eq!(meta.repo, Some(canonical(&f.layout.repo)));
    let mut fake = Fake::green();
    assert_eq!(skip_reason(&tick(&f, &mut fake)), "unchanged");
}

#[test]
fn checkout_build_lock_makes_busy_child_retryable() {
    let f = fixture();
    put_state(
        &f,
        Phase::Running,
        None,
        false,
        &chrono::Utc::now().to_rfc3339(),
    );
    let _lock = crate::dev_build::try_checkout_build_lock(&f.layout.repo)
        .unwrap()
        .unwrap();
    let mut build = FakeBuild::at(TARGET, false);
    let error = run_child(&f.layout, TARGET, 99, &mut build, &mut Vec::new()).unwrap_err();
    assert!(is_refusal(&error));
    assert_eq!(
        read_state(&f.layout.live_dir).unwrap().phase,
        Phase::Refused
    );
    assert_eq!(
        read_promotion_meta(&f.layout.live_dir).unwrap().sha,
        LIVE_SHA
    );
}

#[test]
fn the_child_records_a_failed_build_for_a_single_report() {
    let f = fixture();
    put_state(
        &f,
        Phase::Running,
        None,
        false,
        &chrono::Utc::now().to_rfc3339(),
    );
    let mut build = FakeBuild::at(TARGET, true);
    assert!(run_child(&f.layout, TARGET, 99, &mut build, &mut Vec::new()).is_err());
    let state = read_state(&f.layout.live_dir).unwrap();
    assert_eq!((state.phase, state.reported), (Phase::Failed, false));
    assert_eq!(
        read_promotion_meta(&f.layout.live_dir).unwrap().sha,
        LIVE_SHA
    );
    let mut fake = Fake::green();
    assert!(matches!(tick(&f, &mut fake), TickOutcome::Failure { .. }));
}

#[test]
fn the_child_refuses_a_moved_head_or_an_unlaunched_sha() {
    let f = fixture();
    put_state(
        &f,
        Phase::Running,
        None,
        false,
        &chrono::Utc::now().to_rfc3339(),
    );
    let mut build = FakeBuild::at(LIVE_SHA, false);
    let e = run_child(&f.layout, TARGET, 99, &mut build, &mut Vec::new()).unwrap_err();
    assert!(e.to_string().contains("HEAD moved"), "{e:#}");
    assert!(is_refusal(&e), "{e:#}");
    assert_eq!(
        read_state(&f.layout.live_dir).unwrap().phase,
        Phase::Refused
    );

    let f = fixture();
    let mut build = FakeBuild::at(TARGET, false);
    assert!(run_child(&f.layout, TARGET, 99, &mut build, &mut Vec::new()).is_err());
    assert_eq!(
        read_promotion_meta(&f.layout.live_dir).unwrap().sha,
        LIVE_SHA
    );
}

#[test]
fn the_child_argv_is_the_hidden_flag_and_parses() {
    let argv: Vec<String> = std::iter::once("aida".to_string())
        .chain(child_argv(TARGET))
        .collect();
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    let parsed = crate::TEST_CLI.try_parse_from(&args).unwrap();
    match parsed.command.registered::<crate::dev_cmd::DevArgs>().inner {
        crate::cli::DevCommand::Build {
            autopromote_child, ..
        } => assert_eq!(autopromote_child.as_deref(), Some(TARGET)),
        other => panic!("unexpected parse: {other:?}"),
    }
}

#[test]
fn if_main_green_refuses_the_manual_build_flags() {
    for flag in [
        "--allow-dirty",
        "--no-promote",
        "--install-links",
        "--clean",
        "--after-wave",
    ] {
        let r = crate::TEST_CLI.try_parse_from(&["aida", "dev", "build", "--if-main-green", flag]);
        assert!(r.is_err(), "{flag} must conflict with --if-main-green");
    }
    assert!(crate::TEST_CLI
        .try_parse_from(&["aida", "dev", "build", "--if-main-green"])
        .is_ok());
}

// --- review round 1 -------------------------------------------------------

// trace:STORY-1689.ac3e63c0 | ai:claude
#[test]
fn too_little_budget_left_skips_before_the_ci_read() {
    let f = fixture();
    let mut fake = Fake::green();
    let outcome = run_tick(
        &f.layout,
        &f.paths,
        &mut fake,
        Instant::now() + FF_RESERVE - Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(skip_reason(&outcome), "out-of-time");
    assert!(fake.called("fetch"), "the fetch fit: {:?}", fake.calls);
    assert!(!fake.called("ci"), "the CI read did not: {:?}", fake.calls);
    assert_untouched(&fake);
    assert!(read_state(&f.layout.live_dir).is_none());
}

#[test]
fn the_tick_budget_fits_inside_the_scheduler_kill() {
    let kill = crate::maintenance_schedule::SCHEDULED_COMMAND_TIMEOUT;
    assert!(TICK_BUDGET < kill, "{TICK_BUDGET:?} vs {kill:?}");
    // The fast-forward and launch, started with FF_RESERVE left, end
    // before the budget does.
    assert!(FF_TIMEOUT + crate::schedule_driver::WAVE_UNIT_RUN_TIMEOUT <= FF_RESERVE);
}

/// `[review_watch] required_checks` in a temp project, read through the
/// same reader review-watch readiness uses.
fn configured(toml: &str) -> Result<Vec<String>> {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".aida")).unwrap();
    fs::write(dir.path().join(".aida/config.toml"), toml).unwrap();
    crate::review_watch::required_checks(dir.path())
}

fn lane_only() -> Result<crate::forge::CommitChecks> {
    Ok(crate::forge::CommitChecks {
        sha: TARGET.into(),
        checks: vec![crate::forge::CommitCheck {
            name: "CI (lib-0)".into(),
            outcome: crate::forge::CheckOutcome::Passed,
            run_id: None,
            workflow_id: Some("ci".to_string()),
            cancelled: false,
            run_attempt: None,
        }],
    })
}

#[test]
fn the_configured_required_checks_gate_the_promote() {
    use crate::forge::ForgeKind;
    let aggregator = "[review_watch]\nrequired_checks = [\"Build (ubuntu-latest)\"]\n";
    assert_eq!(
        required_checks(ForgeKind::GitHub, configured(aggregator)).unwrap(),
        vec!["Build (ubuntu-latest)".to_string()]
    );
    // Every lane green but the configured aggregator absent: not green.
    assert!(matches!(
        judged_ci(ForgeKind::GitHub, configured(aggregator), lane_only()).unwrap(),
        CommitCi::MissingRequired(name) if name == "Build (ubuntu-latest)"
    ));
    // GitLab may require nothing beyond every check passing.
    assert_eq!(
        judged_ci(ForgeKind::GitLab, configured(""), lane_only()).unwrap(),
        CommitCi::Green
    );
}

#[test]
fn a_github_repo_with_no_configured_required_checks_skips_as_ci_unknown() {
    use crate::forge::ForgeKind;
    for toml in [
        "",
        "[review_watch]\nrequired_checks = []\n",
        "[review_watch]\nrequired_checks = 3\n",
    ] {
        let ci = judged_ci(ForgeKind::GitHub, configured(toml), lane_only());
        assert_eq!(
            ci_skip(TARGET, ci).map(|s| s.reason()),
            Some("ci-unknown"),
            "{toml:?}"
        );
    }
}

// trace:STORY-1689.ac873b7a | ai:claude
#[test]
fn the_repository_guide_documents_the_scheduled_promote() {
    let guide = include_str!("../../../docs/internal/agents/aida-repository-guide.md");
    let para = guide
        .lines()
        .find(|l| l.contains("**The live binary everyone runs is promoted by `aida dev build`**"))
        .expect("the dev-build paragraph");
    for needle in [
        "`dev-autopromote`",
        "`aida dev build --if-main-green`",
        "aida-autobuild.sh",
    ] {
        assert!(
            para.contains(needle),
            "the dev-build paragraph must name {needle}"
        );
    }
}

// --- review findings (TASK-2258, TASK-2259, TASK-2260) --------------------

// trace:STORY-1689 | ai:claude
#[test]
fn a_checkout_moved_during_the_build_is_a_refusal_not_a_failure() {
    let f = fixture();
    put_state(
        &f,
        Phase::Running,
        None,
        false,
        &chrono::Utc::now().to_rfc3339(),
    );
    let mut build = FakeBuild::at(TARGET, false);
    build.dirtied_by_build = true;
    let e = run_child(&f.layout, TARGET, 99, &mut build, &mut Vec::new()).unwrap_err();
    assert!(e.to_string().contains("checkout changed"), "{e:#}");
    assert!(is_refusal(&e), "{e:#}");
    assert_eq!(
        read_state(&f.layout.live_dir).unwrap().phase,
        Phase::Refused
    );
    assert_eq!(
        read_promotion_meta(&f.layout.live_dir).unwrap().sha,
        LIVE_SHA
    );
    // The next tick neither reports a failure nor parks the SHA as failed:
    // it decides afresh and relaunches.
    let mut fake = Fake::green();
    assert!(
        matches!(tick(&f, &mut fake), TickOutcome::Launched { .. }),
        "{:?}",
        fake.calls
    );
}

// trace:BUG-2628 | ai:codex
#[test]
fn concurrent_live_promotion_settles_the_child_or_retries_the_same_sha() {
    for concurrent_sha in [TARGET, LIVE_SHA] {
        let f = fixture();
        put_state(
            &f,
            Phase::Running,
            None,
            false,
            &chrono::Utc::now().to_rfc3339(),
        );
        let held = crate::dev_build::lock_live_dir(&f.layout).unwrap();
        let mut build = FakeBuild::at(TARGET, false);
        let mut output = Vec::new();
        let result = std::thread::scope(|scope| {
            let concurrent = scope.spawn(|| {
                std::thread::sleep(Duration::from_secs(1));
                set_live(&f, concurrent_sha, Some(canonical(&f.layout.repo)));
                drop(held);
            });
            let result = run_child(&f.layout, TARGET, 99, &mut build, &mut output);
            concurrent.join().unwrap();
            result
        });

        assert!(build.built);
        assert_eq!(
            read_promotion_meta(&f.layout.live_dir).unwrap().sha,
            concurrent_sha
        );
        let phase = read_state(&f.layout.live_dir).unwrap().phase;
        let mut tick_steps = Fake::green();
        if concurrent_sha == TARGET {
            assert_eq!(result.unwrap(), BuildOutcome::AlreadyPromoted);
            assert!(String::from_utf8(output)
                .unwrap()
                .contains("already promoted by a concurrent build"));
            assert_eq!(phase, Phase::Promoted);
            assert!(matches!(
                tick(&f, &mut tick_steps),
                TickOutcome::Skipped(Skip::Unchanged)
            ));
        } else {
            assert!(is_refusal(result.as_ref().unwrap_err()));
            child_exit(result.map(|_| ())).unwrap();
            assert_eq!(phase, Phase::Refused);
            assert!(matches!(
                tick(&f, &mut tick_steps),
                TickOutcome::Launched { sha, .. } if sha == TARGET
            ));
        }
    }
}

// trace:STORY-1689 | ai:claude
#[test]
fn a_real_build_failure_is_still_not_a_refusal() {
    let f = fixture();
    put_state(
        &f,
        Phase::Running,
        None,
        false,
        &chrono::Utc::now().to_rfc3339(),
    );
    let mut build = FakeBuild::at(TARGET, true);
    let e = run_child(&f.layout, TARGET, 99, &mut build, &mut Vec::new()).unwrap_err();
    assert!(!is_refusal(&e), "{e:#}");
}

// trace:STORY-1689 | ai:claude
#[test]
fn the_child_outwaits_the_launch_bound_for_the_lock() {
    assert!(
        CHILD_LOCK_WAIT > crate::schedule_driver::WAVE_UNIT_RUN_TIMEOUT,
        "the tick holds the lock across systemd-run ({:?}); the child waits {:?}",
        crate::schedule_driver::WAVE_UNIT_RUN_TIMEOUT,
        CHILD_LOCK_WAIT
    );
    // A lock released while the child waits is taken, not reported held.
    let f = fixture();
    put_state(
        &f,
        Phase::Running,
        None,
        false,
        &chrono::Utc::now().to_rfc3339(),
    );
    let held = try_lock(&f.layout.live_dir, Duration::ZERO)
        .unwrap()
        .unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(600));
        drop(held);
    });
    let mut build = FakeBuild::at(TARGET, false);
    run_child(&f.layout, TARGET, 99, &mut build, &mut Vec::new()).unwrap();
    release.join().unwrap();
    assert_eq!(
        read_state(&f.layout.live_dir).unwrap().phase,
        Phase::Promoted
    );
}

// trace:STORY-1689 | ai:claude
#[test]
fn an_untracked_file_skip_says_it_stops_the_scheduled_promote() {
    let state = CheckoutState {
        untracked: 7,
        ..clean_main()
    };
    let skip = checkout_skip(&state).unwrap();
    assert_eq!(skip.reason(), "dirty");
    let detail = skip.detail();
    assert!(detail.contains("7 untracked"), "{detail}");
    assert!(detail.contains("remove or ignore them"), "{detail}");
}

// --- review round 3 (C2, TASK-2289, TASK-2291) ---------------------------

// trace:STORY-1689 | ai:claude
#[test]
fn a_reported_failure_reaches_the_routed_cron_error_on_stderr() {
    let f = fixture();
    fs::create_dir_all(&f.paths.build_logs).unwrap();
    fs::write(
        f.paths.build_logs.join("b.log"),
        "compiling\nerror[E0425]: cannot find value `x`\n",
    )
    .unwrap();
    put_state(&f, Phase::Failed, None, false, "2026-10-22T00:00:00Z");
    let TickOutcome::Failure { message, .. } = tick(&f, &mut Fake::green()) else {
        panic!("expected the failure to be reported");
    };
    let mut stderr = Vec::new();
    let e = report_failure(&message, &mut stderr);
    // Nothing more is printed on stdout (agent mode under the scheduler).
    assert!(e.downcast_ref::<crate::TypedPayloadEmitted>().is_some());
    let stderr = String::from_utf8(stderr).unwrap();
    let routed = crate::maintenance_schedule::routed_failure_error(1, &stderr);
    assert!(routed.starts_with("exit 1: "), "{routed}");
    assert!(routed.contains(TARGET), "{routed}");
    assert!(
        routed.contains(&f.paths.build_logs.join("b.log").display().to_string()),
        "{routed}"
    );
    assert!(routed.contains("error[E0425]"), "{routed}");
}

// trace:STORY-1689 | ai:claude
#[test]
fn a_tick_with_no_budget_left_skips_before_the_fetch() {
    let f = fixture();
    let mut fake = Fake::green();
    let outcome = run_tick(&f.layout, &f.paths, &mut fake, Instant::now()).unwrap();
    assert_eq!(skip_reason(&outcome), "out-of-time");
    assert!(!fake.called("fetch"), "{:?}", fake.calls);
    assert!(read_state(&f.layout.live_dir).is_none());
}

// trace:STORY-1689 | ai:claude
#[test]
fn the_read_only_phase_fits_the_budget() {
    // Each step starts only with its own bound left, so the reads end by
    // the deadline, which itself ends before the scheduler's kill.
    assert!(FETCH_TIMEOUT + REV_PARSE_TIMEOUT * 2 + IS_ANCESTOR_TIMEOUT <= TICK_BUDGET);
    assert!(budget_skip(Instant::now() + TICK_BUDGET, CI_READ_RESERVE, "read CI").is_none());
    let skip = budget_skip(Instant::now(), CI_READ_RESERVE, "read CI").unwrap();
    assert_eq!(skip.reason(), "out-of-time");
    assert!(skip.detail().contains("read CI"), "{}", skip.detail());
}

// trace:STORY-1689 | ai:claude
#[test]
fn a_not_live_source_skip_is_never_logged() {
    let f = fixture();
    log_outcome(
        &f.paths.log,
        &TickOutcome::Skipped(Skip::NotLiveSource("another clone".into())),
    )
    .unwrap();
    assert!(!f.paths.log.exists());
}

// trace:STORY-1689 | ai:claude
#[test]
fn a_refused_child_exits_zero_and_a_failed_one_does_not() {
    let refused: Result<()> = Err(crate::dev_build::PromotionRefused("HEAD moved".into()).into());
    assert!(child_exit(refused).is_ok());
    assert!(child_exit(Err(anyhow::anyhow!("cargo build failed"))).is_err());
    assert!(child_exit(Ok(())).is_ok());
}

// TASK-2272 (TASK-2301): after `aida dev rollback` pins origin/main's SHA, a
// green tick skips it instead of re-promoting the commit rolled away from.
// trace:TASK-2272 | ai:claude
#[test]
fn a_tick_skips_the_sha_the_operator_rolled_back_from() {
    let f = fixture();
    assert_eq!(pin_rolled_back(&f.layout.live_dir, TARGET).unwrap(), None);
    let mut fake = Fake::green();
    let outcome = tick(&f, &mut fake);
    assert_eq!(skip_reason(&outcome), "failed-sha");
    let TickOutcome::Skipped(skip) = outcome else {
        unreachable!()
    };
    assert!(
        skip.detail().contains("rolled the live binary back"),
        "{}",
        skip.detail()
    );
    assert_untouched(&fake);
}

// TASK-2272 (TASK-2308): the pin never replaces a failure of another SHA that
// is not reported yet, and never writes while the autopromote lock is held.
// trace:TASK-2272 | ai:claude
#[test]
fn the_rollback_pin_keeps_an_unreported_failure_and_respects_the_lock() {
    let f = fixture();
    put_state(&f, Phase::Failed, None, false, "2026-10-22T00:00:00Z");
    let other = "3333333333333333333333333333333333333333";
    let why = pin_rolled_back(&f.layout.live_dir, other)
        .unwrap()
        .expect("kept");
    assert!(why.contains("not reported yet"), "{why}");
    let kept = read_state(&f.layout.live_dir).unwrap();
    assert_eq!((kept.sha.as_str(), kept.reported), (TARGET, false));

    let f = fixture();
    let _held = try_lock(&f.layout.live_dir, Duration::ZERO)
        .unwrap()
        .unwrap();
    let why = pin_rolled_back(&f.layout.live_dir, other)
        .unwrap()
        .expect("busy");
    assert!(why.contains("holds its lock"), "{why}");
    assert!(read_state(&f.layout.live_dir).is_none());
}

// --- BUG-2687: promote the newest green commit, not only a green tip --------

const MID: &str = "3333333333333333333333333333333333333333";
const OLDER: &str = "4444444444444444444444444444444444444444";
const OLDEST: &str = "5555555555555555555555555555555555555555";

fn pending() -> Result<CommitCi, String> {
    Ok(CommitCi::Pending(vec!["CI".into()]))
}

fn red() -> Result<CommitCi, String> {
    Ok(CommitCi::Failed(vec!["CI".into()]))
}

fn walk_fake(line: &[&str], ci: &[(&str, Result<CommitCi, String>)]) -> Fake {
    let mut fake = Fake::green();
    fake.line = line.iter().map(|s| s.to_string()).collect();
    fake.ci_by = ci.iter().map(|(s, c)| (s.to_string(), c.clone())).collect();
    fake
}

fn ci_reads(fake: &Fake) -> Vec<String> {
    fake.calls
        .iter()
        .filter_map(|c| c.strip_prefix("ci ").map(str::to_string))
        .collect()
}

// trace:BUG-2687 | ai:claude
#[test]
fn an_in_progress_tip_promotes_the_older_green_commit() {
    let f = fixture();
    let mut fake = walk_fake(&[TARGET, OLDER, LIVE_SHA], &[(TARGET, pending())]);
    fake.expect = OLDER.into();
    let TickOutcome::Launched {
        sha, passed_over, ..
    } = tick(&f, &mut fake)
    else {
        panic!("expected a launch: {:?}", fake.calls);
    };
    assert_eq!(sha, OLDER);
    assert_eq!(passed_over.len(), 1, "{passed_over:?}");
    assert!(
        passed_over[0].contains(&TARGET[..12]) && passed_over[0].contains("ci-pending"),
        "{passed_over:?}"
    );
    assert!(
        fake.called("ff") && fake.called("spawn"),
        "{:?}",
        fake.calls
    );
    assert_eq!(read_state(&f.layout.live_dir).unwrap().sha, OLDER);
    let scan = read_scan(&f.layout.live_dir).unwrap();
    assert_eq!(
        (scan.tip.as_str(), scan.newest_green.as_deref()),
        (TARGET, Some(OLDER))
    );
    // The report names the chosen SHA and the skipped newer one with why.
    let line = launch_line(&sha, "unit u", &passed_over);
    assert!(line.starts_with(&format!("launch {OLDER}")), "{line}");
    assert!(
        line.contains(&TARGET[..12]) && line.contains("still running"),
        "{line}"
    );
}

// trace:BUG-2687 | ai:claude
#[test]
fn a_red_tip_promotes_the_newest_green_commit_before_it() {
    let f = fixture();
    let mut fake = walk_fake(
        &[TARGET, MID, OLDER, OLDEST, LIVE_SHA],
        &[(TARGET, red()), (MID, pending())],
    );
    fake.expect = OLDER.into();
    let outcome = tick(&f, &mut fake);
    assert!(
        matches!(&outcome, TickOutcome::Launched { sha, passed_over, .. }
            if sha == OLDER && passed_over.len() == 2
                && passed_over[0].contains("ci-red")
                && passed_over[1].contains("ci-pending")),
        "{outcome:?}"
    );
    // The newest green wins: nothing older is read.
    assert_eq!(ci_reads(&fake), vec![TARGET, MID, OLDER]);
}

// trace:BUG-2687 | ai:claude
#[test]
fn no_green_commit_newer_than_live_skips() {
    let f = fixture();
    let mut fake = walk_fake(
        &[TARGET, MID, OLDER, LIVE_SHA, OLDEST],
        &[
            (TARGET, pending()),
            (MID, red()),
            // A commit whose CI run is missing is not green.
            (OLDER, Ok(CommitCi::NoChecks)),
        ],
    );
    let outcome = tick(&f, &mut fake);
    let TickOutcome::Skipped(skip) = &outcome else {
        panic!("expected a skip: {outcome:?}");
    };
    // The tip's verdict is the reason token; the detail names every commit.
    assert_eq!(skip.reason(), "ci-pending");
    let detail = skip.detail();
    for (sha, why) in [
        (TARGET, "still running"),
        (MID, "failed"),
        (OLDER, "no CI checks"),
    ] {
        assert!(
            detail.contains(&sha[..12]) && detail.contains(why),
            "{detail}"
        );
    }
    // The walk ends at the live commit: neither it nor anything older is read.
    assert_eq!(ci_reads(&fake), vec![TARGET, MID, OLDER]);
    assert_untouched(&fake);
    assert_eq!(read_scan(&f.layout.live_dir).unwrap().newest_green, None);
}

// trace:BUG-2687 | ai:claude
#[test]
fn red_and_unreadable_commits_are_never_promoted() {
    for ci in [
        red(),
        Err("gh api failed: HTTP 502".to_string()),
        Ok(CommitCi::MissingRequired("CI".into())),
    ] {
        let f = fixture();
        let mut fake = walk_fake(&[TARGET, LIVE_SHA], &[(TARGET, ci)]);
        assert!(matches!(tick(&f, &mut fake), TickOutcome::Skipped(_)));
        assert_untouched(&fake);
    }
}

// trace:BUG-2687 | ai:claude
#[test]
fn the_walk_stops_after_head_since_nothing_older_can_be_fast_forwarded_to() {
    let f = fixture();
    let mut fake = walk_fake(
        &[TARGET, MID, OLDER, LIVE_SHA],
        &[(TARGET, pending()), (MID, pending())],
    );
    fake.head = MID.into();
    assert_eq!(skip_reason(&tick(&f, &mut fake)), "ci-pending");
    assert_eq!(ci_reads(&fake), vec![TARGET, MID]);
    assert_untouched(&fake);

    // HEAD itself green: promoted with a no-op fast-forward.
    let f = fixture();
    let mut fake = walk_fake(&[TARGET, MID, LIVE_SHA], &[(TARGET, pending())]);
    fake.head = MID.into();
    fake.expect = MID.into();
    assert!(matches!(tick(&f, &mut fake), TickOutcome::Launched { sha, .. } if sha == MID));
}

// trace:BUG-2687 | ai:claude
#[test]
fn the_walk_stops_at_a_failed_or_rolled_back_commit() {
    let f = fixture();
    write_state(
        &f.layout.live_dir,
        &State {
            sha: MID.into(),
            phase: Phase::Failed,
            pid: None,
            log: String::new(),
            reported: true,
            at: "2026-11-19T00:00:00Z".into(),
            error: Some(ROLLED_BACK_ERROR.into()),
        },
    )
    .unwrap();
    let mut fake = walk_fake(&[TARGET, MID, OLDER, LIVE_SHA], &[(TARGET, pending())]);
    let outcome = tick(&f, &mut fake);
    let TickOutcome::Skipped(skip) = &outcome else {
        panic!("expected a skip: {outcome:?}");
    };
    assert!(
        skip.detail().contains(ROLLED_BACK_ERROR),
        "{}",
        skip.detail()
    );
    assert_eq!(ci_reads(&fake), vec![TARGET]);
    assert_untouched(&fake);

    // A newer green commit past the failed one is promoted.
    let f = fixture();
    put_state(&f, Phase::Failed, None, true, "2026-11-19T00:00:00Z");
    let mut fake = walk_fake(&[MID, TARGET, LIVE_SHA], &[]);
    fake.target = MID.into();
    fake.expect = MID.into();
    assert!(matches!(tick(&f, &mut fake), TickOutcome::Launched { sha, .. } if sha == MID));
}

// trace:BUG-2687 | ai:claude
#[test]
fn the_walk_is_bounded_to_twenty_commits() {
    let f = fixture();
    let line: Vec<String> = (0..30).map(|i| format!("{:040x}", 0xa000 + i)).collect();
    let mut fake = Fake::green();
    fake.target = line[0].clone();
    fake.line = line.clone();
    fake.ci = pending();
    let outcome = tick(&f, &mut fake);
    let TickOutcome::Skipped(skip) = &outcome else {
        panic!("expected a skip: {outcome:?}");
    };
    assert_eq!(ci_reads(&fake).len(), GREEN_SEARCH);
    assert!(
        skip.detail()
            .contains(&format!("none of the {GREEN_SEARCH}")),
        "{}",
        skip.detail()
    );
    assert_untouched(&fake);
}

// trace:BUG-2687 | ai:claude
#[test]
fn a_green_commit_that_does_not_contain_live_is_never_a_downgrade() {
    let f = fixture();
    let mut fake = walk_fake(&[TARGET, OLDER], &[(TARGET, pending())]);
    fake.not_ancestor = vec![(LIVE_SHA.into(), OLDER.into())];
    assert_eq!(skip_reason(&tick(&f, &mut fake)), "not-newer");
    assert_untouched(&fake);
}

// trace:BUG-2687 | ai:claude
#[test]
fn origin_main_moving_mid_check_still_promotes_the_chosen_green_commit() {
    // The tick picked OLDER and fast-forwarded to it; origin/main is 2
    // commits ahead at the start of the build and 5 by the swap.
    let f = fixture();
    put_state(
        &f,
        Phase::Running,
        None,
        false,
        &chrono::Utc::now().to_rfc3339(),
    );
    let mut state = read_state(&f.layout.live_dir).unwrap();
    state.sha = OLDER.into();
    write_state(&f.layout.live_dir, &state).unwrap();
    let mut build = FakeBuild::at(OLDER, false);
    build.behind = (2, 5);
    let outcome = run_child(&f.layout, OLDER, 99, &mut build, &mut Vec::new()).unwrap();
    assert_eq!(outcome, BuildOutcome::Promoted);
    assert_eq!(read_promotion_meta(&f.layout.live_dir).unwrap().sha, OLDER);
    assert_eq!(
        read_state(&f.layout.live_dir).unwrap().phase,
        Phase::Promoted
    );
}

// trace:BUG-2687 | ai:claude
#[test]
fn the_behind_guard_admits_only_the_chosen_green_commit_on_main() {
    use crate::dev_build::{plan_promotion, plan_promotion_at, PromotionPlan};
    let behind = CheckoutState {
        head: Some(OLDER.into()),
        behind: 3,
        ..clean_main()
    };
    assert!(matches!(
        plan_promotion_at(&behind, false, false, Some(OLDER)),
        PromotionPlan::Promote { warnings } if warnings.iter().any(|w| w.contains("newest green"))
    ));
    assert!(matches!(
        plan_promotion_at(&behind, false, false, Some(&OLDER.to_uppercase())),
        PromotionPlan::Promote { .. }
    ));
    // Without a verified green commit, behind still refuses.
    assert!(matches!(
        plan_promotion(&behind, false, false),
        PromotionPlan::Refuse { .. }
    ));
    let refused = [
        // HEAD is not the chosen commit.
        (behind.clone(), Some(MID)),
        // An abbreviated SHA is not the chosen commit.
        (behind.clone(), Some(&OLDER[..12])),
        (behind.clone(), Some("")),
        // HEAD has commits origin/main lacks: not an ancestor of it.
        (
            CheckoutState {
                ahead: 1,
                ..behind.clone()
            },
            Some(OLDER),
        ),
        // Never on `integration`, only `main`.
        (
            CheckoutState {
                branch: Some(crate::dev_build::INTEGRATION_BRANCH.into()),
                ..behind.clone()
            },
            Some(OLDER),
        ),
        // No origin/main to be an ancestor of.
        (
            CheckoutState {
                has_upstream: false,
                ..behind.clone()
            },
            Some(OLDER),
        ),
        // Dirty stays refused.
        (
            CheckoutState {
                tracked_changes: vec!["src/lib.rs".into()],
                ..behind.clone()
            },
            Some(OLDER),
        ),
    ];
    for (state, green) in refused {
        assert!(
            matches!(
                plan_promotion_at(&state, false, false, green),
                PromotionPlan::Refuse { .. }
            ),
            "{state:?} {green:?}"
        );
    }
}

// trace:BUG-2687 | ai:claude
#[test]
fn the_child_refuses_when_head_is_behind_but_not_the_chosen_commit() {
    let f = fixture();
    put_state(
        &f,
        Phase::Running,
        None,
        false,
        &chrono::Utc::now().to_rfc3339(),
    );
    // HEAD (TARGET) matches the launched SHA, but moved to MID during the
    // build while behind origin/main.
    let mut build = FakeBuild::at(TARGET, false);
    build.behind = (2, 2);
    struct Moving(FakeBuild);
    impl BuildSteps for Moving {
        fn probe(&mut self, repo: &Path) -> Result<CheckoutState> {
            let mut s = self.0.probe(repo)?;
            if self.0.built {
                s.head = Some(MID.into());
            }
            Ok(s)
        }
        fn guard(&mut self, r: &Path, b: &Path, w: bool) -> Result<()> {
            self.0.guard(r, b, w)
        }
        fn path_aida(&mut self) -> Option<PathBuf> {
            None
        }
        fn clean_full(&mut self, l: &DevLayout) -> Result<()> {
            self.0.clean_full(l)
        }
        fn build(&mut self, l: &DevLayout) -> Result<()> {
            self.0.build(l)
        }
        fn smoke(&mut self, r: &Path, a: &Path, s: &Path) -> Result<()> {
            self.0.smoke(r, a, s)
        }
        fn version_sha(&mut self, a: &Path) -> Option<String> {
            self.0.version_sha(a)
        }
        fn restart_mcp(&mut self, l: &DevLayout) -> Result<()> {
            self.0.restart_mcp(l)
        }
        fn restart_mail_wake(&mut self, o: &mut dyn std::io::Write) -> Result<()> {
            self.0.restart_mail_wake(o)
        }
        fn rerecord_retention(&mut self, l: &DevLayout) -> RetentionRerecord {
            self.0.rerecord_retention(l)
        }
        fn confirm(&mut self, p: &str) -> bool {
            self.0.confirm(p)
        }
    }
    let mut moving = Moving(build);
    let e = run_child(&f.layout, TARGET, 99, &mut moving, &mut Vec::new()).unwrap_err();
    assert!(is_refusal(&e), "{e:#}");
    assert_eq!(
        read_promotion_meta(&f.layout.live_dir).unwrap().sha,
        LIVE_SHA
    );
}

// trace:BUG-2687 | ai:claude
#[test]
fn dev_status_reports_the_distance_and_the_newest_green_commit() {
    use crate::dev_build::behind_main_line;
    assert_eq!(behind_main_line(None, None), None);
    assert!(behind_main_line(Some(0), None)
        .unwrap()
        .contains("live is at main"));
    let line = behind_main_line(Some(3), None).unwrap();
    assert!(
        line.contains("live is 3 commits behind main; newest green = unknown"),
        "{line}"
    );
    let scan = GreenScan {
        at: "2026-11-19T18:00:00Z".into(),
        tip: TARGET.into(),
        newest_green: Some(OLDER.into()),
        passed_over: vec![],
    };
    let line = behind_main_line(Some(1), Some(&scan)).unwrap();
    assert!(
        line.contains(&format!(
            "live is 1 commit behind main; newest green = {}",
            &OLDER[..12]
        )),
        "{line}"
    );
    assert!(line.contains(&TARGET[..12]), "{line}");
}

// trace:BUG-2687 | ai:claude
#[test]
fn the_cron_green_commit_must_be_a_full_sha() {
    use crate::dev_build::cron_green_commit;
    use std::ffi::OsStr;
    assert_eq!(cron_green_commit(None).unwrap(), None);
    assert_eq!(
        cron_green_commit(Some(OsStr::new(&OLDER.to_uppercase()))).unwrap(),
        Some(OLDER.to_string())
    );
    for bad in [
        "",
        &OLDER[..12],
        "HEAD",
        "origin/main",
        "zz44444444444444444444444444444444444444",
    ] {
        assert!(cron_green_commit(Some(OsStr::new(bad))).is_err(), "{bad}");
    }
}

/// Fake build steps whose HEAD moves to `to` (still level with origin/main)
/// after `stay_for` probes, or once the build has run; the staged build
/// reports whatever HEAD was when cargo ran (BUG-2687).
struct MovesTo {
    inner: FakeBuild,
    to: &'static str,
    stay_for: Option<usize>,
    probes: usize,
    built_at: Option<String>,
}

impl MovesTo {
    fn head(&self) -> String {
        let moved = match self.stay_for {
            Some(n) => self.probes > n,
            None => self.inner.built,
        };
        if moved {
            self.to.into()
        } else {
            self.inner.head.clone()
        }
    }
}

impl BuildSteps for MovesTo {
    fn probe(&mut self, repo: &Path) -> Result<CheckoutState> {
        self.probes += 1;
        let mut s = self.inner.probe(repo)?;
        s.head = Some(self.head());
        Ok(s)
    }
    fn guard(&mut self, r: &Path, b: &Path, w: bool) -> Result<()> {
        self.inner.guard(r, b, w)
    }
    fn path_aida(&mut self) -> Option<PathBuf> {
        None
    }
    fn clean_full(&mut self, l: &DevLayout) -> Result<()> {
        self.inner.clean_full(l)
    }
    fn build(&mut self, l: &DevLayout) -> Result<()> {
        if self.built_at.is_none() {
            self.built_at = Some(self.head());
        }
        self.inner.build(l)
    }
    fn smoke(&mut self, r: &Path, a: &Path, s: &Path) -> Result<()> {
        self.inner.smoke(r, a, s)
    }
    fn version_sha(&mut self, _a: &Path) -> Option<String> {
        self.built_at.as_deref().map(|h| h[..12].to_string())
    }
    fn restart_mcp(&mut self, l: &DevLayout) -> Result<()> {
        self.inner.restart_mcp(l)
    }
    fn restart_mail_wake(&mut self, o: &mut dyn std::io::Write) -> Result<()> {
        self.inner.restart_mail_wake(o)
    }
    fn rerecord_retention(&mut self, l: &DevLayout) -> RetentionRerecord {
        self.inner.rerecord_retention(l)
    }
    fn confirm(&mut self, p: &str) -> bool {
        self.inner.confirm(p)
    }
}

fn launched_older(f: &Fixture) {
    put_state(
        f,
        Phase::Running,
        None,
        false,
        &chrono::Utc::now().to_rfc3339(),
    );
    let mut state = read_state(&f.layout.live_dir).unwrap();
    state.sha = OLDER.into();
    write_state(&f.layout.live_dir, &state).unwrap();
}

// trace:BUG-2687 | ai:claude
#[test]
fn a_clean_head_level_with_origin_main_must_still_be_the_chosen_green_commit() {
    use crate::dev_build::{plan_promotion_at, PromotionPlan};
    // HEAD is TARGET == origin/main (0 ahead, 0 behind), but OLDER was chosen.
    let level = CheckoutState {
        head: Some(TARGET.into()),
        ..clean_main()
    };
    for allow_dirty in [false, true] {
        let PromotionPlan::Refuse { reasons } =
            plan_promotion_at(&level, false, allow_dirty, Some(OLDER))
        else {
            panic!("HEAD {TARGET} promoted for chosen {OLDER} (allow_dirty={allow_dirty})");
        };
        assert!(
            reasons
                .iter()
                .any(|r| r.contains(TARGET) && r.contains(OLDER)),
            "{reasons:?}"
        );
    }
    // HEAD exactly the chosen commit, level with origin/main, promotes.
    let chosen = CheckoutState {
        head: Some(OLDER.into()),
        ..clean_main()
    };
    assert!(matches!(
        plan_promotion_at(&chosen, false, false, Some(OLDER)),
        PromotionPlan::Promote { .. }
    ));
}

// trace:BUG-2687 | ai:claude
#[test]
fn head_fast_forwarded_before_the_build_refuses_without_building() {
    let f = fixture();
    launched_older(&f);
    // The child's own check and the build's first probe see OLDER; the
    // checkout is fast-forwarded to TARGET (== origin/main) before cargo runs.
    let mut steps = MovesTo {
        inner: FakeBuild::at(OLDER, false),
        to: TARGET,
        stay_for: Some(2),
        probes: 0,
        built_at: None,
    };
    let e = run_child(&f.layout, OLDER, 99, &mut steps, &mut Vec::new()).unwrap_err();
    assert!(is_refusal(&e), "{e:#}");
    let msg = format!("{e:#}");
    assert!(msg.contains(OLDER) && msg.contains(TARGET), "{msg}");
    assert!(
        steps.built_at.is_none(),
        "the build ran at {:?}",
        steps.built_at
    );
    assert_eq!(
        read_promotion_meta(&f.layout.live_dir).unwrap().sha,
        LIVE_SHA
    );
}

// trace:BUG-2687 | ai:claude
#[test]
fn head_fast_forwarded_during_the_build_refuses_a_matching_staged_tip() {
    let f = fixture();
    launched_older(&f);
    // Cargo stamps TARGET, and the final probe agrees on TARGET (level with
    // origin/main): the swap still refuses because TARGET was not chosen.
    let mut steps = MovesTo {
        inner: FakeBuild::at(OLDER, false),
        to: TARGET,
        stay_for: None,
        probes: 0,
        // Cargo started just after the move, so the staged build is TARGET.
        built_at: Some(TARGET.into()),
    };
    let e = run_child(&f.layout, OLDER, 99, &mut steps, &mut Vec::new()).unwrap_err();
    assert!(is_refusal(&e), "{e:#}");
    let msg = format!("{e:#}");
    assert!(msg.contains(OLDER) && msg.contains(TARGET), "{msg}");
    assert_eq!(
        read_promotion_meta(&f.layout.live_dir).unwrap().sha,
        LIVE_SHA
    );
    assert!(!f.layout.live_dir.join(".incoming").exists());
}
