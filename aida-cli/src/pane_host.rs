//! Opt-in pane hosting for fanned-out drain implementers.
//!
//! When `aida queue work --auto-complete` (single, `--batch`, or nextN) fans
//! out implementers, the opt-in `--panes` flag hosts each implementer in its
//! own titled terminal window instead of an invisible background subprocess.
//! tmux is the first (and currently only) backend: each implementer runs in a
//! window named by its spec id under a dedicated `aida-drain` tmux session, so
//! the fan-out is visible. Because the windows live in the tmux *server*, they
//! survive the launching terminal closing (detach / reattach). The operator
//! owns pane cleanup — the drain never kills a pane.
//!
//! This module governs *only* WHERE the implementer process is hosted. The
//! lease / phase / merge flow is unchanged: a pane-hosted implementer runs the
//! exact same command with the exact same environment as the background spawn,
//! and its completion + real exit status are observed the same way (a tmux
//! `wait-for` rendezvous propagates the exit code back to the drain).
//!
//! Faithful-launcher rule: absent the flag, nothing here runs and the
//! background spawn is byte-identical to before. Requested-but-unavailable
//! (not inside a tmux server, or the tmux binary / probe fails) degrades
//! gracefully to the background spawn with a single notice line — a display
//! preference must never fail a drain.
//
// trace:TASK-1120 | ai:claude

/// The dedicated detached tmux session implementer windows are created under,
/// so they never crowd the operator's own working session.
pub(crate) const DRAIN_SESSION: &str = "aida-drain";

/// Transport for the resolved `--panes` host: the `queue work` / `burndown run`
/// dispatch exports this so every implementer the drain fans out reads the same
/// preference at its spawn point (single / `--batch` / nextN all share one
/// process). Documented in `docs/environment-variables.md`.
pub(crate) const HOST_ENV: &str = "AIDA_PANES";

/// Which terminal-multiplexer backend hosts the fanned implementers. tmux is
/// the only backend today; the enum leaves room for wezterm / zellij adapters
/// without reshaping the call sites.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaneHost {
    Tmux,
}

impl PaneHost {
    /// Parse a host token (the `--panes` value / `AIDA_PANES`). Case-insensitive;
    /// an unrecognized backend returns `None` so the caller degrades gracefully
    /// rather than erroring.
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "tmux" => Some(PaneHost::Tmux),
            _ => None,
        }
    }
}

/// Where a fanned implementer runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpawnMode {
    /// Host the implementer in a titled tmux window under [`DRAIN_SESSION`].
    TmuxWindow,
    /// The existing background subprocess spawn (the byte-identical default).
    Background,
}

/// PURE decision: given whether the operator asked for pane hosting and whether
/// this process is inside a tmux server, decide where the implementer runs.
///
/// Only a recognized host *and* a live tmux server yields [`SpawnMode::TmuxWindow`];
/// every other combination — no request, or requested-but-not-in-tmux — falls
/// back to [`SpawnMode::Background`]. The "requested but not in tmux" case is the
/// graceful-degrade path: the caller emits a one-line notice and proceeds.
pub(crate) fn spawn_mode(host: Option<PaneHost>, in_tmux: bool) -> SpawnMode {
    match host {
        Some(PaneHost::Tmux) if in_tmux => SpawnMode::TmuxWindow,
        _ => SpawnMode::Background,
    }
}

/// Are we inside a tmux server? tmux exports `$TMUX` to every process in a pane.
pub(crate) fn in_tmux() -> bool {
    std::env::var_os("TMUX").is_some()
}

/// The pane host the operator requested, read from the [`HOST_ENV`] transport.
/// `None` when unset or an unrecognized backend was named (degrade to background).
pub(crate) fn requested_host() -> Option<PaneHost> {
    std::env::var(HOST_ENV)
        .ok()
        .as_deref()
        .and_then(PaneHost::parse)
}

