//! Merge-hold core (STORY-1889, from `merge_hold.rs`): the hold record
//! types, the read path that says whether a hold exists (fail-closed on a
//! missing or unreadable marker), the marker write and clear path (a write
//! that silently fails leaves a PR unheld), and the hold decisions: who may
//! clear a hold, when a refusal hold is released, how holds compose, and the
//! corpus merge gate. Forge labels, pinned-change reads and stray-marker
//! adoption stay in `merge_hold.rs` and call in.
// trace:BUG-1167 | ai:claude
// mandate-gate
// trace:STORY-1889 | ai:claude

use crate::merge_hold::{reconcile_recusal_route, sync_label};
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
            // trace:STORY-1697 | ai:claude
            format!(
                "a fresh APPROVED verdict for {key} recorded at the PR's current head by an independent reviewer releases it (a sensitive spec also needs a human clear or an --include-supervised mandate)"
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

/// BUG-2176: the checkout whose `.aida/merge-holds/` and
/// `.aida/merge-hold-clearances/` every reader and writer shares — the
/// primary checkout, the same main checkout the merge chokepoint reads the
/// launcher's review verdicts from. A caller in a linked worktree (a headless
/// reviewer in `aida-pr-N`) used to write its marker into that worktree,
/// where `aida pr ship` and auto-release never saw it and removing the
/// worktree destroyed it. A seat clone that does not verify keeps its own
/// root, as before, rather than pointing at the unverified-clone sentinel
/// where every hold read would answer "no hold".
///
/// Every `hold_path` / `clearance_path` call lands here, and a hold-read miss
/// walks every placed PR, so this never spawns git: the main checkout is read
/// from `.git` / `commondir` on disk ([`primary_checkout_from_files`]) and
/// memoised per process. An unverified seat clone is never memoised, so it
/// resolves afresh once its lease verifies.
// trace:BUG-2176 | ai:claude
pub(crate) fn hold_store_root(project_root: &Path) -> PathBuf {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static MEMO: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    if let Some(hit) = memo.lock().ok().and_then(|m| m.get(project_root).cloned()) {
        return hit;
    }
    let resolved = match crate::seat_clone::main_for(project_root) {
        crate::seat_clone::MainFor::Unverified(_) => return project_root.to_path_buf(),
        crate::seat_clone::MainFor::Verified(main) => main,
        crate::seat_clone::MainFor::NotAClone => {
            // A root with no `.git` costs one stat and stays unmemoised, so a
            // directory that later becomes a checkout is not pinned stale.
            if std::fs::symlink_metadata(project_root.join(".git")).is_err() {
                return project_root.to_path_buf();
            }
            primary_checkout_from_files(project_root)
        }
    };
    if let Ok(mut m) = memo.lock() {
        m.insert(project_root.to_path_buf(), resolved.clone());
    }
    resolved
}

#[cfg(test)]
thread_local! {
    /// Uncached primary-checkout resolutions on this thread.
    pub(crate) static STORE_RESOLUTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The main checkout of `root`, read from git's own files without running git:
/// a `.git` directory makes `root` the main checkout; a linked worktree's
/// `.git` file names its gitdir, whose `commondir` is the main checkout's
/// `.git`. Anything else (a submodule, a bare main, an unreadable file) keeps
/// `root`, which is where the marker lived before BUG-2176.
// trace:BUG-2176 | ai:claude
fn primary_checkout_from_files(root: &Path) -> PathBuf {
    #[cfg(test)]
    STORE_RESOLUTIONS.with(|n| n.set(n.get() + 1));
    let dot_git = root.join(".git");
    let Ok(meta) = std::fs::symlink_metadata(&dot_git) else {
        return root.to_path_buf();
    };
    if !meta.is_file() {
        return root.to_path_buf();
    }
    let Some(gitdir) = std::fs::read_to_string(&dot_git).ok().and_then(|body| {
        body.lines()
            .find_map(|l| l.strip_prefix("gitdir:"))
            .map(|d| root.join(d.trim()))
    }) else {
        return root.to_path_buf();
    };
    let Ok(common) = std::fs::read_to_string(gitdir.join("commondir")) else {
        return root.to_path_buf();
    };
    let common = gitdir.join(common.trim());
    let common = std::fs::canonicalize(&common).unwrap_or(common);
    match (common.file_name(), common.parent()) {
        (Some(name), Some(main)) if name == ".git" => main.to_path_buf(),
        _ => root.to_path_buf(),
    }
}

/// Tests: the legacy marker path a pre-ledger binary wrote, for fixtures
/// that hand-write the legacy shapes the read-through must keep honouring.
#[cfg(test)]
pub(crate) fn hold_path(project_root: &Path, pr: u64) -> PathBuf {
    crate::gate::ledger::legacy::hold_marker_path(&hold_store_root(project_root), pr)
}

/// Whether PR `pr` is under an open hold, read from `project_root`.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn hold_is_open(project_root: &Path, pr: u64) -> bool {
    read_hold_record(project_root, pr).is_some()
}

/// When PR `pr`'s open hold was placed: its first ledger placement since the
/// last release, or a legacy marker's mtime.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn hold_placed_at(project_root: &Path, pr: u64) -> Option<std::time::SystemTime> {
    crate::gate::ledger::hold::state(project_root, pr).placed_at
}

/// When PR `pr`'s open hold record was last written (a legacy marker's
/// mtime, or its newest ledger placement).
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn hold_written_at(project_root: &Path, pr: u64) -> Option<std::time::SystemTime> {
    crate::gate::ledger::hold::state(project_root, pr).written_at
}

/// Where PR `pr`'s hold state is stored, for messages: its gate ledger file.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn hold_ledger_file(project_root: &Path, pr: u64) -> PathBuf {
    crate::gate::ledger::store::ledger_file(
        &crate::gate::ledger::hold::read_root(project_root),
        crate::gate::ledger::ChangeKey::pr(pr),
    )
}

