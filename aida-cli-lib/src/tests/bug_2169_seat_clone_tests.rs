//! BUG-2169 slice S1: a contained seat's private clone. Creation, the base
//! pin, the loud refusal (no worktree fallback), the lease `tree_kind`, the
//! file-only main resolver, and condition 1 of the advisor's signoff: no git
//! runs with a seat clone as its repository outside this module.
//! trace:BUG-2169 | ai:claude

use super::*;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git_ok(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
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

fn ref_exists(dir: &Path, name: &str) -> bool {
    Command::new("git")
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
struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    main: PathBuf,
    origin: PathBuf,
}

fn fixture() -> Fixture {
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
             worktree_path = \"{}\"\nbranch = \"feat\"\nstarted_at = \"2026-10-16T00:00:00Z\"\n\
             hostname = \"h\"\ntree_kind = \"{kind}\"\n",
            clone.display()
        ),
    )
    .unwrap();
}

#[test]
fn knob_absent_is_worktree_clone_is_clone_and_a_typo_refuses() {
    assert_eq!(parse_seat_tree_knob(None).unwrap(), TreeKind::Worktree);
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
    // The default stays `worktree` (condition 5: no flip before S4).
    let bare = tempfile::tempdir().unwrap();
    assert_eq!(
        tree_kind_for_launch(bare.path(), true).unwrap(),
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

    // No lease yet: the clone is its own root, and git is never asked.
    assert_eq!(main_for(&clone), MainFor::Unverified(clone.clone()));
    assert_eq!(crate::worktree_gc::main_worktree_root_from(&clone), clone);

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
}

/// Condition 1: run the host-side paths S1 adds against a seat clone with a
/// logging `git` first on PATH, and require that every git invocation whose
/// repository or working directory is inside the clone came from this module.
/// The negative control proves the check flags an untagged invocation.
#[test]
fn no_git_runs_in_a_seat_clone_outside_the_accessor_module() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = fixture();
    let real_git = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
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
    let before_control = std::fs::read_to_string(&log).unwrap();
    // Negative control: an untagged host git in the clone.
    let _ = Command::new("git")
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
fn clone_invocations(log: &str, clone: &Path) -> Vec<(bool, String)> {
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
            "[contained]\nseat_tree = \"clone\"\n\n[worktree_pool]\nworktree_parent = \"{}\"\n",
            seats.display()
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

/// O1: when the clone cannot be made the launch refuses, writes no lease and
/// creates no worktree in its place.
#[test]
fn session_start_refuses_a_contained_seat_it_cannot_clone_for() {
    let fx = fixture();
    git_ok(&fx.main, &["remote", "remove", "origin"]);
    git_ok(
        &fx.main,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    std::fs::create_dir_all(fx.main.join(".aida")).unwrap();
    std::fs::write(
        fx.main.join(".aida/config.toml"),
        format!(
            "[contained]\nseat_tree = \"clone\"\n\n[worktree_pool]\nworktree_parent = \"{}\"\n",
            fx.root.join("seats").display()
        ),
    )
    .unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);

    let err = crate::session_start::session_start(
        "TASK-2171",
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
        /* contained_seat */ true,
    )
    .unwrap_err();

    let err = format!("{err:#}");
    assert!(
        err.contains("never falls back to a shared worktree"),
        "{err}"
    );
    assert!(crate::list_leases(&fx.main)
        .iter()
        .all(|l| l.scope != "TASK-2171"));
    let worktrees = git_ok(&fx.main, &["worktree", "list", "--porcelain"]);
    assert_eq!(worktrees.matches("worktree ").count(), 1, "{worktrees}");
}
