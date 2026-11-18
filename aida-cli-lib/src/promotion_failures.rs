//! `[landing] promote_live_binary_from = "no-new-failures"`: the comparison
//! that lets `aida dev build` promote a candidate commit C whose CI is red
//! only when every failure at C also fails at the promoted commit P.
//!
//! Failures are compared at two levels. Per test: the failing test names come
//! from each red test shard's nextest JUnit report (the `test-timings-<shard>`
//! artifact CI uploads). Per check: a red check that is not a test shard (a
//! gate, a lint) must also be red at P. When P has no recorded CI run, C's
//! failing tests are re-run at P in a scratch checkout.
//!
//! Every gap fails closed: CI that cannot be read, is still running, misses a
//! required check, or a red shard whose report is missing, unreadable or names
//! no failing test refuses the promotion with the reason. A test failing at P
//! that is absent at C counts as fixed only with a pass record at C.
// trace:STORY-2912 | ai:claude
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::forge::{CheckOutcome, CommitCheck, CommitChecks, CommitCi};

/// AIDA's nextest test shards (`.github/workflows/ci.yml`, check `CI (<shard>)`).
pub(crate) const TEST_SHARDS: &[&str] = &["lib-0", "lib-1", "cli-0", "cli-1", "rest"];

/// The check that fails whenever any CI lane fails (`build-result`). Its
/// failure beside another red check says nothing of its own, so it is
/// compared only when it is the sole red check.
pub(crate) const AGGREGATE_CHECKS: &[&str] = &["Build (ubuntu-latest)"];

/// AIDA's CI lanes that run no nextest shard: compared by check name.
pub(crate) const NON_TEST_LANES: &[&str] = &["gates", "contracts"];

/// A `CI (<lane>)` check naming neither a test shard nor a known non-test
/// lane: its failures could be tests, so it is never compared by name.
fn unknown_lane(name: &str) -> bool {
    name.strip_prefix("CI (")
        .and_then(|n| n.strip_suffix(')'))
        .is_some_and(|lane| !TEST_SHARDS.contains(&lane) && !NON_TEST_LANES.contains(&lane))
}

/// The test shard a check name reports, if it is one.
pub(crate) fn shard_of_check(name: &str) -> Option<&'static str> {
    let inner = name.strip_prefix("CI (")?.strip_suffix(')')?;
    TEST_SHARDS.iter().copied().find(|s| *s == inner)
}

/// The artifact holding a shard's JUnit reports.
pub(crate) fn report_artifact(shard: &str) -> String {
    format!("test-timings-{shard}")
}

// ---------------------------------------------------------------------------
// JUnit
// ---------------------------------------------------------------------------

/// Per-test results of one or more nextest JUnit reports. A test is named
/// `<binary id> <test name>`, as `scripts/test-timings.py` names it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TestResults {
    pub failed: BTreeSet<String>,
    pub passed: BTreeSet<String>,
}

impl TestResults {
    fn merge(&mut self, other: TestResults) {
        self.failed.extend(other.failed);
        self.passed.extend(other.passed);
        // A test failing in any report is failing.
        let failed = &self.failed;
        self.passed.retain(|t| !failed.contains(t));
    }
}

fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(end) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let entity = &tail[1..end];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// One start or end tag.
struct Tag {
    name: String,
    attrs: BTreeMap<String, String>,
    end: bool,
    self_closing: bool,
}

