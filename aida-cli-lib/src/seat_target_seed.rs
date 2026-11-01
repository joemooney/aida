//! BUG-2383: warm a seat's cargo target directory from the primary's.
//!
//! A seat clone builds into `<main>/target/worktrees/<clone dir>`, which
//! starts empty. The host cargo wrapper seeds a linked worktree's target from
//! `<main>/target/<profile>` on its first run, but it only does so for a
//! linked worktree, and a seat clone has its own `.git`, so a lane's first
//! targeted test was a cold full build (~27 min for aida-cli-lib).
//!
//! The seed never shares an inode with the primary's target (advisor ruling,
//! BUG-2383 round 2). A contained seat's sandbox binds its own target
//! read-write, so a hardlinked file the seat opened for an in-place write
//! would change the primary's artifact, and `aida dev build` links those into
//! the live binary. The seed therefore copies only with reflinks
//! (`cp -a --reflink=always`): copy-on-write clones, so a seat's write never
//! reaches the primary's blocks, at near-zero disk cost. `cp -a` keeps
//! mtimes, which cargo's fingerprints compare. When the filesystem has no
//! reflinks (ext4, for one), nothing is seeded and the seat builds cold. That
//! is acceptable because the lane job runner has no 60-second cap. A plain
//! copy is never made: that would be the 45 GB copy this seed exists to avoid.
//!
//! Seeded: `deps/`, `.fingerprint/` and `build/`. Skipped: `incremental/`
//! (only workspace crates use it, and they recompile in a fresh checkout
//! anyway) and the top-level binaries and `examples/`, which cargo re-creates.
//!
//! Like the wrapper, the seed holds the primary's build lock
//! (`<main>/target/<profile>/.cargo-lock`) while it reads, so it never sees a
//! half-written build. It builds into `<dst>.seed-<pid>` and renames that into
//! place, so a seat never sees a partial seed and a killed seed is swept by the
//! next one.
// trace:BUG-2383 | ai:claude

use std::path::Path;
use std::time::Duration;

/// The cargo profiles a seat target is seeded for.
pub(crate) const SEED_PROFILES: &[&str] = &["agent"];

/// The profile subdirectories a seed clones. Everything else is skipped.
pub(crate) const SEEDED_DIRS: &[&str] = &["deps", ".fingerprint", "build"];

/// How long the lane job runner's seed waits for the primary's build lock
/// before it gives up (the wrapper's `AIDA_CARGO_SEED_LOCK_WAIT` default).
pub(crate) const LOCK_WAIT: Duration = Duration::from_secs(120);

/// How long a seat launch waits for it: a launch does not stall on a busy
/// primary build, and the runner tries again before the first job.
pub(crate) const LAUNCH_LOCK_WAIT: Duration = Duration::from_secs(5);

/// What one [`seed_profile`] call did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Seeded {
    /// The profile directory was created from reflinked copies.
    Reflinked,
    /// The seat already had the profile directory.
    AlreadyWarm,
    /// Nothing seeded, with the reason; the seat builds cold.
    Skipped(String),
}

/// Seed `<seat_target>/<profile>` from `<main_target>/<profile>` for every
/// profile in [`SEED_PROFILES`]. Best effort: each failure is reported in the
/// returned list and leaves the seat to build cold.
pub(crate) fn seed_seat_target(
    main_target: &Path,
    seat_target: &Path,
    lock_wait: Duration,
) -> Vec<(String, Seeded)> {
    SEED_PROFILES
        .iter()
        .map(|profile| {
            (
                (*profile).to_string(),
                seed_profile(main_target, seat_target, profile, lock_wait),
            )
        })
        .collect()
}

