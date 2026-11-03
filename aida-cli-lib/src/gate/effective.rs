//! STORY-2324 PR-A: the effective gate state of one change as a pure
//! function of its head SHA, its canonical spec set and its ledger.
//!
//! [`effective_gate_state`] reads no file, git, clock or environment: every
//! input arrives in a [`CanonicalSet`] (identity, from one committed store
//! snapshot) and a [`LedgerView`] (one checked, bounded read of the change's
//! ledger and its members' unbound entries, built by [`load`]). Ledger
//! events are attributed to a member by its canonical label set, never by
//! the literal text a writer typed, so re-spelling a label cannot change
//! the answer.
//!
//! Selection grants nothing: authority (receipt, grant, independence, solo)
//! stays a separate stage that runs on the event selected here. In PR-A
//! nothing decides from this module; `aida gate state` prints it, and
//! `--compare` sets it beside what today's readers decide.
// trace:STORY-2324 | ai:claude
// mandate-gate

use crate::gate::canon::{CanonicalSet, CanonicalSpec, SpecKey};
use crate::gate::ledger::checked::{self, Budget, Unsafe};
use crate::gate::ledger::{ChangeKey, EventBody, GateEvent};
use crate::gate::review_verdict::{self as rv, JsonObj, VerdictKind};
use std::collections::BTreeMap;
use std::path::Path;

/// How far a superseded-by chain is followed before it counts as a cycle.
const MAX_CARRIER_DEPTH: usize = 8;

/// Everything the ledger says about one change, read once.
// trace:STORY-2324 | ai:claude
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LedgerView {
    /// Every event on the change's own ledger, in seq order.
    pub change_events: Vec<GateEvent>,
    /// Per member: its unbound (pre-change) verdict events not bound to any
    /// other change, oldest first.
    pub unbound: BTreeMap<SpecKey, Vec<GateEvent>>,
}

/// The inputs that are not evidence.
// trace:STORY-2324 | ai:claude
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct GatePolicy {
    /// An interactive human at a TTY is the explicit supervised merge.
    pub interactive_tty: bool,
}

/// One member's (or the change's own) current verdict.
// trace:STORY-2324 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub(crate) enum Verdict {
    /// No verdict recorded.
    Absent,
    /// An open approval that covers the head, directly or through the
    /// ledger's carry-over proof.
    Approved {
        seq: u64,
        reviewed_sha: String,
        carried_over: bool,
    },
    /// An open refusal answers.
    ChangesRequested { seq: u64, findings: Vec<String> },
    /// The approval does not cover the head.
    Stale {
        seq: u64,
        reviewed: Option<String>,
        head: String,
        reason: String,
    },
    /// The newest record's verdict word is not recognised: never approves.
    Unrecognized { seq: u64, raw: String },
    /// The evidence cannot be read safely.
    Unsafe { reason: String },
}

impl Verdict {
    fn rank(&self) -> u8 {
        match self {
            Self::Unsafe { .. } => 5,
            Self::ChangesRequested { .. } => 4,
            Self::Unrecognized { .. } => 3,
            Self::Stale { .. } => 2,
            Self::Approved { .. } => 1,
            Self::Absent => 0,
        }
    }
}

/// The open hold, folded from the change's hold events (no legacy marker).
// trace:STORY-2324 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub(crate) enum Hold {
    None,
    Open {
        reason_kind: String,
        detail: String,
        authorized: bool,
        label_state: Option<String>,
    },
    Released,
}

/// The change's gating execution mode.
// trace:STORY-2324 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub(crate) enum Mode {
    Drain,
    /// `(native ID, mode label)` per member that needs supervision.
    Supervised {
        members: Vec<(String, String)>,
    },
}

/// Why the change may not merge, in precedence order.
// trace:STORY-2324 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "refusal", rename_all = "kebab-case")]
pub(crate) enum Refusal {
    /// A label the change ships names no requirement, or several.
    Unresolved {
        labels: Vec<String>,
    },
    UnsafeEvidence {
        reason: String,
    },
    HoldOpen,
    ChangesRequested,
    UnrecognizedVerdict,
    StaleApproval,
    NoApproval,
    Supervised {
        members: Vec<(String, String)>,
    },
    DraftOnly {
        members: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "decision", rename_all = "kebab-case")]
