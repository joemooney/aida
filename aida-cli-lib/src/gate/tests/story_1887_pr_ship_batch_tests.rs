//! Tests for `aida pr ship-batch` (STORY-1887, STORY-1889).
// trace:STORY-1887 | ai:claude
// trace:STORY-1889 | ai:agy

use super::*;
use crate::pr_ship_batch::{finished_summary, run_batch, ship_batch_event, BatchOptions};
use std::collections::{BTreeMap, VecDeque};

fn open(head: &str) -> PrView {
    PrView {
        state: PrState::Open,
        mergeable: Mergeable::Clean,
        behind: false,
        draft: false,
        head_sha: head.to_string(),
        head_ref: "branch".to_string(),
        base_ref: "main".to_string(),
        title: "title".to_string(),
        body: String::new(),
    }
}

/// A scripted world. Views are consumed per PR (the last repeats); CI and
/// reviews are keyed by head and consumed per poll (the last repeats).
#[derive(Default)]
struct Fake {
    human: bool,
    views: BTreeMap<u64, VecDeque<PrView>>,
    stale: BTreeMap<u64, Vec<String>>,
    behind_base: Vec<u64>,
    ci: BTreeMap<String, VecDeque<CiAtHead>>,
    reviews: BTreeMap<String, VecDeque<ReviewAtHead>>,
    holds: BTreeMap<u64, String>,
    hold_unreadable: Vec<u64>,
    hold_reads: u32,
    ship_refuses: Vec<u64>,
    answers: VecDeque<String>,
    clock: u64,
    forge_calls: u32,
    updates: Vec<(u64, String)>,
    ci_heads: Vec<(u64, String)>,
    ci_held: Vec<(u64, bool)>,
    ci_unreadable_after: Option<usize>,
    enqueued: Vec<(u64, String)>,
    cleared: Vec<u64>,
    shipped: Vec<(u64, ShipOverrides)>,
    said: Vec<String>,
    fetches: u32,
    recorded: Vec<(Option<u64>, String)>,
    default_branch: Option<String>,
}

impl Fake {
    fn human() -> Self {
        Fake {
            human: true,
            ..Fake::default()
        }
    }
    fn views(mut self, pr: u64, views: Vec<PrView>) -> Self {
        self.views.insert(pr, views.into());
        self
    }
    fn ci(mut self, head: &str, states: Vec<CiAtHead>) -> Self {
        self.ci.insert(head.to_string(), states.into());
        self
    }
    fn review(mut self, head: &str, states: Vec<ReviewAtHead>) -> Self {
        self.reviews.insert(head.to_string(), states.into());
        self
    }
    fn answers(mut self, answers: &[&str]) -> Self {
        self.answers = answers.iter().map(|a| a.to_string()).collect();
        self
    }
}

fn next<T: Clone>(q: &mut VecDeque<T>) -> Option<T> {
    if q.len() > 1 {
        q.pop_front()
    } else {
        q.front().cloned()
    }
}

fn approved() -> ReviewAtHead {
    ReviewAtHead::Approved("reviewer".to_string())
}

impl BatchPorts for Fake {
    fn human_at_terminal(&self) -> bool {
        self.human
    }
    fn view(&mut self, pr: u64) -> Result<PrView> {
        self.forge_calls += 1;
        self.views
            .get_mut(&pr)
            .and_then(next)
            .ok_or_else(|| anyhow::anyhow!("no such PR"))
    }
    fn staleness(&mut self, pr: u64, _view: &PrView, fetch: bool) -> Result<Staleness> {
        self.forge_calls += 1;
        self.fetches += u32::from(fetch);
        Ok(Staleness {
            behind: self.behind_base.contains(&pr),
            definition_files: self.stale.get(&pr).cloned().unwrap_or_default(),
        })
    }
    fn update_branch(&mut self, pr: u64, expected_head: &str) -> Result<()> {
        self.forge_calls += 1;
        self.updates.push((pr, expected_head.to_string()));
        Ok(())
    }
    fn ci_at_head(&mut self, pr: u64, head: &str, held: bool) -> Result<CiAtHead> {
        self.forge_calls += 1;
        if self
            .ci_unreadable_after
            .is_some_and(|n| self.ci_heads.len() >= n)
        {
            anyhow::bail!("rollup unavailable");
        }
        self.ci_heads.push((pr, head.to_string()));
        self.ci_held.push((pr, held));
        Ok(self
            .ci
            .get_mut(head)
            .and_then(next)
            .unwrap_or(CiAtHead::HeadMoved))
    }
    fn enqueue_review(&mut self, pr: u64, view: &PrView) {
        self.enqueued.push((pr, view.head_sha.clone()));
    }
    fn review_at_head(&mut self, _pr: u64, view: &PrView) -> ReviewAtHead {
        self.reviews
            .get_mut(&view.head_sha)
            .and_then(next)
            .unwrap_or(ReviewAtHead::Awaiting(None))
    }
    fn hold(&mut self, pr: u64) -> Result<Option<String>> {
        self.hold_reads += 1;
        if self.hold_unreadable.contains(&pr) {
            anyhow::bail!("label lookup failed");
        }
        Ok(self.holds.get(&pr).cloned())
    }
    fn completion_preview(&mut self, _pr: u64, _view: &PrView) -> Option<String> {
        None
    }
    fn ask(&mut self, _prompt: &str) -> Option<String> {
        self.answers.pop_front()
    }
    fn release_merge_hold(&mut self, pr: u64) -> Result<()> {
        self.cleared.push(pr);
        Ok(())
    }
    fn ship(&mut self, pr: u64, overrides: ShipOverrides) -> Result<()> {
        if self.ship_refuses.contains(&pr) {
            anyhow::bail!("the ship gate refused");
        }
        self.shipped.push((pr, overrides));
        Ok(())
    }
    fn sleep(&mut self, secs: u64) {
        self.clock += secs;
    }
    fn elapsed_secs(&self) -> u64 {
        self.clock
    }
    fn say(&mut self, line: &str) {
        self.said.push(line.to_string());
    }
    fn record(&mut self, pr: Option<u64>, outcome: &str, _detail: &str) {
        self.recorded.push((pr, outcome.to_string()));
    }
    fn default_branch(&mut self) -> Option<String> {
        self.default_branch.clone()
    }
}

