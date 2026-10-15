//! Drafted-thought preview, `aida do`/zen drive loop and zen auto-approve (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// Extract the acceptance-criteria body from a requirement description.
/// Returns the body text (without the heading line) when one of the
/// recognized headings is found; None otherwise so the caller can render
/// a "no acceptance criteria documented" placeholder instead of silently
/// emitting an empty section. trace:STORY-67 | ai:claude
pub(crate) fn extract_acceptance_section(description: &str) -> Option<String> {
    let lines: Vec<&str> = description.lines().collect();
    let mut start: Option<usize> = None;
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i].trim();
        if let Some(rest) = line.strip_prefix("## ") {
            let lower = rest.trim().to_ascii_lowercase();
            if ACCEPTANCE_SECTION_HEADINGS
                .iter()
                .any(|h| lower.starts_with(&h.to_ascii_lowercase()))
            {
                start = Some(i + 1);
                break;
            }
        }
        i += 1;
    }
    let start = start?;
    let mut end = lines.len();
    for (j, line) in lines.iter().enumerate().skip(start) {
        if line.trim_start().starts_with("## ") {
            end = j;
            break;
        }
    }
    let body = lines[start..end].join("\n").trim().to_string();
    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}

/// The description prose that precedes the `## Acceptance` section — the lead
/// paragraphs to show under the spec's title. For a fallback (no-AI) draft there
/// is no acceptance heading, so the whole body is the lead. Sibling of
/// [`extract_acceptance_section`].
// trace:STORY-736 | ai:claude
pub(crate) fn drafted_lead_prose(description: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in description.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("## ") {
            let lower = rest.trim().to_ascii_lowercase();
            if ACCEPTANCE_SECTION_HEADINGS
                .iter()
                .any(|h| lower.starts_with(&h.to_ascii_lowercase()))
            {
                break;
            }
        }
        out.push(line);
    }
    out.join("\n").trim().to_string()
}

/// The acceptance bullets of a drafted body, as clean strings (leading `- `/`* `
/// markers stripped). Reuses [`extract_acceptance_section`], then keeps only the
/// genuine bullet lines — so the provenance footer line (`_Drafted by …_`) that
/// follows the bullets in a composed body is filtered out.
// trace:STORY-736 | ai:claude
pub(crate) fn drafted_acceptance_bullets(description: &str) -> Vec<String> {
    let Some(section) = extract_acceptance_section(description) else {
        return Vec::new();
    };
    section
        .lines()
        .map(str::trim)
        .filter_map(|l| l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Render the SAFE `aida zen "<thought>" --dry-run` preview: compose the SAME
/// draft the real run would file (genuine AI draft, with the offline fallback)
/// and pretty-print it in the `aida show` idiom — title + description +
/// acceptance — so the operator sees the structured spec a sentence becomes,
/// not a flat echo. Composes + renders only; nothing is persisted.
// trace:STORY-736 | ai:claude
pub(crate) fn render_zen_dry_run_draft(thought: &str) -> String {
    let ai = zen_try_ai_draft(thought);
    let drafted = zen_drive::compose_draft_from_thought(thought, ai);
    render_drafted_thought_preview(&drafted)
}

/// Pure renderer for a composed [`zen_drive::DraftedThought`] — split from the AI
/// wiring above so the layout is unit-testable without the transport. Mirrors the
/// `aida show` card: a clean rule, the title under a status glyph, the lead
/// description prose, an indented acceptance block, a source note, and the
/// lifecycle line. All glyphs route through the registry (ascii-safe).
// trace:STORY-736 | ai:claude
pub(crate) fn render_drafted_thought_preview(drafted: &zen_drive::DraftedThought) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let rule = "─".repeat(60);
    let arrow = crate::glyph(crate::glyphs::Glyph::Arrow);
    let pending = crate::glyph(crate::glyphs::Glyph::Pending);
    let bullet = crate::glyph(crate::glyphs::Glyph::Bullet);
    let check = crate::glyph(crate::glyphs::Glyph::Check);

    let _ = writeln!(out, "{}", rule.dimmed());
    let _ = writeln!(
        out,
        "{} would draft + file + drive a new {} from your thought:",
        arrow.cyan(),
        "Draft".yellow()
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "  {} {}", pending.yellow(), drafted.title.bold());
    let _ = writeln!(out);

    let lead = drafted_lead_prose(&drafted.description);
    if !lead.is_empty() {
        let _ = writeln!(out, "  {}", format!("{arrow} Description:").bold());
        for line in lead.lines() {
            let _ = writeln!(out, "    {line}");
        }
        let _ = writeln!(out);
    }

    let criteria = drafted_acceptance_bullets(&drafted.description);
    if !criteria.is_empty() {
        let label = if criteria.len() == 1 {
            "1 item".to_string()
        } else {
            format!("{} items", criteria.len())
        };
        let _ = writeln!(out, "  {}", format!("{arrow} Acceptance ({label}):").bold());
        for c in &criteria {
            let _ = writeln!(out, "    {bullet} {c}");
        }
        let _ = writeln!(out);
    }

    let note = match drafted.source {
        zen_drive::DraftSource::Ai => "AI-drafted from your thought.".to_string(),
        zen_drive::DraftSource::Fallback => {
            "no AI was reachable — showing the offline-fallback draft (refine the \
             acceptance criteria before it ships)."
                .to_string()
        }
    };
    let _ = writeln!(out, "  {} {}", check.green(), note.dimmed());
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  …then: approve {arrow} implement {arrow} CI {arrow} review {arrow} merge, \
         fully headless. Run without --dry-run to drive it."
    );
    let _ = writeln!(out, "{}", rule.dimmed());
    out
}

