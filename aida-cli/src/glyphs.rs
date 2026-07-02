//! Central glyph registry + ASCII-profile selector (Phase 1 of EPIC-45).
//!
//! AIDA's CLI output leans heavily on unicode/emoji glyphs (~2568 inline
//! literal sites across `aida-cli`). Some terminals cannot render all of them
//! (e.g. Windows ConEmu drops several). This module introduces:
//!
//!   1. A central *registry* — a named [`Glyph`] enum of the symbols AIDA
//!      prints, each with a default UNICODE value and a curated ASCII fallback.
//!   2. A *selector* — [`active_profile`] resolves, in precedence order:
//!         `AIDA_GLYPHS` env (`unicode`|`ascii`)
//!           > project `.aida/config.toml` `[ui] glyphs`
//!           > user `~/.aida/config.toml` `[ui] glyphs`
//!           > default UNICODE.
//!   3. [`get`] — fetch a glyph honoring the active profile.
//!
//! Phase 2 (STORY-629) layers a *custom per-symbol override table* on top: a
//! `[glyphs]` config section (project `.aida/config.toml` > user
//! `~/.aida/config.toml`) maps a glyph's name → an arbitrary replacement
//! string. Full precedence for a rendered glyph: custom `[glyphs]` entry
//! (project>user) > active profile (unicode|ascii) > registry default. See
//! [`GlyphOverrides`] / [`get_custom`].
//!
//! OPT-IN by design: the default is UNICODE, so `aida` output is byte-for-byte
//! unchanged unless a user explicitly opts into `ascii`. Nothing
//! auto-downgrades (operator decision, Joe 2026-06-15).
//!
//! SCOPE (phase 1): the registry + selector + ASCII profile. The long tail of
//! raw glyph literals is NOT migrated here (that's phase 3 / TASK-835) — until
//! a literal is migrated to read through [`get`], the override has no effect on
//! it. So at first the `ascii` profile only changes glyphs that have been
//! routed through this registry.
//!
//! trace:STORY-628 | ai:claude

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The named set of symbols AIDA prints. Each variant maps to a default
/// UNICODE value and a curated ASCII fallback via [`Glyph::unicode`] /
/// [`Glyph::ascii`].
///
/// Phase 1 wires only a handful of variants to live render sites (the status
/// glyphs via `status_display`); the rest are seeded here so the long-tail
/// migration (phase 3 / TASK-835) routes literals to an already-defined entry
/// rather than growing the enum incrementally. `allow(dead_code)` keeps the
/// not-yet-consumed variants from warning until then. trace:STORY-628
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Glyph {
    /// Success / completed (✓).
    Check,
    /// Failure / rejected (✗).
    Cross,
    /// Draft / not-started (◯).
    Pending,
    /// In-progress / partial (◐).
    InFlight,
    /// Blocked / needs-attention / warning (⚠).
    Blocked,
    /// Queued / waiting (▷).
    Queued,
    /// Forward / approved pointer (▸).
    Arrow,
    /// Return / sub-item pointer (↳).
    SubArrow,
    /// Mailbox / message (✉).
    Mailbox,
    /// Warning (⚠) — alias-distinct from [`Glyph::Blocked`] for callers that
    /// want a warning marker independent of spec-state semantics.
    Warning,
    /// List bullet (•).
    Bullet,
    /// Waiting / pending-time (⏳).
    Hourglass,
    /// Present / home (🏠).
    Home,
    /// Away (🚶).
    Away,
    /// Solo / robot working alone (🤖).
    Solo,
    /// Generic robot / agent (🤖).
    Robot,
    /// Done — work finished on a branch, not yet merged (◉). The bright-green
    /// bold status in [`crate::status_display`]. trace:TASK-835
    Done,
    /// Neutral / unknown-status bullet (·) — the fallback marker for a custom or
    /// unmapped status so a badge's layout stays stable. trace:TASK-835
    Neutral,
    /// Work-routing: a *live* session lease holds the spec right now (▶). Axis is
    /// orthogonal to status — "where in the pipeline now", not "what state".
    /// trace:TASK-835
    FlowActive,
    /// Work-routing: BlockedBy an incomplete spec (⊘). trace:TASK-835
    FlowBlocked,
    /// Work-routing: present in a role queue, not yet started (↑). trace:TASK-835
    FlowQueued,
    // trace:STORY-730 | ai:claude
    /// Paused / walked-away (⏸) — the "since you were away" morning-after banner
    /// marker on `aida status`.
    Pause,
    // trace:TASK-1071 | ai:claude
    /// Informational notice — the circled-i prefix on hints, notes and
    /// skipped-state messages (ⓘ). The most common info marker in the CLI.
    Info,
    /// Informational notice, plain variant (ℹ) — the source-information glyph
    /// used for reclaim/dry-run/no-op notices. Alias-distinct from [`Glyph::Info`]
    /// so a caller can keep the lighter mark where it already reads that way.
    InfoAlt,
    /// "Awaiting you" attention marker (⦿) — the `aida awaiting` roll-up prefix.
    Awaiting,
    /// Incoming mail / unread-message marker (📨) — the per-turn unread-mail
    /// notice. Distinct from [`Glyph::Mailbox`] (✉, the generic mailbox).
    IncomingMail,
}

