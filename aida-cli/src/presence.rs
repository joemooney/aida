//! Operator-presence primitive (TASK-756, the bounded slice of STORY-561).
//!
//! `aida home` / `aida away` set a machine-global presence state — whether the
//! operator is at the keyboard. It's a TIMESTAMPED FILE under `~/.aida/`
//! (`presence.toml`), sibling to `node.toml` / `agents.toml`; there is NO
//! daemon. `aida presence` reads it.
//!
//! Robustness without a process: `away` carries a TTL (default 8h). A stale
//! `away` (`set_at + ttl < now`) reads as `home` — effective presence is
//! COMPUTED, not just stored. `effective_presence()` is the pure function that
//! encodes that, unit-tested below.
//!
//! This slice is READ-ONLY state: nothing consumes presence to change a
//! command's behavior. Consumer wiring (burndown / escalation / questions /
//! pickup) is a deferred follow-up.
//!
// trace:TASK-756 | ai:claude

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Default away TTL when no `[presence] away_ttl` is configured: 8 hours.
pub(crate) const DEFAULT_AWAY_TTL_SECS: u64 = 8 * 60 * 60;

/// Filename under `~/.aida/`.
const PRESENCE_FILENAME: &str = "presence.toml";

/// Effective (computed) presence. The on-disk state may say `away`, but if its
/// TTL has expired the effective presence is `Home`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Presence {
    Home,
    Away,
}

impl Presence {
    /// Short word for status/statusline surfaces.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Presence::Home => "home",
            Presence::Away => "away",
        }
    }

    /// Small glyph for the statusline segment (the user likes glyphs).
    pub(crate) fn glyph(self) -> &'static str {
        match self {
            Presence::Home => "🏠",
            Presence::Away => "🚶",
        }
    }
}

/// The on-disk presence record. Serde-serialized TOML, matching the
/// `~/.aida/*.toml` convention. `state` is the stored intent; effective
/// presence folds in the TTL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PresenceFile {
    /// `"home"` or `"away"`.
    pub state: String,
    /// RFC3339 timestamp the state was set.
    pub set_at: String,
    /// TTL in seconds the `away` state stays effective. Stored on every
    /// record (harmless for `home`) so the file is self-describing.
    pub ttl_secs: u64,
}

impl PresenceFile {
    fn new(state: Presence, now: DateTime<Utc>, ttl_secs: u64) -> Self {
        PresenceFile {
            state: state.word().to_string(),
            set_at: now.to_rfc3339(),
            ttl_secs,
        }
    }

    /// Parse the stored `state` word into a `Presence`. Unknown/garbage reads
    /// as `Home` (the safe default — never wrongly suppress the operator).
    fn stored_state(&self) -> Presence {
        match self.state.trim().to_ascii_lowercase().as_str() {
            "away" => Presence::Away,
            _ => Presence::Home,
        }
    }

    /// Parsed `set_at`, or `None` if it's unparseable.
    fn set_at_utc(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.set_at)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    }
}

/// PURE: compute effective presence from the stored facts. A `home` state is
/// always `Home`. An `away` state is `Away` only while `set_at + ttl >= now`;
/// once it expires it reads as `Home` (the operator never told us they came
/// back, but the away window lapsed). trace:TASK-756 | ai:claude
pub(crate) fn effective_presence(
    stored: Presence,
    set_at: DateTime<Utc>,
    ttl_secs: u64,
    now: DateTime<Utc>,
) -> Presence {
    match stored {
        Presence::Home => Presence::Home,
        Presence::Away => {
            let elapsed = (now - set_at).num_seconds();
            // Negative elapsed (clock skew / set_at in the future) → still
            // within the window → Away.
            if elapsed < 0 || (elapsed as u64) < ttl_secs {
                Presence::Away
            } else {
                Presence::Home
            }
        }
    }
}

/// Resolve `~/.aida/presence.toml`. Honors `AIDA_HOME` (test hook) the same way
/// the rest of the CLI's global-dir lookups do.
pub(crate) fn presence_path() -> Option<PathBuf> {
    let home = if let Ok(p) = std::env::var("AIDA_HOME") {
        if p.is_empty() {
            dirs::home_dir()?
        } else {
            PathBuf::from(p)
        }
    } else {
        dirs::home_dir()?
    };
    Some(home.join(".aida").join(PRESENCE_FILENAME))
}

