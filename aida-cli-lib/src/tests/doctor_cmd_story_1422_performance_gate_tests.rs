// trace:TASK-3780 | ai:codex
use super::*;

/// Fixed clock: the window decision must not depend on when the suite runs.
fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-09-21T12:00:00+00:00")
        .unwrap()
        .with_timezone(&chrono::Utc)
}

fn ev_at(cmd: &str, duration_ms: u64, hours_ago: i64) -> crate::usage::UsageEvent {
    crate::usage::UsageEvent {
        ts: (now() - chrono::Duration::hours(hours_ago)).to_rfc3339(),
        cmd: cmd.to_string(),
        args_count: 0,
        exit_code: 0,
        duration_ms,
        binary_sha: None,
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

/// `slow` of `total` calls over budget, all inside the window.
fn mix(slow: usize, total: usize) -> Vec<crate::usage::UsageEvent> {
    (0..total)
        .map(|i| ev_at("show", if i < slow { 9_900 } else { 400 }, 1))
        .collect()
}

/// TODAY'S MEASURED POPULATION trips: 20.3% of calls over a 1000ms budget
/// against the ruled 10% tolerance.
// trace:STORY-1422 | ai:claude
#[test]
fn the_measured_population_trips_the_gate() {
    let breaches = performance_breaches(
        &mix(203, 1000),
        &[budget("show", 1000)],
        &PerformancePolicy::default(),
        now(),
        &BinaryLineage::unscoped(),
    );
    assert_eq!(breaches.len(), 1, "{breaches:?}");
    assert_eq!(breaches[0].over_budget, 203);
    assert_eq!(breaches[0].samples, 1000);
    assert!((breaches[0].over_pct - 20.3).abs() < 0.01);
}

/// THE ESTIMATED POST-FIX POPULATION MUST NOT TRIP. If the slow mode goes
/// away, the residual 1s-4s band is ~7.1% of traffic. A 5% tolerance — the
/// number this seat first proposed — would leave the gate permanently red
/// AFTER the bug it guards was fixed. This test is why 5% was rejected, and
/// it pins the rejected value so re-tightening is a deliberate act.
// trace:STORY-1422 | ai:claude
#[test]
fn the_estimated_post_fix_population_does_not_trip() {
    let events = mix(71, 1000); // 7.1%
    let budgets = [budget("show", 1000)];
    assert!(
        performance_breaches(
            &events,
            &budgets,
            &PerformancePolicy::default(),
            now(),
            &BinaryLineage::unscoped()
        )
        .is_empty(),
        "7.1% must clear the ruled 10% tolerance"
    );
    let five_pct = PerformancePolicy {
        tolerated_pct: 5.0,
        window_hours: 24,
    };
    assert_eq!(
        performance_breaches(
            &events,
            &budgets,
            &five_pct,
            now(),
            &BinaryLineage::unscoped()
        )
        .len(),
        1,
        "and 5% would have tripped on it — the rejected threshold, pinned"
    );
}

/// The window is the filter, proved in both directions.
// trace:STORY-1422 | ai:claude
#[test]
fn calls_outside_the_window_are_not_judged() {
    let mut events: Vec<_> = (0..100).map(|_| ev_at("show", 9_900, 48)).collect();
    events.extend((0..100).map(|_| ev_at("show", 400, 1)));
    let budgets = [budget("show", 1000)];
    assert!(
        performance_breaches(
            &events,
            &budgets,
            &PerformancePolicy::default(),
            now(),
            &BinaryLineage::unscoped()
        )
        .is_empty(),
        "48h-old breaches fall outside a 24h window"
    );
    let wide = PerformancePolicy {
        tolerated_pct: 10.0,
        window_hours: 72,
    };
    assert_eq!(
        performance_breaches(&events, &budgets, &wide, now(), &BinaryLineage::unscoped()).len(),
        1,
        "widening to 72h brings them back — so the window IS the filter"
    );
}

/// THE TYPO CASE, window-aware: a budget whose shape produced nothing
/// RECENTLY is a finding, not silence.
// trace:STORY-1422 | ai:claude
#[test]
fn a_budget_with_no_recent_calls_is_a_finding_not_a_pass() {
    let events = vec![ev_at("show", 400, 1)];
    let policy = PerformancePolicy::default();
    let typo = [budget("shwo", 1000)];
    let unscoped = BinaryLineage::unscoped();
    assert!(performance_breaches(&events, &typo, &policy, now(), &unscoped).is_empty());
    assert_eq!(
        performance_unobserved(&events, &typo, &policy, now(), &unscoped),
        vec![UnobservedBudget {
            cmd: "shwo".to_string(),
            excluded_samples: 0,
        }]
    );
    let findings = performance_findings(&events, &typo, &policy, now(), &unscoped);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].summary.contains("NO recorded calls"));

    let stale = vec![ev_at("show", 9_900, 48)];
    assert_eq!(
        performance_unobserved(&stale, &[budget("show", 1000)], &policy, now(), &unscoped),
        vec![UnobservedBudget {
            cmd: "show".to_string(),
            excluded_samples: 0,
        }]
    );
}

