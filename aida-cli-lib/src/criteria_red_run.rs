//! Red-first run for criterion-traced tests (STORY-1386, first slice).
//!
//! When a drain's implementer phase starts, the spec's criterion-traced tests
//! (found by the same tracer `aida criteria <SPEC>` prints — no second
//! tracer) are run by `aida queue work` in the lane's OWN fresh worktree,
//! right after the worktree + lease are created and before the implementer
//! agent launches, with a per-lane `CARGO_TARGET_DIR` inside that
//! worktree. It never runs in the operator's main checkout. The run is
//! recorded per criterion under `.aida/red-runs/<SPEC>.json` in the main
//! worktree (written atomically):
//!
//! - `red` — at least one traced test fails: the test discriminates.
//! - `already-satisfied` — every test that ran passed: the test cannot fail,
//!   or the criterion was already satisfied before implementation. Flagged.
//! - `not-run` — no traced test could be run (no runner, filter matched
//!   nothing, crash/signal, timeout — a timeout is recorded as `timed-out`).
//!
//! OPT-IN: off unless `AIDA_RED_RUN_BUDGET_SECS` is a positive number or
//! `[drain] red_run = true` is set in `.aida/config.toml` (120s budget).
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

/// Budget when `[drain] red_run = true` enables the run without an explicit
/// `AIDA_RED_RUN_BUDGET_SECS`.
const CONFIG_BUDGET_SECS: u64 = 120;

/// Bound on the small `git` probes (HEAD, cleanliness).
const GIT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", tag = "outcome", content = "detail")]
pub(crate) enum TestOutcome {
    /// The test ran and failed (the runner's own failure exit) — red.
    Failed,
    /// The test ran and passed — green before any implementation.
    Passed,
    /// The run hit its time budget and the process group was killed.
    TimedOut,
    /// The test could not be run or its result is unknown; the string says
    /// why. PRIN-5: unknown is never red.
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
    /// True when any of this criterion's tests hit the time budget.
    #[serde(default)]
    pub(crate) timed_out: bool,
    pub(crate) tests: Vec<TestRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RedRunRecord {
    pub(crate) spec: String,
    pub(crate) recorded_at: String,
    /// The lane worktree the tests ran in.
    #[serde(default)]
    pub(crate) workspace: Option<String>,
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

/// Runs one traced test inside `workspace`. Injected so unit tests never
/// spawn cargo.
pub(crate) trait TestRunner {
    fn run(&mut self, workspace: &Path, test: &TracedTest, timeout: Duration) -> TestOutcome;
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
/// once) in `workspace` within `budget`. Returns `None` when the spec has no
/// traced tests — the cheap skip.
pub(crate) fn run_red_run(
    workspace: &Path,
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
                    TestOutcome::TimedOut
                } else {
                    runner.run(workspace, test, remaining)
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
            timed_out: runs.iter().any(|t| t.outcome == TestOutcome::TimedOut),
            tests: runs,
        });
    }
    Some(out)
}

/// The red-run budget, or `None` when it is off (the default).
/// `AIDA_RED_RUN_BUDGET_SECS` wins when set (`0` = off); otherwise
/// `[drain] red_run = true` enables it with a 120s budget.
pub(crate) fn budget(project_root: &Path) -> Option<Duration> {
    budget_from(
        std::env::var("AIDA_RED_RUN_BUDGET_SECS").ok().as_deref(),
        project_root,
    )
}

