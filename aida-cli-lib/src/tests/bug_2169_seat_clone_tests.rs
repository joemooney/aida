//! BUG-2169 slice S1: a contained seat's private clone. Creation, the base
//! pin, the loud refusal (no worktree fallback), the lease `tree_kind`, the
//! file-only main resolver, and condition 1 of the advisor's signoff: no git
//! runs with a seat clone as its repository outside this module.
//! trace:BUG-2169 | ai:claude

use super::*;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn git_ok(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub(super) fn ref_exists(dir: &Path, name: &str) -> bool {
    crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--verify", "--quiet", name])
        .output()
        .unwrap()
        .status
        .success()
}

/// A main checkout with one commit, a bare `origin` it pushed to, and
/// `origin/main` fetched. Paths are canonical.
pub(super) struct Fixture {
    _tmp: tempfile::TempDir,
    pub(super) root: PathBuf,
    pub(super) main: PathBuf,
    pub(super) origin: PathBuf,
}

pub(super) fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let main = root.join("aida");
    std::fs::create_dir_all(&main).unwrap();
    git_ok(&main, &["init", "-q", "-b", "main"]);
    git_ok(&main, &["config", "user.email", "test@example.com"]);
    git_ok(&main, &["config", "user.name", "AIDA Test"]);
    std::fs::write(main.join("README.md"), "seed\n").unwrap();
    git_ok(&main, &["add", "README.md"]);
    git_ok(&main, &["commit", "-qm", "seed"]);
    let origin = root.join("origin.git");
    git_ok(
        &root,
        &[
            "clone",
            "-q",
            "--bare",
            main.to_str().unwrap(),
            origin.to_str().unwrap(),
        ],
    );
    git_ok(
        &main,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git_ok(&main, &["fetch", "-q", "origin"]);
    Fixture {
        _tmp: tmp,
        root,
        main,
        origin,
    }
}

/// Write a session lease for `clone` into `main`, the way session_start does.
fn write_lease(main: &Path, clone: &Path, kind: &str) {
    let id = "b2169c10e5e1";
    let path = crate::lease_path(main, id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!(
            "id = \"{id}\"\nscope = \"BUG-2169\"\nslug = \"bug-2169\"\nowner = \"test\"\n\
             worktree_path = {}\nbranch = \"feat\"\nstarted_at = \"2026-10-16T00:00:00Z\"\n\
             hostname = \"h\"\ntree_kind = \"{kind}\"\n",
            aida_core::toml_quote::toml_string(&clone.display().to_string())
        ),
    )
    .unwrap();
}

#[test]
fn knob_absent_is_clone_worktree_opts_out_and_a_typo_refuses() {
    // TASK-2053 S4c: the default flipped to `clone`.
    assert_eq!(parse_seat_tree_knob(None).unwrap(), TreeKind::Clone);
    let clone = toml::Value::String("clone".into());
    assert_eq!(parse_seat_tree_knob(Some(&clone)).unwrap(), TreeKind::Clone);
    let worktree = toml::Value::String("Worktree".into());
    assert_eq!(
        parse_seat_tree_knob(Some(&worktree)).unwrap(),
        TreeKind::Worktree
    );
    let typo = toml::Value::String("clones".into());
    let err = parse_seat_tree_knob(Some(&typo)).unwrap_err().to_string();
    assert!(err.contains("not recognised"), "{err}");
    let not_a_string = toml::Value::Boolean(true);
    assert!(parse_seat_tree_knob(Some(&not_a_string)).is_err());
}

#[test]
fn only_a_contained_seat_under_the_clone_knob_gets_a_clone() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[contained]\nseat_tree = \"clone\"\n",
    )
    .unwrap();
    assert_eq!(tree_kind_for_launch(root, true).unwrap(), TreeKind::Clone);
    assert_eq!(
        tree_kind_for_launch(root, false).unwrap(),
        TreeKind::Worktree
    );
    // TASK-2053 S4c: with no knob a contained seat gets a clone; an
    // uncontained one still gets a worktree.
    let bare = tempfile::tempdir().unwrap();
    assert_eq!(
        tree_kind_for_launch(bare.path(), true).unwrap(),
        TreeKind::Clone
    );
    assert_eq!(
        tree_kind_for_launch(bare.path(), false).unwrap(),
        TreeKind::Worktree
    );
    // `worktree` opts a contained seat out.
    std::fs::write(
        root.join(".aida/config.toml"),
        "[contained]\nseat_tree = \"worktree\"\n",
    )
    .unwrap();
    assert_eq!(
        tree_kind_for_launch(root, true).unwrap(),
        TreeKind::Worktree
    );
}

#[test]
fn a_pre_bug_2169_lease_reads_as_worktree_and_a_worktree_lease_writes_no_key() {
    let old = "id = \"a\"\nscope = \"S\"\nslug = \"s\"\nowner = \"o\"\nworktree_path = \"/w\"\n\
               branch = \"b\"\nstarted_at = \"2026-10-16T00:00:00Z\"\nhostname = \"h\"\n";
    let mut lease: crate::leases::SessionLease = toml::from_str(old).unwrap();
    assert_eq!(lease.tree_kind, TreeKind::Worktree);
    assert!(!toml::to_string(&lease).unwrap().contains("tree_kind"));
    lease.tree_kind = TreeKind::Clone;
    let written = toml::to_string(&lease).unwrap();
    assert!(written.contains("tree_kind = \"clone\""), "{written}");
    let back: crate::leases::SessionLease = toml::from_str(&written).unwrap();
    assert_eq!(back.tree_kind, TreeKind::Clone);
}

#[test]
fn create_makes_a_reference_clone_pins_the_base_and_writes_the_marker() {
    let fx = fixture();
    let dest = fx.root.join("seats").join("aida-seat-aida-bug-2169");
    let base = git_ok(&fx.main, &["rev-parse", "origin/main"]);

    let made = create(&fx.main, &dest, "feat", "origin/main").unwrap();

    assert_eq!(made.base_sha, base);
    assert_eq!(made.pin, "refs/aida/seats/aida-seat-aida-bug-2169");
    assert!(
        dest.join(".git").is_dir(),
        "a clone owns a real .git directory"
    );
    let alternates = std::fs::read_to_string(dest.join(".git/objects/info/alternates")).unwrap();
    assert_eq!(
        Path::new(alternates.trim()).canonicalize().unwrap(),
        fx.main.join(".git/objects").canonicalize().unwrap(),
        "objects are borrowed from main, read-only"
    );
    assert_eq!(git_ok(&dest, &["symbolic-ref", "--short", "HEAD"]), "feat");
    assert_eq!(git_ok(&dest, &["rev-parse", "HEAD"]), base);
    assert_eq!(
        git_ok(&dest, &["config", "--get", "remote.origin.url"]),
        fx.origin.to_str().unwrap(),
        "the seat pushes to origin, never to main"
    );
    assert_eq!(git_ok(&fx.main, &["rev-parse", &made.pin]), base);
    let marker: SeatCloneMarker =
        toml::from_str(&std::fs::read_to_string(dest.join(MARKER_RELATIVE)).unwrap()).unwrap();
    assert_eq!(marker.main, fx.main);
    assert_eq!(marker.pin, made.pin);
    let exclude = std::fs::read_to_string(dest.join(".git/info/exclude")).unwrap();
    assert!(exclude.contains("/.aida/*"), "{exclude}");
    assert!(
        git_ok(&dest, &["status", "--porcelain"]).is_empty(),
        "the marker is excluded, so the clone starts clean"
    );
    // A linked worktree list of main never names the clone.
    assert!(
        !git_ok(&fx.main, &["worktree", "list", "--porcelain"]).contains(dest.to_str().unwrap())
    );

    release_pin(&fx.main, &made.pin).unwrap();
    assert!(!ref_exists(&fx.main, &made.pin));
}

