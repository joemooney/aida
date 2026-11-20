//! The throughput guard: which open changes are stuck, and who can move them.
//!
//! The seats' watchers are event-driven, and an absence of events wakes no
//! one: a push nobody reviews, an approval nobody lands and a conflict nobody
//! owns all look like silence. This module reads the same evidence the
//! landing reconciler derives its table from ([`super::live_pass_with`]),
//! fact by fact, plus CI on the red approved heads, and names every stall.
//!
//! The reconciler's state is one exclusive label in a fixed precedence (a red
//! head is `CI-FAILED` before its review history is read). A stall is judged
//! from the facts themselves: the approval, the review history, the hold, CI
//! and who is working the change, each read on its own.
//!
//! `aida land stalls` prints the classification. The scheduled
//! `throughput-guard` job (`aida doctor check throughput-guard`) computes the
//! same report and, on the scheduler's tick only, routes each new stall once:
//! one consolidated `throughput-guard-route-<seat>` due job per seat and a
//! short mail to that seat. No model runs and nothing is launched, merged or
//! released here: the guard classifies and routes.
//!
//! The scheduled check does nothing unless `[throughput_guard] enabled =
//! true` is set in `.aida/config.toml`.
// trace:STORY-2906 | ai:claude

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use super::{Action, Approval, Ci, Hold, HoldRelease, State};
use crate::events::{self, Event, EventKind};
use crate::schedule_ledger;
use crate::DoctorFinding;

pub(crate) const CATEGORY: &str = "throughput-guard";
/// Prefix of the due jobs the guard raises, one per routed seat.
pub(crate) const ROUTE_JOB_PREFIX: &str = "throughput-guard-route-";
const OPERATOR_SEAT: &str = crate::mail_backlog::OPERATOR_SEAT;
const ADVISOR_SEAT: &str = "advisor";
const SENDER: &str = "aida-throughput-guard";
const NOTIFY_RULE: &str = "throughput-guard-escalation";
/// Bound on one forge call.
const GH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
/// Time one run may spend reading CI detail. Every forge call is given what
/// is left of it, so a scheduled run finishes well inside its kill ceiling
/// with whatever it has read.
pub(crate) const DETAIL_BUDGET: std::time::Duration = std::time::Duration::from_secs(75);
/// Longest evidence text one due job carries.
const MAX_EVIDENCE_CHARS: usize = 4000;
/// Longest test name read from a CI log.
const MAX_TEST_NAME_CHARS: usize = 200;
/// Failing tests one evidence line names before it counts the rest.
const MAX_LISTED_TESTS: usize = 8;
/// Scheduled runs whose observation is kept (a day at the default cadence).
const MAX_RUNS: usize = 72;

/// `[throughput_guard]` in `.aida/config.toml`. Every threshold is a project
/// setting; the scheduled check is off unless `enabled = true`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Policy {
    pub enabled: bool,
    pub merge_mins: i64,
    pub review_mins: i64,
    pub rework_mins: i64,
    pub conflict_mins: i64,
    pub landings_window_mins: i64,
    pub escalate_after_mins: i64,
    /// Red changes whose job logs one run reads.
    pub max_red_logs: i64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            enabled: false,
            merge_mins: 20,
            review_mins: 30,
            rework_mins: 30,
            conflict_mins: 30,
            landings_window_mins: 60,
            escalate_after_mins: 120,
            max_red_logs: 6,
        }
    }
}

impl Policy {
    /// Absent, non-integer or non-positive values keep their defaults.
    pub(crate) fn from_config(cfg: Option<&toml::Value>) -> Self {
        let mut policy = Self::default();
        let Some(table) = cfg
            .and_then(|v| v.get("throughput_guard"))
            .and_then(|v| v.as_table())
        else {
            return policy;
        };
        policy.enabled = table.get("enabled").and_then(|v| v.as_bool()) == Some(true);
        for (key, slot) in [
            ("merge_mins", &mut policy.merge_mins),
            ("review_mins", &mut policy.review_mins),
            ("rework_mins", &mut policy.rework_mins),
            ("conflict_mins", &mut policy.conflict_mins),
            ("landings_window_mins", &mut policy.landings_window_mins),
            ("escalate_after_mins", &mut policy.escalate_after_mins),
            ("max_red_logs", &mut policy.max_red_logs),
        ] {
            if let Some(n) = table
                .get(key)
                .and_then(|v| v.as_integer())
                .filter(|n| *n > 0)
            {
                *slot = n;
            }
        }
        policy
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) enum Class {
    #[serde(rename = "STUCK-MERGE")]
    StuckMerge,
    #[serde(rename = "STUCK-MERGE-RED")]
    StuckMergeRed,
    #[serde(rename = "STUCK-REVIEW")]
    StuckReview,
    #[serde(rename = "STUCK-REWORK")]
    StuckRework,
    #[serde(rename = "STUCK-CONFLICT")]
    StuckConflict,
    #[serde(rename = "moving")]
    Moving,
}

impl Class {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Class::StuckMerge => "STUCK-MERGE",
            Class::StuckMergeRed => "STUCK-MERGE-RED",
            Class::StuckReview => "STUCK-REVIEW",
            Class::StuckRework => "STUCK-REWORK",
            Class::StuckConflict => "STUCK-CONFLICT",
            Class::Moving => "moving",
        }
    }
}

/// One stall: what is stuck, the seat that can move it, and what to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Item {
    /// Identity for routing: the same change with the same blocker (and, for
    /// red CI, the same remedy) is routed once.
    pub key: String,
    pub class: Class,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr: Option<u64>,
    pub seat: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_mins: Option<i64>,
    pub summary: String,
    pub action: String,
}

/// Why a check is red on an approved head.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RedCause {
    /// No failing test is in a file the change touches, and the check passes
    /// on the base.
    ProbableFlake,
    /// The change touches the code a failing test covers.
    PrCaused,
    /// The same check is red on the base too.
    BaseRed,
    Undetermined,
}

/// What the base's CI says about one check that failed on a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BaseCheck {
    /// It passed at the base's tip.
    Pass,
    /// It is red at the base's tip.
    Red,
    Unknown(String),
}

impl Default for BaseCheck {
    fn default() -> Self {
        BaseCheck::Unknown("not read".into())
    }
}

/// One failed check run on a head and what its log said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FailedCheck {
    pub name: String,
    /// The forge's id of the check run; a rerun gives a new one.
    pub id: u64,
    /// The failing tests its log names; `Err` says why the log was not read.
    pub tests: std::result::Result<Vec<String>, String>,
    /// Judged against the base's tip on every run, never kept.
    #[serde(skip)]
    pub base: BaseCheck,
}

/// The facts behind one red, approved head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RedFact {
    pub checks: Vec<FailedCheck>,
    /// `Err` when the change's files could not be listed.
    pub changed_paths: std::result::Result<Vec<String>, String>,
    /// The failed check runs a rerun was already routed for at this head.
    pub rerun_routed: Option<Vec<u64>>,
    /// Why the head's check runs were not listed at all, when they were not.
    pub unread: Option<String>,
}

