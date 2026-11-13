//! BUG-2383 part 2: the generation snapshot plus per-lane overlay that warms a
//! seat target on a filesystem without reflinks.
// trace:BUG-2383 | ai:claude

use super::*;
use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;

/// A fixture primary: `<root>/target/agent` with a dependency artifact, a
/// fingerprint, a build-script output and an incremental dir.
fn primary(root: &Path) -> PathBuf {
    let target = root.join("target");
    let agent = target.join("agent");
    for d in [
        "deps",
        ".fingerprint/dep-1",
        "build/dep-1/out",
        "incremental/x",
    ] {
        std::fs::create_dir_all(agent.join(d)).unwrap();
    }
    std::fs::write(agent.join("deps/libdep-1.rlib"), "primary rlib\n").unwrap();
    std::fs::write(agent.join(".fingerprint/dep-1/lib-dep"), "fp\n").unwrap();
    std::fs::write(agent.join("build/dep-1/out/gen.rs"), "gen\n").unwrap();
    std::fs::write(agent.join("incremental/x/blob"), "inc\n").unwrap();
    std::fs::write(target.join(".rustc_info.json"), "{\"rustc\":\"1.90\"}").unwrap();
    target
}

/// Every regular file's (device, inode) under `root`.
fn inodes(root: &Path) -> HashSet<(u64, u64)> {
    let mut out = HashSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let m = std::fs::symlink_metadata(e.path()).unwrap();
            if m.is_dir() {
                stack.push(e.path());
            } else if m.is_file() {
                out.insert((m.dev(), m.ino()));
            }
        }
    }
    out
}

#[test]
fn a_generation_id_follows_the_fingerprints_and_the_toolchain() {
    let tmp = tempfile::tempdir().unwrap();
    let target = primary(tmp.path());
    let first = generation_id(&target, "agent").unwrap();
    assert!(first.starts_with("agent-"));
    assert_eq!(generation_id(&target, "agent").unwrap(), first);
    std::fs::write(target.join(".rustc_info.json"), "{\"rustc\":\"1.91\"}").unwrap();
    let other_toolchain = generation_id(&target, "agent").unwrap();
    assert_ne!(other_toolchain, first);
    // Review r7, F1: the id follows the content, not mtime seconds. A
    // fingerprint rewritten with new content, and a same-size artifact
    // rewritten within the same second, each give a new id.
    std::fs::write(target.join("agent/.fingerprint/dep-1/lib-dep"), "f2\n").unwrap();
    let new_fingerprint = generation_id(&target, "agent").unwrap();
    assert_ne!(new_fingerprint, other_toolchain);
    let rlib = target.join("agent/deps/libdep-1.rlib");
    let at = std::fs::metadata(&rlib).unwrap().modified().unwrap();
    std::fs::write(&rlib, "primary rlab\n").unwrap();
    filetime_set(&rlib, at + Duration::from_nanos(1));
    assert_ne!(generation_id(&target, "agent").unwrap(), new_fingerprint);
    std::fs::remove_dir_all(target.join("agent/.fingerprint")).unwrap();
    assert!(generation_id(&target, "agent").is_none());
}

// The 1.100 layout must warm without a legacy .fingerprint directory, and
// primary rewrites must neither mutate an existing lower nor reuse its id.
// trace:BUG-2539 | ai:codex
#[test]
fn bug_2539_unit_layout_snapshot_preserves_fingerprints_and_outputs() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("target");
    let unit = target.join("agent/build/fixture-dep/0123456789abcdef");
    std::fs::create_dir_all(unit.join("out")).unwrap();
    assert!(generation_id(&target, "agent").is_none());
    std::fs::create_dir_all(unit.join("fingerprint")).unwrap();
    std::fs::write(
        unit.join("fingerprint/lib-fixture_dep"),
        "original fingerprint",
    )
    .unwrap();
    std::fs::write(unit.join("out/libfixture_dep.rlib"), "original artifact").unwrap();
    let id = generation_id(&target, "agent").unwrap();
    let first = snapshot(&target, "agent", "lease-one", Duration::from_secs(5)).unwrap();
    assert_eq!(first.id, id);
    let relative = "build/fixture-dep/0123456789abcdef";
    let lower = first.dir.join(relative);
    for file in ["fingerprint/lib-fixture_dep", "out/libfixture_dep.rlib"] {
        let source = unit.join(file);
        let copy = lower.join(file);
        assert_eq!(
            std::fs::read(&copy).unwrap(),
            std::fs::read(&source).unwrap()
        );
        assert_ne!(
            std::fs::metadata(&copy).unwrap().ino(),
            std::fs::metadata(&source).unwrap().ino()
        );
    }
    assert_eq!(
        snapshot(&target, "agent", "lease-two", Duration::from_secs(5)).unwrap(),
        first
    );
    // Hold mtimes and lengths constant: the fingerprint's content still changes the id.
    let fingerprint = unit.join("fingerprint/lib-fixture_dep");
    let timestamp = std::fs::metadata(&fingerprint).unwrap().modified().unwrap();
    std::fs::write(&fingerprint, "modified fingerprint").unwrap();
    filetime_set(&fingerprint, timestamp);
    assert_ne!(generation_id(&target, "agent").unwrap(), id);
    assert_eq!(
        std::fs::read_to_string(lower.join("fingerprint/lib-fixture_dep")).unwrap(),
        "original fingerprint"
    );
    let second = snapshot(&target, "agent", "lease-three", Duration::from_secs(5)).unwrap();
    let before_artifact = generation_id(&target, "agent").unwrap();
    std::fs::write(
        unit.join("out/libfixture_dep.rlib"),
        "new artifact with a different length",
    )
    .unwrap();
    assert_ne!(generation_id(&target, "agent").unwrap(), before_artifact);
    assert_eq!(
        std::fs::read_to_string(lower.join("out/libfixture_dep.rlib")).unwrap(),
        "original artifact"
    );
    // A damaged copied output refuses reuse, retaining the old generation for its readers.
    std::fs::remove_file(second.dir.join(relative).join("out/libfixture_dep.rlib")).unwrap();
    assert!(!complete(&second.dir));
    let third = snapshot(&target, "agent", "lease-four", Duration::from_secs(5)).unwrap();
    std::fs::remove_file(third.dir.join(".build-manifest.json")).unwrap();
    assert!(!complete(&third.dir));
    let repaired = snapshot(&target, "agent", "lease-five", Duration::from_secs(5)).unwrap();
    assert_ne!(repaired.dir, third.dir);
    assert!(
        third.dir.is_dir(),
        "a live damaged generation must be retained"
    );
    assert!(complete(&repaired.dir));
}

fn filetime_set(path: &Path, at: std::time::SystemTime) {
    let f = std::fs::File::open(path).unwrap();
    f.set_modified(at).unwrap();
}

// The snapshot hardlinks deps/ and build/, really copies .fingerprint/,
// skips incremental/, and is reused for the same generation.
#[test]
fn a_snapshot_links_artifacts_copies_fingerprints_and_is_reused() {
    let tmp = tempfile::tempdir().unwrap();
    let target = primary(tmp.path());
    let generation = snapshot(&target, "agent", "lease-a", Duration::from_secs(5)).unwrap();
    assert!(generation.dir.starts_with(target.join(LANE_BASE)));
    let ino = |p: &Path| std::fs::metadata(p).unwrap().ino();
    let agent = target.join("agent");
    assert_eq!(
        ino(&generation.dir.join("deps/libdep-1.rlib")),
        ino(&agent.join("deps/libdep-1.rlib"))
    );
    assert_eq!(
        ino(&generation.dir.join("build/dep-1/out/gen.rs")),
        ino(&agent.join("build/dep-1/out/gen.rs"))
    );
    assert_ne!(
        ino(&generation.dir.join(".fingerprint/dep-1/lib-dep")),
        ino(&agent.join(".fingerprint/dep-1/lib-dep"))
    );
    assert!(!generation.dir.join("incremental").exists());
    assert!(!stale(&generation.dir));
    let again = snapshot(&target, "agent", "lease-a", Duration::from_secs(5)).unwrap();
    assert_eq!(again, generation);
    // An in-place rewrite of a hardlinked primary artifact is reported stale.
    std::fs::write(
        agent.join("deps/libdep-1.rlib"),
        "rewritten in place, longer\n",
    )
    .unwrap();
    assert!(stale(&generation.dir));
}