impl Glyph {
    /// The default UNICODE rendering. This is what ships today and stays the
    /// default — capable terminals keep the emoji.
    pub(crate) const fn unicode(self) -> &'static str {
        match self {
            Glyph::Check => "✓",
            Glyph::Cross => "✗",
            Glyph::Pending => "◯",
            Glyph::InFlight => "◐",
            Glyph::Blocked => "⚠",
            Glyph::Queued => "▷",
            Glyph::Arrow => "▸",
            Glyph::SubArrow => "↳",
            Glyph::Mailbox => "✉",
            Glyph::Warning => "⚠",
            Glyph::Bullet => "•",
            Glyph::Hourglass => "⏳",
            Glyph::Home => "🏠",
            Glyph::Away => "🚶",
            Glyph::Solo => "🤖",
            Glyph::Robot => "🤖",
            Glyph::Done => "◉",
            Glyph::Neutral => "·",
            Glyph::FlowActive => "▶",
            Glyph::FlowBlocked => "⊘",
            Glyph::FlowQueued => "↑",
            Glyph::Pause => "⏸",
            Glyph::Info => "ⓘ",
            Glyph::InfoAlt => "ℹ",
            Glyph::Awaiting => "⦿",
            Glyph::IncomingMail => "📨",
        }
    }

    /// The curated ASCII fallback. Chosen to stay clean and column-aligned in
    /// tables (mostly 1-3 ASCII columns).
    pub(crate) const fn ascii(self) -> &'static str {
        match self {
            Glyph::Check => "[x]",
            Glyph::Cross => "[ ]",
            Glyph::Pending => "( )",
            Glyph::InFlight => "[~]",
            Glyph::Blocked => "(!)",
            Glyph::Queued => "[>]",
            Glyph::Arrow => "->",
            Glyph::SubArrow => "\\_",
            Glyph::Mailbox => "[@]",
            Glyph::Warning => "!",
            Glyph::Bullet => "*",
            Glyph::Hourglass => "...",
            Glyph::Home => "[home]",
            Glyph::Away => "[away]",
            Glyph::Solo => "[solo]",
            Glyph::Robot => "[bot]",
            Glyph::Done => "[*]",
            Glyph::Neutral => ".",
            Glyph::FlowActive => ">",
            Glyph::FlowBlocked => "x",
            Glyph::FlowQueued => "^",
            Glyph::Pause => "[paused]",
            Glyph::Info => "(i)",
            Glyph::InfoAlt => "i",
            Glyph::Awaiting => "(*)",
            Glyph::IncomingMail => "[@]",
        }
    }

    /// Render this glyph for an explicit [`GlyphProfile`].
    pub(crate) const fn render(self, profile: GlyphProfile) -> &'static str {
        match profile {
            GlyphProfile::Unicode => self.unicode(),
            GlyphProfile::Ascii => self.ascii(),
        }
    }

    /// The canonical name of this glyph — the key used in the `[glyphs]`
    /// custom-override config table (phase 2 / STORY-629). These match the
    /// lower-snake-case form of the variant names so a config like
    /// `[glyphs]\ncheck = "OK"` targets [`Glyph::Check`]. trace:STORY-629
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Glyph::Check => "check",
            Glyph::Cross => "cross",
            Glyph::Pending => "pending",
            Glyph::InFlight => "in_flight",
            Glyph::Blocked => "blocked",
            Glyph::Queued => "queued",
            Glyph::Arrow => "arrow",
            Glyph::SubArrow => "sub_arrow",
            Glyph::Mailbox => "mailbox",
            Glyph::Warning => "warning",
            Glyph::Bullet => "bullet",
            Glyph::Hourglass => "hourglass",
            Glyph::Home => "home",
            Glyph::Away => "away",
            Glyph::Solo => "solo",
            Glyph::Robot => "robot",
            Glyph::Done => "done",
            Glyph::Neutral => "neutral",
            Glyph::FlowActive => "flow_active",
            Glyph::FlowBlocked => "flow_blocked",
            Glyph::FlowQueued => "flow_queued",
            Glyph::Pause => "pause",
            Glyph::Info => "info",
            Glyph::InfoAlt => "info_alt",
            Glyph::Awaiting => "awaiting",
            Glyph::IncomingMail => "incoming_mail",
        }
    }

    /// Map a `[glyphs]` config key to its [`Glyph`] variant. Accepts the
    /// canonical name (see [`Glyph::name`]); also tolerates a `-` separator in
    /// place of `_` (so `sub-arrow` and `in-flight` work). Case-insensitive.
    /// Unknown key → `None` (silently ignored so a typo or a future glyph name
    /// in config doesn't error an older binary). trace:STORY-629
    ///
    /// Phase 4 (STORY-633) reuses this for the `aida config glyph set <name>`
    /// validation — but the CLI rejects an unknown name with the valid list
    /// rather than silently ignoring it. trace:STORY-633
    pub(crate) fn from_name(raw: &str) -> Option<Glyph> {
        let normalized = raw.trim().to_ascii_lowercase().replace('-', "_");
        Glyph::ALL.iter().copied().find(|g| g.name() == normalized)
    }

    /// Every variant, for iteration (name parsing, exhaustive tests).
    /// trace:STORY-629
    pub(crate) const ALL: [Glyph; 26] = [
        Glyph::Check,
        Glyph::Cross,
        Glyph::Pending,
        Glyph::InFlight,
        Glyph::Blocked,
        Glyph::Queued,
        Glyph::Arrow,
        Glyph::SubArrow,
        Glyph::Mailbox,
        Glyph::Warning,
        Glyph::Bullet,
        Glyph::Hourglass,
        Glyph::Home,
        Glyph::Away,
        Glyph::Solo,
        Glyph::Robot,
        Glyph::Done,
        Glyph::Neutral,
        Glyph::FlowActive,
        Glyph::FlowBlocked,
        Glyph::FlowQueued,
        Glyph::Pause,
        Glyph::Info,
        Glyph::InfoAlt,
        Glyph::Awaiting,
        Glyph::IncomingMail,
    ];
}

