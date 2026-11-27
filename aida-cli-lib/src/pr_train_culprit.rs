//! Culprits of red trains: a PR red alone on top of main is not re-admitted
//! to a train at the same head until the head changes or an operator names
//! it explicitly (`aida pr train <PR>`), which is recorded as a clearance.
//! A PR ejected because its diff changes when stacked (BUG-2777) is held at
//! that head even when named: no rerun can change what would land, so the
//! hold names the route the train took instead (branch update + re-review, or
//! conflict repair).
// trace:BUG-2767 trace:BUG-2777 | ai:claude

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One culprit row: the head that was red, and the train it was red in.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Culprit {
    pub head: String,
    #[serde(default)]
    pub train: Option<u64>,
    pub at: chrono::DateTime<chrono::Utc>,
    /// Who cleared the hold at this head, when it was cleared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared_by: Option<String>,
    /// Why the train ejected the PR. Rows written before BUG-2777 are reds.
    #[serde(default)]
    pub kind: Kind,
    /// The next step the train routed the PR to, for a drift row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
}

/// The typed ejection a row records. trace:BUG-2777 | ai:claude
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Kind {
    /// Red alone on top of main.
    #[default]
    Red,
    /// The car's patch-id differs from the approved diff.
    StackedDiffDrift,
}

/// Outside `.aida/train`, whose every `*.json` is a train journal.
fn path(root: &Path) -> PathBuf {
    root.join(".aida/train-culprits.json")
}

fn load(root: &Path) -> Result<BTreeMap<u64, Culprit>> {
    load_at(&path(root))
}

fn load_at<T: serde::de::DeserializeOwned>(file: &Path) -> Result<BTreeMap<u64, T>> {
    match std::fs::read(file) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("culprit record {} is unreadable", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
    }
}

fn save(root: &Path, rows: &BTreeMap<u64, Culprit>) -> Result<()> {
    save_at(root, &path(root), rows)
}

fn save_at<T: serde::Serialize>(root: &Path, target: &Path, rows: &BTreeMap<u64, T>) -> Result<()> {
    let target = target.to_path_buf();
    std::fs::create_dir_all(target.parent().unwrap_or(root))?;
    let temporary = target.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(rows)?)?;
    std::fs::rename(&temporary, &target).with_context(|| format!("writing {}", target.display()))
}

/// Record `pr` at `head` as the culprit of `train`. A drift row at the same
/// head takes precedence and is kept: naming clears a red, never a drift, so
/// a red written over it would let a rerun retry the drifted head.
// trace:BUG-2777 | ai:claude
pub(crate) fn record(root: &Path, pr: u64, head: &str, train: Option<u64>) -> Result<()> {
    let mut rows = load(root)?;
    if rows
        .get(&pr)
        .is_some_and(|r| r.kind == Kind::StackedDiffDrift && r.head.eq_ignore_ascii_case(head))
    {
        return Ok(());
    }
    rows.insert(
        pr,
        Culprit {
            head: head.to_string(),
            train,
            at: chrono::Utc::now(),
            cleared_by: None,
            kind: Kind::Red,
            route: None,
        },
    );
    save(root, &rows)
}

/// Record `pr` ejected at `head` for stacked-diff drift, routed to `route`.
// trace:BUG-2777 | ai:claude
pub(crate) fn record_drift(
    root: &Path,
    pr: u64,
    head: &str,
    train: Option<u64>,
    route: &str,
) -> Result<()> {
    let mut rows = load(root)?;
    rows.insert(
        pr,
        Culprit {
            head: head.to_string(),
            train,
            at: chrono::Utc::now(),
            cleared_by: None,
            kind: Kind::StackedDiffDrift,
            route: Some(route.to_string()),
        },
    );
    save(root, &rows)
}

/// The drift row on `pr` at `head`, if any: (when it was ejected, the route).
// trace:BUG-2777 | ai:claude
pub(crate) fn drift(
    root: &Path,
    pr: u64,
    head: &str,
) -> Option<(chrono::DateTime<chrono::Utc>, String)> {
    let row = load(root).ok()?.remove(&pr)?;
    (row.kind == Kind::StackedDiffDrift && row.head.eq_ignore_ascii_case(head))
        .then(|| (row.at, row.route.unwrap_or_default()))
}

