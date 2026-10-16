//! STORY-1698: the fire-fighting mandate — a human-minted, time-boxed grant
//! that lets the advisor seat ship held PRs carrying an attested, independent
//! approval while the operator is away.
//!
//! Authority lives ONLY in the user's AIDA home (`~/.aida/mandates/<uuid>.json`,
//! the same trust class as session grants and human-review receipts), written
//! atomically after [`crate::seat_authority::require_direct_human`]. The git
//! store is never consulted for authority: any agent can write it.
//! `.aida/events.jsonl` carries the audit copy (`MandateChanged`).
//!
//! Expiry is never cached: every ship decision re-reads the records and
//! compares `expires_at` / `revoked_at` against the clock at that instant.
// trace:STORY-1698 | ai:claude

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const MANDATE_DIR: &str = "mandates";
/// The only scope a mandate can carry today.
pub(crate) const SCOPE_SHIP_HELD: &str = "ship-held";
/// The only seat a mandate covers.
pub(crate) const MANDATE_SEAT: &str = "advisor";
pub(crate) const DEFAULT_TTL: Duration = Duration::hours(8);
pub(crate) const MAX_TTL: Duration = Duration::hours(24);
/// The tag that keeps a spec human-only whatever the mandate says.
pub(crate) const OPERATOR_ONLY_TAG: &str = "operator-only";

/// One mandate record.
// trace:STORY-1698 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Mandate {
    pub id: String,
    /// The human who minted it at a terminal.
    pub principal: String,
    /// Always [`MANDATE_SEAT`].
    pub seat: String,
    /// Always [`SCOPE_SHIP_HELD`].
    pub scope: String,
    /// Lifts the sensitive spec classes (never operator-only).
    #[serde(default)]
    pub include_supervised: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_by: Option<String>,
}

impl Mandate {
    fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        self.revoked_at.is_none() && now < self.expires_at
    }
}

/// The mandate situation at one instant.
// trace:STORY-1698 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MandateState {
    /// No mandate was ever minted.
    None,
    Active(Mandate),
    /// The newest mandate ran out.
    Expired(Mandate),
    /// The newest mandate was stopped.
    Stopped(Mandate),
}

/// `~/.aida/mandates`, through the hermetic-aware home lookup.
// trace:STORY-1698 | ai:claude
pub(crate) fn mandate_dir() -> Result<PathBuf> {
    let home = crate::aida_home_dir().context("cannot locate the user's home directory")?;
    Ok(home.join(".aida").join(MANDATE_DIR))
}

/// Every well-formed record in `dir`. A file whose name is not its record's
/// uuid is ignored, so a copied record cannot impersonate another id.
// trace:STORY-1698 | ai:claude
pub(crate) fn load_all(dir: &Path) -> Vec<Mandate> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Mandate> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let stem = path.file_stem()?.to_str()?.to_string();
            if path.extension()?.to_str()? != "json" || Uuid::parse_str(&stem).is_err() {
                return None;
            }
            let mandate: Mandate = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
            (mandate.id == stem && mandate.seat == MANDATE_SEAT && mandate.scope == SCOPE_SHIP_HELD)
                .then_some(mandate)
        })
        .collect();
    out.sort_by_key(|m| m.issued_at);
    out
}

/// The state at `now`: the newest active mandate if any, else what became
/// of the newest one.
// trace:STORY-1698 | ai:claude
pub(crate) fn state_at(mandates: &[Mandate], now: DateTime<Utc>) -> MandateState {
    if let Some(active) = mandates
        .iter()
        .filter(|m| m.is_active_at(now))
        .max_by_key(|m| m.issued_at)
    {
        return MandateState::Active(active.clone());
    }
    match mandates.iter().max_by_key(|m| m.issued_at) {
        None => MandateState::None,
        Some(m) if m.revoked_at.is_some() => MandateState::Stopped(m.clone()),
        Some(m) => MandateState::Expired(m.clone()),
    }
}

