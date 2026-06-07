//! `[tui]` configuration block in `.aida/config.toml`.
//!
//! Mirrors the env/config resolution pattern used by `aida-cli`'s
//! `workflow_hints` module — a hand-rolled section-aware scan rather than
//! a full serde dependency for two optional scalars.
//!
//! ```toml
//! [tui]
//! prefix_key = "Ctrl-a"   # command-mode toggle key (PTY-host mode only)
//! max_tabs   = 4          # soft cap on concurrently hosted sessions
//! mode       = "launcher" # "launcher" (default) or "pty-host" (legacy)
//! ```
//!
//! trace:STORY-132 STORY-244 | ai:claude

use crate::theme::ThemeName;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::{Path, PathBuf};

/// Default command-mode prefix key: `Ctrl-a` (tmux-style; see plan Fork 8).
pub fn default_prefix_key() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)
}

/// Which TUI to launch. STORY-244 made `Launcher` the default — the TUI is a
/// full-screen navigator that exits emitting an intent line for a bash
/// wrapper to dispatch. `PtyHost` is the legacy STORY-132 shell that hosts
/// Claude as a PTY child; opt in with `[tui] mode = "pty-host"` if you want
/// concurrent PTY panes and accept the render contention with Claude.
/// trace:STORY-244 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TuiMode {
    #[default]
    Launcher,
    PtyHost,
}

/// Resolved `[tui]` settings for one `aida tui` run.
#[derive(Debug, Clone)]
pub struct TuiConfig {
    pub prefix_key: KeyEvent,
    pub max_tabs: usize,
    pub mode: TuiMode,
    /// Active theme palette. `[tui] theme = "catppuccin-mocha"` selects it;
    /// defaults to [`ThemeName::default`] (Catppuccin Mocha). An unknown
    /// token keeps the default. trace:TASK-256 | ai:claude
    pub theme: ThemeName,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            prefix_key: default_prefix_key(),
            // Soft cap on concurrently hosted sessions (plan risk #6).
            max_tabs: crate::tab::MAX_TABS,
            mode: TuiMode::default(),
            theme: ThemeName::default(),
        }
    }
}

impl TuiConfig {
    /// Load `[tui]` settings, walking up from `cwd` for the nearest
    /// `.aida/config.toml`. Missing file / section / keys fall back to
    /// the defaults — a config error never blocks launching the TUI.
    pub fn load(cwd: &Path) -> Self {
        let mut cfg = TuiConfig::default();
        let Some(root) = find_project_root(cwd) else {
            return cfg;
        };
        let Ok(content) = std::fs::read_to_string(root.join(".aida").join("config.toml")) else {
            return cfg;
        };
        for (key, val) in scan_tui_section(&content) {
            match key.as_str() {
                "prefix_key" => {
                    if let Some(k) = parse_prefix_key(&val) {
                        cfg.prefix_key = k;
                    }
                }
                "max_tabs" => {
                    if let Ok(n) = val.parse::<usize>() {
                        cfg.max_tabs = n.max(1);
                    }
                }
                "mode" => {
                    if let Some(m) = parse_mode(&val) {
                        cfg.mode = m;
                    }
                }
                // trace:TASK-256 | ai:claude
                "theme" => {
                    if let Some(t) = ThemeName::from_config_str(&val) {
                        cfg.theme = t;
                    }
                }
                _ => {}
            }
        }
        cfg
    }
}

/// Walk up from `start` looking for a directory that holds
/// `.aida/config.toml`.
fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join(".aida").join("config.toml").is_file() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// Extract `key = value` pairs from the `[tui]` section of a TOML string.
/// Stops at the next `[section]` header; values are unquoted and
/// inline-comment-stripped.
fn scan_tui_section(content: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut in_tui = false;
    for raw in content.lines() {
        let line = strip_inline_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(stripped) = line.strip_prefix('[') {
            in_tui = stripped.trim_end_matches(']').trim() == "tui";
            continue;
        }
        if in_tui {
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim().trim_matches('"').trim_matches('\'').trim();
                pairs.push((k.trim().to_string(), v.to_string()));
            }
        }
    }
    pairs
}

/// Slice off a trailing `# comment`, honoring single/double quotes so a
/// `#` inside a quoted value is not mistaken for a comment.
fn strip_inline_comment(s: &str) -> &str {
    let (mut dq, mut sq) = (false, false);
    for (i, c) in s.char_indices() {
        match c {
            '"' if !sq => dq = !dq,
            '\'' if !dq => sq = !sq,
            '#' if !dq && !sq => return &s[..i],
            _ => {}
        }
    }
    s
}