/// Seed one profile. `seat_target` must be the seat's own directory, never
/// `main_target` itself or inside the profile being seeded.
pub(crate) fn seed_profile(
    main_target: &Path,
    seat_target: &Path,
    profile: &str,
    lock_wait: Duration,
) -> Seeded {
    let src = main_target.join(profile);
    let dst = seat_target.join(profile);
    if dst.exists() {
        return Seeded::AlreadyWarm;
    }
    if !src.is_dir() {
        return Seeded::Skipped(format!("{} does not exist", src.display()));
    }
    if seat_target == main_target || seat_target.starts_with(&src) {
        return Seeded::Skipped("the seat target is the primary's own".into());
    }
    if let Err(e) = std::fs::create_dir_all(seat_target) {
        return Seeded::Skipped(format!("create {}: {e}", seat_target.display()));
    }
    // One seed at a time per seat target.
    let Ok(seed_lock) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(seat_target.join(".seed.lock"))
    else {
        return Seeded::Skipped("could not open the seed lock".into());
    };
    if !lock_within(&seed_lock, lock_wait) {
        return Seeded::Skipped("another seed of this target is still running".into());
    }
    if dst.exists() {
        return Seeded::AlreadyWarm;
    }
    sweep_stale_seeds(seat_target, profile);
    // The primary's build lock, as cargo itself takes it.
    let build_lock = match std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(src.join(".cargo-lock"))
    {
        Ok(f) => f,
        Err(e) => return Seeded::Skipped(format!("open the primary's build lock: {e}")),
    };
    if !lock_within(&build_lock, lock_wait) {
        return Seeded::Skipped("the primary's build is still running".into());
    }
    let tmp = seat_target.join(format!(".seed-{profile}.{}", std::process::id()));
    let result = reflink_into(&src, &tmp);
    drop(build_lock);
    match result {
        Ok(()) => match std::fs::rename(&tmp, &dst) {
            Ok(()) => {
                let _ = std::fs::write(dst.join(".cargo-lock"), b"");
                Seeded::Reflinked
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&tmp);
                Seeded::Skipped(format!("move the seed into place: {e}"))
            }
        },
        Err(why) => {
            let _ = std::fs::remove_dir_all(&tmp);
            Seeded::Skipped(why)
        }
    }
}

