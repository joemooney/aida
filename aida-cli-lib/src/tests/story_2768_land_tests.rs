// trace:STORY-2768 | ai:claude
use super::*;

/// A snapshot of every fact; each test changes only what it is about.
struct Fake {
    conflict: std::result::Result<bool, String>,
    reviewing: Option<String>,
    ci: std::result::Result<Ci, String>,
    approval: std::result::Result<String, String>,
    hold: std::result::Result<Option<Hold>, String>,
    rounds: std::result::Result<Rounds, String>,
    lane: std::result::Result<Option<String>, String>,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            conflict: Ok(false),
            reviewing: None,
            ci: Ok(Ci::Green),
            approval: Err("no approval covers the head".into()),
            hold: Ok(None),
            rounds: Ok(Rounds::default()),
            lane: Ok(None),
        }
    }
}

impl Evidence for Fake {
    fn conflict(&self, _: &Pr) -> std::result::Result<bool, String> {
        self.conflict.clone()
    }
    fn reviewing(&self, _: &Pr) -> Option<String> {
        self.reviewing.clone()
    }
    fn ci(&self, _: &Pr) -> std::result::Result<Ci, String> {
        self.ci.clone()
    }
    fn approval(&self, _: &Pr) -> std::result::Result<String, String> {
        self.approval.clone()
    }
    fn hold(&self, _: &Pr) -> std::result::Result<Option<Hold>, String> {
        self.hold.clone()
    }
    fn rounds(&self, _: &Pr) -> std::result::Result<Rounds, String> {
        self.rounds.clone()
    }
    fn lane(&self, _: &Pr) -> std::result::Result<Option<String>, String> {
        self.lane.clone()
    }
}

fn pr(base: &str, branch: &str) -> Pr {
    Pr {
        id: 7,
        head: "a".repeat(40),
        base: base.into(),
        branch: Some(branch.into()),
        title: "feat(x): a change (STORY-1)".into(),
        updated_at: None,
        hold_label: None,
        ci: None,
    }
}

fn decided(fake: Fake) -> (State, Action) {
    let row = derive(&pr("main", "feature"), &fake);
    (row.state, row.action)
}

fn approved() -> Fake {
    Fake {
        approval: Ok("approved at aaaaaaaaaaaa".into()),
        ..Fake::default()
    }
}

fn blocking(rounds: usize) -> std::result::Result<Rounds, String> {
    Ok(Rounds {
        at_content: true,
        content_rounds: rounds,
        since: Some("2026-11-13T09:00:00Z".into()),
    })
}

#[test]
fn an_unreviewed_change_needs_a_review() {
    assert_eq!(
        decided(Fake::default()),
        (State::NeedsReview, Action::LaunchReview)
    );
}

#[test]
fn a_conflict_wins_over_every_later_fact_and_dispatches_rework() {
    let fake = Fake {
        conflict: Ok(true),
        reviewing: Some("reviewer".into()),
        ..approved()
    };
    assert_eq!(decided(fake), (State::Conflict, Action::DispatchRework));
}

#[test]
fn rework_is_not_dispatched_twice_while_a_lane_is_live() {
    let fake = Fake {
        conflict: Ok(true),
        lane: Ok(Some("lease-1".into())),
        ..Fake::default()
    };
    assert_eq!(decided(fake), (State::Conflict, Action::Wait));
    let fake = Fake {
        rounds: blocking(1),
        lane: Ok(Some("lease-1".into())),
        ..Fake::default()
    };
    assert_eq!(decided(fake), (State::ChangesRequested, Action::Wait));
}

#[test]
fn a_live_review_waits_even_when_an_approval_exists() {
    let fake = Fake {
        reviewing: Some("codex reviewing".into()),
        ..approved()
    };
    assert_eq!(decided(fake), (State::Reviewing, Action::Wait));
}

#[test]
fn an_approved_change_is_ready_and_lands_by_its_track() {
    assert_eq!(decided(approved()), (State::Ready, Action::Train));
    let row = derive(&pr("integration", "feature"), &approved());
    assert_eq!(
        (row.track, row.state, row.action),
        (
            Track::Integration,
            State::Ready,
            Action::ReportToIntegration
        )
    );
}

#[test]
fn an_approval_wins_over_older_requested_changes() {
    let fake = Fake {
        rounds: blocking(2),
        ..approved()
    };
    assert_eq!(decided(fake), (State::Ready, Action::Train));
}

#[test]
fn a_hold_is_judged_by_its_release_condition() {
    let held = |released_by_approval| Fake {
        hold: Ok(Some(Hold {
            detail: "rework hold: round 1".into(),
            released_by_approval,
        })),
        ..approved()
    };
    assert_eq!(
        decided(held(true)),
        (State::Ready, Action::ClearHoldAndLand)
    );
    assert_eq!(
        decided(held(false)),
        (State::Blocked, Action::AdvisorRuling)
    );
}

