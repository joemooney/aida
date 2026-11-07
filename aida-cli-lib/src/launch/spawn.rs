//! The constructor for agent child processes. [`spawn_agent`] turns a
//! [`LaunchContext`] into a `Command` with its containment markers set, and
//! [`assign_seat`] is the one writer of the child's seat keys.
//!
//! Under posture Aida every launch reaches the ONE private boundary function
//! [`aida_boundary`]. In slice A it keeps today's Claude-native boundary
//! byte-identical (the contained mailbox and the containment markers; the
//! generated `--settings` and the os_wrap program already sit in the argv the
//! site composed). STORY-2238 PR3 replaces its body with `containment::launch`.
// trace:STORY-2326 | ai:claude

use std::process::Command;

use aida_core::launch_policy::{
    LaunchKind, AGENT_CMD_ENV, CONTAINED_ENV, GRANT_ENV, LANE_MARKER_ENV, POSTURE_ENV, PURPOSE_ENV,
    ROLE_ENV, STANDALONE_MARKER_ENV,
};
use anyhow::Result;

use super::context::LaunchContext;
use crate::session::ContainmentPosture;

#[cfg(test)]
thread_local! {
    /// How many Aida-posture spawns reached [`aida_boundary`] on this thread.
    pub(crate) static AIDA_BOUNDARY_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Build the child `Command` for `ctx`: program, argv, working directory and
/// containment markers. The caller adds its own launch env and stdio, then
/// assigns the seat with [`assign_seat`].
pub(crate) fn spawn_agent(ctx: &LaunchContext) -> Result<Command> {
    let mut cmd = Command::new(ctx.program());
    // Claude is the first vendor cutover. The explicit uncontained fixture
    // override keeps integration harness inputs, never contained launches.
    // trace:STORY-2326 | ai:codex
    let profile = super::profile::VendorProfile::for_vendor(ctx.vendor());
    if profile.clears_environment() && !ctx.fixture_override() {
        let parent = aida_core::exec_context::Environment::capture();
        let env = profile.child_env(ctx.kind(), &parent, None)?;
        cmd.env_clear().envs(env.pairs());
    }
    cmd.args(ctx.argv());
    if ctx.sets_cwd() {
        cmd.current_dir(ctx.cwd());
    }
    match ctx.posture() {
        ContainmentPosture::Aida => aida_boundary(ctx, &mut cmd)?,
        posture => {
            cmd.env_remove(CONTAINED_ENV);
            cmd.env(POSTURE_ENV, posture_value(posture));
        }
    }
    Ok(cmd)
}

/// The single AIDA-containment boundary every Aida-posture spawn goes through.
fn aida_boundary(ctx: &LaunchContext, cmd: &mut Command) -> Result<()> {
    #[cfg(test)]
    AIDA_BOUNDARY_CALLS.with(|calls| calls.set(calls.get() + 1));
    // trace:BUG-2218 TASK-2328 | ai:codex
    crate::session::prepare_contained_mailbox_for_launch(ctx.host_main(), ctx.cwd())?;
    // trace:TASK-2032 | ai:claude
    cmd.env(CONTAINED_ENV, "1");
    // trace:TASK-2631 | ai:codex — do not inherit a native parent's display marker.
    cmd.env(POSTURE_ENV, "aida");
    // The program override never crosses the boundary: a contained child
    // that carries one refuses its own launches (`LaunchRefusal`).
    cmd.env_remove(AGENT_CMD_ENV);
    Ok(())
}

/// The `AIDA_CONTAINMENT_POSTURE` value of `posture`.
pub(crate) fn posture_value(posture: ContainmentPosture) -> &'static str {
    match posture {
        ContainmentPosture::Aida => "aida",
        ContainmentPosture::Native => "native",
        ContainmentPosture::None => "none",
    }
}

/// The seat a child runs as. It is never inferred from the environment the
/// child would otherwise inherit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeatDecision<'a> {
    /// A new seat session (headless phase, reviewer, coordinator, advisor):
    /// the role and grant selector are set explicitly (`""` is a baseline
    /// child), and the launcher's own session markers (purpose, standalone
    /// implementer, lane) are removed (BUG-2139, STORY-1662).
    Child { role: &'a str, grant: &'a str },
    /// An `aida agent new` lane: the role and grant are set when given and
    /// removed otherwise, and the launcher's purpose is removed.
    Lane {
        role: Option<&'a str>,
        grant: Option<&'a str>,
    },
    /// The child is this process's own session becoming the vendor
    /// (interactive exec or run): its seat passes unchanged, except that a
    /// given grant replaces the selector.
    ThisSession { grant: Option<&'a str> },
}

/// The seat keys a `--bg` hand-off carries none of.
const NO_SEAT: SeatDecision<'static> = SeatDecision::Lane {
    role: None,
    grant: None,
};

/// Assign `seat` to `cmd`, the command [`spawn_agent`] built for `kind`.
/// A `--bg` hand-off carries no seat at all: the daemon-run session resolves
/// its seat from its lane lease.
pub(crate) fn assign_seat(
    cmd: &mut Command,
    kind: LaunchKind,
    seat: SeatDecision<'_>,
) -> Result<()> {
    if kind == LaunchKind::Background && seat != NO_SEAT {
        anyhow::bail!(
            "a background hand-off cannot carry a seat; the session takes its seat from its lane lease"
        );
    }
    match seat {
        SeatDecision::Child { role, grant } => {
            cmd.env(ROLE_ENV, role)
                .env(GRANT_ENV, grant)
                .env_remove(PURPOSE_ENV)
                .env_remove(STANDALONE_MARKER_ENV)
                .env_remove(LANE_MARKER_ENV);
        }
        SeatDecision::Lane { role, grant } => {
            match role {
                Some(role) => cmd.env(ROLE_ENV, role),
                None => cmd.env_remove(ROLE_ENV),
            };
            match grant {
                Some(grant) => cmd.env(GRANT_ENV, grant),
                None => cmd.env_remove(GRANT_ENV),
            };
            cmd.env_remove(PURPOSE_ENV);
        }
        SeatDecision::ThisSession { grant } => {
            // Exec retains this validated session's selectors after env_clear;
            // new child and lane decisions above always replace them.
            // trace:STORY-2326 | ai:codex
            if super::profile::environment_is_cleared(cmd) {
                let parent = aida_core::exec_context::Environment::capture();
                for key in [
                    ROLE_ENV,
                    GRANT_ENV,
                    PURPOSE_ENV,
                    STANDALONE_MARKER_ENV,
                    LANE_MARKER_ENV,
                ] {
                    if let Some(value) = parent.get(key) {
                        cmd.env(key, value);
                    }
                }
            }
            if let Some(grant) = grant {
                cmd.env(GRANT_ENV, grant);
            }
        }
    }
    Ok(())
}

/// Replace this process with `cmd` (spawn + wait where exec is unavailable).
// trace:BUG-2157 | ai:claude
// trace:STORY-2326 | ai:codex
pub(crate) fn exec_agent(cmd: &mut std::process::Command) -> Result<()> {
    // BUG-2386: a Codex resume execs through here too; name the real program.
    let program = cmd.get_program().to_string_lossy().into_owned();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        anyhow::bail!("failed to exec {program}: {}", err);
    }
    #[cfg(not(unix))]
    {
        let status = cmd
            .status()
            .with_context(|| format!("failed to spawn {program}"))?;
        std::process::exit(status.code().unwrap_or(1));
    }
}
