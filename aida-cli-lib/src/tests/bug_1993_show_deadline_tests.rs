//! BUG-1993: only `aida show` puts a deadline on linkage git calls. The shared
//! resolvers (`main_worktree_root_from`, `resolve_default_branch_ref`) and the
//! non-show linkage collectors wait out a slow git and return the right
//! answer; the show-only `_until` variants return a distinct timeout, and the
//! whole show linkage phase runs under one deadline, not a sum of per-call
//! budgets. Every test shims `git` on PATH, so each runs in its own process.
// trace:BUG-1993 | ai:claude
use super::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn real_git() -> PathBuf {
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join("git"))
                .find(|p| p.is_file())
        })
        .expect("real git on PATH")
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new(real_git())
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repo whose `main` (mirrored as `origin/main`, with `origin/HEAD`) carries
/// one commit naming TASK-7.
fn shipped_repo(tmp: &Path) -> PathBuf {
    let repo = tmp.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--initial-branch=main", "--quiet"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(
        &repo,
        &["commit", "-q", "--allow-empty", "-m", "fix: x (TASK-7)"],
    );
    git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(
        &repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    repo
}

/// Put a `git` on PATH that sleeps `secs` before running the real git for
/// any of the `slow` subcommands, and passes every other call straight
/// through. The returned guard restores PATH.
#[cfg(unix)]
fn slow_git_on_path(tmp: &Path, slow: &[&str], secs: f64) -> crate::test_env::EnvVarGuard {
    let real = real_git();
    let dir = tmp.join("shim");
    std::fs::create_dir_all(&dir).unwrap();
    let shim = dir.join("git");
    let cases = slow.join("|");
    crate::test_exec::write_executable(
        &shim,
        format!(
            "#!/bin/sh\nfor arg in \"$@\"; do\n  case \"$arg\" in\n    {cases}) sleep {secs}; break ;;\n  esac\ndone\nexec '{}' \"$@\"\n",
            real.display()
        ),
    );
    let mut paths = vec![dir];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    crate::test_env::EnvVarGuard::set("PATH", std::env::join_paths(paths).unwrap())
}

/// The shared resolvers have no deadline: a slow `git worktree list` /
/// `git symbolic-ref` still yields the main root and `origin/main`. The
/// show-only resolver, whose deadline passes while the slow call is in
/// flight, returns the distinct timeout instead of a fallback value.
#[cfg(unix)]
#[test]
fn shared_resolvers_wait_out_a_slow_git_while_the_show_variant_times_out() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = shipped_repo(tmp.path());
    let linked = tmp.path().join("linked");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat",
            linked.to_str().unwrap(),
        ],
    );
    let _path = slow_git_on_path(tmp.path(), &["worktree", "symbolic-ref"], 1.5);

    let started = Instant::now();
    let root = crate::main_worktree_root_from(&linked);
    assert!(
        started.elapsed() >= Duration::from_millis(1500),
        "the shim ran"
    );
    assert_eq!(
        root.canonicalize().unwrap(),
        repo.canonicalize().unwrap(),
        "a slow worktree list must not fall back to the linked worktree"
    );

    assert_eq!(
        crate::resolve_default_branch_ref(&repo).as_deref(),
        Some("origin/main"),
        "a slow symbolic-ref must not read as a different default branch"
    );

    let started = Instant::now();
    let bounded =
        crate::resolve_default_branch_ref_until(&repo, Instant::now() + Duration::from_millis(300));
    assert_eq!(bounded, Err(crate::DefaultBranchResolveTimedOut));
    assert!(
        started.elapsed() < Duration::from_millis(1400),
        "the deadline cut the slow call short: {:?}",
        started.elapsed()
    );
}

