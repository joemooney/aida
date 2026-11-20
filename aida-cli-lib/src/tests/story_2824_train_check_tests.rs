//! STORY-2824: `aida pr train --check`. The report runs the train's own
//! member gate, so these tests script that gate (the `FakeChecks` of the
//! train tests) and read the rows it yields.
// trace:STORY-2824 | ai:claude

use super::*;
use crate::pr_train::check::{
    check_all, check_one, gate_token, remedy, render_json, render_text, Evidence, Row,
};

fn checked(c: &mut FakeChecks) -> Row {
    c.read_only = true;
    check_one(c, "main", 41, Ok(None))
}

fn eligible(pr: u64) -> Row {
    Row {
        pr,
        eligible: true,
        head: Some("a".repeat(40)),
        gate: None,
        reason: None,
        remedy: None,
        evidence: Evidence::Read,
    }
}

// Acceptance 2: the verdict is the train gate's own, probe for probe.
#[test]
fn a_member_the_train_admits_is_eligible_at_its_head() {
    let mut c = FakeChecks::new();
    let row = checked(&mut c);
    assert_eq!(row, eligible(41));
    assert_eq!(
        c.calls,
        vec![
            "metadata", "drive", "hold", "labels", "fetch", "approval", "message", "corpus",
            "specs"
        ],
        "the report runs the same probes, in the same order, as the train"
    );
    let mut train = FakeChecks::new();
    assert_eq!(
        gate_member_with(&mut train, "main", 41).unwrap().head,
        row.head.unwrap()
    );
}

// Acceptance 1: a refused PR names the first gate that stopped it, with the
// train's reason and a remedy.
#[test]
fn each_refusal_names_the_gate_that_stopped_it() {
    type Setup = fn(&mut FakeChecks);
    let cases: &[(&str, &str, Setup)] = &[
        ("pr-state", "draft", |c| c.meta.is_draft = true),
        ("pr-state", "targets `release`", |c| {
            c.meta.base_ref = "release".into()
        }),
        ("live-drive", "live drive owns it", |c| {
            c.drive = Some("live drive PID 7".into())
        }),
        ("merge-hold", "under a merge hold", |c| {
            c.hold = Some("waiting on the operator".into())
        }),
        ("hold-labels", "aida:merge-hold label", |c| {
            c.labels = Ok(Some(crate::merge_hold::PinnedChange {
                merged: false,
                state: crate::merge_hold::ChangeState::Open,
                head_sha: None,
                hold_label: true,
                recorded_label: false,
                cleared_label: false,
            }))
        }),
        ("review-in-progress", "under review", |c| {
            claim_for_review(c.root.path(), 41, &"a".repeat(40))
        }),
        ("approval", "no approval is recorded", |c| c.pin = None),
        ("approval", "pinned to", |c| c.pin = Some("c".repeat(40))),
        ("spec-policy", "never auto-merges a supervised spec", |c| {
            c.records = vec![("TASK-9".into(), Vec::new(), None.into())]
        }),
    ];
    for (gate, reason, setup) in cases {
        let mut c = FakeChecks::new();
        setup(&mut c);
        let row = checked(&mut c);
        assert!(!row.eligible, "{gate}");
        assert_eq!(row.gate, Some(*gate), "{:?}", row.reason);
        assert!(row.reason.as_deref().unwrap().contains(reason), "{row:?}");
        assert!(!row.remedy.as_deref().unwrap().is_empty(), "{row:?}");
        assert_eq!(row.evidence, Evidence::Read, "{gate}");
        // The reason is the train's, word for word.
        let mut train = FakeChecks::new();
        setup(&mut train);
        assert_eq!(
            gate_member_with(&mut train, "main", 41).unwrap_err(),
            row.reason.unwrap(),
            "{gate}"
        );
    }
}

#[test]
fn a_held_member_under_a_mandate_that_does_not_release_it_stops_at_the_hold() {
    let mut c = FakeChecks::new();
    c.hold = Some("waiting on the operator".into());
    c.mandate_active = true;
    c.mandate = Err(Some("the hold is sensitive".into()));
    let row = checked(&mut c);
    assert_eq!(row.gate, Some("merge-hold"), "{row:?}");
    assert!(row.remedy.unwrap().contains("aida merge-hold clear 41"));
}

#[test]
fn an_unreadable_probe_is_not_eligible_and_says_to_check_again() {
    let mut c = FakeChecks::new();
    c.labels = Err("gh: 502".into());
    let row = checked(&mut c);
    assert!(!row.eligible);
    assert_eq!(row.gate, Some("hold-labels"));
    assert_eq!(row.evidence, Evidence::Unavailable);
    assert!(row.remedy.unwrap().contains("check again"));
}

#[test]
fn a_pr_already_riding_a_train_stops_before_the_member_gate() {
    let mut c = FakeChecks::new();
    let row = check_one(
        &mut c,
        "main",
        41,
        Ok(Some("it is already riding live train PR #900".into())),
    );
    assert_eq!(row.gate, Some("riding-train"));
    assert!(c.calls.is_empty(), "{:?}", c.calls);
    let row = check_one(&mut c, "main", 41, Err("ownership unreadable".into()));
    assert_eq!(row.evidence, Evidence::Unavailable);
    assert!(!row.eligible);
}

