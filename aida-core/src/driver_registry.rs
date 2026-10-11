//! Driver registry and the derived per-spec "driver" (who is working it).
//!
//! A *driver* is a long-lived process that keeps specs moving without holding
//! each spec's lease itself: a relay (aida-relay working a batch through
//! subagents), a drain, or an explicitly registered agent. Drivers register one
//! JSON file each under `.aida/drivers/<kind>-<id>.json`, written atomically
//! (stage + rename), and refresh it by re-registering.
//!
//! Liveness is DERIVED, never stored: a record is live iff its pid is alive
//! (with the kernel start identity guarding PID reuse when recorded) AND its
//! heartbeat is fresher than the record's TTL (default
//! [`DEFAULT_HEARTBEAT_TTL_SECS`]). Dead or stale records are ignored by every
//! reader and never deleted here — only an explicit [`deregister`] of a
//! caller's own id removes a file.
//!
//! The per-spec driver ([`DerivedDriver`]) is likewise computed on every read
//! from the registry, the spec's lease, the drain lock and the spec's batch
//! tags ([`resolve_driver`]). It is never written to the spec.
// trace:STORY-1653 | ai:claude

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A heartbeat older than this (seconds) makes a record stale even when its
/// pid is still alive — a wedged driver stops covering specs on its own.
// trace:STORY-1653 | ai:claude
pub const DEFAULT_HEARTBEAT_TTL_SECS: i64 = 15 * 60;

/// Environment variable a relay exports to its sessions: the relay run id.
/// Lease-minting verbs stamp `relay:<value>` as the lease's driver.
pub const ENV_RELAY_RUN: &str = "AIDA_RELAY_RUN";
/// Generic driver attribution (`<kind>:<id>`; a bare value means a relay run).
pub const ENV_DRIVER: &str = "AIDA_DRIVER";

/// The kind of a registered driver.
// trace:STORY-1653 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DriverKind {
    Relay,
    Drain,
    Agent,
}

impl DriverKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DriverKind::Relay => "relay",
            DriverKind::Drain => "drain",
            DriverKind::Agent => "agent",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "relay" => Some(DriverKind::Relay),
            "drain" => Some(DriverKind::Drain),
            "agent" => Some(DriverKind::Agent),
            _ => None,
        }
    }
}

/// One registered driver, as stored in `.aida/drivers/<kind>-<id>.json`.
// trace:STORY-1653 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriverRecord {
    pub kind: DriverKind,
    /// Run id (relay), pid string (drain) or session/agent id (agent).
    pub id: String,
    pub pid: u32,
    /// Kernel start identity of `pid` at registration, when the platform can
    /// report it. Guards against a recycled pid keeping a record live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_start_time: Option<String>,
    /// Tag selector the driver works through, e.g. `batch:next`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    pub started_at: DateTime<Utc>,
    pub heartbeat_at: DateTime<Utc>,
    /// Heartbeat TTL override in seconds; `None` means the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_secs: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<String>,
}

impl DriverRecord {
    /// `relay:<run>` — the same `<kind>:<id>` form a lease's `driver` carries.
    pub fn label(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.id)
    }

    pub fn ttl_secs(&self) -> i64 {
        self.ttl_secs
            .filter(|t| *t > 0)
            .unwrap_or(DEFAULT_HEARTBEAT_TTL_SECS)
    }

    /// Pure liveness: pid alive (per `pid_alive`) AND heartbeat within TTL.
    pub fn is_live_with(&self, now: DateTime<Utc>, pid_alive: impl Fn(&Self) -> bool) -> bool {
        let age = now.signed_duration_since(self.heartbeat_at).num_seconds();
        age <= self.ttl_secs() && pid_alive(self)
    }

    /// Liveness against the real process table.
    #[cfg(feature = "native")]
    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        self.is_live_with(now, |r| {
            crate::liveness::process_identity_is_alive(r.pid, r.pid_start_time.as_deref())
        })
    }
}

/// Input to [`register`].
#[derive(Debug, Clone)]
pub struct Registration {
    pub kind: DriverKind,
    pub id: String,
    pub pid: u32,
    pub selector: Option<String>,
    pub seat: Option<String>,
    pub handoff: Option<String>,
    pub ttl_secs: Option<i64>,
}

/// `.aida/drivers/` under a project root.
pub fn drivers_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("drivers")
}