/// Which rendering profile is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum GlyphProfile {
    /// Default — emoji/unicode preserved for capable terminals.
    #[default]
    Unicode,
    /// Curated ASCII fallback.
    Ascii,
}

impl GlyphProfile {
    /// Parse a profile name (`unicode` / `ascii`, case-insensitive). Unknown /
    /// empty → `None` so callers can fall through to the next precedence tier.
    fn parse(raw: &str) -> Option<GlyphProfile> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "unicode" | "emoji" | "utf8" | "utf-8" => Some(GlyphProfile::Unicode),
            "ascii" | "plain" => Some(GlyphProfile::Ascii),
            _ => None,
        }
    }

    /// Canonical profile name for display (e.g. in `aida config show`).
    /// trace:TASK-793 | ai:claude
    pub(crate) const fn name(self) -> &'static str {
        match self {
            GlyphProfile::Unicode => "unicode",
            GlyphProfile::Ascii => "ascii",
        }
    }
}

/// A named, binary-embedded glyph *theme* (phase 4 / STORY-633).
///
/// A theme is a curated preset of `{base profile + per-symbol override bundle}`.
/// It is stored in config as a clean *reference* — `[ui] theme = "<name>"` — and
/// resolved at render time, exactly like `[ui] glyphs = "ascii"` already works,
/// rather than expanding its overrides into `[glyphs]`. The set grows by adding
/// a preset here, not a config file.
///
/// Precedence tier introduced: a per-symbol `[glyphs]` override beats the theme
/// bundle, which beats the base `[ui] glyphs` profile, which beats the registry
/// default. See [`resolve_with_theme`]. trace:STORY-633
#[derive(Debug, Clone, Copy)]
pub(crate) struct Theme {
    /// The reference name written to `[ui] theme` (lowercase, hyphen-friendly).
    pub(crate) name: &'static str,
    /// One-line human description for `aida config glyph theme list`.
    pub(crate) description: &'static str,
    /// The base profile the theme starts from before its bundle is applied.
    pub(crate) base: GlyphProfile,
    /// Per-symbol override bundle: `(Glyph, replacement)` pairs layered over the
    /// base profile. An empty bundle = "just the base profile, named".
    pub(crate) bundle: &'static [(Glyph, &'static str)],
}

impl Theme {
    /// Render `glyph` under this theme: the bundle wins for a symbol it covers,
    /// else the base profile's rendering. trace:STORY-633
    pub(crate) fn render(&self, glyph: Glyph) -> String {
        self.bundle
            .iter()
            .find(|(g, _)| *g == glyph)
            .map(|(_, s)| (*s).to_string())
            .unwrap_or_else(|| glyph.render(self.base).to_string())
    }
}

/// The binary-embedded theme presets (phase 4 / STORY-633). Kept intentionally
/// small — three sensible starters. `unicode` is the implicit default when no
/// theme is set, so it is NOT listed here (selecting it = no theme reference).
///
/// - `ascii`: the pure ASCII fallback profile, named for discoverability.
/// - `minimal`: unicode base, but the noisier emoji collapsed to quiet
///   monochrome marks (no 🤖/🏠/🚶/⏳) for low-chrome terminals.
/// - `nerd-font`: unicode base with heavier Nerd-Font-style status marks for
///   terminals with a patched font. trace:STORY-633
pub(crate) const THEMES: &[Theme] = &[
    Theme {
        name: "ascii",
        description: "Pure ASCII fallback — no unicode/emoji (good for plain terminals).",
        base: GlyphProfile::Ascii,
        bundle: &[],
    },
    Theme {
        name: "minimal",
        description: "Quiet monochrome — unicode marks, emoji collapsed to plain glyphs.",
        base: GlyphProfile::Unicode,
        bundle: &[
            (Glyph::Robot, "*"),
            (Glyph::Solo, "*"),
            (Glyph::Home, "@"),
            (Glyph::Away, "~"),
            (Glyph::Hourglass, "…"),
            (Glyph::Mailbox, "@"),
        ],
    },
    Theme {
        name: "nerd-font",
        description: "Heavy status marks for patched Nerd-Font terminals.",
        base: GlyphProfile::Unicode,
        bundle: &[
            (Glyph::Check, "✔"),
            (Glyph::Cross, "✖"),
            (Glyph::Blocked, "⚠"),
            (Glyph::Warning, "⚠"),
            (Glyph::Done, "●"),
            (Glyph::InFlight, "◑"),
            (Glyph::Bullet, "▪"),
        ],
    },
];

/// Look up an embedded theme by name (case-insensitive; `-`/`_` interchangeable).
/// `None` → unknown name. trace:STORY-633
pub(crate) fn theme_by_name(raw: &str) -> Option<&'static Theme> {
    let normalized = raw.trim().to_ascii_lowercase().replace('_', "-");
    THEMES
        .iter()
        .find(|t| t.name.replace('_', "-") == normalized)
}

/// The comma-separated valid theme names, for error messages. trace:STORY-633
pub(crate) fn valid_theme_names() -> String {
    THEMES.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
}

