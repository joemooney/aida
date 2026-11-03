//! Per-module command registration. Each command module owns its clap
//! definition, dispatch phase, policy and handler in one `CommandReg`, listed
//! once in [`REGISTRY`]; the legacy `Command` enum in `cli.rs` shrinks one
//! slice at a time until it is gone. `built_command()` is the only way
//! production code may obtain the CLI's clap tree, so every consumer (parse,
//! help, catalog, alias and extension shadowing) sees registered commands.
// trace:STORY-1728 | ai:claude

use anyhow::Result;
use clap::{ArgMatches, CommandFactory, FromArgMatches};

use crate::cli::{Cli, Command, GlobalArgs};
use crate::stakeholder_role::StakeholderAction;

/// Every registered command, sorted by command name (byte order) so parallel
/// additions rarely touch the same line.
// trace:STORY-1728 | ai:claude
pub(crate) static REGISTRY: &[&CommandReg] = &[
    &crate::session_misc_cmd::BG_FETCH,
    &crate::requirements_cli::ADD,
    &crate::adoption_cmd::ADOPTION,
    &crate::advisor_cmd_root::ADVISOR,
    &crate::agent_launch::AGENT,
    &crate::user_alias::ALIAS,
    &crate::approvals_cmd::APPROVALS,
    &crate::archive_cmd::ARCHIVE,
    &crate::assign_cmd::ASSIGN,
    &crate::autonomy_cmd::AUTONOMY,
    &crate::autopilot_intake::AUTOPILOT,
    &crate::autoprogress::AUTOPROGRESS,
    &crate::status_awaiting::AWAITING,
    &crate::presence_cmd::AWAY,
    &crate::backlog::BACKLOG,
    &crate::brief_cmd::BRIEF,
    &crate::burndown_run::BURNDOWN,
    &crate::cache_cmd::CACHE,
    &crate::changelog_cmd::CHANGELOG,
    &crate::claim::CLAIM,
    &crate::help_catalog::COMMANDS,
    &crate::comment_cmd::COMMENT,
    &crate::commit::COMMIT,
    &crate::compete_cmd::COMPETE,
    &crate::config_cmd::CONFIG,
    &crate::monitor_contract::CONTRACT,
    &crate::criteria::CRITERIA,
    &crate::db_cmd::DB,
    &crate::decide_cmd::DECIDE,
    &crate::defer_cmd::DEFER,
    &crate::requirements_cli::DEL,
    &crate::deps_cmd::DEPS,
    &crate::deputy_cmd::DEPUTY,
    &crate::derisk_cmd::DERISK,
    &crate::dev_cmd::DEV,
    &crate::digest_cmd::DIGEST,
    &crate::dispatch::DISPATCH,
    &crate::do_dispatch::DO,
    &crate::doc_cmd::DOC,
    &crate::doc_cmd::DOCS,
    &crate::doctor_cmd::DOCTOR,
    &crate::punt_done::DONE,
    &crate::drain_cmd::DRAIN,
    &crate::driver_cmd::DRIVER,
    &crate::requirements_cli::EDIT,
    &crate::exposition::EXPLAIN,
    &crate::import_export_cmd::EXPORT,
    &crate::leases::FASTTRACK,
    &crate::feature_cmd::FEATURE,
    &crate::store_push_pull::FETCH,
    &crate::field_study_cmd::FIELD_STUDY,
    &crate::findings_gate::FINDINGS,
    &crate::mandate_drive::FLEET,
    &crate::focus_cmd::FOCUS,
    &crate::gate_cmd::GATE,
    &crate::tracker_cmd::GITHUB,
    &crate::tracker_cmd::GITLAB,
    &crate::goal_cmd::GOAL,
    &crate::graph_cmd::GRAPH,
    &crate::requirements_cli::GREP,
    &crate::autopilot_intake::GROOM,
    &crate::harvest::HARVEST,
    &crate::queue_misc::HEADLESS,
    &crate::health_vitals_cmd::HEALTH,
    &crate::help::HELP_ALL,
    &crate::history::HISTORY,
    &crate::presence_cmd::HOME,
    &crate::human_cmd::HUMAN,
    &crate::identity_gate::IDENTITY,
    &crate::import_export_cmd::IMPORT,
    &crate::import_plan_cmd::IMPORT_PLAN,
    &crate::init_cmd::INIT,
    &crate::ps_integrate::INTEGRATE,
    &crate::plan_intent::INTENT,
    &crate::internal_cmd::INTERNAL,
    &crate::tracker_cmd::JIRA,
    &crate::lane_cmd::LANE,
    &crate::lifecycle_cmd::LIFECYCLE,
    &crate::lint_cmd::LINT,
    &crate::requirements_cli::LIST,
    &crate::load_cmd::LOAD,
    &crate::lock_cmd::LOCK,
    &crate::mailbox_cmd::MAILBOX,
    &crate::mandate::MANDATE,
    &crate::manual::MANUAL,
    &crate::mcp_cmd::MCP,
    &crate::mcp::MCP_SERVE,
    &crate::memories_cmd::MEMORIES,
    &crate::merge_hold_cmd::MERGE_HOLD,
    &crate::merge_hold_cmd::MERGE_LOCK,
    &crate::metrics_cmd::METRICS,
    &crate::drain_gate::NO_HUMAN,
    &crate::node_cmd::NODE,
    &crate::notify::NOTIFY,
    &crate::orchestrator_cmd::ORCHESTRATOR,
    &crate::plan_cmd::PLAN,
    &crate::pr_cmd::PR,
    &crate::presence_cmd::PRESENCE,
    &crate::protocol_cmd::PROTOCOL,
    &crate::ps_integrate::PS,
    &crate::store_push_pull::PULL,
    &crate::punt_done::PUNT,
    &crate::punt_done::PUNTS,
    &crate::store_push_pull::PUSH,
    &crate::questions::QUESTIONS,
    &crate::queue_cmd::QUEUE,
    &crate::rebase_cmd::REBASE,
    &crate::reconstitute::RECONSTITUTE,
    &crate::record_cmd::RECORD,
    &crate::regen::REGEN,
    &crate::relationship_cmd::REL,
    &crate::rel_def_cmd::REL_DEF,
    &crate::relay_mail::RELAY_MAIL,
    &crate::upgrade_release::RELEASE,
    &crate::remote_cmd::REMOTE,
    &crate::report_cmd::REPORT,
    &crate::questions::RESEARCH,
    &crate::review_cmd::REVIEW,
    &crate::queue_cmd::REWORK,
    &crate::role_cmd::ROLE,
    &crate::rules_cmd::RULES,
    &crate::sandbox_cmd::SANDBOX,
    &crate::scaffold_cmd::SCAFFOLD,
    &crate::maintenance_schedule::SCHEDULE,
    &crate::schema::SCHEMA,
    &crate::requirements_cli::SEARCH,
    &crate::server_cmd::SERVER,
    &crate::agent_launch::SESSION,
    &crate::shift::SHIFT,
    &crate::pr_cmd::SHIP,
    &crate::requirements_cli::SHOW,
    &crate::skill_cmd::SKILL,
    &crate::solo_cmd::SOLO,
    &crate::plan_intent::SPEC,
    &crate::queue_misc::STACK,
    &crate::orchestrator_cmd::STATE_SNAPSHOT,
    &crate::status_cmd::STATUS,
    &crate::statusbar_cmd::STATUSBAR,
    &crate::statusline_cmd::STATUSLINE,
    &crate::store_cmd::STORE,
    &crate::supervise_cmd::SUPERVISE,
    &crate::tail_cmd::TAIL,
    &crate::team_cmd::TEAM,
    &crate::terminal_cmd::TERMINAL,
    &crate::trace_cmd::TRACE,
    &crate::triage_cmd::TRIAGE,
    &crate::tui_entry::TUI,
    &crate::type_cmd::TYPE,
    &crate::ultraplan_cmd::ULTRAPLAN,
    &crate::archive_cmd::UNARCHIVE,
    &crate::assign_cmd::UNASSIGN,
    &crate::claim::UNCLAIM,
    &crate::defer_cmd::UNDEFER,
    &crate::upgrade_cmd::UPGRADE,
    &crate::usage_cmd::USAGE,
    &crate::help::USER_GUIDE,
    &crate::verify_cmd::VERIFY,
    &crate::watch::WATCH,
    &crate::session_misc_cmd::WHOAMI,
    &crate::plan_intent::WHY,
    &crate::wiki::WIKI,
    &crate::queue_misc::WORKER,
    &crate::worktree_gc::WORKTREE,
    &crate::zen_cmd::ZEN,
];

