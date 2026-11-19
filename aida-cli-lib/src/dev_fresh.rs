//! `aida dev check-fresh`: is this binary too far behind `origin/main` to
//! launch lanes? A lane host whose live binary stopped being promoted (no
//! autobuild, a failing fast-forward, CI that never settles) otherwise keeps
//! running lanes without the latest fixes and admission rules.
//!
//! Exit codes are the contract the lane wrapper (`scripts/lane-run.sh`) reads:
//! 0 fresh, 10 stale (refuse), 3 freshness unknown (warn and continue). Stale
//! has its own code so a generic failure (1) or a clap usage error (2) from a
//! binary that predates this command never reads as a refusal.
// trace:TASK-3812 | ai:claude

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// Exit code for a binary that is too far behind `origin/main`.
pub(crate) const EXIT_STALE: i32 = 10;
/// Exit code when freshness cannot be judged.
pub(crate) const EXIT_UNKNOWN: i32 = 3;

/// Limits past which a binary counts as stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FreshLimits {
    /// Most `origin/main` first-parent commits (merged changes) the binary
    /// may lack.
    pub max_behind: u32,
    /// Most hours the oldest missing `origin/main` commit may have waited.
    pub max_hours: u32,
}

/// What the check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Freshness {
    /// The binary has every `origin/main` commit, or is within both limits.
    Fresh { behind: u32, hours: u32 },
    /// The binary exceeds a limit.
    Stale { behind: u32, hours: u32 },
    /// Freshness cannot be judged (the reason is user-facing).
    Unknown(String),
}

/// Pure decision: `oldest_missing_secs` is the commit time of the oldest
/// `origin/main` first-parent commit the binary lacks (None when none).
pub(crate) fn assess(
    behind: u32,
    oldest_missing_secs: Option<i64>,
    now_secs: i64,
    limits: FreshLimits,
) -> Freshness {
    let hours = match (behind, oldest_missing_secs) {
        (0, _) | (_, None) => 0,
        // A commit dated in the future (clock skew) counts as zero hours.
        (_, Some(t)) => u32::try_from(now_secs.saturating_sub(t).max(0) / 3600).unwrap_or(u32::MAX),
    };
    if behind > limits.max_behind || hours > limits.max_hours {
        Freshness::Stale { behind, hours }
    } else {
        Freshness::Fresh { behind, hours }
    }
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let out = aida_core::git_ops::host_git_command(repo)
        .args(args)
        // A hook-exported GIT_DIR would silently redirect `-C`.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Compare `binary_sha` with the repo's local `origin/main` ref.
pub(crate) fn check(
    repo: &Path,
    binary_sha: &str,
    now_secs: i64,
    limits: FreshLimits,
) -> Freshness {
    let sha = binary_sha.trim();
    if sha.is_empty() || sha == "unknown" || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Freshness::Unknown(format!(
            "this binary has no build commit recorded ({sha:?})"
        ));
    }
    let Some(target) = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "refs/remotes/origin/main^{commit}",
        ],
    ) else {
        return Freshness::Unknown(format!("{} has no origin/main ref", repo.display()));
    };
    let Some(full) = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{sha}^{{commit}}"),
        ],
    ) else {
        return Freshness::Unknown(format!(
            "build commit {sha} is not in {}; fetch it, or rebuild there",
            repo.display()
        ));
    };
    let range = format!("{full}..{target}");
    let Some(listing) = git(repo, &["log", "--first-parent", "--format=%ct", &range]) else {
        return Freshness::Unknown(format!("cannot list origin/main commits after {sha}"));
    };
    let times: Vec<i64> = listing
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    let behind = u32::try_from(times.len()).unwrap_or(u32::MAX);
    // `git log` lists newest first, so the last entry is the oldest missing.
    assess(behind, times.last().copied(), now_secs, limits)
}

fn resolve_repo(repo: Option<&str>) -> Result<PathBuf> {
    match repo {
        Some(p) => Ok(PathBuf::from(p)),
        None => crate::find_main_worktree_root()
            .or_else(|_| std::env::current_dir().context("cannot read the current directory")),
    }
}