/// The trip records the numbers IN FORCE, so a later reader can tell a
/// fixed regression from a raised ceiling.
// trace:STORY-1422 | ai:claude
#[test]
fn a_trip_records_the_budget_and_tolerance_in_force() {
    let policy = PerformancePolicy {
        tolerated_pct: 10.0,
        window_hours: 24,
    };
    let events = mix(300, 1000);
    let budgets = [budget("show", 1234)];
    let b = &performance_breaches(
        &events,
        &budgets,
        &policy,
        now(),
        &BinaryLineage::unscoped(),
    )[0];
    assert_eq!(b.budget_ms, 1234);
    assert_eq!(b.tolerated_pct, 10.0);
    assert_eq!(b.window_hours, 24);
    assert_eq!(b.worst_ms, 9_900);
    let summary = &performance_findings(
        &events,
        &budgets,
        &policy,
        now(),
        &BinaryLineage::unscoped(),
    )[0]
    .summary;
    assert!(summary.contains("1234 ms"), "{summary}");
    assert!(summary.contains("tolerated 10.0%"), "{summary}");
}

/// Per-shape budgets, including a shape containing a space.
// trace:STORY-1422 | ai:claude
#[test]
fn budgets_are_per_shape_and_tolerate_spaces() {
    let cfg: toml::Value = "[performance.budgets]\nshow = 1000\n\"queue list\" = 2000\n"
        .parse()
        .unwrap();
    assert_eq!(
        performance_budgets(Some(&cfg)),
        vec![budget("queue list", 2000), budget("show", 1000)]
    );
    assert!(performance_budgets(None).is_empty());
}

/// A zero or non-integer budget is ignored rather than treated as "every
/// call breaches", so a malformed config cannot manufacture findings.
// trace:STORY-1422 | ai:claude
#[test]
fn malformed_budgets_are_ignored_not_treated_as_zero() {
    let cfg: toml::Value = "[performance.budgets]\nshow = 0\nlist = \"fast\"\n"
        .parse()
        .unwrap();
    assert!(performance_budgets(Some(&cfg)).is_empty());
}

/// Policy is configurable; absent or out-of-range values fall back to the
/// provisional default rather than silently disabling the gate.
// trace:STORY-1422 | ai:claude
#[test]
fn policy_falls_back_to_the_provisional_default() {
    assert_eq!(performance_policy(None), PerformancePolicy::default());
    let ok: toml::Value = "[performance]\ntolerated_over_budget_pct = 25\nwindow_hours = 6\n"
        .parse()
        .unwrap();
    let p = performance_policy(Some(&ok));
    assert_eq!(p.tolerated_pct, 25.0);
    assert_eq!(p.window_hours, 6);
    let bad: toml::Value = "[performance]\ntolerated_over_budget_pct = 900\nwindow_hours = 0\n"
        .parse()
        .unwrap();
    assert_eq!(performance_policy(Some(&bad)), PerformancePolicy::default());
}

/// ADR-53: `ceiling_ms` parses from the table form, and the plain-integer
/// form still parses with `ceiling_ms: None` — the pre-ADR-53 config keeps
/// working unchanged.
// trace:ADR-53 | ai:claude
#[test]
fn ceiling_ms_parses_from_the_table_form_and_defaults_to_none() {
    let cfg: toml::Value =
            "[performance.budgets]\nshow = { budget_ms = 1000, ceiling_ms = 5000 }\n\"queue list\" = 2000\n"
                .parse()
                .unwrap();
    let budgets = performance_budgets(Some(&cfg));
    assert_eq!(
        budgets,
        vec![
            budget("queue list", 2000),
            PerformanceBudget {
                cmd: "show".to_string(),
                budget_ms: 1000,
                ceiling_ms: Some(5000),
            },
        ]
    );
}

/// A zero/non-positive `ceiling_ms` is ignored (treated as unset) rather
/// than manufacturing a ceiling that trips on every call — same rule as a
/// malformed `budget_ms`.
// trace:ADR-53 | ai:claude
#[test]
fn a_malformed_ceiling_ms_is_ignored_not_treated_as_zero() {
    let cfg: toml::Value = "[performance.budgets]\nshow = { budget_ms = 1000, ceiling_ms = 0 }\n"
        .parse()
        .unwrap();
    assert_eq!(performance_budgets(Some(&cfg)), vec![budget("show", 1000)]);
}

