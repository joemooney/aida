//! drain human-gate refusals, from-pr entry, plan prelude, and auto-complete run (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// TASK-306: the decision the `--no-human` kickoff gate makes — split from
/// its terminal/stdin I/O so it is unit-testable. `acknowledged` is whether
/// `AIDA_NO_HUMAN_ACKNOWLEDGED=1` is set. STORY-276 dropped `BothUnavailable`
/// — the headless implementer now ships, so `both` is a real, acknowledgeable
/// mode like `reviewer-only`; the gate keys purely off acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoHumanGate {
    /// Already acknowledged via the env var — proceed with a one-line scope
    /// reminder, no prompt.
    Acknowledged,
    /// Not yet acknowledged — show the scope banner and ask.
    NeedsAck,
}

/// TASK-306 / STORY-276: pure classification for [`no_human_kickoff_gate`].
/// The decision no longer depends on the mode (both modes ship) — only on
/// whether the scope was acknowledged. The *banner wording* still varies by
/// mode; that lives in [`no_human_kickoff_gate`].
pub(crate) fn classify_no_human_gate(acknowledged: bool) -> NoHumanGate {
    if acknowledged {
        NoHumanGate::Acknowledged
    } else {
        NoHumanGate::NeedsAck
    }
}

/// STORY-276: the one-line scope description for a `--no-human` mode — used in
/// the acknowledged-path reminder and as the lead line of the ack banner.
pub(crate) fn no_human_scope_line(mode: auto_complete::NoHumanMode) -> &'static str {
    if mode.wants_headless_implementer() {
        "--no-human=both: the implementer AND reviewer phases run headless — \
         no human at the keyboard. On a design-fork it cannot safely resolve, \
         the implementer punts the spec to Needs Attention instead of guessing."
    } else {
        "--no-human: the reviewer phase runs headless; the implementer phase \
         stays interactive and will pause for you."
    }
}

/// BUG-740: fail fast when an auto-complete drive would launch an interactive
/// implementer from a non-TTY context. The failure used to happen much later,
/// after queue/store/lease/worktree setup, when the child vendor CLI finally
/// reported "stdin is not a terminal". `--no-human=both` is the explicit
/// headless implementer mode; `AIDA_HEADLESS=1` is accepted as the environment
/// opt-in used by agent shells.
// trace:BUG-740 | ai:codex
pub(crate) fn non_tty_interactive_implementer_preflight(
    no_human: Option<auto_complete::NoHumanMode>,
    stdin_is_tty: bool,
    stdout_is_tty: bool,
    aida_headless: bool,
) -> Result<Option<auto_complete::NoHumanMode>> {
    let implementer_is_headless = no_human
        .map(auto_complete::NoHumanMode::wants_headless_implementer)
        .unwrap_or(false);
    if implementer_is_headless || (stdin_is_tty && stdout_is_tty) {
        return Ok(no_human);
    }
    if aida_headless {
        return Ok(Some(auto_complete::NoHumanMode::Both));
    }
    anyhow::bail!(
        "aida do needs a terminal for the interactive implementer — run it from \
         your shell, or add --no-human=both for a headless implementer. \
         (`aida queue work --auto-complete` has the same requirement.)"
    );
}

/// TASK-306: the pre-launch gate for `aida queue work --auto-complete
/// --no-human`, run once per kickoff. It prints a loud scope banner — what
/// runs unattended and what does not — and requires a one-time
/// acknowledgement, either interactively or up front with
/// `AIDA_NO_HUMAN_ACKNOWLEDGED=1` for an unattended run. STORY-276: `both` is
/// now a shipped mode, so it is acknowledged like `reviewer-only` rather than
/// rejected; the banner wording differs by mode. trace:TASK-306, STORY-276
/// TASK-394: machine-wide `--no-human` acknowledgement marker
/// (`~/.aida/no-human-acknowledged`) — persists across every project on the host.
pub(crate) fn no_human_machine_marker() -> Option<std::path::PathBuf> {
    crate::home_dir().map(|h| h.join(".aida").join("no-human-acknowledged"))
}

/// TASK-394: project-scoped marker (`.aida/no-human-acknowledged`) — fresh per
/// project, so the safety prompt returns on a new clone unless re-acked.
pub(crate) fn no_human_project_marker() -> std::path::PathBuf {
    std::path::Path::new(".aida").join("no-human-acknowledged")
}

/// TASK-394: is `--no-human` acknowledged via any channel? Returns the source
/// label for the reminder line. Checked order: env var (existing path) → machine
/// marker → project marker. An overnight loop acks once (the marker) instead of
/// re-exporting the env var per iteration. trace:TASK-394 | ai:claude
pub(crate) fn no_human_ack_source() -> Option<&'static str> {
    if std::env::var("AIDA_NO_HUMAN_ACKNOWLEDGED")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return Some("AIDA_NO_HUMAN_ACKNOWLEDGED");
    }
    if no_human_machine_marker()
        .map(|p| p.exists())
        .unwrap_or(false)
    {
        return Some("~/.aida/no-human-acknowledged");
    }
    if no_human_project_marker().exists() {
        return Some(".aida/no-human-acknowledged");
    }
    None
}

/// TASK-394: `aida no-human acknowledge|revoke|status` — manage the persistent
/// `--no-human` scope-acknowledgement marker so an unattended loop acks once
/// rather than re-exporting AIDA_NO_HUMAN_ACKNOWLEDGED per iteration.
/// trace:TASK-394 | ai:claude
pub(crate) fn handle_no_human_command(cmd: &cli::NoHumanCommand) -> Result<()> {
    let marker_for = |project: bool| -> Result<std::path::PathBuf> {
        if project {
            Ok(no_human_project_marker())
        } else {
            no_human_machine_marker()
                .ok_or_else(|| anyhow::anyhow!("could not resolve home directory for the marker"))
        }
    };
    match cmd {
        cli::NoHumanCommand::Acknowledge { project } => {
            let path = marker_for(*project)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, "acknowledged\n")
                .with_context(|| format!("writing {}", path.display()))?;
            let scope = if *project {
                "this project"
            } else {
                "this machine"
            };
            println!(
                "{} --no-human acknowledged for {} ({}). Future drains skip the scope prompt.",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                scope,
                path.display()
            );
            if *project {
                println!(
                    "  {}",
                    "Note: .aida/no-human-acknowledged is gitignored runtime state (per-clone)."
                        .dimmed()
                );
            }
            Ok(())
        }
        cli::NoHumanCommand::Revoke { project } => {
            let path = marker_for(*project)?;
            if path.exists() {
                std::fs::remove_file(&path)
                    .with_context(|| format!("removing {}", path.display()))?;
                println!(
                    "{} revoked {} — the scope prompt will return.",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    path.display()
                );
            } else {
                println!("(no acknowledgement marker at {})", path.display());
            }
            Ok(())
        }
        cli::NoHumanCommand::Status => {
            match no_human_ack_source() {
                Some(src) => println!(
                    "{} --no-human is acknowledged (via {}).",
                    crate::glyph(crate::glyphs::Glyph::Check).green(),
                    src
                ),
                None => println!(
                    "{} --no-human is NOT acknowledged — the scope prompt will fire. \
                     Run `aida no-human acknowledge` to persist it.",
                    "○".dimmed()
                ),
            }
            Ok(())
        }
    }
}

pub(crate) fn no_human_kickoff_gate(mode: auto_complete::NoHumanMode) -> Result<()> {
    let ack_source = no_human_ack_source();
    let both = mode.wants_headless_implementer();
    match classify_no_human_gate(ack_source.is_some()) {
        NoHumanGate::Acknowledged => {
            eprintln!(
                "  {} {} (acknowledged via {})",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                no_human_scope_line(mode),
                ack_source.unwrap_or("acknowledgement"),
            );
            Ok(())
        }
        NoHumanGate::NeedsAck => {
            eprintln!();
            if both {
                eprintln!(
                    "{}  --no-human=both runs the implementer AND reviewer phases \
                     headless.",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                );
                eprintln!("   No human is at the keyboard during phase 1 (implementer).");
                eprintln!(
                    "   On a design-fork it cannot safely resolve, the implementer \
                     punts the spec"
                );
                eprintln!(
                    "   to Needs Attention instead of guessing — review punts later \
                     with `aida findings list`."
                );
            } else {
                eprintln!(
                    "{}  --no-human covers the reviewer phase only.",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                );
                eprintln!("   Phase 1 (implementer) still requires interactive input.");
                eprintln!("   The drain will pause at each phase-1 completion until you act.");
                eprintln!(
                    "   {}",
                    "For a fully headless drain use `--no-human=both`.".dimmed()
                );
            }
            eprintln!(
                "   {}",
                "Set AIDA_NO_HUMAN_ACKNOWLEDGED=1 to skip this prompt on an \
                 unattended run,"
                    .dimmed()
            );
            // BUG-703: keep the SPEC-ID out of this user-facing warning — a
            // trace marker in stderr is developer noise to a first-user.
            // trace:TASK-394 | ai:claude
            eprintln!(
                "   {}",
                "or `aida no-human acknowledge` once to persist it across runs \
                 (per machine; --project to scope to this repo)."
                    .dimmed()
            );
            eprintln!();
            // `--no-human` is meant for unattended drains, so a non-terminal
            // stdin is the expected unattended case — but it cannot answer a
            // prompt. Fail loud with the env-var escape rather than blocking
            // forever on a stdin read. trace:TASK-306 | ai:claude
            if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                anyhow::bail!(
                    "--no-human needs a one-time scope acknowledgement, but stdin \
                     is not a terminal. Re-run with AIDA_NO_HUMAN_ACKNOWLEDGED=1 to \
                     confirm you understand its scope."
                );
            }
            if !prompt_yes_no("   Continue? [y/N] ", false)? {
                anyhow::bail!("aborted — --no-human scope not acknowledged");
            }
            Ok(())
        }
    }
}