fn budget_from(env: Option<&str>, project_root: &Path) -> Option<Duration> {
    if let Some(secs) = env.and_then(|v| v.trim().parse::<u64>().ok()) {
        return (secs > 0).then(|| Duration::from_secs(secs));
    }
    let enabled = crate::read_project_config_value(project_root)
        .and_then(|v| {
            v.get("drain")
                .and_then(|d| d.get("red_run"))
                .and_then(|b| b.as_bool())
        })
        .unwrap_or(false);
    enabled.then(|| Duration::from_secs(CONFIG_BUDGET_SECS))
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

/// Atomic per-spec write: a temp file in the same directory, then rename.
pub(crate) fn save_record(root: &Path, record: &RedRunRecord) -> std::io::Result<PathBuf> {
    let path = record_path(root, &record.spec);
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("record path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let body = serde_json::to_string_pretty(record).map_err(std::io::Error::other)?;
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("record"),
        std::process::id()
    ));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Drain entry point: run the traced tests in the lane `workspace` and record
/// the result once under `record_root` (the main worktree). Returns the
/// record when one was written now; `None` when skipped (already recorded,
/// no traced tests, write failure).
pub(crate) fn record_red_run_once(
    record_root: &Path,
    workspace: &Path,
    report: &CriteriaReport,
    runner: &mut dyn TestRunner,
    budget: Duration,
) -> Option<RedRunRecord> {
    if load_record(record_root, &report.spec).is_some() {
        return None;
    }
    let criteria = run_red_run(workspace, report, runner, budget)?;
    let record = RedRunRecord {
        spec: report.spec.clone(),
        recorded_at: chrono::Utc::now().to_rfc3339(),
        workspace: Some(workspace.display().to_string()),
        head_sha: git_head(workspace),
        criteria,
    };
    save_record(record_root, &record).ok()?;
    Some(record)
}

fn git_probe(dir: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args).stdin(Stdio::null());
    let out = crate::command_output_with_timeout(cmd, GIT_PROBE_TIMEOUT)?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_head(dir: &Path) -> Option<String> {
    git_probe(dir, &["rev-parse", "HEAD"])
}

/// The default-branch tip as seen from `main_root`: the first of
/// `origin/HEAD`, `origin/main`, `origin/master`, `main`, `master` that
/// resolves. Bounded git probes.
fn default_branch_tip(main_root: &Path) -> Option<String> {
    [
        "origin/HEAD",
        "origin/main",
        "origin/master",
        "main",
        "master",
    ]
    .iter()
    .find_map(|r| {
        git_probe(
            main_root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{r}^{{commit}}"),
            ],
        )
        .filter(|sha| !sha.is_empty())
    })
}

/// A lane worktree the red run may use: it exists, it is not the main
/// checkout, its HEAD IS the default-branch tip (no commits ahead — a retry
/// or rework lane that already carries implementation commits would make
/// every criterion read "cannot fail", PRIN-5), and it has no tracked
/// changes (the implementer has not touched it yet).
pub(crate) fn lane_is_fresh(main_root: &Path, workspace: &Path) -> bool {
    let (Ok(main), Ok(lane)) = (main_root.canonicalize(), workspace.canonicalize()) else {
        return false;
    };
    if main == lane {
        return false;
    }
    let (Some(head), Some(tip)) = (git_head(&lane), default_branch_tip(&main)) else {
        return false;
    };
    if head != tip {
        return false;
    }
    git_probe(&lane, &["status", "--porcelain", "--untracked-files=no"])
        .is_some_and(|s| s.is_empty())
}

/// Record the red run for a freshly created lane: skipped unless the lane is
/// fresh and the spec has criterion-traced tests. `description` is the
/// spec's description (its `## Acceptance` section).
pub(crate) fn record_for_lane(
    main_root: &Path,
    workspace: &Path,
    spec: &str,
    description: &str,
    runner: &mut dyn TestRunner,
    budget: Duration,
) -> Option<RedRunRecord> {
    if !lane_is_fresh(main_root, workspace) {
        return None;
    }
    let report = crate::criteria::build_criteria_report(workspace, spec, description).ok()?;
    record_red_run_once(main_root, workspace, &report, runner, budget)
}

/// `aida queue work` hook: called right after the lane's worktree and lease
/// are created and BEFORE the implementer agent launches. Opt-in (see
/// [`budget`]) and best-effort — every failure degrades to "no record".
pub(crate) fn after_lane_created(project_root: &Path, workspace: &Path, spec: &str) {
    let main_root = crate::main_worktree_root_from(project_root);
    let Some(budget) = budget(&main_root) else {
        return;
    };
    let Some(store) = crate::load_store_for_lookup(&main_root) else {
        return;
    };
    let Some(req) = store.requirements.iter().find(|r| {
        r.spec_id
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case(spec))
    }) else {
        return;
    };
    let mut runner = CommandTestRunner;
    if let Some(record) = record_for_lane(
        &main_root,
        workspace,
        spec,
        &req.description,
        &mut runner,
        budget,
    ) {
        eprintln!("  red run (before implementation):");
        for line in summary_lines(&record) {
            eprintln!("    {line}");
        }
    }
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
                CriterionVerdict::NotRun if c.timed_out => "not run (timed out)".to_string(),
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

/// Runs tests in the lane worktree with `CARGO_TARGET_DIR` pinned inside it,
/// so the run never shares or clobbers the operator's main `target/`.
pub(crate) struct CommandTestRunner;

