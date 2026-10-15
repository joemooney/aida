//! BUG-2133: a contained drain contains EVERY agent child it spawns.
//!
//! Advisor ruling (2026-10-14): containment is monotonic. The phase driver's
//! CI-fix child (it writes code) and advisor child (it reads untrusted PR
//! content and runs commands) used to be launched with a hard-coded `false`
//! posture, so under a contained drain they ran uncontained. They now take the
//! drain's contained flag, and a child that cannot be contained (a vendor with
//! no contained posture, or no `bwrap` sandbox) is refused rather than
//! silently downgraded.
//!
//! No real vendor CLI is invoked: spawning tests install `#!/bin/sh`
//! fixtures through the `AIDA_AGENT_CMD` seam and a failing fake `gh` through
//! `AIDA_TEST_GH_BINARY`.
// trace:BUG-2133 | ai:claude
use crate::phase_driver::{
    build_phase_driver_advisor_command, contained_child_program_and_args, RealPhaseDriver,
};
use crate::phase_driver_trait::build_phase_driver_ci_fix_command;
use crate::session::{self, HeadlessVendor};
use crate::test_env::EnvVarsGuard;

const SID: &str = "019e0000-0000-7000-8000-000000002133";

fn git_init(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    let status = std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(dir)
        .status()
        .unwrap();
    assert!(status.success());
}

fn env_value(cmd: &std::process::Command, key: &str) -> Option<Option<String>> {
    cmd.get_envs()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
}

/// The `--settings` JSON value from a contained claude argv.
fn settings_json(args: &[String]) -> String {
    let i = args
        .iter()
        .position(|a| a == "--settings")
        .unwrap_or_else(|| panic!("contained argv carries --settings: {args:?}"));
    args[i + 1].clone()
}

fn permission_mode(args: &[String]) -> &str {
    let i = args
        .iter()
        .position(|a| a == "--permission-mode")
        .unwrap_or_else(|| panic!("argv carries --permission-mode: {args:?}"));
    &args[i + 1]
}

// ── fail closed ─────────────────────────────────────────────────────────────

#[test]
fn uncontained_children_are_never_gated() {
    for vendor in [
        HeadlessVendor::Claude,
        HeadlessVendor::Codex,
        HeadlessVendor::Agy,
    ] {
        for sandbox in [true, false] {
            session::child_containment_preflight(vendor, false, sandbox, "CI-fix session")
                .unwrap_or_else(|e| panic!("{vendor:?} sandbox={sandbox}: {e:#}"));
        }
    }
}

#[test]
fn contained_child_on_a_vendor_without_containment_is_refused() {
    for vendor in [HeadlessVendor::Codex, HeadlessVendor::Agy] {
        let err = session::child_containment_preflight(vendor, true, true, "CI-fix session")
            .expect_err("a contained child on a vendor with no contained posture must be refused");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("refusing to launch the CI-fix session"),
            "{msg}"
        );
        assert!(msg.contains(vendor.as_str()), "{msg}");
    }
}

#[test]
fn contained_child_without_bwrap_is_refused() {
    let err = session::child_containment_preflight(
        HeadlessVendor::Claude,
        true,
        false,
        "advisor session",
    )
    .expect_err("containment requested but unavailable must fail closed");
    let msg = format!("{err:#}");
    assert!(msg.contains("bwrap"), "{msg}");
    assert!(
        msg.contains("refusing to launch the advisor session"),
        "{msg}"
    );
    session::child_containment_preflight(HeadlessVendor::Claude, true, true, "advisor session")
        .expect("claude with a sandbox available is launchable contained");
}

#[test]
fn contained_child_wrapper_refuses_codex_and_passes_uncontained_through() {
    let tmp = tempfile::tempdir().unwrap();
    let args = vec!["exec".to_string(), "prompt".to_string()];
    let (prog, out) = contained_child_program_and_args(
        HeadlessVendor::Codex,
        false,
        None,
        "CI-fix session",
        "codex",
        args.clone(),
    )
    .unwrap();
    assert_eq!(prog, "codex");
    assert_eq!(out, args, "uncontained: argv unchanged, no wrapper");

    let err = contained_child_program_and_args(
        HeadlessVendor::Codex,
        true,
        Some(tmp.path()),
        "CI-fix session",
        "codex",
        args,
    )
    .expect_err("contained codex child must be refused");
    assert!(format!("{err:#}").contains("refusing"), "{err:#}");
}

// ── the flag propagates to each child arm ───────────────────────────────────