const OPTS: BatchOptions = BatchOptions {
    review_timeout_secs: 600,
    ci_timeout_secs: 600,
    dry_run: false,
};

fn ready(f: Fake, pr: u64, head: &str) -> Fake {
    f.views(pr, vec![open(head)])
        .ci(head, vec![CiAtHead::Passing])
        .review(head, vec![approved()])
}

// T1: `aida pr ship-batch 7 3 9` parses to the new subcommand, in order.
// trace:STORY-1887 | ai:claude
#[test]
fn t1_ship_batch_parses_in_order() {
    let cli = crate::command_registry::try_parse_from(["aida", "pr", "ship-batch", "7", "3", "9"])
        .unwrap();
    match cli.command.registered::<crate::pr_cmd::PrArgs>() {
        crate::pr_cmd::PrArgs {
            inner:
                crate::cli::PrCommand::ShipBatch {
                    prs,
                    review_timeout,
                    dry_run,
                },
        } => {
            assert_eq!(prs, vec![7, 3, 9]);
            assert_eq!(review_timeout, None);
            assert!(!dry_run);
        }
        _ => panic!("expected pr ship-batch"),
    }
    let cli = crate::command_registry::try_parse_from([
        "aida",
        "pr",
        "ship-batch",
        "1",
        "--review-timeout",
        "2h",
    ])
    .unwrap();
    match cli.command.registered::<crate::pr_cmd::PrArgs>() {
        crate::pr_cmd::PrArgs {
            inner: crate::cli::PrCommand::ShipBatch { review_timeout, .. },
        } => assert_eq!(review_timeout, Some(7200)),
        _ => panic!("expected pr ship-batch"),
    }
    assert!(crate::command_registry::try_parse_from(["aida", "pr", "ship-batch"]).is_err());
}

// C2: duplicates, more than 20, and an empty list are refused before
// anything else, and nothing touches the forge without 'go'.
// trace:STORY-1887 | ai:claude
#[test]
fn c2_input_hygiene_and_up_front_go() {
    assert!(validate_order(&[1, 2, 1]).unwrap_err().contains("twice"));
    let many: Vec<u64> = (1..=21).collect();
    assert!(validate_order(&many).unwrap_err().contains("at most 20"));
    assert!(validate_order(&[]).is_err());
    assert!(validate_order(&(1..=20).collect::<Vec<u64>>()).is_ok());

    let mut f = Fake::human();
    let err = run_batch(&mut f, &[1, 1], OPTS).unwrap_err();
    assert!(err.contains("twice"));
    assert_eq!(f.forge_calls, 0);

    let mut f = ready(Fake::human(), 1, "aaaaaaaaaaaa1").answers(&["yes"]);
    assert!(run_batch(&mut f, &[1], OPTS).unwrap_err().contains("'go'"));
    assert_eq!(f.forge_calls, 0, "no forge call before 'go'");
    assert!(f.shipped.is_empty());
}

