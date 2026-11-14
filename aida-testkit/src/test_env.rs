//! Test-only env var guard — RAII pattern that serialises process-global
//! env-var swaps so parallel tests within the same process don't trample
//! each other's state. The BUG-371 anti-pattern this module exists to
//! prevent: a test mutates `std::env::set_var("FOO", "x")` without a
//! lock, a sibling test running in parallel reads `FOO`, and the two
//! collide intermittently — symptoms are flaky CI, NotFound at file
//! unwraps, and tests that pass in serial but fail under `--test-threads`.
//!
//! See `.aida/discipline/test-isolation.md` for the full pattern and
//! the per-test temp-path variant (Codex's BUG-371 fix) used when the
//! state crosses a subprocess boundary instead of staying in-process.
//!
//! BUG-697: module-local mutexes (`with_env_vars` in `advisor.rs`, the
//! `with_env` helpers in `glyphs.rs` / `first_run.rs`, `user_alias.rs`,
//! `workflow_hints`, the `OS_WRAP` / `HEADLESS_VENDOR` / `PATH` locks in
//! `session.rs` / `main.rs`) predate this helper. The old claim that they
//! "guard a distinct key so they don't race" was WRONG: `std::env::set_var`
//! mutates the process-global `environ` and is not thread-safe *across keys*
//! (a `setenv` can realloc `environ` while another thread `getenv`s an
//! unrelated key), so a swap under one lock genuinely data-races a read
//! under another. They now all acquire the SAME [`env_lock`] so every
//! env-touching test — whatever helper it uses — serialises against every
//! other. New tests should reach for `EnvVarGuard` rather than coining a
//! local mutex. trace:TASK-521 trace:BUG-697 | ai:claude

// trace:TASK-3784 | ai:codex

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

/// Set by the live-bwrap CI gate: a test that cannot get bwrap fails rather
/// than skipping.
pub const REQUIRE_BWRAP_LIVE_ENV: &str = "AIDA_REQUIRE_BWRAP_LIVE";

/// PATH readers in production code do not take ENV_LOCK. Run a PATH-changing
/// test as one exact test in its own process, before constructing any fixture.
/// Returns true in the parent after the child passed, so the caller returns.
// trace:BUG-1994 trace:TASK-2189 | ai:codex
pub fn run_path_isolated_test() -> bool {
    run_path_isolated_test_output().is_some()
}

