//! BUG-2403: one selection of a requirement's current review verdict across
//! every label the SAME requirement is known by (its native `spec_id` and its
//! friendly `agreed_id`).
//!
//! The gate ledger and the legacy verdict files are partitioned by the
//! literal label a writer used. Older holds and reworks were recorded under
//! the friendly label, current native reviewers record the canonical one, so
//! each label's partition answers with its own newest record. A reader that
//! took the first label that answered (the old `read_recorded_verdict_any`,
//! the old rework display-first lookup) replayed a stale alias over a later
//! record for the same requirement.
//!
//! Here every proved label's current record is read — the legacy file
//! through the anchored reader, the ledger through the strict
//! [`checked`](crate::gate::ledger::checked) reader — and the newest by
//! primary-side order time ([`rv::ledger_order_time`]) answers. The result
//! distinguishes absent, selected, ambiguous and unsafe: a tie between
//! different records, a record with no order time, or refused evidence is
//! never turned into an ordinary missing verdict, and never falls back to
//! some older approval.
//!
//! Identity proof: labels are equal only because the caller read them off
//! ONE requirement object, or [`snapshot_labels`] proved them against ONE
//! committed store snapshot: the store's objects and its
//! `registry/node_aliases.toml` / `registry/retired_ids.toml` mappings clean
//! and read at a pinned HEAD that did not move while they were read, with
//! every bridged label resolving to the same full requirement UUID and no
//! other requirement claiming it. No prefix, substring, UUID fragment, node or
//! sequence comparison links labels; a label that does not resolve stays
//! literal, and an unprovable snapshot (dirty, moved, unreadable, ambiguous)
//! bridges nothing. Selecting a record grants nothing: every authority consumer
//! still validates the selected record's own receipt, attestation and head.
// trace:BUG-2403 | ai:claude
// mandate-gate

use crate::gate::ledger::checked::{self, Budget, Unsafe};
use crate::gate::ledger::{self, GateEvent};
use crate::gate::review_verdict::{self as rv, RecordedVerdict, VerdictSource};
use chrono::{DateTime, FixedOffset};
use std::path::Path;

/// The record a cross-label selection chose, with where it came from.
#[derive(Debug, Clone)]
pub(crate) struct Selected {
    pub verdict: RecordedVerdict,
    /// The record's JSON body, as the gates weigh it.
    pub body: String,
    /// The label whose partition answered.
    pub label: String,
    pub source: VerdictSource,
    /// Primary-side order time; `None` only for a single-label read.
    pub order_time: Option<DateTime<FixedOffset>>,
}

/// The four outcomes amendment 3 requires a selector to keep apart.
#[derive(Debug, Clone)]
pub(crate) enum AliasSelection {
    /// No label holds a verdict.
    Absent,
    /// One current record answers for every label.
    Selected(Box<Selected>),
    /// Different records are equally current, or one cannot be ordered.
    /// `refusal` is the newest blocking candidate, if any: a refusal stays
    /// visible; no approval is chosen.
    Ambiguous {
        reason: String,
        refusal: Option<RecordedVerdict>,
    },
    /// Relevant evidence was refused (linked, oversized, torn, unparseable,
    /// over budget). `refusal` as for `Ambiguous`.
    Unsafe {
        reason: String,
        refusal: Option<RecordedVerdict>,
    },
}

impl AliasSelection {
    /// The verdict an `Option` API may return without losing refusal: the
    /// selected record; for ambiguous/unsafe evidence only a blocking
    /// candidate (never an approval, never `Proceed`-shaped credit).
    pub(crate) fn fail_closed(self) -> Option<RecordedVerdict> {
        match self {
            Self::Absent => None,
            Self::Selected(s) => Some(s.verdict),
            Self::Ambiguous { refusal, .. } | Self::Unsafe { refusal, .. } => refusal,
        }
    }

    /// Why a cross-label selection could not choose, if it could not.
    pub(crate) fn unavailable_reason(&self) -> Option<&str> {
        match self {
            Self::Ambiguous { reason, .. } | Self::Unsafe { reason, .. } => Some(reason.as_str()),
            _ => None,
        }
    }
}

