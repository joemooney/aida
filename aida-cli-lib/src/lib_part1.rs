pub(crate) fn glyph(g: crate::glyphs::Glyph) -> &'static str {
    crate::glyphs::get(g, crate::find_project_root().ok().as_deref())
}

use aida_core::{
    check_migration_status,
    check_scaffold_status,
    determine_requirements_path,
    forbidden_attention_transition,
    merge_agents_md_aida_block,
    seed_meta_requirements,
    // trace:TASK-331 — shared atomic-write util, promoted from BUG-228's local copy
    write_atomic,
    AgentsMdBlockMerge,
    AttentionReason,
    Comment,
    DatabaseBackend,
    FieldChange,
    FileStatus,
    // GitLab integration
    GitLabClient,
    GitLabConfig,
    IdFormat,
    IssueFilter,
    IssueState,
    MigrationCheck,
    NumberingStrategy,
    RelationshipType,
    Requirement,
    RequirementPriority,
    RequirementStatus,
    RequirementType,
    RequirementsStore,
    ScaffoldConfig,
    Scaffolder,
    Storage,
};

use crate::cli::{
    AdvisorCommand, AgentCommand, AgentNewCommand, ApprovalCommand, BacklogCommand, BlockCommand,
    BriefCommand, CacheCommand, Cli, Command, CommentCommand, ConfigCommand, DbCommand,
    DepsCommand, DevCommand, DrainCommand, FindingsCommand, FocusCommand, GitHubCommand,
    GitLabCommand, GlyphCommand, GraphCommand, HeadlessCommand, HistoryCommand, IdentityCommand,
    JiraCommand, LoadCommand, MailboxCommand, McpCommand, MemoriesCommand, NodeCommand,
    OrchestratorCommand, OutputFormat, PlanCommand, PrCommand, PuntsCommand, QuestionsCommand,
    QueueCommand, RelationshipCommand, ReleaseCommand, ReviewCommand, RoleCommand,
    RolePromptCommand, RoleScopeCommand, ScaffoldCommand, SessionCommand, SessionManifestCommand,
    SkillCommand, SoloAction, SpecCommand, StackCommand, TeamCommand, TerminalCommand,
    TraceCommand, UpgradeCommand, UsageCommand, WorkerCommand, WorktreeCommand,
    WorktreePoolCommand, ZenCommand,
};

/// BUG-99: restore SIGPIPE's default behavior so `aida ... | head -N` exits
/// cleanly (status 141) instead of triggering Rust's "failed printing to
/// stdout: Broken pipe" panic. Rust deliberately ignores SIGPIPE by default
/// so library code can decide how to handle it, but for a Unix CLI we want
/// the classic "downstream closed, terminate quietly" semantics. Windows
/// has no SIGPIPE; this is a no-op there. trace:BUG-99 | ai:claude
#[cfg(unix)]
pub(crate) fn install_sigpipe_handler() {
    // Safety: signal() with SIG_DFL is async-signal-safe and is the
    // documented way to restore default disposition. Running here before
    // any output happens means the default kicks in for every println!
    // / eprintln! / write! in the binary.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
pub(crate) fn install_sigpipe_handler() {}

/// STORY-737 (delight #5): a sentinel error meaning "a soft, non-error signpost
/// was already rendered to stderr by the command; the top-level handler must NOT
/// re-print it as a red `Error:`". An empty queue on day one is the EXPECTED
/// state, not a failure — a brand-new user running `aida queue work` should get
/// a forward-pointing nudge, not a red error. The exit code still goes non-zero
/// (scripts that gate on it keep working); only the human-facing RENDER is
/// softened. The signpost itself is emitted at the call site (with the project's
/// info glyph) so this carries no message of its own.
// trace:STORY-737 | ai:claude
#[derive(Debug)]
pub(crate) struct SoftSignpostShown;

impl std::fmt::Display for SoftSignpostShown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Kept terse: only ever surfaces if some future caller wraps it in
        // context (the human render path suppresses it entirely).
        f.write_str("queue is empty")
    }
}

impl std::error::Error for SoftSignpostShown {}

/// BUG-1745: the command already wrote its COMPLETE typed document (JSON/TOON)
/// to stdout with the failure reason inside it. The global renderer must not
/// append a second document to stdout, and must not print a human `Error:` —
/// the payload already carries the reason. Exit code only.
// trace:BUG-1745 | ai:codex
#[derive(Debug)]
pub(crate) struct TypedPayloadEmitted;

impl std::fmt::Display for TypedPayloadEmitted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("typed payload already emitted")
    }
}

impl std::error::Error for TypedPayloadEmitted {}

/// STORY-737 (delight #4): should `aida history` hide the stateless internal
/// META prompt-template rows? Yes by default — they're plumbing seeded by
/// `aida init`, not user-authored work, and drown a fresh project's one real
/// spec (matching `aida list` and `aida status`, which already exclude them).
/// An explicit `--include-meta` OR `--type meta` overrides the hide so META
/// stays reachable.
// trace:STORY-737 | ai:claude
pub(crate) fn history_should_exclude_meta(include_meta: bool, type_filter: Option<&str>) -> bool {
    if include_meta {
        return false;
    }
    let asked_for_meta = type_filter
        .map(|t| t.eq_ignore_ascii_case("meta"))
        .unwrap_or(false);
    !asked_for_meta
}

// trace:BUG-1745 | ai:codex
#[cfg(test)]
#[path = "tests/bug_1745_error_channel_tests.rs"]
mod bug_1745_error_channel_tests;
#[cfg(test)]
#[path = "tests/bug_1749_doctor_toon_tests.rs"]
mod bug_1749_doctor_toon_tests;
#[cfg(test)]
#[path = "tests/story_737_delight_tests.rs"]
mod story_737_delight_tests;
// trace:TASK-1555 | ai:claude
#[cfg(test)]
#[path = "tests/task_1555_ci_gate_tiers_tests.rs"]
mod task_1555_ci_gate_tiers_tests;
// trace:TASK-2238 | ai:claude
#[cfg(test)]
#[path = "tests/task_2238_review_sweep_tests.rs"]
mod task_2238_review_sweep_tests;
// trace:TASK-1562 | ai:claude
#[cfg(test)]
#[path = "tests/task_1562_worktree_reclaim_tests.rs"]
mod task_1562_worktree_reclaim_tests;
// trace:BUG-1723 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1723_config_trust_tests.rs"]
mod bug_1723_config_trust_tests;

/// The commands whose store and cache lock waits run under the fail-fast
/// deadline ([`aida_core::io_pressure::FAIL_FAST_LOCK_BUDGET`]).
// trace:BUG-HOME-041 | ai:claude
fn fail_fast_lock_command(name: &str, subcommand: Option<&str>) -> bool {
    matches!(
        (name, subcommand),
        ("rel" | "relationship", Some("add")) | ("pr", Some("auto-queue-review"))
    )
}

#[cfg(test)]
mod bug_home_041_fail_fast_command_tests {
    // trace:BUG-HOME-041 | ai:claude
    #[test]
    fn only_rel_add_and_auto_queue_review_fail_fast() {
        assert!(super::fail_fast_lock_command("rel", Some("add")));
        assert!(super::fail_fast_lock_command(
            "pr",
            Some("auto-queue-review")
        ));
        assert!(!super::fail_fast_lock_command("rel", Some("remove")));
        assert!(!super::fail_fast_lock_command("pr", None));
        assert!(!super::fail_fast_lock_command("add", None));
        // The parsed name and subcommand are what the gate keys on.
        for (argv, expected) in [
            (
                &["aida", "rel", "add", "A", "B", "--type", "implements"][..],
                true,
            ),
            (
                &["aida", "pr", "auto-queue-review", "--branch", "b"][..],
                true,
            ),
            (
                &["aida", "rel", "remove", "A", "B", "--type", "implements"][..],
                false,
            ),
        ] {
            let cli = crate::command_registry::try_parse_from(argv).unwrap();
            assert_eq!(
                super::fail_fast_lock_command(cli.command.name(), cli.command.subcommand_name()),
                expected,
                "{argv:?}"
            );
        }
    }
}

