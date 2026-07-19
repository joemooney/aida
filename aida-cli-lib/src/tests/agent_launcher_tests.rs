use super::*;
use clap::Parser;
use tempfile::TempDir;

/// BUG-421: `aida agent list-roles` must list exactly the canonical
/// AGENT_ROLES (so the listing and the validator can't drift) and must not
// regress to the pre-correction fabricated/inaccurate phrasing. trace:BUG-421
#[test]
fn agent_role_infos_cover_canonical_set_without_fabrications() {
    let listed: std::collections::HashSet<&str> = AGENT_ROLE_INFOS.iter().map(|i| i.role).collect();
    let canonical: std::collections::HashSet<&str> = AGENT_ROLES.iter().copied().collect();
    assert_eq!(
        listed, canonical,
        "list-roles must cover exactly the validated agent roles"
    );
    for info in AGENT_ROLE_INFOS {
        let s = info.summary.to_lowercase();
        assert!(
            !s.contains("main worktree"),
            "implementer uses a dedicated worktree, not main: {}",
            info.role
        );
        assert!(
            !s.contains("second opinion"),
            "advisor is the coordinator/escalation seat, not 'second opinions'"
        );
        assert!(
            !s.contains("fragments assembly"),
            "garbled reviewer phrasing"
        );
        assert!(
            !info.orchestrator_phase.is_empty(),
            "each role names its orchestrator phase"
        );
    }
}

#[test]
fn parses_agent_new_claude_flags() {
    let cli = Cli::try_parse_from([
        "aida",
        "agent",
        "new",
        "claude",
        "--role",
        "reviewer",
        "--spec",
        "STORY-432",
        "--cwd",
        "/tmp/project",
        "--permission-mode",
        "acceptEdits",
        "--prompt",
        "review STORY-432",
        "--no-default-flags",
        "--extra-flag",
        "--verbose",
    ])
    .unwrap();
    let Command::Agent(AgentCommand::New {
        command:
            Some(AgentNewCommand::Claude {
                role,
                spec,
                cwd,
                permission_mode,
                no_context,
                show_context,
                prompt,
                no_prompt,
                no_default_flags,
                extra_flags,
                ..
            }),
    }) = cli.command
    else {
        panic!("expected agent new claude command");
    };
    assert_eq!(role.as_deref(), Some("reviewer"));
    assert_eq!(spec.as_deref(), Some("STORY-432"));
    assert_eq!(cwd.as_deref(), Some(std::path::Path::new("/tmp/project")));
    assert_eq!(permission_mode.as_deref(), Some("acceptEdits"));
    assert!(!no_context);
    assert!(!show_context);
    assert_eq!(prompt.as_deref(), Some("review STORY-432"));
    assert!(!no_prompt);
    assert!(no_default_flags);
    assert_eq!(extra_flags, vec!["--verbose"]);
}

/// BUG-408: `--show-context` must be a PURE PREVIEW. `prepare_agent_launch_dry`
/// builds the plan WITHOUT `session_start`, so no lease is written, the working
/// directory stays the project root (no worktree is created), and `lease_id`
/// is `None`. This is the core regression guard for the bug where
/// `--show-context` created a worktree + lease and flipped the spec to
// InProgress. trace:BUG-408 | ai:claude
#[test]
fn dry_launch_plan_creates_no_lease_or_worktree() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let plan = prepare_agent_launch_dry(
        root,
        Some("implementer".into()),
        Some("STORY-9".into()),
        "claude",
        None,
    )
    .unwrap();
    assert_eq!(plan.current_spec.as_deref(), Some("STORY-9"));
    assert_eq!(plan.role.as_deref(), Some("implementer"));
    assert_eq!(
        plan.launch_cwd, root,
        "dry preview reports the project root, not a (nonexistent) worktree"
    );
    assert!(plan.lease_id.is_none(), "dry plan holds no lease");
    assert!(
        list_leases(root).is_empty(),
        "dry preview must not write a session lease"
    );
}

#[test]
fn parses_agent_new_codex_flags() {
    let cli = Cli::try_parse_from([
        "aida",
        "agent",
        "new",
        "codex",
        "--role",
        "implementer",
        "--spec",
        "STORY-433",
        "--cwd",
        "/tmp/project",
        "--bypass-sandbox",
        "--no-prompt",
        "--extra-flag",
        "--ask-for-approval=never",
    ])
    .unwrap();
    let Command::Agent(AgentCommand::New {
        command:
            Some(AgentNewCommand::Codex {
                role,
                spec,
                cwd,
                bypass_sandbox,
                no_context,
                show_context,
                no_prompt,
                extra_flags,
                ..
            }),
    }) = cli.command
    else {
        panic!("expected agent new codex command");
    };
    assert_eq!(role.as_deref(), Some("implementer"));
    assert_eq!(spec.as_deref(), Some("STORY-433"));
    assert_eq!(cwd.as_deref(), Some(std::path::Path::new("/tmp/project")));
    assert!(bypass_sandbox);
    assert!(!no_context);
    assert!(!show_context);
    assert!(no_prompt);
    assert_eq!(extra_flags, vec!["--ask-for-approval=never"]);
}

