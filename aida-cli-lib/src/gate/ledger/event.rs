//! The gate ledger's event schema: one JSON object per line.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// mandate-gate

use super::ChangeKey;
use crate::gate::canon::GateSpec;
use crate::gate::merge_hold_core::MergeHoldRecord;
use crate::gate::review_verdict::{JsonObj, RecorderAttestation, ATTESTATION_KEY};
use std::path::Path;

/// Schema version written on every event.
pub(crate) const EVENT_VERSION: u32 = 1;

/// One ledger line. `seq` and `at` are assigned by the appender under the
/// change's lock; a caller's values are overwritten.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct GateEvent {
    pub v: u32,
    pub seq: u64,
    pub at: String,
    /// `PR-<N>` / `MR-<N>`.
    pub change: String,
    /// The full head SHA the event was recorded at.
    #[serde(default)]
    pub head: Option<String>,
    /// `git patch-id --verbatim` of the change's own commits against their
    /// merge base with the protected base; `None` when it cannot be computed.
    #[serde(default)]
    pub patch_id: Option<String>,
    /// The protected base branch the patch identity was taken against.
    #[serde(default)]
    pub base_ref: Option<String>,
    /// That branch's tip when the identity was taken.
    #[serde(default)]
    pub base_tip: Option<String>,
    /// Index only: the spec the event concerns, as the writer spelled it
    /// (upper-cased). Today's readers index by this label.
    #[serde(default)]
    pub spec: Option<String>,
    /// STORY-2324 PR-B: the canonical key `spec` resolved to when the event
    /// was written (or `aida gate migrate` stamped it). `None` when the label
    /// named no requirement. Readers decide by it from PR-C on.
    // trace:STORY-2324 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_key: Option<crate::gate::canon::SpecKey>,
    /// The recorder's attestation, as the verdict record carries it.
    #[serde(default)]
    pub actor: Option<RecorderAttestation>,
    /// Advisory classifier evidence only; never review or shipping authority.
    /// Keep data untyped here: malformed/future advisory metadata must not
    /// prevent the authority-bearing verdict event from being read.
    // trace:STORY-2698 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk: Option<serde_json::Value>,
    #[serde(flatten)]
    pub body: EventBody,
}

// trace:BUG-2517 | ai:codex
impl GateEvent {
    /// Explicit local reviews never acquire change authority through later binding.
    pub(crate) fn is_spec_only(&self) -> bool {
        self.verdict_record()
            .is_some_and(|r| r.get("change_binding").and_then(|v| v.as_str()) == Some("unbound"))
    }
}

/// What happened. Slice 1 carried verdicts; slice 2 adds holds, their
/// releases and label syncs; slice 3 adds carry-over proofs.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum EventBody {
    /// Invocation evidence only. Never deserialized into merge authorization.
    // trace:BUG-b6b1278890774ef993d2ebc327862470-179 | ai:codex
    TrainCoverage { proof: serde_json::Value },
    /// A review verdict: the full record exactly as the verdict writer built
    /// it, so `parse_recorded_verdict` reads it unchanged.
    Verdict { record: JsonObj },
    /// The change's legacy files, verbatim, captured on its first ledger
    /// write so the ledger alone carries the history.
    LegacyImported { legacy: Vec<LegacyFile> },
    /// A merge hold placed (or re-composed, re-armed, re-routed): the full
    /// record as it now stands. The newest `hold` after the newest
    /// `hold-release` is the open hold.
    Hold {
        hold: MergeHoldRecord,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        rearmed: bool,
    },
    /// The open hold was released. Appended, never a deletion: the `hold`
    /// it answers stays on the ledger as evidence (AC3). `clearance` is the
    /// audit record (`HoldClearance` plus any release-path extras).
    HoldRelease { clearance: JsonObj },
    /// An automatic release was authorized and its audit comment posted,
    /// before the release-time recheck (TASK-1757). Not a release.
    ReleaseAuthorized { clearance: JsonObj },
    /// The release-time recheck refused after an authorization: the hold
    /// stays armed and the authorization no longer counts.
    ReleaseWithdrawn { reason: String },
    /// The forge label's sync state for the open hold (BUG-1236).
    LabelSync { label_state: String },
    /// AC4: the approval at `proof.approved_sha` carries over to
    /// `proof.new_head` (clean base merges, unchanged patch identity). The
    /// event's `head`/`patch_id`/`base_ref`/`base_tip` name what it is
    /// authority for; computed once, read by every gate.
    CarryOverProof {
        proof: crate::gate::review_carryover::CarryOver,
        proof_version: u32,
        // trace:STORY-2696 | ai:codex
        #[serde(default = "crate::gate::review_verdict::carry_eligible_default")]
        carry_eligible: bool,
    },
    /// AC4: the approval at `approved_sha` does NOT carry over to `new_head`,
    /// and `reason` says why (a changed patch identity, a non-merge commit,
    /// an edited merge). Kept only for refusals that follow from the commits
    /// alone, so a gate does not recompute them either.
    CarryOverRefused {
        approved_sha: String,
        new_head: String,
        refusal: crate::gate::review_carryover::CarryOverRefusal,
        reason: String,
        proof_version: u32,
        // trace:STORY-2696 | ai:codex
        #[serde(default = "crate::gate::review_verdict::carry_eligible_default")]
        carry_eligible: bool,
    },
}

