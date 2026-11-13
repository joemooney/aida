//! BUG-2393: headless callers honour CONFIGURED containment.
//!
//! `AIDA_CONTAINED` is not set for a cron/systemd tick or a fresh operator
//! shell, so a launch path that reads only the env (or passes a literal
//! `false`) ran uncontained even with `[agents] contained = true` in
//! `agents.toml` or `[contained] enable = true` in `.aida/config.toml`. Every
//! listed caller now resolves through `session::drain_contained`, which is the
//! env OR the project/user configuration. Containment stays monotonic: an
//! explicitly contained parent is never downgraded by a `contained = false`
//! file, and a malformed knob refuses the launch.
// trace:BUG-2393 | ai:claude
use crate::session;

/// A project root (with a `.aida-store` so the seat grant can be minted), a
/// fake agent that records its env and argv, and the
/// capture file it writes.
struct Fixture {
    _tmp: tempfile::TempDir,
    project: std::path::PathBuf,
    fake: String,
    capture: std::path::PathBuf,
}

fn fixture(agents_toml: Option<&str>) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join(".aida-store")).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    if let Some(body) = agents_toml {
        std::fs::write(project.join(".aida/agents.toml"), body).unwrap();
    }
    let fake = tmp.path().join("fake-claude");
    let capture = tmp.path().join("capture");
    crate::test_exec::write_executable(
        &fake,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$AIDA_CONTAINED\" \"$@\" > '{}'\n",
            capture.display()
        ),
    );
    Fixture {
        project,
        fake: fake.to_string_lossy().into_owned(),
        capture,
        _tmp: tmp,
    }
}

impl Fixture {
    /// No `AIDA_HOME` pin: the seat grant is minted under the hermetic test
    /// home before these extra pins apply, so a pinned home would hide it
    /// ("session grant is missing"). `cfg(test)` already fences the home.
    fn ambient(&self, contained_env: Option<&str>) -> crate::test_env::AmbientGuard {
        crate::test_env::AmbientGuard::hermetic_with_seat_and(
            &self.project,
            "advisor",
            &["advisor", "implementer"],
            &[
                ("AIDA_AGENT_CMD", Some(self.fake.as_str())),
                ("AIDA_OS_WRAP", Some("0")),
                ("AIDA_HEADLESS_VENDOR", None),
                ("AIDA_CONTAINED", contained_env),
                ("AIDA_TEST_CAPTURE", Some(self.capture.to_str().unwrap())),
            ],
        )
    }

    fn analysis_pass(&self) -> anyhow::Result<std::process::ExitStatus> {
        let log = self.project.join(".aida/analysis.log");
        let tee = crate::headless_tee::TeeOptions::from_env_and_flag(false);
        session::spawn_analysis_headless(&self.project, "prompt", "sid-2393", &log, &tee)
    }

    fn assert_contained_claude_launch(&self) {
        let captured = std::fs::read_to_string(&self.capture).unwrap();
        assert_eq!(
            captured.lines().next(),
            Some("1"),
            "child carries AIDA_CONTAINED=1: {captured}"
        );
        assert!(captured.lines().any(|l| l == "--settings"), "{captured}");
        assert!(
            captured.lines().any(|l| l == "--setting-sources"),
            "{captured}"
        );
        assert!(!captured.contains("bypassPermissions"), "{captured}");
    }
}

const CONTAINED_AGENTS: &str = "[agents]\ncontained = true\n";

/// Acceptance 2: `questions research`, `dryrun --ai` and `intent` launch via
/// the shared analysis helper; configured containment applies with the env
/// unset, for Claude, and refuses Codex before any spawn.
#[test]
fn analysis_pass_is_contained_by_agents_toml_without_the_env() {
    let fx = fixture(Some(CONTAINED_AGENTS));
    {
        let _ambient = fx.ambient(None);
        fx.analysis_pass().unwrap();
    }
    fx.assert_contained_claude_launch();

    std::fs::remove_file(&fx.capture).unwrap();
    std::fs::write(
        fx.project.join(".aida/agents.toml"),
        "[agents]\ncontained = true\nvendor = \"codex\"\n",
    )
    .unwrap();
    let _ambient = fx.ambient(None);
    let err = fx.analysis_pass().unwrap_err();
    assert!(
        format!("{err:#}").contains("cannot run a contained session"),
        "{err:#}"
    );
    assert!(!fx.capture.exists(), "Codex is refused before the spawn");
}

