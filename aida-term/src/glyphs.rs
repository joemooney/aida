// trace:TASK-3429 | ai:codex
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
pub enum Glyph {
    /// Success / completed (✓).
    Check,
    /// Failure / rejected (✗).
    Cross,
    /// Draft / not-started (○).
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
    /// bold status in the status display. trace:TASK-835
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
    // trace:TASK-1234 | ai:codex
    /// Work-routing: queued, but fenced to guided/operator/decide so a headless
    /// drain skips it (⇈).
    FlowQueuedSupervised,
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
    // trace:BUG-781 | ai:claude
    /// Ratified / accepted (☑) — the terminal state of a decision (ADR) spec,
    /// where `Approved` means ACCEPTED rather than "cleared to start work".
    /// A checked box, deliberately distinct from [`Glyph::Check`] (✓, merged
    /// work) so a decided ADR does not read as a shipped task.
    Accepted,
    // trace:TASK-1176 | ai:claude
    /// Superseded (⊡) — adopted, then replaced by a successor spec. In the same
    /// box family as [`Glyph::Accepted`] (☑) because the spec WAS adopted, but
    /// with the check replaced by a dot: closed, and no longer the live copy.
    /// Deliberately not [`Glyph::Cross`] (✗) — that reads "declined".
    Superseded,
}

impl Glyph {
    /// The default UNICODE rendering. This is what ships today and stays the
    /// default — capable terminals keep the emoji.
    pub const fn unicode(self) -> &'static str {
        match self {
            Glyph::Check => "✓",
            Glyph::Cross => "✗",
            // trace:BUG-1018 | ai:codex
            Glyph::Pending => "○",
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
            Glyph::FlowQueuedSupervised => "⇈",
            Glyph::Pause => "⏸",
            Glyph::Info => "ⓘ",
            Glyph::InfoAlt => "ℹ",
            Glyph::Awaiting => "⦿",
            Glyph::IncomingMail => "📨",
            Glyph::Accepted => "☑",
            // trace:TASK-1176 | ai:claude
            Glyph::Superseded => "⊡",
        }
    }

    /// The curated ASCII fallback. Chosen to stay clean and column-aligned in
    /// tables (mostly 1-3 ASCII columns).
    pub const fn ascii(self) -> &'static str {
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
            Glyph::FlowQueuedSupervised => "^^",
            Glyph::Pause => "[paused]",
            Glyph::Info => "(i)",
            Glyph::InfoAlt => "i",
            Glyph::Awaiting => "(*)",
            Glyph::IncomingMail => "[@]",
            Glyph::Accepted => "[+]",
            // trace:TASK-1176 | ai:claude
            Glyph::Superseded => "[=]",
        }
    }

    /// Render this glyph for an explicit [`GlyphProfile`].
    pub const fn render(self, profile: GlyphProfile) -> &'static str {
        match profile {
            GlyphProfile::Unicode => self.unicode(),
            GlyphProfile::Ascii => self.ascii(),
        }
    }

    /// The canonical name of this glyph — the key used in the `[glyphs]`
    /// custom-override config table (phase 2 / STORY-629). These match the
    /// lower-snake-case form of the variant names so a config like
    /// `[glyphs]\ncheck = "OK"` targets [`Glyph::Check`]. trace:STORY-629
    pub const fn name(self) -> &'static str {
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
            Glyph::FlowQueuedSupervised => "flow_queued_supervised",
            Glyph::Pause => "pause",
            Glyph::Info => "info",
            Glyph::InfoAlt => "info_alt",
            Glyph::Awaiting => "awaiting",
            Glyph::IncomingMail => "incoming_mail",
            Glyph::Accepted => "accepted",
            // trace:TASK-1176 | ai:claude
            Glyph::Superseded => "superseded",
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
    pub fn from_name(raw: &str) -> Option<Glyph> {
        let normalized = raw.trim().to_ascii_lowercase().replace('-', "_");
        Glyph::ALL.iter().copied().find(|g| g.name() == normalized)
    }

    /// Every variant, for iteration (name parsing, exhaustive tests).
    /// trace:STORY-629
    pub const ALL: [Glyph; 29] = [
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
        Glyph::FlowQueuedSupervised,
        Glyph::Pause,
        Glyph::Info,
        Glyph::InfoAlt,
        Glyph::Awaiting,
        Glyph::IncomingMail,
        Glyph::Accepted,
        // trace:TASK-1176 | ai:claude
        Glyph::Superseded,
    ];
}

