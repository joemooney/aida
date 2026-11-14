// trace:STORY-2768 | ai:claude
use super::*;
use crate::land::Ci;

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NOW: &str = "2026-11-13T12:00:00+00:00";

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(NOW)
        .unwrap()
        .with_timezone(&Utc)
}

fn change(id: u64, base: &str) -> Pr {
    Pr {
        id,
        head: HEAD.into(),
        base: base.into(),
        branch: Some(format!("feature-{id}")),
        title: format!("feat(x): a change (STORY-{id})"),
        updated_at: None,
        hold_label: None,
        ci: Some(Ci::Green),
    }
}

fn row(pr: u64, state: State, action: Action) -> Row {
    Row {
        pr,
        track: Track::Main,
        state,
        action,
        since: None,
        reason: String::new(),
        actor: None,
        ci: Some("green".into()),
    }
}

fn child(kind: Kind, pr: u64, minutes_ago: i64) -> Child {
    Child {
        kind,
        prs: vec![pr],
        head: HEAD.into(),
        spec: None,
        merge_tree: None,
        identity: ProcIdentity {
            boot_id: "boot".into(),
            pidns_inode: 1,
            pid: 1000 + pr as u32,
            starttime: 1,
        },
        started_at: (now() - chrono::Duration::minutes(minutes_ago)).to_rfc3339(),
        log: format!("/logs/{pr}.log"),
    }
}

/// Records every side effect a pass asks for.
#[derive(Default)]
struct Fake {
    calls: Vec<String>,
    train_blocker: Option<String>,
    /// How many minutes ago each verdict at the head was recorded.
    verdict: Option<std::result::Result<Vec<i64>, String>>,
    fail_launch: bool,
    release: Option<String>,
}

impl Acts for Fake {
    fn launch_review(&mut self, pr: &Pr, merge_tree: Option<&str>) -> Result<Child> {
        self.calls
            .push(format!("review {} {}", pr.id, merge_tree.unwrap_or("-")));
        anyhow::ensure!(!self.fail_launch, "the store pull was refused");
        Ok(child(Kind::Review, pr.id, 0))
    }
    fn dispatch_rework(&mut self, pr: &Pr, spec: &str) -> Result<Child> {
        self.calls.push(format!("rework {} {spec}", pr.id));
        Ok(child(Kind::Lane, pr.id, 0))
    }
    fn release_hold(&mut self, pr: &Pr) -> Result<String> {
        self.calls.push(format!("release {}", pr.id));
        match &self.release {
            Some(refusal) => anyhow::bail!("{refusal}"),
            None => Ok(format!("PR #{}: released", pr.id)),
        }
    }
    fn train(&mut self, prs: &[u64]) -> Result<Child> {
        self.calls.push(format!("train {prs:?}"));
        Ok(Child {
            prs: prs.to_vec(),
            head: String::new(),
            ..child(Kind::Train, 0, 0)
        })
    }
    fn train_blocker(&mut self) -> Option<String> {
        self.train_blocker.clone()
    }
    fn verdict_times(
        &mut self,
        pr: u64,
        _: &str,
    ) -> std::result::Result<Vec<DateTime<Utc>>, String> {
        self.calls.push(format!("verdict? {pr}"));
        self.verdict.clone().unwrap_or(Ok(Vec::new())).map(|ago| {
            ago.into_iter()
                .map(|minutes| now() - chrono::Duration::minutes(minutes))
                .collect()
        })
    }
    fn stop(&mut self, child: &Child) {
        self.calls.push(format!("stop {}", child.prs[0]));
    }
}

fn acted(changes: &[Pr], rows: &[Row], book: &mut Book, fake: &mut Fake) -> Vec<String> {
    let trees: BTreeMap<u64, String> = changes
        .iter()
        .map(|pr| (pr.id, format!("tree{}", pr.id)))
        .collect();
    act(changes, rows, &trees, book, &Limits::default(), fake, false)
}

