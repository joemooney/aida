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
//!   Cargo 1.100's unit layout stores fingerprints inside `build/`; snapshots
//!   copy that entire directory to keep its mutable unit state off shared inodes.
//! - **upper/work**: per lane, `<main>/target/lane-overlay/<lease>/{upper,work}`,
//!   kept between jobs (the second job is warm) and discarded when the lane
//!   moves to another generation. They live beside `lane-base/`, outside every
//!   job- and seat-writable root: a job binds its seat target read-write, and
//!   the overlay mount runs outside bubblewrap, so an upper a job could reach
//!   would let it plant a symlink that redirects the next job's writes onto
//!   the host (strict review, P1). The mount destination
//!   `<seat target>/<profile>` is in the job's reach, so it is refused when it
//!   is not a real directory.
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
//! that still exists. Snapshot-and-pin and the collection both run under the
//! lane-base lock, so a collection never removes a generation between its
//! snapshot and its pin.
// trace:BUG-2383 | ai:claude

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where generation snapshots live, under the primary's `target/`.
pub(crate) const LANE_BASE: &str = "lane-base";
/// Where each lane's upper, work and generation record live, under the
/// primary's `target/`: `<main>/target/lane-overlay/<lease id>/`.
pub(crate) const LANE_OVERLAY: &str = "lane-overlay";
/// The file in a lane's overlay directory naming the generation its upper
/// was built on.
const GENERATION_RECORD: &str = "generation";
/// The pins directory inside a generation.
const PINS: &str = ".pins";
/// The (name, size, mtime) manifest of a generation's hardlinked `deps/`.
const MANIFEST: &str = ".manifest.json";
/// Recursive manifest for copied Cargo 1.100 unit state and outputs.
const BUILD_MANIFEST: &str = ".build-manifest.json";
/// The directories every complete generation holds.
const REQUIRED_DIRS: &[&str] = &["deps", "build", ".fingerprint"];
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

/// FNV-1a (64 bit), fed incrementally, for generation ids.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    fn feed(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
        // A field separator, so ("ab", "c") and ("a", "bc") differ.
        self.0 = (self.0 ^ 0xff).wrapping_mul(0x0100_0000_01b3);
    }
}

/// A file's modification time in nanoseconds since the epoch: a rebuild
/// that rewrites a file within the same second still changes it.
fn mtime_ns(m: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    m.mtime()
        .saturating_mul(1_000_000_000)
        .saturating_add(m.mtime_nsec())
}

/// Fingerprint files at most this large are hashed by content.
const HASHED_FINGERPRINT_BYTES: u64 = 64 * 1024;

/// Cargo 1.100 stores each unit's fingerprints beside its outputs under
/// build/<package>/<hash>/fingerprint instead of .fingerprint/<unit>.
/// Keep this bounded to the two layout levels; never follow directory links.
// trace:BUG-2539 | ai:codex
fn build_fingerprints(src: &Path) -> Option<Vec<PathBuf>> {
    let build = src.join("build");
    if !build.exists() {
        return Some(Vec::new());
    }
    let mut units = Vec::new();
    for package in std::fs::read_dir(build).ok()? {
        let package = package.ok()?;
        if !package.file_type().ok()?.is_dir() {
            continue;
        }
        for unit in std::fs::read_dir(package.path()).ok()? {
            let unit = unit.ok()?;
            if unit.file_type().ok()?.is_dir() {
                let fingerprint = unit.path().join("fingerprint");
                if std::fs::symlink_metadata(&fingerprint).is_ok_and(|m| m.is_dir()) {
                    units.push(fingerprint);
                }
            }
        }
    }
    units.sort();
    Some(units)
}

/// Metadata of the new layout's outputs and mutable state. Unlike the legacy
/// deps manifest this covers nested files, without following directory links.
// trace:BUG-2539 | ai:codex
fn build_manifest(root: &Path) -> Option<Vec<(String, u64, i64)>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).ok()?;
            if metadata.is_dir() {
                pending.push(path);
            } else {
                files.push((
                    path.strip_prefix(root).ok()?.to_str()?.to_string(),
                    metadata.len(),
                    mtime_ns(&metadata),
                ));
            }
        }
    }
    files.sort();
    Some(files)
}