// Acceptance 3: the review claim is read without clearing an expired one,
// which the gate does only on a read-only run.
#[test]
fn the_report_asks_the_gate_to_run_read_only() {
    let mut c = FakeChecks::new();
    let m = crate::gate::review_marker::Marker {
        pid: 4_000_000_000,
        ..crate::gate::review_marker::Marker::for_this_process(
            41,
            Some(&"a".repeat(40)),
            None,
            "reviewer seat",
        )
    };
    crate::gate::review_marker::write(c.root.path(), &m).unwrap();
    assert!(
        checked(&mut c).eligible,
        "a dead reviewer's claim does not block"
    );
    assert!(
        crate::gate::review_marker::read(c.root.path(), 41).is_some(),
        "the report left the expired claim in place"
    );
    let mut train = FakeChecks::new();
    crate::gate::review_marker::write(train.root.path(), &m).unwrap();
    gate_member_with(&mut train, "main", 41).unwrap();
    assert!(
        crate::gate::review_marker::read(train.root.path(), 41).is_none(),
        "the train clears it"
    );
}

#[test]
fn every_gate_has_a_name_and_a_remedy() {
    let gates = [
        Gate::PrState,
        Gate::Riding,
        Gate::Drive,
        Gate::Hold,
        Gate::HoldLabels,
        Gate::ReviewClaim,
        Gate::Head,
        Gate::Approval,
        Gate::FullCi,
        Gate::Corpus,
        Gate::Message,
        Gate::SpecPolicy,
        Gate::Mandate,
    ];
    let names: std::collections::BTreeSet<_> = gates.iter().map(|g| gate_token(*g)).collect();
    assert_eq!(names.len(), gates.len(), "gate names are distinct");
    for gate in gates {
        assert!(!remedy(gate, 41).is_empty());
    }
    assert!(remedy(Gate::Approval, 41).contains("aida pr review 41"));
}

// Acceptance 5: past the budget no further PR is gated; each PR is checked
// once however often it is named.
#[test]
fn the_time_budget_stops_new_checks_and_reports_the_rest_unchecked() {
    let checks_started = std::cell::Cell::new(0);
    let mut gated = Vec::new();
    let rows = check_all(
        &[5, 3, 5, 9, 2],
        || checks_started.get() >= 3,
        |pr| {
            gated.push(pr);
            checks_started.set(checks_started.get() + 1);
            eligible(pr)
        },
    );
    assert_eq!(gated, vec![5, 3, 9]);
    assert_eq!(
        rows.iter().map(|r| r.pr).collect::<Vec<_>>(),
        vec![5, 3, 9, 2]
    );
    for row in &rows[2..] {
        assert!(!row.eligible);
        assert_eq!(row.evidence, Evidence::NotChecked);
        assert!(row.remedy.as_deref().unwrap().contains("--timeout"));
    }
}

// Acceptance 1 and 4: one line per PR, and JSON whose top-level `eligible`
// is true only when every PR is.
#[test]
fn the_report_is_one_line_per_pr_and_json_says_whether_all_are_eligible() {
    let mut c = FakeChecks::new();
    c.pin = None;
    let rows = vec![eligible(40), checked(&mut c)];
    let text = render_text(Some("main"), &rows);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "{text}");
    assert!(lines[1].starts_with("  #40  yes  head aaaaaaa"), "{text}");
    assert!(
        lines[2].starts_with("  #41  no   approval: no approval is recorded for it — fix: "),
        "{text}"
    );
    assert_eq!(lines[3], "1 of 2 eligible");

    let json = render_json(Some("main"), &rows);
    assert_eq!(json["eligible"], false);
    assert_eq!(json["base"], "main");
    assert_eq!(json["prs"][0]["pr"], 40);
    assert_eq!(json["prs"][0]["eligible"], true);
    assert_eq!(json["prs"][0]["head"], "a".repeat(40));
    assert!(json["prs"][0]["gate"].is_null());
    assert_eq!(json["prs"][1]["gate"], "approval");
    assert_eq!(json["prs"][1]["evidence"], "read");
    assert!(json["prs"][1]["remedy"]
        .as_str()
        .unwrap()
        .contains("aida pr review 41"));
    assert_eq!(render_json(Some("main"), &rows[..1])["eligible"], true);
    assert_eq!(render_json(Some("main"), &[])["eligible"], true);
}

#[test]
fn check_takes_named_prs_or_all_open_and_excludes_the_landing_flags() {
    let parse = |args: &[&str]| {
        crate::TEST_CLI.try_parse_from(["aida", "pr", "train"].iter().chain(args.iter()))
    };
    for ok in [
        &["--check", "4027", "4030"][..],
        &["--check", "--all-open"],
        &["--check", "--all-open", "--json"],
        &["--check", "4027", "--timeout", "20"],
    ] {
        assert!(parse(ok).is_ok(), "{ok:?}");
    }
    for bad in [
        &["--check"][..],
        &["--all-open"],
        &["4027", "--json"],
        &["4027", "--timeout", "20"],
        &["--check", "--all-open", "4027"],
        &["--check", "--all-ready"],
        &["--check", "4027", "--dry-run"],
        &["--check", "4027", "--no-pull"],
        &["--check", "--abandon", "stale-1.json"],
    ] {
        assert!(parse(bad).is_err(), "{bad:?}");
    }
}

// trace:STORY-2824 | ai:codex
#[test]
fn story_2824_all_open_refuses_failed_malformed_and_capped_listings() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let environment = aida_core::exec_context::Environment::capture();
    let root = repo();
    let r = root.path();
    git(
        r,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/project.git",
        ],
    );
    crate::test_exec::write_executable(
        &r.join("gh"),
        r#"#!/bin/sh
printf '%s\n' "$*" >> gh-calls
case "$*" in
  *'number,headRefName,body'*) printf '[]\n' ;;
  *'--limit 1000'*) cat listing; exit "$(cat listing-exit)" ;;
  *) printf '[]\n' ;;