/// Ids land in a file name: keep `[A-Za-z0-9._-]`, map everything else to `_`.
fn file_safe(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The record file for one driver.
pub fn record_path(project_root: &Path, kind: DriverKind, id: &str) -> PathBuf {
    drivers_dir(project_root).join(format!("{}-{}.json", kind.as_str(), file_safe(id)))
}

/// Validate a driver id: non-empty, no whitespace or `:` (it is the right-hand
/// side of a `<kind>:<id>` label).
pub fn validate_id(id: &str) -> Result<(), String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("driver id must not be empty".into());
    }
    if id
        .chars()
        .any(|c| c.is_whitespace() || c == ':' || c == '/')
    {
        return Err(format!(
            "driver id `{id}` must not contain whitespace, `:` or `/`"
        ));
    }
    Ok(())
}

/// Register (or refresh) a driver. Re-registering the same kind+id refreshes
/// `heartbeat_at`; `started_at` is kept while the pid is unchanged, so a
/// relay's per-session refresh does not reset its age.
// trace:STORY-1653 | ai:claude
pub fn register(
    project_root: &Path,
    reg: Registration,
    pid_start_time: Option<String>,
    now: DateTime<Utc>,
) -> std::io::Result<DriverRecord> {
    validate_id(&reg.id).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let path = record_path(project_root, reg.kind, &reg.id);
    let started_at = read_record(&path)
        .filter(|old| old.pid == reg.pid)
        .map(|old| old.started_at)
        .unwrap_or(now);
    let rec = DriverRecord {
        kind: reg.kind,
        id: reg.id.trim().to_string(),
        pid: reg.pid,
        pid_start_time,
        selector: reg.selector.filter(|s| !s.trim().is_empty()),
        started_at,
        heartbeat_at: now,
        ttl_secs: reg.ttl_secs,
        seat: reg.seat.filter(|s| !s.trim().is_empty()),
        handoff: reg.handoff.filter(|s| !s.trim().is_empty()),
    };
    std::fs::create_dir_all(drivers_dir(project_root))?;
    let body = serde_json::to_string_pretty(&rec)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    crate::fs_atomic::write_atomic(&path, body)?;
    Ok(rec)
}

/// Remove one driver's own record. Returns `false` when there was none.
pub fn deregister(project_root: &Path, kind: DriverKind, id: &str) -> std::io::Result<bool> {
    let path = record_path(project_root, kind, id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

fn read_record(path: &Path) -> Option<DriverRecord> {
    let body = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&body).ok()
}

/// Every parseable record, live or not, sorted by kind then id. Malformed or
/// staging files are skipped, never fatal.
pub fn read_all(project_root: &Path) -> Vec<DriverRecord> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(drivers_dir(project_root)) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Some(rec) = read_record(&p) {
            out.push(rec);
        }
    }
    out.sort_by(|a, b| (a.kind.as_str(), &a.id).cmp(&(b.kind.as_str(), &b.id)));
    out
}

/// Only the live records (real process table).
#[cfg(feature = "native")]
pub fn live_drivers(project_root: &Path, now: DateTime<Utc>) -> Vec<DriverRecord> {
    read_all(project_root)
        .into_iter()
        .filter(|r| r.is_live(now))
        .collect()
}

/// The lease `driver` value implied by the environment: `AIDA_RELAY_RUN`
/// (→ `relay:<run>`) wins over `AIDA_DRIVER` (`<kind>:<id>`, bare → relay).
// trace:STORY-1653 | ai:claude
pub fn driver_from_env_values(relay_run: Option<&str>, driver: Option<&str>) -> Option<String> {
    if let Some(run) = relay_run.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(format!("relay:{run}"));
    }
    let d = driver.map(str::trim).filter(|s| !s.is_empty())?;
    match d.split_once(':') {
        Some((k, id)) if DriverKind::parse(k).is_some() && !id.trim().is_empty() => {
            Some(format!("{}:{}", k.trim().to_ascii_lowercase(), id.trim()))
        }
        Some(_) => None,
        None => Some(format!("relay:{d}")),
    }
}

/// [`driver_from_env_values`] over the real process environment.
pub fn driver_from_env() -> Option<String> {
    driver_from_env_values(
        std::env::var(ENV_RELAY_RUN).ok().as_deref(),
        std::env::var(ENV_DRIVER).ok().as_deref(),
    )
}

// ---------------------------------------------------------------------------
// Derived per-spec driver
// ---------------------------------------------------------------------------

/// What (if anything) is working a spec, derived on every read.
// trace:STORY-1653 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DerivedDriver {
    /// A live relay holds the lease or covers the spec's batch tag.
    Relay { run: String },
    /// A live drain covers the spec.
    Drain { pid: u32 },
    /// A live session holds the lease directly (or a live registered agent
    /// driver is named on the lease).
    Agent { who: String },
    /// In a batch that no live driver covers.
    Queued { batch: String },
    /// Nothing will pick it up. `since_secs` is the time since last activity.
    Idle { since_secs: Option<i64> },
}

