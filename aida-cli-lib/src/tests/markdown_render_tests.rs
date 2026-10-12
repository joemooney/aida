//! Snapshot tests for the terminal markdown renderer.
//!
//! Each snapshot lives under `src/tests/markdown_render_snapshots/`. Styled
//! snapshots show ANSI escapes as `␛` so they stay readable in review.
//! Regenerate with `AIDA_UPDATE_SNAPSHOTS=1 cargo test -p aida-cli-lib
//! markdown_render`, then review the diff.
// trace:STORY-1652 | ai:claude

use super::*;

fn snapshot_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tests/markdown_render_snapshots")
}

fn assert_snapshot(name: &str, actual: &str) {
    let path = snapshot_dir().join(format!("{name}.txt"));
    let actual = format!("{}\n", actual.replace('\x1b', "␛"));
    if std::env::var_os("AIDA_UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(snapshot_dir()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing snapshot {}; run with AIDA_UPDATE_SNAPSHOTS=1\n--- actual ---\n{actual}",
            path.display()
        )
    });
    assert_eq!(
        expected, actual,
        "snapshot {name} differs\n--- actual ---\n{actual}"
    );
}

fn plain(src: &str, width: usize) -> String {
    render_markdown(src, width, false).expect("renders")
}

fn styled(src: &str, width: usize) -> String {
    render_markdown(src, width, true).expect("renders")
}

const HEADINGS: &str = "# Top heading\n\nIntro paragraph.\n\n## Second level\n\nBody text.\n\n### Third level\n\nMore.\n\n#### Fourth level\n";

const EMPHASIS: &str = "Plain, **bold**, *italic*, ***both***, `inline code`, ~~struck~~ and a [link](https://example.com/docs).\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n";

const NESTED_LISTS: &str = "- first item that is long enough to wrap onto a second line at forty columns\n- second\n  - nested one\n  - nested two with **bold**\n    1. deep ordered\n    2. deep ordered again\n- third\n\n1. one\n2. two\n10. ten\n\n- [x] done task\n- [ ] open task\n";

const WIDE_TABLE: &str = "| | `aida queue work --auto-complete` | `aida drain start` |\n|---|---|---|\n| No selector | The head of your role's queue: one spec | The whole queue (`--drain`): every spec that can run unattended, skipping blocked, human-only or in-flight ones |\n| Sessions | Interactive by default: you can talk to each phase's session | Headless by default (`--no-human=both`) |\n| Stopping | Ctrl-C, or the budgets (`--max`, `--max-failures`) | Those budgets, plus `aida drain stop` |\n";

#[test]
fn snapshot_headings() {
    assert_snapshot("headings_plain", &plain(HEADINGS, 80));
    assert_snapshot("headings_styled", &styled(HEADINGS, 80));
}

#[test]
fn snapshot_bold_italic_code() {
    assert_snapshot("emphasis_plain", &plain(EMPHASIS, 80));
    assert_snapshot("emphasis_styled", &styled(EMPHASIS, 80));
}

#[test]
fn snapshot_nested_lists() {
    assert_snapshot("nested_lists_40", &plain(NESTED_LISTS, 40));
}

#[test]
fn snapshot_wide_table_at_80_and_120() {
    let at80 = plain(WIDE_TABLE, 80);
    let at120 = plain(WIDE_TABLE, 120);
    assert_snapshot("wide_table_80", &at80);
    assert_snapshot("wide_table_120", &at120);
    for (width, out) in [(80, &at80), (120, &at120)] {
        for line in out.lines() {
            assert!(
                line.width() <= width,
                "table line wider than {width}: {line:?} ({})",
                line.width()
            );
        }
        // Every row of the box is the same width: the columns are aligned.
        let widths: Vec<usize> = out.lines().map(|l| l.width()).collect();
        assert!(widths.windows(2).all(|w| w[0] == w[1]), "{widths:?}");
    }
}

#[test]
fn malformed_markdown_falls_back_to_raw() {
    let src = "Intro with **bold**.\n\n```\nunclosed fence\nstill inside\n";
    assert_eq!(
        render_markdown(src, 80, false),
        Err(MarkdownRenderError::UnclosedCodeFence)
    );
    // The never-fail entry point hands back the source byte-for-byte.
    assert_eq!(render_markdown_for_terminal(src, 80), src);
    assert_snapshot("malformed_fallback", &render_markdown_for_terminal(src, 80));
}

