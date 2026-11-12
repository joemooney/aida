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

/// Runs `git -C repo ARGS` with detached auto-maintenance off, so a fixture
/// commit never leaves a background `git maintenance` behind (BUG-2191).
// trace:TASK-2000 | ai:claude
fn git(repo: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new(real_git())
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
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

/// The shared resolvers preserve the main root and `origin/main` even
/// when worktree enumeration or `git symbolic-ref` is slow. The
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

    // The common-directory fast path need not enumerate worktrees at all.
    // Root identity, rather than paying for that scan, is the contract here.
    // trace:BUG-2540 | ai:codex
    let root = crate::main_worktree_root_from(&linked);
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
/// per-call budgets alone let the phase run for their sum: the log (0.3 s),
/// the shipped check (0.6 s of its 1 s), four branch searches (1.2 s of
/// their 2 s), and the worktree probe (0.3 s), 2.4 s in all. The one show
/// deadline stops the phase at 1.2 s, and what it cut short is labelled
/// unknown.
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
    // trace:BUG-2466 | ai:codex — exercise the production show budget.
    let show = collect_git_linkage_until(&repo, &ids, Instant::now() + SHOW_LINKAGE_DEADLINE);
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(2000),
        "the phase must stop near its 1.2 s deadline, not run the 2.4 s sum of its budgets: {elapsed:?}"
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

/// TASK-2000: MCP `show_requirement` renders linkage under the show deadline
/// too. With `symbolic-ref` slower than the shipped-check budget the
/// unbounded collector still reports the spec shipped, but the MCP section
/// reports the state unknown (and says why) like the CLI show surfaces,
/// instead of waiting out the slow default-branch resolution.
// trace:TASK-2000 | ai:claude
#[cfg(unix)]
#[test]
fn mcp_show_linkage_runs_under_the_show_deadline() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = shipped_repo(tmp.path());
    let _path = slow_git_on_path(tmp.path(), &["symbolic-ref"], 1.5);

    let md = crate::mcp::render_git_linkage_md(&repo, "TASK-7", false);
    assert!(md.contains("- **State:** unknown ("), "{md}");
    assert!(
        md.contains(&format!("- **Incomplete:** {LINKAGE_SHIPPED_UNKNOWN_NOTE}")),
        "{md}"
    );
    assert!(!md.contains("shipped (merged"), "{md}");
}

/// TASK-1999 (folded into TASK-2000): the bounded default-branch resolvers
/// share one candidate walk with their unbounded twins, so with time to spare
/// both agree on every repo shape: `origin/HEAD` set, `init.defaultBranch`
/// naming a local-only branch, a master-only repo, and no candidate at all.
// trace:TASK-2000 trace:TASK-1999 | ai:claude
#[test]
fn bounded_default_branch_resolvers_match_their_unbounded_twins() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let init = |name: &str, branch: &str| -> PathBuf {
        let repo = tmp.path().join(name);
        std::fs::create_dir_all(&repo).unwrap();
        git(
            &repo,
            &["init", &format!("--initial-branch={branch}"), "-q"],
        );
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        repo
    };

    let with_origin_head = shipped_repo(tmp.path());
    let trunk = init("trunk", "trunk");
    git(&trunk, &["config", "init.defaultBranch", "trunk"]);
    let master_only = init("master-only", "master");
    git(&master_only, &["config", "init.defaultBranch", "master"]);
    let none = init("none", "devel");
    git(&none, &["config", "init.defaultBranch", "devel2"]);

    let cases = [
        (&with_origin_head, Some("origin/main")),
        (&trunk, Some("trunk")),
        (&master_only, Some("master")),
        (&none, None),
    ];
    for (repo, want) in cases {
        let far = Instant::now() + Duration::from_secs(30);
        let unbounded = crate::resolve_default_branch_ref(repo);
        assert_eq!(unbounded.as_deref(), want, "{}", repo.display());
        assert_eq!(
            crate::resolve_default_branch_ref_until(repo, far),
            Ok(unbounded),
            "{}",
            repo.display()
        );
        assert_eq!(
            crate::shipped_linkage::default_branch_refs_until(repo, far).ok(),
            Some(crate::shipped_linkage::default_branch_refs(repo)),
            "{}",
            repo.display()
        );
    }
}