impl DerivedDriver {
    /// `driver_kind` in JSON/TOON: relay | drain | agent | queued | idle.
    pub fn kind(&self) -> &'static str {
        match self {
            DerivedDriver::Relay { .. } => "relay",
            DerivedDriver::Drain { .. } => "drain",
            DerivedDriver::Agent { .. } => "agent",
            DerivedDriver::Queued { .. } => "queued",
            DerivedDriver::Idle { .. } => "idle",
        }
    }

    /// The operator-facing label: `relay:lib-split`, `drain:4242`,
    /// `agent:01a128c0`, `queued (batch:next)`, `idle 26h`.
    pub fn label(&self) -> String {
        match self {
            DerivedDriver::Relay { run } => format!("relay:{run}"),
            DerivedDriver::Drain { pid } => format!("drain:{pid}"),
            DerivedDriver::Agent { who } => format!("agent:{who}"),
            DerivedDriver::Queued { batch } => format!("queued ({batch})"),
            DerivedDriver::Idle {
                since_secs: Some(s),
            } => format!("idle {}", short_age(*s)),
            DerivedDriver::Idle { since_secs: None } => "idle".to_string(),
        }
    }

    /// True when something live is working the spec.
    pub fn is_driven(&self) -> bool {
        matches!(
            self,
            DerivedDriver::Relay { .. } | DerivedDriver::Drain { .. } | DerivedDriver::Agent { .. }
        )
    }

    /// Does this driver match a `--driver <kind>` filter value?
    pub fn matches_filter(&self, filter: &str) -> bool {
        self.kind().eq_ignore_ascii_case(filter.trim())
    }
}

/// Compact age: `45s`, `12m`, `26h`, `3d`.
pub fn short_age(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 48 * 3600 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

/// Everything [`resolve_driver`] needs about one spec, already probed.
// trace:STORY-1653 | ai:claude
#[derive(Debug, Clone, Default)]
pub struct DriverInputs<'a> {
    /// `driver` recorded on the spec's lease (`relay:<run>` etc.).
    pub lease_driver: Option<&'a str>,
    /// The spec's lease id (short form shown for `agent:`), when leased.
    pub lease_session: Option<&'a str>,
    /// The lease's own process (active/creator pid or live claude) is alive.
    pub lease_owner_alive: bool,
    /// Pid of a live drain whose lock covers the spec.
    pub drain_pid: Option<u32>,
    /// The spec's tags (only `batch:*` and selector matches are consulted).
    pub tags: &'a [String],
    /// Last activity (lease start, branch movement or spec update), for idle age.
    pub last_activity: Option<DateTime<Utc>>,
}

/// Resolve the single driver of one spec. First match wins:
/// 1. the lease names a live registered driver;
/// 2. a live drain covers the spec;
/// 3. the lease's own process is alive → `agent:<session>`;
/// 4. a spec tag matches a live relay's selector;
/// 5. the spec carries a `batch:*` tag → `queued (batch:X)`;
/// 6. otherwise `idle <age>`.
// trace:STORY-1653 | ai:claude
pub fn resolve_driver(
    inputs: &DriverInputs<'_>,
    live: &[DriverRecord],
    now: DateTime<Utc>,
) -> DerivedDriver {
    if let Some(label) = inputs.lease_driver.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(rec) = live.iter().find(|r| r.label() == label) {
            return derived_from_record(rec);
        }
    }
    if let Some(pid) = inputs.drain_pid {
        return DerivedDriver::Drain { pid };
    }
    if inputs.lease_owner_alive {
        let who = inputs
            .lease_session
            .map(|s| s.chars().take(8).collect::<String>())
            .unwrap_or_else(|| "session".to_string());
        return DerivedDriver::Agent { who };
    }
    if let Some(rec) = live.iter().find(|r| {
        r.selector
            .as_deref()
            .is_some_and(|sel| inputs.tags.iter().any(|t| t == sel))
    }) {
        return derived_from_record(rec);
    }
    if let Some(batch) = inputs.tags.iter().find(|t| t.starts_with("batch:")) {
        return DerivedDriver::Queued {
            batch: batch.clone(),
        };
    }
    DerivedDriver::Idle {
        since_secs: inputs
            .last_activity
            .map(|t| now.signed_duration_since(t).num_seconds().max(0)),
    }
}

