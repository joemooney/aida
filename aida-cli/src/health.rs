//! `aida health` — a fast, honest, at-a-glance vital-signs read on whether an
//! AIDA project is healthy, across two axes: the **backlog** (is open work
//! flowing, or is it stale / blocked / aging / paused?) and **coordination**
//! (queue depth, live leases, drains, open findings — is the agent machinery in
//! a good state, or stuck?).
//!
//! ## Design rationale (STORY-658)
//!
//! The bar set by the brief was *honest, not vanity*. Three choices follow from
//! that:
//!
//! 1. **Worst-anchored rollup, not an average.** A single Critical vital drags
//!    the whole verdict to Critical regardless of how green everything else is.
//!    A dashboard that averages a stuck drain against a tidy backlog and reports
//!    "mostly fine" is exactly the vanity read we refuse to ship. One thing
//!    badly wrong = the project is not healthy, and the headline says so.
//!
//! 2. **Every vital carries a remedy, not just a number.** A health read that
//!    tells you "12 stale specs" without "→ aida list --status approved" makes
//!    you do the triage it just claimed to do. Each non-green vital names the
//!    one command that acts on it.
//!
//! 3. **Pure + cheap.** All scoring lives here as pure functions over plain
//!    inputs (no store, no clock, no I/O) so it unit-tests exhaustively and the
//!    command reuses the cache/read paths the caller already has. No LLM.
//!
//! The aggregation/scoring in this module is deliberately I/O-free; the CLI
//! handler gathers the inputs (cache summaries, leases, drain lock, findings)
//! and feeds them in. trace:STORY-658 | ai:claude

/// The grade a single vital — or the project overall — earns. Ordered so
/// `max()` over a set yields the worst (most-severe) grade, which is exactly the
/// worst-anchored rollup. trace:STORY-658 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Grade {
    /// Nothing to act on; the signal is in a good place.
    Healthy,
    /// Worth a glance — drifting, but not yet a problem.
    Watch,
    /// Acting-now territory — stuck, stale, or piling up.
    Critical,
}

impl Grade {
    /// One-word label for human output.
    pub fn label(self) -> &'static str {
        match self {
            Grade::Healthy => "healthy",
            Grade::Watch => "watch",
            Grade::Critical => "critical",
        }
    }

    /// Stable machine token for `--json`.
    pub fn token(self) -> &'static str {
        self.label()
    }
}

/// Which axis a vital belongs to — the two halves of the read.
/// trace:STORY-658 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Backlog state: the requirement graph's flow and hygiene.
    Backlog,
    /// Coordination / agent state: queue, leases, drains, findings.
    Coordination,
}

impl Axis {
    pub fn label(self) -> &'static str {
        match self {
            Axis::Backlog => "backlog",
            Axis::Coordination => "coordination",
        }
    }
}

/// A single measured vital sign: its grade, a short measured-value phrase, the
/// meaning, and (when not healthy) the one command that acts on it.
/// trace:STORY-658 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vital {
    pub axis: Axis,
    /// Stable short key (e.g. `stale_specs`) for `--json`.
    pub key: &'static str,
    /// Human label (e.g. "Stale open work").
    pub label: &'static str,
    pub grade: Grade,
    /// The measured value, already formatted (e.g. "12 specs").
    pub value: String,
    /// One-line meaning of this reading.
    pub detail: String,
    /// The command that acts on this vital when it isn't healthy; `None` when
    /// healthy (nothing to do).
    pub remedy: Option<&'static str>,
}

