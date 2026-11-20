// trace:TASK-3827 | ai:claude
//
// The foundation the later throughput-guard classes build on: lanes on other
// hosts read from those hosts' own records, the observed cause of a head
// nobody reviews, where an SSH destination may come from, thresholds that
// cannot overflow, and routed text that is one clean line.
use super::*;
use crate::coordination::Claim;
use crate::land::Pr;
use crate::lane_hosts::{HostCount, View};
use std::cell::RefCell;

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-20T21:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn ago(mins: i64) -> DateTime<Utc> {
    now() - Duration::minutes(mins)
}

fn change(number: u64) -> Pr {
    Pr {
        id: number,
        head: format!("{number:040x}"),
        base: "main".into(),
        branch: Some(format!("task-{number}-work")),
        title: format!("change {number} (TASK-{number})"),
        updated_at: None,
        hold_label: None,
        ci: None,
    }
}

/// A change that has waited 65 minutes where `standing` says.
fn fact(number: u64, standing: Standing) -> PrFact {
    let pr = change(number);
    PrFact {
        number,
        title: pr.title,
        head: pr.head,
        state: State::NeedsReview,
        action: Action::Wait,
        reason: String::new(),
        standing,
        since: Some(ago(65)),
        red: None,
        foreign_lane: None,
        review_cause: None,
    }
}

fn rework() -> Standing {
    Standing::ChangesRequested {
        lane: None,
        round: 2,
        red: false,
    }
}

fn unreviewed(red: Option<&str>) -> Standing {
    Standing::Unreviewed {
        refusal: "no approval covers the head".into(),
        red: red.map(str::to_string),
        lane: None,
    }
}

/// The record a lane's launcher keeps on the shared store.
fn lane_record(host: &str, scope: &str, reported_mins_ago: i64) -> Claim {
    let at = ago(reported_mins_ago).to_rfc3339();
    serde_json::from_value(serde_json::json!({
        "scope": scope, "node_id": "node-two", "lease_id": format!("lease-{scope}"),
        "clone_path": "/lanes/clone", "host": host, "pid": 1, "agent": "implementer",
        "started_at": at, "heartbeat_at": at, "ttl_secs": 1800
    }))
    .unwrap()
}

/// A shared store holding `records`, as a host that runs none of them reads it.
fn view_of(records: &[Claim]) -> View {
    let store = tempfile::tempdir().unwrap();
    let dir = crate::coordination::leases_dir(store.path());
    std::fs::create_dir_all(&dir).unwrap();
    for (n, record) in records.iter().enumerate() {
        std::fs::write(
            dir.join(format!("record-{n}.toml")),
            toml::to_string(record).unwrap(),
        )
        .unwrap();
    }
    let view = crate::lane_hosts::read_store(store.path(), "imac", now());
    assert!(view.unreadable.is_empty(), "{:?}", view.unreadable);
    view
}

fn no_review_evidence() -> review_cause::Evidence {
    review_cause::Evidence {
        pass: Some(Err(
            "no review-watch log at .aida/review-watch/service.log".into()
        )),
        leases: Some(Ok(Vec::new())),
        worktrees: Some(Ok(Vec::new())),
        ..review_cause::Evidence::default()
    }
}

/// `standing` for change `number`, with the lanes other hosts have on record.
fn observed(number: u64, standing: Standing, records: &[Claim]) -> PrFact {
    let mut prs = vec![fact(number, standing)];
    attach_evidence(
        &mut prs,
        &[change(number)],
        &view_of(records),
        &no_review_evidence(),
        now(),
    );
    prs.remove(0)
}

fn classified(fact: &PrFact) -> Option<Item> {
    classify_pr(fact, &Policy::default(), now())
}

// The live evidence of 2026-11-20: #4376 was called STUCK-REWORK "no lane or
// session on it for 65 min" while its rework lane ran on the other host.
#[test]
fn a_change_whose_spec_a_reporting_lane_on_another_host_holds_is_not_stuck() {
    // Without any lane it is stuck, in each of the four standings.
    let conflict = Standing::Conflict { owner: None };
    let approved_red = Standing::Approved {
        approval: "approved".into(),
        red: Some("Test (full)".into()),
        hold: None,
        lane: None,
        stacked: false,
    };
    let standings = [
        (rework(), Class::StuckRework),
        (unreviewed(None), Class::StuckReview),
        (unreviewed(Some("Test (full)")), Class::StuckReview),
        (conflict, Class::StuckConflict),
    ];
    for (standing, class) in &standings {
        let alone = observed(7, standing.clone(), &[]);
        assert_eq!(alone.foreign_lane, None);
        assert_eq!(classified(&alone).map(|item| item.class), Some(*class));

        // The lane on dell1 reported four minutes ago: held by the spec id,
        // and by the branch.
        for scope in ["TASK-7", "task-7"] {
            let held = observed(
                7,
                standing.clone(),
                &[
                    lane_record("dell1", scope, 4),
                    lane_record("dell1", "TASK-70", 1),
                ],
            );
            let lane = held.foreign_lane.clone().expect("the lane holds it");
            assert_eq!(
                (lane.host.as_str(), lane.reported_mins_ago, lane.reporting),
                ("dell1", Some(4), true)
            );
            assert_eq!(classified(&held), None, "{standing:?} held through {scope}");
        }
        // A lane on another spec holds nothing here.
        let other = observed(7, standing.clone(), &[lane_record("dell1", "TASK-70", 1)]);
        assert_eq!(classified(&other).map(|item| item.class), Some(*class));
    }
    let red = observed(7, approved_red.clone(), &[]);
    assert_eq!(
        classified(&red).map(|i| i.class),
        Some(Class::StuckMergeRed)
    );
    let red = observed(7, approved_red, &[lane_record("dell1", "TASK-7", 4)]);
    assert_eq!(classified(&red), None);

    // An approved, green head that does not land is stuck whoever holds its
    // spec: a lane is not what it waits for.
    let approved = Standing::Approved {
        approval: "approved".into(),
        red: None,
        hold: None,
        lane: None,
        stacked: false,
    };
    let waiting = observed(7, approved, &[lane_record("dell1", "TASK-7", 4)]);
    assert_eq!(
        classified(&waiting).map(|item| item.class),
        Some(Class::StuckMerge)
    );

    // In the report the held change reads as moving.
    let held = observed(7, rework(), &[lane_record("dell1", "TASK-7", 4)]);
    let report = build_report(
        &Facts {
            prs: vec![held],
            ..Facts::default()
        },
        &Policy::default(),
        &mut Memory::default(),
        now(),
    );
    assert!(report.stuck.is_empty());
    assert_eq!(report.changes[0].class, Class::Moving);
}

