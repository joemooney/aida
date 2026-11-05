//! Typed conditions: the small, machine-checkable language for "when does this
//! come back" (deferral triggers), relay handoff conditionals and recurrence
//! escalation.
//!
//! A condition is one line of text with a canonical form:
//!
//! | Form | Met when |
//! |---|---|
//! | `spec:STORY-12=completed` | the spec has that status |
//! | `pr:2577=merged` | the pull request is merged |
//! | `release:>=0.17` | the latest release tag satisfies the comparison |
//! | `date:2026-12-01` | today (UTC) is on or after the date |
//! | `query:tag=operator-held,status=approved count>=5` | the number of matching, non-archived specs satisfies the comparison |
//! | `manual:joe` | never by machine; a person decides |
//!
//! [`Condition`] parses with [`std::str::FromStr`] and prints its canonical
//! form with [`std::fmt::Display`]; the two round-trip. Evaluation goes
//! through [`ConditionFacts`], so this module does no I/O and tests need no
//! git, store or `gh`.
//!
//! Evaluation has four outcomes ([`Verdict`]). `Unknown` is not `NotMet`: a
//! fact that could not be observed right now (for example `gh` is offline)
//! must never be read as an answer, so a caller must not act on it.
// trace:STORY-2402 | ai:claude

use std::fmt;
use std::str::FromStr;

use chrono::NaiveDate;

use crate::models::{Requirement, RequirementStatus};

/// The forms a condition can take, for error hints.
// trace:STORY-2402 | ai:claude
pub const CONDITION_FORMS: &str = "spec:<ID>=<status>, pr:<N>=merged, release:>=<version>, \
     date:<YYYY-MM-DD>, query:<key>=<value>[,...] count<op><n>, manual:<who>";

/// A typed condition.
// trace:STORY-2402 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// `spec:<ID>=<status>`: the spec has the given status.
    Spec {
        id: String,
        status: RequirementStatus,
    },
    /// `pr:<N>=merged`: the pull request is merged.
    PrMerged { number: u64 },
    /// `release:<op><version>`: the latest release satisfies the comparison.
    Release { op: CmpOp, version: Version },
    /// `date:<YYYY-MM-DD>`: today (UTC) is on or after the date.
    Date { on: NaiveDate },
    /// `query:<filters> count<op><n>`: the count of matching, non-archived
    /// specs satisfies the comparison.
    Query {
        filters: Vec<QueryFilter>,
        op: CmpOp,
        n: u64,
    },
    /// `manual:<who>`: a named person decides; never met by machine.
    Manual { who: String },
}

/// A comparison operator in `release:` and `query:` conditions.
// trace:STORY-2402 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Ge,
    Gt,
    Le,
    Lt,
    Eq,
}

impl CmpOp {
    /// Split a leading operator off `s`. Two-character operators are tried
    /// first so `>=` is not read as `>` followed by `=`.
    fn split_prefix(s: &str) -> Option<(CmpOp, &str)> {
        const OPS: [(&str, CmpOp); 6] = [
            (">=", CmpOp::Ge),
            ("<=", CmpOp::Le),
            ("==", CmpOp::Eq),
            (">", CmpOp::Gt),
            ("<", CmpOp::Lt),
            ("=", CmpOp::Eq),
        ];
        OPS.iter()
            .find_map(|(token, op)| s.strip_prefix(token).map(|rest| (*op, rest)))
    }

    fn holds(self, ord: std::cmp::Ordering) -> bool {
        use std::cmp::Ordering::*;
        match self {
            CmpOp::Ge => ord != Less,
            CmpOp::Gt => ord == Greater,
            CmpOp::Le => ord != Greater,
            CmpOp::Lt => ord == Less,
            CmpOp::Eq => ord == Equal,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            CmpOp::Ge => ">=",
            CmpOp::Gt => ">",
            CmpOp::Le => "<=",
            CmpOp::Lt => "<",
            CmpOp::Eq => "=",
        }
    }
}

/// A dotted numeric release version (`0.17`, `0.17.2`). A leading `v` is
/// accepted on input and dropped. Missing trailing components compare as
/// zero, so `0.17` equals `0.17.0`.
// trace:STORY-2402 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version(Vec<u64>);

impl Version {
    /// Compare two versions, padding the shorter with zeros.
    pub fn cmp_padded(&self, other: &Version) -> std::cmp::Ordering {
        let len = self.0.len().max(other.0.len());
        let at = |v: &Version, i: usize| v.0.get(i).copied().unwrap_or(0);
        (0..len)
            .map(|i| at(self, i).cmp(&at(other, i)))
            .find(|ord| ord.is_ne())
            .unwrap_or(std::cmp::Ordering::Equal)
    }

