//! STORY-2809: full-tier CI evidence on one exact commit, and the label
//! request that starts a full-tier run. Fake check listings and a fake forge
//! CLI runner; no network.
// trace:STORY-2809 | ai:claude
use super::*;
use crate::forge::CheckOutcome::{Failed, Passed, Pending, Skipped};
use crate::forge::CommitCheck;

const SHA: &str = "e33fec11d936d4cc6ce450719a506a68932915eb";

fn checks(rows: &[(&str, CheckOutcome, Option<&str>)]) -> CommitChecks {
    CommitChecks {
        sha: SHA.to_string(),
        checks: rows
            .iter()
            .map(|(name, outcome, run)| CommitCheck {
                name: name.to_string(),
                outcome: *outcome,
                run_id: run.map(str::to_string),
            })
            .collect(),
    }
}

/// A finished FAST run: its aggregate is green, and it proves nothing.
fn fast_run(run: &'static str) -> Vec<(&'static str, CheckOutcome, Option<&'static str>)> {
    vec![
        ("ci-tier: fast", Passed, Some(run)),
        ("CI (fast gates)", Passed, Some(run)),
        ("CI (fast compile)", Passed, Some(run)),
        ("Build (ubuntu-latest)", Passed, Some(run)),
    ]
}

fn full_run(
    run: &'static str,
    shard: CheckOutcome,
    aggregate: Option<CheckOutcome>,
) -> Vec<(&'static str, CheckOutcome, Option<&'static str>)> {
    let mut rows = vec![
        ("ci-tier: full", Passed, Some(run)),
        ("CI (lib-0)", Passed, Some(run)),
        ("CI (cli-1)", shard, Some(run)),
        ("CI (fast gates)", Skipped, Some(run)),
    ];
    if let Some(outcome) = aggregate {
        rows.push(("Build (ubuntu-latest)", outcome, Some(run)));
    }
    rows
}

#[test]
fn a_repository_without_tier_markers_is_untiered_but_no_checks_is_missing() {
    let untiered = checks(&[("Build (ubuntu-latest)", Passed, Some("1"))]);
    assert_eq!(judge(&untiered), FullTier::Untiered);
    assert_eq!(admission_refusal(&FullTier::Untiered, SHA), None);
    assert_eq!(judge(&checks(&[])), FullTier::Missing);
}

#[test]
fn a_green_fast_run_is_not_a_full_tier_run() {
    assert_eq!(judge(&checks(&fast_run("10"))), FullTier::Missing);
    let why = admission_refusal(&FullTier::Missing, SHA).unwrap();
    assert!(
        why.contains("no full-tier CI run exists at its head e33fec1"),
        "{why}"
    );
    assert!(
        why.contains("`ci-tier: full`") && why.contains("`ci:full`"),
        "{why}"
    );
}

#[test]
fn a_full_run_is_green_only_when_its_own_aggregate_passed() {
    let mut rows = fast_run("10");
    rows.extend(full_run("11", Passed, Some(Passed)));
    assert_eq!(judge(&checks(&rows)), FullTier::Green { run: "11".into() });
    assert_eq!(admission_refusal(&judge(&checks(&rows)), SHA), None);

    // Lanes still running, and the aggregate that registers after them.
    let mut rows = fast_run("10");
    rows.extend(full_run("11", Pending, None));
    assert_eq!(
        judge(&checks(&rows)),
        FullTier::Pending {
            run: "11".into(),
            waiting: vec!["CI (cli-1)".into()]
        }
    );
    let mut rows = fast_run("10");
    rows.extend(full_run("11", Passed, None));
    assert_eq!(
        judge(&checks(&rows)),
        FullTier::Pending {
            run: "11".into(),
            waiting: vec!["Build (ubuntu-latest)".into()]
        }
    );

    // The FAST run's green aggregate never stands in for a red full run.
    let mut rows = fast_run("10");
    rows.extend(full_run("11", Failed, Some(Failed)));
    let tier = judge(&checks(&rows));
    assert_eq!(
        tier,
        FullTier::Failed {
            run: "11".into(),
            failing: vec!["CI (cli-1)".into(), "Build (ubuntu-latest)".into()]
        }
    );
    let why = admission_refusal(&tier, SHA).unwrap();
    assert!(
        why.contains("run 11") && why.contains("CI (cli-1)"),
        "{why}"
    );
}

