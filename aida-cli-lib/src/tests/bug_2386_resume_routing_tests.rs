//! BUG-2386: resume routing, refusal and containment for Codex sessions.
//!
//! Covers the review's strict-tier gaps: the recorded-vendor gate, the
//! classification of an id handed to `queue work --resume`, the time bound on
//! the rollout fallback, containment requested by the caller's flag, the
//! headless Codex resume spawn, the punt-resume vendor choice, and the
//! `aida session resume` Codex arm building a real command.
// trace:BUG-2386 | ai:claude
use crate::codex_resume::{recorded_resume, resolve_thread_id, ThreadLookup};
use crate::session::{self, HeadlessVendor, ResumeTarget};
use crate::test_env::EnvVarsGuard;
use std::path::Path;

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

/// `.aida/config.toml` opting Codex into its native sandbox, committed as
/// the trusted (origin/main) policy. `enable` is the `[contained] enable`
/// knob (an always-contained project).
fn codex_native_policy(root: &Path, enable: bool) {
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::create_dir_all(root.join(".aida-store")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        format!(
            "store_path = \".aida-store\"\n[agents]\nvendor = \"codex\"\n[contained]\n\
             enable = {enable}\n[contained.vendors]\ncodex = \"native\"\n"
        ),
    )
    .unwrap();
    if !root.join(".git").exists() {
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.name", "test"]);
        git(root, &["config", "user.email", "test@example.com"]);
    }
    git(root, &["add", ".aida/config.toml"]);
    git(root, &["commit", "-q", "--allow-empty", "-m", "policy"]);
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

fn args_of(cmd: &std::process::Command) -> Vec<String> {
    cmd.get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

fn write_rollout(root: &Path, id: &str, cwd: &Path, ts: &str) {
    let dir = root.join("2026/11/01");
    std::fs::create_dir_all(&dir).unwrap();
    let line = serde_json::json!({
        "timestamp": ts,
        "type": "session_meta",
        "payload": {"id": id, "timestamp": ts, "cwd": cwd},
    });
    std::fs::write(
        dir.join(format!("rollout-2026-11-01T00-00-00-{id}.jsonl")),
        format!("{line}\n"),
    )
    .unwrap();
}

fn write_lease(project: &Path, id: &str, scope: &str, tree: &Path, started: &str, extra: &str) {
    let dir = crate::leases::leases_dir(project);
    std::fs::create_dir_all(&dir).unwrap();
    let mut lease = toml::Table::new();
    for (key, value) in [
        ("id", id),
        ("scope", scope),
        ("slug", "s"),
        ("owner", "t"),
        ("branch", "b"),
        ("started_at", started),
        ("hostname", "h"),
    ] {
        lease.insert(key.into(), toml::Value::String(value.into()));
    }
    lease.insert(
        "worktree_path".into(),
        toml::Value::String(tree.to_string_lossy().into_owned()),
    );
    let body = format!("{}{extra}", toml::to_string(&lease).unwrap());
    std::fs::write(dir.join(format!("{id}.toml")), body).unwrap();
}

// ── require_resume_vendor ──────────────────────────────────────────────────

#[test]
fn resume_gate_refuses_a_recorded_vendor_that_agents_enabled_disables() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _env = EnvVarsGuard::apply(&[("AIDA_HOME", home.path().to_str())]);
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida/agents.toml"),
        "[agents]\nenabled = [\"claude\"]\n",
    )
    .unwrap();
    let err = session::require_resume_vendor(tmp.path(), HeadlessVendor::Codex)
        .expect_err("a disabled recording vendor is refused");
    let msg = format!("{err:#}");
    assert!(msg.contains("`[agents] enabled` disables"), "{msg}");
    assert!(msg.contains("codex"), "{msg}");
    session::require_resume_vendor(tmp.path(), HeadlessVendor::Claude).unwrap();
    let agy = session::require_resume_vendor(tmp.path(), HeadlessVendor::Agy)
        .expect_err("agy cannot resume");
    assert!(format!("{agy:#}").contains("agy session cannot be resumed"));
}

// ── recorded_resume classification ─────────────────────────────────────────