/// The state read from disk now — never cached.
// trace:STORY-1698 | ai:claude
pub(crate) fn current_state() -> MandateState {
    match mandate_dir() {
        Ok(dir) => state_at(&load_all(&dir), Utc::now()),
        Err(_) => MandateState::None,
    }
}

fn write(dir: &Path, mandate: &Mandate) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let body = serde_json::to_vec_pretty(mandate)?;
    aida_core::fs_atomic::write_atomic(&dir.join(format!("{}.json", mandate.id)), &body)?;
    Ok(())
}

/// Parse `--ttl`: `<n>h` or `<n>m`, more than zero and at most [`MAX_TTL`].
// trace:STORY-1698 | ai:claude
pub(crate) fn parse_ttl(raw: &str) -> Result<Duration> {
    let raw = raw.trim().to_ascii_lowercase();
    let (digits, unit) = raw.split_at(raw.len().saturating_sub(1));
    let n: i64 = digits
        .parse()
        .with_context(|| format!("invalid --ttl `{raw}`: use hours or minutes, e.g. 8h or 90m"))?;
    let ttl = match unit {
        "h" => Duration::hours(n),
        "m" => Duration::minutes(n),
        _ => bail!("invalid --ttl `{raw}`: use hours or minutes, e.g. 8h or 90m"),
    };
    if ttl <= Duration::zero() {
        bail!("--ttl must be more than zero");
    }
    if ttl > MAX_TTL {
        bail!("--ttl may be at most 24h; a mandate cannot be extended, only minted again");
    }
    Ok(ttl)
}

/// Mint a mandate. `human` is the integrity floor
/// ([`crate::seat_authority::require_direct_human`] in production) and runs
/// before anything is read or written. Refuses while one is active: there is
/// no extension, only stop and mint again.
// trace:STORY-1698 | ai:claude
pub(crate) fn start(
    dir: &Path,
    human: &dyn Fn() -> Result<()>,
    principal: &str,
    ttl: Duration,
    reason: Option<String>,
    include_supervised: bool,
    now: DateTime<Utc>,
) -> Result<Mandate> {
    if let Err(err) = human() {
        bail!("a mandate can only be started by a human at an interactive terminal, never by an agent ({err})");
    }
    if ttl <= Duration::zero() || ttl > MAX_TTL {
        bail!("a mandate lasts more than zero and at most 24h");
    }
    let principal = principal.trim();
    if principal.is_empty() {
        bail!("cannot start a mandate: the human user identity is unknown");
    }
    if let MandateState::Active(active) = state_at(&load_all(dir), now) {
        bail!(
            "mandate {} is already active until {}; stop it with `aida mandate stop` before starting another (mandates cannot be extended)",
            active.id,
            active.expires_at.to_rfc3339()
        );
    }
    let mandate = Mandate {
        id: Uuid::new_v4().to_string(),
        principal: principal.to_string(),
        seat: MANDATE_SEAT.to_string(),
        scope: SCOPE_SHIP_HELD.to_string(),
        include_supervised,
        reason: reason
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty()),
        issued_at: now,
        expires_at: now + ttl,
        revoked_at: None,
        revoked_by: None,
    };
    write(dir, &mandate)?;
    Ok(mandate)
}

/// Stop every active mandate now. Same floor as [`start`]. Returns the
/// mandates it stopped (empty when none was active).
// trace:STORY-1698 | ai:claude
pub(crate) fn stop(
    dir: &Path,
    human: &dyn Fn() -> Result<()>,
    by: &str,
    now: DateTime<Utc>,
) -> Result<Vec<Mandate>> {
    if let Err(err) = human() {
        bail!("a mandate can only be stopped by a human at an interactive terminal, never by an agent ({err})");
    }
    let mut stopped = Vec::new();
    for mut m in load_all(dir).into_iter().filter(|m| m.is_active_at(now)) {
        m.revoked_at = Some(now);
        m.revoked_by = Some(by.trim().to_string());
        write(dir, &m)?;
        stopped.push(m);
    }
    Ok(stopped)
}

