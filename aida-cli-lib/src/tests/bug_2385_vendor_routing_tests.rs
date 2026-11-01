//! Full wrapper dispatch with synthetic homes, roster/grants and vendor binaries.
// trace:BUG-b6b1278890774ef993d2ebc327862470-159 | ai:codex
use super::*;
use std::path::Path;

fn git(root: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn dispatch(args: &[&str]) -> Result<()> {
    let cli = crate::command_registry::try_parse_from(args).unwrap();
    if args[1] == "agent" {
        let AgentArgs { inner } = cli.command.registered::<AgentArgs>() else {
            panic!("agent args")
        };
        handle_agent_command(&inner)
    } else {
        let SessionArgs { inner } = cli.command.registered::<SessionArgs>() else {
            panic!("session args")
        };
        handle_session_command(&inner)
    }
}

#[test]
fn all_three_wrappers_launch_both_vendors_and_preserve_explicit_precedence() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    for vendor in ["codex", "claude"] {
        for surface in ["new", "start", "bare", "role", "spec", "explicit"] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().join("project");
            let home = tmp.path().join("home");
            let bin = tmp.path().join("bin");
            for dir in [&root, &home, &bin] {
                std::fs::create_dir_all(dir).unwrap();
            }
            let path = format!("{}:/usr/bin:/bin", bin.display());
            let mut env = crate::test_env::EnvVarsGuard::apply(&[
                ("HOME", home.to_str()),
                ("AIDA_HOME", home.join(".aida").to_str()),
                ("PATH", Some(&path)),
                ("AIDA_CONTAINED", None),
                ("AIDA_OS_WRAP", Some("0")),
                ("AIDA_USER", Some("routing-fixture")),
                ("AIDA_SESSION_ROLE", None),
                (crate::gate::seat_authority::GRANT_ENV, None),
                ("AIDA_SESSION_ID", None),
                ("AIDA_SESSION_SCOPE", None),
                ("AIDA_BUILD_PROFILE", None),
                ("AIDA_BIN", None),
                ("AIDA_AGENT_NAME", None),
                ("AIDA_AGENT_TYPE", None),
            ]);
            std::env::set_current_dir(&root).unwrap();
            std::fs::create_dir_all(root.join(".aida")).unwrap();
            std::fs::write(root.join(".aida/config.toml"), format!(
                "store_path = \".aida-store\"\n[agents]\nvendor = \"{vendor}\"\n[contained.vendors]\ncodex = \"native\"\n[worktree_pool]\nenabled = false\nworktree_parent = {}\n",
                aida_core::toml_quote::toml_string(&tmp.path().join("seats").display().to_string())
            )).unwrap();
            // Native policy comes from a committed default branch, never the work branch.
            git(&root, &["init", "-q", "-b", "main"]);
            git(&root, &["config", "user.name", "Routing Fixture"]);
            git(&root, &["config", "user.email", "fixture@example.invalid"]);
            git(&root, &["add", ".aida/config.toml"]);
            git(&root, &["commit", "-qm", "fixture"]);
            git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
            git(
                &root,
                &[
                    "symbolic-ref",
                    "refs/remotes/origin/HEAD",
                    "refs/remotes/origin/main",
                ],
            );
            if vendor == "codex" {
                std::fs::write(
                    root.join(".aida/agents.toml"),
                    "[agents]\ncontained = true\n[agents.codex]\nmail_wake = false\n",
                )
                .unwrap();
            }
            let storage = crate::test_env::home_fixture_storage(root.join(".aida-store"));
            let mut requirement = Requirement::new("routing fixture".into(), String::new());
            requirement.spec_id = Some("BUG-2385".into());
            requirement.status = RequirementStatus::Approved;
            let mut store = aida_core::models::RequirementsStore::new();
            store.requirements.push(requirement);
            storage.save(&store).unwrap();
            let parent = crate::gate::seat_authority::test_support::mint_grant_for(
                &root,
                "routing-fixture",
                "advisor",
                &["implementer"],
            );
            env.set_key(crate::gate::seat_authority::GRANT_ENV, &parent);
            let argv = tmp.path().join("argv");
            let context = tmp.path().join("context");
            let actual_vendor = if surface == "explicit" {
                if vendor == "codex" {
                    "claude"
                } else {
                    "codex"
                }
            } else {
                vendor
            };
            crate::test_exec::write_executable(&bin.join(actual_vendor), format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo '{actual_vendor} 9.9.9'; exit 0; fi\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' \"$AIDA_AGENT_TYPE\" \"$AIDA_SESSION_ROLE\" \"$AIDA_SESSION_SCOPE\" \"$AIDA_CONTAINMENT_POSTURE\" \"${{AIDA_CONTAINED-unset}}\" \"$AIDA_SESSION_GRANT\" > '{}'\n",
                argv.display(), context.display()));
            crate::test_exec::write_executable(
                &bin.join("aida"),
                "#!/bin/sh\necho 'aida 0.16.0 fixture'\n",
            );
            let args = match surface {
                "new" => vec![
                    "aida",
                    "session",
                    "new",
                    "--title",
                    "fixture",
                    "--role",
                    "implementer",
                    "--no-title",
                ],
                "start" => vec![
                    "aida",
                    "session",
                    "start",
                    "--owns",
                    "BUG-2385",
                    "--launch",
                    "--title",
                    "fixture",
                    "--no-title",
                    "--no-pool",
                ],
                "bare" => vec!["aida", "agent", "new"],
                "role" => vec!["aida", "agent", "new", "--role", "implementer"],
                "spec" => vec![
                    "aida",
                    "agent",
                    "new",
                    "--role",
                    "implementer",
                    "--spec",
                    "BUG-2385",
                ],
                _ => vec![
                    "aida",
                    "agent",
                    "new",
                    actual_vendor,
                    "--role",
                    "implementer",
                    "--no-title",
                ],
            };
            dispatch(&args).unwrap_or_else(|e| panic!("{vendor}/{surface}: {e:#}"));
            let args = std::fs::read_to_string(&argv)
                .unwrap_or_else(|e| panic!("{vendor}/{surface}: no actual child argv: {e}"));
            let context = std::fs::read_to_string(context).unwrap();
            let fields: Vec<_> = context.lines().collect();
            if actual_vendor == "codex" {
                assert_eq!(fields[0], "codex");
                assert_eq!(fields[1], "implementer");
                assert!(!fields[5].is_empty());
                assert_ne!(fields[5], parent, "child must hold its own grant");
                assert!(!args.contains("dangerously-bypass"));
                if vendor == "codex" {
                    assert!(
                        args.contains("--sandbox\nworkspace-write\n--ask-for-approval\nnever"),
                        "{args}"
                    );
                    assert_eq!(fields[3], "native");
                    assert_eq!(fields[4], "unset");
                }
            } else {
                assert!(!args.contains("--sandbox\n"));
                assert!(!args.contains("bypassPermissions"));
                // session new/start preserve Claude's existing direct launch contract.
                if !matches!(surface, "new" | "start") {
                    assert_eq!(fields[0], "claude");
                    assert_ne!(fields[5], parent);
                }
            }
            if matches!(surface, "start" | "spec") && actual_vendor == "codex" {
                assert_eq!(fields[2], "BUG-2385");
                assert_eq!(crate::list_leases(&root).len(), 1);
            }
            std::env::set_current_dir("/tmp").unwrap();
        }
    }
}

