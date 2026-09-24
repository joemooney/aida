//! Red-first run for criterion-traced tests (STORY-1386, first slice).
//!
//! When a drain's implementer phase starts, the spec's criterion-traced tests
//! (found by the same tracer `aida criteria <SPEC>` prints — no second
//! tracer) are run BEFORE any implementation exists. The run is recorded per
//! criterion under `.aida/red-runs/<SPEC>.json` in the main worktree:
//!
//! - `red` — at least one traced test fails: the test discriminates.
//! - `already-satisfied` — every test that ran passed: the test cannot fail,
//!   or the criterion was already satisfied before implementation. Flagged.
//! - `not-run` — no traced test could be run (no runner, filter matched
//!   nothing, budget exhausted).
//!
//! The record is written ONCE per spec: a later run would observe the
//! implementation and falsely flag every criterion as already satisfied. It is
//! surfaced to the round-1 reviewer prompt and in `aida criteria`'s human
//! report. Specs with no criterion-traced tests are skipped (nothing written).
// trace:STORY-1386 | ai:claude

use crate::criteria::{CriteriaReport, CriterionState, TracedTest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Default wall-clock budget for the whole red run. Kept small: this is a
/// pre-implementation probe, not a test suite.
const DEFAULT_BUDGET_SECS: u64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", tag = "outcome", content = "detail")]
pub(crate) enum TestOutcome {
    /// The test ran and failed (or failed to build) — red.
    Failed,
    /// The test ran and passed — green before any implementation.
    Passed,
    /// The test could not be run; the string says why.
    NotRun(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CriterionVerdict {
    Red,
    AlreadySatisfied,
    NotRun,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct TestRun {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) line: usize,
    #[serde(flatten)]
    pub(crate) outcome: TestOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CriterionRedRun {
    pub(crate) id: String,
    pub(crate) text: String,
    pub(crate) verdict: CriterionVerdict,
    pub(crate) tests: Vec<TestRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RedRunRecord {
    pub(crate) spec: String,
    pub(crate) recorded_at: String,
    #[serde(default)]
    pub(crate) head_sha: Option<String>,
    pub(crate) criteria: Vec<CriterionRedRun>,
}

impl RedRunRecord {
    /// Criteria whose tests passed before any implementation existed.
    pub(crate) fn already_satisfied(&self) -> Vec<&CriterionRedRun> {
        self.criteria
            .iter()
            .filter(|c| c.verdict == CriterionVerdict::AlreadySatisfied)
            .collect()
    }
}

/// Runs one traced test. Injected so unit tests never spawn cargo.
pub(crate) trait TestRunner {
    fn run(&mut self, root: &Path, test: &TracedTest, timeout: Duration) -> TestOutcome;
}

/// Combine one criterion's test outcomes: any failure is red; otherwise any
/// pass means the test could not fail; otherwise nothing ran.
pub(crate) fn verdict_for(tests: &[TestRun]) -> CriterionVerdict {
    if tests.iter().any(|t| t.outcome == TestOutcome::Failed) {
        CriterionVerdict::Red
    } else if tests.iter().any(|t| t.outcome == TestOutcome::Passed) {
        CriterionVerdict::AlreadySatisfied
    } else {
        CriterionVerdict::NotRun
    }
}

/// Run every criterion-traced test once (a test traced to two criteria runs
/// once) within `budget`. Returns `None` when the spec has no traced tests —
/// the cheap skip.
pub(crate) fn run_red_run(
    root: &Path,
    report: &CriteriaReport,
    runner: &mut dyn TestRunner,
    budget: Duration,
) -> Option<Vec<CriterionRedRun>> {
    let traced: Vec<_> = report
        .criteria
        .iter()
        .filter(|row| row.state == CriterionState::Traced && !row.tests.is_empty())
        .collect();
    if traced.is_empty() {
        return None;
    }
    let started = Instant::now();
    let mut cache: BTreeMap<(String, String), TestOutcome> = BTreeMap::new();
    let mut out = Vec::new();
    for row in traced {
        let mut runs = Vec::new();
        for test in &row.tests {
            let key = (test.path.clone(), test.name.clone());
            let outcome = if let Some(done) = cache.get(&key) {
                done.clone()
            } else {
                let remaining = budget.saturating_sub(started.elapsed());
                let outcome = if remaining.is_zero() {
                    TestOutcome::NotRun("red-run budget exhausted".to_string())
                } else {
                    runner.run(root, test, remaining)
                };
                cache.insert(key, outcome.clone());
                outcome
            };
            runs.push(TestRun {
                name: test.name.clone(),
                path: test.path.clone(),
                line: test.line,
                outcome,
            });
        }
        out.push(CriterionRedRun {
            id: row.criterion.id.clone(),
            text: row.criterion.text.clone(),
            verdict: verdict_for(&runs),
            tests: runs,
        });
    }
    Some(out)
}

/// `AIDA_RED_RUN_BUDGET_SECS`: total wall-clock budget; `0` disables the red
/// run. Unset/unparsable → the default.
pub(crate) fn budget_from_env() -> Option<Duration> {
    let secs = std::env::var("AIDA_RED_RUN_BUDGET_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_BUDGET_SECS);
    (secs > 0).then(|| Duration::from_secs(secs))
}

fn safe_file_stem(spec: &str) -> String {
    spec.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn record_path(root: &Path, spec: &str) -> PathBuf {
    root.join(".aida").join("red-runs").join(format!(
        "{}.json",
        safe_file_stem(&spec.to_ascii_uppercase())
    ))
}

pub(crate) fn load_record(root: &Path, spec: &str) -> Option<RedRunRecord> {
    let raw = std::fs::read_to_string(record_path(root, spec)).ok()?;
    serde_json::from_str(&raw).ok()
}

pub(crate) fn save_record(root: &Path, record: &RedRunRecord) -> std::io::Result<PathBuf> {
    let path = record_path(root, &record.spec);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(record).map_err(std::io::Error::other)?;
    std::fs::write(&path, body)?;
    Ok(path)
}

/// Drain entry point: run and record once. Returns the record when one was
/// written now; `None` when skipped (disabled, already recorded, no traced
/// tests).
pub(crate) fn record_red_run_once(
    root: &Path,
    report: &CriteriaReport,
    runner: &mut dyn TestRunner,
    budget: Duration,
) -> Option<RedRunRecord> {
    if load_record(root, &report.spec).is_some() {
        return None;
    }
    let criteria = run_red_run(root, report, runner, budget)?;
    let record = RedRunRecord {
        spec: report.spec.clone(),
        recorded_at: chrono::Utc::now().to_rfc3339(),
        head_sha: git_head(root),
        criteria,
    };
    save_record(root, &record).ok()?;
    Some(record)
}

fn git_head(root: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// One line per criterion, used by the drain banner and `aida criteria`.
pub(crate) fn summary_lines(record: &RedRunRecord) -> Vec<String> {
    record
        .criteria
        .iter()
        .map(|c| {
            let label = match c.verdict {
                CriterionVerdict::Red => "red".to_string(),
                CriterionVerdict::AlreadySatisfied => {
                    "test cannot fail / already satisfied".to_string()
                }
                CriterionVerdict::NotRun => {
                    let why = c
                        .tests
                        .iter()
                        .find_map(|t| match &t.outcome {
                            TestOutcome::NotRun(why) => Some(why.as_str()),
                            _ => None,
                        })
                        .unwrap_or("not run");
                    format!("not run ({why})")
                }
            };
            format!("{} [{}] {}", c.id, label, c.text)
        })
        .collect()
}

/// Reviewer-prompt block: cite the pre-implementation red run. Criteria that
/// were green before the implementation are called out as tests that cannot
/// fail. `None` when no spec has a record.
pub(crate) fn reviewer_prompt_block(records: &[RedRunRecord]) -> Option<String> {
    if records.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    let mut flagged = Vec::new();
    for record in records {
        for line in summary_lines(record) {
            lines.push(format!("- {}: {line}", record.spec));
        }
        for c in record.already_satisfied() {
            flagged.push(format!("- {}: {} {}", record.spec, c.id, c.text));
        }
    }
    let mut block = format!(
        "\n\nPre-implementation red run (recorded when the implementer phase started, before \
         any implementation): each criterion's traced tests were run on the base commit.\n{}\n",
        lines.join("\n")
    );
    if !flagged.is_empty() {
        block.push_str(&format!(
            "\nThe following criteria's tests PASSED before the implementation existed — the \
             test cannot fail, or the criterion was already satisfied. Check whether the test \
             asserts the criterion at all; a green-before-and-after test is not evidence.\n{}\n",
            flagged.join("\n")
        ));
    }
    Some(block)
}

// ---------------------------------------------------------------------------
// Real runner: cargo (Rust), pytest (Python), go test (Go). JS/TS has no
// single runner convention, so it is reported not-run rather than guessed.
// ---------------------------------------------------------------------------

pub(crate) struct CommandTestRunner;

/// The command that runs exactly one traced test, or why none can.
pub(crate) fn plan_command(
    root: &Path,
    test: &TracedTest,
) -> Result<(String, Vec<String>, PathBuf), String> {
    let path = Path::new(&test.path);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "rs" => {
            let pkg = cargo_package_for(root, path)
                .ok_or_else(|| "no Cargo package found for the test file".to_string())?;
            Ok((
                "cargo".to_string(),
                vec![
                    "test".to_string(),
                    "-q".to_string(),
                    "-p".to_string(),
                    pkg,
                    test.name.clone(),
                ],
                root.to_path_buf(),
            ))
        }
        "py" => Ok((
            "python3".to_string(),
            vec![
                "-m".to_string(),
                "pytest".to_string(),
                "-q".to_string(),
                format!("{}::{}", test.path, test.name),
            ],
            root.to_path_buf(),
        )),
        "go" => {
            let dir = path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let pkg = if dir.is_empty() {
                "./".to_string()
            } else {
                format!("./{dir}")
            };
            Ok((
                "go".to_string(),
                vec![
                    "test".to_string(),
                    "-run".to_string(),
                    format!("^{}$", test.name),
                    pkg,
                ],
                root.to_path_buf(),
            ))
        }
        _ => Err("no runner configured for this test language".to_string()),
    }
}

/// Walk up from the test file to the nearest `Cargo.toml` with a
/// `[package] name`.
fn cargo_package_for(root: &Path, rel: &Path) -> Option<String> {
    let mut dir = root.join(rel);
    while dir.pop() {
        let manifest = dir.join("Cargo.toml");
        if let Ok(raw) = std::fs::read_to_string(&manifest) {
            if let Ok(value) = raw.parse::<toml::Table>() {
                if let Some(name) = value
                    .get("package")
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                {
                    return Some(name.to_string());
                }
            }
        }
        if !dir.starts_with(root) || dir == root {
            break;
        }
    }
    None
}

/// Classify a finished run. A zero exit that ran no test is `NotRun` — a
/// filter matching nothing must never read as "already satisfied".
pub(crate) fn classify_output(
    program: &str,
    success: bool,
    code: Option<i32>,
    out: &str,
) -> TestOutcome {
    match program {
        "cargo" => {
            let passed: u64 = out
                .lines()
                .filter_map(|l| l.split("test result:").nth(1))
                .filter_map(|rest| {
                    rest.split(';')
                        .next()
                        .and_then(|seg| seg.split_whitespace().rev().nth(1))
                        .and_then(|n| n.parse::<u64>().ok())
                })
                .sum();
            if !success {
                TestOutcome::Failed
            } else if passed == 0 {
                TestOutcome::NotRun("the test filter matched no test".to_string())
            } else {
                TestOutcome::Passed
            }
        }
        "python3" => match code {
            Some(0) => TestOutcome::Passed,
            Some(5) => TestOutcome::NotRun("pytest collected no test".to_string()),
            _ => TestOutcome::Failed,
        },
        "go" => {
            if !success {
                TestOutcome::Failed
            } else if out.contains("no tests to run") {
                TestOutcome::NotRun("the test filter matched no test".to_string())
            } else {
                TestOutcome::Passed
            }
        }
        _ => {
            if success {
                TestOutcome::Passed
            } else {
                TestOutcome::Failed
            }
        }
    }
}

impl TestRunner for CommandTestRunner {
    fn run(&mut self, root: &Path, test: &TracedTest, timeout: Duration) -> TestOutcome {
        let (program, args, cwd) = match plan_command(root, test) {
            Ok(plan) => plan,
            Err(why) => return TestOutcome::NotRun(why),
        };
        let mut child = match Command::new(&program)
            .args(&args)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => return TestOutcome::NotRun(format!("`{program}` could not start: {e}")),
        };
        // Drain both pipes on threads so a chatty test cannot block on a full
        // pipe while the deadline loop polls.
        let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
            std::thread::spawn(move || {
                let mut buf = String::new();
                if let Some(mut pipe) = pipe {
                    let _ = pipe.read_to_string(&mut buf);
                }
                buf
            })
        };
        let out_thread = drain(
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        );
        let err_thread = drain(
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        );
        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return TestOutcome::NotRun("red-run budget exhausted".to_string());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(200)),
                Err(e) => return TestOutcome::NotRun(format!("wait failed: {e}")),
            }
        };
        let mut out = out_thread.join().unwrap_or_default();
        out.push_str(&err_thread.join().unwrap_or_default());
        classify_output(&program, status.success(), status.code(), &out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::criteria::build_criteria_report;

    struct FakeRunner {
        outcomes: BTreeMap<String, TestOutcome>,
        calls: Vec<String>,
    }

    impl TestRunner for FakeRunner {
        fn run(&mut self, _root: &Path, test: &TracedTest, _timeout: Duration) -> TestOutcome {
            self.calls.push(test.name.clone());
            self.outcomes
                .get(&test.name)
                .cloned()
                .unwrap_or(TestOutcome::NotRun("unknown".into()))
        }
    }

    fn fixture(spec_desc: &str, test_src: &str) -> (tempfile::TempDir, CriteriaReport) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("tests/red.rs"), test_src).unwrap();
        let report = build_criteria_report(dir.path(), "RED-1", spec_desc).unwrap();
        (dir, report)
    }

    const DESC: &str = "## Acceptance\n- AC1: first\n- AC2: second\n- AC3: third\n";

    // trace:STORY-1386 | ai:claude
    #[test]
    fn records_red_and_flags_already_satisfied_per_criterion() {
        let (dir, report) = fixture(
            DESC,
            "// trace:RED-1.AC1 | ai:claude\n#[test]\nfn fails_first() {\n}\n\n\
             // trace:RED-1.AC2 | ai:claude\n#[test]\nfn passes_first() {\n}\n",
        );
        let mut runner = FakeRunner {
            outcomes: BTreeMap::from([
                ("fails_first".into(), TestOutcome::Failed),
                ("passes_first".into(), TestOutcome::Passed),
            ]),
            calls: vec![],
        };
        let record = record_red_run_once(dir.path(), &report, &mut runner, Duration::from_secs(60))
            .expect("traced tests produce a record");
        // AC3 is untraced: it is not part of the red run.
        assert_eq!(record.criteria.len(), 2);
        assert_eq!(record.criteria[0].verdict, CriterionVerdict::Red);
        assert_eq!(
            record.criteria[1].verdict,
            CriterionVerdict::AlreadySatisfied
        );
        let lines = summary_lines(&record);
        assert!(lines[1].contains("test cannot fail / already satisfied"));
        // Persisted and readable.
        assert_eq!(load_record(dir.path(), "RED-1"), Some(record));
    }

    // trace:STORY-1386 | ai:claude
    #[test]
    fn records_once_so_a_post_implementation_run_cannot_overwrite() {
        let (dir, report) = fixture(
            DESC,
            "// trace:RED-1.AC1 | ai:claude\n#[test]\nfn t() {\n}\n",
        );
        let mut red = FakeRunner {
            outcomes: BTreeMap::from([("t".into(), TestOutcome::Failed)]),
            calls: vec![],
        };
        assert!(
            record_red_run_once(dir.path(), &report, &mut red, Duration::from_secs(9)).is_some()
        );
        let mut green = FakeRunner {
            outcomes: BTreeMap::from([("t".into(), TestOutcome::Passed)]),
            calls: vec![],
        };
        assert!(
            record_red_run_once(dir.path(), &report, &mut green, Duration::from_secs(9)).is_none()
        );
        assert!(green.calls.is_empty(), "no re-run once recorded");
        let kept = load_record(dir.path(), "RED-1").unwrap();
        assert_eq!(kept.criteria[0].verdict, CriterionVerdict::Red);
    }

    // trace:STORY-1386 | ai:claude
    #[test]
    fn skips_specs_with_no_criterion_traced_tests() {
        let (dir, report) = fixture(DESC, "#[test]\nfn untraced() {\n}\n");
        let mut runner = FakeRunner {
            outcomes: BTreeMap::new(),
            calls: vec![],
        };
        assert!(
            record_red_run_once(dir.path(), &report, &mut runner, Duration::from_secs(9)).is_none()
        );
        assert!(runner.calls.is_empty());
        assert!(!record_path(dir.path(), "RED-1").exists());
    }

    // trace:STORY-1386 | ai:claude
    #[test]
    fn shared_test_runs_once_and_zero_budget_is_not_run() {
        let (dir, report) = fixture(
            DESC,
            "#[test]\nfn shared() {\n    // trace:RED-1.AC1 | ai:claude\n    // trace:RED-1.AC2 | ai:claude\n}\n",
        );
        let mut runner = FakeRunner {
            outcomes: BTreeMap::from([("shared".into(), TestOutcome::Failed)]),
            calls: vec![],
        };
        let rows = run_red_run(dir.path(), &report, &mut runner, Duration::from_secs(9)).unwrap();
        assert_eq!(runner.calls.len(), 1);
        assert!(rows.iter().all(|r| r.verdict == CriterionVerdict::Red));

        let mut none = FakeRunner {
            outcomes: BTreeMap::new(),
            calls: vec![],
        };
        let rows = run_red_run(dir.path(), &report, &mut none, Duration::ZERO).unwrap();
        assert!(none.calls.is_empty());
        assert!(rows.iter().all(|r| r.verdict == CriterionVerdict::NotRun));
    }

    // trace:STORY-1386 | ai:claude
    #[test]
    fn a_filter_matching_nothing_is_not_run_rather_than_green() {
        assert_eq!(
            classify_output(
                "cargo",
                true,
                Some(0),
                "test result: ok. 0 passed; 0 failed;"
            ),
            TestOutcome::NotRun("the test filter matched no test".into())
        );
        assert_eq!(
            classify_output(
                "cargo",
                true,
                Some(0),
                "test result: ok. 1 passed; 0 failed;"
            ),
            TestOutcome::Passed
        );
        assert_eq!(
            classify_output("cargo", false, Some(101), ""),
            TestOutcome::Failed
        );
        assert!(matches!(
            classify_output("python3", true, Some(5), ""),
            TestOutcome::NotRun(_)
        ));
        assert!(matches!(
            classify_output("go", true, Some(0), "testing: warning: no tests to run"),
            TestOutcome::NotRun(_)
        ));
    }

    // trace:STORY-1386 | ai:claude
    #[test]
    fn plan_command_resolves_the_cargo_package_and_refuses_js() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("crate-a/src")).unwrap();
        std::fs::write(
            dir.path().join("crate-a/Cargo.toml"),
            "[package]\nname = \"crate-a\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let test = |path: &str| TracedTest {
            name: "my_test".into(),
            path: path.into(),
            line: 1,
            traces: vec![],
        };
        let (program, args, _) = plan_command(dir.path(), &test("crate-a/src/lib.rs")).unwrap();
        assert_eq!(program, "cargo");
        assert_eq!(args, ["test", "-q", "-p", "crate-a", "my_test"]);
        assert!(plan_command(dir.path(), &test("web/a.test.ts")).is_err());
    }

    // trace:STORY-1386 | ai:claude
    #[test]
    fn reviewer_block_names_already_satisfied_criteria() {
        let record = RedRunRecord {
            spec: "RED-1".into(),
            recorded_at: "now".into(),
            head_sha: None,
            criteria: vec![CriterionRedRun {
                id: "RED-1.AC1".into(),
                text: "first".into(),
                verdict: CriterionVerdict::AlreadySatisfied,
                tests: vec![],
            }],
        };
        let block = reviewer_prompt_block(&[record]).unwrap();
        assert!(block.contains("Pre-implementation red run"));
        assert!(block.contains("PASSED before the implementation existed"));
        assert!(block.contains("RED-1.AC1 first"));
        assert!(reviewer_prompt_block(&[]).is_none());
    }
}
