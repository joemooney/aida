//! `[landing] promote_live_binary_from = "no-new-failures"`: the comparison
//! that lets `aida dev build` promote a candidate commit C whose CI is red
//! only when every failure at C also fails at the promoted commit P.
//!
//! Failures are compared per test, from CI's own evidence. Each red test
//! shard (`CI (lib-0)` and the like) uploads a `test-timings-<shard>`
//! artifact: `test-run.log`, the log of every command
//! `scripts/ci-test-shards.py` ran, and one nextest JUnit report per nextest
//! invocation. The evidence counts only when complete: the log shows the
//! shard's expected invocations, every nextest invocation has its report and
//! a summary that agrees with it, every doctest run ends in its result line,
//! and, at C, the only failed step of the shard's job is its test run. A red
//! check that is not a CI lane (another workflow, a status) is compared by
//! name. A red CI lane without per-test evidence (`CI (gates)`,
//! `CI (contracts)`, `CI (fast gates)`) runs tests and gates whose individual
//! failures cannot be read, so it refuses at C.
//!
//! Every gap fails closed: CI that cannot be read or is still running, a
//! missing required check, P with no CI run, and missing, partial or
//! inconsistent shard evidence at either commit refuse the promotion and say
//! which evidence is missing. A test failing at P that is absent at C counts
//! as fixed only with a pass record at C.
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

/// The step of a test shard's job that runs its tests. Any other failed
/// step (a build, the time budget, root hygiene) is not a test failure.
pub(crate) const TEST_STEP: &str = "Run tests";

/// A `CI (<lane>)` check: one lane of the CI workflow.
fn ci_lane(name: &str) -> bool {
    name.starts_with("CI (") && name.ends_with(')')
}

/// The test shard a check name reports, if it is one.
pub(crate) fn shard_of_check(name: &str) -> Option<&'static str> {
    let inner = name.strip_prefix("CI (")?.strip_suffix(')')?;
    TEST_SHARDS.iter().copied().find(|s| *s == inner)
}

/// The artifact holding a shard's test log and JUnit reports.
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
            if attrs
                .insert(key.clone(), unescape(&v[1..1 + vend]))
                .is_some()
            {
                return Err(format!("duplicate attribute `{key}`"));
            }
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

// trace:BUG-2695 | ai:codex
fn is_empty_nextest_report(xml: &str) -> bool {
    // The tag reader skips text and XML directives. Require a single empty
    // root here so skipped content cannot turn a damaged report into evidence.
    static EMPTY_ROOT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(concat!(
            r#"\A[ \t\r\n]*(?:<\?xml version="1\.0" encoding="UTF-8"\?>[ \t\r\n]*)?"#,
            r#"<testsuites(?:[ \t\r\n]+[A-Za-z_][A-Za-z0-9_.-]*[ \t\r\n]*="#,
            r#"[ \t\r\n]*(?:"[^"<&]*"|'[^'<&]*'))+[ \t\r\n]*"#,
            r#"(?:/>|>[ \t\r\n]*</testsuites>)[ \t\r\n]*\z"#,
        ))
        .expect("valid empty nextest report pattern")
    });
    if !EMPTY_ROOT.is_match(xml) {
        return false;
    }
    let Ok(tags) = tags(xml) else {
        return false;
    };
    let root = &tags[0];
    root.attrs
        .get("name")
        .is_some_and(|name| name == "nextest-run")
        && ["tests", "skipped", "failures", "errors"]
            .iter()
            .all(|key| root.attrs.get(*key).is_some_and(|count| count == "0"))
}

// ---------------------------------------------------------------------------
// Shard evidence
// ---------------------------------------------------------------------------

/// What a test shard's `test-timings-<shard>` artifact holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ShardArtifact {
    /// `test-run.log` (`None` when the artifact has none).
    pub log: Option<String>,
    /// JUnit reports by file name (`junit-<shard>-<n>.xml`).
    pub reports: BTreeMap<String, String>,
}

/// One cargo invocation `scripts/ci-test-shards.py` ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `cargo nextest run`: one JUnit report.
    Nextest,
    /// `cargo test --doc`: doctests, in the log only.
    Doc,
}

