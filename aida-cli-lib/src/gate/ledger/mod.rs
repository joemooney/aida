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

// trace:BUG-2531 | ai:claude — write-ahead spec-to-change bindings.
pub(crate) mod bindings;
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

#[cfg(test)]
thread_local! {
    /// BUG-2564 test seam: fail the next deferred-finding delivery on this
    /// thread (an approval's before anything is written, a mixed refusal's
    /// after the refusal is on record).
    // trace:BUG-2564 | ai:claude
    pub(crate) static FAIL_NEXT_FINDING_DELIVERY: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
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
    crate::gate::ledger::record_verdict_with(path, record, spec, false, None)
}

/// [`record_verdict_at`] for one act of review (`aida review record`): one
/// act is one verdict event. Under the destination's locks, a record that
/// repeats the act standing there for the same spec
/// ([`crate::gate::review_verdict::repeats_recorded_act`]) is not appended
/// (`Unchanged`), a record the ledger already holds that no longer stands is
/// refused, and a history that cannot be read whole is refused. A `receipt`
/// is settled with the append, under the same locks ([`store::Receipt`]):
/// none is persisted for a repeat, a refusal or a failed follow-up filing.
// trace:BUG-2662 | ai:claude
pub(crate) fn record_review_act_at(
    path: &Path,
    record: &crate::gate::review_verdict::JsonObj,
    spec: Option<&GateSpec>,
    receipt: Option<&dyn store::Receipt>,
) -> std::io::Result<Delivered> {
    crate::gate::ledger::record_verdict_with(path, record, spec, true, receipt)
}

fn record_verdict_with(
    path: &Path,
    record: &crate::gate::review_verdict::JsonObj,
    spec: Option<&GateSpec>,
    once: bool,
    receipt: Option<&dyn store::Receipt>,
) -> std::io::Result<Delivered> {
    // How follow-up findings and the verdict are ordered depends on the path:
    //
    // - A mixed refusal (blocking findings beside deferred ones) appends the
    //   refusal first, then files the deferred follow-ups. A failed filing
    //   leaves the refusal on record; repeating the record retries the filing
    //   (the sink deduplicates by source and hash) against the refusal that
    //   stands, without a second event when `once`.
    // - A direct approval with follow-up findings files them first and is
    //   appended only once they are durable. It writes no delivery-pending
    //   barrier, so a failed filing records nothing (BUG-2662).
    // - Launcher collection ([`record_collected_verdict`]) is the one path
    //   that persists a delivery-pending barrier before the sweep and keeps
    //   it after a failure (STORY-2698).
    // trace:STORY-2698 | ai:codex
    // trace:BUG-2662 | ai:claude
    let mut normalized = record.clone();
    if let Some(word) = crate::gate::review_verdict::disposition_verdict(record).canonical() {
        normalized.insert("verdict".into(), word.into());
    }
    let record = &normalized;
    let parsed = crate::gate::review_verdict::parse_recorded_verdict(
        &serde_json::Value::Object(record.clone()).to_string(),
    );
    // trace:TASK-3814 | ai:codex
    // A mixed review still files its deferred observations. Persist the refusal
    // first; a failed delivery must never expose an approval or lose its retry input.
    let (root, key) = crate::gate::review_verdict::canonical_verdict_key(path)
        .ok_or_else(|| std::io::Error::other("review has no canonical verdict key"))?;
    if crate::gate::kernel_deps::review_blocking::enabled(&root)
        .map_err(|e| std::io::Error::other(format!("{e:#}")))?
    {
        if let Some(verdict) = parsed.as_ref().filter(|v| !v.kind.approves()) {
            let metadata = record
                .get("finding_dispositions")
                .and_then(|v| v.as_array());
            let deferred: Vec<String> = verdict
                .findings
                .iter()
                .enumerate()
                .filter(|(i, _)| {
                    metadata
                        .and_then(|m| m.get(*i))
                        .is_some_and(crate::gate::review_verdict::finding_is_deferred)
                })
                .map(|(_, text)| text.clone())
                .collect();
            if !deferred.is_empty() {
                // trace:BUG-2662 | ai:claude — a retry finds its refusal
                // standing and files against it: one verdict event in all.
                let delivered = Destination::resolve(&root, &key, record, spec)?
                    .append(record, None, once, receipt)?;
                // A one-shot, thread-local test fault: the filing fails after
                // the refusal is on record, as in production.
                #[cfg(test)]
                if FAIL_NEXT_FINDING_DELIVERY.with(|f| f.replace(false)) {
                    return Err(std::io::Error::other(
                        "deferred findings pending: injected test failure; retry review record",
                    ));
                }
                let store_path = crate::detect_distributed_store_from(&root).ok_or_else(|| {
                    std::io::Error::other("deferred findings pending: no canonical store")
                })?;
                let mut followups = verdict.clone();
                followups.kind = crate::gate::review_verdict::VerdictKind::Approved;
                followups.findings = deferred;
                followups.inherited_findings = false;
                crate::findings_gate::emit_verdict_findings(
                    &root,
                    &store_path,
                    spec.map(|s| s.label.as_str()).unwrap_or(&key),
                    "",
                    None,
                    &followups,
                )
                .map_err(|e| {
                    std::io::Error::other(format!(
                        "deferred findings pending: {e:#}; retry review record"
                    ))
                })?;
                return Ok(delivered);
            }
        }
    }
    if let Some(mut verdict) = parsed.filter(|v| v.kind.approves() && !v.findings.is_empty()) {
        // trace:BUG-2662 | ai:claude — one approval is one verdict event. The
        // follow-up findings are filed first (the sink deduplicates by source
        // and hash, so a retry is harmless) and the approval is appended only
        // once they are durable. A failed filing records nothing: no
        // request-changes barrier stands in for an approval that was refused.
        verdict.inherited_findings = false;
        let source = spec.map(|s| s.label.as_str()).unwrap_or(&key);
        let destination = Destination::resolve(&root, &key, record, spec)?;
        let mut delivered = record.clone();
        delivered.insert("sweep_pending".into(), false.into());
        delivered.insert("verdict".into(), "approved".into());
        // One strict read: what stood when filing began, and whether this
        // approval already stands (its findings were then filed before it was
        // appended). The locked append decides either way.
        let observed = destination.observe()?;
        let standing = observed.standing();
        let stands = once && observed.repeats(destination.event_spec(), &delivered, receipt);
        if !stands {
            // trace:BUG-2564 | ai:claude — a one-shot, thread-local test fault:
            // the filing fails, as in production.
            #[cfg(test)]
            if FAIL_NEXT_FINDING_DELIVERY.with(|f| f.replace(false)) {
                return Err(std::io::Error::other(
                    "deferred findings were not filed: injected test failure; nothing was recorded; retry review record after recovery",
                ));
            }
            let store_path = crate::detect_distributed_store_from(&root).ok_or_else(|| {
                std::io::Error::other(
                    "deferred findings were not filed: no canonical store; nothing was recorded; retry review record after recovery",
                )
            })?;
            crate::findings_gate::emit_verdict_findings(
                &root,
                &store_path,
                source,
                "",
                None,
                &verdict,
            )
            .map_err(|e| {
                std::io::Error::other(format!(
                    "deferred findings were not filed: {e:#}; nothing was recorded; retry review record after recovery"
                ))
            })?;
        }
        return destination.append(&delivered, Some(&standing), once, receipt);
    }
    Destination::resolve(&root, &key, record, spec)?.append(record, None, once, receipt)
}