#[test]
fn ci_fix_argv_is_contained_and_scoped_to_its_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("implementer-wt");
    git_init(&wt);
    let wt = std::fs::canonicalize(&wt).unwrap();

    let contained = session::headless_vendor_args_contained_in(
        HeadlessVendor::Claude,
        "fix ci",
        SID,
        Some(&wt),
        None,
        None,
    );
    assert_eq!(permission_mode(&contained), "dontAsk", "{contained:?}");
    let settings = settings_json(&contained);
    assert!(
        settings.contains(&wt.to_string_lossy().into_owned()),
        "write boundary must be the CI-fix worktree: {settings}"
    );
    assert!(
        !contained.iter().any(|a| a == "bypassPermissions"),
        "{contained:?}"
    );

    // Uncontained is byte-identical to the pre-BUG-2133 call.
    let uncontained = session::headless_vendor_args_contained_in(
        HeadlessVendor::Claude,
        "fix ci",
        SID,
        None,
        None,
        None,
    );
    assert_eq!(
        uncontained,
        session::headless_vendor_args(HeadlessVendor::Claude, "fix ci", SID, false, None, None)
    );
}

#[test]
fn advisor_argv_is_contained_for_cold_boot_and_fork() {
    let _env = EnvVarsGuard::apply(&[("AIDA_AGENT_CMD", None)]);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    git_init(&root);
    let root = std::fs::canonicalize(&root).unwrap();

    for is_fork in [false, true] {
        let (prog, args) = session::advisor_tier_program_and_args_with_posture(
            HeadlessVendor::Claude,
            is_fork,
            "seeded /aida-advise",
            SID,
            None,
            None,
            Some(&root),
        );
        assert_eq!(prog, "claude");
        assert_eq!(
            permission_mode(&args),
            "dontAsk",
            "is_fork={is_fork}: {args:?}"
        );
        assert!(
            settings_json(&args).contains(&root.to_string_lossy().into_owned()),
            "is_fork={is_fork}: advisor write boundary must be its cwd: {args:?}"
        );
        assert_eq!(args.contains(&"--resume".to_string()), is_fork, "{args:?}");

        // Uncontained is byte-identical to the pre-BUG-2133 builder.
        let (_, plain) = session::advisor_tier_program_and_args_with_posture(
            HeadlessVendor::Claude,
            is_fork,
            "seeded /aida-advise",
            SID,
            None,
            None,
            None,
        );
        let (_, legacy) = session::advisor_tier_program_and_args_with_tuning(
            HeadlessVendor::Claude,
            is_fork,
            "seeded /aida-advise",
            SID,
            None,
            None,
        );
        assert_eq!(plain, legacy);
        assert_eq!(permission_mode(&plain), "bypassPermissions");
    }
}

#[test]
fn contained_child_commands_export_aida_contained() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path();

    let ci_fix =
        build_phase_driver_ci_fix_command(p, "claude", &[], "K", "V", HeadlessVendor::Claude, true);
    assert_eq!(env_value(&ci_fix, "AIDA_CONTAINED"), Some(Some("1".into())));
    let advisor = build_phase_driver_advisor_command(
        p,
        "claude",
        &[],
        &p.join("req"),
        &p.join("resp"),
        "K",
        "V",
        HeadlessVendor::Claude,
        true,
    );
    assert_eq!(
        env_value(&advisor, "AIDA_CONTAINED"),
        Some(Some("1".into()))
    );

    // Uncontained still strips an inherited marker (BUG-2124 unchanged).
    let ci_fix_plain = build_phase_driver_ci_fix_command(
        p,
        "claude",
        &[],
        "K",
        "V",
        HeadlessVendor::Claude,
        false,
    );
    assert_eq!(env_value(&ci_fix_plain, "AIDA_CONTAINED"), Some(None));
}

#[test]
fn drain_contained_reads_the_env_and_the_agents_knob() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    let home_s = home.to_string_lossy().into_owned();

    {
        let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home_s)), ("AIDA_CONTAINED", None)]);
        assert!(!session::drain_contained(&root).unwrap());
    }
    {
        let _env =
            EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home_s)), ("AIDA_CONTAINED", Some("1"))]);
        assert!(session::drain_contained(&root).unwrap(), "AIDA_CONTAINED=1");
    }
    std::fs::write(
        root.join(".aida/agents.toml"),
        "[agents]\ncontained = true\n",
    )
    .unwrap();
    {
        let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home_s)), ("AIDA_CONTAINED", None)]);
        assert!(
            session::drain_contained(&root).unwrap(),
            "[agents] contained knob"
        );
    }
}

// ── the real CI-fix arm, end to end ─────────────────────────────────────────

struct CiFixFixture {
    _base: tempfile::TempDir,
    project: std::path::PathBuf,
    worktree: std::path::PathBuf,
    outside: std::path::PathBuf,
    agent: std::path::PathBuf,
    gh: std::path::PathBuf,
}

