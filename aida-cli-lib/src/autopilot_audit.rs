//! TASK-1018 (EPIC-0428 / the TASK-0430 design): the DURABLE execution audit
//! and the one-command reversal that pairs with it.
//!
//! `autopilot.rs` owns the side-effect-free policy envelope — the four gates
//! that decide whether one proposed [`Decision`] may auto-execute. This module
//! owns what happens the instant a gate verdict is [`Outcome::Execute`] and the
//! action actually lands on a spec:
//!
//! 1. **a durable record** ([`ExecutionRecord`]) — WHAT autopilot did, to WHICH
//!    spec, under WHICH action class, justified by WHICH gate/authority, plus
//!    the [`PriorState`] needed to put it back;
//! 2. **a one-command reversal** — [`plan_reversal`] derives the concrete
//!    restore steps from that record alone (pure), and [`apply_reversal`]
//!    executes them through the same in-process edit/queue paths the CLI uses.
//!
//! Three invariants make "every `Execute` outcome is reversible" structural
//! rather than aspirational:
//!
//! - [`execution_record`] REFUSES a non-`Execute` outcome — a held or escalated
//!   decision never produces an execution row (that is the projection log's job).
//! - It REFUSES an action class that is not
//!   [`ActionClass::is_reversible`] — the structural companion to the gate-4
//!   risk ceiling.
//! - It REFUSES an empty [`PriorState`] — a record with nothing to restore is a
//!   record that cannot be reversed, so it is rejected at mint time rather than
//!   discovered to be useless at reversal time.
//!
//! ## Storage
//!
//! Rows append as JSONL to the SAME `.aida/autopilot-audit.jsonl` the TASK-1147
//! projection surface writes, following the `.aida/events.jsonl` /
//! `~/.aida/usage.jsonl` append-only convention (one JSON object per line,
//! never rewritten, gitignored per-clone runtime state). Four `type`
//! discriminators share the one file — `decision` / `challenge` (projection,
//! `autopilot.rs`) and `execution` / `reversal` (durable, here). Each reader
//! filters on `type`, so the two surfaces never see each other's rows.
//!
//! ## Reserved extension points (do not repurpose)
//!
//! Four sibling specs extend this same record. They are already named on the
//! struct with additive-tolerant serde so landing them is a field write, not a
//! schema migration:
//!
//! - `evidence` — the cited substrate backing the grounding. TASK-1019 fills it
//!   with product-role recommendations feeding gate 3 (as EVIDENCE, never as
//!   authority).
//! - `from_product` — the TASK-1013 `--from-product` audit filter: true when the
//!   decision's evidence came from a product handoff. The filter and the
//!   evidence CONVENTION that feeds it are live (see "Product-sourced evidence"
//!   below); the producer that emits product evidence is TASK-0431's half.
//! - `mode` — which composition mode produced the decision (`autopilot` /
//!   `zen+autopilot` / `solo+autopilot`). Derived at mint from the surface and
//!   the live solo posture (see "Composition mode" below); TASK-1014. Noting
//!   product-sourced decisions taken during a headless drain is TASK-1022's half.
//! - `extra` — a `#[serde(flatten)]` catch-all so a record written by a NEWER
//!   binary round-trips through an older one without losing fields.
//!
//! trace:TASK-1018 trace:TASK-0430 | ai:claude
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::autopilot::{audit_log_path, short_id, ActionClass, Authority, Decision, Outcome};

/// Placeholder for a producer that did not name itself. Kept explicit so a
/// blank `source` can never read as a legitimate surface name.
const RECORD_SOURCE_UNKNOWN: &str = "unknown";

/// Version of the [`ExecutionRecord`] shape. Bump only on a BREAKING change —
/// additive fields do not need it (readers tolerate them via `#[serde(default)]`
/// plus the `extra` catch-all).
pub(crate) const EXECUTION_SCHEMA: u32 = 1;

/// `type` discriminator for a durable execution row.
pub(crate) const KIND_EXECUTION: &str = "execution";
/// `type` discriminator for a reversal row.
pub(crate) const KIND_REVERSAL: &str = "reversal";

// ---------------------------------------------------------------------------
// The record shapes
// ---------------------------------------------------------------------------

/// Enough of the spec's pre-action state to put it back.
///
/// Deliberately a DELTA, not a snapshot: recording "the tags autopilot added"
/// reverses cleanly even if a human edited other tags in between, whereas a full
/// tag-set snapshot would clobber that human's concurrent edit on restore. The
/// same reasoning drives `status` (a single before-value) and the two queue
/// fields (the one queue moved, not the whole queue).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PriorState {
    /// The spec's status BEFORE the action; `None` = the action did not touch
    /// status. Reversal writes this value back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Tags the action ADDED. Reversal removes them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags_added: Vec<String>,
    /// Tags the action REMOVED. Reversal restores them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags_removed: Vec<String>,
    /// Role queue the action ADDED the spec to. Reversal removes the entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_to_role: Option<String>,
    /// Role queue the action REMOVED the spec from (the far half of a `route`
    /// move). Reversal re-adds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dequeued_from_role: Option<String>,
    /// The `deferred` view flag BEFORE the action; `None` = untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred: Option<bool>,
    /// A marker for an APPEND-ONLY artifact the action wrote (a comment body
    /// prefix, a finding id). The substrate keeps it, so reversal records a
    /// retraction note rather than deleting — and the plan reports itself
    /// [`ReversalPlan::complete`] `== false` so the operator is never told a
    /// partial undo was total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appended_marker: Option<String>,
}

impl PriorState {
    /// True when there is nothing recorded to restore. A record in this state is
    /// refused at mint time — it could never be reversed.
    pub(crate) fn is_empty(&self) -> bool {
        self.status.is_none()
            && self.tags_added.is_empty()
            && self.tags_removed.is_empty()
            && self.queued_to_role.is_none()
            && self.dequeued_from_role.is_none()
            && self.deferred.is_none()
            && self.appended_marker.is_none()
    }

    /// Convenience: the prior state of a pure status flip (approve / reject /
    /// park), the shape every current producer writes.
    pub(crate) fn from_status(prior_status: &str) -> Self {
        Self {
            status: Some(prior_status.to_string()),
            ..Self::default()
        }
    }

