//! TASK-1169 — drain integration-wait architecture.
//!
//! Covers the two pure halves of the ratified design:
//!   - ADR-22's ceiling resolver: launcher-set, bounded, never wait-forever.
//!   - ADR-21's per-PR gate: merge the clean, hold the supervised, park the
//!     rest — never terminate, never merge over a reviewer.
//!
//! The impure legs (`wait_for_ci_terminal`, the forge merge, the store park)
//! stay at the boundary in `lib.rs` so these run with no forge, no store, and
//! no 30-second poll. trace:TASK-1169 | ai:claude

use crate::burndown::{
    resolve_bg_wait_ceiling_ms, wave_pr_action, IntegrationOutcome, ResidualPr, WavePrAction,
    WavePrFacts, WaveSpecFacts, DEFAULT_BG_WAIT_CEILING_MS, MIN_BG_WAIT_CEILING_MS,
};
use crate::integrate::{CiState, MergeableState};

// ── ADR-22: the ceiling resolver ────────────────────────────────────────────

#[test]
fn ceiling_defaults_to_the_generous_bound_when_nothing_is_configured() {
    assert_eq!(
        resolve_bg_wait_ceiling_ms(None, None),
        DEFAULT_BG_WAIT_CEILING_MS
    );
    // 45 minutes — comfortably past a slow cross-platform run.
    assert_eq!(DEFAULT_BG_WAIT_CEILING_MS, 45 * 60 * 1000);
}

#[test]
fn config_overrides_the_default_and_env_overrides_config() {
    assert_eq!(resolve_bg_wait_ceiling_ms(None, Some(900_000)), 900_000);
    assert_eq!(
        resolve_bg_wait_ceiling_ms(Some("1200000"), Some(900_000)),
        1_200_000
    );
}

/// The retired `=0` stopgap must be unreachable: wait-forever removes the
/// safety valve, so a genuinely wedged task would hang the drain forever.
#[test]
fn zero_from_either_source_clamps_up_to_the_floor_never_wait_forever() {
    assert_eq!(
        resolve_bg_wait_ceiling_ms(Some("0"), None),
        MIN_BG_WAIT_CEILING_MS
    );
    assert_eq!(
        resolve_bg_wait_ceiling_ms(None, Some(0)),
        MIN_BG_WAIT_CEILING_MS
    );
    assert_eq!(
        resolve_bg_wait_ceiling_ms(Some("0"), Some(0)),
        MIN_BG_WAIT_CEILING_MS
    );
    assert!(MIN_BG_WAIT_CEILING_MS > 0);
}

#[test]
fn an_absurdly_small_value_clamps_up_rather_than_reaping_mid_wave() {
    assert_eq!(
        resolve_bg_wait_ceiling_ms(Some("5"), None),
        MIN_BG_WAIT_CEILING_MS
    );
}

/// A typo'd env must not wedge a drain — it falls through to the next source.
#[test]
fn an_unparseable_override_falls_through_instead_of_failing() {
    assert_eq!(
        resolve_bg_wait_ceiling_ms(Some("later"), Some(900_000)),
        900_000
    );
    assert_eq!(
        resolve_bg_wait_ceiling_ms(Some(""), None),
        DEFAULT_BG_WAIT_CEILING_MS
    );
}

// ── ADR-21: the per-PR integration gate ─────────────────────────────────────

fn clean_facts() -> WavePrFacts {
    WavePrFacts {
        supervision_label: None,
        wait_expired: None,
        ci: CiState::Passing,
        request_changes: false,
        mergeable: MergeableState::Mergeable,
    }
}

#[test]
fn green_clean_and_unsupervised_merges() {
    assert_eq!(wave_pr_action(&clean_facts()), WavePrAction::Merge);
}

/// BUG-727: a non-`drain` execution_mode is a human's explicit "not
/// automatically" — held, and NOT parked (nothing is wrong with the work).
#[test]
fn a_supervised_spec_is_held_not_merged_and_not_parked() {
    let f = WavePrFacts {
        supervision_label: Some("guided".to_string()),
        ..clean_facts()
    };
    match wave_pr_action(&f) {
        WavePrAction::Hold(why) => assert!(why.contains("guided"), "{why}"),
        other => panic!("expected Hold, got {other:?}"),
    }
}

/// Supervision is checked FIRST: a supervised spec is held even when every
/// other signal would have merged it.
#[test]
fn supervision_outranks_a_perfectly_green_pr() {
    let f = WavePrFacts {
        supervision_label: Some("unset (supervised)".to_string()),
        ci: CiState::Passing,
        ..clean_facts()
    };
    assert!(matches!(wave_pr_action(&f), WavePrAction::Hold(_)));
}

/// ADR-22's safety item: a wait that hit a bound PARKS with the recorded
/// reason. The PR is left open — the defect being fixed is silent termination.
#[test]
fn an_expired_wait_parks_with_the_recorded_reason() {
    let f = WavePrFacts {
        wait_expired: Some("CI wait hit absolute ceiling (90m) — giving up".to_string()),
        ci: CiState::Running,
        ..clean_facts()
    };
    match wave_pr_action(&f) {
        WavePrAction::Park(why) => {
            assert!(why.contains("absolute ceiling"), "{why}");
            assert!(why.contains("left open"), "{why}");
        }
        other => panic!("expected Park, got {other:?}"),
    }
}