struct Invocation {
    kind: Kind,
    command: String,
    /// The log lines it printed, up to the next invocation.
    lines: Vec<String>,
}

/// The invocations a shard runs, in order (`commands` and `for_runner` in
/// `scripts/ci-test-shards.py`). `rest` runs the workspace (nextest plus
/// its doctests), aida-cli's binaries, aida-cli-lib's doctests and, when
/// aida-cli-lib has integration targets, those.
fn expected_shapes(shard: &str) -> &'static [&'static [Kind]] {
    use Kind::{Doc, Nextest};
    match shard {
        "rest" => &[
            &[Nextest, Doc, Nextest, Doc],
            &[Nextest, Doc, Nextest, Doc, Nextest],
        ],
        _ => &[&[Nextest]],
    }
}

fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Split a shard's log into the cargo invocations it echoed (`+ cargo ...`).
/// `Err` on a runner error before the tests, or a command that is neither a
/// nextest run nor a doctest run (a runner fallback, an unknown step).
fn invocations(log: &str) -> std::result::Result<Vec<Invocation>, String> {
    let mut out: Vec<Invocation> = Vec::new();
    for raw in log.lines() {
        let line = strip_ansi(raw.trim_end_matches('\r'));
        // The runner's planning errors precede every test invocation.
        // external-prose-classifier: promotion_failures::invocations
        if out.is_empty() {
            if let Some(err) = line.strip_prefix("::error::") {
                return Err(format!("the test runner reported `{}`", err.trim()));
            }
        }
        if let Some(cmd) = line.strip_prefix("+ cargo ") {
            let words: Vec<&str> = cmd.split_whitespace().collect();
            if words.contains(&"--list") {
                continue; // The lib halves' test listing runs no test.
            }
            let kind = match words.as_slice() {
                ["nextest", "run", ..] => Kind::Nextest,
                ["test", ..] if words.contains(&"--doc") => Kind::Doc,
                _ => return Err(format!("the log runs an unexpected command `cargo {cmd}`")),
            };
            out.push(Invocation {
                kind,
                command: format!("cargo {}", words.join(" ")),
                lines: Vec::new(),
            });
        } else if let Some(inv) = out.last_mut() {
            inv.lines.push(line);
        }
    }
    Ok(out)
}

/// The failed-test count and whether a nextest invocation ran zero tests,
/// from its one summary line
/// (`Summary [ 1.2s] 120 tests run: 118 passed, 2 failed, 3 skipped`).
/// `Err` when there is no summary (a build failure) or the run was cut
/// short (`118/120 tests run`).
fn nextest_failed(lines: &[String]) -> std::result::Result<(usize, bool), String> {
    // nextest's own summary line; anything else is missing evidence.
    // external-prose-classifier: promotion_failures::nextest_failed
    let summaries: Vec<&str> = lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| l.starts_with("Summary ["))
        .collect();
    let [summary] = summaries.as_slice() else {
        return Err(if summaries.is_empty() {
            "printed no test summary (a build failure or a cancelled run)".into()
        } else {
            format!("printed {} test summaries", summaries.len())
        });
    };
    let parsed = summary
        .split_once("] ")
        .and_then(|(_, rest)| rest.split_once(" run: "));
    let Some((count, parts)) = parsed else {
        return Err(format!("printed an unreadable summary `{summary}`"));
    };
    if count.contains('/') {
        return Err(format!("stopped before every test ran (`{summary}`)"));
    }
    let mut failed = 0;
    for part in parts.split(", ") {
        let (n, kind) = part
            .trim()
            .split_once(' ')
            .ok_or_else(|| format!("printed an unreadable summary `{summary}`"))?;
        let n: usize = n
            .parse()
            .map_err(|_| format!("printed an unreadable summary `{summary}`"))?;
        if ["failed", "exec failed", "timed out"]
            .iter()
            .any(|k| kind.starts_with(k))
        {
            failed += n;
        }
    }
    Ok((failed, count == "0 tests" && parts == "0 passed, 0 skipped"))
}