#[test]
fn each_row_gets_its_one_action_and_waiting_rows_get_none() {
    let changes: Vec<Pr> = (1..=6).map(|id| change(id, "main")).collect();
    let rows = vec![
        row(1, State::NeedsReview, Action::LaunchReview),
        row(2, State::Conflict, Action::DispatchRework),
        row(3, State::Ready, Action::ClearHoldAndLand),
        row(4, State::Ready, Action::Train),
        row(5, State::Reviewing, Action::Wait),
        row(6, State::Blocked, Action::AdvisorRuling),
    ];
    let (mut book, mut fake) = (Book::default(), Fake::default());
    acted(&changes, &rows, &mut book, &mut fake);
    assert_eq!(
        fake.calls,
        [
            "review 1 tree1",
            "rework 2 STORY-2",
            "release 3",
            "train [4]"
        ]
    );
    // Every child started is recorded, so the next pass sees it alive.
    let kinds: Vec<Kind> = book.children.iter().map(|child| child.kind).collect();
    assert_eq!(kinds, [Kind::Review, Kind::Lane, Kind::Train]);
}

#[test]
fn a_released_hold_is_not_also_landed_in_the_same_pass() {
    let changes = vec![change(3, "main")];
    let rows = vec![row(3, State::Ready, Action::ClearHoldAndLand)];
    let (mut book, mut fake) = (Book::default(), Fake::default());
    let notes = acted(&changes, &rows, &mut book, &mut fake);
    assert_eq!(fake.calls, ["release 3"]);
    assert_eq!(notes, ["PR #3: released"]);
    // The release decides for itself; a refusal leaves the hold in place.
    let mut refused = Fake {
        release: Some("the mandate does not cover a sensitive spec".into()),
        ..Fake::default()
    };
    let notes = acted(&changes, &rows, &mut book, &mut refused);
    assert_eq!(refused.calls, ["release 3"]);
    assert!(notes[0].contains("the hold stays"), "{notes:?}");
    assert!(book.children.is_empty());
}

#[test]
fn reviews_and_lanes_stop_at_their_caps() {
    let changes: Vec<Pr> = (1..=5).map(|id| change(id, "main")).collect();
    // One review is already running, whoever started it.
    let rows = vec![
        row(1, State::Reviewing, Action::Wait),
        row(2, State::NeedsReview, Action::LaunchReview),
        row(3, State::NeedsReview, Action::LaunchReview),
        row(4, State::ChangesRequested, Action::DispatchRework),
        row(5, State::CiFailed, Action::DispatchRework),
    ];
    let mut book = Book {
        children: vec![child(Kind::Lane, 9, 5)],
        ..Book::default()
    };
    let mut fake = Fake::default();
    let notes = acted(&changes, &rows, &mut book, &mut fake);
    assert_eq!(fake.calls, ["review 2 tree2", "rework 4 STORY-4"]);
    assert!(
        notes
            .iter()
            .any(|note| note.starts_with("#3: review waits"))
            && notes
                .iter()
                .any(|note| note.starts_with("#5: rework waits")),
        "{notes:?}"
    );
}

#[test]
fn a_launch_that_fails_records_nothing_and_is_retried_by_the_next_pass() {
    let changes = vec![change(1, "main")];
    let rows = vec![row(1, State::NeedsReview, Action::LaunchReview)];
    let mut book = Book::default();
    let mut failing = Fake {
        fail_launch: true,
        ..Fake::default()
    };
    let notes = acted(&changes, &rows, &mut book, &mut failing);
    assert!(book.children.is_empty());
    assert!(notes[0].contains("retried next pass"), "{notes:?}");
    // The same row, derived again, launches again.
    let mut working = Fake::default();
    acted(&changes, &rows, &mut book, &mut working);
    assert_eq!(working.calls, ["review 1 tree1"]);
    assert_eq!(book.children.len(), 1);
}

