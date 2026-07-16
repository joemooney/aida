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
//! The TASK-756 primitive was READ-ONLY state. Consumer wiring (STORY-561) has
//! since landed: presence now sets DEFAULTS for mode selection via the
//! `[presence]` config block — `consumers` (master switch), `away_drain`
//! (consumer a: `aida burndown run` drain mode), `escalation` (consumer b: the
//! punt-handling default when away, its OWN knob decoupled from `away_drain` —
//! see `resolve_drain_mode`), and `home_offer` (consumer d: home-side keystone
//! surfacing). Presence is ADVISORY only: explicit per-command flags always win
//! and integrity gates (CI / merge-on-green / authority) always apply. Still
//! un-wired: consumer (c) `aida questions` quiet-when-away accumulation, which
//! is gated on the questions inbox (STORY-555).
//!
// trace:TASK-756 trace:STORY-561 trace:TASK-769 | ai:claude

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
    ///
    /// Routed through the glyph registry (Home / Away) so an
    /// `[ui] glyphs = "ascii"` / `AIDA_GLYPHS=ascii` profile re-renders it; the
    /// default Unicode profile reproduces the historical literals byte-for-byte.
    // trace:TASK-840 | ai:claude
    pub(crate) fn glyph(self) -> &'static str {
        let glyph = match self {
            Presence::Home => crate::glyphs::Glyph::Home,
            Presence::Away => crate::glyphs::Glyph::Away,
        };
        crate::glyphs::get(glyph, crate::find_project_root().ok().as_deref())
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

// ---------------------------------------------------------------------------
// Solo mode (STORY-624) — a visible work-state flag, sibling to presence.
//
// `aida solo` marks this session as advisor+integrator working the SAFE backlog
// end-to-end with maximum discretion (EPIC-43 / docs/solo-mode.md). Like
// presence it is a timestamped `~/.aida/solo.toml` file (no daemon) and is
// COMPUTED through a TTL so a forgotten solo flag self-clears — a solo session
// should not run forever. The statusline surfaces it; honoring it as a drain
// autonomy posture is a deferred follow-up (this slice is the visible state).
// trace:STORY-624 | ai:claude
// ---------------------------------------------------------------------------

/// Filename under `~/.aida/`.
const SOLO_FILENAME: &str = "solo.toml";

/// Default solo TTL when none is given: 24h (the operator's stated shelf-life
/// for unattended work). A solo flag older than this reads as off.
pub(crate) const DEFAULT_SOLO_TTL_SECS: u64 = 24 * 60 * 60;

/// On-disk solo record. `active` is the stored intent; effective solo folds in
/// the TTL the same way presence does.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SoloFile {
    pub active: bool,
    pub set_at: String,
    pub ttl_secs: u64,
}

/// `~/.aida/solo.toml`, sibling to `presence.toml` (reuses its home resolution).
fn solo_path() -> Option<PathBuf> {
    presence_path().map(|p| p.with_file_name(SOLO_FILENAME))
}

pub(crate) fn read_solo_file() -> Option<SoloFile> {
    let path = solo_path()?;
    toml::from_str(&std::fs::read_to_string(&path).ok()?).ok()
}

