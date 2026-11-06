//! STORY-2401 slice 2: one read type for forge observations.
//!
//! The forge (GitHub/GitLab) is eventually consistent. A read can answer
//! (`Known`), say it has not settled yet (`Pending`: GitHub's `UNKNOWN`
//! mergeable, a check rollup that has not registered), or fail for reasons
//! that say nothing about the answer (`Transient`: DNS, timeout, a missing
//! CLI, a 5xx). Before this type, all three collapsed into `Err`/`false` at
//! each call site, and a "no" read off a timeout could eject or close.
//!
//! Callers decide only on `Known`. `Pending` and `Transient` mean "not
//! known yet". `settle` handles non-destructive reads. Destructive callers
//! use `confirm_negative` or `confirm_snapshot` for repeated observations and
//! independent ground truth.
//!
//! The gh-read ratchet (`tests/story_2401_forge_read_ratchet_tests.rs`)
//! keeps new raw `gh` spawns out of files other than `forge*.rs`.
// trace:STORY-2401 | ai:claude

use std::process::Output;
use std::time::Duration;

/// One forge observation.
// trace:STORY-2401 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observed<T> {
    /// The forge answered and the answer is settled.
    Known(T),
    /// The forge answered but has not settled yet (GitHub's `UNKNOWN`
    /// mergeability, an unregistered check rollup). Re-read later.
    Pending(String),
    /// The read failed for a reason unrelated to the answer: missing CLI,
    /// timeout, network or server error. Re-read later.
    Transient(String),
}

impl<T> Observed<T> {
    pub(crate) fn map<U>(self, f: impl FnOnce(T) -> U) -> Observed<U> {
        match self {
            Observed::Known(v) => Observed::Known(f(v)),
            Observed::Pending(why) => Observed::Pending(why),
            Observed::Transient(why) => Observed::Transient(why),
        }
    }

    /// Chain a parse step that can itself decide `Pending`/`Transient`.
    pub(crate) fn and_then<U>(self, f: impl FnOnce(T) -> Observed<U>) -> Observed<U> {
        match self {
            Observed::Known(v) => f(v),
            Observed::Pending(why) => Observed::Pending(why),
            Observed::Transient(why) => Observed::Transient(why),
        }
    }

    /// The settled value, or `None` when the forge has not answered yet.
    pub(crate) fn known(self) -> Option<T> {
        match self {
            Observed::Known(v) => Some(v),
            _ => None,
        }
    }

    pub(crate) fn as_known(&self) -> Option<&T> {
        match self {
            Observed::Known(v) => Some(v),
            _ => None,
        }
    }

    pub(crate) fn is_settled(&self) -> bool {
        matches!(self, Observed::Known(_))
    }

    /// Why the value is not known; `None` for `Known`.
    pub(crate) fn unsettled_reason(&self) -> Option<&str> {
        match self {
            Observed::Known(_) => None,
            Observed::Pending(why) | Observed::Transient(why) => Some(why),
        }
    }

    /// Bridge from the `Result`-returning read methods: an `Err` says
    /// nothing about the answer, so it is `Transient`.
    pub(crate) fn from_result(r: anyhow::Result<T>) -> Self {
        match r {
            Ok(v) => Observed::Known(v),
            Err(e) => Observed::Transient(format!("{e:#}")),
        }
    }
}

impl Observed<bool> {
    /// True only for a settled `true`.
    pub(crate) fn is_known_true(&self) -> bool {
        matches!(self, Observed::Known(true))
    }

    /// True only for a settled `false`: the one value a refusal may act on.
    pub(crate) fn is_known_false(&self) -> bool {
        matches!(self, Observed::Known(false))
    }
}

/// What a `gh` invocation that ran said back.
// trace:STORY-2401 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GhReply {
    /// Exit 0; trimmed-at-end stdout.
    Ok(String),
    /// Non-zero exit whose stderr is not a known transient failure: the
    /// forge refused or answered "no such thing". Trimmed stderr.
    Refused(String),
}