/// Read `[ui] theme` from a `config.toml`. Missing file / key / unknown theme →
/// `None` so the resolver falls through cleanly. trace:STORY-633
fn read_theme_from_config(config_path: &Path) -> Option<&'static Theme> {
    let body = std::fs::read_to_string(config_path).ok()?;
    let value: toml::Value = toml::from_str(&body).ok()?;
    let raw = value.get("ui").and_then(|t| t.get("theme"))?.as_str()?;
    theme_by_name(raw)
}

/// Resolve the active theme following the same project>user precedence as the
/// profile selector. `AIDA_GLYPHS` env does NOT name a theme (it only forces a
/// raw profile), so the env tier is skipped here. `None` → no theme set.
/// trace:STORY-633
pub(crate) fn active_theme(project_root: Option<&Path>) -> Option<&'static Theme> {
    if let Some(root) = project_root {
        let path = root.join(".aida").join("config.toml");
        if let Some(t) = read_theme_from_config(&path) {
            return Some(t);
        }
    }
    if let Some(home) = aida_home_dir() {
        let path = home.join(".aida").join("config.toml");
        if let Some(t) = read_theme_from_config(&path) {
            return Some(t);
        }
    }
    None
}

/// Render `glyph` honoring the FULL phase-4 precedence resolved from
/// `project_root`:
///
///   per-symbol `[glyphs]` override (project>user)
///     > active `[ui] theme` bundle (project>user)
///     > base `[ui] glyphs` profile (env>project>user)
///     > registry default.
///
/// This is the one resolver new render sites should call. trace:STORY-633
pub(crate) fn resolve_with_theme(glyph: Glyph, project_root: Option<&Path>) -> String {
    let overrides = GlyphOverrides::resolve(project_root);
    if let Some(custom) = overrides.get(glyph) {
        return custom.to_string();
    }
    if let Some(theme) = active_theme(project_root) {
        return theme.render(glyph);
    }
    glyph.render(active_profile(project_root)).to_string()
}

/// Resolve the active profile following the EPIC-45 precedence:
///
///   `AIDA_GLYPHS` env > project `.aida/config.toml [ui] glyphs`
///     > user `~/.aida/config.toml [ui] glyphs` > default UNICODE.
///
/// `project_root` is the directory containing `.aida/` (usually
/// [`crate::find_project_root`]); pass `None` when no project context is
/// available (the project tier is then skipped).
pub(crate) fn active_profile(project_root: Option<&Path>) -> GlyphProfile {
    // 1. Env (per-shell, highest priority — it's a per-terminal property).
    if let Some(raw) = std::env::var_os("AIDA_GLYPHS") {
        if let Some(p) = raw.to_str().and_then(GlyphProfile::parse) {
            return p;
        }
    }

    // 2. Project `.aida/config.toml [ui] glyphs`.
    if let Some(root) = project_root {
        let path = root.join(".aida").join("config.toml");
        if let Some(p) = read_glyphs_from_config(&path) {
            return p;
        }
    }

    // 3. User `~/.aida/config.toml [ui] glyphs`.
    if let Some(home) = aida_home_dir() {
        let path = home.join(".aida").join("config.toml");
        if let Some(p) = read_glyphs_from_config(&path) {
            return p;
        }
    }

    // 4. Default.
    GlyphProfile::Unicode
}

/// Fetch a glyph honoring the active profile resolved from `project_root`.
///
/// Convenience wrapper over [`active_profile`] + [`Glyph::render`]; callers
/// migrating a literal site call this. For tight loops that render many glyphs,
/// resolve [`active_profile`] once and call [`Glyph::render`] per glyph.
///
/// Provided for the phase-3 long-tail migration; phase 1's proof site
/// (`status_display`) resolves the profile once instead. trace:STORY-628
#[allow(dead_code)]
pub(crate) fn get(glyph: Glyph, project_root: Option<&Path>) -> &'static str {
    glyph.render(active_profile(project_root))
}

/// A custom per-symbol override table (phase 2 / STORY-629).
///
/// Maps a [`Glyph`] to an arbitrary replacement string. Loaded from the
/// `[glyphs]` section of `config.toml`, project layered over user. An override
/// for a symbol wins over the active profile's rendering for *that one symbol*;
/// symbols with no entry fall through to the profile (and then to the registry
/// default). An empty table = phase-1 behavior unchanged. trace:STORY-629
#[derive(Debug, Clone, Default)]
pub(crate) struct GlyphOverrides {
    map: HashMap<Glyph, String>,
}

impl GlyphOverrides {
    /// Resolve the custom override table following the same project>user
    /// precedence as the profile selector. The user tier is loaded first, then
    /// the project tier is overlaid on top so a project `[glyphs]` entry wins
    /// over the user's for the same symbol (while still inheriting the user's
    /// entries for symbols the project doesn't set). trace:STORY-629
    pub(crate) fn resolve(project_root: Option<&Path>) -> GlyphOverrides {
        let mut map: HashMap<Glyph, String> = HashMap::new();

        // 1. User `~/.aida/config.toml [glyphs]` (lowest precedence).
        if let Some(home) = aida_home_dir() {
            let path = home.join(".aida").join("config.toml");
            read_overrides_from_config(&path, &mut map);
        }

        // 2. Project `.aida/config.toml [glyphs]` overlaid on top — project
        //    entries replace user entries for the same symbol.
        if let Some(root) = project_root {
            let path = root.join(".aida").join("config.toml");
            read_overrides_from_config(&path, &mut map);
        }

        GlyphOverrides { map }
    }

