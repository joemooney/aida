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

/// A run's checks: every test shard, gates, contracts and the aggregate,
/// red where named (`red_other` adds or turns red any other check).
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
    for lane in ["CI (gates)", "CI (contracts)"] {
        if !red_other.contains(&lane) {
            rows.push(check(lane, CheckOutcome::Passed, run));
        }
    }
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

type Case<'a> = (&'a str, &'a str, bool);

fn junit(cases: &[Case]) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"nextest-run\">\n",
    );
    let mut suites: Vec<&str> = cases.iter().map(|c| c.0).collect();
    suites.dedup();
    if suites.is_empty() {
        suites.push("empty");
    }
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

/// One nextest invocation's log, as `scripts/ci-test-shards.py` tees it.
fn nextest_log(args: &str, cases: &[Case]) -> String {
    let failed = cases.iter().filter(|c| c.2).count();
    let n = cases.len();
    let mut log = format!(
        "+ cargo nextest run --no-fail-fast --profile ci --no-tests warn {args}\n\
         \x1b[1m    Starting\x1b[0m {n} tests across 1 binary\n"
    );
    for (bin, name, _) in cases.iter().filter(|c| c.2) {
        log.push_str(&format!("        FAIL [   0.100s] {bin} {name}\n"));
        log.push_str("  stdout ───\n    + cargo nextest run (a test's own output)\n");
    }
    log.push_str(&format!(
        "------------\n     Summary [   1.234s] {n} tests run: {} passed (1 leaky), {failed} failed, \
         2 skipped\n",
        n - failed
    ));
    log
}

/// One `cargo test --doc` invocation's log: `(name, outcome)` per doctest.
fn doc_log(args: &str, krate: &str, tests: &[(&str, &str)]) -> String {
    let mut log = format!(
        "+ cargo test --no-fail-fast --doc {args}\n   Doc-tests {krate}\n\nrunning {} tests\n",
        tests.len()
    );
    let count = |o: &str| tests.iter().filter(|t| t.1 == o).count();
    for (name, outcome) in tests {
        log.push_str(&format!("test {name} ... {outcome}\n"));
    }
    log.push_str(&format!(
        "\ntest result: {}. {} passed; {} failed; {} ignored; 0 measured; 0 filtered out; \
         finished in 0.50s\n\n",
        if count("FAILED") > 0 { "FAILED" } else { "ok" },
        count("ok"),
        count("FAILED"),
        count("ignored")
    ));
    if count("FAILED") > 0 {
        log.push_str("error: doctest failed, to rerun pass `--doc`\n");
    }
    log
}

/// A single-invocation shard (lib-N, cli-N) artifact.
fn one_run(shard: &str, cases: &[Case]) -> ShardArtifact {
    let mut log = String::new();
    if shard.starts_with("lib-") {
        log.push_str("+ cargo test -p aida-cli-lib --lib -- --list --format terse\n");
        log.push_str(&format!(
            "# {shard}: 3 module filters, 10 tests filtered out\n"
        ));
    }
    log.push_str(&nextest_log("-p aida-cli-lib --lib", cases));
    ShardArtifact {
        log: Some(log),
        reports: BTreeMap::from([(format!("junit-{shard}-0.xml"), junit(cases))]),
    }
}

const DOC_OK: &str = "aida-cli-lib/src/lib.rs - widget (line 3)";
const DOC_NEW: &str = "aida-cli-lib/src/lib.rs - gadget (line 9)";

/// The `rest` shard: workspace nextest, its doctests, aida-cli's bins,
/// aida-cli-lib's doctests.
fn rest_run(workspace: &[Case], bins: &[Case], lib_docs: &[(&str, &str)]) -> ShardArtifact {
    let log = [
        nextest_log(
            "--workspace --exclude aida-cli --exclude aida-cli-lib",
            workspace,
        ),
        doc_log("--workspace --exclude aida-cli", "aida_core", &[]),
        nextest_log("-p aida-cli --bins", bins),
        doc_log("-p aida-cli-lib", "aida_cli_lib", lib_docs),
    ]
    .concat();
    ShardArtifact {
        log: Some(log),
        reports: BTreeMap::from([
            ("junit-rest-0.xml".to_string(), junit(workspace)),
            ("junit-rest-1.xml".to_string(), junit(bins)),
        ]),
    }
}

