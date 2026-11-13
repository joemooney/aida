//! No-write gate-ledger observations for reports. Missing evidence remains unknown.
// trace:STORY-2704 | ai:codex
use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
fn read_ledger(path: &Path) -> Result<(Vec<Value>, bool)> {
    let body = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], false)),
        Err(e) => return Err(e.into()),
    };
    Ok((
        body.lines()
            .filter(|s| !s.trim().is_empty())
            .enumerate()
            .map(|(i, s)| {
                serde_json::from_str(s)
                    .with_context(|| format!("{}:{}: malformed ledger", path.display(), i + 1))
            })
            .collect::<Result<_>>()?,
        true,
    ))
}
/// Read the available events for a GitHub PR. The boolean distinguishes a
/// missing ledger from an existing empty ledger. Malformed/unreadable evidence
/// is an error; this API never initializes a ledger or derived state.
pub fn read_pr(shared: &Path, pr: u64) -> Result<(Vec<Value>, bool)> {
    read_ledger(
        &shared
            .join(".aida/gate-ledger")
            .join(format!("PR-{pr}.jsonl")),
    )
}
// Train coverage is stored on the train PR, with member PRs in proof.sources.
pub fn train_members(shared: &Path) -> Result<BTreeMap<u64, BTreeSet<u64>>> {
    fn visit(value: &Value, train: u64, out: &mut BTreeMap<u64, BTreeSet<u64>>) {
        if value["kind"] == "train-coverage" {
            for source in value["proof"]["sources"].as_array().into_iter().flatten() {
                if let Some(member) = source["pr"].as_u64() {
                    out.entry(member).or_default().insert(train);
                }
            }
        }
        match value {
            Value::Object(o) => {
                for child in o.values() {
                    visit(child, train, out);
                }
            }
            Value::Array(a) => {
                for child in a {
                    visit(child, train, out);
                }
            }
            _ => (),
        }
    }
    let dir = shared.join(".aida/gate-ledger");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = BTreeMap::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(train) = name
            .strip_prefix("PR-")
            .and_then(|s| s.strip_suffix(".jsonl"))
            .and_then(|s| s.parse().ok())
        else {
            continue;
        };
        let (events, _) = read_ledger(&entry.path())?;
        for event in events {
            visit(&event, train, &mut out);
        }
    }
    Ok(out)
}

/// Resolve shared ledger evidence without Git subprocesses or filesystem writes.
pub fn store_root(start: &Path) -> Result<std::path::PathBuf> {
    if let Some(clone_root) = crate::seat_clone_detect::seat_clone_root(start) {
        return crate::seat_clone_detect::clone_writer_main(&clone_root)
            .context("seat clone does not verify against its main checkout");
    }
    Ok(primary_checkout_from_files(start))
}
fn primary_checkout_from_files(root: &Path) -> std::path::PathBuf {
    let dot_git = root.join(".git");
    let Ok(meta) = std::fs::symlink_metadata(&dot_git) else {
        return root.to_path_buf();
    };
    if !meta.is_file() {
        return root.to_path_buf();
    }
    let Some(gitdir) = std::fs::read_to_string(&dot_git).ok().and_then(|body| {
        body.lines()
            .find_map(|l| l.strip_prefix("gitdir:"))
            .map(|d| root.join(d.trim()))
    }) else {
        return root.to_path_buf();
    };
    let Ok(common) = std::fs::read_to_string(gitdir.join("commondir")) else {
        return root.to_path_buf();
    };
    let common = gitdir.join(common.trim());
    let common = std::fs::canonicalize(&common).unwrap_or(common);
    match (common.file_name(), common.parent()) {
        (Some(name), Some(main)) if name == ".git" => main.to_path_buf(),
        _ => root.to_path_buf(),
    }
}