/// Compose an incoming hold with the marker already present on the PR.
// trace:BUG-1691 | ai:codex
// trace:BUG-1691 | ai:codex
pub(crate) enum Composed {
    Placed(MergeHoldRecord),
    Preserved(MergeHoldRecord),
}

// trace:BUG-1691 | ai:codex
pub(crate) fn compose_with_existing(project_root: &Path, incoming: &MergeHoldRecord) -> Composed {
    let Some(existing) = read_hold_record(project_root, incoming.pr) else {
        return Composed::Placed(incoming.clone());
    };
    compose_holds(&existing, incoming)
}

/// Pure precedence policy for two hold records.
// trace:BUG-1691 | ai:codex
pub(crate) fn compose_holds(existing: &MergeHoldRecord, incoming: &MergeHoldRecord) -> Composed {
    if existing.reason_kind == incoming.reason_kind {
        let mut out = incoming.clone();
        let mut carried = existing.absorbed.clone();
        // BUG-2123: a Rework hold armed by a refusal on another verdict keeps
        // that refusal as a shadow; replacing it would let an approval of
        // the newer one release a refusal nobody answered.
        // trace:TASK-1951 trace:BUG-2123 | ai:claude
        if shadow_key(existing.reason_kind, existing.verdict_ref.as_ref())
            != shadow_key(incoming.reason_kind, incoming.verdict_ref.as_ref())
        {
            carried.push(AbsorbedHold::from_record(existing));
        }
        merge_absorbed(&mut out.absorbed, &carried);
        retain_distinct_shadows(&mut out);
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
    retain_distinct_shadows(&mut primary);
    if preserved {
        Composed::Preserved(primary)
    } else {
        Composed::Placed(primary)
    }
}

/// What makes two holds the same hold for composition: the kind, and for a
/// Rework hold also the verdict it stands on (BUG-2123). Other kinds have
/// one shadow per kind.
// trace:TASK-1951 trace:BUG-2123 | ai:claude
fn shadow_key(
    kind: HoldReasonKind,
    verdict: Option<&VerdictRef>,
) -> (HoldReasonKind, Option<&str>) {
    match kind {
        HoldReasonKind::Rework => (kind, verdict.map(|v| v.key.as_str())),
        _ => (kind, None),
    }
}

/// Drop shadows that are the primary hold itself.
// trace:TASK-1951 trace:BUG-2123 | ai:claude
fn retain_distinct_shadows(primary: &mut MergeHoldRecord) {
    let own = shadow_key(primary.reason_kind, primary.verdict_ref.as_ref());
    let own = (own.0, own.1.map(str::to_string));
    primary.absorbed.retain(|a| {
        let key = shadow_key(a.reason_kind, a.verdict_ref.as_ref());
        (key.0, key.1.map(str::to_string)) != own
    });
}

/// Union absorbed shadows by [`shadow_key`], with the newest entry taking
/// precedence.
// trace:BUG-1691 | ai:codex
fn merge_absorbed(into: &mut Vec<AbsorbedHold>, extra: &[AbsorbedHold]) {
    for e in extra {
        let key = shadow_key(e.reason_kind, e.verdict_ref.as_ref());
        if let Some(slot) = into
            .iter_mut()
            .find(|a| shadow_key(a.reason_kind, a.verdict_ref.as_ref()) == key)
        {
            *slot = e.clone();
        } else {
            into.push(e.clone());
        }
    }
    into.sort_by_key(|a| std::cmp::Reverse(a.reason_kind.strictness()));
}

/// Parse typed and legacy markers. A malformed marker remains a typed Unknown
/// hold, preserving the merge chokepoint while exposing the diagnostic.
// trace:STORY-1397 | ai:codex
///
/// M1 slice 2: the open hold on PR `pr`'s gate ledger, with legacy markers
/// read through ([`crate::gate::ledger::hold::state`]).
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn read_hold_record(project_root: &Path, pr: u64) -> Option<MergeHoldRecord> {
    crate::gate::ledger::hold::state(project_root, pr).open
}

/// The record a marker read yields — `None` only for a missing marker.
// trace:BUG-2176 | ai:claude
pub(crate) fn parse_hold_marker(read: std::io::Result<String>, pr: u64) -> Option<MergeHoldRecord> {
    match read {
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
    /// STORY-1698: the mandate under which the advisor released this hold,
    /// when one did. `None` = no mandate was involved.
    // trace:STORY-1698 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_under_mandate: Option<String>,
}

/// Tests: the legacy clearance path a pre-ledger binary wrote.
#[cfg(test)]
pub(crate) fn clearance_path(project_root: &Path, pr: u64) -> PathBuf {
    crate::gate::ledger::legacy::clearance_file_path(&hold_store_root(project_root), pr)
}

/// The clearance of the release that closed PR `pr`'s latest hold, while no
/// hold is open: the `hold-release` event on its ledger, or a legacy
/// clearance file the ledger has not superseded.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn recorded_clearance(
    project_root: &Path,
    pr: u64,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    crate::gate::ledger::hold::state(project_root, pr).release
}

/// Local audit signal for a hold placed through AIDA whose marker disappeared
/// without the human clearance command recording a release.
// trace:BUG-1693 | ai:codex
pub(crate) fn unrecorded_marker_removals(project_root: &Path) -> Vec<u64> {
    use crate::events::EventKind;
    let mut latest = std::collections::BTreeMap::<u32, bool>::new();
    // trace:TASK-1951 trace:TASK-1406 | ai:claude
    for event in crate::events::read_all_with_archive(&hold_store_root(project_root)) {
        if let EventKind::MergeHoldChanged { pr, placed, .. } = event.kind {
            latest.insert(pr, placed);
        }
    }
    latest
        .into_iter()
        .filter_map(|(pr, placed)| {
            let pr = u64::from(pr);
            // trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
            let state = crate::gate::ledger::hold::state(project_root, pr);
            (placed && state.open.is_none() && state.release.is_none()).then_some(pr)
        })
        .collect()
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

/// Every active hold as `(pr_number, reason)`, ascending by PR.
// Consumed by the follow-up `aida merge-hold list` surface; kept here so the
// primitive lands with the safety core.
#[allow(dead_code)]
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn list_holds(project_root: &Path) -> Vec<(u64, String)> {
    crate::gate::ledger::hold::candidate_prs(project_root)
        .into_iter()
        .filter_map(|n| read_hold_record(project_root, n).map(|r| (n, r.detail)))
        .collect()
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
    current_of: impl Fn(&str) -> Option<crate::gate::review_verdict::RecordedVerdict>,
    at_sha: impl Fn(&str, &str) -> Option<crate::gate::review_verdict::RecordedVerdict>,
) -> RefusalRelease {
    use crate::gate::review_verdict::{same_reviewed_sha, short_sha, VerdictKind};
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
    corpus_merge_gate_carrying(pr, bodies, head, &|_| None)
}

/// The TASK-1960 carry-over prover the corpus gate consults: the SAME proof
/// the approval-head gate applies (`review_carryover::prove_for_refusal`),
/// asked about a `Moved` refusal from an approved sha to the head. `None` =
/// no carry-over is available here.
// trace:TASK-1968 | ai:claude
pub(crate) type CorpusCarryOverProver<'a> = &'a dyn Fn(
    &crate::gate::ship::ApprovalHeadRefusal,
) -> Option<
    Result<
        crate::gate::review_carryover::CarryOver,
        crate::gate::review_carryover::CarryOverRefusal,
    >,
>;

/// [`corpus_merge_gate`], except that a head nothing approves may still
/// clear when an approval at an older head carries over to it (TASK-1968):
/// every live verdict's current recording approves, the corpus cleanly
/// approves that older sha, and `prove` shows the head moved only by clean
/// merges of the protected base with the PR's own diff unchanged. Anything
/// short of that refuses exactly as [`corpus_merge_gate`] does.
// trace:TASK-1968 | ai:claude
pub(crate) fn corpus_merge_gate_carrying(
    pr: u64,
    bodies: &[(String, String)],
    head: Option<&str>,
    prove: CorpusCarryOverProver<'_>,
) -> CorpusMergeGate {
    use crate::gate::review_verdict as rv;
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
        //
        // TASK-1968: the one exception is TASK-1960's carry-over — an
        // approval at an older head whose only successors are clean merges of
        // the protected base, with the PR's own diff unchanged.
        // trace:TASK-1968 | ai:claude
        Ok(None) => {
            let not_carried = match corpus_carry_over(&live, head, prove) {
                Some(Ok(_)) => return CorpusMergeGate::Clear,
                Some(Err(why)) => format!(" The approval did not carry over: {}.", why.reason()),
                None => String::new(),
            };
            CorpusMergeGate::Refuse {
                message: format!(
                    "PR-{pr}'s recorded review verdicts ({first_key}) do not approve its current \
                     head {} — a head move alone does not lift a recorded refusal, and an \
                     approval at an older commit is not evidence this head was reviewed.\
                     {not_carried} {}",
                    rv::short_sha(head),
                    corpus_release_hint(pr, &first_key)
                ),
                arm: None,
            }
        }
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

/// Whether an approval in the live corpus carries over to `head`. `None` =
/// no approval is eligible to carry over: a live artifact that does not
/// parse, has no reviewed sha, or whose current recording does not approve
/// (a standing refusal at ANY sha is never carried past), or no approved sha
/// the corpus itself cleanly approves. Otherwise each eligible approved sha
/// is proven in turn; the first proof wins, else the last refusal is
/// returned for the message.
// trace:TASK-1968 | ai:claude
fn corpus_carry_over(
    live: &[&(String, String)],
    head: &str,
    prove: CorpusCarryOverProver<'_>,
) -> Option<
    Result<
        crate::gate::review_carryover::CarryOver,
        crate::gate::review_carryover::CarryOverRefusal,
    >,
> {
    use crate::gate::review_verdict as rv;
    let mut approved: Vec<String> = Vec::new();
    for (_, body) in live {
        let verdict = rv::parse_recorded_verdict(body)?;
        if !verdict.kind.approves() {
            return None;
        }
        let sha = verdict.reviewed_sha?;
        let sha = sha.trim();
        if sha.is_empty() || rv::same_reviewed_sha(sha, head) {
            return None;
        }
        if !approved.iter().any(|a| rv::same_reviewed_sha(a, sha)) {
            approved.push(sha.to_string());
        }
    }
    let mut last = None;
    for sha in approved {
        let corpus_approves = matches!(
            rv::reconcile_artifacts_for_sha(live.iter().map(|(_, b)| b.as_str()), &sha),
            Ok(Some(kind)) if kind.approves()
        );
        if !corpus_approves {
            continue;
        }
        let refusal = crate::gate::ship::ApprovalHeadRefusal::Moved {
            reviewed_sha: sha,
            head_sha: head.to_string(),
        };
        match prove(&refusal)? {
            Ok(proof) => return Some(Ok(proof)),
            Err(why) => last = Some(Err(why)),
        }
    }
    last
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
    // M1: a verdict collected from a contained reviewer is on the change's
    // gate ledger only. Each key's newest ledger record is weighed in place
    // of a file it is newer than (round 3: a file must not hide a newer
    // ledger-only verdict) and alone for a key with no file under any root,
    // so a ledger-only refusal still blocks. Each record counts once.
    // trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
    let mut stores: Vec<PathBuf> = Vec::new();
    for root in roots {
        if let Ok(store) = crate::gate::ledger::store_root(root) {
            if !stores.contains(&store) {
                stores.push(store);
            }
        }
    }
    let ledger: Vec<(String, Vec<crate::gate::ledger::GateEvent>)> = keys
        .iter()
        .map(|key| {
            let events = stores
                .iter()
                .filter_map(|store| crate::gate::ledger::project::newest_verdict_event(store, key))
                .collect();
            (key.clone(), events)
        })
        .collect();
    let ledger_body = |event: &crate::gate::ledger::GateEvent| -> Result<String, String> {
        serde_json::to_string(event.verdict_record().unwrap_or(&Default::default()))
            .map_err(|e| e.to_string())
    };
    let mut counted: Vec<(String, usize)> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut out = Vec::new();
    for root in roots {
        for (key, events) in &ledger {
            let path = crate::gate::review_verdict::verdict_path(root, key);
            if std::fs::symlink_metadata(&path).is_err() {
                continue;
            }
            let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if seen.contains(&canon) {
                continue;
            }
            seen.push(canon);
            // trace:TASK-1801 | ai:claude
            let body = match crate::gate::review_verdict::read_verdict_body(&path) {
                Ok(body) => body,
                Err(e) => {
                    return Err(format!(
                        "verdict artifact {} exists but could not be read ({e})",
                        path.display()
                    ))
                }
            };
            // An unparseable file stays as written: the gate fails closed on it.
            let file = crate::gate::review_verdict::parse_recorded_verdict(&body);
            let newer = file.as_ref().and_then(|file| {
                events.iter().position(|event| {
                    crate::gate::review_verdict::ledger_supersedes(&path, Some(file), event)
                })
            });
            match newer {
                Some(i) if counted.contains(&(key.clone(), i)) => {}
                Some(i) => {
                    counted.push((key.clone(), i));
                    out.push((key.clone(), ledger_body(&events[i])?));
                }
                None => out.push((key.clone(), body)),
            }
        }
    }
    for (key, events) in &ledger {
        if out.iter().any(|(k, _)| k == key) {
            continue;
        }
        for event in events {
            out.push((key.clone(), ledger_body(event)?));
        }
    }
    Ok(out)
}

/// The chokepoint entry every `Forge::merge_change` implementation calls.
/// `head` is resolved lazily — a PR with no verdict artifacts pays no forge
/// read. On a definite at-head refusal the mirrors are refreshed best-effort
/// from the gate's own `CorpusArm` (never from a second predicate), and the
/// refusal stands whether or not the mirrors landed.
///
/// `base_branch` is the PR's base as the caller knows it (`None` = read it
/// from the forge); a carry-over is proven against the remote's default
/// branch and only for a PR that targets it (TASK-1968). The proof here is
/// quiet: it records nothing, since the approval gate that pins the merge
/// records and announces it.
// trace:BUG-1774 | ai:claude
// trace:TASK-1968 | ai:claude
pub(crate) fn enforce_corpus_gate_before_merge(
    project_root: &Path,
    pr: u64,
    spec_hints: &[String],
    base_branch: Option<&str>,
    head: impl FnOnce() -> Option<String>,
) -> anyhow::Result<()> {
    let decision = corpus_gate_decision(project_root, pr, spec_hints, base_branch, head, true)?;
    enforce_corpus_decision(project_root, pr, decision)
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-179 | ai:codex
/// Shared enforcement after evidence coverage has been evaluated: the train
/// keeps the same refusal and hold mirrors as every ordinary merge.
pub(crate) fn enforce_corpus_decision(
    project_root: &Path,
    pr: u64,
    decision: CorpusMergeGate,
) -> anyhow::Result<()> {
    match decision {
        CorpusMergeGate::Clear => Ok(()),
        CorpusMergeGate::Refuse { message, arm } => {
            if let Some(arm) = arm.filter(|_| pr > 0) {
                arm_corpus_mirrors(project_root, pr, &arm);
            }
            anyhow::bail!(message)
        }
    }
}

/// [`enforce_corpus_gate_before_merge`]'s decision: the mirrors a refusal
/// arms are the caller's to write. `Err` when a verdict artifact exists but
/// cannot be read. With `record_proof`, a carry-over proven here goes on the
/// change's gate ledger for every later gate (STORY-080 AC4); without it
/// (a read-only report) the decision leaves no state behind.
// trace:TASK-1945 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn corpus_gate_decision(
    project_root: &Path,
    pr: u64,
    spec_hints: &[String],
    base_branch: Option<&str>,
    head: impl FnOnce() -> Option<String>,
    record_proof: bool,
) -> anyhow::Result<CorpusMergeGate> {
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
        return Ok(CorpusMergeGate::Clear);
    }
    let head = head();
    let base_branch = base_branch.map(str::trim).filter(|b| !b.is_empty());
    let site = if record_proof {
        crate::gate::review_carryover::CarryOverSite::quiet(main_root.as_path(), base_branch)
    } else {
        crate::gate::review_carryover::CarryOverSite::read_only(main_root.as_path(), base_branch)
    };
    let prove = |refusal: &crate::gate::ship::ApprovalHeadRefusal| {
        crate::gate::review_carryover::prove_for_refusal(&site, pr, refusal)
    };
    Ok(corpus_merge_gate_carrying(
        pr,
        &bodies,
        head.as_deref(),
        &prove,
    ))
}

/// Refresh the marker + label mirrors from the gate's definite at-head
/// refusal. Best-effort: a mirror failure is reported, never escalated — the
/// chokepoint refusal already stands. `place_hold` composes with any
/// existing marker, so this programmatic Rework write cannot displace a
/// stricter hold (a Recusal stays primary — BUG-1691).
// trace:BUG-1774 | ai:claude
fn arm_corpus_mirrors(project_root: &Path, pr: u64, arm: &CorpusArm) {
    let reason = format!(
        "{} for {} at {}",
        arm.verdict_raw,
        arm.key,
        crate::gate::review_verdict::short_sha(&arm.sha)
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
    // trace:STORY-1697 | ai:claude
    hold.release_condition = Some(format!(
        "a fresh APPROVED verdict for {} recorded at PR #{pr}'s current head by an independent reviewer releases it (a sensitive spec also needs a human clear or an --include-supervised mandate)",
        arm.key
    ));
    if let Err(err) = place_hold(project_root, &hold) {
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

/// Record a supervised merge-hold for `pr` with a human-readable reason.
/// Idempotent — re-recording refreshes the reason.
///
/// STORY-1397: production writers all emit typed v2 holds via
/// [`place_hold`]; this legacy plaintext marker writer survives only so tests
/// can build the legacy marker shape the read-through must keep honouring.
// trace:STORY-1397 | ai:claude
#[cfg(test)]
pub(crate) fn write_hold(project_root: &Path, pr: u64, reason: &str) -> std::io::Result<()> {
    let path = hold_path(project_root, pr);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let reason = reason.trim();
    let body = if reason.is_empty() {
        format!("PR-{pr} is under a supervised merge-hold\n")
    } else {
        format!("{reason}\n")
    };
    std::fs::write(path, body)
}

/// Place a v2 typed hold on the PR's gate ledger, preserving stricter
/// existing gates.
// trace:STORY-1397 trace:BUG-1691 | ai:codex
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn place_hold(project_root: &Path, record: &MergeHoldRecord) -> std::io::Result<()> {
    place_hold_context(project_root, record, false)
}

/// The same composition and placement, with explicit re-arm audit context.
// trace:TASK-1790 | ai:codex
pub(crate) fn rearm_hold(project_root: &Path, record: &MergeHoldRecord) -> std::io::Result<()> {
    place_hold_context(project_root, record, true)
}

fn place_hold_context(
    project_root: &Path,
    record: &MergeHoldRecord,
    rearmed: bool,
) -> std::io::Result<()> {
    // trace:TASK-1951 trace:TASK-1757 | ai:claude
    with_hold_lock(project_root, record.pr, || {
        match compose_with_existing(project_root, record) {
            Composed::Placed(r) => append_hold(project_root, &r, true, rearmed),
            Composed::Preserved(r) => append_hold(project_root, &r, false, rearmed),
        }
    })?
}

/// Replace the open hold; reserved for the explicit operator escape hatch.
// trace:STORY-1397 trace:BUG-1691 | ai:codex
fn place_hold_replacing(project_root: &Path, record: &MergeHoldRecord) -> std::io::Result<()> {
    // trace:TASK-1951 trace:TASK-1757 | ai:claude
    with_hold_lock(project_root, record.pr, || {
        append_hold(project_root, record, true, false)
    })?
}

// trace:BUG-1691 | ai:codex
pub(crate) fn append_hold(
    project_root: &Path,
    record: &MergeHoldRecord,
    reconcile_route: bool,
    rearmed: bool,
) -> std::io::Result<()> {
    append_hold_with_route(project_root, record, reconcile_route, rearmed, |proposal| {
        reconcile_recusal_route(project_root, proposal)
    })
}

/// Validate, normalize and route `record`, then append it to the PR's gate
/// ledger as a `hold` event (M1 slice 2). There is no worktree-local marker:
/// a contained caller is refused, and the launcher that started a contained
/// reviewer arms the hold for the refusal it collects.
// trace:STORY-1889 | ai:agy
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn append_hold_with_route(
    project_root: &Path,
    record: &MergeHoldRecord,
    reconcile_route: bool,
    rearmed: bool,
    route_fn: impl FnOnce(&mut MergeHoldRecord) -> std::io::Result<()>,
) -> std::io::Result<()> {
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
        apply_recusal_route_proposal(&mut normalized, route_fn)?;
    }
    append_hold_record(project_root, &normalized, rearmed)
}

/// Collect a hold read from a legacy marker file a reviewer worktree still
/// carries onto the PR's gate ledger, composed with any open hold. Once on
/// the ledger it is no longer a legacy marker: an untyped one (a reviewer's
/// refusal, which the ship gate read as one) is recorded as the typed Rework
/// hold it stood for, so it releases like any other refusal. The caller
/// removes the worktree copy afterwards.
// trace:BUG-2176 trace:STORY-1917 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn collect_legacy_hold(
    project_root: &Path,
    record: &MergeHoldRecord,
) -> std::io::Result<()> {
    if record.pr == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a merge hold needs a PR number",
        ));
    }
    let record = &collected_hold(record);
    with_hold_lock(project_root, record.pr, || {
        let composed = match compose_with_existing(project_root, record) {
            Composed::Placed(r) | Composed::Preserved(r) => r,
        };
        append_hold_record(project_root, &composed, false)
    })?
}

