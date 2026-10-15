//! Primary-checkout drift detection.
//!
//! The primary checkout (the main worktree every linked worktree hangs off)
//! is where the live dev binary is built from and where agents are not
//! supposed to leave edits. No git hook sees uncommitted edits or untracked
//! scratch files there, so this module makes them visible on three read-only
//! surfaces: the session-start hook (a shell mirror of [`summary_line`]),
//! the status line ([`STATUSLINE_MARKER`]), and `aida doctor`
//! ([`doctor_findings`]).
//!
//! Everything here only READS git state and file metadata. Nothing is ever
//! deleted, moved, or rewritten.
// trace:STORY-1648 | ai:claude

use std::path::Path;

/// Canonical `aida doctor` category name.
pub(crate) const CATEGORY: &str = "main-checkout-drift";

/// Compact status-line marker shown while the primary checkout has drifted.
pub(crate) const STATUSLINE_MARKER: &str = "⌂!";

/// Cap on paths quoted inline in one doctor finding summary.
const MAX_LISTED_PATHS: usize = 20;

/// Parsed state of one checkout, from `git status --porcelain=v2 --branch`.
// trace:STORY-1648 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CheckoutDrift {
    /// Checked-out branch; `None` when HEAD is detached.
    pub(crate) branch: Option<String>,
    /// Upstream tracking ref (e.g. `origin/main`), when one is configured.
    pub(crate) upstream: Option<String>,
    pub(crate) ahead: u32,
    pub(crate) behind: u32,
    /// Paths with staged, unstaged, renamed, or conflicted changes.
    pub(crate) tracked: Vec<String>,
    /// Untracked paths (directories end with `/`).
    pub(crate) untracked: Vec<String>,
}

impl CheckoutDrift {
    /// True when the checkout sits on its default branch (`main`/`master`).
    pub(crate) fn on_default_branch(&self) -> bool {
        matches!(self.branch.as_deref(), Some("main") | Some("master"))
    }

    /// True when anything about the checkout deserves a flag: tracked
    /// changes, untracked files, an off-default or detached HEAD, or
    /// divergence from the upstream. A missing upstream alone is not drift
    /// (a local-only project has none).
    pub(crate) fn is_drifted(&self) -> bool {
        !self.tracked.is_empty()
            || !self.untracked.is_empty()
            || !self.on_default_branch()
            || self.ahead > 0
            || self.behind > 0
    }

    /// `on <branch>` label; a detached HEAD reads as `detached HEAD`.
    fn branch_label(&self) -> &str {
        self.branch.as_deref().unwrap_or("detached HEAD")
    }

    /// Ahead/behind text relative to the upstream's remote.
    fn sync_label(&self) -> String {
        let Some(upstream) = self.upstream.as_deref() else {
            return "no upstream".to_string();
        };
        let remote = upstream.split('/').next().unwrap_or(upstream);
        let mut parts: Vec<String> = Vec::new();
        if self.ahead > 0 {
            parts.push(format!("{} ahead", self.ahead));
        }
        if self.behind > 0 {
            parts.push(format!("{} behind", self.behind));
        }
        if parts.is_empty() {
            format!("up to date with {remote}")
        } else {
            format!("{} {remote}", parts.join(", "))
        }
    }
}

/// Strip git's C-style quoting from a porcelain path (best-effort: the
/// surrounding quotes only; escapes inside are left as git printed them).
fn unquote(path: &str) -> String {
    let p = path.trim_end_matches(['\r', '\n']);
    match p.strip_prefix('"').and_then(|q| q.strip_suffix('"')) {
        Some(inner) => inner.to_string(),
        None => p.to_string(),
    }
}

/// Parse `git status --porcelain=v2 --branch` (newline-separated, not `-z`).
/// Unknown lines are ignored so a newer git cannot break the parse.
// trace:STORY-1648 | ai:claude
pub(crate) fn parse_porcelain_v2(output: &str) -> CheckoutDrift {
    let mut drift = CheckoutDrift::default();
    for line in output.lines() {
        if let Some(head) = line.strip_prefix("# branch.head ") {
            drift.branch = (head != "(detached)").then(|| head.to_string());
        } else if let Some(up) = line.strip_prefix("# branch.upstream ") {
            drift.upstream = Some(up.to_string());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            for tok in ab.split_whitespace() {
                if let Some(n) = tok.strip_prefix('+') {
                    drift.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = tok.strip_prefix('-') {
                    drift.behind = n.parse().unwrap_or(0);
                }
            }
        } else if let Some(rest) = line.strip_prefix("1 ") {
            // XY sub mH mI mW hH hI path
            if let Some(path) = rest.splitn(8, ' ').nth(7) {
                drift.tracked.push(unquote(path));
            }
        } else if let Some(rest) = line.strip_prefix("2 ") {
            // XY sub mH mI mW hH hI Xscore path<TAB>origPath
            if let Some(paths) = rest.splitn(9, ' ').nth(8) {
                let path = paths.split('\t').next().unwrap_or(paths);
                drift.tracked.push(unquote(path));
            }
        } else if let Some(rest) = line.strip_prefix("u ") {
            // XY sub m1 m2 m3 mW h1 h2 h3 path
            if let Some(path) = rest.splitn(10, ' ').nth(9) {
                drift.tracked.push(unquote(path));
            }
        } else if let Some(path) = line.strip_prefix("? ") {
            drift.untracked.push(unquote(path));
        }
    }
    drift
}

/// The one-line session-start summary, or `None` when the checkout is clean.
/// The session-context hook template renders the identical string in shell.
// Only the tests call this in Rust; it is the reference rendering the shell
// hook must match, so it is kept here rather than duplicated in a test.
// trace:STORY-1648 | ai:claude
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn summary_line(drift: &CheckoutDrift) -> Option<String> {
    if !drift.is_drifted() {
        return None;
    }
    let tracked = drift.tracked.len();
    Some(format!(
        "main checkout: {} tracked change{}, {} untracked · on {} · {}",
        tracked,
        if tracked == 1 { "" } else { "s" },
        drift.untracked.len(),
        drift.branch_label(),
        drift.sync_label()
    ))
}

/// Status-line marker for the drift, or `None` when clean.
// trace:STORY-1648 | ai:claude
pub(crate) fn statusline_marker(drift: Option<&CheckoutDrift>) -> Option<&'static str> {
    drift.filter(|d| d.is_drifted()).map(|_| STATUSLINE_MARKER)
}