    /// The highest version among release tag names (`v0.17.1`, `0.16`),
    /// ignoring tags that are not plain dotted numbers (pre-releases and
    /// other names).
    pub fn latest_of<'a>(tags: impl IntoIterator<Item = &'a str>) -> Option<Version> {
        tags.into_iter()
            .filter_map(|tag| tag.trim().parse::<Version>().ok())
            .max_by(|a, b| a.cmp_padded(b))
    }
}

impl FromStr for Version {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let digits = s.strip_prefix(['v', 'V']).unwrap_or(s);
        if digits.is_empty() {
            return Err(format!("`{s}` is not a version"));
        }
        digits
            .split('.')
            .map(|part| {
                if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(format!("`{s}` is not a version like 0.17 or 0.17.2"));
                }
                part.parse::<u64>()
                    .map_err(|_| format!("`{s}` has a component that is too large"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Version)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self.0.iter().map(u64::to_string).collect();
        f.write_str(&parts.join("."))
    }
}

/// One `key=value` filter of a `query:` condition. The keys are the ones
/// `aida list` filters on: tag, status, type and feature. Values match
/// case-insensitively; type also matches its ID prefix (`story`, `bug`).
// trace:STORY-2402 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryFilter {
    Tag(String),
    Status(RequirementStatus),
    Type(String),
    Feature(String),
}

impl QueryFilter {
    fn parse(s: &str) -> Result<Self, ConditionParseError> {
        let (key, value) = s.split_once('=').ok_or_else(|| {
            ConditionParseError::new(format!(
                "query filter `{s}` must be <key>=<value> (keys: tag, status, type, feature)"
            ))
        })?;
        let value = value.trim();
        if value.is_empty() {
            return Err(ConditionParseError::new(format!(
                "query filter `{s}` has no value"
            )));
        }
        match key.trim().to_ascii_lowercase().as_str() {
            "tag" => Ok(QueryFilter::Tag(value.to_string())),
            "status" => RequirementStatus::from_filter_str(value)
                .map(QueryFilter::Status)
                .ok_or_else(|| ConditionParseError::new(format!("`{value}` is not a spec status"))),
            "type" => Ok(QueryFilter::Type(value.to_string())),
            "feature" => Ok(QueryFilter::Feature(value.to_string())),
            other => Err(ConditionParseError::new(format!(
                "unknown query filter key `{other}` (keys: tag, status, type, feature)"
            ))),
        }
    }

    /// Whether `req` passes this filter.
    pub fn matches(&self, req: &Requirement) -> bool {
        match self {
            QueryFilter::Tag(tag) => req.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)),
            QueryFilter::Status(status) => req.status == *status,
            QueryFilter::Type(ty) => {
                let want = word_key(ty);
                word_key(&req.req_type.to_string()) == want
                    || req.req_type.default_prefix().eq_ignore_ascii_case(ty)
            }
            QueryFilter::Feature(feature) => req.feature.eq_ignore_ascii_case(feature),
        }
    }
}

impl fmt::Display for QueryFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryFilter::Tag(v) => write!(f, "tag={v}"),
            QueryFilter::Status(s) => write!(f, "status={}", status_token(s)),
            QueryFilter::Type(v) => write!(f, "type={v}"),
            QueryFilter::Feature(v) => write!(f, "feature={v}"),
        }
    }
}

/// Count the non-archived requirements that pass every filter (an empty
/// filter list counts every non-archived requirement).
// trace:STORY-2402 | ai:claude
pub fn count_matching<'a>(
    filters: &[QueryFilter],
    reqs: impl IntoIterator<Item = &'a Requirement>,
) -> u64 {
    reqs.into_iter()
        .filter(|req| !req.archived && filters.iter().all(|f| f.matches(req)))
        .count() as u64
}

/// Lowercase and drop space, `-` and `_`, so "Change Request",
/// "change-request" and "changerequest" compare equal.
fn word_key(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_'))
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The canonical lowercase, hyphenated token for a status (`in-progress`).
// trace:STORY-2402 | ai:claude
pub fn status_token(status: &RequirementStatus) -> &'static str {
    match status {
        RequirementStatus::Draft => "draft",
        RequirementStatus::Approved => "approved",
        RequirementStatus::Planned => "planned",
        RequirementStatus::InProgress => "in-progress",
        RequirementStatus::Done => "done",
        RequirementStatus::Completed => "completed",
        RequirementStatus::Rejected => "rejected",
        RequirementStatus::Superseded => "superseded",
        RequirementStatus::NeedsAttention => "needs-attention",
    }
}