/// Read the raw on-disk record. `Ok(None)` when the file is absent (never set)
/// or unreadable/garbage — a missing/garbled presence file is "no opinion",
/// not an error.
pub(crate) fn read_presence_file() -> Option<PresenceFile> {
    let path = presence_path()?;
    let body = std::fs::read_to_string(&path).ok()?;
    toml::from_str(&body).ok()
}

/// Write the record to `~/.aida/presence.toml`, creating `~/.aida/` if needed.
fn write_presence_file(file: &PresenceFile) -> Result<()> {
    let path = presence_path().context("cannot resolve home directory for presence file")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let body = toml::to_string_pretty(file).context("serializing presence file")?;
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Set presence to `Home` (clears any away state). Used by `aida home` and the
/// TTY auto-flip.
pub(crate) fn set_home(ttl_secs: u64) -> Result<()> {
    write_presence_file(&PresenceFile::new(Presence::Home, Utc::now(), ttl_secs))
}

/// Set presence to `Away` with the given TTL. Used by `aida away`.
pub(crate) fn set_away(ttl_secs: u64) -> Result<()> {
    write_presence_file(&PresenceFile::new(Presence::Away, Utc::now(), ttl_secs))
}

/// Effective presence right now, folding the stored state through the TTL.
/// Returns `Presence::Home` when no file exists (the default posture).
pub(crate) fn current_presence(now: DateTime<Utc>) -> Presence {
    match read_presence_file() {
        Some(f) => {
            let set_at = f.set_at_utc().unwrap_or(now);
            effective_presence(f.stored_state(), set_at, f.ttl_secs, now)
        }
        None => Presence::Home,
    }
}

/// Seconds of away-TTL remaining right now, or `None` when the effective
/// presence is `Home` (no file, stored home, or the away-window already
/// lapsed). READ-ONLY: reads the presence file but never writes it — the
/// statusline must NOT auto-flip the operator home just by rendering, which is
/// why this goes through `current_presence`/`effective_presence` and never
/// `auto_flip_if_interactive`. trace:TASK-783
pub(crate) fn current_away_remaining_secs(now: DateTime<Utc>) -> Option<u64> {
    away_remaining_secs(read_presence_file().as_ref(), now)
}

/// PURE: away-TTL-remaining (seconds) from a borrowed record, or `None` when
/// effective presence is `Home`. Borrows the record — never mutates it — so the
/// statusline read is provably non-flipping. trace:TASK-783
fn away_remaining_secs(file: Option<&PresenceFile>, now: DateTime<Utc>) -> Option<u64> {
    let f = file?;
    let set_at = f.set_at_utc().unwrap_or(now);
    if effective_presence(f.stored_state(), set_at, f.ttl_secs, now) != Presence::Away {
        return None;
    }
    let elapsed = (now - set_at).num_seconds().max(0) as u64;
    Some(f.ttl_secs.saturating_sub(elapsed))
}

/// Compact away-TTL-remaining label for the statusline (e.g. `"2h"`), or
/// `None` when home. Mirrors the cache-freshness "only render the non-default
/// state" contract — `home` is the boring default and stays quiet.
/// trace:TASK-783
pub(crate) fn statusline_away_remaining(now: DateTime<Utc>) -> Option<String> {
    current_away_remaining_secs(now).map(|secs| humanize_secs(secs as i64))
}

/// Read `[presence] away_ttl` from a project's `.aida/config.toml`, falling
/// back to the 8h default. Accepts an integer (seconds) or a humantime-ish
/// string (`"8h"`, `"30m"`, `"2h30m"`). Unparseable / absent → default.
///
/// NOTE: presence state lives in `~/.aida/` (machine-global) while config is
/// per-project; this reads the project the command runs in for its TTL. A
/// machine-global TTL would need a `~/.aida/config.toml` convention that does
/// not exist yet. trace:TASK-756 | ai:claude
pub(crate) fn away_ttl_secs(config_path: &Path) -> u64 {
    read_away_ttl_from_config(config_path).unwrap_or(DEFAULT_AWAY_TTL_SECS)
}

fn read_away_ttl_from_config(config_path: &Path) -> Option<u64> {
    let body = std::fs::read_to_string(config_path).ok()?;
    let value: toml::Value = toml::from_str(&body).ok()?;
    let raw = value.get("presence").and_then(|t| t.get("away_ttl"))?;
    if let Some(i) = raw.as_integer() {
        return u64::try_from(i).ok();
    }
    if let Some(s) = raw.as_str() {
        return parse_duration_secs(s);
    }
    None
}

// ---------------------------------------------------------------------------
// `[presence]` consumer policy (STORY-561).
//
// The PRIMITIVE (above) is presence state + display. The CONSUMERS below turn
// that state into ADVISORY mode-selection: away/home shifts the DEFAULTS the
// autonomy ladder keys on. Three knobs under `[presence]`, all with safe
// defaults so they work the moment you `aida away` / `aida home` with zero
// config. Presence is advisory ONLY — explicit per-command flags always win
// and integrity gates (CI, merge-on-green, the kickoff scope-ack) always apply
// (acceptance #4/#5). trace:STORY-561 | ai:claude
// ---------------------------------------------------------------------------

/// P0 — `presence.consumers`: the master switch. `On` (default) makes setting
/// away/home shift the mode defaults below; `Off` reduces presence to a
/// displayed-state-only (the primitive still works; consumers don't fire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ConsumersMode {
    #[default]
    On,
    Off,
}

