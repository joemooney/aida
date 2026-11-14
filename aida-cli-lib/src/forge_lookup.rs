// trace:TASK-3428 | ai:codex
use crate::{command_output_with_timeout, resolve_forge_binary, RetryEtxtbsy};

/// BUG-1594: the message a forge lookup carries when the scoped
/// [`with_forge_lookup_timeout`] ceiling killed it. Rendered by `aida show`
/// as "timed out" rather than a generic unreachable so the output says
/// exactly why the PR/MR state is unknown (PRIN-5).
// trace:BUG-1594 | ai:claude
pub(crate) const FORGE_LOOKUP_TIMED_OUT: &str = "forge lookup timed out";

thread_local! {
    // trace:BUG-1594 | ai:claude
    pub(crate) static FORGE_LOOKUP_TIMEOUT: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
}

/// BUG-1594: run `f` with every open-change forge lookup it makes (`gh pr
/// list` / `glab api`) bounded by `timeout`. Scoped rather than global so
/// interactive read surfaces (`aida show`) get a hard ceiling while the
/// orchestrator's phase probes keep their existing unbounded semantics.
// trace:BUG-1594 | ai:claude
pub(crate) fn with_forge_lookup_timeout<T>(
    timeout: std::time::Duration,
    f: impl FnOnce() -> T,
) -> T {
    let prev = FORGE_LOOKUP_TIMEOUT.with(|c| c.replace(Some(timeout)));
    struct Restore(Option<std::time::Duration>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let prev = self.0;
            FORGE_LOOKUP_TIMEOUT.with(|c| c.set(prev));
        }
    }
    let _restore = Restore(prev);
    f()
}

/// BUG-1594: spawn a forge-CLI lookup honoring the scoped
/// [`with_forge_lookup_timeout`] ceiling. `Ok(None)` means the call was
/// killed at the ceiling; `Err` is a spawn failure; with no scoped ceiling
/// this is exactly `Command::output()`.
// trace:BUG-1594 | ai:claude
pub(crate) fn forge_lookup_output(
    mut cmd: std::process::Command,
) -> std::io::Result<Option<std::process::Output>> {
    match FORGE_LOOKUP_TIMEOUT.with(|c| c.get()) {
        // trace:BUG-1735 | ai:claude
        None => cmd.output_retrying_etxtbsy().map(Some),
        Some(timeout) => {
            let started = std::time::Instant::now();
            match command_output_with_timeout(cmd, timeout) {
                Some(out) => Ok(Some(out)),
                None if started.elapsed() >= timeout => Ok(None),
                None => Err(std::io::Error::other("could not spawn the forge CLI")),
            }
        }
    }
}

/// Resolve the `glab` (GitLab CLI) binary, mirroring `resolve_gh_binary`'s
/// PATH-walk + sanity-spawn (BUG-74/79). Wired into the forge call sites in
/// follow-on STORY-621 slices. trace:STORY-621 | ai:claude
#[allow(dead_code)] // wired into call sites in follow-on STORY-621 slices
pub(crate) fn resolve_glab_binary() -> Option<std::path::PathBuf> {
    resolve_forge_binary("glab", "AIDA_TEST_GLAB_BINARY", "AIDA_DEBUG_GLAB")
}

#[cfg(all(test, unix))]
mod resolve_gh_binary_tests {
    use super::*;
    use crate::{is_executable, resolve_gh_binary};
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    // These tests run in exact single-test children. A shared writer mutex
    // alone cannot protect other tests' unlocked executable lookups.
    /// Prepend a directory only inside the isolated test child.
    // trace:BUG-1994 | ai:codex
    // trace:TASK-1532 | ai:agy
    fn scoped_prepend_path(dir: &std::path::Path) -> impl Drop {
        crate::test_env::EnvVarGuard::prepend_path(dir)
    }

    fn make_executable(path: &std::path::Path) {
        crate::test_exec::write_executable(path, "#!/bin/sh\necho gh fake\n");
    }