/// `[contained] enable = true` in `.aida/config.toml` is the same knob.
#[test]
fn analysis_pass_is_contained_by_config_toml_contained_enable() {
    let fx = fixture(None);
    std::fs::write(
        fx.project.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n\n[contained]\nenable = true\n",
    )
    .unwrap();
    {
        let _ambient = fx.ambient(None);
        fx.analysis_pass().unwrap();
    }
    fx.assert_contained_claude_launch();
}

/// An explicitly contained parent is never downgraded by a project file that
/// says `contained = false`. A contained parent refuses the `AIDA_AGENT_CMD`
/// seam (STORY-2326), so the fake is the `claude` on PATH instead.
// trace:STORY-2326 | ai:claude
#[test]
fn explicit_parent_containment_wins_over_a_false_knob() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = fixture(Some("[agents]\ncontained = false\n"));
    let bin = fx.project.parent().unwrap().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(&fx.fake, bin.join("claude")).unwrap();
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    {
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            &fx.project,
            "advisor",
            &["advisor", "implementer"],
            &[
                ("AIDA_AGENT_CMD", None),
                ("PATH", path.to_str()),
                ("AIDA_OS_WRAP", Some("0")),
                ("AIDA_HEADLESS_VENDOR", None),
                ("AIDA_CONTAINED", Some("1")),
                ("AIDA_TEST_CAPTURE", Some(fx.capture.to_str().unwrap())),
            ],
        );
        fx.analysis_pass().unwrap();
    }
    fx.assert_contained_claude_launch();
}

/// STORY-2326: a contained parent never swaps the vendor for the
/// `AIDA_AGENT_CMD` program; the launch is refused before the spawn.
// trace:STORY-2326 | ai:claude
#[test]
fn contained_parent_refuses_the_agent_cmd_override() {
    let fx = fixture(None);
    let _ambient = fx.ambient(Some("1"));
    let err = fx.analysis_pass().unwrap_err();
    assert!(
        format!("{err:#}").contains("`AIDA_AGENT_CMD` cannot replace"),
        "{err:#}"
    );
    assert!(!fx.capture.exists(), "refused before the spawn");
}

/// Uncontained stays uncontained: no env, no knob, no containment.
#[test]
fn no_env_and_no_knob_launches_uncontained() {
    let fx = fixture(None);
    {
        let _ambient = fx.ambient(None);
        fx.analysis_pass().unwrap();
    }
    let captured = std::fs::read_to_string(&fx.capture).unwrap();
    assert_eq!(captured.lines().next(), Some(""), "{captured}");
}

/// A malformed knob refuses the launch rather than reading as "uncontained".
#[test]
fn malformed_containment_knob_refuses_before_spawn() {
    let fx = fixture(Some("[agents]\ncontained = \"yes\"\n"));
    let _ambient = fx.ambient(None);
    assert!(fx.analysis_pass().is_err());
    assert!(!fx.capture.exists());
}

/// Acceptance 2, `compete`: configured containment refuses unsupported arms
/// and judges, and contained arm/judge commands carry `AIDA_CONTAINED=1`.
#[test]
fn compete_arms_and_judge_honour_configured_containment() {
    let fx = fixture(Some(CONTAINED_AGENTS));
    let _ambient = fx.ambient(None);
    let root = fx.project.as_path();
    let preflight = |vendors: &[&str], judge| {
        let vendors: Vec<String> = vendors.iter().map(|s| s.to_string()).collect();
        crate::compete_cmd::preflight_compete_containment(root, &vendors, judge)
    };
    assert!(preflight(&["claude", "codex"], None).is_err());
    assert!(preflight(&["claude"], Some(crate::compete::JudgeVendor::Codex)).is_err());
    preflight(&["claude"], Some(crate::compete::JudgeVendor::Claude)).unwrap();

    let arm =
        crate::compete_cmd::build_compete_arm_command(root, root, "claude", &[], "claude").unwrap();
    let judge = crate::compete_cmd::build_compete_judge_command(
        root,
        "claude",
        &[],
        crate::compete::JudgeVendor::Claude,
    )
    .unwrap();
    for cmd in [arm, judge] {
        assert!(
            cmd.get_envs()
                .any(|(k, v)| k == "AIDA_CONTAINED" && v == Some(std::ffi::OsStr::new("1"))),
            "contained compete child keeps AIDA_CONTAINED=1"
        );
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.iter().any(|a| a == "--settings"), "{args:?}");
    }
    assert!(
        crate::compete_cmd::build_compete_arm_command(root, root, "codex", &[], "codex").is_err()
    );
}

