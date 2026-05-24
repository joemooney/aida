//! Launcher → bash-wrapper intent channel (STORY-244).
//!
//! The launcher TUI ([`crate::launcher`]) does not host Claude as a PTY
//! child — two full-screen TUIs cannot share the terminal cleanly. Instead
//! it renders a dashboard, the user picks an action, and the launcher
//! exits writing one **intent line** to a dedicated file descriptor. A
//! tiny bash function (`aida-tui`, emitted by `aida dev shell-init`) reads
//! that line and dispatches the action: `eval` the launch command,
//! `claude --resume <id>`, etc. When the dispatched command exits, the
//! function re-launches the launcher and the loop continues.
//!
//! Using a dedicated fd (3 by default) for the intent — not stdout —
//! means the launcher can paint the real terminal directly while the
//! capture pipe stays render-byte clean.
//!
//! trace:STORY-244 | ai:claude

use anyhow::{Context, Result};
use std::io::Write;

/// One user action emitted by the launcher on exit. Exactly one Intent is
/// written per launcher run; the bash wrapper consumes it, dispatches,
/// and re-enters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// User wants to leave the launcher. The wrapper loop terminates.
    Quit,
    /// Run `<command>` as a shell command (typically `aida queue work
    /// <SPEC>`). The wrapper `eval`s it.
    Launch(String),
    /// Resume a recorded Claude conversation via `claude --resume <id>`.
    Resume(String),
    /// Generic shell escape (e.g. `gh pr view 42`). Dispatched by the
    /// wrapper as a plain shell command, like [`Intent::Launch`] but with
    /// a different wire prefix so the operator log is readable.
    Shell(String),
}

/// Characters allowed inside a `launch:` / `shell:` payload. Defense in
/// depth — a SPEC-ID will never legitimately contain shell metacharacters,
/// and the wrapper `eval`s the line, so anything outside this class is
/// refused. Excludes `;` `|` `&` `$` `` ` `` `<` `>` `(` `)` newlines etc.
/// trace:STORY-244 risk #8 | ai:claude
fn is_safe_payload(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, ' ' | '-' | '_' | '.' | '/' | '=' | ':' | ',' | '@' | '+')
        })
}

/// Render `intent` as the single line the wrapper consumes.
/// Newline-terminated. Returns `Err` when the payload contains a
/// character that the wrapper's `eval` would unsafely interpret.
pub fn serialize(intent: &Intent) -> Result<String> {
    match intent {
        Intent::Quit => Ok("quit\n".to_string()),
        Intent::Launch(cmd) => {
            anyhow::ensure!(
                is_safe_payload(cmd),
                "launch payload contains disallowed characters (the wrapper eval's it): {cmd:?}"
            );
            Ok(format!("launch:{cmd}\n"))
        }
        Intent::Resume(id) => {
            anyhow::ensure!(
                is_safe_payload(id),
                "resume payload contains disallowed characters: {id:?}"
            );
            Ok(format!("resume:{id}\n"))
        }
        Intent::Shell(cmd) => {
            anyhow::ensure!(
                is_safe_payload(cmd),
                "shell payload contains disallowed characters (the wrapper eval's it): {cmd:?}"
            );
            Ok(format!("shell:{cmd}\n"))
        }
    }
}

/// Write `intent` to the open file descriptor `fd`. The fd must already
/// be open (the bash wrapper sets up `3>&1` before invoking the launcher);
/// callers that need to detect a missing fd should run [`fd_is_writable`]
/// first.
///
/// On Unix the fd is wrapped with [`std::os::fd::FromRawFd`] for the
/// write, then leaked (`into_raw_fd`) so dropping the `File` doesn't
/// close the underlying fd — that's the wrapper's pipe, not ours.
///
/// On Windows the launcher's primary audience is bash-on-Linux for now;
/// this returns an error directing the user to use PTY-host mode.
/// trace:STORY-244 | ai:claude
#[cfg(unix)]
pub fn write_to_fd(intent: &Intent, fd: u32) -> Result<()> {
    use std::os::fd::{FromRawFd, IntoRawFd};
    let payload = serialize(intent)?;
    // Safety: caller's contract is that the fd is open (e.g. fd 3 wired
    // by the bash wrapper). Wrapping is sound; we leak the fd back via
    // `into_raw_fd` so Drop doesn't close it on the wrapper.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd as i32) };
    let res = file
        .write_all(payload.as_bytes())
        .and_then(|()| file.flush())
        .with_context(|| format!("failed to write intent to fd {fd}"));
    // Re-leak the descriptor so the wrapper's end isn't closed.
    let _ = file.into_raw_fd();
    res
}

#[cfg(not(unix))]
pub fn write_to_fd(_intent: &Intent, _fd: u32) -> Result<()> {
    anyhow::bail!(
        "the launcher's intent fd is unix-only — set `[tui] mode = \"pty-host\"` \
         in .aida/config.toml for the legacy multi-tab shell"
    )
}