// ── Ship decision ───────────────────────────────────────────────────────────

/// What the ship decision knows about one spec the PR ships.
// trace:STORY-1698 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct ShipSpec {
    pub id: String,
    /// The spec as the store holds it now; `None` when the store has no such
    /// spec (fails closed).
    pub current: Option<aida_core::Requirement>,
    /// Every earlier version of the spec found in the store's git history
    /// (plus what its recorded field changes say it once was). `Err` = the
    /// history could not be read, which refuses (fails closed): exclusion is
    /// sticky, so a version we cannot see may be the one that excludes it.
    pub history: std::result::Result<Vec<aida_core::Requirement>, String>,
}

/// Every version of a requirement its own `history:` field changes describe:
/// for each recorded change to `tags`, `execution_mode`, `title` or `type`,
/// one pseudo-version carrying the OLD value and one carrying the NEW value
/// (on top of `req`), so a tag removed by a recorded edit is still seen.
// trace:STORY-1698 | ai:claude
pub(crate) fn field_change_versions(req: &aida_core::Requirement) -> Vec<aida_core::Requirement> {
    let mut out = Vec::new();
    for change in req.history.iter().flat_map(|h| h.changes.iter()) {
        for value in [&change.old_value, &change.new_value] {
            let mut v = req.clone();
            match change.field_name.trim().to_ascii_lowercase().as_str() {
                "tags" | "tag" => {
                    v.tags = value
                        .split([',', ' '])
                        .map(str::trim)
                        .filter(|t| !t.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                "execution_mode" | "mode" => match value.parse::<aida_core::ExecutionMode>() {
                    Ok(mode) => v.execution_mode = Some(mode),
                    Err(_) => continue,
                },
                "title" => v.title = value.clone(),
                "type" | "req_type" => {
                    match serde_yaml::from_str::<aida_core::RequirementType>(value) {
                        Ok(t) => v.req_type = t,
                        Err(_) => continue,
                    }
                }
                _ => continue,
            }
            out.push(v);
        }
    }
    out
}

/// One historical object body, read leniently: the full requirement when it
/// parses, else just the fields exclusion looks at (an older schema). `Err`
/// only when it is not YAML at all.
// trace:STORY-1698 | ai:claude
fn parse_version(text: &str) -> std::result::Result<aida_core::Requirement, String> {
    if let Ok(req) = serde_yaml::from_str::<aida_core::Requirement>(text) {
        return Ok(req);
    }
    let value: serde_yaml::Value =
        serde_yaml::from_str(text).map_err(|e| format!("unparseable version: {e}"))?;
    let field = |k: &str| {
        value
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let mut req = aida_core::Requirement::new(field("title"), String::new());
    if let Some(tags) = value.get("tags").and_then(|v| v.as_sequence()) {
        req.tags = tags
            .iter()
            .filter_map(|t| t.as_str().map(str::to_string))
            .collect();
    }
    req.execution_mode = field("execution_mode").parse().ok();
    if let Some(t) = value
        .get("type")
        .and_then(|t| serde_yaml::from_value(t.clone()).ok())
    {
        req.req_type = t;
    }
    Ok(req)
}

/// STORY-1698: every version of `spec_id` in the store's git history (all
/// refs, so a rewritten local branch still shows what the remote saw), each
/// expanded by its recorded field changes. `Err` when the history cannot be
/// read or holds no version at all — the caller refuses.
// trace:STORY-1698 | ai:claude
pub(crate) fn store_history_versions(
    store_root: &Path,
    spec_id: &str,
) -> std::result::Result<Vec<aida_core::Requirement>, String> {
    let rel = aida_core::object_store::relative_object_path(spec_id)
        .map_err(|e| format!("no object path for {spec_id}: {e}"))?;
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(store_root)
            .args(args)
            .output()
            .map_err(|e| format!("git failed to run: {e}"))
            .and_then(|out| {
                if out.status.success() {
                    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
                } else {
                    Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
                }
            })
    };
    // Deletions carry no body to read; every other touching commit does.
    let log = git(&[
        "log",
        "--all",
        "--format=%H",
        "--diff-filter=ACMRT",
        "--",
        &rel,
    ])?;
    let shas: Vec<&str> = log
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if shas.is_empty() {
        return Err(format!("no version of {spec_id} in the store's history"));
    }
    let mut out = Vec::new();
    for sha in shas {
        let text = git(&["show", &format!("{sha}:{rel}")])
            .map_err(|e| format!("cannot read {spec_id} at {sha}: {e}"))?;
        let req = parse_version(&text).map_err(|e| format!("{spec_id} at {sha}: {e}"))?;
        out.extend(field_change_versions(&req));
        out.push(req);
    }
    Ok(out)
}

/// Everything the mandate ship decision weighs, gathered by the caller.
// trace:STORY-1698 | ai:claude
pub(crate) struct ShipCheck<'a> {
    pub state: &'a MandateState,
    pub now: DateTime<Utc>,
    /// `(seat, principal)` of the caller's VALIDATED seat grant.
    pub grant: Option<(&'a str, &'a str)>,
    pub specs: &'a [ShipSpec],
    /// Did the forge confirm what the PR ships?
    pub specs_resolved: bool,
    pub candidates: &'a [crate::review_verdict::RecordedVerdict],
    pub policy: &'a crate::pr_ship::ShipApprovalPolicy,
    pub head: Option<&'a str>,
    pub override_stale_approval: bool,
}

/// Why a mandate does not cover this ship.
// trace:STORY-1698 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MandateRefusal {
    NoMandate,
    Expired { id: String },
    Stopped { id: String },
    NotAdvisor { seat: Option<String> },
    PrincipalMismatch { mandate: String, grant: String },
    OverrideStale,
    SpecsUnresolved,
    OperatorOnly { spec: String },
    HistoryUnreadable { spec: String, why: String },
    Excluded { spec: String, why: String },
    ShipperUnanchored,
    AdvisorAuthored { author: String },
    ApproverIsShipper,
    Approval(String),
    StaleApproval(String),
}

