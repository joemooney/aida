# RefreshLock load sensitivity — investigation record and reproduction harness

Date: 2026-09-29 · Spec: BUG-1729 (follow-on from TASK-1526 / PR #2283)

## Why this document exists

PR #2283 was blocked by a single CI failure in
`db::cache_refresh::tests::nested_freshen_in_lock_holder_does_not_wait`
(`cache_refresh.rs:427:9`) that no quiet local machine reproduces. This records
what was eliminated, what was found instead, and keeps the reproduction harness
that must not be lost — it is proven to catch a real defect that the shipped
tests miss.

## Eliminated, with evidence — do not redo

| Hypothesis | Verdict | Evidence |
|---|---|---|
| Cross-test sidecar collision | Ruled out | `cache_sidecar_path` -> `shared_cache_path` returns the path unchanged when it is not a symlink, so the sidecar is `<tempdir>/cache.db.refresh.lock`, unique per test |
| `flock` emulated via POSIX record locks (released by closing any fd) | Ruled out | `/tmp` is ext4 (`df -T /tmp`); `flock` is real and per-open-file-description |
| Misclassified errno | Ruled out | `is_lock_contended` accepts only `WouldBlock` or an exact raw-OS match; anything else propagates as `Err` and panics differently |
| Registry branch as the source of the CI `Ok(None)` | Ruled out by consequence | The only entry at that key is owned by the main thread; had it survived `drop(inner)`, the final call would have matched `owner` and returned `Some` — the assertion would have PASSED, not failed |
| External host load alone | Ruled out | The out-of-process probe test passed 60/60 in isolation with the host loaded; the failure requires concurrently running in-process tests |
| Ambient stress on the `cache_refresh::` module alone | Useless as a strategy | 120 consecutive clean runs in session #9; the filter excluded `cached_git_backend::`, where the interference actually shows |

## What reproduced

Filter on `db::cache`, which prefix-matches BOTH `db::cache_refresh::` and
`db::cached_git_backend::`. Four concurrent workers on a 6-core host:

```bash
BIN=target/debug/deps/aida_core-<hash>
for w in 1 2 3 4; do "$BIN" db::cache --test-threads=8 & done
```

`db::cached_git_backend::tests::single_flight_four_readers_one_refresh` failed
4/4 workers on the first iteration (`cached_git_backend.rs:3573`, then `:3582`).
Solo the run takes ~140s; under 4x contention ~360s, so readers exhaust the
1500ms `ReadBudget` before the refresh lands and serve stale rows. See BUG-1729
observation A.

## Reproduction harness — NOT shipped, and why

The test below probes the `flock` from a SEPARATE process, which shares no open
file descriptions with the test process. It is the only check that can observe
the descriptor lifecycle at all; every shipped test only ever re-reads the
in-process registry.

It is **proven load-bearing**. Injecting this defect into the nested branch of
`try_acquire`:

```rust
existing.depth += 1;
existing._file = file;   // closes the LOCKED descriptor, registers an unlocked one
```

makes it fail with "the nested acquire's second descriptor released the
holder's flock when it closed", while BOTH shipped tests —
`nested_freshen_in_lock_holder_does_not_wait` and
`nested_refresh_through_directory_alias_uses_same_registry_entry` — still PASS.
That is a real coverage gap.

It is not shipped because the test itself failed twice at host load ~26 and has
not reproduced since, so it cannot yet be told apart from a genuine transient
single-flight defect (BUG-1729 observation B). Shipping a test that may flake
into an approved PR would trade one unexplained CI failure for another.

```rust
    /// Ask a separate process, which shares no open file descriptions with
    /// this one, whether the sidecar is flocked. `flock` is owned by the open
    /// file description, so a nested acquire opening and closing a SECOND
    /// descriptor must leave the holder's lock intact, and the registry's own
    /// descriptor must close when depth reaches zero. Both are invisible to an
    /// in-process assertion, which only ever re-reads the registry.
    // trace:TASK-1526 | ai:claude
    #[cfg(unix)]
    #[test]
    fn refresh_lock_descriptor_lifecycle_is_visible_to_another_process() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("cache.db");
        let sidecar = super::super::cache_lock::cache_sidecar_path(&path, "refresh.lock");

        let outer = RefreshLock::try_acquire(&path).unwrap().unwrap();
        assert!(!externally_lockable(&sidecar), "holder's flock is not visible outside the process");
        let inner = RefreshLock::try_acquire(&path).unwrap().unwrap();
        assert!(inner.is_nested());
        assert!(
            !externally_lockable(&sidecar),
            "the nested acquire's second descriptor released the holder's flock when it closed; {}",
            lock_diag(&sidecar)
        );
        drop(outer);
        assert!(
            !externally_lockable(&sidecar),
            "depth fell to one but the flock was already released"
        );
        drop(inner);
        assert!(
            externally_lockable(&sidecar),
            "the last guard dropped but its descriptor outlived it"
        );
    }

    /// `LOCK_NB` from a child process: `free` means nothing in this process
    /// still holds the sidecar.
    // trace:TASK-1526 | ai:claude
    #[cfg(unix)]
    #[cfg(unix)]
    fn lock_diag(sidecar: &Path) -> String {
        use std::os::unix::fs::MetadataExt;
        let md = std::fs::metadata(sidecar);
        let ino = md.as_ref().map(|m| (m.dev(), m.ino()));
        let locks = std::fs::read_to_string("/proc/locks").unwrap_or_default();
        let ino_s = ino.map(|(_, i)| format!("{i:x}")).unwrap_or_default();
        let mine: Vec<&str> = locks
            .lines()
            .filter(|l| l.contains(&format!(":{ino_s} ")) || l.contains(&ino_s))
            .collect();
        format!(
            "pid={} sidecar={sidecar:?} dev_ino={ino:?} proc_locks_for_inode={mine:?}",
            std::process::id()
        )
    }

    #[cfg(unix)]
    fn externally_lockable(sidecar: &Path) -> bool {
        let script = r#"
import fcntl, sys
f = open(sys.argv[1], "a")
try:
    fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
    print("free")
except OSError:
    print("locked")
"#;
        let out = std::process::Command::new("python3")
            .arg("-c")
            .arg(script)
            .arg(sidecar)
            .output()
            .unwrap();
        assert!(out.status.success(), "probe failed: {out:?}");
        match String::from_utf8_lossy(&out.stdout).trim() {
            "free" => true,
            "locked" => false,
            other => panic!("unexpected probe output {other:?}"),
        }
    }
```

Diagnostic used alongside it (prints dev/ino and the matching `/proc/locks`
rows, which distinguishes "lock genuinely released" from "sidecar was replaced
by a different inode"):

```rust
#[cfg(unix)]
fn lock_diag(sidecar: &Path) -> String {
    use std::os::unix::fs::MetadataExt;
    let md = std::fs::metadata(sidecar);
    let ino = md.as_ref().map(|m| (m.dev(), m.ino()));
    let locks = std::fs::read_to_string("/proc/locks").unwrap_or_default();
    let ino_s = ino.map(|(_, i)| format!("{i:x}")).unwrap_or_default();
    let mine: Vec<&str> = locks.lines().filter(|l| l.contains(&ino_s)).collect();
    format!(
        "pid={} sidecar={sidecar:?} dev_ino={ino:?} proc_locks_for_inode={mine:?}",
        std::process::id()
    )
}
```

## What DID ship with TASK-1526

The flaking assertion is now self-diagnosing. `RefreshLock::diagnose_unavailable`
reports which of the two `Ok(None)` sources fired — a foreign-owned registry
entry, or a contended `flock` — plus the raw errno. A recurrence in CI is now
root-caused from the log alone, without reproduction.

## Gotcha for the next session

A stop-predicate like `grep -q "<test name>" log && grep -q panicked log` is
WRONG: the test name also appears on its passing `... ok` line, so any other
test's panic reads as a hit. Anchor on the harness's own failure lines
(`^test <name> ... FAILED`, or the `---- <name> stdout ----` block).
