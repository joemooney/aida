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
