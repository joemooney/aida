//! TASK-1915 (F3): a mandate-released ship that fails before or at its merge
//! writes a `refused` MandateDriveAction row under the mandate.
//
// trace:TASK-1915 | ai:claude

use super::*;
use crate::mandate_drive::{ledger_rows, Outcome};

#[test]
fn a_failed_mandate_ship_is_ledgered_as_refused() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    let cover = MandateCover {
        id: "m-1".into(),
        principal: "joe".into(),
    };
    ledger_mandate_ship_failure(
        root.path(),
        Some(&cover),
        42,
        "`gh pr merge` failed: base branch was modified",
    );
    let rows = ledger_rows(root.path(), "m-1");
    assert_eq!(rows.len(), 1, "{rows:?}");
    let (_, actor, row) = &rows[0];
    assert_eq!(actor, "session");
    assert_eq!(row.lane, "pr:42");
    assert_eq!(row.action, "ship");
    assert_eq!(row.outcome, Outcome::Refused);
    assert!(row
        .why
        .as_deref()
        .is_some_and(|w| w.contains("base branch was modified")));
}

#[test]
fn a_ship_no_mandate_released_writes_no_row() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    ledger_mandate_ship_failure(root.path(), None, 42, "`gh pr merge` failed");
    assert!(crate::events::read_all_with_archive(root.path()).is_empty());
}

// Wiring pin (behavioural seam impractical: these exits sit inside
// `pr_ship_with_method` behind live forge, CI and merge-lease calls). Every
// early exit between the mandate release decision and the merge must ledger
// its refusal: each `return Err(` / `bail!(` needs its own
// `ledger_mandate_ship_failure(` call since the previous exit, so deleting a
// call site, or adding an exit without one, turns this red.
// trace:TASK-1915 | ai:claude
#[test]
fn every_exit_between_the_mandate_release_and_the_merge_ledgers_a_refusal() {
    let src = include_str!("../pr_cmd.rs");
    let body = &src[src
        .find(concat!("pub(crate) fn pr_ship_", "with_method("))
        .expect("pr_ship_with_method exists")..];
    let start = body
        .find(concat!("let mandate_release = (!floor_", "authority)"))
        .expect("the mandate release decision");
    let end = body
        .find(concat!(
            "STORY-1700: a ship under a mandate is a ",
            "mandate-driven action"
        ))
        .expect("the post-merge success ledger");
    let lines: Vec<&str> = body[start..end].lines().collect();
    let mut segment = 0;
    let mut exits = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if t.starts_with("return Err(") || t.contains("bail!(") {
            assert!(
                lines[segment..=n]
                    .iter()
                    .any(|l| l.contains(concat!("ledger_mandate_ship_", "failure("))),
                "exit `{t}` (line {n} after the release decision) writes no MandateDriveAction row"
            );
            exits.push(t);
            segment = n + 1;
        }
    }
    // The release floor, the refusal hold, the refusal pin, the rebase pin,
    // both hold-gate timeouts, the mandate ending, the proven base, and the
    // merge failure with and without a hold to re-arm.
    assert_eq!(exits.len(), 10, "{exits:#?}");
}