fn write_solo_file(file: &SoloFile) -> Result<()> {
    let path = solo_path().context("cannot resolve home directory for solo file")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(
        &path,
        toml::to_string_pretty(file).context("serializing solo file")?,
    )
    .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Enter solo mode with the given TTL (seconds).
pub(crate) fn set_solo(ttl_secs: u64) -> Result<()> {
    write_solo_file(&SoloFile {
        active: true,
        set_at: Utc::now().to_rfc3339(),
        ttl_secs,
    })
}

/// Exit solo mode.
pub(crate) fn clear_solo() -> Result<()> {
    write_solo_file(&SoloFile {
        active: false,
        set_at: Utc::now().to_rfc3339(),
        ttl_secs: 0,
    })
}

/// PURE: effective solo state — active AND still within its TTL window. An
/// expired flag (or `active = false`, or no file) reads as off. trace:STORY-624
pub(crate) fn effective_solo(
    active: bool,
    set_at: DateTime<Utc>,
    ttl_secs: u64,
    now: DateTime<Utc>,
) -> bool {
    if !active {
        return false;
    }
    let elapsed = (now - set_at).num_seconds();
    elapsed < 0 || (elapsed as u64) < ttl_secs
}

/// Effective solo state right now (folds the stored flag through its TTL).
pub(crate) fn current_solo(now: DateTime<Utc>) -> bool {
    match read_solo_file() {
        Some(f) => {
            let set_at = DateTime::parse_from_rfc3339(&f.set_at)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or(now);
            effective_solo(f.active, set_at, f.ttl_secs, now)
        }
        None => false,
    }
}

/// PURE: build the compact solo marker from a resolved glyph + effective-solo
/// bool. `None` when solo is off (the quiet default, matching the away segment).
/// Split out from `statusline_solo_marker` so the rendered marker string is
/// unit-testable without touching `~/.aida/solo.toml` or the glyph registry.
/// trace:TASK-880 | ai:claude
pub(crate) fn solo_marker_label(solo_glyph: &str, active: bool) -> Option<String> {
    active.then(|| format!("{solo_glyph} solo"))
}

/// Compact statusline marker when solo is active, else `None` (off is the quiet
/// default, matching the away-segment contract). trace:STORY-624 trace:TASK-880
pub(crate) fn statusline_solo_marker(now: DateTime<Utc>) -> Option<String> {
    // trace:TASK-840 | ai:claude — route the solo marker through the registry.
    let solo = crate::glyphs::get(
        crate::glyphs::Glyph::Solo,
        crate::find_project_root().ok().as_deref(),
    );
    solo_marker_label(&solo, current_solo(now))
}

// ---------------------------------------------------------------------------
// Solo as a drain autonomy POSTURE (TASK-827 / EPIC-43).
//
// STORY-624 shipped solo as a visible FLAG; this folds that flag into the
// escalate-vs-proceed decision the `--no-human=both` drain makes on a punted
// design-fork. The contract (docs/solo-mode.md):
//
//   solo active + SAFE work     → PROCEED on the defensible default
//                                 (max-discretion safe-backlog mode)
//   solo active + KEYSTONE work → PARK for the human (never ship keystone
//                                 unattended), reusing the existing
//                                 NeedsAttention park path
//   solo inactive               → UNCHANGED (the baseline escalate flags win)
//
// PURE — the decision is unit-tested in isolation; the call site reads
// `current_solo` + classifies the spec and feeds them in. trace:TASK-827
// ---------------------------------------------------------------------------

/// PURE: is this spec keystone / architecture-class — the work solo mode must
/// PARK for the human rather than ship on a default?
///
/// Conservative by design (a false negative ships keystone unattended, the
/// expensive error; a false positive merely parks a safe spec for human review,
/// the cheap one). Reuses the existing `supervised` convention (`burndown.rs`)
/// and adds the small documented heuristic the spec calls for: `epic` type, or
/// any `keystone` / `architecture` / `security` / high-blast-radius tag.
/// trace:TASK-827 | ai:claude
pub(crate) fn is_keystone_class<'a, I>(req_type: &str, tags: I) -> bool
where
    I: IntoIterator<Item = &'a str>,
{
    // An epic is architecture-shaped by definition — never ship one on a
    // default.
    if req_type.trim().eq_ignore_ascii_case("epic") {
        return true;
    }
    tags.into_iter().any(|t| {
        let lo = t.trim().to_ascii_lowercase();
        // Exact keystone/architecture/security/supervised markers, plus the
        // `supervised` / `needs-supervised-build` build-gating convention and
        // any explicit `blast-radius:high` / `risk:high` tag.
        lo == "keystone"
            || lo == "architecture"
            || lo == "security"
            || lo == "supervised"
            || lo == "needs-supervised-build"
            || lo == "blast-radius:high"
            || lo == "risk:high"
    })
}

/// The solo posture's verdict for one punted/escalated design-fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SoloPosture {
    /// Solo inactive — solo supplied nothing; the baseline escalate flags win.
    Inactive,
    /// Solo active + safe work → resume on the defensible default
    /// (max-discretion). Maps to `EscalateMode::Defaults`.
    ProceedOnDefault,
    /// Solo active + keystone/architecture work → park `NeedsAttention` for the
    /// human. Maps to `EscalateMode::Blocks`.
    ParkForHuman,
}

impl SoloPosture {
    /// Did solo actually steer the verdict (the trigger for the one-line
    /// banner)? `Inactive` means it did not.
    pub(crate) fn is_active(self) -> bool {
        !matches!(self, SoloPosture::Inactive)
    }

    /// Whether this posture wants the escalate-on-fork behaviour to ship the
    /// defensible default (`true`) or park (`false`). Only consulted when
    /// `is_active()`. trace:TASK-827
    pub(crate) fn escalate_defaults(self) -> bool {
        matches!(self, SoloPosture::ProceedOnDefault)
    }
}

