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
            c.records = vec![("TASK-9".into(), Vec::new(), None)]
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
    let mut polls = 0;
    let mut gated = Vec::new();
    let rows = check_all(
        &[5, 3, 5, 9, 2],
        || {
            polls += 1;
            polls > 2
        },
        |pr| {
            gated.push(pr);
            eligible(pr)
        },
    );
    assert_eq!(gated, vec![5, 3]);
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
    let text = render_text("main", &rows);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "{text}");
    assert!(lines[1].starts_with("  #40  yes  head aaaaaaa"), "{text}");
    assert!(
        lines[2].starts_with("  #41  no   approval: no approval is recorded for it — fix: "),
        "{text}"
    );
    assert_eq!(lines[3], "1 of 2 eligible");

    let json = render_json("main", &rows);
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
    assert_eq!(render_json("main", &rows[..1])["eligible"], true);
    assert_eq!(render_json("main", &[])["eligible"], true);
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
