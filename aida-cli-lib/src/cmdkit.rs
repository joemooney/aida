// trace:TASK-3429 | ai:codex
//! Shared command registration types and dispatch context.
//! The registry table and CLI parser remain in `command_registry`.
// trace:TASK-3425 | ai:codex

use anyhow::Result;
use clap::ArgMatches;
use std::path::PathBuf;

// Shared vocabulary lives here so framework consumers do not reach the CLI
// parser or stakeholder gate. Existing paths re-export these pure types.
// trace:TASK-3425 | ai:codex

/// Explicit output-format pin. Overrides the default, which is chosen from
/// whether stdout is a terminal: the human table at a TTY, token-efficient
/// TOON when the output is piped or captured. That silent switch broke real
/// user scripts that parsed the table, so `--format` (or `AIDA_OUTPUT_FORMAT`)
/// lets a script pin the format it expects regardless of the TTY.
// trace:STORY-764 | ai:claude
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable tables / emoji (the interactive default at a TTY).
    Human,
    /// Token-efficient TOON — the compact agent default when piped/captured.
    Toon,
    /// Machine JSON, for commands that support a JSON projection.
    Json,
}

// trace:BUG-1197 | ai:codex
// trace:TASK-1264 | ai:codex
/// Shared capability vocabulary for the CLI and MCP stakeholder envelopes.
/// Keeping this decision independent of either transport prevents a new write
/// path from silently widening guest/requester authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StakeholderAction {
    Read,
    Intake,
    EditRequirement,
    Queue,
    Database,
    MergeGate,
    Session,
    Role,
    Relationship,
    Comment,
    Type,
    Trace,
    Finding,
    Question,
    Configuration,
    BuildLoop,
    Write,
}

// The top-level options, shared by both dispatch paths: a registered command
// has no `Command` variant to parse through, so its handler reads these from
// the same `ArgMatches`. No doc comment: on a flattened `Args` struct it would
// replace the root `about`. trace:STORY-1728 | ai:claude
#[derive(clap::Args, Debug)]
pub struct GlobalArgs {
    /// Path to the requirements file (overrides auto-detection)
    #[clap(long)]
    pub file: Option<String>,

    /// Project name to use from central registry
    #[clap(long, short = 'p')]
    pub project: Option<String>,

    /// Connect to a remote AIDA server (e.g., "localhost:50051" or "grpc://host:port")
    /// Can also be set via AIDA_SERVER environment variable
    #[clap(long, short = 's')]
    pub server: Option<String>,

    /// Record this invocation with asciinema when running in a terminal
    #[clap(long)]
    pub asciinema: bool,

    /// Output path for --asciinema recordings
    #[clap(long, value_name = "PATH", requires = "asciinema")]
    pub cast_out: Option<PathBuf>,

    /// Title for --asciinema recordings
    #[clap(long, value_name = "TITLE", requires = "asciinema")]
    pub cast_title: Option<String>,

    /// Print the absolute path to the current aida binary and exit.
    // trace:TASK-1718 | ai:antigravity
    #[clap(long)]
    pub path: bool,

    /// Pin the output format regardless of whether stdout is a terminal:
    /// human = tables, toon = compact agent output, json = machine JSON.
    /// Default (unset) is human at a TTY, toon when piped/captured. Also
    /// settable via AIDA_OUTPUT_FORMAT (the flag wins).
    #[clap(long, global = true, value_enum, value_name = "FORMAT")]
    pub format: Option<OutputFormat>,

    /// Show node-local spec IDs in their canonical form instead of the
    /// node's friendly label (for scripts). Also settable via
    /// AIDA_CANONICAL_IDS=1.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-078 | ai:claude
    #[clap(long, global = true)]
    pub canonical_ids: bool,
}

/// How a registered command runs.
// trace:STORY-1728 | ai:claude
#[derive(Clone, Copy)]
pub enum Handler {
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
pub enum Flow {
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
pub fn returned(result: Result<()>) -> Result<Flow> {
    result.map(|()| Flow::Return)
}

/// The open store a store-phase handler runs against.
// trace:STORY-1728 | ai:claude
pub enum StoreCtx<'a> {
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
pub struct CmdPolicy {
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
pub struct DispatchCtx<'a> {
    /// The top-level options; `None` inside the git-backend dispatcher, which
    /// has never received them.
    pub globals: Option<&'a GlobalArgs>,
    /// The open store; `None` at the early dispatch point.
    pub store: Option<StoreCtx<'a>>,
}

/// One command's complete registration.
// trace:STORY-1728 | ai:claude
pub struct CommandReg {
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
