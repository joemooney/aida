//! M1: one append-only gate ledger per change (PR/MR), in the primary
//! checkout's shared state — `<main>/.aida/gate-ledger/PR-<N>.jsonl`.
//!
//! The change is the only primary key. A spec ID is an index to changes
//! (`by-spec/<SPEC>.json`, a derived cache), never a second place state lives.
//! Every event names the head it was recorded at and the change's patch
//! identity, so a gate can tell "the same change on a moved base" from "new
//! code".
//!
//! One writer kind: uncontained AIDA on the primary side ([`store::append`]).
//! A contained seat cannot write `<main>/.aida/` (BUG-1929 / BUG-2218) and has
//! no route to the ledger of its own: a mailbox submission cannot prove which
//! seat sent it, since grant files are readable from every seat. Its verdict
//! stays in its seat clone until the uncontained launcher that started it
//! collects it ([`crate::gate::review_verdict::collect_standalone_verdict`]),
//! checking the PR binding and that launch's own child grant, and appends it
//! here. No verdict or hold state persists in a seat clone after collection.
//!
//! Slice 1 carried review verdicts; slice 2 makes the ledger their only
//! writer and adds merge holds ([`hold`]). The legacy `.aida/review-verdicts/`
//! and `.aida/merge-holds/` files are read through ([`legacy`]) and preserved
//! verbatim in a `legacy-imported` event on a change's first ledger write;
//! nothing here writes or deletes them.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// mandate-gate

// trace:BUG-2403 | ai:claude
pub(crate) mod checked;
pub(crate) mod event;
pub(crate) mod hold;
pub(crate) mod legacy;
// trace:STORY-2324 | ai:claude — the one-time spec_key rewrite.
pub(crate) mod migrate;
pub(crate) mod project;
pub(crate) mod store;

use std::path::{Path, PathBuf};

pub(crate) use crate::gate::canon::GateSpec;
pub(crate) use event::{EventBody, GateEvent};

/// The forge's name for a change: a GitHub pull request or a GitLab merge
/// request.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ChangeForge {
    Pr,
    Mr,
}

/// The ledger's primary key: forge kind plus number, spelled `PR-<N>` /
/// `MR-<N>`.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ChangeKey {
    pub forge: ChangeForge,
    pub number: u64,
}

impl ChangeKey {
    pub(crate) fn pr(number: u64) -> Self {
        Self {
            forge: ChangeForge::Pr,
            number,
        }
    }

    /// `PR-7` / `pr-7` / `MR-7`; anything else (a spec ID) is not a change.
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim().to_ascii_uppercase();
        let (forge, digits) = if let Some(d) = raw.strip_prefix("PR-") {
            (ChangeForge::Pr, d)
        } else if let Some(d) = raw.strip_prefix("MR-") {
            (ChangeForge::Mr, d)
        } else {
            return None;
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some(Self {
            forge,
            number: digits.parse().ok()?,
        })
    }

    pub(crate) fn as_key(&self) -> String {
        let prefix = match self.forge {
            ChangeForge::Pr => "PR",
            ChangeForge::Mr => "MR",
        };
        format!("{prefix}-{}", self.number)
    }
}

/// `<root>/.aida/gate-ledger`.
pub(crate) fn ledger_dir(root: &Path) -> PathBuf {
    root.join(".aida").join("gate-ledger")
}