/// TASK-405: the `--from-pr` entry point — PR-only invocation. Implementation
/// shipped OUTSIDE the orchestrator (a PR is already open for `spec`), so drive
/// the remaining phases (reviewer → CI → merge → pull → build) WITHOUT
/// re-running the implementer. Probes the PR's real state, decides the entry
/// phase (or a clean refusal) via the pure [`drain_resume::from_pr_plan`], and
/// — unless `--dry-run` (the shared `--resume-dry-run` flag) — re-enters at that
/// phase via `run_auto_complete`'s resume-entry seam. Never returns.
///
/// Distinct from `handle_drain_resume`: there is no crashed drain-state file,
/// no PID-liveness gate (the implementer ran elsewhere, possibly by hand), and
/// the entry phase is computed straight from probed PR/spec reality.
/// trace:TASK-405 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_from_pr(
    storage: &Storage,
    user_id: &str,
    scope: &str,
    variant: auto_complete::AutoCompleteVariant,
    dry_run: bool,
    json: bool,
    permission_mode: Option<&str>,
    no_human: Option<auto_complete::NoHumanMode>,
    escalate_mode: auto_complete::EscalateMode,
    steal: bool,
    force_claim: bool,
    allow_stale_base: bool,
    no_auto_rebase: bool,
) -> ! {
    let project_root = match storage.path().parent() {
        Some(p) => p.to_path_buf(),
        None => {
            eprintln!(
                "{} cannot derive project root from the store path",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold()
            );
            std::process::exit(1);
        }
    };
    let (spec, seeded_pr) = match parse_review_scope(scope) {
        Some((forge, n)) => match storage
            .load()
            .map_err(|e| anyhow::anyhow!(e))
            .and_then(|store| resolve_pr_to_spec(&project_root, n as u32, &store))
        {
            Ok(spec) => (spec, Some(n as u32)),
            Err(e) => {
                eprintln!(
                    "{} could not resolve {} to a backing spec: {}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                    format_review_label(forge, n),
                    e
                );
                std::process::exit(1);
            }
        },
        None => (scope.to_string(), None),
    };

    // Probe the world for this spec's PR + per-phase postconditions. Passing
    // `member: None` is exactly the standalone-PR case `probe_resume_facts`
    // already handles (it falls back to a forge lookup by spec when drain-state
    // recorded no PR). trace:TASK-405 | ai:claude
    let (mut facts, branch, pr) = probe_resume_facts(&project_root, storage, &spec, None);
    let pr = seeded_pr.or(pr);
    let branch = if seeded_pr.is_some() && branch.is_none() {
        pr.and_then(|n| pr_head_branch(&project_root, n as u64))
    } else {
        branch
    };
    if let Some(n) = seeded_pr {
        let mut sink = network_retry::StderrSink;
        facts.pr_merged = pr_is_merged_with_sink(&project_root, n, &mut sink).unwrap_or(false);
        facts.branch_exists = true;
    }
    // BUG-1819: `probe_resume_facts` only carries a green bit. Reconcile the
    // provider's terminal state with the exact head it covered so a direct
    // phase-3 entry can inherit the same head-bound proof phase 2 records.
    let forge_kind = crate::forge::resolve_forge_kind(&project_root);
    let mut resume_ci_terminal_sha = None;
    let mut resume_ci_terminal_green = None;
    let mut resume_ci_refusal = None;
    if forge_kind != crate::forge::ForgeKind::None {
        facts.ci_green = false;
        if let (Some(pr), Some(branch)) = (pr, branch.as_deref()) {
            let mut sink = network_retry::StderrSink;
            let expected_head = crate::forge::forge_for_kind(&project_root, forge_kind)
                .change_metadata(pr as u64, &mut sink)
                .ok()
                .map(|metadata| metadata.head_sha);
            let evidence =
                crate::forge::ci_probe_evidence_for_branch(&project_root, forge_kind, branch);
            match verified_from_pr_ci_head(pr, expected_head.as_deref(), &evidence) {
                Ok(head) => {
                    facts.ci_green = true;
                    resume_ci_terminal_sha = Some(head);
                    resume_ci_terminal_green = Some(true);
                }
                Err(failure) => resume_ci_refusal = Some(failure.reason),
            }
        }
    }
    let pr_exists = pr.is_some();

    match drain_resume::from_pr_plan(pr_exists, &facts) {
        drain_resume::FromPrOutcome::RefuseAlreadyCompleted => {
            eprintln!(
                "{} `{}` is already Completed — the merge already promoted it; nothing to drive.",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                spec
            );
            eprintln!(
                "  {} `--from-pr` engages the orchestrator on an OPEN PR — there is no open work here.",
                "→".dimmed()
            );
            std::process::exit(1);
        }
        drain_resume::FromPrOutcome::RefuseNoPr => {
            eprintln!(
                "{} no open PR found for `{}` — nothing to drive with `--from-pr`.",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                spec
            );
            eprintln!(
                "  {} `--from-pr` drives phases 3-6 on a PR shipped outside the orchestrator. \
                 To run the FULL pipeline (implementer first) drop `--from-pr`.",
                "→".dimmed()
            );
            std::process::exit(1);
        }
        drain_resume::FromPrOutcome::RefuseAlreadyMerged => {
            eprintln!(
                "{} {} is already merged — the merge already happened, so there is nothing to drive.",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                pr.map(|n| format!("PR-{n}")).unwrap_or_else(|| "the PR".into())
            );
            eprintln!(
                "  {} run `aida pull` to auto-bump `{}` Done → Completed.",
                "→".dimmed(),
                spec
            );
            std::process::exit(1);
        }
        drain_resume::FromPrOutcome::DriveFrom(probed_start_phase) => {
            // `aida drain resume` pins the failed phase from the newest
            // SpecShelved row; the normal --from-pr path stays reality-based.
            // trace:TASK-1272 | ai:codex
            let recorded_phase = std::env::var("AIDA_DRAIN_RESUME_PHASE")
                .ok()
                .as_deref()
                .and_then(drain_resume::shelved_resume_phase);
            let start_phase = recorded_phase
                .map(|recorded| {
                    drain_resume::reconciled_shelved_phase(recorded, probed_start_phase)
                })
                .unwrap_or(probed_start_phase);
            // Last line of defence, keyed on the FACT not a phase name: a recorded
            // Reviewer outranks a probed Ci through reconciliation and would skip
            // the wait even after `from_pr_plan` demanded CI. See
            // `ci_gated_start_phase` for why a phase-name check is not enough.
            // trace:BUG-1460 trace:TASK-1272 | ai:claude
            let start_phase = drain_resume::ci_gated_start_phase(start_phase, facts.ci_green);
            if start_phase == auto_complete::Phase::Ci {
                if let Some(reason) = resume_ci_refusal.as_deref() {
                    eprintln!(
                        "  {} {reason}; re-running phase 2 before review",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                    );
                }
            }
            println!(
                "{} driving `{}` from phase {} ({}) — implementation shipped outside the \
                 orchestrator (skipping the implementer phase).",
                "↩".cyan().bold(),
                spec,
                start_phase.index(),
                start_phase.slug()
            );
            if let Some(n) = pr {
                println!("  {} seeded PR-{}", "→".dimmed(), n);
            }
            if dry_run {
                println!(
                    "  {} --resume-dry-run — not re-entering. Drop it to drive the PR.",
                    "→".dimmed()
                );
                std::process::exit(0);
            }
            // The implementer ran outside the orchestrator and may hold a stale
            // lease on this scope; release a dead/clean one so the reviewer
            // phase (which resolves PR→spec) doesn't collide. Same guard the
            // resume path applies (BUG-438). trace:TASK-405 | ai:claude
            release_dead_leases_for_resume(&project_root, &spec);
            let resume_entry = Some(ResumeEntry {
                start_phase,
                branch,
                pr,
                head_sha: None,
                from_pr: true,
                ci_terminal_sha: resume_ci_terminal_sha,
                ci_terminal_green: resume_ci_terminal_green,
            });
            let result = run_auto_complete(
                storage,
                user_id,
                &spec,
                variant,
                json,
                permission_mode,
                no_human,
                escalate_mode,
                // A standalone `--from-pr` drive owns its drain-state file.
                true,
                steal,
                force_claim,
                allow_stale_base,
                no_auto_rebase,
                resume_entry,
            );
            // TASK-1054: collapse the failed-phase index to the canonical
            // 0/2/3 process code so a wrapping script can branch on the outcome.
            std::process::exit(result.process_exit_code());
        }
    }
}

