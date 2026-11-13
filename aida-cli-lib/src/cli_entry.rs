// trace:TASK-3428 | ai:codex
use crate::{
    agent_error_block, agent_output_mode, agent_output_mode_quiet, attached_store_present,
    bare_aida_action, bare_aida_picker_argv, branch_exists_anywhere, build_banner, build_sha_short,
    builtin_alias_rewrites, cache_cmd, cache_output, check_migration_status,
    cwd_in_initialized_aida_project, detect_distributed_store, determine_requirements_path,
    did_you_mean_for_not_found, distributed_mode_declared, enforce_json_format_capability,
    enforce_stakeholder_role_capabilities, exit_code_for_error, export_coordinating_bin_env,
    external_cmd, find_main_worktree_root, find_project_root, handle_bare_agent_status, help,
    help_catalog, maybe_prompt_role_restore, maybe_run_asciinema_wrapper, parse_help_commands_args,
    presence, print_help_all, print_help_topic, print_tiered_help, record_ambiguous_id_refusal,
    record_orchestrated_child_refusal, register_filing_identity, register_source_build,
    run_on_distributed_store, set_output_format_override, try_attach_store_worktree, usage,
    user_alias, warn_if_cwd_removed, BareAidaAction, Colorize, Command, MigrationCheck, Result,
    Storage, ASCIINEMA_WRAPPED_ENV,
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
            let cli = crate::TEST_CLI.try_parse_from(argv).unwrap();
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
    // Resolve the gated lane path before identity, telemetry, aliases, cache
    // setup or any ordinary dispatcher can write. Inputs confer no authority.
    // trace:TASK-3223 | ai:codex
    let environment = aida_core::exec_context::Environment::capture();
    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let entry_cwd = std::env::current_dir().ok();
    if let Some(result) =
        crate::store_mutation_entry::entry(&environment, &argv, entry_cwd.as_deref())
    {
        match result {
            Ok(()) => return,
            Err(error) => {
                eprintln!("Error: {error:#}");
                std::process::exit(1);
            }
        }
    }
    // Local observation data must never enter identity/store/telemetry setup.
    // trace:STORY-2479 | ai:codex
    if let Some(result) = crate::observation_envelope::local_entry() {
        match result {
            Ok(()) => return,
            Err(error) => {
                eprintln!("Error: {error}");
                std::process::exit(1);
            }
        }
    }
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
    let mut refusal_to_stderr = false;
    let exit_code: i32 = match run(&mut refusal_to_stderr) {
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
                // Add refusals must survive stdout filters used to extract new
                // spec IDs. Keep the structured diagnostic, on stderr.
                // trace:BUG-2505 | ai:codex
                if refusal_to_stderr {
                    eprintln!("{}", agent_error_block(&err, &msg));
                } else {
                    println!("{}", agent_error_block(&err, &msg));
                }
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

pub(crate) fn run(refusal_to_stderr: &mut bool) -> Result<()> {
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
    let mut bare_picker = false;
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
        // gets the agent picker inside an AIDA project (TASK-1472) and the
        // getting-started menu elsewhere (the Trojan-horse first impression);
        // an agent (non-TTY / `AIDA_AGENT_OUTPUT`) gets the `aida status`
        // snapshot + top queued instead, since CLAUDE.md names `aida status`
        // as the entry point and a help dump is noise to a coding agent. The
        // explicit `aida help` verb is unchanged in both modes.
        // trace:TASK-970
        // TASK-1472: bare `aida` at an interactive terminal inside an
        // initialized AIDA project is a shortcut for `aida agent new` (the
        // agent picker); agent-output / non-interactive callers keep the
        // TASK-970 status snapshot, and outside a project it stays onboarding.
        // trace:TASK-1472 | ai:claude
        if is_bare {
            let agent_mode = agent_output_mode();
            let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin())
                && std::io::IsTerminal::is_terminal(&std::io::stdout());
            let initialized = !agent_mode && interactive && cwd_in_initialized_aida_project();
            match bare_aida_action(agent_mode, interactive, initialized) {
                BareAidaAction::StatusSnapshot => return handle_bare_agent_status(),
                BareAidaAction::AgentPicker => bare_picker = true,
                BareAidaAction::Onboarding => {}
            }
        }
        if is_help_verb || (is_bare && !bare_picker) {
            let want_all = rest.iter().any(|a| a == "--all" || a == "-a");
            if want_all {
                print_help_all();
            } else {
                print_tiered_help();
            }
            return Ok(());
        }
    }
    // TASK-1472: the picker shortcut flows through the normal pipeline as the
    // literal `aida agent new`, so it is the same picker, flags and all.
    // trace:TASK-1472 | ai:claude
    let raw_args = if bare_picker {
        bare_aida_picker_argv(&raw_args[0])
    } else {
        raw_args
    };

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
    // Use the canonical parsed command, including aliases and global flags,
    // before any format, authority, storage or add validation can refuse.
    // trace:BUG-2505 | ai:codex
    *refusal_to_stderr = cli.command.name() == "add";

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
    if invocation.run_early(&cli.globals)? == crate::cmdkit::Flow::Return {
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
            let store = crate::cmdkit::StoreCtx::Legacy {
                storage: &storage,
                requirements_path: &requirements_path,
            };
            if invocation.run_store(store, Some(&cli.globals))? == crate::cmdkit::Flow::Return {
                return Ok(());
            }
        }
    }

    Ok(())
}
