//! STORY-2912: the no-new-failures comparison against fake CI evidence.
// trace:STORY-2912 | ai:claude

use super::*;
use crate::forge::{CheckOutcome, CommitCheck, CommitChecks};
use anyhow::anyhow;
use std::collections::HashMap;
use std::path::Path;

const C: &str = "cccccccccccccccccccccccccccccccccccccccc";
const P: &str = "pppppppppppppppppppppppppppppppppppppppp";
const AGG: &str = "Build (ubuntu-latest)";

fn check(name: &str, outcome: CheckOutcome, run: &str) -> CommitCheck {
    CommitCheck {
        name: name.into(),
        outcome,
        run_id: Some(run.into()),
        workflow_id: Some("1".into()),
    }
}

/// A run's checks: every test shard and the aggregate, red where named.
fn run_checks(run: &str, red_shards: &[&str], red_other: &[&str]) -> Vec<CommitCheck> {
    let mut rows: Vec<CommitCheck> = TEST_SHARDS
        .iter()
        .map(|s| {
            let outcome = if red_shards.contains(s) {
                CheckOutcome::Failed
            } else {
                CheckOutcome::Passed
            };
            check(&format!("CI ({s})"), outcome, run)
        })
        .collect();
    for other in red_other {
        rows.push(check(other, CheckOutcome::Failed, run));
    }
    let red = !red_shards.is_empty() || !red_other.is_empty();
    rows.push(check(
        AGG,
        if red {
            CheckOutcome::Failed
        } else {
            CheckOutcome::Passed
        },
        run,
    ));
    rows
}

fn junit(cases: &[(&str, &str, bool)]) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"nextest-run\">\n",
    );
    let mut suites: Vec<&str> = cases.iter().map(|c| c.0).collect();
    suites.dedup();
    for suite in suites {
        xml.push_str(&format!("  <testsuite name=\"{suite}\" tests=\"1\">\n"));
        for (_, name, failed) in cases.iter().filter(|c| c.0 == suite) {
            if *failed {
                xml.push_str(&format!(
                    "    <testcase name=\"{name}\" classname=\"{suite}\" time=\"0.1\">\n      \
                     <failure type=\"test failure\" message=\"x &lt; y\">thread panicked</failure>\n      \
                     <system-out><![CDATA[<testcase name=\"fake\"/>]]></system-out>\n    </testcase>\n"
                ));
            } else {
                xml.push_str(&format!(
                    "    <testcase name=\"{name}\" classname=\"{suite}\" time=\"0.1\"/>\n"
                ));
            }
        }
        xml.push_str("  </testsuite>\n");
    }
    xml.push_str("</testsuites>\n");
    xml
}

#[derive(Default)]
struct Fake {
    checks: HashMap<String, Result<Vec<CommitCheck>, String>>,
    /// (run, shard) -> reports; missing = download error.
    reports: HashMap<(String, String), Vec<String>>,
    rerun: Option<Result<BTreeMap<String, bool>, String>>,
    reruns: Vec<Vec<String>>,
}

impl Evidence for Fake {
    fn checks(&mut self, sha: &str) -> Result<CommitChecks> {
        match self.checks.get(sha) {
            Some(Ok(rows)) => Ok(CommitChecks {
                sha: sha.into(),
                checks: rows.clone(),
            }),
            Some(Err(e)) => Err(anyhow!("{e}")),
            None => Ok(CommitChecks {
                sha: sha.into(),
                checks: Vec::new(),
            }),
        }
    }
    fn required(&mut self) -> Vec<String> {
        vec![AGG.into(), crate::landing_policy::MERGE_HOLD_GATE.into()]
    }
    fn shard_reports(&mut self, run_id: &str, shard: &str) -> Result<Vec<String>> {
        self.reports
            .get(&(run_id.to_string(), shard.to_string()))
            .cloned()
            .ok_or_else(|| anyhow!("artifact not found"))
    }
    fn rerun(&mut self, _sha: &str, tests: &[String]) -> Result<BTreeMap<String, bool>> {
        self.reruns.push(tests.to_vec());
        match self.rerun.clone() {
            Some(Ok(m)) => Ok(m),
            Some(Err(e)) => Err(anyhow!("{e}")),
            None => Err(anyhow!("no re-run configured")),
        }
    }
}

const FLAKY: &str = "aida-core store_sync::tests::time_dependent";
const BROKEN: &str = "aida-cli-lib dev_build::tests::new_regression";