/// [`run_path_isolated_test`] that hands the parent the passed child's
/// output, so a test can assert what the code under test never printed.
/// Returns None in the child.
// trace:BUG-2119 | ai:claude
pub fn run_path_isolated_test_output() -> Option<std::process::Output> {
    let thread = std::thread::current();
    let name = thread.name().expect("libtest thread name");
    if is_path_isolated_process() {
        assert_eq!(std::env::var("AIDA_TEST_PATH_CHILD").unwrap(), name);
        return None;
    }
    assert!(
        !holds_env_lock(),
        "isolate before taking an environment guard"
    );
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join(".gitconfig"),
        "[user]\nname = AIDA Test\nemail = test@example.invalid\n[init]\ndefaultBranch = main\n",
    )
    .unwrap();
    let mut command = std::process::Command::new(
        std::env::current_exe().expect("resolve current test executable"),
    );
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home.path())
        // The pre-main test-home constructor uses PID-based names and sweeps
        // stale homes. Native seats have separate PID namespaces but share
        // /tmp, so their equal PIDs must never share that constructor's root.
        // Keep every child's scratch files inside this parent's unique dir.
        // trace:TASK-2861 | ai:codex
        .env("TMPDIR", home.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("AIDA_TEST_PATH_CHILD", name)
        .args(["--exact", "--nocapture", "--test-threads=1", name]);
    // Keep the constructor's real-home fence across env_clear. Without it the
    // child treats its assigned HOME as an operator home, replaces it with a
    // PID-named directory and sweeps sibling homes. PID namespaces can share
    // those names while their liveness probes cannot see each other's owners.
    // trace:TASK-2841 | ai:codex
    command.env(
        crate::test_home::REAL_HOMES_ENV,
        std::env::var_os(crate::test_home::REAL_HOMES_ENV)
            .expect("test constructor exports the real-home fence"),
    );
    // Some test binaries are linked against Rust libraries in cargo's search path.
    if let Some(path) = std::env::var_os("LD_LIBRARY_PATH") {
        command.env("LD_LIBRARY_PATH", path);
    }
    // The live-bwrap CI gate turns a bwrap skip into a failure; the child must
    // see it too, or an isolated live test skips silently under the gate.
    // trace:TASK-2161 | ai:claude
    if let Some(live) = std::env::var_os(REQUIRE_BWRAP_LIVE_ENV) {
        command.env(REQUIRE_BWRAP_LIVE_ENV, live);
    }
    let output = command.output().expect("spawn isolated test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The child's output is captured, so re-emit its skip notices.
    for line in String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter(|line| line.starts_with("SKIP"))
    {
        eprintln!("{line}");
    }
    assert!(
        output.status.success(),
        "isolated test {name} failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.starts_with("test result: ok. 1 passed; 0 failed;")),
        "isolated test must actually run one test: {name}\n{stdout}"
    );
    Some(output)
}

// trace:BUG-1994 | ai:codex
pub fn is_path_isolated_process() -> bool {
    let Ok(name) = std::env::var("AIDA_TEST_PATH_CHILD") else {
        return false;
    };
    let args: Vec<_> = std::env::args().skip(1).collect();
    !name.is_empty()
        && args.len() == 4
        && args[0] == "--exact"
        && args[1] == "--nocapture"
        && args[2] == "--test-threads=1"
        && args[3] == name
}

// trace:BUG-1994 | ai:codex
// trace:TASK-1748 | ai:antigravity
pub fn require_path_mutation_scope(key: &str) {
    if key == "PATH" || key == "HOME" {
        assert!(
            is_path_isolated_process(),
            "{key} mutation requires run_path_isolated_test() before any fixture or env guard"
        );
    }
}

/// Global mutex serialising every env-var swap that routes through
/// `EnvVarGuard`. Coarse-grained on purpose: one lock for all vars
/// matches the underlying constraint (`std::env::set_var` is
/// process-global and not thread-safe across keys on most platforms),
/// and parallel tests don't need finer-grained locking — env-var
/// mutations are not hot.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Process-unique token for the calling thread. `ThreadId::as_u64` is
/// unstable, so [`env_lock`] mints its own monotonic token per thread to
/// detect same-thread reentrance. Tokens are never reused, so a finished
/// thread's token can never be mistaken for a live one. 0 means "unowned".
static NEXT_THREAD_TOKEN: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static THREAD_TOKEN: u64 = NEXT_THREAD_TOKEN.fetch_add(1, Ordering::Relaxed);
}

/// Token of the thread currently holding the env lock, or 0 when unheld.
static ENV_LOCK_OWNER: AtomicU64 = AtomicU64::new(0);

fn thread_token() -> u64 {
    THREAD_TOKEN.with(|t| *t)
}

/// TASK-1559: RAII wrapper around the env-lock guard that publishes the
/// owning thread on acquire and clears it on release. `Drop` for this struct
/// runs BEFORE its fields drop, so ownership is cleared while the mutex is
/// still held — a waiter that acquires next can never observe a stale owner.
/// It also runs on unwind, so a holder that panics does not leave ownership
/// pointing at a dead thread (which would wedge that thread's later
/// acquisitions into a spurious panic instead of succeeding).
pub struct EnvLockGuard {
    _inner: MutexGuard<'static, ()>,
}

impl Drop for EnvLockGuard {
    fn drop(&mut self) {
        ENV_LOCK_OWNER.store(0, Ordering::Release);
    }
}

/// BUG-697: the ONE process-global env lock. Every test helper that mutates
/// or reads env-derived state must serialise on this — the module-local
/// mutexes that used to guard individual keys now delegate here so a swap
/// under one helper can never overlap a read/swap under another (the
/// `setenv` realloc race). Poison-tolerant like the guards. Hold the returned
/// guard for the whole mutate→read→restore window; do NOT nest a second
/// acquisition (incl. an `EnvVarGuard`) under it — the lock is not reentrant,
/// and TASK-1559 makes nesting panic immediately rather than hang.
pub fn env_lock() -> EnvLockGuard {
    let me = thread_token();
    // TASK-1559: nesting is a hang, not an error, because the lock is not
    // reentrant — so detect it BEFORE blocking and report it instead. A
    // different thread's token here is ordinary contention: fall through and
    // block. Only this thread ever writes or clears its own token, so a
    // match cannot be a false positive.
    if ENV_LOCK_OWNER.load(Ordering::Acquire) == me {
        // A plain `panic!` rather than `assert_ne!`: the token values are an
        // implementation detail, and `assert_ne!` would append a confusing
        // "left: 1, right: 1" to the one message that has to be legible.
        panic!(
            "ENV_LOCK already held by this thread — do not nest env_lock() or \
             construct an EnvVarGuard/EnvVarsGuard inside an env_lock() scope. \
             Scope the guard to the window that needs it, or use EnvVarsGuard \
             to set several keys under a single acquisition. This panic \
             replaces the self-deadlock that nesting used to cause (TASK-1559)."
        );
    }
    let inner = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    ENV_LOCK_OWNER.store(me, Ordering::Release);
    EnvLockGuard { _inner: inner }
}