pub(crate) enum Decision {
    Mergeable { pin: String },
    Refused { refusals: Vec<Refusal> },
}

/// One member's row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct MemberState {
    pub key: String,
    pub native: String,
    pub mode: String,
    pub verdict: Verdict,
}

/// The effective gate state of one change.
// trace:STORY-2324 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct GateState {
    pub head: String,
    pub members: Vec<MemberState>,
    /// The change-keyed verdict (recorded with no spec).
    pub change_verdict: Verdict,
    pub verdict: Verdict,
    pub hold: Hold,
    pub mode: Mode,
    pub draft_only: Vec<String>,
    /// Spec labels on this change's verdict events that name no member.
    pub foreign_labels: Vec<String>,
    /// The most severe blocking answer (refusal, unrecognized word, unsafe)
    /// among those labels' own sequences. Today's change-keyed read blocks on
    /// such a refusal, so it is never dropped (no loosening): an open refusal
    /// on this change's ledger blocks whatever spec it names.
    pub foreign_verdict: Verdict,
    pub decision: Decision,
}

/// The mode `member` gates on: its own, else (only when it has none) that of
/// the carrier it was superseded into, when the carrier ships in the same
/// set. A chain longer than [`MAX_CARRIER_DEPTH`] (a cycle) is unset.
// trace:STORY-2324 | ai:claude
pub(crate) fn member_mode(
    set: &CanonicalSet,
    member: &CanonicalSpec,
) -> Option<aida_core::ExecutionMode> {
    let mut current = member;
    for _ in 0..MAX_CARRIER_DEPTH {
        if let Some(mode) = current.mode {
            return Some(mode);
        }
        match current.superseded_by.and_then(|k| set.member(k)) {
            Some(carrier) => current = carrier,
            None => return None,
        }
    }
    None
}

fn is_open_refusal(v: &rv::RecordedVerdict) -> bool {
    v.kind.blocks_done() && !v.is_closed()
}

/// The verdict a sequence of verdict events (oldest first) answers with at
/// `head`. The newest event (a proved phase-3 overlay answers with its
/// origin) is the candidate; an approval does not answer an open refusal in
/// the same sequence whose order time is later than its own (a late
/// collection of an old round), nor any refusal when its own order time is
/// unknown.
// trace:STORY-2324 | ai:claude
fn sequence_verdict(events: Vec<GateEvent>, head: &str, change_events: &[GateEvent]) -> Verdict {
    let parsed: Vec<(GateEvent, Option<rv::RecordedVerdict>)> = events
        .iter()
        .map(|e| {
            (
                e.clone(),
                e.verdict_record().and_then(rv::parse_ledger_record),
            )
        })
        .collect();
    let Some(newest) = crate::gate::ledger::project::newest_of(events) else {
        return Verdict::Absent;
    };
    let Some(v) = newest.verdict_record().and_then(rv::parse_ledger_record) else {
        return Verdict::Unsafe {
            reason: format!("the verdict at seq {} does not parse", newest.seq),
        };
    };
    if v.is_closed() {
        return Verdict::Absent;
    }
    match v.kind {
        VerdictKind::RequestChanges | VerdictKind::Rejected => Verdict::ChangesRequested {
            seq: newest.seq,
            findings: v.findings.clone(),
        },
        VerdictKind::Unknown => Verdict::Unrecognized {
            seq: newest.seq,
            raw: v.raw.clone(),
        },
        VerdictKind::Approved => {
            let own = rv::ledger_order_time(&newest);
            let later_refusal = parsed.iter().find(|(e, r)| {
                e.seq != newest.seq
                    && r.as_ref().is_some_and(is_open_refusal)
                    && match (own, rv::ledger_order_time(e)) {
                        (Some(a), Some(b)) => b > a,
                        _ => true,
                    }
            });
            if let Some((e, Some(r))) = later_refusal {
                return Verdict::ChangesRequested {
                    seq: e.seq,
                    findings: r.findings.clone(),
                };
            }
            approval_at_head(newest.seq, &v, head, change_events)
        }
    }
}