/// Where one open change stands, from each fact read on its own. Unlike the
/// reconciler's state, red CI does not hide the review history, and a hold
/// does not hide the approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Standing {
    /// A train, a promotion or the integration branch: the landing itself.
    Landing,
    /// A fact could not be read; nothing is concluded from the rest.
    Unknown(String),
    Conflict {
        /// A lane on the change.
        owner: Option<String>,
    },
    /// A review of the head is running.
    Reviewing(String),
    Approved {
        approval: String,
        /// The failed checks on the head, named.
        red: Option<String>,
        hold: Option<Hold>,
        lane: Option<String>,
        /// Based on another change's branch: it lands after that one.
        stacked: bool,
    },
    /// A blocking verdict stands at the head's content.
    ChangesRequested {
        lane: Option<String>,
        round: usize,
        red: bool,
    },
    /// No verdict covers the head and no review is running.
    Unreviewed {
        refusal: String,
        red: Option<String>,
        lane: Option<String>,
    },
}

/// One open change: where it stands, since when, and the reconciler's row
/// for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrFact {
    pub number: u64,
    pub title: String,
    pub head: String,
    /// The reconciler's row. Shown; a stall is never decided from it.
    pub state: State,
    pub action: Action,
    pub reason: String,
    pub standing: Standing,
    /// The event the wait counts from: the head's commit for an unreviewed
    /// head, the verdict for an approval or requested changes, the base
    /// commit that first conflicted for a conflict. `None` when unreadable.
    pub since: Option<DateTime<Utc>>,
    pub red: Option<RedFact>,
}

impl PrFact {
    /// An approved red head nobody is working: its CI detail is read.
    pub(crate) fn wants_red_detail(&self) -> bool {
        matches!(
            &self.standing,
            Standing::Approved {
                red: Some(_),
                lane: None,
                ..
            }
        )
    }
}

fn parse_time(text: Option<String>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text?.trim())
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// Where `pr` stands. Facts are read independently of the reconciler's
/// precedence; an unreadable one makes the standing unknown.
pub(crate) fn standing(
    pr: &super::Pr,
    row: &super::Row,
    evidence: &dyn super::Evidence,
) -> Standing {
    if row.state == State::Landing {
        return Standing::Landing;
    }
    let unknown = |what: &str, why: String| Standing::Unknown(format!("{what} unknown ({why})"));
    match evidence.conflict(pr) {
        Err(why) => return unknown("mergeability", why),
        Ok(true) => {
            return match evidence.lane(pr) {
                Ok(owner) => Standing::Conflict { owner },
                Err(why) => unknown("lane liveness", why),
            }
        }
        Ok(false) => {}
    }
    match evidence.reviewing(pr) {
        Err(why) => return unknown("review ownership", why),
        Ok(Some(review)) => return Standing::Reviewing(review),
        Ok(None) => {}
    }
    let red = match evidence.ci(pr) {
        Err(why) => return unknown("CI", why),
        Ok(Ci::Red(names)) => Some(names),
        Ok(_) => None,
    };
    let lane = match evidence.lane(pr) {
        Ok(lane) => lane,
        Err(why) => return unknown("lane liveness", why),
    };
    match evidence.approval(pr) {
        Err(why) => unknown("approval", why),
        Ok(Approval::Approved(approval)) => match evidence.hold(pr) {
            Err(why) => unknown("merge hold", why),
            Ok(hold) => Standing::Approved {
                approval,
                red,
                hold,
                lane,
                stacked: pr.base != super::MAIN_BRANCH && pr.base != super::INTEGRATION_BRANCH,
            },
        },
        Ok(Approval::Refused(refusal)) => match evidence.rounds(pr) {
            Err(why) => unknown("review history", why),
            Ok(rounds) if rounds.at_content => Standing::ChangesRequested {
                lane,
                round: rounds.content_rounds,
                red: red.is_some(),
            },
            Ok(_) => Standing::Unreviewed { refusal, red, lane },
        },
    }
}

/// The fact for one change of a pass, with the event its wait counts from.
pub(crate) fn pr_fact(pr: &super::Pr, row: &super::Row, evidence: &dyn super::Evidence) -> PrFact {
    let standing = standing(pr, row, evidence);
    let head_at = || parse_time(evidence.head_at(pr));
    let since = match &standing {
        // The base commit that first conflicted, or the head when it is
        // newer. When that commit cannot be found the head's own time is
        // used.
        // An owned conflict is not searched: it is moving whatever its age.
        Standing::Conflict { owner: Some(_) } => head_at(),
        Standing::Conflict { owner: None } => {
            match (parse_time(evidence.conflict_since(pr)), head_at()) {
                (Some(onset), Some(head)) => Some(onset.max(head)),
                (onset, head) => onset.or(head),
            }
        }
        // An approval carried over a newer head counts from that head.
        Standing::Approved { .. } => parse_time(evidence.approved_at(pr))
            .map(|approved| head_at().map_or(approved, |head| head.max(approved))),
        Standing::ChangesRequested { .. } => evidence
            .rounds(pr)
            .ok()
            .and_then(|rounds| parse_time(rounds.since)),
        Standing::Unreviewed { .. } => head_at(),
        Standing::Landing | Standing::Unknown(_) | Standing::Reviewing(_) => None,
    };
    PrFact {
        number: pr.id,
        title: pr.title.clone(),
        head: pr.head.clone(),
        state: row.state.clone(),
        action: row.action.clone(),
        reason: row.reason.clone(),
        standing,
        since,
        red: None,
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

fn mins_since(since: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Option<i64> {
    since.map(|at| (now - at).num_minutes().max(0))
}

/// The test a runner's failure line names: libtest's `test a::b ... FAILED`,
/// nextest's `FAIL [ 0.5s] crate a::b`.
///
/// The name comes from a log the change's own CI wrote and ends up in a due
/// job and a mail an agent reads, so only a plain test path is accepted:
/// anything else is not a name.
pub(crate) fn test_name(line: &str) -> Option<String> {
    let line = line.trim();
    let nextest = ["FAIL [", "TIMEOUT [", "SIGSEGV [", "SIGABRT [", "SIGKILL ["];
    let name = if let Some(rest) = line.strip_prefix("test ") {
        rest.strip_suffix("... FAILED")?.trim()
    } else if nextest.iter().any(|p| line.starts_with(p)) {
        line.split_once(']')?.1.split_whitespace().last()?
    } else {
        return None;
    };
    let plain = !name.is_empty()
        && name.len() <= MAX_TEST_NAME_CHARS
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_:-.<>,/".contains(&b));
    plain.then(|| name.to_string())
}

/// The tests a line of evidence names: the first few, then a count.
fn listed(tests: &[String]) -> String {
    let shown = tests
        .iter()
        .take(MAX_LISTED_TESTS)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    match tests.len().saturating_sub(MAX_LISTED_TESTS) {
        0 => shown,
        more => format!("{shown} and {more} more"),
    }
}

/// The failing tests among a job log's failure lines, sorted, each once.
pub(crate) fn test_names(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|line| test_name(line))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn normalized(word: &str) -> String {
    word.trim().to_ascii_lowercase().replace('-', "_")
}

/// The changed path that a failing test plausibly covers, if any. A
/// heuristic: a test's module path is compared with the directory and file
/// names of the change (`queue::tests::drains` matches `src/queue.rs` and
/// `src/queue/mod.rs`); a test with no module path is compared by name.
pub(crate) fn touching_path<'a>(test: &str, changed: &'a [String]) -> Option<&'a str> {
    let parts: Vec<String> = test.split("::").map(normalized).collect();
    let (name, modules) = parts.split_last()?;
    let modules: Vec<&String> = modules
        .iter()
        .filter(|m| !matches!(m.as_str(), "tests" | "test" | "super" | "crate" | ""))
        .collect();
    changed
        .iter()
        .find(|path| {
            let path = Path::new(path.as_str());
            let mut names: Vec<String> = path
                .parent()
                .into_iter()
                .flat_map(|dir| dir.components())
                .filter_map(|c| c.as_os_str().to_str())
                .map(normalized)
                .collect();
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push(normalized(stem));
            }
            if modules.is_empty() {
                // A long enough file name that the test's own name starts with.
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(normalized)
                    .is_some_and(|stem| stem.len() >= 4 && name.starts_with(&stem))
            } else {
                modules.iter().any(|m| {
                    names
                        .iter()
                        .any(|n| n == *m && !matches!(n.as_str(), "src"))
                })
            }
        })
        .map(String::as_str)
}