/// The thresholds that turn raw counts into grades. Held as a struct (not magic
/// numbers scattered through the scorers) so the policy is in one place and the
/// tests pin it. trace:STORY-658 | ai:claude
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    /// Days after which an open spec with no activity is "stale".
    pub stale_days: i64,
    /// Days after which an in-progress spec is "aging" (likely stuck).
    pub aging_inprogress_days: i64,
    /// Approved-but-unstarted count → Watch / Critical.
    pub backlog_watch: usize,
    pub backlog_critical: usize,
    /// Stale open-spec count → Watch / Critical.
    pub stale_watch: usize,
    pub stale_critical: usize,
    /// Blocked-spec count → Watch / Critical.
    pub blocked_watch: usize,
    pub blocked_critical: usize,
    /// Queue depth → Watch / Critical.
    pub queue_watch: usize,
    pub queue_critical: usize,
    /// Open-findings count → Watch / Critical.
    pub findings_watch: usize,
    pub findings_critical: usize,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            stale_days: 14,
            aging_inprogress_days: 7,
            backlog_watch: 15,
            backlog_critical: 40,
            stale_watch: 5,
            stale_critical: 15,
            blocked_watch: 1,
            blocked_critical: 4,
            queue_watch: 8,
            queue_critical: 20,
            findings_watch: 3,
            findings_critical: 10,
        }
    }
}

/// Map a count to a grade given watch/critical step thresholds (inclusive).
/// trace:STORY-658 | ai:claude
fn grade_count(n: usize, watch: usize, critical: usize) -> Grade {
    if n >= critical {
        Grade::Critical
    } else if n >= watch {
        Grade::Watch
    } else {
        Grade::Healthy
    }
}

/// One open spec, reduced to exactly what the backlog scorers need. The handler
/// builds these from cache summaries; keeping the scorer over this tiny struct
/// (not `RequirementSummary`) is what keeps the logic pure and testable.
/// trace:STORY-658 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSpec {
    /// Canonical status token, e.g. `Approved`, `InProgress`, `NeedsAttention`.
    pub status: String,
    /// Whole days since this spec was last modified (>= 0).
    pub idle_days: i64,
    /// True when this spec is blocked by an incomplete spec.
    pub blocked: bool,
    /// True when a live lease holds this spec right now.
    pub in_flight: bool,
}

impl OpenSpec {
    fn is(&self, status: &str) -> bool {
        self.status.eq_ignore_ascii_case(status)
    }
}

/// The coordination-side inputs the handler probes from the runtime substrate.
/// trace:STORY-658 | ai:claude
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoordinationInputs {
    /// Implementer-queue depth for the current user.
    pub queue_depth: usize,
    /// Live (non-stale) session leases.
    pub live_leases: usize,
    /// Leases that exist but failed the liveness probe (orphaned worktrees).
    pub stale_leases: usize,
    /// Open (untriaged) findings awaiting attention.
    pub open_findings: usize,
    /// A drain is running and its PID is alive.
    pub drain_running: bool,
    /// A drain lock exists but its PID is dead (crashed mid-drain).
    pub drain_stale: bool,
    /// Specs parked NeedsAttention (punts / shelved drain failures).
    pub needs_attention: usize,
}

