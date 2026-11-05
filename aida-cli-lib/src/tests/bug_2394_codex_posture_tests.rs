//! BUG-2394: Codex plan-only read-only posture, `[agents] mcp` enforcement for
//! Codex, and the vendor gate on the Claude folder-trust doctor check.
// trace:BUG-2394 | ai:claude

use super::*;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn sandbox_values(args: &[String]) -> Vec<String> {
    args.windows(2)
        .filter(|w| w[0] == "--sandbox" || w[0] == "-s")
        .map(|w| w[1].clone())
        .collect()
}

#[test]
fn bug_2394_read_only_replaces_bypass_and_every_earlier_sandbox() {
    let native = s(&[
        "--sandbox",
        "workspace-write",
        "--ask-for-approval",
        "never",
        "-s",
        "danger-full-access",
        "--sandbox=workspace-write",
        "--dangerously-bypass-approvals-and-sandbox",
        "prompt",
    ]);
    let out = codex_read_only_args(native);
    assert_eq!(
        sandbox_values(&out),
        vec!["read-only".to_string()],
        "{out:?}"
    );
    assert!(!out.iter().any(|a| a.starts_with("--sandbox=")), "{out:?}");
    assert!(!out
        .iter()
        .any(|a| a == "--dangerously-bypass-approvals-and-sandbox"));
    // Approval posture is kept so a write is refused rather than escalated.
    assert!(out
        .windows(2)
        .any(|w| w[0] == "--ask-for-approval" && w[1] == "never"));
    assert_eq!(out.last().map(String::as_str), Some("prompt"));
}

#[test]
fn bug_2394_plan_mode_is_the_only_mode_that_selects_read_only() {
    assert!(codex_plan_read_only(Some("plan")));
    for other in [
        None,
        Some("bypassPermissions"),
        Some("dontAsk"),
        Some("default"),
    ] {
        assert!(!codex_plan_read_only(other), "{other:?}");
    }
}

#[test]
fn bug_2394_interactive_args_read_only_wins_over_bypass() {
    let plan = codex_interactive_session_args("/aida-pickup X", true, true, Some("m"));
    assert_eq!(
        sandbox_values(&plan),
        vec!["read-only".to_string()],
        "{plan:?}"
    );
    assert!(!plan
        .iter()
        .any(|a| a == "--dangerously-bypass-approvals-and-sandbox"));
    assert!(plan.windows(2).any(|w| w[0] == "--model" && w[1] == "m"));

    let bypass = codex_interactive_session_args("/aida-pickup X", true, false, None);
    assert!(bypass
        .iter()
        .any(|a| a == "--dangerously-bypass-approvals-and-sandbox"));
    assert!(sandbox_values(&bypass).is_empty());

    let native = codex_interactive_session_args("/aida-pickup X", false, false, None);
    assert_eq!(native, codex_session_args("/aida-pickup X", false, None));
}

#[test]
fn bug_2394_queue_work_codex_preview_maps_plan_to_read_only() {
    use crate::queue_cmd::deferred_interactive_launch_hint;
    let plan = deferred_interactive_launch_hint(
        HeadlessVendor::Codex,
        "/aida-pickup BUG-1",
        Some("plan"),
        false,
        None,
    );
    assert!(plan.contains("--sandbox read-only"), "{plan}");
    assert!(!plan.contains("dangerously"), "{plan}");
    let bypass = deferred_interactive_launch_hint(
        HeadlessVendor::Codex,
        "/aida-pickup BUG-1",
        Some("bypassPermissions"),
        false,
        None,
    );
    assert!(
        bypass.contains("--dangerously-bypass-approvals-and-sandbox"),
        "{bypass}"
    );
    assert!(!bypass.contains("read-only"), "{bypass}");
    // Claude keeps its own plan permission mode.
    let claude = deferred_interactive_launch_hint(
        HeadlessVendor::Claude,
        "/aida-pickup BUG-1",
        Some("plan"),
        false,
        None,
    );
    assert!(claude.contains("--permission-mode plan"), "{claude}");
    assert!(!claude.contains("--sandbox"), "{claude}");
}