/// Render a standalone terminal marker with protective trailing spaces.
/// Keep raw registry values unpadded for JSON, configuration and width calculations.
/// ASCII markers need one separator; wide or East-Asian ambiguous symbols need two.
// trace:TASK-3779 | ai:codex
pub fn terminal_marker(marker: &str) -> String {
    use unicode_width::UnicodeWidthStr;
    let marker = marker.trim_end_matches(' ');
    let padding = if marker.width_cjk() >= 2 && !marker.is_ascii() {
        "  "
    } else {
        " "
    };
    format!("{marker}{padding}")
}

#[cfg(test)]
mod terminal_marker_tests {
    use super::terminal_marker;

    // trace:TASK-3779 | ai:codex
    #[test]
    fn terminal_marker_padding() {
        assert_eq!(terminal_marker("OK"), "OK ");
        assert_eq!(terminal_marker("✓"), "✓ ");
        assert_eq!(terminal_marker("🤖"), "🤖  ");
        assert_eq!(terminal_marker("⚠"), "⚠ ");
        assert_eq!(terminal_marker("▶"), "▶  ");
        assert_eq!(terminal_marker("🤖  "), "🤖  ");
        assert_eq!(terminal_marker("[bot]"), "[bot] ");
    }
}

/// Which rendering profile is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GlyphProfile {
    /// Default — emoji/unicode preserved for capable terminals.
    #[default]
    Unicode,
    /// Curated ASCII fallback.
    Ascii,
}

impl GlyphProfile {
    /// Parse a profile name (`unicode` / `ascii`, case-insensitive). Unknown /
    /// empty → `None` so callers can fall through to the next precedence tier.
    pub fn parse(raw: &str) -> Option<GlyphProfile> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "unicode" | "emoji" | "utf8" | "utf-8" => Some(GlyphProfile::Unicode),
            "ascii" | "plain" => Some(GlyphProfile::Ascii),
            _ => None,
        }
    }

    /// Canonical profile name for display (e.g. in `aida config show`).
    /// trace:TASK-793 | ai:claude
    pub const fn name(self) -> &'static str {
        match self {
            GlyphProfile::Unicode => "unicode",
            GlyphProfile::Ascii => "ascii",
        }
    }
}