/// The whole CLI: sigpipe setup, telemetry wrapping, error rendering,
/// dispatch. The `aida` binary is a stub that calls this — keeping the
/// entry point in the lib means only this one symbol is `pub` and every
/// `pub(crate)` item stays crate-private.
// trace:STORY-772 trace:ADR-16 | ai:claude
pub fn main_entry() {
    // BUG-2221: anchor the foreground-scheduling window (the show negative
    // probe derives its batch budget from process entry + the unchanged
    // 1000 ms objective). trace:BUG-2221 | ai:claude
    crate::show_negative::note_process_entry();
    // trace:BUG-1960 | ai:claude
    // First, while still single-threaded: take the wrapper's init side-channel
    // path out of the environment so no child process inherits it.
    crate::shell_eval::take_init_cd_file_from_env();
    install_sigpipe_handler();
    // trace:STORY-B6B1278890774EF993D2EBC327862470-097 | ai:claude
    crate::store_local::install_store_push_gate();
    // One colour decision for the whole process: apply no-color.org's rule
    // for an empty `NO_COLOR` to the `colored` crate before any thread starts.
    // trace:BUG-1979 | ai:claude
    crate::markdown_render::apply_no_color_policy();
    // trace:BUG-766 | ai:claude
    // Export this binary's identity (AIDA_BIN + a PATH prepend of its
    // directory) into the process env BEFORE any command runs, so every
    // child this process spawns — claude/codex drive and drain sessions,
    // shells they open, nested `aida` invocations — resolves a bare `aida`
    // to THIS coordinating build instead of whatever stale installed binary
    // the raw non-interactive PATH finds first. A months-old installed
    // binary picked up that way ran unguarded bulk store saves (the writer
    // behind the deferred-shelf wipes); making the coordinating build win
    // PATH resolution closes that seam for every spawn site at once.
    export_coordinating_bin_env();
    register_filing_identity();
    // trace:STORY-1843 | ai:claude
    register_source_build();
    // STORY-122: per-invocation telemetry. Wraps run() so every CLI
    // entry point gets recorded with cmd shape + duration + exit code.
    // Local-only; opt-out via `[telemetry] enabled = false` or
    // `AIDA_TELEMETRY=0`. Recording is best-effort and never aborts the
    // foreground command. trace:STORY-122 | ai:claude
    let argv: Vec<String> = std::env::args().collect();
    let started = std::time::Instant::now();

    // TASK-69: render anyhow-propagated errors in red instead of anyhow's
    // default plain-text formatter. Centralizes the coloring so every
    // `bail!` / `?` / `anyhow!` site automatically gets the highlight
    // without per-site refactoring. Exit code 1 on error, 0 on success.
    // trace:TASK-69 | ai:claude
    // trace:TASK-1526 | ai:codex
    let cache_scope = aida_core::db::cache_refresh::CacheReadScope::new();
    let exit_code: i32 = match run() {
        Ok(()) => 0,
        Err(err) if err.is::<aida_core::db::cache_refresh::AdvisoryCacheUnavailable>() => 0,
        Err(err) => {
            // trace:BUG-1940 | ai:claude
            let err = crate::cache_cmd::with_cache_corruption_hint(err);
            record_ambiguous_id_refusal(&err);
            let msg = format!("{:?}", err);
            // trace:BUG-1629 | ai:claude
            record_orchestrated_child_refusal(&msg);
            // TASK-972 (AXI #6): agents read STDOUT. An error printed to stderr
            // with a human `Error:` prefix is invisible to the agent loop,
            // forcing blind retries. In AGENT MODE emit the error as a
            // structured TOON `error:`/`help:` block on STDOUT instead, with an
            // actionable next-command suggestion where the message embeds one.
            // The exit code is unchanged (this only moves the OUTPUT channel),
            // and the human-at-a-TTY path below is byte-identical to before.
            // trace:TASK-972
            if err.downcast_ref::<TypedPayloadEmitted>().is_some() {
                // Print nothing on either channel. trace:BUG-1745 | ai:codex
            } else if agent_output_mode() {
                println!("{}", agent_error_block(&err, &msg));
            } else if err.downcast_ref::<SoftSignpostShown>().is_some() {
                // STORY-737 (delight #5): the command already rendered a soft,
                // forward-pointing signpost to stderr — re-printing it as a red
                // `Error:` would undo the whole point. Exit non-zero silently.
                // trace:STORY-737 | ai:claude
            } else {
                // Anyhow's Debug format prints the chain as
                //     summary
                //     \n
                //     Caused by:\n    inner1\n    inner2
                // Color the first non-empty line bold-red; dim the chain so
                // root summary stands out from causal background.
                let mut lines = msg.lines();
                if let Some(first) = lines.next() {
                    eprintln!("{} {}", "Error:".red().bold(), first.red());
                }
                for rest in lines {
                    eprintln!("{}", rest.dimmed());
                }
                // TASK-1082: when the error is a "Requirement not found" near-miss
                // of a real spec id, add a `did you mean <ID>?` line — the same
                // affordance clap gives for mistyped subcommands. Best-effort and
                // only on this branch; the exit code stays non-zero.
                // trace:TASK-1082 | ai:claude
                if let Some(hint) = did_you_mean_for_not_found(&msg) {
                    eprintln!("  {}", hint.dimmed());
                }
            }
            // BUG-1295: `aida pr rebase` bail sites that must be
            // distinguishable to a subprocess caller (conflict vs.
            // refused-force-push) wrap themselves in a `RebaseFailureExit`
            // instead of a bare `anyhow::bail!`. The printed message above is
            // unchanged either way — only the exit code this process returns
            // to its parent (the orchestrator's `attempt_phase3_auto_rebase`)
            // differs, so the parent can classify by exit code instead of by
            // matching this process's prose. Every other error keeps exit 1.
            // trace:BUG-1295 | ai:claude
            exit_code_for_error(&err)
        }
    };

    // BUG-b6b1278890774ef993d2ebc327862470-008: everything after `run()`
    // returns — the cache footer, the project-root walk and the telemetry
    // config read below — is the `exit` phase. trace:BUG-b6b1278890774ef993d2ebc327862470-008 | ai:claude
    let exit_phase = usage::phase("exit");
    // trace:TASK-1526 | ai:codex
    cache_output::finish(&cache_scope, exit_code == 0, &argv);

    // STORY-122: append the usage record after the command completed
    // so we capture exit_code / duration_ms. Read the env-side opt-out
    // first; checking the project's `.aida/config.toml` requires a
    // project root, which not every invocation has, so fall back to
    // "no project context" gracefully.
    let project_root = find_main_worktree_root().ok();
    if usage::is_enabled(project_root.as_deref()) {
        drop(exit_phase);
        let duration_ms = started.elapsed().as_millis() as u64;
        let ev = usage::UsageEvent {
            ts: chrono::Utc::now().to_rfc3339(),
            cmd: usage::derive_cmd_shape(&argv),
            args_count: usage::count_args(&argv),
            exit_code,
            duration_ms,
            binary_sha: build_sha_short(),
            role: std::env::var("AIDA_SESSION_ROLE")
                .ok()
                .filter(|s| !s.is_empty()),
            scope: std::env::var("AIDA_SESSION_SCOPE")
                .ok()
                .filter(|s| !s.is_empty()),
            // BUG-1600: for `schedule tick` only, record which driver
            // invoked it (hook / cron / manual) alongside the exit_code and
            // duration_ms already captured above — a config error (e.g. an
            // unsupported flag on an installed cron entry) is now visible
            // in scheduler telemetry, not just as silent repeated failures.
            // trace:BUG-1600 | ai:claude
            schedule_source: usage::schedule_invocation_source(&argv),
            // trace:BUG-1993 | ai:claude
            // trace:BUG-b6b1278890774ef993d2ebc327862470-008 | ai:claude
            phases: usage::with_residual(usage::take_phases(), duration_ms),
        };
        usage::append_event(&ev);
    }

    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

// SPIKE-70: `--sequential` names + guards the ordered, per-member-PR SHAPE of the
// batch drain (`auto_complete::drain_batch*`) rather than introducing a parallel
// knob. TASK-185: it does NOT pin concurrency — STORY-1091 made the batch drain
// honour `[drain] pipeline_depth`, so the one-member-at-a-time property comes from
// `drain_state::default_pipeline_depth()` (1), not from this flag. The old
// `SEQUENTIAL_DRAIN_CONCURRENCY` const asserted the pinned-to-1 invariant and was
// removed with the claim; read the default depth instead.
// trace:TASK-1005 trace:TASK-185 | ai:claude

/// STORY-764: pure core of the agent-mode decision with the explicit `--format`
/// pin layered on top of the historical `AIDA_AGENT_OUTPUT`/TTY logic. An
/// explicit pin wins outright (`human` → human path, `toon`/`json` → agent
/// path); with no pin the behavior is byte-identical to [`agent_output_mode_from`].
/// Testable without touching the real env or a real terminal.
// trace:STORY-764 | ai:claude
pub(crate) fn resolve_agent_mode(
    pin: Option<OutputFormat>,
    agent_env: Option<&str>,
    stdout_is_tty: bool,
) -> bool {
    if let Some(fmt) = pin {
        return !matches!(fmt, OutputFormat::Human);
    }
    agent_output_mode_from(agent_env, stdout_is_tty)
}

/// Resolve the selected clap leaf and reject a JSON format pin unless that
/// leaf advertises a dedicated JSON projection. `--format` is global, so clap
/// otherwise accepts it for every command and lets unsupported handlers emit
/// successful human/TOON text. A local `--json` argument is the executable
/// declaration that the leaf owns a machine contract; the renderer must then
/// make both spellings identical.
// trace:BUG-1502 | ai:codex
pub(crate) fn enforce_json_format_capability(argv: &mut Vec<String>) -> Result<()> {
    if !output_format_is_json() {
        return Ok(());
    }
    json_capability_gate(argv)
}

/// [`enforce_json_format_capability`] once JSON has been requested.
pub(crate) fn json_capability_gate(argv: &mut Vec<String>) -> Result<()> {
    let root = crate::command_registry::built_command();
    let matches = root.clone().try_get_matches_from(argv.clone())?;
    let mut command = &root;
    let mut selected = &matches;
    let mut path = vec!["aida".to_string()];
    // BUG-1631: a `global = true` `--json` on an ancestor (e.g. `aida
    // history`) also covers its subcommands (`aida history events`).
    // trace:BUG-1631 | ai:claude
    let mut inherited_json = false;
    while let Some((name, submatches)) = selected.subcommand() {
        inherited_json |= command
            .get_arguments()
            .any(|argument| argument.get_id().as_str() == "json" && argument.is_global_set());
        let Some(subcommand) = command
            .get_subcommands()
            .find(|candidate| candidate.get_name() == name)
        else {
            anyhow::bail!(
                "could not resolve JSON capability for command `{}`",
                path.join(" ")
            );
        };
        path.push(name.to_string());
        command = subcommand;
        selected = submatches;
    }

    let supports_json = inherited_json
        || command
            .get_arguments()
            .any(|argument| argument.get_id().as_str() == "json");
    if !supports_json {
        // The old spelling of a renamed local format option (`export
        // --format json`) names that option, not just `--format human|toon`.
        // trace:BUG-2189 | ai:claude
        if selected.value_source("format") == Some(clap::parser::ValueSource::CommandLine) {
            if let Some(error) =
                crate::command_registry::renamed_format_error(&path[1..].join(" "), "json")
            {
                return Err(error);
            }
        }
        anyhow::bail!(
            "`{} --format json` is unsupported: this command has no JSON projection; use `--format human` or `--format toon`",
            path.join(" ")
        );
    }

    // Materialize the global spelling as the selected leaf's dedicated flag.
    // This is the load-bearing dispatch guarantee: every existing handler sees
    // exactly the same parsed boolean for `--format json` as for `--json`, so
    // capability cannot drift from renderer wiring (for example, fasttrack
    // status previously declared `--json` but only inspected that local bool).
    // trace:BUG-1502 | ai:codex
    if !selected.get_flag("json") {
        argv.push("--json".to_string());
    }
    Ok(())
}

/// True when there is no human at the keyboard to answer a `Type 'y' to
/// confirm:` prompt — agent output mode is forced, or stdin is not a TTY. In
/// this state reading the prompt only hits EOF: the answer is never 'y', so the
/// historical `if !yes { read_line }` gate CANCELLED the write while an agent
/// capturing stdout (the "Cancelled" notice went to stderr) believed it
/// succeeded — a silent no-op that derails the implement -> done -> merge chain.
/// Callers must branch on this up front: AUTO-CONFIRM a reversible write (the
/// agent explicitly invoked the verb), or FAIL LOUDLY on a destructive one.
/// Never silently cancel a non-TTY write.
// trace:BUG-671
pub(crate) fn non_interactive_confirm() -> bool {
    non_interactive_confirm_from(agent_output_mode(), std::io::stdin().is_terminal())
}

/// Pure core of [`non_interactive_confirm`] (testable without a real terminal).
/// `agent_mode` is [`agent_output_mode`]'s verdict; `stdin_is_tty` is whether
/// stdin is a terminal.
// trace:BUG-671
pub(crate) fn non_interactive_confirm_from(agent_mode: bool, stdin_is_tty: bool) -> bool {
    agent_mode || !stdin_is_tty
}

/// BUG-721: gate for the interactive `aida review <spec>` verb's reviewer
/// launch. The verb spawns a `claude -p` reviewer subprocess against the live
/// repo and then drives a follow-up menu — it needs a human at the terminal to
/// watch that reviewer and answer. Returns true (may launch) only when BOTH
/// stdin and stdout are TTYs. When it returns false the verb must NOT spawn a
/// reviewer: a non-interactive `aida review` that silently launched a blind
/// headless reviewer the caller could neither see nor interrupt is exactly the
/// bug. This gate covers ONLY the interactive verb — the orchestrator's drain
/// review phase never reaches this code (it shells out to `aida queue work
/// PR-N` with `AIDA_AUTO_COMPLETE=1` and launches its headless reviewer there
/// on purpose, `run_reviewer`), so the autonomous drain is untouched.
// trace:BUG-721
pub(crate) fn review_may_launch_reviewer(stdin_is_tty: bool, stdout_is_tty: bool) -> bool {
    stdin_is_tty && stdout_is_tty
}

/// TASK-972 (AXI #6): split a formatted anyhow error into the one-line summary
/// the agent error block shows plus an optional suggested next command. The
/// summary is the first line with any leading human `Error:` prefix stripped
/// (several `bail!` sites embed one, which would otherwise read `error: Error:
/// …` once nested under the TOON `error:` key). The suggestion reuses the rich
/// hints the not_found module and friends already embed — see
/// [`extract_aida_suggestion`] — so "where known" comes for free.
// trace:TASK-972
/// BUG-684: the soft-signpost line for a truly-EMPTY `aida list` (human path).
/// Returns `Some(line)` only when nothing is merely hidden behind a filter —
/// a fresh zero-spec repo — so the hint teaches the create move. When rows exist
/// but are filtered out (closed/archived/deferred), returns `None` because the
/// hidden-hints already point at `--all`. Pure, so the empty-state decision is
/// unit-testable without the surrounding render machinery.
// trace:BUG-684 | ai:claude
// BUG-781: the accepted-decision axis counts as "hidden behind a filter" too —
// a project whose only remaining specs are ratified ADRs is not a fresh repo,
// so it must not be told to file its first spec.
// trace:BUG-781 | ai:claude
pub(crate) fn empty_list_hint_line(
    closed_hidden: usize,
    archived_hidden: usize,
    deferred_hidden: usize,
    accepted_decisions_hidden: usize,
) -> Option<&'static str> {
    if closed_hidden == 0
        && archived_hidden == 0
        && deferred_hidden == 0
        && accepted_decisions_hidden == 0
    {
        Some("Nothing here yet — file your first spec: aida add --title \"...\"")
    } else {
        None
    }
}

/// BUG-783: build the `aida list` footer hint lines, in render order.
///
/// Two families live here and they are NOT the same thing:
///
/// * **Lens hints** — closed rows and accepted (terminal) decisions the default
///   OPEN lens drops. These explain why a spec the user KNOWS exists is absent
///   from the rows they just asked for, so they always print.
/// * **View-tier hints** — the archived and deferred tiers. On an explicit
///   open-work request these are noise: the operator asked for open work and
///   does not need reminding on every invocation that the other two tiers
///   exist. Suppressed unless `[list] show_hidden_hints = true` opts back in.
///
/// The tiers stay reachable either way — `--archived`, `--deferred` and `--all`
/// are unaffected (they clear the default lens entirely, so nothing is hidden
/// and no hint applies), and each prints its own row count.
///
/// Returns undecorated lines (2-space indented); the caller dims and prints.
/// Pure so the "default view is quiet" contract is unit-testable without a
/// cache DB or a terminal.
// trace:BUG-783 | ai:claude
pub(crate) fn list_hidden_hint_lines(
    show_view_tier_hints: bool,
    closed_hidden: usize,
    archived_hidden: usize,
    deferred_hidden: usize,
    accepted_decisions_hidden: usize,
) -> Vec<String> {
    let mut lines = Vec::new();
    // STORY-723: the open lens hides closed history — say so.
    if closed_hidden > 0 {
        lines.push(format!(
            "  ({closed_hidden} closed hidden — open lens; pass --all or `--status closed` to see them)"
        ));
    }
    // STORY-441 / STORY-584: the archived + deferred view tiers. Opt-in.
    if show_view_tier_hints {
        if archived_hidden > 0 {
            lines.push(format!(
                "  ({archived_hidden} archived hidden — pass --all or --archived to see them)"
            ));
        }
        if deferred_hidden > 0 {
            lines.push(format!(
                "  ({deferred_hidden} deferred hidden — pass --all or --deferred to see them)"
            ));
        }
    }
    // BUG-781: accepted (terminal) decisions are hidden by the open lens — say
    // so, and name the flag that brings them back.
    if accepted_decisions_hidden > 0 {
        lines.push(format!(
            "  ({accepted_decisions_hidden} accepted decision{} hidden — pass --all or --type decision to see them)",
            if accepted_decisions_hidden == 1 { "" } else { "s" }
        ));
    }
    lines
}

#[cfg(test)]
#[path = "tests/bug_783_list_hidden_hints_tests.rs"]
mod bug_783_list_hidden_hints_tests;

#[cfg(test)]
#[path = "tests/story_1644_list_sort_cap_tests.rs"]
mod story_1644_list_sort_cap_tests;

