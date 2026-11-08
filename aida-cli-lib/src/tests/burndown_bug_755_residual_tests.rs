// trace:TASK-3424 | ai:codex
use super::*;
use std::collections::HashMap;

// BUG-1163: a supervised (drive/guided/operator/decide/unresolved) wave PR
// must HOLD even when reviewer-approved and CI-green — the exact invariant
// that leaked (BUG-1156/STORY-1130/TASK-1233 merged despite the hold while
// STORY-1131, same mode, correctly held). trace:BUG-1163 | ai:claude
#[test]
fn wave_pr_action_holds_supervised_even_when_approved_and_green() {
    let action = wave_pr_action(&WavePrFacts {
        supervision_label: Some("drive".to_string()),
        wait_expired: None,
        ci: crate::integrate::CiState::Passing,
        request_changes: false,
        mergeable: crate::integrate::MergeableState::Mergeable,
    });
    assert!(
        matches!(action, WavePrAction::Hold(_)),
        "supervised + approved + green must Hold, never Merge; got {action:?}"
    );
}

fn facts(status: &str, tags: &[&str]) -> WaveSpecFacts {
    WaveSpecFacts {
        status_norm: status.to_string(),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        // TASK-1169: these BUG-755 cases predate the supervised gate;
        // drain-mode (unsupervised) keeps their expectations unchanged.
        supervised: false,
    }
}

#[test]
fn spec_token_matches_branch_and_title_case_insensitive() {
    assert!(contains_spec_token("bug-723-fix-the-thing", "BUG-723"));
    assert!(contains_spec_token(
        "[AI:claude] fix(x): thing (BUG-723)",
        "BUG-723"
    ));
    assert!(!contains_spec_token("task-1117-other", "BUG-723"));
}

#[test]
fn spec_token_digit_boundary_prevents_prefix_collisions() {
    // TASK-11 must NOT match a task-1117 branch…
    assert!(!contains_spec_token("task-1117-something", "TASK-11"));
    // …but still matches its own branch.
    assert!(contains_spec_token("task-11-something", "TASK-11"));
}

#[test]
fn match_wave_prs_picks_only_blessed_and_sorts_by_number() {
    let blessed = vec!["BUG-723".to_string(), "TASK-1117".to_string()];
    let open = vec![
        (
            1508,
            "feat(x): thing (TASK-1117)".to_string(),
            "task-1117-thing".to_string(),
        ),
        (
            1506,
            "fix(y): other (BUG-723)".to_string(),
            "bug-723-other".to_string(),
        ),
        (
            1500,
            "unrelated PR".to_string(),
            "story-1-unrelated".to_string(),
        ),
    ];
    let got = match_wave_prs(&blessed, &open, &HashMap::new());
    let ids: Vec<(u64, &str)> = got.iter().map(|p| (p.number, p.spec.as_str())).collect();
    assert_eq!(ids, vec![(1506, "BUG-723"), (1508, "TASK-1117")]);
}

#[test]
fn match_wave_prs_excludes_deliberately_held_or_parked_specs() {
    let blessed = vec![
        "BUG-1".to_string(),
        "BUG-2".to_string(),
        "BUG-3".to_string(),
    ];
    let open = vec![
        (1, "a (BUG-1)".to_string(), "bug-1-a".to_string()),
        (2, "b (BUG-2)".to_string(), "bug-2-b".to_string()),
        (3, "c (BUG-3)".to_string(), "bug-3-c".to_string()),
    ];
    let mut f = HashMap::new();
    // Held draft for the operator — a legitimate early exit, not stranded.
    f.insert("BUG-1".to_string(), facts("done", &["review:draft-only"]));
    // Shelved NeedsAttention — parked by the drain, also not stranded.
    f.insert("BUG-2".to_string(), facts("needsattention", &[]));
    // No facts for BUG-3 → conservative: keep it visible.
    let got = match_wave_prs(&blessed, &open, &f);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].spec, "BUG-3");
}

#[test]
fn followup_complete_when_no_residual_regardless_of_code() {
    assert_eq!(drain_followup(0, true, 0, 3), DrainFollowup::Complete);
    assert_eq!(drain_followup(2, true, 0, 3), DrainFollowup::Complete);
}

#[test]
fn followup_resume_on_clean_exit_with_residual_and_budget() {
    // The BUG-755 failure mode: session exited 0 while PRs sat open and
    // blessed specs were unstarted — the launcher must NOT exit.
    assert_eq!(drain_followup(0, false, 0, 3), DrainFollowup::Resume);
    assert_eq!(drain_followup(0, false, 2, 3), DrainFollowup::Resume);
}

#[test]
fn followup_handoff_when_budget_spent_or_deliberate_stop() {
    // Resume budget spent → accurate handoff, not an infinite relaunch loop.
    assert_eq!(drain_followup(0, false, 3, 3), DrainFollowup::Handoff);
    // Non-zero exit = the shelve/cap stop convention — honored, never
    // relaunched, but the residual still gets an accurate handoff.
    assert_eq!(drain_followup(2, false, 0, 3), DrainFollowup::Handoff);
}

#[test]
fn two_wave_regression_resume_then_complete() {
    // Regression shape from the spec: an 8-spec plan whose session exits
    // after wave 1 with 3 open PRs + 4 unstarted blessed specs. Wave-1 PRs
    // must merge and wave 2 must start before the launcher may exit.
    let after_wave_1 = ResidualWork {
        unmerged_prs: vec![ResidualPr {
            number: 1506,
            spec: "BUG-723".to_string(),
            title: "fix".to_string(),
            branch: "bug-723-fix".to_string(),
        }],
        unstarted: vec!["TASK-1098".to_string(), "TASK-1146".to_string()],
    };
    // Turn 1 ended cleanly but left residual → the launcher relaunches.
    assert_eq!(
        drain_followup(0, after_wave_1.is_empty(), 0, 3),
        DrainFollowup::Resume
    );
    // The continuation prompt drives the wave-1 merges AND wave 2.
    let prompt = continuation_prompt("/aida-burndown --status approved", &after_wave_1);
    assert!(prompt.contains("#1506"), "prompt: {prompt}");
    assert!(prompt.contains("BUG-723"), "prompt: {prompt}");
    assert!(prompt.contains("TASK-1098"), "prompt: {prompt}");
    assert!(prompt.contains("FOREGROUND"), "prompt: {prompt}");
    assert!(
        prompt.starts_with("/aida-burndown --status approved"),
        "the skill's gates must re-apply on resume"
    );
    // After the resume turn merges wave 1 and drains wave 2: no residual →
    // the launcher may finally exit.
    assert_eq!(
        drain_followup(0, ResidualWork::default().is_empty(), 1, 3),
        DrainFollowup::Complete
    );
}

#[test]
fn handoff_report_names_prs_specs_and_manual_verbs_no_watch_claims() {
    let residual = ResidualWork {
        unmerged_prs: vec![ResidualPr {
            number: 1507,
            spec: "BUG-734".to_string(),
            title: "fix(z)".to_string(),
            branch: "bug-734-fix".to_string(),
        }],
        unstarted: vec!["TASK-937".to_string()],
    };
    let out = render_drain_handoff(&residual);
    assert!(out.contains("#1507"), "out: {out}");
    assert!(out.contains("BUG-734"), "out: {out}");
    assert!(out.contains("TASK-937"), "out: {out}");
    assert!(out.contains("aida integrate"), "out: {out}");
    assert!(out.contains("no watches are running"), "out: {out}");
}