/// PURE: resolve the solo posture for one design-fork from solo state +
/// keystone classification. **Solo inactive → `Inactive`** (baseline behaviour
/// unchanged, the load-bearing "do not change behaviour when solo is off"
/// guarantee). Solo active biases toward PROCEED on safe work and PARK on
/// keystone work. trace:TASK-827 | ai:claude
pub(crate) fn resolve_solo_posture(solo_active: bool, is_keystone: bool) -> SoloPosture {
    if !solo_active {
        return SoloPosture::Inactive;
    }
    if is_keystone {
        SoloPosture::ParkForHuman
    } else {
        SoloPosture::ProceedOnDefault
    }
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

/// P1b — `presence.escalation` (TASK-769): the punt-handling default when away,
/// as its OWN knob DECOUPLED from `away_drain`. Splits the escalate-vs-park
/// choice out of the `away_drain` rung so the operator can, e.g., pick a
/// max-throughput `headless-both` drain but still PARK punts for triage rather
/// than shipping a default (previously impossible — escalation rode the rung).
///
/// The knob is deliberately absent-by-default: when `[presence] escalation` is
/// unset, escalation is DERIVED from `away_drain` exactly as before
/// (`headless-park` → park, every other rung → ship defaults), so existing
/// configs are byte-for-byte unchanged. Setting it explicitly decouples the two.
// trace:TASK-769 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Escalation {
    /// Punts ship the defensible default (escalate defaults).
    Defaults,
    /// Punts PARK (`NeedsAttention`) for triage (escalate blocks).
    Park,
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
    /// P1b — punt-handling default, decoupled from `away_drain` (TASK-769).
    /// `None` = derive from `away_drain` (the historical coupling, unchanged);
    /// `Some(_)` = the operator picked escalation independently of the rung.
    pub escalation: Option<Escalation>,
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

impl Escalation {
    // trace:TASK-769 | ai:claude
    fn from_config_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "defaults" | "escalate-defaults" | "ship-defaults" => Some(Self::Defaults),
            "park" | "escalate-blocks" | "blocks" => Some(Self::Park),
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
    // TASK-769: the standalone escalation knob. Absent → `None` → derived from
    // `away_drain` at resolve time (the historical coupling); an unparseable
    // value also leaves it absent (falls back, never errors).
    if let Some(s) = table.get("escalation").and_then(|v| v.as_str()) {
        if let Some(m) = Escalation::from_config_str(s) {
            cfg.escalation = Some(m);
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
    // `away_drain` still chooses the `--no-human` axis (always headless-both
    // today) and supplies the LEGACY escalation default it used to imply.
    let (no_human, away_drain_escalate) = match cfg.away_drain {
        AwayDrain::HeadlessBoth | AwayDrain::HeadlessEscalateDefaults => (Some("both"), true),
        AwayDrain::HeadlessPark => (Some("both"), false),
    };
    // TASK-769: escalation is its OWN axis. When `[presence] escalation` is set
    // it wins outright (decoupled); when unset we fall back to the value the
    // away_drain rung implied — preserving the historical coupling exactly.
    // trace:TASK-769 | ai:claude
    let escalate_defaults = match cfg.escalation {
        Some(Escalation::Defaults) => true,
        Some(Escalation::Park) => false,
        None => away_drain_escalate,
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

// ---------------------------------------------------------------------------
// Last-human-input oracle (STORY-769).
//
// The per-turn `aida awaiting --notice` hook stamps a per-session
// last-human-input timestamp under `~/.aida/turn-clock/<session>.toml`. That one
// stamp is the SINGLE source of truth for two coupled surfaces:
//   1. the notice's "Timing:" clause (first-prompt vs continuation-with-gap), and
//   2. a human-presence ORACLE — "operator last seen Nm ago", Active/Idle/Stale
//      — that `aida human presence`, `aida ps`, and the escalation cascade read
//      to decide whether an interactive ask is answerable (operator active) or
//      the punt should park / go headless (operator stale).
//
// No daemon: the hook process writes the stamp; every reader scans the
// per-session files and takes the most-recent one. Fail-open by construction —
// a missing / garbled / unparseable stamp is "unknown", never an error. This is
// DISTINCT from the `home`/`away` primitive above (an explicit operator intent
// with a TTL); the oracle is a passive observation of the last real input.
// trace:STORY-769 | ai:claude
// ---------------------------------------------------------------------------

/// Subdirectory under `~/.aida/` holding the per-session turn-clock files.
const TURN_CLOCK_DIRNAME: &str = "turn-clock";

/// Active/Idle/Stale verdict for the operator, derived from the last-human-input
/// gap. Advisory: it colors an escalation/ask decision, never a hard gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HumanPresence {
    /// Input within the active window — an interactive ask is answerable.
    Active,
    /// Between the active and stale marks — probably around, maybe slow.
    Idle,
    /// No input past the stale mark — treat as away (park / go headless).
    Stale,
}

impl HumanPresence {
    pub(crate) fn word(self) -> &'static str {
        match self {
            HumanPresence::Active => "active",
            HumanPresence::Idle => "idle",
            HumanPresence::Stale => "stale",
        }
    }
}

/// Thresholds for the Active/Idle/Stale bands (seconds). Active strictly below
/// the low mark; Stale at-or-above the high mark; Idle in between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PresenceThresholds {
    pub active_below_secs: u64,
    pub stale_above_secs: u64,
}

impl Default for PresenceThresholds {
    fn default() -> Self {
        // Active < 15m, Stale >= 2h (Idle between) — the spec's sensible default.
        PresenceThresholds {
            active_below_secs: 15 * 60,
            stale_above_secs: 2 * 60 * 60,
        }
    }
}

/// PURE: classify operator presence from the last-human-input gap. `elapsed =
/// now - last_seen`, clamped at 0 for clock skew.
pub(crate) fn human_presence(
    now: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    thresholds: PresenceThresholds,
) -> HumanPresence {
    let elapsed = (now - last_seen).num_seconds().max(0) as u64;
    if elapsed < thresholds.active_below_secs {
        HumanPresence::Active
    } else if elapsed >= thresholds.stale_above_secs {
        HumanPresence::Stale
    } else {
        HumanPresence::Idle
    }
}

/// Per-session turn-clock record. `last_seen` is the last-human-input timestamp;
/// `prompt_count` counts observed USER prompts (SessionStart does not bump it, so
/// the first real prompt still reads "first prompt of this session"). Serde-TOML
/// under `~/.aida/turn-clock/<session>.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct TurnClock {
    pub last_seen: String,
    #[serde(default)]
    pub prompt_count: u64,
}

impl TurnClock {
    fn last_seen_utc(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.last_seen)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    }
}