/// Entry point for `aida queue work <SPEC> --auto-complete`. Never returns:
/// always terminates the process with an exit code (0 success, 1-6 = the
/// 1-based index of the phase that failed). trace:STORY-246 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_auto_complete(
    storage: &Storage,
    user_id: &str,
    spec: &str,
    variant: auto_complete::AutoCompleteVariant,
    json: bool,
    permission_mode: Option<&str>,
    no_human: Option<auto_complete::NoHumanMode>,
    escalate_mode: auto_complete::EscalateMode,
    // BUG-311: thread the outer `--steal` through so phase 1's
    // `aida queue work` subprocess can clear a dormant lease on this scope.
    steal: bool,
    // TASK-559: thread the outer `--force-claim` through so phase 1 can
    // recover NeedsAttention / ambiguous InProgress specs the same way a
    // direct `aida queue work <SPEC> --force-claim` does.
    force_claim: bool,
    // STORY-281: thread the outer `--allow-stale-base` through so phase 3's
    // pre-flight stale-base check warns-only instead of refusing.
    allow_stale_base: bool,
    // STORY-429: opt out of phase-3 auto-rebase recovery.
    no_auto_rebase: bool,
) -> ! {
    let result = run_auto_complete(
        storage,
        user_id,
        spec,
        variant,
        json,
        permission_mode,
        no_human,
        escalate_mode,
        // STORY-301: a bare single-spec drain owns its drain-state file.
        true,
        steal,
        force_claim,
        allow_stale_base,
        no_auto_rebase,
        // STORY-492: not a resume — start at phase 1.
        None,
    );
    // STORY-493: drain-end mailbox digest for the single-spec drain, matching
    // the batch / batch-chain paths. Best-effort + non-fatal. trace:STORY-493
    if let Ok(root) = find_main_worktree_root() {
        maybe_digest_mailbox_best_effort(&root.join(".aida-store"), "drain-end");
    }
    // TASK-1054: collapse the failed-phase index to the canonical 0/2/3 process
    // code (0 clean, 2 shelved/parked, 3 hard fail) so a wrapping script can
    // branch on the outcome. trace:TASK-1054 | ai:claude
    std::process::exit(result.process_exit_code());
}

// trace:BUG-1120 | ai:codex
pub(crate) fn guarded_execution_mode_for_drain(
    storage: &Storage,
    spec: &str,
) -> Option<aida_core::ExecutionMode> {
    storage
        .load()
        .ok()
        .and_then(|store| store.get_requirement_by_spec_id(spec).cloned())
        .and_then(|req| req.execution_mode)
        .filter(|mode| {
            matches!(
                mode,
                aida_core::ExecutionMode::Guided
                    | aida_core::ExecutionMode::Operator
                    | aida_core::ExecutionMode::Decide
            )
        })
}

// trace:BUG-1120 | ai:codex
pub(crate) fn guarded_execution_mode_drain_message(
    spec: &str,
    mode: aida_core::ExecutionMode,
) -> String {
    format!(
        "skipped {spec} — needs guided/operator session ({mode}); use `aida queue work {spec} --guided` or `aida do {spec}`"
    )
}

// trace:BUG-1574 | ai:claude
/// Whether `req` requires a human at the keyboard and must never be driven,
/// stolen, rebased, or force-pushed by a headless/unattended drain. True
/// when EITHER the literal `keyboard-only` tag is present OR the groomed
/// `execution_mode` is one of the supervised/interactive modes
/// (Guided/Operator/Decide). Both signals are checked — independently —
/// because a spec can be tagged `keyboard-only` while still carrying
/// `execution_mode: drain` (the BUG-1574 incident spec, TASK-1274, was
/// exactly this shape: `execution_mode: drain` + the `keyboard-only` tag).
/// The tag is the human's explicit override and must win regardless of the
/// groomed mode.
pub(crate) fn spec_is_keyboard_only(req: &aida_core::Requirement) -> bool {
    req.tags
        .iter()
        .any(|t| t.eq_ignore_ascii_case("keyboard-only"))
        || matches!(
            req.execution_mode,
            Some(aida_core::ExecutionMode::Guided)
                | Some(aida_core::ExecutionMode::Operator)
                | Some(aida_core::ExecutionMode::Decide)
        )
}

// trace:BUG-1574 | ai:claude
/// AC2: dispatch-time refusal for `aida queue work ... --no-human` — a spec
/// that is keyboard-only ([`spec_is_keyboard_only`]) must never be handed to
/// a headless implementer, even when it groomed to `execution_mode: drain`.
/// Fail-closed: a store that won't load or a spec that can't be resolved
/// also refuses (never treated as "nothing to refuse on").
pub(crate) fn keyboard_only_dispatch_refusal(storage: &Storage, spec: &str) -> Option<String> {
    let Some(store) = storage.load().ok() else {
        return Some(format!(
            "skipped {spec} — could not load the store to verify it is safe for a headless \
             (--no-human) drain; refusing (fail closed)"
        ));
    };
    let Some(req) = store.get_requirement_by_spec_id(spec) else {
        return Some(format!(
            "skipped {spec} — could not be resolved in the store; refusing a headless \
             (--no-human) drain (fail closed)"
        ));
    };
    if spec_is_keyboard_only(req) {
        return Some(format!(
            "skipped {spec} — keyboard-only spec refused for a headless (--no-human) drain; \
             use `aida queue work {spec} --guided` or `aida do {spec}`"
        ));
    }
    None
}

// trace:BUG-1574 | ai:claude
/// A parsed snapshot of `.aida/drain-state.json`'s batch-scoping facts —
/// distinguishes "no drain running" from "a drain IS running with no batch"
/// from "the file exists but didn't parse" (which must fail closed, never
/// collapse to "no batch active" the way `DrainState::read`'s `Option`
/// return does).
pub(crate) enum DrainStateProbe {
    NoActiveDrain,
    Malformed,
    Active {
        batch: Option<String>,
        members: Vec<String>,
    },
}

// trace:BUG-1574 | ai:claude
pub(crate) fn probe_drain_state(project_root: &std::path::Path) -> DrainStateProbe {
    let path = drain_state::drain_state_path(project_root);
    match std::fs::read_to_string(&path) {
        Err(_) => DrainStateProbe::NoActiveDrain,
        Ok(body) => match serde_json::from_str::<drain_state::DrainState>(&body) {
            Err(_) => DrainStateProbe::Malformed,
            Ok(state) => DrainStateProbe::Active {
                batch: state.batch,
                members: state.members.into_iter().map(|m| m.spec).collect(),
            },
        },
    }
}

// trace:BUG-1574 | ai:claude
/// Pure fail-closed decision core for any unattended git-mutating action
/// (steal, rebase, force-push) against `spec`'s branch:
///   - `req: None` (store unreadable OR spec unresolved) → refuse, except for
///     exact synthetic review scopes (`PR-N` / `MR-N`), which deliberately do
///     not exist as requirements.
///   - `spec` is keyboard-only ([`spec_is_keyboard_only`]) → refuse.
///   - [`DrainStateProbe::Malformed`] → refuse (a torn/corrupt
///     `drain-state.json` must never read as "no batch active").
///   - a batch IS active and `spec` is not a declared member → refuse.
/// `None` (no refusal) only when the spec resolves and is not keyboard-only,
/// or the scope is an exact synthetic review scope; the drain must also have
/// no active batch or include `spec` among its members.
pub(crate) fn unattended_git_mutation_refusal_for(
    spec: &str,
    req: Option<&aida_core::Requirement>,
    drain: &DrainStateProbe,
) -> Option<String> {
    // Review sessions own synthetic PR-N / MR-N scopes rather than a
    // requirement. Requiring a store row here makes a clean dead reviewer
    // lease impossible to reclaim with --steal. Exact parsing matters: an
    // ordinary unknown spec must retain the fail-closed behavior.
    // trace:BUG-1820 | ai:codex
    match req {
        Some(req) if spec_is_keyboard_only(req) => {
            return Some(format!(
                "{spec} is keyboard-only (tag or execution_mode) — refusing an unattended \
                 rebase/force-push/steal"
            ));
        }
        Some(_) => {}
        None if parse_review_scope(spec).is_some() => {}
        None => {
            return Some(format!(
                "{spec} could not be resolved (store unreadable or spec unknown) — refusing an \
                 unattended rebase/force-push/steal (fail closed)"
            ));
        }
    }
    match drain {
        DrainStateProbe::NoActiveDrain => None,
        DrainStateProbe::Malformed => Some(format!(
            "drain-state.json exists but could not be parsed — refusing to touch {spec} \
             outside a known batch scope (fail closed)"
        )),
        DrainStateProbe::Active { batch: None, .. } => None,
        DrainStateProbe::Active {
            batch: Some(b),
            members,
        } => {
            if members.iter().any(|m| m == spec) {
                None
            } else {
                Some(format!(
                    "{spec} is not a member of the active batch `{b}` — refusing to touch a \
                     branch outside its declared scope"
                ))
            }
        }
    }
}

// trace:BUG-1574 | ai:claude
/// Walk up from `project_root` and report whether `.aida/config.toml` exists
/// anywhere in the ancestor chain — WITHOUT caring whether it parses or
/// declares a resolvable store (that is [`detect_distributed_store_from`]'s
/// job). This is the narrow signal the fail-open carve-out needs: "is this
/// even an AIDA-managed project root at all" — a bare git fixture (a unit
/// test, a non-AIDA repo) has no `.aida/` anywhere and is a clean no-op; a
/// real AIDA project always has one, so from there on a failure to resolve
/// the store/spec is a genuine problem, not an absence, and must fail
/// closed (refuse), never silently read as "nothing to check".
pub(crate) fn config_toml_exists_upward(project_root: &std::path::Path) -> bool {
    config_toml_exists_upward_with_roots(project_root, &aida_core::store_locate::real_temp_roots())
}