// T2: a caller without the integrity floor is refused before any forge
// call, dry run included.
// trace:STORY-1887 | ai:claude
#[test]
fn t2_non_human_refused_before_any_forge_call() {
    let mut f = ready(Fake::default(), 1, "aaaaaaaaaaaa1").answers(&["go", "1"]);
    let err = run_batch(&mut f, &[1], OPTS).unwrap_err();
    assert!(err.contains("human at an interactive terminal"), "{err}");
    assert_eq!(f.forge_calls, 0);
    assert!(f.shipped.is_empty());
    let mut f = ready(Fake::default(), 1, "aaaaaaaaaaaa1");
    let dry = BatchOptions {
        dry_run: true,
        ..OPTS
    };
    assert!(run_batch(&mut f, &[1], dry).is_err());
    assert_eq!(f.forge_calls, 0);
    // TASK-2172: a refused dry run still records the refusal.
    // trace:TASK-2172 | ai:claude
    assert_eq!(f.recorded, vec![(None, "refused".to_string())]);
}

// T3 + C3: order is kept; a BEHIND PR is refreshed, its old head's green
// and old head's approval are ignored, CI and review are awaited at the
// new head; a red CI stops the batch and later PRs are never touched.
// trace:STORY-1887 | ai:claude
#[test]
fn t3_order_refresh_new_head_and_stop() {
    let mut behind = open("bbbbbbbbbbbb_old");
    behind.behind = true;
    let mut f = ready(Fake::human(), 1, "aaaaaaaaaaaa1")
        .views(2, vec![behind, open("bbbbbbbbbbbb_new")])
        .ci("bbbbbbbbbbbb_old", vec![CiAtHead::Passing])
        .review("bbbbbbbbbbbb_old", vec![approved()])
        .ci(
            "bbbbbbbbbbbb_new",
            vec![CiAtHead::Pending, CiAtHead::Pending, CiAtHead::Passing],
        )
        .review(
            "bbbbbbbbbbbb_new",
            vec![
                ReviewAtHead::Awaiting(None),
                ReviewAtHead::Awaiting(None),
                approved(),
            ],
        )
        .views(3, vec![open("cccccccccccc3")])
        .ci(
            "cccccccccccc3",
            vec![CiAtHead::Failing("Build".to_string())],
        )
        .views(4, vec![open("dddddddddddd4")])
        .answers(&["go", "1", "2", "3", "4"]);
    let out = run_batch(&mut f, &[1, 2, 3, 4], OPTS).unwrap();
    assert_eq!(out[0], (1, PrOutcome::Shipped));
    assert_eq!(out[1], (2, PrOutcome::Shipped));
    assert!(matches!(&out[2], (3, PrOutcome::Stopped(why)) if why.contains("red")));
    assert_eq!(out.len(), 3, "PR 4 is never reached");
    assert_eq!(f.updates, vec![(2, "bbbbbbbbbbbb_old".to_string())]);
    assert!(
        f.ci_heads
            .iter()
            .filter(|(pr, _)| *pr == 2)
            .all(|(_, head)| head == "bbbbbbbbbbbb_new"),
        "C3: CI is only ever read at the refreshed head: {:?}",
        f.ci_heads
    );
    assert_eq!(f.enqueued, vec![(2, "bbbbbbbbbbbb_new".to_string())]);
    assert!(f.ci_heads.iter().all(|(pr, _)| *pr != 4));
    assert_eq!(
        f.shipped.iter().map(|(pr, _)| *pr).collect::<Vec<_>>(),
        vec![1, 2]
    );
}

// C1 + Q3: the review wait names the PR, the head, and that an
// independent review is awaited, about once a minute, and stops at the
// timeout without touching later PRs.
// trace:STORY-1887 | ai:claude
#[test]
fn c1_review_wait_is_honest_and_stops_on_timeout() {
    let mut f = Fake::human()
        .views(5, vec![open("eeeeeeeeeeee5")])
        .ci("eeeeeeeeeeee5", vec![CiAtHead::Passing])
        .answers(&["go", "5"]);
    let out = run_batch(&mut f, &[5, 6], OPTS).unwrap();
    assert!(
        matches!(&out[0], (5, PrOutcome::Stopped(why)) if why.contains("no independent approval"))
    );
    assert_eq!(out.len(), 1);
    let awaiting: Vec<_> = f
        .said
        .iter()
        .filter(|l| l.contains("awaiting an independent review at this head"))
        .collect();
    assert!(
        awaiting[0].contains("PR #5 at eeeeeeeeeeee"),
        "{awaiting:?}"
    );
    assert!(
        (9..=11).contains(&awaiting.len()),
        "about one line a minute over 10 minutes: {}",
        awaiting.len()
    );
    assert!(f.shipped.is_empty());
}

