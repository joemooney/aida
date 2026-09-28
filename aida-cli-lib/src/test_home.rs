//! BUG-1642: the lib test binary never touches the operator's real home.
//!
//! Lib tests reach dozens of global writers that resolve `~/.aida/...`
//! (role activity in `roles/<role>.toml`, `usage.jsonl`, `presence.toml`,
//! `turn-clock/`, the schedule files, the mailbox, `shift-local.toml`, ...)
//! through `dirs::home_dir()`, `$HOME`, or an `AIDA_HOME` / `AIDA_TEST_HOME`
//! override. Before this module, any test that did not set an override wrote
//! into the operator's real `~/.aida` (the queue_rework tests appended
//! `[[activity]]` rows for fixture specs to the real `roles/advisor.toml`).
//!
//! Two layers:
//!
//! 1. **Redirect, before any test runs.** A constructor in the platform's
//!    init section (the same mechanism the `ctor` crate uses, without the
//!    dependency) runs before `main`, while the process is still
//!    single-threaded, and points `HOME` (and `USERPROFILE` on Windows) at a
//!    fresh per-process temp dir. It clears the operator's `AIDA_HOME`,
//!    `AIDA_TEST_HOME` and `XDG_{CONFIG,DATA,CACHE,STATE}_HOME` so every
//!    fallback derives from that temp home. This covers every writer at
//!    once, including the ones in `aida-core` (compiled without `cfg(test)`
//!    when it is a dependency) and any `aida`/`git` child process a test
//!    spawns, because they inherit the redirected environment.
//! 2. **Refuse at the source.** Every home lookup in this crate goes through
//!    [`crate::home_dir`], which under `cfg(test)` calls [`home_dir`] here:
//!    it panics if the resolved home is the real one, instead of returning
//!    it. The explicit `AIDA_HOME` / `AIDA_TEST_HOME` resolvers
//!    (global roles dir, presence, turn-clock, schedule home, cache home)
//!    pass their result through [`assert_hermetic`] too. A panic is chosen
//!    over a silent no-op because a no-op would hide the bug the test is
//!    meant to exercise (the recorder would stop recording in tests only),
//!    while a panic names the leaking call site in the failing test.
//!
//! A source-scan test (`no_direct_home_resolution_in_crate`) keeps new code
//! from calling `dirs::home_dir()` / `dirs::config_dir()`, or reading
//! `HOME` / `USERPROFILE` / `APPDATA` directly, and bypassing layer 2. The
//! production resolvers live in `aida_core::home`, which consults the
//! environment before the platform lookup so the redirect also wins on
//! Windows, where `dirs` ignores it. (TASK-1513)
// trace:BUG-1642 | ai:claude
// trace:TASK-1513 | ai:claude

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

struct Redirect {
    /// Home locations the operator's environment named at startup: the
    /// `dirs::home_dir()` answer, `$HOME`, `$USERPROFILE`, `$AIDA_HOME`,
    /// `$AIDA_TEST_HOME`, and the password-database home. None of them may be
    /// resolved as a home by a test.
    real_homes: Vec<PathBuf>,
    /// The per-process temp home every test inherits.
    test_home: PathBuf,
}

static REDIRECT: OnceLock<Redirect> = OnceLock::new();

/// Exported to child processes: the real homes the top-level test process
/// fenced, so a re-exec of the test binary keeps the same fence.
const REAL_HOMES_ENV: &str = "AIDA_LIB_TEST_REAL_HOMES";

/// Env vars cleared before `main` so their fallbacks derive from the temp
/// `HOME` instead of an operator-set location.
const CLEARED_VARS: &[&str] = &[
    "AIDA_HOME",
    "AIDA_TEST_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
];

/// BUG-1642: every env var with this prefix is cleared before `main` in the
/// top-level test process. The operator's session identity
/// (`AIDA_SESSION_ROLE`, `AIDA_SESSION_PROJECT`, `AIDA_SESSION_ID`,
/// `AIDA_SESSION_SCOPE`, `AIDA_SESSION_PURPOSE`, ...) would otherwise steer
/// code under test, e.g. `record_role_activity` resolving the operator's main
/// checkout's `.aida/roles/<role>.toml`. Tests that need a role set it through
/// `EnvVarsGuard` / `AmbientGuard`. (A re-exec'd child keeps what its spawning
/// test chose, since the ambient values were already gone in the parent.)
// trace:BUG-1642 | ai:claude
const CLEARED_PREFIX: &str = "AIDA_SESSION_";

