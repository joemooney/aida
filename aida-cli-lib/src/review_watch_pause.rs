//! Review-watch launch pause: a persisted, audited, expiring record.
//!
//! The pause is never an admission or train-member lock. Watchers read the
//! record (atomically replaced, so lock-free) where they would launch a new
//! review and skip the launch while it is active. Running reviewers are
//! untouched. Only pause/resume/expiry writes serialize, on their own lock.
// trace:BUG-2650 | ai:claude
use super::*;
use chrono::{DateTime, Duration, Utc};

const FILE: &str = "pause.json";
const LOCK: &str = "pause.lock";
const MAX_EVENTS: usize = 256;
/// Defaults for `[review] watch_pause_default` / `watch_pause_max`.
const DEFAULT_SECONDS: u64 = 3600;
const DEFAULT_MAX_SECONDS: u64 = 86400;
/// Absolute ceiling for a configured maximum (seven days).
const CEILING_SECONDS: u64 = 7 * 86400;

#[derive(clap::Subcommand, Clone, Debug)]
pub(super) enum Control {
    /// Pause new review launches for a bounded time.
    Pause {
        /// Reason to record in the audit history.
        #[arg(long)]
        reason: String,
        /// Pause duration, e.g. 3600, 30m or 2h (default and maximum come from
        /// `[review] watch_pause_default` / `watch_pause_max`: 1h / 24h).
        #[arg(long = "for", value_name = "DURATION")]
        duration: Option<String>,
    },
    /// Clear the active review-watch pause.
    Resume,
    /// Show the launch pause and outstanding review reservations.
    Status,
}

/// An active pause, as observed at a launch point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Paused {
    pub(super) actor: String,
    pub(super) reason: String,
    pub(super) expires_at: DateTime<Utc>,
}

impl std::fmt::Display for Paused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "review-watch paused by {} until {}: {}",
            self.actor, self.expires_at, self.reason
        )
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

/// Configured `(default, max)` pause durations in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Limits {
    pub(super) default: u64,
    pub(super) max: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            default: DEFAULT_SECONDS,
            max: DEFAULT_MAX_SECONDS,
        }
    }
}

fn duration_value(value: &toml::Value, key: &str) -> Result<u64> {
    let seconds = match value {
        toml::Value::Integer(n) => u64::try_from(*n).ok(),
        toml::Value::String(s) => crate::presence::parse_duration_secs(s),
        _ => None,
    };
    seconds.filter(|s| *s > 0).with_context(|| {
        format!("`[review] {key}` must be a positive duration (seconds, or e.g. 30m, 2h)")
    })
}

pub(super) fn parse_limits(content: &str) -> Result<Limits> {
    let config: toml::Value =
        toml::from_str(content).context("parse .aida/config.toml for review-watch pause limits")?;
    let review = config.get("review");
    let mut limits = Limits::default();
    if let Some(value) = review.and_then(|r| r.get("watch_pause_max")) {
        limits.max = duration_value(value, "watch_pause_max")?;
    }
    if let Some(value) = review.and_then(|r| r.get("watch_pause_default")) {
        limits.default = duration_value(value, "watch_pause_default")?;
    } else {
        limits.default = limits.default.min(limits.max);
    }
    anyhow::ensure!(
        limits.max <= CEILING_SECONDS,
        "`[review] watch_pause_max` cannot exceed {CEILING_SECONDS} seconds"
    );
    anyhow::ensure!(
        limits.default <= limits.max,
        "`[review] watch_pause_default` cannot exceed `watch_pause_max`"
    );
    Ok(limits)
}

