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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreAutoPushMode {
    Manual,
    SessionEnd,
    PerWrite,
    Periodic,
}

impl StoreAutoPushMode {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "manual" => Some(Self::Manual),
            "session-end" | "session_end" | "sessionend" => Some(Self::SessionEnd),
            "per-write" | "per_write" | "perwrite" => Some(Self::PerWrite),
            "periodic" => Some(Self::Periodic),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::SessionEnd => "session-end",
            Self::PerWrite => "per-write",
            Self::Periodic => "periodic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoreSyncConfig {
    pub(crate) auto_push: StoreAutoPushMode,
    pub(crate) periodic_threshold: Option<u64>,
    pub(crate) periodic_interval: Option<String>,
    // Extra remotes to mirror the store push to after `origin` succeeds — the
    // drift-prevention fan-out. Best-effort: a non-ff / unreachable mirror leg
    // warns, never fails the sync. trace:TASK-1096 | ai:claude
    pub(crate) mirror_remotes: Vec<String>,
    pub(crate) source: String,
}

impl Default for StoreSyncConfig {
    fn default() -> Self {
        Self {
            auto_push: StoreAutoPushMode::Manual,
            periodic_threshold: None,
            periodic_interval: None,
            mirror_remotes: Vec::new(),
            source: "default".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoreAllocationConfig {
    pub(crate) retry_max: usize,
}

impl Default for StoreAllocationConfig {
    fn default() -> Self {
        Self { retry_max: 3 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpecIdCollision {
    pub(crate) spec_id: String,
    pub(crate) claimants: Vec<SpecIdClaimant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpecIdClaimant {
    pub(crate) uuid: Uuid,
    pub(crate) title: String,
}

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
    install_sigpipe_handler();
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
            return pr_list_handler(*json);
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

/// Detect if the current directory has a distributed store configured.
/// Walks up from CWD looking for `.aida/config.toml` with a store_path.
///
/// `AIDA_STORE` overrides everything: when it points at an existing
/// git-canonical store directory (one containing `objects/`), the whole CLI
/// (and the MCP server, which shares this resolver) retargets there instead of
/// walking up from CWD. This is the throwaway dev-playground / sandbox path
/// (SPIKE-48) — run drains and test scenarios against a discardable store
/// without touching the project's real `aida-store` orphan branch. The pointed
/// directory must already exist and look like a store; a missing or malformed
/// `AIDA_STORE` is ignored so a stale export never silently writes to a
/// half-formed location. trace:SPIKE-48 | ai:claude
/// trace:BUG-57 | ai:claude
pub(crate) fn detect_distributed_store() -> Option<std::path::PathBuf> {
    if let Some(store) = aida_store_override() {
        return Some(store);
    }
    let cwd = std::env::current_dir().ok()?;
    detect_distributed_store_from(&cwd)
}

/// Run `command` against a resolved git-canonical store directory. The MCP
/// server and the Jira / GitHub / GitLab tracker commands take a `Storage`;
/// pointing it at the store directory makes its load/save delegate to
/// GitBackend, so their reads and writes land in `objects/` like every other
/// command's. Everything else goes through the git-backend dispatcher.
///
/// `project_root_hint`, when given, is used as `mcp-serve`'s project root
/// instead of walking cwd up to a `.git` — the explicit `--file <dir>` caller
/// passes its own directory, since that directory IS the project root there
/// (no separate project to find, and cwd may be unrelated to it entirely).
/// The other callers pass `None` and keep the existing cwd-walk behavior.
// trace:BUG-310 trace:TASK-1486 trace:TASK-1487 | ai:claude
pub(crate) fn run_on_distributed_store(
    command: &Command,
    store_path: &std::path::Path,
    project_root_hint: Option<&std::path::Path>,
) -> Result<()> {
    let storage = Storage::new(store_path);
    match command {
        // MCP server reads/writes through the same canonical git store the CLI
        // uses. The previous YAML-snapshot approach wrote to
        // `.aida/mcp-cache.yaml` only — invisible to the CLI and overwritten on
        // every MCP restart. trace:BUG-310 | ai:claude
        Command::McpServe => {
            let project_root =
                mcp_serve_project_root(store_path, project_root_hint, find_project_root().ok());
            mcp::run_mcp_server(&storage, project_root)
        }
        // Tracker imports (`aida jira/github pull`) write new requirements
        // through this `Storage`, same as every other distributed-store
        // write path — they need the same node id and per-write auto-push
        // `handle_git_backend_command` gives its own callers, just reached
        // through a different signature (`&Storage`, not `&GitBackend`).
        // trace:TASK-1487 | ai:claude
        Command::Jira(cmd) => run_tracker_command(store_path, command, |storage| {
            tracker_cmd::handle_jira_command(cmd, storage)
        }),
        Command::Github(cmd) => run_tracker_command(store_path, command, |storage| {
            tracker_cmd::handle_github_command(cmd, storage)
        }),
        Command::Gitlab(cmd) => run_tracker_command(store_path, command, |storage| {
            tracker_cmd::handle_gitlab_command(cmd, storage)
        }),
        _ => git_backend_cmd::handle_git_backend_command(store_path, command),
    }
}

/// Pure decision for `mcp-serve`'s project root: an explicit hint (the
/// `--file <dir>` caller's own directory) always wins over a cwd-derived git
/// root, since cwd may be unrelated to — or simply not inside — the
/// directory the user explicitly pointed at. Falls back to the store path
/// itself only when neither resolves. Takes the cwd-walk result as a plain
/// value (rather than calling `find_project_root()` itself) so this decision
/// stays unit-testable without touching the process's actual cwd.
///
/// The hint is a STORE path (the `--file`/`AIDA_STORE` convention: it points
/// at `<repo>/.aida-store`, the directory `GitBackend::new` expects
/// `objects/` directly under — see the ~20 `project_root.join(".aida-store")`
/// call sites), not the project root itself. Adopting it as-is regressed
/// leases, roles, mailbox and ledger paths (all keyed off the real project
/// root) and double-nested the store. `project_root_from_store_hint` derives
/// the real root from it instead.
// trace:TASK-1487 | ai:claude
pub(crate) fn mcp_serve_project_root(
    store_path: &std::path::Path,
    project_root_hint: Option<&std::path::Path>,
    cwd_project_root: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    project_root_hint
        .map(project_root_from_store_hint)
        .or(cwd_project_root)
        .unwrap_or_else(|| store_path.to_path_buf())
}

/// Derive the project root a `--file <dir>`/`AIDA_STORE` hint implies, per
/// the store-path convention:
/// 1. The hint's final component is `.aida-store` → its parent is the root
///    (the overwhelmingly common case: `--file` pointed straight at the
///    worktree, the same as every other resolver in this codebase).
/// 2. Otherwise the hint already looks like a project root itself (it has
///    its own `.git` or `.aida/config.toml`) → use it as-is.
/// 3. Otherwise walk up from the hint looking for an enclosing project root,
///    guarded — like every other `.aida`-seeking walk-up in this codebase —
///    against ever adopting a shared system temp root (BUG-1598).
/// 4. Otherwise (a bare sandbox store — e.g. the `AIDA_STORE` dev-playground
///    path, SPIKE-48 — with no enclosing project) the hint has no project
///    root to point to; keep it as-is.
// trace:TASK-1487 | ai:claude
pub(crate) fn project_root_from_store_hint(hint: &std::path::Path) -> std::path::PathBuf {
    project_root_from_store_hint_with_roots(hint, &aida_core::store_locate::real_temp_roots())
}

/// [`project_root_from_store_hint`], parameterized on the temp roots to
/// guard against, so a test can exercise the guard against a fake root
/// without touching the real, shared system temp dir.
// trace:TASK-1487 | ai:claude
pub(crate) fn project_root_from_store_hint_with_roots(
    hint: &std::path::Path,
    temp_roots: &[std::path::PathBuf],
) -> std::path::PathBuf {
    if hint.file_name() == Some(std::ffi::OsStr::new(".aida-store")) {
        if let Some(parent) = hint.parent() {
            return parent.to_path_buf();
        }
    }
    if looks_like_project_root(hint) {
        return hint.to_path_buf();
    }
    let canonical_roots = aida_core::store_locate::canonicalize_roots(temp_roots);
    let mut current = hint.parent();
    while let Some(dir) = current {
        // BUG-1598: never walk INTO a temp root and adopt it (or whatever's
        // in it) as the project — same guard every other walk-up here uses.
        if aida_core::store_locate::is_in_canonical_roots(dir, &canonical_roots) {
            break;
        }
        if looks_like_project_root(dir) {
            return dir.to_path_buf();
        }
        current = dir.parent();
    }
    hint.to_path_buf()
}

/// Does `dir` look like an AIDA/git project root on its own — a `.git`
/// directory or an `.aida/config.toml` file? Shared by both the direct-hint
/// check and the walk-up in [`project_root_from_store_hint_with_roots`].
// trace:TASK-1487 | ai:claude
pub(crate) fn looks_like_project_root(dir: &std::path::Path) -> bool {
    dir.join(".git").exists() || dir.join(".aida").join("config.toml").exists()
}

/// Run a tracker (jira/github/gitlab) command against the distributed store
/// with the same two things `handle_git_backend_command` gives every other
/// distributed-store command — reused here, not reimplemented, since tracker
/// commands take a `&Storage` and can't go through that dispatcher's match:
/// - the dispenser's node id, so oplog entries this command writes (a tracker
///   pull's bulk import) aren't stamped node_id "0";
/// - the per-write store auto-push, gated the same way
///   `command_triggers_per_write_auto_push` gates every other write command,
///   and only fired on success (an error return skips it, same as the
///   git-backend dispatcher's post-match auto-push never running on an early
///   `?` return from inside its match).
// trace:TASK-1487 | ai:claude
pub(crate) fn run_tracker_command(
    store_path: &std::path::Path,
    command: &Command,
    handler: impl FnOnce(&Storage) -> Result<()>,
) -> Result<()> {
    let storage = Storage::new(store_path).with_dispenser(load_dispenser(store_path)?);
    handler(&storage)?;
    if command_triggers_per_write_auto_push(command) {
        maybe_auto_push_store(store_path, StoreAutoPushMode::PerWrite, "per-write");
    }
    Ok(())
}

/// Where `aida init --refresh` seeds missing type protocols.
// trace:TASK-1486 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RefreshSeedTarget {
    /// A resolved store: the distributed store, or a legacy store in a project
    /// that is not distributed.
    Store(std::path::PathBuf),
    /// The project is distributed but its store isn't attached here. Seeding
    /// a legacy `requirements.db` would write into stale data.
    DistributedUnattached,
    /// No store resolves at all.
    NoStore,
}

/// Pure decision for [`RefreshSeedTarget`]. The legacy resolver runs only when
/// the project is not distributed, so a stale local `requirements.db` in a
/// distributed project is never chosen.
// trace:TASK-1486 | ai:claude
pub(crate) fn refresh_seed_target(
    distributed_store: Option<std::path::PathBuf>,
    distributed_unattached: bool,
    legacy: impl FnOnce() -> Result<std::path::PathBuf>,
) -> RefreshSeedTarget {
    if let Some(store) = distributed_store {
        return RefreshSeedTarget::Store(store);
    }
    if distributed_unattached {
        return RefreshSeedTarget::DistributedUnattached;
    }
    legacy()
        .map(RefreshSeedTarget::Store)
        .unwrap_or(RefreshSeedTarget::NoStore)
}

/// The project root when `start` is inside a distributed AIDA project: one
/// whose `.aida/config.toml` declares distributed mode, whose git repo has an
/// `aida-store` branch, or whose store is physically attached at
/// `<root>/.aida-store` despite neither of those (the BUG-433 shape: a
/// session worktree forked from a commit that predates the committed
/// scaffolding, or a symlink into another repo's store). The same signals the
/// main resolver uses (see the `distributed_root` computation above) to
/// refuse the legacy fallback when the store isn't resolvable through
/// `detect_distributed_store`.
// trace:TASK-1486 trace:BUG-428 trace:BUG-442 trace:BUG-433 trace:TASK-1487 | ai:claude
pub(crate) fn unattached_distributed_root(start: &std::path::Path) -> Option<std::path::PathBuf> {
    distributed_mode_declared_from(start).or_else(|| {
        let root = start.ancestors().find(|d| d.join(".git").exists())?;
        if branch_exists_anywhere(root, "aida-store") {
            return Some(root.to_path_buf());
        }
        attached_store_present(root).then(|| root.to_path_buf())
    })
}

/// BUG-433: is a git-canonical store physically attached at `<root>/.aida-store`?
/// True when `.aida-store/objects/` is a directory — the hallmark of an attached
/// orphan-store worktree, resolved through a symlink too. Used to detect
/// distributed mode from the store's SHAPE when neither `.aida/config.toml` nor
/// an `aida-store` branch is present, so plain `aida` uses the real store
/// instead of silently serving legacy data. Delegates to `aida-core` so this
/// stays in lockstep with every other caller of the SAME resolver (e.g.
/// `aida-tui`'s mail scope).
// trace:BUG-433 trace:TASK-1141 | ai:claude
pub(crate) fn attached_store_present(project_root: &std::path::Path) -> bool {
    aida_core::store_locate::is_store_attached(project_root)
}

/// Classification of an `AIDA_STORE` value: either it resolves to a usable
/// store, or it's set-but-unusable with a specific reason. Lets the
/// resolution core stay pure/unit-testable while the env wrapper decides
/// whether to print the BUG-567 fall-through notice. Re-exported from
/// `aida-core::store_locate`, the canonical resolver both this CLI and
/// `aida-tui` route through.
// trace:BUG-567 trace:TASK-1141 | ai:claude
use aida_core::store_locate::StoreOverride;

/// Resolve the `AIDA_STORE` env override into a usable store path, or `None`
/// when unset / pointing at something that isn't a git-canonical store. A valid
/// store directory is one that exists and contains an `objects/` subdirectory
/// (the per-spec YAML tree GitBackend manages). Validation is deliberately
/// strict — a typo'd or not-yet-created path falls THROUGH to normal
/// resolution rather than erroring, so the override is opt-in and never the
/// thing that breaks a forgotten-export shell (SPIKE-48). Use `aida sandbox
/// create` to produce a directory this accepts.
///
/// BUG-567 Finding 1: the fall-through is no longer SILENT. When `AIDA_STORE`
/// is set but unusable we emit exactly ONE stderr notice naming the path, the
/// reason, and that we fell back to normal resolution — then still fall
/// through (no error, no behavior change; informational only). Suppress with
/// `AIDA_QUIET` so scripts can mute it. The never-break-a-forgotten-export
/// intent (SPIKE-48) is preserved — we inform, we don't error.
/// trace:SPIKE-48 trace:BUG-567 | ai:claude
pub(crate) fn aida_store_override() -> Option<std::path::PathBuf> {
    let raw = std::env::var("AIDA_STORE").ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    // trace:BUG-567 | ai:claude
    match aida_store_override_from(std::path::Path::new(raw)) {
        StoreOverride::Usable(p) => Some(p),
        StoreOverride::Unusable { reason } => {
            if !aida_quiet() {
                eprintln!(
                    "{} AIDA_STORE points at `{raw}` but it's unusable ({reason}); \
                     falling back to normal store resolution.\n  {} set AIDA_QUIET=1 \
                     to silence this, or point AIDA_STORE at a directory holding an \
                     `objects/` subdir (see `aida sandbox create`).",
                    crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
                    "→".cyan(),
                );
            }
            None
        }
    }
}

/// BUG-568: detect a "shared-store-but-multi-repo" context — a workspace where
/// a single `.aida-store` is shared by two or more code repos (via a
/// `.aida-workspace` manifest, as written by `aida init --sibling` /
/// `init_workspace`).
///
/// The completion/linkage scanners (`auto_bump_done_to_completed`,
/// `handle_db_reconcile_status`, `collect_git_linkage*`, `scan_trace_graph`)
/// each scan only the *local* repo's history/source tree. When the store spans
/// multiple repos, a spec whose referencing commit / trace comment lives in a
/// SIBLING repo is silently missed. Full multi-repo scanning is deferred to
/// SPIKE-62 (it needs the repo-identity model); until then we at least make the
/// limitation VISIBLE rather than silent.
///
/// Returns the other repo names (those whose `path` is not the current repo)
/// when a multi-repo workspace is detected, or `None` otherwise. The common
/// single-repo case (no manifest, or a manifest with <2 repos) returns `None`
/// → callers warn nothing → ZERO behavior change. The detection is cheap: walk
/// up from cwd for `.aida-workspace`, parse it, count repos.
// trace:BUG-568 | ai:claude
pub(crate) fn detect_multi_repo_shared_store(from: &std::path::Path) -> Option<Vec<String>> {
    let (workspace_root, manifest) = aida_core::workspace::WorkspaceManifest::discover(from)?;
    if manifest.repos.len() < 2 {
        return None;
    }
    // Identify which repo (if any) we're currently inside, so we can name the
    // OTHERS in the warning. Canonicalize both sides so a symlinked/relative
    // cwd still matches.
    let here = from.canonicalize().ok();
    let others: Vec<String> = manifest
        .repos
        .iter()
        .filter(|r| {
            let repo_abs = workspace_root.join(&r.path);
            match (&here, repo_abs.canonicalize().ok()) {
                (Some(h), Some(repo)) => !h.starts_with(&repo),
                _ => true,
            }
        })
        .map(|r| {
            if r.name.is_empty() {
                r.path.clone()
            } else {
                r.name.clone()
            }
        })
        .collect();
    Some(others)
}

/// Resolve the workspace repo slug for the repo containing `from` — the
/// canonical `origin.repo` join key (ADR-12) shared by specs and every
/// linkage artifact. `None` when no `.aida-workspace` manifest is
/// discoverable, or when `from` sits outside every manifest repo: the
/// single-repo case, where linkage stays unqualified (legacy bare-SHA
/// semantics, zero behavior change).
// trace:STORY-634 | ai:claude
pub(crate) fn workspace_repo_slug(from: &std::path::Path) -> Option<String> {
    let (workspace_root, manifest) = aida_core::workspace::WorkspaceManifest::discover(from)?;
    manifest.repo_slug_containing(&workspace_root, from)
}

/// BUG-568: emit ONE clear stderr warning that a completion/linkage scan
/// covered only the local repo while the store is shared across multiple repos,
/// so cross-repo completions/linkage may be missed. `scan_label` names the scan
/// (e.g. "auto-bump", "linkage scan") so the user can tell which surface was
/// limited. Suppressible via `AIDA_QUIET` (uniform with the BUG-567 store
/// fall-through notice). No-ops in the single-repo case (the detector returns
/// `None`). trace:BUG-568 | ai:claude
pub(crate) fn warn_multi_repo_scan_limited(from: &std::path::Path, scan_label: &str) {
    if aida_quiet() {
        return;
    }
    // Warn at most once per process — several scan sites can fire in one
    // command (e.g. `collect_git_linkage_opts` delegates to `scan_trace_graph`,
    // and a single `aida pull` runs auto-bump then renders linkage), and one
    // clear notice beats a wall of repeats. trace:BUG-568 | ai:claude
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    let Some(others) = detect_multi_repo_shared_store(from) else {
        return;
    };
    if WARNED.swap(true, Ordering::Relaxed) {
        return;
    }
    let other_repos = if others.is_empty() {
        String::new()
    } else {
        format!(" (other repos sharing this store: {})", others.join(", "))
    };
    eprintln!(
        "{} {scan_label} covered only the local repo, but this store is shared across \
         multiple repos{other_repos}. Cross-repo completions/linkage may be missed \
         (full multi-repo scanning is not yet implemented).\n  {} run this from each \
         repo, or set AIDA_QUIET=1 to silence.",
        crate::glyph(crate::glyphs::Glyph::Warning).yellow().bold(),
        "→".cyan(),
    );
}

/// Is output suppression requested? BUG-567: honor `AIDA_QUIET` (any non-empty,
/// non-"0"/"false" value) so the informational store fall-through notice can be
/// muted by scripts. trace:BUG-567 | ai:claude
pub(crate) fn aida_quiet() -> bool {
    match std::env::var("AIDA_QUIET") {
        Ok(v) => {
            let v = v.trim();
            !(v.is_empty() || v == "0" || v.eq_ignore_ascii_case("false"))
        }
        Err(_) => false,
    }
}

/// Path-resolution core of [`aida_store_override`], split out so it can be
/// unit-tested without mutating process env. Returns [`StoreOverride::Usable`]
/// (canonicalized) when `path` exists and holds an `objects/` directory, else
/// [`StoreOverride::Unusable`] carrying the specific reason it was rejected.
/// Delegates to `aida-core::store_locate::classify_store_path` — the
/// canonical resolver — so this CLI and any other caller (`aida-tui`'s mail
/// scope) never drift apart.
// trace:SPIKE-48 trace:BUG-567 trace:TASK-1141 | ai:claude
pub(crate) fn aida_store_override_from(path: &std::path::Path) -> StoreOverride {
    aida_core::store_locate::classify_store_path(path)
}

/// Walk up from `start` and return the project root whose `.aida/config.toml`
/// declares `mode = "distributed"`. Used to distinguish two states that look
/// identical to [`detect_distributed_store`] (both return `None` for the
/// store path): a *legacy* single-file project (no distributed config → the
/// YAML/SQLite fallback is correct) vs. a *distributed* project whose
/// `.aida-store/` worktree just isn't set up in this working copy — the
/// hallmark of a fresh clone, since the worktree is gitignored and only
/// created by `aida init`. In the latter case the caller must refuse the
/// legacy fallback (it would show stale data) and point at `aida init`.
/// trace:BUG-428 | ai:claude
pub(crate) fn distributed_mode_declared_from(
    start: &std::path::Path,
) -> Option<std::path::PathBuf> {
    distributed_mode_declared_from_with_roots(start, &aida_core::store_locate::real_temp_roots())
}

/// [`distributed_mode_declared_from`], parameterized on the temp roots to
/// guard against. Must agree with `detect_distributed_store_from` on where
/// the walk-up stops — otherwise a stray `.aida/config.toml` sitting
/// directly in a temp root (see `aida_core::store_locate` for why that
/// happens) makes THIS function claim distributed mode where the store
/// resolver finds nothing, producing a misleading "run aida init" refusal
/// instead of the correct legacy fallback. Factored out (rather than calling
/// `is_system_temp_dir` inline) so a test can exercise the guard against a
/// fake root without mutating `TMPDIR` or touching the real, shared system
/// temp dir.
// trace:BUG-1598 | ai:claude
pub(crate) fn distributed_mode_declared_from_with_roots(
    start: &std::path::Path,
    temp_roots: &[std::path::PathBuf],
) -> Option<std::path::PathBuf> {
    // Canonicalize the root set ONCE, before the loop — not on every
    // ancestor level.
    let canonical_roots = aida_core::store_locate::canonicalize_roots(temp_roots);
    let mut current = start;
    loop {
        if aida_core::store_locate::is_in_canonical_roots(current, &canonical_roots) {
            return None;
        }
        let config_path = current.join(".aida").join("config.toml");
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            // The first `.aida/config.toml` we hit walking up decides the
            // mode — a config without distributed mode means a legacy
            // project, so stop (don't keep walking to a parent project).
            return config_declares_distributed(&content).then(|| current.to_path_buf());
        }
        current = current.parent()?;
    }
}

pub(crate) fn distributed_mode_declared() -> Option<std::path::PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    distributed_mode_declared_from(&cwd)
}

/// TASK-621: attach the `.aida-store` worktree from the existing `aida-store`
/// branch so read commands "just work" on a fresh clone without a manual
/// `aida init`. Reuses the exact fetch + worktree-add sequence the post-clone
/// init bootstrap uses (handle_init_post_clone) — but NOT its node-id /
/// scaffolding steps. Those stay with `aida init` / `aida node acquire`: a
/// node id is the namespace for newly-issued spec ids, needed only before
/// WRITING. Reading the store needs only the worktree. Idempotent at the
/// git-ops layer (create_store_worktree no-ops when the worktree already
/// exists), but in practice only called when the worktree is absent.
/// trace:TASK-621 | ai:claude
pub(crate) fn try_attach_store_worktree(
    project_root: &std::path::Path,
) -> Result<std::path::PathBuf> {
    use aida_core::git_ops;
    let worktree_dir = ".aida-store";
    let branch = "aida-store";
    // BUG-559: on a fresh clone of an AIDA-on-GitLab repo the working tree can
    // land ON the orphan `aida-store` branch — GitLab's push-to-create adopts
    // the first-pushed branch as the project default, so a clone checks out the
    // internal YAML store as if it were the code. In that state
    // `git fetch origin aida-store:aida-store` refuses ("refusing to fetch into
    // branch refs/heads/aida-store checked out at <clone>") because git won't
    // write into a branch ref that's currently checked out.
    //
    // Fix (b) — harden auto-attach: instead of erroring, RECOVER. Switch the
    // working tree OFF `aida-store` first — to a code branch (`main`/`master`,
    // local or origin) when one exists, else to a detached HEAD — which frees
    // the `aida-store` ref so the fetch + worktree-add can proceed exactly as
    // on a healthy clone. This only fires in the already-failing GitLab state
    // (`aida-store` is the checked-out branch); a normal clone is untouched, so
    // it cannot regress the working path. The source-side prevention (making
    // the GitLab default branch `main` at init) is tracked separately under
    // BUG-559's follow-up.
    // trace:TASK-821 trace:BUG-559 | ai:claude
    if git_ops::current_branch(project_root).ok().as_deref() == Some(branch) {
        let candidates = local_code_branch_candidates(project_root);
        match choose_store_attach_recovery(project_root, &candidates) {
            StoreAttachRecovery::CheckoutCodeBranch(code_branch) => {
                git_ops::checkout_branch(project_root, &code_branch)?;
            }
            StoreAttachRecovery::DetachHead => {
                git_ops::detach_head(project_root)?;
            }
        }
    }
    // Create the local tracking branch from origin/<branch> (network fetch;
    // fails closed → caller drops to the explicit `aida init` guidance).
    git_ops::fetch_branch_into_local(project_root, "origin", branch)?;
    git_ops::create_store_worktree(project_root, worktree_dir, branch)
}

/// The recovery action chosen when `aida-store` is the checked-out branch on a
/// fresh clone (the BUG-559 GitLab state). Pure value so the decision is
/// unit-testable without running git. trace:BUG-559 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StoreAttachRecovery {
    /// Switch the working tree to this code branch before fetching `aida-store`.
    CheckoutCodeBranch(String),
    /// No code branch is available — detach HEAD to free the `aida-store` ref.
    DetachHead,
}

/// The conventional code-branch names to prefer when recovering, in priority
/// order. trace:BUG-559 | ai:claude
pub(crate) const CODE_BRANCH_PREFERENCE: [&str; 2] = ["main", "master"];

/// Collect the code-branch names available to switch to, in preference order:
/// a local `main`/`master` if it exists, else an `origin/main`/`origin/master`
/// (referenced by short name so `git checkout` sets up tracking). Used as input
/// to [`choose_store_attach_recovery`]; split out so the decision logic stays
/// pure and testable. trace:BUG-559 | ai:claude
pub(crate) fn local_code_branch_candidates(project_root: &std::path::Path) -> Vec<String> {
    use aida_core::git_ops;
    let mut out = Vec::new();
    for name in CODE_BRANCH_PREFERENCE {
        if git_ops::local_branch_exists(project_root, name) {
            out.push(name.to_string());
        } else if git_ops::remote_branch_exists(project_root, "origin", name) {
            // `git checkout main` against `origin/main` creates the local
            // tracking branch — git's DWIM behaviour.
            out.push(name.to_string());
        }
    }
    out
}

/// Decide how to get the working tree off the `aida-store` branch so a fetch
/// into the `aida-store` ref can succeed: check out the first available code
/// branch, or detach HEAD when none is available. Pure — given the candidate
/// list it returns the action, no git side effects. trace:BUG-559 | ai:claude
pub(crate) fn choose_store_attach_recovery(
    _project_root: &std::path::Path,
    code_branch_candidates: &[String],
) -> StoreAttachRecovery {
    match code_branch_candidates.first() {
        Some(branch) => StoreAttachRecovery::CheckoutCodeBranch(branch.clone()),
        None => StoreAttachRecovery::DetachHead,
    }
}

/// What `aida init` should push to a fresh/empty origin, and in what order, so a
/// forge that adopts the first-pushed branch as its default (GitLab's
/// push-to-create) ends up with the **code** branch as default — never the
/// orphan `aida-store` (which would make a fresh clone check out the internal
/// YAML store as if it were the project). Root-cause prevention for BUG-559.
/// trace:TASK-844 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InitPushPlan {
    /// A code branch exists and has commits → push it FIRST (so the forge
    /// adopts it as default), then the orphan store. The string is the code
    /// branch name to push.
    CodeBranchFirst(String),
    /// No code branch with commits to push (brand-new repo / nothing committed)
    /// → fall back to the legacy behavior: push only the orphan store. Pushing
    /// uncommitted/empty code would surprise the user (blast-radius guardrail),
    /// so we don't. The set-default-branch call can still run after the push.
    OrphanOnly,
}

/// Decide the init push plan from two facts about the working repo: whether a
/// code branch (`main`/`master`) **exists**, and whether it **has commits**.
/// Pure so the push-order decision is unit-testable without a live remote.
///
/// - code branch exists AND has commits → [`InitPushPlan::CodeBranchFirst`]
/// - otherwise → [`InitPushPlan::OrphanOnly`] (conservative fallback; never
///   pushes code the user hasn't committed).
///
/// trace:TASK-844 | ai:claude
pub(crate) fn decide_init_push_plan(
    code_branch: Option<&str>,
    code_branch_has_commits: bool,
) -> InitPushPlan {
    match code_branch {
        Some(name) if code_branch_has_commits => InitPushPlan::CodeBranchFirst(name.to_string()),
        _ => InitPushPlan::OrphanOnly,
    }
}

#[cfg(test)]
#[path = "tests/task_844_init_push_order_tests.rs"]
mod task_844_init_push_order_tests;

#[cfg(test)]
#[path = "tests/bug_559_clone_recovery_tests.rs"]
mod bug_559_clone_recovery_tests;

/// Walk-up resolver split out from `detect_distributed_store` so the search
/// path is testable without changing process cwd. Returns the absolute store
/// path on the first ancestor whose `.aida/config.toml` declares one.
///
/// Delegates to `aida-core::store_locate::detect_distributed_store_from` —
/// the canonical resolver (including its BUG-331 main-worktree fallback for
/// a linked/nested git worktree) — so this CLI and any other caller
/// (`aida-tui`'s mail scope) resolve to the exact same project root.
// trace:BUG-57 trace:BUG-331 trace:TASK-1141 | ai:claude
pub(crate) fn detect_distributed_store_from(start: &std::path::Path) -> Option<std::path::PathBuf> {
    aida_core::store_locate::detect_distributed_store_from(start)
}

/// Read the `[id_format] policy` from `.aida/config.toml`. Honors the legacy
/// `use_agreed_blocks` boolean as a fallback when the new section is missing
/// (so existing projects keep working unchanged).
///
/// Resolution order:
///   1. `[id_format] policy = "..."`  → parsed (errors on unknown values)
///   2. legacy `use_agreed_blocks = false`  → `node-aware-only`
///   3. legacy `use_agreed_blocks = true`   → `blocks-then-fallback`
///   4. neither set                          → `blocks-then-fallback` (default)
///      trace:EPIC-1-052 | ai:claude
///      Read the agreed-id counter for a given type from the orphan store's
///      `registry/agreed_counters.toml`. Returns 0 when the file doesn't exist
///      or the type has no entry — both mean "no ids issued yet for this type".
///      Used as the floor when claiming a new block so the block doesn't
///      overlap with already-issued agreed-ids.
///      trace:FR-1-073 | ai:claude
pub(crate) fn read_agreed_counter(store_path: &std::path::Path, type_prefix: &str) -> u32 {
    let path = store_path.join("registry").join("agreed_counters.toml");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return 0;
    };
    let prefix_upper = type_prefix.to_uppercase();
    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, '=');
        let (key, val) = match (parts.next(), parts.next()) {
            (Some(k), Some(v)) => (k.trim().trim_matches('"'), v.trim()),
            _ => continue,
        };
        if key.eq_ignore_ascii_case(&prefix_upper) {
            return val.parse::<u32>().unwrap_or(0);
        }
    }
    0
}

