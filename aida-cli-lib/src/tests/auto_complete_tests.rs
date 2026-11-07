// trace:TASK-3424 | ai:codex
    use super::*;

    // TASK-1230: the recovery-path worktree-teardown decision. The safety heart
    // is this pure predicate; the discovery + git removal in the real driver is
    // best-effort I/O on top of it.

    #[test]
    fn recovery_teardown_removes_a_clean_pushed_worktree() {
        // Clean, unlocked, and every local commit is on the pushed PR branch
        // (count 0) — nothing is at risk, so the failed attempt's tree goes.
        assert_eq!(
            recovery_worktree_teardown_decision(false, false, 0),
            RecoveryTeardown::Remove
        );
    }

    #[test]
    fn recovery_teardown_keeps_a_dirty_worktree() {
        let v = recovery_worktree_teardown_decision(true, false, 0);
        assert!(matches!(v, RecoveryTeardown::Keep(_)), "dirty must be kept");
        if let RecoveryTeardown::Keep(reason) = v {
            assert!(
                reason.contains("uncommitted"),
                "reason names the dirt: {reason}"
            );
        }
    }

    #[test]
    fn recovery_teardown_keeps_a_locked_worktree() {
        // Dirty takes precedence in ordering, but a clean+locked tree is kept.
        let v = recovery_worktree_teardown_decision(false, true, 0);
        assert!(
            matches!(v, RecoveryTeardown::Keep(_)),
            "locked must be kept"
        );
        if let RecoveryTeardown::Keep(reason) = v {
            assert!(reason.contains("locked"), "reason names the lock: {reason}");
        }
    }

    #[test]
    fn recovery_teardown_keeps_unpushed_local_commits() {
        // The divergence from the merged-based gc gate: the PR is UNMERGED by
        // design, so we do not require a merge signal — but a commit that never
        // made it onto the pushed PR branch is genuinely at risk and is kept.
        let v = recovery_worktree_teardown_decision(false, false, 2);
        assert!(
            matches!(v, RecoveryTeardown::Keep(_)),
            "unpushed local commits must be kept"
        );
        if let RecoveryTeardown::Keep(reason) = v {
            assert!(
                reason.contains("not on the pushed PR branch"),
                "reason: {reason}"
            );
        }
    }

    /// The kickoff banner must keep advertising the out-of-band live view
    /// (`aida drain status`) so a drive is discoverable.
    // trace:STORY-726 | ai:claude
    #[test]
    fn kickoff_banner_names_drain_status() {
        let hint = watch_live_hint();
        assert!(
            hint.contains("aida drain status"),
            "kickoff banner hint should name the live-view command, got: {hint}"
        );
        assert!(
            hint.contains("watch live"),
            "hint should read as a live-view invite"
        );
    }

    /// STORY-265 slice 3: with `--with-plan`, the prelude is a plan session
    /// THEN a promote — ordered, before the drain. Fully isolated: pure
    /// function, no driver, no filesystem. trace:STORY-265
    #[test]
    fn with_plan_prelude_is_plan_session_then_promote() {
        let steps = plan_prelude_steps("STORY-265", true, false);
        assert_eq!(
            steps,
            vec![
                PlanPreludeStep::PlanSession {
                    spec: "STORY-265".to_string(),
                    headless: false,
                },
                PlanPreludeStep::Promote {
                    spec: "STORY-265".to_string(),
                },
            ],
            "with-plan ⇒ plan session then promote, in that order"
        );
    }

    /// STORY-265 slice 3: without `--with-plan`, there is NO prelude — the
    /// drain runs alone, exactly the historical default (unchanged per the
    /// operator's slice-1 opt-in decision). trace:STORY-265
    #[test]
    fn without_with_plan_prelude_is_empty_drain_only() {
        assert!(
            plan_prelude_steps("STORY-265", false, false).is_empty(),
            "no --with-plan ⇒ drain only, no prelude steps"
        );
        // The headless flag is irrelevant when with_plan is off.
        assert!(plan_prelude_steps("STORY-265", false, true).is_empty());
    }

    /// STORY-265 slice 3: a headless drain (`--no-human=both`) runs the plan
    /// session headless too, mirroring how the phase-1 implementer launches.
    /// trace:STORY-265
    #[test]
    fn with_plan_prelude_propagates_headless_to_plan_session() {
        let steps = plan_prelude_steps("STORY-265", true, true);
        assert_eq!(
            steps[0],
            PlanPreludeStep::PlanSession {
                spec: "STORY-265".to_string(),
                headless: true,
            },
            "headless implementer ⇒ headless plan session"
        );
        // The promote step is plain — it is a status edit, never headless.
        assert_eq!(
            steps[1],
            PlanPreludeStep::Promote {
                spec: "STORY-265".to_string(),
            }
        );
    }

    /// BUG-455: a "database is locked" message is recognised (case-insensitive,
    /// both SQLite lock spellings) and nothing else trips the classifier.
    /// Pure-function test — no driver, no filesystem. trace:BUG-455
    #[test]
    fn database_locked_message_is_recognised() {
        assert!(is_database_locked_message(
            "database is locked while trying to shelve"
        ));
        assert!(is_database_locked_message("DATABASE IS LOCKED")); // case-insensitive
        assert!(is_database_locked_message(
            "Error: the database table is locked: requirements"
        ));
        // Unrelated failures must NOT be misread as a cache lock.
        assert!(!is_database_locked_message("CI run failed (red)"));
        assert!(!is_database_locked_message("no PR was opened"));
        assert!(!is_database_locked_message(""));
    }

    /// BUG-657: an environmental fault (disk full / OOM) is recognised so the
    /// drive suppresses the phantom auto-draft — but unrelated failures (and the
    /// `oom`-in-`groom` false-positive trap) must NOT match. Pure function,
    /// case-insensitive.
    // trace:BUG-657 | ai:claude
    #[test]
    fn environmental_failure_is_recognised_without_false_positives() {
        assert!(is_environmental_failure(
            "write failed: No space left on device"
        ));
        assert!(is_environmental_failure("ENOSPC writing object"));
        assert!(is_environmental_failure("disk full"));
        assert!(is_environmental_failure("process killed: Out Of Memory"));
        assert!(is_environmental_failure("cgroup oom-kill invoked"));
        assert!(is_environmental_failure("cannot allocate memory"));
        // Real spec/orchestrator failures must NOT be suppressed…
        assert!(!is_environmental_failure("CI run failed (red)"));
        assert!(!is_environmental_failure("no PR was opened"));
        // …and the bare-`oom` substring trap (groom / room) must NOT match.
        assert!(!is_environmental_failure(
            "aida groom failed in the reviewer room"
        ));
        assert!(!is_environmental_failure(""));
    }

    /// BUG-455: a transient cache-lock failure is reclassified to the shelvable
    /// `CacheLocked` kind, so a batch drain parks the spec and continues instead
    /// of hard-stopping; an unrelated failure keeps its kind. Idempotent.
    /// Pure-function test — fully isolated. trace:BUG-455
    #[test]
    fn cache_lock_failure_reclassifies_to_shelvable() {
        // A reviewer phase that surfaced a cache lock comes in with the default
        // `Failed` kind (or could even be `Internal`); either way it upgrades.
        let f = PhaseFailure::of(
            FailureKind::Internal,
            "database is locked while trying to shelve SPEC into Needs Attention",
        )
        .reclassify_transient();
        assert_eq!(f.kind, FailureKind::CacheLocked);
        assert!(
            f.kind.is_shelvable(),
            "a cache-lock failure must be shelvable so the drain continues"
        );

        // Idempotent: re-running leaves an already-CacheLocked failure alone.
        let again = f.reclassify_transient();
        assert_eq!(again.kind, FailureKind::CacheLocked);

        // An unrelated failure is untouched.
        let unrelated =
            PhaseFailure::of(FailureKind::CiRed, "CI run failed (red)").reclassify_transient();
        assert_eq!(unrelated.kind, FailureKind::CiRed);

        // A non-shelvable env failure with no lock text stays non-shelvable.
        let spawn = PhaseFailure::of(FailureKind::Spawn, "spawn ENOENT: claude not on PATH")
            .reclassify_transient();
        assert_eq!(spawn.kind, FailureKind::Spawn);
        assert!(!spawn.kind.is_shelvable());
    }

    /// BUG-420: the watchdog trips on no-progress first, ceiling as backstop;
    /// 0 disables a check. trace:BUG-420
    #[test]
    fn watchdog_verdict_no_progress_then_ceiling() {
        use std::time::Duration;
        let (np, ceil) = (Duration::from_secs(600), Duration::from_secs(2700));
        // No progress for >= limit → NoProgress.
        assert_eq!(
            watchdog_verdict(Duration::from_secs(600), Duration::from_secs(100), np, ceil),
            Some(WatchdogTrip::NoProgress)
        );
        // Progress recent but total >= ceiling → Ceiling.
        assert_eq!(
            watchdog_verdict(Duration::from_secs(10), Duration::from_secs(2700), np, ceil),
            Some(WatchdogTrip::Ceiling)
        );
        // Both within limits → None (keep running).
        assert_eq!(
            watchdog_verdict(Duration::from_secs(60), Duration::from_secs(120), np, ceil),
            None
        );
        // No-progress takes precedence when both would trip.
        assert_eq!(
            watchdog_verdict(
                Duration::from_secs(600),
                Duration::from_secs(2700),
                np,
                ceil
            ),
            Some(WatchdogTrip::NoProgress)
        );
        // Zero limits disable both checks.
        assert_eq!(
            watchdog_verdict(
                Duration::from_secs(99_999),
                Duration::from_secs(99_999),
                Duration::ZERO,
                Duration::ZERO
            ),
            None
        );
    }

    /// BUG-749: CI-waiting is progress for the no-progress watchdog, but the
    /// wall-clock ceiling remains a hard cap.
    // trace:BUG-749 | ai:codex
    #[test]
    fn watchdog_verdict_ci_wait_suppresses_no_progress_only() {
        use std::time::Duration;
        let (np, ceil) = (Duration::from_secs(600), Duration::from_secs(2700));

        assert_eq!(
            watchdog_verdict_with_ci_wait(
                Duration::from_secs(600),
                Duration::from_secs(1200),
                np,
                ceil,
                true,
            ),
            None,
            "a live pr-ship CI wait must not trip the no-progress watchdog",
        );
        assert_eq!(
            watchdog_verdict_with_ci_wait(
                Duration::from_secs(600),
                Duration::from_secs(2700),
                np,
                ceil,
                true,
            ),
            Some(WatchdogTrip::Ceiling),
            "CI wait does not disable the phase ceiling",
        );
        assert_eq!(
            watchdog_verdict_with_ci_wait(
                Duration::from_secs(600),
                Duration::from_secs(1200),
                np,
                ceil,
                false,
            ),
            Some(WatchdogTrip::NoProgress),
            "a genuinely idle session still trips no-progress",
        );
    }

    /// BUG-1299: the whole point of `watchdog_trip_report` — a NoProgress
    /// trip and a Ceiling trip must produce DIFFERENT machine-parseable
    /// detail tags AND differently-worded hints, and — critically — neither
    /// hint may contain the other trip's characteristic phrase. Before the
    /// fix, every watchdog hint was the hardcoded NoProgress text regardless
    /// of which trip fired, so a Ceiling detail shipped next to a
    /// "likely a degenerate spin" hint (the TASK-1274 observation).
    // trace:BUG-1299 | ai:claude
    #[test]
    fn watchdog_trip_report_distinguishes_ceiling_from_no_progress() {
        use std::time::Duration;
        let no_progress = Duration::from_secs(600);
        let ceiling = Duration::from_secs(2700);

        let (np_detail, np_hint) = watchdog_trip_report(
            &WatchdogTrip::NoProgress,
            no_progress,
            ceiling,
            false,
            false,
        );
        let (ceil_detail, ceil_hint) =
            watchdog_trip_report(&WatchdogTrip::Ceiling, no_progress, ceiling, false, false);

        // Details carry a machine-parseable `trip-name: limit` tag naming
        // WHICH limit was crossed (acceptance criterion 3).
        assert!(
            np_detail.starts_with("no-progress:") && np_detail.contains("600s"),
            "{np_detail}"
        );
        assert!(
            ceil_detail.starts_with("ceiling:") && ceil_detail.contains("2700s"),
            "{ceil_detail}"
        );
        assert_ne!(np_detail, ceil_detail);

        // The two hints must disagree in kind, not just in wording: a
        // NoProgress hint says the session stalled (go find out why); a
        // Ceiling hint explicitly says NOT a stall, don't assume a spin.
        assert!(
            np_hint.to_lowercase().contains("degenerate spin"),
            "{np_hint}"
        );
        assert!(np_hint.to_lowercase().contains("stopped"), "{np_hint}");
        assert!(
            ceil_hint.to_lowercase().contains("wall-clock ceiling"),
            "{ceil_hint}"
        );
        assert!(
            ceil_hint.to_lowercase().contains("raise the")
                || ceil_hint.to_lowercase().contains("split the spec"),
            "{ceil_hint}"
        );

        // Neither hint may emit the other trip's characteristic phrase — this
        // is the exact contradiction BUG-1299 reported (a ceiling detail next
        // to a hint that both describes NoProgress AND calls it a spin).
        assert!(
            !ceil_hint.to_lowercase().contains("degenerate spin"),
            "ceiling hint must not describe a spin: {ceil_hint}"
        );
        assert!(
            !np_hint.to_lowercase().contains("wall-clock ceiling"),
            "no-progress hint must not mention the ceiling: {np_hint}"
        );
        assert_ne!(np_hint, ceil_hint);

        // The Spinning trip is a third, distinguishable case too.
        let (spin_detail, spin_hint) = watchdog_trip_report(
            &WatchdogTrip::Spinning {
                template: "...thinking...".to_string(),
                count: 40,
            },
            no_progress,
            ceiling,
            false,
            false,
        );
        assert!(spin_detail.starts_with("spinning:"), "{spin_detail}");
        assert!(!spin_hint.to_lowercase().contains("wall-clock ceiling"));
        assert_ne!(spin_detail, np_detail);
        assert_ne!(spin_detail, ceil_detail);
    }

    /// BUG-1299: `failure_with_retry_attempt` rebuilds a fresh `PhaseFailure`
    /// (to append the `(attempt N/max)` suffix to `reason`) — it must carry
    /// `hint_override` forward rather than silently reset it to `None`. This
    /// is the exact mechanism the TASK-1274 contradiction traced back to: a
    /// watchdog failure's Ceiling-specific hint was resolved once at trip
    /// time, then discarded right here on the way to the eventual shelve, so
    /// the generic (NoProgress-shaped) hint won by default.
    // trace:BUG-1299 | ai:claude
    #[test]
    fn failure_with_retry_attempt_preserves_hint_override() {
        let (detail, hint) = watchdog_trip_report(
            &WatchdogTrip::Ceiling,
            std::time::Duration::from_secs(600),
            std::time::Duration::from_secs(2700),
            false,
            false,
        );
        let original = PhaseFailure::of(
            FailureKind::Watchdog,
            format!("the implementer phase watchdog stopped the session — {detail}"),
        )
        .with_hint_override(hint.clone());

        let driver = MockPhaseDriver::base();
        // transient_retry_budget defaults to 0 in `base()`, but
        // `failure_with_retry_attempt` only reads it when `retries_used > 0`.
        let updated = failure_with_retry_attempt(&driver, &original, 1);

        assert!(
            updated.reason.contains("(attempt 2/1)") || updated.reason.contains("attempt 2/"),
            "{}",
            updated.reason
        );
        assert!(
            updated.reason.contains("ceiling:"),
            "attempt-suffixed reason must still carry the trip tag: {}",
            updated.reason
        );
        assert_eq!(
            updated.hint_override,
            Some(hint),
            "hint_override must survive the attempt-count rewrap"
        );
    }

    /// TASK-136: backoff schedule is 30s/1m/5m, then 15m for any beyond. trace:TASK-136
    #[test]
    fn gh_verify_backoff_schedule_steps() {
        use std::time::Duration;
        assert_eq!(
            gh_verify_backoff_schedule(3),
            vec![
                Duration::from_secs(30),
                Duration::from_secs(60),
                Duration::from_secs(300)
            ]
        );
        assert_eq!(
            gh_verify_backoff_schedule(5).last().copied(),
            Some(Duration::from_secs(900))
        );
        assert!(gh_verify_backoff_schedule(0).is_empty());
    }

    /// Mock driver: records the phases invoked, optionally fails at one, and
    /// returns a configurable verdict. Stands in for the real Claude / CI /
    /// gh / cargo machinery so the orchestrator's sequencing is tested
    /// end-to-end without spawning anything.
    struct MockPhaseDriver {
        calls: Vec<Phase>,
        fail_at: Option<Phase>,
        /// BUG-1524: the [`FailureKind`] `record` uses when `fail_at`
        /// matches — `None` (default) keeps the pre-existing generic
        /// `FailureKind::Failed` mock failure. Set via `failing_at_with_kind`
        /// to drive a specific typed failure (e.g. `LaunchRefused`) through
        /// the orchestrator without inventing a bespoke mock method per kind.
        fail_kind: Option<FailureKind>,
        handoff_fails: bool,
        verdict: Verdict,
        /// BUG-241: what [`PhaseDriver::reconcile_failure`] returns. Defaults
        /// to `GenuineFailure` so every pre-BUG-241 failure test is unchanged.
        reconcile: PhaseReconcile,
        /// STORY-276: when `Some`, `run_implementer` returns
        /// [`ImplementerOutcome::Punted`] with this reason instead of opening
        /// a PR — the headless-implementer design-fork punt.
        punt: Option<String>,
        /// BUG-245: what [`PhaseDriver::shipped_spec_id`] returns. `None`
        /// (default) preserves the pre-BUG-245 behaviour — the dispatched id
        /// is credited.
        shipped_spec_id: Option<String>,
        /// STORY-306: when `Some`, `run_reviewer` returns
        /// [`ReviewerOutcome::EscalatedToHuman`] with this reason instead of a
        /// verdict — the reviewer escalated the merge decision.
        reviewer_escalates: Option<String>,
        /// STORY-306: what `run_advisor` returns; `None` ⇒ the trait default
        /// (an `Internal` error). Set by `advisor_resolves` /
        /// `advisor_escalates`.
        advisor: Option<AdvisorOutcome>,
        /// STORY-306: how many times `run_advisor` was called — a re-punt
        /// after a resume must not spawn a second advisor.
        advisor_calls: usize,
        /// STORY-306: what `resume_implementer` returns; `None` ⇒ the trait
        /// default error. Set by `resume_opens_pr` / `resume_repunts`.
        resume: Option<ImplementerOutcome>,
        /// BUG-257: when `Some`, `run_implementer` returns
        /// [`ImplementerOutcome::Inconclusive`] with this reason — the
        /// orchestrator's phase-1 PR lookup hit a transient GH-API blip and
        /// cannot tell whether a PR was opened. The drain pauses.
        inconclusive: Option<String>,
        /// BUG-266/BUG-1522: the `retry_hint` carried alongside `inconclusive`
        /// on the mocked `ImplementerOutcome::Inconclusive`. `None` (default)
        /// matches the pre-existing mock behaviour (BUG-257's plain GH-blip
        /// shape); `Some` simulates the Anthropic-API `--resume` hint so a
        /// test can assert it survives the BUG-1522 rework-no-op guard.
        inconclusive_retry_hint: Option<String>,
        /// BUG-250: when set, `run_implementer` returns
        /// [`ImplementerOutcome::Held`] — the implementer deliberately held the
        /// PR (branch pushed, PR withheld for a manual gate). The drain reports
        /// a clean `Held` outcome, not a phase-1 failure.
        held: Option<String>,
        /// TASK-358: how many times `mark_implementer_lease_escalated` was
        /// called. The `--escalate-blocks` path stamps it once before
        /// `finish_escalated`; the `--escalate-defaults` resume path and
        /// every non-escalation flow must leave it at zero so an advisor
        /// resume's worktree is preserved.
        mark_escalated_calls: usize,
        /// TASK-136: when `true`, `shelve_on_failure` succeeds and returns a
        /// `FailureReason` the way a real store-backed driver does, so a
        /// batch-mode inconclusive can route through the EPIC-28 shelve→advance
        /// path. `false` (default) keeps the trait default `Ok(None)`.
        shelve_succeeds: bool,
        /// BUG-1291: successful shelves must transfer an open PR to review.
        handoff_calls: Vec<(String, Phase, String)>,
        /// BUG-657: when `Some`, `terminal_status` reports the target spec as
        /// already terminal (`Completed`/`Rejected`) so the orchestrator
        /// finishes as a clean NO-OP without ever calling `run_implementer`.
        /// `None` (default) ⇒ the spec is drivable.
        // trace:BUG-657 | ai:claude
        terminal: Option<&'static str>,
        /// TASK-975: what `ci_fix_budget` returns — the `[drain] ci_auto_fix`
        /// budget. `0` (default) keeps the auto-fix loop off.
        ci_fix_budget: usize,
        /// TASK-975: how many times `finish_ci` fails with a red-CI
        /// `PhaseFailure` before going green (decrements per call).
        ci_red_failures: usize,
        /// BUG-1460: reproduce phase 2 reporting PrNoChecks for one head,
        /// followed by another push before the reviewer handoff.
        ci_no_checks_head_advance: bool,
        /// Observable CI/head events in the BUG-1460 ordering fixture.
        ci_ordering_events: Vec<&'static str>,
        /// TASK-975: what `attempt_ci_fix` returns — did the fix session
        /// push a change?
        ci_fix_pushes: bool,
        /// TASK-975: every `(attempt, budget)` pair `attempt_ci_fix` was
        /// called with, so the tests pin the bounded-loop accounting.
        ci_fix_calls: Vec<(usize, usize)>,
        // TASK-1249: how many times the advisory harvest step ran.
        harvest_gate_calls: usize,
        /// TASK-975: how many times `merge` fails with a conflict-shaped
        /// `PhaseFailure` before succeeding (decrements per call).
        merge_conflicts: usize,
        /// TASK-975: what `attempt_merge_conflict_rebase` returns — did the
        /// in-drain rebase land cleanly?
        conflict_rebase_ok: bool,
        /// TASK-975: how many times `attempt_merge_conflict_rebase` was called.
        conflict_rebase_calls: usize,
        /// BUG-1447: forge-side branch-policy refusals before merge succeeds.
        merge_policy_refusals: usize,
        /// TASK-1448: the merge phase's approval-covers-head gate refuses.
        merge_stale_approval: bool,
        /// BUG-727: when `Some`, `merge_supervision_hold` reports the spec's
        /// execution mode as holding the merge — the substrate supervised-merge
        /// gate fires before phase 4. `None` (default) keeps every
        /// pre-BUG-727 flow unchanged.
        merge_hold: Option<String>,
        /// BUG-1168: reasons persisted before a supervised clean-exit. This
        /// stands in for the real driver's `.aida/merge-holds/PR-N` write.
        // trace:BUG-1168 | ai:codex
        recorded_merge_holds: Vec<String>,
        /// BUG-879: PR resolved by the phase-1/2 handoff. `None` or `Some(0)`
        /// must fail before `run_reviewer` is called.
        // trace:BUG-879 | ai:codex
        pr_number: Option<u32>,
        /// STORY-975: mock reviewer failures that classify as watchdog before
        /// succeeding.
        reviewer_watchdog_failures: usize,
        /// BUG-1299: the exact `PhaseFailure` (reason + optional
        /// `hint_override`) returned while `reviewer_watchdog_failures > 0`,
        /// so a test can drive a specific `WatchdogTrip`'s resolved report
        /// through retry-exhaustion and the eventual shelve. Falls back to a
        /// generic watchdog message when unset.
        // trace:BUG-1299 | ai:claude
        reviewer_watchdog_failure: Option<PhaseFailure>,
        /// BUG-906: mock reviewer launch failures caused by a dirty dead
        /// predecessor lease. They classify as cache-locked so the retry gate
        /// parks with a typed cause after the budget is spent.
        // trace:BUG-906 | ai:codex
        reviewer_cache_locked_failures: usize,
        /// STORY-975: whole-phase transient retry budget returned by the mock.
        transient_retry_budget: usize,
        /// STORY-975: retry records captured by `record_transient_retry`.
        /// BUG-1299: the 5th element is the `detail` the real driver now
        /// threads through from `PhaseFailure::reason`.
        transient_retry_events: Vec<(Phase, String, u32, u32, Option<String>)>,
        /// BUG-895: phase-3 missing-PR recovery result.
        // trace:BUG-895 | ai:codex
        recover_review_pr: Option<u32>,
        /// BUG-1145: phase-1 failure redeemed by an already-open PR; the
        /// orchestrator must continue from this PR-only re-entry phase instead
        /// of retrying phase 1 or shelving.
        phase1_pr_recovery: Option<Phase>,
        /// BUG-1213: successful phase 1 that nevertheless left the rework PR
        /// at its prior head.
        rework_no_op: bool,
        /// BUG-709: `run_implementer` returns
        /// [`ImplementerOutcome::AlreadyMerged`] with this PR number instead
        /// of the default `PrOpened`.
        // trace:BUG-1522 | ai:claude
        already_merged: Option<u32>,
        /// BUG-1244: selected phase-1 worktree/branch, when a test needs to
        /// exercise the cross-spec isolation gate.
        workspace: Option<(String, String)>,
        workspace_error: Option<String>,
        /// BUG-1629: how many times phase 1 asked for the workspace, and the
        /// workspace it pinned for the launch.
        // trace:BUG-1629 | ai:claude
        workspace_resolutions: std::cell::Cell<usize>,
        pinned_workspace: Option<(String, String)>,
        /// BUG-1244: run-local phase-1 PR marker; `suppress_phase_done_pr`
        /// models the recurrence where branch lookup found a sibling PR but
        /// this run never emitted PhaseDonePr.
        phase_done_pr: Option<u32>,
        suppress_phase_done_pr: bool,
    }

    impl MockPhaseDriver {
        /// The all-defaults base — every phase green, no punt / escalate /
        /// advisor. The named constructors tweak it with struct-update syntax,
        /// so a new field is added in exactly one place.
        fn base() -> Self {
            Self {
                calls: Vec::new(),
                fail_at: None,
                fail_kind: None,
                handoff_fails: false,
                verdict: Verdict::Approved,
                reconcile: PhaseReconcile::GenuineFailure,
                punt: None,
                shipped_spec_id: None,
                reviewer_escalates: None,
                advisor: None,
                advisor_calls: 0,
                resume: None,
                inconclusive: None,
                inconclusive_retry_hint: None,
                held: None,
                mark_escalated_calls: 0,
                shelve_succeeds: false,
                handoff_calls: Vec::new(),
                terminal: None,
                ci_fix_budget: 0,
                ci_red_failures: 0,
                ci_no_checks_head_advance: false,
                ci_ordering_events: Vec::new(),
                ci_fix_pushes: false,
                ci_fix_calls: Vec::new(),
                merge_conflicts: 0,
                conflict_rebase_ok: false,
                conflict_rebase_calls: 0,
                merge_policy_refusals: 0,
                merge_stale_approval: false,
                merge_hold: None,
                recorded_merge_holds: Vec::new(),
                pr_number: Some(46),
                reviewer_watchdog_failures: 0,
                reviewer_watchdog_failure: None,
                reviewer_cache_locked_failures: 0,
                transient_retry_budget: 0,
                transient_retry_events: Vec::new(),
                recover_review_pr: None,
                phase1_pr_recovery: None,
                harvest_gate_calls: 0,
                rework_no_op: false,
                already_merged: None,
                workspace: None,
                workspace_error: None,
                workspace_resolutions: std::cell::Cell::new(0),
                pinned_workspace: None,
                phase_done_pr: None,
                suppress_phase_done_pr: false,
            }
        }

        /// BUG-727: mark the spec's execution mode as holding the merge —
        /// `merge_supervision_hold` returns `Some(reason)`.
        fn merge_supervised(reason: &str) -> Self {
            Self {
                merge_hold: Some(reason.to_string()),
                ..Self::base()
            }
        }

        /// TASK-975: `finish_ci` reports red CI `reds` times before going
        /// green; `attempt_ci_fix` runs under `budget` and reports
        /// pushed=`pushes`.
        fn red_ci_with_fix(reds: usize, budget: usize, pushes: bool) -> Self {
            Self {
                ci_red_failures: reds,
                ci_fix_budget: budget,
                ci_fix_pushes: pushes,
                ..Self::base()
            }
        }

        // trace:BUG-1460 | ai:codex
        fn no_checks_then_head_advances() -> Self {
            Self {
                ci_no_checks_head_advance: true,
                ..Self::base()
            }
        }

        /// TASK-975: `merge` fails with a conflict-shaped error `conflicts`
        /// times before succeeding; `attempt_merge_conflict_rebase` runs under
        /// `budget` and reports clean=`rebase_ok`.
        fn merge_conflict_with_rebase(conflicts: usize, budget: usize, rebase_ok: bool) -> Self {
            Self {
                merge_conflicts: conflicts,
                ci_fix_budget: budget,
                conflict_rebase_ok: rebase_ok,
                ..Self::base()
            }
        }

        // trace:BUG-1447 | ai:codex
        fn merge_policy_refusal(budget: usize) -> Self {
            Self {
                merge_policy_refusals: 1,
                ci_fix_budget: budget,
                transient_retry_budget: 2,
                ..Self::base()
            }
        }

        fn all_ok() -> Self {
            Self::base()
        }

        /// BUG-657: the target spec is already terminal — `terminal_status`
        /// reports `status` and the orchestrator must finish as a NO-OP without
        /// spawning the implementer.
        // trace:BUG-657 | ai:claude
        fn already_terminal(status: &'static str) -> Self {
            Self {
                terminal: Some(status),
                ..Self::base()
            }
        }

        /// TASK-136: `run_implementer` returns
        /// [`ImplementerOutcome::Inconclusive`] AND `shelve_on_failure`
        /// succeeds — the batch-mode shelve→advance path. In a single-spec
        /// drain the same driver still pauses (the `batch` flag, not the
        /// driver, decides).
        fn inconclusive_and_shelves(reason: &str) -> Self {
            Self {
                inconclusive: Some(reason.to_string()),
                shelve_succeeds: true,
                ..Self::base()
            }
        }

        fn failing_at(phase: Phase) -> Self {
            Self {
                fail_at: Some(phase),
                ..Self::base()
            }
        }

        /// BUG-1524: like `failing_at`, but the mock failure carries `kind`
        /// instead of the generic `FailureKind::Failed` default.
        fn failing_at_with_kind(phase: Phase, kind: FailureKind) -> Self {
            Self {
                fail_at: Some(phase),
                fail_kind: Some(kind),
                ..Self::base()
            }
        }

        fn with_verdict(verdict: Verdict) -> Self {
            Self {
                verdict,
                ..Self::base()
            }
        }

        /// STORY-276: make `run_implementer` punt — phase 1 returns
        /// [`ImplementerOutcome::Punted`] with `reason`.
        fn punting_at_implementer(reason: &str) -> Self {
            Self {
                punt: Some(reason.to_string()),
                ..Self::base()
            }
        }

        /// BUG-257: make `run_implementer` return
        /// [`ImplementerOutcome::Inconclusive`] — the phase-1 PR lookup hit
        /// a transient GH-API blip and cannot confirm a PR either way.
        fn inconclusive_at_implementer(reason: &str) -> Self {
            Self {
                inconclusive: Some(reason.to_string()),
                ..Self::base()
            }
        }

        /// BUG-250: make `run_implementer` return [`ImplementerOutcome::Held`]
        /// — the implementer deliberately held the PR for a manual gate.
        fn holding_at_implementer(reason: &str) -> Self {
            Self {
                held: Some(reason.to_string()),
                ..Self::base()
            }
        }

        /// BUG-709: make `run_implementer` return
        /// [`ImplementerOutcome::AlreadyMerged`] — the implementer discovered
        /// its own PR already merged (it ran the full ship itself).
        // trace:BUG-1522 | ai:claude
        fn already_merged_at_implementer(pr_number: u32) -> Self {
            Self {
                already_merged: Some(pr_number),
                pr_number: Some(pr_number),
                ..Self::base()
            }
        }

        /// BUG-879: simulate phase 2 failing to resolve a real PR before the
        /// reviewer handoff.
        // trace:BUG-879 | ai:codex
        fn with_pr_number(mut self, pr_number: Option<u32>) -> Self {
            self.pr_number = pr_number;
            self
        }

        /// BUG-895: simulate the real driver opening a PR from the branch
        /// phase 2 already pushed after the implementer worktree is gone.
        // trace:BUG-895 | ai:codex
        fn recovering_review_pr(mut self, pr_number: u32) -> Self {
            self.recover_review_pr = Some(pr_number);
            self
        }

        // trace:BUG-1145 | ai:codex
        fn recovering_phase1_failure_from_pr(mut self, phase: Phase) -> Self {
            self.phase1_pr_recovery = Some(phase);
            self
        }

        // trace:STORY-975 | ai:codex
        fn reviewer_watchdog_then_succeeds(failures: usize) -> Self {
            Self {
                reviewer_watchdog_failures: failures,
                transient_retry_budget: 1,
                ..Self::base()
            }
        }

        /// BUG-1299: like [`Self::reviewer_watchdog_then_succeeds`] but with a
        /// caller-supplied `PhaseFailure` (reason + `hint_override`) instead
        /// of the generic message — lets a test drive a specific
        /// `WatchdogTrip`'s resolved report through retry-exhaustion and the
        /// eventual shelve, to prove the detail/hint pair survives intact.
        // trace:BUG-1299 | ai:claude
        fn reviewer_watchdog_then_succeeds_with(failures: usize, failure: PhaseFailure) -> Self {
            Self {
                reviewer_watchdog_failures: failures,
                reviewer_watchdog_failure: Some(failure),
                transient_retry_budget: 1,
                ..Self::base()
            }
        }

        // trace:BUG-906 | ai:codex
        fn reviewer_cache_locked_then_succeeds(failures: usize) -> Self {
            Self {
                reviewer_cache_locked_failures: failures,
                transient_retry_budget: 1,
                ..Self::base()
            }
        }

        /// STORY-306: make `run_reviewer` escalate the merge decision —
        /// phase 3 returns [`ReviewerOutcome::EscalatedToHuman`] with `reason`.
        fn reviewer_escalates_merge(reason: &str) -> Self {
            Self {
                reviewer_escalates: Some(reason.to_string()),
                ..Self::base()
            }
        }

        /// BUG-245: make this driver's [`PhaseDriver::shipped_spec_id`] return
        /// `Some(id)` — phase 1's PR credits this spec instead of the
        /// dispatched id, so the orchestrator must report the mismatch.
        fn shipping_as(mut self, shipped: &str) -> Self {
            self.shipped_spec_id = Some(shipped.to_string());
            self
        }

        /// BUG-241: make this driver's [`PhaseDriver::reconcile_failure`]
        /// report `reconcile` — the ground-truth verdict the orchestrator
        /// consults before declaring a phase failed.
        fn reconciles_as(mut self, reconcile: PhaseReconcile) -> Self {
            self.reconcile = reconcile;
            self
        }

        /// STORY-306: make `run_advisor` resolve the punted fork with `answer`.
        fn advisor_resolves(mut self, answer: &str) -> Self {
            self.advisor = Some(AdvisorOutcome::Resolved {
                answer: answer.to_string(),
                reasoning: "mock advisor reasoning".to_string(),
            });
            self
        }

        /// STORY-306: make `run_advisor` escalate the punted fork to a human.
        fn advisor_escalates(mut self, reason: &str) -> Self {
            self.advisor = Some(AdvisorOutcome::Escalated {
                reason: reason.to_string(),
                category: "strategy".to_string(),
            });
            self
        }

        /// STORY-306: make `resume_implementer` open a PR — the resumed
        /// implementer shipped on the advisor's answer.
        fn resume_opens_pr(mut self) -> Self {
            self.resume = Some(ImplementerOutcome::PrOpened);
            self
        }

        /// STORY-306: make `resume_implementer` punt again — the re-punt that
        /// must be terminal (one advisor round per spec).
        fn resume_repunts(mut self, reason: &str) -> Self {
            self.resume = Some(ImplementerOutcome::Punted {
                reason: reason.to_string(),
            });
            self
        }

        fn record(&mut self, phase: Phase) -> Result<(), PhaseFailure> {
            self.calls.push(phase);
            if self.fail_at == Some(phase) {
                // trace:BUG-1524 | ai:claude
                match self.fail_kind {
                    Some(kind) => Err(PhaseFailure::of(kind, format!("mock failure at {phase:?}"))),
                    None => Err(PhaseFailure::new(format!("mock failure at {phase:?}"))),
                }
            } else {
                Ok(())
            }
        }
    }

    impl PhaseDriver for MockPhaseDriver {
        fn capture_phase_done_pr(&mut self) {
            if !self.suppress_phase_done_pr {
                self.phase_done_pr = self.pr_number;
            }
        }
        fn phase_done_pr_number(&self) -> Option<u32> {
            self.phase_done_pr
        }
        fn requires_phase_done_pr(&self) -> bool {
            self.suppress_phase_done_pr
        }
        fn implementer_workspace(&self) -> Result<Option<(String, String)>, String> {
            self.workspace_resolutions
                .set(self.workspace_resolutions.get() + 1);
            match &self.workspace_error {
                Some(err) => Err(err.clone()),
                None => Ok(self.workspace.clone()),
            }
        }
        fn pin_implementer_workspace(&mut self, worktree: &str, branch: &str) {
            self.pinned_workspace = Some((worktree.to_string(), branch.to_string()));
        }
        fn rework_no_op_failure(&mut self) -> Option<PhaseFailure> {
            self.rework_no_op.then(|| {
                PhaseFailure::of(
                    FailureKind::ReworkNoOp,
                    "rework head abc123 unchanged; open item: fix parity",
                )
            })
        }
        fn run_harvest_gate(&mut self) {
            self.harvest_gate_calls += 1;
        }
        fn run_implementer(&mut self) -> Result<ImplementerOutcome, PhaseFailure> {
            // BUG-657: the orchestrator must skip phase 1 entirely for a
            // terminal target — if it reached here, the no-op guard failed.
            assert!(
                self.terminal.is_none(),
                "BUG-657: run_implementer must never be called for a terminal-status spec",
            );
            self.record(Phase::Implementer)?;
            if let Some(pr_number) = self.already_merged {
                return Ok(ImplementerOutcome::AlreadyMerged { pr_number });
            }
            if let Some(reason) = &self.inconclusive {
                return Ok(ImplementerOutcome::Inconclusive {
                    reason: reason.clone(),
                    retry_hint: self.inconclusive_retry_hint.clone(),
                });
            }
            if let Some(reason) = &self.held {
                return Ok(ImplementerOutcome::Held {
                    reason: Some(reason.clone()),
                    branch: "held-branch".to_string(),
                });
            }
            match &self.punt {
                Some(reason) => Ok(ImplementerOutcome::Punted {
                    reason: reason.clone(),
                }),
                None => Ok(ImplementerOutcome::PrOpened),
            }
        }
        fn finish_ci(&mut self) -> Result<(), PhaseFailure> {
            self.record(Phase::Ci)?;
            if self.ci_no_checks_head_advance {
                // This is the incident's first observation: phase 2 sees the
                // pushed head before its checks have registered.
                self.ci_ordering_events
                    .push("ci:PrNoChecks@pre-review-head");
            }
            // TASK-975: simulate a red terminal CI run that a pushed fix
            // (eventually) turns green.
            if self.ci_red_failures > 0 {
                self.ci_red_failures -= 1;
                return Err(PhaseFailure::of(
                    FailureKind::CiRed,
                    "CI is red on PR-46: mock failing check",
                ));
            }
            Ok(())
        }
        fn confirm_review_handoff(&mut self) -> Result<(), PhaseFailure> {
            if self.handoff_fails {
                Err(PhaseFailure::new("MR-35 reviewer queue insertion failed"))
            } else {
                Ok(())
            }
        }
        fn verify_ci_for_review(&mut self) -> Result<(), PhaseFailure> {
            if self.ci_no_checks_head_advance {
                // The PR advances mid-phase to the would-be reviewed commit;
                // its checks likewise have not registered. The production
                // boundary rejects this before PhaseEntered(reviewer).
                self.ci_ordering_events.push("head:reviewed-head");
                self.ci_ordering_events.push("ci:PrNoChecks@reviewed-head");
                return Err(PhaseFailure::of(
                    FailureKind::CiTimeout,
                    "CI checks did not register for the current head PR-46",
                ));
            }
            Ok(())
        }
        fn run_reviewer(&mut self) -> Result<ReviewerOutcome, PhaseFailure> {
            assert!(
                self.review_pr_number().is_some(),
                "BUG-879: reviewer must never launch without a positive PR number",
            );
            self.record(Phase::Reviewer)?;
            if self.reviewer_watchdog_failures > 0 {
                self.reviewer_watchdog_failures -= 1;
                return Err(self.reviewer_watchdog_failure.clone().unwrap_or_else(|| {
                    PhaseFailure::of(
                        FailureKind::Watchdog,
                        "the reviewer phase watchdog stopped the session",
                    )
                }));
            }
            if self.reviewer_cache_locked_failures > 0 {
                self.reviewer_cache_locked_failures -= 1;
                return Err(PhaseFailure::of(
                    FailureKind::CacheLocked,
                    "phase 3 retry found dead predecessor lease with a dirty worktree",
                ));
            }
            match &self.reviewer_escalates {
                Some(reason) => Ok(ReviewerOutcome::EscalatedToHuman {
                    reason: reason.clone(),
                }),
                None => Ok(ReviewerOutcome::Verdict(self.verdict)),
            }
        }
        fn merge(&mut self) -> Result<(), PhaseFailure> {
            self.record(Phase::Merge)?;
            // trace:TASK-1448 | ai:claude
            if self.merge_stale_approval {
                return Err(PhaseFailure::of(
                    FailureKind::StaleApproval,
                    "PR-46 was not merged: its approval was recorded at 08834c6045a9 but the PR \
                     head is now 1aca4e3e9251. Re-review the current head, then merge again.",
                ));
            }
            if self.merge_policy_refusals > 0 {
                self.merge_policy_refusals -= 1;
                return Err(PhaseFailure::of(
                    FailureKind::MergeHold,
                    "gh pr merge failed for #2014: X Pull request joemooney/aida#2014 is not mergeable:\n\
                     the base branch policy prohibits the merge.",
                ));
            }
            // TASK-975: simulate a merge conflict a clean rebase resolves.
            if self.merge_conflicts > 0 {
                self.merge_conflicts -= 1;
                return Err(PhaseFailure::new(
                    "`gh pr merge 46` failed: Pull Request is not mergeable",
                ));
            }
            Ok(())
        }
        // trace:BUG-727 | ai:claude
        fn merge_supervision_hold(&mut self) -> Option<String> {
            self.merge_hold.clone()
        }
        // trace:BUG-1168 | ai:codex
        fn record_merge_supervision_hold(&mut self, reason: &str) {
            self.recorded_merge_holds.push(reason.to_string());
        }
        fn pull(&mut self) -> Result<(), PhaseFailure> {
            self.record(Phase::Pull)
        }
        fn build(&mut self) -> Result<(), PhaseFailure> {
            self.record(Phase::Build)
        }
        fn hint_context(&self) -> HintContext {
            HintContext {
                spec: "TASK-247".to_string(),
                branch: Some("task-247".to_string()),
                pr_number: self.pr_number,
                implementer_session: Some("019e2f423e7c".to_string()),
                ci_run_id: Some("9988776655".to_string()),
                forge: crate::forge::ForgeKind::GitHub,
            }
        }
        fn reconcile_failure(&mut self, _phase: Phase, _failure: &PhaseFailure) -> PhaseReconcile {
            self.reconcile.clone()
        }
        fn terminal_status(&mut self) -> Option<&'static str> {
            self.terminal
        }
        fn recover_missing_review_pr(&mut self) -> Option<u32> {
            let pr = self.recover_review_pr.filter(|n| *n > 0)?;
            self.pr_number = Some(pr);
            Some(pr)
        }
        fn recover_phase1_failure_with_open_pr(
            &mut self,
            _failure: &PhaseFailure,
        ) -> Option<Phase> {
            self.phase1_pr_recovery
        }
        fn shipped_spec_id(&mut self) -> Option<String> {
            self.shipped_spec_id.clone()
        }
        fn run_advisor(&mut self) -> Result<AdvisorOutcome, PhaseFailure> {
            self.advisor_calls += 1;
            self.advisor.clone().ok_or_else(|| {
                PhaseFailure::of(FailureKind::Internal, "mock: no advisor outcome configured")
            })
        }
        fn resume_implementer(
            &mut self,
            _answer: &str,
        ) -> Result<ImplementerOutcome, PhaseFailure> {
            self.resume.clone().ok_or_else(|| {
                PhaseFailure::of(FailureKind::Internal, "mock: no resume outcome configured")
            })
        }
        fn mark_implementer_lease_escalated(&mut self) {
            self.mark_escalated_calls += 1;
        }
        fn shelve_on_failure(
            &mut self,
            _spec: &str,
            phase: Phase,
            failure: &PhaseFailure,
            recovery_hint: &str,
        ) -> anyhow::Result<Option<aida_core::FailureReason>> {
            if !self.shelve_succeeds {
                return Ok(None);
            }
            Ok(Some(aida_core::FailureReason {
                phase: phase.slug().to_string(),
                phase_index: phase.index() as u8,
                kind: failure.kind.cause_slug().to_string(),
                detail: failure.reason.clone(),
                recovery_hint: Some(recovery_hint.to_string()),
                shelved_by: None,
                shelved_at: chrono::Utc::now(),
            }))
        }
        fn handoff_open_pr_after_shelve(
            &mut self,
            spec: &str,
            phase: Phase,
            failure: &PhaseFailure,
        ) {
            self.handoff_calls
                .push((spec.to_string(), phase, failure.reason.clone()));
        }
        // TASK-975: CI auto-fix + merge-conflict rebase hooks.
        fn ci_fix_budget(&self) -> usize {
            self.ci_fix_budget
        }
        fn attempt_ci_fix(
            &mut self,
            attempt: usize,
            budget: usize,
            _failure: &PhaseFailure,
        ) -> bool {
            self.ci_fix_calls.push((attempt, budget));
            self.ci_fix_pushes
        }
        fn attempt_merge_conflict_rebase(&mut self, _failure: &PhaseFailure) -> bool {
            self.conflict_rebase_calls += 1;
            // TASK-1458: model the real driver faithfully — a clean rebase
            // moves the PR head past the recorded approval, so the retried
            // merge meets the TASK-1448 approval-covers-head gate.
            // trace:TASK-1458 | ai:claude
            if self.conflict_rebase_ok {
                self.merge_stale_approval = true;
            }
            self.conflict_rebase_ok
        }
        fn transient_retry_budget(&self) -> usize {
            self.transient_retry_budget
        }
        fn record_transient_retry(
            &mut self,
            _spec: &str,
            phase: Phase,
            cause: &str,
            attempt: u32,
            max: u32,
            detail: Option<&str>,
        ) {
            self.transient_retry_events.push((
                phase,
                cause.to_string(),
                attempt,
                max,
                detail.map(str::to_string),
            ));
        }
    }

    // --- Core orchestration: the mock-Claude integration test -------------

    #[test]
    fn lifecycle_skip_from_tags_parses_tags_case_insensitively() {
        let skip = LifecycleSkip::from_tags([
            "Lifecycle:No-CI-Wait",
            "lifecycle:no-review",
            "LIFECYCLE:NO-BUILD",
            "lifecycle:unknown",
        ]);
        assert!(skip.no_ci_wait);
        assert!(skip.no_review);
        assert!(skip.no_build);
    }

    /// TASK-525: active_tokens lists exactly the set skips (telemetry payload),
    /// empty when none, all three for `trivial`.
    #[test]
    fn lifecycle_skip_active_tokens() {
        assert!(LifecycleSkip::none().active_tokens().is_empty());
        assert_eq!(
            LifecycleSkip::from_tags(["lifecycle:no-review"]).active_tokens(),
            vec!["no-review".to_string()]
        );
        assert_eq!(
            LifecycleSkip::from_tags(["lifecycle:trivial"]).active_tokens(),
            vec![
                "no-ci-wait".to_string(),
                "no-review".to_string(),
                "no-build".to_string()
            ]
        );
    }

    #[test]
    fn lifecycle_skip_trivial_implies_all_three() {
        let skip = LifecycleSkip::from_tags(["lifecycle:trivial"]);
        assert_eq!(
            skip,
            LifecycleSkip {
                no_preflight: false,
                no_ci_wait: true,
                no_review: true,
                no_build: true,
                no_harvest: false,
                express: false,
            }
        );
    }

    #[test]
    fn lifecycle_no_preflight_is_recognized_and_named_in_banner() {
        let skip = LifecycleSkip::from_tags(["LIFECYCLE:NO-PREFLIGHT"]);
        assert!(skip.no_preflight);
        assert_eq!(skip.active_tokens(), vec!["no-preflight".to_string()]);
        assert_eq!(
            skip.banner_summary().as_deref(),
            Some("skipping implementer preflight")
        );
    }

    // trace:TASK-907 — the fasttrack express tier (`batch:express`) is
    // recognized at the single lifecycle-resolution point the drain uses, sets
    // the `express` marker, and skips NOTHING — full CI + reviewer + build. Its
    // trust contract: even if a `lifecycle:*` short-circuit is also present on
    // the spec, express overrides it so the gate is never silently downgraded.
    #[test]
    fn lifecycle_skip_express_tier_forces_full_gate() {
        // Plain express → marked, nothing skipped.
        let skip = LifecycleSkip::from_tags([EXPRESS_TIER_TAG]);
        assert!(skip.express, "batch:express sets the express marker");
        assert!(
            skip.is_empty(),
            "express skips no phase — is_empty() (the skip set) stays empty"
        );
        assert!(
            skip.active_tokens().is_empty(),
            "express records no short-circuit token in telemetry"
        );

        // Express + a lifecycle skip on EVERY integrity field the struct
        // carries, on the same spec → express wins on all of them; the gate
        // is NOT downgraded (the punt-out / trust-contract invariant).
        //
        // BUG-1507: this used to assert three named fields (no_ci_wait,
        // no_review, no_build) rather than the universal `is_empty()`, so it
        // could not fail when `no_preflight` was left out of the force-off —
        // which is exactly what happened. `is_empty()` is the SAME
        // instrument the plain-express case above already uses correctly;
        // using it here too closes the gap between the two cases.
        //
        // The destructuring bind below additionally makes a sixth skip field
        // fail to COMPILE at this exact site if it is ever added without a
        // decision about whether express should force it off — the
        // strongest form criterion 4 asks for. trace:BUG-1507 | ai:claude
        // (PRIN-5: absent is not good evidence)
        let conflicting = LifecycleSkip::from_tags([
            "lifecycle:trivial",
            EXPRESS_TIER_TAG,
            "lifecycle:no-review",
            "lifecycle:no-preflight",
        ]);
        let LifecycleSkip {
            no_preflight,
            no_ci_wait,
            no_review,
            no_build,
            no_harvest: _, // advisory, not an integrity phase — express does not touch it
            express,
        } = conflicting;
        assert!(express);
        assert!(
            !no_preflight && !no_ci_wait && !no_review && !no_build,
            "express overrides any lifecycle:* short-circuit — full gate enforced"
        );
        assert!(
            conflicting.is_empty(),
            "express skips no phase even when conflicting lifecycle:* tags are present"
        );

        // Case-insensitive, matching the rest of from_tags.
        assert!(LifecycleSkip::from_tags(["Batch:Express"]).express);

        // The banner announces the tier even though nothing is skipped.
        assert_eq!(
            skip.banner_summary().as_deref(),
            Some("express tier — full gate (CI + reviewer + build)")
        );
    }

    /// TASK-524: the typo guard flags `lifecycle:*` tags that aren't recognized
    /// short-circuits (they silently no-op), while passing the real ones and
    /// non-lifecycle tags. Stays in sync with `from_tags` via the shared
    /// RECOGNIZED_LIFECYCLE_TAGS list.
    #[test]
    fn unrecognized_lifecycle_tag_typo_guard() {
        use super::is_unrecognized_lifecycle_tag;
        // Recognized → not flagged (case-insensitive, like from_tags).
        for t in [
            "lifecycle:no-ci-wait",
            "lifecycle:no-review",
            "lifecycle:no-build",
            "lifecycle:trivial",
            "Lifecycle:Trivial",
        ] {
            assert!(!is_unrecognized_lifecycle_tag(t), "recognized: {t}");
        }
        // Typos in the lifecycle namespace → flagged.
        for t in [
            "lifecycle:no-ci",     // missing -wait
            "lifecycle:trivai",    // misspelled
            "lifecycle:no-builds", // plural
            "lifecycle:skip-ci",   // wrong name
        ] {
            assert!(is_unrecognized_lifecycle_tag(t), "typo: {t}");
        }
        // Non-lifecycle tags are never flagged.
        for t in ["papercut", "batch:x", "from-review:PR-1", "aida:queue:work"] {
            assert!(!is_unrecognized_lifecycle_tag(t), "non-lifecycle: {t}");
        }
        // Every recognized tag must agree with from_tags (no drift).
        for t in super::RECOGNIZED_LIFECYCLE_TAGS {
            assert!(
                !LifecycleSkip::from_tags([*t]).is_empty(),
                "{t} should parse"
            );
        }
    }

    #[test]
    fn lifecycle_skip_unknown_lifecycle_tag_is_ignored() {
        assert_eq!(
            LifecycleSkip::from_tags(["lifecycle:no-ci", "papercut"]),
            LifecycleSkip::none()
        );
    }

    // trace:BUG-1222 | ai:codex
    #[test]
    fn lifecycle_skip_banner_summary_names_each_skipped_phase() {
        for (tag, expected) in [
            ("lifecycle:no-ci-wait", "skipping CI wait"),
            ("lifecycle:no-review", "skipping reviewer"),
            ("lifecycle:no-build", "skipping build"),
            ("lifecycle:no-harvest", "skipping harvest"),
        ] {
            assert_eq!(
                LifecycleSkip::from_tags([tag]).banner_summary().as_deref(),
                Some(expected),
                "banner label for {tag}"
            );
        }

        assert_eq!(
            LifecycleSkip::from_tags([
                "lifecycle:no-ci-wait",
                "lifecycle:no-review",
                "lifecycle:no-build",
                "lifecycle:no-harvest",
            ])
            .banner_summary()
            .as_deref(),
            Some("skipping CI wait + reviewer + build + harvest")
        );
    }

    // TASK-1249: the advisory harvest step runs exactly once per full run,
    // after the reviewer and before merge, and a shelved review never reaches it.
    #[test]
    fn harvest_gate_runs_once_between_review_and_merge() {
        let mut driver = MockPhaseDriver::all_ok();
        let r = orchestrate(
            &mut driver,
            "TASK-1249",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(r.exit_code, 0);
        assert_eq!(driver.harvest_gate_calls, 1);
        let mut shelved = MockPhaseDriver::all_ok();
        shelved.verdict = Verdict::RequestChanges;
        let _ = orchestrate(
            &mut shelved,
            "TASK-1249",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(
            shelved.harvest_gate_calls, 0,
            "a failed review never reaches the harvest step"
        );
        assert!(LifecycleSkip::from_tags(["lifecycle:no-harvest"]).no_harvest);
        assert!(!is_unrecognized_lifecycle_tag("lifecycle:no-harvest"));
    }

    /// PR 1970 / STORY-1354 regression: a run that reaches an open PR and
    /// then shelves in the reviewer phase must leave review claimable instead
    /// of dropping ownership between phases.
    // trace:BUG-1291 | ai:codex
    #[test]
    fn reviewer_phase_shelve_hands_open_pr_to_reviewer_queue() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Reviewer);
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "BUG-1268",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );

        assert_eq!(result.failed_phase, Some(Phase::Reviewer));
        assert!(result.shelved_reason.is_some());
        assert_eq!(driver.handoff_calls.len(), 1);
        assert_eq!(driver.handoff_calls[0].0, "BUG-1268");
        assert_eq!(driver.handoff_calls[0].1, Phase::Reviewer);
        assert!(driver.handoff_calls[0].2.contains("mock failure"));
    }

    #[test]
    fn orchestrate_full_pipeline_runs_all_six_phases() {
        let mut driver = MockPhaseDriver::all_ok();
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 0);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ]
        );
    }

    #[test]
    fn phase_one_refuses_a_sibling_specs_worktree_before_implementing() {
        let mut driver = MockPhaseDriver::all_ok();
        driver.workspace = Some((
            std::env::temp_dir()
                .join("aida-story-1221")
                .display()
                .to_string(),
            "story-1221".to_string(),
        ));
        let result = orchestrate(
            &mut driver,
            "BUG-1236",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert!(
            driver.calls.is_empty(),
            "implementer must not run in sibling worktree"
        );
        assert!(result
            .failure
            .as_ref()
            .is_some_and(|f| f.reason.contains("story-1221")));
    }

    // BUG-1628: a custom-prefix spec whose workspace came from the shared
    // pickup resolver (`spec-016`) passes the phase-1 ownership check.
    // trace:BUG-1628 | ai:claude
    #[test]
    fn bug_1628_phase_one_accepts_custom_prefix_pickup_workspace() {
        let mut driver = MockPhaseDriver::all_ok();
        driver.workspace = Some((
            std::env::temp_dir()
                .join("qci-spec-016")
                .display()
                .to_string(),
            "spec-016".to_string(),
        ));
        let result = orchestrate(
            &mut driver,
            "SPEC-016",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
        );
        assert_ne!(result.failed_phase, Some(Phase::Implementer));
        assert!(
            driver.calls.contains(&Phase::Implementer),
            "the implementer must run in the spec's own pickup workspace"
        );
    }

    // BUG-1629: phase 1 resolves the workspace exactly once and pins the
    // value it checked, instead of re-resolving before the launch.
    // trace:BUG-1629 | ai:claude
    #[test]
    fn bug_1629_phase_one_resolves_once_and_pins_the_checked_workspace() {
        let mut driver = MockPhaseDriver::all_ok();
        let checked = (
            std::env::temp_dir()
                .join("qci-spec-016")
                .display()
                .to_string(),
            "spec-016".to_string(),
        );
        driver.workspace = Some(checked.clone());
        let result = orchestrate(
            &mut driver,
            "SPEC-016",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
        );
        assert_ne!(result.failed_phase, Some(Phase::Implementer));
        assert_eq!(driver.workspace_resolutions.get(), 1);
        assert_eq!(driver.pinned_workspace, Some(checked));
    }

    // BUG-1629: a refused workspace is never pinned.
    // trace:BUG-1629 | ai:claude
    #[test]
    fn bug_1629_phase_one_never_pins_a_refused_workspace() {
        let mut driver = MockPhaseDriver::all_ok();
        driver.workspace = Some((
            std::env::temp_dir()
                .join("qci-spec-017")
                .display()
                .to_string(),
            "spec-017".to_string(),
        ));
        let _ = orchestrate(
            &mut driver,
            "SPEC-016",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
        );
        assert_eq!(driver.pinned_workspace, None);
        assert!(driver.calls.is_empty());
    }

    // BUG-1628: a mismatched workspace is refused with a message naming the
    // resolver mismatch, not a generic sibling-workspace refusal.
    // trace:BUG-1628 | ai:claude
    #[test]
    fn bug_1628_phase_one_mismatch_names_the_resolver() {
        let mut driver = MockPhaseDriver::all_ok();
        driver.workspace = Some((
            std::env::temp_dir()
                .join("qci-spec-017")
                .display()
                .to_string(),
            "spec-017".to_string(),
        ));
        let result = orchestrate(
            &mut driver,
            "SPEC-016",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert!(driver.calls.is_empty());
        let reason = &result.failure.as_ref().expect("failure").reason;
        assert!(reason.contains("resolver mismatch"), "{reason}");
        assert!(reason.contains("spec-017"), "{reason}");
        assert!(reason.contains("aida queue work SPEC-016"), "{reason}");
        assert!(!reason.contains("refused sibling workspace"), "{reason}");
    }

    // BUG-1628: a resolver failure is reported as such before any lease.
    // trace:BUG-1628 | ai:claude
    #[test]
    fn bug_1628_phase_one_resolver_failure_is_named() {
        let mut driver = MockPhaseDriver::all_ok();
        driver.workspace_error = Some("all 20 candidate branch names are taken".to_string());
        let result = orchestrate(
            &mut driver,
            "SPEC-016",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert!(driver.calls.is_empty());
        let reason = &result.failure.as_ref().expect("failure").reason;
        assert!(reason.contains("resolver failed for SPEC-016"), "{reason}");
        assert!(
            reason.contains("candidate branch names are taken"),
            "{reason}"
        );
    }

    #[test]
    fn reviewer_refuses_when_this_run_has_no_phase_done_pr() {
        let mut driver = MockPhaseDriver::all_ok();
        driver.suppress_phase_done_pr = true;
        let result = orchestrate(
            &mut driver,
            "BUG-1244",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Reviewer));
        assert!(!driver.calls.contains(&Phase::Reviewer));
        assert!(result
            .failure
            .as_ref()
            .is_some_and(|failure| failure.reason.contains("PhaseDonePr")));
    }

    // --- STORY-975: transient whole-phase self-retry ----------------------

    #[test]
    fn transient_retry_policy_is_closed_and_bounded() {
        for cause in [
            "watchdog",
            "no-verdict",
            "no-pr",
            "tool-exit",
            "cache-locked",
            "headless-wait",
        ] {
            assert!(is_transient_retry_cause(cause), "{cause}");
        }
        for cause in [
            "verdict:request-changes",
            "verdict:reject",
            "ci-red",
            "lease-conflict",
            // trace:BUG-1930 | ai:claude
            "permission-denied",
            "merge-hold",
            "environmental",
            "internal",
            // trace:TASK-1459 | ai:claude
            "review-in-progress",
            // BUG-1629: a refused launch never started anything, and phase 1
            // already spent its single replacement; the orchestrator's
            // transient budget must not multiply it.
            // trace:BUG-1629 | ai:claude
            "launch-refused",
            "lost-child-state",
        ] {
            assert!(!is_transient_retry_cause(cause), "{cause}");
        }
        assert_eq!(clamp_transient_retry_budget(0), 0);
        assert_eq!(clamp_transient_retry_budget(1), 1);
        assert_eq!(clamp_transient_retry_budget(99), 3);
        assert!(should_retry_transient_failure(FailureKind::Watchdog, 0, 1));
        assert!(!should_retry_transient_failure(FailureKind::Watchdog, 1, 1));
        assert!(should_retry_transient_failure(
            FailureKind::HeadlessWait,
            0,
            1
        ));
        assert!(!should_retry_transient_failure(
            FailureKind::VerdictRequestChanges,
            0,
            1
        ));
        assert!(FailureKind::LeaseConflict.is_shelvable());
        assert_eq!(FailureKind::LeaseConflict.cause_slug(), "lease-conflict");
        assert!(FailureKind::CiUnavailable.is_shelvable());
        assert_eq!(FailureKind::CiUnavailable.cause_slug(), "ci-unavailable");
        assert!(FailureKind::MergeHold.is_shelvable());
        assert_eq!(FailureKind::MergeHold.cause_slug(), "merge-hold");
        assert!(!should_retry_transient_failure(
            FailureKind::MergeHold,
            0,
            2
        ));
    }

    #[test]
    fn orchestrate_retries_watchdog_reviewer_once_then_ships() {
        let mut driver = MockPhaseDriver::reviewer_watchdog_then_succeeds(1);
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );

        assert_eq!(result.exit_code, 0);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ]
        );
        assert_eq!(
            driver.transient_retry_events,
            vec![(
                Phase::Reviewer,
                "watchdog".to_string(),
                2,
                2,
                Some("the reviewer phase watchdog stopped the session".to_string()),
            )]
        );
    }

    #[test]
    fn orchestrate_does_not_retry_request_changes_verdict() {
        let mut driver = MockPhaseDriver::with_verdict(Verdict::RequestChanges);
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );

        assert_eq!(result.failed_phase, Some(Phase::Reviewer));
        assert_eq!(
            result.failure.as_ref().map(|f| f.kind),
            Some(FailureKind::VerdictRequestChanges)
        );
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer, Phase::Ci, Phase::Reviewer]
        );
        assert!(driver.transient_retry_events.is_empty());
    }

    #[test]
    fn exhausted_transient_retry_carries_attempt_count_in_failure() {
        let mut driver = MockPhaseDriver::reviewer_watchdog_then_succeeds(2);
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );

        assert_eq!(result.failed_phase, Some(Phase::Reviewer));
        let failure = result.failure.as_ref().expect("failure recorded");
        assert_eq!(failure.kind, FailureKind::Watchdog);
        assert!(failure.reason.contains("attempt 2/2"), "{}", failure.reason);
        let shelved = result.shelved_reason.as_ref().expect("shelved");
        assert!(shelved.detail.contains("attempt 2/2"), "{}", shelved.detail);
    }

    /// BUG-1299: acceptance criterion 8 — end to end, through retry
    /// exhaustion and the eventual shelve, a Ceiling-caused failure must
    /// produce a `shelved_reason` whose `detail` AND `recovery_hint` both
    /// describe the ceiling, and a NoProgress-caused failure must produce
    /// both describing no-progress — never a detail naming one trip beside a
    /// hint describing the other. This reproduces the exact TASK-1274
    /// contradiction (Ceiling detail + a hint hardcoded for NoProgress that
    /// called a 10-commit, still-writing session "likely a degenerate spin")
    /// and proves it can no longer happen.
    // trace:BUG-1299 | ai:claude
    #[test]
    fn exhausted_watchdog_retry_shelve_detail_and_hint_agree_on_the_trip() {
        use std::time::Duration;
        let no_progress = Duration::from_secs(600);
        let ceiling = Duration::from_secs(2700);

        // --- Ceiling: the session ran out of clock, not stuck. -------------
        let (ceil_tag, ceil_hint) =
            watchdog_trip_report(&WatchdogTrip::Ceiling, no_progress, ceiling, false, false);
        let ceil_failure = PhaseFailure::of(
            FailureKind::Watchdog,
            format!("the implementer phase watchdog stopped the session — {ceil_tag}"),
        )
        .with_hint_override(ceil_hint);
        let mut driver = MockPhaseDriver::reviewer_watchdog_then_succeeds_with(2, ceil_failure);
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "TASK-1274",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        let shelved = result.shelved_reason.as_ref().expect("shelved");
        assert!(shelved.detail.contains("ceiling:"), "{}", shelved.detail);
        let ceil_recovery_hint = shelved
            .recovery_hint
            .as_deref()
            .expect("watchdog shelve carries a recovery hint");
        assert!(
            ceil_recovery_hint
                .to_lowercase()
                .contains("wall-clock ceiling"),
            "{ceil_recovery_hint}"
        );
        assert!(
            !ceil_recovery_hint
                .to_lowercase()
                .contains("degenerate spin"),
            "a ceiling shelve must not tell the operator to hunt a spin: {ceil_recovery_hint}"
        );

        // --- NoProgress: the session genuinely stalled. ---------------------
        let (np_tag, np_hint) = watchdog_trip_report(
            &WatchdogTrip::NoProgress,
            no_progress,
            ceiling,
            false,
            false,
        );
        let np_failure = PhaseFailure::of(
            FailureKind::Watchdog,
            format!("the implementer phase watchdog stopped the session — {np_tag}"),
        )
        .with_hint_override(np_hint);
        let mut driver = MockPhaseDriver::reviewer_watchdog_then_succeeds_with(2, np_failure);
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "TASK-1274",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        let shelved = result.shelved_reason.as_ref().expect("shelved");
        assert!(
            shelved.detail.contains("no-progress:"),
            "{}",
            shelved.detail
        );
        let np_recovery_hint = shelved
            .recovery_hint
            .as_deref()
            .expect("watchdog shelve carries a recovery hint");
        assert!(
            np_recovery_hint.to_lowercase().contains("degenerate spin"),
            "{np_recovery_hint}"
        );
        assert!(
            !np_recovery_hint
                .to_lowercase()
                .contains("wall-clock ceiling"),
            "a no-progress shelve must not mention the ceiling: {np_recovery_hint}"
        );

        // The two shelves must not have produced identical hints.
        assert_ne!(ceil_recovery_hint, np_recovery_hint);
    }

    #[test]
    fn exhausted_cache_locked_reviewer_retry_parks_with_typed_cause() {
        let mut driver = MockPhaseDriver::reviewer_cache_locked_then_succeeds(2);
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );

        assert_eq!(result.failed_phase, Some(Phase::Reviewer));
        let failure = result.failure.as_ref().expect("failure recorded");
        assert_eq!(failure.kind, FailureKind::CacheLocked);
        assert!(failure.reason.contains("attempt 2/2"), "{}", failure.reason);
        assert_eq!(
            driver.transient_retry_events,
            vec![(
                Phase::Reviewer,
                "cache-locked".to_string(),
                2,
                2,
                Some(
                    "phase 3 retry found dead predecessor lease with a dirty worktree".to_string()
                ),
            )]
        );
        let shelved = result.shelved_reason.as_ref().expect("shelved");
        assert_eq!(shelved.kind, "cache-locked");
    }

    // --- TASK-975: CI auto-fix loop + in-drain merge-conflict rebase -------

    /// TASK-975: the pure retry-or-shelve decision. Budget 0 (the default —
    /// feature off) always shelves; a red CI run under budget yields 1-based
    /// fix attempts until the budget is exhausted; a non-red failure kind
    /// (timeout, spawn, internal) shelves even with budget left.
    // trace:TASK-975 | ai:claude
    #[test]
    fn red_ci_action_retry_shelve_decision() {
        // Default off: budget 0 shelves immediately.
        assert_eq!(red_ci_action(0, 0, FailureKind::CiRed), RedCiAction::Shelve);
        // Under budget: 1-based attempts.
        assert_eq!(
            red_ci_action(2, 0, FailureKind::CiRed),
            RedCiAction::AttemptFix { attempt: 1 }
        );
        assert_eq!(
            red_ci_action(2, 1, FailureKind::CiRed),
            RedCiAction::AttemptFix { attempt: 2 }
        );
        // Budget exhausted: shelve is the terminal fallback.
        assert_eq!(red_ci_action(2, 2, FailureKind::CiRed), RedCiAction::Shelve);
        // Only a red terminal run is fixable by editing code.
        for kind in [
            FailureKind::CiTimeout,
            FailureKind::Failed,
            FailureKind::Spawn,
            FailureKind::Internal,
        ] {
            assert_eq!(
                red_ci_action(3, 0, kind),
                RedCiAction::Shelve,
                "{kind:?} must not enter the fix loop"
            );
        }
    }

    /// TASK-975: the merge-conflict classifier recognizes the `gh pr merge`
    /// conflict spellings and rejects non-conflict merge failures.
    // trace:TASK-975 | ai:claude
    #[test]
    fn merge_conflict_failure_classifier() {
        assert!(is_merge_conflict_failure(
            "`gh pr merge 46` failed: Pull Request is not mergeable"
        ));
        assert!(is_merge_conflict_failure(
            "`gh pr merge 12` failed: pull request #12 is not mergeable: the merge commit cannot be cleanly created"
        ));
        assert!(is_merge_conflict_failure(
            "merge conflict between task-9 and main"
        ));
        // Non-conflict merge failures are not rebase-recoverable.
        assert!(!is_merge_conflict_failure(
            "`gh pr merge 46` failed: HTTP 502 bad gateway"
        ));
        assert!(!is_merge_conflict_failure(
            "`gh pr merge 46` failed: base branch policy prohibits the merge"
        ));
        assert!(!is_merge_conflict_failure("mock failure at Merge"));

        // BUG-1447: verbatim PR-2014 refusal. The generic prefix overlaps the
        // conflict vocabulary; the policy detail must win.
        let held =
            "gh pr merge failed for #2014: X Pull request joemooney/aida#2014 is not mergeable:\n\
                    the base branch policy prohibits the merge.";
        assert!(is_branch_policy_merge_refusal(held));
        assert!(!is_merge_conflict_failure(held));
    }

    /// TASK-975: the conflict-rebase gate — armed only by a non-zero budget,
    /// a conflict-shaped failure, and no prior attempt this phase.
    // trace:TASK-975 | ai:claude
    #[test]
    fn conflict_rebase_gate_is_bounded_and_opt_in() {
        let conflict = "`gh pr merge 46` failed: Pull Request is not mergeable";
        assert!(should_attempt_conflict_rebase(1, false, conflict));
        // Off by default (budget 0).
        assert!(!should_attempt_conflict_rebase(0, false, conflict));
        // One rebase attempt per phase, then shelve.
        assert!(!should_attempt_conflict_rebase(1, true, conflict));
        // Not a conflict → not rebase-recoverable.
        assert!(!should_attempt_conflict_rebase(
            1,
            false,
            "`gh pr merge 46` failed: HTTP 502"
        ));
    }

    /// TASK-975: a red CI run whose in-drain fix goes green ships the full
    /// pipeline — CI polled twice, exactly one fix attempt, no failure.
    // trace:TASK-975 | ai:claude
    #[test]
    fn orchestrate_red_ci_fixed_in_drain_ships() {
        let mut driver = MockPhaseDriver::red_ci_with_fix(1, 2, true);
        let result = orchestrate(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0, "{:?}", result.failure);
        assert_eq!(driver.ci_fix_calls, vec![(1, 2)]);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Ci, // re-poll after the pushed fix
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ]
        );
    }

    /// TASK-975: budget 0 (the default) is the pre-TASK-975 behaviour — a red
    /// CI run fails phase 2 with no fix attempt.
    // trace:TASK-975 | ai:claude
    #[test]
    fn orchestrate_red_ci_budget_zero_fails_without_fix_attempt() {
        let mut driver = MockPhaseDriver::red_ci_with_fix(1, 0, true);
        let result = orchestrate(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Ci));
        assert!(driver.ci_fix_calls.is_empty(), "no fix under budget 0");
    }

    /// TASK-975: the loop is bounded — CI stays red past the budget, the
    /// fix attempts stop at the budget, and the failure shelves (the EPIC-28
    /// terminal fallback).
    // trace:TASK-975 | ai:claude
    #[test]
    fn orchestrate_red_ci_budget_exhausted_shelves() {
        let mut driver = MockPhaseDriver::red_ci_with_fix(5, 2, true);
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Ci));
        assert_eq!(driver.ci_fix_calls, vec![(1, 2), (2, 2)], "bounded at 2");
        assert!(
            result.shelved_reason.is_some(),
            "shelve stays the terminal fallback"
        );
        assert_eq!(result.process_exit_code(), DRIVE_EXIT_SHELVED);
    }

    /// TASK-975: a fix session that pushes nothing does not spin the loop —
    /// the original red-CI failure proceeds straight to the failure path.
    // trace:TASK-975 | ai:claude
    #[test]
    fn orchestrate_red_ci_fix_pushes_nothing_fails_without_repoll() {
        let mut driver = MockPhaseDriver::red_ci_with_fix(1, 3, false);
        let result = orchestrate(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Ci));
        assert_eq!(driver.ci_fix_calls, vec![(1, 3)], "one attempt, no re-poll");
        let ci_polls = driver.calls.iter().filter(|p| **p == Phase::Ci).count();
        assert_eq!(ci_polls, 1, "no re-poll when nothing was pushed");
    }

    /// TASK-975 + TASK-1458: a merge conflict resolved by one in-drain rebase
    /// retries the merge once — and that retry SHELVES, because the rebase
    /// moved the head past the recorded approval and the TASK-1448 gate
    /// refuses an approval that does not cover the head. The rebase path
    /// cannot ship on its own until re-review (or a patch-id-equivalence
    /// rule) exists; this test pins that honest outcome instead of the old
    /// mock-only "ships".
    // trace:TASK-975 trace:TASK-1458 | ai:claude
    #[test]
    fn orchestrate_merge_conflict_rebased_in_drain_shelves_on_stale_approval() {
        let mut driver = MockPhaseDriver::merge_conflict_with_rebase(1, 1, true);
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(driver.conflict_rebase_calls, 1);
        let merges = driver.calls.iter().filter(|p| **p == Phase::Merge).count();
        assert_eq!(merges, 2, "one retry after the clean rebase");
        assert_eq!(result.failed_phase, Some(Phase::Merge));
        assert_eq!(
            result.failure.as_ref().map(|f| f.kind),
            Some(FailureKind::StaleApproval),
            "the retry meets the approval-covers-head gate"
        );
        assert!(result.shelved_reason.is_some(), "shelved, never shipped");
        assert!(
            !driver.calls.contains(&Phase::Pull),
            "a refused merge never reaches pull"
        );
    }

    /// TASK-975: a rebase that itself conflicts leaves the merge failure
    /// standing — one attempt, then the failure path.
    // trace:TASK-975 | ai:claude
    #[test]
    fn orchestrate_merge_conflict_rebase_fails_leaves_failure_standing() {
        let mut driver = MockPhaseDriver::merge_conflict_with_rebase(1, 1, false);
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Merge));
        assert_eq!(driver.conflict_rebase_calls, 1);
        assert!(result.shelved_reason.is_some());
    }

    /// TASK-975: with the feature off (budget 0) a merge conflict fails phase
    /// 4 with no rebase attempt — the pre-TASK-975 behaviour.
    // trace:TASK-975 | ai:claude
    #[test]
    fn orchestrate_merge_conflict_budget_zero_no_rebase() {
        let mut driver = MockPhaseDriver::merge_conflict_with_rebase(1, 0, true);
        let result = orchestrate(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Merge));
        assert_eq!(driver.conflict_rebase_calls, 0);
    }

    /// TASK-975: a NON-conflict merge failure never triggers the rebase, even
    /// with budget armed.
    // trace:TASK-975 | ai:claude
    #[test]
    fn orchestrate_merge_non_conflict_failure_no_rebase() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Merge);
        driver.ci_fix_budget = 2;
        let result = orchestrate(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Merge));
        assert_eq!(driver.conflict_rebase_calls, 0);
    }

    /// BUG-1447: a forge-enforced supervised hold is stable policy, not a
    /// conflict or transient tool exit. It gets one merge call, zero rebase
    /// calls, and zero retry records even when both budgets are armed.
    // trace:BUG-1447 | ai:codex
    #[test]
    fn orchestrate_branch_policy_refusal_never_rebases_or_retries() {
        let mut driver = MockPhaseDriver::merge_policy_refusal(2);
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "BUG-1447",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Merge));
        assert_eq!(driver.conflict_rebase_calls, 0, "held PR must never rebase");
        assert!(
            driver.transient_retry_events.is_empty(),
            "policy refusal must not spend transient retries"
        );
        assert_eq!(
            driver.calls.iter().filter(|p| **p == Phase::Merge).count(),
            1,
            "policy refusal is attempted once"
        );
    }

    /// TASK-1448: a stale approval at the merge phase SHELVES the spec — the
    /// drain parks it and continues; it never stops the batch, never rebases,
    /// never spends a transient retry, and never reaches pull.
    // trace:TASK-1448 | ai:claude
    #[test]
    fn orchestrate_stale_approval_at_merge_shelves_never_exits() {
        let mut driver = MockPhaseDriver {
            merge_stale_approval: true,
            ci_fix_budget: 2,
            transient_retry_budget: 2,
            ..MockPhaseDriver::base()
        };
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "TASK-1448",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Merge));
        let shelved = result
            .shelved_reason
            .as_ref()
            .expect("stale approval must shelve");
        assert_eq!(shelved.kind, "stale-approval");
        let failure = result.failure.as_ref().expect("failure recorded");
        assert_eq!(failure.kind, FailureKind::StaleApproval);
        assert!(
            failure.reason.contains("Re-review the current head"),
            "{}",
            failure.reason
        );
        assert_eq!(driver.conflict_rebase_calls, 0);
        assert!(driver.transient_retry_events.is_empty());
        assert_eq!(
            driver.calls.iter().filter(|p| **p == Phase::Merge).count(),
            1
        );
        assert!(
            !driver.calls.contains(&Phase::Pull),
            "must never pull a refused merge"
        );
    }

    /// BUG-1460: reproduce the incident ordering. Phase 2 initially sees
    /// PrNoChecks, then the PR head advances before that head's checks start.
    /// The drain must stop in CI and never enter/announce the reviewer phase.
    // trace:BUG-1460 | ai:codex
    #[test]
    fn orchestrate_does_not_review_head_pushed_before_checks_register() {
        let mut driver = MockPhaseDriver::no_checks_then_head_advances();
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "BUG-1460",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );

        assert_eq!(result.failed_phase, Some(Phase::Ci));
        assert_eq!(
            driver.ci_ordering_events,
            vec![
                "ci:PrNoChecks@pre-review-head",
                "head:reviewed-head",
                "ci:PrNoChecks@reviewed-head",
            ],
            "fixture must report no checks, advance the head mid-phase, then reject its unregistered checks"
        );
        assert_eq!(driver.calls, vec![Phase::Implementer, Phase::Ci]);
        assert!(
            !driver.calls.contains(&Phase::Reviewer),
            "reviewer phase must not be entered for a head whose checks have not started"
        );
    }

    /// BUG-657: driving an already-COMPLETED spec is a clean NO-OP — the
    /// orchestrator must NOT spawn an implementer (which would exit 1 and
    /// auto-draft a phantom failure BUG). No phase runs; the result is a clean
    /// exit 0 with no failed phase or failure.
    // trace:BUG-657 | ai:claude
    #[test]
    fn orchestrate_terminal_completed_spec_is_a_noop_no_implementer() {
        let mut driver = MockPhaseDriver::already_terminal("Completed");
        let result = orchestrate(
            &mut driver,
            "BUG-638",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        // No phase ran at all — the implementer was never spawned (the mock's
        // `run_implementer` asserts it is never reached for a terminal spec).
        assert!(
            driver.calls.is_empty(),
            "no phase should run for a terminal spec"
        );
        // A clean non-failure stop: exit 0, no failed phase, no failure → no
        // auto-drafted failure BUG.
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.process_exit_code(), DRIVE_EXIT_CLEAN);
        assert!(result.failed_phase.is_none());
        assert!(result.failure.is_none());
    }

    /// BUG-657: same NO-OP for an already-REJECTED target.
    // trace:BUG-657 | ai:claude
    #[test]
    fn orchestrate_terminal_rejected_spec_is_a_noop() {
        let mut driver = MockPhaseDriver::already_terminal("Rejected");
        let result = orchestrate(
            &mut driver,
            "TASK-9",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert!(driver.calls.is_empty());
        assert_eq!(result.process_exit_code(), DRIVE_EXIT_CLEAN);
        assert!(result.failed_phase.is_none());
    }

    /// TASK-1054: a single-spec drive's PROCESS exit code collapses the failed-
    /// phase index to the canonical 0/2/3 table. A SHELVED failure (the spec was
    /// parked `NeedsAttention`) exits 2 — the preserved EPIC-28 sentinel — even
    /// when the failed phase index is not 2.
    // trace:TASK-1054 | ai:claude
    #[test]
    fn process_exit_code_shelved_failure_is_2() {
        // A build (phase 6) failure that shelved the spec.
        let mut driver = MockPhaseDriver::failing_at(Phase::Build);
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert!(result.shelved_reason.is_some(), "the spec was parked");
        // The internal field stays the failed-phase index (6)…
        assert_eq!(result.exit_code, Phase::Build.index());
        // …but the PROCESS code is the shelved sentinel 2, NOT 6.
        assert_eq!(result.process_exit_code(), DRIVE_EXIT_SHELVED);
    }

    /// TASK-1054: an un-shelvable hard failure (the mock's default
    /// `shelve_on_failure` returns `Ok(None)`) exits 3 — distinct from the
    /// shelved `2` — so a wrapper can tell a hard wall from a parked spec.
    // trace:TASK-1054 | ai:claude
    #[test]
    fn process_exit_code_unshelved_hard_failure_is_3() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Ci);
        let result = orchestrate(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert!(result.shelved_reason.is_none(), "no shelve happened");
        assert_eq!(result.process_exit_code(), DRIVE_EXIT_HARD_FAIL);
    }

    /// TASK-1054: a clean ship exits 0 at the process level.
    // trace:TASK-1054 | ai:claude
    #[test]
    fn process_exit_code_clean_ship_is_0() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.process_exit_code(), DRIVE_EXIT_CLEAN);
    }

    #[test]
    fn orchestrate_no_review_skips_phase_3_and_runs_merge_pull_build() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate_with_lifecycle_skip(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip {
                no_review: true,
                ..LifecycleSkip::none()
            },
            false,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Merge,
                Phase::Pull,
                Phase::Build
            ]
        );
    }

    #[test]
    fn orchestrate_no_build_skips_phase_6_only() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate_with_lifecycle_skip(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip {
                no_build: true,
                ..LifecycleSkip::none()
            },
            false,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull
            ]
        );
    }

    #[test]
    fn orchestrate_trivial_skips_ci_reviewer_and_build_keeps_merge_and_pull() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate_with_lifecycle_skip(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip::from_tags(["lifecycle:trivial"]),
            false,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer, Phase::Merge, Phase::Pull,],
            "trivial skips the ci-wait, reviewer, and build phases; merge + pull \
             are substrate-integrity phases and must not be skipped"
        );
    }

    // trace:TASK-907 — an express-tier spec (`batch:express`) drained by the
    // autonomous loop runs the FULL six-phase pipeline — implementer → CI →
    // reviewer → merge → pull → build. Express is fast because reliably
    // *routed*, not because it is less gated, so no phase is skipped.
    #[test]
    fn orchestrate_express_tier_runs_full_pipeline() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate_with_lifecycle_skip(
            &mut driver,
            "TASK-907",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip::from_tags([EXPRESS_TIER_TAG]),
            false,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ],
            "express runs the full gate — CI + reviewer + build all execute"
        );
    }

    // trace:TASK-907 trust contract: a spec that is on the express tier AND
    // somehow also carries a `lifecycle:no-review` tag still runs the reviewer —
    // the express override means the gate is never silently downgraded.
    #[test]
    fn orchestrate_express_overrides_lifecycle_skip_keeps_reviewer() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate_with_lifecycle_skip(
            &mut driver,
            "TASK-907",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip::from_tags(["lifecycle:no-review", EXPRESS_TIER_TAG]),
            false,
        );
        assert_eq!(result.exit_code, 0);
        assert!(
            driver.calls.contains(&Phase::Reviewer),
            "express overrides lifecycle:no-review — reviewer must still run"
        );
    }

    #[test]
    fn orchestrate_no_ci_wait_skips_ci_keeps_rest() {
        // Review finding: lifecycle:no-ci-wait must skip the blocking CI phase
        // (CI still runs remotely) while every other phase proceeds.
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate_with_lifecycle_skip(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip {
                no_ci_wait: true,
                ..LifecycleSkip::none()
            },
            false,
        );
        assert_eq!(result.exit_code, 0);
        assert!(
            !driver.calls.contains(&Phase::Ci),
            "no-ci-wait must skip the CI phase: {:?}",
            driver.calls
        );
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ],
            "only the ci-wait phase is skipped"
        );
    }

    #[test]
    fn orchestrate_no_review_unreachable_reviewer_escalation_does_not_fire() {
        let mut driver = MockPhaseDriver::reviewer_escalates_merge("would have escalated");
        let result = orchestrate_with_lifecycle_skip(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip {
                no_review: true,
                ..LifecycleSkip::none()
            },
            false,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.escalation.is_none());
        assert!(!driver.calls.contains(&Phase::Reviewer));
    }

    // --- STORY-306: the headless advisor tier -----------------------------

    /// A punt the advisor is confident about: it resolves the fork, the
    /// implementer resumes and opens a PR, and the full pipeline runs. The
    /// run is a clean success — neither an escalation nor a punt.
    #[test]
    fn orchestrate_punt_advisor_resolves_then_resume_runs_full_pipeline() {
        let mut driver = MockPhaseDriver::punting_at_implementer("auth flow fork")
            .advisor_resolves("use OAuth — the recorded convention")
            .resume_opens_pr();
        let result = orchestrate(
            &mut driver,
            "STORY-306",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(
            result.escalation.is_none(),
            "a resolved punt is not an escalation"
        );
        assert!(
            result.punt_reason.is_none(),
            "a resolved-and-resumed punt is not a punt"
        );
        // All six phases ran — the resumed implementer's PR flows through CI.
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ]
        );
        assert_eq!(driver.advisor_calls, 1);
    }

    #[test]
    fn orchestrate_text_question_punt_routes_to_advisor_not_phase1_failure() {
        let mut driver = MockPhaseDriver::punting_at_implementer(
            "headless implementer exited without opening a PR after asking: Confirm and I'll proceed?",
        )
        .advisor_resolves("proceed with option A")
        .resume_opens_pr();
        let result = orchestrate(
            &mut driver,
            "BUG-374",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.failure.is_none(), "text-question punt is not NoPr");
        assert_eq!(driver.advisor_calls, 1);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ]
        );
    }

    /// A punt the advisor cannot safely judge, under `--escalate-blocks`: the
    /// advisor escalates, the run stops clean after phase 1 — phases 2-6 never
    /// run — exits `0`, and the result carries the escalation.
    ///
    /// TASK-358: also asserts the orchestrator stamps the implementer's lease
    /// (`mark_implementer_lease_escalated`) exactly once on this path — the
    /// marker is what `aida edit --status` out of `NeedsAttention` and
    /// `aida session prune --escalations` later use to clean up the
    /// otherwise-lingering worktree. trace:TASK-358 | ai:claude
    #[test]
    fn orchestrate_punt_advisor_escalates_blocks_skips_phases_2_to_6() {
        let mut driver = MockPhaseDriver::punting_at_implementer("project-strategy fork")
            .advisor_escalates("a strategy call with no recorded principle");
        let result = orchestrate(
            &mut driver,
            "STORY-306",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0, "an escalation is a clean exit");
        assert_eq!(result.failed_phase, None);
        let escalation = result.escalation.expect("escalation recorded");
        assert_eq!(escalation.kind, EscalationKind::DesignFork);
        // The pipeline stopped at phase 1 — only the implementer ran.
        assert_eq!(driver.calls, vec![Phase::Implementer]);
        assert_eq!(driver.advisor_calls, 1);
        // TASK-358: the lease was stamped escalated_to_human exactly once.
        assert_eq!(
            driver.mark_escalated_calls, 1,
            "--escalate-blocks must mark the lease so the lingering worktree gets cleaned"
        );
    }

    /// BUG-770 regression: driving the escalation path from a TEST driver must
    /// not append to a REAL project's `.aida/events.jsonl`.
    ///
    /// The bug: `finish_escalated` resolved the project root itself with
    /// `find_project_root()`, which — because `cargo test` runs with the repo
    /// as cwd — resolved to the developer's own clone. Every `cargo test` run
    /// therefore appended synthetic `AdvisorEscalated` lines to the live drain
    /// event stream, for months, and every history consumer (`aida watch
    /// --backlog`, `event_wait`, integrate_view) saw fabricated escalations.
    ///
    /// The assertion is scoped to a marker string unique to this fixture, so
    /// it stays honest under `--test-threads=N` (a sibling test emitting to
    /// its own temp root can never move this count) while still failing loudly
    /// if the resolve-root-from-cwd form ever comes back.
    // trace:BUG-770 | ai:claude
    #[test]
    fn orchestrate_escalation_never_writes_to_the_ambient_project_event_stream() {
        const MARKER: &str = "BUG-770 probe fork — synthetic, must never reach a real stream";
        let ambient = crate::find_project_root().expect("cargo test runs inside a git checkout");
        let stream = crate::events::events_path(&ambient);
        let marker_lines = || {
            std::fs::read_to_string(&stream)
                .map(|body| body.lines().filter(|l| l.contains(MARKER)).count())
                .unwrap_or(0)
        };
        assert_eq!(marker_lines(), 0, "fixture marker must not pre-exist");

        let mut driver = MockPhaseDriver::punting_at_implementer("project-strategy fork")
            .advisor_escalates(MARKER);
        let result = orchestrate(
            &mut driver,
            "STORY-306",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        // The fixture really did take the emitting path.
        assert!(
            result.escalation.is_some(),
            "fixture must actually escalate, or it proves nothing"
        );
        // A test driver keeps the fail-safe default: no event root, so the
        // epilogue has nowhere to write.
        assert!(
            driver.events_root().is_none(),
            "test drivers must not name a real project as their event root"
        );
        assert_eq!(
            marker_lines(),
            0,
            "the test suite appended a synthetic escalation to {} — BUG-770 regressed",
            stream.display()
        );
    }

    /// A punt the advisor escalates, under `--escalate-defaults`: instead of
    /// stopping, the implementer is resumed with the defensible-default
    /// instruction, opens a PR, and the full pipeline runs.
    ///
    /// TASK-358: also asserts the orchestrator does NOT stamp the lease as
    /// escalated_to_human on this path — the resume needs the worktree alive,
    /// and a stray marker would have a later triage out of NeedsAttention
    /// (the implementer's own `aida edit --status in-progress` on resume)
    /// nuke the worktree under its feet. trace:TASK-358 | ai:claude
    #[test]
    fn orchestrate_punt_advisor_escalates_defaults_resumes_with_default() {
        let mut driver = MockPhaseDriver::punting_at_implementer("flag-naming fork")
            .advisor_escalates("no recorded flag convention")
            .resume_opens_pr();
        let result = orchestrate(
            &mut driver,
            "STORY-306",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Defaults,
        );
        assert_eq!(result.exit_code, 0);
        // `--escalate-defaults` resumes rather than stopping → a full ship.
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ]
        );
        assert_eq!(driver.advisor_calls, 1);
        // TASK-358: the lease must stay un-stamped so the resume's worktree
        // is preserved — the resume is what consumes it.
        assert_eq!(
            driver.mark_escalated_calls, 0,
            "--escalate-defaults must NOT mark the lease — the resume relies on its worktree"
        );
    }

    /// A re-punt after a resume is terminal — one advisor round per spec per
    /// drain. The resumed implementer punts again; the run ends via
    /// `finish_punted` (exit `0`, `punt_reason` set) and the advisor is
    /// spawned exactly once.
    #[test]
    fn orchestrate_advisor_resume_repunt_is_terminal() {
        let mut driver = MockPhaseDriver::punting_at_implementer("first fork")
            .advisor_resolves("answer A")
            .resume_repunts("a fresh fork the answer surfaced");
        let result = orchestrate(
            &mut driver,
            "STORY-306",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0, "a re-punt is a clean exit");
        assert_eq!(
            result.punt_reason.as_deref(),
            Some("a fresh fork the answer surfaced"),
        );
        // Phases 2-6 never ran; the advisor was spawned exactly once.
        assert_eq!(driver.calls, vec![Phase::Implementer]);
        assert_eq!(driver.advisor_calls, 1, "no second advisor round");
    }

    /// `EscalateMode::from_flags` — `--escalate-defaults` set ⇒ `Defaults`;
    /// absent ⇒ the conservative `Blocks` default.
    #[test]
    fn escalate_mode_from_flags_defaults_to_blocks() {
        assert_eq!(EscalateMode::from_flags(false), EscalateMode::Blocks);
        assert_eq!(EscalateMode::from_flags(true), EscalateMode::Defaults);
    }

    // --- BUG-257: Inconclusive phase-1 outcome ---------------------------
    //
    // A transient GH-API network error during the post-implementer PR lookup
    // must NOT be reported as a phase-1 failure (that gave the operator a
    // wrong recovery hint and crashed batch drains on a network blip). It is
    // a first-class non-failure outcome: the run exits 0, no `failed_phase`,
    // `inconclusive_reason` is set, and no later phase runs.

    /// Acceptance #4 — "Test with a mocked API failure: orchestrator reports
    /// `Inconclusive`, drain pauses (not fails)." trace:BUG-257 | ai:claude
    #[test]
    fn orchestrate_inconclusive_at_phase1_is_not_a_failure() {
        let mut driver = MockPhaseDriver::inconclusive_at_implementer(
            "GH API unreachable (error connecting to api.github.com); \
             branch `bug-257` is on origin so a PR may exist",
        );
        let result = orchestrate(
            &mut driver,
            "BUG-257",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(
            result.exit_code, 0,
            "an inconclusive run is a clean exit, not a failure"
        );
        assert!(
            result.failed_phase.is_none(),
            "no `failed_phase` — the drain paused, nothing crashed",
        );
        assert!(result.failure.is_none(), "no `PhaseFailure` payload");
        assert!(
            result.inconclusive_reason.is_some(),
            "`inconclusive_reason` distinguishes this from a clean ship",
        );
        assert!(result.punt_reason.is_none(), "not a punt");
        assert!(result.escalation.is_none(), "not an escalation");
        assert!(result.shipped_spec_id.is_none(), "nothing shipped");
        // Phases 2-6 never ran — the drain paused at phase 1.
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    // --- TASK-136: inconclusive → shelve-and-advance in a batch drain -----

    /// TASK-136 acceptance: a phase-1 Inconclusive in BATCH mode shelves the
    /// spec (stamps `shelved_reason`, exit = phase-1 index) so `drain_batch`'s
    /// EPIC-28 shelve→advance path carries it — the whole batch does not pause
    /// at the head. trace:TASK-136 | ai:claude
    #[test]
    fn inconclusive_after_retries_shelves_and_advances_in_batch() {
        let mut driver =
            MockPhaseDriver::inconclusive_and_shelves("GH unreachable after the retry backoff");
        let result = orchestrate_with_lifecycle_skip(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip::none(),
            true, // batch
        );
        let fr = result
            .shelved_reason
            .as_ref()
            .expect("a batch-mode inconclusive must shelve, not pause");
        assert_eq!(
            fr.kind, "environmental",
            "the shelve records the closed environmental cause for triage"
        );
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert_eq!(
            result.exit_code,
            Phase::Implementer.index(),
            "a shelve keeps the failed-phase exit code so drain_batch routes it",
        );
        assert!(
            result.inconclusive_reason.is_none(),
            "in batch mode it is shelved, NOT left as an inconclusive pause",
        );
        // Phases 2-6 never ran.
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    /// TASK-136: the single-spec path is UNCHANGED — the same driver pauses
    /// (exit 0, `inconclusive_reason` set, nothing shelved). The `batch` flag,
    /// not the driver, picks the behaviour. trace:TASK-136 | ai:claude
    #[test]
    fn single_spec_inconclusive_still_pauses() {
        let mut driver = MockPhaseDriver::inconclusive_and_shelves("GH unreachable");
        let result = orchestrate_with_lifecycle_skip(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip::none(),
            false, // single-spec
        );
        assert_eq!(
            result.exit_code, 0,
            "single-spec inconclusive is a clean pause"
        );
        assert!(
            result.inconclusive_reason.is_some(),
            "single-spec keeps the historical Inconclusive pause",
        );
        assert!(
            result.shelved_reason.is_none(),
            "single-spec never shelves an inconclusive",
        );
    }

    // --- STORY-731: single-spec shelve failure reads as parked-not-crashed --

    /// STORY-731 acceptance: a SHELVED single-spec drive failure (a recoverable
    /// phase failure that parked the spec in Needs Attention) renders the
    /// parity tail — it names the spec PARKED (not crashed), tells the operator
    /// to fix-then-re-drive, and points at BOTH recovery commands (`aida why`
    /// for what failed, `aida findings list` for all parked work). This is the
    /// legibility the batch drain already had; the single-spec path now matches.
    #[test]
    fn single_spec_shelve_failure_names_parked_and_recovery_commands() {
        let lines = single_spec_failure_tail("TASK-1", Phase::Ci, 4200, true);
        let joined = lines.join("\n");
        assert!(
            joined.contains("parked for triage"),
            "a shelve must name itself parked-for-triage, not a crash: {joined}"
        );
        assert!(
            joined.contains("not a crash"),
            "a shelve must explicitly say it is not a crash: {joined}"
        );
        assert!(
            joined.contains("re-drive"),
            "a shelve must tell the operator to fix-then-re-drive: {joined}"
        );
        assert!(
            joined.contains("aida why TASK-1"),
            "a shelve must point at `aida why <spec>` for what failed: {joined}"
        );
        assert!(
            joined.contains("aida findings list"),
            "a shelve must point at `aida findings list` for all parked work: {joined}"
        );
        assert!(
            !joined.contains("auto-complete failed"),
            "a shelve must NOT use the terminal crash wording: {joined}"
        );
    }

    /// STORY-731: a genuine hard crash (a NON-shelvable failure — Spawn /
    /// MissingTool / Internal — that never parked the spec) still reads as a
    /// crash: the terminal "auto-complete failed at phase N" line, and NONE of
    /// the parked/recovery language. The parity tail must not soften a real
    /// crash into a recoverable-looking park.
    #[test]
    fn single_spec_hard_crash_still_reads_as_a_crash() {
        let lines = single_spec_failure_tail("TASK-1", Phase::Implementer, 1500, false);
        let joined = lines.join("\n");
        assert!(
            joined.contains("auto-complete failed"),
            "a hard crash keeps the terminal crash wording: {joined}"
        );
        assert!(
            !joined.contains("parked"),
            "a hard crash must NOT be mislabelled as parked: {joined}"
        );
        assert!(
            !joined.contains("aida findings list"),
            "a hard crash is not triageable parked work — no findings-list pointer: {joined}"
        );
    }

    // --- STORY-492: resume re-entry skips already-complete phases ----------

    /// STORY-492: `start_phase = Implementer` (the normal entry) runs the full
    /// pipeline — resume is a strict superset of the historical behaviour.
    #[test]
    fn resume_at_implementer_runs_the_full_pipeline() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate_with_resume(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip::none(),
            false,
            Phase::Implementer,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build
            ]
        );
    }

    /// STORY-492: re-entering at Merge skips implementer / CI / reviewer — their
    /// effects already exist (the resume handler seeded branch + PR) — and runs
    /// merge → pull → build. The implementer is NEVER re-run, so no PR is
    /// re-opened. trace:STORY-492 | ai:claude
    #[test]
    fn resume_at_merge_skips_implementer_ci_reviewer() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate_with_resume(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip::none(),
            false,
            Phase::Merge,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            driver.calls,
            vec![Phase::Merge, Phase::Pull, Phase::Build],
            "resume at merge re-runs only merge/pull/build",
        );
    }

    /// STORY-492: re-entering at Pull (crashed after the merge landed but before
    /// the auto-bump) runs only pull → build — never re-merges. trace:STORY-492
    #[test]
    fn resume_at_pull_runs_only_pull_and_build() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate_with_resume(
            &mut driver,
            "TASK-1",
            AutoCompleteVariant::Full,
            true,
            EscalateMode::Blocks,
            LifecycleSkip::none(),
            false,
            Phase::Pull,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(driver.calls, vec![Phase::Pull, Phase::Build]);
    }

    // --- BUG-250: deliberate PR-hold phase-1 outcome ---------------------
    //
    // A deliberate push-branch, hold-PR finish must NOT be reported as a
    // phase-1 failure (that gave a wrong recovery hint — "run /aida-pr" —
    // which for a deliberate hold would ship un-gated code). It is a clean
    // non-failure stop: exit 0, no `failed_phase`, `held_reason` set, no later
    // phase runs.

    /// BUG-250 acceptance #1/#2/#4 — a held PR reports `Held`, not a phase-1
    /// failure; the drain halts cleanly at phase 1. trace:BUG-250 | ai:claude
    #[test]
    fn orchestrate_held_at_phase1_is_not_a_failure() {
        let mut driver =
            MockPhaseDriver::holding_at_implementer("SPIKE-7 smoke must pass before merge");
        let result = orchestrate(
            &mut driver,
            "STORY-306",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(
            result.exit_code, 0,
            "a deliberate hold is a clean exit, not a failure"
        );
        assert!(
            result.failed_phase.is_none(),
            "no `failed_phase` — the PR was deliberately held, nothing failed",
        );
        assert!(result.failure.is_none(), "no `PhaseFailure` payload");
        assert!(
            result.held_reason.is_some(),
            "`held_reason` distinguishes a deliberate hold from a clean ship",
        );
        assert!(result.punt_reason.is_none(), "not a punt");
        assert!(result.inconclusive_reason.is_none(), "not inconclusive");
        assert!(result.escalation.is_none(), "not an escalation");
        assert!(result.shipped_spec_id.is_none(), "nothing shipped");
        // Phases 2-6 never ran — the drain halted at phase 1.
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    /// `BatchDrainOutcome::Held` — a batch member that deliberately holds its
    /// PR pauses the drain at that spec (does not falsely count it shipped),
    /// exits 0. trace:BUG-250 | ai:claude
    #[test]
    fn drain_batch_held_pauses_without_shipping() {
        struct OneHeldDriver {
            queue: Vec<String>,
            ran: Vec<String>,
        }
        impl BatchDriver for OneHeldDriver {
            fn next_head(&mut self) -> Option<String> {
                self.queue.first().cloned()
            }
            fn stop_requested(&mut self) -> bool {
                false
            }
            fn run_spec(&mut self, spec: &str) -> OrchestrationResult {
                self.ran.push(spec.to_string());
                OrchestrationResult {
                    exit_code: 0,
                    failed_phase: None,
                    failure: None,
                    phase_durations: Vec::new(),
                    total_ms: 0,
                    punt_reason: None,
                    shipped_spec_id: None,
                    escalation: None,
                    inconclusive_reason: None,
                    shelved_reason: None,
                    held_reason: Some("PR held on `story-306` — SPIKE-7 smoke".to_string()),
                }
            }
        }
        let mut driver = OneHeldDriver {
            queue: vec!["STORY-306".to_string(), "STORY-307".to_string()],
            ran: Vec::new(),
        };
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.outcome, BatchDrainOutcome::Held);
        assert_eq!(result.exit_code, 0, "a deliberate hold is not a failure");
        assert!(result.shipped.is_empty(), "the held spec did not ship");
        assert_eq!(result.stopped_at.as_deref(), Some("STORY-306"));
        // The drain paused at the held head — the next member never ran.
        assert_eq!(driver.ran, vec!["STORY-306".to_string()]);
    }

    /// `BatchDrainOutcome::Inconclusive` — a batch drain that hits a phase-1
    /// inconclusive run stops at that spec without claiming ship / punt /
    /// fail, leaves the head un-advanced, and exits 0 so the next drain
    /// retries once the API is reachable. trace:BUG-257 | ai:claude
    #[test]
    fn drain_batch_inconclusive_pauses_without_advancing() {
        struct OneInconclusiveDriver {
            queue: Vec<String>,
            ran: Vec<String>,
        }
        impl BatchDriver for OneInconclusiveDriver {
            fn next_head(&mut self) -> Option<String> {
                self.queue.first().cloned()
            }
            fn stop_requested(&mut self) -> bool {
                false
            }
            fn run_spec(&mut self, spec: &str) -> OrchestrationResult {
                self.ran.push(spec.to_string());
                OrchestrationResult {
                    exit_code: 0,
                    failed_phase: None,
                    failure: None,
                    phase_durations: Vec::new(),
                    total_ms: 0,
                    punt_reason: None,
                    shipped_spec_id: None,
                    escalation: None,
                    inconclusive_reason: Some("GH API unreachable — cannot confirm PR".to_string()),
                    shelved_reason: None,
                    held_reason: None,
                }
            }
        }
        let mut driver = OneInconclusiveDriver {
            queue: vec!["BUG-257".to_string(), "TASK-260".to_string()],
            ran: Vec::new(),
        };
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 0, "inconclusive is a clean exit");
        assert_eq!(result.outcome, BatchDrainOutcome::Inconclusive);
        assert_eq!(result.stopped_at.as_deref(), Some("BUG-257"));
        assert!(result.shipped.is_empty(), "the spec did not ship");
        assert!(result.punted.is_empty(), "the spec did not punt");
        assert!(result.escalated.is_empty(), "the spec did not escalate");
        // The head was not advanced — only the inconclusive spec was driven.
        // The queued TASK-260 stays for the next drain.
        assert_eq!(driver.ran, vec!["BUG-257"]);
    }

    /// BUG-266: when the orchestrator-level Inconclusive carries a leg-
    /// specific retry hint (the Anthropic-API path), the hint flows through
    /// the variant and replaces BUG-257's default GH-flavored hint. This is
    /// the only spot that exercises the new `retry_hint` field at the
    /// orchestrator boundary — the classifier itself is unit-tested in
    /// `main.rs::bug_266_anthropic_api_outage_classifier_tests`.
    /// trace:BUG-266 | ai:claude
    #[test]
    fn inconclusive_with_anthropic_hint_overrides_default() {
        let mut driver = MockPhaseDriver::base();
        driver.inconclusive = Some(
            "Anthropic API outage during the headless implementer: API Error: 529 Overloaded"
                .to_string(),
        );
        // The mock returns `retry_hint: None`; we explicitly run a variant
        // that simulates the production wiring by re-driving through
        // `finish_inconclusive` with `Some(hint)` and asserting the round
        // trip on `OrchestrationResult`. Going via `orchestrate` keeps the
        // assertion at the orchestrator boundary instead of staring at the
        // private epilogue function.
        let result = orchestrate(
            &mut driver,
            "BUG-266",
            AutoCompleteVariant::Full,
            true, // json — keeps the test output clean
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.failed_phase.is_none());
        assert!(
            result
                .inconclusive_reason
                .as_deref()
                .map(|r| r.contains("Anthropic API outage"))
                .unwrap_or(false),
            "BUG-266 reason must surface; got: {:?}",
            result.inconclusive_reason,
        );
        // Only phase 1 ran — the Inconclusive path halts cleanly without
        // touching CI / reviewer / merge.
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    /// A re-punt on the resumed implementer is terminal; symmetrically, an
    /// inconclusive on the resumed implementer is also terminal — the drain
    /// pauses, no second advisor round. trace:BUG-257 | ai:claude
    #[test]
    fn orchestrate_resume_inconclusive_is_terminal_pause() {
        let mut driver =
            MockPhaseDriver::punting_at_implementer("first fork").advisor_resolves("answer A");
        driver.resume = Some(ImplementerOutcome::Inconclusive {
            reason: "GH API unreachable after the implementer resume".to_string(),
            retry_hint: None,
        });
        let result = orchestrate(
            &mut driver,
            "BUG-257",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.failed_phase.is_none());
        assert!(result.inconclusive_reason.is_some());
        assert!(result.punt_reason.is_none());
        // Phases 2-6 never ran; the advisor was spawned exactly once.
        assert_eq!(driver.calls, vec![Phase::Implementer]);
        assert_eq!(driver.advisor_calls, 1, "no second advisor round");
    }

    // --- Variant gating ---------------------------------------------------

    #[test]
    fn orchestrate_through_ci_stops_after_phase_2() {
        // Even with a driver that would fail at phase 3, through-ci never
        // reaches it.
        let mut driver = MockPhaseDriver::failing_at(Phase::Reviewer);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::ThroughCi,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 0);
        assert_eq!(driver.calls, vec![Phase::Implementer, Phase::Ci]);
    }

    #[test]
    fn orchestrate_through_merge_stops_after_phase_4() {
        let mut driver = MockPhaseDriver::all_ok();
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::ThroughMerge,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 0);
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer, Phase::Ci, Phase::Reviewer, Phase::Merge,]
        );
    }

    #[test]
    fn orchestrate_skip_build_stops_after_phase_5() {
        let mut driver = MockPhaseDriver::all_ok();
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::SkipBuild,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 0);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
            ]
        );
    }

    // --- Failure injection: each phase → its exit code --------------------

    #[test]
    fn failure_injection_implementer_exits_1() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Implementer);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 1);
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    #[test]
    fn failure_injection_reviewer_exits_3() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Reviewer);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 3);
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer, Phase::Ci, Phase::Reviewer]
        );
    }

    #[test]
    fn failure_injection_merge_exits_4() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Merge);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 4);
    }

    #[test]
    fn failure_injection_pull_exits_5() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Pull);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 5);
    }

    #[test]
    fn failure_injection_build_exits_6() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Build);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 6);
    }

    // --- CI-failure injection: red CI stops at phase 2 --------------------

    #[test]
    fn ci_red_stops_at_phase_2() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Ci);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 2);
        // The reviewer phase must NOT have run.
        assert_eq!(driver.calls, vec![Phase::Implementer, Phase::Ci]);
    }

    // --- Reviewer verdict gating ------------------------------------------

    #[test]
    fn reviewer_phase_refuses_missing_pr_before_launch() {
        let mut driver = MockPhaseDriver::all_ok().with_pr_number(None);
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 3);
        assert_eq!(result.failed_phase, Some(Phase::Reviewer));
        assert_eq!(
            result.failure.as_ref().map(|f| f.kind),
            Some(FailureKind::NoPr)
        );
        assert_eq!(driver.calls, vec![Phase::Implementer, Phase::Ci]);
    }

    #[test]
    fn reviewer_phase_recovers_missing_pr_before_launch() {
        let mut driver = MockPhaseDriver::all_ok()
            .with_pr_number(None)
            .recovering_review_pr(77);
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.failed_phase, None);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ]
        );
        assert_eq!(driver.pr_number, Some(77));
    }

    #[test]
    fn reviewer_phase_refuses_pr_zero_before_launch() {
        let mut driver = MockPhaseDriver::all_ok().with_pr_number(Some(0));
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 3);
        assert_eq!(result.failed_phase, Some(Phase::Reviewer));
        assert_eq!(
            result.failure.as_ref().map(|f| f.kind),
            Some(FailureKind::NoPr)
        );
        assert_eq!(driver.calls, vec![Phase::Implementer, Phase::Ci]);
    }

    #[test]
    fn reviewer_phase_missing_pr_spends_transient_retry_budget_before_parking() {
        let mut driver = MockPhaseDriver::all_ok().with_pr_number(None);
        driver.transient_retry_budget = 1;
        driver.shelve_succeeds = true;

        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );

        assert_eq!(result.exit_code, 3);
        assert_eq!(result.failed_phase, Some(Phase::Reviewer));
        let failure = result.failure.as_ref().expect("failure recorded");
        assert_eq!(failure.kind, FailureKind::NoPr);
        assert!(failure.reason.contains("attempt 2/2"), "{}", failure.reason);
        assert_eq!(driver.calls, vec![Phase::Implementer, Phase::Ci]);
        assert_eq!(
            driver.transient_retry_events,
            vec![(
                Phase::Reviewer,
                "no-pr".to_string(),
                2,
                2,
                Some(
                    "no open PR resolved before the reviewer phase — refusing to launch \
                     reviewer for PR-0"
                        .to_string()
                ),
            )]
        );
    }

    #[test]
    fn reviewer_rejected_stops_at_phase_3() {
        let mut driver = MockPhaseDriver::with_verdict(Verdict::Rejected);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 3);
        // Reviewer ran; merge did not.
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer, Phase::Ci, Phase::Reviewer]
        );
    }

    #[test]
    fn reviewer_request_changes_stops_at_phase_3() {
        let mut driver = MockPhaseDriver::with_verdict(Verdict::RequestChanges);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 3);
    }

    #[test]
    fn reviewer_approved_proceeds_to_merge() {
        let mut driver = MockPhaseDriver::with_verdict(Verdict::Approved);
        let code = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        )
        .exit_code;
        assert_eq!(code, 0);
    }

    // --- STORY-306: reviewer escalates the merge decision to a human ------

    /// The reviewer escalates the merge decision rather than auto-deciding
    /// it. `orchestrate` stops cleanly after phase 3 — no merge / pull /
    /// build — exits `0`, and the result carries the escalation so the run
    /// is mistaken for neither a clean ship nor a failure.
    #[test]
    fn orchestrate_reviewer_escalated_to_human_skips_merge() {
        let mut driver = MockPhaseDriver::reviewer_escalates_merge(
            "uncertain whether the zen run was corroborated — a human should merge",
        );
        let result = orchestrate(
            &mut driver,
            "BUG-241",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0, "an escalation is a clean exit");
        assert_eq!(result.failed_phase, None);
        assert!(result.failure.is_none());
        let escalation = result.escalation.expect("escalation recorded");
        assert_eq!(escalation.kind, EscalationKind::MergeDecision);
        assert!(
            escalation.reason.contains("zen run"),
            "{}",
            escalation.reason
        );
        // The pipeline stopped at phase 3 — merge / pull / build never ran.
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer, Phase::Ci, Phase::Reviewer]
        );
    }

    /// Regression guard — an escalation is a *distinct* outcome from a
    /// non-Approved verdict. A plain `RequestChanges` verdict still routes
    /// through the phase-3 failure path (exit `3`), unchanged.
    #[test]
    fn orchestrate_reviewer_request_changes_still_fails() {
        let mut driver = MockPhaseDriver::with_verdict(Verdict::RequestChanges);
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 3);
        assert_eq!(result.failed_phase, Some(Phase::Reviewer));
        assert!(result.escalation.is_none());
    }

    // --- BUG-727: the substrate supervised-merge gate ---------------------

    /// A spec whose execution mode holds the merge (anything but `drain`, or
    /// no mode at all) must NOT be auto-merged: the run stops cleanly right
    /// before phase 4 — exit `0`, no merge / pull / build — and the result
    /// carries the SupervisedMerge escalation, so the PR is left mergeable
    /// for the human/advisor instead of merged past the marker.
    #[test]
    fn orchestrate_supervised_spec_holds_merge() {
        let mut driver = MockPhaseDriver::merge_supervised(
            "TASK-1080 is marked drive — auto-merge refused; merge requires \
             human/advisor review",
        );
        let result = orchestrate(
            &mut driver,
            "TASK-1080",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0, "a supervised hold is a clean exit");
        assert_eq!(result.failed_phase, None);
        assert!(result.failure.is_none());
        let escalation = result.escalation.expect("escalation recorded");
        assert_eq!(escalation.kind, EscalationKind::SupervisedMerge);
        assert!(
            escalation.reason.contains("auto-merge refused"),
            "{}",
            escalation.reason
        );
        assert_eq!(
            driver.recorded_merge_holds,
            vec![escalation.reason],
            "supervised clean-exit must publish the durable merge fence"
        );
        // The pipeline stopped before phase 4 — merge / pull / build never ran.
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer, Phase::Ci, Phase::Reviewer]
        );
    }

    /// Regression guard — a spec with no supervision hold (execution mode
    /// `drain`) preserves the current auto-merge: all six phases run.
    #[test]
    fn orchestrate_unsupervised_spec_still_auto_merges() {
        let mut driver = MockPhaseDriver::base();
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.escalation.is_none());
        assert!(
            driver.calls.contains(&Phase::Merge),
            "the drain-mode merge must be unaffected: {:?}",
            driver.calls
        );
    }

    // --- BUG-241: reconcile against reality before declaring a failure ----

    /// Instance B — phase 1 produces no PR because the spec was already
    /// resolved by supersession. The reconcile step finds the spec Completed
    /// and treats the run as a success: exit `0`, no `failed_phase`. A batch
    /// drain advances on exactly this result shape (`drain_batch` keys off
    /// `exit_code` / `failed_phase`), so the false "shipped 0" crash is gone.
    #[test]
    fn orchestrate_reconciles_phase1_no_work_spec() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Implementer).reconciles_as(
            PhaseReconcile::ShippedOutOfBand {
                reason: "BUG-230 is Completed — the spec needed no further work".to_string(),
            },
        );
        let result = orchestrate(
            &mut driver,
            "BUG-230",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(
            result.exit_code, 0,
            "a reconciled phase-1 failure is a success"
        );
        assert!(result.failed_phase.is_none());
        assert!(result.failure.is_none());
        // The pipeline still stopped at phase 1 — reconcile redeems the
        // outcome, it does not resume the remaining phases.
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    /// BUG-1145: a phase-1 child can commit/push/open a PR and then exit in a
    /// way that prevents the parent from matching the lease. Once the driver
    /// verifies an open PR, the drain must continue through the PR path instead
    /// of retrying phase 1 (which collides with the already-Done spec) or
    /// shelving the member — AND it must re-enter at Ci (phase 2), not Reviewer,
    /// so the CI gate still runs before merge (advisor review: main has no
    /// branch protection and merge() does not gate CI, so skipping phase 2 would
    /// merge red CI). The real driver returns Phase::Ci; this asserts the Ci
    /// phase is in the executed sequence.
    #[test]
    fn orchestrate_phase1_failure_with_verified_pr_proceeds_from_ci() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Implementer)
            .recovering_phase1_failure_from_pr(Phase::Ci);
        let result = orchestrate(
            &mut driver,
            "BUG-1145",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.failed_phase.is_none());
        assert!(result.shelved_reason.is_none());
        // Ci is present → the CI gate runs before reviewer/merge.
        assert!(driver.calls.contains(&Phase::Ci));
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ]
        );
    }

    /// BUG-1524: a phase-1 LAUNCH REFUSAL (no implementer session ever
    /// started — a preflight/lease refusal or an agent-binary launch
    /// failure) must never be conflated with the BUG-1145 "launched, then
    /// failed after opening/advancing a PR" case above. Even when an open
    /// PR from a PREVIOUS round exists and the mock is configured to
    /// "recover" through it, a `LaunchRefused` failure must shelve
    /// immediately and never reach Ci or Reviewer — continuing would
    /// manufacture CI/review activity against a head this round never
    /// touched.
    // trace:BUG-1524 | ai:claude
    #[test]
    fn launch_refusal_shelves_and_never_reaches_ci_or_reviewer() {
        let mut driver =
            MockPhaseDriver::failing_at_with_kind(Phase::Implementer, FailureKind::LaunchRefused)
                .recovering_phase1_failure_from_pr(Phase::Ci);
        driver.shelve_succeeds = true;
        // BUG-1629: a full transient budget must still not retry a refused
        // launch; `calls` below proves a single implementer run.
        // trace:BUG-1629 | ai:claude
        driver.transient_retry_budget = 3;
        let result = orchestrate(
            &mut driver,
            "BUG-1524",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert_eq!(
            result.failure.as_ref().map(|f| f.kind),
            Some(FailureKind::LaunchRefused)
        );
        assert!(result.shelved_reason.is_some());
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer],
            "a launch refusal must never advance to Ci/Reviewer, even when an \
             already-open PR from a previous round would otherwise redeem it"
        );
    }

    /// BUG-1524 review fix: a lease conflict is a pre-launch refusal too and
    /// must never advance to Ci/Reviewer through a previous round's open PR.
    // trace:BUG-1524 | ai:claude
    #[test]
    fn lease_conflict_never_recovers_through_an_open_pr() {
        let mut driver =
            MockPhaseDriver::failing_at_with_kind(Phase::Implementer, FailureKind::LeaseConflict)
                .recovering_phase1_failure_from_pr(Phase::Ci);
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "BUG-1524",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer],
            "a lease conflict must never advance to Ci/Reviewer via open-PR recovery"
        );
    }

    /// BUG-1527 review fix: a branch swap onto another spec's work shelves
    /// and never reaches open-PR recovery (which could credit the wrong PR).
    // trace:BUG-1527 | ai:claude
    #[test]
    fn shipped_mismatch_never_recovers_through_an_open_pr() {
        let mut driver =
            MockPhaseDriver::failing_at_with_kind(Phase::Implementer, FailureKind::ShippedMismatch)
                .recovering_phase1_failure_from_pr(Phase::Ci);
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "BUG-1527",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert_eq!(
            result.failure.as_ref().map(|f| f.kind),
            Some(FailureKind::ShippedMismatch)
        );
        assert!(result.shelved_reason.is_some(), "a swap must shelve");
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer],
            "a swap must never advance to Ci/Reviewer via open-PR recovery"
        );
    }

    /// BUG-1527 AC6: a phase-1 outcome whose ending branch differs from the
    /// dispatched branch (`ShippedMismatch`) must never let the dispatched
    /// spec's status change. `MockPhaseDriver` has no store of its own —
    /// the only status-adjacent driver hooks are a later phase's success
    /// path (`finish_success`, the only path resembling a Done-adjacent
    /// write) and the escalation stamp — so "status stays unchanged" is
    /// observed here as: no phase past Implementer ever runs, nothing gets
    /// credited (`shipped_spec_id` stays `None`, so `finish_success` never
    /// fires), and the escalation hook is never touched either. (BUG-1291's
    /// `handoff_open_pr_after_shelve` is deliberately NOT asserted here: the
    /// orchestrator calls it after every successful shelve regardless of
    /// phase, and `MockPhaseDriver` records that call unconditionally too —
    /// its real implementation is the one that gates on `pr_number`, and it
    /// hands an ALREADY-OPEN PR to review, not a Done write, so it is
    /// orthogonal to this spec's status either way.)
    // trace:BUG-1527 trace:TASK-1457 | ai:claude
    #[test]
    fn shipped_mismatch_never_changes_the_dispatched_specs_status() {
        let mut driver =
            MockPhaseDriver::failing_at_with_kind(Phase::Implementer, FailureKind::ShippedMismatch);
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "BUG-1527-AC6",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert_eq!(
            result.failure.as_ref().map(|f| f.kind),
            Some(FailureKind::ShippedMismatch)
        );
        assert_eq!(
            result.shipped_spec_id, None,
            "nothing was credited — finish_success (the only Done-adjacent write) never ran"
        );
        assert_eq!(
            driver.mark_escalated_calls, 0,
            "no escalation stamp either — the dispatched spec was never touched"
        );
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer],
            "no later phase — and so no later phase's status write — ever ran"
        );
    }

    /// BUG-1524 (control): a GENUINE launched-then-failed phase-1 failure
    /// (the default `FailureKind::Failed`, matching `run_implementer`'s
    /// non-`LaunchRefused` failures) must keep the existing BUG-1145
    /// recovery behaviour — this is the same scenario as
    /// `orchestrate_phase1_failure_with_verified_pr_proceeds_from_ci` above,
    /// pinned again here under the BUG-1524 trace so a future change to the
    /// launch-refusal guard cannot silently widen to swallow this case too.
    // trace:BUG-1524 | ai:claude
    #[test]
    fn genuine_launched_failure_still_recovers_through_open_pr() {
        let mut driver =
            MockPhaseDriver::failing_at_with_kind(Phase::Implementer, FailureKind::Failed)
                .recovering_phase1_failure_from_pr(Phase::Ci);
        let result = orchestrate(
            &mut driver,
            "BUG-1524-control",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.failed_phase.is_none());
        assert!(result.shelved_reason.is_none());
        assert!(driver.calls.contains(&Phase::Ci));
    }

    /// Instance A — phase 3 ends with no verdict file because the reviewer
    /// escalated and a human merged the PR out-of-band. The reconcile step
    /// finds the PR merged and treats the run as a success.
    #[test]
    fn orchestrate_reconciles_phase3_out_of_band_merge() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Reviewer).reconciles_as(
            PhaseReconcile::ShippedOutOfBand {
                reason: "PR-94 is merged and BUG-233 is Completed".to_string(),
            },
        );
        let result = orchestrate(
            &mut driver,
            "BUG-233",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.failed_phase.is_none());
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer, Phase::Ci, Phase::Reviewer]
        );
    }

    /// A non-Approved verdict is also routed through reconcile — if reality
    /// shows the PR merged out-of-band, even a Rejected verdict is redeemed.
    #[test]
    fn orchestrate_reconciles_rejected_verdict_when_pr_merged() {
        let mut driver = MockPhaseDriver::with_verdict(Verdict::Rejected).reconciles_as(
            PhaseReconcile::ShippedOutOfBand {
                reason: "PR-94 is already merged".to_string(),
            },
        );
        let result = orchestrate(
            &mut driver,
            "BUG-233",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.failed_phase.is_none());
    }

    /// Regression guard — the reconcile step must NOT mask a real failure.
    /// With the default `GenuineFailure` verdict (reality confirms nothing
    /// shipped) a phase-1 failure still exits `1` and names the failed phase.
    #[test]
    fn orchestrate_genuine_phase1_failure_still_fails() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Implementer);
        let result = orchestrate(
            &mut driver,
            "BUG-241",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 1);
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert!(result.failure.is_some());
    }

    /// TASK-133: the orchestrator-parent phase-1 status-bump compensation
    /// decision. The bug it guards: `prepare_auto_complete_phase1_status`
    /// flips Approved → InProgress before spawning the implementer; a phase-1
    /// failure with no lease ever recorded then strands the spec in
    /// NeedsAttention with no work behind it.
    #[test]
    fn compensate_phase1_bump_when_bumped_no_lease_phase1_failed() {
        // The stranded case: parent bumped, child never leased, phase 1 failed.
        assert!(should_compensate_phase1_bump(
            true,
            false,
            Some(Phase::Implementer)
        ));
    }

    #[test]
    fn no_compensation_when_lease_acquired() {
        // A lease was recorded → real work (commits / worktree) exists to
        // triage; leave the spec shelved rather than silently resetting it.
        assert!(!should_compensate_phase1_bump(
            true,
            true,
            Some(Phase::Implementer)
        ));
    }

    #[test]
    fn no_compensation_when_parent_did_not_bump() {
        // The parent left the status untouched (e.g. already InProgress /
        // Planned) → nothing to restore.
        assert!(!should_compensate_phase1_bump(
            false,
            false,
            Some(Phase::Implementer)
        ));
    }

    #[test]
    fn no_compensation_when_a_later_phase_failed() {
        // The implementer shipped a PR and a later phase failed → the bump
        // was legitimate, the work is real; do not reset.
        assert!(!should_compensate_phase1_bump(true, false, Some(Phase::Ci)));
        assert!(!should_compensate_phase1_bump(
            true,
            true,
            Some(Phase::Reviewer)
        ));
    }

    #[test]
    fn no_compensation_on_success() {
        // No failed phase at all (clean ship / punt / inconclusive) → the
        // status is legitimately advanced or deliberately held; never reset.
        assert!(!should_compensate_phase1_bump(true, false, None));
    }

    // BUG-1134: a lease-less phase-1 failure that masks an already-open PR must
    // NOT restore the spec to un-started (that orphans the PR). Only a
    // DEFINITIVE open PR skips the restore; an absent/flaky forge falls back.
    #[test]
    fn phase1_recovery_preserves_an_existing_open_pr() {
        let found = crate::forge::ChangeLookup::Found(crate::forge::ChangeRef {
            id: 1806,
            url: "https://example.test/pull/1806".to_string(),
            branch: "fix/bug-1130".to_string(),
            base: "main".to_string(),
            title: None,
        });
        assert_eq!(
            classify_phase1_failure_recovery(&found),
            Phase1FailureRecovery::PreserveOpenPr
        );
    }

    #[test]
    fn phase1_recovery_restores_when_no_definitive_pr() {
        // NoChange (definitively none) and every INCONCLUSIVE lookup fall back
        // to the safe restore — a transient/absent forge must not strand the
        // spec InProgress with no lease.
        for lookup in [
            crate::forge::ChangeLookup::NoChange,
            crate::forge::ChangeLookup::CliMissing,
            crate::forge::ChangeLookup::CliFailed("auth".to_string()),
            crate::forge::ChangeLookup::Unreachable("network".to_string()),
        ] {
            assert_eq!(
                classify_phase1_failure_recovery(&lookup),
                Phase1FailureRecovery::RestoreUnstarted,
                "inconclusive/absent forge must restore, not skip: {lookup:?}"
            );
        }
    }

    /// Regression guard at phase 3 — a genuine no-verdict failure (reality
    /// confirms nothing merged) still stops the batch at phase 3.
    #[test]
    fn orchestrate_genuine_phase3_failure_still_fails() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Reviewer);
        let result = orchestrate(
            &mut driver,
            "BUG-241",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 3);
        assert_eq!(result.failed_phase, Some(Phase::Reviewer));
    }

    /// The default `PhaseDriver::reconcile_failure` is `GenuineFailure` — a
    /// driver that does not override it (cannot check reality) leaves every
    /// failure standing. This pins the conservative default.
    #[test]
    fn reconcile_failure_default_is_genuine_failure() {
        let mut driver = MockPhaseDriver::all_ok();
        // `all_ok()` leaves `reconcile` at its `GenuineFailure` default.
        assert_eq!(
            driver.reconcile_failure(Phase::Implementer, &PhaseFailure::new("x")),
            PhaseReconcile::GenuineFailure
        );
    }

    /// TASK-1350: only write-capable denials reclassify a stop as
    /// PermissionDenied — read-only only, write only, both, and empty.
    // trace:TASK-1350 | ai:claude
    #[test]
    fn only_write_capable_denials_reclassify_as_permission_denied() {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Explicit write set.
        for tool in [
            "Edit",
            "Write",
            "MultiEdit",
            "NotebookEdit",
            "Bash",
            "PowerShell",
        ] {
            assert!(is_write_capable_tool(tool), "{tool}");
        }
        // The file-editing half is derived from the contained boundary's list.
        for tool in crate::contained_boundary::WRITE_TOOLS {
            assert!(is_write_capable_tool(tool), "{tool}");
        }
        // Named read-only built-ins.
        for tool in ["Read", "Glob", "Grep", "WebFetch", "WebSearch"] {
            assert!(!is_write_capable_tool(tool), "{tool}");
        }
        // Unknown MCP tools and unnamed denials are conservatively write-capable.
        assert!(is_write_capable_tool("mcp__github__get_issue"));
        assert!(is_write_capable_tool("an unknown tool"));
        assert!(is_write_capable_tool(""));
        assert!(denials_reclassify_as_permission_denied(&v(&[
            "", "WebFetch"
        ])));
        // Only a PowerShell denial reclassifies, like Bash.
        assert!(denials_reclassify_as_permission_denied(&v(&["PowerShell"])));

        // Only a read denial: keeps the original kind.
        assert!(!denials_reclassify_as_permission_denied(&v(&["WebFetch"])));
        assert!(!denials_reclassify_as_permission_denied(&v(&[
            "Read",
            "WebSearch"
        ])));
        // Only a write denial.
        assert!(denials_reclassify_as_permission_denied(&v(&["Edit"])));
        // Both.
        assert!(denials_reclassify_as_permission_denied(&v(&[
            "WebFetch", "Write"
        ])));
        // Empty.
        assert!(!denials_reclassify_as_permission_denied(&[]));
    }

    /// BUG-1930: a synthetic headless stream whose implementer stopped on
    /// permission denials classifies as PermissionDenied (not LeaseConflict,
    /// not a transient NoPr): never retried, the reason names the denied
    /// tools, and the hint points at the permission boundary.
    // trace:BUG-1930 | ai:claude
    #[test]
    fn permission_denied_stream_is_a_distinct_unretried_failure() {
        let stream = concat!(
            "{\"type\":\"system\",\"subtype\":\"init\"}\n",
            "not json\n",
            "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,",
            "\"permission_denials\":[{\"tool_name\":\"Edit\"},{\"tool_name\":\"Edit\"},",
            "{\"tool_name\":\"Write\"},{\"tool\":\"Bash\"}]}\n",
        );
        let tools = crate::questions::headless_log_permission_denied_tools(stream);
        assert_eq!(tools, vec!["Edit", "Write", "Bash"]);
        assert!(crate::questions::headless_log_permission_denied_tools(
            "{\"type\":\"result\",\"permission_denials\":[]}\n"
        )
        .is_empty());

        let failure = permission_denied_failure(&tools);
        assert_eq!(failure.kind, FailureKind::PermissionDenied);
        assert!(
            failure.reason.contains("(Edit, Write, Bash)"),
            "{}",
            failure.reason
        );
        assert!(failure.kind.is_shelvable());
        assert_eq!(failure.kind.cause_slug(), "permission-denied");
        assert!(!should_retry_transient_failure(failure.kind, 0, 3));
        assert_eq!(
            failure.clone().reclassify_transient().kind,
            FailureKind::PermissionDenied
        );
        let hint = recovery_hint(Phase::Implementer, failure.kind, &ctx());
        assert!(hint.contains("writes were denied"), "{hint}");
        assert!(hint.contains("aida queue work TASK-247"), "{hint}");
    }

    // --- Recovery hints: per phase, per failure kind ----------------------

    fn ctx() -> HintContext {
        HintContext {
            spec: "TASK-247".to_string(),
            branch: Some("task-247".to_string()),
            pr_number: Some(46),
            implementer_session: Some("019e2f423e7c".to_string()),
            ci_run_id: Some("9988776655".to_string()),
            forge: crate::forge::ForgeKind::GitHub,
        }
    }

    // The phase-default (`Failed`) hints — the pre-BUG-218 behaviour, now
    // gated behind `FailureKind::Failed` so a spawn ENOENT no longer borrows
    // them.

    #[test]
    fn recovery_hint_implementer_failed_names_resume() {
        let hint = recovery_hint(Phase::Implementer, FailureKind::Failed, &ctx());
        assert!(hint.contains("aida queue work TASK-247 --resume 019e2f423e7c"));
    }

    /// BUG-236: by phase 3 the spec is Done (the implementer opened the PR
    /// in phase 1), so the recovery hint must use `aida queue rework`, the
    /// verb that re-opens a Done spec — never `aida queue work`, which
    /// operates on queued items only and would bounce.
    #[test]
    fn recovery_hint_reviewer_failed_names_rework_verb() {
        let hint = recovery_hint(Phase::Reviewer, FailureKind::Failed, &ctx());
        assert!(
            hint.contains("aida queue rework TASK-247 --work"),
            "hint: {hint}"
        );
        assert!(
            !hint.contains("aida queue work TASK-247"),
            "phase-3 hint must not route a Done spec through `queue work`: {hint}"
        );
    }

    #[test]
    fn recovery_hint_merge_failed_names_pr_view() {
        let hint = recovery_hint(Phase::Merge, FailureKind::Failed, &ctx());
        assert!(hint.contains("gh pr view 46"));
    }

    /// TASK-1459: a live review marker (STORY-1405) is not a merge-lease
    /// conflict — the drain merge refusal must carry its own kind with an
    /// accurate hint, not point the user at `aida merge-lock` (which names a
    /// different seat entirely: another MERGER, not a review still running).
    /// STORY-1690: a behind head shelves with its own kind and a hint that
    /// hands the PR to the merge train.
    #[test]
    fn behind_base_is_distinct_and_hands_off_to_the_train() {
        assert!(FailureKind::BehindBase.is_shelvable());
        assert_eq!(FailureKind::BehindBase.cause_slug(), "behind-base");
        assert_eq!(
            FailureKind::from_cause_slug("behind-base"),
            Some(FailureKind::BehindBase)
        );
        let hint = recovery_hint(Phase::Merge, FailureKind::BehindBase, &ctx());
        assert!(hint.contains("aida pr train 46"), "hint: {hint}");
    }

    #[test]
    fn review_in_progress_is_distinct_from_lease_conflict() {
        assert!(FailureKind::ReviewInProgress.is_shelvable());
        assert_eq!(
            FailureKind::ReviewInProgress.cause_slug(),
            "review-in-progress"
        );
        assert_ne!(FailureKind::ReviewInProgress, FailureKind::LeaseConflict);
        assert_ne!(
            FailureKind::ReviewInProgress.cause_slug(),
            FailureKind::LeaseConflict.cause_slug()
        );

        let hint = recovery_hint(Phase::Merge, FailureKind::ReviewInProgress, &ctx());
        assert!(
            hint.contains("under review") && hint.contains("aida review record"),
            "hint: {hint}"
        );
        assert!(
            !hint.contains("merge-lease") && !hint.contains("aida merge-lock"),
            "must not reuse the merge-lease-conflict hint: {hint}"
        );

        let lease_hint = recovery_hint(Phase::Merge, FailureKind::LeaseConflict, &ctx());
        assert!(
            lease_hint.contains("merge-lease"),
            "the true lease-conflict hint must be unchanged: {lease_hint}"
        );
    }

    /// STORY-508/TASK-651: recovery hints are forge-aware — a GitLab project's
    /// drain failure names glab/MR commands, never gh.
    #[test]
    fn recovery_hints_are_forge_aware_for_gitlab() {
        let mut c = ctx();
        c.forge = crate::forge::ForgeKind::GitLab;

        let merge = recovery_hint(Phase::Merge, FailureKind::Failed, &c);
        assert!(merge.contains("glab mr view 46"), "{merge}");
        assert!(!merge.contains("gh pr"), "{merge}");

        let merge_missing = recovery_hint(Phase::Merge, FailureKind::MissingTool, &c);
        assert!(merge_missing.contains("glab"), "{merge_missing}");
        assert!(merge_missing.contains("GitLab CLI"), "{merge_missing}");
        assert!(!merge_missing.contains("gh"), "{merge_missing}");

        let ci_red = recovery_hint(Phase::Ci, FailureKind::CiRed, &c);
        assert!(ci_red.contains("glab ci view"), "{ci_red}");
        assert!(!ci_red.contains("gh run"), "{ci_red}");

        // Spawn-on-merge workaround also routes through glab.
        // trace:BUG-758 | ai:claude — no branch-cleanup flag: the drain's
        // spec worktree still holds the branch.
        let spawn = recovery_hint(Phase::Merge, FailureKind::Spawn, &c);
        assert!(spawn.contains("glab mr merge 46 --squash"), "{spawn}");
        assert!(!spawn.contains("--remove-source-branch"), "{spawn}");
    }

    /// BUG-236 regression guard: by phase 3 the spec is Done (the
    /// implementer opened the PR in phase 1), and `aida queue work`
    /// operates on queued items only — so no phase-3+ recovery hint, for
    /// any failure kind, may route the spec through `aida queue work
    /// <SPEC>`. The `aida queue work PR-N` form is fine (a PR review is
    /// status-independent); only the bare-spec form bounces. This test
    /// fails against the pre-fix `(Phase::Reviewer, _)` and `Internal`
    /// hints, which both did exactly that.
    #[test]
    fn recovery_hints_never_route_done_spec_through_queue_work() {
        let kinds = [
            FailureKind::Spawn,
            FailureKind::MissingTool,
            FailureKind::Internal,
            FailureKind::NoPr,
            FailureKind::CiRed,
            FailureKind::CiTimeout,
            FailureKind::NoVerdict,
            FailureKind::VerdictRequestChanges,
            FailureKind::VerdictReject,
            FailureKind::Failed,
        ];
        for phase in [Phase::Reviewer, Phase::Merge, Phase::Pull, Phase::Build] {
            for kind in kinds {
                let hint = recovery_hint(phase, kind, &ctx());
                // `ctx()` uses spec TASK-247 — the bare-spec `queue work`
                // form is the bounce; `queue work PR-46` is allowed.
                assert!(
                    !hint.contains("aida queue work TASK-247"),
                    "{phase:?}/{kind:?} routes the Done spec through `aida queue work`: {hint}"
                );
            }
        }
    }

    #[test]
    fn recovery_hint_pull_failed_names_rebase_dry_run() {
        let hint = recovery_hint(Phase::Pull, FailureKind::Failed, &ctx());
        assert!(hint.contains("aida rebase --dry-run --json"));
    }

    #[test]
    fn recovery_hint_build_failed_names_cargo_check() {
        let hint = recovery_hint(Phase::Build, FailureKind::Failed, &ctx());
        assert!(hint.contains("cargo check --workspace"));
    }

    #[test]
    fn recovery_hint_ci_failed_names_session_end() {
        // Phase-2 default (`Failed`) is "session would not end" — distinct
        // from a red CI run, which is its own `CiRed` kind below.
        let hint = recovery_hint(Phase::Ci, FailureKind::Failed, &ctx());
        assert!(hint.contains("would not end cleanly"));
        assert!(hint.contains("aida session end 019e2f423e7c --skip-ci"));
    }

    // CI-specific kinds: a red run vs a timeout vs a spawn ENOENT are three
    // different problems and must not collapse into "CI is red" (BUG-218).

    #[test]
    fn recovery_hint_ci_red_names_run_view_and_steal() {
        let hint = recovery_hint(Phase::Ci, FailureKind::CiRed, &ctx());
        assert!(hint.contains("CI failed on run 9988776655"));
        assert!(hint.contains("gh run view 9988776655"));
        assert!(hint.contains("--steal"));
    }

    #[test]
    fn recovery_hint_ci_red_without_run_id_falls_back_to_run_list() {
        let mut c = ctx();
        c.ci_run_id = None;
        let hint = recovery_hint(Phase::Ci, FailureKind::CiRed, &c);
        assert!(hint.contains("gh run list --branch task-247"));
        assert!(!hint.contains("<ID>"));
        assert!(!hint.contains("CI failed on run"));
    }

    #[test]
    fn recovery_hint_gitlab_ci_red_without_pipeline_id_omits_placeholder() {
        let mut c = ctx();
        c.forge = crate::forge::ForgeKind::GitLab;
        c.ci_run_id = None;
        let hint = recovery_hint(Phase::Ci, FailureKind::CiRed, &c);
        assert!(hint.contains("CI failed — view it: `glab ci status`"));
        assert!(!hint.contains("<ID>"));
        assert!(!hint.contains("CI failed on run"));
    }

    #[test]
    fn recovery_hint_ci_timeout_says_not_red() {
        let hint = recovery_hint(Phase::Ci, FailureKind::CiTimeout, &ctx());
        assert!(hint.contains("terminal state"));
        assert!(hint.contains("CI is not red"));
        // A timeout is not a red run — must not route to `gh run view`.
        assert!(!hint.contains("CI failed on run"));
    }

    // trace:BUG-1250 | ai:codex
    #[test]
    fn ci_unavailable_is_shelvable_with_typed_cause_and_retry_hint() {
        assert!(FailureKind::CiUnavailable.is_shelvable());
        assert_eq!(FailureKind::CiUnavailable.cause_slug(), "ci-unavailable");
        let hint = recovery_hint(Phase::Ci, FailureKind::CiUnavailable, &ctx());
        assert!(hint.contains("gate stayed closed"), "{hint}");
        assert!(hint.contains("gh api /rate_limit"), "{hint}");
        assert!(hint.contains("--auto-complete"), "{hint}");
    }

    /// BUG-218 regression: a phase-2 subprocess spawn ENOENT must NOT be
    /// reported as a red CI run — the failure was local subprocess plumbing.
    #[test]
    fn recovery_hint_ci_spawn_does_not_blame_ci() {
        let hint = recovery_hint(Phase::Ci, FailureKind::Spawn, &ctx());
        assert!(hint.contains("Subprocess spawn failed"));
        assert!(hint.contains("not CI"));
        // The exact mis-routing BUG-218 reported:
        assert!(!hint.contains("CI is red"));
        assert!(!hint.contains("CI failed"));
        assert!(!hint.contains("gh run"));
        // It names the phase-2 manual fallback.
        assert!(hint.contains("aida session end 019e2f423e7c --wait-ci"));
    }

    #[test]
    fn recovery_hint_spawn_names_each_phases_manual_fallback() {
        // Every phase's spawn hint says "spawn failed" + names that phase's
        // own by-hand command — never a CI command.
        for (phase, fallback) in [
            (Phase::Implementer, "aida queue work TASK-247"),
            (Phase::Ci, "aida session end 019e2f423e7c --wait-ci"),
            (Phase::Reviewer, "aida queue work PR-46"),
            // trace:BUG-758 | ai:claude — the spec worktree holds the branch,
            // so the manual fallback must not carry --delete-branch.
            (Phase::Merge, "gh pr merge 46 --squash"),
            (Phase::Pull, "aida pull"),
            (Phase::Build, "cargo build --release"),
        ] {
            let hint = recovery_hint(phase, FailureKind::Spawn, &ctx());
            assert!(
                hint.contains("Subprocess spawn failed"),
                "{phase:?} spawn hint should name a spawn failure: {hint}"
            );
            assert!(
                hint.contains(fallback),
                "{phase:?} spawn hint should name `{fallback}`: {hint}"
            );
        }
    }

    #[test]
    fn recovery_hint_internal_routes_to_file_a_bug() {
        // An internal-invariant violation is an AIDA bug — the hint says so
        // for every phase rather than blaming the user's work.
        for phase in [
            Phase::Implementer,
            Phase::Ci,
            Phase::Reviewer,
            Phase::Merge,
            Phase::Pull,
            Phase::Build,
        ] {
            let hint = recovery_hint(phase, FailureKind::Internal, &ctx());
            assert!(
                hint.contains("orchestrator bug") && hint.contains("file it"),
                "{phase:?} internal hint should route to filing a bug: {hint}"
            );
        }
    }

    #[test]
    fn recovery_hint_implementer_no_pr_says_open_a_pr() {
        let hint = recovery_hint(Phase::Implementer, FailureKind::NoPr, &ctx());
        assert!(hint.contains("without opening a PR"));
        assert!(hint.contains("/aida-pr"));
        // TASK-401: the orchestrator's phase-1 detector fires when a
        // headless implementer commits + exits without pushing or opening
        // a PR (the failure mode `/aida-pickup` Step 5c enforces against).
        // The exit hint is the operator's only thread back into the
        // session — it must name the resume verb with the session ID, so
        // it's a one-command re-entry. A bare "run /aida-pr" without the
        // resume context leaves the operator hunting for the lease.
        // trace:TASK-401 | ai:claude
        assert!(
            hint.contains("aida queue work TASK-247 --resume 019e2f423e7c"),
            "no-PR hint must give the operator a one-command resume back into \
             the phase-1 session, not just a bare `/aida-pr` reference: {hint}"
        );
    }

    #[test]
    fn recovery_hint_missing_tool_says_install_gh() {
        for phase in [Phase::Implementer, Phase::Merge] {
            let hint = recovery_hint(phase, FailureKind::MissingTool, &ctx());
            assert!(hint.contains("`gh` is not on PATH"), "{phase:?}: {hint}");
            assert!(hint.contains("cli.github.com"), "{phase:?}: {hint}");
        }
    }

    #[test]
    fn recovery_hint_reviewer_no_verdict_says_rerun_review() {
        let hint = recovery_hint(Phase::Reviewer, FailureKind::NoVerdict, &ctx());
        assert!(hint.contains("without a usable verdict"));
        assert!(hint.contains("aida queue work PR-46 --steal"));
    }

    /// BUG-218 acceptance: each phase produces at least 3 distinguishable
    /// recovery hints. `Spawn`, `Internal`, and the phase default (`Failed`)
    /// apply to every phase and must each yield a different message.
    #[test]
    fn recovery_hint_three_distinguishable_patterns_per_phase() {
        for phase in [
            Phase::Implementer,
            Phase::Ci,
            Phase::Reviewer,
            Phase::Merge,
            Phase::Pull,
            Phase::Build,
        ] {
            let spawn = recovery_hint(phase, FailureKind::Spawn, &ctx());
            let internal = recovery_hint(phase, FailureKind::Internal, &ctx());
            let failed = recovery_hint(phase, FailureKind::Failed, &ctx());
            assert_ne!(spawn, internal, "{phase:?}: spawn vs internal collapsed");
            assert_ne!(spawn, failed, "{phase:?}: spawn vs failed collapsed");
            assert_ne!(internal, failed, "{phase:?}: internal vs failed collapsed");
        }
    }

    // --- Pure helpers -----------------------------------------------------

    #[test]
    fn variant_parse_accepts_all_forms() {
        assert_eq!(
            AutoCompleteVariant::parse(""),
            Ok(AutoCompleteVariant::Full)
        );
        assert_eq!(
            AutoCompleteVariant::parse("full"),
            Ok(AutoCompleteVariant::Full)
        );
        assert_eq!(
            AutoCompleteVariant::parse("through-ci"),
            Ok(AutoCompleteVariant::ThroughCi)
        );
        assert_eq!(
            AutoCompleteVariant::parse("THROUGH-MERGE"),
            Ok(AutoCompleteVariant::ThroughMerge)
        );
        assert_eq!(
            AutoCompleteVariant::parse("skip-build"),
            Ok(AutoCompleteVariant::SkipBuild)
        );
        assert!(AutoCompleteVariant::parse("bogus").is_err());
    }

    #[test]
    fn variant_last_phase() {
        assert_eq!(AutoCompleteVariant::ThroughCi.last_phase(), 2);
        assert_eq!(AutoCompleteVariant::ThroughMerge.last_phase(), 4);
        assert_eq!(AutoCompleteVariant::SkipBuild.last_phase(), 5);
        assert_eq!(AutoCompleteVariant::Full.last_phase(), 6);
    }

    #[test]
    fn verdict_parse_is_tolerant() {
        assert_eq!(Verdict::parse("Approved"), Some(Verdict::Approved));
        assert_eq!(Verdict::parse("  approved "), Some(Verdict::Approved));
        assert_eq!(
            Verdict::parse("RequestChanges"),
            Some(Verdict::RequestChanges)
        );
        assert_eq!(
            Verdict::parse("request-changes"),
            Some(Verdict::RequestChanges)
        );
        assert_eq!(Verdict::parse("Rejected"), Some(Verdict::Rejected));
        assert_eq!(Verdict::parse("maybe"), None);
    }

    #[test]
    fn phase_event_json_shape() {
        let line = phase_event("implementer", "started", "TASK-247", 1234, None, &[]);
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["phase"], "implementer");
        assert_eq!(v["status"], "started");
        assert_eq!(v["spec"], "TASK-247");
        assert_eq!(v["elapsed_ms"], 1234);
        assert!(v.get("exit_code").is_none());
    }

    #[test]
    fn phase_event_json_includes_exit_code_and_extra() {
        let line = phase_event(
            "ci",
            "failed",
            "TASK-247",
            5000,
            Some(2),
            &[("reason", "CI red"), ("hint", "gh run view 9")],
        );
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["exit_code"], 2);
        assert_eq!(v["reason"], "CI red");
        assert_eq!(v["hint"], "gh run view 9");
    }

    #[test]
    fn fmt_duration_renders_minutes_and_seconds() {
        assert_eq!(fmt_duration(42_000), "42s");
        assert_eq!(fmt_duration(318_000), "5m 18s");
        assert_eq!(fmt_duration(60_000), "1m 0s");
    }

    #[test]
    fn phase_exit_code_equals_index() {
        assert_eq!(Phase::Implementer.index(), 1);
        assert_eq!(Phase::Ci.index(), 2);
        assert_eq!(Phase::Reviewer.index(), 3);
        assert_eq!(Phase::Merge.index(), 4);
        assert_eq!(Phase::Pull.index(), 5);
        assert_eq!(Phase::Build.index(), 6);
    }

    #[test]
    fn phase_from_index_round_trips() {
        for phase in [
            Phase::Implementer,
            Phase::Ci,
            Phase::Reviewer,
            Phase::Merge,
            Phase::Pull,
            Phase::Build,
        ] {
            assert_eq!(Phase::from_index(phase.index()), Some(phase));
        }
        assert_eq!(Phase::from_index(0), None);
        assert_eq!(Phase::from_index(7), None);
    }

    #[test]
    fn variant_slug_matches_parse_spelling() {
        for v in [
            AutoCompleteVariant::Full,
            AutoCompleteVariant::ThroughCi,
            AutoCompleteVariant::ThroughMerge,
            AutoCompleteVariant::SkipBuild,
        ] {
            assert_eq!(AutoCompleteVariant::parse(v.slug()), Ok(v));
        }
    }

    // --- NoHumanMode (STORY-263, TASK-306) --------------------------------

    #[test]
    fn no_human_mode_parse_accepts_all_forms() {
        // TASK-306: bare `--no-human` (clap default-missing) and an empty
        // string resolve to reviewer-only — the honest default of what the
        // flag does today (the headless implementer is not shipped).
        assert_eq!(NoHumanMode::parse(""), Ok(NoHumanMode::ReviewerOnly));
        assert_eq!(
            NoHumanMode::parse("reviewer-only"),
            Ok(NoHumanMode::ReviewerOnly)
        );
        assert_eq!(
            NoHumanMode::parse("reviewer_only"),
            Ok(NoHumanMode::ReviewerOnly)
        );
        assert_eq!(
            NoHumanMode::parse("  reviewer "),
            Ok(NoHumanMode::ReviewerOnly)
        );
        // `both` still parses — the kickoff gate, not the grammar, is what
        // rejects it until the headless implementer ships.
        assert_eq!(NoHumanMode::parse("both"), Ok(NoHumanMode::Both));
        assert_eq!(NoHumanMode::parse("BOTH"), Ok(NoHumanMode::Both));
        assert!(NoHumanMode::parse("bogus").is_err());
    }

    #[test]
    fn no_human_mode_only_both_wants_headless_implementer() {
        assert!(NoHumanMode::Both.wants_headless_implementer());
        assert!(!NoHumanMode::ReviewerOnly.wants_headless_implementer());
    }

    #[test]
    fn no_human_mode_slug_round_trips_through_parse() {
        // TASK-306: the slug is the propagated `AIDA_NO_HUMAN_MODE` value and
        // must parse back to the same mode.
        for mode in [NoHumanMode::ReviewerOnly, NoHumanMode::Both] {
            assert_eq!(NoHumanMode::parse(mode.slug()), Ok(mode));
        }
        assert_eq!(NoHumanMode::ReviewerOnly.slug(), "reviewer-only");
        assert_eq!(NoHumanMode::Both.slug(), "both");
    }

    // --- OrchestrationResult telemetry fields (TASK-266) ------------------

    #[test]
    fn result_success_records_every_phase_duration_in_order() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.failed_phase.is_none());
        assert!(result.failure.is_none());
        let phases: Vec<Phase> = result.phase_durations.iter().map(|(p, _)| *p).collect();
        assert_eq!(
            phases,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ]
        );
    }

    #[test]
    fn result_variant_caps_phase_durations_at_last_phase() {
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::ThroughCi,
            false,
            EscalateMode::Blocks,
        );
        let phases: Vec<Phase> = result.phase_durations.iter().map(|(p, _)| *p).collect();
        assert_eq!(phases, vec![Phase::Implementer, Phase::Ci]);
    }

    // trace:BUG-1807 | ai:codex
    #[test]
    fn bug_1807_through_ci_refuses_success_when_handoff_fails() {
        for skip_ci in [false, true] {
            let mut driver = MockPhaseDriver::all_ok();
            driver.handoff_fails = true;
            let result = orchestrate_with_lifecycle_skip(
                &mut driver,
                "BUG-1807",
                AutoCompleteVariant::ThroughCi,
                false,
                EscalateMode::Blocks,
                LifecycleSkip {
                    no_ci_wait: skip_ci,
                    ..Default::default()
                },
                false,
            );
            assert_ne!(result.exit_code, 0);
            assert_eq!(result.failed_phase, Some(Phase::Ci));
            assert!(!driver.calls.contains(&Phase::Reviewer));
        }
    }

    // trace:TASK-1155 trace:ADR-11 | ai:codex
    #[test]
    fn through_ci_checkpoint_names_pr_ci_review_and_merge_next_steps() {
        let rendered =
            render_through_ci_checkpoint("TASK-1155", Some(123), crate::forge::ForgeKind::GitHub);
        assert!(rendered.contains("TASK-1155 PR checkpoint"));
        assert!(rendered.contains("PR: PR-123"));
        assert!(rendered.contains("CI: green"));
        assert!(rendered.contains("Review: routed to reviewer queue"));
        assert!(rendered.contains("Next: aida queue work PR-123 --role reviewer"));
        // trace:BUG-758 | ai:claude — no --delete-branch (worktree holds the
        // branch), and ';' not '&&' so the pull leg cannot be dropped.
        assert!(rendered.contains("After review: gh pr merge 123 --squash; aida pull"));
        assert!(!rendered.contains("--delete-branch"));
        assert!(!rendered.contains("&& aida pull"));

        let gitlab =
            render_through_ci_checkpoint("TASK-1155", Some(123), crate::forge::ForgeKind::GitLab);
        assert!(gitlab.contains("TASK-1155 MR checkpoint"));
        assert!(gitlab.contains("MR: MR-123"));
        assert!(gitlab.contains("Next: aida queue work MR-123 --role reviewer"));
        assert!(gitlab.contains("After review: glab mr merge 123 --squash; aida pull"));
        assert!(!gitlab.contains("gh pr"));
    }

    #[test]
    fn result_failure_carries_failed_phase_and_reason() {
        let mut driver = MockPhaseDriver::failing_at(Phase::Reviewer);
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 3);
        assert_eq!(result.failed_phase, Some(Phase::Reviewer));
        let failure = result.failure.expect("failure detail should be set");
        assert!(failure.reason.contains("Reviewer"));
        // Durations stop at the failed phase — they include it, not beyond.
        let phases: Vec<Phase> = result.phase_durations.iter().map(|(p, _)| *p).collect();
        assert_eq!(phases, vec![Phase::Implementer, Phase::Ci, Phase::Reviewer]);
    }

    #[test]
    fn result_rejected_verdict_records_reviewer_as_failed_phase() {
        let mut driver = MockPhaseDriver::with_verdict(Verdict::Rejected);
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Reviewer));
        assert!(result
            .failure
            .expect("failure set")
            .reason
            .contains("not Approved"));
    }

    // trace:STORY-974 | ai:codex
    #[test]
    fn reviewer_verdict_failures_record_typed_causes() {
        for (verdict, cause) in [
            (Verdict::RequestChanges, "verdict:request-changes"),
            (Verdict::Rejected, "verdict:reject"),
        ] {
            let mut driver = MockPhaseDriver::with_verdict(verdict);
            let result = orchestrate(
                &mut driver,
                "TASK-247",
                AutoCompleteVariant::Full,
                false,
                EscalateMode::Blocks,
            );
            assert_eq!(result.failed_phase, Some(Phase::Reviewer));
            assert_eq!(
                result.failure.as_ref().map(|f| f.kind.cause_slug()),
                Some(cause)
            );
        }
    }

    // trace:STORY-974 | ai:codex
    #[test]
    fn every_real_failure_kind_maps_to_closed_shelve_cause() {
        let allowed = [
            "verdict:request-changes",
            "verdict:reject",
            "reviewer-wrote",
            "rework-no-op",
            "ci-red",
            "tool-exit",
            "no-verdict",
            "no-pr",
            "watchdog",
            "cache-locked",
            "headless-wait",
            "environmental",
            "internal",
        ];
        for kind in [
            FailureKind::Spawn,
            FailureKind::MissingTool,
            FailureKind::Internal,
            FailureKind::NoPr,
            FailureKind::CiRed,
            FailureKind::CiTimeout,
            FailureKind::NoVerdict,
            FailureKind::VerdictRequestChanges,
            FailureKind::VerdictReject,
            FailureKind::ReviewerWrote,
            FailureKind::ReworkNoOp,
            FailureKind::PrVerificationInconclusive,
            FailureKind::Watchdog,
            FailureKind::CacheLocked,
            FailureKind::LaunchNoOutput,
            FailureKind::HeadlessWait,
            FailureKind::Failed,
        ] {
            assert!(
                allowed.contains(&kind.cause_slug()),
                "{kind:?} mapped outside the closed cause set"
            );
        }
    }

    // BUG-1186: a reviewer that writes to the PR head is shelved under its
    // own typed cause and is never a transient to retry.
    #[test]
    fn reviewer_wrote_is_shelvable_typed_and_not_transient() {
        assert!(FailureKind::ReviewerWrote.is_shelvable());
        assert_eq!(FailureKind::ReviewerWrote.cause_slug(), "reviewer-wrote");
        assert!(!is_transient_retry_cause("reviewer-wrote"));
        let ctx = HintContext {
            spec: "BUG-1177".into(),
            branch: Some("bug-1177-x".into()),
            pr_number: Some(1882),
            ..HintContext::default()
        };
        let hint = recovery_hint(Phase::Reviewer, FailureKind::ReviewerWrote, &ctx);
        assert!(
            hint.contains("origin/bug-1177-x") && hint.contains("PR-1882"),
            "{hint}"
        );
    }

    // trace:BUG-1213 | ai:codex
    #[test]
    fn rework_no_op_is_shelvable_typed_and_not_transient() {
        assert!(FailureKind::ReworkNoOp.is_shelvable());
        assert_eq!(FailureKind::ReworkNoOp.cause_slug(), "rework-no-op");
        assert!(!is_transient_retry_cause("rework-no-op"));
    }

    // trace:BUG-1213 | ai:codex
    #[test]
    fn unchanged_rework_head_shelves_after_phase1_and_never_advances_to_ci() {
        let mut driver = MockPhaseDriver {
            rework_no_op: true,
            shelve_succeeds: true,
            ..MockPhaseDriver::base()
        };
        let result = orchestrate(
            &mut driver,
            "BUG-1213",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert_eq!(
            result.failure.as_ref().map(|f| f.kind),
            Some(FailureKind::ReworkNoOp)
        );
        assert!(result.shelved_reason.is_some());
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    /// BUG-1522: `PrOpened` was the ONLY implementer-advance arm that
    /// consulted `rework_no_op_failure` — but the arm a rework round against
    /// an already-open PR actually returns (confirmed by reading
    /// `RealPhaseDriver::run_implementer`'s `Phase1PrResolve::Found(pr) =>`
    /// handling, which matches to `Ok(ImplementerOutcome::PrOpened)`
    /// regardless of whether the PR was newly opened or already existed) IS
    /// `PrOpened` in the ordinary case — so this arm was never the gap.
    /// BUG-1460's silent readvance instead traveled the `Err(f)` arm's
    /// `recover_phase1_failure_with_open_pr` re-entry (matching BUG-1524's
    /// "phase 1 refuses, the refusal is recovered as an open-PR re-entry"
    /// shape): phase 1 fails, an open PR is found, the drain continues from
    /// the re-entry phase with no no-op check on the way through. This test
    /// exercises exactly that arm.
    // trace:BUG-1522 | ai:claude
    #[test]
    fn unchanged_rework_head_shelves_via_recovered_phase1_failure() {
        let mut driver = MockPhaseDriver {
            rework_no_op: true,
            shelve_succeeds: true,
            fail_at: Some(Phase::Implementer),
            phase1_pr_recovery: Some(Phase::Ci),
            ..MockPhaseDriver::base()
        };
        let result = orchestrate(
            &mut driver,
            "BUG-1522",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert_eq!(
            result.failure.as_ref().map(|f| f.kind),
            Some(FailureKind::ReworkNoOp)
        );
        assert!(result.shelved_reason.is_some());
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer],
            "the guard must fire before Ci — a recovered phase-1 failure must \
             not readvance an unchanged, already-refused head"
        );
    }

    /// BUG-1522: the advisor-resolved-punt `Proceed` arm must also refuse an
    /// unchanged head — the resumed implementer never re-enters via
    /// `PrOpened`.
    // trace:BUG-1522 | ai:claude
    #[test]
    fn unchanged_rework_head_shelves_via_resolved_punt_resume() {
        let mut driver = MockPhaseDriver::punting_at_implementer("auth flow fork")
            .advisor_resolves("use OAuth — the recorded convention")
            .resume_opens_pr();
        driver.rework_no_op = true;
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "BUG-1522",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert_eq!(
            result.failure.as_ref().map(|f| f.kind),
            Some(FailureKind::ReworkNoOp)
        );
        assert!(result.shelved_reason.is_some());
        assert_eq!(
            driver.calls,
            vec![Phase::Implementer],
            "the guard must fire before Ci on the resumed-after-punt path"
        );
    }

    /// BUG-1522 / BUG-709: `AlreadyMerged` must also refuse an unchanged
    /// head — e.g. a redispatch that only rediscovers a merge a prior run
    /// already shipped, with nothing new on the dispatched spec's branch.
    // trace:BUG-1522 | ai:claude
    #[test]
    fn unchanged_rework_head_shelves_via_already_merged() {
        let mut driver = MockPhaseDriver::already_merged_at_implementer(99);
        driver.rework_no_op = true;
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "BUG-1522",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert_eq!(
            result.failure.as_ref().map(|f| f.kind),
            Some(FailureKind::ReworkNoOp)
        );
        assert!(result.shelved_reason.is_some());
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    /// BUG-1522: a genuine `AlreadyMerged` (rework guard not tripped, or no
    /// rework round in play) still completes cleanly — the fix is a targeted
    /// no-op check, not a blanket refusal of this outcome.
    // trace:BUG-1522 | ai:claude
    #[test]
    fn already_merged_without_rework_no_op_still_completes() {
        let mut driver = MockPhaseDriver::already_merged_at_implementer(99);
        let result = orchestrate(
            &mut driver,
            "BUG-1522",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.failed_phase.is_none());
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    /// BUG-1522: `Inconclusive` must also refuse an unchanged head — this is
    /// one of the two "could not determine what happened" shapes the
    /// original report missed, and precisely where a silent no-op is most
    /// likely.
    // trace:BUG-1522 | ai:claude
    #[test]
    fn unchanged_rework_head_shelves_via_inconclusive() {
        let mut driver = MockPhaseDriver::inconclusive_at_implementer("GH API blip");
        driver.rework_no_op = true;
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "BUG-1522",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert_eq!(
            result.failure.as_ref().map(|f| f.kind),
            Some(FailureKind::ReworkNoOp)
        );
        assert!(result.shelved_reason.is_some());
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    /// BUG-1522: when the Inconclusive arm's rework-no-op guard shelves the
    /// round, the shelved `FailureReason` must keep the leg-specific
    /// `--resume` hint (BUG-266) `finish_inconclusive` would have shown had
    /// the guard not intercepted it first — not the generic
    /// `FailureKind::ReworkNoOp` "push a fixup commit or punt" hint, which is
    /// actively misleading when the round was cut short by an API outage
    /// rather than a stalled rework.
    // trace:BUG-1522 | ai:claude
    #[test]
    fn inconclusive_rework_no_op_keeps_the_resume_hint() {
        let mut driver = MockPhaseDriver::inconclusive_at_implementer(
            "Anthropic API outage during the headless implementer: API Error: 529 Overloaded",
        );
        driver.inconclusive_retry_hint =
            Some("aida queue work BUG-1522 --resume 019e2f423e7c".to_string());
        driver.rework_no_op = true;
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "BUG-1522",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert_eq!(
            result.failure.as_ref().map(|f| f.kind),
            Some(FailureKind::ReworkNoOp)
        );
        let shelved = result
            .shelved_reason
            .as_ref()
            .expect("the no-op round must shelve");
        assert_eq!(
            shelved.recovery_hint.as_deref(),
            Some("aida queue work BUG-1522 --resume 019e2f423e7c"),
            "the shelved reason must carry the Inconclusive arm's --resume hint, \
             not the generic ReworkNoOp hint"
        );
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    /// BUG-1522: `Held` must also refuse an unchanged head — the other
    /// "could not determine what happened" shape the original report
    /// missed.
    // trace:BUG-1522 | ai:claude
    #[test]
    fn unchanged_rework_head_shelves_via_held() {
        let mut driver = MockPhaseDriver::holding_at_implementer("waiting on manual gate");
        driver.rework_no_op = true;
        driver.shelve_succeeds = true;
        let result = orchestrate(
            &mut driver,
            "BUG-1522",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Implementer));
        assert_eq!(
            result.failure.as_ref().map(|f| f.kind),
            Some(FailureKind::ReworkNoOp)
        );
        assert!(result.shelved_reason.is_some());
        assert_eq!(driver.calls, vec![Phase::Implementer]);
    }

    // TASK-1244: merge-phase lease contention gets its own recovery hint.
    // trace:TASK-1244.AC1 | ai:claude
    #[test]
    fn merge_lease_conflict_hint_names_merge_lock_and_pr_ship() {
        let ctx = HintContext {
            spec: "TASK-1244".into(),
            pr_number: Some(1875),
            ..HintContext::default()
        };
        let hint = recovery_hint(Phase::Merge, FailureKind::LeaseConflict, &ctx);
        assert!(
            hint.contains("aida merge-lock") && hint.contains("aida pr ship 1875"),
            "{hint}"
        );
    }

    // --- Batch drain (TASK-285) -------------------------------------------

    /// A success / failure [`OrchestrationResult`] without the timing detail,
    /// for exercising [`drain_batch`].
    fn ok_result() -> OrchestrationResult {
        OrchestrationResult {
            exit_code: 0,
            failed_phase: None,
            failure: None,
            phase_durations: Vec::new(),
            total_ms: 0,
            punt_reason: None,
            shipped_spec_id: None,
            escalation: None,
            inconclusive_reason: None,
            shelved_reason: None,
            held_reason: None,
        }
    }

    fn fail_result(phase: Phase) -> OrchestrationResult {
        OrchestrationResult {
            exit_code: phase.index(),
            failed_phase: Some(phase),
            failure: Some(PhaseFailure::new(format!("mock failure at {phase:?}"))),
            phase_durations: Vec::new(),
            total_ms: 0,
            punt_reason: None,
            shipped_spec_id: None,
            escalation: None,
            inconclusive_reason: None,
            shelved_reason: None,
            held_reason: None,
        }
    }

    /// STORY-276: a punt [`OrchestrationResult`] — exit `0`, no `failed_phase`,
    /// `punt_reason` set. The shape `finish_punted` returns.
    fn punt_result() -> OrchestrationResult {
        OrchestrationResult {
            exit_code: 0,
            failed_phase: None,
            failure: None,
            phase_durations: Vec::new(),
            total_ms: 0,
            punt_reason: Some("mock punt: design-fork".to_string()),
            shipped_spec_id: None,
            escalation: None,
            inconclusive_reason: None,
            shelved_reason: None,
            held_reason: None,
        }
    }

    /// BUG-245: a success [`OrchestrationResult`] where phase 1 credited a
    /// different spec than the dispatched id — the shape `finish_success`
    /// returns on a dispatched≠shipped mismatch. trace:BUG-245 | ai:claude
    fn mismatch_result(shipped: &str) -> OrchestrationResult {
        OrchestrationResult {
            exit_code: 0,
            failed_phase: None,
            failure: None,
            phase_durations: Vec::new(),
            total_ms: 0,
            punt_reason: None,
            shipped_spec_id: Some(shipped.to_string()),
            escalation: None,
            inconclusive_reason: None,
            shelved_reason: None,
            held_reason: None,
        }
    }

    /// EPIC-28: a *shelved* failure [`OrchestrationResult`] — the failure
    /// fields stay set (exit code = phase index, `failed_phase` populated),
    /// but `shelved_reason` is `Some` so `drain_batch` recognises it as a
    /// recoverable failure and continues to the next member.
    /// trace:EPIC-28 | ai:claude
    fn shelve_result(phase: Phase) -> OrchestrationResult {
        let fr = aida_core::FailureReason {
            phase: phase.slug().to_string(),
            phase_index: phase.index() as u8,
            kind: "failed".to_string(),
            detail: format!("mock shelved failure at {phase:?}"),
            recovery_hint: None,
            shelved_by: None,
            shelved_at: chrono::Utc::now(),
        };
        OrchestrationResult {
            exit_code: phase.index(),
            failed_phase: Some(phase),
            failure: Some(PhaseFailure::new(format!("mock failure at {phase:?}"))),
            phase_durations: Vec::new(),
            total_ms: 0,
            punt_reason: None,
            shipped_spec_id: None,
            escalation: None,
            inconclusive_reason: None,
            shelved_reason: Some(fr),
            held_reason: None,
        }
    }

    /// STORY-306: an escalated [`OrchestrationResult`] — exit `0`, no
    /// `failed_phase`, `escalation` set. The shape `finish_escalated` returns.
    fn escalated_result() -> OrchestrationResult {
        OrchestrationResult {
            exit_code: 0,
            failed_phase: None,
            failure: None,
            phase_durations: Vec::new(),
            total_ms: 0,
            punt_reason: None,
            shipped_spec_id: None,
            escalation: Some(EscalationSummary {
                kind: EscalationKind::MergeDecision,
                reason: "mock escalation: merge decision".to_string(),
            }),
            inconclusive_reason: None,
            shelved_reason: None,
            held_reason: None,
        }
    }

    /// Mock batch driver: a FIFO of batch-head spec-ids. `run_spec` consumes
    /// the head on success (mirroring a real completed spec leaving the
    /// queue); a `fail_at` spec returns a failure result *without* consuming
    /// the head; a `stall` spec succeeds but is left at the head (the
    /// non-advancing-queue case); a `punt` spec returns a punt result and —
    /// like a real `NeedsAttention` punt — leaves the queue.
    struct MockBatchDriver {
        heads: Vec<String>,
        /// BUG-1930: a shelved spec whose failure carries this kind/reason
        /// instead of the generic mock failure.
        shelve_failure: Vec<(String, PhaseFailure)>,
        fail_at: Option<(String, Phase)>,
        stall_spec: Option<String>,
        punt_spec: Option<String>,
        /// BUG-245: when `run_spec` is called for `dispatched`, return a
        /// success result crediting `shipped` instead. The dispatched head
        /// stays in `heads` (matching reality — the implementer dequeued the
        /// shipped spec, not the dispatched one). trace:BUG-245 | ai:claude
        mismatch: Option<(String, String)>,
        escalate_spec: Option<String>,
        /// EPIC-28: specs that should return a shelvable failure (the
        /// orchestrator wrote a `FailureReason` to the store). A drain that
        /// hits one in this set parks it and continues to the next head
        /// (mirroring the punt path) rather than stopping. The mock
        /// consumes the head on shelving, mirroring the
        /// `resolve_batch_members` filter that would skip the freshly-
        /// shelved spec on the next iteration. trace:EPIC-28 | ai:claude
        shelve_at: Vec<(String, Phase)>,
        /// BUG-852: specs that shelve once without leaving the head, then rerun
        /// successfully later in the same drain. Models a recover-and-retry path
        /// where the final disposition should replace the earlier shelve.
        shelve_once_retry: Vec<(String, Phase)>,
        /// EPIC-28: specs the *test* wants treated as blocked-by a
        /// just-shelved spec. The mock drops them from `heads` *after* the
        /// named blocker is shelved, and records each as a skipped row.
        /// trace:EPIC-28 | ai:claude
        skip_dependent_of: Vec<(String, String, String)>, // (dependent, blocker, reason)
        skipped: Vec<(String, String)>,
        runs: Vec<String>,
        /// TASK-966: reported tokens each `run_spec` adds to the running total,
        /// so a `--max-tokens` cap test can drive the accumulator deterministically.
        tokens_each: u64,
        /// BUG-1980: this driver's own cooperative stop-request file. The
        /// drain sees a stop only when *this* path exists — never through the
        /// process-wide env var a parallel test may have set.
        // trace:BUG-1980 | ai:claude
        stop_file: Option<std::path::PathBuf>,
    }

    impl MockBatchDriver {
        fn new(heads: &[&str]) -> Self {
            Self {
                stop_file: None,
                heads: heads.iter().map(|s| s.to_string()).collect(),
                shelve_failure: Vec::new(),
                fail_at: None,
                stall_spec: None,
                punt_spec: None,
                mismatch: None,
                escalate_spec: None,
                shelve_at: Vec::new(),
                shelve_once_retry: Vec::new(),
                skip_dependent_of: Vec::new(),
                skipped: Vec::new(),
                runs: Vec::new(),
                tokens_each: 0,
            }
        }

        /// TASK-966: each completed spec reports `n` tokens (for `--max-tokens`).
        fn tokens_each(mut self, n: u64) -> Self {
            self.tokens_each = n;
            self
        }

        /// BUG-1980: stop cooperatively once `path` exists.
        fn stop_file(mut self, path: &std::path::Path) -> Self {
            self.stop_file = Some(path.to_path_buf());
            self
        }

        fn failing(mut self, spec: &str, phase: Phase) -> Self {
            self.fail_at = Some((spec.to_string(), phase));
            self
        }

        /// BUG-1930: shelve `spec` at phase 1 with this failure kind/reason.
        fn shelving_with(mut self, spec: &str, kind: FailureKind, reason: &str) -> Self {
            self.shelve_at.push((spec.to_string(), Phase::Implementer));
            self.shelve_failure
                .push((spec.to_string(), PhaseFailure::of(kind, reason)));
            self
        }

        fn stalling(mut self, spec: &str) -> Self {
            self.stall_spec = Some(spec.to_string());
            self
        }

        fn punting(mut self, spec: &str) -> Self {
            self.punt_spec = Some(spec.to_string());
            self
        }

        /// BUG-245: when `run_spec` runs `dispatched`, return a success that
        /// credits `shipped`. The dispatched head is *not* consumed — that
        /// mirrors a real run where the implementer worked a different spec
        /// and the dispatched one is still queued. trace:BUG-245 | ai:claude
        fn mismatching(mut self, dispatched: &str, shipped: &str) -> Self {
            self.mismatch = Some((dispatched.to_string(), shipped.to_string()));
            self
        }

        /// STORY-306: mark `spec` so its run returns an escalated result.
        fn escalating(mut self, spec: &str) -> Self {
            self.escalate_spec = Some(spec.to_string());
            self
        }

        /// EPIC-28: mark `spec` so its run returns a shelvable failure.
        /// The mock consumes the head (the real `resolve_batch_members`
        /// would filter the freshly-NeedsAttention spec out next call).
        fn shelving(mut self, spec: &str, phase: Phase) -> Self {
            self.shelve_at.push((spec.to_string(), phase));
            self
        }

        /// BUG-852: return a shelved failure once while leaving `spec` at the
        /// head so the next iteration retries and can ship it.
        fn shelving_once_then_retry(mut self, spec: &str, phase: Phase) -> Self {
            self.shelve_once_retry.push((spec.to_string(), phase));
            self
        }

        /// EPIC-28: when `blocker` is shelved, also drop `dependent` from
        /// the head queue and record the skip with `reason`. Simulates
        /// `resolve_batch_members`'s pickability filter: a member with
        /// `BlockedBy → <shelved>` becomes un-pickable on the next round.
        fn dependent(mut self, dependent: &str, blocker: &str, reason: &str) -> Self {
            self.skip_dependent_of.push((
                dependent.to_string(),
                blocker.to_string(),
                reason.to_string(),
            ));
            self
        }
    }

    impl BatchDriver for MockBatchDriver {
        fn next_head(&mut self) -> Option<String> {
            self.heads.first().cloned()
        }

        // trace:BUG-1980 | ai:claude
        fn stop_requested(&mut self) -> bool {
            self.stop_file.as_deref().is_some_and(|p| p.exists())
        }

        fn run_spec(&mut self, spec: &str) -> OrchestrationResult {
            self.runs.push(spec.to_string());
            if let Some((fail_spec, phase)) = &self.fail_at {
                if fail_spec == spec {
                    // Failure leaves the spec queued — head does not advance.
                    return fail_result(*phase);
                }
            }
            // EPIC-28: a *shelvable* failure parks the spec in NeedsAttention
            // (the orchestrator wrote the FailureReason), and the next head
            // pickup would skip it. Mirror that here by consuming the head.
            // Dependents of this spec drop off too. trace:EPIC-28 | ai:claude
            if let Some(pos) = self
                .shelve_once_retry
                .iter()
                .position(|(shelve_spec, _)| shelve_spec == spec)
            {
                let (_shelve_spec, phase) = self.shelve_once_retry.remove(pos);
                return shelve_result(phase);
            }
            for (shelve_spec, phase) in self.shelve_at.clone() {
                if shelve_spec == spec {
                    self.heads.retain(|h| h != spec);
                    let to_skip: Vec<(String, String, String)> = self
                        .skip_dependent_of
                        .iter()
                        .filter(|(_, blocker, _)| blocker == spec)
                        .cloned()
                        .collect();
                    for (dep, _blocker, reason) in to_skip {
                        if self.heads.iter().any(|h| h == &dep) {
                            self.heads.retain(|h| h != &dep);
                            self.skipped.push((dep, reason));
                        }
                    }
                    let mut result = shelve_result(phase);
                    if let Some((_, failure)) = self.shelve_failure.iter().find(|(s, _)| s == spec)
                    {
                        result.failure = Some(failure.clone());
                    }
                    return result;
                }
            }
            if self.stall_spec.as_deref() == Some(spec) {
                // "Success" but the head is intentionally not consumed.
                return ok_result();
            }
            if self.punt_spec.as_deref() == Some(spec) {
                // A punt parks the spec in NeedsAttention — it leaves the
                // queue, exactly like a shipped spec, so the drain advances.
                self.heads.retain(|h| h != spec);
                return punt_result();
            }
            if let Some((dispatched, shipped)) = &self.mismatch {
                if dispatched == spec {
                    // BUG-245: the dispatched head stays in `heads` (the
                    // implementer dequeued `shipped`, not the dispatched
                    // spec). drain_batch is expected to stop without
                    // re-running the dispatched head. trace:BUG-245
                    return mismatch_result(shipped);
                }
            }
            if self.escalate_spec.as_deref() == Some(spec) {
                // An escalation leaves the queue and advances the drain, like
                // a punt — but lands in `escalated`, not `shipped`.
                self.heads.retain(|h| h != spec);
                return escalated_result();
            }
            // Normal success — the completed spec leaves the queue.
            self.heads.retain(|h| h != spec);
            ok_result()
        }

        // TASK-966: cumulative reported tokens = per-spec rate × specs run.
        fn cumulative_tokens(&mut self) -> u64 {
            self.tokens_each * self.runs.len() as u64
        }
    }

    struct MockPipelinedBatchDriver {
        heads: Vec<String>,
        depth: usize,
        next_handle: usize,
        through_ci_results: std::collections::HashMap<usize, OrchestrationResult>,
        through_ci_by_spec: std::collections::HashMap<String, OrchestrationResult>,
        finish_by_spec: std::collections::HashMap<String, OrchestrationResult>,
        events: Vec<String>,
        /// STORY-1429: specs that shelve through CI this many more times; each
        /// shelve is requeued (put back at the head), as a triage would.
        reshelve: std::collections::HashMap<String, usize>,
        handle_spec: std::collections::HashMap<usize, String>,
        delayed_child: bool,
        /// BUG-1930: pass each child result through the real sidecar JSON,
        /// as the production child/parent process boundary does.
        via_sidecar: bool,
        reinsert_on_finish: Option<String>,
    }

    impl MockPipelinedBatchDriver {
        fn new(heads: &[&str], depth: usize) -> Self {
            Self {
                heads: heads.iter().map(|s| s.to_string()).collect(),
                depth,
                next_handle: 1,
                through_ci_results: std::collections::HashMap::new(),
                through_ci_by_spec: std::collections::HashMap::new(),
                finish_by_spec: std::collections::HashMap::new(),
                events: Vec::new(),
                reshelve: std::collections::HashMap::new(),
                handle_spec: std::collections::HashMap::new(),
                delayed_child: false,
                via_sidecar: false,
                reinsert_on_finish: None,
            }
        }

        /// BUG-1930: `spec` stops in phase 1 with this failure kind/reason.
        // trace:BUG-1930 | ai:claude
        fn failing_through_ci_with(mut self, spec: &str, kind: FailureKind, reason: &str) -> Self {
            let mut result = shelve_result(Phase::Implementer);
            result.failure = Some(PhaseFailure::of(kind, reason));
            self.through_ci_by_spec.insert(spec.to_string(), result);
            self
        }

        /// STORY-1429: `spec` shelves through CI `times` times, and each
        /// shelve is requeued to the head of the queue.
        fn shelving_and_requeued(mut self, spec: &str, times: usize) -> Self {
            self.reshelve.insert(spec.to_string(), times);
            self
        }

        fn shelving_through_ci(mut self, spec: &str, phase: Phase) -> Self {
            self.through_ci_by_spec
                .insert(spec.to_string(), shelve_result(phase));
            self
        }

        fn failing_finish(mut self, spec: &str, phase: Phase) -> Self {
            self.finish_by_spec
                .insert(spec.to_string(), fail_result(phase));
            self
        }
    }

    impl BatchDriver for MockPipelinedBatchDriver {
        fn next_head(&mut self) -> Option<String> {
            self.heads.first().cloned()
        }

        // trace:BUG-1980 | ai:claude
        fn stop_requested(&mut self) -> bool {
            false
        }

        fn run_spec(&mut self, spec: &str) -> OrchestrationResult {
            self.events.push(format!("run:{spec}"));
            self.heads.retain(|h| h != spec);
            ok_result()
        }
    }

    impl PipelinedBatchDriver for MockPipelinedBatchDriver {
        fn pipeline_depth(&self) -> usize {
            self.depth
        }

        fn start_spec_through_ci(&mut self, spec: &str) -> PipelinedHandle {
            self.events.push(format!("start:{spec}"));
            if !self.delayed_child {
                self.heads.retain(|h| h != spec);
            }
            let handle = PipelinedHandle(self.next_handle);
            self.next_handle += 1;
            let result = match self.reshelve.get_mut(spec) {
                Some(n) if *n > 0 => {
                    *n -= 1;
                    shelve_result(Phase::Ci)
                }
                _ => self
                    .through_ci_by_spec
                    .remove(spec)
                    .unwrap_or_else(ok_result),
            };
            self.through_ci_results.insert(handle.0, result);
            self.handle_spec.insert(handle.0, spec.to_string());
            handle
        }

        fn wait_spec_through_ci(&mut self, handle: PipelinedHandle) -> OrchestrationResult {
            self.events.push(format!("wait:{}", handle.0));
            if self.delayed_child {
                let spec = &self.handle_spec[&handle.0];
                self.heads.retain(|h| h != spec);
            }
            let result = self
                .through_ci_results
                .remove(&handle.0)
                .unwrap_or_else(|| OrchestrationResult::failed(Phase::Ci));
            // BUG-1930: cross the same JSON boundary a real child does.
            let result = if self.via_sidecar {
                let json = crate::pipelined_child_result_sidecar_json(&result);
                crate::parse_pipelined_child_result_sidecar(&json).expect("sidecar parses")
            } else {
                result
            };
            // STORY-1429: a shelved spec with a pending requeue goes back to
            // the head, as `aida rework` would put it.
            if result.shelved_reason.is_some() {
                if let Some(spec) = self.handle_spec.get(&handle.0).cloned() {
                    if self.reshelve.contains_key(&spec) {
                        self.heads.insert(0, spec);
                    }
                }
            }
            result
        }

        fn finish_spec_after_ci(&mut self, spec: &str) -> OrchestrationResult {
            self.events.push(format!("finish:{spec}"));
            if self.reinsert_on_finish.as_deref() == Some(spec) {
                self.heads.insert(0, spec.to_string());
            }
            self.finish_by_spec.remove(spec).unwrap_or_else(ok_result)
        }
    }

    // Both batch and nextN use this scheduler; nextN adds a launch limit.
    // trace:TASK-1603 | ai:codex
    #[test]
    fn pipelined_delayed_child_does_not_exhaust_batch_or_next_n() {
        for limit in [None, Some(3)] {
            let mut driver = MockPipelinedBatchDriver::new(&["TASK-A", "TASK-B", "TASK-C"], 2);
            driver.delayed_child = true;
            let result = drain_batch_pipelined_with_caps(
                &mut driver,
                limit,
                None,
                &crate::drain_caps::DrainCaps::default(),
                std::time::Instant::now(),
                &mut None,
            );
            assert_eq!(result.shipped, vec!["TASK-A", "TASK-B", "TASK-C"]);
            assert_eq!(
                driver
                    .events
                    .iter()
                    .filter(|e| e.starts_with("start:"))
                    .count(),
                3
            );
        }
    }

    // trace:STORY-1091 trace:ADR-28 | ai:codex
    #[test]
    fn drain_batch_pipelined_starts_next_implementer_before_prior_finish() {
        let mut driver = MockPipelinedBatchDriver::new(&["TASK-A", "TASK-B", "TASK-C"], 2);
        let mut cap_stop = None;
        let result = drain_batch_pipelined_with_caps(
            &mut driver,
            None,
            None,
            &crate::drain_caps::DrainCaps::default(),
            std::time::Instant::now(),
            &mut cap_stop,
        );

        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["TASK-A", "TASK-B", "TASK-C"]);
        assert_eq!(
            driver.events,
            vec![
                "start:TASK-A",
                "start:TASK-B",
                "wait:1",
                "finish:TASK-A",
                "start:TASK-C",
                "wait:2",
                "finish:TASK-B",
                "wait:3",
                "finish:TASK-C",
            ]
        );
    }

    // trace:STORY-1091 trace:ADR-28 | ai:codex
    #[test]
    fn drain_batch_pipelined_shelves_one_member_and_continues_others() {
        let mut driver = MockPipelinedBatchDriver::new(&["TASK-A", "TASK-B", "TASK-C"], 2)
            .shelving_through_ci("TASK-B", Phase::Ci);
        let mut cap_stop = None;
        let result = drain_batch_pipelined_with_caps(
            &mut driver,
            None,
            Some(5),
            &crate::drain_caps::DrainCaps::default(),
            std::time::Instant::now(),
            &mut cap_stop,
        );

        assert_eq!(result.outcome, BatchDrainOutcome::DrainedWithShelved);
        assert_eq!(result.exit_code, DRIVE_EXIT_SHELVED);
        assert_eq!(result.shipped, vec!["TASK-A", "TASK-C"]);
        assert_eq!(result.shelved, vec!["TASK-B"]);
        assert!(driver.events.iter().any(|e| e == "finish:TASK-C"));
    }

    // trace:STORY-1091 trace:ADR-28 | ai:codex
    #[test]
    fn drain_batch_pipelined_serial_failure_does_not_count_member_shipped() {
        let mut driver = MockPipelinedBatchDriver::new(&["TASK-A", "TASK-B"], 2)
            .failing_finish("TASK-A", Phase::Reviewer);
        let mut cap_stop = None;
        let result = drain_batch_pipelined_with_caps(
            &mut driver,
            None,
            None,
            &crate::drain_caps::DrainCaps::default(),
            std::time::Instant::now(),
            &mut cap_stop,
        );

        assert_eq!(result.outcome, BatchDrainOutcome::Failed(Phase::Reviewer));
        assert!(result.shipped.is_empty());
        assert_eq!(result.stopped_at.as_deref(), Some("TASK-A"));
    }

    /// Acceptance: a 3-item batch with every phase green ships all three via
    /// the auto-complete chain.
    #[test]
    fn drain_batch_three_green_ships_all_three() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3"]);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["TASK-1", "TASK-2", "TASK-3"]);
        assert_eq!(result.stopped_at, None);
        assert_eq!(driver.runs, vec!["TASK-1", "TASK-2", "TASK-3"]);
    }

    /// TASK-966: helper to drive [`drain_batch_with_caps`] and recover the
    /// cap-stop reason alongside the result.
    fn drain_with_caps(
        driver: &mut dyn BatchDriver,
        caps: &crate::drain_caps::DrainCaps,
        start: std::time::Instant,
    ) -> (BatchDrainResult, Option<crate::drain_caps::CapStop>) {
        let mut cap_stop = None;
        let result = drain_batch_with_caps(driver, None, None, caps, start, &mut cap_stop);
        (result, cap_stop)
    }

    /// TASK-966: `--max-iterations 2` stops the drain BEFORE the third spec
    /// begins — two ship, the third stays queued, and the stop is a clean
    /// `MaxReached` carrying the iteration cap reason.
    #[test]
    fn drain_caps_max_iterations_stops_before_next_spec() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3"]);
        let caps = crate::drain_caps::DrainCaps {
            max_iterations: Some(2),
            ..Default::default()
        };
        let (result, cap_stop) = drain_with_caps(&mut driver, &caps, std::time::Instant::now());
        assert_eq!(result.outcome, BatchDrainOutcome::MaxReached);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.shipped, vec!["TASK-1", "TASK-2"]);
        // The third spec never ran — it stays queued for the next drain.
        assert_eq!(driver.runs, vec!["TASK-1", "TASK-2"]);
        assert_eq!(
            cap_stop,
            Some(crate::drain_caps::CapStop::Iterations {
                completed: 2,
                cap: 2
            })
        );
    }

    // trace:STORY-1130 | ai:codex
    #[test]
    fn drain_stop_request_stops_before_next_spec() {
        let dir = tempfile::tempdir().unwrap();
        let stop_file = dir.path().join("drain-stop.json");
        // BUG-1980: the stop path belongs to this driver, not the process env,
        // so no parallel drain test can observe it.
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2"]).stop_file(&stop_file);

        let first = drain_batch(&mut driver, Some(1), None);
        assert_eq!(first.shipped, vec!["TASK-1"]);
        assert_eq!(driver.runs, vec!["TASK-1"]);

        std::fs::write(&stop_file, "{}").unwrap();
        let second = drain_batch(&mut driver, None, None);
        assert_eq!(second.outcome, BatchDrainOutcome::MaxReached);
        assert_eq!(second.exit_code, DRIVE_EXIT_CLEAN);
        assert_eq!(driver.runs, vec!["TASK-1"]);
    }

    /// TASK-966: `--max-runtime` with a deadline already in the past stops the
    /// drain between specs — nothing runs, the whole batch stays queued.
    #[test]
    fn drain_caps_max_runtime_stops_between_specs() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2"]);
        let caps = crate::drain_caps::DrainCaps {
            max_runtime: Some(std::time::Duration::from_secs(0)),
            ..Default::default()
        };
        // start in the past → elapsed >= 0s deadline immediately.
        let start = std::time::Instant::now() - std::time::Duration::from_secs(5);
        let (result, cap_stop) = drain_with_caps(&mut driver, &caps, start);
        assert_eq!(result.outcome, BatchDrainOutcome::MaxReached);
        assert!(result.shipped.is_empty());
        assert!(driver.runs.is_empty());
        assert!(matches!(
            cap_stop,
            Some(crate::drain_caps::CapStop::Runtime { .. })
        ));
    }

    /// TASK-966: `--max-tokens` stops the drain once accumulated reported tokens
    /// cross the cap, checked at the spec boundary. Each spec reports 30k tokens;
    /// with a 25k cap the first spec already overshoots, so the drain ships one
    /// then stops before the second.
    #[test]
    fn drain_caps_max_tokens_aborts_once_accumulated_exceeds_cap() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3"]).tokens_each(30_000);
        let caps = crate::drain_caps::DrainCaps {
            max_tokens: Some(25_000),
            ..Default::default()
        };
        let (result, cap_stop) = drain_with_caps(&mut driver, &caps, std::time::Instant::now());
        assert_eq!(result.outcome, BatchDrainOutcome::MaxReached);
        assert_eq!(result.shipped, vec!["TASK-1"]);
        assert_eq!(driver.runs, vec!["TASK-1"]);
        assert_eq!(
            cap_stop,
            Some(crate::drain_caps::CapStop::Tokens {
                used: 30_000,
                cap: 25_000
            })
        );
    }

    #[test]
    fn drain_ignores_stop_request_for_a_different_project_root() {
        // BUG-1980 regression: a stop request for another project's drain
        // (published through the process-wide `AIDA_DRAIN_STOP_FILE`, as a
        // sibling test or `aida drain start` would) must not stop this drain.
        // trace:BUG-1980 | ai:claude
        let other_root = tempfile::tempdir().unwrap();
        let other_stop = crate::drain_cmd::drain_stop_path(other_root.path());
        std::fs::create_dir_all(other_stop.parent().unwrap()).unwrap();
        std::fs::write(&other_stop, "{}").unwrap();
        let _env = crate::test_env::EnvVarsGuard::apply(&[(
            "AIDA_DRAIN_STOP_FILE",
            Some(other_stop.to_string_lossy().as_ref()),
        )]);
        let own_root = tempfile::tempdir().unwrap();
        let own_stop = crate::drain_cmd::drain_stop_path(own_root.path());
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2"]).stop_file(&own_stop);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["TASK-1", "TASK-2"]);
    }

    /// TASK-966: inactive caps leave the drain behaving exactly like the
    /// uncapped `drain_batch` — a clean full drain, no cap stop.
    #[test]
    fn drain_caps_inactive_drains_normally() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2"]).tokens_each(1_000_000);
        let caps = crate::drain_caps::DrainCaps::default();
        let (result, cap_stop) = drain_with_caps(&mut driver, &caps, std::time::Instant::now());
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["TASK-1", "TASK-2"]);
        assert_eq!(cap_stop, None);
    }

    /// Acceptance: a 3-item batch where phase 1 fails on item 2 — item 1
    /// shipped, item 2 stopped the drain, item 3 never ran (queue intact).
    #[test]
    fn drain_batch_phase1_failure_on_item2_leaves_item3_untouched() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3"])
            .failing("TASK-2", Phase::Implementer);
        let result = drain_batch(&mut driver, None, None);
        // TASK-1054: a hard-stop batch failure is the canonical hard-fail code 3,
        // not the failed-phase index (1 here) — which would collide with the
        // empty-batch/stall `1`. trace:TASK-1054 | ai:claude
        assert_eq!(
            result.exit_code, DRIVE_EXIT_HARD_FAIL,
            "hard fail → exit code 3"
        );
        assert_eq!(
            result.outcome,
            BatchDrainOutcome::Failed(Phase::Implementer)
        );
        assert_eq!(result.shipped, vec!["TASK-1"]);
        assert_eq!(result.stopped_at, Some("TASK-2".to_string()));
        // TASK-3 was never run — the queue is intact for a retry.
        assert_eq!(driver.runs, vec!["TASK-1", "TASK-2"]);
    }

    /// TASK-1054: a mid-batch un-shelvable failure stops the drain with the
    /// canonical hard-fail exit code 3, regardless of which phase failed.
    #[test]
    fn drain_batch_hard_failure_exits_with_hard_fail_code() {
        let mut driver =
            MockBatchDriver::new(&["TASK-1", "TASK-2"]).failing("TASK-1", Phase::Reviewer);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, DRIVE_EXIT_HARD_FAIL);
        assert_eq!(result.outcome, BatchDrainOutcome::Failed(Phase::Reviewer));
        assert!(result.shipped.is_empty());
    }

    /// `--max N` stops the drain after N items even when the batch has more.
    #[test]
    fn drain_batch_max_caps_the_drain() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3", "TASK-4"]);
        let result = drain_batch(&mut driver, Some(2), None);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.outcome, BatchDrainOutcome::MaxReached);
        assert_eq!(result.shipped, vec!["TASK-1", "TASK-2"]);
        assert_eq!(driver.runs, vec!["TASK-1", "TASK-2"]);
    }

    /// `--max` equal to the batch size reports `Drained`, not `MaxReached` —
    /// the batch genuinely emptied.
    #[test]
    fn drain_batch_max_equal_to_size_reports_drained() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2"]);
        let result = drain_batch(&mut driver, Some(2), None);
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["TASK-1", "TASK-2"]);
    }

    /// An empty batch drains immediately with nothing shipped.
    #[test]
    fn drain_batch_empty_batch_is_a_clean_drain() {
        let mut driver = MockBatchDriver::new(&[]);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert!(result.shipped.is_empty());
    }

    /// TASK-310: a multi-batch drain exhausts each batch before advancing to
    /// the next batch in the user-declared order.
    #[test]
    fn drain_batch_chain_drains_two_batches_in_order() {
        let names = vec!["alpha".to_string(), "beta".to_string()];
        let result = drain_batch_chain(&names, None, None, |name| match name {
            "alpha" => Box::new(MockBatchDriver::new(&["TASK-A1", "TASK-A2"])),
            "beta" => Box::new(MockBatchDriver::new(&["TASK-B1"])),
            other => panic!("unexpected batch {other}"),
        });

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["TASK-A1", "TASK-A2", "TASK-B1"]);
        assert_eq!(
            result
                .steps
                .iter()
                .map(|s| s.batch_name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
    }

    /// TASK-310: an empty intermediate batch is silently skipped; later
    /// batches still run.
    #[test]
    fn drain_batch_chain_skips_empty_middle_batch() {
        let names = vec!["alpha".to_string(), "empty".to_string(), "beta".to_string()];
        let result = drain_batch_chain(&names, None, None, |name| match name {
            "alpha" => Box::new(MockBatchDriver::new(&["TASK-A1"])),
            "empty" => Box::new(MockBatchDriver::new(&[])),
            "beta" => Box::new(MockBatchDriver::new(&["TASK-B1"])),
            other => panic!("unexpected batch {other}"),
        });

        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["TASK-A1", "TASK-B1"]);
        assert_eq!(result.steps.len(), 3);
        assert!(result.steps[1].result.shipped.is_empty());
    }

    /// TASK-310: a failure in one batch stops the whole chain and does not
    /// advance into later batches.
    #[test]
    fn drain_batch_chain_failure_stops_before_next_batch() {
        let names = vec!["alpha".to_string(), "beta".to_string()];
        let result = drain_batch_chain(&names, None, None, |name| match name {
            "alpha" => Box::new(
                MockBatchDriver::new(&["TASK-A1", "TASK-A2"]).failing("TASK-A2", Phase::Reviewer),
            ),
            "beta" => Box::new(MockBatchDriver::new(&["TASK-B1"])),
            other => panic!("unexpected batch {other}"),
        });

        assert_eq!(result.exit_code, 3);
        assert_eq!(result.outcome, BatchDrainOutcome::Failed(Phase::Reviewer));
        assert_eq!(result.shipped, vec!["TASK-A1"]);
        assert_eq!(result.stopped_batch.as_deref(), Some("alpha"));
        assert_eq!(result.stopped_at.as_deref(), Some("TASK-A2"));
        assert_eq!(result.steps.len(), 1, "beta should never start");
    }

    /// TASK-310: `--max` caps total acted-on specs across all batches, not
    /// per batch.
    #[test]
    fn drain_batch_chain_max_caps_across_batches() {
        let names = vec!["alpha".to_string(), "beta".to_string()];
        let result = drain_batch_chain(&names, Some(3), None, |name| match name {
            "alpha" => Box::new(MockBatchDriver::new(&["TASK-A1", "TASK-A2"])),
            "beta" => Box::new(MockBatchDriver::new(&["TASK-B1", "TASK-B2"])),
            other => panic!("unexpected batch {other}"),
        });

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.outcome, BatchDrainOutcome::MaxReached);
        assert_eq!(result.shipped, vec!["TASK-A1", "TASK-A2", "TASK-B1"]);
        assert_eq!(result.stopped_batch.as_deref(), Some("beta"));
    }

    /// BUG-852: chain aggregation honors the final disposition. A spec shelved
    /// in an earlier batch and shipped in a later retry batch must not remain
    /// in the final shelved list or force exit 2.
    // trace:BUG-852 | ai:codex
    #[test]
    fn drain_batch_chain_retry_then_ship_reports_final_clean_state() {
        let names = vec!["first".to_string(), "retry".to_string()];
        let result = drain_batch_chain(&names, None, None, |name| match name {
            "first" => Box::new(
                MockBatchDriver::new(&["STORY-830"]).shelving("STORY-830", Phase::Reviewer),
            ),
            "retry" => Box::new(MockBatchDriver::new(&["STORY-830"])),
            other => panic!("unexpected batch {other}"),
        });

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["STORY-830"]);
        assert!(result.shelved.is_empty());
    }

    /// STORY-276: a punted member advances the drain — it leaves the queue
    /// (`NeedsAttention`), so the batch keeps going — but it lands in `punted`,
    /// not `shipped`, so the summary does not claim a parked spec shipped.
    #[test]
    fn drain_batch_punted_member_advances_the_drain() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3"]).punting("TASK-2");
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 0, "a punt does not fail the drain");
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["TASK-1", "TASK-3"]);
        assert_eq!(result.punted, vec!["TASK-2"]);
        // Every member ran — the punt did not stall or short-circuit the drain.
        assert_eq!(driver.runs, vec!["TASK-1", "TASK-2", "TASK-3"]);
    }

    /// STORY-306: an escalated member advances the drain like a punt — it
    /// leaves the queue, the batch keeps going — but it lands in `escalated`,
    /// not `shipped`, so the summary does not claim it shipped.
    #[test]
    fn drain_batch_escalated_member_advances_the_drain() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3"]).escalating("TASK-2");
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 0, "an escalation does not fail the drain");
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["TASK-1", "TASK-3"]);
        assert_eq!(result.escalated, vec!["TASK-2"]);
        assert!(result.punted.is_empty());
        assert_eq!(driver.runs, vec!["TASK-1", "TASK-2", "TASK-3"]);
    }

    // --- EPIC-28: shelve on failure + dependency-aware skip --------------

    /// EPIC-28: a shelvable phase failure parks the spec in NeedsAttention
    /// and the drain continues — independents after the failure still ship.
    /// trace:EPIC-28 | ai:claude
    #[test]
    fn drain_batch_shelves_failure_and_continues_to_next_member() {
        let mut driver =
            MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3"]).shelving("TASK-2", Phase::Ci);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(
            result.exit_code, 2,
            "a drain with shelved members exits 2, not the phase code"
        );
        assert_eq!(result.outcome, BatchDrainOutcome::DrainedWithShelved);
        assert_eq!(result.shipped, vec!["TASK-1", "TASK-3"]);
        assert_eq!(result.shelved, vec!["TASK-2"]);
        // TASK-3 must have actually run — the whole point of EPIC-28.
        assert_eq!(driver.runs, vec!["TASK-1", "TASK-2", "TASK-3"]);
    }

    /// BUG-852: a spec can shelve, be retried in the same drain, and then ship.
    /// The final disposition wins: it must appear only in `shipped`, not also
    /// in `shelved`, and the drain exits cleanly because nothing remains parked.
    // trace:BUG-852 | ai:codex
    #[test]
    fn drain_batch_retry_then_ship_removes_prior_shelve() {
        let mut driver = MockBatchDriver::new(&["STORY-830"])
            .shelving_once_then_retry("STORY-830", Phase::Reviewer);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.shipped, vec!["STORY-830"]);
        assert!(result.shelved.is_empty());
        assert_eq!(driver.runs, vec!["STORY-830", "STORY-830"]);
    }

    /// STORY-1429: the failure budget counts shelve EVENTS. A spec that shelves,
    /// is requeued, and shelves again in the same drain spends the budget
    /// twice: with `--max-failures 2` the drain stops on the second shelve
    /// (the canonical hard-fail exit) before touching the next spec, and the
    /// spec is still listed once in `shelved` (final dispositions, BUG-852).
    // trace:STORY-1429 | ai:claude
    #[test]
    fn drain_batch_requeued_spec_reshelve_spends_budget_twice() {
        let mut driver = MockBatchDriver::new(&["STORY-1", "STORY-2"])
            .shelving_once_then_retry("STORY-1", Phase::Ci)
            .shelving_once_then_retry("STORY-1", Phase::Ci);
        let result = drain_batch(&mut driver, None, Some(2));
        assert_eq!(result.exit_code, DRIVE_EXIT_HARD_FAIL);
        assert_eq!(result.outcome, BatchDrainOutcome::Failed(Phase::Ci));
        assert_eq!(result.shelved, vec!["STORY-1"]);
        assert_eq!(result.stopped_at.as_deref(), Some("STORY-1"));
        assert_eq!(
            driver.runs,
            vec!["STORY-1", "STORY-1"],
            "the spent budget must stop the drain before STORY-2"
        );
    }

    /// STORY-1429: the pipelined twin. The re-shelved spec spends the budget
    /// again, the drain stops, and `shelved` still lists it once.
    // trace:STORY-1429 | ai:claude
    #[test]
    fn drain_batch_pipelined_requeued_spec_reshelve_spends_budget_twice() {
        let mut driver = MockPipelinedBatchDriver::new(&["STORY-1", "STORY-2", "STORY-3"], 2)
            .shelving_and_requeued("STORY-1", 2);
        let mut cap_stop = None;
        let result = drain_batch_pipelined_with_caps(
            &mut driver,
            None,
            Some(2),
            &crate::drain_caps::DrainCaps::default(),
            std::time::Instant::now(),
            &mut cap_stop,
        );
        assert_eq!(result.exit_code, DRIVE_EXIT_HARD_FAIL);
        assert_eq!(result.outcome, BatchDrainOutcome::Failed(Phase::Ci));
        assert_eq!(result.shelved, vec!["STORY-1"]);
        let starts = driver
            .events
            .iter()
            .filter(|e| e.as_str() == "start:STORY-1")
            .count();
        assert_eq!(starts, 2, "requeued once, re-run once: {:?}", driver.events);
    }

    /// STORY-1429: without a cap a re-shelve changes nothing: the drain goes
    /// on and reports the final disposition once.
    // trace:STORY-1429 | ai:claude
    #[test]
    fn drain_batch_requeued_spec_reshelve_without_cap_reports_once() {
        let mut driver = MockBatchDriver::new(&["STORY-1", "STORY-2"])
            .shelving_once_then_retry("STORY-1", Phase::Ci)
            .shelving("STORY-1", Phase::Ci);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.outcome, BatchDrainOutcome::DrainedWithShelved);
        assert_eq!(result.shelved, vec!["STORY-1"]);
        assert_eq!(result.shipped, vec!["STORY-2"]);
    }

    /// EPIC-28: when a shelving event also makes a downstream member
    /// un-pickable (BlockedBy → shelved), the dependent is skipped — never
    /// run — and shows up in `skipped`, while independents after it still
    /// ship. trace:EPIC-28 | ai:claude
    #[test]
    fn drain_batch_skips_dependent_when_blocker_shelved() {
        let mut driver = MockBatchDriver::new(&["TASK-A", "TASK-B", "TASK-D", "TASK-E"])
            .shelving("TASK-B", Phase::Ci)
            .dependent("TASK-D", "TASK-B", "blocked-by TASK-B (Needs Attention)");
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 2);
        assert_eq!(result.outcome, BatchDrainOutcome::DrainedWithShelved);
        assert_eq!(result.shipped, vec!["TASK-A", "TASK-E"]);
        assert_eq!(result.shelved, vec!["TASK-B"]);
        // TASK-D must NOT appear in `runs` — it never reached `run_spec`.
        assert!(
            !driver.runs.iter().any(|s| s == "TASK-D"),
            "dependent TASK-D should have been skipped, not run: {:?}",
            driver.runs
        );
        // And it should show up in the mock's skipped list (a real CLI surface
        // would surface the same via `result.skipped`, but that's populated
        // by `resolve_batch_members`, not `drain_batch` itself).
        assert_eq!(
            driver.skipped,
            vec![(
                "TASK-D".to_string(),
                "blocked-by TASK-B (Needs Attention)".to_string()
            )]
        );
    }

    /// BUG-1930: two consecutive specs fail phase 1 on permission denials,
    /// so the drain halts before dispatching the third, exits non-zero, and
    /// reports the environment rather than a per-spec failure.
    // trace:BUG-1930 | ai:claude
    #[test]
    fn drain_batch_halts_after_consecutive_environmental_failures() {
        let denied = "phase 1 stopped: permission denied for Edit, Write";
        let mut driver = MockBatchDriver::new(&["A", "B", "C"])
            .shelving_with("A", FailureKind::PermissionDenied, denied)
            .shelving_with("B", FailureKind::PermissionDenied, denied);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(
            result.outcome,
            BatchDrainOutcome::EnvironmentBlocked {
                cause: "permission-denied"
            }
        );
        assert_eq!(result.exit_code, DRIVE_EXIT_HARD_FAIL);
        assert_eq!(result.shelved, vec!["A", "B"]);
        assert_eq!(result.stopped_at, Some("B".to_string()));
        assert_eq!(driver.runs, vec!["A", "B"], "C must never be dispatched");
        assert_eq!(
            driver.heads,
            vec!["C"],
            "the rest of the queue is untouched"
        );
    }

    /// BUG-1930: a `--batches` chain shares one streak, so one denied spec
    /// per single-spec batch still halts the chain instead of shelving every
    /// batch in turn.
    // trace:BUG-1930 | ai:claude
    #[test]
    fn environment_streak_spans_chain_batches() {
        let names = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let result = drain_batch_chain(&names, None, None, |name| match name {
            "alpha" => Box::new(MockBatchDriver::new(&["A"]).shelving_with(
                "A",
                FailureKind::PermissionDenied,
                "denied",
            )),
            "beta" => Box::new(MockBatchDriver::new(&["B"]).shelving_with(
                "B",
                FailureKind::PermissionDenied,
                "denied",
            )),
            "gamma" => Box::new(MockBatchDriver::new(&["C"])),
            other => panic!("unexpected batch {other}"),
        });
        assert_eq!(
            result.outcome,
            BatchDrainOutcome::EnvironmentBlocked {
                cause: "permission-denied"
            }
        );
        assert_eq!(result.exit_code, DRIVE_EXIT_HARD_FAIL);
        assert_eq!(result.stopped_batch.as_deref(), Some("beta"));
        assert_eq!(result.steps.len(), 2, "gamma should never start");
    }

    /// BUG-1930: the streak only counts CONSECUTIVE same-cause failures — a
    /// different failure in between resets it, and so does a ship.
    // trace:BUG-1930 | ai:claude
    #[test]
    fn environment_streak_needs_consecutive_same_cause_failures() {
        let mut driver = MockBatchDriver::new(&["A", "B", "C", "D"])
            .shelving_with("A", FailureKind::PermissionDenied, "denied")
            .shelving_with("B", FailureKind::NoPr, "no PR opened")
            .shelving_with("C", FailureKind::PermissionDenied, "denied");
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.outcome, BatchDrainOutcome::DrainedWithShelved);
        assert_eq!(driver.runs, vec!["A", "B", "C", "D"]);

        let mut disk = MockBatchDriver::new(&["A", "B", "C"])
            .shelving_with("A", FailureKind::Internal, "No space left on device")
            .shelving_with("B", FailureKind::PermissionDenied, "denied");
        let result = drain_batch(&mut disk, None, None);
        assert_eq!(result.outcome, BatchDrainOutcome::DrainedWithShelved);
        assert_eq!(disk.runs, vec!["A", "B", "C"]);
    }

    /// BUG-1930: at pipeline depth 2, two members that stop on permission
    /// denials halt the drain, and the third member is never launched. The
    /// child results cross the real sidecar JSON, so a sidecar that drops the
    /// failure fails this test.
    // trace:BUG-1930 | ai:claude
    #[test]
    fn drain_batch_pipelined_halts_after_consecutive_environmental_failures() {
        let denied = "phase 1 stopped: permission denied for Edit, Write";
        let mut driver = MockPipelinedBatchDriver::new(&["A", "B", "C"], 2)
            .failing_through_ci_with("A", FailureKind::PermissionDenied, denied)
            .failing_through_ci_with("B", FailureKind::PermissionDenied, denied);
        driver.via_sidecar = true;
        let result = drain_batch_pipelined_with_caps(
            &mut driver,
            None,
            None,
            &crate::drain_caps::DrainCaps::default(),
            std::time::Instant::now(),
            &mut None,
        );
        assert_eq!(
            result.outcome,
            BatchDrainOutcome::EnvironmentBlocked {
                cause: "permission-denied"
            }
        );
        assert_eq!(result.exit_code, DRIVE_EXIT_HARD_FAIL);
        assert_eq!(result.shelved, vec!["A", "B"]);
        assert_eq!(result.stopped_at, Some("B".to_string()));
        assert_eq!(
            driver.events,
            vec!["start:A", "start:B", "wait:1", "wait:2"],
            "C must never be launched"
        );
        assert_eq!(
            driver.heads,
            vec!["C"],
            "the rest of the queue is untouched"
        );
    }

    /// TASK-1351: at `pipeline_depth 3` (or after a ship refills the pipeline
    /// before a failure-budget halt), a member already in flight when the halt
    /// fires is waited on and finishes its parent-serialized review/merge leg
    /// while no further queued specs are launched.
    // trace:TASK-1351 | ai:antigravity
    #[test]
    fn drain_batch_pipelined_halt_finishes_already_in_flight_member() {
        let denied = "phase 1 stopped: permission denied for Edit, Write";
        let mut driver = MockPipelinedBatchDriver::new(&["A", "B", "C", "D"], 3)
            .failing_through_ci_with("A", FailureKind::PermissionDenied, denied)
            .failing_through_ci_with("B", FailureKind::PermissionDenied, denied);
        driver.via_sidecar = true;
        let result = drain_batch_pipelined_with_caps(
            &mut driver,
            None,
            None,
            &crate::drain_caps::DrainCaps::default(),
            std::time::Instant::now(),
            &mut None,
        );
        assert_eq!(
            result.outcome,
            BatchDrainOutcome::EnvironmentBlocked {
                cause: "permission-denied"
            }
        );
        assert_eq!(result.exit_code, DRIVE_EXIT_HARD_FAIL);
        assert_eq!(result.stopped_at, Some("B".to_string()));
        assert_eq!(result.shelved, vec!["A", "B"]);
        assert_eq!(
            result.shipped,
            vec!["C"],
            "already-in-flight C must complete its parent-serialized finish leg"
        );
        assert_eq!(
            driver.events,
            vec!["start:A", "start:B", "start:C", "wait:1", "wait:2", "wait:3", "finish:C"]
        );
        assert_eq!(driver.heads, vec!["D"], "D must never be started");

        // Same shape when a ship refills the pipeline and the next shelve
        // exhausts `--max-failures`: D (already in flight when C shelves)
        // finishes its review/merge leg and E never starts.
        let mut budget_driver = MockPipelinedBatchDriver::new(&["A", "B", "C", "D", "E"], 2)
            .shelving_through_ci("A", Phase::Ci)
            .shelving_through_ci("C", Phase::Ci);
        let budget_result = drain_batch_pipelined_with_caps(
            &mut budget_driver,
            None,
            Some(2),
            &crate::drain_caps::DrainCaps::default(),
            std::time::Instant::now(),
            &mut None,
        );
        assert_eq!(budget_result.outcome, BatchDrainOutcome::Failed(Phase::Ci));
        assert_eq!(budget_result.exit_code, DRIVE_EXIT_HARD_FAIL);
        assert_eq!(budget_result.stopped_at, Some("C".to_string()));
        assert_eq!(budget_result.shelved, vec!["A", "C"]);
        assert_eq!(budget_result.shipped, vec!["B", "D"]);
        assert!(budget_driver.events.iter().any(|e| e == "finish:D"));
        assert!(!budget_driver.events.iter().any(|e| e == "start:E"));
        assert_eq!(budget_driver.heads, vec!["E"]);
    }

    /// TASK-1351: cover `serial_leg_broken` (un-shelvable finish-leg failure),
    /// `Stalled` (shipped head reappears), `--max-failures 0` (`Some(0)`), and
    /// `Mismatched` stops with another member already in flight.
    // trace:TASK-1351 | ai:antigravity
    #[test]
    fn drain_batch_pipelined_halt_covers_serial_broken_stalled_zero_budget_and_mismatched() {
        // 1. Un-shelvable finish-leg failure on A with B in flight: B is waited
        // on, its finish leg is NOT called, and B is recorded in `skipped`.
        let mut broken_driver =
            MockPipelinedBatchDriver::new(&["A", "B", "C"], 2).failing_finish("A", Phase::Merge);
        let broken_result = drain_batch_pipelined_with_caps(
            &mut broken_driver,
            None,
            None,
            &crate::drain_caps::DrainCaps::default(),
            std::time::Instant::now(),
            &mut None,
        );
        assert_eq!(
            broken_result.outcome,
            BatchDrainOutcome::Failed(Phase::Merge)
        );
        assert_eq!(broken_result.exit_code, DRIVE_EXIT_HARD_FAIL);
        assert_eq!(broken_result.stopped_at, Some("A".to_string()));
        assert!(broken_result.shipped.is_empty());
        assert_eq!(
            broken_result.skipped,
            vec![(
                "B".to_string(),
                "serial leg broken — PR left open".to_string()
            )]
        );
        assert_eq!(
            broken_driver.events,
            vec!["start:A", "start:B", "wait:1", "finish:A", "wait:2"],
            "B is waited on, finish:B is skipped, and C is never started"
        );

        // 2. Stalled (shipped head A reappears when refilling pipeline while B
        // is in flight): B is waited on, finish:B is skipped, and B is recorded
        // in `skipped`.
        let mut stalled_driver = MockPipelinedBatchDriver::new(&["A", "B", "C"], 2);
        stalled_driver.reinsert_on_finish = Some("A".to_string());
        let stalled_result = drain_batch_pipelined_with_caps(
            &mut stalled_driver,
            None,
            None,
            &crate::drain_caps::DrainCaps::default(),
            std::time::Instant::now(),
            &mut None,
        );
        assert_eq!(stalled_result.outcome, BatchDrainOutcome::Stalled);
        assert_eq!(stalled_result.exit_code, 1);
        assert_eq!(stalled_result.stopped_at, Some("A".to_string()));
        assert!(stalled_result.shipped.is_empty());
        // trace:TASK-1951 trace:TASK-1615 | ai:claude
        assert_eq!(
            stalled_result.skipped,
            vec![(
                "B".to_string(),
                "drain stalled (queue re-offered a shipped spec) — PR left open".to_string()
            )]
        );
        assert_eq!(
            stalled_driver.events,
            vec!["start:A", "start:B", "wait:1", "finish:A", "wait:2"]
        );

        // 3. `--max-failures 0` (`Some(0)`) with depth 2 where both A and B
        // shelve: B's shelve is still recorded in `result.shelved`.
        let mut zero_driver = MockPipelinedBatchDriver::new(&["A", "B", "C"], 2)
            .shelving_through_ci("A", Phase::Ci)
            .shelving_through_ci("B", Phase::Ci);
        let zero_result = drain_batch_pipelined_with_caps(
            &mut zero_driver,
            None,
            Some(0),
            &crate::drain_caps::DrainCaps::default(),
            std::time::Instant::now(),
            &mut None,
        );
        assert_eq!(zero_result.outcome, BatchDrainOutcome::Failed(Phase::Ci));
        assert_eq!(zero_result.exit_code, DRIVE_EXIT_HARD_FAIL);
        assert_eq!(zero_result.stopped_at, Some("A".to_string()));
        assert_eq!(
            zero_result.shelved,
            vec!["B"],
            "under --max-failures 0, A is stopped_at and in-flight B's shelve is recorded in shelved"
        );
        assert_eq!(zero_driver.heads, vec!["C"]);

        // 4. Mismatched stop (`dispatched != shipped`) on A with B in flight:
        // B is waited on and finishes its serial leg while C is never started.
        let mut mismatch_driver = MockPipelinedBatchDriver::new(&["A", "B", "C"], 2);
        let mut mismatch_res = ok_result();
        mismatch_res.shipped_spec_id = Some("OTHER".to_string());
        mismatch_driver
            .through_ci_by_spec
            .insert("A".to_string(), mismatch_res);
        let mismatch_result = drain_batch_pipelined_with_caps(
            &mut mismatch_driver,
            None,
            None,
            &crate::drain_caps::DrainCaps::default(),
            std::time::Instant::now(),
            &mut None,
        );
        assert_eq!(
            mismatch_result.outcome,
            BatchDrainOutcome::Mismatched {
                dispatched: "A".to_string(),
                shipped: "OTHER".to_string(),
            }
        );
        assert_eq!(mismatch_result.exit_code, 0);
        assert_eq!(mismatch_result.stopped_at, Some("A".to_string()));
        assert_eq!(mismatch_result.shipped, vec!["OTHER", "B"]);
        assert_eq!(
            mismatch_driver.events,
            vec!["start:A", "start:B", "wait:1", "wait:2", "finish:B"]
        );
        assert_eq!(mismatch_driver.heads, vec!["C"]);
    }

    /// TASK-1617: after A's un-shelvable finish latches the halt, in-flight
    /// members whose own results are held or inconclusive are recorded in
    /// `skipped` instead of vanishing from every summary list.
    // trace:TASK-1951 trace:TASK-1617 | ai:claude
    #[test]
    fn drain_batch_pipelined_records_held_and_inconclusive_members_after_a_latched_halt() {
        let mut driver = MockPipelinedBatchDriver::new(&["A", "B", "C", "D"], 3)
            .failing_finish("A", Phase::Merge);
        let mut held = ok_result();
        held.held_reason = Some("held for smoke".to_string());
        driver.through_ci_by_spec.insert("B".to_string(), held);
        let mut inconclusive = ok_result();
        inconclusive.inconclusive_reason = Some("gh api unreachable".to_string());
        driver
            .through_ci_by_spec
            .insert("C".to_string(), inconclusive);
        let result = drain_batch_pipelined_with_caps(
            &mut driver,
            None,
            None,
            &crate::drain_caps::DrainCaps::default(),
            std::time::Instant::now(),
            &mut None,
        );
        assert_eq!(result.outcome, BatchDrainOutcome::Failed(Phase::Merge));
        assert_eq!(result.stopped_at, Some("A".to_string()));
        assert_eq!(
            result.skipped,
            vec![
                (
                    "B".to_string(),
                    "held after the drain halted — PR left open".to_string()
                ),
                (
                    "C".to_string(),
                    "inconclusive after the drain halted — left for retry".to_string()
                ),
            ]
        );
        assert_eq!(driver.heads, vec!["D"]);
    }

    /// TASK-2104: at the serial-finish call site, an in-flight member whose
    /// finish fails un-shelvably after another member's halt latched is
    /// recorded in `skipped` with its failed phase, and the broken serial
    /// leg then skips the next in-flight member.
    // trace:TASK-1951 trace:TASK-2104 | ai:claude
    #[test]
    fn drain_batch_pipelined_records_an_unshelvable_finish_after_a_latched_halt() {
        let mut driver = MockPipelinedBatchDriver::new(&["A", "B", "C", "D"], 3)
            .failing_finish("B", Phase::Merge);
        let mut held = ok_result();
        held.held_reason = Some("held for smoke".to_string());
        driver.through_ci_by_spec.insert("A".to_string(), held);
        let result = drain_batch_pipelined_with_caps(
            &mut driver,
            None,
            None,
            &crate::drain_caps::DrainCaps::default(),
            std::time::Instant::now(),
            &mut None,
        );
        assert_eq!(result.outcome, BatchDrainOutcome::Held);
        assert_eq!(result.stopped_at, Some("A".to_string()));
        assert_eq!(
            result.skipped,
            vec![
                (
                    "B".to_string(),
                    "Merge failed after the drain halted — not shelved".to_string()
                ),
                (
                    "C".to_string(),
                    "serial leg broken — PR left open".to_string()
                ),
            ]
        );
        assert_eq!(driver.heads, vec!["D"]);
    }

    /// TASK-2104 + TASK-2105: every arm of the post-halt recorder, and a
    /// requeued spec's earlier disposition is forgotten rather than kept
    /// beside the new one. A result that shelved its member this time is
    /// already recorded.
    // trace:TASK-1951 trace:TASK-2104 trace:TASK-2105 | ai:claude
    #[test]
    fn post_halt_recorder_covers_every_arm_and_forgets_requeued_dispositions() {
        let done = |outcome: BatchDrainOutcome| BatchDrainResult {
            shipped: Vec::new(),
            punted: Vec::new(),
            escalated: Vec::new(),
            shelved: Vec::new(),
            skipped: Vec::new(),
            stopped_at: Some("B".to_string()),
            outcome,
            exit_code: 0,
        };
        let cases = [
            (
                BatchDrainOutcome::Held,
                "held after the drain halted — PR left open",
            ),
            (
                BatchDrainOutcome::Inconclusive,
                "inconclusive after the drain halted — left for retry",
            ),
            (
                BatchDrainOutcome::Failed(Phase::Merge),
                "Merge failed after the drain halted — not shelved",
            ),
            (
                BatchDrainOutcome::EnvironmentBlocked {
                    cause: "resource-exhaustion",
                },
                "environment blocked (resource-exhaustion) after the drain halted",
            ),
        ];
        for (outcome, reason) in cases {
            // B was shelved earlier in the drain and punted before that
            // (requeued twice), then came back after the halt.
            let (mut shipped, mut escalated) = (Vec::new(), Vec::new());
            let mut punted = vec!["B".to_string()];
            let mut shelved = vec!["B".to_string()];
            let mut skipped = Vec::new();
            record_result_after_latched_halt(
                &done(outcome),
                false,
                &mut shipped,
                &mut punted,
                &mut escalated,
                &mut shelved,
                &mut skipped,
            );
            assert!(punted.is_empty() && shelved.is_empty(), "{reason}");
            assert_eq!(skipped, vec![("B".to_string(), reason.to_string())]);
        }
        let mut shelved = vec!["B".to_string()];
        let mut skipped = Vec::new();
        record_result_after_latched_halt(
            &done(BatchDrainOutcome::Failed(Phase::Ci)),
            true,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut Vec::new(),
            &mut shelved,
            &mut skipped,
        );
        assert_eq!(shelved, vec!["B"]);
        assert!(skipped.is_empty());
    }

    /// BUG-1930: resource exhaustion travels the same pipelined path (its
    /// class comes from the failure reason, which the sidecar must carry).
    // trace:BUG-1930 | ai:claude
    #[test]
    fn drain_batch_pipelined_halts_on_resource_exhaustion_via_sidecar() {
        let disk = "No space left on device";
        let mut driver = MockPipelinedBatchDriver::new(&["A", "B", "C"], 2)
            .failing_through_ci_with("A", FailureKind::Internal, disk)
            .failing_through_ci_with("B", FailureKind::Failed, disk);
        driver.via_sidecar = true;
        let result = drain_batch_pipelined_with_caps(
            &mut driver,
            None,
            None,
            &crate::drain_caps::DrainCaps::default(),
            std::time::Instant::now(),
            &mut None,
        );
        assert_eq!(
            result.outcome,
            BatchDrainOutcome::EnvironmentBlocked {
                cause: "resource-exhaustion"
            }
        );
        assert!(!driver.events.iter().any(|e| e == "start:C"));
    }

    /// BUG-1930: an environmental failure only defers launches while the
    /// in-flight members could complete the streak; a ship in between
    /// reopens the pipeline and every member still runs.
    // trace:BUG-1930 | ai:claude
    #[test]
    fn drain_batch_pipelined_environment_gate_reopens_after_a_ship() {
        let mut driver = MockPipelinedBatchDriver::new(&["A", "B", "C", "D"], 2)
            .failing_through_ci_with("A", FailureKind::PermissionDenied, "denied");
        driver.via_sidecar = true;
        let result = drain_batch_pipelined_with_caps(
            &mut driver,
            None,
            None,
            &crate::drain_caps::DrainCaps::default(),
            std::time::Instant::now(),
            &mut None,
        );
        assert_eq!(result.outcome, BatchDrainOutcome::DrainedWithShelved);
        assert_eq!(result.shelved, vec!["A"]);
        assert_eq!(result.shipped, vec!["B", "C", "D"]);

        let streak = EnvironmentStreak {
            class: Some("permission-denied"),
            count: 1,
        };
        assert!(streak.defers_launch(1, 2));
        assert!(
            !streak.defers_launch(0, 2),
            "never stall with nothing in flight"
        );
        assert!(!streak.defers_launch(1, 3));
        assert!(!streak.defers_launch(1, 0), "halt disabled, no gate");
        assert!(!EnvironmentStreak::default().defers_launch(2, 2));
    }

    /// BUG-1930: `environment_halt_after = 0` disables the halt; the drain
    /// falls back to shelving every failure.
    // trace:BUG-1930 | ai:claude
    #[test]
    fn environment_halt_disabled_by_zero() {
        let mut streak = EnvironmentStreak::default();
        let mut denied = shelve_result(Phase::Implementer);
        denied.failure = Some(PhaseFailure::of(FailureKind::PermissionDenied, "denied"));
        assert_eq!(streak.observe(&denied, 0), None);
        assert_eq!(streak.observe(&denied, 0), None);
        assert_eq!(streak.observe(&denied, 2), Some("permission-denied"));
        assert_eq!(streak.observe(&ok_result(), 2), None);
        assert_eq!(streak.observe(&denied, 2), None, "a ship resets the streak");
    }

    /// BUG-1930: the machine summary carries `halt_reason`, and the halt
    /// threshold is configurable under `[orchestrator]`.
    // trace:BUG-1930 | ai:claude
    #[test]
    fn environment_halt_reason_and_config_key() {
        let blocked = BatchDrainOutcome::EnvironmentBlocked {
            cause: "permission-denied",
        };
        assert_eq!(
            crate::drain_next_n::batch_halt_reason(&blocked),
            serde_json::json!("permission-denied")
        );
        assert_eq!(
            crate::drain_next_n::batch_outcome_label(&blocked),
            "environment-blocked"
        );
        assert!(crate::drain_next_n::batch_halt_reason(&BatchDrainOutcome::Drained).is_null());
        assert!(environment_blocked_message("permission-denied").contains("re-run the drain"));

        let default = crate::orchestrator::OrchestratorConfig::from_toml_str("");
        assert_eq!(
            default.environment_halt_after,
            DEFAULT_ENVIRONMENT_HALT_AFTER
        );
        let cfg = crate::orchestrator::OrchestratorConfig::from_toml_str(
            "[orchestrator]\nenvironment_halt_after = 3\n",
        );
        assert_eq!(cfg.environment_halt_after, 3);
    }

    /// EPIC-28: the `max_failures` safety cap stops the drain when too
    /// many specs shelve in a row — the environment is probably broken.
    /// BUG-1608: `max_failures = 2` allows exactly two shelves; the drain
    /// stops the moment the second lands, before it dispatches a third
    /// member. (It used to dispatch C and only stop on C's failure.)
    // trace:EPIC-28 trace:BUG-1608 | ai:claude
    #[test]
    fn drain_batch_caps_at_max_failures_and_stops() {
        let mut driver = MockBatchDriver::new(&["A", "B", "C", "D", "E"])
            .shelving("A", Phase::Ci)
            .shelving("B", Phase::Ci)
            .shelving("C", Phase::Ci);
        let result = drain_batch(&mut driver, None, Some(2));
        assert_eq!(result.outcome, BatchDrainOutcome::Failed(Phase::Ci));
        // TASK-1054: a hard-stop (failure budget spent) is the canonical
        // hard-fail code 3, NOT the CI phase index 2 — which would collide with
        // the EPIC-28 `2 = shelved` sentinel. trace:TASK-1054 | ai:claude
        assert_eq!(result.exit_code, DRIVE_EXIT_HARD_FAIL);
        assert_eq!(result.shelved, vec!["A", "B"]);
        assert_eq!(result.stopped_at, Some("B".to_string()));
        // C, D and E were never attempted — the spent budget stops the drain.
        assert_eq!(driver.runs, vec!["A", "B"]);
    }

    /// BUG-1608: `--max-failures 1` — the first shelved failure spends the
    /// whole budget, so the drain stops before dispatching another spec.
    // trace:BUG-1608 | ai:claude
    #[test]
    fn drain_batch_max_failures_one_stops_after_first_shelve() {
        let mut driver =
            MockBatchDriver::new(&["STORY-52", "TASK-2", "TASK-3"]).shelving("STORY-52", Phase::Ci);
        let result = drain_batch(&mut driver, None, Some(1));
        assert_eq!(result.outcome, BatchDrainOutcome::Failed(Phase::Ci));
        assert_eq!(result.exit_code, DRIVE_EXIT_HARD_FAIL);
        assert_eq!(result.shelved, vec!["STORY-52"]);
        assert_eq!(result.stopped_at, Some("STORY-52".to_string()));
        assert_eq!(
            driver.runs,
            vec!["STORY-52"],
            "no second spec may launch once the budget of one is spent"
        );
    }

    /// BUG-1608: `--max-failures 0` keeps its documented meaning — the first
    /// failure stops the drain (and is not counted as a tolerated shelve).
    // trace:BUG-1608 | ai:claude
    #[test]
    fn drain_batch_max_failures_zero_stops_at_first_failure() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2"]).shelving("TASK-1", Phase::Ci);
        let result = drain_batch(&mut driver, None, Some(0));
        assert_eq!(result.outcome, BatchDrainOutcome::Failed(Phase::Ci));
        assert_eq!(result.exit_code, DRIVE_EXIT_HARD_FAIL);
        assert!(result.shelved.is_empty());
        assert_eq!(result.stopped_at, Some("TASK-1".to_string()));
        assert_eq!(driver.runs, vec!["TASK-1"]);
    }

    /// BUG-1608: the pipelined scheduler honours the same budget — after the
    /// first shelve with a cap of one it launches nothing further.
    // trace:BUG-1608 | ai:claude
    #[test]
    fn drain_batch_pipelined_max_failures_one_launches_nothing_after_shelve() {
        let mut driver = MockPipelinedBatchDriver::new(&["TASK-A", "TASK-B", "TASK-C"], 2)
            .shelving_through_ci("TASK-A", Phase::Ci);
        let mut cap_stop = None;
        let result = drain_batch_pipelined_with_caps(
            &mut driver,
            None,
            Some(1),
            &crate::drain_caps::DrainCaps::default(),
            std::time::Instant::now(),
            &mut cap_stop,
        );
        assert_eq!(result.outcome, BatchDrainOutcome::Failed(Phase::Ci));
        assert_eq!(result.exit_code, DRIVE_EXIT_HARD_FAIL);
        assert_eq!(result.shelved, vec!["TASK-A"]);
        assert!(
            !driver.events.iter().any(|e| e == "start:TASK-C"),
            "TASK-C must not launch after the budget is spent: {:?}",
            driver.events
        );
    }

    /// EPIC-28: regression — a clean drain (nothing shelved, nothing
    /// skipped) keeps the historical `Drained` outcome + exit 0, even
    /// though `DrainedWithShelved` is now reachable. trace:EPIC-28
    #[test]
    fn drain_batch_clean_drain_without_shelved_still_returns_drained_exit_0() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2"]);
        let result = drain_batch(&mut driver, None, Some(5));
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert!(result.shelved.is_empty());
        assert!(result.skipped.is_empty());
    }

    /// EPIC-28: punts and shelvings can both happen in one drain, and the
    /// summary keeps them sorted — neither leaks into `shipped`.
    /// trace:EPIC-28 | ai:claude
    #[test]
    fn drain_batch_punt_and_shelve_in_same_drain_both_counted_separately() {
        let mut driver = MockBatchDriver::new(&["P", "S", "Q", "R"])
            .punting("P")
            .shelving("S", Phase::Ci);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.outcome, BatchDrainOutcome::DrainedWithShelved);
        assert_eq!(result.exit_code, 2);
        assert_eq!(result.punted, vec!["P"]);
        assert_eq!(result.shelved, vec!["S"]);
        assert_eq!(result.shipped, vec!["Q", "R"]);
    }

    /// EPIC-28: an un-shelvable failure (the mock's default `failing` path —
    /// `shelved_reason: None`) still stops the drain at the historical
    /// `Failed(phase)` outcome. Confirms the routing rule: only shelvable
    /// failures continue. trace:EPIC-28 | ai:claude
    #[test]
    fn drain_batch_unshelvable_failure_still_stops_the_drain() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2", "TASK-3"])
            .failing("TASK-2", Phase::Implementer);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(
            result.outcome,
            BatchDrainOutcome::Failed(Phase::Implementer)
        );
        // TASK-1054: hard-fail code 3 (was the failed-phase index 1).
        assert_eq!(result.exit_code, DRIVE_EXIT_HARD_FAIL);
        assert!(result.shelved.is_empty());
        assert_eq!(result.shipped, vec!["TASK-1"]);
    }

    /// A "successful" run that leaves the head in place is caught by the
    /// non-advancing-queue guard rather than looping forever.
    #[test]
    fn drain_batch_stall_guard_stops_a_non_advancing_queue() {
        let mut driver = MockBatchDriver::new(&["TASK-1", "TASK-2"]).stalling("TASK-2");
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 1);
        assert_eq!(result.outcome, BatchDrainOutcome::Stalled);
        assert_eq!(result.shipped, vec!["TASK-1"]);
        assert_eq!(result.stopped_at, Some("TASK-2".to_string()));
        // TASK-2 ran once, was re-yielded as the head, and the guard fired
        // before a second run.
        assert_eq!(driver.runs, vec!["TASK-1", "TASK-2"]);
    }

    // --- BUG-245: dispatched≠shipped attribution ---------------------------

    /// Acceptance: dispatch phase-1 for STORY-A; phase 1 produces a PR
    /// crediting STORY-B (the implementer pragmatically worked a release
    /// blocker). The orchestrator credits B in the success epilogue and
    /// carries the mismatch into `OrchestrationResult::shipped_spec_id` so
    /// the batch drain records the truth. trace:BUG-245 | ai:claude
    #[test]
    fn orchestrate_credits_shipped_when_pr_mismatches_dispatched() {
        let mut driver = MockPhaseDriver::all_ok().shipping_as("BUG-244");
        let result = orchestrate(
            &mut driver,
            "STORY-276",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.failed_phase, None);
        assert_eq!(result.shipped_spec_id, Some("BUG-244".to_string()));
        // Phases 2-6 still ran on the PR (CI, reviewer, merge, pull, build) —
        // the mismatch only re-attributes the credit, it doesn't truncate
        // the pipeline.
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ]
        );
    }

    /// When phase 1's PR credits the dispatched spec (the common path), no
    /// mismatch fires and the result carries no `shipped_spec_id`. A driver
    /// that explicitly reports the dispatched id behaves the same as one
    /// that returns `None` — both mean "the dispatched spec is what
    /// shipped". trace:BUG-245 | ai:claude
    #[test]
    fn orchestrate_no_mismatch_when_pr_credits_dispatched() {
        // The default driver returns `None` for shipped_spec_id — no
        // mismatch.
        let mut driver = MockPhaseDriver::all_ok();
        let result = orchestrate(
            &mut driver,
            "STORY-276",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.shipped_spec_id, None);

        // A driver that names the dispatched id is also not a mismatch.
        let mut driver = MockPhaseDriver::all_ok().shipping_as("STORY-276");
        let result = orchestrate(
            &mut driver,
            "STORY-276",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.shipped_spec_id, None);
    }

    /// Acceptance test from BUG-245's description: a batch drain whose head
    /// is dispatched but ships a different spec. The drain credits the
    /// actual shipped id in `shipped`, leaves the dispatched id queued
    /// (still in `heads`), and stops with `Mismatched` rather than looping
    /// on the un-advanced head. trace:BUG-245 | ai:claude
    #[test]
    fn drain_batch_credits_actual_shipped_on_mismatch() {
        let mut driver =
            MockBatchDriver::new(&["STORY-276", "TASK-99"]).mismatching("STORY-276", "BUG-244");
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.exit_code, 0, "a mismatch is not a failure");
        assert_eq!(
            result.outcome,
            BatchDrainOutcome::Mismatched {
                dispatched: "STORY-276".to_string(),
                shipped: "BUG-244".to_string(),
            }
        );
        // The truth — BUG-244 — is what `shipped` records, not STORY-276.
        assert_eq!(result.shipped, vec!["BUG-244"]);
        // The dispatched spec is reported as where the drain stopped.
        assert_eq!(result.stopped_at, Some("STORY-276".to_string()));
        // STORY-276 is still queued — the drain did not run TASK-99 either,
        // it stopped at the mismatched head for operator inspection.
        assert_eq!(driver.runs, vec!["STORY-276"]);
        assert!(driver.heads.contains(&"STORY-276".to_string()));
    }

    /// Self-consistency invariant from BUG-245's acceptance: the drain
    /// summary must never claim a spec shipped *and* report it stayed at
    /// the head in the same run. The mismatch outcome credits the actual
    /// shipped id (not the dispatched one) in `shipped`, so a reader cannot
    /// see "X shipped" alongside "X stayed at head" for the same X.
    /// trace:BUG-245 | ai:claude
    #[test]
    fn drain_batch_mismatch_summary_is_self_consistent() {
        let mut driver = MockBatchDriver::new(&["STORY-276"]).mismatching("STORY-276", "BUG-244");
        let result = drain_batch(&mut driver, None, None);
        // The dispatched head appears in `stopped_at` (queue did not
        // advance) — it must NOT also appear in `shipped`.
        let dispatched = result.stopped_at.as_deref().unwrap();
        assert!(
            !result.shipped.iter().any(|s| s == dispatched),
            "dispatched spec must not be credited as shipped"
        );
        // And `shipped` must carry the spec the PR actually shipped.
        assert!(
            result.shipped.iter().any(|s| s == "BUG-244"),
            "actual shipped spec must be credited"
        );
    }

    // --- Single-branch coupled-sequential drain (TASK-1003 / SPIKE-70) ------

    /// Mock [`SingleBranchDriver`]. Models a shared feature branch as a growing
    /// `branch_commits` log: `run_member_through_ci` appends the member (and
    /// pops it from the queue so the head advances), so after N clean members
    /// the log is `[m1, m2, …, mN]`. A reset between members WOULD truncate that
    /// log — it never does, which is exactly what the no-reset test asserts.
    /// `run_cluster_finish` records the one-PR member list and the merge's
    /// Done→Completed bump set; `cluster_finishes` counts cluster merges (must be
    /// exactly 1, and 0 if any member failed — proving no intermediate merge).
    struct MockSingleBranch {
        /// Members still queued (front = head); a member is popped on a clean run.
        queue: Vec<String>,
        /// Members configured to FAIL their through-CI run, by id.
        fail: std::collections::HashSet<String>,
        /// Members whose through-CI run PUNTS (clean exit, no ship), by id.
        punt: std::collections::HashSet<String>,
        /// Commits accumulated on the shared branch, in order (no-reset witness).
        branch_commits: Vec<String>,
        /// How many times the cluster Reviewer/Merge ran (no-intermediate-merge
        /// witness — must be 0 or 1).
        cluster_finishes: usize,
        /// The member ids handed to the single cluster PR.
        cluster_members: Vec<String>,
        /// The members the single cluster merge bumped Done→Completed.
        completed: Vec<String>,
        /// `(prev, next)` pairs the per-increment checkpoint fired on.
        checkpoints: Vec<(String, String)>,
        /// If set, the checkpoint returns `false` (stop) when `next` matches.
        stop_before: Option<String>,
    }

    impl MockSingleBranch {
        fn new(members: &[&str]) -> Self {
            Self {
                queue: members.iter().map(|s| s.to_string()).collect(),
                fail: std::collections::HashSet::new(),
                punt: std::collections::HashSet::new(),
                branch_commits: Vec::new(),
                cluster_finishes: 0,
                cluster_members: Vec::new(),
                completed: Vec::new(),
                checkpoints: Vec::new(),
                stop_before: None,
            }
        }
        fn failing(mut self, spec: &str) -> Self {
            self.fail.insert(spec.to_string());
            self
        }
        fn punting(mut self, spec: &str) -> Self {
            self.punt.insert(spec.to_string());
            self
        }
        fn stop_before(mut self, spec: &str) -> Self {
            self.stop_before = Some(spec.to_string());
            self
        }
    }

    impl SingleBranchDriver for MockSingleBranch {
        fn next_head(&mut self) -> Option<String> {
            self.queue.first().cloned()
        }
        fn run_member_through_ci(&mut self, spec: &str) -> OrchestrationResult {
            if self.fail.contains(spec) {
                // Phase-2 (CI) failure — member NOT committed, NOT popped.
                return OrchestrationResult::failed(Phase::Ci);
            }
            if self.punt.contains(spec) {
                // A clean exit (0) but the member did NOT ship — a design-fork
                // punt. The coupled drain must HALT here too. Member not popped.
                let mut r = OrchestrationResult::ok();
                r.punt_reason = Some("design fork".to_string());
                return r;
            }
            // Clean run: the member's commit lands on the shared branch and it
            // leaves the queue (marked Done). NO reset — the prior members'
            // commits stay in the log.
            self.branch_commits.push(spec.to_string());
            self.queue.retain(|s| s != spec);
            OrchestrationResult::ok()
        }
        fn run_cluster_finish(&mut self, members: &[String]) -> OrchestrationResult {
            self.cluster_finishes += 1;
            self.cluster_members = members.to_vec();
            // The ONE cluster merge bumps EVERY member Done→Completed (each
            // member's own `(SPEC-ID)` commit trailer is credited by the pull
            // scan). Model that by completing every linked member.
            self.completed = members.to_vec();
            OrchestrationResult::ok()
        }
        fn checkpoint_between_members(&mut self, prev: &str, next: &str) -> bool {
            self.checkpoints.push((prev.to_string(), next.to_string()));
            self.stop_before.as_deref() != Some(next)
        }
    }

    /// Each member commits in place on the ONE shared branch and the cluster
    /// Reviewer/Merge runs exactly ONCE at the end — there is NO merge-to-main
    /// between members. trace:TASK-1003 | ai:claude
    #[test]
    fn single_branch_commits_each_member_no_intermediate_merge() {
        let mut m = MockSingleBranch::new(&["A", "B", "C"]);
        let r = drain_batch_single_branch(&mut m, None);
        assert_eq!(r.outcome, SingleBranchOutcome::Clustered);
        // All three commits accumulated on the one branch, in order.
        assert_eq!(m.branch_commits, vec!["A", "B", "C"]);
        assert_eq!(r.committed, vec!["A", "B", "C"]);
        // The cluster merge ran ONCE — not once per member.
        assert_eq!(m.cluster_finishes, 1);
    }

    /// A member failure HALTS the drain (NOT EPIC-28 shelve-and-continue): the
    /// prior member's commit stays on the branch, the failed member is the stop
    /// point, and NO cluster PR is opened. trace:TASK-1003 | ai:claude
    #[test]
    fn single_branch_halts_on_member_failure_keeps_prior_commits() {
        let mut m = MockSingleBranch::new(&["A", "B", "C"]).failing("B");
        let r = drain_batch_single_branch(&mut m, None);
        // A committed; B failed; C never ran — accumulated work is kept.
        assert_eq!(m.branch_commits, vec!["A"]);
        assert_eq!(r.committed, vec!["A"]);
        assert_eq!(r.stopped_at.as_deref(), Some("B"));
        assert!(matches!(r.outcome, SingleBranchOutcome::Halted(Phase::Ci)));
        // No cluster PR — the branch is parked for triage, not shipped.
        assert_eq!(m.cluster_finishes, 0);
        assert!(r.cluster_members.is_empty());
        assert_ne!(r.exit_code, 0);
    }

    /// A mid-cluster punt (clean exit, no ship) also HALTS — a coupled drain
    /// cannot stack later members on a branch missing this one's work.
    /// trace:TASK-1003 | ai:claude
    #[test]
    fn single_branch_halts_on_member_punt_keeps_prior_commits() {
        let mut m = MockSingleBranch::new(&["A", "B", "C"]).punting("B");
        let r = drain_batch_single_branch(&mut m, None);
        assert_eq!(m.branch_commits, vec!["A"]);
        assert_eq!(r.stopped_at.as_deref(), Some("B"));
        assert!(matches!(r.outcome, SingleBranchOutcome::Halted(_)));
        assert_eq!(m.cluster_finishes, 0);
    }

    /// All members ship on ONE branch and exactly ONE cluster PR links every
    /// member SPEC-ID. trace:TASK-1003 | ai:claude
    #[test]
    fn single_branch_opens_one_cluster_pr_linking_all_members() {
        let mut m = MockSingleBranch::new(&["A", "B", "C"]);
        let r = drain_batch_single_branch(&mut m, None);
        assert_eq!(m.cluster_finishes, 1);
        // The one PR links every member.
        assert_eq!(m.cluster_members, vec!["A", "B", "C"]);
        assert_eq!(r.cluster_members, vec!["A", "B", "C"]);
    }

    /// The single cluster merge credits ALL member ids — every member is bumped
    /// Done→Completed on the one merge, not just one. trace:TASK-1003 | ai:claude
    #[test]
    fn single_branch_cluster_merge_bumps_all_members_completed() {
        let mut m = MockSingleBranch::new(&["A", "B", "C"]);
        let _ = drain_batch_single_branch(&mut m, None);
        // The one merge's Done→Completed scan credited every member.
        assert_eq!(m.completed, vec!["A", "B", "C"]);
    }

    /// No reset between members: each member's commit STACKS on the shared
    /// branch. After three members the branch retains every prior member's
    /// commit — a BUG-554-style reset between members would have truncated the
    /// log to just the latest. trace:TASK-1003 BUG-554 | ai:claude
    #[test]
    fn single_branch_no_reset_between_members() {
        let mut m = MockSingleBranch::new(&["A", "B", "C"]);
        let _ = drain_batch_single_branch(&mut m, None);
        // Every member's commit is still present and ordered — proof the BUG-554
        // "reset between members" rule is suppressed in single-branch mode.
        assert_eq!(m.branch_commits, vec!["A", "B", "C"]);
        // The checkpoint fired between members (B after A, C after B), never
        // before the first — and never reset.
        assert_eq!(
            m.checkpoints,
            vec![
                ("A".to_string(), "B".to_string()),
                ("B".to_string(), "C".to_string()),
            ]
        );
    }

    /// The per-increment checkpoint can stop the drain before the next member:
    /// prior commits are intact, the remaining members are not started, and NO
    /// cluster PR opens (the operator validates before continuing).
    /// trace:TASK-1003 | ai:claude
    #[test]
    fn single_branch_checkpoint_stop_parks_with_prior_commits() {
        let mut m = MockSingleBranch::new(&["A", "B", "C"]).stop_before("B");
        let r = drain_batch_single_branch(&mut m, None);
        assert!(matches!(r.outcome, SingleBranchOutcome::Paused));
        assert_eq!(r.exit_code, 0);
        // A landed; the checkpoint stopped before B, so B/C never ran.
        assert_eq!(m.branch_commits, vec!["A"]);
        assert_eq!(r.stopped_at.as_deref(), Some("B"));
        assert_eq!(m.cluster_finishes, 0);
    }

    // --- Per-member checkpoint: zen pauses / headless continues (TASK-1137) --

    /// Fake driver whose checkpoint routes through the SAME
    /// [`single_branch_checkpoint_action`] decision the real driver uses, so the
    /// zen-pauses / headless-continues behavior is exercised through the real
    /// drain loop without ever touching stdin. `operator_answer` stands in for
    /// what a human would type at the prompt.
    // trace:TASK-1137 | ai:claude
    struct CheckpointFake {
        queue: Vec<String>,
        branch_commits: Vec<String>,
        cluster_finishes: usize,
        /// The drain's headless mode (`None` = a human is driving).
        no_human: Option<NoHumanMode>,
        /// Whether the process has an answerable terminal.
        interactive: bool,
        /// What the operator would answer AT a prompt — `false` stops the drain.
        operator_answer: bool,
        /// How many times the checkpoint actually prompted. Must be 0 whenever
        /// the run is headless: a prompt there would hang forever.
        prompts: usize,
    }

    impl CheckpointFake {
        fn new(members: &[&str], no_human: Option<NoHumanMode>, interactive: bool) -> Self {
            Self {
                queue: members.iter().map(|s| s.to_string()).collect(),
                branch_commits: Vec::new(),
                cluster_finishes: 0,
                no_human,
                interactive,
                operator_answer: true,
                prompts: 0,
            }
        }
        /// The operator declines at the first checkpoint.
        fn operator_declines(mut self) -> Self {
            self.operator_answer = false;
            self
        }
    }

    impl SingleBranchDriver for CheckpointFake {
        fn next_head(&mut self) -> Option<String> {
            self.queue.first().cloned()
        }
        fn run_member_through_ci(&mut self, spec: &str) -> OrchestrationResult {
            self.branch_commits.push(spec.to_string());
            self.queue.retain(|s| s != spec);
            OrchestrationResult::ok()
        }
        fn run_cluster_finish(&mut self, _members: &[String]) -> OrchestrationResult {
            self.cluster_finishes += 1;
            OrchestrationResult::ok()
        }
        fn checkpoint_between_members(&mut self, _prev: &str, _next: &str) -> bool {
            match single_branch_checkpoint_action(self.no_human, self.interactive) {
                CheckpointAction::AutoContinue => true,
                CheckpointAction::Prompt => {
                    self.prompts += 1;
                    self.operator_answer
                }
            }
        }
    }

    /// `--zen` (a human at the keyboard, no headless mode) PAUSES between
    /// members: the operator is asked before the next member stacks commits on
    /// the shared branch, and declining parks the drain cleanly with prior
    /// commits intact and no cluster PR.
    // trace:TASK-1137 | ai:claude
    #[test]
    fn checkpoint_zen_pauses_between_members() {
        let mut d = CheckpointFake::new(&["A", "B", "C"], None, true).operator_declines();
        let r = drain_batch_single_branch(&mut d, None);
        // The operator WAS asked, exactly once — before B stacked on A.
        assert_eq!(d.prompts, 1);
        assert_eq!(r.outcome, SingleBranchOutcome::Paused);
        // Clean stop: A's commit is kept, B/C never ran, no PR opened.
        assert_eq!(r.exit_code, 0);
        assert_eq!(d.branch_commits, vec!["A"]);
        assert_eq!(r.stopped_at.as_deref(), Some("B"));
        assert_eq!(d.cluster_finishes, 0);
        assert!(r.cluster_members.is_empty());
    }

    /// A `--zen` run whose operator keeps saying yes is asked at every member
    /// boundary and still reaches the ONE cluster PR.
    // trace:TASK-1137 | ai:claude
    #[test]
    fn checkpoint_zen_prompts_each_boundary_then_clusters() {
        let mut d = CheckpointFake::new(&["A", "B", "C"], None, true);
        let r = drain_batch_single_branch(&mut d, None);
        // Two boundaries for three members — never before the first.
        assert_eq!(d.prompts, 2);
        assert_eq!(r.outcome, SingleBranchOutcome::Clustered);
        assert_eq!(d.branch_commits, vec!["A", "B", "C"]);
        assert_eq!(d.cluster_finishes, 1);
    }

    /// `--no-human=both` AUTO-CONTINUES: the checkpoint never prompts, so a
    /// fully headless drain cannot block forever on a question nobody can
    /// answer — even though this operator would have declined.
    // trace:TASK-1137 | ai:claude
    #[test]
    fn checkpoint_headless_both_auto_continues() {
        let mut d = CheckpointFake::new(&["A", "B", "C"], Some(NoHumanMode::Both), false)
            .operator_declines();
        let r = drain_batch_single_branch(&mut d, None);
        // NOT ONE prompt — the whole point of the headless branch.
        assert_eq!(d.prompts, 0);
        assert_eq!(r.outcome, SingleBranchOutcome::Clustered);
        assert_eq!(d.branch_commits, vec!["A", "B", "C"]);
        assert_eq!(d.cluster_finishes, 1);
    }

    /// The no-TTY guard is independent of the autonomy flags: a drain piped into
    /// a script has nobody to answer, so it auto-continues even with no headless
    /// mode set.
    // trace:TASK-1137 | ai:claude
    #[test]
    fn checkpoint_without_a_terminal_auto_continues() {
        let mut d = CheckpointFake::new(&["A", "B"], None, false).operator_declines();
        let r = drain_batch_single_branch(&mut d, None);
        assert_eq!(d.prompts, 0);
        assert_eq!(r.outcome, SingleBranchOutcome::Clustered);
        assert_eq!(d.branch_commits, vec!["A", "B"]);
    }

    /// The decision table itself: only `--no-human=both` or a missing terminal
    /// auto-continue; every human-present mode prompts.
    // trace:TASK-1137 | ai:claude
    #[test]
    fn checkpoint_action_table() {
        // Human at the keyboard (plain interactive drain, or `--zen`) → pause.
        assert_eq!(
            single_branch_checkpoint_action(None, true),
            CheckpointAction::Prompt
        );
        // `--no-human=reviewer-only` still has a human driving the implementer.
        assert_eq!(
            single_branch_checkpoint_action(Some(NoHumanMode::ReviewerOnly), true),
            CheckpointAction::Prompt
        );
        // Fully headless → never ask.
        assert_eq!(
            single_branch_checkpoint_action(Some(NoHumanMode::Both), true),
            CheckpointAction::AutoContinue
        );
        // No terminal → never ask, whatever the flags say.
        assert_eq!(
            single_branch_checkpoint_action(None, false),
            CheckpointAction::AutoContinue
        );
        assert_eq!(
            single_branch_checkpoint_action(Some(NoHumanMode::ReviewerOnly), false),
            CheckpointAction::AutoContinue
        );
    }

    /// An empty batch drives nothing — no branch, no cluster PR, clean exit.
    /// trace:TASK-1003 | ai:claude
    #[test]
    fn single_branch_empty_batch_is_clean_noop() {
        let mut m = MockSingleBranch::new(&[]);
        let r = drain_batch_single_branch(&mut m, None);
        assert_eq!(r.outcome, SingleBranchOutcome::Empty);
        assert_eq!(r.exit_code, 0);
        assert_eq!(m.cluster_finishes, 0);
        assert!(r.committed.is_empty());
    }

    // =====================================================================
    // STORY-749: declarative scenario library on a single ScenarioDriver
    // =====================================================================
    //
    // CONVENTION — read this before adding a scenario:
    //
    // A *scenario* is DATA. A `Scenario` value names the outcome each
    // `PhaseDriver` method should return; the single `ScenarioDriver` replays it
    // against the REAL `orchestrate*` loop. Adding a case means adding a fixture
    // (a named `fn` under `mod scenarios`), NOT writing a new `impl PhaseDriver`.
    //
    //  - WHERE FIXTURES LIVE: `mod scenarios` below — one `fn name() -> Scenario`
    //    per branch. Build with `Scenario::builder()` + its fluent setters.
    //  - REPLAY MODEL: every phase field is a SEQUENCE consumed one entry per
    //    call (the last entry repeats once exhausted). A single-entry sequence
    //    always returns that entry; a multi-entry one advances across calls — so
    //    a phase can return "red then green" across two `orchestrate()` runs on
    //    the SAME driver. That is how the CI-retry / reviewer-fix scenarios work.
    //    An empty sequence yields the phase's clean default (PR opened / Ok /
    //    Approved).
    //  - HOW TO ASSERT AN OUTCOME: run the scenario through `orchestrate(...)`
    //    (single-spec) or a `ScenarioBatchDriver` + `drain_batch(...)` (batch),
    //    then assert on the RESULTING orchestrator outcome — shelved-vs-shipped,
    //    escalation recorded, process/batch exit code, phases run — NOT merely
    //    the phase return value. `scenario_happy_path_ships` is the worked
    //    example.
    //  - GROWTH MODEL (PRIN-2): the seed set is a STARTING point. Grow it from
    //    real incidents — every real drain failure becomes a new regression
    //    fixture — not by enumerating the cartesian product up front.

    /// One programmed outcome for a phase returning `ImplementerOutcome`
    /// (phase-1 `run_implementer` and the advisor-resume `resume_implementer`).
    #[derive(Clone)]
    enum ImplStep {
        /// A PR was opened — the pipeline continues to CI.
        Pr,
        /// The headless implementer punted on a design-fork.
        Punt(String),
        /// The post-implementer PR lookup was inconclusive (transient API blip).
        Inconclusive(String),
        /// The implementer deliberately held the PR for a manual gate.
        Held(String),
        /// The phase failed with this kind + reason.
        Fail(FailureKind, String),
    }

    /// One programmed outcome for a `Result<(), PhaseFailure>` gate phase
    /// (`finish_ci`, `merge`, `pull`, `build`).
    #[derive(Clone)]
    enum GateStep {
        Ok,
        Fail(FailureKind, String),
    }

    /// One programmed outcome for phase-3 `run_reviewer`.
    #[derive(Clone)]
    enum ReviewStep {
        Verdict(Verdict),
        Escalate(String),
        Fail(FailureKind, String),
    }

    /// One programmed outcome for the STORY-306 advisor tier `run_advisor`.
    #[derive(Clone)]
    enum AdvisorStep {
        Resolve(String),
        Escalate(String),
    }

    /// A declarative drain scenario: DATA describing the outcome each
    /// `PhaseDriver` method returns, replayed by `ScenarioDriver`. Every phase
    /// field is a sequence consumed one entry per call (the last entry repeats),
    /// so a scenario can encode a multi-run retry ("CI red then green"). Empty
    /// ⇒ the phase's clean default.
    // trace:STORY-749 | ai:claude
    #[derive(Clone, Default)]
    struct Scenario {
        implementer: Vec<ImplStep>,
        finish_ci: Vec<GateStep>,
        reviewer: Vec<ReviewStep>,
        merge: Vec<GateStep>,
        pull: Vec<GateStep>,
        build: Vec<GateStep>,
        advisor: Vec<AdvisorStep>,
        resume: Vec<ImplStep>,
        /// When set, `terminal_status` reports the spec already-terminal — the
        /// orchestrator finishes as a clean NO-OP without a phase running.
        terminal: Option<&'static str>,
        /// BUG-245: what `shipped_spec_id` returns (a PR-credit mismatch).
        shipped_spec_id: Option<String>,
        /// BUG-241: what `reconcile_failure` returns (default GenuineFailure).
        reconcile: Option<PhaseReconcile>,
        /// EPIC-28: when true, `shelve_on_failure` parks the spec (returns a
        /// `FailureReason`) the way a real store-backed driver does — so a
        /// shelvable failure sets `shelved_reason` and the drive exits `2`.
        shelve_succeeds: bool,
    }

    impl Scenario {
        fn builder() -> Self {
            Self::default()
        }
        fn implementer(mut self, steps: Vec<ImplStep>) -> Self {
            self.implementer = steps;
            self
        }
        fn finish_ci(mut self, steps: Vec<GateStep>) -> Self {
            self.finish_ci = steps;
            self
        }
        fn reviewer(mut self, steps: Vec<ReviewStep>) -> Self {
            self.reviewer = steps;
            self
        }
        fn build(mut self, steps: Vec<GateStep>) -> Self {
            self.build = steps;
            self
        }
        fn advisor(mut self, steps: Vec<AdvisorStep>) -> Self {
            self.advisor = steps;
            self
        }
        fn resume(mut self, steps: Vec<ImplStep>) -> Self {
            self.resume = steps;
            self
        }
        /// EPIC-28: a shelvable phase failure parks the spec (NeedsAttention)
        /// instead of leaving the failure un-shelved.
        fn shelves(mut self) -> Self {
            self.shelve_succeeds = true;
            self
        }
    }

    /// Pop the next programmed step: the entry at `*i`, or the last entry once
    /// the sequence is exhausted (so a single-entry sequence always returns that
    /// entry, and a multi-entry one advances across calls — the retry
    /// mechanism). `None` only for an empty sequence ⇒ the phase's clean
    /// default.
    // trace:STORY-749 | ai:claude
    fn next_step<T: Clone>(steps: &[T], i: &mut usize) -> Option<T> {
        if steps.is_empty() {
            return None;
        }
        let idx = (*i).min(steps.len() - 1);
        *i += 1;
        Some(steps[idx].clone())
    }

    /// Map an `ImplStep` onto the driver's `ImplementerOutcome` result.
    fn impl_outcome(step: ImplStep) -> Result<ImplementerOutcome, PhaseFailure> {
        match step {
            ImplStep::Pr => Ok(ImplementerOutcome::PrOpened),
            ImplStep::Punt(reason) => Ok(ImplementerOutcome::Punted { reason }),
            ImplStep::Inconclusive(reason) => Ok(ImplementerOutcome::Inconclusive {
                reason,
                retry_hint: None,
            }),
            ImplStep::Held(reason) => Ok(ImplementerOutcome::Held {
                reason: Some(reason),
                branch: "held-branch".to_string(),
            }),
            ImplStep::Fail(kind, reason) => Err(PhaseFailure::of(kind, reason)),
        }
    }

    /// Map a `GateStep` onto a gate phase's `Result<(), PhaseFailure>`.
    fn gate_outcome(step: Option<GateStep>) -> Result<(), PhaseFailure> {
        match step {
            None | Some(GateStep::Ok) => Ok(()),
            Some(GateStep::Fail(kind, reason)) => Err(PhaseFailure::of(kind, reason)),
        }
    }

    /// The single declarative-scenario driver: replays a `Scenario` against the
    /// real `orchestrate*` loop. One driver, N scenarios — adding a case is
    /// data, not a new `impl PhaseDriver`.
    // trace:STORY-749 | ai:claude
    struct ScenarioDriver {
        sc: Scenario,
        calls: Vec<Phase>,
        advisor_calls: usize,
        mark_escalated_calls: usize,
        impl_i: usize,
        ci_i: usize,
        rev_i: usize,
        merge_i: usize,
        pull_i: usize,
        build_i: usize,
        resume_i: usize,
    }

    impl ScenarioDriver {
        fn new(sc: Scenario) -> Self {
            Self {
                sc,
                calls: Vec::new(),
                advisor_calls: 0,
                mark_escalated_calls: 0,
                impl_i: 0,
                ci_i: 0,
                rev_i: 0,
                merge_i: 0,
                pull_i: 0,
                build_i: 0,
                resume_i: 0,
            }
        }
    }

    impl PhaseDriver for ScenarioDriver {
        fn run_implementer(&mut self) -> Result<ImplementerOutcome, PhaseFailure> {
            assert!(
                self.sc.terminal.is_none(),
                "run_implementer must never be called for a terminal-status spec",
            );
            self.calls.push(Phase::Implementer);
            match next_step(&self.sc.implementer, &mut self.impl_i) {
                None => Ok(ImplementerOutcome::PrOpened),
                Some(step) => impl_outcome(step),
            }
        }
        fn finish_ci(&mut self) -> Result<(), PhaseFailure> {
            self.calls.push(Phase::Ci);
            gate_outcome(next_step(&self.sc.finish_ci, &mut self.ci_i))
        }
        fn run_reviewer(&mut self) -> Result<ReviewerOutcome, PhaseFailure> {
            self.calls.push(Phase::Reviewer);
            match next_step(&self.sc.reviewer, &mut self.rev_i) {
                None => Ok(ReviewerOutcome::Verdict(Verdict::Approved)),
                Some(ReviewStep::Verdict(v)) => Ok(ReviewerOutcome::Verdict(v)),
                Some(ReviewStep::Escalate(reason)) => {
                    Ok(ReviewerOutcome::EscalatedToHuman { reason })
                }
                Some(ReviewStep::Fail(kind, reason)) => Err(PhaseFailure::of(kind, reason)),
            }
        }
        fn merge(&mut self) -> Result<(), PhaseFailure> {
            self.calls.push(Phase::Merge);
            gate_outcome(next_step(&self.sc.merge, &mut self.merge_i))
        }
        fn pull(&mut self) -> Result<(), PhaseFailure> {
            self.calls.push(Phase::Pull);
            gate_outcome(next_step(&self.sc.pull, &mut self.pull_i))
        }
        fn build(&mut self) -> Result<(), PhaseFailure> {
            self.calls.push(Phase::Build);
            gate_outcome(next_step(&self.sc.build, &mut self.build_i))
        }
        fn hint_context(&self) -> HintContext {
            HintContext {
                spec: "TASK-247".to_string(),
                branch: Some("task-247".to_string()),
                pr_number: Some(46),
                implementer_session: Some("019e2f423e7c".to_string()),
                ci_run_id: Some("9988776655".to_string()),
                forge: crate::forge::ForgeKind::GitHub,
            }
        }
        fn reconcile_failure(&mut self, _phase: Phase, _failure: &PhaseFailure) -> PhaseReconcile {
            self.sc
                .reconcile
                .clone()
                .unwrap_or(PhaseReconcile::GenuineFailure)
        }
        fn terminal_status(&mut self) -> Option<&'static str> {
            self.sc.terminal
        }
        fn shipped_spec_id(&mut self) -> Option<String> {
            self.sc.shipped_spec_id.clone()
        }
        fn run_advisor(&mut self) -> Result<AdvisorOutcome, PhaseFailure> {
            self.advisor_calls += 1;
            match self.sc.advisor.first().cloned() {
                Some(AdvisorStep::Resolve(answer)) => Ok(AdvisorOutcome::Resolved {
                    answer,
                    reasoning: "scenario advisor reasoning".to_string(),
                }),
                Some(AdvisorStep::Escalate(reason)) => Ok(AdvisorOutcome::Escalated {
                    reason,
                    category: "strategy".to_string(),
                }),
                None => Err(PhaseFailure::of(
                    FailureKind::Internal,
                    "scenario: no advisor outcome configured",
                )),
            }
        }
        fn resume_implementer(
            &mut self,
            _answer: &str,
        ) -> Result<ImplementerOutcome, PhaseFailure> {
            match next_step(&self.sc.resume, &mut self.resume_i) {
                None => Err(PhaseFailure::of(
                    FailureKind::Internal,
                    "scenario: no resume outcome configured",
                )),
                Some(step) => impl_outcome(step),
            }
        }
        fn mark_implementer_lease_escalated(&mut self) {
            self.mark_escalated_calls += 1;
        }
        // trace:STORY-975 | ai:codex
        fn transient_retry_budget(&self) -> usize {
            0
        }
        fn shelve_on_failure(
            &mut self,
            _spec: &str,
            phase: Phase,
            failure: &PhaseFailure,
            recovery_hint: &str,
        ) -> anyhow::Result<Option<aida_core::FailureReason>> {
            if !self.sc.shelve_succeeds {
                return Ok(None);
            }
            Ok(Some(aida_core::FailureReason {
                phase: phase.slug().to_string(),
                phase_index: phase.index() as u8,
                kind: failure.kind.cause_slug().to_string(),
                detail: failure.reason.clone(),
                recovery_hint: Some(recovery_hint.to_string()),
                shelved_by: None,
                shelved_at: chrono::Utc::now(),
            }))
        }
    }

    /// One member of a batch scenario: its spec-id, the `Scenario` its drive
    /// replays, and (optionally) the spec it is `BlockedBy`.
    // trace:STORY-749 | ai:claude
    struct ScenarioBatchMember {
        spec: String,
        scenario: Scenario,
        blocked_by: Option<String>,
    }

    impl ScenarioBatchMember {
        fn new(spec: &str, scenario: Scenario) -> Self {
            Self {
                spec: spec.to_string(),
                scenario,
                blocked_by: None,
            }
        }
        fn blocked_by(mut self, blocker: &str) -> Self {
            self.blocked_by = Some(blocker.to_string());
            self
        }
    }

    /// Composes per-spec `Scenario`s into a batch drain. `run_spec` runs the
    /// member's scenario through the real `orchestrate` loop (batch mode); on a
    /// shelve it drops every dependent `BlockedBy` the shelved spec — mirroring
    /// `resolve_batch_members`' pickability filter (a `BlockedBy → <shelved>`
    /// member becomes un-pickable). Drives the real `drain_batch`, so the
    /// EPIC-28 resilient-drain contract (park, continue, skip dependents, exit
    /// 2) is asserted end-to-end.
    // trace:STORY-749 EPIC-28 | ai:claude
    struct ScenarioBatchDriver {
        heads: Vec<ScenarioBatchMember>,
        shelved: Vec<String>,
        skipped: Vec<(String, String)>,
    }

    impl ScenarioBatchDriver {
        fn new(members: Vec<ScenarioBatchMember>) -> Self {
            Self {
                heads: members,
                shelved: Vec::new(),
                skipped: Vec::new(),
            }
        }
    }

    impl BatchDriver for ScenarioBatchDriver {
        fn next_head(&mut self) -> Option<String> {
            self.heads.first().map(|m| m.spec.clone())
        }
        fn stop_requested(&mut self) -> bool {
            false
        }
        fn run_spec(&mut self, spec: &str) -> OrchestrationResult {
            let scenario = self
                .heads
                .iter()
                .find(|m| m.spec == spec)
                .map(|m| m.scenario.clone())
                .expect("run_spec called for a queued head member");
            let mut driver = ScenarioDriver::new(scenario);
            let result = orchestrate_with_lifecycle_skip(
                &mut driver,
                spec,
                AutoCompleteVariant::Full,
                true, // json — keep the drive quiet under test
                EscalateMode::Blocks,
                LifecycleSkip::none(),
                true, // batch — a phase-1 Inconclusive shelve-and-advances
            );
            // The head leaves the queue whether it shipped, punted, escalated,
            // or shelved (a real completed / NeedsAttention spec is no longer
            // pickable). Dependents of a shelved spec drop off too.
            self.heads.retain(|m| m.spec != spec);
            if result.shelved_reason.is_some() {
                self.shelved.push(spec.to_string());
                let deps: Vec<String> = self
                    .heads
                    .iter()
                    .filter(|m| m.blocked_by.as_deref() == Some(spec))
                    .map(|m| m.spec.clone())
                    .collect();
                for dep in deps {
                    self.heads.retain(|m| m.spec != dep);
                    self.skipped
                        .push((dep, format!("blocked by shelved {spec}")));
                }
            }
            result
        }
    }

    /// The seed scenario library (STORY-749 acceptance #2). Each fn returns the
    /// DATA for one orchestrator branch; the tests below replay it and assert
    /// the resulting outcome. Grow this from real incidents (PRIN-2), one fn
    /// per new regression.
    mod scenarios {
        use super::*;

        /// Happy path: every phase clean → the spec ships.
        pub(super) fn happy_path() -> Scenario {
            Scenario::builder()
        }

        /// CI goes red → the phase shelves the spec into NeedsAttention.
        pub(super) fn ci_red() -> Scenario {
            Scenario::builder()
                .finish_ci(vec![GateStep::Fail(FailureKind::CiRed, "CI is red".into())])
                .shelves()
        }

        /// CI red on the first drive, green on the retry (a second drive on the
        /// same driver pops the next `finish_ci` step).
        pub(super) fn ci_red_then_green() -> Scenario {
            Scenario::builder()
                .finish_ci(vec![
                    GateStep::Fail(FailureKind::CiRed, "CI is red".into()),
                    GateStep::Ok,
                ])
                .shelves()
        }

        /// A design-fork punt the advisor resolves; the implementer resumes,
        /// opens a PR, and the full pipeline runs → ship.
        pub(super) fn punt_advisor_resolves() -> Scenario {
            Scenario::builder()
                .implementer(vec![ImplStep::Punt("auth flow fork".into())])
                .advisor(vec![AdvisorStep::Resolve(
                    "use the recorded convention".into(),
                )])
                .resume(vec![ImplStep::Pr])
        }

        /// A design-fork punt the advisor escalates to a human; under the
        /// conservative `--escalate-blocks` the run parks after phase 1.
        pub(super) fn punt_advisor_escalates() -> Scenario {
            Scenario::builder()
                .implementer(vec![ImplStep::Punt("project-strategy fork".into())])
                .advisor(vec![AdvisorStep::Escalate("no recorded principle".into())])
        }

        /// Reviewer RequestChanges on the first drive, Approved on the retry.
        pub(super) fn reviewer_request_changes_then_approve() -> Scenario {
            Scenario::builder()
                .reviewer(vec![
                    ReviewStep::Verdict(Verdict::RequestChanges),
                    ReviewStep::Verdict(Verdict::Approved),
                ])
                .shelves()
        }

        /// The build phase fails → the spec shelves into NeedsAttention.
        pub(super) fn build_fail() -> Scenario {
            Scenario::builder()
                .build(vec![GateStep::Fail(
                    FailureKind::Failed,
                    "cargo build failed".into(),
                )])
                .shelves()
        }
    }

    /// Worked example (STORY-749 acceptance #5): a fixture (DATA) replayed
    /// through the real `orchestrate` loop; the assertion is on the RESULTING
    /// outcome — a clean ship that ran all six phases — not the phase returns.
    #[test]
    fn scenario_happy_path_ships() {
        let mut driver = ScenarioDriver::new(scenarios::happy_path());
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.process_exit_code(), DRIVE_EXIT_CLEAN, "clean ship");
        assert!(result.failed_phase.is_none());
        assert!(result.shelved_reason.is_none());
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ],
        );
    }

    /// CI red → the spec shelves into NeedsAttention; the drive exits `2`.
    #[test]
    fn scenario_ci_red_shelves_needs_attention() {
        let mut driver = ScenarioDriver::new(scenarios::ci_red());
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert!(
            result.shelved_reason.is_some(),
            "a red-CI drive parks the spec",
        );
        assert_eq!(result.failed_phase, Some(Phase::Ci));
        assert_eq!(result.process_exit_code(), DRIVE_EXIT_SHELVED);
        // The pipeline stopped at CI — reviewer/merge/pull/build never ran.
        assert_eq!(driver.calls, vec![Phase::Implementer, Phase::Ci]);
    }

    /// Punt → advisor resolves → implementer resumes → ship. A clean success:
    /// neither a punt nor an escalation, and all six phases ran.
    #[test]
    fn scenario_punt_advisor_resolves_then_ships() {
        let mut driver = ScenarioDriver::new(scenarios::punt_advisor_resolves());
        let result = orchestrate(
            &mut driver,
            "STORY-306",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.process_exit_code(), DRIVE_EXIT_CLEAN);
        assert!(
            result.punt_reason.is_none(),
            "a resolved punt is not a punt"
        );
        assert!(result.escalation.is_none(), "not an escalation");
        assert_eq!(driver.advisor_calls, 1);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ],
        );
    }

    /// Punt → advisor escalates → human (park). Under `--escalate-blocks` the
    /// run stops clean after phase 1, records the escalation, and stamps the
    /// implementer lease exactly once.
    #[test]
    fn scenario_punt_advisor_escalates_parks() {
        let mut driver = ScenarioDriver::new(scenarios::punt_advisor_escalates());
        let result = orchestrate(
            &mut driver,
            "STORY-306",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(
            result.process_exit_code(),
            DRIVE_EXIT_CLEAN,
            "escalation is clean",
        );
        assert!(result.failed_phase.is_none());
        let escalation = result.escalation.expect("escalation recorded");
        assert_eq!(escalation.kind, EscalationKind::DesignFork);
        assert_eq!(driver.calls, vec![Phase::Implementer]);
        assert_eq!(driver.advisor_calls, 1);
        assert_eq!(driver.mark_escalated_calls, 1);
    }

    /// CI red then green on retry: two drives on the SAME driver — the first
    /// shelves, the second (popping the next `finish_ci` step) ships.
    #[test]
    fn scenario_ci_red_then_green_on_retry_ships() {
        let mut driver = ScenarioDriver::new(scenarios::ci_red_then_green());
        let first = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert!(
            first.shelved_reason.is_some(),
            "first drive parks on red CI"
        );
        assert_eq!(first.process_exit_code(), DRIVE_EXIT_SHELVED);

        let second = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(
            second.process_exit_code(),
            DRIVE_EXIT_CLEAN,
            "the retry passes CI and ships",
        );
        assert!(second.shelved_reason.is_none());
    }

    /// Reviewer RequestChanges → fix → approve: the first drive fails/shelves at
    /// the reviewer, the retry (popping the next verdict) approves and ships.
    #[test]
    fn scenario_reviewer_request_changes_then_approve_ships() {
        let mut driver = ScenarioDriver::new(scenarios::reviewer_request_changes_then_approve());
        let first = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(first.failed_phase, Some(Phase::Reviewer));
        assert!(
            first.shelved_reason.is_some(),
            "RequestChanges parks the spec",
        );
        assert_eq!(first.process_exit_code(), DRIVE_EXIT_SHELVED);

        let second = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(
            second.process_exit_code(),
            DRIVE_EXIT_CLEAN,
            "the fix is approved",
        );
        assert!(second.shelved_reason.is_none());
    }

    /// Build fail → shelve. The pipeline runs through merge + pull, then the
    /// build phase fails and parks the spec (exit `2`).
    #[test]
    fn scenario_build_fail_shelves() {
        let mut driver = ScenarioDriver::new(scenarios::build_fail());
        let result = orchestrate(
            &mut driver,
            "TASK-247",
            AutoCompleteVariant::Full,
            false,
            EscalateMode::Blocks,
        );
        assert_eq!(result.failed_phase, Some(Phase::Build));
        assert!(result.shelved_reason.is_some());
        assert_eq!(result.process_exit_code(), DRIVE_EXIT_SHELVED);
        assert_eq!(
            driver.calls,
            vec![
                Phase::Implementer,
                Phase::Ci,
                Phase::Reviewer,
                Phase::Merge,
                Phase::Pull,
                Phase::Build,
            ],
        );
    }

    /// EPIC-28 resilient-drain contract (STORY-749 acceptance #3): a member's
    /// shelvable failure parks it, the drain CONTINUES, the dependent
    /// `BlockedBy` it is SKIPPED (never run), and the batch exits `2`.
    #[test]
    fn scenario_dependent_of_shelved_is_skipped_and_batch_exits_2() {
        let members = vec![
            ScenarioBatchMember::new("TASK-A", scenarios::ci_red()),
            ScenarioBatchMember::new("TASK-B", scenarios::happy_path()).blocked_by("TASK-A"),
        ];
        let mut driver = ScenarioBatchDriver::new(members);
        let result = drain_batch(&mut driver, None, None);

        // TASK-A parked; TASK-B skipped because it was BlockedBy a shelved spec.
        assert_eq!(driver.shelved, vec!["TASK-A".to_string()]);
        assert_eq!(driver.skipped.len(), 1, "the dependent was skipped");
        assert_eq!(driver.skipped[0].0, "TASK-B");
        // The drain drained the whole batch but >=1 member shelved/skipped → the
        // preserved EPIC-28 `2` sentinel, NOT a hard stop.
        assert_eq!(result.outcome, BatchDrainOutcome::DrainedWithShelved);
        assert_eq!(result.exit_code, DRIVE_EXIT_SHELVED);
        assert_eq!(result.shelved, vec!["TASK-A".to_string()]);
        // TASK-B never shipped (it was skipped, not driven).
        assert!(result.shipped.is_empty());
    }

    /// A healthy batch (no shelves, no dependents) drains clean and exits `0` —
    /// the resilient-drain machinery does not penalize an all-green batch.
    #[test]
    fn scenario_all_green_batch_drains_clean() {
        let members = vec![
            ScenarioBatchMember::new("TASK-A", scenarios::happy_path()),
            ScenarioBatchMember::new("TASK-B", scenarios::happy_path()),
        ];
        let mut driver = ScenarioBatchDriver::new(members);
        let result = drain_batch(&mut driver, None, None);
        assert_eq!(result.outcome, BatchDrainOutcome::Drained);
        assert_eq!(result.exit_code, DRIVE_EXIT_CLEAN);
        assert!(driver.shelved.is_empty());
        assert!(driver.skipped.is_empty());
        assert_eq!(
            result.shipped,
            vec!["TASK-A".to_string(), "TASK-B".to_string()],
        );
    }
