//! `aida changelog` command handlers, lifted out of `main.rs` (SPIKE-78).
//!
//! Pure movement: the whole changelog engine lives in `crate::changelog`; this
//! module just maps the subcommand variant to a `ChangelogOptions` and calls
//! `changelog::run`.
// trace:TASK-299 | ai:claude

use anyhow::Result;

use crate::changelog;
use crate::cli;

/// Dispatch `aida changelog <generate|refresh|preview>`. The whole engine
/// lives in `crate::changelog`; this just maps the subcommand variant to a
/// `ChangelogOptions` and calls `changelog::run`.
pub(crate) fn handle_changelog_command(cmd: &cli::ChangelogCommand) -> Result<()> {
    let project_root = crate::find_project_root().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let opts = match cmd {
        cli::ChangelogCommand::Generate { since, until, out } => changelog::ChangelogOptions {
            window: if since.is_some() || until.is_some() {
                changelog::Window::Range {
                    since: since.clone(),
                    until: until.clone(),
                }
            } else {
                changelog::Window::All
            },
            sink: match out {
                Some(p) => changelog::Sink::File(p.clone()),
                None => changelog::Sink::Stdout,
            },
            released_as: None,
        },
        cli::ChangelogCommand::Refresh { released_as, out } => changelog::ChangelogOptions {
            window: changelog::Window::All,
            sink: match out {
                Some(p) => changelog::Sink::File(p.clone()),
                None => changelog::Sink::File(project_root.join("CHANGELOG.md")),
            },
            released_as: released_as.clone(),
        },
        cli::ChangelogCommand::Preview => changelog::ChangelogOptions {
            window: changelog::Window::Unreleased,
            sink: changelog::Sink::Stdout,
            released_as: None,
        },
    };
    changelog::run(opts, &project_root)
}