// A negative verdict at the head stops the batch.
// trace:STORY-1887 | ai:claude
#[test]
fn negative_verdict_at_head_stops() {
    let mut f = Fake::human()
        .views(5, vec![open("eeeeeeeeeeee5")])
        .ci("eeeeeeeeeeee5", vec![CiAtHead::Passing])
        .review(
            "eeeeeeeeeeee5",
            vec![ReviewAtHead::Negative(
                "the reviewer recorded REJECTED".into(),
            )],
        )
        .answers(&["go", "5"]);
    let out = run_batch(&mut f, &[5], OPTS).unwrap();
    assert!(matches!(&out[0], (5, PrOutcome::Stopped(why)) if why.contains("REJECTED")));
    assert!(f.shipped.is_empty());
}

// T4: anything but the exact PR number ships nothing.
// trace:STORY-1887 | ai:claude
#[test]
fn t4_confirmation_must_be_the_exact_pr_number() {
    for answers in [
        vec!["go", "8"],
        vec!["go", "y"],
        vec!["go", ""],
        vec!["go", "78"],
        vec!["go"],
    ] {
        let mut f = ready(Fake::human(), 7, "777777777777a").answers(&answers);
        let out = run_batch(&mut f, &[7], OPTS).unwrap();
        assert!(matches!(&out[0], (7, PrOutcome::Stopped(_))), "{answers:?}");
        assert!(f.shipped.is_empty(), "{answers:?}");
    }
    let mut f = ready(Fake::human(), 7, "777777777777a").answers(&["go", " 7\n"]);
    assert_eq!(
        run_batch(&mut f, &[7], OPTS).unwrap()[0].1,
        PrOutcome::Shipped
    );
}

// Q2: a hold is cleared only after this PR was confirmed and 'clear' typed.
// trace:STORY-1887 | ai:claude
#[test]
fn q2_hold_cleared_only_with_confirmation_and_clear() {
    let held = |answers: &[&str]| {
        let mut f = ready(Fake::human(), 9, "999999999999a").answers(answers);
        f.holds.insert(9, "supervised".to_string());
        let out = run_batch(&mut f, &[9], OPTS).unwrap();
        (out, f)
    };
    let (out, f) = held(&["go", "9", "clear"]);
    assert_eq!(out[0].1, PrOutcome::Shipped);
    assert_eq!(f.cleared, vec![9]);
    let (out, f) = held(&["go", "9", "y"]);
    assert!(matches!(out[0].1, PrOutcome::Stopped(_)));
    assert!(f.cleared.is_empty() && f.shipped.is_empty());
    let (_, f) = held(&["go", "no", "clear"]);
    assert!(f.cleared.is_empty() && f.shipped.is_empty());
}

// T5: the ship is always made with both overrides false.
// trace:STORY-1887 | ai:claude
#[test]
fn t5_ship_never_overrides() {
    let mut f = ready(ready(Fake::human(), 1, "aaaaaaaaaaaa1"), 2, "bbbbbbbbbbbb2")
        .answers(&["go", "1", "2"]);
    run_batch(&mut f, &[1, 2], OPTS).unwrap();
    assert_eq!(f.shipped.len(), 2);
    assert!(f.shipped.iter().all(|(_, o)| *o == ShipOverrides::NONE));
    assert!(!ShipOverrides::NONE.stale_check && !ShipOverrides::NONE.stale_approval);
}

// T6: a merged PR is skipped; closed and conflicting PRs stop the batch.
// trace:STORY-1887 | ai:claude
#[test]
fn t6_merged_skips_closed_and_conflicting_stop() {
    let mut merged = open("111111111111a");
    merged.state = PrState::Merged;
    let mut f =
        ready(Fake::human().views(1, vec![merged]), 2, "222222222222a").answers(&["go", "2"]);
    let out = run_batch(&mut f, &[1, 2], OPTS).unwrap();
    assert_eq!(out[0], (1, PrOutcome::AlreadyMerged));
    assert_eq!(out[1], (2, PrOutcome::Shipped));

    let mut closed = open("111111111111a");
    closed.state = PrState::Closed;
    let mut conflicting = open("111111111111a");
    conflicting.mergeable = Mergeable::Conflicting;
    for (view, word) in [(closed, "closed"), (conflicting, "conflicts")] {
        let mut f = ready(Fake::human().views(1, vec![view]), 2, "222222222222a")
            .answers(&["go", "1", "2"]);
        let out = run_batch(&mut f, &[1, 2], OPTS).unwrap();
        assert!(matches!(&out[0], (1, PrOutcome::Stopped(why)) if why.contains(word)));
        assert_eq!(out.len(), 1);
        assert!(f.shipped.is_empty() && f.updates.is_empty());
    }
}

