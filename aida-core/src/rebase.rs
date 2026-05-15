//! Rebase detection + classification — the reusable substrate behind
//! `aida rebase` (TASK-103). The detection helpers are deliberately
//! stateless and side-effect-light (one optional `git fetch`) so the
//! lifecycle delegators (TASK-97 `aida pull --autorebase`, TASK-98
//! `/aida-commit` precheck) can call `detect` + `classify` instead of
//! re-deriving ahead/behind/overlap themselves.
//!
//! trace:TASK-103 | ai:claude

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

/// The five rebase classifications. `Clean` / `AheadOnly` need no
/// rebase; `BehindOnly` / `DivergedSafe` are safe to auto-execute;
/// `DivergedRisky` has file-path overlap between the two sides and
/// wants a human decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebaseClass {
    /// ahead == 0 && behind == 0 — fully in sync, nothing to do.
    Clean,
    /// ahead > 0, behind == 0 — local commits not yet pushed; no rebase.
    AheadOnly,
    /// ahead == 0, behind > 0 — a straight (fast-forwardable) rebase.
    BehindOnly,
    /// ahead > 0, behind > 0, no file overlap — rebase is safe to auto.
    DivergedSafe,
    /// ahead > 0, behind > 0, file overlap — surface it, ask a human.
    DivergedRisky,
}

impl RebaseClass {
    /// Does this classification have work a rebase would do?
    pub fn needs_rebase(self) -> bool {
        matches!(
            self,
            RebaseClass::BehindOnly | RebaseClass::DivergedSafe | RebaseClass::DivergedRisky
        )
    }

    /// Safe to execute without a human looking at the overlap first?
    pub fn is_safe(self) -> bool {
        matches!(self, RebaseClass::BehindOnly | RebaseClass::DivergedSafe)
    }

    /// Short stable label (used in `--json` output and tests).
    pub fn label(self) -> &'static str {
        match self {
            RebaseClass::Clean => "clean",
            RebaseClass::AheadOnly => "ahead-only",
            RebaseClass::BehindOnly => "behind-only",
            RebaseClass::DivergedSafe => "diverged-safe",
            RebaseClass::DivergedRisky => "diverged-risky",
        }
    }
}

/// Pure classifier — the heart of the command, deliberately decoupled
/// from git so it is trivially testable. `overlap_exists` is whether
/// the file-path sets touched by the two sides intersect.
pub fn classify(ahead: u32, behind: u32, overlap_exists: bool) -> RebaseClass {
    match (ahead, behind) {
        (0, 0) => RebaseClass::Clean,
        (_, 0) => RebaseClass::AheadOnly,
        (0, _) => RebaseClass::BehindOnly,
        (_, _) if overlap_exists => RebaseClass::DivergedRisky,
        (_, _) => RebaseClass::DivergedSafe,
    }
}

/// Full result of the detect phase.
#[derive(Debug, Clone)]
pub struct RebaseDetection {
    /// Current branch name (or "HEAD" when detached).
    pub branch: String,
    /// The ref we compare/rebase against (e.g. `origin/main`).
    pub upstream: String,
    /// Whether a `git fetch` was actually run during detection.
    pub fetched: bool,
    /// Commits HEAD has that `upstream` does not.
    pub ahead: u32,
    /// Commits `upstream` has that HEAD does not.
    pub behind: u32,
    /// Files touched by our (ahead) commits.
    pub our_files: Vec<String>,
    /// Files touched by their (behind) commits.
    pub their_files: Vec<String>,
    /// Files touched by BOTH sides — the overlap that makes a rebase
    /// risky.
    pub overlap: Vec<String>,
    /// True when `git status --porcelain` is empty.
    pub working_tree_clean: bool,
    /// Porcelain lines describing the dirty working tree (empty when
    /// clean).
    pub dirty_files: Vec<String>,
}

impl RebaseDetection {
    /// Classify this detection result.
    pub fn class(&self) -> RebaseClass {
        classify(self.ahead, self.behind, !self.overlap.is_empty())
    }
}

/// Error from the detect phase. Kept as a plain enum so callers can
/// distinguish "no upstream configured" (often benign) from a real
/// git failure.
#[derive(Debug)]
pub enum DetectError {
    /// The branch has no upstream and no `--branch` override was given.
    NoUpstream(String),
    /// A git invocation failed.
    Git(String),
}

impl std::fmt::Display for DetectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DetectError::NoUpstream(b) => write!(
                f,
                "branch '{}' has no upstream — pass --branch <ref> to pick a target",
                b
            ),
            DetectError::Git(msg) => write!(f, "git error: {}", msg),
        }
    }
}

impl std::error::Error for DetectError {}

fn git(repo: &Path, args: &[&str]) -> Result<String, DetectError> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| DetectError::Git(format!("spawn failed: {e}")))?;
    if !out.status.success() {
        return Err(DetectError::Git(format!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_ok(repo: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Files touched by commits in `range` (e.g. `origin/main..HEAD`).
fn files_in_range(repo: &Path, range: &str) -> Vec<String> {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["log", "--name-only", "--pretty=format:", range])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            let mut seen: Vec<String> = Vec::new();
            let mut set: HashSet<String> = HashSet::new();
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                let l = line.trim();
                if !l.is_empty() && set.insert(l.to_string()) {
                    seen.push(l.to_string());
                }
            }
            seen
        })
        .unwrap_or_default()
}

