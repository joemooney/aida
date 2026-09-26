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
//! A source-scan test (`no_direct_dirs_home_dir_in_crate`) keeps new code
//! from calling `dirs::home_dir()` directly and bypassing layer 2.
// trace:BUG-1642 | ai:claude

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

struct Redirect {
    /// Home locations the operator's environment named at startup: the
    /// `dirs::home_dir()` answer, `$HOME`, `$USERPROFILE`, `$AIDA_HOME`,
    /// `$AIDA_TEST_HOME`. None of them may be resolved as a home by a test.
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
    link_section = ".init_array"
)]
#[cfg_attr(target_vendor = "apple", link_section = "__DATA,__mod_init_func")]
#[cfg_attr(windows, link_section = ".CRT$XCU")]
static REDIRECT_HOME_BEFORE_MAIN: extern "C" fn() = redirect_home_before_main;

extern "C" {
    fn atexit(cb: extern "C" fn()) -> std::ffi::c_int;
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
    ]
    .into_iter()
    .flatten()
    {
        if !real_homes.contains(&p) {
            real_homes.push(p);
        }
    }

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

/// Constructor body for a test binary spawned by a test. Whatever home the
/// spawning test chose is kept unless it names a real home; only a real home
/// is replaced or cleared.
fn install_nested(real_homes: Vec<PathBuf>) -> std::io::Result<()> {
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
    let test_home = std::env::temp_dir().join(format!("aida-lib-test-home-{}", std::process::id()));
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
        std::env::set_var("USERPROFILE", test_home);
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

    /// No code in this crate may call `dirs::home_dir()` directly: it would
    /// bypass [`crate::home_dir`]'s `cfg(test)` guard.
    #[test]
    fn no_direct_dirs_home_dir_in_crate() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let needle = ["dirs", "::", "home_dir"].concat();
        let mut offenders = Vec::new();
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&p).unwrap();
                    for (i, line) in text.lines().enumerate() {
                        let allowed = p.ends_with("test_home.rs")
                            || line.trim_start().starts_with("//")
                            || line.contains("allow-direct-home-dir");
                        if line.contains(&needle) && !allowed {
                            offenders.push(format!("{}:{}", p.display(), i + 1));
                        }
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "use crate::home_dir() instead of calling the dirs crate directly:\n{}",
            offenders.join("\n")
        );
    }
}