/// The tags of an XML document; text, comments, CDATA and processing
/// instructions are skipped. `Err` on a truncated document.
fn tags(xml: &str) -> std::result::Result<Vec<Tag>, String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        rest = &rest[i..];
        // The end of a construct opened by `open`, searched after the opener.
        let skip = |rest: &str, open: &str, close: &str| -> Option<usize> {
            rest[open.len()..]
                .find(close)
                .map(|e| open.len() + e + close.len())
        };
        for (open, close) in [
            ("<![CDATA[", "]]>"),
            ("<!--", "-->"),
            ("<?", "?>"),
            ("<!", ">"),
        ] {
            if rest.starts_with(open) {
                let Some(n) = skip(rest, open, close) else {
                    return Err(format!("unterminated `{open}`"));
                };
                rest = &rest[n..];
                break;
            }
        }
        if !rest.starts_with('<') || rest.starts_with("<!") || rest.starts_with("<?") {
            continue;
        }
        // A tag: scan to its `>`, honouring quoted attribute values.
        let mut quote = None;
        let mut close = None;
        for (j, c) in rest.char_indices().skip(1) {
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, '"' | '\'') => quote = Some(c),
                (None, '>') => {
                    close = Some(j);
                    break;
                }
                _ => {}
            }
        }
        let Some(close) = close else {
            return Err("unterminated tag".into());
        };
        let body = &rest[1..close];
        rest = &rest[close + 1..];
        let end = body.starts_with('/');
        let self_closing = body.ends_with('/');
        let body = body.trim_start_matches('/').trim_end_matches('/');
        let name_end = body.find(|c: char| c.is_whitespace()).unwrap_or(body.len());
        let name = body[..name_end].to_string();
        if name.is_empty() {
            return Err("empty tag name".into());
        }
        let mut attrs = BTreeMap::new();
        let mut a = body[name_end..].trim_start();
        while let Some(eq) = a.find('=') {
            let key = a[..eq].trim().to_string();
            let v = a[eq + 1..].trim_start();
            let Some(q) = v.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                return Err(format!("unquoted attribute `{key}`"));
            };
            let Some(vend) = v[1..].find(q) else {
                return Err(format!("unterminated attribute `{key}`"));
            };
            attrs.insert(key, unescape(&v[1..1 + vend]));
            a = v[1 + vend + 1..].trim_start();
        }
        out.push(Tag {
            name,
            attrs,
            end,
            self_closing,
        });
    }
    Ok(out)
}

/// Parse one nextest JUnit report. A `<testcase>` with a direct `<failure>`
/// or `<error>` child (a failed, timed-out or crashed test) failed; any other
/// test case passed. `Err` when the report is not a complete JUnit document
/// with at least one test suite.
pub(crate) fn parse_junit(xml: &str) -> std::result::Result<TestResults, String> {
    let tags = tags(xml)?;
    let mut results = TestResults::default();
    let mut stack: Vec<&str> = Vec::new();
    let mut suite: Option<String> = None;
    let mut case: Option<(String, bool)> = None;
    let mut suites = 0usize;
    fn finish(case: (String, bool), results: &mut TestResults) {
        if case.1 {
            results.failed.insert(case.0);
        } else {
            results.passed.insert(case.0);
        }
    }
    for tag in &tags {
        if tag.end {
            match stack.pop() {
                Some(open) if open == tag.name => {}
                other => {
                    return Err(format!(
                        "`</{}>` closes `{}`",
                        tag.name,
                        other.unwrap_or("nothing")
                    ))
                }
            }
            match tag.name.as_str() {
                "testcase" => {
                    if let Some(c) = case.take() {
                        finish(c, &mut results);
                    }
                }
                "testsuite" => suite = None,
                _ => {}
            }
            continue;
        }
        match tag.name.as_str() {
            "testsuite" => {
                suites += 1;
                suite = Some(
                    tag.attrs
                        .get("name")
                        .cloned()
                        .ok_or("a <testsuite> has no name")?,
                );
            }
            "testcase" => {
                let binary = suite
                    .as_deref()
                    .ok_or("a <testcase> outside a <testsuite>")?;
                let name = tag.attrs.get("name").ok_or("a <testcase> has no name")?;
                let entry = (format!("{binary} {name}"), false);
                if tag.self_closing {
                    finish(entry, &mut results);
                } else {
                    case = Some(entry);
                }
            }
            "failure" | "error" if stack.last() == Some(&"testcase") => {
                if let Some(c) = case.as_mut() {
                    c.1 = true;
                }
            }
            _ => {}
        }
        if !tag.self_closing {
            stack.push(tag.name.as_str());
        }
    }
    if !stack.is_empty() {
        return Err(format!(
            "truncated report: `<{}>` is never closed",
            stack.join("> <")
        ));
    }
    if suites == 0 {
        return Err("no <testsuite> in the report".into());
    }
    let failed = results.failed.clone();
    results.passed.retain(|t| !failed.contains(t));
    Ok(results)
}

