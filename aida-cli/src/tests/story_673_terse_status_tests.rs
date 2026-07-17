use super::*;

fn breakdown(pairs: &[(&str, usize)]) -> std::collections::BTreeMap<String, usize> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), *v))
        .collect::<std::collections::BTreeMap<_, _>>()
}

#[test]
fn requirement_summary_leads_with_open_then_closed_and_points_at_full() {
    let by_status = breakdown(&[
        ("Approved", 128),
        ("InProgress", 7),
        ("Planned", 1),
        ("Completed", 1707),
        ("Rejected", 466),
    ]);
    let line = requirement_breakdown_summary_line(&by_status);
    // Open total = everything non-terminal (128 + 7 + 1 = 136).
    assert!(
        line.starts_with("136 open ("),
        "leads with the OPEN total, got: {line}"
    );
    // Per-status open tail is present (lower-cased).
    assert!(line.contains("128 approved"), "got: {line}");
    assert!(line.contains("7 inprogress"), "got: {line}");
    assert!(line.contains("1 planned"), "got: {line}");
    // Closed tallies follow.
    assert!(line.contains("1707 completed"), "got: {line}");
    assert!(line.contains("466 rejected"), "got: {line}");
    // Terminal states are NOT counted as open.
    assert!(
        !line.contains("1707 completed ·")
            || line.find("open").unwrap() < line.find("completed").unwrap(),
        "completed must come after the open clause, got: {line}"
    );
    // Pointer to the per-status detail.
    assert!(line.contains("aida status --full"), "got: {line}");
}

#[test]
fn requirement_summary_handles_no_open_work() {
    let by_status = breakdown(&[("Completed", 3), ("Rejected", 1)]);
    let line = requirement_breakdown_summary_line(&by_status);
    assert!(line.starts_with("0 open"), "got: {line}");
    assert!(line.contains("3 completed"), "got: {line}");
    assert!(line.contains("1 rejected"), "got: {line}");
}

#[test]
fn requirement_summary_omits_absent_closed_tallies() {
    // No Completed / Rejected rows → those clauses are omitted entirely.
    let by_status = breakdown(&[("Approved", 2), ("Draft", 5)]);
    let line = requirement_breakdown_summary_line(&by_status);
    assert!(line.starts_with("7 open ("), "got: {line}");
    assert!(!line.contains("completed"), "got: {line}");
    assert!(!line.contains("rejected"), "got: {line}");
}

#[test]
fn open_prs_collapse_threshold_is_small() {
    // The terse default caps the open-PR roster at a small number so a
    // large fleet of PRs doesn't dump a wall; the full list is behind
    // `--full`. Guard the constant so the collapse stays terse.
    assert!(
        OPEN_PRS_SUMMARY_THRESHOLD <= 5,
        "open-PR terse cap must stay small (got {OPEN_PRS_SUMMARY_THRESHOLD})"
    );
}