#[test]
fn missing_vendor_refuses_all_wrappers_without_setup() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    for vendor in ["codex", "claude"] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        let home = tmp.path().join("home");
        let bin = tmp.path().join("bin");
        for path in [&root, &home, &bin] {
            std::fs::create_dir_all(path).unwrap();
        }
        let path = bin.to_str().unwrap();
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("HOME", home.to_str()),
            ("AIDA_HOME", home.join(".aida").to_str()),
            ("PATH", Some(path)),
            ("AIDA_CONTAINED", None),
            ("AIDA_SESSION_ROLE", None),
        ]);
        std::env::set_current_dir(&root).unwrap();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            format!("[agents]\nvendor = \"{vendor}\"\n"),
        )
        .unwrap();
        // No git executable: ordinary root discovery still sees the project config.
        for args in [
            vec!["aida", "session", "new", "--title", "fixture", "--no-title"],
            vec!["aida", "session", "start", "--owns", "BUG-2385", "--launch"],
            vec![
                "aida",
                "agent",
                "new",
                "--role",
                "implementer",
                "--spec",
                "BUG-2385",
            ],
        ] {
            let error = dispatch(&args).unwrap_err().to_string();
            assert!(error.contains("not on PATH"), "{vendor}: {error}");
            assert!(!root.join(".aida/sessions").exists());
            assert!(!root.join(".aida/agents").exists());
            assert!(!home.join(".aida/session-launches.log").exists());
        }
        assert!(session::preflight_interactive_session("agy", None, false)
            .unwrap_err()
            .to_string()
            .contains("unsupported"));
        std::env::set_current_dir("/tmp").unwrap();
    }
}