/// Why one failed check is red, with the reason in words. A heuristic, so
/// the reason always says what was compared. A check whose log was not read
/// is never called a flake.
pub(crate) fn check_cause(
    check: &FailedCheck,
    changed: &std::result::Result<Vec<String>, String>,
) -> (RedCause, String) {
    let name = &check.name;
    if check.base == BaseCheck::Red {
        return (
            RedCause::BaseRed,
            format!("`{name}` is red on the base branch too"),
        );
    }
    let tests = match &check.tests {
        Ok(tests) => tests,
        Err(why) => {
            return (
                RedCause::Undetermined,
                format!("the log of `{name}` was not read ({why})"),
            )
        }
    };
    if tests.is_empty() {
        return (
            RedCause::Undetermined,
            format!("the log of `{name}` names no failing test"),
        );
    }
    let changed = match changed {
        Ok(changed) => changed,
        Err(why) => {
            return (
                RedCause::Undetermined,
                format!("the change's files could not be listed ({why})"),
            )
        }
    };
    if let Some((test, path)) = tests
        .iter()
        .find_map(|test| touching_path(test, changed).map(|path| (test, path)))
    {
        return (
            RedCause::PrCaused,
            format!("the change touches `{path}`, which failing test `{test}` of `{name}` covers (matched by module path)"),
        );
    }
    match &check.base {
        BaseCheck::Unknown(why) => (
            RedCause::Undetermined,
            format!("no failing test of `{name}` is in a file this change touches, but whether `{name}` passes on the base branch is unknown ({why})"),
        ),
        BaseCheck::Pass | BaseCheck::Red => (
            RedCause::ProbableFlake,
            format!(
                "none of the {} failing test(s) of `{name}` is in a file this change touches (compared by module path with its {} changed file(s)), and `{name}` passes on the base branch",
                tests.len(),
                changed.len()
            ),
        ),
    }
}

/// What a red head's failed checks add up to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RedVerdict {
    /// The cause that decides the action: the change's own failure first,
    /// then anything undetermined, then a red base, and a flake only when
    /// every failed check is one.
    pub cause: RedCause,
    /// One reason per failed check.
    pub reasons: Vec<String>,
    /// The failed checks that are red on the base too.
    pub base_red: Vec<String>,
}

/// Every failed check is judged on its own: a check that is red on the base
/// says nothing about another check that is not.
pub(crate) fn red_cause(red: &RedFact) -> RedVerdict {
    let undetermined = |reason: String| RedVerdict {
        cause: RedCause::Undetermined,
        reasons: vec![reason],
        base_red: Vec::new(),
    };
    if let Some(why) = &red.unread {
        return undetermined(format!("the failed jobs were not examined ({why})"));
    }
    if red.checks.is_empty() {
        return undetermined("no failed check run is listed at the head".into());
    }
    let causes: Vec<(RedCause, String)> = red
        .checks
        .iter()
        .map(|check| check_cause(check, &red.changed_paths))
        .collect();
    let has = |cause: RedCause| causes.iter().any(|(c, _)| *c == cause);
    let cause = if has(RedCause::PrCaused) {
        RedCause::PrCaused
    } else if has(RedCause::Undetermined) {
        RedCause::Undetermined
    } else if has(RedCause::BaseRed) {
        RedCause::BaseRed
    } else {
        RedCause::ProbableFlake
    };
    RedVerdict {
        cause,
        base_red: red
            .checks
            .iter()
            .zip(&causes)
            .filter(|(_, (cause, _))| *cause == RedCause::BaseRed)
            .map(|(check, _)| check.name.clone())
            .collect(),
        reasons: causes.into_iter().map(|(_, reason)| reason).collect(),
    }
}

fn pr_item(
    class: Class,
    fact: &PrFact,
    suffix: &str,
    now: DateTime<Utc>,
    summary: String,
    action: String,
) -> Item {
    Item {
        key: format!(
            "{}:PR-{}@{}{suffix}",
            class.label(),
            fact.number,
            short(&fact.head)
        ),
        class,
        pr: Some(fact.number),
        seat: ADVISOR_SEAT.into(),
        age_mins: mins_since(fact.since, now),
        summary,
        action,
    }
}

fn merge_red_item(fact: &PrFact, names: &str, waited: &str, now: DateTime<Utc>) -> Item {
    let pr = fact.number;
    let head = short(&fact.head);
    let inspect = format!("read the failed job on #{pr}: rerun it once if the failure is unrelated to the change, otherwise route rework");
    let Some(red) = &fact.red else {
        return pr_item(
            Class::StuckMergeRed,
            fact,
            ":inspect",
            now,
            format!("#{pr} is approved at {head} but its CI is red ({names}), for {waited}; the cause was not examined"),
            inspect,
        );
    };
    let verdict = red_cause(red);
    let failing: BTreeSet<&String> = red
        .checks
        .iter()
        .filter_map(|check| check.tests.as_ref().ok())
        .flatten()
        .collect();
    let failing: Vec<String> = failing.into_iter().cloned().collect();
    let tests = if failing.is_empty() {
        "failing tests: not named".to_string()
    } else {
        format!("failing tests: {}", listed(&failing))
    };
    // A rerun that was routed and came back red on new check runs.
    let rerun_failed = red.rerun_routed.as_ref().is_some_and(|routed| {
        !red.checks.is_empty() && red.checks.iter().all(|check| !routed.contains(&check.id))
    });
    let (suffix, label, action) = match verdict.cause {
        RedCause::ProbableFlake if rerun_failed => (
            ":fix-forward",
            "probable flake that failed again after its rerun",
            format!("do not rerun #{pr} again: route a fix-forward for the flaky test(s), then land #{pr}"),
        ),
        RedCause::ProbableFlake => (
            ":rerun",
            "probable flake",
            format!("rerun the failed job on #{pr} once; if it fails again, route a fix-forward for the test, not rework of #{pr}"),
        ),
        RedCause::PrCaused => (
            ":rework",
            "the change plausibly caused it",
            format!("route rework of #{pr} for the failing tests"),
        ),
        RedCause::BaseRed => (
            ":base-red",
            "the base branch is red",
            format!("#{pr} needs no rework for that"),
        ),
        RedCause::Undetermined => (":inspect", "cause undetermined", inspect),
    };
    // A red base is repaired whatever else is wrong with the head.
    let (suffix, action) = if verdict.base_red.is_empty() {
        (suffix.to_string(), action)
    } else {
        let fix = format!(
            "fix the base branch forward for {} (red there too)",
            verdict.base_red.join(", ")
        );
        match verdict.cause {
            RedCause::BaseRed => (suffix.to_string(), format!("{fix}; {action}")),
            _ => (
                format!("{suffix}+base-red"),
                format!("{action}; also {fix}"),
            ),
        }
    };
    pr_item(
        Class::StuckMergeRed,
        fact,
        &suffix,
        now,
        format!(
            "#{pr} is approved at {head} but its CI is red ({names}), for {waited}; {tests}; {label}: {}",
            verdict.reasons.join("; ")
        ),
        action,
    )
}