struct Fixture {
    _tmp: tempfile::TempDir,
    project: std::path::PathBuf,
    tree: std::path::PathBuf,
    sessions: std::path::PathBuf,
    home: std::path::PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();
    let project = base.join("project");
    let tree = base.join("lane");
    let sessions = base.join("codex-sessions");
    let home = base.join("home");
    for d in [&project, &tree, &sessions, &home] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::create_dir_all(project.join(".aida/headless-logs")).unwrap();
    Fixture {
        _tmp: tmp,
        project,
        tree,
        sessions,
        home,
    }
}

#[test]
fn recorded_resume_places_rollout_log_transcript_and_lease_ids_and_refuses_the_rest() {
    // HOME is swapped below: run in an isolated child with its own fake HOME.
    // trace:BUG-2386 | ai:claude
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = fixture();
    let env = EnvVarsGuard::apply(&[
        ("AIDA_CODEX_SESSIONS_DIR", fx.sessions.to_str()),
        ("HOME", fx.home.to_str()),
        ("AIDA_HEADLESS_VENDOR", Some("claude")),
    ]);
    // 1. A Codex thread id (it has a rollout) resumes on Codex as itself.
    write_rollout(
        &fx.sessions,
        "019a-rollout",
        &fx.tree,
        "2026-11-01T12:00:00Z",
    );
    assert_eq!(
        recorded_resume(&fx.project, "019a-rollout", None).unwrap(),
        (HeadlessVendor::Codex, "019a-rollout".to_string())
    );
    // 2. An AIDA id whose headless log started a thread resumes that thread.
    std::fs::write(
        fx.project
            .join(".aida/headless-logs/branch-aida-log-1.jsonl"),
        "{\"type\":\"thread.started\",\"thread_id\":\"019a-from-log\"}\n",
    )
    .unwrap();
    assert_eq!(
        recorded_resume(&fx.project, "aida-log-1", None).unwrap(),
        (HeadlessVendor::Codex, "019a-from-log".to_string())
    );
    // 3. An id Claude recorded a transcript for is a Claude session.
    let claude_dir = fx.home.join(".claude/projects/-some-project");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(claude_dir.join("claude-sid.jsonl"), "{}\n").unwrap();
    assert_eq!(
        recorded_resume(&fx.project, "claude-sid", None).unwrap(),
        (HeadlessVendor::Claude, "claude-sid".to_string())
    );
    // 4. A Codex lane resumed by its lease id: the scope's stamped lease
    // binds the thread, never a `claude --resume` on a non-Claude id.
    write_lease(
        &fx.project,
        "lease-codex",
        "BUG-1",
        &fx.tree,
        "2026-11-01T00:00:00Z",
        "vendor_session_id = \"019a-stamped\"\n",
    );
    assert_eq!(
        recorded_resume(&fx.project, "lease-codex", Some("BUG-1")).unwrap(),
        (HeadlessVendor::Codex, "019a-stamped".to_string())
    );
    // 4b. A Codex lane's AIDA id whose log never got to `thread.started`:
    // the log name ties the id to the lease's branch, so the stamp answers.
    std::fs::write(
        fx.project
            .join(".aida/headless-logs/b-aida-no-thread.jsonl"),
        "",
    )
    .unwrap();
    assert_eq!(
        recorded_resume(&fx.project, "aida-no-thread", Some("BUG-1")).unwrap(),
        (HeadlessVendor::Codex, "019a-stamped".to_string())
    );
    // 4c. A foreign or mistyped id belongs to no lease in the scope: it falls
    // through to the configured vendor and never resumes the scope's thread.
    assert_eq!(
        recorded_resume(&fx.project, "aida-typo", Some("BUG-1")).unwrap(),
        (HeadlessVendor::Claude, "aida-typo".to_string())
    );
    // 5. Nothing places the id: it resumes on the configured vendor (Claude
    // by default), so resume does not regress for an id no transcript names.
    assert_eq!(
        recorded_resume(&fx.project, "unknown-id", None).unwrap(),
        (HeadlessVendor::Claude, "unknown-id".to_string())
    );
    assert_eq!(
        recorded_resume(&fx.project, "unknown-id", Some("BUG-other")).unwrap(),
        (HeadlessVendor::Claude, "unknown-id".to_string())
    );
    drop(env);
    // 6. A conflict refuses by name: Codex is configured but nothing binds a
    // thread to the id.
    let _env = EnvVarsGuard::apply(&[
        ("AIDA_CODEX_SESSIONS_DIR", fx.sessions.to_str()),
        ("HOME", fx.home.to_str()),
        ("AIDA_HEADLESS_VENDOR", Some("codex")),
    ]);
    let err = recorded_resume(&fx.project, "unknown-id", Some("BUG-other"))
        .expect_err("configured Codex with no bound thread refuses");
    assert!(
        format!("{err:#}").contains("cannot tell which agent recorded session `unknown-id`"),
        "{err:#}"
    );
    // 7. Under configured Codex, a foreign id in a scope whose lease binds a
    // thread still refuses: the scope's newest thread is never a guess.
    let err = recorded_resume(&fx.project, "aida-typo", Some("BUG-1"))
        .expect_err("a foreign id never resumes the scope's thread");
    assert!(
        format!("{err:#}").contains("cannot tell which agent recorded session `aida-typo`"),
        "{err:#}"
    );
}