#[test]
fn every_ready_main_change_with_green_ci_boards_one_train() {
    let changes: Vec<Pr> = (1..=4).map(|id| change(id, "main")).collect();
    let pending = Row {
        ci: Some("pending".into()),
        ..row(3, State::Ready, Action::Train)
    };
    let rows = vec![
        row(1, State::Ready, Action::Train),
        row(2, State::Ready, Action::Train),
        pending,
        Row {
            track: Track::Integration,
            ..row(4, State::Ready, Action::ReportToIntegration)
        },
    ];
    let (mut book, mut fake) = (Book::default(), Fake::default());
    let notes = acted(&changes, &rows, &mut book, &mut fake);
    assert_eq!(fake.calls, ["train [1, 2]"]);
    assert!(
        notes
            .iter()
            .any(|note| note.starts_with("#3: ready, not boarded")),
        "{notes:?}"
    );
    // While that train runs, no second one starts.
    let mut again = Fake::default();
    let notes = acted(&changes, &rows, &mut book, &mut again);
    assert!(again.calls.is_empty(), "{:?}", again.calls);
    assert!(
        notes
            .iter()
            .any(|note| note.contains("the train of #1 #2 is running")),
        "{notes:?}"
    );
}

#[test]
fn no_train_starts_while_a_journal_or_another_train_remains() {
    let changes = vec![change(1, "main")];
    let rows = vec![row(1, State::Ready, Action::Train)];
    let mut book = Book::default();
    let mut fake = Fake {
        train_blocker: Some("the train journal abc-1.json remains".into()),
        ..Fake::default()
    };
    let notes = acted(&changes, &rows, &mut book, &mut fake);
    assert!(fake.calls.is_empty(), "{:?}", fake.calls);
    assert_eq!(notes, ["#1 wait: the train journal abc-1.json remains"]);
}

#[test]
fn a_dry_run_names_every_action_and_takes_none() {
    let changes: Vec<Pr> = (1..=4).map(|id| change(id, "main")).collect();
    let rows = vec![
        row(1, State::NeedsReview, Action::LaunchReview),
        row(2, State::Conflict, Action::DispatchRework),
        row(3, State::Ready, Action::ClearHoldAndLand),
        row(4, State::Ready, Action::Train),
    ];
    let mut book = Book {
        children: vec![child(Kind::Review, 8, 600)],
        ..Book::default()
    };
    let mut fake = Fake::default();
    let mut notes = supervise(
        &changes,
        &mut book,
        &Limits::default(),
        now(),
        &mut fake,
        true,
    );
    notes.extend(act(
        &changes,
        &rows,
        &BTreeMap::new(),
        &mut book,
        &Limits::default(),
        &mut fake,
        true,
    ));
    assert!(fake.calls.is_empty(), "{:?}", fake.calls);
    assert_eq!(book.children.len(), 1);
    assert_eq!(notes.len(), 5, "{notes:?}");
    assert!(
        notes.iter().all(|note| note.contains("would ")),
        "{notes:?}"
    );
}

fn supervised(book: &mut Book, fake: &mut Fake) -> Vec<String> {
    supervise(
        &[change(1, "main")],
        book,
        &Limits::default(),
        now(),
        fake,
        false,
    )
}

#[test]
fn a_reviewer_past_the_limit_is_stopped_and_counted_only_without_a_verdict() {
    let late = || Book {
        children: vec![child(Kind::Review, 1, 46)],
        ..Book::default()
    };
    // No verdict: stopped, and the head has one timeout against it.
    let (mut book, mut fake) = (late(), Fake::default());
    supervised(&mut book, &mut fake);
    assert_eq!(fake.calls, ["verdict? 1", "stop 1"]);
    assert!(book.children.is_empty());
    assert_eq!(book.timeouts(&change(1, "main")), 1);
    // A second reviewer of the same head times out: two, which derives
    // BLOCKED instead of a third launch.
    book.children.push(child(Kind::Review, 1, 46));
    supervised(&mut book, &mut Fake::default());
    assert_eq!(book.timeouts(&change(1, "main")), MAX_REVIEW_TIMEOUTS);

    // The verdict is recorded: the reviewer is only slow to exit. It is
    // stopped and nothing counts against the head.
    let (mut book, mut fake) = (late(), Fake::default());
    fake.verdict = Some(Ok(vec![3]));
    supervised(&mut book, &mut fake);
    assert_eq!(fake.calls, ["verdict? 1", "stop 1"]);
    assert_eq!(book.timeouts(&change(1, "main")), 0);

    // Whether there is a verdict cannot be read: nothing is stopped.
    let (mut book, mut fake) = (late(), Fake::default());
    fake.verdict = Some(Err("the ledger is locked".into()));
    let notes = supervised(&mut book, &mut fake);
    assert_eq!(fake.calls, ["verdict? 1"]);
    assert_eq!(book.children.len(), 1);
    assert!(notes[0].contains("could not be read"), "{notes:?}");
}