// ---------------------------------------------------------------------------
// Evidence
// ---------------------------------------------------------------------------

/// Where the comparison reads CI and re-runs tests. [`RealEvidence`] uses
/// the forge and cargo; tests substitute a fake.
pub(crate) trait Evidence {
    /// Every check the forge reports on `sha` (`Err` when unreadable).
    fn checks(&mut self, sha: &str) -> Result<CommitChecks>;
    /// The checks every commit must pass (the CI-gated set).
    fn required(&mut self) -> Vec<String>;
    /// The JUnit reports `shard` uploaded in workflow run `run_id`.
    fn shard_reports(&mut self, run_id: &str, shard: &str) -> Result<Vec<String>>;
    /// Re-run `tests` at `sha`: `true` = passed, `false` = failed. A test
    /// whose outcome could not be determined is absent.
    fn rerun(&mut self, sha: &str, tests: &[String]) -> Result<BTreeMap<String, bool>>;
}

/// What one commit's recorded CI says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RedCi {
    /// Red checks that are not test shards (an aggregate only when alone).
    checks: BTreeSet<String>,
    /// Red test shards and the workflow run that reported each.
    shards: BTreeMap<String, String>,
    /// Green test shards and their run.
    green_shards: BTreeMap<String, String>,
}

enum Read {
    Green(RedCi),
    Red(RedCi),
    /// Nothing ran on the commit.
    NoRun,
    /// The CI cannot be used; why.
    Unusable(String),
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

fn read_ci(ev: &mut dyn Evidence, sha: &str) -> Read {
    let checks = match ev.checks(sha) {
        Ok(c) => c,
        Err(e) => return Read::Unusable(format!("CI on {} could not be read: {e:#}", short(sha))),
    };
    let required = ev.required();
    let required: Vec<&str> = required
        .iter()
        .map(String::as_str)
        .filter(|n| *n != crate::landing_policy::MERGE_HOLD_GATE)
        .collect();
    let verdict = crate::forge::judge_commit_checks(&checks, &required);
    let rows: Vec<&CommitCheck> = crate::forge::latest_per_name(&checks.checks);
    let mut red = RedCi::default();
    for row in &rows {
        let shard = shard_of_check(&row.name);
        match (row.outcome, shard) {
            (CheckOutcome::Failed, Some(s)) => match &row.run_id {
                Some(run) => {
                    red.shards.insert(s.to_string(), run.clone());
                }
                None => {
                    return Read::Unusable(format!(
                        "test shard check `{}` on {} names no workflow run, so its test \
                         report cannot be found",
                        row.name,
                        short(sha)
                    ))
                }
            },
            (CheckOutcome::Failed, None) if unknown_lane(&row.name) => {
                return Read::Unusable(format!(
                    "CI lane `{}` failed on {}, and it is not a known test shard, so its failures \
                     cannot be compared per test",
                    row.name,
                    short(sha)
                ))
            }
            (CheckOutcome::Failed, None) => {
                red.checks.insert(row.name.clone());
            }
            (CheckOutcome::Passed, Some(s)) => {
                if let Some(run) = &row.run_id {
                    red.green_shards.insert(s.to_string(), run.clone());
                }
            }
            _ => {}
        }
    }
    // The aggregate is explained by any other red check.
    if red.checks.len() + red.shards.len() > 1 {
        red.checks
            .retain(|c| !AGGREGATE_CHECKS.contains(&c.as_str()));
    }
    // A red verdict outranks a running or missing check, but a comparison
    // needs them all finished: a running shard may yet fail a new test.
    if matches!(verdict, CommitCi::Failed(_)) {
        let running: Vec<&str> = rows
            .iter()
            .filter(|r| r.outcome == CheckOutcome::Pending)
            .map(|r| r.name.as_str())
            .collect();
        if !running.is_empty() {
            return Read::Unusable(format!(
                "CI on {} is still running: {}",
                short(sha),
                running.join(", ")
            ));
        }
        let reported = |name: &str| {
            rows.iter().any(|r| {
                r.name == name && matches!(r.outcome, CheckOutcome::Passed | CheckOutcome::Failed)
            })
        };
        if let Some(missing) = required.iter().find(|n| !reported(n)) {
            return Read::Unusable(format!(
                "required check `{missing}` did not run on {}, so its CI is incomplete",
                short(sha)
            ));
        }
    }
    match verdict {
        CommitCi::Green => Read::Green(red),
        CommitCi::NoChecks => Read::NoRun,
        CommitCi::Pending(names) => Read::Unusable(format!(
            "CI on {} is still running: {}",
            short(sha),
            names.join(", ")
        )),
        CommitCi::MissingRequired(name) => Read::Unusable(format!(
            "required check `{name}` did not report success on {}, so its CI is incomplete",
            short(sha)
        )),
        CommitCi::Failed(_) => Read::Red(red),
    }
}

/// Read and merge the JUnit reports of `shards` (`Err` names the gap).
fn shard_results(
    ev: &mut dyn Evidence,
    sha: &str,
    shards: &BTreeMap<String, String>,
    require_failure: bool,
) -> std::result::Result<TestResults, String> {
    let mut all = TestResults::default();
    for (shard, run) in shards {
        let reports = ev.shard_reports(run, shard).map_err(|e| {
            format!(
                "the test report of shard `{shard}` on {} (run {run}) could not be read: {e:#}",
                short(sha)
            )
        })?;
        if reports.is_empty() {
            return Err(format!(
                "shard `{shard}` on {} (run {run}) uploaded no JUnit report",
                short(sha)
            ));
        }
        let mut shard_all = TestResults::default();
        for xml in &reports {
            let parsed = parse_junit(xml).map_err(|e| {
                format!(
                    "a JUnit report of shard `{shard}` on {} (run {run}) is unreadable: {e}",
                    short(sha)
                )
            })?;
            shard_all.merge(parsed);
        }
        if require_failure && shard_all.failed.is_empty() {
            return Err(format!(
                "shard `{shard}` failed on {} but its report names no failing test (a build, \
                 doctest or step failure), so the failure cannot be compared per test",
                short(sha)
            ));
        }
        all.merge(shard_all);
    }
    Ok(all)
}

// ---------------------------------------------------------------------------
// Decision
// ---------------------------------------------------------------------------

/// The no-new-failures verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Verdict {
    Allowed,
    Refused,
}

