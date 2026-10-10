//! BUG-1167: the substrate merge-hold marker (ADR-37 layer 1, client side).
//!
//! A supervised (drive/guided/operator/decide/unset) PR gets a
//! `.aida/merge-holds/PR-<n>` marker. Every AIDA merge path funnels through
//! `forge::merge_change`, which refuses fail-closed while the marker exists — so
//! a concurrent merger (an integrator sweep, a second `aida agent` session, a
//! stale-binary drain, `aida pr merge`) cannot bypass the supervised-merge hold
//! the way BUG-1167 reproduced. The marker is cleared only by an explicit
//! human/advisor review (`aida merge-hold clear <pr>`), which is what "merge
//! requires human/advisor review" means made enforceable rather than advisory.
//!
//! This is the client half. The paired server half (ADR-37 layer 2) is a GitHub
//! required-status-check that reads the equivalent PR label, catching even a raw
//! `gh pr merge` that never touches AIDA. A drain-mode PR is never marked, so its
//! auto-merge is unaffected (the granularity the branch-protection concern needs).
// trace:BUG-1167 | ai:claude

use crate::process_retry::RetryEtxtbsy;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Machine-readable reason for an active merge hold.
// trace:STORY-1397 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum HoldReasonKind {
    Supervision,
    Recusal,
    Rework,
    Decision,
    Unknown,
}

impl HoldReasonKind {
    /// BUG-1691: stricter gates remain primary when different kinds compose.
    // trace:BUG-1691 | ai:codex
    pub(crate) fn strictness(self) -> u8 {
        match self {
            Self::Recusal => 4,
            Self::Decision => 3,
            Self::Rework => 2,
            Self::Supervision => 1,
            Self::Unknown => 0,
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "supervision" => Some(Self::Supervision),
            "recusal" => Some(Self::Recusal),
            "rework" => Some(Self::Rework),
            "decision" => Some(Self::Decision),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Supervision => "supervision",
            Self::Recusal => "recusal",
            Self::Rework => "rework",
            Self::Decision => "decision",
            Self::Unknown => "unknown",
        }
    }
}

/// Routing state is explicit: absence of a reader is operational evidence, not
/// an empty queue that looks complete.
// trace:STORY-1397 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum HoldRoutingState {
    Pending,
    Routed,
    NoIndependentReader,
    ReviewedReady,
    StaleHead,
}

impl HoldRoutingState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Routed => "routed",
            Self::NoIndependentReader => "no-independent-reader",
            Self::ReviewedReady => "reviewed-ready",
            Self::StaleHead => "stale-head",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PrincipalKind {
    RegisteredAgent,
    Operator,
    /// A human acting at an interactive terminal (the integrity floor).
    // trace:STORY-1397 | ai:claude
    Human,
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum IdentityStatus {
    Verified,
    Unresolved,
}

/// A principal named on a hold. `IdentityStatus::Verified` means the name is
/// WELL-FORMED (`agent:` / `operator:` / `human:` prefix with a non-empty id),
/// not that it was authenticated: an `agent:` id is a string any process can
/// assert. Nothing therefore grants authority on the strength of a Verified
/// agent identity alone — clearing a hold is gated on a human at a terminal
/// (the integrity floor), and an agent identity can only ever TIGHTEN a
/// decision (a matching recused id refuses), never loosen one.
// trace:STORY-1397 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PrincipalIdentity {
    pub principal_kind: PrincipalKind,
    pub principal_id: String,
    pub identity_status: IdentityStatus,
}

impl PrincipalIdentity {
    pub(crate) fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        if let Some(id) = raw.strip_prefix("agent:").filter(|id| !id.is_empty()) {
            return Self {
                principal_kind: PrincipalKind::RegisteredAgent,
                principal_id: id.into(),
                identity_status: IdentityStatus::Verified,
            };
        }
        if let Some(id) = raw.strip_prefix("operator:").filter(|id| !id.is_empty()) {
            return Self {
                principal_kind: PrincipalKind::Operator,
                principal_id: id.into(),
                identity_status: IdentityStatus::Verified,
            };
        }
        if let Some(id) = raw.strip_prefix("human:").filter(|id| !id.is_empty()) {
            return Self::human(id);
        }
        Self {
            principal_kind: PrincipalKind::Unresolved,
            principal_id: raw.into(),
            identity_status: IdentityStatus::Unresolved,
        }
    }

    pub(crate) fn registered_agent(id: impl Into<String>) -> Self {
        Self {
            principal_kind: PrincipalKind::RegisteredAgent,
            principal_id: id.into(),
            identity_status: IdentityStatus::Verified,
        }
    }

    /// A human at an interactive terminal, keyed by the shell user id.
    // trace:STORY-1397 | ai:claude
    pub(crate) fn human(user: impl Into<String>) -> Self {
        Self {
            principal_kind: PrincipalKind::Human,
            principal_id: user.into(),
            identity_status: IdentityStatus::Verified,
        }
    }

    pub(crate) fn key(&self) -> String {
        match self.principal_kind {
            PrincipalKind::RegisteredAgent => format!("agent:{}", self.principal_id),
            PrincipalKind::Operator => format!("operator:{}", self.principal_id),
            PrincipalKind::Human => format!("human:{}", self.principal_id),
            PrincipalKind::Unresolved => format!("unresolved:{}", self.principal_id),
        }
    }

    fn is_verified(&self) -> bool {
        self.identity_status == IdentityStatus::Verified
    }
}

impl From<&str> for PrincipalIdentity {
    fn from(value: &str) -> Self {
        Self::parse(value)
    }
}

impl PartialEq<&str> for PrincipalIdentity {
    fn eq(&self, other: &&str) -> bool {
        self.key() == *other
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MergeHoldRecord {
    pub schema_version: u32,
    pub pr: u64,
    pub reason_kind: HoldReasonKind,
    pub detail: String,
    #[serde(default)]
    pub recused_principals: Vec<PrincipalIdentity>,
    #[serde(default)]
    pub routed_to: Vec<PrincipalIdentity>,
    pub routing_state: HoldRoutingState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_state: Option<String>,
    #[serde(default)]
    pub legacy: bool,
    /// BUG-1532 criterion 10: the verdict record a refusal hold stands on.
    /// The marker REFERENCES the verdict rather than quoting it, so a reader
    /// follows the record (which is re-recorded round after round) instead of
    /// a frozen snapshot of the first refusal. `detail` is a convenience
    /// summary from placement time and is never treated as current.
    // trace:BUG-1532 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict_ref: Option<VerdictRef>,
    /// BUG-1562: what must be true for the hold to be released — a FUTURE
    /// check evaluated when read, not a reason describing the past.
    // trace:BUG-1562 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_condition: Option<String>,
    /// BUG-1562: the spec the hold was placed for, when the writer knew it.
    /// Lets a reader check the premise ("is that spec still running?")
    /// without parsing prose.
    // trace:BUG-1562 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<String>,
    /// STORY-1416: the seat that PLACED this marker, so a later verdict write
    /// can show who asserted what the marker says. `None` on markers written
    /// before this field existed.
    // trace:STORY-1416 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placed_by: Option<String>,
    /// Displaced holds retained for audit; primary reason_kind is strictest.
    // trace:BUG-1691 | ai:codex
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub absorbed: Vec<AbsorbedHold>,
}

/// BUG-1532 criterion 10: the identity of a recorded review verdict — the
/// key its file is stored under (`.aida/review-verdicts/<KEY>.json`, a spec
/// id or `PR-<n>`), the PR, the commit it was recorded against, and the seat
/// that recorded it. The OWNER of a refusal hold is read from the verdict
/// this points at, never from the marker's prose.
// trace:BUG-1532 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct VerdictRef {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_by: Option<String>,
}

/// Flat shadow of a hold displaced by a stricter kind.
// trace:BUG-1691 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AbsorbedHold {
    pub reason_kind: HoldReasonKind,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recused_principals: Vec<PrincipalIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict_ref: Option<VerdictRef>,
}

impl AbsorbedHold {
    // trace:BUG-1691 | ai:codex
    fn from_record(record: &MergeHoldRecord) -> Self {
        Self {
            reason_kind: record.reason_kind,
            detail: record.detail.clone(),
            recused_principals: record.recused_principals.clone(),
            target_head_sha: record.target_head_sha.clone(),
            spec: record.spec.clone(),
            placed_by: record.placed_by.clone(),
            verdict_ref: record.verdict_ref.clone(),
        }
    }
}

impl VerdictRef {
    pub(crate) fn new(
        key: &str,
        pr: Option<u64>,
        reviewed_sha: Option<String>,
        recorded_by: Option<String>,
    ) -> Self {
        Self {
            key: key.trim().to_ascii_uppercase(),
            pr,
            reviewed_sha: reviewed_sha.filter(|s| !s.trim().is_empty()),
            recorded_by: recorded_by.filter(|s| !s.trim().is_empty()),
        }
    }

    /// The verdict file this reference names, relative to the project root.
    pub(crate) fn path(&self) -> String {
        format!(".aida/review-verdicts/{}.json", self.key)
    }
}

impl MergeHoldRecord {
    /// BUG-1532 criterion 5: the hold kinds `aida pr ship` reads as a
    /// REVIEWER REFUSAL — typed rework, or an untyped legacy marker (unknown
    /// is not permission). A refusal is released only by a fresh verdict at
    /// the current head ([`refusal_release`]).
    // trace:BUG-1532 | ai:claude
    pub(crate) fn is_refusal(&self) -> bool {
        self.legacy || self.reason_kind == HoldReasonKind::Rework
    }

    /// BUG-1562: record the spec the hold was placed for, so a reader can
    /// check "is it still running?" without parsing prose.
    // trace:BUG-1562 | ai:claude
    pub(crate) fn with_spec(mut self, spec: &str) -> Self {
        let spec = spec.trim();
        self.spec = (!spec.is_empty()).then(|| spec.to_ascii_uppercase());
        self
    }

    /// BUG-1562: the release condition to show — the recorded one, else the
    /// default this hold kind implies (marked as such by the caller).
    // trace:BUG-1562 | ai:claude
    pub(crate) fn release_condition_or_default(&self) -> (String, bool) {
        if let Some(cond) = self
            .release_condition
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            return (cond.to_string(), false);
        }
        let pr = self.pr;
        let default = if self.is_refusal() {
            let key = self
                .verdict_ref
                .as_ref()
                .map(|r| r.key.clone())
                .unwrap_or_else(|| format!("PR-{pr}"));
            format!(
                "a fresh APPROVED verdict for {key} recorded at the PR's current head; then a human ships it"
            )
        } else {
            match self.reason_kind {
                HoldReasonKind::Supervision | HoldReasonKind::Decision => format!(
                    "a human at a terminal decides: `aida pr ship {pr}` or `aida merge-hold clear {pr}`"
                ),
                HoldReasonKind::Recusal => {
                    "an independent reader reviews the exact head; a human clears it".to_string()
                }
                _ => format!("a human inspects it and runs `aida merge-hold clear {pr}`"),
            }
        };
        (default, true)
    }
    fn legacy(pr: u64, body: &str) -> Self {
        let detail = body.lines().next().unwrap_or("").trim();
        let label_state = body
            .lines()
            .nth(1)
            .map(str::trim)
            .and_then(|line| line.strip_prefix("label: ").map(str::to_string));
        Self {
            schema_version: 1,
            pr,
            reason_kind: HoldReasonKind::Supervision,
            detail: if detail.is_empty() {
                format!("PR-{pr} is under a supervised merge-hold")
            } else {
                detail.to_string()
            },
            recused_principals: Vec::new(),
            routed_to: Vec::new(),
            routing_state: HoldRoutingState::Pending,
            target_head_sha: None,
            label_state,
            legacy: true,
            verdict_ref: None,
            release_condition: None,
            spec: None,
            placed_by: None,
            absorbed: Vec::new(),
        }
    }
}

fn holds_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("merge-holds")
}

/// The marker path for one PR. Public so callers can log it.
pub(crate) fn hold_path(project_root: &Path, pr: u64) -> PathBuf {
    holds_dir(project_root).join(format!("PR-{pr}"))
}

/// Record a supervised merge-hold for `pr` with a human-readable reason.
/// Idempotent — re-recording refreshes the reason.
///
/// STORY-1397: production writers all emit typed v2 markers via
/// [`write_typed_hold`]; this legacy plaintext writer survives only so tests
/// can build the legacy marker shape the reader must keep honouring.
// trace:STORY-1397 | ai:claude
#[cfg(test)]
pub(crate) fn write_hold(project_root: &Path, pr: u64, reason: &str) -> std::io::Result<()> {
    let dir = holds_dir(project_root);
    std::fs::create_dir_all(&dir)?;
    let reason = reason.trim();
    let body = if reason.is_empty() {
        format!("PR-{pr} is under a supervised merge-hold\n")
    } else {
        format!("{reason}\n")
    };
    std::fs::write(hold_path(project_root, pr), body)
}

/// Write a v2 typed marker, preserving stricter existing gates.
// trace:STORY-1397 trace:BUG-1691 | ai:codex
pub(crate) fn write_typed_hold(
    project_root: &Path,
    record: &MergeHoldRecord,
) -> std::io::Result<()> {
    match compose_with_existing(project_root, record) {
        Composed::Placed(r) => write_marker(project_root, &r, true),
        Composed::Preserved(r) => write_marker(project_root, &r, false),
    }
}

/// Replace the marker; reserved for the explicit operator escape hatch.
// trace:STORY-1397 trace:BUG-1691 | ai:codex
fn write_typed_hold_replacing(
    project_root: &Path,
    record: &MergeHoldRecord,
) -> std::io::Result<()> {
    write_marker(project_root, record, true)
}

// trace:BUG-1691 | ai:codex
fn write_marker(
    project_root: &Path,
    record: &MergeHoldRecord,
    reconcile_route: bool,
) -> std::io::Result<()> {
    let dir = holds_dir(project_root);
    std::fs::create_dir_all(&dir)?;
    if record.pr == 0
        || (record.reason_kind == HoldReasonKind::Recusal
            && (record.recused_principals.is_empty()
                || record
                    .target_head_sha
                    .as_deref()
                    .is_none_or(|sha| sha.trim().is_empty())))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a recusal hold requires a PR, exact head, and at least one recused principal",
        ));
    }
    let mut normalized = record.clone();
    normalized.schema_version = 2;
    normalized.legacy = false;
    if reconcile_route && normalized.reason_kind == HoldReasonKind::Recusal {
        reconcile_recusal_route(project_root, &mut normalized)?;
    }
    let body = serde_json::to_vec_pretty(&normalized)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    match std::fs::remove_file(clearance_path(project_root, record.pr)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    aida_core::fs_atomic::write_atomic(&hold_path(project_root, record.pr), &body)?;
    // trace:BUG-1693 | ai:codex
    let mut event = crate::events::Event::new(
        None,
        "",
        crate::events::EventKind::MergeHoldChanged {
            pr: record.pr as u32,
            placed: true,
            reason: Some(record.detail.clone()),
        },
    );
    event.seat = crate::events::active_seat();
    crate::events::emit(project_root, &event);
    Ok(())
}

/// BUG-1562: place a hold by hand (`aida merge-hold add`) WITHOUT silently
/// replacing an existing marker's body. A marker already present — typed,
/// legacy, hand-written, even unreadable — refuses unless `replace`, and the
/// refusal quotes what would have been lost. The label half is the caller's.
// trace:BUG-1562 | ai:claude
pub(crate) fn place_hand_hold(
    project_root: &Path,
    record: &MergeHoldRecord,
    replace: bool,
) -> Result<(), String> {
    let path = hold_path(project_root, record.pr);
    if !replace && std::fs::symlink_metadata(&path).is_ok() {
        let existing = read_hold_record(project_root, record.pr)
            .map(|r| r.detail)
            .unwrap_or_default();
        let first = existing.lines().next().unwrap_or("").trim();
        return Err(format!(
            "PR #{pr} already has a merge-hold marker ({shown}); `aida merge-hold add` will not \
             overwrite its body. Keep it and re-apply a missing label with \
             `aida merge-hold list --fix`, or replace it deliberately with \
             `aida merge-hold add {pr} --replace ...`.",
            pr = record.pr,
            shown = if first.is_empty() {
                path.display().to_string()
            } else {
                format!("\"{first}\"")
            }
        ));
    }
    write_typed_hold_replacing(project_root, record).map_err(|e| e.to_string())
}

/// Compose an incoming hold with the marker already present on the PR.
// trace:BUG-1691 | ai:codex
// trace:BUG-1691 | ai:codex
enum Composed {
    Placed(MergeHoldRecord),
    Preserved(MergeHoldRecord),
}

// trace:BUG-1691 | ai:codex
fn compose_with_existing(project_root: &Path, incoming: &MergeHoldRecord) -> Composed {
    let Some(existing) = read_hold_record(project_root, incoming.pr) else {
        return Composed::Placed(incoming.clone());
    };
    compose_holds(&existing, incoming)
}

/// Pure precedence policy for two hold records.
// trace:BUG-1691 | ai:codex
fn compose_holds(existing: &MergeHoldRecord, incoming: &MergeHoldRecord) -> Composed {
    if existing.reason_kind == incoming.reason_kind {
        let mut out = incoming.clone();
        merge_absorbed(&mut out.absorbed, &existing.absorbed);
        return Composed::Placed(out);
    }
    let (mut primary, loser, preserved) =
        if incoming.reason_kind.strictness() > existing.reason_kind.strictness() {
            (incoming.clone(), existing, false)
        } else {
            (existing.clone(), incoming, true)
        };
    let mut carried = loser.absorbed.clone();
    carried.push(AbsorbedHold::from_record(loser));
    merge_absorbed(&mut primary.absorbed, &carried);
    primary
        .absorbed
        .retain(|a| a.reason_kind != primary.reason_kind);
    if preserved {
        Composed::Preserved(primary)
    } else {
        Composed::Placed(primary)
    }
}

/// Union absorbed shadows by kind, with the newest entry taking precedence.
// trace:BUG-1691 | ai:codex
fn merge_absorbed(into: &mut Vec<AbsorbedHold>, extra: &[AbsorbedHold]) {
    for e in extra {
        if let Some(slot) = into.iter_mut().find(|a| a.reason_kind == e.reason_kind) {
            *slot = e.clone();
        } else {
            into.push(e.clone());
        }
    }
    into.sort_by_key(|a| std::cmp::Reverse(a.reason_kind.strictness()));
}

/// Refresh a recusal hold from live evidence: adopt a moved PR head (which
/// invalidates the old route), re-select a live independent reader, and write
/// or retire the routing briefs. Idempotent. A failed refresh leaves the
/// existing marker armed and returns the last durable record.
///
/// This WRITES (marker rewrite, brief creation/retirement) and probes process
/// liveness, so it is called only from explicit merge-hold commands
/// (`aida merge-hold list --fix`, and `add` via [`write_typed_hold`]) — never
/// from a read surface. `aida awaiting` and its per-turn notice stay read-only
/// and use [`project_live_head`] instead.
// trace:STORY-1397 | ai:codex
pub(crate) fn reconcile_recusal_hold(
    project_root: &Path,
    pr: u64,
    live_head: Option<&str>,
) -> Option<MergeHoldRecord> {
    let durable = read_hold_record(project_root, pr)?;
    if durable.reason_kind != HoldReasonKind::Recusal || durable.legacy {
        return Some(durable);
    }
    let mut refreshed = project_live_head(&durable, live_head);
    if reconcile_recusal_route(project_root, &mut refreshed).is_err() {
        return Some(durable);
    }
    if refreshed == durable {
        return Some(refreshed);
    }
    let body = match serde_json::to_vec_pretty(&refreshed) {
        Ok(body) => body,
        Err(_) => return Some(durable),
    };
    if aida_core::fs_atomic::write_atomic(&hold_path(project_root, pr), &body).is_err() {
        return Some(durable);
    }
    Some(refreshed)
}

/// Read-only view of a hold against the PR's live head. A recusal routed at an
/// older head is shown as `StaleHead` with no current route; nothing is
/// written. The durable refresh is [`reconcile_recusal_hold`].
// trace:STORY-1397 | ai:claude
pub(crate) fn project_live_head(
    record: &MergeHoldRecord,
    live_head: Option<&str>,
) -> MergeHoldRecord {
    let mut view = record.clone();
    if view.reason_kind != HoldReasonKind::Recusal || view.legacy {
        return view;
    }
    if let Some(head) = live_head.map(str::trim).filter(|head| !head.is_empty()) {
        if view.target_head_sha.as_deref() != Some(head) {
            view.target_head_sha = Some(head.to_string());
            view.routing_state = HoldRoutingState::StaleHead;
            view.routed_to.clear();
        }
    }
    view
}