/// The checkout whose ledger `start` shares: the main checkout of a linked
/// worktree (read from git's files, no subprocess) or of a verified seat
/// clone. An unverified seat clone is refused rather than given a ledger of
/// its own: a worktree-local gate store is exactly what AC5 removes.
///
/// Memoised per process like `hold_store_root`: readers call this inside
/// per-spec loops, and resolving a seat clone lists leases. A refusal is
/// never memoised, so a clone resolves afresh once its lease verifies.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn store_root(start: &Path) -> std::io::Result<PathBuf> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static MEMO: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    if let Some(hit) = memo.lock().ok().and_then(|m| m.get(start).cloned()) {
        return Ok(hit);
    }
    let resolved = match crate::seat_clone::main_for(start) {
        crate::seat_clone::MainFor::Unverified(clone) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "{} is a seat clone that does not verify against its main checkout; refusing to resolve its gate ledger",
                    clone.display()
                ),
            ))
        }
        crate::seat_clone::MainFor::Verified(main) => main,
        // A root with no `.git` stays unmemoised, as in `hold_store_root`.
        crate::seat_clone::MainFor::NotAClone
            if std::fs::symlink_metadata(start.join(".git")).is_err() =>
        {
            return Ok(start.to_path_buf())
        }
        crate::seat_clone::MainFor::NotAClone => {
            crate::gate::merge_hold_core::hold_store_root(start)
        }
    };
    if let Ok(mut m) = memo.lock() {
        m.insert(start.to_path_buf(), resolved.clone());
    }
    Ok(resolved)
}

/// Whether this process runs in a contained seat, which never writes the
/// gate ledger (its launcher collects what it records). The gate's verdict
/// writer asks here, beside the rule it enforces.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn contained() -> bool {
    crate::session::process_contained()
}

/// What recording an event did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Delivered {
    /// Appended to the ledger.
    Appended,
    /// The ledger already held this record.
    Unchanged,
}

/// Record one event from `start` (any checkout sharing the ledger). A
/// contained caller is refused: only uncontained primary-side code writes
/// the ledger.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn record_event(start: &Path, event: GateEvent) -> std::io::Result<Delivered> {
    if crate::session::process_contained() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "a contained seat cannot write the gate ledger; the launcher that started it collects its verdict",
        ));
    }
    store::append_one(&store_root(start)?, event)
}

/// The single verdict writer (M1 slice 2): append `record` to the gate
/// ledger for the key its canonical path names
/// (`<root>/.aida/review-verdicts/<KEY>.json`). A change key lands on that
/// change, indexed by `spec` when given. A spec key lands on the spec's most
/// recent indexed change; a spec no change has been recorded against yet
/// lands on its unbound entry ([`store::append_unbound`]), read with the
/// spec's change-bound records. No legacy file is written. A spec key read
/// from the path is resolved here, the one place it enters (STORY-2324 E2).
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// trace:STORY-2324 | ai:claude
pub(crate) fn record_verdict_at(
    path: &Path,
    record: &crate::gate::review_verdict::JsonObj,
    spec: Option<&GateSpec>,
) -> std::io::Result<Delivered> {
    // trace:STORY-2698 | ai:codex
    // Persist the complete retry payload before touching the sweep. A crash or
    // failed store transaction leaves a delivery-pending barrier (not a review
    // refusal); repeating the record retries
    // delivery using the sink's locked source/hash deduplication.
    let mut normalized = record.clone();
    if let Some(word) = crate::gate::review_verdict::disposition_verdict(record).canonical() {
        normalized.insert("verdict".into(), word.into());
    }
    let record = &normalized;
    let parsed = crate::gate::review_verdict::parse_recorded_verdict(
        &serde_json::Value::Object(record.clone()).to_string(),
    );
    if let Some(mut verdict) = parsed.filter(|v| v.kind.approves() && !v.findings.is_empty()) {
        // Inherited deferred findings still need durable delivery if an earlier
        // approval attempt failed. Hash deduplication makes retries harmless.
        verdict.inherited_findings = false;
        let (root, key) = crate::gate::review_verdict::canonical_verdict_key(path)
            .ok_or_else(|| std::io::Error::other("approval has no canonical verdict key"))?;
        let source = spec.map(|s| s.label.as_str()).unwrap_or(&key);
        let mut pending = record.clone();
        // Explicit delivery state: readiness blocks, refusal history skips it.
        pending.insert("sweep_pending".into(), true.into());
        pending.insert("verdict".into(), "request-changes".into());
        record_verdict_at_inner(path, &pending, spec, None)?;
        let store_path = crate::detect_distributed_store_from(&root).ok_or_else(|| {
            std::io::Error::other(
                "deferred findings pending: no canonical store; retry review record after recovery",
            )
        })?;
        crate::findings_gate::emit_verdict_findings(&root, &store_path, source, "", None, &verdict)
            .map_err(|e| {
                std::io::Error::other(format!(
                    "deferred findings pending: {e:#}; retry review record after recovery"
                ))
            })?;
        let mut delivered = record.clone();
        delivered.insert("sweep_pending".into(), false.into());
        delivered.insert("verdict".into(), "approved".into());
        return record_verdict_at_inner(path, &delivered, spec, Some(&pending));
    }
    record_verdict_at_inner(path, record, spec, None)
}