/// One no-new-failures decision: what was compared and why it went the way
/// it did. Recorded in the promotion record and shown by `aida dev status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Decision {
    pub candidate: String,
    /// The promoted commit compared against (`None` when none is recorded).
    #[serde(default)]
    pub promoted: Option<String>,
    /// Failing tests and non-test checks (`check: <name>`) at the candidate.
    #[serde(default)]
    pub candidate_failing: Vec<String>,
    /// Failing tests and checks at the promoted commit, as far as compared.
    #[serde(default)]
    pub promoted_failing: Vec<String>,
    /// Where the promoted commit's failures came from: `ci`, `local re-run`
    /// or `not needed`.
    #[serde(default)]
    pub promoted_source: String,
    /// Failing at the candidate, not at the promoted commit.
    #[serde(default)]
    pub new_failures: Vec<String>,
    /// Failing at the promoted commit, passing at the candidate.
    #[serde(default)]
    pub fixed: Vec<String>,
    /// Failing at the promoted commit, with no pass record at the candidate.
    #[serde(default)]
    pub unconfirmed: Vec<String>,
    pub verdict: Verdict,
    /// Why, in one sentence.
    pub reason: String,
    pub decided_at: String,
}

impl Decision {
    fn new(candidate: &str, promoted: Option<&str>) -> Self {
        Self {
            candidate: candidate.to_string(),
            promoted: promoted.map(str::to_string),
            candidate_failing: Vec::new(),
            promoted_failing: Vec::new(),
            promoted_source: "not needed".into(),
            new_failures: Vec::new(),
            fixed: Vec::new(),
            unconfirmed: Vec::new(),
            verdict: Verdict::Refused,
            reason: String::new(),
            decided_at: chrono::Utc::now().to_rfc3339(),
        }
    }
    fn refuse(mut self, reason: impl Into<String>) -> Self {
        self.verdict = Verdict::Refused;
        self.reason = reason.into();
        self
    }
    fn allow(mut self, reason: impl Into<String>) -> Self {
        self.verdict = Verdict::Allowed;
        self.reason = reason.into();
        self
    }
    pub(crate) fn allowed(&self) -> bool {
        self.verdict == Verdict::Allowed
    }