#[test]
fn parses_agent_new_antigravity_flags() {
    let cli = Cli::try_parse_from([
        "aida",
        "agent",
        "new",
        "antigravity",
        "--role",
        "implementer",
        "--spec",
        "STORY-434",
        "--cwd",
        "/tmp/project",
        "--bypass-sandbox",
        "--extra-flag",
        "--model",
        "--extra-flag",
        "fast",
    ])
    .unwrap();
    let Command::Agent(AgentCommand::New {
        command:
            Some(AgentNewCommand::Antigravity {
                role,
                spec,
                cwd,
                bypass_sandbox,
                no_context,
                show_context,
                extra_flags,
                ..
            }),
    }) = cli.command
    else {
        panic!("expected agent new antigravity command");
    };
    assert_eq!(role.as_deref(), Some("implementer"));
    assert_eq!(spec.as_deref(), Some("STORY-434"));
    assert_eq!(cwd.as_deref(), Some(std::path::Path::new("/tmp/project")));
    assert!(bypass_sandbox);
    assert!(!no_context);
    assert!(!show_context);
    assert_eq!(extra_flags, vec!["--model", "fast"]);
}

#[test]
fn agent_default_flags_merge_user_base_project_override_and_extra() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        home.join(".aida/agents.toml"),
        "[agents.codex]\ndefault_flags = [\"--user-codex\"]\n\
             [agents.antigravity]\ndefault_flags = [\"--user-agy\"]\n",
    )
    .unwrap();
    std::fs::write(
        project.join(".aida/agents.toml"),
        "[agents.codex]\ndefault_flags = [\"--project-codex\"]\n",
    )
    .unwrap();
    let _home_guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", &home);

    let mut codex = AgentLaunchConfig {
        agent_type: "codex",
        binary: "codex",
        default_args: vec!["--built-in".to_string()],
        prompt_style: AgentPromptStyle::Positional,
    };
    apply_agent_default_flags(
        &mut codex,
        &project,
        AgentDefaultFlagOptions::new(true, vec!["--extra".to_string()]),
        false,
    )
    .unwrap();
    assert_eq!(
        codex.default_args,
        vec!["--built-in", "--project-codex", "--extra"]
    );

    let mut antigravity = AgentLaunchConfig {
        agent_type: "antigravity",
        binary: "agy",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Flag("--prompt-interactive"),
    };
    apply_agent_default_flags(
        &mut antigravity,
        &project,
        AgentDefaultFlagOptions::new(true, Vec::new()),
        false,
    )
    .unwrap();
    assert_eq!(antigravity.default_args, vec!["--user-agy"]);

    let mut claude = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: vec!["--permission-mode".to_string(), "acceptEdits".to_string()],
        prompt_style: AgentPromptStyle::Positional,
    };
    apply_agent_default_flags(
        &mut claude,
        &project,
        AgentDefaultFlagOptions::new(false, vec!["--one-shot".to_string()]),
        true,
    )
    .unwrap();
    assert_eq!(
        claude.default_args,
        vec!["--permission-mode", "acceptEdits", "--one-shot"]
    );
}

/// STORY-495: each tool maps to its own bypass flag(s).
// trace:STORY-495 | ai:claude
#[test]
fn tool_bypass_flags_per_agent() {
    assert_eq!(
        tool_bypass_flags("claude"),
        vec!["--permission-mode", "bypassPermissions"]
    );
    assert_eq!(
        tool_bypass_flags("codex"),
        vec!["--dangerously-bypass-approvals-and-sandbox"]
    );
    assert_eq!(
        tool_bypass_flags("antigravity"),
        vec!["--dangerously-skip-permissions"]
    );
    assert!(tool_bypass_flags("unknown").is_empty());
}

#[test]
fn tool_contained_flags_for_claude_include_strict_settings() {
    let flags = tool_contained_flags("claude");
    assert_eq!(flags[0], "--permission-mode");
    assert_eq!(flags[1], "dontAsk");
    assert!(flags.contains(&"--setting-sources".to_string()));
    assert!(flags.contains(&"project".to_string()));
    let settings_pos = flags
        .iter()
        .position(|flag| flag == "--settings")
        .expect("contained flags include inline settings");
    let settings: serde_json::Value =
        serde_json::from_str(flags.get(settings_pos + 1).unwrap()).unwrap();
    assert_eq!(settings["sandbox"]["enabled"], true);
    assert_eq!(settings["sandbox"]["failIfUnavailable"], true);
    assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], false);
    assert!(settings["permissions"]["deny"]
        .as_array()
        .unwrap()
        .contains(&serde_json::Value::String(
            "Bash(git push --force *)".into()
        )));
    assert!(tool_contained_flags("codex").is_empty());
}

/// STORY-495: the `[agents] bypass` knob is off when no agents.toml
// declares it; project overrides the user base. trace:STORY-495 | ai:claude
#[test]
fn agents_bypass_knob_user_base_and_project_override() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    let _home_guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", &home);

    // Neither file present → off (faithful default).
    assert!(!load_agents_bypass(&project).unwrap());

    // User base on, no project file → on.
    std::fs::write(home.join(".aida/agents.toml"), "[agents]\nbypass = true\n").unwrap();
    assert!(load_agents_bypass(&project).unwrap());

    // Project explicitly off → overrides user base on.
    std::fs::write(
        project.join(".aida/agents.toml"),
        "[agents]\nbypass = false\n",
    )
    .unwrap();
    assert!(!load_agents_bypass(&project).unwrap());

    // `[agents] bypass` coexists with `[agents.<tool>] default_flags` in
    // the same file without breaking the default_flags loader.
    std::fs::write(
        project.join(".aida/agents.toml"),
        "[agents]\nbypass = true\n\n[agents.codex]\ndefault_flags = [\"--foo\"]\n",
    )
    .unwrap();
    assert!(load_agents_bypass(&project).unwrap());
    let mut flags = Vec::new();
    merge_agent_flags_from_file(&mut flags, &project.join(".aida/agents.toml"), "codex").unwrap();
    assert_eq!(flags, vec!["--foo"]);
}