#[test]
fn a_failed_clone_refuses_loudly_and_leaves_no_directory_and_no_pin() {
    let fx = fixture();
    git_ok(
        &fx.main,
        &[
            "remote",
            "set-url",
            "origin",
            fx.root.join("missing.git").to_str().unwrap(),
        ],
    );
    let dest = fx.root.join("seats").join("aida-seat-aida-x");

    let err = format!(
        "{:#}",
        create(&fx.main, &dest, "feat", "origin/main").unwrap_err()
    );

    assert!(
        err.contains("never falls back to a shared worktree"),
        "{err}"
    );
    assert!(!dest.exists(), "nothing half-made is left behind");
    assert!(!ref_exists(&fx.main, "refs/aida/seats/aida-seat-aida-x"));
    assert!(!git_ok(&fx.main, &["worktree", "list", "--porcelain"]).contains("aida-seat"));
}

// TASK-2318: an ssh origin must never prompt — the clone sets GIT_SSH_COMMAND
// to the operator's configured ssh command (env or core.sshCommand) with batch
// mode appended, so a passphrase or host-key prompt fails at once instead of
// stopping the launch for the whole 30s timeout on SIGTTIN.
// trace:TASK-2272 | ai:claude (TASK-2318)
#[cfg(unix)]
#[test]
fn task_2318_ssh_origin_clone_keeps_operator_ssh_command_and_adds_batch_mode() {
    for via_config in [false, true] {
        let fx = fixture();
        let log = fx.root.join("ssh-calls");
        let ssh = fx.root.join("fake-ssh");
        crate::test_exec::write_executable(
            &ssh,
            format!("#!/bin/sh\necho \"$*\" >> '{}'\nexit 1\n", log.display()),
        );
        let user_command = format!("'{}' --operator-wrapper", ssh.display());
        git_ok(
            &fx.main,
            &[
                "remote",
                "set-url",
                "origin",
                "ssh://forge.invalid/repo.git",
            ],
        );
        if via_config {
            git_ok(&fx.main, &["config", "core.sshCommand", &user_command]);
        }
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            (
                "GIT_SSH_COMMAND",
                (!via_config).then_some(user_command.as_str()),
            ),
            ("GIT_SSH", None),
            ("GIT_SSH_VARIANT", None),
        ]);
        let dest = fx.root.join("seats").join("aida-seat-aida-ssh");
        create(&fx.main, &dest, "feat", "origin/main").unwrap_err();
        let calls = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            calls
                .lines()
                .any(|l| l.contains("--operator-wrapper") && l.contains("BatchMode=yes")),
            "via_config={via_config}: {calls:?}"
        );
        assert!(!dest.exists(), "nothing half-made is left behind");
    }
}

#[cfg(unix)]
#[test]
fn an_unwritable_parent_refuses_before_pinning() {
    use std::os::unix::fs::PermissionsExt;
    let fx = fixture();
    let locked_dir = fx.root.join("locked");
    std::fs::create_dir_all(&locked_dir).unwrap();
    std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let dest = locked_dir.join("seats").join("aida-seat-aida-y");
    let probe = std::fs::create_dir(locked_dir.join("probe"));
    if probe.is_ok() {
        // Running as root: permissions are not enforced, nothing to prove.
        return;
    }

    let err = format!(
        "{:#}",
        create(&fx.main, &dest, "feat", "origin/main").unwrap_err()
    );

    crate::test_exec::mark_executable(&locked_dir);
    assert!(err.contains("never falls back"), "{err}");
    assert!(!ref_exists(&fx.main, "refs/aida/seats/aida-seat-aida-y"));
}

#[test]
fn create_refuses_an_existing_directory_and_an_unresolvable_base() {
    let fx = fixture();
    let taken = fx.root.join("aida-seat-taken");
    std::fs::create_dir_all(&taken).unwrap();
    let err = format!(
        "{:#}",
        create(&fx.main, &taken, "feat", "origin/main").unwrap_err()
    );
    assert!(err.contains("already exists"), "{err}");

    let dest = fx.root.join("aida-seat-nobase");
    let err = format!(
        "{:#}",
        create(&fx.main, &dest, "feat", "origin/nope").unwrap_err()
    );
    assert!(err.contains("origin/nope"), "{err}");
    assert!(!dest.exists());

    let err = format!(
        "{:#}",
        create(&fx.main, &dest, "--evil", "origin/main").unwrap_err()
    );
    assert!(err.contains("never falls back"), "{err}");
    assert!(!dest.exists());
}

#[test]
fn release_pin_touches_only_the_seat_namespace() {
    let fx = fixture();
    assert!(release_pin(&fx.main, "refs/heads/main").is_err());
    assert!(release_pin(&fx.main, PIN_PREFIX).is_err());
    assert!(ref_exists(&fx.main, "refs/heads/main"));
}

#[test]
fn pin_names_are_valid_refs_or_none() {
    assert_eq!(
        pin_ref(Path::new("/w/aida-seat-aida-bug-2169-2")).as_deref(),
        Some("refs/aida/seats/aida-seat-aida-bug-2169-2")
    );
    assert_eq!(pin_ref(Path::new("/w/bad name")), None);
    assert_eq!(pin_ref(Path::new("/w/x.lock")), None);
    assert_eq!(pin_ref(Path::new("/w/a..b")), None);
    assert_eq!(pin_ref(Path::new("/")), None);
}

#[test]
fn the_main_resolver_needs_marker_alternates_and_lease_to_agree() {
    let fx = fixture();
    let clone = fx.root.join("seats").join("aida-seat-aida-z");
    create(&fx.main, &clone, "feat", "origin/main").unwrap();
    let clone = clone.canonicalize().unwrap();

    // The main checkout and an ordinary clone are not seat clones.
    assert_eq!(main_for(&fx.main), MainFor::NotAClone);
    let plain = fx.root.join("plain");
    git_ok(
        &fx.root,
        &[
            "clone",
            "-q",
            fx.origin.to_str().unwrap(),
            plain.to_str().unwrap(),
        ],
    );
    assert_eq!(main_for(&plain), MainFor::NotAClone);

    // No lease yet: refused. The infallible resolver points at a path where
    // everything fails, never at the clone, and git is never asked.
    assert_eq!(main_for(&clone), MainFor::Unverified(clone.clone()));
    let sentinel = PathBuf::from(UNVERIFIED_MAIN_SENTINEL);
    assert_eq!(
        crate::worktree_gc::main_worktree_root_from(&clone),
        sentinel
    );
    assert_eq!(
        crate::session_end::resolve_session_end_root(&clone),
        sentinel
    );
    assert!(
        std::fs::read_dir(&sentinel).is_err()
            && std::fs::create_dir_all(sentinel.join("x")).is_err()
    );

    // A worktree-kind lease for the path does not verify a clone.
    write_lease(&fx.main, &clone, "worktree");
    assert_eq!(main_for(&clone), MainFor::Unverified(clone.clone()));

    write_lease(&fx.main, &clone, "clone");
    assert_eq!(main_for(&clone), MainFor::Verified(fx.main.clone()));
    assert_eq!(
        main_for(&clone.join(".aida")),
        MainFor::Verified(fx.main.clone()),
        "a path inside the clone resolves through its root"
    );
    assert_eq!(crate::worktree_gc::main_worktree_root_from(&clone), fx.main);
    assert_eq!(
        crate::session_end::resolve_session_end_root(&clone),
        fx.main
    );

    // A seat that rewrites its marker to name another checkout fails the
    // alternates check and is back to being its own root.
    let other = fixture();
    let marker_path = clone.join(MARKER_RELATIVE);
    let mut marker: SeatCloneMarker =
        toml::from_str(&std::fs::read_to_string(&marker_path).unwrap()).unwrap();
    marker.main = other.main.clone();
    std::fs::write(&marker_path, toml::to_string(&marker).unwrap()).unwrap();
    write_lease(&other.main, &clone, "clone");
    assert_eq!(main_for(&clone), MainFor::Unverified(clone.clone()));

    // A seat that deletes its marker is still recognised by its directory
    // name, and does not verify.
    std::fs::remove_file(&marker_path).unwrap();
    assert_eq!(main_for(&clone), MainFor::Unverified(clone.clone()));
}

