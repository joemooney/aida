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
    std::fs::write(
        codex_home.join("config.toml"),
        "model = \"x\"\n[mcp_servers.aida]\ncommand = \"aida\"\nargs = [\"mcp-serve\"]\n\
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

    // A missing CODEX_HOME config contributes nothing.
    home.reset(tmp.path().join("absent"));
    assert_eq!(
        codex_configured_mcp_servers(&project),
        s(&["aida", "project_srv"])
    );
}

#[test]
fn bug_2394_claude_trust_check_is_gated_on_claude_being_a_launch_vendor() {
    use crate::doctor_cmd::claude_launch_active;
    // Explicit enabled list decides.
    assert!(!claude_launch_active(Some(false), None));
    assert!(!claude_launch_active(Some(false), Some("claude")));
    assert!(claude_launch_active(Some(true), Some("codex")));
    // No enabled list: an explicit non-Claude default vendor turns it off.
    assert!(!claude_launch_active(None, Some("codex")));
    assert!(claude_launch_active(None, Some("Claude")));
    // Unconfigured keeps the historical Claude default.
    assert!(claude_launch_active(None, None));
}
