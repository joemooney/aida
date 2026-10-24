//! STORY-1698: the fire-fighting mandate — a human-minted, time-boxed grant
//! that lets the advisor seat ship held PRs carrying an attested, independent
//! approval while the operator is away.
//!
//! Records live ONLY in the user's AIDA home (`~/.aida/mandates/<uuid>.json`,
//! mode 0600), written after [`crate::seat_authority::require_direct_human`].
//! The git store is never consulted for authority. `.aida/events.jsonl`
//! carries the audit copy (`MandateChanged`).
//!
//! Every load re-validates each record (24h cap, no future issue time, the
//! advisor seat and known scope, the current user as principal, a 0600
//! regular file owned by this uid, never a symlink); an invalid record is
//! ignored with a warning and never shadows a valid one. Revocation is
//! monotonic: `aida mandate stop` writes a `<id>.stopped` tombstone that is
//! never removed, and any revocation evidence wins.
//!
//! Trust boundary (the same as session grants): start and stop require a
//! human at an interactive terminal. A same-uid process that forges files in
//! the AIDA home is outside the threat model; the mandate is a liveness
//! mechanism, not a defense against a malicious same-uid agent. Accepted
//! risk: a process that wraps itself in a pseudo-terminal passes the
//! terminal check, exactly as it does for `aida role enter`.
//!
//! Expiry is never cached: every ship decision re-reads the records and
//! compares them against the clock at that instant.
// trace:STORY-1698 | ai:claude
// mandate-gate

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use uuid::Uuid;

// trace:STORY-1728 | ai:claude
#[path = "mandate_args.rs"]
pub(crate) mod mandate_args;
pub(crate) use mandate_args::*;

const MANDATE_DIR: &str = "mandates";
const EXCLUSION_LEDGER: &str = "exclusions.jsonl";
/// The only scope a mandate can carry today.
pub(crate) const SCOPE_SHIP_HELD: &str = "ship-held";
/// The only seat a mandate covers.
pub(crate) const MANDATE_SEAT: &str = "advisor";
pub(crate) const DEFAULT_TTL: Duration = Duration::hours(8);
pub(crate) const MAX_TTL: Duration = Duration::hours(24);
/// Clock skew tolerated on a record's `issued_at`.
pub(crate) const ISSUE_SKEW: Duration = Duration::minutes(5);
/// The tag that keeps a spec human-only whatever the mandate says.
pub(crate) const OPERATOR_ONLY_TAG: &str = "operator-only";
/// STORY-1700: the `kind` of a drive mandate (ADR-77).
pub(crate) const KIND_DRIVE: &str = "drive";

/// STORY-1700: what a mandate covers beyond its on-disk `scope`. A closed
/// set with no catch-all arm, so an unknown scope string fails
/// deserialization and [`load_valid`] ignores that record (fail closed).
/// STORY-1706: `relay-restart` lets the drive tick restart a crashed relay
/// from its human launch record. A drive record minted before it does not
/// cover it; the operator re-mints.
// trace:STORY-1700 | ai:claude
// trace:STORY-1706 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Scope {
    ShipHeld,
    SketchSignoff,
    Dispatch,
    RelayRestart,
}

impl Scope {
    pub(crate) const ALL: [Scope; 4] = [
        Scope::ShipHeld,
        Scope::SketchSignoff,
        Scope::Dispatch,
        Scope::RelayRestart,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ShipHeld => SCOPE_SHIP_HELD,
            Self::SketchSignoff => "sketch-signoff",
            Self::Dispatch => "dispatch",
            Self::RelayRestart => "relay-restart",
        }
    }
}

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
    /// Lifts the sensitive spec classes (never operator-only or decide).
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
    /// STORY-1700: scopes beyond `scope`. `scope` stays `ship-held` on disk,
    /// so a STORY-1698 reader still loads a drive record as ship-held.
    // trace:STORY-1700 | ai:claude
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub scopes: BTreeSet<Scope>,
    /// STORY-1700: `Some("drive")` for a drive mandate; absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl Mandate {
    fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        self.revoked_at.is_none() && now < self.expires_at
    }

    /// STORY-1700: a drive mandate (ADR-77)?
    // trace:STORY-1700 | ai:claude
    pub(crate) fn is_drive(&self) -> bool {
        self.kind.as_deref() == Some(KIND_DRIVE)
    }

    /// STORY-1700: `scope` (always ship-held) plus `scopes`.
    // trace:STORY-1700 | ai:claude
    pub(crate) fn effective_scopes(&self) -> BTreeSet<Scope> {
        let mut out = self.scopes.clone();
        out.insert(Scope::ShipHeld);
        out
    }

    pub(crate) fn covers(&self, scope: Scope) -> bool {
        self.effective_scopes().contains(&scope)
    }
}

/// The mandate situation at one instant.
// trace:STORY-1698 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MandateState {
    /// No valid mandate record exists.
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

/// Is `path` a regular file (never a symlink), mode 0600, owned by this uid?
/// `Err` says why not.
// trace:STORY-1698 | ai:claude
fn private_file_check(path: &Path) -> std::result::Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| {
            if e.raw_os_error() == Some(libc::ELOOP) {
                "is a symlink".to_owned()
            } else {
                format!("unreadable: {e}")
            }
        })?;
    opened_private_file_check(&file)
}

/// Write `body` to `path` as a 0600 file, atomically (temp + rename).
// trace:STORY-1698 | ai:claude
fn write_private(path: &Path, body: &[u8]) -> Result<()> {
    use std::io::Write;
    let dir = path.parent().context("mandate path has no parent")?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("mandate"),
        Uuid::new_v4()
    ));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&tmp)?;
    file.write_all(body)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// The `<id>.stopped` tombstone path.
fn tombstone_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.stopped"))
}

/// Revocation evidence for `id`: the tombstone's recorded time and author,
/// or (when present but unreadable) the moment of the read. Any tombstone at
/// all — whatever its mode or content — revokes.
// trace:STORY-1698 | ai:claude
fn tombstone(dir: &Path, id: &str) -> Option<(DateTime<Utc>, Option<String>)> {
    let path = tombstone_path(dir, id);
    std::fs::symlink_metadata(&path).ok()?;
    let parsed = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let at = parsed
        .as_ref()
        .and_then(|v| v.get("stopped_at")?.as_str()?.parse().ok())
        .unwrap_or_else(Utc::now);
    let by = parsed
        .as_ref()
        .and_then(|v| v.get("stopped_by")?.as_str().map(str::to_string));
    Some((at, by))
}

/// Why a parsed record is not a valid mandate for `principal` at `now`.
// trace:STORY-1698 | ai:claude
fn record_problem(m: &Mandate, stem: &str, principal: &str, now: DateTime<Utc>) -> Option<String> {
    if m.id != stem {
        return Some("its id does not match its file name".into());
    }
    if m.seat != MANDATE_SEAT {
        return Some(format!("seat `{}` is not the advisor seat", m.seat));
    }
    if m.scope != SCOPE_SHIP_HELD {
        return Some(format!("unknown scope `{}`", m.scope));
    }
    // trace:STORY-1700 | ai:claude
    if let Some(kind) = m.kind.as_deref().filter(|k| *k != KIND_DRIVE) {
        return Some(format!("unknown kind `{kind}`"));
    }
    if m.principal.trim() != principal.trim() || principal.trim().is_empty() {
        return Some(format!(
            "principal `{}` is not the current user",
            m.principal
        ));
    }
    let ttl = m.expires_at - m.issued_at;
    if ttl <= Duration::zero() || ttl > MAX_TTL {
        return Some("its lifetime exceeds the 24h cap".into());
    }
    if m.issued_at > now + ISSUE_SKEW {
        return Some("it claims to be issued in the future".into());
    }
    None
}