    fn make_non_executable(path: &std::path::Path) {
        std::fs::write(path, "not executable\n").unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(path, perms).unwrap();
    }

    /// PATH walk picks up gh even when the system gh isn't on PATH.
    /// Critical regression guard for BUG-74. trace:BUG-74 | ai:claude
    #[test]
    fn finds_gh_via_path() {
        // trace:BUG-1994 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let gh = tmp.path().join("gh");
        make_executable(&gh);
        let _g = scoped_prepend_path(tmp.path());
        let resolved = resolve_gh_binary().expect("expected to find fake gh on PATH");
        assert_eq!(
            std::fs::canonicalize(&resolved).unwrap(),
            std::fs::canonicalize(&gh).unwrap()
        );
    }

    /// PATH walk ignores non-executable files named gh. trace:BUG-74
    #[test]
    fn rejects_non_executable() {
        // trace:BUG-1994 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let gh = tmp.path().join("gh");
        make_non_executable(&gh);
        let _g = scoped_prepend_path(tmp.path());
        let resolved = resolve_gh_binary();
        drop(_g);
        // We may still pick up a real system gh via the absolute-path
        // fallback — but we should NOT have picked up our broken fake.
        if let Some(p) = resolved {
            assert_ne!(
                std::fs::canonicalize(&p).unwrap(),
                std::fs::canonicalize(&gh).unwrap()
            );
        }
    }

    #[test]
    fn is_executable_checks_perm_bit() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("x");
        make_executable(&path);
        assert!(is_executable(&path));
        make_non_executable(&path);
        assert!(!is_executable(&path));
    }

    #[test]
    fn is_executable_rejects_dirs() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_executable(tmp.path()));
    }

    #[test]
    fn is_executable_rejects_missing() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_executable(&tmp.path().join("nope")));
    }

    /// BUG-79: a file that passes is_executable but whose spawn fails
    /// (broken interpreter line) is not returned — we fall back to the
    /// next candidate or return None. trace:BUG-79 | ai:claude
    #[test]
    fn rejects_is_executable_but_spawn_fails() {
        // trace:BUG-1994 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let bad = tmp.path().join("gh");
        // Shebang points at a non-existent interpreter, so spawn errors
        // with ENOENT even though is_executable returns true.
        crate::test_exec::write_executable(
            &bad,
            "#!/this/interpreter/does/not/exist\necho should never run\n",
        );
        assert!(is_executable(&bad));

        let _g = scoped_prepend_path(tmp.path());
        let resolved = resolve_gh_binary();
        drop(_g);
        // Either None (no other gh) or some other path — never the broken one.
        if let Some(p) = resolved {
            assert_ne!(
                std::fs::canonicalize(&p).unwrap(),
                std::fs::canonicalize(&bad).unwrap()
            );
        }
    }

    /// BUG-79: when PATH has a broken candidate followed by a working one,
    /// the working one wins. trace:BUG-79 | ai:claude
    #[test]
    fn falls_back_past_broken_to_working() {
        // trace:BUG-1994 | ai:codex
        if crate::test_env::run_path_isolated_test() {
            return;
        }
        let broken_dir = TempDir::new().unwrap();
        let good_dir = TempDir::new().unwrap();

        let broken = broken_dir.path().join("gh");
        crate::test_exec::write_executable(&broken, "#!/this/does/not/exist\n");

        let good = good_dir.path().join("gh");
        make_executable(&good);

        // Prepend BOTH dirs so the broken one is hit first, then the good
        // one. Order matters: broken_dir wins on lookup order.
        let combined = std::path::PathBuf::from(format!(
            "{}:{}",
            broken_dir.path().display(),
            good_dir.path().display()
        ));
        let _g = scoped_prepend_path(&combined);
        let resolved = resolve_gh_binary().expect("should fall back to the working gh");
        drop(_g);
        assert_eq!(
            std::fs::canonicalize(&resolved).unwrap(),
            std::fs::canonicalize(&good).unwrap()
        );
    }
}