/// The seat controls its marker and alternates: a FIFO in either place must
/// not hang the host, and a FIFO marker still marks a clone.
#[cfg(unix)]
#[test]
fn fifo_marker_or_alternates_never_hang_and_never_verify() {
    let fx = fixture();
    let clone = fx.root.join("seats").join("aida-seat-aida-fifo");
    create(&fx.main, &clone, "feat", "origin/main").unwrap();
    let clone = clone.canonicalize().unwrap();
    write_lease(&fx.main, &clone, "clone");
    assert_eq!(main_for(&clone), MainFor::Verified(fx.main.clone()));
    let mkfifo = |path: &Path| {
        std::fs::remove_file(path).unwrap();
        assert!(Command::new("mkfifo").arg(path).status().unwrap().success());
    };

    let alternates = clone.join(".git/objects/info/alternates");
    let saved = std::fs::read_to_string(&alternates).unwrap();
    mkfifo(&alternates);
    assert_eq!(main_for(&clone), MainFor::Unverified(clone.clone()));
    std::fs::remove_file(&alternates).unwrap();
    std::fs::write(&alternates, saved).unwrap();
    assert_eq!(main_for(&clone), MainFor::Verified(fx.main.clone()));

    mkfifo(&clone.join(MARKER_RELATIVE));
    assert_eq!(main_for(&clone), MainFor::Unverified(clone.clone()));
}

/// A local-path origin must be copied, not hardlinked: a hardlinked pack
/// would let the seat rewrite origin's objects through its own clone.
#[cfg(unix)]
#[test]
fn clone_objects_are_copies_not_hardlinks() {
    use std::os::unix::fs::MetadataExt;
    let fx = fixture();
    // An origin-only commit, so the clone must fetch a pack from origin.
    let pusher = fx.root.join("pusher");
    git_ok(
        &fx.root,
        &[
            "clone",
            "-q",
            fx.origin.to_str().unwrap(),
            pusher.to_str().unwrap(),
        ],
    );
    git_ok(&pusher, &["config", "user.email", "test@example.com"]);
    git_ok(&pusher, &["config", "user.name", "AIDA Test"]);
    std::fs::write(pusher.join("only-on-origin.txt"), "x\n").unwrap();
    git_ok(&pusher, &["add", "only-on-origin.txt"]);
    git_ok(&pusher, &["commit", "-qm", "origin only"]);
    git_ok(&pusher, &["push", "-q", "origin", "HEAD:refs/heads/side"]);
    git_ok(&fx.origin, &["repack", "-adq"]);
    let dest = fx.root.join("seats").join("aida-seat-aida-links");

    create(&fx.main, &dest, "feat", "origin/main").unwrap();

    let mut files = Vec::new();
    let mut stack = vec![dest.join(".git/objects")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                files.push((path, meta.nlink()));
            }
        }
    }
    assert!(
        files
            .iter()
            .any(|(p, _)| p.extension().is_some_and(|e| e == "pack")),
        "the clone must have fetched a pack, or the check is vacuous: {files:?}"
    );
    let linked: Vec<_> = files.iter().filter(|(_, n)| *n != 1).collect();
    assert!(linked.is_empty(), "hardlinked object files: {linked:?}");
}

#[test]
fn host_git_builders_refuse_a_seat_clone() {
    let fx = fixture();
    let clone = fx.root.join("seats").join("aida-seat-aida-host");
    create(&fx.main, &clone, "feat", "origin/main").unwrap();
    let canary = fx.root.join("fsmonitor-ran");
    git_ok(
        &clone,
        &[
            "config",
            "core.fsmonitor",
            &format!("touch {}; false", canary.display()),
        ],
    );

    let dirty = crate::status_sections::worktree_dirty_entries(&clone);
    assert_eq!(dirty.len(), 1, "{dirty:?}");
    assert!(dirty[0].contains("private clone"), "{dirty:?}");

    let out = aida_core::git_ops::host_git_command(&clone)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("private clone"));
    assert!(
        !canary.exists(),
        "seat-written core.fsmonitor ran on the host"
    );

    // Ordinary trees are unaffected.
    assert!(crate::status_sections::worktree_dirty_entries(&fx.main).is_empty());
}

/// Condition 1: run the host-side paths S1 adds against a seat clone with a
/// logging `git` first on PATH, and require that every git invocation whose
/// repository or working directory is inside the clone came from this module.
/// The negative control proves the check flags an untagged invocation.
#[cfg(unix)]
#[test]
fn no_git_runs_in_a_seat_clone_outside_the_accessor_module() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = fixture();
    let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git on PATH")
        .display()
        .to_string();
    let shim_dir = fx.root.join("shim");
    std::fs::create_dir_all(&shim_dir).unwrap();
    let log = fx.root.join("git.log");
    let shim = shim_dir.join("git");
    crate::test_exec::write_executable(
        &shim,
        format!(
            "#!/bin/sh\n{{ printf '%s\\t%s\\t' \"${{{ACCESSOR_ENV}:-0}}\" \"$PWD\"; \
             for a in \"$@\"; do printf '%s\\037' \"$a\"; done; printf '\\n'; }} >> '{}'\n\
             exec '{real_git}' \"$@\"\n",
            log.display()
        ),
    );
    let clone = fx.root.join("seats").join("aida-seat-aida-lint");

    let _path = crate::test_env::EnvVarGuard::prepend_path(&shim_dir);
    create(&fx.main, &clone, "feat", "origin/main").unwrap();
    let clone = clone.canonicalize().unwrap();
    // Unverified, then verified: neither may ask git inside the clone.
    let _ = crate::worktree_gc::main_worktree_root_from(&clone);
    let _ = crate::session_end::resolve_session_end_root(&clone);
    write_lease(&fx.main, &clone, "clone");
    assert_eq!(crate::worktree_gc::main_worktree_root_from(&clone), fx.main);
    let _ = crate::session_end::resolve_session_end_root(&clone);
    let _ = crate::worktree_gc::main_worktree_root_from(&clone.join(".aida"));
    // The host git builders refuse without spawning git in the clone.
    let _ = crate::status_sections::worktree_dirty_entries(&clone);
    let _ = aida_core::git_ops::host_git_command(&clone)
        .args(["status", "--porcelain"])
        .output();
    let before_control = std::fs::read_to_string(&log).unwrap();
    // Negative control: an untagged host git in the clone.
    let _ = crate::test_git::git_command()
        .arg("-C")
        .arg(&clone)
        .args(["status", "--porcelain"])
        .output();
    let after_control = std::fs::read_to_string(&log).unwrap();

    assert!(
        clone_invocations(&before_control, &clone)
            .iter()
            .any(|(tagged, _)| *tagged),
        "the accessor's own git must reach the shim:\n{before_control}"
    );
    let violations: Vec<String> = clone_invocations(&before_control, &clone)
        .into_iter()
        .filter(|(tagged, _)| !tagged)
        .map(|(_, line)| line)
        .collect();
    assert!(
        violations.is_empty(),
        "git ran in a seat clone outside seat_clone: {violations:#?}"
    );
    assert!(
        clone_invocations(&after_control, &clone)
            .iter()
            .any(|(tagged, _)| !tagged),
        "the check must flag an untagged git in the clone:\n{after_control}"
    );
}