/// [`config_toml_exists_upward`], parameterized on the temp roots to guard
/// against.
///
/// BUG-1598: must agree with `detect_distributed_store_from` on where the
/// walk-up stops. Without this guard, a stray `.aida/config.toml` sitting
/// directly in a temp root makes this function report `true` (an
/// AIDA-managed project root exists) while the now-guarded
/// `detect_distributed_store_from` correctly refuses to resolve a store
/// from it — `unattended_git_mutation_refusal` then falls through PAST its
/// only fail-open carve-out (this function returning `false`) and fails
/// CLOSED (refuses the mutation) for a project that, from
/// `unattended_git_mutation_refusal`'s point of view, has no AIDA config at
/// all. Factored out as `_with_roots` so a test can exercise the guard
/// against a fake root without mutating `TMPDIR` or touching the real,
/// shared system temp dir.
// trace:BUG-1598 | ai:claude
pub(crate) fn config_toml_exists_upward_with_roots(
    project_root: &std::path::Path,
    temp_roots: &[std::path::PathBuf],
) -> bool {
    // Canonicalize the root set ONCE, before the loop — not on every
    // ancestor level.
    let canonical_roots = aida_core::store_locate::canonicalize_roots(temp_roots);
    let mut current = project_root;
    loop {
        if aida_core::store_locate::is_in_canonical_roots(current, &canonical_roots) {
            return false;
        }
        if current.join(".aida").join("config.toml").is_file() {
            return true;
        }
        match current.parent() {
            Some(p) => current = p,
            None => return false,
        }
    }
}

// trace:BUG-1574 | ai:claude
/// Pure: does `store` already carry an OPEN (not Completed/Rejected) finding
/// for this exact `(spec, branch)` pair? Matches on the tag triple a filed
/// refusal carries: `kind:headless-refusal` + `from-implementer:<spec>` +
/// (`branch:<branch>` when `branch` is known). Factored out so "must never
/// file duplicates" is testable against an in-memory store, without a real
/// `aida add`/`aida list` round trip.
pub(crate) fn has_open_refusal_finding(
    store: &aida_core::RequirementsStore,
    spec: &str,
    branch: Option<&str>,
) -> bool {
    let from_tag = format!("from-implementer:{spec}");
    let branch_tag = branch.map(|b| format!("branch:{b}"));
    store.requirements.iter().any(|r| {
        !matches!(
            r.status,
            aida_core::RequirementStatus::Completed | aida_core::RequirementStatus::Rejected
        ) && r.tags.contains("kind:headless-refusal")
            && r.tags.contains(&from_tag)
            && branch_tag.as_ref().is_none_or(|bt| r.tags.contains(bt))
    })
}

// trace:BUG-1574 | ai:claude
/// AC3: best-effort — file a finding (the existing `aida findings` tag
/// convention: a Draft Task tagged `from-implementer:<spec>`) recording that
/// a headless action was refused, so the refusal is visible on
/// `aida findings list` / `aida awaiting` — not just eprintln'd where only
/// the refusing process's own log carries it. This is what lets the OTHER
/// session (whose branch almost got moved) find out what nearly happened.
/// Dedupes via [`has_open_refusal_finding`] first — a drain or `--watch`
/// loop that keeps hitting the same guard (the common case: a stacked child
/// stays outside the batch on every pass until someone acts) must file ONE
/// finding, not one per pass. `branch` — when known — lands in both the
/// title and a `branch:<name>` tag, so which physical branch nearly moved is
/// visible without opening the description. A failure to file (or to check
/// for dupes) is swallowed — filing must never crash the caller.
pub(crate) fn record_headless_refusal_finding(
    project_root: &std::path::Path,
    store: Option<&aida_core::RequirementsStore>,
    spec: &str,
    action: &str,
    branch: Option<&str>,
    reason: &str,
) {
    if let Some(store) = store {
        if has_open_refusal_finding(store, spec, branch) {
            return;
        }
    }
    let title = match branch {
        Some(b) => format!("Headless {action} refused for {spec} (branch {b})"),
        None => format!("Headless {action} refused for {spec}"),
    };
    let mut tags = format!("from-implementer:{spec},severity:notice,kind:headless-refusal");
    if let Some(b) = branch {
        tags.push_str(&format!(",branch:{b}"));
    }
    let _ = std::process::Command::new(aida_exe_path())
        .current_dir(project_root)
        .args([
            "add",
            "--title",
            &title,
            "--description",
            reason,
            "--type",
            "task",
            "--status",
            "draft",
            "--tags",
            &tags,
        ])
        .output_retrying_etxtbsy();
}

// trace:BUG-1574 | ai:claude
/// Impure wrapper: loads the store + probes `.aida/drain-state.json`, then
/// delegates to the pure [`unattended_git_mutation_refusal_for`] core. This
/// is what call sites (the `--steal` scope-conflict loop, the stack-aware
/// promotion rebase/force-push, the phase-3 auto-rebase) use. `action` is a
/// short human phrase ("steal", "rebase/force-push") and `branch` — when
/// known — is the specific branch about to be touched; both land in the
/// finding filed on refusal.
///
/// The ONLY fail-open carve-out: no `.aida/config.toml` anywhere upward from
/// `project_root` ([`config_toml_exists_upward`]) — not an AIDA-managed
/// project root at all (a bare git fixture in a unit test, a legacy repo),
/// so there is nothing to scope/tag-check against. From the moment a config
/// IS found, this fails closed on everything else: a store the config
/// declares but that can't be resolved/loaded, or a spec that can't be
/// found within it, both refuse — that is the real BUG-1574 failure mode (a
/// corrupted/unreadable store, or a since-deleted spec), and must never
/// silently read as "nothing to check". Deliberately bypasses
/// [`load_store_for_lookup`]'s legacy-YAML fallback entirely: this check
/// requires the distributed store the config declares, so a legacy store
/// sitting beside it must not satisfy it. (Until BUG-1732 that fallback also
/// resolved off the process CWD rather than `project_root`, which is the
/// hazard this bypass was originally written against; the fallback is now
/// project-scoped, so only the stricter requirement remains.)
pub(crate) fn unattended_git_mutation_refusal(
    project_root: &std::path::Path,
    spec: &str,
    action: &str,
    branch: Option<&str>,
) -> Option<String> {
    if !config_toml_exists_upward(project_root) {
        return None;
    }
    let store: Option<aida_core::RequirementsStore> = detect_distributed_store_from(project_root)
        .and_then(|store_path| aida_core::GitBackend::new(&store_path).ok())
        .and_then(|backend| aida_core::DatabaseBackend::load(&backend).ok());
    let req = store
        .as_ref()
        .and_then(|s| s.get_requirement_by_spec_id(spec));
    let drain = probe_drain_state(project_root);
    let reason = unattended_git_mutation_refusal_for(spec, req, &drain)?;
    record_headless_refusal_finding(project_root, store.as_ref(), spec, action, branch, &reason);
    Some(reason)
}

#[cfg(test)]
mod bug_1574_unattended_git_mutation_tests {
    use super::*;

    fn req_with(tags: &[&str], mode: Option<aida_core::ExecutionMode>) -> aida_core::Requirement {
        let mut r = aida_core::Requirement::new("t".into(), "d".into());
        for t in tags {
            r.tags.insert(t.to_string());
        }
        r.execution_mode = mode;
        r
    }

    fn active(batch: Option<&str>, members: &[&str]) -> DrainStateProbe {
        DrainStateProbe::Active {
            batch: batch.map(|s| s.to_string()),
            members: members.iter().map(|s| s.to_string()).collect(),
        }
    }

    // ── fail-closed: missing req (store load failure OR unresolved spec) ────