/// The command that runs exactly one traced test, or why none can.
pub(crate) fn plan_command(
    workspace: &Path,
    test: &TracedTest,
) -> Result<(String, Vec<String>), String> {
    let path = Path::new(&test.path);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "rs" => {
            let pkg = cargo_package_for(workspace, path)
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

/// Classify a finished run. PRIN-5: only the runner's own test-failure exit
/// is red. A signal kill (`code == None`), an unexpected exit code, pytest
/// usage/internal errors (2-4), a missing pytest module, or a zero exit that
/// ran no test are all `NotRun` — unknown is never evidence.
pub(crate) fn classify_output(program: &str, code: Option<i32>, out: &str) -> TestOutcome {
    let Some(code) = code else {
        return TestOutcome::NotRun("killed by a signal".to_string());
    };
    match (program, code) {
        ("cargo", 0) => {
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
            if passed == 0 {
                TestOutcome::NotRun("the test filter matched no test".to_string())
            } else {
                TestOutcome::Passed
            }
        }
        ("cargo", 101) => TestOutcome::Failed,
        ("python3", _) if out.contains("No module named pytest") => {
            TestOutcome::NotRun("pytest is not installed".to_string())
        }
        ("python3", 0) => TestOutcome::Passed,
        ("python3", 1) => TestOutcome::Failed,
        ("python3", 5) => TestOutcome::NotRun("pytest collected no test".to_string()),
        ("go", 0) if out.contains("no tests to run") => {
            TestOutcome::NotRun("the test filter matched no test".to_string())
        }
        ("go", 0) => TestOutcome::Passed,
        ("go", 1) if out.contains("--- FAIL") => TestOutcome::Failed,
        (_, code) => TestOutcome::NotRun(format!("`{program}` exited {code}")),
    }
}

/// Run one planned command in `cwd` with `CARGO_TARGET_DIR` pinned, bounded
/// by `timeout`. Reuses [`crate::command_output_with_timeout`], which spawns
/// in its own process group and kills the whole group on timeout.
pub(crate) fn execute(
    program: &str,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
) -> TestOutcome {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .env("CARGO_TARGET_DIR", cwd.join("target"))
        .stdin(Stdio::null());
    let started = Instant::now();
    match crate::command_output_with_timeout(cmd, timeout) {
        Some(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).to_string();
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            classify_output(program, out.status.code(), &text)
        }
        // `None` is a timeout (group killed) or a spawn failure; the elapsed
        // time tells them apart.
        None if started.elapsed() >= timeout => TestOutcome::TimedOut,
        None => TestOutcome::NotRun(format!("`{program}` could not start")),
    }
}

impl TestRunner for CommandTestRunner {
    fn run(&mut self, workspace: &Path, test: &TracedTest, timeout: Duration) -> TestOutcome {
        match plan_command(workspace, test) {
            Ok((program, args)) => execute(&program, &args, workspace, timeout),
            Err(why) => TestOutcome::NotRun(why),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::criteria::build_criteria_report;

    struct FakeRunner {
        outcomes: BTreeMap<String, TestOutcome>,
        calls: Vec<String>,
        workspaces: Vec<PathBuf>,
    }

    fn fake(outcomes: &[(&str, TestOutcome)]) -> FakeRunner {
        FakeRunner {
            outcomes: outcomes
                .iter()
                .map(|(n, o)| (n.to_string(), o.clone()))
                .collect(),
            calls: vec![],
            workspaces: vec![],
        }
    }

    impl TestRunner for FakeRunner {
        fn run(&mut self, workspace: &Path, test: &TracedTest, _timeout: Duration) -> TestOutcome {
            self.calls.push(test.name.clone());
            self.workspaces.push(workspace.to_path_buf());
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
        let mut runner = fake(&[
            ("fails_first", TestOutcome::Failed),
            ("passes_first", TestOutcome::Passed),
        ]);
        let record = record_red_run_once(
            dir.path(),
            dir.path(),
            &report,
            &mut runner,
            Duration::from_secs(60),
        )
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
        let mut red = fake(&[("t", TestOutcome::Failed)]);
        assert!(record_red_run_once(
            dir.path(),
            dir.path(),
            &report,
            &mut red,
            Duration::from_secs(9)
        )
        .is_some());
        let mut green = fake(&[("t", TestOutcome::Passed)]);
        assert!(record_red_run_once(
            dir.path(),
            dir.path(),
            &report,
            &mut green,
            Duration::from_secs(9)
        )
        .is_none());
        assert!(green.calls.is_empty(), "no re-run once recorded");
        let kept = load_record(dir.path(), "RED-1").unwrap();
        assert_eq!(kept.criteria[0].verdict, CriterionVerdict::Red);
    }

    // trace:STORY-1386 | ai:claude
    #[test]
    fn skips_specs_with_no_criterion_traced_tests() {
        let (dir, report) = fixture(DESC, "#[test]\nfn untraced() {\n}\n");
        let mut runner = fake(&[]);
        assert!(record_red_run_once(
            dir.path(),
            dir.path(),
            &report,
            &mut runner,
            Duration::from_secs(9)
        )
        .is_none());
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
        let mut runner = fake(&[("shared", TestOutcome::Failed)]);
        let rows = run_red_run(dir.path(), &report, &mut runner, Duration::from_secs(9)).unwrap();
        assert_eq!(runner.calls.len(), 1);
        assert!(rows.iter().all(|r| r.verdict == CriterionVerdict::Red));

        let mut none = fake(&[]);
        let rows = run_red_run(dir.path(), &report, &mut none, Duration::ZERO).unwrap();
        assert!(none.calls.is_empty());
        assert!(rows.iter().all(|r| r.verdict == CriterionVerdict::NotRun));
    }

    // trace:STORY-1386 | ai:claude
    #[test]
    fn a_filter_matching_nothing_is_not_run_rather_than_green() {
        assert_eq!(
            classify_output("cargo", Some(0), "test result: ok. 0 passed; 0 failed;"),
            TestOutcome::NotRun("the test filter matched no test".into())
        );
        assert_eq!(
            classify_output("cargo", Some(0), "test result: ok. 1 passed; 0 failed;"),
            TestOutcome::Passed
        );
        assert_eq!(classify_output("cargo", Some(101), ""), TestOutcome::Failed);
        assert_eq!(
            classify_output("python3", Some(1), "1 failed"),
            TestOutcome::Failed
        );
        assert!(matches!(
            classify_output("python3", Some(5), ""),
            TestOutcome::NotRun(_)
        ));
        assert!(matches!(
            classify_output("go", Some(0), "testing: warning: no tests to run"),
            TestOutcome::NotRun(_)
        ));
    }

    // PRIN-5: unknown results are never red.
    // trace:STORY-1386 | ai:claude
    #[test]
    fn signals_unexpected_exits_and_missing_pytest_are_not_run() {
        for program in ["cargo", "python3", "go"] {
            assert!(matches!(
                classify_output(program, None, ""),
                TestOutcome::NotRun(_)
            ));
        }
        for code in 2..=4 {
            assert!(matches!(
                classify_output("python3", Some(code), ""),
                TestOutcome::NotRun(_)
            ));
        }
        assert!(matches!(
            classify_output(
                "python3",
                Some(1),
                "/usr/bin/python3: No module named pytest"
            ),
            TestOutcome::NotRun(_)
        ));
        assert!(matches!(
            classify_output("cargo", Some(1), ""),
            TestOutcome::NotRun(_)
        ));
        assert!(matches!(
            classify_output("go", Some(2), ""),
            TestOutcome::NotRun(_)
        ));
    }

    // trace:STORY-1386 | ai:claude
    #[test]
    fn runs_in_the_lane_worktree_and_records_under_main() {
        let main = tempfile::tempdir().unwrap();
        let lane = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(lane.path().join("tests")).unwrap();
        std::fs::write(
            lane.path().join("tests/red.rs"),
            "// trace:RED-1.AC1 | ai:claude\n#[test]\nfn lane_test() {\n}\n",
        )
        .unwrap();
        let report = build_criteria_report(lane.path(), "RED-1", DESC).unwrap();
        let mut runner = fake(&[("lane_test", TestOutcome::Failed)]);
        let record = record_red_run_once(
            main.path(),
            lane.path(),
            &report,
            &mut runner,
            Duration::from_secs(9),
        )
        .unwrap();
        assert_eq!(runner.workspaces, vec![lane.path().to_path_buf()]);
        assert_eq!(
            record.workspace.as_deref(),
            Some(&*lane.path().display().to_string())
        );
        assert!(record_path(main.path(), "RED-1").exists());
        assert!(!record_path(lane.path(), "RED-1").exists());
        // The main checkout itself is never an acceptable lane.
        assert!(!lane_is_fresh(main.path(), main.path()));
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    /// A main repo on `main` with one commit holding a criterion-traced test,
    /// plus a lane worktree forked at the main tip.
    fn repo_with_lane() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let lane = tmp.path().join("lane");
        std::fs::create_dir_all(main.join("tests")).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(
            main.join("tests/red.rs"),
            "// trace:RED-1.AC1 | ai:claude\n#[test]\nfn lane_test() {\n}\n",
        )
        .unwrap();
        git(&main, &["add", "."]);
        git(&main, &["commit", "-q", "-m", "base"]);
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "lane",
                lane.to_str().unwrap(),
            ],
        );
        (tmp, main, lane)
    }

    // The hook `aida queue work` calls after worktree creation fires for a
    // fresh lane at the main tip.
    // trace:STORY-1386 | ai:claude
    #[test]
    fn a_fresh_lane_at_the_main_tip_is_recorded() {
        let (_tmp, main, lane) = repo_with_lane();
        let mut runner = fake(&[("lane_test", TestOutcome::Failed)]);
        let record = record_for_lane(
            &main,
            &lane,
            "RED-1",
            DESC,
            &mut runner,
            Duration::from_secs(9),
        )
        .expect("fresh lane records");
        assert_eq!(runner.workspaces, vec![lane.clone()]);
        assert_eq!(record.criteria[0].verdict, CriterionVerdict::Red);
        assert!(record_path(&main, "RED-1").exists());
    }

    // PRIN-5: a retry/rework lane that already carries commits would read
    // every criterion as "cannot fail" — it is not fresh and is skipped.
    // trace:STORY-1386 | ai:claude
    #[test]
    fn a_clean_lane_ahead_of_main_is_not_fresh_and_is_skipped() {
        let (_tmp, main, lane) = repo_with_lane();
        std::fs::write(lane.join("impl.rs"), "fn implemented() {}\n").unwrap();
        git(&lane, &["add", "."]);
        git(&lane, &["commit", "-q", "-m", "implementation"]);
        assert!(!lane_is_fresh(&main, &lane));
        let mut runner = fake(&[("lane_test", TestOutcome::Passed)]);
        assert!(record_for_lane(
            &main,
            &lane,
            "RED-1",
            DESC,
            &mut runner,
            Duration::from_secs(9)
        )
        .is_none());
        assert!(runner.calls.is_empty());
        assert!(!record_path(&main, "RED-1").exists());
    }

    // The hook lives in `aida queue work`, after the worktree + lease are
    // created (session_start + lease lookup) and before the implementer
    // agent launches.
    // trace:STORY-1386 | ai:claude
    #[test]
    fn queue_work_calls_the_hook_after_worktree_creation_before_launch() {
        let src = include_str!("queue_cmd.rs");
        let start = src.find("pub(crate) fn handle_queue_work(").unwrap();
        let body = &src[start..];
        let created = body.find("    session_start(").unwrap();
        let lease = body.find("let lease = list_leases(&project_root)").unwrap();
        let hook = body.find("criteria_red_run::after_lane_created(").unwrap();
        let launch = body.find("session::exec_claude_with_session(").unwrap();
        assert!(created < lease && lease < hook && hook < launch);
        let auto = include_str!("auto_complete.rs");
        assert!(!auto.contains(&["record_red", "_run("].concat()));
    }

    // trace:STORY-1386 | ai:claude
    #[test]
    fn red_run_is_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(budget_from(None, dir.path()), None);
        assert_eq!(budget_from(Some("0"), dir.path()), None);
        assert_eq!(
            budget_from(Some("45"), dir.path()),
            Some(Duration::from_secs(45))
        );
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            dir.path().join(".aida/config.toml"),
            "[drain]\nred_run = true\n",
        )
        .unwrap();
        assert_eq!(
            budget_from(None, dir.path()),
            Some(Duration::from_secs(CONFIG_BUDGET_SECS))
        );
        assert_eq!(budget_from(Some("0"), dir.path()), None);
    }

    // trace:STORY-1386 | ai:claude
    #[cfg(unix)]
    #[test]
    fn a_timeout_records_timed_out_not_red() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = execute(
            "sleep",
            &["5".to_string()],
            dir.path(),
            Duration::from_millis(300),
        );
        assert_eq!(outcome, TestOutcome::TimedOut);
        let runs = vec![TestRun {
            name: "t".into(),
            path: "t.rs".into(),
            line: 1,
            outcome,
        }];
        assert_eq!(verdict_for(&runs), CriterionVerdict::NotRun);
    }

    // trace:STORY-1386 | ai:claude
    #[cfg(unix)]
    #[test]
    fn a_crash_records_not_run_not_red() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = execute(
            "sh",
            &["-c".to_string(), "kill -9 $$".to_string()],
            dir.path(),
            Duration::from_secs(10),
        );
        assert!(matches!(outcome, TestOutcome::NotRun(_)), "{outcome:?}");
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
        let (program, args) = plan_command(dir.path(), &test("crate-a/src/lib.rs")).unwrap();
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
            workspace: None,
            head_sha: None,
            criteria: vec![CriterionRedRun {
                id: "RED-1.AC1".into(),
                text: "first".into(),
                verdict: CriterionVerdict::AlreadySatisfied,
                timed_out: false,
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
