use super::*;

// STORY-737 (delight #4): `aida history` hides META by default, but
// `--include-meta` and an explicit `--type meta` both keep it visible.
#[test]
fn history_meta_hidden_by_default_but_reachable() {
    // Default view: META is excluded.
    assert!(history_should_exclude_meta(false, None));
    assert!(history_should_exclude_meta(false, Some("bug")));
    // `--include-meta` keeps META visible regardless of type filter.
    assert!(!history_should_exclude_meta(true, None));
    assert!(!history_should_exclude_meta(true, Some("meta")));
    // An explicit `--type meta` keeps META visible (case-insensitive).
    assert!(!history_should_exclude_meta(false, Some("meta")));
    assert!(!history_should_exclude_meta(false, Some("META")));
}

// STORY-737 (delight #5): the empty-queue soft signpost is an Error type
// (so the existing `?`/`Result` plumbing carries it) but the top-level
// handler suppresses its render — proven here via the downcast the handler
// performs.
#[test]
fn soft_signpost_is_downcastable_sentinel() {
    let err: anyhow::Error = anyhow::Error::new(SoftSignpostShown);
    assert!(
        err.downcast_ref::<SoftSignpostShown>().is_some(),
        "the top-level handler keys off this downcast to skip the red render"
    );
}