pub(crate) fn read_id_format_policy(project_dir: &std::path::Path) -> aida_core::IdFormatPolicy {
    read_id_format_settings(project_dir).0
}

/// Read counter_scope from `.aida/config.toml`. When absent, defaults to
/// PerType (back-compat — flipping a live store would conflate FR-100
/// and BUG-100 numerically). Projects created from 2026-05-09 onwards
/// have `counter_scope = "global"` written explicitly at init.
/// trace:FR-271 | ai:claude
#[allow(dead_code)]
pub(crate) fn read_id_counter_scope(project_dir: &std::path::Path) -> aida_core::IdCounterScope {
    read_id_format_settings(project_dir).1
}

/// Single pass over `.aida/config.toml` returning both the policy and
/// the counter scope. Cheaper than two separate reads when a caller
/// needs both. trace:FR-271 | ai:claude
pub(crate) fn read_id_format_settings(
    project_dir: &std::path::Path,
) -> (aida_core::IdFormatPolicy, aida_core::IdCounterScope) {
    let config_path = project_dir.join(".aida").join("config.toml");
    let default_policy = aida_core::IdFormatPolicy::default();
    let default_scope = aida_core::IdCounterScope::default();
    let Ok(content) = std::fs::read_to_string(&config_path) else {
        return (default_policy, default_scope);
    };

    let mut in_id_format = false;
    let mut policy_explicit: Option<aida_core::IdFormatPolicy> = None;
    let mut scope_explicit: Option<aida_core::IdCounterScope> = None;
    let mut legacy_use_blocks: Option<bool> = None;

    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            in_id_format = line == "[id_format]";
            continue;
        }
        if in_id_format && line.starts_with("policy") {
            if let Some(val) = line.split('=').nth(1) {
                let s = val.trim().trim_matches('"').trim_matches('\'');
                match aida_core::IdFormatPolicy::parse(s) {
                    Ok(p) => policy_explicit = Some(p),
                    Err(e) => eprintln!("Warning: {} — using default", e),
                }
            }
        }
        if in_id_format && line.starts_with("counter_scope") {
            if let Some(val) = line.split('=').nth(1) {
                let s = val.trim().trim_matches('"').trim_matches('\'');
                match aida_core::IdCounterScope::parse(s) {
                    Ok(c) => scope_explicit = Some(c),
                    Err(e) => eprintln!("Warning: {} — using default", e),
                }
            }
        }
        if !in_id_format && line.starts_with("use_agreed_blocks") {
            if let Some(val) = line.split('=').nth(1) {
                legacy_use_blocks = Some(val.trim() != "false");
            }
        }
    }

    let policy = policy_explicit.unwrap_or(match legacy_use_blocks {
        Some(false) => aida_core::IdFormatPolicy::NodeAwareOnly,
        Some(true) => aida_core::IdFormatPolicy::BlocksThenFallback,
        None => default_policy,
    });
    let scope = scope_explicit.unwrap_or(default_scope);
    (policy, scope)
}

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

pub fn validate_status_input(raw: &str) -> Result<&'static str, String> {
    let normalized: String = raw
        .chars()
        .filter_map(|c| match c {
            ' ' | '-' | '_' => None,
            c if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
            c => Some(c),
        })
        .collect();
    match normalized.as_str() {
        "draft" => Ok("Draft"),
        "approved" => Ok("Approved"),
        "planned" => Ok("Planned"),
        "inprogress" => Ok("InProgress"),
        // trace:STORY-86 | ai:claude — "done" is now its own state, not an alias for Completed.
        "done" => Ok("Done"),
        "completed" => Ok("Completed"),
        "rejected" => Ok("Rejected"),
        // trace:TASK-1176 | ai:claude — adopted-then-replaced (NOT declined).
        "superseded" => Ok("Superseded"),
        // trace:STORY-332 | ai:claude — the punt/pause state.
        "needsattention" => Ok("NeedsAttention"),
        _ => Err(format!(
            "invalid status `{}` — expected one of: {}",
            raw, VALID_STATUS_INPUTS
        )),
    }
}

/// Same shape, for priority. trace:BUG-47
pub fn validate_priority_input(raw: &str) -> Result<&'static str, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "high" => Ok("High"),
        "medium" | "med" => Ok("Medium"),
        "low" => Ok("Low"),
        _ => Err(format!(
            "invalid priority `{}` — expected one of: high, medium, low",
            raw
        )),
    }
}

/// Detect signs that a `--title` was mangled by shell command-substitution
/// (backticks the user forgot to escape, an unmatched quote that lost the
/// rest of the string). Returns Some(message) if suspicious. Caller should
/// print as a warning — never reject — since false positives are possible.
/// trace:BUG-22 | ai:claude
pub(crate) fn suspicious_title_signal(title: &str) -> Option<String> {
    if title.contains('`') {
        return Some(
            "title contains a backtick — if you meant a literal `, escape it (\\\\`) \
             or quote the whole title in single quotes; otherwise the shell may have \
             mangled it"
                .to_string(),
        );
    }
    // An odd count of unescaped double-quotes is a strong signal of broken quoting.
    let dq_count = title.chars().filter(|c| *c == '"').count();
    if dq_count % 2 == 1 {
        return Some(
            "title contains an unbalanced double-quote — likely a shell-quoting artifact"
                .to_string(),
        );
    }
    None
}

pub(crate) fn config_path_for_project(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root.join(".aida").join("config.toml")
}

pub(crate) fn config_parse_error_message(
    path: &std::path::Path,
    body: &str,
    err: &toml::de::Error,
) -> String {
    let loc = err
        .span()
        .map(|span| byte_offset_line_col(body, span.start))
        .map(|(line, col)| format!(":{line}:{col}"))
        .unwrap_or_default();
    format!(
        "{}{}: failed to parse AIDA config: {err}\n  Fix the TOML syntax, then re-run the command. If this came from `aida pull`, inspect any sibling `{}.conflicted` file and merge it manually.",
        path.display(),
        loc,
        path.display()
    )
}

pub(crate) fn byte_offset_line_col(body: &str, offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut col = 1usize;
    for (idx, ch) in body.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

pub(crate) fn config_conflict_marker_line(body: &str) -> Option<usize> {
    body.lines().enumerate().find_map(|(idx, line)| {
        let trimmed = line.trim_start();
        if trimmed.starts_with("<<<<<<< ")
            || trimmed == "<<<<<<<"
            || trimmed.starts_with("=======")
            || trimmed.starts_with(">>>>>>> ")
            || trimmed == ">>>>>>>"
        {
            Some(idx + 1)
        } else {
            None
        }
    })
}

#[derive(Debug)]
pub(crate) struct ConfigSnapshot {
    pub(crate) path: std::path::PathBuf,
    pub(crate) before: Option<String>,
}

pub(crate) fn known_project_config_paths(
    project_root: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    let aida = project_root.join(".aida");
    vec![aida.join("config.toml"), aida.join("agents.toml")]
}

pub(crate) fn snapshot_known_project_configs(
    project_root: &std::path::Path,
) -> Vec<ConfigSnapshot> {
    known_project_config_paths(project_root)
        .into_iter()
        .map(|path| ConfigSnapshot {
            before: std::fs::read_to_string(&path).ok(),
            path,
        })
        .collect()
}

pub(crate) fn validate_and_restore_project_configs_after_pull(
    snapshots: &[ConfigSnapshot],
) -> Result<()> {
    for snapshot in snapshots {
        let Ok(after) = std::fs::read_to_string(&snapshot.path) else {
            continue;
        };
        let failure = if let Some(line) = config_conflict_marker_line(&after) {
            Some(format!(
                "{}:{line}: conflict marker found in AIDA config after pull",
                snapshot.path.display()
            ))
        } else {
            match toml::from_str::<toml::Value>(&after) {
                Ok(_) => None,
                Err(err) => Some(config_parse_error_message(&snapshot.path, &after, &err)),
            }
        };
        let Some(message) = failure else {
            continue;
        };

        let conflicted = snapshot.path.with_extension("toml.conflicted");
        aida_core::write_atomic(&conflicted, after)
            .with_context(|| format!("failed to quarantine {}", conflicted.display()))?;
        match &snapshot.before {
            Some(before) => {
                // trace:BUG-1025 | ai:codex
                aida_core::write_atomic(&snapshot.path, before.clone())
                    .with_context(|| format!("failed to restore {}", snapshot.path.display()))?;
            }
            None => match std::fs::remove_file(&snapshot.path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("failed to remove {}", snapshot.path.display()));
                }
            },
        }
        anyhow::bail!(
            "{message}\n  Quarantined the conflicted version at {}.\n  Restored the pre-pull version at {}.\n  Manual merge step: compare both files, edit {}, then re-run `aida pull`.",
            conflicted.display(),
            snapshot.path.display(),
            snapshot.path.display()
        );
    }
    Ok(())
}

