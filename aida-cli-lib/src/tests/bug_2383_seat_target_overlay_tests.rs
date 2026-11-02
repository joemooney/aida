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
    let later = std::time::SystemTime::now() + Duration::from_secs(120);
    filetime_set(&target.join("agent/.fingerprint/dep-1"), later);
    assert_ne!(generation_id(&target, "agent").unwrap(), other_toolchain);
    std::fs::remove_dir_all(target.join("agent/.fingerprint")).unwrap();
    assert!(generation_id(&target, "agent").is_none());
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
    let generation = snapshot(&target, "agent", Duration::from_secs(5)).unwrap();
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
    let again = snapshot(&target, "agent", Duration::from_secs(5)).unwrap();
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
    let why = snapshot(&target, "agent", Duration::from_millis(300)).unwrap_err();
    assert!(why.contains("primary's build is still running"), "{why}");
    assert!(std::fs::read_dir(target.join(LANE_BASE))
        .unwrap()
        .flatten()
        .all(|e| e.file_name().to_string_lossy().starts_with('.')));
    drop(held);
    assert!(snapshot(&target, "agent", Duration::from_secs(5)).is_ok());
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
        std::fs::create_dir_all(generation.dir.join("deps")).unwrap();
        for lease in leases {
            pin(&generation, lease).unwrap();
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
    let mut removed = gc(&target, main, Some(&kept.dir));
    removed.sort();
    assert_eq!(removed, vec![dead.dir.clone(), unpinned.dir.clone()]);
    assert!(pinned.dir.is_dir() && kept.dir.is_dir());
    assert!(pinned.dir.join(".pins/live").exists());
    assert!(
        !pinned.dir.join(".pins/gone").exists(),
        "dead pins are pruned"
    );
    std::fs::remove_file(sessions.join("live.toml")).unwrap();
    let mut removed = gc(&target, main, None);
    removed.sort();
    assert_eq!(removed, vec![pinned.dir.clone(), kept.dir.clone()]);
    assert!(target.join(LANE_BASE).join(".agent-5.tmp-1").is_dir());
}

// The lane's upper survives between jobs on one generation and is
// discarded when the lane moves to another.
#[test]
fn a_lane_upper_is_kept_per_generation_and_reset_on_a_new_one() {
    let tmp = tempfile::tempdir().unwrap();
    let seat_target = tmp.path().join("target/worktrees/seat");
    let generation = |id: &str| Generation {
        id: id.into(),
        dir: tmp.path().join(id),
    };
    let first = prepare_lane(&seat_target, "agent", &generation("agent-1-a")).unwrap();
    assert_eq!(first.dest, seat_target.join("agent"));
    assert!(lane_has_overlay(&seat_target));
    std::fs::write(first.upper.join("built"), "x").unwrap();
    let same = prepare_lane(&seat_target, "agent", &generation("agent-1-a")).unwrap();
    assert!(same.upper.join("built").exists());
    let next = prepare_lane(&seat_target, "agent", &generation("agent-2-a")).unwrap();
    assert_eq!(next.lower, tmp.path().join("agent-2-a"));
    assert!(!next.upper.join("built").exists());
}

#[test]
fn the_wrapper_mounts_then_execs_the_unchanged_bwrap_argv() {
    let ov = LaneOverlay {
        generation: "agent-1-a".into(),
        lower: "/m/target/lane-base/agent-1-a".into(),
        upper: "/m/target/worktrees/s/overlay/upper".into(),
        work: "/m/target/worktrees/s/overlay/work".into(),
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
    prepare_lane(&tmp.path().join("target/worktrees/seat"), "agent", &earlier).unwrap();
    let note = crate::lane_job::seed_lane_target_with(&lane, "/nonexistent/unshare");
    assert!(note.cold);
    assert!(note.overlay.is_none());
    assert!(note.note.contains("seed: cold ("), "{}", note.note);
    assert!(note.note.contains("overlay:"), "{}", note.note);
    // No primary fingerprints: cold too, with the reason.
    let bare = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(bare.path().join("target/agent")).unwrap();
    prepare_lane(
        &bare.path().join("target/worktrees/seat"),
        "agent",
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

/// Whether `unshare -Urm` can mount an overlay here.
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
    let base = tempfile::tempdir_in(base_dir).unwrap();
    let target = primary(base.path());
    let lane = lane_under_primary(base.path());
    let seat_target = base.path().join("target/worktrees/seat");
    let generation = snapshot(&target, "agent", Duration::from_secs(5)).unwrap();
    let ov = prepare_lane(&seat_target, "agent", &generation).unwrap();
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
    let (program, args) = wrap("unshare", &ov, "bwrap", argv).unwrap();
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
