//! Exact-child metadata transport; real Codex sandbox fixtures make no model call.
// trace:BUG-2398 | ai:codex
use super::*;
use std::path::Path;

#[test]
fn bug_2398_whitelist_encoding_absent_empty_and_secret_ratchet() {
    for key in TOOL_ENV {
        assert!(key.starts_with("AIDA_"));
        for forbidden in [
            "TOKEN",
            "SECRET",
            "KEY",
            "PASSWORD",
            "GRANT_FILE",
            "CONTAINED",
        ] {
            assert!(!key.contains(forbidden), "{key}");
        }
    }
    let mut cmd = Command::new("codex");
    cmd.env("AIDA_USER", "child's \\\" identity ☃")
        .env("AIDA_SESSION_GRANT", "")
        .env_remove("AIDA_SESSION_SCOPE")
        .env("AIDA_AGENT_REGISTRY_TOKEN", "DO-NOT-TRANSPORT")
        .env("UNRELATED_PARENT", "DO-NOT-TRANSPORT");
    let args = codex_tool_env_overrides(&cmd).unwrap();
    assert_eq!(args.len(), 4);
    assert!(!args.join(" ").contains("DO-NOT-TRANSPORT"));
    for pair in args.chunks_exact(2) {
        assert_eq!(pair[0], "-c");
        let parsed: toml::Value = toml::from_str(&pair[1]).unwrap();
        let table = &parsed["shell_environment_policy"]["set"];
        if let Some(value) = table.get("AIDA_USER") {
            assert_eq!(value.as_str(), Some("child's \\\" identity ☃"));
        } else {
            assert_eq!(table["AIDA_SESSION_GRANT"].as_str(), Some(""));
        }
    }
    for value in ["x\ny", "x\0y", "x\ry"] {
        cmd.env("AIDA_USER", value);
        assert!(codex_tool_env_overrides(&cmd).is_err());
    }
    let mut absent = Command::new("codex");
    absent.env_remove("AIDA_SESSION_GRANT");
    assert!(codex_tool_env_overrides(&absent).unwrap().is_empty());
    absent.env("AIDA_SESSION_GRANT", "not-a-uuid");
    assert!(codex_tool_env_overrides(&absent).is_err());
}

#[test]
fn bug_2398_only_explicit_child_selector_and_sandbox_options() {
    let mut cmd = Command::new("codex");
    let child = uuid::Uuid::new_v4().to_string();
    let parent = uuid::Uuid::new_v4().to_string();
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_GRANT", Some(&parent))]);
    assert!(codex_tool_env_overrides(&cmd).unwrap().is_empty());
    cmd.env("AIDA_SESSION_GRANT", &child);
    let args = codex_tool_env_overrides(&cmd).unwrap();
    assert!(args.join(" ").contains(&child));
    assert!(!args.join(" ").contains(&parent));
    cmd.args([
        "exec",
        "--sandbox",
        "workspace-write",
        "--model",
        "unused",
        "--json",
        "prompt",
    ]);
    assert_eq!(
        sandbox_args(&cmd).unwrap(),
        ["sandbox", "-P", ":workspace", "--include-managed-config"]
    );
    // Pure argv builders for other vendors retain their prior behavior.
    let claude =
        crate::session::claude_session_args(None, None, Some("hello"), None, false, None).unwrap();
    assert!(!claude.join(" ").contains("shell_environment_policy"));
}

fn output(cmd: &mut Command) -> std::process::Output {
    // All fixture invocations get a short watchdog, including blocked namespaces.
    let mut log = tempfile::tempfile().unwrap();
    cmd.stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log.try_clone().unwrap());
    let status = bounded_status(cmd, std::time::Duration::from_secs(10)).unwrap();
    use std::io::{Read, Seek, SeekFrom};
    log.seek(SeekFrom::Start(0)).unwrap();
    let mut bytes = Vec::new();
    log.take(65536).read_to_end(&mut bytes).unwrap();
    std::process::Output {
        status,
        stdout: bytes,
        stderr: Vec::new(),
    }
}

fn run_codex_sandbox(
    home: &Path,
    root: &Path,
    child: &Command,
    argv: &[&str],
    native: bool,
) -> std::process::Output {
    let mut cmd = Command::new("codex");
    cmd.current_dir(root).env("CODEX_HOME", home);
    for (key, value) in child.get_envs() {
        if let Some(value) = value {
            cmd.env(key, value);
        } else {
            cmd.env_remove(key);
        }
    }
    cmd.arg("sandbox");
    if native {
        cmd.args(["-P", ":workspace"]);
    } else {
        cmd.args(["-c", "sandbox_mode=\"danger-full-access\""]);
    }
    cmd.args(codex_tool_env_overrides(child).unwrap())
        .arg("--")
        .args(argv);
    output(&mut cmd)
}

#[test]
fn bug_2398_real_codex_core_delegated_roles_and_reviewer_recording() {
    real_codex_roles(false);
}

#[test]
fn bug_2398_real_native_codex_core_delegated_roles_and_reviewer_recording() {
    real_codex_roles(true);
}