impl MandateRefusal {
    pub(crate) fn message(&self, pr: u64) -> String {
        let why = match self {
            Self::NoMandate => "no mandate is active".to_string(),
            Self::Expired { id } => format!("mandate {id} has expired"),
            Self::Stopped { id } => format!("mandate {id} was stopped"),
            Self::NotAdvisor { seat } => format!(
                "a mandate covers only the advisor seat (this session holds {})",
                seat.as_deref().unwrap_or("no validated seat grant")
            ),
            Self::PrincipalMismatch { mandate, grant } => format!(
                "the mandate was minted by `{mandate}`, but this advisor seat belongs to `{grant}`"
            ),
            Self::OverrideStale => {
                "`--override-stale-approval` is never honoured under a mandate".to_string()
            }
            Self::SpecsUnresolved => {
                "what the PR ships could not be resolved to specs in the store".to_string()
            }
            Self::HistoryUnreadable { spec, why } => format!(
                "{spec}'s store history could not be read ({why}), so it is treated as excluded"
            ),
            Self::OperatorOnly { spec } => {
                format!("{spec} is operator-only; it always needs a human merge")
            }
            Self::Excluded { spec, why } => format!(
                "{spec} is in an excluded class ({why}); the mandate was not minted with --include-supervised"
            ),
            Self::ShipperUnanchored => {
                "this session has no identity AIDA can check against the PR's authors".to_string()
            }
            Self::AdvisorAuthored { author } => {
                format!("this session authored the PR ({author}); it cannot ship it under a mandate")
            }
            Self::ApproverIsShipper => {
                "the approval was recorded by this shipping session; it must come from an independent reviewer or a human".to_string()
            }
            Self::Approval(m) | Self::StaleApproval(m) => m.clone(),
        };
        format!("PR-{pr}: the fire-fighting mandate does not cover this ship: {why}")
    }
}

