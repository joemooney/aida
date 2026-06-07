//! TUI theme system — named color roles + a small set of built-in
//! palettes (TASK-256, minimal first slice).
//!
//! The TUI previously hard-coded ratatui [`Color`] variants (`Color::Cyan`
//! for selection, `Color::DarkGray` for dim text, …) at every render
//! call. That made the palette un-tweakable and the look uniform across
//! terminals. This module introduces a [`Theme`] — a struct of *named
//! color roles* (bg / fg / accent / dim / border / on_accent / error /
//! warn / info) — and a handful of built-in themes, so render code asks
//! for `theme.accent` rather than naming a literal color.
//!
//! Scope note (TASK-256 minimal slice, operator-decided 2026-06-06): this
//! ships the `Theme` struct + role routing + three built-in themes
//! (Catppuccin Mocha as the default reference, plus a standard Dark and
//! Light). The full seven-theme matrix (the other Catppuccin variants +
//! Nord), the `Ctrl-A T` live picker, config persistence on selection,
//! and golden-image snapshot tests are deliberately deferred to
//! follow-ups. The struct shape and [`ThemeName::from_config_str`]
//! selector are designed so adding the remaining palettes is a pure data
//! change.
//!
//! trace:TASK-256 | ai:claude

use ratatui::style::Color;

/// A resolved palette: every UI element asks for one of these *roles*
/// rather than naming a literal color, so swapping a theme is a single
/// lookup table change. trace:TASK-256 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Default text foreground.
    pub fg: Color,
    /// Default surface background (used sparingly — most widgets inherit
    /// the terminal background).
    pub bg: Color,
    /// Primary accent — selection background, active-tab background,
    /// inline emphasis.
    pub accent: Color,
    /// Foreground to pair *on top of* [`Self::accent`] (selected rows /
    /// active tab text).
    pub on_accent: Color,
    /// De-emphasized text — hints, dividers, placeholder lines.
    pub dim: Color,
    /// Block borders and rules.
    pub border: Color,
    /// Error / failure indicators.
    pub error: Color,
    /// Warning / caution indicators.
    pub warn: Color,
    /// Informational / success-positive indicators.
    pub info: Color,
}

/// The set of built-in themes the minimal slice ships. The remaining
/// Catppuccin variants (Macchiato / Frappé / Latte) and Nord are deferred
/// follow-ups; adding them is a new arm here plus a [`Theme`] constant.
/// trace:TASK-256 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeName {
    /// Catppuccin Mocha — the default reference palette (soft pastels on a
    /// deep base). trace:TASK-256 | ai:claude
    #[default]
    CatppuccinMocha,
    /// Standard high-contrast dark.
    Dark,
    /// Standard light.
    Light,
}

impl ThemeName {
    /// Stable config token for this theme (what lands in
    /// `[tui] theme = "..."`).
    pub fn as_config_str(self) -> &'static str {
        match self {
            ThemeName::CatppuccinMocha => "catppuccin-mocha",
            ThemeName::Dark => "dark",
            ThemeName::Light => "light",
        }
    }

    /// Parse a `[tui] theme = "..."` token. Case-insensitive; tolerant of
    /// `_` / space separators. Unknown tokens return `None` so the caller
    /// keeps its default rather than failing the launch.
    /// trace:TASK-256 | ai:claude
    pub fn from_config_str(spec: &str) -> Option<ThemeName> {
        let norm = spec.trim().to_ascii_lowercase().replace([' ', '_'], "-");
        match norm.as_str() {
            "catppuccin-mocha" | "mocha" | "catppuccin" => Some(ThemeName::CatppuccinMocha),
            "dark" => Some(ThemeName::Dark),
            "light" => Some(ThemeName::Light),
            _ => None,
        }
    }

    /// The resolved [`Theme`] palette for this name.
    pub fn theme(self) -> Theme {
        match self {
            ThemeName::CatppuccinMocha => CATPPUCCIN_MOCHA,
            ThemeName::Dark => DARK,
            ThemeName::Light => LIGHT,
        }
    }
}

impl Theme {
    /// Resolve a theme by config token, falling back to the default
    /// ([`ThemeName::CatppuccinMocha`]) for a missing / unknown token.
    /// trace:TASK-256 | ai:claude
    pub fn from_config_str(spec: &str) -> Theme {
        ThemeName::from_config_str(spec).unwrap_or_default().theme()
    }
}

impl Default for Theme {
    fn default() -> Self {
        ThemeName::default().theme()
    }
}

// --- Built-in palettes -----------------------------------------------------

