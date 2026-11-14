// trace:TASK-3783 | ai:codex
pub use aida_term::glyphs::*;
#[cfg(test)]
use std::path::Path;

#[cfg(test)]
mod tests {
    use super::*;
    // BUG-697 / TASK-1532: route env manipulation through EnvVarsGuard.
    // trace:TASK-1532 | ai:agy
    fn with_env(glyphs: Option<&str>, home: Option<&Path>) -> crate::test_env::EnvVarsGuard {
        let home_str = home.and_then(|p| p.to_str());
        crate::test_env::EnvVarsGuard::apply(&[("AIDA_GLYPHS", glyphs), ("HOME", home_str)])
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
        assert_eq!(Glyph::FlowQueuedSupervised.unicode(), "⇈");
        // ASCII fallbacks are single-display-column-friendly.
        assert_eq!(Glyph::Done.ascii(), "[*]");
        assert_eq!(Glyph::FlowActive.ascii(), ">");
        assert_eq!(Glyph::FlowBlocked.ascii(), "x");
        assert_eq!(Glyph::FlowQueued.ascii(), "^");
        assert_eq!(Glyph::FlowQueuedSupervised.ascii(), "^^");
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
        let mut _env = crate::test_env::EnvVarGuard::set("AIDA_GLYPHS", "ascii");
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
        _env.reset("unicode");
        assert_eq!(get(Glyph::Info, None), "ⓘ");
        assert_eq!(get(Glyph::IncomingMail, None), "📨");
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
        // trace:TASK-3783 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }

        let home = tempfile::tempdir().unwrap();
        let _env = with_env(None, Some(home.path()));
        let proj = tempfile::tempdir().unwrap();
        // No config files anywhere.
        assert_eq!(active_profile(Some(proj.path())), GlyphProfile::Unicode);
    }

    #[test]
    fn env_wins_over_project_and_user() {
        // trace:TASK-3783 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }

        let home = tempfile::tempdir().unwrap();
        write_config(home.path(), "unicode");
        let proj = tempfile::tempdir().unwrap();
        write_config(proj.path(), "unicode");
        let _env = with_env(Some("ascii"), Some(home.path()));
        assert_eq!(active_profile(Some(proj.path())), GlyphProfile::Ascii);
    }

    #[test]
    fn project_wins_over_user() {
        // trace:TASK-3783 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }

        let home = tempfile::tempdir().unwrap();
        write_config(home.path(), "unicode");
        let proj = tempfile::tempdir().unwrap();
        write_config(proj.path(), "ascii");
        let _env = with_env(None, Some(home.path()));
        assert_eq!(active_profile(Some(proj.path())), GlyphProfile::Ascii);
    }

    #[test]
    fn user_config_used_when_no_project_setting() {
        // trace:TASK-3783 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }

        let home = tempfile::tempdir().unwrap();
        write_config(home.path(), "ascii");
        let _env = with_env(None, Some(home.path()));
        // Project dir with no config → falls through to user tier.
        let proj = tempfile::tempdir().unwrap();
        assert_eq!(active_profile(Some(proj.path())), GlyphProfile::Ascii);
    }

    #[test]
    fn get_honors_active_profile() {
        let mut _env = crate::test_env::EnvVarGuard::set("AIDA_GLYPHS", "ascii");
        assert_eq!(get(Glyph::Check, None), "[x]");
        _env.reset("unicode");
        assert_eq!(get(Glyph::Check, None), "✓");
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
        // trace:TASK-3783 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }

        let home = tempfile::tempdir().unwrap();
        let _env = with_env(None, Some(home.path()));
        // Project on the ASCII profile, but custom-override `check`.
        let proj = tempfile::tempdir().unwrap();
        write_config_with_glyphs(proj.path(), "ascii", "check = \"OK\"");

        let overrides = GlyphOverrides::resolve(Some(proj.path()));
        let profile = active_profile(Some(proj.path()));
        assert_eq!(profile, GlyphProfile::Ascii);
        // Override beats the ascii profile rendering ("[x]").
        assert_eq!(overrides.render(Glyph::Check, profile), "OK");
        assert_eq!(get_custom(Glyph::Check, Some(proj.path())), "OK");
    }

    #[test]
    fn unset_symbol_falls_through_to_profile() {
        // trace:TASK-3783 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }

        let home = tempfile::tempdir().unwrap();
        let _env = with_env(None, Some(home.path()));
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
    }