#[test]
fn agents_contained_knob_user_base_and_project_override() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    let _home_guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", &home);

    assert!(!load_agents_contained(&project).unwrap());
    std::fs::write(
        home.join(".aida/agents.toml"),
        "[agents]\ncontained = true\n",
    )
    .unwrap();
    assert!(load_agents_contained(&project).unwrap());
    std::fs::write(
        project.join(".aida/agents.toml"),
        "[agents]\ncontained = false\n",
    )
    .unwrap();
    assert!(!load_agents_contained(&project).unwrap());
}

/// TASK-798: `[contained] enable` in the project config.toml is an alias of
/// `[agents] contained`, and wins last so a config migrated to the unified
/// `[contained]` block takes effect over the legacy `[agents] contained`.
// trace:TASK-798 | ai:claude
#[test]
fn contained_enable_alias_in_config_toml() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    let _home_guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", &home);

    // Alias alone enables the posture (no [agents] contained anywhere).
    assert!(!load_agents_contained(&project).unwrap());
    std::fs::write(
        project.join(".aida/config.toml"),
        "[contained]\nenable = true\n",
    )
    .unwrap();
    assert!(load_agents_contained(&project).unwrap());

    // Last-wins: the unified [contained] enable overrides a legacy
    // [agents] contained = false (migration takes effect).
    std::fs::write(
        project.join(".aida/agents.toml"),
        "[agents]\ncontained = false\n",
    )
    .unwrap();
    assert!(load_agents_contained(&project).unwrap());

    // And [contained] enable = false turns it back off.
    std::fs::write(
        project.join(".aida/config.toml"),
        "[contained]\nenable = false\n",
    )
    .unwrap();
    assert!(!load_agents_contained(&project).unwrap());
}

/// TASK-698: the first-machine-setup writer emits a valid agents.toml whose
/// `[agents] bypass` value round-trips back through the resolver, for both
/// the native (false) and bypass (true) postures, and creates ~/.aida/ if
// it is missing. trace:TASK-698 | ai:claude
#[test]
fn agent_posture_writer_round_trips_both_postures() {
    let tmp = TempDir::new().unwrap();

    // Bypass posture — parent dir does not exist yet; writer creates it.
    let bypass_path = tmp.path().join("home-bypass/.aida/agents.toml");
    assert!(!bypass_path.parent().unwrap().exists());
    write_global_agents_posture(&bypass_path, true).unwrap();
    assert!(bypass_path.exists());
    toml::from_str::<toml::Value>(&std::fs::read_to_string(&bypass_path).unwrap())
        .expect("written agents.toml must be valid TOML");
    assert_eq!(
        read_agents_bypass_from_file(&bypass_path).unwrap(),
        Some(true)
    );

    // Native posture — explicit `bypass = false` recorded (idempotent: a
    // future init sees the file and never re-prompts).
    let native_path = tmp.path().join("home-native/.aida/agents.toml");
    write_global_agents_posture(&native_path, false).unwrap();
    assert_eq!(
        read_agents_bypass_from_file(&native_path).unwrap(),
        Some(false)
    );
}

/// TASK-698: the posture prompt is idempotent and TTY-gated — when
/// ~/.aida/agents.toml already exists it is left byte-for-byte untouched,
/// and a non-interactive (no-TTY) init writes nothing. The test harness has
/// no TTY, so `maybe_prompt_agent_posture` must never create or mutate the
// file here. trace:TASK-698 | ai:claude
#[test]
fn agent_posture_prompt_is_idempotent_and_tty_gated() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    let _home_guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", &home);

    // Pre-existing config must be preserved verbatim (idempotent guard).
    let path = home.join(".aida/agents.toml");
    let original = "[agents]\nbypass = true\n# user-authored\n";
    std::fs::write(&path, original).unwrap();
    maybe_prompt_agent_posture().unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);

    // Absent config under no TTY → writes nothing (native default stays).
    std::fs::remove_file(&path).unwrap();
    maybe_prompt_agent_posture().unwrap();
    assert!(!path.exists());
}

/// STORY-495: with the knob on and no explicit posture / per-tool flags,
// the tool's bypass flag is injected. trace:STORY-495 | ai:claude
#[test]
fn apply_agent_default_flags_knob_injects_when_native() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/agents.toml"),
        "[agents]\nbypass = true\n",
    )
    .unwrap();
    let _home_guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", &home);

    let mut claude = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
    };
    apply_agent_default_flags(
        &mut claude,
        &project,
        AgentDefaultFlagOptions::new(true, Vec::new()),
        /* explicit_permission */ false,
    )
    .unwrap();
    assert_eq!(
        claude.default_args,
        vec!["--permission-mode", "bypassPermissions"]
    );
}

#[test]
fn apply_agent_default_flags_contained_injects_when_native() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/agents.toml"),
        "[agents]\ncontained = true\n",
    )
    .unwrap();
    let _home_guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", &home);

    let mut claude = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
    };
    apply_agent_default_flags(
        &mut claude,
        &project,
        AgentDefaultFlagOptions::new(true, Vec::new()),
        false,
    )
    .unwrap();
    assert_eq!(claude.default_args[0], "--permission-mode");
    assert_eq!(claude.default_args[1], "dontAsk");
    assert!(claude.default_args.contains(&"--settings".to_string()));
}

