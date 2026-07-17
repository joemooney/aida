use super::*;
use aida_core::RequirementStatus;

#[test]
fn bucket_classification_covers_every_terminal_axis() {
    // trace:TASK-232 | ai:claude
    assert_eq!(
        classify_progress_bucket(&RequirementStatus::Completed),
        ProgressBucket::Shipped
    );
    assert_eq!(
        classify_progress_bucket(&RequirementStatus::Rejected),
        ProgressBucket::Shipped
    );
    assert_eq!(
        classify_progress_bucket(&RequirementStatus::Done),
        ProgressBucket::InFlight
    );
    assert_eq!(
        classify_progress_bucket(&RequirementStatus::InProgress),
        ProgressBucket::WorkingNow
    );
    assert_eq!(
        classify_progress_bucket(&RequirementStatus::Approved),
        ProgressBucket::Remaining
    );
    assert_eq!(
        classify_progress_bucket(&RequirementStatus::Draft),
        ProgressBucket::Remaining
    );
    assert_eq!(
        classify_progress_bucket(&RequirementStatus::Planned),
        ProgressBucket::Remaining
    );
}

#[test]
fn shelved_callout_is_additive_not_a_rebucketing() {
    // trace:STORY-490 | ai:claude
    // NeedsAttention is the only shelved status...
    assert!(status_is_shelved(&RequirementStatus::NeedsAttention));
    for s in [
        RequirementStatus::Completed,
        RequirementStatus::Rejected,
        RequirementStatus::Done,
        RequirementStatus::InProgress,
        RequirementStatus::Approved,
        RequirementStatus::Draft,
        RequirementStatus::Planned,
    ] {
        assert!(!status_is_shelved(&s), "{s:?} must not count as shelved");
    }
    // ...yet it STILL buckets into Remaining (STORY-332 preserved): the
    // shelved callout is an additive signal, not a re-bucketing.
    assert_eq!(
        classify_progress_bucket(&RequirementStatus::NeedsAttention),
        ProgressBucket::Remaining
    );
}

#[test]
fn parse_since_accepts_relative_d_h_m() {
    // trace:TASK-232 | ai:claude
    let now = chrono::Utc::now();
    let two_days = parse_since_arg("2d").unwrap();
    assert!((now - two_days).num_hours() >= 47);
    assert!((now - two_days).num_hours() <= 49);

    let twelve_hours = parse_since_arg("12h").unwrap();
    assert!((now - twelve_hours).num_minutes() >= 11 * 60);

    let forty_five_min = parse_since_arg("45m").unwrap();
    assert!((now - forty_five_min).num_minutes() >= 44);
}

#[test]
fn parse_since_accepts_rfc3339() {
    // trace:TASK-232 | ai:claude
    let ts = parse_since_arg("2026-05-01T00:00:00Z").unwrap();
    assert_eq!(ts.format("%Y-%m-%d").to_string(), "2026-05-01");
}

#[test]
fn parse_since_rejects_garbage() {
    // trace:TASK-232 | ai:claude
    assert!(parse_since_arg("").is_err());
    assert!(parse_since_arg("xyz").is_err());
    assert!(parse_since_arg("3z").is_err());
}

/// BUG-100: a multi-byte trailing char (e.g. `2日`) used to crash
/// the process via `split_at` on a non-char-boundary byte. After the
// fix it returns a clean Err. trace:BUG-100 | ai:claude
#[test]
fn parse_since_does_not_panic_on_multibyte_unit() {
    // Inputs from the bug repro section.
    assert!(parse_since_arg("2日").is_err());
    assert!(parse_since_arg("3秒").is_err());
    // Pure multi-byte string (no digits to parse, last char is the
    // only char): still a clean error.
    assert!(parse_since_arg("日").is_err());
    // Multi-char multi-byte trailer: also bails cleanly.
    assert!(parse_since_arg("3日間").is_err());
}

/// BUG-100: mirror coverage for parse_days_arg, which uses the same
// split-last-char path. trace:BUG-100 | ai:claude
#[test]
fn parse_days_does_not_panic_on_multibyte_unit() {
    assert!(parse_days_arg("2日").is_err());
    assert!(parse_days_arg("3秒").is_err());
    assert!(parse_days_arg("日").is_err());
}

/// BUG-100: the helper itself stays char-boundary-safe for both
// ASCII and multi-byte trailers. trace:BUG-100 | ai:claude
#[test]
fn split_last_char_is_char_boundary_safe() {
    assert_eq!(split_last_char("2d"), ("2", "d"));
    assert_eq!(split_last_char("12h"), ("12", "h"));
    assert_eq!(split_last_char("2日"), ("2", "日"));
    assert_eq!(split_last_char("日"), ("", "日"));
    assert_eq!(split_last_char(""), ("", ""));
}

