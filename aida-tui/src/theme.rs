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

    // --- Semantic status colors (STORY-691) ---------------------------------
    // The preview pane color-codes a spec's structured status field with the
    // SAME semantics the CLI uses (aida-cli/src/status_display.rs paint_status),
    // ported from the `colored` crate to ratatui [`Color`]. These live on the
    // theme so the operator customizes them by switching `[tui] theme = "..."`
    // — they resolve through the existing palette, not a hardcoded parallel map.
    // trace:STORY-691 | ai:claude
    /// Draft — de-emphasized (CLI: dimmed).
    pub status_draft: Color,
    /// Approved — CLI: cyan.
    pub status_approved: Color,
    /// Planned — CLI: blue.
    pub status_planned: Color,
    /// In Progress — CLI: yellow.
    pub status_in_progress: Color,
    /// Done — "finished on a branch"; CLI: bright green (bold).
    pub status_done: Color,
    /// Completed — "merged to main"; CLI: green.
    pub status_completed: Color,
    /// Rejected — CLI: red.
    pub status_rejected: Color,
    /// Needs Attention — a punted spec; CLI: magenta (bold).
    pub status_needs_attention: Color,

    // --- Semantic priority colors (STORY-691) -------------------------------
    /// High priority.
    pub priority_high: Color,
    /// Medium priority.
    pub priority_medium: Color,
    /// Low priority.
    pub priority_low: Color,
}

/// Collapse a status/priority string to a bare match key: lowercase with
/// whitespace, `-` and `_` stripped, so "In Progress", "in-progress" and a
/// column-padded "Approved   " all resolve to the same arm. Mirrors the CLI's
/// `status_display::normalize`.
//
// trace:STORY-691 | ai:claude
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

impl Theme {
    /// The themeable semantic color for a requirement `status`, matching the
    /// CLI palette (`status_display::paint_status`) but resolved through this
    /// theme's slots so a custom palette recolors it. An unknown / custom
    /// status falls back to the default [`Self::fg`] (the neutral text color),
    /// mirroring the CLI's `_ => text.normal()` arm.
    //
    // trace:STORY-691 | ai:claude
    pub fn status_color(&self, status: &str) -> Color {
        match normalize(status).as_str() {
            "draft" => self.status_draft,
            "approved" => self.status_approved,
            "planned" => self.status_planned,
            "inprogress" => self.status_in_progress,
            "done" => self.status_done,
            "completed" => self.status_completed,
            "rejected" => self.status_rejected,
            "needsattention" => self.status_needs_attention,
            _ => self.fg,
        }
    }

    /// The themeable semantic color for a `priority` label. Unknown priorities
    /// fall back to [`Self::fg`].
    //
    // trace:STORY-691 | ai:claude
    pub fn priority_color(&self, priority: &str) -> Color {
        match normalize(priority).as_str() {
            "high" => self.priority_high,
            "medium" | "med" => self.priority_medium,
            "low" => self.priority_low,
            _ => self.fg,
        }
    }
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
    // Status semantics (STORY-691) — Mocha palette tints matching the CLI map.
    status_draft: Color::Rgb(0x6C, 0x70, 0x86), // Overlay0 (dim)
    status_approved: Color::Rgb(0x89, 0xDC, 0xEB), // Sky (cyan)
    status_planned: Color::Rgb(0x89, 0xB4, 0xFA), // Blue
    status_in_progress: Color::Rgb(0xF9, 0xE2, 0xAF), // Yellow
    status_done: Color::Rgb(0xA6, 0xE3, 0xA1),  // Green (bright)
    status_completed: Color::Rgb(0x40, 0xA0, 0x2B), // darker Green
    status_rejected: Color::Rgb(0xF3, 0x8B, 0xA8), // Red
    status_needs_attention: Color::Rgb(0xCB, 0xA6, 0xF7), // Mauve (magenta)
    priority_high: Color::Rgb(0xF3, 0x8B, 0xA8), // Red
    priority_medium: Color::Rgb(0xF9, 0xE2, 0xAF), // Yellow
    priority_low: Color::Rgb(0x6C, 0x70, 0x86), // Overlay0 (dim)
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
    // Status semantics (STORY-691) — ANSI set, matching the CLI literal map.
    status_draft: Color::DarkGray,
    status_approved: Color::Cyan,
    status_planned: Color::Blue,
    status_in_progress: Color::Yellow,
    status_done: Color::LightGreen,
    status_completed: Color::Green,
    status_rejected: Color::Red,
    status_needs_attention: Color::Magenta,
    priority_high: Color::Red,
    priority_medium: Color::Yellow,
    priority_low: Color::DarkGray,
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
    // Status semantics (STORY-691) — tuned to stay legible on a light bg.
    status_draft: Color::Gray,
    status_approved: Color::Rgb(0x00, 0x80, 0x80), // teal (cyan, darkened)
    status_planned: Color::Blue,
    status_in_progress: Color::Rgb(0xB5, 0x89, 0x00), // darker yellow
    status_done: Color::Rgb(0x18, 0x80, 0x18),        // bright-ish green
    status_completed: Color::Green,
    status_rejected: Color::Red,
    status_needs_attention: Color::Magenta,
    priority_high: Color::Red,
    priority_medium: Color::Rgb(0xB5, 0x89, 0x00), // darker yellow
    priority_low: Color::Gray,
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