/// The stall one change is in, or `None` while it is moving.
pub(crate) fn classify_pr(fact: &PrFact, policy: &Policy, now: DateTime<Utc>) -> Option<Item> {
    let age = mins_since(fact.since, now);
    let past = |limit: i64| age.is_some_and(|age| age > limit);
    let waited = age.map_or("an unknown time".to_string(), |m| format!("{m} min"));
    let pr = fact.number;
    let head = short(&fact.head);
    // Where the reconciler asks for a ruling, a lane is not the remedy.
    let remedy = |default: String| {
        if fact.action == Action::AdvisorRuling {
            format!("rule on #{pr}: {}", fact.reason)
        } else {
            default
        }
    };
    match &fact.standing {
        Standing::Landing | Standing::Unknown(_) | Standing::Reviewing(_) => None,
        Standing::Conflict { owner } => (owner.is_none() && past(policy.conflict_mins)).then(|| {
            pr_item(
                Class::StuckConflict,
                fact,
                "",
                now,
                format!("#{pr} at {head} conflicts with its base and nobody owns it, for {waited}"),
                remedy(format!("route a lane to resolve the conflict on #{pr}")),
            )
        }),
        Standing::Approved {
            red: Some(names),
            lane,
            ..
        } => {
            // A lane already on the red head is moving it.
            (lane.is_none() && past(policy.merge_mins))
                .then(|| merge_red_item(fact, names, &waited, now))
        }
        Standing::Approved {
            hold: None,
            stacked: true,
            ..
        } => None,
        Standing::Approved { approval, hold, .. } => past(policy.merge_mins).then(|| match hold {
            None => pr_item(
                Class::StuckMerge,
                fact,
                "",
                now,
                format!("#{pr} is approved at {head} and has not landed for {waited} ({approval})"),
                format!(
                    "land #{pr} (the reconciler shows {}: {})",
                    fact.state.label(),
                    fact.action.label()
                ),
            ),
            Some(hold) => match &hold.release {
                HoldRelease::Unmet(why) => pr_item(
                    Class::StuckMerge,
                    fact,
                    ":held",
                    now,
                    format!(
                        "#{pr} is approved at {head} but held, and has not landed for {waited}: {} ({why})",
                        hold.detail
                    ),
                    format!("rule on the hold on #{pr}, then land it"),
                ),
                HoldRelease::Met | HoldRelease::PendingCover(_) => pr_item(
                    Class::StuckMerge,
                    fact,
                    "",
                    now,
                    format!(
                        "#{pr} is approved at {head}, its hold can be released, and it has not landed for {waited}: {}",
                        hold.detail
                    ),
                    format!("release the hold on #{pr}, then land it"),
                ),
            },
        }),
        Standing::ChangesRequested { lane, round, red } => {
            (lane.is_none() && past(policy.rework_mins)).then(|| {
                pr_item(
                    Class::StuckRework,
                    fact,
                    "",
                    now,
                    format!(
                        "#{pr} has changes requested at {head} (round {round}{}) and no lane or session on it for {waited}",
                        if *red { ", CI red" } else { "" }
                    ),
                    remedy(format!("route rework of #{pr} to a lane")),
                )
            })
        }
        Standing::Unreviewed { red, lane, .. } => {
            // A lane on a red, unreviewed head is about to replace it.
            let reworking = red.is_some() && lane.is_some();
            (!reworking && past(policy.review_mins)).then(|| match red {
                None => pr_item(
                    Class::StuckReview,
                    fact,
                    "",
                    now,
                    format!("#{pr} at {head} has had no verdict and no reviewer for {waited}"),
                    remedy(format!("launch a reviewer for #{pr} at {head}")),
                ),
                Some(names) => pr_item(
                    Class::StuckReview,
                    fact,
                    "",
                    now,
                    format!("#{pr} at {head} has had no verdict, no reviewer and no lane for {waited}, and its CI is red ({names})"),
                    remedy(format!("launch a reviewer for #{pr} at {head}, or route a lane for its red CI")),
                ),
            })
        }
    }
}

/// Landings on the default branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Landings {
    pub window_mins: i64,
    /// `None` when the branch history could not be read.
    pub count: Option<usize>,
    /// Minutes since the newest landing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_mins_ago: Option<i64>,
    /// No landing for the escalation window.
    pub dry: bool,
    pub dry_after_mins: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HostLine {
    pub host: String,
    pub lanes: usize,
    /// Available memory, measured on the host the run is on.
    pub memory: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PrLine {
    pub pr: u64,
    pub class: Class,
    pub state: &'static str,
    pub head: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_mins: Option<i64>,
    pub title: String,
}

/// What one scheduled run observed, kept whether or not anything was stuck.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RunSnapshot {
    pub at: DateTime<Utc>,
    pub landings: Landings,
    pub hosts: Vec<HostLine>,
    pub changes: usize,
    pub stuck: usize,
}

/// Everything one run found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Report {
    pub at: DateTime<Utc>,
    pub changes: Vec<PrLine>,
    pub stuck: Vec<Item>,
    pub landings: Landings,
    pub hosts: Vec<HostLine>,
    /// What could not be read, and so is not classified.
    pub unknown: Vec<String>,
    /// No landing for the escalation window while something is stuck.
    pub escalate: bool,
    /// The newest scheduled run's saved observation (`aida land stalls`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_scheduled_run: Option<RunSnapshot>,
}

/// The CI detail read for one red head, kept so a later run reads only what
/// is still missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RedDetail {
    /// When its logs were last asked for.
    pub at: DateTime<Utc>,
    pub checks: Vec<FailedCheck>,
}

/// What the guard keeps between runs: what it routed, the CI detail it has
/// read, and what each scheduled run observed. Losing the file only
/// re-routes and re-reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Memory {
    /// Stall key -> when its due job was raised.
    #[serde(default)]
    pub routed: BTreeMap<String, DateTime<Utc>>,
    /// Routed stalls whose mail wake failed; the next run sends it again.
    #[serde(default)]
    pub mail_pending: BTreeSet<String>,
    /// `PR-<n>@<head>` -> the failed check runs a rerun was routed for.
    #[serde(default)]
    pub reruns: BTreeMap<String, Vec<u64>>,
    /// `PR-<n>@<head>` -> the CI detail read so far.
    #[serde(default)]
    pub red_detail: BTreeMap<String, RedDetail>,
    #[serde(default)]
    pub escalated_at: Option<DateTime<Utc>>,
    /// The scheduled runs' observations, oldest first.
    #[serde(default)]
    pub runs: Vec<RunSnapshot>,
}

fn rerun_key(pr: u64, head: &str) -> String {
    format!("PR-{pr}@{}", short(head))
}

/// The inputs of one report, gathered from production records or a test.
#[derive(Debug, Clone, Default)]
pub(crate) struct Facts {
    pub prs: Vec<PrFact>,
    /// When each landing in the escalation window happened; `Err` when the
    /// branch history could not be read.
    pub landings: Option<std::result::Result<Vec<DateTime<Utc>>, String>>,
    pub last_landing: Option<DateTime<Utc>>,
    pub hosts: Vec<HostLine>,
    pub unknown: Vec<String>,
}

