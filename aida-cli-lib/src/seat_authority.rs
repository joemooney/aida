//! Session-bound role grants. `AIDA_SESSION_ROLE` is retained for display and
//! routing only; authorization comes from a validated local grant record.
// trace:STORY-1473 | ai:codex

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub(crate) const GRANT_ENV: &str = "AIDA_SESSION_GRANT";
const GRANT_DIR: &str = "session-grants";
const MAX_GRANT_AGE: Duration = Duration::hours(24);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SeatGrant {
    pub id: String,
    /// Person whose roster ceiling authorized this grant.
    pub principal: String,
    /// Queue/process identity bound to this particular session.
    pub subject: String,
    pub session_id: String,
    pub seat: String,
    pub tty_issued_at: DateTime<Utc>,
    pub delegable_seats: Vec<String>,
    pub parent_grant_id: Option<String>,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

fn grant_dir() -> Result<PathBuf> {
    // BUG-1642 discipline: never resolve the home directly — `crate::aida_home_dir`
    // honours `$AIDA_HOME` / `$HOME` overrides and is fenced to the hermetic
    // temp home under `cfg(test)`. trace:STORY-1473 | ai:claude
    let home = crate::aida_home_dir().context("cannot locate the user's home directory")?;
    Ok(home.join(".aida").join(GRANT_DIR))
}

fn grant_path(id: &str) -> Result<PathBuf> {
    if Uuid::parse_str(id).is_err() {
        bail!("invalid session grant handle");
    }
    Ok(grant_dir()?.join(format!("{id}.json")))
}

fn read_grant(id: &str) -> Result<SeatGrant> {
    let path = grant_path(id)?;
    let bytes = std::fs::read(&path).context("session grant is missing or unreadable")?;
    let grant: SeatGrant = serde_json::from_slice(&bytes).context("session grant is malformed")?;
    if grant.id != id {
        bail!("session grant handle does not match its record");
    }
    Ok(grant)
}

fn write_grant(grant: &SeatGrant) -> Result<()> {
    let dir = grant_dir()?;
    std::fs::create_dir_all(&dir)?;
    // Derive the final path from the SAME resolved dir as the staging file:
    // `grant_path` re-resolves the home from the environment, and under the
    // test harness a sibling test may legitimately swap `$AIDA_HOME`/`$HOME`
    // between the two reads — the rename target would then sit in a directory
    // this call never created. trace:STORY-1473 | ai:claude
    if Uuid::parse_str(&grant.id).is_err() {
        bail!("invalid session grant handle");
    }
    let path = dir.join(format!("{}.json", grant.id));
    let bytes = serde_json::to_vec_pretty(grant)?;
    let tmp = dir.join(format!(".{}.tmp", grant.id));
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)?
            .write_all(&bytes)?;
    }
    #[cfg(not(unix))]
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

fn store_root(project_root: &Path) -> Option<PathBuf> {
    let store = crate::detect_distributed_store_from(project_root)?;
    store.join("objects").is_dir().then_some(store)
}

pub(crate) fn roster_allows(project_root: &Path, principal: &str, seat: &str) -> bool {
    store_root(project_root)
        .and_then(|store| aida_core::team::TeamRoster::load(&store).seats_for(principal))
        .map(|seats| {
            seats
                .iter()
                .any(|allowed| aida_core::team::canonical_role(allowed) == seat)
        })
        .unwrap_or(false)
}

/// Why an exported grant failed validation. The checks and their order are
/// exactly those [`current_grant`] has always applied; this only names which
/// one failed so a refusal can say "expired" instead of "no active role".
// trace:BUG-1983 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GrantRejection {
    /// The exported handle is not a grant id.
    BadHandle,
    /// No readable record for the handle.
    Unreadable,
    /// The record does not parse, or names a different id.
    Malformed,
    Revoked {
        at: DateTime<Utc>,
    },
    Expired {
        at: DateTime<Utc>,
    },
    /// The grant was minted for another identity than this process runs as.
    SubjectMismatch {
        grant_subject: String,
        caller: String,
    },
    /// The team roster no longer allows the principal this seat.
    RosterDenied {
        principal: String,
        seat: String,
    },
    /// The parent grant failed one of the checks above.
    Parent(Box<GrantRejection>),
    /// The parent belongs to another principal than the child.
    ParentPrincipalMismatch,
    /// The parent grant does not delegate the child's seat.
    ParentNotDelegating {
        parent_seat: String,
    },
}

impl std::fmt::Display for GrantRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stamp = |at: &DateTime<Utc>| at.format("%Y-%m-%dT%H:%MZ").to_string();
        match self {
            Self::BadHandle => write!(f, "the exported handle is not a grant id"),
            Self::Unreadable => write!(f, "no grant record exists for it on this machine"),
            Self::Malformed => write!(f, "its record is malformed"),
            Self::Revoked { at } => write!(f, "it was revoked at {}", stamp(at)),
            Self::Expired { at } => write!(f, "it expired at {}", stamp(at)),
            Self::SubjectMismatch {
                grant_subject,
                caller,
            } => write!(
                f,
                "it was issued to `{grant_subject}` but this process runs as `{caller}`"
            ),
            Self::RosterDenied { principal, seat } => write!(
                f,
                "the team roster no longer allows `{principal}` the `{seat}` seat"
            ),
            Self::Parent(inner) => write!(f, "the grant it was delegated from is invalid: {inner}"),
            Self::ParentPrincipalMismatch => {
                write!(
                    f,
                    "the grant it was delegated from belongs to another principal"
                )
            }
            Self::ParentNotDelegating { parent_seat } => write!(
                f,
                "the `{parent_seat}` grant it was delegated from no longer delegates this seat"
            ),
        }
    }
}