/// Read the drift state of the checkout at `root` with one
/// `git status`. `--no-optional-locks` keeps the probe from taking the index
/// lock, so it never contends with an agent working in that checkout.
/// Returns `None` when git fails (not a repository, git missing).
// trace:STORY-1648 | ai:claude
pub(crate) fn probe(root: &Path) -> Option<CheckoutDrift> {
    let out = std::process::Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain=v2", "--branch"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_porcelain_v2(&String::from_utf8_lossy(&out.stdout)))
}

/// A session window used for best-effort attribution: a label naming the
/// session and the moment it started.
pub(crate) type SessionWindow = (String, chrono::DateTime<chrono::Utc>);

/// Pick the session most likely to have created a file modified at `mtime`:
/// the latest-starting session that had already started by then. `None`
/// when no session window covers the time.
// trace:STORY-1648 | ai:claude
pub(crate) fn attribute_by_mtime(
    mtime: chrono::DateTime<chrono::Utc>,
    windows: &[SessionWindow],
) -> Option<&str> {
    windows
        .iter()
        .filter(|(_, started)| *started <= mtime)
        .max_by_key(|(_, started)| *started)
        .map(|(label, _)| label.as_str())
}

fn list_paths(paths: &[String]) -> String {
    let mut shown: Vec<String> = paths.iter().take(MAX_LISTED_PATHS).cloned().collect();
    let more = paths.len().saturating_sub(MAX_LISTED_PATHS);
    if more > 0 {
        shown.push(format!("+{more} more"));
    }
    shown.join(", ")
}

/// Doctor findings for a drifted primary checkout. Pure over its inputs:
/// `mtime_of` resolves a checkout-relative path to its modification time and
/// `windows` are the known session windows. Report-only (`safe_heal: false`).
// trace:STORY-1648 | ai:claude
pub(crate) fn doctor_findings(
    drift: &CheckoutDrift,
    windows: &[SessionWindow],
    mtime_of: impl Fn(&str) -> Option<chrono::DateTime<chrono::Utc>>,
) -> Vec<crate::DoctorFinding> {
    let mut out = Vec::new();
    if !drift.is_drifted() {
        return out;
    }
    let finding = |id: &str, summary: String, action: &str| crate::DoctorFinding {
        category: CATEGORY.to_string(),
        id: id.to_string(),
        summary,
        action: action.to_string(),
        safe_heal: false,
    };
    if !drift.on_default_branch() {
        out.push(finding(
            "branch",
            format!("main checkout is on {}, not main", drift.branch_label()),
            "ask whoever switched it to return the main checkout to main; \
             do work in a linked worktree instead",
        ));
    }
    if drift.ahead > 0 || drift.behind > 0 {
        out.push(finding(
            "sync",
            format!("main checkout is {}", drift.sync_label()),
            "fast-forward (behind) or push/move the local commits (ahead) \
             once nobody is building from the main checkout",
        ));
    }
    if !drift.tracked.is_empty() {
        out.push(finding(
            "tracked",
            format!(
                "main checkout has {} uncommitted tracked change(s): {}",
                drift.tracked.len(),
                list_paths(&drift.tracked)
            ),
            "ask the session that made these edits (see `aida session leases`) \
             to move them to its worktree; doctor never discards them",
        ));
    }
    if !drift.untracked.is_empty() {
        let labelled: Vec<String> = drift
            .untracked
            .iter()
            .map(
                |p| match mtime_of(p).and_then(|m| attribute_by_mtime(m, windows)) {
                    Some(owner) => format!("{p} (likely {owner})"),
                    None => p.clone(),
                },
            )
            .collect();
        out.push(finding(
            "untracked",
            format!(
                "main checkout has {} untracked path(s): {}",
                drift.untracked.len(),
                list_paths(&labelled)
            ),
            "ask the named session (or `aida session leases`) whether these \
             are still needed; doctor never deletes them",
        ));
    }
    out
}

/// Probe the primary checkout at `primary_root` and produce its doctor
/// findings, attributing untracked files to live session leases by mtime.
// trace:STORY-1648 | ai:claude
pub(crate) fn scan(primary_root: &Path) -> Vec<crate::DoctorFinding> {
    let Some(drift) = probe(primary_root) else {
        return Vec::new();
    };
    let windows: Vec<SessionWindow> = crate::list_leases(primary_root)
        .into_iter()
        .map(|l| (format!("session {} ({})", l.id, l.scope), l.started_at))
        .collect();
    doctor_findings(&drift, &windows, |rel| {
        std::fs::symlink_metadata(primary_root.join(rel.trim_end_matches('/')))
            .and_then(|m| m.modified())
            .ok()
            .map(chrono::DateTime::<chrono::Utc>::from)
    })
}

#[cfg(test)]
#[path = "tests/story_1648_main_checkout_drift_tests.rs"]
mod story_1648_main_checkout_drift_tests;