/// Parse a `mode = "..."` string into a [`TuiMode`]. Case-insensitive;
/// accepts `launcher`, `pty-host`, and the hyphen-free `ptyhost` for
/// tolerance. Anything else returns `None` so the caller keeps the
/// default. trace:STORY-244 | ai:claude
pub fn parse_mode(spec: &str) -> Option<TuiMode> {
    match spec.trim().to_ascii_lowercase().as_str() {
        "launcher" => Some(TuiMode::Launcher),
        "pty-host" | "ptyhost" | "pty_host" => Some(TuiMode::PtyHost),
        _ => None,
    }
}

/// Parse a prefix-key string into a [`KeyEvent`]. Accepts `Ctrl-a`,
/// `ctrl+a`, `C-a` (case-insensitive); a bare single character maps to
/// that char with no modifier. Returns `None` for anything unrecognized
/// so the caller keeps the default.
pub fn parse_prefix_key(spec: &str) -> Option<KeyEvent> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    let lower = spec.to_ascii_lowercase();
    // Split on the first `-` or `+` separating a modifier from the key.
    let (modifier, key) = match lower.split_once(['-', '+']) {
        Some((m, k)) => (m.trim(), k.trim()),
        None => ("", lower.as_str()),
    };
    let mods = match modifier {
        "" => KeyModifiers::NONE,
        "ctrl" | "c" | "control" => KeyModifiers::CONTROL,
        "alt" | "a" | "meta" | "m" => KeyModifiers::ALT,
        _ => return None,
    };
    let mut chars = key.chars();
    let first = chars.next()?;
    if chars.next().is_some() {
        // Multi-char key name — only single characters are supported.
        return None;
    }
    Some(KeyEvent::new(KeyCode::Char(first), mods))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_prefix_key_accepts_common_spellings() {
        let ctrl_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(parse_prefix_key("Ctrl-a"), Some(ctrl_a));
        assert_eq!(parse_prefix_key("ctrl+a"), Some(ctrl_a));
        assert_eq!(parse_prefix_key("C-a"), Some(ctrl_a));
        assert_eq!(
            parse_prefix_key("alt-b"),
            Some(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT))
        );
    }

    #[test]
    fn parse_prefix_key_rejects_garbage() {
        assert_eq!(parse_prefix_key(""), None);
        assert_eq!(parse_prefix_key("super-x"), None);
        assert_eq!(parse_prefix_key("ctrl-esc"), None);
    }

    #[test]
    fn scan_tui_section_reads_keys_and_ignores_other_sections() {
        let toml = "\
[hints]
workflow_hints = false

[tui]
prefix_key = \"Ctrl-b\"  # custom
max_tabs = 6

[behavior]
permission_mode = \"auto\"
";
        let pairs = scan_tui_section(toml);
        assert!(pairs.contains(&("prefix_key".into(), "Ctrl-b".into())));
        assert!(pairs.contains(&("max_tabs".into(), "6".into())));
        assert!(!pairs.iter().any(|(k, _)| k == "permission_mode"));
    }

    #[test]
    fn mode_default_is_launcher() {
        // STORY-244 pivot: launcher is the new default.
        assert_eq!(TuiConfig::default().mode, TuiMode::Launcher);
    }

    #[test]
    fn mode_parses_pty_host() {
        assert_eq!(parse_mode("launcher"), Some(TuiMode::Launcher));
        assert_eq!(parse_mode("LAUNCHER"), Some(TuiMode::Launcher));
        assert_eq!(parse_mode("pty-host"), Some(TuiMode::PtyHost));
        assert_eq!(parse_mode("PTY-HOST"), Some(TuiMode::PtyHost));
        assert_eq!(parse_mode("ptyhost"), Some(TuiMode::PtyHost));
        // Unknown spelling falls back to caller's default.
        assert_eq!(parse_mode("bogus"), None);
        assert_eq!(parse_mode(""), None);
    }

    #[test]
    fn config_load_picks_up_mode_field() {
        let toml = "\
[tui]
mode = \"pty-host\"
";
        let pairs = scan_tui_section(toml);
        assert!(pairs.contains(&("mode".into(), "pty-host".into())));
    }

    #[test]
    fn theme_defaults_to_catppuccin_mocha() {
        // TASK-256: the default palette is Catppuccin Mocha.
        assert_eq!(TuiConfig::default().theme, ThemeName::CatppuccinMocha);
    }

    #[test]
    fn config_scan_picks_up_theme_field() {
        let toml = "\
[tui]
theme = \"dark\"
";
        let pairs = scan_tui_section(toml);
        assert!(pairs.contains(&("theme".into(), "dark".into())));
        // And the token resolves to the right palette name.
        assert_eq!(ThemeName::from_config_str("dark"), Some(ThemeName::Dark));
    }
}