esac
"#,
    );
    let path = format!("{}:{}", r.display(), environment.var("PATH").unwrap());
    let _env = crate::test_env::EnvVarsGuard::set(&[("PATH", &path)]);
    let row = |id| {
        serde_json::json!({
            "number": id, "headRefName": format!("work/{id}"),
            "baseRefName": "main", "state": "OPEN"
        })
    };
    let list = |rows: serde_json::Value, code: &str| {
        std::fs::write(r.join("listing"), rows.to_string()).unwrap();
        std::fs::write(r.join("listing-exit"), code).unwrap();
        crate::pr_train::check::all_open_candidates(r, "main")
    };
    assert!(list(serde_json::json!([]), "1").is_err());
    // The command must return an error (a nonzero CLI exit), even though
    // the next train-list read can succeed with no live trains.
    crate::test_exec::write_executable(
        &r.join("git"),
        r#"#!/bin/sh
case "$*" in
  *'ls-remote --symref origin HEAD'*) printf 'ref: refs/heads/main\tHEAD\n' ;;
  *) exec /usr/bin/git "$@" ;;
esac
"#,
    );
    let opts = crate::pr_train::check::CheckOptions {
        prs: &[],
        all_open: true,
        skip_full_check: false,
        json: true,
        timeout: std::time::Duration::from_secs(45),
    };
    let error = crate::pr_train::check::check_at(&opts, r, r).unwrap_err();
    assert!(
        format!("{error:#}").contains("forge observation failed"),
        "{error:#}"
    );
    assert!(live_trains(r).is_ok());
    assert!(list(serde_json::json!([row(1), {"number": 2}]), "0").is_err());
    assert!(list(serde_json::json!({"number": 1}), "0").is_err());
    std::fs::write(r.join("listing"), "not json").unwrap();
    assert!(crate::pr_train::check::all_open_candidates(r, "main").is_err());
    let rows: Vec<_> = (1..=31).map(row).collect();
    assert_eq!(
        list(serde_json::json!(rows), "0").unwrap(),
        (1..=31).collect::<Vec<_>>()
    );
    let rows: Vec<_> = (1..=1000).map(row).collect();
    assert!(list(serde_json::json!(rows), "0").is_err());
    assert!(list(serde_json::json!([]), "0").unwrap().is_empty());
    let calls = std::fs::read_to_string(r.join("gh-calls")).unwrap();
    assert!(calls
        .lines()
        .filter(|s| !s.contains("number,headRefName,body"))
        .all(|s| s.contains("--limit 1000")));
}

// trace:STORY-2824 | ai:codex
#[test]
fn full_ci_report_refuses_missing_pending_and_failed_without_requesting_ci() {
    for tier in [
        crate::full_tier::FullTier::Missing,
        crate::full_tier::FullTier::Pending {
            run: "901".into(),
            waiting: vec!["CI".into()],
        },
        crate::full_tier::FullTier::Failed {
            run: "902".into(),
            failing: vec!["CI".into()],
        },
    ] {
        let mut c = FakeChecks::new();
        c.full_tier = Some(tier);
        let row = checked(&mut c);
        assert!(!row.eligible, "{row:?}");
        assert_eq!(row.gate, Some("full-ci"));
        assert!(c.full_requests.is_empty());
        assert!(!c.root.path().join(".aida/advisor-activity.jsonl").exists());
    }
}

// trace:STORY-2824 | ai:codex
#[test]
fn full_ci_report_override_writes_nothing_even_when_admitting_is_true() {
    let mut c = FakeChecks::new();
    c.full_tier = Some(crate::full_tier::FullTier::Missing);
    std::fs::create_dir_all(c.root.path().join(".aida/advisor-activity.jsonl")).unwrap();
    let row = with_skip_full_check(true, || checked(&mut c));
    assert!(row.eligible, "{row:?}");
    assert!(c.full_requests.is_empty());
}

// trace:STORY-2824 | ai:codex
#[test]
fn check_dispatch_carries_full_ci_override_without_writes() {
    use crate::pr_train::check::TEST_CHECK_DISPATCH;
    fn probe(opts: &crate::pr_train::CheckOptions<'_>) -> anyhow::Result<()> {
        assert_eq!(opts.prs, &[41]);
        let mut c = FakeChecks::new();
        c.full_tier = Some(crate::full_tier::FullTier::Missing);
        // Any attempt to append activity fails, even if this test runs in
        // an admitting session. The report must never request a CI run.
        std::fs::create_dir_all(c.root.path().join(".aida/advisor-activity.jsonl")).unwrap();
        let row = checked(&mut c);
        assert_eq!(row.eligible, opts.skip_full_check, "{row:?}");
        if !opts.skip_full_check {
            assert_eq!(row.gate, Some("full-ci"));
        }
        assert!(c.full_requests.is_empty());
        Ok(())
    }
    for skip_full_check in [false, true, false] {
        TEST_CHECK_DISPATCH.with(|hook| hook.set(Some(probe)));
        crate::pr_cmd::handle_pr_command(&crate::pr_cmd::PrCommand::Train {
            status: false,
            abandon: None,
            prs: vec![41],
            all_ready: false,
            dry_run: false,
            local_ci: None,
            max: 10,
            no_pull: false,
            check: true,
            all_open: false,
            json: true,
            timeout: 45,
            skip_full_check,
        })
        .unwrap();
        assert!(TEST_CHECK_DISPATCH.with(|hook| hook.get().is_none()));
        let mut c = FakeChecks::new();
        c.full_tier = Some(crate::full_tier::FullTier::Missing);
        assert!(!checked(&mut c).eligible, "override leaked past dispatch");
    }
}