/// Why a condition string did not parse. Its message names the problem and
/// lists the accepted forms.
// trace:STORY-2402 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionParseError {
    message: String,
}

impl ConditionParseError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ConditionParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}. Accepted forms: {CONDITION_FORMS}", self.message)
    }
}

impl std::error::Error for ConditionParseError {}

/// A single token with no whitespace, or an error naming `what`.
fn single_token<'a>(value: &'a str, what: &str) -> Result<&'a str, ConditionParseError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ConditionParseError::new(format!("missing {what}")));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(ConditionParseError::new(format!(
            "{what} `{value}` must not contain spaces"
        )));
    }
    Ok(value)
}

impl FromStr for Condition {
    type Err = ConditionParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        let (kind, rest) = input.split_once(':').ok_or_else(|| {
            ConditionParseError::new(format!("`{input}` is not a typed condition"))
        })?;
        match kind.trim().to_ascii_lowercase().as_str() {
            "spec" => {
                let (id, status) = rest.split_once('=').ok_or_else(|| {
                    ConditionParseError::new("spec condition must be spec:<ID>=<status>")
                })?;
                let id = single_token(id, "spec ID")?;
                let status_text = status.trim();
                let status = RequirementStatus::from_filter_str(status_text).ok_or_else(|| {
                    ConditionParseError::new(format!("`{status_text}` is not a spec status"))
                })?;
                Ok(Condition::Spec {
                    id: id.to_string(),
                    status,
                })
            }
            "pr" => {
                let (number, state) = rest.split_once('=').ok_or_else(|| {
                    ConditionParseError::new("pr condition must be pr:<N>=merged")
                })?;
                let number = number.trim();
                let digits = number.strip_prefix('#').unwrap_or(number);
                let number = digits.parse::<u64>().map_err(|_| {
                    ConditionParseError::new(format!("`{number}` is not a pull request number"))
                })?;
                // trace:STORY-2402 | ai:codex
                if number == 0 {
                    return Err(ConditionParseError::new(
                        "pull request number must be positive",
                    ));
                }
                if !state.trim().eq_ignore_ascii_case("merged") {
                    return Err(ConditionParseError::new(format!(
                        "pr state `{}` is not supported; use merged",
                        state.trim()
                    )));
                }
                Ok(Condition::PrMerged { number })
            }
            "release" => {
                let rest = rest.trim();
                let (op, version) = CmpOp::split_prefix(rest).unwrap_or((CmpOp::Ge, rest));
                let version = version
                    .trim()
                    .parse::<Version>()
                    .map_err(ConditionParseError::new)?;
                Ok(Condition::Release { op, version })
            }
            "date" => {
                let text = rest.trim();
                let on = NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|_| {
                    ConditionParseError::new(format!("`{text}` is not a date like 2026-12-01"))
                })?;
                Ok(Condition::Date { on })
            }
            "query" => {
                // The final token is the count predicate; filter values may
                // contain spaces (for example type=Change Request).
                // trace:STORY-2402 | ai:codex
                let rest = rest.trim();
                let (filter_text, count_text) = match rest.rsplit_once(char::is_whitespace) {
                    Some((filters, count)) => (Some(filters.trim()), count),
                    None => (None, rest),
                };
                let filters = filter_text
                    .map(|text| {
                        text.split(',')
                            .map(QueryFilter::parse)
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .transpose()?
                    .unwrap_or_default();
                let comparison = count_text.strip_prefix("count").ok_or_else(|| {
                    ConditionParseError::new(format!(
                        "`{count_text}` must be count<op><n>, for example count>=5"
                    ))
                })?;
                let (op, n) = CmpOp::split_prefix(comparison).ok_or_else(|| {
                    ConditionParseError::new(format!(
                        "`{count_text}` needs a comparison: >=, >, <=, < or ="
                    ))
                })?;
                let n = n.parse::<u64>().map_err(|_| {
                    ConditionParseError::new(format!("`{n}` is not a whole number"))
                })?;
                Ok(Condition::Query { filters, op, n })
            }
            "manual" => {
                let who = single_token(rest, "manual owner")?;
                Ok(Condition::Manual {
                    who: who.to_string(),
                })
            }
            other => Err(ConditionParseError::new(format!(
                "unknown condition kind `{other}`"
            ))),
        }
    }
}

impl fmt::Display for Condition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Condition::Spec { id, status } => write!(f, "spec:{id}={}", status_token(status)),
            Condition::PrMerged { number } => write!(f, "pr:{number}=merged"),
            Condition::Release { op, version } => write!(f, "release:{}{version}", op.as_str()),
            Condition::Date { on } => write!(f, "date:{}", on.format("%Y-%m-%d")),
            Condition::Query { filters, op, n } => {
                f.write_str("query:")?;
                if !filters.is_empty() {
                    let parts: Vec<String> = filters.iter().map(ToString::to_string).collect();
                    write!(f, "{} ", parts.join(","))?;
                }
                write!(f, "count{}{n}", op.as_str())
            }
            Condition::Manual { who } => write!(f, "manual:{who}"),
        }
    }
}

