//! STORY-2243: `aida goal advisor set | show | check | history` and the
//! surfaces that carry the advisor's standing goal.
// trace:STORY-2243 | ai:claude

use super::*;
use crate::forge::ForgeKind;
use crate::goal_cmd::{build_goal_clauses_for_forge, AdvisorGoalAction, GoalArgs, GoalCommand};

fn clauses() -> Vec<GoalClauseRecord> {
    build_goal_clauses_for_forge(
        None,
        Some("EPIC-612"),
        None,
        None,
        Some("implementer"),
        ForgeKind::GitHub,
    )
    .unwrap()
    .iter()
    .map(crate::goal_cmd::GoalClause::to_record)
    .collect()
}

fn goal(text: &str, clauses: Vec<GoalClauseRecord>) -> AdvisorGoal {
    AdvisorGoal {
        text: text.to_string(),
        clauses,
        set_by: "joe".into(),
        set_at: "2026-11-01T04:45:00Z".parse().unwrap(),
        scope: "main".into(),
    }
}

/// A store directory holding `goals` (oldest first) without git.
fn store_with(goals: Vec<AdvisorGoal>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(advisor_goal::ADVISOR_GOAL_REL);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let log = advisor_goal::AdvisorGoalLog { goals };
    std::fs::write(&path, serde_yaml::to_string(&log).unwrap()).unwrap();
    dir
}

// AC1: show prints the text, both clauses with their verify commands, the
// setter and the timestamp, in human, toon and json.
// trace:STORY-b6b1278890774ef993d2ebc327862470-272.ac4bdd03 | ai:claude
#[test]
fn show_prints_text_clauses_setter_and_time_in_every_format() {
    let g = goal("X", clauses());
    assert_eq!(g.clauses[0].kind, ClauseKind::Epic);
    assert_eq!(g.clauses[1].kind, ClauseKind::QueueEmpty);
    assert_eq!(g.clauses[1].arg, "implementer");
    let verify_epic = "`aida list --parent EPIC-612` returns no rows";
    let verify_queue = "`aida queue list --for implementer --no-in-flight` shows no queued items";

    let human = render_goal(Some(&g), Fmt::Human);
    assert!(human.contains("  X\n"), "{human}");
    assert!(
        human.contains("set by joe at 2026-11-01 04:45 UTC"),
        "{human}"
    );
    assert!(human.contains(verify_epic), "{human}");
    assert!(human.contains(verify_queue), "{human}");

    let toon = render_goal(Some(&g), Fmt::Toon);
    assert!(
        toon.starts_with("goal: X\nset_by: joe\nset_at: 2026-11-01T04:45:00"),
        "{toon}"
    );
    assert!(toon.contains("clauses[2]:"), "{toon}");
    assert!(
        toon.contains(verify_epic) && toon.contains(verify_queue),
        "{toon}"
    );

    let json: serde_json::Value = serde_json::from_str(&render_goal(Some(&g), Fmt::Json)).unwrap();
    assert_eq!(json["goal"]["text"], "X");
    assert_eq!(json["goal"]["set_by"], "joe");
    assert_eq!(json["goal"]["set_at"], "2026-11-01T04:45:00Z");
    assert_eq!(json["goal"]["scope"], "main");
    assert_eq!(json["goal"]["clauses"][0]["verify"], verify_epic);
    assert_eq!(json["goal"]["clauses"][1]["verify"], verify_queue);
}

#[test]
fn show_without_a_goal_says_how_to_set_one() {
    assert!(render_goal(None, Fmt::Human).contains("aida goal advisor set"));
    assert_eq!(render_goal(None, Fmt::Toon), "goal: none\n");
    let json: serde_json::Value = serde_json::from_str(&render_goal(None, Fmt::Json)).unwrap();
    assert!(json["goal"].is_null());
}

