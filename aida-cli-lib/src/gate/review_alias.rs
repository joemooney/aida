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
//! ONE requirement object (or [`labels_for`] resolved one through the store
//! with ambiguity refused). No prefix, substring, UUID fragment, node or
//! sequence comparison links labels; a label that does not resolve stays
//! literal. Selecting a record grants nothing: every authority consumer
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

/// The labels of the one requirement `key` resolves to in `store` — `key`
/// itself, the requirement's native `spec_id` and friendly `agreed_id` — or
/// just `key` when it does not resolve, resolves ambiguously, or is a change
/// key. Resolution is literal-first and refuses ambiguity
/// ([`aida_core::RequirementsStore::get_requirement_unambiguous`]):
/// a stale or colliding label never bridges two requirements.
// trace:BUG-2403 | ai:claude
pub(crate) fn labels_for(store: &aida_core::RequirementsStore, key: &str) -> Vec<String> {
    let mut ids = vec![key.to_string()];
    if ledger::ChangeKey::parse(key).is_none() {
        if let Ok(Some(req)) = store.get_requirement_unambiguous(key) {
            ids.extend(req.spec_id.clone());
            ids.extend(req.agreed_id.clone());
        }
    }
    spec_labels(&ids.iter().map(String::as_str).collect::<Vec<_>>())
}

/// One label's current record, read strictly.
struct Candidate {
    selected: Selected,
}

fn file_order_time(path: &Path, v: &RecordedVerdict) -> Option<DateTime<FixedOffset>> {
    v.recorded_at
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s.trim()).ok())
        .or_else(|| {
            rv::verdict_file_modified(path).map(|m| DateTime::<chrono::Utc>::from(m).fixed_offset())
        })
}

fn candidate(
    project_root: &Path,
    store_root: &Path,
    label: &str,
    budget: &mut Budget,
) -> Result<Option<Candidate>, Unsafe> {
    let path = rv::verdict_path(project_root, label);
    if rv::verdict_file_refused(&path) {
        return Err(Unsafe(format!(
            "the verdict file {} exists but was refused",
            path.display()
        )));
    }
    let file_body = rv::read_verdict_body(&path).ok();
    let file = match file_body.as_deref().map(rv::parse_recorded_verdict) {
        Some(None) => {
            return Err(Unsafe(format!(
                "the verdict file {} does not parse",
                path.display()
            )))
        }
        parsed => parsed.flatten(),
    };
    let events = checked::spec_verdict_events(store_root, label, budget)?;
    let newest: Option<GateEvent> = ledger::project::newest_of(events);
    if let Some(event) = newest.filter(|e| rv::ledger_supersedes(&path, file.as_ref(), e)) {
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
            selected: Selected {
                verdict,
                body,
                label: label.to_string(),
                order_time: rv::ledger_order_time(&event),
                source: VerdictSource::Ledger {
                    file: ledger::project::event_file(store_root, &event),
                    seq: event.seq,
                },
            },
        }));
    }
    Ok(match (file, file_body) {
        (Some(verdict), Some(body)) => Some(Candidate {
            selected: Selected {
                order_time: file_order_time(&path, &verdict),
                verdict,
                body,
                label: label.to_string(),
                source: VerdictSource::File(path),
            },
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
    let labels = spec_labels(ids);
    match labels.as_slice() {
        [] => AliasSelection::Absent,
        [label] => single(project_root, label),
        _ => across(project_root, &labels, &mut Budget::default()),
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
pub(crate) fn across(
    project_root: &Path,
    labels: &[String],
    budget: &mut Budget,
) -> AliasSelection {
    let store_root = match ledger::store_root(project_root) {
        Ok(root) => root,
        Err(e) => {
            return AliasSelection::Unsafe {
                reason: format!("the gate ledger cannot be resolved: {e}"),
                refusal: None,
            }
        }
    };
    let mut cands: Vec<Candidate> = Vec::new();
    let mut refused: Option<String> = None;
    for label in labels {
        match candidate(project_root, &store_root, label, budget) {
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
