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
            calls: Vec::new(),
        }
    }
    fn called(&self, step: &str) -> bool {
        self.calls.iter().any(|c| c == step)
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
    fn guard(&mut self, _l: &DevLayout, promoting: bool, _after_wave: bool) -> Result<()> {
        self.calls.push(format!("guard:{promoting}"));
        Ok(())
    }
    fn clean_full(&mut self, _l: &DevLayout) -> Result<()> {
        self.calls.push("clean_full".into());
        Ok(())
    }
    fn build(&mut self, l: &DevLayout) -> Result<()> {
        self.calls.push("build".into());
        if self.fail_build {
            bail!("error[E0425]: cannot find value");
        }
        for n in LIVE_BINARIES {
            write(&l.built(n), &format!("{}-{n}", self.build_tag));
        }
        Ok(())
    }
    fn smoke(&mut self, _l: &DevLayout) -> Result<()> {
        self.calls.push("smoke".into());
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
    assert!(steps.called("guard:true") && steps.called("restart_mcp"));
    assert!(out.contains("Promoted"), "{out}");
    // No staging leftovers.
    let mut names: Vec<_> = fs::read_dir(&f.layout.live_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["aida", "aida-server", "prev"]);
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
    assert!(steps.called("smoke") && steps.called("guard:false"));
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
    // Clean ran before the build.
    let pos = |s: &str| steps.calls.iter().position(|c| c == s).unwrap();
    assert!(steps.called("build") && pos("build") > 0);
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
    rollback(&f.layout).unwrap();
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
