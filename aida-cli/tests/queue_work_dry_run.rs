//! TASK-1053: `aida queue work <spec> --dry-run` previews the single-spec
//! plan and creates NOTHING.
//!
//! This end-to-end test drives the real `aida` binary in a throwaway git repo
//! and asserts both halves of the contract:
//!   * the resolved plan is printed (branch, worktree, session id, lease, role);
//!   * no side effect lands — no worktree directory, no session lease, the
//!     convenience auto-queue is NOT persisted, and the spec's status is
//!     unchanged (still Approved, never bumped to In Progress).
// trace:TASK-1053

use std::path::Path;
use std::process::Command;

fn aida(repo: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo);
    // Hermetic: isolated HOME, no telemetry, and a deterministic surface
    // (clear the advisor role + any inherited permission/agent-output env so
    // the plan renders the same regardless of the developer's shell).
    cmd.env("HOME", home);
    cmd.env("AIDA_TELEMETRY", "0");
    cmd.env_remove("AIDA_SESSION_ROLE");
    cmd.env_remove("AIDA_PERMISSION_MODE");
    cmd.env_remove("AIDA_AGENT_OUTPUT");
    cmd
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn single_spec_dry_run_previews_plan_with_no_side_effects() {
    let base = tempfile::tempdir().expect("tempdir");
    // BUG-671: on macOS the tempdir resolves under /var/folders/… which is a
    // symlink to /private/var/folders/…; canonicalize once so every path the
    // test passes matches the path `aida` records internally, on every OS.
    let base_dir = base.path().canonicalize().expect("canonicalize tempdir");
    let repo = base_dir.join("repo");
    let home = base_dir.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    // Throwaway git repo with one commit so HEAD exists.
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);

    // Distributed AIDA project (skip skills/hooks/agent-config/roles for speed).
    let init = aida(&repo, &home)
        .args([
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ])
        .output()
        .expect("run aida init");
    assert!(
        init.status.success(),
        "aida init failed (exit {:?}):\n--- stdout ---\n{}\n--- stderr ---\n{}",
        init.status.code(),
        String::from_utf8_lossy(&init.stdout),
        String::from_utf8_lossy(&init.stderr)
    );

    // File an Approved spec. The advisor role clears the non-TTY approve gate
    // (TASK-647); the rest of the test runs role-free.
    let add = aida(&repo, &home)
        .env("AIDA_SESSION_ROLE", "advisor")
        .args([
            "add",
            "--type",
            "task",
            "--status",
            "approved",
            "--title",
            "preview me",
        ])
        .output()
        .expect("run aida add");
    assert!(
        add.status.success(),
        "aida add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    let add_out = String::from_utf8_lossy(&add.stdout);
    let spec = add_out
        .split(|c: char| c.is_whitespace() || c == ',')
        .find(|t| is_spec_id(t))
        .unwrap_or_else(|| panic!("could not parse spec id from add output:\n{add_out}"))
        .to_string();

    // Capture the pre-state so we can prove the dry run mutated nothing.
    let sessions_before = list_session_files(&repo);
    let siblings_before = sibling_worktrees(&base_dir);

    // The dry run.
    let dry = aida(&repo, &home)
        .args(["queue", "work", &spec, "--dry-run", "--no-pull"])
        .output()
        .expect("run aida queue work --dry-run");
    assert!(
        dry.status.success(),
        "dry-run exited non-zero ({:?}):\nstderr={}\nstdout={}",
        dry.status.code(),
        String::from_utf8_lossy(&dry.stderr),
        String::from_utf8_lossy(&dry.stdout)
    );

    // --- The plan is printed (goes to stderr, alongside the pre-flight summary). ---
    let plan = String::from_utf8_lossy(&dry.stderr);
    for needle in ["branch:", "worktree:", "session:", "lease:", "dry run"] {
        assert!(
            plan.contains(needle),
            "dry-run plan is missing `{needle}`:\n{plan}"
        );
    }

    // --- No side effects. ---

    // 1. No worktree directory was created (a real pickup would mint a
    //    `<repo>-<slug>` sibling of the project root).
    let siblings_after = sibling_worktrees(&base_dir);
    assert_eq!(
        siblings_before, siblings_after,
        "dry-run created a worktree sibling: before={siblings_before:?} after={siblings_after:?}"
    );

    // 2. No session lease was taken.
    let sessions_after = list_session_files(&repo);
    assert_eq!(
        sessions_before, sessions_after,
        "dry-run wrote a session lease: before={sessions_before:?} after={sessions_after:?}"
    );

    // 3. The convenience auto-queue was NOT persisted — the spec is still not
    //    on the queue after the preview.
    let queue = aida(&repo, &home)
        .args(["queue", "list"])
        .output()
        .expect("run aida queue list");
    let queue_out = String::from_utf8_lossy(&queue.stdout);
    assert!(
        !queue_out.contains(&spec),
        "dry-run persisted the auto-queue ({spec} appears in queue):\n{queue_out}"
    );

    // 4. The spec's status is unchanged — still Approved, never bumped to
    //    In Progress (which is what session_start would have done).
    let show = aida(&repo, &home)
        .args(["show", &spec])
        .output()
        .expect("run aida show");
    let show_out = format!(
        "{}{}",
        String::from_utf8_lossy(&show.stdout),
        String::from_utf8_lossy(&show.stderr)
    )
    .to_lowercase();
    // Match the `status:` field line specifically — the show output also
    // carries next-step hints that mention `in-progress` as a routing label,
    // so a substring search over the whole blob would false-positive.
    let status_line = show_out
        .lines()
        .find(|l| l.trim_start().starts_with("status:"))
        .unwrap_or_else(|| panic!("no status field in show output:\n{show_out}"));
    assert!(
        status_line.contains("approved"),
        "spec status should still be Approved after a dry run, got: `{status_line}`"
    );
}

/// A SPEC-ID is `UPPER-<digits>` (e.g. `TASK-1`).
fn is_spec_id(t: &str) -> bool {
    let mut parts = t.splitn(2, '-');
    match (parts.next(), parts.next()) {
        (Some(prefix), Some(num)) => {
            !prefix.is_empty()
                && prefix.chars().all(|c| c.is_ascii_uppercase())
                && !num.is_empty()
                && num.chars().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

/// Sibling directories of the project root that look like AIDA worktrees
/// (`repo-*`). Empty when no pickup has minted one.
fn sibling_worktrees(base: &Path) -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_dir(base)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.starts_with("repo-"))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

/// Session lease files under `.aida/sessions/`. Empty (or missing dir) when no
/// lease has been taken.
fn list_session_files(repo: &Path) -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_dir(repo.join(".aida").join("sessions"))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}
