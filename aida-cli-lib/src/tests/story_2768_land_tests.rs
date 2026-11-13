// trace:STORY-2768 | ai:claude
use super::*;

/// A snapshot of every fact; each test changes only what it is about.
struct Fake {
    conflict: std::result::Result<bool, String>,
    reviewing: std::result::Result<Option<String>, String>,
    ci: std::result::Result<Ci, String>,
    approval: std::result::Result<Approval, String>,
    hold: std::result::Result<Option<Hold>, String>,
    rounds: std::result::Result<Rounds, String>,
    lane: std::result::Result<Option<String>, String>,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            conflict: Ok(false),
            reviewing: Ok(None),
            ci: Ok(Ci::Green),
            approval: Ok(Approval::Refused("no approval covers the head".into())),
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
    fn reviewing(&self, _: &Pr) -> std::result::Result<Option<String>, String> {
        self.reviewing.clone()
    }
    fn ci(&self, _: &Pr) -> std::result::Result<Ci, String> {
        self.ci.clone()
    }
    fn approval(&self, _: &Pr) -> std::result::Result<Approval, String> {
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
        approval: Ok(Approval::Approved("approved at aaaaaaaaaaaa".into())),
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
        reviewing: Ok(Some("reviewer".into())),
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
        reviewing: Ok(Some("codex reviewing".into())),
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
    assert_eq!(
        decided(held(HoldRelease::Met)),
        (State::Ready, Action::ClearHoldAndLand)
    );
    assert_eq!(
        decided(held(HoldRelease::Unmet(
            "operator-only spec always requires a human clear".into()
        ))),
        (State::Blocked, Action::AdvisorRuling)
    );
}

fn held(release: HoldRelease) -> Fake {
    Fake {
        hold: Ok(Some(Hold {
            detail: "rework hold: round 1".into(),
            release,
            since: Some("2026-11-13T08:00:00Z".into()),
        })),
        ..approved()
    }
}

#[test]
fn a_hold_awaiting_the_mandates_cover_is_not_ready() {
    let row = derive(
        &pr("main", "feature"),
        &held(HoldRelease::PendingCover("m-1".into())),
    );
    assert_eq!(
        (row.state.clone(), row.action.clone()),
        (State::ReadyPendingCover, Action::ClearHoldAndLand)
    );
    assert_eq!(row.state.label(), "READY-PENDING-COVER");
    assert_ne!(row.state, State::Ready);
    assert!(
        row.reason
            .contains("needs mandate m-1's cover, judged at release"),
        "{}",
        row.reason
    );
    // The table's summary counts it under its own label, never as READY.
    let table = render(&[row]);
    assert!(table.contains("READY-PENDING-COVER"), "{table}");
    assert!(
        !table.contains("1 READY,") && !table.contains("1 READY\n"),
        "{table}"
    );
}

#[test]
fn a_blocked_hold_carries_its_reason_and_when_it_was_placed() {
    let row = derive(
        &pr("main", "feature"),
        &held(HoldRelease::Unmet(
            "composed holds require a human clear".into(),
        )),
    );
    assert_eq!(row.state, State::Blocked);
    assert_eq!(row.since.as_deref(), Some("2026-11-13T08:00:00Z"));
    assert!(
        row.reason.contains("composed holds require a human clear"),
        "{}",
        row.reason
    );
}

#[test]
fn unreadable_approval_or_review_ownership_waits_and_launches_nothing() {
    // An unreadable verdict artifact is not "no approval": no reviewer is
    // launched and no rework dispatched, even with a refusal on record.
    let unread = Fake {
        approval: Err("unreadable verdict artifact".into()),
        rounds: blocking(1),
        ..Fake::default()
    };
    let row = derive(&pr("main", "feature"), &unread);
    assert_eq!((row.state, row.action), (State::Unknown, Action::Wait));
    assert!(row.reason.starts_with("approval unknown"), "{}", row.reason);

    // A marker that is there but unreadable is not "no reviewer": an
    // approved change does not land and an unapproved one gets no second
    // reviewer.
    for fake in [
        Fake {
            reviewing: Err("the review marker could not be read".into()),
            ..approved()
        },
        Fake {
            reviewing: Err("the review marker could not be read".into()),
            ..Fake::default()
        },
    ] {
        let row = derive(&pr("main", "feature"), &fake);
        assert_eq!((row.state, row.action), (State::Unknown, Action::Wait));
        assert!(
            row.reason.starts_with("review ownership unknown"),
            "{}",
            row.reason
        );
    }
}

#[test]
fn the_review_marker_read_tells_missing_from_unreadable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let read = |pr| crate::gate::review_marker::live_description_checked(root, pr, None);
    assert_eq!(read(7).unwrap(), None);
    // A directory where the marker file belongs cannot be read as one.
    std::fs::create_dir_all(crate::gate::review_marker::marker_path(root, 7)).unwrap();
    assert!(read(7).is_err());
}

#[test]
fn the_round_history_read_refuses_a_ledger_it_cannot_read_whole() {
    use crate::gate::ledger::{project::change_verdict_records_checked, ChangeKey};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let root = root.as_path();
    // No ledger at all is an empty history.
    assert_eq!(
        change_verdict_records_checked(root, ChangeKey::pr(7)).unwrap(),
        Vec::new()
    );
    // A ledger file that is not valid events is an error, never "no rounds".
    let file = crate::gate::ledger::store::ledger_file(root, ChangeKey::pr(7));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{ this is not an event\n{\"nor\": \"this\"}\n").unwrap();
    let refused = change_verdict_records_checked(root, ChangeKey::pr(7));
    assert!(refused.is_err(), "{refused:?}");
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
        Ci::Red("a, z".into())
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