    /// The refusal shown to the operator (`None` when allowed).
    pub(crate) fn refusal(&self) -> Option<String> {
        (!self.allowed()).then(|| {
            format!(
                "the live binary is promoted only when every failure at the candidate also \
                 fails at the promoted commit (`[landing] promote_live_binary_from = \
                 \"no-new-failures\"`), and {}; the live binary stays where it is.",
                self.reason
            )
        })
    }

    /// The one-line summary `aida dev status` prints.
    pub(crate) fn summary(&self) -> String {
        let verdict = match self.verdict {
            Verdict::Allowed => "allowed",
            Verdict::Refused => "refused",
        };
        let mut line = format!(
            "no-new-failures {verdict}: {} vs promoted {} ({} failing at the candidate, {} at \
             the promoted commit, from {})",
            short(&self.candidate),
            self.promoted.as_deref().map(short).unwrap_or("(none)"),
            self.candidate_failing.len(),
            self.promoted_failing.len(),
            self.promoted_source,
        );
        if !self.new_failures.is_empty() {
            line.push_str(&format!("; new: {}", self.new_failures.join(", ")));
        }
        if !self.fixed.is_empty() {
            line.push_str(&format!("; fixed: {}", self.fixed.join(", ")));
        }
        if !self.unconfirmed.is_empty() {
            line.push_str(&format!(
                "; not run at the candidate: {}",
                self.unconfirmed.join(", ")
            ));
        }
        line
    }
}

fn check_key(name: &str) -> String {
    format!("check: {name}")
}

fn list(items: &[String]) -> String {
    const SHOWN: usize = 10;
    let mut s = items
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > SHOWN {
        s.push_str(&format!(", and {} more", items.len() - SHOWN));
    }
    s
}

