//! BUG-2383 part 2: a warm seat target on a filesystem without reflinks.
//!
//! On ext4 the reflink seed ([`crate::seat_target_seed`]) cannot run, so
//! every lane's first build was cold. Instead, the lane-job sandbox sees its
//! profile directory (`<seat target>/agent`) as a kernel overlayfs:
//!
//! - **lower**: a read-only *generation snapshot* of the primary's
//!   `target/agent`, `<main>/target/lane-base/agent-<gen>/`. `deps/` and
//!   `build/` are hardlinked, `.fingerprint/` is really copied (cargo rewrites
//!   fingerprints in place), `incremental/` is skipped. The snapshot is taken
//!   under the primary's `.cargo-lock`, so it never sees a half-written build.
//! - **upper/work**: per lane, `<seat target>/overlay/{upper,work}`, kept
//!   between jobs (the second job is warm) and discarded when the lane moves
//!   to another generation.
//!
//! The lower layer is read-only and every write, in place or not, copies up
//! to a new inode in the upper layer, so no seat write ever reaches a file of
//! the primary's target (the round-2 inode invariant).
//!
//! bubblewrap 0.9 (Ubuntu's) has no `--overlay`, so the job runs as
//! `unshare -Urm sh -c 'mount -t overlay ...; exec bwrap ...'`: the overlay is
//! mounted in a private user and mount namespace and the lane-job bwrap
//! profile runs unchanged inside it (advisor ruling on fork 2, 2026-11-02
//! 06:36). If unshare or the mount fails, the seat builds cold and the job
//! output says `seed: cold (<reason>)`; the lane never fails over it.
//!
//! A generation is pinned by each lease that uses it
//! (`<gen>/.pins/<lease id>`) and garbage-collected once no pin names a lease
//! that still exists.
// trace:BUG-2383 | ai:claude

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where generation snapshots live, under the primary's `target/`.
pub(crate) const LANE_BASE: &str = "lane-base";
/// A lane's overlay directory, under its seat target.
pub(crate) const OVERLAY_DIR: &str = "overlay";
/// The file in [`OVERLAY_DIR`] naming the generation its upper was built on.
const GENERATION_RECORD: &str = "generation";
/// The pins directory inside a generation.
const PINS: &str = ".pins";
/// The (name, size, mtime) manifest of a generation's hardlinked `deps/`.
const MANIFEST: &str = ".manifest.json";
/// Hardlinked into a snapshot.
const LINKED_DIRS: &[&str] = &["deps", "build"];
/// Copied into a snapshot.
const COPIED_DIRS: &[&str] = &[".fingerprint"];

/// One generation snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Generation {
    pub(crate) id: String,
    pub(crate) dir: PathBuf,
}

/// A lane's overlay: mount `lower` with `upper`/`work` at `dest`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaneOverlay {
    pub(crate) generation: String,
    pub(crate) lower: PathBuf,
    pub(crate) upper: PathBuf,
    pub(crate) work: PathBuf,
    pub(crate) dest: PathBuf,
}

/// A short stable hash of `bytes` (FNV-1a, 64 bit), for generation ids.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The generation id of `<main_target>/<profile>`: the newest fingerprint
/// mtime and a hash of the toolchain cargo recorded in
/// `<main_target>/.rustc_info.json`. `None` when the profile has no
/// fingerprints (nothing to warm from).
pub(crate) fn generation_id(main_target: &Path, profile: &str) -> Option<String> {
    let fingerprints = main_target.join(profile).join(".fingerprint");
    let newest = std::fs::read_dir(&fingerprints)
        .ok()?
        .flatten()
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()?;
    let secs = newest
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let toolchain = std::fs::read(main_target.join(".rustc_info.json")).unwrap_or_default();
    Some(format!("{profile}-{secs}-{:08x}", fnv(&toolchain) as u32))
}

/// Take an exclusive lock on `file` within `wait`.
fn lock_within(file: &std::fs::File, wait: Duration) -> bool {
    let deadline = std::time::Instant::now() + wait;
    loop {
        if fs2::FileExt::try_lock_exclusive(file).is_ok() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn open_lock(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
}

/// `cp -a [-l] -- <from> <to>`; `Err` is the first line cp printed.
fn cp(from: &Path, to: &Path, link: bool) -> Result<(), String> {
    let mut cmd = std::process::Command::new("cp");
    cmd.arg("-a");
    if link {
        cmd.arg("-l");
    }
    let out = cmd
        .arg("--")
        .arg(from)
        .arg(to)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("run cp: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    Err(format!(
        "cp {}: {}",
        from.display(),
        err.lines().next().unwrap_or("").trim()
    ))
}

/// The (name, size, mtime seconds) of every file in `deps`, sorted.
fn deps_manifest(deps: &Path) -> Vec<(String, u64, i64)> {
    use std::os::unix::fs::MetadataExt;
    let mut out: Vec<(String, u64, i64)> = std::fs::read_dir(deps)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| {
                (
                    e.file_name().to_string_lossy().into_owned(),
                    m.len(),
                    m.mtime(),
                )
            })
        })
        .collect();
    out.sort();
    out
}

