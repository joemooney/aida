//! Advisor loops drain: a persisted, audited, expiring record.
//!
//! While a drain is active the loop controllers (review-watch and the
//! auto-train) give up their shared admission claims at their next tick
//! boundary and take no new one, and the deputy's supervisor does not
//! respawn an exited controller. A running review or train is never cut
//! short: controllers yield only between passes. Nothing here grants
//! anything; the drain only withholds the loops' claims, so a land or ship
//! can take the exclusive mode without anyone signalling the workers.
// trace:BUG-2703 | ai:claude
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

// Not `.json`: that extension is reserved for claim receipts in this
// directory, which every admission reads as a holder.
const FILE: &str = "loops-drain.journal";
const LOCK: &str = "drain.lock";
const MAX_EVENTS: usize = 256;
const LIMIT: u64 = 256 * 1024;
/// A drain without `--for` lasts an hour; none may outlast a day.
pub(crate) const DEFAULT_SECONDS: u64 = 3600;
pub(crate) const MAX_SECONDS: u64 = 86400;

/// An active drain, as observed by a controller or the supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Drained {
    pub(crate) actor: String,
    pub(crate) reason: String,
    pub(crate) expires_at: DateTime<Utc>,
}

impl std::fmt::Display for Drained {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "advisor loops drained by {} until {}: {}",
            self.actor,
            self.expires_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            self.reason
        )
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    actor: String,
    reason: String,
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Event {
    Drain(Receipt),
    Resume { actor: String, at: DateTime<Utc> },
    Expired { at: DateTime<Utc> },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u8,
    events: Vec<Event>,
}

impl Journal {
    fn active(&self) -> Option<&Receipt> {
        match self.events.last()? {
            Event::Drain(receipt) => Some(receipt),
            _ => None,
        }
    }
    fn push(&mut self, event: Event) {
        if self.events.len() == MAX_EVENTS {
            self.events.remove(0);
        }
        self.events.push(event);
    }
}

fn directory(root: &Path) -> PathBuf {
    root.join(".aida/admission-owner")
}

fn valid_text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

fn load(root: &Path) -> Result<Journal> {
    let path = directory(root).join(FILE);
    let body = match aida_core::seat_clone_detect::read_regular_file_result_with_limit(&path, LIMIT)
    {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Journal {
                version: 1,
                events: Vec::new(),
            });
        }
        Err(error) => return Err(error).context("advisor loops drain journal is unreadable"),
    };
    let journal: Journal =
        serde_json::from_str(&body).context("invalid advisor loops drain journal")?;
    anyhow::ensure!(
        journal.version == 1
            && !journal.events.is_empty()
            && journal.events.len() <= MAX_EVENTS
            && journal.events.iter().all(|event| match event {
                Event::Drain(receipt) =>
                    valid_text(&receipt.actor)
                        && valid_text(&receipt.reason)
                        && receipt.expires_at > receipt.issued_at
                        && receipt.expires_at - receipt.issued_at
                            <= Duration::seconds(MAX_SECONDS as i64),
                Event::Resume { actor, .. } => valid_text(actor),
                Event::Expired { .. } => true,
            }),
        "invalid advisor loops drain journal"
    );
    Ok(journal)
}

fn save(root: &Path, journal: &Journal) -> Result<()> {
    let dir = directory(root);
    let mut temp = tempfile::NamedTempFile::new_in(&dir)?;
    temp.write_all(&serde_json::to_vec_pretty(journal)?)?;
    temp.as_file().sync_all()?;
    temp.persist(dir.join(FILE)).map_err(|e| e.error)?;
    Ok(())
}

struct Locked(std::fs::File);
impl Drop for Locked {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn lock(root: &Path) -> Result<Locked> {
    let dir = directory(root);
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(LOCK))?;
    file.lock_exclusive()?;
    Ok(Locked(file))
}

// trace:BUG-2703 | ai:claude
fn try_lock(root: &Path) -> Result<Locked> {
    let dir = directory(root);
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(LOCK))?;
    file.try_lock_exclusive()
        .context("the drain lock is busy; expiry audit deferred")?;
    Ok(Locked(file))
}

// Callers hold the drain lock. Expiry is recorded once, at its deadline.
fn observe_locked(root: &Path, now: DateTime<Utc>) -> Result<Journal> {
    let mut journal = load(root)?;
    if let Some(receipt) = journal.active().filter(|r| r.expires_at <= now) {
        journal.push(Event::Expired {
            at: receipt.expires_at,
        });
        save(root, &journal)?;
    }
    Ok(journal)
}

/// The active drain, read without any lock (the journal is atomically
/// replaced). An expired drain is inactive; its expiry is audited on the
/// drain lock best-effort. An unreadable journal is an error: callers treat
/// it as drained (refuse the claim, do not respawn).
pub(crate) fn active(root: &Path) -> Result<Option<Drained>> {
    active_at(root, Utc::now())
}

