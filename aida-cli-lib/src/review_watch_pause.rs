//! Local review-watch admission control. State and audit update atomically.
// trace:BUG-2650 | ai:codex
use super::*;
use chrono::{DateTime, Duration, Utc};

const FILE: &str = "pause.json";
const MAX_SECONDS: u64 = 86400;
const MAX_EVENTS: usize = 256;

#[derive(clap::Subcommand, Clone, Debug)]
pub(super) enum Control {
    /// Pause new review-watch admissions for a bounded time.
    Pause {
        /// Reason to record in the audit history.
        #[arg(long)]
        reason: String,
        /// Pause duration in seconds (default 3600).
        #[arg(long = "for", default_value_t = 3600, hide_default_value = true, value_parser = clap::value_parser!(u64).range(1..=86400))]
        seconds: u64,
    },
    /// Clear the active review-watch pause.
    Resume,
    /// Show the admission pause and outstanding review reservations.
    Status,
}

#[derive(Debug)]
pub(super) struct Paused(DateTime<Utc>);

impl std::fmt::Display for Paused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "review-watch paused until {}", self.0)
    }
}

impl std::error::Error for Paused {}

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
    Pause(Receipt),
    Resume { actor: String, at: DateTime<Utc> },
    Expired { at: DateTime<Utc> },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u8,
    repository: String,
    events: Vec<Event>,
}

impl Journal {
    fn active(&self) -> Option<&Receipt> {
        match self.events.last()? {
            Event::Pause(receipt) => Some(receipt),
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

fn valid_text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

fn load(store: &Store) -> Result<Journal> {
    let Some(bytes) = store.dir.read(FILE)? else {
        return Ok(Journal {
            version: 1,
            repository: store.repository.clone(),
            events: Vec::new(),
        });
    };
    let journal: Journal =
        serde_json::from_slice(&bytes).context("invalid review-watch pause journal")?;
    anyhow::ensure!(
        journal.version == 1
            && journal.repository == store.repository
            && !journal.events.is_empty()
            && journal.events.len() <= MAX_EVENTS
            && journal.events.iter().all(|event| match event {
                Event::Pause(receipt) =>
                    valid_text(&receipt.actor)
                        && valid_text(&receipt.reason)
                        && receipt.expires_at > receipt.issued_at
                        && receipt.expires_at - receipt.issued_at
                            <= Duration::seconds(MAX_SECONDS as i64),
                Event::Resume { actor, .. } => valid_text(actor),
                Event::Expired { .. } => true,
            }),
        "invalid review-watch pause journal"
    );
    Ok(journal)
}

fn save(store: &Store, journal: &Journal) -> Result<()> {
    store.dir.write(FILE, &serde_json::to_vec_pretty(journal)?)
}

// Callers hold admission.lock. Expiry is recorded at first observation.
fn observe(store: &Store, now: DateTime<Utc>) -> Result<Journal> {
    let mut journal = load(store)?;
    if let Some(receipt) = journal.active().filter(|r| r.expires_at <= now) {
        journal.push(Event::Expired {
            at: receipt.expires_at,
        });
        save(store, &journal)?;
    }
    Ok(journal)
}

pub(super) fn effective_locked(store: &Store, now: DateTime<Utc>) -> Result<Option<Paused>> {
    Ok(observe(store, now)?
        .active()
        .map(|receipt| Paused(receipt.expires_at)))
}

// Hold this guard from the final check through queue-work spawn. Mutating
// controls take the same lock before admission.lock. Hub I/O can then occur
// without holding admission.lock or delaying review-state reconciliation.
pub(super) fn launch_guard(store: &Store) -> Result<RuntimeLock> {
    let launch = store
        .dir
        .lock_mode("pause-launch.lock", FileExt::try_lock_shared)?;
    let _admission = store.dir.lock("admission.lock")?;
    if let Some(paused) = effective_locked(store, Utc::now())? {
        return Err(paused.into());
    }
    Ok(launch)
}

fn actor(root: &Path) -> Result<String> {
    use crate::gate::seat_authority::{grant_check, GrantCheck};
    let env = aida_core::exec_context::Environment::capture();
    match grant_check(root) {
        GrantCheck::Valid(grant) if grant.seat == "advisor" => {
            Ok(format!("{} (advisor {})", grant.principal, grant.subject))
        }
        GrantCheck::Absent if env.get("AIDA_SESSION_ROLE").is_none() => Ok(format!(
            "{} (uid {})",
            crate::gate::kernel_deps::operator_principal_from(&env),
            effective_uid()
        )),
        _ => anyhow::bail!(
            "review-watch pause control requires an advisor grant or an unseated operator"
        ),
    }
}

pub(super) fn run(store: &Store, control: &Control) -> Result<()> {
    if matches!(control, Control::Status) {
        let _admission = store.dir.lock("admission.lock")?;
        let journal = observe(store, Utc::now())?;
        if let Some(receipt) = journal.active() {
            println!(
                "Paused until {} by {} ({})",
                receipt.expires_at, receipt.actor, receipt.reason
            );
        } else {
            println!("Review-watch admission pause inactive.");
        }
        let state = store.load()?;
        drop(_admission);
        let holders = capacity_holders(&state);
        if !holders.is_empty() {
            println!("Outstanding review reservations: {holders}");
        }
        for (key, entry) in &state.entries {
            for attempt in &entry.attempts {
                if attempt.phase == Phase::RemoteHeld {
                    println!("{key}: reservation held at the admission hub");
                } else if !active(attempt)
                    && (!attempt.recorded().is_empty()
                        || attempt.unrecorded_descendant()
                        || attempt.pgid.is_some())
                {
                    if let Err(reason) = confirm(attempt) {
                        println!("{key}: reviewer exit unconfirmed: {reason}");
                    }
                }
            }
        }
        return Ok(());
    }
    let _launch = store.dir.lock("pause-launch.lock")?;
    let _admission = store.dir.lock("admission.lock")?;
    let actor = actor(&store.root)?;
    change_locked(store, control, actor, Utc::now())
}

fn change_locked(
    store: &Store,
    control: &Control,
    actor: String,
    now: DateTime<Utc>,
) -> Result<()> {
    anyhow::ensure!(valid_text(&actor), "invalid review-watch control actor");
    let mut journal = observe(store, now)?;
    let message = match control {
        Control::Pause { reason, seconds } => {
            let reason = reason.trim();
            anyhow::ensure!(
                valid_text(reason),
                "pause reason must be 1–512 bytes without control characters"
            );
            anyhow::ensure!(
                (1..=MAX_SECONDS).contains(seconds),
                "pause duration must be 1–86400 seconds"
            );
            let expires_at = now + Duration::seconds(*seconds as i64);
            journal.push(Event::Pause(Receipt {
                actor,
                reason: reason.into(),
                issued_at: now,
                expires_at,
            }));
            format!("Review-watch paused until {expires_at}.")
        }
        Control::Resume if journal.active().is_some() => {
            journal.push(Event::Resume { actor, at: now });
            "Review-watch admission pause cleared.".into()
        }
        Control::Resume => {
            println!("Review-watch admission pause already inactive.");
            return Ok(());
        }
        Control::Status => unreachable!("handled before mutation"),
    };
    save(store, &journal)?;
    println!("{message}");
    Ok(())
}

#[cfg(test)]
pub(super) fn fixture_change(store: &Store, control: &Control, now: DateTime<Utc>) -> Result<()> {
    let _launch = store.dir.lock("pause-launch.lock")?;
    let _admission = store.dir.lock("admission.lock")?;
    change_locked(store, control, "fixture advisor".into(), now)
}
