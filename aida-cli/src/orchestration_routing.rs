//! ADR-7 guardrail: the per-spec orchestration-routing registry.
//!
//! ADR-7 (accepted 2026-06-29): there is ONE per-spec orchestration engine —
//! `auto_complete::orchestrate_with_resume` (implement → CI → review → merge →
//! pull → build). `aida zen`, `aida queue work --auto-complete`, and
//! `aida queue integrate` ALL drive a spec through it, differing only in SCOPE
//! (a from-scratch spec / the queue / an already-Done PR), LIFETIME (one-shot /
//! continuous), START PHASE, and AUTONOMY MODE — never in the per-spec
//! lifecycle itself. The motivating failure (zen slice-1, PR #1231) stopped at
//! implement+PR with ZERO review because it ran an inlined implementer phase
//! instead of reusing the shared engine.
//!
//! ADR-9 (the enforcement decision): a PROSE rule did not hold against a
//! confident agent — so this module makes the invariant a CI gate
//! (substrate-as-bouncer). It is a **registry**: every per-spec driver is
//! classified here, and the tests assert each engine-routed driver actually
//! produces an `--auto-complete` argv (tied to the real `drive_args` helpers,
//! not a restated constant) and that `aida burndown` is the SINGLE,
//! explicitly-named exception.
//!
//! **What this guardrail does and does not catch.** It pins the routing of the
//! *known* drivers — so a regression that drops `--auto-complete` from zen's or
//! integrate's argv (the zen-slice-1 class of bug) trips CI, and changing the
//! registry (e.g. flipping burndown to engine-routed, or adding a second
//! exception) trips the completeness assertions. It is a registry, not an AST
//! scan: a brand-new command that inlines the lifecycle without being added
//! here is not auto-detected. The contract is therefore: **a new per-spec
//! driver MUST be classified in [`routing_table`]** — routing it through the
//! engine, or consciously joining the named-exception list. The architecture
//! doc (`docs/architecture/one-orchestration-engine.md`) states this rule for
//! humans; this registry states it for CI.
//!
//! trace:ADR-7 trace:ADR-9 | ai:claude

/// How a per-spec driver reaches the shared per-spec lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EngineRouting {
    /// This command IS the engine entry point — `aida queue work
    /// --auto-complete` calls `orchestrate_with_resume` in-process.
    DirectEngine,
    /// Self-invokes `aida queue work --auto-complete [--from-pr]` as a
    /// subprocess, re-entering the engine (zen at phase 1, integrate at the
    /// reviewer phase).
    SubprocessAutoComplete,
    /// The SINGLE allow-listed exception: a FLEET-layer orchestrator that fans
    /// out worktree-isolated implementer subagents rather than driving one spec
    /// through the per-spec engine. It sits one level ABOVE the per-spec
    /// lifecycle — its individual specs, if it routed them, would hit the
    /// engine — so it is sanctioned, not non-conforming. (ADR-9)
    FleetException,
}

impl EngineRouting {
    /// True when this driver runs the spec through the shared per-spec engine.
    pub(crate) fn routes_through_engine(self) -> bool {
        matches!(self, Self::DirectEngine | Self::SubprocessAutoComplete)
    }
}

/// One per-spec driver and how it reaches the engine.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PerSpecDriver {
    /// The user-facing command surface (e.g. `aida zen`).
    pub name: &'static str,
    /// How it reaches (or, for the exception, deliberately does not reach) the
    /// shared engine.
    pub routing: EngineRouting,
    /// Why it is classified this way — non-empty by contract, so a new entry
    /// must justify itself.
    pub rationale: &'static str,
}