/// True when the intent fd is open and distinct from stdout/stderr. The
/// launcher refuses to emit when fd shares (dev, ino) with fd 1 or fd 2
/// (typical when the user runs `aida tui --launcher` bare without the
/// `aida-tui` wrapper) — otherwise the intent line would spray into the
/// restored terminal. trace:STORY-244 risk #1 | ai:claude
#[cfg(unix)]
pub fn fd_is_writable_pipe(fd: u32) -> bool {
    use std::os::fd::{FromRawFd, IntoRawFd};

    let file = unsafe { std::fs::File::from_raw_fd(fd as i32) };
    let result = match file.metadata() {
        Ok(meta) => {
            // Reject when fd is the same kernel object as stdout/stderr —
            // the typical bare-invocation failure mode.
            !same_as_std_fd(&meta, 1) && !same_as_std_fd(&meta, 2)
        }
        Err(_) => false,
    };
    let _ = file.into_raw_fd();
    result
}

#[cfg(unix)]
fn same_as_std_fd(meta: &std::fs::Metadata, std_fd: i32) -> bool {
    use std::os::fd::{FromRawFd, IntoRawFd};
    use std::os::unix::fs::MetadataExt;
    let probe = unsafe { std::fs::File::from_raw_fd(std_fd) };
    let same = match probe.metadata() {
        Ok(std_meta) => meta.dev() == std_meta.dev() && meta.ino() == std_meta.ino(),
        Err(_) => false,
    };
    let _ = probe.into_raw_fd();
    same
}

#[cfg(not(unix))]
pub fn fd_is_writable_pipe(_fd: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_serializes_each_variant() {
        assert_eq!(serialize(&Intent::Quit).unwrap(), "quit\n");
        assert_eq!(
            serialize(&Intent::Launch("aida queue work STORY-244".into())).unwrap(),
            "launch:aida queue work STORY-244\n"
        );
        assert_eq!(
            serialize(&Intent::Resume("019e2d4f-1234-7abc".into())).unwrap(),
            "resume:019e2d4f-1234-7abc\n"
        );
        assert_eq!(
            serialize(&Intent::Shell("gh pr view 42".into())).unwrap(),
            "shell:gh pr view 42\n"
        );
    }

    #[test]
    fn quit_terminates_with_newline() {
        let s = serialize(&Intent::Quit).unwrap();
        assert!(s.ends_with('\n'));
        assert_eq!(s.trim_end(), "quit");
    }

    #[test]
    fn launch_preserves_inner_command_spaces() {
        let s = serialize(&Intent::Launch("aida queue work STORY-244".into())).unwrap();
        assert!(s.contains("queue work"), "spaces preserved: {s:?}");
    }

    #[test]
    fn serialize_rejects_metacharacters() {
        // Shell injection vectors: $(...), backticks, ;, |, &, >, <, etc.
        for bad in [
            "aida; rm -rf /",
            "aida && touch /tmp/x",
            "$(whoami)",
            "`whoami`",
            "aida|grep x",
            "aida > /tmp/x",
            "aida < /tmp/x",
            "aida\nrm",
        ] {
            assert!(
                serialize(&Intent::Launch(bad.to_string())).is_err(),
                "should reject {bad:?}"
            );
            assert!(
                serialize(&Intent::Shell(bad.to_string())).is_err(),
                "should reject shell {bad:?}"
            );
        }
    }

    #[test]
    fn serialize_accepts_typical_spec_payloads() {
        // SPEC-IDs, paths, common flags must all pass.
        for ok in [
            "aida queue work STORY-244",
            "aida queue work --auto-complete",
            "aida queue work --batch overnight-1",
            "claude --resume 019e2d4f-7777-7abc",
            "gh pr view 42",
            "aida show STORY-244",
        ] {
            assert!(serialize(&Intent::Launch(ok.to_string())).is_ok(), "{ok}");
        }
    }

    #[test]
    fn empty_payload_is_rejected() {
        assert!(serialize(&Intent::Launch(String::new())).is_err());
        assert!(serialize(&Intent::Resume(String::new())).is_err());
        assert!(serialize(&Intent::Shell(String::new())).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn write_to_fd_round_trips_through_a_regular_file() {
        use std::io::{Read, Seek, SeekFrom};
        use std::os::fd::AsRawFd;

        // A tempfile-backed regular file works the same as a pipe for
        // the writer's perspective: opening it as an fd, writing, then
        // re-reading proves the writer wrote what serialize() promised.
        let mut tmp = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(std::env::temp_dir().join(format!(
                "aida-intent-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )))
            .unwrap();
        let fd = tmp.as_raw_fd() as u32;

        // `write_to_fd` wraps the fd, writes, then leaks it via
        // into_raw_fd — so the underlying fd survives for our `tmp` to
        // continue using.
        write_to_fd(&Intent::Launch("aida queue work STORY-244".into()), fd)
            .expect("write_to_fd succeeds");

        tmp.seek(SeekFrom::Start(0)).unwrap();
        let mut buf = String::new();
        tmp.read_to_string(&mut buf).unwrap();
        assert_eq!(buf, "launch:aida queue work STORY-244\n");
    }
}