// T7: a head that keeps moving after approval is re-awaited, then stops;
// a single move is re-awaited and ships at the new head.
// trace:STORY-1887 | ai:claude
#[test]
fn t7_moving_head_rewaits_then_stops() {
    let heads = [
        "h1h1h1h1h1h1",
        "h2h2h2h2h2h2",
        "h3h3h3h3h3h3",
        "h4h4h4h4h4h4",
    ];
    let views = heads.iter().map(|h| open(h)).collect();
    let mut f = Fake::human().views(3, views).answers(&["go", "3"]);
    for h in heads {
        f = f.ci(h, vec![CiAtHead::Passing]).review(h, vec![approved()]);
    }
    let out = run_batch(&mut f, &[3], OPTS).unwrap();
    assert!(
        matches!(&out[0], (3, PrOutcome::Stopped(why)) if why.contains("kept moving")),
        "{out:?}"
    );
    assert!(f.shipped.is_empty());

    let mut f = Fake::human()
        .views(3, vec![open(heads[0]), open(heads[1])])
        .answers(&["go", "3"]);
    for h in &heads[..2] {
        f = f.ci(h, vec![CiAtHead::Passing]).review(h, vec![approved()]);
    }
    assert_eq!(
        run_batch(&mut f, &[3], OPTS).unwrap()[0].1,
        PrOutcome::Shipped
    );
}

// C4: staleness from the gate's predicate alone triggers a refresh.
// trace:STORY-1887 | ai:claude
#[test]
fn c4_stale_definition_triggers_refresh() {
    let mut f = ready(Fake::human(), 4, "new_head_0000")
        .views(4, vec![open("old_head_0000"), open("new_head_0000")])
        .answers(&["go", "4"]);
    f.stale
        .insert(4, vec![".github/workflows/ci.yml".to_string()]);
    let out = run_batch(&mut f, &[4], OPTS).unwrap();
    assert_eq!(out[0].1, PrOutcome::Shipped);
    assert_eq!(f.updates, vec![(4, "old_head_0000".to_string())]);
    assert!(f
        .said
        .iter()
        .any(|l| l.contains(".github/workflows/ci.yml")));
}

// STORY-1690: a head behind its base is refreshed before ship even when
// the forge does not report BEHIND (strict is off), because ship now
// refuses a behind head.
// trace:STORY-1690.acb4ffd6 | ai:claude
#[test]
fn behind_base_by_git_alone_triggers_refresh() {
    let mut f = ready(Fake::human(), 5, "new_head_0005")
        .views(5, vec![open("old_head_0005"), open("new_head_0005")])
        .answers(&["go", "5"]);
    f.behind_base.push(5);
    let out = run_batch(&mut f, &[5], OPTS).unwrap();
    assert_eq!(out[0].1, PrOutcome::Shipped);
    assert_eq!(f.updates, vec![(5, "old_head_0005".to_string())]);
    assert!(f.said.iter().any(|l| l.contains("behind main")));
}

// C3 for the real port: a rollup from another head never counts, and no
// checks yet is not green.
// trace:STORY-1887 | ai:claude
#[test]
fn c3_rollup_from_another_head_is_head_moved() {
    let cfg = crate::ci_gate::CiGateConfig::default();
    let a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let green = serde_json::json!({
        "headRefOid": a,
        "statusCheckRollup": [{"name": "Build", "status": "COMPLETED", "conclusion": "SUCCESS"}],
    });
    assert_eq!(
        ci_from_rollup(
            &green,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            &cfg,
            false
        ),
        CiAtHead::HeadMoved
    );
    assert_eq!(ci_from_rollup(&green, a, &cfg, false), CiAtHead::Passing);
    let empty = serde_json::json!({ "headRefOid": a, "statusCheckRollup": [] });
    assert_eq!(ci_from_rollup(&empty, a, &cfg, false), CiAtHead::Pending);
    let view = parse_pr_view(&serde_json::json!({
        "state": "OPEN", "mergeable": "CONFLICTING", "mergeStateStatus": "BEHIND",
        "isDraft": true, "headRefOid": "abc", "headRefName": "b", "title": "t", "body": "",
    }));
    assert_eq!(view.mergeable, Mergeable::Conflicting);
    assert!(view.behind && view.draft);
}

// The GitHub update-branch call is pinned to the head it was read at.
// trace:STORY-1887 | ai:claude
#[test]
fn update_branch_is_pinned_to_the_read_head() {
    let argv = crate::forge::github_update_branch_argv(42, "abc123");
    assert!(argv.contains(&"repos/{owner}/{repo}/pulls/42/update-branch".to_string()));
    assert!(argv.contains(&"expected_head_sha=abc123".to_string()));
}

// T8: the batch gate module is a mandate-protected gate file.
// trace:STORY-1887 | ai:claude
// trace:STORY-1889 | ai:agy
#[test]
fn t8_ship_batch_is_gate_protected() {
    // trace:STORY-1888 | ai:claude
    let hit = crate::gate::mandate_gates::touches_gates(
        &["aida-cli-lib/src/gate/pr_ship_batch.rs".to_string()],
        crate::gate::mandate_gates::ProtectedPaths::floor_ref(),
    )
    .map(|m| m.path);
    assert_eq!(
        hit,
        Some("aida-cli-lib/src/gate/pr_ship_batch.rs".to_string())
    );
}

