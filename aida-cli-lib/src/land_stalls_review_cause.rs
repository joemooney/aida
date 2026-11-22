//! Why a head has no review: the cause the guard can observe on this host.
//!
//! Three things are looked at, all read-only: what the review watcher's
//! service log says about the change in its newest pass (`watcher admission
//! is busy`, a refusal, a deferral), a session lease on the change's key
//! (`PR-N`, or `MR-N` on GitLab) whose session is not live, and a reviewer
//! worktree `aida-pr-N` left from an earlier review.
//!
//! The leases are listed once and each lease's liveness is judged on that
//! same listing, so a reviewer that starts while the guard runs is never
//! taken for a dead session. A path named `aida-pr-N` is called a leftover
//! reviewer worktree only when git lists it as a worktree of this project on
//! branch `pr-N` and no session or agent process is in it. Whatever could
//! not be established (a lease whose liveness is unknown, a path that is not
//! such a worktree, an inspection that failed) is said as an observation and
//! leaves the cause unknown.
// trace:TASK-3827 | ai:claude

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::lane_hosts::routed;

/// The review watcher's service log, below the project root.
pub(crate) const WATCH_LOG: &str = ".aida/review-watch/service.log";
/// How much of the end of the log is read: over an hour of passes.
const LOG_TAIL_BYTES: u64 = 64 * 1024;
/// A pass older than this says nothing about the head as it is now.
pub(crate) const PASS_FRESH_MINS: i64 = 15;
const BUSY: &str = "watcher admission is busy";

/// The newest complete pass in the watcher's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pass {
    pub at: DateTime<Utc>,
    /// What the watcher printed while that pass ran, oldest first.
    pub lines: Vec<String>,
}

/// The newest complete pass in `tail`, the end of the watcher's log. Each
/// pass ends with one line `<time> review-watch pass: ...`; what the watcher
/// printed about single changes during the pass comes before it. `cut` says
/// the tail starts in the middle of the log, so its first line is partial.
pub(crate) fn newest_pass(tail: &str, cut: bool) -> Option<Pass> {
    let mut newest = None;
    let mut block: Vec<&str> = Vec::new();
    for line in tail.lines().skip(usize::from(cut)) {
        let ended = line.split_once(' ').and_then(|(time, rest)| {
            rest.starts_with("review-watch pass:")
                .then(|| DateTime::parse_from_rfc3339(time).ok())
                .flatten()
        });
        match ended {
            Some(at) => {
                newest = Some(Pass {
                    at: at.with_timezone(&Utc),
                    lines: std::mem::take(&mut block)
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                });
            }
            None => block.push(line),
        }
    }
    newest
}

/// What the log's newest pass says about the change keyed `key` (`PR-N` or
/// `MR-N`), if anything.
pub(crate) fn from_pass(pass: &Pass, key: &str, now: DateTime<Utc>) -> Option<String> {
    let age = now.signed_duration_since(pass.at).num_minutes();
    if age > PASS_FRESH_MINS {
        return Some(format!(
            "the review watcher has logged no pass for {age} min"
        ));
    }
    let prefix = format!("{key}: ");
    let said = pass
        .lines
        .iter()
        .rev()
        .find_map(|line| line.strip_prefix(&prefix))?;
    let when = pass.at.format("%H:%M UTC");
    Some(if said.contains(BUSY) {
        format!("{BUSY} (review-watch log, pass of {when})")
    } else {
        format!(
            "the review watcher logged \"{}\" (pass of {when})",
            routed(said, 200)
        )
    })
}

/// The end of a regular file, at most `max` bytes, and whether the start was
/// cut. Never follows a symlink and never blocks on a FIFO or a device.
pub(crate) fn read_tail(path: &Path, max: u64) -> std::io::Result<(String, bool)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a regular file",
        ));
    }
    let start = meta.len().saturating_sub(max);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(max).read_to_end(&mut bytes)?;
    Ok((String::from_utf8_lossy(&bytes).into_owned(), start > 0))
}

/// Whether a lease's session runs, judged when the leases were listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Liveness {
    /// A session or its process was found, or the lease is too young to call
    /// abandoned.
    Held,
    /// No session and no process: the lease is left over.
    NotLive,
    /// The owner could not be established.
    Unknown,
}

/// One session lease of this clone, with the liveness judged on the listing
/// it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LeaseSeen {
    pub scope: String,
    pub id: String,
    pub branch: String,
    pub worktree: PathBuf,
    pub liveness: Liveness,
}

