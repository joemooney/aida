// trace:STORY-2768 | ai:claude
use super::*;
use crate::land::{Pr, Row, Track};
use aida_core::models::{Requirement, RequirementStatus};

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-13T18:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn change(id: u64, spec: &str) -> Pr {
    Pr {
        id,
        head: "a".repeat(40),
        base: "main".into(),
        branch: Some(format!("branch-{id}")),
        title: format!("fix(x): change {id} ({spec})"),
        updated_at: Some("2026-11-13T17:30:00Z".into()),
        hold_label: Some(false),
        ci: None,
    }
}

fn row(pr: u64, state: State, action: Action, actor: Option<&str>) -> Row {
    Row {
        pr,
        track: Track::Main,
        state,
        action,
        since: None,
        reason: "the reason".into(),
        actor: actor.map(str::to_string),
        ci: None,
    }
}

fn spec(id: &str, status: RequirementStatus, modified: &str) -> Requirement {
    let mut requirement = Requirement::new(format!("title of {id}"), String::new());
    requirement.spec_id = Some(id.into());
    requirement.status = status;
    requirement.modified_at = DateTime::parse_from_rfc3339(modified)
        .unwrap()
        .with_timezone(&Utc);
    requirement
}

fn claim(scope: &str) -> crate::coordination::Claim {
    serde_json::from_value(serde_json::json!({
        "scope": scope,
        "node_id": "n1",
        "clone_path": "/elsewhere/aida",
        "host": "dell1",
        "pid": 1,
        "agent": "implementer",
        "started_at": "2026-11-13T17:00:00Z",
        "heartbeat_at": "2026-11-13T17:55:00Z",
        "ttl_secs": 1800
    }))
    .unwrap()
}

fn pass(changes: Vec<Pr>, rows: Vec<Row>, foreign: Vec<crate::coordination::Claim>) -> Pass {
    Pass {
        root: PathBuf::from("/nowhere"),
        changes,
        rows,
        local_lanes: Vec::new(),
        foreign_lanes: foreign,
        gitlab: false,
        merge_trees: Default::default(),
        book: Default::default(),
        notes: Vec::new(),
    }
}

fn snapshot(pass: &Pass, requirements: &[Requirement]) -> Snapshot {
    assemble(
        pass,
        requirements,
        MainBranch {
            last_merge: Some("8265057".into()),
            last_merge_at: Some("2026-11-13T16:48:00Z".into()),
            merges_today: 3,
        },
        6,
        now(),
        std::time::Duration::from_secs(40),
    )
}

#[test]
fn every_open_change_is_an_item_in_exactly_one_stage() {
    let pass = pass(
        vec![
            change(1, "BUG-1"),
            change(2, "BUG-2"),
            change(3, "BUG-3"),
            change(4, "BUG-4"),
            change(5, "BUG-5"),
            change(6, "BUG-6"),
            change(7, "BUG-7"),
        ],
        vec![
            row(1, State::NeedsReview, Action::LaunchReview, None),
            row(2, State::Reviewing, Action::Wait, Some("reviewer@home")),
            row(3, State::ChangesRequested, Action::DispatchRework, None),
            row(4, State::Ready, Action::Train, None),
            row(5, State::Landing, Action::Wait, None),
            row(6, State::Blocked, Action::AdvisorRuling, None),
            // Approved, with CI still running on the head.
            Row {
                ci: Some("pending".into()),
                ..row(7, State::Ready, Action::Wait, None)
            },
        ],
        Vec::new(),
    );
    let snapshot = snapshot(
        &pass,
        &[spec(
            "BUG-1",
            RequirementStatus::InProgress,
            "2026-11-13T17:00:00Z",
        )],
    );
    let stages: Vec<(u64, Stage)> = snapshot
        .items
        .iter()
        .map(|item| (item.pr.unwrap(), item.stage))
        .collect();
    assert_eq!(
        stages,
        vec![
            (1, Stage::Reviewing),
            (2, Stage::Reviewing),
            (3, Stage::Reworking),
            (7, Stage::WaitingCi),
            (4, Stage::Ready),
            (5, Stage::Shipping),
            (6, Stage::Blocked),
        ]
    );
    assert_eq!(snapshot.items[0].title, "title of BUG-1");
    assert_eq!(snapshot.items[0].actor, "none");
    assert_eq!(snapshot.items[1].actor, "reviewer@home");
    assert_eq!(snapshot.queues.awaiting_review, 1);
    assert_eq!(snapshot.queues.awaiting_rework, 1);
    assert_eq!(snapshot.open_prs, 7);
}