/// The report for `facts`. Pure: `memory` supplies the reruns already
/// routed, and gains the reruns this report asks for.
pub(crate) fn build_report(
    facts: &Facts,
    policy: &Policy,
    memory: &mut Memory,
    now: DateTime<Utc>,
) -> Report {
    let mut unknown = facts.unknown.clone();
    let mut stuck = Vec::new();
    let mut changes = Vec::new();
    for fact in &facts.prs {
        let item = classify_pr(fact, policy, now);
        match &fact.standing {
            Standing::Unknown(what) => unknown.push(format!("#{}: {what}", fact.number)),
            Standing::Landing | Standing::Reviewing(_) => {}
            _ if fact.since.is_none() => unknown.push(format!(
                "#{}: how long it has waited is unknown, so it is not called stuck",
                fact.number
            )),
            _ => {}
        }
        // The failed check runs a rerun is asked for: the next run tells a
        // rerun that failed again (new check runs) from one nobody started.
        if let (Some(item), Some(red)) = (&item, &fact.red) {
            if item.key.ends_with(":rerun") && !red.checks.is_empty() {
                memory
                    .reruns
                    .entry(rerun_key(fact.number, &fact.head))
                    .or_insert_with(|| red.checks.iter().map(|check| check.id).collect());
            }
        }
        changes.push(PrLine {
            pr: fact.number,
            class: item.as_ref().map_or(Class::Moving, |item| item.class),
            state: fact.state.label(),
            head: short(&fact.head).to_string(),
            age_mins: mins_since(fact.since, now),
            title: super::one_line(&fact.title, 80),
        });
        stuck.extend(item);
    }
    stuck.sort_by(|a, b| a.class.cmp(&b.class).then(a.key.cmp(&b.key)));
    let window = Duration::minutes(policy.landings_window_mins);
    let dry_window = Duration::minutes(policy.escalate_after_mins);
    let (count, dry) = match &facts.landings {
        Some(Ok(times)) => (
            Some(times.iter().filter(|at| now - **at <= window).count()),
            !times.iter().any(|at| now - *at <= dry_window),
        ),
        Some(Err(why)) => {
            unknown.push(format!("landings: unknown ({why})"));
            (None, false)
        }
        None => (None, false),
    };
    let escalate = dry && !stuck.is_empty();
    Report {
        at: now,
        changes,
        stuck,
        landings: Landings {
            window_mins: policy.landings_window_mins,
            count,
            last_mins_ago: mins_since(facts.last_landing, now),
            dry,
            dry_after_mins: policy.escalate_after_mins,
        },
        hosts: facts.hosts.clone(),
        unknown,
        escalate,
        last_scheduled_run: None,
    }
}

/// Keep what a scheduled run observed: the landings, each host's lanes and
/// memory, and the counts, also when nothing is stuck.
pub(crate) fn record_run(memory: &mut Memory, report: &Report) {
    memory.runs.push(RunSnapshot {
        at: report.at,
        landings: report.landings.clone(),
        hosts: report.hosts.clone(),
        changes: report.changes.len(),
        stuck: report.stuck.len(),
    });
    let extra = memory.runs.len().saturating_sub(MAX_RUNS);
    memory.runs.drain(..extra);
}

fn snapshot_line(run: &RunSnapshot) -> String {
    let hosts = run
        .hosts
        .iter()
        .map(|host| {
            format!(
                "{}: {} lane(s), memory {}",
                host.host, host.lanes, host.memory
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "{}: {} landing(s) in {} min; {} open change(s), {} stuck; {}",
        run.at.format("%Y-%m-%d %H:%M UTC"),
        run.landings
            .count
            .map_or("unknown".to_string(), |n| n.to_string()),
        run.landings.window_mins,
        run.changes,
        run.stuck,
        if hosts.is_empty() {
            "no lanes".to_string()
        } else {
            hosts
        }
    )
}

pub(crate) fn render(report: &Report) -> String {
    let mut out = String::new();
    let age = |mins: Option<i64>| mins.map_or("-".to_string(), |m| format!("{m}m"));
    out.push_str(&format!(
        "{:<7} {:<16} {:<19} {:<9} {:>6}  {}\n",
        "CHANGE", "CLASS", "STATE", "HEAD", "WAIT", "TITLE"
    ));
    for line in &report.changes {
        out.push_str(&format!(
            "{:<7} {:<16} {:<19} {:<9} {:>6}  {}\n",
            format!("#{}", line.pr),
            line.class.label(),
            line.state,
            line.head,
            age(line.age_mins),
            line.title
        ));
    }
    if report.changes.is_empty() {
        out.push_str("(no open changes)\n");
    }
    out.push('\n');
    if report.stuck.is_empty() {
        out.push_str("Nothing is stuck.\n");
    } else {
        out.push_str(&format!("Stuck ({}):\n", report.stuck.len()));
        for item in &report.stuck {
            out.push_str(&format!(
                "  [{}] {}\n      -> {}: {}\n",
                item.class.label(),
                item.summary,
                item.seat,
                item.action
            ));
        }
    }
    out.push('\n');
    let landings = &report.landings;
    out.push_str(&format!(
        "Landings in the last {} min: {}; newest {} ago{}\n",
        landings.window_mins,
        landings
            .count
            .map_or("unknown".to_string(), |n| n.to_string()),
        age(landings.last_mins_ago),
        if landings.dry {
            format!(" (none for {} min)", landings.dry_after_mins)
        } else {
            String::new()
        }
    ));
    for host in &report.hosts {
        out.push_str(&format!(
            "Host {}: {} lane(s); memory {}\n",
            host.host, host.lanes, host.memory
        ));
    }
    for unknown in &report.unknown {
        out.push_str(&format!("Not read: {unknown}\n"));
    }
    if report.escalate {
        out.push_str("No landing inside the escalation window while changes are stuck.\n");
    }
    if let Some(run) = &report.last_scheduled_run {
        out.push_str(&format!("Last scheduled run {}\n", snapshot_line(run)));
    }
    out
}

// ---------------------------------------------------------------------------
// Production loaders
// ---------------------------------------------------------------------------

/// The forge reads the guard makes for CI detail, each bounded by `timeout`.
pub(crate) trait CiReads {
    /// The check runs at a commit or branch tip.
    fn check_runs(
        &self,
        rev: &str,
        timeout: std::time::Duration,
    ) -> std::result::Result<Vec<crate::ci_failures::CheckRun>, String>;
    /// The failing tests one job's log names.
    fn job_tests(
        &self,
        job: u64,
        timeout: std::time::Duration,
    ) -> std::result::Result<Vec<String>, String>;
}

struct Gh<'a>(&'a Path);

impl Gh<'_> {
    fn text(
        &self,
        args: &[&str],
        timeout: std::time::Duration,
    ) -> std::result::Result<String, String> {
        // Unit tests never reach a real forge.
        if cfg!(test) {
            return Err("the forge is not read in unit tests".into());
        }
        crate::forge::github_stall_read(self.0, args, timeout)
    }
}

impl CiReads for Gh<'_> {
    fn check_runs(
        &self,
        rev: &str,
        timeout: std::time::Duration,
    ) -> std::result::Result<Vec<crate::ci_failures::CheckRun>, String> {
        let path = format!("repos/{{owner}}/{{repo}}/commits/{rev}/check-runs?per_page=100");
        self.text(&["api", &path], timeout)
            .and_then(|json| crate::ci_failures::parse_check_runs(&json))
    }

    fn job_tests(
        &self,
        job: u64,
        timeout: std::time::Duration,
    ) -> std::result::Result<Vec<String>, String> {
        let path = format!("repos/{{owner}}/{{repo}}/actions/jobs/{job}/logs");
        self.text(&["api", &path], timeout)
            .map(|log| test_names(&crate::ci_failures::extract_failure_lines(&log)))
    }
}