#[test]
fn a_verdict_older_than_the_reviewer_does_not_excuse_its_timeout() {
    // The head already carries a verdict from before this reviewer started
    // (a re-review of the same head). It is not this reviewer's work.
    let mut book = Book {
        children: vec![child(Kind::Review, 1, 46)],
        ..Book::default()
    };
    let mut fake = Fake {
        verdict: Some(Ok(vec![120])),
        ..Fake::default()
    };
    let notes = supervised(&mut book, &mut fake);
    assert_eq!(fake.calls, ["verdict? 1", "stop 1"]);
    assert!(notes[0].contains("no verdict after 46 min"), "{notes:?}");
    assert_eq!(book.timeouts(&change(1, "main")), 1);
    // The relaunched reviewer times out the same way: the head reaches the
    // limit that derives BLOCKED, where a counted-as-done stop never would.
    book.children.push(child(Kind::Review, 1, 46));
    supervised(&mut book, &mut fake);
    assert_eq!(book.timeouts(&change(1, "main")), MAX_REVIEW_TIMEOUTS);

    // A verdict recorded the moment the reviewer started is its own.
    let mut book = Book {
        children: vec![child(Kind::Review, 1, 46)],
        ..Book::default()
    };
    fake.verdict = Some(Ok(vec![120, 46]));
    supervised(&mut book, &mut fake);
    assert_eq!(book.timeouts(&change(1, "main")), 0);
}

#[test]
fn a_live_reviewer_of_ours_counts_against_the_cap_once_per_change() {
    let changes: Vec<Pr> = (1..=4).map(|id| change(id, "main")).collect();
    // Our reviewer of #1 is alive but its row does not read as in review
    // (its lease was not seen); #2 reads as in review and is also ours.
    let rows = vec![
        row(1, State::NeedsReview, Action::Wait),
        row(2, State::Reviewing, Action::Wait),
        row(3, State::NeedsReview, Action::LaunchReview),
    ];
    let mut book = Book {
        children: vec![child(Kind::Review, 1, 5), child(Kind::Review, 2, 5)],
        ..Book::default()
    };
    let mut fake = Fake::default();
    let notes = acted(&changes, &rows, &mut book, &mut fake);
    assert!(fake.calls.is_empty(), "{:?}", fake.calls);
    assert_eq!(notes, ["#3: review waits: 2 of 2 reviews are running"]);

    // One change in review and ours at once is one review, not two.
    let rows = vec![
        row(2, State::Reviewing, Action::Wait),
        row(3, State::NeedsReview, Action::LaunchReview),
        row(4, State::NeedsReview, Action::LaunchReview),
    ];
    let mut book = Book {
        children: vec![child(Kind::Review, 2, 5)],
        ..Book::default()
    };
    let mut fake = Fake::default();
    let notes = acted(&changes, &rows, &mut book, &mut fake);
    assert_eq!(fake.calls, ["review 3 tree3"]);
    assert_eq!(
        notes.last().unwrap(),
        "#4: review waits: 2 of 2 reviews are running"
    );
}

#[test]
fn a_log_starts_with_what_the_child_was_launched_against() {
    let header = log_header(
        now(),
        &[7],
        &[("head", HEAD), ("base", "main"), ("merge tree", "tree7")],
        &strings(&["queue", "work", "PR-7", "--allow-stale-base"]),
    );
    assert_eq!(
        header,
        format!(
            "# aida land run launched this at {NOW}\n# changes: #7\n# head: {HEAD}\n# base: main\n# merge tree: tree7\n# command: aida queue work PR-7 --allow-stale-base\n"
        )
    );
}

