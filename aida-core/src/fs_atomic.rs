//! Atomic file writes — stage in a sibling temp file, then `rename(2)` it
//! over the target.
//!
//! POSIX (and Windows) `rename` is atomic for the *content* the reader
//! observes: a successful read can never see a half-written or
//! byte-interleaved file. Under a write race the failure mode degrades from
//! torn, unparseable content — the corruption BUG-228 hit on a role TOML
//! file when two `aida` processes interleaved their bytes — to a clean
//! last-writer-wins.
//!
//! Use [`write_atomic`] instead of `std::fs::write` on any path that more
//! than one `aida` process (or thread) can touch at once. The autonomous
//! drain workflow runs several `aida` processes concurrently as a matter of
//! course, so the dispenser counters, session manifests, and node registry
//! are all routinely contended.
//!
//! ## Windows: the transient-open caveat
//!
//! POSIX `rename(2)` is atomic for the *open* too — a concurrent `open`
//! always sees old-or-new. Windows file replacement
//! (`MoveFileExW` / `ReplaceFileW`) has a brief window where a reader's
//! `CreateFile` racing a writer's atomic replace can fail with
//! `ERROR_ACCESS_DENIED` (`PermissionDenied`) or `ERROR_FILE_NOT_FOUND`
//! (`NotFound`). The *content* invariant still holds — these are
//! "couldn't open it at all," not "opened a torn version." A retry
//! immediately succeeds. BUG-246 surfaced this in the
//! `concurrent_writers_never_tear_the_file` test on `windows-latest`.
//!
//! Use [`read_atomic`] for reads of paths that may be racing a
//! [`write_atomic`]: it wraps `std::fs::read_to_string` in a bounded retry
//! over those two transient errors and is a no-op on Linux / macOS where
//! they don't occur. Production code paths the autonomous drain hits — the
//! dispenser counter, agreed-counters, the node registry, object-store
//! YAML, session manifests, the workspace manifest — all read through
//! `read_atomic`. trace:TASK-346 | ai:claude
//!
//! trace:TASK-331 | ai:claude

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Wall-clock retry budget for [`read_atomic`]. The Windows transient
/// rename-race window closes in microseconds, so this is multiple orders of
/// magnitude more than required. Exhausting it means the error is real, not
/// transient.
//
// BUG-1654: the budget used to be an attempt count (200 × 1 ms). On a loaded
// runner every `thread::sleep` overshoots, so 200 "1 ms" sleeps took over
// 2 s. The primary bound is now this deadline, measured from the first
// attempt; the attempt cap below is only a secondary ceiling.
// trace:TASK-346 | ai:claude
// trace:BUG-1654 | ai:claude
const READ_ATOMIC_BUDGET: Duration = Duration::from_millis(200);

/// Backoff between [`read_atomic`] retries.
const READ_ATOMIC_BACKOFF: Duration = Duration::from_millis(1);

/// Secondary ceiling on [`read_atomic`] attempts, so a clock that never
/// advances (or sleeps that return early) still terminates.
// trace:BUG-1654 | ai:claude
const READ_ATOMIC_MAX_ATTEMPTS: u32 = 200;

/// Process-global counter making every staging temp name unique, so two
/// threads of the *same* process racing the same target never collide on
/// the temp path (the PID alone only disambiguates across processes).
static STAGING_SEQ: AtomicU64 = AtomicU64::new(0);

