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

/// HEAD of the fake clean checkout; the fake staged banner reports its prefix.
const HEAD: &str = "abc1234567890def1234567890abc1234567890d";

fn clean_main() -> CheckoutState {
    CheckoutState {
        branch: Some("main".into()),
        has_origin_main: true,
        head: Some(HEAD.into()),
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
    /// What `version_sha` reports for the staged `aida`.
    staged_sha: Option<String>,
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
            staged_sha: Some("abc1234567+dirty".into()),
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
    fn version_sha(&mut self, _aida: &Path) -> Option<String> {
        self.calls.push("version_sha".into());
        self.staged_sha.clone()
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
    assert_eq!(names, [".lock", ".promoted", "aida", "aida-server", "prev"]);
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
    assert!(live_status_notes(None, None, None, Some("abc"))[0].contains("no live binary"));
    assert!(live_status_notes(Some("abc"), None, Some("def"), Some("abc")).is_empty());
    let n = live_status_notes(Some("abc+dirty"), None, None, Some("123"));
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

// --- promotion metadata (review fixes) ---------------------------------------------

fn status_notes(f: &Fixture, live_banner: &str) -> Vec<String> {
    let meta = read_promotion_meta(&f.layout.live_dir);
    live_status_notes(Some(live_banner), meta.as_ref(), None, None)
}

#[test]
fn clean_promote_with_untracked_files_shows_no_dirty_warning() {
    let f = fixture();
    let mut steps = FakeSteps::new(CheckoutState {
        untracked: 4,
        ..clean_main()
    });
    run(&f, &DevBuildOptions::default(), &mut steps).0.unwrap();
    let meta = read_promotion_meta(&f.layout.live_dir).expect("live/.promoted written");
    assert_eq!(meta.sha, HEAD);
    assert!(!meta.allow_dirty);
    // build.rs stamps +dirty for untracked files; the record overrides it.
    let notes = status_notes(&f, "abc1234567+dirty");
    assert!(notes.iter().all(|n| !n.contains("dirty")), "{notes:?}");
}

#[test]
fn allow_dirty_promote_records_it_and_status_warns() {
    let f = fixture();
    let mut steps = FakeSteps::new(CheckoutState {
        tracked_changes: vec!["x.rs".into()],
        ..clean_main()
    });
    let opts = DevBuildOptions {
        allow_dirty: true,
        ..Default::default()
    };
    run(&f, &opts, &mut steps).0.unwrap();
    assert!(read_promotion_meta(&f.layout.live_dir).unwrap().allow_dirty);
    let notes = status_notes(&f, "abc1234567+dirty");
    assert!(
        notes
            .iter()
            .any(|n| n.contains("--allow-dirty") && n.contains(HEAD)),
        "{notes:?}"
    );
    // --allow-dirty skips the staged-sha check.
    assert!(!steps.called("version_sha"));
}

#[test]
fn status_falls_back_to_the_banner_without_a_record() {
    let f = fixture();
    let notes = status_notes(&f, "abc1234567+dirty");
    assert!(
        notes.iter().any(|n| n.contains("dirty checkout")),
        "{notes:?}"
    );
}

#[test]
fn rollback_swaps_the_promotion_records() {
    let f = fixture();
    let opts = DevBuildOptions {
        allow_dirty: true,
        ..Default::default()
    };
    let mut steps = FakeSteps::new(CheckoutState {
        tracked_changes: vec!["x.rs".into()],
        ..clean_main()
    });
    run(&f, &opts, &mut steps).0.unwrap();
    let mut steps = FakeSteps::new(clean_main());
    run(&f, &DevBuildOptions::default(), &mut steps).0.unwrap();
    let prev_dir = f.layout.prev_dir();
    assert!(!read_promotion_meta(&f.layout.live_dir).unwrap().allow_dirty);
    assert!(read_promotion_meta(&prev_dir).unwrap().allow_dirty);

    rollback(&f.layout).unwrap();
    assert!(read_promotion_meta(&f.layout.live_dir).unwrap().allow_dirty);
    assert!(!read_promotion_meta(&prev_dir).unwrap().allow_dirty);
    rollback(&f.layout).unwrap();
    assert!(!read_promotion_meta(&f.layout.live_dir).unwrap().allow_dirty);
    assert!(read_promotion_meta(&prev_dir).unwrap().allow_dirty);
}

#[test]
fn rollback_to_a_pair_without_a_record_moves_the_live_record_to_prev() {
    let f = fixture();
    put_live(&f.layout, "OLD");
    let mut steps = FakeSteps::new(clean_main());
    run(&f, &DevBuildOptions::default(), &mut steps).0.unwrap();
    assert!(read_promotion_meta(&f.layout.prev_dir()).is_none());
    rollback(&f.layout).unwrap();
    assert!(read_promotion_meta(&f.layout.live_dir).is_none());
    assert_eq!(read_promotion_meta(&f.layout.prev_dir()).unwrap().sha, HEAD);
}

#[test]
fn staged_build_not_matching_head_is_not_promoted() {
    let f = fixture();
    put_live(&f.layout, "OLD");
    let mut steps = FakeSteps::new(clean_main());
    steps.staged_sha = Some("fff0000000".into());
    let (r, _) = run(&f, &DevBuildOptions::default(), &mut steps);
    let err = format!("{:#}", r.unwrap_err());
    assert!(
        err.contains("does not match HEAD") && err.contains("fff0000000") && err.contains(HEAD),
        "{err}"
    );
    assert_eq!(read(&f.layout.live("aida")), "OLD-aida");
    assert!(!f.layout.prev_dir().exists());
    assert!(!f.layout.live_dir.join(".incoming").exists());

    // An unreadable banner is refused too.
    steps.staged_sha = None;
    let err = format!(
        "{:#}",
        run(&f, &DevBuildOptions::default(), &mut steps)
            .0
            .unwrap_err()
    );
    assert!(err.contains("could not read a sha"), "{err}");
    assert_eq!(read(&f.layout.live("aida")), "OLD-aida");
}

#[test]
fn check_staged_sha_accepts_a_head_prefix_only() {
    assert!(check_staged_sha(Some("abc1234567"), HEAD).is_ok());
    assert!(check_staged_sha(Some("abc1234567+dirty"), HEAD).is_ok());
    assert!(check_staged_sha(Some("abc1234568"), HEAD).is_err());
    assert!(check_staged_sha(Some("abc"), HEAD).is_err());
    assert!(check_staged_sha(Some("abc1234567"), "").is_err());
}

#[test]
fn live_dir_inside_target_is_refused() {
    let f = fixture();
    let inside = DevLayout {
        live_dir: f.layout.target_dir.join("live"),
        ..f.layout.clone()
    };
    let err = format!("{:#}", inside.check_live_outside_target().unwrap_err());
    assert!(
        err.contains("inside cargo's target dir") && err.contains("AIDA_DEV_LIVE_DIR"),
        "{err}"
    );
    // Also through a symlink that resolves into target/.
    fs::create_dir_all(&f.layout.target_dir).unwrap();
    let alias = f.layout.repo.join("alias");
    std::os::unix::fs::symlink(&f.layout.target_dir, &alias).unwrap();
    let via_link = DevLayout {
        live_dir: alias.join("agent").join("live"),
        ..f.layout.clone()
    };
    assert!(via_link.check_live_outside_target().is_err());
    assert!(f.layout.check_live_outside_target().is_ok());
}

#[test]
fn install_links_refusal_after_promote_says_the_promote_succeeded() {
    let f = fixture();
    fs::create_dir_all(&f.layout.link_dir).unwrap();
    write(&f.layout.link("aida"), "REAL-FILE");
    let mut steps = FakeSteps::new(clean_main());
    let opts = DevBuildOptions {
        install_links: true,
        ..Default::default()
    };
    let err = format!("{:#}", run(&f, &opts, &mut steps).0.unwrap_err());
    assert!(
        err.contains("WAS promoted") && err.contains("only the PATH links were left alone"),
        "{err}"
    );
    assert_eq!(read(&f.layout.live("aida")), "NEW-aida");
    assert_eq!(read(&f.layout.link("aida")), "REAL-FILE");
}

// --- review follow-ups ------------------------------------------------------------
// trace:BUG-1959 | ai:claude

fn meta(sha: &str, allow_dirty: bool) -> PromotionMeta {
    PromotionMeta {
        sha: sha.into(),
        allow_dirty,
        promoted_at: "2026-10-12T00:00:00Z".into(),
    }
}

fn write_meta(dir: &Path, m: &PromotionMeta) {
    write(
        &dir.join(PROMOTED_FILE),
        &serde_json::to_string_pretty(m).unwrap(),
    );
}

#[test]
fn smoke_command_removes_the_git_env_vars() {
    let cmd = smoke_command(Path::new("/nonexistent/aida"));
    for var in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
        assert!(
            cmd.get_envs()
                .any(|(k, v)| k == std::ffi::OsStr::new(var) && v.is_none()),
            "{var} is not removed from the smoke command"
        );
    }
}

#[test]
fn status_shows_unknown_for_an_empty_promoted_sha() {
    let line = promoted_line(&meta("", true));
    assert_eq!(
        line,
        "Promoted:     HEAD unknown at 2026-10-12T00:00:00Z (--allow-dirty)"
    );
    assert_eq!(
        promoted_line(&meta(HEAD, false)),
        format!("Promoted:     HEAD {HEAD} at 2026-10-12T00:00:00Z")
    );
    let notes = live_status_notes(Some("abc1234567+dirty"), Some(&meta("", true)), None, None);
    assert!(
        notes.iter().any(|n| n.contains("HEAD unknown")),
        "{notes:?}"
    );
}

// --- rollback crash points ----------------------------------------------------------
// trace:BUG-1959 | ai:claude

/// Live pair A and prev pair B, each with its record when asked for.
/// Returns (A's record, B's record).
fn pairs_a_b(
    f: &Fixture,
    a_rec: bool,
    b_rec: bool,
) -> (Option<PromotionMeta>, Option<PromotionMeta>) {
    put_live(&f.layout, "A");
    for n in LIVE_BINARIES {
        write(&f.layout.prev(n), &format!("B-{n}"));
    }
    let a = a_rec.then(|| meta("aaaaaaaaaa", false));
    let b = b_rec.then(|| meta("bbbbbbbbbb", true));
    if let Some(a) = &a {
        write_meta(&f.layout.live_dir, a);
    }
    if let Some(b) = &b {
        write_meta(&f.layout.prev_dir(), b);
    }
    (a, b)
}

fn swap_dir(f: &Fixture) -> PathBuf {
    f.layout.live_dir.join(".rollback")
}

fn recover(f: &Fixture) {
    let _lock = lock_live_dir(&f.layout).unwrap();
    recover_live_dir(&f.layout).unwrap();
}

/// Shape `.rollback/` the way a finished parking step leaves it.
fn parked_state(f: &Fixture, a: &Option<PromotionMeta>, b: &Option<PromotionMeta>) -> PathBuf {
    let swap = swap_dir(f);
    for n in LIVE_BINARIES {
        write(&swap.join(n), &format!("A-{n}"));
    }
    let _ = fs::remove_file(f.layout.live_dir.join(PROMOTED_FILE));
    let _ = fs::remove_file(f.layout.prev_dir().join(PROMOTED_FILE));
    if let Some(a) = a {
        write(
            &swap.join(PARKED_LIVE_RECORD),
            &serde_json::to_string(a).unwrap(),
        );
    }
    if let Some(b) = b {
        write(
            &swap.join(PARKED_PREV_RECORD),
            &serde_json::to_string(b).unwrap(),
        );
    }
    write(&swap.join(ROLLBACK_MARKER), "");
    swap
}

/// Every binary is in exactly one slot and each record sits beside the pair
/// whose `aida` it describes (records follow `aida` in a mixed pair).
fn assert_consistent(f: &Fixture, a: &Option<PromotionMeta>, b: &Option<PromotionMeta>, ctx: &str) {
    let rec_for = |tag: &str| {
        if tag.starts_with("A-") {
            a.clone()
        } else {
            b.clone()
        }
    };
    let live_aida = read(&f.layout.live("aida"));
    let prev_aida = read(&f.layout.prev("aida"));
    assert_ne!(live_aida[..2], prev_aida[..2], "{ctx}");
    for n in LIVE_BINARIES {
        let mut both = [read(&f.layout.live(n)), read(&f.layout.prev(n))];
        both.sort();
        assert_eq!(both, [format!("A-{n}"), format!("B-{n}")], "{ctx}");
    }
    assert_eq!(
        read_promotion_meta(&f.layout.live_dir),
        rec_for(&live_aida),
        "{ctx}: live record"
    );
    assert_eq!(
        read_promotion_meta(&f.layout.prev_dir()),
        rec_for(&prev_aida),
        "{ctx}: prev record"
    );
    assert!(!swap_dir(f).exists(), "{ctx}: .rollback/ left behind");
}

#[test]
fn a_rollback_crashing_after_any_step_recovers_consistent_records() {
    // Count the steps of a clean run, then crash after each one in turn,
    // for every combination of records present.
    let f = fixture();
    pairs_a_b(&f, true, true);
    let mut total = 0;
    rollback_with_steps(&f.layout, &mut |_| {
        total += 1;
        Ok(())
    })
    .unwrap();
    assert!(total >= 13, "{total}");
    for (a_rec, b_rec) in [(true, true), (true, false), (false, true), (false, false)] {
        for crash_at in 1..=total {
            let f = fixture();
            let (a, b) = pairs_a_b(&f, a_rec, b_rec);
            let mut seen = 0;
            let mut label = "";
            let r = rollback_with_steps(&f.layout, &mut |s| {
                seen += 1;
                if seen == crash_at {
                    label = s;
                    bail!("simulated crash")
                }
                Ok(())
            });
            let ctx = format!("records ({a_rec}, {b_rec}), crash after step {crash_at} ({label})");
            assert!(r.is_err(), "{ctx}");
            recover(&f);
            assert_consistent(&f, &a, &b, &ctx);
            // A second recovery is a no-op.
            fs::create_dir_all(swap_dir(&f)).unwrap();
            recover(&f);
            assert_consistent(&f, &a, &b, &format!("{ctx}, recovered twice"));
        }
    }
}

#[test]
fn rollback_parks_both_records_before_the_marker() {
    let f = fixture();
    pairs_a_b(&f, true, true);
    let swap = swap_dir(&f);
    let mut checked = false;
    rollback_with_steps(&f.layout, &mut |s| {
        if s == "marked" {
            assert!(
                swap.join(ROLLBACK_MARKER).is_file(),
                "no marker at the marker step"
            );
            assert!(swap.join(PARKED_LIVE_RECORD).is_file());
            assert!(swap.join(PARKED_PREV_RECORD).is_file());
            assert!(!f.layout.live_dir.join(PROMOTED_FILE).exists());
            assert!(!f.layout.prev_dir().join(PROMOTED_FILE).exists());
            for n in LIVE_BINARIES {
                assert!(swap.join(n).is_file());
                assert!(f.layout.prev(n).is_file(), "no binary moved yet");
            }
            checked = true;
        } else if s == "parked-prev-record" {
            assert!(
                !swap.join(ROLLBACK_MARKER).exists(),
                "marker before parking finished"
            );
        }
        Ok(())
    })
    .unwrap();
    assert!(checked);
    assert!(!swap.exists());
}

#[test]
fn a_leftover_marker_after_a_finished_rollback_moves_no_record() {
    // Path 1: live A with a record, prev B without; rollback finished but
    // the marker was left behind.
    let f = fixture();
    let (a, b) = pairs_a_b(&f, true, false);
    rollback(&f.layout).unwrap();
    write(&swap_dir(&f).join(ROLLBACK_MARKER), "");
    recover(&f);
    assert_eq!(read(&f.layout.live("aida")), "B-aida");
    assert_eq!(read_promotion_meta(&f.layout.live_dir), b);
    assert_eq!(read_promotion_meta(&f.layout.prev_dir()), a);
    assert_consistent(&f, &a, &b, "leftover marker");
}

#[test]
fn a_partly_deleted_rollback_dir_with_the_marker_moves_no_record() {
    // Path 2: marker down, nothing swapped, records already settled back,
    // then a recovery died mid-delete after removing parked `aida`.
    let f = fixture();
    let (a, b) = pairs_a_b(&f, false, true);
    let swap = swap_dir(&f);
    write(&swap.join("aida-server"), "A-aida-server");
    write(&swap.join(ROLLBACK_MARKER), "");
    recover(&f);
    assert_eq!(read(&f.layout.live("aida")), "A-aida");
    assert_eq!(read_promotion_meta(&f.layout.live_dir), None);
    assert_eq!(read_promotion_meta(&f.layout.prev_dir()), b.clone());
    assert_consistent(&f, &a, &b, "partly deleted .rollback/");
}

#[test]
fn recovery_after_a_crash_while_parking_leaves_the_records_alone() {
    // `.rollback/` holding only `aida`, and empty (crash right after
    // fresh_dir): parking never finished, so nothing was swapped.
    for parked in [&["aida"][..], &[][..]] {
        let f = fixture();
        let (a, b) = pairs_a_b(&f, false, true);
        let swap = swap_dir(&f);
        fs::create_dir_all(&swap).unwrap();
        for n in parked {
            write(&swap.join(n), &format!("A-{n}"));
        }
        recover(&f);
        assert_eq!(read_promotion_meta(&f.layout.live_dir), None, "{parked:?}");
        assert_eq!(
            read_promotion_meta(&f.layout.prev_dir()),
            b.clone(),
            "{parked:?}"
        );
        assert_eq!(read(&f.layout.live("aida")), "A-aida");
        assert_consistent(&f, &a, &b, &format!("{parked:?}"));
    }
}

#[test]
fn recovery_after_parking_but_before_any_swap_restores_both_records() {
    let f = fixture();
    let (a, b) = pairs_a_b(&f, true, true);
    parked_state(&f, &a, &b);
    recover(&f);
    assert_eq!(read(&f.layout.live("aida")), "A-aida");
    assert_eq!(read_promotion_meta(&f.layout.live_dir), a.clone());
    assert_eq!(read_promotion_meta(&f.layout.prev_dir()), b.clone());
    assert_consistent(&f, &a, &b, "parked, nothing swapped");
}

#[test]
fn recovery_between_the_two_binary_swaps_moves_the_records_with_aida() {
    // `aida` fully swapped (A parked into prev/), `aida-server` not started.
    let f = fixture();
    let (a, b) = pairs_a_b(&f, true, true);
    let swap = parked_state(&f, &a, &b);
    fs::rename(f.layout.prev("aida"), f.layout.live("aida")).unwrap();
    fs::rename(swap.join("aida"), f.layout.prev("aida")).unwrap();
    recover(&f);
    assert_eq!(read(&f.layout.live("aida")), "B-aida");
    assert_eq!(read(&f.layout.live("aida-server")), "A-aida-server");
    assert_eq!(read_promotion_meta(&f.layout.live_dir), b.clone());
    assert_eq!(read_promotion_meta(&f.layout.prev_dir()), a.clone());
    assert_consistent(&f, &a, &b, "between binary swaps");
}

#[test]
fn recovery_finishes_the_record_swap_after_both_binaries_were_swapped() {
    let f = fixture();
    let (a, b) = pairs_a_b(&f, true, true);
    let swap = parked_state(&f, &a, &b);
    for n in LIVE_BINARIES {
        fs::rename(f.layout.prev(n), f.layout.live(n)).unwrap();
        fs::rename(swap.join(n), f.layout.prev(n)).unwrap();
    }
    recover(&f);
    assert_eq!(read_promotion_meta(&f.layout.live_dir), b.clone());
    assert_eq!(read_promotion_meta(&f.layout.prev_dir()), a.clone());
    assert_consistent(&f, &a, &b, "both binaries swapped");
}

#[test]
fn recovery_mid_record_swap_keeps_the_parked_record() {
    // Both binaries swapped; B's record already went live, A's is still
    // parked.
    let f = fixture();
    let (a, b) = pairs_a_b(&f, true, true);
    let swap = parked_state(&f, &a, &b);
    for n in LIVE_BINARIES {
        fs::rename(f.layout.prev(n), f.layout.live(n)).unwrap();
        fs::rename(swap.join(n), f.layout.prev(n)).unwrap();
    }
    fs::rename(
        swap.join(PARKED_PREV_RECORD),
        f.layout.live_dir.join(PROMOTED_FILE),
    )
    .unwrap();
    recover(&f);
    assert_eq!(read_promotion_meta(&f.layout.live_dir), b.clone());
    assert_eq!(read_promotion_meta(&f.layout.prev_dir()), a.clone());
    assert_consistent(&f, &a, &b, "mid record swap");
}

#[test]
fn a_completed_rollback_leaves_no_marker_or_swap_dir() {
    let f = fixture();
    let (a, b) = pairs_a_b(&f, true, false);
    rollback(&f.layout).unwrap();
    assert_eq!(read(&f.layout.live("aida")), "B-aida");
    assert_consistent(&f, &a, &b, "completed rollback");
}

// trace:BUG-1999 | ai:antigravity
#[test]
fn recovery_with_old_format_promoted_before_binary_swap_restores_records() {
    // Old binary (STORY-1646) parked binaries and moved live .promoted to
    // .rollback/.promoted, then crashed before any binary swaps began.
    let f = fixture();
    let (a, b) = pairs_a_b(&f, true, true);
    let swap = swap_dir(&f);
    fs::create_dir_all(&swap).unwrap();
    for n in LIVE_BINARIES {
        write(&swap.join(n), &format!("A-{n}"));
    }
    fs::remove_file(f.layout.live_dir.join(PROMOTED_FILE)).unwrap();
    write(
        &swap.join(PROMOTED_FILE),
        &serde_json::to_string(&a).unwrap(),
    );
    // Notice: no ROLLBACK_MARKER, no PARKED_LIVE_RECORD, no PARKED_PREV_RECORD.
    // prev/.promoted still exists in prev_dir.
    recover(&f);
    assert_eq!(read(&f.layout.live("aida")), "A-aida");
    assert_eq!(read_promotion_meta(&f.layout.live_dir), a.clone());
    assert_eq!(read_promotion_meta(&f.layout.prev_dir()), b.clone());
    assert_consistent(&f, &a, &b, "old format before binary swap");
}

// trace:BUG-1999 | ai:antigravity
#[test]
fn recovery_with_old_format_promoted_after_binary_swap_swaps_records() {
    // Old binary swapped both binaries, then crashed before moving records.
    let f = fixture();
    let (a, b) = pairs_a_b(&f, true, true);
    let swap = swap_dir(&f);
    fs::create_dir_all(&swap).unwrap();
    for n in LIVE_BINARIES {
        write(&f.layout.live(n), &format!("B-{n}"));
        write(&f.layout.prev(n), &format!("A-{n}"));
    }
    fs::remove_file(f.layout.live_dir.join(PROMOTED_FILE)).unwrap();
    write(
        &swap.join(PROMOTED_FILE),
        &serde_json::to_string(&a).unwrap(),
    );
    recover(&f);
    assert_eq!(read(&f.layout.live("aida")), "B-aida");
    assert_eq!(read_promotion_meta(&f.layout.live_dir), b.clone());
    assert_eq!(read_promotion_meta(&f.layout.prev_dir()), a.clone());
    assert_consistent(&f, &a, &b, "old format after binary swap");
}

// trace:BUG-1999 | ai:antigravity
#[test]
fn recovery_with_old_format_promoted_when_live_already_has_record() {
    // Old binary swapped both binaries and moved prev/.promoted into live/.promoted,
    // then crashed before moving .rollback/.promoted into prev/.promoted.
    let f = fixture();
    let (a, b) = pairs_a_b(&f, true, true);
    let swap = swap_dir(&f);
    fs::create_dir_all(&swap).unwrap();
    for n in LIVE_BINARIES {
        write(&f.layout.live(n), &format!("B-{n}"));
        write(&f.layout.prev(n), &format!("A-{n}"));
    }
    // live has B's record already
    write_meta(&f.layout.live_dir, b.as_ref().unwrap());
    fs::remove_file(f.layout.prev_dir().join(PROMOTED_FILE)).unwrap();
    // swap has old live record (.promoted)
    write(
        &swap.join(PROMOTED_FILE),
        &serde_json::to_string(&a).unwrap(),
    );
    recover(&f);
    assert_eq!(read(&f.layout.live("aida")), "B-aida");
    assert_eq!(read_promotion_meta(&f.layout.live_dir), b.clone());
    assert_eq!(read_promotion_meta(&f.layout.prev_dir()), a.clone());
    assert_consistent(&f, &a, &b, "old format when live already has record");
}

// trace:BUG-1999 | ai:antigravity
#[test]
fn recovery_with_old_format_promoted_without_prev_record() {
    // Live A has a record, prev B has no record.
    let f = fixture();
    let (a, b) = pairs_a_b(&f, true, false);
    let swap = swap_dir(&f);
    fs::create_dir_all(&swap).unwrap();
    for n in LIVE_BINARIES {
        write(&f.layout.live(n), &format!("B-{n}"));
        write(&f.layout.prev(n), &format!("A-{n}"));
    }
    fs::remove_file(f.layout.live_dir.join(PROMOTED_FILE)).unwrap();
    write(
        &swap.join(PROMOTED_FILE),
        &serde_json::to_string(&a).unwrap(),
    );
    recover(&f);
    assert_eq!(read(&f.layout.live("aida")), "B-aida");
    assert_eq!(read_promotion_meta(&f.layout.live_dir), None);
    assert_eq!(read_promotion_meta(&f.layout.prev_dir()), a.clone());
    assert_consistent(&f, &a, &b, "old format without prev record");
}

// trace:BUG-1999 | ai:antigravity
#[test]
fn recovery_with_old_format_crash_during_normalization_preserves_records() {
    // Interruption after writing ROLLBACK_MARKER and parking prev's record,
    // but before moving .rollback/.promoted to live.promoted.
    let f = fixture();
    let (a, b) = pairs_a_b(&f, true, true);
    let swap = swap_dir(&f);
    fs::create_dir_all(&swap).unwrap();
    for n in LIVE_BINARIES {
        write(&f.layout.live(n), &format!("B-{n}"));
        write(&f.layout.prev(n), &format!("A-{n}"));
    }
    write(&swap.join(ROLLBACK_MARKER), "");
    write(
        &swap.join(PARKED_PREV_RECORD),
        &serde_json::to_string(&b).unwrap(),
    );
    fs::remove_file(f.layout.prev_dir().join(PROMOTED_FILE)).unwrap();
    fs::remove_file(f.layout.live_dir.join(PROMOTED_FILE)).unwrap();
    write(
        &swap.join(PROMOTED_FILE),
        &serde_json::to_string(&a).unwrap(),
    );

    recover(&f);
    assert_eq!(read(&f.layout.live("aida")), "B-aida");
    assert_eq!(read_promotion_meta(&f.layout.live_dir), b.clone());
    assert_eq!(read_promotion_meta(&f.layout.prev_dir()), a.clone());
    assert_consistent(&f, &a, &b, "old format crash during normalization");
}

// trace:BUG-1999 | ai:antigravity
#[test]
fn recovery_with_old_format_crash_between_binary_swaps() {
    // Old binary crashed between the two binary swaps: aida has been swapped
    // (B moved live, A moved to prev/), aida-server not yet started.
    let f = fixture();
    let (a, b) = pairs_a_b(&f, true, true);
    let swap = swap_dir(&f);
    fs::create_dir_all(&swap).unwrap();
    for n in LIVE_BINARIES {
        write(&swap.join(n), &format!("A-{n}"));
    }
    fs::remove_file(f.layout.live_dir.join(PROMOTED_FILE)).unwrap();
    write(
        &swap.join(PROMOTED_FILE),
        &serde_json::to_string(&a).unwrap(),
    );
    fs::rename(f.layout.prev("aida"), f.layout.live("aida")).unwrap();
    fs::rename(swap.join("aida"), f.layout.prev("aida")).unwrap();

    recover(&f);
    assert_eq!(read(&f.layout.live("aida")), "B-aida");
    assert_eq!(read(&f.layout.live("aida-server")), "A-aida-server");
    assert_eq!(read_promotion_meta(&f.layout.live_dir), b.clone());
    assert_eq!(read_promotion_meta(&f.layout.prev_dir()), a.clone());
    assert_consistent(&f, &a, &b, "old format between binary swaps");
}
