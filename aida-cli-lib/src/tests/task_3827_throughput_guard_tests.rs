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
        tree_roots: Vec::new(),
        stale_leases: Some(Ok(Vec::new())),
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
                ("dell1", 4, true)
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

// A record that was never released and went silent is neither a live lane
// nor "no lane": the stall is reported and says the lane's state is unknown.
#[test]
fn a_silent_lane_on_another_host_does_not_hide_the_stall_and_is_named_as_unknown() {
    let silent = [lane_record("dell1", "TASK-7", 65)];
    let held = observed(7, rework(), &silent);
    let lane = held.foreign_lane.clone().expect("on record");
    assert!(!lane.reporting);
    let item = classified(&held).expect("still a stall");
    assert_eq!(item.class, Class::StuckRework);
    assert_eq!(
        item.summary,
        "#7 has changes requested at 00000000 (round 2) and no lane or session known to be live on it for 65 min; a lane on host dell1 (lease-TASK-7) still holds it on record but last reported 65 min ago, past its expiry: whether it runs is unknown"
    );

    let item = classified(&observed(7, unreviewed(None), &silent)).expect("stuck");
    assert_eq!(item.class, Class::StuckReview);
    assert!(
        item.summary
            .contains("no verdict and no reviewer for 65 min")
            && item.summary.ends_with("whether it runs is unknown"),
        "{}",
        item.summary
    );
    let item = classified(&observed(7, unreviewed(Some("Test")), &silent)).expect("stuck");
    assert!(
        item.summary
            .contains("no reviewer and no lane known to be live for 65 min"),
        "{}",
        item.summary
    );
    let item =
        classified(&observed(7, Standing::Conflict { owner: None }, &silent)).expect("stuck");
    assert!(
        item.summary.contains("a lane on host dell1"),
        "{}",
        item.summary
    );

    // A reporting lane on one host outranks a silent record on another.
    let both = observed(
        7,
        rework(),
        &[
            lane_record("dell1", "TASK-7", 65),
            lane_record("dell2", "TASK-7", 2),
        ],
    );
    assert_eq!(classified(&both), None);

    // A record silent for longer than a day is a leftover: the plain wording.
    let leftover = observed(7, rework(), &[lane_record("dell1", "TASK-7", 3 * 24 * 60)]);
    assert_eq!(leftover.foreign_lane, None);
    let item = classified(&leftover).expect("stuck");
    assert!(
        item.summary
            .ends_with("no lane or session on it for 65 min"),
        "{}",
        item.summary
    );
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
        "7 lane(s) by the host's own records, newest report 2 min ago, and 1 more on record with no report since their expiry (state unknown)"
    );
    assert_eq!(dell1.memory, "3.0 GiB available of 16.0 GiB");
    // This host's lanes are counted here, not read from a record.
    assert_eq!(line("imac").lanes_text(), "2 lane(s), counted here now");
    assert_eq!(line("imac").memory, "20.0 GiB available of 64.0 GiB");
    // A host with only a silent record: no live lane is claimed for it.
    assert_eq!(
        line("dell3").lanes_text(),
        "0 lane(s) by the host's own records, newest report 200 min ago, and 1 more on record with no report since their expiry (state unknown)"
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
        stale_leases: Some(Ok(vec![("PR-9".into(), "lease-9".into())])),
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
            && tree.ends_with("aida-pr-8"),
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