/// The generation id of `<main_target>/<profile>`, derived from what a
/// snapshot would hold (BUG-2383 review r7, F1): the toolchain cargo
/// recorded in `<main_target>/.rustc_info.json`, every fingerprint file's
/// name, size, nanosecond mtime and (small ones) content, and the
/// [`deps_manifest`] of `deps/`. A rebuild that rewrites a fingerprint or an
/// artifact, even in place within one second, yields a new id, so it never
/// lands on a generation a live lane still uses. `None` when the profile
/// has no fingerprints (nothing to warm from). The Cargo 1.100 layout also
/// includes nested unit fingerprints and a recursive build/ metadata manifest.
// trace:BUG-2383 | ai:claude
// trace:BUG-2539 | ai:codex
pub(crate) fn generation_id(main_target: &Path, profile: &str) -> Option<String> {
    let src = main_target.join(profile);
    let mut units: Vec<PathBuf> = match std::fs::read_dir(src.join(".fingerprint")) {
        Ok(entries) => entries
            .map(|e| e.ok().map(|e| e.path()))
            .collect::<Option<_>>()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(_) => return None,
    };
    let build_units = build_fingerprints(&src)?;
    let new_layout = !build_units.is_empty();
    units.extend(build_units);
    if units.is_empty() {
        return None;
    }
    units.sort();
    let mut h = Fnv::new();
    h.feed(&std::fs::read(main_target.join(".rustc_info.json")).unwrap_or_default());
    for unit in units {
        if unit.parent() == Some(src.join(".fingerprint").as_path()) {
            // Preserve existing legacy generation identities and their live pins.
            h.feed(unit.file_name()?.as_encoded_bytes());
        } else {
            h.feed(unit.strip_prefix(&src).ok()?.as_os_str().as_encoded_bytes());
        }
        let mut files: Vec<_> = std::fs::read_dir(&unit)
            .into_iter()
            .flatten()
            .flatten()
            .collect();
        files.sort_by_key(|e| e.file_name());
        for file in files {
            let Ok(m) = std::fs::symlink_metadata(file.path()) else {
                continue;
            };
            h.feed(file.file_name().as_encoded_bytes());
            h.feed(&m.len().to_le_bytes());
            h.feed(&mtime_ns(&m).to_le_bytes());
            if m.is_file() && m.len() <= HASHED_FINGERPRINT_BYTES {
                h.feed(&std::fs::read(file.path()).unwrap_or_default());
            }
        }
    }
    for (name, len, mtime) in deps_manifest(&src.join("deps")) {
        h.feed(name.as_bytes());
        h.feed(&len.to_le_bytes());
        h.feed(&mtime.to_le_bytes());
    }
    if new_layout {
        h.feed(b"cargo-unit-layout-v1");
        for (name, len, mtime) in build_manifest(&src.join("build"))? {
            h.feed(name.as_bytes());
            h.feed(&len.to_le_bytes());
            h.feed(&mtime.to_le_bytes());
        }
    }
    Some(format!("{profile}-{:016x}", h.0))
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

/// The (name, size, mtime nanoseconds) of every file in `deps`, sorted.
fn deps_manifest(deps: &Path) -> Vec<(String, u64, i64)> {
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
                    mtime_ns(&m),
                )
            })
        })
        .collect();
    out.sort();
    out
}

/// Whether `dir` is a complete generation: a real directory (not a
/// symlink) holding every [`REQUIRED_DIRS`] entry and a manifest that
/// still matches its `deps/` ([`deps_manifest`]). A generation a collection
/// removed half-way, one missing a dependency file, or any other leftover,
/// is not. Copied new-layout build/ state has its own recursive manifest.
// trace:BUG-2383 | ai:claude
fn complete(dir: &Path) -> bool {
    // trace:BUG-2539 | ai:codex
    let build_complete = if dir.join(BUILD_MANIFEST).exists() {
        std::fs::read(dir.join(BUILD_MANIFEST))
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<(String, u64, i64)>>(&b).ok())
            .is_some_and(|recorded| build_manifest(&dir.join("build")).as_ref() == Some(&recorded))
    } else {
        build_fingerprints(dir).is_some_and(|units| units.is_empty())
    };
    build_complete
        && std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir())
        && REQUIRED_DIRS
            .iter()
            .all(|d| std::fs::symlink_metadata(dir.join(d)).is_ok_and(|m| m.is_dir()))
        && std::fs::symlink_metadata(dir.join(MANIFEST)).is_ok_and(|m| m.is_file())
        && std::fs::read(dir.join(MANIFEST))
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<(String, u64, i64)>>(&b).ok())
            .is_some_and(|recorded| recorded == deps_manifest(&dir.join("deps")))
}