/// `(passed, failed, ignored)` of a libtest `test result:` line's tail.
fn libtest_counts(tail: &str) -> Option<(usize, usize, usize)> {
    let (_, counts) = tail.split_once(". ")?;
    let mut found = [None; 3];
    for part in counts.split(';') {
        let part = part.trim();
        for (i, kind) in ["passed", "failed", "ignored"].iter().enumerate() {
            if let Some(n) = part.strip_suffix(kind) {
                found[i] = n.trim().parse().ok();
            }
        }
    }
    Some((found[0]?, found[1]?, found[2]?))
}

/// The per-doctest results of one `cargo test --doc` invocation's log.
/// Doctests are named `<crate>::doc <libtest name>`. `Err` unless every
/// `Doc-tests <crate>` block ends in a result line whose counts match its
/// per-test lines.
fn doctests(lines: &[String]) -> std::result::Result<TestResults, String> {
    // libtest's own per-test and result lines; anything else is missing evidence.
    // external-prose-classifier: promotion_failures::doctests
    struct Block {
        krate: String,
        passed: Vec<String>,
        failed: Vec<String>,
        ignored: usize,
        result: Option<(usize, usize, usize)>,
    }
    let mut blocks: Vec<Block> = Vec::new();
    for line in lines {
        let t = line.trim();
        if t.starts_with("error: could not compile") {
            return Err(format!("a build failed: `{t}`"));
        }
        if let Some(krate) = t.strip_prefix("Doc-tests ") {
            blocks.push(Block {
                krate: krate.trim().to_string(),
                passed: Vec::new(),
                failed: Vec::new(),
                ignored: 0,
                result: None,
            });
            continue;
        }
        if let Some(tail) = t.strip_prefix("test result: ") {
            let b = blocks
                .last_mut()
                .ok_or("printed a test result before any `Doc-tests` block")?;
            if b.result.is_some() {
                return Err(format!("printed two results for `{}`", b.krate));
            }
            b.result =
                Some(libtest_counts(tail).ok_or_else(|| format!("printed an unreadable `{t}`"))?);
            continue;
        }
        let Some((name, outcome)) = t.strip_prefix("test ").and_then(|r| r.rsplit_once(" ... "))
        else {
            continue;
        };
        let b = blocks
            .last_mut()
            .ok_or("printed a doctest outcome before any `Doc-tests` block")?;
        if b.result.is_some() {
            return Err(format!(
                "printed a doctest outcome after the result of `{}`",
                b.krate
            ));
        }
        let key = format!("{}::doc {name}", b.krate);
        match outcome {
            "ok" => b.passed.push(key),
            "FAILED" => b.failed.push(key),
            o if o.starts_with("ignored") => b.ignored += 1,
            other => return Err(format!("printed an unknown outcome `{other}` for `{name}`")),
        }
    }
    if blocks.is_empty() {
        return Err("ran no doctests (a build failure)".into());
    }
    let mut out = TestResults::default();
    for b in blocks {
        let Some(result) = b.result else {
            return Err(format!("never finished the doctests of `{}`", b.krate));
        };
        if result != (b.passed.len(), b.failed.len(), b.ignored) {
            return Err(format!(
                "reported {} passed, {} failed, {} ignored for `{}` but listed {}, {}, {}",
                result.0,
                result.1,
                result.2,
                b.krate,
                b.passed.len(),
                b.failed.len(),
                b.ignored
            ));
        }
        out.merge(TestResults {
            failed: b.failed.into_iter().collect(),
            passed: b.passed.into_iter().collect(),
        });
    }
    Ok(out)
}