    #[test]
    fn refuses_when_req_is_none() {
        let reason =
            unattended_git_mutation_refusal_for("TASK-1", None, &DrainStateProbe::NoActiveDrain);
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("fail closed"));
    }

    // BUG-1820: reviewer leases use synthetic scopes, so a missing
    // requirement is expected for both forge spellings. The exact parser
    // keeps unknown and malformed ids on the fail-closed path.
    // trace:BUG-1820 | ai:codex
    #[test]
    fn allows_synthetic_pr_and_mr_scopes_without_requirement_rows() {
        for scope in ["PR-26", "MR-26", "pr-26", "mr-26"] {
            assert!(
                unattended_git_mutation_refusal_for(scope, None, &DrainStateProbe::NoActiveDrain)
                    .is_none(),
                "synthetic review scope {scope} must not require a requirement row"
            );
        }
    }

    #[test]
    fn malformed_review_like_scopes_still_fail_closed() {
        for scope in ["PR-x", "MR-", "PR-26-extra", "REVIEW-26"] {
            let reason =
                unattended_git_mutation_refusal_for(scope, None, &DrainStateProbe::NoActiveDrain);
            assert!(
                reason.is_some_and(|reason| reason.contains("fail closed")),
                "review-like scope {scope} must not bypass requirement lookup"
            );
        }
    }

    #[test]
    fn synthetic_review_scope_still_refuses_malformed_drain_state() {
        let reason =
            unattended_git_mutation_refusal_for("MR-26", None, &DrainStateProbe::Malformed);
        assert!(reason.is_some_and(|reason| reason.contains("fail closed")));
    }

    #[test]
    fn synthetic_review_scope_outside_active_batch_still_refuses() {
        let drain = active(Some("night-0920"), &["STORY-207"]);
        let reason = unattended_git_mutation_refusal_for("PR-26", None, &drain);
        assert!(reason.is_some_and(|reason| reason.contains("night-0920")));
    }

    // ── keyboard-only: tag wins even when execution_mode is drain ──────────

    #[test]
    fn refuses_on_keyboard_only_tag_even_with_drain_mode() {
        let req = req_with(&["keyboard-only"], Some(aida_core::ExecutionMode::Drain));
        let reason = unattended_git_mutation_refusal_for(
            "TASK-1274",
            Some(&req),
            &DrainStateProbe::NoActiveDrain,
        );
        assert!(
            reason.is_some(),
            "the tag alone must refuse, mode notwithstanding"
        );
        assert!(reason.unwrap().contains("keyboard-only"));
    }

    #[test]
    fn refuses_on_guided_mode_without_the_tag() {
        let req = req_with(&[], Some(aida_core::ExecutionMode::Guided));
        assert!(unattended_git_mutation_refusal_for(
            "TASK-2",
            Some(&req),
            &DrainStateProbe::NoActiveDrain
        )
        .is_some());
    }

    // ── malformed drain-state.json: fail closed, never "no batch" ──────────

    #[test]
    fn refuses_on_malformed_drain_state() {
        let req = req_with(&[], Some(aida_core::ExecutionMode::Drain));
        let reason =
            unattended_git_mutation_refusal_for("TASK-3", Some(&req), &DrainStateProbe::Malformed);
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("fail closed"));
    }

    // ── batch membership ─────────────────────────────────────────────────

    #[test]
    fn refuses_spec_outside_active_batch() {
        let req = req_with(&[], Some(aida_core::ExecutionMode::Drain));
        let drain = active(Some("night-0920"), &["STORY-1", "STORY-2"]);
        let reason = unattended_git_mutation_refusal_for("TASK-1274", Some(&req), &drain);
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("night-0920"));
    }

    // ── controls: the allowed path ──────────────────────────────────────

    #[test]
    fn allows_drain_mode_member_of_active_batch() {
        let req = req_with(&[], Some(aida_core::ExecutionMode::Drain));
        let drain = active(Some("night-0920"), &["TASK-1274", "STORY-2"]);
        assert!(unattended_git_mutation_refusal_for("TASK-1274", Some(&req), &drain).is_none());
    }

    #[test]
    fn allows_no_execution_mode_no_tag_when_no_batch_active() {
        let req = req_with(&[], None);
        assert!(unattended_git_mutation_refusal_for(
            "TASK-1274",
            Some(&req),
            &DrainStateProbe::NoActiveDrain
        )
        .is_none());
    }

    #[test]
    fn allows_unlisted_spec_when_active_drain_has_no_batch() {
        // A single-spec / next-n drain writes drain-state.json with
        // `batch: None` — never refuse on membership in that case.
        let req = req_with(&[], Some(aida_core::ExecutionMode::Drain));
        let drain = active(None, &[]);
        assert!(unattended_git_mutation_refusal_for("TASK-1274", Some(&req), &drain).is_none());
    }

    // ── probe_drain_state: distinguishes absent vs malformed on disk ──────

    #[test]
    fn probe_distinguishes_absent_from_malformed_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            probe_drain_state(dir.path()),
            DrainStateProbe::NoActiveDrain
        ));
        let path = drain_state::drain_state_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ this is not valid json").unwrap();
        assert!(matches!(
            probe_drain_state(dir.path()),
            DrainStateProbe::Malformed
        ));
    }

    // ── AC2: keyboard_only_dispatch_refusal (the --no-human dispatch gate) ──

    fn storage_with(
        spec: &str,
        tags: &[&str],
        mode: Option<aida_core::ExecutionMode>,
    ) -> (tempfile::TempDir, Storage) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().join("requirements.yaml"));
        let mut store = aida_core::RequirementsStore::new();
        let mut req = req_with(tags, mode);
        req.spec_id = Some(spec.to_string());
        store.requirements.push(req);
        storage.save(&store).unwrap();
        (dir, storage)
    }

    #[test]
    fn dispatch_refuses_keyboard_only_tag() {
        let (_dir, storage) = storage_with(
            "TASK-1274",
            &["keyboard-only"],
            Some(aida_core::ExecutionMode::Drain),
        );
        let reason = keyboard_only_dispatch_refusal(&storage, "TASK-1274");
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("keyboard-only"));
    }

    #[test]
    fn dispatch_refuses_guided_mode() {
        let (_dir, storage) = storage_with("TASK-2", &[], Some(aida_core::ExecutionMode::Guided));
        assert!(keyboard_only_dispatch_refusal(&storage, "TASK-2").is_some());
    }

    #[test]
    fn dispatch_allows_plain_drain_spec() {
        let (_dir, storage) = storage_with("TASK-3", &[], Some(aida_core::ExecutionMode::Drain));
        assert!(keyboard_only_dispatch_refusal(&storage, "TASK-3").is_none());
    }

    // ── AC2, end-to-end through the real dispatch fn (not just the helper) ──

    #[test]
    fn ac2_run_auto_complete_refuses_keyboard_only_tag_under_no_human() {
        // The tag-under-drain-mode shape TASK-1274 actually had — must be
        // refused at dispatch under `--no-human`, before any queueing/I-O
        // past the guard.
        let (_dir, storage) = storage_with(
            "TASK-1274",
            &["keyboard-only"],
            Some(aida_core::ExecutionMode::Drain),
        );
        let result = run_auto_complete(
            &storage,
            "test-user",
            "TASK-1274",
            auto_complete::AutoCompleteVariant::Full,
            false,
            None,
            Some(auto_complete::NoHumanMode::Both),
            auto_complete::EscalateMode::Blocks,
            true,
            false,
            false,
            false,
            false,
            None,
        );
        assert_eq!(result.failed_phase, Some(auto_complete::Phase::Implementer));
    }

    // ── the wrapper's two carve-out cases ────────────────────────────────

    #[test]
    fn wrapper_allows_when_no_config_toml_exists_upward() {
        // A bare fixture with no `.aida/` anywhere — not an AIDA-managed
        // project root at all, so there is nothing to scope/tag-check
        // against. The ONLY fail-open case.
        let dir = tempfile::tempdir().unwrap();
        assert!(unattended_git_mutation_refusal(dir.path(), "TASK-1274", "test", None).is_none());
    }

    /// BUG-1598: `config_toml_exists_upward` must agree with
    /// `detect_distributed_store_from` on where the walk-up stops — a stray
    /// `.aida/config.toml` sitting directly in a temp root must NOT make
    /// this function report `true` (which would push
    /// `unattended_git_mutation_refusal` past its only fail-open carve-out
    /// and refuse a mutation for a project that has no real AIDA config at
    /// all). A FAKE temp root (a plain tempdir, injected — never `TMPDIR`,
    /// never the real shared `/tmp`) holds a planted `.aida/config.toml`
    /// directly at its own root; a fixture one level under it must not see
    /// it.
    // trace:BUG-1598 | ai:claude
    #[test]
    fn config_toml_exists_upward_never_adopts_a_temp_root() {
        let fake_temp_root = tempfile::tempdir().unwrap();
        let roots = vec![fake_temp_root.path().to_path_buf()];

        std::fs::create_dir_all(fake_temp_root.path().join(".aida")).unwrap();
        std::fs::write(
            fake_temp_root.path().join(".aida").join("config.toml"),
            "store_path = \".aida-store\"\n",
        )
        .unwrap();

        let nested = fake_temp_root.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();

        assert!(
            !config_toml_exists_upward_with_roots(&nested, &roots),
            "must never report a temp root's ambient .aida/config.toml as an AIDA project root"
        );

        // Sanity check: WITHOUT the guard (empty roots list), the same
        // fixture DOES report true — proving the guard, not some other
        // difference, is what suppresses the false positive above.
        assert!(
            config_toml_exists_upward_with_roots(&nested, &[]),
            "fixture must be adoptable when nothing is guarded, or this test proves nothing"
        );
    }

    #[test]
    fn wrapper_refuses_when_config_declares_an_unresolvable_store() {
        // A REAL AIDA project root (config.toml present) whose declared
        // store can't be resolved (deleted, corrupted, wrong path) must
        // fail closed — never silently read as "nothing to check".
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            dir.path().join(".aida").join("config.toml"),
            "store_path = \"nonexistent-store\"\n",
        )
        .unwrap();
        let reason = unattended_git_mutation_refusal(dir.path(), "TASK-1274", "test", None);
        assert!(
            reason.is_some(),
            "a declared-but-unresolvable store must refuse"
        );
        assert!(reason.unwrap().contains("fail closed"));
    }

    #[test]
    fn wrapper_allows_synthetic_review_scopes_with_an_unresolvable_store() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            dir.path().join(".aida").join("config.toml"),
            "store_path = \"nonexistent-store\"\n",
        )
        .unwrap();

        for scope in ["PR-26", "MR-26"] {
            assert!(
                unattended_git_mutation_refusal(dir.path(), scope, "steal", None).is_none(),
                "synthetic review scope {scope} must not depend on store lookup"
            );
        }
    }

    // ── dedupe: a second refusal for the same (spec, branch) is a no-op ────

    #[test]
    fn dedupe_second_refusal_sees_the_first_findings_open_dupe() {
        let mut store = aida_core::RequirementsStore::new();
        // Nothing filed yet — the first refusal would file.
        assert!(!has_open_refusal_finding(
            &store,
            "TASK-1274",
            Some("task-1274-work")
        ));

        // Simulate the FIRST refusal's finding landing in the store.
        let mut finding = aida_core::Requirement::new(
            "Headless rebase/force-push refused for TASK-1274 (branch task-1274-work)".to_string(),
            "reason".to_string(),
        );
        finding.status = aida_core::RequirementStatus::Draft;
        finding
            .tags
            .insert("from-implementer:TASK-1274".to_string());
        finding.tags.insert("kind:headless-refusal".to_string());
        finding.tags.insert("branch:task-1274-work".to_string());
        store.requirements.push(finding);

        // A SECOND refusal for the same (spec, branch) must see it and
        // skip — never file a duplicate.
        assert!(has_open_refusal_finding(
            &store,
            "TASK-1274",
            Some("task-1274-work")
        ));
    }

    #[test]
    fn dedupe_ignores_closed_findings_and_other_branches() {
        let mut store = aida_core::RequirementsStore::new();
        let mut closed = aida_core::Requirement::new("t".to_string(), "d".to_string());
        closed.status = aida_core::RequirementStatus::Completed;
        closed.tags.insert("from-implementer:TASK-1274".to_string());
        closed.tags.insert("kind:headless-refusal".to_string());
        closed.tags.insert("branch:task-1274-work".to_string());
        store.requirements.push(closed);
        assert!(
            !has_open_refusal_finding(&store, "TASK-1274", Some("task-1274-work")),
            "a Completed/Rejected finding must not suppress a new refusal"
        );

        let mut other_branch = aida_core::Requirement::new("t".to_string(), "d".to_string());
        other_branch.status = aida_core::RequirementStatus::Draft;
        other_branch
            .tags
            .insert("from-implementer:TASK-1274".to_string());
        other_branch
            .tags
            .insert("kind:headless-refusal".to_string());
        other_branch.tags.insert("branch:other-branch".to_string());
        store.requirements.push(other_branch);
        assert!(
            !has_open_refusal_finding(&store, "TASK-1274", Some("task-1274-work")),
            "a finding for a DIFFERENT branch must not suppress this one"
        );
    }
}

