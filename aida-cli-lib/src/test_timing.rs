//! Load-aware wall-clock budgets for tests.
//!
//! `assert!(started.elapsed() < budget)` is not a statement about the code
//! under test — it is a statement about the code *and* the host it ran on.
//! On an oversubscribed host correct code misses the budget, and the bare
//! assertion reports a host condition as a product defect. That is how two
//! `aida-cli-lib` tests came to be red on developer machines while CI stayed
//! green, which trains everyone to read local red as noise.
//!
//! [`assert_within_budget`] makes the host assumption explicit, and splits the
//! two jobs a single `<` was doing:
//!
//! * a **hard ceiling** that no amount of load excuses. This is the only
//!   thing here that can fail a test, and it is enforced everywhere,
//!   including on a busy CI runner.
//!
//!   It is a *coarse backstop*, not a hang detector: it is checked after the
//!   operation returns, so a call that never returns is caught by the test
//!   harness or the CI job timeout, never by this. What it does catch is an
//!   operation that returns after an order-of-magnitude overrun — notably a
//!   `scan` grinding on toward its own 600s internal budget, which nothing
//!   else here would notice.
//!
//!   Callers set it near ten times nominal (60s against a ~7s configured
//!   worst case). That is a deliberate policy choice, and it is not provably
//!   unreachable: a suspended VM or an extreme scheduler stall could cross it
//!   with nothing wrong. The trade is accepted because *something* must be
//!   able to fail or the timing assertion says nothing at all, and at ten
//!   times nominal a crossing is worth a look on its own. If one ever fires
//!   on a healthy host, raise it — do not re-gate it on load.
//! * a tighter **budget** that is a performance signal. Missing it is
//!   *reported*, never fatal.
//!
//! The budget cannot fail a test because nothing available here can prove the
//! host was quiet across the measured interval. `load_average().one` is
//! sampled after the fact and averaged over a minute, so a burst overlapping
//! a short test need not show in it; and `available_parallelism()` may report
//! a container's CPU quota while the load average describes the whole host,
//! which makes the two operands not strictly comparable under CI
//! containerization. A signal that weak may *excuse* a slow run — that only
//! ever costs a skipped diagnostic — but it must never *accuse* one, which
//! would reintroduce exactly the false red this module exists to remove.
//!
//! So the real coverage belongs elsewhere: assert bounded *work* (bytes read,
//! files scanned) where the code exposes it, and let the ceiling catch hangs.
//! Both hold on any host.
// trace:BUG-1730 | ai:claude

use std::time::{Duration, Instant};

/// What a wall-clock measurement is worth, given the load the host was under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BudgetVerdict {
    /// Inside the budget. Nothing to say.
    Within,
    /// Past the hard ceiling. No host is slow enough to explain this, so it is
    /// a failure regardless of load.
    PastHardCeiling,
    /// Over budget on a host that looked quiet. Worth reporting — it is the
    /// shape a performance regression takes — but not fatal, because a
    /// lagging one-minute average cannot prove the host was quiet while the
    /// measurement ran.
    OverOnQuietHost,
    /// Over budget on a host that was visibly oversubscribed: not even worth
    /// reporting as a suspicion.
    OverOnLoadedHost,
}

/// The whole decision, as a pure function of measurements, so it can be tested
/// without arranging a particular host load.
///
/// The ceiling is checked first and ignores `load` entirely, so a hang is
/// caught on any host. Below it, a host is quiet enough to judge the budget
/// when its 1-minute load average leaves at least one core of headroom. `load`
/// of zero — what `sysinfo` reports on platforms with no load average — reads
/// as quiet, which keeps the budget enforced rather than silently skipped.
///
/// The load average lags by design, so a spike inside a short test may not
/// show, and `OverOnQuietHost` therefore means only "nothing visibly explains
/// this", not "the host was provably idle". [`assert_within_budget`] treats it
/// as a report rather than a failure for that reason.
pub(crate) fn judge_budget(
    elapsed: Duration,
    budget: Duration,
    ceiling: Duration,
    load: f64,
    cpus: usize,
) -> BudgetVerdict {
    if elapsed >= ceiling {
        BudgetVerdict::PastHardCeiling
    } else if elapsed < budget {
        BudgetVerdict::Within
    } else if load > cpus as f64 {
        BudgetVerdict::OverOnLoadedHost
    } else {
        BudgetVerdict::OverOnQuietHost
    }
}

impl BudgetVerdict {
    /// Whether this verdict fails the test. Only the ceiling does: see the
    /// module docs for why a load average may excuse a slow run but must
    /// never accuse one.
    pub(crate) fn is_fatal(self) -> bool {
        matches!(self, BudgetVerdict::PastHardCeiling)
    }
}