/// Where a verdict record for a canonical key lands: on a change's ledger
/// (indexed by a spec when one is named), or on the unbound ledger of a spec
/// no change has been recorded against yet.
// trace:STORY-2698 | ai:codex
// trace:BUG-2662 | ai:claude
enum Destination {
    Change {
        store: std::path::PathBuf,
        change: ChangeKey,
        spec: Option<GateSpec>,
        /// A change key copies the spec's unbound records onto the change.
        bind: bool,
    },
    Unbound {
        store: std::path::PathBuf,
        spec: GateSpec,
    },
}

impl Destination {
    /// Reads only. A contained caller is refused.
    fn resolve(
        root: &Path,
        key: &str,
        record: &crate::gate::review_verdict::JsonObj,
        spec: Option<&GateSpec>,
    ) -> std::io::Result<Self> {
        if crate::session::process_contained() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "a contained seat cannot write the gate ledger; the launcher that started it collects its verdict",
            ));
        }
        let store = store_root(root)?;
        if let Some(change) = ChangeKey::parse(key) {
            return Ok(Self::Change {
                store,
                change,
                spec: spec.cloned(),
                bind: true,
            });
        }
        let spec = GateSpec::resolve(root, key);
        // trace:BUG-2517 | ai:codex
        let changes = if record.get("change_binding").and_then(|v| v.as_str()) == Some("unbound") {
            Vec::new()
        } else {
            project::changes_for_spec(&store, &spec.label)
        };
        Ok(match changes.last() {
            Some(change) => Self::Change {
                store,
                change: *change,
                spec: Some(spec),
                bind: false,
            },
            None => Self::Unbound { store, spec },
        })
    }

    /// The spec label a verdict event here carries ([`event::verdict_event`]).
    fn event_spec(&self) -> Option<&str> {
        match self {
            Self::Change { spec, .. } => spec
                .as_ref()
                .map(|s| s.label.as_str())
                .filter(|l| !l.is_empty() && ChangeKey::parse(l).is_none()),
            Self::Unbound { spec, .. } => Some(spec.label.as_str()),
        }
    }

    /// The review history a verdict here joins, read strictly.
    // trace:BUG-2662 | ai:claude
    fn observe(&self) -> std::io::Result<store::Observed> {
        match self {
            Self::Change { store, change, .. } => {
                store::observe(store, Some(*change), self.event_spec())
            }
            Self::Unbound { store, .. } => store::observe(store, None, self.event_spec()),
        }
    }

    /// Append `record`. With `standing`, only while the history holds no
    /// verdict record that was not standing when it was read; with `once`, as
    /// one act of review ([`store::ReviewRule::once`]); with `receipt`, naming
    /// the receipt settled there. Each is decided under the destination's
    /// locks ([`store::append_review`]).
    fn append(
        &self,
        record: &crate::gate::review_verdict::JsonObj,
        standing: Option<&store::Standing>,
        once: bool,
        receipt: Option<&dyn store::Receipt>,
    ) -> std::io::Result<Delivered> {
        match self {
            Self::Change {
                store,
                change,
                spec,
                bind,
            } => {
                let event = event::verdict_event(store, *change, record, spec.as_ref());
                let delivered = if standing.is_some() || once || receipt.is_some() {
                    let rule = store::ReviewRule {
                        standing,
                        once,
                        bind: spec.as_ref().filter(|_| *bind),
                        receipt,
                    };
                    store::append_review(store, Some(*change), event, &rule)?
                } else {
                    if let Some(spec) = spec.as_ref().filter(|_| *bind) {
                        store::bind_unbound(store, spec, *change)?;
                    }
                    store::append_one(store, event)?
                };
                // A repeated act wrote nothing and publishes nothing.
                if once && delivered == Delivered::Unchanged {
                    return Ok(delivered);
                }
                // trace:STORY-2492 | ai:codex
                // The verdict is durable: publication failure is a reported pending state.
                let labels = spec
                    .as_ref()
                    .map(|s| vec![s.label.clone()])
                    .unwrap_or_default();
                if record.get("sweep_pending").and_then(|v| v.as_bool()) != Some(true) {
                    // The record as appended: it names the receipt settled
                    // under the locks. trace:BUG-2662 | ai:claude
                    let mut appended = record.clone();
                    if let Some(attestation) = receipt.and_then(|r| r.issued()) {
                        appended.insert(
                            crate::gate::review_verdict::ATTESTATION_KEY.to_string(),
                            attestation,
                        );
                    }
                    super::review_publication::publish_reported(store, *change, &appended, &labels);
                }
                Ok(delivered)
            }
            Self::Unbound { store, spec } => {
                if standing.is_some() || once || receipt.is_some() {
                    let event = event::verdict_event_unbound(store, spec, record);
                    let rule = store::ReviewRule {
                        standing,
                        once,
                        bind: None,
                        receipt,
                    };
                    store::append_review(store, None, event, &rule)
                } else {
                    store::append_unbound(store, spec, record)
                }
            }
        }
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
    let tier_policy = crate::gate::kernel_deps::review_blocking::enabled(launcher_root)
        .map_err(|e| std::io::Error::other(format!("{e:#}")))?;
    let needs_delivery = |body: &crate::gate::review_verdict::JsonObj| {
        super::review_verdict::parse_recorded_verdict(
            &serde_json::Value::Object(body.clone()).to_string(),
        )
        .and_then(|mut v| {
            if tier_policy && !v.kind.approves() {
                let metadata = body.get("finding_dispositions").and_then(|v| v.as_array());
                v.findings = v
                    .findings
                    .into_iter()
                    .enumerate()
                    .filter(|(i, _)| {
                        metadata
                            .and_then(|m| m.get(*i))
                            .is_some_and(super::review_verdict::finding_is_deferred)
                    })
                    .map(|(_, text)| text)
                    .collect();
                v.kind = super::review_verdict::VerdictKind::Approved;
            }
            v.inherited_findings = false;
            (v.kind.approves() && !v.findings.is_empty()).then_some(v)
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
        let overall_source = if tier_policy {
            spec_records
                .first()
                .map(|(s, _)| s.label.clone())
                .unwrap_or_else(|| change.as_key())
        } else {
            change.as_key()
        };
        let sources = std::iter::once((overall_source, &record)).chain(
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