// trace:TASK-3792 | ai:codex
// trace:TASK-3792 | ai:claude
#[test]
#[cfg(target_os = "linux")]
fn task_3792_train_check_bounds_discovery_and_member_subprocesses() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    use crate::pr_train::check::{collect_report, CheckOptions};
    use std::time::{Duration, Instant};

    let root = repo();
    let r = root.path();
    git(
        r,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/project.git",
        ],
    );
    // Every stall records the pid of a descendant holding the pipes in
    // `child-<stall>`: `hold` keeps the leader waiting on it, `orphan` exits
    // the leader at once and leaves it behind.
    crate::test_exec::write_executable(
        &r.join("git"),
        &r#"#!/bin/sh
S='@S@'
# aida-core bounds its stored-URL read with its own deadline transport,
# which the `config` stall below proves; every other spawn is marked.
case "$*" in
  *'config --get-regexp ^remote'*) ;;
  *) [ -n "$@G@" ] || echo "git $*" >> "$S/unbounded" ;;
esac
stall=$(cat "$S/stall")
hold() { sleep 20 & echo "$!" > "$S/child-$stall"; wait; }
orphan() { sleep 20 & echo "$!" > "$S/child-$stall"; }
case "$*" in
  *'worktree list --porcelain'*)
    if [ "$stall" = worktree ]; then hold; fi
    exec /usr/bin/git "$@" ;;
  *'remote get-url origin'*)
    if [ "$stall" = origin ]; then hold; fi
    exec /usr/bin/git "$@" ;;
  *'config --get-regexp ^remote'*)
    if [ "$stall" = config ]; then hold; fi
    exec /usr/bin/git "$@" ;;
  *'ls-remote --symref origin HEAD'*)
    echo "$stall" >> "$S/ls-remote"
    if [ "$stall" = base ]; then hold; fi
    if [ "$stall" = orphan ]; then orphan; fi
    if [ "$stall" = shared ]; then sleep 0.65; fi
    printf 'ref: refs/heads/main\tHEAD\n'
    ;;
  *'fetch --quiet --no-tags origin'*)
    if [ "$stall" = fetch ]; then
      echo fetch > "$S/entered"
      hold
    fi
    exit 1 ;;
  *'fetch --no-tags --no-write-fetch-head'*)
    if [ "$stall" = confirm ]; then
      echo confirm > "$S/confirm-entered"
      hold
    fi
    exit 1 ;;
  *) exec /usr/bin/git "$@" ;;
esac
"#
        .replace("@S@", &r.display().to_string())
        .replace(
            "@G@",
            crate::bounded_process::test_hooks::SCOPED_DEADLINE_SPAWN_ENV,
        ),
    );
    crate::test_exec::write_executable(
        &r.join("gh"),
        &r#"#!/bin/sh
[ -n "$@G@" ] || echo "gh $*" >> '@S@/unbounded'
case "$*" in
  --version) echo fixture ;;
  *'number,headRefName,body'*)
    case "$(cat stall)" in
      trains|known-open) exec sleep 20 ;;
      shared) sleep 0.65 ;;
    esac
    echo '[]'
    ;;
  *'--limit 1000'*)
    if [ "$(cat stall)" = known-open ]; then
      echo '[{"number":41,"headRefName":"fixture","baseRefName":"main","state":"OPEN"}]'
    else
      exec sleep 20
    fi
    ;;
  *'view 50 '*'isCrossRepository,headRepository,isDraft'*)
    echo '{"state":"CLOSED","title":"Train","author":{"login":"fixture"},"mergedAt":null,"baseRefName":"main","headRefName":"train","headRefOid":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","isCrossRepository":false,"headRepository":{"name":"project"},"isDraft":false}'
    ;;
  *'isCrossRepository,headRepository,isDraft'*)
    if [ "$(cat stall)" = member ]; then exec sleep 20; fi
    echo '{"state":"OPEN","title":"Fixture","author":{"login":"fixture"},"mergedAt":null,"baseRefName":"main","headRefName":"fixture","headRefOid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","isCrossRepository":false,"headRepository":{"name":"project"},"isDraft":false}'
    ;;
  *'url,state,headRefOid,labels'*)
    if [ "$(cat stall)" = labels ]; then exec sleep 20; fi
    echo '{"url":"https://github.com/example/project/pull/41","state":"OPEN","headRefOid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","labels":[]}'
    ;;
  *) exit 1 ;;