/// TASK-304: `[ultraplan] mode` governs whether AIDA proactively suggests
/// `aida ultraplan <SPEC>` for chunky specs. Planning is human/agent driven,
/// so the realistic surface is `never | on-demand | suggested` — never a
/// "frequently auto-pull" mode.
/// trace:TASK-304 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UltraplanMode {
    /// `aida ultraplan` refuses with a configured-off message; no clipboard.
    Never,
    /// Default — preserves current behavior: the user runs `aida ultraplan
    /// SPEC` explicitly, no pickup-time hints.
    OnDemand,
    /// Pickup surfaces (`/aida-pickup`, `aida queue work` no-arg head,
    /// `aida queue list` head row) hint when the head spec is chunky.
    Suggested,
}

impl UltraplanMode {
    pub(crate) fn from_token(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "never" => Some(Self::Never),
            "on-demand" | "on_demand" | "ondemand" => Some(Self::OnDemand),
            "suggested" => Some(Self::Suggested),
            _ => None,
        }
    }
}

/// TASK-697: which heuristic the `suggested` mode uses to decide a spec is
/// worth a planning prompt.
///
/// SPIKE-8 (`docs/spikes/2026-06-07-spike-8-ultraplan-comparison.md`) found
/// that `aida ultraplan`'s value is a *context-assembly* aid: it helps most on
/// thin / under-specified specs and least on well-formed ones (which already
/// carry their own `## Proposed shape` + Acceptance — the spec *is* the plan).
/// So acceptance-bullet count *anti-correlates* with where planning helps — a
/// 9-bullet spec with a design block needs planning less than a 2-bullet spec
/// with none. `Thinness` keys on that signal and is the default; the legacy
/// `acceptance-bullets>N` heuristic is still honored for backward compat.
/// trace:TASK-697 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SuggestThreshold {
    /// Legacy (TASK-304): suggest when the `## Acceptance` checkbox count is
    /// strictly greater than `N`.
    BulletCount(usize),
    /// TASK-697: suggest when the spec is *under-specified* — no design
    /// section yet a substantive body (see `is_spec_thin`).
    Thinness,
}

/// TASK-304/TASK-697: parsed `[ultraplan]` config. Both fields default safely
/// so a project with no `[ultraplan]` block (or an unparseable one) keeps the
/// opt-in behavior: `mode = on-demand`, threshold `spec-thinness`.
/// trace:TASK-697 | ai:claude
pub(crate) struct UltraplanConfig {
    pub(crate) mode: UltraplanMode,
    /// Which heuristic `suggested` mode applies. Defaults to `Thinness`.
    pub(crate) threshold: SuggestThreshold,
}

impl Default for UltraplanConfig {
    fn default() -> Self {
        Self {
            mode: UltraplanMode::OnDemand,
            threshold: SuggestThreshold::Thinness,
        }
    }
}

/// TASK-304: read `[ultraplan]` from `.aida/config.toml`. Missing file,
/// missing block, unparseable TOML, or unknown token values all fall back to
/// the defaults — the suggestion layer is a soft feature, never load-bearing.
/// trace:TASK-304 | ai:claude
pub(crate) fn read_ultraplan_config(project_root: &std::path::Path) -> UltraplanConfig {
    let mut cfg = UltraplanConfig::default();
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return cfg;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return cfg;
    };
    let Some(table) = value.get("ultraplan") else {
        return cfg;
    };
    if let Some(mode) = table
        .get("mode")
        .and_then(|v| v.as_str())
        .and_then(UltraplanMode::from_token)
    {
        cfg.mode = mode;
    }
    if let Some(threshold) = table
        .get("suggest_threshold")
        .and_then(|v| v.as_str())
        .and_then(parse_suggest_threshold)
    {
        cfg.threshold = threshold;
    }
    cfg
}

/// TASK-304: parse a `suggest_threshold` token. Only `acceptance-bullets>N`
/// is honored today (the spec's chosen default heuristic — mechanical, no
/// NLP, falsifiable); the `N` is extracted. Unknown tokens return `None` so
/// the caller keeps the default threshold. trace:TASK-304 | ai:claude
pub(crate) fn parse_acceptance_bullet_threshold(token: &str) -> Option<usize> {
    token
        .trim()
        .to_ascii_lowercase()
        .strip_prefix("acceptance-bullets>")?
        .trim()
        .parse::<usize>()
        .ok()
}

/// TASK-697: parse a `suggest_threshold` token into a `SuggestThreshold`.
/// `acceptance-bullets>N` → `BulletCount(N)` (legacy, still honored);
/// `spec-thinness` / `thinness` / `under-specified` → `Thinness` (the SPIKE-8
/// recommendation, the new default). Unknown tokens return `None` so the
/// caller keeps the default threshold. trace:TASK-697 | ai:claude
pub(crate) fn parse_suggest_threshold(token: &str) -> Option<SuggestThreshold> {
    if let Some(n) = parse_acceptance_bullet_threshold(token) {
        return Some(SuggestThreshold::BulletCount(n));
    }
    match token.trim().to_ascii_lowercase().as_str() {
        "spec-thinness" | "thinness" | "under-specified" => Some(SuggestThreshold::Thinness),
        _ => None,
    }
}

/// TASK-697: heading prefixes that mark a spec as carrying its own design —
/// the signal that planning would add little (the spec already *is* a plan).
/// Matched case-insensitively against the heading text after the `#` markers.
/// trace:TASK-697 | ai:claude
pub(crate) const DESIGN_SECTION_MARKERS: &[&str] = &[
    "proposed shape",
    "proposed solution",
    "proposed design",
    "design",
    "approach",
    "implementation",
];

/// TASK-697: a spec with `< THIN_MIN_BODY_CHARS` of body is too trivial to
/// plan — planning overhead would exceed the work. Above it, a design-less
/// spec is "thin" in the sense that matters (under-specified, not too-small).
pub(crate) const THIN_MIN_BODY_CHARS: usize = 240;

/// TASK-697: does the spec carry a design / proposed-shape section? Scans
/// markdown headings (any level) for a `DESIGN_SECTION_MARKERS` prefix.
/// trace:TASK-697 | ai:claude
pub(crate) fn has_design_section(description: &str) -> bool {
    for line in description.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let heading = rest.trim_start_matches('#').trim().to_ascii_lowercase();
            if DESIGN_SECTION_MARKERS
                .iter()
                .any(|marker| heading.starts_with(marker))
            {
                return true;
            }
        }
    }
    false
}

/// TASK-697: is this spec "thin" — under-specified enough that a planning
/// prompt would plausibly add value? True when it has NO design section yet a
/// substantive body. Well-specified specs (which embed `## Proposed shape` and
/// the like) stay quiet — exactly the SPIKE-8 finding that bullet count is the
/// wrong signal. Trivial one-liners also stay quiet (too small to plan).
/// trace:TASK-697 | ai:claude
pub(crate) fn is_spec_thin(description: &str) -> bool {
    !has_design_section(description) && description.trim().chars().count() >= THIN_MIN_BODY_CHARS
}

/// TASK-304: count the markdown task-list bullets (`- [ ]` / `- [x]`) inside
/// a spec's `## Acceptance` section — a rough complexity proxy. Returns 0
/// when there's no Acceptance section. Checked and unchecked items both
/// count: a long checklist is chunky regardless of how much is already
/// ticked. Tolerates `*`/`+` bullet markers and `## Acceptance Criteria`
/// heading variants. trace:TASK-304 | ai:claude
pub(crate) fn count_acceptance_bullets(description: &str) -> usize {
    let mut in_section = false;
    let mut count = 0;
    for line in description.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('#') {
            // A markdown heading at any level — entering or leaving the
            // Acceptance section resets the counter scope.
            let heading = rest.trim_start_matches('#').trim().to_ascii_lowercase();
            in_section = heading.starts_with("acceptance");
            continue;
        }
        if in_section && is_task_bullet(trimmed) {
            count += 1;
        }
    }
    count
}

/// TASK-304: is this a markdown task-list item (`- [ ]`, `* [x]`, `+ [X]`)?
pub(crate) fn is_task_bullet(trimmed: &str) -> bool {
    let after_marker = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "));
    match after_marker {
        Some(rest) => {
            let r = rest.trim_start();
            r.starts_with("[ ]") || r.starts_with("[x]") || r.starts_with("[X]")
        }
        None => false,
    }
}

/// TASK-304: the pickup-time suggestion hint. Returns `Some(message)` only
/// when `[ultraplan] mode = "suggested"` AND the spec's acceptance checklist
/// is longer than the configured threshold; `None` otherwise (the common
/// case — on-demand/never, or a spec that isn't chunky). The message format
/// is fixed by the spec so downstream surfaces render it identically.
/// trace:TASK-304 | ai:claude
pub(crate) fn ultraplan_suggestion_hint(
    project_root: &std::path::Path,
    req: &aida_core::Requirement,
) -> Option<String> {
    let cfg = read_ultraplan_config(project_root);
    if cfg.mode != UltraplanMode::Suggested {
        return None;
    }
    let id = req.display_id();
    match cfg.threshold {
        // Legacy (TASK-304): fire on a long acceptance checklist.
        SuggestThreshold::BulletCount(n) => {
            let bullets = count_acceptance_bullets(&req.description);
            if bullets <= n {
                return None;
            }
            Some(format!(
                "{id} has {bullets} acceptance bullets — `aida ultraplan {id}` to \
                 assemble a planning prompt before implementing."
            ))
        }
        // TASK-697: fire on an under-specified (design-less) spec — where
        // planning actually adds value.
        SuggestThreshold::Thinness => {
            if !is_spec_thin(&req.description) {
                return None;
            }
            Some(format!(
                "{id} has no design section yet — `aida ultraplan {id}` to \
                 assemble a planning prompt before implementing."
            ))
        }
    }
}

/// TASK-304: render the ultraplan suggestion hint to stdout under the
/// shared pickup surfaces (queue next / work / list). No-op when the helper
/// returns `None`. trace:TASK-304 | ai:claude
pub(crate) fn print_ultraplan_suggestion_hint(
    project_root: &std::path::Path,
    req: &aida_core::Requirement,
) {
    if let Some(hint) = ultraplan_suggestion_hint(project_root, req) {
        println!();
        println!("  {} {}", "⤷".cyan(), hint);
    }
}

/// TASK-304: the `[ultraplan]` block `aida init` scaffolds into a new
/// project's `.aida/config.toml`. Ships `mode = "on-demand"` (preserves
/// current behavior) plus a comment explaining the never/on-demand/suggested
/// trade-off and why "frequently/auto-pull" isn't an option. Appended like
/// the `[forge]` section so all three init paths share one source of truth.
/// trace:TASK-304 | ai:claude
pub(crate) fn init_ultraplan_config_section() -> &'static str {
    "\n# trace:TASK-304 | ai:claude  (suggest_threshold: trace:TASK-697)\n\
     # Whether AIDA proactively suggests `aida ultraplan <SPEC>` for specs\n\
     # where planning would help. Planning is human/agent driven, so there\n\
     # is no \"auto-pull\" mode — only:\n\
     #   never      — `aida ultraplan` is disabled (refuses with a message)\n\
     #   on-demand  — current behavior: run `aida ultraplan SPEC` yourself\n\
     #   suggested  — pickup surfaces (/aida-pickup, queue work/list head)\n\
     #                hint when the head spec passes suggest_threshold\n\
     # suggest_threshold only applies in `suggested` mode. Heuristics:\n\
     #   spec-thinness       — (default) suggest when the spec is\n\
     #                         under-specified: no design section (## Proposed\n\
     #                         shape / Design / Approach / ...) yet a\n\
     #                         substantive body. Well-specified specs already\n\
     #                         carry their plan, so they stay quiet (SPIKE-8).\n\
     #   acceptance-bullets>N — (legacy) suggest when the `## Acceptance`\n\
     #                         checkbox count exceeds N. Note: bullet count\n\
     #                         anti-correlates with where planning helps.\n\
     [ultraplan]\n\
     mode = \"on-demand\"\n\
     suggest_threshold = \"spec-thinness\"\n"
}

// trace:TASK-760 | ai:claude
// The commented `[intake]` example block `aida init` scaffolds into a new
// project's `.aida/config.toml`. `aida intake` works with zero config (every
// knob has a safe default), so the block ships fully commented-out — it exists
// purely to make the policy knobs discoverable next to the other sections.
// Appended like the `[forge]` / `[ultraplan]` sections so all init paths
// share one source of truth. The example values mirror the defaults in
// `intake::IntakeConfig::default()` — keep them in lockstep.
pub(crate) fn init_intake_config_section() -> &'static str {
    "\n# Policy for `aida intake` — the headless advisor pass that reads open\n\
     # specs and proposes approve/reject/park/queue dispositions (propose-only\n\
     # by default; `--apply` executes). Every knob has a safe default, so this\n\
     # whole section is optional — uncomment a line only to change a default.\n\
     #\n\
     # disposition_bias — how aggressively the agent proposes approve:\n\
     #   approve-eligible  — (default) propose approve for every eligible\n\
     #                       spec; the propose-mode review is the filter\n\
     #   park-aligned      — approve only when a spec is BOTH eligible AND\n\
     #                       clearly aligned with project priorities; else\n\
     #                       park for a human\n\
     #   park-conservative — park whenever unsure\n\
     #\n\
     # do_not_approve_classes — requirement types the agent can NEVER propose\n\
     # approve for; they are fenced out of its candidate set entirely. The\n\
     # default is the strategic + knowledge-graph types. An empty list opens\n\
     # the gate (the propose-mode review still applies).\n\
     #\n\
     # on_apply — what `--apply` does after queuing approved specs:\n\
     #   queue — (default) stop at queuing; draining stays a separate,\n\
     #           explicit command\n\
     #   drain — chain straight into an implementer drain after queuing\n\
     #\n\
     # [intake]\n\
     # disposition_bias = \"approve-eligible\"\n\
     # do_not_approve_classes = [\"vision\", \"epic\", \"principle\", \"constraint\", \"decision\", \"term\"]\n\
     # on_apply = \"queue\"\n"
}

// trace:TASK-1021 trace:TASK-0429 | ai:claude
// The commented `[autopilot]` example block `aida init` scaffolds into a new
// project's `.aida/config.toml`. Same contract as the `[intake]` block above:
// every knob has a conservative default, so the whole section ships
// commented-out and exists purely to make the authority map discoverable next
// to the policy section it composes with.
//
// ONLY the nine action keys are documented, because only those parse today
// (`autopilot::ActionClass::parse` x `autopilot::Authority::parse`). The
// example values mirror `autopilot::AutopilotEnvelope::default()` — keep them
// in lockstep. The comment text is user-facing scaffolding, so it carries no
// SPEC-IDs and states plainly which surfaces honor the envelope today.
pub(crate) fn init_autopilot_config_section() -> &'static str {
    "\n# Authority envelope for advisor autopilot (`aida autopilot`). Autopilot is\n\
     # not a second disposition engine — it is a bounded-authority wrapper over\n\
     # the same grooming pass `[intake]` above configures. For each disposition\n\
     # the advisor proposes, the envelope decides one of: auto-execute, hold for\n\
     # a human, or escalate. Every knob has a conservative default, so this whole\n\
     # section is optional — uncomment a line only to change a default.\n\
     #\n\
     # One key per action class; each takes one of three values:\n\
     #   auto    — autopilot may perform the action on its own\n\
     #   propose — autopilot may only propose it; a human reviews before it runs\n\
     #   never   — autopilot may never perform it\n\
     #\n\
     # Defaults: the reversible, low-blast actions are `auto`; the two\n\
     # irreversible dispositions (approve, reject) are `propose`. An unknown key\n\
     # or an unknown value is ignored, leaving that action at its default.\n\
     #\n\
     # These knobs are a CEILING, not a blanket grant. Three bounds sit OUTSIDE\n\
     # the map and cannot be widened here:\n\
     #   * the candidate fence — which specs are touchable at all (`[intake]`);\n\
     #   * grounding — a call resting on judgment a freshly-started agent cannot\n\
     #     reconstruct from what is written down always escalates;\n\
     #   * the risk ceiling — high-risk and unknown-blast-radius work always\n\
     #     escalates.\n\
     # Context can only TIGHTEN the envelope, never widen it: an unattended run\n\
     # demotes every `propose` to `never` (nobody is there to be asked), and an\n\
     # active solo posture on keystone work demotes everything to `never`.\n\
     #\n\
     # What the envelope drives today: `aida autopilot inspect` — a read-only\n\
     # dry-run of what autopilot WOULD decide over the current groom candidates,\n\
     # recorded to a local audit log you can review (`aida autopilot audit`) and\n\
     # reverse (`aida autopilot challenge`) — and the draft approve-gate on\n\
     # `aida zen <spec>`. There is deliberately NO autopilot execution path on\n\
     # the grooming pass itself yet, so widening a key below changes those two\n\
     # surfaces and nothing else.\n\
     #\n\
     # [autopilot]\n\
     # approve = \"propose\"\n\
     # reject = \"propose\"\n\
     # dedupe = \"auto\"\n\
     # tag = \"auto\"\n\
     # queue = \"auto\"\n\
     # park = \"auto\"\n\
     # route = \"auto\"\n\
     # comment = \"auto\"\n\
     # ask = \"auto\"\n"
}

/// The `[store.sync] mirror_remotes` scaffold — commented. Automatic store
/// pushes go to `origin` only by default; explicit sync commands and an
/// opt-in `hub-mirror-sync` schedule can update configured mirror hubs.
// trace:TASK-1096 | ai:claude
// trace:BUG-1746 | ai:codex
pub(crate) fn init_store_mirror_config_section() -> &'static str {
    r#"
# Store-sync fan-out (STORY-760). By default automatic store pushes send
# the orphan store to `origin` only. Mirror hubs are also updated by
# `aida db sync --push`, `aida pull`, `aida remote mirror-sync`, or an
# opt-in `hub-mirror-sync` schedule job. Register that job hourly (strictly
# shorter than any drift guard interval) so transient failures get retries;
# no mirror-sync route job is intended, since `hub-drift-guard` escalates
# drift that persists. A non-fast-forward mirror-sync failure exits non-zero
# and must be reconciled deliberately; ordinary store writes remain best-effort.
# Check drift anytime with `aida remote status`.
#
# [store.sync]
# mirror_remotes = ["gitlab"]
"#
}

// trace:TASK-1522 | ai:antigravity
pub(crate) fn init_capture_config_section() -> &'static str {
    "\n# Effort-balance intent-capture target (TASK-1522): advisory floor on the\n\
     # share of newly completed specs with at least one traced test criterion.\n\
     # Reported by `aida criteria coverage` and `aida status`; not enforced.\n\
     #\n\
     # [capture]\n\
     # criterion_test_floor_pct = 50\n"
}

/// The `[worktree]` scaffold section. AIDA-created worktrees are expected to be
/// ready for builds, so recursive submodule initialization is default-on with a
/// visible opt-out.
// trace:BUG-899 | ai:codex
pub(crate) fn init_worktree_config_section() -> &'static str {
    "\n# Worktree creation. AIDA initializes recursive git submodules by default\n\
     # after creating or claiming a worktree, so repos that vendor dependencies\n\
     # through `.gitmodules` are build-ready before pickup succeeds. Set this\n\
     # false to skip the potentially-expensive init; AIDA will print the exact\n\
     # recovery command when `.gitmodules` is present.\n\
     [worktree]\n\
     init_submodules = true\n"
}