/// Judge a wall-clock measurement against a hard ceiling and, separately, a
/// performance budget weighed against the load the host was under.
///
/// Only `ceiling` can fail the test, and it does so on any host. Missing
/// `budget` is reported on stderr with the load it was measured against and
/// nothing more — see the module docs for why a load average may excuse a
/// slow run but must never accuse one.
pub(crate) fn assert_within_budget(
    started: Instant,
    budget: Duration,
    ceiling: Duration,
    what: &str,
) {
    let elapsed = started.elapsed();
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let load = sysinfo::System::load_average().one;
    let verdict = judge_budget(elapsed, budget, ceiling, load, cpus);
    debug_assert_eq!(
        verdict.is_fatal(),
        matches!(verdict, BudgetVerdict::PastHardCeiling),
        "only the ceiling may fail a test"
    );
    match verdict {
        BudgetVerdict::Within => {}
        BudgetVerdict::PastHardCeiling => panic!(
            "{what}: took {elapsed:?}, past the {ceiling:?} hard ceiling (1-minute load \
             {load:.2} across {cpus} cpus). A hang is unbounded, so no host load explains \
             this — it hung or it regressed by an order of magnitude."
        ),
        BudgetVerdict::OverOnLoadedHost => eprintln!(
            "BUDGET-SKIPPED {what}: took {elapsed:?}, over its {budget:?} budget but \
             inside the {ceiling:?} ceiling, on an oversubscribed host (1-minute load \
             {load:.2} across {cpus} cpus). Not judged. trace:BUG-1730"
        ),
        BudgetVerdict::OverOnQuietHost => eprintln!(
            "BUDGET-MISSED {what}: took {elapsed:?}, over its {budget:?} budget but \
             inside the {ceiling:?} ceiling, and the host did not look busy (1-minute \
             load {load:.2} across {cpus} cpus). Nothing visibly explains this, so it \
             may be a real slowdown — but a lagging load average cannot prove the host \
             was quiet while this ran, so it is reported, not failed. Grep CI logs for \
             BUDGET-MISSED if these accumulate. trace:BUG-1730"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CEILING: Duration = Duration::from_secs(30);

    #[test]
    fn inside_the_budget_is_within_whatever_the_host_is_doing() {
        let inside = Duration::from_millis(999);
        let budget = Duration::from_secs(1);
        assert_eq!(
            judge_budget(inside, budget, CEILING, 0.1, 8),
            BudgetVerdict::Within,
            "a quiet host inside budget"
        );
        assert_eq!(
            judge_budget(inside, budget, CEILING, 97.0, 8),
            BudgetVerdict::Within,
            "load is irrelevant while the budget is met"
        );
    }

    #[test]
    fn over_budget_on_a_quiet_host_is_the_codes_fault() {
        assert_eq!(
            judge_budget(
                Duration::from_secs(9),
                Duration::from_secs(1),
                CEILING,
                1.2,
                8
            ),
            BudgetVerdict::OverOnQuietHost
        );
    }

    #[test]
    fn over_budget_on_an_oversubscribed_host_is_not_judged() {
        assert_eq!(
            judge_budget(
                Duration::from_secs(9),
                Duration::from_secs(1),
                CEILING,
                8.1,
                8
            ),
            BudgetVerdict::OverOnLoadedHost,
            "BUG-1730: 10 runnable tasks on 6 cores is why these tests went red"
        );
    }

    /// The point of the ceiling: it is the only verdict that fails a test, so
    /// it must not be excusable. Load could otherwise wave away every budget
    /// on a busy CI runner and the fix would be deleting coverage while
    /// claiming to preserve it.
    #[test]
    fn the_hard_ceiling_is_not_excused_by_any_load() {
        assert_eq!(
            judge_budget(
                Duration::from_secs(45),
                Duration::from_secs(1),
                CEILING,
                500.0,
                2
            ),
            BudgetVerdict::PastHardCeiling
        );
    }

    /// Between the budget and the ceiling the load gate still applies, so a
    /// merely slow host is not reported as a defect.
    #[test]
    fn between_budget_and_ceiling_the_load_gate_still_decides() {
        let between = Duration::from_secs(10);
        let budget = Duration::from_secs(1);
        assert_eq!(
            judge_budget(between, budget, CEILING, 500.0, 2),
            BudgetVerdict::OverOnLoadedHost
        );
        assert_eq!(
            judge_budget(between, budget, CEILING, 0.5, 2),
            BudgetVerdict::OverOnQuietHost
        );
    }

    /// The boundary belongs to the quiet side: a host loaded to exactly its
    /// core count still has no task waiting, so the miss is worth reporting.
    /// Erring the other way would let a genuine slowdown go unmentioned
    /// behind a fully-but-not-over-subscribed host.
    #[test]
    fn a_fully_subscribed_host_is_still_judged() {
        assert_eq!(
            judge_budget(
                Duration::from_secs(9),
                Duration::from_secs(1),
                CEILING,
                8.0,
                8
            ),
            BudgetVerdict::OverOnQuietHost
        );
    }

    /// Platforms with no load average report 0.0 through `sysinfo`. That must
    /// still report the miss rather than silently swallow it.
    #[test]
    fn a_platform_without_a_load_average_still_reports_the_miss() {
        assert_eq!(
            judge_budget(
                Duration::from_secs(9),
                Duration::from_secs(1),
                CEILING,
                0.0,
                1
            ),
            BudgetVerdict::OverOnQuietHost
        );
    }

    /// The invariant this whole module exists for: a missed budget is never
    /// fatal, so no host condition can turn a passing test red. Only the
    /// ceiling, which no load excuses, can.
    #[test]
    fn only_the_ceiling_is_fatal() {
        assert!(BudgetVerdict::PastHardCeiling.is_fatal());
        assert!(!BudgetVerdict::Within.is_fatal());
        assert!(
            !BudgetVerdict::OverOnQuietHost.is_fatal(),
            "a lagging load average cannot prove the host was quiet, so a missed \
             budget on an apparently-quiet host must report, not fail"
        );
        assert!(!BudgetVerdict::OverOnLoadedHost.is_fatal());
    }

    /// Exactly at the budget is over it, and exactly at the ceiling is past it
    /// — the same `<` the assertions this replaces used, kept so no test
    /// silently gains a tick of slack.
    #[test]
    fn the_boundaries_are_inclusive_on_the_failing_side() {
        assert_eq!(
            judge_budget(
                Duration::from_secs(1),
                Duration::from_secs(1),
                CEILING,
                0.0,
                8
            ),
            BudgetVerdict::OverOnQuietHost
        );
        assert_eq!(
            judge_budget(CEILING, Duration::from_secs(1), CEILING, 500.0, 8),
            BudgetVerdict::PastHardCeiling
        );
    }
}
