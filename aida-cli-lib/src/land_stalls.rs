//! The throughput guard: which open changes are stuck, and who can move them.
//!
//! The seats' watchers are event-driven, and an absence of events wakes no
//! one: a push nobody reviews, an approval nobody lands, a lane that exits
//! and is never relaunched all look like silence. This module reads what the
//! landing reconciler already derives ([`super::live_pass`]) plus the lane
//! receipts, the shared claims and CI, and names every stall.
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

use super::{Action, Pass, State};
use crate::events::{self, Event, EventKind};
use crate::schedule_ledger;
use crate::DoctorFinding;

pub(crate) const CATEGORY: &str = "throughput-guard";
/// Prefix of the due jobs the guard raises, one per routed seat.
pub(crate) const ROUTE_JOB_PREFIX: &str = "throughput-guard-route-";
const OPERATOR_SEAT: &str = crate::mail_backlog::OPERATOR_SEAT;
const ADVISOR_SEAT: &str = "advisor";
const LAUNCH_SEAT: &str = "product";
const SENDER: &str = "aida-throughput-guard";
const NOTIFY_RULE: &str = "throughput-guard-escalation";
/// Bound on one forge call.
const GH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
/// Time the scan may spend reading CI logs before it reports the rest as
/// unread, so a scheduled run finishes well inside its kill ceiling.
const DETAIL_BUDGET: std::time::Duration = std::time::Duration::from_secs(75);
/// Receipts and claims older than this are no longer reported.
const MAX_AGE_HOURS: i64 = 24;
/// Longest evidence text one due job carries.
const MAX_EVIDENCE_CHARS: usize = 4000;
/// Longest test name read from a CI log.
const MAX_TEST_NAME_CHARS: usize = 200;
/// Failing tests one evidence line names before it counts the rest.
const MAX_LISTED_TESTS: usize = 8;

/// `[throughput_guard]` in `.aida/config.toml`. Every threshold is a project
/// setting; the scheduled check is off unless `enabled = true`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Policy {
    pub enabled: bool,
    pub merge_mins: i64,
    pub review_mins: i64,
    pub rework_mins: i64,
    pub conflict_mins: i64,
    pub lane_outcome_mins: i64,
    pub stale_claim_mins: i64,
    pub flake_window_hours: i64,
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
            lane_outcome_mins: 20,
            stale_claim_mins: 30,
            flake_window_hours: 24,
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
            ("lane_outcome_mins", &mut policy.lane_outcome_mins),
            ("stale_claim_mins", &mut policy.stale_claim_mins),
            ("flake_window_hours", &mut policy.flake_window_hours),
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
    #[serde(rename = "MAIN-FLAKE-RECURRENCE")]
    MainFlakeRecurrence,
    #[serde(rename = "LANE-OUTCOME")]
    LaneOutcome,
    #[serde(rename = "STALE-CLAIM")]
    StaleClaim,
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
            Class::MainFlakeRecurrence => "MAIN-FLAKE-RECURRENCE",
            Class::LaneOutcome => "LANE-OUTCOME",
            Class::StaleClaim => "STALE-CLAIM",
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

/// Why CI is red on an approved head.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RedCause {
    /// No failing test is in a file the change touches, and the same checks
    /// pass on the base.
    ProbableFlake,
    /// The change touches the code a failing test covers.
    PrCaused,
    /// The same check is red on the base too.
    BaseRed,
    Undetermined,
}

/// What the base's CI says about the checks that failed on a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BaseChecks {
    /// Every one of them passed at the base's tip.
    Pass,
    /// These are red at the base's tip.
    Red(Vec<String>),
    Unknown(String),
}

/// The facts behind one red, approved head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RedFact {
    pub failed_checks: Vec<String>,
    /// The forge's ids of the failed check runs; a rerun gives new ones.
    pub check_ids: Vec<u64>,
    pub failing_tests: Vec<String>,
    /// `Err` when the change's files could not be listed.
    pub changed_paths: std::result::Result<Vec<String>, String>,
    pub base: BaseChecks,
    /// The failed check runs a rerun was already routed for at this head.
    pub rerun_routed: Option<Vec<u64>>,
    /// Why the logs were not read, when they were not.
    pub unread: Option<String>,
}

/// One open change as the reconciler derived it, with what the guard adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrFact {
    pub number: u64,
    pub title: String,
    pub head: String,
    pub state: State,
    pub action: Action,
    /// A lane or reviewer working the change.
    pub actor: Option<String>,
    pub reason: String,
    /// CI on the head, as the reconciler labels it.
    pub ci: Option<String>,
    /// When the change was first seen in this state at this head.
    pub since: Option<DateTime<Utc>>,
    /// The approval standing at the head, if any.
    pub approval: Option<String>,
    pub red: Option<RedFact>,
}

/// One failing test seen somewhere: a base run, or a change's head.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct TestEvent {
    pub test: String,
    pub at: DateTime<Utc>,
    /// Where it happened: `run <id>#<attempt>` or `PR-<n>@<head>`.
    pub source: String,
    #[serde(default)]
    pub url: String,
}

