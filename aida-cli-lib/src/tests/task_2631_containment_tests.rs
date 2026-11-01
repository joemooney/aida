//! TASK-2631 acceptance tests: explicit native sandbox never counts as AIDA containment.
// trace:TASK-2631 | ai:codex
use crate::session::{self, ContainmentPosture, HeadlessVendor};
use std::path::Path;

fn policy(root: &Path, vendors: &str) {
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        format!(
        "[agents]\nvendor = \"codex\"\n[contained]\nenable = true\n[contained.vendors]\n{vendors}\n"
    ),
    )
    .unwrap();
    commit_trusted_policy(root);
}

fn git(root: &Path, args: &[&str]) {
    let output = crate::test_git::git_command()
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn commit_trusted_policy(root: &Path) {
    if !root.join(".git").exists() {
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.name", "test"]);
        git(root, &["config", "user.email", "test@example.com"]);
    }
    git(root, &["add", ".aida/config.toml"]);
    git(
        root,
        &["commit", "-q", "--allow-empty", "-m", "trusted policy"],
    );
    git(root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(
        root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
}

// A committed branch-local downgrade cannot supply launch authority.
// trace:TASK-2631 | ai:codex
#[test]
fn containment_branch_local_native_opt_in_is_ignored() {
    let root = tempfile::tempdir().unwrap();
    policy(root.path(), "");
    git(root.path(), &["checkout", "-q", "-b", "untrusted"]);
    std::fs::write(
        root.path().join(".aida/config.toml"),
        "[contained.vendors]\ncodex = \"native\"\n",
    )
    .unwrap();
    git(root.path(), &["add", ".aida/config.toml"]);
    git(root.path(), &["commit", "-q", "-m", "branch opt-in"]);
    assert_eq!(
        crate::agent_config::load_contained_vendor(root.path(), "codex").unwrap(),
        None
    );
    assert!(session::containment_posture_in(root.path(), "codex", true).is_err());

    // Missing trusted config also refuses, even with an opt-in in branch HEAD.
    git(root.path(), &["checkout", "-q", "main"]);
    git(root.path(), &["rm", ".aida/config.toml"]);
    git(root.path(), &["commit", "-q", "-m", "no trusted config"]);
    git(
        root.path(),
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    git(root.path(), &["checkout", "-q", "untrusted"]);
    assert!(session::containment_posture_in(root.path(), "codex", true).is_err());
}

#[test]
fn containment_trusted_native_opt_in_survives_branch_edits() {
    let root = tempfile::tempdir().unwrap();
    policy(root.path(), "codex = \"native\"");
    git(root.path(), &["checkout", "-q", "-b", "branch"]);
    std::fs::write(
        root.path().join(".aida/config.toml"),
        "[contained.vendors]\ncodex = \"aida\"\n",
    )
    .unwrap();
    git(root.path(), &["add", ".aida/config.toml"]);
    git(root.path(), &["commit", "-q", "-m", "branch policy"]);
    assert_eq!(
        session::containment_posture_in(root.path(), "codex", true).unwrap(),
        ContainmentPosture::Native
    );
}

fn config(agent_type: &'static str) -> crate::AgentLaunchConfig {
    crate::AgentLaunchConfig {
        agent_type,
        binary: agent_type,
        default_args: Vec::new(),
        prompt_style: crate::AgentPromptStyle::Positional,
        contained: false,
    }
}

// AC2': config validation, tri-state and no-key fail-closed, including unknown tools.
// trace:TASK-2631 | ai:codex
#[test]
fn containment_policy_is_explicit_and_vendor_specific() {
    let root = tempfile::tempdir().unwrap();
    for vendor in ["codex", "agy", "unknown"] {
        policy(root.path(), "");
        assert!(session::containment_posture_in(root.path(), vendor, true).is_err());
        assert_eq!(
            session::containment_posture_in(root.path(), vendor, false).unwrap(),
            ContainmentPosture::None
        );
        policy(root.path(), &format!("{vendor} = \"native\""));
        let posture = session::containment_posture_in(root.path(), vendor, true);
        if vendor == "codex" {
            assert_eq!(posture.unwrap(), ContainmentPosture::Native);
            assert!(!session::containment_applied(HeadlessVendor::Codex, true));
        } else {
            let error = posture.unwrap_err().to_string();
            assert!(error.contains(vendor), "{error}");
            assert!(error.contains("native sandbox"), "{error}");
        }
    }
    for value in ["\"invalid\"", "true", "4"] {
        policy(root.path(), &format!("codex = {value}"));
        let error = session::containment_posture_in(root.path(), "codex", true).unwrap_err();
        assert!(error.to_string().contains("must be \"aida\" or \"native\""));
    }
    policy(root.path(), "codex = \"aida\"");
    assert!(session::containment_posture_in(root.path(), "codex", true).is_err());
    policy(root.path(), "claude = \"native\"");
    assert!(session::containment_posture_in(root.path(), "claude", true)
        .unwrap_err()
        .to_string()
        .contains("claude"));
    policy(root.path(), "claude = \"aida\"");
    assert_eq!(
        session::containment_posture_in(root.path(), "claude", true).unwrap(),
        ContainmentPosture::Aida
    );
    std::fs::write(
        root.path().join(".aida/config.toml"),
        "[contained.vendors\n",
    )
    .unwrap();
    commit_trusted_policy(root.path());
    assert!(session::containment_posture_in(root.path(), "codex", true).is_err());
}

// AC2': foreground default-flag builder, no widening defaults.
// trace:TASK-2631 | ai:codex
#[test]
fn containment_agent_new_codex_uses_native_flags() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HOME", home.path().to_str()),
        ("AIDA_CONTAINED", None),
    ]);
    let _cwd = session::contained_cwd_seam::pin(root.path());
    policy(root.path(), "codex = \"native\"");
    crate::refuse_uncontainable_agent_tool("codex").unwrap();
    for defaults in ["", "[agents.codex]\ndefault_flags = [\"--dangerously-bypass-approvals-and-sandbox\", \"--model\", \"test-model\"]\n"] {
        std::fs::write(root.path().join(".aida/agents.toml"), defaults).unwrap();
        let mut launch = config("codex");
        crate::apply_agent_default_flags(&mut launch, root.path(), crate::AgentDefaultFlagOptions::new(true, Vec::new(), None), false).unwrap();
        let expected = crate::tool_contained_flags("codex").unwrap();
        assert_eq!(&launch.default_args[..expected.len()], expected);
        assert!(!launch.default_args.iter().any(|a| a == "--dangerously-bypass-approvals-and-sandbox"));
        assert!(!launch.contained);
        assert!(crate::resolved_agent_permission_summary("codex", &launch.default_args).contains("native sandbox, not AIDA-contained"));
    }
    let mut launch = config("codex");
    assert!(crate::apply_agent_default_flags(
        &mut launch,
        root.path(),
        crate::AgentDefaultFlagOptions::new(
            true,
            vec!["--dangerously-bypass-approvals-and-sandbox".into()],
            None
        ),
        false
    )
    .is_err());
}

// AC2': actual mocked headless launch, implementer/reviewer argv and child env.
// trace:TASK-2631 | ai:codex
#[test]
fn containment_native_headless_child_launches_without_aida_marker() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let fake = root.path().join("fake-codex");
    crate::test_exec::write_executable(&fake, "#!/bin/sh\nprintf '%s\\n' \"posture=$AIDA_CONTAINMENT_POSTURE\" \"contained=${AIDA_CONTAINED-unset}\" \"$@\"\n");
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HOME", home.path().to_str()),
        ("AIDA_CONTAINED", None),
        ("AIDA_OS_WRAP", None),
        ("AIDA_AGENT_CMD", fake.to_str()),
        ("AIDA_CONTAINMENT_POSTURE", Some("aida")),
    ]);
    let _cwd = session::contained_cwd_seam::pin(root.path());
    policy(root.path(), "codex = \"native\"");
    session::require_vendor_containment(HeadlessVendor::Codex, true, "implementer").unwrap();
    session::require_child_containment(HeadlessVendor::Codex, true, "reviewer").unwrap();
    session::child_containment_preflight(HeadlessVendor::Codex, true, false, "reviewer").unwrap();
    assert_eq!(
        session::containment_posture(HeadlessVendor::Codex, true).unwrap(),
        ContainmentPosture::Native
    );
    for seat in ["implementer", "reviewer"] {
        let mut cmd = session::build_exec_vendor_headless_command(
            HeadlessVendor::Codex,
            "prompt",
            "sid",
            true,
            None,
            None,
            seat,
            "",
        )
        .unwrap();
        let output = cmd.output().unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("posture=native\ncontained=unset\n"), "{text}");
        assert!(
            text.contains("--sandbox\nworkspace-write\n--ask-for-approval\nnever\nexec\n--json"),
            "{text}"
        );
        assert!(!text.contains("dangerously-bypass"), "{text}");
    }
    let args = session::headless_vendor_args_contained_in(
        HeadlessVendor::Codex,
        "p",
        "sid",
        Some(root.path()),
        None,
        None,
    )
    .unwrap();
    assert!(!args.iter().any(|a| a.contains("dangerously-bypass")));
    let review = session::interactive_reviewer_launch_plan(
        HeadlessVendor::Codex,
        None,
        None,
        "p",
        "sid",
        true,
    )
    .unwrap();
    assert!(!review.contained);
    assert!(review.args.contains(&"workspace-write".into()));
}