/// STORY-265 slice 3: execute the `--with-plan` PLAN PRELUDE for one spec —
/// the plan phase that runs before the auto-complete drain's phase 1. Reuses
/// slice 2's `aida queue work <spec> --plan-only` planning session (headless
/// when the drain runs a headless implementer) and slice 1's
/// `aida plan promote <spec>` Approved→Planned transition, in that order
/// (decided by [`auto_complete::plan_prelude_steps`]). The drain itself is
/// entered unchanged after this returns `Ok`; the Phase enum is untouched (the
/// prelude is NOT a renumbered phase). Each step shells out to the current
/// `aida` binary; a non-zero exit aborts the prelude so the caller skips the
/// drain. trace:STORY-265 | ai:claude
pub(crate) fn run_plan_prelude(spec: &str, headless_implementer: bool, json: bool) -> Result<()> {
    use auto_complete::PlanPreludeStep;
    let exe = aida_exe_path();
    let steps = auto_complete::plan_prelude_steps(spec, true, headless_implementer);
    if !json {
        eprintln!();
        eprintln!(
            "{} {} {}",
            "📝".bold(),
            format!("plan prelude: {spec}").bold(),
            "(plan phase → promote → drain)".dimmed()
        );
    }
    for step in steps {
        match step {
            PlanPreludeStep::PlanSession { spec: s, headless } => {
                let mut args: Vec<String> = vec![
                    "queue".into(),
                    "work".into(),
                    s.clone(),
                    "--plan-only".into(),
                ];
                if headless {
                    // Mirror the phase-1 headless implementer launch onto the
                    // plan session so a `--no-human=both` drain plans headless
                    // too. trace:STORY-265 | ai:claude
                    args.push("--no-human".into());
                }
                let status = std::process::Command::new(&exe)
                    .args(&args)
                    .status_retrying_etxtbsy()
                    .with_context(|| format!("could not launch the plan session for {s}"))?;
                if !status.success() {
                    anyhow::bail!(
                        "the plan session exited with status {} — no plan file was produced",
                        status.code().unwrap_or(-1)
                    );
                }
            }
            PlanPreludeStep::Promote { spec: s } => {
                let status = std::process::Command::new(&exe)
                    .args(["plan", "promote", &s])
                    .status_retrying_etxtbsy()
                    .with_context(|| format!("could not run `aida plan promote {s}`"))?;
                if !status.success() {
                    anyhow::bail!(
                        "`aida plan promote {s}` exited with status {} — the spec was not \
                         promoted to Planned (does the plan file's `Specs:` header list {s}?)",
                        status.code().unwrap_or(-1)
                    );
                }
            }
        }
    }
    Ok(())
}

/// Run one `--auto-complete` orchestration and return its result *without*
/// exiting the process — so a batch drain (TASK-285) can chain runs. Handles
/// the preflight queue, the real driver, and the TASK-266 telemetry record.
/// Hard environment errors (project root unresolvable, preflight queue-add
/// failed) still exit the process directly — they would recur identically on
/// every batch iteration. trace:STORY-246, TASK-285 | ai:claude
#[allow(clippy::too_many_arguments)]
/// BUG-2139: the seats a `--no-human` drain launches headless: the
/// implementer under `both`, and the reviewer under every mode.
// trace:BUG-2139 | ai:claude
pub(crate) fn headless_phase_seats(mode: auto_complete::NoHumanMode) -> Vec<&'static str> {
    let mut seats = Vec::new();
    if mode.wants_headless_implementer() {
        seats.push("implementer");
    }
    seats.push("reviewer");
    seats
}

/// BUG-2139: refuse a `--no-human` drain up front when the launcher cannot
/// give one of its headless phases a child grant (a baseline seat may still
/// run ungranted; see `seat_authority::preflight_headless_child`).
// trace:BUG-2139 | ai:claude
pub(crate) fn preflight_headless_phase_seats(
    project_root: &std::path::Path,
    mode: auto_complete::NoHumanMode,
) -> anyhow::Result<()> {
    for seat in headless_phase_seats(mode) {
        crate::seat_authority::preflight_headless_child(project_root, seat).map_err(|cause| {
            anyhow::anyhow!(
                "headless drain refused before any phase started: the headless `{seat}` phase \
                 needs a `{seat}` child grant, but {cause:#}"
            )
        })?;
    }
    Ok(())
}