// AC4: one met and one unmet clause → not met, the unmet one named; every
// clause met → met.
// trace:STORY-b6b1278890774ef993d2ebc327862470-272.ac73038d | ai:claude
#[test]
fn check_names_each_unmet_clause_and_is_met_only_when_all_are() {
    let g = goal("X", clauses());
    let report = check_goal(&g, |c| match c.kind {
        ClauseKind::Epic => Ok(()),
        _ => Err("2 queued for implementer: TASK-1, TASK-2".into()),
    });
    assert!(!report.met());
    assert!(report.clauses[0].met);
    assert!(!report.clauses[1].met);
    let out = render_check(&report, Fmt::Human);
    assert!(
        out.contains("  met   all direct children of EPIC-612"),
        "{out}"
    );
    assert!(
        out.contains("  UNMET the implementer queue is empty"),
        "{out}"
    );
    assert!(
        out.contains("2 queued for implementer: TASK-1, TASK-2"),
        "{out}"
    );
    assert!(out.contains("1 of 2 clauses unmet."), "{out}");
    let json: serde_json::Value = serde_json::from_str(&render_check(&report, Fmt::Json)).unwrap();
    assert_eq!(json["met"], false);
    assert_eq!(json["clauses"][1]["met"], false);

    let all = check_goal(&g, |_| Ok(()));
    assert!(all.met());
    assert!(render_check(&all, Fmt::Human).contains("All clauses met."));
}

// AC4: a free-text-only goal has no checkable clauses and is never met.
// trace:STORY-b6b1278890774ef993d2ebc327862470-272.ac73038d | ai:claude
#[test]
fn free_text_goal_reports_no_checkable_clauses_and_is_never_met() {
    let g = goal("get the open list to zero", Vec::new());
    let report = check_goal(&g, |_| panic!("no clause to evaluate"));
    assert!(!report.met());
    let out = render_check(&report, Fmt::Human);
    assert!(out.contains("no checkable clauses"), "{out}");
    let json: serde_json::Value = serde_json::from_str(&render_check(&report, Fmt::Json)).unwrap();
    assert_eq!(json["checkable"], false);
    assert_eq!(json["met"], false);
}

// AC5: history lists every goal, newest first, the newest marked active.
// trace:STORY-b6b1278890774ef993d2ebc327862470-272.ac1f6c1e | ai:claude
#[test]
fn history_lists_goals_newest_first() {
    let mut first = goal("first", Vec::new());
    first.set_at = "2026-11-01T01:00:00Z".parse().unwrap();
    let second = goal("second", clauses());
    let dir = store_with(vec![first, second]);
    let log = advisor_goal::load(dir.path()).unwrap();
    assert_eq!(log.active().unwrap().text, "second");
    let goals: Vec<&AdvisorGoal> = log.history().collect();
    let human = render_history(&goals, Fmt::Human);
    let lines: Vec<&str> = human.lines().filter(|l| !l.starts_with(' ')).collect();
    assert!(
        lines[0].contains("active") && lines[0].ends_with("second"),
        "{human}"
    );
    assert!(
        lines[1].contains("replaced") && lines[1].ends_with("first"),
        "{human}"
    );
    let json: serde_json::Value = serde_json::from_str(&render_history(&goals, Fmt::Json)).unwrap();
    assert_eq!(json["goals"][0]["text"], "second");
    assert_eq!(json["goals"][1]["text"], "first");
}

fn tty() -> SetterFacts {
    SetterFacts {
        stdin_tty: true,
        stderr_tty: true,
        markers: Vec::new(),
        under_harness: false,
    }
}