/// A finished lane attempt that nobody followed up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaneOutcomeFact {
    pub attempt: String,
    pub spec: String,
    pub result: &'static str,
    pub reason_code: String,
    pub detail: String,
    pub finished_at: DateTime<Utc>,
    pub launched_by: Option<String>,
    /// A later attempt, a live lane or a live claim has the spec.
    pub followed_up: Option<String>,
}

/// A shared claim past its expiry, or a local lease whose holder cannot be
/// established, that `aida doctor heal stale-leases` leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaimFact {
    pub scope: String,
    pub record: PathBuf,
    /// Why the claim reads as dead.
    pub evidence: String,
    /// Why the doctor does not heal it; `None` when it does.
    pub unhealable: Option<String>,
    /// Minutes past the claim's own expiry (a lease: since its heartbeat).
    pub expired_mins: i64,
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

/// Why an approved head is red, with the reason in words. A heuristic, so
/// the reason always says what was compared.
pub(crate) fn red_cause(red: &RedFact) -> (RedCause, String) {
    if let BaseChecks::Red(names) = &red.base {
        return (
            RedCause::BaseRed,
            format!(
                "the same check is red on the base branch too ({})",
                names.join(", ")
            ),
        );
    }
    if let Some(why) = &red.unread {
        return (
            RedCause::Undetermined,
            format!("the failed jobs' logs were not read ({why})"),
        );
    }
    if red.failing_tests.is_empty() {
        return (
            RedCause::Undetermined,
            "no failing test is named in the failed jobs' logs".into(),
        );
    }
    let changed = match &red.changed_paths {
        Ok(changed) => changed,
        Err(why) => {
            return (
                RedCause::Undetermined,
                format!("the change's files could not be listed ({why})"),
            )
        }
    };
    if let Some((test, path)) = red
        .failing_tests
        .iter()
        .find_map(|test| touching_path(test, changed).map(|path| (test, path)))
    {
        return (
            RedCause::PrCaused,
            format!("the change touches `{path}`, which failing test `{test}` covers (matched by module path)"),
        );
    }
    match &red.base {
        BaseChecks::Pass => (
            RedCause::ProbableFlake,
            format!(
                "none of the {} failing test(s) is in a file this change touches (compared by module path with its {} changed file(s)), and the same check(s) pass on the base branch",
                red.failing_tests.len(),
                changed.len()
            ),
        ),
        BaseChecks::Unknown(why) => (
            RedCause::Undetermined,
            format!(
                "no failing test is in a file this change touches, but whether the same checks pass on the base branch is unknown ({why})"
            ),
        ),
        BaseChecks::Red(_) => unreachable!("handled above"),
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

/// The stall one change is in, or `None` while it is moving. `known_flakes`
/// are the tests already seen flaking inside the window.
pub(crate) fn classify_pr(
    fact: &PrFact,
    policy: &Policy,
    now: DateTime<Utc>,
    known_flakes: &[TestEvent],
) -> Option<Item> {
    let age = mins_since(fact.since, now);
    let past = |limit: i64| age.is_some_and(|age| age > limit);
    let waited = age.map_or("an unknown time".to_string(), |m| format!("{m} min"));
    let pr = fact.number;
    let head = short(&fact.head);
    let red_ci = fact.ci.as_deref().is_some_and(|ci| ci.starts_with("red"));
    let approved_red = (fact.state == State::CiFailed && fact.approval.is_some())
        || (matches!(fact.state, State::Ready | State::ReadyPendingCover) && red_ci);
    if approved_red {
        // A lane already on the red head is moving it.
        if !past(policy.merge_mins) || fact.actor.is_some() {
            return None;
        }
        let ci = fact.ci.clone().unwrap_or_else(|| "red".into());
        let Some(red) = &fact.red else {
            return Some(pr_item(
                Class::StuckMergeRed,
                fact,
                ":inspect",
                now,
                format!("#{pr} is approved at {head} but its CI is {ci}, for {waited}; the cause was not examined"),
                format!("read the failed job on #{pr}: rerun it once if the failure is unrelated to the change, otherwise route rework"),
            ));
        };
        let (cause, reason) = red_cause(red);
        let tests = if red.failing_tests.is_empty() {
            "failing tests: not named".to_string()
        } else {
            format!("failing tests: {}", listed(&red.failing_tests))
        };
        let earlier = red
            .failing_tests
            .iter()
            .find_map(|test| known_flakes.iter().find(|flake| &flake.test == test));
        // A rerun that was routed and came back red on new check runs.
        let rerun_failed = red.rerun_routed.as_ref().is_some_and(|routed| {
            !red.check_ids.is_empty() && red.check_ids.iter().all(|id| !routed.contains(id))
        });
        let (suffix, verdict, action) = match cause {
            RedCause::ProbableFlake if rerun_failed => (
                ":fix-forward",
                "probable flake that failed again after its rerun".to_string(),
                format!("do not rerun #{pr} again: route a fix-forward for the flaky test(s), then land #{pr}"),
            ),
            RedCause::ProbableFlake => match earlier {
                Some(flake) => (
                    ":fix-forward",
                    format!(
                        "probable flake seen before ({} at {})",
                        flake.source,
                        flake.at.format("%Y-%m-%d %H:%M UTC")
                    ),
                    format!("route a fix-forward for `{}` (it flaked before: {}), and rerun the failed job on #{pr} once", flake.test, if flake.url.is_empty() { flake.source.as_str() } else { flake.url.as_str() }),
                ),
                None => (
                    ":rerun",
                    "probable flake".to_string(),
                    format!("rerun the failed job on #{pr} once; if it fails again, route a fix-forward for the test, not rework of #{pr}"),
                ),
            },
            RedCause::PrCaused => (
                ":rework",
                "the change plausibly caused it".to_string(),
                format!("route rework of #{pr} for the failing tests"),
            ),
            RedCause::BaseRed => (
                ":base-red",
                "the base branch is red".to_string(),
                format!("fix the base branch forward first; #{pr} needs no rework for this"),
            ),
            RedCause::Undetermined => (
                ":inspect",
                "cause undetermined".to_string(),
                format!("read the failed job on #{pr}: rerun it once if the failure is unrelated to the change, otherwise route rework"),
            ),
        };
        return Some(pr_item(
            Class::StuckMergeRed,
            fact,
            suffix,
            now,
            format!(
                "#{pr} is approved at {head} but its CI is {ci}, for {waited}; {tests}; {verdict}: {reason}"
            ),
            action,
        ));
    }
    match (&fact.state, &fact.action) {
        (State::Ready, Action::Wait) => None,
        (State::Ready | State::ReadyPendingCover, _) if past(policy.merge_mins) => Some(pr_item(
            Class::StuckMerge,
            fact,
            "",
            now,
            format!(
                "#{pr} is approved at {head} and has not landed for {waited} ({})",
                fact.reason
            ),
            format!("land #{pr}: {}", fact.action.label()),
        )),
        (State::NeedsReview, _) if past(policy.review_mins) => Some(pr_item(
            Class::StuckReview,
            fact,
            "",
            now,
            format!("#{pr} at {head} has had no verdict and no reviewer for {waited}"),
            format!("launch a reviewer for #{pr} at {head}"),
        )),
        (State::ChangesRequested, _) if fact.actor.is_none() && past(policy.rework_mins) => {
            Some(pr_item(
                Class::StuckRework,
                fact,
                "",
                now,
                format!(
                    "#{pr} has changes requested at {head} and no lane or session on it for {waited}"
                ),
                format!("route rework of #{pr} to a lane"),
            ))
        }
        (State::Conflict, _) if fact.actor.is_none() && past(policy.conflict_mins) => {
            Some(pr_item(
                Class::StuckConflict,
                fact,
                "",
                now,
                format!(
                    "#{pr} at {head} {} and nobody owns it, for {waited}",
                    fact.reason
                ),
                format!("route a lane to resolve the conflict on #{pr}"),
            ))
        }
        _ => None,
    }
}

/// Flaky tests that failed again: a failure of a test after an earlier flake
/// of the same test, somewhere else, both inside the window.
pub(crate) fn flake_recurrences(
    flakes: &[TestEvent],
    failures: &[TestEvent],
    policy: &Policy,
    now: DateTime<Utc>,
) -> Vec<Item> {
    let window = Duration::hours(policy.flake_window_hours);
    let mut by_test: BTreeMap<&str, (&TestEvent, &TestEvent)> = BTreeMap::new();
    for failure in failures {
        if failure.at > now || now - failure.at > window {
            continue;
        }
        let earlier = flakes
            .iter()
            .filter(|flake| {
                flake.test == failure.test
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
    by_test
        .into_iter()
        .map(|(test, (earlier, again))| {
            let link = |event: &TestEvent| {
                if event.url.is_empty() {
                    event.source.clone()
                } else {
                    format!("{} {}", event.source, event.url)
                }
            };
            Item {
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
                    "`{test}` flaked at {} ({}) and failed again at {} ({})",
                    earlier.at.format("%Y-%m-%d %H:%M UTC"),
                    link(earlier),
                    again.at.format("%Y-%m-%d %H:%M UTC"),
                    link(again)
                ),
                action: format!(
                    "route a fix-forward for `{test}`; rerunning it again only hides it"
                ),
            }
        })
        .collect()
}

/// A registered seat name, or the launch seat when the receipt names none.
fn launch_seat(launched_by: Option<&str>) -> String {
    launched_by
        .map(|seat| crate::canonical_role_name(seat.trim()).to_ascii_lowercase())
        .filter(|seat| {
            !seat.is_empty()
                && seat != OPERATOR_SEAT
                && seat.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
        })
        .unwrap_or_else(|| LAUNCH_SEAT.to_string())
}

pub(crate) fn classify_lane_outcomes(
    outcomes: &[LaneOutcomeFact],
    policy: &Policy,
    now: DateTime<Utc>,
) -> Vec<Item> {
    outcomes
        .iter()
        .filter(|o| o.followed_up.is_none())
        .filter(|o| {
            let age = now - o.finished_at;
            age > Duration::minutes(policy.lane_outcome_mins)
                && age <= Duration::hours(MAX_AGE_HOURS)
        })
        .map(|o| Item {
            key: format!("{}:{}", Class::LaneOutcome.label(), o.attempt),
            class: Class::LaneOutcome,
            pr: None,
            seat: launch_seat(o.launched_by.as_deref()),
            age_mins: mins_since(Some(o.finished_at), now),
            summary: format!(
                "the lane for {} exited `{} reason={}` ({}) and nothing has relaunched or routed it",
                o.spec,
                o.result,
                o.reason_code,
                super::one_line(&o.detail, 300)
            ),
            action: format!(
                "relaunch the lane for {}, or route the spec (attempt {})",
                o.spec, o.attempt
            ),
        })
        .collect()
}

pub(crate) fn classify_claims(claims: &[ClaimFact], policy: &Policy) -> Vec<Item> {
    claims
        .iter()
        .filter(|c| c.expired_mins > policy.stale_claim_mins)
        .filter_map(|c| {
            let why = c.unhealable.as_deref()?;
            Some(Item {
                key: format!("{}:{}", Class::StaleClaim.label(), c.record.display()),
                class: Class::StaleClaim,
                pr: None,
                seat: ADVISOR_SEAT.into(),
                age_mins: Some(c.expired_mins),
                summary: format!(
                    "the claim on {} has been stale for {} min ({}); `aida doctor heal stale-leases` does not heal it: {why}; record {}",
                    c.scope,
                    c.expired_mins,
                    c.evidence,
                    c.record.display()
                ),
                action: format!(
                    "confirm the holder of {} is gone, then release the record by hand",
                    c.scope
                ),
            })
        })
        .collect()
}

/// Landings on the default branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Landings {
    pub window_mins: i64,
    /// `None` when the branch history could not be read.
    pub count: Option<usize>,
    /// Minutes since the newest landing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_mins_ago: Option<i64>,
    /// No landing for the escalation window.
    pub dry: bool,
    pub dry_after_mins: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct HostLine {
    pub host: String,
    pub lanes: usize,
    /// Available memory, measured on this host only.
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
}

/// What the guard keeps between runs: what it routed, and the flaky tests
/// it has seen. Losing the file only re-routes and forgets flake history.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Memory {
    #[serde(default)]
    pub routed: BTreeMap<String, DateTime<Utc>>,
    /// `PR-<n>@<head>` -> the failed check runs a rerun was routed for.
    #[serde(default)]
    pub reruns: BTreeMap<String, Vec<u64>>,
    #[serde(default)]
    pub flakes: Vec<TestEvent>,
    /// Failures on the base branch's own runs.
    #[serde(default)]
    pub base_failures: Vec<TestEvent>,
    /// Base runs already read, as `<id>#<attempt>`.
    #[serde(default)]
    pub seen_runs: BTreeMap<String, DateTime<Utc>>,
    #[serde(default)]
    pub escalated_at: Option<DateTime<Utc>>,
}

impl Memory {
    fn prune(&mut self, policy: &Policy, now: DateTime<Utc>) {
        let window = Duration::hours(policy.flake_window_hours);
        self.flakes.retain(|e| now - e.at <= window);
        self.base_failures.retain(|e| now - e.at <= window);
        // Runs stay listed by the forge for longer than the window.
        self.seen_runs.retain(|_, at| now - *at <= window * 4);
        self.flakes.sort();
        self.flakes.dedup();
        self.base_failures.sort();
        self.base_failures.dedup();
    }
}

fn rerun_key(pr: u64, head: &str) -> String {
    format!("PR-{pr}@{}", short(head))
}

/// The inputs of one report, gathered from production records or a test.
#[derive(Debug, Clone, Default)]
pub(crate) struct Facts {
    pub prs: Vec<PrFact>,
    pub lane_outcomes: Vec<LaneOutcomeFact>,
    pub claims: Vec<ClaimFact>,
    /// When each landing in the escalation window happened; `Err` when the
    /// branch history could not be read.
    pub landings: Option<std::result::Result<Vec<DateTime<Utc>>, String>>,
    pub last_landing: Option<DateTime<Utc>>,
    pub hosts: Vec<HostLine>,
    pub unknown: Vec<String>,
}

/// The report for `facts`. Pure: `memory` supplies flake history and the
/// reruns already routed, and is updated with the flakes this run saw.
pub(crate) fn build_report(
    facts: &Facts,
    policy: &Policy,
    memory: &mut Memory,
    now: DateTime<Utc>,
) -> Report {
    memory.prune(policy, now);
    let mut unknown = facts.unknown.clone();
    let mut stuck = Vec::new();
    let mut changes = Vec::new();
    let mut failures = memory.base_failures.clone();
    let mut new_flakes = Vec::new();
    for fact in &facts.prs {
        let known: Vec<TestEvent> = memory
            .flakes
            .iter()
            .filter(|flake| flake.source != rerun_key(fact.number, &fact.head))
            .cloned()
            .collect();
        let item = classify_pr(fact, policy, now, &known);
        if let Some(red) = &fact.red {
            let at = fact.since.unwrap_or(now);
            let source = rerun_key(fact.number, &fact.head);
            let events = red.failing_tests.iter().map(|test| TestEvent {
                test: test.clone(),
                at,
                source: source.clone(),
                url: String::new(),
            });
            failures.extend(events.clone());
            if red_cause(red).0 == RedCause::ProbableFlake {
                new_flakes.extend(events);
            }
            // The failed check runs a rerun is asked for: the next run tells
            // a rerun that failed again (new check runs) from one nobody
            // started.
            if item
                .as_ref()
                .is_some_and(|item| item.key.ends_with(":rerun"))
                && !red.check_ids.is_empty()
            {
                memory
                    .reruns
                    .entry(source.clone())
                    .or_insert_with(|| red.check_ids.clone());
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
    stuck.extend(flake_recurrences(&memory.flakes, &failures, policy, now));
    stuck.extend(classify_lane_outcomes(&facts.lane_outcomes, policy, now));
    stuck.extend(classify_claims(&facts.claims, policy));
    stuck.sort_by(|a, b| a.class.cmp(&b.class).then(a.key.cmp(&b.key)));
    for flake in new_flakes {
        if !memory.flakes.contains(&flake) {
            memory.flakes.push(flake);
        }
    }
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
    }
}

pub(crate) fn render(report: &Report) -> String {
    let mut out = String::new();
    let age = |mins: Option<i64>| mins.map_or("-".to_string(), |m| format!("{m}m"));
    out.push_str(&format!(
        "{:<7} {:<22} {:<19} {:<9} {:>6}  {}\n",
        "CHANGE", "CLASS", "STATE", "HEAD", "AGE", "TITLE"
    ));
    for line in &report.changes {
        out.push_str(&format!(
            "{:<7} {:<22} {:<19} {:<9} {:>6}  {}\n",
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
    out
}

// ---------------------------------------------------------------------------
// Production loaders
// ---------------------------------------------------------------------------

fn gh_text(root: &Path, args: &[&str]) -> std::result::Result<String, String> {
    // Unit tests never reach a real forge.
    if cfg!(test) {
        return Err("the forge is not read in unit tests".into());
    }
    crate::forge::github_stall_read(root, args, GH_TIMEOUT)
}

fn check_runs(
    root: &Path,
    rev: &str,
) -> std::result::Result<Vec<crate::ci_failures::CheckRun>, String> {
    let path = format!("repos/{{owner}}/{{repo}}/commits/{rev}/check-runs?per_page=100");
    gh_text(root, &["api", &path]).and_then(|json| crate::ci_failures::parse_check_runs(&json))
}

fn run_failed(conclusion: &str) -> bool {
    matches!(conclusion, "failure" | "timed_out" | "startup_failure")
}

/// The failing tests one job's log names.
fn job_tests(root: &Path, job: u64) -> std::result::Result<Vec<String>, String> {
    let path = format!("repos/{{owner}}/{{repo}}/actions/jobs/{job}/logs");
    gh_text(root, &["api", &path])
        .map(|log| test_names(&crate::ci_failures::extract_failure_lines(&log)))
}

/// What the default branch's tip says about `failed` check names.
fn base_checks(
    tip: &std::result::Result<Vec<crate::ci_failures::CheckRun>, String>,
    failed: &[String],
) -> BaseChecks {
    let runs = match tip {
        Ok(runs) => runs,
        Err(why) => return BaseChecks::Unknown(why.clone()),
    };
    let mut red = Vec::new();
    for name in failed {
        let same: Vec<_> = runs.iter().filter(|run| &run.name == name).collect();
        if same.iter().any(|run| run_failed(&run.conclusion)) {
            red.push(name.clone());
        } else if !same.iter().any(|run| run.conclusion == "success") {
            return BaseChecks::Unknown(format!(
                "check `{name}` has not passed at the base branch's tip"
            ));
        }
    }
    if red.is_empty() {
        BaseChecks::Pass
    } else {
        BaseChecks::Red(red)
    }
}

/// Read the base branch's recent runs: a run that went green on a rerun
/// gives flakes (the tests its earlier attempt failed); a run that ended red
/// gives failures. Each run attempt is read once.
fn read_base_runs(
    root: &Path,
    branch: &str,
    memory: &mut Memory,
    policy: &Policy,
    now: DateTime<Utc>,
    deadline: std::time::Instant,
    unknown: &mut Vec<String>,
) {
    let listed = gh_text(
        root,
        &[
            "run",
            "list",
            "--branch",
            branch,
            "--limit",
            "30",
            "--json",
            "databaseId,attempt,conclusion,status,updatedAt,url",
        ],
    )
    .and_then(|json| {
        serde_json::from_str::<Vec<serde_json::Value>>(&json)
            .map_err(|e| format!("unreadable run list: {e}"))
    });
    let runs = match listed {
        Ok(runs) => runs,
        Err(why) => {
            unknown.push(format!("flaky tests on `{branch}`: unknown ({why})"));
            return;
        }
    };
    let window = Duration::hours(policy.flake_window_hours);
    for run in runs {
        let (Some(id), Some(attempt)) = (run["databaseId"].as_u64(), run["attempt"].as_u64())
        else {
            continue;
        };
        if run["status"].as_str() != Some("completed") {
            continue;
        }
        let Some(at) = run["updatedAt"]
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|at| at.with_timezone(&Utc))
        else {
            continue;
        };
        let key = format!("{id}#{attempt}");
        if now - at > window || memory.seen_runs.contains_key(&key) {
            continue;
        }
        let conclusion = run["conclusion"].as_str().unwrap_or_default();
        let flaked = conclusion == "success" && attempt > 1;
        if !flaked && !run_failed(conclusion) {
            memory.seen_runs.insert(key, at);
            continue;
        }
        if std::time::Instant::now() > deadline {
            unknown.push(format!(
                "flaky tests on `{branch}`: run {key} not read this time (time budget)"
            ));
            continue;
        }
        // The attempt that failed: the one before a green rerun, or this one.
        let jobs_path = if flaked {
            format!(
                "repos/{{owner}}/{{repo}}/actions/runs/{id}/attempts/{}/jobs?per_page=100",
                attempt - 1
            )
        } else {
            format!(
                "repos/{{owner}}/{{repo}}/actions/runs/{id}/attempts/{attempt}/jobs?per_page=100"
            )
        };
        let jobs = gh_text(root, &["api", &jobs_path]).and_then(|json| {
            serde_json::from_str::<serde_json::Value>(&json)
                .map_err(|e| format!("unreadable jobs: {e}"))
        });
        let jobs = match jobs {
            Ok(jobs) => jobs,
            Err(why) => {
                unknown.push(format!(
                    "flaky tests on `{branch}`: run {key} unread ({why})"
                ));
                continue;
            }
        };
        let mut tests = BTreeSet::new();
        let mut complete = true;
        for job in jobs["jobs"].as_array().into_iter().flatten() {
            if !run_failed(job["conclusion"].as_str().unwrap_or_default()) {
                continue;
            }
            match job["id"].as_u64().map(|job| job_tests(root, job)) {
                Some(Ok(names)) => tests.extend(names),
                Some(Err(why)) => {
                    complete = false;
                    unknown.push(format!(
                        "flaky tests on `{branch}`: a job log of run {key} unread ({why})"
                    ));
                }
                None => {}
            }
        }
        let url = run["url"].as_str().unwrap_or_default().to_string();
        let events = tests.into_iter().map(|test| TestEvent {
            test,
            at,
            source: format!("run {key}"),
            url: url.clone(),
        });
        if flaked {
            memory.flakes.extend(events);
        } else {
            memory.base_failures.extend(events);
        }
        // An unread log is asked for again next time.
        if complete {
            memory.seen_runs.insert(key, at);
        }
    }
    memory.prune(policy, now);
}

fn red_fact(
    root: &Path,
    pr: &super::Pr,
    tip: &std::result::Result<Vec<crate::ci_failures::CheckRun>, String>,
    memory: &Memory,
    read_logs: std::result::Result<(), String>,
) -> RedFact {
    let mut fact = RedFact {
        failed_checks: Vec::new(),
        check_ids: Vec::new(),
        failing_tests: Vec::new(),
        changed_paths: super::changed_paths(root, &super::base_ref(&pr.base), &pr.head),
        base: BaseChecks::Unknown("not read".into()),
        rerun_routed: memory.reruns.get(&rerun_key(pr.id, &pr.head)).cloned(),
        unread: None,
    };
    if let Err(why) = read_logs {
        fact.unread = Some(why);
        return fact;
    }
    let runs = match check_runs(root, &pr.head) {
        Ok(runs) => runs,
        Err(why) => {
            fact.unread = Some(why);
            return fact;
        }
    };
    let mut tests = BTreeSet::new();
    for run in runs.iter().filter(|run| run_failed(&run.conclusion)) {
        fact.failed_checks.push(run.name.clone());
        fact.check_ids.push(run.id);
        match job_tests(root, run.id) {
            Ok(names) => tests.extend(names),
            Err(why) => fact.unread = Some(format!("{}: {why}", run.name)),
        }
    }
    fact.failing_tests = tests.into_iter().collect();
    if !fact.failing_tests.is_empty() {
        // Named tests are enough to judge by; a partly unread log is not
        // "undetermined".
        fact.unread = None;
    }
    fact.base = base_checks(tip, &fact.failed_checks);
    fact
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

fn host_lines(pass: &Pass) -> Vec<HostLine> {
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
                "not measured from this host".into()
            },
            host,
            lanes,
        })
        .collect()
}

fn lane_outcome_facts(pass: &Pass, unknown: &mut Vec<String>) -> Vec<LaneOutcomeFact> {
    use crate::lane_result::Kind;
    let (receipts, unreadable) = crate::lane_result::recorded(&pass.root);
    if unreadable > 0 {
        unknown.push(format!("{unreadable} lane receipt(s) could not be read"));
    }
    let same = aida_core::id_identity::same_id;
    receipts
        .iter()
        .filter_map(|r| {
            let kind = r.result?;
            let finished_at = r.finished_at?;
            if !matches!(
                kind,
                Kind::FailedAfterStart
                    | Kind::Punted
                    | Kind::NoChangeWithReason
                    | Kind::RefusedBeforeStart
            ) {
                return None;
            }
            let followed_up = receipts
                .iter()
                .find(|later| later.created_at > finished_at && same(&later.spec, &r.spec))
                .map(|later| format!("attempt {}", later.attempt))
                .or_else(|| {
                    pass.local_lanes
                        .iter()
                        .find(|lease| same(&lease.scope, &r.spec))
                        .map(|lease| format!("lease {}", lease.id))
                })
                .or_else(|| {
                    pass.foreign_lanes
                        .iter()
                        .find(|claim| same(&claim.scope, &r.spec))
                        .map(|claim| format!("a claim on {}", claim.host))
                });
            Some(LaneOutcomeFact {
                attempt: r.attempt.clone(),
                spec: r.spec.clone(),
                result: kind.label(),
                reason_code: r.reason_code.clone().unwrap_or_else(|| "unknown".into()),
                detail: r.detail.clone().unwrap_or_default(),
                finished_at,
                launched_by: r.launched_by.clone(),
                followed_up,
            })
        })
        .collect()
}

fn claim_facts(root: &Path, now: DateTime<Utc>) -> Vec<ClaimFact> {
    let store =
        crate::detect_distributed_store_from(root).unwrap_or_else(|| root.join(".aida-store"));
    let here = crate::coordination::hostname();
    let (_live, stale) =
        crate::coordination::partition_claims(crate::coordination::list_claims(&store), now, &here);
    // A local lease whose holder cannot be established blocks pickup of its
    // scope, and the doctor heals only a lease it can prove stale.
    let leases = crate::list_leases(root);
    let sessions = if leases.is_empty() {
        Vec::new()
    } else {
        crate::process_probe::probe_live_claude_sessions()
    };
    let unknown_leases = leases.into_iter().filter_map(|lease| {
        let state = crate::lease_state_for(&lease, &sessions, now);
        if state != aida_core::liveness::LeaseState::Unknown {
            return None;
        }
        let idle_mins = (now - crate::leases::lease_heartbeat(&lease)).num_minutes();
        Some(ClaimFact {
            record: crate::leases::lease_path(root, &lease.id),
            evidence: format!(
                "lease {} on branch `{}`: holder {}; recorded host `{}`, this host `{here}`; last heartbeat {idle_mins} min ago",
                lease.id,
                lease.branch,
                state.label(),
                lease.hostname
            ),
            scope: lease.scope,
            unhealable: Some(
                "its holder cannot be established from this host, and the doctor ends only a lease it can prove stale".into(),
            ),
            expired_mins: idle_mins,
        })
    });
    stale
        .into_iter()
        .filter_map(|(claim, evidence)| {
            let heartbeat = DateTime::parse_from_rfc3339(&claim.heartbeat_at)
                .ok()?
                .with_timezone(&Utc);
            let expired_mins = (now - heartbeat).num_minutes() - (claim.ttl_secs / 60) as i64;
            if expired_mins > MAX_AGE_HOURS * 60 * 7 {
                // Long-dead records are the store's housekeeping, not a stall.
                return None;
            }
            let healable =
                claim.claim_id.is_some() && crate::coordination::exited_holder_is_dead(&claim, now);
            let unhealable = (!healable).then(|| {
                if claim.claim_id.is_none() {
                    "the record carries no claim id".to_string()
                } else if claim.process_backed {
                    "it is a lock claim, not a lane's".to_string()
                } else if claim.review_verb {
                    "it is a review claim".to_string()
                } else if claim.host != here {
                    format!(
                        "its recorded host `{}` is not this host `{here}`, so its holder cannot be probed here",
                        claim.host
                    )
                } else {
                    "its lane's exit could not be proven from the lane record".to_string()
                }
            });
            Some(ClaimFact {
                record: crate::coordination::claim_path(&store, &claim.scope),
                scope: claim.scope,
                evidence,
                unhealable,
                expired_mins,
            })
        })
        .chain(unknown_leases)
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

/// One live observation: the reconciler's pass plus what the guard reads on
/// top of it. Returns the report and the memory to keep (a scheduled run
/// saves it; `aida land stalls` does not).
fn observe(start: &Path, policy: &Policy, now: DateTime<Utc>) -> Result<(Report, Memory, PathBuf)> {
    let started = std::time::Instant::now();
    let deadline = started + DETAIL_BUDGET;
    let pass = super::live_pass(start)?;
    let root = pass.root.clone();
    let mut memory = load_memory(&root);
    let mut unknown = pass.warnings.clone();
    let since = match super::metrics::observed_since(&root) {
        Ok(since) => since,
        Err(why) => {
            unknown.push(format!(
                "how long each change has waited: unknown ({why:#})"
            ));
            BTreeMap::new()
        }
    };
    let branch = crate::forge::default_branch_of(&root);
    let red_rows: Vec<u64> = pass
        .rows
        .iter()
        .filter(|row| {
            (row.state == State::CiFailed && pass.red_approvals.contains_key(&row.pr))
                || (matches!(row.state, State::Ready | State::ReadyPendingCover)
                    && row.ci.as_deref().is_some_and(|ci| ci.starts_with("red")))
        })
        .map(|row| row.pr)
        .collect();
    let on_github = !pass.gitlab;
    let tip = if !on_github {
        Err("CI detail is read on GitHub only".to_string())
    } else if red_rows.is_empty() {
        Err("not read".to_string())
    } else {
        check_runs(&root, &branch)
    };
    if on_github {
        read_base_runs(
            &root,
            &branch,
            &mut memory,
            policy,
            now,
            deadline,
            &mut unknown,
        );
    } else {
        unknown.push("flaky tests and failing test names: CI detail is read on GitHub only".into());
    }
    let mut logs_read = 0;
    let mut prs = Vec::new();
    for row in &pass.rows {
        let Some(pr) = pass.changes.iter().find(|pr| pr.id == row.pr) else {
            continue;
        };
        let red = red_rows.contains(&pr.id).then(|| {
            let allowed = if !on_github {
                Err("CI detail is read on GitHub only".to_string())
            } else if logs_read >= policy.max_red_logs {
                Err(format!(
                    "only {} red changes are read per run",
                    policy.max_red_logs
                ))
            } else if std::time::Instant::now() > deadline {
                Err("time budget for this run spent".to_string())
            } else {
                logs_read += 1;
                Ok(())
            };
            red_fact(&root, pr, &tip, &memory, allowed)
        });
        prs.push(PrFact {
            number: pr.id,
            title: pr.title.clone(),
            head: pr.head.clone(),
            state: row.state.clone(),
            action: row.action.clone(),
            actor: row.actor.clone(),
            reason: row.reason.clone(),
            ci: row.ci.clone(),
            since: since
                .get(&pr.id)
                .filter(|(head, state, _)| head == &pr.head && state == row.state.label())
                .map(|(_, _, at)| *at),
            approval: pass.red_approvals.get(&pr.id).cloned(),
            red,
        });
    }
    let dry_since =
        now - Duration::minutes(policy.escalate_after_mins.max(policy.landings_window_mins));
    let (landings, last_landing) = landing_times(&root, &branch, dry_since);
    let facts = Facts {
        prs,
        lane_outcomes: lane_outcome_facts(&pass, &mut unknown),
        claims: claim_facts(&root, now),
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
    let (report, _memory, _root) = observe(start, &policy, Utc::now())?;
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
            "The throughput guard found stalled work the {seat} seat can move. Read the evidence in `aida schedule due --seat {seat}` (or run `aida land stalls`). Each line names the stall and the action it calls for: land an approved change, launch a reviewer, route rework or a conflict to a lane, rerun a probable flake once, route a fix-forward for a recurring flake, relaunch or route a lane that exited, or release a claim nothing can heal. Then run `aida schedule done {job}`."
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

/// A short `[fyi]` mail to the seat: the wake for a session that watches
/// its mail and not its due jobs.
fn mail_wake(root: &Path, seat: &str, new: &[&Item], total: usize, now: DateTime<Utc>) {
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
    if let Err(err) = crate::mailbox_store::write_message(root, &msg) {
        eprintln!("warning: throughput guard could not mail the {seat} seat: {err:#}");
    }
}

/// What one routing pass decided, before anything is written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RoutePlan {
    /// Seat -> (every current stall of the seat, the new ones among them).
    pub seats: BTreeMap<String, (Vec<Item>, Vec<Item>)>,
    pub escalate: bool,
}

/// Which seats to wake: a seat is woken only when it has a stall not routed
/// before, and then gets all of its current stalls in one item. The operator
/// is escalated to at most once per escalation window.
pub(crate) fn plan_routes(
    report: &Report,
    memory: &Memory,
    policy: &Policy,
    now: DateTime<Utc>,
) -> RoutePlan {
    let mut seats: BTreeMap<String, (Vec<Item>, Vec<Item>)> = BTreeMap::new();
    for item in &report.stuck {
        let entry = seats.entry(item.seat.clone()).or_default();
        entry.0.push(item.clone());
        if !memory.routed.contains_key(&item.key) {
            entry.1.push(item.clone());
        }
    }
    seats.retain(|_, (_, new)| !new.is_empty());
    let escalate = report.escalate
        && memory
            .escalated_at
            .is_none_or(|at| now - at >= Duration::minutes(policy.escalate_after_mins));
    RoutePlan { seats, escalate }
}

fn route(root: &Path, report: &Report, memory: &mut Memory, policy: &Policy, now: DateTime<Utc>) {
    let store =
        crate::detect_distributed_store_from(root).unwrap_or_else(|| root.join(".aida-store"));
    let plan = plan_routes(report, memory, policy, now);
    // A stall that is gone is forgotten, so its return routes again.
    memory
        .routed
        .retain(|key, _| report.stuck.iter().any(|item| &item.key == key));
    let open: BTreeSet<String> = report
        .changes
        .iter()
        .map(|line| format!("PR-{}@{}", line.pr, line.head))
        .collect();
    memory.reruns.retain(|key, _| open.contains(key));
    for (seat, (all, new)) in &plan.seats {
        let evidence = clip(
            all.iter()
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
        mail_wake(root, seat, &new.iter().collect::<Vec<_>>(), all.len(), now);
        for item in all {
            memory.routed.entry(item.key.clone()).or_insert(now);
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
/// `route` (the scheduler's tick) each new stall is also routed once and the
/// guard's memory is saved. Does nothing unless the guard is enabled.
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
        save_memory(&root, &memory);
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
