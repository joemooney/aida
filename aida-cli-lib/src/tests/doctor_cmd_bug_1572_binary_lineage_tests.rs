// trace:TASK-3780 | ai:codex
use super::*;

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-09-21T12:00:00+00:00")
        .unwrap()
        .with_timezone(&chrono::Utc)
}

fn ev(cmd: &str, duration_ms: u64, sha: Option<&str>) -> crate::usage::UsageEvent {
    crate::usage::UsageEvent {
        ts: (now() - chrono::Duration::hours(1)).to_rfc3339(),
        cmd: cmd.to_string(),
        args_count: 0,
        exit_code: 0,
        duration_ms,
        binary_sha: sha.map(|s| s.to_string()),
        role: None,
        scope: None,
        schedule_source: None,
        phases: Vec::new(),
    }
}

fn budget(cmd: &str, budget_ms: u64) -> PerformanceBudget {
    PerformanceBudget {
        cmd: cmd.to_string(),
        budget_ms,
        ceiling_ms: None,
    }
}

/// `slow` of `total` `show` calls over a 1000ms budget, all served by `sha`.
fn calls(sha: &str, slow: usize, total: usize) -> Vec<crate::usage::UsageEvent> {
    (0..total)
        .map(|i| ev("show", if i < slow { 9_900 } else { 400 }, Some(sha)))
        .collect()
}

/// THE DEFECT, BOTH DIRECTIONS. One window, two binaries: `inlineage` under
/// budget and `otherlane` far over it. Scoped to HEAD's lineage the gate
/// must stay QUIET — a noisy lane cannot hold main red. Flip WHICH binary
/// is slow and the same window MUST trip — otherwise the scope is not a
/// discriminator, it is just a mute button. One direction is half a test.
// trace:BUG-1572 | ai:claude
#[test]
fn a_noisy_off_lineage_binary_does_not_trip_the_in_lineage_gate_but_a_noisy_in_lineage_one_does() {
    let budgets = [budget("show", 1000)];
    let policy = PerformancePolicy::default();
    let scoped = BinaryLineage::scoped(["otherlane".to_string()]);

    // Direction 1: in-lineage clean (5%), off-lineage filthy (80%).
    let mut mixed = calls("inlineage", 5, 100);
    mixed.extend(calls("otherlane", 80, 100));
    assert!(
        performance_breaches(&mixed, &budgets, &policy, now(), &scoped).is_empty(),
        "a lane's own noisy build must not hold this HEAD's gate red"
    );
    // …and the un-scoped population WOULD have tripped, so the scope is
    // what quiets it — not an accidentally-clean fixture.
    assert_eq!(
        performance_breaches(&mixed, &budgets, &policy, now(), &BinaryLineage::unscoped()).len(),
        1,
        "unscoped, the SAME window trips at 42.5% — that is the defect"
    );

    // Direction 2: swap which binary is slow. Same shape, same counts.
    let mut regressed = calls("inlineage", 80, 100);
    regressed.extend(calls("otherlane", 5, 100));
    let breaches = performance_breaches(&regressed, &budgets, &policy, now(), &scoped);
    assert_eq!(
        breaches.len(),
        1,
        "a real regression ON THIS LINEAGE must still trip"
    );
    assert_eq!(breaches[0].samples, 100, "denominator is the scoped set");
    assert_eq!(breaches[0].over_budget, 80);
    assert_eq!(breaches[0].excluded_samples, 100);
}

/// The emitted finding reports the denominator PER BINARY, counted and
/// excluded alike — the mixed population is visible instead of averaged.
// trace:BUG-1572 | ai:claude
#[test]
fn the_finding_reports_the_denominator_per_binary() {
    let mut events = calls("inlineage", 80, 100);
    events.extend(calls("otherlane", 5, 40));
    let findings = performance_findings(
        &events,
        &[budget("show", 1000)],
        &PerformancePolicy::default(),
        now(),
        &BinaryLineage::scoped(["otherlane".to_string()]),
    );
    assert_eq!(findings.len(), 1, "{findings:?}");
    let s = &findings[0].summary;
    assert!(s.contains("80.0% of 100 calls"), "aggregate stays: {s}");
    assert!(s.contains("counted: inlineage 80.0% of 100"), "{s}");
    assert!(
        s.contains("excluded 40 calls from 1 binary outside this HEAD's lineage"),
        "{s}"
    );
    assert!(s.contains("otherlane 12.5% of 40"), "{s}");
}