fn derived_from_record(rec: &DriverRecord) -> DerivedDriver {
    match rec.kind {
        DriverKind::Relay => DerivedDriver::Relay {
            run: rec.id.clone(),
        },
        DriverKind::Drain => DerivedDriver::Drain { pid: rec.pid },
        DriverKind::Agent => DerivedDriver::Agent {
            who: rec.id.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(kind: DriverKind, id: &str, selector: Option<&str>, hb_age: i64) -> DriverRecord {
        let now = Utc::now();
        DriverRecord {
            kind,
            id: id.into(),
            pid: 4242,
            pid_start_time: None,
            selector: selector.map(String::from),
            started_at: now,
            heartbeat_at: now - chrono::Duration::seconds(hb_age),
            ttl_secs: None,
            seat: None,
            handoff: None,
        }
    }

    #[test]
    fn liveness_needs_pid_and_fresh_heartbeat() {
        let now = Utc::now();
        let fresh = rec(DriverKind::Relay, "r", None, 10);
        let stale = rec(DriverKind::Relay, "r", None, DEFAULT_HEARTBEAT_TTL_SECS + 5);
        assert!(fresh.is_live_with(now, |_| true));
        assert!(!fresh.is_live_with(now, |_| false));
        assert!(!stale.is_live_with(now, |_| true));
    }

    #[test]
    fn register_refresh_keeps_started_at_and_deregister_removes_only_own() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let t0 = Utc::now() - chrono::Duration::minutes(5);
        let reg = |id: &str| Registration {
            kind: DriverKind::Relay,
            id: id.into(),
            pid: 7,
            selector: Some("batch:next".into()),
            seat: None,
            handoff: None,
            ttl_secs: None,
        };
        let a = register(root, reg("a"), None, t0).unwrap();
        register(root, reg("b"), None, t0).unwrap();
        let a2 = register(root, reg("a"), None, Utc::now()).unwrap();
        assert_eq!(a.started_at, a2.started_at);
        assert!(a2.heartbeat_at > a.heartbeat_at);
        assert!(deregister(root, DriverKind::Relay, "a").unwrap());
        assert!(!deregister(root, DriverKind::Relay, "a").unwrap());
        let left: Vec<_> = read_all(root).into_iter().map(|r| r.id).collect();
        assert_eq!(left, vec!["b".to_string()]);
    }

    #[test]
    fn resolution_order() {
        let now = Utc::now();
        let relay = rec(DriverKind::Relay, "lib-split", Some("batch:next"), 0);
        let tags = vec!["batch:next".to_string()];
        let none: Vec<String> = vec![];
        // lease names the live relay
        let i = DriverInputs {
            lease_driver: Some("relay:lib-split"),
            tags: &none,
            ..Default::default()
        };
        assert_eq!(
            resolve_driver(&i, &[relay.clone()], now).label(),
            "relay:lib-split"
        );
        // live drain beats a live agent
        let i = DriverInputs {
            drain_pid: Some(99),
            lease_owner_alive: true,
            tags: &none,
            ..Default::default()
        };
        assert_eq!(resolve_driver(&i, &[], now).label(), "drain:99");
        // own pid alive
        let i = DriverInputs {
            lease_owner_alive: true,
            lease_session: Some("01a128c0ffee"),
            tags: &tags,
            ..Default::default()
        };
        assert_eq!(resolve_driver(&i, &[], now).label(), "agent:01a128c0");
        // batch tag matches a live relay selector
        let i = DriverInputs {
            tags: &tags,
            ..Default::default()
        };
        assert_eq!(resolve_driver(&i, &[relay], now).label(), "relay:lib-split");
        // no live driver -> queued
        assert_eq!(resolve_driver(&i, &[], now).label(), "queued (batch:next)");
        // nothing -> idle with age
        let i = DriverInputs {
            tags: &none,
            last_activity: Some(now - chrono::Duration::hours(26)),
            ..Default::default()
        };
        let d = resolve_driver(&i, &[], now);
        assert_eq!(d.label(), "idle 26h");
        assert_eq!(d.kind(), "idle");
        assert!(d.matches_filter("IDLE"));
    }

    #[test]
    fn env_attribution() {
        assert_eq!(
            driver_from_env_values(Some("run1"), Some("drain:5")).as_deref(),
            Some("relay:run1")
        );
        assert_eq!(
            driver_from_env_values(None, Some("drain:5")).as_deref(),
            Some("drain:5")
        );
        assert_eq!(
            driver_from_env_values(None, Some("x")).as_deref(),
            Some("relay:x")
        );
        assert_eq!(driver_from_env_values(None, Some("bogus:5")), None);
        assert_eq!(driver_from_env_values(Some("  "), None), None);
    }
}