/// STORY-1698: the sensitive spec classes a mandate excludes unless minted
/// with `--include-supervised`: STORY-1687's classes plus containment,
/// review-authority and merge-authority.
// trace:STORY-1698 | ai:claude
pub(crate) fn excluded_class(req: &aida_core::Requirement) -> Option<String> {
    if let Some(why) = crate::rework_hold_release::sensitive(req) {
        return Some(why);
    }
    let terms = ["containment", "review-authority", "merge-authority"];
    std::iter::once(&req.title)
        .chain(req.tags.iter())
        .find_map(|text| {
            let text = text.to_ascii_lowercase();
            terms
                .iter()
                .find(|term| text.contains(**term))
                .map(|term| format!("sensitive spec metadata ({term})"))
        })
}

/// STORY-1698: operator-only specs — `execution_mode = operator` or the
/// `operator-only` tag. Never covered, whatever the mandate's flags.
// trace:STORY-1698 | ai:claude
pub(crate) fn is_operator_only(req: &aida_core::Requirement) -> bool {
    req.execution_mode == Some(aida_core::ExecutionMode::Operator)
        || req
            .tags
            .iter()
            .any(|t| t.trim().eq_ignore_ascii_case(OPERATOR_ONLY_TAG))
}

/// STORY-1698: does an active mandate cover this ship? `Ok(mandate id)` =
/// the advisor may release the hold and merge; `Err` = it may not. Pure:
/// every input is gathered by the caller, at the instant of the decision.
// trace:STORY-1698 | ai:claude
pub(crate) fn ship_decision(c: &ShipCheck<'_>) -> std::result::Result<String, MandateRefusal> {
    let mandate = match c.state {
        MandateState::None => return Err(MandateRefusal::NoMandate),
        MandateState::Expired(m) => return Err(MandateRefusal::Expired { id: m.id.clone() }),
        MandateState::Stopped(m) => return Err(MandateRefusal::Stopped { id: m.id.clone() }),
        MandateState::Active(m) => m,
    };
    if mandate.revoked_at.is_some() {
        return Err(MandateRefusal::Stopped {
            id: mandate.id.clone(),
        });
    }
    if c.now >= mandate.expires_at {
        return Err(MandateRefusal::Expired {
            id: mandate.id.clone(),
        });
    }
    match c.grant {
        Some((seat, principal)) if seat == MANDATE_SEAT => {
            if principal.trim() != mandate.principal {
                return Err(MandateRefusal::PrincipalMismatch {
                    mandate: mandate.principal.clone(),
                    grant: principal.to_string(),
                });
            }
        }
        other => {
            return Err(MandateRefusal::NotAdvisor {
                seat: other.map(|(s, _)| s.to_string()),
            })
        }
    }
    if c.override_stale_approval {
        return Err(MandateRefusal::OverrideStale);
    }
    if !c.specs_resolved || c.specs.is_empty() {
        return Err(MandateRefusal::SpecsUnresolved);
    }
    // Exclusion is STICKY: a spec is judged by every version it ever had —
    // now, in the store's git history, and in its recorded field changes —
    // so stripping a tag never lifts it. Unreadable history refuses.
    // trace:STORY-1698 | ai:claude
    for spec in c.specs {
        let id = &spec.id;
        let Some(current) = &spec.current else {
            return Err(MandateRefusal::SpecsUnresolved);
        };
        let history = match &spec.history {
            Ok(history) => history,
            Err(why) => {
                return Err(MandateRefusal::HistoryUnreadable {
                    spec: id.clone(),
                    why: why.clone(),
                })
            }
        };
        let current_changes = field_change_versions(current);
        let versions = std::iter::once(current)
            .chain(current_changes.iter())
            .chain(history.iter());
        let mut excluded: Option<String> = None;
        for (i, version) in versions.enumerate() {
            if is_operator_only(version) {
                return Err(MandateRefusal::OperatorOnly { spec: id.clone() });
            }
            if excluded.is_none() {
                excluded = excluded_class(version).map(|why| {
                    if i == 0 {
                        why
                    } else {
                        format!("{why}, in an earlier version")
                    }
                });
            }
        }
        if let (Some(why), false) = (excluded, mandate.include_supervised) {
            return Err(MandateRefusal::Excluded {
                spec: id.clone(),
                why,
            });
        }
    }
    let shipper = &c.policy.shipper;
    if !crate::review_authority::has_identity_anchor(shipper) {
        return Err(MandateRefusal::ShipperUnanchored);
    }
    if let Some(author) = c.policy.author_sessions.iter().find(|a| a.matches(shipper)) {
        return Err(MandateRefusal::AdvisorAuthored {
            author: author.label.clone(),
        });
    }
    if let Some(refusal) = crate::pr_ship::approval_authority_refusal(c.candidates, c.policy) {
        return Err(MandateRefusal::Approval(
            crate::pr_ship::approval_authority_refusal_message(0, &refusal),
        ));
    }
    // The approval authority gate passed, so a newest approval exists.
    let approvals: Vec<_> = c
        .candidates
        .iter()
        .filter(|v| v.kind.approves() && !v.is_closed())
        .cloned()
        .collect();
    if let Some(newest) = crate::pr_ship::newest_approval(&approvals) {
        let recorder = newest
            .attestation
            .as_ref()
            .map(|a| a.identity.clone())
            .unwrap_or_default();
        let verified = newest
            .attestation
            .as_ref()
            .and_then(|a| a.authority_key(newest.recorded_at.as_deref()))
            .and_then(|k| c.policy.verified_authority.get(&k).cloned())
            .unwrap_or_default();
        if crate::review_authority::identities_overlap(shipper, &recorder)
            || crate::review_authority::identities_overlap(shipper, &verified)
        {
            return Err(MandateRefusal::ApproverIsShipper);
        }
    }
    if let Some(refusal) = crate::pr_ship::approval_head_refusal(c.candidates, c.head) {
        return Err(MandateRefusal::StaleApproval(
            crate::pr_ship::approval_head_refusal_message(0, &refusal),
        ));
    }
    Ok(mandate.id.clone())
}

