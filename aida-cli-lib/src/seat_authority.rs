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
            || !roster_allows(project_root, &parent.principal, &parent.seat)
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

/// BUG-1918: did grant `id` authorize seat `seat` at instant `at`? Re-read
/// from the grant store — never trusted from a recorded copy. The grant (and
/// its parent, for a delegated child) must exist, carry exactly that seat,
/// have been issued before `at`, not expired or been revoked by `at`, and the
/// seat must still be allowed by the roster. An implementer seat never
/// authorizes an approval.
// trace:BUG-1918 | ai:claude
pub(crate) fn grant_authorized_at(
    project_root: &Path,
    id: &str,
    seat: &str,
    at: DateTime<Utc>,
) -> bool {
    let seat = aida_core::team::canonical_role(seat.trim());
    if seat.is_empty() || seat == "implementer" {
        return false;
    }
    let valid_at = |g: &SeatGrant| {
        g.issued_at <= at && at < g.expires_at && g.revoked_at.is_none_or(|r| r > at)
    };
    let Ok(grant) = read_grant(id) else {
        return false;
    };
    if aida_core::team::canonical_role(&grant.seat) != seat
        || !valid_at(&grant)
        || !roster_allows(project_root, &grant.principal, &seat)
    {
        return false;
    }
    match &grant.parent_grant_id {
        None => true,
        Some(parent_id) => read_grant(parent_id).is_ok_and(|parent| {
            valid_at(&parent)
                && parent.principal == grant.principal
                && parent.delegable_seats.iter().any(|s| s == &grant.seat)
        }),
    }
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
}
