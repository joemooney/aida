// trace:STORY-2768 | ai:claude
use super::*;

/// A snapshot of every fact; each test changes only what it is about.
struct Fake {
    conflict: std::result::Result<bool, String>,
    reviewing: Option<String>,
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
    assert!(table.contains("add to next train"), "{table}");
    assert!(table.ends_with("2 open change(s): 2 READY\n"), "{table}");
}
