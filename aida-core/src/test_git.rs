//! Test fixtures spawn git through [`git_command`], which turns off automatic
//! maintenance. Since git 2.54 every commit, merge or fetch starts a detached
//! geometric repack that can prune an `.git/objects/<xx>` dir while the
//! fixture's next git call writes into it ("unable to create temporary file").
//! `tests/test_git_maintenance_ratchet.py` keeps new fixtures on this helper.
// trace:BUG-2191 | ai:claude

/// `git` with `-c maintenance.auto=false -c gc.auto=0` already applied; add
/// the subcommand and its arguments as usual.
///
/// The `-c` pair does not reach the far side of a push: local transport
/// clears `GIT_CONFIG_PARAMETERS`, and a bare remote's `receive-pack` runs
/// auto-maintenance under its own config. So the command also sets
/// `init.templateDir` to [`template_dir`], whose `config` turns maintenance
/// off; every repository this helper initializes or clones (bare remotes
/// included) carries it.
pub(crate) fn git_command() -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command.args(crate::git_ops::NO_AUTO_MAINTENANCE);
    command
        .arg("-c")
        .arg(format!("init.templateDir={}", template_dir().display()));
    command
}

/// The template's `config`: automatic maintenance off, for both the local
/// side and a bare remote's `receive-pack`.
#[allow(dead_code)]
const TEMPLATE_CONFIG: &str =
    "[receive]\n\tautogc = false\n[gc]\n\tauto = 0\n[maintenance]\n\tauto = false\n";
#[allow(dead_code)]
const TEMPLATE_DESCRIPTION: &str = "test fixture\n";

/// A git template directory: the default layout (`hooks/`, `info/exclude`,
/// `description`) plus a `config` with automatic maintenance off. It lives at
/// a stable path under the system temp dir, named by a hash of its contents,
/// so every test process shares one directory instead of leaking its own (a
/// `static` `TempDir` is never dropped). Resolved once per process.
// trace:TASK-1955 | ai:claude (TASK-2017)
#[allow(dead_code)]
fn template_dir() -> &'static std::path::Path {
    static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| ensure_template_dir(&std::env::temp_dir()))
}

/// Create (idempotently) the content-addressed template dir under `parent`
/// and return its path. Concurrent processes race safely: each builds a
/// private (0700) sibling and renames it into place; a loser whose rename
/// finds the directory already there discards its copy and uses the winner's.
///
/// The name carries the owner's uid, and an existing directory is used only
/// when it is a real directory owned by this user with mode 0700: a shared
/// temp dir lets another user pre-create it with a planted hook, so anything
/// else is refused. A partial directory is repaired in place, one atomic
/// rename per missing or wrong file, so a concurrent reader never finds the
/// directory itself missing (renaming it aside did; TASK-2175).
// trace:TASK-1955 | ai:claude (TASK-2017) (TASK-2175)
#[allow(dead_code)]
fn ensure_template_dir(parent: &std::path::Path) -> std::path::PathBuf {
    // FNV-1a: stable across Rust releases and processes, unlike DefaultHasher.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in TEMPLATE_CONFIG.bytes().chain(TEMPLATE_DESCRIPTION.bytes()) {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    std::fs::create_dir_all(parent).expect("create git template parent dir");
    // A dir this process just made is owned by its effective uid. tempfile
    // creates it with the umask's mode (0755 on most hosts), so it is made
    // private before anything is written or renamed into place.
    let staging = tempfile::Builder::new()
        .prefix(".aida-test-git-template-staging-")
        .tempdir_in(parent)
        .expect("create git template staging dir");
    make_private(staging.path());
    let owner = template_owner(staging.path());
    let dir = parent.join(format!("aida-test-git-template-{owner}-{hash:016x}"));
    let complete = |dir: &std::path::Path| {
        std::fs::read_to_string(dir.join("config")).is_ok_and(|c| c == TEMPLATE_CONFIG)
            && dir.join("info/exclude").is_file()
            && dir.join("hooks").is_dir()
    };
    if template_dir_trusted(&dir, owner) {
        if !complete(&dir) {
            // A partial dir (say, half-cleaned by a temp cleaner): rewrite
            // what is missing or wrong through the staging dir. Concurrent
            // repairers write the same bytes, so the last rename wins harmlessly.
            // trace:TASK-1955 | ai:claude (TASK-2175)
            std::fs::create_dir_all(dir.join("hooks")).expect("repair git template hooks");
            std::fs::create_dir_all(dir.join("info")).expect("repair git template info");
            for (rel, content) in TEMPLATE_FILES {
                if std::fs::read_to_string(dir.join(rel)).is_ok_and(|c| c == content) {
                    continue;
                }
                let tmp = staging.path().join(rel.replace('/', "-"));
                std::fs::write(&tmp, content).expect("stage git template file");
                std::fs::rename(&tmp, dir.join(rel)).expect("repair git template file");
            }
            assert!(
                complete(&dir),
                "git template dir {} is incomplete after repair",
                dir.display()
            );
        }
        return dir;
    }
    let root = staging.path();
    std::fs::create_dir_all(root.join("hooks")).unwrap();
    std::fs::create_dir_all(root.join("info")).unwrap();
    for (rel, content) in TEMPLATE_FILES {
        std::fs::write(root.join(rel), content).unwrap();
    }
    if std::fs::rename(root, &dir).is_err() {
        // Lost the race: whoever won must have installed a complete, private copy.
        assert!(
            template_dir_trusted(&dir, owner) && complete(&dir),
            "git template dir {} is incomplete after a concurrent install",
            dir.display()
        );
    }
    // The staging TempDir is now gone (renamed) or a loser's copy; dropping
    // it removes what is left.
    drop(staging);
    dir
}