#[test]
fn absent_default_keeps_existing_noninteractive_help() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("HOME", tmp.path().to_str()),
        ("AIDA_HOME", tmp.path().to_str()),
    ]);
    std::env::set_current_dir(tmp.path()).unwrap();
    std::fs::create_dir(tmp.path().join(".aida")).unwrap();
    std::fs::write(tmp.path().join(".aida/config.toml"), "").unwrap();
    assert_eq!(session::interactive_session_vendor().unwrap(), "claude");
    dispatch(&["aida", "agent", "new"]).unwrap();
    assert!(!tmp.path().join(".aida/agents").exists());
    std::env::set_current_dir("/tmp").unwrap();
}

#[test]
fn trusted_native_and_parent_refusals_precede_session_setup() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    let home = tmp.path().join("home");
    let bin = tmp.path().join("bin");
    for path in [&root, &home, &bin] {
        std::fs::create_dir_all(path).unwrap();
    }
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let mut env = crate::test_env::EnvVarsGuard::apply(&[
        ("HOME", home.to_str()),
        ("AIDA_HOME", home.join(".aida").to_str()),
        ("PATH", Some(&path)),
        ("AIDA_CONTAINED", None),
        ("AIDA_SESSION_ROLE", None),
    ]);
    std::env::set_current_dir(&root).unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[agents]\nvendor = \"codex\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".aida/agents.toml"),
        "[agents]\ncontained = true\n",
    )
    .unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "Routing Fixture"]);
    git(&root, &["config", "user.email", "fixture@example.invalid"]);
    git(&root, &["add", ".aida/config.toml"]);
    git(&root, &["commit", "-qm", "trusted refusal"]);
    git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(
        &root,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    crate::test_exec::write_executable(&bin.join("codex"), "#!/bin/sh\nexit 99\n");
    // An untrusted branch's opt-in is insufficient.
    git(&root, &["checkout", "-qb", "branch"]);
    std::fs::write(
        root.join(".aida/config.toml"),
        "[agents]\nvendor = \"codex\"\n[contained.vendors]\ncodex = \"native\"\n",
    )
    .unwrap();
    for args in [
        vec!["aida", "session", "new", "--title", "fixture"],
        vec!["aida", "session", "start", "--owns", "BUG-2385", "--launch"],
    ] {
        let error = dispatch(&args).unwrap_err().to_string();
        assert!(
            error.contains("cannot run an AIDA-contained session"),
            "{error}"
        );
        assert!(!root.join(".aida/sessions").exists());
        assert!(!root.join(".aida/agents").exists());
        assert!(!home.join(".aida/session-launches.log").exists());
    }
    // Commit the policy to the trusted ref, then verify a contained parent
    // still cannot turn a native child into an AIDA-contained child.
    git(&root, &["add", ".aida/config.toml"]);
    git(&root, &["commit", "-qm", "trusted native"]);
    git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    env.set_key("AIDA_CONTAINED", "1");
    let error = dispatch(&["aida", "session", "start", "--owns", "BUG-2385", "--launch"])
        .unwrap_err()
        .to_string();
    assert!(error.contains("contained"), "{error}");
    assert!(!root.join(".aida/sessions").exists());
    std::env::set_current_dir("/tmp").unwrap();
}