/// C and P both red in lib-0 on the same time-dependent test.
fn same_failure() -> Fake {
    let mut f = Fake::default();
    f.checks
        .insert(C.into(), Ok(run_checks("200", &["lib-0"], &[])));
    f.checks
        .insert(P.into(), Ok(run_checks("100", &["lib-0"], &[])));
    f.reports.insert(
        ("200".into(), "lib-0".into()),
        vec![junit(&[
            ("aida-core", "store_sync::tests::time_dependent", true),
            ("aida-core", "ok_one", false),
        ])],
    );
    f.reports.insert(
        ("100".into(), "lib-0".into()),
        vec![junit(&[
            ("aida-core", "store_sync::tests::time_dependent", true),
            ("aida-core", "ok_one", false),
        ])],
    );
    f
}

/// AC5: equal failing sets allow the promotion.
#[test]
fn equal_failing_sets_allow() {
    let mut f = same_failure();
    let d = decide(&mut f, C, Some(P));
    assert!(d.allowed(), "{d:?}");
    assert_eq!(d.candidate_failing, vec![FLAKY.to_string()]);
    assert_eq!(d.promoted_failing, vec![FLAKY.to_string()]);
    assert!(d.new_failures.is_empty());
    assert_eq!(d.promoted_source, "ci");
    assert_eq!(d.refusal(), None);
    assert!(
        d.summary().contains("no-new-failures allowed"),
        "{}",
        d.summary()
    );
}

/// AC2/AC5: a test failing at C but not at P refuses, naming it.
#[test]
fn a_new_failure_refuses_naming_it() {
    let mut f = same_failure();
    f.reports.insert(
        ("200".into(), "lib-0".into()),
        vec![
            junit(&[("aida-core", "store_sync::tests::time_dependent", true)]),
            junit(&[("aida-cli-lib", "dev_build::tests::new_regression", true)]),
        ],
    );
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert_eq!(d.new_failures, vec![BROKEN.to_string()]);
    let why = d.refusal().unwrap();
    assert!(why.contains(BROKEN), "{why}");
    assert!(!why.contains(FLAKY), "only the new failure is named: {why}");
}

/// A new failure in a shard that was green at P is new too.
#[test]
fn a_failure_in_a_shard_green_at_p_refuses() {
    let mut f = same_failure();
    f.checks
        .insert(C.into(), Ok(run_checks("200", &["lib-0", "cli-1"], &[])));
    f.reports.insert(
        ("200".into(), "cli-1".into()),
        vec![junit(&[("aida-cli::journey", "walks", true)])],
    );
    let d = decide(&mut f, C, Some(P));
    assert_eq!(d.new_failures, vec!["aida-cli::journey walks".to_string()]);
    assert!(!d.allowed());
}

/// AC3/AC5: missing or unreadable evidence for C refuses with a named reason.
#[test]
fn missing_candidate_evidence_refuses() {
    // The shard's artifact is missing.
    let mut f = same_failure();
    f.reports.remove(&("200".into(), "lib-0".into()));
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(
        d.reason.contains("shard `lib-0`") && d.reason.contains("could not be read"),
        "{}",
        d.reason
    );

    // CI cannot be read at all.
    let mut f = same_failure();
    f.checks.insert(C.into(), Err("gh offline".into()));
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(
        d.reason.contains("could not be read") && d.reason.contains("gh offline"),
        "{}",
        d.reason
    );

    // No CI run on C.
    let mut f = same_failure();
    f.checks.remove(C);
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(d.reason.contains("no CI run"), "{}", d.reason);

    // An unreadable report.
    let mut f = same_failure();
    f.reports.insert(
        ("200".into(), "lib-0".into()),
        vec!["<testsuites><testsuite name=\"x\">".into()],
    );
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(d.reason.contains("unreadable"), "{}", d.reason);

    // A red shard whose report names no failing test.
    let mut f = same_failure();
    f.reports.insert(
        ("200".into(), "lib-0".into()),
        vec![junit(&[("aida-core", "ok_one", false)])],
    );
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(d.reason.contains("names no failing test"), "{}", d.reason);

    // An empty artifact.
    let mut f = same_failure();
    f.reports.insert(("200".into(), "lib-0".into()), Vec::new());
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(d.reason.contains("no JUnit report"), "{}", d.reason);

    // No commit at all.
    let d = decide(&mut same_failure(), "", Some(P));
    assert!(!d.allowed());
}

/// Pending CI and a missing required check refuse.
#[test]
fn incomplete_candidate_ci_refuses() {
    let mut f = same_failure();
    let mut rows = run_checks("200", &["lib-0"], &[]);
    rows[1].outcome = CheckOutcome::Pending;
    f.checks.insert(C.into(), Ok(rows));
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(
        d.reason.contains("still running") && d.reason.contains("CI (lib-1)"),
        "{}",
        d.reason
    );

    let mut f = same_failure();
    let rows: Vec<_> = run_checks("200", &[], &[])
        .into_iter()
        .filter(|c| c.name != AGG)
        .collect();
    f.checks.insert(C.into(), Ok(rows));
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(d.reason.contains(AGG), "{}", d.reason);
}