#[derive(Default)]
struct Fake {
    checks: HashMap<String, Result<Vec<CommitCheck>, String>>,
    /// (run, shard) -> artifact; missing = download error.
    artifacts: HashMap<(String, String), ShardArtifact>,
    /// (run, job) -> failed steps; missing = `Run tests` alone.
    steps: HashMap<(String, String), Result<Vec<String>, String>>,
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
    fn shard_artifact(&mut self, run_id: &str, shard: &str) -> Result<ShardArtifact> {
        self.artifacts
            .get(&(run_id.to_string(), shard.to_string()))
            .cloned()
            .ok_or_else(|| anyhow!("artifact not found"))
    }
    fn failed_steps(&mut self, run_id: &str, job: &str) -> Result<Vec<String>> {
        match self.steps.get(&(run_id.to_string(), job.to_string())) {
            Some(Ok(s)) => Ok(s.clone()),
            Some(Err(e)) => Err(anyhow!("{e}")),
            None => Ok(vec![TEST_STEP.to_string()]),
        }
    }
}

impl Fake {
    fn put(&mut self, run: &str, shard: &str, artifact: ShardArtifact) {
        self.artifacts.insert((run.into(), shard.into()), artifact);
    }
}

const FLAKY: &str = "aida-core store_sync::tests::time_dependent";
const BROKEN: &str = "aida-cli-lib dev_build::tests::new_regression";
const FLAKY_CASE: Case = ("aida-core", "store_sync::tests::time_dependent", true);
const OK_CASE: Case = ("aida-core", "ok_one", false);

/// C and P both red in lib-0 on the same time-dependent test.
fn same_failure() -> Fake {
    let mut f = Fake::default();
    f.checks
        .insert(C.into(), Ok(run_checks("200", &["lib-0"], &[])));
    f.checks
        .insert(P.into(), Ok(run_checks("100", &["lib-0"], &[])));
    f.put("200", "lib-0", one_run("lib-0", &[FLAKY_CASE, OK_CASE]));
    f.put("100", "lib-0", one_run("lib-0", &[FLAKY_CASE, OK_CASE]));
    f
}

/// AC5: equal failing sets allow the promotion, and both sets are shown.
// trace:STORY-2912.accdae61 | ai:claude
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
    let lines = d.status_lines();
    assert!(lines[0].contains("no-new-failures allowed"), "{lines:?}");
    assert!(
        lines[1].contains("failing at the candidate") && lines[1].contains(FLAKY),
        "{lines:?}"
    );
    assert!(
        lines[2].contains("failing at the promoted") && lines[2].contains(FLAKY),
        "{lines:?}"
    );
}

/// AC2/AC5: a test failing at C but not at P refuses, naming it.
// trace:STORY-2912.acd07159 | ai:claude
#[test]
fn a_new_failure_refuses_naming_it() {
    let mut f = same_failure();
    f.put(
        "200",
        "lib-0",
        one_run(
            "lib-0",
            &[
                FLAKY_CASE,
                ("aida-cli-lib", "dev_build::tests::new_regression", true),
            ],
        ),
    );
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert_eq!(d.new_failures, vec![BROKEN.to_string()]);
    let why = d.refusal().unwrap();
    assert!(why.contains(BROKEN), "{why}");
    assert!(!why.contains(FLAKY), "only the new failure is named: {why}");
}

/// A new failure in a shard that was green at P is new too.
// trace:STORY-2912.acd07159 | ai:claude
#[test]
fn a_failure_in_a_shard_green_at_p_refuses() {
    let mut f = same_failure();
    f.checks
        .insert(C.into(), Ok(run_checks("200", &["lib-0", "cli-1"], &[])));
    f.put(
        "200",
        "cli-1",
        one_run("cli-1", &[("aida-cli::journey", "walks", true)]),
    );
    let d = decide(&mut f, C, Some(P));
    assert_eq!(d.new_failures, vec!["aida-cli::journey walks".to_string()]);
    assert!(!d.allowed());
}