// The snapshot takes the primary's build lock, so it never reads a build
// that is still being written.
#[test]
fn a_snapshot_waits_for_the_primary_build_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let target = primary(tmp.path());
    let held = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(target.join("agent/.cargo-lock"))
        .unwrap();
    fs2::FileExt::lock_exclusive(&held).unwrap();
    let why = snapshot(&target, "agent", "lease-a", Duration::from_millis(300)).unwrap_err();
    assert!(why.contains("primary's build is still running"), "{why}");
    assert!(std::fs::read_dir(target.join(LANE_BASE))
        .unwrap()
        .flatten()
        .all(|e| e.file_name().to_string_lossy().starts_with('.')));
    drop(held);
    assert!(snapshot(&target, "agent", "lease-a", Duration::from_secs(5)).is_ok());
}

// Amendment 1 (GC): a generation is removed once no pin names a lease that
// still exists; a live pin or the kept generation survives.
#[test]
fn generations_no_live_lease_pins_are_collected() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path();
    let target = main.join("target");
    let make = |id: &str, leases: &[&str]| {
        let generation = Generation {
            id: id.into(),
            dir: target.join(LANE_BASE).join(id),
        };
        std::fs::create_dir_all(generation.dir.join(PINS)).unwrap();
        for lease in leases {
            std::fs::write(generation.dir.join(PINS).join(lease), "").unwrap();
        }
        generation
    };
    let sessions = main.join(".aida/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("live.toml"), "id = \"live\"\n").unwrap();
    let pinned = make("agent-1-a", &["live", "gone"]);
    let dead = make("agent-2-a", &["gone"]);
    let unpinned = make("agent-3-a", &[]);
    let kept = make("agent-4-a", &[]);
    std::fs::create_dir_all(target.join(LANE_BASE).join(".agent-5.tmp-1")).unwrap();
    // A lane overlay is collected with its lease too.
    let live_overlay = lane_overlay_dir(&target, "live");
    let dead_overlay = lane_overlay_dir(&target, "gone");
    for d in [&live_overlay, &dead_overlay] {
        std::fs::create_dir_all(d.join("upper")).unwrap();
    }
    let mut removed = gc(&target, main, Some(&kept.dir), Duration::from_secs(5));
    removed.sort();
    assert_eq!(
        removed,
        vec![dead.dir.clone(), unpinned.dir.clone(), dead_overlay.clone()]
    );
    assert!(live_overlay.is_dir());
    assert!(pinned.dir.is_dir() && kept.dir.is_dir());
    assert!(pinned.dir.join(".pins/live").exists());
    assert!(
        !pinned.dir.join(".pins/gone").exists(),
        "dead pins are pruned"
    );
    std::fs::remove_file(sessions.join("live.toml")).unwrap();
    let mut removed = gc(&target, main, None, Duration::from_secs(5));
    removed.sort();
    assert_eq!(
        removed,
        vec![pinned.dir.clone(), kept.dir.clone(), live_overlay.clone()]
    );
    assert!(target.join(LANE_BASE).join(".agent-5.tmp-1").is_dir());
}

// The lane's upper survives between jobs on one generation and is
// discarded when the lane moves to another. It lives under the primary's
// target, never under the job-writable seat target.
#[test]
fn a_lane_upper_is_kept_per_generation_and_reset_on_a_new_one() {
    let tmp = tempfile::tempdir().unwrap();
    let main_target = tmp.path().join("target");
    let seat_target = main_target.join("worktrees/seat");
    let generation = |id: &str| Generation {
        id: id.into(),
        dir: tmp.path().join(id),
    };
    let first = prepare_lane(
        &main_target,
        &seat_target,
        "agent",
        "lease-a",
        &generation("agent-1-a"),
    )
    .unwrap();
    assert_eq!(first.dest, seat_target.join("agent"));
    let home = lane_overlay_dir(&main_target, "lease-a");
    assert_eq!(first.upper, home.join("upper"));
    assert_eq!(first.work, home.join("work"));
    assert!(!first.upper.starts_with(&seat_target));
    assert!(lane_has_overlay(&main_target, "lease-a"));
    assert!(!lane_has_overlay(&main_target, "lease-b"));
    std::fs::write(first.upper.join("built"), "x").unwrap();
    let same = prepare_lane(
        &main_target,
        &seat_target,
        "agent",
        "lease-a",
        &generation("agent-1-a"),
    )
    .unwrap();
    assert!(same.upper.join("built").exists());
    let next = prepare_lane(
        &main_target,
        &seat_target,
        "agent",
        "lease-a",
        &generation("agent-2-a"),
    )
    .unwrap();
    assert_eq!(next.lower, tmp.path().join("agent-2-a"));
    assert!(!next.upper.join("built").exists());
    assert!(prepare_lane(
        &main_target,
        &seat_target,
        "agent",
        "../x",
        &generation("agent-2-a")
    )
    .is_err());
}

// Strict review P1 (b): a mount destination a job turned into a symlink is
// refused, by prepare_lane and again by the mount script.
// trace:BUG-2383 | ai:claude
#[test]
fn a_symlinked_mount_destination_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let main_target = tmp.path().join("target");
    let seat_target = main_target.join("worktrees/seat");
    let victim = tmp.path().join("victim");
    std::fs::create_dir_all(&victim).unwrap();
    std::fs::create_dir_all(&seat_target).unwrap();
    std::os::unix::fs::symlink(&victim, seat_target.join("agent")).unwrap();
    let generation = Generation {
        id: "agent-1-a".into(),
        dir: tmp.path().join("agent-1-a"),
    };
    let why =
        prepare_lane(&main_target, &seat_target, "agent", "lease-a", &generation).unwrap_err();
    assert!(why.contains("not a directory"), "{why}");
    std::fs::remove_file(seat_target.join("agent")).unwrap();
    std::fs::write(seat_target.join("agent"), "file").unwrap();
    assert!(prepare_lane(&main_target, &seat_target, "agent", "lease-a", &generation).is_err());
    assert!(MOUNT_THEN_EXEC.starts_with("[ -d \"$2\" ] && [ ! -h \"$2\" ] || exit 125;"));
}

// Strict review P2: snapshot pins under the lane-base lock and gc takes the
// same lock, so a gc racing a new lane never removes its generation between
// the snapshot and the pin; an incomplete generation (a gc that removed it
// half-way, say) is rebuilt, never reused. trace:BUG-2383 | ai:claude
#[test]
fn a_racing_gc_never_removes_a_generation_between_snapshot_and_pin() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path();
    let target = primary(main);
    let sessions = main.join(".aida/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("lease-b.toml"), "id = \"lease-b\"\n").unwrap();
    let generation = snapshot(&target, "agent", "lease-b", Duration::from_secs(5)).unwrap();
    // The pin is already there when snapshot returns: the window the old
    // separate pin() left is gone, so the gc that ran in it keeps the
    // generation.
    assert!(generation.dir.join(".pins/lease-b").is_file());
    assert!(gc(&target, main, None, Duration::from_secs(5)).is_empty());
    assert!(complete(&generation.dir));
    // A gc that cannot take the lane-base lock (a snapshot holds it)
    // collects nothing, even an unpinned generation.
    std::fs::remove_file(sessions.join("lease-b.toml")).unwrap();
    let held = open_lock(&target.join(LANE_BASE).join(".lock")).unwrap();
    fs2::FileExt::lock_exclusive(&held).unwrap();
    assert!(gc(&target, main, None, Duration::from_millis(300)).is_empty());
    assert!(generation.dir.is_dir());
    // ...and a snapshot waits for that lock too.
    assert!(
        snapshot(&target, "agent", "lease-b", Duration::from_millis(300))
            .unwrap_err()
            .contains("another generation snapshot")
    );
    drop(held);
    // A generation a gc removed half-way is rebuilt, not reused.
    std::fs::remove_dir_all(generation.dir.join("deps")).unwrap();
    assert!(!complete(&generation.dir));
    let rebuilt = snapshot(&target, "agent", "lease-c", Duration::from_secs(5)).unwrap();
    // Review r7, F1: under the next slot, the incomplete one left alone.
    assert_eq!(rebuilt.id, format!("{}-r1", generation.id));
    assert!(generation.dir.join(".pins/lease-b").is_file());
    assert!(complete(&rebuilt.dir));
    assert!(rebuilt.dir.join("deps/libdep-1.rlib").is_file());
    assert!(rebuilt.dir.join(".pins/lease-c").is_file());
    // Every required directory is there even when the primary has no build/.
    std::fs::remove_dir_all(target.join("agent/build")).unwrap();
    std::fs::remove_dir_all(&rebuilt.dir).unwrap();
    let no_build = snapshot(&target, "agent", "lease-c", Duration::from_secs(5)).unwrap();
    assert!(complete(&no_build.dir));
}