esac
"#
        .replace("@S@", &r.display().to_string())
        .replace("@G@", crate::bounded_process::test_hooks::SCOPED_DEADLINE_SPAWN_ENV),
    );
    let _path = crate::test_env::EnvVarGuard::prepend_path(r);
    let unmarked = std::cell::RefCell::new(Vec::<String>::new());
    let run = |stall: &str, all_open, timeout| {
        std::fs::write(r.join("stall"), stall).unwrap();
        let opts = CheckOptions {
            prs: if all_open { &[] } else { &[41, 42, 41] },
            all_open,
            skip_full_check: false,
            json: true,
            timeout,
        };
        let _ = std::fs::remove_file(r.join("unbounded"));
        let started = Instant::now();
        let result = collect_report(&opts, || {
            assert!(!timeout.is_zero(), "zero budget attempted discovery");
            Ok((r.to_owned(), crate::main_worktree_root_from(r)))
        });
        assert_train_read_elapsed(stall, started.elapsed(), timeout);
        // Guard: under the check's deadline no git or gh ran without it.
        let unbounded = std::fs::read_to_string(r.join("unbounded")).unwrap_or_default();
        for line in unbounded.lines() {
            unmarked.borrow_mut().push(format!("{stall}: {line}"));
        }
        result
    };
    let stopped = |stall: &str| task_3792_recorded_child_stopped(&r.join(format!("child-{stall}")));
    let all_unchecked = |stall: &str, rows: &[Row]| {
        assert_eq!(rows.iter().map(|row| row.pr).collect::<Vec<_>>(), [41, 42]);
        assert!(
            rows.iter()
                .all(|row| !row.eligible && row.evidence == Evidence::NotChecked),
            "{stall}: {rows:?}"
        );
    };
    for (stall, timeout, expected_base) in [
        ("base", Duration::ZERO, None),
        ("base", Duration::from_secs(1), None),
        // The leader prints the answer and exits; its child keeps the pipes.
        ("orphan", Duration::from_secs(1), None),
        ("worktree", Duration::from_secs(1), None),
        ("origin", Duration::from_secs(1), None),
        ("config", Duration::from_secs(1), None),
        ("trains", Duration::from_secs(1), Some("main")),
        ("shared", Duration::from_secs(1), Some("main")),
        ("member", Duration::from_secs(1), Some("main")),
        ("labels", Duration::from_secs(1), Some("main")),
        ("fetch", Duration::from_secs(1), Some("main")),
    ] {
        let (base, rows) = run(stall, false, timeout).unwrap();
        assert_eq!(base.as_deref(), expected_base, "{stall}");
        all_unchecked(stall, &rows);
        if timeout.is_zero() {
            assert!(!r.join("child-base").exists());
            let json = render_json(base.as_deref(), &rows);
            assert!(json["base"].is_null());
            assert_eq!(json["eligible"], false);
            assert_eq!(json["prs"][0]["evidence"], "not-checked");
        } else if !matches!(stall, "trains" | "shared" | "member" | "labels") {
            assert!(
                stopped(stall),
                "{stall}: timed-out descendant still running"
            );
        }
    }
    assert_eq!(
        std::fs::read_to_string(r.join("entered")).unwrap().trim(),
        "fetch"
    );
    // An unread credential config is unknown: the remote command it was
    // preparing never ran.
    let ls_remote = std::fs::read_to_string(r.join("ls-remote")).unwrap();
    assert!(!ls_remote.lines().any(|l| l == "config"), "{ls_remote}");
    let (base, rows) = run("known-open", true, Duration::from_secs(1)).unwrap();
    assert_eq!(base.as_deref(), Some("main"));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pr, 41);
    assert_eq!(rows[0].evidence, Evidence::NotChecked);
    let err = run("all-open", true, Duration::from_secs(1)).unwrap_err();
    assert!(
        err.to_string()
            .contains("before open PR discovery completed"),
        "{err:#}"
    );

    // PR 41 rides train #50, which the forge reports CLOSED: confirming that
    // fetches the base independently, and that fetch shares the budget.
    crate::ownership::dual::enable_for_test();
    crate::ownership::dual::train_opened(r, 50, &[41]);
    assert!(matches!(
        crate::ownership::reader::load(
            r,
            &crate::ownership::WorkKey::Change {
                change: crate::ownership::dual::change(41),
            }
        )
        .unwrap()
        .unwrap()
        .phase,
        crate::ownership::Phase::Training { .. }
    ));
    let (base, rows) = run("confirm", false, Duration::from_secs(1)).unwrap();
    assert_eq!(base.as_deref(), Some("main"));
    all_unchecked("confirm", &rows);
    assert_eq!(
        std::fs::read_to_string(r.join("confirm-entered"))
            .unwrap()
            .trim(),
        "confirm"
    );
    assert!(stopped("confirm"), "confirmation fetch still running");
    let unmarked = unmarked.into_inner();
    assert!(
        unmarked.is_empty(),
        "spawned without the check's deadline:\n{}",
        unmarked.join("\n")
    );
}

// The approval-timestamp fetch for a reviewed commit missing locally shares
// the scoped deadline: a stalled fetch (leader waiting, or leader gone with a
// child holding its pipes) is killed with its descendants, and the unknown
// timestamp grants no authority. A fetch that finishes supplies the date.
// trace:TASK-3792 | ai:claude
#[test]
#[cfg(target_os = "linux")]
fn task_3792_approval_timestamp_fetch_is_bounded_and_fails_closed() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    use crate::gate::review_verdict::{RecordedVerdict, RecorderAttestation, VerdictKind};
    use crate::gate::ship::{approval_authority_refusal, ApprovalAuthorityRefusal};
    use std::time::{Duration, Instant};

    // The reviewed commit exists only on origin.
    let origin = repo();
    write(origin.path(), "reviewed.txt", "reviewed\n");
    git(origin.path(), &["checkout", "--quiet", "-b", "reviewed"]);
    let reviewed = commit(origin.path(), "reviewed work");
    git(origin.path(), &["checkout", "--quiet", "main"]);
    git(
        origin.path(),
        &["config", "uploadpack.allowAnySHA1InWant", "true"],
    );
    let root = repo();
    let r = root.path();
    let url = format!("file://{}", origin.path().display());
    git(r, &["remote", "add", "origin", &url]);
    assert!(crate::pr_cmd::commit_committed_at(r, &reviewed).is_none());

    let bin = tempfile::tempdir().unwrap();
    crate::test_exec::write_executable(
        &bin.path().join("git"),
        &r#"#!/bin/sh
S='@S@'
stall=$(cat "$S/stall")
case "$*" in
  *'fetch --quiet --no-tags origin'*)
    echo "$stall" >> "$S/fetches"
    case "$stall" in
      hold) sleep 20 & echo "$!" > "$S/child-$stall"; wait ;;
      orphan) sleep 20 & echo "$!" > "$S/child-$stall"; exit 0 ;;
    esac
    exec /usr/bin/git "$@" ;;
  *) exec /usr/bin/git "$@" ;;