/// Finding 1: gates and contracts run real tests but upload no per-test
/// evidence, so a red lane refuses even when P's same lane is red too (the
/// lanes may fail on different tests).
// trace:STORY-2912.acd07159 | ai:claude
#[test]
fn different_failures_in_gates_or_contracts_refuse() {
    for lane in ["CI (contracts)", "CI (gates)", "CI (fast gates)"] {
        let mut f = same_failure();
        f.checks
            .insert(C.into(), Ok(run_checks("200", &["lib-0"], &[lane])));
        f.checks
            .insert(P.into(), Ok(run_checks("100", &["lib-0"], &[lane])));
        let d = decide(&mut f, C, Some(P));
        assert!(!d.allowed(), "{lane}: {d:?}");
        assert!(d.reason.contains(lane), "{}", d.reason);
        assert!(d.reason.contains("no per-test evidence"), "{}", d.reason);
        assert!(d.candidate_failing.contains(&format!("check: {lane}")));
    }

    // The lane alone red at C (the aggregate beside it) refuses too.
    let mut f = same_failure();
    f.checks
        .insert(C.into(), Ok(run_checks("200", &[], &["CI (contracts)"])));
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(d.reason.contains("CI (contracts)"), "{}", d.reason);

    // Red only at P: C's green lane makes it fixed, not a blocker.
    let mut f = same_failure();
    f.checks.insert(
        P.into(),
        Ok(run_checks("100", &["lib-0"], &["CI (contracts)"])),
    );
    let d = decide(&mut f, C, Some(P));
    assert!(d.allowed(), "{d:?}");
    assert_eq!(d.fixed, vec!["check: CI (contracts)".to_string()]);
    assert!(d
        .promoted_failing
        .contains(&"check: CI (contracts)".to_string()));
}

/// Finding 2: a shared nextest failure plus a doctest failure new at C
/// refuses, naming the doctest.
// trace:STORY-2912.acd07159 | ai:claude
#[test]
fn shared_nextest_failure_plus_new_doctest_failure_refuses() {
    let mut f = Fake::default();
    f.checks
        .insert(C.into(), Ok(run_checks("200", &["rest"], &[])));
    f.checks
        .insert(P.into(), Ok(run_checks("100", &["rest"], &[])));
    f.put(
        "100",
        "rest",
        rest_run(&[FLAKY_CASE], &[], &[(DOC_OK, "ok"), (DOC_NEW, "ok")]),
    );
    f.put(
        "200",
        "rest",
        rest_run(&[FLAKY_CASE], &[], &[(DOC_OK, "ok"), (DOC_NEW, "FAILED")]),
    );
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed(), "{d:?}");
    let doc = format!("aida_cli_lib::doc {DOC_NEW}");
    assert_eq!(d.new_failures, vec![doc.clone()]);
    assert!(d.refusal().unwrap().contains(&doc));

    // The same doctest failing at P too: allowed.
    f.put(
        "100",
        "rest",
        rest_run(
            &[FLAKY_CASE],
            &[],
            &[(DOC_OK, "ignored"), (DOC_NEW, "FAILED")],
        ),
    );
    let d = decide(&mut f, C, Some(P));
    assert!(d.allowed(), "{d:?}");
    assert_eq!(d.candidate_failing, vec![FLAKY.to_string(), doc]);
}

// From test-timings-rest in GitHub Actions run 44408993587.
const EMPTY_BINS_JUNIT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="0" skipped="0" failures="0" errors="0" uuid="c32f4400-b82e-4f2b-b833-ee2713c4f0a5" timestamp="2026-11-18T22:20:29.087+00:00" time="0.001">
</testsuites>
"#;
const EMPTY_BINS_SUMMARY: &str = "\x1b[33;1m     Summary\x1b[0m [   0.001s] \x1b[1m0\x1b[0m tests run: \x1b[1m0\x1b[0m \x1b[32;1mpassed\x1b[0m, \x1b[1m0\x1b[0m \x1b[33;1mskipped\x1b[0m\n";

