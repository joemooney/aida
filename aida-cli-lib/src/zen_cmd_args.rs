//! `aida zen_cmd` clap types, owned by `zen_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Zen-mode introspection. `AIDA_ZEN=1` enables zen mode but is
/// inherited by every child process — a leaked / stale value is unverifiable
/// on its own. These subcommands let a session — or a skill running inside one
/// — verify whether zen mode is *genuinely* in effect rather than trusting the
/// bare env var.
///
/// Every variant is `hide = true`: these are skill / orchestrator introspection
/// hooks, not daily-driver verbs, so they stay OFF the `aida zen --help` top
/// line (which leads with `aida zen <SPEC>` / `aida zen "<thought>"`). They
/// still parse + run.
// trace:STORY-725 | ai:claude — plain `//` keeps the marker out of `--help`.
// trace:BUG-237 | ai:claude
// trace:TASK-487 | ai:claude
#[derive(Subcommand, Debug)]
pub enum ZenCommand {
    /// Print the corroborated zen context of the current process: `zen` when
    /// `AIDA_ZEN=1` is set AND corroborated (a live `--auto-complete --zen`
    /// orchestrator run, or the session's own `--zen` lease token), else
    /// `interactive`. Skills branch their zen behavior off this word instead
    /// of reading the bare `$AIDA_ZEN` env var, so a leaked `AIDA_ZEN=1` never
    /// silently auto-resolves a confirmation prompt.
    #[clap(hide = true)]
    Status {
        /// Emit `{"context","corroborated","reason"}` JSON instead of the
        /// bare status word.
        #[clap(long)]
        json: bool,
    },
    /// The standalone-`--zen` finish decision. Prints `auto-exit` when this
    /// zen session reached a clean finish — no human ever in the loop, no open
    /// punt, no `--pause-always` — so the `/aida-pr` checkpoint runs
    /// `aida session end` itself and exits; prints `pause` otherwise so it
    /// renders the grab-next/stop table. The gate the skill consults instead
    /// of always pausing.
    // trace:STORY-564 | ai:claude — plain `//` keeps the marker out of `--help`.
    #[clap(hide = true)]
    Finish {
        /// Emit `{"decision","reason","corroborated"}` JSON instead of the
        /// bare decision word.
        #[clap(long)]
        json: bool,
    },
    /// Mark the current `--zen` session as having needed a human — call it the
    /// moment the session pauses on a design-fork for the standby advisor or
    /// raises a punt. Its presence makes `aida zen finish` pause at the
    /// grab-next/stop checkpoint rather than auto-exiting.
    // trace:STORY-564 | ai:claude — plain `//` keeps the marker out of `--help`.
    #[clap(hide = true)]
    NeedsHuman {
        /// One line on why a human was needed (for later triage). Only the
        /// marker's presence drives the gate; the reason is recorded alongside.
        // trace:BUG-1294 | ai:claude
        #[clap(long, value_name = "TEXT", allow_hyphen_values = true)]
        reason: String,
    },
}