/// The distinct spec labels in `ids`, upper-cased, in caller order: empty,
/// placeholder (`???`) and change keys (`PR-N`/`MR-N`, which are their own
/// ledgers, never aliases of a spec) are dropped.
pub(crate) fn spec_labels(ids: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        let id = id.trim();
        if id.is_empty() || id == "???" || ledger::ChangeKey::parse(id).is_some() {
            continue;
        }
        let up = id.to_ascii_uppercase();
        if !out.contains(&up) {
            out.push(up);
        }
    }
    out
}

// ── Snapshot-bound identity (signoff amendment 1) ─────────────────────────

/// Committed node-label map, relative to the store root.
const NODE_ALIASES_REL: &str = "registry/node_aliases.toml";
/// Committed retired-ID map, relative to the store root.
const RETIRED_IDS_REL: &str = "registry/retired_ids.toml";
/// The canonical object directory, relative to the store root.
const OBJECTS_REL: &str = "objects";
/// One snapshot git query's wall-clock bound (amendment 7).
const SNAPSHOT_GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// One snapshot git query's output bound.
const SNAPSHOT_GIT_MAX_OUTPUT: usize = 4 * 1024 * 1024;
/// Node-label bindings one snapshot may hold.
const MAX_ALIAS_BINDINGS: usize = 1_024;
/// Retired IDs one snapshot may hold.
const MAX_RETIRED_IDS: usize = 65_536;

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct AliasFile {
    #[serde(default, rename = "binding")]
    bindings: Vec<AliasBinding>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct AliasBinding {
    label: String,
    node: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct RetiredFile {
    #[serde(default, rename = "retired")]
    entries: Vec<RetiredEntry>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RetiredEntry {
    old: String,
    new: String,
    uuid: String,
}

/// The label mappings of ONE committed store snapshot: every node-label
/// binding (current and retired labels alike resolve for input) and every
/// retired local ID.
#[derive(Debug, Clone, Default)]
pub(crate) struct SnapshotMaps {
    bindings: Vec<AliasBinding>,
    retired: Vec<RetiredEntry>,
}

impl SnapshotMaps {
    /// Parse the committed mapping files' bodies (`None`: not committed).
    pub(crate) fn parse(aliases: Option<&str>, retired: Option<&str>) -> Result<Self, String> {
        let aliases: AliasFile = match aliases {
            Some(t) => toml::from_str(t).map_err(|e| format!("{NODE_ALIASES_REL}: {e}"))?,
            None => AliasFile::default(),
        };
        let retired: RetiredFile = match retired {
            Some(t) => toml::from_str(t).map_err(|e| format!("{RETIRED_IDS_REL}: {e}"))?,
            None => RetiredFile::default(),
        };
        if aliases.bindings.len() > MAX_ALIAS_BINDINGS || retired.entries.len() > MAX_RETIRED_IDS {
            return Err("the committed label mappings exceed their bounds".into());
        }
        Ok(Self {
            bindings: aliases.bindings,
            retired: retired.entries,
        })
    }
}

fn eq_label(a: Option<&str>, b: &str) -> bool {
    a.is_some_and(|a| a.trim().eq_ignore_ascii_case(b.trim()))
}

/// `TYPE-MIDDLE-SEQ` with an alphabetic type and a numeric sequence.
fn split_id(id: &str) -> Option<(&str, &str, &str)> {
    let mut parts = id.trim().splitn(3, '-');
    let (prefix, middle, seq) = (parts.next()?, parts.next()?, parts.next()?);
    let ok = prefix
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && prefix.chars().all(|c| c.is_ascii_alphanumeric())
        && !middle.is_empty()
        && !seq.is_empty()
        && seq.bytes().all(|b| b.is_ascii_digit());
    ok.then_some((prefix, middle, seq))
}

/// The requirements claiming `label` as their native or friendly ID.
fn claimants<'a>(
    reqs: &'a [aida_core::Requirement],
    label: &str,
) -> Vec<&'a aida_core::Requirement> {
    let mut out: Vec<&aida_core::Requirement> = reqs
        .iter()
        .filter(|r| {
            eq_label(r.spec_id.as_deref(), label) || eq_label(r.agreed_id.as_deref(), label)
        })
        .collect();
    out.dedup_by(|a, b| a.id == b.id);
    out
}

/// The requirement `key` resolves to through the snapshot's mappings: a
/// node-label form expanded to its canonical native ID, or a retired ID whose
/// recorded UUID is that requirement's.
fn mapped<'a>(
    reqs: &'a [aida_core::Requirement],
    maps: &SnapshotMaps,
    key: &str,
) -> Vec<&'a aida_core::Requirement> {
    let mut out: Vec<&aida_core::Requirement> = Vec::new();
    if let Some((prefix, middle, seq)) = split_id(key) {
        for b in maps
            .bindings
            .iter()
            .filter(|b| b.label.eq_ignore_ascii_case(middle))
        {
            let seq = seq
                .parse::<u64>()
                .map_or_else(|_| seq.to_string(), |n| format!("{n:03}"));
            let native = format!("{prefix}-{}-{seq}", b.node);
            out.extend(
                reqs.iter()
                    .filter(|r| eq_label(r.spec_id.as_deref(), &native)),
            );
        }
    }
    for r in maps
        .retired
        .iter()
        .filter(|r| r.old.trim().eq_ignore_ascii_case(key.trim()))
    {
        out.extend(reqs.iter().filter(|q| {
            eq_label(q.spec_id.as_deref(), &r.new)
                && q.id.to_string().eq_ignore_ascii_case(r.uuid.trim())
        }));
    }
    out.sort_by_key(|r| r.id);
    out.dedup_by(|a, b| a.id == b.id);
    out
}