// TASK-2155: the start, each PR's outcome and the stop are recorded, and
// the record is an events-feed kind `aida history events` can filter on.
// trace:STORY-1887 | ai:claude
#[test]
fn task_2155_batch_steps_reach_the_events_feed() {
    let mut f = ready(Fake::human(), 1, "aaaaaaaaaaaa1")
        .views(2, vec![open("bbbbbbbbbbbb2")])
        .ci("bbbbbbbbbbbb2", vec![CiAtHead::Failing("Build".into())])
        .answers(&["go", "1"]);
    run_batch(&mut f, &[1, 2], OPTS).unwrap();
    assert_eq!(
        f.recorded,
        vec![
            (None, "started".to_string()),
            (Some(1), "shipped".to_string()),
            (Some(2), "stopped".to_string()),
            (None, "finished".to_string()),
        ]
    );
    assert_eq!(
        finished_summary(&[
            (1, PrOutcome::Shipped),
            (2, PrOutcome::Stopped("CI red".into()))
        ]),
        "1 shipped, 0 already merged, stopped at #2"
    );
    let ev = ship_batch_event(Some(2), "stopped", " CI red \n");
    assert_eq!(ev.kind.name(), "ShipBatchStep");
    assert!(crate::events::EventKind::known_names().contains(&"ShipBatchStep"));
    assert!(!ev.kind.is_actionable());
    let line = serde_json::to_string(&ev).unwrap();
    assert!(line.contains("\"event\":\"ShipBatchStep\""), "{line}");
    assert!(line.contains("\"detail\":\"CI red\""), "{line}");
    let back: crate::events::Event = serde_json::from_str(&line).unwrap();
    assert_eq!(back.kind, ev.kind);
}

// TASK-2156: the review wait looks verdicts up under the same IDs the
// ship gate does, including one that appears only in the PR title.
// trace:STORY-1887 | ai:claude
#[test]
fn task_2156_known_ids_include_a_title_only_spec_id() {
    let tmp = tempfile::tempdir().unwrap();
    let ids = crate::pr_cmd::ship_gate_known_ids(
        tmp.path(),
        "claude/story-1887",
        Some("feat(pr): ship-batch (TASK-9901)"),
        None,
    );
    assert!(ids.contains(&"TASK-9901".to_string()), "{ids:?}");
    assert!(ids.contains(&"STORY-1887".to_string()), "{ids:?}");
}

// TASK-2157: `--dry-run` never fetches; a real run does.
// trace:STORY-1887 | ai:claude
#[test]
fn task_2157_dry_run_never_fetches() {
    let dry = BatchOptions {
        dry_run: true,
        ..OPTS
    };
    let mut f = ready(Fake::human(), 4, "head_0000");
    f.stale
        .insert(4, vec![".github/workflows/ci.yml".to_string()]);
    let out = run_batch(&mut f, &[4], dry).unwrap();
    assert!(
        matches!(&out[0].1, PrOutcome::Planned(p) if p.contains("would refresh")),
        "{out:?}"
    );
    assert_eq!(f.fetches, 0);
    assert!(f.updates.is_empty() && f.shipped.is_empty());

    let mut f = ready(Fake::human(), 4, "head_0000").answers(&["go", "4"]);
    run_batch(&mut f, &[4], OPTS).unwrap();
    assert_eq!(f.fetches, 1);
}

// Round 2: a held PR whose only red check is merge-hold-gate is green for
// the batch and reaches the hold-clear prompt; another red check on a
// held PR still stops it; unheld, merge-hold-gate red is a CI failure.
// trace:STORY-1887 | ai:claude
#[test]
fn held_pr_red_hold_gate_reaches_the_clear_prompt() {
    let cfg = crate::ci_gate::CiGateConfig::default();
    let head = "cccccccccccccccccccccccccccccccccccccccc";
    let rollup = |checks: serde_json::Value| serde_json::json!({ "headRefOid": head, "statusCheckRollup": checks });
    let ok = serde_json::json!({"name": "Build", "status": "COMPLETED", "conclusion": "SUCCESS"});
    let hold_red = serde_json::json!({
        "name": crate::ci_gate::HOLD_GATE_CHECK, "status": "COMPLETED", "conclusion": "FAILURE",
    });
    let build_red =
        serde_json::json!({"name": "Build", "status": "COMPLETED", "conclusion": "FAILURE"});
    let only_hold = rollup(serde_json::json!([ok.clone(), hold_red.clone()]));
    let ci = ci_from_rollup(&only_hold, head, &cfg, true);
    assert_eq!(ci, CiAtHead::Passing);
    assert!(matches!(
        ci_from_rollup(&only_hold, head, &cfg, false),
        CiAtHead::Failing(_)
    ));
    assert!(matches!(
        ci_from_rollup(
            &rollup(serde_json::json!([build_red, hold_red])),
            head,
            &cfg,
            true
        ),
        CiAtHead::Failing(_)
    ));

    let mut f = Fake::human()
        .views(9, vec![open(head)])
        .ci(head, vec![ci])
        .review(head, vec![approved()])
        .answers(&["go", "9", "clear"]);
    f.holds.insert(9, "supervised".to_string());
    let out = run_batch(&mut f, &[9], OPTS).unwrap();
    assert_eq!(out[0].1, PrOutcome::Shipped);
    assert_eq!(f.cleared, vec![9]);
}