/// The ledger form of a collected worktree marker: an untyped (`legacy`)
/// marker becomes a typed Rework hold carrying its text; a typed or
/// unreadable one is kept as read (an unreadable one stays Unknown and holds
/// for safety).
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn collected_hold(record: &MergeHoldRecord) -> MergeHoldRecord {
    if !record.legacy {
        return record.clone();
    }
    let mut typed = typed_hold(
        record.pr,
        HoldReasonKind::Rework,
        record.detail.clone(),
        record.target_head_sha.clone(),
    );
    typed.label_state = record.label_state.clone();
    typed.spec = record.spec.clone();
    typed.release_condition = Some(format!(
        "a fresh APPROVED verdict recorded at PR #{}'s current head by an independent reviewer releases it",
        record.pr
    ));
    typed
}

/// Append `record` as a `hold` event and record its placement in the
/// events log of the store root.
fn append_hold_record(
    project_root: &Path,
    record: &MergeHoldRecord,
    rearmed: bool,
) -> std::io::Result<()> {
    // A real placement supersedes a pending auto-release authorization
    // before the new hold lands, so no release can act on it (TASK-1757).
    // trace:TASK-1757 trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
    if crate::gate::ledger::hold::state(project_root, record.pr)
        .authorized
        .is_some()
    {
        crate::gate::ledger::hold::append(
            project_root,
            record.pr,
            crate::gate::ledger::EventBody::ReleaseWithdrawn {
                reason: "superseded by a new hold placement".to_string(),
            },
            None,
            None,
        )?;
    }
    crate::gate::ledger::hold::place(project_root, record, rearmed)?;
    // TASK-1406: the placement event goes to the events log of the store
    // root, so the primary's tampering check sees holds placed from a
    // linked worktree. trace:TASK-1951 trace:TASK-1406 | ai:claude
    //
    // TASK-2138: the readers that decide anything from hold events read
    // that log: `unrecorded_marker_removals` and lane-liveness's
    // `load_human_holds`. trace:TASK-1951 trace:TASK-2138 | ai:claude
    // trace:BUG-1693 | ai:codex
    let mut event = crate::events::Event::new(
        None,
        "",
        crate::events::EventKind::MergeHoldChanged {
            pr: record.pr as u32,
            placed: true,
            // Only the event reason changes; the persisted hold keeps its
            // original composition, reason and clearance identity.
            // trace:TASK-1790 | ai:codex
            reason: Some(if rearmed {
                format!("Re-armed after mandate ship failure: {}", record.detail)
            } else {
                record.detail.clone()
            }),
            // trace:BUG-2214 | ai:claude
            principal: None,
            human_attested: false,
        },
    );
    event.seat = crate::events::active_seat();
    crate::events::emit(&hold_store_root(project_root), &event);
    Ok(())
}