fn report_of(prs: Vec<PrFact>) -> Report {
    build_report(
        &Facts {
            prs,
            ..Facts::default()
        },
        &Policy::default(),
        &mut Memory::default(),
        now(),
    )
}

// A lane's launcher writes its claim once. After the claim's 30-minute
// expiry a running lane has no heartbeat to show, so a 45-minute-old claim
// stands for a lane that may be live: rework of its change is not routed,
// the report says the lane's state is unknown, and the host's line counts
// it as such instead of showing no lane.
#[test]
fn a_claim_past_its_expiry_with_no_heartbeat_is_unknown_and_never_routes_rework() {
    let old_claim = [lane_record("dell1", "TASK-7", 45)];
    let held = observed(7, rework(), &old_claim);
    let lane = held.foreign_lane.clone().expect("on record");
    assert_eq!(
        (lane.reporting, lane.reported_mins_ago, lane.invalid.clone()),
        (false, Some(45), None)
    );
    assert_eq!(classified(&held), None, "rework is not routed");
    let report = report_of(vec![held.clone()]);
    assert!(report.stuck.is_empty());
    assert_eq!(
        report.unknown,
        ["#7: changes are requested, for 65 min, and rework is not routed: a lane on host dell1 (lease-TASK-7) may be live: no heartbeat for 45 min, past its expiry, so whether it runs is unknown"]
    );
    // Inside the rework threshold there is nothing to say yet.
    let mut recent = held.clone();
    recent.since = Some(ago(10));
    assert!(report_of(vec![recent]).unknown.is_empty());
    // Nothing is routed for it, however long the change has waited.
    let mut waited = held.clone();
    waited.since = Some(ago(20 * 60));
    assert_eq!(classified(&waited), None);
    let plan = plan_routes(
        &report_of(vec![waited]),
        &Memory::default(),
        &Policy::default(),
        now(),
    );
    assert!(plan.seats.is_empty());
    // With CI red on the requested changes too.
    let red = Standing::ChangesRequested {
        lane: None,
        round: 3,
        red: true,
    };
    assert_eq!(classified(&observed(7, red, &old_claim)), None);

    // The host's line: no live lane is claimed, and none is denied.
    let others = view_of(&old_claim).hosts(now(), span(SILENT_LANE_HORIZON_HOURS * 60));
    let lines = host_lines_with(
        "imac",
        0,
        &others,
        &Policy::default(),
        &|| "unknown".to_string(),
        &|_| Err("not probed".to_string()),
    );
    let dell1 = lines.iter().find(|line| line.host == "dell1").unwrap();
    assert_eq!(
        dell1.lanes_text(),
        "0 lane(s) by the host's own records, newest report 45 min ago, and 1 more that may be live: no heartbeat since their expiry (state unknown)"
    );

    // The other stalls are still reported, and say the same about the lane.
    let item = classified(&observed(7, unreviewed(None), &old_claim)).expect("stuck");
    assert_eq!(item.class, Class::StuckReview);
    let said = "; a lane on host dell1 (lease-TASK-7) may be live: no heartbeat for 45 min, past its expiry, so whether it runs is unknown";
    assert!(
        item.summary
            .contains("no verdict and no reviewer for 65 min")
            && item.summary.ends_with(said),
        "{}",
        item.summary
    );
    let item = classified(&observed(7, unreviewed(Some("Test")), &old_claim)).expect("stuck");
    assert!(
        item.summary
            .contains("no reviewer and no lane known to be live for 65 min")
            && item.summary.ends_with(said),
        "{}",
        item.summary
    );
    let item =
        classified(&observed(7, Standing::Conflict { owner: None }, &old_claim)).expect("stuck");
    assert_eq!(item.class, Class::StuckConflict);
    assert!(item.summary.ends_with(said), "{}", item.summary);
    // An approved red head: with and without its CI detail read.
    let approved_red = Standing::Approved {
        approval: "approved".into(),
        red: Some("Test (full)".into()),
        hold: None,
        lane: None,
        stacked: false,
    };
    let mut red = observed(7, approved_red, &old_claim);
    let item = classified(&red).expect("stuck");
    assert_eq!(item.class, Class::StuckMergeRed);
    assert_eq!(
        item.summary,
        format!("#7 is approved at 00000000 but its CI is red (Test (full)), for 65 min; the cause was not examined{said}")
    );
    red.red = Some(RedFact {
        checks: vec![FailedCheck {
            name: "Test (full)".into(),
            id: 1,
            tests: Ok(vec!["queue::tests::drains".into()]),
            base: BaseCheck::Pass,
        }],
        changed_paths: Ok(vec!["docs/readme.md".into()]),
        rerun_routed: None,
        unread: None,
    });
    let item = classified(&red).expect("stuck");
    assert!(
        item.key.ends_with(":rerun") && item.summary.contains("probable flake"),
        "{item:?}"
    );
    assert!(item.summary.ends_with(said), "{}", item.summary);

    // A reporting lane on one host outranks an expired record on another.
    let both = observed(
        7,
        rework(),
        &[
            lane_record("dell1", "TASK-7", 45),
            lane_record("dell2", "TASK-7", 2),
        ],
    );
    assert_eq!(classified(&both), None);
    assert!(report_of(vec![both]).unknown.is_empty());

    // A record silent for longer than a day is a leftover: it holds nothing,
    // and rework is routed in the plain wording.
    let leftover = observed(7, rework(), &[lane_record("dell1", "TASK-7", 3 * 24 * 60)]);
    assert_eq!(leftover.foreign_lane, None);
    let item = classified(&leftover).expect("stuck");
    assert_eq!(item.class, Class::StuckRework);
    assert!(
        item.summary
            .ends_with("no lane or session on it for 65 min"),
        "{}",
        item.summary
    );
}