#[test]
fn apply_agent_default_flags_rejects_bypass_and_contained() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/agents.toml"),
        "[agents]\nbypass = true\ncontained = true\n",
    )
    .unwrap();
    let _home_guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", &home);

    let mut claude = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
    };
    let err = apply_agent_default_flags(
        &mut claude,
        &project,
        AgentDefaultFlagOptions::new(true, Vec::new()),
        false,
    )
    .unwrap_err();
    assert!(format!("{err:?}").contains("mutually exclusive"));
}

/// STORY-495: an explicit posture (e.g. `--permission-mode`) skips the
// knob even when it is on. trace:STORY-495 | ai:claude
#[test]
fn apply_agent_default_flags_explicit_skips_knob() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/agents.toml"),
        "[agents]\nbypass = true\n",
    )
    .unwrap();
    let _home_guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", &home);

    let mut claude = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: vec!["--permission-mode".to_string(), "plan".to_string()],
        prompt_style: AgentPromptStyle::Positional,
    };
    apply_agent_default_flags(
        &mut claude,
        &project,
        AgentDefaultFlagOptions::new(true, Vec::new()),
        /* explicit_permission */ true,
    )
    .unwrap();
    // No extra bypass flag appended — the explicit posture stands.
    assert_eq!(claude.default_args, vec!["--permission-mode", "plan"]);
}

/// STORY-495: per-tool `default_flags` (TASK-557) override the uniform
/// knob — the bypass flag is NOT additionally injected.
// trace:STORY-495 | ai:claude
#[test]
fn apply_agent_default_flags_per_tool_flags_override_knob() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/agents.toml"),
        "[agents]\nbypass = true\n\n[agents.codex]\ndefault_flags = [\"--my-flag\"]\n",
    )
    .unwrap();
    let _home_guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", &home);

    let mut codex = AgentLaunchConfig {
        agent_type: "codex",
        binary: "codex",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
    };
    apply_agent_default_flags(
        &mut codex,
        &project,
        AgentDefaultFlagOptions::new(true, Vec::new()),
        /* explicit_permission */ false,
    )
    .unwrap();
    // Only the per-tool flag — no `--dangerously-bypass-...` injected.
    assert_eq!(codex.default_args, vec!["--my-flag"]);
}

/// STORY-495: `--no-default-flags` skips agents.toml entirely, so the
/// knob is never read → faithful native (empty argv).
// trace:STORY-495 | ai:claude
#[test]
fn apply_agent_default_flags_no_default_flags_is_native() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/agents.toml"),
        "[agents]\nbypass = true\n",
    )
    .unwrap();
    let _home_guard = crate::test_env::EnvVarGuard::set("AIDA_HOME", &home);

    let mut claude = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
    };
    apply_agent_default_flags(
        &mut claude,
        &project,
        AgentDefaultFlagOptions::new(/* use_config_defaults */ false, Vec::new()),
        /* explicit_permission */ false,
    )
    .unwrap();
    assert!(claude.default_args.is_empty());
}

#[test]
fn agent_initial_prompt_args_map_by_agent_and_respect_opt_out() {
    let tmp = TempDir::new().unwrap();
    let plan = AgentLaunchPlan {
        project_root: tmp.path().to_path_buf(),
        launch_cwd: tmp.path().to_path_buf(),
        role: Some("implementer".into()),
        current_spec: Some("TASK-556".into()),
        name: "agent-test".to_string(),
        lease_id: None,
    };
    let claude = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
    };
    let codex = AgentLaunchConfig {
        agent_type: "codex",
        binary: "codex",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
    };
    let antigravity = AgentLaunchConfig {
        agent_type: "antigravity",
        binary: "agy",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Flag("--prompt-interactive"),
    };

    let claude_args = agent_initial_prompt_args(
        &claude,
        &plan,
        &AgentPromptOptions::new(Some("work TASK-556".into()), false),
    );
    assert_eq!(claude_args, vec!["work TASK-556"]);

    let codex_args =
        agent_initial_prompt_args(&codex, &plan, &AgentPromptOptions::new(None, false));
    assert_eq!(codex_args.len(), 1);
    assert!(codex_args[0].contains("cat \"$AIDA_AGENT_CONTEXT_FILE\""));
    assert!(codex_args[0].contains("aida show TASK-556"));
    assert!(codex_args[0].contains("aida brief list --for-agent codex"));
    assert!(codex_args[0].contains("trace:TASK-556"));

    let antigravity_args = agent_initial_prompt_args(
        &antigravity,
        &plan,
        &AgentPromptOptions::new(Some("work TASK-556".into()), false),
    );
    assert_eq!(
        antigravity_args,
        vec!["--prompt-interactive", "work TASK-556"]
    );

    let opted_out = agent_initial_prompt_args(
        &codex,
        &plan,
        &AgentPromptOptions::new(Some("ignored".into()), true),
    );
    assert!(opted_out.is_empty());

    let no_spec_plan = AgentLaunchPlan {
        current_spec: None,
        ..plan
    };
    let no_spec =
        agent_initial_prompt_args(&codex, &no_spec_plan, &AgentPromptOptions::new(None, false));
    assert!(no_spec.is_empty());
}

#[test]
fn parses_agent_new_context_flags() {
    let cli = Cli::try_parse_from([
        "aida",
        "agent",
        "new",
        "codex",
        "--role",
        "advisor",
        "--no-context",
        "--show-context",
    ])
    .unwrap();
    let Command::Agent(AgentCommand::New {
        command:
            Some(AgentNewCommand::Codex {
                no_context,
                show_context,
                ..
            }),
    }) = cli.command
    else {
        panic!("expected agent new codex command");
    };
    assert!(no_context);
    assert!(show_context);
}