/// PURE builder: the `tmux new-window` argv that hosts `window_cmd` in a window
/// titled by `spec` under `session`, capturing the new pane id on stdout.
///
/// `-d` spawns without stealing the operator's focus; `-n <spec>` titles the
/// window by spec id; `-c` sets the working directory; each `-e KEY=VALUE`
/// injects one environment variable into the pane (tmux ≥ 3.0); `-P -F
/// '#{pane_id}'` prints the created pane id so the drain can cross-reference it.
pub(crate) fn tmux_new_window_argv(
    session: &str,
    spec: &str,
    window_cmd: &str,
    envs: &[(String, String)],
    cwd: Option<&str>,
) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "tmux".to_string(),
        "new-window".to_string(),
        "-d".to_string(),
        "-t".to_string(),
        format!("{session}:"),
        "-n".to_string(),
        spec.to_string(),
    ];
    if let Some(dir) = cwd {
        argv.push("-c".to_string());
        argv.push(dir.to_string());
    }
    for (k, v) in envs {
        argv.push("-e".to_string());
        argv.push(format!("{k}={v}"));
    }
    argv.push("-P".to_string());
    argv.push("-F".to_string());
    argv.push("#{pane_id}".to_string());
    argv.push(window_cmd.to_string());
    argv
}

/// Single-quote a token for embedding in a `/bin/sh -c` string (the pane's
/// window command). Wraps in single quotes and escapes any embedded single
/// quote as `'\''`.
#[cfg(unix)]
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Host `cmd` (the fully-prepared implementer subprocess) in a titled tmux
/// window and BLOCK until it completes, returning its real exit status as an
/// [`ExitOutcome::Natural`](crate::exit_signal::ExitOutcome).
///
/// The drain keeps its blocking outcome-tracking contract: tmux has no
/// block-until-exit verb, so the window runs the implementer, records its exit
/// code, then signals a unique `tmux wait-for` channel the drain waits on. The
/// implementer's environment + cwd are carried onto the pane verbatim (via `-e`
/// / `-c`), so the pane-hosted phase behaves identically to the background one.
///
/// Borrows `cmd` (never consumes it) so the caller can fall back to the
/// background spawn on any tmux error. Returns `Err` on any tmux failure; the
/// caller degrades gracefully.
#[cfg(unix)]
pub(crate) fn host_implementer_in_tmux(
    cmd: &std::process::Command,
    spec: &str,
) -> std::io::Result<crate::exit_signal::ExitOutcome> {
    use std::os::unix::process::ExitStatusExt;

    // Ensure the dedicated detached drain session exists. Idempotent: a
    // "duplicate session" error just means it already exists.
    let _ = std::process::Command::new("tmux")
        .args(["new-session", "-d", "-s", DRAIN_SESSION])
        .output();

    // Carry the implementer's explicit env overrides + cwd + argv onto the pane.
    // `get_envs` yields only the overrides the orchestrator set (AUTO_COMPLETE,
    // the run token, the phase index, the punt / hold signal paths, …) — exactly
    // what the pane-hosted phase needs to behave as an orchestrated implementer.
    let envs: Vec<(String, String)> = cmd
        .get_envs()
        .filter_map(|(k, v)| {
            v.map(|v| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
        })
        .collect();
    let cwd = cmd
        .get_current_dir()
        .map(|p| p.to_string_lossy().into_owned());
    let mut impl_argv: Vec<String> = vec![cmd.get_program().to_string_lossy().into_owned()];
    impl_argv.extend(cmd.get_args().map(|a| a.to_string_lossy().into_owned()));

    // Rendezvous channel + return-code file, both keyed by a per-spawn token.
    let token = uuid::Uuid::now_v7().to_string();
    let chan = format!("aida-drain-{token}");
    let rc_path = std::env::temp_dir().join(format!("aida-pane-rc-{token}"));
    let quoted_impl: Vec<String> = impl_argv.iter().map(|a| sh_quote(a)).collect();
    let window_cmd = format!(
        "{}; printf %s \"$?\" > {}; tmux wait-for -S {}",
        quoted_impl.join(" "),
        sh_quote(&rc_path.to_string_lossy()),
        sh_quote(&chan),
    );

    let argv = tmux_new_window_argv(DRAIN_SESSION, spec, &window_cmd, &envs, cwd.as_deref());
    let spawn = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .output()?;
    if !spawn.status.success() {
        let _ = std::fs::remove_file(&rc_path);
        return Err(std::io::Error::other(format!(
            "tmux new-window failed: {}",
            String::from_utf8_lossy(&spawn.stderr).trim()
        )));
    }
    // The created pane id (e.g. `%17`) — printed for cross-referencing; lease-side
    // recording of the pane<->spec link is a follow-up (see the spec).
    let _pane_id = String::from_utf8_lossy(&spawn.stdout).trim().to_string();

    // Block until the window signals completion, then recover the real exit code.
    let _ = std::process::Command::new("tmux")
        .args(["wait-for", &chan])
        .status();
    let code = std::fs::read_to_string(&rc_path)
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .unwrap_or(0);
    let _ = std::fs::remove_file(&rc_path);

    Ok(crate::exit_signal::ExitOutcome::Natural(
        std::process::ExitStatus::from_raw((code & 0xff) << 8),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_is_case_insensitive_and_rejects_unknown_backends() {
        assert_eq!(PaneHost::parse("tmux"), Some(PaneHost::Tmux));
        assert_eq!(PaneHost::parse("TMUX"), Some(PaneHost::Tmux));
        assert_eq!(PaneHost::parse("  tmux  "), Some(PaneHost::Tmux));
        assert_eq!(PaneHost::parse("wezterm"), None);
        assert_eq!(PaneHost::parse("zellij"), None);
        assert_eq!(PaneHost::parse(""), None);
    }

    #[test]
    fn panes_requested_and_in_tmux_selects_tmux_window() {
        assert_eq!(
            spawn_mode(Some(PaneHost::Tmux), true),
            SpawnMode::TmuxWindow
        );
    }

    #[test]
    fn panes_requested_but_not_in_tmux_degrades_to_background() {
        // The graceful-fallback path: the operator asked for panes but there's
        // no tmux server, so the drain uses a background implementer instead.
        assert_eq!(
            spawn_mode(Some(PaneHost::Tmux), false),
            SpawnMode::Background
        );
    }

    #[test]
    fn no_panes_request_is_always_background_regardless_of_tmux() {
        // Faithful-launcher: absent the flag, hosting is byte-identical to the
        // background spawn whether or not we happen to be inside tmux.
        assert_eq!(spawn_mode(None, true), SpawnMode::Background);
        assert_eq!(spawn_mode(None, false), SpawnMode::Background);
    }

    #[test]
    fn new_window_argv_is_titled_by_spec_and_captures_pane_id() {
        let argv = tmux_new_window_argv(
            DRAIN_SESSION,
            "STORY-1",
            "aida queue work STORY-1",
            &[],
            None,
        );
        // Titled by the spec id.
        let n = argv.iter().position(|a| a == "-n").expect("-n present");
        assert_eq!(argv[n + 1], "STORY-1");
        // Targets the dedicated drain session, detached, and captures the pane id.
        assert_eq!(argv[0], "tmux");
        assert_eq!(argv[1], "new-window");
        assert!(argv.contains(&"-d".to_string()));
        let t = argv.iter().position(|a| a == "-t").expect("-t present");
        assert_eq!(argv[t + 1], format!("{DRAIN_SESSION}:"));
        assert!(argv.contains(&"#{pane_id}".to_string()));
        // The window command is the final positional.
        assert_eq!(argv.last().unwrap(), "aida queue work STORY-1");
        // No cwd / env flags when none were supplied.
        assert!(!argv.contains(&"-c".to_string()));
        assert!(!argv.contains(&"-e".to_string()));
    }

    #[test]
    fn new_window_argv_threads_cwd_and_env() {
        let envs = vec![
            ("AIDA_AUTO_COMPLETE".to_string(), "1".to_string()),
            ("AIDA_PHASE".to_string(), "1".to_string()),
        ];
        let argv = tmux_new_window_argv(DRAIN_SESSION, "BUG-9", "run", &envs, Some("/work/tree"));
        let c = argv.iter().position(|a| a == "-c").expect("-c present");
        assert_eq!(argv[c + 1], "/work/tree");
        // Each env pair is passed as a `-e KEY=VALUE` token.
        assert!(argv
            .windows(2)
            .any(|w| w[0] == "-e" && w[1] == "AIDA_AUTO_COMPLETE=1"));
        assert!(argv
            .windows(2)
            .any(|w| w[0] == "-e" && w[1] == "AIDA_PHASE=1"));
    }
}