    /// One-line summary of what reverting would restore. `None` when nothing is
    /// recorded (which the mint path already refuses). Shared by the CLI table
    /// and the durable audit comment so the two never describe it differently.
    pub(crate) fn describe(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(s) = &self.status {
            parts.push(format!("status={s}"));
        }
        if let Some(d) = self.deferred {
            parts.push(format!("deferred={d}"));
        }
        if !self.tags_added.is_empty() {
            parts.push(format!("-tags {}", self.tags_added.join(",")));
        }
        if !self.tags_removed.is_empty() {
            parts.push(format!("+tags {}", self.tags_removed.join(",")));
        }
        if let Some(r) = &self.queued_to_role {
            parts.push(format!("off the {r} queue"));
        }
        if let Some(r) = &self.dequeued_from_role {
            parts.push(format!("back on the {r} queue"));
        }
        if let Some(m) = &self.appended_marker {
            parts.push(format!("retract {m}"));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}

/// One durable row: an autopilot action that ACTUALLY EXECUTED against a spec.
///
/// Distinct from `autopilot::AuditEntry` (the TASK-1147 *projection* row, which
/// records what the envelope WOULD decide during a dry-run). Only an
/// [`Outcome::Execute`] that landed produces one of these.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct ExecutionRecord {
    /// Record-shape version (see [`EXECUTION_SCHEMA`]).
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// Stable short id (`x########`) — the reversal target.
    pub id: String,
    /// RFC-3339 UTC timestamp.
    pub ts: String,
    /// Always [`KIND_EXECUTION`]; the log's `type` discriminator.
    #[serde(rename = "type")]
    pub kind: String,
    /// Display SPEC-ID the action targeted.
    pub spec_id: String,
    /// The action class token (`approve`, `queue`, `tag`, …).
    pub action: String,
    /// The envelope verdict — always `execute` for this row kind, carried
    /// explicitly so the log is self-describing next to the projection rows.
    pub verdict: String,
    /// Which gate combination justified it (`all-gates-pass`).
    pub gate: String,
    /// The gate-2 authority that permitted it (`auto`).
    pub authority: String,
    /// The recorded grounding classification (gate-3 input).
    pub grounding: String,
    /// The recorded risk read (gate-4 input).
    pub risk: String,
    /// One-line rationale carried from the [`Decision`].
    pub reason: String,
    /// Who executed it — the shell/user identity the action ran under.
    pub actor: String,
    /// Which surface produced it (`zen`, `groom`, `inspect`, …).
    pub source: String,
    /// The prior state needed to reverse. Never empty (enforced at mint).
    pub prior: PriorState,

    // ---- reserved extension points (see the module doc) --------------------
    /// RESERVED (TASK-1019): cited substrate backing the grounding.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    /// The composition mode that produced the decision (`autopilot` /
    /// `zen+autopilot` / `solo+autopilot`). Derived at mint from the producing
    /// surface plus the live solo posture ([`composition_mode`]); absent only on
    /// a row minted before the field was written, where [`record_mode`] falls
    /// back to the surface alone.
    // trace:TASK-1014 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// `Some(true)` when the decision's evidence recorded a product handoff —
    /// the field the `--from-product` audit filter reads. Derived at mint from
    /// the `product:<who>` evidence markers ([`from_product_flag`]); absent
    /// rather than `Some(false)` when there was no product input.
    // trace:TASK-1013 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_product: Option<bool>,
    /// Forward-compatibility: fields written by a NEWER binary survive a
    /// read/write round-trip through this one instead of being dropped.
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

fn default_schema() -> u32 {
    EXECUTION_SCHEMA
}

/// One durable row recording that an [`ExecutionRecord`] was reversed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct ReversalRecord {
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// Stable short id (`r########`).
    pub id: String,
    pub ts: String,
    /// Always [`KIND_REVERSAL`].
    #[serde(rename = "type")]
    pub kind: String,
    /// The [`ExecutionRecord::id`] this reverses.
    pub target: String,
    pub spec_id: String,
    /// Human-readable description of each step applied, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<String>,
    /// False when an append-only artifact could only be retracted, not deleted.
    pub complete: bool,
    /// Who reversed it.
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Product-sourced evidence
// ---------------------------------------------------------------------------
//
// The product seat is non-privileged BY CONSTRUCTION: the advisor-authority gate
// downgrades a product `--status approved` to Draft and refuses its queue
// writes, so a product recommendation can only ever reach autopilot as
// EVIDENCE feeding gate 3 — never as authority over gates 1, 2 or 4.
//
// That makes one question an operator has to be able to ask of the trail:
// "which autopilot actions acted on product input?" — the provenance-laundering
// guard, i.e. is a product seat quietly steering the queue. This section is the
// answer side of it: the evidence-marker CONVENTION (`product:<who>`), the pure
// predicates that read it, and the `--from-product` filter they power.
//
// The producer that WRITES product evidence lands separately; the convention is
// fixed here so the writer and the reader cannot disagree on the spelling, and
// so the filter is truthful the moment the first product-sourced record appears
// (no migration, no reindex — the flag is derived from the evidence at mint).
// trace:TASK-1013 | ai:claude

/// Prefix marking one `Decision::evidence` entry as a PRODUCT-ROLE handoff:
/// `product:<who>`. Matched case-insensitively so a hand-written marker is not
/// silently dropped from the audit.
pub(crate) const PRODUCT_EVIDENCE_PREFIX: &str = "product:";

/// The `<who>` half of one `product:<who>` evidence entry, or `None` when the
/// entry is not a product marker. A bare `product:` IS a valid marker (the
/// handoff happened; the seat just did not name itself) and yields `Some("")`.
fn product_marker_who(entry: &str) -> Option<&str> {
    let entry = entry.trim();
    let head = entry.get(..PRODUCT_EVIDENCE_PREFIX.len())?;
    head.eq_ignore_ascii_case(PRODUCT_EVIDENCE_PREFIX)
        .then(|| entry[PRODUCT_EVIDENCE_PREFIX.len()..].trim())
}

/// PURE: does this evidence set record a product handoff?
// trace:TASK-1013 | ai:claude
pub(crate) fn evidence_has_product_handoff(evidence: &[String]) -> bool {
    evidence.iter().any(|e| product_marker_who(e).is_some())
}

/// PURE: who handed the recommendation over, from the first named
/// `product:<who>` entry. `None` when there is no product evidence, or when the
/// marker is present but unnamed.
// trace:TASK-1013 | ai:claude
pub(crate) fn product_provenance(evidence: &[String]) -> Option<String> {
    evidence
        .iter()
        .filter_map(|e| product_marker_who(e))
        .find(|who| !who.is_empty())
        .map(|who| who.to_string())
}

/// PURE: the value the mint path writes into [`ExecutionRecord::from_product`].
///
/// `Some(true)` when the decision consumed a product handoff, `None` when it did
/// not — deliberately absent rather than `Some(false)`, so a row only carries the
/// field when it has something to say (matching the `skip_serializing_if` on the
/// struct, and keeping every existing non-product row byte-identical).
// trace:TASK-1013 | ai:claude
pub(crate) fn from_product_flag(evidence: &[String]) -> Option<bool> {
    evidence_has_product_handoff(evidence).then_some(true)
}

/// PURE: the `--from-product` filter predicate.
///
/// Reads the recorded flag FIRST, then falls back to the evidence markers, so
/// the filter is truthful for three kinds of row at once: one this binary
/// minted (flag set from the evidence), one a newer binary set the flag on
/// directly, and one whose producer only tagged the evidence. An explicit
/// `from_product: false` is honoured as a deliberate "not product-sourced" even
/// if a marker-shaped string is loose in the evidence.
// trace:TASK-1013 | ai:claude
pub(crate) fn is_from_product(rec: &ExecutionRecord) -> bool {
    match rec.from_product {
        Some(flag) => flag,
        None => evidence_has_product_handoff(&rec.evidence),
    }
}

/// PURE: the one-line product-handoff annotation for the executions table, or
/// `None` when the row is not product-sourced. Shared by every surface so the
/// human table and any later renderer describe the handoff identically.
// trace:TASK-1013 | ai:claude
pub(crate) fn product_annotation(rec: &ExecutionRecord) -> Option<String> {
    if !is_from_product(rec) {
        return None;
    }
    Some(match product_provenance(&rec.evidence) {
        Some(who) => format!("product handoff: {who}"),
        None => "product handoff".to_string(),
    })
}

// ---------------------------------------------------------------------------
// Composition mode
// ---------------------------------------------------------------------------
//
// Autopilot is a GROOMING-stage posture; the three-mode autonomy ladder and the
// solo presence marker are DRAINING-stage/session-wide axes. They compose rather
// than conflict, so "autopilot did this" is never the whole story: the same
// envelope executing under an operator-invoked `aida zen`, under an active solo
// posture, or on its own during a headless `groom --autopilot` are materially
// different levels of supervision. A trail that flattens them cannot answer the
// question an operator actually asks of it — "what was I doing when this
// landed?" — and cannot tell a supervised one-shot apart from an unattended pass
// after the fact.
//
// `mode` is that answer, recorded at mint from two things already in hand: the
// SURFACE that produced the action (the record's `source`) and the solo POSTURE
// in effect at the time. Composed outermost-first and always ending in
// `autopilot`, so the vocabulary is `autopilot` / `zen+autopilot` /
// `solo+autopilot` — and a composition of both layers keeps both rather than
// silently dropping one.
// trace:TASK-1014 | ai:claude

/// The innermost layer, present on every mode: the envelope itself. Alone, it
/// is the bare mode — a `groom --autopilot` pass with nothing composed over it.
pub(crate) const MODE_AUTOPILOT: &str = "autopilot";
/// The `zen` SURFACE layer: an operator-invoked one-shot composed the action.
/// Matched against [`ExecutionRecord::source`].
pub(crate) const MODE_LAYER_ZEN: &str = "zen";
/// The solo POSTURE layer: the operator is the only one home, so the session
/// carries the safe-vs-keystone partition `presence::resolve_solo_posture` maps.
pub(crate) const MODE_LAYER_SOLO: &str = "solo";

/// PURE: the composition mode token for one action.
///
/// `solo` is the OUTER layer — a posture spanning the whole session — and the
/// surface is the inner one, so the three named compositions come out as
/// `autopilot`, `zen+autopilot` and `solo+autopilot`. A zen one-shot taken while
/// solo is active records BOTH (`solo+zen+autopilot`): the layers are additive,
/// because dropping one to force the token into a fixed set of three would make
/// the record less true than the run it describes.
// trace:TASK-1014 | ai:claude
pub(crate) fn composition_mode(source: &str, solo: bool) -> String {
    let mut layers: Vec<&str> = Vec::new();
    if solo {
        layers.push(MODE_LAYER_SOLO);
    }
    // `solo` is a posture, so it composes with any surface; `zen` is the one
    // surface that is itself an autonomy composition. Every other source
    // (`groom`, `inspect`, …) IS the bare envelope and adds no layer — the
    // record's `source` already names it.
    if source.trim().eq_ignore_ascii_case(MODE_LAYER_ZEN) {
        layers.push(MODE_LAYER_ZEN);
    }
    layers.push(MODE_AUTOPILOT);
    layers.join("+")
}

/// PURE: the mode to REPORT for one record.
///
/// Reads the recorded field FIRST and falls back to deriving from the surface,
/// so a row minted before the field was written still reports a truthful
/// composition instead of a blank. The fallback deliberately assumes solo was
/// OFF: the posture is unrecoverable after the fact, and under-claiming
/// supervision is the safe direction to be wrong in.
// trace:TASK-1014 | ai:claude
pub(crate) fn record_mode(rec: &ExecutionRecord) -> String {
    match rec.mode.as_deref().map(str::trim) {
        Some(m) if !m.is_empty() => m.to_string(),
        _ => composition_mode(&rec.source, false),
    }
}

/// PURE: the `--mode` filter predicate.
///
/// A full token (`zen+autopilot`) or the bare `autopilot` matches EXACTLY —
/// `--mode autopilot` asks for the un-composed actions, and a layer match would
/// quietly degrade that into "everything", since every mode ends in the
/// envelope. Any other single word is read as a LAYER, so `--mode solo` selects
/// every solo-composed action however else it was composed. Case-insensitive
/// throughout: the tokens are recorded lowercase, an operator need not know.
// trace:TASK-1014 | ai:claude
pub(crate) fn mode_matches(rec: &ExecutionRecord, wanted: &str) -> bool {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return true;
    }
    let mode = record_mode(rec);
    if wanted.contains('+') || wanted.eq_ignore_ascii_case(MODE_AUTOPILOT) {
        return mode.eq_ignore_ascii_case(wanted);
    }
    mode.split('+')
        .any(|l| l.trim().eq_ignore_ascii_case(wanted))
}

/// PURE: the one-line composition annotation for the executions table, or
/// `None` when the action ran under the bare envelope — the default needs no
/// comment, and annotating every row would bury the composed ones.
// trace:TASK-1014 | ai:claude
pub(crate) fn mode_annotation(rec: &ExecutionRecord) -> Option<String> {
    let mode = record_mode(rec);
    (mode != MODE_AUTOPILOT).then(|| format!("mode: {mode}"))
}

// ---------------------------------------------------------------------------
// Minting (pure)
// ---------------------------------------------------------------------------

/// Why a durable execution record could not be minted, or a reversal could not
/// be planned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuditError {
    /// The outcome was not [`Outcome::Execute`] — held/escalated decisions
    /// belong in the projection log, not the execution log.
    NotExecuted(String),
    /// The action class is not reversible, so it must never auto-execute.
    Irreversible(String),
    /// No prior state was recorded — nothing to restore.
    NothingToReverse(String),
    /// The record was already reversed.
    AlreadyReversed(String),
    /// No execution record matches the requested target.
    NoSuchTarget(String),
}

