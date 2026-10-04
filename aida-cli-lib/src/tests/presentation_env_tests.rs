// trace:TASK-1482 | ai:claude
//! Tests for the TASK-1482 presentation-env stripping policy: `aida agent
//! new` must not leak the operator's local output-format/agent-mode/
//! glyph/quiet preferences into a spawned agent's environment unless the
//! operator explicitly opts a var back in via `[agents] inherit_env`.
//!
//! `classify_presentation_env` and `apply_presentation_env_policy` are pure
//! (injectable parent-env map / plain `Command` inspection), so most of
//! this file never touches the real process environment and needs no
//! `test_env` lock. The handful of tests that DO read ambient ("real")
//! env-derived state — `load_agents_inherit_env`'s global tier, and the
//! `--no-exec` / `--show-context` previews, which call
//! `resolve_presentation_env_policy` and therefore `std::env::vars()` —
//! hold `crate::test_env::EnvVarsGuard` for the whole assertion window.

use super::*;
use tempfile::TempDir;

fn env_map(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

// ---------------------------------------------------------------------
// classify_presentation_env — pure, injectable-map tests.
// ---------------------------------------------------------------------

#[test]
fn strips_known_presentation_vars_present_in_parent_with_no_keep_list() {
    let parent = env_map(&[
        ("AIDA_OUTPUT_FORMAT", "human"),
        ("AIDA_AGENT_OUTPUT", "0"),
        ("AIDA_GLYPHS", "ascii"),
        ("AIDA_QUIET", "1"),
        ("AIDA_PUSH_QUIET", "1"),
        // Unrelated vars must never be classified either way.
        ("PATH", "/usr/bin"),
        ("AIDA_PROJECT_ROOT", "/some/project"),
    ]);
    let policy = classify_presentation_env(&parent, &[]);
    assert_eq!(
        policy.stripped,
        vec![
            "AIDA_OUTPUT_FORMAT",
            "AIDA_AGENT_OUTPUT",
            "AIDA_GLYPHS",
            "AIDA_QUIET",
            "AIDA_PUSH_QUIET",
        ]
    );
    assert!(policy.kept.is_empty());
}

#[test]
fn absent_vars_are_neither_stripped_nor_kept() {
    let parent = env_map(&[("PATH", "/usr/bin")]);
    // Even naming it in the keep-list changes nothing — there is no value to
    // preserve either way.
    let policy = classify_presentation_env(&parent, &["AIDA_GLYPHS".to_string()]);
    assert!(policy.stripped.is_empty());
    assert!(policy.kept.is_empty());
}

#[test]
fn explicit_inherit_env_opt_in_keeps_only_the_named_vars() {
    let parent = env_map(&[
        ("AIDA_OUTPUT_FORMAT", "human"),
        ("AIDA_GLYPHS", "ascii"),
        ("AIDA_QUIET", "1"),
    ]);
    let keep = vec!["AIDA_GLYPHS".to_string()];
    let policy = classify_presentation_env(&parent, &keep);
    assert_eq!(policy.kept, vec!["AIDA_GLYPHS"]);
    assert_eq!(policy.stripped, vec!["AIDA_OUTPUT_FORMAT", "AIDA_QUIET"]);
}

#[test]
fn keep_list_naming_an_unrelated_var_is_a_harmless_noop() {
    // `[agents] inherit_env` only ever restores inheritance for vars this
    // task strips — naming something outside PRESENTATION_ENV_VARS grants
    // nothing new, since nothing outside that list is ever touched.
    let parent = env_map(&[("AIDA_OUTPUT_FORMAT", "human")]);
    let keep = vec!["SOME_UNRELATED_VAR".to_string()];
    let policy = classify_presentation_env(&parent, &keep);
    assert_eq!(policy.stripped, vec!["AIDA_OUTPUT_FORMAT"]);
    assert!(policy.kept.is_empty());
}

#[test]
fn describe_presentation_env_reports_none_set_when_nothing_present() {
    assert_eq!(
        describe_presentation_env(&PresentationEnvPolicy::default()),
        "none set in the launching shell"
    );
}

#[test]
fn describe_presentation_env_names_stripped_and_kept_vars() {
    let policy = PresentationEnvPolicy {
        stripped: vec!["AIDA_OUTPUT_FORMAT", "AIDA_AGENT_OUTPUT"],
        kept: vec!["AIDA_GLYPHS"],
    };
    let text = describe_presentation_env(&policy);
    assert!(
        text.contains("AIDA_OUTPUT_FORMAT, AIDA_AGENT_OUTPUT stripped"),
        "{text}"
    );
    assert!(
        text.contains("AIDA_GLYPHS kept ([agents] inherit_env opt-in)"),
        "{text}"
    );
}

// ---------------------------------------------------------------------
// apply_presentation_env_policy — Command-building assertions via
// `Command::get_envs()`, so the removal is proven on the actual `Command`
// object without spawning a process or touching real process env.
// ---------------------------------------------------------------------

#[test]
fn apply_policy_removes_stripped_vars_from_the_command() {
    let mut command = std::process::Command::new("true");
    let policy = PresentationEnvPolicy {
        stripped: vec!["AIDA_OUTPUT_FORMAT", "AIDA_AGENT_OUTPUT"],
        kept: vec!["AIDA_GLYPHS"],
    };
    apply_presentation_env_policy(&mut command, &policy);
    let envs: std::collections::HashMap<_, _> = command.get_envs().collect();
    // `env_remove` records a `(key, None)` override that beats `Command`'s
    // default full-environment inheritance for exactly that key.
    assert_eq!(
        envs.get(std::ffi::OsStr::new("AIDA_OUTPUT_FORMAT")),
        Some(&None)
    );
    assert_eq!(
        envs.get(std::ffi::OsStr::new("AIDA_AGENT_OUTPUT")),
        Some(&None)
    );
    // A kept var gets no Command-level entry at all — nothing to do, since
    // it's already inherited by default.
    assert!(!envs.contains_key(std::ffi::OsStr::new("AIDA_GLYPHS")));
}

#[test]
fn apply_policy_is_a_pure_noop_when_nothing_is_stripped() {
    // TASK-1482 acceptance: existing launch behavior is unchanged when no
    // presentation var was set in the parent and no opt-in is configured.
    let mut command = std::process::Command::new("true");
    apply_presentation_env_policy(&mut command, &PresentationEnvPolicy::default());
    assert_eq!(command.get_envs().count(), 0);
}

// ---------------------------------------------------------------------
// End-to-end: a genuinely spawned child does not see a stripped var even
// though it was set directly on the launch `Command` (modelling exactly
// what `Command`'s default full-parent-environment inheritance would
// otherwise hand it), and genuinely DOES see a kept var. No process-wide
// env mutation — only this one `Command`'s own env — so no lock is needed.
// ---------------------------------------------------------------------

// Uses a real `sh -c` child to prove the effect end-to-end; `sh` isn't a
// portable assumption on Windows, so this one test is unix-only (matches
// the existing `Command::new("sh")` convention elsewhere in this crate).
#[cfg(unix)]
#[test]
fn spawned_child_does_not_see_a_stripped_var_but_does_see_a_kept_var() {
    let parent = env_map(&[("AIDA_OUTPUT_FORMAT", "human"), ("AIDA_GLYPHS", "ascii")]);
    let keep = vec!["AIDA_GLYPHS".to_string()];
    let policy = classify_presentation_env(&parent, &keep);

    let mut command = std::process::Command::new("sh");
    command.arg("-c").arg(
        "printf 'FMT=[%s]\\n' \"$AIDA_OUTPUT_FORMAT\"; printf 'GLYPHS=[%s]\\n' \"$AIDA_GLYPHS\"",
    );
    // Model "present in the parent" the same way a real launch inherits it
    // — set directly on the command — then apply exactly the policy the
    // real launch paths (`run_tracked_agent` / `agent_new_bg_dispatch`)
    // apply.
    for (k, v) in &parent {
        command.env(k, v);
    }
    apply_presentation_env_policy(&mut command, &policy);

    let output = command.output().expect("spawn sh");
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("FMT=[]"), "{stdout}");
    assert!(stdout.contains("GLYPHS=[ascii]"), "{stdout}");
}

