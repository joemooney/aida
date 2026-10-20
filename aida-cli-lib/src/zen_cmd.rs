//! `aida zen` command cluster — `handle_zen_command`, extracted from
//! `lib.rs` (SPIKE-78 / STORY-771; pure movement, no behavior change).
//! The zen drive engine stays in `lib.rs`/`zen_drive.rs`.
// trace:STORY-771 | ai:claude

use crate::*;

// trace:STORY-1728 | ai:claude
#[path = "zen_cmd_args.rs"]
pub(crate) mod zen_cmd_args;
pub(crate) use zen_cmd_args::*;

/// `aida zen status` — print the corroborated zen context of the current
/// process. The bare status word (`zen` / `interactive`) goes to stdout so a
/// skill can branch on it cleanly; any informational note goes to stderr.
/// trace:BUG-237 | ai:claude
pub(crate) fn handle_zen_command(cmd: &ZenCommand) -> Result<()> {
    match cmd {
        ZenCommand::Status { json } => {
            // Resolve the shared `.aida/` root from any worktree so a child in
            // a sibling worktree reads the orchestrator's marker dir and the
            // canonical lease set. On a resolution failure the lookups simply
            // find nothing — the verdict fails safe to interactive.
            let project_root = find_main_worktree_root()
                .or_else(|_| std::env::current_dir())
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());
            let ctx = zen::detect(&project_root, &cwd);
            if *json {
                println!(
                    "{{\"context\":\"{}\",\"corroborated\":{},\"reason\":\"{}\"}}",
                    ctx.status_word(),
                    ctx.is_zen(),
                    ctx.reason_slug()
                );
            } else {
                println!("{}", ctx.status_word());
            }
            // The note is informational. For the no-provenance case it names
            // a real stale value and suggests `unset AIDA_ZEN`.
            if let Some(note) = ctx.informational_note() {
                eprintln!(
                    "  {} {}",
                    crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                    note.dimmed()
                );
            }
            Ok(())
        }
        // STORY-564: the clean-vs-human-needed finish decision for the
        // standalone `--zen` lane. The `/aida-pr` checkpoint branches off
        // the bare decision word: `auto-exit` → run `aida session end` and
        // exit; `pause` → render the grab-next/stop table. trace:STORY-564
        ZenCommand::Finish { json } => {
            let project_root = find_main_worktree_root()
                .or_else(|_| std::env::current_dir())
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());
            let is_zen = zen::detect(&project_root, &cwd).is_zen();
            let lease = active_lease_for_cwd(&project_root, &cwd);
            // Marker: the session flagged itself human-needed (paused on a
            // design-fork / raised a punt) via `aida zen needs-human`.
            let needs_human_marked = lease
                .as_ref()
                .map(|l| zen::has_needs_human_marker(&project_root, &l.id))
                .unwrap_or(false);
            // Punt: a substrate record of a human-needed fork raised during
            // this session — scoped to the lease's spec + start time so a
            // sibling session's punt sharing the ledger doesn't leak in.
            let has_open_punt = lease
                .as_ref()
                .map(|l| {
                    let spec = l.scope.trim().to_ascii_uppercase();
                    punt::read_ledger(&project_root)
                        .iter()
                        .any(|r| r.timestamp >= l.started_at && r.spec.to_ascii_uppercase() == spec)
                })
                .unwrap_or(false);
            let pause_always = zen_pause_always_in_force(&project_root);
            let presence_bias = if presence::read_presence_file().is_some() {
                match presence::current_presence(chrono::Utc::now()) {
                    presence::Presence::Away => zen::FinishPresence::Away,
                    presence::Presence::Home => zen::FinishPresence::Home,
                }
            } else {
                zen::FinishPresence::NoOpinion
            };
            let verdict = zen::classify_finish(
                is_zen,
                needs_human_marked,
                has_open_punt,
                pause_always,
                presence_bias,
            );
            if *json {
                println!(
                    "{{\"decision\":\"{}\",\"reason\":\"{}\",\"corroborated\":{}}}",
                    verdict.decision_word(),
                    verdict.reason_slug(),
                    is_zen
                );
            } else {
                println!("{}", verdict.decision_word());
            }
            // STORY-569: on a CLEAN finish — auto-exit, or a pause only
            // because the operator elected `--pause-always` — hand the
            // just-opened PR to the advisor through the agent mailbox.
            // This lives in the gate (not skill text) so the finish
            // sequence is finish → PR → review brief → exit by
            // construction, and the build→review handoff never falls back
            // to an operator relay. A needs-human / punt pause skips: the
            // operator is actively in the loop and the PR may still
            // change. Fail-open: a notify failure must never block the
            // finish decision. trace:STORY-569 | ai:claude
            let clean_finish = matches!(
                verdict,
                zen::ZenFinish::AutoExit | zen::ZenFinish::Pause(zen::FinishPause::PauseAlways)
            );
            if clean_finish {
                if let Some(l) = lease.as_ref() {
                    match file_zen_review_brief(&project_root, l) {
                        Ok(Some((agent, path))) => eprintln!(
                            "  {} review brief filed to the {} mailbox: {}",
                            crate::glyph(crate::glyphs::Glyph::Info).cyan(),
                            agent.cyan(),
                            path.display()
                        ),
                        Ok(None) => {}
                        Err(e) => eprintln!(
                            "  {} could not file the review brief ({e}) — \
                             hand the PR to your reviewer manually",
                            crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                        ),
                    }
                }
            }
            Ok(())
        }
        // STORY-564: record that this `--zen` session needed a human. Keyed
        // to the session lease so concurrent zen sessions don't collide.
        ZenCommand::NeedsHuman { reason } => {
            let project_root = find_main_worktree_root()
                .or_else(|_| std::env::current_dir())
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());
            match active_lease_for_cwd(&project_root, &cwd) {
                Some(lease) => {
                    zen::mark_needs_human(&project_root, &lease.id, reason)?;
                    println!(
                        "{} marked this --zen session human-needed — the finish checkpoint will pause",
                        crate::glyph(crate::glyphs::Glyph::Check).green()
                    );
                    Ok(())
                }
                None => {
                    // No lease over this cwd: nothing to scope the marker to.
                    // Don't fail — just tell the operator the gate can't see it.
                    eprintln!(
                        "  {} no active session lease covers this directory — `aida zen finish` \
                         keys the needs-human marker off the lease, so there is nothing to mark",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold()
                    );
                    Ok(())
                }
            }
        }
    }
}