// ── the rollout fallback is time-bounded ───────────────────────────────────

#[test]
fn rollout_fallback_refuses_a_rollout_from_before_the_lease_started() {
    // HOME is swapped below: run in an isolated child with its own fake HOME.
    // trace:BUG-2386 | ai:claude
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = fixture();
    let _env = EnvVarsGuard::apply(&[
        ("AIDA_CODEX_SESSIONS_DIR", fx.sessions.to_str()),
        ("HOME", fx.home.to_str()),
    ]);
    // The lane path is reused: the only rollout in it is the previous run's.
    write_rollout(
        &fx.sessions,
        "019a-old-run",
        &fx.tree,
        "2026-10-01T00:00:00Z",
    );
    write_lease(
        &fx.project,
        "lease-new",
        "BUG-1",
        &fx.tree,
        "2026-11-01T00:00:00Z",
        "",
    );
    let lookup = ThreadLookup {
        project_root: &fx.project,
        session_id: "aida-sid",
        lease_id: Some("lease-new"),
        worktree: &fx.tree,
        since: None,
    };
    let err = resolve_thread_id(&lookup).expect_err("a pre-launch rollout is not this run's");
    assert!(
        format!("{err:#}").contains("no Codex thread is bound"),
        "{err:#}"
    );
    // A rollout started after the lease is this run's thread.
    write_rollout(
        &fx.sessions,
        "019a-this-run",
        &fx.tree,
        "2026-11-01T06:00:00Z",
    );
    assert_eq!(resolve_thread_id(&lookup).unwrap(), "019a-this-run");
    // No lease, no log, no caller bound: the launch time is unknown, refuse.
    let unbounded = ThreadLookup {
        lease_id: None,
        ..lookup
    };
    let err = resolve_thread_id(&unbounded).expect_err("no launch bound refuses");
    assert!(
        format!("{err:#}").contains("launch time is unknown"),
        "{err:#}"
    );
}

// ── containment ─────────────────────────────────────────────────────────────

#[test]
fn codex_resume_command_honours_a_flag_only_containment_request() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _env = EnvVarsGuard::apply(&[
        ("AIDA_HOME", home.path().to_str()),
        ("AIDA_CONTAINED", None),
        ("AIDA_OS_WRAP", None),
    ]);
    let _cwd = session::contained_cwd_seam::pin(root.path());
    // Opted in, but the project is not always-contained: only the caller's
    // `--sandbox` asks for containment.
    codex_native_policy(root.path(), false);
    // `--sandbox` alone (no env, no knob) still runs Codex's native sandbox,
    // and a bypass request never widens a contained resume.
    let cmd = session::build_codex_resume_command("019a-thread", true, false, None, true).unwrap();
    let args = args_of(&cmd);
    assert!(
        args.windows(2)
            .any(|w| w == ["--sandbox", "workspace-write"]),
        "{args:?}"
    );
    assert!(
        !args.iter().any(|a| a.contains("dangerously-bypass")),
        "{args:?}"
    );
    assert!(
        args.windows(2).any(|w| w == ["resume", "019a-thread"]),
        "{args:?}"
    );
    // Without the flag, the bypass request maps to Codex's bypass flag.
    let cmd = session::build_codex_resume_command("019a-thread", true, false, None, false).unwrap();
    assert!(
        args_of(&cmd)
            .iter()
            .any(|a| a.contains("dangerously-bypass")),
        "{:?}",
        args_of(&cmd)
    );
    // BUG-2394: a resumed `--plan-only` lane keeps the read-only sandbox on
    // both postures; bypass and workspace-write never survive it.
    for contained in [true, false] {
        let cmd = session::build_codex_resume_command("019a-thread", true, true, None, contained)
            .unwrap();
        let args = args_of(&cmd);
        assert!(
            args.windows(2).any(|w| w == ["--sandbox", "read-only"]),
            "{args:?}"
        );
        assert!(
            !args
                .iter()
                .any(|a| a.contains("dangerously-bypass") || a == "workspace-write"),
            "{args:?}"
        );
        assert!(
            args.windows(2).any(|w| w == ["resume", "019a-thread"]),
            "{args:?}"
        );
    }
}