/// P1 — `presence.away_drain`: the default drain mode when away. All three are
/// fully headless (nobody is supervising); they differ in punt handling. The
/// mapping onto the code's two axes (`--no-human` mode × escalate mode) is
/// operator-confirmed "by behavior" (2026-06-12). trace:STORY-561
// The shared `Headless` prefix is intentional — every away-drain mode is fully
// headless (nobody is supervising); the suffix names the punt-handling.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AwayDrain {
    /// `--no-human=both` + escalate defaults — both phases headless, punts ship
    /// the defensible default. Max unattended throughput. The default.
    #[default]
    HeadlessBoth,
    /// `--no-human=both` + escalate defaults — the explicit "ship defaults on a
    /// punt" spelling of the default.
    HeadlessEscalateDefaults,
    /// `--no-human=both` + escalate blocks — both headless, but punts PARK
    /// (`NeedsAttention`) for triage instead of shipping a default.
    HeadlessPark,
}

/// P2 — `presence.home_offer`: the home-side behavior. `Surface` (default)
/// surfaces keystone / needs-supervised-build specs as "ready for `--zen`" in
/// `aida status` (presence is useful in BOTH directions). `DontBlock` keeps
/// home as merely lifting the away-defaults — specs stay quiet, operator pulls
/// manually.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum HomeOffer {
    #[default]
    Surface,
    DontBlock,
}

/// The parsed `[presence]` consumer policy. Defaults make presence-driven
/// mode-selection work with zero config; the block is opt-out / tuning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PresenceConfig {
    pub consumers: ConsumersMode,
    pub away_drain: AwayDrain,
    pub home_offer: HomeOffer,
}

impl ConsumersMode {
    fn from_config_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "on" | "true" | "enabled" => Some(Self::On),
            "off" | "false" | "disabled" => Some(Self::Off),
            _ => None,
        }
    }
}

impl AwayDrain {
    fn from_config_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "headless-both" | "both" => Some(Self::HeadlessBoth),
            "headless-escalate-defaults" | "escalate-defaults" => {
                Some(Self::HeadlessEscalateDefaults)
            }
            "headless-park" | "park" => Some(Self::HeadlessPark),
            _ => None,
        }
    }
}

impl HomeOffer {
    fn from_config_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "surface" => Some(Self::Surface),
            "dont-block" | "don't-block" | "dontblock" => Some(Self::DontBlock),
            _ => None,
        }
    }
}

/// Read the `[presence]` consumer policy from a project's `.aida/config.toml`,
/// falling back to safe defaults for any absent / unparseable key. A missing
/// file or `[presence]` block → all defaults. trace:STORY-561 | ai:claude
pub(crate) fn read_presence_config(config_path: &Path) -> PresenceConfig {
    let mut cfg = PresenceConfig::default();
    let Ok(body) = std::fs::read_to_string(config_path) else {
        return cfg;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return cfg;
    };
    let Some(table) = value.get("presence") else {
        return cfg;
    };
    if let Some(s) = table.get("consumers").and_then(|v| v.as_str()) {
        if let Some(m) = ConsumersMode::from_config_str(s) {
            cfg.consumers = m;
        }
    }
    if let Some(s) = table.get("away_drain").and_then(|v| v.as_str()) {
        if let Some(m) = AwayDrain::from_config_str(s) {
            cfg.away_drain = m;
        }
    }
    if let Some(s) = table.get("home_offer").and_then(|v| v.as_str()) {
        if let Some(m) = HomeOffer::from_config_str(s) {
            cfg.home_offer = m;
        }
    }
    cfg
}