/// One-shot AUTONOMOUS implement + ship from a spec OR a one-line thought.
/// Pass an approved spec id to drive it; pass free text to draft a spec from
/// the thought, file it, and drive that — thought to (gated) merged in one
/// line. Auto-queues the spec, then drives it through the SAME
/// `--auto-complete` orchestrator `aida burndown` uses (implement → CI →
/// review → merge → pull). FULLY HEADLESS by default — kick one off and
/// walk away; an INDEPENDENT reviewer always runs before the auto-merge.
/// Drives are SERIALIZED per repo: a global drain lock allows one drive at
/// a time, so a second `aida zen` while one is live is refused. To drive
/// several specs at once, use the `aida burndown` fan-out — one
/// orchestrator over worktree-isolated implementer subagents. Use
/// `--supervised` to drive the implementer yourself. The auto-implement
/// counterpart to `aida ship` (which finishes a HUMAN-implemented spec).
/// With no argument, prints help.
// trace:STORY-721 | ai:claude — plain `//` keeps the marker out of `--help`.
// trace:STORY-725 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "zen")]
pub(crate) struct ZenArgs {
    /// An approved SPEC id to drive, OR a free-text thought to draft
    /// into a spec and drive. Free text is drafted into a title +
    /// description + acceptance criteria, filed as a draft, then routed
    /// through the approve-gate before driving.
    #[clap(value_name = "SPEC")]
    pub spec: Option<String>,

    /// Supervised drive: YOU drive the implementer interactively, and the
    /// reviewer still runs headless as an independent gate before the
    /// auto-merge. The opt-in counterpart to the fire-and-forget default
    /// (which runs the implementer headless too). Maps to the
    /// orchestrator's `--no-human=reviewer-only`.
    #[clap(long, conflicts_with = "no_human")]
    pub supervised: bool,