// trace:TASK-587 | ai:antigravity
#[test]
fn parses_agent_list_roles_flags() {
    let cli = Cli::try_parse_from(["aida", "agent", "list-roles"]).unwrap();
    let Command::Agent(AgentCommand::ListRoles { json }) = cli.command else {
        panic!("expected agent list-roles command");
    };
    assert!(!json);

    let cli_json = Cli::try_parse_from(["aida", "agent", "list-roles", "--json"]).unwrap();
    let Command::Agent(AgentCommand::ListRoles { json: json_opt }) = cli_json.command else {
        panic!("expected agent list-roles command");
    };
    assert!(json_opt);
}

// trace:TASK-837 | ai:claude — bare `aida agent new` (no agent-type
// subcommand) must parse to `New { command: None }` rather than erroring to
// clap help, so the interactive picker can take over at a TTY.
#[test]
fn parses_bare_agent_new_as_optional_subcommand() {
    let cli = Cli::try_parse_from(["aida", "agent", "new"]).unwrap();
    let Command::Agent(AgentCommand::New { command }) = cli.command else {
        panic!("expected agent new command");
    };
    assert!(
        command.is_none(),
        "bare `agent new` should carry no subcommand"
    );
}

// trace:TASK-837 | ai:claude — the picker's labels and type tokens are the
// pure, testable surface (mirrors how the role picker label builder is
// tested). They must cover exactly the launchable `AgentNewCommand` variants.
#[test]
fn agent_type_picker_choices_cover_all_launchers() {
    let choices = agent_type_picker_choices();
    let labels: Vec<&str> = choices.iter().map(|(l, _)| *l).collect();
    let tokens: Vec<&str> = choices.iter().map(|(_, t)| *t).collect();
    assert_eq!(labels, vec!["Claude", "Codex", "Antigravity"]);
    assert_eq!(tokens, vec!["claude", "codex", "antigravity"]);

    // Every offered token maps to a default-options AgentNewCommand of the
    // matching variant; an unknown token maps to nothing.
    assert!(matches!(
        agent_new_command_for_type("claude"),
        Some(AgentNewCommand::Claude { .. })
    ));
    assert!(matches!(
        agent_new_command_for_type("codex"),
        Some(AgentNewCommand::Codex { .. })
    ));
    assert!(matches!(
        agent_new_command_for_type("antigravity"),
        Some(AgentNewCommand::Antigravity { .. })
    ));
    assert!(agent_new_command_for_type("nope").is_none());
}

// trace:TASK-837 | ai:claude — a token picked interactively must produce the
// same default-options command as the explicit `aida agent new <type>` lane,
// so the two entry points share one launch path.
#[test]
fn picked_type_matches_explicit_subcommand_defaults() {
    let cli = Cli::try_parse_from(["aida", "agent", "new", "claude"]).unwrap();
    let Command::Agent(AgentCommand::New {
        command: Some(explicit),
    }) = cli.command
    else {
        panic!("expected explicit agent new claude command");
    };
    let picked = agent_new_command_for_type("claude").unwrap();
    // Compare via debug repr: both are the Claude variant with all-default fields.
    assert_eq!(format!("{explicit:?}"), format!("{picked:?}"));
}

// trace:TASK-543 | ai:codex
#[test]
fn parses_agent_register_flags() {
    let cli = Cli::try_parse_from([
        "aida",
        "agent",
        "register",
        "12345",
        "--type",
        "codex",
        "--role",
        "implementer",
        "--spec",
        "TASK-543",
        "--name",
        "codex-1",
    ])
    .unwrap();
    let Command::Agent(AgentCommand::Register {
        pid,
        agent_type,
        role,
        spec,
        name,
    }) = cli.command
    else {
        panic!("expected agent register command");
    };
    assert_eq!(pid, 12345);
    assert_eq!(agent_type, "codex");
    assert_eq!(role, "implementer");
    assert_eq!(spec.as_deref(), Some("TASK-543"));
    assert_eq!(name.as_deref(), Some("codex-1"));
}

// trace:TASK-543 | ai:codex
#[test]
fn agent_register_validation_accepts_locked_taxonomy() {
    assert_eq!(validate_registered_agent_type("codex").unwrap(), "codex");
    assert_eq!(
        validate_registered_agent_type("claude-code").unwrap(),
        "claude"
    );
    assert_eq!(
        validate_registered_agent_type("antigravity").unwrap(),
        "antigravity"
    );
    assert_eq!(validate_registered_agent_type("web").unwrap(), "web");
    assert!(validate_registered_agent_type("random").is_err());

    assert_eq!(
        validate_registered_agent_role("Implementer").unwrap(),
        "implementer"
    );
    assert_eq!(
        validate_registered_agent_role("advisor").unwrap(),
        "advisor"
    );
    assert_eq!(
        validate_registered_agent_role("reviewer").unwrap(),
        "reviewer"
    );
    assert_eq!(
        validate_registered_agent_role("integrator").unwrap(),
        "integrator"
    );
    assert!(validate_registered_agent_role("observer").is_err());
}

#[cfg(unix)]
// trace:TASK-543 | ai:codex
#[test]
fn parses_proc_status_uid() {
    let body = "Name:\ttest\nUid:\t1000\t1000\t1000\t1000\n";
    assert_eq!(parse_proc_status_uid(body), Some(1000));
    assert_eq!(parse_proc_status_uid("Name:\ttest\n"), None);
}

