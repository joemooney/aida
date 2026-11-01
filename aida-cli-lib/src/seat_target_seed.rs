//! BUG-2383: warm a seat's cargo target directory from the primary's.
//!
//! A seat clone builds into `<main>/target/worktrees/<clone dir>`, which
//! starts empty. The host cargo wrapper seeds a linked worktree's target from
//! `<main>/target/<profile>` on its first run, but it only does so for a
//! linked worktree, and a seat clone has its own `.git`, so a lane's first
//! targeted test was a cold full build (~27 min for aida-cli-lib).
//!
//! The seed runs on the host, where no tool-call cap applies. It does not copy
//! the whole 45 GB profile directory. The wrapper copies with `cp -a` and
//! never hardlinks, because rustc and cargo rewrite some files in place and a
//! shared inode would corrupt the primary's cache. This seed hardlinks only the
//! files that are replaced, never rewritten in place, and copies or skips the
//! rest:
//!
//! - `deps/` artifacts (`.rlib`, `.rmeta`, `.so`, test and binary
//!   executables): hardlinked. rustc writes them to a temporary file and
//!   renames it into place, and the linker unlinks its output before it
//!   writes, so a rebuild in the seat gets a new inode and never touches the
//!   primary's file.
//! - `deps/*.d` dep-info, `.fingerprint/` and `build/` (about 90 MB in all):
//!   copied. cargo rewrites them in place, and a build script may rewrite its
//!   `OUT_DIR` files in place.
//! - `incremental/`: skipped. Incremental state is rewritten in place, and
//!   only workspace crates use it. They recompile in a fresh checkout anyway,
//!   because its mtimes are newer than the copied fingerprints.
//! - Top-level files (the hardlinked binaries cargo puts beside `deps/`) and
//!   `examples/`: skipped. cargo re-creates them.
//!
//! Like the wrapper, the seed holds the primary's build lock
//! (`<main>/target/<profile>/.cargo-lock`) while it reads, so it never sees a
//! half-written build. It builds into `<dst>.seed-<pid>` and renames that into
//! place, so a seat never sees a partial seed and a killed seed is swept by the
//! next one.
// trace:BUG-2383 | ai:claude

use std::path::{Path, PathBuf};
use std::time::Duration;

/// The cargo profiles a seat target is seeded for.
pub(crate) const SEED_PROFILES: &[&str] = &["agent"];

/// How long the lane job runner's seed waits for the primary's build lock
/// before it gives up (the wrapper's `AIDA_CARGO_SEED_LOCK_WAIT` default).
pub(crate) const LOCK_WAIT: Duration = Duration::from_secs(120);

/// How long a seat launch waits for it: a launch does not stall on a busy
/// primary build, and the runner tries again before the first job.
pub(crate) const LAUNCH_LOCK_WAIT: Duration = Duration::from_secs(5);

/// What one [`seed_profile`] call did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Seeded {
    /// The profile directory was created. `linked` files share the primary's
    /// inode; `copied` files are the seat's own.
    Done { linked: u64, copied: u64 },
    /// The seat already had the profile directory.
    AlreadyWarm,
    /// Nothing to seed from, or the seed could not run, with the reason.
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

/// Seed one profile. See the module docs for what is linked, copied and
/// skipped. `seat_target` must be the seat's own directory, never
/// `main_target` itself or inside it.
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
    let result = seed_into(&src, &tmp);
    drop(build_lock);
    match result {
        Ok((linked, copied)) => match std::fs::rename(&tmp, &dst) {
            Ok(()) => {
                let _ = std::fs::write(dst.join(".cargo-lock"), b"");
                Seeded::Done { linked, copied }
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&tmp);
                Seeded::Skipped(format!("move the seed into place: {e}"))
            }
        },
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            Seeded::Skipped(format!("seed failed: {e}"))
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

/// How one entry of a profile directory is seeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Plan {
    Link,
    Copy,
    Skip,
}

/// The plan for `rel`, a path relative to the profile directory. Pure, so the
/// rule is tested without a filesystem.
pub(crate) fn plan_for(rel: &Path) -> Plan {
    let mut parts = rel.components();
    let Some(top) = parts.next() else {
        return Plan::Skip;
    };
    let top = top.as_os_str().to_string_lossy();
    let nested = parts.next().is_some();
    match top.as_ref() {
        ".fingerprint" | "build" => Plan::Copy,
        "deps" if nested => {
            if rel.extension().is_some_and(|e| e == "d") {
                Plan::Copy
            } else {
                Plan::Link
            }
        }
        "deps" => Plan::Copy,
        _ => Plan::Skip,
    }
}

