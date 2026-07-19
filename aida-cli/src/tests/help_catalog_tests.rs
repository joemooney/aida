// Tests for the clap-derived `aida help commands` catalog.
// trace:TASK-1098 | ai:claude

use super::*;

#[test]
fn catalog_is_derived_and_comprehensive() {
    let rows = catalog_rows();
    // The surface is large; a hand-maintained list would rot. The derived
    // walk should see well north of 100 commands+subcommands.
    assert!(
        rows.len() > 100,
        "expected a large derived catalog, got {} rows",
        rows.len()
    );

    let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
    // Known top-level commands.
    for expected in ["aida list", "aida show", "aida add", "aida status"] {
        assert!(
            paths.contains(&expected),
            "missing top-level row {expected}"
        );
    }
    // Known nested subcommands — the whole point of the catalog.
    for expected in ["aida queue work", "aida db sync", "aida cache rebuild"] {
        assert!(paths.contains(&expected), "missing nested row {expected}");
    }
}

#[test]
fn catalog_rows_are_full_runnable_paths_sorted() {
    let rows = catalog_rows();
    // Every row starts with the binary name so each line is copy-runnable.
    assert!(rows.iter().all(|r| r.path.starts_with("aida ")));
    // Sorted, so a family's subcommands read directly under the family head.
    let mut sorted: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
    let original = sorted.clone();
    sorted.sort();
    assert_eq!(original, sorted, "catalog rows must be sorted by path");
    // Paths are unique (clap enforces per-level uniqueness; the walk must
    // not duplicate).
    let mut deduped = original.clone();
    deduped.dedup();
    assert_eq!(original.len(), deduped.len(), "duplicate catalog rows");
}

#[test]
fn catalog_skips_claps_auto_help_subcommands() {
    let rows = catalog_rows();
    assert!(
        rows.iter()
            .all(|r| !r.path.split_whitespace().any(|seg| seg == "help")),
        "clap's auto-generated `help` navigation rows must be skipped"
    );
}

#[test]
fn catalog_descriptions_carry_no_spec_ids() {
    // SPEC-IDs stay in developer artifacts, never user-facing output.
    let rows = catalog_rows();
    for row in &rows {
        for prefix in ["TASK-", "BUG-", "STORY-", "EPIC-", "SPIKE-"] {
            if let Some(idx) = row.about.find(prefix) {
                let after = &row.about[idx + prefix.len()..];
                assert!(
                    !after.starts_with(|c: char| c.is_ascii_digit()),
                    "SPEC-ID leaked into catalog row `{}`: {}",
                    row.path,
                    row.about
                );
            }
        }
    }
}