/// Render the agent-mode (TOON) error block for `err`, whose Debug-formatted
/// text is `msg`. A typed error that carries its own help list renders every
/// item, so multi-line guidance survives; anything else keeps the TASK-972
/// shape of a one-line summary plus the first embedded `aida …` command.
// trace:TASK-972 trace:TASK-1486 | ai:claude
pub(crate) fn agent_error_block(err: &anyhow::Error, msg: &str) -> String {
    if let Some(no_project) = err
        .chain()
        .find_map(|e| e.downcast_ref::<aida_core::NoProjectFound>())
    {
        return toon::error_block_with_help_list(no_project.summary(), &no_project.help_lines());
    }
    let (summary, help) = agent_error_summary_help(msg);
    // TASK-1082: fold a did-you-mean suggestion into the agent-mode
    // summary when a "Requirement not found" error is a near-miss of
    // a real spec id. trace:TASK-1082 | ai:claude
    let summary_owned = match did_you_mean_for_not_found(msg) {
        Some(hint) => format!("{summary} ({hint})"),
        None => summary.to_string(),
    };
    toon::error_block(&summary_owned, help.as_deref())
}

pub(crate) fn agent_error_summary_help(msg: &str) -> (&str, Option<String>) {
    let first = msg.lines().next().unwrap_or("").trim();
    let summary = first
        .strip_prefix("Error: ")
        .or_else(|| first.strip_prefix("error: "))
        .unwrap_or(first);
    (summary, extract_aida_suggestion(msg))
}

/// TASK-972: pull the first backtick-quoted `aida …` command out of an error
/// message, to reuse as the agent error block's `help:` next-command
/// suggestion. AIDA's rich not-found / parse-failure errors already embed
/// hints like ``try `aida list` or `aida search <terms>` ``; surfacing the
/// first one keeps the suggestion truthful without a brittle per-error table.
/// Returns None when the message embeds no such command.
// trace:TASK-972
pub(crate) fn extract_aida_suggestion(msg: &str) -> Option<String> {
    let mut rest = msg;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let close = after.find('`')?;
        let candidate = &after[..close];
        if candidate.starts_with("aida ") {
            return Some(candidate.to_string());
        }
        rest = &after[close + 1..];
    }
    None
}

/// Pure decision for the TASK-970 `aida list` default row cap. Returns the cap
/// to apply when AGENT MODE should bound a bare `aida list`, else `None`. The
/// cap is scoped to the default human-readable table render: an explicit
/// `--limit`/`--all`, the machine shapes (`--short`/`--json`), the grouped
/// `--tree` view, and the human TTY path all bypass it.
// trace:TASK-970
///
/// STORY-1644: `cap` is the resolved agent cap ([`resolve_agent_list_limit`]);
/// `0` means unbounded, so no cap applies.
// trace:STORY-1644 | ai:claude
pub(crate) fn agent_list_default_cap(
    limit: Option<usize>,
    all: bool,
    short: bool,
    json: bool,
    tree: bool,
    agent_mode: bool,
    cap: usize,
) -> Option<usize> {
    if limit.is_none() && !all && !short && !json && !tree && agent_mode && cap > 0 {
        Some(cap)
    } else {
        None
    }
}

/// STORY-1644: resolve the agent-mode `aida list` row cap from the env value
/// (`AIDA_LIST_LIMIT`) and the `[list] agent_limit` config value. Env beats
/// config beats [`AGENT_LIST_DEFAULT_LIMIT`]; `0` = unbounded. A value that
/// isn't a non-negative integer is ignored (falls through to the next
/// source). The `--limit` / `--all` flags beat all of these and are applied
/// by the caller.
// trace:STORY-1644 | ai:claude
pub(crate) fn resolve_agent_list_limit(env: Option<&str>, config: Option<i64>) -> usize {
    if let Some(n) = env.and_then(|v| v.trim().parse::<usize>().ok()) {
        return n;
    }
    match config {
        Some(n) if n >= 0 => n as usize,
        _ => AGENT_LIST_DEFAULT_LIMIT,
    }
}

/// STORY-1644: resolve the `aida list` sort key. An explicit `--sort` always
/// wins; else a recognised `[list] default_sort`; else
/// [`LIST_DEFAULT_SORT`]. An unrecognised config value is ignored with a
/// returned warning so the caller can print it.
// trace:STORY-1644 | ai:claude
pub(crate) fn resolve_list_sort(
    flag: Option<&str>,
    config: Option<&str>,
) -> (String, Option<String>) {
    if let Some(f) = flag {
        return (f.to_string(), None);
    }
    match config.map(|c| c.trim().to_ascii_lowercase()) {
        Some(c) if LIST_SORT_VALUES.contains(&c.as_str()) || c == "centrality" => (c, None),
        Some(c) => (
            LIST_DEFAULT_SORT.to_string(),
            Some(format!(
                "warning: unknown [list] default_sort '{c}' in .aida/config.toml (expected one of: {}); using '{LIST_DEFAULT_SORT}'",
                LIST_SORT_VALUES.join(", ")
            )),
        ),
        None => (LIST_DEFAULT_SORT.to_string(), None),
    }
}

/// STORY-1644: the one-line truncation notice for the agent-mode cap. Says
/// what was cut, in what order, and how to see the rest — never silent.
/// e.g. `count: 100 of 142 open — showing first 100 by id; newest hidden:
/// --sort modified, or --limit 0`.
// trace:STORY-1644 | ai:claude
pub(crate) fn agent_list_truncation_header(
    shown: usize,
    total: usize,
    denom_label: &str,
    sort: &str,
) -> String {
    let (hidden, widen) = match sort {
        "id" => ("newest", "--sort modified, or --limit 0"),
        "modified" => ("least recently modified", "--limit 0"),
        "created" => ("oldest", "--limit 0"),
        "completed" => ("least recently completed", "--limit 0"),
        "heft" | "centrality" => ("least connected", "--limit 0"),
        "weight" => ("lightest", "--limit 0"),
        _ => ("rest", "--limit 0"),
    };
    format!(
        "count: {shown} of {total} {denom_label} — showing first {shown} by {sort}; {hidden} hidden: {widen}"
    )
}

// ── TASK-964: TOON agent-output renderers (AXI #1 token-efficient output +
// #2 minimal default schemas). These run ONLY in agent mode; the human emoji /
// table path is left byte-identical. Each command declares a minimal default
// field set; `--fields` expands `aida list`. trace:TASK-964

/// The minimal default column schema for `aida list` in agent mode (AXI #2).
/// Five fields cover the agent's routine memory-lane need — id / title /
/// status / type / modified_at — instead of the human table's
/// id/origin/type/status/title/tags/glyph spread. `--fields` widens it.
// trace:TASK-964
// trace:TASK-1215 | ai:codex
pub(crate) const TOON_LIST_DEFAULT_FIELDS: &[&str] =
    &["id", "title", "status", "type", "modified_at"];

/// The list fields the agent-mode `--fields` selector understands, mapped onto
/// the cache summary plus the work-routing axis. Unknown names are rejected with
/// this list in the error so the agent can self-correct.
// trace:TASK-964
pub(crate) const TOON_LIST_KNOWN_FIELDS: &[&str] = &[
    "id",
    // trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude — local ID
    // beside the agreed one, or `(agreed id pending)`.
    "local_id",
    "title",
    "status",
    "type",
    "priority",
    "feature",
    "owner",
    "assignee",
    "tags",
    "heft",
    // trace:TASK-1215 | ai:codex — freshness signal for authored lane memory.
    "modified_at",
    // trace:TASK-2205 | ai:claude — creation time; see TOON_LIST_FIELD_ALIASES.
    "opened",
    // trace:FR-283 | ai:claude — the optional numeric weight/score.
    "weight",
    "queued",
    "in_flight",
    "blocked",
    // trace:STORY-776 | ai:claude — the advisor's dispatch classification.
    "mode",
    // trace:STORY-634 | ai:claude — the multi-repo repo/component dimension.
    "origin",
    "deferred_reason",
    // trace:STORY-1653 | ai:claude — derived driver of an in-progress spec.
    "driver",
    "driver_kind",
    // trace:TASK-1951 trace:TASK-2003 | ai:claude — NeedsAttention review lens.
    "review_lens",
];

/// Accepted alternate spellings for `--fields` entries, as `(alias, field)`.
/// [`toon_list_fields`] rewrites each alias to its canonical field, and
/// [`list_fields_help`] names them, so help and parser share one table.
// trace:TASK-2205 | ai:claude
pub(crate) const TOON_LIST_FIELD_ALIASES: &[(&str, &str)] = &[
    ("req_type", "type"),
    ("created_at", "opened"),
    ("created", "opened"),
];

/// The `aida list --fields` help text, generated from
/// [`TOON_LIST_KNOWN_FIELDS`], [`TOON_LIST_FIELD_ALIASES`] and
/// [`TOON_LIST_DEFAULT_FIELDS`] so the documented set cannot drift from the
/// set the parser accepts.
// trace:TASK-2205 | ai:claude
pub(crate) fn list_fields_help() -> String {
    let aliases: Vec<String> = TOON_LIST_FIELD_ALIASES
        .iter()
        .map(|(alias, field)| format!("{alias} = {field}"))
        .collect();
    format!(
        "Select AND order the displayed columns, e.g. `--fields id,title,status,priority,tags` \
         or `--fields 'id title opened'`; entries are separated by commas and/or whitespace. \
         Renders exactly those columns in that order — on the human table AND the \
         token-efficient agent output (a real token win for agents). Valid fields: {}. \
         `opened` is the creation time. Aliases: {}. Default (no `--fields`) keeps the \
         standard columns; agent mode defaults to the minimal {} set",
        TOON_LIST_KNOWN_FIELDS.join(", "),
        aliases.join(", "),
        TOON_LIST_DEFAULT_FIELDS.join(",")
    )
}

/// The `aida search --fields` help text. Search parses `--fields` through the
/// same [`toon_list_fields`] as `aida list`, so its help is generated from the
/// same tables as [`list_fields_help`].
// trace:TASK-2238 | ai:claude
pub(crate) fn search_fields_help() -> String {
    let aliases: Vec<String> = TOON_LIST_FIELD_ALIASES
        .iter()
        .map(|(alias, field)| format!("{alias} = {field}"))
        .collect();
    format!(
        "Select AND order the displayed columns, e.g. `--fields id,title,status,priority,tags` \
         or `--fields 'id title opened'`; entries are separated by commas and/or whitespace. \
         Renders exactly those columns in that order. Valid fields: {}. `opened` is the \
         creation time. Aliases: {}. Search rows carry no work-routing state, so queued, \
         in_flight and blocked render false. Default keeps the standard layout. Mirrors \
         `aida list --fields`",
        TOON_LIST_KNOWN_FIELDS.join(", "),
        aliases.join(", ")
    )
}

/// Render a numeric weight/score for display: integral values drop the
/// fractional part (`42`, not `42.0`); everything else uses the shortest
/// round-trip float formatting (`0.75`).
// trace:FR-283 | ai:claude
pub(crate) fn format_weight(w: f64) -> String {
    if w.fract() == 0.0 && w.abs() < 1e15 {
        format!("{}", w as i64)
    } else {
        format!("{}", w)
    }
}

/// Normalize a cache status string (`InProgress`, `in-progress`, `NeedsAttention`,
/// …) to a stable lowercase-hyphen token for agent output, so the TOON value is
/// uniform regardless of how the cache spelled it.
// trace:TASK-964
pub(crate) fn toon_status_token(status: &str) -> String {
    let s = status.trim();
    match s.to_ascii_lowercase().replace([' ', '_'], "-").as_str() {
        "inprogress" | "in-progress" => "in-progress".to_string(),
        "needsattention" | "needs-attention" => "needs-attention".to_string(),
        other => other.to_string(),
    }
}

/// The column header label for a `--fields` field name. The same field
/// vocabulary [`toon_list_cell`] / [`toon_list_fields`] understand, mapped to a
/// human-friendly capitalized header for the `aida list --fields` table.
// trace:STORY-734 | ai:claude
pub(crate) fn list_field_header(field: &str) -> String {
    match field {
        "id" => "ID",
        "title" => "Title",
        "status" => "Status",
        "type" => "Type",
        "priority" => "Priority",
        "feature" => "Feature",
        "owner" => "Owner",
        "assignee" => "Assignee",
        "tags" => "Tags",
        "heft" => "Heft",
        // trace:TASK-1215 | ai:codex
        "modified_at" => "Modified",
        // trace:TASK-2205 | ai:claude
        "opened" => "Opened",
        // trace:FR-283 | ai:claude
        "weight" => "Weight",
        "queued" => "Queued",
        "in_flight" => "In-Flight",
        "blocked" => "Blocked",
        // trace:TASK-1951 trace:TASK-2003 | ai:claude
        "review_lens" => "Review",
        // Validated against the known set upstream, so this is unreachable in
        // practice; fall back to the raw name for robustness.
        other => return other.to_string(),
    }
    .to_string()
}