/// True when THIS thread is currently holding [`env_lock`] — i.e. whatever
/// the ambient env says right now, this test put it there on purpose and no
/// sibling can change it underneath.
///
/// BUG-1743: that distinction is what separates a test that *deliberately*
/// points an env-derived anchor somewhere (and is therefore serialised
/// against every other setter) from one that merely *inherited* a sibling's
/// pin and is about to write into a temp tree it does not own and cannot
/// keep alive. Only the second is a bug, and only this predicate can tell
/// them apart: the paths themselves are indistinguishable, since both are
/// `tempfile` roots under `/tmp`.
// trace:BUG-1743 | ai:claude
pub fn holds_env_lock() -> bool {
    ENV_LOCK_OWNER.load(Ordering::Acquire) == thread_token()
}

/// RAII guard that sets (or unsets) an env var for the guard's lifetime
/// and restores the prior value on drop. Holds `ENV_LOCK` for the whole
/// lifetime — drop the guard before constructing another for a
/// dependent test step, or the next `set`/`unset` will deadlock.
pub struct EnvVarGuard {
    key: &'static str,
    prev: Option<OsString>,
    // _guard holds ENV_LOCK; field-ordered last so it drops after
    // `prev` is read by `Drop` — Rust drops fields in declaration order,
    // so the lock is the last thing released.
    _guard: EnvLockGuard,
}