/// The logged git invocations whose effective directory is inside `clone`:
/// `(tagged_by_accessor, raw line)`. The effective directory starts at the
/// logged `$PWD` and follows every `-C`, `--git-dir` and `--work-tree`.
pub(super) fn clone_invocations(log: &str, clone: &Path) -> Vec<(bool, String)> {
    let mut hits = Vec::new();
    for line in log.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(tag), Some(pwd), Some(args)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let args: Vec<&str> = args.split('\x1f').filter(|a| !a.is_empty()).collect();
        let mut dirs = vec![PathBuf::from(pwd)];
        let mut cwd = PathBuf::from(pwd);
        let mut i = 0;
        while i < args.len() {
            let arg = args[i];
            if arg == "-C" && i + 1 < args.len() {
                cwd = cwd.join(args[i + 1]);
                dirs.push(cwd.clone());
                i += 2;
                continue;
            }
            // `-c <key=value>` (test fixtures and store transactions pass
            // these first) takes a value that is not the subcommand.
            // trace:BUG-2191 | ai:claude
            if (arg == "-c" || arg == "--config-env") && i + 1 < args.len() {
                i += 2;
                continue;
            }
            for flag in ["--git-dir=", "--work-tree="] {
                if let Some(p) = arg.strip_prefix(flag) {
                    dirs.push(cwd.join(p));
                }
            }
            if !arg.starts_with('-') {
                break; // the subcommand: its own arguments are not locations
            }
            i += 1;
        }
        let inside = dirs.iter().any(|d| {
            d.canonicalize()
                .unwrap_or_else(|_| d.clone())
                .starts_with(clone)
        });
        if inside {
            hits.push((tag == "1", line.to_string()));
        }
    }
    hits
}

/// End to end: a contained seat under the clone knob gets a private clone and
/// a `clone` lease; an uncontained one with the same config keeps a worktree.
#[test]
fn session_start_gives_a_contained_seat_a_private_clone() {
    let fx = fixture();
    let seats = fx.root.join("seats");
    std::fs::create_dir_all(fx.main.join(".aida")).unwrap();
    std::fs::write(
        fx.main.join(".aida/config.toml"),
        format!(
            "[contained]\nseat_tree = \"clone\"\n\n[worktree_pool]\nworktree_parent = {}\n",
            aida_core::toml_quote::toml_string(&seats.display().to_string())
        ),
    )
    .unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);

    crate::session_start::session_start(
        "TASK-2169",
        None,
        None,
        false,
        None,
        None,
        "auto",
        false,
        None,
        false,
        None,
        None,
        false,
        None,
        false,
        Some(false),
        /* self_claim */ true,
        /* review_coexistence */ None,
        /* contained_seat */ true,
    )
    .unwrap();

    let leases = crate::list_leases(&fx.main);
    let lease = leases
        .iter()
        .find(|l| l.scope == "TASK-2169")
        .expect("lease");
    assert_eq!(lease.tree_kind, TreeKind::Clone);
    assert!(
        lease.worktree_path.starts_with(&seats),
        "{}",
        lease.worktree_path.display()
    );
    let dir = lease.worktree_path.file_name().unwrap().to_str().unwrap();
    assert!(dir.starts_with("aida-seat-aida-"), "{dir}");
    assert!(lease.worktree_path.join(".git").is_dir());
    assert!(ref_exists(&fx.main, &format!("{PIN_PREFIX}{dir}")));
    assert_eq!(
        crate::worktree_gc::main_worktree_root_from(&lease.worktree_path),
        fx.main
    );
    assert!(!git_ok(&fx.main, &["worktree", "list", "--porcelain"]).contains(dir));

    crate::session_start::session_start(
        "TASK-2170",
        None,
        None,
        false,
        None,
        None,
        "auto",
        false,
        None,
        false,
        None,
        None,
        false,
        None,
        false,
        Some(false),
        /* self_claim */ true,
        /* review_coexistence */ None,
        /* contained_seat */ false,
    )
    .unwrap();
    let leases = crate::list_leases(&fx.main);
    let lease = leases
        .iter()
        .find(|l| l.scope == "TASK-2170")
        .expect("lease");
    assert_eq!(lease.tree_kind, TreeKind::Worktree);
    assert!(git_ok(&fx.main, &["worktree", "list", "--porcelain"])
        .contains(lease.worktree_path.to_str().unwrap()));
}

/// TASK-2053 S4c: with no `seat_tree` knob a contained seat gets a clone,
/// while a contained launch at an explicit `--path` outside the worktree
/// parent keeps the worktree it had before the flip (only an explicit
/// `seat_tree = "clone"` refuses it).
// trace:TASK-2053 | ai:claude
#[test]
fn the_default_is_a_clone_and_an_explicit_path_keeps_a_worktree() {
    let fx = fixture();
    let seats = fx.root.join("seats");
    std::fs::create_dir_all(fx.main.join(".aida")).unwrap();
    std::fs::write(
        fx.main.join(".aida/config.toml"),
        format!(
            "[worktree_pool]\nworktree_parent = {}\n",
            aida_core::toml_quote::toml_string(&seats.display().to_string())
        ),
    )
    .unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let start = |owns: &str, path: Option<&str>| {
        crate::session_start::session_start(
            owns,
            None,
            None,
            false,
            path,
            None,
            "auto",
            false,
            None,
            false,
            None,
            None,
            false,
            None,
            false,
            Some(false),
            /* self_claim */ true,
            /* review_coexistence */ None,
            /* contained_seat */ true,
        )
    };
    let lease_for = |owns: &str| {
        crate::list_leases(&fx.main)
            .into_iter()
            .find(|l| l.scope == owns)
            .expect("lease")
    };

    start("TASK-2172", None).unwrap();
    assert_eq!(lease_for("TASK-2172").tree_kind, TreeKind::Clone);

    let explicit = fx.root.join("hand-picked");
    start("TASK-2173", Some(explicit.to_str().unwrap())).unwrap();
    let lease = lease_for("TASK-2173");
    assert_eq!(lease.tree_kind, TreeKind::Worktree);
    assert!(git_ok(&fx.main, &["worktree", "list", "--porcelain"])
        .contains(lease.worktree_path.to_str().unwrap()));
}

