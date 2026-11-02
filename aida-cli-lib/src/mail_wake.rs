//! Per-vendor mail wake for `aida agent new` sessions.
//!
//! `agent new codex` stamps its launch row `pending`. The Codex thread id
//! first reaches AIDA through the session's own SessionStart hook, so that
//! hook arms the row once it records the exact thread (never `--last` or a
//! guess) and makes sure the project's one multiplexed watcher unit exists.
//! Ending the agent, or the dead-agent GC, retires the row; its ledger stays
//! on disk. Antigravity has no live-injection primitive: its rows read
//! `unsupported`, and its launch warns once that mail arrives on its next
//! turn.
//!
//! A Codex session started any other way (`aida session start` then
//! `codex`) gets the same wake: its SessionStart hook finds a row nothing
//! has decided, records the `[agents.codex] mail_wake` default, and arms it.
// trace:STORY-HOME-096 | ai:claude
// trace:STORY-2256 | ai:claude
use crate::agent_registry::{AgentRegistryEntry, MailWake};
use anyhow::Result;
use std::path::Path;

/// The first Codex release with the native `codex queue` the watcher drives.
pub(crate) const MIN_CODEX_QUEUE: (u64, u64, u64) = (0, 164, 0);

/// What a vendor offers for live mail wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WakeAdapter {
    /// `codex queue` on the session's exact thread.
    Codex,
    /// No live-injection primitive: mail arrives on the next turn.
    Unsupported,
    /// Mail reaches the session through its own per-turn hooks.
    NotApplicable,
}

impl WakeAdapter {
    pub(crate) fn for_vendor(agent_type: &str) -> Self {
        match aida_core::mailbox_identity::normalize_agent_type(agent_type).as_str() {
            "codex" => Self::Codex,
            "antigravity" => Self::Unsupported,
            _ => Self::NotApplicable,
        }
    }
}

/// What the launcher records on a new session's row, and the one warning
/// it prints before the child takes the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchWake {
    pub(crate) state: Option<MailWake>,
    pub(crate) warning: Option<String>,
}

/// Decide a launch's mail wake: the `--no-mail-wake` flag, then the
/// `[agents.codex] mail_wake` default, then the Codex version.
pub(crate) fn launch_wake(
    agent_type: &str,
    opted_out: bool,
    config_default: Option<bool>,
    codex_version: Option<&str>,
) -> LaunchWake {
    let next_turn = "mail arrives on its next turn";
    match WakeAdapter::for_vendor(agent_type) {
        WakeAdapter::NotApplicable => LaunchWake {
            state: None,
            warning: None,
        },
        WakeAdapter::Unsupported => LaunchWake {
            state: Some(MailWake::Unsupported),
            warning: Some(format!("{agent_type} has no live mail wake; {next_turn}")),
        },
        WakeAdapter::Codex if opted_out || config_default == Some(false) => LaunchWake {
            state: Some(MailWake::Off),
            warning: None,
        },
        WakeAdapter::Codex => match codex_version.and_then(parse_version) {
            Some(found) if found < MIN_CODEX_QUEUE => {
                let (major, minor, patch) = MIN_CODEX_QUEUE;
                LaunchWake {
                    state: Some(MailWake::Unsupported),
                    warning: Some(format!(
                        "codex {}.{}.{} predates `codex queue` ({major}.{minor}.{patch}), so \
                         AIDA cannot wake it; {next_turn}",
                        found.0, found.1, found.2
                    )),
                }
            }
            _ => LaunchWake {
                state: Some(MailWake::Pending),
                warning: None,
            },
        },
    }
}

/// The first `x.y.z` in a `--version` line (`codex-cli 0.164.0`).
pub(crate) fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    text.split_whitespace().find_map(|word| {
        let core = word.trim_start_matches('v');
        let core = core.split(['-', '+']).next()?;
        let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
        let version = (parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    })
}