/// Every file the template holds, relative to it, with its content.
#[allow(dead_code)]
const TEMPLATE_FILES: [(&str, &str); 3] = [
    ("info/exclude", ""),
    ("description", TEMPLATE_DESCRIPTION),
    ("config", TEMPLATE_CONFIG),
];

/// Set `dir`'s mode to 0700 (no-op where there are no modes).
#[allow(dead_code)]
fn make_private(dir: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .expect("make git template dir private");
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// The uid that owns `path` (0 where there are no uids).
#[allow(dead_code)]
fn template_owner(path: &std::path::Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        std::fs::symlink_metadata(path)
            .expect("stat git template staging dir")
            .uid()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        0
    }
}

/// Whether `dir` exists. Panics when it exists but is not a real directory
/// owned by `owner` that only `owner` can write, since its hooks would run in
/// every fixture repository. One that is only readable by others (say, from a
/// build that left it 0755) is tightened to 0700 and used.
// trace:TASK-1955 | ai:claude (TASK-2017)
#[allow(dead_code)]
fn template_dir_trusted(dir: &std::path::Path, owner: u32) -> bool {
    let meta = match std::fs::symlink_metadata(dir) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(e) => panic!("stat git template dir {}: {e}", dir.display()),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        assert!(
            meta.file_type().is_dir() && meta.uid() == owner && meta.mode() & 0o022 == 0,
            "refusing git template dir {}: it is not a directory owned and only writable \
             by uid {owner}; remove it and re-run",
            dir.display()
        );
        if meta.mode() & 0o777 != 0o700 {
            make_private(dir);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = owner;
        assert!(
            meta.file_type().is_dir(),
            "refusing git template dir {}: not a directory",
            dir.display()
        );
    }
    true
}

// trace:BUG-2191.aca71745 | ai:claude
#[test]
fn git_command_turns_off_auto_maintenance() {
    let command = git_command();
    assert_eq!(command.get_program(), "git");
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args[..4], crate::git_ops::NO_AUTO_MAINTENANCE);
    // A bare remote made through the helper keeps maintenance off for the
    // receive-pack side of a push, which the -c pair cannot reach.
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote.git");
    assert!(git_command()
        .args(["init", "-q", "--bare"])
        .arg(&remote)
        .status()
        .unwrap()
        .success());
    for (key, value) in [
        ("receive.autogc", "false"),
        ("gc.auto", "0"),
        ("maintenance.auto", "false"),
    ] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&remote)
            .args(["config", "--get", key])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), value, "{key}");
    }
    assert!(remote.join("info/exclude").is_file());
}

// trace:TASK-1955 | ai:claude (TASK-2017)
#[test]
fn template_dir_is_one_stable_content_addressed_dir_not_a_per_process_tempdir() {
    // The process-wide dir is the content-addressed one under the system temp
    // dir, so the next test process reuses it rather than leaking another.
    let shared = template_dir();
    assert_eq!(shared, ensure_template_dir(&std::env::temp_dir()));
    assert_eq!(shared, template_dir());
    assert!(shared.join("config").is_file());

    // Resolving again (as a second process would) is idempotent and leaves
    // no staging copies behind; a partial dir is repaired in place.
    let parent = tempfile::tempdir().unwrap();
    let first = ensure_template_dir(parent.path());
    let second = ensure_template_dir(parent.path());
    assert_eq!(first, second);
    assert!(first
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("aida-test-git-template-"));
    std::fs::remove_file(first.join("config")).unwrap();
    assert_eq!(ensure_template_dir(parent.path()), first);
    assert_eq!(
        std::fs::read_to_string(first.join("config")).unwrap(),
        TEMPLATE_CONFIG
    );
    let entries: Vec<_> = std::fs::read_dir(parent.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, [first.file_name().unwrap()], "{entries:?}");
}