const CONTAINED_CODEX_AGENTS: &str = "[agents]\ncontained = true\nvendor = \"codex\"\n";

/// Acceptance 2, `burndown run` and `autopilot intake`: both coordinators
/// resolve their launch through `resolve_coordinator_launch` (pinned by the
/// source ratchet below). With the env unset, configured containment
/// contains a Claude coordinator (`dontAsk`, no bypass, no
/// `--permission-mode` override) and refuses a Codex one before any spawn.
#[test]
fn coordinator_launch_honours_configured_containment_without_the_env() {
    let fx = fixture(Some(CONTAINED_AGENTS));
    {
        let _ambient = fx.ambient(None);
        let launch = session::resolve_coordinator_launch(&fx.project, None).unwrap();
        assert_eq!(launch.vendor, session::HeadlessVendor::Claude);
        assert!(launch.contained, "configured containment applies");
        assert_eq!(launch.mode, "dontAsk");
        let err = session::resolve_coordinator_launch(&fx.project, Some("bypassPermissions"))
            .err()
            .expect("a contained coordinator refuses a permission override");
        assert!(format!("{err:#}").contains("[contained]"), "{err:#}");
    }
    std::fs::write(fx.project.join(".aida/agents.toml"), CONTAINED_CODEX_AGENTS).unwrap();
    let _ambient = fx.ambient(None);
    let err = session::resolve_coordinator_launch(&fx.project, None)
        .err()
        .expect("Codex cannot run a contained coordinator");
    let msg = format!("{err:#}");
    assert!(msg.contains("cannot run a contained session"), "{msg}");
    assert!(msg.contains("headless coordinator"), "{msg}");
}

/// Without a knob or env the coordinator stays uncontained (bypass default).
#[test]
fn coordinator_launch_without_env_or_knob_is_uncontained() {
    let fx = fixture(None);
    let _ambient = fx.ambient(None);
    let launch = session::resolve_coordinator_launch(&fx.project, None).unwrap();
    assert!(!launch.contained);
    assert_eq!(launch.mode, "bypassPermissions");
}

/// Acceptance 2, `session resume`: with the env unset, configured containment
/// refuses to resume a Codex session (it cannot run contained) instead of
/// resuming it uncontained; a malformed knob refuses before the target lookup.
#[test]
fn session_resume_honours_configured_containment_without_the_env() {
    let fx = fixture(Some(CONTAINED_AGENTS));
    {
        let _ambient = fx.ambient(None);
        let err = session::resume_with(&fx.project, || {
            Ok(session::ResumeTarget {
                agent: "codex".to_string(),
                id: "01a067ec".to_string(),
                worktree: None,
            })
        })
        .expect_err("a configured-contained resume refuses a codex session");
        assert!(format!("{err:#}").contains("refusing to resume"), "{err:#}");
    }
    std::fs::write(
        fx.project.join(".aida/agents.toml"),
        "[agents]\ncontained = \"yes\"\n",
    )
    .unwrap();
    let _ambient = fx.ambient(None);
    let err = session::resume_with(&fx.project, || {
        panic!("a malformed knob must refuse before the session lookup")
    })
    .expect_err("malformed knob refuses");
    assert!(format!("{err:#}").contains("[contained]"), "{err:#}");
}

/// Acceptance 2, `questions remedy`: with the env unset, configured
/// containment contains the Claude advisor child and refuses Codex before any
/// spawn (dry run included).
#[test]
fn questions_remedy_honours_configured_containment_without_the_env() {
    let fx = fixture(Some(CONTAINED_AGENTS));
    {
        let _ambient = fx.ambient(None);
        let status = crate::questions::launch_questions_remedy(&fx.project, "STORY-1", false)
            .unwrap()
            .expect("not a dry run");
        assert!(status.success());
    }
    fx.assert_contained_claude_launch();

    std::fs::remove_file(&fx.capture).unwrap();
    std::fs::write(fx.project.join(".aida/agents.toml"), CONTAINED_CODEX_AGENTS).unwrap();
    let _ambient = fx.ambient(None);
    for dry_run in [true, false] {
        let err = crate::questions::launch_questions_remedy(&fx.project, "STORY-1", dry_run)
            .expect_err("Codex cannot run a contained remedy");
        assert!(
            format!("{err:#}").contains("cannot run a contained session"),
            "dry_run={dry_run}: {err:#}"
        );
    }
    assert!(!fx.capture.exists(), "Codex is refused before the spawn");
}

