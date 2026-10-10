// TASK-1297: one routed-queue row, reduced to the two fields the "what did
// the `--batch` filter exclude" count needs — decoupled from `Storage` /
// `QueueEntry` so the pure derivation below can be pinned with a plain
// fixture instead of a live store.
// trace:TASK-1297 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct RoutedSpecSnapshot {
    pub(crate) tags: Vec<String>,
    pub(crate) status: aida_core::RequirementStatus,
}

// TASK-1297: pure count of "M other approved specs routed to this role,
// excluded from the batch by the `--batch` filter" — the number a mis-scoped
// `--batch` launcher silently drove past for hours (2026-09-19 incident: 3
// open batch members re-driven every wave while 16 approved, role-routed
// specs sat outside the filter). `want_tag` is the `batch:<name>` tag
// (case-insensitively) that marks batch membership; a snapshot carrying it
// is a member and is never counted, regardless of status — only an
// approved, non-member routed spec counts, matching the acceptance wording
// ("M other approved specs routed to role:X are not in this batch"). Pure so
// a fixture pins the derivation without a live queue/store.
// trace:TASK-1297 | ai:claude
pub(crate) fn count_routed_excluded_from_batch(
    snapshots: &[RoutedSpecSnapshot],
    want_tag: &str,
) -> usize {
    snapshots
        .iter()
        .filter(|s| {
            s.status == aida_core::RequirementStatus::Approved
                && !s.tags.iter().any(|t| t.eq_ignore_ascii_case(want_tag))
        })
        .count()
}

// TASK-1297: impure shell for `count_routed_excluded_from_batch` — reads the
// same role-routed queue `resolve_batch_members_with_context` reads
// (`queue_list_with_role_fallback` + `entry_matches_role_filter` +
// `Storage::resolve_queued_requirement`), so the count agrees with batch
// resolution under the BUG-1264 alias fallback (a member whose object lives
// under its origin_id alias resolves the same way here as it does for
// membership). Returns the count plus the resolved role label (for the
// human-facing line), `None` when role resolution fails. Best-effort: a
// queue-read failure surfaces as `Err` so the caller can skip the line
// rather than print a wrong number.
// trace:TASK-1297 | ai:claude
pub(crate) fn count_batch_excluded_routed_specs(
    storage: &Storage,
    user_id: &str,
    batch_name: &str,
    role: Option<&str>,
) -> Result<(usize, Option<String>)> {
    let want = format!("batch:{}", batch_name);
    let session_role = std::env::var("AIDA_SESSION_ROLE").ok();
    let (role_filter, _only_unrouted) =
        resolve_queue_role_filter(role, false, session_role.as_deref());
    let entries = queue_role_fallback::queue_list_with_role_fallback(
        storage,
        user_id,
        role_filter.as_deref(),
        false,
    )?;
    let mut snapshots = Vec::with_capacity(entries.len());
    for entry in entries {
        if !entry_matches_role_filter(entry.for_role.as_deref(), role_filter.as_deref(), false) {
            continue;
        }
        if let Some(req) = storage.resolve_queued_requirement(&entry.requirement_id)? {
            snapshots.push(RoutedSpecSnapshot {
                tags: req.tags.into_iter().collect(),
                status: req.status,
            });
        }
    }
    Ok((
        count_routed_excluded_from_batch(&snapshots, &want),
        role_filter,
    ))
}

// TASK-1297: the operator-facing "M other approved specs routed to role:X
// are not in this batch" clause — `None` when `excluded == 0` so a
// non-filtering batch drain (or one that filtered nothing away) renders no
// extra noise, per acceptance.
// trace:TASK-1297 | ai:claude
pub(crate) fn batch_exclusion_clause(excluded: usize, role_label: Option<&str>) -> Option<String> {
    if excluded == 0 {
        return None;
    }
    let role_phrase = match role_label {
        Some(r) => format!("role:{r}"),
        None => "this role".to_string(),
    };
    let (plural, verb) = if excluded == 1 {
        ("", "is")
    } else {
        ("s", "are")
    };
    Some(format!(
        "{excluded} other approved spec{plural} routed to {role_phrase} {verb} not in this batch"
    ))
}

#[cfg(test)]
#[path = "tests/task_1297_batch_exclusion_tests.rs"]
mod task_1297_batch_exclusion_tests;

/// Real [`auto_complete::BatchDriver`] — re-resolves the `batch:NAME` head
/// against the live queue and runs each member's full `--auto-complete`
/// lifecycle. trace:TASK-285 | ai:claude
pub(crate) struct RealBatchDriver<'a> {
    pub(crate) storage: &'a Storage,
    pub(crate) user_id: String,
    pub(crate) batch_name: String,
    pub(crate) role: Option<String>,
    pub(crate) variant: auto_complete::AutoCompleteVariant,
    pub(crate) json: bool,
    pub(crate) permission_mode: Option<String>,
    /// STORY-263: headless mode, propagated to every member's
    /// `run_auto_complete`.
    pub(crate) no_human: Option<auto_complete::NoHumanMode>,
    /// STORY-306: advisor-escalation mode, propagated to every member.
    pub(crate) escalate_mode: auto_complete::EscalateMode,
    /// BUG-311: outer `--steal` flag, propagated to every member's phase-1
    /// subprocess so a dormant lease on the member's scope is cleared rather
    /// than blocking the drain with the canned "pass --steal" message.
    pub(crate) steal: bool,
    /// TASK-559: outer `--force-claim` flag, propagated to every member's
    /// phase-1 subprocess so ambiguous spec status can be claimed.
    pub(crate) force_claim: bool,
    /// STORY-281: outer `--allow-stale-base` flag, propagated to every
    /// member's phase-3 pre-flight check so the stale-base + overlap
    /// signal becomes a warning instead of a refusal.
    pub(crate) allow_stale_base: bool,
    /// STORY-429: outer `--no-auto-rebase` flag, propagated to every member.
    pub(crate) no_auto_rebase: bool,
    // TASK-966: project root + drain-start for the `--max-tokens` meter. `None`
    // when no token cap is active. trace:TASK-966 | ai:claude
    pub(crate) token_meter: Option<(std::path::PathBuf, std::time::SystemTime)>,
    /// STORY-1091: configured in-flight implementer/CI window.
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    pub(crate) pipeline_depth: usize,
    /// STORY-1091: child `aida queue work <spec> --auto-complete=through-ci`
    /// processes keyed by opaque scheduler handles.
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    pub(crate) pipelined_children: std::collections::HashMap<usize, std::process::Child>,
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    pub(crate) pipelined_result_paths: std::collections::HashMap<usize, std::path::PathBuf>,
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    pub(crate) next_pipelined_handle: usize,
}

impl auto_complete::BatchDriver for RealBatchDriver<'_> {
    fn next_head(&mut self) -> Option<String> {
        match resolve_batch_members(
            self.storage,
            &self.user_id,
            &self.batch_name,
            self.role.as_deref(),
        ) {
            Ok(members) => members.into_iter().next().map(|m| m.1),
            Err(e) => {
                // A store/queue read failure is not "batch drained" — surface
                // it and stop rather than reporting a false clean drain.
                eprintln!(
                    "{} could not resolve batch members: {}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    e
                );
                std::process::exit(1);
            }
        }
    }

    fn run_spec(&mut self, spec: &str) -> auto_complete::OrchestrationResult {
        // BUG-660: retry a transient/retryable per-spec failure (locked cache,
        // GH-API blip) through an exponential backoff instead of hammering or
        // immediately shelving — an unattended drive rides out a brief blip.
        // trace:BUG-660 | ai:claude
        drive_robustness::run_with_transient_backoff(|| {
            run_auto_complete(
                self.storage,
                &self.user_id,
                spec,
                self.variant,
                self.json,
                self.permission_mode.as_deref(),
                self.no_human,
                self.escalate_mode,
                // STORY-301: a batch / nextN member does not own the drain-state
                // file — the batch orchestrator created it.
                false,
                self.steal,
                self.force_claim,
                self.allow_stale_base,
                self.no_auto_rebase,
                None,
            )
        })
    }

    // TASK-966: cumulative reported tokens across this drain's headless logs.
    fn cumulative_tokens(&mut self) -> u64 {
        match &self.token_meter {
            Some((root, since)) => sum_headless_log_tokens(root, *since),
            None => 0,
        }
    }
}

/// The environment a pipelined batch child is spawned with.
///
/// A pure helper for the same reason `integrate::drive_args` is one: the env is
/// load-bearing and otherwise only assertable by spawning a real drain. The
/// borrow flag in particular is invisible in the argv, so a guardrail that only
/// reads arguments cannot see it go missing — which is how BUG-1568 shipped.
// trace:BUG-1568 | ai:claude
pub(crate) fn pipelined_child_env(result_path: &std::path::Path) -> Vec<(&'static str, String)> {
    vec![
        ("AIDA_PIPELINED_BATCH_CHILD", "1".to_string()),
        // The child is a `queue work --auto-complete` in its own right and
        // takes the drain lock at its top, while the parent batch drive still
        // holds it. Borrow rather than acquire. See the call sites.
        ("AIDA_DRAIN_BORROW", "1".to_string()),
        (
            "AIDA_PIPELINED_RESULT_PATH",
            result_path.display().to_string(),
        ),
    ]
}

/// BUG-1570: a pipelined child that exits with an unrecognised code and leaves
/// no result sidecar did NOT report a drive outcome for this spec — and may
/// never have reached the spec at all. A refused drain lock, a spawn failure
/// and a missing binary all land here identically.
///
/// Reporting that as the SPEC's CI phase failing is what let a drain-level lock
/// refusal spend 17.5 hours attributed to an innocent spec: the batch summary
/// read "drain stopped at BUG-1462 (phase 2 failed)", so every reader went to
/// debug BUG-1462, which had nothing wrong with it.
///
/// `FailureKind::Internal` is deliberate. This is an orchestrator-layer fault,
/// which makes it un-shelvable — the spec must not be parked NeedsAttention for
/// something it did not do, and a hint that routes to AIDA rather than to the
/// spec's CI is the correct destination.
// trace:BUG-1570 | ai:claude
pub(crate) fn pipelined_child_reported_nothing(
    spec: &str,
    code: Option<i32>,
) -> auto_complete::OrchestrationResult {
    let exit = code
        .map(|c| format!("exit code {c}"))
        .unwrap_or_else(|| "a signal".to_string());
    let mut result = auto_complete::OrchestrationResult::failed(auto_complete::Phase::Ci);
    result.failure = Some(auto_complete::PhaseFailure::of(
        auto_complete::FailureKind::Internal,
        format!(
            "the pipelined child for {spec} ended with {exit} without reporting a drive \
             outcome, so it may never have started {spec} at all — a refused drain lock, a \
             failed spawn and a missing binary are indistinguishable here. This is an \
             orchestrator-layer fault, NOT a phase failure of {spec}; read the child's own \
             drain log before treating {spec} as the problem."
        ),
    ));
    result
}

/// BUG-1570: decide what a finished pipelined child's exit MEANS.
///
/// Pure, and shared by both pipelined drivers, so the classification is pinned
/// in BOTH directions at one seam. That pairing is the point: a child that
/// reported NOTHING must become an orchestrator-layer `Internal` fault, and a
/// child that DID report a drive outcome — a real CI failure it already shelved
/// the spec for — must keep travelling the shelvable path. Fixing the first
/// without pinning the second would flatten every genuine shelve into
/// `Internal`, which is un-shelvable, and the batch drain would stop dead
/// instead of continuing past a red member.
///
/// `sidecar` is a closure because reading the sidecar CONSUMES the file; the
/// unreported arm must not touch it.
// trace:BUG-1570 | ai:claude
pub(crate) fn classify_pipelined_child_outcome(
    spec: &str,
    exit_code: Option<i32>,
    success: bool,
    sidecar: impl FnOnce() -> Option<auto_complete::OrchestrationResult>,
) -> auto_complete::OrchestrationResult {
    if success {
        return sidecar().unwrap_or_else(auto_complete::OrchestrationResult::ok);
    }
    if exit_code == Some(auto_complete::DRIVE_EXIT_SHELVED) {
        return sidecar().unwrap_or_else(|| {
            let mut result = auto_complete::OrchestrationResult::failed(auto_complete::Phase::Ci);
            result.shelved_reason = Some(aida_core::FailureReason {
                phase: "ci".to_string(),
                phase_index: 2,
                kind: "pipelined-child-shelved".to_string(),
                detail: "pipelined implementer/CI child parked this spec".to_string(),
                recovery_hint: Some(
                    "inspect the child drain output and `aida findings list`".to_string(),
                ),
                shelved_by: Some("orchestrator".to_string()),
                shelved_at: chrono::Utc::now(),
            });
            result
        });
    }
    pipelined_child_reported_nothing(spec, exit_code)
}

/// Shared argv builder for a pipelined batch / `nextN` child drain
/// (`aida queue work <spec> --auto-complete=<mode> ...`), used by both
/// [`RealBatchDriver`] and [`RealNextNDriver`].
///
/// The `--escalate-blocks` / `--escalate-defaults` pair is pushed only when
/// the resolved no-human mode is [`auto_complete::NoHumanMode::Both`],
/// mirroring the child's STORY-306 kickoff validation: the advisor tier
/// exists only in a fully-headless drain, and the child rejects the flags
/// anywhere else. Pushing them unconditionally made every interactive
/// batch / `nextN` child exit at validation before doing any work.
// trace:STORY-1091 trace:ADR-28 | ai:codex
// trace:BUG-1805 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn pipelined_child_common_args(
    spec: &str,
    mode: auto_complete::AutoCompleteVariant,
    json: bool,
    permission_mode: Option<&str>,
    no_human: Option<auto_complete::NoHumanMode>,
    escalate_mode: auto_complete::EscalateMode,
    steal: bool,
    force_claim: bool,
    allow_stale_base: bool,
    no_auto_rebase: bool,
) -> Vec<String> {
    let mut args = vec![
        "queue".to_string(),
        "work".to_string(),
        spec.to_string(),
        format!("--auto-complete={}", mode.slug()),
    ];
    if json {
        args.push("--json".to_string());
    }
    if let Some(permission_mode) = permission_mode {
        args.push("--permission-mode".to_string());
        args.push(permission_mode.to_string());
    }
    if let Some(no_human) = no_human {
        args.push(format!("--no-human={}", no_human.slug()));
    }
    if no_human == Some(auto_complete::NoHumanMode::Both) {
        match escalate_mode {
            auto_complete::EscalateMode::Blocks => args.push("--escalate-blocks".to_string()),
            auto_complete::EscalateMode::Defaults => args.push("--escalate-defaults".to_string()),
        }
    }
    if steal {
        args.push("--steal".to_string());
    }
    if force_claim {
        args.push("--force-claim".to_string());
    }
    if allow_stale_base {
        args.push("--allow-stale-base".to_string());
    }
    if no_auto_rebase {
        args.push("--no-auto-rebase".to_string());
    }
    args
}

impl RealBatchDriver<'_> {
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    // trace:BUG-1805 | ai:claude
    pub(crate) fn child_common_args(
        &self,
        spec: &str,
        mode: auto_complete::AutoCompleteVariant,
    ) -> Vec<String> {
        pipelined_child_common_args(
            spec,
            mode,
            self.json,
            self.permission_mode.as_deref(),
            self.no_human,
            self.escalate_mode,
            self.steal,
            self.force_claim,
            self.allow_stale_base,
            self.no_auto_rebase,
        )
    }
}

impl auto_complete::PipelinedBatchDriver for RealBatchDriver<'_> {
    fn pipeline_depth(&self) -> usize {
        self.pipeline_depth
    }

    fn start_spec_through_ci(&mut self, spec: &str) -> auto_complete::PipelinedHandle {
        let handle = auto_complete::PipelinedHandle(self.next_pipelined_handle);
        self.next_pipelined_handle += 1;
        // trace:TASK-1603 | ai:codex
        // The child registers its run before bumping status. Pre-bumping here
        // creates an InProgress/no-lease window before it has ownership.
        let exe = aida_exe_path();
        let result_path = find_main_worktree_root()
            .unwrap_or_else(|_| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            })
            .join(".aida")
            .join("pipelined-results")
            .join(format!("{}-{}.json", spec, handle.0));
        let mut cmd = std::process::Command::new(exe);
        // BUG-1568: the child takes the drain lock at its top while the parent
        // batch drive still holds it, so it must BORROW. Without that it is
        // refused with "a drain is already running (pid <parent>)", the batch
        // driver reads the refusal as a phase failure of the member, and the
        // whole batch stops having shipped nothing.
        //
        // This is BUG-748's defect in a second place: that fix taught the
        // INTEGRATOR child to borrow, and the pipelined batch child never got
        // the same treatment. It only became reachable when pipeline_depth was
        // raised from 1 to 2 — at depth 1 no child overlaps a live parent lock.
        // Measured after that change: 221 consecutive drains shipping nothing
        // over 17.5 hours. trace:BUG-1568 trace:BUG-748 | ai:claude
        cmd.args(self.child_common_args(spec, auto_complete::AutoCompleteVariant::ThroughCi));
        for (k, v) in pipelined_child_env(&result_path) {
            cmd.env(k, v);
        }
        match cmd.spawn_retrying_etxtbsy() {
            Ok(child) => {
                self.pipelined_children.insert(handle.0, child);
                self.pipelined_result_paths.insert(handle.0, result_path);
                handle
            }
            Err(e) => {
                eprintln!(
                    "{} could not start pipelined implementer for {}: {}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    spec,
                    e
                );
                handle
            }
        }
    }

    fn wait_spec_through_ci(
        &mut self,
        handle: auto_complete::PipelinedHandle,
    ) -> auto_complete::OrchestrationResult {
        let Some(mut child) = self.pipelined_children.remove(&handle.0) else {
            return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Implementer);
        };
        let result_path = self.pipelined_result_paths.remove(&handle.0);
        // The sidecar is named `<spec>-<handle>.json`, and the handle is
        // numeric, so the last `-` separates them. Spec ids contain `-`
        // themselves (BUG-1462), which is why this splits from the RIGHT.
        // trace:BUG-1570 | ai:claude
        let spec_for_report = result_path
            .as_deref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .and_then(|s| s.rsplit_once('-').map(|(spec, _)| spec.to_string()))
            .unwrap_or_else(|| "the dispatched spec".to_string());
        let read_sidecar = || {
            result_path
                .as_deref()
                .and_then(read_pipelined_child_result_sidecar)
        };
        match child.wait() {
            Ok(status) => classify_pipelined_child_outcome(
                &spec_for_report,
                status.code(),
                status.success(),
                read_sidecar,
            ),
            // `wait()` itself failed, so there is no status to read at all.
            Err(_) => classify_pipelined_child_outcome(&spec_for_report, None, false, read_sidecar),
        }
    }

    fn finish_spec_after_ci(&mut self, spec: &str) -> auto_complete::OrchestrationResult {
        if i32::from(self.variant.last_phase()) <= auto_complete::Phase::Ci.index() {
            return auto_complete::OrchestrationResult::ok();
        }
        let project_root = match find_main_worktree_root() {
            Ok(root) => root,
            Err(_) => {
                return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Reviewer)
            }
        };
        let lookup = detect_open_pr_for_spec_via_forge(&project_root, spec);
        let (branch, pr) = match lookup {
            PrLookup::Found(pr) => (pr.head_branch, Some(pr.number as u32)),
            _ => return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Reviewer),
        };
        let resume = Some(ResumeEntry {
            // Re-probe phase 2 in this process so the reviewer carries a
            // head-bound CI conclusion, even if the pipelined child already
            // observed green. trace:BUG-1460 | ai:codex
            start_phase: auto_complete::Phase::Ci,
            branch,
            pr,
            head_sha: None,
            from_pr: true,
            ci_terminal_sha: None,
            ci_terminal_green: None,
        });
        run_auto_complete(
            self.storage,
            &self.user_id,
            spec,
            self.variant,
            self.json,
            self.permission_mode.as_deref(),
            self.no_human,
            self.escalate_mode,
            false,
            self.steal,
            self.force_claim,
            self.allow_stale_base,
            self.no_auto_rebase,
            resume,
        )
    }
}

/// Entry point for `aida queue work --batch NAME --auto-complete` (TASK-285).
/// Drains the whole batch. Depth 1 keeps the historical one-full-lifecycle per
/// member loop; depth >1 starts later implementer/CI children while this parent
/// serializes reviewer/merge/pull/build for ready PRs. Never returns.
// trace:TASK-285 STORY-1091
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_auto_complete_batch(
    storage: &Storage,
    user_id: &str,
    batch_name: &str,
    variant: auto_complete::AutoCompleteVariant,
    json: bool,
    permission_mode: Option<&str>,
    role: Option<&str>,
    max: Option<usize>,
    // EPIC-28: outer `--max-failures` cap — after this many phase failures
    // shelve in a single batch, stop the drain entirely (the env is
    // probably broken). `None` falls back to the built-in default (5).
    // trace:EPIC-28 | ai:claude
    max_failures: Option<usize>,
    no_human: Option<auto_complete::NoHumanMode>,
    escalate_mode: auto_complete::EscalateMode,
    // BUG-311: outer `--steal`, threaded to every batch member's phase 1.
    steal: bool,
    // TASK-559: outer `--force-claim`, threaded to every batch member's phase 1.
    force_claim: bool,
    // STORY-281: outer `--allow-stale-base`, threaded to every batch
    // member's phase 3 pre-flight stale-base check.
    allow_stale_base: bool,
    // STORY-429: outer `--no-auto-rebase`, threaded to every batch member.
    no_auto_rebase: bool,
    // TASK-966: hard budget caps for the whole drain. trace:TASK-966 | ai:claude
    caps: &drain_caps::DrainCaps,
) -> ! {
    // TASK-1297: resolve the batch head AND the "M other approved specs
    // routed to this role are excluded" count up front — both read the same
    // role-routed queue, so the start-up line, the drain-state file, and the
    // closing summary all agree on one snapshot. Best-effort: a resolution
    // failure just means the line/state don't render, not a drain abort.
    // trace:TASK-1297 | ai:claude
    let batch_members = resolve_batch_members(storage, user_id, batch_name, role).ok();
    let batch_member_count = batch_members.as_ref().map(Vec::len).unwrap_or(0);
    let excluded = count_batch_excluded_routed_specs(storage, user_id, batch_name, role).ok();
    let (excluded_count, excluded_role) = excluded.unwrap_or((0, None));

    if !json {
        eprintln!();
        eprintln!(
            "{} {} {}",
            "🚀".bold(),
            format!("auto-complete batch: batch:{batch_name}").bold(),
            format!("({})", variant.describe()).dimmed()
        );
        if let Some(limit) = max {
            eprintln!(
                "  {} drain capped at {} item{}",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                limit,
                if limit == 1 { "" } else { "s" }
            );
        }
        // TASK-1297: the filter is doing what it was asked — the SILENCE
        // about what it excluded was the defect. State both numbers, unless
        // there is nothing to declare (M == 0). trace:TASK-1297 | ai:claude
        if let Some(clause) = batch_exclusion_clause(excluded_count, excluded_role.as_deref()) {
            eprintln!(
                "  {} batch:{batch_name}: {batch_member_count} member{}; {clause}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                if batch_member_count == 1 { "" } else { "s" },
            );
        }
    }

    // STORY-301: write the drain-state file so `aida drain status` can show
    // the batch, its members, the current position, and what happens on exit.
    // Best-effort — a resolution / write failure leaves the drain running,
    // just unobservable. trace:STORY-301 | ai:claude
    let drain_root = find_main_worktree_root().ok();
    if let Some(root) = &drain_root {
        if let Some(members) = &batch_members {
            let specs: Vec<String> = members.iter().map(|m| m.1.clone()).collect();
            let pipeline_depth = DrainTuning::resolve(root).pipeline_depth();
            let _ = drain_state::DrainState::new_batch(batch_name, &specs)
                .with_pipeline_depth(pipeline_depth)
                // TASK-1297: carry the exclusion count so `aida drain status`
                // can show it while the drain is live. trace:TASK-1297
                .with_excluded_from_batch(excluded_count)
                .write(root);
        }
    }

    // TASK-967: drain origin (wall clock + base HEAD) for the exit summary.
    // trace:TASK-967 | ai:claude
    let drain_started = std::time::SystemTime::now();
    let drain_invocation = last_drain::DrainInvocation::capture();
    let drain_clock = std::time::Instant::now();
    let drain_base_sha = drain_root.as_deref().and_then(current_branch_head_sha);
    // TASK-966: arm the token meter only when a `--max-tokens` cap is set.
    let token_meter = caps
        .max_tokens
        .and(drain_root.clone())
        .map(|root| (root, drain_started));
    let pipeline_depth = drain_root
        .as_deref()
        .map(|root| DrainTuning::resolve(root).pipeline_depth())
        .unwrap_or_else(drain_state::default_pipeline_depth);
    let mut driver = RealBatchDriver {
        storage,
        user_id: user_id.to_string(),
        batch_name: batch_name.to_string(),
        role: role.map(|s| s.to_string()),
        variant,
        json,
        permission_mode: permission_mode.map(|s| s.to_string()),
        no_human,
        escalate_mode,
        steal,
        force_claim,
        allow_stale_base,
        no_auto_rebase,
        token_meter,
        pipeline_depth,
        pipelined_children: std::collections::HashMap::new(),
        pipelined_result_paths: std::collections::HashMap::new(),
        next_pipelined_handle: 1,
    };
    // EPIC-28: a `None` `--max-failures` from the CLI means "use the
    // built-in default cap" — set here so the orchestrator never runs
    // with an unbounded failure budget. trace:EPIC-28 | ai:claude
    let max_failures = max_failures.or(Some(DEFAULT_MAX_FAILURES));
    // TASK-966: thread the hard budget caps through the batch drain.
    let mut cap_stop = None;
    let result = auto_complete::drain_batch_pipelined_with_caps(
        &mut driver,
        max,
        max_failures,
        caps,
        drain_clock,
        &mut cap_stop,
    );
    // BUG-1422: re-resolve the members the drain refuses so an empty eligible
    // set is distinguishable from a genuinely completed batch.
    // trace:BUG-1422 | ai:codex
    let ineligible =
        resolve_batch_ineligible_members(storage, user_id, batch_name, role).unwrap_or_default();
    // An empty batch (nothing shipped, nothing punted, nothing to drain) is a
    // user error — the named batch tag matched no queued work. Surface it with
    // a non-zero exit so scripts notice, even though `drain_batch` calls it
    // `Drained`. A batch where every member punted is *not* empty (STORY-276).
    let exit_code = if let Some(stop) = &cap_stop {
        emit_drain_cap_stop(stop, json);
        DRAIN_CAP_EXIT_CODE
    } else if matches!(result.outcome, auto_complete::BatchDrainOutcome::Drained)
        && result.shipped.is_empty()
        && result.punted.is_empty()
        && result.escalated.is_empty()
    {
        1
    } else {
        result.exit_code
    };
    // TASK-1297: re-resolve the exclusion count for the closing summary — the
    // queue can have moved during the drain (more work approved, or this
    // drain's own shipped members left the queue), so the closing number
    // answers "how much is still sitting outside this batch right now" rather
    // than repeating the stale start-of-drain figure. Best-effort.
    // trace:TASK-1297 | ai:claude
    let (closing_excluded_count, closing_excluded_role) =
        count_batch_excluded_routed_specs(storage, user_id, batch_name, role)
            .ok()
            .unwrap_or((excluded_count, excluded_role));
    emit_batch_drain_summary(
        batch_name,
        &result,
        exit_code,
        json,
        closing_excluded_count,
        closing_excluded_role.as_deref(),
        &ineligible,
    );
    // TASK-967: permanent exit summary + cost-per-drain telemetry.
    finalize_drain_summary(
        "batch",
        format!("batch:{batch_name}"),
        &result.outcome,
        cap_stop.as_ref(),
        drain_summary::DrainTallies {
            shipped: result.shipped.len(),
            shelved: result.shelved.len(),
            skipped: result.skipped.len(),
            punted: result.punted.len(),
            escalated: result.escalated.len(),
        },
        drain_root.as_deref(),
        drain_base_sha.as_deref(),
        drain_started,
        drain_clock.elapsed(),
        &drain_invocation,
        json,
        // TASK-1297: the "M other approved specs routed to this role are not
        // in this batch" figure, echoed onto the terminal QueueDrained event
        // so a monitor can alarm on it. trace:TASK-1297 | ai:claude
        Some(closing_excluded_count),
        &ineligible,
    );
    // STORY-493: at drain-end, durably digest any mailbox traffic the drain
    // produced into the git-canonical orphan store. Best-effort + non-fatal —
    // never let a digest failure change the drain's exit code. trace:STORY-493
    if let Some(root) = &drain_root {
        maybe_digest_mailbox_best_effort(&root.join(".aida-store"), "drain-end");
    }
    // STORY-301: clean batch exit removes the drain-state file. A crash mid-
    // drain skips this, leaving the file for `aida drain status` to flag stale.
    if let Some(root) = &drain_root {
        let _ = drain_state::DrainState::clear(root);
    }
    std::process::exit(exit_code);
}

/// Real [`auto_complete::SingleBranchDriver`] (TASK-1003 / SPIKE-70) — drives a
/// coupled batch on ONE shared branch in ONE worktree. `next_head` re-resolves
/// the `batch:NAME` head against the live queue (a member marked Done leaves the
/// queue, so the head advances); `run_member_through_ci` drives that member's
/// Implementer + CI (`AutoCompleteVariant::ThroughCi`) committing on the shared
/// branch — NO per-member merge-to-main, NO reset between members; and
/// `run_cluster_finish` opens ONE PR from the shared branch linking every member
/// SPEC-ID (its merge auto-bumps every member Done→Completed because each member
/// committed with its own `(SPEC-ID)` trailer). A member failure HALTS the drain
/// (the pure engine stops), keeping prior members' commits on the branch.
///
/// The autonomous per-member SPAWN on the shared branch is the reliability-
/// critical keystone path; per the project's "ship autonomy-machinery changes
/// supervised" discipline it is driven through the explicit, operator-visible
/// plan below rather than blind unattended spawning. The engine's halt /
/// no-reset / one-cluster-PR sequencing (the genuinely-missing logic) is
/// exercised here and unit-tested in `auto_complete.rs`. trace:TASK-1003 SPIKE-70 | ai:claude
pub(crate) struct RealSingleBranchDriver<'a> {
    pub(crate) storage: &'a Storage,
    pub(crate) user_id: String,
    pub(crate) batch_name: String,
    pub(crate) role: Option<String>,
    pub(crate) json: bool,
    /// The shared feature branch every member commits onto.
    pub(crate) shared_branch: String,
    /// Members announced/driven so far, in order — drives the plan output and is
    /// the witness that the head advances exactly once per member.
    pub(crate) driven: Vec<String>,
    /// The repo the shared branch lives in — where the ONE cluster PR opens.
    /// `None` when the main worktree could not be resolved (the cluster step
    /// then prints its plan instead of opening anything).
    // trace:TASK-1136 | ai:claude
    pub(crate) project_root: Option<std::path::PathBuf>,
    /// The number of the ONE cluster PR once it opened. `None` when nothing was
    /// on the branch to open a PR for.
    // trace:TASK-1136 | ai:claude
    pub(crate) cluster_pr: Option<u64>,
    /// The drain's headless mode, if any — decides whether the per-member
    /// checkpoint pauses for the operator or auto-continues.
    // trace:TASK-1137 | ai:claude
    pub(crate) no_human: Option<auto_complete::NoHumanMode>,
    /// Whether this process can actually ask a question and get an answer (a
    /// terminal on BOTH stdin and stdout). Captured once at construction so the
    /// checkpoint never blocks on a prompt nobody can answer.
    // trace:TASK-1137 | ai:claude
    pub(crate) interactive: bool,
}

impl auto_complete::SingleBranchDriver for RealSingleBranchDriver<'_> {
    fn next_head(&mut self) -> Option<String> {
        match resolve_batch_members(
            self.storage,
            &self.user_id,
            &self.batch_name,
            self.role.as_deref(),
        ) {
            // Skip any member already driven this run — the plan-mode driver
            // does not mutate queue state, so re-resolving would otherwise loop
            // on the same head. The real autonomous spawn (the supervised
            // follow-on) advances the head by marking each member Done.
            Ok(members) => members
                .into_iter()
                .map(|m| m.1)
                .find(|id| !self.driven.iter().any(|d| d == id)),
            Err(e) => {
                eprintln!(
                    "{} could not resolve batch members: {}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    e
                );
                std::process::exit(1);
            }
        }
    }

    fn run_member_through_ci(&mut self, spec: &str) -> auto_complete::OrchestrationResult {
        let n = self.driven.len() + 1;
        if !self.json {
            eprintln!(
                "  {} member {}: implement + CI on {} (commit in place, no merge, no reset)",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
                n,
                self.shared_branch.cyan(),
            );
        }
        self.driven.push(spec.to_string());
        // The pure engine treats a clean result as "this member landed on the
        // shared branch". The autonomous Implementer+CI spawn on the shared
        // worktree is the supervised follow-on (plan §Followups). trace:TASK-1003
        auto_complete::OrchestrationResult::ok()
    }

    fn run_cluster_finish(&mut self, members: &[String]) -> auto_complete::OrchestrationResult {
        // ONE pull request for the whole cluster. Its title carries EVERY
        // member id in the trailing group the squash subject preserves, and its
        // body lists them under `## Covers` — so the single merge completes all
        // N through the ordinary Done→Completed scan rather than crediting only
        // whichever id reached the subject. trace:TASK-1136 | ai:claude
        let title = pr_ship::cluster_pr_title(&self.batch_name, members);
        let body = pr_ship::cluster_pr_body(&self.batch_name, &self.shared_branch, members);
        if !self.json {
            eprintln!(
                "  {} cluster: ONE PR from {} linking {} member{} → review + merge once \
                 (the one merge completes every linked member)",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
                self.shared_branch.cyan(),
                members.len(),
                if members.len() == 1 { "" } else { "s" },
            );
            for (i, id) in members.iter().enumerate() {
                eprintln!("     {:>2}. {}", i + 1, id.bold());
            }
        }
        // Nothing accumulated on the shared branch yet (the per-member
        // Implementer+CI spawn is the supervised follow-on) means there is
        // nothing to open a PR for — surface the exact cluster PR that WILL be
        // opened instead of failing the wrap-up on an empty branch.
        let ready = self
            .project_root
            .as_deref()
            .map(|root| shared_branch_has_cluster_commits(root, &self.shared_branch))
            .unwrap_or(false);
        if !ready {
            if !self.json {
                eprintln!(
                    "     {} {} {}",
                    "title:".dimmed(),
                    title,
                    "(opens once the members' commits are on the branch)".dimmed(),
                );
            }
            return auto_complete::OrchestrationResult::ok();
        }
        let root = self
            .project_root
            .clone()
            .expect("ready implies a resolved project root");
        match open_cluster_pr(&root, &self.shared_branch, &title, &body) {
            Ok(number) => {
                self.cluster_pr = Some(number);
                auto_complete::OrchestrationResult::ok()
            }
            Err(e) => {
                eprintln!(
                    "{} could not open the cluster pull request: {}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    e
                );
                auto_complete::OrchestrationResult::failed(auto_complete::Phase::Merge)
            }
        }
    }

    /// Per-member checkpoint. In a single-branch drain each member commits ON
    /// TOP of the last one, so a bad increment poisons everything after it —
    /// with a human at the keyboard we stop and let them inspect what just
    /// landed before the next member stacks on it; fully headless we continue,
    /// because a prompt with nobody to answer it would hang the drain forever.
    // trace:TASK-1137 | ai:claude
    fn checkpoint_between_members(&mut self, prev: &str, next: &str) -> bool {
        match auto_complete::single_branch_checkpoint_action(self.no_human, self.interactive) {
            auto_complete::CheckpointAction::AutoContinue => {
                if !self.json {
                    eprintln!(
                        "  {} checkpoint: {} landed on {} — continuing with {} (nobody at the \
                         keyboard to review it)",
                        crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                        prev.bold(),
                        self.shared_branch.cyan(),
                        next.bold(),
                    );
                }
                true
            }
            auto_complete::CheckpointAction::Prompt => {
                eprintln!();
                eprintln!(
                    "  {} checkpoint: {} landed on {} ({} member{} on the branch so far)",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    prev.bold(),
                    self.shared_branch.cyan(),
                    self.driven.len(),
                    if self.driven.len() == 1 { "" } else { "s" },
                );
                eprintln!(
                    "     {} inspect it now — {} commits on top of it and later members \
                     inherit anything wrong with it",
                    "→".dimmed(),
                    next.bold(),
                );
                // Declining is a CLEAN stop: the engine parks the drain with
                // every prior member's commit intact on the shared branch. A
                // read failure is treated the same way — never proceed to stack
                // more work on an increment nobody consented to.
                match prompt_yes_no(&format!("  Continue with {next}? [Y/n] "), true) {
                    Ok(go) => {
                        if !go {
                            eprintln!(
                                "  {} stopping here — the branch keeps every member committed \
                                 so far",
                                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                            );
                        }
                        go
                    }
                    Err(_) => false,
                }
            }
        }
    }
}

/// True when the shared single-branch drain branch exists locally AND carries
/// commits the default branch does not — i.e. there is a real cluster to open a
/// pull request for. A plan-mode run (no member actually spawned) leaves the
/// branch absent or empty, and opening a PR then would fail with "no commits
/// between".
// trace:TASK-1136 | ai:claude
pub(crate) fn shared_branch_has_cluster_commits(
    project_root: &std::path::Path,
    branch: &str,
) -> bool {
    let local_exists = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !local_exists {
        return false;
    }
    let base = crate::forge::default_branch_of(project_root);
    branch_ahead_of(project_root, branch, &format!("origin/{base}"))
        .or_else(|| branch_ahead_of(project_root, branch, &base))
        .is_some_and(|ahead| ahead > 0)
}

/// Push the shared branch and open the ONE cluster pull request through the
/// forge — the same `open_change` path a per-spec PR rides, differing only in
/// that the title/body name every member instead of one. Returns the change
/// number.
// trace:TASK-1136 | ai:claude
pub(crate) fn open_cluster_pr(
    project_root: &std::path::Path,
    branch: &str,
    title: &str,
    body: &str,
) -> Result<u64> {
    let push = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["push", "-u", "origin", branch])
        .status()
        .context("could not invoke `git push`")?;
    if !push.success() {
        anyhow::bail!("`git push -u origin {branch}` failed — investigate before retrying");
    }
    let change = crate::forge::forge_for_open_change(project_root)
        .open_change(crate::forge::OpenChange {
            branch: branch.to_string(),
            base: crate::forge::default_branch_of(project_root),
            title: title.to_string(),
            body: body.to_string(),
            draft: false,
        })
        .context("could not open the cluster pull request")?;
    if change.id == 0 {
        anyhow::bail!(
            "the cluster pull request opened but no number was found in its output: {}",
            change.url
        );
    }
    Ok(change.id)
}

/// Entry point for `aida queue work --batch NAME --auto-complete --single-branch`
/// (TASK-1003 / SPIKE-70). Drives a TIGHTLY-COUPLED batch on ONE shared branch
/// in ONE worktree: implement + CI each member committing in place (no
/// per-member merge, no reset), HALT on the first member failure (prior commits
/// kept), then open ONE cluster PR linking every member. Never returns: exits
/// `0` on a clean coupled plan/drain, else the failed-phase index. trace:TASK-1003
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_auto_complete_single_branch(
    storage: &Storage,
    user_id: &str,
    batch_name: &str,
    variant: auto_complete::AutoCompleteVariant,
    json: bool,
    role: Option<&str>,
    max: Option<usize>,
    // TASK-1137: decides the per-member checkpoint — a human at the keyboard
    // validates each increment before the next stacks on it; a fully headless
    // drain continues without asking. trace:TASK-1137 | ai:claude
    no_human: Option<auto_complete::NoHumanMode>,
) -> ! {
    // Resolve the coupled member set up front so the plan is concrete.
    let members = match resolve_batch_members(storage, user_id, batch_name, role) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "{} could not resolve batch members: {}",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                e
            );
            std::process::exit(1);
        }
    };
    if members.is_empty() {
        eprintln!(
            "{} no queued items tagged `batch:{}` — tag the coupled members via \
             `aida edit <id> --tags batch:{}` first",
            crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
            batch_name,
            batch_name,
        );
        std::process::exit(1);
    }

    // Derive the ONE shared branch every member accumulates onto.
    let drain_root = find_main_worktree_root().ok();
    let short_sha = drain_root
        .as_deref()
        .and_then(current_branch_head_sha)
        .map(|s| s.chars().take(7).collect::<String>())
        .unwrap_or_else(|| "base".to_string());
    let shared_branch = format!("single-branch/{batch_name}-{short_sha}");

    if !json {
        eprintln!();
        eprintln!(
            "{} {} {}",
            "🚀".bold(),
            format!("coupled single-branch drain: batch:{batch_name}").bold(),
            format!("({})", variant.describe()).dimmed(),
        );
        eprintln!(
            "  {} ONE shared branch {} in ONE worktree → ONE cluster PR; \
             commit-per-member, halt-on-failure (prior commits kept)",
            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
            shared_branch.cyan(),
        );
        eprintln!(
            "  {} coupled members, in order:",
            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
        );
        for (i, (_, display_id, title, status)) in members.iter().enumerate() {
            eprintln!(
                "     {:>2}. {} [{}] {}",
                i + 1,
                display_id.bold(),
                format!("{}", status).dimmed(),
                title,
            );
        }
    }

    let mut driver = RealSingleBranchDriver {
        storage,
        user_id: user_id.to_string(),
        batch_name: batch_name.to_string(),
        role: role.map(|s| s.to_string()),
        json,
        shared_branch: shared_branch.clone(),
        driven: Vec::new(),
        project_root: drain_root.clone(),
        cluster_pr: None,
        no_human,
        // A prompt needs a terminal on BOTH ends to be answerable — and `--json`
        // opts out too, since the prompt writes to stdout and would corrupt the
        // machine-readable stream its consumer is parsing.
        // trace:TASK-1137 | ai:claude
        interactive: !json && std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
    };
    let result = auto_complete::drain_batch_single_branch(&mut driver, max);
    let cluster_pr = driver.cluster_pr;

    if !json {
        eprintln!();
        match &result.outcome {
            auto_complete::SingleBranchOutcome::Clustered => {
                eprintln!(
                    "{} coupled plan ready: {} member{} accumulate on {} → ONE cluster PR",
                    crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                    result.cluster_members.len(),
                    if result.cluster_members.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    shared_branch.cyan(),
                );
                match cluster_pr {
                    // The one PR is open and links every member; its merge
                    // completes all of them in a single pass. trace:TASK-1136
                    Some(n) => eprintln!(
                        "  {} cluster pull request #{} links every member — review + merge it \
                         once and all {} complete together",
                        "→".cyan(),
                        n,
                        result.cluster_members.len(),
                    ),
                    // The autonomous Implementer+CI spawn on the shared worktree
                    // is the reliability-critical keystone path — drive it
                    // supervised, then the cluster PR opens over its commits.
                    None => eprintln!(
                        "  {} drive the coupled set on the shared branch supervised \
                         (`--zen`); the one cluster PR then opens over its commits",
                        "→".cyan(),
                    ),
                }
            }
            auto_complete::SingleBranchOutcome::Halted(_) => {
                eprintln!(
                    "{} halted on {} — {} prior member{} kept on the branch; \
                     the failed member is parked for triage (no cluster PR)",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    result.stopped_at.as_deref().unwrap_or("?"),
                    result.committed.len(),
                    if result.committed.len() == 1 { "" } else { "s" },
                );
            }
            // The per-member checkpoint stopped the drain — a clean exit, not a
            // failure. Prior members stay committed on the shared branch; the
            // same command resumes from the next member. trace:TASK-1137
            auto_complete::SingleBranchOutcome::Paused => {
                eprintln!(
                    "{} paused before {} — {} member{} committed on {} (no cluster pull \
                     request yet)",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan().bold(),
                    result.stopped_at.as_deref().unwrap_or("the next member"),
                    result.committed.len(),
                    if result.committed.len() == 1 { "" } else { "s" },
                    shared_branch.cyan(),
                );
                eprintln!(
                    "  {} re-run the same command to pick up from there once you are happy \
                     with what landed",
                    "→".cyan(),
                );
            }
            other => {
                eprintln!(
                    "  {} single-branch drain stopped: {:?}",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    other
                );
            }
        }
    }
    std::process::exit(result.exit_code);
}

/// Entry point for `aida queue work --batches A,B,C --auto-complete`
/// (TASK-310). Drains named batches left-to-right, exhausting one before
/// moving to the next. Empty batches are skipped; a failed phase or `--max`
/// cap stops the chain at the active batch.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_auto_complete_batches(
    storage: &Storage,
    user_id: &str,
    batch_names: &[String],
    variant: auto_complete::AutoCompleteVariant,
    json: bool,
    permission_mode: Option<&str>,
    role: Option<&str>,
    max: Option<usize>,
    // EPIC-28: per-batch `--max-failures` cap — see `handle_auto_complete_batch`.
    // trace:EPIC-28 | ai:claude
    max_failures: Option<usize>,
    no_human: Option<auto_complete::NoHumanMode>,
    escalate_mode: auto_complete::EscalateMode,
    steal: bool,
    force_claim: bool,
    // STORY-281: outer `--allow-stale-base`, threaded into every member's
    // RealBatchDriver via the per-batch closure below.
    allow_stale_base: bool,
    // STORY-429: outer `--no-auto-rebase`, threaded into every member.
    no_auto_rebase: bool,
    // TASK-966: hard budget caps for the whole chain. trace:TASK-966 | ai:claude
    caps: &drain_caps::DrainCaps,
) -> ! {
    if !json {
        eprintln!();
        eprintln!(
            "{} {} {}",
            "🚀".bold(),
            format!(
                "auto-complete batches: {}",
                batch_names
                    .iter()
                    .map(|b| format!("batch:{b}"))
                    .collect::<Vec<_>>()
                    .join(" → ")
            )
            .bold(),
            format!("({})", variant.describe()).dimmed()
        );
        if let Some(limit) = max {
            eprintln!(
                "  {} drain capped at {} item{} total",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                limit,
                if limit == 1 { "" } else { "s" }
            );
        }
    }

    let drain_root = find_main_worktree_root().ok();
    let mut batch_index = 0usize;
    // EPIC-28: same default as the single-batch path. trace:EPIC-28 | ai:claude
    let max_failures = max_failures.or(Some(DEFAULT_MAX_FAILURES));
    // TASK-966: a single drain-start + token meter shared across every batch in
    // the chain so `--max-runtime` / `--max-tokens` are cumulative.
    let chain_started = std::time::SystemTime::now();
    let drain_invocation = last_drain::DrainInvocation::capture();
    // TASK-967: drain-wide wall clock + base HEAD for the exit summary.
    // trace:TASK-967 | ai:claude
    let chain_clock = std::time::Instant::now();
    let drain_base_sha = drain_root.as_deref().and_then(current_branch_head_sha);
    let token_meter_root = caps.max_tokens.and(drain_root.clone());
    let mut cap_stop = None;
    let result = auto_complete::drain_batch_chain_with_caps(
        batch_names,
        max,
        max_failures,
        caps,
        chain_clock,
        &mut cap_stop,
        |batch_name| {
            batch_index += 1;
            let members_for_state = resolve_batch_members(storage, user_id, batch_name, role);
            if !json
                && members_for_state
                    .as_ref()
                    .map(|members| !members.is_empty())
                    .unwrap_or(true)
            {
                eprintln!(
                    "  {} drain: batch:{} ({}/{})",
                    "→".dimmed(),
                    batch_name.cyan(),
                    batch_index,
                    batch_names.len()
                );
            }
            if let Some(root) = &drain_root {
                if let Ok(members) = members_for_state {
                    let specs: Vec<String> = members.into_iter().map(|m| m.1).collect();
                    let pipeline_depth = DrainTuning::resolve(root).pipeline_depth();
                    // TASK-1297: per-sub-batch exclusion count, so `aida
                    // drain status` shows it live for whichever batch in the
                    // chain is currently running. trace:TASK-1297 | ai:claude
                    let excluded =
                        count_batch_excluded_routed_specs(storage, user_id, batch_name, role)
                            .map(|(count, _)| count)
                            .unwrap_or(0);
                    let _ = drain_state::DrainState::new_batch(batch_name, &specs)
                        .with_pipeline_depth(pipeline_depth)
                        .with_excluded_from_batch(excluded)
                        .write(root);
                }
            }
            Box::new(RealBatchDriver {
                storage,
                user_id: user_id.to_string(),
                batch_name: batch_name.to_string(),
                role: role.map(|s| s.to_string()),
                variant,
                json,
                permission_mode: permission_mode.map(|s| s.to_string()),
                no_human,
                escalate_mode,
                steal,
                force_claim,
                allow_stale_base,
                no_auto_rebase,
                // TASK-966: shared start + root → cumulative token meter.
                token_meter: token_meter_root.clone().map(|root| (root, chain_started)),
                pipeline_depth: drain_root
                    .as_deref()
                    .map(|root| DrainTuning::resolve(root).pipeline_depth())
                    .unwrap_or_else(drain_state::default_pipeline_depth),
                pipelined_children: std::collections::HashMap::new(),
                pipelined_result_paths: std::collections::HashMap::new(),
                next_pipelined_handle: 1,
            })
        },
    );

    let no_activity = result.shipped.is_empty()
        && result.punted.is_empty()
        && result.escalated.is_empty()
        && matches!(result.outcome, auto_complete::BatchDrainOutcome::Drained);
    let exit_code = if let Some(stop) = &cap_stop {
        emit_drain_cap_stop(stop, json);
        DRAIN_CAP_EXIT_CODE
    } else if no_activity {
        1
    } else {
        result.exit_code
    };
    emit_batch_chain_summary(batch_names, &result, exit_code, json);
    // TASK-967: permanent exit summary + cost-per-drain telemetry. The label is
    // the chained batches; the chain's wall clock + base HEAD bound the diff.
    finalize_drain_summary(
        "batch-chain",
        batch_names
            .iter()
            .map(|b| format!("batch:{b}"))
            .collect::<Vec<_>>()
            .join(" → "),
        &result.outcome,
        cap_stop.as_ref(),
        drain_summary::DrainTallies {
            shipped: result.shipped.len(),
            shelved: result.shelved.len(),
            skipped: result.skipped.len(),
            punted: result.punted.len(),
            escalated: result.escalated.len(),
        },
        drain_root.as_deref(),
        drain_base_sha.as_deref(),
        chain_started,
        chain_clock.elapsed(),
        &drain_invocation,
        json,
        // TASK-1297: the chained-batches path (`--batch a,b,c`) does not yet
        // surface the per-batch exclusion count — out of scope for this
        // fix, which targets the single-batch drain the 2026-09-19 incident
        // hit. trace:TASK-1297 | ai:claude
        // BUG-1425: the chain does not compute one aggregate exclusion count.
        // trace:BUG-1425 | ai:codex
        None,
        &[],
    );
    // STORY-493: same best-effort drain-end mailbox digest as the single-batch
    // path. Non-fatal — never affects the drain's exit code. trace:STORY-493
    if let Some(root) = &drain_root {
        maybe_digest_mailbox_best_effort(&root.join(".aida-store"), "drain-end");
    }
    if let Some(root) = &drain_root {
        let _ = drain_state::DrainState::clear(root);
    }
    std::process::exit(exit_code);
}

/// Print the closing summary for a multi-batch drain. Per-batch summaries are
/// intentionally compact: empty batches are omitted from human output so an
/// already-drained intermediate batch is a silent skip. trace:TASK-310
pub(crate) fn emit_batch_chain_summary(
    batch_names: &[String],
    result: &auto_complete::BatchChainDrainResult,
    exit_code: i32,
    json: bool,
) {
    use auto_complete::BatchDrainOutcome;

    if json {
        let outcome = match &result.outcome {
            BatchDrainOutcome::Drained => "drained",
            BatchDrainOutcome::MaxReached => "max-reached",
            BatchDrainOutcome::Failed(_) => "failed",
            BatchDrainOutcome::Stalled => "stalled",
            BatchDrainOutcome::Mismatched { .. } => "mismatched",
            BatchDrainOutcome::Inconclusive => "inconclusive",
            BatchDrainOutcome::Held => "held",
            // EPIC-28 trace:EPIC-28 | ai:claude
            BatchDrainOutcome::DrainedWithShelved => "drained-with-shelved",
        };
        let mut obj = serde_json::Map::new();
        obj.insert(
            "event".to_string(),
            serde_json::Value::String("batch-chain-drain".into()),
        );
        obj.insert(
            "batches".to_string(),
            serde_json::Value::Array(
                batch_names
                    .iter()
                    .map(|b| serde_json::Value::String(format!("batch:{b}")))
                    .collect(),
            ),
        );
        obj.insert(
            "outcome".to_string(),
            serde_json::Value::String(outcome.into()),
        );
        obj.insert(
            "shipped".to_string(),
            serde_json::Value::Array(
                result
                    .shipped
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "shipped_count".to_string(),
            serde_json::Value::Number((result.shipped.len() as u64).into()),
        );
        obj.insert(
            "punted".to_string(),
            serde_json::Value::Array(
                result
                    .punted
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "punted_count".to_string(),
            serde_json::Value::Number((result.punted.len() as u64).into()),
        );
        obj.insert(
            "escalated".to_string(),
            serde_json::Value::Array(
                result
                    .escalated
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "escalated_count".to_string(),
            serde_json::Value::Number((result.escalated.len() as u64).into()),
        );
        // EPIC-28: chain-wide shelved + skipped totals. trace:EPIC-28
        obj.insert(
            "shelved".to_string(),
            serde_json::Value::Array(
                result
                    .shelved
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "shelved_count".to_string(),
            serde_json::Value::Number((result.shelved.len() as u64).into()),
        );
        obj.insert(
            "skipped_count".to_string(),
            serde_json::Value::Number((result.skipped.len() as u64).into()),
        );
        obj.insert(
            "stopped_batch".to_string(),
            match &result.stopped_batch {
                Some(b) => serde_json::Value::String(format!("batch:{b}")),
                None => serde_json::Value::Null,
            },
        );
        obj.insert(
            "stopped_at".to_string(),
            match &result.stopped_at {
                Some(s) => serde_json::Value::String(s.clone()),
                None => serde_json::Value::Null,
            },
        );
        obj.insert(
            "exit_code".to_string(),
            serde_json::Value::Number(exit_code.into()),
        );
        println!("{}", serde_json::Value::Object(obj));
        return;
    }

    let shipped_list = if result.shipped.is_empty() {
        "(none)".to_string()
    } else {
        result.shipped.join(", ")
    };
    let n = result.shipped.len();
    let plural = if n == 1 { "" } else { "s" };
    eprintln!();

    if result.shipped.is_empty() && result.punted.is_empty() && result.escalated.is_empty() {
        eprintln!(
            "{} no queued items found in any requested batch: {}",
            crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
            batch_names
                .iter()
                .map(|b| format!("batch:{b}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        return;
    }

    match &result.outcome {
        BatchDrainOutcome::Drained => {
            eprintln!(
                "{} batch chain drained — {n} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
        }
        BatchDrainOutcome::MaxReached => {
            eprintln!(
                "{} batch chain — `--max` reached, {n} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
            if let Some(batch) = &result.stopped_batch {
                eprintln!(
                    "  {} stopped while draining `batch:{batch}` — re-run the same command to continue",
                    "→".dimmed()
                );
            }
        }
        BatchDrainOutcome::Failed(phase) => {
            let batch = result.stopped_batch.as_deref().unwrap_or("<batch>");
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} batch chain stopped in `batch:{batch}` at {} (phase {} failed)",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                stopped.bold(),
                phase.index()
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} later batches were not started — fix {stopped}, then re-run the chain",
                "→".dimmed()
            );
        }
        BatchDrainOutcome::Stalled => {
            let batch = result.stopped_batch.as_deref().unwrap_or("<batch>");
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} batch chain stopped in `batch:{batch}` — {} stayed at the head after a successful run",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                stopped.bold()
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
        }
        BatchDrainOutcome::Mismatched {
            dispatched,
            shipped,
        } => {
            let batch = result.stopped_batch.as_deref().unwrap_or("<batch>");
            eprintln!(
                "{} batch chain stopped in `batch:{batch}` — dispatched {} but PR credited {}",
                crate::glyph(crate::glyphs::Glyph::Info).cyan().bold(),
                dispatched.bold(),
                shipped.bold()
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
        }
        BatchDrainOutcome::Inconclusive => {
            let batch = result.stopped_batch.as_deref().unwrap_or("<batch>");
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} batch chain paused in `batch:{batch}` at {} — phase 1 inconclusive",
                "⏸".yellow().bold(),
                stopped.bold()
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
        }
        // BUG-250: a member deliberately held its PR — the chain pauses there
        // until the operator runs the gate and opens the PR. trace:BUG-250
        BatchDrainOutcome::Held => {
            let batch = result.stopped_batch.as_deref().unwrap_or("<batch>");
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} batch chain paused in `batch:{batch}` at {} — PR deliberately held",
                "⏸".yellow().bold(),
                stopped.bold()
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
        }
        // EPIC-28: every batch drained, but at least one member shelved
        // or was skipped. trace:EPIC-28 | ai:claude
        BatchDrainOutcome::DrainedWithShelved => {
            eprintln!(
                "{} batch chain drained — {n} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
        }
    }

    if !result.shelved.is_empty() {
        let sn = result.shelved.len();
        eprintln!(
            "  {} {sn} spec{} shelved on phase failure: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if sn == 1 { "" } else { "s" },
            result.shelved.join(", ")
        );
    }
    if !result.skipped.is_empty() {
        let kn = result.skipped.len();
        let render: Vec<String> = result
            .skipped
            .iter()
            .map(|(spec, reason)| format!("{spec} ({reason})"))
            .collect();
        eprintln!(
            "  {} {kn} dependent spec{} skipped: {}",
            "⤳".yellow(),
            if kn == 1 { "" } else { "s" },
            render.join(", ")
        );
    }
    if !result.punted.is_empty() {
        let pn = result.punted.len();
        eprintln!(
            "  {} {pn} spec{} punted to Needs Attention: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if pn == 1 { "" } else { "s" },
            result.punted.join(", ")
        );
    }
    if !result.escalated.is_empty() {
        let en = result.escalated.len();
        eprintln!(
            "  {} {en} spec{} escalated to a human: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if en == 1 { "" } else { "s" },
            result.escalated.join(", ")
        );
    }
}

/// Print the closing summary of a `--batch --auto-complete` drain: what
/// shipped, where it stopped, and what is left queued. The per-spec failure
/// epilogue + recovery hint are already printed by `orchestrate`; this adds
/// the batch-level framing (which members shipped, queue-intact-for-retry).
/// `exit_code` is the process's effective exit code (see caller).
/// `excluded_from_batch` is the "M other approved specs routed to this role
/// are not in this batch" count, re-resolved at drain-end so the closing
/// summary reflects the post-drain queue.
// trace:TASK-285 trace:TASK-1297 | ai:claude
pub(crate) fn emit_batch_drain_summary(
    batch_name: &str,
    result: &auto_complete::BatchDrainResult,
    exit_code: i32,
    json: bool,
    excluded_from_batch: usize,
    excluded_from_batch_role: Option<&str>,
    ineligible: &[events::IneligibleBatchMember],
) {
    use auto_complete::BatchDrainOutcome;

    if json {
        let outcome = match &result.outcome {
            BatchDrainOutcome::Drained => "drained",
            BatchDrainOutcome::MaxReached => "max-reached",
            BatchDrainOutcome::Failed(_) => "failed",
            BatchDrainOutcome::Stalled => "stalled",
            BatchDrainOutcome::Mismatched { .. } => "mismatched",
            BatchDrainOutcome::Inconclusive => "inconclusive",
            BatchDrainOutcome::Held => "held",
            // EPIC-28: a clean drain that shelved at least one member —
            // the operator still has triage to do. trace:EPIC-28
            BatchDrainOutcome::DrainedWithShelved => "drained-with-shelved",
        };
        let mut obj = serde_json::Map::new();
        obj.insert(
            "event".to_string(),
            serde_json::Value::String("batch-drain".into()),
        );
        obj.insert(
            "batch".to_string(),
            serde_json::Value::String(format!("batch:{batch_name}")),
        );
        obj.insert(
            "outcome".to_string(),
            serde_json::Value::String(outcome.into()),
        );
        obj.insert(
            "shipped".to_string(),
            serde_json::Value::Array(
                result
                    .shipped
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "shipped_count".to_string(),
            serde_json::Value::Number((result.shipped.len() as u64).into()),
        );
        // STORY-276: punted members — parked in Needs Attention, not shipped.
        obj.insert(
            "punted".to_string(),
            serde_json::Value::Array(
                result
                    .punted
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "punted_count".to_string(),
            serde_json::Value::Number((result.punted.len() as u64).into()),
        );
        // STORY-306: escalated members — left for a human, not shipped.
        obj.insert(
            "escalated".to_string(),
            serde_json::Value::Array(
                result
                    .escalated
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "escalated_count".to_string(),
            serde_json::Value::Number((result.escalated.len() as u64).into()),
        );
        obj.insert(
            "stopped_at".to_string(),
            match &result.stopped_at {
                Some(s) => serde_json::Value::String(s.clone()),
                None => serde_json::Value::Null,
            },
        );
        obj.insert(
            "exit_code".to_string(),
            serde_json::Value::Number(exit_code.into()),
        );
        // TASK-1297: the excluded-by-filter figure, so a machine consumer
        // can alarm without scraping the human line. trace:TASK-1297 | ai:claude
        obj.insert(
            "excluded_from_batch".to_string(),
            serde_json::Value::Number((excluded_from_batch as u64).into()),
        );
        obj.insert(
            "ineligible".to_string(),
            serde_json::to_value(ineligible).unwrap_or_else(|_| serde_json::Value::Array(vec![])),
        );
        println!("{}", serde_json::Value::Object(obj));
        return;
    }

    let shipped_list = if result.shipped.is_empty() {
        "(none)".to_string()
    } else {
        result.shipped.join(", ")
    };
    let n = result.shipped.len();
    let plural = if n == 1 { "" } else { "s" };
    eprintln!();
    match &result.outcome {
        BatchDrainOutcome::Drained
            if result.shipped.is_empty()
                && result.punted.is_empty()
                && result.escalated.is_empty()
                && ineligible.is_empty() =>
        {
            eprintln!(
                "{} no queued items tagged `batch:{batch_name}` — tag members \
                 via `aida edit <id> --tags batch:{batch_name}` first",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold()
            );
        }
        BatchDrainOutcome::Drained
            if result.shipped.is_empty()
                && result.punted.is_empty()
                && result.escalated.is_empty() =>
        {
            let rendered = ineligible
                .iter()
                .map(|member| format!("{} ({})", member.spec, member.reason))
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!(
                "{} batch `batch:{batch_name}` cannot progress — {} member{} remain but none are eligible: {}",
                "⏸".yellow().bold(),
                ineligible.len(),
                if ineligible.len() == 1 { "" } else { "s" },
                rendered,
            );
        }
        BatchDrainOutcome::Drained => {
            eprintln!(
                "{} batch `batch:{batch_name}` drained — {n} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
        }
        BatchDrainOutcome::MaxReached => {
            eprintln!(
                "{} batch `batch:{batch_name}` — `--max` reached, {n} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
            eprintln!(
                "  {} more members remain queued — re-run to continue: \
                 `aida queue work --batch {batch_name} --auto-complete`",
                "→".dimmed()
            );
        }
        BatchDrainOutcome::Failed(phase) => {
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} batch `batch:{batch_name}` drain stopped at {} (phase {} failed)",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                stopped.bold(),
                phase.index()
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} the rest of the batch is untouched — fix {stopped} (hint above), \
                 then re-run `aida queue work --batch {batch_name} --auto-complete`",
                "→".dimmed()
            );
        }
        BatchDrainOutcome::Stalled => {
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} batch `batch:{batch_name}` drain stopped — {} stayed at the head \
                 after a successful run (queue did not advance)",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                stopped.bold()
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} check {stopped}'s status (`aida show {stopped}`) — it may not have \
                 been dequeued",
                "→".dimmed()
            );
        }
        // BUG-245: phase 1 shipped a different spec than the dispatched head.
        // Credit the truth, name what stayed queued, and tell the operator
        // how to advance — either pick the dispatched spec back up or skip
        // it. trace:BUG-245 | ai:claude
        BatchDrainOutcome::Mismatched {
            dispatched,
            shipped,
        } => {
            eprintln!(
                "{} batch `batch:{batch_name}` drain stopped — phase 1 was dispatched \
                 for {} but the PR credited {}",
                crate::glyph(crate::glyphs::Glyph::Info).cyan().bold(),
                dispatched.bold(),
                shipped.bold(),
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} {} is still queued — pick it back up with `aida queue work {}`, \
                 or remove it with `aida queue remove {}` if {} subsumed it",
                "→".dimmed(),
                dispatched,
                dispatched,
                dispatched,
                shipped,
            );
        }
        // BUG-257: a transient GH-API outage during phase-1 PR lookup paused
        // the drain. The spec is left in its current state — neither shipped
        // nor failed — so a retry once the API is reachable proceeds without
        // any cleanup. trace:BUG-257 | ai:claude
        BatchDrainOutcome::Inconclusive => {
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} batch `batch:{batch_name}` drain paused at {} — phase 1 \
                 inconclusive (GH API unreachable)",
                "⏸".yellow().bold(),
                stopped.bold(),
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} transient — retry once the API is reachable: \
                 `gh api /rate_limit` then re-run \
                 `aida queue work --batch {batch_name} --auto-complete`",
                "→".dimmed()
            );
        }
        // BUG-250: a member deliberately held its PR — the batch pauses there
        // for the operator's manual gate. trace:BUG-250 | ai:claude
        BatchDrainOutcome::Held => {
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} batch `batch:{batch_name}` drain paused at {} — PR deliberately held",
                "⏸".yellow().bold(),
                stopped.bold(),
            );
            eprintln!(
                "  {} already shipped ({n}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} run your gate, open the PR (`gh pr create`), then re-run \
                 `aida queue work --batch {batch_name} --auto-complete`",
                "→".dimmed()
            );
        }
        // EPIC-28: the batch drained but parked at least one spec. The
        // top line stays green-ish — independents shipped — and the
        // shelved/skipped summary below points the operator at the
        // triage surface. trace:EPIC-28 | ai:claude
        BatchDrainOutcome::DrainedWithShelved => {
            eprintln!(
                "{} batch `batch:{batch_name}` drained with shelved members — \
                 {n} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
        }
    }
    // STORY-276: name the members a headless implementer punted — the drain
    // advanced past them, but they parked in Needs Attention rather than
    // shipping, so they need advisor triage.
    if !result.punted.is_empty() {
        let pn = result.punted.len();
        eprintln!(
            "  {} {pn} spec{} punted to Needs Attention: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if pn == 1 { "" } else { "s" },
            result.punted.join(", ")
        );
    }
    // STORY-306: name the members the drain escalated to a human — the
    // reviewer would not auto-merge, or the advisor would not resolve a
    // design-fork. The drain advanced past them; a human decides.
    if !result.escalated.is_empty() {
        let en = result.escalated.len();
        eprintln!(
            "  {} {en} spec{} escalated to a human: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if en == 1 { "" } else { "s" },
            result.escalated.join(", ")
        );
    }
    // EPIC-28: shelved + skipped members. Shelved are the failures the
    // drain tolerated; skipped are the dependents we never even tried
    // because their blocker had been shelved. trace:EPIC-28 | ai:claude
    if !result.shelved.is_empty() {
        let sn = result.shelved.len();
        eprintln!(
            "  {} {sn} spec{} shelved on phase failure: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if sn == 1 { "" } else { "s" },
            result.shelved.join(", ")
        );
    }
    if !result.skipped.is_empty() {
        let kn = result.skipped.len();
        let render: Vec<String> = result
            .skipped
            .iter()
            .map(|(spec, reason)| format!("{spec} ({reason})"))
            .collect();
        eprintln!(
            "  {} {kn} dependent spec{} skipped: {}",
            "⤳".yellow(),
            if kn == 1 { "" } else { "s" },
            render.join(", ")
        );
    }
    // TASK-1297: closing-summary echo of the "M other approved specs routed
    // to this role are not in this batch" figure — the same clause printed
    // at start-up, re-resolved post-drain. Silent when M == 0.
    // trace:TASK-1297 | ai:claude
    if let Some(clause) = batch_exclusion_clause(excluded_from_batch, excluded_from_batch_role) {
        eprintln!(
            "  {} {clause}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
    }
}

/// TASK-293 — a parsed `next` / `nextN` keyword positional for `aida queue
/// work`. trace:TASK-293 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NextKeyword {
    /// The positional is not a `next*` keyword — a spec-id, cluster anchor,
    /// or (already rerouted) batch positional.
    NotNext,
    /// `next`, `nextN`, or `next N` — drain `N` specs from the queue head.
    /// `N >= 1`; `N == 1` is the plain head pickup.
    Count(usize),
}

/// Parse the `aida queue work` positional(s) for the `next` / `nextN` keyword
/// (TASK-293). `id` is the first positional, `count` the optional second (the
/// spaced `next 3` form). `next` and `nextN` / `next N` resolve to a drain
/// count; anything else is `NotNext`. Malformed forms (`next0`, `nextfoo`, a
/// count with no `next`, a count alongside the compact `nextN`) are rejected.
/// The keyword matches case-insensitively — a real spec-id always carries a
/// `TYPE-N` hyphen, so `next` / `nextN` never collide with one.
/// trace:TASK-293 | ai:claude
pub(crate) fn parse_next_keyword(id: Option<&str>, count: Option<&str>) -> Result<NextKeyword> {
    let Some(raw) = id else {
        // No positional id. A trailing count with no `next` before it is not
        // reachable through clap's positional fill, but guard rather than
        // silently ignore it.
        if count.is_some() {
            anyhow::bail!(
                "a count positional needs the `next` keyword before it (e.g. `aida queue work next 3`)"
            );
        }
        return Ok(NextKeyword::NotNext);
    };
    let lower = raw.to_ascii_lowercase();
    if !lower.starts_with("next") {
        if count.is_some() {
            anyhow::bail!(
                "`{}` is not the `next` keyword — a trailing count only follows `next`",
                raw
            );
        }
        return Ok(NextKeyword::NotNext);
    }
    let suffix = &lower["next".len()..];
    // `next` exact: the count comes from the optional second positional.
    if suffix.is_empty() {
        let n = match count {
            None => 1,
            Some(c) => parse_next_count(c)?,
        };
        return Ok(NextKeyword::Count(n));
    }
    // `nextN` compact: the suffix must be all digits, and there must be no
    // separate count positional (`next3 5` is contradictory).
    if suffix.chars().all(|c| c.is_ascii_digit()) {
        if let Some(c) = count {
            anyhow::bail!(
                "`{}` already carries its count — drop the extra `{}`",
                raw,
                c
            );
        }
        return Ok(NextKeyword::Count(parse_next_count(suffix)?));
    }
    // `nextfoo` — starts with `next` but is not a valid form.
    anyhow::bail!(
        "`{}` is not a valid pickup target — use `next`, `nextN` (e.g. `next3`), or a SPEC id",
        raw
    )
}

/// Parse + validate the `N` of a `nextN` form: a positive whole number.
/// trace:TASK-293 | ai:claude
pub(crate) fn parse_next_count(raw: &str) -> Result<usize> {
    let n: usize = raw.trim().parse().map_err(|_| {
        anyhow::anyhow!(
            "`{}` is not a whole number — `nextN` needs a count like `next3`",
            raw
        )
    })?;
    if n == 0 {
        anyhow::bail!("`next0` drains nothing — use `next` (or `next1`) for a single pickup");
    }
    Ok(n)
}

/// Non-erroring queue-head resolver for the `nextN` drain (TASK-293): the
/// drivable head of the active role's queue, or `None` when nothing drivable
/// remains (the drain is then complete). A store/queue read failure is fatal —
/// it is not "drained" and would recur on every iteration. trace:TASK-293
/// Resolve the next `nextN` / queue-wide drain head. Returns the pick plus
/// the entries skipped to reach it: `role_skipped` as `(id, routed role)` and
/// (BUG-1608) `blocked_skipped` as `(id, pickability reason)` — dependents
/// whose `BlockedBy` prerequisite is not Completed. The candidates are
/// re-read from storage on every call, so a blocker shelved by the previous
/// member is seen by the very next pick.
#[allow(clippy::type_complexity)]
pub(crate) fn resolve_next_n_head(
    storage: &Storage,
    user_id: &str,
    role_override: Option<&str>,
) -> (
    Option<AutoCompleteHeadPick>,
    Vec<(String, String)>,
    Vec<(String, String)>,
) {
    let effective_role = effective_auto_complete_role(role_override);
    match auto_complete_head_candidates_with_roles(storage, user_id, Some(&effective_role)) {
        Ok(candidates) => {
            let pick = pick_auto_complete_head_for_role(&candidates, &effective_role).unwrap();
            let (role_skipped, blocked_skipped) = match &pick {
                Some(pick) => (pick.role_skipped.clone(), pick.blocked_skipped.clone()),
                None => (
                    candidates
                        .iter()
                        .filter_map(|candidate| {
                            let routed = candidate.for_role.as_deref().map(canonical_role_name)?;
                            (routed != effective_role).then(|| (candidate.id.clone(), routed))
                        })
                        .collect(),
                    // trace:BUG-1608 | ai:claude
                    candidates
                        .iter()
                        .filter(|candidate| {
                            candidate
                                .for_role
                                .as_deref()
                                .map(|r| canonical_role_name(r) == effective_role)
                                .unwrap_or(true)
                                && auto_complete_head_drivable(&candidate.status)
                        })
                        .filter_map(|candidate| {
                            candidate
                                .blocked
                                .clone()
                                .map(|reason| (candidate.id.clone(), reason))
                        })
                        .collect(),
                ),
            };
            (pick, role_skipped, blocked_skipped)
        }
        Err(e) => {
            eprintln!(
                "{} could not resolve the queue head: {}",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                e
            );
            std::process::exit(1);
        }
    }
}

/// Count the orchestrator-drivable items queued for the active role — used to
/// surface the "only K items queued" note when a `nextN` asks for more than
/// the queue holds. trace:TASK-293 | ai:claude
pub(crate) fn drivable_queued_count(
    storage: &Storage,
    user_id: &str,
    role_override: Option<&str>,
) -> Result<usize> {
    let effective_role = effective_auto_complete_role(role_override);
    Ok(
        auto_complete_head_candidates_with_roles(storage, user_id, Some(&effective_role))?
            .iter()
            .filter(|candidate| {
                candidate
                    .for_role
                    .as_deref()
                    .map(|r| canonical_role_name(r) == effective_role)
                    .unwrap_or(true)
                    && auto_complete_head_drivable(&candidate.status)
            })
            .count(),
    )
}

/// TASK-966: process-exit code a drain uses when a hard budget cap
/// (`--max-tokens` / `--max-iterations` / `--max-runtime`) stopped it. Distinct
/// from the orchestrator's phase-failure codes (1-6) and the shelved-drain code
/// (2) so an outer loop like `scripts/drain-loop.sh` can recognise "budget
/// exhausted — stop the loop" vs "chunk drained, keep going".
// trace:TASK-966 | ai:claude
pub(crate) const DRAIN_CAP_EXIT_CODE: i32 = 7;

/// TASK-966: sum the cumulative reported tokens across the headless-phase logs a
/// drain has produced since `since`. Each `claude -p --output-format
/// stream-json` phase writes a `*.jsonl` log under `.aida/headless-logs/`; the
/// terminal `result` event carries that phase's cumulative usage. We sum
/// per-log totals over the logs touched at/after the drain start so the running
/// figure reflects only THIS drain's spend. Best-effort: an unreadable dir /
/// file contributes nothing rather than erroring.
// trace:TASK-966 | ai:claude
pub(crate) fn sum_headless_log_tokens(
    project_root: &std::path::Path,
    since: std::time::SystemTime,
) -> u64 {
    let dir = project_root.join(".aida").join("headless-logs");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let mut total: u64 = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        // Only count logs written during this drain.
        let touched_in_window = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|m| m >= since)
            .unwrap_or(true);
        if !touched_in_window {
            continue;
        }
        if let Ok(contents) = std::fs::read_to_string(&path) {
            total = total.saturating_add(drain_caps::tokens_from_log(&contents));
        }
    }
    total
}

/// Strict drain-exit accounting. Live caps keep using the best-effort scanner
/// above; persisted cost evidence is exact or explicitly unknown.
// trace:BUG-1418 | ai:codex
pub(crate) fn measure_completed_headless_logs(
    project_root: &std::path::Path,
    since: std::time::SystemTime,
) -> Option<u64> {
    let dir = project_root.join(".aida").join("headless-logs");
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut total = 0_u64;
    let mut found = false;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let touched_in_window = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|m| m >= since)
            .unwrap_or(false);
        if !touched_in_window {
            continue;
        }
        found = true;
        let contents = std::fs::read_to_string(&path).ok()?;
        match drain_caps::completed_log_tokens(&contents) {
            drain_caps::CompletedLogTokens::Measured(tokens) => {
                total = total.saturating_add(tokens);
            }
            drain_caps::CompletedLogTokens::Unrecognized { shape } => {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("<non-utf8>");
                eprintln!("{}", unrecognized_usage_diagnostic(name, &shape));
                return None;
            }
            drain_caps::CompletedLogTokens::Truncated => return None,
        }
    }
    found.then_some(total)
}

// trace:BUG-1418 | ai:codex
pub(crate) fn unrecognized_usage_diagnostic(file_name: &str, shape: &str) -> String {
    fn safe_label(value: &str, limit: usize) -> String {
        value
            .chars()
            .take(limit)
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '=' | ',') {
                    ch
                } else {
                    '_'
                }
            })
            .collect()
    }
    format!(
        "token usage unrecognized in headless log {} (shape: {})",
        safe_label(file_name, 80),
        safe_label(shape, 96)
    )
}

/// TASK-967: the `BatchDrainOutcome` → machine label used in the drain exit
/// summary's `outcome` field (mirrors the per-emitter JSON mapping).
// trace:TASK-967 | ai:claude
pub(crate) fn batch_outcome_label(outcome: &auto_complete::BatchDrainOutcome) -> &'static str {
    use auto_complete::BatchDrainOutcome;
    match outcome {
        BatchDrainOutcome::Drained => "drained",
        BatchDrainOutcome::MaxReached => "max-reached",
        BatchDrainOutcome::Failed(_) => "failed",
        BatchDrainOutcome::Stalled => "stalled",
        BatchDrainOutcome::Mismatched { .. } => "mismatched",
        BatchDrainOutcome::Inconclusive => "inconclusive",
        BatchDrainOutcome::Held => "held",
        BatchDrainOutcome::DrainedWithShelved => "drained-with-shelved",
    }
}

/// TASK-967: `git diff --numstat <base>..HEAD` in the drain's main worktree —
/// the cumulative line/file churn the drain landed (each shipped spec merged +
/// pulled advances HEAD on the integration branch). Best-effort: any git error
/// yields an empty body, which `DrainDiffStats::from_numstat` reads as all-zero.
// trace:TASK-967 | ai:claude
pub(crate) fn drain_diff_numstat(root: &std::path::Path, base_sha: &str) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--numstat", &format!("{base_sha}..HEAD")])
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => String::new(),
    }
}

/// TASK-967: assemble + emit + record the permanent drain EXIT SUMMARY.
///
/// Gathers the impure inputs — cumulative headless-log tokens (reusing
/// [`sum_headless_log_tokens`] / [`drain_caps::tokens_from_log`]), the
/// drain-wide `git diff --numstat`, and the wall time — then hands them to the
/// pure [`drain_summary::DrainSummary`]. The block prints to stderr (human) or
/// emits the structured record as a JSON line (machine), and the same record is
/// appended to `~/.aida/usage.jsonl` (gated on telemetry) so `aida usage` can
/// chart cost-per-drain. Works for every exit family — drained, cap-hit (the
/// `cap_stop` label overrides the outcome), shelved, or failed.
///
/// TASK-1165: the project root is the caller-supplied `drain_root` — every
/// caller already resolves it as `find_main_worktree_root().ok()` for the token
/// / diff inputs, so the terminal `QueueDrained` emit (and the `.aida/last-drain.json`
/// write beside it) reuses that one injected value rather than re-deriving a
/// second root from the process cwd at the emit site. `None` skips those
/// side-effects entirely, which is the fail-safe BUG-770 established.
// trace:TASK-967 trace:TASK-1165 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn finalize_drain_summary(
    kind: &str,
    label: String,
    outcome_outcome: &auto_complete::BatchDrainOutcome,
    cap_stop: Option<&drain_caps::CapStop>,
    tallies: drain_summary::DrainTallies,
    drain_root: Option<&std::path::Path>,
    drain_base_sha: Option<&str>,
    started: std::time::SystemTime,
    elapsed: std::time::Duration,
    drain_invocation: &last_drain::DrainInvocation,
    json: bool,
    // TASK-1297: "M other approved specs routed to this role are not in this
    // batch" — 0 for every non-batch drain kind (single, next-n). Echoed onto
    // the terminal QueueDrained event so a monitor can alarm on it without
    // scraping the human closing line. trace:TASK-1297 | ai:claude
    excluded_from_batch: Option<usize>,
    ineligible: &[events::IneligibleBatchMember],
) {
    // A budget-cap stop reports the cap that fired; otherwise the drain outcome.
    let outcome = match cap_stop {
        Some(stop) => stop.flag().to_string(),
        None => batch_outcome_label(outcome_outcome).to_string(),
    };
    let cumulative_tokens =
        drain_root.and_then(|root| measure_completed_headless_logs(root, started));
    let diff = match (drain_root, drain_base_sha) {
        (Some(root), Some(base)) => {
            drain_summary::DrainDiffStats::from_numstat(&drain_diff_numstat(root, base))
        }
        _ => drain_summary::DrainDiffStats::default(),
    };
    // TASK-997: tally what the cheap event classifier absorbed vs surfaced over
    // this drain's window, so the exit summary PROVES the event-driven
    // supervision lever with real numbers instead of asserting it. Read off the
    // same `.aida/events.jsonl` the drain emitted into, classified with the same
    // `is_actionable` predicate `aida watch` wakes on. Best-effort — an
    // unreadable stream yields an all-zero tally that renders as "none
    // recorded", never a failed exit. trace:TASK-997 | ai:claude
    let events_tally = drain_root
        .map(|root| events::tally_window(root, started))
        .unwrap_or_default();
    // TASK-1334: PRs this drain opened and did not merge in its own window —
    // shelved members' open PRs. Without them the summary reads "0 shipped ·
    // diff +0 -0" for a wave that produced real PRs. Derived from the same
    // event window the tally above reads. trace:TASK-1334 | ai:claude
    let open_prs = drain_root
        .map(|root| events::open_prs_window(root, started))
        .unwrap_or_default();
    let summary = drain_summary::DrainSummary {
        kind: kind.to_string(),
        label,
        outcome,
        tallies,
        cumulative_tokens,
        diff,
        open_prs,
        elapsed_secs: elapsed.as_secs(),
        events: events_tally,
    };
    let ts = chrono::Utc::now().to_rfc3339();
    let sha = build_sha_short();
    let role = std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|s| !s.is_empty());
    let vendor = drain_root
        .map(session::resolve_headless_vendor)
        .unwrap_or(session::HeadlessVendor::Claude)
        .as_str();
    let run_key = drain_root
        .map(|root| drain_state::current_context(root).1)
        .filter(|key| !key.is_empty())
        .unwrap_or_else(|| {
            format!(
                "invocation:{}:{}",
                drain_invocation.pid,
                drain_invocation
                    .process_started_at
                    .as_deref()
                    .unwrap_or("unknown")
            )
        });
    let record = summary.to_usage_value(&ts, sha.as_deref(), role.as_deref(), vendor, &run_key);
    if json {
        // Machine consumers read JSONL — the record is a distinct
        // `"event":"drain_summary"` line alongside the per-emitter object.
        println!("{record}");
    } else {
        eprint!("\n{}", summary.render());
    }
    // Persist to ~/.aida/usage.jsonl so `aida usage` can chart cost-per-drain.
    // TASK-1165: the caller's injected root, not a cwd-derived one.
    // trace:TASK-1165 | ai:claude
    let project_root = drain_root;
    if usage::is_enabled(project_root) {
        usage::append_value(&record);
    }
    // STORY-730: persist the compact outcome to `.aida/last-drain.json` so the
    // next bare `aida status` can LEAD with a "since you were away" banner —
    // otherwise this tally dies on the ephemeral stderr render above. Sibling of
    // the live drain-state file (kept distinct so the "presence ⇒ live-or-crashed"
    // invariant of `drain-state.json` is unaffected). Best-effort. trace:STORY-730
    if let Some(root) = project_root {
        let previous = last_drain::LastDrainOutcome::read(root);
        let _ = last_drain::LastDrainOutcome::from_summary_with_previous(
            &summary,
            &ts,
            previous.as_ref(),
            Some(drain_invocation.clone()),
        )
        .write(root);
    }
    // STORY-712: emit the terminal QueueDrained wake — the "agent is done" an
    // overnight loop waits on. Drain-level, so no spec. Best-effort, not
    // telemetry-gated (supervision substrate). trace:TASK-988 | ai:claude
    if let Some(root) = project_root {
        let (_, run_uuid) = drain_state::current_context(root);
        events::emit(
            root,
            &events::Event::new(
                None,
                run_uuid,
                events::EventKind::QueueDrained {
                    shipped: summary.tallies.shipped,
                    shelved: summary.tallies.shelved,
                    // trace:TASK-1297 | ai:claude
                    // trace:BUG-1425 | ai:codex
                    excluded_from_batch,
                    // trace:BUG-1422 | ai:codex
                    ineligible: ineligible.to_vec(),
                },
            ),
        );
        let _ = notify::passive_check(root);
    }
}

/// Real [`auto_complete::BatchDriver`] for a `nextN` drain (TASK-293). Unlike
/// [`RealBatchDriver`], the "members" are not a `batch:NAME` tag — they are
/// simply the drivable queue head, re-resolved each call. A shipped spec
/// leaves the queue, so the head advances naturally; `drain_batch`'s `--max`
/// cap (pinned to `N`) stops the drain after N specs. trace:TASK-293
pub(crate) struct RealNextNDriver<'a> {
    pub(crate) storage: &'a Storage,
    pub(crate) user_id: String,
    pub(crate) role_override: Option<String>,
    pub(crate) variant: auto_complete::AutoCompleteVariant,
    pub(crate) json: bool,
    pub(crate) permission_mode: Option<String>,
    pub(crate) no_human: Option<auto_complete::NoHumanMode>,
    /// STORY-306: advisor-escalation mode, propagated to every member.
    pub(crate) escalate_mode: auto_complete::EscalateMode,
    /// BUG-311: outer `--steal`, propagated to every member's phase 1.
    pub(crate) steal: bool,
    /// TASK-559: outer `--force-claim`, propagated to every member's phase 1.
    pub(crate) force_claim: bool,
    /// STORY-281: outer `--allow-stale-base`, propagated to every member's
    /// phase 3 pre-flight stale-base check.
    pub(crate) allow_stale_base: bool,
    /// STORY-429: outer `--no-auto-rebase`, propagated to every member.
    pub(crate) no_auto_rebase: bool,
    /// TASK-966: project root + drain-start, for the `--max-tokens` meter. `None`
    /// when no token cap is active, so the headless-log scan is skipped.
    pub(crate) token_meter: Option<(std::path::PathBuf, std::time::SystemTime)>,
    /// BUG-862: entries routed to a different role that this implementer drain
    /// skipped before launching any phase-1 session.
    pub(crate) role_skipped: Vec<(String, String)>,
    pub(crate) seen_role_skips: std::collections::HashSet<String>,
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    pub(crate) pipeline_depth: usize,
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    pub(crate) pipelined_children: std::collections::HashMap<usize, std::process::Child>,
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    pub(crate) pipelined_result_paths: std::collections::HashMap<usize, std::path::PathBuf>,
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    pub(crate) next_pipelined_handle: usize,
}

impl auto_complete::BatchDriver for RealNextNDriver<'_> {
    fn next_head(&mut self) -> Option<String> {
        let (pick, role_skipped, blocked_skipped) =
            resolve_next_n_head(self.storage, &self.user_id, self.role_override.as_deref());
        for (spec, role) in &role_skipped {
            if self.seen_role_skips.insert(spec.clone()) {
                eprintln!("skipped {spec} — routed for {role}");
                self.role_skipped
                    .push((spec.clone(), format!("routed for {role}")));
            }
        }
        // BUG-1608: a dependent whose prerequisite is not Completed is
        // skipped and reported with its blocker. The reason is refreshed when
        // the blocker's state moves (e.g. In Progress → Needs Attention after
        // a shelve) so the summary names the blocker's final state.
        // trace:BUG-1608 | ai:claude
        for (spec, reason) in &blocked_skipped {
            match self.role_skipped.iter_mut().find(|(s, _)| s == spec) {
                Some(entry) if &entry.1 != reason => {
                    eprintln!("skipped {spec} — {reason}");
                    entry.1 = reason.clone();
                }
                Some(_) => {}
                None => {
                    eprintln!("skipped {spec} — {reason}");
                    self.role_skipped.push((spec.clone(), reason.clone()));
                }
            }
        }
        let pick = pick.map(|pick| pick.spec);
        // A dependent skipped earlier whose blocker has since Completed is
        // picked now — it is no longer "skipped". trace:BUG-1608 | ai:claude
        if let Some(spec) = &pick {
            self.role_skipped.retain(|(s, _)| s != spec);
        }
        pick
    }

    fn run_spec(&mut self, spec: &str) -> auto_complete::OrchestrationResult {
        // BUG-660: retry a transient/retryable per-spec failure (locked cache,
        // GH-API blip) through an exponential backoff instead of hammering or
        // immediately shelving — an unattended drive rides out a brief blip.
        // trace:BUG-660 | ai:claude
        drive_robustness::run_with_transient_backoff(|| {
            run_auto_complete(
                self.storage,
                &self.user_id,
                spec,
                self.variant,
                self.json,
                self.permission_mode.as_deref(),
                self.no_human,
                self.escalate_mode,
                // STORY-301: a batch / nextN member does not own the drain-state
                // file — the batch orchestrator created it.
                false,
                self.steal,
                self.force_claim,
                self.allow_stale_base,
                self.no_auto_rebase,
                None,
            )
        })
    }

    // TASK-966: cumulative reported tokens across this drain's headless logs.
    fn cumulative_tokens(&mut self) -> u64 {
        match &self.token_meter {
            Some((root, since)) => sum_headless_log_tokens(root, *since),
            None => 0,
        }
    }
}

impl RealNextNDriver<'_> {
    // trace:STORY-1091 trace:ADR-28 | ai:codex
    // trace:BUG-1805 | ai:claude
    pub(crate) fn child_common_args(
        &self,
        spec: &str,
        mode: auto_complete::AutoCompleteVariant,
    ) -> Vec<String> {
        pipelined_child_common_args(
            spec,
            mode,
            self.json,
            self.permission_mode.as_deref(),
            self.no_human,
            self.escalate_mode,
            self.steal,
            self.force_claim,
            self.allow_stale_base,
            self.no_auto_rebase,
        )
    }
}

impl auto_complete::PipelinedBatchDriver for RealNextNDriver<'_> {
    fn pipeline_depth(&self) -> usize {
        self.pipeline_depth
    }

    fn start_spec_through_ci(&mut self, spec: &str) -> auto_complete::PipelinedHandle {
        let handle = auto_complete::PipelinedHandle(self.next_pipelined_handle);
        self.next_pipelined_handle += 1;
        // trace:TASK-1603 | ai:codex
        // The child registers its run before bumping status. Pre-bumping here
        // creates an InProgress/no-lease window before it has ownership.
        let exe = aida_exe_path();
        let result_path = find_main_worktree_root()
            .unwrap_or_else(|_| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            })
            .join(".aida")
            .join("pipelined-results")
            .join(format!("{}-{}.json", spec, handle.0));
        let mut cmd = std::process::Command::new(exe);
        // BUG-1568: the child takes the drain lock at its top while the parent
        // batch drive still holds it, so it must BORROW. Without that it is
        // refused with "a drain is already running (pid <parent>)", the batch
        // driver reads the refusal as a phase failure of the member, and the
        // whole batch stops having shipped nothing.
        //
        // This is BUG-748's defect in a second place: that fix taught the
        // INTEGRATOR child to borrow, and the pipelined batch child never got
        // the same treatment. It only became reachable when pipeline_depth was
        // raised from 1 to 2 — at depth 1 no child overlaps a live parent lock.
        // Measured after that change: 221 consecutive drains shipping nothing
        // over 17.5 hours. trace:BUG-1568 trace:BUG-748 | ai:claude
        cmd.args(self.child_common_args(spec, auto_complete::AutoCompleteVariant::ThroughCi));
        for (k, v) in pipelined_child_env(&result_path) {
            cmd.env(k, v);
        }
        match cmd.spawn_retrying_etxtbsy() {
            Ok(child) => {
                self.pipelined_children.insert(handle.0, child);
                self.pipelined_result_paths.insert(handle.0, result_path);
                handle
            }
            Err(e) => {
                eprintln!(
                    "{} could not start pipelined implementer for {}: {}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    spec,
                    e
                );
                handle
            }
        }
    }

    fn wait_spec_through_ci(
        &mut self,
        handle: auto_complete::PipelinedHandle,
    ) -> auto_complete::OrchestrationResult {
        let Some(mut child) = self.pipelined_children.remove(&handle.0) else {
            return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Implementer);
        };
        let result_path = self.pipelined_result_paths.remove(&handle.0);
        // The sidecar is named `<spec>-<handle>.json`, and the handle is
        // numeric, so the last `-` separates them. Spec ids contain `-`
        // themselves (BUG-1462), which is why this splits from the RIGHT.
        // trace:BUG-1570 | ai:claude
        let spec_for_report = result_path
            .as_deref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .and_then(|s| s.rsplit_once('-').map(|(spec, _)| spec.to_string()))
            .unwrap_or_else(|| "the dispatched spec".to_string());
        let read_sidecar = || {
            result_path
                .as_deref()
                .and_then(read_pipelined_child_result_sidecar)
        };
        match child.wait() {
            Ok(status) => classify_pipelined_child_outcome(
                &spec_for_report,
                status.code(),
                status.success(),
                read_sidecar,
            ),
            // `wait()` itself failed, so there is no status to read at all.
            Err(_) => classify_pipelined_child_outcome(&spec_for_report, None, false, read_sidecar),
        }
    }

    fn finish_spec_after_ci(&mut self, spec: &str) -> auto_complete::OrchestrationResult {
        if i32::from(self.variant.last_phase()) <= auto_complete::Phase::Ci.index() {
            return auto_complete::OrchestrationResult::ok();
        }
        let project_root = match find_main_worktree_root() {
            Ok(root) => root,
            Err(_) => {
                return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Reviewer)
            }
        };
        let lookup = detect_open_pr_for_spec_via_forge(&project_root, spec);
        let (branch, pr) = match lookup {
            PrLookup::Found(pr) => (pr.head_branch, Some(pr.number as u32)),
            _ => return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Reviewer),
        };
        let resume = Some(ResumeEntry {
            // Re-probe phase 2 in this process so the reviewer carries a
            // head-bound CI conclusion, even if the pipelined child already
            // observed green. trace:BUG-1460 | ai:codex
            start_phase: auto_complete::Phase::Ci,
            branch,
            pr,
            head_sha: None,
            from_pr: true,
            ci_terminal_sha: None,
            ci_terminal_green: None,
        });
        run_auto_complete(
            self.storage,
            &self.user_id,
            spec,
            self.variant,
            self.json,
            self.permission_mode.as_deref(),
            self.no_human,
            self.escalate_mode,
            false,
            self.steal,
            self.force_claim,
            self.allow_stale_base,
            self.no_auto_rebase,
            resume,
        )
    }
}

/// Entry point for `aida queue work nextN --auto-complete` (TASK-293). Drains
/// the next `N` items from the queue head. Depth 1 keeps the historical
/// one-full-lifecycle per member loop; depth >1 starts later implementer/CI
/// children while this parent serializes reviewer/merge/pull/build for ready
/// PRs. Never returns.
// trace:TASK-293 STORY-1091 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_auto_complete_next_n(
    storage: &Storage,
    user_id: &str,
    n: usize,
    variant: auto_complete::AutoCompleteVariant,
    json: bool,
    permission_mode: Option<&str>,
    role_override: Option<&str>,
    no_human: Option<auto_complete::NoHumanMode>,
    escalate_mode: auto_complete::EscalateMode,
    // BUG-311: outer `--steal`, threaded to every drained member's phase 1.
    steal: bool,
    // TASK-559: outer `--force-claim`, threaded to every drained member's phase 1.
    force_claim: bool,
    // STORY-281: outer `--allow-stale-base`, threaded to every drained
    // member's phase 3 pre-flight stale-base check.
    allow_stale_base: bool,
    // STORY-429: outer `--no-auto-rebase`, threaded to every drained member.
    no_auto_rebase: bool,
    // EPIC-28: outer `--max-failures` cap. trace:EPIC-28 | ai:claude
    max_failures: Option<usize>,
    // TASK-966: hard budget caps for the whole drain. trace:TASK-966 | ai:claude
    caps: &drain_caps::DrainCaps,
) -> ! {
    if !json {
        eprintln!();
        eprintln!(
            "{} {} {}",
            "🚀".bold(),
            format!("auto-complete: next {n}").bold(),
            format!("({})", variant.describe()).dimmed()
        );
        // Acceptance: when N exceeds the queue length, drain all available
        // and say so up front rather than silently shipping fewer than asked.
        if let Ok(drivable) = drivable_queued_count(storage, user_id, role_override) {
            if drivable < n {
                eprintln!(
                    "  {} only {} drivable item{} queued — draining those",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    drivable,
                    if drivable == 1 { "" } else { "s" }
                );
            }
        }
    }

    // STORY-301: write the drain-state file — the drivable queue head, capped
    // at N, is the member list. Best-effort. trace:STORY-301 | ai:claude
    //
    // TASK-1490: apply the same `aida_core::pickability::pickability()`
    // verdict dispatch is gated on (BUG-1608) before capping at N. A
    // `BlockedBy` dependent is skipped here exactly as `resolve_next_n_head`
    // skips it once the live drain reaches it, so the state file never
    // predicts a member the drain will not actually run.
    let drain_root = find_main_worktree_root().ok();
    if let Some(root) = &drain_root {
        if let Ok(candidates) =
            auto_complete_head_candidates_with_blocked(storage, user_id, role_override)
        {
            let specs: Vec<String> = candidates
                .into_iter()
                .filter(|(_, status, _)| auto_complete_head_drivable(status))
                // trace:TASK-1490 | ai:claude
                .filter(|(_, _, blocked)| blocked.is_none())
                .take(n)
                .map(|(id, _, _)| id)
                .collect();
            let pipeline_depth = DrainTuning::resolve(root).pipeline_depth();
            let _ = drain_state::DrainState::new_next_n(n, &specs)
                .with_pipeline_depth(pipeline_depth)
                .write(root);
        }
    }

    // TASK-967: capture the drain origin (wall clock + base HEAD) up front so
    // the exit summary can report token spend, diff stats, and elapsed time
    // regardless of whether a token cap is active. trace:TASK-967 | ai:claude
    let drain_started = std::time::SystemTime::now();
    let drain_invocation = last_drain::DrainInvocation::capture();
    let drain_clock = std::time::Instant::now();
    let drain_base_sha = drain_root.as_deref().and_then(current_branch_head_sha);
    // TASK-966: arm the token meter only when a `--max-tokens` cap is set.
    let token_meter = caps
        .max_tokens
        .and(drain_root.clone())
        .map(|root| (root, drain_started));
    let pipeline_depth = drain_root
        .as_deref()
        .map(|root| DrainTuning::resolve(root).pipeline_depth())
        .unwrap_or_else(drain_state::default_pipeline_depth);
    let mut driver = RealNextNDriver {
        storage,
        user_id: user_id.to_string(),
        role_override: role_override.map(canonical_role_name),
        variant,
        json,
        permission_mode: permission_mode.map(|s| s.to_string()),
        no_human,
        escalate_mode,
        steal,
        force_claim,
        allow_stale_base,
        no_auto_rebase,
        token_meter,
        role_skipped: Vec::new(),
        seen_role_skips: std::collections::HashSet::new(),
        pipeline_depth,
        pipelined_children: std::collections::HashMap::new(),
        pipelined_result_paths: std::collections::HashMap::new(),
        next_pipelined_handle: 1,
    };
    // EPIC-28: apply the same default failure cap as the batch path.
    // trace:EPIC-28 | ai:claude
    let max_failures = max_failures.or(Some(DEFAULT_MAX_FAILURES));
    // TASK-966: thread the hard budget caps through the drain. A cap stop is a
    // clean `MaxReached` carrying the reason in `cap_stop`. trace:TASK-966
    let mut cap_stop = None;
    let mut result = auto_complete::drain_batch_pipelined_with_caps(
        &mut driver,
        Some(n),
        max_failures,
        caps,
        drain_clock,
        &mut cap_stop,
    );
    result.skipped.extend(driver.role_skipped);
    // An empty queue (nothing shipped, nothing punted, nothing to drain) is a
    // user error — surface it with a non-zero exit even though `drain_batch`
    // reports it as a clean `Drained`, matching the `--batch` drain. A drain
    // where every member punted is *not* empty (STORY-276).
    let exit_code = if let Some(stop) = &cap_stop {
        // TASK-966: a budget cap stopped the drain — report why, exit distinctly.
        emit_drain_cap_stop(stop, json);
        DRAIN_CAP_EXIT_CODE
    } else if matches!(result.outcome, auto_complete::BatchDrainOutcome::Drained)
        && result.shipped.is_empty()
        && result.punted.is_empty()
        && result.escalated.is_empty()
    {
        1
    } else {
        result.exit_code
    };
    // TASK-1456: resolve which of the (otherwise silently skipped) queued
    // candidates are Done-with-a-still-live-refusal, so an all-refused idle
    // wave surfaces them instead of just "nothing to drive". Best-effort —
    // a resolution failure must not change the drain's exit code.
    // trace:TASK-1456 | ai:claude
    let rework_needed =
        resolve_queue_rework_needed(storage, user_id, role_override).unwrap_or_default();
    emit_next_n_drain_summary(n, &result, exit_code, json, &rework_needed);
    // TASK-967: permanent exit summary + cost-per-drain telemetry.
    finalize_drain_summary(
        "next-n",
        format!("next {n}"),
        &result.outcome,
        cap_stop.as_ref(),
        drain_summary::DrainTallies {
            shipped: result.shipped.len(),
            shelved: result.shelved.len(),
            skipped: result.skipped.len(),
            punted: result.punted.len(),
            escalated: result.escalated.len(),
        },
        drain_root.as_deref(),
        drain_base_sha.as_deref(),
        drain_started,
        drain_clock.elapsed(),
        &drain_invocation,
        json,
        // TASK-1297: a nextN drain has no `--batch` filter, so there is
        // nothing excluded to report. trace:TASK-1297 | ai:claude
        Some(0),
        &[],
    );
    // STORY-301: clean exit removes the drain-state file; a crash leaves it.
    if let Some(root) = &drain_root {
        let _ = drain_state::DrainState::clear(root);
    }
    std::process::exit(exit_code);
}

/// TASK-966: announce a clean budget-cap stop — the one-line reason (human) or a
/// JSON event (machine). The in-flight head stays queued; the operator re-runs
/// the drain to continue once the budget refreshes.
// trace:TASK-966 | ai:claude
pub(crate) fn emit_drain_cap_stop(stop: &drain_caps::CapStop, json: bool) {
    if json {
        let obj = serde_json::json!({
            "event": "drain-cap-stop",
            "cap": stop.flag(),
            "reason": stop.reason(),
        });
        println!("{obj}");
    } else {
        eprintln!();
        eprintln!(
            "  {} {}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
            stop.reason()
        );
    }
}

/// Print the closing summary of a `nextN --auto-complete` drain (TASK-293):
/// what shipped, where it stopped, and what is left queued. The per-spec
/// failure epilogue + recovery hint are already printed by `orchestrate`;
/// this adds the drain-level framing. trace:TASK-293 | ai:claude
pub(crate) fn emit_next_n_drain_summary(
    n: usize,
    result: &auto_complete::BatchDrainResult,
    exit_code: i32,
    json: bool,
    // TASK-1456: Done specs the drain skipped because they carry a
    // still-live review refusal — named here so an all-refused wave reports
    // rework instead of going quiet. trace:TASK-1456 | ai:claude
    rework_needed: &[events::IneligibleBatchMember],
) {
    use auto_complete::BatchDrainOutcome;

    if json {
        let outcome = match &result.outcome {
            BatchDrainOutcome::Drained => "drained",
            BatchDrainOutcome::MaxReached => "max-reached",
            BatchDrainOutcome::Failed(_) => "failed",
            BatchDrainOutcome::Stalled => "stalled",
            BatchDrainOutcome::Mismatched { .. } => "mismatched",
            BatchDrainOutcome::Inconclusive => "inconclusive",
            BatchDrainOutcome::Held => "held",
            // EPIC-28 trace:EPIC-28 | ai:claude
            BatchDrainOutcome::DrainedWithShelved => "drained-with-shelved",
        };
        let mut obj = serde_json::Map::new();
        obj.insert(
            "event".to_string(),
            serde_json::Value::String("next-n-drain".into()),
        );
        obj.insert(
            "requested".to_string(),
            serde_json::Value::Number((n as u64).into()),
        );
        obj.insert(
            "outcome".to_string(),
            serde_json::Value::String(outcome.into()),
        );
        obj.insert(
            "shipped".to_string(),
            serde_json::Value::Array(
                result
                    .shipped
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "shipped_count".to_string(),
            serde_json::Value::Number((result.shipped.len() as u64).into()),
        );
        // STORY-276: punted members — parked in Needs Attention, not shipped.
        obj.insert(
            "punted".to_string(),
            serde_json::Value::Array(
                result
                    .punted
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "punted_count".to_string(),
            serde_json::Value::Number((result.punted.len() as u64).into()),
        );
        // STORY-306: escalated members — left for a human, not shipped.
        obj.insert(
            "escalated".to_string(),
            serde_json::Value::Array(
                result
                    .escalated
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "escalated_count".to_string(),
            serde_json::Value::Number((result.escalated.len() as u64).into()),
        );
        obj.insert(
            "stopped_at".to_string(),
            match &result.stopped_at {
                Some(s) => serde_json::Value::String(s.clone()),
                None => serde_json::Value::Null,
            },
        );
        obj.insert(
            "exit_code".to_string(),
            serde_json::Value::Number(exit_code.into()),
        );
        // TASK-1456: mirror the `--batch` drain's `ineligible` field so a
        // machine consumer can tell "genuinely nothing queued" apart from
        // "queued, but every candidate needs a rework round".
        // trace:TASK-1456 | ai:claude
        obj.insert(
            "rework_needed".to_string(),
            serde_json::to_value(rework_needed)
                .unwrap_or_else(|_| serde_json::Value::Array(vec![])),
        );
        println!("{}", serde_json::Value::Object(obj));
        return;
    }

    let shipped_list = if result.shipped.is_empty() {
        "(none)".to_string()
    } else {
        result.shipped.join(", ")
    };
    let count = result.shipped.len();
    let plural = if count == 1 { "" } else { "s" };
    eprintln!();
    match &result.outcome {
        // TASK-1456: an all-refused queue must not read the same as a
        // genuinely empty one — name the rework candidates and the working
        // recovery route instead of the generic "nothing to drive".
        // trace:TASK-1456 | ai:claude
        BatchDrainOutcome::Drained
            if result.shipped.is_empty()
                && result.punted.is_empty()
                && result.escalated.is_empty()
                && !rework_needed.is_empty() =>
        {
            eprintln!(
                "{} {}",
                "⏸".yellow().bold(),
                next_n_rework_idle_message(rework_needed),
            );
        }
        BatchDrainOutcome::Drained
            if result.shipped.is_empty()
                && result.punted.is_empty()
                && result.escalated.is_empty() =>
        {
            eprintln!(
                "{} no drivable items in the queue — nothing to drive",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold()
            );
        }
        BatchDrainOutcome::Drained => {
            eprintln!(
                "{} next {n} drained — queue exhausted, {count} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
        }
        BatchDrainOutcome::MaxReached => {
            eprintln!(
                "{} next {n} done — {count} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
            eprintln!(
                "  {} more items remain queued — re-run `aida queue work next{n} --auto-complete` to continue",
                "→".dimmed()
            );
        }
        BatchDrainOutcome::Failed(phase) => {
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} next {n} drain stopped at {} (phase {} failed)",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                stopped.bold(),
                phase.index()
            );
            eprintln!(
                "  {} already shipped ({count}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} the rest of the queue is untouched — fix {stopped} (hint above), then re-run",
                "→".dimmed()
            );
        }
        BatchDrainOutcome::Stalled => {
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} next {n} drain stopped — {} stayed at the head after a successful run \
                 (queue did not advance)",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                stopped.bold()
            );
            eprintln!(
                "  {} already shipped ({count}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} check {stopped}'s status (`aida show {stopped}`) — it may not have \
                 been dequeued",
                "→".dimmed()
            );
        }
        // BUG-245: phase 1 shipped a different spec than the dispatched head.
        // Credit the truth, name what stayed queued. trace:BUG-245 | ai:claude
        BatchDrainOutcome::Mismatched {
            dispatched,
            shipped,
        } => {
            eprintln!(
                "{} next {n} drain stopped — phase 1 was dispatched for {} \
                 but the PR credited {}",
                crate::glyph(crate::glyphs::Glyph::Info).cyan().bold(),
                dispatched.bold(),
                shipped.bold(),
            );
            eprintln!(
                "  {} already shipped ({count}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} {} is still queued — pick it back up with `aida queue work {}`, \
                 or remove it with `aida queue remove {}` if {} subsumed it",
                "→".dimmed(),
                dispatched,
                dispatched,
                dispatched,
                shipped,
            );
        }
        // BUG-257: a transient GH-API outage during phase-1 PR lookup paused
        // the drain. The spec stays in its current state for retry.
        // trace:BUG-257 | ai:claude
        BatchDrainOutcome::Inconclusive => {
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} next {n} drain paused at {} — phase 1 inconclusive \
                 (GH API unreachable)",
                "⏸".yellow().bold(),
                stopped.bold(),
            );
            eprintln!(
                "  {} already shipped ({count}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} transient — retry once the API is reachable: \
                 `gh api /rate_limit` then re-run",
                "→".dimmed()
            );
        }
        // BUG-250: a member deliberately held its PR — pause for the gate.
        // trace:BUG-250 | ai:claude
        BatchDrainOutcome::Held => {
            let stopped = result.stopped_at.as_deref().unwrap_or("<spec>");
            eprintln!(
                "{} next {n} drain paused at {} — PR deliberately held",
                "⏸".yellow().bold(),
                stopped.bold(),
            );
            eprintln!(
                "  {} already shipped ({count}): {}",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                shipped_list
            );
            eprintln!(
                "  {} run your gate, open the PR (`gh pr create`), then re-run",
                "→".dimmed()
            );
        }
        // EPIC-28 trace:EPIC-28 | ai:claude
        BatchDrainOutcome::DrainedWithShelved => {
            eprintln!(
                "{} next {n} drained with shelved members — \
                 {count} spec{plural} shipped: {}",
                crate::glyph(crate::glyphs::Glyph::Check).green().bold(),
                shipped_list.bold()
            );
        }
    }
    // STORY-276: name the members a headless implementer punted — the drain
    // advanced past them, but they parked in Needs Attention rather than
    // shipping, so they need advisor triage.
    if !result.punted.is_empty() {
        let pn = result.punted.len();
        eprintln!(
            "  {} {pn} spec{} punted to Needs Attention: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if pn == 1 { "" } else { "s" },
            result.punted.join(", ")
        );
    }
    // STORY-306: name the members the drain escalated to a human.
    if !result.escalated.is_empty() {
        let en = result.escalated.len();
        eprintln!(
            "  {} {en} spec{} escalated to a human: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if en == 1 { "" } else { "s" },
            result.escalated.join(", ")
        );
    }
    // EPIC-28: shelved + skipped members. trace:EPIC-28 | ai:claude
    if !result.shelved.is_empty() {
        let sn = result.shelved.len();
        eprintln!(
            "  {} {sn} spec{} shelved on phase failure: {} — triage with \
             `aida findings list`",
            "⏸".yellow(),
            if sn == 1 { "" } else { "s" },
            result.shelved.join(", ")
        );
    }
    if !result.skipped.is_empty() {
        let kn = result.skipped.len();
        let render: Vec<String> = result
            .skipped
            .iter()
            .map(|(spec, reason)| format!("{spec} ({reason})"))
            .collect();
        eprintln!(
            "  {} {kn} dependent spec{} skipped: {}",
            "⤳".yellow(),
            if kn == 1 { "" } else { "s" },
            render.join(", ")
        );
    }
}

/// TASK-266: append this `--auto-complete` run to
/// `~/.aida/auto-complete.jsonl` and, on a phase failure, auto-draft a Draft
/// BUG for the failure. Honours the same opt-out as `aida usage`
/// (`AIDA_TELEMETRY=0` / `[telemetry] enabled = false`). Best-effort
/// throughout — telemetry must never break or delay the orchestrator exit.
/// trace:TASK-266 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_auto_complete_run(
    driver: &RealPhaseDriver,
    project_root: &std::path::Path,
    spec: &str,
    variant: auto_complete::AutoCompleteVariant,
    result: &auto_complete::OrchestrationResult,
    started_at: chrono::DateTime<chrono::Utc>,
    completed_at: chrono::DateTime<chrono::Utc>,
    json: bool,
    lifecycle_skip: auto_complete::LifecycleSkip,
) {
    if !usage::is_enabled(Some(project_root)) {
        return;
    }

    let phase_durations: Vec<auto_complete_telemetry::PhaseDuration> = result
        .phase_durations
        .iter()
        .map(|(phase, ms)| auto_complete_telemetry::PhaseDuration {
            phase: phase.index() as u8,
            slug: phase.slug().to_string(),
            elapsed_ms: *ms as u64,
        })
        .collect();

    let mut event = auto_complete_telemetry::AutoCompleteEvent {
        spec_id: spec.to_string(),
        started_at: started_at.to_rfc3339(),
        completed_at: completed_at.to_rfc3339(),
        outcome: if result.exit_code == 0 {
            "success".to_string()
        } else {
            "failed".to_string()
        },
        variant: variant.slug().to_string(),
        failed_phase: result.failed_phase.map(|p| p.index() as u8),
        failure_kind: result
            .failure
            .as_ref()
            .map(|f| f.kind.cause_slug().to_string()),
        failure_message: result.failure.as_ref().map(|f| f.reason.clone()),
        phase_durations,
        total_ms: result.total_ms as u64,
        drafted_bug: None,
        failure_comment: None,
        binary_sha: build_sha_short(),
        auto_rebase: driver.auto_rebase_events.clone(),
        lifecycle_skips: lifecycle_skip.active_tokens(),
    };

    // On a phase failure, attach a Draft BUG. Reuse an open BUG for the same
    // spec / phase / kind rather than spamming the backlog when a still-broken
    // `--auto-complete` is re-run. trace:BUG-864 | ai:codex
    if let (Some(phase), Some(failure)) = (result.failed_phase, result.failure.as_ref()) {
        match absorb_open_failure_bug(project_root, spec, phase, failure, &completed_at) {
            Some(existing) => {
                if !json {
                    eprintln!(
                        "  {} this failure recurs — tracked in {} (no new BUG drafted)",
                        "📋".dimmed(),
                        existing.cyan(),
                    );
                }
                event.drafted_bug = Some(existing);
            }
            // BUG-657: suppress the auto-file entirely when the failure is
            // ENVIRONMENTAL (disk full / OOM) — the auto-draft text itself tells
            // the triager to reject those, so filing one just creates a Draft to
            // immediately reject. The telemetry line still records the failure.
            None if auto_complete::is_environmental_failure(&failure.reason) => {
                if !json {
                    eprintln!(
                        "  {} environmental failure (disk/OOM) — not auto-drafting a BUG \
                         (fix the host, then re-drive)",
                        "📋".dimmed(),
                    );
                }
            }
            // BUG-908: a lease-conflict shelve already names the blocking
            // lease/worktree in the spec's FailureReason. Auto-drafting a new
            // BUG for that same typed cause recreated the spam loop this fix
            // is meant to stop.
            // trace:BUG-908 | ai:codex
            None if failure.kind == auto_complete::FailureKind::LeaseConflict => {
                if !json {
                    eprintln!(
                        "  {} lease-conflict failure — not auto-drafting a BUG \
                         (inspect the named lease/worktree, then re-drive)",
                        "📋".dimmed(),
                    );
                }
            }
            // TASK-1564: the narrative record is a comment on the parent spec,
            // not a new draft spec. Nothing else moves — the non-zero exit, the
            // ledger line appended below, and the seat routing are untouched.
            // trace:TASK-1564 | ai:claude
            None => {
                let hint = auto_complete::recovery_hint(
                    phase,
                    failure.kind,
                    &auto_complete::PhaseDriver::hint_context(driver),
                );
                if let Some(comment_id) = note_auto_complete_failure_on_parent(
                    project_root,
                    spec,
                    phase,
                    failure,
                    &hint,
                    &event,
                ) {
                    if !json {
                        eprintln!(
                            "  {} recorded this failure as a comment on {} \
                             (no new spec filed) — read it: `aida comment list {}`",
                            "📋".dimmed(),
                            spec.cyan(),
                            spec,
                        );
                    }
                    event.failure_comment = Some(comment_id);
                }
            }
        }
    }

    auto_complete_telemetry::append_event(&event);

    // SPIKE-67 slice 3: record any stated-rule violation this drain observed
    // (CI red on fmt/clippy/provenance, a reviewer RequestChanges on a rule, or
    // a punt citing a rule) — the real-time gate-vs-rule evidence path. Reuses
    // the failure the orchestrator already classified; observe-only, opt-in,
    // privacy-floor-preserving, best-effort. trace:SPIKE-67 | ai:claude
    let outcome = rule_violation::DrainOutcome {
        failed_phase_slug: result.failed_phase.map(|p| p.slug()),
        failure_kind: result.failure.as_ref().map(|f| f.kind.cause_slug()),
        failure_message: result.failure.as_ref().map(|f| f.reason.as_str()),
        punt_reason: result.punt_reason.as_deref(),
    };
    rule_violation::record(
        project_root,
        &outcome,
        spec,
        driver.no_human.is_some(),
        variant.slug(),
        build_sha_short(),
    );
}

// TASK-1564 moved the recurrence bump into `drain_failure_note` so the note
// path and the legacy auto-drafted-BUG absorb path share one implementation.
// trace:TASK-1564 | ai:claude
use drain_failure_note::increment_auto_failure_attempts;

/// BUG-864: absorb a recurring phase failure into the open auto-drafted BUG
/// already tracking that `(spec, phase, failure-kind)` signature. The store is
/// authoritative for the dedupe window: Draft/Approved records absorb, while a
/// triaged Rejected/Completed record ends the window and lets the next failure
/// file fresh. The older JSONL dedupe remains only as a fallback and is verified
/// against the same open-status rule before reuse.
// trace:TASK-266 trace:BUG-657 trace:BUG-864 | ai:codex
pub(crate) fn absorb_open_failure_bug(
    project_root: &std::path::Path,
    spec: &str,
    phase: auto_complete::Phase,
    failure: &auto_complete::PhaseFailure,
    completed_at: &chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    if let Some(id) = absorb_open_failure_bug_from_store(
        project_root,
        spec,
        phase.index() as u8,
        phase.slug(),
        failure.kind.cause_slug(),
        &completed_at.to_rfc3339(),
    ) {
        return Some(id);
    }

    let cutoff = chrono::Utc::now() - chrono::Duration::hours(24);
    let candidate = auto_complete_telemetry::dedup_failure_bug(
        &auto_complete_telemetry::read_events(),
        spec,
        phase.index() as u8,
        failure.kind.cause_slug(),
        cutoff,
    )?;
    open_auto_failure_bug_by_id(project_root, &candidate).map(|_| candidate)
}

pub(crate) fn absorb_open_failure_bug_from_store(
    project_root: &std::path::Path,
    spec: &str,
    phase_index: u8,
    phase_slug: &str,
    failure_kind: &str,
    latest_at: &str,
) -> Option<String> {
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return None;
    };
    let dispenser = load_dispenser(&store_path).ok()?;
    let inner = aida_core::GitBackend::new(&store_path)
        .ok()?
        .with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path).ok()?;
    let mut candidate = backend
        .list_requirements(false)
        .ok()?
        .into_iter()
        .filter(|req| auto_failure_bug_matches(req, spec, phase_index, phase_slug, failure_kind))
        .max_by_key(|req| req.modified_at)?;
    let id = candidate.display_id();
    candidate.description = increment_auto_failure_attempts(&candidate.description, latest_at);
    candidate.modified_at = chrono::Utc::now();
    backend.update_requirement(&candidate).ok()?;
    Some(id)
}

pub(crate) fn open_auto_failure_bug_by_id(
    project_root: &std::path::Path,
    spec_id: &str,
) -> Option<aida_core::Requirement> {
    let store_path = detect_distributed_store_from(project_root)?;
    let dispenser = load_dispenser(&store_path).ok()?;
    let inner = aida_core::GitBackend::new(&store_path)
        .ok()?
        .with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(&store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path).ok()?;
    let req = backend.get_requirement_by_spec_id(spec_id).ok()??;
    if is_open_auto_failure_bug(&req) {
        Some(req)
    } else {
        None
    }
}

pub(crate) fn auto_failure_bug_matches(
    req: &aida_core::Requirement,
    spec: &str,
    phase_index: u8,
    phase_slug: &str,
    failure_kind: &str,
) -> bool {
    let expected_title =
        format!("auto-complete failure: phase {phase_index} ({phase_slug}) on {spec}");
    is_open_auto_failure_bug(req)
        && req.title == expected_title
        && auto_failure_kind_matches(req, failure_kind)
}

pub(crate) fn is_open_auto_failure_bug(req: &aida_core::Requirement) -> bool {
    matches!(
        req.status,
        aida_core::RequirementStatus::Draft | aida_core::RequirementStatus::Approved
    ) && !req.archived
        && req.req_type == aida_core::RequirementType::Bug
        && req.tags.contains("auto-complete")
        && req.tags.contains("auto-drafted")
}

pub(crate) fn auto_failure_kind_matches(req: &aida_core::Requirement, failure_kind: &str) -> bool {
    req.tags.contains(&format!("failure-kind:{failure_kind}"))
        || req
            .description
            .contains(&format!("Failure kind: `{failure_kind}`"))
}

/// Record an `--auto-complete` phase failure as a comment on the parent spec.
///
/// Replaces the TASK-266 Draft-BUG auto-file: the narrative record is the same,
/// but a comment informs the next reader of the parent instead of demanding a
/// triage disposition the way a draft spec does. A recurrence of the same
/// `(phase, failure-kind)` signature bumps the existing note's counter rather
/// than appending a second one — the BUG-864 anti-spam rule, carried over.
///
/// Returns the note's comment UUID, or `None` when the store could not be
/// resolved or written (best-effort — the JSONL ledger line still records the
/// failure either way, and the drain still exits non-zero).
// trace:TASK-1564 trace:TASK-266 trace:BUG-864 | ai:claude
pub(crate) fn note_auto_complete_failure_on_parent(
    project_root: &std::path::Path,
    spec: &str,
    phase: auto_complete::Phase,
    failure: &auto_complete::PhaseFailure,
    hint: &str,
    event: &auto_complete_telemetry::AutoCompleteEvent,
) -> Option<String> {
    let phase_index = phase.index() as u8;
    let phase_slug = phase.slug();
    let failure_kind = failure.kind.cause_slug();

    let seat = match std::env::var("AIDA_SESSION_ROLE") {
        Ok(role) if !role.trim().is_empty() => format!("{phase_slug} (session role: {role})"),
        _ => phase_slug.to_string(),
    };
    let branch = aida_core::git_ops::current_branch(project_root).ok();
    let commit = aida_core::git_ops::head_sha(project_root)
        .ok()
        .map(|sha| sha.chars().take(12).collect::<String>());
    let durations = if event.phase_durations.is_empty() {
        "  (none recorded)".to_string()
    } else {
        event
            .phase_durations
            .iter()
            .map(|d| format!("  - phase {} ({}): {} ms", d.phase, d.slug, d.elapsed_ms))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let json_line = serde_json::to_string(event).unwrap_or_default();

    let body = drain_failure_note::render(&drain_failure_note::NoteContext {
        spec,
        phase_index,
        phase_slug,
        seat: &seat,
        failure_kind,
        failure_reason: &failure.reason,
        hint,
        branch: branch.as_deref(),
        commit: commit.as_deref(),
        completed_at: &event.completed_at,
        durations: &durations,
        telemetry_json: &json_line,
    });

    let store_path = detect_distributed_store_from(project_root)?;
    let mut storage = Storage::new(&store_path);
    if let Ok(dispenser) = load_dispenser(&store_path) {
        storage = storage.with_dispenser(dispenser);
    }
    let sig = drain_failure_note::signature(phase_index, phase_slug, failure_kind);
    match comment_cmd::bump_or_add_marked_comment(
        &storage,
        spec,
        &sig,
        &body,
        "auto-complete",
        &event.completed_at,
    ) {
        Ok(id) => Some(id.to_string()),
        Err(_) => None,
    }
}

#[cfg(test)]
#[path = "tests/task_266_tests.rs"]
mod task_266_tests;

#[cfg(test)]
#[path = "tests/task_1564_drain_failure_note_tests.rs"]
mod task_1564_drain_failure_note_tests;

pub(crate) fn auto_complete_queue_add_args(spec: &str) -> Vec<&str> {
    vec!["queue", "add", spec, "--for", "implementer", "--no-scope"]
}

// trace:TASK-1155 trace:ADR-11 | ai:codex
/// STORY-776: `aida do <spec>` — the universal dispatcher. Reads the advisor's
/// bless-time `execution_mode` and routes to the right harness, printing the
/// human contract BEFORE anything starts. ADR-13 fixes the taxonomy (drain |
/// drive | guided | operator | decide), ADR-14 the asymmetric one-shot
/// `--mode` override, ADR-15 the ungroomed path (TTY micro-groom with a
/// visible reasoning line; headless refusal). The pure policy lives in
/// `do_dispatch.rs`; this handler does the IO.
// trace:STORY-776 | ai:claude
pub(crate) fn run_do_drive(
    storage: &Storage,
    spec: &str,
    mode_flag: Option<&str>,
    force: bool,
) -> Result<()> {
    use aida_core::ExecutionMode;
    let spec = spec.trim();
    if spec.is_empty() {
        anyhow::bail!("aida do needs a spec to dispatch. Usage: aida do <SPEC> [--mode MODE].");
    }
    let store = storage.load()?;
    let req = store
        .requirements
        .iter()
        .find(|r| spec_matches(r, spec))
        .ok_or_else(|| anyhow::anyhow!("no requirement matches `{spec}`"))?;
    let display = req.display_id();

    // An epic is never a unit of dispatch — same rule as the zen gate.
    if req.req_type == aida_core::RequirementType::Epic {
        anyhow::bail!(
            "{display} is an epic — a read-only rollup of its children, not a unit of work. \
             Dispatch one of its bounded children instead (aida graph tree {display})."
        );
    }

    let requested: Option<ExecutionMode> = match mode_flag {
        Some(raw) => Some(raw.parse().map_err(|e: String| anyhow::anyhow!(e))?),
        None => None,
    };

    // Resolve the EFFECTIVE mode: groomed field + ADR-14 override ladder, or
    // the ADR-15 ungroomed path.
    let effective: ExecutionMode = match (req.execution_mode, requested) {
        (Some(groomed), Some(req_mode)) => {
            match do_dispatch::classify_mode_override(groomed, req_mode, force) {
                do_dispatch::OverrideVerdict::Noop => groomed,
                do_dispatch::OverrideVerdict::Allowed { banner } => {
                    eprintln!(
                        "  {} {}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        banner
                    );
                    req_mode
                }
                do_dispatch::OverrideVerdict::NeedsForce { refusal }
                | do_dispatch::OverrideVerdict::Refused { refusal } => {
                    anyhow::bail!("{refusal}");
                }
            }
        }
        (Some(groomed), None) => groomed,
        (None, Some(req_mode)) => {
            // Explicit flag on an ungroomed spec: honor it one-shot — the
            // refusal exists for flag-less guesswork, not explicit intent.
            eprintln!(
                "  {} {display} is ungroomed — using --mode {req_mode} one-shot \
                 (not persisted; `aida groom` or `aida edit {display} --mode {req_mode}` \
                 makes it durable)",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
            req_mode
        }
        (None, None) => do_micro_groom_mode(req, &display, &store)?,
    };

    // The human-contract banner — ALWAYS printed before any harness acts.
    // `aida do` is a pickup surface; keep slice 1 interactive-only (headless
    // prompt propagation belongs to TASK-1278).
    // trace:STORY-1221 | ai:codex
    if std::env::var("AIDA_HEADLESS").ok().as_deref() != Some("1") {
        if let Some(block) = protocol_cmd::pickup_block_for_requirement(&store, req) {
            eprintln!("{}\n", block);
        }
    }
    // STORY-1434: pickup is the moment a carved-out criterion's stale text
    // does damage — a drain/operator harness has nothing else forcing a
    // re-read of the description before it acts. Resolve each
    // `carved-out-to` target's (display id, title) here (this fn already
    // holds the full store) and hand the pure formatter the plain pairs.
    // trace:STORY-1434 | ai:claude
    if matches!(effective, ExecutionMode::Drain | ExecutionMode::Operator) {
        let carried_by: Vec<(String, String)> = carved_out_targets(req)
            .iter()
            .filter_map(|target_id| store.requirements.iter().find(|r| r.id == *target_id))
            .map(|t| (t.display_id(), t.title.clone()))
            .collect();
        if let Some(warning) = do_dispatch::carve_out_pickup_warning(&carried_by) {
            eprintln!(
                "  {} {}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                warning
            );
        }
    }
    eprintln!(
        "  {} {} · {}",
        crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
        display.bold(),
        do_dispatch::human_contract(effective)
    );

    // Route. drain/drive stay thin wrappers over the ONE per-spec engine
    // (ADR-7/ADR-11) via self-invocation; guided/operator/decide are
    // human-seat surfaces.
    let self_invoke = |args: &[&str], context: &str| -> Result<()> {
        eprintln!(
            "  {} aida {}",
            crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
            args.join(" ").dimmed()
        );
        let status = std::process::Command::new(resolve_aida_exe())
            .args(args)
            .status_retrying_etxtbsy()
            .with_context(|| format!("failed to launch `aida {context}`"))?;
        if !status.success() {
            std::process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    };
    match effective {
        ExecutionMode::Drain => {
            // `aida zen` = the one-shot autonomous implement+ship drive: its
            // own preflight gates (eligibility, suitability, scope routing),
            // then the shared engine with review + merge phases.
            self_invoke(&["zen", &display], "zen")
        }
        ExecutionMode::Drive => self_invoke(
            &["queue", "work", &display, "--auto-complete=through-ci"],
            "queue work --auto-complete=through-ci",
        ),
        ExecutionMode::Guided => {
            // A ready worktree + the guided session, prompt pre-filled. The
            // worktree primitive takes the implementer lease (Approved → In
            // Progress) exactly like `aida worktree enter <spec>`.
            //
            // trace:TASK-1162 | ai:claude — the launch honors the session
            // vendor: claude expands the guided-implement skill natively,
            // codex is seeded with the adapted prompt, and agy is refused by
            // dispatch policy — BEFORE the worktree/lease is touched.
            let launch =
                session::guided_session_launch(session::resolve_session_vendor(), &display)?;
            let focus = match classify_worktree_arg(&display) {
                WorktreeTarget::Spec { focus, .. } => focus,
                WorktreeTarget::Epic => unreachable!("epics are refused above"),
            };
            let out = ensure_spec_worktree(&display, &focus, None, None, "do-guided")?;
            // The codex prompt is a whole adapted document — summarize it in
            // the launch line instead of dumping it.
            let prompt_note = if launch.prompt.contains('\n') {
                "(adapted guided prompt)".to_string()
            } else {
                format!("{:?}", launch.prompt)
            };
            eprintln!(
                "  {} worktree {} · launching guided session — {} {}",
                crate::glyph(crate::glyphs::Glyph::Arrow).cyan(),
                out.path.display().to_string().cyan(),
                launch.program,
                prompt_note.dimmed()
            );
            let status = std::process::Command::new(launch.program)
                .arg(&launch.prompt)
                .current_dir(&out.path)
                .env("AIDA_SESSION_ROLE", "implementer")
                .status_retrying_etxtbsy()
                .map_err(|e| {
                    anyhow::anyhow!(
                        "failed to launch `{program}` ({e}) — guided mode drives an \
                         interactive agent session and needs the `{program}` CLI on PATH.\n\
                         The worktree is ready at {} — open your agent there and run the \
                         guided-implement workflow by hand.",
                        out.path.display(),
                        program = launch.program
                    )
                })?;
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
            Ok(())
        }
        ExecutionMode::Operator => {
            // The explicit operator checklist — print and STOP. No agent runs.
            println!("\n{} — {}", display.bold(), req.title);
            if !req.description.trim().is_empty() {
                println!();
                for line in req.description.trim().lines() {
                    println!("  {line}");
                }
            }
            println!("\n{}", "Operator checklist:".bold());
            println!("  1. Do the work above yourself — no agent will be launched.");
            println!(
                "  2. Need an isolated workspace? aida worktree enter {display} (takes the \
                 implementer lease)."
            );
            println!(
                "  3. Commit with the ({display}) trailer so the merge auto-completes the spec."
            );
            println!("  4. Finished on a branch? aida queue done {display} · then open the PR.");
            Ok(())
        }
        ExecutionMode::Decide => {
            // Surface the pending decision — no harness runs until answered.
            println!("\n{} — {}", display.bold(), req.title);
            match req.decision_request.as_ref().filter(|d| d.is_pending()) {
                Some(dr) => {
                    println!("\n{}", "Pending decision:".bold());
                    println!("  {}", dr.question);
                    println!("\n  Answer it: aida questions answer");
                }
                None => {
                    println!(
                        "\n  Groomed `decide` but no pending decision request is recorded — \
                         the spec likely needs acceptance criteria."
                    );
                    println!("  Author them interactively: aida clarify {display}");
                    println!(
                        "  Then re-groom: aida groom (or aida edit {display} --mode <m> as \
                         the advisor)."
                    );
                }
            }
            Ok(())
        }
    }
}

/// ADR-15: the TTY micro-groom for an ungroomed `aida do <spec>`. Proposes a
/// mode seeded from the classify heuristic WITH its reasoning line (so the
/// confirm is informed, and a wrong proposal is visibly a finding to file),
/// shows the mode's human contract, asks one confirm, and persists the
/// accepted mode through the normal advisor-authority path (`aida edit
/// --mode` in a child process) — the store commit records who classified it
/// and when, same provenance as a full groom pass. TTY-only: headless callers
/// are refused with the groom pointer.
// trace:STORY-776 | ai:claude
pub(crate) fn do_micro_groom_mode(
    req: &Requirement,
    display: &str,
    store: &aida_core::RequirementsStore,
) -> Result<aida_core::ExecutionMode> {
    use std::io::IsTerminal;
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        anyhow::bail!(
            "{display} is ungroomed — no execution mode set, and classifying one is an \
             advisor act the headless path must not guess. Either run `aida groom` \
             (advisor disposition pass), set it directly (`aida edit {display} --mode <m>` \
             as the advisor), or dispatch explicitly with `aida do {display} --mode <m>`."
        );
    }
    let req_type = req.req_type.to_string().to_lowercase();
    let tags: Vec<String> = req.tags.iter().cloned().collect();
    let blocked_by_dependent = store.requirements.iter().find_map(|candidate| {
        candidate
            .relationships
            .iter()
            .any(|rel| {
                matches!(rel.rel_type, aida_core::RelationshipType::BlockedBy)
                    && rel.target_id == req.id
            })
            .then(|| candidate.display_id())
    });
    let input = do_dispatch::ModeProposalInput {
        req_type: &req_type,
        tags: &tags,
        human_only: req.human_only,
        has_pending_decision: req
            .decision_request
            .as_ref()
            .is_some_and(|d| d.is_pending()),
        under_specified: spec_is_under_specified(req),
        description: &req.description,
        blocked_by_dependent: blocked_by_dependent.as_deref(),
    };
    let proposal = do_dispatch::propose_execution_mode(&input);
    println!("{} is ungroomed — no execution mode set.", display.bold());
    println!(
        "  proposing {}: {}",
        proposal.mode.to_string().cyan().bold(),
        proposal.reason
    );
    println!(
        "  contract: {}",
        do_dispatch::human_contract(proposal.mode).dimmed()
    );
    if !prompt_yes_no("  Accept and record this mode? [Y/n] ", true)? {
        anyhow::bail!(
            "declined — dispatch explicitly with `aida do {display} --mode <m>`, or run \
             `aida groom` for a full advisor pass. (A wrong proposal is worth filing: \
             aida add --type bug)"
        );
    }
    let mode_str = proposal.mode.to_string();
    let status = std::process::Command::new(resolve_aida_exe())
        .args(["edit", display, "--mode", &mode_str])
        .status_retrying_etxtbsy()
        .context("failed to record the confirmed mode via `aida edit --mode`")?;
    if !status.success() {
        anyhow::bail!(
            "recording the confirmed mode failed (`aida edit {display} --mode {mode_str}`) — \
             not dispatching on an unrecorded classification."
        );
    }
    Ok(proposal.mode)
}

/// STORY-721: `aida zen <spec>` — the one-shot AUTONOMOUS implement+ship drive.
///
/// A THIN wrapper over the existing `--auto-complete` orchestrator — the SAME
/// per-spec engine `aida burndown` / `aida integrate` use (ADR-7). It
/// resolves + validates the spec (refusing a not-yet-approved Draft with
/// guidance), then drives the one spec by self-invoking `aida queue work <spec>
/// --auto-complete --no-human <mode>` so the review + merge phases run for free
/// (TASK-1049). The drive's own preflight
/// (`ensure_queued_for_implementer`, STORY-246) auto-queues the spec, so the
/// operator never has to `aida queue add` first. The orchestrator is NOT
/// reimplemented here — the pure pieces (eligibility, argv, plan formatting)
/// live in [`crate::zen_drive`].
// trace:STORY-721 | ai:claude — plain `//` keeps the marker out of any surface.
// trace:TASK-1037 | ai:claude — slice 2 adds the pre-flight gates around the
// slice-1 drive: the autopilot approve-gate for a Draft, the suitability checks
// for an Approved spec, and the ADR-6 scope-routing. The pure decision logic
// lives in `zen_drive.rs`; this handler does the IO (status write, worktree
// creation, the self-invoked drive).
/// `aida zen <spec> --json` — the machine-readable drive-gate PROBE. Resolves
/// the spec, evaluates the SAME eligibility + suitability gate the drive runs
/// (via the pure [`zen_drive::classify_gate`]), and prints the structured
/// verdict as JSON — WITHOUT driving. A shell-out consumer (the TUI drive verb,
/// STORY-744) reads this to surface a gate hold (and its clarify / force remedy)
/// instead of a false "drive launched".
///
/// The probe reports the HONEST, un-forced verdict: it classifies with
/// `force = false` (so a soft hold surfaces as `soft-block` + `forceable`,
/// letting the consumer decide whether to offer a force affordance) and
/// `coupled = false` (coupling only fires on `--solo`, which the probe does not
/// model — the drive it stands in for does not pass it).
// trace:STORY-744 | ai:claude
pub(crate) fn run_zen_gate_json(storage: &Storage, spec: Option<&str>) -> Result<()> {
    let Some(spec) = spec.map(str::trim).filter(|s| !s.is_empty()) else {
        anyhow::bail!(
            "aida zen --json needs a spec to probe. Usage: aida zen <SPEC> --json \
             (emit the drive-gate verdict as JSON without driving)."
        );
    };
    if !zen_drive::looks_like_spec_id(spec) {
        anyhow::bail!(
            "aida zen --json needs a real SPEC id (e.g. TASK-123), not a free-text \
             thought — the gate probe classifies an existing spec."
        );
    }
    let store = storage.load()?;
    let req = store
        .requirements
        .iter()
        .find(|r| spec_matches(r, spec))
        .ok_or_else(|| anyhow::anyhow!("no requirement matches `{spec}`"))?;
    let display = req.display_id();
    let req_type = req.req_type.to_string();
    let tags: Vec<String> = req.tags.iter().cloned().collect();
    let suit_input = zen_drive::SuitabilityInput {
        req_type: &req_type,
        tags: &tags,
        has_unsatisfied_blocker: aida_core::pickability::blocked_by_incomplete(req, &store),
        under_specified: spec_is_under_specified(req),
        coupled: false,
        force: false,
    };
    let mut verdict = zen_drive::classify_gate(&display, &req.status, &suit_input);
    // TASK-1076: also resolve the DEFAULT (no --solo) ADR-6 scope route and stamp
    // it onto the verdict, so the shell-out consumer (the TUI drive verb) can show
    // WHERE the drive would run — into the parent-epic / focus worktree vs solo —
    // and offer a --solo toggle BEFORE launching, instead of silently routing an
    // epic-parented spec into the epic worktree. classify_gate is routing-agnostic
    // (defaults to solo); this fills the real route from the store. trace:TASK-1076
    let parent_epic = resolve_spec_scope(req, &store);
    let active_focus = find_project_root()
        .ok()
        .as_deref()
        .and_then(crate::focus::resolve_focus);
    match zen_drive::resolve_scope_route(
        parent_epic.as_deref(),
        active_focus.as_deref(),
        false,
        false,
    ) {
        zen_drive::ScopeRoute::IntoScope(scope) => {
            verdict.route = "into-scope";
            verdict.scope = scope;
        }
        zen_drive::ScopeRoute::Solo => {
            verdict.route = "solo";
        }
    }
    println!("{}", serde_json::to_string(&verdict)?);
    Ok(())
}

/// TASK-1116: apply a drive command's `--vendor`/`--agent` value as the
/// top-precedence headless-vendor override for this invocation (both the
/// in-process tier and the `AIDA_HEADLESS_VENDOR` env a child-subprocess drain
/// inherits — see [`session::install_headless_vendor_override`]). An
/// unrecognized token is a hard error (no silent misroute); the recognized set
/// is `claude` / `codex` / `agy`. A `None` flag is a no-op, so an un-flagged
/// drive resolves exactly as before TASK-1116.
// trace:TASK-1116 TASK-1048 | ai:claude
pub(crate) fn apply_drive_vendor_override(vendor: Option<&str>) -> Result<()> {
    if let Some(raw) = vendor {
        if session::install_headless_vendor_override(raw).is_none() {
            anyhow::bail!("unknown --vendor/--agent value `{raw}` (expected: claude, codex, agy)");
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_zen_drive(
    storage: &Storage,
    store_path: Option<&std::path::Path>,
    user_id: &str,
    spec: Option<&str>,
    no_human: Option<&str>,
    supervised: bool,
    no_pull: bool,
    force: bool,
    solo: bool,
    into_epic: bool,
    dry_run: bool,
) -> Result<()> {
    let Some(spec) = spec.map(str::trim).filter(|s| !s.is_empty()) else {
        anyhow::bail!(
            "aida zen needs a spec to drive. Usage: aida zen <SPEC> \
             (the spec to autonomously implement and ship). \
             Add --dry-run to preview the plan without driving it."
        );
    };

    // THOUGHT → spec front door (STORY-725). A positional that is NOT a spec id
    // is free text: draft a spec from it, file it as a draft, and drive THAT new
    // spec through the same status-routing + approve-gate path below. A real
    // spec id keeps the existing resolve-or-refuse behavior.
    // trace:STORY-725 | ai:claude
    let drafted_spec: String;
    let spec: &str = if zen_drive::looks_like_spec_id(spec) {
        spec
    } else {
        if dry_run {
            // --dry-run files nothing — but the whole pitch of zen is "a sentence
            // becomes a structured, shipped spec", so the SAFE preview composes the
            // SAME draft the real run would file (AI, with an offline fallback) and
            // pretty-prints it in the `aida show` idiom — title + description +
            // acceptance — instead of echoing the thought back. Composes + renders
            // only; nothing is persisted. trace:STORY-736
            print!("{}", render_zen_dry_run_draft(spec));
            return Ok(());
        }
        // The drafted spec is born a Draft and routes through the autopilot
        // approve-gate below, which needs git-canonical storage; the legacy
        // (centralized) path can't run it, so refuse early with guidance.
        if store_path.is_none() {
            anyhow::bail!(
                "aida zen \"<thought>\" (free-text drafting) needs git-canonical storage. \
                 File the spec first (aida add ...), then run aida zen <SPEC>."
            );
        }
        let (new_id, source) = zen_file_thought_as_draft(storage, spec)?;
        eprintln!(
            "  {} drafted {} from your thought ({}) — filed as a draft; routing it through the approve-gate.",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            new_id,
            source.label()
        );
        drafted_spec = new_id;
        &drafted_spec
    };

    // Resolve against the store. The spec must exist; status routes it.
    let store = storage.load()?;
    let req = store
        .requirements
        .iter()
        .find(|r| spec_matches(r, spec))
        .ok_or_else(|| anyhow::anyhow!("no requirement matches `{spec}`"))?;
    let display = req.display_id();
    let req_type = req.req_type.to_string();
    let tags: Vec<String> = req.tags.iter().cloned().collect();
    let project_root = find_project_root().ok();

    let arrow = crate::glyph(crate::glyphs::Glyph::Arrow);
    let check = crate::glyph(crate::glyphs::Glyph::Check);
    let warn = crate::glyph(crate::glyphs::Glyph::Warning);

    // ── Status routing ────────────────────────────────────────────────────
    // Terminal states (already shipped / shelved / rejected) keep the slice-1
    // guidance. A Draft runs the autopilot approve-gate; Approved-or-beyond
    // falls straight through to the suitability gate.
    match zen_drive::classify_eligibility(&req.status) {
        zen_drive::ZenEligibility::AlreadyShipped
        | zen_drive::ZenEligibility::Shelved
        | zen_drive::ZenEligibility::Rejected
        // trace:TASK-1176 | ai:claude
        | zen_drive::ZenEligibility::Superseded => {
            if let Some(msg) = zen_drive::classify_eligibility(&req.status).refusal(&display) {
                anyhow::bail!(msg);
            }
        }
        zen_drive::ZenEligibility::NeedsApproval => {
            // DRAFT → autopilot approve-gate (TASK-1037). The first real
            // consumer of the merged `autopilot::evaluate` contract.
            let Some(store_path) = store_path else {
                // Legacy (centralized) storage can't run the gate or write the
                // status flip — keep the slice-1 refusal there.
                anyhow::bail!(zen_drive::ZenEligibility::NeedsApproval
                    .refusal(&display)
                    .unwrap_or_else(|| format!("{display} is a draft")));
            };
            let env = load_autopilot_envelope(project_root.as_deref());
            match zen_drive::run_draft_gate(&env, &display, &req_type, &tags) {
                zen_drive::DraftGate::AutoApprove
                    if !zen_auto_approve_authorized(&req.status, has_advisor_authority()) =>
                {
                    // BUG-1611: `approve = "auto"` without approval authority
                    // routes to the advisor exactly like the propose policy —
                    // the knob cannot approve on its own. trace:BUG-1611 | ai:claude
                    println!(
                        "{display} is a draft — auto-approval needs approval authority this \
                         session does not hold; routed to the advisor for approval."
                    );
                    if !dry_run {
                        if let Some(pr) = project_root.as_deref() {
                            zen_surface_to_advisor(pr, &store, req, &display);
                        }
                    }
                    println!(
                        "  Once an advisor approves it (aida edit {display} --status approved), \
                         re-run aida zen {display}."
                    );
                    return Ok(());
                }
                zen_drive::DraftGate::AutoApprove => {
                    if dry_run {
                        println!(
                            "{display} is a draft — the autopilot approve-gate would \
                             auto-approve it (status → Approved) and drive."
                        );
                    } else {
                        zen_auto_approve(store_path, req, has_advisor_authority())?;
                        // TASK-1018: the ONLY wired `Execute` outcome today —
                        // so it gets the durable record + the one-command
                        // reversal, right where the side effect lands. Prior
                        // state is the Draft status the gate flipped off.
                        record_autopilot_execution(
                            project_root.as_deref(),
                            &display,
                            "zen",
                            autopilot_audit::PriorState::from_status("draft"),
                        );
                        eprintln!(
                            "  {} {display} was a draft — the autopilot approve-gate \
                             auto-approved it; driving. Undo with `aida autopilot revert {display}`.",
                            check.green()
                        );
                    }
                    // Fall through to the suitability gate + drive.
                }
                zen_drive::DraftGate::SurfaceToAdvisor => {
                    println!("{display} is a draft — routed to the advisor for approval.");
                    if let Some(pr) = project_root.as_deref() {
                        zen_surface_to_advisor(pr, &store, req, &display);
                    }
                    println!(
                        "  Approve it (aida edit {display} --status approved) when it's \
                         ready, then re-run aida zen {display}."
                    );
                    return Ok(()); // exit cleanly — no block, no pause.
                }
                zen_drive::DraftGate::Escalate(reason) => {
                    anyhow::bail!(
                        "aida zen will not drive {display}: {reason}. Triage it manually first."
                    );
                }
            }
        }
        zen_drive::ZenEligibility::Ready => {}
    }

    // ── Scope-routing decision (ADR-6) ────────────────────────────────────
    // A scoped spec (parent epic, else active focus) routes into its scope
    // worktree; --solo splits it out; --into-epic forces the cluster route.
    let parent_epic = resolve_spec_scope(req, &store);
    let active_focus = project_root
        .as_deref()
        .and_then(crate::focus::resolve_focus);
    let route = zen_drive::resolve_scope_route(
        parent_epic.as_deref(),
        active_focus.as_deref(),
        solo,
        into_epic,
    );

    // ── Suitability gate ──────────────────────────────────────────────────
    // Coupling fires only on the override-into-collision case (ADR-6): --solo
    // splitting out of a scope that has in-flight work.
    let scope = parent_epic.as_deref().or(active_focus.as_deref());
    let coupled = solo
        && scope
            .map(|s| scope_has_in_flight_work(&store, s, req))
            .unwrap_or(false);
    let suit = zen_drive::classify_suitability(&zen_drive::SuitabilityInput {
        req_type: &req_type,
        tags: &tags,
        has_unsatisfied_blocker: aida_core::pickability::blocked_by_incomplete(req, &store),
        under_specified: spec_is_under_specified(req),
        coupled,
        force,
    });
    match suit {
        zen_drive::Suitability::HardRefuse(msg) => {
            anyhow::bail!("aida zen will not drive {display}: {msg}")
        }
        zen_drive::Suitability::SoftBlock(msg) => {
            anyhow::bail!("aida zen is holding {display}: {msg}.")
        }
        zen_drive::Suitability::WarnProceed(msg) => {
            eprintln!(
                "  {} {display}: {msg} — proceeding (--force).",
                warn.yellow()
            );
        }
        zen_drive::Suitability::Ready => {}
    }

    // Is it already queued for the implementer? (Informational only — the
    // drive auto-queues either way; this just makes the plan honest.)
    let already_queued = storage
        .queue_list(user_id, false)
        .map(|entries| entries.iter().any(|e| e.requirement_id == req.id))
        .unwrap_or(false);

    if dry_run {
        print!(
            "{}",
            zen_drive::format_zen_plan(&display, already_queued, no_human, supervised)
        );
        match &route {
            zen_drive::ScopeRoute::IntoScope(epic) => {
                println!("  scope: routes into the {epic} worktree (ADR-6) — --solo to split out.")
            }
            zen_drive::ScopeRoute::Solo => println!("  scope: own worktree + own PR."),
        }
        return Ok(());
    }

    // Route into the scope worktree (create if absent) when the spec is scoped
    // and not --solo. An info line, not a question (ADR-6). On failure we fall
    // back to the current worktree rather than abort.
    let drive_cwd: Option<std::path::PathBuf> = match &route {
        zen_drive::ScopeRoute::IntoScope(epic) => match ensure_epic_worktree(epic, None, None) {
            Ok(out) => {
                eprintln!(
                    "  {} routing {display} into the {} scope worktree ({}) — ADR-6; --solo to split out.",
                    arrow.cyan(),
                    out.focus,
                    out.path.display().to_string().cyan()
                );
                Some(out.path)
            }
            Err(e) => {
                eprintln!(
                    "  {} could not prepare the {epic} scope worktree ({e}) — driving in the current worktree.",
                    warn.yellow()
                );
                None
            }
        },
        zen_drive::ScopeRoute::Solo => None,
    };

    // Drive the one spec through the EXISTING orchestrator by self-invoking
    // `aida queue work <spec> --auto-complete --no-human <mode> [...]` — the
    // SAME full per-spec engine burndown uses (ADR-7), so the independent
    // reviewer + merge run for free (TASK-1049). resolve_aida_exe() (not raw
    // a fresh OS executable lookup) survives a mid-run `cargo build` swap. The drive owns the
    // implement → CI → review → merge → pull sequence.
    let exe = resolve_aida_exe();
    let args = zen_drive::drive_args(&display, no_human, supervised, no_pull);
    eprintln!(
        "  {} zen-driving {} — aida {}",
        arrow.cyan(),
        display,
        args.join(" ").dimmed()
    );
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(&args);
    if let Some(cwd) = &drive_cwd {
        cmd.current_dir(cwd);
    }
    let status = cmd
        .status_retrying_etxtbsy()
        .context("failed to launch the `aida queue work --auto-complete --no-human` drive")?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// THOUGHT → spec front door (STORY-725): draft a spec from a free-text thought
/// and file it as a Draft, returning its new display id + which path drafted the
/// body. Reuses the same programmatic filing path as `file_task_for_schedule`
/// (load → build → `add_requirement_with_id` → save), so it works on the
/// git-canonical store. The new spec is born a Draft so the caller's autopilot
/// approve-gate decides whether to drive it.
// trace:STORY-725 | ai:claude
pub(crate) fn zen_file_thought_as_draft(
    storage: &Storage,
    thought: &str,
) -> Result<(String, zen_drive::DraftSource)> {
    let ai = zen_try_ai_draft(thought);
    let drafted = zen_drive::compose_draft_from_thought(thought, ai);
    file_drafted_thought(storage, drafted)
}

/// File an already-composed [`zen_drive::DraftedThought`] as a Draft and return
/// its new display id + source. Split from the AI-wiring above so the filing IO
/// is unit-testable without the AI transport.
// trace:STORY-725 | ai:claude
pub(crate) fn file_drafted_thought(
    storage: &Storage,
    drafted: zen_drive::DraftedThought,
) -> Result<(String, zen_drive::DraftSource)> {
    let mut store = storage.load()?;
    let mut req = Requirement::new(drafted.title.clone(), drafted.description.clone());
    req.req_type = RequirementType::Task;
    req.status = RequirementStatus::Draft;
    req.owner = get_default_author();
    req.tags.insert("aida:zen".to_string());
    req.tags.insert("from-thought".to_string());

    let id = req.id;
    let type_prefix = store.get_type_prefix(&req.req_type);
    store.add_requirement_with_id(req, None, type_prefix.as_deref());
    storage.save(&store)?;

    let spec_id = store
        .get_requirement_by_id(&id)
        .and_then(|r| r.spec_id.clone())
        .unwrap_or_else(|| id.to_string());
    Ok((spec_id, drafted.source))
}

/// Best-effort genuine AI draft of a thought via the EXISTING aida-core
/// [`aida_core::AiClient`] path (Claude CLI). Returns `None` — so the caller
/// uses the verbatim fallback — when the AI is unreachable, when the request
/// fails, or when `AIDA_ZEN_NO_AI=1` forces the fallback (tests / offline).
// trace:STORY-725 | ai:claude
pub(crate) fn zen_try_ai_draft(thought: &str) -> Option<aida_core::DraftSpecResponse> {
    if std::env::var("AIDA_ZEN_NO_AI").ok().as_deref() == Some("1") {
        return None;
    }
    let client = aida_core::AiClient::new();
    if !client.is_available() {
        return None;
    }
    client.draft_spec(thought).ok()
}

/// Load the autopilot policy envelope from `[autopilot]` in `.aida/config.toml`,
/// falling back to the conservative [`autopilot::AutopilotEnvelope::default`]
/// (approve = propose) when there is no project root or no config.
// trace:TASK-1037
pub(crate) fn load_autopilot_envelope(
    project_root: Option<&std::path::Path>,
) -> crate::autopilot::AutopilotEnvelope {
    let body = project_root
        .map(|pr| std::fs::read_to_string(pr.join(".aida").join("config.toml")).unwrap_or_default())
        .unwrap_or_default();
    let overrides = crate::autopilot::parse_authority_overrides(&body);
    crate::autopilot::AutopilotEnvelope::default().with_overrides(overrides)
}

/// Auto-approve a Draft spec (status → Approved) in-process via the git
/// backend, the way `aida groom --apply` / the answer path do. Only reached when
/// the autopilot approve-gate returned `Execute`.
// trace:TASK-1037 | ai:claude
pub(crate) fn zen_auto_approve(
    store_path: &std::path::Path,
    req: &Requirement,
    has_advisor_authority: bool,
) -> Result<()> {
    // BUG-1611: defense in depth at the write itself — the approval authority
    // predicate runs here too, so no future caller can reach the flip on the
    // autopilot policy alone. trace:BUG-1611 | ai:claude
    if !zen_auto_approve_authorized(&req.status, has_advisor_authority) {
        anyhow::bail!(
            "{} is {}: auto-approval needs approval authority. Ask an advisor to approve it.",
            req.display_id(),
            req.status
        );
    }
    let dispenser = load_dispenser(store_path)?;
    let inner = aida_core::GitBackend::new(store_path)?.with_dispenser(dispenser);
    let cache_path = aida_core::CachedGitBackend::default_cache_path(store_path);
    let backend = aida_core::CachedGitBackend::with_inner(inner, &cache_path)?;
    // BUG-1637: the flip (and its authority and Draft checks) runs on the copy
    // read under the store lock; a refused flip writes nothing and fails, so
    // no autopilot execution is recorded for it. trace:BUG-1637 | ai:claude
    let mut approved = false;
    let mut seen = req.status.clone();
    let written = backend.update_spec_atomically(req, |r| {
        seen = r.status.clone();
        approved =
            zen_auto_approve_authorized(&r.status, has_advisor_authority) && zen_approve_flip(r);
    })?;
    // BUG-1638: a spec deleted meanwhile is reported as gone, not as "is now
    // Draft, not Draft" (the closure never ran). trace:BUG-1638 | ai:claude
    if written.is_none() {
        anyhow::bail!(
            "{} no longer exists: it was deleted while auto-approving, so nothing was changed.",
            req.display_id()
        );
    }
    if !approved {
        anyhow::bail!(
            "{} is now {}, not Draft: it changed while auto-approving, so nothing was changed.",
            req.display_id(),
            seen
        );
    }
    Ok(())
}

/// TASK-1018: durably record an autopilot action that just EXECUTED, so it has
/// both an audit trail and a one-command reversal (`aida autopilot revert`).
///
/// Call this immediately AFTER the side effect lands, never before — the record
/// asserts "this happened". Best-effort: the action already succeeded, so a
/// missing project root or an unwritable log warns rather than failing the
/// command (and the warning is loud, because an unaudited execution is exactly
/// the state this spec exists to prevent).
// trace:TASK-1018 | ai:claude
pub(crate) fn record_autopilot_execution(
    project_root: Option<&std::path::Path>,
    display: &str,
    source: &str,
    prior: autopilot_audit::PriorState,
) {
    let Some(root) = project_root else {
        eprintln!("  warning: no project root — the autopilot action was not audited.");
        return;
    };
    // The zen approve-gate's decision, re-stated for the record: the operator's
    // explicit invocation IS the recorded preference (RecordedB), approving one
    // named draft is Low risk, and it cleared all four gates under `auto`.
    let decision = zen_drive::draft_approve_decision(display);
    let actor = current_user_id(None);
    if let Err(e) = autopilot_audit::record_execution(
        root,
        &decision,
        crate::autopilot::Outcome::Execute,
        crate::autopilot::Authority::Auto,
        &actor,
        source,
        prior,
    ) {
        eprintln!("  warning: the autopilot action was not audited: {e}");
    }
}

/// Record a pending-approval brief in the advisor's mailbox when a Draft zen
/// request is held for approval. Best-effort — a brief-write failure never
/// breaks the command (the surface line was already printed). Idempotent via the
/// pending-brief sentinel.
// trace:TASK-1037 | ai:claude
pub(crate) fn zen_surface_to_advisor(
    project_root: &std::path::Path,
    store: &RequirementsStore,
    req: &Requirement,
    display: &str,
) {
    let agent = "advisor";
    let spec_id = req.spec_id.as_deref().unwrap_or(display);
    if pending_brief_exists(project_root, agent, spec_id) {
        return;
    }
    let note = format!(
        "aida zen was requested on draft {display}. Approve it \
         (aida edit {display} --status approved) if it is ready to implement, \
         then re-run aida zen {display}."
    );
    if let Ok(path) =
        create_agent_brief(project_root, store, agent, spec_id, Some(&note), None, None)
    {
        eprintln!(
            "  {} recorded a pending-approval brief for the advisor: {}",
            crate::glyph(crate::glyphs::Glyph::Check),
            path.display()
        );
    }
}

/// True when the spec is GENUINELY under-specified for an autonomous drive —
/// its description + acceptance carry essentially no specifiable content
/// (the EARS `EmptyBody` category).
///
/// BUG-708: this used to gate on `!lint_text(text).is_clean()` — i.e. ANY EARS
/// finding, including the *optional* clarity nits (`MissingBehavior`,
/// `VagueTrigger`, `LowTestability`). `aida lint` presents those as drafts
/// ("never auto-applied"), so a well-formed trivial doc task with a real
/// `## Acceptance` section was blocked purely for lacking a "THE SYSTEM SHALL"
/// response clause — which a doc task never has — and the refusal mislabelled
/// it "missing acceptance". The drive gate now fires ONLY on the genuine
/// under-specification signal (empty body / no specifiable content); the
/// stylistic categories stay advisory-only, surfaced by `aida lint`.
// trace:BUG-708 (supersedes TASK-1037) | ai:claude
pub(crate) fn spec_is_under_specified(req: &Requirement) -> bool {
    let mut text = req.description.clone();
    if let Some(acc) = req.custom_fields.get("acceptance_criteria") {
        text.push('\n');
        text.push_str(acc);
    }
    aida_core::ears_lint::lint_text(&text).count(aida_core::ears_lint::Category::EmptyBody) > 0
}

#[cfg(test)]
#[path = "tests/bug708_under_specified_tests.rs"]
mod bug708_under_specified_tests;

/// Resolve a spec's scope epic: walk its `parent:<ID>` tag chain (bounded) to
/// the nearest ancestor of type Epic. Returns its display id, or `None` when no
/// epic ancestor is reachable.
// trace:ADR-6 trace:TASK-1037 | ai:claude
pub(crate) fn resolve_spec_scope(req: &Requirement, store: &RequirementsStore) -> Option<String> {
    let mut current = req;
    let mut seen = std::collections::HashSet::new();
    seen.insert(current.id);
    for _ in 0..16 {
        let parent_ref = current
            .tags
            .iter()
            .find_map(|t| t.strip_prefix("parent:").map(|p| p.trim().to_string()))?;
        let parent = store
            .requirements
            .iter()
            .find(|r| spec_matches(r, &parent_ref))?;
        if !seen.insert(parent.id) {
            break;
        }
        if matches!(parent.req_type, RequirementType::Epic) {
            return Some(parent.display_id());
        }
        current = parent;
    }
    None
}

/// Does the scope (epic / focus) have OTHER in-flight work right now? Used for
/// the ADR-6 override-into-collision warning: a spec directly tagged
/// `parent:<scope>` that is In Progress.
// trace:ADR-6 trace:TASK-1037 | ai:claude
pub(crate) fn scope_has_in_flight_work(
    store: &RequirementsStore,
    scope: &str,
    exclude: &Requirement,
) -> bool {
    store.requirements.iter().any(|r| {
        r.id != exclude.id
            && matches!(r.status, RequirementStatus::InProgress)
            && r.tags.iter().any(|t| {
                t.strip_prefix("parent:")
                    .map(|p| p.trim().eq_ignore_ascii_case(scope))
                    .unwrap_or(false)
            })
    })
}

/// Queue `spec` for the implementer role if it isn't already queued for the
/// current user — the preflight that lets `--auto-complete` accept a
/// freshly-added spec. trace:STORY-246 | ai:claude
pub(crate) fn ensure_queued_for_implementer(
    storage: &Storage,
    user_id: &str,
    spec: &str,
) -> Result<()> {
    let store = storage.load()?;
    let req = store
        .requirements
        .iter()
        .find(|r| spec_matches(r, spec))
        .ok_or_else(|| anyhow::anyhow!("no requirement matches `{spec}`"))?;
    let already = storage
        .queue_list(user_id, false)?
        .iter()
        .any(|e| e.requirement_id == req.id);
    if already {
        return Ok(());
    }
    eprintln!(
        "  {} {} is not queued — queueing it for the implementer",
        crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
        spec
    );
    let exe = aida_exe_path();
    let status = std::process::Command::new(exe)
        .args(auto_complete_queue_add_args(spec))
        .status_retrying_etxtbsy()
        .context("failed to run `aida queue add`")?;
    if !status.success() {
        anyhow::bail!("`aida queue add {spec} --for implementer --no-scope` failed");
    }
    Ok(())
}

pub(crate) fn auto_complete_phase1_target_status(
    status: &RequirementStatus,
) -> Option<RequirementStatus> {
    match status {
        RequirementStatus::Draft | RequirementStatus::Approved | RequirementStatus::Planned => {
            Some(RequirementStatus::InProgress)
        }
        _ => None,
    }
}

/// Registration and the status bump share one checked entry point, so an
/// ownership failure cannot leave Approved work InProgress without a lease.
// trace:TASK-1603 | ai:codex
fn prepare_registered_auto_complete_phase1(
    storage: &Storage,
    project_root: &std::path::Path,
    spec: &str,
    run_token: &str,
    zen: bool,
    owns_drain_state: bool,
) -> Result<Option<(String, RequirementStatus)>> {
    drain_state::register_run(project_root, spec, run_token, zen, owns_drain_state)?;
    let result = prepare_auto_complete_phase1_status(storage, spec);
    if result.is_err() {
        drain_state::clear_run(project_root, run_token);
    }
    result
}

/// BUG-369: mark orchestrator-driven phase-1 work as InProgress before the
/// implementer subprocess starts. `aida punt` correctly allows only
/// InProgress → NeedsAttention; without this pre-spawn flip, an early design
/// fork on an Approved/Planned/Draft spec made `/aida-punt` refuse and the
/// orchestrator misclassified the clean exit as NoPR.
pub(crate) fn prepare_auto_complete_phase1_status(
    storage: &Storage,
    spec: &str,
) -> Result<Option<(String, RequirementStatus)>> {
    let store = storage.load()?;
    let req = store
        .requirements
        .iter()
        .find(|r| spec_matches(r, spec))
        .ok_or_else(|| anyhow::anyhow!("no requirement matches `{spec}`"))?;
    let display_id = req.display_id();
    let current = req.status.clone();
    let Some(target) = auto_complete_phase1_target_status(&current) else {
        return Ok(None);
    };
    let now = chrono::Utc::now();
    // Per-spec compare-and-swap, no whole-store write. trace:BUG-1612 | ai:claude
    // BUG-1637: re-check the source status inside the write (a concurrent
    // Rejected/Completed must not become In Progress), record the bump under
    // the orchestrator's phase-1 author, and take the restore target from the
    // status read under the lock, not the pre-lock read.
    // trace:BUG-1637 | ai:claude
    let mut prior: Option<RequirementStatus> = None;
    storage.update_spec_atomically(req, |r| {
        let before = r.status.clone();
        if phase1_status_bump(r, &target, now) {
            prior = Some(before);
        }
    })?;
    Ok(prior.map(|p| (display_id, p)))
}

pub(crate) fn resolve_lifecycle_skip(
    storage: &Storage,
    spec: &str,
) -> Result<auto_complete::LifecycleSkip> {
    let store = storage.load()?;
    let req = store
        .requirements
        .iter()
        .find(|r| spec_matches(r, spec))
        .ok_or_else(|| anyhow::anyhow!("no requirement matches `{spec}`"))?;
    Ok(auto_complete::LifecycleSkip::from_tags(
        req.tags.iter().map(String::as_str),
    ))
}

/// TASK-827: classify this spec as keystone/architecture-class for the solo
/// posture. Best-effort — if the spec can't be loaded we treat it as
/// non-keystone so the posture errs toward the cheap error (a safe spec parked
/// or proceeded), never the expensive one (shipping keystone unattended is
/// guarded separately by the `supervised` drain exclusion). Returns the
/// `(req_type, tags)` of a conservative classification via
/// `presence::is_keystone_class`. trace:TASK-827 | ai:claude
pub(crate) fn solo_spec_is_keystone(storage: &Storage, spec: &str) -> bool {
    let Ok(store) = storage.load() else {
        return false;
    };
    let Some(req) = store.requirements.iter().find(|r| spec_matches(r, spec)) else {
        return false;
    };
    presence::is_keystone_class(
        &req.req_type.to_string(),
        req.tags.iter().map(String::as_str),
    )
}

/// Best-effort lookup of the most recent workflow run id for `branch`, used
/// to enrich the CI-failure recovery hint. trace:STORY-246 | ai:claude
pub(crate) fn latest_run_id_for_branch(branch: &str) -> Option<String> {
    let gh = resolve_gh_binary()?;
    let out = std::process::Command::new(gh)
        .args([
            "run",
            "list",
            "--branch",
            branch,
            "--limit",
            "1",
            "--json",
            "databaseId",
        ])
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    first_run_id_from_gh_json(&String::from_utf8_lossy(&out.stdout))
}

/// Build the red-CI recovery route from the exact rows that caused the gate
/// to fail. In particular, never perform a second branch-level run lookup:
/// another attached workflow may be newer and have colliding job names.
// trace:BUG-1298 | ai:codex
pub(crate) fn ci_red_recovery_hint(
    forge: crate::forge::ForgeKind,
    spec: &str,
    branch: &str,
    failed: &[ci_gate::CheckRow],
) -> String {
    let subjects = failed
        .iter()
        .map(|row| match row.run_id.as_deref() {
            Some(run) => forge
                .ci_view_cmd(run)
                .map(|cmd| format!("{} (run {run}; `{cmd}`)", row.name))
                .unwrap_or_else(|| format!("{} (run {run})", row.name)),
            None => format!("{} (owning run unavailable)", row.name),
        })
        .collect::<Vec<_>>()
        .join("; ");
    let fallback = match forge {
        crate::forge::ForgeKind::GitHub => format!("`gh run list --branch {branch}`"),
        crate::forge::ForgeKind::GitLab => "`glab ci status`".to_string(),
        crate::forge::ForgeKind::None => "your CI dashboard".to_string(),
    };
    let route = if subjects.is_empty() {
        format!("CI failed — inspect {fallback}")
    } else {
        format!("CI failed: {subjects}")
    };
    format!(
        "{route}. Push fixups to the same branch: `aida queue work {spec} --branch {branch} --steal`"
    )
}

#[cfg(test)]
mod bug_1298_tests {
    use super::*;

    #[test]
    fn ci_red_hint_uses_failing_required_checks_own_run_not_newer_colliding_workflow() {
        let rows = ci_gate::parse_check_rows(
            r#"[
                {"name":"Build (ubuntu-latest)","workflow":"CI","bucket":"fail","link":"https://github.com/acme/repo/actions/runs/35471997271/job/1"},
                {"name":"Build (windows-latest)","workflow":"Cross-platform (nightly)","bucket":"pending","link":"https://github.com/acme/repo/actions/runs/35471997274/job/2"}
            ]"#,
        )
        .unwrap();
        let refined = ci_gate::classify_red(
            &rows,
            &["Build (ubuntu-latest)".to_string()],
            &ci_gate::CiGateConfig::default(),
            false,
            false,
        );
        let hint = ci_red_recovery_hint(
            crate::forge::ForgeKind::GitHub,
            "BUG-1231",
            "bug-1231-work",
            &refined.real,
        );

        assert!(hint.contains("Build (ubuntu-latest)"), "{hint}");
        assert!(hint.contains("gh run view 35471997271"), "{hint}");
        assert!(!hint.contains("35471997274"), "{hint}");
        assert!(!hint.contains("Build (windows-latest)"), "{hint}");
    }
}

/// Read + parse a `.aida/review-verdicts/PR-N.json` verdict file written by
/// the `/aida-review` skill.
///
/// Returns [`auto_complete::ReviewerOutcome::EscalatedToHuman`] when the file
/// carries `merge: escalated-to-human` — the reviewer wrote a verdict but
/// escalated the *merge* decision to a human rather than auto-deciding it
/// (STORY-306) — and [`auto_complete::ReviewerOutcome::Verdict`] otherwise.
/// The `merge` field is dominant: an escalation is honoured whatever the
/// `verdict` field says, so the phase-3 handshake artifact always parses.
/// trace:STORY-246, STORY-306 | ai:claude
/// BUG-806: phase-3 fallback — when the PR-keyed handshake file is absent,
/// accept the SPEC-keyed recorded verdict (`aida review record <SPEC>`) if it
/// was written DURING this reviewer session (file mtime >= session start).
///
/// Five consecutive phase-3 shelves proved the narrow polling wrong: on the
/// fifth, the reviewer ran the record verb with a correct verdict and the
/// orchestrator still shelved because only `PR-N.json` was consulted. The
/// orchestrator drives a known spec; refusing first-class verdict state it can
/// already see is the substrate losing information it holds. The mtime gate
/// means a verdict recorded by an EARLIER review can never advance a later
/// diff, and the PR-keyed file remains the primary contract — this runs only
/// on a miss.
// trace:BUG-806 | ai:claude
pub(crate) fn spec_verdict_fallback_for_phase3(
    project_root: &std::path::Path,
    spec: &str,
    reviewer_started_at: std::time::SystemTime,
    current_head: Option<&str>,
) -> Result<Option<auto_complete::ReviewerOutcome>, auto_complete::PhaseFailure> {
    let path = review_verdict::verdict_path(project_root, spec);
    let Some(mtime) = std::fs::metadata(&path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
    else {
        return Ok(None);
    };
    if mtime < reviewer_started_at {
        return Ok(None); // stale: recorded by some earlier review, not this one
    }
    // TASK-1460: a later round at another commit may have replaced the spec
    // record; the archived round for THIS head (fresh this session) still
    // answers. The current file is consulted first and its error stands when
    // no such archive exists.
    // trace:TASK-1460 | ai:claude
    let outcome = match read_verdict_file_for_head(&path, current_head) {
        Ok(outcome) => outcome,
        Err(e) => {
            phase3_head_archive_fallback(&path, current_head, reviewer_started_at).ok_or(e)?
        }
    };
    eprintln!(
        "  {} no PR-keyed verdict file, but the reviewer recorded {} for {} during this session — accepting it",
        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
        match &outcome {
            auto_complete::ReviewerOutcome::Verdict(verdict) => verdict.label(),
            auto_complete::ReviewerOutcome::EscalatedToHuman { .. } => "ESCALATED TO HUMAN",
        },
        spec
    );
    Ok(Some(outcome))
}

/// BUG-809: last-ditch verdict discovery when both the PR-keyed file and the
/// spec-keyed record are missing from the drive root — sweep SIBLING
/// directories of the drive root for a verdict this reviewer wrote into its
/// own checkout. The env anchor (`AIDA_DRIVE_ROOT` + PATH, BUG-802/BUG-806)
/// does not reliably survive a vendor's tool-call sandbox, so a reviewer that
/// checked the PR out next to the repo and ran `aida review record` from
/// inside the checkout lands both files there. The PR number is unique and
/// the mtime gate (>= reviewer session start) makes stale pickup impossible,
/// so accepting the freshest fresh candidate is safe regardless of what the
/// reviewer named its checkout. A found PR-keyed file is copied back into the
/// drive root's verdict dir so the calibration tag-along and the audit trail
/// see the canonical location.
// trace:BUG-809 | ai:claude
pub(crate) fn sibling_verdict_sweep_for_phase3(
    project_root: &std::path::Path,
    pr: u32,
    spec: &str,
    reviewer_started_at: std::time::SystemTime,
    current_head: Option<&str>,
) -> Result<Option<auto_complete::ReviewerOutcome>, auto_complete::PhaseFailure> {
    let Some(root_canon) = project_root.canonicalize().ok() else {
        return Ok(None);
    };
    let Some(parent) = root_canon.parent() else {
        return Ok(None);
    };
    // Collect fresh candidates: (mtime, path, is_pr_keyed), freshest first.
    let mut candidates: Vec<(std::time::SystemTime, std::path::PathBuf, bool)> = Vec::new();
    let Some(entries) = std::fs::read_dir(parent).ok() else {
        return Ok(None);
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        if dir.canonicalize().ok().as_deref() == Some(root_canon.as_path()) {
            continue; // the drive root itself — already consulted
        }
        for (name, is_pr) in [
            (format!("PR-{pr}.json"), true),
            (format!("{spec}.json"), false),
        ] {
            let cand = dir.join(".aida").join("review-verdicts").join(&name);
            let Some(mtime) = std::fs::metadata(&cand)
                .ok()
                .and_then(|m| m.modified().ok())
            else {
                continue;
            };
            if mtime >= reviewer_started_at {
                candidates.push((mtime, cand, is_pr));
            }
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let Some((_, freshest, _is_pr)) = candidates.first() else {
        return Ok(None);
    };
    // Every fresh artifact is evidence. Reconcile the complete set rather
    // than allowing whichever file has the newest mtime to hide an opposing
    // same-head review.
    // trace:BUG-1581 | ai:codex
    let mut bodies = candidates
        .iter()
        .map(|(_, path, _)| {
            std::fs::read_to_string(path).map_err(|e| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::NoVerdict,
                    format!(
                        "could not read sibling verdict evidence at {}: {e}",
                        path.display()
                    ),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let reconcile = |evidence: &[String], current_head: &str| {
        review_verdict::reconcile_artifacts_for_sha(
            evidence.iter().map(String::as_str),
            current_head,
        )
        .map_err(|reason| {
            auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::NoVerdict,
                format!("{reason} — sibling review evidence cannot be proven safe"),
            )
        })
    };
    let mut outcome = if let Some(current_head) = current_head {
        match reconcile(&bodies, current_head)? {
            Some(review_verdict::VerdictKind::Approved) => {
                auto_complete::ReviewerOutcome::Verdict(auto_complete::Verdict::Approved)
            }
            Some(review_verdict::VerdictKind::RequestChanges)
            | Some(review_verdict::VerdictKind::Rejected) => {
                auto_complete::ReviewerOutcome::Verdict(auto_complete::Verdict::RequestChanges)
            }
            Some(review_verdict::VerdictKind::Unknown) | None => {
                return Err(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::NoVerdict,
                    "fresh sibling verdict evidence does not contain a verdict for the current PR head",
                ));
            }
        }
    } else {
        // Preserve the legacy fail-safe use case: a refusal does not need a
        // forge head to block. Approvals still fail in
        // `read_verdict_file_for_head`, and every fresh file is consulted.
        let mut blocking = None;
        for (_, path, _) in &candidates {
            let candidate = read_verdict_file_for_head(path, None)?;
            match candidate {
                auto_complete::ReviewerOutcome::Verdict(
                    auto_complete::Verdict::RequestChanges | auto_complete::Verdict::Rejected,
                ) => blocking = Some(candidate),
                auto_complete::ReviewerOutcome::EscalatedToHuman { .. } if blocking.is_none() => {
                    blocking = Some(candidate);
                }
                _ => {}
            }
        }
        blocking.expect("fresh candidates are non-empty")
    };

    // Publish without overwriting a verdict that appeared at the canonical
    // path after the caller's initial probe. A hard link is an atomic,
    // complete-file, no-clobber publication boundary. On collision, include
    // that evidence in the same reconciliation and leave its bytes untouched.
    let dest_dir = project_root.join(".aida").join("review-verdicts");
    std::fs::create_dir_all(&dest_dir).map_err(|e| {
        auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::NoVerdict,
            format!("could not create canonical verdict directory: {e}"),
        )
    })?;
    let dest = dest_dir.join(freshest.file_name().expect("candidate has a file name"));
    let temp_name = format!(
        ".{}.publish-{}-{}",
        dest.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("verdict"),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let temp = dest_dir.join(temp_name);
    let staged = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bodies[0].as_bytes())?;
        file.sync_all()
    })();
    if let Err(e) = staged {
        let _ = std::fs::remove_file(&temp);
        return Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::NoVerdict,
            format!("could not stage canonical verdict evidence: {e}"),
        ));
    }
    let publication = std::fs::hard_link(&temp, &dest);
    let _ = std::fs::remove_file(&temp);
    match publication {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let canonical = std::fs::read_to_string(&dest).map_err(|read_error| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::NoVerdict,
                    format!(
                        "canonical verdict evidence appeared concurrently but could not be read: {read_error}"
                    ),
                )
            })?;
            bodies.push(canonical);
            let Some(current_head) = current_head else {
                return Err(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::NoVerdict,
                    "canonical verdict evidence appeared concurrently and cannot be reconciled because the current PR head is unavailable",
                ));
            };
            outcome = match reconcile(&bodies, current_head)? {
                Some(review_verdict::VerdictKind::Approved) => {
                    auto_complete::ReviewerOutcome::Verdict(auto_complete::Verdict::Approved)
                }
                Some(review_verdict::VerdictKind::RequestChanges)
                | Some(review_verdict::VerdictKind::Rejected) => {
                    auto_complete::ReviewerOutcome::Verdict(auto_complete::Verdict::RequestChanges)
                }
                Some(review_verdict::VerdictKind::Unknown) | None => {
                    return Err(auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::NoVerdict,
                        "canonical and sibling verdict evidence do not establish a verdict for the current PR head",
                    ));
                }
            };
        }
        Err(e) => {
            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::NoVerdict,
                format!("could not publish canonical verdict evidence: {e}"),
            ));
        }
    }
    eprintln!(
        "  {} no verdict at the drive root, but the reviewer wrote one in a sibling checkout ({}) during this session — accepting it",
        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
        freshest.display()
    );
    Ok(Some(outcome))
}

/// TASK-1460: the phase-3 handshake's per-commit fallback. The verdict file
/// at `path` is keyed by PR or spec, so a later round at a different commit
/// replaces it; BUG-1539 archived every round at `<key>/<sha>.json`. This
/// reads the archived round for `head` — only when it was written during this
/// review session (mtime >= `started_at`, the same freshness gate BUG-806
/// applies), and only through [`read_verdict_file_for_head`], so an archived
/// approval still has to prove it covers `head`. `None` = no such round; the
/// caller's original failure then stands.
// trace:TASK-1460 | ai:claude
pub(crate) fn phase3_head_archive_fallback(
    path: &std::path::Path,
    head: Option<&str>,
    started_at: std::time::SystemTime,
) -> Option<auto_complete::ReviewerOutcome> {
    let head = head?;
    // Fail closed: the archive may only answer when the CURRENT file is a
    // well-formed, unconflicted verdict that was recorded at a DIFFERENT
    // commit. Malformed JSON, a missing verdict, a reconcile conflict, or a
    // current file already at `head` keep their original error (PRIN-5,
    // TASK-1169) — an older archived approval must never paper over them.
    // trace:TASK-1460 | ai:claude
    read_verdict_file(path).ok()?;
    let current_sha = std::fs::read_to_string(path)
        .ok()
        .and_then(|b| review_verdict::parse_recorded_verdict(&b))
        .and_then(|v| v.reviewed_sha)?;
    if review_verdict::same_reviewed_sha(&current_sha, head) {
        return None;
    }
    let archived = review_verdict::archived_verdict_file_for_sha(path, head)?;
    let mtime = std::fs::metadata(&archived).ok()?.modified().ok()?;
    if mtime < started_at {
        return None;
    }
    read_verdict_file_for_head(&archived, Some(head)).ok()
}

#[cfg(test)]
mod task_1460_phase3_archive_tests {
    use super::*;

    const R1: &str = "3acf3671fd7a1111111111111111111111111111";
    const R2: &str = "cd21a1dc0a9e2222222222222222222222222222";

    fn record(root: &std::path::Path, path: &std::path::Path, verdict: &str, sha: &str) {
        review_verdict::record_verdict_at_path(
            root,
            path,
            Some(verdict),
            Some(sha),
            None,
            None,
            &[],
            "reviewer",
        )
        .unwrap();
    }

    // trace:TASK-1460 | ai:claude
    #[test]
    fn a_fresh_archived_round_at_head_answers_when_the_current_file_moved_on() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let path = review_verdict::verdict_path(root, "PR-50");
        let started = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        record(root, &path, "approved", R1);
        record(root, &path, "approved", R2);
        // The current file is at R2, so it cannot prove an approval of R1…
        assert!(read_verdict_file_for_head(&path, Some(R1)).is_err());
        // …but the archived round for R1 can.
        assert!(matches!(
            phase3_head_archive_fallback(&path, Some(R1), started),
            Some(auto_complete::ReviewerOutcome::Verdict(
                auto_complete::Verdict::Approved
            ))
        ));
        // No head, an unreviewed head, or a round older than this session: none.
        assert!(phase3_head_archive_fallback(&path, None, started).is_none());
        assert!(phase3_head_archive_fallback(&path, Some("deadbeefdeadbeef"), started).is_none());
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
        assert!(phase3_head_archive_fallback(&path, Some(R1), later).is_none());
    }

    // trace:TASK-1460 | ai:claude
    #[test]
    fn a_malformed_current_file_never_falls_back_to_an_archived_approval() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let path = review_verdict::verdict_path(root, "PR-51");
        let started = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        record(root, &path, "approved", R1);
        // The current file is then corrupted, or carries no reviewed commit.
        for bad in [
            "{not json",
            r#"{"summary":"no verdict"}"#,
            r#"{"verdict":"approved"}"#,
        ] {
            std::fs::write(&path, bad).unwrap();
            assert!(read_verdict_file_for_head(&path, Some(R1)).is_err());
            assert!(
                phase3_head_archive_fallback(&path, Some(R1), started).is_none(),
                "must fail closed for {bad}"
            );
            assert!(read_verdict_file_for_head(&path, Some(R1))
                .or_else(|e| phase3_head_archive_fallback(&path, Some(R1), started).ok_or(e))
                .is_err());
        }
        // A spec-keyed malformed record is an error from the spec fallback too.
        let spec_path = review_verdict::verdict_path(root, "TASK-61");
        record(root, &spec_path, "approved", R1);
        std::fs::write(&spec_path, "{not json").unwrap();
        assert!(spec_verdict_fallback_for_phase3(root, "TASK-61", started, Some(R1)).is_err());
    }

    // trace:TASK-1460 | ai:claude
    #[test]
    fn the_spec_fallback_reads_the_archived_round_for_head() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let path = review_verdict::verdict_path(root, "TASK-60");
        let started = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        record(root, &path, "request-changes", R1);
        record(root, &path, "approved", R2);
        let got = spec_verdict_fallback_for_phase3(root, "TASK-60", started, Some(R1))
            .unwrap_or_else(|_| panic!("the archived refusal at R1 must answer"));
        assert!(matches!(
            got,
            Some(auto_complete::ReviewerOutcome::Verdict(
                auto_complete::Verdict::RequestChanges
            ))
        ));
    }
}

pub(crate) fn read_verdict_file(
    path: &std::path::Path,
) -> Result<auto_complete::ReviewerOutcome, auto_complete::PhaseFailure> {
    let body = std::fs::read_to_string(path).map_err(|_| {
        auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::NoVerdict,
            "the reviewer session produced no verdict file — the review did not complete",
        )
    })?;
    if let Some(current_sha) =
        review_verdict::parse_recorded_verdict(&body).and_then(|recorded| recorded.reviewed_sha)
    {
        review_verdict::reconcile_artifacts_for_sha([body.as_str()], &current_sha).map_err(
            |reason| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::NoVerdict,
                    format!("{reason} — review evidence cannot be proven safe"),
                )
            },
        )?;
    }
    if let Some(conflict) = review_verdict::verdict_conflict_for_current_sha(&body) {
        return Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::NoVerdict,
            format!("{conflict} — refusing to select a winner; reconcile the independent reviews"),
        ));
    }
    let value: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
        auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::NoVerdict,
            format!("the verdict file is not valid JSON: {e}"),
        )
    })?;
    // STORY-306: `merge: escalated-to-human` ⇒ the reviewer would not
    // auto-decide the merge. A first-class non-failure outcome — the
    // orchestrator stops clean and leaves the PR for a human. The `summary`
    // field carries the "why".
    if value.get("merge").and_then(|m| m.as_str()).map(str::trim)
        == Some(reviewer_summary::MERGE_ESCALATED_TO_HUMAN)
    {
        let reason = value
            .get("summary")
            .and_then(|s| s.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("the reviewer escalated the merge decision to a human")
            .to_string();
        return Ok(auto_complete::ReviewerOutcome::EscalatedToHuman { reason });
    }
    let raw = value
        .get("verdict")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::NoVerdict,
                "the verdict file has no `verdict` field",
            )
        })?;
    auto_complete::Verdict::parse(raw)
        .map(auto_complete::ReviewerOutcome::Verdict)
        .ok_or_else(|| {
            auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::NoVerdict,
                format!("unrecognised verdict `{raw}` in the verdict file"),
            )
        })
}

/// Read a live phase-3 handshake and prove an approval covers the PR head the
/// orchestrator is about to advance. Refusals and escalations remain usable
/// without this check because they fail closed already.
// trace:BUG-1466 | ai:codex
// trace:BUG-1538 | ai:codex
pub(crate) fn read_verdict_file_for_head(
    path: &std::path::Path,
    current_head: Option<&str>,
) -> Result<auto_complete::ReviewerOutcome, auto_complete::PhaseFailure> {
    let outcome = read_verdict_file(path)?;
    if !matches!(
        outcome,
        auto_complete::ReviewerOutcome::Verdict(auto_complete::Verdict::Approved)
    ) {
        return Ok(outcome);
    }
    let body = std::fs::read_to_string(path).map_err(|e| {
        auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::NoVerdict,
            format!("could not re-read approval provenance: {e}"),
        )
    })?;
    let recorded = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| {
            v.get("reviewed_sha")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        });
    let same_commit = match (recorded.as_deref(), current_head) {
        (Some(a), Some(b)) => {
            let a = a.trim();
            let b = b.trim();
            a.len().min(b.len()) >= 7
                && a.bytes().all(|c| c.is_ascii_hexdigit())
                && b.bytes().all(|c| c.is_ascii_hexdigit())
                && (a.eq_ignore_ascii_case(b)
                    || (a.len() < b.len() && b[..a.len()].eq_ignore_ascii_case(a))
                    || (b.len() < a.len() && a[..b.len()].eq_ignore_ascii_case(b)))
        }
        _ => false,
    };
    if same_commit {
        Ok(outcome)
    } else {
        Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::NoVerdict,
            match (recorded.as_deref(), current_head) {
                (None, _) => "the APPROVED verdict is UNPROVEN because it records no reviewed_sha"
                    .to_string(),
                (_, None) => "the APPROVED verdict is UNPROVEN because the current PR head could not be resolved"
                    .to_string(),
                (Some(reviewed), Some(current)) => format!(
                    "the APPROVED verdict is stale: it reviewed {reviewed}, but the current PR head is {current}"
                ),
            },
        ))
    }
}

/// STORY-439: pick the calibration review-slot fields out of a verdict
/// file and upsert the per-spec capture record. The verdict file is
/// already loaded by `read_verdict_file` for the orchestrator's PASS /
/// FAIL decision; this is a tag-along read that records advisory
/// metadata (it never changes the decision). Best-effort — a missing
/// file, missing fields, or write error all silently no-op. Called for
/// each spec the PR credits; one PR populates N records.
/// trace:STORY-439 | ai:claude
pub(crate) fn capture_review_calibration_for_spec(
    project_root: &std::path::Path,
    verdict_path: &std::path::Path,
    spec: &str,
) {
    let Ok(body) = std::fs::read_to_string(verdict_path) else {
        return;
    };
    let Some(verdict) = reviewer_summary::parse_verdict_file(&body) else {
        return;
    };
    if let Some(level_raw) = verdict
        .implementation_complexity
        .as_deref()
        .map(str::trim)
        .filter(|s: &&str| !s.is_empty())
    {
        if let Some(level) = complexity_calibration::ComplexityLevel::parse_str(level_raw) {
            let agreement = verdict
                .complexity_agreement
                .as_deref()
                .and_then(complexity_calibration::ComplexityAgreement::parse_str);
            if let Err(e) =
                complexity_calibration::upsert_review(project_root, spec, level, agreement)
            {
                eprintln!(
                    "  {} could not record review calibration for {spec}: {e}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
        }
    }
    let effort = verdict
        .implementation_effort
        .as_deref()
        .and_then(effort_calibration::EffortBucket::parse_str);
    if let Err(e) =
        effort_calibration::upsert_review(project_root, spec, effort, Some("reviewer".to_string()))
    {
        eprintln!(
            "  {} could not record review effort for {spec}: {e}",
            crate::glyph(crate::glyphs::Glyph::Warning).yellow()
        );
    }
}

/// When phase 3 ends with [`auto_complete::FailureKind::NoVerdict`] under a
/// headless `--no-human` drain, scan the reviewer's headless log for the
/// BUG-280 signature: an `AskUserQuestion` event. The harness denies
/// AskUserQuestion under `--no-human=both`, the reviewer Claude bails ~10s
/// later, and the verdict file never lands — so an AskUserQuestion event
/// alongside a missing verdict file is a near-certain diagnosis.
///
/// Returns the original failure when no recent log can be located, when the
/// log cannot be read, or when no AskUserQuestion event is found. On a hit,
/// returns a NoVerdict failure whose `reason` names AskUserQuestion as the
/// likely cause and points at the offending log file. trace:BUG-280 | ai:claude
pub(crate) fn enrich_no_verdict_with_headless_diagnostic(
    failure: auto_complete::PhaseFailure,
    project_root: &std::path::Path,
    started_at: std::time::SystemTime,
) -> auto_complete::PhaseFailure {
    if failure.kind != auto_complete::FailureKind::NoVerdict {
        return failure;
    }
    let logs_dir = project_root.join(".aida").join("headless-logs");
    let Ok(entries) = std::fs::read_dir(&logs_dir) else {
        return failure;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified < started_at {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        // The harness emits AskUserQuestion as a tool_use with name field —
        // the substring catches both the structured `"name":"AskUserQuestion"`
        // form and any narrative mention in assistant text.
        if body.contains("AskUserQuestion") {
            return auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::NoVerdict,
                format!(
                    "{} — the reviewer's headless log ({}) contains an \
                     AskUserQuestion call, which is forbidden under \
                     `--no-human` and likely caused the session to bail \
                     before writing the verdict file (BUG-280). The \
                     `/aida-review` skill must auto-resolve every \
                     confirmation under AIDA_HEADLESS=1 instead of \
                     calling AskUserQuestion.",
                    failure.reason,
                    entry.path().display(),
                ),
            );
        }
    }
    enrich_headless_wait_failure(failure, project_root, started_at)
}

// trace:BUG-1063 | ai:codex
pub(crate) fn enrich_headless_wait_failure(
    failure: auto_complete::PhaseFailure,
    project_root: &std::path::Path,
    started_at: std::time::SystemTime,
) -> auto_complete::PhaseFailure {
    if !matches!(
        failure.kind,
        auto_complete::FailureKind::NoPr | auto_complete::FailureKind::NoVerdict
    ) {
        return failure;
    }
    let logs_dir = project_root.join(".aida").join("headless-logs");
    let Ok(entries) = std::fs::read_dir(&logs_dir) else {
        return failure;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified < started_at {
            continue;
        }
        let path = entry.path();
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(last_text) = last_headless_assistant_text(&body) else {
            continue;
        };
        if headless_wait_text(&last_text) {
            return auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::HeadlessWait,
                format!(
                    "{} — the headless log ({}) ended with assistant text that \
                     waits for a future notification/monitor. Under AIDA_HEADLESS=1 \
                     there is no next turn; poll in <=60s bounded steps and write \
                     the phase result with the current wait state instead (BUG-1063).",
                    failure.reason,
                    path.display(),
                ),
            );
        }
    }
    failure
}

// trace:BUG-1063 | ai:codex
pub(crate) fn last_headless_assistant_text(content: &str) -> Option<String> {
    content
        .lines()
        .rev()
        .filter_map(headless_line_assistant_text)
        .find(|s| !s.trim().is_empty())
}

// trace:BUG-1063 | ai:codex
pub(crate) fn headless_line_assistant_text(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if value.get("type").and_then(|v| v.as_str()) == Some("result") {
        return value
            .get("result")
            .and_then(|v| v.as_str())
            .map(str::to_string);
    }
    if value.get("type").and_then(|v| v.as_str()) != Some("assistant") {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
        parts.push(text.to_string());
    }
    let content = value
        .get("message")
        .and_then(|m| m.get("content"))
        .or_else(|| value.get("content"));
    if let Some(blocks) = content.and_then(|v| v.as_array()) {
        for block in blocks {
            if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    parts.push(text.to_string());
                }
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

// trace:BUG-1063 | ai:codex
pub(crate) fn headless_wait_text(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "waiting on",
        "wait for the notification",
        "waiting for the notification",
        "waiting on the watcher notification",
        "armed a monitor",
    ];
    MARKERS.iter().any(|marker| lower.contains(marker))
}

#[cfg(test)]
#[path = "tests/enrich_headless_diagnostic_tests.rs"]
mod enrich_headless_diagnostic_tests;

#[cfg(test)]
#[path = "tests/read_verdict_file_tests.rs"]
mod read_verdict_file_tests;

/// TASK-262: cover the `RealPhaseDriver` subprocess-wiring seams that drive the
/// orchestrator's real phases — the argv each phase hands to its `aida`
/// subprocess, plus the lease-discovery multiplicity (0/1/N) that decides
/// whether phase 1 succeeds or emits a candidate-listing failure. STORY-246
/// shipped the orchestrator with full `MockPhaseDriver` coverage but left the
/// real driver's command-building seams untested; these close that gap without
/// ever spawning `claude`/`aida`/`git`. Every test is fully isolated (its own
/// `tempfile::tempdir`, no shared CWD/HOME, no real git ops) to avoid the
/// BUG-463-class flakiness.
// trace:TASK-262 | ai:claude
#[cfg(test)]
#[path = "tests/real_phase_driver_wiring_tests.rs"]
mod real_phase_driver_wiring_tests;

/// BUG-1716: the never-started launch classification — missing-log evidence,
/// the commits corroboration, and the no-transient-retry contract.
// trace:BUG-1716 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1716_never_started_launch_tests.rs"]
mod bug_1716_never_started_launch_tests;

/// Read a spec's current status straight from the git-canonical store —
/// ground truth for the BUG-241 reconcile step. A spec the implementer (or a
/// human) marked Completed needed no further work, even when the phase
/// produced no PR (instance B: resolved-by-supersession). `None` when the
/// store can't be opened or the spec isn't found. trace:BUG-241 | ai:claude
pub(crate) fn spec_status(project_root: &std::path::Path, spec: &str) -> Option<RequirementStatus> {
    use aida_core::DatabaseBackend;
    let store_path = project_root.join(".aida-store");
    let backend = aida_core::GitBackend::new(&store_path).ok()?;
    let req = backend.get_requirement_by_spec_id(spec).ok()??;
    Some(req.status)
}

// trace:BUG-1112 | ai:codex
pub(crate) fn latest_reopen_transition_at(
    req: &aida_core::Requirement,
) -> Option<chrono::DateTime<chrono::Utc>> {
    req.history
        .iter()
        .filter(|entry| {
            entry.changes.iter().any(|change| {
                change.field_name == "status"
                    && aida_core::lifecycle::State::from_status_str(&change.old_value)
                        .is_some_and(|s| s.is_terminal())
                    && aida_core::lifecycle::State::from_status_str(&change.new_value)
                        .is_some_and(|s| !s.is_terminal())
            })
        })
        .map(|entry| entry.timestamp)
        .max()
}

// trace:BUG-1112 | ai:codex
pub(crate) fn spec_latest_reopen_transition_at(
    project_root: &std::path::Path,
    spec: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    use aida_core::DatabaseBackend;
    let store_path = project_root.join(".aida-store");
    let backend = aida_core::GitBackend::new(&store_path).ok()?;
    let req = backend.get_requirement_by_spec_id(spec).ok()??;
    latest_reopen_transition_at(&req)
}

// trace:BUG-1112 | ai:codex
pub(crate) fn merged_at_satisfies_latest_reopen(
    merged_at: Option<chrono::DateTime<chrono::Utc>>,
    latest_reopen_at: Option<chrono::DateTime<chrono::Utc>>,
) -> bool {
    match latest_reopen_at {
        None => true,
        Some(reopen_at) => merged_at.is_some_and(|merged_at| merged_at > reopen_at),
    }
}

/// Pure decision for the BUG-241 reconcile (`RealPhaseDriver::reconcile_failure`):
/// given the two ground-truth signals — a verified merged PR number, if one
/// was found, and whether the spec reached Completed — decide whether a phase
/// failure is genuine or an out-of-band success. A merged PR reaches this
/// helper only after the caller verifies that the PR credits the dispatched
/// spec; that guard prevents BUG-357's wrong-spec reconcile false positive.
/// Either verified signal alone is proof the spec shipped; needing both would
/// miss instance A (the status auto-bump lags a human's out-of-band merge) and
/// instance B (a no-work spec never gets a PR). Split out so the rule is
/// unit-testable without `gh` or a store.
/// trace:BUG-241 | ai:claude
pub(crate) fn reconcile_verdict(
    verified_merged_pr: Option<u32>,
    spec_completed: bool,
    spec: &str,
) -> auto_complete::PhaseReconcile {
    use auto_complete::PhaseReconcile;
    match (verified_merged_pr, spec_completed) {
        (Some(pr), true) => PhaseReconcile::ShippedOutOfBand {
            reason: format!("PR-{pr} is merged and {spec} is Completed"),
        },
        (Some(pr), false) => PhaseReconcile::ShippedOutOfBand {
            reason: format!("PR-{pr} is already merged"),
        },
        (None, true) => PhaseReconcile::ShippedOutOfBand {
            reason: format!("{spec} is Completed — the spec needed no further work"),
        },
        // Regression guard: no merged PR and the spec is still open — reality
        // confirms nothing shipped, so the phase failure stands. trace:BUG-241
        (None, false) => PhaseReconcile::GenuineFailure,
    }
}

#[cfg(test)]
#[path = "tests/reconcile_verdict_tests.rs"]
mod reconcile_verdict_tests;

/// BUG-1629: the workspace phase 1 launches. `fresh` marks a branch the
/// pickup resolver chose as free that has not been launched yet; see
/// [`RealPhaseDriver::phase1_launch_workspace`].
// trace:BUG-1629 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Phase1Workspace {
    pub(crate) worktree: std::path::PathBuf,
    pub(crate) branch: String,
    pub(crate) fresh: bool,
}

/// BUG-1629: a freshly resolved branch that exists at launch was created by
/// someone else after resolution.
// trace:BUG-1629 | ai:claude
pub(crate) fn phase1_fresh_branch_raced(fresh: bool, branch_exists_now: bool) -> bool {
    fresh && branch_exists_now
}

/// BUG-1629: exactly one clean replacement launch per phase-1 run.
// trace:BUG-1629 | ai:claude
pub(crate) fn phase1_lost_child_retry_allowed(retries_used: usize) -> bool {
    retries_used == 0
}

/// BUG-1769: is this spec's work finished? `Done` means committed on a branch
/// and `Completed` means landed on the default branch, so both say the phase's
/// goal was reached. `Rejected` / `Superseded` / `NeedsAttention` are terminal
/// or parked but are NOT finished work, and reading them as success would turn
/// a declined spec into a shipped one.
// trace:BUG-1769 | ai:claude
pub(crate) fn phase1_status_is_finished(status: &RequirementStatus) -> bool {
    matches!(
        status,
        RequirementStatus::Done | RequirementStatus::Completed
    )
}

/// BUG-1769: did THIS run finish the work? The discriminator is BUG-1524's own
/// before-and-after principle applied to spec status: an advance counts only
/// when the entry reading was not already finished, so a spec that was `Done`
/// *before* phase 1 began is still refused exactly as it is today.
///
/// Either reading being absent is UNKNOWN, not "was not finished", so it can
/// never license the success path — the fail-safe answer is the behaviour that
/// shipped before this fix. That matters because the no-store case is real: a
/// fixture or a checkout without `.aida-store` makes `spec_status` return
/// `None` at both ends.
// trace:BUG-1769 | ai:claude
pub(crate) fn phase1_advanced_to_finished_during_run(
    entry: Option<&RequirementStatus>,
    current: Option<&RequirementStatus>,
) -> bool {
    match (entry, current) {
        (Some(entry), Some(current)) => {
            phase1_status_is_finished(current) && !phase1_status_is_finished(entry)
        }
        _ => false,
    }
}

/// BUG-1769: render a substrate status reading for a diagnostic message, so a
/// lost-child failure can say what it actually read instead of asserting an
/// un-read "was not advanced".
// trace:BUG-1769 | ai:claude
pub(crate) fn phase1_spec_status_text(status: Option<&RequirementStatus>) -> String {
    match status {
        Some(status) => status.to_string(),
        None => "unreadable (no store, or the spec is not in it)".to_string(),
    }
}

// trace:BUG-1629 | ai:claude
pub(crate) fn phase1_branch_race_reason(spec: &str, branch: &str) -> String {
    format!(
        "phase 1 workspace race for {spec}: branch `{branch}` was free when phase 1 resolved \
         it but exists now. `aida queue work --branch` reuses an existing branch, so launching \
         would adopt a branch this run did not create; refused before launching. Re-run to \
         resolve a fresh workspace, or inspect `{branch}` first"
    )
}

/// BUG-1629: retry cause recorded for the phase-1 replacement launch.
// trace:BUG-1629 | ai:claude
pub(crate) const PHASE1_LOST_CHILD_STATE_CAUSE: &str = "lost-child-state";

/// BUG-1629: is `worktree` free of every lease? Lease paths are stored
/// canonicalized while a pinned path may not be (a symlinked checkout, macOS
/// `/var`), so both sides are canonicalized before comparing. Fail safe: if
/// either side cannot be canonicalized, the worktree counts as leased, so
/// the replacement never passes `--force-claim` over a path it cannot prove
/// is free.
// trace:BUG-1629 | ai:claude
pub(crate) fn worktree_is_unleased(
    worktree: &std::path::Path,
    lease_worktrees: &[std::path::PathBuf],
) -> bool {
    let Ok(pinned) = worktree.canonicalize() else {
        return false;
    };
    lease_worktrees
        .iter()
        .all(|leased| leased.canonicalize().is_ok_and(|leased| leased != pinned))
}

/// BUG-1629: where `queue work` records its own refusal for an orchestrated
/// launch — a sibling of the handoff receipt, keyed by the same session id.
// trace:BUG-1629 | ai:claude
pub(crate) fn orchestrated_child_refusal_path(receipt: &std::path::Path) -> std::path::PathBuf {
    receipt.with_extension("refusal.txt")
}

/// BUG-1629: called on the error path of every `aida` invocation. When this
/// process is an orchestrated phase-1 `queue work` child that failed before
/// publishing its handoff receipt, record the refusal next to the receipt so
/// the parent can name the real reason. Best-effort; see
/// [`orchestrated_child_refusal_to_record`] for the gates.
// trace:BUG-1629 | ai:claude
pub(crate) fn record_orchestrated_child_refusal(message: &str) {
    let receipt_env = std::env::var(ORCHESTRATED_LEASE_RECEIPT_ENV).ok();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some((refusal, summary)) =
        orchestrated_child_refusal_to_record(receipt_env.as_deref(), &args, message, |path| {
            path.exists()
        })
    {
        let _ = write_atomic(&refusal, &summary);
    }
}

/// BUG-1629: the pure decision behind [`record_orchestrated_child_refusal`].
/// Records `(refusal path, summary)` only when all hold: the process carries
/// the orchestrator's receipt env var, it is a `queue work` invocation, the
/// receipt does not exist yet (a child that got that far is not a pre-lease
/// refusal), no refusal was recorded already (never overwrite the first
/// cause), and the summary is non-empty.
// trace:BUG-1629 | ai:claude
pub(crate) fn orchestrated_child_refusal_to_record(
    receipt_env: Option<&str>,
    args: &[String],
    message: &str,
    exists: impl Fn(&std::path::Path) -> bool,
) -> Option<(std::path::PathBuf, String)> {
    let receipt = std::path::PathBuf::from(receipt_env.filter(|r| !r.trim().is_empty())?);
    if args.len() < 2 || args[0] != "queue" || args[1] != "work" {
        return None;
    }
    let refusal = orchestrated_child_refusal_path(&receipt);
    if exists(&receipt) || exists(&refusal) {
        return None;
    }
    let summary = orchestrated_child_refusal_summary(message);
    (!summary.is_empty()).then_some((refusal, summary))
}

/// BUG-1629: the most characters of child refusal text either side keeps.
// trace:BUG-1629 | ai:claude
pub(crate) const ORCHESTRATED_CHILD_REFUSAL_MAX_CHARS: usize = 400;

/// BUG-1629: remove ANSI escape sequences and every other control character
/// (ESC, CR, NUL, ...) so child text cannot restyle or rewrite the parent's
/// stderr line, the `SpecRetried` detail, or the shelve reason.
// trace:BUG-1629 | ai:claude
pub(crate) fn strip_control_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                // CSI: ESC [ params... final byte in @..~
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC: ESC ] ... terminated by BEL or ESC \
                Some(']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                // Two-character escape: drop the next character too.
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
            continue;
        }
        if !c.is_control() {
            out.push(c);
        }
    }
    out
}

/// BUG-1629: the first non-empty line of an error, control characters
/// stripped, bounded for a message.
// trace:BUG-1629 | ai:claude
pub(crate) fn orchestrated_child_refusal_summary(message: &str) -> String {
    message
        .lines()
        .map(|line| strip_control_text(line).trim().to_string())
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .chars()
        .take(ORCHESTRATED_CHILD_REFUSAL_MAX_CHARS)
        .collect()
}

/// BUG-1629: read and remove the refusal a phase-1 child recorded. The read
/// is capped (4 KiB) and re-sanitized to the writer's bound, so a file that
/// was not written by the writer above cannot flood or restyle the message.
// trace:BUG-1629 | ai:claude
pub(crate) fn take_orchestrated_child_refusal(
    project_root: &std::path::Path,
    session_uuid: &str,
) -> Option<String> {
    use std::io::Read;
    let path = orchestrated_child_refusal_path(&orchestrated_lease_receipt_path(
        project_root,
        session_uuid,
    ));
    let mut bytes = Vec::new();
    let read = std::fs::File::open(&path)
        .and_then(|file| file.take(4096).read_to_end(&mut bytes))
        .is_ok();
    let _ = std::fs::remove_file(&path);
    if !read {
        return None;
    }
    let summary = orchestrated_child_refusal_summary(&String::from_utf8_lossy(&bytes));
    (!summary.is_empty()).then_some(summary)
}

/// BUG-1629: drop any refusal file for a session whose child did establish
/// its lease or receipt. A child can write its receipt and still fail later,
/// or a refusal can survive a receipt that was published afterwards; the
/// file is keyed by the session uuid so it is harmless, but the parent
/// clears it so it never accumulates.
// trace:BUG-1629 | ai:claude
pub(crate) fn discard_orchestrated_child_refusal(
    project_root: &std::path::Path,
    session_uuid: &str,
) {
    let _ = std::fs::remove_file(orchestrated_child_refusal_path(
        &orchestrated_lease_receipt_path(project_root, session_uuid),
    ));
}

/// BUG-1629: the precise recovery state when a phase-1 child kept neither its
/// session lease nor its handoff receipt.
// trace:BUG-1629 | ai:claude
pub(crate) fn lost_child_state_reason(
    session_uuid: &str,
    exit: &str,
    elapsed: std::time::Duration,
) -> String {
    format!(
        "phase 1 child session {} exited {exit} after {}ms without a session lease or an \
         orchestrator handoff receipt: no implementer session started, so there is nothing \
         to resume",
        &session_uuid[..session_uuid.len().min(8)],
        elapsed.as_millis(),
    )
}

/// Lease ids present in `.aida/sessions/` — `<id>.toml` files only, *not*
/// the `<id>.activity.toml` / `<id>.manifest.toml` companions. A lease id is
/// a dot-free UUID, so a real lease file's name has exactly one `.`; every
/// companion file has two. (The old set-diff filter excluded only
/// `.activity.toml`, so it miscounted `.manifest.toml` as a lease — BUG-114.)
/// trace:BUG-114 | ai:claude
pub(crate) fn lease_ids_in(sessions_dir: &std::path::Path) -> Vec<String> {
    let mut ids = Vec::new();
    if let Ok(rd) = std::fs::read_dir(sessions_dir) {
        for entry in rd.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if let Some(stem) = name.strip_suffix(".toml") {
                    if !stem.contains('.') {
                        ids.push(stem.to_string());
                    }
                }
            }
        }
    }
    ids.sort();
    ids
}

pub(crate) const ORCHESTRATED_LEASE_RECEIPT_ENV: &str = "AIDA_ORCHESTRATED_LEASE_RECEIPT";

pub(crate) fn orchestrated_lease_receipt_path(
    project_root: &std::path::Path,
    claude_session_id: &str,
) -> std::path::PathBuf {
    project_root
        .join(".aida")
        .join("orchestrator-handoffs")
        .join(format!("{claude_session_id}.json"))
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct OrchestratedLeaseReceipt {
    pub(crate) claude_session_id: String,
    pub(crate) lease_id: String,
    pub(crate) branch: String,
    pub(crate) worktree_path: std::path::PathBuf,
}

pub(crate) fn write_orchestrated_lease_receipt(
    path: &std::path::Path,
    claude_session_id: &str,
    lease: &SessionLease,
) -> Result<()> {
    let receipt = OrchestratedLeaseReceipt {
        claude_session_id: claude_session_id.to_string(),
        lease_id: lease.id.clone(),
        branch: lease.branch.clone(),
        worktree_path: lease.worktree_path.clone(),
    };
    let body = serde_json::to_string(&receipt)?;
    Ok(write_atomic(path, &body)?)
}

pub(crate) fn publish_orchestrated_lease_receipt_from_env(
    claude_session_id: Option<&str>,
    lease: &SessionLease,
) -> Result<()> {
    if let (Ok(path), Some(claude_id)) = (
        std::env::var(ORCHESTRATED_LEASE_RECEIPT_ENV),
        claude_session_id,
    ) {
        write_orchestrated_lease_receipt(std::path::Path::new(&path), claude_id, lease)
            .with_context(|| format!("writing orchestrator lease receipt {}", path))?;
    }
    Ok(())
}

pub(crate) fn prepare_orchestrated_lease_receipt(
    cmd: &mut std::process::Command,
    project_root: &std::path::Path,
    claude_session_id: &str,
) -> std::path::PathBuf {
    let receipt = orchestrated_lease_receipt_path(project_root, claude_session_id);
    if let Some(parent) = receipt.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_file(&receipt);
    // trace:BUG-1629 | ai:claude
    let _ = std::fs::remove_file(orchestrated_child_refusal_path(&receipt));
    cmd.env(ORCHESTRATED_LEASE_RECEIPT_ENV, &receipt);
    receipt
}

pub(crate) fn read_orchestrated_lease_receipt(
    path: &std::path::Path,
    claude_session_id: &str,
) -> Option<(String, String, std::path::PathBuf, Option<u32>)> {
    let body = std::fs::read_to_string(path).ok()?;
    let receipt: OrchestratedLeaseReceipt = serde_json::from_str(&body).ok()?;
    (receipt.claude_session_id == claude_session_id).then_some((
        receipt.lease_id,
        receipt.branch,
        receipt.worktree_path,
        None,
    ))
}

/// Find the session lease that `aida queue work --session-id <uuid>` created,
/// by matching the orchestrator-minted `claude_session_id` against the value
/// `aida queue work` records in each session manifest. This is the
/// deterministic replacement for the lease-set-diff heuristic: it pins the
/// orchestrated session by id, so concurrent leases (parallel user sessions,
/// nested `/aida-pickup`) and the `.manifest.toml` companion file cannot
/// confuse it. Returns `(lease_id, branch, worktree_path)`.
/// trace:BUG-114 trace:BUG-223 | ai:claude
pub(crate) fn find_orchestrated_lease(
    project_root: &std::path::Path,
    claude_session_id: &str,
) -> Option<(String, String, std::path::PathBuf, Option<u32>)> {
    let manifest = session_manifest::list_all(project_root)
        .into_iter()
        .find(|m| m.claude_session_id.as_deref() == Some(claude_session_id))?;
    let lease_path = leases_dir(project_root).join(format!("{}.toml", manifest.session_id));
    let body = std::fs::read_to_string(&lease_path).ok()?;
    let peek: LeasePeek = toml::from_str(&body).ok()?;
    Some((
        peek.id,
        peek.branch,
        peek.worktree_path,
        peek.active_pid.or(peek.creator_pid),
    ))
}

/// BUG-1285: when [`RealPhaseDriver::discover_orchestrated_lease`] comes up
/// empty, the dominant real-world cause is a same-scope lease conflict — the
/// child `aida queue work`'s claim gate refused before it ever minted a
/// lease under the orchestrator's session id, most often because a PREVIOUS
/// (now-dead) drain run's own lease on this spec's scope is still on disk.
/// Name that lease and its holder's liveness — per the SAME identity-aware
/// check `stale_lease_recovery_for_lease` uses (pid + kernel start time,
/// degrading to a plain pid check only when start-time data is missing) —
/// instead of leaving the caller to cross-reference a bare id list against
/// `aida ps` by hand. `None` when no lease on disk holds this spec's own
/// scope (a genuinely unmatched-candidates case), which leaves the existing
/// bare-list diagnostic standing.
// trace:BUG-1285 | ai:claude
pub(crate) fn scope_conflict_lease_failure(
    project_root: &std::path::Path,
    spec: &str,
) -> Option<auto_complete::PhaseFailure> {
    let conflict = find_scope_lease_conflict(&list_leases(project_root), spec)?;
    let holder_pid = conflict.active_pid.or(conflict.creator_pid);
    let holder_start = if conflict.active_pid.is_some() {
        conflict.active_pid_start_time.as_deref()
    } else {
        conflict.creator_pid_start_time.as_deref()
    };
    let liveness = match holder_pid {
        Some(pid) if process_probe::process_identity_is_alive(pid, holder_start) => {
            format!("holder pid {pid} is alive")
        }
        Some(pid) => {
            let dead_since = std::fs::metadata(lease_path(project_root, &conflict.id))
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .map(|d| {
                    format!(
                        ", dead since ~{} ago",
                        humanize_secs_short(d.as_secs() as i64)
                    )
                })
                .unwrap_or_default();
            format!("holder pid {pid} is dead{dead_since}")
        }
        None => "the lease records no process-liveness signal".to_string(),
    };
    let short_id = &conflict.id[..conflict.id.len().min(8)];
    Some(auto_complete::PhaseFailure::of(
        auto_complete::FailureKind::LeaseConflict,
        format!(
            "the previous attempt's lease {short_id} is still held on scope `{spec}` \
             ({liveness}) — that is why this run's claim never minted a session lease \
             under it. Recovery: `aida queue work {spec} --force-claim` if the holder is \
             dead and its worktree is clean, or `aida queue work {spec} --resume` to \
             continue that session."
        ),
    ))
}

/// Decide whether the worktree's live branch represents a mid-phase swap
/// away from the branch the lease recorded at session-start. Returns the
/// new branch when a genuine swap is detected; `None` when the recorded
/// branch should stand — unchanged, detached HEAD (`rev-parse
/// --abbrev-ref` yields the literal `HEAD`), or the branch was
/// undetectable (the worktree is gone). Pure — split from
/// [`reconcile_orchestrated_branch`] so the swap rule is unit-testable
/// without a git fixture. trace:BUG-223 | ai:claude
pub(crate) fn swapped_branch(live: Result<String>, recorded: &str) -> Option<String> {
    match live {
        Ok(b) if !b.is_empty() && b != "HEAD" && b != recorded => Some(b),
        _ => None,
    }
}

/// Rewrite the `branch` field of an existing session lease, leaving every
/// other field intact. Used by the orchestrator when it detects a
/// mid-phase branch swap (BUG-223). The orchestrated session has already
/// exited by the time this runs, so there is no concurrent writer — but
/// the write is still atomic for consistency with the other lease-file
/// writers. trace:BUG-223 | ai:claude
pub(crate) fn update_lease_branch(
    project_root: &std::path::Path,
    lease_id: &str,
    branch: &str,
) -> Result<()> {
    let path = lease_path(project_root, lease_id);
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("reading lease {}", path.display()))?;
    let mut lease: SessionLease =
        toml::from_str(&body).with_context(|| format!("parsing lease {}", path.display()))?;
    lease.branch = branch.to_string();
    let content = toml::to_string_pretty(&lease)?;
    write_atomic(&path, &content).with_context(|| format!("writing lease {}", path.display()))?;
    Ok(())
}

/// TASK-358: stamp the lease's `escalated_to_human` timestamp. The
/// orchestrator's `--escalate-blocks` path calls this after the advisor
/// escalates a punted design-fork to a human — the marker tells later
/// cleanup paths (`aida edit --status` out of `NeedsAttention`,
/// `aida session prune --escalations`) that this lease is safe to remove.
/// trace:TASK-358 | ai:claude
pub(crate) fn mark_lease_escalated_to_human(
    project_root: &std::path::Path,
    lease_id: &str,
) -> Result<()> {
    let path = lease_path(project_root, lease_id);
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("reading lease {}", path.display()))?;
    let mut lease: SessionLease =
        toml::from_str(&body).with_context(|| format!("parsing lease {}", path.display()))?;
    lease.escalated_to_human = Some(chrono::Utc::now());
    let content = toml::to_string_pretty(&lease)?;
    write_atomic(&path, &content).with_context(|| format!("writing lease {}", path.display()))?;
    Ok(())
}

/// BUG-778: stamp `manual_enter_at = now` on a lease `aida worktree enter|add`
/// just handed to a human — the provenance marker that keeps `aida ps` from
/// reading the operator's launch-lag as a dead agent (and from offering the
/// `aida queue work` re-dispatch that would race them).
///
/// Patched as a single generic `toml::Value` key insert, the
/// [`crate::worktree_lock`] pattern, rather than a `SessionLease`
/// round-trip: a lease may carry keys this struct doesn't model (STORY-711's
/// `authorized_by` is written the same way), and a deserialize/reserialize
/// would silently drop them.
///
/// Idempotent by construction — re-entering refreshes the stamp, which is
/// exactly right: the worktree is being handed over again.
// trace:BUG-778 | ai:claude
pub(crate) fn mark_lease_manual_enter(
    project_root: &std::path::Path,
    lease_id: &str,
) -> Result<()> {
    let path = lease_path(project_root, lease_id);
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("reading lease {}", path.display()))?;
    let mut value: toml::Value =
        toml::from_str(&body).with_context(|| format!("parsing lease {}", path.display()))?;
    let table = value.as_table_mut().context("lease TOML is not a table")?;
    table.insert(
        "manual_enter_at".to_string(),
        toml::Value::String(chrono::Utc::now().to_rfc3339()),
    );
    let content = toml::to_string_pretty(&value)?;
    write_atomic(&path, &content).with_context(|| format!("writing lease {}", path.display()))?;
    Ok(())
}

/// Re-derive the worktree's current branch and, when it has drifted from
/// the branch the lease recorded at session-start, rewrite the lease so
/// every downstream consumer (`aida session end`, `aida session leases`)
/// sees the live branch. Returns the branch the orchestrator should drive
/// the rest of the pipeline with.
///
/// BUG-223: `/aida-pr`'s BUG-88 merged-branch-name guard can move the
/// commits to a fresh branch mid-implementer-phase. The lease's recorded
/// branch then goes stale and the phase-1 `gh pr list --head <branch>`
/// lookup reports a false "opened no PR". The worktree's live HEAD is the
/// only ground truth. trace:BUG-223 | ai:claude
pub(crate) fn reconcile_orchestrated_branch(
    project_root: &std::path::Path,
    lease_id: &str,
    worktree_path: &std::path::Path,
    recorded: &str,
) -> String {
    let live = aida_core::git_ops::current_branch(worktree_path);
    match swapped_branch(live, recorded) {
        Some(new_branch) => {
            match update_lease_branch(project_root, lease_id, &new_branch) {
                Ok(()) => eprintln!(
                    "  {} branch swapped during the implementer phase: `{}` → `{}` \
                     (lease updated)",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    recorded,
                    new_branch,
                ),
                Err(e) => eprintln!(
                    "  {} branch swapped to `{}` but the lease could not be updated: {} \
                     (continuing with the live branch)",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    new_branch,
                    e,
                ),
            }
            new_branch
        }
        None => recorded.to_string(),
    }
}

// trace:BUG-1199 trace:TASK-1262 | ai:codex
pub(crate) fn resolve_aida_exe_from(current: Option<std::path::PathBuf>) -> std::path::PathBuf {
    if let Some(p) = current {
        // Linux's `/proc/self/exe` can return "<path> (deleted)" when the
        // executable file has been unlinked. Strip that before checking.
        let lossy = p.to_string_lossy();
        let cleaned = lossy
            .strip_suffix(" (deleted)")
            .map(std::path::PathBuf::from)
            .unwrap_or(p);
        if cleaned.exists() {
            return cleaned;
        }
    }
    // Fall back to PATH search. Either the OS lookup failed, or the
    // resolved path no longer points at an existing file.
    std::path::PathBuf::from("aida")
}

/// BUG-766: make every child process resolve `aida` to THIS build.
///
/// Drive/drain child sessions (and anything they spawn) look up a bare
/// `aida` on PATH; on a machine with an old installed binary that lookup
/// can win over the coordinating build and run pre-guard bulk store saves.
/// Called once at the top of `main_entry` (single-threaded, before any
/// command dispatch): sets `AIDA_BIN` to the resolved coordinating binary
/// and prepends its directory to PATH so children inherit both. No-op when
/// the binary can't be resolved to an absolute existing path.
// trace:BUG-766 | ai:claude
pub(crate) fn export_coordinating_bin_env() {
    // Keep an invalid operator override intact so the launch path can report
    // it clearly; silently replacing it with current_exe would hide the error.
    // trace:TASK-1499 | ai:codex
    let ambient_override = std::env::var_os("AIDA_BIN").map(std::path::PathBuf::from);
    let has_aida_parent = ambient_override
        .as_ref()
        .is_some_and(|_| aida_bin::has_aida_ancestor());
    let resolution = aida_bin::process();
    if let Some(ambient_override) = ambient_override.as_ref() {
        if !has_aida_parent {
            match &resolution {
                Ok(resolved) => eprintln!(
                    "AIDA_BIN={} is set but was not honored because no AIDA parent was found; using {} ({})",
                    ambient_override.display(), resolved.path.display(), resolved.source
                ),
                Err(error) => eprintln!(
                    "AIDA_BIN={} is set but was not honored because no AIDA parent was found; binary resolution failed: {error:#}",
                    ambient_override.display()
                ),
            }
        }
    }
    if ambient_override.is_some() && has_aida_parent && resolution.is_err() {
        return;
    }
    let exe = resolution
        .map(|resolved| resolved.path)
        .unwrap_or_else(|_| aida_bin::running_executable_fallback());
    if !exe.is_absolute() || !exe.exists() {
        return;
    }
    let Some(dir) = exe.parent().map(std::path::Path::to_path_buf) else {
        return;
    };
    let path = std::env::var_os("PATH").unwrap_or_default();
    let updated = prepend_dir_to_path(&dir, &path);
    // `set_var` is wrapped in `unsafe` for Edition-2024 forward-compat
    // (mirrors `apply_session_env_to_process`); safe here because
    // `main_entry` hasn't spawned any threads yet.
    #[allow(unused_unsafe)]
    unsafe {
        std::env::set_var("AIDA_BIN", &exe);
        std::env::set_var("PATH", updated);
    }
}

/// BUG-766: pure PATH computation for `export_coordinating_bin_env`, split
/// out so tests can assert the shape without mutating process env. Puts
/// `dir` first and drops any later occurrence of it, so nested
/// aida→claude→aida chains don't grow PATH without bound and a stale
/// binary earlier on the original PATH can never shadow the coordinating
/// build.
// trace:BUG-766 | ai:claude
pub(crate) fn prepend_dir_to_path(
    dir: &std::path::Path,
    path: &std::ffi::OsStr,
) -> std::ffi::OsString {
    let mut parts: Vec<std::path::PathBuf> = std::env::split_paths(path)
        .filter(|p| p.as_path() != dir)
        .collect();
    parts.insert(0, dir.to_path_buf());
    std::env::join_paths(parts).unwrap_or_else(|_| path.to_os_string())
}

#[cfg(test)]
#[path = "tests/resolve_aida_exe_tests.rs"]
mod resolve_aida_exe_tests;

#[cfg(test)]
#[path = "tests/story_1418_completion_seam_tests.rs"]
mod story_1418_completion_seam_tests;

/// BUG-376: tests that pin both substrate halves of the implementer-
/// complete signal — the `aida pr ship` banner emits the load-bearing
/// "EXIT NOW" phrases, and the `aida-implement.md` skill template
/// embeds the matching directive so an implementer agent picking up
/// the skill at session start sees the same rule the substrate enforces
/// at session end. Together they cover acceptance #3 ("run 'aida queue
/// work TASK-X' on a no-op test spec, confirm agent receives the exit
/// directive") at the unit level: the skill is what the agent reads on
/// the way in, the banner is what it reads on the way out.
#[cfg(test)]
#[path = "tests/bug_376_implementer_exit_directive_tests.rs"]
mod bug_376_implementer_exit_directive_tests;

/// The real [`auto_complete::PhaseDriver`]. Holds the project root + the
/// state discovered as phases run (branch, lease id, PR number).
/// trace:STORY-246 | ai:claude
/// BUG-311: build the argv for the orchestrator's phase-1 `aida queue work`
/// subprocess. Pure helper so the steal-threading + headless flag rules are
/// pinnable by unit test without spawning a process. The orchestrator always
/// sets `--session-id` (so it can locate the phase's lease via
/// `claude_session_id`), and conditionally appends `--steal`, `--no-human`,
/// and `--permission-mode <m>`. Order matches the existing inline code.
/// trace:BUG-311 | ai:claude
pub(crate) fn build_implementer_phase_args(
    spec: &str,
    session_uuid: &str,
    steal: bool,
    force_claim: bool,
    branch: Option<&str>,
    path: Option<&std::path::Path>,
    headless_implementer: bool,
    permission_mode: Option<&str>,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "queue".into(),
        "work".into(),
        spec.into(),
        "--session-id".into(),
        session_uuid.into(),
    ];
    // BUG-311: the user's outer `--steal` is threaded so phase 1's
    // `handle_queue_work` can clear a dormant lease on this scope instead of
    // bailing with the canned "pass --steal" message — which was the BUG-311
    // symptom (the flag was passed but the inner subprocess never saw it).
    if steal {
        args.push("--steal".into());
    }
    if force_claim {
        args.push("--force-claim".into());
    }
    if let Some(branch) = branch {
        args.push("--branch".into());
        args.push(branch.into());
    }
    if let Some(path) = path {
        args.push("--path".into());
        args.push(path.to_string_lossy().into_owned());
    }
    // STORY-276: under `--no-human=both` the implementer phase runs headless;
    // `ReviewerOnly` (and a plain `--auto-complete`) leave phase 1 interactive.
    // trace:STORY-276 | ai:claude
    if headless_implementer {
        args.push("--no-human".into());
    }
    if let Some(pm) = permission_mode {
        args.push("--permission-mode".into());
        args.push(pm.into());
    }
    args
}

/// TASK-262: the argv `RealPhaseDriver::attempt_phase3_auto_rebase` hands to
/// the `aida` subprocess for a one-shot clean rebase of the reviewer-phase PR.
/// Factored out of the inline `Command` builder so the subprocess wiring (the
/// `pr rebase <N> --no-smoke` contract) is unit-testable without spawning.
// trace:TASK-262 | ai:claude
pub(crate) fn build_phase3_auto_rebase_args(pr_number: u64) -> Vec<String> {
    vec![
        "pr".into(),
        "rebase".into(),
        pr_number.to_string(),
        "--no-smoke".into(),
    ]
}

/// BUG-1295: classify a finished `aida pr rebase <N> --no-smoke` subprocess
/// by its EXIT CODE alone — never by matching its stdout/stderr text. Pure
/// so the classification is unit-testable without spawning: feed it a
/// process exit code and it names the `FailureKind` the orchestrator routes
/// on, independent of whatever prose the bail site that produced that code
/// happened to print. `None` covers every other outcome (unrecognized code,
/// signal death) — the caller falls back to an untyped
/// `auto_complete::PhaseFailure`.
// trace:BUG-1295 | ai:claude
pub(crate) fn classify_rebase_subprocess_exit(
    code: Option<i32>,
) -> Option<auto_complete::FailureKind> {
    match code {
        Some(c) if c == pr_rebase::REBASE_EXIT_CODE_REFUSED => {
            Some(auto_complete::FailureKind::StaleBaseRefused)
        }
        Some(c) if c == pr_rebase::REBASE_EXIT_CODE_CONFLICT => {
            Some(auto_complete::FailureKind::StaleBaseConflict)
        }
        _ => None,
    }
}

/// BUG-1295: the process exit code `main_entry` returns for a top-level
/// error. Pulled out of `main_entry` so it's unit-testable: a
/// `pr_rebase::RebaseFailureExit` (the sentinel a `pr rebase` conflict /
/// force-push-refusal bail site wraps its message in) carries its own exit
/// code; every other error keeps the historical exit code 1. This is the
/// producer-side half of the exit-code contract `classify_rebase_subprocess_exit`
/// reads on the consumer side.
// trace:BUG-1295 | ai:claude
pub(crate) fn exit_code_for_error(err: &anyhow::Error) -> i32 {
    // trace:TASK-1606 | ai:codex
    if let Some(failure) = err.downcast_ref::<pr_ship::ShipFailure>() {
        return failure.reason as i32;
    }
    err.downcast_ref::<pr_rebase::RebaseFailureExit>()
        .map(|sig| sig.code)
        .unwrap_or(1)
}

/// STORY-335: the argv `aida queue integrate --rebase` hands to the `aida`
/// subprocess to rebase a ready member's PR branch onto current main before
/// merging it. Mirrors the phase-3 auto-rebase (`pr rebase <N> --no-smoke`):
/// the local smoke is skipped because the subsequent `--from-pr` drive runs CI.
/// Factored out so the subprocess contract is unit-testable without spawning.
/// trace:STORY-335 | ai:claude
pub(crate) fn build_integrate_rebase_args(pr_number: u32) -> Vec<String> {
    vec![
        "pr".into(),
        "rebase".into(),
        pr_number.to_string(),
        "--no-smoke".into(),
    ]
}

// trace:STORY-1033 | ai:codex
pub(crate) fn agent_seat_for_phase(
    phase: auto_complete::Phase,
) -> aida_core::agents_config::AgentSeat {
    match phase {
        auto_complete::Phase::Implementer | auto_complete::Phase::Ci => {
            aida_core::agents_config::AgentSeat::Implementer
        }
        auto_complete::Phase::Reviewer => aida_core::agents_config::AgentSeat::Reviewer,
        auto_complete::Phase::Merge | auto_complete::Phase::Pull | auto_complete::Phase::Build => {
            aida_core::agents_config::AgentSeat::Integrator
        }
    }
}

/// TASK-1080: live three-state probe — is `branch` GONE on origin?
/// `git ls-remote --exit-code origin refs/heads/<branch>` distinguishes
/// "ref absent" (exit 2 → `Some(true)`, the merged+deleted signature) from
/// "ref present" (exit 0 → `Some(false)`) from "couldn't tell" (spawn error /
/// offline / auth → `None`). The three states matter: the stacked-promotion
/// path must never read an offline probe as "parent merged" — a false
/// promotion would rewrite a PR branch that still depends on an open parent.
// trace:TASK-1080 | ai:claude
pub(crate) fn remote_branch_gone(project_root: &std::path::Path, branch: &str) -> Option<bool> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args([
            "ls-remote",
            "--exit-code",
            "origin",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .ok()?;
    match out.status.code() {
        Some(0) => Some(false),
        Some(2) => Some(true),
        _ => None,
    }
}

/// TASK-262: the argv `RealPhaseDriver::auto_punt_text_question` hands to the
/// `aida` subprocess to record a headless implementer's design-fork punt.
/// Factored out of the inline `Command` builder so the subprocess wiring (the
/// `punt <spec> --category design-fork --reason <r> --lean <l>` contract) is
/// unit-testable without spawning.
// trace:TASK-262 | ai:claude
pub(crate) fn build_auto_punt_args(spec: &str, reason: &str, lean: &str) -> Vec<String> {
    vec![
        "punt".into(),
        spec.into(),
        "--category".into(),
        "design-fork".into(),
        "--reason".into(),
        reason.into(),
        "--lean".into(),
        lean.into(),
    ]
}

/// TASK-136 / BUG-420: drain-reliability tunables for the orchestrator's phase
/// loop. Resolved once per `RealPhaseDriver` from the `[drain]` section of
/// `.aida/config.toml`, then overridden by env vars (which the `queue work`
/// dispatch sets from the `--no-progress-minutes` / `--phase-ceiling-minutes`
/// flags so the values compose across single-spec / batch / nextN drains
/// without threading through every signature — the same env-var pattern
/// `AIDA_CALIBRATE` uses). A `0`-minute value disables that watchdog check.
/// trace:TASK-136 BUG-420 | ai:claude
#[derive(Debug, Clone, Copy)]
pub(crate) struct DrainTuning {
    /// TASK-136: how many times to retry the phase-1 GH PR-verify on a
    /// transient GH-API outage before declaring the run inconclusive. The
    /// backoff cadence is [`auto_complete::gh_verify_backoff_schedule`]
    /// (30s / 1m / 5m / then 15m). Default 3.
    pub(crate) gh_verify_retries: usize,
    /// BUG-420: no-progress window — a headless phase that makes no commit /
    /// file-change for this long is killed and shelved. Default 10m; `0`
    /// disables.
    pub(crate) no_progress: std::time::Duration,
    /// BUG-420: hard wall-clock ceiling per headless phase, a backstop in case
    /// progress-detection misses. Default 45m; `0` disables.
    pub(crate) ceiling: std::time::Duration,
    /// STORY-998: low-information output must repeat for this long before the
    /// drain shelves it as a spinning session. Default 90s; `0` disables.
    pub(crate) spinning_after: std::time::Duration,
    /// STORY-998: high line rate trips runaway before the window is pruned.
    pub(crate) runaway_rate: f64,
    /// STORY-998: maximum Shannon entropy for the repeated-template window.
    pub(crate) low_entropy_bits: f64,
    /// STORY-998: no information-bearing line for this long is Quiet.
    pub(crate) progress_activity: std::time::Duration,
    /// TASK-975: CI auto-fix budget — how many in-drain headless fix cycles
    /// phase 2 may attempt on a red CI run before the failure proceeds to
    /// shelve. Also arms the phase-4 merge-conflict rebase when > 0.
    /// Default 0 (off — red CI shelves immediately, the pre-TASK-975
    /// behaviour).
    pub(crate) ci_auto_fix: usize,
    /// STORY-975: whole-phase retry budget for transient shelve causes.
    /// Default 1 retry, clamped to 3.
    // trace:STORY-975 | ai:codex
    pub(crate) retry_transient: usize,
    /// STORY-1033: retry attempt 2 may move to the next configured model tier.
    /// Defaults true; `[drain] retry_escalate_model = false` disables it.
    pub(crate) retry_escalate_model: bool,
    /// STORY-1041: in-flight member window for pipelined drains. Clamped 1..3.
    // trace:STORY-1041 trace:ADR-27 | ai:codex
    pub(crate) pipeline_depth: usize,
}

impl DrainTuning {
    /// Resolve from `[drain]` config then env overrides. Env wins so the
    /// per-invocation flags (mapped to env at dispatch) override the project
    /// default. trace:TASK-136 BUG-420 | ai:claude
    pub(crate) fn resolve(project_root: &std::path::Path) -> Self {
        let cfg = read_drain_config(project_root);
        let env_usize = |k: &str| -> Option<usize> {
            std::env::var(k).ok().and_then(|s| s.trim().parse().ok())
        };
        let env_u64 =
            |k: &str| -> Option<u64> { std::env::var(k).ok().and_then(|s| s.trim().parse().ok()) };
        let env_f64 =
            |k: &str| -> Option<f64> { std::env::var(k).ok().and_then(|s| s.trim().parse().ok()) };
        let gh_verify_retries = env_usize("AIDA_GH_VERIFY_RETRIES")
            .or(cfg.gh_verify_retries)
            .unwrap_or(3);
        let no_progress_min = env_u64("AIDA_NO_PROGRESS_MINUTES")
            .or(cfg.no_progress_minutes)
            .unwrap_or(10);
        let ceiling_min = env_u64("AIDA_PHASE_CEILING_MINUTES")
            .or(cfg.phase_ceiling_minutes)
            .unwrap_or(45);
        let idle_defaults = aida_core::idle::IdleConfig::default();
        let spinning_after_secs = env_u64("AIDA_SPINNING_AFTER_SECS")
            .or(cfg.spinning_after)
            .unwrap_or(idle_defaults.spinning_after.as_secs());
        let runaway_rate = env_f64("AIDA_RUNAWAY_RATE")
            .or(cfg.runaway_rate)
            .unwrap_or(idle_defaults.runaway_rate);
        let low_entropy_bits = env_f64("AIDA_LOW_ENTROPY_BITS")
            .or(cfg.low_entropy_bits)
            .unwrap_or(idle_defaults.low_entropy_bits);
        let progress_activity_secs = env_u64("AIDA_PROGRESS_ACTIVITY_SECS")
            .or(cfg.progress_activity)
            .unwrap_or(idle_defaults.progress_activity.as_secs());
        // trace:TASK-975 | ai:claude
        let ci_auto_fix = env_usize("AIDA_CI_AUTO_FIX")
            .or(cfg.ci_auto_fix)
            .unwrap_or(0);
        let retry_transient = auto_complete::clamp_transient_retry_budget(
            env_usize("AIDA_RETRY_TRANSIENT")
                .or(cfg.retry_transient)
                .unwrap_or(1),
        );
        let retry_escalate_model = std::env::var("AIDA_RETRY_ESCALATE_MODEL")
            .ok()
            .and_then(|s| parse_boolish(&s))
            .or(cfg.retry_escalate_model)
            .unwrap_or(true);
        let pipeline_depth = drain_state::clamp_pipeline_depth(
            env_usize("AIDA_DRAIN_PIPELINE_DEPTH")
                .or(cfg.pipeline_depth)
                .unwrap_or_else(drain_state::default_pipeline_depth),
        );
        Self {
            gh_verify_retries,
            no_progress: std::time::Duration::from_secs(no_progress_min.saturating_mul(60)),
            ceiling: std::time::Duration::from_secs(ceiling_min.saturating_mul(60)),
            spinning_after: std::time::Duration::from_secs(spinning_after_secs),
            runaway_rate,
            low_entropy_bits,
            progress_activity: std::time::Duration::from_secs(progress_activity_secs),
            ci_auto_fix,
            retry_transient,
            retry_escalate_model,
            pipeline_depth,
        }
    }

    // trace:STORY-998 | ai:codex
    pub(crate) fn idle_config(&self) -> aida_core::idle::IdleConfig {
        aida_core::idle::IdleConfig {
            spinning_after: self.spinning_after,
            runaway_rate: self.runaway_rate,
            low_entropy_bits: self.low_entropy_bits,
            progress_activity: self.progress_activity,
            ..aida_core::idle::IdleConfig::default()
        }
    }

    // trace:STORY-1041 trace:ADR-27 | ai:codex
    pub(crate) fn pipeline_depth(&self) -> usize {
        self.pipeline_depth
    }
}

/// Format the periodic liveness heartbeat an otherwise-silent headless phase
/// emits every ~30s — actor + spec + elapsed + time since the last observed
/// activity. The watchdog already computes "time since last progress" to decide
/// when to KILL a degenerate session; this turns that same signal into
/// reassurance so the operator can tell "working hard" from "hung". Pure so the
/// elapsed / since-progress formatting is pinned by a unit test (no real drive).
// trace:STORY-726 | ai:claude
pub(crate) fn phase_heartbeat_line(
    actor: &str,
    spec: &str,
    elapsed: std::time::Duration,
    since_progress: std::time::Duration,
) -> String {
    let e = elapsed.as_secs();
    format!(
        "{actor} working on {spec} ({}m {}s elapsed, last activity {}s ago)",
        e / 60,
        e % 60,
        since_progress.as_secs(),
    )
}

/// BUG-875: progress signals are phase-shaped. Implementers can prove progress
/// through the worktree or the stream log; reviewers are read-then-verdict, so
/// their no-progress watchdog must key on stream output activity only.
// trace:BUG-875 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WatchdogProgressSignal {
    WorktreeAndOutput,
    OutputOnly,
}

// trace:BUG-875 | ai:codex
pub(crate) fn watchdog_progress_signal_for_phase(
    phase: auto_complete::Phase,
) -> WatchdogProgressSignal {
    match phase {
        auto_complete::Phase::Reviewer => WatchdogProgressSignal::OutputOnly,
        _ => WatchdogProgressSignal::WorktreeAndOutput,
    }
}

/// BUG-420: a phase-scoped no-progress + wall-clock-ceiling watchdog for a
/// *headless* orchestrator phase. While the `claude -p` child runs, the
/// orchestrator's [`exit_signal::spawn_and_wait_watched`] poll loop calls
/// [`PhaseWatchdog::check`]; it (rate-limited) probes the phase worktree for
/// the phase-appropriate progress signal and trips when it stalls for
/// `no_progress` or when wall-clock exceeds `ceiling`. The pure verdict is
/// [`auto_complete::watchdog_verdict`]; this struct is just the I/O shell that
/// gathers the facts and remembers the last-progress timestamp.
///
/// The worktree is created by the phase child, so it is not known at spawn
/// time — `check` resolves it lazily from the session lease and, until it
/// appears, treats the phase as "making progress" (startup grace) so a slow
/// session launch never trips the watchdog. trace:BUG-420 | ai:claude
pub(crate) struct PhaseWatchdog {
    pub(crate) project_root: std::path::PathBuf,
    pub(crate) session_id: String,
    pub(crate) vendor: session::HeadlessVendor,
    pub(crate) root_pid: Option<u32>,
    pub(crate) phase_start: std::time::Instant,
    pub(crate) last_progress: std::time::Instant,
    pub(crate) last_sig: Option<String>,
    pub(crate) last_poll: std::time::Instant,
    pub(crate) poll_every: std::time::Duration,
    pub(crate) no_progress: std::time::Duration,
    pub(crate) ceiling: std::time::Duration,
    pub(crate) worktree: Option<std::path::PathBuf>,
    pub(crate) pr_ship_wait_seen: bool,
    pub(crate) progress_signal: WatchdogProgressSignal,
    pub(crate) idle_detector: aida_core::idle::IdleDetector,
    pub(crate) idle_log_pos: u64,
    /// STORY-726: when `Some((actor, spec))`, the watchdog also emits a
    /// liveness heartbeat to stderr on each poll tick where it does NOT trip —
    /// reassurance that an otherwise-silent headless phase is alive. Set only
    /// when the phase would run silent (headless + teeing off); a teed or
    /// interactive phase already streams its own progress.
    // trace:STORY-726 | ai:claude
    pub(crate) heartbeat: Option<(String, String)>,
    /// BUG-1299: the recovery hint [`Self::trip_reason`] resolved alongside
    /// the just-returned trip detail, in the same `watchdog_trip_report`
    /// call — the caller `take`s it right after `check()` reports a trip and
    /// attaches it to the `PhaseFailure` as `hint_override`, so the hint the
    /// operator sees is guaranteed to describe the SAME trip as the detail.
    // trace:BUG-1299 | ai:claude
    pub(crate) last_trip_hint: Option<String>,
}

impl PhaseWatchdog {
    /// How often to actually shell out to git — far coarser than the
    /// `spawn_and_wait` poll cadence (which is ~100ms) so the watchdog never
    /// spins git. trace:BUG-420 | ai:claude
    const GIT_POLL: std::time::Duration = std::time::Duration::from_secs(30);

    pub(crate) fn new(
        project_root: std::path::PathBuf,
        session_id: String,
        vendor: session::HeadlessVendor,
        no_progress: std::time::Duration,
        ceiling: std::time::Duration,
    ) -> Self {
        let now = std::time::Instant::now();
        let idle_defaults = aida_core::idle::IdleConfig::default();
        Self {
            project_root,
            session_id,
            vendor,
            root_pid: None,
            phase_start: now,
            last_progress: now,
            last_sig: None,
            last_poll: now,
            poll_every: Self::GIT_POLL,
            no_progress,
            ceiling,
            worktree: None,
            pr_ship_wait_seen: false,
            progress_signal: WatchdogProgressSignal::WorktreeAndOutput,
            idle_detector: aida_core::idle::IdleDetector::new(idle_defaults),
            idle_log_pos: 0,
            heartbeat: None,
            last_trip_hint: None,
        }
    }

    // trace:STORY-998 | ai:codex
    pub(crate) fn with_idle_config(mut self, cfg: aida_core::idle::IdleConfig) -> Self {
        self.idle_detector = aida_core::idle::IdleDetector::new(cfg);
        self
    }

    // trace:BUG-875 | ai:codex
    pub(crate) fn new_for_phase(
        project_root: std::path::PathBuf,
        session_id: String,
        vendor: session::HeadlessVendor,
        no_progress: std::time::Duration,
        ceiling: std::time::Duration,
        idle_config: aida_core::idle::IdleConfig,
        phase: auto_complete::Phase,
    ) -> Self {
        Self::new(project_root, session_id, vendor, no_progress, ceiling)
            .with_progress_signal(watchdog_progress_signal_for_phase(phase))
            .with_idle_config(idle_config)
    }

    // trace:BUG-875 | ai:codex
    pub(crate) fn with_progress_signal(mut self, progress_signal: WatchdogProgressSignal) -> Self {
        self.progress_signal = progress_signal;
        self
    }

    /// STORY-726: arm the periodic liveness heartbeat. `actor` is the phase
    /// noun ("implementer"), `spec` the SPEC-ID being worked. Call only when the
    /// phase would otherwise be silent so the heartbeat never double-prints over
    /// a teed/interactive child.
    // trace:STORY-726 | ai:claude
    pub(crate) fn with_heartbeat(
        mut self,
        actor: impl Into<String>,
        spec: impl Into<String>,
    ) -> Self {
        self.heartbeat = Some((actor.into(), spec.into()));
        self
    }

    /// Emit one liveness heartbeat (when armed). Mirrors the CI-wait phase's
    /// `CI still running …` reassurance so an unattended drive is never dark.
    // trace:STORY-726 | ai:claude
    pub(crate) fn emit_heartbeat(
        &self,
        since_progress: std::time::Duration,
        total: std::time::Duration,
    ) {
        if let Some((actor, spec)) = &self.heartbeat {
            eprintln!(
                "  {} {}",
                crate::glyph(crate::glyphs::Glyph::InFlight).yellow(),
                phase_heartbeat_line(actor, spec, total, since_progress),
            );
        }
    }

    /// A progress signature for the worktree: HEAD sha + a hash of the
    /// porcelain status + the newest mtime among changed files. A degenerate
    /// echo/sleep spin moves none of these; a real session commits, stages, or
    /// re-edits a file (advancing its mtime). `None` when git can't be read.
    /// trace:BUG-420 | ai:claude
    pub(crate) fn progress_signature(worktree: &std::path::Path) -> Option<String> {
        let sha = git_capture(worktree, &["rev-parse", "HEAD"]).unwrap_or_default();
        let porcelain = git_capture(worktree, &["status", "--porcelain"]).unwrap_or_default();
        // Newest mtime among the changed paths — catches repeated edits to an
        // already-dirty file (porcelain text alone would not change).
        let mut newest: u128 = 0;
        for line in porcelain.lines() {
            // Porcelain lines are `XY <path>`; take everything after the status
            // columns. Rename lines (`R  old -> new`) end with the new path.
            let path = line
                .get(3..)
                .unwrap_or("")
                .rsplit(" -> ")
                .next()
                .unwrap_or("");
            let path = path.trim().trim_matches('"');
            if path.is_empty() {
                continue;
            }
            if let Ok(meta) = std::fs::metadata(worktree.join(path)) {
                if let Ok(m) = meta.modified() {
                    if let Ok(d) = m.duration_since(std::time::UNIX_EPOCH) {
                        newest = newest.max(d.as_nanos());
                    }
                }
            }
        }
        if sha.is_empty() && porcelain.is_empty() {
            return None;
        }
        Some(format!("{sha}|{}|{newest}", porcelain.len()))
    }

    // trace:BUG-875 | ai:codex
    pub(crate) fn select_progress_signature(
        progress_signal: WatchdogProgressSignal,
        worktree_sig: Option<String>,
        output_sig: Option<String>,
    ) -> Option<String> {
        match progress_signal {
            WatchdogProgressSignal::OutputOnly => output_sig,
            WatchdogProgressSignal::WorktreeAndOutput => match (worktree_sig, output_sig) {
                (Some(w), Some(o)) => Some(format!("{w}|{o}")),
                (Some(w), None) => Some(w),
                (None, Some(o)) => Some(o),
                (None, None) => None,
            },
        }
    }

    // trace:BUG-875 | ai:codex
    pub(crate) fn observed_progress_signature(&self, worktree: &std::path::Path) -> Option<String> {
        let ctx = vendor_activity::VendorActivityContext::new(&self.project_root, &self.session_id);
        let output_sig = vendor_activity::snapshot(self.vendor, &ctx).signature();
        let worktree_sig = match self.progress_signal {
            WatchdogProgressSignal::WorktreeAndOutput => Self::progress_signature(worktree),
            WatchdogProgressSignal::OutputOnly => None,
        };
        Self::select_progress_signature(self.progress_signal, worktree_sig, output_sig)
    }

    // trace:STORY-998 | ai:codex
    pub(crate) fn idle_verdict_from_log(
        &mut self,
        now: std::time::Instant,
    ) -> aida_core::idle::IdleVerdict {
        let Some(path) = headless_log_path_for_session(&self.project_root, &self.session_id) else {
            return self.idle_detector.verdict(now);
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            return self.idle_detector.verdict(now);
        };
        let len = meta.len();
        if len < self.idle_log_pos {
            self.idle_log_pos = 0;
        }
        if len > self.idle_log_pos {
            if let Ok(mut file) = std::fs::File::open(&path) {
                use std::io::{BufRead, Seek};
                let _ = file.seek(std::io::SeekFrom::Start(self.idle_log_pos));
                let mut reader = std::io::BufReader::new(file);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    let Ok(n) = reader.read_until(b'\n', &mut line) else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }
                    self.idle_detector.feed_bytes(&line, now);
                }
                if let Ok(pos) = reader.stream_position() {
                    self.idle_log_pos = pos;
                } else {
                    self.idle_log_pos = len;
                }
            }
        }
        self.idle_detector.verdict(now)
    }

    /// Probe (rate-limited) and return `Some(reason)` if the watchdog should
    /// trip — the caller then reaps the child. trace:BUG-420 | ai:claude
    pub(crate) fn check(&mut self) -> Option<String> {
        // Both kill-checks disabled AND no heartbeat to emit → nothing to do.
        // STORY-726: with the heartbeat armed we still poll (on the same coarse
        // cadence) so a silent headless phase stays visibly alive even if a user
        // disabled the no-progress / ceiling kills.
        if self.no_progress.is_zero() && self.ceiling.is_zero() && self.heartbeat.is_none() {
            return None;
        }
        let now = std::time::Instant::now();
        if now.duration_since(self.last_poll) < self.poll_every {
            return None;
        }
        self.last_poll = now;

        // Lazily resolve the worktree from the session lease. Until it appears
        // (the child is still starting up), keep resetting last_progress so a
        // slow launch can't trip the no-progress check.
        if self.worktree.is_none() {
            if let Some((_, _, wt, creator_pid)) =
                find_orchestrated_lease(&self.project_root, &self.session_id)
            {
                self.worktree = Some(wt);
                self.root_pid = creator_pid;
            }
            if self.worktree.is_none() {
                self.last_progress = now;
                // Ceiling still applies even before the worktree resolves.
                return auto_complete::watchdog_verdict(
                    now.duration_since(self.last_progress),
                    now.duration_since(self.phase_start),
                    self.no_progress,
                    self.ceiling,
                )
                .map(|t| self.trip_reason(t));
            }
        }
        let worktree = self.worktree.clone().unwrap();

        // BUG-453 / BUG-875: implementers use worktree change OR output
        // activity; reviewers use output activity only because their normal
        // work is read-then-verdict with no intermediate file edits.
        let combined = self.observed_progress_signature(&worktree);
        if let Some(sig) = combined {
            if self.last_sig.as_deref() != Some(sig.as_str()) {
                self.last_sig = Some(sig);
                self.last_progress = now;
            }
        }
        // BUG-749: once the implementer has opened a PR and is blocked inside
        // `aida pr ship` watching CI, worktree/log movement can legitimately
        // stop. A live pr-ship descendant is therefore progress for the
        // no-progress watchdog; the phase ceiling still applies.
        // trace:BUG-749 | ai:codex
        let pr_ship_waiting = self.root_pid.is_some_and(live_aida_pr_ship_descendant);
        if pr_ship_waiting {
            self.pr_ship_wait_seen = true;
            self.last_progress = now;
        }

        // STORY-998: a session can produce output forever without adding
        // information (e.g. status/redraw loops). Classify the stream itself,
        // but preserve the BUG-749 CI wait exemption.
        if !pr_ship_waiting {
            if let aida_core::idle::IdleVerdict::Spinning { template, count } =
                self.idle_verdict_from_log(now)
            {
                return Some(
                    self.trip_reason(auto_complete::WatchdogTrip::Spinning { template, count }),
                );
            }
        }

        // TASK-298: a `--no-human` headless run that hit a permission gate (or
        // reported an `is_error` envelope) is silently stuck — `claude -p`
        // exits 0 even when it bailed (SPIKE-7), so neither the natural-exit
        // path nor the no-progress timer would catch it promptly. The log
        // keeps the offending event, so scan it and trip the watchdog
        // *immediately* with the specific reason rather than waiting out the
        // no-progress / ceiling window. trace:TASK-298 | ai:claude
        if let Some(reason) = read_headless_log_for_session(&self.project_root, &self.session_id)
            .as_deref()
            .and_then(headless_log_permission_stall)
        {
            return Some(reason);
        }

        let since_progress = now.duration_since(self.last_progress);
        let total = now.duration_since(self.phase_start);
        let verdict = auto_complete::watchdog_verdict_with_ci_wait(
            since_progress,
            total,
            self.no_progress,
            self.ceiling,
            pr_ship_waiting,
        );
        // STORY-726: not tripping → the phase is alive; emit the reassurance
        // heartbeat (when armed) using the very signal the watchdog would
        // otherwise only weaponize to kill.
        if verdict.is_none() {
            self.emit_heartbeat(since_progress, total);
        }
        verdict.map(|t| self.trip_reason(t))
    }

    /// BUG-1299: resolve BOTH the failure detail (returned) and the recovery
    /// hint (stashed on `self.last_trip_hint`, fetched by the caller via
    /// [`Self::take_trip_hint`]) from `auto_complete::watchdog_trip_report` —
    /// one function, one match on `trip`, so the two strings the operator
    /// eventually sees cannot name different trips.
    // trace:BUG-1299 | ai:claude
    pub(crate) fn trip_reason(&mut self, trip: auto_complete::WatchdogTrip) -> String {
        let output_only = matches!(self.progress_signal, WatchdogProgressSignal::OutputOnly);
        let (detail, hint) = auto_complete::watchdog_trip_report(
            &trip,
            self.no_progress,
            self.ceiling,
            self.pr_ship_wait_seen,
            output_only,
        );
        self.last_trip_hint = Some(hint);
        detail
    }

    /// Take the recovery hint resolved alongside the most recent trip detail
    /// [`Self::check`] returned. `None` once taken, or if the watchdog never
    /// tripped.
    // trace:BUG-1299 | ai:claude
    pub(crate) fn take_trip_hint(&mut self) -> Option<String> {
        self.last_trip_hint.take()
    }
}

/// BUG-749: recognize commands equivalent to `aida pr ship`, including
/// explicit binary paths (`target/debug/aida pr ship`) and shell wrappers
/// (`bash -lc 'aida pr ship 123'`). Kept pure so the watchdog regression does
/// not need to spawn or block real processes.
// trace:BUG-749 | ai:codex
pub(crate) fn command_line_runs_aida_pr_ship(parts: &[String]) -> bool {
    if parts.len() < 3 {
        return false;
    }
    let mut words = Vec::new();
    for part in parts {
        words.extend(part.split_whitespace().map(str::to_string));
    }
    words.windows(3).any(|w| {
        std::path::Path::new(&w[0])
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == "aida")
            && w[1] == "pr"
            && w[2] == "ship"
    })
}

/// Best-effort local process-table probe for a live `aida pr ship` descendant
/// under the orchestrated implementer session. Failure to inspect the table is
/// conservative: no CI-wait signal, so the existing watchdog behavior remains.
// trace:BUG-749 | ai:codex
pub(crate) fn live_aida_pr_ship_descendant(root_pid: u32) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};

    let root = Pid::from_u32(root_pid);
    let mut sys =
        System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));
    sys.refresh_processes_specifics(ProcessRefreshKind::new());

    let mut tree = vec![root];
    let mut idx = 0;
    while idx < tree.len() {
        let parent = tree[idx];
        idx += 1;
        for (pid, proc_) in sys.processes() {
            if proc_.parent() == Some(parent) && !tree.contains(pid) {
                tree.push(*pid);
                let parts: Vec<String> = proc_.cmd().iter().map(|p| p.to_string()).collect();
                if command_line_runs_aida_pr_ship(&parts) {
                    return true;
                }
            }
        }
    }
    false
}

/// Run `git -C <worktree> <args>` and capture trimmed stdout, or `None` on any
/// spawn / non-zero / decode error. trace:BUG-420 | ai:claude
pub(crate) fn git_capture(worktree: &std::path::Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The `[drain]` config values, all optional. trace:TASK-136 BUG-420 | ai:claude
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct DrainConfigToml {
    pub(crate) gh_verify_retries: Option<usize>,
    pub(crate) no_progress_minutes: Option<u64>,
    pub(crate) phase_ceiling_minutes: Option<u64>,
    pub(crate) spinning_after: Option<u64>,
    pub(crate) runaway_rate: Option<f64>,
    pub(crate) low_entropy_bits: Option<f64>,
    pub(crate) progress_activity: Option<u64>,
    /// TASK-975: `[drain] ci_auto_fix = N` — how many in-drain CI-fix cycles
    /// phase 2 may attempt on a red CI run before shelving. Also arms the
    /// phase-4 merge-conflict rebase when > 0. Absent/0 = off.
    pub(crate) ci_auto_fix: Option<usize>,
    /// STORY-975: `[drain] retry_transient = N` — whole-phase retries for
    /// transient typed shelve causes. Default 1, max 3.
    // trace:STORY-975 | ai:codex
    pub(crate) retry_transient: Option<usize>,
    /// STORY-1033: `[drain] retry_escalate_model = true|false`.
    pub(crate) retry_escalate_model: Option<bool>,
    /// STORY-1041: `[drain] pipeline_depth = 1..3`.
    // trace:STORY-1041 trace:ADR-27 | ai:codex
    pub(crate) pipeline_depth: Option<usize>,
    /// BUG-1275: tracked CI wait bounds. Durations accept seconds, `s`, or `m`.
    pub(crate) ci_idle: Option<u64>,
    pub(crate) ci_absolute: Option<u64>,
    /// STORY-1414: `[drain] require_head = true` — refuse a drain launch on a
    /// stale dev binary instead of warning.
    // trace:STORY-1414 | ai:claude
    pub(crate) require_head: Option<bool>,
}

/// Hand-rolled `[drain]`-section scanner for `.aida/config.toml`, mirroring the
/// other section readers (e.g. `read_behavior_permission_mode`) so the crate
/// stays serde-free for one small optional section. Unknown keys are ignored.
/// trace:TASK-136 BUG-420 | ai:claude
pub(crate) fn read_drain_config(project_dir: &std::path::Path) -> DrainConfigToml {
    let mut out = DrainConfigToml::default();
    let config_path = project_dir.join(".aida").join("config.toml");
    let Ok(content) = std::fs::read_to_string(&config_path) else {
        return out;
    };
    let mut in_drain = false;
    for raw in content.lines() {
        let line = strip_toml_inline_comment(raw).trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            in_drain = rest.trim_start_matches('[').starts_with("drain");
            continue;
        }
        if !in_drain {
            continue;
        }
        if let Some((key, val)) = line.split_once('=') {
            let key = key.trim();
            let val = val.trim().trim_matches('"').trim();
            match key {
                "gh_verify_retries" => out.gh_verify_retries = val.parse().ok(),
                "no_progress_minutes" => out.no_progress_minutes = val.parse().ok(),
                "phase_ceiling_minutes" => out.phase_ceiling_minutes = val.parse().ok(),
                "spinning_after" => out.spinning_after = parse_duration_seconds(val),
                "runaway_rate" => out.runaway_rate = val.parse().ok(),
                "low_entropy_bits" => out.low_entropy_bits = val.parse().ok(),
                "progress_activity" => out.progress_activity = parse_duration_seconds(val),
                // trace:TASK-975 | ai:claude
                "ci_auto_fix" => out.ci_auto_fix = val.parse().ok(),
                "retry_transient" => out.retry_transient = val.parse().ok(),
                "retry_escalate_model" => out.retry_escalate_model = parse_boolish(val),
                "pipeline_depth" => out.pipeline_depth = val.parse().ok(),
                "ci_idle" => out.ci_idle = parse_duration_seconds(val),
                "ci_absolute" => out.ci_absolute = parse_duration_seconds(val),
                "require_head" => out.require_head = parse_boolish(val),
                _ => {}
            }
        }
    }
    out
}

// trace:STORY-1033 | ai:codex
pub(crate) fn parse_boolish(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

// trace:STORY-998 | ai:codex
pub(crate) fn parse_duration_seconds(raw: &str) -> Option<u64> {
    let trimmed = raw.trim().trim_matches('"');
    if let Some(secs) = trimmed.strip_suffix('s') {
        return secs.trim().parse().ok();
    }
    if let Some(mins) = trimmed.strip_suffix('m') {
        return mins
            .trim()
            .parse::<u64>()
            .ok()
            .map(|m| m.saturating_mul(60));
    }
    trimmed.parse().ok()
}

pub(crate) struct RealPhaseDriver {
    pub(crate) project_root: std::path::PathBuf,
    /// BUG-1037: the auto-complete lifecycle forge, resolved once from the
    /// target project root and reused by phase consumers. This intentionally
    /// uses the open-change resolver so a stale pure-git config cannot make
    /// reviewer preflights disagree with auto-open on a GitHub-origin repo.
    // trace:BUG-1037 | ai:codex
    pub(crate) lifecycle_forge: crate::forge::ForgeKind,
    /// TASK-1421: injectable forge constructor. `None` in every production
    /// path (the driver falls back to `forge_for_kind`); a test sets it to
    /// observe the forge calls the publication boundary makes.
    // trace:TASK-1421 | ai:claude
    pub(crate) forge_factory: Option<crate::forge::ForgeFactory>,
    pub(crate) spec: String,
    /// Queue owner captured when the drain selected this pipeline member. Phase
    /// children keep this identity while their role changes per phase.
    // trace:BUG-1038 | ai:codex
    pub(crate) queue_user_id: String,
    pub(crate) permission_mode: Option<String>,
    pub(crate) json: bool,
    pub(crate) branch: Option<String>,
    pub(crate) implementer_lease: Option<String>,
    pub(crate) pr_number: Option<u32>,
    /// BUG-1244: PR proven by phase 1 (or explicitly seeded resume) for this
    /// run. CI/reviewer probes must remain on this PR.
    // trace:BUG-1244 | ai:codex
    pub(crate) phase_done_pr: Option<u32>,
    /// Exact commit produced by phase 1 and bound to `phase_done_pr`.
    // trace:BUG-1818 | ai:codex
    pub(crate) phase_done_head: Option<String>,
    pub(crate) ci_run_id: Option<String>,
    /// PR head for which phase 2 observed terminal CI. Phase 3 may only review
    /// this exact head and records the conclusion in its verdict artifact.
    // trace:BUG-1460 | ai:codex
    pub(crate) ci_terminal_sha: Option<String>,
    pub(crate) ci_terminal_green: Option<bool>,
    /// Cached aida binary path, resolved once at construction. Re-resolving
    /// per-call broke in BUG-217 when the implementer's phase-1 `cargo build`
    /// replaced the running dev binary: `/proc/self/exe` then includes a
    /// " (deleted)" suffix, and `Command::new("<path> (deleted)").spawn()`
    /// fails with ENOENT. trace:BUG-217 | ai:claude
    pub(crate) aida_exe: std::path::PathBuf,
    /// STORY-263: headless mode. `Some` → the reviewer phase is launched
    /// headless (`claude -p`); the implementer phase stays interactive in
    /// this cut (STORY-276 wires it). `None` → fully interactive.
    pub(crate) no_human: Option<auto_complete::NoHumanMode>,
    /// ADR-10: the run's typed autonomy mode, resolved ONCE at dispatch (via
    /// [`AutonomyMode::for_auto_complete_run`]) and carried here alongside
    /// `no_human` so the engine's in-process zen branches consult this typed
    /// field instead of re-reading the bare `AIDA_ZEN` env var. `AIDA_ZEN`
    /// survives strictly as the cross-process TRANSPORT to spawned phase
    /// children / skill templates; this field is the in-process SOURCE OF
    /// TRUTH. Symmetric with `no_human`, closing the `--zen`/`--no-human`
    /// asymmetry ADR-7 named.
    // trace:ADR-10 | ai:claude
    pub(crate) autonomy_mode: AutonomyMode,
    /// TASK-329: poll cadence + SIGTERM→SIGKILL grace for the graceful-exit
    /// signal. Resolved once from `AIDA_EXIT_POLL_MS` / `AIDA_EXIT_GRACE_MS`.
    pub(crate) exit_cfg: exit_signal::ExitSignalConfig,
    /// BUG-233: the corroboration token for this orchestrator run. Passed to
    /// every phase child as `AIDA_AUTO_COMPLETE_TOKEN` so the child can verify
    /// — against the live marker file — that its `AIDA_AUTO_COMPLETE=1` comes
    /// from a real orchestrator rather than guessing.
    pub(crate) run_token: String,
    /// STORY-306: the Claude `--session-id` the phase-1 implementer was
    /// launched with. When phase 1 punts and the advisor tier resolves the
    /// fork, `resume_implementer` `--resume`s exactly this session so the
    /// implementer re-enters its own conversation with the working model it
    /// had built. `None` until `run_implementer` mints it.
    pub(crate) implementer_session: Option<String>,
    /// STORY-306: the worktree the phase-1 implementer ran in. Required by
    /// `resume_implementer`: `claude --resume` finds the persisted session
    /// only when its cwd matches the cwd the session was created with (Claude
    /// derives the project slug under `~/.claude/projects/<slug>/` from cwd),
    /// so the resume must `current_dir(<this worktree>)`. `None` until
    /// `run_implementer` locates the lease.
    pub(crate) implementer_worktree: Option<std::path::PathBuf>,
    /// BUG-311: thread the user's `aida queue work --auto-complete --steal`
    /// flag through to the phase-1 implementer subprocess. Without this the
    /// outer `--steal` is dropped on the floor — phase 1's `handle_queue_work`
    /// sees the dormant lease, bails with the canned "pass --steal" message,
    /// and the orchestrator reports a generic phase-1 failure even though
    /// the user *did* pass --steal. trace:BUG-311 | ai:claude
    pub(crate) steal: bool,
    /// TASK-559: thread the user's `aida queue work --auto-complete
    /// --force-claim` flag through to the phase-1 implementer subprocess.
    pub(crate) force_claim: bool,
    /// STORY-281: opt out of the reviewer pre-flight stale-base refusal.
    /// When false (default), phase 3 refuses to launch the reviewer if the
    /// PR's base is behind origin AND a file the PR touches has moved on
    /// the base since the PR forked. When true, the refusal becomes a
    /// warning so review-against-stale proceeds. Also propagated to the
    /// reviewer subprocess so its own pre-flight respects the opt-out.
    /// trace:STORY-281 | ai:claude
    pub(crate) allow_stale_base: bool,
    /// STORY-429: opt out of phase-3 auto-rebase recovery.
    pub(crate) no_auto_rebase: bool,
    /// STORY-429: embedded telemetry for phase-3 stale-base auto-rebase.
    pub(crate) auto_rebase_events: Vec<auto_complete_telemetry::AutoRebaseEvent>,
    /// STORY-442: opt-in lifecycle short-circuit tags for non-integrity
    /// phases. Phase 2 owns CI waiting; phases 3/6 are sequenced in
    /// `auto_complete.rs`.
    pub(crate) lifecycle_skip: auto_complete::LifecycleSkip,
    /// BUG-742: the auto-complete stop mode this drive was launched with.
    /// Phase children receive it via `AIDA_AUTO_COMPLETE_VARIANT` so their
    /// pickup prompt says "open the PR and stop" instead of the direct-ship
    /// cadence. The merge bouncer does not trust this value; it only informs
    /// child-session guidance.
    // trace:BUG-742 | ai:codex
    pub(crate) variant: auto_complete::AutoCompleteVariant,
    /// BUG-868: this drive entered through `queue work --from-pr`, so phase 3
    /// must tell the reviewer that a Done spec is expected and the concrete
    /// contract is reviewing the already-open PR, not queue cleanup.
    // trace:BUG-868 | ai:codex
    pub(crate) from_pr: bool,
    /// TASK-136 / BUG-420: GH-verify retry budget + watchdog thresholds,
    /// resolved once from `[drain]` config + env/flag overrides.
    pub(crate) drain_tuning: DrainTuning,
    /// BUG-826: bounded retry count for an instant headless-vendor death that
    /// leaves a zero-byte log. This is phase-1 launch resilience, not a retry
    /// of real implementer work.
    // trace:BUG-826 | ai:codex
    pub(crate) empty_launch_retries_used: usize,
    /// BUG-908: a transient phase-1 retry re-enters the predecessor worktree
    /// after releasing its dead lease, instead of creating a fresh worktree and
    /// colliding with the dirty WIP the first attempt intentionally preserved.
    // trace:BUG-908 | ai:codex
    pub(crate) retry_implementer_worktree: Option<std::path::PathBuf>,
    pub(crate) retry_implementer_branch: Option<String>,
    /// BUG-1629: the phase-1 workspace pinned for launch — the value the
    /// orchestrator checked, never a second resolution.
    // trace:BUG-1629 | ai:claude
    pub(crate) phase1_workspace: Option<Phase1Workspace>,
    /// BUG-1629: the single clean replacement launch spent when a phase-1
    /// child exits without its session lease and handoff receipt.
    // trace:BUG-1629 | ai:claude
    pub(crate) lost_child_state_retries_used: usize,
    /// BUG-1769: the spec's store status read ONCE at phase-1 entry, before any
    /// child launches. [`RealPhaseDriver::recover_lost_child_state`] compares it
    /// against a fresh read to tell "this run finished the work" (the BUG-1140
    /// substrate verification) from the stale-Done refusal BUG-1524 must keep
    /// surfacing.
    ///
    /// Set by [`RealPhaseDriver::capture_phase1_entry_spec_status`], called at
    /// the top of `RealPhaseDriver::run_implementer`. The OUTER `Option` is the
    /// captured/not-captured flag, not a status: `run_implementer` is re-entered
    /// by the BUG-1629 replacement launch and the BUG-826 relaunches, and a
    /// second capture would overwrite the entry reading with a mid-run one and
    /// destroy the comparison. The INNER `Option` is `spec_status`'s own
    /// "no store, or the spec is not in it".
    // trace:BUG-1769 | ai:claude
    pub(crate) phase1_entry_spec_status: Option<Option<RequirementStatus>>,
    /// BUG-1213 / TASK-1265: `(PR, dispatched branch, blocking verdict's
    /// reviewed_sha, the dispatched branch's head AT ARM TIME, authoritative
    /// review delta, round)` captured immediately before a rework implementer
    /// runs. `None` for ordinary first-pass work (no blocking verdict exists,
    /// so the no-op guard must never fire).
    /// TASK-1449 (rework, common-path regression): `reviewed_sha` is
    /// `Option` because ~86% of the verdict corpus carries no sha. That is
    /// UNKNOWN, not "no refusal", but it must not become an automatic
    /// refusal either — `rework_no_op_failure` falls back to the arm-time
    /// head as the comparison baseline and only refuses when NEITHER is
    /// available.
    // trace:BUG-1213 trace:TASK-1265 trace:TASK-1449 | ai:claude
    pub(crate) rework_guard: Option<(u32, String, Option<String>, Option<String>, String, usize)>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Phase3StaleOverlapAction {
    Proceed,
    Refuse(auto_complete::PhaseFailure),
}

/// TASK-136: the outcome of one phase-1 PR-verify attempt. The verify can
/// settle definitively (a PR found, no PR, or a hard failure) or hit a
/// *transient* GH-API outage that leaves it unable to confirm a PR — only the
/// last case is retried (with the [`auto_complete::gh_verify_backoff_schedule`]
/// backoff). trace:TASK-136 | ai:claude
/// BUG-444: given that BOTH phase-1 PR lookups returned empty-but-successful,
/// decide whether that's a *definitive* no-PR (the branch was never pushed) or a
/// *retryable* eventual-consistency window (the branch is on origin, so a PR
/// likely exists but isn't indexed yet). Only an `Absent` branch is a definitive
/// NoPr; `Present` and `LsRemoteFailed` both retry. Pure so the keystone
/// decision is unit-tested without gh/git. trace:BUG-444 | ai:claude
pub(crate) fn empty_phase1_lookup_is_definitive_nopr(origin: &BranchOriginProbe) -> bool {
    matches!(origin, BranchOriginProbe::Absent)
}

pub(crate) enum Phase1PrResolve {
    /// An open PR was found (directly or recovered via the spec-id search).
    Found(OpenPrInfo),
    /// BUG-709: no OPEN PR, but the branch's PR is already MERGED — the
    /// implementer ran the full ship itself. The work landed; the drive
    /// completes cleanly rather than retrying the open-PR verify.
    // trace:BUG-709 | ai:claude
    AlreadyMerged(OpenPrInfo),
    /// Definitively no open PR — falls through to the punt / NoPr path.
    NoPr,
    /// A definitive hard failure (gh missing / gh errored / branch not on
    /// origin) — return it as the phase failure, no retry.
    Fail(auto_complete::PhaseFailure),
    /// Transient: GH was unreachable but the branch may carry a PR — retry,
    /// then declare Inconclusive with this reason if the budget is exhausted.
    Retry(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentGateOnFail {
    Shelve,
    Warn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentGateConfig {
    pub(crate) name: String,
    pub(crate) role: String,
    pub(crate) applies_to: String,
    pub(crate) on_fail: AgentGateOnFail,
}

pub(crate) fn normalize_gate_token(s: &str) -> String {
    s.trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

pub(crate) fn sanitize_gate_file_token(s: &str) -> String {
    let token: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let token = token.trim_matches('-');
    if token.is_empty() {
        "agent-gate".to_string()
    } else {
        token.to_string()
    }
}

pub(crate) fn parse_agent_gates_from_config(value: Option<&toml::Value>) -> Vec<AgentGateConfig> {
    let Some(gates) = value
        .and_then(|v| v.get("pipeline"))
        .and_then(|v| v.get("gate"))
        .and_then(|v| v.as_table())
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, gate) in gates {
        let kind = gate
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if !kind.eq_ignore_ascii_case("agent") {
            continue;
        }
        let Some(role) = gate.get("role").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(applies_to) = gate.get("applies_to").and_then(|v| v.as_str()) else {
            continue;
        };
        let on_fail = match gate
            .get("on_fail")
            .and_then(|v| v.as_str())
            .unwrap_or("shelve")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "warn" => AgentGateOnFail::Warn,
            _ => AgentGateOnFail::Shelve,
        };
        out.push(AgentGateConfig {
            name: name.to_string(),
            role: role.trim().to_string(),
            applies_to: applies_to.trim().to_string(),
            on_fail,
        });
    }
    out
}

pub(crate) fn selector_tokens(selector: &str) -> impl Iterator<Item = &str> {
    selector
        .split(|c: char| c == ',' || c == '|' || c == '/' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

pub(crate) fn agent_gate_matches_req(gate: &AgentGateConfig, req: &aida_core::Requirement) -> bool {
    selector_tokens(&gate.applies_to).any(|token| {
        if let Some(want) = token.strip_prefix("type:") {
            normalize_gate_token(want) == normalize_gate_token(&req.req_type.to_string())
                || normalize_gate_token(want) == normalize_gate_token(req.req_type.default_prefix())
        } else if let Some(want) = token.strip_prefix("tag:") {
            let want = want.trim();
            req.tags.iter().any(|tag| tag.eq_ignore_ascii_case(want))
        } else {
            false
        }
    })
}

pub(crate) fn matching_agent_gates_for_spec(
    project_root: &std::path::Path,
    spec: &str,
) -> Vec<AgentGateConfig> {
    let gates = parse_agent_gates_from_config(read_project_config_value(project_root).as_ref());
    if gates.is_empty() {
        return Vec::new();
    }
    let Some(store) = load_store_for_lookup(project_root) else {
        return Vec::new();
    };
    let Some(req) = store
        .requirements
        .iter()
        .find(|r| r.spec_id.as_deref() == Some(spec))
    else {
        return Vec::new();
    };
    gates
        .into_iter()
        .filter(|gate| agent_gate_matches_req(gate, req))
        .collect()
}

/// STORY-492: a `--resume-drain` re-entry context for [`run_auto_complete`] —
/// the reconciled phase to re-enter at plus the branch + PR to seed into the
/// driver (the skipped earlier phases would have discovered these). `None`
/// passed to `run_auto_complete` means a normal phase-1 drain.
/// trace:STORY-492 | ai:claude
pub(crate) struct ResumeEntry {
    pub(crate) start_phase: auto_complete::Phase,
    pub(crate) branch: Option<String>,
    pub(crate) pr: Option<u32>,
    /// Phase-1 head persisted in drain state. A resumed phase 2 compares the
    /// forge's current head to this value instead of blessing a moved change.
    // trace:BUG-1818 | ai:codex
    pub(crate) head_sha: Option<String>,
    pub(crate) from_pr: bool,
    /// BUG-1819: terminal CI evidence verified during `--from-pr`
    /// reconciliation. A direct reviewer re-entry must inherit this rather
    /// than pretending only an in-process phase 2 can establish it.
    pub(crate) ci_terminal_sha: Option<String>,
    pub(crate) ci_terminal_green: Option<bool>,
}

/// What the ownership probe concluded about a same-branch open change before
/// TASK-1529 retracts it. `Foreign` and `Unverified` both refuse the close, but
/// they are different operator situations: a change we could not identify may
/// still be ours and still carries failing guards, so it is reported as loudly
/// as a failed close, while somebody else's change is left alone quietly.
// trace:TASK-1529 | ai:claude
pub(crate) enum RetractionOwnership {
    /// Open, and its author is this forge identity.
    Ours,
    /// Open, and confirmed somebody else's.
    Foreign,
    /// The probe itself failed, or the forge withheld the author — the
    /// change may still be ours and may still be open.
    Unverified,
    /// Resolved as closed or merged: nothing to retract, nothing to hold.
    /// The AlreadyMerged path owns the merged case.
    // trace:TASK-1552 | ai:claude
    NoLongerOpen,
}

/// What `retract_publication_with_notice` left behind on the forge.
///
/// TASK-1552 finding 1: the refused-preflight caller must know not merely
/// whether the close happened, but whether a guards-failing change is STILL
/// OPEN — that is the case that needs a merge-hold, or the refusal is
/// enforced by stderr prose only.
// trace:TASK-1552 | ai:claude
pub(crate) enum RetractionOutcome {
    /// The open change was closed; nothing is published.
    Closed,
    /// A guards-failing change remains open (or could not be confirmed
    /// closed): foreign or unverified ownership, or the close call failed.
    LeftOpen(crate::forge::ChangeRef),
    /// Nothing to act on: no change found, or it is no longer open.
    Nothing,
}

/// Validate the phase-1 change binding before phase 2 uses its source branch.
/// Kept pure so exact-head and attribution failures stay regression-testable.
// trace:BUG-1818 | ai:codex
pub(crate) fn verified_phase2_branch(
    pr: u32,
    expected_head: &str,
    metadata: &crate::forge::ChangeMetadata,
    credit: PrCreditMatch,
    spec: &str,
) -> Result<String, auto_complete::PhaseFailure> {
    if metadata.state != crate::forge::ChangeState::Open {
        return Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::CiUnavailable,
            format!("phase-1 PR/MR {pr} is {:?}, not open", metadata.state),
        ));
    }
    if !expected_head.eq_ignore_ascii_case(metadata.head_sha.trim()) {
        return Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::CiUnavailable,
            format!(
                "phase-1 PR/MR {pr} head mismatch: produced `{expected_head}`, forge reports `{}`",
                metadata.head_sha
            ),
        ));
    }
    match credit {
        PrCreditMatch::Dispatched => {}
        PrCreditMatch::Other(other) => {
            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::ShippedMismatch,
                format!(
                    "phase-1 PR/MR {pr} at `{}` credits {other}, not {spec}",
                    metadata.head_sha
                ),
            ));
        }
        PrCreditMatch::Unknown => {
            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::ShippedMismatch,
                format!(
                    "could not verify that phase-1 PR/MR {pr} at `{}` covers {spec}; refusing phase 2",
                    metadata.head_sha
                ),
            ));
        }
    }
    if metadata.head_ref.trim().is_empty() {
        return Err(auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::CiUnavailable,
            format!("phase-1 PR/MR {pr} did not report a source branch"),
        ));
    }
    Ok(metadata.head_ref.clone())
}

impl RealPhaseDriver {
    /// Set the PR number AND, if the spec is supervised, stamp the merge-hold
    /// marker the MOMENT the PR becomes known (fresh creation or recovery/detect)
    /// — not at the later hold-decision. Closes the BUG-1167 residual race where a
    /// concurrent/stale merger merged in the window between "PR goes green" and
    /// the drain's hold firing (STORY-1163 #1861 merged ~49s before its hold).
    /// record_merge_supervision_hold stays as the merge-time backstop; this is the
    /// early stamp every pr_number-set path funnels through.
    // trace:BUG-1173 | ai:claude
    pub(crate) fn set_pr_number(&mut self, pr: u32) {
        self.pr_number = Some(pr);
        if let Some(reason) = auto_complete::PhaseDriver::merge_supervision_hold(self) {
            let pr = u64::from(pr);
            let _ = crate::merge_hold::write_typed_hold(
                &self.project_root,
                &crate::merge_hold::typed_hold(
                    pr,
                    crate::merge_hold::HoldReasonKind::Supervision,
                    &reason,
                    None,
                )
                .with_spec(&self.spec), // trace:BUG-1562 | ai:claude
            );
            // BUG-1173 deferred the LABEL to merge time because an early red
            // merge-hold-gate read as a CI failure. Since BUG-1180 / ADR-39 the
            // drain's CI watch and `aida pr ship` classify that red as THE HOLD
            // when the marker exists, so the label is safe at PR-detection —
            // and BUG-1236 showed the merge-time sync silently never landed on
            // three supervised PRs, leaving Layer 2 off. Sync here, surface
            // failure, and let `aida merge-hold list --fix` repair it.
            // trace:BUG-1173 trace:BUG-1236 | ai:claude
            if let Err(err) = crate::merge_hold::sync_label(&self.project_root, pr, true) {
                eprintln!(
                    "  {} merge-hold label not applied on PR-{pr}: {err} — Layer 2 is off for this PR until `aida merge-hold list --fix` re-syncs it",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
        }
    }

    // why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        project_root: std::path::PathBuf,
        spec: String,
        queue_user_id: String,
        permission_mode: Option<String>,
        json: bool,
        no_human: Option<auto_complete::NoHumanMode>,
        // ADR-10: the run's typed autonomy mode, resolved ONCE at dispatch and
        // carried alongside `no_human`. trace:ADR-10 | ai:claude
        autonomy_mode: AutonomyMode,
        run_token: String,
        steal: bool,
        force_claim: bool,
        allow_stale_base: bool,
        no_auto_rebase: bool,
        lifecycle_skip: auto_complete::LifecycleSkip,
        variant: auto_complete::AutoCompleteVariant,
    ) -> Self {
        let drain_tuning = DrainTuning::resolve(&project_root);
        let lifecycle_forge = crate::forge::resolve_open_change_forge_kind(&project_root);
        Self {
            project_root,
            lifecycle_forge,
            forge_factory: None,
            spec,
            queue_user_id,
            permission_mode,
            json,
            branch: None,
            implementer_lease: None,
            pr_number: None,
            phase_done_pr: None,
            phase_done_head: None,
            ci_run_id: None,
            ci_terminal_sha: None,
            ci_terminal_green: None,
            aida_exe: resolve_aida_exe(),
            no_human,
            autonomy_mode,
            exit_cfg: exit_signal::ExitSignalConfig::from_env(),
            run_token,
            implementer_session: None,
            implementer_worktree: None,
            steal,
            force_claim,
            allow_stale_base,
            no_auto_rebase,
            auto_rebase_events: Vec::new(),
            lifecycle_skip,
            variant,
            from_pr: false,
            drain_tuning,
            empty_launch_retries_used: 0,
            retry_implementer_worktree: None,
            retry_implementer_branch: None,
            phase1_workspace: None,
            lost_child_state_retries_used: 0,
            phase1_entry_spec_status: None,
            rework_guard: None,
        }
    }

    pub(crate) fn sessions_dir(&self) -> std::path::PathBuf {
        self.project_root.join(".aida").join("sessions")
    }

    /// ADR-10: whether this run is a supervised `--zen` drive, read from the
    /// carried typed `autonomy_mode` field — the in-process source of truth —
    /// NOT from a bare `AIDA_ZEN` env re-read.
    // trace:ADR-10 | ai:claude
    pub(crate) fn is_zen_run(&self) -> bool {
        self.autonomy_mode.is_zen()
    }

    pub(crate) fn aida_exe(&self) -> std::path::PathBuf {
        self.aida_exe.clone()
    }

    /// TASK-1421: the injection seam. An injected
    /// [`crate::forge::ForgeFactory`] reaches the forges built through
    /// `lifecycle_forge()` and `project_forge()`: merge, the phase-1 PR
    /// lookups in `detect_phase1_pr`, and the refused-preflight retraction.
    /// Paths that still call `forge_for_kind` directly (`detect_merged_pr`,
    /// `ci_probe_with_forge`, the PR-opening free functions) bypass it and
    /// are out of scope for TASK-1421. With no factory this is exactly
    /// `forge_for_kind`.
    // trace:TASK-1421 | ai:claude
    pub(crate) fn forge_of_kind(
        &self,
        kind: crate::forge::ForgeKind,
    ) -> Box<dyn crate::forge::Forge> {
        match &self.forge_factory {
            Some(factory) => factory(&self.project_root, kind),
            None => crate::forge::forge_for_kind(&self.project_root, kind),
        }
    }

    /// TASK-1529: the publication guards refused, but the implementer may
    /// already have opened the PR. Retract an OPEN change through the forge so
    /// "the guards refused" and "nothing is published" are the same state. An
    /// already-merged change is left alone: closing is meaningless there and
    /// the drive's AlreadyMerged handling owns it. Best-effort: a failed
    /// close is reported and the phase still fails. Split out of
    /// `run_implementer` so a test can drive it with an injected forge.
    // trace:TASK-1529 trace:TASK-1421 | ai:codex
    pub(crate) fn retract_refused_publication(&mut self, branch: &str, detail: &str) {
        let note = implementer_preflight::retraction_notice(detail);
        // TASK-1552 finding 1: a refused change this identity could not
        // close stays OPEN on the forge — under the branch-keyed ownership
        // rule that is the COMMON rework-round case, not an edge case. Hold
        // it unmergeable instead of trusting stderr prose.
        // trace:TASK-1552 | ai:claude
        if let RetractionOutcome::LeftOpen(change) =
            self.retract_publication_with_notice(branch, &note)
        {
            self.hold_refused_publication(&change, detail);
        }
    }

    /// Says what the retraction left behind; every arm that deliberately
    /// leaves an open change on the forge (foreign or unverified ownership,
    /// or a failed close call) reports it as [`RetractionOutcome::LeftOpen`].
    pub(crate) fn retract_publication_with_notice(
        &mut self,
        branch: &str,
        note: &str,
    ) -> RetractionOutcome {
        if let Phase1PrResolve::Found(pr) = self.detect_phase1_pr(branch) {
            // A same-branch PR may have been opened by an operator while the
            // implementer was running. Branch presence alone does not prove
            // the agent published it. Require both forge identities to resolve
            // and match before applying this destructive lifecycle action.
            let forge = self.project_forge();
            let mut sink = crate::network_retry::NoopSink;
            let metadata = forge.change_metadata(pr.number, &mut sink);
            let owner = forge.authenticated_user_login();
            let ownership = match (metadata, owner) {
                (Ok(meta), Ok(owner)) if meta.state == crate::forge::ChangeState::Open => {
                    match meta.author.as_deref() {
                        Some(author) if author.eq_ignore_ascii_case(&owner) => {
                            RetractionOwnership::Ours
                        }
                        Some(_) => RetractionOwnership::Foreign,
                        None => RetractionOwnership::Unverified,
                    }
                }
                // A change that is no longer open needs no retraction, and the
                // AlreadyMerged path owns the merged case.
                (Ok(_), Ok(_)) => RetractionOwnership::NoLongerOpen,
                _ => RetractionOwnership::Unverified,
            };
            let change = crate::forge::ChangeRef {
                id: pr.number,
                url: pr.url.clone(),
                branch: pr.head_branch.clone().unwrap_or_else(|| branch.to_string()),
                base: String::new(),
                title: Some(pr.title.clone()),
            };
            match ownership {
                RetractionOwnership::Ours => {}
                RetractionOwnership::Foreign => {
                    if !self.json {
                        eprintln!(
                            "  {} left PR-{} open — this forge identity did not open it",
                            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                            pr.number,
                        );
                    }
                    return RetractionOutcome::LeftOpen(change);
                }
                RetractionOwnership::Unverified => {
                    if !self.json {
                        eprintln!(
                            "  {} PR-{} is OPEN and its publication guards FAILED, but who \
                             opened it could not be confirmed — check it by hand",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                            pr.number,
                        );
                    }
                    return RetractionOutcome::LeftOpen(change);
                }
                // trace:TASK-1552 | ai:claude
                RetractionOwnership::NoLongerOpen => {
                    return RetractionOutcome::Nothing;
                }
            }
            match forge.close_change(&change, note) {
                Ok(()) => {
                    if !self.json {
                        eprintln!(
                            "  {} closed PR-{} — it was opened before the publication \
                             guards ran, and they refused it",
                            crate::glyph(crate::glyphs::Glyph::Check).green(),
                            pr.number,
                        );
                    }
                    RetractionOutcome::Closed
                }
                Err(e) => {
                    if !self.json {
                        eprintln!(
                            "  {} PR-{} is OPEN and its publication guards FAILED — \
                             close it by hand: {e}",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                            pr.number,
                        );
                    }
                    RetractionOutcome::LeftOpen(change)
                }
            }
        } else {
            RetractionOutcome::Nothing
        }
    }

    /// TASK-1552 finding 1: a guards-REFUSED change that could not be closed
    /// (foreign or unverified ownership, or the close call itself failed)
    /// must not stay quietly mergeable. Stamp the typed merge-hold and its
    /// label — no ownership check, deliberately: a hold only ever TIGHTENS
    /// (the BUG-1714 doctrine) — and explain on the PR why it is held. It is
    /// released through the ordinary `aida merge-hold clear` path.
    /// Best-effort like the retraction: every miss is reported and the
    /// phase still fails.
    // trace:TASK-1552 | ai:claude
    pub(crate) fn hold_refused_publication(
        &mut self,
        change: &crate::forge::ChangeRef,
        detail: &str,
    ) {
        let number = change.id;
        let hold = crate::merge_hold::typed_hold(
            number,
            crate::merge_hold::HoldReasonKind::Supervision,
            format!(
                "publication guards refused this change and it could not be closed; \
                 it must not merge unreviewed:\n{detail}"
            ),
            None,
        )
        .with_spec(&self.spec);
        match crate::merge_hold::write_typed_hold(&self.project_root, &hold) {
            Ok(()) => {
                if !self.json {
                    eprintln!(
                        "  {} PR-{number} held unmergeable — its publication guards refused \
                         it and it could not be closed",
                        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    );
                }
            }
            Err(e) => {
                if !self.json {
                    eprintln!(
                        "  {} PR-{number} is OPEN, its publication guards REFUSED it, and \
                         the merge-hold marker could not be written — hold it by hand \
                         (`aida merge-hold add {number}`): {e}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    );
                }
            }
        }
        if let Err(err) = crate::merge_hold::sync_label(&self.project_root, number, true) {
            if !self.json {
                eprintln!(
                    "  {} merge-hold label not applied on PR-{number}: {err} — Layer 2 is off \
                     for this PR until `aida merge-hold list --fix` re-syncs it",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
        }
        let note = implementer_preflight::refused_hold_notice(detail);
        if let Err(e) = self.project_forge().comment(change, &note) {
            if !self.json {
                eprintln!(
                    "  {} the hold could not be explained on PR-{number}: {e}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                );
            }
        }
    }

    /// BUG-1690: phase 1 adopts an already-open PR as the publication to
    /// shepherd, but nothing ever un-drafted one — so a PR sitting in draft
    /// state (converted by the STORY-529 ship gate, by an operator, or opened
    /// as a draft) passed CI and review and then stalled at the phase-4 merge,
    /// which every forge refuses for a draft. Un-draft the reused PR here, at
    /// the adoption point, so a retry reaches a mergeable state. Chosen over
    /// opening a fresh PR because draft-first reuse was an operator-approved
    /// decision (see the BUG-1690 spec trail).
    ///
    /// A spec tagged `review:draft-only` is the deliberate exception: its
    /// draft IS the STORY-529 human-review hold, and the drain's merge phase
    /// never reads that tag — the draft state is what keeps the PR unmerged.
    /// Leave it, and say so.
    ///
    /// Best-effort like the retraction: a failed un-draft is reported loudly
    /// and the drive proceeds — the merge then fails with the forge's own
    /// draft refusal instead of silently stalling.
    // trace:BUG-1690 | ai:claude
    pub(crate) fn undraft_reused_publication(&mut self, pr: &OpenPrInfo) {
        let forge = self.project_forge();
        let mut sink = crate::network_retry::NoopSink;
        let Ok(meta) = forge.change_metadata(pr.number, &mut sink) else {
            // Nothing claims the PR is a draft; a wrong guess here would
            // un-draft someone else's deliberate draft. The merge's own
            // refusal remains the backstop.
            return;
        };
        if meta.state != crate::forge::ChangeState::Open || !meta.is_draft {
            return;
        }
        if self.spec_is_draft_only_tagged() {
            if !self.json {
                eprintln!(
                    "  {} PR-{} stays a draft — {} is tagged `{}`, so the draft is the \
                     human-review hold, not a stall",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    pr.number,
                    self.spec,
                    pr_ship::DRAFT_ONLY_TAG,
                );
            }
            return;
        }
        let change = crate::forge::ChangeRef {
            id: pr.number,
            url: pr.url.clone(),
            branch: pr.head_branch.clone().unwrap_or_default(),
            base: String::new(),
            title: Some(pr.title.clone()),
        };
        match forge.mark_change_ready(&change) {
            Ok(()) => {
                if !self.json {
                    eprintln!(
                        "  {} PR-{} was a draft — marked it ready so this attempt can merge it",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        pr.number,
                    );
                }
            }
            Err(e) => {
                if !self.json {
                    // The repair hint speaks the resolved forge's own syntax.
                    let repair = match forge.kind() {
                        crate::forge::ForgeKind::GitHub => format!("gh pr ready {}", pr.number),
                        crate::forge::ForgeKind::GitLab => {
                            format!("glab mr update {} --ready", pr.number)
                        }
                        crate::forge::ForgeKind::None => {
                            format!("mark change {} ready on the forge", pr.number)
                        }
                    };
                    eprintln!(
                        "  {} PR-{} is a DRAFT and could not be marked ready — the forge \
                         will refuse the merge until `{repair}` un-drafts it: {e}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        pr.number,
                    );
                }
            }
        }
    }

    /// Whether this drive's spec carries the STORY-529 `review:draft-only`
    /// tag — the one case a reused draft PR must stay a draft.
    // trace:BUG-1690 | ai:claude
    pub(crate) fn spec_is_draft_only_tagged(&self) -> bool {
        let Some(store) = load_store_for_lookup(&self.project_root) else {
            return false;
        };
        let want = self.spec.to_ascii_uppercase();
        store.requirements.iter().any(|r| {
            let matches_spec = r
                .spec_id
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case(&want))
                .unwrap_or(false)
                || r.agreed_id
                    .as_deref()
                    .map(|s| s.eq_ignore_ascii_case(&want))
                    .unwrap_or(false);
            matches_spec && {
                // The ship gate's own predicate, so the two surfaces cannot
                // drift on what counts as draft-only-tagged.
                let tags: Vec<String> = r.tags.iter().cloned().collect();
                pr_ship::is_draft_only_tagged(&tags)
            }
        })
    }

    /// BUG-1714: the publication guards came back inconclusive-only — nothing
    /// objected, nothing verified. If the implementer already opened a PR,
    /// leave it OPEN but unmergeable: stamp a typed merge-hold (plus the
    /// label) and say on the PR why it is held and that the guards will be
    /// retried. No ownership check, deliberately: a hold only ever TIGHTENS —
    /// whoever opened the PR, the work on this branch is unverified and must
    /// not merge unreviewed — and it is released through the ordinary
    /// `aida merge-hold clear` path. Best-effort like the retraction: every
    /// miss is reported and the phase still fails.
    // trace:BUG-1714 | ai:claude
    pub(crate) fn hold_inconclusive_publication(&mut self, branch: &str, detail: &str) {
        let Phase1PrResolve::Found(pr) = self.detect_phase1_pr(branch) else {
            return;
        };
        let number = pr.number;
        let hold = crate::merge_hold::typed_hold(
            number,
            crate::merge_hold::HoldReasonKind::Supervision,
            format!(
                "publication guards could not complete (inconclusive); the change is \
                 unverified and must not merge unreviewed:\n{detail}"
            ),
            None,
        )
        .with_spec(&self.spec);
        let marker_result = crate::merge_hold::write_typed_hold(&self.project_root, &hold);
        let label_result = crate::merge_hold::sync_label(&self.project_root, number, true);
        match &marker_result {
            Ok(()) => {
                if !self.json {
                    eprintln!(
                        "  {} PR-{number} left open under a merge-hold — the publication \
                         guards could not complete and will be retried",
                        crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    );
                }
            }
            Err(e) => {
                if !self.json {
                    eprintln!(
                        "  {} PR-{number} is OPEN and UNVERIFIED but the merge-hold marker \
                         could not be written — hold it by hand (`aida merge-hold add \
                         {number}`): {e}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    );
                }
            }
        }
        if let Err(err) = &label_result {
            if !self.json {
                eprintln!(
                    "  {} merge-hold label not applied on PR-{number}: {err} — Layer 2 is off \
                     for this PR until `aida merge-hold list --fix` re-syncs it",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
        }
        let change = crate::forge::ChangeRef {
            id: number,
            url: pr.url.clone(),
            branch: pr.head_branch.clone().unwrap_or_else(|| branch.to_string()),
            base: String::new(),
            title: Some(pr.title.clone()),
        };
        if let (Err(marker_error), Err(label_error)) = (&marker_result, &label_result) {
            let notice = format!(
                "Publication guards could not complete, and the merge-hold could not be applied.\n\n\
                 Merge-hold marker error: {marker_error}\n\
                 Merge-hold label error: {label_error}\n\n\
                 This PR was closed only because it could not be made unmergeable. The branch is \
                 preserved, and the guards will be retried on the next publication attempt.\n\n{detail}"
            );
            if !self.json {
                eprintln!(
                    "  {} both merge-hold layers failed for PR-{number}; closing it because it could not be made unmergeable",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                );
            }
            if !matches!(
                self.retract_publication_with_notice(branch, &notice),
                RetractionOutcome::Closed
            ) {
                // The PR could be neither held nor closed. The last honest
                // lever is a comment: put the warning ON the PR so whoever
                // can merge it sees that the work is unverified.
                let warning = format!(
                    "Publication guards could not complete, and the merge-hold could not \
                     be applied.\n\n\
                     Merge-hold marker error: {marker_error}\n\
                     Merge-hold label error: {label_error}\n\n\
                     This PR could be neither held nor closed: it is OPEN and UNVERIFIED \
                     and must not merge unreviewed. The guards will be retried on the \
                     next publication attempt.\n\n{detail}"
                );
                if let Err(e) = self.project_forge().comment(&change, &warning) {
                    if !self.json {
                        eprintln!(
                            "  {} PR-{number} is OPEN, UNVERIFIED, and carries no hold \
                             layer — could not even warn on it: {e}",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                        );
                    }
                }
            }
            return;
        }
        let note = implementer_preflight::inconclusive_hold_notice(detail);
        if let Err(e) = self.project_forge().comment(&change, &note) {
            if !self.json {
                eprintln!(
                    "  {} could not explain the hold on PR-{number}: {e}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                );
            }
        }
    }

    pub(crate) fn lifecycle_forge(&self) -> Box<dyn crate::forge::Forge> {
        self.forge_of_kind(self.lifecycle_forge)
    }

    /// The project's configured forge — the driver-side equivalent of
    /// `forge_for(&self.project_root)`, routed through the injection seam.
    // trace:TASK-1421 | ai:claude
    pub(crate) fn project_forge(&self) -> Box<dyn crate::forge::Forge> {
        self.forge_of_kind(crate::forge::resolve_forge_kind(&self.project_root))
    }

    pub(crate) fn record_auto_rebase(&mut self, pr_number: u64, outcome: impl Into<String>) {
        self.auto_rebase_events
            .push(auto_complete_telemetry::AutoRebaseEvent {
                phase: auto_complete::Phase::Reviewer.index() as u8,
                pr_number,
                outcome: outcome.into(),
            });
    }

    pub(crate) fn should_auto_rebase_stale_base(
        &self,
        attempted: bool,
    ) -> Result<(), &'static str> {
        if self.allow_stale_base {
            return Err("allow-stale-base");
        }
        if self.no_auto_rebase {
            return Err("disabled");
        }
        // BUG-1268: reviewer-only headless runs have nobody available to
        // perform the mechanical recovery either. The safety boundary is a
        // headless reviewer, not whether phase 1 was also headless.
        // trace:BUG-1268 | ai:codex
        if self.no_human.is_none() {
            return Err("not-headless");
        }
        if attempted {
            return Err("retry-limit");
        }
        Ok(())
    }

    pub(crate) fn attempt_phase3_auto_rebase(
        &mut self,
        pr_number: u64,
    ) -> Result<(), auto_complete::PhaseFailure> {
        // BUG-1574: this is the concrete force-push path an unattended drain
        // takes on the spec's OWN branch (`aida pr rebase` → BUG-640's
        // anchored-lease push). Refuse BEFORE spawning it — never crash —
        // when the spec is keyboard-only or outside the currently active
        // batch's declared member set. Fail-closed: a declared-but-broken
        // store or malformed drain-state.json also refuses. Typed
        // `StaleBaseRefused` (not a generic `Failed`) because this refusal
        // has the exact same shape and handling contract as the OTHER
        // force-push refusal below (BUG-1218: a non-transient, operator-
        // reconciliation shelve — never worth a retry, which would
        // deterministically hit the same guard again); the caller's
        // shelve-and-continue path (resilient-drain / STORY-281) reads that
        // typed kind, not this prose. trace:BUG-1574 | ai:claude
        if let Some(reason) = unattended_git_mutation_refusal(
            &self.project_root,
            &self.spec,
            "rebase/force-push",
            self.branch.as_deref(),
        ) {
            if !self.json {
                eprintln!(
                    "  {} auto-rebase of PR-{pr_number} refused: {reason}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold()
                );
            }
            self.record_auto_rebase(pr_number, "refused-scope-guard");
            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::StaleBaseRefused,
                reason,
            ));
        }
        // (classification helper `classify_rebase_subprocess_exit` lives at
        // crate scope — see below — so it's unit-testable without spawning
        // a subprocess. trace:BUG-1295 | ai:claude)
        if !self.json {
            eprintln!(
                "  {} stale-base + overlap detected on PR-{pr_number}; attempting one clean auto-rebase…",
                "↻".cyan()
            );
        }
        let status = std::process::Command::new(self.aida_exe())
            .current_dir(&self.project_root)
            .args(build_phase3_auto_rebase_args(pr_number))
            .output_retrying_etxtbsy();
        match status {
            Ok(output) if output.status.success() => {
                if !output.stdout.is_empty() {
                    eprint!("{}", String::from_utf8_lossy(&output.stdout));
                }
                if !output.stderr.is_empty() {
                    eprint!("{}", String::from_utf8_lossy(&output.stderr));
                }
                self.record_auto_rebase(pr_number, "clean");
                if !self.json {
                    eprintln!(
                        "  {} PR-{pr_number} auto-rebased cleanly; proceeding with phase 3",
                        crate::glyph(crate::glyphs::Glyph::Check).green().bold()
                    );
                }
                Ok(())
            }
            Ok(output) => {
                let detail = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                if !detail.is_empty() {
                    eprint!("{detail}");
                }
                // BUG-1295: classify on the subprocess's EXIT CODE via the
                // pure `classify_rebase_subprocess_exit` helper, not by
                // substring-matching its stdout/stderr prose. The six bail
                // sites this used to match (pr_cmd.rs conflict/force-push
                // bails, rebase_cmd.rs, pr_rebase.rs) could be reworded
                // independently and silently fall through to the untyped
                // arm below — a conflict losing `StaleBaseConflict` (and the
                // `--interactive` recovery recipe attached to it) or a
                // safety refusal being reclassified as a generic failure,
                // with nothing to catch the drift. `pr_cmd.rs`'s conflict
                // and force-push-refused bails now return
                // `pr_rebase::rebase_conflict_error` / `rebase_refused_error`,
                // which `main_entry` downcasts to pick
                // `pr_rebase::REBASE_EXIT_CODE_CONFLICT` /
                // `REBASE_EXIT_CODE_REFUSED` as this process's exit code —
                // the one signal both ends read from the same shared
                // constant. See
                // `tests/bug_1295_rebase_exit_code_tests.rs` for the drift
                // guard. trace:BUG-1295 | ai:claude
                match classify_rebase_subprocess_exit(output.status.code()) {
                    Some(auto_complete::FailureKind::StaleBaseRefused) => {
                        self.record_auto_rebase(pr_number, "stale-base-refused");
                        return Err(auto_complete::PhaseFailure::of(
                            auto_complete::FailureKind::StaleBaseRefused,
                            detail.trim().to_string(),
                        ));
                    }
                    Some(auto_complete::FailureKind::StaleBaseConflict) => {
                        self.record_auto_rebase(pr_number, "conflict");
                        let recipe = format!(
                            "{}\n\nManual recovery: `aida pr rebase {pr_number} --interactive`",
                            detail.trim()
                        );
                        return Err(auto_complete::PhaseFailure::of(
                            auto_complete::FailureKind::StaleBaseConflict,
                            recipe,
                        ));
                    }
                    _ => {}
                }
                self.record_auto_rebase(pr_number, "failed");
                Err(auto_complete::PhaseFailure::new(detail.trim().to_string()))
            }
            Err(e) => {
                self.record_auto_rebase(pr_number, format!("failed:{e}"));
                Err(auto_complete::PhaseFailure::new(format!(
                    "auto-rebase failed to start: {e}"
                )))
            }
        }
    }

    pub(crate) fn resolve_phase3_stale_overlap(
        &mut self,
        pr_number: u32,
        auto_rebase_attempted: &mut bool,
        msg: String,
    ) -> Phase3StaleOverlapAction {
        match self.should_auto_rebase_stale_base(*auto_rebase_attempted) {
            Ok(()) => {
                *auto_rebase_attempted = true;
                match self.attempt_phase3_auto_rebase(pr_number as u64) {
                    Ok(()) => return Phase3StaleOverlapAction::Proceed,
                    Err(failure)
                        if matches!(
                            failure.kind,
                            auto_complete::FailureKind::StaleBaseRefused
                                | auto_complete::FailureKind::StaleBaseConflict
                        ) =>
                    {
                        return Phase3StaleOverlapAction::Refuse(failure);
                    }
                    Err(_) => {}
                }
                Phase3StaleOverlapAction::Refuse(auto_complete::PhaseFailure::new(msg))
            }
            Err("allow-stale-base") => {
                self.record_auto_rebase(pr_number as u64, "skipped:allow-stale-base");
                eprintln!(
                    "  {} stale-base + overlap detected; `--allow-stale-base` is set, \
                     proceeding with reviewer.\n{}",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                    msg.yellow()
                );
                Phase3StaleOverlapAction::Proceed
            }
            Err(reason) => {
                self.record_auto_rebase(pr_number as u64, format!("skipped:{reason}"));
                Phase3StaleOverlapAction::Refuse(auto_complete::PhaseFailure::new(msg))
            }
        }
    }

    pub(crate) fn auto_punt_text_question(
        &self,
        worktree_path: &std::path::Path,
        session_uuid: &str,
    ) -> Option<String> {
        if !self
            .no_human
            .map(|m| m.wants_headless_implementer())
            .unwrap_or(false)
        {
            return None;
        }
        let content = read_headless_log_for_session(&self.project_root, session_uuid)?;
        let punt = pending_text_question_from_headless_log(&content)?;
        let status = std::process::Command::new(self.aida_exe())
            .current_dir(worktree_path)
            .args(build_auto_punt_args(&self.spec, &punt.detail, &punt.lean))
            .env("AIDA_SESSION_ROLE", "implementer")
            .status_retrying_etxtbsy()
            .ok()?;
        if !status.success() {
            return None;
        }
        Some(format!(
            "[design-fork] {} (lean: {})",
            punt.detail, punt.lean
        ))
    }

    /// Locate the session lease `aida queue work --session-id <uuid>` created
    /// for this orchestrator phase, pinned by the `claude_session_id` the
    /// orchestrator minted. Deterministic — unaffected by concurrent leases
    /// from parallel user sessions or nested `/aida-pickup`, and immune to
    /// the `.manifest.toml` companion file that the old lease-set diff
    /// miscounted as a second lease (the BUG-114 phase-1 failure). On a miss,
    /// the error suggests bare `--resume` (continues the most recent recorded
    /// claude session) and lists the candidate lease ids as diagnostic-only —
    /// they are lease ids, not `--resume` arguments (TASK-271).
    /// trace:BUG-114 trace:TASK-271 | ai:claude
    pub(crate) fn discover_orchestrated_lease(
        &self,
        claude_session_id: &str,
    ) -> Result<(String, String, std::path::PathBuf), auto_complete::PhaseFailure> {
        find_orchestrated_lease(&self.project_root, claude_session_id)
            .or_else(|| {
                read_orchestrated_lease_receipt(
                    &orchestrated_lease_receipt_path(&self.project_root, claude_session_id),
                    claude_session_id,
                )
            })
            .map(|(id, branch, worktree, _)| (id, branch, worktree))
            .ok_or_else(|| {
                let candidates = lease_ids_in(&self.sessions_dir());
                if candidates.is_empty() {
                    // BUG-1524: no lease, no receipt, no other candidate in
                    // flight — the child never got far enough to mint
                    // anything, i.e. it never launched an implementer
                    // session at all (a preflight/lease refusal or an
                    // agent-binary launch failure). Typed as
                    // `LaunchRefused` so the orchestrator never treats an
                    // already-open PR from a previous round as evidence
                    // this round did anything. trace:BUG-1524 | ai:claude
                    auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::LaunchRefused,
                        "no session lease appeared — `aida queue work` did not start a session",
                    )
                } else if let Some(conflict_failure) =
                    scope_conflict_lease_failure(&self.project_root, &self.spec)
                {
                    // BUG-1285: the dominant real-world cause of "no lease
                    // minted under this run's session id" is a same-scope
                    // lease conflict — the child `aida queue work` claim gate
                    // refused before it ever got to mint one. Name the
                    // holding lease + its liveness, not a bare id list the
                    // operator has to cross-reference by hand — and route it
                    // through `FailureKind::LeaseConflict`, which is NOT a
                    // transient-retry cause, so a refusal that IS correct
                    // (the holder is still live) does not burn the drain's
                    // retry budget rediscovering the same conflict. This is
                    // also a launch refusal (the claim gate blocked the
                    // child before it started). It keeps its own kind; the
                    // orchestrator skips open-PR recovery for it exactly as
                    // for `LaunchRefused` (BUG-1524).
                    // trace:BUG-1285 | ai:claude
                    conflict_failure
                } else {
                    // BUG-1524: candidates exist (other leases are live),
                    // but none of them is THIS session's — and no BUG-1485
                    // handoff receipt exists either, so this session did
                    // not even reach the point of completing genuine work
                    // and releasing its lease cleanly. Typed the same way
                    // as the empty-candidates case above.
                    // trace:BUG-1485 | ai:codex trace:BUG-1524 | ai:claude
                    // BUG-1629: no lease and no receipt means the child
                    // never started an implementer session, so there is no
                    // session to `--resume`; say so instead of suggesting it.
                    // trace:BUG-1629 | ai:claude
                    auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::LaunchRefused,
                        format!(
                            "the child session {} neither retained its session lease nor wrote its \
                             orchestrator handoff receipt, so no implementer session started and \
                             there is nothing to resume; unrelated active leases were ignored.",
                            &claude_session_id[..claude_session_id.len().min(8)],
                        ),
                    )
                }
            })
    }

    /// BUG-1629: the workspace this phase-1 launch uses. A BUG-908 retry pin
    /// wins (the substrate re-targets the predecessor's worktree); otherwise
    /// the workspace the orchestrator checked and pinned. A direct caller that
    /// skipped the orchestrator check resolves exactly once and pins the
    /// result. A resolver failure refuses the launch: the implementer is never
    /// launched without a pinned worktree (BUG-1244).
    ///
    /// The first launch of a freshly resolved branch also closes the explicit
    /// `--branch` reuse race. `queue work --branch X` checks out `X` when it
    /// already exists (the TASK-245 fixup rule), while an ordinary pickup only
    /// ever picks a free name. A branch that was free when phase 1 resolved it
    /// but exists at launch was created by someone else in between, so the
    /// launch is refused instead of adopting it. Retries, and workspaces taken
    /// from this spec's own lease, keep the TASK-245 reuse: that branch is
    /// this spec's own.
    // trace:BUG-1629 | ai:claude
    pub(crate) fn phase1_launch_workspace(
        &mut self,
    ) -> Result<(std::path::PathBuf, String), auto_complete::PhaseFailure> {
        if let (Some(worktree), Some(branch)) = (
            &self.retry_implementer_worktree,
            &self.retry_implementer_branch,
        ) {
            return Ok((worktree.clone(), branch.clone()));
        }
        if self.phase1_workspace.is_none() {
            match auto_complete::PhaseDriver::implementer_workspace(self) {
                Ok(Some((worktree, branch))) => {
                    auto_complete::PhaseDriver::pin_implementer_workspace(self, &worktree, &branch)
                }
                Ok(None) => {
                    return Err(auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::LaunchRefused,
                        auto_complete::phase1_resolver_failure_reason(
                            &self.spec,
                            "no workspace was resolved",
                        ),
                    ))
                }
                Err(err) => {
                    return Err(auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::LaunchRefused,
                        auto_complete::phase1_resolver_failure_reason(&self.spec, &err),
                    ))
                }
            }
        }
        let project_root = self.project_root.clone();
        let spec = self.spec.clone();
        let pin = self
            .phase1_workspace
            .as_mut()
            .expect("phase-1 workspace pinned above");
        if pin.fresh {
            let exists_now = branch_exists_anywhere(&project_root, &pin.branch);
            if phase1_fresh_branch_raced(pin.fresh, exists_now) {
                return Err(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::LaunchRefused,
                    phase1_branch_race_reason(&spec, &pin.branch),
                ));
            }
            pin.fresh = false;
        }
        Ok((pin.worktree.clone(), pin.branch.clone()))
    }

    /// BUG-1629: a phase-1 child exited without its session lease and without
    /// its orchestrator handoff receipt, so no implementer session started
    /// (nothing ran, nothing to resume). Leave no lease behind, then launch
    /// one clean replacement. When the pinned worktree already exists on the
    /// pinned branch with no lease holding it, the replacement re-enters it
    /// through the BUG-908 retry pin; otherwise it launches fresh on the same
    /// pinned workspace. A second loss is final: the typed `LaunchRefused`
    /// failure shelves the spec, so dependents stay blocked.
    ///
    /// Launch budget: this replacement is the only launch this method adds.
    /// Each launch (first or replacement) can still spend the separate
    /// BUG-826 zero-byte-log budget (two relaunches in total per phase 1
    /// run), so the total number of child launches can exceed two. Both
    /// counters live on the driver and are never reset, so the total stays
    /// bounded.
    // trace:BUG-1629 | ai:claude
    pub(crate) fn recover_lost_child_state(
        &mut self,
        session_uuid: &str,
        exit: &str,
        elapsed: std::time::Duration,
    ) -> Result<auto_complete::ImplementerOutcome, auto_complete::PhaseFailure> {
        // No orphan: release any lease keyed to this session id (a no-op when
        // the child minted none) and drop a partial receipt.
        self.release_empty_launch_lease(session_uuid);
        let _ = std::fs::remove_file(orchestrated_lease_receipt_path(
            &self.project_root,
            session_uuid,
        ));
        // The child's own refusal (a preflight/lease refusal printed on its
        // stderr), when `queue work` recorded one for this session.
        let refusal = take_orchestrated_child_refusal(&self.project_root, session_uuid);
        let mut state = lost_child_state_reason(session_uuid, exit, elapsed);
        if let Some(refusal) = &refusal {
            state.push_str(&format!("; the child refused: {refusal}"));
        }
        // BUG-1769: this is the arbiter BUG-1140 mandated, and until now it
        // consulted no substrate signal at all. Its whole decision input was
        // lease-absent + receipt-absent + the retry budget — which cannot see
        // the case BUG-1140 describes: a child that committed, pushed, advanced
        // the spec to Done and then exited without its lease or receipt. For
        // BUG-1817 that cost a replacement launch the child's own claim gate
        // refused with "Status is Done (work finished on a branch)" — a success
        // report — and then demoted a finished spec to NeedsAttention.
        // trace:BUG-1769 trace:BUG-1140 | ai:claude
        let current_status = spec_status(&self.project_root, &self.spec);
        let entry_status = self
            .phase1_entry_spec_status
            .as_ref()
            .and_then(|status| status.as_ref());
        if phase1_advanced_to_finished_during_run(entry_status, current_status.as_ref()) {
            return self.resolve_phase1_finished_without_lease(&state, current_status.as_ref());
        }
        if !phase1_lost_child_retry_allowed(self.lost_child_state_retries_used) {
            let how = if refusal.is_some() {
                "was refused"
            } else {
                "lost its state the same way"
            };
            // BUG-1769: this message used to assert "<spec> was not advanced"
            // with nothing behind it, and said exactly that about a run that
            // HAD advanced the spec to Done. The no-advance claim is now
            // scoped to the replacement launch this method spent, and the
            // status it reports is the `spec_status` read above.
            //
            // The elapsed time in `{state}` is the replacement child's OWN
            // lifetime. Phase 1's recorded `phase_durations` entry covers every
            // launch in the phase — BUG-1817 recorded 426 837 ms for a phase
            // whose blamed child lived 302 ms — so the message says which child
            // the elapsed time belongs to rather than leaving the reader to
            // attribute the phase total to it. trace:BUG-1769 | ai:claude
            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::LaunchRefused,
                format!(
                    "{state}. The single clean replacement launch already ran and {how}; no \
                     session lease was left behind, and that replacement launch did not \
                     advance {spec} — the store reports {spec} as {status}. The elapsed time \
                     above is that replacement child's own lifetime; phase 1's recorded \
                     duration covers every launch in the phase, not this child alone",
                    spec = self.spec,
                    status = phase1_spec_status_text(current_status.as_ref()),
                ),
            ));
        }
        self.lost_child_state_retries_used += 1;
        if self.retry_implementer_worktree.is_none() {
            if let Some(pin) = &self.phase1_workspace {
                let lease_worktrees: Vec<std::path::PathBuf> = list_leases(&self.project_root)
                    .into_iter()
                    .map(|lease| lease.worktree_path)
                    .collect();
                if pin.worktree.exists()
                    && current_branch_at(&pin.worktree).as_deref() == Some(pin.branch.as_str())
                    && worktree_is_unleased(&pin.worktree, &lease_worktrees)
                {
                    self.retry_implementer_worktree = Some(pin.worktree.clone());
                    self.retry_implementer_branch = Some(pin.branch.clone());
                }
            }
        }
        self.record_lost_child_replacement(&state);
        auto_complete::PhaseDriver::run_implementer(self)
    }

    /// BUG-1769: read the spec's store status once, at phase-1 entry. Later
    /// calls are no-ops: `run_implementer` is re-entered by the BUG-1629
    /// replacement launch and the BUG-826 zero-byte-log relaunches, and a
    /// second capture would replace the entry reading with a mid-run one, which
    /// is precisely the comparison this field exists to make.
    // trace:BUG-1769 | ai:claude
    pub(crate) fn capture_phase1_entry_spec_status(&mut self) {
        if self.phase1_entry_spec_status.is_none() {
            self.phase1_entry_spec_status = Some(spec_status(&self.project_root, &self.spec));
        }
    }

    /// BUG-1769: phase 1's child lost its lease and its handoff receipt, but
    /// the substrate says THIS run finished the work — the spec advanced to
    /// `Done` (or beyond) from an entry status that was not finished. Spending
    /// the BUG-1629 replacement launch here is pure waste: the child's own
    /// claim gate refuses an already-`Done` spec, and its refusal text ends
    /// "nothing to do — auto-bump fires when the PR merges", which is a success
    /// report. Typing that refusal `LaunchRefused` is what demoted BUG-1817
    /// from `Done` to `NeedsAttention` and auto-drafted a non-bug for a human.
    ///
    /// So resolve it the way a lease-bearing child's work is resolved, against
    /// the pinned phase-1 branch: an open PR means `PrOpened` and the run
    /// continues into CI + review (the real quality gates), a merged one means
    /// `AlreadyMerged` (BUG-709 — the implementer ran the full ship itself),
    /// and committed-but-unopened work is recovered into a PR (BUG-893). This
    /// is the `ImplementerOutcome::AlreadyMerged` shape applied to a second
    /// instance of the same defect class: the work succeeded, the orchestrator
    /// looked for its customary artifact, did not find it, and false-negatived
    /// a finished drive.
    ///
    /// Only when no PR can be found OR opened is the result `Inconclusive`: the
    /// work is real but its artifact is not locatable, so the drain pauses and
    /// leaves the spec where it is. That is deliberately not a failure — the
    /// one thing this path must never do is demote a spec this run finished.
    // trace:BUG-1769 | ai:claude
    pub(crate) fn resolve_phase1_finished_without_lease(
        &mut self,
        state: &str,
        current_status: Option<&RequirementStatus>,
    ) -> Result<auto_complete::ImplementerOutcome, auto_complete::PhaseFailure> {
        let status = phase1_spec_status_text(current_status);
        let Some((worktree, branch)) = self
            .phase1_workspace
            .as_ref()
            .map(|pin| (pin.worktree.clone(), pin.branch.clone()))
        else {
            return Ok(auto_complete::ImplementerOutcome::Inconclusive {
                reason: format!(
                    "{state}; but the store reports {} as {status}, so this run finished the \
                     work — no replacement launch was spent and nothing was shelved. No \
                     phase-1 workspace was pinned, so there is no branch to look a PR up on",
                    self.spec
                ),
                retry_hint: None,
            });
        };
        if !self.json {
            eprintln!(
                "  {} {} is {} — this run finished the work, so the lost child's missing lease \
                 is not a failure; resolving the PR on `{}` instead of relaunching",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                self.spec,
                status,
                branch,
            );
        }
        self.implementer_worktree = Some(worktree.clone());
        self.branch = Some(branch.clone());
        match self.detect_phase1_pr(&branch) {
            Phase1PrResolve::Found(pr) => {
                self.set_pr_number(pr.number as u32);
                // Same adoption point as the main phase-1 verify: a reused
                // draft PR must be un-drafted or the merge is refused.
                // trace:BUG-1690 | ai:claude
                self.undraft_reused_publication(&pr);
                if !self.json {
                    eprintln!(
                        "  {} PR-{} found on `{}` — continuing into CI + review",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        pr.number,
                        branch,
                    );
                }
                Ok(auto_complete::ImplementerOutcome::PrOpened)
            }
            // BUG-709: the implementer ran the full ship itself, so there is no
            // open PR to shepherd and nothing to tear down (this child never
            // took a lease). trace:BUG-709 | ai:claude
            Phase1PrResolve::AlreadyMerged(pr) => {
                self.set_pr_number(pr.number as u32);
                if !self.json {
                    eprintln!(
                        "  {} PR-{} already merged — completing the drive",
                        crate::glyph(crate::glyphs::Glyph::Check).green(),
                        pr.number,
                    );
                }
                Ok(auto_complete::ImplementerOutcome::AlreadyMerged {
                    pr_number: pr.number as u32,
                })
            }
            // No open PR, or the lookup could not reach the forge. Either way
            // the commits may be sitting in the pinned worktree unopened —
            // BUG-893's recovery is the same one the lease-bearing path runs.
            // trace:BUG-893 | ai:claude
            other => {
                if let Some((ahead, pr)) = try_open_orchestrator_pr_for_no_pr_worktree(
                    &self.project_root,
                    &worktree,
                    &branch,
                    self.lifecycle_forge,
                    &self.spec,
                ) {
                    if !self.json {
                        eprintln!(
                            "  {} {} commit(s) on `{}` with no PR — opened PR-{} for them \
                             (BUG-893 recovery)",
                            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                            ahead,
                            branch,
                            pr,
                        );
                    }
                    self.set_pr_number(pr as u32);
                    return Ok(auto_complete::ImplementerOutcome::PrOpened);
                }
                let why = match other {
                    Phase1PrResolve::NoPr => "no open PR on the branch".to_string(),
                    Phase1PrResolve::Fail(f) => f.reason,
                    Phase1PrResolve::Retry(reason) => reason,
                    Phase1PrResolve::Found(_) | Phase1PrResolve::AlreadyMerged(_) => {
                        unreachable!("resolved above")
                    }
                };
                Ok(auto_complete::ImplementerOutcome::Inconclusive {
                    reason: format!(
                        "{state}; but the store reports {} as {status}, so this run finished \
                         the work — no replacement launch was spent and nothing was shelved. \
                         No PR could be found or opened for `{branch}`: {why}",
                        self.spec
                    ),
                    retry_hint: None,
                })
            }
        }
    }

    /// BUG-1629: leave a durable trace of the replacement launch — the drain
    /// retry ledger, a `SpecRetried` event (cause `lost-child-state`), and a
    /// `retrying` line under `--json` — the same channels
    /// `record_transient_retry` uses, without its model escalation.
    // trace:BUG-1629 | ai:claude
    pub(crate) fn record_lost_child_replacement(&self, state: &str) {
        let phase = auto_complete::Phase::Implementer;
        drain_state::append_phase_retry(
            &self.project_root,
            &self.spec,
            &format!("{} ({})", phase.index(), phase.slug()),
            PHASE1_LOST_CHILD_STATE_CAUSE,
            2,
            2,
        );
        let (_, run_uuid) = drain_state::current_context(&self.project_root);
        events::emit(
            &self.project_root,
            &events::Event::new(
                Some(self.spec.clone()),
                run_uuid,
                events::EventKind::SpecRetried {
                    phase: phase.slug().to_string(),
                    cause: PHASE1_LOST_CHILD_STATE_CAUSE.to_string(),
                    attempt: 2,
                    max: 2,
                    model_before: None,
                    model_after: None,
                    detail: Some(state.to_string()),
                },
            ),
        );
        if self.json {
            println!(
                "{}",
                auto_complete::phase_event(
                    phase.slug(),
                    "retrying",
                    &self.spec,
                    0,
                    None,
                    &[
                        ("kind", PHASE1_LOST_CHILD_STATE_CAUSE),
                        ("attempt", "2"),
                        ("max", "2"),
                        ("detail", state),
                    ],
                )
            );
        } else {
            eprintln!(
                "  {} {state}; launching one clean replacement (attempt 2/2)",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
            );
        }
    }

    /// BUG-826: after an empty-log phase-1 vendor death, release only the lease
    /// minted for that failed session UUID so the next retry starts clean.
    // trace:BUG-826 | ai:codex
    pub(crate) fn release_empty_launch_lease(&mut self, session_uuid: &str) {
        let Some((lease_id, _branch, worktree, _)) =
            find_orchestrated_lease(&self.project_root, session_uuid)
        else {
            return;
        };
        self.implementer_lease = Some(lease_id.clone());
        self.implementer_worktree = Some(worktree);
        if let Err(e) = self.end_implementer_session() {
            if !self.json {
                eprintln!(
                    "  {} empty-log launch left lease {}, but auto-release failed ({}) - \
                     the next retry may need `--force-claim`",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                    &lease_id[..lease_id.len().min(8)],
                    e.reason,
                );
            }
        } else {
            // BUG-1716: the lease is gone — forget it. Leaving the field set
            // made the TASK-133 compensation gate read "lease acquired ⇒ work
            // may exist" and skip the pre-bump status restore, stranding the
            // never-started spec NeedsAttention (blocked from the safe retry
            // as `needs-triage`) — the exact demotion this empty-launch lane
            // exists to prevent. On a failed auto-release the fields stay set
            // on purpose: a real lease survives on disk, and the BUG-479
            // probe should keep refusing the restore for it.
            // trace:BUG-1716 | ai:claude
            self.implementer_lease = None;
            self.implementer_worktree = None;
            if !self.json {
                eprintln!(
                    "  {} released empty-log launch lease {}",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    &lease_id[..lease_id.len().min(8)],
                );
            }
        }
    }

    /// Ground-truth check for the BUG-241 reconcile: find a *merged* PR that
    /// credits the dispatched spec. BUG-357 makes the spec-credit check
    /// mandatory: an unrelated merged PR on the same/misattributed branch is
    /// not evidence that this spec shipped. Prefers the PR number an earlier
    /// phase already discovered (checked directly with
    /// [`pr_is_merged_with_sink`]); otherwise looks one up by branch.
    /// `None` when no merged PR exists, the PR credits a different spec, or
    /// `gh` can't answer — all leave the original failure standing.
    /// trace:BUG-241, BUG-286, BUG-357 | ai:claude
    pub(crate) fn detect_merged_pr(&self) -> Option<u32> {
        // BUG-286: route the orchestrator's reconcile-time `gh pr view`
        // through the retry helper with stderr + drain-state sinks. A
        // transient blip in this read used to leave the failure standing
        // (returning `None` is "cannot confirm", which leaves reconcile_failure
        // at GenuineFailure). Retry first, surface the retry to both channels.
        // trace:BUG-286 | ai:claude
        let mut stderr_sink = network_retry::StderrSink;
        let mut state_sink = drain_state::DrainStateSink {
            project_root: &self.project_root,
            spec: self.spec.clone(),
            phase: Some(format!(
                "{} (reconcile)",
                auto_complete::Phase::Reviewer.index()
            )),
        };
        let mut sink = network_retry::DualSink {
            a: &mut stderr_sink,
            b: &mut state_sink,
        };
        let latest_reopen_at = spec_latest_reopen_transition_at(&self.project_root, &self.spec);
        if let Some(pr) = self.pr_number {
            // We already know the PR — `gh pr view` is the direct check.
            if pr_is_merged_with_sink(&self.project_root, pr, &mut sink) != Some(true) {
                return None;
            }
            if !self.merged_pr_satisfies_latest_reopen(pr, latest_reopen_at, &mut sink) {
                return None;
            }
            return matches!(
                pr_credit_match_with_sink(&self.project_root, pr, &self.spec, None, &mut sink),
                PrCreditMatch::Dispatched
            )
            .then_some(pr);
        }
        // Phase 1 never resolved a PR — try a branch-keyed merged-PR lookup.
        // `detect_merged_pr_for_branch` already classifies network errors
        // into `PrLookup::GhUnreachable` and the orchestrator already absorbs
        // them as "cannot confirm" — so the branch leg stays as-is.
        match self
            .branch
            .as_deref()
            .map(|b| detect_merged_pr_for_branch_via_forge(&self.project_root, b))
        {
            Some(PrLookup::Found(pr)) => {
                if !self.merged_pr_satisfies_latest_reopen(
                    pr.number as u32,
                    latest_reopen_at,
                    &mut sink,
                ) {
                    return None;
                }
                matches!(
                    pr_credit_match_with_sink(
                        &self.project_root,
                        pr.number as u32,
                        &self.spec,
                        Some(&pr.title),
                        &mut sink,
                    ),
                    PrCreditMatch::Dispatched
                )
                .then_some(pr.number as u32)
            }
            _ => None,
        }
    }

    // trace:BUG-1112 | ai:codex
    pub(crate) fn merged_pr_satisfies_latest_reopen(
        &self,
        pr: u32,
        latest_reopen_at: Option<chrono::DateTime<chrono::Utc>>,
        sink: &mut dyn network_retry::RetrySink,
    ) -> bool {
        if latest_reopen_at.is_none() {
            return true;
        }
        let metadata = self.project_forge().change_metadata(pr as u64, sink).ok();
        let merged_at = metadata
            .filter(|m| m.state == crate::forge::ChangeState::Merged)
            .and_then(|m| m.merged_at);
        merged_at_satisfies_latest_reopen(merged_at, latest_reopen_at)
    }

    /// STORY-301: stamp the drain-state file with the phase about to run, so
    /// `aida drain status` (and the `/aida-pickup` banner) show live progress.
    /// Best-effort — a missing file is a silent no-op, never blocks the phase.
    ///
    /// TASK-1292: also carries the run's frozen PR binding (`phase_done_pr`,
    /// falling back to the raw `pr_number` before phase 1 has frozen it) so
    /// the member's `pr` field is live the moment a phase starts, not only
    /// once it finishes. This is what makes `aida pr ship`'s drive-ownership
    /// check PR-keyed instead of spec-keyed — see
    /// `pr_ship::reviewer_liveness_for_pr`.
    // trace:STORY-301 | ai:claude
    // trace:TASK-1292 | ai:claude
    pub(crate) fn mark_drain_phase(&self, phase: auto_complete::Phase) {
        let vendor = session::resolve_headless_vendor(&self.project_root);
        let seat = agent_seat_for_phase(phase);
        let tuning = session::resolve_agent_tuning(&self.project_root, vendor, seat);
        drain_state::set_phase_with_tuning(
            &self.project_root,
            &self.spec,
            phase.index(),
            phase.slug(),
            Some(vendor.as_str()),
            Some(seat.as_str()),
            tuning.model.as_deref(),
            tuning.effort.as_deref(),
            self.phase_done_pr.or(self.pr_number),
        );
    }

    /// BUG-872: stamp the phase with the exact headless session id whose log
    /// should be treated as current. Status/tail readers must not select an
    /// older retry's log for the same spec while the fresh phase is starting.
    ///
    /// BUG-1290: `announce` must be `true` ONLY when this call is the phase's
    /// sole entry point (no preceding `mark_drain_phase` call in this same
    /// function — today that's just `run_implementer`, phase 1). Every other
    /// caller already announced the phase via `mark_drain_phase` and must
    /// pass `false` so this call attaches the session without re-emitting a
    /// second `PhaseEntered` for the same entry — the duplicate this spec
    /// fixes.
    // trace:BUG-872 trace:BUG-1290 | ai:claude
    pub(crate) fn mark_drain_phase_session(
        &self,
        phase: auto_complete::Phase,
        session_id: &str,
        vendor: session::HeadlessVendor,
        announce: bool,
    ) {
        drain_state::set_phase_session_vendor(
            &self.project_root,
            &self.spec,
            phase.index(),
            phase.slug(),
            session_id,
            vendor,
            announce,
        );
    }

    /// STORY-347: spawn one headless advisor (cold-boot OR fork) against the
    /// already-written punt request and collect its [`punt::PuntResponse`] +
    /// JSONL log path. Factored out so the calibration loop can run two
    /// passes against the same request — one cold-boot driving the drain,
    /// one fork-from-live as shadow. Returns the response and absolute log
    /// path; an error is a phase-fatal (the spawn failed or the advisor
    /// wrote no usable response).
    ///
    /// `response_path` is the file the advisor will write to — distinct
    /// paths for the primary and shadow passes so they don't clobber. The
    /// caller is responsible for clearing any stale file beforehand.
    /// trace:STORY-347 | ai:claude
    pub(crate) fn spawn_advisor_session(
        &self,
        pass: AdvisorPass,
        advisor_cfg: &advisor::AdvisorConfig,
        response_path: &std::path::Path,
        request_path: &std::path::Path,
    ) -> Result<(punt::PuntResponse, std::path::PathBuf), auto_complete::PhaseFailure> {
        use auto_complete::{FailureKind, PhaseFailure};

        // TASK-894: resolve the vendor that hosts the advisor tier. Default is
        // Claude — selecting Codex (via `AIDA_HEADLESS_VENDOR=codex` or
        // `[orchestrator] headless_vendor = "codex"`) is the explicit opt-in,
        // mirroring STORY-683's drain-phase generalization. A non-Claude vendor
        // has no `--resume` / session model, so the fork-from-live JSONL
        // machinery (Claude-specific) must not run; force the pass to cold-boot
        // and host a fresh spawn per punt. trace:TASK-894 | ai:claude
        let vendor = session::resolve_headless_vendor(&self.project_root);
        let pass = match pass {
            AdvisorPass::Fork(_) if vendor != session::HeadlessVendor::Claude => {
                if !self.json {
                    eprintln!(
                        "  {} vendor `{}` has no resumable session — hosting a fresh advisor per punt (no fork).",
                        "◆".dimmed(),
                        vendor.as_str()
                    );
                }
                AdvisorPass::ColdBoot
            }
            other => other,
        };

        let (advisor_uuid, forked_from) = match &pass {
            AdvisorPass::Fork(plan) => match advisor::execute_fork(plan) {
                Ok(_) => (plan.fork_uuid.clone(), Some(plan.live.clone())),
                Err(e) => {
                    if !self.json {
                        eprintln!(
                            "  {} advisor fork failed ({e}); falling back to cold-boot.",
                            "◆".yellow()
                        );
                    }
                    (uuid::Uuid::now_v7().to_string(), None)
                }
            },
            AdvisorPass::ColdBoot => (uuid::Uuid::now_v7().to_string(), None),
        };
        let is_fork = forked_from.is_some();

        let log_path = self
            .project_root
            .join(".aida")
            .join("headless-logs")
            .join(format!(
                "advise-{}-{}.jsonl",
                self.spec,
                &advisor_uuid[..advisor_uuid.len().min(8)]
            ));
        if let Some(dir) = log_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let log = std::fs::File::create(&log_path).map_err(|e| {
            PhaseFailure::of(
                FailureKind::Spawn,
                format!("advisor tier: could not create the headless log: {e}"),
            )
        })?;
        if !self.json {
            if let Some(live) = forked_from.as_ref() {
                let mb = live.jsonl_size_bytes as f64 / (1024.0 * 1024.0);
                eprintln!(
                    "  {} forking live advisor session {} ({:.2} MB transcript, via {}) for the design-fork…",
                    "◆".cyan(),
                    &live.uuid[..live.uuid.len().min(8)],
                    mb,
                    live.discovery.label(),
                );
            } else {
                eprintln!(
                    "  {} spawning a headless advisor (cold-boot) for the design-fork…",
                    "◆".cyan()
                );
            }
        }
        let tee_label = if is_fork { "advisor-fork" } else { "advisor" };
        let tee_opts =
            crate::headless_tee::TeeOptions::from_env_and_flag(false).with_label(tee_label);
        let tee_handle = crate::headless_tee::start_tee(&log_path, &tee_opts);
        // STORY-626: the cold-boot branch is context-poor — seed it with the
        // live advisor context file (same prepend the assess cold-boot uses),
        // so unattended punt decisions match the live session. trace:STORY-626
        // TASK-894: build the per-vendor `(program, args)` — Claude resumes the
        // forked session or cold-boots; Codex always hosts a fresh `codex exec`
        // (no resume). `is_fork` is always false for a non-Claude vendor (the
        // pass was forced to cold-boot above), so the fork JSONL is never read.
        // trace:TASK-894 | ai:claude
        // TASK-1045: for a non-Claude vendor the `/aida-advise` slash token is
        // inert (Codex/Gemini don't read `.claude/skills/`), so inline the
        // embedded skill body; Claude keeps the slash form and expands it.
        let is_claude = matches!(vendor, session::HeadlessVendor::Claude);
        let seeded = crate::intake::seeded_advise_prompt_for_vendor(&self.project_root, is_claude);
        let advisor_tuning = session::resolve_agent_tuning(
            &self.project_root,
            vendor,
            aida_core::agents_config::AgentSeat::Advisor,
        );
        let (program, advisor_args) = session::advisor_tier_program_and_args_with_tuning(
            vendor,
            is_fork,
            &seeded,
            &advisor_uuid,
            advisor_tuning.model.as_deref(),
            advisor_tuning.effort.as_deref(),
        );
        // trace:TASK-1169 | ai:claude
        let (ceiling_key, ceiling_value) = crate::bg_wait_ceiling_env(Some(&self.project_root));
        let status = std::process::Command::new(&program)
            .current_dir(&self.project_root)
            .args(advisor_args)
            .env("AIDA_HEADLESS", "1")
            .env(punt::REQUEST_FILE_ENV, request_path)
            .env(punt::RESPONSE_FILE_ENV, response_path)
            // TASK-586: the advisor tier runs as the advisor role.
            .env("AIDA_SESSION_ROLE", "advisor")
            // TASK-1169 / ADR-22: bounded, launcher-set background-wait ceiling
            // — every headless child, not just the burndown ones.
            // trace:TASK-1169 | ai:claude
            .env(ceiling_key, ceiling_value)
            .stdout(std::process::Stdio::from(log))
            .status_retrying_etxtbsy()
            .map_err(|e| {
                PhaseFailure::of(
                    FailureKind::Spawn,
                    format!("advisor tier: could not launch the advisor session: {e}"),
                )
            })?;
        tee_handle.stop();

        // Clean up the fork JSONL if the config asks us to. Default is to
        // keep it for audit. Errors are non-fatal.
        if is_fork && !advisor_cfg.keep_fork_jsonls {
            if let AdvisorPass::Fork(plan) = &pass {
                let _ = std::fs::remove_file(&plan.fork_jsonl);
            }
        }
        if !status.success() {
            return Err(PhaseFailure::new(format!(
                "the headless advisor session exited {} — see {}",
                status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "with a signal".to_string()),
                log_path.display(),
            )));
        }
        let response = punt::read_punt_response(response_path).ok_or_else(|| {
            PhaseFailure::of(
                FailureKind::Internal,
                format!(
                    "the headless advisor wrote no usable response — see {}",
                    log_path.display()
                ),
            )
        })?;
        Ok((response, log_path))
    }
}

/// STORY-347: which kind of advisor pass to spawn — a fresh cold-boot
/// `claude -p` (substrate only) or a fork-from-live `claude --resume` on a
/// copied JSONL. The orchestrator decides which up front; this enum is the
/// internal type that flows into `spawn_advisor_session`.
/// trace:STORY-347 | ai:claude
pub(crate) enum AdvisorPass {
    ColdBoot,
    Fork(advisor::ForkPlan),
}

/// STORY-347: resolve the effective calibration mode for the current
/// orchestrator run. The `AIDA_CALIBRATE=1`/`0` env var (set by the
/// `--calibrate`/`--no-calibrate` queue-work flags) overrides
/// `[advisor] calibration_mode` from `.aida/config.toml`.
/// trace:STORY-347 | ai:claude
pub(crate) fn effective_calibration_mode(cfg: &advisor::AdvisorConfig) -> advisor::CalibrationMode {
    match std::env::var("AIDA_CALIBRATE").ok().as_deref() {
        Some("1") | Some("true") | Some("on") | Some("yes") => advisor::CalibrationMode::On,
        Some("0") | Some("false") | Some("off") | Some("no") => advisor::CalibrationMode::Off,
        _ => cfg.calibration_mode,
    }
}

/// Decide whether phase 2 can safely continue after phase 1 removed its
/// recorded worktree. Both durable views must identify the same published
/// commit; otherwise the push/teardown guard has lost the evidence it needs.
// trace:BUG-1823 | ai:codex
pub(crate) fn missing_implementer_worktree_push_gate(
    worktree: &std::path::Path,
    branch: &str,
    pr: u32,
    remote_head: Option<&str>,
    pr_head: Option<&str>,
) -> Result<(), auto_complete::PhaseFailure> {
    let evidence_failure = |detail: &str| {
        auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::CiUnavailable,
            format!(
                "implementer worktree `{}` disappeared before phase-2 CI/teardown, and {detail}",
                worktree.display()
            ),
        )
    };
    let remote_head = remote_head
        .filter(|head| !head.trim().is_empty())
        .ok_or_else(|| {
            evidence_failure(&format!(
                "origin branch `{branch}` could not be verified as published"
            ))
        })?;
    let pr_head = pr_head
        .filter(|head| !head.trim().is_empty())
        .ok_or_else(|| evidence_failure(&format!("PR/MR {pr} head could not be read")))?;
    if !remote_head.eq_ignore_ascii_case(pr_head) {
        return Err(evidence_failure(&format!(
            "origin branch `{branch}` is at `{remote_head}` while PR/MR {pr} is at `{pr_head}`"
        )));
    }
    Ok(())
}

impl RealPhaseDriver {
    pub(crate) fn ensure_implementer_branch_pushed(
        &self,
        branch: &str,
    ) -> Result<(), auto_complete::PhaseFailure> {
        let Some(worktree) = self.implementer_worktree.as_deref() else {
            // BUG-1151 (BUG-1145 residual): the open-PR recovery re-enters at Ci
            // with no local worktree recorded (the lease-match failure returned
            // before it was set). The branch is already pushed — the PR exists
            // remotely — so there is nothing to push here. Only error when this
            // is NOT the recovery path (no PR / not from_pr), where a missing
            // worktree is a genuine internal bug. trace:BUG-1151 | ai:claude
            if self.from_pr || self.pr_number.is_some() {
                return Ok(());
            }
            return Err(auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::Internal,
                "internal: implementer worktree not recorded before the CI phase",
            ));
        };
        if !worktree.is_dir() {
            let pr = self.pr_number.ok_or_else(|| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::Internal,
                    format!(
                        "implementer worktree `{}` disappeared before phase-2 CI/teardown and no verified PR/MR was recorded",
                        worktree.display()
                    ),
                )
            })?;
            let remote_head = dispatched_branch_head_sha(&self.project_root, branch);
            let pr_head = pr_head_sha_best_effort(self, pr);
            return missing_implementer_worktree_push_gate(
                worktree,
                branch,
                pr,
                remote_head.as_deref(),
                pr_head.as_deref(),
            );
        }
        ensure_implementer_branch_pushed(worktree, branch, self.json)
    }

    /// End the implementer session: `aida session end <lease> --yes --skip-ci`.
    /// Releases the lease, returns the warm-pool worktree to idle, and (only
    /// when an OPEN PR still exists on the branch) auto-queues the `Review
    /// PR-N` hand-off. Shared by phase 2 (`finish_ci`) and the BUG-709
    /// AlreadyMerged completion so a self-merged drive tears its session down
    /// the same way a normal drive does, instead of leaking the lease + pool
    /// worktree. A merged PR leaves no open PR, so no spurious reviewer item is
    /// filed.
    // trace:BUG-711 | ai:claude
    pub(crate) fn end_implementer_session(&self) -> Result<(), auto_complete::PhaseFailure> {
        // BUG-1145: the phase-1 recovery path (open PR found after a lease-match
        // failure) re-enters at Ci with NO matched implementer lease recorded —
        // discover_orchestrated_lease failed before self.implementer_lease was
        // set. There is then no session to end here; the orphan lease/worktree
        // is cleaned by a later `aida session reap`. Treat a missing lease as a
        // best-effort no-op rather than an Internal failure that would shelve
        // the (successful) PR. trace:BUG-1145 | ai:claude
        let Some(lease) = self.implementer_lease.clone() else {
            return Ok(());
        };
        let status = std::process::Command::new(self.aida_exe())
            .current_dir(&self.project_root)
            .args(["session", "end", &lease, "--yes", "--skip-ci"])
            .status_retrying_etxtbsy()
            // A spawn failure here is local subprocess plumbing, not red CI —
            // tag it `Spawn` so the hint says so. trace:BUG-218 | ai:claude
            .map_err(|e| {
                auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::Spawn,
                    format!("could not run `aida session end`: {e}"),
                )
            })?;
        if !status.success() {
            return Err(auto_complete::PhaseFailure::new(
                "could not end the implementer session — it may have uncommitted \
                 changes; commit or discard them, then re-run",
            ));
        }
        Ok(())
    }

    /// TASK-1230: best-effort removal of a FAILED phase-1 attempt's orphan
    /// worktree during open-PR recovery. NEVER fails/shelves the spec — a
    /// removal problem is logged and swallowed; the drain proceeds to CI on the
    /// verified PR exactly as before. Safety is the pure
    /// [`auto_complete::recovery_worktree_teardown_decision`] gate: a dirty,
    /// locked, or unpushed-local-commit worktree is kept + logged.
    ///
    /// Discovery is deliberately conservative. The recovery path usually has no
    /// recorded lease/worktree (the BUG-1151 reason), so we fall back to
    /// matching the UNIQUE registered worktree checked out on the PR branch —
    /// and we HARD-GUARD against ever removing the main checkout or the
    /// `.aida-store` worktree. Ambiguity (zero or >1 match) → do nothing.
    // trace:TASK-1230 | ai:claude
    pub(crate) fn teardown_failed_attempt_worktree(&mut self, pr_branch: &str) {
        let log = |msg: String| {
            if !self.json {
                eprintln!(
                    "  {} {}",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    msg
                );
            }
        };

        // The one path we never touch, whatever else happens.
        let main_root = find_main_worktree_root().unwrap_or_else(|_| self.project_root.clone());
        let store_wt = main_root.join(".aida-store");

        // Prefer a worktree the driver already recorded for the failed attempt;
        // otherwise discover the unique one on the PR branch.
        let branch = if !pr_branch.is_empty() {
            pr_branch.to_string()
        } else {
            self.branch.clone().unwrap_or_default()
        };
        let candidate = self
            .implementer_worktree
            .clone()
            .or_else(|| self.retry_implementer_worktree.clone())
            .or_else(|| {
                unique_worktree_on_branch(&self.project_root, &branch, &main_root, &store_wt)
            });

        let Some(worktree) = candidate else {
            log(format!(
                "no unambiguous orphan worktree for {} on `{}` — leaving cleanup to `aida session reap`",
                self.spec, branch
            ));
            return;
        };

        // HARD GUARD: never remove the main checkout or the store worktree, even
        // if a recorded field somehow points at one.
        let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        if canon(&worktree) == canon(&main_root) || canon(&worktree) == canon(&store_wt) {
            log(format!(
                "refusing to tear down {} — it is the main/store worktree",
                worktree.display()
            ));
            return;
        }

        let dirty = aida_core::git_ops::worktree_is_dirty(&worktree);
        let locked = worktree_is_locked(&self.project_root, &worktree);
        let unpushed = local_commits_not_on_branch(&worktree, &branch);
        match auto_complete::recovery_worktree_teardown_decision(dirty, locked, unpushed) {
            auto_complete::RecoveryTeardown::Keep(reason) => {
                log(format!(
                    "keeping the failed attempt's worktree {} — {reason}",
                    worktree.display()
                ));
            }
            auto_complete::RecoveryTeardown::Remove => {
                match aida_core::git_ops::remove_worktree_at(&self.project_root, &worktree, false) {
                    Ok(()) => {
                        log(format!(
                            "removed the failed attempt's worktree {} (clean, its work is on PR branch `{}`)",
                            worktree.display(),
                            branch
                        ));
                        // Release the failed attempt's lease so `aida ps` /
                        // `aida session leases` stop listing a gone worktree.
                        if let Some(lease) = self.implementer_lease.clone() {
                            let _ = std::process::Command::new(self.aida_exe())
                                .current_dir(&self.project_root)
                                .args(["session", "end", &lease, "--yes", "--skip-ci"])
                                .status_retrying_etxtbsy();
                        }
                    }
                    Err(e) => log(format!(
                        "worktree {} teardown failed ({e}) — harmless; `aida session reap` will retry",
                        worktree.display()
                    )),
                }
            }
        }
    }

    /// STORY-492: seed the branch + PR a `--resume-drain` re-entry skipped
    /// phases would have discovered, so the resumed phases (CI / reviewer /
    /// merge / pull) have the context they need. Called once before
    /// `orchestrate_with_resume` on a resume. trace:STORY-492 | ai:claude
    pub(crate) fn seed_resume_state(
        &mut self,
        branch: Option<String>,
        pr: Option<u32>,
        head_sha: Option<String>,
        ci_terminal_sha: Option<String>,
        ci_terminal_green: Option<bool>,
    ) {
        if branch.is_some() {
            self.branch = branch;
        }
        if pr.is_some() {
            self.pr_number = pr;
        }
        if head_sha.is_some() {
            self.phase_done_head = head_sha;
        }
        if ci_terminal_sha.is_some() {
            self.ci_terminal_sha = ci_terminal_sha;
        }
        if ci_terminal_green.is_some() {
            self.ci_terminal_green = ci_terminal_green;
        }
    }

    /// TASK-136: one phase-1 PR-verify attempt, factored out of
    /// [`Self::run_implementer`] so the transient GH-unreachable case can be
    /// retried with a backoff. Encapsulates the BUG-223 branch-then-spec
    /// fallback and the BUG-257 `git ls-remote` narrowing. Mutates `self.branch`
    /// when the spec-id search recovers a swapped branch. trace:TASK-136
    pub(crate) fn detect_phase1_pr(&mut self, branch: &str) -> Phase1PrResolve {
        // STORY-516: forge-routed branch lookup (view op). The inner spec-id
        // search (detect_open_pr_for_spec) is the LIST op — routed in a later
        // slice — so it stays PrLookup here. The BUG-444/BUG-257 narrowing
        // bodies are unchanged; only the outer variant names move to
        // ChangeLookup. trace:STORY-516 trace:BUG-444 trace:BUG-257 | ai:claude
        // TASK-1421: the lookups go through the driver's forge seam; with no
        // injected factory this is the same `forge_for` dispatch as
        // `change_lookup_for_branch`. trace:TASK-1421 | ai:claude
        let open_for_branch = pr_lookup_from_change_lookup(
            self.project_forge()
                .change_for_branch(branch)
                .unwrap_or_else(|e| crate::forge::ChangeLookup::CliFailed(format!("{e:#}"))),
        );
        match change_lookup_from_pr_lookup_for_branch(open_for_branch, branch) {
            crate::forge::ChangeLookup::Found(c) => Phase1PrResolve::Found(OpenPrInfo {
                number: c.id,
                title: c.title.unwrap_or_default(),
                url: c.url,
                head_branch: (!c.branch.is_empty()).then_some(c.branch),
            }),
            crate::forge::ChangeLookup::NoChange => {
                let open_for_spec = pr_lookup_from_change_lookup(
                    self.project_forge()
                        .change_for_spec(&self.spec)
                        .unwrap_or_else(|e| {
                            crate::forge::ChangeLookup::CliFailed(format!("{e:#}"))
                        }),
                );
                match open_for_spec {
                    PrLookup::Found(pr) => {
                        // Realign the branch so the CI / merge phases probe the
                        // PR's actual head, not the worktree HEAD the branch-keyed
                        // lookup just missed on. trace:BUG-223
                        if let Some(head) = pr_head_branch(&self.project_root, pr.number) {
                            self.branch = Some(head);
                        }
                        eprintln!(
                            "  {} no open PR on branch `{}` — recovered PR-{} via a `{}` \
                         search (the implementer branch was swapped by /aida-pr's \
                         merged-branch guard)",
                            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                            branch,
                            pr.number,
                            self.spec,
                        );
                        Phase1PrResolve::Found(pr)
                    }
                    // BUG-444: both the `--head` and `--search` lookups returned
                    // empty-BUT-SUCCESSFUL. These are eventually-consistent GitHub
                    // LIST queries (`--search` hits the search index, which lags PR
                    // creation by seconds), so an empty result right after
                    // `/aida-pr` ran `gh pr create` does NOT mean "no PR" — it
                    // usually means "not indexed yet". If the branch is on origin
                    // the session pushed, so a PR very likely exists: RETRY within
                    // the eventual-consistency window (reusing the TASK-136 backoff)
                    // instead of declaring a false NoPr. Only a branch that is
                    // genuinely absent from origin is an immediate NoPr (the session
                    // never pushed). This was the dominant drain-failure cause:
                    // ~98% of sessions ship a PR, yet a whole batch could false-fail
                    // by racing the index. trace:BUG-444 EPIC-33 | ai:claude
                    _ => {
                        let origin = probe_branch_on_origin(&self.project_root, branch);
                        if empty_phase1_lookup_is_definitive_nopr(&origin) {
                            Phase1PrResolve::NoPr
                        } else if let PrLookup::Found(merged) = pr_lookup_from_change_lookup(
                            self.project_forge()
                                .merged_change_for_branch(branch)
                                .unwrap_or_else(|e| {
                                    crate::forge::ChangeLookup::CliFailed(format!("{e:#}"))
                                }),
                        ) {
                            // BUG-709: no OPEN PR but the branch IS on origin —
                            // before assuming eventual-consistency lag and
                            // retrying, check whether the branch's PR already
                            // MERGED. A codex (or any) implementer that ran the
                            // full ship itself (create + CI + merge via `aida pr
                            // ship`) leaves a MERGED PR and no open one, so the
                            // open-PR verify would otherwise spin to its retry
                            // ceiling and false-negative a shipped drive as
                            // "inconclusive, retry". A merged PR means the work
                            // landed — resolve AlreadyMerged so the drive
                            // completes cleanly. trace:BUG-709 | ai:claude
                            Phase1PrResolve::AlreadyMerged(merged)
                        } else {
                            let reason = match origin {
                                BranchOriginProbe::Present => format!(
                                    "no open PR indexed yet for `{branch}` (and the \
                                 `{spec}` search), but the branch is on origin — \
                                 likely GitHub eventual-consistency lag after \
                                 `gh pr create`",
                                    spec = self.spec
                                ),
                                _ => format!(
                                    "no open PR found for `{branch}` and `git ls-remote` \
                                 could not confirm whether the branch is on origin — \
                                 retrying before concluding no PR exists"
                                ),
                            };
                            Phase1PrResolve::Retry(reason)
                        }
                    }
                }
            }
            crate::forge::ChangeLookup::CliMissing => {
                Phase1PrResolve::Fail(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::MissingTool,
                    "`gh` is not on PATH — auto-complete needs it to track the PR",
                ))
            }
            crate::forge::ChangeLookup::CliFailed(why) => {
                Phase1PrResolve::Fail(auto_complete::PhaseFailure::new(format!(
                    "could not look up the PR for `{branch}`: {why}"
                )))
            }
            // BUG-257: the GH API was unreachable — *transient*, not "no PR".
            // Narrow with `git ls-remote` (git protocol, separate from the
            // HTTPS API): if the branch isn't on origin no PR can exist, so
            // that collapses to a genuine NoPR failure. Otherwise it is a
            // retryable inconclusive. trace:BUG-257 TASK-136 | ai:claude
            crate::forge::ChangeLookup::Unreachable(why) => {
                match probe_branch_on_origin(&self.project_root, branch) {
                    BranchOriginProbe::Absent => {
                        Phase1PrResolve::Fail(auto_complete::PhaseFailure::of(
                            auto_complete::FailureKind::NoPr,
                            format!(
                                "GH API unreachable ({why}), and `git ls-remote` confirms \
                             branch `{branch}` is not on origin — no PR can exist. \
                             Resume the session and push: `aida queue work {spec} --resume`",
                                spec = self.spec
                            ),
                        ))
                    }
                    BranchOriginProbe::Present => Phase1PrResolve::Retry(format!(
                        "GH API unreachable ({why}); branch `{branch}` is on origin \
                     so a PR may exist — cannot confirm without the API"
                    )),
                    BranchOriginProbe::LsRemoteFailed => Phase1PrResolve::Retry(format!(
                        "GH API unreachable ({why}); `git ls-remote` also failed — \
                     cannot determine whether a PR exists for `{branch}`"
                    )),
                }
            }
        }
    }
}

pub(crate) fn orchestrator_phase_child_env(
    run_token: &str,
    phase: auto_complete::Phase,
    variant: auto_complete::AutoCompleteVariant,
    queue_user_id: &str,
) -> Vec<(&'static str, String)> {
    let phase_role = match phase {
        auto_complete::Phase::Implementer => Some("implementer"),
        auto_complete::Phase::Reviewer => Some("reviewer"),
        _ => None,
    };
    // trace:BUG-901 | ai:codex
    let mut env = vec![
        (orchestrator::AUTO_COMPLETE_ENV, "1".to_string()),
        (orchestrator::TOKEN_ENV, run_token.to_string()),
        (orchestrator::VARIANT_ENV, variant.slug().to_string()),
        (orchestrator::PHASE_ENV, phase.index().to_string()),
    ];
    // trace:BUG-1038 | ai:codex
    if !queue_user_id.trim().is_empty() {
        env.push(("AIDA_USER", queue_user_id.to_string()));
    }
    if let Some(role) = phase_role {
        env.push(("AIDA_SESSION_ROLE", role.to_string()));
    }
    env
}

pub(crate) fn git_output_checked(
    worktree: &std::path::Path,
    args: &[&str],
) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .current_dir(worktree)
        .args(args)
        .output()
        .map_err(|e| format!("could not invoke `git {}`: {e}", args.join(" ")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("`git {}` exited {}", args.join(" "), out.status)
        } else {
            format!("`git {}` failed: {stderr}", args.join(" "))
        })
    }
}

pub(crate) fn parse_git_count(s: &str, context: &str) -> Result<u32, auto_complete::PhaseFailure> {
    s.trim().parse::<u32>().map_err(|_| {
        auto_complete::PhaseFailure::of(
            auto_complete::FailureKind::Internal,
            format!(
                "could not parse `{context}` count from git output `{}`",
                s.trim()
            ),
        )
    })
}

pub(crate) fn push_branch_from_implementer_worktree(
    worktree: &std::path::Path,
    branch: &str,
) -> Result<(), auto_complete::PhaseFailure> {
    // trace:BUG-878 | ai:codex
    let refspec = format!("HEAD:refs/heads/{branch}");
    let out = std::process::Command::new("git")
        .current_dir(worktree)
        .args(["push", "-u", "origin", &refspec])
        .output()
        .map_err(|e| {
            auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::Spawn,
                format!("could not invoke `git push -u origin {refspec}`: {e}"),
            )
        })?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(auto_complete::PhaseFailure::new(format!(
        "could not push implementer branch `{branch}` from `{}` before phase-2 CI/teardown{}",
        worktree.display(),
        if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        }
    )))
}

pub(crate) fn ensure_implementer_branch_pushed(
    worktree: &std::path::Path,
    branch: &str,
    json: bool,
) -> Result<(), auto_complete::PhaseFailure> {
    // trace:BUG-878 | ai:codex
    let current = git_output_checked(worktree, &["branch", "--show-current"]).map_err(|e| {
        auto_complete::PhaseFailure::new(format!(
            "could not verify implementer branch before phase-2 CI/teardown: {e}"
        ))
    })?;
    if current != branch {
        return Err(auto_complete::PhaseFailure::new(format!(
            "implementer worktree `{}` is on `{}` but phase 2 is driving `{}` — refusing to push or tear down",
            worktree.display(),
            if current.is_empty() { "DETACHED" } else { &current },
            branch
        )));
    }

    match git_output_checked(
        worktree,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    ) {
        Ok(upstream) => {
            let ahead = git_output_checked(worktree, &["rev-list", "--count", &format!("{upstream}..HEAD")])
                .map_err(|e| {
                    auto_complete::PhaseFailure::new(format!(
                        "could not compare implementer branch `{branch}` to `{upstream}` before phase-2 CI/teardown: {e}"
                    ))
                })
                .and_then(|s| parse_git_count(&s, "ahead-of-upstream"))?;
            if ahead == 0 {
                return Ok(());
            }
            if !json {
                eprintln!(
                    "  {} implementer branch `{}` is {} commit(s) ahead of `{}` — pushing before CI/teardown",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    branch,
                    ahead,
                    upstream,
                );
            }
            push_branch_from_implementer_worktree(worktree, branch)
        }
        Err(_) => {
            if !json {
                eprintln!(
                    "  {} implementer branch `{}` has no upstream — pushing before CI/teardown",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    branch,
                );
            }
            push_branch_from_implementer_worktree(worktree, branch)
        }
    }
}

pub(crate) fn orchestrator_pr_title_and_body(commit_msg: &str) -> Result<(String, String)> {
    // trace:BUG-893 | ai:codex
    let title = pr_ship::derive_pr_title_from_commit(commit_msg);
    if title.is_empty() {
        anyhow::bail!("could not derive a non-empty PR title from the latest commit");
    }
    let commit_body = pr_ship::derive_pr_body_from_commit(commit_msg);
    let note = "Opened by the AIDA orchestrator because the implementer completed with committed work but no open PR.";
    let body = if commit_body.trim().is_empty() {
        note.to_string()
    } else {
        format!("{note}\n\n{commit_body}")
    };
    Ok((title, body))
}

// TASK-1443: adopt an already-open PR for `branch` instead of opening a
// second one. #2042 and #2043 shared a head and were created 106 seconds
// apart — two orchestrator drives raced the same branch and neither one
// saw the other's freshly-opened PR before calling `open_change` again.
// Reuses the same forge-neutral lookup `aida pr ship` uses to resume onto
// an existing PR (`branch_pr_resolution_from_lookup`, TASK-141/STORY-516)
// rather than re-deriving branch->PR lookup here. `Found` adopts (returns
// the existing PR id); `Create` and every inconclusive lookup state
// (`LookupFailed`) fall through to the caller's normal open-PR path — a
// lookup we cannot trust must not block recovery, it only means this
// race-guard cannot help for that call. trace:TASK-1443 | ai:claude
pub(crate) fn existing_open_pr_for_branch(
    project_root: &std::path::Path,
    branch: &str,
    forge_kind: crate::forge::ForgeKind,
) -> Option<u64> {
    // Route through the EXPLICIT `forge_kind` the caller already resolved for
    // this drive, not `change_lookup_for_branch`'s own auto-detection — a
    // fixture/test remote (or a repo mid-migration) can auto-resolve to
    // `PureGitForge`, whose `change_for_branch` deliberately reports the
    // branch itself as `Found(id: 0)` (no PR concept). That sentinel is not
    // an adoptable PR number, so it is filtered out here in addition to
    // matching the caller's real forge. trace:TASK-1443 | ai:claude
    let lookup = crate::forge::forge_for_kind(project_root, forge_kind)
        .change_for_branch(branch)
        .unwrap_or_else(|e| crate::forge::ChangeLookup::CliFailed(format!("{e:#}")));
    match crate::pr_ship::branch_pr_resolution_from_lookup(&lookup) {
        crate::pr_ship::BranchPrResolution::Found(id) if id != 0 => Some(id),
        _ => None,
    }
}

pub(crate) fn open_orchestrator_pr_for_implementer_worktree(
    project_root: &std::path::Path,
    worktree: &std::path::Path,
    branch: &str,
    forge_kind: crate::forge::ForgeKind,
    spec: &str,
) -> Result<u64> {
    // trace:BUG-893 | ai:codex
    push_branch_from_implementer_worktree(worktree, branch)
        .map_err(|e| anyhow::anyhow!("{}", e.reason))?;
    // TASK-1442 follow-up (containment for BUG-1510): before opening the PR,
    // verify the branch actually carries a commit trailered for the spec
    // this drive is for.
    ensure_pr_open_spec_attribution(worktree, branch, spec)?;
    // TASK-1443: a concurrent drive may have already opened a PR for this
    // head between the push above and this point — adopt it rather than
    // opening a second one. trace:TASK-1443 | ai:claude
    if let Some(existing) = existing_open_pr_for_branch(project_root, branch, forge_kind) {
        return Ok(existing);
    }
    let commit_msg_out = std::process::Command::new("git")
        .current_dir(worktree)
        .args(["log", "-1", "--format=%B"])
        .output()
        .context("could not invoke `git log` to derive orchestrator PR title/body")?;
    if !commit_msg_out.status.success() {
        anyhow::bail!(
            "`git log -1 --format=%B` failed: {}",
            String::from_utf8_lossy(&commit_msg_out.stderr).trim()
        );
    }
    let commit_msg = String::from_utf8_lossy(&commit_msg_out.stdout).to_string();
    let (title, body) = orchestrator_pr_title_and_body(&commit_msg)?;
    let change = crate::forge::forge_for_kind(project_root, forge_kind)
        .open_change(crate::forge::OpenChange {
            branch: branch.to_string(),
            base: crate::forge::default_branch_of(project_root),
            title,
            body,
            draft: false,
        })
        .context("could not open orchestrator-created pull request")?;
    if change.id == 0 {
        anyhow::bail!(
            "the orchestrator-created pull request opened but no number was found in its output: {}",
            change.url
        );
    }
    Ok(change.id)
}

pub(crate) fn origin_branch_ref(branch: &str) -> String {
    format!("origin/{branch}")
}

pub(crate) fn origin_default_ref_for_branch_recovery(
    project_root: &std::path::Path,
) -> Option<String> {
    resolve_default_branch_ref(project_root).map(|r| {
        if r.starts_with("origin/") {
            r
        } else {
            format!("origin/{r}")
        }
    })
}

pub(crate) fn pushed_branch_commits_ahead_default(
    project_root: &std::path::Path,
    branch: &str,
) -> Result<u32, auto_complete::PhaseFailure> {
    // trace:BUG-895 | ai:codex
    let branch_ref = origin_branch_ref(branch);
    git_output_checked(
        project_root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{branch_ref}^{{commit}}"),
        ],
    )
    .map_err(|e| {
        auto_complete::PhaseFailure::new(format!(
            "could not verify pushed branch `{branch_ref}` before phase-3 PR recovery: {e}"
        ))
    })?;
    let default_ref = origin_default_ref_for_branch_recovery(project_root).ok_or_else(|| {
        auto_complete::PhaseFailure::new(
            "could not resolve origin default branch before phase-3 PR recovery",
        )
    })?;
    git_output_checked(
        project_root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{default_ref}^{{commit}}"),
        ],
    )
    .map_err(|e| {
        auto_complete::PhaseFailure::new(format!(
            "could not verify origin default branch `{default_ref}` before phase-3 PR recovery: {e}"
        ))
    })?;
    let range = format!("{default_ref}..{branch_ref}");
    let ahead = git_output_checked(project_root, &["rev-list", "--count", &range]).map_err(|e| {
        auto_complete::PhaseFailure::new(format!(
            "could not compare pushed branch `{branch_ref}` to `{default_ref}` before phase-3 PR recovery: {e}"
        ))
    })?;
    parse_git_count(&ahead, "pushed-branch-ahead-default")
}

impl RealPhaseDriver {
    pub(crate) fn run_one_agent_gate(
        &mut self,
        pr: u32,
        gate: &AgentGateConfig,
    ) -> Result<auto_complete::ReviewerOutcome, auto_complete::PhaseFailure> {
        let verdict_dir = self.project_root.join(".aida").join("review-verdicts");
        std::fs::create_dir_all(&verdict_dir).map_err(|e| {
            auto_complete::PhaseFailure::new(format!(
                "could not create {}: {e}",
                verdict_dir.display()
            ))
        })?;
        let gate_token = sanitize_gate_file_token(&gate.name);
        let verdict_path = verdict_dir.join(format!("PR-{pr}-{gate_token}.json"));
        let _ = std::fs::remove_file(&verdict_path);

        let gate_started_at = std::time::SystemTime::now();
        let session_uuid = uuid::Uuid::now_v7().to_string();
        let headless_vendor = session::resolve_headless_vendor(&self.project_root);
        // BUG-1290: an agent gate always runs after `run_reviewer` already
        // announced the Reviewer phase — attach this gate's session, don't
        // re-announce the entry.
        self.mark_drain_phase_session(
            auto_complete::Phase::Reviewer,
            &session_uuid,
            headless_vendor,
            false,
        );
        let scope = format!("PR-{pr}");
        release_dead_phase_predecessor_leases(
            &self.project_root,
            &scope,
            auto_complete::Phase::Reviewer,
        )?;

        let mut cmd = std::process::Command::new(self.aida_exe());
        cmd.current_dir(&self.project_root)
            .args([
                "queue",
                "work",
                &scope,
                "--role",
                &gate.role,
                "--session-id",
                &session_uuid,
                "--no-pull",
            ])
            .env("AIDA_REVIEW_VERDICT_FILE", &verdict_path)
            .env("AIDA_AGENT_GATE_NAME", &gate.name)
            .env("AIDA_AGENT_GATE_ROLE", &gate.role)
            .env("AIDA_AGENT_GATE_APPLIES_TO", &gate.applies_to);
        for (key, value) in orchestrator_phase_child_env(
            &self.run_token,
            auto_complete::Phase::Reviewer,
            self.variant,
            &self.queue_user_id,
        ) {
            cmd.env(key, value);
        }
        cmd.env("AIDA_SESSION_ROLE", &gate.role);
        if let Some(mode) = self.no_human {
            cmd.env(orchestrator::NO_HUMAN_MODE_ENV, mode.slug());
        }
        if let Some(pm) = &self.permission_mode {
            cmd.args(["--permission-mode", pm]);
        }
        if self.no_human.is_some() {
            cmd.arg("--no-human");
        }
        if self.allow_stale_base {
            cmd.arg("--allow-stale-base");
        }

        let sentinel = exit_signal::sentinel_path(&self.sessions_dir(), &session_uuid);
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
                format!("could not launch agent gate `{}`: {e}", gate.name),
            )
        })?;
        if let exit_signal::ExitOutcome::WatchdogTripped(reason) = &outcome {
            // BUG-1299: the hint was resolved alongside `reason` in the same
            // `watchdog_trip_report` match arm — attach it verbatim rather
            // than let `recovery_hint` re-derive one from `FailureKind`
            // alone (which cannot tell NoProgress from Ceiling).
            // trace:BUG-1299 | ai:claude
            let mut failure = auto_complete::PhaseFailure::of(
                auto_complete::FailureKind::Watchdog,
                format!(
                    "agent gate `{}` watchdog stopped the session — {reason}",
                    gate.name
                ),
            );
            if let Some(hint) = watchdog.as_mut().and_then(|w| w.take_trip_hint()) {
                failure = failure.with_hint_override(hint);
            }
            return Err(failure);
        }
        if let exit_signal::ExitOutcome::Natural(status) = &outcome {
            if !status.success() {
                return Err(auto_complete::PhaseFailure::new(format!(
                    "agent gate `{}` session exited {}",
                    gate.name,
                    status
                        .code()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "with a signal".to_string())
                )));
            }
        } else if !self.json {
            eprintln!(
                "  {} agent gate `{}` signalled completion — session reaped",
                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                gate.name
            );
        }

        let gate_head_sha = pr_head_sha_best_effort(self, pr);
        // TASK-1460: a skill heredoc write bypassed the record path; archive
        // it, then let a fresh archived verdict AT the head answer when the
        // current file does not. trace:TASK-1460 | ai:claude
        let _ = review_verdict::adopt_direct_write(&verdict_path);
        let outcome = match read_verdict_file_for_head(&verdict_path, gate_head_sha.as_deref())
            .or_else(|e| {
                phase3_head_archive_fallback(
                    &verdict_path,
                    gate_head_sha.as_deref(),
                    gate_started_at,
                )
                .ok_or(e)
            }) {
            Ok(o) => o,
            Err(primary_failure) => {
                if verdict_path.is_file() {
                    return Err(primary_failure);
                }
                let fallback = spec_verdict_fallback_for_phase3(
                    &self.project_root,
                    &self.spec,
                    gate_started_at,
                    gate_head_sha.as_deref(),
                )?;
                let fallback = match fallback {
                    some @ Some(_) => some,
                    None => sibling_verdict_sweep_for_phase3(
                        &self.project_root,
                        pr,
                        &self.spec,
                        gate_started_at,
                        gate_head_sha.as_deref(),
                    )?,
                };
                if let Some(o) = fallback {
                    o
                } else if self.no_human.is_some() {
                    return Err(enrich_no_verdict_with_headless_diagnostic(
                        primary_failure,
                        &self.project_root,
                        gate_started_at,
                    ));
                } else {
                    return Err(primary_failure);
                }
            }
        };
        capture_review_calibration_for_spec(&self.project_root, &verdict_path, &self.spec);

        if let Ok((gate_lease, _, _)) = self.discover_orchestrated_lease(&session_uuid) {
            let _ = std::process::Command::new(self.aida_exe())
                .current_dir(&self.project_root)
                .args(["session", "end", &gate_lease, "--yes", "--skip-ci"])
                .status_retrying_etxtbsy();
        }

        Ok(outcome)
    }
}

pub(crate) fn head_commit_message(project_root: &std::path::Path, rev: &str) -> Result<String> {
    let commit_msg_out = std::process::Command::new("git")
        .current_dir(project_root)
        .args(["log", "-1", "--format=%B", rev])
        .output()
        .context("could not invoke `git log` to derive orchestrator PR title/body")?;
    if !commit_msg_out.status.success() {
        anyhow::bail!(
            "`git log -1 --format=%B {rev}` failed: {}",
            String::from_utf8_lossy(&commit_msg_out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&commit_msg_out.stdout).to_string())
}

pub(crate) fn open_orchestrator_pr_for_pushed_branch(
    project_root: &std::path::Path,
    branch: &str,
    forge_kind: crate::forge::ForgeKind,
    spec: &str,
) -> Result<u64> {
    // BUG-895: phase 2 may have already pushed and removed the implementer
    // worktree. Recover from origin/<branch> without trying to push again.
    // trace:BUG-895 | ai:codex
    let ahead = pushed_branch_commits_ahead_default(project_root, branch)
        .map_err(|e| anyhow::anyhow!("{}", e.reason))?;
    if ahead == 0 {
        anyhow::bail!("pushed branch `origin/{branch}` has no commits ahead of origin default");
    }
    let branch_ref = origin_branch_ref(branch);
    // TASK-1442 follow-up (containment for BUG-1510): before opening the PR,
    // verify the pushed branch actually carries a commit trailered for the
    // spec this drive is for.
    ensure_pr_open_spec_attribution(project_root, &branch_ref, spec)?;
    // TASK-1443: adopt an already-open PR for this head instead of opening a
    // second one (see `existing_open_pr_for_branch` above). trace:TASK-1443 | ai:claude
    if let Some(existing) = existing_open_pr_for_branch(project_root, branch, forge_kind) {
        return Ok(existing);
    }
    let commit_msg = head_commit_message(project_root, &branch_ref)?;
    let (title, body) = orchestrator_pr_title_and_body(&commit_msg)?;
    let change = crate::forge::forge_for_kind(project_root, forge_kind)
        .open_change(crate::forge::OpenChange {
            branch: branch.to_string(),
            base: crate::forge::default_branch_of(project_root),
            title,
            body,
            draft: false,
        })
        .context("could not open orchestrator-created pull request")?;
    if change.id == 0 {
        anyhow::bail!(
            "the orchestrator-created pull request opened but no number was found in its output: {}",
            change.url
        );
    }
    Ok(change.id)
}

pub(crate) fn try_open_orchestrator_pr_for_no_pr_worktree(
    project_root: &std::path::Path,
    worktree: &std::path::Path,
    branch: &str,
    forge_kind: crate::forge::ForgeKind,
    spec: &str,
) -> Option<(u32, u64)> {
    // trace:BUG-893 trace:BUG-1037 | ai:codex
    let ahead = branch_commits_ahead_main(worktree, branch).unwrap_or(0);
    if ahead == 0 {
        // BUG-1485: `ahead == 0` does NOT mean there is nothing to recover. It
        // is also what a MISSING WORKTREE produces: `git -C <gone>` fails, the
        // Option is None, and `unwrap_or(0)` flattens "could not look" into
        // "looked and found nothing". Phase 2 may have already pushed the
        // branch and torn the worktree down, in which case the work is safe on
        // `origin/<branch>` and perfectly recoverable — but phase 1 asked the
        // wrong repository and concluded there was nothing there, so recovery
        // fell through to the generic "run /aida-pr inside the session"
        // failure, against a session that no longer exists.
        //
        // Phase 3 already solved this for its own NoPr case (BUG-895). Reuse
        // that helper rather than re-deriving the logic: it verifies
        // `origin/<branch>` and the origin default ref, counts ahead between
        // them, and opens the PR from the pushed ref WITHOUT pushing again.
        // It returns None when the branch genuinely is not ahead, so the
        // no-work case still falls through exactly as before.
        // trace:BUG-1485 | ai:claude
        return try_open_orchestrator_pr_for_no_pr_pushed_branch(
            project_root,
            branch,
            forge_kind,
            spec,
        );
    }
    match open_orchestrator_pr_for_implementer_worktree(
        project_root,
        worktree,
        branch,
        forge_kind,
        spec,
    ) {
        Ok(pr) => Some((ahead, pr)),
        Err(e) => {
            eprintln!(
                "  {} could not auto-open a PR from the implementer worktree \
                 ({e:#}) — retrying from the pushed branch",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
            // The worktree exists and is ahead, so the failure was the push or
            // the forge call. If the branch had already reached origin, the
            // pushed-branch path can still succeed; if it never did, this
            // returns None and the caller falls back to punt/fail as before.
            // trace:BUG-1485 | ai:claude
            try_open_orchestrator_pr_for_no_pr_pushed_branch(project_root, branch, forge_kind, spec)
        }
    }
}

/// Preserve the watchdog diagnosis while adding the concrete review artifact
/// it left behind. The caller only uses this after PR recovery failed, so the
/// shelve record must tell triage that this is reviewable branch work rather
/// than an empty timed-out session.
// trace:BUG-1450 | ai:codex
pub(crate) fn watchdog_failure_with_committed_work(
    mut failure: auto_complete::PhaseFailure,
    worktree: &std::path::Path,
    branch: &str,
) -> auto_complete::PhaseFailure {
    let ahead = branch_commits_ahead_main(worktree, branch).unwrap_or(0);
    if ahead > 0 {
        failure.reason = format!(
            "{}; left {ahead} committed commit(s) on reviewable branch `{branch}` with no open PR",
            failure.reason
        );
        failure.hint_override = Some(format!(
            "Reviewable work survived on `{branch}` ({ahead} commit(s) ahead); open or recover its PR before re-driving the implementer"
        ));
    } else {
        failure.reason = format!("{}; left no committed work", failure.reason);
    }
    failure
}

pub(crate) fn try_open_orchestrator_pr_for_no_pr_pushed_branch(
    project_root: &std::path::Path,
    branch: &str,
    forge_kind: crate::forge::ForgeKind,
    spec: &str,
) -> Option<(u32, u64)> {
    // trace:BUG-895 trace:BUG-1037 | ai:codex
    let ahead = match pushed_branch_commits_ahead_default(project_root, branch) {
        Ok(ahead) if ahead > 0 => ahead,
        _ => return None,
    };
    match open_orchestrator_pr_for_pushed_branch(project_root, branch, forge_kind, spec) {
        Ok(pr) => Some((ahead, pr)),
        Err(e) => {
            eprintln!(
                "  {} could not auto-open a PR for pushed branch `{branch}` \
                 ({e:#}) — falling back to phase-3 NoPr",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
            None
        }
    }
}

/// Run STORY-1424's deterministic rung in an isolated checkout of the exact
/// reviewed head. `None` means the spec has no executable criteria and the
/// legacy reviewer path must remain byte-for-byte unchanged.
// trace:STORY-1424 trace:ADR-55 | ai:codex
pub(crate) fn prepare_graded_review(
    project_root: &std::path::Path,
    spec: &str,
    pr: u32,
    reviewed_sha: &str,
    branch: Option<&str>,
) -> Result<Option<graded_review::GradedReviewVerdict>, auto_complete::PhaseFailure> {
    let store = load_store_for_lookup(project_root).ok_or_else(|| {
        auto_complete::PhaseFailure::new("could not load spec store for graded review")
    })?;
    let req = store.get_requirement_by_spec_id(spec).ok_or_else(|| {
        auto_complete::PhaseFailure::new(format!("could not resolve {spec} for graded review"))
    })?;
    let criteria = graded_review::parse_acceptance_criteria(&req.description);
    if !criteria
        .iter()
        .any(|c| matches!(c, graded_review::CriterionKind::Executable { .. }))
    {
        return Ok(None);
    }
    // Trust snapshot, taken once per review before anything is spawned and
    // never re-read (no check-then-reread gap). Global config only; the repo
    // read below is display-only. trace:STORY-1476 | ai:claude
    let mut policy = acceptance_command_policy_global();
    policy.repo_optin_ignored = repo_review_optin_present(project_root);

    // The branch is a forge-reported PR head and the sha a forge-reported
    // commit; keep both from reading as git options. trace:BUG-1622 | ai:claude
    if !git_arg_guard::is_hex_sha(reviewed_sha.trim()) {
        return Err(auto_complete::PhaseFailure::new(format!(
            "reviewed head `{reviewed_sha}` is not a commit ID; refusing graded review"
        )));
    }
    if let Some(branch) = branch {
        if let Err(e) = git_arg_guard::reject_option_like("branch", branch) {
            return Err(auto_complete::PhaseFailure::new(e.to_string()));
        }
        let fetched = std::process::Command::new("git")
            .current_dir(project_root)
            .args(["fetch", git_arg_guard::END_OF_OPTIONS, "origin", branch])
            .output();
        if !fetched.as_ref().is_ok_and(|o| o.status.success()) {
            return Err(auto_complete::PhaseFailure::new(format!(
                "could not fetch branch {branch} before graded review"
            )));
        }
    }
    let checkout =
        std::env::temp_dir().join(format!("aida-graded-pr-{pr}-{}", uuid::Uuid::now_v7()));
    let added = std::process::Command::new("git")
        .current_dir(project_root)
        // trace:BUG-1622 | ai:claude
        .args(["worktree", "add", "--detach", "--"])
        .arg(&checkout)
        .arg(reviewed_sha.trim())
        .output()
        .map_err(|e| {
            auto_complete::PhaseFailure::new(format!(
                "could not create graded-review checkout: {e}"
            ))
        })?;
    if !added.status.success() {
        return Err(auto_complete::PhaseFailure::new(format!(
            "could not check out reviewed head {reviewed_sha}: {}",
            String::from_utf8_lossy(&added.stderr).trim()
        )));
    }

    let diff = std::process::Command::new("gh")
        .current_dir(project_root)
        .args(["pr", "diff", &pr.to_string()])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let evaluator: Option<Box<dyn evaluator::EvaluatorEngine>> =
        evaluator::JevEvaluator::from_env()
            .ok()
            .map(|v| Box::new(v) as Box<dyn evaluator::EvaluatorEngine>)
            .or_else(|| {
                std::env::var("AIDA_LOCAL_LLM_ENDPOINT").ok().map(|_| {
                    Box::new(evaluator::LocalLlmEvaluator::from_env())
                        as Box<dyn evaluator::EvaluatorEngine>
                })
            });
    let result = graded_review::execute_graded_review(
        spec,
        &req.title,
        &req.description,
        &diff,
        reviewed_sha,
        &checkout,
        evaluator.as_deref(),
        &policy,
    )
    .map_err(|e| auto_complete::PhaseFailure::new(format!("graded review failed closed: {e:#}")));
    let _ = std::process::Command::new("git")
        .current_dir(project_root)
        .args(["worktree", "remove", "--force"])
        .arg(&checkout)
        .status();
    let verdict = result?;
    let path = project_root
        .join(".aida")
        .join("review-verdicts")
        .join(format!("PR-{pr}-graded.json"));
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| {
        auto_complete::PhaseFailure::new(format!(
            "could not create graded review record directory: {e}"
        ))
    })?;
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&verdict).unwrap_or_default(),
    )
    .map_err(|e| {
        auto_complete::PhaseFailure::new(format!(
            "could not write graded review record {}: {e}",
            path.display()
        ))
    })?;
    Ok(Some(verdict))
}

/// TASK-1421: the publication boundary driven end to end through an injected
/// forge — no env var, no global, just the driver's `forge_factory` field.
// trace:TASK-1421 | ai:claude
#[cfg(test)]
mod forge_seam_tests {
    use super::*;
    use crate::forge::fake::RecordingForge;
    use crate::forge::{ChangeLookup, ChangeRef};

    fn driver_with(root: &std::path::Path, forge: &RecordingForge) -> RealPhaseDriver {
        let mut driver = RealPhaseDriver::new(
            root.to_path_buf(),
            "TASK-1421".into(),
            "test".into(),
            None,
            true,
            None,
            AutonomyMode::Default,
            "test-token".into(),
            false,
            false,
            false,
            false,
            auto_complete::LifecycleSkip::none(),
            auto_complete::AutoCompleteVariant::ThroughCi,
        );
        driver.forge_factory = Some(forge.factory());
        driver
    }

    fn change(id: u64, branch: &str) -> ChangeRef {
        ChangeRef {
            id,
            url: format!("https://example.invalid/pull/{id}"),
            branch: branch.into(),
            base: String::new(),
            title: Some("feat: work the guards refused".into()),
        }
    }

    #[test]
    fn refused_preflight_closes_the_open_change() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(42, "claude/task-1421"));
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1421", "guard `fmt` failed:\ndrift");

        let closed = forge.closed();
        assert_eq!(closed.len(), 1, "exactly one close_change call: {closed:?}");
        assert_eq!(closed[0].0, 42, "closed the PR the implementer opened");
        assert!(
            closed[0].1.contains("guard `fmt` failed"),
            "the retraction note carries the guard output: {}",
            closed[0].1
        );
    }

    #[test]
    fn refused_preflight_leaves_an_already_merged_change_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.merged_for_branch = ChangeLookup::Found(change(7, "claude/task-1421"));
        let mut driver = driver_with(tmp.path(), &forge);

        // The scenario is real only if the lookup resolves to AlreadyMerged
        // (no open change, the branch's change merged) through the fake.
        assert!(matches!(
            driver.detect_phase1_pr("claude/task-1421"),
            Phase1PrResolve::AlreadyMerged(ref pr) if pr.number == 7
        ));
        driver.retract_refused_publication("claude/task-1421", "guard `fmt` failed");

        assert!(
            forge.closed().is_empty(),
            "a merged change must never be closed: {:?}",
            forge.closed()
        );
    }

    #[test]
    fn refused_preflight_leaves_same_branch_pr_by_another_user_open() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(43, "claude/task-1421"));
        forge.author = Some("operator".into());
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1421", "guard `fmt` failed");

        assert!(
            forge.closed().is_empty(),
            "same-branch PR by a different forge user must remain untouched"
        );
    }

    /// BUG-1714 AC1: an inconclusive-only preflight outcome leaves the PR
    /// OPEN, stamps a merge-hold marker so it cannot merge unreviewed, and
    /// explains on the PR — without the word "refused" — that the guards
    /// could not complete and will be retried.
    // trace:BUG-1714 | ai:claude
    #[test]
    fn inconclusive_preflight_holds_the_open_change_instead_of_closing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(42, "claude/bug-1714"));
        let mut driver = driver_with(tmp.path(), &forge);

        let detail = implementer_preflight::inconclusive_detail(&[(
            "Run clippy".into(),
            "timed out after 900s; binary: /t/aida (build timed out after 900s)".into(),
        )]);
        driver.hold_inconclusive_publication("claude/bug-1714", &detail);

        assert!(
            forge.closed().is_empty(),
            "an inconclusive outcome must never close the PR: {:?}",
            forge.closed()
        );
        let record = crate::merge_hold::read_hold_record(tmp.path(), 42)
            .expect("the PR must be left unmergeable by a typed merge-hold marker");
        assert!(
            record.detail.contains("could not complete"),
            "the hold explains itself: {}",
            record.detail
        );
        let commented = forge.commented();
        assert_eq!(commented.len(), 1, "the hold is explained on the PR");
        assert_eq!(commented[0].0, 42);
        assert!(
            !commented[0].1.to_ascii_lowercase().contains("refus"),
            "nothing refused an unverified change: {}",
            commented[0].1
        );
        assert!(commented[0].1.contains("retried"), "{}", commented[0].1);
    }

    #[test]
    fn inconclusive_preflight_closes_when_both_hold_layers_fail() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(45, "claude/bug-1714"));
        let mut driver = driver_with(tmp.path(), &forge);

        // Occupy the hold directory path with a file, forcing the marker write
        // to fail. Label sync only fails when a forge is declared but the repo
        // cannot be pinned (a pure-git root short-circuits to Ok): a git repo
        // with a github provider and no origin remote is unpinnable.
        std::process::Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["init", "-q"])
            .output()
            .unwrap();
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        std::fs::write(
            tmp.path().join(".aida/config.toml"),
            "[forge]\nprovider = \"github\"\n",
        )
        .unwrap();
        std::fs::write(tmp.path().join(".aida/merge-holds"), "occupied").unwrap();
        let detail = "guard `Run clippy` could not complete: timed out";
        driver.hold_inconclusive_publication("claude/bug-1714", detail);

        let closed = forge.closed();
        assert_eq!(
            closed.len(),
            1,
            "double failure must retract the PR: {closed:?}"
        );
        assert_eq!(closed[0].0, 45);
        let notice = &closed[0].1;
        assert!(notice.contains("guards could not complete"), "{notice}");
        assert!(
            notice.contains("merge-hold could not be applied"),
            "{notice}"
        );
        assert!(notice.contains("Merge-hold marker error:"), "{notice}");
        assert!(notice.contains("Merge-hold label error:"), "{notice}");
        assert!(
            notice.contains("closed only because it could not be made unmergeable"),
            "{notice}"
        );
        assert!(notice.contains("branch is preserved"), "{notice}");
        assert!(notice.contains("guards will be retried"), "{notice}");
        // The close must never reuse the refused-path notice: the guards did
        // not refuse this change, they could not complete. (The embedded
        // label error may legitimately say "refusing to call `gh`".)
        assert!(!notice.contains("guards refused"), "{notice}");
        assert!(!notice.contains("Closed automatically:"), "{notice}");
    }

    #[test]
    fn inconclusive_preflight_warns_on_the_pr_when_it_can_neither_hold_nor_close() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(46, "claude/bug-1714"));
        // A foreign author forbids the close; with both hold layers down the
        // last lever is a warning comment on the PR itself.
        forge.author = Some("operator".into());
        let mut driver = driver_with(tmp.path(), &forge);

        std::process::Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["init", "-q"])
            .output()
            .unwrap();
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        std::fs::write(
            tmp.path().join(".aida/config.toml"),
            "[forge]\nprovider = \"github\"\n",
        )
        .unwrap();
        std::fs::write(tmp.path().join(".aida/merge-holds"), "occupied").unwrap();
        let detail = "guard `Run clippy` could not complete: timed out";
        driver.hold_inconclusive_publication("claude/bug-1714", detail);

        assert!(
            forge.closed().is_empty(),
            "a foreign PR must not be closed: {:?}",
            forge.closed()
        );
        let commented = forge.commented();
        assert_eq!(
            commented.len(),
            1,
            "an uncloseable unheld PR must carry a warning comment: {commented:?}"
        );
        assert_eq!(commented[0].0, 46);
        let warning = &commented[0].1;
        assert!(warning.contains("Merge-hold marker error:"), "{warning}");
        assert!(warning.contains("Merge-hold label error:"), "{warning}");
        assert!(warning.contains("neither held nor closed"), "{warning}");
        assert!(warning.contains("OPEN and UNVERIFIED"), "{warning}");
        assert!(warning.contains("guards will be retried"), "{warning}");
        // The PR stayed open, so the comment must not claim it was closed.
        assert!(!warning.contains("was closed"), "{warning}");
    }

    #[test]
    fn refused_preflight_fails_closed_when_pr_author_is_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(44, "claude/task-1421"));
        forge.author = None;
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1421", "guard `fmt` failed");

        assert!(
            forge.closed().is_empty(),
            "unknown ownership must fail closed"
        );
    }

    /// TASK-1552 finding 1: a refused change this identity did not open is
    /// left open — but it must be left UNMERGEABLE, not merely talked about
    /// on stderr. Under the branch-keyed ownership rule this is the common
    /// rework-round shape: round 1's PR is still open when round 2 refuses.
    // trace:TASK-1552 | ai:claude
    #[test]
    fn refused_preflight_holds_a_foreign_open_change_unmergeable() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(47, "claude/task-1552"));
        forge.author = Some("operator".into());
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1552", "guard `fmt` failed:\ndrift");

        assert!(
            forge.closed().is_empty(),
            "a foreign PR must not be closed: {:?}",
            forge.closed()
        );
        let record = crate::merge_hold::read_hold_record(tmp.path(), 47)
            .expect("the unclosable refused PR must carry a typed merge-hold marker");
        assert!(
            record.detail.contains("refused"),
            "the hold says a guard objected: {}",
            record.detail
        );
        assert!(
            record.detail.contains("guard `fmt` failed"),
            "the hold carries the guard output: {}",
            record.detail
        );
        let commented = forge.commented();
        assert_eq!(commented.len(), 1, "the hold is explained on the PR");
        assert_eq!(commented[0].0, 47);
        assert!(
            commented[0].1.contains("aida merge-hold clear"),
            "the PR comment names the release lever: {}",
            commented[0].1
        );
    }

    /// TASK-1552 finding 1, unverified arm: when the author cannot be
    /// confirmed the change may still be ours and still carries failing
    /// guards — it gets the same hold.
    // trace:TASK-1552 | ai:claude
    #[test]
    fn refused_preflight_holds_an_author_unknown_change_unmergeable() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(48, "claude/task-1552"));
        forge.author = None;
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1552", "guard `fmt` failed");

        assert!(
            forge.closed().is_empty(),
            "unknown ownership must fail closed"
        );
        assert!(
            crate::merge_hold::read_hold_record(tmp.path(), 48).is_some(),
            "an author-unknown refused PR must be held unmergeable"
        );
    }

    /// TASK-1552 finding 3: the unreadable-forge path. The ownership probe
    /// itself fails — the PR's state is unknowable, so the close is refused
    /// AND the hold is stamped (it only tightens).
    // trace:TASK-1552 | ai:claude
    #[test]
    fn refused_preflight_holds_when_the_ownership_probe_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(49, "claude/task-1552"));
        forge.metadata_error = Some("change_metadata unavailable".into());
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1552", "guard `fmt` failed");

        assert!(
            forge.closed().is_empty(),
            "an unverifiable PR must not be closed: {:?}",
            forge.closed()
        );
        assert!(
            crate::merge_hold::read_hold_record(tmp.path(), 49).is_some(),
            "an unverifiable refused PR must be held unmergeable"
        );
    }

    /// TASK-1552: a change that resolved but is no longer open needs no
    /// retraction and no hold — holding a closed PR would strand a stale
    /// marker on a number that may be reused by the forge UI as noise.
    // trace:TASK-1552 | ai:claude
    #[test]
    fn refused_preflight_leaves_a_no_longer_open_change_unheld() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(50, "claude/task-1552"));
        forge.metadata_state = crate::forge::ChangeState::Closed;
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1552", "guard `fmt` failed");

        assert!(
            forge.closed().is_empty(),
            "a no-longer-open change must be left alone: {:?}",
            forge.closed()
        );
        assert!(
            crate::merge_hold::read_hold_record(tmp.path(), 50).is_none(),
            "a no-longer-open change must not be held"
        );
        assert!(
            forge.commented().is_empty(),
            "nothing to explain on a change that is already resolved: {:?}",
            forge.commented()
        );
    }

    /// TASK-1552 finding 1: even OUR OWN refused PR whose close call fails
    /// stays open and guards-failing — it gets the hold too.
    // trace:TASK-1552 | ai:claude
    #[test]
    fn refused_preflight_holds_when_the_close_call_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_branch = ChangeLookup::Found(change(51, "claude/task-1552"));
        forge.close_error = Some("close rejected by the forge".into());
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1552", "guard `fmt` failed");

        assert_eq!(
            forge.closed().len(),
            1,
            "the close must have been attempted"
        );
        assert!(
            crate::merge_hold::read_hold_record(tmp.path(), 51).is_some(),
            "a refused PR whose close failed must be held unmergeable"
        );
    }

    /// TASK-1552 finding 3: a retraction whose PR was resolved through the
    /// spec-id search (the branch-keyed lookup missed) closes that PR like
    /// any other — the ownership probe and close run on the recovered number.
    // trace:TASK-1552 | ai:claude
    #[test]
    fn refused_preflight_closes_a_spec_search_resolved_change() {
        let tmp = tempfile::tempdir().unwrap();
        let mut forge = RecordingForge::new();
        forge.open_for_spec = ChangeLookup::Found(change(52, "claude/task-1421-swapped"));
        let mut driver = driver_with(tmp.path(), &forge);

        driver.retract_refused_publication("claude/task-1421", "guard `fmt` failed");

        let closed = forge.closed();
        assert_eq!(
            closed.len(),
            1,
            "the spec-search-recovered PR must be retracted: {closed:?}"
        );
        assert_eq!(closed[0].0, 52);
    }
}

// The BUG-1690 reused-draft-PR seam tests live in their own file so the
// `aida criteria` scanner can anchor their AC markers (the convention for
// every AC-traced driver test).
// trace:BUG-1690 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1690_undraft_reuse_tests.rs"]
mod bug_1690_undraft_reuse_tests;

/// BUG-1628: auto-complete phase 1 and ordinary pickup resolve the same
/// branch + worktree for a custom-prefix requirement. Temp repos only.
// trace:BUG-1628 | ai:claude
#[cfg(test)]
mod bug_1628_pickup_resolver_tests {
    use super::*;

    fn temp_project(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(name);
        std::fs::create_dir_all(&root).unwrap();
        let status = std::process::Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(&root)
            .status()
            .unwrap();
        assert!(status.success());
        (temp, root)
    }

    fn driver_for(root: &std::path::Path, spec: &str) -> RealPhaseDriver {
        RealPhaseDriver::new(
            root.to_path_buf(),
            spec.into(),
            "test".into(),
            None,
            true,
            None,
            AutonomyMode::Default,
            "test-token".into(),
            false,
            false,
            false,
            false,
            auto_complete::LifecycleSkip::none(),
            auto_complete::AutoCompleteVariant::ThroughCi,
        )
    }

    #[test]
    fn bug_1628_auto_complete_resolves_same_workspace_as_pickup_for_custom_prefix() {
        let (temp, root) = temp_project("qci");
        let driver = driver_for(&root, "SPEC-016");

        let (auto_path, auto_branch) = auto_complete::PhaseDriver::implementer_workspace(&driver)
            .expect("resolver succeeds")
            .expect("a workspace is resolved");
        // Ordinary pickup (`queue work` / `session_start`) resolver.
        let (pickup_path, pickup_branch) =
            resolve_pickup_workspace(&root, "SPEC-016", "auto").unwrap();

        assert_eq!(auto_path, pickup_path.display().to_string());
        assert_eq!(auto_branch, pickup_branch);
        // Project slug kept, ID separator kept, no epic `-work` branch.
        assert_eq!(pickup_path, temp.path().join("qci-spec-016"));
        assert_eq!(pickup_branch, "spec-016");
        assert!(!auto_path.contains("aida-spec016"), "{auto_path}");
        assert!(!auto_branch.ends_with("-work"), "{auto_branch}");
        assert!(crate::workflow_hints::branch_belongs_to_spec(
            &auto_branch,
            "SPEC-016"
        ));
    }

    #[test]
    fn bug_1628_pickup_resolver_skips_a_taken_branch_for_both_paths() {
        let (_temp, root) = temp_project("qci");
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        git(&["branch", "spec-016"]);

        let driver = driver_for(&root, "SPEC-016");
        let (_, auto_branch) = auto_complete::PhaseDriver::implementer_workspace(&driver)
            .unwrap()
            .unwrap();
        let (_, pickup_branch) = resolve_pickup_workspace(&root, "SPEC-016", "auto").unwrap();
        assert_eq!(auto_branch, pickup_branch);
        assert_eq!(pickup_branch, "spec-016-2");
        assert!(crate::workflow_hints::branch_belongs_to_spec(
            &auto_branch,
            "SPEC-016"
        ));
    }
}

// trace:BUG-1629 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1629_phase1_recovery_tests.rs"]
mod bug_1629_phase1_recovery_tests;

impl RealPhaseDriver {
    /// Finish adopting the PR found after an implementer resumes. Keep this
    /// transition independently testable: a resumed session may reuse a
    /// draft PR just like phase 1 does.
    // trace:BUG-1791 | ai:codex
    pub(crate) fn adopt_resumed_pr(&mut self, pr: OpenPrInfo) -> auto_complete::ImplementerOutcome {
        self.undraft_reused_publication(&pr);
        self.set_pr_number(pr.number as u32);
        if let Some(head) = pr_head_branch(&self.project_root, pr.number) {
            self.branch = Some(head);
        }
        auto_complete::ImplementerOutcome::PrOpened
    }
}

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

/// Keep the production retry cadence patient while letting the drain-level
/// `finish_ci` regression exercise all retries without making the unit suite
/// sleep for thirty seconds. The retry count and control flow are identical.
// trace:BUG-1265 | ai:codex
pub(crate) fn ci_red_refine_poll_interval() -> std::time::Duration {
    if cfg!(test) {
        std::time::Duration::from_millis(1)
    } else {
        std::time::Duration::from_secs(15)
    }
}

/// TASK-975: how much of the failing-check log tail rides in the CI-fix
/// prompt. The failure detail lives at the end of a CI log; the cap keeps the
/// prompt bounded on a pathologically chatty run.
// trace:TASK-975 | ai:claude
pub(crate) const CI_FIX_LOG_TAIL_BYTES: usize = 16_000;

/// TASK-975: fetch the failing-step logs for a CI run (`gh run view <id>
/// --log-failed`), tail-bounded. `None` when `gh` is missing, the run cannot
/// be read, or the log is empty — the prompt then tells the fix session to
/// fetch the checks itself.
// trace:TASK-975 | ai:claude
pub(crate) fn failing_ci_log_excerpt(
    project_root: &std::path::Path,
    run_id: &str,
) -> Option<String> {
    let gh = resolve_gh_binary()?;
    let out = std::process::Command::new(gh)
        .current_dir(project_root)
        .args(["run", "view", run_id, "--log-failed"])
        .output_retrying_etxtbsy()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(tail_bounded(trimmed, CI_FIX_LOG_TAIL_BYTES))
}

/// TASK-975: keep the LAST `max_bytes` of `text` (on a char boundary) with a
/// truncation marker — a CI log's failure detail is at its tail.
// trace:TASK-975 | ai:claude
pub(crate) fn tail_bounded(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…(log truncated)…\n{}", &text[start..])
}

/// TASK-975: the prompt handed to the headless CI-fix session. Pure so the
/// contract lines (smallest fix, push to the existing branch, never a new PR,
/// exit without pushing when unfixable) are pinned by a unit test.
// trace:TASK-975 | ai:claude
pub(crate) fn build_ci_fix_prompt(
    spec: &str,
    branch: &str,
    failure_reason: &str,
    log_excerpt: &str,
) -> String {
    let log_block = if log_excerpt.is_empty() {
        "No failing-check log could be fetched — run `gh pr checks` / \
         `gh run view --log-failed` yourself to see the failure."
            .to_string()
    } else {
        format!("Failing-check log (tail):\n```\n{log_excerpt}\n```")
    };
    format!(
        "You are a CI-fix session inside an autonomous drain. The PR branch \
         `{branch}` implementing {spec} has failing CI checks.\n\
         \n\
         Reported failure: {failure_reason}\n\
         \n\
         {log_block}\n\
         \n\
         Your job, in this worktree (the branch is already checked out):\n\
         1. Read the spec for context: `aida show {spec} --full`.\n\
         2. Diagnose the failing checks from the log above and make the \
         smallest correct fix. Do not rewrite unrelated code.\n\
         3. Run the relevant local checks (build / fmt / clippy / tests) \
         until they pass.\n\
         4. Commit with a message crediting {spec} and push to the existing \
         branch: `git push origin {branch}`.\n\
         \n\
         Hard rules: do NOT open a new PR, do NOT merge, do NOT force-push, \
         do NOT switch branches. If the failure is not fixable from this \
         worktree (broken infrastructure, a flaky external service), exit \
         WITHOUT committing or pushing anything."
    )
}

/// TASK-975: the current head SHA of `branch` on origin, or `None` when the
/// remote cannot be read. Ground truth for "did the fix session push?".
// trace:TASK-975 | ai:claude
pub(crate) fn remote_head_sha(project_root: &std::path::Path, branch: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["ls-remote", "origin", &format!("refs/heads/{branch}")])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// TASK-84: pure resolver for `aida queue work` permission-mode.
/// Order (first non-None wins):
///   1. `flag`   — `--permission-mode` on the command line
///   2. `env`    — `AIDA_PERMISSION_MODE` env var
///   3. `config` — `.aida/config.toml [behavior] permission_mode`
///   4. `is_aida_worktree=true` default → `bypassPermissions` (worktree is
///      git-sandboxed; prompt flood ate autonomous runs — see TASK-84
///      description)
///   5. fallback → `acceptEdits` (safe default for non-AIDA cwd)
///      Returns the mode + a short origin string for the pre-flight summary.
///      Pure decision helper — kept separate so unit tests can pin the
///      resolution order without env/config side effects. trace:TASK-84 | ai:claude
///      STORY-495: resolve the effective `--permission-mode` for an interactive
///      `aida queue work` launch. `None` → honor Claude's native posture (no flag
///      injected — the faithful default). Precedence, highest first:
///   1. `--permission-mode` flag (explicit override)
///   2. `AIDA_PERMISSION_MODE` env (process-level opt-in)
///   3. `.aida/config.toml [behavior] permission_mode` (project policy)
///   4. `[agents] bypass` knob → `bypassPermissions` (uniform fleet opt-in)
///   5. otherwise → `None` (native)
///
/// Pre-STORY-495 this defaulted to `bypassPermissions` inside an AIDA worktree
/// and `acceptEdits` elsewhere; both auto-injections are gone — the uniform
/// knob is the single place that restores bypass posture. trace:STORY-495
pub(crate) fn resolve_queue_work_permission_mode(
    flag: Option<&str>,
    env: Option<&str>,
    config: Option<&str>,
    bypass_knob: bool,
    plan_only: bool,
) -> (Option<String>, &'static str) {
    if let Some(m) = flag.filter(|s| !s.is_empty()) {
        return (Some(m.to_string()), "--permission-mode flag");
    }
    // STORY-265: a plan-only session is read-only by default — it writes a
    // docs/plans/ file, not code. An explicit --permission-mode flag (above)
    // still wins; otherwise `plan` overrides env/config/bypass so the planning
    // session can't edit code even on a bypass-by-default project.
    if plan_only {
        return (Some("plan".to_string()), "--plan-only (read-only default)");
    }
    if let Some(m) = env.filter(|s| !s.is_empty()) {
        return (Some(m.to_string()), "AIDA_PERMISSION_MODE env");
    }
    if let Some(m) = config.filter(|s| !s.is_empty()) {
        return (Some(m.to_string()), ".aida/config.toml");
    }
    if bypass_knob {
        return (
            Some("bypassPermissions".to_string()),
            "[agents] bypass knob",
        );
    }
    (None, "native (faithful default)")
}

/// STORY-287: the three-mode autonomy ladder for `aida queue work`. Aligns
/// the human's role with the implementer's pause behavior:
///   - `Default`  — human is driving; every prompt pauses.
///   - `Zen`      — advisor on standby; mechanical `kind:confirmation`
///     prompts auto-resolve, `kind:design-fork` prompts pause.
///   - `NoHuman`  — nobody reachable; the headless drain (STORY-263).
///     trace:STORY-287 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutonomyMode {
    Default,
    Zen,
    NoHuman,
}

impl AutonomyMode {
    /// ADR-7/ADR-10: the SINGLE in-process resolution of an in-flight
    /// `--auto-complete` run's autonomy mode. Unifies both autonomy axes into
    /// one typed value — the typed `--no-human` mode (which gates PARENT-engine
    /// behavior: headless implementer spawn, CI-watch) plus `--zen` (a
    /// CHILD-session signal: skill templates auto-resolve `kind:confirmation`
    /// prompts when `AIDA_ZEN` is set).
    ///
    /// `--zen` is recovered from the zen-INTENT TOKEN (`AIDA_ZEN_TOKEN`), not a
    /// bare `AIDA_ZEN` read: the token is minted only by the `--zen` dispatch
    /// arm and scrubbed by the `Default` / `--no-human` arms, so a leaked
    /// `AIDA_ZEN=1` is NOT mistaken for a zen run (BUG-237). The `AIDA_ZEN` env
    /// var remains the cross-process TRANSPORT to spawned phase children / skill
    /// templates; this typed value is the in-process SOURCE OF TRUTH, so the
    /// engine path no longer re-derives zen-ness from scattered bare env reads.
    /// ADR-10 carries this resolved value onto the phase driver as the
    /// `autonomy_mode` field (symmetric with `no_human`); in-process zen
    /// branches read `RealPhaseDriver::is_zen_run()`, not a bare env re-read.
    // trace:ADR-7 trace:ADR-10 | ai:claude
    pub(crate) fn for_auto_complete_run(no_human: Option<auto_complete::NoHumanMode>) -> Self {
        Self::resolve_run(no_human, std::env::var(zen::ZEN_TOKEN_ENV).is_ok())
    }

    /// The pure core of [`for_auto_complete_run`]: resolve the autonomy mode
    /// from the typed `--no-human` mode and whether the zen-intent token is
    /// present. Precedence `--no-human` > `--zen` > default matches
    /// [`resolve_autonomy_mode`]. Pure, so the precedence + the BUG-237
    /// leak-resistance are unit-testable without mutating process env.
    // trace:ADR-7 trace:ADR-10 | ai:claude
    pub(crate) fn resolve_run(
        no_human: Option<auto_complete::NoHumanMode>,
        zen_token_present: bool,
    ) -> Self {
        match (no_human.is_some(), zen_token_present) {
            (true, _) => AutonomyMode::NoHuman,
            (false, true) => AutonomyMode::Zen,
            (false, false) => AutonomyMode::Default,
        }
    }

    /// True when this run is a supervised `--zen` drive.
    // trace:ADR-10
    pub(crate) fn is_zen(self) -> bool {
        matches!(self, AutonomyMode::Zen)
    }
}

/// Resolve the effective autonomy mode from the `--zen` flag and the
/// presence of `--no-human`. Precedence is `--no-human` > `--zen` >
/// default — when both `--zen` and `--no-human` are set, `--no-human`
/// wins because it is the strictly stronger mode (it removes the human
/// entirely). Pure, so the precedence rule is unit-testable without
/// spawning a session. trace:STORY-287 | ai:claude
pub(crate) fn resolve_autonomy_mode(zen_flag: bool, no_human: bool) -> AutonomyMode {
    match (no_human, zen_flag) {
        (true, _) => AutonomyMode::NoHuman,
        (false, true) => AutonomyMode::Zen,
        (false, false) => AutonomyMode::Default,
    }
}

/// Resolved flag values after expanding the `aida queue work --drain` alias
/// (TASK-578). `--drain` is pure discoverability sugar: it sets the same
/// internal state that `--auto-complete --no-human=both --max <queue-size>`
/// already produced. Explicit flags always win, so `--drain
/// --no-human=reviewer-only` keeps `reviewer-only`,
/// `--drain --auto-complete=through-ci` keeps the early stop, and an explicit
/// `--max N` caps the drain instead of using the queue size.
/// trace:TASK-578 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DrainResolution {
    pub(crate) auto_complete: Option<String>,
    pub(crate) no_human: Option<String>,
    pub(crate) max: Option<usize>,
}

/// Expand the `--drain` alias into the underlying auto-complete / no-human / max
/// state. When `drain` is false this is an identity map over the caller's flags.
/// When `drain` is true the unset fields take the drain defaults — full
/// auto-complete, fully-headless (`both`), and a `--max` of the drivable queue
/// size (falling back to 99 when the size is unknown / zero, per the spec). An
/// explicitly-supplied flag is never overwritten. `queue_size` is the number of
/// drivable queued items the caller has already resolved (0 ⇒ unknown). The
/// function is pure so the expansion can be unit-tested without a live queue.
/// trace:TASK-578 | ai:claude
pub(crate) fn resolve_drain_alias(
    drain: bool,
    auto_complete: Option<&str>,
    no_human: Option<&str>,
    max: Option<usize>,
    queue_size: usize,
) -> DrainResolution {
    if !drain {
        return DrainResolution {
            auto_complete: auto_complete.map(str::to_string),
            no_human: no_human.map(str::to_string),
            max,
        };
    }
    DrainResolution {
        auto_complete: Some(auto_complete.unwrap_or("full").to_string()),
        no_human: Some(no_human.unwrap_or("both").to_string()),
        // Explicit --max wins; otherwise bound the drain to the queue size, or
        // fall back to 99 when the size is unknown (the spec's "--max 99").
        max: Some(max.unwrap_or(if queue_size > 0 { queue_size } else { 99 })),
    }
}

/// TASK-84: read `[behavior] permission_mode = "..."` from
/// `.aida/config.toml`. Returns `None` when the file is missing, the
/// section is absent, or the value is empty. We use a hand-rolled
/// section-aware scan (rather than pulling in serde for one optional
/// string) to stay consistent with `read_id_format_settings`.
/// trace:TASK-84 | ai:claude
pub(crate) fn read_behavior_permission_mode(project_dir: &std::path::Path) -> Option<String> {
    let config_path = project_dir.join(".aida").join("config.toml");
    let content = std::fs::read_to_string(&config_path).ok()?;
    let mut in_behavior = false;
    for raw in content.lines() {
        // Strip trailing inline comments before any other parsing so a
        // legitimate `permission_mode = "auto"  # default` doesn't end up
        // with the comment glued onto the value. trace:TASK-84 | ai:claude
        let line = strip_toml_inline_comment(raw).trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            in_behavior = rest.trim_end_matches(']').trim() == "behavior";
            continue;
        }
        if !in_behavior {
            continue;
        }
        if let Some(rest) = line.strip_prefix("permission_mode") {
            if let Some(val) = rest.split('=').nth(1) {
                let v = val.trim().trim_matches('"').trim_matches('\'').to_string();
                if v.is_empty() {
                    return None;
                }
                return Some(v);
            }
        }
    }
    None
}

/// TASK-81: search a lease list for any active lease whose scope matches
/// the requested scope (case-insensitive). Returns the freshest match by
/// `started_at` so the caller has a stable target for `session_end` even
/// if multiple stale leases somehow share a scope. Pure decision helper —
/// kept separate so the unit test in `queue_work_tests` doesn't need to
/// touch disk. trace:TASK-81 | ai:claude
pub(crate) fn find_scope_lease_conflict(
    leases: &[SessionLease],
    scope: &str,
) -> Option<SessionLease> {
    leases
        .iter()
        .filter(|l| l.scope.eq_ignore_ascii_case(scope))
        .max_by_key(|l| l.started_at)
        .cloned()
}

/// BUG-307: gather the three independent liveness signals for `lease` and
/// hand them to [`orchestrator::classify_for_auto_release`]. The classifier
/// is pure; this wrapper is where we touch the process table, lease-file
/// mtime, and `git status --porcelain`. Returns
/// [`orchestrator::AutoReleaseDecision::Live`] when the feature is disabled
/// so callers can branch uniformly. trace:BUG-307 | ai:claude
pub(crate) fn auto_release_decision_for_lease(
    project_root: &std::path::Path,
    lease: &SessionLease,
    config: &orchestrator::OrchestratorConfig,
) -> orchestrator::AutoReleaseDecision {
    // BUG-511: worktree-less review-verb leases are advisory PID locks — no
    // worktree, no uncommitted work to lose — so liveness is the creator
    // process, full stop. Decided BEFORE the config gate and the mtime clock:
    // a dead advisory review lease is always safe to release, a live one
    // always refuses. BUG-882 reviewer worktree leases carry review_verb as
    // metadata but still use the normal worktree cleanup gates below.
    if lease.review_verb && lease.worktree_path.as_os_str().is_empty() {
        let pid_alive = lease
            .creator_pid
            .map(process_probe::pid_is_alive)
            .unwrap_or(false);
        return if pid_alive {
            orchestrator::AutoReleaseDecision::Live
        } else {
            let mtime_age_secs = std::fs::metadata(lease_path(project_root, &lease.id))
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            orchestrator::AutoReleaseDecision::SafelyDormant {
                process_dead: true,
                mtime_age_secs,
                worktree_missing: false,
            }
        };
    }

    if !config.auto_release_dormant_leases {
        return orchestrator::AutoReleaseDecision::Live;
    }

    // Signal 1: creator shell still alive? A lease without a recorded
    // `creator_pid` (pre-STORY-73) defaults to "dead" — those leases also
    // pre-date the orchestrator drain and are the canonical leaked-lease
    // case, so the auto-release path catches them.
    // BUG-1111: process-backed headless leases use `active_pid` as the
    // authoritative child liveness signal; `creator_pid` may be the still-live
    // orchestrator that is trying to reclaim the dead child.
    // trace:BUG-1111 | ai:codex
    let pid_alive = lease
        .active_pid
        .or(lease.creator_pid)
        .map(process_probe::pid_is_alive)
        .unwrap_or(false);

    // Signal 2: lease-file mtime age. We use the filesystem mtime rather
    // than `started_at` because future writers (mark_lease_escalated_to_human
    // etc.) bump the mtime — the file's freshness is the authoritative
    // "this lease was touched recently" signal.
    let lease_file = lease_path(project_root, &lease.id);
    let mtime_age_secs = std::fs::metadata(&lease_file)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs() as i64)
        // No mtime readable → treat as old (the lease was just listed from
        // disk, so this only fires under genuine fs weirdness; failing
        // "stale" lets the cleanup proceed).
        .unwrap_or(i64::MAX);

    // Signal 3: live `claude` running inside the worktree. Reuses the same
    // probe `aida session leases --all` uses for the ● live glyph so the
    // classifier agrees with what the operator sees in `session leases`.
    let live_claude_in_worktree = process_probe::probe_live_claude_sessions().iter().any(|s| {
        !s.stale_cwd && (s.cwd == lease.worktree_path || s.cwd.starts_with(&lease.worktree_path))
    });

    let worktree_exists = lease.worktree_path.exists();
    let worktree_dirty_count = if worktree_exists {
        worktree_dirty_entries(&lease.worktree_path).len()
    } else {
        0
    };

    orchestrator::classify_for_auto_release(
        pid_alive,
        mtime_age_secs,
        live_claude_in_worktree,
        worktree_exists,
        worktree_dirty_count,
        config.stale_lease_threshold_minutes,
    )
}

/// BUG-777: the recovery picture for one same-scope lease conflict — the
/// verdict plus the evidence a refusal message needs to be *evaluable*
/// ("work there is lost unless committed" is useless if the operator has to
/// go run `git status` in that worktree to find out whether it applies).
// trace:BUG-777 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct StaleLeaseRecoveryReport {
    pub(crate) verdict: StaleLeaseRecovery,
    /// `git status --porcelain` lines in the lease's worktree — empty when the
    /// tree is clean, missing, or the lease is worktree-less (advisory lock).
    pub(crate) dirty: Vec<String>,
    pub(crate) worktree_exists: bool,
}

impl StaleLeaseRecoveryReport {
    /// One human clause describing what is (or is not) at risk in the
    /// conflicting lease's worktree — the fix for "work there is lost unless
    /// committed", which the operator previously could not assess.
    // trace:BUG-777 | ai:claude
    pub(crate) fn worktree_state_phrase(&self) -> String {
        if !self.worktree_exists {
            return "worktree GONE (nothing to lose)".to_string();
        }
        if self.dirty.is_empty() {
            return "worktree CLEAN (no uncommitted changes)".to_string();
        }
        format!(
            "worktree DIRTY ({} uncommitted change(s))",
            self.dirty.len()
        )
    }

    /// A short, bounded rendering of the uncommitted entries so a refusal can
    /// name exactly what is unsaved rather than just counting it.
    // trace:BUG-777 | ai:claude
    pub(crate) fn dirty_sample(&self) -> String {
        const MAX: usize = 5;
        let shown: Vec<&str> = self.dirty.iter().take(MAX).map(|s| s.trim()).collect();
        if self.dirty.len() > MAX {
            format!("{}, … (+{} more)", shown.join("; "), self.dirty.len() - MAX)
        } else {
            shown.join("; ")
        }
    }
}

/// BUG-777: gather the recovery picture for a blocking lease.
///
/// Liveness comes from [`lease_state_for`] — the SAME classifier `aida ps` and
/// `aida status <spec>` render as `● live` / `⚠ STALE` — plus the lease's own
/// recorded pids, so this surface can never disagree with what the operator
/// just saw in `aida ps`. The dirty/clean reading is the same
/// `git status --porcelain` probe the BUG-307 auto-release gate uses.
// trace:BUG-777 | ai:claude
pub(crate) fn stale_lease_recovery_for_lease(lease: &SessionLease) -> StaleLeaseRecoveryReport {
    let live = process_probe::probe_live_claude_sessions();
    let lease_state = lease_state_for(lease, &live, chrono::Utc::now());
    // BUG-1111: `lease_owner_process_gone` treats an `active_pid` vendor child
    // as authoritative over the creator/orchestrator pid.
    // trace:BUG-1111 | ai:codex
    let owner_gone = lease_owner_process_gone(
        lease.active_pid,
        lease.active_pid_start_time.as_deref(),
        lease.creator_pid,
        lease.creator_pid_start_time.as_deref(),
        process_probe::process_identity_is_alive,
    );
    // A worktree-less advisory lease (review / claim verb, or the TASK-474
    // empty-path MCP claim) has no tree to inspect — treat it as "missing",
    // which is exactly right: there is no uncommitted work to strand.
    let worktree_exists =
        !lease.worktree_path.as_os_str().is_empty() && lease.worktree_path.exists();
    let dirty = if worktree_exists {
        worktree_dirty_entries(&lease.worktree_path)
    } else {
        Vec::new()
    };
    let verdict =
        classify_stale_lease_recovery(lease_state, owner_gone, worktree_exists, dirty.len());
    StaleLeaseRecoveryReport {
        verdict,
        dirty,
        worktree_exists,
    }
}

/// BUG-906: before a transient phase retry relaunches a child session, reap any
/// predecessor lease on the same scope whose owner process is verifiably gone.
/// Clean/missing worktrees are safe to release; dirty worktrees become a typed,
/// shelvable `cache-locked` failure so the drain parks with the exact worktree
/// that needs human salvage instead of burning the retry on its own leftover
/// lease.
// trace:BUG-906 | ai:codex
pub(crate) fn release_dead_phase_predecessor_leases(
    project_root: &std::path::Path,
    scope: &str,
    phase: auto_complete::Phase,
) -> Result<(), auto_complete::PhaseFailure> {
    let mut remaining = 16usize;
    while remaining > 0 {
        let leases = list_leases(project_root);
        let Some(conflict) = find_scope_lease_conflict(&leases, scope) else {
            return Ok(());
        };
        let report = stale_lease_recovery_for_lease(&conflict);
        match report.verdict {
            StaleLeaseRecovery::ReclaimableClean { .. } => {
                if !force_cleanup_lease(project_root, &conflict) {
                    return Err(auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::CacheLocked,
                        format!(
                            "phase {} retry found dead predecessor lease {} on `{scope}`, \
                             but auto-release did not finish; worktree: {}",
                            phase.index(),
                            &conflict.id[..conflict.id.len().min(8)],
                            conflict.worktree_path.display(),
                        ),
                    ));
                }
                eprintln!(
                    "  {} released dead predecessor lease {} on `{scope}` before retrying phase {}",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    (&conflict.id[..conflict.id.len().min(8)]).yellow(),
                    phase.index()
                );
                remaining -= 1;
            }
            StaleLeaseRecovery::StaleDirty { dirty_entries } => {
                return Err(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::CacheLocked,
                    format!(
                        "phase {} retry found dead predecessor lease {} on `{scope}`, \
                         but its worktree is dirty ({dirty_entries} uncommitted change(s)); \
                         worktree: {}; changes: {}",
                        phase.index(),
                        &conflict.id[..conflict.id.len().min(8)],
                        conflict.worktree_path.display(),
                        report.dirty_sample(),
                    ),
                ));
            }
            StaleLeaseRecovery::Live | StaleLeaseRecovery::UnknownLiveness => return Ok(()),
        }
    }
    Err(auto_complete::PhaseFailure::of(
        auto_complete::FailureKind::CacheLocked,
        format!(
            "phase {} retry found too many predecessor leases on `{scope}`; \
             release stale leases manually with `aida session leases` / `aida session end`",
            phase.index()
        ),
    ))
}

pub(crate) fn remove_session_lease_record_only(
    project_root: &std::path::Path,
    lease_id: &str,
) -> bool {
    let lease_file = lease_path(project_root, lease_id);
    let removed = match std::fs::remove_file(&lease_file) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    };
    let manifest = session_manifest::manifest_path(project_root, lease_id);
    if manifest.exists() {
        let _ = std::fs::remove_file(manifest);
    }
    let activity = session_activity_path(project_root, lease_id);
    if activity.exists() {
        let _ = std::fs::remove_file(activity);
    }
    removed
}

pub(crate) const IMPLEMENTER_RETRY_PREDECESSOR_REPROBES: usize = 3;
pub(crate) const IMPLEMENTER_RETRY_PREDECESSOR_REPROBE_DELAY: std::time::Duration =
    std::time::Duration::from_secs(5);

/// BUG-908: a phase-1 transient retry is continuation, not takeover. Release
/// the dead predecessor lease even when its worktree is dirty, then relaunch
/// the implementer against that same branch/path with force-claim. The dirty
/// tree is the attempt-1 work product; `--steal` is never involved.
// trace:BUG-908 BUG-1078 | ai:codex
pub(crate) fn reclaim_implementer_retry_predecessor(
    project_root: &std::path::Path,
    scope: &str,
) -> Result<Option<(String, std::path::PathBuf, String)>, auto_complete::PhaseFailure> {
    reclaim_implementer_retry_predecessor_with(
        project_root,
        scope,
        IMPLEMENTER_RETRY_PREDECESSOR_REPROBES,
        IMPLEMENTER_RETRY_PREDECESSOR_REPROBE_DELAY,
        |lease| stale_lease_recovery_for_lease(lease),
        std::thread::sleep,
    )
}

// trace:BUG-1078 | ai:codex
pub(crate) fn reclaim_implementer_retry_predecessor_with(
    project_root: &std::path::Path,
    scope: &str,
    reprobes: usize,
    reprobe_delay: std::time::Duration,
    mut recovery_for_lease: impl FnMut(&SessionLease) -> StaleLeaseRecoveryReport,
    mut sleep: impl FnMut(std::time::Duration),
) -> Result<Option<(String, std::path::PathBuf, String)>, auto_complete::PhaseFailure> {
    let mut live_or_unknown_seen = 0usize;
    loop {
        let leases = list_leases(project_root);
        let Some(conflict) = find_scope_lease_conflict(&leases, scope) else {
            return Ok(None);
        };
        let report = recovery_for_lease(&conflict);
        match report.verdict {
            StaleLeaseRecovery::ReclaimableClean { .. } | StaleLeaseRecovery::StaleDirty { .. } => {
                if !remove_session_lease_record_only(project_root, &conflict.id) {
                    return Err(auto_complete::PhaseFailure::of(
                        auto_complete::FailureKind::LeaseConflict,
                        format!(
                            "phase 1 retry could not release predecessor lease {} on `{scope}`; \
                             worktree: {}",
                            &conflict.id[..conflict.id.len().min(8)],
                            conflict.worktree_path.display(),
                        ),
                    ));
                }
                return Ok(Some((
                    conflict.branch.clone(),
                    conflict.worktree_path.clone(),
                    conflict.id.clone(),
                )));
            }
            StaleLeaseRecovery::Live | StaleLeaseRecovery::UnknownLiveness
                if live_or_unknown_seen < reprobes =>
            {
                live_or_unknown_seen += 1;
                sleep(reprobe_delay);
            }
            StaleLeaseRecovery::Live | StaleLeaseRecovery::UnknownLiveness => {
                return Err(auto_complete::PhaseFailure::of(
                    auto_complete::FailureKind::LeaseConflict,
                    format!(
                        "phase 1 retry cannot reclaim predecessor lease {} on `{scope}` after \
                         {} liveness re-probe(s); worktree: {}",
                        &conflict.id[..conflict.id.len().min(8)],
                        reprobes,
                        conflict.worktree_path.display(),
                    ),
                ));
            }
        }
    }
}

/// BUG-438: on `--resume-drain`, proactively release the crashed orchestrator's
/// lease(s) on `scope` whose creator process is dead — *independent of the
/// staleness clock*. The time-based auto-release ([`auto_release_decision_for_lease`])
/// keeps a dead-PID lease "Live" while its mtime is still fresh, so a **fast**
/// resume (within `stale_lease_threshold_minutes`) collides with the crashed
/// implementer's lease when the reviewer phase resolves PR→spec. We have already
/// passed the PID-liveness double-drive gate, so the original orchestrator is
/// dead — its child leases are too. We force `threshold = 0` so a dead-PID
/// *clean-worktree* lease releases regardless of mtime, while a **dirty**
/// worktree still classifies `DormantDirty` and is left untouched (no
/// uncommitted work is lost — the operator handles it). trace:BUG-438 | ai:claude
pub(crate) fn release_dead_leases_for_resume(project_root: &std::path::Path, scope: &str) {
    let mut cfg = orchestrator::OrchestratorConfig::load(project_root);
    // Resume is a deliberate recovery — release the crashed lease even if the
    // project disabled the dormant-lease auto-release, and ignore the mtime
    // clock (PID-death is the authoritative signal here).
    cfg.auto_release_dormant_leases = true;
    cfg.stale_lease_threshold_minutes = 0;
    let mut remaining = 16usize; // defense-in-depth bound, mirrors session_start
    while remaining > 0 {
        let leases = list_leases(project_root);
        let Some(conflict) = find_scope_lease_conflict(&leases, scope) else {
            break;
        };
        match auto_release_decision_for_lease(project_root, &conflict, &cfg) {
            orchestrator::AutoReleaseDecision::SafelyDormant { .. } => {
                eprintln!(
                    "  {} released the crashed drain's lease {} on `{}` (process dead)",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    (&conflict.id[..conflict.id.len().min(8)]).yellow(),
                    scope
                );
                let _ = force_cleanup_lease(project_root, &conflict);
                remaining -= 1;
            }
            // Live (process somehow still alive — won't happen post-gate) or
            // DormantDirty (uncommitted work) → leave it; the reviewer phase's
            // own conflict path / the operator handles those cases.
            _ => break,
        }
    }
}

/// BUG-307: format a seconds-old duration into the short human form used in
/// the auto-release log line (`"2h"`, `"45m"`, `"30s"`). Distinct from
/// [`humanize_relative`] — that one takes a chrono datetime and appends
/// " ago"; here the caller controls the surrounding text.
pub(crate) fn humanize_secs_short(secs: i64) -> String {
    if secs < 0 {
        return "0s".to_string();
    }
    let secs = secs as u64;
    if secs < 60 {
        return format!("{}s", secs);
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m", mins);
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{}h", hours);
    }
    let days = hours / 24;
    format!("{}d", days)
}

/// STORY-42: shell out to `aida db sync --pull`. We could refactor the
/// inline implementation in main(), but it's deeply tangled with the
/// dispatch and backend init — a child-process invocation is cheap
/// (sub-second when there's nothing to fetch) and lets queue work
/// reuse the canonical sync flow verbatim (conflict detection, cache
/// rebuild, the works). trace:STORY-42 | ai:claude
pub(crate) fn run_aida_db_sync_pull(store_path: &std::path::Path) -> Result<()> {
    eprintln!(
        "  {} pulling orphan-store via `aida db sync --pull` ...",
        "↻".cyan()
    );
    let mut cmd = std::process::Command::new(aida_exe_path());
    cmd.args(["db", "sync", "--pull"]);
    // Force the child to look at the same store as us — avoids ambiguity
    // when queue work runs from a sibling worktree.
    let _ = store_path; // signature future-proofing; current binary
                        // resolves the store via cwd's project root,
                        // which matches ours.
    let status = cmd
        .status_retrying_etxtbsy()
        .with_context(|| "spawn `aida db sync --pull`")?;
    if !status.success() {
        anyhow::bail!("`aida db sync --pull` exited with {}", status);
    }
    Ok(())
}

/// STORY-42: write the session manifest from a queue-work plan so
/// /aida-pickup can walk the cluster top-down on launch. Items carry
/// the spec_id + status_at_plan that the resolver already computed,
/// so this function does not re-load the store.
/// trace:STORY-42 STORY-98 | ai:claude
pub(crate) fn write_queue_work_manifest(
    project_root: &std::path::Path,
    lease: &SessionLease,
    plan: &QueueWorkPlan,
    plan_context: Option<session_manifest::PlanContext>,
    claude_session_id: Option<String>,
    batch_name: Option<&str>,
) -> Result<()> {
    use crate::session_manifest::{ManifestItem, SessionManifest};

    let items: Vec<ManifestItem> = plan
        .entries
        .iter()
        .enumerate()
        .map(|(idx, e)| ManifestItem {
            spec_id: e.spec_id.clone(),
            position: (idx + 1) as u32,
            status_at_plan: e.status_at_plan.clone(),
            started_at: None,
            completed_at: None,
            note: None,
        })
        .collect();
    let manifest = SessionManifest {
        session_id: lease.id.clone(),
        planned_at: chrono::Utc::now(),
        plan_source: "queue work".to_string(),
        claude_session_id,
        // TASK-272: record the batch when set up via `aida queue work
        // --batch NAME` so /aida-pickup detects batch context.
        batch_name: batch_name.map(str::to_string),
        plan: plan_context,
        items,
    };
    let path = session_manifest::manifest_path(project_root, &lease.id);
    session_manifest::save(&path, &manifest)?;
    Ok(())
}

/// TASK-112: how `aida queue work` should launch claude — a cold launch
/// with a freshly-minted session id, or a resume of a recorded one.
/// trace:TASK-112 | ai:claude
#[derive(Debug)]
pub(crate) enum QueueWorkLaunch {
    /// Cold launch; the `String` is a freshly-minted UUID passed as
    /// `claude --session-id` so the new conversation is itself resumable.
    Fresh(String),
    /// Resume a recorded conversation by its claude session id.
    Resume(String),
}

impl QueueWorkLaunch {
    pub(crate) fn session_id(&self) -> &str {
        match self {
            QueueWorkLaunch::Fresh(id) | QueueWorkLaunch::Resume(id) => id,
        }
    }
}

/// TASK-402: does `s` look like an AIDA *lease* id rather than a Claude
/// *session* UUID? A lease id is a hyphenless hex run (e.g. `019e45cfc559`,
/// the short form printed by `session … started` check line); a Claude session UUID
/// always carries hyphens (`019e45cf-acea-73c1-9476-…`). The orchestrator's
/// phase-1 banner prominently shows the lease id, so pasting *that* into
/// `--resume <id>` is the #1 recovery papercut — it never prefix-matches a
/// recorded session. Detect the shape so we can point at the right id form
/// instead of the generic "no recorded session" miss. Pure + heuristic.
/// trace:TASK-402 | ai:claude
pub(crate) fn looks_like_lease_id(s: &str) -> bool {
    !s.is_empty()
        && !s.contains('-')
        && (8..=16).contains(&s.len())
        && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// TASK-112: resolve a (possibly truncated) session id against the
/// recorded sessions for a scope. Returns the full id on a unique match;
/// a non-matching value of UUID-ish length is passed through verbatim
/// (the user pasted a full id we didn't index). trace:TASK-112 | ai:claude
pub(crate) fn resolve_resume_id(recorded: &[String], requested: &str) -> Result<String> {
    let matches: Vec<&String> = recorded
        .iter()
        .filter(|id| id.starts_with(requested))
        .collect();
    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 if requested.len() >= 16 => Ok(requested.to_string()),
        // TASK-402 (friction #1): the pasted value has the shape of an AIDA
        // lease id, not a Claude session UUID. The banner that advertised
        // `session <lease> started` check line is the trap — `--resume` wants the
        // Claude session UUID. Name the mismatch and point at the right id.
        // trace:TASK-402 | ai:claude
        0 if looks_like_lease_id(requested) => anyhow::bail!(
            "`{}` looks like an AIDA lease id, but --resume needs the Claude \
             session UUID (e.g. 019e45cf-acea-73c1-…) — run \
             `aida queue work <scope> --list-sessions` to see the resumable \
             session ids (or pass bare `--resume` to take the most recent)",
            requested
        ),
        0 => anyhow::bail!(
            "no recorded claude session matches `{}` — run \
             `aida queue work <scope> --list-sessions` to see recorded ids",
            requested
        ),
        n => anyhow::bail!(
            "{} recorded sessions match `{}` — use a longer prefix",
            n,
            requested
        ),
    }
}

/// TASK-112: decide fresh-vs-resume for `aida queue work`. Called before
/// the worktree is created so a bad `--resume` fails clean.
///   * `--session-id <uuid>` → cold launch with that caller-minted id
///     (STORY-132 — the TUI tracks the conversation deterministically)
///   * `--fresh`           → cold launch, new minted id
///   * `--resume <id>`     → resume that id (prefix-matched)
///   * bare `--resume`     → resume the most recent recorded session
///   * neither, prior exists, interactive → prompt
///   * neither, otherwise  → cold launch
///     trace:TASK-112, STORY-132 | ai:claude
pub(crate) fn resolve_queue_work_launch(
    scope: &str,
    resume: Option<&str>,
    fresh: bool,
    session_id_override: Option<&str>,
) -> Result<QueueWorkLaunch> {
    use std::io::IsTerminal;

    let mint_fresh = || QueueWorkLaunch::Fresh(uuid::Uuid::now_v7().to_string());

    // STORY-132: a caller-minted session id is a fresh cold launch with
    // that exact UUID — validated up front so a malformed id fails clean
    // before any worktree side effects. Clap already makes it mutually
    // exclusive with `--resume`.
    if let Some(sid) = session_id_override {
        uuid::Uuid::parse_str(sid)
            .with_context(|| format!("--session-id `{}` is not a valid UUID", sid))?;
        return Ok(QueueWorkLaunch::Fresh(sid.to_string()));
    }

    if fresh {
        return Ok(mint_fresh());
    }

    let prior = session::list_scope_sessions(scope).unwrap_or_default();

    match resume {
        // Explicit `--resume <id>`.
        Some(id) if !id.is_empty() => {
            let recorded: Vec<String> = prior.iter().map(|m| m.id.clone()).collect();
            Ok(QueueWorkLaunch::Resume(resolve_resume_id(&recorded, id)?))
        }
        // Bare `--resume` — most recent recorded session for the scope.
        Some(_) => {
            let m = prior.first().ok_or_else(|| {
                anyhow::anyhow!(
                    "no recorded claude session for scope `{}` to resume — \
                     drop --resume (or pass --fresh) to cold-launch",
                    scope
                )
            })?;
            Ok(QueueWorkLaunch::Resume(m.id.clone()))
        }
        // Default: prompt when prior sessions exist and we're
        // interactive; cold-launch otherwise (non-interactive /
        // autonomous runs default to fresh — no blocking prompt).
        None => {
            if prior.is_empty() || !std::io::stdin().is_terminal() {
                return Ok(mint_fresh());
            }
            let fresh_label = "○  start a fresh session".to_string();
            let mut labels: Vec<String> = vec![fresh_label.clone()];
            for m in prior.iter().take(8) {
                labels.push(session::format_session_line(m));
            }
            let pick = inquire::Select::new(
                &format!(
                    "{} prior session(s) for {} — resume one?",
                    prior.len(),
                    scope
                ),
                labels.clone(),
            )
            .with_help_message("Use arrow keys to move, Enter to choose")
            .prompt()
            .context("session-resume picker cancelled")?;
            if pick == fresh_label {
                Ok(mint_fresh())
            } else {
                let idx = labels
                    .iter()
                    .position(|l| l == &pick)
                    .map(|p| p.saturating_sub(1))
                    .unwrap_or(0);
                Ok(QueueWorkLaunch::Resume(prior[idx].id.clone()))
            }
        }
    }
}

/// TASK-112: `aida queue work <scope> --list-sessions` — print recorded
/// conversations for the scope, newest first.
// trace:TASK-112
pub(crate) fn print_scope_sessions(scope: &str) -> Result<()> {
    let sessions = session::list_scope_sessions(scope)?;
    if sessions.is_empty() {
        println!(
            "{}",
            format!("(no recorded sessions for scope `{}`)", scope).dimmed()
        );
        println!(
            "  {}",
            "a session is recorded the first time `aida queue work` launches an agent for this scope"
                .dimmed()
        );
        return Ok(());
    }
    println!(
        "{} session(s) for {} (most recent first):",
        sessions.len(),
        scope.cyan()
    );
    for m in &sessions {
        println!("  {}", session::format_session_line(m));
    }
    println!();
    // TASK-402 (friction #1 + #5): make the resume hint paste-ready. Show the
    // *full* Claude session UUID (the id `--resume` actually needs — not the
    // AIDA lease id the kickoff banner shows, and not the 8-char prefix in the
    // table above) and, when the session's recorded worktree differs from the
    // current cwd, prepend the `cd <worktree>` step so a headless session
    // launched in a sibling worktree is resumable in one paste.
    // trace:TASK-402 | ai:claude
    let most_recent = &sessions[0];
    let resume_cmd = paste_ready_resume_command(scope, most_recent);
    println!("  {}", "resume the most recent:".dimmed());
    println!("    {}", resume_cmd.cyan());
    println!();
    println!(
        "  {}",
        format!(
            "(or `aida queue work {} --resume <full-session-uuid>` for a specific one above)",
            scope
        )
        .dimmed()
    );
    Ok(())
}

/// TASK-402: assemble a single paste-ready resume command for a recorded
/// session. Uses the FULL session UUID (`--resume` rejects the AIDA lease id
/// and a truncated prefix can be ambiguous) and, when the session's recorded
/// worktree is known and is not the current cwd, prepends `cd <worktree> && `
/// so a headless session launched in a sibling worktree (Claude's `--resume`
/// is project-slug scoped — invisible from the main repo's cwd) resumes in one
/// paste. Pure given the cwd argument resolved by the caller.
/// trace:TASK-402 | ai:claude
pub(crate) fn paste_ready_resume_command(scope: &str, m: &session::SessionMeta) -> String {
    let base = format!("aida queue work {} --resume {}", scope, m.id);
    let current = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    resume_command_with_cwd(&base, m.last_cwd.as_deref(), current.as_deref())
}

/// TASK-402: pure helper — prepend a `cd <worktree> && ` step when the
/// session's recorded worktree is known and differs from the current cwd.
/// Keeps the cwd-resolution side effect out so it's unit-testable.
/// trace:TASK-402 | ai:claude
pub(crate) fn resume_command_with_cwd(
    base: &str,
    worktree: Option<&str>,
    current: Option<&str>,
) -> String {
    match worktree {
        // The recorded cwd is session metadata and the line is meant to be
        // pasted into a shell: quote it. trace:BUG-1624 | ai:claude
        Some(wt) if !wt.is_empty() && Some(wt) != current => {
            format!("cd {} && {}", shell_quote(wt), base)
        }
        _ => base.to_string(),
    }
}

#[cfg(test)]
#[path = "tests/queue_work_resume_tests.rs"]
mod queue_work_resume_tests;

#[cfg(test)]
#[path = "tests/drain_reliability_wiring_tests.rs"]
mod drain_reliability_wiring_tests;

pub(crate) fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

#[cfg(test)]
#[path = "tests/story_255_discipline_pack_tests.rs"]
mod story_255_discipline_pack_tests;

#[cfg(test)]
#[path = "tests/story_781_project_manifest_tests.rs"]
mod story_781_project_manifest_tests;

/// STORY-127: unit tests for the Scope-B runtime anti-pattern detector
/// predicates. Each predicate is pure over its inputs so the warning
/// conditions are exercised without git/forge/queue I/O.
/// trace:STORY-127 | ai:claude
#[cfg(test)]
#[path = "tests/story_127_antipattern_detectors.rs"]
mod story_127_antipattern_detectors;

// trace:TASK-0414 — opt-in statusline bootstrap.
#[cfg(test)]
#[path = "tests/task_0414_statusline_setup.rs"]
mod task_0414_statusline_setup;

/// trace:STORY-582 | ai:claude — durable processing-record audit trail.
#[cfg(test)]
#[path = "tests/story_582_processing_record_tests.rs"]
mod story_582_processing_record_tests;

// BUG-618: lock the `aida queue list --json` shape — the TUI cockpit panel
// parses {spec_id,title,status,for_role}, so the cache-summary resolution must
// keep emitting exactly those keys in queue order. trace:BUG-618 | ai:claude
#[cfg(test)]
#[path = "tests/queue_json_rows_tests.rs"]
mod queue_json_rows_tests;

#[cfg(test)]
#[path = "tests/phase_heartbeat_tests.rs"]
mod phase_heartbeat_tests;

#[cfg(test)]
#[path = "tests/merge_contiguous_blocks_tests.rs"]
mod merge_contiguous_blocks_tests;

// trace:STORY-716 | ai:claude
#[cfg(test)]
#[path = "tests/worktree_handler_tests.rs"]
mod worktree_handler_tests;

// trace:STORY-725 | ai:claude — the THOUGHT → spec front door + zen help cleanup.
#[cfg(test)]
#[path = "tests/zen_front_door_tests.rs"]
mod zen_front_door_tests;

// TASK-1063: the three dead-queue-pruning verbs (`queue prune --orphaned`,
// `queue prune --merged`, `queue gc`) must cross-reference one another in
// their --help so an operator can pick the right one without reading all
// three pages — and `queue prune` with no predicate must name every verb.
// trace:TASK-1063 | ai:claude
#[cfg(test)]
#[path = "tests/task_1063_prune_verb_crossref_tests.rs"]
mod task_1063_prune_verb_crossref_tests;

// TASK-1056: the batched git-fanout collapse for `aida status --full`. These
// tests pin the substitution to OUTPUT EQUIVALENCE — each batched helper must
// return exactly the data the per-branch git loop it replaced produced, so the
// rendered status is byte-identical while the process count drops from O(local
// branches) to one `git for-each-ref`. trace:TASK-1056 | ai:claude
#[cfg(test)]
#[path = "tests/task_1056_batched_git_fanout_tests.rs"]
mod task_1056_batched_git_fanout_tests;

// STORY-698: verification-step capture at `aida queue done`. The precedence
// decision is factored into `decide_test_plan_capture` precisely so it can be
// exercised without a TTY. trace:STORY-698 | ai:claude
#[cfg(test)]
#[path = "tests/story_698_test_plan_capture_tests.rs"]
mod story_698_test_plan_capture_tests;

// BUG-800: review worksheets must name runnable focused-test commands instead
// of bare test identifiers, so Codex does not pass the name to the wrong
// binary. trace:BUG-800 | ai:codex
#[cfg(test)]
#[path = "tests/bug_800_review_test_command_prompt_tests.rs"]
mod bug_800_review_test_command_prompt_tests;

// STORY-1350: round-one review compares the recorded implementation intent
// with the resulting diff, using the pickup heading as a stable marker.
#[cfg(test)]
#[path = "tests/story_1350_review_approach_tests.rs"]
mod story_1350_review_approach_tests;

// BUG-1434: the review prompt states how far a PR's head trails the
// default branch, computed locally, never rendering "unknown" as 0.
#[cfg(test)]
#[path = "tests/bug_1434_stale_base_tests.rs"]
mod bug_1434_stale_base_tests;

// STORY-790 review findings: drift brief then-vs-now + mail/briefs since exit.
// trace:STORY-790 | ai:claude
#[cfg(test)]
#[path = "tests/story_790_drift_brief_tests.rs"]
mod story_790_drift_brief_tests;

// STORY-780: init-from-nothing bootstrap — the ordered sequence, its
// refusals, idempotent resume, and the forge repo-create argv.
// trace:STORY-780 | ai:claude
#[cfg(test)]
#[path = "tests/story_780_init_bootstrap_tests.rs"]
mod story_780_init_bootstrap_tests;

// trace:STORY-827 | ai:codex
#[cfg(test)]
#[path = "tests/story_827_init_readme_registry_tests.rs"]
mod story_827_init_readme_registry_tests;

// BUG-777: recovering a lease left behind by an already-exited session — the
// verifiably-gone + clean-worktree reclaim, the never-reap-a-live-lease floor,
// and the clean/dirty reporting that makes the refusal evaluable.
// trace:BUG-777 | ai:claude
#[cfg(test)]
#[path = "tests/bug_777_stale_lease_recovery_tests.rs"]
mod bug_777_stale_lease_recovery_tests;

// TASK-1175: the burndown ready set orders on QUEUE-INSERTION time. These tests
// cover the impure halves — the per-user queue-file `added_at` join and the
// env → project → global → default resolution ladder for `[burndown] order`.
// trace:TASK-1175 | ai:claude
#[cfg(test)]
#[path = "tests/task_1175_queue_insertion_order_tests.rs"]
mod task_1175_queue_insertion_order_tests;

// trace:TASK-1265 | ai:codex
#[cfg(test)]
#[path = "tests/task_1265_rework_no_op_tests.rs"]
mod task_1265_rework_no_op_tests;

// BUG-1295: the orchestrator's phase-3 auto-rebase failure classification
// must ride on the `aida pr rebase` subprocess's EXIT CODE, never on
// substring-matching its message text — the drift guard.
// trace:BUG-1295 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1295_rebase_exit_code_tests.rs"]
mod bug_1295_rebase_exit_code_tests;

// trace:ADR-55 | ai:antigravity
#[cfg(test)]
#[path = "tests/adr_55_evaluator_tests.rs"]
mod adr_55_evaluator_tests;

// trace:STORY-1426 | ai:antigravity
#[cfg(test)]
#[path = "tests/story_1426_contradictions_tests.rs"]
mod story_1426_contradictions_tests;

// trace:STORY-1424 | ai:antigravity
#[cfg(test)]
#[path = "tests/story_1424_graded_review_tests.rs"]
mod story_1424_graded_review_tests;

// trace:BUG-1668 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1668_trivial_criteria_tests.rs"]
mod bug_1668_trivial_criteria_tests;

// trace:BUG-1418 | ai:codex
#[cfg(test)]
#[path = "tests/bug_1418_drain_token_measurement_tests.rs"]
mod bug_1418_drain_token_measurement_tests;

// trace:BUG-1510 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1510_lease_brief_dispatch_tests.rs"]
mod bug_1510_lease_brief_dispatch_tests;
// trace:TASK-1442 | ai:claude
#[cfg(test)]
#[path = "tests/task_1442_pr_open_spec_guard_tests.rs"]
mod task_1442_pr_open_spec_guard_tests;

// trace:TASK-1444 | ai:claude
#[cfg(test)]
#[path = "tests/task_1444_shelve_attribution_tests.rs"]
mod task_1444_shelve_attribution_tests;

// trace:TASK-1445 | ai:claude
#[cfg(test)]
#[path = "tests/task_1445_pr_attribution_disagreement_tests.rs"]
mod task_1445_pr_attribution_disagreement_tests;

// trace:BUG-1486 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1486_stakeholder_db_verb_tests.rs"]
mod bug_1486_stakeholder_db_verb_tests;

// trace:EPIC-72 trace:TASK-1435 trace:TASK-1436 trace:TASK-1438 trace:TASK-1439 | ai:antigravity
#[cfg(test)]
#[path = "tests/epic_72_exposition_tests.rs"]
mod epic_72_exposition_tests;

// Git option injection through user-supplied refs, ranges and branches.
// trace:BUG-1622 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1622_git_option_injection_tests.rs"]
mod bug_1622_git_option_injection_tests;

// trace:BUG-1624 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1624_shell_and_git_guard_tests.rs"]
mod bug_1624_shell_and_git_guard_tests;

// trace:BUG-1627 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1627_session_env_hardening_tests.rs"]
mod bug_1627_session_env_hardening_tests;

// trace:BUG-1650 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1650_store_resolver_tests.rs"]
mod bug_1650_store_resolver_tests;

// Mutating callers re-validate against the current store, not a stale cache.
// trace:BUG-1670 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1670_stale_cache_callers_tests.rs"]
mod bug_1670_stale_cache_callers_tests;
#[cfg(test)]
#[path = "tests/bug_1799_carry_forward_tests.rs"]
mod bug_1799_carry_forward_tests;

pub mod worktree_dismiss;

#[cfg(test)]
#[path = "tests/bug_1800_worktree_undecidable_cache_tests.rs"]
mod bug_1800_worktree_undecidable_cache_tests;

// trace:BUG-1807 | ai:codex
#[cfg(test)]
mod bug_1807_handoff_tests {
    use super::*;

    #[test]
    fn only_confirmed_queue_ownership_allows_completion() {
        assert!(require_review_handoff(&AutoQueueOutcome::filed("queued MR-35")).is_ok());
        assert!(require_review_handoff(&AutoQueueOutcome::already_exists("queued MR-35")).is_ok());
        for outcome in [
            AutoQueueOutcome::skipped_needs_attention("MR-35 queue insertion failed"),
            AutoQueueOutcome::skipped_by_design("no open MR"),
        ] {
            assert!(require_review_handoff(&outcome).is_err());
        }
    }

    #[test]
    fn reviewer_pickup_commands_parse_for_both_forges() {
        for label in ["MR-35", "PR-35"] {
            assert!(
                Cli::try_parse_from(["aida", "queue", "work", label, "--role", "reviewer"]).is_ok()
            );
        }
    }

    #[test]
    fn configured_missing_canonical_store_never_uses_legacy_inventory() {
        let root = tempfile::tempdir().unwrap();
        Storage::new(root.path().join("requirements.yaml"))
            .save(&RequirementsStore::default())
            .unwrap();
        std::fs::create_dir(root.path().join(".aida")).unwrap();
        for config in [
            "mode = \"distributed\"\nstore_path = \".aida-store\"\n",
            "store_path = \".aida-store\"\n",
        ] {
            std::fs::write(root.path().join(".aida/config.toml"), config).unwrap();
            assert!(load_review_story_inventory(root.path()).is_err());
            assert!(!root.path().join(".aida-store").exists());
        }
        // A partial store must not become an empty inventory or get repaired
        // as a side effect of this read gate.
        std::fs::create_dir(root.path().join(".aida-store")).unwrap();
        assert!(load_review_story_inventory(root.path()).is_err());
        assert!(!root.path().join(".aida-store/objects").exists());
    }

    #[test]
    fn existing_mr35_story_is_recognized_without_github_collision() {
        let mut store = RequirementsStore::default();
        for (id, title) in [
            ("STORY-1", "Review PR-35: unrelated"),
            ("STORY-2", "Review MR-35: fixture"),
        ] {
            let mut req = aida_core::Requirement::new(title.into(), String::new());
            req.spec_id = Some(id.into());
            req.status = RequirementStatus::Approved;
            store.requirements.push(req);
        }
        // Legacy lookup must read this project's existing story, too.
        let root = tempfile::tempdir().unwrap();
        Storage::new(root.path().join("requirements.yaml"))
            .save(&store)
            .unwrap();
        let store = load_review_story_inventory(root.path()).expect("legacy store");
        assert_eq!(
            canonical_review_story(&store, ReviewForge::GitLab, 35, None, None)
                .unwrap()
                .display_id(),
            "STORY-2"
        );
        assert_eq!(
            canonical_review_story(&store, ReviewForge::GitHub, 35, None, None)
                .unwrap()
                .display_id(),
            "STORY-1"
        );
        assert!(canonical_review_story(&store, ReviewForge::GitLab, 3, None, None).is_none());
    }
}
