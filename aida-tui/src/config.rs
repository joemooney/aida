//! `[tui]` configuration block in `.aida/config.toml`.
//!
//! Mirrors the env/config resolution pattern used by `aida-cli`'s
//! `workflow_hints` module — a hand-rolled section-aware scan rather than
//! a full serde dependency for two optional scalars.
//!
//! ```toml
//! [tui]
//! prefix_key     = "Ctrl-a"   # command-mode toggle key (PTY-host mode only)
//! max_tabs       = 4          # soft cap on concurrently hosted sessions
//! mode           = "launcher" # "launcher" (default) or "pty-host" (legacy)
//! ctrl_d_palette = false      # opt-in: Ctrl-D from chat opens the palette
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

/// Which vendor CLI a hosted tab launches behind `aida queue work`. The TUI's
/// PtyHost is argv-agnostic, but `spawn_tab` historically hosted a Claude-only
/// `aida queue work` (it threaded Claude's `--session-id`/`--resume`). This
/// enum lets a tab host a Codex session instead — the interactive analogue of
/// the STORY-683 headless `HeadlessVendor`.
///
/// `Claude` is the default everywhere, so an un-configured TUI is byte-identical
/// to before; `Codex` is the explicit opt-in (`[tui] vendor = "codex"` or
/// `AIDA_TUI_VENDOR=codex`). Codex's interactive CLI has no caller-minted
/// `--session-id`, so a Codex tab hosts a fresh session and does not thread
/// `--session-id`/`--resume` (resume-parity is a follow-up).
// trace:TASK-895 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabVendor {
    #[default]
    Claude,
    Codex,
}