pub(crate) fn run_auto_complete(
    storage: &Storage,
    user_id: &str,
    spec: &str,
    variant: auto_complete::AutoCompleteVariant,
    json: bool,
    permission_mode: Option<&str>,
    no_human: Option<auto_complete::NoHumanMode>,
    // STORY-306: how an advisor escalation is handled — `Blocks` (default) or
    // `Defaults`. Only meaningful under `--no-human=both`.
    escalate_mode: auto_complete::EscalateMode,
    // STORY-301: `true` for a standalone single-spec drain — this run creates
    // the `.aida/drain-state.json` file and clears it on return. `false` for a
    // batch / nextN member — the batch orchestrator owns the file; this run
    // only announces it is the current spec.
    owns_drain_state: bool,
    // BUG-311: pass-through of the outer `--steal` flag — appended to the
    // phase-1 `aida queue work` subprocess so it can clear a dormant lease
    // on this scope. Without this thread-through the outer flag silently
    // dropped on the floor and the canned "pass --steal" message recurred.
    // trace:BUG-311 | ai:claude
    steal: bool,
    // TASK-559: pass-through of the outer `--force-claim` flag, appended to
    // phase-1 `aida queue work` subprocesses just like `--steal`.
    force_claim: bool,
    // STORY-281: pass-through of the outer `--allow-stale-base` flag. When
    // true, phase 3's pre-flight check warns-only instead of refusing on a
    // stale-base + file-overlap; also propagated to the reviewer subprocess.
    // trace:STORY-281 | ai:claude
    allow_stale_base: bool,
    // STORY-429: pass-through of the outer `--no-auto-rebase` flag. When
    // false, fully-headless phase 3 can attempt one clean PR rebase before
    // falling back to STORY-281 refusal.
    no_auto_rebase: bool,
    // STORY-492: `None` for a normal drain (start at phase 1, run everything).
    // `Some` for a `--resume-drain` re-entry — carries the reconciled
    // `start_phase` (skip earlier phases) plus the branch + PR to seed into the
    // driver so the resumed phases have the context the skipped phases would
    // have discovered. trace:STORY-492 | ai:claude
    resume: Option<ResumeEntry>,
) -> auto_complete::OrchestrationResult {
    // BUG-657: a spec that is already terminal (Completed / Rejected) is a clean
    // NO-OP — return BEFORE touching the queue or spawning anything. Driving it
    // would queue it, spawn an implementer that exits 1 ("nothing to implement"),
    // and auto-draft a phantom failure BUG — the BUG-638 → BUG-644..649 incident
    // (6 identical drafts in 6 minutes). A `--resume` / `--from-pr` re-entry
    // (`resume.is_some()`) is exempt: it legitimately re-drives a spec that
    // reached Completed mid-pipeline (the merge promoted it; the BUG-241 reconcile
    // treats that as an out-of-band success). The pure orchestrator carries the
    // same guard via `PhaseDriver::terminal_status` for defense-in-depth + unit
    // testing; this earlier check is what keeps the queue side-effect-free.
    // trace:BUG-657 | ai:claude
    if resume.is_none() {
        if let Some(mode) = guarded_execution_mode_for_drain(storage, spec) {
            eprintln!("{}", guarded_execution_mode_drain_message(spec, mode));
            return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Implementer);
        }
        // BUG-1574 AC2: a spec tagged keyboard-only must be refused at
        // dispatch under a headless (`--no-human`) drain even when it
        // groomed to `execution_mode: drain` — the tag above wins. This is
        // in addition to the execution_mode-only check above, which already
        // covers Guided/Operator/Decide unconditionally; this one covers the
        // tag specifically for the headless path. trace:BUG-1574 | ai:claude
        if no_human.is_some() {
            if let Some(reason) = keyboard_only_dispatch_refusal(storage, spec) {
                eprintln!("{reason}");
                return auto_complete::OrchestrationResult::failed(
                    auto_complete::Phase::Implementer,
                );
            }
        }
        if let Ok(root) = find_main_worktree_root() {
            let terminal = match spec_status(&root, spec) {
                Some(RequirementStatus::Completed) => Some("Completed"),
                Some(RequirementStatus::Rejected) => Some("Rejected"),
                _ => None,
            };
            if let Some(status) = terminal {
                return auto_complete::finish_noop(spec, status, json, &std::time::Instant::now());
            }
        }
    }
    // BUG-2139: every headless phase runs under its own child grant, so a
    // launcher that cannot delegate a headless phase's seat would otherwise be
    // refused only when that phase starts — the reviewer after phase 1 has
    // already pushed. Preflight every headless phase seat up front, before the
    // queue add below or any phase-1 mutation. Read-only.
    // trace:BUG-2139 | ai:claude
    if let Some(mode) = no_human {
        if let Ok(root) = find_main_worktree_root() {
            if let Err(reason) = preflight_headless_phase_seats(&root, mode) {
                eprintln!(
                    "{} {reason:#}",
                    crate::glyph(crate::glyphs::Glyph::Cross).red().bold()
                );
                return auto_complete::OrchestrationResult::failed(
                    auto_complete::Phase::Implementer,
                );
            }
        }
    }
    // Preflight: `aida queue work <spec>` can only pick up a spec that's
    // queued for the implementer. Queue it if it isn't — so a fresh
    // `aida add` flows straight into `--auto-complete`.
    if let Err(e) = ensure_queued_for_implementer(storage, user_id, spec) {
        eprintln!(
            "{} {}",
            crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
            e
        );
        std::process::exit(1);
    }
    // STORY-265 slice 3: the `--with-plan` PLAN PRELUDE. When `--with-plan` is
    // set (propagated as `AIDA_WITH_PLAN=1` by the dispatch arm), run a plan
    // session + Approved→Planned promote BEFORE the phase-1 status bump and the
    // drain. This is a prelude, NOT a renumbered phase — the 6-phase drain
    // below is entered unchanged, so the Phase enum's index-as-exit-code
    // contract is preserved. The prelude runs once per drained spec (single /
    // batch / nextN all route through here). A prelude failure aborts this
    // spec's run before any work is bumped/leased. trace:STORY-265 | ai:claude
    if std::env::var("AIDA_WITH_PLAN").is_ok() {
        let headless_implementer = no_human
            .map(auto_complete::NoHumanMode::wants_headless_implementer)
            .unwrap_or(false);
        if let Err(e) = run_plan_prelude(spec, headless_implementer, json) {
            eprintln!(
                "{} plan prelude failed for {}: {} — the drain did not start \
                 (re-run, or drop --with-plan to implement without a plan phase)",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                spec,
                e
            );
            return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Implementer);
        }
    }
    let lifecycle_skip = match resolve_lifecycle_skip(storage, spec) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "{} {}",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                e
            );
            std::process::exit(1);
        }
    };

    let project_root = match find_main_worktree_root() {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "{} could not resolve the project root: {}",
                crate::glyph(crate::glyphs::Glyph::Cross).red().bold(),
                e
            );
            std::process::exit(1);
        }
    };

    // STORY-263 / TASK-306 / STORY-276: a one-line per-run reminder of the
    // headless scope. The loud scope banner + acknowledgement already fired
    // once at kickoff (`no_human_kickoff_gate`); this keeps the scope visible
    // in a batch drain's per-member scrollback. The wording follows the mode:
    // `both` runs phase 1 headless (with the punt safety net), `reviewer-only`
    // leaves it interactive. trace:STORY-263, TASK-306, STORY-276 | ai:claude
    if let Some(mode) = no_human {
        if !json {
            if mode.wants_headless_implementer() {
                eprintln!(
                    "  {} headless implementer + reviewer — phase 1 punts to \
                     Needs Attention on a design-fork it cannot resolve",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
                );
            } else {
                eprintln!(
                    "  {} headless reviewer phase — phase 1 (implementer) stays \
                     interactive and will pause for you",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
                );
            }
        }
    }

    // BUG-233 / TASK-336: mint this orchestrator run's corroboration UUID and
    // persist it into the drain-state file. A phase child carrying
    // `AIDA_AUTO_COMPLETE_TOKEN=<uuid>` corroborates orchestrator-mode against
    // `DrainState::run_uuid` + a `pid_is_alive` check on the recorded
    // orchestrator PID. Before TASK-336 this lived in a sidecar
    // `.aida/orchestrator-runs/<uuid>` marker file; that file has been removed
    // since the drain-state file already records every other field the check
    // needs. trace:BUG-233 trace:TASK-336 | ai:claude
    //
    // ADR-7/ADR-10: resolve this run's autonomy ONCE into a typed value (the
    // single in-process source of truth) rather than re-reading `AIDA_ZEN` as a
    // bare env bool. `for_auto_complete_run` unifies the `--no-human` mode and
    // the `--zen` intent token; it preserves the BUG-237 guarantee that a
    // leaked `AIDA_ZEN=1` (no token) is not recorded as a zen run. The env var
    // stays as the cross-process transport to phase children / skill templates.
    // trace:ADR-7 trace:ADR-10 trace:BUG-237 | ai:claude
    let autonomy = AutonomyMode::for_auto_complete_run(no_human);
    let run_token = uuid::Uuid::now_v7().to_string();

    // ADR-10: build the driver FIRST, carrying the resolved-once typed
    // `autonomy` alongside `no_human`, so the drain-state stamping below reads
    // the carried field (`driver.is_zen_run()`) rather than re-deriving zen-ness
    // from a bare `AIDA_ZEN` env read. The env var stays as the cross-process
    // transport to phase children / skill templates. trace:ADR-10 | ai:claude
    // BUG-2146: resolve the drain's contained posture as a hard precondition.
    // A malformed agents.toml is a refusal naming the file — never a silent
    // uncontained drain. (`RealPhaseDriver::new` re-reads it and fails closed.)
    // trace:BUG-2146 | ai:claude
    if let Err(err) = crate::session::drain_contained(&project_root) {
        eprintln!(
            "{} {err:#}",
            crate::glyph(crate::glyphs::Glyph::Cross).red().bold()
        );
        return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Implementer);
    }
    let mut driver = RealPhaseDriver::new(
        project_root.clone(),
        spec.to_string(),
        user_id.to_string(),
        permission_mode.map(|s| s.to_string()),
        json,
        no_human,
        autonomy,
        run_token.clone(),
        steal,
        force_claim,
        allow_stale_base,
        no_auto_rebase,
        lifecycle_skip,
        variant,
    );

    // trace:TASK-1603 | ai:codex
    // Publish ownership before changing status or launching any phase child.
    // This is authority state, not optional telemetry: a missing/failed write
    // would make the first child apply standalone lease rules to our own bump.
    let owns_drain_state =
        owns_drain_state && std::env::var_os("AIDA_PIPELINED_BATCH_CHILD").is_none();
    // Retain the prior status for TASK-133's lease-less failure recovery.
    let phase1_bump = match prepare_registered_auto_complete_phase1(
        storage,
        &project_root,
        spec,
        &run_token,
        driver.is_zen_run(),
        owns_drain_state,
    ) {
        Ok(bump) => bump,
        Err(e) => {
            eprintln!("could not prepare orchestrator ownership/status for {spec}: {e} — no phase launched");
            return auto_complete::OrchestrationResult::failed(auto_complete::Phase::Implementer);
        }
    };
    // STORY-492: a resume re-entry seeds the driver with the branch + PR the
    // skipped phases would otherwise have discovered, so the resumed phases
    // (CI / reviewer / merge / …) have the context they need.
    let start_phase = match &resume {
        Some(r) => {
            driver.seed_resume_state(
                r.branch.clone(),
                r.pr,
                r.head_sha.clone(),
                r.ci_terminal_sha.clone(),
                r.ci_terminal_green,
            );
            driver.from_pr = r.from_pr;
            r.start_phase
        }
        None => auto_complete::Phase::Implementer,
    };
    let started_at = chrono::Utc::now();
    // TASK-136: a batch / nextN member (`owns_drain_state == false`) shelves a
    // still-inconclusive phase-1 verify so the drain advances; a standalone
    // single-spec drain keeps the Inconclusive pause. The two are exact
    // inverses, so `batch` derives directly from `owns_drain_state`.
    let batch = !owns_drain_state;
    // TASK-827: solo mode as a max-discretion safe-backlog POSTURE. When solo is
    // active (`presence::current_solo`), fold it into this spec's escalate
    // behaviour per-spec: SAFE work proceeds on the defensible default
    // (ProceedOnDefault → Defaults — maximum discretion), KEYSTONE/architecture
    // work parks for the human (ParkForHuman → Blocks — never ship keystone
    // unattended). Reuses the existing escalate mechanism + park path; when solo
    // is inactive the posture is `Inactive` and `escalate_mode` is UNCHANGED, so
    // non-solo behaviour is untouched. Only meaningful under `--no-human=both`
    // where the advisor escalation tier runs. trace:TASK-827 | ai:claude
    let escalate_mode = {
        // The advisor escalation tier (where `escalate_mode` is consulted) only
        // runs under `--no-human=both`; gate the posture there so a non-headless
        // solo drain neither prints a misleading banner nor flips a no-op flag.
        let advisor_tier_runs = no_human == Some(auto_complete::NoHumanMode::Both);
        let solo_active = advisor_tier_runs && presence::current_solo(chrono::Utc::now());
        let is_keystone = solo_active && solo_spec_is_keystone(storage, spec);
        let posture = presence::resolve_solo_posture(solo_active, is_keystone);
        if posture.is_active() {
            if !json {
                if matches!(posture, presence::SoloPosture::ParkForHuman) {
                    eprintln!(
                        "  {} solo posture: working safe backlog, parking keystone for human ({} classified keystone — parks on a design-fork)",
                        crate::glyph(crate::glyphs::Glyph::Robot).bold(),
                        spec
                    );
                } else {
                    eprintln!(
                        "  {} solo posture: working safe backlog, parking keystone for human ({} is safe — proceeds on the defensible default)",
                        crate::glyph(crate::glyphs::Glyph::Robot).bold(),
                        spec
                    );
                }
            }
            auto_complete::EscalateMode::from_flags(posture.escalate_defaults())
        } else {
            escalate_mode
        }
    };
    let result = auto_complete::orchestrate_with_resume(
        &mut driver,
        spec,
        variant,
        json,
        escalate_mode,
        lifecycle_skip,
        batch,
        start_phase,
    );
    let completed_at = chrono::Utc::now();

    // TASK-133: compensate the pre-spawn phase-1 status bump. If phase 1
    // failed *and* the implementer child never recorded a lease, no work
    // happened — restore the captured prior status (clearing the spurious
    // shelve `failure_reason`) so the spec is cleanly re-queueable rather than
    // stranded InProgress→NeedsAttention behind a transient spawn/contention
    // error. A lease-acquired or later-phase failure leaves real work to
    // triage and is left shelved. trace:TASK-133 | ai:claude
    if let Some((display_id, prior)) = &phase1_bump {
        let lease_acquired = driver.implementer_lease.is_some();
        if auto_complete::should_compensate_phase1_bump(true, lease_acquired, result.failed_phase) {
            // BUG-1134: a lease-less phase-1 failure usually means no work happened —
            // but it can be a worktree-reuse collision AFTER a prior attempt already
            // opened a PR. Restoring to un-started would orphan that PR and mislead
            // with "no work was stranded". Detect a definitive open PR first; an
            // absent/flaky forge falls back to the safe restore. trace:BUG-1134
            let pr_lookup = crate::forge::forge_for(&project_root)
                .change_for_spec(spec)
                .unwrap_or(crate::forge::ChangeLookup::NoChange);
            match auto_complete::classify_phase1_failure_recovery(&pr_lookup) {
                auto_complete::Phase1FailureRecovery::PreserveOpenPr => {
                    if let crate::forge::ChangeLookup::Found(pr) = &pr_lookup {
                        eprintln!(
                            "  {} phase-1 failed, but {} ALREADY HAS AN OPEN PR (#{}) — the \
                             work shipped (a worktree-reuse collision, not lost work). Not \
                             restoring to un-started; resume with `aida queue work {} --resume` \
                             or review it: {}",
                            "⚠".yellow(),
                            display_id,
                            pr.id,
                            display_id,
                            pr.url
                        );
                    }
                    // Leave the status as phase-1 left it — the open PR is real work;
                    // restoring to a clean un-started status would orphan it.
                }
                auto_complete::Phase1FailureRecovery::RestoreUnstarted => {
                    // BUG-1638: report the restore only when it happened.
                    // trace:BUG-1638 | ai:claude
                    match restore_phase1_status_on_lease_failure(&project_root, spec, prior) {
                        // BUG-1647: the refusal honours --json like the
                        // restore does (BUG-1651: pinned through
                        // `phase1_restore_report_line`).
                        // trace:BUG-1647 trace:BUG-1651 | ai:claude
                        Ok(outcome) => {
                            if let Some(line) =
                                phase1_restore_report_line(&outcome, display_id, prior, json)
                            {
                                eprintln!("{line}");
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "  {} could not restore {}'s status after a lease-less phase-1 \
                                 failure: {} — reset it manually with `aida edit {} --status {}`",
                                crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                                display_id,
                                e,
                                display_id,
                                format!("{prior:?}").to_lowercase(),
                            );
                        }
                    }
                }
            }
        }
    }

    // trace:TASK-1603 | ai:codex
    // All this run's children have been reaped. Revoke its token only;
    // concurrent members must retain authority through their later phases.
    drain_state::clear_run(&project_root, &run_token);

    // TASK-266: log the run to `~/.aida/auto-complete.jsonl` and, on a phase
    // failure, auto-draft a Draft BUG so the friction surfaces back to the
    // project instead of dying in scrollback. Best-effort — never blocks the
    // exit. trace:TASK-266 | ai:claude
    // STORY-301: stamp this member's terminal state — completed / failed plus
    // the PR a phase discovered — into the drain-state file. For a batch drain
    // this leaves a member-by-member trail; for a single drain the file is
    // cleared just below, so this update only matters mid-run.
    drain_state::set_member_outcome(
        &project_root,
        spec,
        result.exit_code == 0,
        auto_complete::PhaseDriver::hint_context(&driver).pr_number,
    );
    write_pipelined_child_result_sidecar(&result);

    record_auto_complete_run(
        &driver,
        &project_root,
        spec,
        variant,
        &result,
        started_at,
        completed_at,
        json,
        lifecycle_skip,
    );

    // STORY-301: a single-spec drain owns the file — clear it now the run is
    // over. A clean exit removes the file; only a crash leaves it behind for
    // `aida drain status` to flag as a stale drain. trace:STORY-301
    //
    // BUG-438: but a *resume* that FAILED at a phase must KEEP the drain-state,
    // or the operator can't `--resume-drain` again after fixing the blocker —
    // the checkpoint would be consumed on failure. A resume that succeeded
    // clears it (the work is done); a non-resume drain clears as before.
    // trace:BUG-438 | ai:claude
    if owns_drain_state {
        if should_clear_drain_state(owns_drain_state, resume.is_some(), result.exit_code) {
            let _ = drain_state::DrainState::clear(&project_root);
        } else if !json {
            eprintln!(
                "  {} keeping drain-state — fix the blocker, then re-run \
                 `aida queue work --auto-complete --resume-drain`",
                crate::glyph(crate::glyphs::Glyph::Info).cyan()
            );
        }
    }

    // trace:TASK-1184 | ai:codex
    let _ = agent_registry::gc_dead_agents(&project_root, false, None);

    result
}