#[used]
#[cfg_attr(
    any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly",
        target_os = "illumos",
        target_os = "solaris"
    ),
    unsafe(link_section = ".init_array")
)]
#[cfg_attr(
    target_vendor = "apple",
    unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(windows, unsafe(link_section = ".CRT$XCU"))]
static REDIRECT_HOME_BEFORE_MAIN: extern "C" fn() = redirect_home_before_main;

// The `unsafe extern` block and the `unsafe(link_section = ..)` attributes
// above are the forms edition 2024 requires; both are accepted on edition 2021
// since Rust 1.82, so moving this workspace to 2024 needs no change here.
// trace:TASK-1513 | ai:claude
unsafe extern "C" {
    fn atexit(cb: extern "C" fn()) -> std::ffi::c_int;
}

/// Name prefix of the per-process temp homes [`create_test_home`] makes.
const TEST_HOME_PREFIX: &str = "aida-lib-test-home-";

/// The home recorded in the password database. `$HOME` cannot change it, so it
/// fences the operator's real home even when the environment lies.
// trace:TASK-1513 | ai:claude
#[cfg(unix)]
fn passwd_home() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;

    // SAFETY: after the redirect, `$HOME` is always set and `dirs::home_dir()`
    // returns it, so no other code in this process calls `getpw*` concurrently.
    // The pointer stays valid until the next `getpw*` call on this thread.
    let dir = unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() {
            return None;
        }
        (*pw).pw_dir
    };
    if dir.is_null() {
        return None;
    }
    // SAFETY: `pw_dir` is a NUL-terminated C string owned by libc.
    let bytes = unsafe { std::ffi::CStr::from_ptr(dir) }.to_bytes();
    (!bytes.is_empty()).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

/// No password database to consult off unix.
// trace:TASK-1513 | ai:claude
#[cfg(not(unix))]
fn passwd_home() -> Option<PathBuf> {
    None
}

/// Is `pid` still running? "Unknown" counts as alive, so
/// [`sweep_stale_test_homes`] only removes a directory whose owner is
/// definitely gone.
// trace:TASK-1513 | ai:claude
fn pid_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return true;
        };
        // Never probe 0 or a negative pid: those address process *groups*.
        if pid <= 0 {
            return true;
        }
        // SAFETY: signal 0 performs the permission check only; nothing is
        // delivered to the target process.
        if unsafe { libc::kill(pid, 0) } == 0 {
            return true;
        }
        // `EPERM` means alive but not ours; only `ESRCH` means gone.
        std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// A test binary killed by a signal never runs its `atexit` hook, so its temp
/// home survives in the temp dir. Remove the leftovers whose owning process is
/// gone before this run makes its own. Best-effort throughout: a live pid's
/// directory, a symlink, a directory owned by another user, and any I/O error
/// are all left alone, and `install` never fails because of the sweep.
// trace:TASK-1513 | ai:claude
fn sweep_stale_test_homes(root: &Path, own_pid: u32, is_alive: &dyn Fn(u32) -> bool) -> usize {
    // Enough to clear a backlog without stalling startup on a busy temp dir.
    const MAX_SWEEP: usize = 64;

    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut swept = 0;
    for entry in entries.flatten() {
        if swept >= MAX_SWEEP {
            break;
        }
        // `file_type` does not follow symlinks, so a symlinked directory is
        // skipped rather than followed out of the temp dir.
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix(TEST_HOME_PREFIX))
            .and_then(|p| p.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == own_pid || is_alive(pid) {
            continue;
        }
        if std::fs::remove_dir_all(entry.path()).is_ok() {
            swept += 1;
        }
    }
    swept
}