/// Decide whether `candidate` may be promoted over `promoted` under
/// `no-new-failures`.
pub(crate) fn decide(ev: &mut dyn Evidence, candidate: &str, promoted: Option<&str>) -> Decision {
    let candidate = candidate.trim();
    let promoted = promoted.map(str::trim).filter(|p| !p.is_empty());
    let d = Decision::new(candidate, promoted);
    if candidate.is_empty() {
        return d.refuse("the commit being promoted could not be read, so its CI is unknown");
    }
    let c_red = match read_ci(ev, candidate) {
        Read::Green(_) => {
            return d.allow(format!("CI on {} is green", short(candidate)));
        }
        Read::Red(red) => red,
        Read::NoRun => {
            return d.refuse(format!(
                "there is no CI run on {}, so its failures are unknown",
                short(candidate)
            ))
        }
        Read::Unusable(why) => return d.refuse(why),
    };
    let c_tests = match shard_results(ev, candidate, &c_red.shards, true) {
        Ok(r) => r,
        Err(why) => return d.refuse(why),
    };
    let mut d = d;
    d.candidate_failing = c_tests
        .failed
        .iter()
        .cloned()
        .chain(c_red.checks.iter().map(|c| check_key(c)))
        .collect();
    let Some(promoted) = promoted else {
        let why = format!(
            "CI on {} has failures ({}) and no promoted commit is recorded to compare them with",
            short(candidate),
            list(&d.candidate_failing)
        );
        return d.refuse(why);
    };
    if promoted.eq_ignore_ascii_case(candidate) {
        d.promoted_failing = d.candidate_failing.clone();
        return d.allow("the candidate is the promoted commit");
    }

    // The promoted commit's failures: from its CI, or re-run here.
    let mut p_red_shards = BTreeSet::new();
    let (p_tests, p_checks, source) = match read_ci(ev, promoted) {
        Read::Unusable(why) => return d.refuse(why),
        Read::Green(_) => (TestResults::default(), BTreeSet::new(), "ci"),
        Read::Red(p_red) => match shard_results(ev, promoted, &p_red.shards, false) {
            Ok(r) => {
                p_red_shards.extend(p_red.shards.into_keys());
                (r, p_red.checks, "ci")
            }
            Err(why) => return d.refuse(why),
        },
        Read::NoRun => {
            if !c_red.checks.is_empty() {
                let names: Vec<String> = c_red.checks.iter().cloned().collect();
                d.promoted_source = "local re-run".into();
                return d.refuse(format!(
                    "{} has no recorded CI run, and checks failing at {} cannot be re-run \
                     locally: {}",
                    short(promoted),
                    short(candidate),
                    list(&names)
                ));
            }
            let tests: Vec<String> = c_tests.failed.iter().cloned().collect();
            let outcomes = match ev.rerun(promoted, &tests) {
                Ok(o) => o,
                Err(e) => {
                    d.promoted_source = "local re-run".into();
                    return d.refuse(format!(
                        "{} has no recorded CI run, and re-running the failing tests there \
                         failed: {e:#}",
                        short(promoted)
                    ));
                }
            };
            let undetermined: Vec<String> = tests
                .iter()
                .filter(|t| !outcomes.contains_key(*t))
                .cloned()
                .collect();
            if !undetermined.is_empty() {
                d.promoted_source = "local re-run".into();
                return d.refuse(format!(
                    "{} has no recorded CI run, and re-running there did not determine the \
                     outcome of: {}",
                    short(promoted),
                    list(&undetermined)
                ));
            }
            let mut r = TestResults::default();
            for (t, passed) in outcomes {
                if passed {
                    r.passed.insert(t);
                } else {
                    r.failed.insert(t);
                }
            }
            (r, BTreeSet::new(), "local re-run")
        }
    };
    d.promoted_source = source.into();
    d.promoted_failing = p_tests
        .failed
        .iter()
        .cloned()
        .chain(p_checks.iter().map(|c| check_key(c)))
        .collect();
    d.new_failures = c_tests
        .failed
        .difference(&p_tests.failed)
        .cloned()
        .chain(c_red.checks.difference(&p_checks).map(|c| check_key(c)))
        .collect();

    // Audit only: a test failing at P counts as fixed only with a pass
    // record at C. Read C's reports for P's red shards, best effort.
    let mut c_passed = c_tests.passed.clone();
    if !p_tests.failed.is_subset(&c_tests.failed) {
        for (shard, run) in c_red
            .green_shards
            .iter()
            .filter(|(s, _)| p_red_shards.contains(*s))
        {
            let one = BTreeMap::from([(shard.clone(), run.clone())]);
            if let Ok(more) = shard_results(ev, candidate, &one, false) {
                c_passed.extend(more.passed);
            }
        }
    }
    for t in p_tests.failed.difference(&c_tests.failed) {
        if c_passed.contains(t) {
            d.fixed.push(t.clone());
        } else {
            d.unconfirmed.push(t.clone());
        }
    }
    for c in p_checks.difference(&c_red.checks) {
        d.unconfirmed.push(check_key(c));
    }

    if d.new_failures.is_empty() {
        let n = d.candidate_failing.len();
        d.allow(format!(
            "all {n} failure(s) at {} also fail at the promoted {}",
            short(candidate),
            short(promoted)
        ))
    } else {
        let msg = format!(
            "{} failure(s) at {} do not fail at the promoted {}: {}",
            d.new_failures.len(),
            short(candidate),
            short(promoted),
            list(&d.new_failures)
        );
        d.refuse(msg)
    }
}