/// Take an exclusive lock on `file`, retrying until `wait` passes.
fn lock_within(file: &std::fs::File, wait: Duration) -> bool {
    let deadline = std::time::Instant::now() + wait;
    loop {
        if fs2::FileExt::try_lock_exclusive(file).is_ok() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Remove `.seed-<profile>.*` left by a killed seed.
fn sweep_stale_seeds(seat_target: &Path, profile: &str) {
    let prefix = format!(".seed-{profile}.");
    let Ok(entries) = std::fs::read_dir(seat_target) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Clone each of [`SEEDED_DIRS`] from `src` into `dst` with
/// `cp -a --reflink=always`, which fails rather than fall back to a copy. `Err`
/// is the reason, worded for the job output.
fn reflink_into(src: &Path, dst: &Path) -> std::result::Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("create {}: {e}", dst.display()))?;
    for dir in SEEDED_DIRS {
        let from = src.join(dir);
        if !from.is_dir() {
            continue;
        }
        let out = std::process::Command::new("cp")
            .arg("-a")
            .arg("--reflink=always")
            .arg("--")
            .arg(&from)
            .arg(dst.join(dir))
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("run cp: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let first = err.lines().next().unwrap_or("").trim();
            return Err(format!(
                "this filesystem cannot reflink, so the seat builds cold (cp: {first})"
            ));
        }
    }
    Ok(())
}

/// A one-line summary of [`seed_seat_target`]'s result, for a log and the
/// job output.
pub(crate) fn describe(results: &[(String, Seeded)]) -> String {
    results
        .iter()
        .map(|(profile, seeded)| match seeded {
            Seeded::Reflinked => format!("{profile}: seeded with reflinks (copy-on-write)"),
            Seeded::AlreadyWarm => format!("{profile}: already warm"),
            Seeded::Skipped(why) => format!("{profile}: not seeded ({why})"),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every regular file's (device, inode) under `root`.
    #[cfg(unix)]
    fn inodes(root: &Path) -> HashSet<(u64, u64)> {
        use std::os::unix::fs::MetadataExt;
        let mut out = HashSet::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let meta = std::fs::symlink_metadata(entry.path()).unwrap();
                if meta.is_dir() {
                    stack.push(entry.path());
                } else if meta.is_file() {
                    out.insert((meta.dev(), meta.ino()));
                }
            }
        }
        out
    }

    fn fixture_primary(main_target: &Path) {
        let src = main_target.join("agent");
        for dir in [
            "deps",
            ".fingerprint/foo-1",
            "build/foo-1/out",
            "incremental/foo-1",
        ] {
            std::fs::create_dir_all(src.join(dir)).unwrap();
        }
        std::fs::write(src.join("deps/libfoo-1.rlib"), b"rlib").unwrap();
        std::fs::write(src.join("deps/foo-1.d"), b"dep").unwrap();
        std::fs::write(src.join(".fingerprint/foo-1/lib"), b"fp").unwrap();
        std::fs::write(src.join("build/foo-1/out/gen.rs"), b"gen").unwrap();
        std::fs::write(src.join("incremental/foo-1/q"), b"inc").unwrap();
        std::fs::write(src.join("aida"), b"bin").unwrap();
    }

    // The ruling's test: whatever path the seed takes on this filesystem, no
    // seeded file shares an inode with the primary's target.
    #[cfg(unix)]
    #[test]
    fn seed_never_shares_an_inode_with_the_primary() {
        let tmp = tempfile::tempdir().unwrap();
        let main_target = tmp.path().join("target");
        fixture_primary(&main_target);
        let seat_target = main_target.join("worktrees").join("aida-seat-x");

        let got = seed_profile(&main_target, &seat_target, "agent", Duration::from_secs(5));
        let dst = seat_target.join("agent");
        match &got {
            Seeded::Reflinked => {
                let shared: Vec<_> = inodes(&main_target.join("agent"))
                    .intersection(&inodes(&dst))
                    .copied()
                    .collect();
                assert!(shared.is_empty(), "shared inodes: {shared:?}");
                assert_eq!(
                    std::fs::read(dst.join("deps/libfoo-1.rlib")).unwrap(),
                    b"rlib"
                );
                assert!(dst.join(".fingerprint/foo-1/lib").is_file());
                assert!(!dst.join("incremental").exists());
                assert!(!dst.join("aida").exists());
                // cp -a keeps the mtimes cargo's fingerprints compare.
                let a = std::fs::metadata(main_target.join("agent/deps/libfoo-1.rlib"))
                    .unwrap()
                    .modified()
                    .unwrap();
                let b = std::fs::metadata(dst.join("deps/libfoo-1.rlib"))
                    .unwrap()
                    .modified()
                    .unwrap();
                assert_eq!(a, b);
                // An in-place write in the seat leaves the primary untouched.
                std::fs::write(dst.join("deps/libfoo-1.rlib"), b"seat").unwrap();
                assert_eq!(
                    std::fs::read(main_target.join("agent/deps/libfoo-1.rlib")).unwrap(),
                    b"rlib"
                );
                assert_eq!(
                    seed_profile(&main_target, &seat_target, "agent", Duration::from_secs(5)),
                    Seeded::AlreadyWarm
                );
            }
            Seeded::Skipped(why) => {
                assert!(why.contains("reflink"), "{why}");
                assert!(!dst.exists(), "a failed seed leaves no partial target");
                let leftovers: Vec<_> = std::fs::read_dir(&seat_target)
                    .unwrap()
                    .flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with(".seed-"))
                    .collect();
                assert!(leftovers.is_empty(), "the temporary seed is removed");
            }
            Seeded::AlreadyWarm => panic!("a fresh seat target is not warm"),
        }
        assert!(describe(&[("agent".into(), got)]).starts_with("agent: "));
    }

    #[test]
    fn seed_skips_without_a_primary_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let got = seed_profile(
            &tmp.path().join("target"),
            &tmp.path().join("seat"),
            "agent",
            Duration::from_secs(1),
        );
        assert!(matches!(got, Seeded::Skipped(_)), "{got:?}");
    }

    #[test]
    fn describe_names_the_path_that_ran() {
        assert_eq!(
            describe(&[("agent".into(), Seeded::Reflinked)]),
            "agent: seeded with reflinks (copy-on-write)"
        );
        assert_eq!(
            describe(&[("agent".into(), Seeded::Skipped("no reflinks".into()))]),
            "agent: not seeded (no reflinks)"
        );
    }
}