/// O1: when the clone cannot be made the launch refuses, writes no lease and
/// creates no worktree in its place. An explicit `seat_tree = "clone"` with
/// no origin remote refuses up front with the fix (S4c ruling 2); one whose
/// origin cannot be cloned refuses from the clone step.
// trace:TASK-2053 | ai:claude
#[test]
fn session_start_refuses_a_contained_seat_it_cannot_clone_for() {
    for (scope, broken_origin, expected) in [
        (
            "TASK-2171",
            None,
            "add an origin remote or set seat_tree = \"worktree\"",
        ),
        (
            "TASK-2174",
            Some("/nonexistent/aida-origin.git"),
            "never falls back to a shared worktree",
        ),
    ] {
        let fx = fixture();
        git_ok(&fx.main, &["remote", "remove", "origin"]);
        if let Some(url) = broken_origin {
            git_ok(&fx.main, &["remote", "add", "origin", url]);
        }
        git_ok(
            &fx.main,
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
        );
        std::fs::create_dir_all(fx.main.join(".aida")).unwrap();
        std::fs::write(
            fx.main.join(".aida/config.toml"),
            format!(
                "[contained]\nseat_tree = \"clone\"\n\n[worktree_pool]\nworktree_parent = {}\n",
                aida_core::toml_quote::toml_string(&fx.root.join("seats").display().to_string())
            ),
        )
        .unwrap();
        let _ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);

        let err = contained_start(scope, None).unwrap_err();

        let err = format!("{err:#}");
        assert!(err.contains(expected), "{err}");
        assert!(crate::list_leases(&fx.main)
            .iter()
            .all(|l| l.scope != scope));
        let worktrees = git_ok(&fx.main, &["worktree", "list", "--porcelain"]);
        assert_eq!(worktrees.matches("worktree ").count(), 1, "{worktrees}");
    }
}

/// S4c ruling 2: with no `seat_tree` knob and no origin remote, a contained
/// launch gets a worktree (with a note) instead of refusing.
// trace:TASK-2053 | ai:claude
#[test]
fn the_default_without_an_origin_keeps_a_worktree() {
    let fx = fixture();
    git_ok(&fx.main, &["remote", "remove", "origin"]);
    std::fs::create_dir_all(fx.main.join(".aida")).unwrap();
    std::fs::write(
        fx.main.join(".aida/config.toml"),
        format!(
            "[worktree_pool]\nworktree_parent = {}\n",
            aida_core::toml_quote::toml_string(&fx.root.join("seats").display().to_string())
        ),
    )
    .unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);

    contained_start("TASK-2175", None).unwrap();

    let lease = crate::list_leases(&fx.main)
        .into_iter()
        .find(|l| l.scope == "TASK-2175")
        .expect("lease");
    assert_eq!(lease.tree_kind, TreeKind::Worktree);
    assert!(git_ok(&fx.main, &["worktree", "list", "--porcelain"])
        .contains(lease.worktree_path.to_str().unwrap()));
}

/// S4c rulings 1 and 2: each fallback with no knob names its reason; an
/// explicit `clone` never falls back, and refuses only a missing origin here.
// trace:TASK-2053 | ai:claude
#[test]
fn default_clone_fallback_notes_each_reason_and_explicit_never_downgrades() {
    use crate::seat_clone::{default_clone_fallback, CloneLaunch};
    let launch = |review, reentry, path_not_seat_location, has_origin| CloneLaunch {
        review,
        reentry,
        path_not_seat_location,
        has_origin,
    };
    let plain = launch(false, false, false, true);
    assert_eq!(
        default_clone_fallback(TreeKind::Clone, false, &plain).unwrap(),
        (TreeKind::Clone, None)
    );
    for (case, why) in [
        (launch(false, false, false, false), "no `origin` remote"),
        (launch(true, false, false, true), "a review session"),
        (
            launch(false, true, false, true),
            "re-enters an existing worktree",
        ),
        (
            launch(false, false, true, true),
            "not an `aida-seat-*` directory directly in the worktree parent",
        ),
    ] {
        let (kind, note) = default_clone_fallback(TreeKind::Clone, false, &case).unwrap();
        assert_eq!(kind, TreeKind::Worktree);
        let note = note.expect("a fallback is never silent");
        assert!(note.contains(why), "{note}");
        // An explicit clone keeps the clone (the later checks refuse the
        // rest) or refuses a missing origin with the fix.
        match default_clone_fallback(TreeKind::Clone, true, &case) {
            Ok(kept) => assert_eq!(kept, (TreeKind::Clone, None)),
            Err(err) => {
                assert!(!case.has_origin);
                assert!(err.to_string().contains("add an origin remote"), "{err}");
            }
        }
    }
    let worktree = launch(true, true, true, false);
    for explicit in [false, true] {
        assert_eq!(
            default_clone_fallback(TreeKind::Worktree, explicit, &worktree).unwrap(),
            (TreeKind::Worktree, None)
        );
    }
}

/// Review item 2: an explicit --path INSIDE the worktree parent that is not
/// an `aida-seat-*` directory also keeps a worktree with no knob set; the
/// note names that condition, not "outside the parent".
// trace:TASK-2053 | ai:claude
#[test]
fn the_default_keeps_a_worktree_for_an_in_parent_path_that_is_not_a_seat_dir() {
    let fx = fixture();
    let seats = fx.root.join("seats");
    std::fs::create_dir_all(&seats).unwrap();
    std::fs::create_dir_all(fx.main.join(".aida")).unwrap();
    std::fs::write(
        fx.main.join(".aida/config.toml"),
        format!(
            "[worktree_pool]\nworktree_parent = {}\n",
            aida_core::toml_quote::toml_string(&seats.display().to_string())
        ),
    )
    .unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let in_parent = seats.join("foo");
    assert!(!crate::seat_clone::is_seat_clone_location(
        &fx.main, &in_parent
    ));

    contained_start("TASK-2176", Some(in_parent.to_str().unwrap())).unwrap();

    let lease = crate::list_leases(&fx.main)
        .into_iter()
        .find(|l| l.scope == "TASK-2176")
        .expect("lease");
    assert_eq!(lease.tree_kind, TreeKind::Worktree);
    assert!(git_ok(&fx.main, &["worktree", "list", "--porcelain"])
        .contains(lease.worktree_path.to_str().unwrap()));
}