/// Compute the backlog-axis vitals from the open specs + a velocity reading.
/// `net_per_day` is the burn-down net (completed − added) per day over a recent
/// window; `None` when unknown. trace:STORY-658 | ai:claude
pub fn backlog_vitals(specs: &[OpenSpec], net_per_day: Option<f64>, t: &Thresholds) -> Vec<Vital> {
    let approved = specs
        .iter()
        .filter(|s| s.is("Approved") || s.is("Planned"))
        .count();
    let stale = specs
        .iter()
        .filter(|s| s.idle_days >= t.stale_days && !s.in_flight)
        .count();
    let blocked = specs.iter().filter(|s| s.blocked).count();
    let aging = specs
        .iter()
        .filter(|s| s.is("InProgress") && s.idle_days >= t.aging_inprogress_days && !s.in_flight)
        .count();

    let mut vitals = Vec::new();

    vitals.push(Vital {
        axis: Axis::Backlog,
        key: "ready_backlog",
        label: "Ready backlog",
        grade: grade_count(approved, t.backlog_watch, t.backlog_critical),
        value: format!("{approved} approved/planned"),
        detail: "Specs ready to pick up but not yet started".to_string(),
        remedy: (approved >= t.backlog_watch).then_some("aida queue work --auto-complete"),
    });

    vitals.push(Vital {
        axis: Axis::Backlog,
        key: "stale_specs",
        label: "Stale open work",
        grade: grade_count(stale, t.stale_watch, t.stale_critical),
        value: format!("{stale} idle >{}d", t.stale_days),
        detail: "Open specs untouched long enough to be forgotten".to_string(),
        remedy: (stale >= t.stale_watch)
            .then_some("aida list --status approved   # then defer or archive"),
    });

    vitals.push(Vital {
        axis: Axis::Backlog,
        key: "blocked_specs",
        label: "Blocked work",
        grade: grade_count(blocked, t.blocked_watch, t.blocked_critical),
        value: format!("{blocked} blocked"),
        detail: "Specs waiting on an incomplete blocker".to_string(),
        remedy: (blocked >= t.blocked_watch).then_some("aida graph <ID> --blocked-by"),
    });

    vitals.push(Vital {
        axis: Axis::Backlog,
        key: "aging_inprogress",
        label: "Aging in-progress",
        grade: grade_count(aging, 1, 3),
        value: format!("{aging} stalled >{}d", t.aging_inprogress_days),
        detail: "In-progress specs with no live lease and no recent activity".to_string(),
        remedy: (aging >= 1).then_some("aida session leases   # is anyone actually on these?"),
    });

    // Velocity is a direction signal, not a count: shrinking backlog = healthy,
    // treading water = watch, growing faster than it ships = critical.
    let (vel_grade, vel_value) = match net_per_day {
        None => (Grade::Healthy, "no recent activity".to_string()),
        Some(n) if n >= 0.5 => (Grade::Healthy, format!("{n:+.1}/day (shrinking)")),
        Some(n) if n > -0.5 => (Grade::Watch, format!("{n:+.1}/day (treading water)")),
        Some(n) => (Grade::Critical, format!("{n:+.1}/day (growing)")),
    };
    vitals.push(Vital {
        axis: Axis::Backlog,
        key: "burn_velocity",
        label: "Burn-down velocity",
        grade: vel_grade,
        value: vel_value,
        detail: "Net completed minus added per day (recent window)".to_string(),
        remedy: (vel_grade != Grade::Healthy)
            .then_some("aida usage --auto-complete   # is the drain shipping?"),
    });

    vitals
}

/// Compute the coordination-axis vitals from the runtime substrate inputs.
/// trace:STORY-658 | ai:claude
pub fn coordination_vitals(c: &CoordinationInputs, t: &Thresholds) -> Vec<Vital> {
    let mut vitals = Vec::new();

    vitals.push(Vital {
        axis: Axis::Coordination,
        key: "queue_depth",
        label: "Queue depth",
        grade: grade_count(c.queue_depth, t.queue_watch, t.queue_critical),
        value: format!("{} queued", c.queue_depth),
        detail: "Items waiting in the implementer queue".to_string(),
        remedy: (c.queue_depth >= t.queue_watch).then_some("aida queue work --auto-complete"),
    });

    // A stale drain lock is the single worst coordination signal — a crashed
    // drain blocks the next one and leaves work half-done. It is always
    // Critical when present, never softened by anything green.
    let drain_grade = if c.drain_stale {
        Grade::Critical
    } else {
        Grade::Healthy
    };
    vitals.push(Vital {
        axis: Axis::Coordination,
        key: "drain_state",
        label: "Drain state",
        grade: drain_grade,
        value: if c.drain_stale {
            "stale lock (crashed)".to_string()
        } else if c.drain_running {
            "running".to_string()
        } else {
            "idle".to_string()
        },
        detail: "Autonomous burn-down orchestrator state".to_string(),
        remedy: c
            .drain_stale
            .then_some("aida burndown status   # clear the stale lock"),
    });

    // Stale leases = orphaned worktrees holding specs hostage from the next
    // pickup. Any stale lease is Watch; several is Critical.
    vitals.push(Vital {
        axis: Axis::Coordination,
        key: "stale_leases",
        label: "Stale leases",
        grade: grade_count(c.stale_leases, 1, 3),
        value: format!("{} stale / {} live", c.stale_leases, c.live_leases),
        detail: "Leases whose worktree/process is gone but still registered".to_string(),
        remedy: (c.stale_leases >= 1).then_some("aida session leases"),
    });

    vitals.push(Vital {
        axis: Axis::Coordination,
        key: "open_findings",
        label: "Open findings",
        grade: grade_count(c.open_findings, t.findings_watch, t.findings_critical),
        value: format!("{} untriaged", c.open_findings),
        detail: "Deviations / observations awaiting triage".to_string(),
        remedy: (c.open_findings >= t.findings_watch).then_some("aida findings list"),
    });

    // NeedsAttention specs are punts / shelved drain failures — work the
    // machinery explicitly couldn't finish. Any is Watch; a pile is Critical.
    vitals.push(Vital {
        axis: Axis::Coordination,
        key: "needs_attention",
        label: "Parked (needs attention)",
        grade: grade_count(c.needs_attention, 1, 4),
        value: format!("{} parked", c.needs_attention),
        detail: "Specs punted or shelved — a human/advisor must decide".to_string(),
        remedy: (c.needs_attention >= 1).then_some("aida findings list   # triage the punts"),
    });

    vitals
}