// trace:STORY-2698 | ai:codex
fn append_review_delivery(
    root: &Path,
    change: ChangeKey,
    record: &crate::gate::review_verdict::JsonObj,
    spec: Option<&GateSpec>,
    pending: Option<&crate::gate::review_verdict::JsonObj>,
) -> std::io::Result<Delivered> {
    let event = event::verdict_event(root, change, record, spec);
    if let Some(pending) = pending {
        let pending = event::verdict_event(root, change, pending, spec);
        let appended = store::append_delivered(root, change, vec![event], &[pending])?;
        Ok(if appended.is_empty() {
            Delivered::Unchanged
        } else {
            Delivered::Appended
        })
    } else {
        store::append_one(root, event)
    }
}

// trace:STORY-2698 | ai:codex
fn record_verdict_at_inner(
    path: &Path,
    record: &crate::gate::review_verdict::JsonObj,
    spec: Option<&GateSpec>,
    pending: Option<&crate::gate::review_verdict::JsonObj>,
) -> std::io::Result<Delivered> {
    let (root, key) =
        crate::gate::review_verdict::canonical_verdict_key(path).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} is not a verdict key path", path.display()),
            )
        })?;
    if crate::session::process_contained() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "a contained seat cannot write the gate ledger; the launcher that started it collects its verdict",
        ));
    }
    let store = store_root(&root)?;
    if let Some(change) = ChangeKey::parse(&key) {
        if let Some(spec) = spec {
            store::bind_unbound(&store, spec, change)?;
        }
        let delivered = append_review_delivery(&store, change, record, spec, pending)?;
        // trace:STORY-2492 | ai:codex
        // The verdict is durable: publication failure is a reported pending state.
        let labels = spec.map(|s| vec![s.label.clone()]).unwrap_or_default();
        if record.get("sweep_pending").and_then(|v| v.as_bool()) != Some(true) {
            super::review_publication::publish_reported(&store, change, record, &labels);
        }
        return Ok(delivered);
    }
    let spec = GateSpec::resolve(&root, &key);
    // trace:BUG-2517 | ai:codex
    let changes = if record.get("change_binding").and_then(|v| v.as_str()) == Some("unbound") {
        Vec::new()
    } else {
        project::changes_for_spec(&store, &spec.label)
    };
    match changes.last() {
        Some(change) => {
            let result = append_review_delivery(&store, *change, record, Some(&spec), pending)?;
            if record.get("sweep_pending").and_then(|v| v.as_bool()) != Some(true) {
                super::review_publication::publish_reported(
                    &store,
                    *change,
                    record,
                    &[spec.label.clone()],
                );
            }
            Ok(result)
        }
        None => match pending {
            Some(pending) => store::append_unbound_expected(&store, &spec, record, Some(pending)),
            None => store::append_unbound(&store, &spec, record),
        },
    }
}

/// A verdict file written outside the record path (the review skill's
/// heredoc) at a canonical verdict path: append it to the ledger as it
/// stands ([`record_verdict_at`]). The file is the skill's hand-off and is
/// left as written. A contained caller records nothing: its launcher collects.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn adopt_verdict_file(
    path: &Path,
    record: &crate::gate::review_verdict::JsonObj,
) -> std::io::Result<()> {
    if crate::session::process_contained()
        || crate::gate::review_verdict::canonical_verdict_key(path).is_none()
    {
        return Ok(());
    }
    record_verdict_at(path, record, None).map(|_| ())
}

