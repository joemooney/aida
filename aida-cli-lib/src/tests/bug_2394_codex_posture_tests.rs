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

// `aida queue work X --plan-only --no-human --vendor codex`: the standalone
// implementer and the drain lane both compose through
// `compose_headless_command_for` with `read_only = plan_only`.
#[test]
fn bug_2394_headless_no_human_plan_only_codex_exec_is_read_only() {
    let _env = headless_env();
    let (_, args) = compose_headless_command_for(
        HeadlessVendor::Codex,
        "/aida-plan BUG-1",
        "sid",
        false,
        Some("m"),
        None,
        false,
        true,
    )
    .unwrap();
    assert_eq!(
        sandbox_values(&args),
        vec!["read-only".to_string()],
        "{args:?}"
    );
    assert!(
        !args
            .iter()
            .any(|a| a == "--dangerously-bypass-approvals-and-sandbox"),
        "{args:?}"
    );
    assert!(args.iter().any(|a| a == "exec"), "{args:?}");
    assert!(args.windows(2).any(|w| w[0] == "--model" && w[1] == "m"));

    // The ordinary (non-plan) headless lane keeps its historical posture.
    let (_, write) = compose_headless_command_for(
        HeadlessVendor::Codex,
        "/aida-pickup BUG-1",
        "sid",
        false,
        None,
        None,
        false,
        false,
    )
    .unwrap();
    assert!(sandbox_values(&write).is_empty(), "{write:?}");

    // Claude's headless argv is unchanged by the flag.
    let claude = |read_only| {
        compose_headless_command_for(
            HeadlessVendor::Claude,
            "/aida-plan BUG-1",
            "sid",
            false,
            None,
            None,
            false,
            read_only,
        )
        .unwrap()
        .1
    };
    assert_eq!(claude(true), claude(false));
}

// The drain's `--plan-phase` prelude reaches the vendor through the
// `exec_vendor_headless` hooks: the built child command carries the read-only
// sandbox.
#[test]
fn bug_2394_drain_plan_phase_codex_child_command_is_read_only() {
    let _env = headless_env();
    let cmd = build_exec_vendor_headless_command_for(
        HeadlessVendor::Codex,
        "/aida-plan BUG-1",
        "sid",
        false,
        None,
        None,
        "implementer",
        "",
        false,
        true,
    )
    .unwrap();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        sandbox_values(&args),
        vec!["read-only".to_string()],
        "{args:?}"
    );
    assert!(!args
        .iter()
        .any(|a| a == "--dangerously-bypass-approvals-and-sandbox"));
}

// Both headless plan paths in `handle_queue_work` pass `plan_only` (which is
// `--plan-only || --plan-phase`) as the read-only flag.
#[test]
fn bug_2394_queue_work_headless_plan_paths_pass_plan_only() {
    let queue =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/queue_cmd.rs"))
            .unwrap();
    let call = queue
        .find("session::exec_vendor_headless(")
        .expect("drain lane launch");
    let args = &queue[call..call + queue[call..].find(");").unwrap()];
    assert!(args.trim_end().ends_with("plan_only,"), "{args}");
    let launch = queue
        .find("crate::standalone_implementer::Launch {")
        .expect("standalone launch");
    let fields = &queue[launch..launch + queue[launch..].find("});").unwrap()];
    assert!(fields.contains("read_only: plan_only"), "{fields}");
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
