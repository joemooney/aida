//! Culprits of red trains: a PR red alone on top of main is not re-admitted
//! to a train at the same head until the head changes or an operator names
//! it explicitly (`aida pr train <PR>`), which is recorded as a clearance.
// trace:BUG-2767 | ai:claude

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
}

/// Outside `.aida/train`, whose every `*.json` is a train journal.
fn path(root: &Path) -> PathBuf {
    root.join(".aida/train-culprits.json")
}

fn load(root: &Path) -> Result<BTreeMap<u64, Culprit>> {
    match std::fs::read(path(root)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("culprit record {} is unreadable", path(root).display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path(root).display())),
    }
}

fn save(root: &Path, rows: &BTreeMap<u64, Culprit>) -> Result<()> {
    let target = path(root);
    std::fs::create_dir_all(target.parent().unwrap_or(root))?;
    let temporary = target.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(rows)?)?;
    std::fs::rename(&temporary, &target).with_context(|| format!("writing {}", target.display()))
}

/// Record `pr` at `head` as the culprit of `train`.
pub(crate) fn record(root: &Path, pr: u64, head: &str, train: Option<u64>) -> Result<()> {
    let mut rows = load(root)?;
    rows.insert(
        pr,
        Culprit {
            head: head.to_string(),
            train,
            at: chrono::Utc::now(),
            cleared_by: None,
        },
    );
    save(root, &rows)
}

/// The hold on `pr` at `head`, worded for a refusal. A row for another head
/// or a cleared row holds nothing. An unreadable record holds (fail closed).
pub(crate) fn hold(root: &Path, pr: u64, head: &str) -> Option<String> {
    let rows = match load(root) {
        Ok(rows) => rows,
        Err(e) => return Some(format!("held: {e:#}")),
    };
    let row = rows.get(&pr)?;
    if row.cleared_by.is_some() || !row.head.eq_ignore_ascii_case(head) {
        return None;
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
    if hold(root, pr, head).is_none() {
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

    #[test]
    fn bug2767_unreadable_record_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(path(dir.path()), b"{not json").unwrap();
        assert!(hold(dir.path(), 1, "abc").unwrap().starts_with("held: "));
    }
}