/// With `symbolic-ref` slower than the shipped-check budget, the non-show
/// collectors (linkage for `aida human` / drain / MCP, and the completion
/// warning's scan) still find the spec shipped on `origin/main`; only the
/// show variants report the state unknown, never "not shipped" or "no
/// default branch".
#[cfg(unix)]
#[test]
fn only_show_linkage_bounds_default_branch_resolution() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = shipped_repo(tmp.path());
    let ids = vec!["TASK-7".to_string()];
    let _path = slow_git_on_path(tmp.path(), &["symbolic-ref"], 1.5);

    let shared = collect_git_linkage_opts(&repo, &ids, false);
    assert!(shared.shipped && !shared.shipped_unknown, "{shared:?}");

    let show = collect_git_linkage_until(&repo, &ids, Instant::now() + SHOW_LINKAGE_DEADLINE);
    assert!(!show.shipped && show.shipped_unknown, "{show:?}");
    assert!(
        show.incomplete_notes()
            .iter()
            .any(|n| n == LINKAGE_SHIPPED_UNKNOWN_NOTE),
        "{show:?}"
    );

    let warning_scan = crate::shipped_linkage::scan_default_branch_linkage(&repo, &ids);
    assert!(warning_scan.corroborated(), "{warning_scan:?}");

    let toon_scan = crate::shipped_linkage::scan_default_branch_linkage_until(
        &repo,
        &ids,
        Instant::now() + Duration::from_millis(500),
    );
    let line = crate::shipped_linkage::render_toon_linkage(&toon_scan);
    assert!(line.starts_with("shipped=unknown "), "{line}");
    assert!(!line.contains("no default branch"), "{line}");
}

/// Every linkage call is slow but inside its own per-call budget, so the
/// per-call budgets alone let the phase run for their sum: the negative
/// probe's snapshot (0.3 s: its concurrent `log --no-walk` and `worktree
/// list` reads), the log (0.3 s), the shipped check (0.6 s of its 1 s), four
/// branch searches (1.2 s of their 2 s), and the worktree probe (0.3 s),
/// 2.7 s in all. The one show deadline stops the phase at 1.5 s, and what it
/// cut short is labelled unknown.
///
/// BUG-2221: the show path runs the negative probe's snapshot before the
/// log on a cold key, so the deadline sits 0.3 s later than the original
/// 1.2 s — the shipped check still fits with the same margin, and the
/// branch search is still what the deadline cuts. trace:BUG-2221 | ai:claude
#[cfg(unix)]
#[test]
fn show_linkage_phase_runs_under_one_deadline() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = shipped_repo(tmp.path());
    git(&repo, &["checkout", "-q", "-b", "task-8"]);
    for n in 1..=4 {
        git(
            &repo,
            &[
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                &format!("wip {n} (TASK-8)"),
            ],
        );
    }
    let ids = vec!["TASK-8".to_string()];
    let _path = slow_git_on_path(
        tmp.path(),
        &["log", "symbolic-ref", "merge-base", "branch", "worktree"],
        0.3,
    );

    let started = Instant::now();
    let show = collect_git_linkage_until(&repo, &ids, Instant::now() + Duration::from_millis(1500));
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(2000),
        "the phase must stop near its 1.5 s deadline, not run the 2.7 s sum of its budgets: {elapsed:?}"
    );
    assert_eq!(show.commits.len(), 4, "{show:?}");
    assert!(
        !show.shipped_unknown,
        "the shipped check fit its budget: {show:?}"
    );
    assert!(
        show.branch_scan_truncated.is_some(),
        "the deadline cut the branch search short and says so: {show:?}"
    );
}

/// A `git worktree list` slower than its show cap leaves the worktree path
/// unknown, and says so, only under the show deadline; the non-show
/// collector waits for it and finds the checkout.
#[cfg(unix)]
#[test]
fn only_show_linkage_bounds_the_worktree_probe() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = shipped_repo(tmp.path());
    git(&repo, &["checkout", "-q", "-b", "task-8"]);
    git(
        &repo,
        &["commit", "-q", "--allow-empty", "-m", "wip (TASK-8)"],
    );
    let ids = vec!["TASK-8".to_string()];
    let _path = slow_git_on_path(tmp.path(), &["worktree"], 1.5);

    let shared = collect_git_linkage_opts(&repo, &ids, false);
    assert_eq!(shared.branch.as_deref(), Some("task-8"), "{shared:?}");
    assert!(
        shared.worktree.is_some() && !shared.worktree_scan_incomplete,
        "{shared:?}"
    );

    let show = collect_git_linkage_until(&repo, &ids, Instant::now() + SHOW_LINKAGE_DEADLINE);
    assert_eq!(show.branch.as_deref(), Some("task-8"), "{show:?}");
    assert!(
        show.worktree.is_none() && show.worktree_scan_incomplete,
        "{show:?}"
    );
    assert!(
        show.incomplete_notes()
            .iter()
            .any(|n| n == LINKAGE_WORKTREE_SCAN_INCOMPLETE_NOTE),
        "{show:?}"
    );
}
