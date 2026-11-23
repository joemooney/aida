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
const RECOVERY_PREFIX: &str = "recovered unreadable journal by ";
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
                Event::Resume { actor, .. } => {
                    valid_text(actor) || actor.strip_prefix(RECOVERY_PREFIX).is_some_and(valid_text)
                }
                Event::Expired { .. } => true,
            }),
        "invalid advisor loops drain journal"
    );
    Ok(journal)
}

/// The journal as written: always within what [`load`] reads. A journal of
/// long reasons can exceed that well before [`MAX_EVENTS`] (a reason of
/// backslashes or quotes doubles in JSON), so the oldest events are dropped
/// until it fits. The newest event, which decides whether a drain is active,
/// is never dropped: one event is far below the limit (review round 4,
/// finding 4).
// trace:BUG-2703 | ai:claude
fn encode(journal: &mut Journal) -> Result<Vec<u8>> {
    loop {
        let body = serde_json::to_vec_pretty(journal)?;
        if (body.len() as u64) < LIMIT {
            return Ok(body);
        }
        anyhow::ensure!(
            journal.events.len() > 1,
            "advisor loops drain event exceeds the journal limit"
        );
        // Shed in proportion to the excess, so one write re-encodes a few
        // times at most.
        let excess = body.len() as u64 - LIMIT + 1;
        let shed = (excess as usize * journal.events.len()).div_ceil(body.len());
        let shed = shed.clamp(1, journal.events.len() - 1);
        journal.events.drain(..shed);
    }
}