/// Build the lines of the human `aida list --fields <csv>` table: exactly the
/// requested columns, in the requested order, each left-aligned and sized to its
/// widest cell (header included). Cell values reuse [`toon_list_cell`] so the
/// human and agent surfaces share one field vocabulary; the `status` and `id`
/// columns are coloured AFTER padding so ANSI escapes don't break alignment, and
/// the trailing column is not padded. Returned as a `Vec<String>` (header,
/// divider, then one line per row) so the layout is unit-testable; the colour
/// helpers no-op when stdout isn't a TTY.
// trace:STORY-734 | ai:claude — plain `//` keeps the marker out of any doc/help.
pub(crate) fn build_list_fields_lines<F>(
    reqs: &[aida_core::RequirementSummary],
    fields: &[String],
    routing_of: F,
) -> Vec<String>
where
    F: Fn(&aida_core::RequirementSummary) -> (bool, bool, bool),
{
    build_list_fields_lines_with(reqs, fields, routing_of, |_, _| None)
}

/// [`build_list_fields_lines`] with `cell_of` answering the columns that need
/// state beyond the summary (the TASK-2003 `review_lens`); `None` falls back
/// to [`toon_list_cell`].
// trace:TASK-1951 trace:TASK-2003 | ai:claude
pub(crate) fn build_list_fields_lines_with<F, C>(
    reqs: &[aida_core::RequirementSummary],
    fields: &[String],
    routing_of: F,
    cell_of: C,
) -> Vec<String>
where
    F: Fn(&aida_core::RequirementSummary) -> (bool, bool, bool),
    C: Fn(&aida_core::RequirementSummary, &str) -> Option<String>,
{
    // Plain (uncoloured) cell text per row per column — width math runs on this.
    let plain: Vec<Vec<String>> = reqs
        .iter()
        .map(|r| {
            let routing = routing_of(r);
            fields
                .iter()
                .map(|f| cell_of(r, f).unwrap_or_else(|| toon_list_cell(r, routing, f)))
                .collect()
        })
        .collect();

    // Column width = widest of (header, every cell). The last column is left
    // unpadded so a long Title never trails whitespace.
    let widths: Vec<usize> = fields
        .iter()
        .enumerate()
        .map(|(c, f)| {
            let header = list_field_header(f).len();
            plain
                .iter()
                .map(|row| row[c].len())
                .max()
                .unwrap_or(0)
                .max(header)
        })
        .collect();
    let last = fields.len().saturating_sub(1);

    let mut lines = Vec::with_capacity(reqs.len() + 2);

    // Header row + divider.
    let mut header = String::new();
    for (c, f) in fields.iter().enumerate() {
        if c > 0 {
            header.push(' ');
        }
        let label = list_field_header(f);
        if c == last {
            header.push_str(&label);
        } else {
            header.push_str(&format!("{:<width$}", label, width = widths[c]));
        }
    }
    lines.push(header.bold().to_string());
    let rule: usize = widths.iter().sum::<usize>() + widths.len().saturating_sub(1);
    lines.push("─".repeat(rule));

    // Data rows: pad the plain cell to its column width, THEN colour status / id
    // so the ANSI escapes don't inflate the `{:<}` byte count. trace:STORY-734
    for (row, req) in plain.iter().zip(reqs.iter()) {
        let mut line = String::new();
        for (c, f) in fields.iter().enumerate() {
            if c > 0 {
                line.push(' ');
            }
            let padded = if c == last {
                row[c].clone()
            } else {
                format!("{:<width$}", row[c], width = widths[c])
            };
            let cell = match f.as_str() {
                // BUG-781: paint on the DISPLAY label, so an accepted decision
                // gets the terminal green rather than the un-started cyan.
                // trace:BUG-781 | ai:claude
                "status" => status_display::paint_status(
                    &padded,
                    status_display::display_status_for_type(&req.req_type, &req.status),
                )
                .to_string(),
                "id" => padded.bold().to_string(),
                _ => padded,
            };
            line.push_str(&cell);
        }
        lines.push(line);
    }
    lines
}

/// Render the body of `aida search --fields <csv>` for a non-empty result set,
/// branching on agent vs human surface the same way `aida list --fields` does.
/// In agent mode (`agent == true`) it's the lean TOON `specs[N]{...}` projection
/// — the ~2x token win the agent surface exists for, instead of the verbose
/// box-table an agent would otherwise pay for. On a human TTY it's the aligned
/// box-table shared with `aida list` via [`build_list_fields_lines`]. Both
/// surfaces resolve cells through [`toon_list_cell`] over the already-validated
/// `selected` field set, so the two surfaces never drift in field vocabulary.
/// Search rows carry no work-routing axis, so queued/in_flight/blocked render
/// false; `review_lens` answers the `review_lens` column as in `aida list`.
/// Returned as a `String` so the branch is unit-testable.
// trace:STORY-734 trace:BUG-668 | ai:claude — plain `//` keeps the marker out of any doc/help.
// trace:TASK-1951 trace:TASK-2003 | ai:claude
pub(crate) fn render_search_fields(
    results: &[aida_core::RequirementSummary],
    selected: &[String],
    agent: bool,
    review_lens: &std::collections::HashMap<uuid::Uuid, String>,
) -> String {
    let cell = |r: &aida_core::RequirementSummary, f: &str| {
        (f == "review_lens").then(|| review_lens.get(&r.id).cloned().unwrap_or_default())
    };
    if agent {
        let field_refs: Vec<&str> = selected.iter().map(String::as_str).collect();
        let rows: Vec<Vec<String>> = results
            .iter()
            .map(|r| {
                selected
                    .iter()
                    .map(|f| {
                        cell(r, f).unwrap_or_else(|| toon_list_cell(r, (false, false, false), f))
                    })
                    .collect()
            })
            .collect();
        let mut out = format!("count: {} results\n", results.len());
        out.push_str(&crate::toon::table_raw("specs", &field_refs, &rows));
        out
    } else {
        let mut lines =
            build_list_fields_lines_with(results, selected, |_| (false, false, false), cell);
        lines.push(format!("\n{} results", results.len()));
        lines.join("\n")
    }
}

#[cfg(test)]
#[path = "tests/task_1486_store_refusal_tests.rs"]
mod task_1486_store_refusal_tests;

#[cfg(test)]
#[path = "tests/task_1487_store_polish_tests.rs"]
mod task_1487_store_polish_tests;

pub(crate) fn maybe_run_asciinema_wrapper(raw_args: &[String], cli: &Cli) -> Result<Option<i32>> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!(
            "{} --asciinema requested but stdin/stdout is not a TTY; running without recording.",
            crate::glyph(crate::glyphs::Glyph::Info).cyan()
        );
        return Ok(None);
    }

    if !asciinema_available() {
        eprintln!(
            "{} --asciinema requested but `asciinema` is not installed or not on PATH; running without recording.",
            crate::glyph(crate::glyphs::Glyph::Info).cyan()
        );
        return Ok(None);
    }

    let aida_exe = resolve_aida_exe();
    let inner_args = strip_asciinema_wrapper_args(raw_args);
    let cast_path = resolve_cast_output_path(cli.globals.cast_out.as_ref(), &inner_args)?;
    if let Some(parent) = cast_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating cast output directory {}", parent.display()))?;
    }
    let title = cli
        .globals
        .cast_title
        .clone()
        .unwrap_or_else(|| default_cast_title(&inner_args));
    let command = shell_command_for_asciinema(&aida_exe, &inner_args);

    let status = std::process::Command::new("asciinema")
        .args(["rec", "--command", &command, "--title", &title])
        .arg(&cast_path)
        .env(ASCIINEMA_WRAPPED_ENV, "1")
        .status()
        .context("could not invoke `asciinema rec`")?;

    if status.success() {
        println!("Cast saved: {}", cast_path.display());
    }
    Ok(Some(status.code().unwrap_or(1)))
}