/// How many directories one generation id may take (`<id>`, `<id>-r1`, ...)
/// before a snapshot gives up: each extra one is an incomplete generation a
/// live lane still pins.
const MAX_SLOTS: usize = 16;

/// The directory names generation `id` may live under, in order.
fn slots<'a>(base: &'a Path, id: &str) -> impl Iterator<Item = (String, PathBuf)> + 'a {
    let id = id.to_string();
    (0..MAX_SLOTS).map(move |n| {
        let name = if n == 0 {
            id.clone()
        } else {
            format!("{id}-r{n}")
        };
        let dir = base.join(&name);
        (name, dir)
    })
}

/// A [`complete`] generation for `id` that already exists, if any. An
/// incomplete one is skipped, never touched.
fn reusable(base: &Path, id: &str) -> Option<Generation> {
    for (name, dir) in slots(base, id) {
        if std::fs::symlink_metadata(&dir).is_err() {
            return None;
        }
        if complete(&dir) {
            return Some(Generation { id: name, dir });
        }
    }
    None
}

/// Create (or reuse) the generation snapshot of `<main_target>/<profile>`
/// under the primary's build lock, and pin it for `lease_id`, all under the
/// lane-base lock (strict review, P2): a concurrent [`gc`] waits for that
/// lock, so it always sees the pin.
///
/// An existing generation is never replaced or removed here (review r7,
/// F1): its id is derived from its content ([`generation_id`]), so changed
/// content gets a new id, and an existing generation under the same id that
/// is not [`complete`] (the primary rewrote one of its hardlinked files, or
/// a collection stopped half-way) may still be a live lane's lower layer, so
/// the new snapshot takes the next free `<id>-r<n>` instead. Only [`gc`]
/// removes a generation, once no live lease pins it. The lease's pins on
/// other generations are dropped: its lane moves to this one. `Err` is the
/// reason, worded for the job output.
// trace:BUG-2383 | ai:claude
pub(crate) fn snapshot(
    main_target: &Path,
    profile: &str,
    lease_id: &str,
    lock_wait: Duration,
) -> Result<Generation, String> {
    if !crate::lane_job::valid_id(lease_id) {
        return Err(format!("`{lease_id}` is not a lease id"));
    }
    let src = main_target.join(profile);
    if !src.is_dir() {
        return Err(format!("{} does not exist", src.display()));
    }
    let no_fingerprints = || format!("{} has no fingerprints", src.display());
    let base = main_target.join(LANE_BASE);
    std::fs::create_dir_all(&base).map_err(|e| format!("create {}: {e}", base.display()))?;
    let base_lock =
        open_lock(&base.join(".lock")).map_err(|e| format!("open the lane-base lock: {e}"))?;
    if !lock_within(&base_lock, lock_wait) {
        return Err("another generation snapshot is still running".into());
    }
    let id = generation_id(main_target, profile).ok_or_else(no_fingerprints)?;
    if let Some(generation) = reusable(&base, &id) {
        pin(&base, &generation, lease_id)?;
        return Ok(generation);
    }
    let build_lock = open_lock(&src.join(".cargo-lock"))
        .map_err(|e| format!("open the primary's build lock: {e}"))?;
    if !lock_within(&build_lock, lock_wait) {
        return Err("the primary's build is still running".into());
    }
    // Under the build lock the primary holds still: this id names exactly
    // what is copied.
    let id = generation_id(main_target, profile).ok_or_else(no_fingerprints)?;
    if let Some(generation) = reusable(&base, &id) {
        drop(build_lock);
        pin(&base, &generation, lease_id)?;
        return Ok(generation);
    }
    let Some((name, dir)) =
        slots(&base, &id).find(|(_, dir)| std::fs::symlink_metadata(dir).is_err())
    else {
        return Err(format!(
            "{MAX_SLOTS} incomplete generations of {id} are still pinned"
        ));
    };
    let generation = Generation { id: name, dir };
    let tmp = base.join(format!(".{}.tmp-{}", generation.id, std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    // New-layout build/ contains fingerprints and other mutable unit state.
    // Copy it all: hardlinking it would let a primary rebuild mutate a live lower.
    // trace:BUG-2539 | ai:codex
    let new_layout = !build_fingerprints(&src)
        .ok_or_else(|| "cannot read build fingerprints".to_string())?
        .is_empty();
    let built = (|| {
        std::fs::create_dir_all(&tmp).map_err(|e| format!("create {}: {e}", tmp.display()))?;
        for (dirs, link) in [(LINKED_DIRS, true), (COPIED_DIRS, false)] {
            for d in dirs {
                let from = src.join(d);
                if from.is_dir() {
                    cp(&from, &tmp.join(d), link && !(new_layout && *d == "build"))?;
                }
            }
        }
        // A primary without build scripts has no build/: the generation
        // still holds every required directory, so it reads as complete.
        for d in REQUIRED_DIRS {
            std::fs::create_dir_all(tmp.join(d)).map_err(|e| format!("create {d}: {e}"))?;
        }
        let manifest = serde_json::to_vec(&deps_manifest(&tmp.join("deps")))
            .map_err(|e| format!("manifest: {e}"))?;
        std::fs::write(tmp.join(MANIFEST), manifest).map_err(|e| format!("manifest: {e}"))?;
        if new_layout {
            let files = build_manifest(&tmp.join("build"))
                .ok_or_else(|| "cannot read build snapshot".to_string())?;
            let manifest = serde_json::to_vec(&files).map_err(|e| format!("manifest: {e}"))?;
            std::fs::write(tmp.join(BUILD_MANIFEST), manifest)
                .map_err(|e| format!("build manifest: {e}"))?;
        }
        std::fs::create_dir_all(tmp.join(PINS)).map_err(|e| format!("pins: {e}"))
    })();
    drop(build_lock);
    // `rename` onto a path that appeared meanwhile fails rather than
    // replacing it; only this process creates slots under the base lock.
    match built.and_then(|()| {
        std::fs::rename(&tmp, &generation.dir)
            .map_err(|e| format!("move the snapshot into place: {e}"))
    }) {
        Ok(()) => {
            pin(&base, &generation, lease_id)?;
            Ok(generation)
        }
        Err(why) => {
            let _ = std::fs::remove_dir_all(&tmp);
            Err(why)
        }
    }
}

/// Remove `path` whatever it is, never following a symlink.
fn remove_any(path: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(path)?.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Pin `generation` for `lease_id` and drop the lease's pins on every
/// other generation under `base`: a lane uses one lower layer at a time
/// ([`prepare_lane`] discards an upper built on another one). Called by
/// [`snapshot`] under the lane-base lock.
// trace:BUG-2383 | ai:claude
fn pin(base: &Path, generation: &Generation, lease_id: &str) -> Result<(), String> {
    let pins = generation.dir.join(PINS);
    std::fs::create_dir_all(&pins).map_err(|e| format!("pins: {e}"))?;
    std::fs::write(pins.join(lease_id), b"").map_err(|e| format!("pin: {e}"))?;
    for entry in std::fs::read_dir(base).into_iter().flatten().flatten() {
        let dir = entry.path();
        if dir != generation.dir && !entry.file_name().to_string_lossy().starts_with('.') {
            let _ = std::fs::remove_file(dir.join(PINS).join(lease_id));
        }
    }
    Ok(())
}

/// Under the lane-base lock (waiting at most `lock_wait`, else collecting
/// nothing), remove every generation under `<main_target>/lane-base` that no
/// live lease pins, except `keep`, and every lane overlay under
/// `<main_target>/lane-overlay` whose lease is gone. A lease is live while
/// `<main_root>/.aida/sessions/<lease id>.toml` exists. Returns the removed
/// directories.
// trace:BUG-2383 | ai:claude
pub(crate) fn gc(
    main_target: &Path,
    main_root: &Path,
    keep: Option<&Path>,
    lock_wait: Duration,
) -> Vec<PathBuf> {
    let sessions = main_root.join(".aida").join("sessions");
    let live = |lease: &str| sessions.join(format!("{lease}.toml")).is_file();
    let mut removed = Vec::new();
    let base = main_target.join(LANE_BASE);
    let Ok(base_lock) = open_lock(&base.join(".lock")) else {
        return removed;
    };
    if !lock_within(&base_lock, lock_wait) {
        return removed;
    }
    for entry in std::fs::read_dir(&base).into_iter().flatten().flatten() {
        let dir = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !dir.is_dir() || Some(dir.as_path()) == keep {
            continue;
        }
        let pins = dir.join(PINS);
        let mut pinned = false;
        for pin in std::fs::read_dir(&pins).into_iter().flatten().flatten() {
            if live(&pin.file_name().to_string_lossy()) {
                pinned = true;
            } else {
                let _ = std::fs::remove_file(pin.path());
            }
        }
        if !pinned && std::fs::remove_dir_all(&dir).is_ok() {
            removed.push(dir);
        }
    }
    for entry in std::fs::read_dir(main_target.join(LANE_OVERLAY))
        .into_iter()
        .flatten()
        .flatten()
    {
        let lease = entry.file_name().to_string_lossy().into_owned();
        if !live(&lease) && remove_any(&entry.path()).is_ok() {
            removed.push(entry.path());
        }
    }
    removed
}

/// `<main_target>/lane-overlay/<lease_id>`: the host-only home of a lane's
/// upper, work and generation record.
pub(crate) fn lane_overlay_dir(main_target: &Path, lease_id: &str) -> PathBuf {
    main_target.join(LANE_OVERLAY).join(lease_id)
}

/// Create `path` as a real directory, refusing a symlink or other file at
/// it: `mkdir` never follows a final symlink, and the result is opened with
/// `O_NOFOLLOW | O_DIRECTORY` to prove what is there now.
// trace:BUG-2383 | ai:claude
fn real_dir(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            return Err(format!(
                "{} is not a directory (a symlink?); the overlay is not mounted on it",
                path.display()
            ))
        }
        Err(_) => match std::fs::create_dir(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("create {}: {e}", path.display())),
        },
    }
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(path)
        .map(drop)
        .map_err(|e| {
            format!(
                "{} is not a directory (a symlink?); the overlay is not mounted on it: {e}",
                path.display()
            )
        })
}