/// How a registered command runs.
// trace:STORY-1728 | ai:claude
#[derive(Clone, Copy)]
pub(crate) enum Handler {
    /// Runs once, before project discovery and storage init, alongside the
    /// other store-less handlers, and always finishes the invocation there.
    Early(fn(&ArgMatches, &DispatchCtx) -> Result<()>),
    /// Runs first at the early point with no store (`ctx.store` is `None`),
    /// where it may finish (`Flow::Return`) or defer (`Flow::Continue`); then
    /// once more inside whichever storage dispatcher serves the project.
    /// There, `Flow::Return` returns from the dispatcher (skipping its
    /// post-dispatch hooks, as a legacy arm's `return` did) and
    /// `Flow::Continue` falls through to them.
    Store(fn(&ArgMatches, &DispatchCtx) -> Result<Flow>),
}

/// Whether a store-phase handler finished the invocation.
// trace:STORY-1728 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    Return,
    Continue,
}

/// The option that replaced a command's former local `--format`, for the
/// commands whose own format option has another name; `value` is the global
/// `--format` value given.
// trace:BUG-2189 | ai:claude
fn renamed_format_hint(command: &str, value: &str) -> Option<String> {
    match command {
        "export" => {
            let value = if value == "json" {
                value
            } else {
                "<mapping|json|spec|impl|tree>"
            };
            Some(format!(
                "For the export format, use --export-format {value} (or -f {value})."
            ))
        }
        "report ai-integration" => {
            Some("For the report format, use --report-format markdown|html (or -f).".into())
        }
        "config format" => Some(
            "Pass the ID format as a positional argument: aida config format single|two.".into(),
        ),
        _ => None,
    }
}