#[test]
fn find_aida_project_root_walks_up_from_descendant() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    let nested = root.join("a/b/c");
    std::fs::create_dir_all(&nested).unwrap();

    let found = find_aida_project_root_from(&nested).unwrap();
    assert_eq!(found, root.canonicalize().unwrap());
}

#[test]
fn spawned_agent_registry_entry_is_pid_keyed_and_removable() {
    let tmp = TempDir::new().unwrap();
    let binary = agent_registry::AgentBinaryIdentity::new("0.9.1".into(), "abc123".into());
    let first = agent_registry::register_spawned_agent(
        tmp.path(),
        "claude",
        111,
        Some("implementer".into()),
        None,
        tmp.path().into(),
        Some(&binary),
        None,
    )
    .unwrap();
    let second = agent_registry::register_spawned_agent(
        tmp.path(),
        "claude",
        222,
        Some("implementer".into()),
        None,
        tmp.path().into(),
        Some(&binary),
        None,
    )
    .unwrap();

    assert_eq!(first.id, "claude-111");
    assert_eq!(second.id, "claude-222");
    assert!(agent_registry::remove_agent(tmp.path(), "claude", 111).unwrap());
    assert!(agent_registry::remove_agent(tmp.path(), "claude", 222).unwrap());
    assert!(!agent_registry::remove_agent(tmp.path(), "claude", 222).unwrap());
}

// trace:TASK-543 | ai:codex
#[test]
fn register_existing_agent_entry_is_status_visible_and_pid_keyed() {
    let tmp = TempDir::new().unwrap();
    let entry = agent_registry::register_existing_agent(
        tmp.path(),
        "web",
        std::process::id(),
        "advisor".to_string(),
        Some("TASK-543".to_string()),
        tmp.path().into(),
        Some("browser-advisor".to_string()),
    )
    .unwrap();

    assert_eq!(entry.id, format!("web-{}", std::process::id()));
    assert_eq!(entry.agent_type, "web");
    assert_eq!(entry.role.as_deref(), Some("advisor"));
    assert_eq!(entry.current_spec.as_deref(), Some("TASK-543"));
    assert_eq!(entry.name.as_deref(), Some("browser-advisor"));
    assert_eq!(entry.source, "manual-register");

    let ctx = agent_registry::AgentClassifyContext::new(chrono::Utc::now(), 30, vec![]);
    let views = agent_registry::list_agent_views(tmp.path(), &ctx);
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].status, agent_registry::AgentStatus::Busy);
    let lines = agent_registry::format_agent_status_lines(&views);
    assert!(
        lines[0].contains("browser-advisor"),
        "unexpected status line: {}",
        lines[0]
    );
}

#[cfg(unix)]
#[test]
fn tracked_fake_agent_receives_env_and_registry_is_removed() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    let fake_bin = tmp.path().join("bin");
    std::fs::create_dir_all(&fake_bin).unwrap();
    let fake_agent = fake_bin.join("agent");
    let env_out = tmp.path().join("env.txt");
    let argv_out = tmp.path().join("argv.txt");
    std::fs::write(
        &fake_agent,
        format!(
            "#!/bin/sh\nenv | sort > '{}'\nprintf '%s\\n' \"$@\" > '{}'\n",
            env_out.display(),
            argv_out.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&fake_agent).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_agent, perms).unwrap();
    let config = AgentLaunchConfig {
        agent_type: "codex",
        binary: "codex",
        default_args: vec!["--dangerously-bypass-approvals-and-sandbox".to_string()],
        prompt_style: AgentPromptStyle::Positional,
    };
    let plan = AgentLaunchPlan {
        project_root: project.clone(),
        launch_cwd: project.clone(),
        role: Some("implementer".into()),
        current_spec: Some("STORY-433".into()),
        name: "codex-test".to_string(),
        lease_id: None,
    };

    let prompt_args =
        agent_initial_prompt_args(&config, &plan, &AgentPromptOptions::new(None, false));
    // BUG-423: spawning a just-written temp binary can transiently fail
    // under CI load / parallel-test execution (e.g. ETXTBSY — the file is
    // briefly busy after write+chmod). Retry a few times with a short
    // backoff. This is fixture-only: production launches an INSTALLED
    // binary, never a freshly-written one, so it can't hit this. Safe to
    // retry the whole call — run_tracked_agent registers the agent only
    // AFTER a successful spawn, so a failed attempt leaves no partial
    // registry state. trace:BUG-423 | ai:claude
    let mut spawn_result = run_tracked_agent(&fake_agent, &config, &plan, None, &prompt_args);
    for _ in 0..5 {
        if spawn_result.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        spawn_result = run_tracked_agent(&fake_agent, &config, &plan, None, &prompt_args);
    }
    spawn_result.expect("run_tracked_agent should succeed after retrying transient spawn");

    let env = std::fs::read_to_string(&env_out).unwrap();
    let argv = std::fs::read_to_string(&argv_out).unwrap();
    assert!(env.contains("AIDA_AGENT_TYPE=codex"), "{env}");
    assert!(env.contains("AIDA_SESSION_ROLE=implementer"), "{env}");
    assert!(env.contains("AIDA_SESSION_SCOPE=STORY-433"), "{env}");
    assert!(env.contains("AIDA_PROJECT_ROOT="), "{env}");
    assert!(
        argv.contains("--dangerously-bypass-approvals-and-sandbox"),
        "{argv}"
    );
    assert!(argv.contains("aida show STORY-433"), "{argv}");
    assert!(argv.contains("trace:STORY-433"), "{argv}");
    assert!(agent_registry::list_agent_views(
        &project,
        &agent_registry::AgentClassifyContext::new(chrono::Utc::now(), 30, vec![])
    )
    .is_empty());
    std::env::remove_var("AIDA_TEST_ENV_OUT");
    std::env::remove_var("AIDA_TEST_ARGV_OUT");
}

