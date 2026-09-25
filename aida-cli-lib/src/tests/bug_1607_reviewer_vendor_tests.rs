//! BUG-1607: integration coverage for the reviewer-vendor resolution +
//! launch wiring, exercising the REAL entry points end to end — not just
//! the pure `interactive_reviewer_launch_plan`/`preflight_*` helpers. A
//! codex-only project (`[agents] enabled = ["codex"]`) must actually spawn
//! `codex`, not `claude`, from both:
//!   - `run_standalone_reviewer` (the `aida queue work --role reviewer
//!     --vendor codex` launch), and
//!   - `review_spec_resolve_vendor` + `review_spec_launch_reviewer` (the
//!     `aida review` / `aida human review` launch kernel
//!     `handle_review_spec` calls — `handle_review_spec` itself cannot run
//!     under `cargo test` because it gates on a real TTY, so these two
//!     functions carry its exact vendor-resolution and launch code).
//!
//! Every spawn here is a tiny local mock script installed via
//! `AIDA_AGENT_CMD` — the BUG-705 mock seam `session::tests` already uses
//! — so no real `claude`/`codex`/`agy` binary is ever required or invoked.
//!
//! All env mutation for a test goes through ONE `EnvVarsGuard::apply` call:
//! the guard holds the shared `ENV_LOCK` for its whole lifetime and is not
//! reentrant, so every key (`AIDA_AGENT_CMD` included) must be set together.
use super::*;

/// Write a trivial "vendor" mock at `dir/mock-agent.sh` that appends each of
/// its argv[1..] to `capture`, one per line, then exits 0.
// trace:BUG-1607 | ai:claude
fn write_argv_capture_mock(dir: &std::path::Path, capture: &std::path::Path) -> std::path::PathBuf {
    let script = dir.join("mock-agent.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> \"{}\"; done\nexit 0\n",
            capture.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    script
}

fn read_captured_argv(capture: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(capture)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// A codex-only project fixture: `.aida/config.toml` enabling ONLY codex,
/// under an isolated `home` dir with nothing configured — mirrors BUG-898's
/// `resolve_enabled_headless_vendor_autopicks_single_enabled_profile`
/// fixture. Returns `(home, project)`.
fn codex_only_project(tmp: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = tmp.join("home");
    let project = tmp.join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/config.toml"),
        "[agents]\nenabled = [\"codex\"]\n",
    )
    .unwrap();
    (home, project)
}

/// BUG-1607: `aida queue work --role reviewer --vendor codex`'s standalone
/// reviewer launch — `run_standalone_reviewer`'s interactive (`no_human =
/// false`) Fresh arm, the exact site that used to hardcode `claude`.
///
/// Resolves the vendor from a REAL codex-only project fixture (the same
/// `resolve_enabled_headless_vendor` call `handle_queue_work` makes before
/// `session_start`), pre-creates the reviewer worktree dir (mirroring
/// `session_start` having already run by the time this function is called
/// in production), then calls the REAL `run_standalone_reviewer` and
/// asserts the mock "codex" was spawned with exactly `codex_session_args`'
/// argv — after the worktree already existed.
// trace:BUG-1607 | ai:claude
#[test]
fn run_standalone_reviewer_with_codex_only_project_launches_codex_after_worktree_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, project) = codex_only_project(tmp.path());
    let capture = tmp.path().join("captured-argv.txt");
    let mock = write_argv_capture_mock(tmp.path(), &capture);

    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HEADLESS_VENDOR", None),
        ("AIDA_HOME", Some(home.to_str().unwrap())),
        ("HOME", Some(home.to_str().unwrap())),
        ("AIDA_AGENT_CMD", Some(mock.to_str().unwrap())),
    ]);
    crate::session::set_headless_vendor_override(None);

    // Resolve the vendor from the REAL codex-only project config — the same
    // call `handle_queue_work` makes before `session_start`.
    let launch_vendor = crate::session::resolve_enabled_headless_vendor(&project).unwrap();
    assert_eq!(launch_vendor, crate::session::HeadlessVendor::Codex);

    // The worktree `session_start` would already have minted by the time
    // `run_standalone_reviewer` runs in production — pre-created here, so
    // "launches codex AFTER the worktree is created" is literal: it exists
    // before the launch call below.
    let worktree = tmp.path().join("reviewer-worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    assert!(worktree.is_dir(), "worktree must exist before launch");

    let verdict_path = tmp.path().join("verdict.json");
    let prompt = "/aida-review --pr 59";

    run_standalone_reviewer(
        &project,
        59,
        QueueWorkLaunch::Fresh("019e0000-0000-7000-8000-0000000000bb".to_string()),
        prompt,
        None,
        "STORY-59",
        "review/story-59",
        "reviewer",
        &worktree,
        false, // no_human: false — the interactive launch (the actual bug)
        true,  // quiet: suppress the end-of-command summary noise
        &verdict_path,
        false,
        launch_vendor,
    )
    .expect("standalone reviewer launch with a reachable mock must succeed");

    let argv = read_captured_argv(&capture);
    assert_eq!(
        argv,
        crate::session::codex_session_args(prompt, false, None),
        "run_standalone_reviewer must spawn codex with codex_session_args' argv, not claude"
    );

    crate::session::set_headless_vendor_override(None);
}