/// The error for an explicit global `--format <value>` on `aida <command>`
/// when that command's own format option has another name; `None` otherwise.
// trace:BUG-2189 | ai:claude
pub(crate) fn renamed_format_error(command: &str, value: &str) -> Option<anyhow::Error> {
    let hint = renamed_format_hint(command, value)?;
    Some(anyhow::anyhow!(
        "--format {value} sets the CLI output format (human/toon/json), which `aida {command}` \
         does not use. {hint}"
    ))
}

/// Refuse an explicit global `--format` on a command whose own format option
/// has another name, so the former local `--format` spelling fails loudly
/// instead of silently producing the default format.
// trace:BUG-2189 | ai:claude
pub(crate) fn refuse_global_format(ctx: &DispatchCtx, command: &str) -> Result<()> {
    let Some(format) = ctx.globals.and_then(|globals| globals.format) else {
        return Ok(());
    };
    let value = clap::ValueEnum::to_possible_value(&format)
        .map(|value| value.get_name().to_string())
        .unwrap_or_default();
    match renamed_format_error(command, &value) {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// A `return <Result<()>>` from a legacy dispatcher arm, as a `Flow`.
// trace:STORY-1728 | ai:claude
pub(crate) fn returned(result: Result<()>) -> Result<Flow> {
    result.map(|()| Flow::Return)
}

/// The open store a store-phase handler runs against.
// trace:STORY-1728 | ai:claude
pub(crate) enum StoreCtx<'a> {
    /// The git-canonical store, from `handle_git_backend_command`.
    Git {
        store_path: &'a std::path::Path,
        backend: &'a aida_core::CachedGitBackend,
    },
    /// The git-canonical store before its backend is opened, from
    /// `run_on_distributed_store`. Handlers that need no cached backend
    /// (regen, mcp-serve, the issue trackers) finish here; everything else
    /// returns `Flow::Continue` and reaches the git-backend dispatcher.
    Distributed {
        store_path: &'a std::path::Path,
        project_root_hint: Option<&'a std::path::Path>,
    },
    /// The deprecated centralized store, from the legacy dispatcher in `run()`.
    Legacy {
        storage: &'a crate::Storage,
        requirements_path: &'a std::path::Path,
    },
}

/// The per-command answers the cross-cutting gates in `run()` need. Every
/// field is required and there is no `Default`, so an unclassified command
/// does not compile.
// trace:STORY-1728 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CmdPolicy {
    /// What a guest/requester stakeholder role is attempting.
    pub stakeholder: StakeholderAction,
    /// Whether the command writes the store and so triggers per-write
    /// auto-push.
    pub per_write_auto_push: bool,
    /// Whether the interactive last-role restore prompt is skipped.
    pub skip_role_restore_prompt: bool,
    /// Whether the team-identity guard treats the command as a shared-store
    /// write (refused under a shared `default` identity when required).
    pub team_identity_write: bool,
}

