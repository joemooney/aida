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
    let home = dirs::home_dir().context("cannot locate the user's home directory")?;
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
    let path = grant_path(&grant.id)?;
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

fn roster_allows(project_root: &Path, principal: &str, seat: &str) -> bool {
    store_root(project_root)
        .and_then(|store| aida_core::team::TeamRoster::load(&store).seats_for(principal))
        .map(|seats| {
            seats
                .iter()
                .any(|allowed| aida_core::team::canonical_role(allowed) == seat)
        })
        .unwrap_or(false)
}

pub(crate) fn current_grant(project_root: &Path) -> Option<SeatGrant> {
    let id = std::env::var(GRANT_ENV).ok()?;
    let grant = read_grant(&id).ok()?;
    let now = Utc::now();
    if grant.revoked_at.is_some()
        || now >= grant.expires_at
        || grant.subject != crate::current_user_id(None)
        || !roster_allows(project_root, &grant.principal, &grant.seat)
    {
        return None;
    }
    if let Some(parent_id) = &grant.parent_grant_id {
        let parent = read_grant(parent_id).ok()?;
        if parent.revoked_at.is_some()
            || now >= parent.expires_at
            || parent.principal != grant.principal
            || !parent
                .delegable_seats
                .iter()
                .any(|seat| seat == &grant.seat)
        {
            return None;
        }
    }
    Some(grant)
}

pub(crate) fn current_seat(project_root: &Path) -> Option<String> {
    current_grant(project_root).map(|grant| grant.seat)
}

pub(crate) fn require_direct_tty() -> Result<()> {
    use std::io::IsTerminal;
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        bail!("role grants require a human at an interactive TTY; run `aida role enter` from your terminal");
    }
    Ok(())
}

pub(crate) fn issue_direct(
    project_root: &Path,
    seat: &str,
    delegable_seats: Vec<String>,
) -> Result<SeatGrant> {
    require_direct_tty()?;
    if ["AIDA_AGENT_NAME", "AIDA_AGENT_TYPE"].iter().any(|key| {
        std::env::var(key)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
    }) {
        bail!("AIDA-managed agent sessions cannot issue direct TTY grants; request a scoped child seat from the launcher");
    }
    let subject = crate::current_user_id(None);
    let principal = current_grant(project_root)
        .map(|parent| parent.principal)
        .unwrap_or_else(|| subject.clone());
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

pub(crate) fn issue_child(
    project_root: &Path,
    requested_seat: &str,
    child_subject: &str,
) -> Result<SeatGrant> {
    let parent = current_grant(project_root).context("no valid parent seat grant is active")?;
    if !parent
        .delegable_seats
        .iter()
        .any(|seat| seat == requested_seat)
    {
        bail!(
            "the active `{}` grant does not delegate `{requested_seat}`",
            parent.seat
        );
    }
    if !roster_allows(project_root, &parent.principal, requested_seat) {
        bail!("requested child seat is no longer allowed by the team roster");
    }
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