fn reconcile_recusal_route(
    project_root: &Path,
    record: &mut MergeHoldRecord,
) -> std::io::Result<()> {
    let agents = project_root.join(".aida/agents");
    let mut candidates = Vec::new();
    if let Ok(entries) = std::fs::read_dir(agents) {
        for entry in entries.flatten() {
            let Ok(body) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(value) = toml::from_str::<toml::Value>(&body) else {
                continue;
            };
            let Some(id) = value.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let role = value.get("role").and_then(|v| v.as_str()).unwrap_or("");
            let agent_type = value
                .get("agent_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let ended = value.get("ended_at").is_some();
            let paused = value
                .get("availability")
                .and_then(|v| v.as_str())
                .is_some_and(|v| v.eq_ignore_ascii_case("paused"));
            let principal = PrincipalIdentity::registered_agent(id);
            let eligible_role = matches!(
                role.to_ascii_lowercase().as_str(),
                "reviewer" | "advisor" | "integrator"
            );
            // Every cheap filter runs before the liveness probe, and the probe
            // is a single-pid check (kill(pid, 0) on Unix), never a full
            // process-table scan. trace:STORY-1397 | ai:claude
            if ended
                || paused
                || !eligible_role
                || agent_type.is_empty()
                || principal_is_recused(record, &principal)
            {
                continue;
            }
            let live = value
                .get("pid")
                .and_then(|v| v.as_integer())
                .and_then(|v| u32::try_from(v).ok())
                .is_some_and(aida_core::liveness::pid_is_alive);
            if live {
                candidates.push((principal, agent_type.to_string()));
            }
        }
    }
    candidates.sort_by(|a, b| a.0.principal_id.cmp(&b.0.principal_id));
    let Some((principal, agent_type)) = candidates.into_iter().next() else {
        retire_stale_route_briefs(project_root, record.pr, None)?;
        record.routed_to.clear();
        record.routing_state = HoldRoutingState::NoIndependentReader;
        return Ok(());
    };
    record.routed_to = vec![principal.clone()];
    record.routing_state = HoldRoutingState::Routed;
    let head = record.target_head_sha.as_deref().unwrap_or("unknown");
    let dir = project_root.join(".aida/agent-briefs").join(&agent_type);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("PR-{}-{}.md", record.pr, head));
    retire_stale_route_briefs(project_root, record.pr, Some(&path))?;
    if !path.exists() {
        let recused = record
            .recused_principals
            .iter()
            .map(PrincipalIdentity::key)
            .collect::<Vec<_>>()
            .join(", ");
        let body = format!("---\nspec_id: PR-{}\ntitle: Independent exact-head review\n---\n\nIndependently review PR #{} at exact head `{}`.\n\nRecused principals: {}\n\nRecord a durable verdict; acknowledging this brief does not clear or merge the hold.\n", record.pr, record.pr, head, recused);
        aida_core::fs_atomic::write_atomic(&path, body.as_bytes())?;
    }
    Ok(())
}

fn retire_stale_route_briefs(
    project_root: &Path,
    pr: u64,
    keep: Option<&Path>,
) -> std::io::Result<()> {
    let root = project_root.join(".aida/agent-briefs");
    let Ok(agent_dirs) = std::fs::read_dir(root) else {
        return Ok(());
    };
    let prefix = format!("PR-{pr}-");
    for agent_dir in agent_dirs.flatten() {
        let Ok(entries) = std::fs::read_dir(agent_dir.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&prefix) && name.ends_with(".md") && keep != Some(path.as_path()) {
                let stale = path.with_extension("md.stale");
                if stale.exists() {
                    std::fs::remove_file(&stale)?;
                }
                std::fs::rename(path, stale)?;
            }
        }
    }
    Ok(())
}

/// Parse typed and legacy markers. A malformed marker remains a typed Unknown
/// hold, preserving the merge chokepoint while exposing the diagnostic.
// trace:STORY-1397 | ai:codex
pub(crate) fn read_hold_record(project_root: &Path, pr: u64) -> Option<MergeHoldRecord> {
    match std::fs::read_to_string(hold_path(project_root, pr)) {
        Ok(body) if body.trim_start().starts_with('{') => {
            match serde_json::from_str::<MergeHoldRecord>(&body) {
                Ok(record) if record.schema_version == 2 && record.pr == pr => Some(record),
                Ok(_) | Err(_) => Some(MergeHoldRecord {
                    schema_version: 0,
                    pr,
                    reason_kind: HoldReasonKind::Unknown,
                    detail: format!(
                        "PR-{pr} merge-hold marker malformed or unsupported — held for safety"
                    ),
                    recused_principals: Vec::new(),
                    routed_to: Vec::new(),
                    routing_state: HoldRoutingState::NoIndependentReader,
                    target_head_sha: None,
                    label_state: None,
                    legacy: false,
                    verdict_ref: None,
                    release_condition: None,
                    spec: None,
                    placed_by: None,
                    absorbed: Vec::new(),
                }),
            }
        }
        Ok(body) => Some(MergeHoldRecord::legacy(pr, &body)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => Some(MergeHoldRecord {
            schema_version: 0,
            pr,
            reason_kind: HoldReasonKind::Unknown,
            detail: format!("PR-{pr} merge-hold marker present but unreadable — held for safety"),
            recused_principals: Vec::new(),
            routed_to: Vec::new(),
            routing_state: HoldRoutingState::NoIndependentReader,
            target_head_sha: None,
            label_state: None,
            legacy: false,
            verdict_ref: None,
            release_condition: None,
            spec: None,
            placed_by: None,
            absorbed: Vec::new(),
        }),
    }
}

// trace:STORY-1397 | ai:codex
pub(crate) fn principal_is_recused(
    record: &MergeHoldRecord,
    principal: &PrincipalIdentity,
) -> bool {
    principal.is_verified()
        && record.recused_principals.iter().any(|p| {
            p.is_verified()
                && p.principal_kind == principal.principal_kind
                && p.principal_id == principal.principal_id
        })
}

/// The acting agent's self-declared identity, from `AIDA_AGENT_ID`.
///
/// LIMITATION: this is a self-set environment variable, not an established,
/// authenticated identity — nothing in AIDA's launch path sets it, and any
/// process can set it to anything. It is therefore used only to PERSONALIZE
/// the read-only awaiting projection and to TIGHTEN a clear (an id matching a
/// recused principal refuses). It never satisfies independence and never
/// grants a clear.
// trace:STORY-1397 | ai:claude
pub(crate) fn current_principal() -> Option<PrincipalIdentity> {
    std::env::var("AIDA_AGENT_ID")
        .ok()
        .filter(|id| !id.trim().is_empty())
        .map(PrincipalIdentity::registered_agent)
}

pub(crate) fn typed_hold(
    pr: u64,
    kind: HoldReasonKind,
    detail: impl Into<String>,
    target_head_sha: Option<String>,
) -> MergeHoldRecord {
    MergeHoldRecord {
        schema_version: 2,
        pr,
        reason_kind: kind,
        detail: detail.into(),
        recused_principals: Vec::new(),
        routed_to: Vec::new(),
        routing_state: HoldRoutingState::Pending,
        target_head_sha,
        label_state: None,
        legacy: false,
        verdict_ref: None,
        release_condition: None,
        spec: None,
        placed_by: Some(placing_seat()),
        absorbed: Vec::new(),
    }
}

/// STORY-1416: the seat placing a marker — the launched agent's name (with
/// its role when known), else the session role, else the shell user.
// trace:STORY-1416 | ai:claude
pub(crate) fn placing_seat() -> String {
    let env = |k: &str| {
        std::env::var(k)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    match (env("AIDA_AGENT_NAME"), env("AIDA_SESSION_ROLE")) {
        (Some(name), Some(role)) => format!("{name} ({role})"),
        (Some(name), None) => name,
        (None, Some(role)) => format!("{role} seat"),
        (None, None) => env("USER")
            .or_else(|| env("USERNAME"))
            .map(|u| format!("user:{u}"))
            .unwrap_or_else(|| "unknown".to_string()),
    }
}

/// Who clears a hold, given the human at the terminal and any self-declared
/// agent identity. Callers have ALREADY enforced the integrity floor (a human
/// at an interactive terminal); this function never relaxes that.
///
/// The independence rule restricts the recused principals. A human clearing
/// at a terminal is independent of a recused AUTHOR AGENT by definition, so no
/// agent identity or agent-recorded verdict is required and the clearance is
/// attributed to `human:<user>`. The human is refused only when the hold
/// itself names that human (`human:<user>` or `operator:<user>`) as recused.
/// A self-declared agent id can only refuse (when it is recused), never grant.
// trace:STORY-1397 | ai:claude
pub(crate) fn human_clear_actor(
    record: &MergeHoldRecord,
    human_user: &str,
    declared_agent: Option<&PrincipalIdentity>,
) -> Result<PrincipalIdentity, String> {
    if declared_agent.is_some_and(|agent| principal_is_recused(record, agent)) {
        return Err(format!(
            "the acting agent identity is recused from PR-{}; only an independent reader may clear this hold",
            record.pr
        ));
    }
    let user = human_user.trim();
    if user.is_empty() {
        return Err(format!(
            "PR-{} hold cannot be cleared: the human user identity is unknown",
            record.pr
        ));
    }
    let human = PrincipalIdentity::human(user);
    let operator = PrincipalIdentity {
        principal_kind: PrincipalKind::Operator,
        principal_id: user.to_string(),
        identity_status: IdentityStatus::Verified,
    };
    if principal_is_recused(record, &human) || principal_is_recused(record, &operator) {
        return Err(format!(
            "{} is recused from PR-{}; another independent reader must clear this hold",
            human.key(),
            record.pr
        ));
    }
    Ok(human)
}

/// Durable audit record of who cleared a hold, written by the explicit
/// `aida merge-hold clear` command (never by a read surface). Per-clone
/// runtime state under `.aida/`, like the markers themselves.
// trace:STORY-1397 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HoldClearance {
    pub pr: u64,
    pub reason_kind: HoldReasonKind,
    pub detail: String,
    pub cleared_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_head_sha: Option<String>,
    pub cleared_at: String,
    /// BUG-1532: the fresh verdict at head that met a refusal hold's release
    /// condition, when one did. `None` = a human cleared it without one.
    // trace:BUG-1532 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_by_verdict: Option<VerdictRef>,
}

pub(crate) fn clearance_path(project_root: &Path, pr: u64) -> PathBuf {
    project_root
        .join(".aida")
        .join("merge-hold-clearances")
        .join(format!("PR-{pr}.json"))
}

/// Local audit signal for a hold placed through AIDA whose marker disappeared
/// without the human clearance command recording a release.
// trace:BUG-1693 | ai:codex
pub(crate) fn unrecorded_marker_removals(project_root: &Path) -> Vec<u64> {
    use crate::events::EventKind;
    let mut latest = std::collections::BTreeMap::<u32, bool>::new();
    for event in crate::events::read_all_with_archive(project_root) {
        if let EventKind::MergeHoldChanged { pr, placed, .. } = event.kind {
            latest.insert(pr, placed);
        }
    }
    latest
        .into_iter()
        .filter_map(|(pr, placed)| {
            let pr = u64::from(pr);
            (placed
                && !hold_path(project_root, pr).exists()
                && !clearance_path(project_root, pr).exists())
            .then_some(pr)
        })
        .collect()
}

pub(crate) fn record_clearance(
    project_root: &Path,
    record: &MergeHoldRecord,
    actor: &PrincipalIdentity,
) -> std::io::Result<()> {
    record_clearance_with_verdict(project_root, record, actor, None)
}

/// [`record_clearance`], naming the fresh verdict that met a refusal hold's
/// release condition.
// trace:BUG-1532 | ai:claude
pub(crate) fn record_clearance_with_verdict(
    project_root: &Path,
    record: &MergeHoldRecord,
    actor: &PrincipalIdentity,
    released_by_verdict: Option<VerdictRef>,
) -> std::io::Result<()> {
    let path = clearance_path(project_root, record.pr);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let clearance = HoldClearance {
        pr: record.pr,
        reason_kind: record.reason_kind,
        detail: record.detail.clone(),
        cleared_by: actor.key(),
        target_head_sha: record.target_head_sha.clone(),
        cleared_at: chrono::Utc::now().to_rfc3339(),
        released_by_verdict,
    };
    let body = serde_json::to_vec_pretty(&clearance)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    aida_core::fs_atomic::write_atomic(&path, &body)
}

/// The hold reason if `pr` is under a supervised merge-hold, else `None`.
/// The merge chokepoint refuses whenever this is `Some`.
///
/// TASK-1238: fails CLOSED on a present-but-unreadable marker. `NotFound` is the
/// only "no hold" answer; ANY other read error (permissions, a directory in its
/// place, a transient IO fault) means a marker may be there but we can't confirm
/// it isn't — so we HOLD. The prior `.ok()?` collapsed every error to `None`,
/// which would let a merge through when the marker was present but unreadable —
/// the exact fail-open class this whole marker exists to prevent.
pub(crate) fn read_hold(project_root: &Path, pr: u64) -> Option<String> {
    read_hold_record(project_root, pr)
        .map(|record| record.detail)
        .or_else(|| {
            unrecorded_marker_removals(project_root)
                .contains(&pr)
                .then(|| MARKER_MISSING_REASON.to_string())
        })
}

/// Clear the hold — an explicit human/advisor review. Idempotent: clearing a
/// PR with no hold is a no-op success.
pub(crate) fn clear_hold(project_root: &Path, pr: u64) -> std::io::Result<()> {
    match std::fs::remove_file(hold_path(project_root, pr)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Every active hold as `(pr_number, reason)`, ascending by PR.
// Consumed by the follow-up `aida merge-hold list` surface; kept here so the
// primitive lands with the safety core.
#[allow(dead_code)]
pub(crate) fn list_holds(project_root: &Path) -> Vec<(u64, String)> {
    let dir = holds_dir(project_root);
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(n) = name.strip_prefix("PR-").and_then(|s| s.parse::<u64>().ok()) {
                if let Some(reason) = read_hold(project_root, n) {
                    out.push((n, reason));
                }
            }
        }
    }
    out.sort_by_key(|(n, _)| *n);
    out
}

/// The GitHub label mirroring the merge-hold marker for ADR-37 Layer 2 (a
/// required status check keyed to this label blocks a raw/stale/UI merge the
/// client chokepoint never sees). trace on the item below stays a plain comment.
// trace:BUG-1167 | ai:claude
pub(crate) const HOLD_LABEL: &str = "aida:merge-hold";
/// Permanent forge-side evidence that this PR has been held at least once.
// trace:BUG-1693 | ai:codex
pub(crate) const HOLD_RECORDED_LABEL: &str = "aida:merge-hold-recorded";
/// Forge-side mirror of the human-gated clearance record.
// trace:BUG-1693 | ai:codex
pub(crate) const HOLD_CLEARED_LABEL: &str = "aida:merge-hold-cleared";

// trace:BUG-1747 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MergeHoldLabel {
    pub name: &'static str,
    pub color: &'static str,
    pub description: &'static str,
}

pub(crate) const MERGE_HOLD_LABELS: [MergeHoldLabel; 3] = [
    MergeHoldLabel {
        name: HOLD_LABEL,
        color: "B60205",
        description: "Supervised merge hold — merge-hold-gate fails while present",
    },
    MergeHoldLabel {
        name: HOLD_RECORDED_LABEL,
        color: "B60205",
        description: "Supervised merge hold was recorded — persists across clearance",
    },
    MergeHoldLabel {
        name: HOLD_CLEARED_LABEL,
        color: "0E8A16",
        description: "Recorded merge hold has a human-gated clearance",
    },
];

// trace:BUG-1747 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LabelDefinitions {
    Read {
        present: Vec<&'static str>,
        missing: Vec<&'static str>,
    },
    NoForge,
    Unknown(String),
}

pub(crate) fn label_definitions(project_root: &Path) -> LabelDefinitions {
    let kind = crate::forge::resolve_forge_kind(project_root);
    if kind == crate::forge::ForgeKind::None {
        return LabelDefinitions::NoForge;
    }
    let pin = match resolve_pinned_repo(project_root, kind) {
        Ok(Some(pin)) => pin,
        Ok(None) => return LabelDefinitions::NoForge,
        Err(err) => return LabelDefinitions::Unknown(err),
    };
    label_definitions_with(project_root, &pin, run_forge_cli_stdout_bounded)
}

// A doctor probe must not let a hung forge CLI stall diagnostics indefinitely.
// Keep this separate from the shared runner used by hold placement.
const LABEL_DEFINITIONS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

fn run_forge_cli_stdout_bounded(
    project_root: &Path,
    cli: &str,
    args: &[String],
) -> Result<(bool, String), String> {
    run_forge_cli_stdout_bounded_with(project_root, cli, args, LABEL_DEFINITIONS_TIMEOUT)
}