#[test]
fn bug_2394_codex_mcp_off_disables_every_configured_server() {
    let servers = s(&["aida", "node_repl", "my-tool"]);
    let flags = codex_mcp_flags(AgentMcpSurface::Off, &servers).unwrap();
    assert_eq!(
        flags,
        s(&[
            "-c",
            "mcp_servers.aida.enabled=false",
            "-c",
            "mcp_servers.node_repl.enabled=false",
            "-c",
            "mcp_servers.my-tool.enabled=false",
        ])
    );
    assert!(codex_mcp_flags(AgentMcpSurface::Off, &[])
        .unwrap()
        .is_empty());
}

#[test]
fn bug_2394_codex_mcp_aida_keeps_only_aida() {
    let flags = codex_mcp_flags(AgentMcpSurface::Aida, &s(&["aida", "node_repl"])).unwrap();
    assert_eq!(
        flags,
        s(&[
            "-c",
            "mcp_servers.node_repl.enabled=false",
            "-c",
            "mcp_servers.aida.enabled=true",
        ])
    );
    // Not configured: inject AIDA's vouched definition, nothing else.
    let flags = codex_mcp_flags(AgentMcpSurface::Aida, &s(&["node_repl"])).unwrap();
    assert_eq!(
        flags,
        s(&[
            "-c",
            "mcp_servers.node_repl.enabled=false",
            "-c",
            "mcp_servers.aida.command=\"aida\"",
            "-c",
            "mcp_servers.aida.args=[\"mcp-serve\"]",
        ])
    );
}

#[test]
fn bug_2394_codex_mcp_native_is_untouched_and_unaddressable_names_fail() {
    assert!(codex_mcp_flags(AgentMcpSurface::Native, &s(&["a.b"]))
        .unwrap()
        .is_empty());
    let err = codex_mcp_flags(AgentMcpSurface::Off, &s(&["a.b"])).unwrap_err();
    assert!(
        format!("{err}").contains("cannot be enforced for codex"),
        "{err}"
    );
}

#[test]
fn bug_2394_configured_servers_read_names_only_never_credentials() {
    let tmp = tempfile::TempDir::new().unwrap();
    let codex_home = tmp.path().join("codex-home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&codex_home).unwrap();
    std::fs::create_dir_all(project.join(".codex")).unwrap();
    // The project is trusted, so Codex loads (and AIDA reads) its config.
    std::fs::write(
        codex_home.join("config.toml"),
        "model = \"x\"\n".to_string()
            + &trusted_project_toml(&project)
            + "[mcp_servers.aida]\ncommand = \"aida\"\nargs = [\"mcp-serve\"]\n\
         [mcp_servers.secret_srv]\ncommand = \"srv\"\n[mcp_servers.secret_srv.env]\n\
         API_TOKEN = \"SUPER-SECRET-VALUE\"\n",
    )
    .unwrap();
    std::fs::write(
        project.join(".codex").join("config.toml"),
        "[mcp_servers.project_srv]\ncommand = \"p\"\n[mcp_servers.aida]\ncommand = \"evil\"\n",
    )
    .unwrap();
    let mut home = crate::test_env::EnvVarGuard::set("CODEX_HOME", &codex_home);
    let servers = codex_configured_mcp_servers(&project);
    assert_eq!(servers, s(&["aida", "project_srv", "secret_srv"]));
    let flags = codex_mcp_flags(AgentMcpSurface::Off, &servers).unwrap();
    let joined = flags.join(" ");
    assert!(!joined.contains("SUPER-SECRET-VALUE"), "{joined}");
    assert!(!joined.contains("API_TOKEN"), "{joined}");
    assert!(!joined.contains("evil"), "{joined}");

    // A missing CODEX_HOME config contributes nothing, and without it no
    // project is trusted, so the project file is not read either.
    home.reset(tmp.path().join("absent"));
    assert!(codex_configured_mcp_servers(&project).is_empty());
}

#[test]
fn bug_2394_claude_trust_check_is_gated_on_claude_being_a_launch_vendor() {
    use crate::doctor_cmd::claude_launch_active;
    // An explicit enabled list decides.
    assert!(!claude_launch_active(Some(false)));
    assert!(claude_launch_active(Some(true)));
    // No enabled list: every vendor is enabled, whatever `[agents] vendor` says.
    assert!(claude_launch_active(None));
}

/// Env for composing a headless argv: no agent-cmd override, no inherited
/// containment, no OS wrapper.
fn headless_env() -> crate::test_env::EnvVarsGuard {
    crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_AGENT_CMD", None),
        ("AIDA_CONTAINED", None),
        ("AIDA_OS_WRAP", None),
    ])
}

