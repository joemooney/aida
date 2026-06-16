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
        }
    }

    /// Render this glyph for an explicit [`GlyphProfile`].
    pub(crate) const fn render(self, profile: GlyphProfile) -> &'static str {
        match profile {
            GlyphProfile::Unicode => self.unicode(),
            GlyphProfile::Ascii => self.ascii(),
        }
    }
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
}