pub(crate) fn asciinema_available() -> bool {
    std::process::Command::new("asciinema")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub(crate) fn strip_asciinema_wrapper_args(raw_args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(bin) = raw_args.first() {
        out.push(bin.clone());
    }
    let mut i = 1;
    while i < raw_args.len() {
        let arg = &raw_args[i];
        match arg.as_str() {
            "--asciinema" => {
                i += 1;
            }
            "--cast-out" | "--cast-title" => {
                i += 2;
            }
            _ if arg.starts_with("--cast-out=") || arg.starts_with("--cast-title=") => {
                i += 1;
            }
            _ => {
                out.push(arg.clone());
                i += 1;
            }
        }
    }
    out
}

pub(crate) fn resolve_cast_output_path(
    override_path: Option<&std::path::PathBuf>,
    inner_args: &[String],
) -> Result<std::path::PathBuf> {
    if let Some(path) = override_path {
        return Ok(path.clone());
    }
    let base_dir =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let stamp = chrono::Utc::now().format("%Y-%m-%dT%H%M%SZ");
    let slug = asciinema_command_slug(inner_args);
    Ok(base_dir
        .join(".aida")
        .join("casts")
        .join(format!("{stamp}-{slug}.cast")))
}

pub(crate) fn derive_semantic_title(inner_args: &[String]) -> Option<String> {
    let has_queue_work =
        inner_args.iter().any(|arg| arg == "queue") && inner_args.iter().any(|arg| arg == "work");
    let has_pr_ship =
        inner_args.iter().any(|arg| arg == "pr") && inner_args.iter().any(|arg| arg == "ship");

    if has_queue_work {
        let mut specs = Vec::new();
        let mut batch_name = None;

        let mut i = 1;
        let spec_re = regex::Regex::new(r"(?i)^[a-z]+-\d+$").unwrap();
        while i < inner_args.len() {
            let arg = &inner_args[i];
            if arg == "--batch" && i + 1 < inner_args.len() {
                batch_name = Some(inner_args[i + 1].clone());
                i += 2;
                continue;
            } else if let Some(val) = arg.strip_prefix("--batch=") {
                batch_name = Some(val.to_string());
                i += 1;
                continue;
            }

            if spec_re.is_match(arg) {
                specs.push(arg.to_uppercase());
            }
            i += 1;
        }

        if !specs.is_empty() {
            return Some(format!("AIDA drain: {}", specs.join(", ")));
        } else if let Some(batch) = batch_name {
            return Some(format!("AIDA drain: batch {batch}"));
        }
    } else if has_pr_ship {
        let pr_re = regex::Regex::new(r"^\d+$").unwrap();
        for arg in inner_args.iter().skip(1) {
            if pr_re.is_match(arg) {
                return Some(format!("AIDA pr ship: PR-{arg}"));
            }
        }
    }
    None
}

pub(crate) fn default_cast_title(inner_args: &[String]) -> String {
    if let Some(semantic) = derive_semantic_title(inner_args) {
        return semantic;
    }
    let rest = inner_args
        .iter()
        .skip(1)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    if rest.trim().is_empty() {
        "aida".to_string()
    } else {
        format!("aida {rest}")
    }
}

pub(crate) fn slugify_str(s: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

pub(crate) fn derive_spec_aware_slug(inner_args: &[String]) -> Option<String> {
    let mut parts = Vec::new();
    let mut i = 1;

    let has_pr_ship =
        inner_args.iter().any(|arg| arg == "pr") && inner_args.iter().any(|arg| arg == "ship");
    let spec_re = regex::Regex::new(r"(?i)^[a-z]+-\d+$").unwrap();
    let pr_re = regex::Regex::new(r"^\d+$").unwrap();

    while i < inner_args.len() {
        let arg = &inner_args[i];

        if arg == "--batch" {
            if i + 1 < inner_args.len() {
                let val = slugify_str(&inner_args[i + 1]);
                parts.push(format!("batch-{val}"));
                i += 2;
                continue;
            }
        } else if let Some(raw) = arg.strip_prefix("--batch=") {
            let val = slugify_str(raw);
            parts.push(format!("batch-{val}"));
            i += 1;
            continue;
        }

        if spec_re.is_match(arg) {
            parts.push(arg.to_lowercase());
        } else if has_pr_ship && pr_re.is_match(arg) {
            parts.push(format!("pr-{arg}"));
        }

        i += 1;
    }

    if !parts.is_empty() {
        let mut seen = std::collections::HashSet::new();
        let mut unique_parts = Vec::new();
        for p in parts {
            if seen.insert(p.clone()) {
                unique_parts.push(p);
            }
        }
        Some(unique_parts.join("-"))
    } else {
        None
    }
}

pub(crate) fn asciinema_command_slug(inner_args: &[String]) -> String {
    if let Some(spec_slug) = derive_spec_aware_slug(inner_args) {
        return truncate_asciinema_slug(&spec_slug);
    }

    // Generic fallback: use subcommand + first non-flag arg
    let pos_args: Vec<String> = inner_args
        .iter()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .map(|arg| slugify_str(arg))
        .filter(|arg| !arg.is_empty())
        .collect();

    let fallback_slug = if pos_args.len() >= 2 {
        format!("{}-{}", pos_args[0], pos_args[1])
    } else if pos_args.len() == 1 {
        pos_args[0].clone()
    } else {
        "aida".to_string()
    };

    truncate_asciinema_slug(&fallback_slug)
}

pub(crate) fn truncate_asciinema_slug(slug: &str) -> String {
    let count = slug.chars().count();
    if count <= ASCIINEMA_SLUG_MAX_CHARS {
        return slug.to_string();
    }
    let keep = ASCIINEMA_SLUG_MAX_CHARS.saturating_sub("-trunc".len());
    let mut truncated = slug.chars().take(keep).collect::<String>();
    truncated = truncated.trim_end_matches('-').to_string();
    format!("{truncated}-trunc")
}

pub(crate) fn shell_command_for_asciinema(
    aida_exe: &std::path::Path,
    inner_args: &[String],
) -> String {
    std::iter::once(aida_exe.to_string_lossy().to_string())
        .chain(inner_args.iter().skip(1).cloned())
        .map(|arg| shell_quote(&arg))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn shell_quote(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_string();
    }
    if arg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '='))
    {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
#[path = "tests/story_423_asciinema_tests.rs"]
mod story_423_asciinema_tests;

/// TASK-822 / 824 / 828 / 831: rewrite the `aida list <lens>` discoverability
/// aliases into their canonical commands before clap parses, so the list family
/// (`open` / `human` / `queue` / `why` / `advisor` / `inflight`) reads uniformly
/// while each alias is a pure passthrough of the target command's flags. The
/// target may be one token (`aida advisor`) or two (`aida queue list`). Unmatched
/// input is returned unchanged. `args[0]` is the binary name.
/// trace:TASK-822 trace:TASK-824 trace:TASK-828 trace:TASK-831
/// STORY-623: rewrite `aida advisor assess [args]` → `aida groom [args]` before
/// clap, so the advisor's draft-disposition verb is reachable under its seat
/// while the implementation stays the single `groom` (aka deprecated `assess` /
/// `intake`) command. STORY-708: the canonical target is now `groom` — the
/// advisor-seat spelling normalizes straight to the canonical verb.
/// Unmatched input is returned unchanged.
// trace:STORY-623 trace:STORY-708 | ai:claude
pub(crate) fn rewrite_advisor_assess(args: &[String]) -> Vec<String> {
    if args.len() >= 3 && args[1] == "advisor" && args[2] == "assess" {
        let mut out = Vec::with_capacity(args.len() - 1);
        out.push(args[0].clone());
        out.push("groom".to_string());
        out.extend_from_slice(&args[3..]);
        return out;
    }
    args.to_vec()
}

/// STORY-708: `groom` is the canonical advisor-disposition verb; `assess` and
/// `intake` are deprecated aliases. clap still accepts both (the `Groom` command
/// carries `#[command(alias = "assess", alias = "intake")]`), but we normalize a
/// top-level `aida assess`/`aida intake` to `aida groom` BEFORE clap so the
/// dispatch sees one canonical token, and we print a one-line, non-blocking
/// deprecation hint to stderr pointing at `aida groom`. The rewrite only fires
/// when the deprecated verb is the SUBCOMMAND (argv[1]) — a later positional or
/// flag value that happens to read `assess`/`intake` is left untouched. Returns
/// (rewritten argv, deprecated-verb-seen) so the caller can emit the hint after
/// argv parsing succeeds.
// trace:STORY-708 | ai:claude
pub(crate) fn rewrite_groom_alias(args: &[String]) -> (Vec<String>, Option<String>) {
    if args.len() >= 2 && (args[1] == "assess" || args[1] == "intake") {
        let deprecated = args[1].clone();
        let mut out = Vec::with_capacity(args.len());
        out.push(args[0].clone());
        out.push("groom".to_string());
        out.extend_from_slice(&args[2..]);
        return (out, Some(deprecated));
    }
    (args.to_vec(), None)
}

/// TASK-858: bare `aida agent` (no recognized subcommand) defaults to
/// `aida agent new`, git-style, forwarding any flags/args to `new`. The
/// recognized subcommands (`new`, `register`, `list`, `ls`, `status`, `gc`,
/// `dispatch-health`, `pause`, `resume`, `stop`, `list-roles`) pass through
/// unchanged, and `aida agent --help` / `-h` keeps clap's parent help (so the
/// surface stays discoverable). Mirrors the pre-clap argv-rewrite pattern used
/// for the `list` lens aliases and `advisor assess`.
// trace:TASK-858 | ai:claude
pub(crate) fn rewrite_agent_default_new(args: &[String]) -> Vec<String> {
    if args.len() >= 2 && args[1] == "agent" {
        // The next token decides: a recognized subcommand or a help flag is
        // left alone; anything else (including bare `aida agent`) gets `new`
        // spliced in so flags/args forward to the `new` launcher.
        const KNOWN_SUBCOMMANDS: &[&str] = &[
            "new",
            "register",
            // trace:TASK-B6B1278890774EF993D2EBC327862470-117 | ai:codex
            "list",
            "ls",
            "status",
            "gc",
            "dispatch-health",
            "pause",
            "resume",
            "stop",
            // The hidden `wake` (and `describe`) were rewritten to
            // `agent new wake …`, so the documented manual-arm command
            // failed. trace:STORY-2256 | ai:claude
            "describe",
            "wake",
            "list-roles",
            "help",
        ];
        let next = args.get(2).map(String::as_str);
        let passthrough = matches!(next, Some(t) if
            KNOWN_SUBCOMMANDS.contains(&t) || t == "--help" || t == "-h");
        if !passthrough {
            let mut out = Vec::with_capacity(args.len() + 1);
            out.push(args[0].clone());
            out.push("agent".to_string());
            out.push("new".to_string());
            out.extend_from_slice(&args[2..]);
            return out;
        }
    }
    args.to_vec()
}

/// TASK-881: bare `aida queue` (no recognized subcommand) defaults to
/// `aida queue list`, matching the `aida list` / `aida status` "bare form is
/// the read view" ergonomics — instead of dumping the clap subcommand help
/// wall (exit 2), which reads as an error to a newcomer asking "what's
/// queued?". Recognized subcommands pass through unchanged, and `aida queue
/// --help` / `-h` / `help` keep clap's parent help so the surface stays
/// discoverable. Mirrors `rewrite_agent_default_new` (TASK-858); flags-only
/// invocations (`aida queue --role implementer`) forward to `list`.
// trace:TASK-881 | ai:claude
pub(crate) fn rewrite_queue_default_list(args: &[String]) -> Vec<String> {
    if args.len() >= 2 && args[1] == "queue" {
        // The next token decides: a recognized subcommand or a help flag is
        // left alone; anything else (including bare `aida queue` and
        // flags-only forms) gets `list` spliced in so flags forward to it.
        const KNOWN_SUBCOMMANDS: &[&str] = &[
            "list",
            "add",
            "load",
            "remove",
            "move",
            "clear",
            "prune",
            "gc",
            "next",
            "advance",
            "done",
            "work",
            "progress",
            "rework",
            "recover",
            "integrate",
            "help",
        ];
        let next = args.get(2).map(String::as_str);
        let passthrough = matches!(next, Some(t) if
            KNOWN_SUBCOMMANDS.contains(&t) || t == "--help" || t == "-h");
        if !passthrough {
            let mut out = Vec::with_capacity(args.len() + 1);
            out.push(args[0].clone());
            out.push("queue".to_string());
            out.push("list".to_string());
            out.extend_from_slice(&args[2..]);
            return out;
        }
    }
    args.to_vec()
}

/// One `aida list <lens>` argv-rewrite row. See [`LIST_LENS_ALIASES`].
/// trace:STORY-667 | ai:claude
pub(crate) struct ListLensAlias {
    /// The positional lens token(s) that trigger this rewrite (first is canonical).
    pub tokens: &'static [&'static str],
    /// The canonical command tokens the lens expands to (after `aida`).
    pub canonical: &'static [&'static str],
    /// One-line human meaning for the `aida alias` registry.
    pub meaning: &'static str,
}

/// The `aida list <lens>` argv-rewrite table — the SINGLE SOURCE OF TRUTH for
/// the list-lens aliases. Each row maps the positional lens token(s) it accepts
/// to the canonical command tokens it expands to, plus a one-line meaning for
/// the `aida alias` registry. `rewrite_list_alias` resolves against this table
/// and the `aida alias` registry enumerates it — so the surface and its catalog
/// can't drift. trace:STORY-667 | ai:claude
pub(crate) const LIST_LENS_ALIASES: &[ListLensAlias] = &[
    ListLensAlias {
        tokens: &["queue"],
        canonical: &["queue", "list"],
        meaning: "your personal work queue",
    },
    ListLensAlias {
        tokens: &["why"],
        canonical: &["burndown", "explain"],
        meaning: "why each open spec sits where it does",
    },
    ListLensAlias {
        tokens: &["advisor"],
        canonical: &["advisor"],
        meaning: "the live-advisor registration view",
    },
    // TASK-831: active work — leased specs + drain in-flight status.
    ListLensAlias {
        tokens: &["inflight", "in-flight"],
        canonical: &["burndown", "status"],
        meaning: "active work — leased specs + drain in-flight status",
    },
];

/// Every built-in pre-clap argv rewrite, in dispatch order, returning the
/// rewritten argv and the deprecated groom verb typed (if any). One function so
/// `aida docs sweep` checks documented commands exactly as dispatch reads them.
// trace:STORY-2220 | ai:claude
pub(crate) fn builtin_alias_rewrites(args: &[String]) -> (Vec<String>, Option<String>) {
    rewrite_groom_alias(&rewrite_queue_default_list(&rewrite_agent_default_new(
        &rewrite_list_alias(&rewrite_type_alias(&rewrite_advisor_assess(
            &rewrite_personal_view_alias(args),
        ))),
    )))
}

/// TASK-862: top-level personal-view shortcuts. `aida mylist` rewrites to
/// `aida list me` (the current user's owned/assigned specs — STORY-662's `me`
/// filter), forwarding any extra flags (`aida mylist --status open`). `aida
/// myqueue` rewrites to `aida queue list`, which is already user-scoped via
/// `current_user_id()` (BUG-89) — so `myqueue` is just a discoverable alias for
/// "my queue". Both are pre-clap argv rewrites, the same idiom as
/// `rewrite_list_alias` / `rewrite_advisor_assess`, so every downstream flag
/// passes through unchanged. They deliberately do NOT touch the `aida list`
/// default scope (`aida list` still shows ALL) — the default-scope question is
/// recorded on the spec for the operator, not actioned here.
// trace:TASK-862 | ai:claude
pub(crate) fn rewrite_personal_view_alias(args: &[String]) -> Vec<String> {
    if args.len() >= 2 {
        match args[1].as_str() {
            "mylist" => {
                let mut out = Vec::with_capacity(args.len() + 1);
                out.push(args[0].clone());
                out.push("list".to_string());
                out.push("me".to_string());
                out.extend_from_slice(&args[2..]);
                return out;
            }
            "myqueue" => {
                let mut out = Vec::with_capacity(args.len() + 1);
                out.push(args[0].clone());
                out.push("queue".to_string());
                out.push("list".to_string());
                out.extend_from_slice(&args[2..]);
                return out;
            }
            _ => {}
        }
    }
    args.to_vec()
}

pub(crate) fn rewrite_list_alias(args: &[String]) -> Vec<String> {
    if args.len() >= 3 && args[1] == "list" {
        let target: Option<&[&str]> = LIST_LENS_ALIASES
            .iter()
            .find(|a| a.tokens.contains(&args[2].as_str()))
            .map(|a| a.canonical);
        if let Some(tokens) = target {
            let mut out = Vec::with_capacity(args.len());
            out.push(args[0].clone());
            out.extend(tokens.iter().map(|s| s.to_string()));
            out.extend_from_slice(&args[3..]);
            return out;
        }
    }
    args.to_vec()
}

// trace:STORY-1050 | ai:antigravity
pub(crate) fn rewrite_type_alias(args: &[String]) -> Vec<String> {
    if args.len() >= 2 {
        let ty = args[1].to_lowercase();
        // AIDA's built-in requirement types — one table shared with the
        // `aida list <type>` positional. trace:STORY-1642 | ai:claude
        let valid_types = crate::knowledge_list::BUILTIN_TYPE_WORDS;
        // `doc` is a real top-level subcommand (`aida doc add/list`), so it
        // must never be rewritten into `--type doc` (regression of the
        // STORY-1050 fix). trace:STORY-1642 | ai:claude
        let is_type_alias = ty != "doc" && valid_types.contains(&ty.as_str());

        if is_type_alias {
            if args.len() == 2 {
                // aida bug -> aida list --type bug
                return vec![
                    args[0].clone(),
                    "list".to_string(),
                    "--type".to_string(),
                    ty,
                ];
            } else if args[2] == "list" {
                // aida bug list ... -> aida list --type bug ...
                let mut out = vec![
                    args[0].clone(),
                    "list".to_string(),
                    "--type".to_string(),
                    ty,
                ];
                out.extend_from_slice(&args[3..]);
                return out;
            } else if ty == "faq" && args[2] == "topics" {
                // aida faq topics ... -> aida list --type faq --topics ...
                // trace:STORY-1645 | ai:claude
                let mut out = vec![
                    args[0].clone(),
                    "list".to_string(),
                    "--type".to_string(),
                    ty,
                    "--topics".to_string(),
                ];
                out.extend_from_slice(&args[3..]);
                return out;
            } else if args[2] == "add" {
                // aida bug add ... -> aida add --type bug ...
                let mut out = vec![args[0].clone(), "add".to_string(), "--type".to_string(), ty];
                out.extend_from_slice(&args[3..]);
                return out;
            }
        }
    }
    args.to_vec()
}

// trace:STORY-1027 | ai:codex
pub(crate) fn parse_help_commands_args(args: &[String]) -> Result<(bool, bool, bool)> {
    let mut flags = false;
    let mut json = false;
    let mut hidden = false;
    for arg in args {
        match arg.as_str() {
            "--flags" => flags = true,
            "--json" => json = true,
            "--hidden" => hidden = true,
            other => anyhow::bail!(
                "unknown option for `aida help commands`: {other}\n\
                 supported options: --flags, --json, --hidden"
            ),
        }
    }
    Ok((flags, json, hidden))
}