/// An approval against the head: covered directly, or through the newest
/// carry-over answer on the change's ledger for that approval and head.
fn approval_at_head(
    seq: u64,
    v: &rv::RecordedVerdict,
    head: &str,
    change_events: &[GateEvent],
) -> Verdict {
    let Some(reviewed) = v.reviewed_sha.as_deref().map(str::trim) else {
        return Verdict::Stale {
            seq,
            reviewed: None,
            head: head.to_string(),
            reason: "the approval records no reviewed commit".into(),
        };
    };
    if rv::same_reviewed_sha(reviewed, head) {
        return Verdict::Approved {
            seq,
            reviewed_sha: reviewed.to_string(),
            carried_over: false,
        };
    }
    let answer = change_events.iter().rev().find_map(|e| match &e.body {
        EventBody::CarryOverProof { proof, .. }
            if rv::same_reviewed_sha(&proof.approved_sha, reviewed)
                && rv::same_reviewed_sha(&proof.new_head, head) =>
        {
            Some(Ok(()))
        }
        EventBody::CarryOverRefused {
            approved_sha,
            new_head,
            reason,
            ..
        } if rv::same_reviewed_sha(approved_sha, reviewed)
            && rv::same_reviewed_sha(new_head, head) =>
        {
            Some(Err(reason.clone()))
        }
        _ => None,
    });
    match answer {
        Some(Ok(())) => Verdict::Approved {
            seq,
            reviewed_sha: reviewed.to_string(),
            carried_over: true,
        },
        Some(Err(reason)) => Verdict::Stale {
            seq,
            reviewed: Some(reviewed.to_string()),
            head: head.to_string(),
            reason: format!("carry-over refused: {reason}"),
        },
        None => Verdict::Stale {
            seq,
            reviewed: Some(reviewed.to_string()),
            head: head.to_string(),
            reason: "the approval was recorded at another commit".into(),
        },
    }
}

/// The aggregate over the change's own verdict and each member's: the most
/// severe answer wins; absent answers are skipped.
fn aggregate<'a>(verdicts: impl Iterator<Item = &'a Verdict>) -> Verdict {
    verdicts
        .filter(|v| **v != Verdict::Absent)
        .max_by_key(|v| v.rank())
        .cloned()
        .unwrap_or(Verdict::Absent)
}

fn fold_hold(events: &[GateEvent]) -> Hold {
    let st = crate::gate::ledger::hold::fold(events);
    match (st.open, st.release) {
        (Some(open), _) => Hold::Open {
            reason_kind: open.reason_kind.as_str().to_string(),
            detail: open.detail,
            authorized: st.authorized.is_some(),
            label_state: open.label_state,
        },
        (None, Some(_)) => Hold::Released,
        (None, None) => Hold::None,
    }
}

/// The member's events with their spec label replaced by its canonical
/// spelling, so overlay pairing does not depend on how a writer spelled it.
fn canonical_spelling(member: &CanonicalSpec, mut e: GateEvent) -> GateEvent {
    e.spec = Some(member.native.to_ascii_uppercase());
    e
}