// Strict review P3: a generation whose deps/ no longer matches its stored
// manifest (one rlib deleted) is incomplete, so snapshot rebuilds it rather
// than reusing it. trace:BUG-2383 | ai:claude
#[test]
fn a_generation_missing_an_rlib_is_rebuilt_not_reused() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path();
    let target = primary(main);
    let sessions = main.join(".aida/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("lease-d.toml"), "id = \"lease-d\"\n").unwrap();
    let generation = snapshot(&target, "agent", "lease-d", Duration::from_secs(5)).unwrap();
    assert!(complete(&generation.dir));
    std::fs::remove_file(generation.dir.join("deps/libdep-1.rlib")).unwrap();
    assert!(!complete(&generation.dir));
    let rebuilt = snapshot(&target, "agent", "lease-d", Duration::from_secs(5)).unwrap();
    assert_eq!(rebuilt.id, format!("{}-r1", generation.id));
    assert!(
        generation.dir.is_dir(),
        "an incomplete generation is never removed by a snapshot"
    );
    assert!(complete(&rebuilt.dir));
    assert!(rebuilt.dir.join("deps/libdep-1.rlib").is_file());
    // A manifest that does not parse is not reused either.
    std::fs::write(rebuilt.dir.join(MANIFEST), "not json").unwrap();
    assert!(!complete(&rebuilt.dir));
}

// Review r7, F1 (data loss): a primary rebuild that rewrites an artifact
// in place and a fingerprint never replaces a generation a live lane pins.
// The next lane gets a fresh generation id, the first lane's pin survives,
// and a collection with only the first lane live keeps its generation.
// trace:BUG-2383 | ai:claude
#[test]
fn a_rebuild_never_replaces_a_generation_a_live_lane_pins() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path();
    let target = primary(main);
    let sessions = main.join(".aida/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("lease-a.toml"), "id = \"lease-a\"\n").unwrap();
    let wait = Duration::from_secs(5);
    let a = snapshot(&target, "agent", "lease-a", wait).unwrap();
    let a_inodes = inodes(&a.dir);
    // The primary rebuilds: the hardlinked artifact is rewritten in place
    // (so generation a's copy changes too) and the fingerprint is rewritten,
    // all within the same second.
    let agent = target.join("agent");
    std::fs::write(agent.join("deps/libdep-1.rlib"), "rebuilt rlib\n").unwrap();
    std::fs::write(agent.join(".fingerprint/dep-1/lib-dep"), "fq\n").unwrap();
    std::fs::write(sessions.join("lease-b.toml"), "id = \"lease-b\"\n").unwrap();
    let b = snapshot(&target, "agent", "lease-b", wait).unwrap();
    assert_ne!(b.id, a.id, "changed content got a fresh generation");
    assert_ne!(b.dir, a.dir);
    assert!(
        a.dir.join(".pins/lease-a").is_file(),
        "lease-a's pin survives"
    );
    assert!(b.dir.join(".pins/lease-b").is_file());
    assert_eq!(inodes(&a.dir), a_inodes, "generation a was not replaced");
    assert_eq!(
        std::fs::read_to_string(b.dir.join(".fingerprint/dep-1/lib-dep")).unwrap(),
        "fq\n"
    );
    // Only lease-a is live: its generation stays, b goes.
    std::fs::remove_file(sessions.join("lease-b.toml")).unwrap();
    let removed = gc(&target, main, None, wait);
    assert_eq!(removed, vec![b.dir.clone()]);
    assert!(a.dir.is_dir() && a.dir.join(".pins/lease-a").is_file());
    // Same-id collision: even when a generation under the current id exists
    // but is incomplete, a snapshot takes a new slot and leaves it alone.
    std::fs::write(sessions.join("lease-c.toml"), "id = \"lease-c\"\n").unwrap();
    let c = snapshot(&target, "agent", "lease-c", wait).unwrap();
    std::fs::remove_file(c.dir.join("deps/libdep-1.rlib")).unwrap();
    let c_again = snapshot(&target, "agent", "lease-a", wait).unwrap();
    assert_eq!(c_again.id, format!("{}-r1", c.id));
    assert!(c.dir.join(".pins/lease-c").is_file());
    // lease-a moved to the new generation: its old pin is dropped, so a
    // collection may now remove generation a, never c (lease-c is live).
    assert!(!a.dir.join(".pins/lease-a").exists());
    assert!(c_again.dir.join(".pins/lease-a").is_file());
    let removed = gc(&target, main, Some(&c_again.dir), wait);
    assert_eq!(removed, vec![a.dir.clone()]);
    assert!(c.dir.is_dir());
}

#[test]
fn the_wrapper_mounts_then_execs_the_unchanged_bwrap_argv() {
    let ov = LaneOverlay {
        generation: "agent-1-a".into(),
        lower: "/m/target/lane-base/agent-1-a".into(),
        upper: "/m/target/lane-overlay/l/upper".into(),
        work: "/m/target/lane-overlay/l/work".into(),
        dest: "/m/target/worktrees/s/agent".into(),
    };
    let bwrap_args = vec![
        "--ro-bind".to_string(),
        "/".into(),
        "/".into(),
        "cargo".into(),
    ];
    let (program, args) = wrap("unshare", &ov, "bwrap", bwrap_args.clone()).unwrap();
    assert_eq!(program, "unshare");
    assert_eq!(&args[..2], ["-Urm", "--"]);
    let at = args.iter().position(|a| a == "bwrap").unwrap();
    assert_eq!(args[at + 1..], bwrap_args[..]);
    assert!(args.contains(&"/m/target/worktrees/s/agent".to_string()));
    let bad = LaneOverlay {
        lower: "/m/a,b".into(),
        ..ov
    };
    assert!(wrap("unshare", &bad, "bwrap", Vec::new()).is_err());
}

// BUG-2435: the lane-job bwrap on an overlay re-enters the host uid and gid
// with no capabilities, ahead of the profile's own options.
// trace:BUG-2435 | ai:claude
#[test]
fn bug_2435_the_overlay_job_bwrap_runs_as_the_host_ids_without_caps() {
    assert_eq!(
        host_ids_bwrap_args(1000, 1001),
        [
            "--unshare-user",
            "--uid",
            "1000",
            "--gid",
            "1001",
            "--cap-drop",
            "ALL"
        ]
    );
    let ov = LaneOverlay {
        generation: "agent-1-a".into(),
        lower: "/m/target/lane-base/agent-1-a".into(),
        upper: "/m/target/lane-overlay/l/upper".into(),
        work: "/m/target/lane-overlay/l/work".into(),
        dest: "/m/target/worktrees/s/agent".into(),
    };
    let profile = vec![
        "--ro-bind".to_string(),
        "/".into(),
        "/".into(),
        "cargo".into(),
    ];
    let (program, args) = wrap_lane_job("unshare", &ov, "bwrap", profile.clone()).unwrap();
    assert_eq!(program, "unshare");
    // SAFETY: getuid and getgid have no preconditions and cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let at = args.iter().position(|a| a == "bwrap").unwrap();
    let mut expected = host_ids_bwrap_args(uid, gid);
    expected.extend(profile);
    assert_eq!(args[at + 1..], expected[..]);
}

