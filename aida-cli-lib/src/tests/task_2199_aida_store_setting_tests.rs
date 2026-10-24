//! TASK-2199 (TASK-1955; duplicates TASK-1705..1707): a blank or
//! whitespace-only `AIDA_STORE` is unset, so the seat check never prints a
//! spurious "AIDA_STORE is …" note; a set value is trimmed.
// trace:TASK-1955 | ai:claude (TASK-2199)

use super::*;

#[test]
fn a_blank_or_whitespace_aida_store_is_unset() {
    for raw in [None, Some(""), Some("   "), Some("\t\n ")] {
        assert!(aida_store_setting_from(raw).is_none(), "{raw:?}");
    }
    let dir = tempfile::tempdir().unwrap();
    let padded = format!("  {}  ", dir.path().display());
    let (value, _) = aida_store_setting_from(Some(&padded)).expect("a set value");
    assert_eq!(value, dir.path().display().to_string());
}