/// Pure resolution of `key` against one snapshot's requirements and label
/// mappings: literal first, then the snapshot's node-label and retired-ID
/// mappings. Returns the requirement's labels (`key`, its native and friendly
/// IDs, its node-label forms and its retired IDs) — each kept only when the
/// snapshot resolves it to the SAME full UUID and to nothing else — or just
/// `key` when it names no requirement. `Err` when `key` itself is ambiguous.
// trace:BUG-2403 | ai:claude
pub(crate) fn resolve_labels(
    reqs: &[aida_core::Requirement],
    maps: &SnapshotMaps,
    key: &str,
) -> Result<Vec<String>, String> {
    if ledger::ChangeKey::parse(key).is_some() || ledger::project::spec_key(key).is_none() {
        return Ok(vec![key.to_string()]);
    }
    let literal = claimants(reqs, key);
    let found = if literal.is_empty() {
        mapped(reqs, maps, key)
    } else {
        literal
    };
    let req = match found.as_slice() {
        [] => return Ok(spec_labels(&[key])),
        [one] => *one,
        _ => return Err(format!("{key} names {} requirements", found.len())),
    };
    let mut ids: Vec<String> = vec![key.to_string()];
    ids.extend(req.spec_id.clone());
    ids.extend(req.agreed_id.clone());
    if let Some((prefix, node, seq)) = req.spec_id.as_deref().and_then(split_id) {
        for b in maps
            .bindings
            .iter()
            .filter(|b| b.node.eq_ignore_ascii_case(node))
        {
            ids.push(format!("{prefix}-{}-{seq}", b.label));
        }
    }
    for r in &maps.retired {
        if eq_label(req.spec_id.as_deref(), &r.new)
            && req.id.to_string().eq_ignore_ascii_case(r.uuid.trim())
        {
            ids.push(r.old.clone());
        }
    }
    // A label survives only when the snapshot resolves it to this one
    // requirement: a colliding or stale label bridges nothing.
    let proved = |label: &str| {
        let hits = claimants(reqs, label);
        let hits = if hits.is_empty() {
            mapped(reqs, maps, label)
        } else {
            hits
        };
        matches!(hits.as_slice(), [one] if one.id == req.id)
    };
    let ids: Vec<String> = ids.into_iter().filter(|l| l == key || proved(l)).collect();
    Ok(spec_labels(
        &ids.iter().map(String::as_str).collect::<Vec<_>>(),
    ))
}

/// The config-independent git command a snapshot query runs.
fn snapshot_command(store: &Path) -> std::process::Command {
    let mut cmd = aida_core::git_ops::host_git_command(store);
    cmd.args([
        "-c",
        "core.quotePath=false",
        "-c",
        "core.excludesFile=/dev/null",
    ])
    .env("GIT_NO_REPLACE_OBJECTS", "1")
    .env("GIT_CONFIG_NOSYSTEM", "1")
    .env_remove("GIT_LITERAL_PATHSPECS")
    .env_remove("GIT_NOGLOB_PATHSPECS")
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null());
    cmd
}