// trace:BUG-2695 | ai:codex
#[test]
fn empty_rest_bins_report_requires_matching_completion() {
    let mut artifact = rest_run(&[FLAKY_CASE], &[], &[(DOC_OK, "ok")]);
    let log = artifact.log.as_mut().unwrap();
    *log = log.replace(
        &nextest_log("-p aida-cli --bins", &[]),
        &format!(
            "+ cargo nextest run --no-fail-fast --profile ci --no-tests warn -p aida-cli --bins\n\
             {EMPTY_BINS_SUMMARY}\x1b[33;1mwarning\x1b[0m: no tests to run\n"
        ),
    );
    artifact
        .reports
        .insert("junit-rest-1.xml".into(), EMPTY_BINS_JUNIT.into());

    let mut fake = Fake::default();
    for (sha, run) in [(C, "200"), (P, "100")] {
        fake.checks
            .insert(sha.into(), Ok(run_checks(run, &["rest"], &[])));
        fake.put(run, "rest", artifact.clone());
    }
    let decision = decide(&mut fake, C, Some(P));
    assert!(decision.allowed(), "{decision:?}");
    assert_eq!(decision.candidate_failing, vec![FLAKY.to_string()]);

    let complete_log = artifact.log.clone().unwrap();
    for (old, new, expected) in [
        (EMPTY_BINS_SUMMARY, "", "printed no test summary"),
        (
            "] \x1b[1m0\x1b[0m tests run:",
            "] 0/1 tests run:",
            "stopped before every test ran",
        ),
        (
            "] \x1b[1m0\x1b[0m tests run:",
            "] 1 test run:",
            "is unreadable",
        ),
        ("-p aida-cli --bins", "-p aida-cli --lib", "is unreadable"),
        (
            "--no-tests warn -p aida-cli --bins",
            "--no-tests pass -p aida-cli --bins",
            "is unreadable",
        ),
    ] {
        let changed_log = complete_log.replace(old, new);
        assert_ne!(changed_log, complete_log, "fixture must contain {old:?}");
        artifact.log = Some(changed_log);
        let error = shard_evidence("rest", &artifact).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }

    artifact.log = Some(complete_log);
    for invalid in [
        String::new(),
        EMPTY_BINS_JUNIT.replace("</testsuites>", ""),
        EMPTY_BINS_JUNIT.replace("</testsuites>", "stray text</testsuites>"),
        EMPTY_BINS_JUNIT.replace("failures=\"0\"", "failures=\"1\""),
        EMPTY_BINS_JUNIT.replace("tests=\"0\"", "tests=\"1\" tests=\"0\""),
    ] {
        artifact.reports.insert("junit-rest-1.xml".into(), invalid);
        let error = shard_evidence("rest", &artifact).unwrap_err();
        assert!(
            error.contains("junit-rest-1.xml") && error.contains("unreadable"),
            "{error}"
        );
    }
}