/// How long `agent new` waits for `<codex> --version` before treating the
/// version as unknown.
const CODEX_VERSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// `<codex> --version`, when it answers in time; a wedged binary or wrapper
/// reads as an unknown version instead of hanging the launch.
pub(crate) fn codex_version(binary: &Path) -> Option<String> {
    version_within(binary, CODEX_VERSION_TIMEOUT)
}

fn version_within(binary: &Path, timeout: std::time::Duration) -> Option<String> {
    use std::io::Read;
    let mut child = std::process::Command::new(binary)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    // Read on a thread: a wrapper's grandchild can hold stdout open after
    // the child exits.
    let mut pipe = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let _ = tx.send(pipe.read_to_string(&mut out).map(|_| out));
    });
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let remaining = deadline
        .saturating_duration_since(std::time::Instant::now())
        .max(std::time::Duration::from_millis(500));
    let stdout = rx.recv_timeout(remaining).ok()?.ok()?;
    status.success().then(|| stdout.trim().to_string())
}

/// The exact commands that arm `handle` by hand.
pub(crate) fn manual_arm_commands(project_root: &Path, handle: &str) -> String {
    format!(
        "(cd {} && aida mailbox wake-service install) && aida agent wake arm {handle}",
        project_root.display()
    )
}

/// The outcome of arming one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Armed {
    pub(crate) state: MailWake,
    pub(crate) warning: Option<String>,
}

/// Arm a `pending` Codex row that now records its thread: make sure the
/// project's watcher unit exists, then flip the row to `armed` (or `failed`
/// with the manual commands). `None` when the row is not waiting to arm.
pub(crate) fn arm_pending(
    project_root: &Path,
    entry: &AgentRegistryEntry,
    ensure_service: impl FnOnce(&Path) -> Result<()>,
) -> Option<Armed> {
    if entry.mail_wake != Some(MailWake::Pending)
        || WakeAdapter::for_vendor(&entry.agent_type) != WakeAdapter::Codex
    {
        return None;
    }
    crate::mailbox_wake_run::row_thread(entry)?;
    let outcome = match entry.name.as_deref() {
        None => Armed {
            state: MailWake::Failed,
            warning: Some("mail wake not armed: the session has no handle to read mail as".into()),
        },
        Some(handle) => match ensure_service(project_root) {
            Ok(()) => Armed {
                state: MailWake::Armed,
                warning: None,
            },
            Err(why) => Armed {
                state: MailWake::Failed,
                warning: Some(format!(
                    "mail wake not armed for {handle}: {why:#}. To arm it by hand, run: {}",
                    manual_arm_commands(project_root, handle)
                )),
            },
        },
    };
    // Both the launcher and the hook may arm one launch: the first outcome
    // moves the row off pending, and a later success also replaces the other
    // side's failure, since the unit is installed after all. A failure never
    // overwrites an armed row. trace:STORY-HOME-096 | ai:claude
    let from: &[MailWake] = match outcome.state {
        MailWake::Armed => &[MailWake::Pending, MailWake::Failed],
        _ => &[MailWake::Pending],
    };
    match crate::agent_registry::transition_mail_wake_on_row(
        project_root,
        entry,
        from,
        outcome.state,
    ) {
        Ok(row) if row.mail_wake == Some(outcome.state) => Some(outcome),
        Ok(_) => None,
        Err(why) => Some(Armed {
            state: MailWake::Failed,
            warning: Some(format!("mail wake not recorded: {why:#}")),
        }),
    }
}

/// The variable `agent new` sets on its child naming the mail wake the
/// launch decided (`pending`, `off`, `unsupported`), so the child's own
/// SessionStart hook honours `--no-mail-wake` even when it writes the row
/// before the launcher stamps it.
// trace:STORY-2256 | ai:claude
pub(crate) const LAUNCH_WAKE_ENV: &str = "AIDA_MAIL_WAKE";