fn run_forge_cli_stdout_bounded_with(
    project_root: &Path,
    cli: &str,
    args: &[String],
    timeout: std::time::Duration,
) -> Result<(bool, String), String> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::Instant;

    let mut command = std::process::Command::new(cli);
    command
        .current_dir(project_root)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not run {cli}: {e}"))?;
    let (sender, receiver) = std::sync::mpsc::channel();
    let stdout = child.stdout.take().expect("piped stdout");
    let stdout_sender = sender.clone();
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let result = stdout.take(4 * 1024 * 1024).read_to_end(&mut output);
        let _ = stdout_sender.send((false, result, output));
    });
    let stderr = child.stderr.take().expect("piped stderr");
    let stderr_sender = sender.clone();
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let result = stderr.take(4 * 1024 * 1024).read_to_end(&mut output);
        let _ = stderr_sender.send((true, result, output));
    });
    drop(sender);
    let deadline = Instant::now() + timeout;
    let mut status = None;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdout_read = false;
    let mut stderr_read = false;
    loop {
        if status.is_none() {
            status = child
                .try_wait()
                .map_err(|e| format!("could not wait for {cli}: {e}"))?;
        }
        while let Ok((is_stderr, result, output)) = receiver.try_recv() {
            result.map_err(|e| format!("could not read {cli} output: {e}"))?;
            if is_stderr {
                stderr = output;
                stderr_read = true;
            } else {
                stdout = output;
                stdout_read = true;
            }
        }
        if let Some(status) = status.as_ref().filter(|_| stdout_read && stderr_read) {
            let output = if status.success() { stdout } else { stderr };
            return Ok((
                status.success(),
                String::from_utf8_lossy(&output).to_string(),
            ));
        }
        if Instant::now() >= deadline {
            #[cfg(unix)]
            unsafe {
                libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("timed out after {}s", timeout.as_secs()));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn label_definitions_with(
    root: &Path,
    pin: &PinnedRepo,
    runner: impl Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
) -> LabelDefinitions {
    use crate::forge::ForgeKind;
    let (cli, args) = match pin.kind {
        ForgeKind::GitHub => (
            "gh",
            vec![
                "label".into(),
                "list".into(),
                "-R".into(),
                pin.repo_arg(),
                "--json".into(),
                "name".into(),
                "--limit".into(),
                "200".into(),
            ],
        ),
        ForgeKind::GitLab => (
            "glab",
            vec!["label".into(), "list".into(), "-R".into(), pin.repo_arg()],
        ),
        ForgeKind::None => return LabelDefinitions::NoForge,
    };
    let output = match runner(root, cli, &args) {
        Ok((true, out)) => out,
        Ok((false, out)) => return LabelDefinitions::Unknown(out),
        Err(err) => return LabelDefinitions::Unknown(err),
    };
    let names: Vec<String> = if pin.kind == ForgeKind::GitHub {
        match serde_json::from_str::<Vec<serde_json::Value>>(&output) {
            Ok(rows) => match rows
                .iter()
                .map(|row| {
                    row.get("name")
                        .and_then(|value| value.as_str())
                        .map(str::to_owned)
                })
                .collect::<Option<Vec<_>>>()
            {
                Some(names) => names,
                None => {
                    return LabelDefinitions::Unknown(
                        "gh label list output did not contain a name for every row".into(),
                    )
                }
            },
            Err(err) => {
                return LabelDefinitions::Unknown(format!(
                    "could not parse gh label list output: {err}"
                ))
            }
        }
    } else {
        output.lines().map(str::to_owned).collect()
    };
    let exists = |name: &&str| {
        if pin.kind == ForgeKind::GitHub {
            names.iter().any(|candidate| candidate == *name)
        } else {
            names.iter().any(|line| line_defines(line, name))
        }
    };
    let present = MERGE_HOLD_LABELS
        .iter()
        .map(|l| l.name)
        .filter(exists)
        .collect();
    let missing = MERGE_HOLD_LABELS
        .iter()
        .map(|l| l.name)
        .filter(|n| !exists(n))
        .collect();
    LabelDefinitions::Read { present, missing }
}

// GitLab emits decorated raw lines. A label match is valid only when its next
// character cannot continue a label identifier (for example, a longer label).
fn line_defines(line: &str, name: &str) -> bool {
    line.match_indices(name).any(|(start, _)| {
        line[start + name.len()..]
            .chars()
            .next()
            .is_none_or(|ch| !(ch.is_alphanumeric() || matches!(ch, '-' | '_' | ':')))
    })
}

pub(crate) fn create_label_command(
    pin: &PinnedRepo,
    label: MergeHoldLabel,
) -> (&'static str, Vec<String>) {
    use crate::forge::ForgeKind;
    let args = match pin.kind {
        ForgeKind::GitHub => vec![
            "label".into(),
            "create".into(),
            label.name.into(),
            "-R".into(),
            pin.repo_arg(),
            "--color".into(),
            label.color.into(),
            "--description".into(),
            label.description.into(),
            "--force".into(),
        ],
        ForgeKind::GitLab => vec![
            "label".into(),
            "create".into(),
            "-R".into(),
            pin.repo_arg(),
            "--name".into(),
            label.name.into(),
            "--color".into(),
            label.color.into(),
            "--description".into(),
            label.description.into(),
        ],
        ForgeKind::None => Vec::new(),
    };
    (pin.kind.cli_name(), args)
}

pub(crate) fn provision_label_definitions(
    root: &Path,
    pin: &PinnedRepo,
    missing: &[&'static str],
) -> Vec<(&'static str, Result<(), String>)> {
    MERGE_HOLD_LABELS
        .iter()
        .filter(|label| missing.contains(&label.name))
        .map(|label| {
            let (cli, args) = create_label_command(pin, *label);
            let result = match run_forge_cli(root, cli, &args) {
                Ok((true, _)) => Ok(()),
                Ok((false, output))
                    if pin.kind == crate::forge::ForgeKind::GitLab
                        && output.to_ascii_lowercase().contains("already exists") =>
                {
                    Ok(())
                }
                Ok((false, output)) => {
                    Err(output.lines().next().unwrap_or("non-zero exit").to_string())
                }
                Err(err) => Err(err),
            };
            (label.name, result)
        })
        .collect()
}

/// Whether the forge change currently carries the Layer-2 merge-hold label.
///
/// This is intentionally a live forge read rather than marker metadata: an
/// operator may add the label directly, leaving no Layer-1 marker to inspect.
/// Errors are returned so callers keep their existing fail-closed behavior.
///
/// TASK-1455: the read is pinned to the project's own forge repo (`-R`), and
/// the answer is refused if the forge reports a change from any other repo.
// trace:TASK-1287 | ai:codex
// trace:TASK-1455 | ai:claude
pub(crate) fn label_present(project_root: &Path, pr: u64) -> Result<bool, String> {
    let kind = crate::forge::resolve_forge_kind(project_root);
    Ok(fetch_pinned_change(project_root, kind, pr)?.is_some_and(|c| c.hold_label))
}

/// The forge repository every merge-hold forge call is pinned to, resolved
/// once from the project's `origin` remote. A PR number is only meaningful
/// inside one repo; letting `gh`/`glab` infer the repo from the cwd or ambient
/// config means the same number can label, read or verdict a DIFFERENT repo's
/// change.
// trace:TASK-1455 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PinnedRepo {
    pub kind: crate::forge::ForgeKind,
    /// Lowercased host from `origin` (may be an ssh alias with no dot).
    pub host: String,
    /// Forge-native project path: `owner/repo` or `group/subgroup/project`.
    pub path: String,
}

impl PinnedRepo {
    /// Build the pin from an `origin` URL. `None` when the URL does not name
    /// a host AND an `owner/repo`-shaped path (a local-path remote, an empty
    /// remote, a bare host).
    // trace:TASK-1455 | ai:claude
    pub(crate) fn from_origin(kind: crate::forge::ForgeKind, origin: &str) -> Option<Self> {
        let host = crate::forge::forge_host_of(origin)?
            .trim()
            .to_ascii_lowercase();
        let path = crate::forge::project_path_of(origin)?;
        let path = path.trim_matches('/').to_string();
        let well_formed = !host.is_empty()
            && !host.contains('/')
            && !host.contains('\\')
            && path.contains('/')
            && path.split('/').all(|seg| !seg.trim().is_empty());
        well_formed.then_some(Self { kind, host, path })
    }

    /// An ssh-config alias (`git@github-work:o/r.git`) has no dot and is not a
    /// forge hostname; `gh`/`glab` resolve it themselves, so the pin names the
    /// repo path only and lets the CLI's default host apply.
    fn host_is_alias(&self) -> bool {
        !self.host.contains('.')
    }

    /// The `-R` value: `owner/repo` on the public forge (or an ssh alias),
    /// host-qualified elsewhere so a GitHub Enterprise / self-managed GitLab
    /// repo is never resolved against the public host.
    // trace:TASK-1455 | ai:claude
    pub(crate) fn repo_arg(&self) -> String {
        use crate::forge::ForgeKind;
        let public = match self.kind {
            ForgeKind::GitHub => "github.com",
            ForgeKind::GitLab => "gitlab.com",
            ForgeKind::None => "",
        };
        if self.host == public || self.host_is_alias() {
            self.path.clone()
        } else if self.kind == ForgeKind::GitLab {
            format!("https://{}/{}", self.host, self.path)
        } else {
            format!("{}/{}", self.host, self.path)
        }
    }

    /// Does a forge-reported change URL name THIS repo's change `pr`? The
    /// post-hoc check behind the pin: a response for any other repo or number
    /// is refused rather than trusted.
    // trace:TASK-1455 | ai:claude
    pub(crate) fn change_url_matches(&self, url: &str, pr: u64) -> bool {
        use crate::forge::ForgeKind;
        let Some(host) = crate::forge::forge_host_of(url) else {
            return false;
        };
        let Some(path) = crate::forge::project_path_of(url) else {
            return false;
        };
        let suffix = match self.kind {
            ForgeKind::GitHub => format!("/pull/{pr}"),
            ForgeKind::GitLab => format!("/-/merge_requests/{pr}"),
            ForgeKind::None => return false,
        };
        let Some(repo) = path.strip_suffix(&suffix) else {
            return false;
        };
        let host_ok = self.host_is_alias() || host.eq_ignore_ascii_case(&self.host);
        host_ok && repo.eq_ignore_ascii_case(&self.path)
    }
}

/// Resolve the pin for `kind` from the project's `origin`. `Ok(None)` means a
/// pure-git project (no forge, so no forge call is ever made). An unresolvable
/// repo on a real forge is an ERROR, never an unpinned call (PRIN-5: absent
/// evidence of which repo is not evidence that the ambient one is right).
// trace:TASK-1455 | ai:claude
pub(crate) fn resolve_pinned_repo(
    project_root: &Path,
    kind: crate::forge::ForgeKind,
) -> Result<Option<PinnedRepo>, String> {
    if kind == crate::forge::ForgeKind::None {
        return Ok(None);
    }
    pin_from_origin(kind, crate::forge::origin_url(project_root).as_deref()).map(Some)
}

fn pin_from_origin(
    kind: crate::forge::ForgeKind,
    origin: Option<&str>,
) -> Result<PinnedRepo, String> {
    let cli = kind.cli_name();
    let origin = origin.map(str::trim).filter(|o| !o.is_empty()).ok_or_else(|| {
        format!(
            "refusing to call `{cli}` unpinned: this project has no `origin` remote to name the forge repo"
        )
    })?;
    PinnedRepo::from_origin(kind, origin).ok_or_else(|| {
        format!(
            "refusing to call `{cli}` unpinned: could not resolve an owner/repo from origin `{origin}`"
        )
    })
}

/// The forge facts the merge-hold paths need about one change, read in ONE
/// pinned call and verified to belong to the pinned repo.
// trace:TASK-1455 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PinnedChange {
    pub merged: bool,
    /// BUG-1541: the full lifecycle state, so a sweep can tell an OPEN change
    /// from one CLOSED without merging. `merged` stays for existing callers
    /// and is exactly `state == ChangeState::Merged`.
    pub state: ChangeState,
    pub head_sha: Option<String>,
    pub hold_label: bool,
}

/// BUG-1541: the forge's lifecycle state for a change. `Unknown` (a missing
/// or unrecognised state word) is its own answer so a sweep fails CLOSED —
/// only a definite terminal state authorises dropping a marker.
// trace:BUG-1541 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChangeState {
    Open,
    Merged,
    ClosedUnmerged,
    Unknown,
}

impl ChangeState {
    /// Parse GitHub (`OPEN`/`CLOSED`/`MERGED`) and GitLab
    /// (`opened`/`closed`/`merged`/`locked`) state words.
    // trace:BUG-1541 | ai:claude
    pub(crate) fn parse(raw: Option<&str>) -> Self {
        match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("open" | "opened" | "locked") => Self::Open,
            Some("merged") => Self::Merged,
            Some("closed") => Self::ClosedUnmerged,
            _ => Self::Unknown,
        }
    }

    /// The terminal class, when the change can never merge (again): a marker
    /// on such a change protects nothing. `None` = open or unknown → live.
    // trace:BUG-1541 | ai:claude
    pub(crate) fn terminal(self) -> Option<TerminalState> {
        match self {
            Self::Merged => Some(TerminalState::Merged),
            Self::ClosedUnmerged => Some(TerminalState::ClosedUnmerged),
            Self::Open | Self::Unknown => None,
        }
    }
}

/// BUG-1541: why a marker is a phantom. Reported distinctly — a merged PR and
/// a PR closed without merging mean different things about how a hold ended.
// trace:BUG-1541 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalState {
    Merged,
    ClosedUnmerged,
}

impl TerminalState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::ClosedUnmerged => "closed-unmerged",
        }
    }

    pub(crate) fn describe(self) -> &'static str {
        match self {
            Self::Merged => "PR merged",
            Self::ClosedUnmerged => "PR closed without merging",
        }
    }
}

/// Read `pr` from the project's pinned forge repo. `Ok(None)` = pure-git.
// trace:TASK-1455 | ai:claude
pub(crate) fn fetch_pinned_change(
    project_root: &Path,
    kind: crate::forge::ForgeKind,
    pr: u64,
) -> Result<Option<PinnedChange>, String> {
    let Some(pin) = resolve_pinned_repo(project_root, kind)? else {
        return Ok(None);
    };
    fetch_pinned_change_with(project_root, &pin, pr, run_forge_cli_stdout).map(Some)
}

/// The argv for the pinned change read, per forge — pure for testing.
// trace:TASK-1455 | ai:claude
fn pinned_change_command(pin: &PinnedRepo, pr: u64) -> Option<(&'static str, Vec<String>)> {
    use crate::forge::ForgeKind;
    match pin.kind {
        ForgeKind::GitHub => Some((
            "gh",
            vec![
                "pr".into(),
                "view".into(),
                pr.to_string(),
                "-R".into(),
                pin.repo_arg(),
                "--json".into(),
                "url,state,headRefOid,labels".into(),
            ],
        )),
        // REST read (BUG-639: glab has no reliable `mr view --output json`).
        // The project is named IN the endpoint, so the read is pinned by
        // construction; `--hostname` pins a self-managed host.
        ForgeKind::GitLab => {
            let mut args = vec!["api".to_string()];
            if !pin.host_is_alias() {
                args.push("--hostname".into());
                args.push(pin.host.clone());
            }
            args.push(format!(
                "projects/{}/merge_requests/{pr}",
                pin.path.replace('/', "%2F")
            ));
            Some(("glab", args))
        }
        ForgeKind::None => None,
    }
}

fn fetch_pinned_change_with(
    project_root: &Path,
    pin: &PinnedRepo,
    pr: u64,
    runner: impl Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
) -> Result<PinnedChange, String> {
    let (cli, args) = pinned_change_command(pin, pr)
        .ok_or_else(|| "no forge to read a change from (pure-git)".to_string())?;
    let (ok, stdout) = runner(project_root, cli, &args)?;
    if !ok {
        return Err(format!("`{cli} {}` failed", args.join(" ")));
    }
    parse_pinned_change(pin, pr, &stdout)
}

fn parse_pinned_change(pin: &PinnedRepo, pr: u64, json: &str) -> Result<PinnedChange, String> {
    use crate::forge::ForgeKind;
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("could not parse forge response: {e}"))?;
    let url_key = if pin.kind == ForgeKind::GitLab {
        "web_url"
    } else {
        "url"
    };
    let url = value.get(url_key).and_then(|v| v.as_str()).unwrap_or("");
    if !pin.change_url_matches(url, pr) {
        return Err(format!(
            "refusing forge answer for #{pr}: it names `{}`, not a change in the pinned repo `{}`",
            if url.is_empty() { "<no url>" } else { url },
            pin.repo_arg()
        ));
    }
    let (state_key, head_key) = if pin.kind == ForgeKind::GitLab {
        ("state", "sha")
    } else {
        ("state", "headRefOid")
    };
    let state = ChangeState::parse(value.get(state_key).and_then(|v| v.as_str()));
    let merged = state == ChangeState::Merged;
    let head_sha = value
        .get(head_key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let hold_label = labels_json_contains(json, HOLD_LABEL)?;
    Ok(PinnedChange {
        merged,
        state,
        head_sha,
        hold_label,
    })
}

/// What the FORGE says about the Layer-2 label, as opposed to what the marker
/// recorded when it was written ([`LabelState`]). `Unknown` is its own state:
/// an unreachable forge is never rendered as the marker's claim.
// trace:TASK-189 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ForgeLabel {
    Present,
    Absent,
    /// Pure-git project: there is no forge label to have.
    NoForge,
    Unknown(String),
}

impl ForgeLabel {
    pub(crate) fn from_fetch(fetched: &Result<Option<PinnedChange>, String>) -> Self {
        match fetched {
            Ok(Some(c)) if c.hold_label => Self::Present,
            Ok(Some(_)) => Self::Absent,
            Ok(None) => Self::NoForge,
            Err(e) => Self::Unknown(e.lines().next().unwrap_or("").to_string()),
        }
    }

    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Absent => "absent",
            Self::NoForge => "no-forge",
            Self::Unknown(_) => "unknown",
        }
    }
}

/// Marker/label divergence for a LIVE hold (the marker exists by definition).
/// `Some(true)`: the forge has no label, so the server-side gate is not armed
/// even though the client chokepoint is. `Some(false)`: the two agree.
/// `None`: the forge could not be read — divergence is UNKNOWN, never reported
/// as agreement. Pure-git has no label layer, so it cannot diverge.
// trace:TASK-189 | ai:claude
pub(crate) fn label_diverged(forge: &ForgeLabel) -> Option<bool> {
    match forge {
        ForgeLabel::Present | ForgeLabel::NoForge => Some(false),
        ForgeLabel::Absent => Some(true),
        ForgeLabel::Unknown(_) => None,
    }
}

fn labels_json_contains(json: &str, wanted: &str) -> Result<bool, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("could not parse forge labels: {e}"))?;
    let labels = value
        .get("labels")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "forge response did not contain a labels array".to_string())?;
    Ok(labels.iter().any(|label| {
        label
            .as_str()
            .or_else(|| label.get("name").and_then(|v| v.as_str()))
            == Some(wanted)
    }))
}

/// BUG-1469: every OPEN change in the pinned repo that carries the hold
/// label. The label alone blocks a merge (it drives the required
/// merge-hold-gate check), so an inventory that only reads marker files
/// under-reports. `Ok(None)` = pure-git (no forge label layer). An error —
/// unpinnable repo, forge failure, a truncated page, an answer naming another
/// repo — is returned so the listing can REPORT the gap, never render the
/// smaller marker-only set as complete.
///
/// Only called from the explicit `aida merge-hold list`; never from
/// `aida awaiting` or its per-turn notice.
// trace:BUG-1469 | ai:claude
pub(crate) fn list_labeled_open_changes(
    project_root: &Path,
    kind: crate::forge::ForgeKind,
) -> Result<Option<Vec<u64>>, String> {
    let Some(pin) = resolve_pinned_repo(project_root, kind)? else {
        return Ok(None);
    };
    list_labeled_open_changes_with(project_root, &pin, run_forge_cli_stdout).map(Some)
}

/// Page ceiling for the labeled-change query. A full page is reported as
/// truncated rather than trusted as the whole set.
const LABELED_QUERY_LIMIT: usize = 100;

/// The argv for the pinned labeled-change query, per forge — pure for testing.
// trace:BUG-1469 | ai:claude
fn labeled_changes_command(pin: &PinnedRepo) -> Option<(&'static str, Vec<String>)> {
    use crate::forge::ForgeKind;
    match pin.kind {
        ForgeKind::GitHub => Some((
            "gh",
            vec![
                "pr".into(),
                "list".into(),
                "-R".into(),
                pin.repo_arg(),
                "--state".into(),
                "open".into(),
                "--label".into(),
                HOLD_LABEL.into(),
                "--limit".into(),
                LABELED_QUERY_LIMIT.to_string(),
                "--json".into(),
                "number,url,labels".into(),
            ],
        )),
        ForgeKind::GitLab => {
            let mut args = vec!["api".to_string()];
            if !pin.host_is_alias() {
                args.push("--hostname".into());
                args.push(pin.host.clone());
            }
            args.push(format!(
                "projects/{}/merge_requests?state=opened&labels={}&per_page={}",
                pin.path.replace('/', "%2F"),
                HOLD_LABEL.replace(':', "%3A"),
                LABELED_QUERY_LIMIT
            ));
            Some(("glab", args))
        }
        ForgeKind::None => None,
    }
}

fn list_labeled_open_changes_with(
    project_root: &Path,
    pin: &PinnedRepo,
    runner: impl Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
) -> Result<Vec<u64>, String> {
    let (cli, args) = labeled_changes_command(pin)
        .ok_or_else(|| "no forge to list labeled changes from (pure-git)".to_string())?;
    let (ok, stdout) = runner(project_root, cli, &args)?;
    if !ok {
        return Err(format!("`{cli} {}` failed", args.join(" ")));
    }
    parse_labeled_changes(pin, &stdout)
}

/// Parse the labeled-change listing. Every entry must name a change in the
/// pinned repo AND carry the label; any entry that does not refuses the whole
/// answer (TASK-1455: a response we cannot attribute is not evidence).
// trace:BUG-1469 | ai:claude
fn parse_labeled_changes(pin: &PinnedRepo, json: &str) -> Result<Vec<u64>, String> {
    use crate::forge::ForgeKind;
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| format!("could not parse labeled-change listing: {e}"))?;
    let entries = value
        .as_array()
        .ok_or_else(|| "labeled-change listing was not an array".to_string())?;
    if entries.len() >= LABELED_QUERY_LIMIT {
        return Err(format!(
            "labeled-change listing hit the {LABELED_QUERY_LIMIT}-row page limit; it may be truncated"
        ));
    }
    let (num_key, url_key) = if pin.kind == ForgeKind::GitLab {
        ("iid", "web_url")
    } else {
        ("number", "url")
    };
    let mut out = Vec::new();
    for entry in entries {
        let pr = entry
            .get(num_key)
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("labeled-change entry has no `{num_key}`"))?;
        let url = entry.get(url_key).and_then(|v| v.as_str()).unwrap_or("");
        if !pin.change_url_matches(url, pr) {
            return Err(format!(
                "refusing labeled-change listing: #{pr} names `{}`, not a change in the pinned repo `{}`",
                if url.is_empty() { "<no url>" } else { url },
                pin.repo_arg()
            ));
        }
        let labeled = labels_json_contains(&entry.to_string(), HOLD_LABEL)?;
        if !labeled {
            return Err(format!(
                "refusing labeled-change listing: #{pr} does not carry `{HOLD_LABEL}`"
            ));
        }
        if !out.contains(&pr) {
            out.push(pr);
        }
    }
    out.sort_unstable();
    Ok(out)
}

/// The reason shown for a hold that exists only as the forge label.
// trace:BUG-1469 | ai:claude
pub(crate) const LABEL_ONLY_REASON: &str = "label-only (no marker)";

/// The reason shown for a hold AIDA recorded placing whose marker then
/// disappeared with no clearance record — the tampering shape this spec exists
/// to make visible. Single-sourced so the `read_hold` synthetic value, the
/// `merge-hold clear` acknowledgement and the doctor finding all say the same
/// thing.
// trace:BUG-1693 | ai:claude
pub(crate) const MARKER_MISSING_REASON: &str =
    "hold marker missing without a recorded clearance (tampering suspected)";

/// Which half of a hold exists. `Marker` covers both the marker-only and the
/// marker+label shapes (the label column says which); `LabelOnly` is a hold a
/// seat placed with a bare forge label.
// trace:BUG-1469 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HoldSource {
    Marker,
    MarkerAndLabel,
    LabelOnly,
}

impl HoldSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Marker => "marker",
            Self::MarkerAndLabel => "marker+label",
            Self::LabelOnly => "label",
        }
    }
}

/// The label-only holds: open labeled changes with no marker. Pure.
// trace:BUG-1469 | ai:claude
pub(crate) fn label_only_holds(labeled: &[u64], marker_prs: &[u64]) -> Vec<u64> {
    let mut out: Vec<u64> = labeled
        .iter()
        .copied()
        .filter(|pr| !marker_prs.contains(pr))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Commit-sha-shaped tokens cited in a marker's prose: 7–40 hex chars with at
/// least one digit and one letter (so a bare PR number or an English word is
/// never read as a sha).
// trace:BUG-1562 | ai:claude
fn cited_shas(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|tok| {
            (7..=40).contains(&tok.len())
                && tok.bytes().all(|b| b.is_ascii_hexdigit())
                && tok.bytes().any(|b| b.is_ascii_digit())
                && tok.bytes().any(|b| b.is_ascii_alphabetic())
        })
        .map(str::to_ascii_lowercase)
        .collect()
}