/// One bounded snapshot git query: killed past [`SNAPSHOT_GIT_TIMEOUT`],
/// refused past [`SNAPSHOT_GIT_MAX_OUTPUT`]; a non-zero exit refuses.
fn snapshot_git(store: &Path, args: &[&str]) -> Result<Vec<u8>, Unsafe> {
    use std::io::Read as _;
    let fail = |why: String| Unsafe(format!("store snapshot: git {}: {why}", args[0]));
    let mut child = snapshot_command(store)
        .args(args)
        .spawn()
        .map_err(|e| fail(e.to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| fail("no stdout".into()))?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        stdout
            .take(SNAPSHOT_GIT_MAX_OUTPUT as u64 + 1)
            .read_to_end(&mut buf)
            .map(|_| buf)
    });
    let deadline = std::time::Instant::now() + SNAPSHOT_GIT_TIMEOUT;
    let status = loop {
        match child.try_wait().map_err(|e| fail(e.to_string()))? {
            Some(status) => break status,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(fail("timed out".into()));
            }
            None => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    };
    let out = reader
        .join()
        .map_err(|_| fail("reader panicked".into()))?
        .map_err(|e| fail(e.to_string()))?;
    if out.len() > SNAPSHOT_GIT_MAX_OUTPUT {
        return Err(fail("output exceeds its bound".into()));
    }
    if !status.success() {
        return Err(fail(format!("exited {status}")));
    }
    Ok(out)
}

/// The snapshot's identity-relevant paths have no uncommitted change, staged
/// or not, and no untracked file (ignore rules applied by none).
fn snapshot_clean(store: &Path) -> Result<(), Unsafe> {
    let paths = [OBJECTS_REL, NODE_ALIASES_REL, RETIRED_IDS_REL];
    let mut status = vec![
        "status",
        "--porcelain=v1",
        "--untracked-files=all",
        "--ignore-submodules=none",
        "--no-renames",
        "--",
    ];
    status.extend(paths);
    let mut others = vec!["ls-files", "--others", "-z", "--"];
    others.extend(paths);
    if !snapshot_git(store, &status)?.is_empty() || !snapshot_git(store, &others)?.is_empty() {
        return Err(Unsafe(
            "the store's objects or label mappings have uncommitted changes".into(),
        ));
    }
    Ok(())
}

fn snapshot_head(store: &Path) -> Result<String, Unsafe> {
    let head = snapshot_git(store, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let head = String::from_utf8(head)
        .map_err(|_| Unsafe("store snapshot: a non-text HEAD".into()))?
        .trim()
        .to_string();
    if head.len() < 40 || !head.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Unsafe("store snapshot: an unreadable HEAD".into()));
    }
    Ok(head)
}

/// The committed body of `rel` at `head`, read by blob ID; `None` when the
/// commit holds no such file.
fn committed_text(store: &Path, head: &str, rel: &str) -> Result<Option<String>, Unsafe> {
    let listing = snapshot_git(store, &["ls-tree", "-z", head, "--", rel])?;
    let Some(entry) = listing.split(|b| *b == 0).find(|e| !e.is_empty()) else {
        return Ok(None);
    };
    let entry = String::from_utf8_lossy(entry);
    let mut meta = entry.split('\t').next().unwrap_or_default().split(' ');
    let (_mode, kind, oid) = (meta.next(), meta.next(), meta.next());
    let (Some("blob"), Some(oid)) = (kind, oid) else {
        return Err(Unsafe(format!(
            "store snapshot: {rel} is not a committed file"
        )));
    };
    let body = snapshot_git(store, &["cat-file", "blob", oid])?;
    String::from_utf8(body)
        .map(Some)
        .map_err(|_| Unsafe(format!("store snapshot: {rel} is not UTF-8")))
}