/// The registry of every per-spec driver. Adding a new command that drives a
/// spec through implement → … → pull MUST add a row here (and is then checked
/// by the tests below). This is the conscious allow-list step ADR-9 requires.
// trace:ADR-7 trace:ADR-9 | ai:claude
pub(crate) fn routing_table() -> Vec<PerSpecDriver> {
    vec![
        PerSpecDriver {
            name: "aida queue work --auto-complete",
            routing: EngineRouting::DirectEngine,
            rationale: "IS the engine: calls auto_complete::orchestrate_with_resume in-process.",
        },
        PerSpecDriver {
            name: "aida zen",
            routing: EngineRouting::SubprocessAutoComplete,
            rationale: "Self-invokes `queue work --auto-complete` (phases 1-6); review + merge \
                        come for free from the engine (TASK-1049).",
        },
        PerSpecDriver {
            name: "aida do",
            routing: EngineRouting::SubprocessAutoComplete,
            rationale: "Self-invokes `queue work --auto-complete=through-ci` (phases 1-2); \
                        stops at the ready-PR checkpoint by design (TASK-1155 / ADR-11).",
        },
        PerSpecDriver {
            name: "aida queue integrate",
            routing: EngineRouting::SubprocessAutoComplete,
            rationale: "Self-invokes `queue work --auto-complete --from-pr`, re-entering the \
                        engine at the reviewer phase (phases 3-6) for an already-Done PR.",
        },
        PerSpecDriver {
            name: "aida burndown",
            routing: EngineRouting::FleetException,
            rationale: "FLEET-layer orchestrator: fans out worktree-isolated implementer \
                        subagents via the harness's native subagent fan-out, one level above the \
                        per-spec lifecycle. The single allow-listed exception (ADR-9).",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The known per-spec drivers — the set a new driver must consciously join.
    /// Kept in sync with [`routing_table`]; a divergence trips
    /// `registry_covers_exactly_the_known_drivers`.
    const KNOWN_DRIVERS: &[&str] = &[
        "aida queue work --auto-complete",
        "aida zen",
        "aida do",
        "aida queue integrate",
        "aida burndown",
    ];

    #[test]
    fn registry_covers_exactly_the_known_drivers() {
        let mut names: Vec<&str> = routing_table().iter().map(|d| d.name).collect();
        names.sort_unstable();
        let mut known: Vec<&str> = KNOWN_DRIVERS.to_vec();
        known.sort_unstable();
        // Adding a new per-spec driver without classifying it here (or removing
        // one) trips this — the conscious allow-list gate ADR-9 requires.
        assert_eq!(
            names, known,
            "a per-spec driver was added/removed without updating the routing registry — \
             classify it (route it through the engine, or consciously add it to the named \
             exception list). See docs/architecture/one-orchestration-engine.md."
        );
    }

    #[test]
    fn every_driver_has_a_rationale() {
        for d in routing_table() {
            assert!(
                !d.rationale.trim().is_empty(),
                "driver {} must justify its routing classification",
                d.name
            );
        }
    }

    #[test]
    fn burndown_is_the_single_allow_listed_exception() {
        let exceptions: Vec<&str> = routing_table()
            .iter()
            .filter(|d| d.routing == EngineRouting::FleetException)
            .map(|d| d.name)
            .collect();
        // Exactly one fleet-layer exception, and it is burndown. A second
        // exception, or any other command opting out of the engine, trips this.
        assert_eq!(
            exceptions,
            vec!["aida burndown"],
            "the one-engine invariant allows exactly ONE named exception (aida burndown's \
             fleet-layer fan-out). Routing another command outside the engine needs an ADR."
        );
    }

    #[test]
    fn all_non_exception_drivers_route_through_the_engine() {
        for d in routing_table() {
            if d.routing == EngineRouting::FleetException {
                continue;
            }
            assert!(
                d.routing.routes_through_engine(),
                "{} must route through the shared per-spec engine",
                d.name
            );
        }
    }

    /// The teeth: tie the registry to the REAL argv helpers. A regression that
    /// drops `--auto-complete` (the zen-slice-1 class of bug — running an
    /// inlined implementer instead of the full engine) trips here.
    #[test]
    fn zen_drive_argv_actually_routes_through_the_engine() {
        let args = crate::zen_drive::drive_args("SPEC-1", None, false, false);
        assert!(
            args.contains(&"--auto-complete".to_string()),
            "aida zen must hand the spec to the --auto-complete engine, not an inlined phase"
        );
    }

    #[test]
    fn integrate_argv_actually_routes_through_the_engine() {
        let args = crate::integrate::drive_args("SPEC-1");
        assert!(
            args.contains(&"--auto-complete".to_string()),
            "aida integrate must hand the spec to the --auto-complete engine, not an inlined merge"
        );
        assert!(
            args.contains(&"--from-pr".to_string()),
            "aida integrate re-enters the engine at the reviewer phase via --from-pr"
        );
    }
}