// AC2': Claude's generated flags are byte-identical with and without Codex opt-in.
// trace:TASK-2631 | ai:codex
#[test]
fn containment_codex_opt_in_preserves_claude_bytes() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HOME", home.path().to_str()),
        ("AIDA_CONTAINED", None),
        ("AIDA_OS_WRAP", None),
    ]);
    let _cwd = session::contained_cwd_seam::pin(root.path());
    policy(root.path(), "");
    let interactive = crate::tool_contained_flags("claude").unwrap();
    let headless =
        session::headless_vendor_args(HeadlessVendor::Claude, "p", "sid", true, None, None)
            .unwrap();
    policy(root.path(), "codex = \"native\"");
    assert_eq!(interactive, crate::tool_contained_flags("claude").unwrap());
    assert_eq!(
        headless,
        session::headless_vendor_args(HeadlessVendor::Claude, "p", "sid", true, None, None)
            .unwrap()
    );
    assert!(session::containment_applied(HeadlessVendor::Claude, true));
    let mut launch = config("claude");
    crate::apply_agent_default_flags(
        &mut launch,
        root.path(),
        crate::AgentDefaultFlagOptions::new(true, Vec::new(), None),
        false,
    )
    .unwrap();
    assert!(launch.contained);
}

// AC3 (fixture) and AC2' reporting: contradiction names settings/fix, native clears it.
// trace:TASK-2631 | ai:codex
#[test]
fn containment_doctor_and_permission_reports_distinguish_native() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HOME", home.path().to_str()),
        ("AIDA_CONTAINED", None),
        ("AIDA_CONTAINMENT_POSTURE", None),
        ("AIDA_AGENT_TYPE", Some("codex")),
    ]);
    policy(root.path(), "");
    let findings = crate::config_cmd::scan_permission_posture_findings(root.path());
    let contradiction = findings
        .iter()
        .find(|f| f.id == "default-headless-vendor-refused-under-containment")
        .unwrap();
    assert!(contradiction
        .summary
        .contains("[agents] vendor = \"codex\""));
    assert!(contradiction.summary.contains("[contained] enable = true"));
    assert!(contradiction
        .action
        .contains("[contained.vendors] codex = \"native\""));
    policy(root.path(), "codex = \"native\"");
    let report = crate::config_cmd::permission_posture_report(root.path());
    let codex = report.agents.iter().find(|r| r.agent == "codex").unwrap();
    assert!(!codex.refused_under_containment);
    assert_eq!(codex.net_effect, "native sandbox, not AIDA-contained");
    assert_eq!(
        codex.headless_flags,
        crate::tool_contained_flags("codex").unwrap()
    );
    assert!(
        !crate::config_cmd::scan_permission_posture_findings(root.path())
            .iter()
            .any(|f| f.id == contradiction.id)
    );
    assert!(
        crate::statusline_cmd::native_containment_statusline_segment(root.path(), Some("codex"))
            .unwrap()
            .contains("native sandbox, not AIDA-contained")
    );
    assert!(
        crate::statusline_cmd::native_containment_statusline_segment(root.path(), Some("claude"))
            .is_none()
    );
}