/// The verdict on the grant this process exports through `AIDA_SESSION_GRANT`.
// trace:BUG-1983 | ai:claude
#[derive(Debug, Clone)]
pub(crate) enum GrantCheck {
    /// No grant is exported.
    Absent,
    Valid(SeatGrant),
    /// A grant is exported but fails validation. `seat` is known when the
    /// record could be read.
    Rejected {
        id: String,
        seat: Option<String>,
        reason: GrantRejection,
    },
}

impl GrantCheck {
    pub(crate) fn into_valid(self) -> Option<SeatGrant> {
        match self {
            Self::Valid(grant) => Some(grant),
            _ => None,
        }
    }

    /// One sentence for a refusal, naming the grant by an 8-character prefix
    /// only (never the full handle) and the failed check. `None` unless the
    /// grant was rejected.
    // trace:BUG-1983 | ai:claude
    pub(crate) fn rejection_sentence(&self) -> Option<String> {
        let Self::Rejected { id, seat, reason } = self else {
            return None;
        };
        let prefix: String = id.chars().take(8).collect();
        let seat_label = seat.as_deref().unwrap_or("unknown seat");
        let recovery = match seat.as_deref() {
            Some(seat) => format!("Re-enter it at a terminal: `aida role enter {seat}`"),
            None => "Re-enter your role at a terminal: `aida role enter <role>`".to_string(),
        };
        Some(format!(
            "Session grant {prefix}… ({seat_label}) was rejected: {reason}. {recovery}"
        ))
    }
}

fn check_lifetime(grant: &SeatGrant, now: DateTime<Utc>) -> Result<(), GrantRejection> {
    if let Some(at) = grant.revoked_at {
        return Err(GrantRejection::Revoked { at });
    }
    if now >= grant.expires_at {
        return Err(GrantRejection::Expired {
            at: grant.expires_at,
        });
    }
    Ok(())
}

fn check_roster(project_root: &Path, grant: &SeatGrant) -> Result<(), GrantRejection> {
    if roster_allows(project_root, &grant.principal, &grant.seat) {
        Ok(())
    } else {
        Err(GrantRejection::RosterDenied {
            principal: grant.principal.clone(),
            seat: grant.seat.clone(),
        })
    }
}

fn read_grant_checked(id: &str) -> Result<SeatGrant, GrantRejection> {
    if Uuid::parse_str(id).is_err() {
        return Err(GrantRejection::BadHandle);
    }
    let path = grant_path(id).map_err(|_| GrantRejection::BadHandle)?;
    let bytes = std::fs::read(&path).map_err(|_| GrantRejection::Unreadable)?;
    let grant: SeatGrant = serde_json::from_slice(&bytes).map_err(|_| GrantRejection::Malformed)?;
    if grant.id != id {
        return Err(GrantRejection::Malformed);
    }
    Ok(grant)
}

// trace:BUG-1983 | ai:claude
pub(crate) fn grant_check(project_root: &Path) -> GrantCheck {
    let Some(id) = std::env::var(GRANT_ENV)
        .ok()
        .filter(|id| !id.trim().is_empty())
    else {
        return GrantCheck::Absent;
    };
    grant_check_for(project_root, &id, &crate::current_user_id(None))
}

/// Revalidate a recorded reviewer through the same grant verdict as record time.
// trace:STORY-1687 | ai:codex
pub(crate) fn grant_check_for(project_root: &Path, id: &str, caller: &str) -> GrantCheck {
    let rejected = |seat: Option<String>, reason| GrantCheck::Rejected {
        id: id.to_string(),
        seat,
        reason,
    };
    let grant = match read_grant_checked(id) {
        Ok(grant) => grant,
        Err(reason) => return rejected(None, reason),
    };
    // Checks run in the order `current_grant` always used: revoked, expired,
    // subject, roster; then the parent's revoked, expired, principal,
    // roster, delegable.
    let now = Utc::now();
    let own = check_lifetime(&grant, now).and_then(|()| {
        if grant.subject == caller {
            check_roster(project_root, &grant)
        } else {
            Err(GrantRejection::SubjectMismatch {
                grant_subject: grant.subject.clone(),
                caller: caller.to_string(),
            })
        }
    });
    if let Err(reason) = own {
        return rejected(Some(grant.seat), reason);
    }
    if let Some(parent_id) = &grant.parent_grant_id {
        let parent = read_grant_checked(parent_id)
            .and_then(|parent| check_lifetime(&parent, now).map(|()| parent))
            .map_err(|reason| GrantRejection::Parent(Box::new(reason)))
            .and_then(|parent| {
                if parent.principal == grant.principal {
                    Ok(parent)
                } else {
                    Err(GrantRejection::ParentPrincipalMismatch)
                }
            })
            .and_then(|parent| {
                check_roster(project_root, &parent)
                    .map(|()| parent)
                    .map_err(|reason| GrantRejection::Parent(Box::new(reason)))
            });
        let parent = match parent {
            Ok(parent) => parent,
            Err(reason) => return rejected(Some(grant.seat), reason),
        };
        if !parent
            .delegable_seats
            .iter()
            .any(|seat| seat == &grant.seat)
        {
            return rejected(
                Some(grant.seat),
                GrantRejection::ParentNotDelegating {
                    parent_seat: parent.seat,
                },
            );
        }
    }
    GrantCheck::Valid(grant)
}