esac
"#
        .replace("@S@", &bin.path().display().to_string()),
    );
    let _path = crate::test_env::EnvVarGuard::prepend_path(bin.path());

    let approval = RecordedVerdict {
        kind: VerdictKind::Approved,
        raw: "approved".into(),
        reviewed_sha: Some(reviewed.clone()),
        // Recorded long after the commit, so a known date admits it.
        recorded_at: Some((chrono::Utc::now() + chrono::Duration::seconds(5)).to_rfc3339()),
        recorded_by: Some("aida review record (operator)".into()),
        attestation: Some(RecorderAttestation {
            sha: reviewed.clone(),
            human_at_tty: true,
            receipt_id: Some("rcpt-1".into()),
            identity: vec!["session:independent-reviewer".into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let v = [approval];
    let decide = |stall: &str| {
        std::fs::write(bin.path().join("stall"), stall).unwrap();
        let started = Instant::now();
        let read = crate::forge_lookup::with_forge_lookup_deadline(Duration::from_secs(1), || {
            crate::gate::approval::ship_approval_policy(r, &[], None, &v)
        });
        assert!(started.elapsed() < Duration::from_secs(4), "{stall}");
        // Isolate the timestamp: vouch for the attestation as its store would.
        let mut policy = read.clone();
        let a = v[0].attestation.as_ref().unwrap();
        let key = a.authority_key(v[0].recorded_at.as_deref()).unwrap();
        policy.verified_authority.insert(key, a.identity.clone());
        (
            read.reviewed_committed_at,
            approval_authority_refusal(&v, &policy),
        )
    };
    let stopped = |stall: &str| {
        let child = std::fs::read_to_string(bin.path().join(format!("child-{stall}"))).unwrap();
        let proc_stat = format!("/proc/{}/stat", child.trim());
        let gone = || {
            std::fs::read_to_string(&proc_stat)
                .map(|stat| stat.rsplit_once(") ").unwrap().1.starts_with('Z'))
                .unwrap_or(true)
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while !gone() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        gone()
    };
    for stall in ["hold", "orphan"] {
        let (at, refusal) = decide(stall);
        assert_eq!(at, None, "{stall}: a timed-out fetch supplied a date");
        assert!(
            matches!(
                &refusal,
                Some(ApprovalAuthorityRefusal::Unattested { reason, .. })
                    if reason.contains("date could not be read")
            ),
            "{stall}: {refusal:?}"
        );
        assert!(stopped(stall), "{stall}: fetch descendant still running");
        assert!(crate::pr_cmd::commit_committed_at(r, &reviewed).is_none());
    }
    // Control: the same fetch, finishing, supplies the date and admits it.
    let (at, refusal) = decide("none");
    assert_eq!(
        at,
        crate::pr_cmd::commit_committed_at(origin.path(), &reviewed)
    );
    assert!(at.is_some());
    assert_eq!(refusal, None);
    assert_eq!(
        std::fs::read_to_string(bin.path().join("fetches")).unwrap(),
        "hold\norphan\nnone\n"
    );
}

// trace:TASK-3825 | ai:codex
#[cfg(target_os = "linux")]
fn assert_train_read_elapsed(
    stall: &str,
    elapsed: std::time::Duration,
    timeout: std::time::Duration,
) {
    // Removing the deadline waits out the 20s fixture sleep. Removing just the
    // group kill leaves the child live beyond the separate 5s exit poll below.
    let limit = timeout + std::time::Duration::from_secs(5);
    assert!(
        elapsed < limit,
        "{stall}: took {elapsed:?}, limit {limit:?}"
    );
}

/// True once the pid recorded in `file` is gone or a zombie, waiting up to 5s.
#[cfg(target_os = "linux")]
fn task_3792_recorded_child_stopped(file: &std::path::Path) -> bool {
    use std::time::{Duration, Instant};
    let child = std::fs::read_to_string(file)
        .unwrap_or_else(|_| panic!("{}: the stalled subprocess never ran", file.display()));
    let pid: u32 = child.trim().parse().expect("expected a recorded child PID");
    assert!(pid > 0, "expected a positive child PID");
    let proc_stat = format!("/proc/{pid}/stat");
    let gone = || match std::fs::read_to_string(&proc_stat) {
        Ok(stat) => stat.rsplit_once(") ").unwrap().1.starts_with('Z'),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
        // The PID may disappear between opening its stat file and reading it.
        Err(err) if err.raw_os_error() == Some(libc::ESRCH) => true,
        Err(err) => panic!("reading {proc_stat}: {err}"),
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if gone() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// The member gate's full-ci step reads commit checks through `gh api`, whose
// own per-request ceiling is 30s. Under the check's deadline a stalled read
// is killed with its descendants at the deadline, and once the deadline has
// passed no request is spawned at all.
// trace:TASK-3792 | ai:claude
#[test]
#[cfg(target_os = "linux")]
fn task_3792_full_ci_commit_reader_shares_the_deadline() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    use crate::pr_train::{LiveChecks, MemberChecks};
    use std::time::{Duration, Instant};

    let root = repo();
    let r = root.path();
    let bin = tempfile::tempdir().unwrap();
    let s = bin.path();
    crate::test_exec::write_executable(
        &s.join("gh"),
        &r#"#!/bin/sh
S='@S@'
case "$*" in
  --version) echo fixture ;;
  api*)
    echo api >> "$S/api"
    sleep 20 & echo "$!" > "$S/child-api"; wait ;;
  *) exit 1 ;;
esac
"#
        .replace("@S@", &s.display().to_string()),
    );
    let _path = crate::test_env::EnvVarGuard::prepend_path(s);
    let forge = crate::forge::forge_for_kind(r, crate::forge::ForgeKind::GitHub);
    let mut checks = LiveChecks {
        project_root: r,
        main_worktree: r,
        forge: forge.as_ref(),
        base: "main",
        named: true,
        read_only: true,
        actor_probe: None,
        request_full: false,
        released_member: None,
    };
    let head = "a".repeat(40);
    let started = Instant::now();
    let tier = crate::forge_lookup::with_forge_lookup_deadline(Duration::from_secs(1), || {
        checks.full_tier(&head)
    });
    assert!(tier.is_err(), "a stalled CI read judged the tier: {tier:?}");
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "the CI read outlived the deadline: {:?}",
        started.elapsed()
    );
    assert!(
        task_3792_recorded_child_stopped(&s.join("child-api")),
        "the stalled CI read's descendant is still running"
    );
    // An expired deadline spawns no further request.
    let tier =
        crate::forge_lookup::with_forge_lookup_deadline(Duration::ZERO, || checks.full_tier(&head));
    assert!(tier.is_err(), "{tier:?}");
    assert_eq!(std::fs::read_to_string(s.join("api")).unwrap(), "api\n");
}

// The approval gate's local reads — the reviewed commit's date (`git log`),
// carry-over merge-base and patch-id, and the ancestry probe — have no
// deadline of their own; under the check's deadline a stalled one is killed
// with its descendants and reads as unavailable evidence. With no stall the
// same reads answer.
// trace:TASK-3792 | ai:claude
#[test]
#[cfg(target_os = "linux")]
fn task_3792_approval_local_git_reads_share_the_deadline() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    use crate::gate::review_carryover::{patch_id, single_merge_base};
    use std::time::{Duration, Instant};

    let root = repo();
    let r = root.path();
    let base = git(r, &["rev-parse", "HEAD"]).trim().to_string();
    write(r, "work.txt", "work\n");
    let tip = commit(r, "work");
    let bin = tempfile::tempdir().unwrap();
    let s = bin.path();
    crate::test_exec::write_executable(
        &s.join("git"),
        &r#"#!/bin/sh
S='@S@'
stall=$(cat "$S/stall")
hold() { sleep 20 & echo "$!" > "$S/child-$stall"; wait; }
case "$*" in
  *'log -1 --format=%cI'*) [ "$stall" = log ] && hold ;;
  *'merge-base --all'*) [ "$stall" = merge-base ] && hold ;;
  *'merge-base --is-ancestor'*) [ "$stall" = ancestry ] && hold ;;
  *'patch-id --verbatim'*) [ "$stall" = patch-id ] && hold ;;
  *'diff --no-color'*) [ "$stall" = diff ] && hold ;;