/// Lane `lease_id`'s overlay for `generation`: its upper and work
/// directories ready under [`lane_overlay_dir`] (an upper built on another
/// generation is discarded), and its mount destination
/// `<seat_target>/<profile>` a real directory.
// trace:BUG-2383 | ai:claude
pub(crate) fn prepare_lane(
    main_target: &Path,
    seat_target: &Path,
    profile: &str,
    lease_id: &str,
    generation: &Generation,
) -> Result<LaneOverlay, String> {
    if !crate::lane_job::valid_id(lease_id) {
        return Err(format!("`{lease_id}` is not a lease id"));
    }
    let root = main_target.join(LANE_OVERLAY);
    std::fs::create_dir_all(main_target)
        .map_err(|e| format!("create {}: {e}", main_target.display()))?;
    real_dir(&root)?;
    let dir = root.join(lease_id);
    real_dir(&dir)?;
    let upper = dir.join("upper");
    let work = dir.join("work");
    let record = dir.join(GENERATION_RECORD);
    let recorded = std::fs::read_to_string(&record).ok();
    if recorded.as_deref().map(str::trim) != Some(generation.id.as_str()) {
        for d in [&upper, &work] {
            if std::fs::symlink_metadata(d).is_ok() {
                remove_any(d).map_err(|e| format!("reset {}: {e}", d.display()))?;
            }
        }
    }
    real_dir(&upper)?;
    real_dir(&work)?;
    std::fs::create_dir_all(seat_target)
        .map_err(|e| format!("create {}: {e}", seat_target.display()))?;
    let dest = seat_target.join(profile);
    real_dir(&dest)?;
    std::fs::write(&record, &generation.id).map_err(|e| format!("record: {e}"))?;
    Ok(LaneOverlay {
        generation: generation.id.clone(),
        lower: generation.dir.clone(),
        upper,
        work,
        dest,
    })
}