// Ruling 2026-11-02 10:59: a headless Codex plan lane (`--plan-only
// --no-human`, the drain's `--plan-phase`) must write
// docs/plans/<date>-<slug>.md for `aida plan promote`, and Claude's headless
// plan lane keeps its write posture, so the Codex one does too. Only the
// interactive `--plan-only` lane is read-only.
#[test]
fn bug_2394_headless_codex_plan_lane_keeps_write_posture() {
    let _env = headless_env();
    let compose = |prompt| {
        compose_headless_command_for(
            crate::seat_clone::launcher_main().as_deref(),
            HeadlessVendor::Codex,
            prompt,
            "sid",
            false,
            None,
            None,
            false,
        )
        .unwrap()
        .1
    };
    let plan = compose("/aida-plan BUG-1");
    assert!(
        !sandbox_values(&plan).iter().any(|v| v == "read-only"),
        "{plan:?}"
    );
    assert_eq!(
        sandbox_values(&plan),
        sandbox_values(&compose("/aida-pickup BUG-1"))
    );

    // The drain's `--plan-phase` child is built the same way: no read-only.
    let cmd = build_exec_vendor_headless_command_for(
        crate::seat_clone::launcher_main().as_deref(),
        HeadlessVendor::Codex,
        "/aida-plan BUG-1",
        "sid",
        false,
        None,
        None,
        "implementer",
        "",
        false,
    )
    .unwrap();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        !sandbox_values(&args).iter().any(|v| v == "read-only"),
        "{args:?}"
    );
}

/// `[projects."<project>"] trust_level = "trusted"`, serialized by the TOML
/// writer so any path (quotes, backslashes) is escaped correctly.
fn trusted_project_toml(project: &Path) -> String {
    let mut entry = toml::Table::new();
    entry.insert("trust_level".into(), "trusted".into());
    let mut projects = toml::Table::new();
    projects.insert(project.to_string_lossy().into_owned(), entry.into());
    let mut root = toml::Table::new();
    root.insert("projects".into(), projects.into());
    toml::to_string(&root).unwrap()
}

/// A temp CODEX_HOME whose config declares an `aida` server, plus a project
/// whose `.codex/config.toml` declares `project_only`; `trusted` marks that
/// project trusted in the CODEX_HOME config.
fn codex_home_with(trusted: bool) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let codex_home = tmp.path().join("codex-home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&codex_home).unwrap();
    std::fs::create_dir_all(project.join(".codex")).unwrap();
    let trust = if trusted {
        trusted_project_toml(&project)
    } else {
        String::new()
    };
    std::fs::write(
        codex_home.join("config.toml"),
        trust + "[mcp_servers.aida]\ncommand = \"aida\"\n",
    )
    .unwrap();
    std::fs::write(
        project.join(".codex").join("config.toml"),
        "[mcp_servers.project_only]\ncommand = \"p\"\n",
    )
    .unwrap();
    (tmp, codex_home, project)
}

// A server named only in an untrusted project's `.codex/config.toml` is never
// loaded by Codex, so no override (which would create a transport-less entry)
// is emitted for it.
#[test]
fn bug_2394_untrusted_project_only_server_gets_no_override() {
    let (_tmp, codex_home, project) = codex_home_with(false);
    let _home = crate::test_env::EnvVarGuard::set("CODEX_HOME", &codex_home);
    let servers = codex_configured_mcp_servers(&project);
    assert_eq!(servers, s(&["aida"]));
    for surface in [AgentMcpSurface::Off, AgentMcpSurface::Aida] {
        let flags = codex_mcp_flags(surface, &servers).unwrap().join(" ");
        assert!(!flags.contains("project_only"), "{surface:?}: {flags}");
    }
}

#[test]
fn bug_2394_trusted_project_server_is_overridden() {
    let (_tmp, codex_home, project) = codex_home_with(true);
    let _home = crate::test_env::EnvVarGuard::set("CODEX_HOME", &codex_home);
    let servers = codex_configured_mcp_servers(&project);
    assert_eq!(servers, s(&["aida", "project_only"]));
    let flags = codex_mcp_flags(AgentMcpSurface::Off, &servers).unwrap();
    assert!(flags.contains(&"mcp_servers.project_only.enabled=false".to_string()));
}

