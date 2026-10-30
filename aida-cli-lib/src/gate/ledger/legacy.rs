//! Read-only access to the legacy files a change's ledger replaces:
//! `.aida/review-verdicts/<CHANGE>.json` and its per-commit archive
//! `.aida/review-verdicts/<CHANGE>/<sha>.json`, the merge-hold marker
//! `.aida/merge-holds/PR-<N>` and its clearance
//! `.aida/merge-hold-clearances/PR-<N>.json`. Code here never writes or
//! deletes them (signoff Q4: read-through, no bulk rewrite).
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// mandate-gate

use super::event::{EventBody, GateEvent, LegacyFile};
use super::ChangeKey;
use crate::gate::review_verdict as rv;
use std::path::{Path, PathBuf};

/// The change's legacy verdict files, current file first, then archived
/// rounds by file name. Paths are relative to `root`.
pub(crate) fn verdict_files(root: &Path, change: ChangeKey) -> Vec<LegacyFile> {
    let current = rv::verdict_path(root, &change.as_key());
    let mut out = Vec::new();
    let rel = |p: &Path| {
        p.strip_prefix(root)
            .unwrap_or(p)
            .to_string_lossy()
            .into_owned()
    };
    if let Ok(body) = rv::read_verdict_body(&current) {
        out.push(LegacyFile {
            path: rel(&current),
            body,
        });
    }
    let Some(dir) = rv::verdict_archive_dir(&current) else {
        return out;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    let mut archived: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    archived.sort();
    for path in archived {
        if let Ok(body) = rv::read_verdict_body(&path) {
            out.push(LegacyFile {
                path: rel(&path),
                body,
            });
        }
    }
    out
}

/// The legacy merge-hold marker directory under `root`.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn marker_dir(root: &Path) -> PathBuf {
    root.join(".aida").join("merge-holds")
}

/// The legacy merge-hold marker for PR `pr` under the store root `root`.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn hold_marker_path(root: &Path, pr: u64) -> PathBuf {
    marker_dir(root).join(format!("PR-{pr}"))
}

/// The legacy clearance record for PR `pr` under the store root `root`.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn clearance_file_path(root: &Path, pr: u64) -> PathBuf {
    root.join(".aida")
        .join("merge-hold-clearances")
        .join(format!("PR-{pr}.json"))
}

/// The PRs with a legacy marker file under `root`, ascending.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn marker_prs(root: &Path) -> Vec<u64> {
    let dir = marker_dir(root);
    let mut prs: Vec<u64> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_prefix("PR-")?
                .parse::<u64>()
                .ok()
        })
        .collect();
    prs.sort_unstable();
    prs.dedup();
    prs
}

/// The legacy hold files of a PR change, verbatim, for its import event.
fn hold_files(root: &Path, change: ChangeKey) -> Vec<LegacyFile> {
    if change.forge != super::ChangeForge::Pr {
        return Vec::new();
    }
    [
        hold_marker_path(root, change.number),
        clearance_file_path(root, change.number),
    ]
    .into_iter()
    .filter_map(|path| {
        let body = std::fs::read_to_string(&path).ok()?;
        Some(LegacyFile {
            path: path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned(),
            body,
        })
    })
    .collect()
}

/// The `legacy-imported` event for a change's first ledger write. Always
/// returned, even with no files, so the import happens exactly once.
pub(crate) fn import_event(root: &Path, change: ChangeKey) -> GateEvent {
    let mut legacy = verdict_files(root, change);
    legacy.extend(hold_files(root, change));
    GateEvent::new(change, EventBody::LegacyImported { legacy })
}

/// The root and change of a PR-keyed legacy verdict path laid out as
/// `<root>/.aida/review-verdicts/PR-<N>.json`; `None` for a spec-keyed path
/// or any other layout.
pub(crate) fn change_of_verdict_path(path: &Path) -> Option<(PathBuf, ChangeKey)> {
    let change = ChangeKey::parse(path.file_stem()?.to_str()?)?;
    let dir = path.parent()?;
    let aida = dir.parent()?;
    if dir.file_name()? != "review-verdicts" || aida.file_name()? != ".aida" {
        return None;
    }
    Some((aida.parent()?.to_path_buf(), change))
}