/// BUG-1562: place a hold by hand (`aida merge-hold add`) WITHOUT silently
/// replacing an open one. An open hold — typed, legacy, hand-written, even an
/// unreadable legacy marker — refuses unless `replace`, and the refusal
/// quotes what would have been lost. The label half is the caller's.
// trace:BUG-1562 | ai:claude
pub(crate) fn place_hold_by_hand(
    project_root: &Path,
    record: &MergeHoldRecord,
    replace: bool,
) -> Result<(), String> {
    if !replace {
        if let Some(existing) = read_hold_record(project_root, record.pr) {
            let first = existing
                .detail
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            return Err(format!(
                "PR #{pr} already has a merge hold ({shown}); `aida merge-hold add` will not \
                 overwrite it. Keep it and re-apply a missing label with \
                 `aida merge-hold list --fix`, or replace it deliberately with \
                 `aida merge-hold add {pr} --replace ...`.",
                pr = record.pr,
                shown = if first.is_empty() {
                    hold_ledger_file(project_root, record.pr)
                        .display()
                        .to_string()
                } else {
                    format!("\"{first}\"")
                }
            ));
        }
    }
    place_hold_replacing(project_root, record).map_err(|e| e.to_string())
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

/// Run `route_fn` on a cloned proposal and copy ONLY `routed_to` and
/// `routing_state` back onto the admitted `record`, so unprotected routing
/// plumbing outside `gate/` can never mutate `reason_kind`, `legacy`,
/// `absorbed`, `recused_principals`, or any other authority-bearing field.
// trace:STORY-1889 | ai:agy
pub(crate) fn apply_recusal_route_proposal(
    record: &mut MergeHoldRecord,
    route_fn: impl FnOnce(&mut MergeHoldRecord) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut proposal = record.clone();
    route_fn(&mut proposal)?;
    record.routed_to = proposal.routed_to;
    record.routing_state = proposal.routing_state;
    Ok(())
}

/// Refresh a recusal hold from live evidence: adopt a moved PR head (which
/// invalidates the old route), re-select a live independent reader, and write
/// or retire the routing briefs. Idempotent. A failed refresh leaves the
/// existing marker armed and returns the last durable record.
///
/// This WRITES (marker rewrite, brief creation/retirement) and probes process
/// liveness, so it is called only from explicit merge-hold commands
/// (`aida merge-hold list --fix`, and `add` via [`place_hold`]) — never
/// from a read surface. `aida awaiting` and its per-turn notice stay read-only
/// and use [`project_live_head`] instead.
// trace:STORY-1397 | ai:codex
pub(crate) fn reconcile_recusal_hold(
    project_root: &Path,
    pr: u64,
    live_head: Option<&str>,
) -> Option<MergeHoldRecord> {
    reconcile_recusal_hold_with(project_root, pr, live_head, |proposal| {
        reconcile_recusal_route(project_root, proposal)
    })
}

// trace:STORY-1889 | ai:agy
pub(crate) fn reconcile_recusal_hold_with(
    project_root: &Path,
    pr: u64,
    live_head: Option<&str>,
    route_fn: impl FnOnce(&mut MergeHoldRecord) -> std::io::Result<()>,
) -> Option<MergeHoldRecord> {
    let durable = read_hold_record(project_root, pr)?;
    if durable.reason_kind != HoldReasonKind::Recusal || durable.legacy {
        return Some(durable);
    }
    let mut refreshed = project_live_head(&durable, live_head);
    if apply_recusal_route_proposal(&mut refreshed, route_fn).is_err() {
        return Some(durable);
    }
    if refreshed == durable {
        return Some(refreshed);
    }
    // trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
    if crate::gate::ledger::hold::place(project_root, &refreshed, false).is_err() {
        return Some(durable);
    }
    Some(refreshed)
}

/// The Rework hold a refusing verdict arms (BUG-1452 / STORY-1397): typed
/// Rework, referencing the verdict record (key, PR, sha, recording seat)
/// rather than quoting it, with its release condition (BUG-1532 / BUG-1562).
/// One builder for `aida review record` and the launcher that collects a
/// contained reviewer's refusal (M1 slice 2).
// trace:BUG-1532 trace:BUG-1562 trace:STORY-1697 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn refusal_hold(
    pr: u64,
    key: &str,
    kind_label: &str,
    reviewed_sha: Option<String>,
    recorded_by: Option<String>,
) -> MergeHoldRecord {
    let key = key.trim().to_ascii_uppercase();
    let reason = format!(
        "{kind_label} for {key} at {}",
        reviewed_sha
            .as_deref()
            .map(crate::gate::review_verdict::short_sha)
            .unwrap_or("unknown")
    );
    let mut hold = typed_hold(pr, HoldReasonKind::Rework, reason, reviewed_sha.clone());
    hold.verdict_ref = Some(VerdictRef::new(&key, Some(pr), reviewed_sha, recorded_by));
    hold.spec = Some(key.clone());
    hold.release_condition = Some(format!(
        "a fresh APPROVED verdict for {key} recorded at PR #{pr}'s current head by an independent reviewer releases it (a sensitive spec also needs a human clear or an --include-supervised mandate)"
    ));
    hold
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

/// Release `record`'s hold as `actor`: append a `hold-release` event whose
/// clearance names who released it. Nothing is deleted; the `hold` stays on
/// the ledger as evidence (AC3).
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn release_hold(
    project_root: &Path,
    record: &MergeHoldRecord,
    actor: &PrincipalIdentity,
    released_by_verdict: Option<VerdictRef>,
) -> std::io::Result<()> {
    release_hold_by(project_root, record, &actor.key(), released_by_verdict)
}

// trace:STORY-1687 | ai:codex
pub(crate) fn release_hold_by(
    project_root: &Path,
    record: &MergeHoldRecord,
    principal: &str,
    released_by_verdict: Option<VerdictRef>,
) -> std::io::Result<()> {
    let clearance = clearance_for(record, principal, released_by_verdict, None)?;
    with_hold_lock(project_root, record.pr, || {
        crate::gate::ledger::hold::release(project_root, record.pr, clearance)
    })?
}

/// STORY-1698: the release of a hold the advisor performed under a
/// fire-fighting mandate — `cleared_by` names the advisor seat's principal,
/// `released_under_mandate` the mandate.
// trace:STORY-1698 | ai:claude
pub(crate) fn release_hold_under_mandate(
    project_root: &Path,
    record: &MergeHoldRecord,
    advisor_principal: &str,
    mandate_id: &str,
    released_by_verdict: Option<VerdictRef>,
) -> std::io::Result<()> {
    let clearance = clearance_for(
        record,
        &format!("advisor:{}", advisor_principal.trim()),
        released_by_verdict,
        Some(mandate_id.to_string()),
    )?;
    with_hold_lock(project_root, record.pr, || {
        crate::gate::ledger::hold::release(project_root, record.pr, clearance)
    })?
}

/// The clearance audit record for releasing `record`.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn clearance_for(
    record: &MergeHoldRecord,
    principal: &str,
    released_by_verdict: Option<VerdictRef>,
    released_under_mandate: Option<String>,
) -> std::io::Result<serde_json::Map<String, serde_json::Value>> {
    let clearance = HoldClearance {
        pr: record.pr,
        reason_kind: record.reason_kind,
        detail: record.detail.clone(),
        cleared_by: principal.to_string(),
        target_head_sha: record.target_head_sha.clone(),
        cleared_at: chrono::Utc::now().to_rfc3339(),
        released_by_verdict,
        released_under_mandate,
    };
    match serde_json::to_value(&clearance)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
    {
        serde_json::Value::Object(map) => Ok(map),
        _ => Err(std::io::Error::other("a clearance is a JSON object")),
    }
}