/// Classify one `gh` run. `None` (the CLI was missing, could not spawn, or
/// hit its timeout) is `Transient`; a non-zero exit is `Transient` when its
/// stderr matches the shared network-transient allow-list
/// (`network_retry::default_transient_patterns`), else `Known(Refused)`.
/// This is the one place that turns a raw `gh` exit status into a decision.
// trace:STORY-2401 | ai:claude
pub(crate) fn observe_gh(out: Option<Output>) -> Observed<GhReply> {
    let Some(out) = out else {
        return Observed::Transient("gh did not run or timed out".to_string());
    };
    if out.status.success() {
        return Observed::Known(GhReply::Ok(
            String::from_utf8_lossy(&out.stdout).trim_end().to_string(),
        ));
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    // trace:STORY-2401 | ai:codex
    if out.status.code().is_none() || stderr.is_empty() {
        Observed::Transient("gh exited without a usable reply".into())
    } else if stderr_is_transient(&stderr) {
        Observed::Transient(stderr)
    } else {
        Observed::Known(GhReply::Refused(stderr))
    }
}

/// [`observe_gh`] narrowed to "stdout of a successful read": a refusal is
/// reported as `Transient` with the stderr, for reads whose callers have no
/// use for a definitive refusal (a policy read, a compare).
// trace:STORY-2401 | ai:claude
pub(crate) fn observe_gh_stdout(out: Option<Output>) -> Observed<String> {
    observe_gh(out).and_then(|reply| match reply {
        GhReply::Ok(stdout) => Observed::Known(stdout),
        GhReply::Refused(stderr) => Observed::Transient(stderr),
    })
}

fn stderr_is_transient(stderr: &str) -> bool {
    // The shared retry list names common 5xx messages, but every HTTP 5xx
    // (and HTTP 429) describes an unavailable read, not a settled refusal.
    // trace:STORY-2401 | ai:codex
    static HTTP_TRANSIENT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\bHTTP(?:/\d(?:\.\d)?)?\s+(?:5\d\d|429)\b").unwrap()
    });
    // external-prose-classifier: forge_observed::stderr_is_transient
    let lower = stderr.to_ascii_lowercase();
    HTTP_TRANSIENT.is_match(stderr)
        || crate::network_retry::default_transient_patterns()
            .iter()
            .any(|p| !p.is_empty() && lower.contains(&p.to_ascii_lowercase()))
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("rate limit")
}

/// Re-read until the observation settles, at most `attempts` reads with
/// `backoff` between them. Returns the last observation, so a caller that
/// still sees `Pending`/`Transient` knows it has an unconfirmed answer and
/// must not take a destructive branch on it.
// trace:STORY-2401 | ai:claude
pub(crate) fn settle<T>(
    attempts: u32,
    backoff: Duration,
    mut read: impl FnMut() -> Observed<T>,
) -> Observed<T> {
    let attempts = attempts.max(1);
    let mut last = read();
    for _ in 1..attempts {
        if last.is_settled() {
            break;
        }
        std::thread::sleep(backoff);
        last = read();
    }
    last
}

/// A negative is actionable only after two matching reads and independent
/// ground truth. Unavailable or contradictory evidence never grants mutation.
// trace:STORY-2401 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Confirmed {
    Positive,
    Negative,
    Unconfirmed,
}

/// `true` means the change landed. Require two consecutive negative reads,
/// separated by backoff, and a fresh negative Git observation before cleanup.
/// A positive forge answer wins; Git alone proves landing only when the forge
/// never supplied a contradictory negative.
// trace:STORY-2401 | ai:codex
pub(crate) fn confirm_negative(
    attempts: u32,
    mut read: impl FnMut() -> Observed<bool>,
    ground_truth: impl FnOnce() -> Observed<bool>,
    mut pause: impl FnMut(),
) -> Confirmed {
    let mut consecutive = 0;
    let mut saw_negative = false;
    for attempt in 0..attempts.max(2) {
        if attempt > 0 {
            pause();
        }
        match read() {
            Observed::Known(true) => return Confirmed::Positive,
            Observed::Known(false) => {
                saw_negative = true;
                consecutive += 1;
                if consecutive >= 2 {
                    break;
                }
            }
            _ => consecutive = 0,
        }
    }
    match ground_truth() {
        Observed::Known(false) if consecutive >= 2 => Confirmed::Negative,
        Observed::Known(true) if !saw_negative => Confirmed::Positive,
        _ => Confirmed::Unconfirmed,
    }
}

/// Confirm a complete snapshot before a status flip or publication retraction.
/// Unlike `settle`, a single settled answer is insufficient. The independent
/// probe must agree with the stable snapshot; reads changing head/base/author
/// cannot be combined into permission to act.
// trace:STORY-2401 | ai:codex
pub(crate) fn confirm_snapshot<T: PartialEq>(
    mut read: impl FnMut() -> Observed<T>,
    ground_truth: impl FnOnce(&T) -> Observed<bool>,
    mut pause: impl FnMut(),
) -> Observed<T> {
    let mut previous = None;
    let mut unavailable = None;
    for attempt in 0..3 {
        if attempt > 0 {
            pause();
        }
        match read() {
            Observed::Known(value) => {
                if previous.as_ref() == Some(&value) {
                    return match ground_truth(&value) {
                        Observed::Known(true) => Observed::Known(value),
                        Observed::Known(false) => {
                            Observed::Pending("forge state conflicts with Git evidence".into())
                        }
                        Observed::Pending(why) => Observed::Pending(why),
                        Observed::Transient(why) => Observed::Transient(why),
                    };
                }
                previous = Some(value);
                unavailable = None;
            }
            Observed::Pending(why) => {
                previous = None;
                unavailable = Some((false, why));
            }
            Observed::Transient(why) => {
                previous = None;
                unavailable = Some((true, why));
            }
        }
    }
    match unavailable {
        Some((true, why)) => Observed::Transient(why),
        Some((false, why)) => Observed::Pending(why),
        None => Observed::Pending("forge state could not be confirmed on re-read".into()),
    }
}

#[cfg(test)]
#[path = "tests/story_2401_forge_observed_tests.rs"]
mod forge_observed_tests;

#[cfg(test)]
#[path = "tests/story_2401_forge_read_ratchet_tests.rs"]
mod forge_read_ratchet_tests;
