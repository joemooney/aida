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
    &crate::presence_cmd::AWAY,
    &crate::presence_cmd::HOME,
];

/// Where in `run()` a registered command dispatches: the point at which the
/// legacy handlers it replaces ran, so every hook ahead of that point still
/// applies to it.
// trace:STORY-1728 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Before project discovery and storage init, alongside the other
    /// store-less handlers.
    PreStore,
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
}

/// What a registered handler receives besides its own `ArgMatches`.
// trace:STORY-1728 | ai:claude
pub(crate) struct DispatchCtx<'a> {
    pub globals: &'a GlobalArgs,
}

/// One command's complete registration.
// trace:STORY-1728 | ai:claude
pub(crate) struct CommandReg {
    /// The subcommand name; must equal the name `command()` builds.
    pub name: &'static str,
    /// The top-level command this one follows in help and the catalog.
    pub help_after: &'static str,
    /// The command's clap definition (usually `<Args as CommandFactory>::command`).
    pub command: fn() -> clap::Command,
    pub phase: Phase,
    pub policy: fn(&ArgMatches) -> CmdPolicy,
    pub run: fn(&ArgMatches, &DispatchCtx) -> Result<()>,
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

    pub(crate) fn run(&self, globals: &GlobalArgs) -> Result<()> {
        (self.reg.run)(&self.matches, &DispatchCtx { globals })
    }
}

pub(crate) fn lookup(name: &str) -> Option<&'static CommandReg> {
    REGISTRY.iter().copied().find(|reg| reg.name == name)
}

/// The full CLI clap tree: the legacy derived `Cli` plus every registered
/// command, each placed after its `help_after` anchor.
// trace:STORY-1728 | ai:claude
pub(crate) fn built_command() -> clap::Command {
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
/// entries sharing an anchor land in name order whatever the input order. An
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
        pending.retain(
            |reg| match order.iter().position(|name| *name == reg.help_after) {
                Some(at) => {
                    order.insert(at + 1, reg.name);
                    false
                }
                None => true,
            },
        );
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

#[cfg(test)]
#[path = "tests/story_1728_command_registry_tests.rs"]
mod story_1728_command_registry_tests;