/// The per-test results of shard `shard`, from its complete artifact. `Err`
/// names the missing or inconsistent evidence.
pub(crate) fn shard_evidence(
    shard: &str,
    artifact: &ShardArtifact,
) -> std::result::Result<TestResults, String> {
    let log = artifact
        .log
        .as_deref()
        .ok_or("its test log `test-run.log` is missing, so its tests did not all run")?;
    let invs = invocations(log)?;
    let kinds: Vec<Kind> = invs.iter().map(|i| i.kind).collect();
    if !expected_shapes(shard)
        .iter()
        .any(|s| *s == kinds.as_slice())
    {
        let shown: Vec<&str> = invs.iter().map(|i| i.command.as_str()).collect();
        return Err(format!(
            "its log shows {} test invocation(s), not the shard's full set{}",
            invs.len(),
            if shown.is_empty() {
                String::new()
            } else {
                format!(": {}", shown.join("; "))
            }
        ));
    }
    let nextest: Vec<&Invocation> = invs.iter().filter(|i| i.kind == Kind::Nextest).collect();
    let want: BTreeSet<String> = (0..nextest.len())
        .map(|i| format!("junit-{shard}-{i}.xml"))
        .collect();
    let have: BTreeSet<String> = artifact.reports.keys().cloned().collect();
    if have != want {
        let names = |s: &BTreeSet<String>| {
            if s.is_empty() {
                "none".to_string()
            } else {
                s.iter().cloned().collect::<Vec<_>>().join(", ")
            }
        };
        return Err(format!(
            "its {} nextest invocation(s) need JUnit reports {}, but it has {}",
            nextest.len(),
            names(&want),
            names(&have)
        ));
    }
    let mut all = TestResults::default();
    for (i, inv) in nextest.iter().enumerate() {
        let name = format!("junit-{shard}-{i}.xml");
        let xml = &artifact.reports[&name];
        let (failed, zero_tests) =
            nextest_failed(&inv.lines).map_err(|e| format!("`{}` {e}", inv.command))?;
        // Only the rest shard's binary selection is expected to have no suites.
        let parsed = if shard == "rest"
            && i == 1
            && inv.command
                == "cargo nextest run --no-fail-fast --profile ci --no-tests warn -p aida-cli --bins"
            && zero_tests
            && is_empty_nextest_report(xml)
        {
            TestResults::default()
        } else {
            parse_junit(xml)
                .map_err(|e| format!("its JUnit report `{name}` is unreadable: {e}"))?
        };
        if failed != parsed.failed.len() {
            return Err(format!(
                "`{}` reports {failed} failed test(s), but its JUnit report `{name}` names {}",
                inv.command,
                parsed.failed.len()
            ));
        }
        all.merge(parsed);
    }
    for inv in invs.iter().filter(|i| i.kind == Kind::Doc) {
        all.merge(doctests(&inv.lines).map_err(|e| format!("`{}` {e}", inv.command))?);
    }
    Ok(all)
}

// ---------------------------------------------------------------------------
// Evidence
// ---------------------------------------------------------------------------

/// Where the comparison reads CI. [`RealEvidence`] uses the forge; tests
/// substitute a fake.
pub(crate) trait Evidence {
    /// Every check the forge reports on `sha` (`Err` when unreadable).
    fn checks(&mut self, sha: &str) -> Result<CommitChecks>;
    /// The checks every commit must pass (the CI-gated set).
    fn required(&mut self) -> Vec<String>;
    /// The `test-timings-<shard>` artifact of workflow run `run_id`.
    fn shard_artifact(&mut self, run_id: &str, shard: &str) -> Result<ShardArtifact>;
    /// The steps of job `job` in workflow run `run_id` that did not succeed.
    fn failed_steps(&mut self, run_id: &str, job: &str) -> Result<Vec<String>>;
}

/// What one commit's recorded CI says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RedCi {
    /// Red checks that are not CI lanes (an aggregate only when alone).
    checks: BTreeSet<String>,
    /// Red CI lanes that are not test shards: no per-test evidence.
    lanes: BTreeSet<String>,
    /// Red test shards and the workflow run that reported each.
    shards: BTreeMap<String, String>,
    /// Green test shards and their run.
    green_shards: BTreeMap<String, String>,
    /// Every check that passed.
    passed: BTreeSet<String>,
}