/// The `[worktree_pool]` scaffold section. Pooling is ON by default (TASK-985):
/// `aida session start` (and the agent-new / queue-work / orchestrator paths)
/// reuse a recycled warm worktree instead of `git worktree add`, keeping the
/// build cache warm across fan-out (~30× faster per-spec builds — see
/// docs/research/2026-06-29-warm-pool-build-delta.md).
// trace:TASK-985 | ai:claude
pub(crate) fn init_worktree_pool_config_section() -> &'static str {
    "\n# Worktree warm-pool (STORY-714). Pooling is ON by default: a session's\n\
     # worktree is acquired from a recycled pool and RESET (not deleted) on\n\
     # hand-back, so its compiled `target/` stays warm across fan-out. Escape\n\
     # hatches: set `enabled = false` here, pass `--no-pool` to a single\n\
     # `aida session start`, or pass `--remove` to `aida session end` to delete\n\
     # the worktree instead of returning it.\n\
     [worktree_pool]\n\
     enabled = true\n\
     # max_trees = 16   # cap on pooled worktrees (default 16)\n\
     # worktree_parent = \"../aida-worktrees\"   # nest every AIDA-created worktree\n\
     #                          # under ONE directory instead of scattering them as\n\
     #                          # siblings of the repo. An editor/agent folder-trust\n\
     #                          # grant inherits to children, so you trust this one\n\
     #                          # directory once and no future worktree prompts\n\
     #                          # again. Relative paths resolve against the repo\n\
     #                          # root; unset keeps the sibling layout (default)\n\
     # lease_ttl_secs = 21600   # a durable lease older than this (with no live\n\
     #                          # owner) is treated as EXPIRED and reclaimed by\n\
     #                          # the next acquire — guards against reservation\n\
     #                          # leaks when a session dies (default 6h)\n\
     #\n\
     # Pre-warm on create: set `prewarm_build = true` to kick off a backgrounded\n\
     # `cargo build` when a NEW pool tree is created, so its `target/` is warm\n\
     # before the first fanned agent builds in it (best-effort, non-blocking).\n\
     # Like every worktree-pool hook, this executes a build, so it is honored\n\
     # ONLY from your machine-global `~/.aida/config.toml` — never a checked-in\n\
     # repo config — and is ignored here.\n"
}

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

pub(crate) fn line_for_key(body: &str, section: &str, key: &str) -> Option<usize> {
    let mut in_section = false;
    for (idx, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = trimmed.trim_matches(&['[', ']'][..]) == section;
            continue;
        }
        if in_section && trimmed.starts_with(key) {
            return Some(idx + 1);
        }
    }
    None
}

pub(crate) fn read_store_sync_config(project_root: &std::path::Path) -> Result<StoreSyncConfig> {
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Ok(StoreSyncConfig::default());
    };
    let value: toml::Value = toml::from_str(&body)
        .map_err(|err| anyhow::anyhow!(config_parse_error_message(&path, &body, &err)))?;
    let Some(sync) = value.get("store").and_then(|s| s.get("sync")) else {
        return Ok(StoreSyncConfig {
            source: path.display().to_string(),
            ..StoreSyncConfig::default()
        });
    };
    let raw = sync
        .get("auto_push")
        .and_then(|v| v.as_str())
        .unwrap_or("manual");
    let Some(auto_push) = StoreAutoPushMode::parse(raw) else {
        let line = line_for_key(&body, "store.sync", "auto_push")
            .map(|n| format!(":{n}"))
            .unwrap_or_default();
        anyhow::bail!(
            "{}{}: invalid [store.sync] auto_push value `{}` (expected: manual, session-end, per-write, periodic)",
            path.display(),
            line,
            raw
        );
    };
    // TASK-1096: mirror_remotes = ["gitlab", ...] — extra hubs the store push
    // fans out to. Non-string entries are skipped; a bare string is tolerated as
    // a single-remote shorthand.
    let mirror_remotes = match sync.get("mirror_remotes") {
        Some(toml::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::to_string)
            .collect(),
        Some(toml::Value::String(s)) => vec![s.to_string()],
        _ => Vec::new(),
    };
    Ok(StoreSyncConfig {
        auto_push,
        periodic_threshold: sync
            .get("periodic_threshold")
            .and_then(|v| v.as_integer())
            .and_then(|n| u64::try_from(n).ok()),
        periodic_interval: sync
            .get("periodic_interval")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        mirror_remotes,
        source: path.display().to_string(),
    })
}

pub(crate) fn read_store_allocation_config(
    project_root: &std::path::Path,
) -> Result<StoreAllocationConfig> {
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Ok(StoreAllocationConfig::default());
    };
    let value: toml::Value = toml::from_str(&body)
        .map_err(|err| anyhow::anyhow!(config_parse_error_message(&path, &body, &err)))?;
    let Some(allocation) = value
        .get("store")
        .and_then(|s| s.get("allocation"))
        .and_then(|v| v.as_table())
    else {
        return Ok(StoreAllocationConfig::default());
    };
    let retry_max = allocation
        .get("retry_max")
        .and_then(|v| v.as_integer())
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0)
        .unwrap_or(StoreAllocationConfig::default().retry_max);
    Ok(StoreAllocationConfig { retry_max })
}

pub(crate) fn warn_if_periodic_auto_push(project_root: &std::path::Path) {
    if let Ok(cfg) = read_store_sync_config(project_root) {
        if cfg.auto_push == StoreAutoPushMode::Periodic {
            eprintln!(
                "{} [store.sync] auto_push = \"periodic\" requires aida-worker (EPIC-30); falling back to manual until shipped",
                "Warning:".yellow().bold()
            );
        }
    }
}

pub(crate) fn find_spec_id_collisions(store: &RequirementsStore) -> Vec<SpecIdCollision> {
    use std::collections::{BTreeMap, BTreeSet};

    let mut by_spec: BTreeMap<String, Vec<SpecIdClaimant>> = BTreeMap::new();
    for req in &store.requirements {
        let Some(spec_id) = req.spec_id.as_deref() else {
            continue;
        };
        by_spec
            .entry(spec_id.to_ascii_uppercase())
            .or_default()
            .push(SpecIdClaimant {
                uuid: req.id,
                title: req.title.clone(),
            });
    }

    by_spec
        .into_iter()
        .filter_map(|(spec_id, mut claimants)| {
            let unique_uuids: BTreeSet<Uuid> = claimants.iter().map(|c| c.uuid).collect();
            if unique_uuids.len() <= 1 {
                return None;
            }
            claimants.sort_by(|a, b| a.uuid.cmp(&b.uuid));
            Some(SpecIdCollision { spec_id, claimants })
        })
        .collect()
}

pub(crate) fn spec_id_collision_recovery_message(
    collisions: &[SpecIdCollision],
    store_path: &std::path::Path,
) -> String {
    let mut out = String::new();
    out.push_str("duplicate AIDA spec IDs detected after syncing the git-canonical store\n");
    out.push_str(
        "AIDA is refusing to continue before a divergent SPEC-ID silently drops content.\n\n",
    );
    for collision in collisions.iter().take(5) {
        out.push_str(&format!("  {} is claimed by:\n", collision.spec_id));
        for claimant in &collision.claimants {
            out.push_str(&format!("    - {} — {}\n", claimant.uuid, claimant.title));
        }
    }
    if collisions.len() > 5 {
        out.push_str(&format!(
            "  ... plus {} more duplicate id(s)\n",
            collisions.len() - 5
        ));
    }
    out.push_str("\nPaste-ready recovery:\n");
    out.push_str(&format!("  cd {}\n", store_path.display()));
    out.push_str("  git status\n");
    out.push_str("  # inspect the duplicate object(s), then preserve both contents manually\n");
    out.push_str("  # planned tooling: aida db check --collisions --show-conflict\n");
    out.push_str("  # planned tooling: aida db check --collisions --repair\n");
    out.push_str(
        "\nDo not use `git rebase --skip` unless you intentionally want to drop one side.\n",
    );
    out
}

/// Group the flat `(spec_id, uuid, title)` rows the cache returns into
/// `SpecIdCollision`s — one per spec_id claimed by ≥2 distinct uuids. Same shape
/// `find_spec_id_collisions` produces from a full store, so the recovery message
/// is identical whichever path detected the clash.
// trace:BUG-701 | ai:claude
pub(crate) fn group_spec_id_collisions(rows: Vec<(String, Uuid, String)>) -> Vec<SpecIdCollision> {
    use std::collections::BTreeMap;
    let mut by_spec: BTreeMap<String, Vec<SpecIdClaimant>> = BTreeMap::new();
    for (spec_id, uuid, title) in rows {
        by_spec
            .entry(spec_id)
            .or_default()
            .push(SpecIdClaimant { uuid, title });
    }
    by_spec
        .into_iter()
        .filter_map(|(spec_id, mut claimants)| {
            claimants.sort_by(|a, b| a.uuid.cmp(&b.uuid));
            claimants.dedup_by(|a, b| a.uuid == b.uuid);
            if claimants.len() <= 1 {
                return None;
            }
            Some(SpecIdCollision { spec_id, claimants })
        })
        .collect()
}

pub(crate) fn ensure_no_spec_id_collisions(store_path: &std::path::Path) -> Result<()> {
    // BUG-701: on the hot `aida add` path this ran a full O(n) `GitBackend::load()`
    // (re-parsing every spec YAML) purely to detect duplicate spec_ids — ~2s and
    // growing with the store. Use the cache's indexed spec_id group-by instead
    // (sub-ms, size-independent). `spec_id_collisions` freshens the cache first,
    // so a collision a just-completed pre-allocation pull introduced is still
    // caught. Fall back to the authoritative full-store scan whenever the cache
    // is unavailable/unreadable (legacy centralized mode, torn cache) so the
    // duplicate guard never silently weakens. trace:BUG-701 | ai:claude
    let cache_path = aida_core::CachedGitBackend::default_cache_path(store_path);
    if let Ok(backend) = aida_core::CachedGitBackend::open(store_path, &cache_path) {
        if let Ok(rows) = backend.spec_id_collisions() {
            let collisions = group_spec_id_collisions(rows);
            if collisions.is_empty() {
                return Ok(());
            }
            anyhow::bail!(
                "{}",
                spec_id_collision_recovery_message(&collisions, store_path)
            );
        }
    }

    // Fallback: authoritative full-store scan (cache absent or unreadable).
    let backend = aida_core::GitBackend::new(store_path)?;
    let store = backend.load()?;
    let collisions = find_spec_id_collisions(&store);
    if collisions.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "{}",
        spec_id_collision_recovery_message(&collisions, store_path)
    );
}

