//! Read-only access to the legacy verdict files a change's ledger replaces:
//! `.aida/review-verdicts/<CHANGE>.json` and its per-commit archive
//! `.aida/review-verdicts/<CHANGE>/<sha>.json`. Code here never writes or
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

/// The `legacy-imported` event for a change's first ledger write. Always
/// returned, even with no files, so the import happens exactly once.
pub(crate) fn import_event(root: &Path, change: ChangeKey) -> GateEvent {
    GateEvent::new(
        change,
        EventBody::LegacyImported {
            legacy: verdict_files(root, change),
        },
    )
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