    #[test]
    fn unset_symbol_falls_through_to_unicode_default() {
        // trace:TASK-3783 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }

        let home = tempfile::tempdir().unwrap();
        let _env = with_env(None, Some(home.path()));
        // Default (unicode) profile, override only `check`.
        let proj = tempfile::tempdir().unwrap();
        write_config_with_glyphs(proj.path(), "unicode", "check = \"OK\"");

        let overrides = GlyphOverrides::resolve(Some(proj.path()));
        let profile = active_profile(Some(proj.path()));
        assert_eq!(profile, GlyphProfile::Unicode);
        assert_eq!(overrides.render(Glyph::Check, profile), "OK");
        // Unset `warning` falls through to the unicode default.
        assert_eq!(overrides.render(Glyph::Warning, profile), "⚠");
    }

    #[test]
    fn project_override_beats_user_override() {
        // trace:TASK-3783 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }

        // User sets check + warning; project overrides ONLY check.
        let home = tempfile::tempdir().unwrap();
        write_config_with_glyphs(home.path(), "unicode", "check = \"USER\"\nwarning = \"UW\"");
        let _env = with_env(None, Some(home.path()));
        let proj = tempfile::tempdir().unwrap();
        write_config_with_glyphs(proj.path(), "unicode", "check = \"PROJ\"");

        let overrides = GlyphOverrides::resolve(Some(proj.path()));
        // Project wins for `check`.
        assert_eq!(overrides.get(Glyph::Check), Some("PROJ"));
        // User entry survives for a symbol the project didn't override.
        assert_eq!(overrides.get(Glyph::Warning), Some("UW"));
    }

    #[test]
    fn absent_glyphs_section_is_phase1_behavior() {
        // trace:TASK-3783 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }

        let home = tempfile::tempdir().unwrap();
        let _env = with_env(None, Some(home.path()));
        // Only a [ui] section, no [glyphs] — overrides empty, falls to profile.
        let proj = tempfile::tempdir().unwrap();
        write_config(proj.path(), "ascii");

        let overrides = GlyphOverrides::resolve(Some(proj.path()));
        assert!(overrides.is_empty());
        let profile = active_profile(Some(proj.path()));
        assert_eq!(overrides.render(Glyph::Check, profile), "[x]");
        assert_eq!(get_custom(Glyph::Check, Some(proj.path())), "[x]");
    }

    #[test]
    fn unknown_override_key_is_ignored() {
        // trace:TASK-3783 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }

        let home = tempfile::tempdir().unwrap();
        let _env = with_env(None, Some(home.path()));
        let proj = tempfile::tempdir().unwrap();
        write_config_with_glyphs(proj.path(), "unicode", "check = \"OK\"\nbogus = \"X\"");

        let overrides = GlyphOverrides::resolve(Some(proj.path()));
        assert_eq!(overrides.get(Glyph::Check), Some("OK"));
        // Only the valid key landed.
        assert_eq!(overrides.len(), 1);
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
        // trace:TASK-3783 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }

        let home = tempfile::tempdir().unwrap();
        let _env = with_env(None, Some(home.path()));
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
    }

    #[test]
    fn no_theme_falls_through_to_profile_then_default() {
        // trace:TASK-3783 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }

        let home = tempfile::tempdir().unwrap();
        let _env = with_env(None, Some(home.path()));
        let proj = tempfile::tempdir().unwrap();
        // Only a profile, no theme, no overrides.
        write_config_raw(proj.path(), "glyphs = \"ascii\"", None);
        assert!(active_theme(Some(proj.path())).is_none());
        assert_eq!(resolve_with_theme(Glyph::Check, Some(proj.path())), "[x]");

        // Empty project → unicode default.
        let bare = tempfile::tempdir().unwrap();
        assert_eq!(resolve_with_theme(Glyph::Check, Some(bare.path())), "✓");
    }

    #[test]
    fn project_theme_beats_user_theme() {
        // trace:TASK-3783 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }

        let home = tempfile::tempdir().unwrap();
        let _env = with_env(None, Some(home.path()));
        let proj = tempfile::tempdir().unwrap();
        write_config_raw(home.path(), "theme = \"ascii\"", None);
        write_config_raw(proj.path(), "theme = \"nerd-font\"", None);

        assert_eq!(
            active_theme(Some(proj.path())).map(|t| t.name),
            Some("nerd-font")
        );
    }
}