// ---------------------------------------------------------------------
// `[agents] inherit_env` config loader — project over global, same
// present-key-wins precedence as `[agents] bypass`/`contained`.
// ---------------------------------------------------------------------

#[test]
fn inherit_env_config_is_absent_by_default() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    let _env_guard =
        crate::test_env::EnvVarsGuard::apply(&[("AIDA_HOME", Some(home.to_str().unwrap()))]);
    assert!(load_agents_inherit_env(&project).unwrap().is_empty());
}

#[test]
fn inherit_env_config_reads_project_agents_toml() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/agents.toml"),
        "[agents]\ninherit_env = [\"AIDA_GLYPHS\", \"AIDA_QUIET\"]\n",
    )
    .unwrap();
    let _env_guard =
        crate::test_env::EnvVarsGuard::apply(&[("AIDA_HOME", Some(home.to_str().unwrap()))]);
    assert_eq!(
        load_agents_inherit_env(&project).unwrap(),
        vec!["AIDA_GLYPHS".to_string(), "AIDA_QUIET".to_string()]
    );
}

#[test]
fn inherit_env_config_project_replaces_global_when_project_sets_the_key() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        home.join(".aida/agents.toml"),
        "[agents]\ninherit_env = [\"AIDA_OUTPUT_FORMAT\"]\n",
    )
    .unwrap();
    std::fs::write(
        project.join(".aida/agents.toml"),
        "[agents]\ninherit_env = [\"AIDA_GLYPHS\"]\n",
    )
    .unwrap();
    let _env_guard =
        crate::test_env::EnvVarsGuard::apply(&[("AIDA_HOME", Some(home.to_str().unwrap()))]);
    // Project sets the key at all, so it REPLACES (not merges with) global
    // — mirrors the existing bool "last one wins" rule for bypass/contained.
    assert_eq!(
        load_agents_inherit_env(&project).unwrap(),
        vec!["AIDA_GLYPHS".to_string()]
    );
}

