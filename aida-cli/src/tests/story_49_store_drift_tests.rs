use super::{store_drift_verdict, StoreDriftVerdict};

#[test]
fn aligned_when_paired_equals_current() {
    // Ancestry is irrelevant when the two SHAs are equal.
    assert_eq!(
        store_drift_verdict(Some("abc123"), Some("abc123"), false),
        StoreDriftVerdict::Aligned
    );
}

#[test]
fn whitespace_around_shas_is_ignored() {
    assert_eq!(
        store_drift_verdict(Some("  abc123\n"), Some("abc123"), false),
        StoreDriftVerdict::Aligned
    );
}

// BUG-584: the store legitimately advances ahead of the pin on every
// normal `aida add` / `aida done`. When the paired SHA is an ancestor of
// the current store HEAD, that is the healthy "store ahead" state — NOT
// drift. This is the false-positive the day-one dogfood hit.
#[test]
fn store_ahead_is_not_drift() {
    assert_eq!(
        store_drift_verdict(Some("abc123"), Some("def456"), /* is_ancestor */ true),
        StoreDriftVerdict::StoreAhead
    );
}

// BUG-584: genuine divergence — paired SHA is NOT an ancestor of the
// current store HEAD (history rewrite / rewind, or paired SHA missing
// locally). This must still be reported as drift.
#[test]
fn genuine_divergence_is_drift() {
    assert_eq!(
        store_drift_verdict(Some("abc123"), Some("def456"), /* is_ancestor */ false),
        StoreDriftVerdict::Diverged
    );
}

#[test]
fn no_verdict_without_a_paired_sha() {
    // No `Aida-Store:` trailer on the commit — nothing to compare.
    assert_eq!(
        store_drift_verdict(None, Some("abc123"), false),
        StoreDriftVerdict::NoVerdict
    );
}

#[test]
fn no_verdict_without_a_current_store_head() {
    // No `.aida-store/` worktree — nothing to compare.
    assert_eq!(
        store_drift_verdict(Some("abc123"), None, false),
        StoreDriftVerdict::NoVerdict
    );
}

#[test]
fn no_verdict_when_both_absent() {
    assert_eq!(
        store_drift_verdict(None, None, false),
        StoreDriftVerdict::NoVerdict
    );
}