/// Every mandate record in `dir`, validated: `(valid, warnings)`. Valid
/// records carry any tombstone's revocation; invalid ones are only named in
/// a warning, so they never shadow a valid mandate.
// trace:STORY-1698 | ai:claude
pub(crate) fn load_valid(
    dir: &Path,
    principal: &str,
    now: DateTime<Utc>,
) -> (Vec<Mandate>, Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (Vec::new(), Vec::new());
    };
    let mut valid = Vec::new();
    let mut warnings = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let (Some(stem), Some(ext)) = (
            path.file_stem().and_then(|s| s.to_str()),
            path.extension().and_then(|s| s.to_str()),
        ) else {
            continue;
        };
        if ext != "json" || Uuid::parse_str(stem).is_err() {
            continue;
        }
        let name = path.display();
        if let Err(why) = private_file_check(&path) {
            warnings.push(format!("ignoring mandate record {name}: it {why}"));
            continue;
        }
        let mut m: Mandate = match std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
        {
            Ok(m) => m,
            Err(e) => {
                warnings.push(format!("ignoring mandate record {name}: unreadable ({e})"));
                continue;
            }
        };
        if let Some(why) = record_problem(&m, stem, principal, now) {
            warnings.push(format!("ignoring mandate record {name}: {why}"));
            continue;
        }
        if let Some((at, by)) = tombstone(dir, &m.id) {
            if m.revoked_at.is_none() {
                m.revoked_at = Some(at);
                m.revoked_by = by;
            }
        }
        valid.push(m);
    }
    valid.sort_by_key(|m| m.issued_at);
    (valid, warnings)
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

/// The state read from disk now — never cached. `warn` prints the records
/// it ignored to stderr.
// trace:STORY-1698 | ai:claude
pub(crate) fn current_state_reporting(warn: bool) -> MandateState {
    let Ok(dir) = mandate_dir() else {
        return MandateState::None;
    };
    let now = Utc::now();
    let (valid, warnings) = load_valid(&dir, &crate::current_user_id(None), now);
    if warn {
        for w in &warnings {
            eprintln!("  warning: {w}");
        }
    }
    state_at(&valid, now)
}

/// [`current_state_reporting`] without warnings (the statusline).
pub(crate) fn current_state() -> MandateState {
    current_state_reporting(false)
}

/// STORY-1700 (R3): is mandate `id` still the newest active mandate at
/// `now`? Re-reads `dir` (stop tombstones included); nothing is cached. The
/// drive tick calls this immediately before every side effect.
// trace:STORY-1700 | ai:claude
pub(crate) fn recheck(
    dir: &Path,
    id: &str,
    principal: &str,
    now: DateTime<Utc>,
) -> std::result::Result<Mandate, MandateRefusal> {
    let (valid, _) = load_valid(dir, principal, now);
    if let MandateState::Active(m) = state_at(&valid, now) {
        if m.id == id {
            return Ok(m);
        }
    }
    match valid.iter().find(|m| m.id == id) {
        Some(m) if m.revoked_at.is_none() && now >= m.expires_at => {
            Err(MandateRefusal::Expired { id: id.into() })
        }
        // Stopped, superseded, or its record vanished: no longer this mandate.
        _ => Err(MandateRefusal::Stopped { id: id.into() }),
    }
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

/// Mint a mandate. `human` is the terminal floor
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
    start_with(
        dir,
        human,
        principal,
        ttl,
        reason,
        include_supervised,
        false,
        now,
    )
}

/// [`start`], optionally as a STORY-1700 drive mandate: `drive` records
/// `kind = "drive"` and every [`Scope`]. Same floor, cap and no-extension
/// rule.
// trace:STORY-1700 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn start_with(
    dir: &Path,
    human: &dyn Fn() -> Result<()>,
    principal: &str,
    ttl: Duration,
    reason: Option<String>,
    include_supervised: bool,
    drive: bool,
    now: DateTime<Utc>,
) -> Result<Mandate> {
    if let Err(err) = human() {
        bail!("a mandate can only be started by a human at an interactive terminal ({err})");
    }
    if ttl <= Duration::zero() || ttl > MAX_TTL {
        bail!("a mandate lasts more than zero and at most 24h");
    }
    let principal = principal.trim();
    if principal.is_empty() {
        bail!("cannot start a mandate: the human user identity is unknown");
    }
    if let MandateState::Active(active) = state_at(&load_valid(dir, principal, now).0, now) {
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
        scopes: if drive {
            Scope::ALL.into_iter().collect()
        } else {
            BTreeSet::new()
        },
        kind: drive.then(|| KIND_DRIVE.to_string()),
    };
    write_private(
        &dir.join(format!("{}.json", mandate.id)),
        &serde_json::to_vec_pretty(&mandate)?,
    )?;
    Ok(mandate)
}

/// Stop mandates now: writes a never-removed `<id>.stopped` tombstone for
/// EVERY record file in `dir` not already tombstoned (valid or not), so no
/// edit to a record can revive it. Same floor as [`start`]. Returns the
/// valid mandates that were active.
// trace:STORY-1698 | ai:claude
pub(crate) fn stop(
    dir: &Path,
    human: &dyn Fn() -> Result<()>,
    by: &str,
    now: DateTime<Utc>,
) -> Result<Vec<Mandate>> {
    if let Err(err) = human() {
        bail!("a mandate can only be stopped by a human at an interactive terminal ({err})");
    }
    let (valid, _) = load_valid(dir, by, now);
    let active: Vec<Mandate> = valid.into_iter().filter(|m| m.is_active_at(now)).collect();
    let ids: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    let stem = p.file_stem()?.to_str()?.to_string();
                    (p.extension()?.to_str()? == "json" && Uuid::parse_str(&stem).is_ok())
                        .then_some(stem)
                })
                .collect()
        })
        .unwrap_or_default();
    for id in ids {
        let path = tombstone_path(dir, &id);
        if std::fs::symlink_metadata(&path).is_ok() {
            continue;
        }
        let body = serde_json::json!({
            "id": id,
            "stopped_at": now.to_rfc3339(),
            "stopped_by": by.trim(),
        });
        write_private(&path, &serde_json::to_vec_pretty(&body)?)
            .with_context(|| format!("could not write the stop tombstone for mandate {id}"))?;
    }
    Ok(active
        .into_iter()
        .map(|mut m| {
            m.revoked_at = Some(now);
            m.revoked_by = Some(by.trim().to_string());
            m
        })
        .collect())
}

// ── Exclusion ledger ────────────────────────────────────────────────────────

/// One append-only exclusion: once a spec is seen in an excluded class, a
/// mandate never covers it again, whatever its history shows later.
// trace:STORY-1698 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Exclusion {
    pub spec: String,
    pub class: String,
    /// Operator-only or decide: never lifted, even by --include-supervised.
    #[serde(default)]
    pub always_human: bool,
    pub first_seen: DateTime<Utc>,
}

/// The ledger by spec: the strongest exclusion seen for each. `Err` when the
/// ledger exists but cannot be trusted to be complete (not a regular file,
/// unreadable, or a malformed line) — the caller refuses.
// trace:STORY-1698 | ai:claude
pub(crate) fn read_exclusions(
    dir: &Path,
) -> std::result::Result<std::collections::BTreeMap<String, Exclusion>, String> {
    let path = dir.join(EXCLUSION_LEDGER);
    // Validate the opened object; neither a symlink swap nor a FIFO may turn
    // this authority read into an unchecked target or a blocking open.
    // trace:TASK-1790 | ai:codex
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    opened_private_file_check(&file).map_err(|why| format!("{} {why}", path.display()))?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = std::collections::BTreeMap::<String, Exclusion>::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let e: Exclusion = serde_json::from_str(line).map_err(|err| {
            format!(
                "{} line {} is malformed ({err}); fix or remove that line by hand",
                path.display(),
                n + 1
            )
        })?;
        let key = e.spec.to_ascii_uppercase();
        let keep_old = out
            .get(&key)
            .is_some_and(|old| old.always_human || !e.always_human);
        if !keep_old {
            out.insert(key, e);
        }
    }
    Ok(out)
}

// trace:TASK-1790 | ai:codex
fn opened_private_file_check(file: &std::fs::File) -> std::result::Result<(), String> {
    let meta = file.metadata().map_err(|e| format!("unreadable: {e}"))?;
    exclusion_metadata_check(&meta, unsafe { libc::geteuid() })
}

