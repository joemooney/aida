//! BUG-2554: the persisted, append-only index of store history that the
//! mandate exclusion gate reads instead of walking every commit on every ref
//! on every train gate.
//!
//! It records exactly what the two full walks found: every object version
//! (object path → the commits that added or changed it) and every
//! `spec_id` / `agreed_id` line a version added or removed (ID → the object
//! stems that carried it, or the refusal a non-canonical path earns). Each
//! read refreshes it by walking only the commits reachable from start points
//! it has not indexed yet. A missing, unreadable or version-skewed index, or
//! an anchor (a start point whose history it holds) that no longer exists,
//! rebuilds it with one full walk, so a doubt never reads as "no history".
//! The start points are exactly `git log --all`'s (every ref and the HEAD of
//! every worktree), and anchors are kept for every history the index still
//! holds, deleted refs included, so a later collection is always noticed.
//! The file lives only in the git common dir (never tracked, never pushed);
//! every git call, the anchor check included, keeps the store-history
//! deadline, and any git failure is an error (the gate refuses), never a
//! reuse of what the index held.
// trace:BUG-2554 | ai:claude

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

/// Bumped whenever what the index records changes; a skewed index rebuilds.
/// 2: `tips` became anchors for every retained history (a version-1 index
/// may have dropped the anchor of a deleted ref, so it rebuilds).
// trace:BUG-2554 | ai:claude
const INDEX_VERSION: u32 = 2;

/// The `-G` pattern every alias walk uses, whatever IDs are asked for later.
const ANY_ID_LINE: &str = "^(spec_id|agreed_id): *['\"]?([A-Za-z0-9_-]+)['\"]? *$";

/// One ID's alias evidence: the canonical stems whose versions carried it,
/// newest first, or the refusal a hit outside a canonical path earns.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct IndexedAlias {
    pub stems: Vec<String>,
    pub refusal: Option<String>,
}

/// The store-history index (see the module docs).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct HistoryIndex {
    version: u32,
    /// Anchors: commits whose whole history the index holds, sorted. Every
    /// start point of the last refresh, plus every earlier anchor no other
    /// anchor reaches (a deleted ref's history stays anchored, so its
    /// collection rebuilds the index instead of leaving stale versions).
    // trace:BUG-2554 | ai:claude
    tips: Vec<String>,
    /// Object path → commits that added or changed it (newest first).
    pub versions: BTreeMap<String, Vec<String>>,
    /// Upper-cased ID → its alias evidence.
    pub aliases: BTreeMap<String, IndexedAlias>,
}

/// How [`HistoryIndex::current`] brought the index up to date.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refresh {
    /// Every tip was already indexed: no history walk.
    Current,
    /// Walked only the commits behind new tips.
    Incremental,
    /// Walked the whole history (first build, or a doubtful index).
    Rebuild,
}