enum Read {
    Green,
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
                         evidence cannot be found",
                        row.name,
                        short(sha)
                    ))
                }
            },
            (CheckOutcome::Failed, None) if ci_lane(&row.name) => {
                red.lanes.insert(row.name.clone());
            }
            (CheckOutcome::Failed, None) => {
                red.checks.insert(row.name.clone());
            }
            (CheckOutcome::Passed, shard) => {
                red.passed.insert(row.name.clone());
                if let (Some(s), Some(run)) = (shard, &row.run_id) {
                    red.green_shards.insert(s.to_string(), run.clone());
                }
            }
            _ => {}
        }
    }
    // The aggregate is explained by any other red check.
    if red.checks.len() + red.lanes.len() + red.shards.len() > 1 {
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
        CommitCi::Green => Read::Green,
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

/// Read and merge the complete test evidence of `shards`. At the candidate
/// (`candidate`), each red shard's job must also have failed only in its
/// test run, and its evidence must name a failing test: anything else is a
/// failure the evidence cannot explain. `Err` names the gap.
fn shard_results(
    ev: &mut dyn Evidence,
    sha: &str,
    shards: &BTreeMap<String, String>,
    candidate: bool,
) -> std::result::Result<TestResults, String> {
    let mut all = TestResults::default();
    for (shard, run) in shards {
        let at = format!("shard `{shard}` on {} (run {run})", short(sha));
        let artifact = ev
            .shard_artifact(run, shard)
            .map_err(|e| format!("the test evidence of {at} could not be read: {e:#}"))?;
        let results = shard_evidence(shard, &artifact)
            .map_err(|e| format!("the test evidence of {at} is incomplete: {e}"))?;
        if candidate {
            let job = format!("CI ({shard})");
            let steps = ev
                .failed_steps(run, &job)
                .map_err(|e| format!("the steps of {at} could not be read: {e:#}"))?;
            let other: Vec<String> = steps
                .iter()
                .filter(|s| *s != TEST_STEP)
                .map(|s| format!("`{s}`"))
                .collect();
            if !other.is_empty() {
                return Err(format!(
                    "{at} failed in {}, which is not its test run, so that failure cannot be \
                     compared per test",
                    other.join(", ")
                ));
            }
            if !steps.iter().any(|s| s == TEST_STEP) || results.failed.is_empty() {
                return Err(format!(
                    "{at} is red, but its test run and test evidence do not both name a \
                     failing test, so the failure cannot be compared per test"
                ));
            }
        }
        all.merge(results);
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
    /// Where the promoted commit's failures came from: `ci` or `not needed`.
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

    /// The one-line summary: verdict, both commits, set sizes, differences.
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

    /// The summary, then both failing sets in full, as `aida dev status`
    /// shows them.
    pub(crate) fn status_lines(&self) -> Vec<String> {
        let set = |items: &[String]| {
            if items.is_empty() {
                "none".to_string()
            } else {
                items.join(", ")
            }
        };
        vec![
            self.summary(),
            format!(
                "  failing at the candidate {}: {}",
                short(&self.candidate),
                set(&self.candidate_failing)
            ),
            format!(
                "  failing at the promoted {}: {}",
                self.promoted.as_deref().map(short).unwrap_or("(none)"),
                set(&self.promoted_failing)
            ),
        ]
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
    let mut d = Decision::new(candidate, promoted);
    if candidate.is_empty() {
        return d.refuse("the commit being promoted could not be read, so its CI is unknown");
    }
    let c_red = match read_ci(ev, candidate) {
        Read::Green => {
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
    if !c_red.lanes.is_empty() {
        let lanes: Vec<String> = c_red.lanes.iter().cloned().collect();
        d.candidate_failing = c_red
            .lanes
            .iter()
            .chain(&c_red.checks)
            .map(|c| check_key(c))
            .collect();
        return d.refuse(format!(
            "CI lane(s) {} failed on {}, and they upload no per-test evidence, so which of \
             their tests or gates failed cannot be compared with the promoted commit",
            list(&lanes),
            short(candidate)
        ));
    }
    let c_tests = match shard_results(ev, candidate, &c_red.shards, true) {
        Ok(r) => r,
        Err(why) => return d.refuse(why),
    };
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

    // The promoted commit's failures, from its own CI evidence only.
    let p_red = match read_ci(ev, promoted) {
        Read::Unusable(why) => return d.refuse(why),
        Read::NoRun => {
            return d.refuse(format!(
                "{} has no recorded CI run, so its failures are unknown",
                short(promoted)
            ))
        }
        Read::Green => RedCi::default(),
        Read::Red(red) => red,
    };
    let p_tests = match shard_results(ev, promoted, &p_red.shards, false) {
        Ok(r) => r,
        Err(why) => return d.refuse(why),
    };
    let p_checks: BTreeSet<String> = p_red.checks.union(&p_red.lanes).cloned().collect();
    d.promoted_source = "ci".into();
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
    // record at C. Read C's evidence for P's red shards, best effort.
    let mut c_passed = c_tests.passed.clone();
    if !p_tests.failed.is_subset(&c_tests.failed) {
        for (shard, run) in c_red
            .green_shards
            .iter()
            .filter(|(s, _)| p_red.shards.contains_key(*s))
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
        if c_red.passed.contains(c) {
            d.fixed.push(check_key(c));
        } else {
            d.unconfirmed.push(check_key(c));
        }
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

/// The forge, for `aida dev build`.
pub(crate) struct RealEvidence<'a> {
    pub repo: &'a Path,
    /// Parent of each download's private scratch directory.
    pub scratch: &'a Path,
}

impl Evidence for RealEvidence<'_> {
    fn checks(&mut self, sha: &str) -> Result<CommitChecks> {
        crate::forge::forge_for(self.repo).commit_checks(sha)
    }

    fn required(&mut self) -> Vec<String> {
        crate::landing_policy::load(self.repo).ci_gated_checks
    }

    fn shard_artifact(&mut self, run_id: &str, shard: &str) -> Result<ShardArtifact> {
        std::fs::create_dir_all(self.scratch)?;
        // Private to this download: concurrent builds never share, clear or
        // read each other's artifacts. Removed when dropped.
        let dest = tempfile::Builder::new()
            .prefix("artifact-")
            .tempdir_in(self.scratch)?;
        crate::forge::forge_for(self.repo).download_run_artifact(
            run_id,
            &report_artifact(shard),
            dest.path(),
        )?;
        let mut artifact = ShardArtifact::default();
        let mut dirs = vec![dest.path().to_path_buf()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir)? {
                let path = entry?.path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let read = || {
                    std::fs::read_to_string(&path)
                        .with_context(|| format!("reading {}", path.display()))
                };
                if name == "test-run.log" {
                    anyhow::ensure!(artifact.log.is_none(), "the artifact has two test logs");
                    artifact.log = Some(read()?);
                } else if name.starts_with("junit") && name.ends_with(".xml") {
                    anyhow::ensure!(
                        artifact.reports.insert(name.to_string(), read()?).is_none(),
                        "the artifact has two JUnit reports named `{name}`"
                    );
                }
            }
        }
        Ok(artifact)
    }

    fn failed_steps(&mut self, run_id: &str, job: &str) -> Result<Vec<String>> {
        crate::forge::forge_for(self.repo).run_job_failed_steps(run_id, job)
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

/// Record `decision` as the last one made for `live_dir`: an atomic replace
/// through a temporary private to this write, so concurrent builds never
/// clobber each other's half-written record.
pub(crate) fn record_last(live_dir: &Path, decision: &Decision) -> Result<()> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::fs::create_dir_all(live_dir)?;
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = live_dir.join(format!("{DECISION_FILE}.{}-{seq}.tmp", std::process::id()));
    let result = std::fs::write(&tmp, serde_json::to_vec_pretty(decision)?)
        .map_err(anyhow::Error::from)
        .and_then(|()| {
            std::fs::rename(&tmp, live_dir.join(DECISION_FILE))
                .with_context(|| format!("writing {}", live_dir.join(DECISION_FILE).display()))
        });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// The last decision recorded for `live_dir`, if readable.
pub(crate) fn read_last(live_dir: &Path) -> Option<Decision> {
    serde_json::from_slice(&std::fs::read(live_dir.join(DECISION_FILE)).ok()?).ok()
}

#[cfg(test)]
#[path = "tests/promotion_failures_tests.rs"]
mod tests;
