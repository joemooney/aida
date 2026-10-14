//! the impl auto_complete::PhaseDriver for RealPhaseDriver trait impl (STORY-1488 slice 1; Q9 split)
// trace:STORY-1488 | ai:claude

use crate::*;

impl auto_complete::PhaseDriver for RealPhaseDriver {
    fn capture_phase_done_pr(&mut self) {
        self.phase_done_pr = self.pr_number;
        if self.phase_done_head.is_none() {
            self.phase_done_head = self
                .implementer_worktree
                .as_deref()
                .and_then(|worktree| aida_core::git_ops::head_sha(worktree).ok())
                .or_else(|| {
                    self.pr_number
                        .and_then(|pr| pr_head_sha_best_effort(self, pr))
                });
        }
        if let (Some(pr), Some(head)) = (self.phase_done_pr, self.phase_done_head.as_deref()) {
            drain_state::set_change_binding(&self.project_root, &self.spec, pr, head);
        }
    }

    fn phase_done_pr_number(&self) -> Option<u32> {
        self.phase_done_pr
    }

    fn requires_phase_done_pr(&self) -> bool {
        true
    }
    // The initial driver cwd is diagnostic until the phase lease is minted;
    // retries/resumes carry the exact selected worktree and branch here.
    // trace:BUG-1244 | ai:codex
    // BUG-1628: with no retry pin and no live lease, fall back to the SAME
    // resolver an ordinary pickup uses (`resolve_pickup_workspace`), not the
    // epic-worktree defaults, which synthesized `~/ai/aida-<slug>` on a
    // `<id>-work` branch outside the project naming convention.
    // trace:BUG-1628 | ai:claude
    fn implementer_workspace(&self) -> Result<Option<(String, String)>, String> {
        match (
            &self.retry_implementer_worktree,
            &self.retry_implementer_branch,
        ) {
            (Some(worktree), Some(branch)) => {
                Ok(Some((worktree.display().to_string(), branch.clone())))
            }
            _ => {
                if let Some(lease) = list_leases(&self.project_root)
                    .into_iter()
                    .filter(|lease| lease.scope.eq_ignore_ascii_case(&self.spec))
                    .max_by_key(|lease| lease.started_at)
                {
                    return Ok(Some((
                        lease.worktree_path.display().to_string(),
                        lease.branch.clone(),
                    )));
                }
                resolve_pickup_workspace(&self.project_root, &self.spec, "auto")
                    .map(|(path, branch)| Some((path.display().to_string(), branch)))
                    .map_err(|err| format!("{err:#}"))
            }
        }
    }

    // BUG-1629: read-only and cheap — the same project-rooted lookup
    // `queue done` uses, so the two ownership checks agree.
    // trace:BUG-1628 trace:BUG-1629 | ai:claude
    fn known_spec_prefixes(&self) -> Vec<String> {
        crate::workflow_hints::project_spec_prefixes(&self.project_root)
    }

    // trace:BUG-1629 | ai:claude
    fn pin_implementer_workspace(&mut self, worktree: &str, branch: &str) {
        let worktree = std::path::PathBuf::from(worktree);
        let retry_pinned = self.retry_implementer_worktree.as_ref() == Some(&worktree)
            && self.retry_implementer_branch.as_deref() == Some(branch);
        let lease_owned = list_leases(&self.project_root)
            .iter()
            .any(|lease| lease.scope.eq_ignore_ascii_case(&self.spec) && lease.branch == branch);
        self.phase1_workspace = Some(Phase1Workspace {
            worktree,
            branch: branch.to_string(),
            fresh: !retry_pinned && !lease_owned,
        });
    }
    /// BUG-770: the real driver already knows the project it is driving, so it
    /// hands the orchestrator that root rather than letting the escalation
    /// epilogue re-derive one from the process cwd. This is the only
    /// implementor that returns `Some` — the test drivers keep the fail-safe
    /// `None` default, which is what stops `cargo test` from appending
    /// synthetic escalation events to the developer's real supervision stream.
    // trace:BUG-770 | ai:claude
    fn events_root(&self) -> Option<std::path::PathBuf> {
        Some(self.project_root.clone())
    }

    // trace:BUG-1213 trace:BUG-1445 | ai:codex
    fn begin_rework_guard(&mut self) {
        self.rework_guard = None;
        let Ok(crate::forge::ChangeLookup::Found(change)) =
            self.project_forge().change_for_spec(&self.spec)
        else {
            return;
        };
        let body = fetch_pr_ship_metadata_via_gh(&self.project_root, change.id)
            .map(|meta| meta.body)
            .unwrap_or_default();
        if !queue_cmd::rework_pr_head_matches_spec(
            &self.spec,
            &change.branch,
            change.title.as_deref().unwrap_or_default(),
            &body,
        ) {
            return;
        }
        let pr = change.id as u32;
        let Some(verdict) = blocking_rework_verdict(&self.project_root, &self.spec, pr) else {
            return;
        };
        let reason = review_verdict::rework_findings_comment(
            &self.spec,
            &format!("PR #{}", change.id),
            &verdict,
        )
        .unwrap_or_else(|| "blocking review findings remain open".to_string());
        let round = load_store_for_lookup(&self.project_root)
            .and_then(|store| {
                store
                    .get_requirement_by_spec_id(&self.spec)
                    .map(|req| queue_cmd::rework_round_from_comments(&req.comments))
            })
            .unwrap_or(2);
        // TASK-1449 (BUG-1522 AC5/AC6): arm on the blocking verdict's own
        // `reviewed_sha` and the DISPATCHED branch — not the PR head sha at
        // arm time, which is unreadable exactly when the guard matters most
        // (a Held/Inconclusive round captures no PR at all) and can't tell
        // "unchanged" from "moved, but not the dispatched branch". Arming no
        // longer depends on reading the PR head, so a forge hiccup at arm
        // time can't silently disarm the guard either.
        //
        // TASK-1449 (rework, common-path regression): ALSO capture the
        // dispatched branch's head at arm time — before the implementer
        // this round has touched anything. ~86% of verdicts record no
        // `reviewed_sha`; when that's the case, this arm-time head is the
        // fallback baseline `rework_no_op_failure` compares the post-round
        // head against, so a sha-less verdict does not turn into an
        // automatic refusal on every subsequent round.
        let arm_time_head = dispatched_branch_head_sha(&self.project_root, &change.branch);
        self.rework_guard = Some((
            pr,
            change.branch,
            verdict.reviewed_sha,
            arm_time_head,
            reason,
            round,
        ));
    }

    // trace:BUG-1213 trace:BUG-1445 trace:TASK-1449 | ai:claude
    fn rework_no_op_failure(&mut self) -> Option<auto_complete::PhaseFailure> {
        let (pr, branch, reviewed_sha, arm_time_head, reason, round) = self.rework_guard.clone()?;

        // TASK-1449 AC3 (the BUG-1527 shape): this round's own PR capture may
        // be bound to a DIFFERENT spec's PR rather than the PR this guard
        // armed against. The old `phase_done_pr != Some(pr) => return None`
        // exit treated any mismatch as license to advance — exactly BUG-1527's
        // false Done. Name the misattribution via the TASK-1442 guard instead
        // of silently trusting an unrelated PR's existence as progress; the
        // dispatched-branch comparison below still runs regardless (it does
        // not depend on `phase_done_pr` matching at all), so a resolvable
        // forge miss here just falls through rather than masking anything.
        if let Some(done_pr) = self.phase_done_pr {
            if done_pr != pr {
                if let Some(head_ref) = pr_head_ref_best_effort(self, done_pr) {
                    if let Err(e) =
                        ensure_pr_open_spec_attribution(&self.project_root, &head_ref, &self.spec)
                    {
                        return Some(auto_complete::PhaseFailure::of(
                            auto_complete::FailureKind::ReworkNoOp,
                            format!(
                                "ROUND {round} rework: this phase captured PR-{done_pr}, not \
                                 PR-{pr} — the PR this guard armed against — and PR-{done_pr}'s \
                                 commits are not attributed to {}: {e}. Refusing rather than \
                                 crediting an unrelated PR toward this spec's rework. \
                                 Authoritative open items:\n{reason}",
                                self.spec,
                            ),
                        ));
                    }
                }
            }
        }

        // TASK-1449 (rework, common-path regression fix): a missing
        // `reviewed_sha` alone must NOT refuse — ~86% of the verdict corpus
        // carries none, and refusing on that would shelve every rework round
        // after a sha-less refusal even when real commits were pushed. Fall
        // back to the DISPATCHED branch's head AT ARM TIME as the baseline;
        // only refuse (UNKNOWN, PRIN-5) when neither is available.
        let Some(baseline) = reviewed_sha.clone().or_else(|| arm_time_head.clone()) else {
            return Some(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::ReworkNoOp,
                format!(
                    "ROUND {round} rework against PR-{pr} on `{branch}` cannot be verified — \
                     neither the blocking review verdict's reviewed_sha nor the dispatched \
                     branch's head at arm time could be established. Refusing rather than \
                     advancing on an unknown. Authoritative open items:\n{reason}"
                ),
            ));
        };

