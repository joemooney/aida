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

use crate::cli::DevCommand;

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
///
/// The age limit compares whole seconds against `max_hours * 3600`; the
/// reported `hours` is rounded down for display only, so a change 6h59m old
/// is stale under a 6h limit even though it reports 6h.
pub(crate) fn assess(
    behind: u32,
    oldest_missing_secs: Option<i64>,
    now_secs: i64,
    limits: FreshLimits,
) -> Freshness {
    let age_secs = match (behind, oldest_missing_secs) {
        (0, _) | (_, None) => 0,
        // A commit dated in the future (clock skew) counts as zero seconds.
        (_, Some(t)) => now_secs.saturating_sub(t).max(0),
    };
    let hours = u32::try_from(age_secs / 3600).unwrap_or(u32::MAX);
    if behind > limits.max_behind || age_secs > i64::from(limits.max_hours) * 3600 {
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

/// `git fetch origin main` with the remote's stored credentials routed, no
/// terminal prompt, and a bound so a hung remote cannot stall a lane launch.
fn fetch_origin_main(repo: &Path) -> bool {
    use aida_core::git_ops::RemoteCredentialsExt as _;
    let mut cmd = aida_core::git_ops::host_git_command(repo);
    cmd.args(["fetch", "-q", "origin", "main"])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .remote_credentials(repo, "origin");
    crate::command_output_with_timeout(cmd, std::time::Duration::from_secs(60))
        .is_some_and(|out| out.status.success())
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

/// The autobuild cron's log: `AIDA_AUTOBUILD_LOG`, else the script's default
/// `~/.aida/autobuild.log`.
// trace:BUG-2787 | ai:claude
pub(crate) fn autobuild_log_path() -> Option<PathBuf> {
    aida_core::exec_context::Environment::capture()
        .get("AIDA_AUTOBUILD_LOG")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::home_dir().map(|h| h.join(".aida/autobuild.log")))
}

/// The trailing run of `skip:` lines in an autobuild log: (first skip time,
/// last skip time, last reason). A promotion or a failure ends a run; other
/// lines (CI notes, fetch output) are neutral. None when the log does not end
/// in skips.
fn skip_streak(log: &str) -> Option<(String, String, String)> {
    let mut streak: Option<(String, String, String)> = None;
    for line in log.lines() {
        let Some(stamp) = line
            .get(..19)
            .filter(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").is_ok())
        else {
            continue;
        };
        let msg = line[19..].trim_start();
        if let Some(reason) = msg.strip_prefix("skip:") {
            let since = streak.take().map_or_else(|| stamp.to_string(), |s| s.0);
            streak = Some((since, stamp.to_string(), reason.trim().to_string()));
        } else if msg.starts_with("promoted and verified") || msg.starts_with("FAILED:") {
            streak = None;
        }
    }
    streak
}

/// The last 256 KiB of the log, starting at a whole line.
fn read_log_tail(path: &Path) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let start = file.metadata()?.len().saturating_sub(256 * 1024);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(match (start, text.find('\n')) {
        (0, _) | (_, None) => text,
        (_, Some(nl)) => text[nl + 1..].to_string(),
    })
}

/// Why the autobuild is not promoting, and the one hint that clears it:
/// (note, hint). None when the log does not end in skips. A missing or
/// unreadable log is reported, never an error. Report only: the primary
/// checkout is never touched.
// trace:BUG-2787 | ai:claude
pub(crate) fn autobuild_note(log: Option<&Path>, primary: &Path) -> Option<(String, String)> {
    let rerun = "then rerun the autobuild (scripts/aida-autobuild.sh)";
    let Some(path) = log else {
        return Some((
            "autobuild log location unknown (no home directory)".into(),
            format!("set AIDA_AUTOBUILD_LOG, {rerun}"),
        ));
    };
    let text = match read_log_tail(path) {
        Ok(text) => text,
        Err(e) => {
            let note = if e.kind() == std::io::ErrorKind::NotFound {
                format!("no autobuild log at {}", path.display())
            } else {
                format!("cannot read autobuild log {}: {e}", path.display())
            };
            return Some((
                note,
                format!(
                    "install the autobuild cron or run `aida dev build` from clean main in {}",
                    primary.display()
                ),
            ));
        }
    };
    let (since, last, reason) = skip_streak(&text)?;
    let hint = if reason.contains("primary checkout must be on main") {
        format!(
            "inspect `git -C {} status`; stash or commit stray changes, {rerun}",
            primary.display()
        )
    } else {
        format!("fix the skip reason above, {rerun}")
    };
    Some((
        format!("autobuild skipping since {since} (last {last}): {reason}"),
        hint,
    ))
}

/// "live binary Nh / M change(s) behind main", with the autobuild note.
pub(crate) fn behind_line(behind: u32, hours: u32, note: Option<&str>) -> String {
    let head = format!("live binary {hours}h / {behind} change(s) behind main");
    note.map_or(head.clone(), |n| format!("{head}; {n}"))
}

fn resolve_repo(repo: Option<&str>) -> Result<PathBuf> {
    match repo {
        Some(p) => Ok(PathBuf::from(p)),
        None => crate::find_main_worktree_root()
            .or_else(|_| std::env::current_dir().context("cannot read the current directory")),
    }
}

/// `aida dev check-fresh`. Prints one line and exits with the code above.
pub(crate) fn handle_check_fresh(cmd: &DevCommand) -> Result<()> {
    let code = check_fresh_exit_code(
        cmd,
        crate::build_git_sha(),
        crate::build_git_dirty(),
        chrono::Utc::now().timestamp(),
    );
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// The parsed command's exit code for a binary built from `sha`: the whole
/// dispatch path except the process exit, so tests drive the real arm mapping.
fn check_fresh_exit_code(cmd: &DevCommand, sha: &str, dirty: bool, now: i64) -> i32 {
    let DevCommand::CheckFresh {
        repo,
        fetch,
        max_behind,
        max_hours,
    } = cmd
    else {
        eprintln!("unknown: not a check-fresh command");
        return EXIT_UNKNOWN;
    };
    let limits = FreshLimits {
        max_behind: *max_behind,
        max_hours: *max_hours,
    };
    match resolve_repo(repo.as_deref()) {
        Ok(repo) => run_check_fresh(&repo, *fetch, limits, sha, dirty, now),
        Err(e) => {
            eprintln!("unknown: cannot find the checkout to compare with: {e:#}");
            EXIT_UNKNOWN
        }
    }
}

/// The command body: print the verdict for `sha` against `repo` and return
/// the process exit code `handle_check_fresh` exits with.
fn run_check_fresh(
    repo: &Path,
    fetch: bool,
    limits: FreshLimits,
    sha: &str,
    dirty: bool,
    now: i64,
) -> i32 {
    if fetch && !fetch_origin_main(repo) {
        eprintln!("warning: could not fetch origin/main; comparing with the local ref");
    }
    let verdict = check(repo, sha, now, limits);
    let dirty = if dirty {
        " (built from a dirty tree)"
    } else {
        ""
    };
    match &verdict {
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
            // trace:BUG-2787 | ai:claude
            if let Some((note, hint)) = autobuild_note(autobuild_log_path().as_deref(), repo) {
                eprintln!("  {}", behind_line(*behind, *hours, Some(&note)));
                eprintln!("  Clear it: {hint}");
            }
            EXIT_STALE
        }
        Freshness::Unknown(why) => {
            eprintln!("unknown: cannot judge whether aida {sha} is current: {why}");
            EXIT_UNKNOWN
        }
    }
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

    fn git_cmd(repo: &Path, args: &[&str]) -> std::process::Command {
        let mut cmd = crate::test_git::git_command();
        cmd.arg("-C")
            .arg(repo)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        cmd
    }

    fn run(repo: &Path, args: &[&str]) -> String {
        let out = git_cmd(repo, args).output().expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit_at(repo: &Path, secs: i64) -> String {
        let date = format!("@{secs} +0000");
        let out = git_cmd(
            repo,
            &["commit", "-q", "--allow-empty", "--no-verify", "-m", "c"],
        )
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
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

    #[test]
    fn assess_compares_seconds_not_rounded_hours() {
        let now = 100 * 3600;
        // 6h00m01s and 6h59m under a 6h limit: stale, though both report 6h.
        for age in [6 * 3600 + 1, 6 * 3600 + 59 * 60] {
            assert_eq!(
                assess(1, Some(now - age), now, LIMITS),
                Freshness::Stale {
                    behind: 1,
                    hours: 6
                },
                "age {age}s"
            );
        }
        // Exactly 6h is still within the limit.
        assert_eq!(
            assess(1, Some(now - 6 * 3600), now, LIMITS),
            Freshness::Fresh {
                behind: 1,
                hours: 6
            }
        );
    }

    // trace:BUG-2787 | ai:claude
    #[test]
    fn stale_binary_names_the_autobuild_skip_streak_and_its_clearing_hint() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        run(repo, &["init", "-q", "-b", "main"]);
        let t0 = 1_700_000_000;
        let built = commit_at(repo, t0);
        let now = t0 + 12 * 3600;
        for age in [11, 6, 1] {
            commit_at(repo, now - age * 3600);
        }
        run(repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        let Freshness::Stale { behind, hours } = check(repo, &built, now, LIMITS) else {
            panic!("the built sha is stale");
        };
        let dirty = "skip: primary checkout must be on main with no visible tracked changes or other hidden entries";
        let log = dir.path().join("autobuild.log");
        std::fs::write(
            &log,
            format!(
                "2026-11-26 23:30:01 promoted and verified: abc\n\
                 2026-11-27 00:00:01 {dirty}\n\
                 From github.com:x/y\n\
                 2026-11-27 00:30:01 newer commits passed over: abc pending\n\
                 2026-11-27 00:30:02 {dirty}\n\
                 2026-11-27 11:30:01 {dirty}\n"
            ),
        )
        .unwrap();
        let (note, hint) = autobuild_note(Some(&log), repo).expect("skipping");
        assert_eq!(
            note,
            "autobuild skipping since 2026-11-27 00:00:01 (last 2026-11-27 11:30:01): \
             primary checkout must be on main with no visible tracked changes or other hidden entries"
        );
        assert!(
            hint.contains(&format!("git -C {} status", repo.display())),
            "{hint}"
        );
        assert_eq!(
            behind_line(behind, hours, Some(&note)),
            format!("live binary 11h / 3 change(s) behind main; {note}")
        );
        // A skip streak broken by a failure, then a success: no skip line.
        std::fs::write(
            &log,
            format!("2026-11-27 00:00:01 {dirty}\n2026-11-27 00:30:01 FAILED: x\n2026-11-27 01:00:01 promoted and verified: abc\n"),
        )
        .unwrap();
        assert_eq!(autobuild_note(Some(&log), repo), None);
        // A missing log is reported, not an error.
        let missing = dir.path().join("absent.log");
        let (note, _) = autobuild_note(Some(&missing), repo).expect("reported");
        assert!(note.contains("no autobuild log at"), "{note}");
    }

    /// Parse `aida dev check-fresh <args>` through the real CLI.
    fn parse(args: &[&str]) -> DevCommand {
        let argv: Vec<&str> = ["aida", "dev", "check-fresh"]
            .into_iter()
            .chain(args.iter().copied())
            .collect();
        crate::TEST_CLI
            .try_parse_from(&argv)
            .unwrap()
            .command
            .registered::<crate::dev_cmd::DevArgs>()
            .inner
    }

    #[test]
    fn command_exit_codes_are_zero_stale_and_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let r = repo.to_str().unwrap();
        run(repo, &["init", "-q", "-b", "main"]);
        let t0 = 1_700_000_000;
        let built = commit_at(repo, t0);
        let now = t0 + 10 * 3600;
        let code = |args: &[&str], sha: &str| check_fresh_exit_code(&parse(args), sha, false, now);
        // No origin/main ref yet: unknown.
        assert_eq!(code(&["--repo", r], &built), EXIT_UNKNOWN);
        run(repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        // Binary at origin/main: fresh (a dirty build too).
        assert_eq!(code(&["--repo", r], &built), 0);
        assert_eq!(
            check_fresh_exit_code(&parse(&["--repo", r]), &built, true, now),
            0
        );
        // One change landed 8h ago: within the default 10 changes, past 6h.
        commit_at(repo, now - 8 * 3600);
        run(repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        assert_eq!(code(&["--repo", r], &built), EXIT_STALE);
        // The flags reach the limits: --max-hours 8 makes it fresh, and
        // --max-behind 0 makes it stale again.
        assert_eq!(code(&["--repo", r, "--max-hours", "8"], &built), 0);
        assert_eq!(
            code(
                &["--repo", r, "--max-hours", "8", "--max-behind", "0"],
                &built
            ),
            EXIT_STALE
        );
        // 6h00m01s past a 6h limit is stale at the command level as well.
        let edge = run(repo, &["rev-parse", "HEAD"]);
        commit_at(repo, now - 6 * 3600 - 1);
        run(repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        assert_eq!(code(&["--repo", r], &edge), EXIT_STALE);
        assert_eq!(code(&["--repo", r, "--max-hours", "7"], &edge), 0);
        // No recorded build commit, and a commit this repo lacks: unknown.
        assert_eq!(code(&["--repo", r], "unknown"), EXIT_UNKNOWN);
        assert_eq!(code(&["--repo", r], "deadbeef"), EXIT_UNKNOWN);
        // A failed fetch (no origin remote) warns and still judges the local ref.
        assert_eq!(code(&["--repo", r, "--fetch"], &built), EXIT_STALE);
        // The exact codes the lane wrapper keys on, distinct from a generic
        // failure (1) and a clap usage error (2).
        assert_eq!((EXIT_STALE, EXIT_UNKNOWN), (10, 3));
    }
}
