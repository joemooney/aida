//! MAIN-FLAKE-RECURRENCE: a test that failed on the base branch and went
//! green on a rerun, and then failed again, there or on any open change.
//!
//! Every failed attempt is kept as a failure with its own identity and time:
//! `run <id>#<attempt>` for a base-branch run, `PR-<n>@<head>` for a change.
//! An attempt whose run later went green is a flake as well, and stays a
//! failure, so two flakes of one test read in the same pass are a flake and
//! its recurrence.
// trace:TASK-3827 | ai:claude

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use super::{mins_since, run_failed, Budget, CiReads, Class, Item, Memory, Policy, ADVISOR_SEAT};

/// Failed base-run attempts one pass reads; the rest wait for the next pass.
const MAX_ATTEMPTS_PER_PASS: usize = 8;
/// Earlier attempts of one run that are read.
const MAX_ATTEMPTS_PER_RUN: u64 = 4;

/// One test failing in one attempt.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct TestFailure {
    pub test: String,
    /// When the failed job finished.
    pub at: DateTime<Utc>,
    /// `run <id>#<attempt>` or `PR-<n>@<head>`.
    pub source: String,
    #[serde(default)]
    pub url: String,
    /// A base-branch attempt whose run went green on a later attempt.
    #[serde(default)]
    pub flake: bool,
}

impl TestFailure {
    fn on_base(&self) -> bool {
        self.source.starts_with("run ")
    }
}

/// One workflow run on the base branch, at its latest attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BaseRun {
    pub id: u64,
    pub attempt: u64,
    pub completed: bool,
    pub conclusion: String,
    pub updated_at: Option<DateTime<Utc>>,
    pub url: String,
}

/// One job of one attempt of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttemptJob {
    pub id: u64,
    pub conclusion: String,
    pub completed_at: Option<DateTime<Utc>>,
}