/// Finding 2: partial or inconsistent shard evidence refuses, at C and at P.
// trace:STORY-2912.ac7366e2 | ai:claude
#[test]
fn partial_shard_evidence_refuses() {
    let full = || {
        rest_run(
            &[FLAKY_CASE],
            &[("aida-cli::bin/aida", "parses", false)],
            &[(DOC_OK, "ok")],
        )
    };
    let rest = || {
        let mut f = Fake::default();
        f.checks
            .insert(C.into(), Ok(run_checks("200", &["rest"], &[])));
        f.checks
            .insert(P.into(), Ok(run_checks("100", &["rest"], &[])));
        f.put("100", "rest", full());
        f.put("200", "rest", full());
        f
    };
    assert!(decide(&mut rest(), C, Some(P)).allowed());

    type Break = Box<dyn Fn(&mut ShardArtifact)>;
    let mut cases: Vec<(&str, Break, &str)> = vec![
        (
            "a later invocation's report is missing",
            Box::new(|a: &mut ShardArtifact| {
                a.reports.remove("junit-rest-1.xml");
            }),
            "need JUnit reports",
        ),
        (
            "an extra report",
            Box::new(|a: &mut ShardArtifact| {
                a.reports.insert("junit-rest-2.xml".into(), junit(&[]));
            }),
            "need JUnit reports",
        ),
        (
            "the log stops after the first doctest run",
            Box::new(|a: &mut ShardArtifact| {
                let log = a.log.take().unwrap();
                let cut = log.find("+ cargo nextest run --no-fail-fast --profile ci --no-tests warn -p aida-cli --bins").unwrap();
                a.log = Some(log[..cut].to_string());
                a.reports.remove("junit-rest-1.xml");
            }),
            "not the shard's full set",
        ),
        (
            "no log",
            Box::new(|a: &mut ShardArtifact| a.log = None),
            "test-run.log",
        ),
        (
            "a doctest run that never finished",
            Box::new(|a: &mut ShardArtifact| {
                let log = a.log.take().unwrap();
                let cut = log.rfind("test result:").unwrap();
                a.log = Some(log[..cut].to_string());
            }),
            "never finished",
        ),
        (
            "a doctest build failure",
            Box::new(|a: &mut ShardArtifact| {
                let log = a.log.take().unwrap();
                a.log = Some(log.replace(
                    "   Doc-tests aida_cli_lib",
                    "error: could not compile `aida-cli-lib` (lib) due to 1 previous error\n   Doc-tests aida_cli_lib",
                ));
            }),
            "could not compile",
        ),
        (
            "a summary disagreeing with its report",
            Box::new(|a: &mut ShardArtifact| {
                a.reports
                    .insert("junit-rest-0.xml".into(), junit(&[OK_CASE]));
            }),
            "reports 1 failed test(s)",
        ),
        (
            "a cancelled nextest run",
            Box::new(|a: &mut ShardArtifact| {
                let log = a.log.take().unwrap();
                a.log = Some(log.replacen("] 1 tests run:", "] 1/3 tests run:", 1));
            }),
            "stopped before every test ran",
        ),
        (
            "a nextest build failure",
            Box::new(|a: &mut ShardArtifact| {
                let log = a.log.take().unwrap();
                let first = log.find("     Summary").unwrap();
                let end = first + log[first..].find('\n').unwrap() + 1;
                a.log = Some(format!("{}{}", &log[..first], &log[end..]));
            }),
            "printed no test summary",
        ),
        (
            "a runner fallback to cargo test",
            Box::new(|a: &mut ShardArtifact| {
                let log = a.log.take().unwrap();
                a.log = Some(log.replacen(
                    "+ cargo nextest run --no-fail-fast --profile ci --no-tests warn",
                    "+ cargo test --no-fail-fast",
                    1,
                ));
            }),
            "unexpected command",
        ),
        (
            "a planning error",
            Box::new(|a: &mut ShardArtifact| {
                let log = a.log.take().unwrap();
                a.log = Some(format!("::error::cannot plan rest: boom\n{log}"));
            }),
            "cannot plan rest",
        ),
    ];
    for (what, break_it, expect) in cases.drain(..) {
        for (sha, run) in [(C, "200"), (P, "100")] {
            let mut f = rest();
            let mut a = full();
            break_it(&mut a);
            f.put(run, "rest", a);
            let d = decide(&mut f, C, Some(P));
            assert!(!d.allowed(), "{what} at {sha}: {d:?}");
            assert!(
                d.reason.contains("incomplete") && d.reason.contains(expect),
                "{what} at {sha}: {}",
                d.reason
            );
            assert!(d.reason.contains(&sha[..12]), "{what}: {}", d.reason);
        }
    }
}

