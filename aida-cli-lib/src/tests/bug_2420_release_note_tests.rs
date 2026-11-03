//! A refused delegated add never loses its dispensed agreed ID silently: the
//! note says it was returned, burned (a concurrent add advanced the block),
//! or could not be returned.
// trace:BUG-2420 | ai:claude

use crate::requirements_cli::undelivered_release_note;

#[test]
fn bug_2420_returned_id_is_named() {
    let note = undelivered_release_note("BUG-102", &Ok(true));
    assert!(note.contains("BUG-102 was returned"), "{note}");
}

#[test]
fn bug_2420_burned_id_is_named_when_a_concurrent_add_advanced_the_block() {
    let note = undelivered_release_note("BUG-102", &Ok(false));
    assert!(note.contains("BUG-102 is burned"), "{note}");
    assert!(note.contains("will not be issued"), "{note}");
}

#[test]
fn bug_2420_failed_release_is_a_warning() {
    let note = undelivered_release_note("BUG-102", &Err(anyhow::anyhow!("disk full")));
    assert!(note.starts_with("Warning:"), "{note}");
    assert!(
        note.contains("BUG-102") && note.contains("disk full"),
        "{note}"
    );
}