/// Review item 1: with an explicit `seat_tree = "clone"`, a review session,
/// a --path re-entry and an explicit --path that is not a seat-clone
/// location each refuse from session start, name the reason, and leave no
/// lease, new worktree or clone behind.
// trace:TASK-2053 | ai:claude
#[test]
fn an_explicit_clone_refuses_review_reentry_and_a_non_seat_path() {
    let fx = fixture();
    let seats = fx.root.join("seats");
    std::fs::create_dir_all(&seats).unwrap();
    std::fs::create_dir_all(fx.main.join(".aida")).unwrap();
    std::fs::write(
        fx.main.join(".aida/config.toml"),
        format!(
            "[contained]\nseat_tree = \"clone\"\n\n[worktree_pool]\nworktree_parent = {}\n",
            aida_core::toml_quote::toml_string(&seats.display().to_string())
        ),
    )
    .unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    // An existing registered worktree to re-enter.
    let existing = fx.root.join("existing");
    git_ok(
        &fx.main,
        &[
            "worktree",
            "add",
            "-b",
            "task-2178",
            existing.to_str().unwrap(),
        ],
    );
    // Worktree paths only: session start may move main's HEAD (its store write).
    let worktree_paths = || {
        git_ok(&fx.main, &["worktree", "list", "--porcelain"])
            .lines()
            .filter(|l| l.starts_with("worktree "))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let worktrees_before = worktree_paths();
    let hand_picked = fx.root.join("hand-picked");
    assert_eq!(std::fs::read_dir(&seats).unwrap().count(), 0);
    let start = |owns: &str, path: Option<&std::path::Path>, force_claim: bool| {
        crate::session_start::session_start(
            owns,
            None,
            None,
            false,
            path.map(|p| p.to_str().unwrap()),
            None,
            "auto",
            false,
            None,
            false,
            None,
            None,
            false,
            None,
            force_claim,
            Some(false),
            /* self_claim */ true,
            /* review_coexistence */ None,
            /* contained_seat */ true,
        )
    };

    for (owns, path, force_claim, expected) in [
        ("PR-7", None, false, "cannot start a review session yet"),
        (
            "TASK-2178",
            Some(existing.as_path()),
            true,
            "cannot re-enter an existing --path yet",
        ),
        (
            "TASK-2179",
            Some(hand_picked.as_path()),
            false,
            "is not one. Drop --path",
        ),
    ] {
        let err = format!("{:#}", start(owns, path, force_claim).unwrap_err());
        assert!(err.contains(expected), "{owns}: {err}");
        assert!(
            crate::list_leases(&fx.main).iter().all(|l| l.scope != owns),
            "{owns} left a lease"
        );
        assert_eq!(
            worktree_paths(),
            worktrees_before,
            "{owns} changed the worktrees"
        );
        assert_eq!(
            std::fs::read_dir(&seats).unwrap().count(),
            0,
            "{owns} left a clone"
        );
    }
    assert!(!hand_picked.exists());
}

/// A contained `session start` owning `owns`, optionally at `path`.
fn contained_start(owns: &str, path: Option<&str>) -> anyhow::Result<()> {
    crate::session_start::session_start(
        owns,
        None,
        None,
        false,
        path,
        None,
        "auto",
        false,
        None,
        false,
        None,
        None,
        false,
        None,
        false,
        Some(false),
        /* self_claim */ true,
        /* review_coexistence */ None,
        /* contained_seat */ true,
    )
}

/// A failure after the clone exists but before the lease is written removes
/// the clone and releases its pin, so the next launch can reuse the name.
#[test]
fn a_failure_after_create_removes_the_clone_and_releases_the_pin() {
    let fx = fixture();
    let seats = fx.root.join("seats");
    std::fs::create_dir_all(fx.main.join(".aida")).unwrap();
    std::fs::write(
        fx.main.join(".aida/config.toml"),
        format!(
            "[contained]\nseat_tree = \"clone\"\n\n[worktree_pool]\nworktree_parent = {}\n",
            aida_core::toml_quote::toml_string(&seats.display().to_string())
        ),
    )
    .unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    failpoint::fail_before_lease(true);

    let err = crate::session_start::session_start(
        "TASK-2172",
        None,
        None,
        false,
        None,
        None,
        "auto",
        false,
        None,
        false,
        None,
        None,
        false,
        None,
        false,
        Some(false),
        /* self_claim */ true,
        /* review_coexistence */ None,
        /* contained_seat */ true,
    )
    .unwrap_err();

    assert!(format!("{err:#}").contains("injected failure"), "{err:#}");
    let left: Vec<_> = std::fs::read_dir(&seats)
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "the clone was left behind: {left:?}");
    assert!(git_ok(&fx.main, &["for-each-ref", PIN_PREFIX]).is_empty());
    assert!(crate::list_leases(&fx.main)
        .iter()
        .all(|l| l.scope != "TASK-2172"));

    crate::session_start::session_start(
        "TASK-2172",
        None,
        None,
        false,
        None,
        None,
        "auto",
        false,
        None,
        false,
        None,
        None,
        false,
        None,
        false,
        Some(false),
        /* self_claim */ true,
        /* review_coexistence */ None,
        /* contained_seat */ true,
    )
    .unwrap();
    let leases = crate::list_leases(&fx.main);
    let lease = leases
        .iter()
        .find(|l| l.scope == "TASK-2172")
        .expect("lease");
    assert!(lease.worktree_path.exists());
    assert_eq!(
        git_ok(
            &fx.main,
            &["for-each-ref", "--format=%(refname)", PIN_PREFIX]
        )
        .lines()
        .count(),
        1
    );
}

/// Plant seat-written execution hooks in the clone's git config: a
/// `core.fsmonitor` and a `filter.<x>.clean` bound by `.gitattributes`. Each
/// touches its canary if host git ever runs them.
fn plant_canaries(clone: &Path, fsmonitor: &Path, filter: &Path) {
    git_ok(
        clone,
        &[
            "config",
            "core.fsmonitor",
            &format!("touch {}; false", fsmonitor.display()),
        ],
    );
    git_ok(
        clone,
        &[
            "config",
            "filter.evil.clean",
            &format!("touch {}; cat", filter.display()),
        ],
    );
    std::fs::write(clone.join(".gitattributes"), "* filter=evil\n").unwrap();
    std::fs::write(clone.join("dirty.txt"), "x\n").unwrap();
}

/// Run the host builders against `clone` and require that both refuse and
/// neither canary fires.
fn assert_host_git_refused(clone: &Path, fsmonitor: &Path, filter: &Path) {
    assert!(
        matches!(main_for(clone), MainFor::Unverified(_)),
        "a rewired .git must still be a seat clone and must not verify"
    );
    assert_host_builders_refuse(clone, fsmonitor, filter);
}

/// Both host git builders refuse `dir`, and neither canary fires.
fn assert_host_builders_refuse(clone: &Path, fsmonitor: &Path, filter: &Path) {
    let dirty = crate::status_sections::worktree_dirty_entries(clone);
    assert_eq!(dirty.len(), 1, "{dirty:?}");
    assert!(dirty[0].contains("private clone"), "{dirty:?}");
    for args in [
        &["status", "--porcelain"][..],
        &["add", "-A"][..],
        &["diff"][..],
    ] {
        let out = aida_core::git_ops::host_git_command(clone)
            .args(args)
            .output()
            .unwrap();
        assert!(!out.status.success(), "host git {args:?} ran in the clone");
    }
    assert!(!fsmonitor.exists(), "seat core.fsmonitor ran on the host");
    assert!(!filter.exists(), "seat filter.clean ran on the host");
}

/// A seat replaces its `.git` directory with a symlink to a renamed copy.
#[cfg(unix)]
#[test]
fn a_symlinked_git_dir_is_still_refused() {
    let fx = fixture();
    let clone = fx.root.join("seats").join("aida-seat-aida-symlink");
    create(&fx.main, &clone, "feat", "origin/main").unwrap();
    write_lease(&fx.main, &clone.canonicalize().unwrap(), "clone");
    let (fsmonitor, filter) = (fx.root.join("fsmonitor-ran"), fx.root.join("filter-ran"));
    plant_canaries(&clone, &fsmonitor, &filter);
    std::fs::rename(clone.join(".git"), clone.join(".gitx")).unwrap();
    std::os::unix::fs::symlink(".gitx", clone.join(".git")).unwrap();
    std::fs::remove_file(clone.join(MARKER_RELATIVE)).unwrap();

    assert_host_git_refused(&clone, &fsmonitor, &filter);
    // Control: the canaries are live, so the refusal is what kept them quiet.
    let _ = crate::test_git::git_command()
        .arg("-C")
        .arg(&clone)
        .args(["add", "-A"])
        .output();
    assert!(filter.exists(), "control: the planted filter should fire");
}

/// A seat replaces its `.git` directory with a `gitdir:` file.
#[test]
fn a_gitdir_file_is_still_refused() {
    let fx = fixture();
    let clone = fx.root.join("seats").join("aida-seat-aida-gitfile");
    create(&fx.main, &clone, "feat", "origin/main").unwrap();
    write_lease(&fx.main, &clone.canonicalize().unwrap(), "clone");
    let (fsmonitor, filter) = (fx.root.join("fsmonitor-ran"), fx.root.join("filter-ran"));
    plant_canaries(&clone, &fsmonitor, &filter);
    std::fs::rename(clone.join(".git"), clone.join(".gitx")).unwrap();
    std::fs::write(clone.join(".git"), "gitdir: .gitx\n").unwrap();

    assert_host_git_refused(&clone, &fsmonitor, &filter);
    let _ = crate::test_git::git_command()
        .arg("-C")
        .arg(&clone)
        .args(["add", "-A"])
        .output();
    assert!(filter.exists(), "control: the planted filter should fire");
}