/// BUG-1607: the `aida review <spec>` / `aida human review <spec>` launch
/// kernel — `review_spec_resolve_vendor` (resolves + preflights, the code
/// `handle_review_spec` runs before acquiring its review lease) followed by
/// `review_spec_launch_reviewer` (the code it runs at the actual launch
/// site). Same codex-only fixture + `AIDA_AGENT_CMD` mock as above; this is
/// `handle_review_spec`'s real vendor-resolution-and-launch code, not a
/// re-implemented stand-in for it — `handle_review_spec` itself cannot run
/// here because it gates on a real interactive TTY.
// trace:BUG-1607 | ai:claude
#[test]
fn review_spec_resolve_and_launch_uses_codex_for_codex_only_project() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, project) = codex_only_project(tmp.path());
    let capture = tmp.path().join("captured-argv.txt");
    let mock = write_argv_capture_mock(tmp.path(), &capture);

    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HEADLESS_VENDOR", None),
        ("AIDA_HOME", Some(home.to_str().unwrap())),
        ("HOME", Some(home.to_str().unwrap())),
        ("AIDA_AGENT_CMD", Some(mock.to_str().unwrap())),
    ]);
    crate::session::set_headless_vendor_override(None);

    // The exact resolution `handle_review_spec` runs before acquiring its
    // review lease.
    let vendor = review_spec_resolve_vendor(&project, false)
        .expect("codex-only project must resolve cleanly")
        .expect("no_agent is false, so a vendor must be resolved");
    assert_eq!(vendor, crate::session::HeadlessVendor::Codex);

    let prompt = "/aida-review --pr 52";
    let status = review_spec_launch_reviewer(vendor, "review-story-52", prompt, "session-52")
        .expect("launch with a reachable mock must succeed");
    assert!(status.success());

    let argv = read_captured_argv(&capture);
    assert_eq!(
        argv,
        crate::session::codex_session_args(prompt, false, None),
        "review_spec_launch_reviewer must spawn codex with codex_session_args' argv, not claude"
    );

    crate::session::set_headless_vendor_override(None);
}

/// BUG-1607: `no_agent` never resolves or launches anything — including on
/// a codex-only project — matching `handle_review_spec`'s degrade-honest
/// `--no-agent` path.
// trace:BUG-1607 | ai:claude
#[test]
fn review_spec_resolve_vendor_no_agent_skips_resolution() {
    let tmp = tempfile::tempdir().unwrap();
    let (_home, project) = codex_only_project(tmp.path());
    assert_eq!(review_spec_resolve_vendor(&project, true).unwrap(), None);
}