fn save(root: &Path, journal: &mut Journal) -> Result<()> {
    let dir = directory(root);
    let body = encode(journal)?;
    let mut temp = tempfile::NamedTempFile::new_in(&dir)?;
    temp.write_all(&body)?;
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
fn observe_locked(root: &Path, mut journal: Journal, now: DateTime<Utc>) -> Result<Journal> {
    if let Some(receipt) = journal.active().filter(|r| r.expires_at <= now) {
        journal.push(Event::Expired {
            at: receipt.expires_at,
        });
        save(root, &mut journal)?;
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
    if let Err(error) = try_lock(root).and_then(|_lock| observe_locked(root, load(root)?, now)) {
        eprintln!("advisor loops drain expiry not yet recorded: {error:#}");
    }
    Ok(None)
}

/// Drain the loops' admission claims for `duration` (default one hour).
pub(crate) fn drain(root: &Path, reason: &str, duration: Option<&str>) -> Result<String> {
    let seconds = match duration {
        Some(raw) => crate::presence::parse_duration_secs(raw)
            .filter(|s| *s > 0)
            .with_context(|| {
                format!("invalid drain duration `{raw}` (use seconds, or e.g. 30m, 2h)")
            })?,
        None => DEFAULT_SECONDS,
    };
    change(root, Some((reason, seconds)), || {
        Ok((
            crate::review_watch::control_actor(root, "advisor loops drain")?,
            Utc::now(),
        ))
    })
}

/// Clear an active drain.
pub(crate) fn resume(root: &Path) -> Result<String> {
    change(root, None, || {
        Ok((
            crate::review_watch::control_actor(root, "advisor loops resume")?,
            Utc::now(),
        ))
    })
}

/// `authority` yields the caller's actor and the operation time. It runs
/// under the drain lock: the wait for that lock has no bound, and a grant
/// that expired or was revoked during it must not write the journal, nor may
/// an operation be dated before the events it queued behind (review round 4,
/// finding 3).
// trace:BUG-2703 | ai:claude
fn change(
    root: &Path,
    drain: Option<(&str, u64)>,
    authority: impl FnOnce() -> Result<(String, DateTime<Utc>)>,
) -> Result<String> {
    let _lock = lock(root)?;
    let (actor, now) = authority()?;
    anyhow::ensure!(valid_text(&actor), "invalid advisor loops control actor");
    // Only an explicit, authorized resume may replace unreadable history.
    // trace:TASK-3246 | ai:codex
    let mut journal = match load(root) {
        Ok(journal) => observe_locked(root, journal, now)?,
        Err(error) if drain.is_none() => {
            // Version 1 rejects extra event kinds and fields. Qualify the
            // already validated actor in the existing resume audit.
            let actor = format!("{RECOVERY_PREFIX}{actor}");
            let mut replacement = Journal {
                version: 1,
                events: vec![Event::Resume { actor, at: now }],
            };
            save(root, &mut replacement).context("could not replace unreadable drain journal")?;
            return Ok(format!(
                "Advisor loops resumed. Replaced unreadable drain journal: {error:#}"
            ));
        }
        Err(error) => return Err(error),
    };
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
    save(root, &mut journal)?;
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
    change(root, Some((reason, seconds)), || Ok((actor.into(), now)))
}

#[cfg(test)]
pub(crate) fn fixture_resume(root: &Path, actor: &str) -> Result<String> {
    change(root, None, || Ok((actor.into(), Utc::now())))
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
        fixture_resume(root, "joe (uid 1)").unwrap();
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

    // trace:TASK-3246 | ai:codex
    #[test]
    fn recovery_preserves_a_maximum_length_actor() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let actor = "é".repeat(256);
        assert_eq!(actor.len(), 512);
        fixture_drain(root, &actor, "maintenance", 60, Utc::now()).unwrap();
        assert_eq!(active(root).unwrap().unwrap().actor, actor);

        std::fs::write(directory(root).join(FILE), b"{broken").unwrap();
        assert!(active(root).is_err());
        assert!(fixture_resume(root, &actor)
            .unwrap()
            .contains("Replaced unreadable"));
        assert_eq!(active(root).unwrap(), None);
        assert!(matches!(
            load(root).unwrap().events.as_slice(),
            [Event::Resume { actor: audit, .. }]
                if audit == &format!("recovered unreadable journal by {actor}")
        ));
    }

    /// Review round 4, finding 4: every journal the writer emits is one the
    /// reader accepts. Far more renewals than the journal keeps, each with a
    /// reason and an actor that double in JSON (backslashes, quotes), then a
    /// resume, a drain and an expiry: after every write the journal loads,
    /// stays within the reader's limit and still ends with the newest event.
    // trace:BUG-2703 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-465.ac00712a | ai:claude
    #[test]
    fn bug2703_journal_of_maximum_escaped_reasons_stays_readable() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let path = directory(root).join(FILE);
        let reasons = ["\\".repeat(512), "\"".repeat(512)];
        let actor = "\\".repeat(512);
        // One event of this shape, as the journal holds it: without the
        // writer's cap, MAX_EVENTS of them are several times the limit.
        let one = serde_json::to_vec_pretty(&Event::Drain(Receipt {
            actor: actor.clone(),
            reason: reasons[0].clone(),
            issued_at: Utc::now(),
            expires_at: Utc::now(),
        }))
        .unwrap()
        .len() as u64;
        assert!(one >= 2 * 1024, "{one}");
        assert!(one * MAX_EVENTS as u64 > 2 * LIMIT, "{one}");
        let now = Utc::now();
        let mut largest = 0;
        for round in 0..(MAX_EVENTS + 8) {
            let reason = &reasons[round % 2];
            let at = now + Duration::seconds(round as i64);
            fixture_drain(root, &actor, reason, MAX_SECONDS, at)
                .unwrap_or_else(|error| panic!("renewal {round}: {error:#}"));
            let size = std::fs::metadata(&path).unwrap().len();
            assert!(size <= LIMIT, "renewal {round} wrote {size} bytes");
            largest = largest.max(size);
            let journal = load(root).unwrap_or_else(|error| panic!("renewal {round}: {error:#}"));
            let newest = journal.active().expect("the renewal is the active drain");
            assert_eq!((&newest.reason, newest.issued_at), (reason, at));
            let drained = active_at(root, at).unwrap().expect("drained");
            assert_eq!(&drained.reason, reason);
        }
        // The cap was reached, and it bit before the event count did.
        assert!(largest > LIMIT / 2, "{largest}");
        assert!(load(root).unwrap().events.len() < MAX_EVENTS);
        // Resume, drain again and let it expire: each still reads and writes.
        assert_eq!(
            fixture_resume(root, &actor).unwrap(),
            "Advisor loops drain cleared."
        );
        assert_eq!(active(root).unwrap(), None);
        let later = Utc::now();
        fixture_drain(root, &actor, &reasons[0], 60, later).unwrap();
        assert!(active_at(root, later).unwrap().is_some());
        assert_eq!(
            active_at(root, later + Duration::seconds(61)).unwrap(),
            None
        );
        assert!(matches!(
            load(root).unwrap().events.last(),
            Some(Event::Expired { .. })
        ));
        assert!(std::fs::metadata(&path).unwrap().len() <= LIMIT);
    }

    /// Review round 4, finding 3: authority is checked under the drain
    /// lock, not before the wait for it. An advisor whose grant is valid when
    /// `drain()` or `resume()` is called, and revoked while the call waits
    /// for the lock, is refused, and the journal is left exactly as it was.
    // trace:BUG-2703 | ai:claude
    // trace:BUG-b6b1278890774ef993d2ebc327862470-465.ac00712a | ai:claude
    #[test]
    fn bug2703_grant_revoked_while_waiting_for_the_drain_lock_is_refused() {
        use crate::gate::seat_authority::GRANT_ENV;
        use crate::gate::seat_authority::{
            grant_check, revoke_by_id, test_support::mint_grant_for, GrantCheck,
        };
        // The grant store lives under a pinned HOME: run in its own process.
        if aida_testkit::test_env::run_path_isolated_test_in_scratch() {
            return;
        }
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
        env.set_key("AIDA_SESSION_ROLE", "advisor");
        let journal = || std::fs::read(directory(&root).join(FILE)).ok();
        for resuming in [false, true] {
            let grant = mint_grant_for(&root, crate::test_env::HERMETIC_TEST_USER, "advisor", &[]);
            env.set_key(GRANT_ENV, &grant);
            if resuming {
                // A drain for the resume to clear, were it let through.
                drain(&root, "advisor's own drain", Some("10m")).unwrap();
                assert!(active(&root).unwrap().is_some());
            }
            let before = journal();
            assert!(
                matches!(grant_check(&root), GrantCheck::Valid(ref g) if g.seat == "advisor"),
                "the grant is valid when the call starts"
            );
            let held = lock(&root).unwrap();
            let waiter = {
                let root = root.clone();
                std::thread::spawn(move || {
                    if resuming {
                        resume(&root)
                    } else {
                        drain(&root, "land #4324", Some("10m"))
                    }
                })
            };
            // The call is waiting for the lock this test holds.
            std::thread::sleep(std::time::Duration::from_millis(500));
            assert!(!waiter.is_finished(), "the call waits for the drain lock");
            assert!(revoke_by_id(&grant).unwrap(), "revoked during the wait");
            drop(held);
            let error = format!("{:#}", waiter.join().unwrap().expect_err("refused"));
            assert!(
                error.contains(
                    "requires an advisor grant or an operator at an interactive terminal"
                ),
                "{error}"
            );
            assert_eq!(journal(), before, "the journal changed");
            assert_eq!(active(&root).unwrap().is_some(), resuming);
        }
    }

    /// The operation time is sampled under the lock too: a drain that waited
    /// is dated, and expires, from when it took effect.
    // trace:BUG-2703 | ai:claude
    #[test]
    fn bug2703_drain_is_dated_after_its_wait_for_the_lock() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let held = lock(&root).unwrap();
        let waiter = {
            let root = root.clone();
            std::thread::spawn(move || {
                change(&root, Some(("land #4324", 60)), || {
                    Ok(("joe (uid 1)".into(), Utc::now()))
                })
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(300));
        let released = Utc::now();
        drop(held);
        waiter.join().unwrap().unwrap();
        let journal = load(&root).unwrap();
        let receipt = journal.active().unwrap();
        assert!(receipt.issued_at >= released, "{}", receipt.issued_at);
    }

    /// Review round 3, finding 1: the refusal side of the authority gate,
    /// through the real `drain()` and `resume()`. A valid grant for a seat
    /// other than advisor, a seated session with no grant, an invalid grant
    /// and (review round 4, finding 1) a grantless process that is not a
    /// person at a terminal outside any agent session are all refused, and
    /// the journal is left exactly as it was.
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
        const REFUSAL: &str =
            "requires an advisor grant or an operator at an interactive terminal outside any agent session";
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
                // The suite may itself run inside an agent session.
                ("AIDA_AGENT_NAME", None),
                ("AIDA_AGENT_TYPE", None),
                ("AIDA_SESSION_ID", None),
                ("AIDA_HEADLESS", None),
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
            // (d) Review round 4, finding 1: no grant and no role is not an
            // operator. A process with no terminal is refused, and so is
            // one at a terminal that an agent identity, a session id or a
            // headless lane marks as AIDA-managed, each on its own.
            env.unset_key(GRANT_ENV);
            assert!(matches!(grant_check(&root), GrantCheck::Absent));
            refused("no grant, no role, no terminal", &before);
            env.set_terminal(true);
            for (key, value) in [
                ("AIDA_AGENT_NAME", "claude-implementer"),
                ("AIDA_AGENT_TYPE", "claude"),
                ("AIDA_SESSION_ID", "01a1fad87977"),
                ("AIDA_HEADLESS", "1"),
                // Set at all counts: an empty identity is not an operator.
                ("AIDA_AGENT_TYPE", ""),
            ] {
                env.set_key(key, value);
                refused(&format!("managed process ({key}) without a grant"), &before);
                env.unset_key(key);
            }
            env.set_key("AIDA_SESSION_ROLE", "advisor");
            refused("seated at a terminal without a grant", &before);
            env.unset_key("AIDA_SESSION_ROLE");
            env.set_terminal(false);
            assert_eq!(active(&root).unwrap().is_some(), drained);
        }
        // The same calls succeed with the authority, and name their actor:
        // the operator at a terminal resumes, the advisor drains.
        env.unset_key(GRANT_ENV);
        env.unset_key("AIDA_SESSION_ROLE");
        env.set_terminal(true);
        assert_eq!(resume(&root).unwrap(), "Advisor loops drain cleared.");
        assert!(matches!(
            load(&root).unwrap().events.last(),
            Some(Event::Resume { actor, .. }) if actor.contains("(uid ")
        ));
        // The advisor's grant needs no terminal: its worker has none.
        env.set_terminal(false);
        env.set_key("AIDA_SESSION_ROLE", "advisor");
        env.set_key(GRANT_ENV, &advisor);
        drain(&root, "land #4324", None).unwrap();
        let drained = active(&root).unwrap().unwrap();
        assert!(drained.actor.contains("(advisor "), "{}", drained.actor);

        // trace:TASK-3246 | ai:codex
        for damaged in [&b"{broken"[..], &[0xff][..]] {
            let path = directory(&root).join(FILE);
            std::fs::write(&path, damaged).unwrap();
            assert!(active(&root).is_err());
            env.unset_key(GRANT_ENV);
            assert!(resume(&root).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), damaged);
            env.set_key(GRANT_ENV, &advisor);
            assert!(drain(&root, "land #4324", None).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), damaged);
            assert!(active(&root).is_err());
            assert!(resume(&root).unwrap().contains("Replaced unreadable"));
            assert_eq!(active(&root).unwrap(), None);
            assert!(matches!(
                load(&root).unwrap().events.as_slice(),
                [Event::Resume { actor, .. }]
                    if actor == &format!("recovered unreadable journal by {}", drained.actor)
            ));
        }
    }
}