/// A red shard whose job failed in another step than its test run, or
/// whose steps cannot be read, refuses: that failure is not a test's.
// trace:STORY-2912.ac7366e2 | ai:claude
#[test]
fn a_red_shard_failing_outside_its_test_run_refuses() {
    let key = ("200".to_string(), "CI (lib-0)".to_string());
    for (steps, expect) in [
        (
            Ok(vec![TEST_STEP.to_string(), "Test time budget".to_string()]),
            "`Test time budget`",
        ),
        (
            Ok(vec!["Test shard root hygiene".to_string()]),
            "root hygiene",
        ),
        (Ok(vec![]), "do not both name a failing test"),
        (Err("api down".to_string()), "api down"),
    ] {
        let mut f = same_failure();
        f.steps.insert(key.clone(), steps);
        let d = decide(&mut f, C, Some(P));
        assert!(!d.allowed(), "{expect}: {d:?}");
        assert!(d.reason.contains(expect), "{}", d.reason);
    }
}

/// AC3/AC5: missing or unreadable evidence for C refuses with a named reason.
// trace:STORY-2912.ac7366e2 | ai:claude
#[test]
fn missing_candidate_evidence_refuses() {
    // The shard's artifact is missing.
    let mut f = same_failure();
    f.artifacts.remove(&("200".into(), "lib-0".into()));
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
    let mut a = one_run("lib-0", &[FLAKY_CASE]);
    a.reports.insert(
        "junit-lib-0-0.xml".into(),
        "<testsuites><testsuite name=\"x\">".into(),
    );
    f.put("200", "lib-0", a);
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(d.reason.contains("unreadable"), "{}", d.reason);

    // A red shard whose complete evidence names no failing test.
    let mut f = same_failure();
    f.put("200", "lib-0", one_run("lib-0", &[OK_CASE]));
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(
        d.reason.contains("do not both name a failing test"),
        "{}",
        d.reason
    );

    // No commit at all.
    let d = decide(&mut same_failure(), "", Some(P));
    assert!(!d.allowed());
}

/// Pending CI and a missing required check refuse.
// trace:STORY-2912.ac7366e2 | ai:claude
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

/// Missing or partial evidence at P refuses too; P with no CI run refuses
/// (there is no local re-run fallback).
// trace:STORY-2912.ac7366e2 | ai:claude
#[test]
fn missing_promoted_evidence_refuses() {
    let mut f = same_failure();
    f.artifacts.remove(&("100".into(), "lib-0".into()));
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(d.reason.contains(&P[..12]), "{}", d.reason);

    let mut f = same_failure();
    f.checks.insert(P.into(), Err("rate limited".into()));
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(d.reason.contains("rate limited"), "{}", d.reason);

    let mut f = same_failure();
    f.checks.remove(P);
    let d = decide(&mut f, C, Some(P));
    assert!(!d.allowed());
    assert!(
        d.reason.contains("no recorded CI run") && d.reason.contains(&P[..12]),
        "{}",
        d.reason
    );

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

/// A non-lane check red at C must be red at P; the aggregate beside other
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
    f.put(
        "100",
        "rest",
        rest_run(
            &[("aida-mcp", "gone", true), ("aida-mcp", "healed", true)],
            &[],
            &[],
        ),
    );
    // C's green `rest` shard reports `healed` passing; `gone` is absent.
    f.put(
        "200",
        "rest",
        rest_run(&[("aida-mcp", "healed", false)], &[], &[]),
    );
    let d = decide(&mut f, C, Some(P));
    assert!(d.allowed(), "{d:?}");
    assert_eq!(d.fixed, vec!["aida-mcp healed".to_string()]);
    assert_eq!(d.unconfirmed, vec!["aida-mcp gone".to_string()]);
}

/// The decision round-trips through its JSON record, and concurrent
/// writers never clobber each other's temporary (finding 5).
// trace:STORY-2912.ac77195c | ai:claude
#[test]
fn decision_records_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let d = decide(&mut same_failure(), C, Some(P));
    record_last(dir.path(), &d).unwrap();
    assert_eq!(read_last(dir.path()), Some(d.clone()));

    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                for _ in 0..25 {
                    record_last(dir.path(), &d).expect("a concurrent write succeeds");
                }
            });
        }
    });
    assert_eq!(read_last(dir.path()), Some(d));
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|n| n != DECISION_FILE)
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
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