impl TabVendor {
    /// The canonical lowercase token (`claude` / `codex`).
    // trace:TASK-895 | ai:claude
    pub fn as_str(self) -> &'static str {
        match self {
            TabVendor::Claude => "claude",
            TabVendor::Codex => "codex",
        }
    }

    /// Parse a vendor token. Case-insensitive, surrounding whitespace tolerated.
    /// `None` for an unrecognized token so the caller keeps the Claude default
    /// rather than route to an unknown CLI.
    // trace:TASK-895 | ai:claude
    pub fn parse(raw: &str) -> Option<TabVendor> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "claude" => Some(TabVendor::Claude),
            "codex" => Some(TabVendor::Codex),
            _ => None,
        }
    }
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
    /// Which vendor CLI a hosted tab launches. `[tui] vendor = "codex"` (or
    /// `AIDA_TUI_VENDOR=codex`) opts a tab into hosting Codex instead of
    /// Claude; defaults to [`TabVendor::Claude`].
    // trace:TASK-895 | ai:claude
    pub vendor: TabVendor,
    /// Opt-in: treat a raw `Ctrl-D` from the hosted chat as an alternate
    /// trigger to open the deterministic action palette (suspend the chat →
    /// palette), the same surface `prefix p` opens — the immediate-response
    /// vision's literal entry point ("Ctrl-D from chat → palette"). Default
    /// `false` because `Ctrl-D` is the terminal EOF byte (0x04): with this off
    /// it passes straight through to the child unchanged, so nobody relying on
    /// Ctrl-D in their chat/REPL is surprised. `[tui] ctrl_d_palette = true`
    /// opts in.
    // trace:TASK-909 | ai:claude
    pub ctrl_d_palette: bool,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            prefix_key: default_prefix_key(),
            // Soft cap on concurrently hosted sessions (plan risk #6).
            max_tabs: crate::tab::MAX_TABS,
            mode: TuiMode::default(),
            theme: ThemeName::default(),
            vendor: TabVendor::default(),
            // Off by default: `Ctrl-D` keeps passing through to the child as
            // the EOF byte until the user opts in. trace:TASK-909 | ai:claude
            ctrl_d_palette: false,
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
        // STORY-761: seed from the uniform `[agents] vendor` knob (agents.toml,
        // project over global) BEFORE scanning `[tui]`, so the per-surface
        // `[tui] vendor` and `AIDA_TUI_VENDOR` both still override it.
        // trace:STORY-761 | ai:claude
        if let Some(v) = aida_core::agents_config::resolve_default_vendor(&root)
            .and_then(|s| TabVendor::parse(&s))
        {
            cfg.vendor = v;
        }
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
                // trace:TASK-895 | ai:claude
                "vendor" => {
                    if let Some(v) = TabVendor::parse(&val) {
                        cfg.vendor = v;
                    }
                }
                // trace:TASK-909 | ai:claude
                "ctrl_d_palette" => {
                    if let Some(b) = parse_bool(&val) {
                        cfg.ctrl_d_palette = b;
                    }
                }
                _ => {}
            }
        }
        // TASK-895: `AIDA_TUI_VENDOR` overrides the config block, mirroring the
        // STORY-683 `AIDA_HEADLESS_VENDOR` precedence convention (env beats
        // config). An unrecognized token is ignored, keeping the prior value.
        if let Ok(raw) = std::env::var("AIDA_TUI_VENDOR") {
            if let Some(v) = TabVendor::parse(&raw) {
                cfg.vendor = v;
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

/// Parse a boolean config value. Case-insensitive; accepts the common
/// truthy/falsy spellings (`true`/`false`, `yes`/`no`, `on`/`off`, `1`/`0`).
/// Anything else returns `None` so the caller keeps the default.
// trace:TASK-909 | ai:claude
pub fn parse_bool(spec: &str) -> Option<bool> {
    match spec.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
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
    fn parse_bool_accepts_common_spellings() {
        // TASK-909: the `ctrl_d_palette` opt-in parses the usual truthy/falsy
        // forms case-insensitively; garbage keeps the caller's default.
        for t in ["true", "TRUE", "yes", "on", "1"] {
            assert_eq!(parse_bool(t), Some(true), "{t:?} should be true");
        }
        for f in ["false", "No", "off", "0"] {
            assert_eq!(parse_bool(f), Some(false), "{f:?} should be false");
        }
        assert_eq!(parse_bool("maybe"), None);
        assert_eq!(parse_bool(""), None);
    }

    #[test]
    fn ctrl_d_palette_defaults_off() {
        // The EOF-guarding default: Ctrl-D passes through unless opted in.
        assert!(!TuiConfig::default().ctrl_d_palette);
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

    // TASK-895: vendor defaults to Claude; parse is case/whitespace-tolerant.
    #[test]
    fn vendor_default_is_claude() {
        assert_eq!(TuiConfig::default().vendor, TabVendor::Claude);
    }

    #[test]
    fn vendor_parses_known_tokens() {
        assert_eq!(TabVendor::parse("claude"), Some(TabVendor::Claude));
        assert_eq!(TabVendor::parse("CODEX"), Some(TabVendor::Codex));
        assert_eq!(TabVendor::parse("  codex  "), Some(TabVendor::Codex));
        // Unknown token keeps the caller's default.
        assert_eq!(TabVendor::parse("gemini"), None);
        assert_eq!(TabVendor::parse(""), None);
    }

    /// STORY-761: the uniform `[agents] vendor` knob (project agents.toml)
    /// seeds the tab vendor, and the per-surface `[tui] vendor` still
    /// overrides it.
    // trace:STORY-761 | ai:claude
    #[test]
    fn vendor_seeded_from_agents_knob_and_tui_section_overrides() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        std::fs::write(tmp.path().join(".aida/config.toml"), "[tui]\n").unwrap();
        std::fs::write(
            tmp.path().join(".aida/agents.toml"),
            "[agents]\nvendor = \"codex\"\n",
        )
        .unwrap();
        assert_eq!(TuiConfig::load(tmp.path()).vendor, TabVendor::Codex);

        // Per-surface `[tui] vendor` beats the knob.
        std::fs::write(
            tmp.path().join(".aida/config.toml"),
            "[tui]\nvendor = \"claude\"\n",
        )
        .unwrap();
        assert_eq!(TuiConfig::load(tmp.path()).vendor, TabVendor::Claude);
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