/// BUG-1562: re-evaluate a marker's premise where it is machine-checkable,
/// at READ time — a reason is written once and the world moves on. `Some`
/// explains why the stated premise no longer holds. This only FLAGS: a hold
/// is a deliberate gate and its owner releases it; nothing here clears.
///
/// Checks, each skipped when its input is unknown (unknown is never stale):
///   - the marker cites a commit (its `target_head_sha`, else a sha in its
///     prose) and the PR head has moved past every cited commit;
///   - a REWORK hold's verdict (the typed `verdict_ref` when present, else a
///     spec named in its prose) is now APPROVED, or has been closed by a
///     merge — the refusal it quotes is no longer the reviewer's position;
///   - the spec the hold was placed for (typed `spec`, else a spec named in
///     its prose) is no longer running: its local status is terminal
///     (completed / rejected / superseded). `status_of` is a cheap, local
///     lookup (the cache), never a forge call.
// trace:BUG-1562 | ai:claude
pub(crate) fn premise_stale(
    record: &MergeHoldRecord,
    live_head: Option<&str>,
    verdict_of: impl Fn(&str) -> Option<crate::review_verdict::RecordedVerdict>,
    status_of: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    use crate::review_verdict::{same_reviewed_sha, short_sha};
    let mut why = Vec::new();
    let prose_specs = || crate::pr_ship::extract_spec_ids_from_text(&record.detail);
    if record.reason_kind == HoldReasonKind::Rework {
        let keys: Vec<String> = match &record.verdict_ref {
            Some(r) => vec![r.key.clone()],
            None => prose_specs(),
        };
        for spec in keys {
            let Some(verdict) = verdict_of(&spec) else {
                continue;
            };
            if let Some(merge) = verdict.closed_by_merge.as_deref() {
                why.push(format!(
                    "{spec}'s verdict was closed by merge {}",
                    short_sha(merge)
                ));
            } else if verdict.kind.approves() {
                why.push(match verdict.reviewed_sha.as_deref() {
                    Some(sha) => format!("{spec}'s verdict is now APPROVED at {}", short_sha(sha)),
                    None => format!("{spec}'s verdict is now APPROVED"),
                });
            }
        }
    }
    // BUG-1562: "is the spec this hold was placed for still running?"
    let specs: Vec<String> = match record.spec.as_deref().map(str::trim) {
        Some(spec) if !spec.is_empty() => vec![spec.to_ascii_uppercase()],
        _ => prose_specs(),
    };
    for spec in specs {
        if let Some(status) = status_of(&spec).filter(|s| crate::is_terminal_status_str(s)) {
            why.push(format!(
                "{spec} is no longer running (status {})",
                status.trim().to_ascii_lowercase()
            ));
        }
    }
    let cited: Vec<String> = match record.target_head_sha.as_deref().map(str::trim) {
        Some(sha) if !sha.is_empty() => vec![sha.to_ascii_lowercase()],
        _ => cited_shas(&record.detail),
    };
    if let Some(head) = live_head.map(str::trim).filter(|h| !h.is_empty()) {
        if !cited.is_empty() && !cited.iter().any(|sha| same_reviewed_sha(sha, head)) {
            why.push(format!(
                "marker cites {} but the PR head is now {}",
                short_sha(&cited[0]),
                short_sha(head)
            ));
        }
    }
    (!why.is_empty()).then(|| why.join("; "))
}

/// BUG-1532 criterion 4: whether a REFUSAL hold has been answered by a fresh
/// verdict at the PR's current head.
// trace:BUG-1532 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RefusalRelease {
    /// An APPROVED verdict recorded at exactly the current head, and no newer
    /// refusal under the same key. The hold's condition is met.
    Released(VerdictRef),
    /// Still refused. The text names the verdict on record and the sha it
    /// was recorded against — or, when the verdict carries no sha, names the
    /// MISSING PROVENANCE as the reason (criterion 11).
    Held(String),
}

/// TASK-1582: `aida merge-hold list`'s next-action line once a refusal hold's
/// release condition is MET. The hold is still ARMED, and `aida pr ship`
/// refuses an armed hold for any seat without the human integrity floor
/// (BUG-1566) — so pointing at ship alone sent agent seats in a loop between
/// the two commands. The line names the CLEAR step first (clear → ship). It
/// still names the human's one-step `aida pr ship`, because that path
/// releases the hold AND pins the merge to the verdict's head (BUG-1532);
/// after a plain clear the ship falls back to the approval-gate pin.
/// `short_sha` is the verdict's reviewed sha (already shortened) or `?`.
// trace:TASK-1582 | ai:antigravity
pub(crate) fn release_met_next_action(pr: u64, key: &str, short_sha: &str) -> String {
    format!(
        "release condition MET: APPROVED for {key} at {short_sha} — the hold is still armed; \
         next: `aida merge-hold clear {pr}` (human, recorded), then `aida pr ship {pr}`. \
         A human at a terminal may instead run `aida pr ship {pr}` alone, which releases the \
         hold and pins the merge to {short_sha}; agent seats are refused until it is cleared"
    )
}

/// The verdict keys a refusal hold is answered under: its typed
/// `verdict_ref` first, then the PR-keyed record (`PR-<n>`). Structural
/// only — the marker's prose is never parsed for a key (criterion 6).
// trace:BUG-1532 | ai:claude
pub(crate) fn refusal_verdict_keys(record: &MergeHoldRecord) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(r) = &record.verdict_ref {
        keys.push(r.key.clone());
    }
    let pr_key = format!("PR-{}", record.pr);
    if !keys.contains(&pr_key) {
        keys.push(pr_key);
    }
    keys
}

/// BUG-1532 criteria 4 + 11: a refusal hold is released ONLY by a fresh
/// verdict at the current head. Pure over its two readers so it is testable:
/// `current_of(key)` is the latest verdict recorded under `key`;
/// `at_sha(key, sha)` is the verdict recorded under `key` for commit `sha`
/// (`review_verdict::read_verdict_for_sha`).
///
/// Fails CLOSED: an unknown head, no verdict, a verdict at another sha, a
/// refusal at the head, or a verdict with NO sha all keep the hold. A
/// refusal that cannot be tied to a commit stays refused until a verdict
/// that DOES carry a sha is recorded at the head.
///
/// This decides whether the condition is MET. It never clears anything:
/// the release itself stays behind the human integrity floor (`aida pr
/// ship` / `aida merge-hold clear` at a terminal).
// trace:BUG-1532 | ai:claude
pub(crate) fn refusal_release(
    record: &MergeHoldRecord,
    live_head: Option<&str>,
    current_of: impl Fn(&str) -> Option<crate::review_verdict::RecordedVerdict>,
    at_sha: impl Fn(&str, &str) -> Option<crate::review_verdict::RecordedVerdict>,
) -> RefusalRelease {
    use crate::review_verdict::{same_reviewed_sha, short_sha, VerdictKind};
    let keys = refusal_verdict_keys(record);
    let Some(head) = live_head.map(str::trim).filter(|h| !h.is_empty()) else {
        return RefusalRelease::Held(
            "the PR's current head could not be read, so no verdict can be shown to cover it"
                .to_string(),
        );
    };
    for key in &keys {
        let Some(approval) = at_sha(key, head) else {
            continue;
        };
        let at_head = approval
            .reviewed_sha
            .as_deref()
            .is_some_and(|sha| same_reviewed_sha(sha, head));
        if approval.kind != VerdictKind::Approved || !at_head {
            continue;
        }
        // A newer refusal under the same key (at another sha) outranks an
        // older approval at this head.
        let superseded = current_of(key).is_some_and(|cur| {
            cur.kind.blocks_done()
                && !cur
                    .reviewed_sha
                    .as_deref()
                    .is_some_and(|sha| same_reviewed_sha(sha, head))
                && cur.recorded_at > approval.recorded_at
        });
        if superseded {
            continue;
        }
        return RefusalRelease::Released(VerdictRef::new(
            key,
            Some(record.pr),
            approval.reviewed_sha.clone(),
            approval.recorded_by.clone(),
        ));
    }
    // Held: name the verdict on record (criterion 3) — or its missing sha.
    let head_short = short_sha(head);
    let on_record = keys
        .iter()
        .find_map(|key| current_of(key).map(|v| (key.clone(), v)));
    let Some((key, verdict)) = on_record else {
        return RefusalRelease::Held(format!(
            "no verdict is recorded under {} — review the current head {head_short} and record a verdict with its sha",
            keys.join(" or ")
        ));
    };
    let by = verdict
        .recorded_by
        .as_deref()
        .map(|b| format!(", recorded by {b}"))
        .unwrap_or_default();
    let path = VerdictRef::new(&key, None, None, None).path();
    match verdict
        .reviewed_sha
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => RefusalRelease::Held(format!(
            "the verdict on record ({} for {key}{by}, {path}) carries NO reviewed sha — the missing \
             provenance, not the refusal itself, is why it cannot release: a verdict that cannot be \
             tied to a commit is answered only by a fresh verdict that records one. Re-review the \
             current head {head_short} and record the verdict with `--sha`",
            verdict.kind.label()
        )),
        Some(sha) => RefusalRelease::Held(format!(
            "the verdict on record is {} for {key} at {}{by} ({path}); no APPROVED verdict is \
             recorded at the current head {head_short} — re-review that head",
            verdict.kind.label(),
            short_sha(sha)
        )),
    }
}

/// BUG-1236: whether the `aida:merge-hold` label on the change mirrors the
/// marker, as RECORDED when the marker's label was last synced. TASK-189: this
/// is the marker's claim, not the forge's state — `aida merge-hold list` shows
/// it as `recorded:` beside a live forge read ([`ForgeLabel`]).
// trace:BUG-1236 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LabelState {
    Synced,
    Unsynced(String),
    Unknown,
}

/// Record the label sync state on an existing marker (no-op without one).
// trace:BUG-1236 | ai:claude
pub(crate) fn record_label_state(
    project_root: &Path,
    pr: u64,
    state: &LabelState,
) -> std::io::Result<()> {
    let path = hold_path(project_root, pr);
    let body = match std::fs::read_to_string(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if body.trim_start().starts_with('{') {
        let Some(mut record) = read_hold_record(project_root, pr) else {
            return Ok(());
        };
        record.label_state = Some(match state {
            LabelState::Synced => "synced".to_string(),
            LabelState::Unsynced(err) => format!("unsynced: {}", err.lines().next().unwrap_or("")),
            LabelState::Unknown => "unknown".to_string(),
        });
        return write_typed_hold(project_root, &record);
    }
    let reason = body.lines().next().unwrap_or("").trim();
    let line = match state {
        LabelState::Synced => "label: synced".to_string(),
        LabelState::Unsynced(err) => {
            format!("label: unsynced: {}", err.lines().next().unwrap_or(""))
        }
        LabelState::Unknown => String::new(),
    };
    let out = if line.is_empty() {
        format!("{reason}\n")
    } else {
        format!("{reason}\n{line}\n")
    };
    std::fs::write(path, out)
}

/// The recorded label state for `pr` (Unknown when the marker predates
/// BUG-1236 or carries no state line).
// trace:BUG-1236 | ai:claude
pub(crate) fn read_label_state(project_root: &Path, pr: u64) -> LabelState {
    let Ok(body) = std::fs::read_to_string(hold_path(project_root, pr)) else {
        return LabelState::Unknown;
    };
    if body.trim_start().starts_with('{') {
        return match read_hold_record(project_root, pr).and_then(|r| r.label_state) {
            Some(s) if s == "synced" => LabelState::Synced,
            Some(s) if s.starts_with("unsynced:") => {
                LabelState::Unsynced(s.trim_start_matches("unsynced:").trim().to_string())
            }
            _ => LabelState::Unknown,
        };
    }
    match body.lines().nth(1).map(str::trim) {
        Some("label: synced") => LabelState::Synced,
        Some(l) if l.starts_with("label: unsynced:") => {
            LabelState::Unsynced(l["label: unsynced:".len()..].trim().to_string())
        }
        _ => LabelState::Unknown,
    }
}

/// Mirror active hold state and its history/clearance proof to forge labels so
/// Layer 2 can reject label-only release. Failures are returned to callers;
/// otherwise a failed clearance-label update could look like a successful clear.
///
/// STORY-1165: forge-routed. GitHub → `gh pr edit --add-label/--remove-label`;
/// GitLab → `glab mr update --label/--unlabel`; pure-git → no-op (no forge to
/// labels. This lets both merge-hold-gate workflows require clearance after a
/// hold.
// trace:BUG-1167 | ai:claude (STORY-1165 forge-routes it)
// trace:BUG-1747 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LabelSyncError {
    /// A named label is not DEFINED in the repository. Deterministic and
    /// operator-fixable; never retried.
    DefinitionMissing {
        names: Vec<String>,
        detail: String,
    },
    Other(String),
}

impl LabelSyncError {
    pub(crate) fn is_definition_missing(&self) -> bool {
        matches!(self, Self::DefinitionMissing { .. })
    }
}

impl std::fmt::Display for LabelSyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DefinitionMissing { names, detail } => write!(f, "{detail}; missing label definition(s): {}. Run `aida merge-hold labels --create-missing`.", names.join(", ")),
            Self::Other(message) => f.write_str(message),
        }
    }
}

fn classify_label_sync_failure(stderr: &str, candidates: &[&str]) -> LabelSyncError {
    let names: Vec<String> = candidates
        .iter()
        .filter(|name| stderr.contains(&format!("'{}' not found", name)))
        .map(|name| (*name).to_string())
        .collect();
    if names.is_empty() {
        LabelSyncError::Other(
            stderr
                .lines()
                .next()
                .unwrap_or("label update failed")
                .trim()
                .to_string(),
        )
    } else {
        LabelSyncError::DefinitionMissing {
            names,
            detail: stderr
                .lines()
                .next()
                .unwrap_or("label definition missing")
                .trim()
                .to_string(),
        }
    }
}

pub(crate) fn sync_label(project_root: &Path, pr: u64, held: bool) -> Result<(), LabelSyncError> {
    sync_label_with(project_root, pr, held, run_forge_cli)
}

/// BUG-1236: the real sync loop with the forge CLI injected. Retries once on
/// failure, records the outcome on the marker (`label: synced` /
/// `label: unsynced: <err>`) when holding, and RETURNS the failure instead of
/// swallowing it — every caller prints it, `aida merge-hold list` shows it,
/// and `--fix` re-syncs it. Before this the label silently never landed on
/// three supervised PRs while the required merge-hold-gate check read pass.
// trace:BUG-1236 | ai:claude
pub(crate) fn sync_label_with(
    project_root: &Path,
    pr: u64,
    held: bool,
    runner: impl Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
) -> Result<(), LabelSyncError> {
    let kind = crate::forge::resolve_forge_kind(project_root);
    // TASK-1455: pin the repo; an unresolvable one refuses (and is recorded
    // as unsynced) instead of letting `gh`/`glab` guess from the cwd.
    let pin = match resolve_pinned_repo(project_root, kind) {
        Ok(Some(pin)) => pin,
        // pure-git has no forge to carry a label; the file marker still holds.
        Ok(None) => return Ok(()),
        Err(err) => {
            if held {
                let _ = record_label_state(project_root, pr, &LabelState::Unsynced(err.clone()));
            }
            return Err(LabelSyncError::Other(err));
        }
    };
    let Some((cli, args)) = sync_label_command(&pin, pr, held) else {
        return Ok(());
    };
    let mut last_err = String::new();
    let mut sync_error = None;
    for attempt in 0..2 {
        match runner(project_root, cli, &args) {
            Ok((true, _)) => {
                if held {
                    let _ = record_label_state(project_root, pr, &LabelState::Synced);
                }
                return Ok(());
            }
            Ok((false, stderr)) => {
                let classified = classify_label_sync_failure(
                    &stderr,
                    &[HOLD_LABEL, HOLD_RECORDED_LABEL, HOLD_CLEARED_LABEL],
                );
                if classified.is_definition_missing() {
                    last_err = stderr
                        .lines()
                        .next()
                        .unwrap_or("label definition missing")
                        .trim()
                        .to_string();
                    sync_error = Some(classified);
                    break;
                }
                last_err = stderr
                    .lines()
                    .next()
                    .unwrap_or("non-zero exit")
                    .trim()
                    .to_string();
            }
            Err(e) => last_err = e,
        }
        if attempt == 0 {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
    if held {
        let _ = record_label_state(project_root, pr, &LabelState::Unsynced(last_err.clone()));
    }
    let legacy = format!("`{cli} {}` failed: {last_err}", args.join(" "));
    Err(sync_error.unwrap_or(LabelSyncError::Other(legacy)))
}

fn run_forge_cli(
    project_root: &Path,
    cli: &str,
    args: &[String],
) -> Result<(bool, String), String> {
    let out = std::process::Command::new(cli)
        .current_dir(project_root)
        .args(args)
        .output_retrying_etxtbsy()
        .map_err(|e| format!("could not run {cli}: {e}"))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    ))
}

fn run_forge_cli_stdout(
    project_root: &Path,
    cli: &str,
    args: &[String],
) -> Result<(bool, String), String> {
    let out = std::process::Command::new(cli)
        .current_dir(project_root)
        .args(args)
        .output_retrying_etxtbsy()
        .map_err(|e| format!("could not run {cli}: {e}"))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
    ))
}

/// The CLI + argv for mirroring the merge-hold label on a change, per forge —
/// kept pure so the routing is unit-testable. `None` = no forge to label
/// (pure-git).
// trace:STORY-1165 | ai:claude
// trace:TASK-1455 | ai:claude
fn sync_label_command(
    pin: &PinnedRepo,
    pr: u64,
    held: bool,
) -> Option<(&'static str, Vec<String>)> {
    use crate::forge::ForgeKind;
    match pin.kind {
        ForgeKind::GitHub => {
            let (flag, label) = if held {
                ("--add-label", format!("{HOLD_LABEL},{HOLD_RECORDED_LABEL}"))
            } else {
                ("--remove-label", HOLD_LABEL.to_string())
            };
            let mut args = vec![
                "pr".into(),
                "edit".into(),
                pr.to_string(),
                "-R".into(),
                pin.repo_arg(),
                flag.into(),
                label,
            ];
            if held {
                // Both partial-failure interleavings fail closed: {hold, recorded,
                // cleared} fails gate rule 1; {recorded} fails rule 2. No
                // interleaving yields a passing gate.
                // trace:BUG-1693 | ai:codex
                args.extend(["--remove-label".into(), HOLD_CLEARED_LABEL.into()]);
            } else {
                args.extend(["--add-label".into(), HOLD_CLEARED_LABEL.into()]);
            }
            Some(("gh", args))
        }
        ForgeKind::GitLab => {
            let (flag, label) = if held {
                ("--label", format!("{HOLD_LABEL},{HOLD_RECORDED_LABEL}"))
            } else {
                ("--unlabel", HOLD_LABEL.to_string())
            };
            let mut args = vec![
                "mr".into(),
                "update".into(),
                pr.to_string(),
                "-R".into(),
                pin.repo_arg(),
                flag.into(),
                label,
            ];
            if held {
                // Both partial-failure interleavings fail closed: {hold, recorded,
                // cleared} fails gate rule 1; {recorded} fails rule 2. No
                // interleaving yields a passing gate.
                // trace:BUG-1693 | ai:codex
                args.extend(["--unlabel".into(), HOLD_CLEARED_LABEL.into()]);
            } else {
                args.extend(["--label".into(), HOLD_CLEARED_LABEL.into()]);
            }
            Some(("glab", args))
        }
        ForgeKind::None => None,
    }
}

// ── BUG-1774: the corpus-derived half of the merge chokepoint ───────────────
//
// A refusing verdict used to hold a PR only when it was recorded through
// `handle_review_record_at`, the one producer that stamps the marker + label.
// Every other producer of a verdict artifact (`adopt_direct_write`, the drain's
// `stamp_pr_review_verdict`, a hand-written file) armed nothing. Enforcement
// state is therefore DERIVED from the verdict corpus here, at the chokepoint
// every AIDA merge funnels through, rather than stamped once per writer. The
// marker and the label stay as mirrors (ADR-37's server half is a GitHub
// required check that can read a label but not the local corpus), and they are
// refreshed from the SAME predicate the gate refuses on, so they cannot
// disagree with the corpus.
// trace:BUG-1774 | ai:claude

/// What the verdict corpus says about merging a PR at its current head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CorpusMergeGate {
    /// No live verdict evidence, or the corpus cleanly approves the head.
    Clear,
    /// The merge is refused. `arm` carries the definite blocking verdict AT
    /// the evaluated head when there is one — the only evidence the mirrors
    /// (marker + label) are armed from. A fail-closed refusal with no definite
    /// at-head blocker (unknown head, stale verdicts, conflicting corpus)
    /// refuses without arming: mirroring uncertainty as state would put the
    /// label on every PR whose head cannot be read.
    Refuse {
        message: String,
        arm: Option<CorpusArm>,
    },
}

/// The definite at-head blocking verdict backing a refusal (AC4: whatever
/// refreshes the mirror does so from the gate's own predicate result).
// trace:BUG-1774 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CorpusArm {
    pub key: String,
    pub sha: String,
    pub verdict_raw: String,
    pub recorded_by: Option<String>,
}

/// The gate's release syntax, spoken in every refusal (a refusal must name the
/// repair the caller can actually run).
fn corpus_release_hint(pr: u64, key: &str) -> String {
    format!(
        "The hold lifts only when the findings are addressed on a NEW head and a fresh \
         APPROVED verdict is recorded at it — `aida review record {key} --verdict approved \
         --sha <new-head> --pr {pr}` after reviewing it — then merge again."
    )
}