/// GitHub's job listing yields the not-succeeded steps of exactly one job.
#[test]
fn failed_job_steps_are_read_from_the_run() {
    let page = serde_json::json!({
        "total_count": 2,
        "jobs": [
            {"name": "CI (lib-0)", "status": "completed", "steps": [
                {"name": "Build aida binary for lib unit tests", "conclusion": "success"},
                {"name": "Run tests", "conclusion": "failure"},
                {"name": "Test time budget", "conclusion": "failure"},
                {"name": "Upload test timings", "conclusion": "success"},
                {"name": "Install zsh", "conclusion": "skipped"},
                {"name": "Post", "conclusion": null},
            ]},
            {"name": "CI (gates)", "status": "completed", "steps": []},
        ]
    });
    let mut calls = Vec::new();
    let mut api = |endpoint: &str| -> Result<Vec<u8>> {
        calls.push(endpoint.to_string());
        Ok(serde_json::to_vec(&page).unwrap())
    };
    let steps =
        crate::forge::github_run_job_failed_steps("o/r", "123", "CI (lib-0)", &mut api).unwrap();
    assert_eq!(steps, vec!["Run tests", "Test time budget", "Post"]);
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0].contains("actions/runs/123/jobs?filter=latest"),
        "{calls:?}"
    );

    let jobs = page["jobs"].as_array().unwrap();
    assert!(crate::forge::failed_steps_of(jobs, "CI (rest)").is_err());
    let twice = [jobs[0].clone(), jobs[0].clone()];
    assert!(crate::forge::failed_steps_of(&twice, "CI (lib-0)").is_err());
    let mut running = jobs[0].clone();
    running["status"] = "in_progress".into();
    assert!(crate::forge::failed_steps_of(&[running], "CI (lib-0)").is_err());
    let mut api = |_: &str| -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(&serde_json::json!({"total_count": 3, "jobs": jobs})).unwrap())
    };
    assert!(
        crate::forge::github_run_job_failed_steps("o/r", "123", "CI (lib-0)", &mut api).is_err(),
        "an incomplete listing is refused"
    );
    let mut never = |_: &str| -> Result<Vec<u8>> { panic!("no call for a bad run id") };
    assert!(crate::forge::github_run_job_failed_steps("o/r", "1;x", "j", &mut never).is_err());
}

/// The shard names, check names, step and artifact contents match the CI
/// workflow and its runner script, so the comparison cannot silently stop
/// finding its evidence.
#[test]
fn shards_match_the_ci_workflow() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let ci = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
    let matrix = format!("shard: [{}, gates, contracts]", TEST_SHARDS.join(", "));
    assert!(ci.contains(&matrix), "ci.yml matrix is not `{matrix}`");
    assert!(ci.contains("name: CI (${{ matrix.shard }})"));
    assert!(ci.contains(&format!("- name: {TEST_STEP}\n")));
    assert!(ci.contains("name: test-timings-${{ matrix.shard }}"));
    assert!(ci.contains("target/test-run.log\n            target/nextest/ci/junit-*.xml"));
    assert!(ci.contains("| tee target/test-run.log"));
    let script = std::fs::read_to_string(root.join("scripts/ci-test-shards.py")).unwrap();
    assert!(script.contains(&format!("SHARDS = ('{}')", TEST_SHARDS.join("', '"))));
    assert!(script.contains("f'junit-{shard}-{index}.xml'"));
    assert!(script.contains("print('+ ' + ' '.join(command), flush=True)"));
    assert_eq!(report_artifact("lib-0"), "test-timings-lib-0");
    assert_eq!(shard_of_check("CI (lib-0)"), Some("lib-0"));
    assert_eq!(shard_of_check("CI (gates)"), None);
}
