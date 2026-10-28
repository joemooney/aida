//! `aida relay-mail` clap types, owned by `relay_mail.rs`.
// trace:STORY-1843 | ai:claude

use clap::{Subcommand, ValueEnum};
use std::path::PathBuf;

/// Relay mail lifetime: one recipient per relay run, across its sessions.
///
/// A relay controller binds its label once at start, binds each child
/// session it launches to the same run, and records the run's end. Mail sent
/// to the label between sessions is accepted while the controller is alive.
/// These commands are the relay's plumbing; operators rarely run them.
#[derive(clap::Parser, Debug)]
#[clap(name = "relay-mail")]
pub(crate) struct RelayMailArgs {
    #[clap(subcommand)]
    pub inner: RelayMailCommand,
}

#[derive(Subcommand, Debug)]
pub enum RelayMailCommand {
    /// Print the relay mail protocol this binary speaks. A relay refuses to
    /// advertise its label unless the protocol matches the one it needs.
    Probe {
        /// Print JSON.
        #[clap(long)]
        json: bool,
    },

    /// Reserve the relay label for a new run and print the run id. Run by
    /// the relay controller before it launches any session.
    Bind {
        /// The relay label other agents address mail to.
        #[clap(long)]
        label: String,
        /// The relay controller's process id.
        #[clap(long)]
        controller_pid: u32,
        /// The relay's run lock file. The controller must hold it.
        #[clap(long)]
        run_lock: PathBuf,
        /// The relay mail protocol the caller requires (see `probe`).
        #[clap(long)]
        protocol: String,
        /// Print JSON.
        #[clap(long)]
        json: bool,
    },

    /// Bind the next child session to an existing run. With
    /// `--native-session`, that session is bound now; without it, a launch
    /// token is printed for the child to present at its own startup.
    ChildBind {
        /// The run id `bind` printed.
        #[clap(long)]
        run: String,
        /// The child's native client session id, when known before launch.
        #[clap(long)]
        native_session: Option<String>,
        /// Print JSON.
        #[clap(long)]
        json: bool,
    },

    /// Record the run's end once. Mail to the label is refused afterwards.
    End {
        /// The run id `bind` printed.
        #[clap(long)]
        run: String,
        /// The relay controller's process id, as given to `bind`.
        #[clap(long)]
        controller_pid: u32,
        /// Why the run ended.
        #[clap(long, value_enum, default_value_t = EndReason::Stop)]
        reason: EndReason,
        /// Print JSON.
        #[clap(long)]
        json: bool,
    },
}

/// Why a relay run ended, as its controller reports it. A controller that
/// died without reporting is recorded by reconciliation as `crash`.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// Stopped by the operator or a signal.
    Stop,
    /// The relay finished and exited.
    Exit,
    /// The relay stopped waiting on a human.
    Blocked,
}