fn non_empty_env(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

extern "C" fn redirect_home_before_main() {
    if let Err(e) = install() {
        // Running the tests against the real home is exactly the bug; refuse.
        eprintln!("BUG-1642: could not install the hermetic test HOME: {e}");
        std::process::abort();
    }
}

fn install() -> std::io::Result<()> {
    // A test can spawn this very binary (`aida_exe_path()` resolves to the
    // test executable under `cargo test`), and that child runs this
    // constructor too. The parent exports the real homes it fenced, so the
    // child keeps the same fence and reuses the temp home it was handed
    // instead of creating (and leaking) its own.
    if let Some(inherited) = std::env::var_os(REAL_HOMES_ENV) {
        return install_nested(std::env::split_paths(&inherited).collect());
    }

    let mut real_homes: Vec<PathBuf> = Vec::new();
    for p in [
        dirs::home_dir(),
        non_empty_env("HOME"),
        non_empty_env("USERPROFILE"),
        non_empty_env("AIDA_HOME"),
        non_empty_env("AIDA_TEST_HOME"),
        // trace:TASK-1513 | ai:claude
        passwd_home(),
    ]
    .into_iter()
    .flatten()
    {
        if !real_homes.contains(&p) {
            real_homes.push(p);
        }
    }

    // trace:TASK-1513 | ai:claude
    sweep_stale_test_homes(&std::env::temp_dir(), std::process::id(), &pid_is_alive);
    let test_home = create_test_home()?;

    // Toolchains resolve from `$HOME` too; pin them to the real locations so
    // a test that spawns `cargo` keeps working.
    let real_home = real_homes.first().cloned();
    let pins: Vec<(&str, PathBuf)> = [("CARGO_HOME", ".cargo"), ("RUSTUP_HOME", ".rustup")]
        .into_iter()
        .filter(|(k, _)| std::env::var_os(k).is_none())
        .filter_map(|(k, sub)| {
            let p = real_home.as_ref()?.join(sub);
            p.is_dir().then_some((k, p))
        })
        .collect();
    let exported = std::env::join_paths(&real_homes).map_err(std::io::Error::other)?;

    // SAFETY: this runs from the init section before `main`, so no other
    // thread exists yet to race the environment.
    #[allow(unused_unsafe)]
    unsafe {
        for (k, p) in &pins {
            std::env::set_var(k, p);
        }
        for k in CLEARED_VARS {
            std::env::remove_var(k);
        }
        let session_vars: Vec<std::ffi::OsString> = std::env::vars_os()
            .map(|(k, _)| k)
            .filter(|k| k.to_str().is_some_and(|k| k.starts_with(CLEARED_PREFIX)))
            .collect();
        for k in session_vars {
            std::env::remove_var(k);
        }
        std::env::set_var(REAL_HOMES_ENV, exported);
        set_home_vars(&test_home);
    }

    finish(real_homes, test_home, true);
    Ok(())
}

/// The real homes a nested test binary fences. `AIDA_LIB_TEST_REAL_HOMES` is a
/// hint, not a guarantee: a test that spawns this binary can set it to anything
/// (or to something useless), and trusting it alone would disable the fence.
/// The password-database home cannot be forged from the environment, so it is
/// fenced whatever the inherited list says.
// trace:TASK-1513 | ai:claude
fn nested_real_homes(mut inherited: Vec<PathBuf>) -> Vec<PathBuf> {
    if let Some(pw) = passwd_home() {
        if !inherited.contains(&pw) {
            inherited.push(pw);
        }
    }
    inherited
}

/// Constructor body for a test binary spawned by a test. Whatever home the
/// spawning test chose is kept unless it names a real home; only a real home
/// is replaced or cleared.
fn install_nested(real_homes: Vec<PathBuf>) -> std::io::Result<()> {
    let real_homes = nested_real_homes(real_homes);
    let is_real = |p: &Path| real_homes.iter().any(|r| r == p);
    let inherited = non_empty_env("HOME").filter(|h| !is_real(h));
    let (test_home, owned) = match inherited {
        Some(h) => (h, false),
        None => (create_test_home()?, true),
    };
    // SAFETY: before `main`, single-threaded (see `install`).
    #[allow(unused_unsafe)]
    unsafe {
        for k in CLEARED_VARS {
            if non_empty_env(k).is_some_and(|v| is_real(&v)) {
                std::env::remove_var(k);
            }
        }
        set_home_vars(&test_home);
    }
    finish(real_homes, test_home, owned);
    Ok(())
}

fn create_test_home() -> std::io::Result<PathBuf> {
    let test_home = std::env::temp_dir().join(format!("{TEST_HOME_PREFIX}{}", std::process::id()));
    // A leftover from a crashed run with a recycled pid is stale; start clean.
    let _ = std::fs::remove_dir_all(&test_home);
    std::fs::create_dir_all(test_home.join(".aida"))?;
    // Git reads identity and defaults from `$HOME/.gitconfig`; give spawned
    // `git` a deterministic one so tests do not depend on the operator's.
    std::fs::write(
        test_home.join(".gitconfig"),
        "[user]\n\tname = AIDA Test\n\temail = aida-test@example.invalid\n\
         [init]\n\tdefaultBranch = main\n[commit]\n\tgpgsign = false\n\
         [tag]\n\tgpgsign = false\n",
    )?;
    Ok(test_home)
}

/// SAFETY: caller guarantees no other thread exists (pre-`main`).
unsafe fn set_home_vars(test_home: &Path) {
    #[allow(unused_unsafe)]
    unsafe {
        std::env::set_var("HOME", test_home);
        #[cfg(windows)]
        {
            std::env::set_var("USERPROFILE", test_home);
            // `aida_core::home::config_dir()` reads `%APPDATA%` before the
            // platform lookup on Windows; point it inside the test home so the
            // config dir is fenced there too.
            // trace:TASK-1513 | ai:claude
            std::env::set_var("APPDATA", test_home.join("AppData").join("Roaming"));
        }
    }
}

fn finish(real_homes: Vec<PathBuf>, test_home: PathBuf, owned: bool) {
    let _ = REDIRECT.set(Redirect {
        real_homes,
        test_home,
    });
    if owned {
        // SAFETY: `atexit` is the C runtime's; the callback is a plain
        // `extern "C" fn()` with no captured state.
        unsafe {
            atexit(remove_test_home_at_exit);
        }
    }
}

extern "C" fn remove_test_home_at_exit() {
    if let Some(r) = REDIRECT.get() {
        let _ = std::fs::remove_dir_all(&r.test_home);
    }
}

fn installed() -> &'static Redirect {
    REDIRECT.get().expect(
        "BUG-1642: the hermetic test HOME was not installed before main; \
         refusing to resolve a home directory (it would be the operator's real one)",
    )
}

