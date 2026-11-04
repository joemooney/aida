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
//! known yet". A destructive caller re-reads with [`settle`], which keeps
//! the re-read loop bounded and in one place.
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
    if stderr_is_transient(&stderr) {
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
    let lower = stderr.to_ascii_lowercase();
    crate::network_retry::default_transient_patterns()
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

#[cfg(test)]
#[path = "tests/story_2401_forge_observed_tests.rs"]
mod forge_observed_tests;

#[cfg(test)]
#[path = "tests/story_2401_forge_read_ratchet_tests.rs"]
mod forge_read_ratchet_tests;