#[test]
fn batch_tag_matches_case_insensitively() {
    // trace:TASK-229 | ai:claude
    let mut tags = std::collections::HashSet::new();
    tags.insert("batch:Observability".to_string());
    tags.insert("queue".to_string());
    let want = "batch:observability";
    assert!(tags.iter().any(|t| t.eq_ignore_ascii_case(want)));

    let want_miss = "batch:elsewhere";
    assert!(!tags.iter().any(|t| t.eq_ignore_ascii_case(want_miss)));
}

// TASK-270: `batch:NAME` is the literal tag printed by `aida queue
// list`; first-users copy-paste it back. Accept it as a positional id
// and tolerate the redundant prefix on `--batch`. trace:TASK-270
#[test]
fn strip_batch_prefix_only_matches_the_prefix() {
    assert_eq!(
        strip_batch_prefix("batch:plan-tooling"),
        Some("plan-tooling")
    );
    // Case-insensitive on the prefix.
    assert_eq!(strip_batch_prefix("BATCH:Plan"), Some("Plan"));
    assert_eq!(strip_batch_prefix("Batch:x"), Some("x"));
    // Empty name after the prefix is still a (degenerate) match.
    assert_eq!(strip_batch_prefix("batch:"), Some(""));
    // Non-batch identifiers are left alone.
    assert_eq!(strip_batch_prefix("TASK-270"), None);
    assert_eq!(strip_batch_prefix("batchx"), None);
    assert_eq!(strip_batch_prefix(""), None);
    // A multi-byte leading char must not panic the byte-slice.
    assert_eq!(strip_batch_prefix("日本語"), None);
}

#[test]
fn normalize_batch_name_strips_redundant_prefix() {
    // `--batch batch:NAME` and `--batch NAME` collapse to the same name.
    assert_eq!(
        normalize_batch_name("batch:workflow-hint-polish"),
        "workflow-hint-polish"
    );
    assert_eq!(
        normalize_batch_name("workflow-hint-polish"),
        "workflow-hint-polish"
    );
    assert_eq!(normalize_batch_name("BATCH:x"), "x");
}

#[test]
fn queue_work_positional_batch_equals_batch_flag() {
    // The whole point of TASK-270: `aida queue work batch:NAME` and
    // `aida queue work --batch NAME` resolve to the identical
    // (effective_id, effective_batch) pair — None id, Some(name).
    let from_positional = resolve_queue_work_batch(Some("batch:workflow-hint-polish"), None);
    let from_flag = resolve_queue_work_batch(None, Some("workflow-hint-polish"));
    assert_eq!(from_positional, (None, Some("workflow-hint-polish")));
    assert_eq!(from_positional, from_flag);

    // A redundant `batch:` on the flag value is tolerated too.
    assert_eq!(
        resolve_queue_work_batch(None, Some("batch:workflow-hint-polish")),
        (None, Some("workflow-hint-polish"))
    );

    // A plain SPEC-ID positional stays an id, not a batch.
    assert_eq!(
        resolve_queue_work_batch(Some("TASK-270"), None),
        (Some("TASK-270"), None)
    );

    // Bare positional `batch:` resolves to an empty batch name — the
    // Work handler bails on it with a clear message.
    assert_eq!(
        resolve_queue_work_batch(Some("batch:"), None),
        (None, Some(""))
    );

    // No id, no flag → nothing resolved (head-pickup path).
    assert_eq!(resolve_queue_work_batch(None, None), (None, None));
}

/// TASK-322: the predicate behind the `--batch` + `nextN` collision guard.
#[test]
fn is_next_keyword_id_classifies() {
    assert!(is_next_keyword_id("next"));
    assert!(is_next_keyword_id("next3"));
    assert!(is_next_keyword_id("NEXT5"));
    assert!(is_next_keyword_id("Next10"));
    // Not next-keywords.
    assert!(!is_next_keyword_id("TASK-270"));
    assert!(!is_next_keyword_id("batch:foo"));
    assert!(!is_next_keyword_id("nextish")); // suffix not all-digits
    assert!(!is_next_keyword_id("next-3")); // hyphen is not a digit
}

#[test]
fn parse_batch_chain_preserves_order_and_strips_prefixes() {
    assert_eq!(
        parse_batch_chain("batch:display-polish, workflow-hint-polish,BUGS").unwrap(),
        vec!["display-polish", "workflow-hint-polish", "BUGS"]
    );
}

#[test]
fn parse_batch_chain_rejects_empty_names() {
    let err = parse_batch_chain("alpha,,beta").unwrap_err();
    assert!(
        err.to_string().contains("empty name"),
        "unexpected error: {err}"
    );
}
