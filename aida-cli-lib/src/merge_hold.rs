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
}

impl MergeHoldRecord {
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

/// Write a v2 typed marker. JSON is intentionally self-contained so all
/// offline surfaces can route it without parsing operator prose.
// trace:STORY-1397 | ai:codex
pub(crate) fn write_typed_hold(
    project_root: &Path,
    record: &MergeHoldRecord,
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
    if normalized.reason_kind == HoldReasonKind::Recusal {
        reconcile_recusal_route(project_root, &mut normalized)?;
    }
    let body = serde_json::to_vec_pretty(&normalized)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    aida_core::fs_atomic::write_atomic(&hold_path(project_root, record.pr), &body)
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
}

pub(crate) fn clearance_path(project_root: &Path, pr: u64) -> PathBuf {
    project_root
        .join(".aida")
        .join("merge-hold-clearances")
        .join(format!("PR-{pr}.json"))
}

pub(crate) fn record_clearance(
    project_root: &Path,
    record: &MergeHoldRecord,
    actor: &PrincipalIdentity,
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
    read_hold_record(project_root, pr).map(|record| record.detail)
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

/// Whether the forge change currently carries the Layer-2 merge-hold label.
///
/// This is intentionally a live forge read rather than marker metadata: an
/// operator may add the label directly, leaving no Layer-1 marker to inspect.
/// Errors are returned so callers keep their existing fail-closed behavior.
// trace:TASK-1287 | ai:codex
pub(crate) fn label_present(project_root: &Path, pr: u64) -> Result<bool, String> {
    let kind = crate::forge::resolve_forge_kind(project_root);
    let (cli, args): (&str, Vec<String>) = match kind {
        crate::forge::ForgeKind::GitHub => (
            "gh",
            vec![
                "pr".into(),
                "view".into(),
                pr.to_string(),
                "--json".into(),
                "labels".into(),
                "--jq".into(),
                format!("any(.labels[]; .name == \"{HOLD_LABEL}\")"),
            ],
        ),
        crate::forge::ForgeKind::GitLab => (
            "glab",
            vec![
                "mr".into(),
                "view".into(),
                pr.to_string(),
                "--output".into(),
                "json".into(),
            ],
        ),
        crate::forge::ForgeKind::None => return Ok(false),
    };
    let (ok, stdout) = run_forge_cli_stdout(project_root, cli, &args)?;
    if !ok {
        return Err(format!("`{cli} {}` failed", args.join(" ")));
    }
    match kind {
        crate::forge::ForgeKind::GitHub => Ok(stdout.trim().eq_ignore_ascii_case("true")),
        crate::forge::ForgeKind::GitLab => labels_json_contains(&stdout, HOLD_LABEL),
        crate::forge::ForgeKind::None => Ok(false),
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

/// BUG-1236: whether the `aida:merge-hold` label on the change mirrors the
/// marker. Recorded on the marker's second line so `aida merge-hold list`
/// can show it without a network call and `--fix` can re-sync it.
// trace:BUG-1236 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LabelState {
    Synced,
    Unsynced(String),
    Unknown,
}

impl LabelState {
    pub(crate) fn render(&self) -> String {
        match self {
            LabelState::Synced => "label: synced".to_string(),
            LabelState::Unsynced(err) => format!("label: UNSYNCED — {err}"),
            LabelState::Unknown => "label: unknown".to_string(),
        }
    }
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

/// Best-effort mirror of the marker state to the `aida:merge-hold` label on the
/// change (PR/MR), so Layer 2 can enforce server-side. Failures are swallowed:
/// the file marker is the source of truth; the label is a convenience mirror and
/// its absence only weakens Layer 2, never the client chokepoint.
///
/// STORY-1165: forge-routed. GitHub → `gh pr edit --add-label/--remove-label`;
/// GitLab → `glab mr update --label/--unlabel`; pure-git → no-op (no forge to
/// label). This is what lets the GitLab merge-hold-gate CI job (the Layer-2
/// analog of merge-hold-gate.yml) see the label on an MR.
// trace:BUG-1167 | ai:claude (STORY-1165 forge-routes it)
pub(crate) fn sync_label(project_root: &Path, pr: u64, held: bool) -> Result<(), String> {
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
) -> Result<(), String> {
    let kind = crate::forge::resolve_forge_kind(project_root);
    let Some((cli, args)) = sync_label_command(kind, pr, held) else {
        // pure-git has no forge to carry a label; the file marker still holds.
        return Ok(());
    };
    let mut last_err = String::new();
    for attempt in 0..2 {
        match runner(project_root, cli, &args) {
            Ok((true, _)) => {
                if held {
                    let _ = record_label_state(project_root, pr, &LabelState::Synced);
                }
                return Ok(());
            }
            Ok((false, stderr)) => {
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
    Err(format!("`{cli} {}` failed: {last_err}", args.join(" ")))
}

fn run_forge_cli(
    project_root: &Path,
    cli: &str,
    args: &[String],
) -> Result<(bool, String), String> {
    let out = std::process::Command::new(cli)
        .current_dir(project_root)
        .args(args)
        .output()
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
        .output()
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
fn sync_label_command(
    kind: crate::forge::ForgeKind,
    pr: u64,
    held: bool,
) -> Option<(&'static str, Vec<String>)> {
    use crate::forge::ForgeKind;
    match kind {
        ForgeKind::GitHub => {
            let flag = if held {
                "--add-label"
            } else {
                "--remove-label"
            };
            Some((
                "gh",
                vec![
                    "pr".into(),
                    "edit".into(),
                    pr.to_string(),
                    flag.into(),
                    HOLD_LABEL.into(),
                ],
            ))
        }
        ForgeKind::GitLab => {
            let flag = if held { "--label" } else { "--unlabel" };
            Some((
                "glab",
                vec![
                    "mr".into(),
                    "update".into(),
                    pr.to_string(),
                    flag.into(),
                    HOLD_LABEL.into(),
                ],
            ))
        }
        ForgeKind::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // STORY-1165: the label mirror must route to the right forge CLI — gh for
    // GitHub, glab for GitLab (mr update --label/--unlabel), nothing for pure-git.
    #[test]
    fn sync_label_command_routes_per_forge() {
        use crate::forge::ForgeKind;
        let (cli, args) = sync_label_command(ForgeKind::GitHub, 42, true).unwrap();
        assert_eq!(cli, "gh");
        assert!(
            args.contains(&"--add-label".to_string()) && args.contains(&HOLD_LABEL.to_string())
        );

        let (cli, args) = sync_label_command(ForgeKind::GitLab, 42, true).unwrap();
        assert_eq!(cli, "glab");
        assert!(args.contains(&"mr".to_string()) && args.contains(&"update".to_string()));
        assert!(args.contains(&"--label".to_string()) && args.contains(&HOLD_LABEL.to_string()));

        let (_, args) = sync_label_command(ForgeKind::GitLab, 42, false).unwrap();
        assert!(
            args.contains(&"--unlabel".to_string()),
            "unheld → remove the label"
        );

        assert!(
            sync_label_command(ForgeKind::None, 42, true).is_none(),
            "pure-git has no forge to label"
        );
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
        assert!(err.contains("502"), "{err}");
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
        let lib_source = include_str!("lib.rs");
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
        let handler = lib_source
            .split("fn handle_merge_hold(")
            .nth(1)
            .and_then(|body| body.split("MergeHoldAction::Add").next())
            .expect("merge-hold list handler must remain inspectable");
        assert!(
            handler.contains("merge_hold::reconcile_recusal_hold"),
            "`aida merge-hold list --fix` must reconcile recusal routing"
        );
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
}