/// Catppuccin Mocha (https://catppuccin.com/palette) — the default.
/// Hex values are the canonical Mocha palette. trace:TASK-256 | ai:claude
pub const CATPPUCCIN_MOCHA: Theme = Theme {
    fg: Color::Rgb(0xCD, 0xD6, 0xF4),        // Text
    bg: Color::Rgb(0x1E, 0x1E, 0x2E),        // Base
    accent: Color::Rgb(0x89, 0xB4, 0xFA),    // Blue
    on_accent: Color::Rgb(0x1E, 0x1E, 0x2E), // Base (text on the accent fill)
    dim: Color::Rgb(0x6C, 0x70, 0x86),       // Overlay0
    border: Color::Rgb(0x58, 0x5B, 0x70),    // Surface2
    error: Color::Rgb(0xF3, 0x8B, 0xA8),     // Red
    warn: Color::Rgb(0xF9, 0xE2, 0xAF),      // Yellow
    info: Color::Rgb(0xA6, 0xE3, 0xA1),      // Green
};

/// Standard high-contrast dark — the conservative alpha-safe palette,
/// built from the 16-color ANSI set so it renders identically on any
/// terminal. trace:TASK-256 | ai:claude
pub const DARK: Theme = Theme {
    fg: Color::White,
    bg: Color::Black,
    accent: Color::Cyan,
    on_accent: Color::Black,
    dim: Color::DarkGray,
    border: Color::Gray,
    error: Color::Red,
    warn: Color::Yellow,
    info: Color::Green,
};

/// Standard light — ANSI-set palette tuned for a light terminal
/// background. trace:TASK-256 | ai:claude
pub const LIGHT: Theme = Theme {
    fg: Color::Black,
    bg: Color::White,
    accent: Color::Blue,
    on_accent: Color::White,
    dim: Color::Gray,
    border: Color::DarkGray,
    error: Color::Red,
    warn: Color::Rgb(0xB5, 0x89, 0x00), // a darker yellow legible on white
    info: Color::Green,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_is_catppuccin_mocha() {
        assert_eq!(ThemeName::default(), ThemeName::CatppuccinMocha);
        assert_eq!(Theme::default(), CATPPUCCIN_MOCHA);
    }

    #[test]
    fn from_config_str_parses_known_tokens() {
        assert_eq!(
            ThemeName::from_config_str("catppuccin-mocha"),
            Some(ThemeName::CatppuccinMocha)
        );
        assert_eq!(
            ThemeName::from_config_str("MOCHA"),
            Some(ThemeName::CatppuccinMocha)
        );
        assert_eq!(
            ThemeName::from_config_str("catppuccin"),
            Some(ThemeName::CatppuccinMocha)
        );
        assert_eq!(ThemeName::from_config_str("dark"), Some(ThemeName::Dark));
        assert_eq!(ThemeName::from_config_str("Light"), Some(ThemeName::Light));
    }

    #[test]
    fn from_config_str_tolerates_separators() {
        assert_eq!(
            ThemeName::from_config_str("catppuccin_mocha"),
            Some(ThemeName::CatppuccinMocha)
        );
        assert_eq!(
            ThemeName::from_config_str(" catppuccin mocha "),
            Some(ThemeName::CatppuccinMocha)
        );
    }

    #[test]
    fn unknown_token_falls_back_to_default_theme() {
        assert_eq!(ThemeName::from_config_str("nord"), None);
        assert_eq!(ThemeName::from_config_str(""), None);
        // The lenient resolver picks the default for an unknown token.
        assert_eq!(Theme::from_config_str("nord"), CATPPUCCIN_MOCHA);
        assert_eq!(Theme::from_config_str("dark"), DARK);
    }

    #[test]
    fn as_config_str_round_trips() {
        for name in [
            ThemeName::CatppuccinMocha,
            ThemeName::Dark,
            ThemeName::Light,
        ] {
            assert_eq!(ThemeName::from_config_str(name.as_config_str()), Some(name));
        }
    }

    #[test]
    fn each_theme_assigns_distinct_load_bearing_roles() {
        // A theme that accidentally collapses selection text into its own
        // fill (or foreground into background) is a render bug; assert the
        // load-bearing contrasts differ so a future palette edit can't
        // silently make a region unreadable.
        for theme in [CATPPUCCIN_MOCHA, DARK, LIGHT] {
            assert_ne!(theme.accent, theme.on_accent);
            assert_ne!(theme.fg, theme.bg);
            assert_ne!(theme.fg, theme.dim);
        }
    }
}
