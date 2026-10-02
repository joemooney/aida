//! Test-only env var guard for `aida-core` — serialises process-global
//! env-var swaps so parallel tests don't trample each other's state.
//! Uses `crate::TEST_ENV_LOCK`.
// trace:TASK-1532 | ai:agy

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::MutexGuard;

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
pub(crate) struct EnvLockGuard {
    _inner: MutexGuard<'static, ()>,
}

impl Drop for EnvLockGuard {
    fn drop(&mut self) {
        ENV_LOCK_OWNER.store(0, Ordering::Release);
    }
}

/// Acquire the crate-wide test env lock. The lock is NOT reentrant: do not
/// nest a second acquisition (including an `EnvVarGuard`/`EnvVarsGuard`)
/// inside a scope that already holds it. TASK-1559 makes that nesting panic
/// immediately rather than deadlock.
pub(crate) fn env_lock() -> EnvLockGuard {
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
    let inner = crate::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    ENV_LOCK_OWNER.store(me, Ordering::Release);
    EnvLockGuard { _inner: inner }
}

/// RAII guard that sets (or unsets) an env var for the guard's lifetime
/// and restores the prior value on drop.
pub(crate) struct EnvVarGuard {
    key: &'static str,
    prev: Option<OsString>,
    _guard: EnvLockGuard,
}

#[allow(dead_code)]
impl EnvVarGuard {
    pub(crate) fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        let guard = env_lock();
        let prev = std::env::var_os(key);
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

    pub(crate) fn unset(key: &'static str) -> Self {
        let guard = env_lock();
        let prev = std::env::var_os(key);
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

    pub(crate) fn reset(&mut self, value: impl AsRef<OsStr>) {
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var(self.key, value);
        }
    }

    pub(crate) fn reset_unset(&mut self) {
        #[allow(unused_unsafe)]
        unsafe {
            std::env::remove_var(self.key);
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        #[allow(unused_unsafe)]
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

/// Multi-key RAII guard.
pub(crate) struct EnvVarsGuard {
    prev: Vec<(&'static str, Option<OsString>)>,
    _guard: EnvLockGuard,
}

#[allow(dead_code)]
impl EnvVarsGuard {
    pub(crate) fn set(pairs: &[(&'static str, &str)]) -> Self {
        let owned: Vec<(&'static str, Option<&str>)> =
            pairs.iter().map(|(k, v)| (*k, Some(*v))).collect();
        Self::apply(&owned)
    }

    pub(crate) fn apply(pairs: &[(&'static str, Option<&str>)]) -> Self {
        let guard = env_lock();
        let mut prev = Vec::with_capacity(pairs.len());
        for (key, value) in pairs {
            prev.push((*key, std::env::var_os(key)));
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
        }
    }

    pub(crate) fn snapshot(keys: &[&'static str]) -> Self {
        let guard = env_lock();
        let prev = keys.iter().map(|k| (*k, std::env::var_os(k))).collect();
        Self {
            prev,
            _guard: guard,
        }
    }

    pub(crate) fn set_key(&mut self, key: &'static str, value: impl AsRef<OsStr>) {
        if !self.prev.iter().any(|(k, _)| *k == key) {
            self.prev.push((key, std::env::var_os(key)));
        }
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var(key, value);
        }
    }

    pub(crate) fn unset_key(&mut self, key: &'static str) {
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
        for (key, prev) in self.prev.iter().rev() {
            #[allow(unused_unsafe)]
            unsafe {
                match prev {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

/// Scan `dir` for raw `env::set_var` / `env::remove_var` in test code.
pub(crate) fn scan_raw_env_writes_in_dir(src_dir: &Path) -> Vec<String> {
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
            // Attribute like #[path = "..."]
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
                _ => return None, // out-of-line mod declaration
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
        // Skip strings
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

#[cfg(test)]
mod tests {
    use super::*;

    /// TASK-1559 AC1: a second `env_lock()` on a thread that already holds it
    /// panics immediately instead of self-deadlocking.
    // trace:TASK-1559 | ai:claude
    #[test]
    #[should_panic(expected = "ENV_LOCK already held by this thread")]
    fn core_nested_env_lock_panics_instead_of_deadlocking() {
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
    fn core_env_var_guard_inside_env_lock_scope_panics() {
        let _outer = env_lock();
        let _guard = EnvVarGuard::set("AIDA_CORE_TEST_GUARD_REENTRANCE", "x");
    }

    /// TASK-1559 AC2: the multi-key guard is covered too — it acquires the
    /// same lock, so nesting it is the same defect.
    // trace:TASK-1559 | ai:claude
    #[test]
    #[should_panic(expected = "ENV_LOCK already held by this thread")]
    fn core_env_vars_guard_inside_env_lock_scope_panics() {
        let _outer = env_lock();
        let _guard = EnvVarsGuard::set(&[("AIDA_CORE_TEST_GUARD_REENTRANCE_MULTI", "x")]);
    }

    /// TASK-1559 AC3 + AC4: the tripwire must not fire on legitimate use.
    /// Sequential same-thread acquisitions, a hand-off to another thread, and
    /// genuine cross-thread contention all still succeed.
    // trace:TASK-1559 | ai:claude
    #[test]
    fn core_sequential_and_cross_thread_acquisition_still_work() {
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
    fn core_env_lock_ownership_is_cleared_when_a_holder_panics() {
        let outcome = std::panic::catch_unwind(|| {
            let _g = env_lock();
            panic!("poisoning the env lock intentionally");
        });
        assert!(outcome.is_err());
        drop(env_lock());
    }

    #[test]
    fn core_env_guard_restores_and_survives_poison() {
        const KEY: &str = "AIDA_CORE_TEST_GUARD_RESTORE";
        let outer = EnvVarGuard::set(KEY, "outer");
        assert_eq!(std::env::var(KEY).unwrap(), "outer");
        drop(outer);
        assert!(std::env::var(KEY).is_err());

        // Survives poison test
        let _ = std::panic::catch_unwind(|| {
            let _g = env_lock();
            panic!("poisoning lock intentionally");
        });
        let g = EnvVarGuard::set(KEY, "recovered");
        assert_eq!(std::env::var(KEY).unwrap(), "recovered");
        drop(g);
        assert!(std::env::var(KEY).is_err());
    }

    #[test]
    fn core_no_raw_env_writes_in_test_code() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut offenders = scan_raw_env_writes_in_dir(&manifest_dir.join("src"));
        if manifest_dir.join("tests").is_dir() {
            offenders.extend(scan_raw_env_writes_in_dir(&manifest_dir.join("tests")));
        }
        assert!(
            offenders.is_empty(),
            "found raw env::set_var / remove_var in aida-core test code outside helpers:\n{}",
            offenders.join("\n")
        );
    }
}