/// The facts a condition is evaluated against. The CLI implements it over
/// the requirement store, `gh` and git tags; tests use a fake.
// trace:STORY-2402 | ai:claude
pub trait ConditionFacts {
    /// The spec's status, `Ok(None)` when no spec has that ID, `Err` when
    /// the store could not be read.
    fn spec_status(&self, id: &str) -> Result<Option<RequirementStatus>, String>;
    /// Whether the pull request is merged; `None` when it could not be
    /// observed right now.
    fn pr_merged(&self, number: u64) -> Option<bool>;
    /// The latest release, `Ok(None)` when nothing has been released,
    /// `Err` when it could not be observed right now.
    fn latest_release(&self) -> Result<Option<Version>, String>;
    /// Today's date (UTC).
    fn today(&self) -> NaiveDate;
    /// The number of non-archived specs passing every filter; `Err` when
    /// the store could not be read.
    fn count(&self, filters: &[QueryFilter]) -> Result<u64, String>;
}

/// The outcome of evaluating a condition.
// trace:STORY-2402 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The condition holds.
    Met,
    /// The condition does not hold yet.
    NotMet,
    /// A fact could not be observed right now. Not an answer: never act on
    /// it as if the condition were met or not met.
    Unknown,
    /// A `manual:` condition; the named person decides.
    Manual,
}

impl Verdict {
    /// A lowercase, stable name for output.
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Met => "met",
            Verdict::NotMet => "not-met",
            Verdict::Unknown => "unknown",
            Verdict::Manual => "manual",
        }
    }
}

/// A verdict and what was observed to reach it, in plain words.
// trace:STORY-2402 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    pub verdict: Verdict,
    pub observed: String,
}

/// A condition that cannot be evaluated as written (its spec does not
/// exist), or a fact source that failed internally (the store is
/// unreadable). Unlike [`Verdict::Unknown`], retrying later will not help.
// trace:STORY-2402 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalError(pub String);

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EvalError {}

impl Condition {
    /// Evaluate the condition against `facts`.
    // trace:STORY-2402 | ai:claude
    pub fn evaluate(&self, facts: &dyn ConditionFacts) -> Result<Evaluation, EvalError> {
        let judged = |met: bool, observed: String| Evaluation {
            verdict: if met { Verdict::Met } else { Verdict::NotMet },
            observed,
        };
        match self {
            Condition::Spec { id, status } => {
                let actual = facts
                    .spec_status(id)
                    .map_err(|e| EvalError(format!("could not read the spec store: {e}")))?
                    .ok_or_else(|| EvalError(format!("no spec has the ID {id}")))?;
                Ok(judged(
                    actual == *status,
                    format!("{id} is {}", status_token(&actual)),
                ))
            }
            Condition::PrMerged { number } => Ok(match facts.pr_merged(*number) {
                Some(true) => judged(true, format!("pull request #{number} is merged")),
                Some(false) => judged(false, format!("pull request #{number} is not merged")),
                None => Evaluation {
                    verdict: Verdict::Unknown,
                    observed: format!("could not read the state of pull request #{number}"),
                },
            }),
            Condition::Release { op, version } => Ok(match facts.latest_release() {
                Ok(Some(latest)) => judged(
                    op.holds(latest.cmp_padded(version)),
                    format!("the latest release is {latest}"),
                ),
                Ok(None) => judged(false, "no release has been tagged".to_string()),
                Err(e) => Evaluation {
                    verdict: Verdict::Unknown,
                    observed: format!("could not read the latest release: {e}"),
                },
            }),
            Condition::Date { on } => {
                let today = facts.today();
                Ok(judged(
                    today >= *on,
                    format!("today is {}", today.format("%Y-%m-%d")),
                ))
            }
            Condition::Query { filters, op, n } => {
                let count = facts
                    .count(filters)
                    .map_err(|e| EvalError(format!("could not read the spec store: {e}")))?;
                Ok(judged(
                    op.holds(count.cmp(n)),
                    format!("{count} matching specs"),
                ))
            }
            Condition::Manual { who } => Ok(Evaluation {
                verdict: Verdict::Manual,
                observed: format!("waiting on {who} to decide"),
            }),
        }
    }
}

#[cfg(test)]
#[path = "condition_tests.rs"]
mod tests;