// ---------------------------------------------------------------------------
// Real evidence
// ---------------------------------------------------------------------------

/// The cargo arguments selecting a nextest binary id's test target
/// (`crate`, `crate::test`, `crate::bin/name`).
pub(crate) fn cargo_target_args(binary_id: &str) -> Option<Vec<String>> {
    let valid = |s: &str| {
        !s.is_empty()
            && !s.starts_with('-')
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    let (krate, target) = match binary_id.split_once("::") {
        Some((k, t)) => (k, Some(t)),
        None => (binary_id, None),
    };
    if !valid(krate) {
        return None;
    }
    let mut args = vec!["-p".to_string(), krate.to_string()];
    match target {
        None => args.push("--lib".into()),
        Some(t) => {
            let (flag, name) = match t.split_once('/') {
                Some(("bin", n)) => ("--bin", n),
                Some(("test", n)) => ("--test", n),
                Some(_) => return None,
                None => ("--test", t),
            };
            if !valid(name) {
                return None;
            }
            args.push(flag.into());
            args.push(name.into());
        }
    }
    Some(args)
}

/// The outcome libtest printed for `name` (`Some(true)` = ok).
pub(crate) fn libtest_outcome(output: &str, name: &str) -> Option<bool> {
    // libtest's own per-test line; anything else is undetermined, which refuses.
    // external-prose-classifier: promotion_failures::libtest_outcome
    let prefix = format!("test {name} ... ");
    output.lines().find_map(|l| {
        let rest = l.trim_end().strip_prefix(&prefix)?;
        match rest {
            "ok" => Some(true),
            "FAILED" => Some(false),
            _ if rest.starts_with("FAILED") => Some(false),
            _ => None,
        }
    })
}

/// How long one local re-run of a failing test may take (a cold build of
/// the promoted commit included).
const RERUN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3600);

/// The forge and cargo, for `aida dev build`.
pub(crate) struct RealEvidence<'a> {
    pub repo: &'a Path,
    /// Scratch space for artifacts, the re-run checkout and its target dir.
    pub scratch: &'a Path,
}