    /// Explicit autonomy mode override, forwarded to the orchestrator. The
    /// reviewer ALWAYS runs headless as an independent gate; this only
    /// governs the implementer. The DEFAULT is `both` (fully headless
    /// fire-and-forget — kick one off and walk away; drives are serialized
    /// per repo, use `aida burndown` to drive several specs at once).
    /// `--no-human=reviewer-only` (= `--supervised`) keeps the implementer
    /// interactive. Aliases: `--unattended`, `--headless`.
    #[clap(
        long,
        value_name = "MODE",
        num_args = 0..=1,
        default_missing_value = "both",
        aliases = ["unattended", "headless"]
    )]
    pub no_human: Option<String>,

    /// Skip the post-merge `aida pull` (the Done → Completed auto-bump) —
    /// forwarded to the orchestrator.
    #[clap(long)]
    pub no_pull: bool,

    /// Override the soft suitability warnings (under-specified / coupled)
    /// and drive anyway. Does NOT override the hard refusals (epic,
    /// keystone, blocked).
    // trace:TASK-1037 | ai:claude
    #[clap(long)]
    pub force: bool,

    /// Own worktree + own PR: split this spec out of its scope (parent epic
    /// / active focus) instead of routing into the scope worktree. The
    /// explicit exception to the default-into-scope routing.
    // trace:ADR-6 | ai:claude
    #[clap(long, conflicts_with = "into_epic")]
    pub solo: bool,

    /// Force the cluster route: drive inside the scope (parent epic / focus)
    /// worktree even when the default would otherwise pick its own. The
    /// counterpart to `--solo`.
    // trace:ADR-6 | ai:claude
    #[clap(long)]
    pub into_epic: bool,

    /// Resolve + validate the spec and PRINT the plan WITHOUT driving it.
    #[clap(long)]
    pub dry_run: bool,

    /// Emit the drive-suitability GATE VERDICT as JSON and exit WITHOUT
    /// driving — the machine-readable pre-flight a shell-out consumer (the
    /// TUI drive verb) reads to surface a gate hold, and its clarify / force
    /// remedy, instead of a false "drive launched". Implies a dry run; needs
    /// a real SPEC id (not a free-text thought).
    // trace:STORY-744 | ai:claude
    #[clap(long)]
    pub json: bool,

    /// Which vendor CLI runs the headless implementer for this drive:
    /// `claude`, `codex`, or `agy` (alias: `--agent`, matching `aida agent
    /// new <vendor>`). A per-invocation override — no need to export
    /// `AIDA_HEADLESS_VENDOR` just to drive one spec on Codex. Resolution
    /// order: this flag > the `AIDA_HEADLESS_VENDOR` env > the
    /// `[orchestrator] headless_vendor` / `[agents] vendor` config knob >
    /// `claude` (default).
    // trace:TASK-1116 | ai:claude
    #[clap(long, visible_alias = "agent", value_name = "VENDOR")]
    pub vendor: Option<String>,

    /// 2-agent bake-off: fan claude + codex to INDEPENDENTLY implement this
    /// spec in separate worktrees (same acceptance, no shared context), run
    /// each candidate's CI (a failing candidate is eliminated), have ONE
    /// BLIND reviewer score the passers side-by-side (vendor identity
    /// stripped), merge the winner into the current branch, delete the
    /// loser branch, and record the outcome (a CompeteOutcome row in
    /// `.aida/events.jsonl` + a spec comment) for dispatch-policy data.
    /// Premium mode — 2x implement cost; best for hard/ambiguous specs.
    // trace:STORY-722 | ai:claude
    #[clap(
        long,
        conflicts_with_all = ["vendor", "supervised", "no_human", "solo", "into_epic", "json"]
    )]
    pub compete: bool,

    /// Inspect the corroborated zen context — whether `AIDA_ZEN=1` is backed
    /// by a verifiable provenance or is a stale / leaked value. A skill
    /// introspection hook, not a daily-driver.
    // trace:BUG-237 | ai:claude
    #[clap(subcommand)]
    pub command: Option<crate::cli::ZenCommand>,
}