impl std::fmt::Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditError::NotExecuted(id) => write!(
                f,
                "{id}: only an auto-executed action gets a durable record (this one was held or escalated)"
            ),
            AuditError::Irreversible(a) => write!(
                f,
                "the `{a}` action is not reversible, so it may never auto-execute"
            ),
            AuditError::NothingToReverse(id) => {
                write!(f, "{id}: no prior state was recorded, so it cannot be reversed")
            }
            AuditError::AlreadyReversed(id) => write!(f, "{id} was already reversed"),
            AuditError::NoSuchTarget(t) => write!(
                f,
                "no autopilot execution matches `{t}` — pass an execution id or a SPEC-ID with an un-reversed execution"
            ),
        }
    }
}

impl std::error::Error for AuditError {}

/// PURE: mint the durable record for one action that auto-executed.
///
/// Refuses (rather than silently degrading) on all three reversibility
/// preconditions — see the module doc. `seq` disambiguates records minted in the
/// same millisecond so ids stay unique within a batch, exactly as
/// `autopilot::decision_entry` does. `solo` is the solo posture in effect at the
/// time, the one composition layer the record cannot recover from its own
/// fields — [`record_execution`] resolves it so no producer has to remember to.
// trace:TASK-1018 trace:TASK-1014 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn execution_record(
    ts: &str,
    seq: usize,
    decision: &Decision,
    outcome: Outcome,
    authority: Authority,
    actor: &str,
    source: &str,
    solo: bool,
    prior: PriorState,
) -> Result<ExecutionRecord, AuditError> {
    if outcome != Outcome::Execute {
        return Err(AuditError::NotExecuted(decision.spec_id.clone()));
    }
    if !decision.action.is_reversible() {
        return Err(AuditError::Irreversible(
            decision.action.token().to_string(),
        ));
    }
    if prior.is_empty() {
        return Err(AuditError::NothingToReverse(decision.spec_id.clone()));
    }
    let seed = format!(
        "{ts}|{seq}|{}|{}|{}",
        decision.spec_id,
        decision.action.token(),
        source
    );
    Ok(ExecutionRecord {
        schema: EXECUTION_SCHEMA,
        id: short_id('x', &seed),
        ts: ts.to_string(),
        kind: KIND_EXECUTION.to_string(),
        spec_id: decision.spec_id.clone(),
        action: decision.action.token().to_string(),
        verdict: outcome.verdict_token().to_string(),
        gate: outcome.gate_label().to_string(),
        authority: authority.token().to_string(),
        grounding: decision.grounding.token().to_string(),
        risk: decision.risk.token().to_string(),
        reason: decision.reason.clone(),
        actor: actor.to_string(),
        source: if source.trim().is_empty() {
            RECORD_SOURCE_UNKNOWN.to_string()
        } else {
            source.to_string()
        },
        prior,
        // TASK-1013: derived from the evidence, not passed in — the flag and the
        // markers it summarizes can never drift apart, and every producer that
        // cites a product handoff becomes filterable without touching this call.
        from_product: from_product_flag(&decision.evidence),
        evidence: decision.evidence.clone(),
        // TASK-1014: recorded at mint, because supervision context is exactly
        // the thing that cannot be reconstructed later — the posture has moved
        // on by the time anyone reads the trail.
        mode: Some(composition_mode(source, solo)),
        extra: BTreeMap::new(),
    })
}

// ---------------------------------------------------------------------------
// Reversal planning (pure)
// ---------------------------------------------------------------------------

/// One concrete restore operation derived from a record's [`PriorState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReversalStep {
    /// Put the spec's status back.
    SetStatus { spec_id: String, status: String },
    /// Restore tags the action removed.
    AddTags { spec_id: String, tags: Vec<String> },
    /// Strip tags the action added.
    RemoveTags { spec_id: String, tags: Vec<String> },
    /// Pop the spec off a role queue the action added it to.
    QueueRemove { spec_id: String, role: String },
    /// Put the spec back on a role queue the action moved it off.
    QueueAdd { spec_id: String, role: String },
    /// Restore the `deferred` view flag.
    SetDeferred { spec_id: String, deferred: bool },
    /// Append a retraction note for an append-only artifact (comment/finding).
    /// The artifact itself survives — the audit trail stays honest.
    Retract { spec_id: String, marker: String },
}

impl ReversalStep {
    /// Stable one-line description, stored on the [`ReversalRecord`] and printed
    /// by the CLI (both the `--dry-run` preview and the applied summary).
    pub(crate) fn describe(&self) -> String {
        match self {
            ReversalStep::SetStatus { spec_id, status } => {
                format!("{spec_id}: status → {status}")
            }
            ReversalStep::AddTags { spec_id, tags } => {
                format!("{spec_id}: restore tag(s) {}", tags.join(","))
            }
            ReversalStep::RemoveTags { spec_id, tags } => {
                format!("{spec_id}: remove tag(s) {}", tags.join(","))
            }
            ReversalStep::QueueRemove { spec_id, role } => {
                format!("{spec_id}: remove from the {role} queue")
            }
            ReversalStep::QueueAdd { spec_id, role } => {
                format!("{spec_id}: restore to the {role} queue")
            }
            ReversalStep::SetDeferred { spec_id, deferred } => {
                format!("{spec_id}: deferred → {deferred}")
            }
            ReversalStep::Retract { spec_id, marker } => {
                format!("{spec_id}: append a retraction note for {marker}")
            }
        }
    }
}

/// The one-command reversal, fully derived from one durable record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReversalPlan {
    /// The [`ExecutionRecord::id`] being reversed.
    pub target: String,
    pub spec_id: String,
    pub steps: Vec<ReversalStep>,
    /// False when at least one step is a retraction of an append-only artifact,
    /// i.e. the prior state is restored but the artifact remains on the record.
    pub complete: bool,
}

/// PURE: derive the reversal plan from a durable record alone.
///
/// Note this reads ONLY the record — no store, no config, no clock. That is the
/// point: a reversal must work months later, from a different machine, after the
/// binary that wrote the row is gone. Steps are ordered so state restores before
/// queue membership (a queue entry pointing at a spec whose status is mid-flight
/// is the confusing intermediate).
// trace:TASK-1018 | ai:claude
pub(crate) fn plan_reversal(rec: &ExecutionRecord) -> Result<ReversalPlan, AuditError> {
    if rec.verdict != Outcome::Execute.verdict_token() {
        return Err(AuditError::NotExecuted(rec.id.clone()));
    }
    if let Some(action) = ActionClass::parse(&rec.action) {
        if !action.is_reversible() {
            return Err(AuditError::Irreversible(rec.action.clone()));
        }
    }
    let spec = rec.spec_id.clone();
    let mut steps = Vec::new();
    if let Some(status) = &rec.prior.status {
        steps.push(ReversalStep::SetStatus {
            spec_id: spec.clone(),
            status: status.clone(),
        });
    }
    if let Some(deferred) = rec.prior.deferred {
        steps.push(ReversalStep::SetDeferred {
            spec_id: spec.clone(),
            deferred,
        });
    }
    if !rec.prior.tags_added.is_empty() {
        steps.push(ReversalStep::RemoveTags {
            spec_id: spec.clone(),
            tags: rec.prior.tags_added.clone(),
        });
    }
    if !rec.prior.tags_removed.is_empty() {
        steps.push(ReversalStep::AddTags {
            spec_id: spec.clone(),
            tags: rec.prior.tags_removed.clone(),
        });
    }
    if let Some(role) = &rec.prior.queued_to_role {
        steps.push(ReversalStep::QueueRemove {
            spec_id: spec.clone(),
            role: role.clone(),
        });
    }
    if let Some(role) = &rec.prior.dequeued_from_role {
        steps.push(ReversalStep::QueueAdd {
            spec_id: spec.clone(),
            role: role.clone(),
        });
    }
    let mut complete = true;
    if let Some(marker) = &rec.prior.appended_marker {
        complete = false;
        steps.push(ReversalStep::Retract {
            spec_id: spec.clone(),
            marker: marker.clone(),
        });
    }
    if steps.is_empty() {
        return Err(AuditError::NothingToReverse(rec.id.clone()));
    }
    Ok(ReversalPlan {
        target: rec.id.clone(),
        spec_id: spec,
        steps,
        complete,
    })
}

/// True iff a [`ReversalRecord`] already targets this execution id.
pub(crate) fn is_reversed(reversals: &[ReversalRecord], execution_id: &str) -> bool {
    reversals.iter().any(|r| r.target == execution_id)
}