#[test]
fn inherit_env_config_falls_back_to_global_when_project_is_silent() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        home.join(".aida/agents.toml"),
        "[agents]\ninherit_env = [\"AIDA_OUTPUT_FORMAT\"]\n",
    )
    .unwrap();
    // Project agents.toml exists but never mentions `inherit_env`.
    std::fs::write(
        project.join(".aida/agents.toml"),
        "[agents]\nbypass = true\n",
    )
    .unwrap();
    let _env_guard =
        crate::test_env::EnvVarsGuard::apply(&[("AIDA_HOME", Some(home.to_str().unwrap()))]);
    assert_eq!(
        load_agents_inherit_env(&project).unwrap(),
        vec!["AIDA_OUTPUT_FORMAT".to_string()]
    );
}

// ---------------------------------------------------------------------
// Integration: the `--no-exec` preview and the `--show-context` body both
// surface `resolve_presentation_env_policy` (real ambient env + resolved
// config), which is where an operator actually sees this. These hold a
// full `EnvVarsGuard` covering AIDA_HOME + every PRESENTATION_ENV_VARS key
// so the assertions are hermetic regardless of the host's real shell env.
// ---------------------------------------------------------------------

fn isolated_agent_launch_plan(project: &std::path::Path, name: &str) -> AgentLaunchPlan {
    AgentLaunchPlan {
        project_root: project.to_path_buf(),
        launch_cwd: project.to_path_buf(),
        role: Some("implementer".into()),
        role_instance: RoleInstanceKind::Driver,
        current_spec: Some("TASK-1482".into()),
        name: name.to_string(),
        lease_id: None,
        native_session_id: None,
        resumed_from: None,
    }
}