// A record of another host that parses but is not a usable report (no
// usable heartbeat, a heartbeat far ahead, an expiry no date holds) is not
// dropped: the same reading names its file, counts it on its host, and makes
// the lane on its spec unknown. A host never shows "no lane" while its
// record decides a stall.
#[test]
fn an_invalid_record_is_unknown_for_the_stall_and_for_its_hosts_count_alike() {
    let mut garbled = lane_record("dell1", "TASK-7", 4);
    garbled.heartbeat_at = "not a time".into();
    let mut future = lane_record("dell1", "TASK-8", 4);
    future.heartbeat_at = (now() + Duration::hours(3)).to_rfc3339();
    // The largest expiry a TOML record can carry.
    let mut endless = lane_record("dell2", "TASK-9", 4);
    endless.ttl_secs = i64::MAX as u64;
    let mut hostless = lane_record("", "TASK-10", 4);
    hostless.lease_id = None;
    let store = tempfile::tempdir().unwrap();
    let dir = crate::coordination::leases_dir(store.path());
    std::fs::create_dir_all(&dir).unwrap();
    for (name, record) in [
        ("garbled.toml", &garbled),
        ("future.toml", &future),
        ("endless.toml", &endless),
        ("hostless.toml", &hostless),
    ] {
        std::fs::write(dir.join(name), toml::to_string(record).unwrap()).unwrap();
    }
    let view = crate::lane_hosts::read_store(store.path(), "imac", now());
    assert_eq!((view.lanes.len(), view.invalid.len()), (0, 4));
    assert_eq!(view.unreadable.len(), 1);
    for name in [
        "garbled.toml",
        "future.toml",
        "endless.toml",
        "hostless.toml",
    ] {
        assert!(view.unreadable[0].contains(name), "{}", view.unreadable[0]);
    }

    // Ownership and the count are answered by the one reading.
    let mut prs: Vec<PrFact> = (7..=11).map(|n| fact(n, rework())).collect();
    let changes: Vec<Pr> = (7..=11).map(change).collect();
    attach_evidence(&mut prs, &changes, &view, &no_review_evidence(), now());
    for fact in &prs[..4] {
        let lane = fact.foreign_lane.as_ref().expect("held as unknown");
        assert!(!lane.reporting && lane.invalid.is_some(), "{lane:?}");
        assert_eq!(classified(fact), None, "#{} is not routed", fact.number);
    }
    assert_eq!(
        classified(&prs[4]).map(|item| item.class),
        Some(Class::StuckRework)
    );
    let report = report_of(prs);
    assert_eq!(report.stuck.len(), 1);
    assert_eq!(
        report.unknown[0],
        "#7: changes are requested, for 65 min, and rework is not routed: a lane on host dell1 (lease-TASK-7) may be live: its record is not a usable report (garbled.toml: its heartbeat is not a time), so whether it runs is unknown"
    );
    assert!(
        report.unknown[3].contains("a lane on an unnamed host (no lease id) may be live")
            && report.unknown[3].contains("hostless.toml: it names no host"),
        "{}",
        report.unknown[3]
    );
    let others = view.hosts(now(), span(SILENT_LANE_HORIZON_HOURS * 60));
    assert_eq!(others.len(), 2, "{others:?}");
    let lines = host_lines_with(
        "imac",
        0,
        &others,
        &Policy::default(),
        &|| "unknown".to_string(),
        &|_| Err("not probed".to_string()),
    );
    let line = |host: &str| lines.iter().find(|line| line.host == host).unwrap();
    assert_eq!(
        line("dell1").lanes_text(),
        "0 lane(s), and 2 record(s) that are not a usable report (state unknown)"
    );
    assert_eq!(line("dell1").invalid_lanes, 2);
    assert_eq!(line("dell2").invalid_lanes, 1);
    // An unreviewed head says the same in its stall.
    let mut unreviewed_prs = vec![fact(7, unreviewed(None))];
    attach_evidence(
        &mut unreviewed_prs,
        &[change(7)],
        &view,
        &no_review_evidence(),
        now(),
    );
    let item = classified(&unreviewed_prs[0]).expect("stuck");
    assert!(
        item.summary
            .contains("its record is not a usable report (garbled.toml"),
        "{}",
        item.summary
    );
}