// BUG-2435: a job on the lane overlay runs as the host uid, so the cargo
// wrapper's `$XDG_RUNTIME_DIR/cargo-slots-$(id -u)` is the host slot
// directory the sandbox binds read-write: what the job writes there (the
// wrapper's acquisition marker) is on the host, where host `cargo ps` reads
// it, and not on a private copy inside the sandbox. Runs the production
// snapshot, lane overlay, wrapper and lane-job bwrap profile.
// trace:BUG-2435 | ai:claude
#[test]
fn bug_2435_bwrap_overlay_job_takes_its_slot_in_the_host_slot_dir() {
    let base = live_base();
    let target = primary(base.path());
    let lane = lane_under_primary(base.path());
    let seat_target = base.path().join("target/worktrees/seat");
    let generation = snapshot(&target, "agent", "lease-ov", Duration::from_secs(5)).unwrap();
    let ov = prepare_lane(&target, &seat_target, "agent", "lease-ov", &generation).unwrap();
    if !overlay_usable(&ov) || !bwrap_runs() {
        return skip_live(&format!(
            "userns overlay or bwrap unavailable: {:?}",
            probe("unshare", &ov)
        ));
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    let runtime = base.path().join("run");
    let slots = runtime.join(format!("cargo-slots-{uid}"));
    std::fs::create_dir_all(&slots).unwrap();
    let env: BTreeMap<String, String> =
        [("XDG_RUNTIME_DIR".to_string(), runtime.display().to_string())].into();
    let rw = lane.sandbox_rw_paths(&env);
    assert!(rw.contains(&slots), "{rw:?}");
    let script = "set -e; id -u; d=\"$XDG_RUNTIME_DIR/cargo-slots-$(id -u)\"; \
                  [ -w \"$d\" ] || exit 4; echo \"$$\" > \"$d/acq.job\"";
    let argv = crate::session::lane_job_bwrap_argv(
        &lane.seat,
        &rw,
        &[],
        "/bin/sh",
        vec!["-c".into(), script.into()],
    );
    let (program, args) = wrap_lane_job("unshare", &ov, "bwrap", argv).unwrap();
    let out = std::process::Command::new(program)
        .args(&args)
        .env("XDG_RUNTIME_DIR", &runtime)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).lines().next(),
        Some(uid.to_string().as_str()),
        "the job did not run as the host uid"
    );
    assert!(
        slots.join("acq.job").is_file(),
        "the job's slot marker is not in the host slot directory"
    );
}

// TASK-2829: the overlay probe runs the same nested bwrap a lane job runs
// (a user namespace back to the host ids, no capabilities), so a host that
// mounts the overlay but refuses the nested user namespace falls back to cold
// instead of failing every job. A stand-in `unshare` records the argv.
// trace:TASK-2829 trace:TASK-3111 | ai:codex
#[test]
fn task_2829_the_probe_runs_the_lane_job_bwrap_in_a_nested_userns() {
    let tmp = tempfile::tempdir().unwrap();
    let ov = LaneOverlay {
        generation: "agent-1-a".into(),
        lower: tmp.path().join("lower"),
        upper: tmp.path().join("upper"),
        work: tmp.path().join("work"),
        dest: tmp.path().join("dest"),
    };
    let record = tmp.path().join("argv");
    let fake = tmp.path().join("unshare");
    crate::test_exec::write_executable(
        &fake,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            record.display()
        ),
    );
    let fake = fake.display().to_string();
    probe_with(&fake, "/opt/bwrap", &ov).unwrap();
    let argv: Vec<String> = std::fs::read_to_string(&record)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    let at = argv.iter().position(|a| a == "/opt/bwrap").unwrap();
    // SAFETY: getuid and getgid have no preconditions and cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let mut expected = host_ids_bwrap_args(uid, gid);
    expected.extend(PROBE_BWRAP_ARGS.iter().map(|a| a.to_string()));
    assert_eq!(argv[at + 1..], expected[..], "{argv:?}");
    // The argv is the lane job's own wrapping, only the bwrap profile differs.
    let job_args: Vec<String> = PROBE_BWRAP_ARGS.iter().map(|a| a.to_string()).collect();
    let (_, job) = wrap_lane_job(&fake, &ov, "/opt/bwrap", job_args).unwrap();
    assert_eq!(argv, job);
}

// TASK-2829: with the real `unshare` the probe fails when the nested bwrap
// stage fails, even though the overlay mounted, and passes with the real
// bubblewrap. Joins the forced-live CI gate.
// trace:TASK-2829 | ai:claude
#[test]
fn bug_2435_bwrap_probe_fails_when_the_nested_bwrap_fails() {
    let base = live_base();
    let target = primary(base.path());
    let seat_target = base.path().join("target/worktrees/seat");
    let generation = snapshot(&target, "agent", "lease-ov", Duration::from_secs(5)).unwrap();
    let ov = prepare_lane(&target, &seat_target, "agent", "lease-ov", &generation).unwrap();
    if !overlay_usable(&ov) || !bwrap_runs() {
        return skip_live(&format!(
            "userns overlay or bwrap unavailable: {:?}",
            probe("unshare", &ov)
        ));
    }
    assert!(probe_with("unshare", "bwrap", &ov).is_ok());
    let refused = probe_with("unshare", "false", &ov);
    assert!(refused.is_err(), "{refused:?}");
}

// TASK-2830: `--cap-drop ALL` on the overlay job's bwrap keeps the job from
// remounting the read-only binds it runs under: `/` and a fresh bind of a
// directory outside the job's writable paths both stay read-only, and the
// host file under them is unchanged. Runs the production snapshot, lane
// overlay, wrapper and lane-job bwrap profile. Joins the forced-live CI gate.
// trace:TASK-2830 | ai:claude
#[test]
fn bug_2435_bwrap_overlay_job_cannot_remount_a_ro_bind_rw() {
    let base = live_base();
    let target = primary(base.path());
    let lane = lane_under_primary(base.path());
    let seat_target = base.path().join("target/worktrees/seat");
    let generation = snapshot(&target, "agent", "lease-ov", Duration::from_secs(5)).unwrap();
    let ov = prepare_lane(&target, &seat_target, "agent", "lease-ov", &generation).unwrap();
    if !overlay_usable(&ov) || !bwrap_runs() {
        return skip_live(&format!(
            "userns overlay or bwrap unavailable: {:?}",
            probe("unshare", &ov)
        ));
    }
    let ro = base.path().join("ro");
    std::fs::create_dir_all(&ro).unwrap();
    let victim = ro.join("victim");
    std::fs::write(&victim, "host\n").unwrap();
    let env: BTreeMap<String, String> = BTreeMap::new();
    let rw = lane.sandbox_rw_paths(&env);
    assert!(!rw.iter().any(|p| ro.starts_with(p)), "{rw:?}");
    let ro_s = ro.display().to_string();
    let script = format!(
        "command -v mount >/dev/null || exit 3; \
         echo x > '{ro_s}/victim' 2>/dev/null && echo WROTE-BEFORE; \
         mount -o remount,rw / 2>&1 && echo REMOUNTED-ROOT; \
         mount -o remount,bind,rw / 2>&1 && echo REMOUNTED-ROOT-BIND; \
         mount --bind '{ro_s}' '{ro_s}' 2>&1 && echo BOUND; \
         mount -o remount,bind,rw '{ro_s}' 2>&1 && echo REMOUNTED-RO; \
         echo x > '{ro_s}/victim' 2>/dev/null && echo WROTE-AFTER; \
         grep -q ' / / ro[, ]' /proc/self/mountinfo && echo ROOT-STILL-RO; \
         echo READY"
    );
    let argv = crate::session::lane_job_bwrap_argv(
        &lane.seat,
        &rw,
        &[],
        "/bin/sh",
        vec!["-c".into(), script],
    );
    let (program, args) = wrap_lane_job("unshare", &ov, "bwrap", argv).unwrap();
    let out = std::process::Command::new(program)
        .args(&args)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{out:?}");
    assert!(stdout.contains("READY"), "the job did not run: {stdout}");
    for refused in [
        "WROTE-BEFORE",
        "REMOUNTED-ROOT",
        "BOUND",
        "REMOUNTED-RO",
        "WROTE-AFTER",
    ] {
        assert!(!stdout.contains(refused), "{refused}: {stdout}");
    }
    assert!(stdout.contains("ROOT-STILL-RO"), "{stdout}");
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "host\n");

    // mount(8) checks uid before trying the syscall. Use the syscall directly
    // for the capability control, retaining the same non-root uid and wrapper.
    // The inherited read-only binds can also be locked by the user namespace:
    // do not claim removing cap-drop alone makes those binds writable. A fresh
    // job-owned read-only tmpfs demonstrates mount/remount authority separately.
    // trace:TASK-3111 trace:TASK-2847 | ai:codex
    // Host policy may forbid even a capable process from creating a mount
    // namespace or mounting tmpfs. Only that positive control is optional;
    // all production-profile denial assertions above remain required-live.
    let control = r#"import ctypes, errno, os, sys