/// `aida dev check-fresh`. Prints one line and exits with the code above.
pub(crate) fn handle_check_fresh(
    repo: Option<&str>,
    fetch: bool,
    limits: FreshLimits,
) -> Result<()> {
    let repo = match resolve_repo(repo) {
        Ok(repo) => repo,
        Err(e) => {
            eprintln!("unknown: cannot find the checkout to compare with: {e:#}");
            std::process::exit(EXIT_UNKNOWN);
        }
    };
    if fetch && git(&repo, &["fetch", "-q", "origin", "main"]).is_none() {
        eprintln!("warning: could not fetch origin/main; comparing with the local ref");
    }
    let sha = crate::build_git_sha();
    let now = chrono::Utc::now().timestamp();
    let verdict = check(&repo, sha, now, limits);
    let dirty = if crate::build_git_dirty() {
        " (built from a dirty tree)"
    } else {
        ""
    };
    let code = match &verdict {
        Freshness::Fresh { behind: 0, .. } => {
            println!("fresh: aida {sha}{dirty} matches origin/main");
            0
        }
        Freshness::Fresh { behind, hours } => {
            println!(
                "fresh: aida {sha}{dirty} is {behind} change(s) and {hours}h behind origin/main \
                 (limits {} changes, {}h)",
                limits.max_behind, limits.max_hours
            );
            0
        }
        Freshness::Stale { behind, hours } => {
            eprintln!(
                "STALE: aida {sha}{dirty} is {behind} change(s) and {hours}h behind origin/main \
                 (limits {} changes, {}h). Rebuild the live binary with `aida dev build` from a \
                 clean main at origin/main in {}.",
                limits.max_behind,
                limits.max_hours,
                repo.display()
            );
            EXIT_STALE
        }
        Freshness::Unknown(why) => {
            eprintln!("unknown: cannot judge whether aida {sha} is current: {why}");
            EXIT_UNKNOWN
        }
    };
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: FreshLimits = FreshLimits {
        max_behind: 10,
        max_hours: 6,
    };

    #[test]
    fn assess_counts_zero_behind_as_fresh_whatever_the_time() {
        assert_eq!(
            assess(0, Some(0), 1_000_000, LIMITS),
            Freshness::Fresh {
                behind: 0,
                hours: 0
            }
        );
    }

    #[test]
    fn assess_refuses_past_either_limit() {
        let now = 100 * 3600;
        assert_eq!(
            assess(10, Some(now - 6 * 3600), now, LIMITS),
            Freshness::Fresh {
                behind: 10,
                hours: 6
            }
        );
        assert_eq!(
            assess(11, Some(now), now, LIMITS),
            Freshness::Stale {
                behind: 11,
                hours: 0
            }
        );
        assert_eq!(
            assess(1, Some(now - 7 * 3600), now, LIMITS),
            Freshness::Stale {
                behind: 1,
                hours: 7
            }
        );
        // Clock skew: a future commit is zero hours old, not a negative wrap.
        assert_eq!(
            assess(1, Some(now + 3600), now, LIMITS),
            Freshness::Fresh {
                behind: 1,
                hours: 0
            }
        );
    }

    fn run(repo: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit_at(repo: &Path, secs: i64) -> String {
        let date = format!("@{secs} +0000");
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["commit", "-q", "--allow-empty", "-m", "c"])
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .expect("git commit runs");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        run(repo, &["rev-parse", "--short", "HEAD"])
    }

    #[test]
    fn check_measures_a_binary_against_the_origin_main_ref() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        run(repo, &["init", "-q", "-b", "main"]);
        let t0 = 1_700_000_000;
        let built = commit_at(repo, t0);
        // Three changes land on main after the build; the first 8 hours ago.
        let now = t0 + 10 * 3600;
        commit_at(repo, now - 8 * 3600);
        commit_at(repo, now - 3600);
        commit_at(repo, now - 60);
        run(repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);

        assert_eq!(
            check(repo, &built, now, LIMITS),
            Freshness::Stale {
                behind: 3,
                hours: 8
            }
        );
        let loose = FreshLimits {
            max_behind: 3,
            max_hours: 8,
        };
        assert_eq!(
            check(repo, &built, now, loose),
            Freshness::Fresh {
                behind: 3,
                hours: 8
            }
        );
        let head = run(repo, &["rev-parse", "--short", "HEAD"]);
        assert_eq!(
            check(repo, &head, now, LIMITS),
            Freshness::Fresh {
                behind: 0,
                hours: 0
            }
        );
    }

    #[test]
    fn check_reports_unknown_instead_of_fresh_when_it_cannot_tell() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        run(repo, &["init", "-q", "-b", "main"]);
        let built = commit_at(repo, 1_700_000_000);
        assert!(
            matches!(check(repo, &built, 0, LIMITS), Freshness::Unknown(why) if why.contains("origin/main"))
        );
        run(repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        assert!(matches!(
            check(repo, "unknown", 0, LIMITS),
            Freshness::Unknown(_)
        ));
        assert!(matches!(check(repo, "", 0, LIMITS), Freshness::Unknown(_)));
        assert!(matches!(
            check(repo, "--all", 0, LIMITS),
            Freshness::Unknown(_)
        ));
        assert!(
            matches!(check(repo, "deadbeef", 0, LIMITS), Freshness::Unknown(why) if why.contains("not in"))
        );
    }
}