/// STORY-2324: the effective gate state. Pure.
// trace:STORY-2324 | ai:claude
pub(crate) fn effective_gate_state(
    head: &str,
    set: &CanonicalSet,
    view: &LedgerView,
    policy: &GatePolicy,
) -> GateState {
    let head = head.trim();
    let verdict_events: Vec<&GateEvent> = view
        .change_events
        .iter()
        .filter(|e| e.verdict_record().is_some())
        .collect();
    let mut foreign: BTreeMap<String, Vec<GateEvent>> = BTreeMap::new();
    let mut change_own: Vec<GateEvent> = Vec::new();
    let mut per_member: BTreeMap<SpecKey, Vec<GateEvent>> = BTreeMap::new();
    for e in &verdict_events {
        match e.spec.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            None => change_own.push((*e).clone()),
            Some(label) => match set.member_for_label(label) {
                Some(m) => per_member
                    .entry(m.key)
                    .or_default()
                    .push(canonical_spelling(m, (*e).clone())),
                None => foreign
                    .entry(label.to_ascii_uppercase())
                    .or_default()
                    .push((*e).clone()),
            },
        }
    }
    let foreign_labels: Vec<String> = foreign.keys().cloned().collect();
    // trace:STORY-2324 | ai:claude
    let foreign_answers: Vec<Verdict> = foreign
        .into_values()
        .map(|events| sequence_verdict(events, head, &view.change_events))
        .filter(|v| {
            matches!(
                v,
                Verdict::ChangesRequested { .. }
                    | Verdict::Unrecognized { .. }
                    | Verdict::Unsafe { .. }
            )
        })
        .collect();
    let foreign_verdict = aggregate(foreign_answers.iter());

    let mut members = Vec::new();
    for m in &set.members {
        let bound = per_member.remove(&m.key).unwrap_or_default();
        // An unbound record already on this change is read once, from it.
        let unbound: Vec<GateEvent> = view
            .unbound
            .get(&m.key)
            .into_iter()
            .flatten()
            .filter(|u| {
                !verdict_events
                    .iter()
                    .any(|b| b.verdict_record() == u.verdict_record())
            })
            .cloned()
            .map(|e| canonical_spelling(m, e))
            .collect();
        let events: Vec<GateEvent> = unbound.into_iter().chain(bound).collect();
        let mode = member_mode(set, m);
        members.push((m, mode, sequence_verdict(events, head, &view.change_events)));
    }
    let change_verdict = sequence_verdict(change_own, head, &view.change_events);
    let verdict = aggregate(
        std::iter::once(&change_verdict)
            .chain(std::iter::once(&foreign_verdict))
            .chain(members.iter().map(|(_, _, v)| v)),
    );
    let hold = fold_hold(&view.change_events);
    let supervised: Vec<(String, String)> = members
        .iter()
        .filter(|(_, mode, _)| crate::gate::ship::merge_requires_supervision(*mode))
        .map(|(m, mode, _)| {
            (
                m.native.clone(),
                crate::gate::ship::supervision_mode_label(*mode),
            )
        })
        .collect();
    let mode = if supervised.is_empty() {
        Mode::Drain
    } else {
        Mode::Supervised {
            members: supervised,
        }
    };
    let draft_only: Vec<String> = set
        .members
        .iter()
        .filter(|m| crate::gate::ship::is_draft_only_tagged(&m.tags))
        .map(|m| m.native.clone())
        .collect();

    let mut refusals = Vec::new();
    let mut unresolved = set.unresolved.clone();
    unresolved.extend(set.ambiguous.iter().map(|(l, _)| l.clone()));
    if !unresolved.is_empty() {
        unresolved.sort();
        refusals.push(Refusal::Unresolved { labels: unresolved });
    }
    if head.is_empty() {
        refusals.push(Refusal::UnsafeEvidence {
            reason: "the change's head is unknown".into(),
        });
    }
    if let Verdict::Unsafe { reason } = &verdict {
        refusals.push(Refusal::UnsafeEvidence {
            reason: reason.clone(),
        });
    }
    if matches!(hold, Hold::Open { .. }) {
        refusals.push(Refusal::HoldOpen);
    }
    match &verdict {
        Verdict::ChangesRequested { .. } => refusals.push(Refusal::ChangesRequested),
        Verdict::Unrecognized { .. } => refusals.push(Refusal::UnrecognizedVerdict),
        Verdict::Stale { .. } => refusals.push(Refusal::StaleApproval),
        Verdict::Absent => refusals.push(Refusal::NoApproval),
        Verdict::Approved { .. } | Verdict::Unsafe { .. } => {}
    }
    if let (Mode::Supervised { members }, false) = (&mode, policy.interactive_tty) {
        refusals.push(Refusal::Supervised {
            members: members.clone(),
        });
    }
    if !draft_only.is_empty() {
        refusals.push(Refusal::DraftOnly {
            members: draft_only.clone(),
        });
    }
    let decision = if refusals.is_empty() {
        Decision::Mergeable {
            pin: head.to_string(),
        }
    } else {
        Decision::Refused { refusals }
    };
    GateState {
        head: head.to_string(),
        members: members
            .into_iter()
            .map(|(m, mode, verdict)| MemberState {
                key: m.key.to_string(),
                native: m.native.clone(),
                mode: crate::gate::ship::supervision_mode_label(mode),
                verdict,
            })
            .collect(),
        change_verdict,
        verdict,
        hold,
        mode,
        draft_only,
        foreign_labels,
        foreign_verdict,
        decision,
    }
}