/// The wake a SessionStart hook records on a row nothing has decided yet:
/// the launcher's decision when it named one, else the default a hand-started
/// Codex session gets (`[agents.codex] mail_wake`, on unless set to false).
/// `None` for vendors the hook leaves alone.
// trace:STORY-2256 | ai:claude
pub(crate) fn session_wake(
    agent_type: &str,
    launcher: Option<&str>,
    config_default: impl FnOnce() -> Option<bool>,
) -> Option<MailWake> {
    if WakeAdapter::for_vendor(agent_type) != WakeAdapter::Codex {
        return None;
    }
    let decided = match launcher.map(str::trim) {
        Some("pending") => Some(MailWake::Pending),
        Some("off") => Some(MailWake::Off),
        Some("unsupported") => Some(MailWake::Unsupported),
        _ => None,
    };
    decided.or_else(|| launch_wake(agent_type, false, config_default(), None).state)
}

/// Arm the mail wake of the session whose SessionStart hook recorded
/// `entry`, whatever launched it: a Codex row nothing has decided yet takes
/// [`session_wake`]'s state first, then a `pending` row arms on its exact
/// recorded thread through [`arm_pending`].
// trace:STORY-2256 | ai:claude
pub(crate) fn arm_session(
    project_root: &Path,
    entry: &AgentRegistryEntry,
    launcher: Option<&str>,
    config_default: impl FnOnce() -> Option<bool>,
    ensure_service: impl FnOnce(&Path) -> Result<()>,
) -> Option<Armed> {
    let adopted;
    let entry = match entry.mail_wake {
        Some(_) => entry,
        None => {
            let state = session_wake(&entry.agent_type, launcher, config_default)?;
            adopted =
                match crate::agent_registry::adopt_mail_wake_on_row(project_root, entry, state) {
                    Ok(row) => row,
                    Err(why) => {
                        return Some(Armed {
                            state: MailWake::Failed,
                            warning: Some(format!("mail wake not recorded: {why:#}")),
                        })
                    }
                };
            &adopted
        }
    };
    arm_pending(project_root, entry, ensure_service)
}

/// The `Mail wake:` line `aida session start` prints for a Codex project:
/// whether the seat's SessionStart hook will arm its wake, and the one
/// command that installs the service when it is not there yet.
// trace:STORY-2256 | ai:claude
pub(crate) fn session_start_wake_line(
    vendor: &str,
    config_default: Option<bool>,
    service_installed: bool,
    project_root: &Path,
) -> Option<String> {
    if WakeAdapter::for_vendor(vendor) != WakeAdapter::Codex {
        return None;
    }
    Some(match (config_default, service_installed) {
        (Some(false), _) => {
            "Mail wake: off ([agents.codex] mail_wake = false); mail arrives on the next turn"
                .to_string()
        }
        (_, true) => {
            "Mail wake: arms when codex starts (the wake service is installed)".to_string()
        }
        (_, false) => format!(
            "Mail wake: arms when codex starts; it installs the wake service first. \
             If that is refused, run: (cd {} && aida mailbox wake-service install)",
            project_root.display()
        ),
    })
}

/// Retire a row's wake when its session ends. True when it changed.
pub(crate) fn retire(entry: &mut AgentRegistryEntry) -> bool {
    match entry.mail_wake {
        Some(MailWake::Pending | MailWake::Armed | MailWake::Failed) => {
            entry.mail_wake = Some(MailWake::Retired);
            true
        }
        _ => false,
    }
}

/// The `agent ls` WAKE cell.
pub(crate) fn wake_cell(agent_type: &str, state: Option<MailWake>) -> &'static str {
    match state {
        Some(MailWake::Pending) => "pending-thread",
        Some(state) => state.as_str(),
        None if WakeAdapter::for_vendor(agent_type) == WakeAdapter::Unsupported => "unsupported",
        None => "-",
    }
}

#[cfg(test)]
#[path = "mail_wake_tests.rs"]
mod tests;
