//! BUG-2151: `aida agent new` honours a contained parent, and `--bg` is not a
//! posture flag.
//!
//! Ruling on BUG-2133 (monotonic containment): a contained parent's env
//! (`AIDA_CONTAINED`, parsed strictly) ALWAYS wins and no flag overrides it;
//! only real operator posture flags (`--permission-mode`, `--bypass-sandbox`)
//! may override the `[agents] contained` CONFIG knob, never the env; `--bg`
//! is not a posture flag.
// trace:BUG-2151 | ai:claude
use crate::test_env::EnvVarsGuard;
use crate::{
    apply_agent_default_flags, refuse_uncontained_child_of_contained_parent,
    resolve_claude_new_posture, AgentDefaultFlagOptions, AgentLaunchConfig, AgentPromptStyle,
};

fn has_bypass(args: &[String]) -> bool {
    args.iter().any(|a| {
        a == "bypassPermissions" || a.starts_with("--dangerously") || a == "--permission-mode"
    })
}

/// A temp project whose `[agents] contained` knob is `knob`.
fn project(tmp: &std::path::Path, knob: bool) -> (std::path::PathBuf, String) {
    let home = tmp.join("home");
    let root = tmp.join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/agents.toml"),
        format!("[agents]\ncontained = {knob}\n"),
    )
    .unwrap();
    (root, home.to_string_lossy().into_owned())
}

/// The `agent new claude` pipeline minus the spawn: posture → agents.toml
/// defaults → the contained-parent guard.
fn claude_launch(
    root: &std::path::Path,
    permission_mode: Option<&str>,
    bg: bool,
    extra_flags: Vec<String>,
) -> anyhow::Result<AgentLaunchConfig> {
    let parent = crate::session::aida_contained_env()?;
    let posture = resolve_claude_new_posture(permission_mode, false, bg, parent, || {
        crate::agent_config::load_agents_contained(root)
    })?;
    let mut config = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: posture.default_args,
        prompt_style: AgentPromptStyle::Positional,
        contained: posture.contained,
    };
    apply_agent_default_flags(
        &mut config,
        root,
        AgentDefaultFlagOptions::new(true, extra_flags, None),
        posture.explicit,
    )?;
    refuse_uncontained_child_of_contained_parent(&config)?;
    Ok(config)
}

// ── (1) a contained parent contains the child ───────────────────────────────

#[test]
fn contained_parent_contains_claude_child_with_the_knob_off() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    let cfg = claude_launch(&root, None, false, Vec::new()).unwrap();
    assert!(cfg.contained, "the child must be contained");
    assert!(crate::agent_launch::is_contained_launch(&cfg));
    assert!(
        cfg.default_args.iter().any(|a| a == "--settings"),
        "{:?}",
        cfg.default_args
    );
    assert!(!has_bypass(&cfg.default_args), "{:?}", cfg.default_args);
}

#[test]
fn contained_parent_refuses_a_permission_mode_override() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    let err = claude_launch(&root, Some("bypassPermissions"), false, Vec::new())
        .expect_err("a contained parent refuses --permission-mode");
    assert!(format!("{err:#}").contains("AIDA_CONTAINED"), "{err:#}");
}

#[test]
fn contained_parent_refuses_a_passthrough_posture_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    for extra in [
        vec!["--dangerously-skip-permissions".to_string()],
        vec![
            "--permission-mode".to_string(),
            "bypassPermissions".to_string(),
        ],
        vec!["--settings".to_string(), "{}".to_string()],
    ] {
        let err = claude_launch(&root, None, false, extra.clone())
            .expect_err("a `--` posture passthrough must not escape containment");
        assert!(
            format!("{err:#}").contains("refusing to launch"),
            "{extra:?}: {err:#}"
        );
    }
}

#[test]
fn contained_parent_refuses_codex_and_agy_even_with_bypass_sandbox() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    for (agent_type, binary, bypass) in [
        (
            "codex",
            "codex",
            "--dangerously-bypass-approvals-and-sandbox",
        ),
        ("antigravity", "agy", "--dangerously-skip-permissions"),
    ] {
        for explicit in [false, true] {
            let mut cfg = AgentLaunchConfig {
                agent_type,
                binary,
                default_args: if explicit {
                    vec![bypass.to_string()]
                } else {
                    Vec::new()
                },
                prompt_style: AgentPromptStyle::Positional,
                contained: false,
            };
            apply_agent_default_flags(
                &mut cfg,
                &root,
                AgentDefaultFlagOptions::new(true, Vec::new(), None),
                explicit,
            )
            .unwrap();
            let err = refuse_uncontained_child_of_contained_parent(&cfg)
                .expect_err("a contained parent never launches codex/agy");
            assert!(
                format!("{err:#}").contains("cannot run an AIDA-contained session"),
                "{agent_type}: {err:#}"
            );
        }
    }
}