libc = ctypes.CDLL(None, use_errno=True)
admin = int(open('/proc/self/status').read().split('CapEff:')[1].split()[0], 16) & (1 << 21)
assert bool(admin) == (sys.argv[1] == 'true'), 'unexpected CAP_SYS_ADMIN state'
def setup_unavailable(operation, rc):
    error = ctypes.get_errno()
    if admin and rc == -1 and error in (errno.EPERM, errno.EACCES, errno.ENOSYS):
        print(f'{operation} refused by host policy: errno={error}')
        sys.exit(77)
# Own the mount namespace: bwrap's /dev setup may leave its namespace
# owned by a parent user namespace, independently of effective capabilities.
rc = libc.unshare(0x20000) # CLONE_NEWNS
if admin:
    setup_unavailable('unshare(CLONE_NEWNS)', rc)
    assert rc == 0, ctypes.get_errno()
else:
    assert rc == -1 and ctypes.get_errno() == errno.EPERM
os.mkdir('/tmp/cap-control')
rc = libc.mount(b'tmpfs', b'/tmp/cap-control', b'tmpfs', 1, None)
if admin:
    setup_unavailable('mount(tmpfs)', rc)
    assert rc == 0, ctypes.get_errno()
    assert libc.mount(None, b'/tmp/cap-control', None, 32, None) == 0, ctypes.get_errno()
    open('/tmp/cap-control/proof', 'w').write('private tmpfs')
    print('REMOUNT-CONTROL')
else:
    assert rc == -1 and ctypes.get_errno() == errno.EPERM
    print('CAP-DROP-REFUSED')
"#;
    for retain_admin in [false, true] {
        let argv = crate::session::lane_job_bwrap_argv(
            &lane.seat,
            &rw,
            &[],
            "/bin/sh",
            vec![
                "-c".into(),
                "command -v python3 >/dev/null || { echo 'python3 unavailable'; exit 77; }; exec python3 -c \"$1\" \"$2\"".into(),
                "cap-control".into(),
                control.into(),
                retain_admin.to_string(),
            ],
        );
        let (program, mut args) = wrap_lane_job("unshare", &ov, "bwrap", argv).unwrap();
        let drop = args.iter().position(|arg| arg == "--cap-drop").unwrap();
        if retain_admin {
            args.splice(
                drop + 2..drop + 2,
                ["--cap-add".into(), "CAP_SYS_ADMIN".into()],
            );
        }
        let out = std::process::Command::new(program)
            .args(&args)
            .output()
            .unwrap();
        if out.status.code() == Some(77) {
            eprintln!(
                "SKIP (capability control unavailable), retain_admin={retain_admin}: {}",
                String::from_utf8_lossy(&out.stdout).trim()
            );
            continue;
        }
        assert!(out.status.success(), "retain_admin={retain_admin}: {out:?}");
        let expected = if retain_admin {
            "REMOUNT-CONTROL"
        } else {
            "CAP-DROP-REFUSED"
        };
        assert!(
            String::from_utf8_lossy(&out.stdout).contains(expected),
            "{out:?}"
        );
    }
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "host\n");
}

/// A lane whose seat target is `<root>/target/worktrees/seat`, under the
/// fixture primary.
fn lane_under_primary(root: &Path) -> crate::lane_job::LaneJobs {
    let seat = root.join("seat");
    std::fs::create_dir_all(&seat).unwrap();
    crate::lane_job::LaneJobs {
        inbox: seat.join(crate::lane_job::INBOX_RELATIVE),
        results: crate::lane_job::results_dir(root, "lease-ov"),
        seat,
        lease_id: "lease-ov".into(),
        build_env: vec![(
            "CARGO_TARGET_DIR".into(),
            root.join("target/worktrees/seat").display().to_string(),
        )],
        host_env: Vec::new(),
        program: "cargo".into(),
        sandbox: crate::lane_job::Sandbox::Bwrap,
    }
}

// Amendment 2: when the overlay cannot be set up the lane builds cold and
// only says `seed: cold (<reason>)`; nothing fails.
#[test]
fn an_unavailable_overlay_falls_back_to_a_reported_cold_seed() {
    let tmp = tempfile::tempdir().unwrap();
    primary(tmp.path());
    let lane = lane_under_primary(tmp.path());
    // A lane already on an overlay skips the reflink seed (which a
    // reflink-capable temp filesystem would otherwise take).
    let earlier = Generation {
        id: "agent-0-0".into(),
        dir: tmp.path().join("target").join(LANE_BASE).join("agent-0-0"),
    };
    prepare_lane(
        &tmp.path().join("target"),
        &tmp.path().join("target/worktrees/seat"),
        "agent",
        "lease-ov",
        &earlier,
    )
    .unwrap();
    let note = crate::lane_job::seed_lane_target_with(&lane, "/nonexistent/unshare");
    assert!(note.cold);
    assert!(note.overlay.is_none());
    assert!(note.note.contains("seed: cold ("), "{}", note.note);
    assert!(note.note.contains("overlay:"), "{}", note.note);
    // No primary fingerprints: cold too, with the reason.
    let bare = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(bare.path().join("target/agent")).unwrap();
    prepare_lane(
        &bare.path().join("target"),
        &bare.path().join("target/worktrees/seat"),
        "agent",
        "lease-ov",
        &earlier,
    )
    .unwrap();
    let note = crate::lane_job::seed_lane_target_with(&lane_under_primary(bare.path()), "unshare");
    assert!(
        note.cold && note.note.contains("seed: cold ("),
        "{}",
        note.note
    );
}

/// Whether `unshare -Urm` can mount an overlay here and run the lane job's
/// nested bwrap on it.
fn overlay_usable(ov: &LaneOverlay) -> bool {
    probe("unshare", ov).is_ok()
}

