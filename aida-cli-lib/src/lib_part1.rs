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
// trace:TASK-1562 | ai:claude
#[cfg(test)]
#[path = "tests/task_1562_worktree_reclaim_tests.rs"]
mod task_1562_worktree_reclaim_tests;
// trace:BUG-1723 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1723_config_trust_tests.rs"]
mod bug_1723_config_trust_tests;

/// The whole CLI: sigpipe setup, telemetry wrapping, error rendering,
/// dispatch. The `aida` binary is a stub that calls this — keeping the
/// entry point in the lib means only this one symbol is `pub` and every
/// `pub(crate)` item stays crate-private.
// trace:STORY-772 trace:ADR-16 | ai:claude
pub fn main_entry() {
    // trace:BUG-1960 | ai:claude
    // First, while still single-threaded: take the wrapper's init side-channel
    // path out of the environment so no child process inherits it.
    crate::shell_eval::take_init_cd_file_from_env();
    install_sigpipe_handler();
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

    // trace:TASK-1526 | ai:codex
    cache_output::finish(&cache_scope, exit_code == 0, &argv);

    // STORY-122: append the usage record after the command completed
    // so we capture exit_code / duration_ms. Read the env-side opt-out
    // first; checking the project's `.aida/config.toml` requires a
    // project root, which not every invocation has, so fall back to
    // "no project context" gracefully.
    let project_root = find_main_worktree_root().ok();
    if usage::is_enabled(project_root.as_deref()) {
        let ev = usage::UsageEvent {
            ts: chrono::Utc::now().to_rfc3339(),
            cmd: usage::derive_cmd_shape(&argv),
            args_count: usage::count_args(&argv),
            exit_code,
            duration_ms: started.elapsed().as_millis() as u64,
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
            phases: usage::take_phases(),
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

    use clap::CommandFactory;
    let root = Cli::command();
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
];

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
        // trace:FR-283 | ai:claude
        "weight" => "Weight",
        "queued" => "Queued",
        "in_flight" => "In-Flight",
        "blocked" => "Blocked",
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
    // Plain (uncoloured) cell text per row per column — width math runs on this.
    let plain: Vec<Vec<String>> = reqs
        .iter()
        .map(|r| {
            let routing = routing_of(r);
            fields
                .iter()
                .map(|f| toon_list_cell(r, routing, f))
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
/// false. Returned as a `String` so the branch is unit-testable.
// trace:STORY-734 trace:BUG-668 | ai:claude — plain `//` keeps the marker out of any doc/help.
pub(crate) fn render_search_fields(
    results: &[aida_core::RequirementSummary],
    selected: &[String],
    agent: bool,
) -> String {
    if agent {
        let field_refs: Vec<&str> = selected.iter().map(String::as_str).collect();
        let rows: Vec<Vec<String>> = results
            .iter()
            .map(|r| {
                selected
                    .iter()
                    .map(|f| toon_list_cell(r, (false, false, false), f))
                    .collect()
            })
            .collect();
        let mut out = format!("count: {} results\n", results.len());
        out.push_str(&crate::toon::table_raw("specs", &field_refs, &rows));
        out
    } else {
        let mut lines = build_list_fields_lines(results, selected, |_| (false, false, false));
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
    let cast_path = resolve_cast_output_path(cli.cast_out.as_ref(), &inner_args)?;
    if let Some(parent) = cast_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating cast output directory {}", parent.display()))?;
    }
    let title = cli
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
/// recognized subcommands (`new`, `register`, `ls`, `status`, `gc`,
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
            "ls",
            "status",
            "gc",
            "dispatch-health",
            "pause",
            "resume",
            "stop",
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
    let (after_alias_rewrites, groom_deprecated_verb) =
        rewrite_groom_alias(&rewrite_queue_default_list(&rewrite_agent_default_new(
            &rewrite_list_alias(&rewrite_type_alias(&rewrite_advisor_assess(
                &rewrite_personal_view_alias(&expanded),
            ))),
        )));
    // Git-style extensions: a head that is still not a real subcommand after
    // every built-in and user-alias stage runs `aida-<name>` from PATH.
    // trace:STORY-1641 | ai:claude
    if let Some(code) = external_cmd::maybe_dispatch(&after_alias_rewrites)? {
        std::process::exit(code);
    }
    let mut cli = Cli::parse_from(after_alias_rewrites.clone());

    // STORY-764: install the explicit output-format pin before any handler
    // renders. `--format` wins; else `AIDA_OUTPUT_FORMAT`; else the TTY-based
    // default stands. trace:STORY-764 | ai:claude
    set_output_format_override(cli.format);
    // trace:TASK-1526 | ai:codex
    cache_output::set_toon_cache_output(matches!(
        &cli.command,
        Command::History { .. } | Command::Graph { .. }
    ));
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
    let single_spec_read = matches!(&cli.command, Command::Show { .. });
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
        cli = Cli::parse_from(dispatch_argv);
    }

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

    if cli.asciinema && std::env::var_os(ASCIINEMA_WRAPPED_ENV).is_none() {
        if let Some(exit_code) = maybe_run_asciinema_wrapper(&raw_args, &cli)? {
            std::process::exit(exit_code);
        }
    }

    // Check for AIDA_SERVER environment variable if --server not specified
    if cli.server.is_none() {
        cli.server = std::env::var("AIDA_SERVER").ok();
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

    // Handle init before path resolution (no DB exists yet)
    if let Command::Init {
        no_skills,
        with_mcp,
        agent,
        no_hooks,
        no_post_hooks,
        no_roles,
        no_agent_config,
        no_schedule,
        force,
        footprint,
        distributed: _,
        centralized,
        sibling,
        attach,
        store_path,
        registry_remote,
        verbose,
        name,
        with_memories,
        refresh,
        focus,
        git_init,
        commit_scaffold,
        node_name,
        minimal,
        dir,
        lang,
        github,
        public,
        remote,
        forge,
        gitlab,
    } = &cli.command
    {
        // STORY-757: the markdown-only first run — scaffold just a `specs/`
        // folder + a runnable `aida why` demo, none of the orphan-branch / cache
        // / MCP / skills / roles machine. Proves the 60-second magic needs no
        // setup. Short-circuits before every heavy init path. trace:STORY-757
        if *minimal {
            handle_init_minimal(*force)?;
            return Ok(());
        }
        // STORY-780: `aida init <DIR>` bootstraps a brand-new project. The
        // pre-steps (mkdir → native-tool scaffold → git init → first commit)
        // run from the caller's cwd, then cwd moves INTO the new dir so the
        // entire standard init below runs there unchanged; the remote/push
        // post-steps run at the end of this block, after the store branch
        // exists. trace:STORY-780 | ai:claude
        let bootstrap: Option<init_bootstrap::BootstrapPlan> = match dir {
            Some(d) => {
                let mut plan = init_bootstrap::BootstrapPlan {
                    dir: std::path::PathBuf::from(d),
                    lang: lang
                        .as_deref()
                        .map(init_bootstrap::Lang::parse)
                        .transpose()?,
                    github: *github,
                    public: *public,
                    remote: remote.clone(),
                    forge: forge.clone(),
                    github_repo: None,
                    // trace:STORY-1656 | ai:claude
                    gitlab: gitlab
                        .as_deref()
                        .map(gitlab_project_create::GitLabHub::parse)
                        .transpose()?,
                };
                init_bootstrap::resolve_forge_profile(&mut plan)?;
                init_bootstrap::preflight(&plan)?;
                init_bootstrap::create_and_scaffold(&plan)?;
                std::env::set_current_dir(&plan.dir)
                    .with_context(|| format!("could not enter {}", plan.dir.display()))?;
                Some(plan)
            }
            None => None,
        };
        // Default: distributed (git-canonical) mode per EPIC-1-001.
        // --sibling implies distributed-sibling. --centralized opts into
        // the deprecated SQLite-canonical path.
        // trace:EPIC-1-001 | ai:claude
        if *centralized {
            init_cmd::handle_init_command(
                *no_skills,
                agent.as_deref(),
                *with_mcp,
                *no_hooks,
                *force,
                *verbose,
                name.as_deref(),
                *footprint,
            )?;
        } else if *sibling || store_path.is_some() {
            // STORY-676: --store-path implies the sibling storage model at an
            // explicit path; --sibling is sugar for --store-path ../aida-store.
            init_cmd::handle_init_distributed_sibling(
                registry_remote.as_deref(),
                *force,
                *attach,
                store_path.as_deref(),
                *no_skills,
                agent.as_deref(),
                *with_mcp,
                *no_hooks,
                *verbose,
                name.as_deref(),
                *footprint,
            )?;
        } else {
            init_cmd::handle_init_distributed_worktree(
                *force,
                *no_skills,
                agent.as_deref(),
                *with_mcp,
                *no_hooks,
                *verbose,
                name.as_deref(),
                *git_init,
                *commit_scaffold,
                node_name.as_deref(),
                *footprint,
            )?;
        }
        // TASK-638: bootstrap the default GLOBAL role set so a fresh machine is
        // ready out of the box — consistent with how init already scaffolds
        // skills / hooks / discipline-pack. Idempotent + non-destructive
        // (existing roles preserved). The write lands in GLOBAL ~/.aida/roles/
        // from a project-scoped command, so report it explicitly. Non-fatal:
        // a hiccup writing global state must not abort an otherwise-successful
        // init. trace:TASK-638 | ai:claude
        let init_footprint =
            init_cmd::resolve_init_footprint(&statusline_project_root(), *footprint)?;
        if !*no_roles && init_footprint == cli::InitFootprint::Full {
            match scaffold_starter_roles(&statusline_project_root()) {
                Ok((created, skipped)) => {
                    println!(
                        "  {} global roles installed: implementer, product, advisor, reviewer, integrator",
                        crate::glyph(crate::glyphs::Glyph::Check).green()
                    );
                    if created.is_empty() {
                        println!(
                            "  {} starter global roles already present at ~/.aida/roles/ ({} role(s))",
                            "Note:".dimmed(),
                            skipped.len()
                        );
                    } else {
                        println!(
                            "  {} scaffolded {} starter global role(s) at ~/.aida/roles/: {}",
                            "+".green(),
                            created.len(),
                            created.join(", ")
                        );
                    }
                }
                Err(e) => {
                    eprintln!("  {} starter roles skipped: {}", "Note:".dimmed(), e);
                }
            }
        }
        // TASK-698 / TASK-1233: first-machine setup plus existing-project
        // posture repair. The first-machine prompt writes GLOBAL ~/.aida/ and,
        // for contained, the matching ~/.codex/config.toml sandbox table. A
        // re-init over existing config never re-prompts unless the read-only
        // posture report already flags an incoherent state, and even then only
        // at a TTY. Non-interactive / --no-agent-config init writes nothing.
        // trace:TASK-698 trace:TASK-1233 | ai:codex
        if !*no_agent_config && init_footprint == cli::InitFootprint::Full {
            if let Err(e) = maybe_prompt_agent_posture() {
                eprintln!(
                    "  {} agent permission posture skipped: {}",
                    "Note:".dimmed(),
                    e
                );
            }
            if let Err(e) =
                config_cmd::maybe_offer_permission_posture_fix(&statusline_project_root())
            {
                eprintln!(
                    "  {} agent permission posture check skipped: {}",
                    "Note:".dimmed(),
                    e
                );
            }
            if let Err(e) = maybe_prompt_confirm_bypass() {
                eprintln!(
                    "  {} bypass confirmation setting skipped: {}",
                    "Note:".dimmed(),
                    e
                );
            }
        }
        if init_footprint == cli::InitFootprint::Full {
            let setting = bypass_confirm::load(&statusline_project_root());
            println!(
                "  Agent bypass confirmation: {} ({})",
                if setting.on { "on" } else { "off" },
                setting.source
            );
        }
        // STORY-1463: a registered `[schedule]` job only ever runs when
        // something invokes `aida schedule tick`; nothing does that by
        // default. At a TTY, offer to install the crontab entry that drives
        // it (default: no — writing to the operator's crontab is a real
        // side effect). Non-interactive init and --no-schedule never prompt
        // and never install anything. trace:STORY-1463 | ai:claude
        if !*no_schedule && init_footprint == cli::InitFootprint::Full {
            maintenance_schedule::maybe_offer_tick_install(&statusline_project_root());
        }
        // STORY-831: minimal-footprint projects intentionally do not commit
        // agent instruction files, so the machine-global awareness snippet is
        // the path that lets a fresh agent notice AIDA. Keep every other
        // first-machine global setup full-only, but offer this idempotent
        // snippet for both full and minimal TTY init. Non-interactive init and
        // a declined prompt leave user files untouched.
        if let Err(e) = maybe_offer_user_aida_instructions() {
            eprintln!(
                "  {} user AIDA instructions skipped: {}",
                "Note:".dimmed(),
                e
            );
        }
        // Starter memory pack — opt-in, orthogonal to storage mode. Failure
        // is non-fatal: it writes outside the project root, so a hiccup
        // there must not abort an otherwise-successful init. trace:STORY-255
        if (*with_memories || *refresh) && init_footprint == cli::InitFootprint::Full {
            // STORY-362: --focus <subsystem> scopes the pack to universal
            // (untagged) memories plus those whose `subsystem:` frontmatter
            // matches. trace:STORY-362 | ai:claude
            if let Err(e) = scaffold_memory_pack(*refresh, focus.as_deref()) {
                eprintln!("  {} starter memory pack skipped: {}", "Note:".dimmed(), e);
            }
        }
        // TASK-1170: --refresh is not memory-pack-only any more. The same
        // edit-preserving overlay now converges every installed agent pack
        // (Claude skills/commands, Codex skills, Antigravity skills, and the
        // machine-global Codex custom prompts) onto this binary's templates,
        // so a template fix reaches a non-Claude agent without --force.
        if *refresh && init_footprint == cli::InitFootprint::Full {
            let packs = scaffold_refresh::refresh_agent_packs(&statusline_project_root(), None);
            scaffold_refresh::print_refresh_summary(&packs);
            // Seed into THIS project's store: the distributed store first, then
            // a local legacy store. Never the registry's ambient default, which
            // could be an unrelated project's database. trace:BUG-1603 | ai:claude
            // A distributed project whose store isn't attached here must not
            // fall through to a stale `requirements.db` in the cwd either: try
            // to attach the store, and otherwise skip seeding. trace:TASK-1486 | ai:claude
            let cwd = std::env::current_dir()?;
            let target = refresh_seed_target(
                detect_distributed_store(),
                unattached_distributed_root(&cwd).is_some(),
                || determine_requirements_path(None),
            );
            let target = match target {
                RefreshSeedTarget::DistributedUnattached => {
                    match unattached_distributed_root(&cwd) {
                        // BUG-433 shape: the store is already physically
                        // attached at `<root>/.aida-store` (no fetch/attach
                        // needed) — use it directly, matching the main
                        // resolver's `store_path_opt` computation.
                        // trace:TASK-1487 | ai:claude
                        Some(ref root) if attached_store_present(root) => {
                            RefreshSeedTarget::Store(root.join(".aida-store"))
                        }
                        Some(root) if branch_exists_anywhere(&root, "aida-store") => {
                            match try_attach_store_worktree(&root) {
                                Ok(store_path) => {
                                    eprintln!(
                                        "  {} attached the AIDA store worktree from \
                                         `aida-store`",
                                        "Note:".dimmed()
                                    );
                                    RefreshSeedTarget::Store(store_path)
                                }
                                // Auto-attach failed (offline, diverged/locked
                                // branch, git too old, …) — print the cause,
                                // as the main resolver does for the same
                                // failure, instead of silently dropping it.
                                // trace:TASK-1487 | ai:claude
                                Err(e) => {
                                    eprintln!(
                                        "  {} couldn't auto-attach the store worktree: {}",
                                        "Note:".dimmed(),
                                        e
                                    );
                                    RefreshSeedTarget::DistributedUnattached
                                }
                            }
                        }
                        _ => RefreshSeedTarget::DistributedUnattached,
                    }
                }
                other => other,
            };
            match target {
                RefreshSeedTarget::Store(store_path) => {
                    let storage = Storage::new(store_path);
                    let seeded = protocol_cmd::seed_missing_protocols(&storage)?;
                    if seeded > 0 {
                        println!("  {} seeded {seeded} missing type protocol(s)", "+".green());
                    }
                }
                RefreshSeedTarget::DistributedUnattached => eprintln!(
                    "  {} type protocols not seeded: this is a distributed AIDA project, but \
                     its store isn't attached in this working copy (no `.aida-store/` \
                     worktree). A local legacy requirements store here was left untouched.",
                    "Note:".dimmed()
                ),
                RefreshSeedTarget::NoStore => eprintln!(
                    "  {} type protocols not seeded: no requirements store found here",
                    "Note:".dimmed()
                ),
            }
        }
        // TASK-859: surface a small curated set of high-value config knobs that
        // are otherwise silent defaults (telemetry opt-out today) and offer to
        // open `aida config menu` for the full surface. TTY-gated + idempotent,
        // mirroring the agent-posture prompt above — non-interactive init writes
        // nothing and keeps every default. Non-fatal: a hiccup here must not
        // abort an otherwise-successful init. trace:TASK-859 | ai:claude
        if init_footprint == cli::InitFootprint::Full {
            if let Err(e) = maybe_prompt_init_config(&statusline_project_root()) {
                eprintln!("  {} config prompts skipped: {}", "Note:".dimmed(), e);
            }
        }
        // BUG-1938: the steps below may rewrite the committed config
        // (provider refresh, capabilities); snapshot its state first.
        let cfg_root = statusline_project_root();
        let cfg_was_clean = config_commit::config_is_clean(&cfg_root);
        // STORY-780: remote + ordered pushes, only for a bootstrap init and
        // only now that the store branch exists. trace:STORY-780 | ai:claude
        if let Some(plan) = &bootstrap {
            init_bootstrap::finish_remote(plan)?;
        }
        // STORY-1467: classify forge / CI / local-only capabilities now that
        // any bootstrap remote exists, record them under
        // `[project.capabilities]`, and print them so forge-dependent features
        // degrade explicitly. Best-effort. trace:STORY-1467 | ai:claude
        project_capabilities::record_and_report(&statusline_project_root());
        // trace:BUG-1938 | ai:claude
        config_commit::finish_init_refresh(&cfg_root, cfg_was_clean);
        // Register the initialized project in the machine-global project
        // registry after bootstrap remote setup, so `repo` reflects the final
        // origin URL when one was configured. Best-effort because this writes
        // outside the project root. trace:STORY-827 | ai:codex
        if init_footprint == cli::InitFootprint::Full {
            if let Err(e) = register_project_in_global_registry(&statusline_project_root()) {
                eprintln!("  {} project registry skipped: {}", "Note:".dimmed(), e);
            }
        }
        // STORY-828: machine-global personal hooks run only after the whole
        // init lifecycle has succeeded, including bootstrap remote setup and
        // the best-effort global project registry write.
        if !*no_post_hooks && init_footprint == cli::InitFootprint::Full {
            let project_root = statusline_project_root();
            let remote_url = init_cmd::git_origin_url(&project_root).unwrap_or_default();
            let preferred_forge = bootstrap.as_ref().and_then(|plan| {
                if plan.github {
                    Some("github")
                } else if plan.forge.is_some() && plan.remote.is_some() {
                    Some("gitlab")
                } else {
                    None
                }
            });
            let forge = init_cmd::detect_init_forge(preferred_forge, &remote_url);
            let context = init_cmd::PostInitHookContext {
                project_root,
                project_name: init_cmd::init_project_name(name.as_deref()),
                lang: lang.clone().unwrap_or_default(),
                remote_url,
                forge,
            };
            if let Err(e) = init_cmd::run_post_init_hooks(&context) {
                eprintln!("  {} post-init hooks skipped: {}", "Note:".dimmed(), e);
            }
        }
        return Ok(());
    }

    // Handle upgrade before storage resolution — it needs no DB.
    // trace:EPIC-1-001 | ai:claude
    if let Command::Upgrade {
        check,
        version,
        yes,
        target,
        diff,
        cmd,
    } = &cli.command
    {
        let (check, diff) = normalize_upgrade_mode(*check, *diff, cmd.as_ref());
        return upgrade_cmd::handle_upgrade_command(
            check,
            version.as_deref(),
            *yes,
            target.as_deref(),
            diff,
        );
    }

    // BUG-1977: the hook-only refresh repairs what blocks store writes, so
    // it must not need a resolvable store (the refusal names --project-root).
    // trace:BUG-1977 | ai:claude
    if let Command::Scaffold(ScaffoldCommand::Upgrade {
        project_root,
        dry_run,
        only: Some(cli::ScaffoldUpgradeOnly::GitHooks),
        ..
    }) = &cli.command
    {
        let root = project_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        if !root.exists() {
            anyhow::bail!("Project root does not exist: {}", root.display());
        }
        return scaffold_cmd::run_scaffold_upgrade_git_hooks(&root, *dry_run);
    }

    // STORY-410: `aida memories check` compares the local Claude Code memory
    // pack to the binary's embedded master — no store access needed, so handle
    // it before storage resolution (like init/upgrade). trace:STORY-410
    if let Command::Memories(MemoriesCommand::Check { verbose, json }) = &cli.command {
        return memories_cmd::handle_memories_check(*verbose, *json);
    }

    // `aida schema` is a pure reflection read — it touches no store, so
    // dispatch it before storage resolution (like `memories`/`dev`).
    // trace:STORY-538 | ai:claude
    if let Command::Schema {
        object,
        all,
        explain,
        json,
    } = &cli.command
    {
        // `--all` is the whole-catalog dump; a positional `<object>` is a
        // single-object request. The two are contradictory, so `--all` carries
        // `conflicts_with = "object"` in the clap definition (cli.rs) — clap
        // rejects the combination with a clear error before this dispatch runs,
        // so no runtime guard is needed here. trace:TASK-775 | ai:claude
        match object.as_deref().map(str::to_lowercase).as_deref() {
            // `--all` (human) and no-arg `--json` are both a full dump: the
            // catalog plus every object's reflection-derived field detail in
            // catalog order. For JSON we always include the per-object fields
            // (a no-arg `--json` was field-less before), so the machine surface
            // is a true one-fetch full dump. trace:TASK-799 trace:TASK-775 | ai:claude
            None if *all || *json => schema::print_all(*json, *explain),
            // `aida schema --explain` (no object) adds the lifecycle blocks to
            // the catalog view. trace:STORY-630 | ai:claude
            None if *explain => schema::print_catalog(*json, true),
            None => schema::print_catalog(*json, false),
            Some("requirement") | Some("requirements") | Some("req") => {
                schema::print_requirement(*json, *explain)
            }
            Some(other) if schema::is_catalog_object(other) => {
                // A catalog object other than Requirement — render its
                // reflection-derived field table. STORY-538 shipped Requirement
                // detail only; TASK-714 extended the reflection registry to
                // every remaining kind. trace:STORY-538 trace:TASK-714
                schema::print_object(other, *json, *explain)
            }
            Some(other) => {
                anyhow::bail!(
                    "unknown schema object `{other}` — run `aida schema` for the catalog, \
                     or `aida schema requirement` for the Requirement detail"
                );
            }
        }
        return Ok(());
    }

    // Handle skill commands before storage resolution — needs no DB.
    if let Command::Skill(skill_cmd) = &cli.command {
        match skill_cmd {
            SkillCommand::Render { name } => {
                let project_root = find_project_root()
                    .or_else(|_| std::env::current_dir())
                    .map_err(|e| {
                        anyhow::anyhow!(
                            "Error: could not find project root or current directory: {}",
                            e
                        )
                    })?;
                let skills_dir = project_root.join(".claude").join("skills");
                let stock_path = skills_dir.join(format!("{}.md", name));

                if !stock_path.exists() {
                    anyhow::bail!("Error: skill '{}' not found under .claude/skills/", name);
                }

                let stock_content = std::fs::read_to_string(&stock_path)
                    .map_err(|e| anyhow::anyhow!("Failed to read skill file: {}", e))?;

                let local_path = skills_dir.join(format!("{}.local.md", name));
                if local_path.exists() {
                    let local_content = std::fs::read_to_string(&local_path)
                        .map_err(|e| anyhow::anyhow!("Failed to read local skill file: {}", e))?;

                    let sep = if stock_content.ends_with('\n') {
                        ""
                    } else {
                        "\n"
                    };
                    print!("{}{}{}", stock_content, sep, local_content);
                } else {
                    print!("{}", stock_content);
                }
                return Ok(());
            }
            // `aida skill lint` reads `.claude/skills/*.md` and the plan files
            // they reference — purely filesystem, no store handle. Dispatch it
            // here alongside the other skill tooling. trace:TASK-927 | ai:claude
            SkillCommand::Lint { skill, json, quiet } => {
                return lint_skills(skill.as_deref(), *json, *quiet);
            }
        }
    }

    // Handle dev commands before storage resolution — most need no DB.
    // (Dev::Serve does interact with storage but spawns aida-server which
    // handles that itself; the wrapper just supervises the children.)
    // trace:EPIC-1-001 | ai:claude
    if let Command::Dev(dev_cmd) = &cli.command {
        return crate::dev_cmd::handle_dev_command(dev_cmd);
    }
    // `aida driver` reads/writes `.aida/drivers/` only. trace:STORY-1653 | ai:claude
    if let Command::Driver(cmd) = &cli.command {
        return crate::driver_cmd::handle_driver_command(cmd);
    }

    // STORY-472: `aida release` — a memorable top-level verb wrapping the dev
    // release flow (store pull → scripts/release.sh → tarball wait → sibling
    // upgrade). Operates on the repo, not the requirements store, so dispatch
    // early like `dev`. trace:STORY-472 | ai:claude
    if let Command::Release {
        patch,
        minor,
        major,
        check,
        after_pr,
        skip_xplat_check,
        cmd,
    } = &cli.command
    {
        let (patch, minor, major, check) =
            normalize_release_mode(*patch, *minor, *major, *check, cmd.as_ref());
        return handle_release(patch, minor, major, check, *after_pr, *skip_xplat_check);
    }

    // STORY-527: `aida burndown` reads the requirement graph (like `graph` /
    // `plan`) — no shared storage handle needed. Dispatch early.
    // trace:STORY-527 | ai:claude
    if let Command::Burndown(cmd) = &cli.command {
        return handle_burndown_command(cmd);
    }

    // STORY-658: `aida health` self-loads the store + probes the runtime
    // substrate (leases, drain lock, queue) — no shared storage handle needed,
    // so dispatch early like burndown. trace:STORY-658 | ai:claude
    if let Command::Health { json, brief } = &cli.command {
        return health_vitals_cmd::handle_health_vitals_command(*json, *brief);
    }

    // STORY-560: `aida groom` (canonical; `assess`/`intake` are deprecated
    // aliases — STORY-708) self-loads the store to compute its candidate
    // fence and then launches a headless `claude -p` — no shared storage handle
    // needed. Dispatch early, like burndown. trace:STORY-560 trace:STORY-708 | ai:claude
    if let Command::Groom {
        apply,
        max_approvals,
        only_tag,
        exclude_tag,
        risk,
        then_drain,
        dry_run,
        permission_mode,
    } = &cli.command
    {
        return handle_intake_command(
            *apply,
            *max_approvals,
            only_tag.as_deref(),
            exclude_tag.as_deref(),
            risk,
            *then_drain,
            *dry_run,
            permission_mode.as_deref(),
        );
    }

    // TASK-1147: `aida autopilot` (inspect / audit / challenge) is the read-only
    // auditability + reversal surface over the policy envelope. Like `aida
    // groom` it self-loads the store to compute its candidate fence and writes
    // nothing to any spec — dispatch early, no shared storage handle needed.
    // trace:TASK-1147 | ai:claude
    if let Command::Autopilot(cmd) = &cli.command {
        return handle_autopilot_command(cmd);
    }

    // STORY-547: `aida why <ID>` reads the requirement graph like burndown —
    // dispatch early, no shared storage handle needed. trace:STORY-547
    if let Command::Why { id, plain, json } = &cli.command {
        return handle_why(id, *plain, *json);
    }

    // trace:EPIC-72 trace:TASK-1436 | ai:antigravity
    if let Command::Explain {
        spec,
        audience,
        refresh,
        force,
        json,
    } = &cli.command
    {
        return exposition::handle_explain_command(spec, audience, *refresh, *force, *json);
    }

    // trace:EPIC-72 trace:TASK-1439 | ai:antigravity
    if let Command::Wiki(command) = &cli.command {
        return wiki::handle_wiki_command(command);
    }

    // STORY-694: `aida status <spec>` is the per-spec liveness view — it reads
    // the local session leases + probes pid liveness and self-loads the store
    // read-only for the spec's lifecycle status. Like `aida why` it needs no
    // shared storage handle, so dispatch early (before storage init). The
    // bare `aida status` (no spec) still flows through the normal dispatch.
    // trace:STORY-694 | ai:claude
    if let Command::Status {
        spec: Some(spec),
        idle_minutes,
        json,
        ..
    } = &cli.command
    {
        // trace:BUG-1289 | ai:claude
        return handle_status_spec(spec, *idle_minutes, *json || output_format_is_json());
    }

    // TASK-188: make dev-binary staleness visible at the point of use. One
    // stderr line, local git only (no network), silent outside the AIDA
    // workspace or when the running build matches the default branch.
    // trace:TASK-188 | ai:claude
    if let Command::Status { spec: None, .. } = &cli.command {
        crate::freshness_gate::warn_if_running_binary_stale();
    }

    // STORY-769: the `aida awaiting --notice` per-turn hook ALWAYS leads with a
    // "Current date/time + Timing" line — a deliberate contract change from the
    // old silent-when-empty behaviour. Emit it EARLY, before store init, so the
    // hook still gets its time context even if the store/backend can't be
    // resolved (offline, cache-locked, unattached), and so the last-human-input
    // presence oracle is stamped every turn. The compact "Awaiting you" line
    // still prints later from `handle_awaiting_command`; this only prepends the
    // always-on time line. trace:STORY-769 | ai:claude
    if let Command::Awaiting {
        notice: true,
        json: false,
        ..
    } = &cli.command
    {
        // Arm before any notice-specific filesystem work. Session-start
        // registration and the turn-clock stamp are intentionally fail-open,
        // but can still stall on a contended filesystem.
        arm_notice_deadline();
        emit_notice_time_line();
    }

    // STORY-696: `aida ps` is the GLOBAL running-work table — the project-wide
    // companion to `aida status <spec>`. Like that command it reads the local
    // session leases + probes pid liveness and self-loads the store read-only
    // (to find orphaned In-Progress specs), so it needs no shared storage
    // handle — dispatch early. trace:STORY-696 | ai:claude
    if let Command::Ps { json, all } = &cli.command {
        // trace:BUG-1289 | ai:claude
        return handle_ps(*json || output_format_is_json(), *all);
    }

    // `aida merge-lock` shows active branch merge-leases (STORY-1171). Read-only
    // (reads `.aida/merge-locks/`), needs no store handle — dispatch early like
    // `aida ps`. trace:STORY-1171 | ai:claude
    if let Command::MergeLock { json } = &cli.command {
        return handle_merge_lock_status(*json);
    }

    // `aida merge-hold list|clear` inspects/clears supervised merge-hold markers.
    // Read/writes only `.aida/merge-holds/` (+ a best-effort `gh` label/merged
    // probe), needs no store handle — dispatch early like `aida merge-lock`.
    // trace:TASK-161 | ai:claude
    if let Command::MergeHold { action } = &cli.command {
        return handle_merge_hold(action);
    }

    // `aida tail` is `aida ps`'s streaming companion: it resolves a session /
    // spec / drain id to the ONE log file that work streams into and tails it.
    // Reads only `.aida/burndown/`, `.aida/headless-logs/` and the lease dir —
    // no store, no network — so dispatch early like `aida ps`.
    // trace:TASK-1167 | ai:claude
    if let Command::Tail {
        target,
        list,
        json,
        lines,
        since,
        no_follow,
        with_tools,
        no_timestamp,
        annotate,
    } = &cli.command
    {
        return handle_tail_cli(
            target.clone(),
            *list,
            *json,
            *lines,
            since.as_deref(),
            *no_follow,
            *with_tools,
            *no_timestamp,
            *annotate,
        );
    }

    // `aida statusbar` — the ambient, read-only terminal-title meter. Like
    // `aida ps` / `aida integrate` it self-resolves everything it needs
    // (queue YAML, session leases, cache-backed awaiting channels) and
    // degrades gracefully offline, so it needs no shared storage handle —
    // dispatch early. trace:STORY-715 | ai:claude
    if let Command::Statusbar {
        interval,
        once,
        plain,
        restore_title,
    } = &cli.command
    {
        return statusbar_cmd::handle_statusbar_command(*interval, *once, *plain, *restore_title);
    }

    if let Command::Contract { json: _ } = &cli.command {
        return monitor_contract::print_json();
    }

    // `aida watch` is a read-only consumer of the slice-1 `.aida/events.jsonl`
    // stream (STORY-712): it tails the file, classifies each event in cheap
    // code, and wakes only on actionable verbs. It touches no store/backend, so
    // dispatch early like `aida ps`. trace:TASK-990 | ai:claude
    if let Command::Watch {
        emit_wakes: _,
        all,
        json,
        verbose,
        once,
        backlog,
    } = &cli.command
    {
        let project_root =
            find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        return watch::handle_watch(
            &project_root,
            &watch::WatchOpts {
                all: *all,
                // trace:BUG-1289 | ai:claude
                json: *json || output_format_is_json(),
                // trace:TASK-994 | ai:claude
                verbose: *verbose,
                once: *once,
                backlog: *backlog,
            },
        )
        .and_then(|()| notify::passive_check(&project_root));
    }

    // `aida notify` has no need for the requirement store: it consumes the
    // local event stream plus `.aida/config.toml`, so it is safe for cron and
    // for hooks that run while the cache/store may be locked.
    // trace:STORY-1029 | ai:codex
    if let Command::Notify(cmd) = &cli.command {
        let project_root =
            find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        return notify::handle_notify_command(&project_root, cmd);
    }

    // `aida integrate` is the integrator front door. Bare remains the read-only
    // throughput/merge-queue view (TASK-1034); `--run` / `--watch` / `--dry-run`
    // enter the same serialized engine as `aida queue integrate`, with rebase
    // enabled by default so Done-with-PR specs land through one current-main
    // queue instead of racing each other stale. trace:STORY-1024 | ai:codex
    if let Command::Integrate {
        json,
        run,
        dry_run,
        watch,
        interval,
        max,
        wait_ci: _,
        no_wait_ci,
        no_rebase,
        strategy,
        focus,
        idle_minutes,
        force,
        user,
    } = &cli.command
    {
        return handle_integrate(IntegrateCommandOpts {
            // trace:BUG-1289 | ai:claude
            json: *json || output_format_is_json(),
            run: *run,
            dry_run: *dry_run,
            watch: *watch,
            interval: *interval,
            max: *max,
            wait_ci: !*no_wait_ci,
            rebase: !*no_rebase,
            strategy: *strategy,
            focus: focus.clone(),
            idle_minutes: *idle_minutes,
            force: *force,
            user: user.clone(),
        });
    }

    // TASK-957: `aida claim` / `aida unclaim` self-load the store read-only to
    // resolve the spec id forms, then write/remove a lightweight advisory lease
    // under `.aida/sessions/`. Like `aida ps` / `aida status <spec>` they touch
    // only the local lease dir + a cache-backed lookup, so they need no shared
    // storage handle — dispatch early. trace:TASK-957 | ai:claude
    if let Command::Claim { spec, worktree } = &cli.command {
        return handle_claim(spec, worktree.as_deref());
    }
    if let Command::Unclaim { spec } = &cli.command {
        return handle_unclaim(spec);
    }

    // STORY-631: `aida intent <ID>` self-loads the store (read for cache-print,
    // read+write for generate), and may shell out to a headless `claude -p`
    // running `/aida-intent`. Like `aida why` / `aida intake`, it needs no
    // shared storage handle — dispatch early. trace:STORY-631 | ai:claude
    if let Command::Intent {
        id,
        audience,
        refresh,
        json,
    } = &cli.command
    {
        return handle_intent(id, audience, *refresh, *json);
    }

    // STORY-656: `aida spec dryrun <ID>` self-loads the store (read-only) for
    // the deterministic pre-check, and with `--ai` may shell out to a headless
    // `claude -p`. Like `aida intent`, it needs no shared storage handle —
    // dispatch early. trace:STORY-656 | ai:claude
    if let Command::Spec(SpecCommand::Dryrun { id, ai, json }) = &cli.command {
        return handle_spec_dryrun(id, *ai, *json);
    }

    // STORY-657: `aida spec interview <ID>` reuses dryrun's scorer to derive
    // clarifying questions, prompts the human (TTY) or emits them as JSON
    // (headless), and with `--apply` folds the answers back into the spec. Like
    // dryrun it self-loads the store; dispatch early. trace:STORY-657 | ai:claude
    if let Command::Spec(SpecCommand::Interview {
        id,
        apply,
        ai,
        answers,
        json,
    }) = &cli.command
    {
        return handle_spec_interview(id, *apply, *ai, answers.as_deref(), *json);
    }

    // STORY-716: `aida worktree` (EPIC-55 workspace layer) operates on git +
    // the filesystem; `enter` emits `cd` shell on stdout for the `aida()`
    // wrapper to auto-eval. Like `role enter` it needs no shared storage
    // handle and its stdout must stay clean of store-init noise, so it
    // dispatches early. trace:STORY-716 | ai:claude
    if let Command::Worktree(wt_cmd) = &cli.command {
        return handle_worktree_command(wt_cmd);
    }

    // STORY-563: `aida human <subcommand>` self-loads the store like `aida why`
    // / `burndown explain`, or delegates to the top-level presence handlers.
    // Dispatch early, no shared storage handle needed.
    // trace:STORY-563 | ai:claude
    // Presence + unblock subcommands need no storage handle, so dispatch them
    // here before store init. The STORY-611 action aliases (`answer`/`review`/
    // `decide`) DO need the backend — they fall through to the main dispatch.
    // trace:STORY-611 | ai:claude
    if let Command::Human {
        command: Some(human_cmd),
        ..
    } = &cli.command
    {
        if human_subcommand_needs_no_storage(human_cmd) {
            return handle_human_subcommand(human_cmd);
        }
    }

    // Doctor commands run before storage init — they may need to operate
    // on broken or partially-migrated stores. trace:EPIC-19 | ai:claude
    if let Command::Doctor {
        heal,
        yes,
        category,
        json,
        force,
        all,
        since,
        fix_sandbox,
        contradictions,
        contradictions_limit,
        contradictions_offset,
        cmd,
    } = &cli.command
    {
        // STORY-665: `aida doctor --fix-sandbox` is a guided printer for bringing
        // bwrap OS-confinement up on a new machine. Short-circuit before the
        // multi-agent drift scan — it's a standalone setup helper, not a check.
        // trace:STORY-665 | ai:claude
        if *fix_sandbox {
            return doctor_cmd::doctor_fix_sandbox();
        }
        // STORY-1426: `aida doctor --contradictions` runs the store-wide semantic
        // contradiction sweep combining mechanical joins with Jev choice queries.
        // trace:STORY-1426 | ai:antigravity
        if *contradictions {
            return doctor_cmd::doctor_contradictions(
                *json,
                *contradictions_limit,
                *contradictions_offset,
                *all,
            );
        }
        return doctor_cmd::handle_doctor_command(
            *heal,
            *yes,
            category.as_deref(),
            *json,
            *force,
            *all,
            since.as_deref(),
            cmd.as_ref(),
        );
    }

    // Store commands inspect git state, no AIDA storage needed.
    // trace:EPIC-21 | ai:claude
    if let Command::Store(store_cmd) = &cli.command {
        return store_cmd::handle_store_command(store_cmd);
    }

    // `aida remote create`/`attach` bootstrap a git origin — they operate on a
    // project that, by definition, may not have a working store/remote yet, so
    // dispatch before storage init. trace:STORY-537 | ai:claude
    if let Command::Remote(remote_cmd) = &cli.command {
        let project_root =
            find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        return match remote_cmd {
            crate::cli::RemoteCommand::Create {
                attach,
                github,
                gitlab,
                public,
            } => remote_create::handle_remote_create(
                &project_root,
                attach.as_deref(),
                *github,
                gitlab.as_deref(),
                !*public,
            ),
            crate::cli::RemoteCommand::Attach { url } => {
                remote_create::handle_remote_attach(&project_root, url)
            }
            crate::cli::RemoteCommand::Status { json, no_fetch } => {
                remote_create::handle_remote_status(&project_root, *json, *no_fetch)
            }
            // trace:TASK-1097 | ai:claude
            crate::cli::RemoteCommand::Mirror { name, url } => {
                remote_create::handle_remote_mirror(&project_root, name, url.as_deref())
            }
            crate::cli::RemoteCommand::MirrorPush {
                pushed_remote,
                dry_run,
            } => remote_create::handle_remote_mirror_push(&project_root, pushed_remote, *dry_run),
            // trace:BUG-1676 | ai:claude
            crate::cli::RemoteCommand::MirrorSync { json } => {
                remote_create::handle_remote_mirror_sync(&project_root, *json)
            }
            crate::cli::RemoteCommand::Reconcile { execute, json, yes } => {
                remote_create::handle_remote_reconcile(&project_root, *execute, *json, *yes)
            }
        };
    }

    // TASK-1330: the identity HYGIENE subcommands read only git state and
    // `.aida/config.toml` — no store needed, and the pre-push hook plumbing
    // must work even when the store is unavailable — so dispatch them before
    // storage init, like `remote`. The person-alias registry subcommands
    // (link/list/show) read the store and fall through to the normal path.
    // trace:TASK-1330 | ai:claude
    if let Command::Identity { cmd } = &cli.command {
        let project_root =
            find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        match cmd {
            crate::cli::IdentityCommand::Check => {
                return identity_gate::handle_identity_check(&project_root);
            }
            // BUG-1926: `project_root` here is the checkout being pushed
            // from (possibly the `.aida-store` worktree); the handler resolves
            // the governing allowlist from the owning project itself.
            // trace:BUG-1926 | ai:claude
            crate::cli::IdentityCommand::CheckPush { pushed_remote } => {
                return identity_gate::handle_identity_check_push(&project_root, pushed_remote);
            }
            crate::cli::IdentityCommand::InstallHook => {
                return identity_gate::handle_identity_install_hook(&project_root);
            }
            crate::cli::IdentityCommand::Link { .. }
            | crate::cli::IdentityCommand::List { .. }
            | crate::cli::IdentityCommand::Show { .. } => {}
        }
    }

    // Sandbox commands MANAGE a throwaway store directory; they don't read the
    // project's store, so dispatch before store detection (and so they work
    // even from a non-AIDA cwd). trace:SPIKE-48 | ai:claude
    if let Command::Sandbox(sandbox_cmd) = &cli.command {
        return sandbox_cmd::handle_sandbox_command(sandbox_cmd);
    }

    // Help-all / commands are pure text; no storage needed.
    if let Command::HelpAll = &cli.command {
        print_help_all();
        return Ok(());
    }
    if let Command::Commands {
        flags,
        json,
        hidden,
    } = &cli.command
    {
        help_catalog::print_command_catalog(*flags, *hidden, *json);
        return Ok(());
    }

    // `aida alias` / `aida alias list` is a built-in-shortcut registry (now also
    // surfacing user-defined aliases) — it reads no requirement store and needs
    // no storage handle, so dispatch it early like help-all. Bare `aida alias`
    // defaults to `list`. `add` / `remove` manage user aliases under the chosen
    // scope. trace:STORY-667 | ai:claude
    // trace:TASK-877 | ai:claude — user-alias CRUD dispatch.
    if let Command::Alias { json, command } = &cli.command {
        match command {
            Some(crate::cli::AliasCommand::Add {
                name,
                command: cmd_tokens,
                global,
                project,
            }) => {
                let scope = user_alias::resolve_scope(*project, *global)?;
                return user_alias::add(scope, name, cmd_tokens);
            }
            Some(crate::cli::AliasCommand::Remove {
                name,
                global,
                project,
            }) => {
                let scope = user_alias::resolve_scope(*project, *global)?;
                return user_alias::remove(scope, name);
            }
            Some(crate::cli::AliasCommand::List { json: sub_json }) => {
                return alias::run(*json || *sub_json);
            }
            None => return alias::run(*json),
        }
    }

    // Plan tooling is self-contained: `verify` reads a markdown file +
    // source files; `helpers` loads the store itself via
    // `load_store_for_lookup`. Neither needs the shared storage handle, so
    // dispatch the whole group early. trace:TASK-93 TASK-94 | ai:claude
    if let Command::Plan(plan_cmd) = &cli.command {
        return plan_cmd::handle_plan_command(plan_cmd);
    }

    // `aida deps sweep` self-loads the store via `load_store_for_lookup`
    // and scans source for trace comments — like `plan helpers`, it needs
    // no shared storage handle. Read-only. trace:STORY-447 | ai:claude
    if let Command::Deps(deps_cmd) = &cli.command {
        return deps_cmd::handle_deps_command(deps_cmd);
    }

    // `aida lint` self-loads the store via `load_store_for_lookup` and scores
    // requirement text with the EARS heuristics — read-only, no shared storage
    // handle, no LLM. trace:TASK-0417 | ai:claude
    if let Command::Lint { spec, scope, json } = &cli.command {
        return lint_cmd::handle_lint_command(spec.as_deref(), scope.as_deref(), *json);
    }

    // Explicit only: no sync/drain/hook path invokes host probes.
    // trace:STORY-1673 | ai:codex
    if let Command::Verify(cmd) = &cli.command {
        return verify_cmd::handle_verify_command(cmd, cli.file.as_deref());
    }

    // `aida gate` reads the embedded gate library (and, for `run`, self-loads
    // the store) — no shared storage handle. trace:STORY-1427 | ai:claude
    if let Command::Gate(gate_cmd) = &cli.command {
        return gate_cmd::handle_gate_command(gate_cmd);
    }

    // `aida lifecycle` (Phase 1, generate-only) is self-contained: it renders a
    // Mermaid diagram from the declared transition model in aida-core and
    // optionally pins it against `docs/lifecycle.md`. No storage handle, no LLM,
    // no behavior change to any other command. trace:TASK-737 | ai:claude
    if let Command::Lifecycle {
        diagram,
        check,
        write,
        doc,
        empirical,
        diff,
    } = &cli.command
    {
        return lifecycle_cmd::handle_lifecycle_command(
            *diagram,
            *check,
            *write,
            doc.as_deref(),
            *empirical,
            *diff,
        );
    }

    // STORY-498: `aida trace gate` is self-contained — it reads git for the
    // commit range and self-loads the store via `load_store_for_lookup` to
    // resolve each `(SPEC-ID)` trailer. Dispatch it early (the other `trace`
    // subcommands still need the shared storage handle and fall through to the
    // backend path). Exits non-zero on a dead/dangling reference.
    // trace:STORY-498 | ai:claude
    if let Command::Trace(TraceCommand::Gate { range, json }) = &cli.command {
        return handle_trace_gate(range.as_deref(), *json);
    }

    // STORY-499: `aida trace coverage` is self-contained the same way `gate` is
    // — it reads git for the diff + range and self-loads the store to resolve
    // `// trace:` anchors / `(SPEC-ID)` trailers. Dispatch it early so it never
    // needs the shared storage handle. trace:STORY-499 | ai:claude
    if let Command::Trace(TraceCommand::Coverage { range, json, block }) = &cli.command {
        return handle_trace_coverage(range.as_deref(), *json, *block);
    }

    // `aida changelog` is self-contained: git + read-only store, no shared
    // storage handle. Dispatch alongside Plan/Ultraplan. trace:TASK-299 | ai:claude
    if let Command::Changelog(cl_cmd) = &cli.command {
        return changelog_cmd::handle_changelog_command(cl_cmd);
    }

    // `aida manual <cmd>` only reads the markdown chapters under docs/cli/ —
    // no store, no cache, no network. Dispatch it before storage init like
    // changelog. trace:STORY-600 | ai:claude
    if let Command::Manual { command } = &cli.command {
        let project_root =
            find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        return manual::run(command, &project_root);
    }

    // `aida derisk` is a thin advisor-session launcher; it uses the shared
    // agent-new machinery and does not need the shared storage handle.
    // trace:TASK-1235 | ai:codex
    if let Command::Derisk { spec } = &cli.command {
        return handle_derisk_command(spec);
    }

    // `aida ultraplan` also self-loads the store via `load_store_for_lookup`.
    // trace:TASK-113 | ai:claude
    if let Command::Ultraplan {
        spec,
        stdout,
        json,
        copy: _,
        no_comments,
    } = &cli.command
    {
        return ultraplan_cmd::handle_ultraplan_command(spec, *stdout, *json, *no_comments);
    }

    // `aida compete` self-loads the store (to assemble the brief) and
    // orchestrates worktrees + headless vendor runs + the objective gate.
    // Dispatch early alongside ultraplan — it doesn't need the shared storage
    // handle. trace:STORY-659 | ai:claude
    if let Command::Compete {
        spec,
        vendors,
        gate,
        dry_run,
        judge,
        judge_vendor,
    } = &cli.command
    {
        return compete_cmd::handle_compete_command(
            spec,
            vendors,
            gate.as_deref(),
            *dry_run,
            *judge,
            judge_vendor,
        );
    }

    // `aida commit` is self-contained — git + the staged diff (for REQ-ID
    // inference), no requirements store. Dispatch before storage init like
    // goal / ultraplan. trace:STORY-663 | ai:claude
    if let Command::Commit {
        commit_type,
        scope,
        message,
        spec,
        ai,
        no_ai,
        all,
        dry_run,
    } = &cli.command
    {
        return commit::handle_commit_command(&commit::CommitArgs {
            commit_type: commit_type.clone(),
            scope: scope.clone(),
            message: message.clone(),
            spec: spec.clone(),
            ai: ai.clone(),
            no_ai: *no_ai,
            all: *all,
            dry_run: *dry_run,
        });
    }

    // `aida internal advisor-code-gate` is the vendor-agnostic substrate
    // enforcement of the advisor-no-code-write invariant (STORY-684). The
    // scaffolded git pre-commit hook shells out to it so ANY vendor's
    // `git commit` (Codex, raw terminal, headless) hits the same gate the
    // Claude PreToolUse hook only ever gave Claude. Self-contained (git diff +
    // env + the solo marker), so it dispatches before storage init.
    // trace:STORY-684
    if let Command::Internal { command } = &cli.command {
        return internal_cmd::handle_internal_command(command);
    }

    // `aida goal` is a pure condition generator — no store needed.
    // trace:TASK-242 | ai:claude
    if let Command::Goal {
        batch,
        epic,
        spec,
        pr,
        queue_empty,
        copy,
        invoke,
        as_deep_link,
    } = &cli.command
    {
        return goal_cmd::handle_goal_command(
            batch.as_deref(),
            epic.as_deref(),
            spec.as_deref(),
            *pr,
            queue_empty.as_deref(),
            *copy,
            *invoke,
            *as_deep_link,
        );
    }

    // `aida tui` is the EPIC-26 TUI shell — a process supervisor that
    // holds no storage handle (it shells out to `aida` subcommands), so
    // it dispatches before storage init like Goal / Statusline.
    // trace:STORY-132 | ai:claude
    if let Command::Tui {
        scope,
        no_recover,
        launcher,
        intent_fd,
    } = &cli.command
    {
        return handle_tui_command(scope.clone(), *no_recover, *launcher, *intent_fd);
    }

    // `aida config menu` is a navigable TUI view over the resolved config
    // surface (STORY-661). Like `aida tui` it holds no storage handle — the
    // policy registry it renders resolves from `.aida/config.toml` + the
    // global files + env directly — so it dispatches before storage init.
    // trace:STORY-661 | ai:claude
    if let Command::Config(ConfigCommand::Menu) = &cli.command {
        return config_cmd::handle_config_menu_command();
    }

    // Roles + statusline dispatch before storage init — roles are TOML
    // files at .aida/roles/, statusline reads the cache directly.
    if let Command::Role(role_cmd) = &cli.command {
        return role_cmd::handle_role_command(role_cmd);
    }
    if let Command::Statusline {
        color,
        title,
        client,
        action,
    } = &cli.command
    {
        // trace:TASK-0414 — the opt-in `setup` subcommand bootstraps the
        // statusline; with no subcommand we render the one-liner (default).
        // trace:TASK-896 — `--title` emits the same one-liner wrapped in an
        // OSC terminal-title escape so the AIDA segment rides the terminal
        // title bar (the in-agent parity surface for clients without a
        // command-backed footer, e.g. Codex CLI).
        // trace:TASK-1479 — `--client` opts into merging a client's live
        // stdin JSON payload (model/context/activity/VCS) with the AIDA
        // segment; omitted, behavior is unchanged (no stdin read).
        match action {
            Some(cli::StatuslineAction::Title) => {
                return statusline_cmd::handle_statusline_command(color, true, client.as_deref());
            }
            Some(act) => return statusline_cmd::handle_statusline_setup_command(act),
            None => {
                if *title {
                    note_hidden_alias(
                        concat!("aida statusline ", "--title"),
                        "aida statusline title",
                    );
                }
                return statusline_cmd::handle_statusline_command(color, *title, client.as_deref());
            }
        }
    }
    // STORY-79: hidden background-fetch worker spawned by statusline.
    // Dispatch before storage init — this is a self-contained worker
    // that owns its own git/toml side effects. trace:STORY-79 | ai:claude
    if let Command::BgFetch { store_path } = &cli.command {
        return session_misc_cmd::handle_bg_fetch_command(store_path);
    }
    // TASK-756: operator-presence primitive. Dispatched before storage init —
    // these only touch the machine-global `~/.aida/presence.toml` file; no
    // requirement-store handle needed. trace:TASK-756 | ai:claude
    match &cli.command {
        Command::Away => return presence_cmd::handle_away_command(),
        Command::Home => return presence_cmd::handle_home_command(),
        // trace:TASK-1231 | ai:claude
        Command::Autoprogress {
            project,
            groom,
            max,
            dry_run,
        } => {
            return autoprogress::handle_autoprogress(autoprogress::AutoprogressOpts {
                project: project.clone(),
                groom: *groom,
                max: max.unwrap_or(5),
                dry_run: *dry_run,
            })
        }
        // TASK-851: `aida presence [away|home|status]` is the canonical surface;
        // bare `aida presence` shows status (back-compat). The top-level
        // `aida away` / `aida home` stay as hidden aliases above.
        // trace:TASK-851 | ai:claude
        Command::Presence { action } => {
            return match action {
                Some(cli::PresenceCommand::Away) => presence_cmd::handle_away_command(),
                Some(cli::PresenceCommand::Home) => presence_cmd::handle_home_command(),
                Some(cli::PresenceCommand::Status) | None => {
                    presence_cmd::handle_presence_command()
                }
            };
        }
        // STORY-624: solo-mode flag — machine-global ~/.aida/solo.toml, no
        // requirement store needed (mirrors the away/home early dispatch).
        Command::Solo {
            action,
            off,
            status,
            ttl,
            watch,
            dry_run,
            interval,
        } => {
            return handle_solo_command(
                *action,
                *off,
                *status,
                ttl.as_deref(),
                *watch,
                *dry_run,
                *interval,
            );
        }
        // TASK-784: pure read of the caller-identity resolvers (env vars +
        // current_user_id + detect_agent_type). No project store needed, so
        // it dispatches here alongside the other env-only commands.
        // trace:TASK-784
        Command::Whoami => return session_misc_cmd::handle_whoami_command(),
        _ => {}
    }
    // trace:FR-1-043 | ai:claude
    if let Command::Session(session_cmd) = &cli.command {
        return handle_session_command(session_cmd);
    }
    // trace:STORY-995 | ai:codex — terminal helper installation is user-local
    // and store-independent, so it belongs in the early command tier.
    if let Command::Terminal(terminal_cmd) = &cli.command {
        return handle_terminal_command(terminal_cmd);
    }
    // TASK-661 (ADR-3): disposition/triage lease. Dispatched before storage
    // init — it reads + writes only `.aida/triage-leases/` files and probes
    // PIDs; no requirement-store handle needed. trace:TASK-661 | ai:claude
    if let Command::Triage(triage_cmd) = &cli.command {
        return triage_cmd::handle_triage_command(triage_cmd);
    }
    // STORY-711 slice 1: `aida lock` reads/writes only `.aida/sessions/`
    // lease files (the same registry `aida session leases` uses) — no
    // requirement-store handle needed, same reasoning as `aida triage`
    // above. trace:STORY-711 | ai:claude
    if let Command::Lock(lock_cmd) = &cli.command {
        return lock_cmd::handle_lock_command(lock_cmd);
    }
    // EPIC-31 agent launchers supervise external CLIs and write process
    // registry state directly; no requirement store handle needed.
    // trace:STORY-432 | ai:codex
    if let Command::Agent(agent_cmd) = &cli.command {
        return handle_agent_command(agent_cmd);
    }

    // STORY-90: PR side-effects dispatch before storage init — the
    // command's side-effects all live in git / gh / self-invoked `aida
    // add` calls; it doesn't need the global Storage handle.
    // trace:STORY-90 | ai:claude
    if let Command::Pr(pr_cmd) = &cli.command {
        return handle_pr_command(pr_cmd);
    }

    // trace:TASK-1583 | ai:antigravity
    if let Command::List {
        shortcut: Some(s),
        json,
        ..
    } = &cli.command
    {
        if s == "pr" || s == "mr" {
            return pr_list_handler(*json, false);
        }
        // trace:TASK-1590 | ai:antigravity
        if s == "review" {
            return handle_review_list(*json);
        }
    }

    // STORY-720: `aida ship` dispatches before storage init for the same
    // reason as `aida pr` — every side-effect is a git / gh / self-invoked
    // `aida` call (the finish tail reuses `pr_ship_handler`). The spec is
    // resolved from the branch / lease, not the requirement store.
    // trace:STORY-720 | ai:claude
    if let Command::Ship {
        spec,
        message,
        no_merge,
        no_pr,
        keep_worktree,
        dry_run,
        no_trailer_check,
    } = &cli.command
    {
        return run_human_finish_ceremony(HumanFinishOptions {
            spec: spec.clone(),
            message: message.clone(),
            no_merge: *no_merge,
            no_pr: *no_pr,
            keep_worktree: *keep_worktree,
            dry_run: *dry_run,
            no_trailer_check: *no_trailer_check,
        });
    }

    // BUG-233 / TASK-336: orchestrator-context introspection. Dispatched
    // before storage init — it reads only env vars + the `.aida/drain-
    // state.json` file's `run_uuid` + PID, no requirement store. (Before
    // TASK-336 it read a sidecar `.aida/orchestrator-runs/<uuid>` marker
    // file; that has been folded into the drain-state file.)
    // trace:BUG-233 trace:TASK-336 | ai:claude
    if let Command::Orchestrator(orch_cmd) = &cli.command {
        return handle_orchestrator_command(orch_cmd);
    }

    // BUG-237: zen-context introspection. Like `orchestrator status`, reads
    // only env vars + the drain-state file + session leases — no requirement
    // store. The `aida zen <spec>` autonomous-drive form (STORY-721) needs the
    // store to validate the spec's status, so only the introspection
    // subcommands short-circuit here; the spec-drive path falls through to the
    // post-storage dispatch. trace:BUG-237 trace:TASK-336 trace:STORY-721 | ai:claude
    if let Command::Zen {
        command: Some(zen_cmd),
        ..
    } = &cli.command
    {
        return handle_zen_command(zen_cmd);
    }

    // STORY-722: `aida zen <spec> --compete` — the 2-agent bake-off. Like
    // `aida compete` it self-loads the store (to resolve the spec + assemble
    // the brief) and orchestrates worktrees + headless vendor runs, so it
    // dispatches early and works identically on both storage backends.
    // trace:STORY-722 | ai:claude
    if let Command::Zen {
        spec,
        compete: true,
        dry_run,
        command: None,
        ..
    } = &cli.command
    {
        let Some(spec) = spec.as_deref() else {
            anyhow::bail!(
                "`aida zen --compete` needs a SPEC id (e.g. `aida zen TASK-123 --compete`)"
            );
        };
        return compete_cmd::handle_zen_compete(spec, *dry_run);
    }

    // STORY-360: live-advisor registration. `aida advisor` writes to
    // `~/.aida/advisor.toml` (per-user, not per-project) and is intended to
    // be runnable from anywhere — including outside any AIDA project. Reads
    // no requirement store. trace:STORY-360 | ai:claude
    // STORY-262: `aida advisor schedule` needs the requirement store (it
    // files TASKs into the queue), so it must NOT short-circuit here — it
    // falls through to the post-storage-init dispatch. Only the
    // store-less registration subcommands run early.
    // trace:STORY-262 | ai:claude
    if let Command::Advisor { command, .. } = &cli.command {
        // STORY-559: `advisor status` (default = dashboard) aggregates the
        // requirement store (burndown / backlog / queue / leases), so it must
        // fall through to the post-storage dispatch. The narrow
        // `--registration` view reads only `~/.aida/advisor.toml` and stays in
        // this store-less early handler so it still works outside a project.
        // STORY-618: bare `aida advisor` (command = None) IS the worklist, which
        // reads the store, so it also falls through. trace:STORY-559 STORY-618
        let needs_store = match command {
            None => true,
            Some(c) => {
                matches!(c, AdvisorCommand::Schedule(_))
                    || matches!(
                        c,
                        AdvisorCommand::Status {
                            registration: false,
                            ..
                        }
                    )
            }
        };
        if !needs_store {
            if let Some(c) = command {
                return handle_advisor_command(c);
            }
        }
    }

    // STORY-301: drain-state introspection. Dispatched before storage init —
    // it reads only `.aida/drain-state.json`, no requirement store.
    // trace:STORY-301 | ai:claude
    if let Command::Drain(drain_cmd) = &cli.command {
        return handle_drain_command(drain_cmd);
    }

    // STORY-248: stack-graph introspection. Dispatched before storage
    // init — reads only `.aida/stacks.json`, no requirement store.
    // Mirrors the STORY-301 drain dispatch pattern. trace:STORY-248
    if let Command::Stack(stack_cmd) = &cli.command {
        return handle_stack_command(stack_cmd);
    }

    // TASK-294: worker-directive introspection. Dispatched before storage
    // init — reads only `.aida/worker.cmd`, no requirement store. Mirrors
    // the STORY-301 drain dispatch pattern exactly. trace:TASK-294 | ai:claude
    if let Command::Worker(worker_cmd) = &cli.command {
        return handle_worker_command(worker_cmd);
    }

    // TASK-398: headless-log tailer. Dispatched before storage init — reads
    // only `.aida/headless-logs/`, no requirement store. trace:TASK-398
    if let Command::Headless(headless_cmd) = &cli.command {
        return handle_headless_command(headless_cmd);
    }

    // STORY-325: punt ledger CLI command. Dispatched before storage init —
    // it reads only `.aida/punts.jsonl`, no requirement store.
    // trace:STORY-325 | ai:antigravity
    if let Command::Punts(punts_cmd) = &cli.command {
        return handle_punts_command(punts_cmd.clone());
    }

    // STORY-439: autonomy / calibration surface. Dispatched before storage
    // init — it reads only `.aida/complexity-calibration/`, no requirement
    // store. trace:STORY-439 | ai:claude
    if let Command::Autonomy { command } = &cli.command {
        return autonomy_cmd::handle_autonomy_command(command.as_ref());
    }

    // TASK-394: the `--no-human` acknowledgement marker just touches/removes a
    // file (no requirement store). Dispatch before storage init so it works on
    // a fresh clone too. trace:TASK-394 | ai:claude
    if let Command::NoHuman(no_human_cmd) = &cli.command {
        return handle_no_human_command(no_human_cmd);
    }

    // Determine which requirements file to use
    // trace:REQ-0231 | ai:claude:high
    let requirements_path = if let Some(ref explicit_file) = cli.file {
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
        if let Some(store_path) = detect_distributed_store() {
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
        let initial_path = determine_requirements_path(cli.project.as_deref())?;

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
        // TASK-728: `aida done` is a distributed-mode (default) verb. Legacy
        // --centralized mode keeps the explicit edit path. trace:TASK-727
        Command::Done { .. } => {
            anyhow::bail!(
                "`aida done` is available in the default (distributed) mode. In legacy \
                 --centralized mode, use `aida edit <spec> --status completed`."
            );
        }
        // STORY-741: `aida awaiting` (the unified coordination inbox) is a
        // distributed-mode surface — briefs, findings, escalations, reviewer
        // verdicts and mail are all distributed-mode concepts. trace:STORY-741
        Command::Awaiting { .. } => {
            anyhow::bail!("`aida awaiting` is available in the default (distributed) mode.");
        }
        Command::Protocol(_) => {
            anyhow::bail!("`aida protocol` is available in the default (distributed) mode.");
        }
        Command::Schedule(_) => {
            anyhow::bail!("`aida schedule` is available in the default (distributed) mode.");
        }
        // TASK-777: the fasttrack lane is a distributed-mode convention (it
        // queues + batches), so it isn't wired into the deprecated legacy
        // storage path. trace:TASK-777
        Command::Fasttrack { .. } => {
            anyhow::bail!(
                "`aida fasttrack` is available in the default (distributed) mode. In legacy \
                 --centralized mode, use `aida add \"<title>\" --status approved`."
            );
        }
        // STORY-706: `aida focus` scopes reads by walking the requirement
        // hierarchy graph — a distributed-mode (cache-backed) concept.
        Command::Focus { .. } => {
            anyhow::bail!(
                "`aida focus` is available in the default (distributed) mode. Legacy \
                 --centralized mode has no per-worktree focus context."
            );
        }
        Command::Add {
            title,
            title_positional,
            description,
            description_from_file,
            description_stdin,
            status,
            priority,
            r#type,
            owner,
            feature,
            tags,
            prefix,
            parent,
            blocked_by: _, // STORY-446: blocked-by edges land on the git-backend path
            force_parent,
            interactive,
            // STORY-333: legacy centralized backend does not persist the
            // human-only marker; ignore the flags here. trace:TASK-130 | ai:claude
            human_only: _,
            no_human_only: _,
            effort: _,
            // TASK-754: --queue/--batch are git-backend-path features (they
            // reuse the backlog-groom enqueue helper); the deprecated
            // centralized backend ignores them. trace:TASK-754 | ai:claude
            // BUG-528: --for routes the queued spec; also git-backend-only.
            // trace:BUG-528 | ai:claude
            queue: _,
            batch: _,
            r#for: _,
            // FR-283: the numeric weight is git-canonical only; the deprecated
            // centralized backend ignores it. trace:FR-283 | ai:claude
            weight: _,
            // TASK-1267: execution modes are git-canonical only.
            mode: _,
            // STORY-1427: intake gates run on the git-canonical path only.
            gates: _,
        } => {
            // TASK-725: positional title (`aida add "do X"`) — --title wins.
            let title = title.clone().or_else(|| title_positional.clone());
            let title = &title;
            // trace:BUG-17 | ai:claude — resolve description from inline,
            // file, or stdin sources before dispatching.
            let resolved_description =
                resolve_description(description, description_from_file, *description_stdin)?;
            let description = &resolved_description;

            // trace:BUG-22 | ai:claude — warn if the title looks shell-mangled.
            if let Some(ref t) = title {
                if let Some(msg) = suspicious_title_signal(t) {
                    eprintln!("{} {}", "Warning:".yellow().bold(), msg);
                }
            }

            // Default to interactive mode if no specific arguments are provided
            let should_be_interactive = *interactive
                || (title.is_none()
                    && description.is_none()
                    && status.is_none()
                    && priority.is_none()
                    && r#type.is_none()
                    && owner.is_none()
                    && feature.is_none()
                    && tags.is_none()
                    && prefix.is_none()
                    && parent.is_none());

            if should_be_interactive {
                add_requirement_interactive(&storage)?;
            } else {
                add_requirement_cli(
                    &storage,
                    title,
                    description,
                    status,
                    priority,
                    r#type,
                    owner,
                    feature,
                    tags,
                    prefix,
                    parent,
                    *force_parent,
                )?;
            }
        }
        Command::List {
            status,
            priority,
            r#type,
            feature,
            tags,
            json,
            ..
        } => {
            // Legacy SQLite path doesn't honor role scope (deprecated
            // backend). STORY-244 `--json` is git-backend only — the
            // legacy path stays human-only since `aida init --centralized`
            // is deprecated. trace:STORY-244 | ai:claude
            if *json {
                anyhow::bail!(
                    "`aida list --json` requires the default git-canonical \
                     backend; the deprecated --centralized SQLite mode does \
                     not emit JSON. Run `aida init` (no flags) on a new \
                     project, or upgrade an existing one."
                );
            }
            list_requirements(&storage, status, priority, r#type, feature, tags)?;
        }
        Command::Show { id, .. } => {
            // Legacy SQLite show_requirement always prints comments inline,
            // so the --comments flag is a no-op here. Git backend honors it.
            show_requirement(&storage, id)?;
        }
        Command::Approvals { .. } => {
            // trace:STORY-1173 | ai:codex
            anyhow::bail!(
                "aida approvals requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Graph {
            id: graph_id,
            blocked_by,
            blocks,
            tree,
            impact,
            follow,
            depth,
            json,
            cmd,
        } => {
            let store = storage.load_for_read()?;
            let (id, blocked_by, blocks, tree, impact) = match cmd {
                Some(GraphCommand::BlockedBy { id }) => (id.as_str(), true, false, false, false),
                Some(GraphCommand::Blocks { id }) => (id.as_str(), false, true, false, false),
                Some(GraphCommand::Tree { id }) => (id.as_str(), false, false, true, false),
                Some(GraphCommand::Impact { id }) => (id.as_str(), false, false, false, true),
                None => {
                    let id = graph_id.as_deref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "aida graph needs a spec id, e.g. `aida graph tree STORY-1`"
                        )
                    })?;
                    if *blocked_by {
                        note_hidden_alias(
                            concat!("aida graph <id> ", "--blocked-by"),
                            "aida graph blocked-by <id>",
                        );
                    }
                    if *blocks {
                        note_hidden_alias(
                            concat!("aida graph <id> ", "--blocks"),
                            "aida graph blocks <id>",
                        );
                    }
                    if *tree {
                        note_hidden_alias(
                            concat!("aida graph <id> ", "--tree"),
                            "aida graph tree <id>",
                        );
                    }
                    if *impact {
                        note_hidden_alias(
                            concat!("aida graph <id> ", "--impact"),
                            "aida graph impact <id>",
                        );
                    }
                    (id, *blocked_by, *blocks, *tree, *impact)
                }
            };
            graph_cmd::handle_graph_command(
                &store, id, blocked_by, blocks, tree, impact, follow, *depth, *json,
            )?;
        }
        Command::Criteria {
            spec,
            json,
            window_days,
        } => {
            let store = storage.load()?;
            let project_root = find_project_root()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| ".".into()));
            // `coverage` / `gap` is the project-wide report, never a spec id.
            // trace:STORY-1487 | ai:claude
            criteria_coverage::dispatch_criteria(&project_root, &store, spec, *window_days, *json)?;
        }
        Command::Reconstitute {
            spec,
            json,
            dry_run,
            yes,
        } => {
            let store = storage.load()?;
            let project_root = find_project_root()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| ".".into()));
            reconstitute::handle_reconstitute_command(
                &project_root,
                &store,
                spec,
                reconstitute::ReconstituteOptions {
                    json: *json,
                    dry_run: *dry_run,
                    yes: *yes,
                },
            )?;
        }
        Command::Harvest {
            spec,
            from,
            pr,
            base,
            yes_all,
            dry_run,
            json,
        } => {
            let store = storage.load()?;
            let project_root = find_project_root()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| ".".into()));
            harvest::handle_harvest_command(
                &project_root,
                &store,
                spec,
                harvest::HarvestOptions {
                    from: from.clone(),
                    pr: *pr,
                    base: base.clone(),
                    yes_all: *yes_all,
                    dry_run: *dry_run,
                    json: *json,
                },
            )?;
        }
        Command::Brief {
            agent,
            spec,
            note,
            depends_on,
            as_deep_link,
            notify,
            authorized_by,
            cmd,
        } => {
            let store = storage.load()?;
            let project_root = find_project_root()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| ".".into()));
            brief_cmd::handle_brief_command(
                agent.as_deref(),
                spec.as_deref(),
                note.as_deref(),
                depends_on.as_deref(),
                *as_deep_link,
                *notify,
                authorized_by.as_deref(),
                cmd,
                &store,
                &project_root,
            )?;
        }
        Command::Agent(agent_cmd) => {
            handle_agent_command(agent_cmd)?;
        }
        Command::Terminal(cmd) => {
            handle_terminal_command(cmd)?;
        }
        Command::Edit {
            id,
            title,
            description,
            description_from_file,
            description_stdin,
            verify,
            clear_verify,
            status,
            priority,
            r#type,
            owner,
            feature,
            tags,
            add_tag,
            remove_tag,
            blocked_by: _, // STORY-446: blocked-by edges land on the git-backend path
            remove_blocked_by: _,
            // TASK-1176: the supersede lineage edge is git-canonical only, same
            // rule as blocked-by. trace:TASK-1176 | ai:claude
            superseded_by: _,
            // STORY-1434: the carve-out edge + comment are git-canonical
            // only, same rule as superseded_by. trace:STORY-1434 | ai:claude
            carve_out: _,
            carve_into: _,
            carve_reason: _,
            // STORY-476: external refs land on the git-backend path only;
            // the legacy SQLite path ignores them. trace:STORY-476 | ai:claude
            add_ref: _,
            remove_ref: _,
            interactive,
            strict: _, // legacy SQLite path ignores session leases
            force: _,  // TASK-47 guard only applies to git-canonical Edit
            // STORY-333: legacy centralized backend does not persist the
            // human-only marker; ignore the flags here.
            // trace:STORY-333 | ai:claude
            human_only: _,
            no_human_only: _,
            // TASK-1148: the narrative fields are git-canonical only; the legacy
            // centralized backend does not persist them. trace:TASK-1148 | ai:claude
            implementation_summary: _,
            risk_notes: _,
            test_coverage_notes: _,
            // STORY-776: execution_mode is git-canonical only, same rule.
            mode: _,
            // STORY-634: the multi-repo origin dimension is git-canonical
            // only, same rule.
            origin: _,
            // FR-283: the numeric weight is git-canonical only, same rule.
            weight: _,
        } => {
            // trace:STORY-1673 | ai:codex
            anyhow::ensure!(
                verify.is_none() && !*clear_verify,
                "--verify and --clear-verify require a git-canonical store"
            );

            // trace:BUG-1234 | ai:codex — keep legacy and git-canonical edit
            // behavior aligned for file/stdin description replacement.
            let resolved_description =
                resolve_edit_description(description, description_from_file, *description_stdin)?;
            let description = &resolved_description;
            // If any flags provided, use non-interactive mode; otherwise interactive
            // trace:TASK-351 | ai:claude — --add-tag / --remove-tag count too
            let has_flags = title.is_some()
                || description.is_some()
                || status.is_some()
                || priority.is_some()
                || r#type.is_some()
                || owner.is_some()
                || feature.is_some()
                || tags.is_some()
                || !add_tag.is_empty()
                || !remove_tag.is_empty();

            if *interactive || !has_flags {
                edit_requirement_interactive(&storage, id)?;
            } else {
                edit_requirement_cli(
                    &storage,
                    id,
                    title,
                    description,
                    status,
                    priority,
                    r#type,
                    owner,
                    feature,
                    tags,
                    add_tag,
                    remove_tag,
                )?;
            }
        }
        Command::Del { id, yes } => {
            delete_requirement(&storage, id, *yes)?;
        }
        Command::Feature(feature_cmd) => {
            feature_cmd::handle_feature_command(feature_cmd, &storage)?;
        }
        Command::Db(db_cmd) => {
            db_cmd::handle_db_command(db_cmd, &requirements_path)?;
        }
        Command::Cache(_) => {
            anyhow::bail!(
                "aida cache commands are only available in git-canonical (distributed) mode. \
                 Run `aida init --distributed` first, or pass --file pointing to a git store."
            );
        }
        Command::Record(_) => {
            // STORY-582: the processing-record trail is a git-canonical field.
            anyhow::bail!(
                "aida record commands are only available in git-canonical (distributed) mode. \
                 Run `aida init` (defaults to distributed) first."
            );
        }
        Command::Node(_) => {
            anyhow::bail!(
                "aida node commands are only available in git-canonical (distributed) mode. \
                 Run `aida init` (defaults to distributed) first."
            );
        }
        Command::Mailbox(_) => {
            anyhow::bail!(
                "aida mailbox commands are only available in git-canonical (distributed) mode. \
                 Run `aida init` (defaults to distributed) first."
            );
        }
        Command::Supervise(_) => {
            anyhow::bail!(
                "aida supervise commands are only available in git-canonical (distributed) mode. \
                 Run `aida init` (defaults to distributed) first."
            );
        }
        // trace:STORY-1218 | ai:claude
        Command::Shift(_) => {
            anyhow::bail!(
                "aida shift commands are only available in git-canonical (distributed) mode. \
                 Run `aida init` (defaults to distributed) first."
            );
        }
        Command::Status {
            spec: _,
            idle_minutes: _,
            no_dev_context,
            short: _,
            json: _,
            queue: _,
            ci: _,
            no_ci: _,
            cleanup: _,
            activity: _,
            since: _,
            awaiting: _,
            verbose: _,
            no_hygiene: _,
            all: _,
            stale: _,
            full: _,
            no_focus: _,
        } => {
            handle_status_command(*no_dev_context, None, &storage)?;
        }
        // STORY-640: `aida team` reads the shared node roster on the orphan
        // `aida-store` branch — a distributed-mode concept. Legacy
        // --centralized stores have no roster. trace:STORY-640 | ai:claude
        Command::Team { .. } => {
            anyhow::bail!(
                "`aida team` is available in the default (distributed) mode — it lists the \
                 nodes sharing the orphan `aida-store` branch. Legacy --centralized mode has \
                 no node roster."
            );
        }
        // TASK-845: the person-alias registry lives on the orphan `aida-store`
        // branch — a distributed-mode concept. trace:TASK-845 | ai:claude
        Command::Identity { .. } => {
            anyhow::bail!(
                "`aida identity` is available in the default (distributed) mode — it manages the \
                 shared person-alias registry on the orphan `aida-store` branch. Legacy \
                 --centralized mode has no shared registry."
            );
        }
        Command::Usage {
            since,
            unused,
            errors,
            json,
            limit,
            auto_complete,
            failures,
            pattern,
            health,
            read_write,
            slowest,
            events,
            cmd,
            slower_than,
            action,
        } => {
            // TASK-1427's derived token ledger is separate from the historical
            // command-shape telemetry handled below.
            if let Some(action) = action {
                match action {
                    UsageCommand::Rebuild { project, json } => {
                        let root = project.clone().unwrap_or(find_project_root()?);
                        token_ledger::rebuild(&root, *json || output_format_is_json())?;
                        return Ok(());
                    }
                    UsageCommand::Show {
                        spec,
                        group_by,
                        cost,
                        json,
                        toon,
                    } => {
                        token_ledger::show(
                            &find_project_root()?,
                            spec,
                            group_by,
                            *cost,
                            *json || output_format_is_json(),
                            *toon,
                        )?;
                        return Ok(());
                    }
                    _ => {}
                }
            }
            // TASK-266: only the `--auto-complete` view needs the store
            // (to resolve drafted-BUG statuses) — keep plain `aida usage`
            // store-load-free. STORY-530: the `--health` catalog also needs
            // the store for draft-inbox depth + burn-down velocity.
            let (
                unused,
                errors,
                auto_complete,
                failures,
                pattern,
                health,
                slowest,
                events,
                timeline,
            ) = normalize_usage_mode(
                unused.as_deref(),
                *errors,
                *auto_complete,
                *failures,
                *pattern,
                *health,
                *slowest,
                *events,
                action.as_ref(),
            );
            let store = if auto_complete || health {
                storage.load().ok()
            } else {
                None
            };
            usage_cmd::handle_usage_command(
                since,
                unused,
                errors,
                // trace:BUG-1289 | ai:claude
                *json || output_format_is_json(),
                *limit,
                auto_complete,
                failures,
                pattern,
                health,
                *read_write,
                slowest,
                events,
                timeline,
                cmd.as_deref(),
                *slower_than,
                store.as_ref(),
            )?;
        }
        Command::Metrics { cmd } => {
            // trace:STORY-477 | ai:claude — reporting layer over the local
            // telemetry logs; backend-agnostic.
            metrics_cmd::handle_metrics_command(cmd)?;
        }
        Command::FieldStudy { cmd } => {
            // trace:SPIKE-67 | ai:claude — observe-only rule-adherence study
            // over the git log; opt-in, local-only.
            field_study_cmd::handle_field_study_command(cmd)?;
        }
        Command::Push { .. } => {
            anyhow::bail!(
                "`aida push` requires a git-canonical store. Run `aida init` (or upgrade from \
                 the deprecated centralized backend with `aida db export-git`)."
            );
        }
        Command::Pull { .. } => {
            anyhow::bail!(
                "`aida pull` requires a git-canonical store. Run `aida init` (or upgrade from \
                 the deprecated centralized backend with `aida db export-git`)."
            );
        }
        Command::Digest { .. } => {
            anyhow::bail!(
                "`aida digest` requires a git-canonical store. Run `aida init` (or upgrade from \
                 the deprecated centralized backend with `aida db export-git`)."
            );
        }
        Command::Fetch { .. } => {
            anyhow::bail!(
                "`aida fetch` requires a git-canonical store. Run `aida init` (or upgrade from \
                 the deprecated centralized backend with `aida db export-git`)."
            );
        }
        Command::Rebase { .. } => {
            anyhow::bail!(
                "`aida rebase` requires a git-canonical store. Run `aida init` (or upgrade from \
                 the deprecated centralized backend with `aida db export-git`)."
            );
        }
        Command::Upgrade { .. } => unreachable!("upgrade is dispatched before storage init"),
        Command::Memories(_) => unreachable!("memories is dispatched before storage init"),
        Command::Schema { .. } => unreachable!("schema is dispatched before storage init"),
        Command::Dev(_) => unreachable!("dev is dispatched before storage init"),
        Command::Driver(_) => unreachable!("driver is dispatched before storage init"),
        Command::Release { .. } => unreachable!("release is dispatched before storage init"),
        Command::Burndown(_) => unreachable!("burndown is dispatched before storage init"),
        Command::Health { .. } => unreachable!("health is dispatched before storage init"),
        // trace:TASK-1231 | ai:claude
        Command::Autoprogress { .. } => {
            unreachable!("autoprogress is dispatched before storage init")
        }
        Command::Groom { .. } => {
            unreachable!("groom (assess/intake) is dispatched before storage init")
        }
        // trace:TASK-1147
        Command::Autopilot(_) => {
            unreachable!("autopilot is dispatched before storage init")
        }
        Command::Why { .. } => unreachable!("why is dispatched before storage init"),
        Command::Explain { .. } => unreachable!("explain is dispatched before storage init"),
        Command::Wiki(_) => unreachable!("wiki is dispatched before storage init"),
        Command::Intent { .. } => unreachable!("intent is dispatched before storage init"),
        // trace:STORY-696
        Command::Ps { .. } => unreachable!("ps is dispatched before storage init"),
        Command::MergeLock { .. } => {
            unreachable!("merge-lock is dispatched before storage init")
        }
        Command::MergeHold { .. } => {
            unreachable!("merge-hold is dispatched before storage init")
        }
        Command::Watch { .. } => unreachable!("watch is dispatched before storage init"),
        Command::Contract { .. } => unreachable!("contract is dispatched before storage init"),
        // trace:TASK-1034
        Command::Integrate { .. } => {
            unreachable!("integrate is dispatched before storage init")
        }
        // trace:TASK-957
        Command::Claim { .. } => unreachable!("claim is dispatched before storage init"),
        Command::Unclaim { .. } => unreachable!("unclaim is dispatched before storage init"),
        Command::Spec(_) => unreachable!("spec subcommands are dispatched before storage init"),
        Command::Worktree(_) => {
            unreachable!("worktree subcommands are dispatched before storage init")
        }
        Command::Doctor { .. } => unreachable!("doctor is dispatched before storage init"),
        Command::Store(_) => unreachable!("store is dispatched before storage init"),
        Command::Remote(_) => unreachable!("remote is dispatched before storage init"),
        Command::Sandbox(_) => unreachable!("sandbox is dispatched before storage init"),
        Command::HelpAll => unreachable!("help-all is dispatched before storage init"),
        Command::Commands { .. } => unreachable!("commands is dispatched before storage init"),
        Command::Alias { .. } => unreachable!("alias is dispatched before storage init"),
        Command::Plan(_) => unreachable!("plan is dispatched before storage init"),
        Command::Deps(_) => unreachable!("deps is dispatched before storage init"),
        Command::Lint { .. } => unreachable!("lint is dispatched before storage init"),
        Command::Verify(_) => unreachable!("verify is dispatched before storage init"),
        Command::Gate(_) => unreachable!("gate is dispatched before storage init"),
        Command::Lifecycle { .. } => {
            unreachable!("lifecycle is dispatched before storage init")
        }
        Command::Changelog(_) => unreachable!("changelog is dispatched before storage init"),
        Command::Manual { .. } => unreachable!("manual is dispatched before storage init"),
        Command::Derisk { .. } => unreachable!("derisk is dispatched before storage init"),
        Command::Ultraplan { .. } => unreachable!("ultraplan is dispatched before storage init"),
        Command::Compete { .. } => unreachable!("compete is dispatched before storage init"),
        Command::Goal { .. } => unreachable!("goal is dispatched before storage init"),
        Command::Commit { .. } => unreachable!("commit is dispatched before storage init"),
        Command::Internal { .. } => unreachable!("internal is dispatched before storage init"),
        Command::Tui { .. } => unreachable!("tui is dispatched before storage init"),
        Command::Role(_) => unreachable!("role is dispatched before storage init"),
        Command::Statusbar { .. } => unreachable!("statusbar is dispatched before storage init"),
        // trace:TASK-1167
        Command::Tail { .. } => unreachable!("tail is dispatched before storage init"),
        Command::Statusline { .. } => unreachable!("statusline is dispatched before storage init"),
        Command::BgFetch { .. } => unreachable!("_bg-fetch is dispatched before storage init"),
        Command::Away | Command::Home | Command::Presence { .. } | Command::Solo { .. } => {
            unreachable!("presence/solo commands are dispatched before storage init")
        }
        // trace:TASK-784
        Command::Whoami => unreachable!("whoami is dispatched before storage init"),
        Command::Session(_) => unreachable!("session is dispatched before storage init"),
        Command::Triage(_) => unreachable!("triage is dispatched before storage init"),
        Command::Lock(_) => unreachable!("lock is dispatched before storage init"),
        Command::Pr(_) => unreachable!("pr is dispatched before storage init"),
        Command::Ship { .. } => unreachable!("ship is dispatched before storage init"),
        Command::Orchestrator(_) => {
            unreachable!("orchestrator is dispatched before storage init")
        }
        // STORY-721: `aida zen <spec>` autonomous implement+ship drive on the
        // legacy (centralized) storage path. The introspection subcommands
        // short-circuit before storage init; this arm only sees the spec-drive
        // form. trace:STORY-721 | ai:claude
        Command::Zen {
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
        Command::Drain(_) => unreachable!("drain is dispatched before storage init"),
        Command::Stack(_) => unreachable!("stack is dispatched before storage init"),
        Command::Worker(_) => unreachable!("worker is dispatched before storage init"),
        Command::Headless(_) => unreachable!("headless is dispatched before storage init"),
        Command::Punts(_) => unreachable!("punts is dispatched before storage init"),
        Command::Autonomy { .. } => unreachable!("autonomy is dispatched before storage init"),
        Command::NoHuman(_) => unreachable!("no-human is dispatched before storage init"),
        Command::Rel(rel_cmd) => {
            relationship_cmd::handle_relationship_command(rel_cmd, &storage)?;
        }
        Command::RelDef(rel_def_cmd) => {
            rel_def_cmd::handle_rel_def_command(rel_def_cmd, &storage)?;
        }
        Command::Comment(comment_cmd) => {
            comment_cmd::handle_comment_command(comment_cmd, &storage)?;
        }
        // STORY-633: glyph CLI surface — config.toml writer, no store needed.
        // trace:STORY-633 | ai:claude
        Command::Config(ConfigCommand::Glyph(glyph_cmd)) => {
            config_cmd::handle_config_glyph(glyph_cmd)?;
        }
        Command::Config(config_cmd) => {
            config_cmd::handle_config_command(config_cmd, &storage)?;
        }
        Command::Type(type_cmd) => {
            type_cmd::handle_type_command(type_cmd, &storage)?;
        }
        // trace:TASK-1729 | ai:codex
        Command::Regen(_) => {
            anyhow::bail!(
                "regeneration requires a git-canonical store; migrate the legacy store first"
            );
        }
        Command::Export { format, output, id } => {
            import_export_cmd::handle_export_command(
                &storage,
                format,
                output.as_deref(),
                id.as_deref(),
            )?;
        }
        Command::Import {
            file,
            parent,
            on_conflict,
        } => {
            import_export_cmd::handle_import_command(
                &storage,
                file,
                parent.as_deref(),
                on_conflict,
            )?;
        }
        Command::UserGuide { dark } => {
            open_user_guide(*dark)?;
        }
        Command::Server(server_cmd) => {
            server_cmd::handle_server_command(server_cmd, cli.server.as_deref())?;
        }
        Command::Trace(trace_cmd) => {
            crate::trace_cmd::handle_trace_command(trace_cmd, &storage)?;
        }
        Command::Review {
            spec,
            no_agent,
            allow_stale_base,
            target_branch,
            cmd,
        } => {
            // trace:STORY-553 | ai:claude — `aida review <SPEC>` drives the
            // human-decision review; `prompt` / `assemble` stay the helper
            // subcommands. The legacy (SQLite) path supports the prompt
            // helpers only; the spec-review verb needs the git-canonical
            // backend's surface resolution, so guide the user there.
            match (spec, cmd) {
                (Some(_), _) => anyhow::bail!(
                    "`aida review <SPEC>` requires the default git-canonical \
                     backend; the deprecated --centralized SQLite mode does \
                     not support it."
                ),
                (None, Some(review_cmd)) => handle_review_command(review_cmd, &storage)?,
                (None, None) => {
                    let _ = (no_agent, allow_stale_base, target_branch);
                    anyhow::bail!("pass a spec id (`aida review <SPEC>`) or a subcommand (`prompt` / `assemble`)");
                }
            }
        }
        Command::Report {
            recheck,
            command: report_cmd,
        } => {
            let db_path_str = requirements_path.display().to_string();
            report_cmd::handle_report_command(
                *recheck,
                report_cmd.as_ref(),
                &storage,
                &db_path_str,
            )?;
        }
        Command::Scaffold(scaffold_cmd) => {
            scaffold_cmd::handle_scaffold_command(scaffold_cmd, &storage, &requirements_path)?;
        }
        Command::Gitlab(gitlab_cmd) => {
            tracker_cmd::handle_gitlab_command(gitlab_cmd, &storage)?;
        }
        Command::Github(github_cmd) => {
            tracker_cmd::handle_github_command(github_cmd, &storage)?;
        }
        Command::Jira(jira_cmd) => {
            tracker_cmd::handle_jira_command(jira_cmd, &storage)?;
        }
        Command::McpServe => {
            // STORY-361: project root for coordination tools.
            let project_root =
                find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
            mcp::run_mcp_server(&storage, project_root)?;
        }
        Command::Mcp(mcp_cmd) => {
            // STORY-361: management commands for the MCP coordination surface.
            handle_mcp_command(mcp_cmd)?;
        }
        Command::Docs(docs_cmd) => {
            // trace:FR-1-077 | ai:claude
            doc_cmd::handle_docs_command(docs_cmd, &storage)?;
        }
        Command::Grep {
            pattern,
            ignore_case,
            extended_regex,
            after_context,
            before_context,
            context,
            field,
            status,
            r#type,
            feature,
            files_with_matches,
            count,
            invert_match,
        } => {
            grep_requirements(
                &storage,
                pattern,
                *ignore_case,
                *extended_regex,
                *after_context,
                *before_context,
                *context,
                field.as_deref(),
                status.as_deref(),
                r#type.as_deref(),
                feature.as_deref(),
                *files_with_matches,
                *count,
                *invert_match,
            )?;
        }
        Command::Queue(queue_cmd) => {
            handle_queue_command(queue_cmd, &storage, &requirements_path)?;
        }
        Command::Do { spec, mode, force } => {
            run_do_drive(&storage, spec, mode.as_deref(), *force)?;
        }
        Command::Load(load_cmd) => {
            load_cmd::handle_load_command(load_cmd, &storage)?;
        }
        // STORY-444 + STORY-451: `aida backlog` owns grooming plus the
        // `load` alias for quantitative effort summaries.
        Command::Backlog(backlog_cmd) => match backlog_cmd {
            BacklogCommand::Load => load_cmd::handle_load_command(&LoadCommand::Backlog, &storage)?,
            _ => backlog::handle_backlog_command(backlog_cmd, &storage)?,
        },
        // TASK-218: top-level alias in the legacy SQLite dispatch path —
        // forwards to the same handler as `aida queue rework SPEC`.
        // trace:TASK-218 | ai:claude
        Command::Rework {
            id,
            work,
            r#for,
            tail,
            status,
            reason,
            resume,
            force,
            steal,
            permission_mode,
            no_pull,
            user,
        } => {
            // STORY-1429: an omitted ID opens the triage loop. trace:STORY-1429 | ai:claude
            crate::queue_cmd::handle_rework_entry(
                &storage,
                id.as_deref(),
                &crate::queue_cmd::ReworkFlags {
                    work: *work,
                    for_role: r#for.as_deref(),
                    tail: *tail,
                    status: status.as_deref(),
                    reason: reason.as_deref(),
                    resume: *resume,
                    force: *force,
                    steal: *steal,
                    permission_mode: permission_mode.as_deref(),
                    no_pull: *no_pull,
                    user: user.as_deref(),
                },
            )?;
        }
        Command::Search {
            query,
            case_sensitive,
            status,
            feature,
            ..
        } => {
            // Search is a simplified version of grep with sensible defaults:
            // - Case insensitive by default (unless -s/--case-sensitive)
            // - Searches all text fields (title, description, comments)
            // The --limit flag is honored by the git backend's FTS5 path; the
            // legacy grep walks the in-memory store and ignores it.
            grep_requirements(
                &storage,
                query,
                !case_sensitive, // invert: case_sensitive=false means ignore_case=true
                false,           // no extended regex
                0,               // no after context
                0,               // no before context
                None,            // no context
                None,            // search all fields
                status.as_deref(),
                None, // no type filter
                feature.as_deref(),
                false, // show full matches, not just IDs
                false, // don't show count
                false, // don't invert match
            )?;
        }
        Command::Init { .. } => {
            // Handled before path resolution above; unreachable
            unreachable!("Init command should be handled before path resolution");
        }
        Command::History {
            kind: Some(kind),
            since,
            until,
            author,
            limit,
            ..
        } => {
            // STORY-1436: the event feed is local runtime state, readable
            // without the orphan store. trace:STORY-1436 | ai:claude
            history_kind_report(
                kind,
                since.as_deref(),
                until.as_deref(),
                author.as_deref(),
                effective_history_limit(*limit),
            )?;
        }
        Command::History { .. } => {
            // History walks the orphan branch; only meaningful in git-canonical
            // mode, which is dispatched via handle_git_backend_command. Falling
            // through to legacy means the user is on a SQLite-only project.
            anyhow::bail!(
                "aida history requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Doc(_) => {
            // `aida doc` is git-canonical-only — Doc entries live in the
            // orphan store alongside every other requirement. Legacy SQLite
            // projects fall through to this arm. trace:STORY-104 | ai:claude
            anyhow::bail!(
                "aida doc requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Rules(_) => {
            // `aida rules` reads the cache projection — git-canonical only.
            // trace:SPIKE-31 | ai:claude
            anyhow::bail!(
                "aida rules requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Findings { .. } => {
            // `aida findings` queries the cache for `from-review:` /
            // `from-implementer:` tags; the deprecated SQLite backend has no
            // equivalent. trace:STORY-278 trace:STORY-285 | ai:claude
            anyhow::bail!(
                "aida findings requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Advisor { .. } => {
            // STORY-360: `aida advisor` (register/unregister/status) dispatches
            // before storage resolution. STORY-262's `advisor schedule`
            // dispatches in `handle_git_backend_command` (it needs the store).
            // This legacy-SQLite arm is unreachable for both. The schedule
            // subcommand and the STORY-618 bare worklist additionally require
            // the git-canonical store, so a legacy project never reaches it.
            // trace:STORY-360 trace:STORY-262 trace:STORY-618 | ai:claude
            unreachable!("Command::Advisor dispatched before legacy storage init");
        }
        Command::Questions { .. } => {
            // `aida questions` reads/writes the `decision_request` field on a
            // spec; the deprecated SQLite backend does not persist it.
            // trace:STORY-522 | ai:claude
            anyhow::bail!(
                "aida questions requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Decide { .. } => {
            // `aida decide` routes to `aida questions answer`/`clarify`, both of
            // which need the git-canonical store. trace:TASK-779 | ai:claude
            anyhow::bail!(
                "aida decide requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Research { .. } => {
            // `aida research` attaches a comment + escalates a decision_request
            // on a spike; the deprecated SQLite backend persists neither.
            // trace:STORY-568 | ai:claude
            anyhow::bail!(
                "aida research requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Punt { .. } => {
            // `aida punt` writes the NeedsAttention status + structured punt
            // metadata; the deprecated SQLite backend does not persist it.
            // trace:STORY-332 | ai:claude
            anyhow::bail!(
                "aida punt requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Archive { .. } | Command::Unarchive { .. } => {
            // STORY-441: archive uses the git-canonical write path.
            anyhow::bail!(
                "aida archive/unarchive requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Defer { .. } | Command::Undefer { .. } => {
            // STORY-584: defer uses the git-canonical write path.
            anyhow::bail!(
                "aida defer/undefer requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Assign { .. } | Command::Unassign { .. } => {
            // STORY-639: assignment uses the git-canonical write path.
            anyhow::bail!(
                "aida assign/unassign requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }

        Command::StateSnapshot { .. } => {
            // The finish-state preamble's Spec row needs git-canonical
            // metadata (spec_id + status). trace:TASK-391 | ai:claude
            anyhow::bail!(
                "aida state-snapshot requires the distributed git-canonical \
                 store (run `aida init` to migrate, or this project is on \
                 the deprecated --centralized backend)"
            );
        }
        Command::ImportPlan { .. } => {
            // The plan-review handshake tags + comments on the spec via the
            // git-canonical write path. trace:TASK-516 | ai:claude
            anyhow::bail!(
                "aida import-plan requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Human { .. } => {
            // The `aida human` bottleneck view reads the git-canonical store
            // (the burndown classifier needs the relationship graph + tags);
            // the deprecated --centralized backend can't back it. trace:TASK-746
            anyhow::bail!(
                "aida human requires the distributed git-canonical store \
                 (run `aida init` to migrate, or this project is on the \
                 deprecated --centralized backend)"
            );
        }
        Command::Skill(_) => {
            unreachable!("Command::Skill dispatched before storage init");
        }
        Command::Notify(_) => {
            unreachable!("Command::Notify dispatched before storage init");
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

/// `aida advisor` — manage the live-advisor registration the
/// `--no-human=both` orchestrator reads (STORY-360). Three actions:
///   - `register` writes the current session's UUID + project slug to
///     `~/.aida/advisor.toml`. Auto-detects from `CLAUDE_CODE_SESSION_ID`.
///   - `unregister` clears the file.
///   - `status` prints what's registered and an estimated $/fork.
///     trace:STORY-360 | ai:claude
pub(crate) fn handle_advisor_command(cmd: &AdvisorCommand) -> Result<()> {
    match cmd {
        AdvisorCommand::Register { uuid, project_slug } => {
            handle_advisor_register(uuid.as_deref(), project_slug.as_deref())
        }
        AdvisorCommand::Unregister => {
            advisor::clear_registration()?;
            println!("Advisor registration cleared.");
            Ok(())
        }
        // STORY-586: presence-gated fork-from-live watch loop. Store-less —
        // self-loads advisor config + presence. trace:STORY-586 | ai:claude
        AdvisorCommand::Watch {
            dry_run,
            once,
            triage_only,
            poll_interval,
            fork_interval,
        } => {
            let project_root = main_worktree_root_from(&find_project_root()?);
            advisor_watch::run_advisor_watch(
                &project_root,
                &advisor_watch::WatchOpts {
                    poll_interval_secs: *poll_interval,
                    fork_interval_secs: *fork_interval,
                    dry_run: *dry_run,
                    once: *once,
                    triage_only: *triage_only,
                },
            )
        }
        // STORY-559: only the narrow `--registration` view reaches this
        // store-less early handler; the default dashboard dispatches after
        // storage init. trace:STORY-559 | ai:claude
        AdvisorCommand::Status { json, .. } => handle_advisor_registration_status(*json),
        // STORY-262: `schedule` needs the requirement store, so it's routed
        // through the post-storage-init dispatch instead of this store-less
        // early handler. trace:STORY-262 | ai:claude
        AdvisorCommand::Schedule(_) => {
            unreachable!("advisor schedule dispatches after storage init")
        }
        // STORY-363: `handoff` writes a checked-in brief into a sibling
        // project. It reads no requirement store — parent identity is derived
        // from the current project root and the rest is an operator-authored
        // template — so it dispatches in this store-less early handler.
        // trace:STORY-363 | ai:claude
        AdvisorCommand::Handoff { to, focus, force } => handle_advisor_handoff(to, focus, *force),
    }
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

    if !gitignore_path.exists() {
        std::fs::write(
            &gitignore_path,
            format!(
                "{}{}{}{}{}{}{}{}",
                store_entry,
                runtime_entry,
                project_manifest_entry,
                discipline_pack_entry,
                claude_local_entry,
                rules_sync_entry,
                plans_draft_entry,
                settings_local_entry
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
    match command {
        Command::Add { .. }
        | Command::Edit { .. }
        | Command::Del { .. }
        | Command::Rel(_)
        | Command::RelDef(_)
        | Command::Comment(_)
        | Command::Type(_)
        | Command::Import { .. }
        | Command::Doc(_)
        | Command::Docs(_)
        | Command::Punt { .. }
        | Command::Archive { .. }
        | Command::Unarchive { .. }
        | Command::Defer { .. }
        | Command::Undefer { .. }
        | Command::Assign { .. }
        | Command::Unassign { .. }
        | Command::Rework { .. } => true,
        Command::Queue(cmd) => matches!(
            cmd,
            QueueCommand::Add { .. }
                | QueueCommand::Remove { .. }
                | QueueCommand::Move { .. }
                | QueueCommand::Clear { .. }
                | QueueCommand::Done { .. }
                | QueueCommand::Rework { .. }
                | QueueCommand::Prune { .. }
                | QueueCommand::Gc { .. }
        ),
        // STORY-444: `aida backlog groom` writes (queue + tag); list /
        // analyze are read-only. trace:STORY-444 | ai:claude
        Command::Backlog(cmd) => matches!(cmd, BacklogCommand::Groom { .. }),
        // FR-267: trace add/remove always mutate the store; `trace scan`
        // only writes with `--update`. scan/list/gate/sweep without a write
        // are read-only. trace:FR-267 | ai:claude
        Command::Trace(cmd) => matches!(
            cmd,
            TraceCommand::Add { .. }
                | TraceCommand::Remove { .. }
                | TraceCommand::Scan { update: true, .. }
        ),
        // STORY-732: bare `aida findings` (None) defaults to list — read-only.
        Command::Findings { cmd } => cmd.as_ref().is_some_and(|c| {
            !matches!(
                c,
                FindingsCommand::List { .. } | FindingsCommand::Classes { .. }
            )
        }),
        // STORY-522: `aida questions ask` / `answer` write the
        // decision_request field; bare list / `list` are read-only.
        // trace:STORY-522 | ai:claude
        Command::Questions { cmd } => matches!(
            cmd,
            Some(QuestionsCommand::Ask { .. })
                | Some(QuestionsCommand::Answer { .. })
                | Some(QuestionsCommand::Remedy { .. })
        ),
        // TASK-779: `aida decide` may route to the answer path, which writes
        // the decision_request + status + queue — treat it as a store writer.
        // trace:TASK-779 | ai:claude
        Command::Decide { .. } => true,
        Command::Config(cmd) => matches!(
            cmd,
            ConfigCommand::Format { .. }
                | ConfigCommand::Numbering { .. }
                | ConfigCommand::Digits { .. }
                | ConfigCommand::Migrate { .. }
        ),
        // TASK-1487: `jira/github pull` bulk-import new requirements into the
        // local store (via `bulk_import_via_writer`) unless `--dry-run`; every
        // other tracker subcommand (config/test/list/show/push/sync/labels/…)
        // only reads the store or talks to the remote tracker. GitLab has no
        // local-write subcommand for a git-canonical store (`refresh`'s sync
        // state is SQLite-only). trace:TASK-1487 | ai:claude
        Command::Jira(JiraCommand::Pull { dry_run, .. }) => !dry_run,
        Command::Github(GitHubCommand::Pull { dry_run, .. }) => !dry_run,
        _ => false,
    }
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
