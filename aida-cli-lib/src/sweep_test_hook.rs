//! A test-only seam at the exact instant a destructive sweep has finished
//! choosing its candidates and is about to write.
//!
//! The bug the sweeps carry is an interleaving, not a calculation: a spec
//! reopened between the eligibility re-check and the write used to be
//! overwritten. A test that merely runs a sweep never produces that
//! interleaving, so the sweeps call [`fire`] at that one point and a test
//! installs the reopen there. Outside `cfg(test)` this is an empty function
//! call with no state behind it.
// trace:BUG-1671 | ai:claude

#[cfg(test)]
mod imp {
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard};

    type Hook = Box<dyn Fn() + Send>;

    /// The pending hook and the store it belongs to, taken by the first
    /// [`fire`] for that store. The store key matters: the test binary runs
    /// tests in parallel, and another test's sweep over its own fixture must
    /// not consume this one's hook.
    static HOOK: Mutex<Option<(PathBuf, Hook)>> = Mutex::new(None);
    /// Serializes the tests that install a hook — the slot is process-wide.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// Holds the serialization lock and clears the slot when the test ends
    /// (including on a panic, so one failure cannot leak a hook into another
    /// test).
    #[must_use = "the hook is uninstalled when the guard is dropped"]
    pub(crate) struct Installed(#[allow(dead_code)] MutexGuard<'static, ()>);

    impl Drop for Installed {
        fn drop(&mut self) {
            *HOOK.lock().unwrap_or_else(|p| p.into_inner()) = None;
        }
    }

    fn key(root: &Path) -> PathBuf {
        std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
    }

    /// Install `hook` to run once, at the next sweep write over the store at
    /// `store_root`.
    pub(crate) fn install(store_root: &Path, hook: Hook) -> Installed {
        let serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        *HOOK.lock().unwrap_or_else(|p| p.into_inner()) = Some((key(store_root), hook));
        Installed(serial)
    }

    pub(crate) fn fire(store_root: &Path) {
        // Take the hook out before running it: it fires once, and the slot lock
        // is released first so the hook may itself write the store.
        let taken = {
            let mut slot = HOOK.lock().unwrap_or_else(|p| p.into_inner());
            match slot.as_ref() {
                Some((root, _)) if *root == key(store_root) => slot.take().map(|(_, h)| h),
                _ => None,
            }
        };
        if let Some(hook) = taken {
            hook();
        }
    }
}

#[cfg(test)]
pub(crate) use imp::{install, Installed};

/// Run the installed between-selection-and-write hook for the store at
/// `store_root`, once. A no-op in a non-test build.
// trace:BUG-1671 | ai:claude
#[inline]
pub(crate) fn fire(store_root: &std::path::Path) {
    #[cfg(test)]
    imp::fire(store_root);
    #[cfg(not(test))]
    let _ = store_root;
}