/// The hold a named PR cannot clear: a drift ejection at this very head. An
/// unreadable record holds (fail closed).
// trace:BUG-2777 | ai:claude
pub(crate) fn drift_hold(root: &Path, pr: u64, head: &str) -> Option<String> {
    if let Err(e) = load(root) {
        return Some(format!("held: {e:#}"));
    }
    let (_, route) = drift(root, pr, head)?;
    Some(format!(
        "held: ejected for stacked-diff drift at {} (the approval does not cover what \
         would land, and re-training the same head cannot change that); routed: {route}",
        crate::pr_train::short(head)
    ))
}

/// An advisor's FIX-FIRST ruling on a head. Kept in its own file, so a binary
/// that predates it ignores the ruling instead of failing closed on every PR.
// trace:TASK-3923 | ai:claude
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct FixFirst {
    pub head: String,
    pub at: chrono::DateTime<chrono::Utc>,
    pub ruled_by: String,
    pub reason: String,
}

fn fix_first_path(root: &Path) -> PathBuf {
    root.join(".aida/train-fix-first.json")
}

/// How a fix-first refusal starts, so `--check` can name its gate.
pub(crate) const FIX_FIRST: &str = "held: FIX-FIRST ruling";

/// Record a fix-first ruling on `pr` at `head`. Only a new head clears it;
/// naming the PR on `aida pr train` does not. `false`: already ruled here.
// trace:TASK-3923 | ai:claude
pub(crate) fn record_fix_first(
    root: &Path,
    pr: u64,
    head: &str,
    by: &str,
    reason: &str,
) -> Result<bool> {
    let file = fix_first_path(root);
    let mut rows: BTreeMap<u64, FixFirst> = load_at(&file)?;
    if rows
        .get(&pr)
        .is_some_and(|r| r.head.eq_ignore_ascii_case(head))
    {
        return Ok(false);
    }
    let (ruled_by, reason) = (by.to_string(), reason.to_string());
    let (head, at) = (head.to_string(), chrono::Utc::now());
    rows.insert(
        pr,
        FixFirst {
            head,
            at,
            ruled_by,
            reason,
        },
    );
    save_at(root, &file, &rows)?;
    Ok(true)
}

/// The fix-first ruling holding `pr` at `head`, worded for a refusal. An
/// unreadable record holds (fail closed); a ruling on another head holds nothing.
// trace:TASK-3923 | ai:claude
pub(crate) fn fix_first_hold(root: &Path, pr: u64, head: &str) -> Option<String> {
    let row = match load_at::<FixFirst>(&fix_first_path(root)) {
        Ok(mut rows) => rows
            .remove(&pr)
            .filter(|r| r.head.eq_ignore_ascii_case(head))?,
        Err(e) => return Some(format!("held: {e:#}")),
    };
    let short = crate::pr_train::short(head);
    Some(format!(
        "{FIX_FIRST} at {short} by {}: {}",
        row.ruled_by,
        row.reason.trim()
    ))
}

/// The hold on `pr` at `head`, worded for a refusal. A row for another head
/// or a cleared row holds nothing. An unreadable record holds (fail closed).
pub(crate) fn hold(root: &Path, pr: u64, head: &str) -> Option<String> {
    // A ruling outranks a culprit row. trace:TASK-3923 | ai:claude
    if let Some(why) = fix_first_hold(root, pr, head) {
        return Some(why);
    }
    let rows = match load(root) {
        Ok(rows) => rows,
        Err(e) => return Some(format!("held: {e:#}")),
    };
    let row = rows.get(&pr)?;
    if row.cleared_by.is_some() || !row.head.eq_ignore_ascii_case(head) {
        return None;
    }
    if row.kind == Kind::StackedDiffDrift {
        return drift_hold(root, pr, head);
    }
    let train = row
        .train
        .map_or_else(|| "a train".into(), |n| format!("train #{n}"));
    Some(format!(
        "held: culprit of {train} at {}; push a new head, or name it explicitly \
         (`aida pr train {pr}`) to clear the hold",
        crate::pr_train::short(&row.head)
    ))
}

