//! STORY-1646: `aida dev build` / `aida dev rollback` pipeline tests.
//!
//! Every test runs against temp dirs (target, live, link) and fake
//! [`BuildSteps`]; nothing here runs a cargo build, touches the host's live
//! binaries, or signals MCP servers.
// trace:STORY-1646 | ai:claude

use super::*;
use std::fs;

struct Fixture {
    _tmp: tempfile::TempDir,
    layout: DevLayout,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let layout = DevLayout {
        repo: root.join("repo"),
        target_dir: root.join("repo").join("target"),
        live_dir: root.join("share").join("aida").join("live"),
        link_dir: root.join("bin"),
    };
    fs::create_dir_all(&layout.repo).unwrap();
    Fixture { _tmp: tmp, layout }
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

fn put_live(layout: &DevLayout, tag: &str) {
    for n in LIVE_BINARIES {
        write(&layout.live(n), &format!("{tag}-{n}"));
    }
}

fn clean_main() -> CheckoutState {
    CheckoutState {
        branch: Some("main".into()),
        has_origin_main: true,
        ..Default::default()
    }
}

/// Fake steps: `build` writes `<tag>-<name>` into target/agent; failures are
/// switchable; every call is recorded.
struct FakeSteps {
    states: Vec<CheckoutState>,
    build_tag: String,
    fail_build: bool,
    fail_smoke: bool,
    confirm_answer: bool,
    fail_guard: bool,
    /// What `aida` on PATH resolves to (None: not on PATH).
    path_aida: Option<PathBuf>,
    /// The `aida` path and its contents at each smoke test.
    smoked: Vec<(PathBuf, String)>,
    /// Runs inside the build, before the binaries are written.
    on_build: Option<Box<dyn FnMut()>>,
    /// Runs inside the smoke test (e.g. a concurrent build rewriting target/).
    on_smoke: Option<Box<dyn FnMut()>>,
    calls: Vec<String>,
}

impl FakeSteps {
    fn new(state: CheckoutState) -> Self {
        Self {
            states: vec![state],
            build_tag: "NEW".into(),
            fail_build: false,
            fail_smoke: false,
            confirm_answer: false,
            fail_guard: false,
            path_aida: None,
            smoked: Vec::new(),
            on_build: None,
            on_smoke: None,
            calls: Vec::new(),
        }
    }
    fn called(&self, step: &str) -> bool {
        self.calls.iter().any(|c| c == step)
    }
    fn guarded(&self, binary: &Path) -> bool {
        self.called(&format!("guard:{}", binary.display()))
    }
}

impl BuildSteps for FakeSteps {
    fn probe(&mut self, _repo: &Path) -> Result<CheckoutState> {
        self.calls.push("probe".into());
        let s = if self.states.len() > 1 {
            self.states.remove(0)
        } else {
            self.states[0].clone()
        };
        Ok(s)
    }
    fn guard(&mut self, _repo: &Path, binary: &Path, _after_wave: bool) -> Result<()> {
        self.calls.push(format!("guard:{}", binary.display()));
        if self.fail_guard {
            bail!("refusing to replace the binary pinned by live wave w-1 (pid 42); retry with --after-wave");
        }
        Ok(())
    }
    fn path_aida(&mut self) -> Option<PathBuf> {
        self.path_aida.clone()
    }
    fn clean_full(&mut self, _l: &DevLayout) -> Result<()> {
        self.calls.push("clean_full".into());
        Ok(())
    }
    fn build(&mut self, l: &DevLayout) -> Result<()> {
        self.calls.push("build".into());
        if let Some(hook) = self.on_build.as_mut() {
            hook();
        }
        if self.fail_build {
            bail!("error[E0425]: cannot find value");
        }
        for n in LIVE_BINARIES {
            write(&l.built(n), &format!("{}-{n}", self.build_tag));
        }
        Ok(())
    }
    fn smoke(&mut self, _repo: &Path, aida: &Path, _server: &Path) -> Result<()> {
        self.calls.push("smoke".into());
        self.smoked.push((
            aida.to_path_buf(),
            fs::read_to_string(aida).unwrap_or_default(),
        ));
        if let Some(hook) = self.on_smoke.as_mut() {
            hook();
        }
        if self.fail_smoke {
            bail!("`aida list --limit 1` exited non-zero");
        }
        Ok(())
    }
    fn restart_mcp(&mut self, _l: &DevLayout) -> Result<()> {
        self.calls.push("restart_mcp".into());
        Ok(())
    }
    fn confirm(&mut self, _prompt: &str) -> bool {
        self.calls.push("confirm".into());
        self.confirm_answer
    }
}

fn run(
    f: &Fixture,
    opts: &DevBuildOptions,
    steps: &mut FakeSteps,
) -> (Result<BuildOutcome>, String) {
    let mut out = Vec::new();
    let r = run_dev_build(&f.layout, opts, steps, &mut out);
    (r, String::from_utf8(out).unwrap())
}

// --- promotion gate (pure) ---------------------------------------------------

#[test]
fn clean_main_at_origin_promotes_without_warnings() {
    assert_eq!(
        plan_promotion(&clean_main(), false, false),
        PromotionPlan::Promote { warnings: vec![] }
    );
}

#[test]
fn untracked_only_warns_but_promotes() {
    let s = CheckoutState {
        untracked: 3,
        ..clean_main()
    };
    match plan_promotion(&s, false, false) {
        PromotionPlan::Promote { warnings } => {
            assert_eq!(warnings.len(), 1);
            assert!(warnings[0].contains("3 untracked"), "{warnings:?}");
        }
        other => panic!("expected promote, got {other:?}"),
    }
}

fn refusal(s: &CheckoutState) -> Vec<String> {
    match plan_promotion(s, false, false) {
        PromotionPlan::Refuse { reasons } => reasons,
        other => panic!("expected refusal for {s:?}, got {other:?}"),
    }
}

#[test]
fn every_refusal_case_refuses_and_allow_dirty_overrides_it() {
    let cases = [
        (
            CheckoutState {
                tracked_changes: vec!["aida-cli-lib/src/lib.rs".into()],
                ..clean_main()
            },
            "uncommitted changes: aida-cli-lib/src/lib.rs",
        ),
        (
            CheckoutState {
                branch: Some("feature".into()),
                ..clean_main()
            },
            "on branch `feature`",
        ),
        (
            CheckoutState {
                branch: None,
                ..clean_main()
            },
            "detached",
        ),
        (
            CheckoutState {
                behind: 2,
                ..clean_main()
            },
            "2 commit(s) behind origin/main",
        ),
        (
            CheckoutState {
                ahead: 1,
                behind: 4,
                ..clean_main()
            },
            "diverged from origin/main (1 ahead, 4 behind)",
        ),
        (
            CheckoutState {
                ahead: 1,
                ..clean_main()
            },
            "ahead of origin/main",
        ),
        (
            CheckoutState {
                has_origin_main: false,
                ..clean_main()
            },
            "no `origin/main`",
        ),
    ];
    for (state, needle) in cases {
        let reasons = refusal(&state);
        assert!(
            reasons.iter().any(|r| r.contains(needle)),
            "{needle:?} not in {reasons:?}"
        );
        match plan_promotion(&state, false, true) {
            PromotionPlan::Promote { warnings } => assert!(
                warnings
                    .iter()
                    .any(|w| w.contains("--allow-dirty") && w.contains(needle)),
                "{warnings:?}"
            ),
            other => panic!("--allow-dirty should promote {state:?}, got {other:?}"),
        }
    }
}

#[test]
fn worktree_never_promotes_even_with_allow_dirty() {
    let s = CheckoutState {
        is_linked_worktree: true,
        ..clean_main()
    };
    for allow in [false, true] {
        assert_eq!(
            plan_promotion(&s, false, allow),
            PromotionPlan::BuildOnly(BuildOnlyReason::Worktree)
        );
    }
    assert_eq!(
        plan_promotion(&clean_main(), true, true),
        PromotionPlan::BuildOnly(BuildOnlyReason::NoPromote)
    );
}

// --- probe against real temp git repos ----------------------------------------

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// origin (bare) + a primary clone on main with one pushed commit.
fn git_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    let clone = tmp.path().join("clone");
    fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "--bare", "-q", "-b", "main"]);
    fs::create_dir_all(&clone).unwrap();
    git(&clone, &["init", "-q", "-b", "main"]);
    git(
        &clone,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    write(&clone.join("a.txt"), "one");
    git(&clone, &["add", "a.txt"]);
    git(&clone, &["commit", "-q", "-m", "one"]);
    git(&clone, &["push", "-q", "origin", "main"]);
    (tmp, origin, clone)
}