// ── Ledger ──────────────────────────────────────────────────────────────────

/// The words every mandate-authorized release carries, in the hold ledger and
/// on the PR.
// trace:STORY-1698 | ai:claude
pub(crate) fn release_note(mandate_id: &str) -> String {
    format!("released under mandate {mandate_id} by advisor")
}

/// The audit copy of a start / stop in `.aida/events.jsonl`. Best-effort.
// trace:STORY-1698 | ai:claude
pub(crate) fn emit_changed(project_root: &Path, mandate: &Mandate, active: bool) {
    let mut event = crate::events::Event::new(
        None,
        "",
        crate::events::EventKind::MandateChanged {
            id: mandate.id.clone(),
            active,
            principal: mandate.principal.clone(),
            expires_at: mandate.expires_at.to_rfc3339(),
            include_supervised: mandate.include_supervised,
            reason: mandate.reason.clone(),
        },
    );
    event.seat = crate::events::active_seat();
    crate::events::emit(project_root, &event);
}

// ── Display ─────────────────────────────────────────────────────────────────

/// The statusline segment: `MANDATE 3h12m` while one is active.
// trace:STORY-1698 | ai:claude
pub(crate) fn statusline_segment(state: &MandateState, now: DateTime<Utc>) -> Option<String> {
    match state {
        MandateState::Active(m) if m.is_active_at(now) => Some(format!(
            "MANDATE {}",
            crate::seat_authority::format_remaining(m.expires_at - now)
        )),
        _ => None,
    }
}