/// Line-art glyphs for rendered markdown: heading rules, the quote / code
/// gutter, and table borders. Selected by [`GlyphProfile`] like the registry.
///
/// Deliberately NOT [`Glyph`] variants: these are layout chrome whose width
/// must stay exactly one column (a table border is repeated per column, so a
/// custom `[glyphs]` override like `"--"` would break alignment), and adding
/// box-drawing to [`Glyph::ALL`] would make the skill-body glyph count and
/// `aida config glyph list` treat ordinary box-drawing text as registry
/// glyphs.
///
/// The markdown renderer picks the line art AND its list bullet
/// ([`Glyph::Bullet`] rendered for the same profile) from the profile alone:
/// per-symbol `[glyphs]` overrides and theme bundles do not apply to rendered
/// markdown. A configured `[ui] theme` contributes only its base profile.
// trace:BUG-1979 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineArt {
    /// Level-1 heading underline (═ / =).
    pub rule_heavy: &'static str,
    /// Level-2 heading underline, thematic break, table horizontal (─ / -).
    pub rule_light: &'static str,
    /// Blockquote bar, code gutter, table cell separator (│ / |).
    pub vertical: &'static str,
    /// Table top border: left corner, column junction, right corner.
    pub top: [&'static str; 3],
    /// Table header separator: left tee, cross, right tee.
    pub middle: [&'static str; 3],
    /// Table bottom border: left corner, column junction, right corner.
    pub bottom: [&'static str; 3],
}

impl LineArt {
    /// Box-drawing line art — the historical literals, byte for byte.
    pub const UNICODE: LineArt = LineArt {
        rule_heavy: "═",
        rule_light: "─",
        vertical: "│",
        top: ["┌", "┬", "┐"],
        middle: ["├", "┼", "┤"],
        bottom: ["└", "┴", "┘"],
    };

    /// ASCII line art for terminals that cannot draw box characters.
    pub const ASCII: LineArt = LineArt {
        rule_heavy: "=",
        rule_light: "-",
        vertical: "|",
        top: ["+", "+", "+"],
        middle: ["+", "+", "+"],
        bottom: ["+", "+", "+"],
    };

    /// The line art for `profile`.
    pub const fn for_profile(profile: GlyphProfile) -> LineArt {
        match profile {
            GlyphProfile::Unicode => LineArt::UNICODE,
            GlyphProfile::Ascii => LineArt::ASCII,
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
pub struct Theme {
    /// The reference name written to `[ui] theme` (lowercase, hyphen-friendly).
    pub name: &'static str,
    /// One-line human description for `aida config glyph theme list`.
    pub description: &'static str,
    /// The base profile the theme starts from before its bundle is applied.
    pub base: GlyphProfile,
    /// Per-symbol override bundle: `(Glyph, replacement)` pairs layered over the
    /// base profile. An empty bundle = "just the base profile, named".
    pub bundle: &'static [(Glyph, &'static str)],
}

impl Theme {
    /// Render `glyph` under this theme: the bundle wins for a symbol it covers,
    /// else the base profile's rendering. trace:STORY-633
    pub fn render(&self, glyph: Glyph) -> String {
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
pub const THEMES: &[Theme] = &[
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
pub fn theme_by_name(raw: &str) -> Option<&'static Theme> {
    let normalized = raw.trim().to_ascii_lowercase().replace('_', "-");
    THEMES
        .iter()
        .find(|t| t.name.replace('_', "-") == normalized)
}

/// The comma-separated valid theme names, for error messages. trace:STORY-633
pub fn valid_theme_names() -> String {
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
pub fn active_theme(project_root: Option<&Path>) -> Option<&'static Theme> {
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
pub fn resolve_with_theme(glyph: Glyph, project_root: Option<&Path>) -> String {
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
/// the discovered project root); pass `None` when no project context is
/// available (the project tier is then skipped).
pub fn active_profile(project_root: Option<&Path>) -> GlyphProfile {
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
pub fn get(glyph: Glyph, project_root: Option<&Path>) -> &'static str {
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
pub struct GlyphOverrides {
    map: HashMap<Glyph, String>,
}

impl GlyphOverrides {
    /// Resolve the custom override table following the same project>user
    /// precedence as the profile selector. The user tier is loaded first, then
    /// the project tier is overlaid on top so a project `[glyphs]` entry wins
    /// over the user's for the same symbol (while still inheriting the user's
    /// entries for symbols the project doesn't set). trace:STORY-629
    pub fn resolve(project_root: Option<&Path>) -> GlyphOverrides {
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
    pub fn get(&self, glyph: Glyph) -> Option<&str> {
        self.map.get(&glyph).map(String::as_str)
    }

    /// `true` when no custom overrides are configured (phase-1 behavior).
    /// trace:STORY-629
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Render `glyph` honoring this override table layered over `profile`:
    /// a custom `[glyphs]` entry wins; otherwise the profile's rendering.
    /// trace:STORY-629
    #[allow(dead_code)]
    pub fn render(&self, glyph: Glyph, profile: GlyphProfile) -> String {
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
pub fn get_custom(glyph: Glyph, project_root: Option<&Path>) -> String {
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
    aida_core::home::home_dir()
}