// Round 2: an unreadable hold label is reported as such and stops the PR;
// it is never offered as a hold to clear.
// trace:STORY-1887 | ai:claude
#[test]
fn unreadable_hold_label_stops_without_a_clear_prompt() {
    let mut f = ready(Fake::human(), 9, "999999999999a").answers(&["go", "9", "clear"]);
    f.hold_unreadable.push(9);
    let out = run_batch(&mut f, &[9], OPTS).unwrap();
    assert!(
        matches!(&out[0].1, PrOutcome::Stopped(why) if why.contains("could not read the hold label")),
        "{out:?}"
    );
    assert!(f.cleared.is_empty() && f.shipped.is_empty());
    assert_eq!(f.answers.len(), 2, "only 'go' was consumed");
}

// TASK-2172: a ship refused after the hold was cleared says the hold is
// already gone, so the operator does not believe the PR is still held.
// trace:TASK-2172 | ai:claude
#[test]
fn task_2172_ship_refused_after_clear_says_hold_already_cleared() {
    let mut f = ready(Fake::human(), 9, "999999999999a").answers(&["go", "9", "clear"]);
    f.holds.insert(9, "supervised".to_string());
    f.ship_refuses.push(9);
    let out = run_batch(&mut f, &[9], OPTS).unwrap();
    assert_eq!(f.cleared, vec![9]);
    assert!(
        matches!(&out[0].1, PrOutcome::Stopped(why)
            if why.contains("refused") && why.contains("merge-hold was already cleared")),
        "{out:?}"
    );
    // Without a hold to clear, the refusal does not mention one.
    let mut f = ready(Fake::human(), 9, "999999999999a").answers(&["go", "9"]);
    f.ship_refuses.push(9);
    let out = run_batch(&mut f, &[9], OPTS).unwrap();
    assert!(
        matches!(&out[0].1, PrOutcome::Stopped(why)
            if why.contains("refused") && !why.contains("merge-hold")),
        "{out:?}"
    );
}

// TASK-2172: a hold label that cannot be read while waiting for CI is
// reported as the hold label, not as unreadable CI.
// trace:TASK-2172 | ai:claude
#[test]
fn task_2172_unreadable_hold_label_during_ci_wait_names_the_hold_label() {
    let mut f = ready(Fake::human(), 9, "999999999999a").answers(&["go", "9"]);
    f.hold_unreadable.push(9);
    let out = run_batch(&mut f, &[9], OPTS).unwrap();
    assert!(
        matches!(&out[0].1, PrOutcome::Stopped(why)
            if why.contains("could not read the hold label") && !why.contains("CI")),
        "{out:?}"
    );
    assert!(
        f.ci_heads.is_empty(),
        "CI is not read without the hold state"
    );
    assert!(f.shipped.is_empty());
}

// TASK-2180: a dry run reads the hold once per PR; an unreadable hold is
// reported once, as the hold, and never as unreadable CI.
// trace:TASK-2180 | ai:claude
#[test]
fn task_2180_dry_run_reads_the_hold_once_and_names_it() {
    let dry = BatchOptions {
        dry_run: true,
        ..OPTS
    };
    let mut f = ready(Fake::human(), 9, "999999999999a");
    f.holds.insert(9, "supervised".to_string());
    let out = run_batch(&mut f, &[9], dry).unwrap();
    assert!(
        matches!(&out[0].1, PrOutcome::Planned(p)
            if p.contains("CI at 999999999999: green") && p.contains("merge-hold: supervised")),
        "{out:?}"
    );
    assert_eq!(f.hold_reads, 1);
    // TASK-2193: CI is judged with the hold state that was read.
    // trace:TASK-1951 trace:TASK-2193 | ai:claude
    assert_eq!(f.ci_held, vec![(9, true)]);

    let mut f = ready(Fake::human(), 9, "999999999999a");
    f.hold_unreadable.push(9);
    let out = run_batch(&mut f, &[9], dry).unwrap();
    let PrOutcome::Planned(plan) = &out[0].1 else {
        panic!("{out:?}")
    };
    assert_eq!(
        plan.matches("could not read the hold label").count(),
        1,
        "{plan}"
    );
    assert!(!plan.contains("unreadable"), "{plan}");
    assert_eq!(f.hold_reads, 1);
    assert!(
        f.ci_heads.is_empty(),
        "CI is not judged without the hold state"
    );
}