/// The effective drain mode after folding presence into the explicit flags.
/// `presence_applied` is true only when presence supplied a default the
/// operator did NOT spell explicitly — the trigger for the advisory banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DrainModeResolution {
    /// `--no-human` slug (`"both"` / `"reviewer-only"`) or `None` for the
    /// interactive default. Parsed by `NoHumanMode::parse` at the call site.
    pub no_human: Option<String>,
    /// Whether escalation should ship defaults (`true`) or park (`false`).
    /// Only meaningful when `no_human == Some("both")`.
    pub escalate_defaults: bool,
    /// Presence supplied a default the operator didn't pass explicitly.
    pub presence_applied: bool,
}

/// PURE: resolve the effective `queue work --auto-complete` drain mode from the
/// explicit flags + current presence + config. **Explicit flags ALWAYS win**
/// (acceptance #5): if the operator passed `--no-human` or an escalate flag,
/// presence is ignored for that axis. Presence only fills an ABSENT default,
/// and only when away + consumers are on. Home is the interactive default
/// (presence supplies nothing — today's behavior). This never bypasses the
/// kickoff scope-ack or any integrity gate; it only chooses a default mode
/// (acceptance #4). trace:STORY-561 | ai:claude
pub(crate) fn resolve_drain_mode(
    explicit_no_human: Option<&str>,
    explicit_escalate_blocks: bool,
    explicit_escalate_defaults: bool,
    presence: Presence,
    cfg: &PresenceConfig,
) -> DrainModeResolution {
    // Any explicit flag on either axis means the operator is steering — leave
    // their values untouched and don't apply a presence default anywhere.
    let any_explicit =
        explicit_no_human.is_some() || explicit_escalate_blocks || explicit_escalate_defaults;
    let presence_eligible =
        matches!(presence, Presence::Away) && cfg.consumers == ConsumersMode::On && !any_explicit;

    if !presence_eligible {
        return DrainModeResolution {
            no_human: explicit_no_human.map(|s| s.to_string()),
            escalate_defaults: explicit_escalate_defaults,
            presence_applied: false,
        };
    }

    // Away + consumers on + no explicit steering → apply the away_drain advice.
    let (no_human, escalate_defaults) = match cfg.away_drain {
        AwayDrain::HeadlessBoth | AwayDrain::HeadlessEscalateDefaults => (Some("both"), true),
        AwayDrain::HeadlessPark => (Some("both"), false),
    };
    DrainModeResolution {
        no_human: no_human.map(|s| s.to_string()),
        escalate_defaults,
        presence_applied: true,
    }
}

/// Parse a small humantime-ish duration string into seconds. Supports a bare
/// integer (`"3600"` = seconds) and `h`/`m`/`s` suffixed components that may be
/// concatenated (`"8h"`, `"30m"`, `"2h30m"`, `"1h30m15s"`). Returns `None` on
/// anything it doesn't recognize. trace:TASK-756 | ai:claude
pub(crate) fn parse_duration_secs(input: &str) -> Option<u64> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    // Bare integer → seconds.
    if let Ok(n) = s.parse::<u64>() {
        return Some(n);
    }
    let mut total: u64 = 0;
    let mut num: u64 = 0;
    let mut saw_digit = false;
    let mut saw_unit = false;
    for ch in s.chars() {
        if let Some(d) = ch.to_digit(10) {
            num = num.checked_mul(10)?.checked_add(d as u64)?;
            saw_digit = true;
        } else {
            if !saw_digit {
                return None;
            }
            let mult = match ch.to_ascii_lowercase() {
                'h' => 3600,
                'm' => 60,
                's' => 1,
                _ => return None,
            };
            total = total.checked_add(num.checked_mul(mult)?)?;
            num = 0;
            saw_digit = false;
            saw_unit = true;
        }
    }
    // A trailing number with no unit is only valid if the whole thing was a
    // bare integer (handled above); mixed `"2h30"` is rejected.
    if saw_digit || !saw_unit {
        return None;
    }
    Some(total)
}