/// A collected standalone verdict: the reviewer's record for `change`, then
/// one record per spec the PR credits, appended from the launcher's side in
/// one locked write. Returns how many events were new.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// trace:STORY-2324 | ai:claude
pub(crate) fn record_collected_verdict(
    launcher_root: &Path,
    change: ChangeKey,
    record: &crate::gate::review_verdict::JsonObj,
    spec_records: &[(GateSpec, crate::gate::review_verdict::JsonObj)],
) -> std::io::Result<usize> {
    if crate::session::process_contained() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "a contained seat cannot collect a verdict onto the gate ledger",
        ));
    }
    let root = store_root(launcher_root)?;
    // trace:STORY-2698 | ai:codex
    let normalize = |body: &crate::gate::review_verdict::JsonObj| {
        let mut body = body.clone();
        if let Some(word) = super::review_verdict::disposition_verdict(&body).canonical() {
            body.insert("verdict".into(), word.into());
        }
        body
    };
    let record = normalize(record);
    let spec_records: Vec<_> = spec_records
        .iter()
        .map(|(spec, body)| (spec.clone(), normalize(body)))
        .collect();
    let needs_delivery = |body: &crate::gate::review_verdict::JsonObj| {
        super::review_verdict::parse_recorded_verdict(
            &serde_json::Value::Object(body.clone()).to_string(),
        )
        .filter(|v| v.kind.approves() && !v.findings.is_empty())
        .map(|mut v| {
            v.inherited_findings = false;
            v
        })
    };
    let mut pending_events = None;
    if needs_delivery(&record).is_some()
        || spec_records
            .iter()
            .any(|(_, b)| needs_delivery(b).is_some())
    {
        let pending = |body: &crate::gate::review_verdict::JsonObj| {
            let mut body = body.clone();
            // Explicit delivery state, including the same handoff origin.
            body.insert("sweep_pending".into(), true.into());
            body.insert("verdict".into(), "request-changes".into());
            body
        };
        let mut events = vec![event::verdict_event(&root, change, &pending(&record), None)];
        for (spec, body) in &spec_records {
            events.push(event::verdict_event(
                &root,
                change,
                &pending(body),
                Some(spec),
            ));
        }
        store::append(&root, change, events.clone())?;
        pending_events = Some(events);
        let store_path = crate::detect_distributed_store_from(launcher_root).ok_or_else(|| {
            std::io::Error::other(
                "deferred findings pending: canonical store unavailable; retry collection",
            )
        })?;
        // Change-keyed evidence is retained too: source-specific findings can
        // differ from the overall PR record.
        let sources = std::iter::once((change.as_key(), &record)).chain(
            spec_records
                .iter()
                .map(|(spec, body)| (spec.label.clone(), body)),
        );
        for (source, body) in sources {
            if let Some(verdict) = needs_delivery(body) {
                crate::findings_gate::emit_verdict_findings(
                    launcher_root,
                    &store_path,
                    &source,
                    "",
                    None,
                    &verdict,
                )
                .map_err(|e| {
                    std::io::Error::other(format!(
                        "deferred findings pending: {e:#}; retry collection"
                    ))
                })?;
            }
        }
    }
    let record = &record;
    let spec_records = &spec_records;
    for (spec, _) in spec_records {
        store::bind_unbound(&root, spec, change)?;
    }
    let mut events = vec![event::verdict_event(&root, change, record, None)];
    for (spec, body) in spec_records {
        events.push(event::verdict_event(&root, change, body, Some(spec)));
    }
    let appended = match pending_events {
        Some(pending) => store::append_delivered(&root, change, events, &pending)?,
        None => store::append(&root, change, events)?,
    };
    // trace:STORY-2492 | ai:codex — collector publishes only after its local checks;
    // the appended verdict stands even when the publication stays pending.
    super::review_publication::publish_reported(
        &root,
        change,
        record,
        &spec_records
            .iter()
            .map(|(spec, _)| spec.label.clone())
            .collect::<Vec<_>>(),
    );

    Ok(appended
        .iter()
        .filter(|e| e.verdict_record().is_some())
        .count())
}