/// The time a run may still spend on CI detail.
pub(crate) struct Budget<'a> {
    pub limit: std::time::Duration,
    /// Time spent since the detail reads began.
    pub elapsed: &'a dyn Fn() -> std::time::Duration,
}

impl Budget<'_> {
    /// The bound for the next forge call: what is left, at most one call's
    /// own bound. `Err` once nothing is left.
    pub(crate) fn call_timeout(&self) -> std::result::Result<std::time::Duration, String> {
        self.limit
            .checked_sub((self.elapsed)())
            .filter(|left| !left.is_zero())
            .map(|left| left.min(GH_TIMEOUT))
            .ok_or_else(|| "the time budget for this run is spent".to_string())
    }
}

fn run_failed(conclusion: &str) -> bool {
    matches!(conclusion, "failure" | "timed_out" | "startup_failure")
}

/// What the base's tip says about the check named `name`.
fn base_check(
    tip: &std::result::Result<Vec<crate::ci_failures::CheckRun>, String>,
    name: &str,
) -> BaseCheck {
    let runs = match tip {
        Ok(runs) => runs,
        Err(why) => return BaseCheck::Unknown(why.clone()),
    };
    let same: Vec<_> = runs.iter().filter(|run| run.name == name).collect();
    if same.iter().any(|run| run_failed(&run.conclusion)) {
        BaseCheck::Red
    } else if same.iter().any(|run| run.conclusion == "success") {
        BaseCheck::Pass
    } else {
        BaseCheck::Unknown(format!("`{name}` has not passed at the base branch's tip"))
    }
}

/// Read the failed checks of each red head in `heads` (`PR-<n>@<head>` key,
/// head commit), within `budget` and `max_red_logs`.
///
/// A log read once is kept in `memory.red_detail` and not asked for again, so
/// the allowance goes to the heads still missing detail, the ones never
/// asked for first: every red head is examined within a few runs however
/// many there are. Every forge call gets what is left of the budget; once it
/// is spent the rest is returned unread and the run carries on.
pub(crate) fn read_red_details(
    ci: &dyn CiReads,
    budget: &Budget,
    base_branch: &str,
    heads: &[(String, String)],
    memory: &mut Memory,
    policy: &Policy,
    now: DateTime<Utc>,
) -> BTreeMap<String, (Vec<FailedCheck>, Option<String>)> {
    let mut out = BTreeMap::new();
    memory
        .red_detail
        .retain(|key, _| heads.iter().any(|(head_key, _)| head_key == key));
    if heads.is_empty() {
        return out;
    }
    let tip = budget
        .call_timeout()
        .and_then(|timeout| ci.check_runs(base_branch, timeout));
    let mut order: Vec<&(String, String)> = heads.iter().collect();
    order.sort_by_key(|(key, _)| memory.red_detail.get(key).map(|detail| detail.at));
    let mut logs_read = 0;
    for (key, head) in order {
        let listed = budget
            .call_timeout()
            .and_then(|timeout| ci.check_runs(head, timeout));
        let runs = match listed {
            Ok(runs) => runs,
            Err(why) => {
                out.insert(key.clone(), (Vec::new(), Some(why)));
                continue;
            }
        };
        let known = memory.red_detail.get(key);
        let mut checks: Vec<FailedCheck> = runs
            .iter()
            .filter(|run| run_failed(&run.conclusion))
            .map(|run| FailedCheck {
                name: run.name.clone(),
                id: run.id,
                tests: known
                    .and_then(|detail| detail.checks.iter().find(|check| check.id == run.id))
                    .map_or(Err("not read yet".to_string()), |check| check.tests.clone()),
                base: base_check(&tip, &run.name),
            })
            .collect();
        if checks.iter().any(|check| check.tests.is_err()) {
            if logs_read >= policy.max_red_logs {
                for check in checks.iter_mut().filter(|check| check.tests.is_err()) {
                    check.tests = Err(format!(
                        "only {} red changes are read per run; this one is read on a later run",
                        policy.max_red_logs
                    ));
                }
            } else {
                logs_read += 1;
                for check in checks.iter_mut().filter(|check| check.tests.is_err()) {
                    let id = check.id;
                    check.tests = budget
                        .call_timeout()
                        .and_then(|timeout| ci.job_tests(id, timeout));
                }
                memory.red_detail.insert(
                    key.clone(),
                    RedDetail {
                        at: now,
                        checks: checks.clone(),
                    },
                );
            }
        }
        out.insert(key.clone(), (checks, None));
    }
    out
}