/// Multi-key variant of [`EnvVarGuard`] for tests that need SEVERAL env
/// vars set simultaneously. `EnvVarGuard` holds `ENV_LOCK` for its whole
/// lifetime, so two `EnvVarGuard::set` calls deadlock — this acquires the
/// lock ONCE and sets/restores every key under it. trace:TASK-818
pub struct EnvVarsGuard {
    prev: Vec<(&'static str, Option<OsString>)>,
    _guard: EnvLockGuard,
    cleanup: Option<fn(&[(&'static str, Option<OsString>)])>,
}

impl EnvVarsGuard {
    /// Install an owner-specific cleanup, run before releasing the env lock.
    pub fn with_cleanup(mut self, cleanup: fn(&[(&'static str, Option<OsString>)])) -> Self {
        self.cleanup = Some(cleanup);
        self
    }

    /// Set each `(key, value)` for the guard's lifetime; restore prior
    /// values on drop. Same lock-poisoning tolerance as [`EnvVarGuard::set`].
    pub fn set(pairs: &[(&'static str, &str)]) -> Self {
        let owned: Vec<(&'static str, Option<&str>)> =
            pairs.iter().map(|(k, v)| (*k, Some(*v))).collect();
        Self::apply(&owned)
    }

    /// General form of [`Self::set`]: `Some(value)` sets the key, `None`
    /// REMOVES it, for the guard's lifetime. The removal case is what lets a
    /// test assert the *absent-env* behaviour hermetically — it both clears an
    /// ambient value and (because the guard holds `ENV_LOCK` for its whole
    /// lifetime) serialises against every sibling test that sets those same
    /// keys. A test that merely *reads* env-derived state through the code
    /// under test must hold one of these guards too, or it races the setters.
    // trace:TASK-148 | ai:claude
    pub fn apply(pairs: &[(&'static str, Option<&str>)]) -> Self {
        for (key, _) in pairs {
            require_path_mutation_scope(key);
        }
        let guard = env_lock();
        let mut prev = Vec::with_capacity(pairs.len());
        for (key, value) in pairs {
            require_path_mutation_scope(key);
            prev.push((*key, std::env::var_os(key)));
            // SAFETY: serialised by ENV_LOCK.
            #[allow(unused_unsafe)]
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
        Self {
            prev,
            _guard: guard,
            cleanup: None,
        }
    }
}

impl EnvVarsGuard {
    /// Hold [`env_lock`] and remember each key's current value WITHOUT
    /// changing it; on drop every key is restored. For a test whose code
    /// under test may mutate the env (or would, if a guard regressed), so a
    /// failure cannot leave `PATH` / `LD_PRELOAD` poisoned for the rest of
    /// the test process.
    // trace:BUG-1627 | ai:claude
    pub fn snapshot(keys: &[&'static str]) -> Self {
        for key in keys {
            require_path_mutation_scope(key);
        }
        let guard = env_lock();
        let prev = keys.iter().map(|k| (*k, std::env::var_os(k))).collect();
        Self {
            prev,
            _guard: guard,
            cleanup: None,
        }
    }

    pub fn set_key(&mut self, key: &'static str, value: impl AsRef<OsStr>) {
        require_path_mutation_scope(key);
        if !self.prev.iter().any(|(k, _)| *k == key) {
            self.prev.push((key, std::env::var_os(key)));
        }
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var(key, value);
        }
    }

    pub fn unset_key(&mut self, key: &'static str) {
        require_path_mutation_scope(key);
        if !self.prev.iter().any(|(k, _)| *k == key) {
            self.prev.push((key, std::env::var_os(key)));
        }
        #[allow(unused_unsafe)]
        unsafe {
            std::env::remove_var(key);
        }
    }
}

impl Drop for EnvVarsGuard {
    fn drop(&mut self) {
        // Reverse order, so a var applied after the ones it depends on (a
        // GIT_CONFIG_COUNT after its pairs) is removed before them.
        // trace:TASK-1979 | ai:claude
        for (key, prev) in self.prev.iter().rev() {
            // SAFETY: still holding ENV_LOCK via self._guard.
            #[allow(unused_unsafe)]
            unsafe {
                match prev {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
        if let Some(cleanup) = self.cleanup {
            cleanup(&self.prev);
        }
    }
}

impl EnvVarGuard {
    /// Set `key` to `value` for the guard's lifetime. Lock poisoning is
    /// tolerated (`into_inner` on a poisoned lock) so a panic in one
    /// test doesn't cascade into "every later test panics on lock
    /// acquisition" and mask the real failure.
    pub fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        require_path_mutation_scope(key);
        let guard = env_lock();
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
    pub fn unset(key: &'static str) -> Self {
        require_path_mutation_scope(key);
        let guard = env_lock();
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
    pub fn reset(&mut self, value: impl AsRef<OsStr>) {
        require_path_mutation_scope(self.key);
        // SAFETY: still holding ENV_LOCK via self._guard.
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var(self.key, value);
        }
    }

    /// Companion to [`Self::reset`] for the unset case.
    pub fn reset_unset(&mut self) {
        require_path_mutation_scope(self.key);
        // SAFETY: still holding ENV_LOCK via self._guard.
        #[allow(unused_unsafe)]
        unsafe {
            std::env::remove_var(self.key);
        }
    }

    /// Acquire the crate-wide env lock, prepend `dir` to `PATH`, and restore
    /// the previous `PATH` on drop. Allowed only in a one-test child process;
    /// a writer-only mutex cannot protect concurrent unlocked readers.
    // trace:TASK-1532 | ai:agy
    pub fn prepend_path(dir: impl AsRef<Path>) -> Self {
        require_path_mutation_scope("PATH");
        let guard = env_lock();
        let prev = std::env::var_os("PATH");
        let new = match &prev {
            Some(v) => {
                let mut s = std::ffi::OsString::from(dir.as_ref().as_os_str());
                s.push(":");
                s.push(v);
                s
            }
            None => dir.as_ref().as_os_str().to_os_string(),
        };
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var("PATH", &new);
        }
        Self {
            key: "PATH",
            prev,
            _guard: guard,
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

/// Pin the queue identity (`current_user_id` reads `AIDA_USER`) for a
/// queue-asserting test, holding the process-wide env lock for the guard's
/// lifetime. Without it, a parallel test that sets `AIDA_USER` under its own
/// guard can flip the identity between a fixture's queue writes, the code
/// under test, and the final `queue_list`, so the test reads (or writes) a
/// different user's queue file. Hold the guard for the whole test; do not
/// take another env guard under it.
// trace:BUG-1658 trace:BUG-1666 | ai:claude
pub fn pin_queue_user() -> EnvVarGuard {
    EnvVarGuard::set("AIDA_USER", "queue-fixture-user")
}

#[cfg(test)]
mod tests {
    use super::*;

    // trace:TASK-3784 | ai:codex
    #[test]
    fn owner_cleanup_runs_before_unlock_and_sees_added_keys() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static CALLED: AtomicBool = AtomicBool::new(false);
        fn cleanup(keys: &[(&'static str, Option<OsString>)]) {
            assert!(holds_env_lock());
            assert!(keys.iter().any(|(key, _)| *key == "AIDA_TESTKIT_CLEANUP"));
            CALLED.store(true, Ordering::SeqCst);
        }
        let mut guard = EnvVarsGuard::snapshot(&[]).with_cleanup(cleanup);
        guard.unset_key("AIDA_TESTKIT_CLEANUP");
        drop(guard);
        assert!(CALLED.load(Ordering::SeqCst));
        assert!(!holds_env_lock());
    }

    // The child constructor must keep the home supplied by this helper,
    // including its Git config, and must not add it to the real-home fence.
    // trace:TASK-2841 | ai:codex
    #[test]
    fn path_isolated_child_retains_assigned_home_and_fence() {
        if run_path_isolated_test() {
            return;
        }
        let home = crate::test_home::home_dir().unwrap();
        let config = std::fs::read_to_string(home.join(".gitconfig")).unwrap();
        assert!(config.contains("email = test@example.invalid"), "{config}");
        let fence = std::env::var_os(crate::test_home::REAL_HOMES_ENV).unwrap();
        assert!(
            !std::env::split_paths(&fence).any(|real| real == home),
            "the assigned temporary home must not become a fenced operator home"
        );
    }

    /// TASK-1559 AC1: a second `env_lock()` on a thread that already holds it
    /// panics immediately instead of self-deadlocking.
    // trace:TASK-1559 | ai:claude
    #[test]
    #[should_panic(expected = "ENV_LOCK already held by this thread")]
    fn nested_env_lock_panics_instead_of_deadlocking() {
        let _outer = env_lock();
        let _inner = env_lock();
    }

    /// TASK-1559 AC2: constructing an `EnvVarGuard` inside an `env_lock()`
    /// scope panics. This is the exact shape that shipped in TASK-1532 —
    /// `publish_fixture_receipt` held the lock and then set a guard under it —
    /// so it is the case the tripwire exists for.
    // trace:TASK-1559 | ai:claude
    #[test]
    #[should_panic(expected = "ENV_LOCK already held by this thread")]
    fn env_var_guard_inside_env_lock_scope_panics() {
        let _outer = env_lock();
        let _guard = EnvVarGuard::set("AIDA_TEST_GUARD_REENTRANCE", "x");
    }

    /// TASK-1559 AC2: the multi-key guard is covered too — it acquires the
    /// same lock, so nesting it is the same defect.
    // trace:TASK-1559 | ai:claude
    #[test]
    #[should_panic(expected = "ENV_LOCK already held by this thread")]
    fn env_vars_guard_inside_env_lock_scope_panics() {
        let _outer = env_lock();
        let _guard = EnvVarsGuard::set(&[("AIDA_TEST_GUARD_REENTRANCE_MULTI", "x")]);
    }

    /// TASK-1559 AC3 + AC4: the tripwire must not fire on legitimate use.
    /// Sequential same-thread acquisitions, a hand-off to another thread, and
    /// genuine cross-thread contention all still succeed.
    // trace:TASK-1559 | ai:claude
    #[test]
    fn sequential_and_cross_thread_acquisition_still_work() {
        // AC4: acquire/release/acquire on one thread.
        drop(env_lock());
        drop(env_lock());

        // AC4: released here, acquired on a different thread — ownership must
        // not be left behind pointing at this thread.
        std::thread::spawn(|| drop(env_lock())).join().unwrap();
        // ...and back again, so the other thread left nothing behind either.
        drop(env_lock());

        // AC3: cross-thread contention. The waiter signals before it tries to
        // acquire and this thread only releases afterwards. That ordering does
        // not *guarantee* the waiter is parked in the mutex when the release
        // happens, but either interleaving must complete without panicking,
        // which is the property under test — a stale or cross-thread owner
        // would trip the assert instead.
        let held = env_lock();
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            tx.send(()).unwrap();
            let _g = env_lock();
            "acquired"
        });
        rx.recv().unwrap();
        drop(held);
        assert_eq!(waiter.join().unwrap(), "acquired");

        // The lock is free and unowned again.
        drop(env_lock());
    }

    /// TASK-1559 AC5: a holder that panics must clear ownership on unwind.
    /// If it did not, this thread's next acquisition would hit the reentrance
    /// tripwire instead of succeeding — trading the old deadlock for a new
    /// spurious panic.
    // trace:TASK-1559 | ai:claude
    #[test]
    fn env_lock_ownership_is_cleared_when_a_holder_panics() {
        let outcome = std::panic::catch_unwind(|| {
            let _g = env_lock();
            panic!("poisoning the env lock intentionally");
        });
        assert!(outcome.is_err());
        drop(env_lock());
    }

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

    // trace:TASK-1970 | ai:antigravity
    #[test]
    fn scan_block_for_unisolated_cwd_flags_seeded_offenders_across_fn_visibilities() {
        let sample = r#"
#[test]
fn isolated_caller() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    std::env::set_current_dir("fixture-isolated").unwrap();
}

pub fn pub_helper_without_guard() {
    std::env::set_current_dir("fixture-pub").unwrap();
}

#[test]
fn guarded_middle() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    std::env::set_current_dir("fixture-mid").unwrap();
}

pub fn pub_crate_helper_without_guard() {
    std::env::set_current_dir("fixture-pub-crate").unwrap();
}

pub(super) async fn async_helper_without_guard() {
    std::env::set_current_dir("fixture-async").unwrap();
}

#[test]
fn guarded_by_output_variant() {
    if let Some(out) = crate::test_env::run_path_isolated_test_output() {
        return;
    }
    std::env::set_current_dir("fixture-output").unwrap();
}
"#;
        let mut offenders = Vec::new();
        scan_block_for_unisolated_cwd(Path::new("seeded.rs"), sample, 1, &mut offenders);
        assert_eq!(
            offenders,
            vec![
                "seeded.rs:11: std::env::set_current_dir(\"fixture-pub\").unwrap();",
                "seeded.rs:23: std::env::set_current_dir(\"fixture-pub-crate\").unwrap();",
                "seeded.rs:27: std::env::set_current_dir(\"fixture-async\").unwrap();",
            ]
        );
    }
}

/// Scan `src_dir` for direct `set_current_dir(` calls in test code whose
/// enclosing function does not call `run_path_isolated_test()`.
///
/// Limitation: only literal `set_current_dir(` calls in test modules/files are
/// inspected; indirect cwd mutation through production code
/// (`queue_cmd/work.rs::handle_queue_work`,
/// `session_start.rs:2516`, ...) is not detected.
// trace:TASK-1970 | ai:antigravity
pub fn scan_unisolated_set_current_dir_in_dir(src_dir: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    let mut stack = vec![src_dir.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "rs") {
                let file_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if file_name == "test_env.rs" {
                    continue;
                }
                let is_test_file = p.components().any(|c| c.as_os_str() == "tests");
                let content = match std::fs::read_to_string(&p) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                if is_test_file {
                    scan_block_for_unisolated_cwd(&p, &content, 1, &mut offenders);
                } else {
                    let mut idx = 0;
                    while idx < content.len() {
                        let rest = &content[idx..];
                        let pos = match rest.find("#[cfg(") {
                            Some(p) => idx + p,
                            None => break,
                        };
                        let bracket_close = match content[pos..].find(']') {
                            Some(b) => pos + b,
                            None => {
                                idx = pos + "#[cfg(".len();
                                continue;
                            }
                        };
                        let attr = &content[pos..=bracket_close];
                        if !attr.contains("test") {
                            idx = bracket_close + 1;
                            continue;
                        }
                        let after_cfg = bracket_close + 1;
                        if let Some(brace_at) = find_mod_block_brace(&content, after_cfg) {
                            if let Some(end_brace) = find_matching_brace(&content, brace_at) {
                                let block = &content[brace_at..end_brace];
                                let start_line = content[..brace_at].matches('\n').count() + 1;
                                scan_block_for_unisolated_cwd(
                                    &p,
                                    block,
                                    start_line,
                                    &mut offenders,
                                );
                                idx = end_brace;
                                continue;
                            }
                        }
                        idx = after_cfg;
                    }
                }
            }
        }
    }
    offenders
}

// trace:TASK-1970 | ai:antigravity
fn is_fn_item_start(trimmed: &str) -> bool {
    let mut s = trimmed.trim_start();
    if let Some(after_pub) = s.strip_prefix("pub") {
        let trimmed_after = after_pub.trim_start();
        if let Some(in_parens) = trimmed_after.strip_prefix('(') {
            let Some(close_idx) = in_parens.find(')') else {
                return false;
            };
            s = in_parens[close_idx + 1..].trim_start();
        } else if after_pub.starts_with(char::is_whitespace) {
            s = trimmed_after;
        } else {
            return false;
        }
    }
    loop {
        let mut progressed = false;
        for kw in ["const", "async", "unsafe"] {
            if let Some(rest) = s.strip_prefix(kw) {
                if rest.starts_with(char::is_whitespace) {
                    s = rest.trim_start();
                    progressed = true;
                }
            }
        }
        if let Some(rest) = s.strip_prefix("extern") {
            let rest_trimmed = rest.trim_start();
            if let Some(after_quote) = rest_trimmed.strip_prefix('"') {
                if let Some(end_quote) = after_quote.find('"') {
                    s = after_quote[end_quote + 1..].trim_start();
                    progressed = true;
                }
            } else if rest.starts_with(char::is_whitespace) {
                s = rest_trimmed;
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    s.starts_with("fn ") || s.starts_with("fn\t") || s.starts_with("fn<")
}

// trace:TASK-1970 | ai:antigravity
fn scan_block_for_unisolated_cwd(
    path: &Path,
    block: &str,
    start_line: usize,
    offenders: &mut Vec<String>,
) {
    let lines: Vec<&str> = block.lines().collect();
    let mut current_fn_has_isolated_guard = false;
    for (j, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("/*") {
            continue;
        }
        if trimmed.starts_with("#[test]") || is_fn_item_start(trimmed) {
            current_fn_has_isolated_guard = false;
        }
        // trace:TASK-2270 | ai:claude
        if line.contains("run_path_isolated_test()")
            || line.contains("run_path_isolated_test_output()")
        {
            current_fn_has_isolated_guard = true;
        }
        if line.contains("set_current_dir(") && !current_fn_has_isolated_guard {
            offenders.push(format!(
                "{}:{}: {}",
                path.display(),
                start_line + j,
                line.trim()
            ));
        }
    }
}

/// Scan `src_dir` for raw `env::set_var` / `env::remove_var` in test code.
pub fn scan_raw_env_writes_in_dir(src_dir: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    let mut stack = vec![src_dir.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "rs") {
                let file_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if file_name == "test_env.rs" || file_name == "test_home.rs" {
                    continue;
                }
                let is_test_file = p.components().any(|c| c.as_os_str() == "tests");
                let content = match std::fs::read_to_string(&p) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                let lines: Vec<&str> = content.lines().collect();
                if is_test_file {
                    for (i, line) in lines.iter().enumerate() {
                        let trimmed = line.trim_start();
                        if trimmed.starts_with("//")
                            || trimmed.starts_with("*")
                            || trimmed.starts_with("/*")
                        {
                            continue;
                        }
                        if line.contains("allow-raw-env-write:") {
                            continue;
                        }
                        if line.contains("env::set_var(") || line.contains("env::remove_var(") {
                            offenders.push(format!("{}:{}: {}", p.display(), i + 1, line.trim()));
                        }
                    }
                } else {
                    // Scan #[cfg(...test...)] mod blocks
                    let mut idx = 0;
                    while idx < content.len() {
                        let rest = &content[idx..];
                        let pos = match rest.find("#[cfg(") {
                            Some(p) => idx + p,
                            None => break,
                        };
                        let bracket_close = match content[pos..].find(']') {
                            Some(b) => pos + b,
                            None => {
                                idx = pos + "#[cfg(".len();
                                continue;
                            }
                        };
                        let attr = &content[pos..=bracket_close];
                        if !attr.contains("test") {
                            idx = bracket_close + 1;
                            continue;
                        }
                        let after_cfg = bracket_close + 1;
                        if let Some(brace_at) = find_mod_block_brace(&content, after_cfg) {
                            if let Some(end_brace) = find_matching_brace(&content, brace_at) {
                                let block = &content[brace_at..end_brace];
                                let start_line = content[..brace_at].matches('\n').count() + 1;
                                for (j, line) in block.lines().enumerate() {
                                    let trimmed = line.trim_start();
                                    if trimmed.starts_with("//")
                                        || trimmed.starts_with("*")
                                        || trimmed.starts_with("/*")
                                    {
                                        continue;
                                    }
                                    if line.contains("allow-raw-env-write:") {
                                        continue;
                                    }
                                    if line.contains("env::set_var(")
                                        || line.contains("env::remove_var(")
                                    {
                                        offenders.push(format!(
                                            "{}:{}: {}",
                                            p.display(),
                                            start_line + j,
                                            line.trim()
                                        ));
                                    }
                                }
                                idx = end_brace;
                                continue;
                            }
                        }
                        idx = after_cfg;
                    }
                }
            }
        }
    }

    offenders
}

fn find_mod_block_brace(source: &str, start: usize) -> Option<usize> {
    let mut i = start;
    let len = source.len();
    while i < len {
        let rest = &source[i..];
        if let Some(c) = rest.chars().next() {
            if c.is_whitespace() {
                i += c.len_utf8();
                continue;
            }
        }
        if rest.starts_with("//") {
            i = rest.find('\n').map_or(len, |p| i + p);
            continue;
        }
        if rest.starts_with("/*") {
            if let Some(p) = rest.find("*/") {
                i += p + 2;
                continue;
            } else {
                return None;
            }
        }
        if rest.starts_with('#') {
            if let Some(bracket_open) = rest.find('[') {
                if let Some(bracket_close) = rest[bracket_open..].find(']') {
                    i += bracket_open + bracket_close + 1;
                    continue;
                }
            }
            return None;
        }
        if rest.starts_with("mod ") || rest.starts_with("mod\t") || rest.starts_with("mod\n") {
            let semi = rest.find(';');
            let brace = rest.find('{');
            match (semi, brace) {
                (Some(s), Some(b)) if b < s => return Some(i + b),
                (None, Some(b)) => return Some(i + b),
                _ => return None,
            }
        }
        break;
    }
    None
}

fn find_matching_brace(source: &str, open_brace: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = open_brace;
    let len = source.len();

    while i < len {
        let rest = &source[i..];
        if rest.starts_with("//") {
            i = rest.find('\n').map_or(len, |p| i + p);
            continue;
        }
        if rest.starts_with("/*") {
            if let Some(p) = rest.find("*/") {
                i += p + 2;
                continue;
            } else {
                return None;
            }
        }
        if rest.starts_with('"') {
            i += 1;
            while i < len {
                if source.as_bytes()[i] == b'\\' {
                    i += 2;
                } else if source.as_bytes()[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        if rest.starts_with('\'') {
            i += 1;
            while i < len {
                if source.as_bytes()[i] == b'\\' {
                    i += 2;
                } else if source.as_bytes()[i] == b'\'' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        let c = rest.chars().next().unwrap();
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(i + 1);
            }
        }
        i += c.len_utf8();
    }
    None
}

/// Explicit HOME mode for legacy global-ID fixtures, never a production clone.
// trace:BUG-2017 | ai:codex
pub fn enroll_synthetic_home(root: &Path) -> anyhow::Result<()> {
    let resolved = root.canonicalize()?;
    anyhow::ensure!(
        resolved.starts_with(std::env::temp_dir().canonicalize()?),
        "fixture authority requires a temporary repository"
    );
    if aida_core::allocation_authority::common_dir(&resolved).is_err() {
        aida_core::git_ops::init(&resolved)?;
    }
    let common = aida_core::allocation_authority::common_dir(&resolved)?;
    anyhow::ensure!(
        common.starts_with(std::env::temp_dir().canonicalize()?),
        "fixture authority requires a temporary common Git directory"
    );
    let setting = common.join(aida_core::allocation_authority::CONFIG_FILE);
    if setting.exists() {
        aida_core::allocation_authority::require_global(&resolved)?;
    } else {
        aida_core::write_atomic(&setting, "version=1\nauthoritative=true\n")?;
        // The allocation reader refuses shared writes. Synthetic authority must
        // satisfy that contract even under a collaborative host's umask 0002.
        // Match the core fixture: shared-write umasks must not invalidate authority.
        // trace:STORY-2640 | ai:codex
        // trace:TASK-3420 trace:TASK-3749 trace:BUG-2502 | ai:codex
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&setting, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(())
}

/// Positive global-ID fixture opt-in; unlike production constructors this
/// explicitly initializes and enrolls only a temporary synthetic HOME clone.
// trace:BUG-2017 | ai:codex
pub fn home_fixture_backend(root: &Path) -> anyhow::Result<aida_core::GitBackend> {
    let backend = aida_core::GitBackend::new(root)?;
    enroll_synthetic_home(root)?;
    Ok(backend)
}

// trace:BUG-2017 | ai:codex
pub fn home_fixture_cached_backend(
    root: &Path,
    cache: &Path,
) -> anyhow::Result<aida_core::CachedGitBackend> {
    let backend = home_fixture_backend(root)?;
    aida_core::CachedGitBackend::with_inner(backend, cache)
}

/// Positive Storage adapter fixture with directory-backed global IDs.
// trace:BUG-2017 | ai:codex
pub fn home_fixture_storage(root: impl AsRef<Path>) -> aida_core::Storage {
    let root = root.as_ref();
    home_fixture_backend(root).expect("temporary synthetic HOME store");
    aida_core::Storage::new(root)
}

/// Write a fixture project's `.aida` config under `project` (a temp dir)
/// through the adoption write guard, so a fixture names no config path
/// literal of its own (BUG-1723's site inventory stays production-only).
// trace:STORY-2324 | ai:claude
#[cfg(unix)]
pub fn write_project_config_fixture(project: &Path, content: &str) {
    let path = project.join(aida_core::adoption_projection::CONFIG);
    std::fs::create_dir_all(path.parent().expect("config has a parent dir")).unwrap();
    aida_core::write_config_atomic(&path, content).unwrap();
}