#[test]
fn noexec_preview_shows_none_set_when_no_presentation_vars_are_present() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    let _env_guard = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HOME", Some(home.to_str().unwrap())),
        ("AIDA_OUTPUT_FORMAT", None),
        ("AIDA_AGENT_OUTPUT", None),
        ("AIDA_GLYPHS", None),
        ("AIDA_QUIET", None),
        ("AIDA_PUSH_QUIET", None),
    ]);

    let config = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: vec![],
        prompt_style: AgentPromptStyle::Positional,
    };
    let plan = isolated_agent_launch_plan(&project, "claude-preview");
    let prompt = AgentPromptOptions::new(None, false);
    let preview = render_agent_launch_noexec(
        std::path::Path::new("/usr/bin/claude"),
        &config,
        &plan,
        &prompt,
        &["generated prompt text".to_string()],
        true,
        true,
    )
    .unwrap();

    assert!(
        preview.contains("presentation_env: none set in the launching shell"),
        "{preview}"
    );
    // TASK-1482 acceptance: existing preview contract (the AIDA_* env
    // inputs section) is unaffected by the new presentation_env line.
    assert!(
        preview.contains("env:\n  AIDA_AGENT_TYPE=claude"),
        "{preview}"
    );
}

#[test]
fn noexec_preview_reports_stripped_and_kept_presentation_vars() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/agents.toml"),
        "[agents]\ninherit_env = [\"AIDA_GLYPHS\"]\n",
    )
    .unwrap();
    let _env_guard = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HOME", Some(home.to_str().unwrap())),
        ("AIDA_OUTPUT_FORMAT", Some("human")),
        ("AIDA_AGENT_OUTPUT", None),
        ("AIDA_GLYPHS", Some("ascii")),
        ("AIDA_QUIET", None),
        ("AIDA_PUSH_QUIET", None),
    ]);

    let config = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: vec![],
        prompt_style: AgentPromptStyle::Positional,
    };
    let plan = isolated_agent_launch_plan(&project, "claude-preview");
    let prompt = AgentPromptOptions::new(None, false);
    let preview = render_agent_launch_noexec(
        std::path::Path::new("/usr/bin/claude"),
        &config,
        &plan,
        &prompt,
        &["generated prompt text".to_string()],
        true,
        true,
    )
    .unwrap();

    assert!(
        preview.contains("presentation_env: AIDA_OUTPUT_FORMAT stripped")
            && preview.contains("AIDA_GLYPHS kept ([agents] inherit_env opt-in)"),
        "{preview}"
    );
}

#[test]
fn show_context_body_surfaces_kept_presentation_vars() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/agents.toml"),
        "[agents]\ninherit_env = [\"AIDA_GLYPHS\"]\n",
    )
    .unwrap();
    let _env_guard = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HOME", Some(home.to_str().unwrap())),
        ("AIDA_OUTPUT_FORMAT", Some("human")),
        ("AIDA_AGENT_OUTPUT", None),
        ("AIDA_GLYPHS", Some("ascii")),
        ("AIDA_QUIET", None),
        ("AIDA_PUSH_QUIET", None),
    ]);

    let config = AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: vec![],
        prompt_style: AgentPromptStyle::Positional,
    };
    let plan = isolated_agent_launch_plan(&project, "claude-context");
    let body = render_agent_launch_context(&config, &plan, "test-token").unwrap();

    assert!(
        body.contains("- Presentation env: AIDA_OUTPUT_FORMAT stripped")
            && body.contains("AIDA_GLYPHS kept ([agents] inherit_env opt-in)"),
        "{body}"
    );
}