// A claim covers a change by its whole spec ID or branch, never by a prefix:
// a live lane on TASK-7 does not hide the unreviewed head of task-70-work,
// and a lane on TASK-70 does not hold task-7-work.
#[test]
fn a_lane_on_a_spec_whose_id_is_a_prefix_of_anothers_holds_only_its_own_change() {
    for (number, other) in [
        (70u64, "TASK-7"),
        (7, "TASK-70"),
        (70, "task-7"),
        (7, "task-70"),
    ] {
        let fact = observed(number, unreviewed(None), &[lane_record("dell1", other, 2)]);
        assert_eq!(fact.foreign_lane, None, "{other} does not hold #{number}");
        assert_eq!(
            classified(&fact).map(|item| item.class),
            Some(Class::StuckReview),
            "#{number} with a live lane on {other}"
        );
    }
    for (number, own) in [
        (70u64, "TASK-70"),
        (70, "task-70"),
        (70, "task-70-work"),
        (7, "TASK-7"),
    ] {
        let fact = observed(number, unreviewed(None), &[lane_record("dell1", own, 2)]);
        assert!(fact.foreign_lane.is_some(), "{own} holds #{number}");
        assert_eq!(classified(&fact), None);
    }
    // A change whose title names no spec is held through its branch alone.
    let mut untitled = change(70);
    untitled.title = "a change".into();
    for (scope, holds) in [("TASK-7", false), ("TASK-70", true)] {
        let mut prs = vec![fact(70, unreviewed(None))];
        attach_evidence(
            &mut prs,
            std::slice::from_ref(&untitled),
            &view_of(&[lane_record("dell1", scope, 2)]),
            &no_review_evidence(),
            now(),
        );
        assert_eq!(prs[0].foreign_lane.is_some(), holds, "{scope}");
    }
}

/// Evidence in which the reconciler's lane (any host's claim) and the lane
/// this host can vouch for differ.
struct ForeignClaimOnly;

impl crate::land::Evidence for ForeignClaimOnly {
    fn conflict(&self, _: &Pr) -> std::result::Result<bool, String> {
        Ok(false)
    }
    fn reviewing(&self, _: &Pr) -> std::result::Result<Option<String>, String> {
        Ok(None)
    }
    fn ci(&self, _: &Pr) -> std::result::Result<Ci, String> {
        Ok(Ci::Green)
    }
    fn head_red(&self, _: &Pr) -> std::result::Result<Option<String>, String> {
        Ok(None)
    }
    fn approval(&self, _: &Pr) -> std::result::Result<Approval, String> {
        Ok(Approval::Refused("changes requested".into()))
    }
    fn hold(&self, _: &Pr) -> std::result::Result<Option<Hold>, String> {
        Ok(None)
    }
    fn rounds(&self, _: &Pr) -> std::result::Result<crate::land::Rounds, String> {
        Ok(crate::land::Rounds {
            at_content: true,
            content_rounds: 1,
            since: Some(ago(65).to_rfc3339()),
        })
    }
    fn lane(&self, _: &Pr) -> std::result::Result<Option<String>, String> {
        Ok(Some("implementer@dell1".into()))
    }
    fn lane_on_this_host(&self, _: &Pr) -> std::result::Result<Option<String>, String> {
        Ok(None)
    }
    fn head_at(&self, _: &Pr) -> Option<String> {
        Some(ago(65).to_rfc3339())
    }
    fn approved_at(&self, _: &Pr) -> Option<String> {
        None
    }
    fn conflict_since(&self, _: &Pr) -> Option<String> {
        None
    }
}

// Another host's claim never enters a change's standing through the
// reconciler's own partition of the claims (which keeps a claim whose
// heartbeat it cannot read): what such a record means is decided by the one
// reading of the lane records.
#[test]
fn another_hosts_claim_reaches_the_standing_only_through_the_lane_records() {
    let pr = change(7);
    let row = crate::land::derive(&pr, &ForeignClaimOnly);
    let fact = pr_fact(&pr, &row, &ForeignClaimOnly);
    assert!(
        matches!(fact.standing, Standing::ChangesRequested { lane: None, .. }),
        "{:?}",
        fact.standing
    );
    // No record of it at all: it is stuck. A usable, fresh record: moving.
    assert_eq!(
        classified(&fact).map(|item| item.class),
        Some(Class::StuckRework)
    );
    let mut prs = vec![fact];
    attach_evidence(
        &mut prs,
        &[pr],
        &view_of(&[lane_record("dell1", "TASK-7", 3)]),
        &no_review_evidence(),
        now(),
    );
    assert_eq!(classified(&prs[0]), None);
}