pub(crate) fn current_grant(project_root: &Path) -> Option<SeatGrant> {
    grant_check(project_root).into_valid()
}

pub(crate) fn current_seat(project_root: &Path) -> Option<String> {
    current_grant(project_root).map(|grant| grant.seat)
}

/// What a display surface (the statusline) should show as the session's
/// role. The `AIDA_SESSION_ROLE` label is only a display/routing hint, so a
/// label naming a seat the session holds no valid grant for must read as
/// ungranted rather than as authority the session doesn't have.
// trace:BUG-1924 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RoleDisplay {
    /// The session holds this seat: a validated grant, or the `implementer`
    /// baseline, which needs no grant.
    Held(String),
    /// The label names this seat, but there is no valid grant for it
    /// (missing, expired, revoked, or minted in another shell).
    Ungranted(String),
    /// Neither a label nor a grant: the implicit `implementer` baseline.
    Default,
}

/// Pure classifier behind [`role_display`]: `grant_seat` is the seat of the
/// validated grant, if any.
// trace:BUG-1924 | ai:claude
pub(crate) fn classify_role_display(label: Option<&str>, grant_seat: Option<&str>) -> RoleDisplay {
    if let Some(seat) = grant_seat.map(str::trim).filter(|s| !s.is_empty()) {
        return RoleDisplay::Held(crate::canonical_role_name(seat));
    }
    match label.map(str::trim).filter(|s| !s.is_empty()) {
        None => RoleDisplay::Default,
        Some(label) => {
            let role = crate::canonical_role_name(label);
            // The implementer baseline and the lower-privilege stakeholder
            // personas confer nothing a grant would gate, so the label is
            // already the truth for them.
            if matches!(role.as_str(), "implementer" | "guest" | "requester") {
                RoleDisplay::Held(role)
            } else {
                RoleDisplay::Ungranted(role)
            }
        }
    }
}

/// Resolve the displayed role from the validated grant (local file reads
/// only — cheap enough for the per-prompt statusline), falling back to the
/// label marked ungranted.
// trace:BUG-1924 | ai:claude
pub(crate) fn role_display(project_root: &Path, label: Option<&str>) -> RoleDisplay {
    let seat = current_seat(project_root);
    classify_role_display(label, seat.as_deref())
}

/// A seat grant inside this window is warned about (SessionStart hook,
/// statusline), so expiry never surfaces first as a mid-run refusal.
// trace:TASK-1736 | ai:claude
pub(crate) const EXPIRY_WARN_WITHIN: Duration = Duration::hours(2);

/// When this session's seat grant stops conferring its seat.
// trace:TASK-1736 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GrantExpiry {
    pub seat: String,
    /// The earlier of the grant's and its parent's expiry: a child grant
    /// dies with its parent.
    pub expires_at: DateTime<Utc>,
}

/// What `AIDA_SESSION_GRANT` resolves to for expiry purposes.
// trace:TASK-1736 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GrantLookup {
    /// No grant handle in the environment.
    NoGrant,
    /// A grant for a seat the baseline confers anyway: its expiry costs nothing.
    Baseline,
    /// The handle is set but names no usable record (missing, malformed,
    /// expired, revoked, subject/roster rejected, or its parent is invalid).
    /// A launcher must not treat
    /// this as "nothing to worry about".
    Unresolved(String),
    /// A live record and when it stops conferring its seat.
    Expiry(GrantExpiry),
}

/// Classify the exported grant through the same verdict as authority gates.
/// Informational expired-record display is kept in `session_grant_expiry`.
// trace:TASK-1736 | ai:claude
pub(crate) fn session_grant_lookup() -> GrantLookup {
    // Use the same root resolver and verdict as the authority gates. A missing
    // project fails closed through the roster check, while Absent stays silent.
    // trace:BUG-2011 | ai:codex
    let root = crate::find_project_root().unwrap_or_else(|_| PathBuf::from("."));
    match grant_check(&root) {
        GrantCheck::Absent => GrantLookup::NoGrant,
        GrantCheck::Rejected { reason, .. } => {
            // Keep the existing informational revocation wording; the decision
            // and all other reasons come from the shared typed verdict.
            let reason = match reason {
                GrantRejection::Revoked { .. } => "session grant was revoked".to_string(),
                other @ GrantRejection::Unreadable => {
                    format!("{other} (grant record missing)")
                }
                other => other.to_string(),
            };
            GrantLookup::Unresolved(reason)
        }
        GrantCheck::Valid(grant) => {
            let seat = crate::canonical_role_name(&grant.seat);
            let mut expires_at = grant.expires_at;
            if let Some(parent_id) = grant.parent_grant_id.as_deref() {
                match read_grant(parent_id) {
                    Ok(parent) => expires_at = expires_at.min(parent.expires_at),
                    Err(error) => return GrantLookup::Unresolved(error.to_string()),
                }
            }
            GrantLookup::Expiry(GrantExpiry { seat, expires_at })
        }
    }
}