/// What a registered handler receives besides its own `ArgMatches`.
// trace:STORY-1728 | ai:claude
pub(crate) struct DispatchCtx<'a> {
    /// The top-level options; `None` inside the git-backend dispatcher, which
    /// has never received them.
    pub globals: Option<&'a GlobalArgs>,
    /// The open store; `None` at the early dispatch point.
    pub store: Option<StoreCtx<'a>>,
}

/// One command's complete registration.
// trace:STORY-1728 | ai:claude
pub(crate) struct CommandReg {
    /// The subcommand name; must equal the name `command()` builds.
    pub name: &'static str,
    /// The top-level command this one follows in help and the catalog; `""`
    /// for the first.
    pub help_after: &'static str,
    /// The command's clap definition (usually `<Args as CommandFactory>::command`).
    pub command: fn() -> clap::Command,
    pub policy: fn(&ArgMatches) -> CmdPolicy,
    pub run: Handler,
}

impl std::fmt::Debug for CommandReg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandReg")
            .field("name", &self.name)
            .finish()
    }
}

/// A parsed registered command: its registration and its own sub-matches.
// trace:STORY-1728 | ai:claude
#[derive(Debug)]
pub(crate) struct Invocation {
    pub reg: &'static CommandReg,
    pub matches: ArgMatches,
}

impl Invocation {
    pub(crate) fn policy(&self) -> CmdPolicy {
        (self.reg.policy)(&self.matches)
    }

    /// The early dispatch point, before storage init.
    pub(crate) fn run_early(&self, globals: &GlobalArgs) -> Result<Flow> {
        let ctx = DispatchCtx {
            globals: Some(globals),
            store: None,
        };
        // Early handlers resolve spec IDs before the store dispatch installs
        // the node-alias map, so install it here; the per-prompt status
        // renderers skip it to stay on their latency budget.
        // trace:BUG-B6B1278890774EF993D2EBC327862470-015 | ai:claude
        if matches!(self.reg.run, Handler::Early(_))
            && !matches!(self.reg.name, "statusline" | "statusbar")
        {
            crate::spec_arg::ensure_alias_map();
        }
        match self.reg.run {
            Handler::Early(run) => run(&self.matches, &ctx).map(|()| Flow::Return),
            Handler::Store(run) => run(&self.matches, &ctx),
        }
    }

    /// A storage dispatcher. Early handlers always finish before storage
    /// init, so only store-phase handlers reach here.
    pub(crate) fn run_store(&self, store: StoreCtx, globals: Option<&GlobalArgs>) -> Result<Flow> {
        let ctx = DispatchCtx {
            globals,
            store: Some(store),
        };
        match self.reg.run {
            Handler::Early(_) => unreachable!("`{}` finished before storage init", self.reg.name),
            Handler::Store(run) => run(&self.matches, &ctx),
        }
    }
}

