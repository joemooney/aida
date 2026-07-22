//! Shared harness for the tests that drive the REAL `aida()` shell wrapper
//! (`crate::dev_cmd::SHELL_HELPERS`) through a REAL shell with a stub `aida`
//! on PATH.
//!
//! `aida dev shell-init --install` installs the wrapper into `~/.bashrc` AND
//! `~/.zshrc`, so every guarantee the wrapper implements — above all the
//! marker-delimited eval channel — has to hold in both shells. bash and zsh
//! disagree on word splitting and array indexing, and a slicing divergence
//! would otherwise surface as a broken shell for the first zsh user instead of
//! as a red test here.
//!
//! So the shell is a PARAMETER: tests iterate `wrapper_shells()`, which is
//! bash plus zsh when the runner has zsh. A machine without zsh (the common
//! case — it is not installed by default on Ubuntu) is a clean SKIP, never a
//! failure.
// trace:TASK-1174 | ai:claude

use std::sync::OnceLock;

/// Can we actually run this shell? Probed by executing a no-op script rather
/// than by looking for the binary on PATH, so a shell that exists but cannot
/// start counts as absent.
// trace:TASK-1174 | ai:claude
fn shell_runs(shell: &str) -> bool {
    std::process::Command::new(shell)
        .args(shell_args(shell))
        .arg("exit 0")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Args that put a shell into "run this script string" mode.
///
/// zsh gets `-f` (no rc files) so the harness never picks up the developer's
/// own `~/.zshenv`; bash keeps the plain `-c` the existing tests have always
/// used.
// trace:TASK-1174 | ai:claude
fn shell_args(shell: &str) -> &'static [&'static str] {
    match shell {
        "zsh" => &["-f", "-c"],
        _ => &["-c"],
    }
}

/// The shells to exercise the wrapper in: bash always, zsh when present.
///
/// bash is treated as required — every platform the test suite runs on has it,
/// and losing bash coverage silently would be worse than a hard failure.
// trace:TASK-1174 | ai:claude
pub(crate) fn wrapper_shells() -> &'static [&'static str] {
    static SHELLS: OnceLock<Vec<&'static str>> = OnceLock::new();
    SHELLS.get_or_init(|| {
        let mut shells = vec!["bash"];
        if shell_runs("zsh") {
            shells.push("zsh");
        } else {
            // Visible under `cargo test -- --nocapture` so a skipped matrix
            // leg is never mistaken for a passing one.
            eprintln!(
                "note: zsh not available on this machine — wrapper tests cover bash only. \
                 Install zsh to exercise the zsh leg."
            );
        }
        shells
    })
}

/// Drive the real `SHELL_HELPERS` wrapper through `shell` with a stub `aida`
/// on PATH. `stub` is the body of the fake binary; `body` runs after the
/// helpers are sourced. Returns (stdout, stderr, exit code).
// trace:TASK-1174 | ai:claude
pub(crate) fn run_wrapper_in(shell: &str, stub: &str, body: &str) -> (String, String, Option<i32>) {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("aida");
    std::fs::write(&bin, stub).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let script = format!(
        "PATH='{path}':\"$PATH\"\nexport PATH\n{helpers}\n{body}\n",
        path = dir.path().display(),
        helpers = crate::dev_cmd::SHELL_HELPERS,
        body = body,
    );
    let out = std::process::Command::new(shell)
        .args(shell_args(shell))
        .arg(&script)
        .output()
        .unwrap_or_else(|e| panic!("{shell} available: {e}"));
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code(),
    )
}
