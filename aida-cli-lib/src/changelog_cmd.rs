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
    // The tag bounds only select among `v*` tags, but refuse a dash-led
    // value anyway so none can ever reach git as an option.
    // trace:BUG-1622 | ai:claude
    if let cli::ChangelogCommand::Generate { since, until, .. } = cmd {
        for (flag, value) in [("--since", since), ("--until", until)] {
            if let Some(v) = value.as_deref() {
                crate::git_arg_guard::reject_option_like(flag, v)?;
            }
        }
    }
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

/// Generate `CHANGELOG.md` mechanically from git tags + the spec
/// graph. Walks `v*` tags as release boundaries, scans commits
/// between them for `(SPEC-ID)` references, classifies each spec
/// (Features / Fixes / Documentation / Infrastructure / Internal /
/// Other), and renders one structured markdown section per release.
/// `generate` prints to stdout or `--out`; `refresh` writes
/// `CHANGELOG.md` (idempotent — same git state → byte-identical
/// output); `preview` is `[Unreleased]`-only.
// trace:TASK-299 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "changelog")]
pub(crate) struct ChangelogArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::ChangelogCommand,
}

// `aida changelog` is self-contained: git + read-only store, no shared
// storage handle. Dispatch alongside Plan/Ultraplan. trace:TASK-299 | ai:claude
fn run_changelog(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    use crate::*;
    let args = <ChangelogArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let cl_cmd = &args.inner;
    return self::handle_changelog_command(cl_cmd);
}

// trace:STORY-1728 | ai:claude
pub(crate) static CHANGELOG: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "changelog",
        help_after: "gate",
        command: <ChangelogArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_changelog),
    };