// TASK-857: opt-out for the pre-allocation remote sync. `aida add`'s remaining
// multi-second cost is the unconditional network round-trips guarding the
// cross-clone duplicate-id race. Setting AIDA_ADD_NO_REMOTE_SYNC truthy makes
// every store-syncing step on the add path purely local — no ls-remote probe,
// no pull --rebase, no push. The local collision check still runs, so the
// in-clone duplicate guard is preserved; the cross-clone guard degrades to
// "reconverge on the next online add / `aida db sync`". For fully-offline or
// solo-clone workflows where the network floor is pure latency.
// trace:TASK-857 | ai:claude
pub(crate) fn add_remote_sync_disabled() -> bool {
    std::env::var("AIDA_ADD_NO_REMOTE_SYNC")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

pub(crate) fn pull_store_before_id_allocation(
    store_path: &std::path::Path,
    project_root: &std::path::Path,
) -> Result<()> {
    use aida_core::git_ops;

    if !git_ops::is_git_repo(store_path) || !git_ops::has_remote(store_path, "origin") {
        ensure_no_spec_id_collisions(store_path)?;
        return Ok(());
    }

    // TASK-857: explicit purely-local mode — skip every network leg, keep the
    // local collision guard. trace:TASK-857 | ai:claude
    if add_remote_sync_disabled() {
        return ensure_no_spec_id_collisions(store_path);
    }

    if git_ops::has_changes(store_path).unwrap_or(false) {
        let _ = git_ops::add(store_path, &["."]);
        let _ = git_ops::commit(
            store_path,
            "chore: sync pending changes before id allocation",
        );
    }

    let cfg = read_store_allocation_config(project_root)?;
    let branch = git_ops::current_branch(store_path).unwrap_or_else(|_| "aida-store".to_string());

    // TASK-857 (option a): the network floor. Instead of an UNCONDITIONAL full
    // `pull --rebase` (fetch + rebase the whole orphan store) on every add,
    // first ask the remote for just the branch head SHA via a single
    // `ls-remote` (refs only, no object transfer). Three outcomes:
    //
    //   * remote head == local HEAD  → the local store already contains the
    //     remote's latest state. Allocating from it is provably safe, so SKIP
    //     the heavy pull entirely and just run the local collision check. This
    //     is the common steady-state case and the whole latency win.
    //   * remote head != local HEAD  → the remote moved; fall through to the
    //     existing pull --rebase retry loop to converge before allocating
    //     (duplicate-id guarantee preserved exactly as before).
    //   * ls-remote returns None     → remote unreachable (offline). Rather
    //     than hard-failing every add while offline, degrade gracefully: warn
    //     once and allocate from local state under the local collision guard.
    //     The push-after-allocation leg re-converges with the remote (and is
    //     itself offline-tolerant), so a sibling clone reconciles on its next
    //     online add. trace:TASK-857 | ai:claude
    match git_ops::remote_branch_head_sha(store_path, "origin", &branch) {
        Some(remote_sha) => {
            if let Ok(local_sha) = git_ops::head_sha(store_path) {
                if remote_sha == local_sha {
                    // Already current with origin — no objects to fetch.
                    return ensure_no_spec_id_collisions(store_path);
                }
            }
            // Remote diverged (or couldn't read local HEAD) → heavy pull below.
        }
        None => {
            // Offline / unreachable: file locally, reconverge later.
            eprintln!(
                "{} origin unreachable — filing offline; the new id syncs on the next online `aida add` or `aida db sync --push`.",
                crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
            );
            return ensure_no_spec_id_collisions(store_path);
        }
    }

    for attempt in 0..cfg.retry_max {
        match git_ops::pull_rebase(store_path, "origin", &branch) {
            Ok(()) => return ensure_no_spec_id_collisions(store_path),
            Err(e) if attempt + 1 < cfg.retry_max => {
                eprintln!(
                    "{} store allocation pull failed ({}) — retrying ({}/{})",
                    "Warning:".yellow().bold(),
                    e,
                    attempt + 1,
                    cfg.retry_max
                );
            }
            Err(e) => {
                anyhow::bail!(
                    "store allocation pull failed after {} attempt(s): {}\n\
                     To recover:\n  cd {} && git rebase --abort\n  aida db sync --pull",
                    cfg.retry_max,
                    e,
                    store_path.display()
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn push_store_after_id_allocation(
    store_path: &std::path::Path,
    project_root: &std::path::Path,
    spec_id: &str,
) -> Result<()> {
    use aida_core::git_ops;

    if !git_ops::is_git_repo(store_path) || !git_ops::has_remote(store_path, "origin") {
        return Ok(());
    }

    // TASK-857: explicit purely-local mode — the new id is committed locally and
    // will publish on the next online `aida add` / `aida db sync --push`.
    // trace:TASK-857 | ai:claude
    if add_remote_sync_disabled() {
        return Ok(());
    }

    let cfg = read_store_allocation_config(project_root)?;
    let branch = git_ops::current_branch(store_path).unwrap_or_else(|_| "aida-store".to_string());
    for attempt in 0..cfg.retry_max {
        ensure_no_spec_id_collisions(store_path)?;
        match git_ops::push(store_path, "origin", &branch) {
            Ok(true) => return Ok(()),
            Ok(false) if attempt + 1 < cfg.retry_max => {
                eprintln!(
                    "{} store push rejected after allocating {} — pulling/retrying ({}/{})",
                    "Warning:".yellow().bold(),
                    spec_id,
                    attempt + 1,
                    cfg.retry_max
                );
                git_ops::pull_rebase(store_path, "origin", &branch)?;
                ensure_no_spec_id_collisions(store_path)?;
            }
            Ok(false) => {
                anyhow::bail!(
                    "store push rejected after allocating {} and {} attempt(s) were exhausted.\n\
                     Your local store still has the new spec commit; do not re-file blindly.\n\
                     To recover:\n  aida db sync --pull\n  aida db sync --push",
                    spec_id,
                    cfg.retry_max
                );
            }
            // TASK-857: a push that ERRORS (vs is rejected) is the offline /
            // network-down case. A spec is already filed and committed locally;
            // the duplicate-id guarantee only requires the publish to land
            // *eventually* (a sibling clone reconciles when it next pulls). So
            // DEFER rather than fail the whole `aida add`: warn and return Ok.
            // The CAS-reject retry above is untouched — that's the real
            // serialize-the-winners guard and only fires when online.
            // trace:TASK-857 | ai:claude
            Err(_) => {
                eprintln!(
                    "{} {} filed locally but not yet published (origin unreachable). \
                     It syncs on the next online `aida add` or `aida db sync --push`.",
                    crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
                    spec_id
                );
                return Ok(());
            }
        }
    }
    Ok(())
}

/// STORY-441: read `[archive] auto_after_days` from `.aida/config.toml`.
/// Returns `Some(days)` when the user has opted in, `None` when the key is
/// absent (auto-sweep stays off). Clamps below 7 to 7 with a stderr warning
/// — auto-archiving a freshly-shipped spec defeats the whole point.
/// Missing config file or unparseable TOML returns `None` silently (the
/// optional auto-sweep is a soft feature, not load-bearing).
/// trace:STORY-441 | ai:claude
pub(crate) fn read_archive_auto_after_days(project_root: &std::path::Path) -> Option<u64> {
    let path = config_path_for_project(project_root);
    let body = std::fs::read_to_string(&path).ok()?;
    let value: toml::Value = toml::from_str(&body).ok()?;
    let raw = value
        .get("archive")
        .and_then(|t| t.get("auto_after_days"))
        .and_then(|v| v.as_integer())?;
    let days = u64::try_from(raw).ok()?;
    if days < 7 {
        eprintln!(
            "{} [archive] auto_after_days = {days} clamped to minimum of 7 days \
             (archiving sooner hides freshly-shipped specs)",
            "Warning:".yellow().bold()
        );
        Some(7)
    } else {
        Some(days)
    }
}

/// BUG-783: read `[list] show_hidden_hints` from `.aida/config.toml`. Governs
/// whether the default `aida list` view footers the archived / deferred
/// view-tier counts ("(138 archived hidden — pass --all or --archived to see
/// them)"). Default is `false` — on an explicit open-work request those lines
/// are pure noise on every single invocation, and the tiers stay one flag away
/// (`--archived` / `--deferred` / `--all`). Set the key to `true` to bring the
/// nudges back. Missing config file / key / unparseable TOML → the default.
// trace:BUG-783 | ai:claude
pub(crate) fn read_list_show_hidden_hints(project_root: &std::path::Path) -> bool {
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return false;
    };
    value
        .get("list")
        .and_then(|t| t.get("show_hidden_hints"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// STORY-1644: read a `[section] key` value from `.aida/config.toml` via the
/// shared [`crate::kernel::read_project_config_value`] loader. Missing config
/// file / key / unparseable TOML → `None`.
// trace:STORY-1644 | ai:claude
fn project_config_key(
    project_root: &std::path::Path,
    section: &str,
    key: &str,
) -> Option<toml::Value> {
    crate::kernel::read_project_config_value(project_root)?
        .get(section)?
        .get(key)
        .cloned()
}

/// STORY-1644: read `[list] default_sort` (the `aida list` order when no
/// `--sort` is passed). `None` when unset.
// trace:STORY-1644 | ai:claude
pub(crate) fn read_list_default_sort(project_root: &std::path::Path) -> Option<String> {
    project_config_key(project_root, "list", "default_sort")
        .and_then(|v| v.as_str().map(str::to_string))
}

/// STORY-1644: read `[list] agent_limit` (the agent-mode `aida list` row
/// cap; 0 = unbounded). `None` when unset or not an integer.
// trace:STORY-1644 | ai:claude
pub(crate) fn read_list_agent_limit(project_root: &std::path::Path) -> Option<i64> {
    project_config_key(project_root, "list", "agent_limit").and_then(|v| v.as_integer())
}

/// STORY-1644: read `[history] limit` (the `aida history` default row
/// count). `None` when unset or not a positive integer.
// trace:STORY-1644 | ai:claude
pub(crate) fn read_history_limit(project_root: &std::path::Path) -> Option<i64> {
    project_config_key(project_root, "history", "limit")
        .and_then(|v| v.as_integer())
        .filter(|n| *n > 0)
}

/// STORY-1644: resolve `aida history`'s row count: an explicit `--limit`
/// wins, then env `AIDA_HISTORY_LIMIT`, then `[history] limit`, then the
/// built-in default. Non-positive / unparseable env values are ignored.
// trace:STORY-1644 | ai:claude
pub(crate) fn resolve_history_limit(
    flag: Option<usize>,
    env: Option<&str>,
    config: Option<i64>,
    default: usize,
) -> usize {
    if let Some(n) = flag {
        return n;
    }
    if let Some(n) = env
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
    {
        return n;
    }
    match config {
        Some(n) if n > 0 => n as usize,
        _ => default,
    }
}

/// STORY-1644: `aida history`'s effective row count for this invocation —
/// [`resolve_history_limit`] over the flag, `AIDA_HISTORY_LIMIT`, and the
/// project's `[history] limit`.
// trace:STORY-1644 | ai:claude
pub(crate) fn effective_history_limit(flag: Option<usize>) -> usize {
    let env = std::env::var("AIDA_HISTORY_LIMIT").ok();
    let config = if flag.is_some() {
        None
    } else {
        find_project_root()
            .ok()
            .and_then(|root| read_history_limit(&root))
    };
    resolve_history_limit(
        flag,
        env.as_deref(),
        config,
        crate::history_timeline::HISTORY_DEFAULT_LIMIT,
    )
}

/// STORY-717: read `[focus] out_of_scope` from `.aida/config.toml`. Governs the
/// focus-scope drift guard at work-start (off/warn/block). Missing config /
/// key / unparseable TOML falls back to the default (`warn`). The optional
/// `none`/`silent` and `refuse`/`hard` aliases are accepted by the parser.
// trace:STORY-717 | ai:claude
pub(crate) fn read_focus_out_of_scope_policy(
    project_root: &std::path::Path,
) -> focus::OutOfScopePolicy {
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return focus::OutOfScopePolicy::default();
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return focus::OutOfScopePolicy::default();
    };
    value
        .get("focus")
        .and_then(|t| t.get("out_of_scope"))
        .and_then(|v| v.as_str())
        .map(focus::parse_out_of_scope_policy)
        .unwrap_or_default()
}

/// STORY-717: derive a cheap "did you mean" focus suggestion for an out-of-scope
/// target — its `parent:` tag when present (the AIDA convention is
/// `parent:EPIC-NN`). Returns `None` when no cheap hint exists, in which case
/// the nudge just names the mismatch (per the spec: "if cheap; else just name
/// the mismatch").
// trace:STORY-717 | ai:claude
pub(crate) fn suggested_focus_for(target: &aida_core::Requirement) -> Option<String> {
    target
        .tags
        .iter()
        .find_map(|t| t.strip_prefix("parent:"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// STORY-717: the focus-scope drift guard. At a work-START moment, if the
/// current worktree has a focus set (STORY-706) and `target` is OUTSIDE that
/// focus's transitive subtree, apply the configured `[focus] out_of_scope`
/// policy. `force` ALWAYS overrides. Membership reuses the cache's
/// `descendant_ids` closure (TASK-955) — the same subtree the focus read-scope
/// uses — rather than re-walking the hierarchy. An unresolvable focus spec
/// skips the guard rather than blocking real work. The subtree is read with a
/// strict cache refresh, and a cache or refresh error fails the start (fail
/// closed) instead of judging scope on a stale graph; a `Block` policy also
/// returns `Err` on a genuine out-of-scope start.
// trace:STORY-717 | ai:claude
// trace:BUG-1670 | ai:claude
pub(crate) fn focus_scope_guard(
    project_root: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
    target: &aida_core::Requirement,
    force: bool,
) -> Result<()> {
    // No focus set in this worktree → no check.
    let Some(focus_ref) = focus::resolve_focus(project_root) else {
        return Ok(());
    };
    let policy = read_focus_out_of_scope_policy(project_root);
    // Cheap exit before touching the cache when the guard is muted anyway.
    if policy == focus::OutOfScopePolicy::Off {
        return Ok(());
    }
    // The focus label must still resolve to a real spec; if it no longer does,
    // don't block work (the read-scope path warns about a dangling focus).
    let Some(focus_req) = backend.get_requirement_by_spec_id(&focus_ref)? else {
        return Ok(());
    };
    // REUSE the TASK-955 subtree closure (includes the root) for membership.
    // Strict: this gates a write, so a child added or re-parented by another
    // writer since the last cache refresh must be classified on the current
    // graph, not on a snapshot served while the cache is being written.
    // trace:BUG-1670 | ai:claude
    let subtree = backend.descendant_ids_strict(&focus_req.id)?;
    let in_scope = focus::is_in_focus_scope(&target.id, &subtree);
    match focus::decide_focus_action(policy, in_scope, force) {
        focus::FocusGuardAction::Proceed => Ok(()),
        focus::FocusGuardAction::Warn => {
            let core = focus::out_of_scope_message(
                &target.display_id(),
                &focus_req.display_id(),
                suggested_focus_for(target).as_deref(),
            );
            eprintln!(
                "{} {} (--force to silence)",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow(),
                core
            );
            Ok(())
        }
        focus::FocusGuardAction::Block => {
            let core = focus::out_of_scope_message(
                &target.display_id(),
                &focus_req.display_id(),
                suggested_focus_for(target).as_deref(),
            );
            anyhow::bail!("{} (pass --force to override)", core)
        }
    }
}

/// STORY-717: focus guard for callers that don't already hold a backend
/// (`queue work`, `agent new --spec`). Resolves `spec` against a cache-backed
/// backend, then defers to [`focus_scope_guard`]. Skips early (no backend work)
/// when no focus is set; best-effort on store/spec resolution. A `Block` policy
/// still propagates the `Err`.
// trace:STORY-717 | ai:claude
pub(crate) fn focus_scope_guard_for_spec(
    project_root: &std::path::Path,
    spec: &str,
    force: bool,
) -> Result<()> {
    // Fast path: no focus → skip the (potentially cache-rebuilding) backend open.
    if focus::resolve_focus(project_root).is_none() {
        return Ok(());
    }
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Ok(());
    };
    let Ok(backend) = advance_backend(&store_path) else {
        return Ok(());
    };
    let Some(target) = backend.get_requirement_by_spec_id(spec)? else {
        return Ok(());
    };
    focus_scope_guard(project_root, &backend, &target, force)
}

// BUG-653: `aida agent new --spec <epic>` used to dead-end. The launch path
// routes through `session_start` -> `preflight_spec_status`, which refuses a
// Draft spec with "transition it to Approved first". But an epic's status is a
// read-only rollup of its children (BUG-626) -- `aida edit <epic> --status
// approved` is rejected -- so that advice sends the operator to a command that
// will refuse. And you don't implement an epic directly anyway; work happens on
// its children. Detect the epic up front and give an epic-appropriate message
// instead of routing into the implement-readiness gate. Non-epic specs (incl.
// genuinely-Draft tasks/stories) fall through untouched, so their correct
// "transition to Approved" refusal is preserved. trace:BUG-653 | ai:claude
pub(crate) fn epic_agent_new_guard(project_root: &std::path::Path, spec: &str) -> Result<()> {
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Ok(());
    };
    let Ok(backend) = advance_backend(&store_path) else {
        return Ok(());
    };
    let Some(target) = backend.get_requirement_by_spec_id(spec)? else {
        return Ok(());
    };
    match epic_agent_new_refusal(&target.req_type, &target.display_id()) {
        Some(message) => anyhow::bail!("{message}"),
        None => Ok(()),
    }
}

/// BUG-653: pure decision half of [`epic_agent_new_guard`] — returns the
/// epic-appropriate refusal message when the resolved `--spec` is an epic, or
/// `None` for every other type (so non-epic specs fall through to the existing
/// readiness gate untouched). Pure so the message + the type gate are
/// unit-testable without a store fixture.
// trace:BUG-653 | ai:claude
pub(crate) fn epic_agent_new_refusal(
    req_type: &RequirementType,
    display_id: &str,
) -> Option<String> {
    if !matches!(req_type, RequirementType::Epic) {
        return None;
    }
    Some(format!(
        "{display_id}'s status is a read-only rollup of its children -- you don't implement an \
         epic directly.\n  \
         Pick an approved child and launch on that:\n    \
         aida list --parent {display_id} --status approved\n  \
         Or open a design/advisor session scoped to the epic in a focused worktree (no --spec):\n    \
         aida worktree enter {display_id}   # then a plain `aida agent new ...` in that worktree"
    ))
}

// BUG-1701: route a drain-mode spec away from the interactive lane. `aida agent
// new --spec <ID>` spawns a vendor TUI and BLOCKS until that TUI exits; a TUI
// does not exit when its turn ends, so an orchestrator that dispatched a
// drain-groomed spec here stranded both the seat and itself. The one-shot lane
// already exists -- `aida do <SPEC>` routes by groomed execution mode, and for
// drain that is `aida queue work <SPEC> --auto-complete`, which runs headless
// and returns an exit status. Nothing steered callers there, so this guard does.
// Sits beside the epic and focus-scope guards: AFTER the dry-preview returns (a
// preview is never blocked) and BEFORE any worktree/lease/status side effect.
// trace:BUG-1701 | ai:claude
pub(crate) fn drain_mode_agent_new_guard(
    project_root: &std::path::Path,
    spec: &str,
    force: bool,
    holds_caller: bool,
) -> Result<()> {
    if force {
        return Ok(());
    }
    let Some(store_path) = detect_distributed_store_from(project_root) else {
        return Ok(());
    };
    let Ok(backend) = advance_backend(&store_path) else {
        return Ok(());
    };
    // Fail OPEN on a lookup failure as well as a miss. `?` here would abort the
    // launch when the store is unreadable, which is the opposite of this guard's
    // contract: it exists to REDIRECT a drain-groomed spec, never to become a new
    // way for every launch in the fleet to fail. An independent review of PR #2249
    // caught the `?`. trace:BUG-1701 | ai:claude
    let Ok(Some(target)) = backend.get_requirement_by_spec_id(spec) else {
        return Ok(());
    };
    match drain_mode_agent_new_refusal(target.execution_mode, &target.display_id(), holds_caller) {
        Some(message) => anyhow::bail!("{message}"),
        None => Ok(()),
    }
}

/// Pure decision half of [`drain_mode_agent_new_guard`]: the refusal message for a
/// spec groomed `execution_mode = drain`, or `None` for every other mode (and for
/// an ungroomed spec, which must keep working -- the fail-open default).
///
/// Only `drain` is refused. The other modes legitimately want a human-attended
/// seat, which is exactly what this lane provides; refusing them would take away
/// the only lane they have.
///
/// `holds_caller` distinguishes the foreground launch (which blocks the caller on
/// the child TUI -- the BUG-1701 stall) from `--bg` (which detaches, so the lane
/// is still wrong but for the pipeline reason alone). Stating only what is true of
/// the lane actually being refused keeps the message trustworthy.
// trace:BUG-1701 | ai:claude
pub(crate) fn drain_mode_agent_new_refusal(
    mode: Option<aida_core::ExecutionMode>,
    display_id: &str,
    holds_caller: bool,
) -> Option<String> {
    if mode != Some(aida_core::ExecutionMode::Drain) {
        return None;
    }
    // The first line must stand alone: in agent mode only the first line becomes
    // the `error:` summary. And `aida do` must be the FIRST backtick-quoted
    // `aida ...` command in the whole message, because that is what agent mode
    // lifts into `help:` -- an unbackticked recommendation silently loses to a
    // backticked one further down. trace:BUG-1701 | ai:claude
    let lane = if holds_caller {
        "which spawns an interactive seat and holds your caller until that seat's TUI exits -- \
         and a TUI does not exit when its turn ends"
    } else {
        "which runs neither CI, the reviewer phase, nor the merge for you"
    };
    Some(format!(
        "{display_id} is groomed `execution_mode = drain` -- use `aida do {display_id}`, not this \
         lane, {lane}.\n  \
         `aida do` routes by the groomed mode; for drain it runs \
         `aida queue work {display_id} --auto-complete`, which is headless, runs to completion, \
         and returns an exit status.\n  \
         To sit in this spec interactively anyway (to debug it by hand), pass --force."
    ))
}

/// STORY-564: read `[zen] auto_exit` from `.aida/config.toml`. Returns the
/// operator's persistent preference for whether a clean standalone-`--zen`
/// finish auto-exits (`true`, the default) or always pauses (`false`).
/// Missing config / key / unparseable TOML → `true` (the new default
/// behavior). The per-invocation `--pause-always` flag (→ `AIDA_ZEN_PAUSE_ALWAYS`)
/// overrides this toward pausing; this is the standing preference when the
/// flag isn't passed. trace:STORY-564 | ai:claude
pub(crate) fn read_zen_auto_exit(project_root: &std::path::Path) -> bool {
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return true;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return true;
    };
    value
        .get("zen")
        .and_then(|t| t.get("auto_exit"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

/// STORY-564: should the standalone-`--zen` finish *pause* (vs auto-exit)
/// because the operator asked it to? True when `--pause-always` was passed
/// (propagated as `AIDA_ZEN_PAUSE_ALWAYS=1`) OR `[zen] auto_exit = false` is
/// configured. Either is the operator electing to drive grab-next by hand.
/// trace:STORY-564 | ai:claude
pub(crate) fn zen_pause_always_in_force(project_root: &std::path::Path) -> bool {
    std::env::var(zen::ZEN_PAUSE_ALWAYS_ENV).as_deref() == Ok("1")
        || !read_zen_auto_exit(project_root)
}

/// STORY-569: read `[zen] review_brief_agent` from `.aida/config.toml` — the
/// mailbox target for the clean-finish build→review handoff. Missing config /
/// key / unparseable TOML → the default `advisor`. An explicitly empty string
/// disables the handoff. trace:STORY-569 | ai:claude
pub(crate) fn read_zen_review_brief_agent(project_root: &std::path::Path) -> Option<String> {
    let default = || Some("advisor".to_string());
    let path = config_path_for_project(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return default();
    };
    let Ok(value) = toml::from_str::<toml::Value>(&body) else {
        return default();
    };
    match value
        .get("zen")
        .and_then(|t| t.get("review_brief_agent"))
        .and_then(|v| v.as_str())
    {
        None => default(),
        Some(s) if s.trim().is_empty() => None,
        Some(s) => Some(s.trim().to_string()),
    }
}

/// STORY-441: opt-out env var matching the `AIDA_AUTO_BUMP` shape.
pub(crate) fn auto_archive_enabled() -> bool {
    match std::env::var("AIDA_AUTO_ARCHIVE") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// STORY-441: run the same `--older-than N days --status completed,rejected`
/// sweep that `aida archive --older-than` exposes, but as a side-effect of
/// `aida pull` once the auto-bump has finished. Off by default — gated on
/// `[archive] auto_after_days` being set. Best-effort: any error is printed
/// as a warning, never fails the pull. trace:STORY-441 | ai:claude
pub(crate) fn maybe_auto_archive_sweep(
    project_root: &std::path::Path,
    backend: &aida_core::CachedGitBackend,
    quiet: bool,
) {
    if !auto_archive_enabled() {
        return;
    }
    let Some(days) = read_archive_auto_after_days(project_root) else {
        return;
    };
    let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
    let statuses = ["Completed", "Rejected"];
    let mut to_archive: Vec<aida_core::Requirement> = Vec::new();
    // cache-tolerant-read: selection only — each candidate's YAML object is
    // re-read and its eligibility re-decided inside the store write lock
    // (`bulk_update_atomically`) before anything is written.
    // trace:BUG-1671 | ai:claude
    for s in &statuses {
        let filter = aida_core::ListFilter {
            status: Some((*s).to_string()),
            archive: aida_core::ArchiveFilter::NonArchivedOnly,
            ..Default::default()
        };
        let Ok(rows) = backend.list_summaries(&filter) else {
            continue;
        };
        for row in rows {
            let stale = chrono::DateTime::parse_from_rfc3339(&row.modified_at)
                .map(|dt| dt.with_timezone(&chrono::Utc) < cutoff)
                .unwrap_or(false);
            if !stale {
                continue;
            }
            // BUG-1664: `row` may come from a stale cache snapshot. The
            // object read is authoritative, so re-check status and age on it,
            // not only the archive flag.
            // trace:BUG-1664 | ai:claude
            if let Ok(Some(req)) = backend.get_requirement(&row.id) {
                if archive_cmd::archive_sweep_still_eligible(&req, &statuses, cutoff) {
                    to_archive.push(req);
                }
            }
        }
    }
    if to_archive.is_empty() {
        return;
    }
    let now = chrono::Utc::now();
    crate::sweep_test_hook::fire(backend.path());
    // BUG-1671: re-decide each candidate on the object read INSIDE the store
    // write lock, so a spec reopened between the pass above and this write is
    // skipped instead of being reverted by the whole-object write. Collapses
    // the old commit-per-spec loop into one commit as well.
    // trace:BUG-1671 | ai:claude
    let count = backend
        .bulk_update_atomically(&to_archive, "chore(archive)", |req| {
            if !archive_cmd::archive_sweep_still_eligible(req, &statuses, cutoff) {
                return false;
            }
            req.archived = true;
            req.archived_at = Some(now);
            req.modified_at = now;
            true
        })
        .map(|report| report.written.len())
        .unwrap_or(0);
    if !quiet && count > 0 {
        println!(
            "  {} {count} spec(s) older than {days}d (auto-sweep, opt out via AIDA_AUTO_ARCHIVE=0)",
            "auto-archived:".cyan()
        );
    }
}

#[cfg(test)]
#[path = "tests/story_760_store_mirror_tests.rs"]
mod story_760_store_mirror_tests;

#[cfg(test)]
#[path = "tests/story_441_archive_config_tests.rs"]
mod story_441_archive_config_tests;

#[cfg(test)]
#[path = "tests/story_284_store_sync_tests.rs"]
mod story_284_store_sync_tests;

/// Ensure git/claude hook files are executable. Called from scaffolder
/// write paths so freshly-scaffolded hooks don't trigger git's "hook was
/// ignored because not executable" warning.
/// trace:BUG-21 | ai:claude
pub(crate) fn ensure_executable_if_hook(rel_path: &std::path::Path, full_path: &std::path::Path) {
    let s = rel_path.to_string_lossy();
    let is_hook =
        s.starts_with(".git/hooks/") || s.starts_with(".claude/hooks/") || s.ends_with(".sh");
    if !is_hook {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(full_path) {
            let mut p = meta.permissions();
            p.set_mode(0o755);
            let _ = std::fs::set_permissions(full_path, p);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = full_path; // no-op on non-unix
    }
}

/// Resolve the description from one of three sources: inline `--description`,
/// `--description-from-file PATH`, or `--description-stdin`. The CLI struct
/// already enforces mutual exclusion via `conflicts_with_all`; here we just
/// fetch the content from the right source. Returns `Ok(None)` when no
/// source is set (caller falls back to empty / interactive prompt).
/// trace:BUG-17 | ai:claude
pub(crate) fn resolve_description(
    description: &Option<String>,
    description_from_file: &Option<std::path::PathBuf>,
    description_stdin: bool,
) -> Result<Option<String>> {
    if let Some(d) = description {
        return Ok(Some(d.clone()));
    }
    if let Some(path) = description_from_file {
        let body = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read description from {}", path.display()))?;
        return Ok(Some(body));
    }
    if description_stdin {
        use std::io::Read;
        let mut body = String::new();
        std::io::stdin()
            .read_to_string(&mut body)
            .context("failed to read description from stdin")?;
        return Ok(Some(body));
    }
    Ok(None)
}

// trace:BUG-1234 | ai:codex
/// Resolve an edit description and refuse an empty file/stdin body so a
/// failed producer cannot erase an existing requirement description.
/// Inline `--description ""` remains the explicit way to clear the field.
pub(crate) fn resolve_edit_description(
    description: &Option<String>,
    description_from_file: &Option<std::path::PathBuf>,
    description_stdin: bool,
) -> Result<Option<String>> {
    let resolved = resolve_description(description, description_from_file, description_stdin)?;
    if (description_from_file.is_some() || description_stdin)
        && resolved.as_deref().is_some_and(str::is_empty)
    {
        anyhow::bail!("description input is empty; existing description left unchanged");
    }
    Ok(resolved)
}

pub(crate) fn parse_requirement_type(s: &str) -> Result<RequirementType> {
    match s.to_lowercase().as_str() {
        "functional" | "fr" => Ok(RequirementType::Functional),
        "non-functional" | "nonfunctional" | "nfr" => Ok(RequirementType::NonFunctional),
        "system" | "sr" => Ok(RequirementType::System),
        "user" | "ur" => Ok(RequirementType::User),
        // Workflow type: a proposed change. trace:TASK-716 | ai:claude
        "change-request" | "changerequest" | "change" | "cr" => Ok(RequirementType::ChangeRequest),
        "bug" => Ok(RequirementType::Bug),
        "epic" => Ok(RequirementType::Epic),
        "story" => Ok(RequirementType::Story),
        "task" => Ok(RequirementType::Task),
        "spike" => Ok(RequirementType::Spike),
        "sprint" => Ok(RequirementType::Sprint),
        "folder" => Ok(RequirementType::Folder),
        "meta" => Ok(RequirementType::Meta),
        // Docs-layer types (FR-1-074). Aliases match the type prefix used
        // in agreed-id format (`PRIN`, `VIS`, `CON`, `ADR`, `TERM`).
        // trace:FR-1-074 | ai:claude
        "principle" | "prin" => Ok(RequirementType::Principle),
        "vision" | "vis" => Ok(RequirementType::Vision),
        "constraint" | "con" => Ok(RequirementType::Constraint),
        "decision" | "adr" => Ok(RequirementType::Decision),
        "term" | "glossary" => Ok(RequirementType::Term),
        // trace:STORY-104 | ai:claude
        "doc" | "documentation" => Ok(RequirementType::Doc),
        "faq" => Ok(RequirementType::Faq),
        _ => anyhow::bail!("Unknown requirement type: {}", s),
    }
}

#[cfg(test)]
#[path = "tests/parse_requirement_type_tests.rs"]
mod parse_requirement_type_tests;

/// Initialize distributed mode using an orphan branch + worktree.
/// This is the default for single-repo projects.
/// Store lives at .aida-store/ (worktree of orphan branch 'aida-store').
/// BUG-446 / TASK-686: immediate-child directories of `cwd` that are their OWN
/// project — a nested git repo (a `.git` directory, or a `.git` file =
/// gitlink/submodule worktree) OR an AIDA project (a child `.aida/` dir) — and
/// are NOT registered submodules. A non-empty result means `cwd` is a
/// workspace-/parent-of-projects, not a single project.
///
/// We deliberately test for a child's own `.git`/`.aida` entry rather than
/// calling `git_ops::is_git_repo` (which shells out to `git rev-parse`): from
/// inside `cwd` — itself a git repo by the time this runs — `rev-parse`
/// resolves to `cwd`'s git-dir for EVERY subdirectory, so it would flag plain
/// subdirs too. Dotted children (`.git`, `.aida-store`, …) and
/// `.gitmodules`-declared submodules are intentional and excluded.
///
/// TASK-686 added the child-`.aida/` arm: a parent of AIDA projects that aren't
/// all plain git repos (the `~/ai/` case — ~80 children) would otherwise slip
/// the guard and leave a scaffold every child inherits via ancestor CLAUDE.md.
/// trace:BUG-446 trace:TASK-686 | ai:claude
pub(crate) fn unmanaged_nested_projects(cwd: &std::path::Path) -> Vec<String> {
    let submodule_paths = gitmodule_child_paths(cwd);
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(cwd) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || submodule_paths.contains(&name) {
            continue;
        }
        if path.join(".git").exists() || path.join(".aida").is_dir() {
            found.push(name);
        }
    }
    found.sort();
    found
}

/// First path component of each `path = …` entry in the top-level `.gitmodules`
/// — enough to exclude an immediate-child submodule directory from the
/// workspace-of-projects guard. trace:BUG-446 | ai:claude
pub(crate) fn gitmodule_child_paths(cwd: &std::path::Path) -> std::collections::HashSet<String> {
    let mut paths = std::collections::HashSet::new();
    let Ok(content) = std::fs::read_to_string(cwd.join(".gitmodules")) else {
        return paths;
    };
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("path") {
            if let Some(value) = rest.split('=').nth(1) {
                if let Some(first) = value.trim().split('/').next() {
                    if !first.is_empty() {
                        paths.insert(first.to_string());
                    }
                }
            }
        }
    }
    paths
}

/// What to do when `aida init` runs in a directory that isn't a git repo yet.
/// trace:STORY-552 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GitInitDecision {
    /// Run `git init` without asking (explicit `--git-init`).
    Yes,
    /// Offer interactively (TTY, no flag).
    Prompt,
    /// Keep the safe bail+recipe (non-interactive, no flag — don't
    /// silently git-init in scripts).
    Bail,
}

/// Decide how to handle a non-git folder at the front of init. `--git-init`
/// always wins; otherwise prompt at a TTY and bail elsewhere. Pure so it can be
/// unit-tested without a terminal. trace:STORY-552 | ai:claude
pub(crate) fn git_init_decision(git_init_flag: bool, at_tty: bool) -> GitInitDecision {
    if git_init_flag {
        GitInitDecision::Yes
    } else if at_tty {
        GitInitDecision::Prompt
    } else {
        GitInitDecision::Bail
    }
}

/// Get a git config value from the global config.
pub(crate) fn _git_config_get_global(key: &str) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(["config", "--global", key])
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        anyhow::bail!("git config {} not set", key)
    }
}

pub(crate) fn add_requirement_interactive(storage: &Storage) -> Result<()> {
    // Load existing requirements
    let mut store = storage.load()?;

    // Prompt user for requirement details
    let requirement = crate::prompts::prompt_new_requirement(&mut store)?;
    let id = requirement.id;

    // Get prefixes for ID generation
    let feature_prefix = store
        .get_feature_by_name(&requirement.feature)
        .map(|f| f.prefix.clone());
    let type_prefix = store.get_type_prefix(&requirement.req_type);

    // Add the requirement with auto-assigned ID based on configuration
    store.add_requirement_with_id(
        requirement,
        feature_prefix.as_deref(),
        type_prefix.as_deref(),
    );
    storage.save(&store)?;

    // Get the added requirement to show its ID
    let added_req = store
        .get_requirement_by_id(&id)
        .expect("Just added requirement");

    println!("{}", "Requirement added successfully!".green());
    println!("UUID: {}", id);
    if let Some(spec_id) = &added_req.spec_id {
        println!("ID: {}", spec_id.green());
    }

    Ok(())
}

// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_requirement_cli(
    storage: &Storage,
    title: &Option<String>,
    description: &Option<String>,
    status_str: &Option<String>,
    priority_str: &Option<String>,
    type_str: &Option<String>,
    owner: &Option<String>,
    feature: &Option<String>,
    tags_str: &Option<String>,
    prefix: &Option<String>,
    parent: &Option<String>,
    force_parent: bool,
) -> Result<()> {
    // Load existing requirements
    let mut store = storage.load()?;

    // Check required fields
    let title = match title {
        Some(t) => t.clone(),
        None => anyhow::bail!("Title is required. Use --title to specify a title."),
    };

    let description = match description {
        Some(d) => d.clone(),
        None => String::new(),
    };

    // Validate parent exists if specified
    let parent_uuid = if let Some(parent_id) = parent {
        let uuid = parse_requirement_id(parent_id, &store)?;
        // BUG-64: terminal-status guard. Refuse to file a new child
        // under a Completed/Rejected parent unless --force-parent.
        // trace:BUG-64 | ai:claude
        if !force_parent {
            let pr = store
                .get_requirement_by_id(&uuid)
                .ok_or_else(|| anyhow::anyhow!("parent {} not found", parent_id))?;
            if is_terminal_status(&pr.status) {
                anyhow::bail!(
                    "parent {} is {} — adding new children to a closed parent is usually a mistake. \
                     Pass `--force-parent` to override.",
                    pr.spec_id.as_deref().unwrap_or(parent_id),
                    pr.status,
                );
            }
        }
        Some(uuid)
    } else {
        None
    };

    // Create a requirement with basic data
    let mut requirement = Requirement::new(title, description);

    // Set optional fields
    if let Some(status) = status_str {
        requirement.status = parse_status(status)?;
    }

    // TASK-647 (ADR-3): advisor-gate the production of approved+ specs. A
    // non-advisor, non-TTY caller (headless agent, drain/auto capture) can
    // only file `draft`; a requested approved+ status is downgraded with a
    // one-line triage notice. Advisor role or an interactive human is
    // unaffected. trace:TASK-647 | ai:claude
    let intake_downgraded_from =
        if status_requires_advisor_authority(&requirement.status) && !has_advisor_authority() {
            let from = requirement.status;
            requirement.status = RequirementStatus::Draft;
            Some(from)
        } else {
            // BUG-498: an approved+ status that was NOT downgraded means the
            // caller exercised advisor authority — if that authority came from
            // an `AIDA_SESSION_ROLE=advisor` prefix rather than a seated role,
            // nudge them to seat it. trace:BUG-498 | ai:claude
            if status_requires_advisor_authority(&requirement.status) {
                maybe_hint_advisor_seat();
            }
            None
        };

    if let Some(priority) = priority_str {
        requirement.priority = parse_priority(priority)?;
    }

    if let Some(req_type) = type_str {
        requirement.req_type = parse_type(req_type)?;
    }

    // Set owner: use explicit value, AIDA_AUTHOR env var, or system username
    requirement.owner = owner.clone().unwrap_or_else(get_default_author);

    if let Some(feature_val) = feature {
        requirement.feature = feature_val.clone();
    }

    if let Some(tags) = tags_str {
        let tag_set: HashSet<String> = tags
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        requirement.tags = tag_set;
    }

    // Set prefix override if specified
    if let Some(prefix_val) = prefix {
        requirement
            .set_prefix_override(prefix_val)
            .map_err(|e| anyhow::anyhow!(e))?;
    }

    let id = requirement.id;

    // Get prefixes for ID generation
    let feature_prefix = store
        .get_feature_by_name(&requirement.feature)
        .map(|f| f.prefix.clone());
    let type_prefix = store.get_type_prefix(&requirement.req_type);

    // Add the requirement with auto-assigned ID based on configuration
    store.add_requirement_with_id(
        requirement,
        feature_prefix.as_deref(),
        type_prefix.as_deref(),
    );

    // Add parent relationship if specified.
    //
    // BUG-58: previously stored `(child, Parent, parent)` with
    // bidirectional=false, which is doubly broken:
    //   1. Relationship type is "I am X to target", so Parent on the
    //      child says "I AM the parent of <parent>" — backwards.
    //   2. Without bidirectional=true, the parent never gets the inverse
    //      Parent edge pointing at the child, so `rel list <parent>`
    //      didn't show its new child.
    // Fix: store `Child` on the source (child) pointing at the parent,
    // bidirectional so the parent gets the matching `Parent` edge.
    // Matches the convention already used by the git-canonical add
    // path (FR-215). trace:BUG-58 | ai:claude
    if let Some(parent_id) = parent_uuid {
        store
            .add_relationship(&id, RelationshipType::Child, &parent_id, true)
            .map_err(|e| anyhow::anyhow!("Failed to add parent relationship: {}", e))?;
    }

    storage.save(&store)?;

    // Get the added requirement to show its ID
    let added_req = store
        .get_requirement_by_id(&id)
        .expect("Just added requirement");

    println!("{}", "Requirement added successfully!".green());
    println!("UUID: {}", id);
    if let Some(spec_id) = &added_req.spec_id {
        println!("ID: {}", spec_id.green());
    }

    // TASK-647 (ADR-3): tell the caller their requested status was held for
    // triage. Quiet and non-fatal — the spec is filed, just as draft.
    if let Some(from) = intake_downgraded_from {
        eprintln!(
            "{} filed as {} (requested {} needs advisor authority) — queued for advisor triage.",
            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan(),
            "draft".yellow(),
            from.to_string().to_lowercase().dimmed()
        );
    }

    // Show parent relationship if created
    if let Some(parent_id_str) = parent {
        println!("Parent: {}", parent_id_str.cyan());
    }

    Ok(())
}

/// TASK-527: match one `--tags` filter token against a spec's tag set, with
/// prefix-glob support. A trailing `*` (`aida:queue:*`) matches any tag starting
/// with the literal prefix, plus the bare prefix without its trailing `:` (so
/// `aida:queue:*` also matches an exact `aida:queue` tag). Without `*` it's the
/// existing exact membership test. Comma-OR composition happens at the caller.
/// trace:TASK-527 | ai:claude
pub(crate) fn tag_filter_matches(filter: &str, tags: &std::collections::HashSet<String>) -> bool {
    if let Some(prefix) = filter.strip_suffix('*') {
        let bare = prefix.strip_suffix(':').unwrap_or(prefix);
        tags.iter().any(|t| t.starts_with(prefix) || t == bare)
    } else {
        tags.contains(filter)
    }
}

#[cfg(test)]
#[path = "tests/task_527_tag_glob_tests.rs"]
mod task_527_tag_glob_tests;

pub(crate) fn list_requirements(
    storage: &Storage,
    status: &Option<String>,
    priority: &Option<String>,
    req_type: &Option<String>,
    feature: &Option<String>,
    tags: &Option<String>,
) -> Result<()> {
    // Load requirements
    let store = storage.load_for_read()?;
    let mut requirements = store.requirements.clone();

    // Apply filters if provided
    if let Some(status_str) = status {
        let status_filter = parse_list_status_filter(status_str)?;
        requirements.retain(|r| requirement_matches_status_filter(&store, r, &status_filter));
    }

    if let Some(priority_str) = priority {
        let priority_filter = parse_priority(priority_str)?;
        requirements.retain(|r| r.priority == priority_filter);
    }

    if let Some(type_str) = req_type {
        let type_filter = parse_type(type_str)?;
        requirements.retain(|r| r.req_type == type_filter);
    }

    if let Some(feature_str) = feature {
        requirements.retain(|r| r.feature == *feature_str);
    }

    if let Some(tags_str) = tags {
        let tag_filters: Vec<String> = tags_str.split(',').map(|s| s.trim().to_string()).collect();
        requirements.retain(|r| tag_filters.iter().any(|f| tag_filter_matches(f, &r.tags)));
    }

    // Display the requirements
    if requirements.is_empty() {
        println!("{}", "No requirements found.".yellow());
        return Ok(());
    }

    const STATUS_COLUMN_WIDTH: usize = 22;
    println!(
        "{:<10} | {:<36} | {:<30} | {:<STATUS_COLUMN_WIDTH$} | {:<10} | {:<15}",
        "SPEC-ID", "UUID", "Title", "Status", "Priority", "Feature"
    );
    println!("{}", "-".repeat(132));

    for req in requirements {
        let display_status = effective_display_status(&store, &req);
        let status_str =
            list_requirement_status_cell(&store, &req, &display_status, STATUS_COLUMN_WIDTH);
        let priority_str = match req.priority {
            RequirementPriority::High => "High".red(),
            RequirementPriority::Medium => "Medium".yellow(),
            RequirementPriority::Low => "Low".green(),
        };

        let spec_id_display = req.spec_id.as_deref().unwrap_or("-");

        println!(
            "{:<10} | {:<36} | {:<30} | {} | {:<10} | {:<15}",
            spec_id_display,
            req.id.to_string(),
            req.title,
            status_str,
            priority_str,
            req.feature
        );
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ListStatusFilter {
    Stored(RequirementStatus),
    Shelved,
    NeedsDecision,
    // trace:BUG-1687 | ai:claude — the deferred VIEW axis as a `--status`
    // token. Kept in step with the git-backend path on purpose: BUG-1771's
    // lesson was that a token accepted on one listing path and refused on the
    // other is worse than one refused everywhere.
    Deferred,
}

// trace:STORY-1023 | ai:codex
// trace:BUG-1771 | ai:claude — the token set now comes from the shared
// `status_display::LENS_FILTER_TOKENS` table the git-backend list path and its
// "Unknown status filter" refusal also read, instead of a second hand-written
// `match`. The two paths had already drifted once: these tokens worked here and
// errored on the shipped (distributed-mode) CLI.
pub(crate) fn parse_list_status_filter(status_str: &str) -> Result<ListStatusFilter> {
    if let Some(key) = status_display::lens_filter_key(status_str) {
        return match key {
            "Shelved" => Ok(ListStatusFilter::Shelved),
            "NeedsDecision" => Ok(ListStatusFilter::NeedsDecision),
            // A token in the shared table with no arm here is a wiring gap, not
            // a stored status — say so rather than letting `parse_status` reject
            // it with a message that denies the token exists.
            key => anyhow::bail!(
                "status lens '{status_str}' (key '{key}') is accepted by \
                 `aida list --status` but not wired into this listing path"
            ),
        };
    }
    // trace:BUG-1687 | ai:claude — same single-table discipline for the view
    // axes: the token set lives in `VIEW_FILTER_TOKENS`, not in a second
    // hand-written match that can drift from it.
    if let Some(axis) = status_display::view_filter_axis(status_str) {
        return match axis {
            status_display::ViewFilterAxis::Deferred => Ok(ListStatusFilter::Deferred),
        };
    }
    parse_status(status_str).map(ListStatusFilter::Stored)
}

pub(crate) fn requirement_matches_status_filter(
    store: &aida_core::RequirementsStore,
    req: &aida_core::models::Requirement,
    status_filter: &ListStatusFilter,
) -> bool {
    // trace:STORY-1023 | ai:codex
    let display_status = effective_display_status(store, req);
    match status_filter {
        ListStatusFilter::Stored(status) => display_status == *status,
        ListStatusFilter::Shelved => matches!(
            effective_needs_attention_lens(store, req, &display_status),
            Some(status_display::NeedsAttentionLens::Shelved { .. })
        ),
        ListStatusFilter::NeedsDecision => matches!(
            effective_needs_attention_lens(store, req, &display_status),
            Some(status_display::NeedsAttentionLens::NeedsDecision { .. })
        ),
        // trace:BUG-1687 | ai:claude — the defer axis, read through the same
        // flag-OR-legacy-tag predicate the cache query uses, so the two listing
        // paths return the same set.
        ListStatusFilter::Deferred => status_display::is_deferred(req),
    }
}

pub(crate) fn list_requirement_status_cell(
    store: &aida_core::RequirementsStore,
    req: &aida_core::models::Requirement,
    display_status: &RequirementStatus,
    width: usize,
) -> String {
    // trace:STORY-1023 | ai:codex
    let (label, palette_key) = if matches!(display_status, RequirementStatus::NeedsAttention) {
        effective_needs_attention_lens(store, req, display_status)
            .map(|lens| {
                let label = lens.label();
                let key = lens.palette_key();
                (label, key)
            })
            .unwrap_or_else(|| ("Needs Decision".to_string(), "NeedsDecision"))
    } else {
        let label = match display_status {
            RequirementStatus::Draft => "Draft",
            RequirementStatus::Approved => "Approved",
            RequirementStatus::Planned => "Planned",
            RequirementStatus::InProgress => "In Progress",
            RequirementStatus::Done => "Done",
            RequirementStatus::Completed => "Completed",
            RequirementStatus::Rejected => "Rejected",
            RequirementStatus::Superseded => "Superseded",
            RequirementStatus::NeedsAttention => unreachable!("handled above"),
        };
        (label.to_string(), label)
    };
    let padded = format!("{label:<width$}");
    status_display::paint_status(&padded, palette_key).to_string()
}

pub(crate) fn needs_attention_badge_for_lens(lens: &status_display::NeedsAttentionLens) -> String {
    let label = lens.label();
    let key = lens.palette_key();
    format!(
        "{} {}",
        status_display::status_glyph(key),
        status_display::paint_status(&label, key)
    )
}

pub(crate) fn effective_needs_attention_lens(
    store: &aida_core::RequirementsStore,
    req: &aida_core::models::Requirement,
    display_status: &RequirementStatus,
) -> Option<status_display::NeedsAttentionLens> {
    effective_needs_attention_lens_with_source(store, req, display_status).map(|(_, lens)| lens)
}

pub(crate) fn effective_needs_attention_lens_with_source<'a>(
    store: &'a aida_core::RequirementsStore,
    req: &'a aida_core::models::Requirement,
    display_status: &RequirementStatus,
) -> Option<(
    &'a aida_core::models::Requirement,
    status_display::NeedsAttentionLens,
)> {
    if !matches!(display_status, RequirementStatus::NeedsAttention) {
        return None;
    }
    if let Some(lens) = status_display::needs_attention_lens(req) {
        return Some((req, lens));
    }
    if req.req_type == RequirementType::Epic {
        let mut decision_lens = None;
        // trace:STORY-1023 | ai:codex
        for child_id in aida_core::graph_walk::subtree_ids(store, req.id, None).nodes {
            let Some(child) = store.get_requirement_by_id(&child_id) else {
                continue;
            };
            let child_display_status = effective_display_status(store, child);
            match effective_needs_attention_lens_with_source(store, child, &child_display_status) {
                Some((source, status_display::NeedsAttentionLens::Shelved { cause })) => {
                    return Some((
                        source,
                        status_display::NeedsAttentionLens::Shelved { cause },
                    ));
                }
                Some((source, status_display::NeedsAttentionLens::NeedsDecision { reason })) => {
                    decision_lens.get_or_insert((
                        source,
                        status_display::NeedsAttentionLens::NeedsDecision { reason },
                    ));
                }
                None => {}
            }
        }
        if decision_lens.is_some() {
            return decision_lens;
        }
    }
    Some((
        req,
        status_display::NeedsAttentionLens::NeedsDecision { reason: None },
    ))
}

#[cfg(test)]
mod story_1023_list_render_tests {
    use super::*;

    #[test]
    fn list_status_cell_splits_shelved_from_needs_decision() {
        let mut shelved =
            aida_core::models::Requirement::new("stale base".to_string(), String::new());
        shelved.status = RequirementStatus::NeedsAttention;
        shelved.failure_reason = Some(aida_core::FailureReason {
            phase: "review".to_string(),
            phase_index: 3,
            kind: "stale-base".to_string(),
            detail: "base moved under the branch".to_string(),
            recovery_hint: Some("rebase and retry".to_string()),
            shelved_by: Some("codex".to_string()),
            shelved_at: chrono::Utc::now(),
        });

        let mut decision =
            aida_core::models::Requirement::new("design fork".to_string(), String::new());
        decision.status = RequirementStatus::NeedsAttention;
        decision.attention_reason = Some(aida_core::AttentionReason {
            category: aida_core::PuntCategory::DesignFork,
            detail: "choose the public API shape".to_string(),
            lean: None,
            raised_by: Some("codex".to_string()),
            raised_at: chrono::Utc::now(),
        });

        let store = aida_core::RequirementsStore::new();
        colored::control::set_override(false);
        let shelved_cell = list_requirement_status_cell(&store, &shelved, &shelved.status, 22);
        let decision_cell = list_requirement_status_cell(&store, &decision, &decision.status, 22);
        colored::control::unset_override();

        assert!(
            shelved_cell.contains("Shelved (stale-base)"),
            "cell: {shelved_cell:?}"
        );
        assert!(
            !shelved_cell.contains("Needs Attention"),
            "cell: {shelved_cell:?}"
        );
        assert!(
            decision_cell.contains("Needs Decision (design-fork)"),
            "cell: {decision_cell:?}"
        );
    }

    #[test]
    fn status_filter_uses_effective_display_status_and_parked_lens() {
        let mut store = aida_core::RequirementsStore::new();

        let mut epic = aida_core::models::Requirement::new("Epic".to_string(), String::new());
        epic.req_type = RequirementType::Epic;
        epic.status = RequirementStatus::Approved;
        let mut child =
            aida_core::models::Requirement::new("stale child".to_string(), String::new());
        child.status = RequirementStatus::NeedsAttention;
        child.failure_reason = Some(aida_core::FailureReason {
            phase: "review".to_string(),
            phase_index: 3,
            kind: "stale-base".to_string(),
            detail: "base moved under the branch".to_string(),
            recovery_hint: Some("rebase and retry".to_string()),
            shelved_by: Some("codex".to_string()),
            shelved_at: chrono::Utc::now(),
        });
        child.relationships.push(aida_core::models::Relationship {
            rel_type: aida_core::models::RelationshipType::Parent,
            target_id: epic.id,
            created_at: None,
            created_by: None,
        });

        let epic_id = epic.id;
        let child_id = child.id;
        store.requirements.push(epic);
        store.requirements.push(child);

        let epic = store.get_requirement_by_id(&epic_id).unwrap();
        assert_eq!(
            effective_display_status(&store, epic),
            RequirementStatus::NeedsAttention
        );
        assert_ne!(epic.status, RequirementStatus::NeedsAttention);

        colored::control::set_override(false);
        let epic_cell =
            list_requirement_status_cell(&store, epic, &RequirementStatus::NeedsAttention, 22);
        colored::control::unset_override();
        assert!(
            epic_cell.contains("Shelved (stale-base)"),
            "cell: {epic_cell:?}"
        );

        assert!(requirement_matches_status_filter(
            &store,
            epic,
            &ListStatusFilter::Stored(RequirementStatus::NeedsAttention)
        ));
        assert!(requirement_matches_status_filter(
            &store,
            epic,
            &ListStatusFilter::Shelved
        ));
        assert!(!requirement_matches_status_filter(
            &store,
            epic,
            &ListStatusFilter::NeedsDecision
        ));

        let (source, lens) = effective_needs_attention_lens_with_source(
            &store,
            epic,
            &RequirementStatus::NeedsAttention,
        )
        .expect("effective lens");
        assert_eq!(source.id, child_id);
        assert_eq!(lens.label(), "Shelved (stale-base)");
    }
}

/// Read the `[external_refs]` provider → base-URL map from `.aida/config.toml`.
///
/// Each line under the section is `provider = "https://..."` (e.g.
/// `linear = "https://linear.app/acme/issue/"`). The project root is the
/// parent of the orphan store path; fall back to `find_project_root()` and
/// finally CWD. Missing file or section yields an empty map (callers then use
/// the built-in defaults — see `aida_core::external_refs::render_ref_url`).
/// Mirrors the line-by-line `read_config_workflow_hints` pattern.
/// trace:STORY-476 | ai:claude
pub(crate) fn read_external_ref_base_urls(
    store_path: &std::path::Path,
) -> std::collections::HashMap<String, String> {
    let project_root = store_path
        .parent()
        .map(|p| p.to_path_buf())
        .or_else(|| find_project_root().ok())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let config_path = project_root.join(".aida").join("config.toml");
    let mut map = std::collections::HashMap::new();
    let content = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return map,
    };
    let mut in_section = false;
    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(stripped) = line.strip_prefix('[') {
            in_section = stripped.trim_end_matches(']').trim() == "external_refs";
            continue;
        }
        if in_section {
            if let Some((key, val)) = line.split_once('=') {
                let provider = key.trim().to_ascii_lowercase();
                // Strip a trailing inline `# comment`, then surrounding quotes.
                let v = val
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string();
                if !provider.is_empty() && !v.is_empty() {
                    map.insert(provider, v);
                }
            }
        }
    }
    map
}

/// The status to DISPLAY for a requirement. For an EPIC this is the read-only
/// rollup of its children (`derive_epic_status`), so the full-store display
/// surfaces (`aida show`, `aida why`, `aida status`) agree with the cache-backed
/// `aida list`, whose `status` column already holds the derived value. For every
/// non-epic, and for an epic whose only children are Rejected (the derivation
/// declines to auto-reject), this is the stored status.
// trace:BUG-626 | ai:claude
pub(crate) fn effective_display_status(
    store: &aida_core::RequirementsStore,
    req: &aida_core::models::Requirement,
) -> RequirementStatus {
    if req.req_type == RequirementType::Epic {
        if let Some(derived) = aida_core::rollup::derive_epic_status(store, req.id) {
            return derived;
        }
    }
    req.status.clone()
}

pub(crate) fn show_requirement(storage: &Storage, id_str: &str) -> Result<()> {
    // Load requirements first (needed for SPEC-ID lookup)
    let store = storage.load_for_read()?;

    // Parse UUID or SPEC-ID
    let id = parse_requirement_id(id_str, &store)?;

    // Find the specified requirement
    let req = store
        .get_requirement_by_id(&id)
        .context("Requirement not found")?;

    // Display the requirement details
    println!("{}: {}", "ID".blue(), req.id);
    if let Some(spec_id) = &req.spec_id {
        println!("{}: {}", "SPEC-ID".blue(), spec_id);
    }
    println!("{}: {}", "Title".blue(), req.title);
    println!("{}: {}", "Description".blue(), req.description);

    // BUG-626: an epic's displayed status is the read-only rollup of its
    // children, not the stored field. trace:BUG-626 | ai:claude
    let display_status = effective_display_status(&store, req);
    // BUG-1687: deferral is a display override that outranks both the parked
    // lens and the stored status — a deferred spec must not read as "act now".
    // trace:BUG-1687 | ai:claude
    let status_str = if let Some(badge) = status_display::deferred_badge(req) {
        badge
    } else if matches!(display_status, RequirementStatus::NeedsAttention) {
        effective_needs_attention_lens(&store, req, &display_status)
            .map(|lens| needs_attention_badge_for_lens(&lens))
            .unwrap_or_else(|| status_display::parked_status_badge(req))
    } else {
        match display_status {
            RequirementStatus::Draft => "Draft".yellow().to_string(),
            RequirementStatus::Approved => "Approved".blue().to_string(),
            RequirementStatus::Planned => "Planned".cyan().to_string(),
            RequirementStatus::InProgress => "In Progress".magenta().to_string(),
            RequirementStatus::Done => "Done".bright_green().bold().to_string(),
            RequirementStatus::Completed => "Completed".green().to_string(),
            RequirementStatus::Rejected => "Rejected".red().to_string(),
            // trace:TASK-1176 | ai:claude
            RequirementStatus::Superseded => "Superseded".green().dimmed().to_string(),
            RequirementStatus::NeedsAttention => unreachable!("handled above"),
        }
    };
    println!("{}: {}", "Status".blue(), status_str);

    let priority_str = match req.priority {
        RequirementPriority::High => "High".red(),
        RequirementPriority::Medium => "Medium".yellow(),
        RequirementPriority::Low => "Low".green(),
    };
    println!("{}: {}", "Priority".blue(), priority_str);

    let type_str = match req.req_type {
        RequirementType::Functional => "Functional",
        RequirementType::NonFunctional => "Non-Functional",
        RequirementType::System => "System",
        RequirementType::User => "User",
        RequirementType::ChangeRequest => "Change Request",
        RequirementType::Bug => "Bug",
        RequirementType::Epic => "Epic",
        RequirementType::Story => "Story",
        RequirementType::Task => "Task",
        RequirementType::Spike => "Spike",
        RequirementType::Sprint => "Sprint",
        RequirementType::Folder => "Folder",
        RequirementType::Meta => "Meta",
        RequirementType::Principle => "Principle",
        RequirementType::Vision => "Vision",
        RequirementType::Constraint => "Constraint",
        RequirementType::Decision => "Decision",
        RequirementType::Term => "Term",
        RequirementType::Doc => "Doc",
        RequirementType::Faq => "Faq",
    };
    println!("{}: {}", "Type".blue(), type_str);

    println!("{}: {}", "Owner".blue(), req.owner);
    // STORY-639: render assignee only when set. trace:STORY-639 | ai:claude
    if let Some(assignee) = req.assignee.as_deref() {
        println!("{}: {}", "Assignee".blue(), assignee);
    }
    println!("{}: {}", "Feature".blue(), req.feature);
    println!("{}: {}", "Created".blue(), req.created_at);
    println!("{}: {}", "Modified".blue(), req.modified_at);

    if !req.tags.is_empty() {
        let tags_str = req.tags.iter().cloned().collect::<Vec<_>>().join(", ");
        println!("{}: {}", "Tags".blue(), tags_str);
    }

    if !req.dependencies.is_empty() {
        let deps_str = req
            .dependencies
            .iter()
            .map(|uuid| uuid.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        println!("{}: {}", "Dependencies".blue(), deps_str);
    }

    // STORY-542: user-facing interface changes captured at close — the
    // deterministic source the operator digest reads. trace:STORY-542
    if let Some(ic) = req.interface_changes.as_ref() {
        if !ic.is_empty() {
            println!("\n{}:", "Interface changes".green());
            let surfaces: [(&str, &Vec<String>); 4] = [
                ("CLI", &ic.cli),
                ("MCP", &ic.mcp),
                ("TUI", &ic.tui),
                ("Other", &ic.other),
            ];
            for (label, lines) in surfaces {
                for line in lines {
                    println!("  {} {}", format!("[{label}]").cyan(), line);
                }
            }
        }
    }

    // STORY-698: the verification steps the builder ran, captured at
    // `aida queue done` — the implementation audit trail the PR body surfaces.
    // trace:STORY-698 | ai:claude
    if let Some(steps) = req
        .implementation_info
        .as_ref()
        .and_then(|info| info.test_coverage_notes.as_ref())
        .filter(|s| !s.trim().is_empty())
    {
        println!("\n{}:", "Verification steps".green());
        for line in steps.lines().filter(|l| !l.trim().is_empty()) {
            println!("  {} {}", "-".dimmed(), line);
        }
    }

    if !req.relationships.is_empty() {
        println!("\n{}:", "Relationships".green());
        for relationship in &req.relationships {
            let target = store.get_requirement_by_id(&relationship.target_id);
            if let Some(target_req) = target {
                let target_spec = target_req.spec_id.as_deref().unwrap_or("N/A");

                // TASK-102: shared phrase mapping (STORY-333 added typed
                // blocked-by/blocks prose; now via `relationship_phrase`).
                let description = relationship_phrase(&relationship.rel_type);

                println!(
                    "  {} {} - {}",
                    description.cyan(),
                    target_spec.yellow(),
                    target_req.title
                );
            } else {
                println!(
                    "  {} {} {}",
                    relationship.rel_type.to_string().cyan(),
                    relationship.target_id.to_string().yellow(),
                    "(not found)".red()
                );
            }
        }
    }

    if !req.comments.is_empty() {
        println!("\n{}:", "Comments".green());
        for comment in &req.comments {
            print_comment(comment, 0);
        }
    }

    if !req.history.is_empty() {
        println!("\n{}:", "History".green());
        for entry in &req.history {
            println!(
                "\n{}:",
                entry
                    .timestamp
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
                    .yellow()
            );
            println!("  {} {}", "By:".dimmed(), entry.author.cyan());
            for change in &entry.changes {
                println!(
                    "  {} {} → {}",
                    change.field_name.magenta(),
                    change.old_value.red(),
                    change.new_value.green()
                );
            }
        }
    }

    Ok(())
}

/// STORY-582: render the durable processing-record audit trail for `aida
/// show`. One block per record: timestamp + agent header, the linkage
/// (PR/commit/brief), the summary, and the decisions / punted / verdict
/// tails when present. trace:STORY-582 | ai:claude
pub(crate) fn print_processing_records(records: &[aida_core::ProcessingRecord]) {
    println!("\n{}:", "Processing record".green().bold());
    for rec in records {
        println!(
            "\n{}  {}",
            rec.timestamp
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
                .yellow(),
            format!("by {}", rec.agent).cyan(),
        );
        // Linkage line — PR / commit / brief, whichever are present.
        let mut linkage: Vec<String> = Vec::new();
        if let Some(pr) = rec.pr {
            linkage.push(format!("PR #{pr}"));
        }
        if let Some(sha) = rec.commit_sha.as_deref() {
            linkage.push(format!("commit {}", &sha[..sha.len().min(8)]));
        }
        if let Some(brief) = rec.brief_ref.as_deref() {
            linkage.push(format!("brief {brief}"));
        }
        if let Some(v) = rec.review_verdict.as_deref() {
            linkage.push(format!("verdict {v}"));
        }
        if !linkage.is_empty() {
            println!(
                "  {} {}",
                crate::glyph(crate::glyphs::Glyph::SubArrow).dimmed(),
                linkage.join(" · ").dimmed()
            );
        }
        println!("  {}", rec.summary);
        for d in &rec.decisions {
            println!("  {} {}", "decision:".magenta(), d);
        }
        for p in &rec.punted {
            println!("  {} {}", "punted:".yellow(), p);
        }
    }
}

/// STORY-582: `aida record list|prune` — inspect or trim the durable
/// processing-record audit trail. List is read-only (backend); prune writes
/// through `Storage::update_atomically`, propose-by-default. trace:STORY-582
// trace:REQ-0232 | ai:claude:high
/// What a `--add-tag` / `--remove-tag` pass actually did.
///
/// BUG-1770: the bool this replaces could not distinguish "removed nothing
/// because the tag was absent" from "removed nothing because something went
/// wrong", and the caller rendered both as `No changes specified` — a message
/// that sends the user looking for a flag they already passed. Naming the tags
/// that matched nothing turns a silent no-op into a visible one.
// trace:BUG-1770 | ai:claude
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TagDeltaReport {
    /// Tags newly inserted.
    pub(crate) added: Vec<String>,
    /// Tags actually removed.
    pub(crate) removed: Vec<String>,
    /// `--add-tag` values that were already present.
    pub(crate) already_present: Vec<String>,
    /// `--remove-tag` values that matched no existing tag.
    pub(crate) absent: Vec<String>,
}

impl TagDeltaReport {
    pub(crate) fn changed(&self) -> bool {
        !self.added.is_empty() || !self.removed.is_empty()
    }

    /// One line per outcome, or `None` when nothing was requested at all.
    /// Deterministic order so output is stable for tests and scripts.
    pub(crate) fn summary_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        let list = |v: &[String]| v.join(", ");
        if !self.added.is_empty() {
            out.push(format!(
                "added {}: {}",
                plural(self.added.len(), "tag"),
                list(&self.added)
            ));
        }
        if !self.removed.is_empty() {
            out.push(format!(
                "removed {}: {}",
                plural(self.removed.len(), "tag"),
                list(&self.removed)
            ));
        }
        if !self.already_present.is_empty() {
            out.push(format!(
                "already present, nothing added: {}",
                list(&self.already_present)
            ));
        }
        if !self.absent.is_empty() {
            out.push(format!(
                "no matching {} to remove: {}",
                if self.absent.len() == 1 {
                    "tag"
                } else {
                    "tags"
                },
                list(&self.absent)
            ));
        }
        out
    }
}

pub(crate) fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// The flag name used in a whitespace refusal raised for `aida add --tags` /
/// `aida edit --tags`, where the pasteable repair is one comma-separated value.
// trace:BUG-1770 | ai:claude
pub(crate) const TAGS_FLAG: &str = "--tags";

/// How a refused tag value arrived, so the refusal can show a repair the
/// caller can actually paste back.
///
/// A refusal that suggests `--tags a,b` to an MCP client, or a comma list to a
/// repeatable flag, names a form that surface does not accept — which is a
/// worse failure than the blob, because it reads as authoritative.
// trace:BUG-1770 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagRepair {
    /// A comma-separated argument: `--tags a,b`.
    CommaFlag(&'static str),
    /// A repeatable single-value flag: `--add-tag a --add-tag b`.
    RepeatedFlag(&'static str),
    /// A JSON array field on an MCP tool call: `"tags": ["a", "b"]`.
    JsonArray(&'static str),
}

impl TagRepair {
    /// The pasteable repair for `parts`, in this surface's own syntax.
    pub(crate) fn suggestion(&self, parts: &[&str]) -> String {
        match self {
            Self::CommaFlag(flag) => format!("{flag} {}", parts.join(",")),
            Self::RepeatedFlag(flag) => parts
                .iter()
                .map(|p| format!("{flag} {p}"))
                .collect::<Vec<_>>()
                .join(" "),
            Self::JsonArray(field) => format!(
                "\"{field}\": [{}]",
                parts
                    .iter()
                    .map(|p| format!("\"{p}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

/// Refuse a tag value that contains whitespace.
///
/// A space-separated `--tags` argument is stored as ONE tag whose human
/// rendering is byte-identical to the N tags it was meant to be, so the
/// malformation is invisible by eye while being unmatchable by `--remove-tag`
/// and invisible to every tag-keyed filter and sweep. Refusing is preferred
/// over silently splitting on whitespace: splitting would make a genuinely
/// intended multi-word tag unrepresentable without warning, and the caller's
/// intent here is unambiguous enough to just report.
///
/// `flag` is the flag the value arrived on, so the suggestion is pasteable.
/// `--remove-tag` is deliberately NOT validated: a tag already malformed in the
/// store can only be named by reproducing it verbatim, and refusing that would
/// leave the existing blobs unrepairable by the incremental form.
// trace:BUG-1770 | ai:claude
pub(crate) fn validate_tag_value(tag: &str, repair: TagRepair) -> anyhow::Result<()> {
    if !tag.chars().any(char::is_whitespace) {
        return Ok(());
    }
    let parts: Vec<&str> = tag.split_whitespace().collect();
    let suggestion = repair.suggestion(&parts);
    anyhow::bail!(
        "tag \"{tag}\" contains whitespace — did you mean `{suggestion}` ?\n  \
         A tag may not contain whitespace: the whole value would be stored as ONE tag that \
         renders identically to the {} separate tags it looks like, and would then be \
         invisible to --remove-tag and to every tag-keyed filter.",
        parts.len()
    )
}

/// Parse a comma-separated `--tags` argument into validated tag values,
/// trimmed, empties dropped, order preserved.
///
/// Every `--tags` write path goes through this so the whitespace rule cannot be
/// reintroduced by a fourth site splitting the string itself.
// trace:BUG-1770 | ai:claude
pub(crate) fn parse_tag_list(raw: &str) -> anyhow::Result<Vec<String>> {
    let mut out = Vec::new();
    for tag in raw.split(',') {
        let trimmed = tag.trim();
        if trimmed.is_empty() {
            continue;
        }
        validate_tag_value(trimmed, TagRepair::CommaFlag(TAGS_FLAG))?;
        out.push(trimmed.to_string());
    }
    Ok(out)
}

/// The reporting form. [`apply_tag_deltas`] is the bool-returning wrapper kept
/// for callers that only need "did anything change".
// trace:BUG-1770 | ai:claude
pub(crate) fn apply_tag_deltas_report(
    tags: &mut HashSet<String>,
    add: &[String],
    remove: &[String],
) -> anyhow::Result<TagDeltaReport> {
    // BUG-1770: validate the entire `add` list BEFORE mutating anything, so a
    // refusal can never leave a half-applied tag set behind for the caller to
    // save. trace:BUG-1770 | ai:claude
    for raw in add {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            validate_tag_value(trimmed, TagRepair::RepeatedFlag("--add-tag"))?;
        }
    }
    let mut report = TagDeltaReport::default();
    for raw in add {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if tags.insert(trimmed.to_string()) {
            report.added.push(trimmed.to_string());
        } else {
            report.already_present.push(trimmed.to_string());
        }
    }
    for raw in remove {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if tags.remove(trimmed) {
            report.removed.push(trimmed.to_string());
        } else {
            report.absent.push(trimmed.to_string());
        }
    }
    Ok(report)
}

/// Apply additive (`--add-tag`) and subtractive (`--remove-tag`) tag deltas
/// without disturbing tags the caller didn't name. Empty / whitespace-only
/// entries are ignored. Adding a present tag or removing an absent one is
/// a graceful no-op. Returns whether the set actually changed.
// trace:TASK-351 | ai:claude
pub(crate) fn apply_tag_deltas(
    tags: &mut HashSet<String>,
    add: &[String],
    remove: &[String],
) -> anyhow::Result<bool> {
    Ok(apply_tag_deltas_report(tags, add, remove)?.changed())
}

/// Build the loud-on-clobber warning shown when `aida edit --tags` REPLACES the
/// whole tag set. `--tags` is a full replace (other scripts depend on that), so
/// we don't change its semantics — we surface the old→new diff so the caller
/// sees that provenance/routing tags were dropped, and point at the incremental
/// `--add-tag` / `--remove-tag` forms. Returns `None` when the set is unchanged
/// (no warning needed). The returned string is sorted for deterministic output.
/// trace:BUG-545 | ai:claude
pub(crate) fn tags_replace_warning(
    old_tags: &HashSet<String>,
    new_tags: &HashSet<String>,
) -> Option<String> {
    if old_tags == new_tags {
        return None;
    }
    let fmt_set = |s: &HashSet<String>| -> String {
        if s.is_empty() {
            "(none)".to_string()
        } else {
            let mut v: Vec<&String> = s.iter().collect();
            v.sort();
            v.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(",")
        }
    };
    Some(format!(
        "--tags REPLACES all tags (was: {} → now: {}). \
         Use --add-tag/--remove-tag to modify incrementally.",
        fmt_set(old_tags),
        fmt_set(new_tags)
    ))
}

// trace:BUG-1252 | ai:codex
/// Structural tags participate in routing and graph integrity. A full tag-set
/// replacement must not silently erase them.
pub(crate) fn dropped_structural_tags(
    old_tags: &HashSet<String>,
    new_tags: &HashSet<String>,
) -> Vec<String> {
    const PREFIXES: &[&str] = &[
        "parent:",
        "batch:",
        "lane:",
        "severity:",
        "lifecycle:",
        "aida:",
    ];
    let mut dropped: Vec<String> = old_tags
        .difference(new_tags)
        .filter(|tag| PREFIXES.iter().any(|prefix| tag.starts_with(prefix)))
        .cloned()
        .collect();
    dropped.sort();
    dropped
}

pub(crate) fn enforce_structural_tag_replacement(
    old_tags: &HashSet<String>,
    new_tags: &HashSet<String>,
    force: bool,
) -> Result<Vec<String>> {
    let dropped = dropped_structural_tags(old_tags, new_tags);
    if !dropped.is_empty() && !force {
        anyhow::bail!(
            "AIDA_AGENT_OUTPUT structural_tags_dropped=[{}] refusal=use_--add-tag/--remove-tag_or_pass_--force; refusing --tags replacement because it would drop structural tags: {}. Use --add-tag/--remove-tag for incremental edits, or --force/--replace-tags to replace intentionally.",
            dropped.join(","), dropped.join(", ")
        );
    }
    Ok(dropped)
}

/// STORY-439: stamp `complexity:<level>` / `estimated-assistance:<level>`
/// tags on `spec` so the new dimension composes with existing tag tooling
/// (`aida queue list --tag-prefix complexity:`, batch routing). Mirrors
/// `load_store_for_lookup` for backend resolution. Best-effort — a missing
/// store / missing spec / save failure logs and returns; the pickup itself
/// is unaffected. trace:STORY-439 | ai:claude
pub(crate) fn apply_calibration_tags(
    storage: &Storage,
    spec: &str,
    complexity: Option<complexity_calibration::ComplexityLevel>,
    assist_est: Option<complexity_calibration::AssistanceLevel>,
) {
    if complexity.is_none() && assist_est.is_none() {
        return;
    }
    // Git-canonical path — direct backend write, exactly like Command::Edit.
    if let Some(store_path) = detect_distributed_store() {
        // Cache-backed so the checked lookup below does not load the whole
        // store. trace:TASK-1468 | ai:claude
        if let Ok(backend) = aida_core::CachedGitBackend::open(
            &store_path,
            &aida_core::CachedGitBackend::default_cache_path(&store_path),
        ) {
            use aida_core::DatabaseBackend;
            // trace:TASK-1468 | ai:claude
            let Ok(Some(mut req)) = backend.get_requirement_unambiguous(spec) else {
                return;
            };
            let mut changed = false;
            if let Some(c) = complexity {
                changed |= complexity_calibration::apply_complexity_tag(&mut req.tags, c);
            }
            if let Some(a) = assist_est {
                changed |= complexity_calibration::apply_assistance_tag(&mut req.tags, a);
            }
            if changed {
                req.modified_at = chrono::Utc::now();
                if let Err(e) = backend.update_requirement(&req) {
                    eprintln!(
                        "  {} could not stamp calibration tags on {spec}: {e}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                    );
                }
            }
            return;
        }
    }
    // Legacy fallback — load the whole store, mutate, save. Heavier but
    // only fires on projects that haven't migrated.
    let Ok(mut store) = storage.load() else {
        return;
    };
    let Some(req) = store
        .requirements
        .iter_mut()
        .find(|r| r.spec_id.as_deref() == Some(spec) || r.agreed_id.as_deref() == Some(spec))
    else {
        return;
    };
    let mut changed = false;
    if let Some(c) = complexity {
        changed |= complexity_calibration::apply_complexity_tag(&mut req.tags, c);
    }
    if let Some(a) = assist_est {
        changed |= complexity_calibration::apply_assistance_tag(&mut req.tags, a);
    }
    if changed {
        req.modified_at = chrono::Utc::now();
        if let Err(e) = storage.save(&store) {
            eprintln!(
                "  {} could not save calibration tags on {spec}: {e}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
    }
}

/// STORY-451: stamp `effort:<touchpoint>:<bucket>` while preserving the
/// other effort touchpoints. Best-effort sibling of [`apply_calibration_tags`].
/// trace:STORY-451 | ai:codex
pub(crate) fn apply_effort_tag(
    storage: &Storage,
    spec: &str,
    touchpoint: effort_calibration::EffortTouchpoint,
    effort: Option<effort_calibration::EffortBucket>,
) {
    let Some(effort) = effort else {
        return;
    };
    if let Some(store_path) = detect_distributed_store() {
        // Cache-backed so the checked lookup below does not load the whole
        // store. trace:TASK-1468 | ai:claude
        if let Ok(backend) = aida_core::CachedGitBackend::open(
            &store_path,
            &aida_core::CachedGitBackend::default_cache_path(&store_path),
        ) {
            use aida_core::DatabaseBackend;
            // trace:TASK-1468 | ai:claude
            let Ok(Some(mut req)) = backend.get_requirement_unambiguous(spec) else {
                return;
            };
            if effort_calibration::apply_effort_tag(&mut req.tags, touchpoint, effort) {
                req.modified_at = chrono::Utc::now();
                if let Err(e) = backend.update_requirement(&req) {
                    eprintln!(
                        "  {} could not stamp effort tag on {spec}: {e}",
                        crate::glyph(crate::glyphs::Glyph::Warning).yellow()
                    );
                }
            }
            return;
        }
    }
    let Ok(mut store) = storage.load() else {
        return;
    };
    let Some(req) = store
        .requirements
        .iter_mut()
        .find(|r| r.spec_id.as_deref() == Some(spec) || r.agreed_id.as_deref() == Some(spec))
    else {
        return;
    };
    if effort_calibration::apply_effort_tag(&mut req.tags, touchpoint, effort) {
        req.modified_at = chrono::Utc::now();
        if let Err(e) = storage.save(&store) {
            eprintln!(
                "  {} could not save effort tag on {spec}: {e}",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
        }
    }
}

#[cfg(test)]
#[path = "tests/apply_tag_deltas_tests.rs"]
mod apply_tag_deltas_tests;

#[cfg(test)]
#[path = "tests/bug_1770_tag_whitespace_tests.rs"]
mod bug_1770_tag_whitespace_tests;

#[cfg(test)]
#[path = "tests/tags_replace_warning_tests.rs"]
mod tags_replace_warning_tests;

/// Edit a requirement non-interactively using CLI flags
// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn edit_requirement_cli(
    storage: &Storage,
    id_str: &str,
    title: &Option<String>,
    description: &Option<String>,
    status: &Option<String>,
    priority: &Option<String>,
    req_type: &Option<String>,
    owner: &Option<String>,
    feature: &Option<String>,
    tags: &Option<String>,
    // trace:TASK-351 | ai:claude
    add_tag: &[String],
    remove_tag: &[String],
) -> Result<()> {
    // Load requirements
    let store_for_lookup = storage.load()?;
    let id = parse_requirement_id(id_str, &store_for_lookup)?;

    let mut store = storage.load()?;
    let req = store
        .get_requirement_by_id_mut(&id)
        .context("Requirement not found")?;

    let mut changes: Vec<FieldChange> = Vec::new();
    let spec_id = req.spec_id.clone().unwrap_or_else(|| req.id.to_string());

    // Update title
    if let Some(new_title) = title {
        if !new_title.is_empty() && new_title != &req.title {
            changes.push(Requirement::field_change(
                "title",
                req.title.clone(),
                new_title.clone(),
            ));
            req.title = new_title.clone();
        }
    }

    // Update description
    if let Some(new_desc) = description {
        if new_desc != &req.description {
            changes.push(Requirement::field_change(
                "description",
                req.description.clone(),
                new_desc.clone(),
            ));
            req.description = new_desc.clone();
        }
    }

    // Update status
    let mut left_needs_attention = false;
    // STORY-1649: captured against the pre-edit status so the completion
    // linkage warning fires only on the into-Completed transition.
    // trace:STORY-1649 | ai:claude
    let mut into_completed = false;
    if let Some(status_str) = status {
        let new_status = match status_str.to_lowercase().as_str() {
            "draft" => RequirementStatus::Draft,
            "approved" => RequirementStatus::Approved,
            "planned" => RequirementStatus::Planned,
            "in_progress" | "in-progress" | "inprogress" => RequirementStatus::InProgress,
            "done" => RequirementStatus::Done,
            "completed" => RequirementStatus::Completed,
            "rejected" => RequirementStatus::Rejected,
            "needs_attention" | "needs-attention" | "needsattention" => {
                RequirementStatus::NeedsAttention
            }
            _ => anyhow::bail!("Invalid status '{}'. Use: draft, approved, planned, in_progress, done, completed, rejected, needs-attention", status_str),
        };
        // STORY-332: enforce the NeedsAttention transition rules here too.
        if let Some(msg) = forbidden_attention_transition(&req.status, &new_status) {
            anyhow::bail!(msg);
        }
        if new_status != req.status {
            // TASK-358: a triage that takes a spec out of NeedsAttention is
            // the trigger for cleaning up any orchestrator-escalated worktree
            // for it. Capture the transition direction before the field
            // changes — the actual cleanup fires after the store save so a
            // failed save doesn't leave a half-cleaned state.
            // trace:TASK-358 | ai:claude
            left_needs_attention = matches!(req.status, RequirementStatus::NeedsAttention)
                && !matches!(new_status, RequirementStatus::NeedsAttention);
            changes.push(Requirement::field_change(
                "status",
                format!("{:?}", req.status),
                format!("{:?}", new_status),
            ));
            let prior = req.status.clone();
            into_completed = !matches!(prior, RequirementStatus::Completed)
                && matches!(new_status, RequirementStatus::Completed);
            req.status = new_status;
            // trace:TASK-1600 | ai:codex
            completion::record_reopen(req, &prior, Some(&queue_cmd::requeue_project_root(storage)));
        }
    }

    // Update priority
    if let Some(priority_str) = priority {
        let new_priority = match priority_str.to_lowercase().as_str() {
            "high" => RequirementPriority::High,
            "medium" | "med" => RequirementPriority::Medium,
            "low" => RequirementPriority::Low,
            _ => anyhow::bail!(
                "Invalid priority '{}'. Use: high, medium, low",
                priority_str
            ),
        };
        if new_priority != req.priority {
            changes.push(Requirement::field_change(
                "priority",
                format!("{:?}", req.priority),
                format!("{:?}", new_priority),
            ));
            req.priority = new_priority;
        }
    }

    // Update type
    if let Some(type_str) = req_type {
        let new_type = match type_str.to_lowercase().as_str() {
            "functional" | "func" => RequirementType::Functional,
            "non-functional" | "nonfunctional" | "nfr" => RequirementType::NonFunctional,
            "system" | "sys" => RequirementType::System,
            "user" => RequirementType::User,
            "change-request" | "change" | "cr" => RequirementType::ChangeRequest,
            "bug" => RequirementType::Bug,
            "epic" => RequirementType::Epic,
            "story" => RequirementType::Story,
            "task" => RequirementType::Task,
            "spike" => RequirementType::Spike,
            "sprint" => RequirementType::Sprint,
            "folder" => RequirementType::Folder,
            "meta" => RequirementType::Meta,
            // ADR / knowledge-graph family (FR-1-074). trace:TASK-716 | ai:claude
            "principle" | "prin" => RequirementType::Principle,
            "vision" | "vis" => RequirementType::Vision,
            "constraint" | "con" => RequirementType::Constraint,
            "decision" | "adr" => RequirementType::Decision,
            "term" | "glossary" => RequirementType::Term,
            // trace:STORY-104 | ai:claude
            "doc" | "documentation" => RequirementType::Doc,
            "faq" => RequirementType::Faq,
            _ => anyhow::bail!("Invalid type '{}'. Use: functional, non-functional, system, user, change-request, bug, epic, story, task, spike, sprint, folder, meta, principle, vision, constraint, decision, term, doc, faq", type_str),
        };
        if new_type != req.req_type {
            changes.push(Requirement::field_change(
                "type",
                format!("{:?}", req.req_type),
                format!("{:?}", new_type),
            ));
            req.req_type = new_type;
        }
    }

    // Update owner
    if let Some(new_owner) = owner {
        if new_owner != &req.owner {
            changes.push(Requirement::field_change(
                "owner",
                req.owner.clone(),
                new_owner.clone(),
            ));
            req.owner = new_owner.clone();
        }
    }

    // Update feature
    if let Some(new_feature) = feature {
        if new_feature != &req.feature {
            changes.push(Requirement::field_change(
                "feature",
                req.feature.clone(),
                new_feature.clone(),
            ));
            req.feature = new_feature.clone();
        }
    }

    // Update tags
    if let Some(tags_str) = tags {
        // BUG-1770: this is the SECOND edit backend the bug's acceptance names,
        // and it split the argument itself rather than sharing a parser — so
        // the whitespace rule has to be routed through `parse_tag_list` here
        // too or `aida edit --tags "a b"` stays reachable from this path.
        // trace:BUG-1770 | ai:claude
        let new_tags: HashSet<String> = parse_tag_list(tags_str)?.into_iter().collect();
        let old_tags: String = req.tags.iter().cloned().collect::<Vec<_>>().join(", ");
        let new_tags_str: String = new_tags.iter().cloned().collect::<Vec<_>>().join(", ");
        if new_tags != req.tags {
            changes.push(Requirement::field_change("tags", old_tags, new_tags_str));
            req.tags = new_tags;
        }
    }

    // TASK-351: partial tag edits — additive / subtractive variants that
    // don't clobber the rest. `--tags` and `--add-tag`/`--remove-tag` are
    // mutually exclusive at the clap layer, so at most one of the two
    // blocks fires.
    // trace:TASK-351 | ai:claude
    // BUG-1770: keep the REPORT here too. This backend discarded the bool and
    // then fell through to "No changes made to <ID>" — the same conflation of
    // "nothing was asked" with "a tag flag matched nothing" that AC3 narrows on
    // the canonical backend. trace:BUG-1770 | ai:claude
    let mut tag_report = TagDeltaReport::default();
    if !add_tag.is_empty() || !remove_tag.is_empty() {
        let old_tags: String = req.tags.iter().cloned().collect::<Vec<_>>().join(", ");
        tag_report = apply_tag_deltas_report(&mut req.tags, add_tag, remove_tag)?;
        if tag_report.changed() {
            let new_tags_str: String = req.tags.iter().cloned().collect::<Vec<_>>().join(", ");
            changes.push(Requirement::field_change("tags", old_tags, new_tags_str));
        }
    }

    if changes.is_empty() {
        let tag_lines = tag_report.summary_lines();
        if tag_lines.is_empty() {
            println!("{} No changes made to {}", "!".yellow(), spec_id);
        } else {
            for line in tag_lines {
                println!("{line}");
            }
        }
        return Ok(());
    }

    // BUG-1637: record under the caller's identity (was the literal "CLI"),
    // and the status change through the one shared history helper.
    // trace:BUG-1637 | ai:claude
    record_edit_changes(req, &get_default_author(), &changes);
    // STORY-1649: computed while `req` is still borrowed, printed after the
    // save so the warning never precedes the success line.
    // trace:STORY-1649 | ai:claude
    let completion_warning = if into_completed {
        crate::shipped_linkage::completion_linkage_warning(
            &queue_cmd::requeue_project_root(storage),
            req,
        )
    } else {
        None
    };

    // Save changes
    storage.save(&store)?;
    println!(
        "{} Updated {} ({} field(s) changed)",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        spec_id,
        changes.len()
    );
    if let Some(text) = completion_warning {
        eprintln!("{} {text}", "Warning:".yellow().bold());
    }
    // BUG-1770: say what the tag flags did on the success path too — a caller
    // who removes two tags and mistypes one must not see only "Updated".
    // trace:BUG-1770 | ai:claude
    for line in tag_report.summary_lines() {
        println!("  {line}");
    }

    // TASK-358: triage out of NeedsAttention — clean up any orchestrator-
    // escalated worktree for this spec. The lease's `escalated_to_human`
    // marker (stamped by the `--escalate-blocks` path) is the safety gate:
    // an interactive user session on the same spec, or an
    // `--escalate-defaults` advisor-resume, has the marker absent and is
    // left alone. trace:TASK-358 | ai:claude
    if left_needs_attention {
        if let Ok(project_root) = find_project_root() {
            cleanup_escalated_leases_for_spec(&project_root, &spec_id);
            // BUG-674: close the now-stale open punt-ledger record for a spec
            // resumed out of NeedsAttention. trace:BUG-674 | ai:claude
            let _ = punt::close_open_records(
                &project_root,
                &spec_id,
                punt::RESOLUTION_HUMAN_RESOLVED,
                "resolved",
                None,
                Some("resume"),
            );
        }
    }

    Ok(())
}