/// PURE: resolve what `aida autopilot revert <TARGET>` refers to.
///
/// `target` matches either an execution id EXACTLY, or (fallback) the most
/// recent still-UN-REVERSED execution for that SPEC-ID (case-insensitive) — the
/// "undo the last thing autopilot did to this spec" form, which is what an
/// operator reaching for a reversal actually has in hand. Mirrors
/// `autopilot::resolve_challenge_target` so the two surfaces resolve alike.
// trace:TASK-1018 | ai:claude
pub(crate) fn resolve_revert_target(
    executions: &[ExecutionRecord],
    reversals: &[ReversalRecord],
    target: &str,
) -> Result<ExecutionRecord, AuditError> {
    if let Some(rec) = executions.iter().find(|e| e.id == target) {
        if is_reversed(reversals, &rec.id) {
            return Err(AuditError::AlreadyReversed(rec.id.clone()));
        }
        return Ok(rec.clone());
    }
    executions
        .iter()
        .rev()
        .find(|e| e.spec_id.eq_ignore_ascii_case(target) && !is_reversed(reversals, &e.id))
        .cloned()
        .ok_or_else(|| AuditError::NoSuchTarget(target.to_string()))
}

/// PURE: build the reversal row for a plan that was applied.
pub(crate) fn reversal_record(
    ts: &str,
    plan: &ReversalPlan,
    actor: &str,
    note: Option<&str>,
) -> ReversalRecord {
    ReversalRecord {
        schema: EXECUTION_SCHEMA,
        id: short_id('r', &format!("{ts}|{}|reversal", plan.target)),
        ts: ts.to_string(),
        kind: KIND_REVERSAL.to_string(),
        target: plan.target.clone(),
        spec_id: plan.spec_id.clone(),
        steps: plan.steps.iter().map(|s| s.describe()).collect(),
        complete: plan.complete,
        actor: actor.to_string(),
        note: note.map(|s| s.to_string()),
        extra: BTreeMap::new(),
    }
}

// ---------------------------------------------------------------------------
// The DURABLE trail: a structured comment on the spec itself
// ---------------------------------------------------------------------------
//
// `.aida/` is gitignored, per-clone runtime state by the deny-by-default
// convention — so the JSONL log below is a FAST INDEX, not the trail that
// "survives agent/vendor changes" (the TASK-0430 acceptance). The trail that
// survives is a comment on the spec: comments live in the spec's YAML, on the
// `aida-store` orphan branch, replicated and diffable, independent of which
// agent or vendor ran the disposition. Same two-layer shape AIDA already uses
// for the git-canonical store + rebuildable `.aida/cache.db`.
//
// The comment is BOTH human-readable (it renders in `aida show`) and exactly
// machine-recoverable: a prose line for the reader, then a marker plus the
// record's own JSON for [`parse_audit_comment`]. Anchoring recovery on the
// record's serde shape rather than re-parsing prose means the round-trip is
// lossless by construction and cannot drift as fields are added.
// trace:TASK-1018 trace:TASK-0430 | ai:claude

/// Anchors the machine-recoverable half of an autopilot audit comment.
pub(crate) const AUDIT_COMMENT_MARKER: &str = "<!-- aida:autopilot -->";

/// PURE: render the durable audit comment for one execution record.
pub(crate) fn audit_comment(rec: &ExecutionRecord) -> String {
    let restores = rec
        .prior
        .describe()
        .map(|d| format!(" · restores: {d}"))
        .unwrap_or_default();
    let reason = if rec.reason.trim().is_empty() {
        String::new()
    } else {
        format!(" — {}", rec.reason)
    };
    let payload = serde_json::to_string(rec).unwrap_or_default();
    format!(
        "autopilot: {} {}{reason}{restores} · grounding: {} · risk: {} · authority: {} \
         · actor: {} · source: {} · id: {} · {}\n{AUDIT_COMMENT_MARKER}{payload}",
        rec.action,
        rec.spec_id,
        rec.grounding,
        rec.risk,
        rec.authority,
        rec.actor,
        rec.source,
        rec.id,
        rec.ts,
    )
}

/// PURE: recover an execution record from a durable audit comment.
///
/// Tolerant by design: a comment without the marker, or with a payload a
/// hand-edit corrupted, yields `None` and is skipped — one bad comment never
/// blinds the reindex.
pub(crate) fn parse_audit_comment(body: &str) -> Option<ExecutionRecord> {
    let (_, tail) = body.split_once(AUDIT_COMMENT_MARKER)?;
    let payload = tail.lines().next()?.trim();
    let rec: ExecutionRecord = serde_json::from_str(payload).ok()?;
    (rec.kind == KIND_EXECUTION).then_some(rec)
}

/// PURE: the records present in the durable comments but MISSING from the fast
/// index — what a reindex must append.
///
/// Append-only by construction: reindex never rewrites the log (which also
/// carries projection and reversal rows), it only fills gaps. Idempotent — a
/// second run finds nothing to add.
pub(crate) fn reindex_missing(
    indexed: &[ExecutionRecord],
    recovered: &[ExecutionRecord],
) -> Vec<ExecutionRecord> {
    let known: std::collections::HashSet<&str> = indexed.iter().map(|e| e.id.as_str()).collect();
    let mut seen = std::collections::HashSet::new();
    recovered
        .iter()
        .filter(|r| !known.contains(r.id.as_str()) && seen.insert(r.id.clone()))
        .cloned()
        .collect()
}

/// Render the durable comment recording that an execution was REVERSED.
/// Reversals are themselves audited events — the trail must show not just what
/// autopilot did but what the human did about it.
pub(crate) fn reversal_comment(rev: &ReversalRecord) -> String {
    let note = rev
        .note
        .as_deref()
        .filter(|n| !n.trim().is_empty())
        .map(|n| format!(" — {n}"))
        .unwrap_or_default();
    let partial = if rev.complete {
        String::new()
    } else {
        " · partial: an appended note was retracted, not erased".to_string()
    };
    format!(
        "autopilot: reverted {} by {}{note} · restored: {}{partial} · id: {} · {}",
        rev.target,
        rev.actor,
        rev.steps.join("; "),
        rev.id,
        rev.ts,
    )
}

// ---------------------------------------------------------------------------
// Durable append / read (I/O, deliberately thin)
// ---------------------------------------------------------------------------

/// Append JSONL rows to the shared audit log, creating `.aida/` if needed.
/// Generic over the row type so `execution` and `reversal` share the one path.
fn append_rows<T: Serialize>(project_root: &Path, rows: &[T]) -> std::io::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let path = audit_log_path(project_root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    for row in rows {
        let line = serde_json::to_string(row)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        writeln!(file, "{line}")?;
    }
    Ok(())
}

/// Read the rows of one `type` out of the shared log. A missing file is an empty
/// log (not an error); a malformed or foreign-`type` line is skipped so one bad
/// row never blinds the whole trail.
fn read_rows<T: serde::de::DeserializeOwned>(
    project_root: &Path,
    kind: &str,
) -> std::io::Result<Vec<T>> {
    let path = audit_log_path(project_root);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some(kind))
        .filter_map(|v| serde_json::from_value::<T>(v).ok())
        .collect())
}

/// Durably append execution rows.
// trace:TASK-1018 | ai:claude
pub(crate) fn append_executions(
    project_root: &Path,
    rows: &[ExecutionRecord],
) -> std::io::Result<()> {
    append_rows(project_root, rows)
}

/// Durably append reversal rows.
// trace:TASK-1018 | ai:claude
pub(crate) fn append_reversals(
    project_root: &Path,
    rows: &[ReversalRecord],
) -> std::io::Result<()> {
    append_rows(project_root, rows)
}

/// Read every durable execution row, oldest first.
pub(crate) fn read_executions(project_root: &Path) -> std::io::Result<Vec<ExecutionRecord>> {
    read_rows(project_root, KIND_EXECUTION)
}

/// Read every reversal row, oldest first.
pub(crate) fn read_reversals(project_root: &Path) -> std::io::Result<Vec<ReversalRecord>> {
    read_rows(project_root, KIND_REVERSAL)
}

/// Mint + durably record one executed action in a single call — the surface a
/// producer wires in right after the side effect lands.
///
/// Writes BOTH layers: the durable spec comment (git-canonical, survives
/// agent/vendor changes) and the fast JSONL index. Strict on the MINT (an
/// unmintable record is a programming error worth surfacing) and loud but
/// non-fatal on either WRITE — the action has already landed, so failing here
/// would leave the caller unable to complete something that already happened.
/// The loudness matters: an unaudited execution is precisely the state this
/// machinery exists to prevent.
///
/// TASK-1014: the composition mode is resolved HERE, not threaded through the
/// producers — the solo posture is a runtime read, so it belongs next to the
/// clock, and deriving it in the one recording surface means a producer cannot
/// forget to record the supervision context its action ran under.
// trace:TASK-1018 trace:TASK-0430 trace:TASK-1014 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_execution(
    project_root: &Path,
    decision: &Decision,
    outcome: Outcome,
    authority: Authority,
    actor: &str,
    source: &str,
    prior: PriorState,
) -> Result<ExecutionRecord, AuditError> {
    let now = chrono::Utc::now();
    let ts = now.to_rfc3339();
    let solo = crate::presence::current_solo(now);
    let rec = execution_record(
        &ts, 0, decision, outcome, authority, actor, source, solo, prior,
    )?;
    if let Err(e) = write_durable_comment(project_root, &rec.spec_id, &audit_comment(&rec)) {
        eprintln!(
            "  warning: the durable audit comment could not be written ({e}) — \
             the local index still has the record, but it will not replicate."
        );
    }
    if let Err(e) = append_executions(project_root, std::slice::from_ref(&rec)) {
        eprintln!("  warning: could not write the autopilot audit index: {e}");
    }
    Ok(rec)
}

/// Append one audit comment to a spec through the same git-canonical path
/// `aida comment add` uses, so the trail lands on the orphan branch exactly the
/// way every other comment does.
fn write_durable_comment(project_root: &Path, spec_id: &str, body: &str) -> anyhow::Result<()> {
    let storage = crate::Storage::new(project_root.join(".aida-store"));
    crate::comment_cmd::add_comment_cli(&storage, spec_id, body, Some("autopilot"), None)
}

