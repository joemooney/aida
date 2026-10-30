//! M1 slice 2: merge holds on the gate ledger. A placement appends a `hold`
//! event with the full record, a release appends `hold-release`, and nothing
//! deletes evidence: "a hold existed and was cleared" is two lines on the
//! change's ledger (AC3). The open hold is a fold over those events.
//!
//! Legacy `.aida/merge-holds/PR-<N>` markers are read through ([`state`]): a
//! marker counts while the ledger has no placement or release for the PR, or
//! when the marker is newer than the ledger's last one (a binary from before
//! slice 2 wrote it). An unreadable marker still holds (TASK-1238). The first
//! ledger write that is not itself a placement imports a marker-held hold as a
//! `hold` event, so the release that follows answers a hold on the ledger.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// mandate-gate

use super::event::{EventBody, GateEvent};
use super::{legacy, store, store_root, ChangeKey};
use crate::gate::merge_hold_core::{parse_hold_marker, MergeHoldRecord};
use crate::gate::review_verdict::JsonObj;
use std::path::{Path, PathBuf};

/// Where the open hold was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenSource {
    /// A `hold` event on the ledger.
    Ledger,
    /// A legacy marker file, read through.
    LegacyMarker,
}

/// The hold state of one PR, folded from its ledger with legacy read-through.
#[derive(Debug, Clone, Default)]
pub(crate) struct HoldState {
    /// The open hold, if any.
    pub open: Option<MergeHoldRecord>,
    pub open_source: Option<OpenSource>,
    /// When the open hold was first placed: its first `hold` event since the
    /// last release, or the marker's mtime.
    pub placed_at: Option<std::time::SystemTime>,
    /// When the open hold's record was last written: its newest `hold`
    /// event, or the marker's mtime.
    pub written_at: Option<std::time::SystemTime>,
    /// The clearance of the release that closed the latest hold; `None`
    /// while a hold is open or when none was ever released.
    pub release: Option<JsonObj>,
    /// An automatic release authorized for the open hold and not withdrawn.
    pub authorized: Option<JsonObj>,
    /// Whether the ledger holds any placement or release for the PR.
    pub on_ledger: bool,
}

fn parse_at(at: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(at.trim())
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

/// The ledger half of [`state`]: a pure fold over the change's events.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn fold(events: &[GateEvent]) -> HoldState {
    let mut st = HoldState::default();
    for event in events {
        match &event.body {
            EventBody::Hold { hold, .. } => {
                let at = parse_at(&event.at).map(std::time::SystemTime::from);
                if st.open.is_none() {
                    st.placed_at = at;
                }
                st.written_at = at;
                st.open = Some(hold.clone());
                st.open_source = Some(OpenSource::Ledger);
                st.release = None;
                // A pending authorization survives a bare `hold` event: the
                // release path's second pass refuses a changed hold and
                // withdraws it on the PR (TASK-1757). A placement through
                // `merge_hold_core` supersedes it with an explicit
                // `release-withdrawn` first, as the legacy placement removed
                // the clearance file.
                st.on_ledger = true;
            }
            EventBody::HoldRelease { clearance } => {
                st.open = None;
                st.open_source = None;
                st.placed_at = None;
                st.written_at = None;
                st.authorized = None;
                st.release = Some(clearance.clone());
                st.on_ledger = true;
            }
            EventBody::ReleaseAuthorized { clearance } if st.open.is_some() => {
                st.authorized = Some(clearance.clone());
            }
            EventBody::ReleaseWithdrawn { .. } => st.authorized = None,
            EventBody::LabelSync { label_state } => {
                if let Some(open) = st.open.as_mut() {
                    open.label_state = Some(label_state.clone());
                }
            }
            _ => {}
        }
    }
    st
}

/// The root whose ledger `start` reads: its store root, or `start` itself
/// for a seat clone that does not verify (it then reads its own files, as
/// before slice 2, rather than answering "no hold").
pub(crate) fn read_root(start: &Path) -> PathBuf {
    store_root(start).unwrap_or_else(|_| start.to_path_buf())
}

/// PR `pr`'s hold state, read from `start` (any checkout sharing the ledger).
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn state(start: &Path, pr: u64) -> HoldState {
    state_at(&read_root(start), pr)
}