/// Create (or reuse) the generation snapshot of `<main_target>/<profile>`
/// under the primary's build lock. `Err` is the reason, worded for the job
/// output.
pub(crate) fn snapshot(
    main_target: &Path,
    profile: &str,
    lock_wait: Duration,
) -> Result<Generation, String> {
    let src = main_target.join(profile);
    if !src.is_dir() {
        return Err(format!("{} does not exist", src.display()));
    }
    let id = generation_id(main_target, profile)
        .ok_or_else(|| format!("{} has no fingerprints", src.display()))?;
    let base = main_target.join(LANE_BASE);
    std::fs::create_dir_all(&base).map_err(|e| format!("create {}: {e}", base.display()))?;
    let dir = base.join(&id);
    if dir.is_dir() {
        return Ok(Generation { id, dir });
    }
    // One snapshot at a time.
    let base_lock =
        open_lock(&base.join(".lock")).map_err(|e| format!("open the lane-base lock: {e}"))?;
    if !lock_within(&base_lock, lock_wait) {
        return Err("another generation snapshot is still running".into());
    }
    if dir.is_dir() {
        return Ok(Generation { id, dir });
    }
    let build_lock = open_lock(&src.join(".cargo-lock"))
        .map_err(|e| format!("open the primary's build lock: {e}"))?;
    if !lock_within(&build_lock, lock_wait) {
        return Err("the primary's build is still running".into());
    }
    let tmp = base.join(format!(".{id}.tmp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let built = (|| {
        std::fs::create_dir_all(&tmp).map_err(|e| format!("create {}: {e}", tmp.display()))?;
        for (dirs, link) in [(LINKED_DIRS, true), (COPIED_DIRS, false)] {
            for d in dirs {
                let from = src.join(d);
                if from.is_dir() {
                    cp(&from, &tmp.join(d), link)?;
                }
            }
        }
        let manifest = serde_json::to_vec(&deps_manifest(&tmp.join("deps")))
            .map_err(|e| format!("manifest: {e}"))?;
        std::fs::write(tmp.join(MANIFEST), manifest).map_err(|e| format!("manifest: {e}"))?;
        std::fs::create_dir_all(tmp.join(PINS)).map_err(|e| format!("pins: {e}"))
    })();
    drop(build_lock);
    match built.and_then(|()| {
        std::fs::rename(&tmp, &dir).map_err(|e| format!("move the snapshot into place: {e}"))
    }) {
        Ok(()) => Ok(Generation { id, dir }),
        Err(why) => {
            let _ = std::fs::remove_dir_all(&tmp);
            Err(why)
        }
    }
}

/// Pin `generation` for `lease_id`.
pub(crate) fn pin(generation: &Generation, lease_id: &str) -> Result<(), String> {
    let pins = generation.dir.join(PINS);
    std::fs::create_dir_all(&pins).map_err(|e| format!("pins: {e}"))?;
    std::fs::write(pins.join(lease_id), b"").map_err(|e| format!("pin: {e}"))
}

/// Remove every generation under `<main_target>/lane-base` that no live
/// lease pins, except `keep`. A pin is live while
/// `<main_root>/.aida/sessions/<lease id>.toml` exists. Returns the removed
/// directories.
pub(crate) fn gc(main_target: &Path, main_root: &Path, keep: Option<&Path>) -> Vec<PathBuf> {
    let sessions = main_root.join(".aida").join("sessions");
    let mut removed = Vec::new();
    let Ok(entries) = std::fs::read_dir(main_target.join(LANE_BASE)) else {
        return removed;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !dir.is_dir() || Some(dir.as_path()) == keep {
            continue;
        }
        let pins = dir.join(PINS);
        let mut live = false;
        for pin in std::fs::read_dir(&pins).into_iter().flatten().flatten() {
            let lease = pin.file_name().to_string_lossy().into_owned();
            if sessions.join(format!("{lease}.toml")).is_file() {
                live = true;
            } else {
                let _ = std::fs::remove_file(pin.path());
            }
        }
        if !live && std::fs::remove_dir_all(&dir).is_ok() {
            removed.push(dir);
        }
    }
    removed
}