/// Missing or partial evidence at P refuses too.
#[test]
fn missing_promoted_evidence_refuses() {
    let mut f = same_failure();
    f.reports.remove(&("100".into(), "lib-0".into()));
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(d.reason.contains(&P[..12]), "{}", d.reason);

    let mut f = same_failure();
    f.checks.insert(P.into(), Err("rate limited".into()));
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(d.reason.contains("rate limited"), "{}", d.reason);

    let d = decide(&mut same_failure(), C, None);
    assert!(!d.allowed());
    assert!(d.reason.contains("no promoted commit"), "{}", d.reason);
}

/// A green candidate promotes without reading P; a green P makes every
/// candidate failure new.
#[test]
fn green_candidate_allows_and_green_promoted_refuses_red_candidate() {
    let mut f = Fake::default();
    f.checks.insert(C.into(), Ok(run_checks("200", &[], &[])));
    let d = decide(&mut f, C, None);
    assert!(d.allowed(), "{d:?}");

    let mut f = same_failure();
    f.checks.insert(P.into(), Ok(run_checks("100", &[], &[])));
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert_eq!(d.new_failures, vec![FLAKY.to_string()]);
}

/// A non-test check red at C must be red at P; the aggregate beside other
/// red checks is not compared on its own.
#[test]
fn non_test_checks_are_compared_by_name() {
    let mut f = same_failure();
    f.checks
        .insert(C.into(), Ok(run_checks("200", &["lib-0"], &["Clippy"])));
    let d = decide(&mut f, C, Some(P));
    assert_eq!(d.new_failures, vec!["check: Clippy".to_string()]);
    assert!(!d.allowed());

    let mut f = same_failure();
    f.checks
        .insert(C.into(), Ok(run_checks("200", &["lib-0"], &["Clippy"])));
    f.checks
        .insert(P.into(), Ok(run_checks("100", &["lib-0"], &["Clippy"])));
    let d = decide(&mut f, C, Some(P));
    assert!(d.allowed(), "{d:?}");
    assert!(
        !d.candidate_failing.iter().any(|c| c.contains(AGG)),
        "{d:?}"
    );
}

/// A test failing at P and absent at C is not "fixed" without a pass record.
#[test]
fn absent_at_candidate_is_unconfirmed_not_fixed() {
    let mut f = same_failure();
    f.checks
        .insert(P.into(), Ok(run_checks("100", &["lib-0", "rest"], &[])));
    f.reports.insert(
        ("100".into(), "rest".into()),
        vec![junit(&[
            ("aida-mcp", "gone", true),
            ("aida-mcp", "healed", true),
        ])],
    );
    // C's green `rest` shard reports `healed` passing; `gone` is absent.
    f.reports.insert(
        ("200".into(), "rest".into()),
        vec![junit(&[("aida-mcp", "healed", false)])],
    );
    let d = decide(&mut f, C, Some(P));
    assert!(d.allowed(), "{d:?}");
    assert_eq!(d.fixed, vec!["aida-mcp healed".to_string()]);
    assert_eq!(d.unconfirmed, vec!["aida-mcp gone".to_string()]);
}

/// AC1: P with no recorded CI run re-runs C's failing tests at P.
#[test]
fn promoted_without_a_run_reruns_the_failing_tests() {
    let mut f = same_failure();
    f.checks.remove(P);
    f.rerun = Some(Ok(BTreeMap::from([(FLAKY.to_string(), false)])));
    let d = decide(&mut f, C, Some(P));
    assert!(d.allowed(), "{d:?}");
    assert_eq!(d.promoted_source, "local re-run");
    assert_eq!(f.reruns, vec![vec![FLAKY.to_string()]]);

    // Passing at P: the failure is new.
    let mut f = same_failure();
    f.checks.remove(P);
    f.rerun = Some(Ok(BTreeMap::from([(FLAKY.to_string(), true)])));
    let d = decide(&mut f, C, Some(P));
    assert_eq!(d.new_failures, vec![FLAKY.to_string()]);
    assert!(!d.allowed());

    // Undetermined or failed re-run: refused.
    let mut f = same_failure();
    f.checks.remove(P);
    f.rerun = Some(Ok(BTreeMap::new()));
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(d.reason.contains("did not determine"), "{}", d.reason);
    let mut f = same_failure();
    f.checks.remove(P);
    f.rerun = Some(Err("cargo exploded".into()));
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(d.reason.contains("cargo exploded"), "{}", d.reason);

    // A non-test check cannot be re-run.
    let mut f = same_failure();
    f.checks
        .insert(C.into(), Ok(run_checks("200", &["lib-0"], &["Clippy"])));
    f.checks.remove(P);
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(d.reason.contains("Clippy"), "{}", d.reason);
    assert!(f.reruns.is_empty());
}