fn active_at(root: &Path, now: DateTime<Utc>) -> Result<Option<Drained>> {
    let journal = load(root)?;
    let Some(receipt) = journal.active() else {
        return Ok(None);
    };
    if receipt.expires_at > now {
        return Ok(Some(Drained {
            actor: receipt.actor.clone(),
            reason: receipt.reason.clone(),
            expires_at: receipt.expires_at,
        }));
    }
    // Best effort and never a wait: admission callers carry a bound, so a
    // busy drain lock defers the audit to a later reader (BUG-2703 round 2).
    // trace:BUG-2703 | ai:claude
    if let Err(error) = try_lock(root).and_then(|_lock| observe_locked(root, now)) {
        eprintln!("advisor loops drain expiry not yet recorded: {error:#}");
    }
    Ok(None)
}

/// Drain the loops' admission claims for `duration` (default one hour).
pub(crate) fn drain(root: &Path, reason: &str, duration: Option<&str>) -> Result<String> {
    let actor = crate::review_watch::control_actor(root, "advisor loops drain")?;
    let seconds = match duration {
        Some(raw) => crate::presence::parse_duration_secs(raw)
            .filter(|s| *s > 0)
            .with_context(|| {
                format!("invalid drain duration `{raw}` (use seconds, or e.g. 30m, 2h)")
            })?,
        None => DEFAULT_SECONDS,
    };
    change(root, Some((reason, seconds)), actor, Utc::now())
}

/// Clear an active drain.
pub(crate) fn resume(root: &Path) -> Result<String> {
    let actor = crate::review_watch::control_actor(root, "advisor loops resume")?;
    change(root, None, actor, Utc::now())
}

fn change(
    root: &Path,
    drain: Option<(&str, u64)>,
    actor: String,
    now: DateTime<Utc>,
) -> Result<String> {
    anyhow::ensure!(valid_text(&actor), "invalid advisor loops control actor");
    let _lock = lock(root)?;
    let mut journal = observe_locked(root, now)?;
    let message = match drain {
        Some((reason, seconds)) => {
            let reason = reason.trim();
            anyhow::ensure!(
                valid_text(reason),
                "drain reason must be 1–512 bytes without control characters"
            );
            anyhow::ensure!(
                seconds <= MAX_SECONDS,
                "drain duration {seconds}s exceeds the maximum of {MAX_SECONDS}s"
            );
            let expires_at = now + Duration::seconds(seconds as i64);
            journal.push(Event::Drain(Receipt {
                actor,
                reason: reason.into(),
                issued_at: now,
                expires_at,
            }));
            format!(
                "Advisor loops drained until {}: review-watch and auto-train give up their admission claims at their next tick boundary, and exited loop workers are not restarted. Running reviews and trains finish first.",
                expires_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            )
        }
        None if journal.active().is_some() => {
            journal.push(Event::Resume { actor, at: now });
            "Advisor loops drain cleared.".into()
        }
        None => return Ok("Advisor loops drain already inactive.".into()),
    };
    save(root, &journal)?;
    Ok(message)
}

#[cfg(test)]
pub(crate) fn fixture_drain(
    root: &Path,
    actor: &str,
    reason: &str,
    seconds: u64,
    now: DateTime<Utc>,
) -> Result<String> {
    change(root, Some((reason, seconds)), actor.into(), now)
}