/// Whether lane `lease_id` already runs on an overlay (its runner set one up
/// before), so an empty mountpoint is not mistaken for a warm target.
pub(crate) fn lane_has_overlay(main_target: &Path, lease_id: &str) -> bool {
    lane_overlay_dir(main_target, lease_id)
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
/// It refuses a destination that turned into a symlink since
/// [`prepare_lane`] checked it.
const MOUNT_THEN_EXEC: &str = "[ -d \"$2\" ] && [ ! -h \"$2\" ] || exit 125; \
     mount -t overlay overlay -o \"$1\" \"$2\" || exit 125; shift 2; exec \"$@\"";

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

/// The bwrap options that give a lane job back the host's own uid and gid,
/// with no capabilities.
///
/// `unshare -Urm` maps the launcher's user to root so the overlay can be
/// mounted, and the lane-job bwrap inherits that: run as uid 0 it makes no
/// user namespace of its own and keeps every capability. A job then sees
/// `id -u` = 0, so the host cargo wrapper looks for its slot locks under
/// `cargo-slots-0` rather than the `cargo-slots-<uid>` directory the sandbox
/// binds, finds it missing and queues on a private copy under the job's
/// tmpfs: every lane took a slot of its own while host `cargo ps` showed
/// them all as queued (BUG-2435). A nested user namespace mapped to the host
/// ids puts the job back on the host's slot directory, and `--cap-drop ALL`
/// keeps it from remounting the read-only binds it runs under.
// trace:BUG-2435 | ai:claude
pub(crate) fn host_ids_bwrap_args(uid: u32, gid: u32) -> Vec<String> {
    vec![
        "--unshare-user".to_string(),
        "--uid".into(),
        uid.to_string(),
        "--gid".into(),
        gid.to_string(),
        "--cap-drop".into(),
        "ALL".into(),
    ]
}

/// [`wrap`] for the lane-job bubblewrap `bwrap bwrap_args`: the job runs on
/// the overlay with the launcher's own uid and gid ([`host_ids_bwrap_args`]).
// trace:BUG-2435 | ai:claude
#[cfg(unix)]
pub(crate) fn wrap_lane_job(
    unshare: &str,
    ov: &LaneOverlay,
    bwrap: &str,
    bwrap_args: Vec<String>,
) -> Result<(String, Vec<String>), String> {
    // SAFETY: getuid and getgid have no preconditions and cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let mut args = host_ids_bwrap_args(uid, gid);
    args.extend(bwrap_args);
    wrap(unshare, ov, bwrap, args)
}

/// The bubblewrap argv [`probe`] runs on the overlay: the smallest sandbox
/// that still needs the nested user namespace every lane job takes.
// trace:TASK-2829 | ai:claude
pub(crate) const PROBE_BWRAP_ARGS: [&str; 4] = ["--ro-bind", "/", "/", "true"];

/// Mount the overlay once in a throwaway namespace and run `bwrap ... true`
/// in it exactly as a lane job's bwrap runs there ([`wrap_lane_job`]: a
/// nested user namespace back to the host uid and gid), so a host that
/// refuses unprivileged user namespaces, overlay mounts or the nested user
/// namespace falls back to cold before the first job instead of failing
/// every job. `Err` is the reason.
// trace:TASK-2829 | ai:claude
pub(crate) fn probe(unshare: &str, ov: &LaneOverlay) -> Result<(), String> {
    probe_with(unshare, "bwrap", ov)
}

/// [`probe`] with the bubblewrap program to use.
// trace:TASK-2829 | ai:claude
pub(crate) fn probe_with(unshare: &str, bwrap: &str, ov: &LaneOverlay) -> Result<(), String> {
    let bwrap_args = PROBE_BWRAP_ARGS.iter().map(|a| a.to_string()).collect();
    let (program, args) = wrap_lane_job(unshare, ov, bwrap, bwrap_args)?;
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
        "the overlay mount or the lane-job bwrap on it failed (exit {:?}): {}",
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