// Amendment 1 (inode): a seat write through the overlay, in place or not,
// never changes a file of the primary's target. Runs the production
// snapshot, lane overlay and wrapper around the real lane-job bwrap profile.
// Joins the forced-live CI gate. trace:BUG-2383 | ai:claude
#[test]
fn bug_2383_bwrap_overlay_seat_write_never_changes_a_primary_inode() {
    // Not under /tmp: the lane-job profile mounts a private tmpfs there.
    let base_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/bug-2383-bwrap");
    std::fs::create_dir_all(&base_dir).unwrap();
    // A worktree's target/ may be a symlink, which bwrap cannot bind through.
    let base_dir = std::fs::canonicalize(base_dir).unwrap();
    let base = tempfile::tempdir_in(base_dir).unwrap();
    let target = primary(base.path());
    let lane = lane_under_primary(base.path());
    let seat_target = base.path().join("target/worktrees/seat");
    let generation = snapshot(&target, "agent", "lease-ov", Duration::from_secs(5)).unwrap();
    let ov = prepare_lane(&target, &seat_target, "agent", "lease-ov", &generation).unwrap();
    if !overlay_usable(&ov) || !bwrap_runs() {
        assert!(
            std::env::var_os("AIDA_REQUIRE_BWRAP_LIVE").is_none(),
            "AIDA_REQUIRE_BWRAP_LIVE requires the overlay inode test to run: {:?}",
            probe("unshare", &ov)
        );
        eprintln!("SKIP (bwrap unavailable)");
        return;
    }
    let agent = target.join("agent");
    let primary_files = inodes(&agent);
    let before = |p: &str| {
        let m = std::fs::metadata(agent.join(p)).unwrap();
        (std::fs::read(agent.join(p)).unwrap(), m.ino(), m.mtime())
    };
    let watched = [
        "deps/libdep-1.rlib",
        ".fingerprint/dep-1/lib-dep",
        "build/dep-1/out/gen.rs",
    ];
    let snapshot_before: Vec<_> = watched.iter().map(|p| before(p)).collect();
    let env: BTreeMap<String, String> = BTreeMap::new();
    let rw = lane.sandbox_rw_paths(&env);
    let dest = ov.dest.display().to_string();
    let script = format!(
        "set -e; cd '{dest}'; \
         grep -q 'primary rlib' deps/libdep-1.rlib; \
         echo seat >> deps/libdep-1.rlib; \
         printf 'truncated' > build/dep-1/out/gen.rs; \
         echo seat > .fingerprint/dep-1/lib-dep; \
         ln deps/libdep-1.rlib deps/linked.rlib; \
         echo new > deps/new.rlib"
    );
    let argv = crate::session::lane_job_bwrap_argv(
        &lane.seat,
        &rw,
        &[],
        "/bin/sh",
        vec!["-c".into(), script],
    );
    let (program, args) = wrap_lane_job("unshare", &ov, "bwrap", argv).unwrap();
    let out = std::process::Command::new(program)
        .args(&args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    // The seat saw the primary's artifact (warm) and its writes landed in
    // its own upper layer.
    assert!(std::fs::read_to_string(ov.upper.join("deps/libdep-1.rlib"))
        .unwrap()
        .ends_with("seat\n"));
    assert!(ov.upper.join("deps/new.rlib").is_file());
    for (path, was) in watched.iter().zip(&snapshot_before) {
        assert_eq!(&before(path), was, "{path} changed in the primary");
    }
    assert_eq!(inodes(&agent), primary_files);
    let shared: Vec<_> = inodes(&ov.upper)
        .intersection(&primary_files)
        .copied()
        .collect();
    assert!(shared.is_empty(), "upper shares primary inodes: {shared:?}");
}

fn bwrap_runs() -> bool {
    std::process::Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--dev", "/dev", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

use std::collections::BTreeMap;

/// A fixture base under the repository's `target/`: not under /tmp, where
/// the lane-job profile mounts a private tmpfs.
fn live_base() -> tempfile::TempDir {
    let base_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/bug-2383-bwrap");
    std::fs::create_dir_all(&base_dir).unwrap();
    // A worktree's target/ may be a symlink, which bwrap cannot bind through.
    let base_dir = std::fs::canonicalize(base_dir).unwrap();
    tempfile::tempdir_in(base_dir).unwrap()
}

/// Skip a live test where userns overlay mounts or bubblewrap cannot run,
/// unless the forced-live CI gate requires it.
fn skip_live(why: &str) {
    assert!(
        std::env::var_os("AIDA_REQUIRE_BWRAP_LIVE").is_none(),
        "AIDA_REQUIRE_BWRAP_LIVE requires this live test to run: {why}"
    );
    eprintln!("SKIP ({why})");
}

// Strict review P1 (c): job N plants an upperdir symlink, wherever a job can
// write, and job N+1's writes still land in the lane's host-only upper, never
// through the symlink. Job N cannot touch the real upper at all, and a mount
// destination turned into a symlink stops the next mount. Runs the
// production snapshot, lane overlay and wrapper around the real lane-job
// bwrap profile. trace:BUG-2383 | ai:claude
#[test]
fn bug_2383_bwrap_a_planted_upper_symlink_never_redirects_a_later_job() {
    let base = live_base();
    let target = primary(base.path());
    let lane = lane_under_primary(base.path());
    let seat_target = base.path().join("target/worktrees/seat");
    let generation = snapshot(&target, "agent", "lease-ov", Duration::from_secs(5)).unwrap();
    let ov = prepare_lane(&target, &seat_target, "agent", "lease-ov", &generation).unwrap();
    if !overlay_usable(&ov) || !bwrap_runs() {
        return skip_live(&format!(
            "userns overlay or bwrap unavailable: {:?}",
            probe("unshare", &ov)
        ));
    }
    let env: BTreeMap<String, String> = BTreeMap::new();
    let rw = lane.sandbox_rw_paths(&env);
    // (d) The upper, work and generation record are outside every root a
    // job or its seat can write.
    for host_only in [&ov.upper, &ov.work, &generation.dir] {
        for root in rw.iter().chain([&lane.seat]) {
            assert!(
                !host_only.starts_with(root),
                "{} is inside job-writable {}",
                host_only.display(),
                root.display()
            );
        }
    }
    let victim = base.path().join("victim");
    std::fs::create_dir_all(&victim).unwrap();
    let run = |script: String, overlay: bool| {
        let argv = crate::session::lane_job_bwrap_argv(
            &lane.seat,
            &rw,
            &[],
            "/bin/sh",
            vec!["-c".into(), script],
        );
        let (program, args) = if overlay {
            wrap_lane_job("unshare", &ov, "bwrap", argv).unwrap()
        } else {
            ("bwrap".to_string(), argv)
        };
        std::process::Command::new(program)
            .args(&args)
            .output()
            .unwrap()
    };
    let (upper, victim_s, st, dest) = (
        ov.upper.display().to_string(),
        victim.display().to_string(),
        seat_target.display().to_string(),
        ov.dest.display().to_string(),
    );
    // Job N: plant the symlink at the old in-seat-target upper (writable)
    // and try the real one (read-only to a job).
    let planted = run(
        format!(
            "mkdir -p '{st}/overlay' && rm -rf '{st}/overlay/upper' && ln -s '{victim_s}' '{st}/overlay/upper' || exit 3; \
             rm -rf '{upper}' 2>/dev/null; ln -sfn '{victim_s}' '{upper}' 2>/dev/null; exit 0"
        ),
        true,
    );
    assert!(planted.status.success(), "{planted:?}");
    assert!(std::fs::symlink_metadata(seat_target.join("overlay/upper"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(
        std::fs::symlink_metadata(&ov.upper).unwrap().is_dir(),
        "a job replaced the real upper"
    );
    // Job N+1 writes through the overlay: into the real upper only.
    let wrote = run(format!("echo canary > '{dest}/canary'"), true);
    assert!(wrote.status.success(), "{wrote:?}");
    assert!(ov.upper.join("canary").is_file());
    assert_eq!(
        std::fs::read_dir(&victim).unwrap().count(),
        0,
        "a job wrote through the planted symlink"
    );
    // A job on no overlay (a cold lane) turns the mount destination into a
    // symlink: prepare_lane refuses it, and so does the mount script.
    let swapped = run(
        format!("rm -rf '{dest}' && ln -s '{victim_s}' '{dest}'"),
        false,
    );
    assert!(swapped.status.success(), "{swapped:?}");
    assert!(prepare_lane(&target, &seat_target, "agent", "lease-ov", &generation).is_err());
    let refused = run(format!("echo canary > '{dest}/canary2'"), true);
    assert_eq!(refused.status.code(), Some(125), "{refused:?}");
    assert_eq!(std::fs::read_dir(&victim).unwrap().count(), 0);
}

// Strict review P2 / AC2 (accf30b1): on a host without reflinks the first
// lane job already sees a dependency the primary built as Fresh. A fixture
// primary builds a tiny crate with one dependency; the seat runs `cargo
// build -v` through the real lane job runner, bubblewrap and overlay.
// trace:BUG-2383 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-157.accf30b1 | ai:claude
#[test]
fn bug_2383_bwrap_overlay_first_job_sees_a_prebuilt_dependency_as_fresh() {
    let Some(cargo) = std::env::var_os("CARGO").map(PathBuf::from) else {
        return skip_live("no CARGO in the test environment");
    };
    if !bwrap_runs() {
        return skip_live("bwrap unavailable");
    }
    let base = live_base();
    let root = base.path();
    let lease = "lease-fresh";
    // The dependency sits at one absolute path both checkouts name, as a
    // registry dependency does.
    let dep = root.join("vendor/fixture-dep");
    std::fs::create_dir_all(dep.join("src")).unwrap();
    std::fs::write(
        dep.join("Cargo.toml"),
        "[package]\nname = \"fixture-dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(dep.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n").unwrap();
    let manifest = format!(
        "[package]\nname = \"fixture-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nfixture-dep = {{ path = '{}' }}\n\n\
         [profile.agent]\ninherits = \"dev\"\n\n[workspace]\n",
        dep.display()
    );
    let checkout = |name: &str| {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), &manifest).unwrap();
        std::fs::write(
            dir.join("src/main.rs"),
            "fn main() { println!(\"{}\", fixture_dep::answer()); }\n",
        )
        .unwrap();
        dir
    };
    let primary_dir = checkout("primary");
    let seat = checkout("seat");
    std::fs::create_dir_all(seat.join(".git")).unwrap();
    std::fs::create_dir_all(seat.join(".aida")).unwrap();
    std::fs::write(
        seat.join(".aida/config.toml"),
        "[contained]\nos_wrap = true\n",
    )
    .unwrap();
    let main_target = root.join("target");
    let seat_target = main_target.join("worktrees/seat");
    // CI sets CARGO_TERM_COLOR=always; the job's cargo output is matched
    // as text, so colour is off in the environment every build here gets.
    let mut host_env = crate::lane_job::scrub_host_env(
        std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))),
    );
    host_env.retain(|(k, _)| k != "CARGO_TERM_COLOR");
    host_env.push(("CARGO_TERM_COLOR".into(), "never".into()));
    // The primary builds with the same environment a job gets.
    let built = std::process::Command::new(&cargo)
        .args(["build", "--profile", "agent", "-v"])
        .current_dir(&primary_dir)
        .env_clear()
        .envs(host_env.iter().cloned())
        .env("CARGO_TARGET_DIR", &main_target)
        .env("RUSTC_WRAPPER", "")
        .env(
            crate::lane_job::OFFLINE_ENV.0,
            crate::lane_job::OFFLINE_ENV.1,
        )
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "the primary build failed: {built:?}"
    );
    // Diagnose fixture/layout failures at the primary build, before a cold lane
    // fallback obscures the successful cargo output. trace:BUG-2539 | ai:codex
    assert!(
        generation_id(&main_target, "agent").is_some(),
        "the primary build produced no recognized fingerprints: {built:?}"
    );
    std::fs::copy(primary_dir.join("Cargo.lock"), seat.join("Cargo.lock")).unwrap();
    let sessions = root.join(".aida/sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join(format!("{lease}.toml")),
        format!("id = \"{lease}\"\n"),
    )
    .unwrap();
    // Take the overlay path even on a reflink-capable filesystem: the lane
    // already ran on an overlay (an older generation), so no reflink seed.
    let earlier = Generation {
        id: "agent-0-0".into(),
        dir: main_target.join(LANE_BASE).join("agent-0-0"),
    };
    prepare_lane(&main_target, &seat_target, "agent", lease, &earlier).unwrap();
    let lane = crate::lane_job::LaneJobs {
        inbox: seat.join(crate::lane_job::INBOX_RELATIVE),
        results: crate::lane_job::results_dir(root, lease),
        seat: seat.clone(),
        lease_id: lease.into(),
        build_env: vec![
            ("CARGO_TARGET_DIR".into(), seat_target.display().to_string()),
            ("RUSTC_WRAPPER".into(), String::new()),
        ],
        host_env,
        program: cargo.display().to_string(),
        sandbox: crate::lane_job::Sandbox::Bwrap,
    };
    lane.prepare().unwrap();
    let runner = crate::lane_job::Runner::start(lane.clone());
    let id = crate::lane_job::new_id();
    let job = crate::lane_job::JobRequest {
        argv: ["cargo", "build", "--profile", "agent", "-v"]
            .map(String::from)
            .to_vec(),
        env: BTreeMap::new(),
    };
    crate::lane_job::submit(&lane.inbox, &id, &job).unwrap();
    let waited = crate::lane_job::wait_for(
        &lane.inbox,
        &lane.results,
        &id,
        Duration::from_secs(600),
        Duration::from_secs(600),
        |_, _| {},
    );
    drop(runner);
    let output =
        std::fs::read_to_string(crate::lane_job::out_path(&lane.results, &id)).unwrap_or_default();
    eprintln!("---- lane job output (host-witnessed) ----\n{output}---- end ----");
    if output.contains("seed: cold (") || output.contains("lane job refused") {
        return skip_live(&format!(
            "the lane could not use the overlay or bwrap: {output}"
        ));
    }
    assert_eq!(waited, crate::lane_job::Waited::Done(0), "{output}");
    assert!(output.contains("seed: overlay gen="), "{output}");
    assert!(output.contains("on the lane's target overlay"), "{output}");
    assert!(
        cargo_reports_fresh(&output, "fixture-dep"),
        "the pre-built dependency was not Fresh:\n{output}"
    );
}