// AC2': native posture cannot be widened by aliases in per-tool defaults/passthrough.
// trace:TASK-2631 | ai:codex
#[test]
fn containment_native_codex_partitions_posture_aliases() {
    for flags in [
        vec!["-s", "danger-full-access"],
        vec!["-sdanger-full-access"],
        vec!["-a", "on-request"],
        vec!["--yolo"],
        vec!["--approve-for-me"],
    ] {
        let (kept, dropped) =
            crate::split_posture_flags("codex", flags.iter().map(|s| s.to_string()).collect());
        assert!(kept.is_empty(), "{flags:?}: {kept:?}");
        assert!(!dropped.is_empty());
    }
}

// AC2': a contained Claude child cannot inherit a native display label or select native policy.
// trace:TASK-2631 | ai:codex
#[test]
fn containment_claude_rejects_native_even_for_generated_explicit_flags() {
    let root = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_CONTAINED", Some("1")),
        ("AIDA_CONTAINMENT_POSTURE", Some("native")),
    ]);
    policy(root.path(), "claude = \"native\"");
    let mut launch = config("claude");
    launch.contained = true;
    let error = crate::apply_agent_default_flags(
        &mut launch,
        root.path(),
        crate::AgentDefaultFlagOptions::new(false, Vec::new(), None),
        true,
    )
    .unwrap_err();
    assert!(error.to_string().contains("claude"));
    assert!(error.to_string().contains("native sandbox"));
    assert!(
        crate::statusline_cmd::native_containment_statusline_segment(root.path(), Some("claude"))
            .is_none()
    );
}