// The guard's reads of shared records never block and never follow a link:
// a FIFO, a symlink or a directory where a claim, a session lease or the
// guard's own state is expected is skipped or refused at once.
#[cfg(unix)]
#[test]
fn the_guards_reads_of_shared_records_do_not_block_on_a_fifo() {
    let fifo = |path: &std::path::Path| {
        let c = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    };
    let (done, finished) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        // The shared claims, as the reconciler's pass lists them.
        let store = root.join(".aida-store");
        let claims = crate::coordination::leases_dir(&store);
        std::fs::create_dir_all(&claims).unwrap();
        std::fs::write(
            claims.join("good.toml"),
            toml::to_string(&lane_record("dell1", "TASK-7", 4)).unwrap(),
        )
        .unwrap();
        fifo(&claims.join("pipe.toml"));
        std::os::unix::fs::symlink(claims.join("good.toml"), claims.join("link.toml")).unwrap();
        std::fs::create_dir(claims.join("dir.toml")).unwrap();
        let listed = crate::coordination::list_claims(&store);
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(listed[0].scope, "TASK-7");
        // The same directory through the lane records: each is named.
        let view = crate::lane_hosts::read_store(&store, "imac", now());
        assert_eq!(view.lanes.len(), 1);
        for name in ["pipe.toml", "link.toml", "dir.toml"] {
            assert!(view.unreadable[0].contains(name), "{:?}", view.unreadable);
        }
        // This clone's session leases: a FIFO is an unreadable lease, and
        // the strict listing says so instead of waiting on it.
        let sessions = crate::leases::leases_dir(root);
        std::fs::create_dir_all(&sessions).unwrap();
        let count = |root: &std::path::Path| {
            crate::leases::list_leases_strict_all(root).map(|leases| leases.len())
        };
        assert_eq!(count(root), Ok(0));
        fifo(&sessions.join("0f0f0f0f-pipe.toml"));
        assert!(count(root).is_err(), "{:?}", count(root));
        let evidence = review_cause::Evidence::read(root, false);
        assert!(
            matches!(&evidence.leases, Some(Err(_))),
            "{:?}",
            evidence.leases
        );
        assert!(evidence.cause(7, now()).starts_with("unknown ("));
        // The guard's own state.
        let state = memory_path(root);
        std::fs::create_dir_all(state.parent().unwrap()).unwrap();
        fifo(&state);
        assert_eq!(load_memory(root), Memory::default());
        done.send(()).unwrap();
    });
    let outcome = finished.recv_timeout(std::time::Duration::from_secs(30));
    assert!(
        outcome.is_ok(),
        "a read blocked or a check failed: {outcome:?}"
    );
    worker.join().unwrap();
}