fn probe_plan(repo: &Path) -> PromotionPlan {
    plan_promotion(&probe_checkout(repo).unwrap(), false, false)
}

#[test]
fn probe_clean_and_untracked_only_promote() {
    let (_t, _o, clone) = git_fixture();
    assert_eq!(
        probe_plan(&clone),
        PromotionPlan::Promote { warnings: vec![] }
    );
    write(&clone.join("scratch.py"), "x");
    match probe_plan(&clone) {
        PromotionPlan::Promote { warnings } => assert!(warnings[0].contains("1 untracked")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn probe_tracked_dirty_refuses() {
    let (_t, _o, clone) = git_fixture();
    write(&clone.join("a.txt"), "edited");
    assert!(
        matches!(probe_plan(&clone), PromotionPlan::Refuse { reasons } if reasons[0].contains("a.txt"))
    );
}

#[test]
fn probe_off_main_refuses() {
    let (_t, _o, clone) = git_fixture();
    git(&clone, &["checkout", "-q", "-b", "feature"]);
    assert!(
        matches!(probe_plan(&clone), PromotionPlan::Refuse { reasons } if reasons.iter().any(|r| r.contains("`feature`")))
    );
}

#[test]
fn probe_behind_and_diverged_refuse() {
    let (tmp, origin, clone) = git_fixture();
    // A second clone pushes a commit; fetching it leaves `clone` behind.
    let other = tmp.path().join("other");
    let out = Command::new("git")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .args([
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    write(&other.join("b.txt"), "two");
    git(&other, &["add", "b.txt"]);
    git(&other, &["commit", "-q", "-m", "two"]);
    git(&other, &["push", "-q", "origin", "main"]);
    git(&clone, &["fetch", "-q", "origin"]);
    assert!(
        matches!(probe_plan(&clone), PromotionPlan::Refuse { reasons } if reasons.iter().any(|r| r.contains("1 commit(s) behind")))
    );
    // A local commit on top makes it diverged.
    write(&clone.join("c.txt"), "three");
    git(&clone, &["add", "c.txt"]);
    git(&clone, &["commit", "-q", "-m", "three"]);
    assert!(
        matches!(probe_plan(&clone), PromotionPlan::Refuse { reasons } if reasons.iter().any(|r| r.contains("diverged")))
    );
}

#[test]
fn probe_linked_worktree_is_build_only() {
    let (tmp, _o, clone) = git_fixture();
    let wt = tmp.path().join("wt");
    git(
        &clone,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt-branch",
            wt.to_str().unwrap(),
        ],
    );
    let state = probe_checkout(&wt).unwrap();
    assert!(state.is_linked_worktree);
    assert!(!probe_checkout(&clone).unwrap().is_linked_worktree);
    assert_eq!(
        plan_promotion(&state, false, true),
        PromotionPlan::BuildOnly(BuildOnlyReason::Worktree)
    );
}

// --- pipeline -------------------------------------------------------------------

#[test]
fn failed_build_leaves_live_untouched_and_suggests_clean() {
    let f = fixture();
    put_live(&f.layout, "OLD");
    let mut steps = FakeSteps::new(clean_main());
    steps.fail_build = true;
    let (r, _) = run(&f, &DevBuildOptions::default(), &mut steps);
    let err = format!("{:#}", r.unwrap_err());
    assert!(
        err.contains("build failed") && err.contains("E0425"),
        "{err}"
    );
    assert!(err.contains("aida dev build --clean"), "{err}");
    assert!(!steps.called("smoke") && !steps.called("restart_mcp"));
    assert_eq!(read(&f.layout.live("aida")), "OLD-aida");
    assert!(!f.layout.prev_dir().exists());
}

#[test]
fn failed_smoke_test_leaves_live_untouched_and_suggests_clean() {
    let f = fixture();
    put_live(&f.layout, "OLD");
    let mut steps = FakeSteps::new(clean_main());
    steps.fail_smoke = true;
    let (r, _) = run(&f, &DevBuildOptions::default(), &mut steps);
    let err = format!("{:#}", r.unwrap_err());
    assert!(
        err.contains("smoke test") && err.contains("aida list --limit 1"),
        "{err}"
    );
    assert!(err.contains("aida dev build --clean"), "{err}");
    for n in LIVE_BINARIES {
        assert_eq!(read(&f.layout.live(n)), format!("OLD-{n}"));
    }
    assert!(!f.layout.prev_dir().exists());
}

#[test]
fn successful_build_promotes_keeps_prev_and_restarts_mcp() {
    let f = fixture();
    put_live(&f.layout, "OLD");
    let mut steps = FakeSteps::new(clean_main());
    let (r, out) = run(&f, &DevBuildOptions::default(), &mut steps);
    assert_eq!(r.unwrap(), BuildOutcome::Promoted);
    for n in LIVE_BINARIES {
        assert_eq!(read(&f.layout.live(n)), format!("NEW-{n}"));
        assert_eq!(read(&f.layout.prev(n)), format!("OLD-{n}"));
        // The build stays in target/ for incremental reuse.
        assert_eq!(read(&f.layout.built(n)), format!("NEW-{n}"));
    }
    assert!(steps.guarded(&f.layout.live("aida")) && steps.called("restart_mcp"));
    assert!(out.contains("Promoted"), "{out}");
    // No staging leftovers.
    let mut names: Vec<_> = fs::read_dir(&f.layout.live_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    // `.lock` is the persistent live-dir lock file.
    assert_eq!(names, [".lock", "aida", "aida-server", "prev"]);
}

#[test]
fn first_promotion_has_no_prev() {
    let f = fixture();
    let mut steps = FakeSteps::new(clean_main());
    run(&f, &DevBuildOptions::default(), &mut steps).0.unwrap();
    assert_eq!(read(&f.layout.live("aida")), "NEW-aida");
    assert!(!f.layout.prev_dir().exists());
}

#[test]
fn refusal_builds_nothing_and_leaves_live_untouched() {
    let f = fixture();
    put_live(&f.layout, "OLD");
    let mut steps = FakeSteps::new(CheckoutState {
        tracked_changes: vec!["x.rs".into()],
        ..clean_main()
    });
    let (r, _) = run(&f, &DevBuildOptions::default(), &mut steps);
    let err = format!("{:#}", r.unwrap_err());
    assert!(
        err.contains("refusing to promote") && err.contains("--allow-dirty"),
        "{err}"
    );
    assert!(!steps.called("build"));
    assert_eq!(read(&f.layout.live("aida")), "OLD-aida");
}

#[test]
fn allow_dirty_promotes_with_warning() {
    let f = fixture();
    let mut steps = FakeSteps::new(CheckoutState {
        tracked_changes: vec!["x.rs".into()],
        ..clean_main()
    });
    let opts = DevBuildOptions {
        allow_dirty: true,
        ..Default::default()
    };
    let (r, out) = run(&f, &opts, &mut steps);
    assert_eq!(r.unwrap(), BuildOutcome::Promoted);
    assert!(out.contains("warning: --allow-dirty"), "{out}");
    assert_eq!(read(&f.layout.live("aida")), "NEW-aida");
}

#[test]
fn checkout_change_during_build_blocks_promotion() {
    let f = fixture();
    put_live(&f.layout, "OLD");
    let mut steps = FakeSteps::new(clean_main());
    steps.states.push(CheckoutState {
        tracked_changes: vec!["late.rs".into()],
        ..clean_main()
    });
    let (r, _) = run(&f, &DevBuildOptions::default(), &mut steps);
    let err = format!("{:#}", r.unwrap_err());
    assert!(
        err.contains("changed during the build") && err.contains("late.rs"),
        "{err}"
    );
    assert_eq!(read(&f.layout.live("aida")), "OLD-aida");
}

#[test]
fn worktree_build_smoke_tests_but_never_promotes() {
    let f = fixture();
    put_live(&f.layout, "OLD");
    let mut steps = FakeSteps::new(CheckoutState {
        is_linked_worktree: true,
        branch: Some("story-x".into()),
        tracked_changes: vec!["wip.rs".into()],
        ..Default::default()
    });
    let (r, out) = run(&f, &DevBuildOptions::default(), &mut steps);
    assert_eq!(
        r.unwrap(),
        BuildOutcome::BuiltOnly(BuildOnlyReason::Worktree)
    );
    assert!(steps.called("smoke") && steps.guarded(&f.layout.built("aida")));
    assert!(!steps.guarded(&f.layout.live("aida")));
    // Build-only smoke-tests the build in place and never stages into or
    // locks the live dir.
    assert_eq!(steps.smoked[0].0, f.layout.built("aida"));
    assert!(!f.layout.live_dir.join(".incoming").exists());
    assert!(!f.layout.live_dir.join(".lock").exists());
    assert!(!steps.called("restart_mcp"));
    assert!(out.contains("aida dev activate"), "{out}");
    assert_eq!(read(&f.layout.live("aida")), "OLD-aida");
    assert!(!f.layout.prev_dir().exists());
}

#[test]
fn no_promote_builds_and_tests_only() {
    let f = fixture();
    put_live(&f.layout, "OLD");
    let mut steps = FakeSteps::new(clean_main());
    let opts = DevBuildOptions {
        no_promote: true,
        ..Default::default()
    };
    let (r, out) = run(&f, &opts, &mut steps);
    assert_eq!(
        r.unwrap(),
        BuildOutcome::BuiltOnly(BuildOnlyReason::NoPromote)
    );
    assert!(steps.called("smoke") && out.contains("--no-promote"));
    assert_eq!(read(&f.layout.live("aida")), "OLD-aida");
}

// --- --clean ------------------------------------------------------------------------

#[test]
fn clean_deletes_only_the_incremental_dir() {
    let f = fixture();
    let t = &f.layout.target_dir;
    write(&t.join("agent/incremental/aida_cli-1/s-abc/x.o"), "inc");
    write(&t.join("agent/deps/libserde.rlib"), "dep");
    write(&t.join("agent/build/x/out"), "build");
    write(&t.join("release/aida"), "rel");
    let mut steps = FakeSteps::new(CheckoutState {
        is_linked_worktree: true,
        ..clean_main()
    });
    // Clean ran before the build: the incremental dir is already gone when
    // the build starts.
    let inc = f.layout.incremental_dir();
    let seen_at_build = std::rc::Rc::new(std::cell::Cell::new(true));
    let seen = seen_at_build.clone();
    steps.on_build = Some(Box::new(move || seen.set(inc.exists())));
    let opts = DevBuildOptions {
        clean: Some(DevCleanMode::Incremental),
        ..Default::default()
    };
    let (r, out) = run(&f, &opts, &mut steps);
    r.unwrap();
    assert!(!t.join("agent/incremental").exists());
    assert!(t.join("agent/deps/libserde.rlib").exists());
    assert!(t.join("agent/build/x/out").exists());
    assert!(t.join("release/aida").exists());
    assert!(!steps.called("clean_full"));
    assert!(out.contains("deleted"), "{out}");
    assert!(steps.called("build"));
    assert!(
        !seen_at_build.get(),
        "incremental dir still existed when the build ran"
    );
}

#[test]
fn clean_full_runs_cargo_clean_then_the_normal_pipeline() {
    let f = fixture();
    let mut steps = FakeSteps::new(clean_main());
    let opts = DevBuildOptions {
        clean: Some(DevCleanMode::Full),
        ..Default::default()
    };
    assert_eq!(
        run(&f, &opts, &mut steps).0.unwrap(),
        BuildOutcome::Promoted
    );
    let pos = |s: &str| steps.calls.iter().position(|c| c == s).unwrap();
    assert!(pos("clean_full") < pos("build") && pos("build") < pos("smoke"));
}

#[test]
fn full_clean_command_is_scoped_to_the_agent_profile_and_target_dir() {
    let f = fixture();
    let cmd = full_clean_command(&f.layout);
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(cmd.get_program(), "cargo");
    assert_eq!(&args[..3], ["clean", "--profile", "agent"]);
    assert!(args.contains(&f.layout.target_dir.to_string_lossy().into_owned()));
}

/// Real `cargo clean --profile agent` against a throwaway crate with a temp
/// target dir: removes target/agent and nothing else. No compilation happens.
#[test]
fn full_clean_removes_exactly_the_agent_profile_dir() {
    if Command::new("cargo").arg("--version").output().is_err() {
        return;
    }
    let f = fixture();
    write(
        &f.layout.repo.join("Cargo.toml"),
        "[package]\nname = \"scratch\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [workspace]\n\n[profile.agent]\ninherits = \"release\"\n",
    );
    write(&f.layout.repo.join("src/lib.rs"), "");
    let t = &f.layout.target_dir;
    // Newer cargo refuses to clean a target dir without cargo's own tag file.
    write(
        &t.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    );
    write(&t.join("agent/incremental/x"), "inc");
    write(&t.join("agent/deps/y"), "dep");
    write(&t.join("release/aida"), "rel");
    write(&t.join("debug/aida"), "dbg");
    let out = full_clean_command(&f.layout)
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!t.join("agent").exists());
    assert!(t.join("release/aida").exists() && t.join("debug/aida").exists());
}

// --- rollback -----------------------------------------------------------------------

#[test]
fn rollback_swaps_prev_back_and_again_rolls_forward() {
    let f = fixture();
    put_live(&f.layout, "OLD");
    let mut steps = FakeSteps::new(clean_main());
    run(&f, &DevBuildOptions::default(), &mut steps).0.unwrap();
    assert!(rollback(&f.layout).unwrap().is_empty());
    for n in LIVE_BINARIES {
        assert_eq!(read(&f.layout.live(n)), format!("OLD-{n}"));
        assert_eq!(read(&f.layout.prev(n)), format!("NEW-{n}"));
    }
    rollback(&f.layout).unwrap();
    assert_eq!(read(&f.layout.live("aida")), "NEW-aida");
    assert!(!f.layout.live_dir.join(".rollback").exists());
}

#[test]
fn rollback_without_prev_errors_and_changes_nothing() {
    let f = fixture();
    put_live(&f.layout, "OLD");
    let err = format!("{:#}", rollback(&f.layout).unwrap_err());
    assert!(err.contains("no previous build"), "{err}");
    assert_eq!(read(&f.layout.live("aida")), "OLD-aida");
    assert!(!f.layout.prev_dir().exists());
    // Half a prev pair is also refused before anything moves.
    write(&f.layout.prev("aida"), "PREV-aida");
    assert!(rollback(&f.layout).is_err());
    assert_eq!(read(&f.layout.live("aida")), "OLD-aida");
    assert_eq!(read(&f.layout.prev("aida")), "PREV-aida");
}

// --- --install-links ----------------------------------------------------------------

#[test]
fn install_links_creates_missing_links() {
    let f = fixture();
    put_live(&f.layout, "LIVE");
    let mut steps = FakeSteps::new(clean_main());
    let mut out = Vec::new();
    install_links(&f.layout, false, &mut steps, &mut out).unwrap();
    for n in LIVE_BINARIES {
        assert_eq!(fs::read_link(f.layout.link(n)).unwrap(), f.layout.live(n));
    }
    assert!(!steps.called("confirm"));
    // Idempotent.
    install_links(&f.layout, false, &mut steps, &mut Vec::new()).unwrap();
    assert!(!steps.called("confirm"));
}

#[test]
fn install_links_shows_and_confirms_before_replacing() {
    let f = fixture();
    put_live(&f.layout, "LIVE");
    fs::create_dir_all(&f.layout.link_dir).unwrap();
    std::os::unix::fs::symlink("/old/target/agent/aida", f.layout.link("aida")).unwrap();
    write(&f.layout.link("aida-server"), "REAL-FILE");

    // Declined: nothing changes.
    let mut steps = FakeSteps::new(clean_main());
    let mut out = Vec::new();
    let err = install_links(&f.layout, false, &mut steps, &mut out).unwrap_err();
    let shown = String::from_utf8(out).unwrap();
    assert!(steps.called("confirm"));
    assert!(
        shown.contains("/old/target/agent/aida") && shown.contains("regular file"),
        "{shown}"
    );
    assert!(format!("{err:#}").contains("--yes"));
    assert_eq!(
        fs::read_link(f.layout.link("aida")).unwrap(),
        PathBuf::from("/old/target/agent/aida")
    );
    assert_eq!(read(&f.layout.link("aida-server")), "REAL-FILE");

    // Confirmed: replaced, and the real file is kept as a backup.
    steps.confirm_answer = true;
    install_links(&f.layout, false, &mut steps, &mut Vec::new()).unwrap();
    for n in LIVE_BINARIES {
        assert_eq!(fs::read_link(f.layout.link(n)).unwrap(), f.layout.live(n));
    }
    assert_eq!(
        read(&f.layout.link_dir.join("aida-server.pre-live")),
        "REAL-FILE"
    );
}

#[test]
fn install_links_yes_skips_the_prompt_and_directories_are_never_replaced() {
    let f = fixture();
    put_live(&f.layout, "LIVE");
    fs::create_dir_all(&f.layout.link_dir).unwrap();
    std::os::unix::fs::symlink("/stale", f.layout.link("aida")).unwrap();
    let mut steps = FakeSteps::new(clean_main());
    install_links(&f.layout, true, &mut steps, &mut Vec::new()).unwrap();
    assert!(!steps.called("confirm"));
    assert_eq!(
        fs::read_link(f.layout.link("aida")).unwrap(),
        f.layout.live("aida")
    );

    fs::remove_file(f.layout.link("aida-server")).unwrap();
    fs::create_dir_all(f.layout.link("aida-server")).unwrap();
    let err = install_links(&f.layout, true, &mut steps, &mut Vec::new()).unwrap_err();
    assert!(format!("{err:#}").contains("directory"));
}

#[test]
fn install_links_requires_a_live_build() {
    let f = fixture();
    let mut steps = FakeSteps::new(clean_main());
    let err = install_links(&f.layout, true, &mut steps, &mut Vec::new()).unwrap_err();
    assert!(format!("{err:#}").contains("nothing to link"));
    assert!(!f.layout.link_dir.exists());
}

#[test]
fn build_with_install_links_promotes_then_links() {
    let f = fixture();
    let mut steps = FakeSteps::new(clean_main());
    let opts = DevBuildOptions {
        install_links: true,
        ..Default::default()
    };
    run(&f, &opts, &mut steps).0.unwrap();
    assert_eq!(
        fs::read_link(f.layout.link("aida")).unwrap(),
        f.layout.live("aida")
    );
}

// --- status helpers -----------------------------------------------------------------

#[test]
fn banner_sha_token_keeps_dirty_suffix() {
    assert_eq!(
        banner_sha_token("aida 0.15.0 (built 2026-10-10 23:42:03 -07:00, sha 9116ba7f23+dirty)")
            .as_deref(),
        Some("9116ba7f23+dirty")
    );
    assert_eq!(
        banner_sha_token("aida 0.15.0 (built x, sha 36c906f453)").as_deref(),
        Some("36c906f453")
    );
    assert_eq!(banner_sha_token("aida-server 0.15.0"), None);
}

#[test]
fn live_status_notes_flag_differences_and_dirty_builds() {
    assert!(live_status_notes(None, None, Some("abc"))[0].contains("no live binary"));
    assert!(live_status_notes(Some("abc"), Some("def"), Some("abc")).is_empty());
    let n = live_status_notes(Some("abc+dirty"), None, Some("123"));
    assert!(
        n.iter().any(|s| s.contains("dirty checkout (abc+dirty)")),
        "{n:?}"
    );
    assert!(
        n.iter()
            .any(|s| s.contains("latest build (123) is not the live one")),
        "{n:?}"
    );
}

// --- CLI surface --------------------------------------------------------------------

#[test]
fn cli_parses_build_flags_and_rebuild_is_gone() {
    use clap::Parser;
    let parse = |args: &[&str]| crate::cli::Cli::try_parse_from(args);
    let clean_of = |args: &[&str]| match parse(args).unwrap().command {
        crate::cli::Command::Dev(crate::cli::DevCommand::Build { clean, .. }) => clean,
        other => panic!("unexpected parse: {other:?}"),
    };
    assert_eq!(clean_of(&["aida", "dev", "build"]), None);
    assert_eq!(
        clean_of(&["aida", "dev", "build", "--clean"]),
        Some(DevCleanMode::Incremental)
    );
    assert_eq!(
        clean_of(&["aida", "dev", "build", "--clean=full"]),
        Some(DevCleanMode::Full)
    );
    assert!(parse(&["aida", "dev", "rollback"]).is_ok());
    assert!(
        parse(&["aida", "dev", "build", "--yes"]).is_err(),
        "--yes needs --install-links"
    );
    assert!(parse(&["aida", "dev", "rebuild"]).is_err());
    assert!(!crate::dev_cmd::SHELL_HELPERS.contains("dev rebuild"));
}

// --- review round 1: live binary in target/, staged smoke, lock, recovery ---------

/// Make `<link_dir>/aida` a symlink to `<target>/agent/aida`, the pre-live
/// layout where the PATH binary is the build output itself.
fn link_into_target(layout: &DevLayout) {
    write(&layout.built("aida"), "CURRENT-aida");
    fs::create_dir_all(&layout.link_dir).unwrap();
    std::os::unix::fs::symlink(layout.built("aida"), layout.link("aida")).unwrap();
}

#[test]
fn clean_full_refuses_while_the_path_binary_lives_in_target() {
    let f = fixture();
    link_into_target(&f.layout);
    let mut steps = FakeSteps::new(clean_main());
    let opts = DevBuildOptions {
        clean: Some(DevCleanMode::Full),
        ..Default::default()
    };
    let (r, _) = run(&f, &opts, &mut steps);
    let err = format!("{:#}", r.unwrap_err());
    assert!(
        err.contains("refusing --clean=full") && err.contains("--install-links"),
        "{err}"
    );
    assert!(!steps.called("clean_full") && !steps.called("build"));
    assert_eq!(read(&f.layout.built("aida")), "CURRENT-aida");
}

#[test]
fn clean_full_refuses_when_path_aida_resolves_into_target() {
    let f = fixture();
    write(&f.layout.built("aida"), "CURRENT-aida");
    let mut steps = FakeSteps::new(clean_main());
    steps.path_aida = Some(f.layout.built("aida"));
    let opts = DevBuildOptions {
        clean: Some(DevCleanMode::Full),
        ..Default::default()
    };
    assert!(run(&f, &opts, &mut steps).0.is_err());
    assert!(!steps.called("clean_full"));
}

#[test]
fn plain_build_warns_while_the_path_binary_lives_in_target() {
    let f = fixture();
    link_into_target(&f.layout);
    let mut steps = FakeSteps::new(clean_main());
    let (r, out) = run(&f, &DevBuildOptions::default(), &mut steps);
    r.unwrap();
    assert!(
        out.contains("warning: the binary on your PATH lives in this build's target dir")
            && out.contains("--install-links"),
        "{out}"
    );
    // Incremental --clean never touches binaries, so it is allowed (with the warning).
    let mut steps = FakeSteps::new(clean_main());
    let opts = DevBuildOptions {
        clean: Some(DevCleanMode::Incremental),
        ..Default::default()
    };
    run(&f, &opts, &mut steps).0.unwrap();
}

#[test]
fn links_into_the_live_dir_do_not_warn() {
    let f = fixture();
    put_live(&f.layout, "OLD");
    fs::create_dir_all(&f.layout.link_dir).unwrap();
    std::os::unix::fs::symlink(f.layout.live("aida"), f.layout.link("aida")).unwrap();
    let mut steps = FakeSteps::new(clean_main());
    let (r, out) = run(&f, &DevBuildOptions::default(), &mut steps);
    r.unwrap();
    assert!(!out.contains("warning: the binary on your PATH"), "{out}");
}

#[test]
fn promote_smoke_tests_the_staged_copy_and_promotes_exactly_that() {
    let f = fixture();
    put_live(&f.layout, "OLD");
    let mut steps = FakeSteps::new(clean_main());
    // A concurrent build rewrites target/ while the smoke test runs.
    let layout = f.layout.clone();
    steps.on_smoke = Some(Box::new(move || {
        for n in LIVE_BINARIES {
            write(&layout.built(n), &format!("RACE-{n}"));
        }
    }));
    run(&f, &DevBuildOptions::default(), &mut steps).0.unwrap();
    let (path, body) = &steps.smoked[0];
    assert_eq!(path, &f.layout.live_dir.join(".incoming").join("aida"));
    assert_eq!(body, "NEW-aida");
    for n in LIVE_BINARIES {
        assert_eq!(read(&f.layout.live(n)), format!("NEW-{n}"));
    }
}

#[test]
fn staged_smoke_failure_leaves_no_staging_behind() {
    let f = fixture();
    put_live(&f.layout, "OLD");
    let mut steps = FakeSteps::new(clean_main());
    steps.fail_smoke = true;
    assert!(run(&f, &DevBuildOptions::default(), &mut steps).0.is_err());
    assert!(!f.layout.live_dir.join(".incoming").exists());
    assert_eq!(read(&f.layout.live("aida")), "OLD-aida");
}

#[test]
fn a_held_live_lock_blocks_promote_and_rollback() {
    let f = fixture();
    put_live(&f.layout, "OLD");
    write(&f.layout.prev("aida"), "PREV-aida");
    write(&f.layout.prev("aida-server"), "PREV-aida-server");
    let held = lock_live_dir(&f.layout).unwrap();

    let mut steps = FakeSteps::new(clean_main());
    let err = format!(
        "{:#}",
        run(&f, &DevBuildOptions::default(), &mut steps)
            .0
            .unwrap_err()
    );
    assert!(err.contains("another `aida dev build`"), "{err}");
    assert!(
        !steps.called("smoke"),
        "smoke must not run without the lock"
    );
    let err = format!("{:#}", rollback(&f.layout).unwrap_err());
    assert!(err.contains("another `aida dev build`"), "{err}");
    assert_eq!(read(&f.layout.live("aida")), "OLD-aida");
    assert_eq!(read(&f.layout.prev("aida")), "PREV-aida");

    drop(held);
    rollback(&f.layout).unwrap();
    assert_eq!(read(&f.layout.live("aida")), "PREV-aida");
}

#[test]
fn recovery_restores_prev_after_a_promote_crashed_mid_rotation() {
    let f = fixture();
    put_live(&f.layout, "LIVE");
    // Crash after `prev -> .prev-old`, before `.prev-new -> prev`.
    for n in LIVE_BINARIES {
        write(
            &f.layout.live_dir.join(".prev-old").join(n),
            &format!("PREV-{n}"),
        );
        write(
            &f.layout.live_dir.join(".prev-new").join(n),
            &format!("LIVE-{n}"),
        );
        write(&f.layout.live_dir.join(".incoming").join(n), "STAGED");
    }
    let notes = rollback(&f.layout).unwrap();
    assert!(
        notes.iter().any(|n| n.contains("interrupted promote")),
        "{notes:?}"
    );
    for n in LIVE_BINARIES {
        assert_eq!(read(&f.layout.live(n)), format!("PREV-{n}"));
        assert_eq!(read(&f.layout.prev(n)), format!("LIVE-{n}"));
    }
    for d in [".prev-old", ".prev-new", ".incoming", ".rollback"] {
        assert!(!f.layout.live_dir.join(d).exists(), "{d} left behind");
    }
}

#[test]
fn recovery_finishes_a_rollback_interrupted_mid_swap() {
    let f = fixture();
    // Rollback parked LIVE in .rollback, moved prev/aida live, then died
    // before moving the parked aida into prev/.
    write(&f.layout.live("aida"), "PREV-aida");
    write(&f.layout.live("aida-server"), "LIVE-aida-server");
    write(&f.layout.prev("aida-server"), "PREV-aida-server");
    let swap = f.layout.live_dir.join(".rollback");
    write(&swap.join("aida"), "LIVE-aida");
    write(&swap.join("aida-server"), "LIVE-aida-server");
    let _lock = lock_live_dir(&f.layout).unwrap();
    let notes = recover_live_dir(&f.layout).unwrap();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(read(&f.layout.prev("aida")), "LIVE-aida");
    assert_eq!(read(&f.layout.prev("aida-server")), "PREV-aida-server");
    assert!(!swap.exists());
}

#[test]
fn rollback_respects_the_live_wave_guard_unless_forced() {
    let f = fixture();
    put_live(&f.layout, "LIVE");
    write(&f.layout.prev("aida"), "PREV-aida");
    write(&f.layout.prev("aida-server"), "PREV-aida-server");
    let mut steps = FakeSteps::new(clean_main());
    steps.fail_guard = true;
    let mut out = Vec::new();
    let err = format!(
        "{:#}",
        run_dev_rollback(&f.layout, false, false, &mut steps, &mut out).unwrap_err()
    );
    assert!(
        err.contains("live wave") && err.contains("--force"),
        "{err}"
    );
    assert!(steps.guarded(&f.layout.live("aida")));
    assert_eq!(read(&f.layout.live("aida")), "LIVE-aida");

    let mut steps = FakeSteps::new(clean_main());
    steps.fail_guard = true;
    run_dev_rollback(&f.layout, false, true, &mut steps, &mut Vec::new()).unwrap();
    assert!(!steps.calls.iter().any(|c| c.starts_with("guard:")));
    assert!(steps.called("restart_mcp"));
    assert_eq!(read(&f.layout.live("aida")), "PREV-aida");
}

#[test]
fn an_existing_pre_live_backup_is_never_overwritten() {
    let f = fixture();
    put_live(&f.layout, "LIVE");
    write(&f.layout.link_dir.join("aida.pre-live"), "FIRST-BACKUP");
    write(&f.layout.link("aida"), "SECOND-REAL-FILE");
    let mut steps = FakeSteps::new(clean_main());
    install_links(&f.layout, true, &mut steps, &mut Vec::new()).unwrap();
    assert_eq!(
        read(&f.layout.link_dir.join("aida.pre-live")),
        "FIRST-BACKUP"
    );
    let second: Vec<_> = fs::read_dir(&f.layout.link_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("aida.pre-live."))
        .collect();
    assert_eq!(second.len(), 1, "{second:?}");
    assert_eq!(
        read(&f.layout.link_dir.join(&second[0])),
        "SECOND-REAL-FILE"
    );
}

#[test]
fn refusal_says_the_gate_does_not_fetch() {
    let msg = refusal_message(&["HEAD is 1 commit(s) behind origin/main".into()]);
    assert!(
        msg.contains("does not fetch") && msg.contains("git fetch"),
        "{msg}"
    );
}

#[test]
fn cli_parses_rollback_flags() {
    use clap::Parser;
    let parse = |args: &[&str]| crate::cli::Cli::try_parse_from(args);
    match parse(&["aida", "dev", "rollback", "--force"])
        .unwrap()
        .command
    {
        crate::cli::Command::Dev(crate::cli::DevCommand::Rollback { force, after_wave }) => {
            assert!(force && !after_wave)
        }
        other => panic!("unexpected parse: {other:?}"),
    }
    assert!(parse(&["aida", "dev", "rollback", "--after-wave"]).is_ok());
    assert!(parse(&["aida", "dev", "rollback", "--force", "--after-wave"]).is_err());
}
