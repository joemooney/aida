//! Load-aware wall-clock budgets for tests.
//!
//! `assert!(started.elapsed() < budget)` is not a statement about the code
//! under test — it is a statement about the code *and* the host it ran on.
//! On an oversubscribed host correct code misses the budget, and the bare
//! assertion reports a host condition as a product defect. That is how two
//! `aida-cli-lib` tests came to be red on developer machines while CI stayed
//! green, which trains everyone to read local red as noise.
//!
//! [`assert_within_budget`] makes the host assumption explicit: the budget is
//! judged only on a host quiet enough for it to mean anything, and either
//! outcome names the load it was measured against. Prefer asserting bounded
//! *work* (bytes read, files scanned, syscalls) where the code exposes it —
//! that is host-independent and needs none of this.
// trace:BUG-1730 | ai:claude

use std::time::{Duration, Instant};

/// What a wall-clock budget is worth, given the load the host was under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BudgetVerdict {
    /// Inside the budget. Nothing to say.
    Within,
    /// Over budget on a host with a spare core: the precondition a wall-clock
    /// claim needs was met, so the overrun is the code's.
    OverOnQuietHost,
    /// Over budget on an oversubscribed host: the budget is unjudgeable and
    /// must not be reported as a defect.
    OverOnLoadedHost,
}

/// The whole decision, as a pure function of measurements, so it can be tested
/// without arranging a particular host load.
///
/// A host is quiet enough to judge a wall-clock budget when its 1-minute load
/// average leaves at least one core of headroom. `load` of zero — what
/// `sysinfo` reports on platforms with no load average — reads as quiet, which
/// keeps the budget enforced rather than silently skipped.
pub(crate) fn judge_budget(
    elapsed: Duration,
    budget: Duration,
    load: f64,
    cpus: usize,
) -> BudgetVerdict {
    if elapsed < budget {
        BudgetVerdict::Within
    } else if load > cpus as f64 {
        BudgetVerdict::OverOnLoadedHost
    } else {
        BudgetVerdict::OverOnQuietHost
    }
}

/// Judge a wall-clock budget against the load the host was actually under.
///
/// Passes inside `budget`. Over budget on a quiet host the test fails, naming
/// the load it was judged against; over budget on an oversubscribed host the
/// budget is reported and skipped, because it says nothing under those
/// conditions.
pub(crate) fn assert_within_budget(started: Instant, budget: Duration, what: &str) {
    let elapsed = started.elapsed();
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let load = sysinfo::System::load_average().one;
    match judge_budget(elapsed, budget, load, cpus) {
        BudgetVerdict::Within => {}
        BudgetVerdict::OverOnLoadedHost => eprintln!(
            "{what}: took {elapsed:?}, over its {budget:?} budget, but the host was \
             oversubscribed (1-minute load {load:.2} across {cpus} cpus). A wall-clock \
             budget says nothing under those conditions, so it was not enforced. \
             trace:BUG-1730"
        ),
        BudgetVerdict::OverOnQuietHost => panic!(
            "{what}: took {elapsed:?}, over its {budget:?} budget, on a host that was NOT \
             oversubscribed (1-minute load {load:.2} across {cpus} cpus). The precondition \
             a wall-clock budget needs was met, so this is a real slowdown, not host \
             contention."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_the_budget_is_within_whatever_the_host_is_doing() {
        let inside = Duration::from_millis(999);
        let budget = Duration::from_secs(1);
        assert_eq!(
            judge_budget(inside, budget, 0.1, 8),
            BudgetVerdict::Within,
            "a quiet host inside budget"
        );
        assert_eq!(
            judge_budget(inside, budget, 97.0, 8),
            BudgetVerdict::Within,
            "load is irrelevant while the budget is met"
        );
    }

    #[test]
    fn over_budget_on_a_quiet_host_is_the_codes_fault() {
        assert_eq!(
            judge_budget(Duration::from_secs(9), Duration::from_secs(1), 1.2, 8),
            BudgetVerdict::OverOnQuietHost
        );
    }

    #[test]
    fn over_budget_on_an_oversubscribed_host_is_not_judged() {
        assert_eq!(
            judge_budget(Duration::from_secs(9), Duration::from_secs(1), 8.1, 8),
            BudgetVerdict::OverOnLoadedHost,
            "BUG-1730: 10 runnable tasks on 6 cores is why these tests went red"
        );
    }

    /// The boundary belongs to the quiet side: a host loaded to exactly its
    /// core count still has no task waiting, so the budget stays enforced.
    /// Erring the other way would let a genuine regression hide behind a
    /// fully-but-not-over-subscribed host.
    #[test]
    fn a_fully_subscribed_host_is_still_judged() {
        assert_eq!(
            judge_budget(Duration::from_secs(9), Duration::from_secs(1), 8.0, 8),
            BudgetVerdict::OverOnQuietHost
        );
    }

    /// Platforms with no load average report 0.0 through `sysinfo`. That must
    /// enforce the budget, never skip it — a silent skip everywhere would be
    /// worse than the flake this replaces.
    #[test]
    fn a_platform_without_a_load_average_still_enforces_the_budget() {
        assert_eq!(
            judge_budget(Duration::from_secs(9), Duration::from_secs(1), 0.0, 1),
            BudgetVerdict::OverOnQuietHost
        );
    }

    /// Exactly at the budget is over it — the same `<` the assertions it
    /// replaces used, kept so no test silently gains a tick of slack.
    #[test]
    fn exactly_at_the_budget_is_over_it() {
        assert_eq!(
            judge_budget(Duration::from_secs(1), Duration::from_secs(1), 0.0, 8),
            BudgetVerdict::OverOnQuietHost
        );
    }
}