#[test]
fn a_malformed_aida_contained_fails_closed_on_agent_new() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("ture"))]);
    let err = claude_launch(&root, None, false, Vec::new()).expect_err("typo must refuse");
    assert!(format!("{err:#}").contains("not recognised"), "{err:#}");
    let cfg = AgentLaunchConfig {
        agent_type: "codex",
        binary: "codex",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    };
    assert!(refuse_uncontained_child_of_contained_parent(&cfg).is_err());
}

#[test]
fn an_uncontained_parent_is_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", None)]);
    let cfg = claude_launch(&root, Some("acceptEdits"), false, Vec::new()).unwrap();
    assert!(!cfg.contained);
    let cfg = claude_launch(&root, None, false, Vec::new()).unwrap();
    assert!(!cfg.contained);
}

// ── (2) `--bg` is not a posture flag ────────────────────────────────────────

#[test]
fn bg_under_a_contained_parent_stays_contained() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    let cfg = claude_launch(&root, None, true, Vec::new()).unwrap();
    assert!(cfg.contained);
    assert!(!has_bypass(&cfg.default_args), "{:?}", cfg.default_args);
}

#[test]
fn bg_under_the_contained_knob_stays_contained() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), true);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", None)]);
    let cfg = claude_launch(&root, None, true, Vec::new()).unwrap();
    assert!(cfg.contained, "--bg must not escape [agents] contained");
    assert!(crate::agent_launch::is_contained_launch(&cfg));
    assert!(!has_bypass(&cfg.default_args), "{:?}", cfg.default_args);
}

#[test]
fn bg_without_containment_still_forces_bypass() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", None)]);
    let cfg = claude_launch(&root, None, true, Vec::new()).unwrap();
    assert!(!cfg.contained);
    assert!(cfg
        .default_args
        .windows(2)
        .any(|w| w == ["--permission-mode", "bypassPermissions"]));
}

#[test]
fn an_operator_posture_flag_still_overrides_the_config_knob() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), true);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", None)]);
    let cfg = claude_launch(&root, Some("acceptEdits"), true, Vec::new()).unwrap();
    assert!(!cfg.contained);
    assert!(cfg
        .default_args
        .windows(2)
        .any(|w| w == ["--permission-mode", "acceptEdits"]));
}

// ── (3) #2672 round 1: resume paths ─────────────────────────────────────────

#[test]
fn contained_parent_refuses_allow_dangerously_skip_permissions_passthrough() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    let err = claude_launch(
        &root,
        None,
        false,
        vec!["--allow-dangerously-skip-permissions".to_string()],
    )
    .expect_err("--allow-dangerously-skip-permissions must not escape containment");
    assert!(format!("{err:#}").contains("refusing to launch"), "{err:#}");
}

/// Register an ended, resumable agent of `agent_type` named `name`.
fn ended_agent(root: &std::path::Path, agent_type: &str, name: &str) {
    let dead_pid = 4_294_967_290u32;
    crate::agent_registry::register_spawned_agent(
        root,
        agent_type,
        dead_pid,
        Some("implementer".into()),
        None,
        root.into(),
        None,
        Some(name.into()),
        Some("018f-session".into()),
        None,
        None,
    )
    .unwrap();
    crate::agent_registry::mark_agent_ended(root, agent_type, dead_pid).unwrap();
}