/// The labels of the ONE requirement `key` names, proved against one
/// committed store snapshot (see the module docs): the store's objects and
/// label mappings must be clean, the mappings are read from the pinned HEAD's
/// blobs, the requirements are loaded, and HEAD and cleanliness are checked
/// again afterwards — any move, dirt, read error or ambiguity is `Err`, and
/// the caller then bridges nothing. A project with no git-canonical store,
/// a change key, or a key naming no requirement is just `[key]`.
// trace:BUG-2403 | ai:claude
pub(crate) fn snapshot_labels(project_root: &Path, key: &str) -> Result<Vec<String>, Unsafe> {
    if ledger::ChangeKey::parse(key).is_some() {
        return Ok(vec![key.to_string()]);
    }
    let Some(store) = aida_core::store_locate::detect_distributed_store_from(project_root) else {
        return Ok(spec_labels(&[key]));
    };
    // The store must be its OWN repository (the `aida-store` worktree), never
    // a directory some enclosing project repository happens to track.
    let top = snapshot_git(&store, &["rev-parse", "--show-toplevel"])?;
    let top = std::path::PathBuf::from(String::from_utf8_lossy(&top).trim());
    let same_dir = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if !same_dir(&top, &store) {
        return Err(Unsafe(format!(
            "store snapshot: {} is not the top of its own git repository",
            store.display()
        )));
    }
    snapshot_clean(&store)?;
    let head = snapshot_head(&store)?;
    let maps = SnapshotMaps::parse(
        committed_text(&store, &head, NODE_ALIASES_REL)?.as_deref(),
        committed_text(&store, &head, RETIRED_IDS_REL)?.as_deref(),
    )
    .map_err(|e| Unsafe(format!("store snapshot: {e}")))?;
    let backend =
        aida_core::GitBackend::new(&store).map_err(|e| Unsafe(format!("store snapshot: {e}")))?;
    let loaded: aida_core::RequirementsStore = aida_core::DatabaseBackend::load(&backend)
        .map_err(|e| Unsafe(format!("store snapshot: {e}")))?;
    snapshot_clean(&store)?;
    if snapshot_head(&store)? != head {
        return Err(Unsafe(
            "store snapshot: HEAD moved while it was read".into(),
        ));
    }
    resolve_labels(&loaded.requirements, &maps, key)
        .map_err(|e| Unsafe(format!("store snapshot: {e}")))
}

/// One label's current record, read strictly.
struct Candidate {
    selected: Selected,
}

fn candidate(
    project_root: &Path,
    store_root: &Path,
    label: &str,
    events: Vec<GateEvent>,
) -> Result<Option<Candidate>, Unsafe> {
    let path = rv::verdict_path(project_root, label);
    let file = rv::checked_verdict_file(&path).map_err(Unsafe)?;
    let parsed = match &file {
        Some((body, _)) => Some(rv::parse_recorded_verdict(body).ok_or_else(|| {
            Unsafe(format!(
                "the verdict file {} does not parse",
                path.display()
            ))
        })?),
        None => None,
    };
    // Keep exact-source annotations from the checked snapshot even when raw
    // projection retains an independent review instead of the annotation.
    // trace:BUG-2525 | ai:codex
    let annotation = events
        .iter()
        .rev()
        .filter_map(|e| e.verdict_record())
        .find(|r| {
            r.get("terminal_wait_key").and_then(|v| v.as_str()) == Some(label)
                && r.contains_key("terminal_wait_source")
        })
        .cloned();
    let overlay = |mut selected: Selected| {
        if let Some(body) = annotation
            .as_ref()
            .and_then(|r| rv::terminal_wait_overlay_body(label, &selected.body, r))
        {
            if let Some(verdict) = rv::parse_recorded_verdict(&body) {
                selected.verdict = verdict;
                selected.body = body;
            }
        }
        selected
    };
    let newest: Option<GateEvent> = ledger::project::newest_of(events);
    if let Some(event) = newest.filter(|e| rv::ledger_supersedes(&path, parsed.as_ref(), e)) {
        let record = event
            .verdict_record()
            .ok_or_else(|| Unsafe("a ledger verdict event lost its record".into()))?;
        let body = serde_json::to_string(record).map_err(|e| Unsafe(e.to_string()))?;
        let verdict = rv::parse_ledger_record(record).ok_or_else(|| {
            Unsafe(format!(
                "ledger verdict {}#{} does not parse",
                event.change, event.seq
            ))
        })?;
        return Ok(Some(Candidate {
            selected: overlay(Selected {
                verdict,
                body,
                label: label.to_string(),
                order_time: rv::ledger_order_time(&event),
                source: VerdictSource::Ledger {
                    file: ledger::project::event_file(store_root, &event),
                    seq: event.seq,
                },
            }),
        }));
    }
    Ok(match (parsed, file) {
        (Some(verdict), Some((body, mtime))) => Some(Candidate {
            selected: overlay(Selected {
                order_time: verdict
                    .recorded_at
                    .as_deref()
                    .and_then(|s| DateTime::parse_from_rfc3339(s.trim()).ok())
                    .or_else(|| mtime.map(|m| DateTime::<chrono::Utc>::from(m).fixed_offset())),
                verdict,
                body,
                label: label.to_string(),
                source: VerdictSource::File(path),
            }),
        }),
        _ => None,
    })
}