/// The per-process temp home the lib tests run under.
pub(crate) fn test_home() -> &'static Path {
    &installed().test_home
}

/// Panic if `path` is one of the operator's real homes or lies under a real
/// `~/.aida`. Called by every global-dir resolver under `cfg(test)`.
#[track_caller]
pub(crate) fn assert_hermetic(path: &Path) {
    for real in &installed().real_homes {
        if path == real.as_path() || path.starts_with(real.join(".aida")) {
            panic!(
                "BUG-1642: a lib test resolved {} under the operator's real home {}. \
                 Tests must never read or write the real ~/.aida; point HOME / AIDA_HOME / \
                 AIDA_TEST_HOME at a temp dir instead.",
                path.display(),
                real.display()
            );
        }
    }
}

/// `cfg(test)` body of [`crate::home_dir`]: `$HOME` (which the redirect set,
/// and which tests override per-case under the env lock) on every platform,
/// falling back to `dirs::home_dir()`, and never the operator's real home.
#[track_caller]
pub(crate) fn home_dir() -> Option<PathBuf> {
    // `dirs::home_dir()` ignores `$HOME` on Windows; reading it directly
    // makes the redirect effective there too.
    let home = non_empty_env("HOME").or_else(dirs::home_dir)?;
    assert_hermetic(&home);
    Some(home)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The redirect is in effect before any test body runs: the process
    /// `HOME` is the temp home, not any real one, and the operator's
    /// overrides are gone.
    #[test]
    fn redirect_is_installed_before_tests_run() {
        let _env = crate::test_env::EnvVarsGuard::snapshot(&["HOME"]);
        let r = installed();
        assert!(r.test_home.join(".gitconfig").is_file());
        assert!(
            !r.real_homes.iter().any(|h| h == &r.test_home),
            "temp home must differ from every real home"
        );
        let home = crate::home_dir().expect("home resolves");
        assert!(home.starts_with(&r.test_home), "{}", home.display());
        #[cfg(unix)]
        assert_eq!(dirs::home_dir().as_deref(), Some(r.test_home.as_path()));
    }

    /// The operator's ambient session identity never reaches a test: the
    /// constructor cleared every `AIDA_SESSION_*` var before `main`. Checked
    /// under the env lock so a sibling test's explicit guard can't interfere.
    // trace:BUG-1642 | ai:claude
    #[test]
    fn ambient_session_vars_are_cleared_before_tests_run() {
        let _env = crate::test_env::EnvVarsGuard::snapshot(&[]);
        let leaked: Vec<String> = std::env::vars_os()
            .filter_map(|(k, _)| k.into_string().ok())
            .filter(|k| k.starts_with(CLEARED_PREFIX))
            .collect();
        assert!(leaked.is_empty(), "inherited session vars: {leaked:?}");
    }

    /// The source guard refuses the real home and anything under a real
    /// `~/.aida`, but allows other paths (a worktree can live under the real
    /// home, so only the home itself and its `.aida` are fenced).
    #[test]
    fn assert_hermetic_refuses_the_real_home_and_its_aida_dir() {
        let r = installed();
        let Some(real) = r.real_homes.first() else {
            return; // no real home on this machine: nothing to fence
        };
        for bad in [
            real.clone(),
            real.join(".aida"),
            real.join(".aida").join("roles").join("advisor.toml"),
        ] {
            let res = std::panic::catch_unwind(|| assert_hermetic(&bad));
            assert!(res.is_err(), "{} must be refused", bad.display());
        }
        assert_hermetic(&real.join("some-worktree").join(".aida"));
        assert_hermetic(&r.test_home.join(".aida").join("roles"));
    }

    /// The resolver that leaked in BUG-1642: with no override set, the global
    /// roles dir (where rework / queue-add record activity) is under the temp
    /// home.
    #[test]
    fn global_roles_dir_resolves_under_the_temp_home() {
        let _env =
            crate::test_env::EnvVarsGuard::apply(&[("AIDA_TEST_HOME", None), ("AIDA_HOME", None)]);
        let dir = crate::global_roles_dir().expect("roles dir");
        assert!(dir.starts_with(test_home()), "{}", dir.display());
        let presence = crate::presence::presence_path().expect("presence path");
        assert!(presence.starts_with(test_home()), "{}", presence.display());
        let clock = crate::presence::turn_clock_dir().expect("turn clock");
        assert!(clock.starts_with(test_home()), "{}", clock.display());
    }

    /// No code in this crate may resolve a home or config directory itself.
    /// `dirs::home_dir()` bypasses [`crate::home_dir`]'s `cfg(test)` guard;
    /// `dirs::config_dir()` and a direct `HOME` / `USERPROFILE` / `APPDATA`
    /// read bypass [`aida_core::home`], which is what makes the redirect
    /// effective on Windows. A deliberate exception carries an
    /// `allow-direct-home-dir` marker on the line.
    // trace:TASK-1513 | ai:claude
    #[test]
    fn no_direct_home_resolution_in_crate() {
        let mut needles = vec![
            ["dirs", "::", "home_dir"].concat(),
            ["dirs", "::", "config_dir"].concat(),
        ];
        for key in ["HOME", "USERPROFILE", "APPDATA"] {
            needles.push(format!("var(\"{key}\")"));
            needles.push(format!("var_os(\"{key}\")"));
        }

        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().is_none_or(|e| e != "rs") || p.ends_with("test_home.rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap();
                for (i, line) in text.lines().enumerate() {
                    if line.trim_start().starts_with("//") || line.contains("allow-direct-home-dir")
                    {
                        continue;
                    }
                    if needles.iter().any(|n| line.contains(n)) {
                        offenders.push(format!("{}:{}", p.display(), i + 1));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "use crate::home_dir() / aida_core::home::config_dir() instead of resolving \
             a home directory directly:\n{}",
            offenders.join("\n")
        );
    }

    /// The shared `aida-core` resolvers follow the redirected home too, so a
    /// lib test that reaches a `~/.config/aida` writer through `aida-core`
    /// stays inside the temp home. On unix `dirs::config_dir()` is
    /// `$XDG_CONFIG_HOME` or `$HOME/.config` and the constructor cleared the
    /// former; on Windows the constructor redirects `%APPDATA%`.
    // trace:TASK-1513 | ai:claude
    #[test]
    fn core_resolvers_follow_the_redirected_home() {
        let _env = crate::test_env::EnvVarsGuard::apply(&[("XDG_CONFIG_HOME", None)]);
        assert_eq!(
            aida_core::home::home_dir().as_deref(),
            Some(test_home()),
            "aida-core must resolve the temp home"
        );
        let config = aida_core::home::config_dir().expect("config dir");
        assert!(config.starts_with(test_home()), "{}", config.display());
    }

    /// A bogus (or empty) inherited `AIDA_LIB_TEST_REAL_HOMES` must not
    /// disable the fence: the password-database home is added regardless.
    // trace:TASK-1513 | ai:claude
    #[test]
    fn nested_real_homes_fences_the_passwd_home_whatever_was_inherited() {
        let bogus = vec![PathBuf::from("/nonexistent/forged-home")];
        let homes = nested_real_homes(bogus.clone());
        assert!(homes.starts_with(&bogus), "the inherited list is kept");

        match passwd_home() {
            Some(pw) => {
                assert!(
                    homes.contains(&pw),
                    "the password-database home {} must be fenced",
                    pw.display()
                );
                assert!(!bogus.contains(&pw), "the bogus list did not name it");
                // Added once, not on every call.
                assert_eq!(nested_real_homes(homes.clone()), homes);
            }
            // No password database on this platform: the inherited list is all
            // there is, and the fence is unchanged.
            None => assert_eq!(homes, bogus),
        }
    }

    /// Only a directory whose owning process is gone is swept: a live pid's
    /// directory, this process's own, an unrelated name and a regular file all
    /// survive.
    // trace:TASK-1513 | ai:claude
    #[test]
    fn stale_test_home_dirs_are_swept_only_for_dead_pids() {
        let root = test_home().join("sweep-fixture");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let dead = root.join(format!("{TEST_HOME_PREFIX}1001"));
        let live = root.join(format!("{TEST_HOME_PREFIX}1002"));
        let own = root.join(format!("{TEST_HOME_PREFIX}{}", std::process::id()));
        let unrelated = root.join("not-a-test-home");
        for dir in [&dead, &live, &own, &unrelated] {
            // A non-empty directory, so a shallow `remove_dir` would not do.
            std::fs::create_dir_all(dir.join(".aida")).unwrap();
        }
        let file = root.join(format!("{TEST_HOME_PREFIX}1003"));
        std::fs::write(&file, b"not a directory").unwrap();

        let swept = sweep_stale_test_homes(&root, std::process::id(), &|pid| pid == 1002);

        assert_eq!(swept, 1, "only the dead pid's directory is removed");
        assert!(!dead.exists(), "the dead pid's temp home is gone");
        assert!(live.is_dir(), "a live pid's temp home is untouched");
        assert!(own.is_dir(), "this process's own temp home is untouched");
        assert!(unrelated.is_dir(), "an unrelated directory is untouched");
        assert!(file.is_file(), "a regular file is never removed");

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The liveness probe errs towards "alive": a pid outside `pid_t`, and the
    /// process-group pid 0, are never treated as sweepable.
    // trace:TASK-1513 | ai:claude
    #[test]
    fn pid_is_alive_errs_towards_alive() {
        assert!(pid_is_alive(std::process::id()), "this process is alive");
        assert!(
            pid_is_alive(0),
            "pid 0 addresses a process group; never sweep"
        );
        assert!(pid_is_alive(u32::MAX), "outside pid_t: unknown, so alive");
        #[cfg(unix)]
        {
            // Far above every platform's `pid_max`, so it cannot exist.
            let impossible = i32::MAX as u32 - 1;
            assert!(!pid_is_alive(impossible), "an impossible pid is gone");
        }
    }
}
