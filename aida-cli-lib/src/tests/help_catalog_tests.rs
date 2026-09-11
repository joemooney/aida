// Tests for the clap-derived `aida commands` / `aida help commands` catalog.
// trace:STORY-1027 | ai:codex

use super::*;

#[test]
fn catalog_is_derived_and_comprehensive() {
    let rows = catalog_rows(false);
    // The surface is large; a hand-maintained list would rot. The derived
    // walk should see well north of 100 commands+subcommands.
    assert!(
        rows.len() > 100,
        "expected a large derived catalog, got {} rows",
        rows.len()
    );

    let paths: Vec<String> = rows.iter().map(|r| r.path.join(" ")).collect();
    // Known top-level commands.
    for expected in ["aida list", "aida show", "aida add", "aida status"] {
        assert!(
            paths.iter().any(|path| path == expected),
            "missing top-level row {expected}"
        );
    }
    // Known nested subcommands — the whole point of the catalog.
    for expected in [
        "aida findings list",
        "aida questions answer",
        "aida advisor schedule list",
    ] {
        assert!(
            paths.iter().any(|path| path == expected),
            "missing nested row {expected}"
        );
    }
}

#[test]
fn catalog_rows_are_full_runnable_paths_depth_first() {
    let rows = catalog_rows(false);
    // Every row starts with the binary name so each line is copy-runnable.
    assert!(rows
        .iter()
        .all(|r| r.path.first().map(String::as_str) == Some("aida")));
    // Depth-first means a parent appears before its descendants.
    let paths: Vec<String> = rows.iter().map(|r| r.path.join(" ")).collect();
    let findings_idx = paths
        .iter()
        .position(|path| path == "aida findings")
        .expect("findings parent");
    let findings_list_idx = paths
        .iter()
        .position(|path| path == "aida findings list")
        .expect("findings list child");
    assert!(
        findings_idx < findings_list_idx,
        "parent must precede child"
    );
    // Paths are unique (clap enforces per-level uniqueness; the walk must
    // not duplicate).
    let deduped: std::collections::HashSet<&String> = paths.iter().collect();
    assert_eq!(paths.len(), deduped.len(), "duplicate catalog rows");
}

#[test]
fn catalog_skips_claps_auto_help_subcommands() {
    let rows = catalog_rows(true);
    assert!(
        rows.iter().all(|r| !r.path.iter().any(|seg| seg == "help")),
        "clap's auto-generated `help` navigation rows must be skipped"
    );
}

#[test]
fn catalog_descriptions_carry_no_spec_ids() {
    // SPEC-IDs stay in developer artifacts, never user-facing output.
    let rows = catalog_rows(false);
    for row in &rows {
        for prefix in ["TASK-", "BUG-", "STORY-", "EPIC-", "SPIKE-"] {
            if let Some(idx) = row.about.find(prefix) {
                let after = &row.about[idx + prefix.len()..];
                assert!(
                    !after.starts_with(|c: char| c.is_ascii_digit()),
                    "SPEC-ID leaked into catalog row `{}`: {}",
                    row.path.join(" "),
                    row.about
                );
            }
        }
    }
}

#[test]
fn visible_catalog_requires_about_for_every_command() {
    for row in catalog_rows(false) {
        assert!(
            !row.about.trim().is_empty(),
            "missing one-line about for `{}`",
            row.path.join(" ")
        );
    }
}

#[test]
fn catalog_contains_every_visible_top_level_help_command() {
    let mut cmd = crate::cli::Cli::command();
    cmd.build();
    let rows: std::collections::HashSet<String> = catalog_rows(false)
        .into_iter()
        .map(|r| r.path.join(" "))
        .collect();
    for sub in cmd
        .get_subcommands()
        .filter(|sub| sub.get_name() != "help" && !sub.is_hide_set())
    {
        let path = format!("aida {}", sub.get_name());
        assert!(rows.contains(&path), "missing top-level command `{path}`");
    }
}

#[test]
fn catalog_flags_are_long_option_names_only() {
    let rows = catalog_rows(false);
    let list = rows
        .iter()
        .find(|row| row.path.join(" ") == "aida list")
        .expect("aida list row");
    assert!(
        list.flags.iter().any(|flag| flag.name == "status"),
        "expected aida list --status flag in catalog"
    );
    assert!(
        list.flags
            .iter()
            .all(|flag| !flag.name.starts_with("--") && !flag.name.contains('<')),
        "flags should be bare long names without value placeholders"
    );
}

#[test]
fn catalog_hidden_filter_is_opt_in() {
    let visible = catalog_rows(false);
    assert!(visible.iter().all(|row| !row.hidden));
    let hidden = catalog_rows(true);
    assert!(
        hidden.iter().any(|row| row.hidden),
        "--hidden should include clap-hidden command rows"
    );
}

#[test]
fn concept_index_resolves_inbox_surfaces() {
    let rows = concept_matches("inbox");
    let surfaces: Vec<&str> = rows.iter().map(|r| r.surface).collect();
    assert!(surfaces.contains(&"aida awaiting"));
    assert!(surfaces.contains(&"aida mailbox inbox"));
    assert!(surfaces.contains(&"aida brief list"));
    assert!(rows.iter().all(|r| !r.why.trim().is_empty()));
}

#[test]
fn concept_index_surfaces_exist_in_clap_catalog() {
    let paths: std::collections::HashSet<String> = catalog_rows(true)
        .into_iter()
        .map(|r| r.path.join(" "))
        .collect();
    for row in CONCEPT_INDEX {
        assert!(
            paths.contains(row.surface),
            "concept `{}` references missing surface `{}`",
            row.concept,
            row.surface
        );
    }
}

#[test]
fn help_corpus_search_finds_command_help_long_tail() {
    let hits = search_help_corpus_in_memory("pickup brief", 5);
    assert!(
        hits.iter().any(|hit| hit.path.starts_with("aida brief")),
        "expected brief help hit, got {hits:?}"
    );
}

#[test]
fn help_query_telemetry_term_privacy_filter() {
    assert!(is_safe_help_query_term("inbox"));
    assert!(is_safe_help_query_term("cache-rebuild"));
    assert!(!is_safe_help_query_term("STORY-837"));
    assert!(!is_safe_help_query_term("contains/slash"));
    assert!(!is_safe_help_query_term("this-term-is-far-too-long"));
}