#[cfg(test)]
pub(crate) fn fixture_resume(root: &Path, actor: &str) -> Result<String> {
    change(root, None, actor.into(), Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    // trace:BUG-2703 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-465.ac00712a | ai:claude
    #[test]
    fn bug2703_drain_is_audited_bounded_and_expires() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        assert_eq!(active(root).unwrap(), None);
        let now = Utc::now();
        assert!(fixture_drain(root, "joe (uid 1)", "land #4324", MAX_SECONDS + 1, now).is_err());
        assert!(fixture_drain(root, "joe (uid 1)", "  ", 60, now).is_err());
        fixture_drain(root, "joe (uid 1)", "land #4324", 60, now).unwrap();
        let drained = active_at(root, now + Duration::seconds(30))
            .unwrap()
            .unwrap();
        assert_eq!(drained.actor, "joe (uid 1)");
        assert!(drained.to_string().contains("land #4324"));
        // It expires by itself, and the expiry is journaled once.
        assert_eq!(active_at(root, now + Duration::seconds(61)).unwrap(), None);
        let journal = load(root).unwrap();
        assert!(matches!(journal.events.last(), Some(Event::Expired { .. })));
        assert_eq!(journal.events.len(), 2);
        // Resume is audited with its actor.
        fixture_drain(root, "advisor-121", "again", 60, Utc::now()).unwrap();
        change(root, None, "joe (uid 1)".into(), Utc::now()).unwrap();
        assert_eq!(active(root).unwrap(), None);
        assert!(matches!(
            load(root).unwrap().events.last(),
            Some(Event::Resume { actor, .. }) if actor == "joe (uid 1)"
        ));
    }

    // An unreadable journal is never "not drained".
    // trace:BUG-2703 | ai:claude
    #[test]
    fn bug2703_unreadable_drain_journal_refuses() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory(temp.path())).unwrap();
        std::fs::write(directory(temp.path()).join(FILE), b"{broken").unwrap();
        assert!(active(temp.path()).is_err());
    }

    /// Review round 3, finding 1: the refusal side of the authority gate,
    /// through the real `drain()` and `resume()`. A valid grant for a seat
    /// other than advisor, a seated session with no grant and an invalid
    /// grant are all refused, and the journal is left exactly as it was.
    /// Each seat grant is asserted Valid first, so only the advisor guard in
    /// `control_actor` can refuse it: accept any seat there, or drop the
    /// call from `drain()` or `resume()`, and this test fails.
    // trace:BUG-2703 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-465.ac00712a | ai:claude
    #[test]
    fn bug2703_drain_and_resume_refuse_without_advisor_authority() {
        use crate::gate::seat_authority::GRANT_ENV;
        use crate::gate::seat_authority::{grant_check, test_support::mint_grant_for, GrantCheck};
        // The grant store lives under a pinned HOME: run in its own process.
        if aida_testkit::test_env::run_path_isolated_test_in_scratch() {
            return;
        }
        const REFUSAL: &str = "requires an advisor grant or an unseated operator";
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let home = temp.path().join("home");
        for dir in [&root, &home] {
            std::fs::create_dir(dir).unwrap();
        }
        let mut env = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            &root,
            "advisor",
            &[],
            &[
                ("HOME", Some(home.to_str().unwrap())),
                ("AIDA_HOME", Some(home.to_str().unwrap())),
            ],
        );
        // Mint in the pinned home, as the session's launcher would.
        let mint =
            |seat: &str| mint_grant_for(&root, crate::test_env::HERMETIC_TEST_USER, seat, &[]);
        let advisor = mint("advisor");
        let journal = || std::fs::read(directory(&root).join(FILE)).ok();
        let refused = |what: &str, journal_before: &Option<Vec<u8>>| {
            for (control, result) in [
                (
                    "advisor loops drain",
                    drain(&root, "land #4324", Some("10m")),
                ),
                ("advisor loops resume", resume(&root)),
            ] {
                let error = format!("{:#}", result.expect_err(what));
                assert!(
                    error.contains(&format!("{control} {REFUSAL}")),
                    "{what}: {error}"
                );
            }
            assert_eq!(&journal(), journal_before, "{what}: the journal changed");
        };
        // Run every refusal twice: with no drain recorded (nothing may be
        // written) and with an advisor's drain active (it may not be
        // cleared, replaced or extended).
        for drained in [false, true] {
            if drained {
                env.set_key("AIDA_SESSION_ROLE", "advisor");
                env.set_key(GRANT_ENV, &advisor);
                drain(&root, "advisor's own drain", Some("10m")).unwrap();
                assert!(active(&root).unwrap().is_some());
            } else {
                assert_eq!(journal(), None);
            }
            let before = journal();
            // (a) A valid grant for a seat that is not the advisor.
            for seat in ["implementer", "reviewer"] {
                let grant = mint(seat);
                env.set_key("AIDA_SESSION_ROLE", seat);
                env.set_key(GRANT_ENV, &grant);
                assert!(
                    matches!(grant_check(&root), GrantCheck::Valid(ref g) if g.seat == seat),
                    "fixture {seat} grant must validate so only the seat guard can refuse"
                );
                refused(seat, &before);
            }
            // (b) A seated session with no grant at all.
            env.unset_key(GRANT_ENV);
            for seat in ["implementer", "advisor"] {
                env.set_key("AIDA_SESSION_ROLE", seat);
                assert!(matches!(grant_check(&root), GrantCheck::Absent));
                refused("seated without a grant", &before);
            }
            // (c) A grant handle that does not validate, seated or not.
            env.set_key(GRANT_ENV, "invalid-grant");
            refused("invalid grant, seated", &before);
            env.unset_key("AIDA_SESSION_ROLE");
            refused("invalid grant, unseated", &before);
            assert_eq!(active(&root).unwrap().is_some(), drained);
        }
        // The same calls succeed with the authority, and name their actor:
        // the unseated operator resumes, the advisor drains.
        env.unset_key(GRANT_ENV);
        env.unset_key("AIDA_SESSION_ROLE");
        assert_eq!(resume(&root).unwrap(), "Advisor loops drain cleared.");
        assert!(matches!(
            load(&root).unwrap().events.last(),
            Some(Event::Resume { actor, .. }) if actor.contains("(uid ")
        ));
        env.set_key("AIDA_SESSION_ROLE", "advisor");
        env.set_key(GRANT_ENV, &advisor);
        drain(&root, "land #4324", None).unwrap();
        let drained = active(&root).unwrap().unwrap();
        assert!(drained.actor.contains("(advisor "), "{}", drained.actor);
    }
}