fn real_codex_roles(native: bool) {
    let sandbox = |home: &Path, root: &Path, child: &Command, argv: &[&str]| {
        run_codex_sandbox(home, root, child, argv, native)
    };
    use crate::gate::seat_authority::{self as authority, test_support::mint_grant_for};
    use aida_core::db::DatabaseBackend;
    let codex = Command::new("codex").arg("--version").output();
    if !codex.is_ok_and(|out| out.status.success()) {
        eprintln!("SKIP: codex is not installed");
        return;
    }
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_USER", Some("bug2398-parent"))]);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let config = root.join("codex-config");
    std::fs::create_dir(&config).unwrap();
    let contents = "[shell_environment_policy]\ninherit = \"core\"\n[shell_environment_policy.set]\nKEEP_ME = \"preserved\"\n";
    std::fs::write(config.join("config.toml"), contents).unwrap();
    let parent = mint_grant_for(
        root,
        "bug2398-parent",
        "advisor",
        &["implementer", "reviewer"],
    );
    let store = crate::detect_distributed_store_from(root).unwrap();
    aida_core::git_ops::init(&store).unwrap();
    aida_core::git_ops::configure_user(&store, "fixture", "fixture@localhost").unwrap();
    let backend = crate::test_env::home_fixture_cached_backend(
        &store,
        &aida_core::CachedGitBackend::default_cache_path(&store),
    )
    .unwrap();
    let mut req = aida_core::models::Requirement::new("fixture".into(), "desc".into());
    req.spec_id = Some("TASK-1".into());
    backend.add_requirement(req).unwrap();
    crate::test_git::commit_fixture(root);
    let parent = authority::read_grant(&parent).unwrap();
    let implementer =
        authority::issue_child_from(parent.clone(), "implementer", "bug2398-child", true).unwrap();
    let mut child = Command::new("codex");
    child
        .env("AIDA_SESSION_GRANT", &implementer.id)
        .env("AIDA_SESSION_ROLE", "implementer")
        .env("AIDA_USER", "bug2398-child")
        .env("AIDA_PROJECT_ROOT", root)
        .env("AIDA_SESSION_SCOPE", "TASK-1")
        .env("AIDA_SESSION_ID", "bug2398-independent-session")
        .env("AIDA_AGENT_TYPE", "codex")
        .env("AIDA_ROLE_INSTANCE", "companion")
        .env("AIDA_AGENT_NAME", "codex-implementer-fixture")
        .env("AIDA_AGENT_REGISTRY_TOKEN", "fixture-excluded-bearer")
        .env("UNRELATED_PARENT", "fixture-excluded-value");
    let aida = crate::session::resolve_agent_program("aida");
    let out = sandbox(
        &config,
        root,
        &child,
        &[&aida, "role", "current", "--check"],
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && text.lines().any(|l| l == "implementer"),
        "actual Codex sandbox role probe: {text}"
    );
    let out = sandbox(&config, root, &child, &["sh", "-c", "test \"$KEEP_ME\" = preserved && test \"$AIDA_USER\" = bug2398-child && test \"$AIDA_SESSION_SCOPE\" = TASK-1 && test -z \"${AIDA_AGENT_REGISTRY_TOKEN-}\""]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let out = sandbox(
        &config,
        root,
        &child,
        &["sh", "-c", "test \"$AIDA_ROLE_INSTANCE\" = companion"],
    );
    assert!(
        out.status.success(),
        "companion metadata must survive core policy"
    );
    child.env("AIDA_ROLE_INSTANCE", "driver");
    // Permitted own-scope write and real advisor/wrong-seat refusals.
    for (argv, success) in [
        (
            vec![
                aida.as_str(),
                "comment",
                "add",
                "TASK-1",
                "own-scope fixture",
            ],
            true,
        ),
        (
            vec![aida.as_str(), "edit", "TASK-1", "--status", "approved"],
            false,
        ),
        (
            vec![
                aida.as_str(),
                "review",
                "record",
                "TASK-1",
                "--verdict",
                "approved",
            ],
            false,
        ),
    ] {
        let out = sandbox(&config, root, &child, &argv);
        assert_eq!(
            out.status.success(),
            success,
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
    let reviewer =
        authority::issue_child_from(parent, "reviewer", "bug2398-reviewer", true).unwrap();
    child
        .env("AIDA_SESSION_GRANT", &reviewer.id)
        .env("AIDA_SESSION_ROLE", "reviewer")
        .env("AIDA_USER", "bug2398-reviewer")
        .env("AIDA_AGENT_NAME", "codex-reviewer-fixture");
    let out = sandbox(
        &config,
        root,
        &child,
        &[&aida, "review", "record", "TASK-1", "--verdict", "approved"],
    );
    assert!(
        out.status.success(),
        "granted reviewer: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    authority::revoke_by_id(&reviewer.id).unwrap();
    let out = sandbox(
        &config,
        root,
        &child,
        &[&aida, "role", "current", "--check"],
    );
    assert!(!out.status.success(), "revoked reviewer must refuse");
    child.env("AIDA_SESSION_GRANT", "");
    let out = sandbox(
        &config,
        root,
        &child,
        &[
            "sh",
            "-c",
            "test \"${AIDA_SESSION_GRANT+x}\" = x && test -z \"$AIDA_SESSION_GRANT\"",
        ],
    );
    assert!(out.status.success());
    let out = sandbox(
        &config,
        root,
        &child,
        &[&aida, "review", "record", "TASK-1", "--verdict", "approved"],
    );
    assert!(!out.status.success(), "ungranted reviewer must refuse");
    assert_eq!(
        std::fs::read_to_string(config.join("config.toml")).unwrap(),
        contents
    );
}

#[test]
fn bug_2398_preflight_refuses_cause_before_model_and_baseline_skips_probe() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = tmp.path().join("codex");
    crate::test_exec::write_executable(
        &fake,
        "#!/bin/sh\necho 'bwrap: No permissions to create new namespace' >&2\nexit 1\n",
    );
    let mut cmd = Command::new(&fake);
    cmd.args(["exec", "--sandbox", "workspace-write", "do work"])
        .env("AIDA_SESSION_GRANT", uuid::Uuid::new_v4().to_string())
        .env("AIDA_SESSION_ROLE", "reviewer");
    let error = prepare_launch(&mut cmd).unwrap_err().to_string();
    assert!(
        error.contains("No permissions to create new namespace"),
        "{error}"
    );
    assert!(!cmd.get_args().any(|arg| arg
        .to_string_lossy()
        .starts_with("shell_environment_policy.set.")));
    cmd.env("AIDA_SESSION_GRANT", "")
        .env("AIDA_SESSION_ROLE", "implementer");
    prepare_launch(&mut cmd).unwrap();
    // A stuck namespace helper is bounded too.
    let error = bounded_status(
        Command::new("sh").args(["-c", "sleep 30"]),
        std::time::Duration::from_millis(50),
    )
    .unwrap_err();
    assert!(error.to_string().contains("timed out"));
}

#[test]
fn bug_2398_prefix_and_wrapped_prefix_preserve_companion_without_tokens() {
    for wrapped in [false, true] {
        let mut cmd = Command::new(if wrapped { "bwrap" } else { "codex" });
        if wrapped {
            cmd.args(["--ro-bind", "/", "/", "--", "/usr/bin/codex"]);
        }
        cmd.args(["exec", "--", "prompt"])
            .env("AIDA_SESSION_GRANT", "")
            .env("AIDA_ROLE_INSTANCE", "companion")
            .env("AIDA_HANDLE_LAUNCH_TOKEN", "excluded-handle-bearer")
            .env("AIDA_AGENT_REGISTRY_TOKEN", "excluded-registry-bearer");
        prepare_launch(&mut cmd).unwrap();
        let args: Vec<_> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let prefix = if wrapped { 5 } else { 0 };
        assert_eq!(args[prefix], "-c");
        assert!(args[prefix + 1].starts_with("shell_environment_policy.set.AIDA_ROLE_INSTANCE="));
        let exec = args.iter().position(|arg| arg == "exec").unwrap();
        assert!(exec > prefix);
        assert_eq!(&args[exec..], ["exec", "--", "prompt"]);
        assert!(!args.join(" ").contains("excluded-"));
        assert!(cmd
            .get_envs()
            .any(|(key, value)| key == "AIDA_ROLE_INSTANCE"
                && value == Some(std::ffi::OsStr::new("companion"))));
        cmd.env("AIDA_ROLE_INSTANCE", "invalid");
        assert!(codex_tool_env_overrides(&cmd).is_err());
    }
    for grant in [None, Some("")] {
        let mut cmd = Command::new("codex");
        cmd.env("AIDA_SESSION_ROLE", "reviewer");
        if let Some(grant) = grant {
            cmd.env("AIDA_SESSION_GRANT", grant);
        }
        assert!(
            prepare_launch(&mut cmd).is_err(),
            "a requested reviewer cannot launch without its grant"
        );
    }
}

#[test]
fn bug_2398_final_spawn_boundary_ratchet() {
    for (source, expected) in [
        (include_str!("../agent_launch.rs"), 2),
        (include_str!("../session.rs"), 4),
        (include_str!("../phase_driver_trait.rs"), 1),
        (include_str!("../phase_driver.rs"), 1),
        (include_str!("../dispatch.rs"), 1),
        (include_str!("../compete_cmd.rs"), 1),
    ] {
        assert!(
            source
                .matches("crate::codex_tool_env::prepare_launch")
                .count()
                >= expected,
            "a managed Codex spawn path lost finalization"
        );
    }
    let queue = include_str!("../queue_cmd.rs");
    assert!(queue.contains("session::exec_codex_session("));
    // Pure composers stay pure; caller-side customization precedes finalization.
    let session = include_str!("../session.rs");
    let run = session
        .split("fn run_vendor_headless_child(")
        .nth(1)
        .unwrap();
    assert!(
        run.find("customize(&mut child_cmd)").unwrap()
            < run.find("crate::codex_tool_env::prepare_launch").unwrap()
    );
}