/// Recover every execution record from the durable spec comments — the
/// rebuild-from-source-of-truth path. Unparseable / non-autopilot comments are
/// skipped, never fatal.
// trace:TASK-1018 trace:TASK-0430 | ai:claude
pub(crate) fn recover_from_comments(project_root: &Path) -> anyhow::Result<Vec<ExecutionRecord>> {
    let store = crate::Storage::new(project_root.join(".aida-store")).load()?;
    let mut out: Vec<ExecutionRecord> = store
        .requirements
        .iter()
        .flat_map(|r| r.comments.iter())
        .filter_map(|c| parse_audit_comment(&c.content))
        .collect();
    out.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| a.id.cmp(&b.id)));
    Ok(out)
}

// ---------------------------------------------------------------------------
// Applying a reversal (I/O)
// ---------------------------------------------------------------------------

/// PURE: refuse a status reversal on a spec that has since SHIPPED.
///
/// Defence in depth against the stale-revert hazard: autopilot should never
/// have auto-executed something that could reach the default branch before a
/// human looked at it, but if state moved on anyway, walking a merged spec back
/// to Draft would rewrite shipped history rather than undo a decision. Returns
/// the refusal message, or `None` when the reversal may proceed.
// trace:TASK-1018 | ai:claude
pub(crate) fn revert_blocked_by_status(current_status: &str) -> Option<String> {
    let normalized = current_status.trim().to_ascii_lowercase().replace('_', "-");
    matches!(normalized.as_str(), "done" | "completed").then(|| {
        format!(
            "this spec has moved on since the action was taken (it is now {normalized}) — \
             reverting its status would rewrite shipped state. Undo it by hand if that is \
             really what you want."
        )
    })
}