#[test]
fn a_reviewer_inside_the_limit_is_left_alone_and_one_of_a_moved_head_is_stopped() {
    let mut book = Book {
        children: vec![child(Kind::Review, 1, 44)],
        ..Book::default()
    };
    let mut fake = Fake::default();
    supervised(&mut book, &mut fake);
    assert!(fake.calls.is_empty(), "{:?}", fake.calls);
    assert_eq!(book.children.len(), 1);

    let moved = Pr {
        head: "b".repeat(40),
        ..change(1, "main")
    };
    let notes = supervise(
        &[moved],
        &mut book,
        &Limits::default(),
        now(),
        &mut fake,
        false,
    );
    assert_eq!(fake.calls, ["stop 1"]);
    assert!(book.children.is_empty());
    assert!(notes[0].contains("the head moved"), "{notes:?}");
}

#[test]
fn a_train_is_never_stopped_however_long_it_runs() {
    let mut book = Book {
        children: vec![Child {
            prs: vec![1, 2],
            ..child(Kind::Train, 1, 300)
        }],
        ..Book::default()
    };
    let mut fake = Fake::default();
    let notes = supervised(&mut book, &mut fake);
    assert!(fake.calls.is_empty(), "{:?}", fake.calls);
    assert_eq!(book.children.len(), 1);
    assert!(notes[0].contains("left running"), "{notes:?}");
}

#[test]
fn settling_drops_dead_children_and_keeps_what_a_refusing_lane_said() {
    let changes = vec![change(1, "main"), change(2, "main")];
    let mut book = Book {
        children: vec![
            child(Kind::Review, 1, 5),
            child(Kind::Lane, 1, 5),
            child(Kind::Lane, 2, 5),
        ],
        timeouts: vec![
            Mark {
                pr: 1,
                head: HEAD.into(),
                count: 1,
                reason: String::new(),
            },
            // Recorded against a head the change no longer has.
            Mark {
                pr: 2,
                head: "b".repeat(40),
                count: 2,
                reason: String::new(),
            },
        ],
        stops: Vec::new(),
    };
    // The reviewer is alive; both lanes are dead, and the first one's log
    // ends with the seat refusal.
    let notes = book.settle(&changes, &|child| child.kind == Kind::Review, &|child| {
        if child.prs == [1] {
            "claiming STORY-1\nrefused: its seat has 3 commit(s) that no remote has\n".into()
        } else {
            "exited".into()
        }
    });
    assert_eq!(book.children.len(), 1);
    assert_eq!(book.children[0].kind, Kind::Review);
    assert!(book.stop(&changes[0]).unwrap().contains("3 commit(s)"));
    assert_eq!(book.stop(&changes[1]), None);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(book.timeouts(&changes[0]), 1);
    assert_eq!(book.timeouts.len(), 1);
    // The stop goes when the head moves: a new head may be reworked.
    let moved = Pr {
        head: "c".repeat(40),
        ..change(1, "main")
    };
    book.settle(&[moved.clone()], &|_| true, &|_| String::new());
    assert_eq!(book.stop(&moved), None);
}

#[test]
fn a_record_that_cannot_be_parsed_is_an_error_and_a_missing_one_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(Book::read(dir.path()).unwrap(), Book::default());
    let book = Book {
        children: vec![child(Kind::Review, 1, 5)],
        ..Book::default()
    };
    book.write(dir.path()).unwrap();
    assert_eq!(Book::read(dir.path()).unwrap(), book);
    std::fs::write(dir.path().join(BOOK), "{ not json").unwrap();
    assert!(Book::read(dir.path()).is_err());
}

#[test]
fn the_record_finds_a_reviewer_by_head_and_a_lane_by_change() {
    let book = Book {
        children: vec![child(Kind::Review, 1, 5), child(Kind::Lane, 2, 5)],
        ..Book::default()
    };
    let moved = |id: u64| Pr {
        head: "b".repeat(40),
        ..change(id, "main")
    };
    assert!(book.reviewer(&change(1, "main")).is_some());
    // A reviewer of an older head is not a review of this one.
    assert!(book.reviewer(&moved(1)).is_none());
    // A lane is what moves the head, so it still counts.
    assert!(book.lane(&moved(2)).is_some());
    assert!(book.lane(&change(1, "main")).is_none());
}