impl EventBody {
    /// A hold placement or release: the events whose time decides whether a
    /// legacy marker file is newer than the ledger's answer.
    pub(crate) fn is_hold_transition(&self) -> bool {
        matches!(self, Self::Hold { .. } | Self::HoldRelease { .. })
    }

    /// Any event of the hold family.
    pub(crate) fn is_hold_family(&self) -> bool {
        matches!(
            self,
            Self::Hold { .. }
                | Self::HoldRelease { .. }
                | Self::ReleaseAuthorized { .. }
                | Self::ReleaseWithdrawn { .. }
                | Self::LabelSync { .. }
        )
    }
}

/// One legacy file preserved in a `legacy-imported` event.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct LegacyFile {
    /// Relative to the store root.
    pub path: String,
    pub body: String,
}

impl GateEvent {
    /// An event for `change` with nothing but its body; the appender fills
    /// in `seq` and `at`.
    pub(crate) fn new(change: ChangeKey, body: EventBody) -> Self {
        Self {
            v: EVENT_VERSION,
            seq: 0,
            at: String::new(),
            change: change.as_key(),
            head: None,
            patch_id: None,
            base_ref: None,
            base_tip: None,
            spec: None,
            spec_key: None,
            actor: None,
            risk: None,
            body,
        }
    }

    pub(crate) fn verdict_record(&self) -> Option<&JsonObj> {
        match &self.body {
            EventBody::Verdict { record } => Some(record),
            _ => None,
        }
    }
}

/// The patch identity of the change at `head`, taken in `repo`: the protected
/// base branch, its tip as last fetched, and the patch id of
/// `merge-base(base_tip, head)..head`. Each part is `None` when git cannot
/// answer (no remote, an unfetched head, criss-cross history).
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn patch_identity(
    repo: &Path,
    head: &str,
) -> (Option<String>, Option<String>, Option<String>) {
    use crate::gate::review_carryover as rc;
    let Ok(base) = rc::protected_base(repo) else {
        return (None, None, None);
    };
    let Some(tip) = rc::resolve_commit(repo, &format!("refs/remotes/origin/{base}")) else {
        return (Some(base), None, None);
    };
    let patch = rc::single_merge_base(repo, &tip, head)
        .and_then(|mb| rc::patch_id(repo, &mb, head))
        .ok();
    (Some(base), Some(tip), patch)
}

/// The `verdict` event for `record` on `change`, recorded from `root`. A
/// spec label (never a change key) is written with its canonical key.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// trace:STORY-2324 | ai:claude
pub(crate) fn verdict_event(
    root: &Path,
    change: ChangeKey,
    record: &JsonObj,
    spec: Option<&GateSpec>,
) -> GateEvent {
    let mut event = GateEvent::new(
        change,
        EventBody::Verdict {
            record: record.clone(),
        },
    );
    let parsed = serde_json::to_string(record)
        .ok()
        .and_then(|b| crate::gate::review_verdict::parse_recorded_verdict(&b));
    event.head = parsed.as_ref().and_then(|v| v.reviewed_sha.clone());
    event.actor = record
        .get(ATTESTATION_KEY)
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    if let Some(spec) = spec.filter(|s| !s.label.is_empty() && ChangeKey::parse(&s.label).is_none())
    {
        event.spec = Some(spec.label.clone());
        event.spec_key = spec.key;
    }
    if let Some(head) = event.head.as_deref() {
        (event.base_ref, event.base_tip, event.patch_id) = patch_identity(root, head);
    }
    // Compute afresh on every verdict, including refusals and collected verdicts.
    // Keep this outside the signed body and delivery reconciliation identity.
    // trace:STORY-2698 | ai:codex
    event.risk = serde_json::to_value(crate::review_risk::assess(
        root,
        event.base_tip.as_deref(),
        event.head.as_deref(),
    ))
    .ok();
    event
}

/// The `verdict` event for `record` on `spec`'s unbound ledger: `change` is
/// the spec label, and the patch identity is taken at the record's head.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// trace:STORY-2324 | ai:claude
pub(crate) fn verdict_event_unbound(root: &Path, spec: &GateSpec, record: &JsonObj) -> GateEvent {
    let mut event = verdict_event(root, ChangeKey::pr(0), record, Some(spec));
    event.change = spec.label.clone();
    event.spec = Some(event.change.clone());
    event.spec_key = spec.key;
    event
}