/// BUG-2214 corroboration 1 of 3: `pr`'s recorded clearance, only when it
/// names a human or operator principal and no mandate — a release a human
/// decided, not one an advisor's mandate performed. Unreadable or unqualified
/// records answer `None` (fail closed). Read from the ledger's latest
/// release ([`recorded_clearance`]), so the evidence survives the release.
// trace:BUG-2214 | ai:claude
pub(crate) fn human_recorded_clearance(project_root: &Path, pr: u64) -> Option<HoldClearance> {
    let body = recorded_clearance(project_root, pr)?;
    let clearance: HoldClearance = serde_json::from_value(serde_json::Value::Object(body)).ok()?;
    // A clearance copied from another PR's file does not qualify.
    if clearance.pr != pr || clearance.released_under_mandate.is_some() {
        return None;
    }
    matches!(
        PrincipalIdentity::parse(&clearance.cleared_by).principal_kind,
        PrincipalKind::Human | PrincipalKind::Operator
    )
    .then_some(clearance)
}

/// BUG-2214 corroboration 2 of 3: does the events ledger say PR `pr`'s most
/// recent hold change is a release by a Human/Operator principal that carried
/// the human integrity-floor attestation? Any later placement (including a
/// mandate-failure re-arm), an unattested release, or an agent/advisor release
/// answers `false`, as does a ledger with no hold events for the PR.
// trace:BUG-2214 | ai:claude
pub(crate) fn human_attested_release(project_root: &Path, pr: u64) -> bool {
    use crate::events::EventKind;
    let mut released = false;
    for event in crate::events::read_all_with_archive(&hold_store_root(project_root)) {
        if let EventKind::MergeHoldChanged {
            pr: event_pr,
            placed,
            principal,
            human_attested,
            ..
        } = event.kind
        {
            if u64::from(event_pr) != pr {
                continue;
            }
            released = !placed
                && human_attested
                && principal.as_deref().is_some_and(|p| {
                    matches!(
                        PrincipalIdentity::parse(p).principal_kind,
                        PrincipalKind::Human | PrincipalKind::Operator
                    )
                });
        }
    }
    released
}