fn status_json(state: &MandateState, now: DateTime<Utc>) -> serde_json::Value {
    let (status, mandate) = match state {
        MandateState::None => ("none", None),
        MandateState::Active(m) => ("active", Some(m)),
        MandateState::Expired(m) => ("expired", Some(m)),
        MandateState::Stopped(m) => ("stopped", Some(m)),
    };
    let mut out = serde_json::json!({ "status": status });
    if let Some(m) = mandate {
        out["mandate"] = serde_json::to_value(m).unwrap_or_default();
        if status == "active" {
            out["remaining_secs"] = serde_json::json!((m.expires_at - now).num_seconds());
        }
    }
    out
}

fn status_text(state: &MandateState, now: DateTime<Utc>) -> String {
    let describe = |m: &Mandate| {
        let mut lines = vec![
            format!("  id:        {}", m.id),
            format!(
                "  minted by: {} at {}",
                m.principal,
                m.issued_at.to_rfc3339()
            ),
            format!("  covers:    the advisor seat shipping held PRs"),
            format!(
                "  includes sensitive specs: {}",
                if m.include_supervised {
                    "yes (--include-supervised)"
                } else {
                    "no"
                }
            ),
        ];
        if let Some(reason) = &m.reason {
            lines.push(format!("  reason:    {reason}"));
        }
        lines.join("\n")
    };
    match state {
        MandateState::None => "No mandate has been started.".to_string(),
        MandateState::Active(m) => format!(
            "Mandate ACTIVE until {} ({} left)\n{}",
            m.expires_at.to_rfc3339(),
            crate::seat_authority::format_remaining(m.expires_at - now),
            describe(m)
        ),
        MandateState::Expired(m) => format!(
            "No active mandate. The last one expired at {}.\n{}",
            m.expires_at.to_rfc3339(),
            describe(m)
        ),
        MandateState::Stopped(m) => format!(
            "No active mandate. The last one was stopped at {} by {}.\n{}",
            m.revoked_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
            m.revoked_by.as_deref().unwrap_or("?"),
            describe(m)
        ),
    }
}

/// `aida mandate start|stop|status`.
// trace:STORY-1698 | ai:claude
pub(crate) fn handle_mandate(action: &crate::cli::MandateAction) -> Result<()> {
    let dir = mandate_dir()?;
    let audit_root = crate::find_main_worktree_root().ok();
    let human = || crate::seat_authority::require_direct_human();
    match action {
        crate::cli::MandateAction::Start {
            ship_held,
            ttl,
            reason,
            include_supervised,
        } => {
            if !ship_held {
                bail!("say what the mandate covers: `aida mandate start --ship-held` (the only scope today)");
            }
            let ttl = match ttl {
                Some(raw) => parse_ttl(raw)?,
                None => DEFAULT_TTL,
            };
            let m = start(
                &dir,
                &human,
                &crate::current_user_id(None),
                ttl,
                reason.clone(),
                *include_supervised,
                Utc::now(),
            )?;
            if let Some(root) = &audit_root {
                emit_changed(root, &m, true);
            }
            println!(
                "Mandate {} started: the advisor seat may ship held PRs with an attested, independent approval and green CI until {}.",
                m.id,
                m.expires_at.to_rfc3339()
            );
            if !m.include_supervised {
                println!("Sensitive specs (security, containment, authority, guided, keystone) stay human-only.");
            }
            println!("Operator-only specs always stay human-only. Stop it any time: `aida mandate stop`.");
            Ok(())
        }
        crate::cli::MandateAction::Stop => {
            let stopped = stop(&dir, &human, &crate::current_user_id(None), Utc::now())?;
            if stopped.is_empty() {
                println!("No mandate was active.");
            }
            for m in &stopped {
                if let Some(root) = &audit_root {
                    emit_changed(root, m, false);
                }
                println!(
                    "Mandate {} stopped; the advisor can no longer ship held PRs.",
                    m.id
                );
            }
            Ok(())
        }
        crate::cli::MandateAction::Status { json } => {
            let now = Utc::now();
            let state = state_at(&load_all(&dir), now);
            if *json {
                println!("{}", status_json(&state, now));
            } else {
                println!("{}", status_text(&state, now));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "tests/story_1698_mandate_tests.rs"]
mod story_1698_mandate_tests;