/// Pure decision over the gathered verdict bodies. This is the fail-closed
/// MERGE form, not BUG-1773's fail-open view form (`corpus_hold_at_head`):
/// once live verdict evidence exists, only a clean APPROVED reconciliation at
/// the current head lets the merge proceed. An unknown head, a verdict left at
/// an older head, an approval recorded at the OLD rejected sha, and a corpus
/// that cannot be reconciled all refuse (parent AC2/AC3).
///
/// A verdict closed by a merge is the original verdict PLUS a record that the
/// branch moved on (BUG-1529) — history, not evidence about this head — so it
/// is excluded before the corpus is judged, exactly as the view excludes it.
// trace:BUG-1774 | ai:claude
pub(crate) fn corpus_merge_gate(
    pr: u64,
    bodies: &[(String, String)],
    head: Option<&str>,
) -> CorpusMergeGate {
    use crate::review_verdict as rv;
    let live: Vec<&(String, String)> = bodies
        .iter()
        .filter(|(_, body)| rv::parse_recorded_verdict(body).is_none_or(|v| !v.is_closed()))
        .collect();
    if live.is_empty() {
        return CorpusMergeGate::Clear;
    }
    let first_key = live[0].0.clone();
    let head = head.map(str::trim).filter(|h| !h.is_empty());
    let Some(head) = head else {
        // AC2: an artifact on file + an unreadable head = the gate cannot show
        // the recorded verdict does not cover the commit about to land.
        return CorpusMergeGate::Refuse {
            message: format!(
                "PR-{pr} has a recorded review verdict ({first_key}) but its current head could \
                 not be read, so the merge fails closed. {}",
                corpus_release_hint(pr, &first_key)
            ),
            arm: None,
        };
    };
    match rv::reconcile_artifacts_for_sha(live.iter().map(|(_, b)| b.as_str()), head) {
        Ok(Some(kind)) if kind.approves() => CorpusMergeGate::Clear,
        Ok(Some(kind)) => {
            // A blocker recorded against this exact head — the definite case
            // the mirrors are armed from, whatever producer wrote the file.
            let blocking = live.iter().find_map(|(key, body)| {
                rv::parse_recorded_verdict(body)
                    .filter(|v| {
                        v.kind.blocks_done()
                            && v.reviewed_sha
                                .as_deref()
                                .is_some_and(|s| rv::same_reviewed_sha(s, head))
                    })
                    .map(|v| (key.clone(), v))
            });
            let (key, raw, recorded_by) = match blocking {
                Some((key, v)) => (key, v.raw.clone(), v.recorded_by.clone()),
                None => (
                    first_key.clone(),
                    kind.canonical().unwrap_or("request-changes").to_string(),
                    None,
                ),
            };
            let message = format!(
                "PR-{pr} has an outstanding {raw} verdict for {key} at its current head {} — \
                 the verdict corpus holds the merge regardless of which producer wrote the \
                 artifact. {}",
                rv::short_sha(head),
                corpus_release_hint(pr, &key)
            );
            CorpusMergeGate::Refuse {
                message,
                arm: Some(CorpusArm {
                    key,
                    sha: head.to_string(),
                    verdict_raw: raw,
                    recorded_by,
                }),
            }
        }
        // Verdict evidence exists but nothing cleanly approves this head: a
        // refusal left at an older sha (a head move alone does not lift the
        // hold) or an approval recorded at the OLD rejected sha (parent AC3).
        Ok(None) => CorpusMergeGate::Refuse {
            message: format!(
                "PR-{pr}'s recorded review verdicts ({first_key}) do not approve its current \
                 head {} — a head move alone does not lift a recorded refusal, and an approval \
                 at an older commit is not evidence this head was reviewed. {}",
                rv::short_sha(head),
                corpus_release_hint(pr, &first_key)
            ),
            arm: None,
        },
        Err(message) => CorpusMergeGate::Refuse {
            message: format!(
                "PR-{pr}'s verdict corpus cannot be reconciled at its current head {}: {message} \
                 — a human must resolve the conflicting recordings before this merges. {}",
                rv::short_sha(head),
                corpus_release_hint(pr, &first_key)
            ),
            arm: None,
        },
    }
}

/// Gather the `(key, body)` pairs the gate judges: the PR-keyed record plus
/// each spec hint, under each root (a worktree and the main clone can both
/// hold `.aida/review-verdicts/`); a file reachable under two roots is read
/// once. `Err` = an artifact exists but could not be read — AC2 fails closed.
// trace:BUG-1774 | ai:claude
pub(crate) fn corpus_gate_bodies(
    roots: &[&Path],
    pr: u64,
    spec_hints: &[String],
) -> Result<Vec<(String, String)>, String> {
    let mut keys: Vec<String> = Vec::new();
    if pr > 0 {
        keys.push(format!("PR-{pr}"));
    }
    for id in spec_hints {
        let id = id.trim().to_ascii_uppercase();
        if !id.is_empty() && !keys.contains(&id) {
            keys.push(id);
        }
    }
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut out = Vec::new();
    for root in roots {
        for key in &keys {
            let path = crate::review_verdict::verdict_path(root, key);
            if std::fs::symlink_metadata(&path).is_err() {
                continue;
            }
            let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if seen.contains(&canon) {
                continue;
            }
            seen.push(canon);
            match std::fs::read_to_string(&path) {
                Ok(body) => out.push((key.clone(), body)),
                Err(e) => {
                    return Err(format!(
                        "verdict artifact {} exists but could not be read ({e})",
                        path.display()
                    ))
                }
            }
        }
    }
    Ok(out)
}

/// The chokepoint entry every `Forge::merge_change` implementation calls.
/// `head` is resolved lazily — a PR with no verdict artifacts pays no forge
/// read. On a definite at-head refusal the mirrors are refreshed best-effort
/// from the gate's own `CorpusArm` (never from a second predicate), and the
/// refusal stands whether or not the mirrors landed.
// trace:BUG-1774 | ai:claude
pub(crate) fn enforce_corpus_gate_before_merge(
    project_root: &Path,
    pr: u64,
    spec_hints: &[String],
    head: impl FnOnce() -> Option<String>,
) -> anyhow::Result<()> {
    let main_root = crate::main_worktree_root_from(project_root);
    let mut roots: Vec<&Path> = vec![project_root];
    if main_root != project_root {
        roots.push(main_root.as_path());
    }
    let bodies = match corpus_gate_bodies(&roots, pr, spec_hints) {
        Ok(bodies) => bodies,
        Err(why) => anyhow::bail!(
            "PR-{pr} was not merged: {why} — the merge fails closed on an unreadable verdict \
             artifact. Restore or repair the file, then merge again."
        ),
    };
    if bodies.is_empty() {
        return Ok(());
    }
    let head = head();
    match corpus_merge_gate(pr, &bodies, head.as_deref()) {
        CorpusMergeGate::Clear => Ok(()),
        CorpusMergeGate::Refuse { message, arm } => {
            if let Some(arm) = arm.filter(|_| pr > 0) {
                arm_corpus_mirrors(project_root, pr, &arm);
            }
            anyhow::bail!(message)
        }
    }
}