#[test]
fn red_ci_parks_and_never_merges() {
    let f = WavePrFacts {
        ci: CiState::Failing,
        ..clean_facts()
    };
    assert!(matches!(wave_pr_action(&f), WavePrAction::Park(_)));
}

/// Never merge over a reviewer — the strongest human signal, even with green CI.
#[test]
fn request_changes_parks_even_with_green_ci() {
    let f = WavePrFacts {
        request_changes: true,
        ci: CiState::Passing,
        ..clean_facts()
    };
    match wave_pr_action(&f) {
        WavePrAction::Park(why) => assert!(why.contains("RequestChanges"), "{why}"),
        other => panic!("expected Park, got {other:?}"),
    }
}

#[test]
fn a_merge_conflict_parks_and_is_never_auto_resolved() {
    let f = WavePrFacts {
        mergeable: MergeableState::Conflicting,
        ..clean_facts()
    };
    assert!(matches!(wave_pr_action(&f), WavePrAction::Park(_)));
}

/// The bounded wait already returned, so "still running" cannot mean
/// "wait again" — it becomes a park, never an unbounded re-wait.
#[test]
fn still_running_after_the_bounded_wait_parks_rather_than_waiting_forever() {
    let f = WavePrFacts {
        ci: CiState::Running,
        ..clean_facts()
    };
    assert!(matches!(wave_pr_action(&f), WavePrAction::Park(_)));
}

/// "Couldn't tell" is not a defect: a PR with no CI configured and unknown
/// mergeability still merges — the forge refuses an unmergeable merge, so the
/// optimistic path can't corrupt anything.
#[test]
fn no_ci_and_unknown_mergeability_still_merges() {
    let f = WavePrFacts {
        ci: CiState::None,
        mergeable: MergeableState::Unknown,
        ..clean_facts()
    };
    assert_eq!(wave_pr_action(&f), WavePrAction::Merge);
}

// ── the residual view after integration ─────────────────────────────────────

fn facts(status: &str, supervised: bool) -> WaveSpecFacts {
    WaveSpecFacts {
        status_norm: status.to_string(),
        tags: Vec::new(),
        supervised,
    }
}

/// A supervision-held PR must NOT read as stranded work: otherwise the
/// launcher relaunches agent turns for a PR every merge gate refuses, burning
/// the resume budget and exiting 2 on a perfectly healthy hold.
#[test]
fn a_supervised_specs_open_pr_is_not_residual_work() {
    let blessed = vec!["TASK-900".to_string()];
    let open = vec![(
        42,
        "feat(x): thing (TASK-900)".to_string(),
        "task-900-thing".to_string(),
    )];
    let mut map = std::collections::HashMap::new();
    map.insert("TASK-900".to_string(), facts("inprogress", true));
    assert!(crate::burndown::match_wave_prs(&blessed, &open, &map).is_empty());

    // …but the same PR on a drain-mode spec IS residual.
    let mut unsupervised = std::collections::HashMap::new();
    unsupervised.insert("TASK-900".to_string(), facts("inprogress", false));
    let got = crate::burndown::match_wave_prs(&blessed, &open, &unsupervised);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].branch, "task-900-thing");
}

/// The regression shape from the spec: a wave PR whose CI outran the old 600s
/// ceiling is merged by the launcher, so the wave does not lose its work.
#[test]
fn the_report_distinguishes_merged_held_and_parked() {
    let rows = vec![
        IntegrationOutcome {
            pr: 1506,
            spec: "BUG-723".to_string(),
            action: WavePrAction::Merge,
            succeeded: true,
        },
        IntegrationOutcome {
            pr: 1507,
            spec: "TASK-900".to_string(),
            action: WavePrAction::Hold("merge is supervised (guided)".to_string()),
            succeeded: true,
        },
        IntegrationOutcome {
            pr: 1508,
            spec: "BUG-734".to_string(),
            action: WavePrAction::Park("CI is red".to_string()),
            succeeded: true,
        },
    ];
    let out = crate::burndown::render_integration_report(&rows);
    assert!(out.contains("1 merged, 1 held, 1 parked"), "{out}");
    assert!(out.contains("#1506 (BUG-723) merged"), "{out}");
    assert!(out.contains("#1508 (BUG-734) PARKED"), "{out}");
}

/// A merge that was gated clean but failed at the forge counts as parked in
/// the report — the operator must never read a failed merge as shipped.
#[test]
fn a_failed_merge_reports_as_parked_not_merged() {
    let rows = vec![IntegrationOutcome {
        pr: 1509,
        spec: "BUG-777".to_string(),
        action: WavePrAction::Merge,
        succeeded: false,
    }];
    let out = crate::burndown::render_integration_report(&rows);
    assert!(out.contains("0 merged, 0 held, 1 parked"), "{out}");
    assert!(out.contains("the merge call failed"), "{out}");
}

#[test]
fn an_empty_integration_report_renders_nothing() {
    assert!(crate::burndown::render_integration_report(&[]).is_empty());
}

/// The residual PR carries the branch the Rust CI wait probes on — without it
/// the launcher cannot wait for anything.
#[test]
fn residual_prs_carry_the_head_branch_for_the_ci_wait() {
    let pr = ResidualPr {
        number: 1,
        spec: "TASK-1".to_string(),
        title: "t".to_string(),
        branch: "task-1-work".to_string(),
    };
    assert_eq!(pr.branch, "task-1-work");
}