/// Same act of review seen under two labels (e.g. one legacy file read
/// through both): equal verdict, head, recorder, time and findings.
fn same_record(a: &RecordedVerdict, b: &RecordedVerdict) -> bool {
    a.recorded_at.is_some()
        && a.kind == b.kind
        && a.reviewed_sha == b.reviewed_sha
        && a.recorded_by == b.recorded_by
        && a.recorded_at == b.recorded_at
        && a.findings == b.findings
}

fn newest_refusal(cands: &[Candidate]) -> Option<RecordedVerdict> {
    cands
        .iter()
        .filter(|c| c.selected.verdict.kind.blocks_done())
        .max_by_key(|c| c.selected.order_time)
        .map(|c| c.selected.verdict.clone())
}

/// Select the current verdict across `ids`, the labels of ONE requirement
/// (see the module docs for the identity contract). One label keeps the
/// established single-key reader exactly ([`rv::current_verdict`]); two or
/// more compare every label's strictly read current record.
// trace:BUG-2403 | ai:claude
pub(crate) fn select_verdict(project_root: &Path, ids: &[&str]) -> AliasSelection {
    Selector::new(project_root).select(ids)
}

/// Selections for many requirements in one operation (a list view): the
/// gate ledger is discovered once, on the first multi-label selection, and
/// every later selection reuses that discovery and the same aggregate
/// budget, so no ledger file is read twice per operation.
// trace:BUG-2403 | ai:claude
pub(crate) struct Selector<'a> {
    project_root: &'a Path,
    budget: Budget,
    discovery: Option<Result<(std::path::PathBuf, Option<checked::Discovery>), Unsafe>>,
}

impl<'a> Selector<'a> {
    pub(crate) fn new(project_root: &'a Path) -> Self {
        Self::with_budget(project_root, Budget::default())
    }

    pub(crate) fn with_budget(project_root: &'a Path, budget: Budget) -> Self {
        Self {
            project_root,
            budget,
            discovery: None,
        }
    }

    /// [`select_verdict`] for one requirement's labels.
    pub(crate) fn select(&mut self, ids: &[&str]) -> AliasSelection {
        // trace:STORY-2492 | ai:codex — remote refusals cannot lose to alias cache order.
        if let Some(mut remote) = super::review_publication::current(self.project_root, ids) {
            // Terminal annotations retire only this exact signed round.
            // trace:BUG-2525 | ai:codex
            for label in spec_labels(ids) {
                remote = rv::terminal_wait_overlay(self.project_root, &label, remote);
            }
            let verdict = rv::parse_recorded_verdict(&remote.body)
                .expect("publication projection is a verdict");
            return AliasSelection::Selected(Box::new(Selected {
                verdict,
                body: remote.body,
                label: ids.first().unwrap_or(&"").to_string(),
                source: remote.source,
                order_time: None,
            }));
        }
        let labels = spec_labels(ids);
        match labels.as_slice() {
            [] => AliasSelection::Absent,
            [label] => single(self.project_root, label),
            _ => self.across(&labels),
        }
    }