/// Refresh the marker + label mirrors from the gate's definite at-head
/// refusal. Best-effort: a mirror failure is reported, never escalated — the
/// chokepoint refusal already stands. `write_typed_hold` composes with any
/// existing marker, so this programmatic Rework write cannot displace a
/// stricter hold (a Recusal stays primary — BUG-1691).
// trace:BUG-1774 | ai:claude
fn arm_corpus_mirrors(project_root: &Path, pr: u64, arm: &CorpusArm) {
    let reason = format!(
        "{} for {} at {}",
        arm.verdict_raw,
        arm.key,
        crate::review_verdict::short_sha(&arm.sha)
    );
    let mut hold = typed_hold(pr, HoldReasonKind::Rework, &reason, Some(arm.sha.clone()));
    hold.verdict_ref = Some(VerdictRef::new(
        &arm.key,
        Some(pr),
        Some(arm.sha.clone()),
        arm.recorded_by.clone(),
    ));
    if !arm.key.starts_with("PR-") {
        hold.spec = Some(arm.key.clone());
    }
    hold.release_condition = Some(format!(
        "a fresh APPROVED verdict for {} recorded at PR #{pr}'s current head; then a human ships it",
        arm.key
    ));
    if let Err(err) = write_typed_hold(project_root, &hold) {
        eprintln!(
            "  warning: could not mirror the corpus refusal into PR-{pr}'s merge-hold marker \
             ({err}) — the chokepoint refusal stands on the verdict corpus alone"
        );
        return;
    }
    if let Err(err) = sync_label(project_root, pr, true) {
        eprintln!(
            "  warning: merge-hold label not applied on PR-{pr}: {err} — the local merge \
             chokepoint remains armed; run `aida merge-hold list --fix`"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // TASK-1582 acceptance 1: with a hold armed, the next-action line names
    // `aida merge-hold clear` BEFORE `aida pr ship` (no circular hints).
    // trace:TASK-1582.ac14557d | ai:antigravity
    #[test]
    fn task_1582_release_met_next_action_names_clear_before_ship() {
        let line = release_met_next_action(2386, "TASK-1", "abc1234");
        let clear = line
            .find("`aida merge-hold clear 2386`")
            .unwrap_or_else(|| panic!("no clear step: {line}"));
        let ship = line
            .find("`aida pr ship 2386`")
            .unwrap_or_else(|| panic!("no ship step: {line}"));
        assert!(clear < ship, "clear must come first: {line}");
        assert!(line.contains("APPROVED for TASK-1 at abc1234"), "{line}");
        assert!(line.contains("still armed"), "{line}");
        assert!(line.contains("pins the merge to abc1234"), "{line}");
        assert!(!line.contains("may now `aida pr ship"), "{line}");
    }

    #[cfg(unix)]
    fn shell_args(script: String) -> Vec<String> {
        vec!["-c".into(), script]
    }

    #[cfg(unix)]
    fn kill_if_alive(pid: libc::pid_t) {
        // SAFETY: signal 0 only probes the child pid; SIGKILL is limited to
        // the pid started by this test.
        if unsafe { libc::kill(pid, 0) } == 0 {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }

    #[cfg(unix)]
    struct ChildPidGuard(libc::pid_t);

    #[cfg(unix)]
    impl Drop for ChildPidGuard {
        fn drop(&mut self) {
            kill_if_alive(self.0);
        }
    }

    // The definition probe intentionally has a distinct bounded runner from
    // hold placement; timeout coverage keeps the doctor call from hanging.
    // trace:BUG-1747 | ai:codex
    #[cfg(unix)]
    #[test]
    fn forge_definition_probe_times_out_near_its_injected_bound() {
        let dir = tempfile::tempdir().unwrap();
        let timeout = std::time::Duration::from_millis(200);
        let started = std::time::Instant::now();
        let result = run_forge_cli_stdout_bounded_with(
            dir.path(),
            "sh",
            &shell_args("sleep 30".into()),
            timeout,
        );
        let elapsed = started.elapsed();
        assert!(result.is_err(), "expected timeout, got {result:?}");
        assert_eq!(
            result.unwrap_err(),
            format!("timed out after {}s", timeout.as_secs())
        );
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "call took {elapsed:?}"
        );
    }

    // trace:BUG-1747 | ai:codex
    #[cfg(unix)]
    #[test]
    fn forge_definition_probe_timeout_kills_the_whole_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("grandchild.pid");
        let script = format!("sleep 30 & echo $! > '{}' ; wait", pidfile.display());
        let timeout = std::time::Duration::from_millis(200);
        let result =
            run_forge_cli_stdout_bounded_with(dir.path(), "sh", &shell_args(script), timeout);
        assert!(result.is_err(), "expected timeout, got {result:?}");
        let pid: libc::pid_t = std::fs::read_to_string(pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let _child_guard = ChildPidGuard(pid);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            // SAFETY: signal 0 only probes whether the test grandchild exists.
            if unsafe { libc::kill(pid, 0) } == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "grandchild {pid} survived timeout"
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }

    // trace:BUG-1747 | ai:codex
    #[cfg(unix)]
    #[test]
    fn forge_definition_probe_returns_stdout_without_waiting_for_bound() {
        let dir = tempfile::tempdir().unwrap();
        let timeout = std::time::Duration::from_secs(5);
        let started = std::time::Instant::now();
        let result = run_forge_cli_stdout_bounded_with(
            dir.path(),
            "sh",
            &shell_args("printf hello".into()),
            timeout,
        );
        let elapsed = started.elapsed();
        assert_eq!(result.unwrap(), (true, "hello".into()));
        assert!(elapsed < timeout / 2, "call took {elapsed:?}");
    }

    // Unlike run_forge_cli_stdout (used for hold placement), this diagnostic
    // probe returns stderr on failure so LabelDefinitions::Unknown keeps gh's
    // actionable error text instead of becoming Unknown("").
    // trace:BUG-1747 | ai:codex
    #[cfg(unix)]
    #[test]
    fn forge_definition_probe_returns_stderr_for_failing_child() {
        let dir = tempfile::tempdir().unwrap();
        let result = run_forge_cli_stdout_bounded_with(
            dir.path(),
            "sh",
            &shell_args("echo OUT; echo ERR >&2; exit 3".into()),
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        assert!(!result.0);
        assert!(result.1.contains("ERR"), "{}", result.1);
        assert!(!result.1.contains("OUT"), "{}", result.1);
    }

    // trace:BUG-1693 | ai:codex
    #[test]
    fn unrecorded_manual_marker_removal_is_detected_but_recorded_clear_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let record = typed_hold(4321, HoldReasonKind::Supervision, "fixture", None);
        write_typed_hold(root, &record).unwrap();
        let mut placed = crate::events::Event::new(
            None,
            "",
            crate::events::EventKind::MergeHoldChanged {
                pr: 4321,
                placed: true,
                reason: Some("fixture".into()),
            },
        );
        std::fs::remove_file(hold_path(root, 4321)).unwrap();
        assert_eq!(unrecorded_marker_removals(root), vec![4321]);
        assert!(read_hold(root, 4321).is_some());

        record_clearance(root, &record, &PrincipalIdentity::human("joe")).unwrap();
        placed.kind = crate::events::EventKind::MergeHoldChanged {
            pr: 4321,
            placed: false,
            reason: Some("fixture cleared".into()),
        };
        crate::events::emit(root, &placed);
        assert!(unrecorded_marker_removals(root).is_empty());
    }

    // BUG-1693: `read_hold` gained a SECOND meaning — it now answers `Some`
    // for a PR with no marker at all when AIDA recorded placing one. Every
    // caller reads that as "held", which is the fail-closed behaviour we want
    // at the merge chokepoint (forge.rs refuses the merge). Pin both halves of
    // that invariant, and pin that a clearance recorded by the human-gated
    // path is what re-opens the PR — otherwise a tampered PR is unmergeable
    // through AIDA forever.
    // trace:BUG-1693 | ai:claude
    #[test]
    fn tampered_hold_reads_as_held_until_a_clearance_is_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let record = typed_hold(8801, HoldReasonKind::Supervision, "supervised spec", None);
        write_typed_hold(root, &record).unwrap();
        std::fs::remove_file(hold_path(root, 8801)).unwrap();

        // Fail closed: the merge chokepoint must still see a hold, and it must
        // say WHY, not repeat the deleted marker's reason as if it were read.
        assert_eq!(
            read_hold(root, 8801).as_deref(),
            Some(MARKER_MISSING_REASON)
        );

        // …but the synthetic value must not masquerade as a hold RECORD:
        // `list_holds` walks the marker directory, so a deleted marker has no
        // entry and cannot be reported as a live hold with a real reason.
        assert!(
            list_holds(root).iter().all(|(pr, _)| *pr != 8801),
            "a missing marker must not appear as a live hold record"
        );
        assert!(read_hold_record(root, 8801).is_none());

        // The human-gated clearance is the door back out. After it, the PR
        // reads unheld and the tampering report stops.
        record_clearance(
            root,
            &typed_hold(8801, HoldReasonKind::Unknown, MARKER_MISSING_REASON, None),
            &PrincipalIdentity::human("joe"),
        )
        .unwrap();
        assert!(unrecorded_marker_removals(root).is_empty());
        assert!(read_hold(root, 8801).is_none());
    }

    // STORY-1165: the label mirror must route to the right forge CLI — gh for
    // GitHub, glab for GitLab (mr update --label/--unlabel), nothing for pure-git.
    fn pin(kind: crate::forge::ForgeKind, origin: &str) -> PinnedRepo {
        PinnedRepo::from_origin(kind, origin).expect("origin must pin")
    }

    // STORY-1165: the label mirror must route to the right forge CLI — gh for
    // GitHub, glab for GitLab (mr update --label/--unlabel), nothing for pure-git.
    // TASK-1455: and every routed call carries the pinned `-R <repo>`.
    #[test]
    fn sync_label_command_routes_per_forge() {
        use crate::forge::ForgeKind;
        let gh = pin(ForgeKind::GitHub, "git@github.com:o/r.git");
        let (cli, args) = sync_label_command(&gh, 42, true).unwrap();
        assert_eq!(cli, "gh");
        assert!(
            args.contains(&"--add-label".to_string())
                && args.contains(&format!("{HOLD_LABEL},{HOLD_RECORDED_LABEL}"))
        );
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--remove-label" && w[1] == HOLD_CLEARED_LABEL));
        assert!(
            args.windows(2).any(|w| w[0] == "-R" && w[1] == "o/r"),
            "{args:?}"
        );

        let gl = pin(ForgeKind::GitLab, "https://gitlab.com/g/sub/p.git");
        let (cli, args) = sync_label_command(&gl, 42, true).unwrap();
        assert_eq!(cli, "glab");
        assert!(args.contains(&"mr".to_string()) && args.contains(&"update".to_string()));
        assert!(
            args.contains(&"--label".to_string())
                && args.contains(&format!("{HOLD_LABEL},{HOLD_RECORDED_LABEL}"))
        );
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--unlabel" && w[1] == HOLD_CLEARED_LABEL));
        assert!(
            args.windows(2).any(|w| w[0] == "-R" && w[1] == "g/sub/p"),
            "{args:?}"
        );

        let (_, args) = sync_label_command(&gl, 42, false).unwrap();
        assert!(
            args.contains(&"--unlabel".to_string()),
            "unheld → remove the label"
        );
        assert!(args.contains(&HOLD_LABEL.to_string()));
        assert!(args.contains(&"--label".to_string()));
        assert!(args.contains(&HOLD_CLEARED_LABEL.to_string()));

        let none = PinnedRepo {
            kind: ForgeKind::None,
            host: "example.org".into(),
            path: "o/r".into(),
        };
        assert!(
            sync_label_command(&none, 42, true).is_none(),
            "pure-git has no forge to label"
        );
    }

    // trace:BUG-1747 | ai:codex
    #[test]
    fn held_github_label_update_is_one_atomic_comma_list() {
        let repo = pin(crate::forge::ForgeKind::GitHub, "git@github.com:o/r.git");
        let (_, args) = sync_label_command(&repo, 9, true).unwrap();
        // An undefined member of the comma list exits 1 and applies neither label,
        // so provisioning must cover all three definitions before any hold is placed.
        assert_eq!(
            args[args.iter().position(|arg| arg == "--add-label").unwrap() + 1],
            format!("{HOLD_LABEL},{HOLD_RECORDED_LABEL}")
        );
    }

    // trace:BUG-1747 | ai:codex
    #[test]
    fn label_sync_failure_classification_matches_forge_diagnostics() {
        assert_eq!(
            classify_label_sync_failure(
                "'aida:merge-hold-cleared' not found",
                &[HOLD_LABEL, HOLD_RECORDED_LABEL, HOLD_CLEARED_LABEL]
            ),
            LabelSyncError::DefinitionMissing {
                names: vec![HOLD_CLEARED_LABEL.into()],
                detail: "'aida:merge-hold-cleared' not found".into()
            }
        );
        assert!(matches!(
            classify_label_sync_failure("HTTP 502", &[HOLD_LABEL]),
            LabelSyncError::Other(_)
        ));
        assert_eq!(
            classify_label_sync_failure(
                "'aida:merge-hold' not found and 'aida:merge-hold-recorded' not found",
                &[HOLD_LABEL, HOLD_RECORDED_LABEL, HOLD_CLEARED_LABEL]
            ),
            LabelSyncError::DefinitionMissing {
                names: vec![HOLD_LABEL.into(), HOLD_RECORDED_LABEL.into()],
                detail: "'aida:merge-hold' not found and 'aida:merge-hold-recorded' not found"
                    .into()
            }
        );
    }

    // trace:BUG-1747 | ai:codex
    #[test]
    fn label_definition_failure_skips_retry_but_transient_retries() {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["init", "-q"])
            .status()
            .unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["remote", "add", "origin", "git@github.com:o/r.git"])
            .status()
            .unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            dir.path().join(".aida/config.toml"),
            "[forge]\nprovider = \"github\"\n",
        )
        .unwrap();
        let definition_calls = std::cell::Cell::new(0);
        let err = sync_label_with(dir.path(), 42, true, |_, _, _| {
            definition_calls.set(definition_calls.get() + 1);
            Ok((false, "'aida:merge-hold-cleared' not found".into()))
        })
        .unwrap_err();
        assert_eq!(definition_calls.get(), 1);
        assert!(err.is_definition_missing());
        let transient_calls = std::cell::Cell::new(0);
        let _ = sync_label_with(dir.path(), 43, false, |_, _, _| {
            transient_calls.set(transient_calls.get() + 1);
            Ok((false, "HTTP 502".into()))
        });
        assert_eq!(transient_calls.get(), 2);
    }

    // trace:BUG-1747 | ai:codex
    #[test]
    fn label_definition_failure_keeps_the_local_hold_marker() {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["init", "-q"])
            .status()
            .unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["remote", "add", "origin", "git@github.com:o/r.git"])
            .status()
            .unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            dir.path().join(".aida/config.toml"),
            "[forge]\nprovider = \"github\"\n",
        )
        .unwrap();
        write_typed_hold(
            dir.path(),
            &typed_hold(44, HoldReasonKind::Supervision, "test", None),
        )
        .unwrap();
        let err = sync_label_with(dir.path(), 44, true, |_, _, _| {
            Ok((false, "'aida:merge-hold-cleared' not found".into()))
        })
        .unwrap_err();
        assert!(hold_path(dir.path(), 44).exists());
        assert!(read_hold(dir.path(), 44).is_some());
        assert!(matches!(
            read_label_state(dir.path(), 44),
            LabelState::Unsynced(_)
        ));
        assert!(err.is_definition_missing());
    }

    // trace:BUG-1747 | ai:codex
    #[test]
    fn merge_hold_label_create_argv_covers_registry_with_force_and_metadata() {
        let repo = pin(crate::forge::ForgeKind::GitHub, "git@github.com:o/r.git");
        for label in MERGE_HOLD_LABELS {
            let (cli, args) = create_label_command(&repo, label);
            assert_eq!(cli, "gh");
            assert_eq!(args[0..3], ["label", "create", label.name]);
            assert!(args
                .windows(2)
                .any(|w| w == ["-R", repo.repo_arg().as_str()]));
            assert!(args.windows(2).any(|w| w == ["--color", label.color]));
            assert!(args
                .windows(2)
                .any(|w| w == ["--description", label.description]));
            assert!(args.contains(&"--force".into()));
        }
    }

    // trace:BUG-1747 | ai:codex
    #[test]
    fn repo_label_definition_probe_maps_output_without_treating_failures_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let repo = pin(crate::forge::ForgeKind::GitHub, "git@github.com:o/r.git");
        let all = serde_json::to_string(
            &MERGE_HOLD_LABELS
                .iter()
                .map(|l| serde_json::json!({"name": l.name}))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let LabelDefinitions::Read { missing, .. } =
            label_definitions_with(dir.path(), &repo, |_, _, _| Ok((true, all.clone())))
        else {
            panic!("expected read")
        };
        assert!(missing.is_empty());
        let partial = format!(r#"[{{"name":"{}"}}]"#, HOLD_LABEL);
        let LabelDefinitions::Read { missing, .. } =
            label_definitions_with(dir.path(), &repo, |_, _, _| Ok((true, partial.clone())))
        else {
            panic!("expected read")
        };
        assert_eq!(missing, vec![HOLD_RECORDED_LABEL, HOLD_CLEARED_LABEL]);
        assert!(matches!(
            label_definitions_with(dir.path(), &repo, |_, _, _| Ok((false, String::new()))),
            LabelDefinitions::Unknown(_)
        ));
        assert!(matches!(
            label_definitions_with(dir.path(), &repo, |_, _, _| Ok((true, "not json".into()))),
            LabelDefinitions::Unknown(_)
        ));
    }

    // trace:BUG-1747 | ai:codex
    #[test]
    fn github_probe_does_not_treat_longer_labels_as_the_base_label() {
        let dir = tempfile::tempdir().unwrap();
        let repo = pin(crate::forge::ForgeKind::GitHub, "git@github.com:o/r.git");
        let only_long_names =
            r#"[{"name":"aida:merge-hold-recorded"},{"name":"aida:merge-hold-cleared"}]"#;
        let LabelDefinitions::Read { present, missing } =
            label_definitions_with(dir.path(), &repo, |_, _, _| {
                Ok((true, only_long_names.into()))
            })
        else {
            panic!("expected read")
        };
        assert_eq!(missing, vec![HOLD_LABEL]);
        assert_eq!(present, vec![HOLD_RECORDED_LABEL, HOLD_CLEARED_LABEL]);
    }

    // trace:BUG-1747 | ai:codex
    #[test]
    fn gitlab_probe_does_not_treat_longer_labels_as_the_base_label() {
        let dir = tempfile::tempdir().unwrap();
        let repo = pin(crate::forge::ForgeKind::GitLab, "git@gitlab.com:o/r.git");
        let only_long_names = "aida:merge-hold-recorded\naida:merge-hold-cleared\n";
        let LabelDefinitions::Read { present, missing } =
            label_definitions_with(dir.path(), &repo, |_, _, _| {
                Ok((true, only_long_names.into()))
            })
        else {
            panic!("expected read")
        };
        assert_eq!(missing, vec![HOLD_LABEL]);
        assert_eq!(present, vec![HOLD_RECORDED_LABEL, HOLD_CLEARED_LABEL]);
    }

    // trace:BUG-1693 | ai:codex
    #[test]
    fn a_second_hold_drops_the_stale_clearance_so_manual_label_removal_cannot_release() {
        use crate::forge::ForgeKind;
        use std::collections::BTreeSet;

        fn apply_label_args(labels: &mut BTreeSet<String>, argv: &[String]) {
            let mut index = 0;
            while index + 1 < argv.len() {
                let add = match argv[index].as_str() {
                    "--add-label" | "--label" => true,
                    "--remove-label" | "--unlabel" => false,
                    _ => {
                        index += 1;
                        continue;
                    }
                };
                for label in argv[index + 1].split(',') {
                    if add {
                        labels.insert(label.to_string());
                    } else {
                        labels.remove(label);
                    }
                }
                index += 2;
            }
        }

        fn run_sequence(kind: ForgeKind) {
            let repo_origin = match kind {
                ForgeKind::GitHub => "git@github.com:o/r.git",
                ForgeKind::GitLab => "https://gitlab.com/g/sub/p.git",
                ForgeKind::None => unreachable!(),
            };
            let repo = pin(kind, repo_origin);
            let mut labels = BTreeSet::new();
            for held in [true, false, true] {
                let (_, args) = sync_label_command(&repo, 42, held).unwrap();
                apply_label_args(&mut labels, &args);
            }
            labels.remove(HOLD_LABEL);

            // tests/test_merge_hold_gate.sh case
            // 'aida:merge-hold-recorded|1|dropping the active label by hand leaves history without clearance'
            // proves the shipped gate fails on this set.
            assert_eq!(labels, BTreeSet::from([HOLD_RECORDED_LABEL.to_string()]));
        }

        run_sequence(ForgeKind::GitHub);
        run_sequence(ForgeKind::GitLab);
    }

    // TASK-1455: the pin comes from origin; enterprise / self-managed hosts
    // are host-qualified so a number never resolves against the public forge.
    #[test]
    fn pinned_repo_arg_is_host_qualified_off_the_public_forge() {
        use crate::forge::ForgeKind;
        assert_eq!(
            pin(ForgeKind::GitHub, "https://github.com/Joe/aida.git").repo_arg(),
            "Joe/aida"
        );
        assert_eq!(
            pin(ForgeKind::GitHub, "git@ghe.corp.example:team/svc.git").repo_arg(),
            "ghe.corp.example/team/svc"
        );
        // An ssh alias is resolved by the CLI itself; pin the path.
        assert_eq!(
            pin(ForgeKind::GitHub, "git@github-work:team/svc.git").repo_arg(),
            "team/svc"
        );
        assert_eq!(
            pin(
                ForgeKind::GitLab,
                "ssh://git@gitlab.corp.example:2222/g/p.git"
            )
            .repo_arg(),
            "https://gitlab.corp.example/g/p"
        );
    }

    // TASK-1455 / PRIN-5: no resolvable repo means REFUSE, never call unpinned.
    #[test]
    fn unresolvable_origin_refuses_instead_of_calling_unpinned() {
        use crate::forge::ForgeKind;
        for origin in [
            None,
            Some(""),
            Some("/srv/git/repo"),
            Some("https://github.com/"),
        ] {
            let err = pin_from_origin(ForgeKind::GitHub, origin).unwrap_err();
            assert!(
                err.contains("refusing to call `gh` unpinned"),
                "{origin:?}: {err}"
            );
        }
        assert!(pin_from_origin(ForgeKind::GitHub, Some("git@github.com:o/r.git")).is_ok());
    }

    // TASK-1455 acceptance 2: a forge answer naming a DIFFERENT repo (or a
    // different number) is refused, and the pinned argv names the repo.
    #[test]
    fn pinned_change_refuses_an_answer_from_a_mismatched_repo() {
        use crate::forge::ForgeKind;
        let dir = tempfile::tempdir().unwrap();
        let gh = pin(ForgeKind::GitHub, "https://github.com/o/r.git");
        let seen = std::cell::RefCell::new(Vec::new());
        let answer = |url: &'static str| {
            let seen = &seen;
            move |_: &Path, cli: &str, args: &[String]| {
                seen.borrow_mut().push((cli.to_string(), args.to_vec()));
                Ok((
                    true,
                    format!(
                        r#"{{"url":"{url}","state":"OPEN","headRefOid":"abc","labels":[{{"name":"aida:merge-hold"}}]}}"#
                    ),
                ))
            }
        };
        let ok =
            fetch_pinned_change_with(dir.path(), &gh, 7, answer("https://github.com/o/r/pull/7"))
                .unwrap();
        assert_eq!(
            ok,
            PinnedChange {
                merged: false,
                state: ChangeState::Open,
                head_sha: Some("abc".into()),
                hold_label: true
            }
        );
        let (cli, args) = seen.borrow()[0].clone();
        assert_eq!(cli, "gh");
        assert!(
            args.windows(2).any(|w| w[0] == "-R" && w[1] == "o/r"),
            "{args:?}"
        );

        for wrong in [
            "https://github.com/other/r/pull/7",
            "https://github.com/o/r/pull/8",
            "https://ghe.example.com/o/r/pull/7",
        ] {
            let err = fetch_pinned_change_with(dir.path(), &gh, 7, answer(wrong)).unwrap_err();
            assert!(err.contains("refusing forge answer"), "{wrong}: {err}");
        }
        let err =
            fetch_pinned_change_with(dir.path(), &gh, 7, |_: &Path, _: &str, _: &[String]| {
                Ok((true, r#"{"state":"OPEN","labels":[]}"#.to_string()))
            })
            .unwrap_err();
        assert!(err.contains("<no url>"), "{err}");

        let gl = pin(ForgeKind::GitLab, "https://gitlab.com/g/p.git");
        let merged = fetch_pinned_change_with(dir.path(), &gl, 3, |_: &Path, cli: &str, args: &[String]| {
            assert_eq!(cli, "glab");
            assert!(args.contains(&"projects/g%2Fp/merge_requests/3".to_string()), "{args:?}");
            Ok((
                true,
                r#"{"web_url":"https://gitlab.com/g/p/-/merge_requests/3","state":"merged","sha":"d1","labels":["bug"]}"#
                    .to_string(),
            ))
        })
        .unwrap();
        assert!(merged.merged && !merged.hold_label);
    }

    // TASK-1455: a label sync on a repo that cannot be pinned refuses AND
    // records the marker unsynced — it never shells out unpinned.
    #[test]
    fn sync_label_refuses_and_records_when_the_repo_cannot_be_pinned() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["init", "-q"])
            .output()
            .unwrap();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "[forge]\nprovider = \"github\"\n",
        )
        .unwrap();
        write_hold(root, 5, "drive").unwrap();
        let err = sync_label_with(root, 5, true, |_, _, _| {
            panic!("an unpinned forge call must never be made")
        })
        .unwrap_err();
        assert!(format!("{err}").contains("unpinned"), "{err}");
        assert!(matches!(read_label_state(root, 5), LabelState::Unsynced(_)));
    }

    // TASK-189: the forge label is its own state; an unreadable forge is
    // divergence-UNKNOWN, never agreement and never the marker's claim.
    #[test]
    fn forge_label_state_and_divergence() {
        let change = |hold_label| {
            Ok(Some(PinnedChange {
                merged: false,
                state: ChangeState::Open,
                head_sha: None,
                hold_label,
            }))
        };
        assert_eq!(ForgeLabel::from_fetch(&change(true)), ForgeLabel::Present);
        assert_eq!(ForgeLabel::from_fetch(&change(false)), ForgeLabel::Absent);
        assert_eq!(ForgeLabel::from_fetch(&Ok(None)), ForgeLabel::NoForge);
        let unknown = ForgeLabel::from_fetch(&Err("gh: offline".into()));
        assert_eq!(unknown.as_str(), "unknown");
        assert_eq!(label_diverged(&ForgeLabel::Present), Some(false));
        assert_eq!(label_diverged(&ForgeLabel::Absent), Some(true));
        assert_eq!(label_diverged(&ForgeLabel::NoForge), Some(false));
        assert_eq!(label_diverged(&unknown), None);
    }

    // BUG-1469: the label scan is ONE pinned query; every entry must belong
    // to the pinned repo and carry the label, and a full page is reported as
    // possibly truncated rather than trusted.
    // trace:BUG-1469 | ai:claude
    #[test]
    fn labeled_change_scan_is_pinned_and_refuses_unattributable_answers() {
        use crate::forge::ForgeKind;
        let dir = tempfile::tempdir().unwrap();
        let gh = pin(ForgeKind::GitHub, "https://github.com/o/r.git");
        let (cli, args) = labeled_changes_command(&gh).unwrap();
        assert_eq!(cli, "gh");
        assert!(
            args.windows(2).any(|w| w[0] == "-R" && w[1] == "o/r"),
            "{args:?}"
        );
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--label" && w[1] == HOLD_LABEL));
        assert!(args.windows(2).any(|w| w[0] == "--state" && w[1] == "open"));

        let row = |n: u64, repo: &str, label: &str| {
            format!(
                r#"{{"number":{n},"url":"https://github.com/{repo}/pull/{n}","labels":[{{"name":"{label}"}}]}}"#
            )
        };
        let answer = |body: String| move |_: &Path, _: &str, _: &[String]| Ok((true, body.clone()));
        let ok = list_labeled_open_changes_with(
            dir.path(),
            &gh,
            answer(format!(
                "[{},{}]",
                row(1978, "o/r", HOLD_LABEL),
                row(1974, "o/r", HOLD_LABEL)
            )),
        )
        .unwrap();
        assert_eq!(ok, vec![1974, 1978]);

        let foreign = format!("[{}]", row(7, "other/r", HOLD_LABEL));
        let err = list_labeled_open_changes_with(dir.path(), &gh, answer(foreign)).unwrap_err();
        assert!(err.contains("pinned repo"), "{err}");
        let unlabeled = format!("[{}]", row(7, "o/r", "bug"));
        let err = list_labeled_open_changes_with(dir.path(), &gh, answer(unlabeled)).unwrap_err();
        assert!(err.contains("does not carry"), "{err}");
        let full = format!(
            "[{}]",
            (1..=LABELED_QUERY_LIMIT as u64)
                .map(|n| row(n, "o/r", HOLD_LABEL))
                .collect::<Vec<_>>()
                .join(",")
        );
        let err = list_labeled_open_changes_with(dir.path(), &gh, answer(full)).unwrap_err();
        assert!(err.contains("truncated"), "{err}");
        let err =
            list_labeled_open_changes_with(dir.path(), &gh, |_: &Path, _: &str, _: &[String]| {
                Ok((false, String::new()))
            })
            .unwrap_err();
        assert!(err.contains("failed"), "{err}");

        let gl = pin(ForgeKind::GitLab, "https://gitlab.com/g/p.git");
        let (cli, args) = labeled_changes_command(&gl).unwrap();
        assert_eq!(cli, "glab");
        assert!(
            args.iter().any(|a| a.starts_with(
                "projects/g%2Fp/merge_requests?state=opened&labels=aida%3Amerge-hold"
            )),
            "{args:?}"
        );
        let got = list_labeled_open_changes_with(dir.path(), &gl, |_: &Path, _: &str, _: &[String]| {
            Ok((
                true,
                r#"[{"iid":3,"web_url":"https://gitlab.com/g/p/-/merge_requests/3","labels":["aida:merge-hold"]}]"#
                    .to_string(),
            ))
        })
        .unwrap();
        assert_eq!(got, vec![3]);
    }

    // BUG-1469 verification shape: N labeled PRs (some also marker-backed)
    // plus M marker-only holds list as N+M rows — the label-only ones are
    // exactly the labeled PRs with no marker.
    // trace:BUG-1469 | ai:claude
    #[test]
    fn label_only_holds_are_the_labeled_prs_without_a_marker() {
        let labeled = [1972, 1974, 1978, 1979, 2014];
        let markers = [1972, 1979, 2050];
        let label_only = label_only_holds(&labeled, &markers);
        assert_eq!(label_only, vec![1974, 1978, 2014]);
        let rows = markers.len() + label_only.len();
        let marker_only = markers.iter().filter(|m| !labeled.contains(m)).count();
        assert_eq!(
            rows,
            labeled.len() + marker_only,
            "N labeled + M marker-only"
        );
        assert_eq!(HoldSource::LabelOnly.as_str(), "label");
    }

    // BUG-1499: a label-only hold is cleared by the same human floor and the
    // clearance is RECORDED like a marker's, naming the missing half. The
    // handler reads the forge label before recording (never guesses).
    // trace:BUG-1499 | ai:claude
    #[test]
    fn label_only_clear_is_recorded_as_the_human_principal() {
        let dir = tempfile::tempdir().unwrap();
        let record = typed_hold(2019, HoldReasonKind::Unknown, LABEL_ONLY_REASON, None);
        let actor = human_clear_actor(&record, "joe", None).unwrap();
        record_clearance(dir.path(), &record, &actor).unwrap();
        let clearance: HoldClearance = serde_json::from_str(
            &std::fs::read_to_string(clearance_path(dir.path(), 2019)).unwrap(),
        )
        .unwrap();
        assert_eq!(clearance.cleared_by, "human:joe");
        assert_eq!(clearance.detail, LABEL_ONLY_REASON);

        let lib_source = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}",
            include_str!("lib.rs"),
            include_str!("lib_part1.rs"),
            include_str!("lib_part2.rs"),
            include_str!("lib_part3.rs"),
            include_str!("lib_part4.rs"),
            include_str!("lib_part5.rs"),
            include_str!("lib_part6.rs")
        );
        let clear = lib_source
            .split("crate::cli::MergeHoldAction::Clear { pr, stale } =>")
            .nth(1)
            .and_then(|body| body.split("(None, true) =>").next())
            .expect("clear handler must remain inspectable");
        let floor = clear.find("has_integrity_floor_authority()").unwrap();
        let read = clear.find("merge_hold::fetch_pinned_change(").unwrap();
        let recorded = clear
            .rfind("merge_hold::record_clearance(&root, record, actor)")
            .unwrap();
        assert!(
            floor < read && read < recorded,
            "floor, then forge read, then record"
        );
        assert!(clear.contains("merge_hold::LABEL_ONLY_REASON"));
        // BUG-1693: a marker deleted by hand leaves AIDA fail-closed on that
        // PR forever unless `merge-hold clear` can RECORD the release. Pin the
        // door itself, not merely a mention of it: whitespace-normalised so
        // `cargo fmt` cannot break the assertion, and ordered before the
        // "nothing to clear" early return a missing marker would otherwise
        // take. trace:BUG-1693 | ai:claude
        let squashed = clear.split_whitespace().collect::<Vec<_>>().join(" ");
        let door = squashed
            .find(
                "if let Some((record, actor)) = &tampering_record { \
                 merge_hold::record_clearance(&root, record, actor)?; }",
            )
            .expect("clear must RECORD a clearance for a hold whose marker went missing");
        let no_hold = squashed
            .find("No merge-hold on PR #{pr}")
            .expect("the no-hold early return must remain inspectable");
        assert!(
            door < no_hold,
            "the tampering clearance must be recorded before the no-hold early return"
        );
    }

    // BUG-1693 AC1: `merge-hold list` must SURFACE a tampered PR, and the
    // "nothing to see here" early return must not swallow it. The render is a
    // thin loop over `unrecorded_marker_removals` (tested above), so the one
    // place a regression could still mislead an operator is the empty-state
    // conjunct: drop it and the command prints TAMPERING and then claims "No
    // active merge-holds." in the same breath. Pin both, whitespace-normalised
    // so `cargo fmt` cannot break the assertion.
    // trace:BUG-1693 | ai:claude
    #[test]
    fn list_surfaces_a_tampered_pr_and_its_empty_state_cannot_swallow_it() {
        let lib_source = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}",
            include_str!("lib.rs"),
            include_str!("lib_part1.rs"),
            include_str!("lib_part2.rs"),
            include_str!("lib_part3.rs"),
            include_str!("lib_part4.rs"),
            include_str!("lib_part5.rs"),
            include_str!("lib_part6.rs")
        );
        let list = lib_source
            .split("crate::cli::MergeHoldAction::List { json, fix } =>")
            .nth(1)
            .and_then(|body| body.split("crate::cli::MergeHoldAction::Add {").next())
            .expect("list handler must remain inspectable");
        let squashed = list.split_whitespace().collect::<Vec<_>>().join(" ");

        let render = squashed
            .find("for pr in &unrecorded_removals {")
            .expect("list must RENDER the unrecorded marker removals it computed");
        assert!(
            squashed[render..].contains("TAMPERING"),
            "the tampering render must name the condition in the operator's words"
        );

        let empty = squashed
            .find(
                "if live.is_empty() && stale.is_empty() && label_only.is_empty() \
                   && unrecorded_removals.is_empty() { println!(\"No active merge-holds.\");",
            )
            .expect(
                "the empty-state early return must count unrecorded marker removals, \
                 or list prints TAMPERING and then claims there are no holds",
            );
        assert!(
            render < empty,
            "the tampering line must be rendered before the empty-state early return"
        );
    }

    // BUG-1541: state words parse per forge; anything unrecognised is Unknown
    // and never terminal (fail closed).
    // trace:BUG-1541 | ai:claude
    #[test]
    fn change_state_parses_both_forges_and_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let gh = pin(
            crate::forge::ForgeKind::GitHub,
            "https://github.com/o/r.git",
        );
        let closed =
            fetch_pinned_change_with(dir.path(), &gh, 7, |_: &Path, _: &str, _: &[String]| {
                Ok((
                    true,
                    r#"{"url":"https://github.com/o/r/pull/7","state":"CLOSED","labels":[]}"#
                        .to_string(),
                ))
            })
            .unwrap();
        assert_eq!(closed.state, ChangeState::ClosedUnmerged);
        assert!(!closed.merged);
        assert_eq!(closed.state.terminal(), Some(TerminalState::ClosedUnmerged));
        let missing =
            fetch_pinned_change_with(dir.path(), &gh, 7, |_: &Path, _: &str, _: &[String]| {
                Ok((
                    true,
                    r#"{"url":"https://github.com/o/r/pull/7","labels":[]}"#.to_string(),
                ))
            })
            .unwrap();
        assert_eq!(missing.state, ChangeState::Unknown);
        assert_eq!(missing.state.terminal(), None);
        assert_eq!(ChangeState::parse(Some("locked")).terminal(), None);
    }

    // BUG-1562, red-first: a marker citing a sha is REPORTED once the PR head
    // moves past it, and not while the head still matches. A rework hold whose
    // verdict is now approved (or closed by a merge) is reported too. Nothing
    // is cleared — the marker is untouched.
    // trace:BUG-1562 | ai:claude
    #[test]
    fn premise_stale_fires_when_the_world_moves_and_never_clears() {
        use crate::review_verdict::{RecordedVerdict, VerdictKind};
        let dir = tempfile::tempdir().unwrap();
        write_hold(
            dir.path(),
            2049,
            "CHANGES REQUESTED for STORY-1422 at 3acf3671fd7a",
        )
        .unwrap();
        let before = std::fs::read(hold_path(dir.path(), 2049)).unwrap();
        let legacy = read_hold_record(dir.path(), 2049).unwrap();
        let none = |_: &str| None;
        let ns = |_: &str| -> Option<String> { None };
        // Current: head still at the cited sha → not stale.
        assert_eq!(
            premise_stale(&legacy, Some("3acf3671fd7a0000"), none, ns),
            None
        );
        // Head unknown → unknown is never stale.
        assert_eq!(premise_stale(&legacy, None, none, ns), None);
        // Advance the head past it → reported.
        let why = premise_stale(&legacy, Some("cd21a1dc0a9e1111"), none, ns).expect("stale");
        assert!(
            why.contains("3acf3671fd7a") && why.contains("cd21a1dc0a9e"),
            "{why}"
        );
        assert_eq!(std::fs::read(hold_path(dir.path(), 2049)).unwrap(), before);

        // A PR number or plain word is never mistaken for a cited sha.
        let plain = typed_hold(
            5,
            HoldReasonKind::Supervision,
            "PR 20490123 is marked drive",
            None,
        );
        assert_eq!(premise_stale(&plain, Some("abcdef1234567"), none, ns), None);

        // Typed target_head_sha wins over prose.
        let rework = typed_hold(
            2001,
            HoldReasonKind::Rework,
            "stranded refusal recovered for BUG-1291 at 64e4e5755b",
            Some("64e4e5755b".into()),
        );
        assert_eq!(premise_stale(&rework, Some("64e4e5755b99"), none, ns), None);
        let approved = |spec: &str| {
            (spec == "BUG-1291").then(|| RecordedVerdict {
                kind: VerdictKind::Approved,
                raw: "approved".into(),
                reviewed_sha: Some("af49b12b83fc".into()),
                ..Default::default()
            })
        };
        let why = premise_stale(&rework, Some("af49b12b83fc"), approved, ns).expect("stale");
        assert!(
            why.contains("BUG-1291's verdict is now APPROVED at af49b12b83fc"),
            "{why}"
        );
        assert!(why.contains("PR head is now af49b12b83fc"), "{why}");
        let closed = |_: &str| {
            Some(RecordedVerdict {
                kind: VerdictKind::RequestChanges,
                closed_by_merge: Some("deadbeef1234".into()),
                ..Default::default()
            })
        };
        let why = premise_stale(&rework, Some("64e4e5755b"), closed, ns).expect("closed");
        assert!(why.contains("closed by merge deadbeef1234"), "{why}");
        // A supervision hold's premise is not the verdict — an approval does
        // not make "awaiting a human merge" stale.
        let supervised = typed_hold(
            9,
            HoldReasonKind::Supervision,
            "BUG-1291 is marked drive",
            None,
        );
        assert_eq!(
            premise_stale(&supervised, Some("af49b12b83fc"), approved, ns),
            None
        );
    }

    // BUG-1532 criteria 4 + 3 + 11, red-first against REAL verdict files: a
    // refusal hold is released only by an APPROVED verdict recorded at the
    // current head. A verdict at an older head, a refusal at the head, a
    // newer refusal, a verdict with no sha, or an unknown head all keep it —
    // and the held message names the verdict + sha (or the missing sha).
    // trace:BUG-1532 | ai:claude
    #[test]
    fn refusal_hold_releases_only_on_a_fresh_verdict_at_the_current_head() {
        use crate::review_verdict::{read_recorded_verdict, read_verdict_for_sha, record_verdict};
        const OLD: &str = "3acf3671fd7a0000000000000000000000000000";
        const HEAD: &str = "cd21a1dc0a9e1111111111111111111111111111";
        const OTHER: &str = "af49b12b83fc2222222222222222222222222222";
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut hold = typed_hold(
            2049,
            HoldReasonKind::Rework,
            "CHANGES REQUESTED for STORY-1422 at 3acf3671fd7a",
            Some(OLD.into()),
        );
        hold.verdict_ref = Some(VerdictRef::new(
            "story-1422",
            Some(2049),
            Some(OLD.into()),
            Some("claude-reviewer-1".into()),
        ));
        write_typed_hold(root, &hold).unwrap();
        let before = std::fs::read(hold_path(root, 2049)).unwrap();
        let release = |head: Option<&str>| {
            refusal_release(
                &hold,
                head,
                |k| read_recorded_verdict(root, k),
                |k, sha| read_verdict_for_sha(root, k, sha),
            )
        };

        // No verdict at all → held, naming where it looked.
        match release(Some(HEAD)) {
            RefusalRelease::Held(why) => {
                assert!(why.contains("STORY-1422 or PR-2049"), "{why}")
            }
            other => panic!("no verdict must hold: {other:?}"),
        }
        // The refusal at the OLD head → held, naming the verdict and its sha.
        record_verdict(
            root,
            "STORY-1422",
            Some("request-changes"),
            Some(OLD),
            None,
            Some("tests assert source order"),
            &[],
            "claude-reviewer-1",
        )
        .unwrap();
        match release(Some(HEAD)) {
            RefusalRelease::Held(why) => {
                assert!(
                    why.contains("CHANGES REQUESTED for STORY-1422 at 3acf3671fd7a"),
                    "{why}"
                );
                assert!(why.contains("claude-reviewer-1"), "{why}");
                assert!(
                    why.contains(".aida/review-verdicts/STORY-1422.json"),
                    "{why}"
                );
                assert!(why.contains("cd21a1dc0a9e"), "{why}");
            }
            other => panic!("a stale refusal must hold: {other:?}"),
        }
        // Unknown head → held (fail closed).
        assert!(matches!(release(None), RefusalRelease::Held(_)));
        // An approval at a DIFFERENT sha does not cover the head.
        record_verdict(
            root,
            "STORY-1422",
            Some("approved"),
            Some(OTHER),
            None,
            None,
            &[],
            "claude-reviewer-1",
        )
        .unwrap();
        assert!(matches!(release(Some(HEAD)), RefusalRelease::Held(_)));
        // A fresh APPROVED verdict at exactly the current head releases it,
        // referencing the verdict record it rests on.
        record_verdict(
            root,
            "STORY-1422",
            Some("approved"),
            Some(HEAD),
            None,
            None,
            &[],
            "claude-reviewer-1",
        )
        .unwrap();
        match release(Some(HEAD)) {
            RefusalRelease::Released(r) => {
                assert_eq!(r.key, "STORY-1422");
                assert_eq!(r.reviewed_sha.as_deref(), Some(HEAD));
                assert_eq!(r.recorded_by.as_deref(), Some("claude-reviewer-1"));
            }
            other => panic!("a fresh approval at head must release: {other:?}"),
        }
        // A refusal re-recorded AT the head after that approval → held again.
        record_verdict(
            root,
            "STORY-1422",
            Some("request-changes"),
            Some(HEAD),
            None,
            None,
            &[],
            "claude-reviewer-1",
        )
        .unwrap();
        assert!(matches!(release(Some(HEAD)), RefusalRelease::Held(_)));
        // Deciding the condition never touches the marker.
        assert_eq!(std::fs::read(hold_path(root, 2049)).unwrap(), before);
    }

    // BUG-1532 criterion 11: a refusal whose verdict has NO sha is released
    // only by a fresh verdict that records one; the message names the missing
    // provenance. Untyped legacy markers are refusals (criterion 5) and are
    // answered under the PR-keyed record — never a key parsed from prose.
    // trace:BUG-1532 | ai:claude
    #[test]
    fn refusal_with_an_unstamped_verdict_names_the_missing_provenance() {
        use crate::review_verdict::{read_recorded_verdict, read_verdict_for_sha, verdict_path};
        const HEAD: &str = "35de6d3abb00000000000000000000000000000a";
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_hold(root, 2040, "REVIEWER HOLD: BUG-1 needs work").unwrap();
        let legacy = read_hold_record(root, 2040).unwrap();
        assert!(legacy.is_refusal());
        assert_eq!(refusal_verdict_keys(&legacy), vec!["PR-2040".to_string()]);
        let path = verdict_path(root, "PR-2040");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"verdict":"request-changes"}"#).unwrap();
        // Even an unstamped approval is not a verdict AT the head.
        let bug1 = verdict_path(root, "BUG-1");
        std::fs::write(
            &bug1,
            format!(r#"{{"verdict":"approved","reviewed_sha":"{HEAD}"}}"#),
        )
        .unwrap();
        match refusal_release(
            &legacy,
            Some(HEAD),
            |k| read_recorded_verdict(root, k),
            |k, sha| read_verdict_for_sha(root, k, sha),
        ) {
            RefusalRelease::Held(why) => {
                assert!(why.contains("NO reviewed sha"), "{why}");
                assert!(why.contains("missing"), "{why}");
            }
            other => panic!("unstamped refusal must hold: {other:?}"),
        }
        // Supervision is not a refusal; recusal and malformed are not either
        // (they have their own release paths).
        assert!(!typed_hold(1, HoldReasonKind::Supervision, "x", None).is_refusal());
        assert!(typed_hold(1, HoldReasonKind::Rework, "x", None).is_refusal());
    }

    // BUG-1532 criterion 10 + BUG-1562: a typed marker round-trips its
    // verdict reference, release condition and spec; an older marker without
    // them still reads (every new field is optional).
    // trace:BUG-1532 trace:BUG-1562 | ai:claude
    #[test]
    fn marker_references_its_verdict_and_carries_a_release_condition() {
        let dir = tempfile::tempdir().unwrap();
        let mut record = typed_hold(7, HoldReasonKind::Rework, "summary", Some("abc1234".into()));
        record.verdict_ref = Some(VerdictRef::new(
            "bug-1460",
            Some(7),
            Some("abc1234".into()),
            Some("claude-reviewer-1".into()),
        ));
        record.release_condition = Some("1. approved at head 2. squash names BUG-1460".into());
        record.spec = Some("BUG-1460".into());
        write_typed_hold(dir.path(), &record).unwrap();
        let body = std::fs::read_to_string(hold_path(dir.path(), 7)).unwrap();
        assert!(body.contains("\"verdict_ref\""), "{body}");
        let back = read_hold_record(dir.path(), 7).unwrap();
        assert_eq!(back.verdict_ref.as_ref().unwrap().key, "BUG-1460");
        assert_eq!(
            back.verdict_ref.as_ref().unwrap().path(),
            ".aida/review-verdicts/BUG-1460.json"
        );
        assert_eq!(back.spec.as_deref(), Some("BUG-1460"));
        assert_eq!(
            back.release_condition_or_default(),
            (
                "1. approved at head 2. squash names BUG-1460".to_string(),
                false
            )
        );
        // A pre-BUG-1532 typed marker (no new fields) still parses.
        let old = r#"{"schema_version":2,"pr":8,"reason_kind":"supervision","detail":"d","routing_state":"pending"}"#;
        std::fs::write(hold_path(dir.path(), 8), old).unwrap();
        let back = read_hold_record(dir.path(), 8).unwrap();
        assert_eq!(back.reason_kind, HoldReasonKind::Supervision);
        assert!(back.verdict_ref.is_none() && back.release_condition.is_none());
        let (cond, is_default) = back.release_condition_or_default();
        assert!(is_default && cond.contains("aida pr ship 8"), "{cond}");
    }

    // BUG-1562: "spec no longer running" — a hold whose spec is now terminal
    // in the local store is flagged; a live one is not. Flag only.
    // trace:BUG-1562 | ai:claude
    #[test]
    fn premise_flags_a_hold_whose_spec_is_no_longer_running() {
        let none = |_: &str| None;
        let mut hold = typed_hold(9, HoldReasonKind::Supervision, "is marked drive", None);
        hold.spec = Some("STORY-1".into());
        let running = |_: &str| Some("In Progress".to_string());
        assert_eq!(premise_stale(&hold, None, none, running), None);
        let done_on_branch = |_: &str| Some("Done".to_string());
        assert_eq!(premise_stale(&hold, None, none, done_on_branch), None);
        let completed = |s: &str| (s == "STORY-1").then(|| "Completed".to_string());
        let why = premise_stale(&hold, None, none, completed).expect("stale");
        assert!(
            why.contains("STORY-1 is no longer running (status completed)"),
            "{why}"
        );
        // Untyped: the spec named in the prose is checked (flag only).
        let prose = typed_hold(
            9,
            HoldReasonKind::Supervision,
            "STORY-1 is marked drive",
            None,
        );
        assert!(premise_stale(&prose, None, none, completed).is_some());
        let unknown = |_: &str| None;
        assert_eq!(premise_stale(&prose, None, none, unknown), None);
    }

    // BUG-1562, red-first: a marker carrying a distinctive release condition
    // SURVIVES `merge-hold add` on the same PR; only `--replace` replaces it.
    // trace:BUG-1562 | ai:claude
    #[test]
    fn hand_add_never_silently_overwrites_an_existing_marker() {
        let dir = tempfile::tempdir().unwrap();
        let line = "RELEASE: squash subject must not name BUG-1288";
        write_hold(dir.path(), 1999, line).unwrap();
        let fresh = typed_hold(1999, HoldReasonKind::Supervision, "one line", None);
        let err = place_hand_hold(dir.path(), &fresh, false).unwrap_err();
        assert!(err.contains(line) && err.contains("--replace"), "{err}");
        let body = std::fs::read_to_string(hold_path(dir.path(), 1999)).unwrap();
        assert!(
            body.contains(line),
            "the existing body must survive: {body}"
        );
        place_hand_hold(dir.path(), &fresh, true).unwrap();
        assert_eq!(
            read_hold_record(dir.path(), 1999).unwrap().detail,
            "one line"
        );
        // No marker → placed.
        place_hand_hold(
            dir.path(),
            &typed_hold(2000, HoldReasonKind::Decision, "d", None),
            false,
        )
        .unwrap();
        assert!(read_hold_record(dir.path(), 2000).is_some());
    }

    #[test]
    fn parses_gitlab_label_shapes() {
        assert!(
            labels_json_contains(r#"{"labels":["bug","aida:merge-hold"]}"#, HOLD_LABEL).unwrap()
        );
        assert!(
            labels_json_contains(r#"{"labels":[{"name":"aida:merge-hold"}]}"#, HOLD_LABEL).unwrap()
        );
        assert!(!labels_json_contains(r#"{"labels":["bug"]}"#, HOLD_LABEL).unwrap());
    }

    #[test]
    fn write_read_clear_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // No hold initially.
        assert!(read_hold(root, 42).is_none());
        // Write a hold → read returns the reason.
        write_hold(
            root,
            42,
            "STORY-1155 is marked drive — merge requires review",
        )
        .unwrap();
        let reason = read_hold(root, 42).expect("hold must be present");
        assert!(reason.contains("drive"), "{reason}");
        // Clear → gone.
        clear_hold(root, 42).unwrap();
        assert!(read_hold(root, 42).is_none());
        // Clearing an absent hold is a no-op success.
        clear_hold(root, 42).unwrap();
    }

    #[test]
    fn empty_reason_still_holds_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_hold(root, 7, "   ").unwrap();
        // An empty/whitespace reason must STILL register as a hold — the marker's
        // presence is the signal, never its content. A blank marker that merged
        // would be the fail-open bug this fix exists to prevent.
        assert!(
            read_hold(root, 7).is_some(),
            "a marker with a blank reason must still hold"
        );
    }

    #[test]
    fn present_but_unreadable_marker_holds_fail_closed() {
        // TASK-1238: a marker that EXISTS but can't be read as a file must HOLD,
        // not merge. Simulate it by putting a directory where the marker file
        // would be — `read_to_string` then errors with something other than
        // NotFound. The old `.ok()?` returned None here (fail-open → merge); the
        // fix must return Some.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(hold_path(root, 99)).unwrap();
        assert!(
            read_hold(root, 99).is_some(),
            "a present-but-unreadable marker must HOLD (fail closed), never merge"
        );
    }

    #[test]
    fn list_holds_enumerates_ascending() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_hold(root, 30, "b").unwrap();
        write_hold(root, 5, "a").unwrap();
        let holds = list_holds(root);
        assert_eq!(
            holds.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
            vec![5, 30]
        );
    }

    // trace:BUG-1236 | ai:claude
    #[test]
    fn sync_failure_is_returned_and_recorded_on_the_marker() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // A git repo with a GitHub origin so the forge resolves to GitHub.
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap()
        };
        git(&["init", "-q"]);
        git(&["remote", "add", "origin", "https://github.com/o/r.git"]);
        write_hold(root, 7, "drive").unwrap();
        let calls = std::cell::Cell::new(0);
        let err = sync_label_with(root, 7, true, |_, _, _| {
            calls.set(calls.get() + 1);
            Ok((false, "gh: HTTP 502 bad gateway\n".to_string()))
        })
        .unwrap_err();
        assert_eq!(calls.get(), 2, "one retry");
        assert!(format!("{err}").contains("502"), "{err}");
        assert_eq!(
            read_label_state(root, 7),
            LabelState::Unsynced("gh: HTTP 502 bad gateway".to_string())
        );
        assert_eq!(
            read_hold(root, 7).as_deref(),
            Some("drive"),
            "reason line untouched"
        );
        // A later successful sync flips the state.
        sync_label_with(root, 7, true, |_, _, _| Ok((true, String::new()))).unwrap();
        assert_eq!(read_label_state(root, 7), LabelState::Synced);
    }

    // trace:BUG-1236 | ai:claude
    #[test]
    fn markers_without_a_state_line_read_as_unknown() {
        let dir = tempfile::tempdir().unwrap();
        write_hold(dir.path(), 9, "guided").unwrap();
        assert_eq!(read_label_state(dir.path(), 9), LabelState::Unknown);
        assert_eq!(read_label_state(dir.path(), 10), LabelState::Unknown);
    }

    // trace:STORY-1397 | ai:codex
    #[test]
    fn typed_recusal_round_trip_and_label_update_preserve_routing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida/agents")).unwrap();
        std::fs::write(dir.path().join(".aida/agents/reader.toml"), format!("id = \"cold-reader\"\nagent_type = \"codex\"\npid = {}\nrole = \"reviewer\"\navailability = \"available\"\n", std::process::id())).unwrap();
        let record = MergeHoldRecord {
            schema_version: 2,
            pr: 2023,
            reason_kind: HoldReasonKind::Recusal,
            detail: "author cannot merge".into(),
            recused_principals: vec!["agent:author".into()],
            routed_to: vec!["agent:cold-reader".into()],
            routing_state: HoldRoutingState::Routed,
            target_head_sha: Some("abcdef0123456789".into()),
            label_state: None,
            legacy: false,
            verdict_ref: None,
            release_condition: None,
            spec: None,
            placed_by: None,
            absorbed: Vec::new(),
        };
        write_typed_hold(dir.path(), &record).unwrap();
        record_label_state(dir.path(), 2023, &LabelState::Synced).unwrap();
        let reread = read_hold_record(dir.path(), 2023).unwrap();
        assert_eq!(reread.reason_kind, HoldReasonKind::Recusal);
        assert_eq!(reread.recused_principals, vec!["agent:author"]);
        assert_eq!(reread.routed_to, vec!["agent:cold-reader"]);
        assert_eq!(reread.target_head_sha.as_deref(), Some("abcdef0123456789"));
        assert_eq!(read_label_state(dir.path(), 2023), LabelState::Synced);
        assert!(principal_is_recused(
            &reread,
            &PrincipalIdentity::parse("agent:author")
        ));
        assert!(!principal_is_recused(
            &reread,
            &PrincipalIdentity::parse("agent:cold-reader")
        ));
        assert!(dir
            .path()
            .join(".aida/agent-briefs/codex/PR-2023-abcdef0123456789.md")
            .exists());
    }

    #[test]
    fn recusal_requires_named_principal_and_malformed_json_stays_held() {
        let dir = tempfile::tempdir().unwrap();
        let record = MergeHoldRecord {
            schema_version: 2,
            pr: 7,
            reason_kind: HoldReasonKind::Recusal,
            detail: "recused".into(),
            recused_principals: Vec::new(),
            routed_to: Vec::new(),
            routing_state: HoldRoutingState::NoIndependentReader,
            target_head_sha: None,
            label_state: None,
            legacy: false,
            verdict_ref: None,
            release_condition: None,
            spec: None,
            placed_by: None,
            absorbed: Vec::new(),
        };
        assert!(write_typed_hold(dir.path(), &record).is_err());
        std::fs::create_dir_all(holds_dir(dir.path())).unwrap();
        std::fs::write(hold_path(dir.path(), 7), "{not-json").unwrap();
        let held = read_hold_record(dir.path(), 7).unwrap();
        assert_eq!(held.reason_kind, HoldReasonKind::Unknown);
        assert!(read_hold(dir.path(), 7).is_some());
    }

    #[test]
    fn legacy_marker_is_supervision_without_guessing_recusal_from_prose() {
        let dir = tempfile::tempdir().unwrap();
        write_hold(dir.path(), 8, "I wrote this; another reader is needed").unwrap();
        let held = read_hold_record(dir.path(), 8).unwrap();
        assert!(held.legacy);
        assert_eq!(held.reason_kind, HoldReasonKind::Supervision);
        assert!(held.recused_principals.is_empty());
    }

    // trace:BUG-1691 | ai:codex
    #[test]
    fn legacy_detail_is_preserved_when_absorbed_by_rework() {
        let dir = tempfile::tempdir().unwrap();
        let legacy_detail = "I wrote this; another reader is needed";
        write_hold(dir.path(), 8, legacy_detail).unwrap();
        let rework = typed_hold(8, HoldReasonKind::Rework, "changes requested", None);
        write_typed_hold(dir.path(), &rework).unwrap();
        let got = read_hold_record(dir.path(), 8).unwrap();
        assert_eq!(got.reason_kind, HoldReasonKind::Rework);
        assert_eq!(got.absorbed[0].reason_kind, HoldReasonKind::Supervision);
        assert_eq!(got.absorbed[0].detail, legacy_detail);
    }

    fn recused_record() -> MergeHoldRecord {
        let mut record = typed_hold(
            42,
            HoldReasonKind::Recusal,
            "author recused",
            Some("head-a".into()),
        );
        record.recused_principals = vec![PrincipalIdentity::parse("agent:author")];
        record
    }

    // trace:STORY-1397 | ai:claude
    #[test]
    fn human_at_terminal_clears_recusal_without_agent_id() {
        let record = recused_record();
        let actor = human_clear_actor(&record, "joe", None).expect("human clears");
        assert_eq!(actor.key(), "human:joe");
        // An unrelated declared agent id neither helps nor hinders the human.
        let other = PrincipalIdentity::parse("agent:someone-else");
        assert_eq!(
            human_clear_actor(&record, "joe", Some(&other))
                .unwrap()
                .key(),
            "human:joe"
        );
    }

    #[test]
    fn human_clear_refuses_a_recused_declared_agent_and_a_recused_human() {
        let record = recused_record();
        let author = PrincipalIdentity::parse("agent:author");
        assert!(human_clear_actor(&record, "joe", Some(&author)).is_err());

        let mut human_recused = recused_record();
        human_recused
            .recused_principals
            .push(PrincipalIdentity::parse("human:joe"));
        assert!(human_clear_actor(&human_recused, "joe", None).is_err());
        assert!(human_clear_actor(&human_recused, "ann", None).is_ok());

        let mut operator_recused = recused_record();
        operator_recused
            .recused_principals
            .push(PrincipalIdentity::parse("operator:joe"));
        assert!(human_clear_actor(&operator_recused, "joe", None).is_err());

        assert!(human_clear_actor(&record, "  ", None).is_err());
    }

    #[test]
    fn clearance_is_recorded_as_the_human_principal() {
        let dir = tempfile::tempdir().unwrap();
        let record = recused_record();
        let actor = human_clear_actor(&record, "joe", None).unwrap();
        record_clearance(dir.path(), &record, &actor).unwrap();
        let body = std::fs::read_to_string(clearance_path(dir.path(), 42)).unwrap();
        let clearance: HoldClearance = serde_json::from_str(&body).unwrap();
        assert_eq!(clearance.cleared_by, "human:joe");
        assert_eq!(clearance.reason_kind, HoldReasonKind::Recusal);
        assert_eq!(clearance.target_head_sha.as_deref(), Some("head-a"));
    }

    #[test]
    fn human_principal_round_trips() {
        let human = PrincipalIdentity::parse("human:joe");
        assert_eq!(human.principal_kind, PrincipalKind::Human);
        assert_eq!(human.key(), "human:joe");
        let json = serde_json::to_string(&human).unwrap();
        assert_eq!(
            serde_json::from_str::<PrincipalIdentity>(&json).unwrap(),
            human
        );
    }

    #[test]
    fn live_head_projection_marks_stale_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let mut record = recused_record();
        record.routing_state = HoldRoutingState::Routed;
        record.routed_to = vec![PrincipalIdentity::parse("agent:reader")];
        std::fs::create_dir_all(holds_dir(dir.path())).unwrap();
        let body = serde_json::to_vec_pretty(&record).unwrap();
        std::fs::write(hold_path(dir.path(), 42), &body).unwrap();

        let view = project_live_head(&record, Some("head-b"));
        assert_eq!(view.routing_state, HoldRoutingState::StaleHead);
        assert!(view.routed_to.is_empty());
        assert_eq!(view.target_head_sha.as_deref(), Some("head-b"));
        // Same head: unchanged. No head known: unchanged.
        assert_eq!(project_live_head(&record, Some("head-a")), record);
        assert_eq!(project_live_head(&record, None), record);
        // The durable marker is untouched.
        assert_eq!(std::fs::read(hold_path(dir.path(), 42)).unwrap(), body);
    }

    #[test]
    fn recusal_routing_is_idempotent_and_excludes_recused_agent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida/agents")).unwrap();
        for (file, id) in [("author.toml", "author"), ("reader.toml", "reader")] {
            std::fs::write(dir.path().join(".aida/agents").join(file), format!("id = \"{id}\"\nagent_type = \"codex\"\npid = {}\nrole = \"reviewer\"\navailability = \"available\"\n", std::process::id())).unwrap();
        }
        let mut record = typed_hold(43, HoldReasonKind::Recusal, "recused", Some("abc".into()));
        record.recused_principals = vec![PrincipalIdentity::parse("agent:author")];
        write_typed_hold(dir.path(), &record).unwrap();
        write_typed_hold(dir.path(), &record).unwrap();
        let reread = read_hold_record(dir.path(), 43).unwrap();
        assert_eq!(
            reread.routed_to,
            vec![PrincipalIdentity::parse("agent:reader")]
        );
        let briefs = std::fs::read_dir(dir.path().join(".aida/agent-briefs/codex"))
            .unwrap()
            .count();
        assert_eq!(briefs, 1);

        std::fs::remove_file(dir.path().join(".aida/agents/reader.toml")).unwrap();
        write_typed_hold(dir.path(), &reread).unwrap();
        let unrouted = read_hold_record(dir.path(), 43).unwrap();
        assert_eq!(
            unrouted.routing_state,
            HoldRoutingState::NoIndependentReader
        );
        assert!(dir
            .path()
            .join(".aida/agent-briefs/codex/PR-43-abc.md.stale")
            .exists());

        std::fs::write(dir.path().join(".aida/agents/new-reader.toml"), format!("id = \"new-reader\"\nagent_type = \"codex\"\npid = {}\nrole = \"reviewer\"\navailability = \"available\"\n", std::process::id())).unwrap();
        let mut moved = unrouted;
        moved.target_head_sha = Some("def".into());
        moved.routing_state = HoldRoutingState::StaleHead;
        write_typed_hold(dir.path(), &moved).unwrap();
        let rerouted = read_hold_record(dir.path(), 43).unwrap();
        assert_eq!(
            rerouted.routed_to,
            vec![PrincipalIdentity::parse("agent:new-reader")]
        );
        assert!(dir
            .path()
            .join(".aida/agent-briefs/codex/PR-43-def.md")
            .exists());
    }

    #[test]
    fn explicit_reconciliation_refreshes_routes_and_render_stays_read_only() {
        // Normalise CRLF (Windows autocrlf checkout) so the column-0
        // closing-brace split below still finds the body end.
        // trace:BUG-1556 | ai:claude
        let lib_source = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}",
            include_str!("lib.rs"),
            include_str!("lib_part1.rs"),
            include_str!("lib_part2.rs"),
            include_str!("lib_part3.rs"),
            include_str!("lib_part4.rs"),
            include_str!("lib_part5.rs"),
            include_str!("lib_part6.rs")
        )
        .replace("\r\n", "\n");
        let awaiting = lib_source
            .split("fn collect_awaiting_report_inner(")
            .nth(1)
            // The collector body ends at its closing brace in column 0.
            .and_then(|body| body.split("\n}\n").next())
            .expect("awaiting/status collector must remain inspectable");
        // STORY-1397 review: the awaiting render (incl. the per-turn notice)
        // is read-only and cheap. Reconciliation writes markers and briefs and
        // probes liveness, so it lives on the explicit `merge-hold list --fix`.
        for forbidden in [
            "reconcile_recusal_hold",
            "write_typed_hold",
            "merge_hold::write_hold",
            "sysinfo::",
        ] {
            assert!(
                !awaiting.contains(forbidden),
                "awaiting render must not call `{forbidden}`"
            );
        }
        assert!(
            awaiting.contains("awaiting_you::project_held_pr"),
            "every non-recusal held PR must be projected, not dropped"
        );
        // BUG-1773: a refusal recorded outside `aida review record` arms no
        // marker, so the corpus arm is the only thing that surfaces it. It is
        // part of the same read-only contract as the forbidden list above —
        // it derives the hold and writes neither marker nor label.
        // trace:BUG-1773 | ai:claude
        assert!(
            awaiting.contains("awaiting_you::corpus_held_prs"),
            "a refusal with no hold marker must still reach `held_prs`"
        );
        let handler = lib_source
            .split("fn handle_merge_hold(")
            .nth(1)
            .and_then(|body| body.split("MergeHoldAction::Add").next())
            .expect("merge-hold list handler must remain inspectable");
        assert!(
            handler.contains("merge_hold::reconcile_recusal_hold"),
            "`aida merge-hold list --fix` must reconcile recusal routing"
        );
        // TASK-189: the label column is a live (pinned) forge read; the
        // marker's recorded claim is shown only as `label_recorded`.
        assert!(handler.contains("merge_hold::fetch_pinned_change"));
        assert!(handler.contains("\"label\": forge_label.as_str()"));
        assert!(handler.contains("\"label_recorded\": recorded_of(pr)"));
        assert!(handler.contains("\"label_diverged\""));
        let own_source = include_str!("merge_hold.rs");
        let route = own_source
            .split("fn reconcile_recusal_route(")
            .nth(1)
            .and_then(|body| body.split("fn retire_stale_route_briefs(").next())
            .unwrap();
        assert!(
            !route.contains(concat!("new", "_all")),
            "routing must probe one pid, never scan the whole process table"
        );

        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida/agents")).unwrap();
        let mut record = typed_hold(44, HoldReasonKind::Recusal, "recused", Some("abc".into()));
        record.recused_principals = vec![PrincipalIdentity::parse("agent:author")];
        write_typed_hold(dir.path(), &record).unwrap();

        std::fs::write(dir.path().join(".aida/agents/reader.toml"), format!("id = \"reader\"\nagent_type = \"codex\"\npid = {}\nrole = \"reviewer\"\navailability = \"available\"\n", std::process::id())).unwrap();
        let routed = reconcile_recusal_hold(dir.path(), 44, Some("abc")).unwrap();
        assert_eq!(routed.routing_state, HoldRoutingState::Routed);
        assert!(dir
            .path()
            .join(".aida/agent-briefs/codex/PR-44-abc.md")
            .exists());

        std::fs::remove_file(dir.path().join(".aida/agents/reader.toml")).unwrap();
        let retired = reconcile_recusal_hold(dir.path(), 44, Some("abc")).unwrap();
        assert_eq!(retired.routing_state, HoldRoutingState::NoIndependentReader);
        assert!(dir
            .path()
            .join(".aida/agent-briefs/codex/PR-44-abc.md.stale")
            .exists());

        std::fs::write(dir.path().join(".aida/agents/new-reader.toml"), format!("id = \"new-reader\"\nagent_type = \"codex\"\npid = {}\nrole = \"reviewer\"\navailability = \"available\"\n", std::process::id())).unwrap();
        let moved = reconcile_recusal_hold(dir.path(), 44, Some("def")).unwrap();
        assert_eq!(moved.target_head_sha.as_deref(), Some("def"));
        assert_eq!(moved.routing_state, HoldRoutingState::Routed);
        assert!(dir
            .path()
            .join(".aida/agent-briefs/codex/PR-44-def.md")
            .exists());
    }

    // trace:BUG-1691 | ai:codex
    #[test]
    fn a_rework_hold_never_displaces_a_recusal_hold() {
        let dir = tempfile::tempdir().unwrap();
        let recusal = recused_record();
        write_typed_hold(dir.path(), &recusal).unwrap();
        let mut rework = typed_hold(
            42,
            HoldReasonKind::Rework,
            "CHANGES REQUESTED for BUG-1 at abc1234",
            Some("abc1234-full".into()),
        );
        rework.spec = Some("BUG-1".into());
        write_typed_hold(dir.path(), &rework).unwrap();
        let got = read_hold_record(dir.path(), 42).unwrap();
        assert_eq!(got.reason_kind, HoldReasonKind::Recusal);
        assert_eq!(got.recused_principals, recusal.recused_principals);
        assert_eq!(got.target_head_sha.as_deref(), Some("head-a"));
        assert_eq!(got.absorbed.len(), 1);
        assert_eq!(got.absorbed[0].reason_kind, HoldReasonKind::Rework);
        assert_eq!(got.absorbed[0].detail, rework.detail);
    }

    // trace:BUG-1691 | ai:codex
    #[test]
    fn recusal_written_after_rework_is_primary_and_absorbs_rework() {
        let dir = tempfile::tempdir().unwrap();
        write_typed_hold(
            dir.path(),
            &typed_hold(
                42,
                HoldReasonKind::Rework,
                "changes",
                Some("new-head".into()),
            ),
        )
        .unwrap();
        let recusal = recused_record();
        write_typed_hold(dir.path(), &recusal).unwrap();
        let got = read_hold_record(dir.path(), 42).unwrap();
        assert_eq!(got.reason_kind, HoldReasonKind::Recusal);
        assert_eq!(got.recused_principals, recusal.recused_principals);
        assert_eq!(got.target_head_sha.as_deref(), Some("head-a"));
        assert_eq!(
            got.absorbed
                .iter()
                .map(|a| a.reason_kind)
                .collect::<Vec<_>>(),
            vec![HoldReasonKind::Rework]
        );
    }

    // trace:BUG-1691 | ai:codex
    #[test]
    fn same_kind_refresh_unions_existing_absorbed_holds() {
        let dir = tempfile::tempdir().unwrap();
        let recusal = recused_record();
        write_typed_hold(dir.path(), &recusal).unwrap();
        write_typed_hold(
            dir.path(),
            &typed_hold(42, HoldReasonKind::Rework, "changes", None),
        )
        .unwrap();
        let mut refreshed = recused_record();
        refreshed.label_state = Some("synced".into());
        write_typed_hold(dir.path(), &refreshed).unwrap();
        let got = read_hold_record(dir.path(), 42).unwrap();
        assert_eq!(got.reason_kind, HoldReasonKind::Recusal);
        assert_eq!(got.label_state.as_deref(), Some("synced"));
        assert_eq!(got.absorbed.len(), 1);
        assert_eq!(got.absorbed[0].reason_kind, HoldReasonKind::Rework);
    }

    // trace:BUG-1691 | ai:codex
    #[test]
    fn composition_is_flat_bounded_and_unique_by_kind() {
        let mut record = typed_hold(42, HoldReasonKind::Supervision, "supervision", None);
        for (kind, detail) in [
            (HoldReasonKind::Rework, "rework"),
            (HoldReasonKind::Decision, "decision"),
            (HoldReasonKind::Rework, "rework refreshed"),
        ] {
            record = match compose_holds(&record, &typed_hold(42, kind, detail, None)) {
                Composed::Placed(r) | Composed::Preserved(r) => r,
            };
        }
        assert_eq!(record.reason_kind, HoldReasonKind::Decision);
        assert!(record.absorbed.len() <= 3);
        let mut kinds = record
            .absorbed
            .iter()
            .map(|a| a.reason_kind)
            .collect::<Vec<_>>();
        kinds.sort_by_key(|k| k.strictness());
        kinds.dedup();
        assert_eq!(kinds.len(), record.absorbed.len());
    }

    // trace:BUG-1691 | ai:codex
    #[test]
    fn composition_rejects_invalid_recusal_and_accepts_valid_existing_recusal() {
        let dir = tempfile::tempdir().unwrap();
        let mut invalid = recused_record();
        invalid.recused_principals.clear();
        assert!(write_typed_hold(dir.path(), &invalid).is_err());
        write_typed_hold(dir.path(), &recused_record()).unwrap();
        assert!(write_typed_hold(
            dir.path(),
            &typed_hold(42, HoldReasonKind::Rework, "changes", None)
        )
        .is_ok());
    }

    // trace:BUG-1691 | ai:codex
    #[test]
    fn hand_replace_discards_recusal_but_default_hand_add_refuses() {
        let dir = tempfile::tempdir().unwrap();
        write_typed_hold(dir.path(), &recused_record()).unwrap();
        let rework = typed_hold(42, HoldReasonKind::Rework, "manual rework", None);
        assert!(place_hand_hold(dir.path(), &rework, false).is_err());
        place_hand_hold(dir.path(), &rework, true).unwrap();
        let got = read_hold_record(dir.path(), 42).unwrap();
        assert_eq!(got.reason_kind, HoldReasonKind::Rework);
        assert!(got.absorbed.is_empty());
    }

    // trace:BUG-1691 | ai:codex
    #[test]
    fn old_typed_markers_without_absorbed_still_parse() {
        let dir = tempfile::tempdir().unwrap();
        let record = typed_hold(42, HoldReasonKind::Supervision, "old marker", None);
        let mut json = serde_json::to_value(record).unwrap();
        json.as_object_mut().unwrap().remove("absorbed");
        std::fs::create_dir_all(holds_dir(dir.path())).unwrap();
        std::fs::write(
            hold_path(dir.path(), 42),
            serde_json::to_vec(&json).unwrap(),
        )
        .unwrap();
        let got = read_hold_record(dir.path(), 42).unwrap();
        assert!(got.absorbed.is_empty());
    }

    // trace:BUG-1691 | ai:codex
    #[test]
    fn preserving_rework_write_does_not_reconcile_existing_recusal_route() {
        let dir = tempfile::tempdir().unwrap();
        let mut recusal = recused_record();
        recusal.routed_to = vec![PrincipalIdentity::parse("agent:cold-reader")];
        recusal.routing_state = HoldRoutingState::Routed;
        write_marker(dir.path(), &recusal, false).unwrap();
        let expected_route = recusal.routed_to.clone();
        write_typed_hold(
            dir.path(),
            &typed_hold(42, HoldReasonKind::Rework, "changes requested", None),
        )
        .unwrap();
        let got = read_hold_record(dir.path(), 42).unwrap();
        assert_eq!(got.routed_to, expected_route);
        assert_eq!(got.routing_state, HoldRoutingState::Routed);
        assert_ne!(got.routing_state, HoldRoutingState::NoIndependentReader);
    }

    // trace:BUG-1691 | ai:codex
    #[test]
    fn preserving_path_still_rejects_invalid_recusal_refresh() {
        let dir = tempfile::tempdir().unwrap();
        write_marker(dir.path(), &recused_record(), false).unwrap();
        let mut invalid = recused_record();
        invalid.recused_principals.clear();
        assert!(write_typed_hold(dir.path(), &invalid).is_err());
    }
}
