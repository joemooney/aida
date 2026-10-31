// trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
//
// Command-level: `aida agent new --rotate` with a real `sh` child. The first
// generation writes a fresh handoff and exits; the supervisor relaunches it
// under the same handle with the handoff and the previous session named in
// its env. The second exits fast without a handoff, which stops the run.
use super::*;

#[cfg(unix)]
#[test]
fn a_fresh_handoff_relaunches_the_same_handle_and_a_bare_fast_exit_stops() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().canonicalize().unwrap().join("project");
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(project.join(".aida/config.toml"), "").unwrap();
    let binary = tmp.path().canonicalize().unwrap().join("fake-claude");
    crate::test_exec::write_executable(
        &binary,
        r#"#!/bin/sh
case "$1" in --version) echo "2.0.0 (Claude Code)"; exit 0;; esac
sid=""
prev=""
for a in "$@"; do
  [ "$prev" = "--session-id" ] && sid="$a"
  prev="$a"
done
echo "gen=$AIDA_ROTATION_GENERATION name=$AIDA_AGENT_NAME sid=$sid handoff=${AIDA_ROTATION_HANDOFF:-} previous=${AIDA_ROTATION_PREVIOUS_SESSION:-}" >> "$PWD/generations.log"
if [ "$AIDA_ROTATION_GENERATION" = 1 ]; then
  mkdir -p "$AIDA_PROJECT_ROOT/.aida/handoff/claude"
  printf '{"ts":"%s","seat":"claude","event":"handoff","path":".aida/handoff/claude/h1.md","handle":"%s","native_session_id":"%s"}\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$AIDA_AGENT_NAME" "$sid" >> "$AIDA_PROJECT_ROOT/.aida/handoff/claude/log.jsonl"
fi
exit 0
"#,
    );
    let first_sid = agent_native_session_id_for_new("claude");
    let mut config = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: vec![],
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    };
    config
        .default_args
        .extend(agent_seed_session_args("claude", first_sid.as_deref()));
    let plan = AgentLaunchPlan {
        project_root: project.clone(),
        launch_cwd: project.clone(),
        role: None,
        role_instance: RoleInstanceKind::Driver,
        current_spec: None,
        name: "claude-advisor-08".into(),
        lease_id: None,
        native_session_id: first_sid.clone(),
        resumed_from: None,
    };
    let mut spawned = 0;
    let status = run_rotating_agent(
        &binary,
        &config,
        &plan,
        None,
        &["first prompt".to_string()],
        None,
        false,
        false,
        None,
        || spawned += 1,
    )
    .unwrap();
    assert!(status.success());
    assert_eq!(spawned, 1, "the launch's spawn hook runs once");
    let log = std::fs::read_to_string(project.join("generations.log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "{log}");
    let sid1 = first_sid.unwrap();
    assert!(
        lines[0].starts_with(&format!(
            "gen=1 name=claude-advisor-08 sid={sid1} handoff= previous="
        )),
        "{log}"
    );
    assert!(
        lines[1].starts_with("gen=2 name=claude-advisor-08 sid="),
        "{log}"
    );
    assert!(!lines[1].contains(&format!("sid={sid1} ")), "{log}");
    assert!(
        lines[1].contains(&format!(
            "handoff={}",
            project.join(".aida/handoff/claude/h1.md").display()
        )),
        "{log}"
    );
    assert!(lines[1].ends_with(&format!("previous={sid1}")), "{log}");
    // The run is over: the handle's relay run no longer holds it.
    let reservation = crate::agent_handle::load_reservation(&project, "claude-advisor-08").unwrap();
    assert!(crate::relay_mail::run_holding(&project, &reservation).is_none());
}