#[test]
fn requested_changes_at_the_content_dispatch_rework_until_the_round_cap() {
    let fake = |rounds| Fake {
        rounds: blocking(rounds),
        ..Fake::default()
    };
    assert_eq!(
        decided(fake(2)),
        (State::ChangesRequested, Action::DispatchRework)
    );
    assert_eq!(decided(fake(3)), (State::Blocked, Action::AdvisorRuling));
}

#[test]
fn requested_changes_at_older_content_only_need_a_new_review() {
    let fake = Fake {
        rounds: Ok(Rounds {
            at_content: false,
            content_rounds: 1,
            since: None,
        }),
        ..Fake::default()
    };
    assert_eq!(decided(fake), (State::NeedsReview, Action::LaunchReview));
}

#[test]
fn red_ci_sends_the_head_to_rework_before_any_review_or_landing() {
    let red = || Ok::<Ci, String>(Ci::Red("build".into()));
    let fake = Fake {
        ci: red(),
        ..Fake::default()
    };
    assert_eq!(decided(fake), (State::CiFailed, Action::DispatchRework));
    let fake = Fake {
        ci: red(),
        ..approved()
    };
    assert_eq!(decided(fake), (State::CiFailed, Action::DispatchRework));
    let fake = Fake {
        ci: red(),
        lane: Ok(Some("implementer@dell1".into())),
        ..Fake::default()
    };
    let row = derive(&pr("main", "feature"), &fake);
    assert_eq!((row.state, row.action), (State::CiFailed, Action::Wait));
    assert_eq!(row.actor.as_deref(), Some("implementer@dell1"));
}

#[test]
fn running_ci_gates_the_landing_and_never_the_review() {
    for ci in [Ci::Pending, Ci::None, Ci::Green] {
        let fake = Fake {
            ci: Ok(ci.clone()),
            ..Fake::default()
        };
        assert_eq!(decided(fake), (State::NeedsReview, Action::LaunchReview));
        let fake = Fake {
            ci: Ok(ci),
            ..approved()
        };
        assert_eq!(decided(fake), (State::Ready, Action::Train));
    }
    let fake = Fake {
        ci: Ok(Ci::Pending),
        ..approved()
    };
    assert_eq!(
        derive(&pr("main", "feature"), &fake).ci.as_deref(),
        Some("pending")
    );
}

#[test]
fn a_met_hold_is_the_operators_to_clear() {
    assert_eq!(Action::ClearHoldAndLand.owner(), "operator");
    assert!(Action::ClearHoldAndLand.label().starts_with("operator"));
}

#[test]
fn a_check_rollup_sums_to_one_ci_state() {
    let ci = |json: &str| ci_of_rollup(&serde_json::from_str(json).unwrap());
    assert_eq!(ci("[]"), Ci::None);
    assert_eq!(ci("null"), Ci::None);
    assert_eq!(
        ci(
            r#"[{"name":"a","status":"COMPLETED","conclusion":"SUCCESS"},{"context":"b","state":"SUCCESS"},{"name":"c","status":"COMPLETED","conclusion":"SKIPPED"}]"#
        ),
        Ci::Green
    );
    assert_eq!(
        ci(
            r#"[{"name":"a","status":"COMPLETED","conclusion":"SUCCESS"},{"name":"b","status":"IN_PROGRESS","conclusion":""}]"#
        ),
        Ci::Pending
    );
    assert_eq!(
        ci(
            r#"[{"name":"b","status":"IN_PROGRESS","conclusion":""},{"name":"z","status":"COMPLETED","conclusion":"FAILURE"},{"name":"a","status":"COMPLETED","conclusion":"CANCELLED"}]"#
        ),
        Ci::Red("z".into())
    );
}