/// The operator named `pr`: record the clearance of a hold at `head`.
/// Returns whether a hold was cleared.
pub(crate) fn clear(root: &Path, pr: u64, head: &str, by: &str) -> Result<bool> {
    // A drift hold is cleared only by a new head. trace:BUG-2777 | ai:claude
    // Nor is a fix-first ruling. trace:TASK-3923 | ai:claude
    if hold(root, pr, head).is_none()
        || drift(root, pr, head).is_some()
        || fix_first_hold(root, pr, head).is_some()
    {
        return Ok(false);
    }
    let mut rows = load(root)?;
    if let Some(row) = rows.get_mut(&pr) {
        row.cleared_by = Some(by.to_string());
    }
    save(root, &rows)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bug2767_same_head_is_held_new_head_and_clearance_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(hold(root, 4482, "abc"), None, "no record holds nothing");
        record(root, 4482, "abcdef0123", Some(4609)).unwrap();
        let why = hold(root, 4482, "ABCDEF0123").expect("same head is held");
        assert!(why.starts_with("held: culprit of train #4609 at "), "{why}");
        assert_eq!(hold(root, 4482, "fedcba9876"), None, "a new head rides");
        assert_eq!(hold(root, 4475, "abcdef0123"), None, "other PRs ride");
        assert!(!clear(root, 4482, "fedcba9876", "op").unwrap());
        assert!(clear(root, 4482, "abcdef0123", "joe").unwrap());
        assert_eq!(hold(root, 4482, "abcdef0123"), None, "cleared");
        let rows = load(root).unwrap();
        assert_eq!(rows[&4482].cleared_by.as_deref(), Some("joe"), "recorded");
        assert!(!root.join(".aida/train").exists(), "never a journal file");
    }

    // trace:BUG-2777 | ai:claude
    #[test]
    fn bug2777_drift_row_is_typed_and_naming_does_not_clear_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let route = "branch updated; independent re-review queued at the new head";
        record_drift(root, 4464, "7d711a72aa", Some(4650), route).unwrap();
        let raw = std::fs::read_to_string(path(root)).unwrap();
        assert!(raw.contains("\"kind\": \"stacked-diff-drift\""), "{raw}");
        let why = hold(root, 4464, "7D711A72AA").expect("same head is held");
        assert!(
            why.contains("stacked-diff drift") && why.ends_with(route),
            "{why}"
        );
        assert_eq!(drift_hold(root, 4464, "7D711A72AA"), Some(why));
        assert!(
            !clear(root, 4464, "7d711a72aa", "joe").unwrap(),
            "naming never clears drift"
        );
        assert!(
            drift_hold(root, 4464, "7d711a72aa").is_some(),
            "still held when named"
        );
        assert_eq!(hold(root, 4464, "0123456789"), None, "a new head rides");
        assert_eq!(drift_hold(root, 4464, "0123456789"), None);
        record(root, 4482, "abcdef0123", None).unwrap();
        assert_eq!(
            drift_hold(root, 4482, "abcdef0123"),
            None,
            "a red is not drift"
        );
        assert_eq!(load(root).unwrap()[&4482].kind, Kind::Red);
        // A red at the drifted head keeps the drift row; a red at a new head
        // replaces it.
        record(root, 4464, "7d711a72aa", Some(4700)).unwrap();
        assert_eq!(load(root).unwrap()[&4464].kind, Kind::StackedDiffDrift);
        assert!(!clear(root, 4464, "7d711a72aa", "joe").unwrap());
        record(root, 4464, "0123456789", Some(4700)).unwrap();
        assert_eq!(load(root).unwrap()[&4464].kind, Kind::Red);
    }

    #[test]
    fn bug2767_unreadable_record_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(path(dir.path()), b"{not json").unwrap();
        assert!(hold(dir.path(), 1, "abc").unwrap().starts_with("held: "));
        assert!(drift_hold(dir.path(), 1, "abc")
            .unwrap()
            .starts_with("held: "));
    }
}