/// FAIL OPEN on a sha that cannot be placed. A row with no `binary_sha`,
/// and a sha git never classified (short, purged, or from another
/// project's repo), are COUNTED — only a positively off-lineage sha is
/// dropped. Silently dropping unplaceable rows would shrink the
/// denominator, which is the defect being fixed; failing closed on them
/// would make the gate unusable.
// trace:BUG-1572 | ai:claude
#[test]
fn unplaceable_and_absent_shas_are_counted_not_dropped() {
    let scoped = BinaryLineage::scoped(["otherlane".to_string()]);
    assert!(scoped.counts(None), "absent binary_sha counts");
    assert!(
        scoped.counts(Some("abc")),
        "a short/unresolvable sha counts"
    );
    assert!(
        scoped.counts(Some("deadbeefdeadbeef")),
        "a sha from another project's repo counts"
    );
    assert!(
        !scoped.counts(Some("otherlane")),
        "only proven off-lineage drops"
    );
    assert!(
        scoped.counts(Some("INLINEAGE")) && !scoped.counts(Some("OTHERLANE")),
        "sha matching is case-insensitive in both directions"
    );

    // …and it shows up in the population, not just the predicate.
    let mut events = calls("otherlane", 10, 10);
    events.push(ev("show", 9_900, None));
    events.push(ev("show", 9_900, Some("abc")));
    let breaches = performance_breaches(
        &events,
        &[budget("show", 1000)],
        &PerformancePolicy::default(),
        now(),
        &scoped,
    );
    assert_eq!(breaches.len(), 1, "{breaches:?}");
    assert_eq!(
        breaches[0].samples, 2,
        "the unplaceable rows form the denominator"
    );
}

/// An unscoped lineage is byte-for-byte the pre-BUG-1572 behaviour, so the
/// no-git-repo / opted-out path cannot silently change a verdict.
// trace:BUG-1572 | ai:claude
#[test]
fn an_unscoped_lineage_counts_every_binary() {
    let unscoped = BinaryLineage::unscoped();
    assert!(!unscoped.is_scoped());
    assert!(unscoped.counts(None) && unscoped.counts(Some("anything")));
    let mut events = calls("inlineage", 0, 100);
    events.extend(calls("otherlane", 100, 100));
    let b = performance_breaches(
        &events,
        &[budget("show", 1000)],
        &PerformancePolicy::default(),
        now(),
        &unscoped,
    );
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].samples, 200);
    assert_eq!(b[0].excluded_samples, 0);
    assert!(!b[0].lineage_scoped);
    assert!(
        binary_breakdown_phrase(&b[0].per_binary, b[0].lineage_scoped)
            .contains("lineage scope OFF"),
        "an unscoped denominator must SAY it is machine-wide"
    );
}

/// A shape whose every in-window call came from off-lineage binaries is
/// UNOBSERVED, and the finding says so — "no recorded calls" would be
/// plainly false with calls sitting in the window, and would send a reader
/// hunting a telemetry outage instead of a binary mismatch.
// trace:BUG-1572 | ai:claude
#[test]
fn an_all_off_lineage_window_reports_why_it_is_silent() {
    let events = calls("otherlane", 50, 60);
    let budgets = [budget("show", 1000)];
    let policy = PerformancePolicy::default();
    let scoped = BinaryLineage::scoped(["otherlane".to_string()]);
    assert_eq!(
        performance_unobserved(&events, &budgets, &policy, now(), &scoped),
        vec![UnobservedBudget {
            cmd: "show".to_string(),
            excluded_samples: 60,
        }]
    );
    let findings = performance_findings(&events, &budgets, &policy, now(), &scoped);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(
        findings[0]
            .summary
            .contains("60 in-window call(s) were served"),
        "{}",
        findings[0].summary
    );
}

/// The opt-out env value is parsed, not guessed.
// trace:BUG-1572 | ai:claude
#[test]
fn the_opt_out_recognizes_only_affirmative_values() {
    for on in ["1", "true", "yes", "on", " 1 "] {
        assert!(perf_gate_scope_disabled_by(Some(on)), "{on}");
    }
    for off in ["0", "false", "", "no", "off", "maybe"] {
        assert!(!perf_gate_scope_disabled_by(Some(off)), "{off}");
    }
    assert!(!perf_gate_scope_disabled_by(None));
}

/// The tally is ordered by sample count descending, so the breakdown is
/// stable across runs and the biggest contributor reads first.
// trace:BUG-1572 | ai:claude
#[test]
fn the_per_binary_tally_is_stable_and_labels_absent_shas() {
    let window: Vec<(Option<&str>, u64)> = vec![
        (Some("aaa"), 9_900),
        (Some("bbb"), 400),
        (Some("bbb"), 400),
        (Some("bbb"), 9_900),
        (None, 400),
    ];
    let tallies = tally_by_binary(&window, 1000, &BinaryLineage::scoped(["bbb".to_string()]));
    assert_eq!(
        tallies
            .iter()
            .map(|t| (t.sha.as_str(), t.samples, t.over_budget, t.counted))
            .collect::<Vec<_>>(),
        vec![
            ("bbb", 3, 1, false),
            ("aaa", 1, 1, true),
            (UNKNOWN_BINARY_LABEL, 1, 0, true),
        ],
        "biggest contributor first, then sha — and \"aaa\" sorts before the \
             absent-sha label, so the tie-break is by name, not by insertion order"
    );
}