/// Acceptance 2, `questions clarify`: it resolves through the same
/// `resolve_headless_launch_posture` as `questions remedy` (pinned by the
/// ratchet), so configured containment with the env unset refuses Codex and
/// yields a contained Claude advisor plan.
#[test]
fn questions_clarify_posture_honours_configured_containment_without_the_env() {
    let fx = fixture(Some(CONTAINED_AGENTS));
    {
        let _ambient = fx.ambient(None);
        let (vendor, contained) =
            session::resolve_headless_launch_posture(&fx.project, "interactive advisor session")
                .unwrap();
        assert_eq!(vendor, session::HeadlessVendor::Claude);
        assert!(contained);
        let plan = session::advisor_interactive_launch_plan(
            crate::seat_clone::launcher_main().as_deref(),
            vendor,
            "/aida-clarify X",
            contained,
        )
        .unwrap();
        assert!(
            plan.args.iter().any(|a| a == "--settings"),
            "{:?}",
            plan.args
        );
        assert!(
            !plan.args.iter().any(|a| a.contains("bypassPermissions")),
            "{:?}",
            plan.args
        );
    }
    std::fs::write(fx.project.join(".aida/agents.toml"), CONTAINED_CODEX_AGENTS).unwrap();
    let _ambient = fx.ambient(None);
    let err = session::resolve_headless_launch_posture(&fx.project, "interactive advisor session")
        .err()
        .expect("Codex cannot run a contained clarify");
    assert!(
        format!("{err:#}").contains("cannot run a contained session"),
        "{err:#}"
    );
}

/// Acceptance 1: every listed headless spawn or resume call site resolves
/// containment through `drain_contained`. The env-only read and the literal
/// `false` argument are gone from the listed modules, so a new call site there
/// cannot quietly reintroduce them.
#[test]
fn listed_headless_callers_resolve_configured_containment() {
    for (file, src) in [
        ("advisor_watch.rs", include_str!("../advisor_watch.rs")),
        ("questions.rs", include_str!("../questions.rs")),
        ("plan_intent.rs", include_str!("../plan_intent.rs")),
        ("reconstitute.rs", include_str!("../reconstitute.rs")),
        ("burndown_run.rs", include_str!("../burndown_run.rs")),
        (
            "autopilot_intake.rs",
            include_str!("../autopilot_intake.rs"),
        ),
        ("compete_cmd.rs", include_str!("../compete_cmd.rs")),
    ] {
        assert!(
            !src.contains("aida_contained_env("),
            "{file}: resolve containment with session::drain_contained, not the env alone"
        );
        let flat: String = src.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            !flat.contains("&tee, false, ") && !flat.contains("&tee, false)"),
            "{file}: a headless spawn passes a literal `false` for contained"
        );
    }
    // The behavioural tests above drive the shared resolvers; these pin that
    // the entry points actually use them.
    for (file, src, resolver, uses) in [
        (
            "burndown_run.rs",
            include_str!("../burndown_run.rs"),
            "resolve_coordinator_launch(",
            1,
        ),
        (
            "autopilot_intake.rs",
            include_str!("../autopilot_intake.rs"),
            "resolve_coordinator_launch(",
            1,
        ),
        (
            "questions.rs",
            include_str!("../questions.rs"),
            "resolve_headless_launch_posture(",
            2,
        ),
    ] {
        assert!(
            src.matches(resolver).count() >= uses,
            "{file}: launch resolves through `{resolver}`"
        );
    }
    let session_src = include_str!("../session.rs");
    for (func, end) in [
        (
            "pub fn resume(",
            "pub(crate) fn build_contained_resume_command(",
        ),
        (
            "pub(crate) fn spawn_analysis_headless(",
            "pub(crate) fn codex_inline_prompt(",
        ),
    ] {
        let body = session_src
            .split(func)
            .nth(1)
            .and_then(|rest| rest.split(end).next())
            .unwrap_or_else(|| panic!("session.rs: `{func}` not found"));
        assert!(
            body.contains("drain_contained("),
            "session.rs `{func}` resolves configured containment"
        );
    }
}