    /// The custom override string for `glyph`, if any. `None` → no override,
    /// caller falls through to the profile. trace:STORY-629
    pub(crate) fn get(&self, glyph: Glyph) -> Option<&str> {
        self.map.get(&glyph).map(String::as_str)
    }

    /// `true` when no custom overrides are configured (phase-1 behavior).
    /// trace:STORY-629
    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Render `glyph` honoring this override table layered over `profile`:
    /// a custom `[glyphs]` entry wins; otherwise the profile's rendering.
    /// trace:STORY-629
    #[allow(dead_code)]
    pub(crate) fn render(&self, glyph: Glyph, profile: GlyphProfile) -> String {
        match self.get(glyph) {
            Some(custom) => custom.to_string(),
            None => glyph.render(profile).to_string(),
        }
    }
}

/// Fetch a glyph honoring BOTH the custom `[glyphs]` override table and the
/// active profile, resolved from `project_root`. Full precedence:
///
///   custom `[glyphs]` (project>user) > active profile (unicode|ascii)
///     > registry default.
///
/// Convenience wrapper for one-off sites; for tight loops resolve
/// [`GlyphOverrides::resolve`] + [`active_profile`] once and call
/// [`GlyphOverrides::render`] per glyph. trace:STORY-629
#[allow(dead_code)]
pub(crate) fn get_custom(glyph: Glyph, project_root: Option<&Path>) -> String {
    let overrides = GlyphOverrides::resolve(project_root);
    let profile = active_profile(project_root);
    overrides.render(glyph, profile)
}

/// Read the `[glyphs]` table from a `config.toml` into `map`, overwriting any
/// existing entries for the same symbol (so the caller controls precedence by
/// load order). Missing file / missing section / non-string values are skipped
/// cleanly; unknown keys are ignored (forward-compat). trace:STORY-629
fn read_overrides_from_config(config_path: &Path, map: &mut HashMap<Glyph, String>) {
    let Ok(body) = std::fs::read_to_string(config_path) else {
        return;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return;
    };
    let Some(table) = value.get("glyphs").and_then(|t| t.as_table()) else {
        return;
    };
    for (key, val) in table {
        if let (Some(glyph), Some(custom)) = (Glyph::from_name(key), val.as_str()) {
            map.insert(glyph, custom.to_string());
        }
    }
}

/// Read `[ui] glyphs` from a `config.toml`. Missing file / missing key /
/// unparseable value → `None` so the selector falls through cleanly.
fn read_glyphs_from_config(config_path: &Path) -> Option<GlyphProfile> {
    let body = std::fs::read_to_string(config_path).ok()?;
    let value: toml::Value = toml::from_str(&body).ok()?;
    let raw = value.get("ui").and_then(|t| t.get("glyphs"))?.as_str()?;
    GlyphProfile::parse(raw)
}