/// A project, an implementer worktree, an "outside" directory, a fake agent
/// that tries to write in both, and a fake `gh` that always fails. Rooted in
/// the crate directory, not the system temp dir: the `bwrap` sandbox mounts a
/// fresh tmpfs on `/tmp` and a fresh `/dev`, so an "outside" path there would
/// not be a host path at all.
fn ci_fix_fixture() -> CiFixFixture {
    let base = tempfile::Builder::new()
        .prefix(".bug2133-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .unwrap();
    let project = base.path().join("project");
    let worktree = base.path().join("implementer-wt");
    let outside = base.path().join("outside");
    git_init(&project);
    git_init(&worktree);
    std::fs::create_dir_all(&outside).unwrap();
    let agent = base.path().join("fake-agent");
    crate::test_exec::write_executable(
        &agent,
        format!(
            "#!/bin/sh\n\
             if echo in > \"$PWD/inside.txt\"; then r=0; else r=1; fi\n\
             if echo out > '{out}/outside.txt' 2>/dev/null; then o=0; else o=1; fi\n\
             echo \"inside_rc=$r outside_rc=$o\" > '{wt}/result.txt'\n\
             exit 0\n",
            out = outside.display(),
            wt = worktree.display(),
        ),
    );
    let gh = base.path().join("fake-gh");
    crate::test_exec::write_executable(&gh, "#!/bin/sh\nexit 1\n");
    CiFixFixture {
        project,
        worktree,
        outside,
        agent,
        gh,
        _base: base,
    }
}

fn ci_fix_driver(fx: &CiFixFixture, contained: bool) -> RealPhaseDriver {
    let mut d = RealPhaseDriver::new(
        fx.project.clone(),
        "BUG-2133".to_string(),
        "test-queue".to_string(),
        None,
        true,
        None,
        crate::AutonomyMode::Default,
        "run-token".to_string(),
        false,
        false,
        false,
        false,
        crate::auto_complete::LifecycleSkip::default(),
        crate::auto_complete::AutoCompleteVariant::Full,
    );
    d.contained = contained;
    d.branch = Some("bug-2133-work".to_string());
    d.implementer_worktree = Some(fx.worktree.clone());
    d.ci_run_id = Some("1".to_string());
    d
}

fn run_ci_fix(fx: &CiFixFixture, vendor: &str, os_wrap: &str, contained: bool) {
    use crate::auto_complete::PhaseDriver;
    let agent = fx.agent.to_string_lossy().into_owned();
    let gh = fx.gh.to_string_lossy().into_owned();
    let _env = EnvVarsGuard::apply(&[
        ("AIDA_HEADLESS_VENDOR", Some(vendor)),
        ("AIDA_AGENT_CMD", Some(&agent)),
        ("AIDA_TEST_GH_BINARY", Some(&gh)),
        ("AIDA_OS_WRAP", Some(os_wrap)),
        ("AIDA_TEE_HEADLESS", Some("0")),
        ("AIDA_CONTAINED", None),
    ]);
    let mut driver = ci_fix_driver(fx, contained);
    let failure = crate::auto_complete::PhaseFailure::new("CI red".to_string());
    // No origin, so no push is ever observed: the return is always false.
    // The assertions are on what the child could and could not write.
    assert!(!driver.attempt_ci_fix(1, 1, &failure));
}

#[test]
fn contained_ci_fix_on_codex_is_refused_before_launch() {
    let fx = ci_fix_fixture();
    run_ci_fix(&fx, "codex", "0", true);
    assert!(
        !fx.worktree.join("result.txt").exists(),
        "a contained drain must not launch an uncontainable CI-fix child"
    );
}

/// Whether this host can actually run the bwrap sandbox (installed, and the
/// user namespace is permitted — not the case on stock Ubuntu 24.04 runners).
fn bwrap_usable() -> bool {
    std::process::Command::new("bwrap")
        .args([
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "true",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Functional verification (STRICT tier): a contained CI-fix child, launched
/// through the real `attempt_ci_fix` arm with the OS boundary on, writes
/// inside its worktree but is refused a write outside it. The uncontained
/// control run shows the same write succeeding, so the refusal is the
/// containment, not the fixture.
#[test]
fn contained_ci_fix_child_writes_in_worktree_and_is_refused_outside() {
    if !bwrap_usable() {
        eprintln!("skipping: bwrap cannot create a sandbox on this host");
        return;
    }
    let fx = ci_fix_fixture();
    run_ci_fix(&fx, "claude", "1", true);
    let result = std::fs::read_to_string(fx.worktree.join("result.txt"))
        .expect("the contained CI-fix child ran");
    assert_eq!(result.trim(), "inside_rc=0 outside_rc=1", "{result}");
    assert!(fx.worktree.join("inside.txt").exists());
    assert!(
        !fx.outside.join("outside.txt").exists(),
        "a contained CI-fix child must not write outside its worktree"
    );

    // Control: the same child, uncontained, does write outside.
    std::fs::remove_file(fx.worktree.join("result.txt")).unwrap();
    run_ci_fix(&fx, "claude", "0", false);
    let result = std::fs::read_to_string(fx.worktree.join("result.txt")).unwrap();
    assert_eq!(result.trim(), "inside_rc=0 outside_rc=0", "{result}");
    assert!(fx.outside.join("outside.txt").exists());
}
