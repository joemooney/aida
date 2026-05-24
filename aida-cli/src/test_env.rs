//! Test-only env var guard — RAII pattern that serialises process-global
//! env-var swaps so parallel tests within the same process don't trample
//! each other's state. The BUG-371 anti-pattern this module exists to
//! prevent: a test mutates `std::env::set_var("FOO", "x")` without a
//! lock, a sibling test running in parallel reads `FOO`, and the two
//! collide intermittently — symptoms are flaky CI, NotFound at file
//! unwraps, and tests that pass in serial but fail under `--test-threads`.
//!
//! See `docs/aida/discipline/test-isolation.md` for the full pattern and
//! the per-test temp-path variant (Codex's BUG-371 fix) used when the
//! state crosses a subprocess boundary instead of staying in-process.
//!
//! Existing module-local mutexes (`with_env_vars` in `advisor.rs`,
//! `with_bg_fetch_env` and `scoped_prepend_path` in `main.rs`, the
//! `workflow_hints` LOCK) predate this helper and are functionally
//! equivalent — they each guard a single distinct key, so they don't
//! race with each other or with `ENV_LOCK` here. New tests should reach
//! for `EnvVarGuard` rather than coining another local mutex.
//! trace:TASK-521 | ai:claude

use std::ffi::{OsStr, OsString};
use std::sync::{Mutex, MutexGuard};

/// Global mutex serialising every env-var swap that routes through
/// `EnvVarGuard`. Coarse-grained on purpose: one lock for all vars
/// matches the underlying constraint (`std::env::set_var` is
/// process-global and not thread-safe across keys on most platforms),
/// and parallel tests don't need finer-grained locking — env-var
/// mutations are not hot.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard that sets (or unsets) an env var for the guard's lifetime
/// and restores the prior value on drop. Holds `ENV_LOCK` for the whole
/// lifetime — drop the guard before constructing another for a
/// dependent test step, or the next `set`/`unset` will deadlock.
pub(crate) struct EnvVarGuard {
    key: &'static str,
    prev: Option<OsString>,
    // _guard holds ENV_LOCK; field-ordered last so it drops after
    // `prev` is read by `Drop` — Rust drops fields in declaration order,
    // so the lock is the last thing released.
    _guard: MutexGuard<'static, ()>,
}

impl EnvVarGuard {
    /// Set `key` to `value` for the guard's lifetime. Lock poisoning is
    /// tolerated (`into_inner` on a poisoned lock) so a panic in one
    /// test doesn't cascade into "every later test panics on lock
    /// acquisition" and mask the real failure.
    pub(crate) fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        let guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os(key);
        // SAFETY: serialised by ENV_LOCK; no other test routed through
        // this helper mutates env vars without acquiring the same lock.
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var(key, value);
        }
        Self {
            key,
            prev,
            _guard: guard,
        }
    }

    /// Remove `key` for the guard's lifetime. Same locking + restoration
    /// discipline as [`Self::set`].
    pub(crate) fn unset(key: &'static str) -> Self {
        let guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os(key);
        // SAFETY: serialised by ENV_LOCK.
        #[allow(unused_unsafe)]
        unsafe {
            std::env::remove_var(key);
        }
        Self {
            key,
            prev,
            _guard: guard,
        }
    }

    /// Mutate `key` to `value` while still holding the lock — equivalent
    /// to dropping and re-acquiring, but without the round trip. Used
    /// for tests that iterate over many values for the same key
    /// (`auto_bump_env_flag_respects_opt_out` is the canonical case).
    pub(crate) fn reset(&mut self, value: impl AsRef<OsStr>) {
        // SAFETY: still holding ENV_LOCK via self._guard.
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var(self.key, value);
        }
    }

    /// Companion to [`Self::reset`] for the unset case.
    pub(crate) fn reset_unset(&mut self) {
        // SAFETY: still holding ENV_LOCK via self._guard.
        #[allow(unused_unsafe)]
        unsafe {
            std::env::remove_var(self.key);
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: still holding ENV_LOCK via self._guard.
        #[allow(unused_unsafe)]
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Setting then dropping restores the prior value (when one existed).
    #[test]
    fn set_then_drop_restores_prior_value() {
        const KEY: &str = "AIDA_TEST_GUARD_RESTORE";
        // Establish a prior value via a first guard.
        let outer = EnvVarGuard::set(KEY, "outer");
        assert_eq!(std::env::var(KEY).unwrap(), "outer");
        drop(outer);
        // Outer was the first to touch KEY, so drop should remove it
        // (no prior value existed when the guard was created).
        assert!(std::env::var(KEY).is_err());
    }

    /// Nested guards stack: dropping the inner restores the outer's
    /// value. (Not actually nested at runtime — each `set` takes the
    /// lock — but verifies the prev-capture is correct.)
    #[test]
    fn set_captures_existing_value_for_restore() {
        const KEY: &str = "AIDA_TEST_GUARD_NESTED";
        let outer = EnvVarGuard::set(KEY, "outer");
        drop(outer);
        let _outer = EnvVarGuard::set(KEY, "outer");
        // Drop the outer to release the lock before re-acquiring.
        drop(_outer);
        let inner = EnvVarGuard::set(KEY, "inner");
        assert_eq!(std::env::var(KEY).unwrap(), "inner");
        drop(inner);
        // No prior value at inner's set time → KEY is gone.
        assert!(std::env::var(KEY).is_err());
    }

    /// `unset` removes the var for the guard's lifetime and restores
    /// the prior value on drop.
    #[test]
    fn unset_removes_and_restores() {
        const KEY: &str = "AIDA_TEST_GUARD_UNSET";
        let seeded = EnvVarGuard::set(KEY, "seeded");
        drop(seeded);
        let g = EnvVarGuard::set(KEY, "live");
        assert_eq!(std::env::var(KEY).unwrap(), "live");
        drop(g);
        // KEY had no prior value when the first `set` happened, so it's
        // unset now.
        assert!(std::env::var(KEY).is_err());
    }

    /// `reset` mutates the value without releasing the lock — used by
    /// tests that loop over many spellings for a single key.
    #[test]
    fn reset_swaps_value_under_held_lock() {
        const KEY: &str = "AIDA_TEST_GUARD_RESET";
        let mut g = EnvVarGuard::set(KEY, "first");
        assert_eq!(std::env::var(KEY).unwrap(), "first");
        g.reset("second");
        assert_eq!(std::env::var(KEY).unwrap(), "second");
        g.reset_unset();
        assert!(std::env::var(KEY).is_err());
        drop(g);
        assert!(std::env::var(KEY).is_err());
    }
}