/// Home directory for the user-global `~/.aida/config.toml` tier. Honors the
/// test override so unit tests don't read the real home.
fn aida_home_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(home) = std::env::var_os("AIDA_TEST_HOME") {
        return Some(PathBuf::from(home));
    }
    dirs::home_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    // `AIDA_GLYPHS` / `AIDA_TEST_HOME` are process-global; serialize the tests
    // that mutate them so they don't race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn write_config(dir: &Path, glyphs: &str) {
        let aida = dir.join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        std::fs::write(
            aida.join("config.toml"),
            format!("[ui]\nglyphs = \"{glyphs}\"\n"),
        )
        .unwrap();
    }

    #[test]
    fn ascii_profile_returns_ascii_unicode_returns_emoji() {
        assert_eq!(Glyph::Check.render(GlyphProfile::Unicode), "✓");
        assert_eq!(Glyph::Check.render(GlyphProfile::Ascii), "[x]");
        assert_eq!(Glyph::Arrow.render(GlyphProfile::Unicode), "▸");
        assert_eq!(Glyph::Arrow.render(GlyphProfile::Ascii), "->");
        assert_eq!(Glyph::SubArrow.render(GlyphProfile::Unicode), "↳");
        assert_eq!(Glyph::SubArrow.render(GlyphProfile::Ascii), "\\_");
        assert_eq!(Glyph::Robot.render(GlyphProfile::Unicode), "🤖");
        assert_eq!(Glyph::Robot.render(GlyphProfile::Ascii), "[bot]");
    }

    /// TASK-835: the registry entries added for the status_display migration
    /// must reproduce the historical literals byte-for-byte under the default
    /// Unicode profile (the migration's correctness guarantee).
    #[test]
    fn task835_entries_match_historical_literals() {
        assert_eq!(Glyph::Done.unicode(), "◉");
        assert_eq!(Glyph::Neutral.unicode(), "·");
        assert_eq!(Glyph::FlowActive.unicode(), "▶");
        assert_eq!(Glyph::FlowBlocked.unicode(), "⊘");
        assert_eq!(Glyph::FlowQueued.unicode(), "↑");
        // ASCII fallbacks are single-display-column-friendly.
        assert_eq!(Glyph::Done.ascii(), "[*]");
        assert_eq!(Glyph::FlowActive.ascii(), ">");
        assert_eq!(Glyph::FlowBlocked.ascii(), "x");
        assert_eq!(Glyph::FlowQueued.ascii(), "^");
    }

    /// TASK-1071: the info/notice entries must reproduce the historical raw
    /// literals byte-for-byte under the default Unicode profile, and render a
    /// clean ASCII fallback under the ascii profile.
    #[test]
    fn task1071_info_notice_entries_match_historical_literals() {
        assert_eq!(Glyph::Info.unicode(), "ⓘ");
        assert_eq!(Glyph::InfoAlt.unicode(), "ℹ");
        assert_eq!(Glyph::Awaiting.unicode(), "⦿");
        assert_eq!(Glyph::IncomingMail.unicode(), "📨");
        assert_eq!(Glyph::Info.ascii(), "(i)");
        assert_eq!(Glyph::InfoAlt.ascii(), "i");
        assert_eq!(Glyph::Awaiting.ascii(), "(*)");
        assert_eq!(Glyph::IncomingMail.ascii(), "[@]");
        // Names round-trip through the config-key parser.
        assert_eq!(Glyph::from_name("info"), Some(Glyph::Info));
        assert_eq!(Glyph::from_name("info-alt"), Some(Glyph::InfoAlt));
        assert_eq!(Glyph::from_name("awaiting"), Some(Glyph::Awaiting));
        assert_eq!(Glyph::from_name("incoming_mail"), Some(Glyph::IncomingMail));
    }

    /// TASK-1071 acceptance: with `AIDA_GLYPHS=ascii` set, the rendered output
    /// (via the env → profile → render path, not `.ascii()` directly) emits the
    /// ASCII fallback for every glyph in the info/notice set rather than its raw
    /// unicode. This is the env-path guarantee that the raw-literal bug broke.
    #[test]
    fn task1071_ascii_env_renders_fallback_not_raw_unicode_for_info_set() {
        let _g = lock();
        std::env::set_var("AIDA_GLYPHS", "ascii");
        // Rendered through the active profile resolved from the env.
        assert_eq!(get(Glyph::Info, None), "(i)");
        assert_eq!(get(Glyph::InfoAlt, None), "i");
        assert_eq!(get(Glyph::Awaiting, None), "(*)");
        assert_eq!(get(Glyph::IncomingMail, None), "[@]");
        // And explicitly NOT the raw unicode literal.
        for g in [
            Glyph::Info,
            Glyph::InfoAlt,
            Glyph::Awaiting,
            Glyph::IncomingMail,
        ] {
            assert_ne!(
                get(g, None),
                g.unicode(),
                "{} rendered raw under ascii",
                g.name()
            );
        }
        // Round-trip back to unicode so the env override is directional.
        std::env::set_var("AIDA_GLYPHS", "unicode");
        assert_eq!(get(Glyph::Info, None), "ⓘ");
        assert_eq!(get(Glyph::IncomingMail, None), "📨");
        std::env::remove_var("AIDA_GLYPHS");
    }

    #[test]
    fn parse_is_case_insensitive_and_rejects_garbage() {
        assert_eq!(GlyphProfile::parse("ASCII"), Some(GlyphProfile::Ascii));
        assert_eq!(
            GlyphProfile::parse(" Unicode "),
            Some(GlyphProfile::Unicode)
        );
        assert_eq!(GlyphProfile::parse("nonsense"), None);
        assert_eq!(GlyphProfile::parse(""), None);
    }

    #[test]
    fn absent_config_defaults_to_unicode() {
        let _g = lock();
        std::env::remove_var("AIDA_GLYPHS");
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("AIDA_TEST_HOME", home.path());
        let proj = tempfile::tempdir().unwrap();
        // No config files anywhere.
        assert_eq!(active_profile(Some(proj.path())), GlyphProfile::Unicode);
        std::env::remove_var("AIDA_TEST_HOME");
    }

    #[test]
    fn env_wins_over_project_and_user() {
        let _g = lock();
        let home = tempfile::tempdir().unwrap();
        write_config(home.path(), "unicode");
        std::env::set_var("AIDA_TEST_HOME", home.path());
        let proj = tempfile::tempdir().unwrap();
        write_config(proj.path(), "unicode");
        std::env::set_var("AIDA_GLYPHS", "ascii");
        assert_eq!(active_profile(Some(proj.path())), GlyphProfile::Ascii);
        std::env::remove_var("AIDA_GLYPHS");
        std::env::remove_var("AIDA_TEST_HOME");
    }

    #[test]
    fn project_wins_over_user() {
        let _g = lock();
        std::env::remove_var("AIDA_GLYPHS");
        let home = tempfile::tempdir().unwrap();
        write_config(home.path(), "unicode");
        std::env::set_var("AIDA_TEST_HOME", home.path());
        let proj = tempfile::tempdir().unwrap();
        write_config(proj.path(), "ascii");
        assert_eq!(active_profile(Some(proj.path())), GlyphProfile::Ascii);
        std::env::remove_var("AIDA_TEST_HOME");
    }

    #[test]
    fn user_config_used_when_no_project_setting() {
        let _g = lock();
        std::env::remove_var("AIDA_GLYPHS");
        let home = tempfile::tempdir().unwrap();
        write_config(home.path(), "ascii");
        std::env::set_var("AIDA_TEST_HOME", home.path());
        // Project dir with no config → falls through to user tier.
        let proj = tempfile::tempdir().unwrap();
        assert_eq!(active_profile(Some(proj.path())), GlyphProfile::Ascii);
        std::env::remove_var("AIDA_TEST_HOME");
    }

    #[test]
    fn get_honors_active_profile() {
        let _g = lock();
        std::env::set_var("AIDA_GLYPHS", "ascii");
        assert_eq!(get(Glyph::Check, None), "[x]");
        std::env::set_var("AIDA_GLYPHS", "unicode");
        assert_eq!(get(Glyph::Check, None), "✓");
        std::env::remove_var("AIDA_GLYPHS");
    }

    // ----- Phase 2: custom [glyphs] override table (STORY-629) -----

    /// Write a `config.toml` with an explicit `[ui] glyphs` profile plus a
    /// `[glyphs]` override body (raw TOML lines, e.g. `check = "OK"`).
    fn write_config_with_glyphs(dir: &Path, profile: &str, glyphs_body: &str) {
        let aida = dir.join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        std::fs::write(
            aida.join("config.toml"),
            format!("[ui]\nglyphs = \"{profile}\"\n\n[glyphs]\n{glyphs_body}\n"),
        )
        .unwrap();
    }

    #[test]
    fn from_name_maps_canonical_and_hyphen_forms() {
        assert_eq!(Glyph::from_name("check"), Some(Glyph::Check));
        assert_eq!(Glyph::from_name("CHECK"), Some(Glyph::Check));
        assert_eq!(Glyph::from_name("sub_arrow"), Some(Glyph::SubArrow));
        assert_eq!(Glyph::from_name("sub-arrow"), Some(Glyph::SubArrow));
        assert_eq!(Glyph::from_name("in-flight"), Some(Glyph::InFlight));
        assert_eq!(Glyph::from_name("nonsense"), None);
    }

    #[test]
    fn every_glyph_name_round_trips() {
        for g in Glyph::ALL {
            assert_eq!(Glyph::from_name(g.name()), Some(g));
        }
    }

    #[test]
    fn override_wins_over_profile() {
        let _g = lock();
        std::env::remove_var("AIDA_GLYPHS");
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("AIDA_TEST_HOME", home.path());
        // Project on the ASCII profile, but custom-override `check`.
        let proj = tempfile::tempdir().unwrap();
        write_config_with_glyphs(proj.path(), "ascii", "check = \"OK\"");

        let overrides = GlyphOverrides::resolve(Some(proj.path()));
        let profile = active_profile(Some(proj.path()));
        assert_eq!(profile, GlyphProfile::Ascii);
        // Override beats the ascii profile rendering ("[x]").
        assert_eq!(overrides.render(Glyph::Check, profile), "OK");
        assert_eq!(get_custom(Glyph::Check, Some(proj.path())), "OK");

        std::env::remove_var("AIDA_TEST_HOME");
    }

    #[test]
    fn unset_symbol_falls_through_to_profile() {
        let _g = lock();
        std::env::remove_var("AIDA_GLYPHS");
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("AIDA_TEST_HOME", home.path());
        // ASCII profile, override ONLY `check` — `cross` must fall through.
        let proj = tempfile::tempdir().unwrap();
        write_config_with_glyphs(proj.path(), "ascii", "check = \"OK\"");

        let overrides = GlyphOverrides::resolve(Some(proj.path()));
        let profile = active_profile(Some(proj.path()));
        assert_eq!(overrides.render(Glyph::Check, profile), "OK");
        // `cross` has no override → ascii profile rendering.
        assert_eq!(overrides.get(Glyph::Cross), None);
        assert_eq!(overrides.render(Glyph::Cross, profile), "[ ]");
        assert_eq!(get_custom(Glyph::Cross, Some(proj.path())), "[ ]");

        std::env::remove_var("AIDA_TEST_HOME");
    }

    #[test]
    fn unset_symbol_falls_through_to_unicode_default() {
        let _g = lock();
        std::env::remove_var("AIDA_GLYPHS");
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("AIDA_TEST_HOME", home.path());
        // Default (unicode) profile, override only `check`.
        let proj = tempfile::tempdir().unwrap();
        write_config_with_glyphs(proj.path(), "unicode", "check = \"OK\"");

        let overrides = GlyphOverrides::resolve(Some(proj.path()));
        let profile = active_profile(Some(proj.path()));
        assert_eq!(profile, GlyphProfile::Unicode);
        assert_eq!(overrides.render(Glyph::Check, profile), "OK");
        // Unset `warning` falls through to the unicode default.
        assert_eq!(overrides.render(Glyph::Warning, profile), "⚠");

        std::env::remove_var("AIDA_TEST_HOME");
    }

    #[test]
    fn project_override_beats_user_override() {
        let _g = lock();
        std::env::remove_var("AIDA_GLYPHS");
        // User sets check + warning; project overrides ONLY check.
        let home = tempfile::tempdir().unwrap();
        write_config_with_glyphs(home.path(), "unicode", "check = \"USER\"\nwarning = \"UW\"");
        std::env::set_var("AIDA_TEST_HOME", home.path());
        let proj = tempfile::tempdir().unwrap();
        write_config_with_glyphs(proj.path(), "unicode", "check = \"PROJ\"");

        let overrides = GlyphOverrides::resolve(Some(proj.path()));
        // Project wins for `check`.
        assert_eq!(overrides.get(Glyph::Check), Some("PROJ"));
        // User entry survives for a symbol the project didn't override.
        assert_eq!(overrides.get(Glyph::Warning), Some("UW"));

        std::env::remove_var("AIDA_TEST_HOME");
    }

    #[test]
    fn absent_glyphs_section_is_phase1_behavior() {
        let _g = lock();
        std::env::remove_var("AIDA_GLYPHS");
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("AIDA_TEST_HOME", home.path());
        // Only a [ui] section, no [glyphs] — overrides empty, falls to profile.
        let proj = tempfile::tempdir().unwrap();
        write_config(proj.path(), "ascii");

        let overrides = GlyphOverrides::resolve(Some(proj.path()));
        assert!(overrides.is_empty());
        let profile = active_profile(Some(proj.path()));
        assert_eq!(overrides.render(Glyph::Check, profile), "[x]");
        assert_eq!(get_custom(Glyph::Check, Some(proj.path())), "[x]");

        std::env::remove_var("AIDA_TEST_HOME");
    }

    #[test]
    fn unknown_override_key_is_ignored() {
        let _g = lock();
        std::env::remove_var("AIDA_GLYPHS");
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("AIDA_TEST_HOME", home.path());
        let proj = tempfile::tempdir().unwrap();
        write_config_with_glyphs(proj.path(), "unicode", "check = \"OK\"\nbogus = \"X\"");

        let overrides = GlyphOverrides::resolve(Some(proj.path()));
        assert_eq!(overrides.get(Glyph::Check), Some("OK"));
        // Only the valid key landed.
        assert_eq!(overrides.map.len(), 1);

        std::env::remove_var("AIDA_TEST_HOME");
    }

    // ----- Phase 4: themes + full precedence (STORY-633) -----

    /// Write a `config.toml` with an explicit `[ui]` block (raw body lines) and
    /// optionally a `[glyphs]` override body.
    fn write_config_raw(dir: &Path, ui_body: &str, glyphs_body: Option<&str>) {
        let aida = dir.join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        let mut body = format!("[ui]\n{ui_body}\n");
        if let Some(g) = glyphs_body {
            body.push_str(&format!("\n[glyphs]\n{g}\n"));
        }
        std::fs::write(aida.join("config.toml"), body).unwrap();
    }

    #[test]
    fn theme_lookup_is_case_and_separator_insensitive() {
        assert_eq!(theme_by_name("ascii").map(|t| t.name), Some("ascii"));
        assert_eq!(
            theme_by_name("NERD_FONT").map(|t| t.name),
            Some("nerd-font")
        );
        assert_eq!(
            theme_by_name("nerd-font").map(|t| t.name),
            Some("nerd-font")
        );
        assert_eq!(theme_by_name("minimal").map(|t| t.name), Some("minimal"));
        assert!(theme_by_name("does-not-exist").is_none());
    }

    #[test]
    fn theme_render_bundle_wins_else_base() {
        let ascii = theme_by_name("ascii").unwrap();
        // ascii theme has an empty bundle → base ASCII profile rendering.
        assert_eq!(ascii.render(Glyph::Check), "[x]");
        let nerd = theme_by_name("nerd-font").unwrap();
        // bundle covers check; base is unicode.
        assert_eq!(nerd.render(Glyph::Check), "✔");
        // not in bundle → unicode base default.
        assert_eq!(nerd.render(Glyph::Arrow), "▸");
    }

    /// The headline precedence pin: override > theme > profile > default.
    #[test]
    fn full_precedence_override_beats_theme_beats_profile_beats_default() {
        let _g = lock();
        std::env::remove_var("AIDA_GLYPHS");
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("AIDA_TEST_HOME", home.path());
        let proj = tempfile::tempdir().unwrap();
        // Base profile ascii, theme nerd-font (overrides check→✔), and a
        // per-symbol [glyphs] override check→OK.
        write_config_raw(
            proj.path(),
            "glyphs = \"ascii\"\ntheme = \"nerd-font\"",
            Some("check = \"OK\""),
        );

        // check: per-symbol override wins over the theme bundle + profile.
        assert_eq!(resolve_with_theme(Glyph::Check, Some(proj.path())), "OK");
        // arrow: no override, theme nerd-font has no arrow entry → theme's
        // unicode BASE default, NOT the ascii [ui] glyphs profile ("->").
        assert_eq!(resolve_with_theme(Glyph::Arrow, Some(proj.path())), "▸");
        // done: theme bundle entry beats profile.
        assert_eq!(resolve_with_theme(Glyph::Done, Some(proj.path())), "●");

        std::env::remove_var("AIDA_TEST_HOME");
    }

    #[test]
    fn no_theme_falls_through_to_profile_then_default() {
        let _g = lock();
        std::env::remove_var("AIDA_GLYPHS");
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("AIDA_TEST_HOME", home.path());
        let proj = tempfile::tempdir().unwrap();
        // Only a profile, no theme, no overrides.
        write_config_raw(proj.path(), "glyphs = \"ascii\"", None);
        assert!(active_theme(Some(proj.path())).is_none());
        assert_eq!(resolve_with_theme(Glyph::Check, Some(proj.path())), "[x]");

        // Empty project → unicode default.
        let bare = tempfile::tempdir().unwrap();
        assert_eq!(resolve_with_theme(Glyph::Check, Some(bare.path())), "✓");

        std::env::remove_var("AIDA_TEST_HOME");
    }

    #[test]
    fn project_theme_beats_user_theme() {
        let _g = lock();
        std::env::remove_var("AIDA_GLYPHS");
        let home = tempfile::tempdir().unwrap();
        write_config_raw(home.path(), "theme = \"ascii\"", None);
        std::env::set_var("AIDA_TEST_HOME", home.path());
        let proj = tempfile::tempdir().unwrap();
        write_config_raw(proj.path(), "theme = \"nerd-font\"", None);

        assert_eq!(
            active_theme(Some(proj.path())).map(|t| t.name),
            Some("nerd-font")
        );
        std::env::remove_var("AIDA_TEST_HOME");
    }
}