        // TASK-1449 AC1/AC2/AC4: compare the baseline against the
        // DISPATCHED branch's CURRENT head, read directly — never the PR
        // head, so a Held/Inconclusive round with no `pr_number` this round
        // still gets checked (AC4). `dispatched_branch_head_sha` fetches
        // `origin/<branch>` first (best effort) before reading it, so a
        // stale remote-tracking ref is refreshed here too, not only at arm
        // time. An unreadable head is UNKNOWN and refuses.
        let Some(after) = dispatched_branch_head_sha(&self.project_root, &branch) else {
            return Some(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::ReworkNoOp,
                format!(
                    "ROUND {round} rework against PR-{pr}: the dispatched branch `{branch}`'s \
                     head could not be read. Refusing rather than advancing on an unknown. \
                     Authoritative open items:\n{reason}"
                ),
            ));
        };

        // AC1: an unclassifiable delta (no default branch ref, a git error)
        // is UNKNOWN and refuses rather than advancing.
        let Some(change) = classify_rework_head_change(&self.project_root, &baseline, &after)
        else {
            return Some(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::ReworkNoOp,
                format!(
                    "ROUND {round} rework against PR-{pr}: the change between the baseline \
                     `{baseline}` and `{branch}`'s current head `{after}` could not be \
                     classified (no default branch, or a git error). Refusing rather than \
                     advancing on an unknown. Authoritative open items:\n{reason}"
                ),
            ));
        };
        if change == ReworkHeadChange::ContentChanged {
            return None;
        }
        Some(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::ReworkNoOp,
            rework_no_op_message(pr, &baseline, &after, &reason, round, change),
        ))
    }

    fn run_implementer(
        &mut self,
    ) -> Result<auto_complete::ImplementerOutcome, auto_complete::PhaseFailure> {
        // BUG-716 invariant guard: every orchestrated implementer passes through
        // here, and the pr-ship self-merge gate depends on a LIVE drain lock
        // being held while it runs (that is how it tells a drive from a plain
        // session). Assert it at this single chokepoint so a future spawn path
        // that drops the lock trips in dev/CI-debug rather than silently
        // reopening the reviewer bypass. `debug_assert` keeps release drives
        // unaffected; the main worktree is resolved explicitly (that is where
        // `.aida/drain.lock` lives) and a resolution failure is tolerated.
        // trace:BUG-716 | ai:claude
        // In-crate harness tests (the BUG-826 stub-vendor launch test) drive
        // `run_implementer` directly with no real drive running; the invariant
        // is about production spawn paths, so `cfg!(test)` is exempt — the
        // probe would otherwise read the DEVELOPER's real .aida/drain.lock.
        debug_assert!(
            cfg!(test)
                || find_main_worktree_root()
                    .map(|root| {
                        drain_lock::drive_lock_invariant_holds(&drain_lock::probe_lock(&root))
                    })
                    .unwrap_or(true),
            "orchestrated implementer running without a live drain lock — the \
             pr-ship self-merge guard (BUG-716) keys on it; a drive path dropped \
             the lock"
        );
        // BUG-1769: read the spec's status BEFORE any child runs, so the
        // lost-child recovery can tell an advance this run produced from one
        // that was already there. Idempotent, so the replacement launch
        // re-entering here keeps the original entry reading.
        // trace:BUG-1769 | ai:claude
        self.capture_phase1_entry_spec_status();
        let session_uuid = uuid::Uuid::now_v7().to_string();
        // trace:BUG-1063 | ai:codex
        let implementer_started_at = std::time::SystemTime::now();
        let headless_vendor = session::resolve_headless_vendor(&self.project_root);
        // BUG-1290: phase 1 has no separate `mark_drain_phase` call — this IS
        // the announcement for the Implementer phase entry.
        self.mark_drain_phase_session(
            auto_complete::Phase::Implementer,
            &session_uuid,
            headless_vendor,
            true,
        );
        // STORY-306: remember the minted session id — if phase 1 punts and the
        // advisor tier resolves the fork, `resume_implementer` `--resume`s
        // exactly this session. trace:STORY-306 | ai:claude
        self.implementer_session = Some(session_uuid.clone());

        // BUG-1285: a FRESH drain launch (a brand-new orchestrator process,
        // not an in-run retry) can find a predecessor lease its own PREVIOUS
        // run left on this exact scope — the crash-and-restart shape (a
        // launcher swap, an OOM kill, a reboot) that killed the earlier
        // drain mid-implementer without releasing the lease it took. Reuse
        // the same identity-aware reclaim the Reviewer phase already applies
        // (BUG-906) so a lease whose holder is PROVABLY DEAD (per the
        // TASK-1284 pid+start-time identity check inside
        // `stale_lease_recovery_for_lease`) never blocks this fresh claim —
        // reclaimed here, before spawning, rather than discovered only after
        // the child subprocess refuses and exits with no lease to match.
        // BUG-908's in-run retry reclaim (`prepare_transient_retry`) already
        // clears any same-run predecessor by the time a retry reaches here,
        // so this is a no-op on that path and only does work on the first,
        // cross-run collision. trace:BUG-1285 | ai:claude
        release_dead_phase_predecessor_leases(
            &self.project_root,
            &self.spec,
            auto_complete::Phase::Implementer,
        )?;

        let headless_impl = self
            .no_human
            .map(|m| m.wants_headless_implementer())
            .unwrap_or(false);
        // BUG-1244: pin even the first attempt to the selected spec workspace.
        // Leaving these unset let queue-work inherit a sibling's live cwd/lease.
        // BUG-1629: launch the workspace phase 1 checked and pinned — never a
        // second resolution, and never an unpinned launch when resolution
        // fails (the old `.ok().flatten()` launched with no --branch/--path).
        // trace:BUG-1628 trace:BUG-1629 | ai:claude
        let (intended_path, intended_branch) = self.phase1_launch_workspace()?;
        let args = build_implementer_phase_args(
            &self.spec,
            &session_uuid,
            self.steal,
            self.force_claim || self.retry_implementer_worktree.is_some(),
            Some(intended_branch.as_str()),
            Some(intended_path.as_path()),
            headless_impl,
            self.permission_mode.as_deref(),
        );
        let mut cmd = std::process::Command::new(self.aida_exe());
        cmd.current_dir(&self.project_root).args(&args);
        // BUG-1485: the child may complete `queue done` / `pr ship` and release
        // its lease before this parent regains control. Give queue-work a
        // durable, session-keyed handoff path so phase 1 can still recover the
        // exact branch and worktree instead of inspecting unrelated leases.
        // trace:BUG-1485 | ai:codex
        prepare_orchestrated_lease_receipt(&mut cmd, &self.project_root, &session_uuid);
        // BUG-233: the corroboration token proves the bare auto-complete env
        // belongs to this live run. TASK-306 names the phase for statusline
        // context. BUG-742 carries the run variant so pickup prompts preserve
        // the stop-before-merge contract. trace:BUG-742 | ai:codex
        for (key, value) in orchestrator_phase_child_env(
            &self.run_token,
            auto_complete::Phase::Implementer,
            self.variant,
            &self.queue_user_id,
        ) {
            cmd.env(key, value);
        }
        if let Some(mode) = self.no_human {
            cmd.env(orchestrator::NO_HUMAN_MODE_ENV, mode.slug());
        }

        // STORY-276: provision the punt-signal handshake. An implementer that
        // hits a design-fork runs `aida punt`, which drops a signal file at
        // this path (it reads `AIDA_PUNT_SIGNAL_FILE`). Clear any stale file
        // from a prior run, then point the subprocess at it. Set for every
        // mode — an interactive `--auto-complete` implementer can punt too,
        // and the orchestrator must park gracefully rather than report a
        // phantom NoPr failure. Mirrors the reviewer's verdict-file handshake.
        // trace:STORY-276 | ai:claude
        let punt_signal = punt::signal_path(&self.project_root, &self.spec);
        if let Some(parent) = punt_signal.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::remove_file(&punt_signal);
        cmd.env(punt::SIGNAL_FILE_ENV, &punt_signal);

        // BUG-250: provision the PR-hold handshake the same way. An implementer
        // that deliberately holds the PR (branch pushed, PR withheld for a
        // manual gate) runs `aida pr hold`, which drops a `HoldSignal` at this
        // path. The orchestrator reads it after the session exits and reports a
        // clean `Held` outcome rather than a phantom NoPr failure. Clear any
        // stale file from a prior run first. trace:BUG-250 | ai:claude
        let hold_signal = punt::hold_signal_path(&self.project_root, &self.spec);
        if let Some(parent) = hold_signal.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::remove_file(&hold_signal);
        cmd.env(punt::HOLD_SIGNAL_FILE_ENV, &hold_signal);

        // TASK-329: spawn + poll for the graceful-exit sentinel rather than a
        // blocking `.status()`. In interactive `--zen` mode the skill
        // auto-resolves its end-of-drain prompt and goes idle, but the Claude
        // Code REPL has no EOF to synthesize from inside its own session
        // (BUG-230) — so the orchestrator reaps the child when the skill
        // touches `.aida/sessions/<session-id>.exit-requested`. A child that
        // exits on its own is still reaped immediately; the sentinel is
        // additive, not a replacement. Protocol: see the `exit_signal` module.
        // trace:TASK-329 | ai:claude
        let sentinel = exit_signal::sentinel_path(&self.sessions_dir(), &session_uuid);
        // BUG-420: arm the no-progress / ceiling watchdog for a *headless*
        // implementer only — an interactive REPL legitimately idles waiting for
        // the user, so it must never be killed for "no progress". The closure
        // owns its own coarse git-poll cadence; the spawn loop just consults it.
        // trace:BUG-420 | ai:claude
        let mut watchdog = headless_impl.then(|| {
            let wd = PhaseWatchdog::new_for_phase(
                self.project_root.clone(),
                session_uuid.clone(),
                headless_vendor,
                self.drain_tuning.no_progress,
                self.drain_tuning.ceiling,
                self.drain_tuning.idle_config(),
                auto_complete::Phase::Implementer,
            );
            // STORY-726: a headless implementer with teeing off (`--no-tee-headless`
            // / `AIDA_TEE_HEADLESS=0`) runs in TOTAL silence until it exits or the
            // watchdog kills it — the operator can't tell "thinking hard" from
            // "hung". Arm the liveness heartbeat ONLY in that silent case; a teed
            // child already streams its own progress, so don't double up.
            // trace:STORY-726 | ai:claude
            if crate::headless_tee::TeeOptions::from_env_and_flag(false).enabled {
                wd
            } else {
                wd.with_heartbeat("implementer", &self.spec)
            }
        });
        let mut wd_closure = watchdog.as_mut().map(|w| move || w.check());
        let wd_dyn: Option<&mut dyn FnMut() -> Option<String>> = wd_closure
            .as_mut()
            .map(|c| c as &mut dyn FnMut() -> Option<String>);
        // TASK-298: a headless implementer launches `claude -p`, which spawns
        // an agent test-worker pool; put it in its own process group so the reap
        // sweeps the whole group on every exit path. An interactive implementer
        // keeps the tree-walk-only path so its REPL stays interactive.
        // trace:TASK-298 | ai:claude
        // TASK-1120: opt-in pane hosting. The ONLY thing that changes here is
        // WHERE the implementer process is hosted — a titled tmux window vs the
        // background subprocess. The lease / phase / merge flow, the sentinel
        // handshake, and the exit-status classification below are all unchanged
        // (the tmux path recovers the implementer's real exit status and returns
        // the same `ExitOutcome::Natural`). Resolve the mode from the exported
        // `--panes` host + whether we're inside a tmux server. trace:TASK-1120
        let pane_mode = pane_host::spawn_mode(pane_host::requested_host(), pane_host::in_tmux());
        let tmux_outcome: Option<exit_signal::ExitOutcome> = match pane_mode {
            #[cfg(unix)]
            pane_host::SpawnMode::TmuxWindow => {
                eprintln!(
                    "  {} hosting the {} implementer in a titled tmux window under session `{}`",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    self.spec,
                    pane_host::DRAIN_SESSION,
                );
                match pane_host::host_implementer_in_tmux(&cmd, &self.spec) {
                    Ok(o) => Some(o),
                    Err(e) => {
                        // Graceful degrade: a display preference never fails a
                        // drain — fall back to the background spawn.
                        eprintln!(
                            "  {} tmux pane hosting unavailable ({e}); falling back to a \
                             background implementer",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        );
                        None
                    }
                }
            }
            #[cfg(not(unix))]
            pane_host::SpawnMode::TmuxWindow => None,
            pane_host::SpawnMode::Background => {
                // Requested-but-not-in-tmux is the graceful-degrade notice.
                if pane_host::requested_host().is_some() {
                    eprintln!(
                        "  {} --panes requested but not inside a tmux server; using a \
                         background implementer",
                        crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    );
                }
                None
            }
        };
        let outcome = match tmux_outcome {
            Some(o) => o,
            None => exit_signal::spawn_and_wait_watched(
                cmd,
                &sentinel,
                &self.exit_cfg,
                wd_dyn,
                headless_impl,
            )
            .map_err(|e| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::Spawn,
                    format!("could not launch the implementer session: {e}"),
                )
            })?,
        };
        // BUG-420: the watchdog killed a degenerate headless session — surface
        // it as a shelvable phase-1 failure so a batch drain parks the spec and
        // advances. trace:BUG-420 | ai:claude
        let watchdog_failure = if let exit_signal::ExitOutcome::WatchdogTripped(reason) = &outcome {
            // BUG-1299: attach the hint resolved alongside `reason` in the
            // same `watchdog_trip_report` match arm rather than let
            // `recovery_hint` re-derive one from `FailureKind` alone.
            // trace:BUG-1299 | ai:claude
            let mut failure = auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::Watchdog,
                format!("the implementer phase watchdog stopped the session — {reason}"),
            );
            if let Some(hint) = watchdog.as_mut().and_then(|w| w.take_trip_hint()) {
                failure = failure.with_hint_override(hint);
            }
            Some(failure)
        } else {
            None
        };
        if let exit_signal::ExitOutcome::Natural(status) = &outcome {
            if !status.success() {
                // BUG-1716: BUG-826's zero-byte check alone missed the
                // never-started sibling — a vendor that rejects its own argv
                // exits non-zero WITHOUT ever creating the session log, so the
                // launch failure fell through to the work-failure path:
                // classified as implementer work that failed, it burned the
                // whole STORY-975 transient budget (attempt 3/3) and parked a
                // healthy spec NeedsAttention behind the live empty lease.
                // The missing-log arm claims ONLY the shape where this
                // session's lease exists and its branch has no commits; a
                // child that never claimed a lease belongs to the BUG-1629/
                // BUG-1769 lost-child recovery below, and a branch with
                // commits falls through to the BUG-1140 substrate
                // verification. See `empty_launch_decision`.
                // trace:BUG-1716 | ai:claude
                let launch_log_evidence = headless_impl
                    .then(|| headless_launch_log_evidence(&self.project_root, &session_uuid))
                    .flatten();
                let session_lease = matches!(launch_log_evidence, Some(EmptyLaunchLog::Missing))
                    .then(|| orchestrated_session_lease_evidence(&self.project_root, &session_uuid))
                    .flatten();
                let is_empty_launch = empty_launch_decision(launch_log_evidence, session_lease);
                if is_empty_launch {
                    let log_shape = match launch_log_evidence {
                        Some(EmptyLaunchLog::Missing) => {
                            "no session log was ever created; the agent never started - \
                             a launcher/argv rejection, not a work failure"
                        }
                        _ => "zero-byte session log",
                    };
                    self.release_empty_launch_lease(&session_uuid);
                    if let Some(delay) =
                        phase1_empty_launch_retry_delay(self.empty_launch_retries_used)
                    {
                        self.empty_launch_retries_used += 1;
                        if !self.json {
                            eprintln!(
                                "  {} headless vendor exited {} ({log_shape}) - \
                                 retrying phase 1 in {}s (attempt {}/2)",
                                crate::glyph(crate::glyphs::Glyph::Hourglass).yellow(),
                                status
                                    .code()
                                    .map(|c| c.to_string())
                                    .unwrap_or_else(|| "with a signal".to_string()),
                                delay.as_secs(),
                                self.empty_launch_retries_used,
                            );
                        }
                        phase1_empty_launch_retry_sleep(delay);
                        return self.run_implementer();
                    }
                    return Err(auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::LaunchNoOutput,
                        format!(
                            "the headless vendor exited {} before emitting any output \
                             ({log_shape}) - retried phase 1 twice and released the empty \
                             session lease",
                            status
                                .code()
                                .map(|c| c.to_string())
                                .unwrap_or_else(|| "with a signal".to_string())
                        ),
                    ));
                }
                // BUG-266: before classifying this as a phase-1 failure,
                // inspect the headless `claude -p` JSONL log for telltale
                // signs that the Anthropic API took the session out
                // (529 / 5xx / `Overloaded` / `upstream connect error` /
                // `stream timeout`). A transient upstream outage is not the
                // implementer attempting and failing the work — it is
                // *Inconclusive*, on the same terms BUG-257 introduced for
                // the GH-API leg. The drain pauses cleanly; the operator
                // resumes with the printed `aida queue work <spec> --resume
                // <session-id>` hint, which re-attaches to this exact
                // session via Claude Code's persisted JSONL.
                // trace:BUG-266 | ai:claude
                if self
                    .no_human
                    .map(|m| m.wants_headless_implementer())
                    .unwrap_or(false)
                {
                    if let Some(reason_line) =
                        read_headless_log_for_session(&self.project_root, &session_uuid)
                            .as_deref()
                            .and_then(claude_log_indicates_api_outage)
                    {
                        // TASK-402 (friction #1 + #5): make this retry hint
                        // paste-ready. The headless implementer ran in a
                        // sibling worktree, so `claude --resume` (project-slug
                        // scoped by cwd) is invisible from the main repo — emit
                        // the `cd <worktree>` step alongside the *full* session
                        // UUID. trace:TASK-402 | ai:claude
                        let resume_base =
                            format!("aida queue work {} --resume {}", self.spec, session_uuid);
                        let resume_cmd = resume_command_with_cwd(
                            &resume_base,
                            self.implementer_worktree
                                .as_deref()
                                .map(|p| p.to_string_lossy())
                                .as_deref(),
                            std::env::current_dir()
                                .ok()
                                .map(|p| p.to_string_lossy().into_owned())
                                .as_deref(),
                        );
                        let hint =
                            format!("Anthropic API was unavailable; resume with: `{resume_cmd}`");
                        return Ok(auto_complete::ImplementerOutcome::Inconclusive {
                            reason: format!(
                                "Anthropic API outage during the headless implementer: {reason_line}"
                            ),
                            retry_hint: Some(hint),
                        });
                    }
                }
                // BUG-1140: a non-zero exit is NOT proof the work failed. The
                // codex implementer commits + pushes and advances the spec to
                // Done, then its session exits 1 — and shelving as "tool-exit"
                // here orphaned correct work behind a manual `--resume` (every
                // codex-drained spec ended Done-on-a-branch, un-landed). Verify
                // by SUBSTRATE instead of trusting the exit code: fall through to
                // the lease/PR resolution below, which — via the BUG-893 recovery
                // — opens a PR for committed-but-unopened work and proceeds into
                // CI + review (the real quality gates). A non-zero exit with NO
                // landed work still falls through, to the accurate NoPr shelve
                // (or an errored lease-discovery), not a misleading tool-exit.
                // The zero-byte-log (LaunchNoOutput) and API-outage (Inconclusive)
                // cases returned above already; this is the residual "errored
                // after doing the work" case. trace:BUG-1140 | ai:claude
                if !self.json {
                    eprintln!(
                        "  {} implementer session exited {} — verifying by substrate before \
                         classifying (a non-zero exit after committed work is not a failure)",
                        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                        status
                            .code()
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "with a signal".to_string())
                    );
                }
                // Deliberately fall through — the resolution below is the arbiter.
            }
        } else if watchdog_failure.is_none() {
            eprintln!(
                "  {} implementer skill signalled completion — session reaped",
                crate::glyph(crate::glyphs::Glyph::Info).cyan()
            );
        }

        // Locate the session `queue work` created — pinned by the id we
        // minted into `--session-id`, not a lease-set diff (BUG-114).
        // BUG-1629: a child that kept neither its lease nor its handoff
        // receipt never started an implementer session. Report that state
        // exactly and spend the single clean replacement launch.
        // trace:BUG-1629 | ai:claude
        let (lease_id, recorded_branch, worktree_path) =
            match self.discover_orchestrated_lease(&session_uuid) {
                Ok(found) => {
                    // trace:BUG-1629 | ai:claude
                    discard_orchestrated_child_refusal(&self.project_root, &session_uuid);
                    found
                }
                Err(failure)
                    if failure.kind == auto_complete::FailureKind::LaunchRefused
                        && watchdog_failure.is_none() =>
                {
                    let exit = match &outcome {
                        exit_signal::ExitOutcome::Natural(status)
                        | exit_signal::ExitOutcome::Reaped(status) => status
                            .code()
                            .map(|c| format!("with code {c}"))
                            .unwrap_or_else(|| "on a signal".to_string()),
                        _ => "abnormally".to_string(),
                    };
                    let elapsed = implementer_started_at.elapsed().unwrap_or_default();
                    return self.recover_lost_child_state(&session_uuid, &exit, elapsed);
                }
                Err(failure) => return Err(failure),
            };
        // A BUG-1485 receipt can outlive the lease it describes. Preserve its
        // branch/worktree recovery data, but do not later try to end a lease
        // the child already released.
        self.implementer_lease = lease_path(&self.project_root, &lease_id)
            .exists()
            .then(|| lease_id.clone());
        // STORY-306: remember the worktree — `resume_implementer` must run
        // `claude --resume` with this exact cwd so Claude's project-slug
        // derivation finds the persisted session.
        self.implementer_worktree = Some(worktree_path.clone());

        // STORY-276: did the implementer punt? `aida punt` dropped the signal
        // file and the spec is now parked in NeedsAttention — there is no PR
        // to chase. Hand the punt up so `orchestrate` routes it through the
        // STORY-306 advisor tier.
        //
        // STORY-306: do NOT end the session here. STORY-276 ran `aida session
        // end` on a punt (a punt was terminal then) — but now the advisor tier
        // may resolve the fork and resume *this exact session* (`claude -p
        // --resume`), which needs the lease + worktree intact. So the punted
        // session persists. An escalate-blocks punt that is never resumed
        // leaves the worktree to a later cleanup pass — a filed followup. The
        // branch is reconciled from the worktree HEAD now so a resume +
        // PR-lookup has it. trace:STORY-276, STORY-306 | ai:claude
        if let Some(signal) = punt::read_signal(&punt_signal) {
            let _ = std::fs::remove_file(&punt_signal);
            self.branch = Some(reconcile_orchestrated_branch(
                &self.project_root,
                &lease_id,
                &worktree_path,
                &recorded_branch,
            ));
            return Ok(auto_complete::ImplementerOutcome::Punted {
                reason: signal.summary(),
            });
        }

        // BUG-250: did the implementer deliberately HOLD the PR? `aida pr hold`
        // dropped the hold-signal file — the branch is pushed but the PR was
        // intentionally withheld for a manual gate. This must be checked BEFORE
        // the "no open PR → failure" path below, or a deliberate hold gets
        // mis-filed as a phase-1 failure with a wrong recovery hint (the exact
        // BUG-250 symptom). Reconcile the branch from the worktree HEAD so the
        // epilogue's `gh pr create` hint + a later resume target the pushed
        // branch. Unlike the punt signal, the hold marker is NOT removed here —
        // it persists so a resume can recognise the deliberate-hold state.
        // trace:BUG-250 | ai:claude
        if let Some(signal) = punt::read_hold_signal(&hold_signal) {
            let held_branch = reconcile_orchestrated_branch(
                &self.project_root,
                &lease_id,
                &worktree_path,
                &recorded_branch,
            );
            self.branch = Some(held_branch.clone());
            return Ok(auto_complete::ImplementerOutcome::Held {
                reason: signal.reason,
                branch: held_branch,
            });
        }

        // BUG-223: the branch recorded in the lease is a session-start
        // snapshot. If `/aida-pr` hit BUG-88's merged-branch-name guard
        // during the implementer phase it moved the commits to a fresh
        // branch, and the recorded branch is now stale. The worktree's
        // live HEAD is ground truth — reconcile from it (rewriting the
        // lease so `aida session end` agrees) before the PR lookup.
        // trace:BUG-223 | ai:claude
        let branch = reconcile_orchestrated_branch(
            &self.project_root,
            &lease_id,
            &worktree_path,
            &recorded_branch,
        );
        self.branch = Some(branch.clone());

        // BUG-1527: the worktree-HEAD reconciliation above just proved the
        // implementer ended on a DIFFERENT branch than this phase was
        // dispatched for. That alone is not new (BUG-223's merged-branch-name
        // guard legitimately renames the SAME spec's branch mid-phase) — the
        // defect is accepting the swap when the new branch's commits credit a
        // DIFFERENT spec. Classify the branch with `decide_shelve_attribution`
        // (TASK-1444) via `classify_branch_swap_attribution` (TASK-1457) to
        // tell THREE cases apart, not two: `Confirmed` (a rename that still
        // credits `self.spec`) proceeds exactly as before; `Reattributed` (a
        // trailer confidently naming a DIFFERENT spec) is the genuine swap
        // and fails this phase with the swap named; `Uncertain` (no commit on
        // the new branch carries any spec-ID trailer yet) is absent evidence,
        // not contrary evidence — PRIN-5 forbids reporting it as a confident
        // "swap" — so it fails this phase too (the attribution cannot be
        // confirmed, so the Done write below must not run either way) but
        // with wording that says exactly that instead of asserting a swap
        // that was never established. Either way this runs BEFORE any PR
        // lookup or Done write, not after, which is how BUG-1527's
        // contradictory log lines happened.
        // trace:BUG-1527 trace:TASK-1457 | ai:claude
        if branch != recorded_branch {
            match classify_branch_swap_attribution(&worktree_path, &branch, &self.spec) {
                ShelveAttribution::Confirmed(_) => {}
                ShelveAttribution::Reattributed(other) => {
                    return Err(auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::ShippedMismatch,
                        format!(
                            "the implementer's branch swapped mid-phase — dispatched on `{}`, \
                             ended on `{}`, whose commits credit {} instead of {}",
                            recorded_branch, branch, other, self.spec
                        ),
                    ));
                }
                ShelveAttribution::Uncertain(reason) => {
                    return Err(auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::ShippedMismatch,
                        format!(
                            "the implementer's branch changed mid-phase — dispatched on `{}`, \
                             ended on `{}` — attribution unknown ({reason}): this may be a \
                             same-spec rename (BUG-223) whose commits simply have not been \
                             trailered yet, not a confirmed swap onto another spec's work",
                            recorded_branch, branch
                        ),
                    ));
                }
            }
        }

        // TASK-1289: the orchestrator owns the publication boundary. Run the
        // configured commands exactly as CI defines them, in the implementer
        // worktree, before either accepting an agent-opened PR or exercising
        // the BUG-893 auto-open recovery below.
        if self.lifecycle_skip.no_preflight {
            if !self.json {
                eprintln!(
                    "  {} implementer preflight skipped per lifecycle:no-preflight (binary: none; guards not executed)",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
                );
            }
        } else {
            let results = implementer_preflight::run(&worktree_path);
            if !self.json {
                for result in &results {
                    match result {
                        implementer_preflight::GuardResult::Passed(name) => eprintln!(
                            "  {} preflight passed: {name}",
                            crate::glyph(crate::glyphs::Glyph::Check).green()
                        ),
                        implementer_preflight::GuardResult::Skipped(note) => eprintln!(
                            "  {} preflight skipped: {note}",
                            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
                        ),
                        // An inconclusive guard is NOT a skip: it was selected and could not
                        // finish, so it is reported in its own right and refuses below. The
                        // detail reaches the operator through the refusal message.
                        // trace:TASK-1289 | ai:claude
                        implementer_preflight::GuardResult::Inconclusive { name, reason } => {
                            eprintln!(
                                "  {} preflight inconclusive: {name} ({reason})",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                            )
                        }
                        implementer_preflight::GuardResult::Failed { .. } => {}
                    }
                }
            }
            match implementer_preflight::decide(&results) {
                implementer_preflight::PreflightDecision::Refuse { failed } => {
                    let detail = failed
                        .into_iter()
                        .map(|(name, output)| format!("guard `{name}` failed:\n{output}"))
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    // TASK-1289: the guards refused — but the implementer may have
                    // ALREADY opened the PR, because `/aida-pr` runs inside the
                    // implementer phase, before the orchestrator regains control.
                    // A refusal that leaves that PR open is advisory, not a gate:
                    // the work the guards rejected sits published and mergeable by
                    // anyone who never reads this log line, and the only thing
                    // standing between it and `main` is prose in a skill file.
                    // Retract it here, so "the guards refused" and "nothing is
                    // published" are the same state.
                    //
                    // Best-effort by design: a retraction that fails is reported
                    // loudly and the phase still fails. The branch is untouched
                    // either way, so no work is lost.
                    // trace:TASK-1289 | ai:claude
                    self.retract_refused_publication(&branch, &detail);
                    return Err(auto_complete::PhaseFailure::new(format!(
                        "implementer preflight refused to open the PR:\n{detail}"
                    )));
                }
                // BUG-1714: nothing objected, and nothing verified — an
                // infrastructure outcome, not a code verdict. Closing here
                // discarded reviewable work whose CI was green (#2255), and
                // told the reader the guards "refused" a change no guard ever
                // looked at. Hold the PR open and unmergeable instead; the
                // phase still fails, so nothing downstream treats the work as
                // verified, and the guards run again on the next attempt.
                // trace:BUG-1714 | ai:claude
                implementer_preflight::PreflightDecision::Hold { inconclusive } => {
                    let detail = implementer_preflight::inconclusive_detail(&inconclusive);
                    self.hold_inconclusive_publication(&branch, &detail);
                    return Err(auto_complete::PhaseFailure::new(format!(
                        "implementer preflight could not complete — no guard objected, none \
                         verified; any open PR is held unmergeable and the guards will be \
                         retried:\n{detail}"
                    )));
                }
                implementer_preflight::PreflightDecision::Open => {}
            }
        }

        // The pipeline has nothing to review or merge without a PR — verify
        // the implementer opened one before exiting. The branch-keyed lookup
        // is primary; if it comes up empty — a swap the worktree-HEAD
        // reconciliation above could not catch (detached HEAD, worktree
        // already gone) — fall back to a spec-id search before declaring a
        // false negative. trace:BUG-223 | ai:claude
        //
        // TASK-136: a *transient* GH-API outage (the BUG-257 `Inconclusive`
        // case) is retried with a bounded backoff before the orchestrator
        // gives up — a momentary blip clears here rather than pausing /
        // shelving the whole drain. Only the unreachable case retries; a
        // definitive Found / NoPr / hard-failure breaks out immediately.
        // trace:TASK-136 | ai:claude
        let schedule =
            auto_complete::gh_verify_backoff_schedule(self.drain_tuning.gh_verify_retries);
        let mut attempt = 0usize;
        let pr = loop {
            match self.detect_phase1_pr(&branch) {
                Phase1PrResolve::Found(pr) => break Some(pr),
                // BUG-709: the implementer already merged its own PR — the work
                // shipped and the spec auto-bumped. Return a terminal
                // AlreadyMerged outcome so the orchestrator completes cleanly
                // instead of shepherding a merged PR through CI/review/merge or
                // spinning the open-PR verify. trace:BUG-709 | ai:claude
                Phase1PrResolve::AlreadyMerged(pr) => {
                    self.set_pr_number(pr.number as u32);
                    // BUG-711: the implementer already merged, so phases 2-5
                    // (which include phase 2's session-end) will be skipped —
                    // tear the implementer session down HERE so the lease + pool
                    // worktree don't leak. Best-effort: the work already
                    // shipped, so a teardown hiccup warns rather than failing an
                    // otherwise-successful drive. trace:BUG-711 | ai:claude
                    if let Err(e) = self.end_implementer_session() {
                        if !self.json {
                            eprintln!(
                                "  {} PR-{} shipped, but ending the implementer session failed \
                                 ({}) — release it manually: `aida session end`",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                                pr.number,
                                e.reason,
                            );
                        }
                    }
                    if !self.json {
                        eprintln!(
                            "  {} PR-{} already merged (the implementer ran the full ship) — \
                             no open PR to shepherd; completing the drive",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            pr.number,
                        );
                    }
                    return Ok(auto_complete::ImplementerOutcome::AlreadyMerged {
                        pr_number: pr.number as u32,
                    });
                }
                Phase1PrResolve::NoPr => break None,
                Phase1PrResolve::Fail(f) => return Err(f),
                Phase1PrResolve::Retry(reason) => {
                    if attempt < schedule.len() {
                        let wait = schedule[attempt];
                        attempt += 1;
                        if !self.json {
                            eprintln!(
                                "  {} GH verify inconclusive (attempt {}/{}) — retrying in {}s: {}",
                                crate::glyph(crate::glyphs::Glyph::Hourglass).yellow(),
                                attempt,
                                schedule.len(),
                                wait.as_secs(),
                                reason,
                            );
                        }
                        std::thread::sleep(wait);
                        continue;
                    }
                    // Retry budget exhausted and still unreachable. The
                    // batch-vs-single shelve/pause decision is made upstream in
                    // `orchestrate_with_lifecycle_skip` from this Inconclusive.
                    if let Some((ahead, pr)) = try_open_orchestrator_pr_for_no_pr_worktree(
                        &self.project_root,
                        &worktree_path,
                        &branch,
                        self.lifecycle_forge,
                        &self.spec,
                    ) {
                        if !self.json {
                            eprintln!(
                                "  {} implementer left {} commit(s) with no PR after PR lookup retries — opened PR-{} \
                                 for it (BUG-893 recovery)",
                                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                                ahead,
                                pr
                            );
                        }
                        self.set_pr_number(pr as u32);
                        break Some(OpenPrInfo {
                            number: pr,
                            title: String::new(),
                            url: String::new(),
                            head_branch: Some(branch.clone()),
                        });
                    }
                    return Ok(auto_complete::ImplementerOutcome::Inconclusive {
                        reason,
                        retry_hint: None,
                    });
                }
            }
        };

        match pr {
            Some(pr) => {
                self.set_pr_number(pr.number as u32);
                // A reused PR may be sitting in draft state (a drafted
                // retraction, the ship gate, an operator) — un-draft it now
                // or the phase-4 merge is refused. trace:BUG-1690 | ai:claude
                self.undraft_reused_publication(&pr);
                // BUG-1527: the PR just resolved may be on a branch the
                // implementer swapped to mid-phase — prefer the PR's own
                // head branch (ground truth for what it actually contains)
                // over the pre-loop-captured `branch`, so the attribution
                // check inside `ensure_spec_done_after_pr` reads the right
                // commits. trace:BUG-1527 | ai:claude
                let credited_branch = pr.head_branch.clone().unwrap_or_else(|| branch.clone());
                ensure_spec_done_after_pr(
                    &self.project_root,
                    &worktree_path,
                    &credited_branch,
                    &self.spec,
                    pr.number as u32,
                    self.json,
                );
                Ok(auto_complete::ImplementerOutcome::PrOpened)
            }
            None => {
                // BUG-893: substrate-as-bouncer for the "implementer committed
                // work but exited without opening a PR" gap. The committed HEAD
                // lives in the implementer worktree; the main orchestrator
                // checkout often has no local copy of the branch, so a
                // project-root ahead check reads as zero and leaves the later
                // reviewer phase stuck at PR-0. Use the worktree as ground
                // truth, push it, open the PR with the head commit subject, and
                // continue into CI/review. trace:BUG-893 | ai:codex
                if let Some((ahead, pr)) = try_open_orchestrator_pr_for_no_pr_worktree(
                    &self.project_root,
                    &worktree_path,
                    &branch,
                    self.lifecycle_forge,
                    &self.spec,
                ) {
                    if !self.json {
                        eprintln!(
                            "  {} implementer left {} commit(s) with no PR — opened PR-{} \
                             for it (BUG-893 recovery)",
                            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                            ahead,
                            pr
                        );
                    }
                    self.set_pr_number(pr as u32);
                    ensure_spec_done_after_pr(
                        &self.project_root,
                        &worktree_path,
                        &branch,
                        &self.spec,
                        pr as u32,
                        self.json,
                    );
                    return Ok(auto_complete::ImplementerOutcome::PrOpened);
                }
                if let Some(reason) = self.auto_punt_text_question(&worktree_path, &session_uuid) {
                    return Ok(auto_complete::ImplementerOutcome::Punted { reason });
                }
                // A watchdog is allowed to stop the process, but not to hide
                // artifacts the process already produced. We deliberately ran
                // the normal lease/branch/PR recovery above first. If opening
                // the PR was impossible, retain the watchdog classification
                // and make the shelve event name the branch + commit count.
                // The punt signal was also consumed before this point, so a
                // design fork filed by the stopped phase is still delivered.
                // trace:BUG-1450 | ai:codex
                if let Some(failure) = watchdog_failure {
                    return Err(watchdog_failure_with_committed_work(
                        failure,
                        &worktree_path,
                        &branch,
                    ));
                }
                // No PR and no commit to open one from: a stream that
                // recorded permission denials means the writes were refused.
                // trace:BUG-1930 | ai:claude
                if headless_impl {
                    if let Some(failure) =
                        no_pr_permission_denied_failure(&self.project_root, &session_uuid)
                    {
                        return Err(failure);
                    }
                }
                let failure = auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::NoPr,
                    "the implementer session exited cleanly but opened no PR — \
                     run `/aida-pr` inside the session before exiting",
                );
                if headless_impl {
                    Err(enrich_headless_wait_failure(
                        failure,
                        &self.project_root,
                        implementer_started_at,
                    ))
                } else {
                    Err(failure)
                }
            }
        }
    }

    fn recover_missing_review_pr(&mut self) -> Option<u32> {
        // By phase 3 the implementer worktree may already be torn down, but
        // phase 2 pushed `self.branch` before teardown. Open from the pushed
        // branch and seed the PR number so the reviewer preflight proceeds.
        // trace:BUG-895 | ai:codex
        let branch = self.branch.clone()?;
        let (ahead, pr) = try_open_orchestrator_pr_for_no_pr_pushed_branch(
            &self.project_root,
            &branch,
            self.lifecycle_forge,
            &self.spec,
        )?;
        if !self.json {
            eprintln!(
                "  {} pushed branch `{}` is {} commit(s) ahead with no review PR — opened PR-{} \
                 for it (BUG-895 recovery)",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                branch,
                ahead,
                pr
            );
        }
        self.set_pr_number(pr as u32);
        self.pr_number
    }

    fn recover_phase1_failure_with_open_pr(
        &mut self,
        _failure: &auto_complete::PhaseFailure,
    ) -> Option<auto_complete::Phase> {
        // BUG-1145: if phase 1 fails after the child already opened a PR, the
        // substrate beats the local failure. Continue through the PR-only path
        // instead of retrying phase 1, which can collide with the already-Done
        // spec/worktree. This is the in-process counterpart of `--from-pr`.
        // trace:BUG-1145 | ai:codex
        let pr = match self.project_forge().change_for_spec(&self.spec) {
            Ok(crate::forge::ChangeLookup::Found(pr)) => pr,
            _ => return None,
        };
        // BUG-1791: phase-1 failure recovery adopts an existing publication
        // just like the primary phase-1 lookup. Clear a reused draft before
        // phase 2 can shepherd it toward merge (unless it is draft-only).
        // trace:BUG-1791 | ai:codex
        let adopted_pr = OpenPrInfo {
            number: pr.id,
            title: pr.title.clone().unwrap_or_default(),
            url: pr.url.clone(),
            head_branch: (!pr.branch.is_empty()).then_some(pr.branch.clone()),
        };
        self.undraft_reused_publication(&adopted_pr);
        self.set_pr_number(pr.id as u32);
        if !pr.branch.is_empty() {
            self.branch = Some(pr.branch.clone());
        } else if let Some(head) = pr_head_branch(&self.project_root, pr.id) {
            self.branch = Some(head);
        }
        self.from_pr = true;
        if !self.json {
            eprintln!(
                "  {} phase-1 failed, but {} has open PR-{} — continuing from the \
                 CI gate (phase 2) on the PR's branch instead of retrying phase 1",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                self.spec,
                pr.id,
            );
        }
        // TASK-1230: the recovery decision is now recorded (from_pr + branch +
        // pr_number set). Tear down the failed attempt's orphan worktree BEFORE
        // re-entering at Ci — best-effort, honoring the safety predicate. This
        // closes the cruft-accumulation half of BUG-1145 (the stale worktree the
        // proceed-from-PR path used to leave for a later `aida session reap`).
        // trace:TASK-1230 | ai:claude
        self.teardown_failed_attempt_worktree(&pr.branch);
        // BUG-1145 (advisor review of the initial re-enter-at-Reviewer): re-enter
        // at Ci, NOT Reviewer. finish_ci (phase 2) is the drain's ONLY CI gate
        // (CiProbe::Red -> CiRed shelve; InProgress -> CiTimeout shelve); merge()
        // uses `gh pr merge` which does NOT gate CI, and main has no branch
        // protection — so re-entering past phase 2 would merge red/in-progress
        // CI ungated. Ci runs on the seeded branch (set above), gating before
        // reviewer/merge. The failed attempt left no matched lease, so
        // end_implementer_session is best-effort on a None lease.
        // trace:BUG-1145 | ai:claude
        Some(auto_complete::Phase::Ci)
    }

    fn finish_ci(&mut self) -> Result<(), auto_complete::PhaseFailure> {
        self.mark_drain_phase(auto_complete::Phase::Ci);
        let mut branch = self.branch.clone().unwrap_or_default();

        // Phase 1's verified change identity is authoritative. Re-resolve it
        // by ID, prove both attribution and exact head, then use the forge's
        // source branch for CI. This supports local staging branches (and
        // branches without upstreams) that pushed into an existing PR/MR.
        // Only legacy/resume state without a binding falls back to branch
        // lookup and its local push behavior. trace:BUG-1818 | ai:codex
        if let Some(pr) = self.phase_done_pr {
            let expected_head = self.phase_done_head.as_deref().ok_or_else(|| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::CiUnavailable,
                    format!(
                        "phase 1 recorded PR/MR {pr} without its produced head SHA; refusing an unpinned phase-2 lookup"
                    ),
                )
            })?;
            let forge = self.lifecycle_forge();
            let mut sink = network_retry::NoopSink;
            let metadata = forge
                .change_metadata(u64::from(pr), &mut sink)
                .map_err(|e| {
                    auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::CiUnavailable,
                        format!("could not resolve phase-1 PR/MR {pr} by identifier: {e:#}"),
                    )
                })?;
            let credit = pr_credit_match_with_sink(
                &self.project_root,
                pr,
                &self.spec,
                Some(&metadata.title),
                &mut sink,
            );
            branch = verified_phase2_branch(pr, expected_head, &metadata, credit, &self.spec)?;
            self.branch = Some(branch.clone());
        } else {
            if branch.is_empty() {
                return Err(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::Internal,
                    "internal: neither a verified PR/MR identifier nor a local branch was resolved before the CI phase",
                ));
            }
            let forge = self.lifecycle_forge();
            if let Ok(changes) = forge.list_changes(crate::forge::ChangeFilter {
                base: None,
                open_only: true,
            }) {
                let matches = changes
                    .into_iter()
                    .filter(|change| change.branch == branch)
                    .collect::<Vec<_>>();
                if matches.len() > 1 {
                    let evidence = matches
                        .iter()
                        .map(|change| format!("{} (`{}`)", change.id, change.branch))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::CiUnavailable,
                        format!(
                            "multiple open PR/MR matches for fallback branch `{branch}`: {evidence}; refusing an ambiguous phase-2 lookup"
                        ),
                    ));
                }
            }
            self.ensure_implementer_branch_pushed(&branch)?;
        }

        // Probe, then block until CI is terminal. `lifecycle:trivial` remains
        // represented by a forge-level NoCi result, but a running check is
        // never allowed to overlap review: a verdict without terminal CI for
        // its exact head cannot drive a shelf or merge. trace:BUG-1460
        // STORY-516: forge-routed.
        let mut probe = ci_probe_with_forge(&self.project_root, self.lifecycle_forge, &branch);
        // A freshly pushed head briefly has no check rows. On a hosted forge,
        // wait for workflow registration before treating that as a conclusion;
        // otherwise phase 3 can start before the new head's checks exist.
        // trace:BUG-1460 | ai:codex
        if let CiProbe::PrNoChecks { pr_number } = probe {
            let change = crate::forge::ChangeRef {
                id: u64::from(pr_number),
                url: String::new(),
                branch: branch.clone(),
                base: String::new(),
                title: None,
            };
            let forge = self.lifecycle_forge();
            crate::ci_gate::wait_for_checks_to_register(
                forge.as_ref(),
                &change,
                std::time::Duration::from_secs(60),
                std::time::Duration::from_secs(10),
            )
            .map_err(|error| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::CiTimeout,
                    format!("CI checks did not register for PR-{pr_number}: {error:#}"),
                )
            })?;
            probe = ci_probe_with_forge(&self.project_root, self.lifecycle_forge, &branch);
        }
        let mut terminal_event_emitted = false;
        if matches!(probe, CiProbe::InProgress { .. }) {
            eprintln!(
                "  {} waiting for CI on `{}` to finish…",
                crate::glyph(crate::glyphs::Glyph::InFlight).yellow(),
                branch
            );
            // TASK-1165: the driver already knows the project it is driving —
            // hand it down rather than letting the CiTerminal emit re-derive
            // one from the process cwd. trace:TASK-1165 | ai:claude
            probe = if self.json {
                wait_for_ci_terminal(Some(&self.project_root), &branch)
            } else {
                watch_ci_for_context_with_forge(
                    &self.project_root,
                    self.lifecycle_forge,
                    &branch,
                    self.no_human.is_some(),
                )
                // STORY-516
            };
            terminal_event_emitted =
                !matches!(probe, CiProbe::InProgress { .. } | CiProbe::NoSignal(_));
        }

        if let (Some(expected), Some(actual)) = (self.phase_done_pr, ci_probe_change_id(&probe)) {
            if actual != expected {
                return Err(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::CiUnavailable,
                    format!(
                        "phase-2 CI lookup for source branch `{branch}` resolved PR/MR {actual}, but phase 1 verified PR/MR {expected}; refusing the ambiguous match"
                    ),
                ));
            }
        }

        match probe {
            CiProbe::Green { pr_number } => {
                self.set_pr_number(pr_number);
                self.ci_terminal_green = Some(true);
            }
            CiProbe::Red {
                pr_number,
                failed_summary,
            } => {
                self.set_pr_number(pr_number);
                // BUG-1180 / ADR-39: the coarse red may be the supervised
                // merge-hold gate itself (released at merge, not a CI failure)
                // or an informational matrix job. Re-read the per-check rows
                // and shelve CiRed only for a REAL failing check. GitHub only;
                // other forges keep the coarse verdict.
                // trace:BUG-1180 | ai:claude
                //
                // BUG-1205: an informational-only red is treated EXACTLY like
                // Green — it must fall through to the shared post-probe steps
                // (end the implementer session, auto-queue the review, …).
                // Returning early here left the implementer lease held and the
                // headless reviewer died on the "branch held by lease" prompt.
                // trace:BUG-1205 | ai:claude
                // BUG-1265: when a marker or confirmed label makes the coarse
                // red ambiguous, row-read failure is ci-unavailable, never
                // evidence of a real failing check. trace:BUG-1265 | ai:codex
                let refined_green = {
                    let hold_present =
                        merge_hold::read_hold(&self.project_root, pr_number as u64).is_some();
                    let hold_label_present = !hold_present
                        && merge_hold::label_present(&self.project_root, pr_number as u64)
                            .unwrap_or(false);
                    let refine_change = crate::forge::ChangeRef {
                        id: pr_number as u64,
                        url: String::new(),
                        branch: branch.clone(),
                        base: String::new(),
                        title: None,
                    };
                    match ci_gate::refine_red(
                        &self.project_root,
                        self.lifecycle_forge().as_ref(),
                        &refine_change,
                        hold_present,
                        hold_label_present,
                        std::time::Duration::from_secs(20 * 60),
                        ci_red_refine_poll_interval(),
                    ) {
                        Ok(r) if !r.is_real() && !r.has_pending() => {
                            if !self.json {
                                eprintln!(
                                    "  {} CI red on PR-{pr_number} is not a failure — {}",
                                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                                    r.describe(pr_number as u64),
                                );
                            }
                            true
                        }
                        Ok(r) if !r.is_real() => {
                            self.ci_run_id = latest_run_id_for_branch(&branch);
                            return Err(auto_complete::PhaseFailure::of(
                                auto_complete::FailureKind::CiTimeout,
                                format!(
                                    "CI on PR-{pr_number} did not settle in time — still pending: {}",
                                    r.pending.join(", ")
                                ),
                            ));
                        }
                        Ok(r) => {
                            let failed_names = r
                                .real
                                .iter()
                                .map(|row| row.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ");
                            self.ci_run_id = r.real.first().and_then(|row| row.run_id.clone());
                            let hint = ci_red_recovery_hint(
                                self.lifecycle_forge,
                                &self.spec,
                                &branch,
                                &r.real,
                            );
                            return Err(auto_complete::PhaseFailure::of(
                                auto_complete::FailureKind::CiRed,
                                format!(
                                    "CI is red on PR-{pr_number}: {}",
                                    // The supervised gate is the hold, not a
                                    // failed check. A ci-red shelf must name
                                    // only the genuine required failures.
                                    // trace:BUG-1265 | ai:codex
                                    failed_names
                                ),
                            )
                            .with_hint_override(hint));
                        }
                        Err(error) if hold_present || hold_label_present => {
                            self.ci_run_id = latest_run_id_for_branch(&branch);
                            return Err(auto_complete::PhaseFailure::of(
                                auto_complete::FailureKind::CiUnavailable,
                                format!(
                                    "CI check details are unavailable on PR-{pr_number} after retries while a supervised merge-hold is present: {error:#}"
                                ),
                            ));
                        }
                        Err(_) => false, // no per-check rows on this forge — coarse verdict
                    }
                };
                if !refined_green {
                    self.ci_run_id = latest_run_id_for_branch(&branch);
                    return Err(auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::CiRed,
                        format!("CI is red on PR-{pr_number}: {failed_summary}"),
                    ));
                }
                self.ci_terminal_green = Some(true);
                // Treated as Green from here: the post-probe steps below run.
            }
            CiProbe::PrNoChecks { pr_number } => {
                self.set_pr_number(pr_number);
                self.ci_terminal_green = Some(true);
                eprintln!(
                    "  {} PR-{pr_number} has no CI checks — nothing to gate on.",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan()
                );
            }
            CiProbe::InProgress { pr_number } => {
                self.set_pr_number(pr_number);
                return Err(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::CiTimeout,
                    format!("CI on PR-{pr_number} did not reach a terminal state in time"),
                ));
            }
            CiProbe::NoSignal(reason) => {
                // Phase 1 confirmed a PR, so an unreadable CI state must keep
                // the gate closed. A re-drive will retry the probe.
                // trace:BUG-1250 | ai:codex
                return Err(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::CiUnavailable,
                    format!("CI state unavailable: {reason}"),
                ));
            }
        }

        // Bind the conclusion to the forge's current PR head. If the head is
        // unavailable, phase 3 cannot prove it is reviewing what CI tested.
        // trace:BUG-1460 | ai:codex
        let pr = self.pr_number.expect("terminal CI probe resolves a PR");
        self.ci_terminal_sha = pr_head_sha_best_effort(self, pr);
        if self.ci_terminal_sha.is_none() && self.lifecycle_forge != crate::forge::ForgeKind::None {
            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::CiUnavailable,
                format!(
                    "CI reached a conclusion for PR-{pr}, but its current head SHA is unavailable"
                ),
            ));
        }
        if !terminal_event_emitted {
            emit_ci_terminal(
                Some(&self.project_root),
                self.ci_terminal_green == Some(true),
            );
        }

        // CI cleared — end the implementer session (releases the lease,
        // returns the pool worktree, and — only if an OPEN PR still exists —
        // auto-queues the `Review PR-N` item for the reviewer).
        self.end_implementer_session()
    }

    // trace:BUG-1807 | ai:codex
    fn confirm_review_handoff(&mut self) -> Result<(), auto_complete::PhaseFailure> {
        if self.lifecycle_forge == crate::forge::ForgeKind::None {
            return Ok(());
        }
        let branch = self.branch.as_deref().unwrap_or_default();
        let outcome = try_auto_queue_pr_review(
            &self.project_root,
            branch,
            self.implementer_lease.as_deref().unwrap_or("(no-sess)"),
            AutoQueueOrigin::SessionEnd,
        );
        require_review_handoff(&outcome)
    }

    fn verify_ci_for_review(&mut self) -> Result<(), auto_complete::PhaseFailure> {
        if self.lifecycle_forge == crate::forge::ForgeKind::None {
            return Ok(());
        }
        let pr = self.pr_number.ok_or_else(|| {
            auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::Internal,
                "internal: PR number not resolved before the review phase",
            )
        })?;
        let ci_sha = self.ci_terminal_sha.as_deref().ok_or_else(|| {
            auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::CiUnavailable,
                format!(
                    "review of PR-{pr} refused: this run has no terminal CI result for a known head"
                ),
            )
        })?;
        let current = pr_head_sha_best_effort(self, pr).ok_or_else(|| {
            auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::CiUnavailable,
                format!("cannot verify the current head of PR-{pr} before review"),
            )
        })?;
        if !ci_sha.eq_ignore_ascii_case(current.trim()) {
            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::CiUnavailable,
                format!(
                    "PR-{pr} moved after CI concluded ({} -> {}); re-run CI for the current head",
                    &ci_sha[..ci_sha.len().min(9)],
                    &current[..current.len().min(9)]
                ),
            ));
        }
        Ok(())
    }

    fn run_reviewer(
        &mut self,
    ) -> Result<auto_complete::ReviewerOutcome, auto_complete::PhaseFailure> {
        let pr = self.pr_number.ok_or_else(|| {
            auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::Internal,
                "internal: PR number not resolved before the review phase",
            )
        })?;
        if self.ci_terminal_sha.is_none() && self.lifecycle_forge != crate::forge::ForgeKind::None {
            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::CiUnavailable,
                format!("review of PR-{pr} refused: this run has no terminal CI result for a known head"),
            ));
        }
        // A verdict is valid only for the exact head whose CI conclusion phase
        // 2 observed. Fail closed before launching the reviewer if it moved.
        // trace:BUG-1460 | ai:codex
        if let Some(ci_sha) = self.ci_terminal_sha.as_deref() {
            let current = pr_head_sha_best_effort(self, pr).ok_or_else(|| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::CiUnavailable,
                    format!("cannot verify the current head of PR-{pr} before review"),
                )
            })?;
            if !ci_sha.eq_ignore_ascii_case(current.trim()) {
                return Err(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::CiUnavailable,
                    format!(
                        "PR-{pr} moved after CI concluded ({} -> {}); re-run CI for the current head",
                        &ci_sha[..ci_sha.len().min(9)],
                        &current[..current.len().min(9)]
                    ),
                ));
            }
        }
        // Announce phase 3 only after the current head has been proven to be
        // the head that received phase 2's terminal CI conclusion.
        self.mark_drain_phase(auto_complete::Phase::Reviewer);

        // STORY-1405: mark PR-N "under review" at the head just proven above,
        // so `aida pr ship` / another drain's merge phase / `aida awaiting` see
        // it. Held by this process for the whole phase and released when this
        // method returns with (or without) a verdict; a crash leaves a marker
        // whose dead pid expires it. This is a marker, not the BUG-511 review
        // lease — the drain's reviewer still takes no lease (see the gate
        // comment in `handle_review_spec`). trace:STORY-1405 | ai:claude
        let _review_marker = match crate::review_marker::hold(
            &main_worktree_root_from(&self.project_root),
            crate::review_marker::Marker::for_this_process(
                pr as u64,
                self.ci_terminal_sha.as_deref(),
                Some(&self.spec),
                "the drain's reviewer phase",
            ),
        ) {
            Ok(g) => Some(g),
            Err(e) => {
                eprintln!(
                    "  {} could not mark PR-{pr} as under review ({e}) — continuing",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
                None
            }
        };

        // STORY-501: ensure the "Review PR-N" hand-off story exists before we
        // hand the PR to `aida queue work PR-N`. Normally phase 2 (finish_ci →
        // `aida session end`) mints it — but a `--resume-drain` that re-enters
        // here SKIPPED phase 2, so without this the review story is missing and
        // `queue work PR-N` falls back to PR→spec→`/aida-pickup`: an implementer
        // pickup that OWNS and re-implements the spec (the shared root of
        // BUG-436, BUG-438, and the duplicate-PR). With the story present,
        // `queue work PR-N` takes the review path (review_target=Some → role
        // reviewer, `/aida-review`, PR-scoped lease — no spec ownership).
        //
        // Idempotent: a normal drain finds the story already queued and no-ops
        // (silent). A transient `gh` failure to mint is non-fatal — the reviewer
        // proceeds and the BUG-436/BUG-438 guards backstop the fallback.
        // trace:STORY-501 | ai:claude
        if let Some(branch) = self.branch.clone() {
            let outcome = try_auto_queue_pr_review(
                &self.project_root,
                &branch,
                &uuid::Uuid::now_v7().to_string(),
                AutoQueueOrigin::SessionEnd,
            );
            if let Some(review_spec) = outcome.review_spec.as_deref() {
                drain_state::set_review_spec(&self.project_root, review_spec);
            }
            match outcome.status {
                // Minted now (the resume case) or couldn't mint (gh down) —
                // surface it. AlreadyExists (normal drain) / by-design skips
                // stay silent so the normal path is unchanged.
                AutoQueueStatus::Filed | AutoQueueStatus::SkippedNeedsAttention => {
                    render_auto_queue_outcome(&outcome);
                }
                AutoQueueStatus::AlreadyExists | AutoQueueStatus::SkippedByDesign => {}
            }
        }

        // STORY-281: pre-flight stale-base check before launching the
        // headless reviewer. The 2026-05-17 self-test surfaced the
        // failure mode — PR queued for review with two PRs merged in
        // the interim, one touching the same file. Block if the PR's
        // base is behind origin AND a file the PR touches has moved on
        // base since fork. `--allow-stale-base` opts out.
        // A failure inside the check itself (gh missing, fetch failure)
        // logs a warning and proceeds — we never block a drain on
        // transient infrastructure issues, only on confirmed-stale
        // confirmed-overlap. trace:STORY-281 | ai:claude
        // trace:BUG-473 | ai:claude
        // Single-pass check (not a loop): BUG-364 deliberately changed the old
        // `continue` (re-check the base after rebasing) into trust-the-rebase-and-
        // proceed, so every arm settles the decision on the first evaluation. The
        // `auto_rebase_attempted` flag is retained because `resolve_phase3_stale_overlap`
        // / `should_auto_rebase_stale_base` take it by `&mut` to guard a second
        // rebase attempt; with no re-entry it is always `false` here.
        let mut auto_rebase_attempted = false;
        match preflight_stale_base_check_with_forge(
            &self.project_root,
            pr as u64,
            self.lifecycle_forge,
        ) {
            Ok(pr_rebase::StaleBaseOutcome::Current) => {}
            Ok(pr_rebase::StaleBaseOutcome::StaleNoOverlap { behind }) => {
                eprintln!(
                    "  {} {}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                    pr_rebase::stale_base_warn_message(pr as u64, behind).yellow()
                );
            }
            Ok(pr_rebase::StaleBaseOutcome::StaleOverlap {
                behind,
                overlap_files,
                ..
            }) => {
                let msg = pr_rebase::stale_base_block_message(pr as u64, behind, &overlap_files);
                match self.resolve_phase3_stale_overlap(pr, &mut auto_rebase_attempted, msg) {
                    Phase3StaleOverlapAction::Proceed => {}
                    Phase3StaleOverlapAction::Refuse(failure) => {
                        return Err(failure);
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "  {} pre-flight stale-base check failed ({e}); proceeding with reviewer",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                );
            }
        }

        // TASK-480: pre-flight intermediate-only check — sibling
        // substrate-as-bouncer gate to the stale-base loop above. Refuse
        // the reviewer (park NeedsAttention via PhaseFailure) when the
        // PR's diff is exclusively intermediate/generated paths, because
        // the fix is not reproducible. `--allow-intermediate-only`
        // (env `AIDA_ALLOW_INTERMEDIATE_ONLY`) opts out. Fails open on
        // infra error, same as stale-base. trace:TASK-480 | ai:claude
        {
            let allow = std::env::var("AIDA_ALLOW_INTERMEDIATE_ONLY")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            match preflight_intermediate_only_check_with_forge(
                &self.project_root,
                pr as u64,
                self.lifecycle_forge,
            ) {
                Ok(pr_rebase::IntermediateOnlyOutcome::Clean) => {}
                Ok(pr_rebase::IntermediateOnlyOutcome::SourcePlusIntermediate { intermediate }) => {
                    eprintln!(
                        "  {} {}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                        pr_rebase::intermediate_only_warn_message(pr as u64, &intermediate)
                            .yellow()
                    );
                }
                Ok(pr_rebase::IntermediateOnlyOutcome::IntermediateOnly { intermediate }) => {
                    let msg = pr_rebase::intermediate_only_block_message(pr as u64, &intermediate);
                    if allow {
                        eprintln!(
                            "  {} intermediate-only diff detected; \
                             `--allow-intermediate-only` set, proceeding.\n{}",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                            msg.yellow()
                        );
                    } else {
                        return Err(auto_complete::PhaseFailure::new(msg));
                    }
                }
                Err(e) => {
                    eprintln!(
                        "  {} pre-flight intermediate-only check failed ({e}); \
                         proceeding with reviewer",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                    );
                }
            }
        }

        let mode = read_review_mode(&self.project_root);
        // STORY-1166 / ADR-43: delegated (check-run tally) review is a GitHub
        // capability. On any other forge, say so and run the standard
        // reviewer — never a silent NoVerdict, never a shelve.
        // trace:STORY-1166 | ai:claude
        let delegated_supported = self.lifecycle_forge().supports_delegated_review();
        if mode == "delegated" && !delegated_supported {
            eprintln!(
                "  {} review mode 'delegated' is not supported on this forge ({}) — running the standard reviewer instead",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                self.lifecycle_forge.change_noun()
            );
        }
        if mode == "delegated" && delegated_supported {
            eprintln!(
                "  {} Review mode set to 'delegated' — triggering remote review on PR {}",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                pr
            );

            // The MissingTool guard also provides `gh` for the downstream
            // check-run polling (step 2), which is not part of the comment op.
            let gh = resolve_gh_binary().ok_or_else(|| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::MissingTool,
                    "`gh` is not on PATH — cannot trigger delegated review",
                )
            })?;

            // 1. Trigger review via PR comment (STORY-516: forge-routed `gh pr
            // comment`). trace:STORY-516 | ai:claude
            let review_change = crate::forge::ChangeRef {
                id: pr as u64,
                url: String::new(),
                branch: String::new(),
                base: String::new(),
                title: None,
            };
            if let Err(e) = self
                .lifecycle_forge()
                .comment(&review_change, "@claude review once")
            {
                return Err(auto_complete::PhaseFailure::new(format!(
                    "failed to comment on PR {pr} to trigger delegated review: {e:#}"
                )));
            }

            // 2. Poll check-runs on the head SHA
            let sha = current_branch_head_sha(&self.project_root).ok_or_else(|| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::Internal,
                    "could not resolve head SHA of the current branch",
                )
            })?;

            eprintln!(
                "  {} Head SHA is {}; polling check-runs for severity tally...",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                sha
            );

            let start_poll = std::time::Instant::now();
            let timeout = std::time::Duration::from_secs(600); // 10 minutes timeout
            let interval = std::time::Duration::from_secs(10); // poll every 10 seconds
            let mut severity_data: Option<serde_json::Value> = None;

            while start_poll.elapsed() < timeout {
                let cmd_output = std::process::Command::new(&gh)
                    .current_dir(&self.project_root)
                    .args([
                        "api",
                        &format!("repos/:owner/:repo/commits/{}/check-runs", sha),
                    ])
                    .output_retrying_etxtbsy();

                match cmd_output {
                    Ok(output) if output.status.success() => {
                        let body = String::from_utf8_lossy(&output.stdout);
                        if let Some(parsed) = extract_bughunter_severity(&body) {
                            severity_data = Some(parsed);
                            break;
                        }
                    }
                    Ok(output) => {
                        let err_msg = String::from_utf8_lossy(&output.stderr);
                        eprintln!(
                            "    {} Poll error: gh api returned non-zero status: {}",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                            err_msg.trim()
                        );
                    }
                    Err(e) => {
                        eprintln!(
                            "    {} Poll error: could not execute gh api: {}",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                            e
                        );
                    }
                }

                std::thread::sleep(interval);
            }

            // 3. Process outcomes / fail-safe
            if let Some(data) = severity_data {
                if let Some(obj) = data.as_object() {
                    let normal = obj.get("normal").and_then(|n| n.as_i64()).unwrap_or(0);

                    eprintln!(
                        "  {} Tally resolved: normal={}",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        normal
                    );

                    if normal > 0 {
                        eprintln!(
                            "  {} Major/critical issues detected; requesting changes",
                            crate::glyph(crate::glyphs::Glyph::Cross).red()
                        );
                        return Ok(auto_complete::ReviewerOutcome::Verdict(
                            auto_complete::Verdict::RequestChanges,
                        ));
                    } else {
                        eprintln!(
                            "  {} No major/critical issues detected; approving",
                            crate::glyph(crate::glyphs::Glyph::Check).green()
                        );
                        return Ok(auto_complete::ReviewerOutcome::Verdict(
                            auto_complete::Verdict::Approved,
                        ));
                    }
                } else {
                    eprintln!(
                        "  {} bughunter-severity data is malformed",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                    );
                }
            } else {
                eprintln!(
                    "  {} bughunter-severity polling timed out or was missing",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }

            // Fail-safe trigger: file a finding & park in NeedsAttention
            eprintln!(
                "  {} Triggering fail-safe: filing ReviewerVerdictUnavailable finding...",
                crate::glyph(crate::glyphs::Glyph::Info).cyan()
            );
            if let Err(e) = file_reviewer_verdict_unavailable_finding(&self.project_root, pr) {
                eprintln!(
                    "    {} could not file ReviewerVerdictUnavailable finding: {}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    e
                );
            }

            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::NoVerdict,
                "delegated review failed: check-run or bughunter-severity was missing, malformed, or timed out",
            ));
        }

        // Verdict handshake: the `/aida-review` skill writes the verdict
        // JSON to AIDA_REVIEW_VERDICT_FILE and stops before merge.
        let verdict_dir = self.project_root.join(".aida").join("review-verdicts");
        std::fs::create_dir_all(&verdict_dir).map_err(|e| {
            auto_complete::PhaseFailure::new(format!(
                "could not create {}: {e}",
                verdict_dir.display()
            ))
        })?;
        let verdict_path = verdict_dir.join(format!("PR-{pr}.json"));
        let _ = std::fs::remove_file(&verdict_path); // clear any stale verdict

        // BUG-280: capture the wall-clock start of the reviewer subprocess so
        // a NoVerdict failure can scan the corresponding headless log for the
        // AskUserQuestion-in-headless symptom. trace:BUG-280 | ai:claude
        let reviewer_started_at = std::time::SystemTime::now();

        let session_uuid = uuid::Uuid::now_v7().to_string();
        let headless_vendor = session::resolve_headless_vendor(&self.project_root);
        // BUG-1290: `mark_drain_phase(Reviewer)` at the top of `run_reviewer`
        // already announced this entry — this is the duplicate-emission site
        // the spec measured (292/292 spurious second events had no `seat`
        // because this path never threaded one). Attach the session only.
        self.mark_drain_phase_session(
            auto_complete::Phase::Reviewer,
            &session_uuid,
            headless_vendor,
            false,
        );
        let scope = format!("PR-{pr}");
        // BUG-906: a transient retry can relaunch this phase immediately after
        // a predecessor reviewer process died. Reap that dead PR-scoped lease
        // before spawning the next `queue work PR-N`; dirty leftovers park with
        // a typed cause instead of self-colliding again.
        release_dead_phase_predecessor_leases(
            &self.project_root,
            &scope,
            auto_complete::Phase::Reviewer,
        )?;
        // BUG-868: prompt text anchors the review to the PR head the
        // orchestrator is about to gate. BUG-1186: captured for EVERY review
        // phase (not only `--from-pr`) — it is also the pre-review head the
        // post-review integrity guard below compares against. Advisory on the
        // prompt side, so a transient forge failure does not block the review.
        // trace:BUG-1186 | ai:claude
        let pre_review_head_sha = pr_head_sha_best_effort(self, pr);
        let mut graded_context = None;
        if let Some(head_sha) = pre_review_head_sha.as_deref() {
            if let Some(graded) = prepare_graded_review(
                &self.project_root,
                &self.spec,
                pr,
                head_sha,
                self.branch.as_deref(),
            )? {
                if !graded.escalated_to_seat {
                    return Ok(auto_complete::ReviewerOutcome::Verdict(
                        // trace:BUG-1505 | ai:claude — canonical parser; anything
                        // it cannot classify fails closed as Rejected.
                        auto_complete::Verdict::parse(&graded.overall_verdict)
                            .unwrap_or(auto_complete::Verdict::Rejected),
                    ));
                }
                let prompt = graded_review::generate_graded_reviewer_prompt(
                    &self.spec,
                    Some(pr as u64),
                    &graded,
                );
                graded_context = prompt
                    .split_once("\n\n")
                    .map(|(_, suffix)| suffix.to_string());
            }
        }
        let mut cmd = std::process::Command::new(self.aida_exe());
        cmd.current_dir(&self.project_root)
            .args([
                "queue",
                "work",
                &scope,
                "--session-id",
                &session_uuid,
                "--no-pull",
            ])
            .env("AIDA_REVIEW_VERDICT_FILE", &verdict_path);
        if let Some(context) = graded_context.as_deref() {
            cmd.env("AIDA_GRADED_REVIEW_CONTEXT", context);
        }
        // BUG-233/BUG-901/BUG-1038: use the same orchestrator phase envelope as
        // the implementer child: token, variant, phase role, and captured queue
        // owner all travel together.
        for (key, value) in orchestrator_phase_child_env(
            &self.run_token,
            auto_complete::Phase::Reviewer,
            self.variant,
            &self.queue_user_id,
        ) {
            cmd.env(key, value);
        }
        // BUG-1186 / ADR-40: the review envelope is ALWAYS set, not only for a
        // `--from-pr` drive. `queue work PR-N` honors it by refusing to fall
        // back to an implementer pickup of the backing spec when no pickable
        // review story is queued — the seat swap that executed a rework reason
        // as an implementer and pushed to the PR under review.
        // trace:BUG-1186 | ai:claude
        cmd.env("AIDA_FROM_PR_REVIEW", "1")
            .env("AIDA_FROM_PR_NUMBER", pr.to_string());
        // Round 1 deliberately gets a broader prompt so the first reviewer
        // reports the entire acceptance/defect surface. Re-review prompts stay
        // unchanged and focus on the carried prior findings.
        // trace:TASK-1291 | ai:codex
        let review_round = load_store_for_lookup(&self.project_root)
            .and_then(|store| {
                store
                    .get_requirement_by_spec_id(&self.spec)
                    .map(|req| queue_cmd::review_round_from_comments(&req.comments))
            })
            .unwrap_or(1);
        cmd.env("AIDA_REVIEW_ROUND", review_round.to_string());
        if let Some(head_sha) = pre_review_head_sha.as_deref() {
            cmd.env("AIDA_FROM_PR_HEAD_SHA", head_sha);
        }
        // TASK-306: `--no-human` scope for the child statusline. The reviewer runs headless
        // under `--no-human` (so usually no statusline renders), but a plain
        // `--auto-complete` reviewer is interactive and shows `auto:3/6`; the
        // phase index itself comes from orchestrator_phase_child_env above.
        // trace:TASK-306 | ai:claude
        if let Some(mode) = self.no_human {
            cmd.env(orchestrator::NO_HUMAN_MODE_ENV, mode.slug());
        }
        if let Some(pm) = &self.permission_mode {
            cmd.args(["--permission-mode", pm]);
        }
        // STORY-263: under `--no-human`, run the reviewer headless. Both
        // NoHumanMode variants (ReviewerOnly, Both) include the reviewer, so
        // a bare `--no-human` on the subprocess is all the orchestrator needs
        // to pass — `handle_queue_work` then launches `claude -p`. The
        // verdict-file handshake below is unchanged: headless or not, the
        // reviewer writes the verdict and the orchestrator reads it.
        // trace:STORY-263 | ai:claude
        if self.no_human.is_some() {
            cmd.arg("--no-human");
        }
        // STORY-281: propagate the stale-base opt-out so the child's own
        // pre-flight (in handle_queue_work) doesn't re-block on the same
        // signal the orchestrator-level check already cleared.
        // trace:STORY-281 | ai:claude
        if self.allow_stale_base {
            cmd.arg("--allow-stale-base");
        }

        // TASK-329: same graceful-exit handling as the implementer phase —
        // the reviewer skill touches the sentinel as its absolute last action,
        // after the verdict file is written. The interactive `--zen` reviewer
        // would otherwise sit at the REPL with no EOF to synthesize (BUG-230).
        // trace:TASK-329 | ai:claude
        let sentinel = exit_signal::sentinel_path(&self.sessions_dir(), &session_uuid);
        // BUG-420: arm the no-progress / ceiling watchdog for a *headless*
        // reviewer (both `--no-human` variants run the reviewer headless). An
        // interactive `--zen` reviewer is left unwatched — it may sit at the
        // REPL legitimately. trace:BUG-420 | ai:claude
        let mut watchdog = self.no_human.is_some().then(|| {
            PhaseWatchdog::new_for_phase(
                self.project_root.clone(),
                session_uuid.clone(),
                headless_vendor,
                self.drain_tuning.no_progress,
                self.drain_tuning.ceiling,
                self.drain_tuning.idle_config(),
                auto_complete::Phase::Reviewer,
            )
        });
        let mut wd_closure = watchdog.as_mut().map(|w| move || w.check());
        let wd_dyn: Option<&mut dyn FnMut() -> Option<String>> = wd_closure
            .as_mut()
            .map(|c| c as &mut dyn FnMut() -> Option<String>);
        // TASK-298: both `--no-human` variants run the reviewer headless (it too
        // launches `claude -p` + an agent worker pool), so reap by process group
        // on every exit path. The interactive `--zen` reviewer stays on the
        // tree-walk path. trace:TASK-298 | ai:claude
        let outcome = exit_signal::spawn_and_wait_watched(
            cmd,
            &sentinel,
            &self.exit_cfg,
            wd_dyn,
            self.no_human.is_some(),
        )
        .map_err(|e| {
            auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::Spawn,
                format!("could not launch the reviewer session: {e}"),
            )
        })?;
        if let exit_signal::ExitOutcome::WatchdogTripped(reason) = &outcome {
            // BUG-1299: attach the hint resolved alongside `reason` in the
            // same `watchdog_trip_report` match arm rather than let
            // `recovery_hint` re-derive one from `FailureKind` alone.
            // trace:BUG-1299 | ai:claude
            let mut failure = auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::Watchdog,
                format!("the reviewer phase watchdog stopped the session — {reason}"),
            );
            if let Some(hint) = watchdog.as_mut().and_then(|w| w.take_trip_hint()) {
                failure = failure.with_hint_override(hint);
            }
            return Err(failure);
        }
        if let exit_signal::ExitOutcome::Natural(status) = &outcome {
            if !status.success() {
                return Err(auto_complete::PhaseFailure::new(format!(
                    "the reviewer session exited {}",
                    status
                        .code()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "with a signal".to_string())
                )));
            }
        } else {
            eprintln!(
                "  {} reviewer skill signalled completion — session reaped",
                crate::glyph(crate::glyphs::Glyph::Info).cyan()
            );
        }

        // TASK-1460: see `run_one_agent_gate` — archive a direct skill write,
        // then fall back to a fresh archived verdict at the reviewed head.
        // trace:TASK-1460 | ai:claude
        let _ = review_verdict::adopt_direct_write(&verdict_path);
        let outcome = match read_verdict_file_for_head(
            &verdict_path,
            pre_review_head_sha.as_deref(),
        )
        .or_else(|e| {
            phase3_head_archive_fallback(
                &verdict_path,
                pre_review_head_sha.as_deref(),
                reviewer_started_at,
            )
            .ok_or(e)
        }) {
            Ok(o) => o,
            Err(primary_failure) => {
                if verdict_path.is_file() {
                    return Err(primary_failure);
                }
                // BUG-806: the spec-keyed record, when fresh, IS the verdict.
                // BUG-809: failing that, sweep sibling checkouts — the env
                // anchor does not reliably survive a vendor tool sandbox.
                let fallback = spec_verdict_fallback_for_phase3(
                    &self.project_root,
                    &self.spec,
                    reviewer_started_at,
                    pre_review_head_sha.as_deref(),
                )?;
                let fallback = match fallback {
                    some @ Some(_) => some,
                    None => sibling_verdict_sweep_for_phase3(
                        &self.project_root,
                        pr,
                        &self.spec,
                        reviewer_started_at,
                        pre_review_head_sha.as_deref(),
                    )?,
                };
                if let Some(o) = fallback {
                    o
                } else if self.no_human.is_some() {
                    // BUG-280: under a headless `--no-human` drain, a NoVerdict
                    // failure is most often the AskUserQuestion-in-headless
                    // symptom (reviewer skill called a confirmation prompt
                    // forbidden by the harness, bailed before writing the
                    // verdict file). Enrich the error message so the recovery
                    // hint names the likely cause instead of the generic
                    // "no verdict file." trace:BUG-280 | ai:claude
                    return Err(enrich_no_verdict_with_headless_diagnostic(
                        primary_failure,
                        &self.project_root,
                        reviewer_started_at,
                    ));
                } else {
                    return Err(primary_failure);
                }
            }
        };

        // The reviewer writes the decision fields, but the drain owns the
        // authoritative PR head and branch context. Normalize a PR-keyed
        // artifact through the shared recorder so it has the same diagnostic
        // metadata as spec-keyed verdicts. Best-effort, matching `aida review`:
        // metadata persistence must not change the review gate's decision.
        // trace:TASK-168 | ai:codex
        if verdict_path.is_file() {
            let _ = stamp_pr_review_verdict(
                &self.project_root,
                pr,
                pre_review_head_sha.as_deref(),
                self.branch.as_deref(),
                self.ci_terminal_sha.as_deref(),
                self.ci_terminal_green,
            );
        }

        // STORY-439: tag-along read for reviewer-side calibration. Same
        // verdict file we just parsed; we re-read so the orchestrator's
        // PASS/FAIL decision stays untouched even if the calibration
        // capture changes. trace:STORY-439 | ai:claude
        capture_review_calibration_for_spec(&self.project_root, &verdict_path, &self.spec);

        // End the reviewer session (best-effort — the verdict is already in
        // hand, so a stuck reviewer worktree must not block the merge).
        if let Ok((reviewer_lease, _, _)) = self.discover_orchestrated_lease(&session_uuid) {
            let _ = std::process::Command::new(self.aida_exe())
                .current_dir(&self.project_root)
                .args(["session", "end", &reviewer_lease, "--yes", "--skip-ci"])
                .status_retrying_etxtbsy();
        }

        // BUG-1186 / ADR-40: post-review integrity guard. A reviewer that moved
        // the PR head modified the code it was asked to judge — an independence
        // breach. Shelve with a typed cause instead of merging reviewer-authored
        // commits under the drain's own approval (or retrying, which would
        // launder them). Best-effort on the forge side: if either SHA is
        // unavailable the guard cannot fire and the verdict stands.
        // trace:BUG-1186 | ai:claude
        if let (Some(pre), Some(post)) = (
            pre_review_head_sha.as_deref(),
            pr_head_sha_best_effort(self, pr),
        ) {
            if let Some(msg) = reviewer_wrote_message(pr, pre, &post) {
                return Err(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::ReviewerWrote,
                    msg,
                ));
            }
        }

        Ok(outcome)
    }

    fn run_agent_gates(&mut self) -> Result<(), auto_complete::PhaseFailure> {
        let gates = matching_agent_gates_for_spec(&self.project_root, &self.spec);
        if gates.is_empty() {
            return Ok(());
        }
        let pr = self.pr_number.ok_or_else(|| {
            auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::Internal,
                "internal: PR number not resolved before the agent-gate phase",
            )
        })?;

        for gate in gates {
            if !self.json {
                eprintln!(
                    "  {} running agent gate `{}` as role `{}` before merge",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    gate.name,
                    gate.role,
                );
            }
            let outcome = self.run_one_agent_gate(pr, &gate)?;
            match outcome {
                auto_complete::ReviewerOutcome::EscalatedToHuman { reason } => {
                    return Err(auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::NoVerdict,
                        format!(
                            "agent gate `{}` escalated instead of producing a verdict: {reason}",
                            gate.name
                        ),
                    ));
                }
                auto_complete::ReviewerOutcome::Verdict(auto_complete::Verdict::Approved) => {}
                auto_complete::ReviewerOutcome::Verdict(verdict) => {
                    let reason = format!(
                        "agent gate `{}` verdict is {} — not Approved",
                        gate.name,
                        verdict.label()
                    );
                    if gate.on_fail == AgentGateOnFail::Warn {
                        if let Err(e) = file_agent_gate_warning_finding(
                            &self.project_root,
                            &self.spec,
                            pr,
                            &gate,
                            verdict,
                        ) {
                            eprintln!(
                                "    {} could not file agent-gate warning finding: {}",
                                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                                e
                            );
                        }
                        eprintln!(
                            "  {} {} — continuing because on_fail='warn'",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                            reason
                        );
                        continue;
                    }
                    let kind = match verdict {
                        auto_complete::Verdict::RequestChanges => {
                            auto_complete::FailureKind::VerdictRequestChanges
                        }
                        auto_complete::Verdict::Rejected => {
                            auto_complete::FailureKind::VerdictReject
                        }
                        auto_complete::Verdict::Approved => auto_complete::FailureKind::Failed,
                    };
                    return Err(auto_complete::PhaseFailure::of(kind, reason));
                }
            }
        }
        Ok(())
    }

    // TASK-1249 / ADR-45: advisory harvest between the review gates and merge.
    // Propose-only: ledger comment + candidates file + a brief for the advisor;
    // never lands anything, never fails the run. `[harvest] gate = "off"` or
    // `lifecycle:no-harvest` skips it.
    // trace:TASK-1249 | ai:claude
    fn run_harvest_gate(&mut self) {
        if self.lifecycle_skip.no_harvest {
            if !self.json {
                eprintln!("  {} harvest skipped per lifecycle tag", "↷".cyan());
            }
            return;
        }
        let cfg =
            harvest::read_harvest_config(&self.project_root.join(".aida").join("config.toml"));
        if cfg.gate == harvest::HarvestGate::Off {
            return;
        }
        let Some(pr) = self.pr_number else {
            return;
        };
        let Some(store) = load_store_for_lookup(&self.project_root) else {
            return;
        };
        if !self.json {
            eprintln!(
                "  {} advisory harvest of PR-{pr} for {} (propose-only)…",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                self.spec
            );
        }
        match harvest::propose_only(&self.project_root, &store, &self.spec, pr as u64) {
            Ok(s) => {
                if !self.json {
                    match &s.file {
                        Some(f) => eprintln!(
                            "  {} harvest proposed {} of {} candidate(s) ({} filtered) — confirm later: `aida harvest {} --from {}`",
                            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                            s.proposed,
                            s.candidates,
                            s.skipped,
                            self.spec,
                            f.display()
                        ),
                        None => eprintln!(
                            "  {} harvest found nothing to propose ({} candidate(s), {} filtered) — ledgered",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            s.candidates,
                            s.skipped
                        ),
                    }
                }
            }
            Err(e) => {
                if !self.json {
                    eprintln!(
                        "  {} advisory harvest did not complete ({e:#}) — continuing to merge",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                    );
                }
            }
        }
    }

    fn merge(&mut self) -> Result<(), auto_complete::PhaseFailure> {
        self.mark_drain_phase(auto_complete::Phase::Merge);
        let pr = self.pr_number.ok_or_else(|| {
            auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::Internal,
                "internal: PR number not resolved before the merge phase",
            )
        })?;
        // AC1 follow-up (BUG-1468, then narrowed by the follow-up 2 proxy
        // review): the drain's merge phase gets the same staleness *signal*
        // as `aida pr ship`, but only ever WARNS and proceeds — it never
        // shelves on this check. The spec asks the drain only to warn; only
        // `aida pr ship` (an interactive/human-gated command) refuses. A
        // definition-tier change (`.github/workflows/*` whose `on:`
        // triggers include `pull_request`, or a script one of those
        // workflows invokes directly) prints which files changed; an
        // ordinary test-file change prints the lighter commits/test-count
        // line. Never blocks the merge. trace:BUG-1468 | ai:claude
        if let Some(branch) = self.branch.clone() {
            let base_branch = crate::pr_cmd::pr_ship_target_branch(pr as u64);
            let base_ref = format!("origin/{base_branch}");
            if let Some(warning) = pr_stale_check_warning(&self.project_root, &branch, &base_ref) {
                if !warning.definition_files.is_empty() {
                    println!(
                        "  {} PR-{pr}'s green check completed before {} changed on {base_branch}: \
                         {} — its CI ran against an OLDER definition of that check. Proceeding \
                         (the drain warns; only `aida pr ship` refuses).",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        if warning.definition_files.len() == 1 {
                            "a CI definition file"
                        } else {
                            "CI definition files"
                        },
                        warning.definition_files.join(", "),
                    );
                } else if !warning.test_files.is_empty() {
                    println!(
                        "  {} {} commits behind; {} test files changed on main since this branch's base",
                        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                        warning.behind_commits,
                        warning.test_files.len(),
                    );
                }
            }
        }

        // STORY-516/TASK-669 + BUG-1037: keep the forge CLI-on-PATH guard as a
        // MissingTool (non-shelvable -> stop-the-drain) pre-check, but key it
        // to the run's resolved lifecycle forge. Pure-git has no CLI precheck.
        if !self.lifecycle_forge.cli_name().is_empty() && !self.lifecycle_forge.cli_on_path() {
            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::MissingTool,
                format!(
                    "`{}` is not on PATH — cannot merge the {}",
                    self.lifecycle_forge.cli_name(),
                    self.lifecycle_forge.change_noun()
                ),
            ));
        }
        // BUG-286/TASK-669: route the phase-4 merge through Forge::merge_change,
        // passing a DualSink (stderr + drain-state) so transient-blip retry
        // events still correlate into `.aida/drain-state.json` for post-hoc
        // analysis. merge_change reuses the SPEC-410-pinned argv (`pr merge <pr>
        // --squash --delete-branch`, byte-identical) and the same network_retry
        // wrapper, and owns the gh subprocess — so the prior raw gh-stdout echo
        // is replaced by AIDA's own check line (consistent with `aida pr ship`). A
        // failed merge returns the unified Err → a shelvable `Failed`
        // PhaseFailure, matching the previous non-zero-exit behaviour.
        // trace:STORY-516 trace:TASK-669 trace:BUG-286 | ai:claude
        let mut stderr_sink = network_retry::StderrSink;
        let mut state_sink = drain_state::DrainStateSink {
            project_root: &self.project_root,
            spec: self.spec.clone(),
            phase: Some(format!(
                "{} ({})",
                auto_complete::Phase::Merge.index(),
                auto_complete::Phase::Merge.slug()
            )),
        };
        let mut sink = network_retry::DualSink {
            a: &mut stderr_sink,
            b: &mut state_sink,
        };
        let change_ref = crate::forge::ChangeRef {
            id: pr as u64,
            url: String::new(),
            branch: String::new(),
            base: String::new(),
            title: None,
        };
        let mut opts = crate::forge::MergeOptions {
            method: crate::forge::MergeMethod::Squash,
            squash_subject: None,
            squash_body: None, // trace:TASK-1330 | ai:claude
            delete_branch: true,
            match_head: None,
        };
        // TASK-1244 / ADR-41: serialize this merge against every other AIDA
        // merger (`aida pr ship`, the burndown wave) on the per-branch
        // merge-lease (STORY-1171). Merge-scoped: the RAII guard drops when this
        // method returns; contention past the bounded wait shelves the spec as
        // `lease-conflict` (a stuck merger is a triage item, not a transient).
        // trace:TASK-1244 | ai:claude
        let lease_root = main_worktree_root_from(&self.project_root);
        let lease_target = crate::pr_cmd::pr_ship_target_branch(pr as u64);
        let _merge_lease = crate::merge_lock::acquire(
            &lease_root,
            &lease_target,
            Some(pr as u64),
            "aida queue work (drain merge phase)",
            crate::merge_lock::DEFAULT_WAIT,
        )
        .map_err(|e| drain_merge_lease_failure(&e, &lease_target, pr))?;
        // TASK-1448: refuse (shelve, never exit) when the newest recorded
        // approval does not cover the PR's current head. Checked under the
        // merge lease, immediately before the merge. trace:TASK-1448 | ai:claude
        let head_sha = pr_head_sha_best_effort(self, pr).or_else(|| {
            let branch = self.branch.clone().unwrap_or_default();
            let status_ref = crate::forge::ChangeRef {
                id: pr as u64,
                url: String::new(),
                branch,
                base: String::new(),
                title: None,
            };
            self.lifecycle_forge()
                .change_status(&status_ref)
                .ok()
                .map(|status| status.head_sha.trim().to_string())
                .filter(|sha| !sha.is_empty())
        });
        let candidates = pr_ship::merge_gate_verdict_candidates(
            &[self.project_root.as_path(), lease_root.as_path()],
            pr as u64,
            std::slice::from_ref(&self.spec),
            head_sha.as_deref(),
        );
        drain_merge_approval_gate(&candidates, head_sha.as_deref(), pr as u64)?;
        // TASK-1458: pin the merge to the approved head the gate just
        // verified, so a push landing between this check and the merge is
        // refused by the forge rather than merged unreviewed.
        // trace:TASK-1458 | ai:claude
        opts.match_head = drain_merge_match_head(&candidates, head_sha.as_deref());
        // STORY-1405: another seat's review in progress on this head stops
        // the drain's merge the same way it stops `aida pr ship`. The drain's
        // own phase-3 marker was released when `run_reviewer` returned.
        // trace:STORY-1405 | ai:claude
        let review_gate = crate::review_marker::merge_gate(&lease_root, pr as u64, || {
            pr_head_sha_best_effort(self, pr)
        });
        if let crate::review_marker::MergeGate::UnderReview(m) = &review_gate {
            // TASK-1459: this is a live REVIEW, not a merge-LEASE conflict —
            // FailureKind::LeaseConflict previously mislabeled it, so the
            // recovery hint wrongly pointed at `aida merge-lock`.
            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::ReviewInProgress,
                crate::review_marker::refusal_message(m),
            ));
        }
        if let Some(note) = crate::review_marker::proceed_note(&review_gate) {
            println!(
                "  {} {note}",
                crate::glyph(crate::glyphs::Glyph::Info).cyan()
            );
        }
        self.lifecycle_forge()
            .merge_change(&change_ref, &opts, &mut sink)
            .map_err(|e| classify_drain_merge_failure(self.lifecycle_forge, pr, &e))?;
        println!(
            "  {} merged PR-{}",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            pr
        );
        // STORY-712: phase-4 merge success is an integration-milestone wake.
        // Best-effort, no control-flow change. trace:TASK-988 | ai:claude
        let (_, run_uuid) = drain_state::current_context(&self.project_root);
        events::emit(
            &self.project_root,
            &events::Event::new(
                Some(self.spec.clone()),
                run_uuid,
                events::EventKind::PrMerged { pr },
            ),
        );
        Ok(())
    }

    // The substrate supervised-merge gate: load the dispatched spec's
    // `execution_mode` from the store — any mode but `drain`, or no mode at
    // all (fail safe), holds the phase-4 merge for a human/advisor. The
    // keystone "do not merge, advisor-review" directive used to live only in
    // spec prose, which no merge path ever read. A spec the store cannot
    // resolve behaves as before (no hold) — the same tolerance as the
    // STORY-529 draft gate, so a missing store never wedges a drain.
    // trace:BUG-727 | ai:claude
    fn merge_supervision_hold(&mut self) -> Option<String> {
        let store = load_store_for_lookup(&self.project_root)?;
        let want = self.spec.to_ascii_uppercase();
        let req = store.requirements.iter().find(|r| {
            r.spec_id
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case(&want))
                .unwrap_or(false)
                || r.agreed_id
                    .as_deref()
                    .map(|s| s.eq_ignore_ascii_case(&want))
                    .unwrap_or(false)
        })?;
        if pr_ship::merge_requires_supervision(req.execution_mode) {
            Some(format!(
                "{} is marked {} — auto-merge refused; merge requires human/advisor review",
                self.spec,
                pr_ship::supervision_mode_label(req.execution_mode)
            ))
        } else {
            None
        }
    }

    // BUG-1168: a DRIVE/supervised execution-mode hold must publish the same
    // fence as the merge-wave hold, otherwise concurrent merge paths see no
    // `.aida/merge-holds/PR-N` marker and can merge before advisor review.
    // trace:BUG-1168 | ai:codex
    fn record_merge_supervision_hold(&mut self, reason: &str) {
        if let Some(pr) = self.pr_number {
            let pr = u64::from(pr);
            let _ = crate::merge_hold::write_typed_hold(
                &self.project_root,
                &crate::merge_hold::typed_hold(
                    pr,
                    crate::merge_hold::HoldReasonKind::Supervision,
                    reason,
                    None,
                )
                .with_spec(&self.spec), // trace:BUG-1562 | ai:claude
            );
            // trace:BUG-1236 | ai:claude
            if let Err(err) = crate::merge_hold::sync_label(&self.project_root, pr, true) {
                eprintln!(
                    "  {} merge-hold label not applied on PR-{pr}: {err} — run `aida merge-hold list --fix`",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
        }
    }

    fn pull(&mut self) -> Result<(), auto_complete::PhaseFailure> {
        self.mark_drain_phase(auto_complete::Phase::Pull);
        let mut status = std::process::Command::new(self.aida_exe())
            .current_dir(&self.project_root)
            .arg("pull")
            .status_retrying_etxtbsy()
            .map_err(|e| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::Spawn,
                    format!("could not run `aida pull`: {e}"),
                )
            })?;
        if !status.success() {
            // BUG-690: `aida pull`'s code leg is `--ff-only` by design, so it
            // refuses a divergent main and fails the whole drain — the failure
            // report's own recovery hint is "run `aida rebase` to classify the
            // divergence." Do that automatically for the SAFE cases: `aida
            // rebase --auto` executes only behind-only / diverged-safe (no
            // file overlap) and REFUSES diverged-risky, so this rescues the
            // common one-local/one-remote-commit-no-overlap drift without ever
            // forcing a conflicting rebase, then retries the pull once. A risky
            // divergence (or a store-leg failure, which `rebase` doesn't touch)
            // leaves the retry failing and falls through to the park below.
            // trace:BUG-690 | ai:claude
            let rebased = std::process::Command::new(self.aida_exe())
                .current_dir(&self.project_root)
                .args(["rebase", "--auto"])
                .status_retrying_etxtbsy();
            if matches!(&rebased, Ok(s) if s.success()) {
                status = std::process::Command::new(self.aida_exe())
                    .current_dir(&self.project_root)
                    .arg("pull")
                    .status_retrying_etxtbsy()
                    .map_err(|e| {
                        auto_complete::PhaseFailure::of(
                            auto_complete::FailureKind::Spawn,
                            format!("could not re-run `aida pull` after safe rebase: {e}"),
                        )
                    })?;
            }
        }
        if !status.success() {
            // BUG-254: `aida pull` now exits non-zero when either leg
            // failed; the per-leg detail + recovery hint was printed by
            // the subprocess. Halting here keeps phase 6 from running
            // over a stale tree and masking the real cause.
            // trace:BUG-254 BUG-690 | ai:claude
            return Err(auto_complete::PhaseFailure::new(
                "`aida pull` failed — the code or store leg did not advance \
                 (see the subprocess output above for the recovery hint)",
            ));
        }
        Ok(())
    }

    fn build(&mut self) -> Result<(), auto_complete::PhaseFailure> {
        self.mark_drain_phase(auto_complete::Phase::Build);
        let status = std::process::Command::new("cargo")
            .current_dir(&self.project_root)
            .args(["build", "--release"])
            .status()
            .map_err(|e| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::Spawn,
                    format!("could not run `cargo build`: {e}"),
                )
            })?;
        if !status.success() {
            return Err(auto_complete::PhaseFailure::new(
                "`cargo build --release` failed",
            ));
        }
        // BUG-1286: the trailing pull+build sequence finished. This is useful
        // duration telemetry, but terminality remains kind-based (PrMerged /
        // SpecCompleted), never dependent on this event being last.
        let (_, run_uuid) = drain_state::current_context(&self.project_root);
        events::emit(
            &self.project_root,
            &events::Event::new(
                Some(self.spec.clone()),
                run_uuid,
                events::EventKind::RunCompleted {
                    pull_completed: true,
                    build_completed: true,
                },
            ),
        );
        Ok(())
    }

    fn hint_context(&self) -> auto_complete::HintContext {
        auto_complete::HintContext {
            spec: self.spec.clone(),
            branch: self.branch.clone(),
            pr_number: self.pr_number,
            implementer_session: self.implementer_lease.clone(),
            ci_run_id: self.ci_run_id.clone(),
            // STORY-508/TASK-651: resolve the active forge so recovery hints
            // name the right CLI when a phase fails.
            forge: crate::forge::resolve_forge_kind(&self.project_root),
        }
    }

    /// BUG-657: report the target spec's status label when it is already
    /// terminal (`Completed` / `Rejected`), so the orchestrator finishes as a
    /// clean NO-OP instead of spawning an implementer that would exit 1 and
    /// auto-draft a phantom failure BUG. Reads the canonical YAML status (the
    /// same source `reconcile_failure`'s out-of-band check uses). A store it
    /// cannot read returns `None` — the drive proceeds exactly as before, so a
    /// transient read hiccup never wrongly skips real work.
    // trace:BUG-657 | ai:claude
    fn terminal_status(&mut self) -> Option<&'static str> {
        match spec_status(&self.project_root, &self.spec) {
            Some(RequirementStatus::Completed) => Some("Completed"),
            Some(RequirementStatus::Rejected) => Some("Rejected"),
            _ => None,
        }
    }

    /// BUG-241: before the orchestrator declares a phase failed, check whether
    /// the spec shipped anyway. Two real cases this redeems:
    ///   - phase 1 produced no PR because the spec was already resolved by
    ///     supersession (instance B);
    ///   - phase 3 left no verdict file because the reviewer escalated and a
    ///     human merged the PR out-of-band (instance A).
    ///     trace:BUG-241 | ai:claude
    fn reconcile_failure(
        &mut self,
        phase: auto_complete::Phase,
        failure: &auto_complete::PhaseFailure,
    ) -> auto_complete::PhaseReconcile {
        use auto_complete::{FailureKind, Phase, PhaseReconcile};

        // Reconcile only the two phases that can succeed out-of-band. Phase 1
        // (implementer) can correctly produce no PR — the spec needed no code.
        // Phase 3 (reviewer) can end with no verdict file — a human merged
        // out-of-band. Phases 2/4/5/6 fail by a gate or command genuinely not
        // passing (red CI, a failed merge, a divergent pull, a broken build);
        // those are real regardless of whether the spec's code reached main,
        // so reconciling them would mask a true failure.
        if !matches!(phase, Phase::Implementer | Phase::Reviewer) {
            return PhaseReconcile::GenuineFailure;
        }
        // A spawn ENOENT, a missing tool, or a violated invariant is a local
        // / tooling fault — not an out-of-band success. Leave it standing
        // without spending a `gh` round-trip on it.
        if matches!(
            failure.kind,
            FailureKind::Spawn | FailureKind::MissingTool | FailureKind::Internal
        ) {
            return PhaseReconcile::GenuineFailure;
        }

        // Ground truth: a merged PR (instance A) and/or a Completed spec
        // (instance B). Either alone is proof the spec shipped.
        let merged_pr = self.detect_merged_pr();
        let completed = matches!(
            spec_status(&self.project_root, &self.spec),
            Some(RequirementStatus::Completed)
        );
        reconcile_verdict(merged_pr, completed, &self.spec)
    }

    /// BUG-245: read the PR's commit subjects to see which SPEC-ID they
    /// actually credit. Called by `orchestrate` after phase 1 confirms an
    /// open PR; on a dispatched≠credited mismatch the success epilogue
    /// credits the truth and the dispatched spec is left queued. Returns
    /// `None` when there is no PR yet, or `gh` cannot answer — "cannot
    /// determine" preserves the pre-BUG-245 behaviour (dispatched id is
    /// credited). trace:BUG-245 | ai:claude
    fn shipped_spec_id(&mut self) -> Option<String> {
        let pr = self.pr_number?;
        // BUG-286: same retry treatment as `detect_merged_pr` — a transient
        // blip during the dispatched≠credited check left the orchestrator
        // unable to detect a mismatch. The retry keeps the BUG-245 detection
        // robust against the BUG-286 blip class. trace:BUG-286 | ai:claude
        let mut stderr_sink = network_retry::StderrSink;
        let mut state_sink = drain_state::DrainStateSink {
            project_root: &self.project_root,
            spec: self.spec.clone(),
            phase: Some("1 (shipped-spec-check)".to_string()),
        };
        let mut sink = network_retry::DualSink {
            a: &mut stderr_sink,
            b: &mut state_sink,
        };
        pr_credited_spec_id_with_sink(&self.project_root, pr, &self.spec, &mut sink)
    }

    /// STORY-306 advisor tier — spawn a headless advisor to judge the design-
    /// fork phase 1 punted on. Assembles the rich payload, writes the request
    /// file, runs `/aida-advise` in the advisor role on the resolved headless
    /// vendor (Claude by default; Codex via `AIDA_HEADLESS_VENDOR`/config —
    /// TASK-894), reads the response, appends a punt-ledger record, and — on an
    /// escalate — tags the spec `needs-human` + leaves the advisor's reasoning
    /// as a comment.
    // trace:STORY-306 trace:TASK-894 | ai:claude
    fn run_advisor(
        &mut self,
    ) -> Result<auto_complete::AdvisorOutcome, auto_complete::PhaseFailure> {
        use auto_complete::{AdvisorOutcome, FailureKind, PhaseFailure};

        // Load the punted spec + its AttentionReason — `aida punt` recorded it.
        let store = load_store_for_lookup(&self.project_root).ok_or_else(|| {
            PhaseFailure::of(
                FailureKind::Internal,
                "advisor tier: could not load the requirements store",
            )
        })?;
        let target = store
            .get_requirement_by_spec_id(&self.spec)
            .ok_or_else(|| {
                PhaseFailure::of(
                    FailureKind::Internal,
                    format!("advisor tier: spec {} not found in the store", self.spec),
                )
            })?;
        let attention = target.attention_reason.clone().ok_or_else(|| {
            PhaseFailure::of(
                FailureKind::Internal,
                "advisor tier: the punted spec carries no AttentionReason",
            )
        })?;

        // Assemble the rich, ultraplan-grade payload and write the request.
        let request = assemble_punt_payload(&store, target, &self.project_root, &attention);
        let request_path = punt::punt_request_path(&self.project_root, &self.spec);
        let response_path = punt::punt_response_path(&self.project_root, &self.spec);
        let _ = std::fs::remove_file(&response_path); // clear any stale response
        punt::write_punt_request(&request_path, &request).map_err(|e| {
            PhaseFailure::of(
                FailureKind::Internal,
                format!("advisor tier: could not write the punt request: {e}"),
            )
        })?;

        // STORY-347: resolve effective calibration mode. Per-drain `--calibrate`
        // / `--no-calibrate` (via `AIDA_CALIBRATE`) overrides `[advisor]
        // calibration_mode` in `.aida/config.toml`. When ON, the cold-boot
        // verdict drives the drain *and* the orchestrator fires a fork-from-
        // live shadow verdict (when a live advisor is registered) to record
        // both into `.aida/punts/<punt-id>/calibration.yaml`. When OFF, the
        // drain is byte-identical to STORY-360's path — that path is left
        // unchanged below. trace:STORY-347 | ai:claude
        let advisor_cfg = advisor::AdvisorConfig::load(&self.project_root);
        let calibration_mode = effective_calibration_mode(&advisor_cfg);
        let punt_timestamp = chrono::Utc::now();
        let punt_id = calibration::build_punt_id(&self.spec, punt_timestamp);

        // Decide the primary path (the one whose verdict drives the drain).
        //   - Calibration OFF (today's STORY-360 behaviour): use `plan_fork` —
        //     fork if a live advisor exists and the config permits; cold-boot
        //     otherwise.
        //   - Calibration ON: force cold-boot for the primary path — the spec
        //     explicitly says "cold-boot drives the drain, the fork is shadow."
        //     trace:STORY-347 | ai:claude
        let (primary_response, primary_log) = if calibration_mode.is_on() {
            self.spawn_advisor_session(
                AdvisorPass::ColdBoot,
                &advisor_cfg,
                &response_path,
                &request_path,
            )?
        } else {
            let fork_plan = advisor::plan_fork(&self.project_root, &advisor_cfg);
            let pass = match fork_plan {
                Some(plan) => AdvisorPass::Fork(plan),
                None => AdvisorPass::ColdBoot,
            };
            self.spawn_advisor_session(pass, &advisor_cfg, &response_path, &request_path)?
        };

        // STORY-347: shadow fork in calibration mode. We've already spent the
        // cold-boot's cost; only run the fork when a live advisor exists so
        // the operator's `aida advisor register` is the explicit opt-in for
        // the second-fork cost. Failures here are non-fatal — the drain still
        // ships on the cold-boot verdict.
        let (shadow_response, shadow_log, fork_skip_reason) = if calibration_mode.is_on() {
            let live =
                advisor::discover_live_advisor_session(&advisor_cfg, Some(&self.project_root));
            match live {
                None => {
                    if !self.json {
                        eprintln!(
                            "  {} calibration: no live advisor registered, skipping fork.",
                            "◆".dimmed()
                        );
                    }
                    (None, None, Some("no-live-advisor".to_string()))
                }
                Some(_live) => {
                    // Re-plan now that we have a target so the size cap +
                    // mtime fallback rules in `plan_fork` apply.
                    match advisor::plan_fork(&self.project_root, &advisor_cfg) {
                        None => (None, None, Some("plan-skipped".to_string())),
                        Some(plan) => {
                            // Use a separate response file path so the shadow
                            // does not clobber the cold-boot's response we
                            // already consumed.
                            let shadow_response_path = self
                                .project_root
                                .join(".aida")
                                .join("punts")
                                .join(format!("{}.calibration-fork.response.json", self.spec));
                            let _ = std::fs::remove_file(&shadow_response_path);
                            match self.spawn_advisor_session(
                                AdvisorPass::Fork(plan),
                                &advisor_cfg,
                                &shadow_response_path,
                                &request_path,
                            ) {
                                Ok((resp, log)) => (Some(resp), Some(log), None),
                                Err(e) => {
                                    if !self.json {
                                        eprintln!(
                                            "  {} calibration: shadow fork failed ({}); recording cold-boot only.",
                                            "◆".dimmed(),
                                            e.reason
                                        );
                                    }
                                    (None, None, Some("fork-failed".to_string()))
                                }
                            }
                        }
                    }
                }
            }
        } else {
            (None, None, None)
        };

        // STORY-347: write the calibration ledger. Best-effort — a write
        // failure logs and continues; the drain still ships on the cold-boot.
        if calibration_mode.is_on() {
            let cold_log_rel = primary_log
                .strip_prefix(&self.project_root)
                .ok()
                .map(|p| p.to_string_lossy().to_string());
            let shadow_log_rel = shadow_log.as_ref().and_then(|p| {
                p.strip_prefix(&self.project_root)
                    .ok()
                    .map(|s| s.to_string_lossy().to_string())
            });
            let record = calibration::CalibrationRecord {
                punt_id: punt_id.clone(),
                spec: self.spec.clone(),
                timestamp: punt_timestamp,
                cold_boot: calibration::CalibrationVerdict::from_response(
                    &primary_response,
                    cold_log_rel,
                ),
                fork: shadow_response
                    .as_ref()
                    .map(|r| calibration::CalibrationVerdict::from_response(r, shadow_log_rel)),
                fork_skip_reason,
                drove_drain: "cold-boot".to_string(),
                annotation: None,
            };
            if let Err(e) = calibration::write_calibration(&self.project_root, &record) {
                if !self.json {
                    eprintln!(
                        "  {} calibration: failed to write ledger ({e}); continuing.",
                        "◆".yellow()
                    );
                }
            } else if !self.json {
                let summary = match record.agreement() {
                    Some(true) => "agreement",
                    Some(false) => "disagreement",
                    None => "cold-boot only",
                };
                eprintln!(
                    "  {} calibration: recorded {} at {}",
                    "◆".cyan(),
                    summary,
                    calibration::calibration_path(&self.project_root, &record.punt_id).display(),
                );
            }
        }

        let response = primary_response;
        let _ = primary_log; // referenced in calibration ledger above; not needed by post-processing

        // Append the advisor decision to the punt ledger (STORY-325 coupling —
        // v1's escalation rate must be measurable from day one).
        let resolution_path = match response.resolution {
            punt::PuntResolution::Resolved => "advisor-resolved",
            punt::PuntResolution::Escalated => "escalated-to-human",
        };
        let decision = match response.resolution {
            punt::PuntResolution::Resolved => Some("resolved".to_string()),
            punt::PuntResolution::Escalated => Some("escalated".to_string()),
        };
        let calibration_pair = if calibration_mode.is_on() {
            Some(punt_id)
        } else {
            None
        };
        let record = punt::PuntRecord {
            timestamp: chrono::Utc::now(),
            spec: self.spec.clone(),
            category: attention.category,
            detail: attention.detail.clone(),
            lean: attention.lean.clone(),
            raised_by: attention.raised_by.clone(),
            resolution_path: resolution_path.to_string(),
            classification: response.classification.clone(),
            escalation_reason: response.escalation_reason.clone(),
            answer: response.answer.clone(),
            answered_by: Some("advisor".to_string()),
            decision,
            principle_link: None,
            calibration_pair,
            paused_at: Some(attention.raised_at),
            resolved_at: Some(chrono::Utc::now()),
        };
        let _ = punt::append_to_ledger(&self.project_root, &record);

        match response.resolution {
            punt::PuntResolution::Resolved => {
                let answer = response.answer.clone().unwrap_or_default();
                if answer.trim().is_empty() {
                    return Err(PhaseFailure::of(
                        FailureKind::Internal,
                        "the advisor reported `resolved` but wrote no answer",
                    ));
                }
                // Leave the resolution on the spec so it is not invisible —
                // a resolved fork carries the advisor's answer + rationale
                // into the spec's comment trail.
                let _ = std::process::Command::new(self.aida_exe())
                    .current_dir(&self.project_root)
                    .args([
                        "comment",
                        "add",
                        &self.spec,
                        &format!(
                            "Advisor resolved the punted design-fork (STORY-306).\n\n\
                             Decision: {answer}\n\nReasoning: {}",
                            response.reasoning
                        ),
                    ])
                    .status_retrying_etxtbsy();
                Ok(AdvisorOutcome::Resolved {
                    answer,
                    reasoning: response.reasoning.clone(),
                })
            }
            punt::PuntResolution::Escalated => {
                // Tag the spec `needs-human` (preserving existing tags) so
                // `aida findings` can tell an advisor-escalated fork from one
                // not yet triaged, and leave the advisor's reasoning — plus a
                // nudge toward the corpus-growth loop — as a comment.
                let mut tags: Vec<String> = target.tags.iter().cloned().collect();
                if !tags.iter().any(|t| t == "needs-human") {
                    tags.push("needs-human".to_string());
                }
                let _ = std::process::Command::new(self.aida_exe())
                    .current_dir(&self.project_root)
                    .args(["edit", &self.spec, "--tags", &tags.join(",")])
                    .status_retrying_etxtbsy();
                let _ = std::process::Command::new(self.aida_exe())
                    .current_dir(&self.project_root)
                    .args([
                        "comment",
                        "add",
                        &self.spec,
                        &format!(
                            "Advisor escalated this design-fork to a human (STORY-306).\n\n\
                             Reasoning: {}\n\nWhen you resolve this, record the answer \
                             (a memory, an acceptance-criteria edit, or a discipline doc) \
                             so a future advisor can resolve the same kind of fork as a \
                             recorded principle.",
                            response.reasoning
                        ),
                    ])
                    .status_retrying_etxtbsy();
                Ok(AdvisorOutcome::Escalated {
                    reason: response.reasoning.clone(),
                    category: response
                        .escalation_reason
                        .clone()
                        .unwrap_or_else(|| "unspecified".to_string()),
                })
            }
        }
    }

    /// STORY-306 advisor tier — resume the punted phase-1 implementer session
    /// with the advisor's judged `answer` (or, under `--escalate-defaults`, an
    /// authorization to ship the defensible default). `--resume`s the exact
    /// phase-1 Claude session so the implementer keeps the working model it
    /// built before punting; the worktree survived because STORY-306's punt
    /// path no longer ends the session. Classifies the outcome from ground
    /// truth — a re-punt leaves the spec in `NeedsAttention`, a ship opens a
    /// PR. trace:STORY-306 | ai:claude
    fn resume_implementer(
        &mut self,
        answer: &str,
    ) -> Result<auto_complete::ImplementerOutcome, auto_complete::PhaseFailure> {
        use auto_complete::{FailureKind, ImplementerOutcome, PhaseFailure};

        let session_id = self.implementer_session.clone().ok_or_else(|| {
            PhaseFailure::of(
                FailureKind::Internal,
                "advisor tier: no implementer session id to resume",
            )
        })?;
        let worktree = self.implementer_worktree.clone().ok_or_else(|| {
            PhaseFailure::of(
                FailureKind::Internal,
                "advisor tier: no implementer worktree recorded for the resume",
            )
        })?;

        let prompt = format!(
            "You punted spec {spec} on a design-fork. A headless advisor has now \
             judged it. Apply this decision and finish the spec — do NOT punt \
             again.\n\nADVISOR DECISION:\n{answer}\n\nTake the spec from Needs \
             Attention back to In Progress (`aida edit {spec} --status \
             in-progress`), implement the decision, and open the PR with \
             `/aida-pr` before exiting.",
            spec = self.spec,
        );

        let resume_uuid = uuid::Uuid::now_v7().to_string();
        let log_path = self
            .project_root
            .join(".aida")
            .join("headless-logs")
            .join(format!(
                "resume-{}-{}.jsonl",
                self.spec,
                &resume_uuid[..resume_uuid.len().min(8)]
            ));
        if !self.json {
            eprintln!(
                "  {} resuming the implementer session with the advisor's answer…",
                "◆".cyan()
            );
        }
        // TASK-307: tee the advisor-resume leg too. Label = "resume" so an
        // operator watching a drain can tell the resumed implementer apart
        // from the surrounding `[headless]` chatter. trace:TASK-307
        let tee_opts =
            crate::headless_tee::TeeOptions::from_env_and_flag(false).with_label("resume");
        let status = session::spawn_claude_headless_resume(
            &prompt,
            &session_id,
            &log_path,
            &worktree,
            &tee_opts,
            false,
            "implementer",
        )
        .map_err(|e| {
            PhaseFailure::of(
                FailureKind::Spawn,
                format!("advisor tier: could not resume the implementer session: {e}"),
            )
        })?;
        if !status.success() {
            // BUG-266: same Anthropic-API outage classification on the
            // advisor-resume leg — `spawn_claude_headless_resume` is the
            // same headless `claude -p` substrate, so a 529/5xx mid-resume
            // is still Inconclusive (terminal here per the BUG-257 path: no
            // second advisor round, drain pauses for retry). The resume
            // hint points at the SAME session id so the operator's next
            // retry resumes where the API outage interrupted.
            // trace:BUG-266 | ai:claude
            if let Ok(content) = std::fs::read_to_string(&log_path) {
                if let Some(reason_line) = claude_log_indicates_api_outage(&content) {
                    let hint = format!(
                        "Anthropic API was unavailable; resume with: \
                         `aida queue work {spec} --resume {session_id}`",
                        spec = self.spec,
                    );
                    return Ok(ImplementerOutcome::Inconclusive {
                        reason: format!(
                            "Anthropic API outage during the advisor-resume implementer: {reason_line}"
                        ),
                        retry_hint: Some(hint),
                    });
                }
            }
            return Err(PhaseFailure::new(format!(
                "the resumed implementer session exited {} — see {}",
                status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "with a signal".to_string()),
                log_path.display(),
            )));
        }

        // Classify from ground truth (the STORY-276 detect-punt logic): a
        // re-punt leaves the spec back in `NeedsAttention`.
        if matches!(
            spec_status(&self.project_root, &self.spec),
            Some(RequirementStatus::NeedsAttention)
        ) {
            let reason = load_store_for_lookup(&self.project_root)
                .and_then(|s| {
                    s.get_requirement_by_spec_id(&self.spec)
                        .and_then(|r| r.attention_reason.clone())
                })
                .map(|a| a.detail)
                .unwrap_or_else(|| "the resumed implementer punted again".to_string());
            return Ok(ImplementerOutcome::Punted { reason });
        }

        // A ship opens a PR — a spec-id lookup is branch-independent and
        // robust, with the recorded branch as a fallback.
        let pr =
            match detect_open_pr_for_spec_via_forge(&self.project_root, &self.spec) {
                PrLookup::Found(pr) => Some(pr),
                // STORY-516: inner branch lookup forge-routed (the outer spec search
                // is the list op, routed later). trace:STORY-516 | ai:claude
                PrLookup::NoOpenPr => self.branch.as_deref().and_then(|b| {
                    match change_lookup_for_branch(&self.project_root, b) {
                        crate::forge::ChangeLookup::Found(c) => Some(OpenPrInfo {
                            number: c.id,
                            title: c.title.unwrap_or_default(),
                            url: c.url,
                            head_branch: (!c.branch.is_empty()).then_some(c.branch),
                        }),
                        _ => None,
                    }
                }),
                PrLookup::GhMissing => {
                    return Err(PhaseFailure::of(
                        FailureKind::MissingTool,
                        "`gh` is not on PATH — auto-complete needs it to track the PR",
                    ));
                }
                PrLookup::GhFailed(why) => {
                    return Err(PhaseFailure::new(format!(
                        "could not look up the PR after the implementer resume: {why}"
                    )));
                }
                // BUG-257: a transient GH-API outage on the resumed-implementer PR
                // lookup is Inconclusive, not a failure. Drain pauses; the next
                // retry re-attempts the lookup once the API is reachable.
                // trace:BUG-257 | ai:claude
                PrLookup::GhUnreachable(why) => {
                    return Ok(ImplementerOutcome::Inconclusive {
                        reason: format!("GH API unreachable after the implementer resume: {why}"),
                        retry_hint: None,
                    });
                }
            };
        match pr {
            Some(pr) => Ok(self.adopt_resumed_pr(pr)),
            None => Err(PhaseFailure::of(
                FailureKind::NoPr,
                "the resumed implementer session exited but opened no PR — \
                 the advisor's answer did not produce shippable work",
            )),
        }
    }

    /// TASK-358: stamp the phase-1 implementer's lease as
    /// `escalated_to_human` once the orchestrator's `--escalate-blocks`
    /// path has decided this punt will not be resumed. The marker is read
    /// by `aida edit --status` (out of `NeedsAttention`) and
    /// `aida session prune --escalations` to know the lingering worktree
    /// is safe to remove. Best-effort: a stamp failure must not block the
    /// terminal `finish_escalated` — the cleanup just falls to the explicit
    /// prune verb instead. trace:TASK-358 | ai:claude
    fn mark_implementer_lease_escalated(&mut self) {
        let Some(lease_id) = self.implementer_lease.clone() else {
            return;
        };
        if let Err(e) = mark_lease_escalated_to_human(&self.project_root, &lease_id) {
            eprintln!(
                "  {} could not stamp lease {} as escalated: {e} — \
                 `aida session prune --escalations` can clean it up later",
                "Note:".dimmed(),
                lease_id,
            );
        }
    }

    // EPIC-28: park the spec in NeedsAttention with a structured FailureReason
    // so the batch drain can continue past this failure. trace:EPIC-28 | ai:claude
    fn shelve_on_failure(
        &mut self,
        spec: &str,
        phase: auto_complete::Phase,
        failure: &auto_complete::PhaseFailure,
        recovery_hint: &str,
    ) -> anyhow::Result<Option<aida_core::FailureReason>> {
        // TASK-1444 / BUG-1510: a reviewer-verdict shelve must ALWAYS land
        // on the lease's own spec — the drain only holds a lease on `spec`,
        // and flipping some other spec's status because a commit trailer
        // *mentions* it would be its own attribution error (that spec's
        // owner never asked this drain to touch it). What changes on
        // attribution is not the TARGET, only the NOTE: when the PR's
        // commits confidently credit a different spec, or the attribution
        // can't be confirmed either way, the lease spec's FailureReason
        // detail says so explicitly instead of reading as a silent,
        // unconditional "this spec failed review".
        // trace:TASK-1444 | ai:claude
        let detail: String = if matches!(
            failure.kind,
            auto_complete::FailureKind::VerdictRequestChanges
                | auto_complete::FailureKind::VerdictReject
        ) {
            match resolve_shelve_gate_range(&self.project_root, self.branch.as_deref()) {
                Some(range) => match read_commits_in_range(&self.project_root, &range) {
                    Ok(commits) => match decide_shelve_attribution(&commits, spec) {
                        ShelveAttribution::Confirmed(_) => failure.reason.clone(),
                        ShelveAttribution::Reattributed(other) => format!(
                            "{} (attribution: this verdict's commits carry {}'s trailer, not this spec's)",
                            failure.reason, other
                        ),
                        ShelveAttribution::Uncertain(note) => format!(
                            "{} (attribution uncertain: {})",
                            failure.reason, note
                        ),
                    },
                    // A git hiccup reading the commit range must not block
                    // the shelve itself — fall back to no note, same as
                    // pre-TASK-1444 behaviour.
                    Err(_) => failure.reason.clone(),
                },
                // Couldn't resolve the PR's own branch (no `self.branch`,
                // and no local/origin ref for it) — the attribution is
                // Uncertain, never a guess dressed up as Reattributed.
                None => format!(
                    "{} (attribution uncertain: could not resolve the PR branch to read its commits)",
                    failure.reason
                ),
            }
        } else {
            failure.reason.clone()
        };
        shelve_spec_on_failure(
            &self.project_root,
            spec,
            phase.slug(),
            phase.index() as u8,
            failure.kind.cause_slug(),
            &detail,
            recovery_hint,
        )
    }

    // trace:BUG-1291 | ai:codex
    fn handoff_open_pr_after_shelve(
        &mut self,
        spec: &str,
        phase: auto_complete::Phase,
        failure: &auto_complete::PhaseFailure,
    ) {
        let Some(pr) = self.pr_number else {
            return;
        };
        let Some(story_id) =
            open_pr_review_story_using(&self.project_root, pr as u64, &self.aida_exe)
        else {
            // Normally `/aida-pr` already filed the story. Re-run that
            // idempotent path as a repair before looking it up once more.
            if let Some(branch) = self.branch.as_deref() {
                let _ = try_auto_queue_pr_review(
                    &self.project_root,
                    branch,
                    "shelve-recovery",
                    AutoQueueOrigin::PrSkill,
                );
            }
            let Some(story_id) =
                open_pr_review_story_using(&self.project_root, pr as u64, &self.aida_exe)
            else {
                eprintln!(
                    "  {} shelved {} with PR #{} but could not resolve its review story",
                    "Warning:".yellow().bold(),
                    spec,
                    pr
                );
                return;
            };
            let note = format!(
                "BUG-1291 recovery: PR #{} handed off after shelving in phase {} ({}): {}",
                pr,
                phase.index(),
                phase.slug(),
                failure.reason
            );
            if let Err(err) = aida_subcmd_queue_add_for_reviewer_using(
                &self.project_root,
                &story_id,
                &note,
                &self.aida_exe,
            ) {
                eprintln!(
                    "  {} shelved {} with PR #{} but reviewer handoff failed: {}; {} remains as a durable retry signal",
                    "Warning:".yellow().bold(),
                    spec,
                    pr,
                    err,
                    story_id
                );
            }
            return;
        };
        let note = format!(
            "BUG-1291 recovery: PR #{} handed off after shelving in phase {} ({}): {}",
            pr,
            phase.index(),
            phase.slug(),
            failure.reason
        );
        if let Err(err) = aida_subcmd_queue_add_for_reviewer_using(
            &self.project_root,
            &story_id,
            &note,
            &self.aida_exe,
        ) {
            eprintln!(
                "  {} shelved {} with PR #{} but reviewer handoff failed: {}; {} remains as a durable retry signal",
                "Warning:".yellow().bold(),
                spec,
                pr,
                err,
                story_id
            );
        }
    }

    /// TASK-975: the `[drain] ci_auto_fix` budget (env override
    /// `AIDA_CI_AUTO_FIX`), resolved once into `drain_tuning`. `0` (the
    /// default) keeps the CI-fix loop and the merge-conflict rebase off.
    // trace:TASK-975 | ai:claude
    fn ci_fix_budget(&self) -> usize {
        self.drain_tuning.ci_auto_fix
    }

    // trace:STORY-975 | ai:codex
    fn transient_retry_budget(&self) -> usize {
        self.drain_tuning.retry_transient
    }

    // trace:STORY-975 | ai:codex
    fn record_transient_retry(
        &mut self,
        spec: &str,
        phase: auto_complete::Phase,
        cause: &str,
        attempt: u32,
        max: u32,
        detail: Option<&str>,
    ) {
        let vendor = session::resolve_headless_vendor(&self.project_root);
        let seat = agent_seat_for_phase(phase);
        let tuning = session::resolve_agent_tuning(&self.project_root, vendor, seat);
        let model_before = tuning.model;
        let model_after = if self.drain_tuning.retry_escalate_model && attempt == 2 {
            model_before.as_deref().map(|model| {
                let tiers = aida_core::agents_config::resolve_agent_model_tiers(
                    &self.project_root,
                    vendor.as_str(),
                );
                match aida_core::agents_config::next_model_tier(model, &tiers.tiers) {
                    Some(next) => {
                        std::env::set_var("AIDA_AGENT_MODEL", &next);
                        next
                    }
                    None => {
                        eprintln!(
                            "  {} retry model escalation skipped: `{}` has no next tier for vendor `{}`",
                            "Note:".dimmed(),
                            model,
                            vendor.as_str()
                        );
                        model.to_string()
                    }
                }
            })
        } else {
            model_before.clone()
        };
        let phase_label = format!("{} ({})", phase.index(), phase.slug());
        drain_state::append_phase_retry(
            &self.project_root,
            spec,
            &phase_label,
            cause,
            attempt,
            max,
        );
        let (_, run_uuid) = drain_state::current_context(&self.project_root);
        events::emit(
            &self.project_root,
            &events::Event::new(
                Some(spec.to_string()),
                run_uuid,
                events::EventKind::SpecRetried {
                    phase: phase.slug().to_string(),
                    cause: cause.to_string(),
                    attempt,
                    max,
                    model_before,
                    model_after,
                    // trace:BUG-1299 | ai:claude
                    detail: detail.map(str::to_string),
                },
            ),
        );
    }

    // trace:BUG-908 | ai:codex
    fn prepare_transient_retry(
        &mut self,
        spec: &str,
        phase: auto_complete::Phase,
        _cause: &str,
    ) -> Result<(), auto_complete::PhaseFailure> {
        if phase != auto_complete::Phase::Implementer {
            return Ok(());
        }
        if let Some((branch, worktree, lease_id)) =
            reclaim_implementer_retry_predecessor(&self.project_root, spec)?
        {
            if !self.json {
                eprintln!(
                    "  {} released predecessor lease {} on `{spec}`; retrying in {}",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    (&lease_id[..lease_id.len().min(8)]).yellow(),
                    worktree.display(),
                );
            }
            self.retry_implementer_branch = Some(branch);
            self.retry_implementer_worktree = Some(worktree);
        }
        Ok(())
    }

    /// TASK-975: one in-drain CI-fix cycle — spawn a headless fix session
    /// (vendor-routed, same adapter as the other drain phases) in the
    /// still-leased phase-1 worktree with the failing-check log + spec
    /// context; the session commits and pushes the fix to the PR branch.
    /// Returns `true` only when origin's branch head actually advanced (the
    /// ground truth that a re-poll is meaningful).
    // trace:TASK-975 | ai:claude
    fn attempt_ci_fix(
        &mut self,
        attempt: usize,
        budget: usize,
        failure: &auto_complete::PhaseFailure,
    ) -> bool {
        let Some(branch) = self.branch.clone() else {
            return false;
        };
        // The fix session needs a checkout of the PR branch. Phase 1's
        // worktree is still leased (the session only ends once CI clears), so
        // host the fix there. A resumed drain that skipped phase 1 has no
        // worktree — fall through to shelve rather than improvising one.
        let Some(worktree) = self.implementer_worktree.clone() else {
            eprintln!(
                "  {} no implementer worktree in this run (resumed drain?) — \
                 cannot host a CI-fix session; leaving the red-CI failure standing",
                crate::glyph(crate::glyphs::Glyph::Info).cyan()
            );
            return false;
        };
        eprintln!("  🔧 CI is red on `{branch}` — attempting in-drain fix {attempt}/{budget}…");
        let run_id = self
            .ci_run_id
            .clone()
            .or_else(|| latest_run_id_for_branch(&branch));
        let log_excerpt = run_id
            .as_deref()
            .and_then(|id| failing_ci_log_excerpt(&self.project_root, id))
            .unwrap_or_default();
        let head_before = remote_head_sha(&self.project_root, &branch);
        let prompt = build_ci_fix_prompt(&self.spec, &branch, &failure.reason, &log_excerpt);

        let vendor = session::resolve_headless_vendor(&self.project_root);
        let fix_uuid = uuid::Uuid::now_v7().to_string();
        let log_path = self
            .project_root
            .join(".aida")
            .join("headless-logs")
            .join(format!(
                "ci-fix-{}-a{}-{}.jsonl",
                self.spec,
                attempt,
                &fix_uuid[..fix_uuid.len().min(8)]
            ));
        if let Some(dir) = log_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let log = match std::fs::File::create(&log_path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!(
                    "  {} could not create the CI-fix headless log ({e}) — \
                     leaving the red-CI failure standing",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
                return false;
            }
        };
        let tee_opts =
            crate::headless_tee::TeeOptions::from_env_and_flag(false).with_label("ci-fix");
        let tee = crate::headless_tee::start_tee(&log_path, &tee_opts);
        // BUG-342 posture: the argv comes from the shared vendor-neutral
        // builder (mirrors the advisor tier's spawn), never a hand-rolled
        // `claude -p`.
        let program = session::resolve_agent_program(vendor.program());
        let fix_args = session::headless_vendor_args(vendor, &prompt, &fix_uuid, false, None, None);
        // trace:TASK-1169 | ai:claude
        let (ceiling_key, ceiling_value) = crate::bg_wait_ceiling_env(Some(&self.project_root));
        let status = std::process::Command::new(&program)
            .current_dir(&worktree)
            .args(fix_args)
            .env("AIDA_HEADLESS", "1")
            // TASK-1169 / ADR-22: bounded, launcher-set background-wait ceiling.
            .env(ceiling_key, ceiling_value)
            .stdout(std::process::Stdio::from(log))
            .status_retrying_etxtbsy();
        tee.stop();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                eprintln!(
                    "  {} the CI-fix session exited {} — see {}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    s.code()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "with a signal".to_string()),
                    log_path.display()
                );
                return false;
            }
            Err(e) => {
                eprintln!(
                    "  {} could not launch the CI-fix session ({e}) — \
                     leaving the red-CI failure standing",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
                return false;
            }
        }
        // Ground truth: only a pushed change makes the re-poll meaningful. A
        // session that exited clean but pushed nothing (it judged the failure
        // unfixable) proceeds to shelve.
        let head_after = remote_head_sha(&self.project_root, &branch);
        let pushed = match (&head_before, &head_after) {
            (_, None) => false,
            (before, Some(after)) => before.as_deref() != Some(after.as_str()),
        };
        if pushed {
            eprintln!(
                "  {} fix pushed to `{}` — re-polling CI…",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                branch
            );
            // The next CI probe belongs to the fresh push; drop the stale
            // run id so a later failure captures the new run.
            self.ci_run_id = None;
            true
        } else {
            eprintln!(
                "  {} the CI-fix session pushed no change — leaving the \
                 red-CI failure standing (see {})",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                log_path.display()
            );
            false
        }
    }

    /// TASK-975: one in-drain rebase after a merge-conflict phase-4 failure —
    /// the same `pr rebase <N> --no-smoke` subprocess as the STORY-429
    /// phase-3 stale-base auto-rebase. `true` = rebased clean and pushed; the
    /// orchestrator retries the merge once.
    // trace:TASK-975 | ai:claude
    fn attempt_merge_conflict_rebase(&mut self, _failure: &auto_complete::PhaseFailure) -> bool {
        let Some(pr) = self.pr_number else {
            return false;
        };
        eprintln!(
            "  {} merge hit a conflict on PR-{pr} — attempting one in-drain rebase…",
            "↻".cyan()
        );
        let record = |events: &mut Vec<auto_complete_telemetry::AutoRebaseEvent>,
                      outcome: String| {
            events.push(auto_complete_telemetry::AutoRebaseEvent {
                phase: auto_complete::Phase::Merge.index() as u8,
                pr_number: pr as u64,
                outcome,
            });
        };
        let status = std::process::Command::new(self.aida_exe())
            .current_dir(&self.project_root)
            .args(build_phase3_auto_rebase_args(pr as u64))
            .status_retrying_etxtbsy();
        match status {
            Ok(s) if s.success() => {
                record(&mut self.auto_rebase_events, "clean".to_string());
                eprintln!(
                    "  {} PR-{pr} rebased cleanly — retrying the merge",
                    crate::glyph(crate::glyphs::Glyph::Check).green().bold()
                );
                true
            }
            Ok(_) => {
                record(&mut self.auto_rebase_events, "conflict".to_string());
                eprintln!(
                    "  {} the rebase itself conflicted — leaving the merge \
                     failure standing",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
                false
            }
            Err(e) => {
                record(&mut self.auto_rebase_events, format!("failed:{e}"));
                eprintln!(
                    "  {} could not run the rebase ({e}) — leaving the merge \
                     failure standing",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
                false
            }
        }
    }
}

/// BUG-1930: the failure for a headless implementer that opened no PR and
/// made no commit, when its stream recorded permission denials; `None`
/// otherwise (no log, or no denials), leaving the ordinary NoPr path.
// trace:BUG-1930 | ai:claude
fn no_pr_permission_denied_failure(
    project_root: &std::path::Path,
    session_uuid: &str,
) -> Option<auto_complete::PhaseFailure> {
    let log = crate::questions::read_headless_log_for_session(project_root, session_uuid)?;
    let denied = crate::questions::headless_log_permission_denied_tools(&log);
    (!denied.is_empty()).then(|| auto_complete::permission_denied_failure(&denied))
}

#[cfg(test)]
mod bug_1930_tests {
    use super::*;

    /// BUG-1930: the NoPr exit reads the session's own headless log; denials
    /// there turn it into PermissionDenied, and a clean log (or none) keeps
    /// the ordinary NoPr path.
    // trace:BUG-1930 | ai:claude
    #[test]
    fn no_pr_with_denied_stream_becomes_permission_denied() {
        let dir = tempfile::tempdir().expect("tempdir");
        let logs = dir.path().join(".aida").join("headless-logs");
        std::fs::create_dir_all(&logs).expect("log dir");
        std::fs::write(
            logs.join("bug-81-denied-sess.jsonl"),
            "{\"type\":\"result\",\"permission_denials\":[{\"tool_name\":\"Edit\"},\
             {\"tool_name\":\"Write\"}]}\n",
        )
        .expect("denied log");
        std::fs::write(
            logs.join("bug-83-clean-sess.jsonl"),
            "{\"type\":\"result\",\"permission_denials\":[]}\n",
        )
        .expect("clean log");

        let failure = no_pr_permission_denied_failure(dir.path(), "denied-sess")
            .expect("denials classify the stop");
        assert_eq!(failure.kind, auto_complete::FailureKind::PermissionDenied);
        assert!(failure.reason.contains("Edit, Write"), "{}", failure.reason);

        assert_eq!(
            no_pr_permission_denied_failure(dir.path(), "clean-sess"),
            None
        );
        assert_eq!(
            no_pr_permission_denied_failure(dir.path(), "missing-sess"),
            None
        );
    }
}