fn budget_with_ceiling(cmd: &str, budget_ms: u64, ceiling_ms: u64) -> PerformanceBudget {
    PerformanceBudget {
        cmd: cmd.to_string(),
        budget_ms,
        ceiling_ms: Some(ceiling_ms),
    }
}

/// ADR-53's whole point: a single call over the ceiling trips the gate
/// even while the PROPORTION sits comfortably under tolerance — the exact
/// gap the measured 2.9%-at-n=170 / 69,240ms-worst-call reading exposed.
// trace:ADR-53 | ai:claude
#[test]
fn a_single_call_over_the_ceiling_trips_the_gate_even_with_a_healthy_proportion() {
    // 1 of 170 over budget — 0.6%, nowhere near the 10% tolerance — but
    // that one call is a 69,240ms outlier, over the 5000ms ceiling.
    let mut events: Vec<_> = (0..169).map(|_| ev_at("show", 400, 1)).collect();
    events.push(ev_at("show", 69_240, 1));
    let budgets = [budget_with_ceiling("show", 1000, 5_000)];
    let breaches = performance_breaches(
        &events,
        &budgets,
        &PerformancePolicy::default(),
        now(),
        &BinaryLineage::unscoped(),
    );
    assert_eq!(breaches.len(), 1, "{breaches:?}");
    assert!(
        breaches[0].over_pct < PerformancePolicy::default().tolerated_pct,
        "the proportion limb alone must NOT explain this trip: {}",
        breaches[0].over_pct
    );
    assert!(breaches[0].ceiling_breached);
    assert_eq!(breaches[0].worst_ms, 69_240);

    let summary = &performance_findings(
        &events,
        &budgets,
        &PerformancePolicy::default(),
        now(),
        &BinaryLineage::unscoped(),
    )[0]
    .summary;
    assert!(summary.contains("CEILING BREACHED"), "{summary}");
    assert!(summary.contains("69240 ms"), "{summary}");
    assert!(summary.contains("5000 ms ceiling"), "{summary}");
}

/// The mirror image: proportion AND ceiling both clear must NOT trip —
/// a configured-but-unbreached ceiling is not itself a finding.
// trace:ADR-53 | ai:claude
#[test]
fn a_configured_ceiling_that_is_not_breached_does_not_trip() {
    let events = mix(29, 1000); // 2.9% over budget, worst call 9,900ms
    let budgets = [budget_with_ceiling("show", 1000, 15_000)];
    assert!(
        performance_breaches(
            &events,
            &budgets,
            &PerformancePolicy::default(),
            now(),
            &BinaryLineage::unscoped()
        )
        .is_empty(),
        "2.9% clears the 10% tolerance and the 9,900ms worst call clears a 15,000ms ceiling"
    );
}

/// STORY-1423 criterion 1 (ADR-53's "quiet on both limbs" amendment): a
/// dogfood regression test over this repo's OWN `.aida/config.toml`, not a
/// fixture. Every prior confirmation that the `show` budget carries a
/// `ceiling_ms` was a point-in-time reading (`aida show --json` on some
/// date); nothing stopped a later config edit from quietly dropping the
/// table form back to a plain integer and losing the second limb without
/// any test failing. Reads the real file via `CARGO_MANIFEST_DIR` so this
/// fails the moment the dogfood surface regresses. The 2000 ms `budget_ms`
/// is the operator's deliberate 2026-10-29 decision for this build-loaded
/// host (TASK-HOME-116); the ceiling still catches a hung call.
// trace:STORY-1423 trace:ADR-53 trace:TASK-b6b1278890774ef993d2ebc327862470-116 | ai:claude
#[test]
fn this_repo_configures_a_ceiling_for_the_show_budget() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("aida-cli-lib has a workspace parent");
    let body = std::fs::read_to_string(repo_root.join(".aida/config.toml"))
        .expect("this repo's .aida/config.toml must be readable");
    let cfg: toml::Value = body
        .parse()
        .expect("this repo's .aida/config.toml must be valid TOML");
    let budgets = performance_budgets(Some(&cfg));
    let show = budgets
        .iter()
        .find(|b| b.cmd == "show")
        .expect("this repo must declare a [performance.budgets] entry for `show`");
    assert_eq!(
        show.budget_ms, 2000,
        "budget_ms must stay the operator's 2026-10-29 build-loaded-host value (TASK-HOME-116)"
    );
    assert!(
        show.ceiling_ms.is_some(),
        "the `show` budget must keep ADR-53's ceiling_ms limb, not regress to proportion-only"
    );
}