/// The fixture crate a lane job tests for real (review r7, F2): no
/// dependencies, one passing and one failing unit test.
const FIXTURE_CRATE_TOML: &str = "[package]\nname = \"bug-2383-lane-fixture\"\n\
    version = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n\n\
    [profile.agent]\ninherits = \"dev\"\n\n[workspace]\n";
const FIXTURE_CRATE_LIB: &str = "pub fn answer() -> u32 { 42 }\n\n\
    #[cfg(test)]\nmod tests {\n    \
    #[test]\n    fn lane_fixture_passes() { assert_eq!(super::answer(), 42); }\n    \
    #[test]\n    fn lane_fixture_fails() { assert_eq!(super::answer(), 41, \"fixture failure\"); }\n}\n";

// Review r7, F2 / AC1 (acd61bb0) and AC4: a headless lane runs
// `AIDA_CARGO_TIMEOUT=900 cargo test --profile agent -p <crate> --lib --
// <filter>` with REAL cargo against a real fixture crate, through the
// published runner and the real lane-job bubblewrap sandbox, submitted and
// polled through the `aida lane job run|wait` code path. The passing filter
// exits 0 with libtest's own `ok` result, the failing one exits 101 with its
// `FAILED` result, both take a cargo slot through the limiter on the job's
// PATH, and the tally counts two executed test runs, so the result line
// carries no `untested=true`. trace:BUG-2383 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-157.acd61bb0 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-157.Test | ai:claude
#[test]
fn bug_2383_bwrap_real_cargo_test_job_reports_pass_and_fail_through_the_cli() {
    let Some(cargo) = std::env::var_os("CARGO").map(PathBuf::from) else {
        return skip_live("no CARGO in the test environment");
    };
    if !bwrap_runs() {
        return skip_live("bwrap unavailable");
    }
    let base = live_base();
    let root = base.path();
    let lease = "lease-real";
    let seat = root.join("seat");
    std::fs::create_dir_all(seat.join("src")).unwrap();
    std::fs::write(seat.join("Cargo.toml"), FIXTURE_CRATE_TOML).unwrap();
    std::fs::write(seat.join("src/lib.rs"), FIXTURE_CRATE_LIB).unwrap();
    std::fs::create_dir_all(seat.join(".git")).unwrap();
    std::fs::create_dir_all(seat.join(".aida")).unwrap();
    std::fs::write(
        seat.join(".aida/config.toml"),
        "[contained]\nos_wrap = true\n",
    )
    .unwrap();
    // The slot limiter: the `cargo` on the job's PATH records the slot it
    // takes in the sandbox-writable slot directory, then runs real cargo.
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    let runtime = root.join("run");
    let slots = runtime.join(format!("cargo-slots-{uid}"));
    std::fs::create_dir_all(&slots).unwrap();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let limiter = bin.join("cargo");
    crate::test_exec::write_executable(
        &limiter,
        &format!(
            "#!/bin/sh\necho \"slot taken: $*\" >> \"$XDG_RUNTIME_DIR/cargo-slots-{uid}/slots.log\" || exit 97\n\
             exec '{}' \"$@\"\n",
            cargo.display()
        ),
    );
    let mut host_env = crate::lane_job::scrub_host_env(
        std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))),
    );
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    host_env
        .retain(|(k, _)| !matches!(k.as_str(), "PATH" | "XDG_RUNTIME_DIR" | "CARGO_TERM_COLOR"));
    host_env.push(("PATH".into(), path));
    host_env.push(("XDG_RUNTIME_DIR".into(), runtime.display().to_string()));
    host_env.push(("CARGO_TERM_COLOR".into(), "never".into()));
    let seat_target = root.join("target/worktrees/seat");
    let lane = crate::lane_job::LaneJobs {
        inbox: seat.join(crate::lane_job::INBOX_RELATIVE),
        results: crate::lane_job::results_dir(root, lease),
        seat: seat.clone(),
        lease_id: lease.into(),
        build_env: vec![
            ("CARGO_TARGET_DIR".into(), seat_target.display().to_string()),
            ("RUSTC_WRAPPER".into(), String::new()),
        ],
        host_env,
        program: "cargo".into(),
        sandbox: crate::lane_job::Sandbox::Bwrap,
    };
    lane.prepare().unwrap();
    let runner = crate::lane_job::Runner::start(lane.clone());
    // `aida lane job run` returns at once (wait 0) with exit 75, then
    // `aida lane job wait <id>` polls until the job finishes.
    let through_cli = |filter: &str| -> (i32, String, String) {
        let mut job = crate::lane_cmd::job_from(
            [
                "cargo",
                "test",
                "--profile",
                "agent",
                "-p",
                "bug-2383-lane-fixture",
                "--lib",
                "--",
                filter,
            ]
            .map(String::from)
            .to_vec(),
            |k| (k == "AIDA_CARGO_TIMEOUT").then(|| "900".to_string()),
        );
        assert_eq!(crate::lane_job::validate(&job), Ok(()));
        job.env.retain(|k, _| k == "AIDA_CARGO_TIMEOUT");
        let mut printed = Vec::new();
        let mut rc =
            crate::lane_cmd::submit_and_wait(&lane.inbox, &lane.results, &job, 0, 40, &mut printed)
                .unwrap();
        assert_eq!(
            rc,
            crate::lane_job::STILL_RUNNING_EXIT,
            "run returns before the job ends"
        );
        let text = String::from_utf8_lossy(&printed).into_owned();
        let id = text
            .split_whitespace()
            .nth(2)
            .filter(|id| crate::lane_job::valid_id(id))
            .unwrap_or_else(|| panic!("no job id in {text}"))
            .to_string();
        assert!(text.contains("(timeout 900s)"), "{text}");
        for _ in 0..24 {
            if rc != crate::lane_job::STILL_RUNNING_EXIT {
                break;
            }
            rc = crate::lane_cmd::wait_in(&lane.inbox, &lane.results, &id, 50, 40, &mut printed)
                .unwrap();
        }
        let printed = String::from_utf8_lossy(&printed).into_owned();
        let output = std::fs::read_to_string(crate::lane_job::out_path(&lane.results, &id))
            .unwrap_or_default();
        eprintln!("---- lane job {filter} (host-witnessed) ----\n{output}---- end ----");
        (rc, output, printed)
    };
    let (pass_rc, pass_out, pass_cli) = through_cli("lane_fixture_passes");
    let (fail_rc, fail_out, fail_cli) = through_cli("lane_fixture_fails");
    let tally = runner.stop();
    assert!(!pass_out.contains("lane job refused"), "{pass_out}");
    assert_eq!(pass_rc, 0, "{pass_out}");
    assert!(pass_out.contains("running 1 test"), "{pass_out}");
    assert!(
        pass_out.contains("test tests::lane_fixture_passes ... ok"),
        "{pass_out}"
    );
    assert!(
        pass_out.contains("test result: ok. 1 passed; 0 failed"),
        "{pass_out}"
    );
    assert!(pass_cli.contains("finished: exit 0"), "{pass_cli}");
    assert_eq!(fail_rc, 101, "{fail_out}");
    assert!(
        fail_out.contains("test tests::lane_fixture_fails ... FAILED"),
        "{fail_out}"
    );
    assert!(fail_out.contains("fixture failure"), "{fail_out}");
    assert!(
        fail_out.contains("test result: FAILED. 0 passed; 1 failed"),
        "{fail_out}"
    );
    assert!(fail_cli.contains("finished: exit 101"), "{fail_cli}");
    // Both ran inside the real sandbox, through the limiter on PATH.
    let ran_with = format!("cargo is {}, run inside bubblewrap", limiter.display());
    for out in [&pass_out, &fail_out] {
        // The seed note names the seat target: `lane job: seat target <path>: seed: ...`.
        assert!(out.contains("lane job: seat target "), "{out}");
        assert!(out.contains(": seed: "), "{out}");
        assert!(out.contains(&ran_with), "{out}");
    }
    let slot_log = std::fs::read_to_string(slots.join("slots.log")).unwrap();
    for filter in ["lane_fixture_passes", "lane_fixture_fails"] {
        let line =
            format!("slot taken: test --profile agent -p bug-2383-lane-fixture --lib -- {filter}");
        assert!(slot_log.contains(&line), "{slot_log}");
    }
    let log = std::fs::read_to_string(lane.results.join("jobs.log")).unwrap();
    assert_eq!(log.matches("(timeout 900s; ").count(), 2, "{log}");
    assert!(
        log.contains(" exited rc=0 ") && log.contains(" exited rc=101 "),
        "{log}"
    );
    // The tally: two completed, executed test runs; the result line says
    // nothing about untested.
    use std::sync::atomic::Ordering;
    assert_eq!(tally.completed.load(Ordering::Relaxed), 2);
    assert_eq!(tally.tested.load(Ordering::Relaxed), 2);
    assert_eq!(tally.incomplete.load(Ordering::Relaxed), 0);
    assert!(!tally.untested());
    let line = crate::standalone_implementer::with_untested(
        "result: pr=1".into(),
        &crate::standalone_implementer::Outcome::PrOpened { pr: 1, head: None },
        tally.untested(),
    );
    assert_eq!(line, "result: pr=1");
}

/// `text` without ANSI CSI escape sequences (`ESC [ ... final-byte`).
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.next() == Some('[') {
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

/// Whether cargo's output names `krate` as `Fresh`, whatever its colour and
/// column padding.
// trace:BUG-2383 | ai:claude
fn cargo_reports_fresh(output: &str, krate: &str) -> bool {
    strip_ansi(output).lines().any(|l| {
        let mut words = l.split_whitespace();
        words.next() == Some("Fresh") && words.next() == Some(krate)
    })
}

#[test]
fn the_fresh_match_ignores_ansi_colour_and_padding() {
    let ci = "\u{1b}[92m       Fresh\u{1b}[0m fixture-dep v0.1.0\n";
    assert!(cargo_reports_fresh(ci, "fixture-dep"));
    assert!(cargo_reports_fresh(
        "  Fresh   fixture-dep v0.1.0",
        "fixture-dep"
    ));
    assert!(!cargo_reports_fresh(
        "\u{1b}[92m   Compiling\u{1b}[0m fixture-dep v0.1.0",
        "fixture-dep"
    ));
    assert!(!cargo_reports_fresh(
        "Fresh fixture-dep-extra v0.1.0",
        "fixture-dep"
    ));
}