// trace:BUG-2139 | ai:claude
#[cfg(test)]
mod bug_2139_headless_phase_seat_tests {
    use super::*;

    #[test]
    fn every_headless_phase_seat_is_preflighted() {
        assert_eq!(
            headless_phase_seats(auto_complete::NoHumanMode::Both),
            vec!["implementer", "reviewer"]
        );
        assert_eq!(
            headless_phase_seats(auto_complete::NoHumanMode::ReviewerOnly),
            vec!["reviewer"]
        );
    }

    /// A launcher that cannot delegate `reviewer` is refused before any phase
    /// runs; the implementer phase alone would have fallen back to baseline.
    #[test]
    fn a_drain_without_reviewer_delegation_is_refused_up_front() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut env = crate::test_env::EnvVarsGuard::snapshot(&[
            "AIDA_USER",
            crate::seat_authority::GRANT_ENV,
        ]);
        env.set_key("AIDA_USER", "bug-2139-drain-user");
        let narrow = crate::seat_authority::test_support::mint_grant_for(
            root,
            "bug-2139-drain-user",
            "advisor",
            &["implementer"],
        );
        env.set_key(crate::seat_authority::GRANT_ENV, &narrow);
        let err = preflight_headless_phase_seats(root, auto_complete::NoHumanMode::Both)
            .expect_err("the reviewer phase cannot be granted");
        assert!(
            format!("{err:#}").contains("headless `reviewer` phase"),
            "{err:#}"
        );
        let full = crate::seat_authority::test_support::mint_grant_for(
            root,
            "bug-2139-drain-user",
            "advisor",
            &["implementer", "reviewer"],
        );
        env.set_key(crate::seat_authority::GRANT_ENV, &full);
        preflight_headless_phase_seats(root, auto_complete::NoHumanMode::Both).unwrap();
    }
}