// Informational statusline metadata deliberately includes expired records.
// It conveys no authority and is never consumed by the launch-refusal gate.
// Keeping it separate preserves the existing expired-record warning contract.
// trace:BUG-2011 | ai:codex
fn display_grant_lookup() -> GrantLookup {
    let Some(id) = std::env::var(GRANT_ENV)
        .ok()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
    else {
        return GrantLookup::NoGrant;
    };
    let grant = match read_grant(&id) {
        Ok(grant) => grant,
        Err(err) => return GrantLookup::Unresolved(err.to_string()),
    };
    if grant.revoked_at.is_some() {
        return GrantLookup::Unresolved("session grant was revoked".to_string());
    }
    let seat = crate::canonical_role_name(&grant.seat);
    if matches!(seat.as_str(), "implementer" | "guest" | "requester") {
        return GrantLookup::Baseline;
    }
    let mut expires_at = grant.expires_at;
    if let Some(parent_id) = grant.parent_grant_id.as_deref() {
        match read_grant(parent_id) {
            Ok(parent) if parent.revoked_at.is_some() => {
                return GrantLookup::Unresolved(
                    "the grant this session's grant was delegated from was revoked".to_string(),
                )
            }
            Ok(parent) => expires_at = expires_at.min(parent.expires_at),
            Err(err) => {
                return GrantLookup::Unresolved(format!(
                    "the grant this session's grant was delegated from: {err}"
                ))
            }
        }
    }
    GrantLookup::Expiry(GrantExpiry { seat, expires_at })
}

/// The expiry of the grant named by `AIDA_SESSION_GRANT`. `None` when there
/// is no grant, it is unresolvable or revoked, or its seat is one the
/// baseline confers anyway (its expiry costs nothing).
// trace:TASK-1736 | ai:claude
pub(crate) fn session_grant_expiry() -> Option<GrantExpiry> {
    match display_grant_lookup() {
        GrantLookup::Expiry(expiry) => Some(expiry),
        _ => None,
    }
}

/// `1h05m` / `42m` / `under 1m` for a positive span.
// trace:TASK-1736 | ai:claude
pub(crate) fn format_remaining(left: Duration) -> String {
    let mins = left.num_minutes();
    match mins {
        m if m < 1 => "under 1m".to_string(),
        m if m < 60 => format!("{m}m"),
        m => format!("{}h{:02}m", m / 60, m % 60),
    }
}

fn format_expiry_time(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M UTC").to_string()
}

fn reenter_command(seat: &str) -> String {
    format!("eval \"$(aida role enter {seat})\"")
}

/// Short statusline suffix for a grant inside `within` of expiry:
/// `expires in 42m`, or `grant expired`.
// trace:TASK-1736 | ai:claude
pub(crate) fn expiry_suffix(
    expiry: &GrantExpiry,
    now: DateTime<Utc>,
    within: Duration,
) -> Option<String> {
    let left = expiry.expires_at - now;
    if left >= within {
        None
    } else if left <= Duration::zero() {
        Some("grant expired".to_string())
    } else {
        Some(format!("expires in {}", format_remaining(left)))
    }
}

/// One-line warning for a grant inside `within` of expiry, naming the
/// expiry time and the command that renews it.
// trace:TASK-1736 | ai:claude
pub(crate) fn expiry_warning(
    expiry: &GrantExpiry,
    now: DateTime<Utc>,
    within: Duration,
) -> Option<String> {
    let left = expiry.expires_at - now;
    if left >= within {
        return None;
    }
    let seat = &expiry.seat;
    let at = format_expiry_time(expiry.expires_at);
    let cmd = reenter_command(seat);
    Some(if left <= Duration::zero() {
        format!(
            "The {seat} seat grant expired at {at}. Commands gated on {seat} will refuse until a human re-enters it at a terminal: {cmd}"
        )
    } else {
        format!(
            "The {seat} seat grant expires at {at} (in {}). Commands gated on {seat} will refuse after that. A human renews it at a terminal: {cmd}",
            format_remaining(left)
        )
    })
}

/// The refusal a relay prints instead of launching a session whose grant
/// has less than `within` left.
// trace:TASK-1736 | ai:claude
pub(crate) fn launch_refusal(
    expiry: &GrantExpiry,
    now: DateTime<Utc>,
    within: Duration,
) -> Option<String> {
    let left = expiry.expires_at - now;
    if left >= within {
        return None;
    }
    let seat = &expiry.seat;
    let at = format_expiry_time(expiry.expires_at);
    let state = if left <= Duration::zero() {
        format!("expired at {at}")
    } else {
        format!("expires at {at}, in {}", format_remaining(left))
    };
    Some(format!(
        "Not launching: the {seat} seat grant {state}, and a session needs at least {} left. Re-enter the seat at a terminal, then start again: {}",
        format_remaining(within),
        reenter_command(seat)
    ))
}

/// The refusal a relay prints when `AIDA_SESSION_GRANT` is set but names no
/// usable grant record: launching would only meet refusals mid-run. `label`
/// is the session's role label, used to name the renew command when known.
// trace:TASK-1736 | ai:claude
pub(crate) fn unresolved_launch_refusal(reason: &str, label: Option<&str>) -> String {
    let seat = label
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| crate::canonical_role_name(&s.to_ascii_lowercase()))
        .unwrap_or_else(|| "<seat>".to_string());
    format!(
        "Not launching: this session's seat grant cannot be used ({reason}). Re-enter the seat at a terminal, then start again: {}",
        reenter_command(&seat)
    )
}

pub(crate) fn require_direct_tty() -> Result<()> {
    use std::io::IsTerminal;
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        bail!("role grants require a human at an interactive TTY; run `aida role enter` from your terminal");
    }
    Ok(())
}