/// Human-relative "since" string for a `set_at` timestamp (e.g. `"2h ago"`).
/// Mirrors the `humanize_elapsed` thresholds used elsewhere.
pub(crate) fn since_label(set_at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - set_at).num_seconds().max(0);
    format!("{} ago", humanize_secs(secs))
}

/// TTL-remaining label for an away state (e.g. `"5h left"`), or `"expired"`
/// once the window has lapsed.
pub(crate) fn ttl_remaining_label(
    set_at: DateTime<Utc>,
    ttl_secs: u64,
    now: DateTime<Utc>,
) -> String {
    let elapsed = (now - set_at).num_seconds().max(0) as u64;
    if elapsed >= ttl_secs {
        "expired".to_string()
    } else {
        format!("{} left", humanize_secs((ttl_secs - elapsed) as i64))
    }
}

/// Same threshold ladder as `agent_registry::humanize_elapsed` (kept local so
/// presence has no cross-module coupling). trace:TASK-756 | ai:claude
fn humanize_secs(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// TTY auto-flip: if presence is STORED `away` and this is an interactive
/// session (stdout + stdin are a TTY), the operator is demonstrably back —
/// rewrite the file to `home`. Cheap: only touches the file when a flip is
/// actually needed. NON-fatal: any error is swallowed so a presence-file
/// problem never breaks the real command. trace:TASK-756 | ai:claude
pub(crate) fn auto_flip_if_interactive() {
    use std::io::IsTerminal;
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        return;
    }
    // Only read+write when the file says away — avoid a write on every
    // interactive invocation.
    let Some(file) = read_presence_file() else {
        return;
    };
    if !matches!(file.stored_state(), Presence::Away) {
        return;
    }
    // Preserve the configured TTL on the flipped-home record.
    let _ = set_home(file.ttl_secs);
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-11T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn home_state_is_always_home() {
        let now = t0();
        assert_eq!(
            effective_presence(Presence::Home, now, DEFAULT_AWAY_TTL_SECS, now),
            Presence::Home
        );
        // Even far in the future, home stays home.
        assert_eq!(
            effective_presence(
                Presence::Home,
                now - Duration::hours(100),
                DEFAULT_AWAY_TTL_SECS,
                now
            ),
            Presence::Home
        );
    }

    #[test]
    fn fresh_away_reads_away() {
        let set_at = t0();
        let now = set_at + Duration::hours(1);
        assert_eq!(
            effective_presence(Presence::Away, set_at, DEFAULT_AWAY_TTL_SECS, now),
            Presence::Away
        );
    }

    #[test]
    fn expired_away_reads_home() {
        let set_at = t0();
        // 8h TTL; 9h later → expired → home.
        let now = set_at + Duration::hours(9);
        assert_eq!(
            effective_presence(Presence::Away, set_at, DEFAULT_AWAY_TTL_SECS, now),
            Presence::Home
        );
    }

    #[test]
    fn away_at_exact_ttl_boundary_reads_home() {
        let set_at = t0();
        // elapsed == ttl → not strictly less than ttl → expired → home.
        let now = set_at + Duration::seconds(DEFAULT_AWAY_TTL_SECS as i64);
        assert_eq!(
            effective_presence(Presence::Away, set_at, DEFAULT_AWAY_TTL_SECS, now),
            Presence::Home
        );
    }

    #[test]
    fn away_just_before_ttl_reads_away() {
        let set_at = t0();
        let now = set_at + Duration::seconds(DEFAULT_AWAY_TTL_SECS as i64 - 1);
        assert_eq!(
            effective_presence(Presence::Away, set_at, DEFAULT_AWAY_TTL_SECS, now),
            Presence::Away
        );
    }

    #[test]
    fn clock_skew_future_set_at_reads_away() {
        let now = t0();
        // set_at in the future (negative elapsed) → still within window.
        let set_at = now + Duration::minutes(5);
        assert_eq!(
            effective_presence(Presence::Away, set_at, DEFAULT_AWAY_TTL_SECS, now),
            Presence::Away
        );
    }

    #[test]
    fn parse_duration_bare_integer_is_seconds() {
        assert_eq!(parse_duration_secs("3600"), Some(3600));
        assert_eq!(parse_duration_secs(" 120 "), Some(120));
    }

    #[test]
    fn parse_duration_suffixed() {
        assert_eq!(parse_duration_secs("8h"), Some(8 * 3600));
        assert_eq!(parse_duration_secs("30m"), Some(30 * 60));
        assert_eq!(parse_duration_secs("45s"), Some(45));
        assert_eq!(parse_duration_secs("2h30m"), Some(2 * 3600 + 30 * 60));
        assert_eq!(parse_duration_secs("1h30m15s"), Some(3600 + 30 * 60 + 15));
    }

    #[test]
    fn parse_duration_rejects_garbage() {
        assert_eq!(parse_duration_secs(""), None);
        assert_eq!(parse_duration_secs("abc"), None);
        assert_eq!(parse_duration_secs("2h30"), None); // trailing unitless
        assert_eq!(parse_duration_secs("h"), None); // unit, no number
        assert_eq!(parse_duration_secs("2x"), None); // unknown unit
    }

    #[test]
    fn since_and_ttl_labels() {
        let set_at = t0();
        let now = set_at + Duration::hours(2);
        assert_eq!(since_label(set_at, now), "2h ago");
        // 8h TTL, 2h elapsed → 6h left.
        assert_eq!(
            ttl_remaining_label(set_at, DEFAULT_AWAY_TTL_SECS, now),
            "6h left"
        );
        // Past TTL.
        let later = set_at + Duration::hours(9);
        assert_eq!(
            ttl_remaining_label(set_at, DEFAULT_AWAY_TTL_SECS, later),
            "expired"
        );
    }

    // --- TASK-783: statusline away-TTL-remaining (compact, non-flipping) ----

    #[test]
    fn away_remaining_compact_label_when_away() {
        let set_at = t0();
        // 8h TTL, 2h elapsed → 6h remaining → compact "6h".
        let now = set_at + Duration::hours(2);
        let file = PresenceFile::new(Presence::Away, set_at, DEFAULT_AWAY_TTL_SECS);
        assert_eq!(
            away_remaining_secs(Some(&file), now),
            Some(DEFAULT_AWAY_TTL_SECS - 2 * 3600)
        );
        assert_eq!(
            away_remaining_secs(Some(&file), now).map(|s| humanize_secs(s as i64)),
            Some("6h".to_string())
        );
    }

    #[test]
    fn away_remaining_is_none_when_home() {
        let now = t0();
        let file = PresenceFile::new(Presence::Home, now, DEFAULT_AWAY_TTL_SECS);
        assert_eq!(away_remaining_secs(Some(&file), now), None);
        // No file at all → home → None.
        assert_eq!(away_remaining_secs(None, now), None);
    }

    #[test]
    fn away_remaining_is_none_when_ttl_expired() {
        let set_at = t0();
        // 8h TTL, 9h elapsed → effective home → None (the statusline goes quiet).
        let now = set_at + Duration::hours(9);
        let file = PresenceFile::new(Presence::Away, set_at, DEFAULT_AWAY_TTL_SECS);
        assert_eq!(away_remaining_secs(Some(&file), now), None);
    }

    /// The remaining computation borrows the record and returns owned data — it
    /// cannot mutate presence state. Rendering the statusline must never flip
    /// the operator home. trace:TASK-783
    #[test]
    fn away_remaining_does_not_mutate_record() {
        let set_at = t0();
        let now = set_at + Duration::hours(1);
        let file = PresenceFile::new(Presence::Away, set_at, DEFAULT_AWAY_TTL_SECS);
        let before = (file.state.clone(), file.set_at.clone(), file.ttl_secs);
        let _ = away_remaining_secs(Some(&file), now);
        assert_eq!(
            before,
            (file.state.clone(), file.set_at.clone(), file.ttl_secs)
        );
        // Stored state is still away after the read.
        assert_eq!(file.stored_state(), Presence::Away);
    }

    // --- STORY-561: `[presence]` consumer policy + drain-mode resolver ------

    use std::io::Write;

    fn write_config(body: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(body.as_bytes()).unwrap();
        f
    }

    #[test]
    fn presence_config_defaults_when_absent() {
        // No file at all.
        let cfg = read_presence_config(Path::new("/nonexistent/.aida/config.toml"));
        assert_eq!(cfg, PresenceConfig::default());
        assert_eq!(cfg.consumers, ConsumersMode::On);
        assert_eq!(cfg.away_drain, AwayDrain::HeadlessBoth);
        assert_eq!(cfg.home_offer, HomeOffer::Surface);

        // File present but no [presence] block → still all defaults.
        let f = write_config("[other]\nkey = 1\n");
        assert_eq!(read_presence_config(f.path()), PresenceConfig::default());
    }

    #[test]
    fn presence_config_parses_knobs_and_defaults() {
        let f = write_config(
            "[presence]\nconsumers = \"off\"\naway_drain = \"headless-park\"\nhome_offer = \"dont-block\"\n",
        );
        let cfg = read_presence_config(f.path());
        assert_eq!(cfg.consumers, ConsumersMode::Off);
        assert_eq!(cfg.away_drain, AwayDrain::HeadlessPark);
        assert_eq!(cfg.home_offer, HomeOffer::DontBlock);

        // A garbage value for one key falls back to that key's default without
        // poisoning the others.
        let f = write_config("[presence]\naway_drain = \"nonsense\"\nconsumers = \"off\"\n");
        let cfg = read_presence_config(f.path());
        assert_eq!(cfg.away_drain, AwayDrain::HeadlessBoth); // default
        assert_eq!(cfg.consumers, ConsumersMode::Off); // parsed
    }

    #[test]
    fn away_drain_advice_maps_three_rungs() {
        let away = Presence::Away;
        let on = PresenceConfig {
            consumers: ConsumersMode::On,
            home_offer: HomeOffer::Surface,
            away_drain: AwayDrain::HeadlessBoth,
        };
        // headless-both → both + escalate defaults
        let r = resolve_drain_mode(None, false, false, away, &on);
        assert_eq!(r.no_human.as_deref(), Some("both"));
        assert!(r.escalate_defaults);
        assert!(r.presence_applied);
        // headless-escalate-defaults → both + escalate defaults
        let r = resolve_drain_mode(
            None,
            false,
            false,
            away,
            &PresenceConfig {
                away_drain: AwayDrain::HeadlessEscalateDefaults,
                ..on
            },
        );
        assert_eq!(r.no_human.as_deref(), Some("both"));
        assert!(r.escalate_defaults);
        // headless-park → both + escalate blocks (defaults=false)
        let r = resolve_drain_mode(
            None,
            false,
            false,
            away,
            &PresenceConfig {
                away_drain: AwayDrain::HeadlessPark,
                ..on
            },
        );
        assert_eq!(r.no_human.as_deref(), Some("both"));
        assert!(!r.escalate_defaults);
    }

    #[test]
    fn resolve_drain_mode_explicit_no_human_wins() {
        // Explicit --no-human=reviewer-only while away → presence does NOT
        // override; reviewer-only is preserved, nothing presence-applied.
        let r = resolve_drain_mode(
            Some("reviewer-only"),
            false,
            false,
            Presence::Away,
            &PresenceConfig::default(),
        );
        assert_eq!(r.no_human.as_deref(), Some("reviewer-only"));
        assert!(!r.presence_applied);
    }

    #[test]
    fn resolve_drain_mode_explicit_escalate_wins() {
        // Explicit --escalate-blocks while away → presence does NOT flip it to
        // defaults, and supplies no no_human default either (operator steering).
        let r = resolve_drain_mode(
            None,
            true, // --escalate-blocks
            false,
            Presence::Away,
            &PresenceConfig::default(),
        );
        assert!(!r.escalate_defaults);
        assert_eq!(r.no_human, None);
        assert!(!r.presence_applied);
    }

    #[test]
    fn resolve_drain_mode_home_is_interactive() {
        // Home → presence supplies nothing (today's interactive default).
        let r = resolve_drain_mode(
            None,
            false,
            false,
            Presence::Home,
            &PresenceConfig::default(),
        );
        assert_eq!(r.no_human, None);
        assert!(!r.presence_applied);
    }

    #[test]
    fn resolve_drain_mode_consumers_off_disables() {
        // Away but consumers=off → no presence default (display-only mode).
        let off = PresenceConfig {
            consumers: ConsumersMode::Off,
            ..PresenceConfig::default()
        };
        let r = resolve_drain_mode(None, false, false, Presence::Away, &off);
        assert_eq!(r.no_human, None);
        assert!(!r.presence_applied);
    }
}
