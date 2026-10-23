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
#[allow(dead_code)]
pub fn git_command() -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command.args(aida_core::git_ops::NO_AUTO_MAINTENANCE);
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
/// else is refused. A partial directory is repaired atomically: it is renamed
/// aside into a fresh `mkdtemp` dir, never removed in place while another
/// process may be installing over it.
// trace:TASK-1955 | ai:claude (TASK-2017)
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
    let existed = template_dir_trusted(&dir, owner);
    if existed && complete(&dir) {
        return dir;
    }
    let root = staging.path();
    std::fs::create_dir_all(root.join("hooks")).unwrap();
    std::fs::create_dir_all(root.join("info")).unwrap();
    std::fs::write(root.join("info/exclude"), "").unwrap();
    std::fs::write(root.join("description"), TEMPLATE_DESCRIPTION).unwrap();
    std::fs::write(root.join("config"), TEMPLATE_CONFIG).unwrap();
    // Move a partial dir (say, half-removed by a temp cleaner) aside; another
    // process may already have moved it, which is fine.
    let aside = tempfile::Builder::new()
        .prefix(".aida-test-git-template-stale-")
        .tempdir_in(parent)
        .expect("create git template stale dir");
    if existed {
        let _ = std::fs::rename(&dir, aside.path().join("old"));
    }
    if std::fs::rename(root, &dir).is_err() {
        // Lost the race: whoever won must have installed a complete, private copy.
        assert!(
            template_dir_trusted(&dir, owner) && complete(&dir),
            "git template dir {} is incomplete after a concurrent install",
            dir.display()
        );
    }
    // The staging TempDir is now gone (renamed) or a loser's copy, and the
    // stale one holds the moved-aside dir; dropping them removes what is left.
    drop(staging);
    drop(aside);
    dir
}

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