pub(crate) fn lookup(name: &str) -> Option<&'static CommandReg> {
    REGISTRY.iter().copied().find(|reg| reg.name == name)
}

/// The full CLI clap tree: the legacy derived `Cli` plus every registered
/// command, each placed after its `help_after` anchor.
///
/// BUG-2221: built once per process into [`PRISTINE_COMMAND`] and cloned per
/// caller. `aida show <id> --format json` used to construct this tree up to
/// five times before dispatch (alias guarding, extension shadowing, the parse,
/// the JSON capability gate, and the capability reparse); each construction
/// re-derives the legacy `Cli`, instantiates every registered command, and
/// re-sorts help order. The tree is immutable per process (the registry is
/// static), so one build is semantically identical — callers that mutate
/// (parsing) mutate their own clone. trace:BUG-2221 | ai:claude
// trace:STORY-1728 | ai:claude
pub(crate) fn built_command() -> clap::Command {
    PRISTINE_COMMAND.get_or_init(build_pristine_command).clone()
}

/// BUG-2221: the one pristine tree per process. trace:BUG-2221 | ai:claude
static PRISTINE_COMMAND: std::sync::OnceLock<clap::Command> = std::sync::OnceLock::new();

/// BUG-2221: how many times the tree was actually CONSTRUCTED (not cloned) in
/// this process — the structural regression surface for the one-pristine-tree
/// guarantee. trace:BUG-2221 | ai:claude
static PRISTINE_BUILDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// BUG-2221: observed pristine-tree construction count (tests only assert it
/// stops growing; they never assume a starting value, since any earlier call
/// in the process initializes the tree). trace:BUG-2221 | ai:claude
pub(crate) fn pristine_tree_builds() -> usize {
    PRISTINE_BUILDS.load(std::sync::atomic::Ordering::SeqCst)
}

fn build_pristine_command() -> clap::Command {
    PRISTINE_BUILDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut root = Cli::command();
    let legacy: Vec<String> = root
        .get_subcommands()
        .map(|sub| sub.get_name().to_string())
        .collect();
    for reg in REGISTRY {
        root = root.subcommand((reg.command)());
    }
    let legacy: Vec<&str> = legacy.iter().map(String::as_str).collect();
    let order = help_order(&legacy, REGISTRY).unwrap_or_else(|err| panic!("{err}"));
    // `mut_subcommand` re-appends, so this also fixes iteration order.
    for (at, name) in order.iter().enumerate() {
        root = root.mut_subcommand(name, |sub| sub.display_order(at));
    }
    root
}

/// Top-level display order: the legacy commands in their own order, each
/// registered command placed right after its `help_after` anchor. Anchors may
/// themselves be registered, so entries are placed in rounds until none moves;
/// `help_after: ""` places an entry first, and entries sharing an anchor land
/// in name order whatever the input order. An
/// anchor that never resolves (unknown, or a cycle among registered entries)
/// is an error naming every stuck entry, never a silent append.
// trace:STORY-1728 | ai:claude
pub(crate) fn help_order<'a>(
    legacy: &[&'a str],
    regs: &[&'a CommandReg],
) -> Result<Vec<&'a str>, String> {
    let mut order: Vec<&'a str> = legacy.to_vec();
    // Inserting each sibling straight after the anchor reverses insertion
    // order, so place them in reverse name order.
    let mut pending: Vec<&CommandReg> = regs.to_vec();
    pending.sort_by(|a, b| b.name.cmp(a.name));
    while !pending.is_empty() {
        let before = pending.len();
        pending.retain(|reg| {
            let at = if reg.help_after.is_empty() {
                Some(0)
            } else {
                order
                    .iter()
                    .position(|name| *name == reg.help_after)
                    .map(|at| at + 1)
            };
            match at {
                Some(at) => {
                    order.insert(at, reg.name);
                    false
                }
                None => true,
            }
        });
        if pending.len() == before {
            let mut stuck: Vec<String> = pending
                .iter()
                .map(|reg| format!("`{}` (help_after `{}`)", reg.name, reg.help_after))
                .collect();
            stuck.sort();
            return Err(format!(
                "registered commands with an unknown or cyclic help_after anchor: {}",
                stuck.join(", ")
            ));
        }
    }
    Ok(order)
}