// trace:TASK-1790 | ai:codex
fn exclusion_metadata_check(meta: &std::fs::Metadata, uid: u32) -> std::result::Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    if !meta.is_file() {
        return Err("is not a regular file".into());
    }
    let mode = meta.mode() & 0o7777;
    if mode != 0o600 {
        return Err(format!("has mode {mode:o}, not 600"));
    }
    if meta.uid() != uid {
        return Err(format!("is owned by uid {}, not {uid}", meta.uid()));
    }
    Ok(())
}

/// Append each `(spec, class, always_human)` not already covered by an equal
/// or stronger ledger entry. Mode 0600; never follows a symlink.
// trace:STORY-1698 | ai:claude
pub(crate) fn record_exclusions(
    dir: &Path,
    seen: &[(String, String, bool)],
    now: DateTime<Utc>,
) -> Result<()> {
    use std::io::Write;
    // A read failure may hide sticky exclusions. Never append through it.
    let existing = read_exclusions(dir).map_err(anyhow::Error::msg)?;
    let mut lines = String::new();
    let mut added = std::collections::BTreeSet::new();
    for (spec, class, always_human) in seen {
        let key = spec.to_ascii_uppercase();
        let covered = existing
            .get(&key)
            .is_some_and(|e| e.always_human || !always_human);
        if covered || !added.insert((key.clone(), *always_human)) {
            continue;
        }
        let entry = Exclusion {
            spec: key,
            class: class.clone(),
            always_human: *always_human,
            first_seen: now,
        };
        lines.push_str(&serde_json::to_string(&entry)?);
        lines.push('\n');
    }
    if lines.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let mut opts = std::fs::OpenOptions::new();
    opts.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = opts.open(dir.join(EXCLUSION_LEDGER))?;
    opened_private_file_check(&file).map_err(anyhow::Error::msg)?;
    file.write_all(lines.as_bytes())?;
    Ok(())
}

/// The exclusion a requirement version carries: `(class, always_human)`.
// trace:STORY-1698 | ai:claude
pub(crate) fn exclusion_of(req: &aida_core::Requirement) -> Option<(String, bool)> {
    if let Some(class) = always_human_class(req) {
        return Some((class, true));
    }
    excluded_class(req).map(|why| (why, false))
}

/// Record every exclusion any of `versions` of `spec` carries in the ledger
/// under `dir`. Best-effort: a ledger write failure is warned about, and the
/// git-history check still applies.
// trace:STORY-1698 | ai:claude
pub(crate) fn note_exclusions<'a>(
    dir: &Path,
    spec: &str,
    versions: impl IntoIterator<Item = &'a aida_core::Requirement>,
) {
    let seen: Vec<(String, String, bool)> = versions
        .into_iter()
        .filter_map(exclusion_of)
        .map(|(class, always)| (spec.to_string(), class, always))
        .collect();
    if let Err(err) = record_exclusions(dir, &seen, Utc::now()) {
        eprintln!("  warning: could not record a mandate exclusion for {spec}: {err:#}");
    }
}

// ── Ship decision ───────────────────────────────────────────────────────────

