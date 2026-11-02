//! Exact-child metadata transport; real Codex sandbox fixtures make no model call.
// trace:BUG-2398 | ai:codex
#![cfg(target_os = "linux")]

use super::*;
use std::path::Path;

// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac7d6a3b | ai:claude
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
    let mut sandbox = Command::new("codex");
    sandbox.args([
        "sandbox",
        "-P",
        ":workspace",
        "--",
        "sh",
        "-c",
        "target shell script",
    ]);
    assert_eq!(
        sandbox_args(&sandbox).unwrap(),
        ["sandbox", "-P", ":workspace", "--include-managed-config"]
    );
    // Pure argv builders for other vendors retain their prior behavior.
    assert_eq!(
        crate::session::headless_vendor_args(
            crate::session::HeadlessVendor::Agy,
            "fixture",
            "session",
            false,
            None,
            None
        )
        .unwrap(),
        crate::session::agy_headless_args("fixture")
    );
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
    cmd.arg("--").args(argv);
    // Exercise the actual finalizer, including requested-seat preflight.
    if child
        .get_envs()
        .any(|(key, value)| key == "BUG2398_INVALID_RECORDER_PROBE" && value.is_some())
    {
        let overrides = codex_tool_env_overrides(child).unwrap();
        let original: Vec<_> = cmd.get_args().map(|arg| arg.to_owned()).collect();
        let mut raw = Command::new(cmd.get_program());
        raw.current_dir(root)
            .env("CODEX_HOME", home)
            .args(overrides)
            .args(original);
        for (key, value) in cmd.get_envs() {
            match value {
                Some(value) => {
                    raw.env(key, value);
                }
                None => {
                    raw.env_remove(key);
                }
            }
        }
        return output(&mut raw);
    }
    prepare_launch(&mut cmd).unwrap();
    output(&mut cmd)
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac022c22 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac7d6a3b | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.acc739ef | ai:claude
#[test]
fn bug_2398_real_codex_core_delegated_roles_and_reviewer_recording() {
    real_codex_roles(false);
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac022c22 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac7d6a3b | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.acc739ef | ai:claude
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
    let mut env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_USER", Some("bug2398-parent"))]);
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
        .env(
            "AIDA_CONTAINMENT_POSTURE",
            if native { "native" } else { "aida" },
        )
        .env("AIDA_SESSION_ID", "bug2398-independent-session")
        .env("AIDA_AGENT_TYPE", "codex")
        .env("AIDA_ROLE_INSTANCE", "companion")
        .env("AIDA_AGENT_NAME", "codex-implementer-fixture")
        .env("AIDA_AGENT_REGISTRY_TOKEN", "fixture-excluded-bearer")
        .env("UNRELATED_PARENT", "fixture-excluded-value");
    // The fixture must exercise this implementation's probe, never an older
    // installed CLI that lacks --probe-seat. CI builds the sibling binary.
    let aida_path = crate::test_exec::matching_aida_cli();
    let _aida_probe = crate::kernel::test_aida_exe::set(aida_path.clone());
    let aida = aida_path.to_string_lossy().into_owned();
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
    assert_effective_child_context(&config, root, &child, &parent, native);
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
    env.set_key("AIDA_USER", "bug2398-reviewer");
    assert_recorded_authority_boundaries(root, &reviewer.id, true);
    authority::revoke_by_id(&reviewer.id).unwrap();
    // Direct tool probes intentionally present invalid authority; a managed
    // launch would already refuse it in preflight.
    child.env("BUG2398_INVALID_RECORDER_PROBE", "1");
    let out = sandbox(
        &config,
        root,
        &child,
        &[&aida, "role", "current", "--check"],
    );
    assert!(!out.status.success(), "revoked reviewer must refuse");
    // Presented invalid grants must refuse the recorder, not merely role display.
    let out = sandbox(
        &config,
        root,
        &child,
        &[&aida, "review", "record", "TASK-1", "--verdict", "approved"],
    );
    assert!(
        !out.status.success(),
        "presented revoked grant must refuse recording"
    );
    child
        .env("AIDA_SESSION_GRANT", "")
        .env_remove("AIDA_SESSION_ROLE")
        .env_remove("BUG2398_INVALID_RECORDER_PROBE");
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
    assert!(
        out.status.success(),
        "named absent-grant compatibility record: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_recorded_authority_boundaries(root, &reviewer.id, false);
    assert_eq!(
        std::fs::read_to_string(config.join("config.toml")).unwrap(),
        contents
    );
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac51a4f1 | ai:claude
#[test]
#[cfg(target_os = "linux")]
fn bug_2398_preflight_refuses_cause_before_model_and_baseline_skips_probe() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = tmp.path().join("codex");
    crate::test_exec::write_executable(
        &fake,
        "#!/bin/sh\necho 'password=must-never-be-relayed' >&2\necho 'bwrap: No permissions to create new namespace' >&2\nexit 1\n",
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
    assert!(!error.contains("must-never-be-relayed"));
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

// Actual supported recorder output, copied byte-for-byte to the PR key.
// No attestation or verified-authority map is fabricated by this matrix.
// trace:BUG-2398 | ai:codex
fn assert_recorded_authority_boundaries(root: &Path, child_grant: &str, authorized: bool) {
    use crate::gate::{review_verdict as rv, ship};
    let body = rv::current_verdict(root, "TASK-1").unwrap().body;
    let v = rv::parse_recorded_verdict(&body).unwrap();
    let head = v.reviewed_sha.as_deref().unwrap();
    let pr_path = rv::verdict_path(root, "PR-2398");
    std::fs::create_dir_all(pr_path.parent().unwrap()).unwrap();
    std::fs::write(&pr_path, &body).unwrap();
    let mut req = aida_core::Requirement::new("fixture".into(), "desc".into());
    req.spec_id = Some("TASK-1".into());

    // Prose must not restore the same positive claim via an alternate source.
    req.comments.push(aida_core::Comment::new(
        "reviewer".into(),
        "Review: APPROVED".into(),
    ));
    assert_eq!(
        crate::gate::dispatch_authority::verdict_at_head(root, 2398, head),
        authorized.then_some(rv::VerdictKind::Approved)
    );
    assert_eq!(
        crate::human::spec_review_approved_at_head(
            root,
            &req,
            "TASK-1",
            2398,
            Some(head),
            Some("main")
        ),
        authorized
    );
    assert_eq!(
        crate::human::pr_has_approved_verdict(root, 2398),
        authorized
    );
    let action = rv::review_actionability(root, "TASK-1", Some(&v), rv::TipRelation::AtReviewedSha);
    assert_eq!(action == rv::ReviewActionability::Resolved, authorized);
    let (_, display, _) =
        crate::review_verdict_show_body(root, "TASK-1", |_| rv::TipRelation::AtReviewedSha)
            .unwrap();
    assert_eq!(
        display["actionability"],
        if authorized {
            "resolved"
        } else {
            "needs-review"
        }
    );
    assert_eq!(
        crate::review_gates::reviewer_row_actionability(root, &req, &[], |_| None)
            == rv::ReviewActionability::Resolved,
        authorized
    );
    let policy = crate::gate::approval::ship_approval_policy(
        root,
        &["TASK-1".into()],
        Some("main"),
        std::slice::from_ref(&v),
    );
    assert_eq!(
        ship::approval_authority_refusal(std::slice::from_ref(&v), &policy).is_none(),
        authorized
    );
    assert_eq!(
        crate::gate::rework_hold_release::approval_attested(
            std::slice::from_ref(&v),
            &policy,
            head,
            2398
        )
        .is_ok(),
        authorized
    );
    // Collection additionally requires THIS launch's currently live grant.
    let mut checks = crate::queue_cmd::StandaloneVerdictChecks {
        project_root: root,
        pr: 2398,
        head_sha: Some(head.into()),
        launcher_grant: None,
        child_grant: Some(child_grant.into()),
    };
    let collected = rv::collect_standalone_verdict(
        root,
        root,
        &pr_path,
        &["TASK-1".into()],
        std::time::UNIX_EPOCH,
        &checks,
    );
    assert_eq!(collected.is_ok(), authorized, "{collected:?}");
    checks.child_grant = None;
    assert!(rv::collect_standalone_verdict(
        root,
        root,
        &pr_path,
        &["TASK-1".into()],
        std::time::UNIX_EPOCH,
        &checks
    )
    .is_err());
    assert!(crate::gate::dispatch_authority::verdict_at_head(
        root,
        2398,
        "ffffffffffffffffffffffffffffffffffffffff"
    )
    .is_none());
    assert_eq!(
        rv::review_actionability(root, "TASK-1", Some(&v), rv::TipRelation::Rewritten),
        rv::ReviewActionability::NeedsReview
    );
    if authorized {
        let mut closed = v.clone();
        closed.closed_by_merge = Some(head.into());
        assert_eq!(
            rv::review_actionability(root, "TASK-1", Some(&closed), rv::TipRelation::Unknown),
            rv::ReviewActionability::Resolved
        );
        // Persisted authorship must defeat approval labels even after the
        // author's lease is gone. The recorder's actual identity is reused.
        let authorship = crate::gate::review_authority::AuthorshipRecord {
            scope: "TASK-1".into(),
            lease_id: "old-author-lease".into(),
            branch: "main".into(),
            tokens: v.attestation.as_ref().unwrap().identity.clone(),
            recorded_at: chrono::Utc::now().to_rfc3339(),
            launch: None,
        };
        let ledger = root.join(".aida/review-authorship.jsonl");
        assert!(!ledger.exists());
        std::fs::write(
            &ledger,
            format!("{}\n", serde_json::to_string(&authorship).unwrap()),
        )
        .unwrap();
        assert!(crate::gate::dispatch_authority::verdict_at_head(root, 2398, head).is_none());
        assert!(!crate::human::spec_review_approved_at_head(
            root,
            &req,
            "TASK-1",
            2398,
            Some(head),
            Some("main")
        ));
        let mut no_branch = v.clone();
        no_branch.reviewed_branch = None;
        assert_eq!(
            rv::review_actionability(
                root,
                "PR-2398",
                Some(&no_branch),
                rv::TipRelation::AtReviewedSha
            ),
            rv::ReviewActionability::NeedsReview
        );
        std::fs::remove_file(ledger).unwrap();
    }
    if !authorized {
        // Persisted compatibility artifacts and the closed-approval shortcut.
        let mut closed = v.clone();
        closed.closed_by_merge = Some(head.into());
        assert_eq!(
            rv::review_actionability(root, "TASK-1", Some(&closed), rv::TipRelation::Unknown),
            rv::ReviewActionability::NeedsReview
        );
        let alias_root = tempfile::tempdir().unwrap();
        req.agreed_id = Some("TASK-2".into());
        let alias = rv::verdict_path(alias_root.path(), "TASK-2");
        std::fs::create_dir_all(alias.parent().unwrap()).unwrap();
        std::fs::write(alias, &body).unwrap();
        assert!(!crate::human::spec_review_approved_at_head(
            alias_root.path(),
            &req,
            "TASK-1",
            2398,
            Some(head),
            Some("main")
        ));
    }
}

/// This command-level matrix runs in CI even when Codex is unavailable.
/// The real-tool fixture above adds the core-policy crossing to the same checks.
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.acc739ef | ai:claude
#[test]
fn bug_2398_actual_recorder_artifacts_require_durable_independent_authority() {
    use crate::gate::{review_authority as ra, seat_authority as sa};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut ambient = crate::test_env::AmbientGuard::hermetic_and(
        root,
        None,
        &[
            ("AIDA_AGENT_NAME", Some("codex-reviewer-fixture")),
            ("AIDA_AGENT_TYPE", Some("codex")),
            ("AIDA_SESSION_ID", Some("independent-fixture-reviewer")),
            ("AIDA_REVIEW_VERDICT_FILE", None),
            ("AIDA_ROLE_INSTANCE", Some("driver")),
        ],
    );
    let head = crate::test_git::commit_fixture(root);
    let record = || {
        crate::handle_review_record_at(
            root.to_path_buf(),
            "TASK-1",
            "approved",
            Some(&head),
            Some("main"),
            Some("actual recorder output"),
            &[],
            &[],
            None,
        )
    };
    let identity = ra::IdentitySource {
        session_ids: vec!["independent-fixture-reviewer".into()],
        ..Default::default()
    };
    ra::with_identity_source(identity.clone(), record).unwrap();
    assert_recorded_authority_boundaries(root, "no-launched-reviewer", false);
    ambient.set_key(sa::GRANT_ENV, uuid::Uuid::new_v4().to_string());
    assert!(
        ra::with_identity_source(identity.clone(), record).is_err(),
        "presented invalid grant refuses recording"
    );
    let grant = sa::test_support::mint_grant_for(
        root,
        crate::test_env::HERMETIC_TEST_USER,
        "reviewer",
        &[],
    );
    ambient.set_key(sa::GRANT_ENV, &grant);
    ra::with_identity_source(identity, record).unwrap();
    assert_recorded_authority_boundaries(root, &grant, true);
    // Persisted legacy refusals carry no positive attestation, yet retain
    // their protective dispatch/actionability meaning.
    for refusal in ["request-changes", "rejected"] {
        crate::gate::review_verdict::record_verdict(
            root,
            "PR-2399",
            Some(refusal),
            Some(&head),
            Some("main"),
            Some("protective compatibility artifact"),
            &[],
            "ungranted-reviewer",
        )
        .unwrap();
        let v = crate::gate::review_verdict::read_recorded_verdict(root, "PR-2399").unwrap();
        assert!(v.attestation.is_none());
        assert_eq!(
            crate::gate::dispatch_authority::verdict_at_head(root, 2399, &head),
            Some(v.kind)
        );
        assert_eq!(
            crate::gate::review_verdict::review_actionability(
                root,
                "PR-2399",
                Some(&v),
                crate::gate::review_verdict::TipRelation::AtReviewedSha
            ),
            crate::gate::review_verdict::ReviewActionability::AwaitingRework
        );
    }
}

// Real installed Codex policy merging, including later custom overrides. The
// second grant is valid for the same seat: ordinary seat validity alone must
// not accept its different selector/subject as this intended child.
// trace:BUG-2398 | ai:codex
fn assert_effective_child_context(
    config: &Path,
    root: &Path,
    child: &Command,
    parent: &crate::gate::seat_authority::SeatGrant,
    native: bool,
) {
    let other = crate::gate::seat_authority::issue_child_from(
        parent.clone(),
        "implementer",
        "bug2398-other-child",
        true,
    )
    .unwrap();
    let mut other_child = Command::new("codex");
    for (key, value) in child.get_envs() {
        if let Some(value) = value {
            other_child.env(key, value);
        }
    }
    other_child
        .env(crate::gate::seat_authority::GRANT_ENV, &other.id)
        .env("AIDA_USER", "bug2398-other-child");
    let aida = crate::test_exec::matching_aida_cli();
    let out = run_codex_sandbox(
        config,
        root,
        &other_child,
        &[aida.to_str().unwrap(), "role", "current", "--check"],
        native,
    );
    assert!(
        out.status.success(),
        "the alternative same-seat child must itself be valid"
    );
    let invoke = |extra: &[String]| {
        let mut cmd = Command::new("codex");
        cmd.current_dir(root).env("CODEX_HOME", config);
        for (key, value) in child.get_envs() {
            match value {
                Some(v) => {
                    cmd.env(key, v);
                }
                None => {
                    cmd.env_remove(key);
                }
            }
        }
        cmd.arg("exec");
        if native {
            cmd.args(["--sandbox", "workspace-write"]);
        } else {
            cmd.args(["-c", "sandbox_mode=\"danger-full-access\""]);
        }
        cmd.args(extra).arg("unused model prompt");
        prepare_launch(&mut cmd)
    };
    let original = std::fs::read_to_string(config.join("config.toml")).unwrap();
    std::fs::write(config.join("fixture.config.toml"), "[shell_environment_policy]\ninherit = \"core\"\n[shell_environment_policy.set]\nAIDA_SESSION_SCOPE = \"TASK-profile\"\n").unwrap();
    for profile in [false, true] {
        let base = if profile {
            vec!["--profile".into(), "fixture".into()]
        } else {
            vec![]
        };
        invoke(&base).expect("profile-only control preserves exact prefix overrides");
        for assignment in [
            "shell_environment_policy.set.AIDA_SESSION_SCOPE=\"TASK-other\"".to_string(),
            "shell_environment_policy.set={AIDA_SESSION_SCOPE=\"TASK-table\"}".to_string(),
            "shell_environment_policy={inherit=\"core\",set={AIDA_SESSION_SCOPE=\"TASK-full\"}}"
                .to_string(),
            "shell_environment_policy.set.AIDA_ROLE_INSTANCE=\"driver\"".to_string(),
            "shell_environment_policy.set.AIDA_CONTAINMENT_POSTURE=\"different\"".to_string(),
            "shell_environment_policy.set.AIDA_SESSION_ROLE=\"reviewer\"".to_string(),
            format!(
                "shell_environment_policy.set.{}=\"{}\"",
                crate::gate::seat_authority::GRANT_ENV,
                other.id
            ),
            "shell_environment_policy.set.AIDA_USER=\"bug2398-other-child\"".to_string(),
        ] {
            let mut args = base.clone();
            args.extend(["-c".into(), assignment]);
            let err = invoke(&args).unwrap_err().to_string();
            assert!(err.contains("ChildContextMismatch"), "{err}");
            assert!(
                !err.contains(&other.id),
                "selector must not enter diagnostics"
            );
        }
        let mut args = base.clone();
        args.extend([
            "-c".into(),
            format!(
                "shell_environment_policy.set.{}=\"{}\"",
                crate::gate::seat_authority::GRANT_ENV,
                other.id
            ),
            "-c".into(),
            "shell_environment_policy.set.AIDA_USER=\"bug2398-other-child\"".into(),
        ]);
        assert!(invoke(&args)
            .unwrap_err()
            .to_string()
            .contains("ChildContextMismatch"));
        let mut args = base;
        args.extend([
            "-c".into(),
            "model_reasoning_effort=\"low\"".into(),
            "-c".into(),
            "shell_environment_policy.set.CUSTOM_OK=\"preserved\"".into(),
        ]);
        invoke(&args).expect("nonprotected custom configuration remains usable");
    }
    std::fs::write(config.join("config.toml"), original).unwrap();
}

#[test]
fn bug_2398_baseline_cannot_be_replaced_by_later_child_context() {
    for override_value in [
        "shell_environment_policy.set.AIDA_SESSION_GRANT=\"11111111-1111-1111-1111-111111111111\"",
        "shell_environment_policy.set={KEEP_ME=\"preserved\"}",
        "shell_environment_policy={inherit=\"all\",set={KEEP_ME=\"preserved\"}}",
    ] {
        let mut child = Command::new("codex");
        child
            .env(crate::gate::seat_authority::GRANT_ENV, "")
            .args(["exec", "-c", override_value]);
        assert!(prepare_launch(&mut child)
            .unwrap_err()
            .to_string()
            .contains("baseline context conflict"));
    }
    let mut child = Command::new("codex");
    child.env(crate::gate::seat_authority::GRANT_ENV, "").args([
        "exec",
        "-c",
        "shell_environment_policy.set.KEEP_ME=\"preserved\"",
    ]);
    prepare_launch(&mut child).unwrap();
}

/// Codex's DEFAULT `inherit = "all"` passes the launcher's own environment to
/// tool shells. A whitelisted key the launcher has but the child never
/// assigned must not reach the child (nor fail its context preflight).
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac022c22 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac7d6a3b | ai:claude
#[test]
fn bug_2398_real_codex_inherit_all_drops_unassigned_launcher_context() {
    use crate::gate::seat_authority::{self as authority, test_support::mint_grant_for};
    let codex = Command::new("codex").arg("--version").output();
    if !codex.is_ok_and(|out| out.status.success()) {
        eprintln!("SKIP: codex is not installed");
        return;
    }
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_USER", Some("bug2398-parent")),
        ("AIDA_SESSION_SCOPE", Some("PARENT-SCOPE")),
        (
            "AIDA_AGENT_CONTEXT_FILE",
            Some("/parent/launcher-context.md"),
        ),
        ("AIDA_ROLE_INSTANCE", Some("driver")),
    ]);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let config = root.join("codex-config");
    std::fs::create_dir(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        "[shell_environment_policy]\ninherit = \"all\"\n",
    )
    .unwrap();
    let parent = mint_grant_for(root, "bug2398-parent", "advisor", &["implementer"]);
    let store = crate::detect_distributed_store_from(root).unwrap();
    aida_core::git_ops::init(&store).unwrap();
    crate::test_git::commit_fixture(root);
    let parent = authority::read_grant(&parent).unwrap();
    let implementer =
        authority::issue_child_from(parent, "implementer", "bug2398-child", true).unwrap();
    // Control: the fixture really inherits the launcher's ambient value.
    let mut raw = Command::new("codex");
    raw.current_dir(root)
        .env("CODEX_HOME", &config)
        .args(["sandbox", "-c", "sandbox_mode=\"danger-full-access\"", "--"])
        .args(["sh", "-c", "test \"$AIDA_SESSION_SCOPE\" = PARENT-SCOPE"]);
    assert!(
        output(&mut raw).status.success(),
        "inherit=all fixture must expose ambient launcher keys"
    );
    let mut child = Command::new("codex");
    child
        .env("AIDA_SESSION_GRANT", &implementer.id)
        .env("AIDA_SESSION_ROLE", "implementer")
        .env("AIDA_USER", "bug2398-child")
        .env("AIDA_PROJECT_ROOT", root)
        .env("AIDA_CONTAINMENT_POSTURE", "aida")
        .env("AIDA_AGENT_TYPE", "codex");
    let aida_path = crate::test_exec::matching_aida_cli();
    let _aida_probe = crate::kernel::test_aida_exe::set(aida_path.clone());
    let aida = aida_path.to_string_lossy().into_owned();
    // run_codex_sandbox unwraps prepare_launch: the preflight must pass.
    let out = run_codex_sandbox(
        &config,
        root,
        &child,
        &[&aida, "role", "current", "--check"],
        false,
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && text.lines().any(|l| l == "implementer"),
        "inherit=all launch with ambient launcher context: {text}"
    );
    let out = run_codex_sandbox(
        &config,
        root,
        &child,
        &[
            "sh",
            "-c",
            "test -z \"${AIDA_SESSION_SCOPE+x}\" && test -z \"${AIDA_AGENT_CONTEXT_FILE+x}\" && test -z \"${AIDA_ROLE_INSTANCE+x}\" && test \"$AIDA_USER\" = bug2398-child",
        ],
        false,
    );
    assert!(
        out.status.success(),
        "unassigned launcher keys must be absent: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// The Codex advisor-tier consult mints its own delegated `advisor` child,
/// revokes it after exit, and refuses before launch when the launcher's
/// grant does not delegate `advisor`.
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac022c22 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac51a4f1 | ai:claude
#[test]
fn bug_2398_codex_advisor_consult_delegates_own_child_and_refuses_without_delegation() {
    use crate::gate::seat_authority::{self as authority, test_support::mint_grant_for};
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(&project)
        .status()
        .unwrap()
        .success());
    let capture = tmp.path().join("advisor-grant.txt");
    let fake = tmp.path().join("fake-codex");
    crate::test_exec::write_executable(
        &fake,
        crate::test_exec::codex_probe_script(format!(
            "#!/bin/sh\nprintf '%s' \"$AIDA_SESSION_GRANT\" > '{}'\nexit 0\n",
            capture.display()
        )),
    );
    let fake = fake.to_string_lossy().into_owned();
    let mut env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HOME", Some(home.to_str().unwrap())),
        ("HOME", Some(home.to_str().unwrap())),
        ("AIDA_HEADLESS_VENDOR", Some("codex")),
        ("AIDA_AGENT_CMD", Some(&fake)),
        ("AIDA_OS_WRAP", Some("0")),
        ("AIDA_TEE_HEADLESS", Some("0")),
        ("AIDA_CONTAINED", None),
        (authority::GRANT_ENV, None),
    ]);
    let _aida_probe = crate::kernel::test_aida_exe::set(crate::test_exec::matching_aida_cli());
    crate::session::set_headless_vendor_override(None);
    let driver = crate::phase_driver::RealPhaseDriver::new(
        project.clone(),
        "BUG-2398".to_string(),
        "test-queue".to_string(),
        None,
        true,
        None,
        crate::AutonomyMode::Default,
        "run-token".to_string(),
        false,
        false,
        false,
        false,
        crate::auto_complete::LifecycleSkip::default(),
        crate::auto_complete::AutoCompleteVariant::Full,
    );
    let consult = || {
        driver.spawn_advisor_session(
            crate::advisor_cmd_root::AdvisorPass::ColdBoot,
            &crate::advisor::AdvisorConfig::default(),
            &tmp.path().join("response.json"),
            &tmp.path().join("request.json"),
        )
    };
    let user = crate::current_user_id(None);

    // Refusal: the launcher grant does not delegate `advisor`.
    let narrow = mint_grant_for(&project, &user, "advisor", &["implementer"]);
    env.set_key(authority::GRANT_ENV, &narrow);
    let failure = consult().expect_err("undelegated advisor consult must refuse");
    assert!(
        failure.reason.contains("advisor delegation")
            && failure.reason.contains("does not delegate `advisor`")
            && failure.reason.contains("--delegate-seat advisor"),
        "{}",
        failure.reason
    );
    assert!(!capture.exists(), "refusal must precede any model launch");

    // Success: a fresh advisor child, never the launcher's selector, revoked
    // once the consult exits.
    let launcher = mint_grant_for(&project, &user, "advisor", &["implementer", "advisor"]);
    env.set_key(authority::GRANT_ENV, &launcher);
    let failure = consult().expect_err("the fake advisor writes no response");
    assert!(
        failure.reason.contains("no usable response"),
        "the consult must fail only for the missing response: {}",
        failure.reason
    );
    let child = std::fs::read_to_string(&capture).unwrap();
    assert!(!child.is_empty() && child != launcher && child != narrow);
    let record = authority::read_grant(&child).unwrap();
    assert_eq!(record.seat, "advisor");
    assert_eq!(record.parent_grant_id.as_deref(), Some(launcher.as_str()));
    assert!(
        record.revoked_at.is_some(),
        "consult child must be revoked after exit"
    );
    crate::session::set_headless_vendor_override(None);
}