pub(crate) fn run() -> Result<()> {
    // BUG-2221: anchor for the `argv` phase recorded below once dispatch
    // knows the command is `show`. trace:BUG-2221 | ai:claude
    let run_started = std::time::Instant::now();
    crate::gate::seat_authority::ensure_commit_authority_resolver_registered();
    let raw_args: Vec<String> = std::env::args().collect();
    // Intercept --version / -V before clap so we can include the build-time
    // banner (build.rs stamps build time + git sha + dirty flag). Clap's
    // built-in #[clap(version)] only knows the package version, which can't
    // distinguish two binaries at the same version built at different times.
    if raw_args.len() == 2 && (raw_args[1] == "--version" || raw_args[1] == "-V") {
        println!("aida {}", build_banner());
        return Ok(());
    }

    // trace:TASK-1718 | ai:antigravity
    if raw_args.len() == 2 && raw_args[1] == "--path" {
        let exe = crate::resolve_aida_exe();
        println!("{}", exe.display());
        return Ok(());
    }

    // Tiered help: bare `aida` and `aida help` LEAD with a small curated
    // "Getting started" set + a teaser of the grouped surface, so a newcomer
    // sees an approachable path instead of a flat 40-command clap dump. The
    // full grouped inventory is one step away via `aida help --all` (or the
    // legacy `aida help-all`). Intercepting here is required because bare
    // `aida` otherwise fails clap parsing (subcommand required → exit 2).
    // trace:STORY-556
    {
        let first = raw_args.get(1).map(String::as_str);
        // Args after the `help` verb (empty for bare `aida`).
        let rest = raw_args.get(2..).unwrap_or(&[]);
        // `aida help` (clap's builtin help verb) with no further positional —
        // optionally `--all`. `aida --help` / `aida -h` keep clap's own flag
        // help (a deliberate escape hatch to the raw flag-level usage).
        let is_help_verb = first == Some("help") && rest.iter().all(|a| a == "--all" || a == "-a");
        let is_bare = raw_args.len() == 1;
        // `aida help <topic>` — the middle granularity between bare `help`
        // (group names) and `help --all` (everything): expand one group. A
        // single non-flag positional after `help` is a topic name.
        // trace:TASK-861 | ai:claude
        let help_topic: Option<&str> =
            if first == Some("help") && rest.len() == 1 && !rest[0].starts_with('-') {
                Some(rest[0].as_str())
            } else {
                None
            };
        if first == Some("help")
            && rest
                .first()
                .is_some_and(|topic| topic.eq_ignore_ascii_case("commands"))
        {
            let (flags, json, hidden) = parse_help_commands_args(&rest[1..])?;
            help_catalog::print_command_catalog(flags, hidden, json);
            return Ok(());
        }
        if let Some(topic) = help_topic {
            // `aida help commands` — the flat clap-derived catalog of every
            // command and subcommand, one line each. Intercepted before the
            // group-topic resolver so it can't be shadowed by a group name.
            // trace:TASK-1098 | ai:claude
            if topic.eq_ignore_ascii_case("commands") {
                help_catalog::print_command_catalog(false, false, false);
                return Ok(());
            }
            // Exact command help keeps clap's native rendering ahead of the
            // semantic concept/FTS layers. trace:STORY-837 | ai:codex
            if help_catalog::print_exact_command_help(topic) {
                return Ok(());
            }
            // `aida help <ext>` delegates to `aida-<ext> --help` once no
            // built-in command or help group claims the name.
            // trace:STORY-1641 | ai:claude
            if help::resolve_help_topic(topic).is_err() {
                if let Some(code) = external_cmd::help_for(topic) {
                    std::process::exit(code?);
                }
            }
            return print_help_topic(topic);
        }
        // TASK-970: content-first bare `aida` in AGENT MODE. A human at a TTY
        // keeps the getting-started menu (the Trojan-horse first impression);
        // an agent (non-TTY / `AIDA_AGENT_OUTPUT`) gets the `aida status`
        // snapshot + top queued instead, since CLAUDE.md names `aida status`
        // as the entry point and a help dump is noise to a coding agent. The
        // explicit `aida help` verb is unchanged in both modes.
        // trace:TASK-970
        if is_bare && agent_output_mode() {
            return handle_bare_agent_status();
        }
        if is_help_verb || is_bare {
            let want_all = rest.iter().any(|a| a == "--all" || a == "-a");
            if want_all {
                print_help_all();
            } else {
                print_tiered_help();
            }
            return Ok(());
        }
    }

    // TASK-822 / TASK-824 / STORY-623: pre-clap argv rewrites so aliases pass
    // every downstream flag through unchanged. `aida list <lens>` → its command
    // (queue/why/advisor/inflight); `aida advisor assess` → `aida assess` (the
    // advisor's verb spelled under its seat). `aida assess` itself + the `intake`
    // alias are handled by clap's visible_alias on the Assess variant.
    // trace:TASK-822 trace:TASK-824 trace:STORY-623 | ai:claude
    // TASK-862: `aida mylist` / `aida myqueue` personal-view shortcuts rewrite
    // before the list-lens / advisor-assess passes so `mylist` reaches the same
    // `list me` path the lens resolvers feed.
    // trace:TASK-881 | ai:claude — bare `aida queue` -> `aida queue list`.
    // TASK-877: user/project-defined aliases expand FIRST (outermost), so an
    // alias's stored expansion (e.g. `list draft`) then flows through the
    // built-in personal-view / list-lens / queue-default passes. The expansion
    // is gated on an interactive HUMAN caller inside `user_alias::expand` — an
    // agent / headless / MCP / non-TTY caller gets argv back unchanged, so the
    // canonical surface every vendor sees is never reshaped by a user alias.
    // trace:TASK-877 | ai:claude
    let expanded = user_alias::expand(&raw_args);
    // STORY-708: normalize the deprecated `aida assess` / `aida intake` verbs to
    // the canonical `aida groom` before clap, and capture which deprecated
    // spelling (if any) the operator typed so we can print a non-blocking hint
    // once argv parsing succeeds. trace:STORY-708 | ai:claude
    let (after_alias_rewrites, groom_deprecated_verb) = builtin_alias_rewrites(&expanded);
    // Git-style extensions: a head that is still not a real subcommand after
    // every built-in and user-alias stage runs `aida-<name>` from PATH.
    // trace:STORY-1641 | ai:claude
    if let Some(code) = external_cmd::maybe_dispatch(&after_alias_rewrites)? {
        std::process::exit(code);
    }
    let mut cli = crate::command_registry::parse_from(after_alias_rewrites.clone());

    // STORY-764: install the explicit output-format pin before any handler
    // renders. `--format` wins; else `AIDA_OUTPUT_FORMAT`; else the TTY-based
    // default stands. trace:STORY-764 | ai:claude
    set_output_format_override(cli.globals.format);
    // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
    aida_core::node_alias::set_canonical_ids(cli.globals.canonical_ids);
    // trace:TASK-1526 | ai:codex
    cache_output::set_toon_cache_output(matches!(cli.command.name(), "history" | "graph"));
    // `rel add` and `pr auto-queue-review` run inside automated hand-offs: a
    // lock held by a holder that cannot let go soon must fail them fast. A
    // child they spawn (`aida add`, `aida queue add`) inherits the deadline.
    // trace:BUG-HOME-041 | ai:claude
    if fail_fast_lock_command(cli.command.name(), cli.command.subcommand_name())
        || std::env::var_os(aida_core::io_pressure::FAIL_FAST_DEADLINE_ENV).is_some()
    {
        aida_core::io_pressure::arm_fail_fast_lock_deadline(
            aida_core::io_pressure::FAIL_FAST_LOCK_BUDGET,
        );
    }
    // Only human output at a real terminal may perform the pre-C inline full
    // rebuild. Explicit JSON/TOON and advisory paths stay bounded at a TTY too.
    // trace:TASK-1526 | ai:codex
    let advisory = raw_args
        .iter()
        .any(|s| s == "--notice" || s == "statusline" || s == "statusbar");
    let machine =
        raw_args.iter().any(|s| s == "--json" || s == "--toon") || agent_output_mode_quiet();
    // A single-spec read must not inherit the 1500ms refresh wait or a
    // terminal's inline full rebuild. Its object is read canonically; the
    // existing snapshot labels disclose stale derived graph context.
    // Strict mutations and explicit cache refresh keep their own policy.
    // trace:BUG-1801 | ai:codex
    let single_spec_read = cli.command.name() == "show";
    aida_core::db::cache_refresh::configure_read_policy(
        std::io::IsTerminal::is_terminal(&std::io::stdout())
            && !machine
            && !advisory
            && !single_spec_read,
        (advisory || single_spec_read).then_some(aida_core::db::cache_refresh::ReadBudget(
            std::time::Duration::ZERO,
        )),
    );

    // A global clap flag is syntactically accepted everywhere. Convert an
    // unsupported JSON request into an explicit error before any command can
    // silently fall back to prose. trace:BUG-1502 | ai:codex
    let mut dispatch_argv = after_alias_rewrites;
    let dispatch_len = dispatch_argv.len();
    enforce_json_format_capability(&mut dispatch_argv)?;
    // Reparse only when capability materialization changed argv. This makes
    // the local `json` field authoritative for every downstream dispatcher.
    if dispatch_argv.len() != dispatch_len {
        cli = crate::command_registry::parse_from(dispatch_argv);
    }

    // BUG-2221: `show` records per-phase timings (BUG-1993); everything from
    // process entry to here — alias rewrites, extension-shadowing guards, the
    // clap parse, the JSON capability gate and its reparse — is the `argv`
    // phase. Arming here (earlier than the storage-dispatch arm, which
    // `arm_phases` tolerates re-running) is what lets this pre-dispatch span
    // be attributed at all. trace:BUG-2221 | ai:claude
    if cli.command.name() == "show" {
        crate::usage::arm_phases();
        crate::usage::record_phase("argv", run_started.elapsed());
    }
    // BUG-b6b1278890774ef993d2ebc327862470-008: the pre-dispatch hooks below
    // (presence flip, removed-cwd warning, role-restore offer, store-less
    // early dispatch) are the `preflight` phase. No-op unless armed.
    // trace:BUG-b6b1278890774ef993d2ebc327862470-008 | ai:claude
    let preflight_phase = crate::usage::phase("preflight");

    // STORY-708: one-line, non-blocking deprecation hint. Printed to stderr (so
    // it never pollutes machine-readable stdout) only when the operator reached
    // groom via a deprecated alias. trace:STORY-708 | ai:claude
    if let Some(verb) = &groom_deprecated_verb {
        eprintln!("note: `aida {verb}` is deprecated — use `aida groom` (the canonical disposition verb).");
    }

    // The detached cache refresh worker carries explicit `--store`/`--cache`
    // paths and runs from a neutral cwd (never a removable session worktree),
    // so it is dispatched before any cwd-based project discovery, which
    // would find no project there and fail. trace:BUG-1947 | ai:claude
    if let Some(result) = cache_cmd::early_refresh(&cli.command) {
        return result;
    }

    if cli.globals.asciinema && std::env::var_os(ASCIINEMA_WRAPPED_ENV).is_none() {
        if let Some(exit_code) = maybe_run_asciinema_wrapper(&raw_args, &cli)? {
            std::process::exit(exit_code);
        }
    }

    // Check for AIDA_SERVER environment variable if --server not specified
    if cli.globals.server.is_none() {
        cli.globals.server = std::env::var("AIDA_SERVER").ok();
    }

    enforce_stakeholder_role_capabilities(&mut cli.command)?;

    // TASK-756: operator-presence TTY auto-flip. Any interactive aida
    // command (stdout+stdin are a TTY) means the operator is demonstrably
    // back — flip a stored `away` to `home`. Cheap (only writes when a flip
    // is needed) and non-fatal (a presence-file error never breaks the
    // actual command). This is the one consumer-free hook for the primitive;
    // it does NOT change any command's behavior. trace:TASK-756 | ai:claude
    presence::auto_flip_if_interactive();

    // BUG-108: a shell whose worktree was removed by `aida session end`
    // (in this or another terminal) has a dangling cwd — flag it before
    // any project-root lookup silently degrades to empty state and makes
    // `aida queue list` print "Your queue is empty". trace:BUG-108
    warn_if_cwd_removed();

    // BUG-1044: when explicitly enabled, an interactive shell that lost its
    // role env after a reboot gets one early, once-per-terminal restoration
    // offer before a later advisor gate surprises it.
    maybe_prompt_role_restore(&cli.command)?;

    // Registered store-less commands dispatch here, where the legacy
    // `_bg-fetch` / `away` / `home` arms they replaced ran.
    // trace:STORY-1728 | ai:claude
    let Command::Registered(invocation) = &cli.command;
    if invocation.run_early(&cli.globals)? == crate::command_registry::Flow::Return {
        return Ok(());
    }
    drop(preflight_phase);

    // Determine which requirements file to use
    // trace:REQ-0231 | ai:claude:high
    let requirements_path = if let Some(ref explicit_file) = cli.globals.file {
        let explicit_path = std::path::PathBuf::from(explicit_file);
        // If path is a directory, use GitBackend and route through the backend
        // API. Goes through `run_on_distributed_store` — not straight to
        // `handle_git_backend_command` — so an explicit `--file <dir>` gets
        // the same tracker (jira/github/gitlab) and mcp-serve special-casing
        // as every other distributed-store resolution path; the git-backend
        // handler alone has no arms for those commands and used to exit
        // "not yet supported". trace:TASK-1487 | ai:claude
        if explicit_path.is_dir()
            || (!explicit_path.exists() && explicit_path.extension().is_none())
        {
            // The explicit --file directory IS the project root here — there's
            // no separate project to walk cwd up to, and cwd may not even be
            // inside it (or may be inside some unrelated git repo). Pass it
            // through explicitly so `mcp-serve` doesn't derive the wrong
            // project root from cwd. trace:TASK-1487 | ai:claude
            return run_on_distributed_store(&cli.command, &explicit_path, Some(&explicit_path));
        }
        // User explicitly specified a file path - use it directly
        explicit_path
    } else {
        // Check for distributed mode config (.aida/config.toml with store_path).
        // The Jira / GitHub / GitLab tracker commands go through the same
        // distributed-first resolution as everything else: they used to skip
        // it, so inside a distributed project with no local db they fell to the
        // legacy resolver and wrongly reported there was no `.aida/config.toml`.
        // trace:TASK-1486 | ai:claude
        // BUG-2221: store discovery is its own `show` phase so the walk-up /
        // config read cost is attributable. trace:BUG-2221 | ai:claude
        let discovery_phase = crate::usage::phase("discovery");
        let detected_store = detect_distributed_store();
        drop(discovery_phase);
        if let Some(store_path) = detected_store {
            return run_on_distributed_store(&cli.command, &store_path, None);
        }
        // Distributed mode is declared in `.aida/config.toml` but the store
        // worktree isn't resolvable here — the hallmark of a freshly-cloned
        // AIDA project (`.aida-store/` is gitignored and only created by
        // `aida init`). We must NOT silently fall back to the legacy
        // requirements.yaml/SQLite: that shows STALE data with no signal
        // it's wrong (a first-user lands on someone's pre-migration specs
        // and never knows). trace:BUG-428 | ai:claude
        // BUG-442 / TASK-621: a project is distributed if local
        // `.aida/config.toml` declares it OR — on a FRESH CLONE with no
        // local config yet — the `aida-store` branch exists (origin or
        // local). Detecting from the git ref is what makes auto-attach fire
        // on a fresh clone AND, critically, prevents falling through to
        // `determine_requirements_path`'s global-registry fallback, which
        // would SILENTLY read an UNRELATED project's store (the worst
        // failure mode — wrong data, no warning). BUG-428's refuse-fallback
        // guard only covered the *declared* case; a fresh clone isn't
        // declared yet, which is exactly the gap this closes.
        // trace:BUG-442 trace:TASK-621 trace:BUG-428 | ai:claude
        let distributed_root = distributed_mode_declared().or_else(|| {
            let root = find_project_root().ok()?;
            if branch_exists_anywhere(&root, "aida-store") {
                return Some(root);
            }
            // BUG-433: the store is physically PRESENT here (`.aida-store/`
            // with an `objects/` dir — even via a symlink to the main store)
            // but there's no `.aida/config.toml` marker AND no detectable
            // `aida-store` branch — e.g. a session worktree forked from a
            // commit that predates the committed scaffolding, or a symlink
            // into another repo's store. Detect distributed mode from the
            // store's SHAPE so we use the real store instead of silently
            // falling through to the legacy backend (which serves
            // stale/unrelated data with no signal — the inverse of BUG-428).
            // trace:BUG-433 | ai:claude
            attached_store_present(&root).then_some(root)
        });
        if let Some(project_root) = distributed_root {
            // An already-attached fresh clone (no config, but the worktree
            // exists from a prior auto-attach) must be used directly — no
            // re-fetch, no repeated "attached…" noise on every command.
            let existing_worktree = project_root.join(".aida-store");
            let store_path_opt = if attached_store_present(&project_root) {
                Some(existing_worktree)
            } else if branch_exists_anywhere(&project_root, "aida-store") {
                // Auto-attach the store worktree from the `aida-store` branch
                // (the same fetch + worktree-add the post-clone init does).
                // Node-id / scaffolding stay with `aida init` / `aida node
                // acquire`: a node id is needed before WRITING, not reading.
                match try_attach_store_worktree(&project_root) {
                    Ok(store_path) => {
                        eprintln!(
                            "{} attached the AIDA store worktree from `aida-store`. \
                             Run `{}` to claim a node id before issuing new IDs (`aida add`).",
                            "Note:".dimmed(),
                            "aida node acquire".cyan()
                        );
                        Some(store_path)
                    }
                    // Auto-attach failed (offline, diverged/locked branch,
                    // git too old, …). Don't fall to legacy/ambient — drop to
                    // the explicit setup guidance below, naming the cause.
                    Err(e) => {
                        eprintln!(
                            "{} couldn't auto-attach the store worktree: {}",
                            "Note:".dimmed(),
                            e
                        );
                        None
                    }
                }
            } else {
                None
            };
            if let Some(store_path) = store_path_opt {
                return run_on_distributed_store(&cli.command, &store_path, None);
            }
            let on_store_ref = branch_exists_anywhere(&project_root, "aida-store");
            let branch_hint = if on_store_ref {
                "\n  Its `aida-store` branch is available, ready to attach."
            } else {
                ""
            };
            anyhow::bail!(
                "This is a distributed AIDA project, but its store isn't set up in \
                 this working copy yet (no `.aida-store/` worktree).{branch_hint}\n\n  \
                 Run `aida init` to attach it — it creates the `.aida-store` worktree \
                 from the `aida-store` branch and rebuilds the cache.\n\n  \
                 (Refusing to fall back to a legacy or ambient requirements store: \
                 that would show stale or UNRELATED data with no indication it's wrong.)"
            );
        }

        // Auto-detect: first find the base path, then check migration status
        let initial_path = determine_requirements_path(cli.globals.project.as_deref())?;

        // If path is already a .db file, skip migration check (no YAML to migrate from)
        if initial_path.extension().and_then(|e| e.to_str()) == Some("db") {
            initial_path
        } else {
            // Check for migration status (REQ-0231)
            // Storage class now auto-detects SQLite vs YAML by file extension
            match check_migration_status(&initial_path) {
                MigrationCheck::NoMigration(path) => path,
                MigrationCheck::MigratedToSqlite {
                    yaml_path: _,
                    sqlite_path,
                } => {
                    // YAML was officially migrated - use SQLite
                    eprintln!(
                        "{}: Using SQLite database: {}",
                        "INFO".blue(),
                        sqlite_path.display()
                    );
                    sqlite_path
                }
                MigrationCheck::PossibleStaleYaml {
                    yaml_path: _,
                    sqlite_path,
                } => {
                    // Both exist but no marker - default to SQLite as it's likely more current
                    eprintln!(
                        "{}: Both YAML and SQLite exist. Using SQLite.",
                        "INFO".blue()
                    );
                    eprintln!("Use --file requirements.yaml to use YAML instead.");
                    sqlite_path
                }
            }
        }
    };

    let storage = Storage::new(requirements_path.clone());

    match &cli.command {
        // trace:STORY-1728 | ai:claude
        Command::Registered(invocation) => {
            let store = crate::command_registry::StoreCtx::Legacy {
                storage: &storage,
                requirements_path: &requirements_path,
            };
            if invocation.run_store(store, Some(&cli.globals))?
                == crate::command_registry::Flow::Return
            {
                return Ok(());
            }
        }
    }

    Ok(())
}