#[test]
fn resume_config_under_a_contained_parent_contains_claude() {
    let _env = EnvVarsGuard::apply(&[("AIDA_CONTAINED", Some("1"))]);
    let cfg = crate::agent_launch::agent_resume_launch_config(
        "claude",
        &std::env::current_dir().unwrap(),
        "018f-session",
        "go",
    )
    .unwrap();
    assert!(cfg.contained, "a resumed claude child must be contained");
    assert!(crate::agent_launch::is_contained_launch(&cfg));
    assert!(
        cfg.default_args.iter().any(|a| a == "--settings"),
        "{:?}",
        cfg.default_args
    );
    assert!(
        cfg.default_args
            .windows(2)
            .any(|w| w == ["--resume", "018f-session"]),
        "{:?}",
        cfg.default_args
    );
    assert!(!has_bypass(&cfg.default_args), "{:?}", cfg.default_args);
}

#[test]
fn resume_config_without_a_contained_parent_is_unchanged() {
    let _env = EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    for t in ["claude", "codex", "antigravity"] {
        let cfg = crate::agent_launch::agent_resume_launch_config(
            t,
            &std::env::current_dir().unwrap(),
            "sid",
            "go",
        )
        .unwrap();
        assert!(!cfg.contained, "{t}");
        assert!(!cfg.default_args.iter().any(|a| a == "--settings"), "{t}");
    }
}

#[test]
fn agent_new_resume_under_a_contained_parent_refuses_codex_and_agy() {
    for (agent_type, name) in [("codex", "codex-impl-1"), ("antigravity", "agy-impl-1")] {
        let tmp = tempfile::tempdir().unwrap();
        let (root, home) = project(tmp.path(), false);
        ended_agent(&root, agent_type, name);
        let _env =
            EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
        let resume = crate::AgentResumeOptions::new(false, Some(name.to_string()), false, false);
        let err = crate::agent_launch::maybe_handle_agent_resume_or_duplicate(
            &root, agent_type, None, None, &resume,
        )
        .expect_err("--resume under a contained parent must refuse codex/agy");
        assert!(
            format!("{err:#}").contains("cannot run an AIDA-contained session"),
            "{agent_type}: {err:#}"
        );
    }
}

#[test]
fn agent_resume_under_a_contained_parent_refuses_codex() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    ended_agent(&root, "codex", "codex-impl-2");
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    let err = crate::agent_launch::agent_resume_in(&root, "codex-impl-2")
        .expect_err("`agent resume` under a contained parent must refuse codex");
    assert!(
        format!("{err:#}").contains("cannot run an AIDA-contained session"),
        "{err:#}"
    );
}

#[test]
fn agent_resume_under_a_malformed_aida_contained_fails_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    ended_agent(&root, "codex", "codex-impl-3");
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("ture"))]);
    let err = crate::agent_launch::agent_resume_in(&root, "codex-impl-3").unwrap_err();
    assert!(format!("{err:#}").contains("not recognised"), "{err:#}");
}

/// The value following `--settings` in `args`.
fn settings_value(args: &[String]) -> String {
    args.windows(2)
        .find(|w| w[0] == "--settings")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| panic!("no --settings in {args:?}"))
}

/// Round-2 review: the resumed child runs in the agent's recorded worktree,
/// so its contained write roots must be THAT worktree, not the launcher's cwd.
// trace:BUG-2151 | ai:claude
#[test]
fn resumed_contained_child_write_roots_follow_its_worktree_not_the_launcher_cwd() {
    let _env = EnvVarsGuard::apply(&[("AIDA_CONTAINED", Some("1"))]);
    let tmp = tempfile::tempdir().unwrap();
    let worktree = std::fs::canonicalize(tmp.path()).unwrap().join("proj-wt");
    std::fs::create_dir_all(&worktree).unwrap();

    let cfg =
        crate::agent_launch::agent_resume_launch_config("claude", &worktree, "018f-session", "go")
            .unwrap();
    let settings = settings_value(&cfg.default_args);
    let expected = settings_value(&crate::session::claude_contained_flags_for(&worktree).unwrap());
    assert_eq!(
        settings, expected,
        "resumed child's write roots must be its worktree"
    );
    assert!(
        settings.contains(&*worktree.to_string_lossy()),
        "worktree {} missing from {settings}",
        worktree.display()
    );
    let launcher = settings_value(&crate::session::claude_contained_flags().unwrap());
    assert_ne!(
        settings, launcher,
        "resumed child must not inherit the launcher cwd's write roots"
    );
}