// A template dir another user (or a lax umask) could have planted hooks in is
// refused, never used. trace:TASK-1955 | ai:claude (TASK-2017)
#[cfg(unix)]
#[test]
fn template_dir_refuses_a_dir_that_is_not_private_to_this_user() {
    use std::os::unix::fs::PermissionsExt as _;
    let parent = tempfile::tempdir().unwrap();
    let dir = ensure_template_dir(parent.path());
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    // Readable by others only (a dir an earlier build left 0755) is tightened.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(ensure_template_dir(parent.path()), dir);
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    // Writable by others: refused.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    let refused = std::panic::catch_unwind(|| ensure_template_dir(parent.path()));
    assert!(refused.is_err(), "a 0777 template dir must be refused");

    // A symlink in its place (say, to a dir someone else controls) is refused too.
    std::fs::remove_dir_all(&dir).unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), &dir).unwrap();
    let refused = std::panic::catch_unwind(|| ensure_template_dir(parent.path()));
    assert!(refused.is_err(), "a symlinked template dir must be refused");
}

// TASK-2175: a partial dir is repaired in place, so a reader racing the
// repair never finds the dir missing (it keeps its inode), and racing
// repairers neither panic nor leave staging copies.
// trace:TASK-1955 | ai:claude (TASK-2175)
#[cfg(unix)]
#[test]
fn template_dir_repair_keeps_the_dir_in_place_under_racing_repairers() {
    use std::os::unix::fs::MetadataExt as _;
    let parent = tempfile::tempdir().unwrap();
    let dir = ensure_template_dir(parent.path());
    let inode = std::fs::metadata(&dir).unwrap().ino();
    std::fs::remove_file(dir.join("config")).unwrap();
    std::fs::write(dir.join("description"), "tampered\n").unwrap();
    std::fs::remove_dir_all(dir.join("info")).unwrap();
    let stop = std::sync::atomic::AtomicBool::new(false);
    let barrier = std::sync::Barrier::new(9);
    std::thread::scope(|s| {
        let reader = s.spawn(|| {
            barrier.wait();
            let mut seen = 0u64;
            // Checks at least once, even if scheduled only after the repair.
            loop {
                assert!(dir.is_dir(), "the template dir went missing mid-repair");
                seen += 1;
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    break seen;
                }
            }
        });
        let repairers: Vec<_> = (0..8)
            .map(|_| {
                s.spawn(|| {
                    barrier.wait();
                    ensure_template_dir(parent.path())
                })
            })
            .collect();
        for h in repairers {
            assert_eq!(h.join().unwrap(), dir);
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(reader.join().unwrap() > 0);
    });
    assert_eq!(
        std::fs::metadata(&dir).unwrap().ino(),
        inode,
        "repaired in place"
    );
    for (rel, content) in TEMPLATE_FILES {
        assert_eq!(
            std::fs::read_to_string(dir.join(rel)).unwrap(),
            content,
            "{rel}"
        );
    }
    let entries: Vec<_> = std::fs::read_dir(parent.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, [dir.file_name().unwrap()], "{entries:?}");
}

// The first creation on an empty temp dir, raced by many threads: none
// panics, all get the one 0700 dir, and no staging copy is left behind.
// trace:TASK-1955 | ai:claude (TASK-2017)
#[cfg(unix)]
#[test]
fn template_dir_first_creation_races_safely() {
    use std::os::unix::fs::PermissionsExt as _;
    let parent = tempfile::tempdir().unwrap();
    let barrier = std::sync::Barrier::new(16);
    let dirs: Vec<std::path::PathBuf> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..16)
            .map(|_| {
                s.spawn(|| {
                    barrier.wait();
                    ensure_template_dir(parent.path())
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!(dirs.iter().all(|d| d == &dirs[0]), "{dirs:?}");
    assert_eq!(
        std::fs::metadata(&dirs[0]).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let entries: Vec<_> = std::fs::read_dir(parent.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, [dirs[0].file_name().unwrap()], "{entries:?}");
}