// The live evidence of 2026-11-20: "2 lane(s)" was shown while seven lanes
// ran on the other host. The count now comes from the host's own records and
// carries the age of the newest one.
#[test]
fn another_hosts_lanes_are_counted_from_its_own_records_with_their_age() {
    let mut records: Vec<Claim> = [2i64, 3, 5, 8, 13, 21, 29]
        .iter()
        .enumerate()
        .map(|(n, mins)| lane_record("dell1", &format!("TASK-{n}"), *mins))
        .collect();
    records.push(lane_record("dell1", "TASK-90", 65));
    records.push(lane_record("dell1", "TASK-91", 40 * 60));
    records.push(lane_record("dell3", "TASK-92", 200));
    let view = view_of(&records);
    let others = view.hosts(now(), span(SILENT_LANE_HORIZON_HOURS * 60));
    let policy = Policy {
        remote_hosts: BTreeMap::from([
            ("dell1".to_string(), "dell1-alias".to_string()),
            ("quiet-host".to_string(), "quiet".to_string()),
        ]),
        ..Policy::default()
    };
    let probed = RefCell::new(Vec::new());
    let probe = |alias: &str| -> std::result::Result<String, String> {
        probed.borrow_mut().push(alias.to_string());
        match alias {
            "dell1-alias" => {
                Ok("MemTotal: 16777216 kB\nMemFree: 1000 kB\nMemAvailable: 3145728 kB\n".into())
            }
            _ => Err("ssh exited with exit status: 255\nsecond \u{1b}[31mline".into()),
        }
    };
    let local = || "20.0 GiB available of 64.0 GiB".to_string();
    let lines = host_lines_with("imac", 2, &others, &policy, &local, &probe);
    let line = |host: &str| lines.iter().find(|line| line.host == host).unwrap();
    assert_eq!(lines.len(), 4);

    let dell1 = line("dell1");
    assert_eq!(
        (dell1.lanes, dell1.silent_lanes, dell1.reported_mins_ago),
        (7, 1, Some(2))
    );
    assert_eq!(
        dell1.lanes_text(),
        "7 lane(s) by the host's own records, newest report 2 min ago, and 1 more that may be live: no heartbeat since their expiry (state unknown)"
    );
    assert_eq!(dell1.memory, "3.0 GiB available of 16.0 GiB");
    // This host's lanes are counted here, not read from a record.
    assert_eq!(line("imac").lanes_text(), "2 lane(s), counted here now");
    assert_eq!(line("imac").memory, "20.0 GiB available of 64.0 GiB");
    // A host with only a silent record: no live lane is claimed for it.
    assert_eq!(
        line("dell3").lanes_text(),
        "0 lane(s) by the host's own records, newest report 200 min ago, and 1 more that may be live: no heartbeat since their expiry (state unknown)"
    );
    assert_eq!(line("dell3").memory, NO_REMOTE_MEMORY);
    // A named host with nothing on record is listed and measured.
    assert_eq!(line("quiet-host").lanes_text(), "no lane on record");
    assert_eq!(
        line("quiet-host").memory,
        "unknown (the probe of `quiet` failed: ssh exited with exit status: 255 second [31mline)"
    );
    // Only the named hosts are contacted.
    let mut asked = probed.borrow().clone();
    asked.sort();
    assert_eq!(asked, ["dell1-alias", "quiet"]);

    let report = Report {
        hosts: lines,
        ..build_report(
            &Facts::default(),
            &Policy::default(),
            &mut Memory::default(),
            now(),
        )
    };
    let text = render(&report);
    assert!(
        text.contains(
            "Host dell1: 7 lane(s) by the host's own records, newest report 2 min ago, and 1 more"
        ),
        "{text}"
    );
    // The saved observation of a scheduled run keeps the age and reads back.
    let mut memory = Memory::default();
    record_run(&mut memory, &report);
    let saved: Memory = serde_json::from_str(&serde_json::to_string(&memory).unwrap()).unwrap();
    assert_eq!(saved, memory);
    assert!(snapshot_line(&saved.runs[0]).contains("newest report 2 min ago"));
    // A snapshot saved before the age was kept still reads.
    let old: HostLine =
        serde_json::from_str(r#"{"host":"dell1","lanes":2,"memory":"unknown"}"#).unwrap();
    assert_eq!(old.lanes_text(), "2 lane(s)");
}

// STUCK-REVIEW names what this host can observe about why nobody reviews
// the head, and says "unknown" when it observed nothing.
#[test]
fn a_head_nobody_reviews_names_the_observed_cause_or_says_it_is_unknown() {
    let trees = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(trees.path().join("aida-pr-8")).unwrap();
    let lease = |scope: &str, id: &str, liveness| review_cause::LeaseSeen {
        scope: scope.into(),
        id: id.into(),
        branch: String::new(),
        worktree: std::path::PathBuf::new(),
        liveness,
    };
    let pass = review_cause::Pass {
        at: ago(1),
        lines: vec![
            "PR-7: watcher admission is busy".into(),
            "PR-70: refused: something else".into(),
        ],
    };
    let evidence = review_cause::Evidence {
        pass: Some(Ok(pass)),
        tree_roots: vec![trees.path().to_path_buf()],
        leases: Some(Ok(vec![
            lease("PR-9", "lease-9", review_cause::Liveness::NotLive),
            lease("PR-10", "lease-10", review_cause::Liveness::Held),
        ])),
        worktrees: Some(Ok(vec![(trees.path().join("aida-pr-8"), "pr-8".into())])),
        ..review_cause::Evidence::default()
    };
    let mut prs = vec![
        fact(7, unreviewed(None)),
        fact(8, unreviewed(Some("Test (full)"))),
        fact(9, unreviewed(None)),
        fact(10, unreviewed(None)),
        fact(11, rework()),
    ];
    let changes: Vec<Pr> = (7..=11).map(change).collect();
    attach_evidence(&mut prs, &changes, &View::default(), &evidence, now());
    let summary = |n: usize| classified(&prs[n]).expect("stuck").summary;
    assert_eq!(
        summary(0),
        "#7 at 00000000 has had no verdict and no reviewer for 65 min; cause: watcher admission is busy (review-watch log, pass of 20:59 UTC)"
    );
    let tree = summary(1);
    assert!(
        tree.contains("its CI is red (Test (full)); cause: a reviewer worktree from an earlier review is still at ")
            && tree.ends_with("aida-pr-8 (branch pr-8, with no live session or agent process in it)"),
        "{tree}"
    );
    assert!(
        summary(2).ends_with(
            "; cause: lease lease-9 on PR-9 is still recorded and its session is not live"
        ),
        "{}",
        summary(2)
    );
    assert!(
        summary(3).ends_with("; cause: unknown (the review-watch log names no cause for it, and no reviewer worktree or lease is left from an earlier review)"),
        "{}",
        summary(3)
    );
    // Only a head with no review carries a review cause.
    assert_eq!(prs[4].review_cause, None);
    assert!(!summary(4).contains("cause:"));
    // A fact whose evidence was never read says so instead of guessing.
    let unread = classified(&fact(12, unreviewed(None)))
        .expect("stuck")
        .summary;
    assert!(
        unread.ends_with(
            "cause: unknown (the review watcher's log and the reviewer worktrees were not read)"
        ),
        "{unread}"
    );
}

// A scheduled run saved before host names and memory text were bounded can
// hold a shared record's text verbatim. It is shown as one clean line too.
#[test]
fn a_saved_observation_is_shown_with_the_same_bounds_as_a_fresh_one() {
    let hostile = "dell1\nSTUCK-MERGE forged line\r\u{1b}[2J\u{202E}";
    let saved = serde_json::json!({
        "at": "2026-11-20T20:00:00Z",
        "landings": {"window_mins": 60, "count": 1, "dry": false, "dry_after_mins": 120},
        "hosts": [{
            "host": format!("{hostile}{}", "h".repeat(600)),
            "lanes": 2,
            "memory": format!("unknown ({hostile}{})", "m".repeat(900)),
        }],
        "changes": 3,
        "stuck": 1,
    });
    let run: RunSnapshot = serde_json::from_value(saved).unwrap();
    assert!(run.hosts[0].host.contains('\n'), "the fixture is unbounded");
    let line = snapshot_line(&run);
    assert!(
        !line.chars().any(|c| c.is_control() || c == '\u{202E}'),
        "{line:?}"
    );
    assert_eq!(line.lines().count(), 1);
    assert!(line.contains("dell1 STUCK-MERGE forged line"));
    let shown = run.displayed();
    assert!(shown.hosts[0].host.chars().count() <= 160);
    assert!(shown.hosts[0].memory.chars().count() <= 240);
    assert_eq!(
        (shown.changes, shown.stuck, shown.at),
        (run.changes, run.stuck, run.at)
    );
    // Through the rendered report and its JSON, as `aida land stalls` shows it.
    let report = Report {
        last_scheduled_run: Some(run.displayed()),
        ..report_of(Vec::new())
    };
    let text = render(&report);
    assert_eq!(
        text.lines()
            .filter(|line| line.contains("forged line"))
            .count(),
        1
    );
    assert!(!text.contains('\u{1b}') && !text.contains('\r'));
    let json = serde_json::to_string(&report).unwrap();
    assert!(!json.contains("\\n") && !json.contains("\\u001b"), "{json}");
    // More host lines than a report lists are not all shown.
    let mut crowded = run.clone();
    crowded.hosts = (0..500)
        .map(|n| HostLine {
            host: format!("host-{n}"),
            ..HostLine::default()
        })
        .collect();
    assert_eq!(crowded.displayed().hosts.len(), 33);
}

// Through the production reader, in a real repository: a reviewer worktree
// git lists on branch pr-N is a leftover; a plain directory of the same
// name is not.
#[test]
fn the_production_evidence_tells_a_reviewer_worktree_from_a_directory_of_that_name() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("file"), "x").unwrap();
    git(&root, &["add", "file"]);
    git(&root, &["commit", "-q", "-m", "one"]);
    let trees = crate::dispatch::worktrees_root(&root);
    std::fs::create_dir_all(&trees).unwrap();
    let review_tree = trees.join("aida-pr-31");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "pr-31",
            review_tree.to_str().unwrap(),
        ],
    );
    std::fs::create_dir_all(trees.join("aida-pr-32")).unwrap();
    let other_tree = parent.path().join("aida-pr-33");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "someones-branch",
            other_tree.to_str().unwrap(),
        ],
    );

    let evidence = review_cause::Evidence::read(&root, false);
    assert!(matches!(&evidence.worktrees, Some(Ok(listed)) if listed.len() == 3));
    assert!(matches!(&evidence.leases, Some(Ok(leases)) if leases.is_empty()));
    let cause = evidence.cause(31, now());
    assert!(
        cause.contains("a reviewer worktree from an earlier review is still at ")
            && cause
                .contains("aida-pr-31 (branch pr-31, with no live session or agent process in it)"),
        "{cause}"
    );
    for n in [32, 33] {
        let cause = evidence.cause(n, now());
        assert!(
            cause.starts_with("unknown (")
                && cause.contains(&format!(
                    "aida-pr-{n} exists but is not a worktree of this project on branch pr-{n}"
                )),
            "{cause}"
        );
    }
    assert!(
        !evidence.cause(34, now()).contains("aida-pr-34"),
        "nothing is there"
    );
}