/// How long a hold write or release waits for another one on the same PR.
// trace:TASK-1951 trace:TASK-1757 | ai:claude
const HOLD_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(test)]
thread_local! {
    /// Tests shorten the wait so a contention test does not take 10 s.
    pub(crate) static HOLD_LOCK_WAIT_OVERRIDE: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
}

fn hold_lock_wait() -> std::time::Duration {
    #[cfg(test)]
    if let Some(wait) = HOLD_LOCK_WAIT_OVERRIDE.with(|w| w.get()) {
        return wait;
    }
    HOLD_LOCK_WAIT
}

/// The per-PR hold lock, beside the change's gate ledger file.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn hold_lock_path(project_root: &Path, pr: u64) -> PathBuf {
    crate::gate::ledger::ledger_dir(&crate::gate::ledger::hold::read_root(project_root))
        .join(format!("PR-{pr}.hold.lock"))
}

/// TASK-1757: run `f` holding PR `pr`'s hold lock, so a placement (or
/// re-arm) cannot interleave with a release's second validation pass and its
/// marker removal. Every hold write and every release takes it here. After
/// [`HOLD_LOCK_WAIT`] it refuses with a named reason; it never proceeds
/// unlocked. A caller that cannot create the lock (a contained seat) is
/// refused: there is no worktree-local fallback (M1 slice 2).
// trace:TASK-1951 trace:TASK-1757 | ai:claude
pub(crate) fn with_hold_lock<T>(
    project_root: &Path,
    pr: u64,
    f: impl FnOnce() -> T,
) -> std::io::Result<T> {
    use fs2::FileExt;
    let open = |path: &Path| -> std::io::Result<std::fs::File> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
    };
    let path = hold_lock_path(project_root, pr);
    let file = open(&path)?;
    // Re-entrant per thread: a hold write nested inside a release on the
    // same thread is not a race, and a second flock on a fresh descriptor
    // would wait on itself.
    thread_local! {
        static HELD: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    if HELD.with(|held| held.borrow().contains(&path)) {
        return Ok(f());
    }
    let deadline = std::time::Instant::now() + hold_lock_wait();
    while file.try_lock_exclusive().is_err() {
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!(
                    "merge-hold PR-{pr} is busy: another hold placement or release holds {} \
                     (waited {}s); nothing was changed — re-run once it finishes",
                    path.display(),
                    hold_lock_wait().as_secs()
                ),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // The guard drops the thread's entry and the flock even when `f`
    // panics, so a caught panic cannot leave later calls on this thread
    // taking the re-entrant shortcut unlocked.
    // trace:TASK-1951 trace:TASK-1757 | ai:claude
    struct Held<'a> {
        path: &'a Path,
        file: &'a std::fs::File,
    }
    impl Drop for Held<'_> {
        fn drop(&mut self) {
            HELD.with(|held| held.borrow_mut().retain(|p| p != self.path));
            let _ = FileExt::unlock(self.file);
        }
    }
    HELD.with(|held| held.borrow_mut().push(path.clone()));
    let _held = Held {
        path: &path,
        file: &file,
    };
    Ok(f())
}