fn landing_times(
    root: &Path,
    branch: &str,
    since: DateTime<Utc>,
) -> (
    std::result::Result<Vec<DateTime<Utc>>, String>,
    Option<DateTime<Utc>>,
) {
    let pinned = super::base_ref(branch);
    let rev = if super::git(root, &["rev-parse", "--verify", "--quiet", &pinned], 10)
        .is_ok_and(|out| out.status.success())
    {
        pinned
    } else {
        format!("origin/{branch}")
    };
    let times = |args: &[&str]| -> std::result::Result<Vec<DateTime<Utc>>, String> {
        let out = super::git(root, args, 20).map_err(|e| format!("{e:#}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| DateTime::parse_from_rfc3339(line.trim()).ok())
            .map(|at| at.with_timezone(&Utc))
            .collect())
    };
    let since_arg = format!("--since={}", since.to_rfc3339());
    let window = times(&["log", "--first-parent", "--format=%cI", &since_arg, &rev]);
    let last = times(&["log", "--first-parent", "-1", "--format=%cI", &rev])
        .ok()
        .and_then(|times| times.into_iter().next());
    (window, last)
}

fn memory_line() -> String {
    let read = |key: &str, text: &str| -> Option<u64> {
        text.lines()
            .find_map(|line| line.strip_prefix(key))?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    };
    match std::fs::read_to_string("/proc/meminfo") {
        Ok(text) => match (read("MemAvailable:", &text), read("MemTotal:", &text)) {
            (Some(available), Some(total)) => format!(
                "{:.1} GiB available of {:.1} GiB",
                available as f64 / 1_048_576.0,
                total as f64 / 1_048_576.0
            ),
            _ => "unknown (unreadable /proc/meminfo)".into(),
        },
        Err(_) => "unknown (no /proc/meminfo on this host)".into(),
    }
}

fn host_lines(pass: &super::Pass) -> Vec<HostLine> {
    let here = crate::coordination::hostname();
    let mut lanes: BTreeMap<String, usize> = BTreeMap::new();
    *lanes.entry(here.clone()).or_default() += pass.local_lanes.len();
    for claim in &pass.foreign_lanes {
        let host = if claim.host.is_empty() {
            "unknown".to_string()
        } else {
            claim.host.clone()
        };
        *lanes.entry(host).or_default() += 1;
    }
    lanes
        .into_iter()
        .map(|(host, lanes)| HostLine {
            memory: if host == here {
                memory_line()
            } else {
                "unknown from this host (that host's own scheduled run records it)".into()
            },
            host,
            lanes,
        })
        .collect()
}

fn memory_path(root: &Path) -> PathBuf {
    root.join(".aida").join(CATEGORY).join("state.json")
}

fn load_memory(root: &Path) -> Memory {
    std::fs::read_to_string(memory_path(root))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_memory(root: &Path, memory: &Memory) {
    let path = memory_path(root);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string_pretty(memory) {
        Ok(body) => {
            if let Err(err) = aida_core::write_atomic(&path, body.as_bytes()) {
                eprintln!("warning: throughput guard state was not saved: {err:#}");
            }
        }
        Err(err) => eprintln!("warning: throughput guard state was not saved: {err:#}"),
    }
}

/// One live observation: the reconciler's pass, the same evidence read fact
/// by fact, and CI detail for the red approved heads. Returns the report and
/// the memory to keep (a scheduled run saves it; `aida land stalls` does
/// not).
fn observe(start: &Path, policy: &Policy, now: DateTime<Utc>) -> Result<(Report, Memory, PathBuf)> {
    let (pass, mut prs) = super::live_pass_with(start, |changes, rows, evidence| {
        rows.iter()
            .filter_map(|row| {
                let pr = changes.iter().find(|pr| pr.id == row.pr)?;
                Some(pr_fact(pr, row, evidence))
            })
            .collect::<Vec<PrFact>>()
    })?;
    let started = std::time::Instant::now();
    let elapsed = move || started.elapsed();
    let budget = Budget {
        limit: DETAIL_BUDGET,
        elapsed: &elapsed,
    };
    let root = pass.root.clone();
    let mut memory = load_memory(&root);
    let mut unknown = pass.warnings.clone();
    let branch = crate::forge::default_branch_of(&root);
    let red_heads: Vec<(String, String)> = prs
        .iter()
        .filter(|fact| fact.wants_red_detail())
        .map(|fact| (rerun_key(fact.number, &fact.head), fact.head.clone()))
        .collect();
    let mut details = if pass.gitlab {
        if !red_heads.is_empty() {
            unknown.push("failing test names: CI detail is read on GitHub only".into());
        }
        red_heads
            .iter()
            .map(|(key, _)| {
                let why = "CI detail is read on GitHub only".to_string();
                (key.clone(), (Vec::new(), Some(why)))
            })
            .collect()
    } else {
        read_red_details(
            &Gh(&root),
            &budget,
            &branch,
            &red_heads,
            &mut memory,
            policy,
            now,
        )
    };
    for fact in prs.iter_mut() {
        let key = rerun_key(fact.number, &fact.head);
        let Some((checks, unread)) = details.remove(&key) else {
            continue;
        };
        let changed_paths = match pass.changes.iter().find(|pr| pr.id == fact.number) {
            Some(pr) => super::changed_paths(&root, &super::base_ref(&pr.base), &pr.head),
            None => Err("the change left the pass".to_string()),
        };
        fact.red = Some(RedFact {
            checks,
            changed_paths,
            rerun_routed: memory.reruns.get(&key).cloned(),
            unread,
        });
    }
    let dry_since =
        now - Duration::minutes(policy.escalate_after_mins.max(policy.landings_window_mins));
    let (landings, last_landing) = landing_times(&root, &branch, dry_since);
    let facts = Facts {
        prs,
        landings: Some(landings),
        last_landing,
        hosts: host_lines(&pass),
        unknown,
    };
    let report = build_report(&facts, policy, &mut memory, now);
    Ok((report, memory, root))
}

/// `aida land stalls`.
pub(crate) fn run(start: &Path, json: bool) -> Result<()> {
    let cfg = crate::read_project_config_value(&crate::main_worktree_root_from(start));
    let policy = Policy::from_config(cfg.as_ref());
    let (mut report, memory, _root) = observe(start, &policy, Utc::now())?;
    report.last_scheduled_run = memory.runs.last().cloned();
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render(&report));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Routing (the scheduled tick only)
// ---------------------------------------------------------------------------

/// Schedule job name of the route for `seat`.
pub(crate) fn route_job_name(seat: &str) -> String {
    format!("{ROUTE_JOB_PREFIX}{seat}")
}

/// The seat a `throughput-guard-route-<seat>` job routes to, if it is one.
pub(crate) fn seat_for_job(job: &str) -> Option<&str> {
    job.strip_prefix(ROUTE_JOB_PREFIX).filter(|s| !s.is_empty())
}

/// The seat of a route job the guard actually raised: its ledger is in the
/// store before the job surfaces anywhere. Any other name is not a job.
pub(crate) fn known_route_seat<'a>(store_root: &Path, job: &'a str) -> Option<&'a str> {
    let seat = seat_for_job(job)?;
    schedule_ledger::ledger_path(store_root, job)
        .is_file()
        .then_some(seat)
}

/// The prompt the routed seat job carries.
pub(crate) fn route_prompt(seat: &str) -> String {
    let job = route_job_name(seat);
    if seat == OPERATOR_SEAT {
        format!(
            "Nothing has landed inside the escalation window while changes are stuck. Read the list in `aida schedule due --seat {seat}` (or run `aida land stalls`), then decide what to unblock first. Then run `aida schedule done {job}`."
        )
    } else {
        format!(
            "The throughput guard found stalled work the {seat} seat can move. Read the evidence in `aida schedule due --seat {seat}` (or run `aida land stalls`). Each line names the stall and the action it calls for: land an approved change, launch a reviewer, route rework or a conflict to a lane, rerun a probable flake once, route a fix-forward for a flake that failed its rerun, or repair a red base branch. Then run `aida schedule done {job}`."
        )
    }
}

fn clip(text: String) -> String {
    if text.chars().count() <= MAX_EVIDENCE_CHARS {
        return text;
    }
    let mut out: String = text.chars().take(MAX_EVIDENCE_CHARS).collect();
    out.push_str("… (run `aida land stalls` for the rest)");
    out
}

/// Mark `throughput-guard-route-<seat>` due with `evidence` and wake the seat.
fn raise_due(root: &Path, store: &Path, seat: &str, evidence: &str, now: DateTime<Utc>) -> bool {
    let job = route_job_name(seat);
    let already_due = schedule_ledger::load(store, &job).is_some_and(|l| l.due_since.is_some());
    let trip_id = format!(
        "{job}@{}",
        now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    );
    if let Err(err) = schedule_ledger::write_cas(store, &job, |ledger| {
        if ledger.due_since.is_none() {
            ledger.due_since = Some(now);
        }
        ledger.due_reason = Some(format!("on CronJobFired:{job}"));
        ledger.note = Some(evidence.to_string());
        ledger.due_failure = Some(schedule_ledger::RoutedFailure {
            trip_id: trip_id.clone(),
            performance: Vec::new(),
            job: Some(CATEGORY.to_string()),
            error: Some(evidence.to_string()),
        });
    }) {
        eprintln!("warning: could not write schedule ledger for '{job}': {err}");
        return false;
    }
    if !already_due {
        events::emit(
            root,
            &Event::new(
                None,
                "",
                EventKind::CronJobFired {
                    job,
                    seat: seat.to_string(),
                },
            ),
        );
    }
    true
}

/// How a wake mail is written; a test injects a mailbox that fails.
pub(crate) type Mailer<'a> = &'a dyn Fn(&Path, &aida_core::mailbox::Message) -> Result<()>;