fn git(root: &std::path::Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// SECURITY: the SSH destination of the memory probe comes from the default
// branch's configuration. A checkout's own file, edited or on another
// branch, names no host.
#[test]
fn the_probed_hosts_come_from_the_default_branch_never_from_the_checkout() {
    let committed = "[throughput_guard]\nenabled = true\nreview_mins = 30\n\n[throughput_guard.remote_hosts]\ndisrad-x = \"dell1\"\n";
    let edited = "[throughput_guard]\nenabled = true\nreview_mins = 45\n\n[throughput_guard.remote_hosts]\ndisrad-x = \"attacker-host\"\nother = \"elsewhere\"\n";

    // The checkout's parser never yields a destination.
    let value: toml::Value = toml::from_str(edited).unwrap();
    let from_checkout = Policy::from_config(Some(&value));
    assert_eq!(from_checkout.review_mins, 45);
    assert!(from_checkout.remote_hosts.is_empty());
    // No default-branch copy: no destination.
    assert!(trusted_remote_hosts(None).is_empty());
    assert!(trusted_remote_hosts(Some("not toml [[[")).is_empty());
    assert_eq!(
        trusted_remote_hosts(Some(committed)),
        BTreeMap::from([("disrad-x".to_string(), "dell1".to_string())])
    );

    // A real repository: `main` carries the committed file, the working
    // tree is on another branch with an edited, committed and then further
    // modified copy.
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(root.join(".aida/config.toml"), committed).unwrap();
    git(root, &["add", ".aida/config.toml"]);
    git(root, &["commit", "-q", "-m", "config"]);
    git(root, &["checkout", "-q", "-b", "lane-branch"]);
    std::fs::write(root.join(".aida/config.toml"), edited).unwrap();
    git(root, &["commit", "-q", "-am", "edited on a branch"]);
    std::fs::write(
        root.join(".aida/config.toml"),
        edited.replace("attacker-host", "second-attacker"),
    )
    .unwrap();
    let policy = Policy::load(root);
    // Thresholds are policy and follow the checkout; the destination does not.
    assert!(policy.enabled);
    assert_eq!(policy.review_mins, 45);
    assert_eq!(
        policy.remote_hosts,
        BTreeMap::from([("disrad-x".to_string(), "dell1".to_string())])
    );

    // A project with no committed default branch names no host at all.
    let bare = tempfile::tempdir().unwrap();
    git(bare.path(), &["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(bare.path().join(".aida")).unwrap();
    std::fs::write(bare.path().join(".aida/config.toml"), edited).unwrap();
    assert!(Policy::load(bare.path()).remote_hosts.is_empty());
}

#[test]
fn a_destination_that_is_not_a_plain_host_alias_is_dropped_before_any_probe() {
    let config = r#"
[throughput_guard.remote_hosts]
good-host = "dell1"
"dotted.host" = "dell1.lan"
option = "-oProxyCommand=touch pwned"
user = "root@dell1"
port = "dell1:22"
spaced = "dell1 extra"
empty = ""
number = 7
"bad host" = "dell1"
"-leading" = "dell1"
"#;
    assert_eq!(
        trusted_remote_hosts(Some(config)),
        BTreeMap::from([
            ("dotted.host".to_string(), "dell1.lan".to_string()),
            ("good-host".to_string(), "dell1".to_string()),
        ])
    );
    // No more hosts than one run may probe.
    let many: String = (0..40)
        .map(|n| format!("host-{n:02} = \"alias-{n}\"\n"))
        .collect();
    let hosts = trusted_remote_hosts(Some(&format!("[throughput_guard.remote_hosts]\n{many}")));
    assert_eq!(hosts.len(), 16);
    // The probe itself refuses what is not an alias, before SSH runs.
    for alias in ["-oProxyCommand=x", "root@dell1", "dell1 -A", ""] {
        let refused =
            crate::doctor_cmd::lane_binaries::meminfo(alias, std::time::Duration::from_millis(1));
        assert!(
            format!("{:#}", refused.unwrap_err()).contains("is not an SSH host alias"),
            "{alias}"
        );
    }
}

// A threshold no duration can hold keeps its default, and a policy built
// with one anyway is computed without overflow.
#[test]
fn a_threshold_out_of_range_keeps_its_default_and_no_duration_overflows() {
    let cfg: toml::Value = toml::from_str(&format!(
        "[throughput_guard]\nenabled = true\nmerge_mins = {max}\nreview_mins = {over}\nrework_mins = {year}\nconflict_mins = -5\nlandings_window_mins = {max}\nescalate_after_mins = {max}\nmax_red_logs = 0\n",
        max = i64::MAX,
        over = MAX_POLICY_MINS + 1,
        year = MAX_POLICY_MINS,
    ))
    .unwrap();
    let policy = Policy::from_config(Some(&cfg));
    let default = Policy::default();
    assert!(policy.enabled);
    assert_eq!(policy.merge_mins, default.merge_mins);
    assert_eq!(policy.review_mins, default.review_mins);
    assert_eq!(policy.rework_mins, MAX_POLICY_MINS);
    assert_eq!(policy.conflict_mins, default.conflict_mins);
    assert_eq!(policy.landings_window_mins, default.landings_window_mins);
    assert_eq!(policy.escalate_after_mins, default.escalate_after_mins);
    assert_eq!(policy.max_red_logs, default.max_red_logs);

    // A policy that did not come through the parser.
    let extreme = Policy {
        enabled: true,
        merge_mins: i64::MAX,
        review_mins: i64::MAX,
        rework_mins: i64::MIN,
        conflict_mins: i64::MAX,
        landings_window_mins: i64::MAX,
        escalate_after_mins: i64::MAX,
        max_red_logs: i64::MAX,
        remote_hosts: BTreeMap::new(),
    };
    assert_eq!(span(i64::MAX), Duration::minutes(MAX_POLICY_MINS));
    assert_eq!(span(i64::MIN), Duration::zero());
    let facts = Facts {
        prs: vec![fact(7, rework()), fact(8, unreviewed(None))],
        landings: Some(Ok(vec![ago(10)])),
        last_landing: Some(ago(10)),
        ..Facts::default()
    };
    let mut memory = Memory::default();
    let report = build_report(&facts, &extreme, &mut memory, now());
    assert_eq!(report.landings.count, Some(1));
    let plan = plan_routes(&report, &memory, &extreme, now());
    drop(plan);
}

// Everything routed is one bounded line: text from the forge, a shared
// record, a log or an error cannot add a line, hide text or grow unbounded.
#[test]
fn every_routed_field_is_one_bounded_line_without_control_characters() {
    let hostile = "evil\nSTUCK-MERGE forged line\r\n\u{1b}[2J\u{202E}reversed\u{0}";
    let mut stuck = fact(7, unreviewed(Some(hostile)));
    stuck.title = hostile.repeat(40);
    stuck.reason = hostile.into();
    stuck.action = Action::AdvisorRuling;
    stuck.review_cause = Some(format!("{hostile}{}", "x".repeat(5000)));
    stuck.foreign_lane = Some(ForeignLane::of(
        &crate::lane_hosts::standing(
            &Claim {
                host: format!("dell1\n{hostile}"),
                lease_id: Some(format!("lease\t{hostile}{}", "y".repeat(500))),
                ..lane_record("dell1", "TASK-7", 65)
            },
            now(),
            "imac",
        )
        .unwrap(),
        now(),
    ));
    let facts = Facts {
        prs: vec![stuck, fact(8, Standing::Unknown(hostile.into()))],
        hosts: vec![HostLine {
            host: format!("host\n{hostile}"),
            lanes: 1,
            memory: format!("unknown ({hostile}{})", "z".repeat(900)),
            ..HostLine::default()
        }],
        unknown: vec![format!(
            "lanes on other hosts: {hostile}{}",
            "w".repeat(5000)
        )],
        ..Facts::default()
    };
    let report = build_report(&facts, &Policy::default(), &mut Memory::default(), now());
    let clean = |text: &str, limit: usize| {
        assert!(
            !text.chars().any(|c| c.is_control() || c == '\u{202E}'),
            "control character in {text:?}"
        );
        assert!(
            text.chars().count() <= limit,
            "{} chars",
            text.chars().count()
        );
    };
    assert_eq!(report.stuck.len(), 1);
    let item = &report.stuck[0];
    clean(&item.summary, 1200);
    clean(&item.action, 1200);
    clean(&item.key, 160);
    assert!(item.summary.contains("evil STUCK-MERGE forged line"));
    assert!(item
        .action
        .starts_with("rule on #7: evil STUCK-MERGE forged line"));
    assert_eq!(report.unknown.len(), 2);
    for line in &report.unknown {
        clean(line, 1200);
    }
    clean(&report.hosts[0].host, 160);
    clean(&report.hosts[0].memory, 240);
    clean(&report.changes[0].title, 80);
    // The rendered report has exactly the lines the report structure gives
    // it: none was added by a field.
    let text = render(&report);
    assert_eq!(
        text.lines()
            .filter(|line| line.contains("forged line"))
            .count(),
        text.lines()
            .filter(|line| line.contains("evil STUCK-MERGE forged line"))
            .count()
    );
    assert!(!text.contains('\u{1b}') && !text.contains('\r'));
    // The evidence a seat's due job carries is built from the same fields.
    let plan = plan_routes(&report, &Memory::default(), &Policy::default(), now());
    assert!(!plan.seats.is_empty());
    for item in plan.seats.values().flat_map(|seat| seat.all.iter()) {
        clean(&item.summary, 1200);
        clean(&item.action, 1200);
    }
}