#[test]
fn a_green_rerun_at_the_same_head_outranks_an_earlier_red_or_cancelled_run() {
    let mut rows = full_run("11", Failed, Some(Failed));
    rows.extend(full_run("12", Passed, Some(Passed)));
    assert_eq!(judge(&checks(&rows)), FullTier::Green { run: "12".into() });
    // A newer run still in flight is reported ahead of the older failure.
    let mut rows = full_run("11", Failed, Some(Failed));
    rows.extend(full_run("12", Pending, None));
    assert!(matches!(judge(&checks(&rows)), FullTier::Pending { run, .. } if run == "12"));
}

#[test]
fn a_marker_without_a_run_or_with_a_second_tier_proves_nothing() {
    let unowned = checks(&[
        ("ci-tier: full", Passed, None),
        ("Build (ubuntu-latest)", Passed, None),
    ]);
    assert_eq!(judge(&unowned), FullTier::Missing);
    let mut rows = full_run("11", Passed, Some(Passed));
    rows.push(("ci-tier: fast", Passed, Some("11")));
    assert_eq!(judge(&checks(&rows)), FullTier::Missing);
}

#[test]
fn check_runs_carry_the_workflow_run_that_owns_them() {
    let page = serde_json::json!({"total_count": 2, "check_runs": [
        {"name": "ci-tier: full", "status": "completed", "conclusion": "success", "head_sha": SHA,
         "details_url": "https://github.com/o/r/actions/runs/777/job/5"},
        {"name": "external", "status": "completed", "conclusion": "success", "head_sha": SHA,
         "details_url": "https://example.test/build/9"},
    ]});
    let (_, runs) = crate::forge::parse_gh_check_runs(&page, SHA).unwrap();
    assert_eq!(runs[0].run_id.as_deref(), Some("777"));
    assert_eq!(runs[1].run_id, None);
}

fn project(origin: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut commands = vec![vec!["init", "-q"]];
    if let Some(url) = origin {
        commands.push(vec!["remote", "add", "origin", url]);
    }
    for args in commands {
        assert!(crate::test_git::git_command()
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    dir
}

fn github_project() -> tempfile::TempDir {
    let dir = project(Some("git@github.com:o/r.git"));
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(
        dir.path().join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    dir
}

#[test]
fn a_request_drops_a_stale_label_then_adds_it_so_a_run_starts_on_this_head() {
    let dir = github_project();
    let calls = std::cell::RefCell::new(Vec::new());
    let requested = request_full_ci_with(dir.path(), 41, |_, cli, args| {
        calls.borrow_mut().push(format!("{cli} {}", args.join(" ")));
        Ok((true, String::new()))
    });
    assert_eq!(requested, Ok(true));
    assert_eq!(
        *calls.borrow(),
        vec![
            "gh pr edit 41 -R o/r --remove-label ci:full",
            "gh pr edit 41 -R o/r --add-label ci:full",
        ]
    );
}

#[test]
fn a_failed_label_add_is_reported_and_a_project_without_github_asks_nothing() {
    let dir = github_project();
    let err = request_full_ci_with(dir.path(), 41, |_, _, args| {
        Ok((
            !args.contains(&"--add-label".to_string()),
            "label not found".into(),
        ))
    })
    .unwrap_err();
    assert!(
        err.contains("--add-label ci:full") && err.contains("label not found"),
        "{err}"
    );

    let plain = project(None);
    let requested = request_full_ci_with(plain.path(), 41, |_, _, _| {
        panic!("no forge call without a GitHub forge")
    });
    assert_eq!(requested, Ok(false));
}

#[test]
fn the_skip_event_records_the_pr_head_and_reason() {
    let event: serde_json::Value =
        serde_json::from_str(&skip_event("2026-11-14T12:00:00Z", 41, SHA, "why")).unwrap();
    assert_eq!(event["command"], "aida pr train");
    assert_eq!(event["step"], "train-admit-skip-full-check");
    assert_eq!(event["pr"], 41);
    assert_eq!(event["head_sha"], SHA);
    assert_eq!(event["detail"], "why");
}