// Shared by direct seat issuance and operator-only ship explanations.
// trace:TASK-1602 | ai:codex
pub(crate) fn require_direct_human() -> Result<()> {
    require_direct_tty()?;
    if ["AIDA_AGENT_NAME", "AIDA_AGENT_TYPE"].iter().any(|key| {
        std::env::var(key)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
    }) {
        bail!("AIDA-managed agent sessions cannot issue direct TTY grants; request a scoped child seat from the launcher");
    }
    Ok(())
}

// trace:STORY-1473, TASK-1592 | ai:antigravity
pub(crate) fn issue_direct(
    project_root: &Path,
    seat: &str,
    delegable_seats: Vec<String>,
) -> Result<SeatGrant> {
    require_direct_human()?;
    // trace:TASK-1594 | ai:claude
    let subject = crate::current_user_id(None);
    let principal = subject.clone();
    if !roster_allows(project_root, &principal, seat) {
        bail!(
            "seat `{seat}` is not allowed by the team roster for `{principal}`; an interactive human must update the roster with `aida team allow-seat`"
        );
    }
    let allowed = aida_core::team::TeamRoster::load(
        &store_root(project_root)
            .context("session grants require an available AIDA team roster")?,
    )
    .seats_for(&principal)
    .unwrap_or_default()
    .into_iter()
    .map(|value| aida_core::team::canonical_role(&value))
    .collect::<std::collections::BTreeSet<_>>();
    let mut delegable_seats = delegable_seats
        .into_iter()
        .map(|seat| aida_core::team::canonical_role(&seat))
        .collect::<Vec<_>>();
    delegable_seats.sort();
    delegable_seats.dedup();
    if delegable_seats.iter().any(|seat| !allowed.contains(seat)) {
        bail!("delegated seats must all be present in the team roster");
    }
    let now = Utc::now();
    let grant = SeatGrant {
        id: Uuid::new_v4().to_string(),
        subject,
        principal,
        session_id: Uuid::new_v4().to_string(),
        seat: seat.to_string(),
        tty_issued_at: now,
        delegable_seats,
        parent_grant_id: None,
        issued_at: now,
        expires_at: now + MAX_GRANT_AGE,
        revoked_at: None,
    };
    write_grant(&grant)?;
    Ok(grant)
}

/// Read-only launch preflight; issuance repeats this validation after setup.
// trace:TASK-1337 | ai:codex
pub(crate) fn validate_child_delegation(
    project_root: &Path,
    requested_seat: &str,
) -> Result<SeatGrant> {
    let parent = current_grant(project_root).context("no valid parent seat grant is active")?;
    if !parent
        .delegable_seats
        .iter()
        .any(|seat| seat == requested_seat)
    {
        // trace:TASK-1597 | ai:claude
        bail!(
            "the active `{}` grant does not delegate `{requested_seat}` (re-enter the role to include it: `aida role enter {} --delegate-seat {requested_seat}`)",

            parent.seat,
            parent.seat
        );
    }
    if !roster_allows(project_root, &parent.principal, requested_seat) {
        bail!("requested child seat is no longer allowed by the team roster");
    }
    Ok(parent)
}

pub(crate) fn issue_child(
    project_root: &Path,
    requested_seat: &str,
    child_subject: &str,
) -> Result<SeatGrant> {
    let parent = validate_child_delegation(project_root, requested_seat)?;
    let now = Utc::now();
    let grant = SeatGrant {
        id: Uuid::new_v4().to_string(),
        principal: parent.principal,
        subject: child_subject.to_string(),
        session_id: Uuid::new_v4().to_string(),
        seat: requested_seat.to_string(),
        tty_issued_at: parent.tty_issued_at,
        delegable_seats: Vec::new(),
        parent_grant_id: Some(parent.id),
        issued_at: now,
        expires_at: parent.expires_at,
        revoked_at: None,
    };
    write_grant(&grant)?;
    Ok(grant)
}

pub(crate) fn revoke_current() -> Result<bool> {
    let Some(id) = std::env::var(GRANT_ENV).ok() else {
        return Ok(false);
    };
    let Ok(mut grant) = read_grant(&id) else {
        return Ok(false);
    };
    if grant.revoked_at.is_some() {
        return Ok(false);
    }
    grant.revoked_at = Some(Utc::now());
    write_grant(&grant)?;
    Ok(true)
}

pub(crate) fn grant_id(grant: &SeatGrant) -> &str {
    &grant.id
}

