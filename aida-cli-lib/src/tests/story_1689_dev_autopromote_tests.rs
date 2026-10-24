//! STORY-1689: `aida dev build --if-main-green`. Every test runs against
//! temp dirs and fake steps (a fake forge verdict, a fake git, a fake
//! spawn and fake build steps); nothing fetches, builds or launches.
// trace:STORY-1689 | ai:claude

use super::*;
use crate::dev_build::{PromotionMeta, PROMOTED_FILE};
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
    fs::create_dir_all(&layout.repo).unwrap();
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
        has_origin_main: true,
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
            LIVE_SHA.into()
        } else {
            self.target.clone()
        })
    }
    fn is_ancestor(&mut self, _repo: &Path, _a: &str, _d: &str) -> Result<bool> {
        self.calls.push("is-ancestor".into());
        err(&self.ancestor)
    }
    fn commit_ci(&mut self, _repo: &Path, sha: &str) -> Result<CommitCi> {
        assert_eq!(sha, self.target, "CI is read on origin/main's exact SHA");
        self.calls.push("ci".into());
        err(&self.ci)
    }
    fn wave_guard(&mut self, _repo: &Path, _binary: &Path) -> Result<()> {
        self.calls.push("wave".into());
        err(&self.wave)
    }
    fn ff_only(&mut self, _repo: &Path, sha: &str) -> Result<()> {
        assert_eq!(sha, self.target);
        self.calls.push("ff".into());
        err(&self.ff)
    }
    fn spawn_child(
        &mut self,
        _repo: &Path,
        sha: &str,
        _log: &Path,
    ) -> Result<(Option<u32>, String)> {
        assert_eq!(sha, self.target);
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
            how: "detached pid 4242".into()
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
}

impl FakeBuild {
    fn at(head: &str, fail_build: bool) -> Self {
        Self {
            head: head.into(),
            fail_build,
            dirtied_by_build: false,
            built: false,
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
    let parsed = crate::command_registry::try_parse_from(&args).unwrap();
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
        let r = crate::command_registry::try_parse_from(&[
            "aida",
            "dev",
            "build",
            "--if-main-green",
            flag,
        ]);
        assert!(r.is_err(), "{flag} must conflict with --if-main-green");
    }
    assert!(
        crate::command_registry::try_parse_from(&["aida", "dev", "build", "--if-main-green"])
            .is_ok()
    );
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
    let guide = include_str!("../../../docs/agents/aida-repository-guide.md");
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