/// What the ship decision knows about one spec the PR ships.
// trace:STORY-1698 | ai:claude
// trace:TASK-1800 | ai:antigravity
#[derive(Debug, Clone)]
pub(crate) struct ShipSpec {
    pub id: String,
    /// Additional ID keys (e.g., an agreed_id alias when `id` is the canonical
    /// spec_id, or vice versa) to consult in the append-only exclusion ledger.
    pub aliases: Vec<String>,
    /// True when this spec was named by the PR/branch/title (fails closed when
    /// absent from the store). False when it was only discovered via a trace
    /// comment in a touched file.
    pub named: bool,
    /// True when the spec's git linkage shows a merged commit on the default
    /// branch (`main`). Used by [`trace_operator_only_expired`] so an agent
    /// cannot expire a trace-derived `operator-only` exclusion merely by
    /// editing the spec's status to `Completed` without a shipped commit.
    // trace:TASK-2314 | ai:agy
    pub shipped: bool,
    /// The spec as the store holds it now; `None` when the store has no such
    /// spec currently.
    pub current: Option<aida_core::Requirement>,
    /// Every earlier version of the spec found in the store's git history
    /// (plus what its recorded field changes say it once was). `Err` = the
    /// history could not be read, which refuses (fails closed): exclusion is
    /// sticky, so a version we cannot see may be the one that excludes it.
    /// `Ok(empty)` means the store git history is readable and holds no version
    /// of this object on any ref.
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

/// One git invocation in the store repository; `Err` carries git's stderr.
// trace:BUG-2179 | ai:claude
fn store_git(
    store_root: &Path,
    args: &[&str],
    stdin: Option<&[u8]>,
) -> std::result::Result<Vec<u8>, String> {
    use std::io::Write;
    let mut cmd = aida_core::git_ops::host_git_command(store_root);
    // TASK-2067 (advisor C1): no system config, and no path quoting, decide
    // what a history walk reports. trace:TASK-2067 | ai:claude
    // TASK-2238: nor do refs/replace/* objects, as for the snapshot queries.
    // trace:TASK-2238 | ai:claude
    cmd.args(["-c", "core.quotePath=false"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(if stdin.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        });
    let Some(input) = stdin else {
        // TASK-1662: the history walks (`git log --all -G …` over the whole
        // store) have a deadline; a timeout is an unreadable history, which
        // the ship path already refuses as HistoryUnreadable.
        // trace:TASK-1951 trace:TASK-1662 | ai:claude
        let budget = store_history_timeout();
        return match crate::command_output_with_timeout_detail(cmd, budget) {
            crate::BoundedCommandOutput::Completed(out) => git_output(out),
            crate::BoundedCommandOutput::SpawnFailed => Err("git failed to run".to_string()),
            // TASK-2141: name the knob, so a cold-cache walk on a large
            // store has a way forward. trace:TASK-1951 trace:TASK-2141 | ai:claude
            crate::BoundedCommandOutput::TimedOut => Err(format!(
                "store history read timed out after {}s; raise {STORE_HISTORY_TIMEOUT_ENV} \
                 (seconds) for a large or cold store",
                budget.as_secs()
            )),
        };
    };
    let mut child = cmd.spawn().map_err(|e| format!("git failed to run: {e}"))?;
    // A writer thread, so a large reply cannot deadlock against the input.
    let mut pipe = child.stdin.take().ok_or("git stdin unavailable")?;
    let input = input.to_vec();
    let writer = std::thread::spawn(move || pipe.write_all(&input));
    let out = child
        .wait_with_output()
        .map_err(|e| format!("git failed: {e}"))?;
    writer
        .join()
        .map_err(|_| "git stdin writer panicked".to_string())?
        .map_err(|e| format!("cannot write to git: {e}"))?;
    git_output(out)
}

const STORE_HISTORY_TIMEOUT_ENV: &str = "AIDA_STORE_HISTORY_TIMEOUT_SECS";

/// The deadline for one store-history git walk:
/// `AIDA_STORE_HISTORY_TIMEOUT_SECS`, else 600 s. TASK-2141: the default sits
/// above the measured cold walk of the live store (4m26s, TASK-1662), so a
/// cold cache does not refuse a ship; the deadline still bounds a hung walk.
// trace:TASK-1951 trace:TASK-1662 trace:TASK-2141 | ai:claude
fn store_history_timeout() -> std::time::Duration {
    let secs = std::env::var(STORE_HISTORY_TIMEOUT_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(600);
    std::time::Duration::from_secs(secs)
}

fn git_output(out: std::process::Output) -> std::result::Result<Vec<u8>, String> {
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// BUG-2179: the store history the mandate exclusion check reads, gathered in
/// a bounded number of git invocations however many specs a PR traces: one
/// `git log --all` walk over every spec's object path, one `git cat-file
/// --batch` for every stored version, and (only when some ID has no current
/// record) one `-G` walk for the aliases. Strictness is the per-spec walks':
/// every ref (`--all`), every version that added or changed the object,
/// and fail-closed per spec when a version cannot be read.
// trace:BUG-2179 | ai:claude
#[derive(Debug, Default)]
pub(crate) struct StoreHistoryBatch {
    /// Upper-cased spec ID → every stored version, each expanded by its
    /// recorded field changes; `Err` when one of its versions is unreadable.
    versions: std::collections::BTreeMap<
        String,
        std::result::Result<Vec<aida_core::Requirement>, String>,
    >,
}

impl StoreHistoryBatch {
    /// Every stored version of `spec_id` (`Ok(empty)` when no ref ever held it).
    pub(crate) fn versions(
        &self,
        spec_id: &str,
    ) -> std::result::Result<Vec<aida_core::Requirement>, String> {
        self.versions
            .get(&spec_id.to_ascii_uppercase())
            .cloned()
            .unwrap_or_else(|| Err(format!("{spec_id} was not part of the history read")))
    }

    /// Read every version of each of `spec_ids`. `Err` when the store history
    /// cannot be walked at all.
    pub(crate) fn read(
        store_root: &Path,
        spec_ids: &[String],
    ) -> std::result::Result<Self, String> {
        let mut batch = Self::default();
        let mut paths: Vec<(String, String)> = Vec::new();
        for id in spec_ids {
            let key = id.to_ascii_uppercase();
            if batch.versions.contains_key(&key) {
                continue;
            }
            match aida_core::object_store::relative_object_path(id) {
                Ok(rel) => {
                    batch.versions.insert(key.clone(), Ok(Vec::new()));
                    paths.push((key, rel));
                }
                Err(e) => {
                    batch
                        .versions
                        .insert(key, Err(format!("no object path for {id}: {e}")));
                }
            }
        }
        if paths.is_empty() {
            return Ok(batch);
        }
        // Deletions carry no body to read; every other touching commit does.
        // The walk is limited by each path's top directory, not by every path:
        // git checks each pathspec against every commit, so a PR tracing ~100
        // specs took ~50 s over a ~50k-commit store, while one directory
        // pathspec takes seconds whatever the spec count. The names it lists
        // are filtered to the wanted paths below.
        let mut tops: Vec<&str> = paths
            .iter()
            .map(|(_, rel)| rel.split_once('/').map_or(rel.as_str(), |(top, _)| top))
            .collect();
        tops.sort_unstable();
        tops.dedup();
        // TASK-2067 (advisor C1): renames off explicitly, so the listed names
        // never depend on diff.renames or diff.renameLimit; a renamed-to path
        // is listed as an addition. trace:TASK-2067 | ai:claude
        let mut args: Vec<&str> = vec![
            "log",
            "--all",
            "--format=%x00%H",
            "--no-renames",
            "--diff-filter=ACMRT",
            "--name-only",
            "--",
        ];
        args.extend(tops);
        let log = store_git(store_root, &args, None)?;
        let by_path: std::collections::BTreeMap<&str, &str> = paths
            .iter()
            .map(|(key, rel)| (rel.as_str(), key.as_str()))
            .collect();
        // `<sha>:<path>` per version, in log order (newest first).
        let mut wanted: Vec<(String, String)> = Vec::new();
        for record in String::from_utf8_lossy(&log).split('\0').skip(1) {
            let mut lines = record.lines().map(str::trim).filter(|l| !l.is_empty());
            let Some(sha) = lines.next() else { continue };
            for file in lines {
                if let Some(key) = by_path.get(file) {
                    wanted.push((key.to_string(), format!("{sha}:{file}")));
                }
            }
        }
        if wanted.is_empty() {
            return Ok(batch);
        }
        let input: String = wanted.iter().map(|(_, obj)| format!("{obj}\n")).collect();
        let bodies = store_git(store_root, &["cat-file", "--batch"], Some(input.as_bytes()))?;
        let mut rest: &[u8] = &bodies;
        for (key, obj) in &wanted {
            let (body, tail) = next_batch_object(rest)
                .map_err(|e| format!("cannot read {obj} from the store: {e}"))?;
            rest = tail;
            let Some(Ok(versions)) = batch.versions.get_mut(key) else {
                continue;
            };
            let spec = obj.split_once(':').map_or(obj.as_str(), |(sha, _)| sha);
            let parsed = body
                .ok_or_else(|| format!("cannot read {key} at {spec}: missing object"))
                .and_then(|text| {
                    parse_version(&String::from_utf8_lossy(text))
                        .map_err(|e| format!("{key} at {spec}: {e}"))
                });
            match parsed {
                Ok(req) => {
                    versions.extend(field_change_versions(&req));
                    versions.push(req);
                }
                Err(e) => {
                    batch.versions.insert(key.clone(), Err(e));
                }
            }
        }
        Ok(batch)
    }
}

/// One `git cat-file --batch` reply: `(Some(body), rest)` for a found object,
/// `(None, rest)` for a missing one.
// trace:BUG-2179 | ai:claude
fn next_batch_object(out: &[u8]) -> std::result::Result<(Option<&[u8]>, &[u8]), String> {
    let nl = out
        .iter()
        .position(|&b| b == b'\n')
        .ok_or("truncated git cat-file reply")?;
    let header = String::from_utf8_lossy(&out[..nl]);
    let rest = &out[nl + 1..];
    if header.ends_with(" missing") || header.ends_with(" ambiguous") {
        return Ok((None, rest));
    }
    let size: usize = header
        .rsplit(' ')
        .next()
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| format!("unexpected git cat-file header {header:?}"))?;
    if rest.len() < size + 1 {
        return Err("truncated git cat-file body".into());
    }
    Ok((Some(&rest[..size]), &rest[size + 1..]))
}

/// STORY-1698 / TASK-1800: every version of `spec_id` in the store's git
/// history (all refs, so a rewritten local branch still shows what the remote
/// saw), each expanded by its recorded field changes. `Ok(empty)` when the
/// store repository is readable and no ref ever held `spec_id`; `Err` when
/// git history cannot be read or a stored version cannot be parsed.
// trace:STORY-1698 | ai:claude
// trace:TASK-1800 | ai:antigravity
pub(crate) fn store_history_versions_optional(
    store_root: &Path,
    spec_id: &str,
) -> std::result::Result<Vec<aida_core::Requirement>, String> {
    StoreHistoryBatch::read(store_root, &[spec_id.to_string()])?.versions(spec_id)
}

/// TASK-1800 / BUG-2179: for each of `ids`, the stored object IDs whose YAML
/// carried it as its `spec_id` or `agreed_id` in any version on any ref, other
/// than the ID itself. This is how a DELETED spec named by its agreed_id
/// (stored under a different spec_id path) is still found and judged. One
/// `-G` walk serves every ID; each matching added or removed line is
/// attributed to the ID it names. `Err` when an ID is not searchable, the
/// history cannot be searched, or a match is in a file that is not at its
/// canonical object path (whose versions the history read cannot find); the
/// caller refuses rather than treating an ID as dangling.
// trace:TASK-1800 | ai:claude
// trace:BUG-2179 | ai:claude
pub(crate) fn store_history_alias_map(
    store_root: &Path,
    ids: &[String],
) -> std::result::Result<std::collections::BTreeMap<String, Vec<String>>, String> {
    let mut found: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for id in ids {
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!("cannot search store history for {id:?}"));
        }
        found.insert(id.to_ascii_uppercase(), Vec::new());
    }
    if found.is_empty() {
        return Ok(found);
    }
    let alternation = found.keys().cloned().collect::<Vec<_>>().join("|");
    let pattern = format!("^(spec_id|agreed_id): *['\"]?({alternation})['\"]? *$");
    let line_re = regex::RegexBuilder::new(&pattern)
        .case_insensitive(true)
        .build()
        .map_err(|e| format!("cannot search store history: {e}"))?;
    let out = store_git(
        store_root,
        &[
            "log",
            "--all",
            "--format=",
            "-p",
            "--unified=0",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--no-show-signature",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--find-renames",
            "-l0",
            "--regexp-ignore-case",
            &format!("-G{pattern}"),
            "--",
            "objects/",
        ],
        None,
    )
    .map_err(|e| format!("cannot search store history for {}: {e}", ids.join(", ")))?;
    // TASK-2067: the (old, new) paths of the file pair being read. Renames
    // are detected explicitly (`--find-renames -l0` above), whatever the
    // user's diff.renames or diff.renameLimit, so a renumbered object is one
    // pair: a removed line belongs to the old path, an added line to the new
    // one, and both stems of a rename are the same spec.
    // trace:TASK-2067 | ai:claude
    let mut pair: Option<(String, String)> = None;
    for line in String::from_utf8_lossy(&out).lines() {
        if let Some(paths) = line.strip_prefix("diff --git ") {
            pair = paths
                .strip_prefix("a/")
                .and_then(|paths| paths.rsplit_once(" b/"))
                .map(|(a, b)| (a.to_string(), b.to_string()));
            continue;
        }
        if let (Some(from), Some((old, _))) = (line.strip_prefix("rename from "), pair.as_mut()) {
            *old = from.to_string();
            continue;
        }
        if let (Some(to), Some((_, new))) = (line.strip_prefix("rename to "), pair.as_mut()) {
            *new = to.to_string();
            continue;
        }
        if line.starts_with("+++ ") || line.starts_with("--- ") {
            continue;
        }
        let (body, removed) = match (line.strip_prefix('-'), line.strip_prefix('+')) {
            (Some(body), _) => (body, true),
            (None, Some(body)) => (body, false),
            (None, None) => continue,
        };
        let Some(caps) = line_re.captures(body) else {
            continue;
        };
        let (hit, other) = match &pair {
            Some((old, new)) if removed => (Some(old.as_str()), Some(new.as_str())),
            Some((old, new)) => (Some(new.as_str()), Some(old.as_str())),
            None => (None, None),
        };
        // TASK-1800 (comment 24): the history read below finds versions only
        // at an object's canonical path. A hit anywhere else would read as no
        // history and drop the ID as dangling, so it refuses instead.
        // trace:TASK-1800 | ai:claude
        let stem = hit.and_then(canonical_object_stem).ok_or_else(|| {
            format!(
                "store history carries {} outside a canonical object path ({})",
                &caps[2],
                hit.unwrap_or("no file")
            )
        })?;
        let id = caps[2].to_ascii_uppercase();
        // The other side of a rename only adds versions to judge. TASK-2238:
        // a non-canonical other side would be versions never judged, so it
        // refuses like a non-canonical hit rather than being dropped.
        // trace:TASK-2238 | ai:claude
        let renamed = match other.filter(|other| Some(*other) != hit) {
            None => None,
            Some(other) => Some(canonical_object_stem(other).ok_or_else(|| {
                format!(
                    "store history renames {} to or from a non-canonical object path ({other})",
                    &caps[2]
                )
            })?),
        };
        if let Some(aliases) = found.get_mut(&id) {
            for stem in std::iter::once(stem).chain(renamed) {
                let stem = stem.to_ascii_uppercase();
                if stem != id && !aliases.contains(&stem) {
                    aliases.push(stem);
                }
            }
        }
    }
    Ok(found)
}

/// The object ID whose canonical object path is `path`, if it is one.
// trace:TASK-1800 trace:TASK-2067 | ai:claude
fn canonical_object_stem(path: &str) -> Option<&str> {
    let stem = Path::new(path)
        .file_name()?
        .to_str()?
        .strip_suffix(".yaml")?;
    let canonical = aida_core::object_store::relative_object_path(stem).ok()?;
    (canonical == path).then_some(stem)
}

/// TASK-1800: [`store_history_alias_map`] for one ID.
// trace:TASK-1800 | ai:claude
#[cfg(test)]
pub(crate) fn store_history_alias_ids(
    store_root: &Path,
    id: &str,
) -> std::result::Result<Vec<String>, String> {
    let mut map = store_history_alias_map(store_root, &[id.to_string()])?;
    Ok(map.remove(&id.to_ascii_uppercase()).unwrap_or_default())
}

/// STORY-1698: every version of `spec_id` in the store's git history (all
/// refs, so a rewritten local branch still shows what the remote saw), each
/// expanded by its recorded field changes. `Err` when the history cannot be
/// read or holds no version at all — the caller refuses.
// trace:STORY-1698 | ai:claude
#[cfg(test)]
pub(crate) fn store_history_versions(
    store_root: &Path,
    spec_id: &str,
) -> std::result::Result<Vec<aida_core::Requirement>, String> {
    let out = store_history_versions_optional(store_root, spec_id)?;
    if out.is_empty() {
        return Err(format!("no version of {spec_id} in the store's history"));
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
    /// The append-only exclusion ledger; `Err` = untrustworthy (refuses).
    pub ledger: &'a std::result::Result<std::collections::BTreeMap<String, Exclusion>, String>,
    /// The PR's merge-hold as the release step will see it.
    pub hold: &'a HoldFacts,
    /// STORY-1700 (R1): every path the PR changes at the head it ships;
    /// `None` = they could not be resolved (refuses). Read only by
    /// [`ship_decision`].
    // trace:STORY-1700 | ai:claude
    pub changed_paths: Option<&'a [String]>,
    /// STORY-1888: the effective protected set (floor plus both config
    /// layers; the floor alone when the layers are unreadable).
    pub protected: &'a crate::mandate_gates::ProtectedPaths,
    /// STORY-1700: why `ship` is a human gate right now (listed in
    /// `[mandate.human_gates]`, or the gate config is unreadable); `None` =
    /// not gated. Read by [`ship_decision`] and [`rework_release_decision`].
    pub ship_gate: Option<String>,
    /// STORY-1700: why `hold-clear` is a human gate right now; `None` = not
    /// gated. Read by [`rework_release_decision`].
    pub hold_clear_gate: Option<String>,
    /// TASK-1960: the attested-approval carry-over for a moved head, proven
    /// by the caller (`pr_cmd::mandate_carry_over`): `Ok(head)` = proven for
    /// that head, `Err(reason)` = the proof failed, `None` = not attempted.
    pub carry_over: Option<std::result::Result<String, String>>,
}

/// STORY-1700 (R1): the gate checks every mandate path runs before hold
/// cover. A PR that touches the mandate or its gates, or whose files cannot
/// be resolved, is never shipped nor released under a mandate; nor is any PR
/// while one of `gates` (a human gate in force) names why.
// trace:STORY-1700 | ai:claude
fn gate_checks(
    c: &ShipCheck<'_>,
    gates: &[&Option<String>],
) -> std::result::Result<(), MandateRefusal> {
    let paths = c
        .changed_paths
        .ok_or(MandateRefusal::ChangedPathsUnresolved)?;
    // trace:STORY-1888 | ai:claude
    if let Some(hit) = crate::mandate_gates::touches_gates(paths, c.protected) {
        return Err(MandateRefusal::TouchesGates(hit));
    }
    if let Some(why) = gates.iter().find_map(|g| g.as_ref()) {
        return Err(MandateRefusal::HumanGated { why: why.clone() });
    }
    Ok(())
}

/// STORY-1698: the merge-hold a mandate would release. A mandate only ever
/// releases a typed marker of a releasable kind.
// trace:STORY-1698 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HoldFacts {
    /// No marker at all.
    None,
    /// AIDA recorded placing a hold whose marker vanished without a clearance.
    MarkerMissing,
    /// BUG-2214: no marker because a human already cleared the hold, proven by
    /// three independent corroborations (the local clearance record, a
    /// TTY-attested human release event newer than the last placement, and the
    /// forge's label state). The mandate has nothing left to release; the
    /// cover treats the PR as held-and-released rather than never held.
    // trace:BUG-2214 | ai:claude
    ClearedByHuman,
    Marker {
        kind: crate::merge_hold::HoldReasonKind,
        legacy: bool,
    },
}

impl HoldFacts {
    pub(crate) fn of(
        record: Option<&crate::merge_hold::MergeHoldRecord>,
        reason: Option<&str>,
    ) -> Self {
        match record {
            Some(r) => Self::Marker {
                kind: r.reason_kind,
                legacy: r.legacy,
            },
            None if reason == Some(crate::merge_hold::MARKER_MISSING_REASON) => Self::MarkerMissing,
            None => Self::None,
        }
    }
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
    // trace:TASK-1790 | ai:codex
    TraceEvidenceUnreadable(String),
    AlwaysHuman { spec: String, class: String },
    LedgerUnreadable(String),
    NoHoldMarker,
    MarkerMissing,
    HoldNotReleasable { kind: String },
    HistoryUnreadable { spec: String, why: String },
    Excluded { spec: String, why: String },
    ShipperUnanchored,
    AdvisorAuthored { author: String },
    ApproverIsShipper,
    Approval(String),
    StaleApproval(String),
    SupervisedNotIncluded { id: String }, // trace:STORY-1697 | ai:claude
    // trace:STORY-1700 | ai:claude
    ChangedPathsUnresolved,
    TouchesGates(crate::mandate_gates::GateMatch), // trace:STORY-1888 | ai:claude
    HumanGated { why: String },
    SoloSelfApproval,
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
            Self::TraceEvidenceUnreadable(why) => format!(
                "the PR's mandate exclusion evidence could not be read ({why})"
            ),
            Self::HistoryUnreadable { spec, why } => format!(
                "{spec}'s store history could not be read ({why}), so it is treated as excluded"
            ),
            Self::AlwaysHuman { spec, class } => {
                format!("{spec} is {class}; it always needs a human merge")
            }
            Self::LedgerUnreadable(why) => {
                format!("the mandate exclusion ledger cannot be trusted: {why}")
            }
            Self::NoHoldMarker => {
                "the PR has no typed merge-hold marker for a mandate to release; a human must ship it".to_string()
            }
            Self::MarkerMissing => format!(
                "the PR's merge-hold marker is missing without a recorded clearance ({}); a human must run `aida merge-hold clear`",
                crate::merge_hold::MARKER_MISSING_REASON
            ),
            Self::HoldNotReleasable { kind } => {
                format!("a {kind} hold is never released under a mandate; a human must clear it")
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
            Self::SupervisedNotIncluded { id } => format!(
                "mandate {id} was not minted with --include-supervised, so it does not cover sensitive specs"
            ),
            Self::ChangedPathsUnresolved => {
                "the files the PR changes could not be resolved, so it may touch the mandate's own gates".to_string()
            }
            Self::TouchesGates(hit) => format!(
                "it changes {}, part of the mandate and its human gates; a human must merge it",
                hit.describe()
            ),
            Self::HumanGated { why } => format!("shipping is a human gate ({why})"),
            Self::SoloSelfApproval => "the approval is a solo-human self-approval, not an independent review; a human must ship it".to_string(),
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

/// STORY-1698: always-human specs — `execution_mode` operator or decide, or
/// the `operator-only` tag. Never covered, whatever the mandate's flags.
/// `Some(class)` names which.
// trace:STORY-1698 | ai:claude
pub(crate) fn always_human_class(req: &aida_core::Requirement) -> Option<String> {
    match req.execution_mode {
        Some(aida_core::ExecutionMode::Operator) => return Some("operator-only".into()),
        Some(aida_core::ExecutionMode::Decide) => return Some("decide-mode".into()),
        _ => {}
    }
    req.tags
        .iter()
        .any(|t| t.trim().eq_ignore_ascii_case(OPERATOR_ONLY_TAG))
        .then(|| "operator-only".into())
}

/// TASK-2168 / TASK-2314: a trace-derived `operator-only` exclusion stops
/// applying ONLY once the traced spec's current status is `Completed` AND its
/// linkage shows a merged commit on the default branch (`shipped`). `Rejected`
/// or `Superseded`, `Completed` with no shipped commit, exclusions named
/// directly on the PR's specs, and unreadable current status (`current: None`)
/// keep the exclusion (fail closed).
// trace:TASK-2168 trace:TASK-2314 | ai:agy
pub(crate) fn trace_operator_only_expired(spec: &ShipSpec) -> bool {
    !spec.named
        && spec.shipped
        && spec
            .current
            .as_ref()
            .is_some_and(|req| matches!(req.status, aida_core::RequirementStatus::Completed))
}

// trace:TASK-2168 | ai:agy
fn excluded_class_for(req: &aida_core::Requirement, ignore_operator_mode: bool) -> Option<String> {
    if ignore_operator_mode && req.execution_mode == Some(aida_core::ExecutionMode::Operator) {
        let mut without_operator = req.clone();
        without_operator.execution_mode = None;
        return excluded_class(&without_operator);
    }
    excluded_class(req)
}

/// STORY-1698: does an active mandate cover this ship? `Ok(mandate id)` =
/// the advisor may release the hold and merge; `Err` = it may not. Pure:
/// every input is gathered by the caller, at the instant of the decision.
// trace:STORY-1698 | ai:claude
pub(crate) fn ship_decision(c: &ShipCheck<'_>) -> std::result::Result<String, MandateRefusal> {
    // STORY-1700 (R1): checked before hold cover, on every mandate ship
    // path. trace:STORY-1700 | ai:claude
    gate_checks(c, &[&c.ship_gate])?;
    let mandate = hold_cover(c)?;
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
    approver_is_not_shipper(c)?;
    if let Some(refusal) = crate::pr_ship::approval_head_refusal(c.candidates, c.head) {
        // TASK-1960: a head moved only by clean merges of the protected base
        // keeps its approval here exactly as it does at the merge gate.
        // trace:TASK-1960 | ai:claude
        let carried = match (&refusal, &c.carry_over) {
            (crate::pr_ship::ApprovalHeadRefusal::Moved { head_sha, .. }, Some(Ok(proven))) => {
                !head_sha.trim().is_empty() && proven.eq_ignore_ascii_case(head_sha.trim())
            }
            _ => false,
        };
        if !carried {
            let mut message = crate::pr_ship::approval_head_refusal_message(0, &refusal);
            if let Some(Err(why)) = &c.carry_over {
                message = format!("{message} The approval did not carry over: {why}.");
            }
            return Err(MandateRefusal::StaleApproval(message));
        }
    }
    Ok(mandate.id.clone())
}

/// The two-person rule: the newest open approval was not recorded by — and
/// its re-validated authority does not resolve to — the session acting under
/// the mandate. Shared by [`ship_decision`] and [`rework_release_decision`]:
/// an advisor never ships, nor releases a hold, on its own approval.
// trace:STORY-1698 | ai:claude
// trace:STORY-1697 | ai:claude
fn approver_is_not_shipper(c: &ShipCheck<'_>) -> std::result::Result<(), MandateRefusal> {
    let shipper = &c.policy.shipper;
    let approvals: Vec<_> = c
        .candidates
        .iter()
        .filter(|v| v.kind.approves() && !v.is_closed())
        .cloned()
        .collect();
    let Some(newest) = crate::pr_ship::newest_approval(&approvals) else {
        return Ok(());
    };
    // STORY-1700: a STORY-1655 solo-human SELF-approval is not the
    // independent review a mandate needs: it may clear the author check for
    // a human shipping by hand, never a mandate ship or release.
    // trace:STORY-1700 | ai:claude
    if crate::pr_ship::solo_self_approval(newest, c.policy) {
        return Err(MandateRefusal::SoloSelfApproval);
    }
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
    Ok(())
}

/// STORY-1697: may an active mandate release a REWORK hold on a sensitive
/// spec when its advisor records an approval? The same mandate, seat, hold
/// and sticky spec-class checks as [`ship_decision`], restricted to a typed
/// rework hold and to a mandate minted with `--include-supervised`.
/// Operator-only and decide-mode specs are never covered, and the session
/// releasing must not be the approval's recorder (the same two-person rule
/// as [`ship_decision`]). The approval's own authority is judged by the
/// caller (`rework_hold_release`), which also owns the head check — this
/// releases a hold, it does not merge.
// trace:STORY-1697 | ai:claude
pub(crate) fn rework_release_decision(
    c: &ShipCheck<'_>,
) -> std::result::Result<String, MandateRefusal> {
    match c.hold {
        HoldFacts::Marker {
            kind: crate::merge_hold::HoldReasonKind::Rework,
            legacy: false,
        } => {}
        HoldFacts::Marker { kind, legacy } => {
            return Err(MandateRefusal::HoldNotReleasable {
                kind: format!(
                    "{}{} (an approval releases only a typed rework hold)",
                    if *legacy { "untyped legacy " } else { "" },
                    kind.as_str()
                ),
            })
        }
        HoldFacts::None => return Err(MandateRefusal::NoHoldMarker),
        HoldFacts::MarkerMissing => return Err(MandateRefusal::MarkerMissing),
        // BUG-2214: an approval releases a live rework hold; a hold a human
        // already cleared leaves nothing for this path to release.
        // trace:BUG-2214 | ai:claude
        HoldFacts::ClearedByHuman => return Err(MandateRefusal::NoHoldMarker),
    }
    // STORY-1700 (R1): a released rework hold lets the PR ship, so the
    // release takes the same gate checks as a mandate ship, plus hold-clear.
    // trace:STORY-1700 | ai:claude
    gate_checks(c, &[&c.ship_gate, &c.hold_clear_gate])?;
    let mandate = hold_cover(c)?;
    if !mandate.include_supervised {
        return Err(MandateRefusal::SupervisedNotIncluded {
            id: mandate.id.clone(),
        });
    }
    // STORY-1698's two-person rule holds for a release too: the advisor that
    // recorded the approval cannot be the one its mandate releases the hold
    // for. An unanchored releasing session cannot be told apart from the
    // approver, so it is refused. trace:STORY-1697 | ai:claude
    if !crate::review_authority::has_identity_anchor(&c.policy.shipper) {
        return Err(MandateRefusal::ShipperUnanchored);
    }
    approver_is_not_shipper(c)?;
    Ok(mandate.id.clone())
}

/// STORY-1698: the half of [`ship_decision`] that judges the mandate, the
/// seat, the hold and the specs — everything but the shipping session and
/// its approval. Shared with [`rework_release_decision`] (STORY-1697).
// trace:STORY-1698 | ai:claude
// trace:TASK-1800 | ai:antigravity
fn hold_cover<'a>(c: &ShipCheck<'a>) -> std::result::Result<&'a Mandate, MandateRefusal> {
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
    // A mandate releases only a typed marker of a releasable kind.
    // trace:STORY-1698 | ai:claude
    match c.hold {
        HoldFacts::None => return Err(MandateRefusal::NoHoldMarker),
        HoldFacts::MarkerMissing => return Err(MandateRefusal::MarkerMissing),
        // BUG-2214: a hold a human already cleared (triply corroborated by the
        // caller) leaves nothing to release; every other check — sticky
        // exclusions, always-human classes, approval at head, the two-person
        // rules — still runs below and in `ship_decision`.
        // trace:BUG-2214 | ai:claude
        HoldFacts::ClearedByHuman => {}
        HoldFacts::Marker { legacy: true, .. } => {
            return Err(MandateRefusal::HoldNotReleasable {
                kind: "untyped legacy".into(),
            })
        }
        HoldFacts::Marker { kind, .. } => {
            use crate::merge_hold::HoldReasonKind as K;
            if !matches!(kind, K::Supervision | K::Decision | K::Rework) {
                return Err(MandateRefusal::HoldNotReleasable {
                    kind: kind.as_str().to_string(),
                });
            }
        }
    }
    if !c.specs_resolved || !c.specs.iter().any(|s| s.named) {
        return Err(MandateRefusal::SpecsUnresolved);
    }
    let ledger = c
        .ledger
        .as_ref()
        .map_err(|why| MandateRefusal::LedgerUnreadable(why.clone()))?;
    // Exclusion is STICKY: a spec is judged by every version it ever had —
    // now, in the store's git history, and in its recorded field changes —
    // so stripping a tag never lifts it. Unreadable history refuses.
    // TASK-1800: a trace-derived ID (never a named ID) with no current version,
    // no store-history version on any ref, and no ledger entry is a dangling
    // reference and is dropped. Any history version (deleted spec) is still
    // judged by that history; named IDs keep fail-closed behaviour.
    // trace:STORY-1698 | ai:claude
    // trace:TASK-1800 | ai:antigravity
    for spec in c.specs {
        judge_exclusion_spec(spec, ledger, mandate.include_supervised)?;
    }
    Ok(mandate)
}

/// One exclusion subject under the per-spec rules [`hold_cover`] applies to
/// every spec: fail closed on a missing named spec or unreadable history, the
/// ledger first, then every version; a dangling trace-derived ID passes.
/// Shared with the TASK-1800 replay (TASK-2067).
// trace:STORY-1698 | ai:claude
// trace:TASK-1800 | ai:antigravity
// trace:TASK-2067 | ai:claude
pub(crate) fn judge_exclusion_spec(
    spec: &ShipSpec,
    ledger: &std::collections::BTreeMap<String, Exclusion>,
    include_supervised: bool,
) -> std::result::Result<(), MandateRefusal> {
    let id = &spec.id;
    if spec.named && spec.current.is_none() {
        return Err(MandateRefusal::SpecsUnresolved);
    }
    let history = match &spec.history {
        Ok(history) => history,
        Err(why) => {
            return Err(MandateRefusal::HistoryUnreadable {
                spec: id.clone(),
                why: why.clone(),
            })
        }
    };
    if spec.named && history.is_empty() {
        return Err(MandateRefusal::HistoryUnreadable {
            spec: id.clone(),
            why: format!("no version of {id} in the store's history"),
        });
    }
    // The ledger is the first source: once seen excluded, always excluded.
    // Consult both the ID and any recorded aliases.
    // TASK-2168 / TASK-2314: a trace-derived operator-only exclusion expires
    // only once the traced spec is Completed AND shipped on the default branch
    // (fail closed when current status is unreadable or unshipped).
    // trace:TASK-2168 trace:TASK-2314 | ai:agy
    let operator_only_expired = trace_operator_only_expired(spec);
    let mut excluded: Option<String> = None;
    for key in std::iter::once(id).chain(spec.aliases.iter()) {
        if let Some(entry) = ledger.get(&key.to_ascii_uppercase()) {
            if entry.always_human {
                if !(operator_only_expired && entry.class == "operator-only") {
                    return Err(MandateRefusal::AlwaysHuman {
                        spec: id.clone(),
                        class: entry.class.clone(),
                    });
                }
            } else if excluded.is_none() {
                excluded = Some(format!(
                    "{}, recorded {}",
                    entry.class,
                    entry.first_seen.to_rfc3339()
                ));
            }
        }
    }
    if !spec.named && spec.current.is_none() && history.is_empty() && excluded.is_none() {
        // Dangling trace-derived reference: no current version, no store
        // history on any ref, and no exclusion ledger entry.
        return Ok(());
    }
    let current_changes = spec
        .current
        .as_ref()
        .map(field_change_versions)
        .unwrap_or_default();
    let versions = spec
        .current
        .iter()
        .chain(current_changes.iter())
        .chain(history.iter());
    for (i, version) in versions.enumerate() {
        if let Some(class) = always_human_class(version) {
            if !(operator_only_expired && class == "operator-only") {
                return Err(MandateRefusal::AlwaysHuman {
                    spec: id.clone(),
                    class,
                });
            }
        }
        if excluded.is_none() {
            excluded = excluded_class_for(version, operator_only_expired).map(|why| {
                if i == 0 && spec.current.is_some() {
                    why
                } else {
                    format!("{why}, in an earlier version")
                }
            });
        }
    }
    if let (Some(why), false) = (excluded, include_supervised) {
        return Err(MandateRefusal::Excluded {
            spec: id.clone(),
            why,
        });
    }
    Ok(())
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

/// STORY-1700: the one-line mandate display shared by `aida mandate status
/// --line`, SessionStart, the per-turn notice and `aida session handoff
/// --show`: `MANDATE drive · 5h40m left · by joe · reason · gates: none`.
/// `None` unless a mandate is active. `gates` describes the human gates.
// trace:STORY-1700 | ai:claude
pub(crate) fn status_line(state: &MandateState, now: DateTime<Utc>, gates: &str) -> Option<String> {
    let MandateState::Active(m) = state else {
        return None;
    };
    if !m.is_active_at(now) {
        return None;
    }
    let mut parts = vec![
        format!(
            "MANDATE {}",
            if m.is_drive() {
                KIND_DRIVE
            } else {
                SCOPE_SHIP_HELD
            }
        ),
        format!(
            "{} left",
            crate::seat_authority::format_remaining(m.expires_at - now)
        ),
        format!("by {}", m.principal),
    ];
    if let Some(reason) = &m.reason {
        parts.push(reason.clone());
    }
    parts.push(format!("gates: {gates}"));
    Some(parts.join(" · "))
}

/// The [`status_line`] for the mandates in `dir` right now, with the human
/// gates of `audit_root` — what `aida mandate status --line` prints.
// trace:TASK-1955 | ai:claude (TASK-1806)
pub(crate) fn current_status_line(
    dir: &Path,
    audit_root: Option<&Path>,
    now: DateTime<Utc>,
) -> Option<String> {
    let (valid, _) = load_valid(dir, &crate::current_user_id(None), now);
    let state = state_at(&valid, now);
    status_line(&state, now, &crate::mandate_gates::describe(audit_root))
}

/// STORY-1700: the per-turn notice for `seat`: the [`status_line`] while a
/// mandate is active, and ONE line per seat when it expires or is stopped
/// (marker `<dir>/.seen/<id>.<seat>`, written here). `None` otherwise.
// trace:STORY-1700 | ai:claude
pub(crate) fn notice_line(
    dir: &Path,
    state: &MandateState,
    now: DateTime<Utc>,
    seat: &str,
    gates: &str,
) -> Option<String> {
    let (m, how) = match state {
        MandateState::Active(_) => return status_line(state, now, gates),
        MandateState::None => return None,
        MandateState::Expired(m) => (m, "expired"),
        MandateState::Stopped(m) => (m, "was stopped"),
    };
    // Only a recent end is news; a seat started days later is not told.
    let ended = m.revoked_at.unwrap_or(m.expires_at).min(m.expires_at);
    if now - ended > MAX_TTL {
        return None;
    }
    let seat: String = seat
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let marker = dir.join(".seen").join(format!("{}.{seat}", m.id));
    if std::fs::symlink_metadata(&marker).is_ok() {
        return None;
    }
    write_private(&marker, now.to_rfc3339().as_bytes()).ok()?;
    Some(format!(
        "MANDATE {} {how}; the advisor seat no longer acts under it (a human starts a new one with `aida mandate start`)",
        m.id
    ))
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
            if m.is_drive() {
                format!(
                    "  covers:    drive ({}): the advisor seat ships held PRs, signs off sketches and dispatches fixers; the 15m drive tick routes stalled lanes",
                    m.effective_scopes()
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                "  covers:    the advisor seat shipping held PRs".to_string()
            },
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

/// STORY-1700: `--include-supervised` is ON by default for `--drive`.
// trace:STORY-1700 | ai:claude
pub(crate) fn drive_includes_supervised(drive: bool, include: bool, exclude: bool) -> bool {
    if drive {
        !exclude
    } else {
        include
    }
}

fn state_mandate(state: &MandateState) -> Option<&Mandate> {
    match state {
        MandateState::None => None,
        MandateState::Active(m) | MandateState::Expired(m) | MandateState::Stopped(m) => Some(m),
    }
}

// Preserve the shared direct-human authority check, while giving mandate
// callers a relevant recovery command instead of role-grant wording.
// trace:TASK-1790 | ai:codex
fn require_mandate_human() -> Result<()> {
    crate::seat_authority::require_direct_human().map_err(|_| anyhow::anyhow!(
        "mandates require a direct human session at an interactive TTY; run `aida mandate start` or `aida mandate stop` from your terminal"
    ))
}

/// `aida mandate start|stop|status`.
// trace:STORY-1698 | ai:claude
pub(crate) fn handle_mandate(action: &crate::cli::MandateAction) -> Result<()> {
    let dir = mandate_dir()?;
    let audit_root = crate::find_main_worktree_root().ok();
    let human = require_mandate_human;
    match action {
        crate::cli::MandateAction::Start {
            ship_held,
            drive,
            ttl,
            reason,
            include_supervised,
            exclude_supervised,
        } => {
            if !ship_held && !drive {
                bail!("say what the mandate covers: `aida mandate start --ship-held`, or `--drive` for a drive mandate");
            }
            let ttl = match ttl {
                Some(raw) => parse_ttl(raw)?,
                None => DEFAULT_TTL,
            };
            // STORY-1700: a drive mandate covers sensitive specs unless the
            // human opts out. trace:STORY-1700 | ai:claude
            let include =
                drive_includes_supervised(*drive, *include_supervised, *exclude_supervised);
            let m = start_with(
                &dir,
                &human,
                &crate::current_user_id(None),
                ttl,
                reason.clone(),
                include,
                *drive,
                Utc::now(),
            )?;
            if let Some(root) = &audit_root {
                emit_changed(root, &m, true);
            }
            if m.is_drive() {
                println!(
                    "Drive mandate {} started until {}: the advisor seat may ship held PRs with an attested, independent approval and green CI, sign off sketches and dispatch fixers; every 15 minutes the drive tick routes each stalled lane.",
                    m.id,
                    m.expires_at.to_rfc3339()
                );
                if let Some(root) = &audit_root {
                    for warning in crate::mandate_drive::start_warnings(root, &m.principal) {
                        eprintln!("warning: {warning}");
                    }
                }
            } else {
                println!(
                    "Mandate {} started: the advisor seat may ship held PRs with an attested, independent approval and green CI until {}.",
                    m.id,
                    m.expires_at.to_rfc3339()
                );
            }
            if !m.include_supervised {
                println!("Sensitive specs (security, containment, authority, guided, keystone) stay human-only.");
            }
            println!("Operator-only and decide-mode specs always stay human-only. Stop it any time: `aida mandate stop`.");
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
        crate::cli::MandateAction::Status { json, line } => {
            let now = Utc::now();
            // trace:STORY-1700 | ai:claude
            if *line {
                if let Some(line) = current_status_line(&dir, audit_root.as_deref(), now) {
                    println!("{line}");
                }
                return Ok(());
            }
            let (valid, warnings) = load_valid(&dir, &crate::current_user_id(None), now);
            let state = state_at(&valid, now);
            for w in &warnings {
                eprintln!("warning: {w}");
            }
            print!(
                "{}",
                status_report(&state, now, audit_root.as_deref(), *json)
            );
            Ok(())
        }
    }
}

/// The full `aida mandate status` output (text or JSON), newline-terminated.
// trace:STORY-1888 | ai:claude
pub(crate) fn status_report(
    state: &MandateState,
    now: DateTime<Utc>,
    audit_root: Option<&Path>,
    json: bool,
) -> String {
    if json {
        let mut out = status_json(state, now);
        if let (Some(root), Some(m)) = (audit_root, state_mandate(state)) {
            out["drive_actions"] = crate::mandate_drive::ledger_json(root, &m.id);
        }
        if let Some(root) = audit_root {
            out["protected_paths"] = crate::mandate_gates::protected_paths_json(root);
        }
        return format!("{out}\n");
    }
    let mut out = format!("{}\n", status_text(state, now));
    if let (Some(root), Some(m)) = (audit_root, state_mandate(state)) {
        let gates = crate::mandate_gates::describe(Some(root));
        out.push_str(&format!("  human gates: {gates}\n"));
        if let Some(summary) = crate::mandate_drive::ledger_text(root, &m.id) {
            out.push_str(&format!("{summary}\n"));
        }
    }
    if let Some(root) = audit_root {
        out.push_str(&crate::mandate_gates::protected_paths_text(root));
    }
    out
}

#[cfg(test)]
#[path = "tests/story_1698_mandate_tests.rs"]
mod story_1698_mandate_tests;

/// Fire-fighting mandate: a human-minted, time-boxed grant that lets the
/// advisor seat ship held PRs with an attested, independent approval.
///
/// `start` and `stop` need a human at an interactive terminal; `status`
/// is open to anyone. Operator-only specs always stay human-only.
// trace:STORY-1698 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "mandate")]
pub(crate) struct MandateArgs {
    #[clap(subcommand)]
    pub action: crate::cli::MandateAction,
}

// STORY-1698: the fire-fighting mandate reads/writes only the user's AIDA
// home (+ a best-effort audit event); no store handle. trace:STORY-1698 | ai:claude
fn run_mandate(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    use crate::*;
    let args = <MandateArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let MandateArgs { action } = &args;
    return self::handle_mandate(action);
}

// trace:STORY-1728 | ai:claude
pub(crate) static MANDATE: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "mandate",
        help_after: "merge-hold",
        command: <MandateArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_mandate),
    };