// TASK-2193: the dry-run plan judges CI with the PR's hold state, so a
// held PR's red merge-hold-gate is not shown as red CI.
// trace:TASK-1951 trace:TASK-2193 | ai:claude
#[test]
fn task_2193_dry_run_passes_the_hold_state_to_ci() {
    let dry = BatchOptions {
        dry_run: true,
        ..OPTS
    };
    let mut f = ready(Fake::human(), 9, "999999999999a");
    f.holds.insert(9, "supervised".to_string());
    let out = run_batch(&mut f, &[9], dry).unwrap();
    assert!(
        matches!(&out[0].1, PrOutcome::Planned(p) if p.contains("merge-hold: supervised")),
        "{out:?}"
    );
    assert_eq!(f.ci_held, vec![(9, true)]);

    let mut f = ready(Fake::human(), 9, "999999999999a");
    run_batch(&mut f, &[9], dry).unwrap();
    assert_eq!(f.ci_held, vec![(9, false)]);
}

// TASK-2194: a CI read error while the review is awaited is said once,
// not retried silently, and the timeout names it.
// trace:TASK-1951 trace:TASK-2194 | ai:claude
#[test]
fn task_2194_review_wait_surfaces_ci_read_errors() {
    let head = "999999999999a";
    let mut f = Fake::human()
        .views(9, vec![open(head)])
        .ci(head, vec![CiAtHead::Passing])
        .answers(&["go", "9"]);
    f.ci_unreadable_after = Some(1);
    let out = run_batch(&mut f, &[9], OPTS).unwrap();
    assert!(
        matches!(&out[0].1, PrOutcome::Stopped(why)
            if why.contains("no independent approval")
                && why.contains("the last CI read failed")
                && why.contains("rollup unavailable")),
        "{out:?}"
    );
    let said: Vec<_> = f
        .said
        .iter()
        .filter(|l| l.contains("rollup unavailable"))
        .collect();
    assert_eq!(said.len(), 1, "the same error is said once: {:?}", f.said);
    assert!(said[0].contains("retrying"), "{said:?}");
    assert!(f.shipped.is_empty());
}

// BUG-HOME-042: a PR still based on its merged parent's branch stops the
// batch before any refresh or ship; the dry run says it would stop; a PR on
// the default branch, or an unknown default, ships as before.
// trace:BUG-B6B1278890774EF993D2EBC327862470-042 | ai:claude
#[test]
fn bug_home_042_stacked_base_stops_before_refresh() {
    let mut stacked = open("333333333333a");
    stacked.base_ref = "task-106-work".to_string();
    stacked.behind = true;
    let mut f = ready(Fake::human(), 2, "222222222222a")
        .views(3, vec![stacked.clone()])
        .answers(&["go", "3", "2"]);
    f.default_branch = Some("main".to_string());
    let out = run_batch(&mut f, &[3, 2], OPTS).unwrap();
    assert_eq!(out.len(), 1, "{out:?}");
    assert!(
        matches!(&out[0], (3, PrOutcome::Stopped(why))
            if why.contains("targets `task-106-work`, not `main`")
                && why.contains("gh pr edit 3 --base main")),
        "{out:?}"
    );
    assert!(
        f.updates.is_empty() && f.shipped.is_empty(),
        "nothing refreshed or shipped"
    );

    let dry = BatchOptions {
        dry_run: true,
        ..OPTS
    };
    let mut f = Fake::human().views(3, vec![stacked]);
    f.default_branch = Some("main".to_string());
    let out = run_batch(&mut f, &[3], dry).unwrap();
    assert!(
        matches!(&out[0].1, PrOutcome::Planned(p) if p.contains("targets `task-106-work`")
            && p.contains("the batch would stop here")),
        "{out:?}"
    );

    let mut f = ready(Fake::human(), 2, "222222222222a").answers(&["go", "2"]);
    f.default_branch = Some("main".to_string());
    assert_eq!(
        run_batch(&mut f, &[2], OPTS).unwrap()[0].1,
        PrOutcome::Shipped
    );
}

#[test]
fn bug_home_042_parse_pr_view_reads_the_base() {
    let v = serde_json::json!({"state": "OPEN", "baseRefName": "task-106-work"});
    assert_eq!(parse_pr_view(&v).base_ref, "task-106-work");
}