#[cfg(test)]
thread_local! {
    /// The refreshes this thread ran, for the walk-count regressions.
    pub(crate) static REFRESHES: std::cell::RefCell<Vec<Refresh>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn note(_refresh: &'static str) {
    #[cfg(test)]
    REFRESHES.with(|r| {
        r.borrow_mut().push(match _refresh {
            "current" => Refresh::Current,
            "incremental" => Refresh::Incremental,
            _ => Refresh::Rebuild,
        })
    });
}

fn git_text(store_root: &Path, args: &[&str]) -> Result<String, String> {
    super::store_git(store_root, args, None).map(|out| String::from_utf8_lossy(&out).into_owned())
}

/// Where the index lives: `<git-common-dir>/aida/mandate-history-index.json`.
fn index_path(store_root: &Path) -> Result<PathBuf, String> {
    let dir = git_text(store_root, &["rev-parse", "--git-common-dir"])?;
    let dir = PathBuf::from(dir.trim());
    let dir = if dir.is_absolute() {
        dir
    } else {
        store_root.join(dir)
    };
    Ok(dir.join("aida").join("mandate-history-index.json"))
}

/// The commits `git log --all` starts from (every ref, and the HEAD of
/// every worktree, detached ones included), sorted. Asked of git's own
/// revision machinery so the set cannot drift from the full walk's; an
/// unborn HEAD is simply absent, and any failure is an error, never "no tip".
// trace:BUG-2554 | ai:claude
fn current_tips(store_root: &Path) -> Result<Vec<String>, String> {
    let out = git_text(store_root, &["rev-list", "--all", "--no-walk"])?;
    Ok(shas(&out).into_iter().collect())
}

/// A full SHA-1 or SHA-256 object name.
fn is_object_name(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The distinct object names in `out`, one per line.
fn shas(out: &str) -> BTreeSet<String> {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether every one of `anchors` still exists in the object database.
/// Arguments, not stdin, so the call runs under the store-history deadline
/// (a stalled check is a timeout error, never an unbounded wait).
// trace:BUG-2554 | ai:claude
fn all_exist(store_root: &Path, anchors: &[String]) -> Result<bool, String> {
    if anchors.is_empty() {
        return Ok(true);
    }
    let mut args = vec!["rev-list", "--no-walk", "--ignore-missing"];
    args.extend(anchors.iter().map(String::as_str));
    let found = shas(&git_text(store_root, &args)?);
    Ok(anchors.iter().all(|a| found.contains(a)))
}

/// The smallest set of `commits` that reaches all of them, so the anchors
/// of deleted refs do not pile up once a live ref has moved past them.
// trace:BUG-2554 | ai:claude
fn independent(store_root: &Path, commits: &BTreeSet<String>) -> Result<BTreeSet<String>, String> {
    if commits.len() < 2 {
        return Ok(commits.clone());
    }
    let mut args = vec!["merge-base", "--independent"];
    args.extend(commits.iter().map(String::as_str));
    Ok(shas(&git_text(store_root, &args)?))
}

impl HistoryIndex {
    /// The index, brought up to date with every ref of the store repository.
    /// `Err` when any git call fails or outlives its deadline (the gate then
    /// refuses); never an answer from a stale or partial index.
    pub(crate) fn current(store_root: &Path) -> Result<Self, String> {
        let path = index_path(store_root)?;
        let tips = current_tips(store_root)?;
        let stored = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<HistoryIndex>(&bytes).ok())
            .filter(|index| index.version == INDEX_VERSION)
            // Anchors go on git command lines: anything but an object name
            // is a corrupt index (rebuilt), never an option. trace:BUG-2554 | ai:claude
            .filter(|index| index.tips.iter().all(|t| is_object_name(t)));
        let index = match stored {
            // trace:BUG-2554 | ai:claude
            Some(mut index) if all_exist(store_root, &index.tips)? => {
                let known: HashSet<&String> = index.tips.iter().collect();
                let fresh: Vec<String> = tips
                    .iter()
                    .filter(|t| !known.contains(t))
                    .cloned()
                    .collect();
                if fresh.is_empty() {
                    note("current");
                    return Ok(index);
                }
                let mut range: Vec<&str> = fresh.iter().map(String::as_str).collect();
                range.push("--not");
                range.extend(index.tips.iter().map(String::as_str));
                let newer = Self::walk(store_root, &range)?;
                index.prepend(newer);
                note("incremental");
                // Keep an anchor for every history still held: the live
                // start points, and whatever older anchor they do not reach.
                let mut anchors: BTreeSet<String> = index.tips.iter().cloned().collect();
                anchors.extend(fresh);
                let mut anchors = independent(store_root, &anchors)?;
                anchors.extend(tips.iter().cloned());
                index.tips = anchors.into_iter().collect();
                index
            }
            _ => {
                note("rebuild");
                // The start points just listed, not a second `--all`: a ref
                // moving meanwhile must not leave an anchor whose history
                // was never walked.
                let mut index = if tips.is_empty() {
                    Self {
                        version: INDEX_VERSION,
                        ..Self::default()
                    }
                } else {
                    let range: Vec<&str> = tips.iter().map(String::as_str).collect();
                    Self::walk(store_root, &range)?
                };
                index.tips = tips;
                index
            }
        };
        // Best effort: an unwritable git dir still answers from this build.
        if let Some(dir) = path.parent() {
            if std::fs::create_dir_all(dir).is_ok() {
                if let Ok(bytes) = serde_json::to_vec(&index) {
                    let _ = aida_core::fs_atomic::write_atomic(&path, bytes);
                }
            }
        }
        Ok(index)
    }

    /// Index the commits `range` selects: the object versions they add or
    /// change, and the ID lines they add or remove.
    fn walk(store_root: &Path, range: &[&str]) -> Result<Self, String> {
        let mut args: Vec<&str> = vec![
            "log",
            "--format=%x00%H",
            "--no-renames",
            "--diff-filter=ACMRT",
            "--name-only",
        ];
        args.extend(range);
        args.extend(["--", "objects"]);
        let log = super::store_git(store_root, &args, None)?;
        let mut versions: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for record in String::from_utf8_lossy(&log).split('\0').skip(1) {
            let mut lines = record.lines().map(str::trim).filter(|l| !l.is_empty());
            let Some(sha) = lines.next() else { continue };
            for file in lines {
                versions
                    .entry(file.to_string())
                    .or_default()
                    .push(sha.to_string());
            }
        }
        let mut aliases: BTreeMap<String, IndexedAlias> = BTreeMap::new();
        for (id, hit) in super::alias_walk(store_root, range, ANY_ID_LINE)? {
            let entry = aliases.entry(id).or_default();
            match hit {
                Ok(stems) => {
                    for stem in stems {
                        if !entry.stems.contains(&stem) {
                            entry.stems.push(stem);
                        }
                    }
                }
                Err(refusal) => {
                    entry.refusal.get_or_insert(refusal);
                }
            }
        }
        Ok(Self {
            version: INDEX_VERSION,
            tips: Vec::new(),
            versions,
            aliases,
        })
    }

    /// Merge `newer` (the walk of commits newer than every indexed one) in
    /// front of what the index holds, skipping anything already recorded.
    fn prepend(&mut self, newer: Self) {
        for (path, shas) in newer.versions {
            let old = self.versions.entry(path).or_default();
            let seen: HashSet<String> = old.iter().cloned().collect();
            let mut merged: Vec<String> = shas.into_iter().filter(|s| !seen.contains(s)).collect();
            merged.append(old);
            *old = merged;
        }
        for (id, alias) in newer.aliases {
            let old = self.aliases.entry(id).or_default();
            let mut stems: Vec<String> = alias
                .stems
                .into_iter()
                .filter(|s| !old.stems.contains(s))
                .collect();
            stems.append(&mut old.stems);
            old.stems = stems;
            if let Some(refusal) = alias.refusal {
                old.refusal = Some(refusal);
            }
        }
    }
}
