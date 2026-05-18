//! Atomic file writes — stage in a sibling temp file, then `rename(2)` it
//! over the target.
//!
//! POSIX (and Windows) `rename` is atomic: a concurrent reader or writer can
//! never observe a half-written or byte-interleaved file. Under a write race
//! the failure mode degrades from torn, unparseable content — the corruption
//! BUG-228 hit on a role TOML file when two `aida` processes interleaved
//! their bytes — to a clean last-writer-wins.
//!
//! Use [`write_atomic`] instead of `std::fs::write` on any path that more
//! than one `aida` process (or thread) can touch at once. The autonomous
//! drain workflow runs several `aida` processes concurrently as a matter of
//! course, so the dispenser counters, session manifests, and node registry
//! are all routinely contended.
//!
//! trace:TASK-331 | ai:claude

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

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

#[cfg(test)]
mod tests {
    use super::write_atomic;
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
                    let observed = std::fs::read_to_string(&*path).unwrap();
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
}