/// [`state`] at a resolved store root.
pub(crate) fn state_at(root: &Path, pr: u64) -> HoldState {
    let events = store::read_file(root, ChangeKey::pr(pr));
    let mut st = fold(&events);
    let marker = legacy::hold_marker_path(root, pr);
    if let Ok(meta) = std::fs::symlink_metadata(&marker) {
        let last = events
            .iter()
            .filter(|e| e.body.is_hold_transition())
            .filter_map(|e| parse_at(&e.at))
            .next_back();
        let mtime = meta.modified().ok();
        let newer = match (last, mtime) {
            (None, _) => true,
            (Some(last), Some(m)) => chrono::DateTime::<chrono::Utc>::from(m) > last,
            // No mtime to order by: the marker holds (fail closed).
            (Some(_), None) => true,
        };
        if newer {
            st.open = parse_hold_marker(std::fs::read_to_string(&marker), pr);
            st.open_source = st.open.as_ref().map(|_| OpenSource::LegacyMarker);
            st.placed_at = mtime;
            st.written_at = mtime;
            st.release = None;
            st.authorized = None;
        }
    } else if !st.on_ledger {
        st.release = std::fs::read(legacy::clearance_file_path(root, pr))
            .ok()
            .and_then(|b| serde_json::from_slice::<JsonObj>(&b).ok());
    }
    st
}

/// The PRs whose ledger or legacy marker directory may hold a hold, read
/// from `start`, ascending. Ledger files are pre-filtered on the event kind
/// text so a change with only verdicts is not parsed.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn candidate_prs(start: &Path) -> Vec<u64> {
    let root = read_root(start);
    let mut prs = legacy::marker_prs(&root);
    for entry in std::fs::read_dir(super::ledger_dir(&root))
        .into_iter()
        .flatten()
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(change) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(ChangeKey::parse)
        else {
            continue;
        };
        if change.forge != super::ChangeForge::Pr {
            continue;
        }
        let holds =
            std::fs::read_to_string(&path).is_ok_and(|body| body.contains("\"kind\":\"hold\""));
        if holds {
            prs.push(change.number);
        }
    }
    prs.sort_unstable();
    prs.dedup();
    prs
}

/// Append one hold-family event for PR `pr` from `start`. Uncontained
/// primary-side code only: a contained caller is refused, and there is no
/// worktree-local fallback (AC5). A hold still read from a legacy marker is
/// imported as a `hold` event first, so a release or label sync answers a
/// hold the ledger itself carries.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn append(
    start: &Path,
    pr: u64,
    body: EventBody,
    head: Option<&str>,
    spec: Option<&str>,
) -> std::io::Result<GateEvent> {
    if crate::session::process_contained() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "a contained seat cannot write merge-hold state; the launcher that started it arms \
             holds for the verdicts it collects",
        ));
    }
    if pr == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a merge hold needs a PR number",
        ));
    }
    let root = store_root(start)?;
    let change = ChangeKey::pr(pr);
    let mut events = Vec::new();
    let current = state_at(&root, pr);
    if current.open_source == Some(OpenSource::LegacyMarker)
        && !matches!(body, EventBody::Hold { .. })
    {
        if let Some(open) = current.open.clone() {
            let mut import = GateEvent::new(
                change,
                EventBody::Hold {
                    hold: open.clone(),
                    rearmed: false,
                },
            );
            import.head = open.target_head_sha.clone();
            import.spec = open.spec.clone();
            events.push(import);
        }
    }
    let mut event = GateEvent::new(change, body);
    event.head = head
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(str::to_string);
    event.spec = spec
        .map(|s| s.trim().to_ascii_uppercase())
        .filter(|s| !s.is_empty() && ChangeKey::parse(s).is_none());
    if let Some(head) = event.head.as_deref() {
        (event.base_ref, event.base_tip, event.patch_id) =
            super::event::patch_identity(&root, head);
    }
    events.push(event);
    let appended = store::append(&root, change, events)?;
    appended
        .into_iter()
        .next_back()
        .ok_or_else(|| std::io::Error::other("the hold event was not appended"))
}

/// Append the `hold` event for `record`.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn place(start: &Path, record: &MergeHoldRecord, rearmed: bool) -> std::io::Result<()> {
    append(
        start,
        record.pr,
        EventBody::Hold {
            hold: record.clone(),
            rearmed,
        },
        record.target_head_sha.as_deref(),
        record.spec.as_deref(),
    )
    .map(|_| ())
}

/// Append the `hold-release` event with `clearance` as its audit record.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn release(start: &Path, pr: u64, clearance: JsonObj) -> std::io::Result<()> {
    let head = clearance
        .get("target_head_sha")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    append(
        start,
        pr,
        EventBody::HoldRelease { clearance },
        head.as_deref(),
        None,
    )
    .map(|_| ())
}