esac
exec /usr/bin/git "$@"
"#
        .replace("@S@", &s.display().to_string()),
    );
    let _path = crate::test_env::EnvVarGuard::prepend_path(s);
    let reads = |stall: &str| {
        std::fs::write(s.join("stall"), stall).unwrap();
        let timeout = Duration::from_secs(1);
        let started = Instant::now();
        let got = crate::forge_lookup::with_forge_lookup_deadline(timeout, || match stall {
            "log" => crate::gate::kernel_deps::commit_committed_at(r, &tip).is_some(),
            "merge-base" => single_merge_base(r, &base, &tip).is_ok(),
            "ancestry" => crate::gate::kernel_deps::is_ancestor_commit(r, &base, &tip).is_some(),
            "patch-id" | "diff" => patch_id(r, &base, &tip).is_ok(),
            other => unreachable!("{other}"),
        });
        assert_train_read_elapsed(stall, started.elapsed(), timeout);
        got
    };
    for stall in ["log", "merge-base", "ancestry", "patch-id", "diff"] {
        assert!(!reads(stall), "{stall}: a stalled read supplied evidence");
        assert!(
            task_3792_recorded_child_stopped(&s.join(format!("child-{stall}"))),
            "{stall}: the stalled read's descendant is still running"
        );
    }
    // Control: the same reads, unstalled, answer.
    std::fs::write(s.join("stall"), "none").unwrap();
    for stall in ["log", "merge-base", "ancestry", "patch-id"] {
        std::fs::write(s.join("stall"), "none").unwrap();
        let got =
            crate::forge_lookup::with_forge_lookup_deadline(
                Duration::from_secs(5),
                || match stall {
                    "log" => crate::gate::kernel_deps::commit_committed_at(r, &tip).is_some(),
                    "merge-base" => single_merge_base(r, &base, &tip).is_ok(),
                    "ancestry" => {
                        crate::gate::kernel_deps::is_ancestor_commit(r, &base, &tip) == Some(true)
                    }
                    _ => patch_id(r, &base, &tip).is_ok(),
                },
            );
        assert!(got, "{stall}: the unstalled read failed");
    }
}