impl Evidence for RealEvidence<'_> {
    fn checks(&mut self, sha: &str) -> Result<CommitChecks> {
        crate::forge::forge_for(self.repo).commit_checks(sha)
    }

    fn required(&mut self) -> Vec<String> {
        crate::landing_policy::load(self.repo).ci_gated_checks
    }

    fn shard_reports(&mut self, run_id: &str, shard: &str) -> Result<Vec<String>> {
        let dest = self.scratch.join("artifacts").join(run_id).join(shard);
        if dest.exists() {
            std::fs::remove_dir_all(&dest)?;
        }
        std::fs::create_dir_all(&dest)?;
        crate::forge::forge_for(self.repo).download_run_artifact(
            run_id,
            &report_artifact(shard),
            &dest,
        )?;
        let mut reports = Vec::new();
        let mut dirs = vec![dest.clone()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir)? {
                let path = entry?.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("junit") && n.ends_with(".xml"))
                {
                    reports.push(
                        std::fs::read_to_string(&path)
                            .with_context(|| format!("reading {}", path.display()))?,
                    );
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dest);
        Ok(reports)
    }

    fn rerun(&mut self, sha: &str, tests: &[String]) -> Result<BTreeMap<String, bool>> {
        anyhow::ensure!(
            crate::forge::is_full_commit_sha(sha),
            "invalid commit `{sha}` to re-run tests at"
        );
        let checkout = self.scratch.join(format!("rerun-{}", short(sha)));
        let target = self.scratch.join("rerun-target");
        let git = |args: &[&str]| -> Result<()> {
            let out = aida_core::git_ops::host_git_command(self.repo)
                .args(args)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .stdin(std::process::Stdio::null())
                .output()?;
            anyhow::ensure!(
                out.status.success(),
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
            Ok(())
        };
        if checkout.exists() {
            let _ = git(&["worktree", "remove", "--force", &checkout.to_string_lossy()]);
            let _ = std::fs::remove_dir_all(&checkout);
        }
        std::fs::create_dir_all(self.scratch)?;
        git(&[
            "worktree",
            "add",
            "--detach",
            &checkout.to_string_lossy(),
            sha,
        ])?;
        let result = (|| {
            let mut outcomes = BTreeMap::new();
            for test in tests {
                // A name libtest would read as an option is left
                // undetermined, which refuses.
                let Some((binary, name)) = test
                    .split_once(' ')
                    .filter(|(_, n)| !n.is_empty() && !n.starts_with('-'))
                else {
                    continue;
                };
                let Some(args) = cargo_target_args(binary) else {
                    continue;
                };
                let mut cmd = std::process::Command::new("cargo");
                cmd.current_dir(&checkout)
                    .env("RUSTC_WRAPPER", "")
                    .args(["test", "--profile", crate::dev_build::PROFILE])
                    .arg("--target-dir")
                    .arg(&target)
                    .args(&args)
                    .args(["--", "--exact", name]);
                let (_, out, err) = crate::dev_build::run_with_timeout(&mut cmd, RERUN_TIMEOUT)
                    .with_context(|| format!("re-running `{test}`"))?;
                if let Some(passed) = libtest_outcome(&format!("{out}\n{err}"), name) {
                    outcomes.insert(test.clone(), passed);
                }
            }
            Ok(outcomes)
        })();
        let _ = git(&["worktree", "remove", "--force", &checkout.to_string_lossy()]);
        result
    }
}

/// `decide` against the forge, for the live promotion of `repo`.
pub(crate) fn decide_live(
    repo: &Path,
    scratch: &Path,
    candidate: Option<&str>,
    promoted: Option<&str>,
) -> Decision {
    let mut ev = RealEvidence { repo, scratch };
    decide(&mut ev, candidate.unwrap_or_default(), promoted)
}

/// Name of the last no-new-failures decision kept in the live dir, refused
/// or allowed, for `aida dev status`.
pub(crate) const DECISION_FILE: &str = ".promotion-decision.json";

/// Record `decision` as the last one made for `live_dir` (atomic replace).
pub(crate) fn record_last(live_dir: &Path, decision: &Decision) -> Result<()> {
    std::fs::create_dir_all(live_dir)?;
    let tmp = live_dir.join(format!("{DECISION_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(decision)?)?;
    std::fs::rename(&tmp, live_dir.join(DECISION_FILE))
        .with_context(|| format!("writing {}", live_dir.join(DECISION_FILE).display()))?;
    Ok(())
}

/// The last decision recorded for `live_dir`, if readable.
pub(crate) fn read_last(live_dir: &Path) -> Option<Decision> {
    serde_json::from_slice(&std::fs::read(live_dir.join(DECISION_FILE)).ok()?).ok()
}

#[cfg(test)]
#[path = "tests/promotion_failures_tests.rs"]
mod tests;