// TASK-1781 (b): a contained parent may not pass --no-default-flags, which
// skips the agents.toml contained posture; an uncontained shell still may.
// trace:TASK-1952 | trace:TASK-1781 | ai:claude
#[test]
fn task_1781_contained_parent_refuses_no_default_flags() {
    let skip = AgentDefaultFlagOptions::new(false, Vec::new(), None);
    let err = crate::agent_launch::refuse_no_default_flags_under_contained_parent(true, &skip)
        .expect_err("refused under a contained parent");
    assert!(format!("{err:#}").contains("--no-default-flags"), "{err:#}");
    crate::agent_launch::refuse_no_default_flags_under_contained_parent(false, &skip)
        .expect("the operator's escape hatch from an uncontained shell");
    let defaults = AgentDefaultFlagOptions::new(true, Vec::new(), None);
    crate::agent_launch::refuse_no_default_flags_under_contained_parent(true, &defaults)
        .expect("defaults applied");
}

// TASK-2317: the refusal holds at the agent_new_claude call site, not only in
// the helper — a contained parent passing --no-default-flags refuses before
// the role picker or any project lookup (`cwd` is not an AIDA project, so
// reaching either would fail with a different error).
// trace:TASK-2272 | ai:claude (TASK-2317)
#[test]
fn task_2317_agent_new_claude_refuses_no_default_flags_under_contained_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let (_root, home) = project(tmp.path(), false);
    let nowhere = tmp.path().join("not-a-project");
    std::fs::create_dir_all(&nowhere).unwrap();
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    let err = crate::agent_launch::agent_new_claude(
        None,
        None,
        false,
        Some(&nowhere),
        None,
        false,
        crate::agent_launch::AgentContextOptions::new(false, false),
        false,
        true,
        false,
        false,
        crate::agent_launch::AgentPromptOptions::new(None, true),
        crate::agent_launch::AgentResumeOptions::new(false, None, false, false),
        AgentDefaultFlagOptions::new(false, Vec::new(), None),
        None,
        None,
        false,
    )
    .expect_err("a contained parent may not pass --no-default-flags");
    assert!(format!("{err:#}").contains("--no-default-flags"), "{err:#}");
}

// TASK-2315: the staged resume transcript's source is opened O_NOFOLLOW and
// fstat'ed on the handle, so a symlinked source — whenever it was swapped in —
// is refused, while a regular source still stages from that same handle.
// trace:TASK-2272 | ai:claude (TASK-2315)
#[cfg(unix)]
#[test]
fn task_2315_symlinked_source_transcript_is_refused_via_o_nofollow() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _env = EnvVarsGuard::apply(&[("AIDA_TEST_HOME", Some(home.to_str().unwrap()))]);
    let listed_in = tmp.path().join("main");
    let cwd = tmp.path().join("tree");
    let from_dir = crate::session::claude_project_dir(&listed_in).unwrap();
    std::fs::create_dir_all(&from_dir).unwrap();
    let secret = tmp.path().join("secret.jsonl");
    std::fs::write(&secret, b"{\"secret\":1}").unwrap();
    std::os::unix::fs::symlink(&secret, from_dir.join("sess-2315.jsonl")).unwrap();

    let err = crate::session::stage_resume_transcript("sess-2315", &listed_in, &cwd).unwrap_err();
    assert!(format!("{err:#}").contains("not a regular file"), "{err:#}");
    let dest = crate::session::claude_project_dir(&cwd)
        .unwrap()
        .join("sess-2315.jsonl");
    assert!(!dest.exists(), "nothing staged from a symlinked source");

    // A regular source still stages, read from the O_NOFOLLOW handle.
    std::fs::remove_file(from_dir.join("sess-2315.jsonl")).unwrap();
    std::fs::write(from_dir.join("sess-2315.jsonl"), b"{\"ok\":1}").unwrap();
    crate::session::stage_resume_transcript("sess-2315", &listed_in, &cwd).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b"{\"ok\":1}");

    // And a missing source stays a quiet no-op.
    crate::session::stage_resume_transcript("sess-absent", &listed_in, &cwd).unwrap();
}