#[test]
fn contained_parent_resumes_an_opted_in_codex_session_natively() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _env = EnvVarsGuard::apply(&[
        ("AIDA_HOME", home.path().to_str()),
        ("AIDA_CONTAINED", Some("1")),
        ("AIDA_OS_WRAP", None),
    ]);
    let _cwd = session::contained_cwd_seam::pin(root.path());
    codex_native_policy(root.path(), true);
    let target = ResumeTarget {
        agent: "codex".to_string(),
        id: "019a-thread".to_string(),
        worktree: None,
    };
    let cmd = session::build_contained_resume_command(&target, root.path()).unwrap();
    let args = args_of(&cmd);
    assert!(
        args.windows(2)
            .any(|w| w == ["--sandbox", "workspace-write"]),
        "{args:?}"
    );
    assert!(
        !args.iter().any(|a| a.contains("dangerously-bypass")),
        "{args:?}"
    );
    assert!(
        cmd.get_envs()
            .any(|(k, v)| k == "AIDA_CONTAINED" && v.is_none()),
        "AIDA_CONTAINED is removed from the Codex child"
    );
    assert_eq!(cmd.get_current_dir(), Some(root.path()));
}

#[test]
fn session_resume_codex_arm_builds_a_codex_resume_command() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let _env = EnvVarsGuard::apply(&[
        ("AIDA_HOME", home.path().to_str()),
        ("AIDA_CONTAINED", None),
        ("AIDA_AGENT_CMD", None),
    ]);
    let _cwd = session::contained_cwd_seam::pin(root.path());
    let target = ResumeTarget {
        agent: "codex".to_string(),
        id: "019a-thread".to_string(),
        worktree: None,
    };
    let cmd = session::codex_session_resume_command(
        crate::seat_clone::launcher_main().as_deref(),
        &target,
        false,
    )
    .unwrap();
    assert!(
        cmd.get_program().to_string_lossy().ends_with("codex"),
        "{:?}",
        cmd.get_program()
    );
    let args = args_of(&cmd);
    assert!(
        args.windows(2).any(|w| w == ["resume", "019a-thread"]),
        "{args:?}"
    );
    assert!(
        !args.iter().any(|a| a.contains("dangerously-bypass")),
        "native: {args:?}"
    );
    // A resumed Codex child is finalized through codex_tool_env::prepare_launch
    // like a fresh one: an explicit (here baseline) grant decision and its
    // public metadata reach the child's tool shells.
    // trace:BUG-2386 | ai:claude
    for expected in [
        "shell_environment_policy.set.AIDA_SESSION_GRANT=\"\"",
        "shell_environment_policy.set.AIDA_AGENT_TYPE=\"codex\"",
    ] {
        assert!(
            args.windows(2).any(|w| w[0] == "-c" && w[1] == expected),
            "{expected}: {args:?}"
        );
    }
    assert!(
        args.iter()
            .any(|a| a.starts_with("shell_environment_policy.set.AIDA_USER=")),
        "{args:?}"
    );
    assert!(
        cmd.get_envs()
            .any(|(k, v)| k == crate::gate::seat_authority::GRANT_ENV
                && v == Some(std::ffi::OsStr::new(""))),
        "explicit baseline grant decision on the child"
    );
    // A flag-shaped id is refused, never passed as a positional.
    let bad = ResumeTarget {
        id: "--last".to_string(),
        ..target
    };
    assert!(session::codex_session_resume_command(
        crate::seat_clone::launcher_main().as_deref(),
        &bad,
        false
    )
    .is_err());
}

// ── headless Codex resume spawn ─────────────────────────────────────────────