// trace:STORY-2893 | ai:codex
#[test]
fn story_2893_actor_follows_the_train_gate() {
    use crate::gate::mandate::MandateRefusal;
    use crate::pr_train::check::{next_actor, ActorProbe};

    let mut missing = FakeChecks::new();
    missing.pin = None;
    let mut stale = FakeChecks::new();
    stale.pin = Some("c".repeat(40));
    for checks in [&mut missing, &mut stale] {
        let next = next_actor(checked(checks), ActorProbe::default());
        assert_eq!((next.actor, next.gate), ("reviewer", "approval"));
    }

    let mut ci = FakeChecks::new();
    ci.full_tier = Some(crate::full_tier::FullTier::Missing);
    let next = next_actor(checked(&mut ci), ActorProbe::default());
    assert_eq!((next.actor, next.gate), ("CI", "full-ci"));

    let mut supervised =
        FakeChecks::shipping("TASK-9", &[], Some(aida_core::ExecutionMode::Guided));
    supervised.mandate_active = true;
    supervised.mandate = Ok(fake_cover());
    let next = next_actor(checked(&mut supervised), ActorProbe::default());
    assert_eq!((next.actor, next.gate), ("train", "train-eligible"));

    let next = next_actor(
        check_one(
            &mut FakeChecks::new(),
            "main",
            41,
            Ok(Some("riding #900".into())),
        ),
        ActorProbe::default(),
    );
    assert_eq!((next.actor, next.gate), ("train", "riding-train"));

    let row = Row {
        pr: 41,
        eligible: false,
        head: None,
        gate: Some("mandate"),
        reason: Some("mandate excludes TASK-9".into()),
        remedy: None,
        evidence: Evidence::Read,
    };
    let next = next_actor(
        row.clone(),
        ActorProbe {
            mandate_refusal: Some(MandateRefusal::Excluded {
                spec: "TASK-9".into(),
                why: "excluded execution mode".into(),
            }),
            ..ActorProbe::default()
        },
    );
    assert_eq!((next.actor, next.gate), ("operator", "mandate"));
    assert!(next.reason.contains("excludes TASK-9"));
    let next = next_actor(
        row.clone(),
        ActorProbe {
            mandate_refusal: Some(MandateRefusal::LedgerUnreadable("malformed ledger".into())),
            ..ActorProbe::default()
        },
    );
    assert_eq!(next.actor, "unverified");
    assert_eq!(next_actor(row, ActorProbe::default()).actor, "unverified");

    let mut unavailable = FakeChecks::new();
    unavailable.labels = Err("unreadable".into());
    assert_eq!(
        next_actor(checked(&mut unavailable), ActorProbe::default()).actor,
        "unverified"
    );
}

// STORY-2884: a spec on the project's unset-mode drain default makes the
// member gate read the change's files (`git diff`). Under the check's
// deadline a stalled diff is killed with its descendants, and the unknown
// files withdraw the default (the spec holds, supervised). Unstalled, the
// same read answers and an ordinary change keeps the default.
// trace:STORY-2884 trace:TASK-3792 | ai:claude
#[test]
#[cfg(target_os = "linux")]
fn story_2884_defaulted_spec_diff_read_shares_the_deadline() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    use crate::gate::unset_mode::{ModeSource, UnsetModePolicy};
    use crate::pr_train::{LiveChecks, MemberChecks};
    use std::time::{Duration, Instant};

    let root = repo();
    let r = root.path();
    let base = git(r, &["rev-parse", "HEAD"]).trim().to_string();
    git(r, &["update-ref", "refs/remotes/origin/main", &base]);
    write(r, "work.txt", "work\n");
    let tip = commit(r, "work");
    let bin = tempfile::tempdir().unwrap();
    let s = bin.path();
    crate::test_exec::write_executable(
        &s.join("git"),
        &r#"#!/bin/sh
S='@S@'
stall=$(cat "$S/stall")
case "$*" in
  *'diff --name-only'*)
    [ -n "$@G@" ] || echo "git $*" >> "$S/unbounded"
    if [ "$stall" = diff ]; then sleep 20 & echo "$!" > "$S/child-diff"; wait; fi ;;
esac
exec /usr/bin/git "$@"
"#
        .replace("@S@", &s.display().to_string())
        .replace(
            "@G@",
            crate::bounded_process::test_hooks::SCOPED_DEADLINE_SPAWN_ENV,
        ),
    );
    let _path = crate::test_env::EnvVarGuard::prepend_path(s);
    let policy = UnsetModePolicy::from_body(Some(
        "[gate.unset_mode]\ndefault = \"drain\"\ndrain_types = [\"bug\"]\n",
    ));
    let mut bug = aida_core::Requirement::new("fix the sort".into(), "d".into());
    bug.req_type = aida_core::RequirementType::Bug;
    let defaulted = || vec![("BUG-1".to_string(), Vec::new(), policy.resolve(None, &bug))];
    assert!(defaulted()[0].2.is_project_default());
    let forge = crate::forge::forge_for_kind(r, crate::forge::ForgeKind::GitHub);
    let mut checks = LiveChecks {
        project_root: r,
        main_worktree: r,
        forge: forge.as_ref(),
        base: "main",
        named: true,
        read_only: true,
        actor_probe: None,
        request_full: false,
        released_member: None,
    };

    std::fs::write(s.join("stall"), "diff").unwrap();
    let mut records = defaulted();
    let started = Instant::now();
    crate::forge_lookup::with_forge_lookup_deadline(Duration::from_secs(1), || {
        checks.withdraw_unset_defaults(&mut records, &tip)
    });
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "the diff read outlived the deadline: {:?}",
        started.elapsed()
    );
    assert!(
        task_3792_recorded_child_stopped(&s.join("child-diff")),
        "the stalled diff's descendant is still running"
    );
    let mode = &records[0].2;
    assert!(mode.requires_supervision(), "{mode:?}");
    assert!(
        matches!(&mode.source, ModeSource::Unset { withheld: Some(why) }
            if why.contains("could not be read")),
        "{mode:?}"
    );

    // Control: unstalled, the files are read and an ordinary change keeps
    // the default.
    std::fs::write(s.join("stall"), "none").unwrap();
    let mut records = defaulted();
    crate::forge_lookup::with_forge_lookup_deadline(Duration::from_secs(5), || {
        checks.withdraw_unset_defaults(&mut records, &tip)
    });
    assert!(records[0].2.is_project_default(), "{:?}", records[0].2);
    assert!(
        !s.join("unbounded").exists(),
        "a diff spawned without the check's deadline: {}",
        std::fs::read_to_string(s.join("unbounded")).unwrap_or_default()
    );
}