/// Recreate `src` at `dst` by [`plan_for`]. Returns (linked, copied) counts.
fn seed_into(src: &Path, dst: &Path) -> std::io::Result<(u64, u64)> {
    std::fs::create_dir_all(dst)?;
    let mut linked = 0;
    let mut copied = 0;
    let mut stack: Vec<PathBuf> = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        for entry in std::fs::read_dir(src.join(&rel))? {
            let entry = entry?;
            let rel_child = rel.join(entry.file_name());
            let plan = plan_for(&rel_child);
            if plan == Plan::Skip {
                continue;
            }
            let file_type = entry.file_type()?;
            let target = dst.join(&rel_child);
            if file_type.is_dir() {
                std::fs::create_dir_all(&target)?;
                stack.push(rel_child);
            } else if file_type.is_symlink() {
                let link = std::fs::read_link(entry.path())?;
                #[cfg(unix)]
                std::os::unix::fs::symlink(link, &target)?;
                #[cfg(not(unix))]
                let _ = link;
                copied += 1;
            } else if plan == Plan::Link {
                // Never fall back to a copy: that would be the 45 GB copy
                // this seed exists to avoid (a hardlink fails across devices,
                // as inside a sandbox's bind mounts). The seat builds cold.
                std::fs::hard_link(entry.path(), &target)?;
                linked += 1;
            } else {
                std::fs::copy(entry.path(), &target)?;
                copied += 1;
            }
        }
    }
    Ok((linked, copied))
}

/// A one-line summary of [`seed_seat_target`]'s result, for a log.
pub(crate) fn describe(results: &[(String, Seeded)]) -> String {
    results
        .iter()
        .map(|(profile, seeded)| match seeded {
            Seeded::Done { linked, copied } => {
                format!("{profile}: seeded ({linked} linked, {copied} copied)")
            }
            Seeded::AlreadyWarm => format!("{profile}: already warm"),
            Seeded::Skipped(why) => format!("{profile}: not seeded ({why})"),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_links_only_replaced_artifacts() {
        assert_eq!(plan_for(Path::new("deps/libfoo-1.rlib")), Plan::Link);
        assert_eq!(plan_for(Path::new("deps/libfoo-1.rmeta")), Plan::Link);
        assert_eq!(plan_for(Path::new("deps/aida_cli_lib-1")), Plan::Link);
        assert_eq!(plan_for(Path::new("deps/foo-1.d")), Plan::Copy);
        assert_eq!(plan_for(Path::new("deps")), Plan::Copy);
        assert_eq!(plan_for(Path::new(".fingerprint/foo-1/lib")), Plan::Copy);
        assert_eq!(plan_for(Path::new("build/foo-1/out/x.rs")), Plan::Copy);
        assert_eq!(plan_for(Path::new("incremental/foo-1/s-x")), Plan::Skip);
        assert_eq!(plan_for(Path::new("incremental")), Plan::Skip);
        assert_eq!(plan_for(Path::new("aida")), Plan::Skip);
        assert_eq!(plan_for(Path::new("examples/x")), Plan::Skip);
    }

    #[test]
    fn seed_links_deps_copies_metadata_and_never_shares_a_rewritten_file() {
        let tmp = tempfile::tempdir().unwrap();
        let main_target = tmp.path().join("target");
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
        let seat_target = main_target.join("worktrees").join("aida-seat-x");

        let got = seed_profile(&main_target, &seat_target, "agent", Duration::from_secs(5));
        assert_eq!(
            got,
            Seeded::Done {
                linked: 1,
                copied: 3
            }
        );
        let dst = seat_target.join("agent");
        assert_eq!(
            std::fs::read(dst.join("deps/libfoo-1.rlib")).unwrap(),
            b"rlib"
        );
        assert!(!dst.join("incremental").exists());
        assert!(!dst.join("aida").exists());

        // An in-place rewrite of a copied file in the seat leaves the
        // primary's file untouched.
        std::fs::write(dst.join(".fingerprint/foo-1/lib"), b"seat").unwrap();
        std::fs::write(dst.join("deps/foo-1.d"), b"seat").unwrap();
        assert_eq!(
            std::fs::read(src.join(".fingerprint/foo-1/lib")).unwrap(),
            b"fp"
        );
        assert_eq!(std::fs::read(src.join("deps/foo-1.d")).unwrap(), b"dep");

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let a = std::fs::metadata(src.join("deps/libfoo-1.rlib")).unwrap();
            let b = std::fs::metadata(dst.join("deps/libfoo-1.rlib")).unwrap();
            assert_eq!(a.ino(), b.ino(), "deps artifacts are hardlinked");
        }

        // A second seed is a no-op.
        assert_eq!(
            seed_profile(&main_target, &seat_target, "agent", Duration::from_secs(5)),
            Seeded::AlreadyWarm
        );
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
}