/// Plain host git in `dir` with the planted filter: proves the layout is
/// live, so the builders' refusal is what kept the canary quiet.
fn control_filter_fires(dir: &Path, filter: &Path) {
    let _ = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args(["add", "-A"])
        .output();
    assert!(
        filter.exists(),
        "control: the planted filter should fire in {}",
        dir.display()
    );
}

/// Round 3 (a): the seat deletes `.git` and lays its root out as the gitdir
/// itself (`core.bare = false`, `core.worktree = <root>`).
#[test]
fn a_root_laid_out_as_a_gitdir_is_still_refused() {
    let fx = fixture();
    let clone = fx.root.join("seats").join("aida-seat-aida-flat");
    create(&fx.main, &clone, "feat", "origin/main").unwrap();
    let clone = clone.canonicalize().unwrap();
    write_lease(&fx.main, &clone, "clone");
    let (fsmonitor, filter) = (fx.root.join("fsmonitor-ran"), fx.root.join("filter-ran"));
    plant_canaries(&clone, &fsmonitor, &filter);
    for entry in std::fs::read_dir(clone.join(".git")).unwrap().flatten() {
        std::fs::rename(entry.path(), clone.join(entry.file_name())).unwrap();
    }
    std::fs::remove_dir(clone.join(".git")).unwrap();
    std::fs::remove_file(clone.join(MARKER_RELATIVE)).unwrap();
    let config = clone.join("config");
    for (key, value) in [
        ("core.bare", "false"),
        ("core.worktree", clone.to_str().unwrap()),
    ] {
        let ok = crate::test_git::git_command()
            .args(["config", "--file"])
            .arg(&config)
            .args([key, value])
            .status()
            .unwrap()
            .success();
        assert!(ok);
    }

    assert_host_git_refused(&clone, &fsmonitor, &filter);
    control_filter_fires(&clone, &filter);
}

/// Round 3 (b): a nested `sub/.git` gitfile points at the clone's gitdir and
/// host git runs from `sub`.
#[test]
fn a_nested_gitfile_inside_a_clone_is_still_refused() {
    let fx = fixture();
    let clone = fx.root.join("seats").join("aida-seat-aida-nested");
    create(&fx.main, &clone, "feat", "origin/main").unwrap();
    let clone = clone.canonicalize().unwrap();
    write_lease(&fx.main, &clone, "clone");
    let (fsmonitor, filter) = (fx.root.join("fsmonitor-ran"), fx.root.join("filter-ran"));
    plant_canaries(&clone, &fsmonitor, &filter);
    let sub = clone.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(
        sub.join(".git"),
        format!("gitdir: {}\n", clone.join(".git").display()),
    )
    .unwrap();
    std::fs::write(sub.join(".gitattributes"), "* filter=evil\n").unwrap();
    std::fs::write(sub.join("payload.txt"), "x\n").unwrap();

    assert_eq!(seat_clone_root(&sub), Some(clone.clone()));
    assert_host_builders_refuse(&sub, &fsmonitor, &filter);
    control_filter_fires(&sub, &filter);
}

/// Round 3 (c): a clone under a name the prefix test misses and with its
/// marker deleted is still caught by the host-side lease, and only by it.
#[test]
fn a_renamed_root_without_a_marker_is_caught_by_the_host_lease() {
    let fx = fixture();
    let made = fx.root.join("seats").join("aida-seat-aida-renamed");
    create(&fx.main, &made, "feat", "origin/main").unwrap();
    let (fsmonitor, filter) = (fx.root.join("fsmonitor-ran"), fx.root.join("filter-ran"));
    plant_canaries(&made, &fsmonitor, &filter);
    let renamed = fx.root.join("seats").join("renamed");
    std::fs::rename(&made, &renamed).unwrap();
    std::fs::remove_file(renamed.join(MARKER_RELATIVE)).unwrap();
    let renamed = renamed.canonicalize().unwrap();
    aida_core::seat_clone_detect::note_host_main(&fx.main);
    assert_eq!(
        seat_clone_root(&renamed),
        None,
        "without a lease nothing marks it"
    );

    write_lease(&fx.main, &renamed, "clone");

    assert_eq!(seat_clone_root(&renamed), Some(renamed.clone()));
    assert_eq!(
        seat_clone_root(&renamed.join(".aida")),
        Some(renamed.clone())
    );
    assert_host_git_refused(&renamed, &fsmonitor, &filter);
    control_filter_fires(&renamed, &filter);
}

/// The default launches offline from local refs, cleans the failed clone/pin,
/// and records a worktree lease at the originally selected automatic path.
/// An explicit reserved clone path refuses before creating a worktree.
// trace:TASK-2300 | ai:codex
#[test]
fn task_2300_default_origin_failure_falls_back_and_cleans_clone_resources() {
    for explicit_path in [false, true] {
        let fx = fixture();
        git_ok(
            &fx.main,
            &[
                "remote",
                "set-url",
                "origin",
                "/nonexistent/aida-offline-origin.git",
            ],
        );
        let seats = fx.root.join("seats");
        std::fs::create_dir_all(&seats).unwrap();
        std::fs::create_dir_all(fx.main.join(".aida")).unwrap();
        std::fs::write(
            fx.main.join(".aida/config.toml"),
            format!(
                "[worktree_pool]\nworktree_parent = {}\n",
                aida_core::toml_quote::toml_string(&seats.display().to_string())
            ),
        )
        .unwrap();
        let _ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
        let requested = seats.join("aida-seat-offline-explicit");
        assert!(is_seat_clone_location(&fx.main, &requested));
        let started = contained_start(
            "TASK-2300",
            explicit_path.then(|| requested.to_str().unwrap()),
        );
        if explicit_path {
            let error = started.unwrap_err().to_string();
            assert!(
                error.contains("reserved") && error.contains("drop --path"),
                "{error}"
            );
            assert!(!requested.exists());
            assert!(crate::list_leases(&fx.main).is_empty());
            assert!(git_ok(
                &fx.main,
                &["for-each-ref", "--format=%(refname)", PIN_PREFIX]
            )
            .is_empty());
            assert_eq!(std::fs::read_dir(&seats).unwrap().count(), 0);
            assert_eq!(
                git_ok(&fx.main, &["worktree", "list", "--porcelain"])
                    .matches("worktree ")
                    .count(),
                1
            );
            continue;
        }
        started.unwrap();

        let lease = crate::list_leases(&fx.main)
            .into_iter()
            .find(|lease| lease.scope == "TASK-2300")
            .expect("fallback lease");
        assert_eq!(lease.tree_kind, TreeKind::Worktree);
        assert!(lease.worktree_path.join(".git").is_file());
        assert!(!lease.worktree_path.join(MARKER_RELATIVE).exists());
        let worktrees = git_ok(&fx.main, &["worktree", "list", "--porcelain"]);
        assert_eq!(worktrees.matches("worktree ").count(), 2, "{worktrees}");
        assert!(git_ok(
            &fx.main,
            &["for-each-ref", "--format=%(refname)", PIN_PREFIX]
        )
        .is_empty());
        assert_eq!(std::fs::read_dir(&seats).unwrap().count(), 1);
    }
}