/// The evidence of one guard run, read once and asked per change.
#[derive(Debug, Clone, Default)]
pub(crate) struct Evidence {
    /// The watcher log's newest pass, or why there is none to go by.
    pub pass: Option<std::result::Result<Pass, String>>,
    /// The forge keys its changes `MR-N` (GitLab), not `PR-N`.
    pub gitlab: bool,
    /// Where a reviewer worktree for a change would be.
    pub tree_roots: Vec<PathBuf>,
    /// One listing of this clone's session leases with each lease's
    /// liveness, or why they could not be listed.
    pub leases: Option<std::result::Result<Vec<LeaseSeen>, String>>,
    /// `(path, branch)` of every worktree git lists for this project, or why
    /// they could not be listed.
    pub worktrees: Option<std::result::Result<Vec<(PathBuf, String)>, String>>,
    /// Where each live agent process on this host runs.
    pub agent_cwds: Vec<PathBuf>,
}

/// `(path, branch)` of each worktree in `git worktree list --porcelain`. A
/// worktree with a detached head has an empty branch.
pub(crate) fn listed_worktrees(porcelain: &str) -> Vec<(PathBuf, String)> {
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    for line in porcelain.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            out.push((PathBuf::from(path), String::new()));
        } else if let (Some(branch), Some(last)) =
            (line.strip_prefix("branch refs/heads/"), out.last_mut())
        {
            last.1 = branch.to_string();
        }
    }
    out
}