/// A short `[fyi]` mail to the seat: the wake for a session that watches
/// its mail and not its due jobs.
fn mail_wake(
    root: &Path,
    seat: &str,
    new: &[&Item],
    total: usize,
    now: DateTime<Utc>,
    mailer: Mailer,
) -> Result<()> {
    use aida_core::mailbox::{Intent, Message, Recipient};
    let id = uuid::Uuid::new_v4().to_string();
    let lines = new
        .iter()
        .map(|item| {
            format!(
                "- [{}] {}\n  -> {}",
                item.class.label(),
                item.summary,
                item.action
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let msg = Message {
        subject: Some(format!(
            "Throughput guard: {} new stall(s) for the {seat} seat",
            new.len()
        )),
        id: id.clone(),
        thread_id: id,
        from: SENDER.to_string(),
        to: Recipient::Agent(seat.to_string()),
        timestamp: now.timestamp_millis(),
        in_reply_to: None,
        body: clip(format!(
            "{lines}\n\n{total} stall(s) in all are listed in `aida schedule due --seat {seat}`; `aida land stalls` prints the current picture. When done: `aida schedule done {}`.",
            route_job_name(seat)
        )),
        urgent: false,
        intent: Intent::Fyi,
        retracted: false,
        deleted: false,
        archived: false,
        from_source: aida_core::mailbox::SenderSource::Explicit,
        from_role: None,
        from_seat: None,
        relayed_from: None,
    };
    mailer(root, &msg)
}

/// What one routing pass decided for one seat.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SeatPlan {
    /// Every current stall of the seat.
    pub all: Vec<Item>,
    /// The ones not routed before.
    pub new: Vec<Item>,
    /// Routed before, but the mail wake failed: mailed again, with no
    /// second due job.
    pub remail: Vec<Item>,
}

/// What one routing pass decided, before anything is written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RoutePlan {
    pub seats: BTreeMap<String, SeatPlan>,
    pub escalate: bool,
}

/// Which seats to wake: a seat is woken only when it has a stall not routed
/// before (it then gets all of its current stalls in one item) or a routed
/// stall whose mail wake failed. The operator is escalated to at most once
/// per escalation window.
pub(crate) fn plan_routes(
    report: &Report,
    memory: &Memory,
    policy: &Policy,
    now: DateTime<Utc>,
) -> RoutePlan {
    let mut seats: BTreeMap<String, SeatPlan> = BTreeMap::new();
    for item in &report.stuck {
        let plan = seats.entry(item.seat.clone()).or_default();
        plan.all.push(item.clone());
        if !memory.routed.contains_key(&item.key) {
            plan.new.push(item.clone());
        } else if memory.mail_pending.contains(&item.key) {
            plan.remail.push(item.clone());
        }
    }
    seats.retain(|_, plan| !plan.new.is_empty() || !plan.remail.is_empty());
    let escalate = report.escalate
        && memory
            .escalated_at
            .is_none_or(|at| now - at >= Duration::minutes(policy.escalate_after_mins));
    RoutePlan { seats, escalate }
}

fn route(root: &Path, report: &Report, memory: &mut Memory, policy: &Policy, now: DateTime<Utc>) {
    route_with(root, report, memory, policy, now, &|root, msg| {
        crate::mailbox_store::write_message(root, msg)
    });
}

pub(crate) fn route_with(
    root: &Path,
    report: &Report,
    memory: &mut Memory,
    policy: &Policy,
    now: DateTime<Utc>,
    mailer: Mailer,
) {
    let store =
        crate::detect_distributed_store_from(root).unwrap_or_else(|| root.join(".aida-store"));
    // A stall that is gone is forgotten, so its return routes again.
    let current = |key: &String| report.stuck.iter().any(|item| &item.key == key);
    memory.routed.retain(|key, _| current(key));
    memory.mail_pending.retain(current);
    let plan = plan_routes(report, memory, policy, now);
    let open: BTreeSet<String> = report
        .changes
        .iter()
        .map(|line| format!("PR-{}@{}", line.pr, line.head))
        .collect();
    memory.reruns.retain(|key, _| open.contains(key));
    for (seat, plan) in &plan.seats {
        // The due job is raised for a new stall only; a failed mail alone
        // never raises it a second time.
        if !plan.new.is_empty() {
            let evidence = clip(
                plan.all
                    .iter()
                    .map(|item| {
                        format!(
                            "[{}] {} -> {}",
                            item.class.label(),
                            item.summary,
                            item.action
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
            );
            if !raise_due(root, &store, seat, &evidence, now) {
                continue;
            }
            for item in &plan.all {
                memory.routed.entry(item.key.clone()).or_insert(now);
            }
        }
        let wake: Vec<&Item> = plan.new.iter().chain(&plan.remail).collect();
        match mail_wake(root, seat, &wake, plan.all.len(), now, mailer) {
            Ok(()) => {
                for item in &wake {
                    memory.mail_pending.remove(&item.key);
                }
            }
            // The wake is owed: the next run sends it again.
            Err(err) => {
                eprintln!("warning: throughput guard could not mail the {seat} seat: {err:#}");
                memory
                    .mail_pending
                    .extend(wake.iter().map(|item| item.key.clone()));
            }
        }
    }
    if !report.landings.dry {
        memory.escalated_at = None;
    }
    if plan.escalate {
        let list = report
            .stuck
            .iter()
            .map(|item| format!("[{}] {}", item.class.label(), item.summary))
            .collect::<Vec<_>>();
        let evidence = clip(format!(
            "no landing for {} min with {} stall(s): {}",
            report.landings.dry_after_mins,
            list.len(),
            list.join("; ")
        ));
        if raise_due(root, &store, OPERATOR_SEAT, &evidence, now) {
            memory.escalated_at = Some(now);
            // A push when the project configured one; the due job is the record.
            let title = format!(
                "AIDA: no landing for {} min, {} stall(s)",
                report.landings.dry_after_mins,
                list.len()
            );
            if let Err(err) = crate::notify::send_direct_bounded(
                root,
                NOTIFY_RULE,
                &title,
                &list.join("\n"),
                Some(std::time::Duration::from_secs(10)),
            ) {
                eprintln!("warning: throughput guard operator notification failed: {err:#}");
            }
        }
    }
}

/// `aida doctor check throughput-guard`: the stalls as findings. With
/// `route` (the scheduler's tick) each new stall is also routed once, and
/// the run's observation (landings, lanes and memory per host) and the
/// guard's memory are saved, whether or not anything is stuck. Does nothing
/// unless the guard is enabled.
pub(crate) fn scan_and_route(
    project_root: &Path,
    route_findings: bool,
) -> Result<Vec<DoctorFinding>> {
    let cfg = crate::read_project_config_value(project_root);
    let policy = Policy::from_config(cfg.as_ref());
    if !policy.enabled {
        return Ok(Vec::new());
    }
    let now = Utc::now();
    let (report, mut memory, root) = observe(project_root, &policy, now)?;
    if route_findings {
        route(&root, &report, &mut memory, &policy, now);
        record_run(&mut memory, &report);
        save_memory(&root, &memory);
        if let Some(run) = memory.runs.last() {
            eprintln!("throughput-guard: {}", snapshot_line(run));
        }
    }
    for unknown in &report.unknown {
        eprintln!("throughput-guard: not read: {unknown}");
    }
    Ok(report
        .stuck
        .into_iter()
        .map(|item| DoctorFinding {
            category: CATEGORY.to_string(),
            id: format!("{CATEGORY}:{}", item.key),
            summary: item.summary,
            action: format!("{}: {}", item.seat, item.action),
            safe_heal: false,
        })
        .collect())
}

#[cfg(test)]
#[path = "tests/story_2906_throughput_guard_tests.rs"]
mod tests;