/// Write `content` to `path` atomically.
///
/// Stages the content in a sibling temp file, then `rename`s it over the
/// target. The temp name carries the writer's PID plus a process-global
/// sequence number so neither two processes nor two threads racing the same
/// target collide on the staging path. On any error the temp file is cleaned
/// up so a failed write leaves no litter behind.
///
/// Drop-in replacement for [`std::fs::write`] on concurrent-writer paths —
/// same `(path, content)` shape, same `io::Result`.
///
/// trace:TASK-331 | ai:claude
pub fn write_atomic(path: &Path, content: impl AsRef<[u8]>) -> std::io::Result<()> {
    let seq = STAGING_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp.{}.{}", std::process::id(), seq));
    if let Err(e) = std::fs::write(&tmp, content) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Read `path` to a `String`, retrying transient open failures that occur
/// while a concurrent [`write_atomic`] is replacing the file on Windows.
///
/// Drop-in replacement for [`std::fs::read_to_string`] on paths that may be
/// racing an atomic write — same `Path` input, same `io::Result<String>`
/// output. The retry catches the two `CreateFile` failure modes Windows
/// surfaces during the brief `MoveFileExW` / `ReplaceFileW` window
/// (`PermissionDenied` = `ERROR_ACCESS_DENIED`, `NotFound` =
/// `ERROR_FILE_NOT_FOUND`); every other error returns immediately. On
/// Linux / macOS those errors don't occur on a contended-but-existing path,
/// so this is a no-op there.
///
/// A genuinely missing file still surfaces as `NotFound` — the retry is
/// bounded by a wall-clock deadline (about 200 ms, plus at most one sleep's
/// overshoot) and a secondary attempt cap, so an exhausted retry returns
/// the last error and callers' existing missing-file handling works
/// unchanged.
///
/// trace:TASK-346 | ai:claude
pub fn read_atomic(path: &Path) -> std::io::Result<String> {
    read_atomic_with(path, Instant::now, std::thread::sleep)
}

/// [`read_atomic`] with an injectable clock and sleep, so the wall-clock
/// budget can be tested deterministically against overshooting sleeps.
// trace:BUG-1654 | ai:claude
fn read_atomic_with(
    path: &Path,
    mut now: impl FnMut() -> Instant,
    mut sleep: impl FnMut(Duration),
) -> std::io::Result<String> {
    let deadline = now() + READ_ATOMIC_BUDGET;
    let mut attempts = 0u32;
    loop {
        match std::fs::read_to_string(path) {
            Ok(s) => return Ok(s),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound
                ) =>
            {
                attempts += 1;
                let remaining = deadline.saturating_duration_since(now());
                if remaining.is_zero() || attempts >= READ_ATOMIC_MAX_ATTEMPTS {
                    return Err(e);
                }
                sleep(READ_ATOMIC_BACKOFF.min(remaining));
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        read_atomic, read_atomic_with, write_atomic, READ_ATOMIC_BACKOFF, READ_ATOMIC_BUDGET,
        READ_ATOMIC_MAX_ATTEMPTS,
    };
    use std::cell::Cell;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    // write_atomic replaces content cleanly and leaves no temp file behind.
    #[test]
    fn replaces_content_no_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("state");
        std::fs::create_dir_all(&sub).unwrap();
        let path = sub.join("dispenser.toml");
        write_atomic(&path, "first").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
        write_atomic(&path, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        let leftovers: Vec<_> = std::fs::read_dir(&sub)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "atomic write left a temp file behind");
    }

    // Accepts both &str and owned String / &[u8] — the same content shapes
    // the converted call sites pass (toml/yaml serialize -> String).
    #[test]
    fn accepts_str_string_and_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        write_atomic(&path, "borrowed").unwrap();
        write_atomic(&path, String::from("owned")).unwrap();
        write_atomic(&path, b"bytes".as_slice()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "bytes");
    }

    // read_atomic returns the file contents unchanged on the happy path —
    // it is a drop-in for fs::read_to_string when the file is stable.
    #[test]
    fn read_atomic_reads_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        write_atomic(&path, "key = \"value\"").unwrap();
        assert_eq!(read_atomic(&path).unwrap(), "key = \"value\"");
    }

    // A genuinely missing file still surfaces NotFound after the retry
    // budget exhausts — the helper bounds its waiting, then returns the
    // last error so callers' existing missing-file handling works.
    // ~200 ms wall-clock is the cap; the test gives generous slack. The
    // deterministic overshoot case is `bug_1654_*` below.
    #[test]
    fn read_atomic_returns_notfound_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist");
        let start = std::time::Instant::now();
        let err = read_atomic(&path).expect_err("missing file must error");
        let elapsed = start.elapsed();
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::NotFound,
            "missing file should surface as NotFound after retry exhaustion"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "retry budget should be bounded, got {elapsed:?}"
        );
    }

    // BUG-1654: the budget is wall-clock, not an attempt count. A fake clock
    // whose every "1 ms" sleep actually advances 50 ms (a badly loaded
    // runner) must stop at the deadline — a handful of attempts — instead of
    // running all 200 attempts (10 s of virtual time). Deterministic: no real
    // sleeping, no dependence on the host's scheduler.
    // trace:BUG-1654 | ai:claude
    #[test]
    fn bug_1654_budget_holds_when_sleeps_overshoot() {
        const OVERSHOOT: std::time::Duration = std::time::Duration::from_millis(50);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist");
        let start = std::time::Instant::now();
        let virtual_elapsed = Cell::new(std::time::Duration::ZERO);
        let sleeps = Cell::new(0u32);
        let err = read_atomic_with(
            &path,
            || start + virtual_elapsed.get(),
            |requested| {
                assert!(
                    requested <= READ_ATOMIC_BACKOFF,
                    "backoff grew: {requested:?}"
                );
                sleeps.set(sleeps.get() + 1);
                virtual_elapsed.set(virtual_elapsed.get() + OVERSHOOT);
            },
        )
        .expect_err("missing file must error");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        // Bounded by the deadline plus at most one overshooting sleep.
        assert!(
            virtual_elapsed.get() <= READ_ATOMIC_BUDGET + OVERSHOOT,
            "wall-clock budget exceeded: {:?}",
            virtual_elapsed.get()
        );
        // 200 ms / 50 ms per sleep = 4 sleeps, not 199.
        assert_eq!(sleeps.get(), 4, "retry should stop at the deadline");
    }

    // BUG-1654: with an accurate clock the full nominal budget is still
    // spent retrying (backoff semantics unchanged), and a clock that never
    // advances still terminates at the secondary attempt cap.
    // trace:BUG-1654 | ai:claude
    #[test]
    fn bug_1654_accurate_and_frozen_clocks_keep_backoff_semantics() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist");
        let start = std::time::Instant::now();

        // Accurate clock: each sleep advances exactly what was requested.
        let elapsed = Cell::new(std::time::Duration::ZERO);
        let sleeps = Cell::new(0u32);
        let err = read_atomic_with(
            &path,
            || start + elapsed.get(),
            |d| {
                sleeps.set(sleeps.get() + 1);
                elapsed.set(elapsed.get() + d);
            },
        )
        .expect_err("missing file must error");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(sleeps.get(), READ_ATOMIC_MAX_ATTEMPTS - 1);
        assert!(elapsed.get() <= READ_ATOMIC_BUDGET);

        // Frozen clock: the attempt cap is the backstop.
        let frozen_sleeps = Cell::new(0u32);
        let err = read_atomic_with(
            &path,
            || start,
            |_| frozen_sleeps.set(frozen_sleeps.get() + 1),
        )
        .expect_err("missing file must error");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(frozen_sleeps.get(), READ_ATOMIC_MAX_ATTEMPTS - 1);
    }

    // BUG-1654: a file that appears mid-retry (the Windows transient window
    // closing) is read successfully through the injected path.
    // trace:BUG-1654 | ai:claude
    #[test]
    fn bug_1654_transient_missing_file_is_read_after_retry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("appears.toml");
        let start = std::time::Instant::now();
        let elapsed = Cell::new(std::time::Duration::ZERO);
        let got = read_atomic_with(
            &path,
            || start + elapsed.get(),
            |d| {
                elapsed.set(elapsed.get() + d);
                if elapsed.get() >= std::time::Duration::from_millis(3) {
                    write_atomic(&path, "ok").unwrap();
                }
            },
        )
        .expect("file appears within the budget");
        assert_eq!(got, "ok");
    }

    // Concurrent-writer storm against read_atomic — the helper itself, not
    // an inline retry loop in the reader. Every observed content is one of
    // the writers' payloads (no torn read), and the reader never panics on
    // a transient open (the helper's whole point on Windows). Mirrors
    // `concurrent_writers_never_tear_the_file` but exercises the public
    // helper end-to-end. trace:TASK-346 | ai:claude
    #[test]
    fn read_atomic_handles_concurrent_writes() {
        const WRITERS: usize = 6;
        const ROUNDS: usize = 50;
        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().join("racy.toml"));

        let payloads: Vec<String> = (0..WRITERS)
            .map(|i| format!("writer-{:02}-", i).repeat(128))
            .collect();
        let valid: HashSet<String> = payloads.iter().cloned().collect();
        write_atomic(&path, payloads[0].as_str()).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let reader = {
            let path = Arc::clone(&path);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let observed = read_atomic(&path)
                        .expect("read_atomic must absorb transient open failures");
                    assert!(
                        valid.contains(&observed),
                        "reader observed a torn write ({} bytes)",
                        observed.len()
                    );
                }
            })
        };

        let writers: Vec<_> = (0..WRITERS)
            .map(|i| {
                let path = Arc::clone(&path);
                let payload = payloads[i].clone();
                std::thread::spawn(move || {
                    for _ in 0..ROUNDS {
                        write_atomic(&path, payload.as_str()).unwrap();
                    }
                })
            })
            .collect();

        for w in writers {
            w.join().unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        reader.join().unwrap();
    }

    // AC6: concurrent-writer stress test. N threads hammer one path; a reader
    // thread checks every observed state is one COMPLETE written value, never
    // a byte-interleaved mix. A bare `std::fs::write` fails this — that is the
    // exact torn-write corruption BUG-228 hit.
    #[test]
    fn concurrent_writers_never_tear_the_file() {
        const WRITERS: usize = 8;
        const ROUNDS: usize = 60;
        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().join("contended.toml"));

        // Distinct, equal-length payloads: a torn write yields content that
        // matches none of them, so the reader's set membership check catches
        // any interleaving unambiguously.
        let payloads: Vec<String> = (0..WRITERS)
            .map(|i| format!("writer-{:02}-", i).repeat(256))
            .collect();
        let valid: HashSet<String> = payloads.iter().cloned().collect();
        write_atomic(&path, payloads[0].as_str()).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let reader = {
            let path = Arc::clone(&path);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut reads = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    // A concurrent open can transiently fail on Windows
                    // (ERROR_ACCESS_DENIED / ERROR_FILE_NOT_FOUND) while a
                    // writer's atomic rename is in flight — POSIX rename has
                    // no such window. That is not a torn write, so retry it;
                    // a torn write surfaces instead as a *successful* read of
                    // out-of-set content, caught by the assert below.
                    // trace:BUG-246 | ai:claude
                    let observed = {
                        let mut attempts = 0u32;
                        loop {
                            match std::fs::read_to_string(&*path) {
                                Ok(s) => break s,
                                Err(e)
                                    if matches!(
                                        e.kind(),
                                        std::io::ErrorKind::PermissionDenied
                                            | std::io::ErrorKind::NotFound
                                    ) =>
                                {
                                    attempts += 1;
                                    assert!(
                                        attempts < 200,
                                        "reader: transient read failure persisted: {e}"
                                    );
                                    std::thread::sleep(std::time::Duration::from_millis(1));
                                }
                                Err(e) => {
                                    panic!("reader: unexpected read error: {e}")
                                }
                            }
                        }
                    };
                    assert!(
                        valid.contains(&observed),
                        "reader observed a torn write ({} bytes)",
                        observed.len()
                    );
                    reads += 1;
                }
                reads
            })
        };

        let writers: Vec<_> = (0..WRITERS)
            .map(|i| {
                let path = Arc::clone(&path);
                let payload = payloads[i].clone();
                std::thread::spawn(move || {
                    for _ in 0..ROUNDS {
                        write_atomic(&path, payload.as_str()).unwrap();
                    }
                })
            })
            .collect();

        for w in writers {
            w.join().unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        reader.join().unwrap();

        // No staging temp files survive the storm.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "concurrent writes left temp files behind"
        );
    }

    // AC7: grep guard. The known-concurrent write paths converted by TASK-331
    // must not regress to a bare `fs::write` — atomic rename is the only safe
    // shape there. Scans production code (everything before the test module)
    // and fails loudly with a nudge toward write_atomic.
    #[test]
    fn known_concurrent_paths_stay_atomic() {
        // Files whose production write paths TASK-331 converted. Their test
        // modules legitimately use fs::write for fixtures, so only the code
        // before the first `#[cfg(test)]` is scanned.
        const CONCURRENT_FILES: &[&str] =
            &["src/dispenser.rs", "src/git_ops.rs", "src/registry.rs"];
        let crate_root = env!("CARGO_MANIFEST_DIR");
        for rel in CONCURRENT_FILES {
            let path = std::path::Path::new(crate_root).join(rel);
            let src = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("guard cannot read {}: {e}", path.display()));
            let production = src.split("#[cfg(test)]").next().unwrap_or(&src);
            for (n, line) in production.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                assert!(
                    !line.contains("fs::write("),
                    "{}:{} uses a bare fs::write on a known-concurrent path — \
                     use aida_core::write_atomic instead (torn-write race, TASK-331)",
                    rel,
                    n + 1
                );
            }
        }
    }

    // TASK-346: grep guard for the *read* side. The contended production
    // reads converted to `read_atomic` must not regress to a bare
    // `fs::read_to_string` — on Windows a concurrent `write_atomic` makes
    // those transiently fail with PermissionDenied/NotFound. Scans the
    // same production-code-only slice and fails loudly with a nudge
    // toward read_atomic.
    #[test]
    fn known_concurrent_paths_use_read_atomic() {
        // Files whose production read paths TASK-346 converted. Each reads
        // a path that pairs with a `write_atomic` writer (its own, or
        // git's rename-based plumbing on the orphan store).
        const CONCURRENT_READ_FILES: &[&str] = &[
            "src/dispenser.rs",
            "src/git_ops.rs",
            "src/registry.rs",
            "src/object_store.rs",
            "src/workspace.rs",
        ];
        let crate_root = env!("CARGO_MANIFEST_DIR");
        for rel in CONCURRENT_READ_FILES {
            let path = std::path::Path::new(crate_root).join(rel);
            let src = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("guard cannot read {}: {e}", path.display()));
            let production = src.split("#[cfg(test)]").next().unwrap_or(&src);
            for (n, line) in production.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                assert!(
                    !line.contains("fs::read_to_string("),
                    "{}:{} uses a bare fs::read_to_string on a \
                     known-concurrent path — use aida_core::read_atomic instead \
                     (Windows transient-open race, TASK-346)",
                    rel,
                    n + 1
                );
            }
        }
    }
}