fn limits(root: &Path) -> Result<Limits> {
    match crate::review_helpers::project_config_text(root) {
        Ok(content) => parse_limits(&content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Limits::default()),
        Err(error) => Err(error).context("read .aida/config.toml for review-watch pause limits"),
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
                            <= Duration::seconds(CEILING_SECONDS as i64),
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

// Callers hold the pause lock. Expiry is recorded once, at its deadline.
fn observe_locked(store: &Store, now: DateTime<Utc>) -> Result<Journal> {
    let mut journal = load(store)?;
    if let Some(receipt) = journal.active().filter(|r| r.expires_at <= now) {
        journal.push(Event::Expired {
            at: receipt.expires_at,
        });
        save(store, &journal)?;
    }
    Ok(journal)
}

/// The active pause, read without any admission or train lock. An expired
/// pause is inactive; its expiry is audited best-effort on the pause lock.
pub(super) fn current(store: &Store, now: DateTime<Utc>) -> Result<Option<Paused>> {
    let journal = load(store)?;
    let Some(receipt) = journal.active() else {
        return Ok(None);
    };
    if receipt.expires_at > now {
        return Ok(Some(Paused {
            actor: receipt.actor.clone(),
            reason: receipt.reason.clone(),
            expires_at: receipt.expires_at,
        }));
    }
    let recorded = store
        .dir
        .lock(LOCK)
        .and_then(|_lock| observe_locked(store, now));
    if let Err(error) = recorded {
        eprintln!("review-watch pause expiry not yet recorded: {error:#}");
    }
    Ok(None)
}

/// The launch-point check: `Err(Paused)` while a pause is active.
pub(super) fn check_launch(store: &Store) -> Result<()> {
    match current(store, Utc::now())? {
        Some(paused) => Err(paused.into()),
        None => Ok(()),
    }
}

/// The audited actor of a loop control: an advisor grant, or an unseated
/// operator. Shared by the review-watch pause and the advisor loops drain.
// trace:BUG-2703 | ai:claude
pub(crate) fn control_actor(root: &Path, control: &str) -> Result<String> {
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
        _ => anyhow::bail!("{control} requires an advisor grant or an unseated operator"),
    }
}

pub(super) fn run(store: &Store, control: &Control) -> Result<()> {
    if matches!(control, Control::Status) {
        let journal = {
            let _lock = store.dir.lock(LOCK)?;
            observe_locked(store, Utc::now())?
        };
        if let Some(receipt) = journal.active() {
            println!(
                "Paused until {} by {} ({})",
                receipt.expires_at, receipt.actor, receipt.reason
            );
        } else {
            println!("Review-watch launch pause inactive.");
        }
        let state = store.load()?;
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
    let actor = control_actor(&store.root, "review-watch pause control")?;
    let limits = limits(&store.root)?;
    let _lock = store.dir.lock(LOCK)?;
    let message = change_locked(store, control, actor, Utc::now(), limits)?;
    println!("{message}");
    Ok(())
}

fn change_locked(
    store: &Store,
    control: &Control,
    actor: String,
    now: DateTime<Utc>,
    limits: Limits,
) -> Result<String> {
    anyhow::ensure!(valid_text(&actor), "invalid review-watch control actor");
    let mut journal = observe_locked(store, now)?;
    let message = match control {
        Control::Pause { reason, duration } => {
            let reason = reason.trim();
            anyhow::ensure!(
                valid_text(reason),
                "pause reason must be 1–512 bytes without control characters"
            );
            let seconds = match duration {
                Some(raw) => crate::presence::parse_duration_secs(raw)
                    .filter(|s| *s > 0)
                    .with_context(|| {
                        format!("invalid pause duration `{raw}` (use seconds, or e.g. 30m, 2h)")
                    })?,
                None => limits.default,
            };
            anyhow::ensure!(
                seconds <= limits.max,
                "pause duration {seconds}s exceeds the configured maximum of {}s (`[review] watch_pause_max`)",
                limits.max
            );
            let expires_at = now + Duration::seconds(seconds as i64);
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
            "Review-watch launch pause cleared.".into()
        }
        Control::Resume => return Ok("Review-watch launch pause already inactive.".into()),
        Control::Status => unreachable!("handled before mutation"),
    };
    save(store, &journal)?;
    Ok(message)
}

#[cfg(test)]
pub(super) fn fixture_change(
    store: &Store,
    control: &Control,
    actor: &str,
    now: DateTime<Utc>,
) -> Result<String> {
    let _lock = store.dir.lock(LOCK)?;
    change_locked(store, control, actor.into(), now, Limits::default())
}