/// Parse argv against [`built_command`]; clap's own error on failure.
// trace:STORY-1728 | ai:claude
pub(crate) fn try_parse_from<I, T>(argv: I) -> Result<Cli, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let mut root = built_command();
    let mut matches = root.try_get_matches_from_mut(argv)?;
    cli_from_matches(&mut matches).map_err(|err| err.format(&mut root))
}

/// [`try_parse_from`], exiting with clap's message and code on error (the
/// `Parser::parse_from` contract).
// trace:STORY-1728 | ai:claude
pub(crate) fn parse_from<I, T>(argv: I) -> Cli
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    try_parse_from(argv).unwrap_or_else(|err| err.exit())
}

fn cli_from_matches(matches: &mut ArgMatches) -> Result<Cli, clap::Error> {
    let Some(reg) = matches.subcommand_name().and_then(lookup) else {
        return Cli::from_arg_matches_mut(matches);
    };
    let globals = GlobalArgs::from_arg_matches_mut(matches)?;
    let (_, sub) = matches
        .remove_subcommand()
        .expect("subcommand_name returned a name");
    Ok(Cli {
        globals,
        command: Command::Registered(Invocation { reg, matches: sub }),
    })
}

impl Command {
    /// The registered command's name (`"show"`, `"queue"`, ...).
    pub(crate) fn name(&self) -> &'static str {
        let Command::Registered(invocation) = self;
        invocation.reg.name
    }

    /// The registered command's own subcommand (`"add"` for `rel add`).
    // trace:BUG-HOME-041 | ai:claude
    pub(crate) fn subcommand_name(&self) -> Option<&str> {
        let Command::Registered(invocation) = self;
        invocation.matches.subcommand_name()
    }

    /// The command's typed args when it is the command `T` describes, for
    /// the few cross-cutting hooks in `run()` that key on one command's flags.
    pub(crate) fn args<T: FromArgMatches + CommandFactory>(&self) -> Option<T> {
        let Command::Registered(invocation) = self;
        (T::command().get_name() == invocation.reg.name)
            .then(|| T::from_arg_matches(&invocation.matches).ok())
            .flatten()
    }
}

/// Tests build a command the way a user types it.
// trace:STORY-1728 | ai:claude
#[cfg(test)]
pub(crate) fn parse_command<I, T>(argv: I) -> Command
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    try_parse_from(argv)
        .unwrap_or_else(|err| panic!("test argv does not parse: {err}"))
        .command
}

/// Tests that inspect a parsed registered command read its typed args here.
// trace:STORY-1728 | ai:claude
#[cfg(test)]
impl Command {
    pub(crate) fn registered<T: FromArgMatches>(&self) -> T {
        match self {
            Command::Registered(invocation) => T::from_arg_matches(&invocation.matches)
                .unwrap_or_else(|err| panic!("`{}` args: {err}", invocation.reg.name)),
            other => panic!("expected a registered command, got {other:?}"),
        }
    }
}

#[cfg(test)]
#[path = "tests/story_1728_command_registry_tests.rs"]
mod story_1728_command_registry_tests;

#[cfg(test)]
#[path = "tests/story_1728_policy_parity_tests.rs"]
mod story_1728_policy_parity_tests;

#[cfg(test)]
#[path = "tests/bug_2189_format_collision_tests.rs"]
mod bug_2189_format_collision_tests;

#[cfg(test)]
#[path = "tests/bug_2221_pristine_tree_tests.rs"]
mod bug_2221_pristine_tree_tests;