// trace:STORY-1473 | ai:claude
/// Hermetic grant establishment for tests.
///
/// ADR-66 removes every ambient authority path — `AIDA_SESSION_ROLE`, a bare
/// TTY, and roster-membership-as-seat — so a test that exercises an authorized
/// operation must establish a REAL grant the same way `aida role enter` does:
/// a roster ceiling in the project store plus a validated grant record in the
/// (hermetic) home, exported through `AIDA_SESSION_GRANT`. This writes exactly
/// the records production validates; nothing here is reachable from, or
/// weakens, the production issuance paths (`issue_direct` still requires a
/// controlling TTY).
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// Write the store plumbing (`.aida/config.toml`, `objects/`) plus a
    /// roster ceiling allowing `seat` ∪ `delegable` for `subject`, then mint
    /// a direct grant bound to `subject` and return its handle. The caller
    /// exports it via `AIDA_SESSION_GRANT` through the shared env guards
    /// (`test_env::AmbientGuard::hermetic_with_seat` bundles all of it).
    pub(crate) fn mint_grant_for(
        project_root: &Path,
        subject: &str,
        seat: &str,
        delegable: &[&str],
    ) -> String {
        // The mint resolves env-derived paths (the grant-store home via
        // `$AIDA_HOME`/`$HOME`); hold the process env lock for the duration
        // unless the caller already does, so a sibling test's guard cannot
        // swap those variables mid-mint and strand the record in a home this
        // mint never prepared. trace:STORY-1473 | ai:claude
        let _lock = (!crate::test_env::holds_env_lock()).then(crate::test_env::env_lock);
        let aida_dir = project_root.join(".aida");
        std::fs::create_dir_all(&aida_dir).unwrap();
        let config = aida_dir.join("config.toml");
        if !config.exists() {
            std::fs::write(&config, "store_path = \".aida-store\"\n").unwrap();
        }
        let store = crate::detect_distributed_store_from(project_root).unwrap_or_else(|| {
            // The fixture config exists but names no reachable store (e.g. an
            // `[agents]`-only config): append the default so the validation
            // path detects the same store this fixture seeds.
            let store = project_root.join(".aida-store");
            std::fs::create_dir_all(store.join("objects")).unwrap();
            let content = std::fs::read_to_string(&config).unwrap();
            if !content.contains("store_path") {
                // Prepended so the key stays top-level (appending after a
                // `[table]` header would land inside that table).
                std::fs::write(&config, format!("store_path = \".aida-store\"\n{content}"))
                    .unwrap();
            }
            store
        });
        std::fs::create_dir_all(store.join("objects")).unwrap();
        let registry = store.join("registry");
        std::fs::create_dir_all(&registry).unwrap();

        // Merge (never clobber) the roster ceiling for `subject`.
        let seat = aida_core::team::canonical_role(seat);
        let mut members: std::collections::BTreeMap<String, Vec<String>> =
            aida_core::team::TeamRoster::load(&store)
                .entries()
                .into_iter()
                .collect();
        let seats = members.entry(subject.to_string()).or_default();
        for requested in std::iter::once(seat.as_str()).chain(delegable.iter().copied()) {
            let canonical = aida_core::team::canonical_role(requested);
            if !seats.iter().any(|have| *have == canonical) {
                seats.push(canonical);
            }
        }
        #[derive(serde::Serialize)]
        struct RosterFile {
            members: std::collections::BTreeMap<String, Vec<String>>,
        }
        std::fs::write(
            registry.join("team.toml"),
            toml::to_string(&RosterFile { members }).unwrap(),
        )
        .unwrap();

        let now = Utc::now();
        let grant = SeatGrant {
            id: Uuid::new_v4().to_string(),
            principal: subject.to_string(),
            subject: subject.to_string(),
            session_id: Uuid::new_v4().to_string(),
            seat,
            tty_issued_at: now,
            delegable_seats: delegable
                .iter()
                .map(|role| aida_core::team::canonical_role(role))
                .collect(),
            parent_grant_id: None,
            issued_at: now,
            expires_at: now + MAX_GRANT_AGE,
            revoked_at: None,
        };
        write_grant(&grant).unwrap();
        grant.id
    }
}