// TASK-2321: a FIFO planted as the source must refuse at once — the open
// carries O_NONBLOCK, so it cannot park the resume until a writer appears.
// trace:TASK-2321 | ai:claude
#[cfg(unix)]
#[test]
fn task_2321_fifo_source_transcript_refuses_without_hanging() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _env = EnvVarsGuard::apply(&[("AIDA_TEST_HOME", Some(home.to_str().unwrap()))]);
    let listed_in = tmp.path().join("main");
    let cwd = tmp.path().join("tree");
    let from_dir = crate::session::claude_project_dir(&listed_in).unwrap();
    std::fs::create_dir_all(&from_dir).unwrap();
    let fifo = from_dir.join("sess-2321.jsonl");
    let c_path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: mkfifo on a fresh path inside a private tempdir.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

    let start = std::time::Instant::now();
    let err = crate::session::stage_resume_transcript("sess-2321", &listed_in, &cwd).unwrap_err();
    assert!(format!("{err:#}").contains("not a regular file"), "{err:#}");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "the open must not block on the FIFO"
    );
    let dest = crate::session::claude_project_dir(&cwd)
        .unwrap()
        .join("sess-2321.jsonl");
    assert!(!dest.exists(), "nothing staged from a FIFO source");
}

// TASK-1781 (d): codex and agy refuse under a contained parent before the
// role picker or any project lookup runs. `cwd` is not an AIDA project, so
// reaching the picker would fail with a different error.
// trace:TASK-1952 | trace:TASK-1781 | ai:claude
#[test]
fn task_1781_codex_and_agy_refuse_before_the_role_picker() {
    let tmp = tempfile::tempdir().unwrap();
    let (_root, home) = project(tmp.path(), false);
    let nowhere = tmp.path().join("not-a-project");
    std::fs::create_dir_all(&nowhere).unwrap();
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", Some("1"))]);
    type Launch = fn(
        Option<String>,
        Option<String>,
        bool,
        Option<&std::path::Path>,
        bool,
        crate::agent_launch::AgentContextOptions,
        bool,
        bool,
        bool,
        bool,
        crate::agent_launch::AgentPromptOptions,
        crate::agent_launch::AgentResumeOptions,
        AgentDefaultFlagOptions,
        Option<String>,
        Option<String>,
    ) -> anyhow::Result<()>;
    for (tool, launch) in [
        ("codex", crate::agent_launch::agent_new_codex as Launch),
        (
            "antigravity",
            crate::agent_launch::agent_new_antigravity as Launch,
        ),
    ] {
        let err = launch(
            None,
            None,
            false,
            Some(&nowhere),
            false,
            crate::agent_launch::AgentContextOptions::new(false, false),
            false,
            true,
            false,
            false,
            crate::agent_launch::AgentPromptOptions::new(None, true),
            crate::agent_launch::AgentResumeOptions::new(false, None, false, false),
            AgentDefaultFlagOptions::new(true, Vec::new(), None),
            None,
            None,
        )
        .expect_err("a contained parent refuses");
        assert!(
            format!("{err:#}").contains("cannot run an AIDA-contained session"),
            "{tool}: {err:#}"
        );
    }
}

// TASK-1761 (b): `--sandbox` alone contains a claude child, and a per-tool
// `--settings` in agents.toml does not make an uncontained launch contained.
// trace:TASK-1952 | trace:TASK-1761 | ai:claude
#[test]
fn task_1761_sandbox_contains_and_per_tool_settings_do_not() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = project(tmp.path(), false);
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", Some(&home)), ("AIDA_CONTAINED", None)]);
    let posture = aida_core::git_ops::with_host_git_version("git version 2.43.0", || {
        resolve_claude_new_posture(None, true, false, false, || Ok(false))
    })
    .unwrap();
    assert!(posture.contained, "--sandbox sets contained");
    let cfg = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: posture.default_args,
        prompt_style: AgentPromptStyle::Positional,
        contained: posture.contained,
    };
    assert!(crate::agent_launch::is_contained_launch(&cfg));

    std::fs::write(
        root.join(".aida/agents.toml"),
        "[agents]\ncontained = false\n\n[agents.claude]\ndefault_flags = [\"--settings\", \"{}\"]\n",
    )
    .unwrap();
    let mut cfg = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    };
    apply_agent_default_flags(
        &mut cfg,
        &root,
        AgentDefaultFlagOptions::new(true, Vec::new(), None),
        false,
    )
    .unwrap();
    assert!(
        cfg.default_args.iter().any(|a| a == "--settings"),
        "{:?}",
        cfg.default_args
    );
    assert!(
        !crate::agent_launch::is_contained_launch(&cfg),
        "a per-tool --settings reads as uncontained"
    );
}