#[test]
fn closed_fences_are_not_malformed() {
    for src in [
        "```\ncode\n```\n",
        "~~~~\ncode\n~~~~",
        "```\n```",
        "  ```\n  x\n  ```\n",
        "> ```\n> code\n> ```\n\nafter",
        "> > ~~~\n> > code\n> > ~~~\n",
    ] {
        assert!(render_markdown(src, 80, false).is_ok(), "{src:?}");
    }
    assert!(fence_is_unclosed("```\n", 0));
    assert!(fence_is_unclosed("````\ncode\n```\n", 0));
    // A quoted-looking fence line inside an unquoted fence is content.
    assert!(fence_is_unclosed("```\ncode\n> ```", 0));
    assert_eq!(
        render_markdown("```\ncode\n> ```", 80, false),
        Err(MarkdownRenderError::UnclosedCodeFence)
    );
}

#[test]
fn plain_output_carries_no_escape_codes() {
    for src in [HEADINGS, EMPHASIS, NESTED_LISTS, WIDE_TABLE] {
        assert!(!plain(src, 80).contains('\x1b'));
    }
    assert!(styled(EMPHASIS, 80).contains("\x1b[1m"));
}

#[test]
fn plain_text_survives_unchanged_when_it_fits() {
    assert_eq!(plain("just words", 80), "just words");
    assert_eq!(plain("", 80), "");
}

#[test]
fn wraps_to_width_with_hard_split_of_long_words() {
    let out = plain(&"x".repeat(50), 20);
    assert_eq!(out.lines().count(), 3);
    assert!(out.lines().all(|l| l.width() <= 20));
}

#[test]
fn render_decision_only_at_a_tty_and_not_raw() {
    assert!(markdown_render_decision(false, None, true));
    assert!(markdown_render_decision(false, Some("rendered"), true));
    assert!(
        !markdown_render_decision(false, None, false),
        "piped stays raw"
    );
    assert!(!markdown_render_decision(true, None, true), "--raw");
    assert!(!markdown_render_decision(false, Some("raw"), true));
    assert!(!markdown_render_decision(false, Some(" RAW "), true));
}

#[test]
fn unstyled_inline_code_keeps_its_backticks() {
    assert_eq!(
        plain("run `aida drain stop` now", 80),
        "run `aida drain stop` now"
    );
    // Styled output marks code with colour instead of backticks.
    assert!(!styled("run `x` now", 80).contains('`'));
}

#[test]
fn multi_line_html_block_is_one_gutter_block() {
    let out = plain(
        "<details>\n<summary>More</summary>\nbody\n</details>\n\nafter",
        80,
    );
    assert_eq!(
        out,
        "  │ <details>\n  │ <summary>More</summary>\n  │ body\n  │ </details>\n\nafter"
    );
    // Inside a blockquote the `> ` prefixes sit between the events; the
    // block still merges, and a quoted blank line still separates blocks.
    let out = plain(
        "> <div>\n> <p>x</p>\n> </div>\n>\n> <div>\n> y\n> </div>\n",
        80,
    );
    assert_eq!(
        out,
        "│   │ <div>\n│   │ <p>x</p>\n│   │ </div>\n│\n│   │ <div>\n│   │ y\n│   │ </div>"
    );
}

#[test]
fn nested_emphasis_keeps_the_outer_style_until_it_closes() {
    // `*a _b_ c*`: italic must still be on for "c" after the inner span closes.
    let out = styled("*a _b_ c*", 80);
    assert!(out.ends_with("\x1b[3mc\x1b[0m"), "{out:?}");
    let out = styled("**a __b__ c**", 80);
    assert!(out.ends_with("\x1b[1mc\x1b[0m"), "{out:?}");
}

#[test]
fn deeply_nested_lists_keep_a_readable_body_width() {
    let mut src = String::new();
    for depth in 0..12 {
        src.push_str(&"  ".repeat(depth));
        src.push_str("- several words in this item\n");
    }
    let out = plain(&src, 20);
    // Every item, however deep, wraps to at most three lines (a 12-column
    // body) instead of being split a character at a time.
    assert!(out.lines().count() <= 12 * 3, "{out}");
}

// ---------------------------------------------------------------------------
// Glyph profile (BUG-1979): AIDA_GLYPHS=ascii draws ASCII-only line art.
// trace:BUG-1979 | ai:claude
// ---------------------------------------------------------------------------

const QUOTE_AND_RULE: &str = "> quoted line\n> > nested quote\n\n---\n\n```\ncode\n```\n";

fn ascii(src: &str, width: usize) -> String {
    render_markdown_with(src, width, false, GlyphProfile::Ascii).expect("renders")
}

fn assert_all_ascii(out: &str) {
    if let Some(c) = out.chars().find(|c| !c.is_ascii()) {
        panic!("non-ASCII glyph {c:?} in ascii-profile output:\n{out}");
    }
}

#[test]
fn ascii_profile_headings_use_ascii_rules() {
    let out = ascii(HEADINGS, 80);
    assert_all_ascii(&out);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "Top heading");
    assert_eq!(lines[1], "===========");
    let second = lines.iter().position(|l| *l == "Second level").unwrap();
    assert_eq!(lines[second + 1], "------------");
}