// AC6 (+ advisor disposition): `set` is refused from a non-TTY session, from
// any session carrying an agent marker or a seat grant, and from under an
// agent harness even with a PTY. The message says the operator sets it.
// trace:STORY-b6b1278890774ef993d2ebc327862470-272.acb9b660 | ai:claude
#[test]
fn set_is_operator_only() {
    assert_eq!(set_refusal(&tty()), None, "a direct operator may set");

    let no_tty = SetterFacts {
        stdin_tty: false,
        ..tty()
    };
    let why = set_refusal(&no_tty).expect("non-TTY refused");
    assert!(
        why.contains("only the operator sets the advisor's goal"),
        "{why}"
    );

    for marker in AGENT_SESSION_MARKERS {
        let facts = SetterFacts {
            markers: vec![marker.to_string()],
            ..tty()
        };
        let why = set_refusal(&facts).unwrap_or_else(|| panic!("{marker} refused"));
        assert!(why.contains(marker) && why.contains("operator"), "{why}");
    }
    assert!(AGENT_SESSION_MARKERS.contains(&"AIDA_SESSION_GRANT"));

    let harness = SetterFacts {
        under_harness: true,
        ..tty()
    };
    assert!(set_refusal(&harness).unwrap().contains("agent harness"));
}

// AC6: from an agent environment the observed facts refuse `set`.
// trace:STORY-b6b1278890774ef993d2ebc327862470-272.acb9b660 | ai:claude
#[test]
fn observed_agent_session_is_refused() {
    let _env = crate::test_env::EnvVarsGuard::set(&[("AIDA_AGENT_NAME", "claude-advisor-11")]);
    let facts = SetterFacts::observed();
    assert!(facts.markers.contains(&"AIDA_AGENT_NAME".to_string()));
    assert!(set_refusal(&facts).is_some());
}

// AC3 + AC9: the launch context carries a `## Goal` section in the advisor
// seat only, and only when a goal is set. Nothing depends on a vendor.
// trace:STORY-b6b1278890774ef993d2ebc327862470-272.acd25002 | ai:claude
#[test]
fn launch_context_goal_section_is_advisor_only() {
    let dir = store_with(vec![goal("X", clauses())]);
    let section = render_launch_goal_section(dir.path(), "advisor").expect("advisor sees it");
    assert!(section.starts_with("## Goal\n"), "{section}");
    assert!(section.contains("  X\n") && section.contains("aida list --parent EPIC-612"));
    assert!(section.contains("aida goal advisor check"));
    for role in ["implementer", "reviewer", "product", "unspecified"] {
        assert_eq!(render_launch_goal_section(dir.path(), role), None, "{role}");
    }
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(render_launch_goal_section(empty.path(), "advisor"), None);
}

// AC3: `aida session handoff --seat advisor --show` carries the goal after
// the mandate, before the standing orders and the handoff.
#[test]
fn handoff_show_carries_the_goal_for_the_advisor_seat() {
    let dir = store_with(vec![goal("X", Vec::new())]);
    assert_eq!(handoff_goal_block(dir.path(), "implementer"), None);
    let block = handoff_goal_block(dir.path(), "advisor").unwrap();
    assert!(block.starts_with("ADVISOR GOAL\n"), "{block}");
    let out = crate::seat_rotation::compose_handoff_show_with_goal(
        Some("MANDATE ship-held"),
        Some(&block),
        Some("STANDING ORDERS (x)\nobey"),
        Some("handoff body\n"),
        "advisor",
    );
    let at = |needle: &str| {
        out.find(needle)
            .unwrap_or_else(|| panic!("{needle}: {out}"))
    };
    assert!(at("ACTIVE MANDATE") < at("ADVISOR GOAL"));
    assert!(at("ADVISOR GOAL") < at("STANDING ORDERS"));
    assert!(at("STANDING ORDERS") < at("handoff body"));
    assert_eq!(
        crate::seat_rotation::compose_handoff_show_with_goal(None, None, None, Some("b\n"), "x"),
        crate::seat_rotation::compose_handoff_show(None, Some("b\n"), "x"),
        "no goal: unchanged"
    );
}

fn view(role: &str, goal: Option<crate::whoami_view::GoalLine>) -> crate::whoami_view::WhoamiView {
    use crate::whoami_view::*;
    WhoamiView {
        handle: format!("claude-{role}-01"),
        role: role.into(),
        agent_type: "codex".into(),
        session_id: None,
        lease_id: None,
        node: None,
        mail: MailLine::default(),
        working: Working::Idle { next: None },
        goal,
        worktree: WorktreeLine {
            path: "/w".into(),
            branch: None,
        },
    }
}

