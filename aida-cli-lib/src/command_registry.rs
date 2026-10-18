//! Per-module command registration. Each command module owns its clap
//! definition, dispatch phase, policy and handler in one `CommandReg`, listed
//! once in [`REGISTRY`]; the legacy `Command` enum in `cli.rs` shrinks one
//! slice at a time until it is gone. `built_command()` is the only way
//! production code may obtain the CLI's clap tree, so every consumer (parse,
//! help, catalog, alias and extension shadowing) sees registered commands.
// trace:STORY-1728 | ai:claude

use clap::CommandFactory;

use crate::cli::Cli;

/// The full CLI clap tree: the legacy derived `Cli` plus every registered
/// command.
// trace:STORY-1728 | ai:claude
pub(crate) fn built_command() -> clap::Command {
    Cli::command()
}

#[cfg(test)]
#[path = "tests/story_1728_command_registry_tests.rs"]
mod story_1728_command_registry_tests;