fn time(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.as_str()?.trim())
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// `gh run list --json databaseId,attempt,conclusion,status,updatedAt,url`.
pub(crate) fn parse_base_runs(json: &str) -> std::result::Result<Vec<BaseRun>, String> {
    let runs: Vec<serde_json::Value> =
        serde_json::from_str(json).map_err(|e| format!("unreadable run list: {e}"))?;
    Ok(runs
        .iter()
        .filter_map(|run| {
            Some(BaseRun {
                id: run["databaseId"].as_u64()?,
                attempt: run["attempt"].as_u64()?,
                completed: run["status"].as_str() == Some("completed"),
                conclusion: run["conclusion"].as_str().unwrap_or_default().to_string(),
                updated_at: time(&run["updatedAt"]),
                url: run["url"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect())
}

/// The forge's jobs of one run attempt.
pub(crate) fn parse_attempt_jobs(json: &str) -> std::result::Result<Vec<AttemptJob>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("unreadable jobs: {e}"))?;
    let jobs = value["jobs"]
        .as_array()
        .ok_or_else(|| "the jobs answer has no `jobs` array".to_string())?;
    Ok(jobs
        .iter()
        .filter_map(|job| {
            Some(AttemptJob {
                id: job["id"].as_u64()?,
                conclusion: job["conclusion"].as_str().unwrap_or_default().to_string(),
                completed_at: time(&job["completed_at"]),
            })
        })
        .collect())
}

/// Keep `failure`, once per test and attempt. A flake stays one.
fn record(memory: &mut Memory, failure: TestFailure) {
    match memory
        .failures
        .iter_mut()
        .find(|known| known.test == failure.test && known.source == failure.source)
    {
        Some(known) => known.flake |= failure.flake,
        None => memory.failures.push(failure),
    }
}

/// Forget what is too old to be either half of a recurrence.
pub(crate) fn prune(memory: &mut Memory, policy: &Policy, now: DateTime<Utc>) {
    let keep = Duration::hours(policy.flake_window_hours) * 2;
    memory.failures.retain(|failure| now - failure.at <= keep);
    memory.failures.sort();
    memory.seen_runs.retain(|_, at| now - *at <= keep);
}

/// What one pass could not read, said once.
struct Unread {
    count: usize,
    why: Option<String>,
}

impl Unread {
    fn note(&mut self, why: String) {
        self.count += 1;
        self.why.get_or_insert(why);
    }

    fn report(self, what: &str, unknown: &mut Vec<String>) {
        if let Some(why) = self.why {
            unknown.push(format!(
                "{what}: {} not read on this run ({why}); they are read on a later run",
                self.count
            ));
        }
    }
}

/// Read the base branch's runs inside the window. Every failed attempt gives
/// failures, read once; an attempt of a run that is green now is a flake,
/// also when it was first read while the run was still red.
pub(crate) fn read_base_failures(
    ci: &dyn CiReads,
    budget: &Budget,
    branch: &str,
    memory: &mut Memory,
    policy: &Policy,
    now: DateTime<Utc>,
    unknown: &mut Vec<String>,
) {
    let listed = budget
        .call_timeout()
        .and_then(|timeout| ci.base_runs(branch, timeout));
    let mut runs = match listed {
        Ok(runs) => runs,
        Err(why) => {
            unknown.push(format!("flaky tests on `{branch}`: unknown ({why})"));
            return;
        }
    };
    runs.sort_by_key(|run| run.updated_at);
    let window = Duration::hours(policy.flake_window_hours);
    let mut unread = Unread {
        count: 0,
        why: None,
    };
    let mut read = 0;
    for run in &runs {
        let Some(updated) = run.updated_at else {
            continue;
        };
        if now - updated > window {
            continue;
        }
        let green = run.completed && run.conclusion == "success";
        let red = run.completed && run_failed(&run.conclusion);
        // The attempts that failed: each one a rerun followed, and the
        // latest when the run ended red.
        let last = if red {
            run.attempt
        } else {
            run.attempt.saturating_sub(1)
        };
        let first = last.saturating_sub(MAX_ATTEMPTS_PER_RUN - 1).max(1);
        for attempt in first..=last {
            let source = format!("run {}#{attempt}", run.id);
            if green {
                for known in memory.failures.iter_mut().filter(|f| f.source == source) {
                    known.flake = true;
                }
            }
            if memory.seen_runs.contains_key(&source) {
                continue;
            }
            if read >= MAX_ATTEMPTS_PER_PASS {
                unread.note(format!(
                    "only {MAX_ATTEMPTS_PER_PASS} failed attempts are read per run of the guard"
                ));
                continue;
            }
            read += 1;
            let jobs = budget
                .call_timeout()
                .and_then(|timeout| ci.attempt_jobs(run.id, attempt, timeout));
            let jobs = match jobs {
                Ok(jobs) => jobs,
                Err(why) => {
                    unread.note(why);
                    continue;
                }
            };
            let mut complete = true;
            for job in jobs.iter().filter(|job| run_failed(&job.conclusion)) {
                let tests = budget
                    .call_timeout()
                    .and_then(|timeout| ci.job_tests(job.id, timeout));
                match tests {
                    Ok(tests) => {
                        for test in tests {
                            record(
                                memory,
                                TestFailure {
                                    test,
                                    at: job.completed_at.unwrap_or(updated),
                                    source: source.clone(),
                                    url: run.url.clone(),
                                    flake: green,
                                },
                            );
                        }
                    }
                    Err(why) => {
                        complete = false;
                        unread.note(why);
                    }
                }
            }
            // An unread log is asked for again on the next pass.
            if complete {
                memory.seen_runs.insert(source, updated);
            }
        }
    }
    unread.report(
        &format!("flaky tests on `{branch}`: failed run attempts"),
        unknown,
    );
}

/// Read the failing tests of every open change whose head is red (`heads`:
/// `PR-<n>@<head>` key and head commit), approved or not: a flake recurs on
/// any change. Each head is read once. Nothing is read while the base branch
/// has no failure inside the window, since then nothing can recur.
pub(crate) fn read_pr_failures(
    ci: &dyn CiReads,
    budget: &Budget,
    heads: &[(String, String)],
    memory: &mut Memory,
    policy: &Policy,
    now: DateTime<Utc>,
    unknown: &mut Vec<String>,
) {
    memory
        .pr_failures_read
        .retain(|key, _| heads.iter().any(|(head_key, _)| head_key == key));
    let window = Duration::hours(policy.flake_window_hours);
    let base_failed = memory
        .failures
        .iter()
        .any(|failure| failure.on_base() && now - failure.at <= window);
    if !base_failed {
        return;
    }
    let mut unread = Unread {
        count: 0,
        why: None,
    };
    let mut read = 0;
    for (key, head) in heads {
        if memory.pr_failures_read.contains_key(key) {
            continue;
        }
        if read >= policy.max_red_logs {
            unread.note(format!(
                "only {} red changes are read per run",
                policy.max_red_logs
            ));
            continue;
        }
        read += 1;
        let listed = budget
            .call_timeout()
            .and_then(|timeout| ci.check_runs(head, timeout));
        let runs = match listed {
            Ok(runs) => runs,
            Err(why) => {
                unread.note(why);
                continue;
            }
        };
        // A red head that lists no failed run is being rerun: it is asked
        // again, not marked read.
        let mut complete = runs.iter().any(|run| run_failed(&run.conclusion));
        for run in runs.iter().filter(|run| run_failed(&run.conclusion)) {
            // A log the approved-red reader already has is not read twice.
            let kept = memory
                .red_detail
                .get(key)
                .and_then(|detail| detail.checks.iter().find(|check| check.id == run.id))
                .and_then(|check| check.tests.clone().ok());
            let tests = match kept {
                Some(tests) => Ok(tests),
                None => budget
                    .call_timeout()
                    .and_then(|timeout| ci.job_tests(run.id, timeout)),
            };
            match tests {
                Ok(tests) => {
                    let at = DateTime::parse_from_rfc3339(run.completed_at.trim())
                        .map_or(now, |at| at.with_timezone(&Utc));
                    for test in tests {
                        record(
                            memory,
                            TestFailure {
                                test,
                                at,
                                source: key.clone(),
                                url: run.html_url.clone(),
                                flake: false,
                            },
                        );
                    }
                }
                Err(why) => {
                    complete = false;
                    unread.note(why);
                }
            }
        }
        if complete {
            memory.pr_failures_read.insert(key.clone(), now);
        }
    }
    unread.report("flaky tests failing again: red changes", unknown);
}

/// Flaky tests that failed again: a failure of a test after a flake of the
/// same test in another attempt, both inside the window. One item per test,
/// for its newest such failure, linking the earliest flake before it.
pub(crate) fn flake_recurrences(
    failures: &[TestFailure],
    policy: &Policy,
    now: DateTime<Utc>,
) -> Vec<Item> {
    let window = Duration::hours(policy.flake_window_hours);
    let mut by_test: std::collections::BTreeMap<&str, (&TestFailure, &TestFailure)> =
        std::collections::BTreeMap::new();
    for failure in failures {
        if failure.at > now || now - failure.at > window {
            continue;
        }
        let earlier = failures
            .iter()
            .filter(|flake| {
                flake.flake
                    && flake.test == failure.test
                    && flake.source != failure.source
                    && flake.at < failure.at
                    && failure.at - flake.at <= window
            })
            .min_by_key(|flake| flake.at);
        let Some(earlier) = earlier else { continue };
        let entry = by_test
            .entry(failure.test.as_str())
            .or_insert((earlier, failure));
        if failure.at > entry.1.at {
            *entry = (earlier, failure);
        }
    }
    let link = |event: &TestFailure| {
        if event.url.is_empty() {
            event.source.clone()
        } else {
            format!("{} {}", event.source, event.url)
        }
    };
    by_test
        .into_iter()
        .map(|(test, (earlier, again))| Item {
            key: format!(
                "{}:{test}@{}",
                Class::MainFlakeRecurrence.label(),
                again.source
            ),
            class: Class::MainFlakeRecurrence,
            pr: None,
            seat: ADVISOR_SEAT.into(),
            age_mins: mins_since(Some(again.at), now),
            summary: format!(
                "`{test}` failed on the base branch and passed on a rerun at {} ({}), then failed again at {} ({})",
                earlier.at.format("%Y-%m-%d %H:%M UTC"),
                link(earlier),
                again.at.format("%Y-%m-%d %H:%M UTC"),
                link(again)
            ),
            action: format!(
                "route a fix-forward for `{test}`; another rerun only hides it"
            ),
        })
        .collect()
}