fn run_zen(
    matches: &clap::ArgMatches,
    ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<crate::command_registry::Flow> {
    use crate::*;
    let args = <ZenArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => {
            // BUG-237: zen-context introspection. Like `orchestrator status`, reads
            // only env vars + the drain-state file + session leases — no requirement
            // store. The `aida zen <spec>` autonomous-drive form (STORY-721) needs the
            // store to validate the spec's status, so only the introspection
            // subcommands short-circuit here; the spec-drive path falls through to the
            // post-storage dispatch. trace:BUG-237 trace:TASK-336 trace:STORY-721 | ai:claude
            if let ZenArgs {
                command: Some(zen_cmd),
                ..
            } = &args
            {
                return crate::command_registry::returned(handle_zen_command(zen_cmd));
            }
            // STORY-722: `aida zen <spec> --compete` — the 2-agent bake-off. Like
            // `aida compete` it self-loads the store (to resolve the spec + assemble
            // the brief) and orchestrates worktrees + headless vendor runs, so it
            // dispatches early and works identically on both storage backends.
            // trace:STORY-722 | ai:claude
            if let ZenArgs {
                spec,
                compete: true,
                dry_run,
                command: None,
                ..
            } = &args
            {
                let Some(spec) = spec.as_deref() else {
                    anyhow::bail!(
                        "`aida zen --compete` needs a SPEC id (e.g. `aida zen TASK-123 --compete`)"
                    );
                };
                return crate::command_registry::returned(compete_cmd::handle_zen_compete(
                    spec, *dry_run,
                ));
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Distributed { .. }) => {
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                // STORY-721: `aida zen <spec>` autonomous implement+ship drive. The
                // introspection subcommands (`zen status` / `finish` / `needs-human`)
                // short-circuit before storage init in `run()`; this arm only sees the
                // spec-drive form (`command: None`), which needs the store to validate
                // the spec's status before driving it. trace:STORY-721 | ai:claude
                ZenArgs {
                    spec,
                    supervised,
                    no_human,
                    no_pull,
                    force,
                    solo,
                    into_epic,
                    dry_run,
                    json,
                    vendor,
                    // trace:STORY-722 — the --compete form dispatches before storage init.
                    compete: _,
                    command: _,
                } => {
                    let storage = Storage::new(store_path.to_path_buf());
                    // STORY-744: the machine-readable gate probe short-circuits the
                    // drive — resolve + classify the suitability gate and print the
                    // verdict as JSON so a shell-out consumer (the TUI drive verb) reads
                    // the SAME gate the drive runs instead of parsing human prose.
                    if *json {
                        return crate::command_registry::returned(run_zen_gate_json(
                            &storage,
                            spec.as_deref(),
                        ));
                    }
                    // TASK-1116: a per-invocation `--vendor`/`--agent` picks the headless
                    // implementer for this drive (top precedence). Installed before the
                    // self-invoked `aida queue work --auto-complete` child spawns so it
                    // inherits the choice via `AIDA_HEADLESS_VENDOR`.
                    apply_drive_vendor_override(vendor.as_deref())?;
                    let user_id = current_user_id(None);
                    return crate::command_registry::returned(run_zen_drive(
                        &storage,
                        Some(store_path),
                        &user_id,
                        spec.as_deref(),
                        no_human.as_deref(),
                        *supervised,
                        *no_pull,
                        *force,
                        *solo,
                        *into_epic,
                        *dry_run,
                    ));
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
        Some(crate::command_registry::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                // STORY-721: `aida zen <spec>` autonomous implement+ship drive on the
                // legacy (centralized) storage path. The introspection subcommands
                // short-circuit before storage init; this arm only sees the spec-drive
                // form. trace:STORY-721 | ai:claude
                ZenArgs {
                    spec,
                    supervised,
                    no_human,
                    no_pull,
                    force,
                    solo,
                    into_epic,
                    dry_run,
                    json,
                    vendor,
                    // trace:STORY-722 — the --compete form dispatches before storage init.
                    compete: _,
                    command: _,
                } => {
                    // STORY-744: the machine-readable gate probe short-circuits the
                    // drive entirely — it resolves + classifies and prints the verdict.
                    if *json {
                        run_zen_gate_json(&storage, spec.as_deref())?;
                    } else {
                        // TASK-1116: a per-invocation `--vendor`/`--agent` picks the
                        // headless implementer for this drive (top precedence).
                        apply_drive_vendor_override(vendor.as_deref())?;
                        let user_id = current_user_id(None);
                        run_zen_drive(
                            &storage,
                            None, // legacy storage path: no git backend for auto-approve
                            &user_id,
                            spec.as_deref(),
                            no_human.as_deref(),
                            *supervised,
                            *no_pull,
                            *force,
                            *solo,
                            *into_epic,
                            *dry_run,
                        )?;
                    }
                }
            }
            Ok(crate::command_registry::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static ZEN: crate::command_registry::CommandReg = crate::command_registry::CommandReg {
    name: "zen",
    help_after: "orchestrator",
    command: <ZenArgs as clap::CommandFactory>::command,
    policy: |_| crate::command_registry::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::BuildLoop,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::command_registry::Handler::Store(run_zen),
};