fn resolved(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

impl Evidence {
    /// Reads the evidence for the project at `root`.
    pub(crate) fn read(root: &Path, gitlab: bool) -> Self {
        let pass = match read_tail(&root.join(WATCH_LOG), LOG_TAIL_BYTES) {
            Ok((tail, cut)) => newest_pass(&tail, cut)
                .ok_or_else(|| format!("{WATCH_LOG} records no finished pass")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(format!("no review-watch log at {WATCH_LOG}"))
            }
            Err(error) => Err(format!(
                "{WATCH_LOG} could not be read: {}",
                routed(&error.to_string(), 120)
            )),
        };
        // One listing; liveness is judged on exactly these leases.
        let leases = crate::leases::list_leases_strict_all(root)
            .map(|leases| {
                crate::leases::classify_session_leases(root, &leases)
                    .into_iter()
                    .map(|(lease, state, _)| LeaseSeen {
                        liveness: match state {
                            aida_core::liveness::LeaseState::Stale => Liveness::NotLive,
                            aida_core::liveness::LeaseState::Unknown => Liveness::Unknown,
                            _ => Liveness::Held,
                        },
                        scope: lease.scope,
                        id: lease.id,
                        branch: lease.branch,
                        worktree: lease.worktree_path,
                    })
                    .collect()
            })
            .map_err(|why| routed(&why, 160));
        let worktrees = match crate::land::git(root, &["worktree", "list", "--porcelain"], 10) {
            Ok(out) if out.status.success() => {
                Ok(listed_worktrees(&String::from_utf8_lossy(&out.stdout)))
            }
            Ok(out) => Err(routed(&String::from_utf8_lossy(&out.stderr), 160)),
            Err(error) => Err(routed(&format!("{error:#}"), 160)),
        };
        Self {
            pass: Some(pass),
            gitlab,
            tree_roots: vec![
                crate::dispatch::worktrees_root(root),
                root.parent().unwrap_or(root).to_path_buf(),
            ],
            leases: Some(leases),
            worktrees: Some(worktrees),
            agent_cwds: aida_core::liveness::probe_live_agent_processes()
                .into_iter()
                .filter(|process| !process.stale_cwd)
                .map(|process| process.cwd)
                .collect(),
        }
    }

    /// The key the forge, the watcher's log and a reviewer's lease give
    /// change `pr`.
    pub(crate) fn key(&self, pr: u64) -> String {
        crate::land::reviewer_key(self.gitlab, pr)
    }

    /// What the path `tree` (named `aida-pr-N`) is: `Ok(Some(cause))` for a
    /// reviewer worktree nobody is in, `Ok(None)` when nothing is there or a
    /// session or process is in it, `Err(observation)` when it cannot be
    /// called a leftover reviewer worktree.
    fn tree(&self, tree: &Path, pr: u64) -> std::result::Result<Option<String>, String> {
        let shown = routed(&tree.display().to_string(), 200);
        let meta = match std::fs::symlink_metadata(tree) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "{shown} could not be inspected ({})",
                    routed(&error.to_string(), 120)
                ))
            }
        };
        let branch = format!("pr-{pr}");
        let not_ours =
            || format!("{shown} exists but is not a worktree of this project on branch {branch}");
        if !meta.is_dir() {
            return Err(not_ours());
        }
        let worktrees = match &self.worktrees {
            Some(Ok(worktrees)) => worktrees,
            Some(Err(why)) => {
                return Err(format!(
                    "{shown} exists, but the project's worktrees could not be listed ({why})"
                ))
            }
            None => {
                return Err(format!(
                    "{shown} exists, but the project's worktrees were not listed"
                ))
            }
        };
        let real = resolved(tree);
        if !worktrees
            .iter()
            .any(|(path, on)| *on == branch && resolved(path) == real)
        {
            return Err(not_ours());
        }
        let leases = match &self.leases {
            Some(Ok(leases)) => leases,
            _ => {
                return Err(format!(
                    "a reviewer worktree is at {shown}, but whether a session owns it is unknown (the leases were not listed)"
                ))
            }
        };
        let key = self.key(pr);
        let mut owners = leases.iter().filter(|lease| {
            lease.scope.eq_ignore_ascii_case(&key)
                || lease.branch == branch
                || (!lease.worktree.as_os_str().is_empty() && resolved(&lease.worktree) == real)
        });
        if owners.clone().any(|lease| lease.liveness == Liveness::Held) {
            return Ok(None);
        }
        if let Some(lease) = owners.find(|lease| lease.liveness == Liveness::Unknown) {
            return Err(format!(
                "a reviewer worktree is at {shown}, and whether the session of lease {} still owns it is unknown",
                routed(&lease.id, 80)
            ));
        }
        if self
            .agent_cwds
            .iter()
            .any(|cwd| resolved(cwd).starts_with(&real))
        {
            return Ok(None);
        }
        Ok(Some(format!(
            "a reviewer worktree from an earlier review is still at {shown} (branch {branch}, with no live session or agent process in it)"
        )))
    }

    /// The observed cause for change `pr` having no review, or `unknown`
    /// with what was looked at.
    pub(crate) fn cause(&self, pr: u64, now: DateTime<Utc>) -> String {
        let mut causes = Vec::new();
        let mut unread = Vec::new();
        let key = self.key(pr);
        match &self.pass {
            Some(Ok(pass)) => causes.extend(from_pass(pass, &key, now)),
            Some(Err(why)) => unread.push(why.clone()),
            None => unread.push("the review-watch log was not read".to_string()),
        }
        let name = format!("aida-pr-{pr}");
        let mut trees: Vec<PathBuf> = Vec::new();
        for root in &self.tree_roots {
            let tree = root.join(&name);
            if !trees.contains(&tree) {
                trees.push(tree);
            }
        }
        for tree in trees {
            match self.tree(&tree, pr) {
                Ok(cause) => causes.extend(cause),
                Err(seen) => unread.push(seen),
            }
        }
        match &self.leases {
            Some(Ok(leases)) => {
                for lease in leases
                    .iter()
                    .filter(|lease| lease.scope.eq_ignore_ascii_case(&key))
                {
                    let id = routed(&lease.id, 80);
                    match lease.liveness {
                        Liveness::Held => {}
                        Liveness::NotLive => causes.push(format!(
                            "lease {id} on {key} is still recorded and its session is not live"
                        )),
                        Liveness::Unknown => unread.push(format!(
                            "lease {id} on {key} is recorded, and whether its session is live is unknown"
                        )),
                    }
                }
            }
            Some(Err(why)) => unread.push(format!("the leases could not be listed: {why}")),
            None => unread.push("the leases were not read".to_string()),
        }
        if !causes.is_empty() {
            return causes.join("; ");
        }
        if unread.is_empty() {
            "unknown (the review-watch log names no cause for it, and no reviewer worktree or lease is left from an earlier review)".to_string()
        } else {
            format!("unknown ({})", unread.join("; "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn log(now: DateTime<Utc>) -> String {
        let stamp = |mins: i64| {
            (now - Duration::minutes(mins)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        };
        format!(
            "ial line cut by the tail\n\
             PR-7: watcher admission is busy\n\
             {} review-watch pass: 3 open change(s), 0 dispatched, 0 not admitted, 1 busy, 0 refused\n\
             PR-8: refused: the\thead moved\n\
             PR-9: watcher admission is busy\n\
             PR-90: something about another change\n\
             {} review-watch pass: 3 open change(s), 0 dispatched, 0 not admitted, 1 busy, 1 refused\n\
             PR-7: printed by a pass that has not finished\n",
            stamp(3),
            stamp(2)
        )
    }

    #[test]
    fn task_3827_the_newest_finished_pass_names_a_busy_watcher_or_what_it_logged() {
        let now = Utc::now();
        let pass = newest_pass(&log(now), true).unwrap();
        assert_eq!(pass.lines.len(), 3, "{:?}", pass.lines);
        let busy = from_pass(&pass, "PR-9", now).unwrap();
        assert!(
            busy.starts_with("watcher admission is busy (review-watch log, pass of "),
            "{busy}"
        );
        // Another line of the same pass, flattened to one line.
        let refused = from_pass(&pass, "PR-8", now).unwrap();
        assert!(
            refused.starts_with("the review watcher logged \"refused: the head moved\""),
            "{refused}"
        );
        // #7 was busy one pass earlier, not in the newest one; #9's line is
        // not #90's and the reverse.
        assert_eq!(from_pass(&pass, "PR-7", now), None);
        assert!(from_pass(&pass, "PR-90", now)
            .unwrap()
            .contains("another change"));
        assert_eq!(from_pass(&pass, "PR-1", now), None);
    }

    #[test]
    fn task_3827_a_watcher_that_stopped_logging_passes_is_the_cause() {
        let now = Utc::now();
        let pass = newest_pass(&log(now - Duration::minutes(40)), false).unwrap();
        assert_eq!(
            from_pass(&pass, "PR-9", now).as_deref(),
            Some("the review watcher has logged no pass for 42 min")
        );
        assert_eq!(
            newest_pass("PR-1: watcher admission is busy\n", false),
            None
        );
        assert_eq!(newest_pass("", true), None);
        // A first line cut by the tail is not taken for a pass line.
        let cut = format!(
            "{} review-watch pass: half\n",
            now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        );
        assert_eq!(newest_pass(&cut, true), None);
        assert!(newest_pass(&cut, false).is_some());
    }

    fn lease(scope: &str, id: &str, liveness: Liveness) -> LeaseSeen {
        LeaseSeen {
            scope: scope.into(),
            id: id.into(),
            branch: String::new(),
            worktree: PathBuf::new(),
            liveness,
        }
    }

    #[test]
    fn task_3827_a_leftover_lease_is_named_and_nothing_found_is_unknown() {
        let now = Utc::now();
        let pass = newest_pass(&log(now), true).unwrap();
        let evidence = Evidence {
            pass: Some(Ok(pass)),
            leases: Some(Ok(vec![
                lease("pr-13", "lease-a\nforged line", Liveness::NotLive),
                lease("PR-130", "lease-b", Liveness::NotLive),
                lease("TASK-13", "lease-c", Liveness::NotLive),
                lease("MR-14", "lease-mr", Liveness::NotLive),
                lease("PR-15", "lease-live", Liveness::Held),
                lease("PR-16", "lease-new", Liveness::Unknown),
            ])),
            worktrees: Some(Ok(Vec::new())),
            ..Evidence::default()
        };
        assert_eq!(
            evidence.cause(13, now),
            "lease lease-a forged line on PR-13 is still recorded and its session is not live"
        );
        // Both the log and a leftover lease: each is named.
        let mut both = evidence.clone();
        both.leases = Some(Ok(vec![lease("PR-9", "lease-d", Liveness::NotLive)]));
        let cause = both.cause(9, now);
        assert!(cause.starts_with("watcher admission is busy"), "{cause}");
        assert!(cause.contains("; lease lease-d on PR-9"), "{cause}");
        let nothing = "unknown (the review-watch log names no cause";
        assert!(evidence.cause(1, now).starts_with(nothing));
        // On GitHub a lease on MR-14 is another forge's key, and a lease
        // whose session is held is no cause.
        assert!(evidence.cause(14, now).starts_with(nothing));
        assert!(evidence.cause(15, now).starts_with(nothing));
        // A lease whose liveness could not be established (a reviewer that
        // is just starting) is never called a dead session.
        assert_eq!(
            evidence.cause(16, now),
            "unknown (lease lease-new on PR-16 is recorded, and whether its session is live is unknown)"
        );

        // What could not be looked at is said, never passed off as "nothing".
        let blind = Evidence {
            pass: Some(Err(format!("no review-watch log at {WATCH_LOG}"))),
            leases: Some(Err("lease table unreadable".into())),
            ..Evidence::default()
        };
        assert_eq!(
            blind.cause(1, now),
            "unknown (no review-watch log at .aida/review-watch/service.log; the leases could not be listed: lease table unreadable)"
        );
        assert!(Evidence::default()
            .cause(1, now)
            .starts_with("unknown (the review-watch log was not read"));
    }

    // GitLab keys a change MR-N in the watcher's log and in a reviewer's
    // lease; the reviewer worktree keeps its name.
    #[test]
    fn task_3827_on_gitlab_the_log_line_and_the_stale_lease_are_matched_by_mr_key() {
        let now = Utc::now();
        let stamp = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let tail = format!(
            "MR-21: watcher admission is busy\nPR-22: watcher admission is busy\n{stamp} review-watch pass: 2 open change(s)\n"
        );
        let evidence = Evidence {
            pass: Some(Ok(newest_pass(&tail, false).unwrap())),
            gitlab: true,
            leases: Some(Ok(vec![
                lease("MR-14", "lease-mr", Liveness::NotLive),
                lease("mr-15", "lease-lower", Liveness::NotLive),
                lease("PR-16", "lease-pr", Liveness::NotLive),
                lease("MR-140", "lease-other", Liveness::NotLive),
            ])),
            worktrees: Some(Ok(Vec::new())),
            ..Evidence::default()
        };
        assert_eq!(evidence.key(14), "MR-14");
        assert_eq!(
            evidence.cause(14, now),
            "lease lease-mr on MR-14 is still recorded and its session is not live"
        );
        assert!(evidence
            .cause(15, now)
            .starts_with("lease lease-lower on MR-15"));
        let nothing = "unknown (the review-watch log names no cause";
        assert!(evidence.cause(16, now).starts_with(nothing));
        assert!(evidence
            .cause(21, now)
            .starts_with("watcher admission is busy"));
        assert!(evidence.cause(22, now).starts_with(nothing));
        // The same leases read on GitHub: the MR lease is not this change's.
        let github = Evidence {
            gitlab: false,
            ..evidence
        };
        assert!(github.cause(14, now).starts_with(nothing));
        assert!(github.cause(16, now).starts_with("lease lease-pr on PR-16"));
    }

    // A path named aida-pr-N is a leftover reviewer worktree only when git
    // lists it for this project on branch pr-N and nobody is in it.
    #[cfg(unix)]
    #[test]
    fn task_3827_only_a_listed_unoccupied_worktree_on_the_review_branch_is_a_leftover_tree() {
        let now = Utc::now();
        let temp = tempfile::tempdir().unwrap();
        let trees = temp.path().join("trees");
        for n in [12, 13, 14, 15, 16, 17, 18] {
            std::fs::create_dir_all(trees.join(format!("aida-pr-{n}"))).unwrap();
        }
        std::fs::create_dir_all(trees.join("aida-pr-18/src")).unwrap();
        std::fs::write(trees.join("aida-pr-19"), "a file").unwrap();
        std::os::unix::fs::symlink(trees.join("aida-pr-12"), trees.join("aida-pr-20")).unwrap();
        let listed =
            |n: u64, branch: &str| (trees.join(format!("aida-pr-{n}")), branch.to_string());
        let in_tree = |n: u64, liveness| LeaseSeen {
            worktree: trees.join(format!("aida-pr-{n}")),
            ..lease("REVIEW", &format!("lease-{n}"), liveness)
        };
        let evidence = Evidence {
            pass: Some(Ok(Pass {
                at: now,
                lines: Vec::new(),
            })),
            tree_roots: vec![trees.clone(), trees.clone(), temp.path().join("absent")],
            worktrees: Some(Ok(vec![
                listed(12, "pr-12"),
                // Listed, but on another branch: someone's own worktree.
                listed(14, "task-14-work"),
                listed(15, "pr-15"),
                listed(16, "pr-16"),
                listed(17, "pr-17"),
                listed(18, "pr-18"),
                // The link's target is listed under another change's branch.
                listed(20, "pr-20"),
            ])),
            leases: Some(Ok(vec![
                in_tree(15, Liveness::Held),
                in_tree(16, Liveness::Unknown),
                LeaseSeen {
                    branch: "pr-17".into(),
                    ..lease("X", "lease-17", Liveness::Held)
                },
                in_tree(12, Liveness::NotLive),
            ])),
            agent_cwds: vec![trees.join("aida-pr-18").join("src")],
            ..Evidence::default()
        };
        let cause = evidence.cause(12, now);
        assert!(
            cause.starts_with("a reviewer worktree from an earlier review is still at ")
                && cause.ends_with(
                    "aida-pr-12 (branch pr-12, with no live session or agent process in it)"
                ),
            "{cause}"
        );
        // Named once though two roots hold the same path.
        assert_eq!(cause.matches("aida-pr-12").count(), 1);
        // An unrelated directory, a worktree on another branch, a regular
        // file and a symlink: described, and the cause stays unknown.
        for n in [13, 14, 19, 20] {
            let cause = evidence.cause(n, now);
            assert!(
                cause.starts_with("unknown (")
                    && cause.contains(&format!(
                        "aida-pr-{n} exists but is not a worktree of this project on branch pr-{n}"
                    )),
                "{cause}"
            );
        }
        // A live session's worktree, a live lease on the branch and a live
        // agent process inside are occupants: nothing is left over.
        let nothing = "unknown (the review-watch log names no cause";
        for n in [15, 17, 18] {
            assert!(evidence.cause(n, now).starts_with(nothing), "{n}");
        }
        // An owner whose liveness is unknown is said, not guessed.
        let cause = evidence.cause(16, now);
        assert!(
            cause.starts_with("unknown (a reviewer worktree is at ")
                && cause
                    .ends_with("whether the session of lease lease-16 still owns it is unknown)"),
            "{cause}"
        );
        // Worktrees or leases that could not be listed leave it unknown.
        let unlisted = Evidence {
            worktrees: Some(Err("git failed".into())),
            ..evidence.clone()
        };
        assert!(unlisted
            .cause(12, now)
            .contains("the project's worktrees could not be listed (git failed)"));
        let no_leases = Evidence {
            leases: Some(Err("unreadable".into())),
            ..evidence.clone()
        };
        let cause = no_leases.cause(12, now);
        assert!(
            cause.starts_with("unknown (")
                && cause.contains("whether a session owns it is unknown"),
            "{cause}"
        );
        // A path that cannot be inspected is an observation, not absence.
        let locked = temp.path().join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        let blocked = Evidence {
            tree_roots: vec![locked.join("aida-pr-1").join("deeper")],
            ..evidence.clone()
        };
        std::fs::write(locked.join("aida-pr-1"), "file in the way").unwrap();
        let cause = blocked.cause(1, now);
        assert!(
            cause.starts_with("unknown (") && cause.contains("could not be inspected"),
            "{cause}"
        );
    }

    #[test]
    fn task_3827_worktrees_are_parsed_from_gits_porcelain_listing() {
        let text = "worktree /p/aida\nHEAD abc\nbranch refs/heads/main\n\nworktree /p/aida-worktrees/aida-pr-7\nHEAD def\nbranch refs/heads/pr-7\n\nworktree /p/detached\nHEAD 123\ndetached\n";
        assert_eq!(
            listed_worktrees(text),
            vec![
                (PathBuf::from("/p/aida"), "main".to_string()),
                (
                    PathBuf::from("/p/aida-worktrees/aida-pr-7"),
                    "pr-7".to_string()
                ),
                (PathBuf::from("/p/detached"), String::new()),
            ]
        );
        assert!(listed_worktrees("").is_empty());
        assert!(listed_worktrees("branch refs/heads/orphan\n").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn task_3827_the_log_is_read_from_its_end_and_only_as_a_regular_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("service.log");
        std::fs::write(&path, format!("{}\nlast line\n", "x".repeat(500))).unwrap();
        let (tail, cut) = read_tail(&path, 64).unwrap();
        assert!(cut && tail.len() == 64 && tail.ends_with("last line\n"));
        let (whole, cut) = read_tail(&path, 4096).unwrap();
        assert!(!cut && whole.len() == 511);
        let link = temp.path().join("link.log");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(read_tail(&link, 64).is_err());
        assert!(read_tail(temp.path(), 64).is_err());
        assert_eq!(
            read_tail(&temp.path().join("absent"), 64)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound
        );

        // Through `Evidence::read`: the log of a real project directory.
        let now = Utc::now();
        let root = temp.path().join("project");
        std::fs::create_dir_all(root.join(".aida/review-watch")).unwrap();
        let evidence = Evidence::read(&root, false);
        assert!(matches!(&evidence.pass, Some(Err(why)) if why.starts_with("no review-watch log")));
        std::fs::write(root.join(WATCH_LOG), log(now)).unwrap();
        let evidence = Evidence::read(&root, false);
        assert!(evidence
            .cause(9, now)
            .starts_with("watcher admission is busy"));
    }
}