#[cfg(unix)]
#[test]
fn tracked_fake_antigravity_receives_env_args_and_registry_is_removed() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    let fake_agent = tmp.path().join("agy");
    let env_out = tmp.path().join("env.txt");
    let argv_out = tmp.path().join("argv.txt");
    std::fs::write(
        &fake_agent,
        format!(
            "#!/bin/sh\nenv | sort > '{}'\nprintf '%s\\n' \"$@\" > '{}'\n",
            env_out.display(),
            argv_out.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&fake_agent).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_agent, perms).unwrap();
    let config = AgentLaunchConfig {
        agent_type: "antigravity",
        binary: "agy",
        default_args: vec!["--dangerously-skip-permissions".to_string()],
        prompt_style: AgentPromptStyle::Flag("--prompt-interactive"),
    };
    let plan = AgentLaunchPlan {
        project_root: project.clone(),
        launch_cwd: project.clone(),
        role: Some("implementer".into()),
        current_spec: Some("STORY-434".into()),
        name: "agy-test".to_string(),
        lease_id: None,
    };

    let prompt_args = agent_initial_prompt_args(
        &config,
        &plan,
        &AgentPromptOptions::new(Some("work STORY-434".into()), false),
    );
    // BUG-423: spawning a just-written temp binary can transiently fail
    // under CI load / parallel-test execution (e.g. ETXTBSY — the file is
    // briefly busy after write+chmod). Retry a few times with a short
    // backoff. This is fixture-only: production launches an INSTALLED
    // binary, never a freshly-written one, so it can't hit this. Safe to
    // retry the whole call — run_tracked_agent registers the agent only
    // AFTER a successful spawn, so a failed attempt leaves no partial
    // registry state. trace:BUG-423 | ai:claude
    let mut spawn_result = run_tracked_agent(&fake_agent, &config, &plan, None, &prompt_args);
    for _ in 0..5 {
        if spawn_result.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        spawn_result = run_tracked_agent(&fake_agent, &config, &plan, None, &prompt_args);
    }
    spawn_result.expect("run_tracked_agent should succeed after retrying transient spawn");

    let env = std::fs::read_to_string(&env_out).unwrap();
    let argv = std::fs::read_to_string(&argv_out).unwrap();
    assert!(env.contains("AIDA_AGENT_TYPE=antigravity"), "{env}");
    assert!(env.contains("AIDA_SESSION_ROLE=implementer"), "{env}");
    assert!(env.contains("AIDA_SESSION_SCOPE=STORY-434"), "{env}");
    assert!(env.contains("AIDA_PROJECT_ROOT="), "{env}");
    assert!(argv.contains("--dangerously-skip-permissions"), "{argv}");
    assert!(argv.contains("--prompt-interactive"), "{argv}");
    assert!(argv.contains("work STORY-434"), "{argv}");
    assert!(agent_registry::list_agent_views(
        &project,
        &agent_registry::AgentClassifyContext::new(chrono::Utc::now(), 30, vec![])
    )
    .is_empty());
    std::env::remove_var("AIDA_TEST_ENV_OUT");
    std::env::remove_var("AIDA_TEST_ARGV_OUT");
}

#[test]
fn renders_agent_launch_context_with_role_guidance_and_briefs() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join(".aida/agent-briefs/codex")).unwrap();
    std::fs::write(
            project.join(".aida/agent-briefs/codex/STORY-436-2026-05-24T025247Z.md"),
            "---\nspec_id: STORY-436\nagent: codex\ngenerated_at: 2026-05-24T025247Z\nstatus: pending\n---\n\n## Spec\n\n- Title: Role-context auto-injection\n",
        )
        .unwrap();
    let config = AgentLaunchConfig {
        agent_type: "codex",
        binary: "codex",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
    };
    let plan = AgentLaunchPlan {
        project_root: project.clone(),
        launch_cwd: project,
        role: Some("implementer".into()),
        current_spec: Some("STORY-436".into()),
        name: "codex-test".to_string(),
        lease_id: None,
    };

    let context = render_agent_launch_context(&config, &plan, "token-123").unwrap();

    assert!(
        context.contains("This is a point-in-time spawn snapshot"),
        "{context}"
    );
    assert!(context.contains("- Agent: codex"), "{context}");
    assert!(context.contains("- Role: implementer"), "{context}");
    assert!(context.contains("## Role Guidance"), "{context}");
    assert!(
        context.contains("- **Spec: STORY-436 (your scope for this session)**"),
        "{context}"
    );
    assert!(context.contains("**SCOPE BINDING**"), "{context}");
    assert!(
        context.contains("Do not pick up other specs from this session"),
        "{context}"
    );
    assert!(
        context.contains("STORY-436 — Role-context auto-injection"),
        "{context}"
    );
    // STORY-619: the snapshot is self-describing about the mailbox for any
    // vendor — section header, the empty-inbox line for this agent, and the
    // vendor-neutral poll guidance naming the inbox command.
    assert!(context.contains("## Mailbox"), "{context}");
    assert!(
        context.contains("No unread mailbox messages for `codex-test`"),
        "{context}"
    );
    assert!(context.contains("aida mailbox inbox"), "{context}");
    assert!(context.contains("re-check"), "{context}");
}