    // STORY-691: the structured-preview status→color and priority→color maps.
    #[test]
    fn status_color_matches_cli_semantics() {
        // Each canonical status resolves to its dedicated themed slot — and
        // the slots are the same colors the CLI's `paint_status` paints
        // (Approved=cyan, In Progress=yellow, Completed=green, Rejected=red,
        // Needs Attention=magenta), here on the conservative ANSI Dark theme.
        let t = DARK;
        assert_eq!(t.status_color("Draft"), Color::DarkGray);
        assert_eq!(t.status_color("Approved"), Color::Cyan);
        assert_eq!(t.status_color("Planned"), Color::Blue);
        assert_eq!(t.status_color("In Progress"), Color::Yellow);
        assert_eq!(t.status_color("Done"), Color::LightGreen);
        assert_eq!(t.status_color("Completed"), Color::Green);
        assert_eq!(t.status_color("Rejected"), Color::Red);
        assert_eq!(t.status_color("Needs Attention"), Color::Magenta);
    }

    #[test]
    fn status_color_normalizes_spelling() {
        // The status label reaches the preview in several spellings; all
        // resolve to the same slot (mirrors the CLI normalize()).
        let t = DARK;
        for spelling in ["In Progress", "InProgress", "in-progress", "in_progress"] {
            assert_eq!(
                t.status_color(spelling),
                Color::Yellow,
                "spelling {spelling:?} did not resolve to in-progress",
            );
        }
        assert_eq!(t.status_color("APPROVED"), Color::Cyan);
    }

    #[test]
    fn status_color_unknown_falls_back_to_fg() {
        // A project-specific custom status degrades to the neutral text color
        // rather than a wrong semantic color (CLI: `_ => text.normal()`).
        for theme in [CATPPUCCIN_MOCHA, DARK, LIGHT] {
            assert_eq!(theme.status_color("Frobnicate"), theme.fg);
            assert_eq!(theme.status_color(""), theme.fg);
        }
    }

    #[test]
    fn priority_color_maps_and_falls_back() {
        let t = DARK;
        assert_eq!(t.priority_color("High"), Color::Red);
        assert_eq!(t.priority_color("Medium"), Color::Yellow);
        assert_eq!(t.priority_color("med"), Color::Yellow);
        assert_eq!(t.priority_color("Low"), Color::DarkGray);
        // Unknown priority → neutral text color.
        assert_eq!(t.priority_color("urgent"), t.fg);
    }

    #[test]
    fn status_color_is_themeable_not_hardcoded() {
        // The whole point of STORY-691: switching the theme recolors the
        // status semantics. Approved is cyan on Dark but the Mocha "Sky" tint
        // on the default theme — different colors, same semantic slot.
        assert_ne!(
            CATPPUCCIN_MOCHA.status_color("Approved"),
            DARK.status_color("Approved"),
        );
        assert_ne!(
            CATPPUCCIN_MOCHA.priority_color("High"),
            LIGHT.priority_color("Low"),
        );
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