fn count_range(repo: &Path, range: &str) -> Result<u32, DetectError> {
    let s = git(repo, &["rev-list", "--count", range])?;
    s.parse()
        .map_err(|_| DetectError::Git(format!("could not parse commit count from '{s}'")))
}

/// Run the detect phase against `repo`.
///
/// * `branch_override` — when `Some`, used directly as the upstream ref
///   to compare against (a `--branch` argument); when `None` the
///   branch's tracked `@{u}` is used.
/// * `fetch` — when true, a `git fetch <remote> <branch>` is run first
///   so ahead/behind reflect the true remote state. Set false for
///   offline / `--no-fetch`.
pub fn detect(
    repo: &Path,
    branch_override: Option<&str>,
    fetch: bool,
) -> Result<RebaseDetection, DetectError> {
    let branch = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;

    // Resolve the upstream ref we measure against.
    let upstream = match branch_override {
        Some(b) => b.to_string(),
        None => git(
            repo,
            &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        )
        .map_err(|_| DetectError::NoUpstream(branch.clone()))?,
    };

    // Fetch so the upstream ref is current. Best-effort: a fetch
    // failure (offline, etc.) is downgraded to "didn't fetch" rather
    // than aborting detection, which still works against cached refs.
    let mut fetched = false;
    if fetch {
        if let Some((remote, remote_branch)) = upstream.split_once('/') {
            if git_ok(repo, &["fetch", remote, remote_branch]) {
                fetched = true;
            }
        }
    }

    if !git_ok(repo, &["rev-parse", "--verify", "--quiet", &upstream]) {
        return Err(DetectError::Git(format!(
            "upstream ref '{upstream}' does not exist locally"
        )));
    }

    let ahead = count_range(repo, &format!("{upstream}..HEAD"))?;
    let behind = count_range(repo, &format!("HEAD..{upstream}"))?;

    let our_files = files_in_range(repo, &format!("{upstream}..HEAD"));
    let their_files = files_in_range(repo, &format!("HEAD..{upstream}"));
    let their_set: HashSet<&String> = their_files.iter().collect();
    let overlap: Vec<String> = our_files
        .iter()
        .filter(|f| their_set.contains(f))
        .cloned()
        .collect();

    let porcelain = git(repo, &["status", "--porcelain"])?;
    let dirty_files: Vec<String> = porcelain
        .lines()
        .map(|l| l.to_string())
        .filter(|l| !l.is_empty())
        .collect();

    Ok(RebaseDetection {
        branch,
        upstream,
        fetched,
        ahead,
        behind,
        our_files,
        their_files,
        overlap,
        working_tree_clean: dirty_files.is_empty(),
        dirty_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_clean() {
        assert_eq!(classify(0, 0, false), RebaseClass::Clean);
        assert_eq!(classify(0, 0, true), RebaseClass::Clean);
    }

    #[test]
    fn classify_ahead_only() {
        assert_eq!(classify(3, 0, false), RebaseClass::AheadOnly);
        // overlap is irrelevant when behind == 0
        assert_eq!(classify(3, 0, true), RebaseClass::AheadOnly);
    }

    #[test]
    fn classify_behind_only() {
        assert_eq!(classify(0, 5, false), RebaseClass::BehindOnly);
        assert_eq!(classify(0, 5, true), RebaseClass::BehindOnly);
    }

    #[test]
    fn classify_diverged_safe() {
        assert_eq!(classify(2, 4, false), RebaseClass::DivergedSafe);
    }

    #[test]
    fn classify_diverged_risky() {
        assert_eq!(classify(2, 4, true), RebaseClass::DivergedRisky);
    }

    #[test]
    fn class_predicates() {
        assert!(!RebaseClass::Clean.needs_rebase());
        assert!(!RebaseClass::AheadOnly.needs_rebase());
        assert!(RebaseClass::BehindOnly.needs_rebase());
        assert!(RebaseClass::DivergedSafe.needs_rebase());
        assert!(RebaseClass::DivergedRisky.needs_rebase());

        assert!(RebaseClass::BehindOnly.is_safe());
        assert!(RebaseClass::DivergedSafe.is_safe());
        assert!(!RebaseClass::DivergedRisky.is_safe());
        assert!(!RebaseClass::Clean.is_safe());
    }

    #[test]
    fn labels_are_stable() {
        assert_eq!(RebaseClass::Clean.label(), "clean");
        assert_eq!(RebaseClass::AheadOnly.label(), "ahead-only");
        assert_eq!(RebaseClass::BehindOnly.label(), "behind-only");
        assert_eq!(RebaseClass::DivergedSafe.label(), "diverged-safe");
        assert_eq!(RebaseClass::DivergedRisky.label(), "diverged-risky");
    }
}