/// Both folded findings are the same regression: failed origin cloning has
/// a visible note only for the default; an explicit clone keeps its refusal.
// trace:TASK-2300 | ai:codex
#[test]
fn task_2300_origin_failure_note_and_explicit_clone_policy() {
    let fx = fixture();
    git_ok(
        &fx.main,
        &[
            "remote",
            "set-url",
            "origin",
            "/nonexistent/aida-offline-origin.git",
        ],
    );
    let dest = fx.root.join("aida-seat-offline");
    let error = create(&fx.main, &dest, "offline", "main").unwrap_err();
    let note = origin_failure_fallback(false, &error).expect("default fallback note");
    assert!(
        note.contains("linked worktree") && note.contains("`origin`"),
        "{note}"
    );
    assert!(note.contains("credentials"), "{note}");
    assert!(origin_failure_fallback(true, &error).is_none());
    assert!(!dest.exists());
    assert!(!ref_exists(&fx.main, &pin_ref(&dest).unwrap()));
}

/// A valid origin followed by a checkout error is not an origin outage;
/// likewise, a bad local base or occupied destination must never downgrade.
// trace:TASK-2300 | ai:codex
#[test]
fn task_2300_local_clone_setup_errors_do_not_fall_back() {
    let fx = fixture();
    for (branch, base) in [("bad..branch", "main"), ("valid", "missing-base")] {
        let dest = fx.root.join("aida-seat-local-error");
        let error = create(&fx.main, &dest, branch, base).unwrap_err();
        assert!(
            origin_failure_fallback(false, &error).is_none(),
            "{error:#}"
        );
        assert!(!dest.exists());
        assert!(!ref_exists(&fx.main, &pin_ref(&dest).unwrap()));
    }
    let dest = fx.root.join("aida-seat-occupied");
    std::fs::create_dir(&dest).unwrap();
    std::fs::write(dest.join("keep"), "operator data").unwrap();
    let error = create(&fx.main, &dest, "valid", "main").unwrap_err();
    assert!(origin_failure_fallback(false, &error).is_none());
    assert_eq!(
        std::fs::read_to_string(dest.join("keep")).unwrap(),
        "operator data"
    );
}

/// A stalled transport is bounded and selects the same default-only policy;
/// inability to spawn the executable is a local error, not an outage.
// trace:TASK-2300 | ai:codex
#[cfg(unix)]
#[test]
fn task_2300_clone_timeout_falls_back_but_spawn_failure_refuses() {
    let mut stall = Command::new("/bin/sh");
    stall.args(["-c", "exec /bin/sleep 30"]);
    let start = std::time::Instant::now();
    let error = run_origin_clone(stall, std::time::Duration::from_millis(30)).unwrap_err();
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    assert!(origin_failure_fallback(false, &error).is_some());
    assert!(origin_failure_fallback(true, &error).is_none());
    let error = run_origin_clone(
        Command::new("/nonexistent/aida-git"),
        std::time::Duration::from_millis(30),
    )
    .unwrap_err();
    assert!(origin_failure_fallback(false, &error).is_none());
}

/// Failed directory removal or pin deletion must erase fallback eligibility,
/// preserving recovery evidence rather than proceeding with a worktree.
// trace:TASK-2300 | ai:codex
#[test]
fn task_2300_failed_cleanup_refuses_fallback() {
    let fx = fixture();
    let dest = fx.root.join("aida-seat-cleanup-failed");
    let pin = pin_ref(&dest).unwrap();
    git_ok(&fx.main, &["update-ref", &pin, "HEAD"]);
    // An unexpected file cannot be removed as a clone directory.
    std::fs::write(&dest, "retain this unexpected allocation").unwrap();
    let error = cleanup_failed_clone(
        &fx.main,
        &dest,
        &pin,
        anyhow::Error::new(OriginCloneFailure("origin unavailable".into())),
    );
    assert!(
        error.to_string().contains("clone cleanup failed"),
        "{error:#}"
    );
    assert!(origin_failure_fallback(false, &error).is_none());
    assert!(dest.is_file());
    assert!(ref_exists(&fx.main, &pin));
    std::fs::remove_file(&dest).unwrap();

    // Git's ref lock makes the second cleanup fail after directory removal.
    std::fs::create_dir(&dest).unwrap();
    let lock = fx.main.join(".git").join(format!("{pin}.lock"));
    std::fs::write(&lock, "held by another writer").unwrap();
    let error = cleanup_failed_clone(
        &fx.main,
        &dest,
        &pin,
        anyhow::Error::new(OriginCloneFailure("origin unavailable".into())),
    );
    assert!(
        error.to_string().contains("base-pin cleanup failed"),
        "{error:#}"
    );
    assert!(origin_failure_fallback(false, &error).is_none());
    assert!(!dest.exists());
    assert!(ref_exists(&fx.main, &pin));
    std::fs::remove_file(lock).unwrap();
    release_pin(&fx.main, &pin).unwrap();
}

/// TASK-2350: host git refuses to run in a seat clone, and the review
/// auto-queue used to read that refusal as "no `origin` remote" and skip as a
/// local-only project. The presence probe answers for a verified clone from
/// its host main, and reports an unverified clone or a failed lookup as an
/// error instead of `false`.
// trace:TASK-2350 | ai:claude
#[test]
fn origin_presence_answers_for_a_seat_clone_and_never_guesses_absent() {
    use crate::review_gates::origin_remote_presence;
    let fx = fixture();
    assert!(origin_remote_presence(&fx.main).unwrap());

    let bare = fx.root.join("no-remote");
    std::fs::create_dir_all(&bare).unwrap();
    git_ok(&bare, &["init", "-q"]);
    assert!(!origin_remote_presence(&bare).unwrap());

    let not_a_repo = fx.root.join("not-a-repo");
    std::fs::create_dir_all(&not_a_repo).unwrap();
    assert!(origin_remote_presence(&not_a_repo).is_err());

    let clone = fx.root.join("seats").join("aida-seat-aida-t2350");
    create(&fx.main, &clone, "feat", "origin/main").unwrap();
    let clone = clone.canonicalize().unwrap();
    assert!(
        !aida_core::git_ops::has_remote(&clone, "origin"),
        "host git is refused in the clone, which is the false negative"
    );
    let err = origin_remote_presence(&clone).unwrap_err().to_string();
    assert!(err.contains("could not be verified"), "{err}");

    write_lease(&fx.main, &clone, "clone");
    assert!(origin_remote_presence(&clone).unwrap());
    assert!(origin_remote_presence(&clone.join(".aida")).unwrap());
}

/// TASK-2357: the auto-queue must turn a failed `origin` lookup into a
/// needs-attention skip, never the dim local-only skip. Covers both failure
/// shapes: a project root that is not a git repo (which used to read as
/// local-only) and an unverified seat clone.
// trace:TASK-2357 | ai:claude
#[test]
fn auto_queue_reports_a_failed_origin_lookup_as_needs_attention() {
    use crate::review_gates::{
        try_auto_queue_pr_review_unbumped, AutoQueueOrigin, AutoQueueStatus,
    };
    let fx = fixture();

    let not_a_repo = fx.root.join("not-a-repo");
    std::fs::create_dir_all(&not_a_repo).unwrap();
    let clone = fx.root.join("seats").join("aida-seat-aida-t2357");
    create(&fx.main, &clone, "feat", "origin/main").unwrap();
    let clone = clone.canonicalize().unwrap();

    for root in [&not_a_repo, &clone] {
        let outcome = try_auto_queue_pr_review_unbumped(
            root,
            "feat",
            "sess-2357",
            AutoQueueOrigin::SessionEnd,
        );
        assert!(
            matches!(outcome.status, AutoQueueStatus::SkippedNeedsAttention),
            "{}: expected needs-attention, got: {}",
            root.display(),
            outcome.summary
        );
        assert!(
            outcome
                .summary
                .contains("could not determine whether an `origin` remote exists"),
            "{}: {}",
            root.display(),
            outcome.summary
        );
    }
}