/// The overall health read: the rolled-up grade plus every vital.
/// trace:STORY-658 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthReport {
    pub overall: Grade,
    pub vitals: Vec<Vital>,
}

impl HealthReport {
    /// Build the report from both axes' vitals, rolling the overall grade up as
    /// the **worst** vital (worst-anchored — see module docs). An empty vital
    /// set is Healthy (nothing measured = nothing wrong). trace:STORY-658
    pub fn build(mut backlog: Vec<Vital>, coordination: Vec<Vital>) -> HealthReport {
        backlog.extend(coordination);
        let overall = backlog
            .iter()
            .map(|v| v.grade)
            .max()
            .unwrap_or(Grade::Healthy);
        // Severity-order the issue list: worst (Critical) first, healthy last.
        // `sort_by_key` is stable, so within one grade the original per-axis
        // vital order is preserved — the things to act on bubble to the top of
        // each axis section while the green readings sink, without ever hiding
        // them. The grouped human view filters by axis afterward, so each axis
        // section inherits this worst-first order. trace:TASK-853 | ai:claude
        backlog.sort_by_key(|v| std::cmp::Reverse(v.grade));
        HealthReport {
            overall,
            vitals: backlog,
        }
    }

    /// Count of vitals at each grade — drives the one-line summary.
    /// trace:STORY-658 | ai:claude
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut healthy = 0;
        let mut watch = 0;
        let mut critical = 0;
        for v in &self.vitals {
            match v.grade {
                Grade::Healthy => healthy += 1,
                Grade::Watch => watch += 1,
                Grade::Critical => critical += 1,
            }
        }
        (healthy, watch, critical)
    }

    /// The honest one-line headline. Leads with the worst, names the count, so a
    /// once-a-day glance gets the truth in one sentence. trace:STORY-658
    pub fn headline(&self) -> String {
        let (_, watch, critical) = self.counts();
        match self.overall {
            Grade::Critical => format!(
                "CRITICAL — {critical} vital{} need acting on now",
                if critical == 1 { "" } else { "s" }
            ),
            Grade::Watch => format!(
                "WATCH — {watch} vital{} drifting; nothing on fire",
                if watch == 1 { "" } else { "s" }
            ),
            Grade::Healthy => "HEALTHY — every vital in a good place".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(status: &str, idle: i64, blocked: bool, in_flight: bool) -> OpenSpec {
        OpenSpec {
            status: status.to_string(),
            idle_days: idle,
            blocked,
            in_flight,
        }
    }

    #[test]
    fn grade_count_steps() {
        assert_eq!(grade_count(0, 5, 15), Grade::Healthy);
        assert_eq!(grade_count(4, 5, 15), Grade::Healthy);
        assert_eq!(grade_count(5, 5, 15), Grade::Watch);
        assert_eq!(grade_count(14, 5, 15), Grade::Watch);
        assert_eq!(grade_count(15, 5, 15), Grade::Critical);
        assert_eq!(grade_count(99, 5, 15), Grade::Critical);
    }

    #[test]
    fn grade_ordering_is_severity() {
        // max() must yield the worst grade — the rollup depends on this.
        assert!(Grade::Critical > Grade::Watch);
        assert!(Grade::Watch > Grade::Healthy);
        assert_eq!(
            [Grade::Healthy, Grade::Critical, Grade::Watch]
                .into_iter()
                .max()
                .unwrap(),
            Grade::Critical
        );
    }

    #[test]
    fn empty_project_is_healthy() {
        let r = HealthReport::build(
            backlog_vitals(&[], None, &Thresholds::default()),
            coordination_vitals(&CoordinationInputs::default(), &Thresholds::default()),
        );
        assert_eq!(r.overall, Grade::Healthy);
        assert!(r.headline().starts_with("HEALTHY"));
    }

    #[test]
    fn stale_pile_drives_backlog_critical() {
        let specs: Vec<OpenSpec> = (0..20)
            .map(|_| spec("Approved", 30, false, false))
            .collect();
        let vitals = backlog_vitals(&specs, Some(1.0), &Thresholds::default());
        let stale = vitals.iter().find(|v| v.key == "stale_specs").unwrap();
        assert_eq!(stale.grade, Grade::Critical);
        assert!(stale.remedy.is_some());
    }

    #[test]
    fn in_flight_specs_are_not_counted_stale() {
        // A spec idle 30d but held by a live lease is being worked, not stale.
        let specs = vec![spec("InProgress", 30, false, true)];
        let vitals = backlog_vitals(&specs, Some(1.0), &Thresholds::default());
        let stale = vitals.iter().find(|v| v.key == "stale_specs").unwrap();
        assert_eq!(stale.value, "0 idle >14d");
        assert_eq!(stale.grade, Grade::Healthy);
        let aging = vitals.iter().find(|v| v.key == "aging_inprogress").unwrap();
        assert_eq!(aging.grade, Grade::Healthy);
    }

    #[test]
    fn blocked_thresholds() {
        let one = vec![spec("Approved", 1, true, false)];
        let v = backlog_vitals(&one, Some(1.0), &Thresholds::default());
        assert_eq!(
            v.iter().find(|x| x.key == "blocked_specs").unwrap().grade,
            Grade::Watch
        );
        let many: Vec<OpenSpec> = (0..5).map(|_| spec("Approved", 1, true, false)).collect();
        let v = backlog_vitals(&many, Some(1.0), &Thresholds::default());
        assert_eq!(
            v.iter().find(|x| x.key == "blocked_specs").unwrap().grade,
            Grade::Critical
        );
    }

    #[test]
    fn velocity_direction_grades() {
        let g = |n: Option<f64>| {
            backlog_vitals(&[], n, &Thresholds::default())
                .into_iter()
                .find(|v| v.key == "burn_velocity")
                .unwrap()
                .grade
        };
        assert_eq!(g(Some(2.0)), Grade::Healthy);
        assert_eq!(g(Some(0.0)), Grade::Watch);
        assert_eq!(g(Some(-3.0)), Grade::Critical);
        assert_eq!(g(None), Grade::Healthy);
    }

    #[test]
    fn stale_drain_lock_is_always_critical() {
        let c = CoordinationInputs {
            drain_stale: true,
            ..Default::default()
        };
        let v = coordination_vitals(&c, &Thresholds::default());
        let drain = v.iter().find(|x| x.key == "drain_state").unwrap();
        assert_eq!(drain.grade, Grade::Critical);
        assert!(drain.value.contains("crashed"));
    }

    #[test]
    fn running_drain_is_healthy() {
        let c = CoordinationInputs {
            drain_running: true,
            ..Default::default()
        };
        let v = coordination_vitals(&c, &Thresholds::default());
        let drain = v.iter().find(|x| x.key == "drain_state").unwrap();
        assert_eq!(drain.grade, Grade::Healthy);
        assert_eq!(drain.value, "running");
    }

    #[test]
    fn one_critical_anchors_overall_critical() {
        // Everything green except a stale drain lock → overall Critical.
        let backlog = backlog_vitals(&[], Some(5.0), &Thresholds::default());
        let coord = coordination_vitals(
            &CoordinationInputs {
                drain_stale: true,
                ..Default::default()
            },
            &Thresholds::default(),
        );
        let r = HealthReport::build(backlog, coord);
        assert_eq!(r.overall, Grade::Critical);
        assert!(r.headline().starts_with("CRITICAL"));
        // And the green vitals are still present (we don't hide them).
        assert!(r.vitals.iter().any(|v| v.grade == Grade::Healthy));
    }

    #[test]
    fn watch_does_not_escalate_to_critical() {
        let specs: Vec<OpenSpec> = (0..6).map(|_| spec("Approved", 30, false, false)).collect();
        let r = HealthReport::build(
            backlog_vitals(&specs, Some(1.0), &Thresholds::default()),
            coordination_vitals(&CoordinationInputs::default(), &Thresholds::default()),
        );
        assert_eq!(r.overall, Grade::Watch);
        assert!(r.headline().starts_with("WATCH"));
    }

    #[test]
    fn counts_sum_to_total() {
        let specs: Vec<OpenSpec> = (0..20).map(|_| spec("Approved", 30, true, false)).collect();
        let r = HealthReport::build(
            backlog_vitals(&specs, Some(-5.0), &Thresholds::default()),
            coordination_vitals(
                &CoordinationInputs {
                    drain_stale: true,
                    open_findings: 12,
                    ..Default::default()
                },
                &Thresholds::default(),
            ),
        );
        let (h, w, c) = r.counts();
        assert_eq!(h + w + c, r.vitals.len());
        assert!(c >= 3); // stale, blocked, velocity, drain, findings all bad
    }

    #[test]
    fn needs_attention_pile_is_critical() {
        let c = CoordinationInputs {
            needs_attention: 5,
            ..Default::default()
        };
        let v = coordination_vitals(&c, &Thresholds::default());
        assert_eq!(
            v.iter().find(|x| x.key == "needs_attention").unwrap().grade,
            Grade::Critical
        );
    }

    #[test]
    fn vitals_are_severity_ordered_worst_first() {
        // A spread of grades across both axes: stale/blocked piles (Critical),
        // a moderate ready backlog (Watch), and several green readings.
        let specs: Vec<OpenSpec> = (0..20).map(|_| spec("Approved", 30, true, false)).collect();
        let r = HealthReport::build(
            backlog_vitals(&specs, Some(1.0), &Thresholds::default()),
            coordination_vitals(
                &CoordinationInputs {
                    drain_stale: true,
                    ..Default::default()
                },
                &Thresholds::default(),
            ),
        );
        // The whole vital list must be non-increasing in severity (worst first).
        let grades: Vec<Grade> = r.vitals.iter().map(|v| v.grade).collect();
        let mut expected = grades.clone();
        expected.sort_by_key(|g| std::cmp::Reverse(*g));
        assert_eq!(grades, expected, "vitals must be ordered worst-first");
        // The lead vital is the worst, and it matches the rolled-up overall.
        assert_eq!(r.vitals.first().unwrap().grade, r.overall);
        // Stable within a grade: among the Critical backlog vitals the original
        // definition order survives — stale_specs precedes blocked_specs.
        let crit_keys: Vec<&str> = r
            .vitals
            .iter()
            .filter(|v| v.axis == Axis::Backlog && v.grade == Grade::Critical)
            .map(|v| v.key)
            .collect();
        assert_eq!(crit_keys, vec!["stale_specs", "blocked_specs"]);
    }

    #[test]
    fn healthy_vitals_have_no_remedy() {
        let v = coordination_vitals(&CoordinationInputs::default(), &Thresholds::default());
        for vital in v.iter().filter(|x| x.grade == Grade::Healthy) {
            assert!(
                vital.remedy.is_none(),
                "healthy vital {} should have no remedy",
                vital.key
            );
        }
    }
}