/// Pull `(REQ-ID)` trailers from a commit message **subject** only. AIDA's
/// commit format wraps the requirement id in parens at end-of-subject:
/// `[AI:tool] feat(scope): description (REQ-ID)`. Also tolerates the
/// shorter form without `[AI:tool]` (chores/docs).
///
/// Body content is ignored — those IDs are "referenced" (trace comments,
/// prose, sub-commit lines in squash bodies) and are returned by
/// [`extract_referenced_spec_ids_from_commit`] instead. trace:BUG-85 | ai:claude
/// trace:STORY-67 | ai:claude
/// True when a commit subject is an AIDA *plan* commit — `docs(plans): …`
/// or `docs(plan): …`, optionally behind an `[AI:tool]` authorship prefix
/// (`[AI:claude] docs(plans): …`). Plan commits are pre-implementation
/// artifacts (the plan template leaves the planned specs at Approved), so
/// their trailing `(SPEC-ID …)` group names what the plan is FOR, not what
/// shipped. The auto-bump completion-candidate scan skips them so a plan-only
/// commit never false-completes the specs it plans. trace:BUG-426 | ai:claude
pub(crate) fn is_plan_commit_subject(subject: &str) -> bool {
    let s = subject.trim();
    // Strip an optional leading `[AI:tool]` authorship tag.
    let s = s
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
        .map(|(_, after)| after.trim_start())
        .unwrap_or(s);
    let lower = s.to_ascii_lowercase();
    lower.starts_with("docs(plans)") || lower.starts_with("docs(plan)")
}

pub(crate) fn extract_spec_ids_from_commit(message: &str) -> Vec<String> {
    let subject = message.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let mut out = Vec::new();
    push_paren_spec_ids_from_line(subject, &mut out);
    // BUG-270: also recognize a leading `SPEC-ID:` prefix, e.g.
    // "STORY-439: three-way complexity calibration substrate (#270)". Some
    // merge commits (external planner output, hand-authored squashes) put the
    // spec id at the FRONT with a colon instead of the AIDA-convention
    // trailing `(REQ-ID)`. Without this, such a spec is never auto-bumped /
    // reconciled and strands at its pre-merge status. The `looks_like_spec_id`
    // guard keeps conventional-commit heads (`fix:`, `feat(scope):`,
    // `[AI:claude] ...`) from false-matching — none of them parse as a bare
    // `<ALPHA>-<DIGITS...>` token. trace:BUG-270 | ai:claude
    if let Some((head, _)) = subject.split_once(':') {
        let head = head.trim();
        if looks_like_spec_id(head) && !out.iter().any(|x| x.eq_ignore_ascii_case(head)) {
            out.push(head.to_string());
        }
    }
    out
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