/// The lane's overlay for `generation`, with its upper and work directories
/// ready. An upper built on another generation is discarded.
pub(crate) fn prepare_lane(
    seat_target: &Path,
    profile: &str,
    generation: &Generation,
) -> Result<LaneOverlay, String> {
    let dir = seat_target.join(OVERLAY_DIR);
    let upper = dir.join("upper");
    let work = dir.join("work");
    let record = dir.join(GENERATION_RECORD);
    let recorded = std::fs::read_to_string(&record).ok();
    if recorded.as_deref().map(str::trim) != Some(generation.id.as_str()) {
        for d in [&upper, &work] {
            if d.exists() {
                std::fs::remove_dir_all(d).map_err(|e| format!("reset {}: {e}", d.display()))?;
            }
        }
    }
    let dest = seat_target.join(profile);
    for d in [&upper, &work, &dest] {
        std::fs::create_dir_all(d).map_err(|e| format!("create {}: {e}", d.display()))?;
    }
    std::fs::write(&record, &generation.id).map_err(|e| format!("record: {e}"))?;
    Ok(LaneOverlay {
        generation: generation.id.clone(),
        lower: generation.dir.clone(),
        upper,
        work,
        dest,
    })
}

/// Whether the seat target already runs on an overlay (a previous lane on
/// this seat set it up), so an empty mountpoint is not mistaken for a warm
/// target.
pub(crate) fn lane_has_overlay(seat_target: &Path) -> bool {
    seat_target
        .join(OVERLAY_DIR)
        .join(GENERATION_RECORD)
        .is_file()
}

/// The overlayfs mount option string, refusing paths overlayfs would split.
fn mount_options(ov: &LaneOverlay) -> Result<String, String> {
    for p in [&ov.lower, &ov.upper, &ov.work] {
        let s = p.to_string_lossy();
        if s.contains([',', ':', '\\']) {
            return Err(format!(
                "{} holds a character overlayfs cannot take",
                p.display()
            ));
        }
    }
    Ok(format!(
        "lowerdir={},upperdir={},workdir={}",
        ov.lower.display(),
        ov.upper.display(),
        ov.work.display()
    ))
}

/// The shell script that mounts the overlay then execs the rest of argv.
const MOUNT_THEN_EXEC: &str =
    "mount -t overlay overlay -o \"$1\" \"$2\" || exit 125; shift 2; exec \"$@\"";

/// Wrap `program args` (the lane-job bwrap) so it runs with the overlay
/// mounted: `unshare -Urm sh -c <mount; exec> sh <opts> <dest> program args`.
pub(crate) fn wrap(
    unshare: &str,
    ov: &LaneOverlay,
    program: &str,
    args: Vec<String>,
) -> Result<(String, Vec<String>), String> {
    let mut full = vec![
        "-Urm".to_string(),
        "--".into(),
        "/bin/sh".into(),
        "-c".into(),
        MOUNT_THEN_EXEC.into(),
        "sh".into(),
        mount_options(ov)?,
        ov.dest.display().to_string(),
        program.to_string(),
    ];
    full.extend(args);
    Ok((unshare.to_string(), full))
}

/// Mount the overlay once in a throwaway namespace and run `true`, so a host
/// that refuses unprivileged user namespaces or overlay mounts falls back to
/// cold before the first job. `Err` is the reason.
pub(crate) fn probe(unshare: &str, ov: &LaneOverlay) -> Result<(), String> {
    let (program, args) = wrap(unshare, ov, "true", Vec::new())?;
    let out = std::process::Command::new(&program)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("run {program}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    Err(format!(
        "the overlay mount failed (exit {:?}): {}",
        out.status.code(),
        err.lines().next().unwrap_or("").trim()
    ))
}

/// Whether a hardlinked `deps/` file of `generation` changed since its
/// snapshot (the primary rewrote it in place), which can make the seat's
/// build inconsistent. Only the seat is affected; the job output says
/// `seed: overlay-stale`.
pub(crate) fn stale(lower: &Path) -> bool {
    let Ok(recorded) = std::fs::read(lower.join(MANIFEST)) else {
        return false;
    };
    let Ok(recorded) = serde_json::from_slice::<Vec<(String, u64, i64)>>(&recorded) else {
        return false;
    };
    recorded != deps_manifest(&lower.join("deps"))
}

#[cfg(test)]
#[path = "tests/bug_2383_seat_target_overlay_tests.rs"]
mod tests;