    fn across(&mut self, labels: &[String]) -> AliasSelection {
        let project_root = self.project_root;
        let budget = &mut self.budget;
        let discovery = self.discovery.get_or_insert_with(|| {
            let root = ledger::store_root(project_root)
                .map_err(|e| Unsafe(format!("the gate ledger cannot be resolved: {e}")))?;
            let found = checked::discover(&root, budget)?;
            Ok((root, found))
        });
        let (store_root, found) = match discovery {
            Ok(d) => d,
            Err(e) => {
                return AliasSelection::Unsafe {
                    reason: e.to_string(),
                    refusal: None,
                }
            }
        };
        let per_label = match found {
            Some(d) => d.events_for(labels, budget),
            None => Ok(labels.iter().map(|l| (l.clone(), Vec::new())).collect()),
        };
        match per_label {
            Ok(per_label) => choose(project_root, store_root, per_label),
            Err(e) => AliasSelection::Unsafe {
                reason: e.to_string(),
                refusal: None,
            },
        }
    }
}

fn single(project_root: &Path, label: &str) -> AliasSelection {
    let Some(current) = rv::current_verdict(project_root, label) else {
        return AliasSelection::Absent;
    };
    match rv::parse_recorded_verdict(&current.body) {
        Some(verdict) => AliasSelection::Selected(Box::new(Selected {
            verdict,
            body: current.body,
            label: label.to_string(),
            source: current.source,
            order_time: None,
        })),
        // An unparseable current file: the single-key readers treat it as
        // no verdict (and the gates that read it directly fail closed).
        None => AliasSelection::Absent,
    }
}

/// [`select_verdict`] over two or more labels with an explicit `budget`.
// trace:BUG-2403 | ai:claude
#[cfg(test)]
pub(crate) fn across(
    project_root: &Path,
    labels: &[String],
    budget: &mut Budget,
) -> AliasSelection {
    let mut selector = Selector::with_budget(project_root, std::mem::take(budget));
    let out = selector.across(labels);
    *budget = selector.budget;
    out
}

/// Choose among every label's current record (see [`select_verdict`]).
fn choose(
    project_root: &Path,
    store_root: &Path,
    per_label: Vec<(String, Vec<GateEvent>)>,
) -> AliasSelection {
    let mut cands: Vec<Candidate> = Vec::new();
    let mut refused: Option<String> = None;
    for (label, events) in per_label {
        match candidate(project_root, store_root, &label, events) {
            Ok(Some(c)) => cands.push(c),
            Ok(None) => {}
            Err(e) => {
                refused.get_or_insert_with(|| format!("{label}: {e}"));
            }
        }
    }
    if let Some(reason) = refused {
        return AliasSelection::Unsafe {
            reason,
            refusal: newest_refusal(&cands),
        };
    }
    if cands.len() <= 1 {
        return cands
            .pop()
            .map(|c| AliasSelection::Selected(Box::new(c.selected)))
            .unwrap_or(AliasSelection::Absent);
    }
    if let Some(c) = cands.iter().find(|c| c.selected.order_time.is_none()) {
        return AliasSelection::Ambiguous {
            reason: format!(
                "the {} record at {} has no order time to compare",
                c.selected.label,
                c.selected.source.display()
            ),
            refusal: newest_refusal(&cands),
        };
    }
    // Newest first; the caller's label order breaks no ties.
    let mut order: Vec<usize> = (0..cands.len()).collect();
    order.sort_by(|a, b| {
        cands[*b]
            .selected
            .order_time
            .cmp(&cands[*a].selected.order_time)
    });
    let top = &cands[order[0]].selected;
    let rival = order[1..]
        .iter()
        .map(|i| &cands[*i].selected)
        .find(|c| c.order_time == top.order_time && !same_record(&c.verdict, &top.verdict));
    if let Some(rival) = rival {
        return AliasSelection::Ambiguous {
            reason: format!(
                "{} ({}) and {} ({}) hold different records at the same time",
                top.label,
                top.source.display(),
                rival.label,
                rival.source.display()
            ),
            refusal: newest_refusal(&cands),
        };
    }
    let winner = order[0];
    AliasSelection::Selected(Box::new(cands.swap_remove(winner).selected))
}

#[cfg(test)]
#[path = "tests/bug_2403_alias_selection_tests.rs"]
mod bug_2403_alias_selection_tests;

#[cfg(test)]
#[path = "tests/bug_2430_rework_change_key_tests.rs"]
mod bug_2430_rework_change_key_tests;

#[cfg(test)]
#[path = "tests/bug_2500_waiting_tests.rs"]
mod bug_2500_waiting_tests;