// Ruling: an unset `[agents] mcp` keeps the aida server on a Codex seat; only
// an explicit `off` drops it. Claude's default is unchanged.
#[test]
fn bug_2394_codex_mcp_unset_means_aida_and_off_must_be_explicit() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let home = tempfile::TempDir::new().unwrap();
    let project = tempfile::TempDir::new().unwrap();
    let home_path = home.path().to_str().unwrap();
    let _env =
        crate::test_env::EnvVarsGuard::apply(&[("HOME", Some(home_path)), ("AIDA_HOME", None)]);
    assert_eq!(
        crate::agent_config::load_codex_agents_mcp(project.path()).unwrap(),
        AgentMcpSurface::Aida
    );
    assert_eq!(
        crate::agent_config::load_agents_mcp(project.path()).unwrap(),
        AgentMcpSurface::Off
    );
    // Unset → aida → the aida server is kept (or injected), never disabled.
    let flags = codex_mcp_flags(AgentMcpSurface::Aida, &s(&["aida", "other"])).unwrap();
    assert!(!flags.contains(&"mcp_servers.aida.enabled=false".to_string()));
    assert!(flags.contains(&"mcp_servers.aida.enabled=true".to_string()));

    std::fs::create_dir_all(project.path().join(".aida")).unwrap();
    std::fs::write(
        project.path().join(".aida/agents.toml"),
        "[agents]\nmcp = \"off\"\n",
    )
    .unwrap();
    assert_eq!(
        crate::agent_config::load_codex_agents_mcp(project.path()).unwrap(),
        AgentMcpSurface::Off
    );
}

// Ruling 2026-11-02 10:59 item 4: Codex trusts the directory it runs in, so the
// overrides are re-keyed on the seat once the launch plan names it. Here the
// launcher's checkout is untrusted and the seat is trusted: the seat's
// project-only server gains its override; the reverse drops it.
#[test]
fn bug_2394_codex_mcp_trust_is_keyed_on_the_seat_not_the_launcher_root() {
    let (tmp, codex_home, seat) = codex_home_with(true);
    let launcher_root = tmp.path().join("launcher");
    std::fs::create_dir_all(launcher_root.join(".aida")).unwrap();
    std::fs::write(
        launcher_root.join(".aida/agents.toml"),
        "[agents]\nmcp = \"off\"\n",
    )
    .unwrap();
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("CODEX_HOME", codex_home.to_str().unwrap()),
        ("AIDA_HOME", tmp.path().join("aida-home").to_str().unwrap()),
    ]);
    let (_, at_root) = codex_mcp_launch_flags(&launcher_root, &launcher_root).unwrap();
    let (_, at_seat) = codex_mcp_launch_flags(&launcher_root, &seat).unwrap();
    assert_eq!(at_root, s(&["-c", "mcp_servers.aida.enabled=false"]));
    assert_eq!(
        at_seat,
        s(&[
            "-c",
            "mcp_servers.aida.enabled=false",
            "-c",
            "mcp_servers.project_only.enabled=false",
        ])
    );

    let config = |args: Vec<String>| crate::AgentLaunchConfig {
        agent_type: "codex",
        binary: "codex",
        default_args: args,
        prompt_style: crate::AgentPromptStyle::Positional,
        contained: false,
    };
    let mut launch = config([s(&["--no-daemon"]), at_root.clone(), s(&["--x"])].concat());
    crate::rescope_codex_mcp_flags(&mut launch, &launcher_root, &seat, true).unwrap();
    assert_eq!(
        launch.default_args,
        [s(&["--no-daemon"]), at_seat.clone(), s(&["--x"])].concat()
    );

    // `--no-default-flags`: no overrides were applied, none are added.
    let mut bare = config(s(&["--no-daemon"]));
    crate::rescope_codex_mcp_flags(&mut bare, &launcher_root, &seat, false).unwrap();
    assert_eq!(bare.default_args, s(&["--no-daemon"]));

    // Claude launches are never touched.
    let mut claude = config(at_root.clone());
    claude.agent_type = "claude";
    crate::rescope_codex_mcp_flags(&mut claude, &launcher_root, &seat, true).unwrap();
    assert_eq!(claude.default_args, at_root);
}