/// The decision round-trips through its JSON record.
#[test]
fn decision_records_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let d = decide(&mut same_failure(), C, Some(P));
    record_last(dir.path(), &d).unwrap();
    assert_eq!(read_last(dir.path()), Some(d));
}

#[test]
fn junit_parsing_is_strict_and_skips_cdata() {
    let r = parse_junit(&junit(&[
        ("a", "x", true),
        ("a", "y", false),
        ("b::t", "z", false),
    ]))
    .unwrap();
    assert_eq!(r.failed, BTreeSet::from(["a x".to_string()]));
    assert_eq!(
        r.passed,
        BTreeSet::from(["a y".to_string(), "b::t z".to_string()])
    );
    // `<error>` (a crash or timeout) is a failure; entities are decoded.
    let r = parse_junit(
        "<testsuites><testsuite name=\"s\"><testcase name=\"a&lt;b&gt;\"><error/></testcase></testsuite></testsuites>",
    )
    .unwrap();
    assert_eq!(r.failed, BTreeSet::from(["s a<b>".to_string()]));
    assert!(parse_junit("").is_err());
    assert!(parse_junit("<testsuites></testsuites>").is_err());
    assert!(parse_junit("<testsuites><testsuite name=\"s\">").is_err());
    assert!(parse_junit("<testsuites><testsuite name=\"s\"></testcase></testsuites>").is_err());
}

#[test]
fn cargo_args_and_libtest_outcomes() {
    assert_eq!(
        cargo_target_args("aida-cli-lib").unwrap(),
        vec!["-p", "aida-cli-lib", "--lib"]
    );
    assert_eq!(
        cargo_target_args("aida-cli::journey").unwrap(),
        vec!["-p", "aida-cli", "--test", "journey"]
    );
    assert_eq!(
        cargo_target_args("aida-cli::bin/aida").unwrap(),
        vec!["-p", "aida-cli", "--bin", "aida"]
    );
    assert_eq!(cargo_target_args("x::bench/b"), None);
    assert_eq!(cargo_target_args("--evil"), None);
    assert_eq!(cargo_target_args("a::bin/--evil"), None);
    let out = "running 1 test\ntest a::b ... FAILED\n\ntest a::c ... ok\n";
    assert_eq!(libtest_outcome(out, "a::b"), Some(false));
    assert_eq!(libtest_outcome(out, "a::c"), Some(true));
    assert_eq!(libtest_outcome(out, "a::d"), None);
    assert_eq!(libtest_outcome("test a::b ... ignored", "a::b"), None);
}

/// A red lane that is neither a known test shard nor a known non-test lane
/// refuses: it might hold test failures, so it is never compared by name.
#[test]
fn an_unknown_red_lane_refuses() {
    let mut f = same_failure();
    f.checks
        .insert(C.into(), Ok(run_checks("200", &["lib-0"], &["CI (lib-2)"])));
    f.checks
        .insert(P.into(), Ok(run_checks("100", &["lib-0"], &["CI (lib-2)"])));
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(d.reason.contains("CI (lib-2)"), "{}", d.reason);

    let mut f = same_failure();
    f.checks
        .insert(C.into(), Ok(run_checks("200", &["lib-0"], &["CI (gates)"])));
    f.checks
        .insert(P.into(), Ok(run_checks("100", &["lib-0"], &["CI (gates)"])));
    assert!(decide(&mut f, C, Some(P)).allowed());
}

/// The shard names, check names and report artifact match the CI workflow,
/// so the comparison cannot silently stop finding test reports.
#[test]
fn shards_match_the_ci_workflow() {
    let ci = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.github/workflows/ci.yml"),
    )
    .unwrap();
    let lanes: Vec<&str> = TEST_SHARDS.iter().chain(NON_TEST_LANES).copied().collect();
    let matrix = format!("shard: [{}]", lanes.join(", "));
    assert!(ci.contains(&matrix), "ci.yml matrix is not `{matrix}`");
    assert!(ci.contains("name: CI (${{ matrix.shard }})"));
    assert!(ci.contains("name: test-timings-${{ matrix.shard }}"));
    assert!(ci.contains("target/nextest/ci/junit-*.xml"));
    assert_eq!(report_artifact("lib-0"), "test-timings-lib-0");
    assert_eq!(shard_of_check("CI (lib-0)"), Some("lib-0"));
    assert_eq!(shard_of_check("CI (gates)"), None);
}