/// `~/.aida/turn-clock/`, honoring `AIDA_HOME` like every other global-dir
/// lookup. Falls back to `$XDG_RUNTIME_DIR/aida-turn-clock/` when no home dir
/// resolves (headless/CI) — the same XDG fallback the trial hook used. Reader
/// and writer share this resolution, so they never diverge.
pub(crate) fn turn_clock_dir() -> Option<PathBuf> {
    let home = match std::env::var("AIDA_HOME") {
        Ok(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => dirs::home_dir(),
    };
    if let Some(home) = home {
        return Some(home.join(".aida").join(TURN_CLOCK_DIRNAME));
    }
    std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .filter(|p| !p.is_empty())
        .map(|p| PathBuf::from(p).join("aida-turn-clock"))
}

/// Filesystem-safe session key: keep `[A-Za-z0-9._-]`, map everything else to
/// `_`, and never let it resolve to empty / `..`. Guards a session id with
/// slashes or spaces from escaping the turn-clock dir.
fn sanitize_session_id(session_id: &str) -> String {
    let s: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = s.trim_matches('.');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

fn turn_clock_path_in(dir: &Path, session_id: &str) -> PathBuf {
    dir.join(format!("{}.toml", sanitize_session_id(session_id)))
}

fn read_turn_clock_in(dir: &Path, session_id: &str) -> Option<TurnClock> {
    let path = turn_clock_path_in(dir, session_id);
    toml::from_str(&std::fs::read_to_string(&path).ok()?).ok()
}

fn write_turn_clock_in(dir: &Path, session_id: &str, clock: &TurnClock) -> Option<()> {
    std::fs::create_dir_all(dir).ok()?;
    let path = turn_clock_path_in(dir, session_id);
    std::fs::write(&path, toml::to_string_pretty(clock).ok()?).ok()
}

/// PURE: the notice's "Timing:" clause, from the prior turn-clock record.
///   - session-start event → `"session start"`
///   - no prior record / `prompt_count == 0` → `"first prompt of this session"`
///   - otherwise → `"continuation (<gap> since last prompt)"`
// trace:STORY-769 | ai:claude
pub(crate) fn timing_label(
    prev: Option<&TurnClock>,
    is_session_start: bool,
    now: DateTime<Utc>,
) -> String {
    if is_session_start {
        return "session start".to_string();
    }
    match prev {
        Some(c) if c.prompt_count > 0 => match c.last_seen_utc() {
            Some(last) => {
                let gap = (now - last).num_seconds().max(0);
                format!("continuation ({} since last prompt)", humanize_secs(gap))
            }
            None => "first prompt of this session".to_string(),
        },
        _ => "first prompt of this session".to_string(),
    }
}

/// Stamp the per-session last-human-input timestamp and return the notice's
/// "Timing:" clause. A SessionStart event updates `last_seen` (the human just
/// launched/resumed — a real presence signal) WITHOUT bumping `prompt_count`, so
/// the first typed prompt still reads "first prompt of this session"; a prompt
/// event bumps the count. Fail-open: with no session key (manual run /
/// unparseable payload) it persists nothing but still returns a truthful label,
/// and any IO error is swallowed.
pub(crate) fn stamp_turn_clock(
    session_id: Option<&str>,
    is_session_start: bool,
    now: DateTime<Utc>,
) -> String {
    let Some(dir) = turn_clock_dir() else {
        return timing_label(None, is_session_start, now);
    };
    stamp_turn_clock_in(&dir, session_id, is_session_start, now)
}

/// Dir-parameterized core of [`stamp_turn_clock`] — testable without env vars.
fn stamp_turn_clock_in(
    dir: &Path,
    session_id: Option<&str>,
    is_session_start: bool,
    now: DateTime<Utc>,
) -> String {
    let Some(session_id) = session_id else {
        // No session key (manual run / unparseable payload) → persist nothing,
        // but still return a truthful label.
        return timing_label(None, is_session_start, now);
    };
    let prev = read_turn_clock_in(dir, session_id);
    let label = timing_label(prev.as_ref(), is_session_start, now);
    let prompt_count =
        prev.as_ref().map(|c| c.prompt_count).unwrap_or(0) + u64::from(!is_session_start);
    let _ = write_turn_clock_in(
        dir,
        session_id,
        &TurnClock {
            last_seen: now.to_rfc3339(),
            prompt_count,
        },
    );
    label
}

/// The most-recent last-human-input timestamp across ALL per-session turn-clock
/// files — the machine-wide "operator last seen" oracle. `None` when nothing has
/// stamped yet (fresh machine / hook never ran).
pub(crate) fn latest_human_input() -> Option<DateTime<Utc>> {
    latest_human_input_in(&turn_clock_dir()?)
}

/// Dir-parameterized core of [`latest_human_input`] — testable without env vars.
fn latest_human_input_in(dir: &Path) -> Option<DateTime<Utc>> {
    let mut latest: Option<DateTime<Utc>> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(clock) = toml::from_str::<TurnClock>(&body) else {
            continue;
        };
        if let Some(ts) = clock.last_seen_utc() {
            if latest.is_none_or(|cur| ts > cur) {
                latest = Some(ts);
            }
        }
    }
    latest
}

/// PURE: extract `(session_id, is_session_start)` from a UserPromptSubmit /
/// SessionStart hook JSON payload. Missing / garbled / non-JSON → `(None,
/// false)`.
pub(crate) fn parse_hook_payload(stdin: &str) -> (Option<String>, bool) {
    let trimmed = stdin.trim();
    if trimmed.is_empty() {
        return (None, false);
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return (None, false);
    };
    let session_id = v
        .get("session_id")
        .and_then(|s| s.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string());
    let is_session_start = v
        .get("hook_event_name")
        .and_then(|s| s.as_str())
        .map(|s| s.eq_ignore_ascii_case("SessionStart"))
        .unwrap_or(false);
    (session_id, is_session_start)
}

/// Read `[presence] active_within` / `stale_after` from a project's config,
/// falling back to the defaults (Active < 15m, Stale >= 2h). Values accept the
/// same forms as `away_ttl` — integer seconds or a humantime-ish string
/// (`"15m"`, `"2h"`).
pub(crate) fn read_presence_thresholds(config_path: &Path) -> PresenceThresholds {
    let mut th = PresenceThresholds::default();
    let Ok(body) = std::fs::read_to_string(config_path) else {
        return th;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return th;
    };
    let Some(table) = value.get("presence") else {
        return th;
    };
    if let Some(secs) = table.get("active_within").and_then(duration_value_secs) {
        th.active_below_secs = secs;
    }
    if let Some(secs) = table.get("stale_after").and_then(duration_value_secs) {
        th.stale_above_secs = secs;
    }
    th
}

fn duration_value_secs(v: &toml::Value) -> Option<u64> {
    if let Some(i) = v.as_integer() {
        return u64::try_from(i).ok();
    }
    if let Some(s) = v.as_str() {
        return parse_duration_secs(s);
    }
    None
}

/// Compact "operator last seen Nm ago — active" line for `aida ps` /
/// `aida human presence`, or `None` when the oracle has no stamp yet.
// trace:STORY-769 | ai:claude
pub(crate) fn last_seen_line(now: DateTime<Utc>, thresholds: PresenceThresholds) -> Option<String> {
    let last = latest_human_input()?;
    let verdict = human_presence(now, last, thresholds);
    Some(format!(
        "operator last seen {} — {}",
        since_label(last, now),
        verdict.word()
    ))
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

    /// STORY-624: effective solo is active-AND-within-TTL; inactive or expired
    /// reads as off (so a forgotten solo flag self-clears).
    #[test]
    fn effective_solo_folds_active_through_ttl() {
        let now = t0();
        // Active, just set → on.
        assert!(effective_solo(true, now, 3600, now));
        // Active, within TTL → on.
        assert!(effective_solo(true, now - Duration::minutes(30), 3600, now));
        // Active, past TTL → off (self-clears).
        assert!(!effective_solo(true, now - Duration::hours(2), 3600, now));
        // Inactive → off regardless of TTL.
        assert!(!effective_solo(false, now, 3600, now));
        // Clock skew (set_at in the future) → still on within window.
        assert!(effective_solo(true, now + Duration::minutes(5), 3600, now));
    }

    // --- TASK-827: solo as a drain autonomy posture ------------------------

    /// Solo INACTIVE → posture is `Inactive` regardless of keystone-ness, and
    /// the baseline escalate behaviour is left untouched. This is the
    /// load-bearing "don't change behaviour when solo is off" guarantee.
    #[test]
    fn solo_inactive_posture_is_unchanged() {
        assert_eq!(resolve_solo_posture(false, false), SoloPosture::Inactive);
        assert_eq!(resolve_solo_posture(false, true), SoloPosture::Inactive);
        assert!(!SoloPosture::Inactive.is_active());
    }

    /// Solo ACTIVE + SAFE work → proceed on the defensible default (maximum
    /// discretion safe-backlog mode → `EscalateMode::Defaults`).
    #[test]
    fn solo_active_safe_proceeds_on_default() {
        let posture = resolve_solo_posture(true, false);
        assert_eq!(posture, SoloPosture::ProceedOnDefault);
        assert!(posture.is_active());
        assert!(posture.escalate_defaults()); // → Defaults (proceed)
    }

    /// Solo ACTIVE + KEYSTONE work → park for the human (never ship keystone
    /// unattended → `EscalateMode::Blocks`).
    #[test]
    fn solo_active_keystone_parks_for_human() {
        let posture = resolve_solo_posture(true, true);
        assert_eq!(posture, SoloPosture::ParkForHuman);
        assert!(posture.is_active());
        assert!(!posture.escalate_defaults()); // → Blocks (park)
    }

    /// Keystone classification: epic type and the documented keystone /
    /// architecture / security / supervised / high-blast-radius tags trip it;
    /// an ordinary task with no such tag does not.
    #[test]
    fn keystone_classification_heuristic() {
        // Epic type → keystone regardless of tags.
        assert!(is_keystone_class("Epic", std::iter::empty()));
        assert!(is_keystone_class("epic", std::iter::empty()));
        // Ordinary task, no keystone tag → safe.
        assert!(!is_keystone_class("Task", std::iter::empty()));
        assert!(!is_keystone_class("Story", ["cleanup"]));
        // Each documented keystone tag trips it (case-insensitive).
        for tag in [
            "keystone",
            "Architecture",
            "security",
            "supervised",
            "needs-supervised-build",
            "blast-radius:high",
            "risk:high",
        ] {
            assert!(
                is_keystone_class("Task", [tag]),
                "tag {tag} should classify keystone"
            );
        }
        // A benign tag alongside no keystone marker stays safe.
        assert!(!is_keystone_class("Task", ["batch:nightly", "papercut"]));
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

    // --- TASK-880: statusline solo marker renders when solo is active --------

    /// When solo is active the statusline gets a compact `<glyph> solo` segment;
    /// when it's off the marker is `None` (the segment stays quiet). The glyph is
    /// resolved through the registry (no raw literal) so the assertion matches the
    /// same `Glyph::Solo` the real `statusline_solo_marker` renders. trace:TASK-880
    #[test]
    fn solo_marker_present_when_active_absent_when_off() {
        let solo = crate::glyphs::get(crate::glyphs::Glyph::Solo, None);
        // Active → a non-empty marker carrying both the glyph and the word.
        let marker = solo_marker_label(solo, true).expect("active solo renders a marker");
        assert!(
            marker.contains("solo"),
            "marker names the solo state: {marker}"
        );
        assert!(
            marker.contains(solo),
            "marker includes the registry glyph: {marker}"
        );
        assert_eq!(marker, format!("{solo} solo"));

        // Off → no segment at all (matches the quiet-default away contract).
        assert_eq!(solo_marker_label(solo, false), None);
    }

    /// The marker honors whatever glyph the registry resolves (ascii fallback,
    /// custom override, …) — the label is glyph-agnostic. trace:TASK-880
    #[test]
    fn solo_marker_uses_the_supplied_glyph() {
        assert_eq!(solo_marker_label("*", true), Some("* solo".to_string()));
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
        // No `escalation` key → absent (derives from away_drain). trace:TASK-769
        assert_eq!(cfg.escalation, None);

        // A garbage value for one key falls back to that key's default without
        // poisoning the others.
        let f = write_config("[presence]\naway_drain = \"nonsense\"\nconsumers = \"off\"\n");
        let cfg = read_presence_config(f.path());
        assert_eq!(cfg.away_drain, AwayDrain::HeadlessBoth); // default
        assert_eq!(cfg.consumers, ConsumersMode::Off); // parsed
    }

    // trace:TASK-769 | ai:claude
    #[test]
    fn presence_config_parses_escalation_knob() {
        // The standalone escalation knob parses independently of away_drain.
        let f = write_config("[presence]\nescalation = \"park\"\n");
        assert_eq!(
            read_presence_config(f.path()).escalation,
            Some(Escalation::Park)
        );
        let f = write_config("[presence]\nescalation = \"defaults\"\n");
        assert_eq!(
            read_presence_config(f.path()).escalation,
            Some(Escalation::Defaults)
        );
        // Aliases + underscore normalization.
        let f = write_config("[presence]\nescalation = \"escalate_blocks\"\n");
        assert_eq!(
            read_presence_config(f.path()).escalation,
            Some(Escalation::Park)
        );
        let f = write_config("[presence]\nescalation = \"ship-defaults\"\n");
        assert_eq!(
            read_presence_config(f.path()).escalation,
            Some(Escalation::Defaults)
        );
        // Garbage → absent (falls back to away_drain-derived), never errors.
        let f = write_config("[presence]\nescalation = \"nonsense\"\n");
        assert_eq!(read_presence_config(f.path()).escalation, None);
    }

    // trace:TASK-769 | ai:claude
    #[test]
    fn escalation_knob_decouples_from_away_drain() {
        let away = Presence::Away;
        // headless-both (would imply escalate-defaults) + escalation=park →
        // PARK wins: max-throughput drain but punts still park for triage. This
        // combination was impossible before the decoupling.
        let cfg = PresenceConfig {
            consumers: ConsumersMode::On,
            home_offer: HomeOffer::Surface,
            away_drain: AwayDrain::HeadlessBoth,
            escalation: Some(Escalation::Park),
        };
        let r = resolve_drain_mode(None, false, false, away, &cfg);
        assert_eq!(r.no_human.as_deref(), Some("both"));
        assert!(!r.escalate_defaults); // decoupled: park despite headless-both
        assert!(r.presence_applied);

        // headless-park (would imply park) + escalation=defaults → DEFAULTS
        // wins: the symmetric override.
        let cfg = PresenceConfig {
            away_drain: AwayDrain::HeadlessPark,
            escalation: Some(Escalation::Defaults),
            ..cfg
        };
        let r = resolve_drain_mode(None, false, false, away, &cfg);
        assert!(r.escalate_defaults); // decoupled: defaults despite headless-park

        // escalation=None → falls back to the away_drain-derived value exactly
        // as before (backward-compatible: headless-park → park).
        let cfg = PresenceConfig {
            away_drain: AwayDrain::HeadlessPark,
            escalation: None,
            ..cfg
        };
        let r = resolve_drain_mode(None, false, false, away, &cfg);
        assert!(!r.escalate_defaults);
    }

    #[test]
    fn away_drain_advice_maps_three_rungs() {
        let away = Presence::Away;
        let on = PresenceConfig {
            consumers: ConsumersMode::On,
            home_offer: HomeOffer::Surface,
            away_drain: AwayDrain::HeadlessBoth,
            escalation: None,
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

    // --- STORY-769: last-human-input oracle ---------------------------------

    /// The pure Active/Idle/Stale verdict lands correctly on each side of both
    /// threshold boundaries (Active strictly below the low mark; Stale
    /// at-or-above the high mark; Idle strictly between).
    #[test]
    fn human_presence_bands_at_each_threshold_boundary() {
        let now = t0();
        let th = PresenceThresholds::default(); // active < 15m, stale >= 2h
        let seen = |mins: i64| now - Duration::minutes(mins);

        // Just now → active.
        assert_eq!(human_presence(now, now, th), HumanPresence::Active);
        // 14m59s ago → still active (strictly below 15m).
        assert_eq!(
            human_presence(now, now - Duration::seconds(14 * 60 + 59), th),
            HumanPresence::Active
        );
        // Exactly 15m ago → no longer active (boundary is exclusive) → idle.
        assert_eq!(human_presence(now, seen(15), th), HumanPresence::Idle);
        // 1h ago → idle (between the marks).
        assert_eq!(human_presence(now, seen(60), th), HumanPresence::Idle);
        // 1h59m59s ago → still idle (strictly below 2h).
        assert_eq!(
            human_presence(now, now - Duration::seconds(2 * 3600 - 1), th),
            HumanPresence::Idle
        );
        // Exactly 2h ago → stale (boundary is inclusive).
        assert_eq!(human_presence(now, seen(120), th), HumanPresence::Stale);
        // 5h ago → stale.
        assert_eq!(human_presence(now, seen(300), th), HumanPresence::Stale);
        // Clock skew: last_seen in the future → elapsed clamps to 0 → active.
        assert_eq!(
            human_presence(now, now + Duration::minutes(5), th),
            HumanPresence::Active
        );
    }

    /// Configurable thresholds move the bands.
    #[test]
    fn human_presence_honors_custom_thresholds() {
        let now = t0();
        let th = PresenceThresholds {
            active_below_secs: 60,    // 1m
            stale_above_secs: 5 * 60, // 5m
        };
        assert_eq!(
            human_presence(now, now - Duration::seconds(30), th),
            HumanPresence::Active
        );
        assert_eq!(
            human_presence(now, now - Duration::minutes(3), th),
            HumanPresence::Idle
        );
        assert_eq!(
            human_presence(now, now - Duration::minutes(6), th),
            HumanPresence::Stale
        );
    }

    /// `timing_label`: session-start, first-prompt (no prior / zero count), and
    /// continuation-with-gap.
    #[test]
    fn timing_label_first_prompt_vs_continuation() {
        let now = t0();
        // SessionStart event → "session start" regardless of prior.
        assert_eq!(timing_label(None, true, now), "session start");
        // No prior record → first prompt.
        assert_eq!(
            timing_label(None, false, now),
            "first prompt of this session"
        );
        // Prior record with prompt_count == 0 (only SessionStart seen) → still
        // first prompt.
        let seeded = TurnClock {
            last_seen: (now - Duration::minutes(1)).to_rfc3339(),
            prompt_count: 0,
        };
        assert_eq!(
            timing_label(Some(&seeded), false, now),
            "first prompt of this session"
        );
        // Prior prompt 3m ago → continuation with the gap.
        let prev = TurnClock {
            last_seen: (now - Duration::minutes(3)).to_rfc3339(),
            prompt_count: 2,
        };
        assert_eq!(
            timing_label(Some(&prev), false, now),
            "continuation (3m since last prompt)"
        );
        // A 2h gap humanizes to hours.
        let prev = TurnClock {
            last_seen: (now - Duration::hours(2)).to_rfc3339(),
            prompt_count: 5,
        };
        assert_eq!(
            timing_label(Some(&prev), false, now),
            "continuation (2h since last prompt)"
        );
    }

    /// Round-trip: SessionStart stamps `last_seen` but leaves the first typed
    /// prompt reading "first prompt"; subsequent prompts read continuation with
    /// the true gap; and the oracle scan returns the latest stamp.
    #[test]
    fn stamp_turn_clock_round_trip_and_oracle_scan() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let t = t0();
        let sid = "sess-abc";

        // SessionStart: stamps last_seen, prompt_count stays 0 → label is the
        // session-start wording, not "continuation".
        assert_eq!(
            stamp_turn_clock_in(base, Some(sid), true, t),
            "session start"
        );
        let after_start = read_turn_clock_in(base, sid).unwrap();
        assert_eq!(after_start.prompt_count, 0);
        assert!(after_start.last_seen_utc().is_some());

        // First real prompt 2m later → "first prompt" despite the SessionStart
        // stamp (prompt_count was still 0), and now the count bumps to 1.
        let t1 = t + Duration::minutes(2);
        assert_eq!(
            stamp_turn_clock_in(base, Some(sid), false, t1),
            "first prompt of this session"
        );
        assert_eq!(read_turn_clock_in(base, sid).unwrap().prompt_count, 1);

        // Second prompt 5m after the first → continuation with a 5m gap.
        let t2 = t1 + Duration::minutes(5);
        assert_eq!(
            stamp_turn_clock_in(base, Some(sid), false, t2),
            "continuation (5m since last prompt)"
        );

        // The oracle scan returns the most-recent stamp across sessions. Add an
        // older second session; the latest is still session `sid` at t2.
        stamp_turn_clock_in(base, Some("sess-old"), false, t - Duration::hours(3));
        let latest = latest_human_input_in(base).unwrap();
        assert_eq!(latest, t2);

        // No session key → persists nothing, still returns a truthful label.
        assert_eq!(
            stamp_turn_clock_in(base, None, false, t2),
            "first prompt of this session"
        );
    }

    /// The oracle scan is `None` on an empty / missing dir (fresh machine). And
    /// `human_presence` off the scanned stamp verdicts correctly.
    #[test]
    fn latest_human_input_none_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        // Empty dir → None.
        assert_eq!(latest_human_input_in(dir.path()), None);
        // Missing dir → None (no panic).
        assert_eq!(
            latest_human_input_in(&dir.path().join("does-not-exist")),
            None
        );
    }

    /// Session-id sanitization keeps safe chars and neutralizes traversal.
    // trace:STORY-769 | ai:claude
    #[test]
    fn sanitize_session_id_is_path_safe() {
        assert_eq!(sanitize_session_id("abc-123_XY.z"), "abc-123_XY.z");
        assert_eq!(sanitize_session_id("a/b/c"), "a_b_c");
        // Slashes become `_` and leading/trailing dots are trimmed, so the key
        // can never traverse out of the turn-clock dir.
        assert_eq!(sanitize_session_id("../../etc/passwd"), "_.._etc_passwd");
        assert!(!sanitize_session_id("../../etc/passwd").contains('/'));
        assert_eq!(sanitize_session_id(""), "unknown");
        assert_eq!(sanitize_session_id("..."), "unknown");
    }

    /// Hook-payload parsing pulls session_id + the SessionStart flag; garbage is
    /// `(None, false)`.
    #[test]
    fn parse_hook_payload_extracts_session_and_event() {
        let (sid, start) =
            parse_hook_payload(r#"{"session_id":"s1","hook_event_name":"UserPromptSubmit"}"#);
        assert_eq!(sid.as_deref(), Some("s1"));
        assert!(!start);

        let (sid, start) =
            parse_hook_payload(r#"{"session_id":"s2","hook_event_name":"SessionStart"}"#);
        assert_eq!(sid.as_deref(), Some("s2"));
        assert!(start);

        // Missing fields / empty session_id / non-JSON → (None, false).
        assert_eq!(parse_hook_payload("{}"), (None, false));
        assert_eq!(parse_hook_payload(r#"{"session_id":""}"#), (None, false));
        assert_eq!(parse_hook_payload("not json"), (None, false));
        assert_eq!(parse_hook_payload(""), (None, false));
    }

    /// Configurable thresholds parse from `[presence]`, defaulting when absent.
    // trace:STORY-769 | ai:claude
    #[test]
    fn read_presence_thresholds_parses_and_defaults() {
        // Absent file → defaults.
        let th = read_presence_thresholds(Path::new("/nonexistent/config.toml"));
        assert_eq!(th, PresenceThresholds::default());

        // No [presence] block → defaults.
        let f = write_config("[other]\nk = 1\n");
        assert_eq!(
            read_presence_thresholds(f.path()),
            PresenceThresholds::default()
        );

        // Both knobs, humantime + integer.
        let f = write_config("[presence]\nactive_within = \"5m\"\nstale_after = 3600\n");
        let th = read_presence_thresholds(f.path());
        assert_eq!(th.active_below_secs, 300);
        assert_eq!(th.stale_above_secs, 3600);

        // Garbage on one knob falls back to that knob's default, keeps the other.
        let f = write_config("[presence]\nactive_within = \"nonsense\"\nstale_after = \"30m\"\n");
        let th = read_presence_thresholds(f.path());
        assert_eq!(
            th.active_below_secs,
            PresenceThresholds::default().active_below_secs
        );
        assert_eq!(th.stale_above_secs, 30 * 60);
    }
}