/// AC2: a change was opened for `specs`: bind each spec's unbound
/// (pre-change) verdicts to it ([`store::bind_unbound`]), so the change is
/// the one key that answers for them. Best effort, from `start` (any
/// checkout sharing the ledger); a contained seat binds nothing.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// trace:STORY-2324 | ai:claude
pub(crate) fn bind_specs(start: &Path, change: ChangeKey, specs: &[GateSpec]) {
    if crate::session::process_contained() {
        return;
    }
    let Ok(root) = store_root(start) else {
        return;
    };
    for spec in specs {
        if let Err(e) = store::bind_unbound(&root, spec, change) {
            eprintln!(
                "warning: could not bind {}'s recorded verdicts to {}: {e}",
                spec.label,
                change.as_key()
            );
        }
    }
}

/// AC2's open-change hook, the one way production code opens a change: open
/// `req` through `forge`, then bind `specs`' pre-change verdicts to the
/// opened change ([`bind_specs`], from `start`). A forge that opens no
/// numbered change (`id == 0`) binds nothing. Ship, the orchestrator, the
/// drain cluster, the train and queue recover all open through here, so a
/// spec verdict recorded before the PR reads under the PR whichever path
/// opened it.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn open_change_bound(
    forge: &dyn crate::forge::Forge,
    start: &Path,
    req: crate::forge::OpenChange,
    specs: &[String],
) -> anyhow::Result<crate::forge::ChangeRef> {
    open_change_bound_with_riders(forge, start, req, specs, &[])
}

// trace:BUG-2451 | ai:codex — train riders remain open until the train lands.
pub(crate) fn open_change_bound_with_riders(
    forge: &dyn crate::forge::Forge,
    start: &Path,
    req: crate::forge::OpenChange,
    specs: &[String],
    riders: &[u64],
) -> anyhow::Result<crate::forge::ChangeRef> {
    // trace:BUG-2451 | ai:codex — all AIDA publication paths use this boundary.
    let branch = req.branch.clone();
    let mut existing = None;
    for spec in specs {
        let changes: Vec<_> = forge
            .open_changes_for_spec(spec)?
            .into_iter()
            .filter(|change| change.branch == req.branch || !riders.contains(&change.id))
            .collect();
        if changes.len() > 1 {
            anyhow::bail!(
                "{spec} has multiple open changes: {}; resolve duplicates before publication",
                changes
                    .iter()
                    .map(|c| format!("#{} on `{}`", c.id, c.branch))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        for change in changes {
            if change.branch != req.branch {
                anyhow::bail!("{spec} already has open change #{} on `{}`; refusing a second change from `{}`. Continue the existing branch.", change.id, change.branch, req.branch);
            }
            if existing
                .as_ref()
                .is_some_and(|prior: &crate::forge::ChangeRef| prior.id != change.id)
            {
                anyhow::bail!(
                    "credited specs belong to different open changes; refusing publication"
                );
            }
            existing = Some(change);
        }
    }
    let change = match existing {
        Some(change) => change,
        None => forge.open_change(req)?,
    };
    if change.id != 0 {
        bind_specs(
            start,
            ChangeKey::pr(change.id),
            &GateSpec::resolve_all(start, specs),
        );
        // trace:STORY-2492 | ai:claude — author sessions bind to the opened
        // branch's pushed tip (not `start`'s HEAD); a failure is reported,
        // never a failed open, because the PR already exists.
        super::review_publication::publish_change_authorship(
            start,
            ChangeKey::pr(change.id),
            &branch,
            specs,
        );
    }
    Ok(change)
}

/// AC2's adopt hook: a change opened outside aida (`gh pr create`, or a
/// raced drive) and adopted by number binds `specs`' pre-change verdicts
/// exactly as [`open_change_bound`] would have. Binding is idempotent, so
/// adopting a change aida already bound adds nothing.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn adopt_change_bound(start: &Path, id: u64, branch: &str, specs: &[String]) -> u64 {
    if id != 0 {
        bind_specs(
            start,
            ChangeKey::pr(id),
            &GateSpec::resolve_all(start, specs),
        );
        // trace:STORY-2492 | ai:claude — a push to an adopted change is a new head.
        super::review_publication::publish_change_authorship(
            start,
            ChangeKey::pr(id),
            branch,
            specs,
        );
    }
    id
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