// trace:STORY-1473 | ai:claude
#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-66 acceptance: an env-only `AIDA_SESSION_ROLE=advisor` confers no
    /// seat; a validated grant does; revocation and a shrunk roster ceiling
    /// fail closed.
    #[test]
    fn env_role_alone_confers_no_seat_and_grants_fail_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Env-only role, no grant: no seat.
        {
            let _env = crate::test_env::EnvVarsGuard::apply(&[
                ("AIDA_SESSION_ROLE", Some("advisor")),
                (GRANT_ENV, None),
            ]);
            assert_eq!(current_seat(root), None, "env role alone must not seat");
        }

        // trace:TASK-1594 | ai:claude
        let subject = crate::current_user_id(None);
        let id = test_support::mint_grant_for(root, &subject, "advisor", &["implementer"]);

        // A validated grant seats the session.
        {
            let _env = crate::test_env::EnvVarsGuard::apply(&[(GRANT_ENV, Some(id.as_str()))]);
            assert_eq!(current_seat(root).as_deref(), Some("advisor"));

            // trace:TASK-1337 | ai:codex
            let before = std::fs::read_dir(grant_dir().unwrap()).unwrap().count();
            validate_child_delegation(root, "implementer").unwrap();
            validate_child_delegation(root, "reviewer")
                .expect_err("preflight must preserve delegation gate");
            assert_eq!(
                std::fs::read_dir(grant_dir().unwrap()).unwrap().count(),
                before,
                "read-only validation must not issue a child grant"
            );
            // The grant delegates only its explicit subset.
            let child = issue_child(root, "implementer", "child-subject")
                .expect("delegable child seat is issuable");
            assert_eq!(child.seat, "implementer");
            assert_eq!(child.parent_grant_id.as_deref(), Some(id.as_str()));
            assert!(
                child.delegable_seats.is_empty(),
                "children default to an empty delegation set"
            );
            issue_child(root, "reviewer", "child-subject")
                .expect_err("a seat outside the delegation set must refuse");

            // Revocation fails closed.
            assert!(revoke_current().unwrap());
            assert_eq!(current_seat(root), None, "revoked grant must not seat");
        }

        // A fresh grant dies when the roster ceiling no longer allows it.
        let id = test_support::mint_grant_for(root, &subject, "reviewer", &[]);
        {
            let _env = crate::test_env::EnvVarsGuard::apply(&[(GRANT_ENV, Some(id.as_str()))]);
            assert_eq!(current_seat(root).as_deref(), Some("reviewer"));
            let store = crate::detect_distributed_store_from(root).unwrap();
            std::fs::write(store.join("registry").join("team.toml"), "[members]\n").unwrap();
            assert_eq!(
                current_seat(root),
                None,
                "a shrunk roster ceiling invalidates the grant"
            );
        }
    }

    /// A role label with no valid grant reads as ungranted; a validated grant
    /// reads as held; the implementer baseline needs no grant.
    // trace:BUG-1924 | ai:claude
    #[test]
    fn role_display_follows_the_grant_not_the_label() {
        assert_eq!(
            classify_role_display(Some("advisor"), None),
            RoleDisplay::Ungranted("advisor".into())
        );
        assert_eq!(
            classify_role_display(Some("dialog"), None),
            RoleDisplay::Ungranted("advisor".into()),
            "the deprecated alias canonicalizes before display"
        );
        assert_eq!(
            classify_role_display(Some("advisor"), Some("advisor")),
            RoleDisplay::Held("advisor".into())
        );
        assert_eq!(
            classify_role_display(Some("advisor"), Some("reviewer")),
            RoleDisplay::Held("reviewer".into()),
            "the grant, not the label, names the displayed seat"
        );
        assert_eq!(
            classify_role_display(None, Some("reviewer")),
            RoleDisplay::Held("reviewer".into())
        );
        assert_eq!(
            classify_role_display(Some("implementer"), None),
            RoleDisplay::Held("implementer".into())
        );
        assert_eq!(classify_role_display(None, None), RoleDisplay::Default);
        assert_eq!(
            classify_role_display(Some("  "), None),
            RoleDisplay::Default
        );
    }

    /// End to end through the grant store: an env-only advisor label is
    /// ungranted, a minted advisor grant is held, and revocation drops it
    /// back to ungranted.
    // trace:BUG-1924 | ai:claude
    #[test]
    fn role_display_validates_the_grant_store() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        {
            let _env = crate::test_env::EnvVarsGuard::apply(&[
                ("AIDA_SESSION_ROLE", Some("advisor")),
                (GRANT_ENV, None),
            ]);
            assert_eq!(
                role_display(root, Some("advisor")),
                RoleDisplay::Ungranted("advisor".into())
            );
        }
        let subject = crate::current_user_id(None);
        let id = test_support::mint_grant_for(root, &subject, "advisor", &[]);
        {
            let _env = crate::test_env::EnvVarsGuard::apply(&[
                ("AIDA_SESSION_ROLE", Some("advisor")),
                (GRANT_ENV, Some(id.as_str())),
            ]);
            assert_eq!(
                role_display(root, Some("advisor")),
                RoleDisplay::Held("advisor".into())
            );
            assert!(revoke_current().unwrap());
            assert_eq!(
                role_display(root, Some("advisor")),
                RoleDisplay::Ungranted("advisor".into()),
                "a revoked grant reads as ungranted"
            );
        }
    }

    fn expiry_in(seat: &str, left: Duration) -> (GrantExpiry, DateTime<Utc>) {
        let now = Utc::now();
        (
            GrantExpiry {
                seat: seat.to_string(),
                expires_at: now + left,
            },
            now,
        )
    }

    /// A grant near expiry warns and refuses a launch; one with time to
    /// spare does neither; an expired one says it expired.
    // trace:TASK-1736 | ai:claude
    #[test]
    fn expiry_messages_follow_the_time_left() {
        let (fresh, now) = expiry_in("advisor", Duration::hours(3));
        assert_eq!(expiry_warning(&fresh, now, EXPIRY_WARN_WITHIN), None);
        assert_eq!(expiry_suffix(&fresh, now, EXPIRY_WARN_WITHIN), None);
        assert_eq!(launch_refusal(&fresh, now, Duration::hours(1)), None);

        let (soon, now) = expiry_in("advisor", Duration::minutes(90));
        let warning = expiry_warning(&soon, now, EXPIRY_WARN_WITHIN).expect("warns inside 2h");
        assert!(warning.contains("expires at"), "{warning}");
        assert!(warning.contains("(in 1h30m)"), "{warning}");
        assert!(
            warning.contains("eval \"$(aida role enter advisor)\""),
            "{warning}"
        );
        assert_eq!(
            expiry_suffix(&soon, now, EXPIRY_WARN_WITHIN).as_deref(),
            Some("expires in 1h30m")
        );
        assert_eq!(
            launch_refusal(&soon, now, Duration::hours(1)),
            None,
            "90m left is enough to launch"
        );

        let (close, now) = expiry_in("reviewer", Duration::minutes(30));
        let refusal = launch_refusal(&close, now, Duration::hours(1)).expect("refuses under 1h");
        assert!(refusal.starts_with("Not launching"), "{refusal}");
        assert!(refusal.contains("in 30m"), "{refusal}");
        assert!(refusal.contains("aida role enter reviewer"), "{refusal}");

        let (gone, now) = expiry_in("advisor", -Duration::minutes(5));
        let warning = expiry_warning(&gone, now, EXPIRY_WARN_WITHIN).unwrap();
        assert!(warning.contains("expired at"), "{warning}");
        assert_eq!(
            expiry_suffix(&gone, now, EXPIRY_WARN_WITHIN).as_deref(),
            Some("grant expired")
        );
        assert!(launch_refusal(&gone, now, Duration::hours(1))
            .unwrap()
            .contains("expired at"));
    }

    /// A set handle naming no usable record is `Unresolved`, never silently
    /// `NoGrant`; the launch refusal names the renew command.
    // trace:TASK-1736 | ai:claude
    #[test]
    fn unresolvable_grant_is_distinguished_from_no_grant() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let subject = crate::current_user_id(None);
        {
            let _env = crate::test_env::EnvVarsGuard::apply(&[(GRANT_ENV, Some("  "))]);
            assert_eq!(session_grant_lookup(), GrantLookup::NoGrant);
        }
        let missing = Uuid::new_v4().to_string();
        for handle in [missing.as_str(), "not-a-uuid"] {
            let _env = crate::test_env::EnvVarsGuard::apply(&[(GRANT_ENV, Some(handle))]);
            assert!(
                matches!(session_grant_lookup(), GrantLookup::Unresolved(_)),
                "{handle}"
            );
        }
        let id = test_support::mint_grant_for(root, &subject, "advisor", &[]);
        let mut grant = read_grant(&id).unwrap();
        grant.revoked_at = Some(Utc::now());
        write_grant(&grant).unwrap();
        {
            let _env = crate::test_env::EnvVarsGuard::apply(&[(GRANT_ENV, Some(id.as_str()))]);
            assert_eq!(
                session_grant_lookup(),
                GrantLookup::Unresolved("session grant was revoked".into())
            );
        }
        let refusal = unresolved_launch_refusal("session grant was revoked", Some("Advisor"));
        assert!(refusal.starts_with("Not launching"), "{refusal}");
        assert!(refusal.contains("aida role enter advisor"), "{refusal}");
        assert!(unresolved_launch_refusal("x", None).contains("aida role enter <seat>"));
    }

    // trace:TASK-1736 | ai:claude
    #[test]
    fn format_remaining_is_compact() {
        assert_eq!(format_remaining(Duration::seconds(20)), "under 1m");
        assert_eq!(format_remaining(Duration::minutes(42)), "42m");
        assert_eq!(format_remaining(Duration::minutes(65)), "1h05m");
        assert_eq!(format_remaining(Duration::hours(2)), "2h00m");
    }

    /// The expiry comes from the raw record, so an expired grant still
    /// reports; a child grant dies with its parent; a revoked grant and the
    /// implementer baseline report nothing.
    // trace:TASK-1736 | ai:claude
    #[test]
    fn session_grant_expiry_reads_the_record_and_its_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let subject = crate::current_user_id(None);
        let rewrite = |id: &str, edit: &dyn Fn(&mut SeatGrant)| {
            let mut grant = read_grant(id).unwrap();
            edit(&mut grant);
            write_grant(&grant).unwrap();
            grant
        };

        {
            let _env = crate::test_env::EnvVarsGuard::apply(&[(GRANT_ENV, None)]);
            assert_eq!(session_grant_expiry(), None, "no grant, nothing to report");
        }

        let id = test_support::mint_grant_for(root, &subject, "advisor", &["reviewer"]);
        let soon = Utc::now() + Duration::minutes(30);
        rewrite(&id, &|g| g.expires_at = soon);
        {
            let _env = crate::test_env::EnvVarsGuard::apply(&[(GRANT_ENV, Some(id.as_str()))]);
            assert_eq!(
                session_grant_expiry(),
                Some(GrantExpiry {
                    seat: "advisor".into(),
                    expires_at: soon,
                })
            );
        }

        // A child carrying a later expiry than its parent still dies with it.
        let parent = rewrite(&id, &|g| g.expires_at = soon);
        let mut kid = parent.clone();
        kid.id = Uuid::new_v4().to_string();
        kid.seat = "reviewer".into();
        kid.parent_grant_id = Some(parent.id.clone());
        kid.expires_at = soon + Duration::hours(10);
        write_grant(&kid).unwrap();
        {
            let _env = crate::test_env::EnvVarsGuard::apply(&[(GRANT_ENV, Some(kid.id.as_str()))]);
            assert_eq!(session_grant_expiry().map(|e| e.expires_at), Some(soon));
        }

        // Already expired: current_grant drops it, but the expiry still reports.
        let past = Utc::now() - Duration::minutes(5);
        rewrite(&id, &|g| g.expires_at = past);
        {
            let _env = crate::test_env::EnvVarsGuard::apply(&[(GRANT_ENV, Some(id.as_str()))]);
            assert!(current_grant(root).is_none());
            assert_eq!(session_grant_expiry().map(|e| e.expires_at), Some(past));
        }

        rewrite(&id, &|g| g.revoked_at = Some(Utc::now()));
        {
            let _env = crate::test_env::EnvVarsGuard::apply(&[(GRANT_ENV, Some(id.as_str()))]);
            assert_eq!(
                session_grant_expiry(),
                None,
                "a revoked grant reports nothing"
            );
        }

        let implementer = test_support::mint_grant_for(root, &subject, "implementer", &[]);
        rewrite(&implementer, &|g| g.expires_at = soon);
        {
            let _env =
                crate::test_env::EnvVarsGuard::apply(&[(GRANT_ENV, Some(implementer.as_str()))]);
            assert_eq!(
                session_grant_expiry(),
                None,
                "the implementer baseline needs no grant, so its expiry costs nothing"
            );
        }
    }
}

// trace:BUG-2011 | ai:codex
#[cfg(test)]
#[path = "tests/bug_2011_grant_expiry_verdict_tests.rs"]
mod bug_2011_grant_expiry_verdict_tests;