/// Execute a [`ReversalPlan`] against the live store.
///
/// Routes every state change through the SAME in-process paths the CLI verbs use
/// (`edit_requirement_cli` for status/tags, `global_queue` for role queues), so
/// a reversal can never diverge from what `aida edit` / `aida queue` would do —
/// including their lifecycle guards. `dry_run` returns the step descriptions
/// without touching anything.
///
/// Returns the applied step descriptions in order.
// trace:TASK-1018 | ai:claude
pub(crate) fn apply_reversal(
    project_root: &Path,
    plan: &ReversalPlan,
    dry_run: bool,
) -> anyhow::Result<Vec<String>> {
    let mut applied = Vec::new();
    if dry_run {
        return Ok(plan.steps.iter().map(|s| s.describe()).collect());
    }
    let storage = crate::Storage::new(project_root.join(".aida-store"));
    for step in &plan.steps {
        match step {
            ReversalStep::SetStatus { spec_id, status } => {
                let current = storage.load().ok().and_then(|s| {
                    s.requirements
                        .iter()
                        .find(|r| crate::queue_cmd::spec_matches(r, spec_id))
                        .map(|r| format!("{:?}", r.status))
                });
                if let Some(msg) = current.as_deref().and_then(revert_blocked_by_status) {
                    anyhow::bail!("{spec_id}: {msg}");
                }
                crate::edit_requirement_cli(
                    &storage,
                    spec_id,
                    &None,
                    &None,
                    &Some(status.clone()),
                    &None,
                    &None,
                    &None,
                    &None,
                    &None,
                    &[],
                    &[],
                )?;
            }
            ReversalStep::AddTags { spec_id, tags } => {
                crate::edit_requirement_cli(
                    &storage,
                    spec_id,
                    &None,
                    &None,
                    &None,
                    &None,
                    &None,
                    &None,
                    &None,
                    &None,
                    tags,
                    &[],
                )?;
            }
            ReversalStep::RemoveTags { spec_id, tags } => {
                crate::edit_requirement_cli(
                    &storage,
                    spec_id,
                    &None,
                    &None,
                    &None,
                    &None,
                    &None,
                    &None,
                    &None,
                    &None,
                    &[],
                    tags,
                )?;
            }
            ReversalStep::SetDeferred { spec_id, deferred } => {
                // The `deferred:*` parking tag is the portable representation of
                // the flag (STORY-584 honors it in every view), so restoring it
                // needs no new write path.
                let tag = vec!["deferred:autopilot".to_string()];
                let (add, remove): (&[String], &[String]) =
                    if *deferred { (&tag, &[]) } else { (&[], &tag) };
                crate::edit_requirement_cli(
                    &storage, spec_id, &None, &None, &None, &None, &None, &None, &None, &None, add,
                    remove,
                )?;
            }
            ReversalStep::QueueRemove { spec_id, role } => {
                let store = storage.load()?;
                let req = store
                    .requirements
                    .iter()
                    .find(|r| crate::queue_cmd::spec_matches(r, spec_id))
                    .ok_or_else(|| anyhow::anyhow!("no requirement matches `{spec_id}`"))?;
                crate::global_queue::remove(role, &req.id, Some(project_root))?;
            }
            ReversalStep::QueueAdd { spec_id, role } => {
                let store = storage.load()?;
                let req = store
                    .requirements
                    .iter()
                    .find(|r| crate::queue_cmd::spec_matches(r, spec_id))
                    .ok_or_else(|| anyhow::anyhow!("no requirement matches `{spec_id}`"))?;
                let position = crate::global_queue::load(role)
                    .unwrap_or_default()
                    .iter()
                    .map(|e| e.position)
                    .max()
                    .unwrap_or(0)
                    + 1;
                crate::global_queue::add(
                    role,
                    crate::global_queue::GlobalQueueEntry {
                        requirement_id: req.id,
                        project_root: project_root.to_path_buf(),
                        project_name: crate::global_queue::project_name_for(project_root),
                        spec_id: req.spec_id.clone(),
                        agreed_id: req.agreed_id.clone(),
                        title: Some(req.title.clone()),
                        position,
                        added_by: "autopilot-reversal".to_string(),
                        added_at: chrono::Utc::now(),
                        note: Some("restored by an autopilot reversal".to_string()),
                        for_role: role.clone(),
                    },
                )?;
            }
            ReversalStep::Retract { spec_id, marker } => {
                // Append-only substrate: the artifact stays, the retraction is
                // recorded next to it so the trail reads true.
                crate::comment_cmd::add_comment_cli(
                    &storage,
                    spec_id,
                    &format!(
                        "Retracted by an autopilot reversal: the earlier automated \
                         note ({marker}) no longer reflects the disposition."
                    ),
                    None,
                    None,
                )?;
            }
        }
        applied.push(step.describe());
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autopilot::{ActionClass, EscalateReason, Grounding};
    use crate::backlog::RiskLevel;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aida-ap-exec-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn decision(spec: &str, action: ActionClass) -> Decision {
        Decision {
            spec_id: spec.to_string(),
            action,
            grounding: Grounding::RecordedB,
            risk: RiskLevel::Low,
            reason: "operator asked for it".to_string(),
            evidence: vec!["aida zen <draft> invocation".to_string()],
        }
    }

    // ---- record shape ------------------------------------------------------

    #[test]
    fn execution_record_captures_action_authority_and_prior_state() {
        let d = decision("TASK-1", ActionClass::Approve);
        let rec = execution_record(
            "2026-07-20T00:00:00Z",
            0,
            &d,
            Outcome::Execute,
            Authority::Auto,
            "joe",
            "zen",
            false,
            PriorState::from_status("draft"),
        )
        .expect("an executed, reversible, prior-state-bearing action mints");

        assert_eq!(rec.schema, EXECUTION_SCHEMA);
        assert!(rec.id.starts_with('x'));
        assert_eq!(rec.kind, KIND_EXECUTION);
        assert_eq!(rec.spec_id, "TASK-1");
        assert_eq!(rec.action, "approve");
        assert_eq!(rec.verdict, "execute");
        assert_eq!(rec.gate, "all-gates-pass");
        assert_eq!(rec.authority, "auto");
        assert_eq!(rec.grounding, "recorded-b");
        assert_eq!(rec.risk, "low");
        assert_eq!(rec.actor, "joe");
        assert_eq!(rec.source, "zen");
        assert_eq!(rec.prior.status.as_deref(), Some("draft"));
        // Evidence rides across from the decision (the TASK-1019 extension
        // point); mode is derived from the surface + posture (TASK-1014);
        // from_product stays absent with no product handoff in the evidence.
        assert_eq!(
            rec.evidence,
            vec!["aida zen <draft> invocation".to_string()]
        );
        assert_eq!(rec.mode.as_deref(), Some("zen+autopilot"));
        assert_eq!(rec.from_product, None);
    }

    #[test]
    fn execution_record_refuses_non_execute_outcomes() {
        // A held or escalated decision belongs in the PROJECTION log; minting a
        // durable "this happened" row for it would be a lie.
        let d = decision("TASK-1", ActionClass::Approve);
        for outcome in [
            Outcome::Hold,
            Outcome::Escalate(EscalateReason::RiskCeiling),
            Outcome::Escalate(EscalateReason::GroundingGap),
            Outcome::Escalate(EscalateReason::NeverAuthority),
        ] {
            let err = execution_record(
                "2026-07-20T00:00:00Z",
                0,
                &d,
                outcome,
                Authority::Auto,
                "joe",
                "zen",
                false,
                PriorState::from_status("draft"),
            )
            .unwrap_err();
            assert!(matches!(err, AuditError::NotExecuted(_)), "{outcome:?}");
        }
    }

    #[test]
    fn execution_record_refuses_empty_prior_state() {
        // No prior state = no reversal. Caught at mint, not at revert time.
        let d = decision("TASK-1", ActionClass::Tag);
        let err = execution_record(
            "2026-07-20T00:00:00Z",
            0,
            &d,
            Outcome::Execute,
            Authority::Auto,
            "joe",
            "zen",
            false,
            PriorState::default(),
        )
        .unwrap_err();
        assert!(matches!(err, AuditError::NothingToReverse(_)));
    }

    #[test]
    fn execution_record_ids_are_unique_within_a_batch() {
        let d1 = decision("TASK-1", ActionClass::Approve);
        let d2 = decision("TASK-2", ActionClass::Approve);
        let mint = |seq, d: &Decision| {
            execution_record(
                "2026-07-20T00:00:00Z",
                seq,
                d,
                Outcome::Execute,
                Authority::Auto,
                "joe",
                "zen",
                false,
                PriorState::from_status("draft"),
            )
            .unwrap()
        };
        assert_ne!(mint(0, &d1).id, mint(1, &d2).id);
        // Same seed → same id (stable, content-addressed).
        assert_eq!(mint(0, &d1).id, mint(0, &d1).id);
    }

    #[test]
    fn execution_record_tolerates_additive_fields_from_a_newer_binary() {
        // Forward-compat contract for TASK-1013/1014/1019/1022: a row written by
        // a newer binary round-trips through this one WITHOUT losing the fields
        // it doesn't know about, and the reserved fields deserialize by name.
        let json = r#"{"schema":1,"id":"xdeadbeef","ts":"2026-07-20T00:00:00Z","type":"execution",
            "spec_id":"TASK-1","action":"approve","verdict":"execute","gate":"all-gates-pass",
            "authority":"auto","grounding":"type-a","risk":"low","reason":"r","actor":"joe",
            "source":"groom","prior":{"status":"draft"},"evidence":["PRIN-3"],
            "mode":"solo+autopilot","from_product":true,"future_field":{"nested":42}}"#;
        let rec: ExecutionRecord = serde_json::from_str(json).unwrap();
        assert_eq!(rec.mode.as_deref(), Some("solo+autopilot"));
        assert_eq!(rec.from_product, Some(true));
        assert_eq!(rec.evidence, vec!["PRIN-3".to_string()]);
        assert!(rec.extra.contains_key("future_field"));

        let round: ExecutionRecord = serde_json::from_str(&serde_json::to_string(&rec).unwrap())
            .expect("re-serialized row still parses");
        assert_eq!(round, rec);
        assert!(serde_json::to_string(&rec)
            .unwrap()
            .contains("future_field"));
    }

    // ---- product-sourced evidence + the --from-product filter --------------

    /// A decision whose evidence cites a product handoff.
    fn product_decision(spec: &str, marker: &str) -> Decision {
        Decision {
            evidence: vec!["PRIN-3".to_string(), marker.to_string()],
            ..decision(spec, ActionClass::Approve)
        }
    }

    fn mint(d: &Decision) -> ExecutionRecord {
        execution_record(
            "2026-07-20T00:00:00Z",
            0,
            d,
            Outcome::Execute,
            Authority::Auto,
            "joe",
            "groom",
            false,
            PriorState::from_status("draft"),
        )
        .expect("a reversible executed action with prior state mints")
    }

    #[test]
    fn product_evidence_marker_sets_from_product_at_mint() {
        // The whole point of deriving rather than passing: the moment a producer
        // cites a product handoff, the record is filterable — no second write.
        let rec = mint(&product_decision("TASK-1", "product:pat"));
        assert_eq!(rec.from_product, Some(true));
        assert!(is_from_product(&rec));
        assert_eq!(product_provenance(&rec.evidence).as_deref(), Some("pat"));
    }

    #[test]
    fn non_product_evidence_leaves_from_product_absent() {
        // Absent, not `Some(false)` — a row says nothing about product input
        // unless there WAS product input, so non-product rows stay byte-identical
        // to the ones minted before this filter existed.
        let rec = mint(&decision("TASK-1", ActionClass::Approve));
        assert_eq!(rec.from_product, None);
        assert!(!is_from_product(&rec));
        assert!(!serde_json::to_string(&rec)
            .unwrap()
            .contains("from_product"));
    }

    #[test]
    fn product_marker_is_case_insensitive_and_tolerates_whitespace() {
        // A hand-written marker must not fall out of the audit on capitalization.
        for marker in ["Product:Pat", "  PRODUCT: pat  ", "product:pat"] {
            let rec = mint(&product_decision("TASK-1", marker));
            assert!(is_from_product(&rec), "{marker}");
        }
    }

    #[test]
    fn unnamed_product_marker_still_counts_as_a_handoff() {
        // The handoff happened even if the seat did not name itself; the filter
        // must not lose the row just because provenance is anonymous.
        let rec = mint(&product_decision("TASK-1", "product:"));
        assert!(is_from_product(&rec));
        assert_eq!(product_provenance(&rec.evidence), None);
        assert_eq!(product_annotation(&rec).as_deref(), Some("product handoff"));
    }

    #[test]
    fn product_lookalike_evidence_is_not_a_handoff() {
        // Only the `product:` PREFIX marks a handoff — prose that merely mentions
        // the product seat must never light up the provenance filter.
        for entry in [
            "the product roadmap says so",
            "ADR-4 product positioning",
            "productivity:pat",
        ] {
            let rec = mint(&product_decision("TASK-1", entry));
            assert!(!is_from_product(&rec), "{entry}");
        }
    }

    #[test]
    fn from_product_filter_reads_the_flag_before_the_evidence() {
        // Three row shapes must all filter correctly: flag-only (a newer binary
        // set it directly), evidence-only (a producer that only tagged), and an
        // explicit false (a deliberate "not product-sourced" wins over a loose
        // marker-shaped string).
        let mut flag_only = mint(&decision("TASK-1", ActionClass::Approve));
        flag_only.from_product = Some(true);
        assert!(is_from_product(&flag_only));

        let mut evidence_only = mint(&product_decision("TASK-2", "product:pat"));
        evidence_only.from_product = None;
        assert!(is_from_product(&evidence_only));

        let mut explicit_false = mint(&product_decision("TASK-3", "product:pat"));
        explicit_false.from_product = Some(false);
        assert!(!is_from_product(&explicit_false));
    }

    #[test]
    fn from_product_filter_selects_only_product_sourced_rows() {
        // The filter as the CLI applies it, over a mixed trail.
        let rows = vec![
            mint(&decision("TASK-1", ActionClass::Approve)),
            mint(&product_decision("TASK-2", "product:pat")),
            mint(&decision("TASK-3", ActionClass::Tag)),
            mint(&product_decision("TASK-4", "product:sam")),
        ];
        let selected: Vec<&str> = rows
            .iter()
            .filter(|r| is_from_product(r))
            .map(|r| r.spec_id.as_str())
            .collect();
        assert_eq!(selected, vec!["TASK-2", "TASK-4"]);
    }

    #[test]
    fn product_annotation_names_the_seat_and_stays_none_otherwise() {
        assert_eq!(
            product_annotation(&mint(&product_decision("TASK-1", "product:pat"))).as_deref(),
            Some("product handoff: pat")
        );
        assert_eq!(
            product_annotation(&mint(&decision("TASK-1", ActionClass::Approve))),
            None
        );
    }

    #[test]
    fn product_provenance_survives_the_durable_comment_round_trip() {
        // The filter must still be truthful after a reindex from the durable
        // comments — the trail that outlives `.aida/`.
        let rec = mint(&product_decision("TASK-1", "product:pat"));
        let recovered =
            parse_audit_comment(&audit_comment(&rec)).expect("the comment round-trips losslessly");
        assert_eq!(recovered, rec);
        assert!(is_from_product(&recovered));
        assert_eq!(
            product_provenance(&recovered.evidence).as_deref(),
            Some("pat")
        );
    }

    // ---- composition mode + the --mode filter ------------------------------

    /// Mint under a named surface and solo posture — the two inputs the
    /// composition mode is derived from.
    fn mint_under(source: &str, solo: bool) -> ExecutionRecord {
        execution_record(
            "2026-07-20T00:00:00Z",
            0,
            &decision("TASK-1", ActionClass::Approve),
            Outcome::Execute,
            Authority::Auto,
            "joe",
            source,
            solo,
            PriorState::from_status("draft"),
        )
        .expect("a reversible executed action with prior state mints")
    }

    #[test]
    fn mode_records_the_three_supervision_levels_at_mint() {
        // The vocabulary the trail is read with: autopilot alone, autopilot
        // under an operator-invoked one-shot, autopilot under a solo posture.
        assert_eq!(
            mint_under("groom", false).mode.as_deref(),
            Some("autopilot")
        );
        assert_eq!(
            mint_under("zen", false).mode.as_deref(),
            Some("zen+autopilot")
        );
        assert_eq!(
            mint_under("groom", true).mode.as_deref(),
            Some("solo+autopilot")
        );
    }

    #[test]
    fn mode_keeps_both_layers_when_a_zen_one_shot_runs_under_solo() {
        // Layers are additive: dropping one to force the token into a fixed set
        // of three would make the record less true than the run it describes.
        assert_eq!(
            mint_under("zen", true).mode.as_deref(),
            Some("solo+zen+autopilot")
        );
    }

    #[test]
    fn unnamed_surfaces_record_the_bare_envelope() {
        // Every other surface IS the bare envelope — `source` already names it,
        // so the mode must not invent a layer for it.
        for source in ["", "inspect", "groom", "something-new"] {
            assert_eq!(
                mint_under(source, false).mode.as_deref(),
                Some("autopilot"),
                "{source}"
            );
        }
    }

    #[test]
    fn zen_surface_matches_case_insensitively() {
        assert_eq!(
            mint_under("ZEN", false).mode.as_deref(),
            Some("zen+autopilot")
        );
        assert_eq!(
            mint_under("  Zen ", false).mode.as_deref(),
            Some("zen+autopilot")
        );
    }

    #[test]
    fn record_mode_falls_back_to_the_surface_on_a_pre_mode_row() {
        // A row minted before the field existed still reports a truthful
        // SURFACE composition. The solo posture is unrecoverable after the fact,
        // so the fallback under-claims composition rather than guessing at it.
        let mut old = mint_under("zen", true);
        old.mode = None;
        assert_eq!(record_mode(&old), "zen+autopilot");

        let mut blank = mint_under("groom", false);
        blank.mode = Some("   ".to_string());
        assert_eq!(record_mode(&blank), "autopilot");
    }

    #[test]
    fn mode_filter_matches_a_whole_token_or_a_single_layer() {
        let zen_solo = mint_under("zen", true);
        // The whole token, exactly.
        assert!(mode_matches(&zen_solo, "solo+zen+autopilot"));
        assert!(mode_matches(&zen_solo, "SOLO+ZEN+AUTOPILOT"));
        assert!(!mode_matches(&zen_solo, "zen+autopilot"));
        // Either layer, broadly — an operator can ask "anything solo-composed?"
        // without knowing what else was composed over it.
        assert!(mode_matches(&zen_solo, "solo"));
        assert!(mode_matches(&zen_solo, "zen"));
        assert!(!mode_matches(&zen_solo, "groom"));
    }

    #[test]
    fn mode_filter_reads_bare_autopilot_as_exact_not_as_everything() {
        // Every mode ends in the envelope, so a layer match would silently turn
        // `--mode autopilot` — "what did autopilot do entirely on its own?" —
        // into "show me everything".
        assert!(mode_matches(&mint_under("groom", false), "autopilot"));
        assert!(!mode_matches(&mint_under("zen", false), "autopilot"));
        assert!(!mode_matches(&mint_under("groom", true), "autopilot"));
    }

    #[test]
    fn an_empty_mode_filter_narrows_nothing() {
        for rec in [mint_under("groom", false), mint_under("zen", true)] {
            assert!(mode_matches(&rec, ""));
            assert!(mode_matches(&rec, "  "));
        }
    }

    #[test]
    fn mode_filter_selects_only_the_matching_rows() {
        // The filter as the CLI applies it, over a mixed trail.
        let rows = vec![
            mint_under("groom", false),
            mint_under("zen", false),
            mint_under("groom", true),
        ];
        let modes: Vec<String> = rows
            .iter()
            .filter(|r| mode_matches(r, "solo"))
            .map(record_mode)
            .collect();
        assert_eq!(modes, vec!["solo+autopilot".to_string()]);
    }

    #[test]
    fn mode_annotation_stays_quiet_for_the_bare_envelope() {
        // Annotating the default on every row would bury the rows where
        // something else was steering.
        assert_eq!(mode_annotation(&mint_under("groom", false)), None);
        assert_eq!(
            mode_annotation(&mint_under("zen", false)).as_deref(),
            Some("mode: zen+autopilot")
        );
        assert_eq!(
            mode_annotation(&mint_under("groom", true)).as_deref(),
            Some("mode: solo+autopilot")
        );
    }

    #[test]
    fn mode_survives_the_durable_comment_round_trip() {
        // The supervision context must still be readable after a reindex from
        // the durable comments — the trail that outlives `.aida/`.
        let rec = mint_under("zen", true);
        let recovered =
            parse_audit_comment(&audit_comment(&rec)).expect("the comment round-trips losslessly");
        assert_eq!(recovered, rec);
        assert_eq!(record_mode(&recovered), "solo+zen+autopilot");
        assert!(mode_matches(&recovered, "solo"));
    }

    // ---- durable append ----------------------------------------------------

    #[test]
    fn audit_append_and_read_executions_roundtrip() {
        let dir = temp_root("append");
        assert!(read_executions(&dir).unwrap().is_empty());

        let d = decision("TASK-7", ActionClass::Queue);
        let rec = execution_record(
            "2026-07-20T00:00:00Z",
            0,
            &d,
            Outcome::Execute,
            Authority::Auto,
            "joe",
            "groom",
            false,
            PriorState {
                queued_to_role: Some("implementer".to_string()),
                ..PriorState::default()
            },
        )
        .unwrap();
        append_executions(&dir, std::slice::from_ref(&rec)).unwrap();
        // Appending is additive, never a rewrite.
        let d2 = decision("TASK-8", ActionClass::Approve);
        let rec2 = execution_record(
            "2026-07-20T00:01:00Z",
            0,
            &d2,
            Outcome::Execute,
            Authority::Auto,
            "joe",
            "groom",
            false,
            PriorState::from_status("draft"),
        )
        .unwrap();
        append_executions(&dir, std::slice::from_ref(&rec2)).unwrap();

        let read = read_executions(&dir).unwrap();
        assert_eq!(read, vec![rec, rec2]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn execution_rows_and_projection_rows_share_the_log_without_cross_talk() {
        // One file, four `type` discriminators. The TASK-1147 projection reader
        // must not see execution rows and vice versa.
        let dir = temp_root("mixed");
        let projection = crate::autopilot::decision_entry(
            "2026-07-20T00:00:00Z",
            0,
            "TASK-1",
            ActionClass::Approve,
            Outcome::Hold,
            "held",
            "inspect",
        );
        crate::autopilot::append_audit_entries(&dir, std::slice::from_ref(&projection)).unwrap();

        let d = decision("TASK-2", ActionClass::Approve);
        let exec = execution_record(
            "2026-07-20T00:01:00Z",
            0,
            &d,
            Outcome::Execute,
            Authority::Auto,
            "joe",
            "zen",
            false,
            PriorState::from_status("draft"),
        )
        .unwrap();
        append_executions(&dir, std::slice::from_ref(&exec)).unwrap();

        let projections = crate::autopilot::read_audit_entries(&dir).unwrap();
        assert_eq!(projections, vec![projection]);
        assert_eq!(read_executions(&dir).unwrap(), vec![exec]);
        assert!(read_reversals(&dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_line_does_not_blind_the_trail() {
        let dir = temp_root("malformed");
        let d = decision("TASK-3", ActionClass::Approve);
        let exec = execution_record(
            "2026-07-20T00:00:00Z",
            0,
            &d,
            Outcome::Execute,
            Authority::Auto,
            "joe",
            "zen",
            false,
            PriorState::from_status("draft"),
        )
        .unwrap();
        append_executions(&dir, std::slice::from_ref(&exec)).unwrap();
        let path = audit_log_path(&dir);
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, "{{not json").unwrap();
        drop(f);
        append_executions(&dir, std::slice::from_ref(&exec)).unwrap();

        assert_eq!(read_executions(&dir).unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- the durable trail (spec comments) ---------------------------------

    #[test]
    fn audit_comment_round_trips_through_parse() {
        // The durability contract: the comment on the spec — which is what
        // survives a lost `.aida/`, a different machine, a different vendor —
        // reconstructs the record EXACTLY.
        let d = decision("TASK-4", ActionClass::Approve);
        let rec = execution_record(
            "2026-07-20T00:00:00Z",
            0,
            &d,
            Outcome::Execute,
            Authority::Auto,
            "joe",
            "zen",
            false,
            PriorState::from_status("draft"),
        )
        .unwrap();
        let body = audit_comment(&rec);
        // Human-readable half.
        assert!(body.starts_with("autopilot: approve TASK-4"));
        assert!(body.contains("restores: status=draft"));
        assert!(body.contains(&format!("id: {}", rec.id)));
        // Machine-recoverable half.
        assert_eq!(parse_audit_comment(&body), Some(rec));
    }

    #[test]
    fn parse_audit_comment_skips_unparseable_and_foreign_comments() {
        // Tolerance: a hand-edited or plain human comment is skipped, never a
        // panic and never a bogus record.
        assert_eq!(parse_audit_comment("just a normal human comment"), None);
        assert_eq!(
            parse_audit_comment(&format!(
                "autopilot: approve X\n{AUDIT_COMMENT_MARKER}{{oops"
            )),
            None
        );
        // A well-formed row of the WRONG kind is not an execution.
        let not_execution = format!(
            "autopilot: x\n{AUDIT_COMMENT_MARKER}{}",
            r#"{"schema":1,"id":"r1","ts":"t","type":"reversal","spec_id":"S","action":"approve",
               "verdict":"execute","gate":"g","authority":"auto","grounding":"type-a","risk":"low",
               "reason":"","actor":"a","source":"s","prior":{"status":"draft"}}"#
                .replace('\n', "")
        );
        assert_eq!(parse_audit_comment(&not_execution), None);
    }

    #[test]
    fn reindex_recovers_only_the_rows_the_index_is_missing() {
        let a = record_with(PriorState::from_status("draft"), ActionClass::Approve);
        let mut b = record_with(PriorState::from_status("approved"), ActionClass::Reject);
        b.id = "xb0000001".to_string();

        // Everything missing → everything recovered.
        let missing = reindex_missing(&[], &[a.clone(), b.clone()]);
        assert_eq!(missing, vec![a.clone(), b.clone()]);
        // Partially indexed → only the gap.
        assert_eq!(
            reindex_missing(std::slice::from_ref(&a), &[a.clone(), b.clone()]),
            vec![b.clone()]
        );
        // Idempotent: a second pass finds nothing.
        assert!(reindex_missing(&[a.clone(), b.clone()], &[a.clone(), b.clone()]).is_empty());
        // Duplicate comments (same record recovered twice) collapse.
        assert_eq!(reindex_missing(&[], &[a.clone(), a.clone()]), vec![a]);
    }

    #[test]
    fn reversal_comment_records_who_undid_what() {
        // A reversal is itself an audited event on the durable trail.
        let rec = record_with(PriorState::from_status("draft"), ActionClass::Approve);
        let plan = plan_reversal(&rec).unwrap();
        let rev = reversal_record("2026-07-20T01:00:00Z", &plan, "joe", Some("premature"));
        let body = reversal_comment(&rev);
        assert!(body.contains(&format!("reverted {} by joe", rec.id)));
        assert!(body.contains("premature"));
        assert!(body.contains("TASK-5: status → draft"));
        assert!(!body.contains("partial"));

        // A partial (append-only) reversal says so.
        let partial_rec = record_with(
            PriorState {
                appended_marker: Some("note".to_string()),
                ..PriorState::default()
            },
            ActionClass::Comment,
        );
        let partial_plan = plan_reversal(&partial_rec).unwrap();
        let partial = reversal_record("2026-07-20T01:00:00Z", &partial_plan, "joe", None);
        assert!(reversal_comment(&partial).contains("partial"));
    }

    // ---- reversal ----------------------------------------------------------

    fn record_with(prior: PriorState, action: ActionClass) -> ExecutionRecord {
        execution_record(
            "2026-07-20T00:00:00Z",
            0,
            &decision("TASK-5", action),
            Outcome::Execute,
            Authority::Auto,
            "joe",
            "groom",
            false,
            prior,
        )
        .unwrap()
    }

    #[test]
    fn plan_reversal_restores_status_for_an_approve() {
        let rec = record_with(PriorState::from_status("draft"), ActionClass::Approve);
        let plan = plan_reversal(&rec).unwrap();
        assert_eq!(plan.target, rec.id);
        assert!(plan.complete);
        assert_eq!(
            plan.steps,
            vec![ReversalStep::SetStatus {
                spec_id: "TASK-5".to_string(),
                status: "draft".to_string(),
            }]
        );
        assert_eq!(plan.steps[0].describe(), "TASK-5: status → draft");
    }

    #[test]
    fn plan_reversal_inverts_tag_and_queue_deltas() {
        let rec = record_with(
            PriorState {
                tags_added: vec!["duplicate-of:TASK-9".to_string()],
                tags_removed: vec!["needs-triage".to_string()],
                queued_to_role: Some("implementer".to_string()),
                dequeued_from_role: Some("advisor".to_string()),
                ..PriorState::default()
            },
            ActionClass::Route,
        );
        let plan = plan_reversal(&rec).unwrap();
        assert!(plan.complete);
        assert_eq!(
            plan.steps,
            vec![
                ReversalStep::RemoveTags {
                    spec_id: "TASK-5".to_string(),
                    tags: vec!["duplicate-of:TASK-9".to_string()],
                },
                ReversalStep::AddTags {
                    spec_id: "TASK-5".to_string(),
                    tags: vec!["needs-triage".to_string()],
                },
                ReversalStep::QueueRemove {
                    spec_id: "TASK-5".to_string(),
                    role: "implementer".to_string(),
                },
                ReversalStep::QueueAdd {
                    spec_id: "TASK-5".to_string(),
                    role: "advisor".to_string(),
                },
            ]
        );
    }

    #[test]
    fn plan_reversal_marks_append_only_artifacts_incomplete() {
        // A comment cannot be deleted from an append-only substrate; the plan
        // says so rather than claiming a total undo.
        let rec = record_with(
            PriorState {
                appended_marker: Some("autopilot dedupe note".to_string()),
                ..PriorState::default()
            },
            ActionClass::Comment,
        );
        let plan = plan_reversal(&rec).unwrap();
        assert!(!plan.complete);
        assert!(matches!(plan.steps[0], ReversalStep::Retract { .. }));
    }

    #[test]
    fn plan_reversal_orders_state_before_queue_membership() {
        let rec = record_with(
            PriorState {
                status: Some("draft".to_string()),
                deferred: Some(true),
                queued_to_role: Some("implementer".to_string()),
                ..PriorState::default()
            },
            ActionClass::Queue,
        );
        let plan = plan_reversal(&rec).unwrap();
        assert!(matches!(plan.steps[0], ReversalStep::SetStatus { .. }));
        assert!(matches!(plan.steps[1], ReversalStep::SetDeferred { .. }));
        assert!(matches!(plan.steps[2], ReversalStep::QueueRemove { .. }));
    }

    #[test]
    fn plan_reversal_refuses_a_row_that_did_not_execute() {
        // Defence in depth: a hand-edited / foreign row claiming a non-execute
        // verdict still cannot be "reversed".
        let mut rec = record_with(PriorState::from_status("draft"), ActionClass::Approve);
        rec.verdict = "hold".to_string();
        assert!(matches!(
            plan_reversal(&rec).unwrap_err(),
            AuditError::NotExecuted(_)
        ));
    }

    #[test]
    fn plan_reversal_is_derived_from_the_record_alone() {
        // A reversal must work from a bare JSON line months later — no store,
        // no config, no clock. Round-trip through JSON and re-plan.
        let rec = record_with(PriorState::from_status("draft"), ActionClass::Approve);
        let reparsed: ExecutionRecord =
            serde_json::from_str(&serde_json::to_string(&rec).unwrap()).unwrap();
        assert_eq!(
            plan_reversal(&reparsed).unwrap(),
            plan_reversal(&rec).unwrap()
        );
    }

    #[test]
    fn revert_target_resolves_by_id_and_by_spec_and_refuses_a_double_reversal() {
        let exec_a = record_with(PriorState::from_status("draft"), ActionClass::Approve);
        let mut exec_b = record_with(
            PriorState {
                tags_added: vec!["t".to_string()],
                ..PriorState::default()
            },
            ActionClass::Tag,
        );
        exec_b.id = "xfeedface".to_string();
        exec_b.ts = "2026-07-20T00:05:00Z".to_string();
        let execs = vec![exec_a.clone(), exec_b.clone()];

        // Exact id wins.
        assert_eq!(
            resolve_revert_target(&execs, &[], &exec_a.id).unwrap().id,
            exec_a.id
        );
        // SPEC-ID (case-insensitive) resolves to the LATEST un-reversed row.
        assert_eq!(
            resolve_revert_target(&execs, &[], "task-5").unwrap().id,
            exec_b.id
        );
        // Unknown target.
        assert!(matches!(
            resolve_revert_target(&execs, &[], "TASK-404").unwrap_err(),
            AuditError::NoSuchTarget(_)
        ));

        // Once reversed: the id form says so explicitly, the spec form falls
        // back to the next un-reversed row.
        let plan = plan_reversal(&exec_b).unwrap();
        let rev = reversal_record("2026-07-20T00:06:00Z", &plan, "joe", Some("wrong call"));
        assert!(is_reversed(std::slice::from_ref(&rev), &exec_b.id));
        assert!(matches!(
            resolve_revert_target(&execs, std::slice::from_ref(&rev), &exec_b.id).unwrap_err(),
            AuditError::AlreadyReversed(_)
        ));
        assert_eq!(
            resolve_revert_target(&execs, std::slice::from_ref(&rev), "TASK-5")
                .unwrap()
                .id,
            exec_a.id
        );
    }

    #[test]
    fn reversal_record_is_durable_and_readable() {
        let dir = temp_root("reversal");
        let rec = record_with(PriorState::from_status("draft"), ActionClass::Approve);
        append_executions(&dir, std::slice::from_ref(&rec)).unwrap();

        let plan = plan_reversal(&rec).unwrap();
        let rev = reversal_record("2026-07-20T00:10:00Z", &plan, "joe", Some("premature"));
        append_reversals(&dir, std::slice::from_ref(&rev)).unwrap();

        let read = read_reversals(&dir).unwrap();
        assert_eq!(read, vec![rev.clone()]);
        assert!(read[0].id.starts_with('r'));
        assert_eq!(read[0].target, rec.id);
        assert_eq!(read[0].steps, vec!["TASK-5: status → draft".to_string()]);
        assert!(read[0].complete);
        // The execution row is untouched by the reversal — append-only.
        assert_eq!(read_executions(&dir).unwrap(), vec![rec.clone()]);
        // And the recorded reversal makes the row un-revertable a second time.
        assert!(matches!(
            resolve_revert_target(&[rec], &read, "TASK-5").unwrap_err(),
            AuditError::NoSuchTarget(_)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn revert_refuses_a_spec_that_has_since_shipped() {
        // Stale-revert guard: walking a merged spec back to Draft would rewrite
        // shipped state, not undo a decision.
        for shipped in ["Done", "done", "Completed", "completed"] {
            assert!(
                revert_blocked_by_status(shipped).is_some(),
                "{shipped} should block"
            );
        }
        for live in [
            "Draft",
            "Approved",
            "InProgress",
            "NeedsAttention",
            "Rejected",
        ] {
            assert_eq!(
                revert_blocked_by_status(live),
                None,
                "{live} should proceed"
            );
        }
    }

    #[test]
    fn apply_reversal_dry_run_touches_nothing_and_previews_every_step() {
        let dir = temp_root("dryrun");
        let rec = record_with(
            PriorState {
                status: Some("draft".to_string()),
                tags_added: vec!["auto".to_string()],
                ..PriorState::default()
            },
            ActionClass::Approve,
        );
        let plan = plan_reversal(&rec).unwrap();
        let lines = apply_reversal(&dir, &plan, true).unwrap();
        assert_eq!(
            lines,
            vec![
                "TASK-5: status → draft".to_string(),
                "TASK-5: remove tag(s) auto".to_string(),
            ]
        );
        // No store was created, no row was written.
        assert!(!dir.join(".aida-store").exists());
        assert!(read_reversals(&dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