#[test]
fn a_live_lane_without_a_change_is_implementing_and_one_with_a_change_is_not_listed_twice() {
    let pass = pass(
        vec![change(1, "BUG-1")],
        vec![row(
            1,
            State::CiFailed,
            Action::Wait,
            Some("implementer@dell1"),
        )],
        vec![claim("BUG-1"), claim("STORY-9"), claim("PR-77")],
    );
    let snapshot = snapshot(
        &pass,
        &[
            spec(
                "STORY-9",
                RequirementStatus::InProgress,
                "2026-11-13T10:00:00Z",
            ),
            spec(
                "TASK-3",
                RequirementStatus::Approved,
                "2026-11-13T10:00:00Z",
            ),
        ],
    );
    let lanes: Vec<(&str, Stage, &str)> = snapshot
        .items
        .iter()
        .map(|item| (item.spec.as_str(), item.stage, item.actor.as_str()))
        .collect();
    assert_eq!(
        lanes,
        vec![
            ("STORY-9", Stage::Implementing, "implementer@dell1"),
            ("BUG-1", Stage::Reworking, "implementer@dell1"),
        ]
    );
    assert_eq!(snapshot.queues.approved_not_started, 1);
    assert!(snapshot.attention.is_empty(), "{:?}", snapshot.attention);
}

#[test]
fn attention_names_a_reason_a_next_action_and_an_owner_for_every_entry() {
    let mut stale = change(2, "BUG-2");
    stale.updated_at = Some("2026-11-13T12:00:00Z".into());
    let pass = pass(
        vec![change(1, "BUG-1"), stale, change(3, "BUG-3")],
        vec![
            row(1, State::Blocked, Action::AdvisorRuling, None),
            row(2, State::NeedsReview, Action::LaunchReview, None),
            row(3, State::NeedsReview, Action::LaunchReview, None),
        ],
        Vec::new(),
    );
    let snapshot = snapshot(
        &pass,
        &[
            spec(
                "TASK-7",
                RequirementStatus::NeedsAttention,
                "2026-11-11T18:00:00Z",
            ),
            spec(
                "TASK-8",
                RequirementStatus::InProgress,
                "2026-11-13T09:00:00Z",
            ),
            spec(
                "TASK-9",
                RequirementStatus::InProgress,
                "2026-11-13T17:30:00Z",
            ),
        ],
    );
    let entries: Vec<(&str, &str)> = snapshot
        .attention
        .iter()
        .map(|entry| (entry.spec.as_str(), entry.owner.as_str()))
        .collect();
    assert_eq!(
        entries,
        vec![
            ("TASK-7", "advisor"),
            ("TASK-8", "reconciler"),
            ("BUG-2 (#2)", "reconciler"),
            ("BUG-1 (#1)", "advisor"),
        ]
    );
    for entry in &snapshot.attention {
        assert!(!entry.why.is_empty() && !entry.next_action.is_empty());
    }
    assert!(snapshot.attention[0].why.contains("unowned"));
    assert!(snapshot.attention[3].why.contains("the reason"));
}

#[test]
fn the_screen_shows_the_header_stages_queues_and_attention() {
    let pass = pass(
        vec![change(1, "BUG-1")],
        vec![row(1, State::Ready, Action::ClearHoldAndLand, None)],
        Vec::new(),
    );
    let snapshot = snapshot(&pass, &[]);
    colored::control::set_override(false);
    let screen = render(&snapshot, now());
    assert!(
        screen.starts_with(
            "NOW snapshot 0s old · main last merge 8265057 1h12m ago · 3 today · 1 open PRs (WIP limit 6)\n"
        ),
        "{screen}"
    );
    assert!(screen.contains("\nREADY (1)\n"), "{screen}");
    assert!(
        screen.contains("release the met hold, then land (reconciler)"),
        "{screen}"
    );
    assert!(
        screen.contains("QUEUES approved, not started 0"),
        "{screen}"
    );
    assert!(
        screen.contains("ATTENTION (0)\n  nothing needs you"),
        "{screen}"
    );
    assert!(!screen.contains("not running"), "{screen}");
    let later = render(&snapshot, now() + chrono::Duration::minutes(11));
    assert!(
        later.contains("the landing reconciler is not running"),
        "{later}"
    );
}

#[test]
fn the_snapshot_round_trips_and_the_primary_checkout_is_found_from_files() {
    let dir = tempfile::tempdir().unwrap();
    let primary = dir.path().join("primary");
    let linked = dir.path().join("linked").join("deep");
    std::fs::create_dir_all(primary.join(".git").join("worktrees").join("w")).unwrap();
    std::fs::create_dir_all(&linked).unwrap();
    std::fs::write(
        dir.path().join("linked").join(".git"),
        format!("gitdir: {}/.git/worktrees/w\n", primary.display()),
    )
    .unwrap();
    assert_eq!(primary_root(&linked), primary);
    assert_eq!(primary_root(&primary), primary);

    let pass = pass(Vec::new(), Vec::new(), Vec::new());
    let written = snapshot(&pass, &[]);
    write(&primary, &written).unwrap();
    let (_, read_back) = read(&primary).unwrap();
    assert_eq!(read_back, written);
    assert!(read(&linked).is_err());
}