// AC3: in the advisor seat whoami prints the goal on `goal:` and the seat
// purpose on `purpose:`; other seats keep their line and never see the goal.
// trace:STORY-b6b1278890774ef993d2ebc327862470-272.acd25002 | ai:claude
#[test]
fn whoami_prints_the_goal_in_the_advisor_seat_only() {
    use crate::whoami_view::{goal_line, with_advisor_goal};
    let dir = store_with(vec![goal("X", clauses())]);
    assert_eq!(
        whoami_goal_text(dir.path(), "advisor").as_deref(),
        Some("X")
    );
    assert_eq!(whoami_goal_text(dir.path(), "implementer"), None);

    let now = chrono::Utc::now();
    let purpose = || {
        goal_line(
            &crate::gate::mandate::MandateState::None,
            Some("triage and route".into()),
            now,
            String::new,
        )
    };
    let advisor = view(
        "advisor",
        with_advisor_goal(purpose(), whoami_goal_text(dir.path(), "advisor")),
    );
    let toon = advisor.render_toon();
    assert!(toon.contains("\ngoal: X\n"), "{toon}");
    assert!(toon.contains("\npurpose: triage and route\n"), "{toon}");
    let human = advisor.render_human();
    assert!(human.contains("goal:     X\n"), "{human}");
    assert!(human.contains("purpose:  triage and route\n"), "{human}");
    let json = serde_json::to_value(&advisor).unwrap();
    assert_eq!(json["goal"]["advisor_goal"], "X");

    let implementer = view(
        "implementer",
        with_advisor_goal(purpose(), whoami_goal_text(dir.path(), "implementer")),
    );
    let toon = implementer.render_toon();
    assert!(toon.contains("goal: purpose: triage and route\n"), "{toon}");
    assert!(
        !toon.contains("purpose: triage and route\npurpose"),
        "{toon}"
    );
    assert!(!toon.contains("goal: X"), "{toon}");
}

fn parse(argv: &[&str]) -> GoalArgs {
    <GoalArgs as clap::Parser>::try_parse_from(argv).unwrap()
}

// AC8: bare `aida goal --epic … --invoke` still derives a `/goal` line.
#[test]
fn bare_goal_flags_are_unchanged() {
    let args = parse(&["goal", "--epic", "EPIC-612", "--invoke"]);
    assert!(args.command.is_none());
    assert_eq!(args.epic.as_deref(), Some("EPIC-612"));
    assert!(args.invoke);
    let c =
        build_goal_clauses_for_forge(None, Some("EPIC-612"), None, None, None, ForgeKind::GitHub)
            .unwrap();
    assert_eq!(
        format!("/goal {}", crate::goal_cmd::assemble_goal_condition(&c)),
        "/goal all direct children of EPIC-612 are resolved (Completed or Rejected) \
         (verify: `aida list --parent EPIC-612` returns no rows)"
    );
}

#[test]
fn advisor_subcommands_parse() {
    let args = parse(&[
        "goal",
        "advisor",
        "set",
        "X",
        "--epic",
        "EPIC-612",
        "--queue-empty",
        "implementer",
    ]);
    match args.command {
        Some(GoalCommand::Advisor {
            action:
                AdvisorGoalAction::Set {
                    text,
                    epic,
                    queue_empty,
                    ..
                },
        }) => {
            assert_eq!(text, "X");
            assert_eq!(epic.as_deref(), Some("EPIC-612"));
            assert_eq!(queue_empty.as_deref(), Some("implementer"));
        }
        other => panic!("{other:?}"),
    }
    for action in ["show", "check", "history"] {
        assert!(parse(&["goal", "advisor", action]).command.is_some());
    }
}