#[test]
fn a_cancelled_run_is_not_a_failure_and_a_later_run_replaces_it() {
    let ci = |json: &str| ci_of_rollup(&serde_json::from_str(json).unwrap());
    // Cancelled and never rerun: the head was not tested.
    assert_eq!(
        ci(
            r#"[{"name":"a","workflowName":"CI","conclusion":"CANCELLED","startedAt":"2026-11-13T10:00:00Z"}]"#
        ),
        Ci::Pending
    );
    // The same check ran again later at this head: only that run counts.
    assert_eq!(
        ci(
            r#"[{"name":"a","workflowName":"CI","conclusion":"SUCCESS","startedAt":"2026-11-13T11:00:00Z"},{"name":"a","workflowName":"CI","conclusion":"CANCELLED","startedAt":"2026-11-13T10:00:00Z"}]"#
        ),
        Ci::Green
    );
    assert_eq!(
        ci(
            r#"[{"name":"a","workflowName":"CI","conclusion":"FAILURE","startedAt":"2026-11-13T10:00:00Z"},{"name":"a","workflowName":"CI","conclusion":"SUCCESS","startedAt":"2026-11-13T11:00:00Z"}]"#
        ),
        Ci::Green
    );
    // A later failure is a failure, whatever ran before it.
    assert_eq!(
        ci(
            r#"[{"name":"a","workflowName":"CI","conclusion":"SUCCESS","startedAt":"2026-11-13T10:00:00Z"},{"name":"a","workflowName":"CI","conclusion":"FAILURE","startedAt":"2026-11-13T11:00:00Z"}]"#
        ),
        Ci::Red("a".into())
    );
    // The same name in another workflow is another check.
    assert_eq!(
        ci(
            r#"[{"name":"a","workflowName":"CI","conclusion":"SUCCESS","startedAt":"2026-11-13T11:00:00Z"},{"name":"a","workflowName":"Docs","conclusion":"FAILURE","startedAt":"2026-11-13T10:00:00Z"}]"#
        ),
        Ci::Red("a".into())
    );
}

#[test]
fn a_change_from_a_forge_without_a_check_rollup_has_no_checks_and_keeps_its_title() {
    let change = crate::review_watch::forge::Change {
        id: 7,
        head: "a".repeat(40),
        base: "main".into(),
        author: "someone".into(),
        gitlab: true,
        overlap: None,
        behind: None,
        head_branch: Some("feature".into()),
    };
    let pr = plain_change(change, "feat(x): a change (STORY-1)".into());
    // "No check has reported", not "unknown": the change is derived.
    assert_eq!(pr.ci, Some(Ci::None));
    assert_eq!(specs_of(&pr), ["STORY-1"]);
    let fake = Fake {
        ci: Ok(pr.ci.clone().unwrap()),
        ..Fake::default()
    };
    let row = derive(&pr, &fake);
    assert_eq!(
        (row.state, row.action),
        (State::NeedsReview, Action::LaunchReview)
    );
}

#[test]
fn a_claim_covers_a_change_by_spec_or_branch() {
    let change = Pr {
        title: "[AI:codex] fix(x): y (Refs BUG-2546)".into(),
        ..pr("main", "bug-b6b1278890774ef993d2ebc327862470-311")
    };
    assert_eq!(specs_of(&change), vec!["BUG-2546".to_string()]);
    assert!(claim_covers("BUG-2546", &change));
    assert!(claim_covers(
        "BUG-b6b1278890774ef993d2ebc327862470-311",
        &change
    ));
    assert!(!claim_covers("BUG-2547", &change));
    assert!(!claim_covers("", &change));
}

#[test]
fn unreadable_evidence_is_unknown_and_never_acts() {
    for fake in [
        Fake {
            conflict: Err("fetch failed".into()),
            ..approved()
        },
        Fake {
            hold: Err("label unreadable".into()),
            ..approved()
        },
        Fake {
            ci: Err("checks unreadable".into()),
            ..approved()
        },
        Fake {
            rounds: Err("ledger unreadable".into()),
            ..Fake::default()
        },
        Fake {
            conflict: Ok(true),
            lane: Err("leases unreadable".into()),
            ..Fake::default()
        },
    ] {
        assert_eq!(decided(fake), (State::Unknown, Action::Wait));
    }
}

#[test]
fn trains_and_the_integration_branch_are_the_landing_itself() {
    let row = derive(&pr("main", "train/abc-1"), &Fake::default());
    assert_eq!((row.state, row.action), (State::Landing, Action::Wait));
    let row = derive(&pr("main", "integration"), &Fake::default());
    assert_eq!((row.state, row.action), (State::Landing, Action::Wait));
}

#[test]
fn the_table_lists_each_change_once_with_a_state_summary() {
    let changes = [
        Pr {
            id: 9,
            ..pr("main", "b")
        },
        Pr {
            id: 3,
            ..pr("integration", "a")
        },
    ];
    let rows = pass(&changes, &approved());
    assert_eq!(rows.iter().map(|r| r.pr).collect::<Vec<_>>(), vec![3, 9]);
    let table = render(&rows);
    assert!(table.contains("report to integration seat"), "{table}");
    assert_eq!(rows[0].action.owner(), "integration seat");
    assert!(table.contains("add to next train"), "{table}");
    assert!(table.ends_with("2 open change(s): 2 READY\n"), "{table}");
}