/// STORY-718: the integrator is a first-class agent-wired role, so its
/// built-in role guidance carries seat-specific context (the merge cascade +
/// the escalate-never-resolve invariant), not the generic "no stored role
/// file" fallback. Mirrors the advisor/implementer/reviewer arms. Targets
/// `default_role_guidance` directly so a machine's scaffolded
/// `~/.aida/roles/integrator.toml` can't shadow the built-in arm under test.
#[test]
fn role_guidance_for_integrator_is_first_class() {
    let guidance = default_role_guidance("integrator");
    assert!(
        guidance.contains("integrating"),
        "integrator guidance names the seat: {guidance}"
    );
    assert!(
        guidance.contains("squash-merge") && guidance.contains("escalate"),
        "integrator guidance covers the merge cascade + escalation: {guidance}"
    );
    assert!(
        !guidance.contains("No stored role file was found"),
        "integrator must NOT fall through to the unknown-role arm: {guidance}"
    );

    // The unspecified arm now names integrator alongside the other seats.
    let unspecified = default_role_guidance("unspecified");
    assert!(
        unspecified.contains("integrator"),
        "unspecified guidance lists integrator as a candidate seat: {unspecified}"
    );
}

#[test]
fn agent_launch_context_always_includes_mailbox_guidance_when_caught_up() {
    // STORY-619: even with no unread mail, the launch context must name the
    // inbox command + poll cadence so a non-Claude vendor (which has no
    // auto-hook) learns the mailbox exists and to poll it.
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    let config = AgentLaunchConfig {
        agent_type: "antigravity",
        binary: "antigravity",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
    };
    let plan = AgentLaunchPlan {
        project_root: project.clone(),
        launch_cwd: project,
        role: Some("implementer".into()),
        current_spec: None,
        name: "antigravity-test".to_string(),
        lease_id: None,
    };

    let context = render_agent_launch_context(&config, &plan, "token-123").unwrap();

    assert!(context.contains("## Mailbox"), "{context}");
    assert!(context.contains("aida mailbox inbox"), "{context}");
    assert!(context.contains("aida mailbox notice"), "{context}");
    assert!(
        context.contains("Only Claude Code auto-surfaces new mail"),
        "{context}"
    );
    assert!(
        context.contains("No unread mailbox messages for `antigravity-test`"),
        "{context}"
    );
}

#[test]
fn agent_launch_context_without_spec_preserves_open_ended_hint() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    let config = AgentLaunchConfig {
        agent_type: "codex",
        binary: "codex",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
    };
    let plan = AgentLaunchPlan {
        project_root: project.clone(),
        launch_cwd: project,
        role: Some("implementer".into()),
        current_spec: None,
        name: "codex-test".to_string(),
        lease_id: None,
    };

    let context = render_agent_launch_context(&config, &plan, "token-123").unwrap();

    assert!(
        context.contains(
            "No spec was provided at launch; start from the relevant queue head or pending brief."
        ),
        "{context}"
    );
    assert!(!context.contains("SCOPE BINDING"), "{context}");
}

#[cfg(unix)]
#[test]
fn tracked_fake_agent_receives_context_file_env_and_cleans_file() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join(".aida/agents/context")).unwrap();
    let fake_agent = tmp.path().join("agent");
    let env_out = tmp.path().join("env.txt");
    let context_out = tmp.path().join("context-copy.md");
    std::fs::write(
        &fake_agent,
        format!(
            "#!/bin/sh\nenv | sort > '{}'\ncp \"$AIDA_AGENT_CONTEXT_FILE\" '{}'\n",
            env_out.display(),
            context_out.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&fake_agent).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_agent, perms).unwrap();
    let context_path = project
        .join(".aida/agents/context")
        .join("codex-token.context.md");
    std::fs::write(&context_path, "launch context body").unwrap();
    let launch_context = AgentLaunchContext {
        path: context_path.clone(),
        token: "token".into(),
    };
    let config = AgentLaunchConfig {
        agent_type: "codex",
        binary: "codex",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
    };
    let plan = AgentLaunchPlan {
        project_root: project.clone(),
        launch_cwd: project.clone(),
        role: Some("advisor".into()),
        current_spec: None,
        name: "codex-test".to_string(),
        lease_id: None,
    };

    let prompt_args =
        agent_initial_prompt_args(&config, &plan, &AgentPromptOptions::new(None, false));
    // BUG-423: retry the transient temp-binary spawn (see the other
    // tracked_fake_* tests). Fixture-only; no partial state on failure.
    // trace:BUG-423 | ai:claude
    let mut spawn_result = run_tracked_agent(
        &fake_agent,
        &config,
        &plan,
        Some(&launch_context),
        &prompt_args,
    );
    for _ in 0..5 {
        if spawn_result.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        spawn_result = run_tracked_agent(
            &fake_agent,
            &config,
            &plan,
            Some(&launch_context),
            &prompt_args,
        );
    }
    spawn_result.expect("run_tracked_agent should succeed after retrying transient spawn");

    let env = std::fs::read_to_string(&env_out).unwrap();
    let copied = std::fs::read_to_string(&context_out).unwrap();
    assert!(env.contains("AIDA_AGENT_CONTEXT_FILE="), "{env}");
    assert!(env.contains("AIDA_AGENT_REGISTRY_TOKEN=token"), "{env}");
    assert_eq!(copied, "launch context body");
    assert!(!context_path.exists());
}