#[test]
fn ascii_profile_bullets_use_the_registry_bullet() {
    let out = ascii(NESTED_LISTS, 40);
    assert_all_ascii(&out);
    assert!(out.starts_with("* first item"), "{out}");
    assert!(out.contains("  * nested one"), "{out}");
    assert_snapshot("nested_lists_40_ascii", &out);
}

#[test]
fn ascii_profile_blockquote_code_and_rule_use_pipes_and_dashes() {
    let out = ascii(QUOTE_AND_RULE, 80);
    assert_all_ascii(&out);
    assert_eq!(
        out,
        "| quoted line\n|\n| | nested quote\n\n----------------------------------------\n\n  | code"
    );
}

#[test]
fn ascii_profile_table_uses_plus_dash_pipe_borders() {
    let out = ascii(WIDE_TABLE, 80);
    assert_all_ascii(&out);
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines[0].starts_with("+-") && lines[0].ends_with("-+"),
        "{out}"
    );
    assert!(
        lines[1].starts_with("| ") && lines[1].ends_with(" |"),
        "{out}"
    );
    assert!(lines.last().unwrap().starts_with("+-"), "{out}");
    // Same geometry as the Unicode table: only the glyphs differ.
    let unicode = plain(WIDE_TABLE, 80);
    let widths = |s: &str| s.lines().map(|l| l.width()).collect::<Vec<_>>();
    assert_eq!(widths(&out), widths(&unicode));
    assert_snapshot("wide_table_80_ascii", &out);
}

#[test]
fn ascii_profile_styled_output_is_ascii_too() {
    for src in [HEADINGS, EMPHASIS, NESTED_LISTS, WIDE_TABLE, QUOTE_AND_RULE] {
        let out = render_markdown_with(src, 80, true, GlyphProfile::Ascii).unwrap();
        assert_all_ascii(&out);
    }
}

#[test]
fn unicode_profile_is_the_default_and_unchanged() {
    for src in [HEADINGS, EMPHASIS, NESTED_LISTS, WIDE_TABLE, QUOTE_AND_RULE] {
        assert_eq!(
            render_markdown_with(src, 80, false, GlyphProfile::Unicode).unwrap(),
            plain(src, 80)
        );
    }
    // The historical box-drawing literals, byte for byte.
    assert_eq!(
        plain(QUOTE_AND_RULE, 80),
        "│ quoted line\n│\n│ │ nested quote\n\n────────────────────────────────────────\n\n  │ code"
    );
    assert!(plain(HEADINGS, 80).contains("═══"));
    assert!(plain(NESTED_LISTS, 80).starts_with("• first"));
    let table = plain(WIDE_TABLE, 80);
    for corner in ["┌", "┬", "┐", "├", "┼", "┤", "└", "┴", "┘"] {
        assert!(table.contains(corner), "missing {corner}");
    }
}

#[test]
fn aida_glyphs_env_drives_the_terminal_renderer() {
    let home = tempfile::tempdir().unwrap();
    let home_str = home.path().to_str().unwrap();
    {
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_GLYPHS", Some("ascii")),
            ("AIDA_TEST_HOME", Some(home_str)),
        ]);
        let out = render_markdown_for_terminal(WIDE_TABLE, 80);
        assert_all_ascii(&out);
        assert!(out.starts_with("+-"), "{out}");
    }
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_GLYPHS", Some("unicode")),
        ("AIDA_TEST_HOME", Some(home_str)),
    ]);
    assert!(render_markdown_for_terminal(WIDE_TABLE, 80).starts_with("┌"));
}

#[test]
fn empty_no_color_is_ignored_per_no_color_org() {
    // Unset or empty NO_COLOR leaves colour on at a terminal.
    assert!(styling_decision(None, None, None, true));
    assert!(styling_decision(Some(""), None, None, true));
    // A non-empty NO_COLOR turns it off, whatever its value.
    assert!(!styling_decision(Some("1"), None, None, true));
    assert!(!styling_decision(Some("0"), None, None, true));
    // CLICOLOR_FORCE wins over NO_COLOR; CLICOLOR=0 and a pipe turn it off.
    assert!(styling_decision(Some("1"), Some("1"), None, false));
    assert!(!styling_decision(None, Some("0"), None, false));
    assert!(!styling_decision(None, None, Some("0"), true));
    assert!(!styling_decision(None, None, None, false));
}

#[test]
fn quiet_render_hook_only_silences_render_panics() {
    install_quiet_render_hook();
    // Outside a render, the flag is clear, so other panics still reach the
    // previous hook.
    assert!(!RENDERING.with(|r| r.get()));
    let _ = render_markdown_for_terminal("text", 80);
    assert!(!RENDERING.with(|r| r.get()), "flag reset after render");
}
