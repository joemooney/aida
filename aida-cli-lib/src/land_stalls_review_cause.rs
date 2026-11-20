//! Why a head has no review: the cause the guard can observe on this host.
//!
//! Three things are looked at, all read-only: what the review watcher's
//! service log says about the change in its newest pass (`watcher admission
//! is busy`, a refusal, a deferral), a reviewer worktree `aida-pr-N` left
//! from an earlier review, and a session lease on `PR-N` that is no longer
//! live. When none of them says anything the cause is reported as unknown,
//! with what was looked at.
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

/// What the log's newest pass says about change `pr`, if anything.
pub(crate) fn from_pass(pass: &Pass, pr: u64, now: DateTime<Utc>) -> Option<String> {
    let age = now.signed_duration_since(pass.at).num_minutes();
    if age > PASS_FRESH_MINS {
        return Some(format!(
            "the review watcher has logged no pass for {age} min"
        ));
    }
    let said = pass.lines.iter().rev().find_map(|line| {
        ["PR", "MR"]
            .iter()
            .find_map(|kind| line.strip_prefix(&format!("{kind}-{pr}: ")))
    })?;
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

/// The evidence of one guard run, read once and asked per change.
#[derive(Debug, Clone, Default)]
pub(crate) struct Evidence {
    /// The watcher log's newest pass, or why there is none to go by.
    pub pass: Option<std::result::Result<Pass, String>>,
    /// Where a reviewer worktree for a change would be.
    pub tree_roots: Vec<PathBuf>,
    /// `(scope, lease id)` of each session lease that is no longer live, or
    /// why the leases could not be listed.
    pub stale_leases: Option<std::result::Result<Vec<(String, String)>, String>>,
}

impl Evidence {
    /// Reads the evidence for the project at `root`. `live` holds the ids of
    /// the session leases this run found live.
    pub(crate) fn read(root: &Path, live: &[String]) -> Self {
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
        let stale_leases = crate::leases::list_leases_strict_all(root)
            .map(|leases| {
                leases
                    .into_iter()
                    .filter(|lease| !live.contains(&lease.id))
                    .map(|lease| (lease.scope, lease.id))
                    .collect()
            })
            .map_err(|why| routed(&why, 160));
        Self {
            pass: Some(pass),
            tree_roots: vec![
                crate::dispatch::worktrees_root(root),
                root.parent().unwrap_or(root).to_path_buf(),
            ],
            stale_leases: Some(stale_leases),
        }
    }

    /// The observed cause for change `pr` having no review, or `unknown`
    /// with what was looked at.
    pub(crate) fn cause(&self, pr: u64, now: DateTime<Utc>) -> String {
        let mut causes = Vec::new();
        let mut unread = Vec::new();
        match &self.pass {
            Some(Ok(pass)) => causes.extend(from_pass(pass, pr, now)),
            Some(Err(why)) => unread.push(why.clone()),
            None => unread.push("the review-watch log was not read".to_string()),
        }
        let name = format!("aida-pr-{pr}");
        let mut trees: Vec<PathBuf> = self
            .tree_roots
            .iter()
            .map(|root| root.join(&name))
            .filter(|tree| std::fs::symlink_metadata(tree).is_ok())
            .collect();
        trees.dedup();
        for tree in trees {
            causes.push(format!(
                "a reviewer worktree from an earlier review is still at {}",
                routed(&tree.display().to_string(), 200)
            ));
        }
        let scope = format!("PR-{pr}");
        match &self.stale_leases {
            Some(Ok(leases)) => {
                for (_, id) in leases
                    .iter()
                    .filter(|(held, _)| held.eq_ignore_ascii_case(&scope))
                {
                    causes.push(format!(
                        "lease {} on {scope} is still recorded and its session is not live",
                        routed(id, 80)
                    ));
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
        let busy = from_pass(&pass, 9, now).unwrap();
        assert!(
            busy.starts_with("watcher admission is busy (review-watch log, pass of "),
            "{busy}"
        );
        // Another line of the same pass, flattened to one line.
        let refused = from_pass(&pass, 8, now).unwrap();
        assert!(
            refused.starts_with("the review watcher logged \"refused: the head moved\""),
            "{refused}"
        );
        // #7 was busy one pass earlier, not in the newest one; #9's line is
        // not #90's and the reverse.
        assert_eq!(from_pass(&pass, 7, now), None);
        assert!(from_pass(&pass, 90, now)
            .unwrap()
            .contains("another change"));
        assert_eq!(from_pass(&pass, 1, now), None);
    }

    #[test]
    fn task_3827_a_watcher_that_stopped_logging_passes_is_the_cause() {
        let now = Utc::now();
        let pass = newest_pass(&log(now - Duration::minutes(40)), false).unwrap();
        assert_eq!(
            from_pass(&pass, 9, now).as_deref(),
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

    #[test]
    fn task_3827_a_leftover_reviewer_tree_or_lease_is_named_and_nothing_found_is_unknown() {
        let now = Utc::now();
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("trees/aida-pr-12")).unwrap();
        let pass = newest_pass(&log(now), true).unwrap();
        let evidence = Evidence {
            pass: Some(Ok(pass)),
            tree_roots: vec![temp.path().join("trees"), temp.path().join("absent")],
            stale_leases: Some(Ok(vec![
                ("pr-13".into(), "lease-a\nforged line".into()),
                ("PR-130".into(), "lease-b".into()),
                ("TASK-13".into(), "lease-c".into()),
            ])),
        };
        let tree = evidence.cause(12, now);
        assert!(
            tree.starts_with("a reviewer worktree from an earlier review is still at ")
                && tree.ends_with("trees/aida-pr-12"),
            "{tree}"
        );
        assert_eq!(
            evidence.cause(13, now),
            "lease lease-a forged line on PR-13 is still recorded and its session is not live"
        );
        // Both the log and a leftover lease: each is named.
        let mut both = evidence.clone();
        both.stale_leases = Some(Ok(vec![("PR-9".into(), "lease-d".into())]));
        let cause = both.cause(9, now);
        assert!(cause.starts_with("watcher admission is busy"), "{cause}");
        assert!(cause.contains("; lease lease-d on PR-9"), "{cause}");
        assert!(evidence
            .cause(1, now)
            .starts_with("unknown (the review-watch log names no cause"));

        // What could not be looked at is said, never passed off as "nothing".
        let blind = Evidence {
            pass: Some(Err(format!("no review-watch log at {WATCH_LOG}"))),
            tree_roots: Vec::new(),
            stale_leases: Some(Err("lease table unreadable".into())),
        };
        assert_eq!(
            blind.cause(1, now),
            "unknown (no review-watch log at .aida/review-watch/service.log; the leases could not be listed: lease table unreadable)"
        );
        assert!(Evidence::default()
            .cause(1, now)
            .starts_with("unknown (the review-watch log was not read"));
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
        let evidence = Evidence::read(&root, &[]);
        assert!(matches!(&evidence.pass, Some(Err(why)) if why.starts_with("no review-watch log")));
        std::fs::write(root.join(WATCH_LOG), log(now)).unwrap();
        let evidence = Evidence::read(&root, &[]);
        assert!(evidence
            .cause(9, now)
            .starts_with("watcher admission is busy"));
    }
}