// ===================================================================
// `aida questions` — the async decision inbox (STORY-522, slice 1).
//
// The advisor distills a fork it can't resolve into a structured
// DecisionRequest on the spec; the human batch-answers it OUTSIDE any
// agent (plain CLI, no LLM session). Slice 1 RECORDS the answer (a pure
// data op); the loop-resume auto-applier that applies the chosen
// resolution token is DEFERRED per the operator decision.
// trace:STORY-522 | ai:claude
// ===================================================================

// ===================================================================
// STORY-555: the slice-2 auto-applier. The slice-1 `record_answer`
// only recorded the chosen index; here, answering a decision APPLIES
// the chosen resolution token (clears a gate / binds a decision /
// rejects), re-checks pickability, and — when the spec is now genuinely
// decision-free + unblocked — auto-queues it onto the burndown ready
// set. This closes the `aida questions` -> `aida burndown` loop: the
// human-decision drain is the symmetric complement of the autonomous
// drain. trace:STORY-555 | ai:claude
// ===================================================================

/// BUG-1186: the PR head SHA as the forge reports it right now, or `None` on
/// any forge fault. Used both to anchor the reviewer prompt (BUG-868) and as
/// the pre/post pair for the post-review integrity guard.
// trace:BUG-1186 | ai:claude
pub(crate) fn pr_head_sha_best_effort(driver: &RealPhaseDriver, pr: u32) -> Option<String> {
    let mut sink = crate::network_retry::NoopSink;
    driver
        .lifecycle_forge()
        .change_metadata(pr as u64, &mut sink)
        .ok()
        .map(|m| m.head_sha)
        .filter(|s| !s.trim().is_empty())
}

/// STORY-439: `aida autonomy calibration mismatches` — the substrate-gap
/// signal. Walks `.aida/complexity-calibration/*.yaml`, drops records
/// missing a pickup-or-review half, and surfaces the rest ranked by
/// `|delta_steps|` descending. Tied gaps break by recency. The mismatch
/// view IS the calibration view this STORY adds; the broader autonomy
/// report (TASK-340) gains `--by` / `--calibration` slices in a
/// follow-up that hangs off this same parent enum.
/// trace:STORY-439 | ai:claude
/// Add a promoted finding to a role's work queue.
///
/// Findings are follow-ups that usually need an implementer, so the default
/// route is the `implementer` queue; `for_override` (the `--for` flag) picks
/// a different role. Returns the role it routed to so the caller can name it
/// in the success message.
///
/// Kept separate from the status flip in `FindingsCommand::Promote` so a
/// queue-add failure surfaces as a non-zero exit *before* the finding is
/// marked Approved — BUG-231 was a silent "Approved but in no queue"
/// half-state because the old promote path flipped status and printed
/// "joins the work queue" without ever calling `queue add`.
/// trace:BUG-231 | ai:claude
///
/// BUG-1651: production promotes go through [`queue_promoted_finding_entry`];
/// this role-returning form remains for the BUG-231 tests.
// trace:BUG-1651 | ai:claude
#[cfg(test)]
pub(crate) fn queue_promoted_finding(
    store_path: &std::path::Path,
    requirement_id: Uuid,
    display_id: &str,
    for_override: Option<&str>,
) -> Result<String> {
    queue_promoted_finding_entry(store_path, requirement_id, display_id, for_override)
        .map(|entry| entry.for_role.unwrap_or_default())
}

// ──────────────────────────────────────────────────────────────────────────
// STORY-410: existing-project substrate-drift discovery.
//
// `aida init --with-memories --refresh` (STORY-255) lets a project overlay
// newer scaffolding-pack memories onto its existing dir, preserving user
// edits via body-checksum. But the user has to KNOW the pack is stale to run
// it. `aida memories check` is the discoverability surface: it compares the
// local memory dir against the binary's embedded master pack and reports
// drift, without writing anything. `aida status` surfaces a one-line summary
// when the pack is significantly behind.
// trace:STORY-410 | ai:claude
// ──────────────────────────────────────────────────────────────────────────