#[test]
fn headless_resume_refuses_agy_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    let tee = crate::headless_tee::TeeOptions {
        enabled: false,
        label: None,
    };
    let err = session::spawn_vendor_headless_resume_granted(
        crate::seat_clone::launcher_main().as_deref(),
        &session::HeadlessResume {
            vendor: HeadlessVendor::Agy,
            session_id: "sid",
            resume_id: "sid",
        },
        "p",
        &tmp.path().join("log.jsonl"),
        tmp.path(),
        &tee,
        false,
        "implementer",
    )
    .expect_err("agy has no resume");
    assert!(format!("{err:#}").contains("agy session cannot be resumed"));
    assert!(
        !tmp.path().join("log.jsonl").exists(),
        "refused before the log"
    );
}

#[cfg(unix)]
#[test]
fn headless_codex_resume_gets_grant_role_native_posture_and_no_aida_contained() {
    // The fake answers the BUG-2398 seat preflight through the real CLI.
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap().join("lane");
    std::fs::create_dir_all(&root).unwrap();
    codex_native_policy(&root, false);
    let fake = tmp.path().join("fake-codex");
    crate::test_exec::write_executable(
        &fake,
        crate::test_exec::codex_probe_script(
            "#!/bin/sh\nprintf '%s\\n' \"posture=$AIDA_CONTAINMENT_POSTURE\" \
             \"contained=${AIDA_CONTAINED-unset}\" \"role=$AIDA_SESSION_ROLE\" \
             \"grant=$AIDA_SESSION_GRANT\" \"$@\"\n",
        ),
    );
    // A contained parent: the Codex child resumes under its native sandbox.
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &root,
        "advisor",
        &["implementer"],
        &[
            ("AIDA_AGENT_CMD", Some(fake.to_str().unwrap())),
            ("AIDA_OS_WRAP", Some("0")),
            ("AIDA_CONTAINED", Some("1")),
        ],
    );
    let _cwd = session::contained_cwd_seam::pin(&root);
    let log = tmp.path().join("resume.jsonl");
    let tee = crate::headless_tee::TeeOptions {
        enabled: false,
        label: None,
    };
    let (status, grant) = session::spawn_vendor_headless_resume_granted(
        crate::seat_clone::launcher_main().as_deref(),
        &session::HeadlessResume {
            vendor: HeadlessVendor::Codex,
            session_id: "aida-sid",
            resume_id: "019a-thread",
        },
        "carry on",
        &log,
        &root,
        &tee,
        false,
        "implementer",
    )
    .unwrap();
    assert!(status.success());
    let out = std::fs::read_to_string(&log).unwrap();
    assert!(out.contains("posture=native\n"), "{out}");
    assert!(
        out.contains("contained=unset\n"),
        "AIDA_CONTAINED removed: {out}"
    );
    assert!(out.contains("role=implementer\n"), "{out}");
    assert!(out.contains(&format!("grant={grant}\n")), "{out}");
    assert!(out.contains("--sandbox\nworkspace-write\n"), "{out}");
    assert!(out.contains("exec\nresume\n"), "{out}");
    assert!(out.contains("\n019a-thread\n"), "{out}");
    assert!(!out.contains("dangerously-bypass"), "{out}");
    // BUG-2398 finalization: the child's tool shells receive its seat.
    assert!(
        out.contains(&format!(
            "shell_environment_policy.set.AIDA_SESSION_GRANT=\"{grant}\"\n"
        )),
        "{out}"
    );
    assert!(
        out.contains("shell_environment_policy.set.AIDA_SESSION_ROLE=\"implementer\"\n"),
        "{out}"
    );
}

// ── punt resume vendor ──────────────────────────────────────────────────────

#[test]
fn punt_resume_uses_the_implementer_vendor_over_config() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
    std::fs::write(
        tmp.path().join(".aida/agents.toml"),
        "[agents]\nvendor = \"claude\"\n",
    )
    .unwrap();
    assert_eq!(
        crate::phase_driver_trait::punt_resume_vendor(Some(HeadlessVendor::Codex), tmp.path()),
        HeadlessVendor::Codex,
        "the vendor the implementer ran on wins over the configured one"
    );
    assert_eq!(
        crate::phase_driver_trait::punt_resume_vendor(None, tmp.path()),
        session::resolve_headless_vendor(tmp.path())
    );
}