/// STORY-2324: the one checked, bounded ledger read [`effective_gate_state`]
/// needs for `change` under the store root `root`: the change's own ledger,
/// and per member the unbound entries of every label it answers to, minus
/// any record already bound to some other change (a spec approval recorded
/// on a different change never counts here).
// trace:STORY-2324 | ai:claude
pub(crate) fn load_view(
    root: &Path,
    change: ChangeKey,
    set: &CanonicalSet,
) -> Result<LedgerView, Unsafe> {
    let mut budget = Budget::default();
    let change_events = checked::read_change_events(root, change, &mut budget)?;
    let mut unbound = BTreeMap::new();
    let mut others: std::collections::HashMap<ChangeKey, Vec<GateEvent>> = Default::default();
    for m in &set.members {
        let mut events: Vec<GateEvent> = Vec::new();
        let mut indexed: Vec<ChangeKey> = Vec::new();
        for label in &m.labels {
            if crate::gate::ledger::project::spec_key(label).is_none() {
                continue;
            }
            let (u, c) = checked::read_spec_entries(root, label, &mut budget)?;
            events.extend(u);
            indexed.extend(c);
        }
        indexed.sort_by_key(|c| c.as_key());
        indexed.dedup();
        let mut bound_elsewhere: Vec<JsonObj> = Vec::new();
        for other in indexed.into_iter().filter(|c| *c != change) {
            if !others.contains_key(&other) {
                let read = checked::read_change_events(root, other, &mut budget)?;
                others.insert(other, read);
            }
            bound_elsewhere.extend(
                others[&other]
                    .iter()
                    .filter(|e| e.spec.as_deref().is_some_and(|s| m.answers_to(s)))
                    .filter_map(|e| e.verdict_record().cloned()),
            );
        }
        events.retain(|e| {
            e.verdict_record()
                .is_some_and(|r| !bound_elsewhere.contains(r))
        });
        // Unbound entries live in one file per label; the primary writer's
        // append time orders them across files, seq within one.
        events.sort_by(|a, b| a.at.cmp(&b.at).then(a.seq.cmp(&b.seq)));
        if !events.is_empty() {
            unbound.insert(m.key, events);
        }
    }
    Ok(LedgerView {
        change_events,
        unbound,
    })
}

/// What [`load`] read.
#[derive(Debug, Clone)]
pub(crate) struct Loaded {
    pub set: CanonicalSet,
    pub view: LedgerView,
    /// The store commit identity was read at.
    pub store_head: String,
}

/// STORY-2324: resolve `labels` (the change's E1 subject) against the store
/// HEAD snapshot and read the change's ledger from the gate-ledger root
/// `start` shares. Any refusal is an error: a caller never answers past it.
// trace:STORY-2324 | ai:claude
pub(crate) fn load(start: &Path, change: ChangeKey, labels: &[String]) -> anyhow::Result<Loaded> {
    let snapshot = crate::gate::canon::Snapshot::at_head(start)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .ok_or_else(|| anyhow::anyhow!("this project has no git-canonical requirements store"))?;
    let set = snapshot.resolve(labels);
    let root = crate::gate::ledger::store_root(start)?;
    let view = load_view(&root, change, &set).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(Loaded {
        set,
        view,
        store_head: snapshot.head,
    })
}

#[cfg(test)]
#[path = "tests/story_2324_effective_tests.rs"]
mod tests;
