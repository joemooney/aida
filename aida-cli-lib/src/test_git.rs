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
/// private sibling and renames it into place; a loser whose rename finds the
/// directory already there discards its copy and uses the winner's.
// trace:TASK-1955 | ai:claude (TASK-2017)
#[allow(dead_code)]
fn ensure_template_dir(parent: &std::path::Path) -> std::path::PathBuf {
    // FNV-1a: stable across Rust releases and processes, unlike DefaultHasher.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in TEMPLATE_CONFIG.bytes().chain(TEMPLATE_DESCRIPTION.bytes()) {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    let dir = parent.join(format!("aida-test-git-template-{hash:016x}"));
    let complete = |dir: &std::path::Path| {
        std::fs::read_to_string(dir.join("config")).is_ok_and(|c| c == TEMPLATE_CONFIG)
            && dir.join("info/exclude").is_file()
            && dir.join("hooks").is_dir()
    };
    if complete(&dir) {
        return dir;
    }
    std::fs::create_dir_all(parent).expect("create git template parent dir");
    let staging = tempfile::Builder::new()
        .prefix(".aida-test-git-template-staging-")
        .tempdir_in(parent)
        .expect("create git template staging dir");
    let root = staging.path();
    std::fs::create_dir_all(root.join("hooks")).unwrap();
    std::fs::create_dir_all(root.join("info")).unwrap();
    std::fs::write(root.join("info/exclude"), "").unwrap();
    std::fs::write(root.join("description"), TEMPLATE_DESCRIPTION).unwrap();
    std::fs::write(root.join("config"), TEMPLATE_CONFIG).unwrap();
    if std::fs::rename(root, &dir).is_err() && !complete(&dir) {
        // Not a lost race: a stale or partial directory (say, half-removed by
        // a temp cleaner). Replace it.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::rename(root, &dir).expect("install git template dir");
    }
    // The staging TempDir is now gone (renamed) or a loser's copy; dropping it
    // removes whatever is left.
    drop(staging);
    dir
}

// trace:BUG-2191.aca71745 | ai:claude
#[test]
fn git_command_turns_off_auto_maintenance() {
    let command = git_command();
    assert_eq!(command.get_program(), "git");
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args[..4], aida_core::git_ops::NO_AUTO_MAINTENANCE);
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