/// `aida merge-hold clear`: clearing a supervised hold is a non-grantable
/// human integrity floor, independent of dispatch/disposition roles. The
/// refusal when `floor_authority` (a human at an interactive terminal) is
/// absent.
// trace:STORY-1353 | ai:codex
// trace:STORY-1889 | ai:claude
pub(crate) fn hold_clear_floor_refusal(floor_authority: bool) -> Option<&'static str> {
    (!floor_authority).then_some(
        "clearing a merge-hold requires a human at an interactive terminal; \
                     dispatch or advisor authority cannot override this integrity floor",
    )
}

/// BUG-1236: whether the `aida:merge-hold` label on the change mirrors the
/// marker, as RECORDED when the marker's label was last synced. TASK-189: this
/// is the marker's claim, not the forge's state — `aida merge-hold list` shows
/// it as `recorded:` beside a live forge read (`ForgeLabel`).
// trace:BUG-1236 | ai:claude
// trace:STORY-1889 | ai:agy
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LabelState {
    Synced,
    Unsynced(String),
    Unknown,
}

/// Record the label sync state on the open hold (no-op without one), as a
/// `label-sync` event on the PR's gate ledger.
// trace:BUG-1236 | ai:claude
// trace:STORY-1889 | ai:agy
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn record_label_state(
    project_root: &Path,
    pr: u64,
    state: &LabelState,
) -> std::io::Result<()> {
    if read_hold_record(project_root, pr).is_none() {
        return Ok(());
    }
    let label_state = match state {
        LabelState::Synced => "synced".to_string(),
        LabelState::Unsynced(err) => format!("unsynced: {}", err.lines().next().unwrap_or("")),
        LabelState::Unknown => "unknown".to_string(),
    };
    with_hold_lock(project_root, pr, || {
        crate::gate::ledger::hold::append(
            project_root,
            pr,
            crate::gate::ledger::EventBody::LabelSync { label_state },
            None,
            None,
        )
        .map(|_| ())
    })?
}

/// The recorded label state for `pr` (Unknown when the hold predates
/// BUG-1236 or carries no state).
// trace:BUG-1236 | ai:claude
// trace:STORY-1889 | ai:agy
pub(crate) fn read_label_state(project_root: &Path, pr: u64) -> LabelState {
    match read_hold_record(project_root, pr).and_then(|r| r.label_state) {
        Some(s) if s == "synced" => LabelState::Synced,
        Some(s) if s.starts_with("unsynced:") => {
            LabelState::Unsynced(s.trim_start_matches("unsynced:").trim().to_string())
        }
        _ => LabelState::Unknown,
    }
}

// trace:STORY-1889 | ai:claude
#[cfg(test)]
#[path = "tests/merge_hold_core_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/bug_2176_merge_hold_primary_root_tests.rs"]
mod bug_2176_merge_hold_primary_root_tests;