/// Append AIDA's `.gitignore` entries if any are missing. Returns `true` if a
/// new entry was written (so callers can echo "updated .gitignore"); `false`
/// if everything was already covered or the file had to be created from
/// scratch.
///
/// Writes two blocks:
/// - `<worktree_dir>/` (+ bare symlink form) — the orphan-branch worktree
///   (`.aida-store/` by default). Bare-name pattern needed because session
///   worktrees link to the canonical store. trace:EPIC-21 | ai:claude
/// - `.aida/*` deny-by-default + config allow-list entries — covers every
///   per-clone runtime file under `.aida/` (sessions,
///   roles, cache, session-env, server data, review prompts, future runtime
///   additions) without per-file whack-a-mole, while keeping team-shareable
///   project config tracked. trace:BUG-73 trace:TASK-877 | ai:claude
///
/// Migration: when invoked against a `.gitignore` from an older AIDA scaffold
/// (which listed each runtime path individually), the deny block is appended.
/// Legacy per-file entries become redundant but remain harmless.
pub(crate) fn add_aida_gitignore_entries(
    cwd: &std::path::Path,
    worktree_dir: &str,
) -> Result<bool> {
    use std::io::Write;
    let gitignore_path = cwd.join(".gitignore");
    let store_entry = format!(
        "\n# AIDA distributed store (orphan branch worktree)\n\
         # Bare-name pattern catches session-worktree symlinks back to the\n\
         # canonical store. trace:EPIC-21 | ai:claude\n\
         {0}/\n\
         {0}\n",
        worktree_dir
    );
    let runtime_entry = "\n# AIDA runtime state — deny-by-default. Anything under .aida/ is\n\
         # per-clone runtime state (cache, sessions, roles, env shims, review\n\
         # prompts, server data, etc.) unless explicitly allow-listed below.\n\
         # Adding a new runtime file under .aida/ requires no gitignore change;\n\
         # tracking a new project-config file requires an explicit `!` line.\n\
         # trace:BUG-73 | ai:claude\n\
         .aida/*\n\
         \n\
         # Tracked exceptions: project-level config that lives in the repo.\n\
         !.aida/config.toml\n\
         # Project-level command aliases are shareable across the team.\n\
         # trace:TASK-877 | ai:claude\n\
         !.aida/aliases.toml\n\
         \n\
         # Per-project skill extensions are tracked project assets, not\n\
         # runtime state — they live under .claude/ which is tracked by\n\
         # default. Do not add ignore rules for .claude/skills/local/ or\n\
         # any <skill>.local.md file; the whole team should pick them up on\n\
         # `git pull`. See docs/extending-skills.md. trace:STORY-305 | ai:claude\n";

    // trace:STORY-781 | ai:claude — the project manifest is a TRACKED
    // project-config file under .aida/, so the deny-by-default `.aida/*` rule
    // needs an explicit allow-line or the manifest is silently gitignored and
    // never checked in — which would defeat the entire point of a manifest
    // that travels with the repository.
    //
    // This is deliberately a SEPARATE append rather than another line inside
    // `runtime_entry`: that block is only written when `.aida/*` is absent, so
    // an already-initialized project would never receive a newly-added
    // allow-line. Every existing repo needs this on its next `aida init`.
    let project_manifest_entry =
        "\n# The AIDA project manifest is checked in: it describes what this\n\
         # project IS (description, why it exists, liveness, stage, owner) and\n\
         # is meant to travel with the repository. trace:STORY-781 | ai:claude\n\
         !.aida/project.toml\n";

    let discipline_pack_entry =
        "\n# AIDA-using discipline guides are checked in with the project.\n\
         # trace:STORY-829 | ai:codex\n\
         !.aida/discipline/\n\
         !.aida/discipline/**\n";

    // trace:TASK-572 | ai:claude — CLAUDE.local.md is per-machine,
    // gitignored. Pattern mirrors CLAUDE.md but personal notes are
    // never team-shared. Comment block explains the convention so
    // operators new to the file see why.
    let claude_local_entry = "\n# Personal Claude Code notes — per-machine, never team-shared.\n\
         # CLAUDE.local.md loads alongside CLAUDE.md at session start but\n\
         # stays out of git so reviewer-feedback dumps + personal-habit\n\
         # reminders stay private. See the scaffolded file's own header\n\
         # for usage guidance.\n\
         CLAUDE.local.md\n";

    // trace:SPIKE-31 | ai:claude — `aida rules sync` regenerates path-
    // gated Claude Code rules from the spec graph. Committed copies would
    // thrash across worktrees as specs flip Active/Completed; keep them
    // per-clone.
    let rules_sync_entry = "\n# AIDA path-gated rules generated by `aida rules sync` — per-clone\n\
         # derived state. Hand-authored rules outside this subdir stay\n\
         # tracked. trace:SPIKE-31 | ai:claude\n\
         .claude/rules/aida-specs/\n";

    // trace:TASK-383 | ai:claude — a "planning pass" (a loop of `/aida-plan`
    // over queue items run from main) writes scratch plans to
    // docs/plans/_draft/. Gitignored so they never linger as untracked files
    // that abort a later PR's `git pull --ff-only` when the implementer's plan
    // lands at docs/plans/<same>.md. Promote a draft to docs/plans/ when adopted.
    let plans_draft_entry = "\n# AIDA planning-pass drafts — scratch plans; promote to\n\
         # docs/plans/<name>.md when adopted. Gitignored so they don't conflict\n\
         # with a later PR's landed plan. trace:TASK-383 | ai:claude\n\
         docs/plans/_draft/\n";

    // trace:BUG-484 — .claude/settings.local.json holds the per-user MCP
    // pre-approval (enabledMcpjsonServers: ["aida"]) that init scaffolds so a
    // fresh project trusts its own .mcp.json server. It MUST stay gitignored:
    // a committed pre-approval is the exact clone-attack vector Claude Code
    // guards against. Each clone's `aida init` grants its own local trust.
    let settings_local_entry = "\n# Per-user Claude Code overrides — never team-shared.\n\
         # .claude/settings.local.json holds local MCP trust (the aida server\n\
         # pre-approval) and personal settings. Gitignored so a committed\n\
         # pre-approval can't become a clone-attack vector. trace:BUG-484\n\
         .claude/settings.local.json\n";

    // trace:TASK-1744 | ai:antigravity — `.agents/hooks/` and `.agents/hooks.json`
    // hold Antigravity hook handler copies and wiring generated on `aida init`.
    let agents_hooks_entry =
        "\n# Antigravity hook wiring and handler copies generated by `aida init` —\n\
         # machine-local output repaired from the embedded templates.\n\
         # trace:TASK-1744 | ai:antigravity\n\
         .agents/hooks/\n\
         .agents/hooks.json\n";

    if !gitignore_path.exists() {
        std::fs::write(
            &gitignore_path,
            format!(
                "{}{}{}{}{}{}{}{}{}",
                store_entry,
                runtime_entry,
                project_manifest_entry,
                discipline_pack_entry,
                claude_local_entry,
                rules_sync_entry,
                plans_draft_entry,
                settings_local_entry,
                agents_hooks_entry
            ),
        )?;
        return Ok(false);
    }

    let content = std::fs::read_to_string(&gitignore_path)?;
    let mut wrote = false;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&gitignore_path)?;
    if !content.contains(worktree_dir) {
        file.write_all(store_entry.as_bytes())?;
        wrote = true;
    }
    if !has_aida_runtime_deny_pattern(&content) {
        file.write_all(runtime_entry.as_bytes())?;
        wrote = true;
    }
    // trace:STORY-781 — same idempotency pattern: match the bare path so an
    // operator-added variant doesn't produce a duplicate. Appended even when
    // the runtime block already exists, which is the whole point.
    if !content.lines().any(|line| {
        line.trim()
            .trim_start_matches('!')
            .trim_end_matches('/')
            .ends_with(".aida/project.toml")
    }) {
        file.write_all(project_manifest_entry.as_bytes())?;
        wrote = true;
    }
    if !gitignore_has_discipline_pack_allow_list(&content) {
        file.write_all(discipline_pack_entry.as_bytes())?;
        wrote = true;
    }
    // trace:TASK-572 | ai:claude — only append if not already covered.
    // Match against the bare filename to detect both this scaffolded
    // form and operator-added entries like `**/CLAUDE.local.md`.
    if !content.lines().any(|line| {
        line.trim()
            .trim_end_matches('/')
            .ends_with("CLAUDE.local.md")
    }) {
        file.write_all(claude_local_entry.as_bytes())?;
        wrote = true;
    }
    // trace:SPIKE-31 | ai:claude — same idempotency pattern as
    // CLAUDE.local.md: scan for the bare path so operator-added globs
    // (e.g. `**/.claude/rules/aida-specs/`) don't trigger a duplicate.
    if !content.lines().any(|line| {
        line.trim()
            .trim_end_matches('/')
            .ends_with(".claude/rules/aida-specs")
    }) {
        file.write_all(rules_sync_entry.as_bytes())?;
        wrote = true;
    }
    // trace:TASK-383 | ai:claude — same idempotency pattern; match the bare
    // path so an operator-added glob doesn't trigger a duplicate.
    if !content.lines().any(|line| {
        line.trim()
            .trim_end_matches('/')
            .ends_with("docs/plans/_draft")
    }) {
        file.write_all(plans_draft_entry.as_bytes())?;
        wrote = true;
    }
    // trace:BUG-484 — same idempotency pattern; match the bare path so an
    // operator-added glob (e.g. `**/settings.local.json`) doesn't duplicate it.
    if !content.lines().any(|line| {
        line.trim()
            .trim_end_matches('/')
            .ends_with("settings.local.json")
    }) {
        file.write_all(settings_local_entry.as_bytes())?;
        wrote = true;
    }
    // trace:TASK-1744 | ai:antigravity
    let has_agents_umbrella = content.lines().any(|line| {
        let trimmed = line.trim().trim_end_matches('/');
        trimmed == ".agents" || trimmed == "/.agents"
    });
    let has_agents_hooks_dir = has_agents_umbrella
        || content
            .lines()
            .any(|line| line.trim().trim_end_matches('/').ends_with(".agents/hooks"));
    let has_agents_hooks_json = has_agents_umbrella
        || content
            .lines()
            .any(|line| line.trim().ends_with(".agents/hooks.json"));
    if !has_agents_hooks_dir && !has_agents_hooks_json {
        file.write_all(agents_hooks_entry.as_bytes())?;
        wrote = true;
    } else if !has_agents_hooks_dir {
        file.write_all(b".agents/hooks/\n")?;
        wrote = true;
    } else if !has_agents_hooks_json {
        file.write_all(b".agents/hooks.json\n")?;
        wrote = true;
    }
    Ok(wrote)
}

// trace:BUG-588 | ai:claude
#[cfg(test)]
#[path = "tests/bug_588_history_id_resolves_uuid_tests.rs"]
mod bug_588_history_id_resolves_uuid_tests;

// trace:TASK-1480 | ai:claude
#[cfg(test)]
#[path = "tests/task_1480_history_id_alias_tests.rs"]
mod task_1480_history_id_alias_tests;

// trace:TASK-1507 | ai:claude
#[cfg(test)]
#[path = "tests/task_1507_history_cache_tests.rs"]
mod task_1507_history_cache_tests;

// trace:TASK-1508 | ai:claude
#[cfg(test)]
#[path = "tests/task_1508_history_source_tests.rs"]
mod task_1508_history_source_tests;

// trace:STORY-1650 | ai:claude
#[cfg(test)]
#[path = "tests/story_1650_history_flow_tests.rs"]
mod story_1650_history_flow_tests;

// trace:BUG-1631 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1631_history_spec_id_tests.rs"]
mod bug_1631_history_spec_id_tests;

// trace:BUG-1635 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1635_history_event_modes_tests.rs"]
mod bug_1635_history_event_modes_tests;

// trace:TASK-1512 | ai:claude
#[cfg(test)]
#[path = "tests/task_1512_history_transition_filters_tests.rs"]
mod task_1512_history_transition_filters_tests;

/// Classification of an `AIDA_STORE` value: either it resolves to a usable
/// store, or it's set-but-unusable with a specific reason. Lets the
/// resolution core stay pure/unit-testable while the env wrapper decides
/// whether to print the BUG-567 fall-through notice. Re-exported from
/// `aida-core::store_locate`, the canonical resolver both this CLI and
/// `aida-tui` route through.
// trace:BUG-567 trace:TASK-1141 | ai:claude
use aida_core::store_locate::StoreOverride;

pub(crate) fn read_review_mode(project_root: &std::path::Path) -> String {
    let path = project_root.join(".aida").join("config.toml");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return "local".to_string();
    };
    parse_review_mode(&content)
}

pub(crate) fn parse_review_mode(content: &str) -> String {
    let mut in_review = false;
    for raw in content.lines() {
        let line = match raw.split('#').next() {
            Some(l) => l.trim(),
            None => continue,
        };
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            in_review = rest.trim_end_matches(']').trim() == "review";
            continue;
        }
        if !in_review {
            continue;
        }
        if let Some(rest) = line.strip_prefix("mode") {
            let val = match rest.split('=').nth(1) {
                Some(v) => v.trim().trim_matches('"').trim_matches('\'').to_string(),
                None => continue,
            };
            return val;
        }
    }
    "local".to_string()
}

#[cfg(test)]
#[path = "tests/delegated_reviewer_tests.rs"]
mod delegated_reviewer_tests;

pub(crate) fn command_triggers_per_write_auto_push(command: &Command) -> bool {
    // trace:STORY-1728 | ai:claude
    let Command::Registered(invocation) = command;
    invocation.policy().per_write_auto_push
}

/// Validate a status string against the canonical set. Accepts case-
/// insensitive matches and common spelling variants (`in-progress`,
/// `inprogress`, `in_progress`). Returns Ok with the canonical form, or
/// Err with a list-of-valid-values message. Use at the CLI layer before
/// calling `Requirement::set_status_from_str` to prevent typos like
/// `approvedxxx` from silently landing as a `custom_status`. trace:BUG-47
/// The canonical status list echoed by every "invalid status" refusal, so the
/// plain validator and its type-aware wrapper can never disagree on what the
/// CLI accepts.
// trace:TASK-1176 | ai:claude
pub(crate) const VALID_STATUS_INPUTS: &str =
    "draft, approved, planned, in-progress, done, completed, rejected, superseded, needs-attention";

#[cfg(test)]
#[path = "tests/story_569_review_brief_tests.rs"]
mod story_569_review_brief_tests;

#[cfg(test)]
#[path = "tests/task_760_intake_config_section_tests.rs"]
mod task_760_intake_config_section_tests;

#[cfg(test)]
#[path = "tests/task_1021_autopilot_config_section_tests.rs"]
mod task_1021_autopilot_config_section_tests;

#[cfg(test)]
#[path = "tests/task_304_ultraplan_cadence_tests.rs"]
mod task_304_ultraplan_cadence_tests;

#[cfg(test)]
#[path = "tests/story_760_store_mirror_tests.rs"]
mod story_760_store_mirror_tests;

#[cfg(test)]
#[path = "tests/story_441_archive_config_tests.rs"]
mod story_441_archive_config_tests;

#[cfg(test)]
#[path = "tests/story_284_store_sync_tests.rs"]
mod story_284_store_sync_tests;
